//! JPEG, 16-bit TIFF and 8-bit PNG encoding with an ICC profile, EXIF
//! directories and XMP, and JPEG XL with EXIF and XMP.
use super::{Embed, metadata::Directories};
use crate::{
    develop::ExportImage,
    exif::{
        Field,
        tag::{
            EXIF_IFD, GPS_IFD, ICC_PROFILE, INTEROPERABILITY_IFD, RESOLUTION_UNIT, X_RESOLUTION,
            XMP, Y_RESOLUTION,
        },
    },
    export_settings::ExportOptions,
    tiff::kind::{ASCII, FLOAT, LONG, RATIONAL, SHORT, SLONG, SRATIONAL, SSHORT, UNDEFINED},
};
use anyhow::{Result, ensure};
use image::{ImageEncoder, codecs::jpeg::JpegEncoder};
use std::io::{Seek, Write};
use tiff::{
    encoder::{DirectoryEncoder, Rational, SRational, TiffEncoder, TiffKind, colortype::RGB16},
    tags::{ResolutionUnit, Tag},
};

pub(super) fn jpeg(
    out: &mut impl Write,
    image: &ExportImage,
    quality: u8,
    profile: Vec<u8>,
    directories: Directories,
    embed: &Embed,
) -> Result<()> {
    let mut jpeg = Vec::new();
    let mut e = JpegEncoder::new_with_quality(&mut jpeg, quality);
    e.set_icc_profile(profile)?;
    e.set_exif_metadata(super::metadata::jpeg_exif(directories))?;
    e.write_image(
        &image.rgb8(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    if let Some(xmp) = &embed.xmp {
        jpeg = insert_xmp(jpeg, xmp)?;
    }
    out.write_all(&jpeg)?;
    Ok(())
}

pub(super) fn tiff(
    out: &mut (impl Write + Seek),
    image: &ExportImage,
    profile: Vec<u8>,
    directories: &Directories,
    embed: &Embed,
) -> Result<()> {
    tiff_of::<RGB16, _>(
        out,
        [image.width(), image.height()],
        &image.rgb16(),
        profile,
        directories,
        embed,
    )
}

/// An HDR export's TIFF: PQ and HLG as 16-bit signal values, linear light as 32-bit
/// float (1 = graphics white), with `profile` naming the encoding.
pub(super) fn tiff_hdr(
    out: &mut (impl Write + Seek),
    image: &super::HdrPixels,
    profile: Vec<u8>,
    directories: &Directories,
    embed: &Embed,
) -> Result<()> {
    use crate::color::hdr_output::HdrTransfer;
    let size = [image.width, image.height];
    match image.output.transfer {
        HdrTransfer::Pq | HdrTransfer::Hlg => {
            let data: Vec<u16> = image
                .pixels
                .iter()
                .flatten()
                .map(|v| (v.clamp(0., 1.) * 65535. + 0.5) as u16)
                .collect();
            tiff_of::<RGB16, _>(out, size, &data, profile, directories, embed)
        }
        HdrTransfer::Linear => {
            let data: Vec<f32> = image.pixels.iter().flatten().copied().collect();
            tiff_of::<tiff::encoder::colortype::RGB32Float, _>(
                out,
                size,
                &data,
                profile,
                directories,
                embed,
            )
        }
    }
}

fn tiff_of<C, W>(
    out: &mut W,
    [width, height]: [u32; 2],
    data: &[C::Inner],
    profile: Vec<u8>,
    directories: &Directories,
    embed: &Embed,
) -> Result<()>
where
    C: tiff::encoder::colortype::ColorType,
    [C::Inner]: tiff::encoder::TiffValue,
    W: Write + Seek,
{
    let Directories {
        exif: directories,
        interop,
    } = directories;
    let mut e = TiffEncoder::new(out)?;
    let interop_offset = if interop.is_empty() {
        None
    } else {
        let mut dir = e.extra_directory()?;
        for f in interop {
            write_field(&mut dir, f)?;
        }
        Some(dir.finish_with_offsets()?.offset)
    };
    let mut exif = e.extra_directory()?;
    for f in &directories.exif {
        write_field(&mut exif, f)?;
    }
    if let Some(offset) = interop_offset {
        exif.write_tag(Tag::Unknown(INTEROPERABILITY_IFD), offset)?;
    }
    let exif_offset = exif.finish_with_offsets()?.offset;
    let gps_offset = if directories.gps.is_empty() {
        None
    } else {
        let mut gps = e.extra_directory()?;
        for f in &directories.gps {
            write_field(&mut gps, f)?;
        }
        Some(gps.finish_with_offsets()?.offset)
    };
    let mut im = e.new_image::<C>(width, height)?;
    im.encoder()
        .write_tag(Tag::Unknown(EXIF_IFD), exif_offset)?;
    if let Some(offset) = gps_offset {
        im.encoder().write_tag(Tag::Unknown(GPS_IFD), offset)?;
    }
    im.encoder()
        .write_tag(Tag::Unknown(ICC_PROFILE), profile.as_slice())?;
    // The encoder writes its own resolution tags.
    for f in directories
        .main
        .iter()
        .filter(|f| ![X_RESOLUTION, Y_RESOLUTION, RESOLUTION_UNIT].contains(&f.tag))
    {
        write_field(im.encoder(), f)?;
    }
    im.resolution(
        ResolutionUnit::Inch,
        Rational {
            n: embed.ppi.clamp(1, 10_000),
            d: 1,
        },
    );
    if let Some(xmp) = &embed.xmp {
        im.encoder().write_tag(Tag::Unknown(XMP), xmp.as_bytes())?;
    }
    im.write_data(data)?;
    Ok(())
}

/// The VarDCT distance of a JPEG XL export with `options`: `None` when it is
/// lossless.
fn jxl_distance(options: &ExportOptions) -> Option<f32> {
    options
        .jxl_lossy
        .then(|| super::jxl::distance(options.quality))
}

/// An 8-bit sRGB JPEG XL, lossless or lossy as `options` say, its EXIF and XMP in
/// container boxes.
pub(super) fn jxl(
    out: &mut impl Write,
    image: &ExportImage,
    options: &ExportOptions,
    directories: Directories,
    embed: &Embed,
) -> Result<()> {
    ensure!(
        image.space() == crate::color::OutputSpace::Srgb,
        "JPEG XL exports are sRGB"
    );
    let codestream = super::jxl::codestream(
        image.width(),
        image.height(),
        super::jxl::Samples::Eight(&image.rgb8()),
        super::jxl::Colour::Srgb,
        jxl_distance(options),
    )?;
    let exif = super::metadata::png_exif(directories);
    out.write_all(&super::jxl::container(
        &codestream,
        Some(&exif),
        embed.xmp.as_deref(),
        None,
    )?)?;
    Ok(())
}

/// An HDR export's 16-bit JPEG XL: the header naming the primaries and transfer
/// (PQ or HLG), with the intensity target their maximum stands for (10,000 cd/m² for
/// PQ, HLG's 1,000 cd/m² reference display). Lossless, the PQ or HLG signal values;
/// lossy as `options` say, the PQ of the light they stand for
/// ([`super::jxl::lossy_hdr_codestream`]).
pub(super) fn jxl_hdr(
    out: &mut impl Write,
    image: &super::HdrPixels,
    options: &ExportOptions,
    directories: Directories,
    embed: &Embed,
) -> Result<()> {
    use crate::color::hdr_output::{
        GRAPHICS_WHITE_NITS, HLG_PEAK_NITS, HdrGamut, HdrTransfer, hlg_decode, pq_encode,
    };
    let intensity_target = match image.output.transfer {
        HdrTransfer::Pq => 10_000.,
        HdrTransfer::Hlg => HLG_PEAK_NITS,
        HdrTransfer::Linear => anyhow::bail!("Linear HDR is written as TIFF only"),
    };
    let colour = super::jxl::Colour::Hdr {
        // JPEG XL's own primaries enumeration: CICP's but for P3, which it numbers 11.
        primaries: match image.output.gamut {
            HdrGamut::Rec2020 => 9,
            HdrGamut::DisplayP3 => 11,
            HdrGamut::Rec709 => 1,
        },
        // Its transfer functions are numbered as CICP's.
        transfer: image.output.transfer.cicp(),
        intensity_target,
    };
    let sample = |v: f32| (v.clamp(0., 1.) * 65535. + 0.5) as u16;
    let codestream = match jxl_distance(options) {
        None => {
            let samples: Vec<u16> = image.pixels.iter().flatten().map(|v| sample(*v)).collect();
            super::jxl::codestream(
                image.width,
                image.height,
                super::jxl::Samples::Sixteen(&samples),
                colour,
                None,
            )?
        }
        Some(distance) => {
            let weights = image.output.gamut.luminance();
            let pq: Vec<u16> = image
                .pixels
                .iter()
                .flat_map(|p| match image.output.transfer {
                    HdrTransfer::Hlg => {
                        hlg_decode(*p, weights).map(|v| sample(pq_encode(v * GRAPHICS_WHITE_NITS)))
                    }
                    _ => p.map(sample),
                })
                .collect();
            super::jxl::lossy_hdr_codestream(image.width, image.height, &pq, colour, distance)?
        }
    };
    let exif = super::metadata::png_exif(directories);
    out.write_all(&super::jxl::container(
        &codestream,
        Some(&exif),
        embed.xmp.as_deref(),
        None,
    )?)?;
    Ok(())
}

/// A gain-map JPEG XL: the sRGB base as an SDR JPEG XL export is, with the map's
/// grayscale codestream and its ISO 21496-1 metadata in a `jhgm` box. A lossy export
/// codes the map at the base's distance, as a gain-map JPEG's map has its quality.
pub(super) fn jxl_gain_map(
    out: &mut impl Write,
    image: &super::GainMapPixels,
    options: &ExportOptions,
    directories: Directories,
    embed: &Embed,
) -> Result<()> {
    let base = &image.base;
    ensure!(
        base.space() == crate::color::OutputSpace::Srgb,
        "A gain map's base is sRGB"
    );
    let codestream = super::jxl::codestream(
        base.width(),
        base.height(),
        super::jxl::Samples::Eight(&base.rgb8()),
        super::jxl::Colour::Srgb,
        jxl_distance(options),
    )?;
    let map = super::jxl::gray_codestream(
        base.width(),
        base.height(),
        &image.map,
        jxl_distance(options),
    )?;
    let bundle = super::jxl::gain_map_bundle(&image.metadata.iso_21496(), &map)?;
    let exif = super::metadata::png_exif(directories);
    out.write_all(&super::jxl::container(
        &codestream,
        Some(&exif),
        embed.xmp.as_deref(),
        Some(&bundle),
    )?)?;
    Ok(())
}

/// An 8-bit PNG, its EXIF in an eXIf chunk and its XMP in an uncompressed
/// iTXt chunk, as the XMP specification asks.
pub(super) fn png(
    out: &mut impl Write,
    image: &ExportImage,
    profile: Vec<u8>,
    directories: Directories,
    embed: &Embed,
) -> Result<()> {
    let mut info = png::Info::with_size(image.width(), image.height());
    info.color_type = png::ColorType::Rgb;
    info.bit_depth = png::BitDepth::Eight;
    info.icc_profile = Some(profile.into());
    info.exif_metadata = Some(super::metadata::png_exif(directories).into());
    // pHYs counts pixels per metre.
    let ppm = (embed.ppi.clamp(1, 10_000) as f64 / 0.0254).round() as u32;
    info.pixel_dims = Some(png::PixelDimensions {
        xppu: ppm,
        yppu: ppm,
        unit: png::Unit::Meter,
    });
    if let Some(xmp) = &embed.xmp {
        info.utf8_text
            .push(png::text_metadata::ITXtChunk::new("XML:com.adobe.xmp", xmp));
    }
    let mut e = png::Encoder::with_info(out, info)?;
    e.set_compression(png::Compression::High);
    let mut w = e.write_header()?;
    w.write_image_data(&image.rgb8())?;
    w.finish()?;
    Ok(())
}

/// XMP goes in its own APP1 segment, after the EXIF and ICC ones; what does
/// not fit one follows as ExtendedXMP.
pub(super) fn insert_xmp(jpeg: Vec<u8>, xmp: &str) -> Result<Vec<u8>> {
    use super::extended_xmp::{segments, split};
    use crate::{jpeg::Segments, xml::ns::JPEG_HEADER};
    ensure!(jpeg.starts_with(&[0xff, 0xd8]), "Not a JPEG");
    let split = split(xmp)?;
    let mut payloads = vec![[JPEG_HEADER, split.standard.as_bytes()].concat()];
    if let Some((guid, extended)) = &split.extended {
        payloads.extend(segments(guid, extended));
    }
    for payload in &payloads {
        ensure!(
            2 + payload.len() <= 65_535,
            "XMP is too large for a JPEG segment"
        );
    }
    // After the APPn segments the encoder wrote.
    let mut at = 2;
    if let Some(mut s) = Segments::new(std::io::Cursor::new(&jpeg))? {
        while let Some(segment) = s.next_segment()?
            && (0xe0..=0xef).contains(&segment.marker)
        {
            at = segment.offset as usize + segment.length;
        }
    }
    let extra: usize = payloads.iter().map(|p| p.len() + 4).sum();
    let mut out = Vec::with_capacity(jpeg.len() + extra);
    out.extend_from_slice(&jpeg[..at]);
    for payload in &payloads {
        out.extend([0xff, 0xe1]);
        out.extend(((2 + payload.len()) as u16).to_be_bytes());
        out.extend_from_slice(payload);
    }
    out.extend_from_slice(&jpeg[at..]);
    Ok(out)
}

/// Writes one EXIF field into a TIFF directory with its own type. UNDEFINED
/// values keep their type; BYTE and SBYTE values are written as bytes.
fn write_field<W: Write + Seek, K: TiffKind>(
    dir: &mut DirectoryEncoder<'_, W, K>,
    f: &Field,
) -> Result<()> {
    let tag = Tag::Unknown(f.tag);
    let halves = || f.bytes.as_chunks::<2>().0.iter().copied();
    let words = || f.bytes.as_chunks::<4>().0.iter().copied();
    // Numerator and denominator of each rational.
    let pairs = || {
        f.bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| c.as_chunks::<4>().0)
    };
    match f.kind {
        ASCII => dir.write_tag(tag, CameraText::new(&f.bytes))?,
        SHORT => dir.write_tag(
            tag,
            halves()
                .map(u16::from_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        )?,
        SSHORT => dir.write_tag(
            tag,
            halves()
                .map(i16::from_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        )?,
        LONG => dir.write_tag(
            tag,
            words()
                .map(u32::from_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        )?,
        SLONG => dir.write_tag(
            tag,
            words()
                .map(i32::from_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        )?,
        FLOAT => dir.write_tag(
            tag,
            words()
                .map(f32::from_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        )?,
        RATIONAL => {
            let v: Vec<Rational> = pairs()
                .map(|p| Rational {
                    n: u32::from_le_bytes(p[0]),
                    d: u32::from_le_bytes(p[1]),
                })
                .collect();
            write_array(dir, tag, v)?
        }
        SRATIONAL => {
            let v: Vec<SRational> = pairs()
                .map(|p| SRational {
                    n: i32::from_le_bytes(p[0]),
                    d: i32::from_le_bytes(p[1]),
                })
                .collect();
            write_array(dir, tag, v)?
        }
        UNDEFINED => dir.write_tag(tag, Undefined(&f.bytes))?,
        _ => dir.write_tag(tag, f.bytes.as_slice())?,
    }
    Ok(())
}

/// An ASCII value as the camera wrote it, ending in one NUL. The encoder's own
/// strings refuse an inner NUL and bytes past ASCII, but EXIF's Copyright holds
/// two strings separated by NUL (photographer, editor), and cameras and
/// catalogs write names in UTF-8, which JPEG exports keep too.
struct CameraText(Vec<u8>);
impl CameraText {
    fn new(bytes: &[u8]) -> Self {
        let end = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
        Self([&bytes[..end], &[0]].concat())
    }
}
impl tiff::encoder::TiffValue for CameraText {
    const BYTE_LEN: u8 = 1;
    const FIELD_TYPE: tiff::tags::Type = tiff::tags::Type::ASCII;
    fn count(&self) -> usize {
        self.0.len()
    }
    fn data(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Borrowed(&self.0)
    }
}

/// An UNDEFINED value (ExifVersion, InteroperabilityVersion…), which the encoder
/// would write as BYTE from a plain byte slice.
struct Undefined<'a>(&'a [u8]);
impl tiff::encoder::TiffValue for Undefined<'_> {
    const BYTE_LEN: u8 = 1;
    const FIELD_TYPE: tiff::tags::Type = tiff::tags::Type::UNDEFINED;
    fn count(&self) -> usize {
        self.0.len()
    }
    fn data(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Borrowed(self.0)
    }
}

/// The encoder takes rationals only as fixed-size arrays; EXIF uses up to four
/// (LensInfo), GPS three (latitude, longitude, time).
fn write_array<W: Write + Seek, K: TiffKind, T>(
    dir: &mut DirectoryEncoder<'_, W, K>,
    tag: Tag,
    v: Vec<T>,
) -> Result<()>
where
    [T]: tiff::encoder::TiffValue,
{
    fn take<T, const N: usize>(v: Vec<T>) -> Option<[T; N]> {
        v.try_into().ok()
    }
    match v.len() {
        1 => dir.write_tag(tag, take::<T, 1>(v).unwrap())?,
        2 => dir.write_tag(tag, take::<T, 2>(v).unwrap())?,
        3 => dir.write_tag(tag, take::<T, 3>(v).unwrap())?,
        4 => dir.write_tag(tag, take::<T, 4>(v).unwrap())?,
        _ => {}
    }
    Ok(())
}
