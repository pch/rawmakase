//! Opening a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL, the files RAWmakase shows without developing.
use anyhow::{Context, Result, anyhow};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, RgbImage};
use moxcms::{
    ColorProfile, DataColorSpace, Layout, ToneReprCurve, TransformExecutor, TransformOptions,
};
use rawmakase_cancel::Cancel;
use rawmakase_model::camera_data::{CameraImage, Metadata, Source};
use rayon::prelude::*;
use std::path::Path;

mod av1;
mod avif;
mod heic;
mod heif;
mod hevc;
mod jxl;

/// What a file's first bytes say it is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Avif,
    Heic,
    Jxl,
    Webp { animated: bool },
    Other,
}

fn sniff(head: &[u8]) -> Kind {
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        // A VP8X header's flags say whether the file is animated.
        let animated = head.len() >= 21 && &head[12..16] == b"VP8X" && head[20] & 0x02 != 0;
        return Kind::Webp { animated };
    }
    if head.starts_with(&[0xff, 0x0a])
        || head.starts_with(&rawmakase_interop::isobmff::JXL_CONTAINER)
    {
        return Kind::Jxl;
    }
    use rawmakase_interop::isobmff::{AVIF_BRANDS, HEIF_BRANDS, ftyp_brands};
    if ftyp_brands(head).any(|b| AVIF_BRANDS.contains(b)) {
        return Kind::Avif;
    }
    // Any other HEIF, by its image or sequence brands: HEVC's, or MIAF's,
    // which name no codec (the HEIC reader says when it isn't HEVC).
    if ftyp_brands(head).any(|b| HEIF_BRANDS.contains(b)) {
        return Kind::Heic;
    }
    Kind::Other
}

/// A file's decoder with what both ways of reading it need first: refused
/// above 150 MP, its orientation, its colour profile when it has one that can
/// be read, and why Develop must not edit it when it must not.
struct Prepared {
    decoder: Box<dyn ImageDecoder>,
    orientation: image::metadata::Orientation,
    profile: Option<ColorProfile>,
    /// Why Develop must not edit it, though the Library can show it.
    refused: Option<Unsupported>,
    /// How many times HDR reference white its linear 1.0 is, for an HDR file.
    hdr_white: Option<f32>,
    /// How its light decodes exactly, for an HDR file that opens as one.
    hdr: Option<HdrDecoding>,
}

/// How an HDR file's samples decode exactly to its light: linear ProPhoto,
/// 1 = graphics white, signed, in f32, without moxcms's bounded tables.
#[derive(Clone, Copy, Debug)]
enum HdrDecoding {
    /// A signal of a CICP gamut and transfer, decoded as RAWmakase's HDR exports
    /// encode it (`HdrOutput::decode`).
    Signal(rawmakase_model::color::hdr_output::HdrOutput),
    /// Linear light through an ICC profile's colorants (D50, as ICC keeps them).
    Colorants([[f32; 3]; 3]),
}
impl HdrDecoding {
    fn transfer(self) -> rawmakase_model::color::hdr_output::HdrTransfer {
        match self {
            Self::Signal(output) => output.transfer,
            Self::Colorants(_) => rawmakase_model::color::hdr_output::HdrTransfer::Linear,
        }
    }
    /// The light of one pixel's samples.
    fn light(self, rgb: [f32; 3]) -> [f32; 3] {
        let rgb = rgb.map(|v| if v.is_finite() { v } else { 0. });
        match self {
            Self::Signal(output) => output.decode(rgb),
            Self::Colorants(m) => rawmakase_model::color::mul(m, rgb),
        }
    }
    /// An image's samples as light, row by row.
    fn apply(self, image: DynamicImage, cancel: Cancel<'_>) -> Result<Vec<[f32; 3]>> {
        let width = image.width() as usize;
        let samples = image.into_rgb32f().into_raw();
        let mut out = vec![[0f32; 3]; samples.len() / 3];
        if out.is_empty() {
            return Ok(out);
        }
        samples
            .par_chunks(width * 3)
            .zip(out.par_chunks_mut(width))
            .try_for_each(|(from, to)| {
                anyhow::ensure!(!cancel.is_cancelled(), "Opening cancelled");
                for (to, px) in to.iter_mut().zip(from.as_chunks::<3>().0) {
                    *to = self.light(*px);
                }
                Ok(())
            })?;
        Ok(out)
    }
}

/// How an HDR file's light decodes exactly, if it does:
/// - a CICP transfer of PQ, HLG or linear light, at full range, in BT.709,
///   BT.2020 or Display P3;
/// - an ICC profile without CICP whose curves are the identity, through its
///   colorants;
/// - `float` samples without a profile, as linear light in sRGB's primaries.
///
/// `None` for anything else, which stays refused.
fn hdr_decoding(profile: Option<&ColorProfile>, float: bool) -> Option<HdrDecoding> {
    use moxcms::TransferCharacteristics::{Hlg, Linear, Smpte2084};
    use rawmakase_model::color::hdr_output::{HdrGamut, HdrOutput, HdrTransfer};
    let signal = |gamut, transfer| {
        HdrDecoding::Signal(HdrOutput {
            gamut,
            transfer,
            ..Default::default()
        })
    };
    let Some(profile) = profile else {
        return float.then(|| signal(HdrGamut::Rec709, HdrTransfer::Linear));
    };
    match profile.cicp {
        Some(cicp) => {
            let transfer = match cicp.transfer_characteristics {
                Smpte2084 => HdrTransfer::Pq,
                Hlg => HdrTransfer::Hlg,
                Linear => HdrTransfer::Linear,
                _ => return None,
            };
            let gamut = HdrGamut::from_cicp(cicp.color_primaries as u8)?;
            cicp.full_range.then(|| signal(gamut, transfer))
        }
        None if linear_curves(profile) => {
            Some(HdrDecoding::Colorants(colorants_to_pro_photo(profile)?))
        }
        None => None,
    }
}

/// An ICC profile's linear RGB to linear ProPhoto: its colorants (XYZ D50) and
/// ProPhoto's inverse, in f64. `None` for colorants that don't invert.
fn colorants_to_pro_photo(profile: &ColorProfile) -> Option<[[f32; 3]; 3]> {
    let columns = |p: &ColorProfile| {
        let c = [p.red_colorant, p.green_colorant, p.blue_colorant];
        [
            [c[0].x, c[1].x, c[2].x],
            [c[0].y, c[1].y, c[2].y],
            [c[0].z, c[1].z, c[2].z],
        ]
    };
    let to_xyz = columns(profile);
    let from_xyz = inverse3(columns(&ColorProfile::new_pro_photo_rgb()))?;
    let mut m = [[0f32; 3]; 3];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| from_xyz[r][k] * to_xyz[k][c]).sum::<f64>() as f32;
        }
    }
    Some(m)
}

/// The inverse of a 3×3 matrix; `None` when it is singular.
fn inverse3(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let mut inv = [[0f64; 3]; 3];
    for (r, row) in inv.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            let (r1, r2) = ((c + 1) % 3, (c + 2) % 3);
            let (c1, c2) = ((r + 1) % 3, (r + 2) % 3);
            *v = (m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]) / det;
        }
    }
    Some(inv)
}

/// The decoder for the file's content (not its extension), with the reason
/// it can't be edited if any, and what the content is. Applies no size limit.
fn open_decoder(path: &Path) -> Result<(Box<dyn ImageDecoder>, Option<Unsupported>, Kind)> {
    let mut head = [0u8; 64];
    let n = {
        use std::io::Read;
        let mut f = std::fs::File::open(path)?;
        let mut n = 0;
        while n < head.len() {
            match f.read(&mut head[n..])? {
                0 => break,
                read => n += read,
            }
        }
        n
    };
    let kind = sniff(&head[..n]);
    Ok(match kind {
        Kind::Webp { animated } => (
            Box::new(
                image::ImageReader::open(path)?
                    .with_guessed_format()?
                    .into_decoder()
                    .map_err(read_error(path))?,
            ),
            animated.then(|| Unsupported("animated".into())),
            kind,
        ),
        Kind::Jxl => {
            let file = jxl::JxlFile::open(path)?;
            let refused = file.refusal();
            (Box::new(file), refused, kind)
        }
        Kind::Avif => {
            let file = avif::AvifFile::open(path)?;
            let refused = file.refusal();
            (Box::new(file), refused, kind)
        }
        Kind::Heic => {
            let file = heic::HeicFile::open(path)?;
            let refused = file.refusal();
            (Box::new(file), refused, kind)
        }
        _ => (
            Box::new(
                image::ImageReader::open(path)?
                    .with_guessed_format()?
                    .into_decoder()
                    .map_err(read_error(path))?,
            ),
            None,
            kind,
        ),
    })
}

fn prepare(path: &Path) -> Result<Prepared> {
    let (mut decoder, refused, kind) = open_decoder(path)?;
    // The `image` crate's decoders, whose float samples are linear light (a 32-bit
    // TIFF); JPEG XL, AVIF and HEIC have readers of their own.
    let generic = matches!(kind, Kind::Webp { .. } | Kind::Other);
    let (w, h) = decoder.dimensions();
    anyhow::ensure!(
        (w as u64) * (h as u64) <= 150_000_000,
        "Image too large to preview"
    );
    let orientation = decoder.orientation()?;
    // A PNG's cICP chunk takes precedence over its ICC profile (PNG 3rd edition);
    // the `image` crate reads the ICC profile alone.
    let png = match generic {
        true => png_facts(path)?.unwrap_or_default(),
        false => PngFacts::default(),
    };
    let profile = png.cicp.clone().or_else(|| {
        decoder
            .icc_profile()
            .ok()
            .flatten()
            .or_else(|| generic.then(|| tiff_icc(path)).flatten())
            .and_then(|icc| ColorProfile::new_from_slice(&icc).ok())
    });
    let float = generic
        && matches!(
            decoder.color_type(),
            image::ColorType::Rgb32F | image::ColorType::Rgba32F
        );
    let hdr_white = hdr_white(profile.as_ref(), float);
    // An HDR file opens as an HDR edit where its light decodes exactly
    // (docs/records/hdr-stage4-design.md); others stay refused, as HDR HEIC files
    // are: ImageIO and libheif draw them their own way.
    let hdr = hdr_white
        .and_then(|_| hdr_decoding(profile.as_ref(), float))
        .filter(|_| kind != Kind::Heic);
    // An APNG is an animation, and narrow-range samples aren't expanded: both are
    // refused whatever else the file is.
    let refused = refused
        .or_else(|| png.animated.then(|| Unsupported("animated".into())))
        .or_else(|| png.narrow.then(|| Unsupported("narrow-range".into())));
    let refused = match refused {
        Some(Unsupported(reason)) if reason == "HDR" && hdr.is_some() => None,
        refused => refused,
    }
    .or_else(|| (hdr_white.is_some() && hdr.is_none()).then(|| Unsupported("HDR".into())));
    Ok(Prepared {
        decoder,
        orientation,
        profile,
        refused,
        hdr_white,
        hdr,
    })
}

/// The ICC profile of the TIFF at `path`, if it is one and has one, read apart
/// from `image`: through it, the `tiff` crate keeps a tag only while its values,
/// 32 bytes or more each as it holds them, fit in the size of the pixels, so a
/// small image's profile was lost and the image read as sRGB. Profiles to 1 MiB.
fn tiff_icc(path: &Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek};
    let mut file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).ok()?;
    // Classic TIFF and BigTIFF, in either byte order.
    if ![*b"II*\0", *b"MM\0*", *b"II+\0", *b"MM\0+"].contains(&magic) {
        return None;
    }
    file.rewind().ok()?;
    let mut limits = tiff::decoder::Limits::default();
    limits.ifd_value_size = 1 << 20;
    limits.decoding_buffer_size = 64 << 20;
    tiff::decoder::Decoder::new(file)
        .ok()?
        .with_limits(limits)
        .get_tag_u8_vec(tiff::tags::Tag::IccProfile)
        .ok()
}

/// What a PNG's own chunks say that `image` doesn't report, read by the `png`
/// crate, which checks each chunk's CRC and drops a damaged ancillary one.
#[derive(Default)]
struct PngFacts {
    /// The colour its cICP chunk gives it, when moxcms makes a full-range profile of it.
    cicp: Option<ColorProfile>,
    /// Its cICP says narrow range, which neither conversion expands.
    narrow: bool,
    /// An APNG: an animation, whatever its default image is.
    animated: bool,
}

/// [`PngFacts`] of the file at `path`; `None` when it isn't a PNG. A PNG whose
/// chunks don't read is an error, not a PNG without facts: its colour or animation
/// would be lost. It reads within the allocation budget `image` decodes it with.
fn png_facts(path: &Path) -> Result<Option<PngFacts>> {
    use moxcms::{CicpColorPrimaries, CicpProfile, MatrixCoefficients, TransferCharacteristics};
    use std::io::{Read, Seek};
    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut signature = [0u8; 8];
    if file.read_exact(&mut signature).is_err() || signature != *b"\x89PNG\r\n\x1a\n" {
        return Ok(None);
    }
    file.rewind()?;
    let limits = png::Limits {
        bytes: 512 * 1024 * 1024,
    };
    let reader = png::Decoder::new_with_limits(file, limits)
        .read_info()
        .map_err(|e| anyhow!("Could not read the PNG file: {e}"))?;
    let info = reader.info();
    let mut facts = PngFacts {
        animated: info.animation_control.is_some(),
        ..Default::default()
    };
    if let Some(c) = info.coding_independent_code_points {
        facts.narrow = !c.is_video_full_range_image;
        facts.cicp = (|| {
            let profile = ColorProfile::new_from_cicp(CicpProfile {
                color_primaries: CicpColorPrimaries::try_from(c.color_primaries).ok()?,
                transfer_characteristics: TransferCharacteristics::try_from(c.transfer_function)
                    .ok()?,
                matrix_coefficients: MatrixCoefficients::try_from(c.matrix_coefficients).ok()?,
                full_range: true,
            });
            profile.cicp.is_some().then_some(profile)
        })()
        .filter(|_| !facts.narrow);
    }
    Ok(Some(facts))
}

/// The size of the image at `path` as shown, after its orientation, from its
/// header alone; `None` when it can't be read.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let (mut decoder, ..) = open_decoder(path).ok()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let (w, h) = decoder.dimensions();
    use Orientation::*;
    Some(match orientation {
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH => (h, w),
        _ => (w, h),
    })
}

/// The image at `path` decoded, turned upright and converted from its embedded
/// ICC profile to 8-bit sRGB; refused above 150 MP. A file without a profile,
/// or with one that cannot be read or does not describe its pixels (RGB, or
/// grey for a grey image), is taken to be sRGB already. An HDR (PQ or HLG)
/// file is tone mapped: its reference white shown as white.
pub fn decode(path: &Path) -> Result<RgbImage> {
    let Prepared {
        decoder,
        orientation,
        profile,
        hdr_white,
        hdr,
        ..
    } = prepare(path)?;
    if let Some(hdr) = hdr {
        let mut image = DynamicImage::from_decoder(decoder).map_err(read_error(path))?;
        image.apply_orientation(orientation);
        let (width, height) = (image.width(), image.height());
        let mut pixels = hdr.apply(image, Cancel::NEVER)?;
        pixels.par_iter_mut().for_each(|p| *p = hdr_sdr(*p));
        return Ok(linear_pro_photo_to_srgb(&pixels, width, height));
    }
    // HDR that doesn't decode exactly (float samples through a curved profile):
    // previewed through moxcms.
    if let Some(white) = hdr_white {
        use moxcms::TransferCharacteristics::{Hlg, Linear};
        let transfer = profile
            .as_ref()
            .and_then(|p| p.cicp)
            .map(|c| c.transfer_characteristics);
        let mut signal = Signal::Plain;
        let profile = match (profile, transfer) {
            // Float samples without a profile are linear, with sRGB's primaries.
            (None, _) => {
                signal = Signal::Unbounded;
                linear_in(&ColorProfile::new_srgb())
            }
            (Some(profile), Some(Linear)) => {
                signal = Signal::Unbounded;
                linear_in(&profile)
            }
            // A linear profile without CICP, as Photoshop's 32-bit documents have.
            (Some(profile), None) if linear_curves(&profile) => {
                signal = Signal::Unbounded;
                linear_in(&profile)
            }
            // HLG's system gamma works on luminance, which moxcms's per-channel
            // decoding doesn't do: decoded here, then converted as linear light.
            (Some(profile), Some(Hlg)) => {
                signal = Signal::Hlg(luminance_weights(&profile));
                linear_in(&profile)
            }
            (Some(profile), _) => profile,
        };
        let mut linear = to_linear(&profile, decoder.color_type())?;
        linear.signal = signal;
        let mut image = DynamicImage::from_decoder(decoder).map_err(read_error(path))?;
        image.apply_orientation(orientation);
        let (width, height) = (image.width(), image.height());
        let mut pixels = linear.apply(image, Cancel::NEVER)?;
        pixels
            .par_iter_mut()
            .for_each(|p| *p = tone_map(p.map(|v| v * white)));
        return Ok(linear_pro_photo_to_srgb(&pixels, width, height));
    }
    let transform = profile.and_then(|profile| ToSrgb::new(&profile, decoder.color_type()));
    let mut image = DynamicImage::from_decoder(decoder).map_err(read_error(path))?;
    image.apply_orientation(orientation);
    match transform {
        Some(transform) => transform.apply(image),
        None => Ok(image.into_rgb8()),
    }
}

/// The photo at `path` for Develop: decoded, turned upright and converted
/// from its embedded ICC profile to linear ProPhoto (D50) in f32, so wide
/// gamuts and 16-bit levels survive. A file without a readable profile is taken
/// to be sRGB. Transparency is dropped, as Lightroom ignores it in Develop.
/// A colour space other than RGB or grey is refused with [`Unsupported`]: an
/// edit of wrong colours is worse than none. `cancel` is checked between rows.
/// The EXIF camera and exposure are kept when the file has them.
///
/// This is the file as decoded, not yet the pseudo-scene Develop renders: open it
/// for Develop through [`crate::photo::open_source`], which converts it.
pub(crate) fn open(path: &Path, cancel: Cancel<'_>) -> Result<CameraImage> {
    let Prepared {
        decoder,
        orientation,
        profile,
        refused,
        hdr,
        ..
    } = prepare(path)?;
    if let Some(refused) = refused {
        return Err(refused.into());
    }
    let mut image = DynamicImage::from_decoder(decoder).map_err(read_error(path))?;
    image.apply_orientation(orientation);
    let (width, height) = (image.width(), image.height());
    // An HDR file: its light, and the SDR rendition the Library shows of it.
    let (light, pixels) = match hdr {
        Some(hdr) => {
            let light = rawmakase_model::camera_data::HdrLight::new(
                width,
                height,
                hdr.apply(image, cancel)?,
                hdr.transfer(),
            )
            .context("Could not decode the HDR image")?;
            let sdr = light.pixels().par_iter().map(|p| hdr_sdr(*p)).collect();
            (Some(light), sdr)
        }
        None => {
            let profile = profile.unwrap_or_else(ColorProfile::new_srgb);
            let linear = to_linear(&profile, image.color())?;
            (None, linear.apply(image, cancel)?)
        }
    };
    let info = rawmakase_interop::exif::photo_info(path).unwrap_or_default();
    let (make, model) = rawmakase_interop::exif::camera_names(path).unwrap_or_default();
    let number = |v: Option<f64>| v.map_or(0., |v| v as f32);
    let metadata = Metadata {
        make,
        model,
        width,
        height,
        raw_width: width,
        raw_height: height,
        iso: number(info.iso),
        shutter: number(info.exposure),
        aperture: number(info.aperture),
        focal: number(info.focal),
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        source: Source::Rendered,
        ..Default::default()
    };
    let image = CameraImage {
        width,
        height,
        recovered: Default::default(),
        pixels,
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    match light {
        Some(light) => image
            .with_hdr(std::sync::Arc::new(light))
            .context("Could not decode the HDR image"),
        None => Ok(image),
    }
}

/// Whether this system can decode a HEIF (HEIC) photo's pixels, or what to
/// install: macOS can; Windows says so only when decoding; elsewhere, as on
/// Linux, it takes the system's libheif with its HEVC decoder.
pub fn heif_decoder() -> std::result::Result<(), String> {
    heic::decoder()
}

/// Whether Develop can convert the pixels of the file at `path`, from its header
/// and embedded ICC profile alone, without decoding them: [`Unsupported`] for a
/// colour space [`open`] refuses. A file that can't be read is not refused here;
/// opening it says what is wrong.
pub fn probe(path: &Path) -> std::result::Result<(), Unsupported> {
    match prepare(path) {
        Ok(Prepared {
            refused: Some(refused),
            ..
        }) => Err(refused),
        Ok(Prepared {
            decoder,
            profile: Some(profile),
            ..
        }) => layout(&profile, decoder.color_type()).map(|_| ()),
        _ => Ok(()),
    }
}

/// A file's colour space is one Develop does not handle, e.g. "CMYK images".
#[derive(Debug)]
pub struct Unsupported(pub String);
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} images", self.0)
    }
}
impl std::error::Error for Unsupported {}

/// How pixels of type `color` in `profile` are read: RGB, or grey for a colourless
/// image. `None` for a grey profile on a colour image, which does not describe
/// its pixels, so they are taken as sRGB, as [`decode`] does. Any other colour
/// space is [`Unsupported`].
fn layout(
    profile: &ColorProfile,
    color: image::ColorType,
) -> std::result::Result<Option<Layout>, Unsupported> {
    match profile.color_space {
        DataColorSpace::Rgb => Ok(Some(Layout::Rgb)),
        DataColorSpace::Gray if !color.has_color() => Ok(Some(Layout::Gray)),
        DataColorSpace::Gray => Ok(None),
        DataColorSpace::Cmyk => Err(Unsupported("CMYK".into())),
        other => Err(Unsupported(format!("{other:?}"))),
    }
}

/// What decoding the file at `path` failed with, as "Could not read the WebP
/// file: …", its cause said once: `image`'s own message repeats its cause,
/// which a whole error chain then shows twice.
fn read_error(path: &Path) -> impl Fn(image::ImageError) -> anyhow::Error + '_ {
    move |e| match &e {
        image::ImageError::Decoding(decoding) => {
            let mut cause: &dyn std::error::Error = decoding;
            while let Some(inner) = cause.source() {
                cause = inner;
            }
            let name = rawmakase_model::storage::RasterFormat::of(path)
                .map_or("image", |format| format.name());
            anyhow!("Could not read the {name} file: {cause}")
        }
        _ => e.into(),
    }
}

/// How many times HDR reference white (203 nits, ITU-R BT.2408) the linear
/// 1.0 of an HDR file is: PQ's 10,000 nits, HLG's nominal 1,000-nit peak, or 1 for
/// linear light, which RAWmakase's HDR exports and Photoshop's 32-bit documents
/// keep with 1.0 at graphics white. Linear light is a profile's CICP transfer, or
/// `float` samples (a 32-bit TIFF's). `None` for an SDR file.
fn hdr_white(profile: Option<&ColorProfile>, float: bool) -> Option<f32> {
    use moxcms::TransferCharacteristics::{Hlg, Linear, Smpte2084};
    match profile
        .and_then(|p| p.cicp)
        .map(|c| c.transfer_characteristics)
    {
        Some(Smpte2084) => Some(10_000. / 203.),
        Some(Hlg) => Some(1_000. / 203.),
        Some(Linear) => Some(1.),
        _ => float.then_some(1.),
    }
}

/// `profile`'s primaries with no curve: its pixels as linear light. Empty curves
/// are the identity in ICC; moxcms would use a CICP transfer over them.
fn linear_in(profile: &ColorProfile) -> ColorProfile {
    let mut linear = profile.clone();
    linear.red_trc = Some(ToneReprCurve::Lut(vec![]));
    linear.green_trc = Some(ToneReprCurve::Lut(vec![]));
    linear.blue_trc = Some(ToneReprCurve::Lut(vec![]));
    linear.gray_trc = Some(ToneReprCurve::Lut(vec![]));
    linear.cicp = None;
    linear
}

/// Whether `profile`'s RGB curves are all the identity, however each is written.
fn linear_curves(profile: &ColorProfile) -> bool {
    let identity = |curve: &Option<ToneReprCurve>| curve.as_ref().is_some_and(is_identity);
    identity(&profile.red_trc) && identity(&profile.green_trc) && identity(&profile.blue_trc)
}

/// Whether an ICC tone curve is the identity on 0–1: a table that is empty, a u8.8
/// gamma of exactly 1, or samples each within one 16-bit step of y = x; or a
/// parametric curve of ICC types 0–4 whose every segment that applies within 0–1
/// is y = x, by its parameters.
fn is_identity(curve: &ToneReprCurve) -> bool {
    const EPS: f32 = 1e-5;
    let one = |v: f32| (v - 1.).abs() < EPS;
    let zero = |v: f32| v.abs() < EPS;
    match curve {
        ToneReprCurve::Lut(table) => match table.as_slice() {
            [] => true,
            [gamma] => *gamma == 256,
            table => {
                let last = (table.len() - 1) as f64;
                table
                    .iter()
                    .enumerate()
                    .all(|(i, v)| (*v as f64 - i as f64 / last * 65535.).abs() <= 1.)
            }
        },
        // Types 1–4 share (aX + b)^g above their break, plus an offset in types 2
        // and 4: with g = 1 and a = 1 that is y = x when the offsets cancel, and it
        // then covers 0–1 when the break is at 0 or below.
        ToneReprCurve::Parametric(p) => match *p.as_slice() {
            [g] => one(g),
            [g, a, b] => one(g) && one(a) && zero(b),
            // Below the break, at -b, the constant c.
            [g, a, b, c] => one(g) && one(a) && b >= -EPS && zero(b + c),
            // Below d, cX; that segment applies only when d > 0.
            [g, a, b, c, d] => one(g) && one(a) && zero(b) && (d <= 0. || one(c)),
            // Below d, cX + f; above it, (aX + b)^g + e.
            [g, a, b, c, d, e, f] => {
                one(g) && one(a) && zero(b + e) && (d <= 0. || (one(c) && zero(f)))
            }
            _ => false,
        },
    }
}

/// The luminance weights of `profile`'s RGB, from its colorants.
fn luminance_weights(profile: &ColorProfile) -> [f32; 3] {
    let y = [
        profile.red_colorant.y,
        profile.green_colorant.y,
        profile.blue_colorant.y,
    ];
    let sum: f64 = y.iter().sum();
    if sum > 0. {
        y.map(|v| (v / sum) as f32)
    } else {
        [1. / 3.; 3]
    }
}

/// An HDR file's SDR rendition of light `p` (linear ProPhoto, 1 = graphics white):
/// negative channels at 0, then [`tone_map`]. The Library's preview and Develop's
/// SDR rendition both take it, so they agree.
fn hdr_sdr(p: [f32; 3]) -> [f32; 3] {
    tone_map(p.map(|v| v.max(0.)))
}

/// Light of an HDR image relative to its reference white, brought into the
/// 0–1 of SDR for a preview: kept below `KNEE` and rolled off smoothly into
/// white above it, by the brightest channel, so hues stay.
fn tone_map(rgb: [f32; 3]) -> [f32; 3] {
    const KNEE: f32 = 0.8;
    let peak = rgb[0].max(rgb[1]).max(rgb[2]);
    if peak <= KNEE {
        return rgb;
    }
    let over = (peak - KNEE) / (1. - KNEE);
    let mapped = KNEE + (1. - KNEE) * (1. - (-over).exp());
    rgb.map(|v| v * mapped / peak)
}

/// Linear ProPhoto pixels of a `width` × `height` image in 8-bit sRGB.
/// Converted here rather than by moxcms, whose f32 conversion to sRGB on ARM
/// (NEON) darkens red by about 1.5%.
fn linear_pro_photo_to_srgb(pixels: &[[f32; 3]], width: u32, height: u32) -> RgbImage {
    use rawmakase_model::{camera_profiles::PRO_TO_RGB, color};
    let mut out = RgbImage::new(width, height);
    out.par_chunks_mut(3).zip(pixels).for_each(|(to, from)| {
        let rgb = color::mul(PRO_TO_RGB, *from);
        for (to, v) in to.iter_mut().zip(rgb) {
            *to = (color::srgb_encode(v.clamp(0., 1.)) * 255.).round() as u8;
        }
    });
    out
}

/// ProPhoto's primaries with no gamma: an empty curve is the identity in ICC.
fn linear_pro_photo() -> ColorProfile {
    let mut linear_pro = ColorProfile::new_pro_photo_rgb();
    linear_pro.red_trc = Some(ToneReprCurve::Lut(vec![]));
    linear_pro.green_trc = Some(ToneReprCurve::Lut(vec![]));
    linear_pro.blue_trc = Some(ToneReprCurve::Lut(vec![]));
    linear_pro
}

/// The colour space an embedded preview is in.
pub(crate) enum PreviewSpace<'a> {
    Icc(&'a [u8]),
    Srgb,
    AdobeRgb,
}
/// An 8-bit RGB `image` in `space` as linear ProPhoto RGB; an ICC profile that does
/// not parse, is not RGB or has no transform is an error, never taken for sRGB.
pub(crate) fn rgb8_linear_pro_photo(
    image: &RgbImage,
    space: PreviewSpace,
) -> Result<Vec<[f32; 3]>> {
    let profile = match space {
        PreviewSpace::Icc(icc) => {
            ColorProfile::new_from_slice(icc).map_err(|e| anyhow!("Unreadable ICC profile: {e}"))?
        }
        PreviewSpace::Srgb => ColorProfile::new_srgb(),
        PreviewSpace::AdobeRgb => ColorProfile::new_adobe_rgb(),
    };
    anyhow::ensure!(
        profile.color_space == DataColorSpace::Rgb,
        "The preview's ICC profile is not RGB"
    );
    let transform = profile
        .create_transform_f32(
            Layout::Rgb,
            &linear_pro_photo(),
            Layout::Rgb,
            TransformOptions::default(),
        )
        .map_err(|e| anyhow!("Could not convert the preview to ProPhoto: {e}"))?;
    let from: Vec<f32> = image
        .as_raw()
        .iter()
        .map(|v| f32::from(*v) / 255.)
        .collect();
    let mut out = vec![[0f32; 3]; image.as_raw().len() / 3];
    transform
        .transform(&from, bytemuck::cast_slice_mut(&mut out))
        .map_err(|e| anyhow!("Could not convert the preview to ProPhoto: {e}"))?;
    Ok(out)
}

/// A conversion from a file's profile to linear ProPhoto, in f32.
struct ToLinear {
    transform: std::sync::Arc<moxcms::TransformF32Executor>,
    gray: bool,
    /// What the samples are before the profile's transform.
    signal: Signal,
}

/// What a file's samples hold, where the profile's own transform can't say it.
#[derive(Clone, Copy, PartialEq)]
enum Signal {
    /// What the profile says.
    Plain,
    /// Linear light that may exceed 1, which moxcms clamps, through a linear
    /// transform: converted scaled into 0–1 by its peak and scaled back, which the
    /// linear transform keeps exact.
    Unbounded,
    /// BT.2100 HLG, through the transform of its primaries alone: the inverse OETF
    /// to scene light, then the OOTF of HLG's 1,000-nit reference display (system
    /// gamma 1.2 on the luminance these weights give), to display light where 1 is
    /// that display's peak.
    Hlg([f32; 3]),
}

/// The conversion for pixels of type `color` in `profile`; RGB, or grey for a
/// colourless image. As in [`decode`], a profile that does not describe the
/// pixels (a grey one on a colour image) or that has no transform is taken to be
/// sRGB. Any other colour space is [`Unsupported`].
fn to_linear(profile: &ColorProfile, color: image::ColorType) -> Result<ToLinear> {
    let linear_pro = linear_pro_photo();
    let build = |profile: &ColorProfile, layout| {
        profile.create_transform_f32(
            layout,
            &linear_pro,
            Layout::Rgb,
            TransformOptions::default(),
        )
    };
    let own =
        layout(profile, color)?.and_then(|layout| Some((build(profile, layout).ok()?, layout)));
    let (transform, layout) = match own {
        Some(own) => own,
        None => (
            build(&ColorProfile::new_srgb(), Layout::Rgb)
                .map_err(|e| anyhow!("Could not convert the image to ProPhoto: {e}"))?,
            Layout::Rgb,
        ),
    };
    Ok(ToLinear {
        transform,
        gray: layout == Layout::Gray,
        signal: Signal::Plain,
    })
}
impl ToLinear {
    /// `image`'s pixels as linear ProPhoto, a row at a time.
    fn apply(&self, image: DynamicImage, cancel: Cancel<'_>) -> Result<Vec<[f32; 3]>> {
        let (width, height) = (image.width() as usize, image.height() as usize);
        let mut out = vec![[0f32; 3]; width * height];
        if out.is_empty() {
            return Ok(out);
        }
        let (mut samples, per_pixel) = if self.gray {
            (image.to_luma32f().into_raw(), 1)
        } else {
            (image.into_rgb32f().into_raw(), 3)
        };
        if let Signal::Hlg(weights) = self.signal {
            samples.par_chunks_mut(per_pixel).for_each(|px| {
                for v in px.iter_mut() {
                    *v = rawmakase_model::color::hdr_output::hlg_inverse_oetf(*v);
                }
                let y = if px.len() == 3 {
                    weights[0] * px[0] + weights[1] * px[1] + weights[2] * px[2]
                } else {
                    px[0]
                };
                let gain = if y > 0. { y.powf(0.2) } else { 0. };
                for v in px.iter_mut() {
                    *v *= gain;
                }
            });
        }
        let scale = if self.signal == Signal::Unbounded {
            samples
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .fold(1f32, f32::max)
        } else {
            1.
        };
        if scale > 1. {
            samples.par_iter_mut().for_each(|v| *v /= scale);
        }
        samples
            .par_chunks(width * per_pixel)
            .zip(out.par_chunks_mut(width))
            .try_for_each(|(from, to)| {
                anyhow::ensure!(!cancel.is_cancelled(), "Opening cancelled");
                self.transform
                    .transform(from, bytemuck::cast_slice_mut(to))
                    .map_err(|e| anyhow!("Could not convert the image to ProPhoto: {e}"))
            })?;
        if scale > 1. {
            out.par_iter_mut().for_each(|p| *p = p.map(|v| v * scale));
        }
        Ok(out)
    }
}

/// A conversion from a file's profile to sRGB, at the depth its samples need.
enum ToSrgb {
    Eight(std::sync::Arc<moxcms::Transform8BitExecutor>, Layout),
    Sixteen(std::sync::Arc<moxcms::Transform16BitExecutor>, Layout),
}
impl ToSrgb {
    /// The conversion for pixels of type `color` in `profile`. Samples of
    /// more than 8 bits are converted at 16 and only then rounded to 8.
    fn new(profile: &ColorProfile, color: image::ColorType) -> Option<Self> {
        let layout = match profile.color_space {
            DataColorSpace::Rgb => Layout::Rgb,
            DataColorSpace::Gray if !color.has_color() => Layout::Gray,
            _ => return None,
        };
        let deep = color.bytes_per_pixel() > color.channel_count();
        let srgb = ColorProfile::new_srgb();
        let options = TransformOptions::default();
        Some(if deep {
            // moxcms's fixed-point 16-bit conversion on ARM clips matrix
            // coefficients above 2, darkening red from ProPhoto (moxcms#202).
            let options = TransformOptions {
                prefer_fixed_point: false,
                ..options
            };
            let t = profile.create_transform_16bit(layout, &srgb, Layout::Rgb, options);
            Self::Sixteen(t.ok()?, layout)
        } else {
            let t = profile.create_transform_8bit(layout, &srgb, Layout::Rgb, options);
            Self::Eight(t.ok()?, layout)
        })
    }
    fn apply(&self, image: DynamicImage) -> Result<RgbImage> {
        let mut out = RgbImage::new(image.width(), image.height());
        let gray = |layout: &Layout| *layout == Layout::Gray;
        match self {
            Self::Eight(t, l) if gray(l) => rows(&image.into_luma8(), &**t, &mut out, |v| v),
            Self::Eight(t, _) => rows(&image.into_rgb8(), &**t, &mut out, |v| v),
            Self::Sixteen(t, l) if gray(l) => rows(&image.into_luma16(), &**t, &mut out, to_8),
            Self::Sixteen(t, _) => rows(&image.into_rgb16(), &**t, &mut out, to_8),
        }?;
        Ok(out)
    }
}

/// A 16-bit sample rounded to 8 bits.
fn to_8(v: u16) -> u8 {
    ((u32::from(v) + 128) / 257) as u8
}

/// Converts `source` into `out`, which has as many rows, a row at a time, so a
/// 16-bit image never needs a second full-size 16-bit copy.
fn rows<T: Copy + Default + Send + Sync>(
    source: &[T],
    transform: &(dyn TransformExecutor<T> + Send + Sync),
    out: &mut RgbImage,
    quantise: impl Fn(T) -> u8 + Sync,
) -> Result<()> {
    let height = out.height() as usize;
    if height == 0 || out.is_empty() {
        return Ok(());
    }
    let out_row = out.len() / height;
    source
        .par_chunks(source.len() / height)
        .zip(out.par_chunks_mut(out_row))
        .try_for_each(|(from, to)| {
            let mut row = vec![T::default(); to.len()];
            transform.transform(from, &mut row)?;
            for (to, v) in to.iter_mut().zip(row) {
                *to = quantise(v);
            }
            Ok(())
        })
        .map_err(|e: moxcms::CmsError| anyhow!("Could not convert the image to sRGB: {e}"))
}

#[cfg(test)]
mod tests {
    use super::decode;
    use image::{ExtendedColorType, ImageEncoder};
    use rawmakase_cancel::Cancel;
    use std::path::Path;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/formats")
            .join(name)
    }
    /// The colour of the upright image at the centre of each 4×4 corner.
    fn corners(image: &image::RgbImage) -> [[u8; 3]; 4] {
        let (w, h) = image.dimensions();
        [(1, 1), (w - 2, 1), (1, h - 2), (w - 2, h - 2)].map(|(x, y)| image.get_pixel(x, y).0)
    }
    const UPRIGHT: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];

    /// Whether this system decodes a HEIC's pixels in this test: macOS, and
    /// Linux with libheif; Windows, whose extensions CI lacks, has its own
    /// test. CI's Arch job installs libheif and sets RAWMAKASE_REQUIRE_HEIF,
    /// so there a test of HEIC pixels fails rather than skips without it.
    fn heic_decodes() -> bool {
        let decodes = !cfg!(windows) && super::heif_decoder().is_ok();
        if !decodes {
            assert!(
                std::env::var_os("RAWMAKASE_REQUIRE_HEIF").is_none(),
                "a HEIC decoder is required: {:?}",
                super::heif_decoder()
            );
            eprintln!("HEIC pixels not tested: {:?}", super::heif_decoder());
        }
        decodes
    }
    /// `got` within `tolerance` per channel of `want`: for files that went
    /// through a lossy JPEG before, whose pixels are not exactly 0 or 255.
    fn assert_corners_near(got: [[u8; 3]; 4], want: [[u8; 3]; 4], tolerance: u8) {
        for (g, w) in got.iter().zip(&want) {
            assert!(
                g.iter().zip(w).all(|(a, b)| a.abs_diff(*b) <= tolerance),
                "{got:?} is not within {tolerance} of {want:?}"
            );
        }
    }

    #[test]
    fn a_jxl_decodes_with_its_colours() {
        let image = decode(&fixture("metadata.jxl")).unwrap();
        assert_corners_near(corners(&image), UPRIGHT, 8);
    }

    #[test]
    fn a_jxl_is_not_rotated_again_by_its_exif() {
        // `jxlinfo` reports codestream Orientation 6 for this file (and 132
        // bytes of EXIF from the JPEG it was transcoded from); jxl-oxide
        // renders that orientation itself, so we must not turn it again.
        let path = fixture("exif-rotated.jxl");
        let decoded = decode(&path).unwrap();
        let reference = jxl_oxide::JxlImage::builder().open(&path).unwrap();
        assert_eq!(
            decoded.dimensions(),
            (reference.width(), reference.height())
        );
    }

    #[test]
    fn a_sixteen_bit_jxl_keeps_its_depth_for_develop() -> anyhow::Result<()> {
        let im = super::open(&fixture("sixteen-bit.jxl"), Cancel::NEVER)?;
        assert_eq!((im.width, im.height), (16, 12));
        // Red corner, linear ProPhoto: a strong red, no green or blue left.
        let red = im.pixels[16 + 1];
        assert!(red[0] > 0.4 && red[1] < 0.1 && red[2] < 0.1, "{red:?}");
        Ok(())
    }

    #[test]
    fn animated_jxls_show_but_are_refused_for_develop_and_hdr_ones_open() {
        let path = fixture("animated.jxl");
        assert!(decode(&path).is_ok());
        assert_eq!(super::probe(&path).unwrap_err().0, "animated");
        let path = fixture("pq.jxl");
        assert!(super::probe(&path).is_ok());
        assert!(super::open(&path, Cancel::NEVER).unwrap().hdr().is_some());
    }

    #[test]
    fn a_webp_decodes_upright_by_its_exif() {
        let image = decode(&fixture("rotated.webp")).unwrap();
        // Orientation 6 turns the 16x12 drawing a quarter clockwise.
        assert_eq!(image.dimensions(), (12, 16));
        // Its top-left corner is the drawing's bottom-left (blue).
        assert_eq!(corners(&image)[0], [0, 0, 255]);
        let plain = decode(&fixture("metadata.webp")).unwrap();
        assert_eq!(corners(&plain), UPRIGHT);
    }

    #[test]
    fn an_animated_webp_shows_its_first_frame_but_is_refused_for_develop() {
        let path = fixture("animated.webp");
        assert_eq!(corners(&decode(&path).unwrap()), UPRIGHT);
        assert_eq!(super::probe(&path).unwrap_err().0, "animated");
        let err = super::open(&path, Cancel::NEVER).err().unwrap();
        assert_eq!(
            err.downcast_ref::<super::Unsupported>().unwrap().0,
            "animated"
        );
    }

    #[test]
    fn a_still_webp_opens_for_develop() -> anyhow::Result<()> {
        let im = super::open(&fixture("metadata.webp"), Cancel::NEVER)?;
        assert_eq!((im.width, im.height), (16, 12));
        Ok(())
    }

    fn p3() -> Vec<u8> {
        moxcms::ColorProfile::new_display_p3().encode().unwrap()
    }
    /// A one-row PNG of `pixels`, with `icc` embedded when given.
    fn png(path: &Path, pixels: &[u8], color: ExtendedColorType, icc: Option<Vec<u8>>) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = image::codecs::png::PngEncoder::new(file);
        if let Some(icc) = icc {
            encoder.set_icc_profile(icc).unwrap();
        }
        let bytes_per_pixel = match color {
            ExtendedColorType::L8 => 1,
            ExtendedColorType::Rgb8 => 3,
            ExtendedColorType::Rgb16 => 6,
            _ => unreachable!(),
        };
        let width = (pixels.len() / bytes_per_pixel) as u32;
        encoder.write_image(pixels, width, 1, color).unwrap();
    }
    /// Display P3 `rgb` (0–255) as sRGB, worked out apart from any CMS: both
    /// encode with the sRGB curve and share D65, so only the primaries differ.
    fn p3_as_srgb(rgb: [f64; 3]) -> [f64; 3] {
        let linear = |v: f64| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let encode = |v: f64| {
            let v = v.clamp(0., 1.);
            255. * if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1. / 2.4) - 0.055
            }
        };
        let [r, g, b] = rgb.map(|v| linear(v / 255.));
        [
            encode(1.224_940_2 * r - 0.224_940_2 * g),
            encode(-0.042_056_95 * r + 1.042_056_95 * g),
            encode(-0.019_637_55 * r - 0.078_636_05 * g + 1.098_273_6 * b),
        ]
    }
    fn assert_near(actual: image::Rgb<u8>, expected: [f64; 3], tolerance: f64) {
        for (a, e) in actual.0.iter().zip(expected) {
            assert!(
                (*a as f64 - e).abs() <= tolerance,
                "{actual:?} is not {expected:?}"
            );
        }
    }

    #[test]
    fn a_display_p3_png_is_converted_to_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p3.png");
        let pixels = [200, 100, 50, 40, 180, 90];
        png(&path, &pixels, ExtendedColorType::Rgb8, Some(p3()));
        let image = decode(&path).unwrap();
        assert_near(*image.get_pixel(0, 0), p3_as_srgb([200., 100., 50.]), 1.5);
        assert_near(*image.get_pixel(1, 0), p3_as_srgb([40., 180., 90.]), 1.5);
    }

    #[test]
    fn a_sixteen_bit_png_is_converted_to_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p3-16.png");
        // Red and green half-way between 8-bit codes. These are 194.77, 6.01
        // and 130.93 in sRGB; rounded to 8 bits first, they would come out
        // 194.24, 4.86 and 130.96.
        let samples: [u16; 3] = [45_874, 9_894, 32_896];
        let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_ne_bytes()).collect();
        png(&path, &bytes, ExtendedColorType::Rgb16, Some(p3()));
        let image = decode(&path).unwrap();
        let expected = p3_as_srgb(samples.map(|v| v as f64 / 257.));
        assert_near(*image.get_pixel(0, 0), expected, 0.5);
    }

    /// ProPhoto to sRGB needs a matrix coefficient above 2, which moxcms's
    /// fixed-point 16-bit conversion on ARM clips, darkening red.
    #[test]
    fn a_sixteen_bit_pro_photo_grey_stays_grey() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pro-photo-16.png");
        let bytes: Vec<u8> = [41_943u16; 3]
            .iter()
            .flat_map(|v| v.to_ne_bytes())
            .collect();
        let pro_photo = moxcms::ColorProfile::new_pro_photo_rgb().encode().unwrap();
        png(&path, &bytes, ExtendedColorType::Rgb16, Some(pro_photo));
        let image = decode(&path).unwrap();
        assert_near(*image.get_pixel(0, 0), [178.; 3], 0.5);
    }

    #[test]
    fn a_display_p3_jpeg_is_converted_to_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p3.jpg");
        let source = image::RgbImage::from_pixel(16, 16, image::Rgb([200, 100, 50]));
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 100);
        encoder.set_icc_profile(p3()).unwrap();
        encoder
            .write_image(source.as_raw(), 16, 16, ExtendedColorType::Rgb8)
            .unwrap();
        let image = decode(&path).unwrap();
        // JPEG's own rounding comes on top of the conversion's.
        assert_near(*image.get_pixel(8, 8), p3_as_srgb([200., 100., 50.]), 3.);
    }

    #[test]
    fn a_grey_png_is_converted_through_its_grey_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("linear-grey.png");
        let linear = moxcms::ColorProfile::new_gray_with_gamma(1.)
            .encode()
            .unwrap();
        png(&path, &[128], ExtendedColorType::L8, Some(linear));
        // Linear 128/255 is 188 in sRGB.
        assert_near(*decode(&path).unwrap().get_pixel(0, 0), [188.; 3], 1.);
    }

    #[test]
    fn a_file_without_a_profile_is_taken_as_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.png");
        let pixels = [200, 100, 50, 0, 255, 7];
        png(&path, &pixels, ExtendedColorType::Rgb8, None);
        assert_eq!(decode(&path).unwrap().as_raw(), &pixels);
    }

    #[test]
    fn an_unreadable_profile_is_taken_as_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.png");
        let pixels = [200, 100, 50];
        let icc = b"not a profile".to_vec();
        png(&path, &pixels, ExtendedColorType::Rgb8, Some(icc));
        assert_eq!(decode(&path).unwrap().as_raw(), &pixels);
    }

    fn linear_srgb_to_prophoto(rgb: [f32; 3]) -> [f32; 3] {
        // Bradford-adapted sRGB D65 -> ProPhoto D50.
        rawmakase_model::color::mul(rawmakase_model::camera_profiles::RGB_TO_PRO, rgb)
    }
    fn srgb_decode(v: u8) -> f32 {
        let v = v as f32 / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    #[test]
    fn an_srgb_png_opens_as_linear_prophoto() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("a.png");
        // 4x1: black, mid grey, a saturated red, white; no ICC (taken as sRGB).
        let px: [[u8; 3]; 4] = [[0, 0, 0], [118, 118, 118], [200, 30, 40], [255, 255, 255]];
        image::RgbImage::from_fn(4, 1, |x, _| image::Rgb(px[x as usize])).save(&path)?;
        let im = super::open(&path, Cancel::NEVER)?;
        assert_eq!((im.width, im.height), (4, 1));
        assert!(im.metadata.is_rendered());
        for (got, want) in im.pixels.iter().zip(px) {
            let want = linear_srgb_to_prophoto(want.map(srgb_decode));
            for c in 0..3 {
                assert!((got[c] - want[c]).abs() < 2e-3, "{got:?} vs {want:?}");
            }
        }
        Ok(())
    }

    #[test]
    fn an_adobe_rgb_green_keeps_its_gamut() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("adobe.png");
        let mut bytes = Vec::new();
        let mut e = image::codecs::png::PngEncoder::new(&mut bytes);
        e.set_icc_profile(moxcms::ColorProfile::new_adobe_rgb().encode()?)?;
        e.write_image(&[0, 255, 0], 1, 1, ExtendedColorType::Rgb8)?;
        std::fs::write(&path, bytes)?;
        let im = super::open(&path, Cancel::NEVER)?;
        // Adobe RGB green -> XYZ D65 -> Bradford D50 -> linear ProPhoto,
        // worked out from the published matrices.
        let want = [0.1133, 0.8330, 0.0738];
        for (got, want) in im.pixels[0].iter().zip(want) {
            assert!((got - want).abs() < 5e-3, "{:?}", im.pixels[0]);
        }
        // sRGB's own green is [0.330, 0.874, 0.118] in ProPhoto: an sRGB clip
        // on the way in would have pulled R up to about there.
        assert!(im.pixels[0][0] < 0.2);
        Ok(())
    }

    #[test]
    fn a_sixteen_bit_tiff_keeps_every_level() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("ramp.tif");
        let ramp = image::ImageBuffer::<image::Rgb<u16>, _>::from_fn(1024, 1, |x, _| {
            let v = (x * 64) as u16;
            image::Rgb([v, v, v])
        });
        ramp.save(&path)?;
        let im = super::open(&path, Cancel::NEVER)?;
        let distinct: std::collections::BTreeSet<u32> =
            im.pixels.iter().map(|p| p[1].to_bits()).collect();
        assert_eq!(distinct.len(), 1024);
        Ok(())
    }

    #[test]
    fn a_grey_profile_on_a_colour_image_opens_as_srgb() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("grey-profile.png");
        let grey = moxcms::ColorProfile::new_gray_with_gamma(1.).encode()?;
        png(&path, &[128, 128, 128], ExtendedColorType::Rgb8, Some(grey));
        assert!(super::probe(&path).is_ok());
        let plain = dir.path().join("plain.png");
        png(&plain, &[128, 128, 128], ExtendedColorType::Rgb8, None);
        let opened = super::open(&path, Cancel::NEVER)?;
        let srgb = super::open(&plain, Cancel::NEVER)?;
        assert_eq!(opened.pixels, srgb.pixels);
        Ok(())
    }

    #[test]
    fn a_cmyk_profile_is_refused_for_develop() {
        let mut profile = moxcms::ColorProfile::new_srgb();
        profile.color_space = moxcms::DataColorSpace::Cmyk;
        let err = super::to_linear(&profile, image::ColorType::Rgb8)
            .map(|_| ())
            .unwrap_err();
        let refused = err
            .downcast_ref::<super::Unsupported>()
            .expect("Unsupported");
        assert_eq!(refused.0, "CMYK");
    }

    #[test]
    fn a_probe_refuses_a_cmyk_file_without_decoding_it() {
        // A minimal ICC header that says its pixels are CMYK.
        let mut icc = vec![0u8; 132];
        icc[0..4].copy_from_slice(&132u32.to_be_bytes());
        icc[8..12].copy_from_slice(&0x0240_0000u32.to_be_bytes());
        icc[12..16].copy_from_slice(b"prtr");
        icc[16..20].copy_from_slice(b"CMYK");
        icc[20..24].copy_from_slice(b"Lab ");
        icc[36..40].copy_from_slice(b"acsp");
        for (i, v) in [0x0000_F6D6u32, 0x0001_0000, 0x0000_D32D]
            .iter()
            .enumerate()
        {
            icc[68 + 4 * i..72 + 4 * i].copy_from_slice(&v.to_be_bytes());
        }
        let dir = tempfile::tempdir().unwrap();
        let cmyk = dir.path().join("cmyk.png");
        png(&cmyk, &[10; 6], ExtendedColorType::Rgb8, Some(icc));
        assert_eq!(super::probe(&cmyk).unwrap_err().0, "CMYK");
        let srgb = dir.path().join("srgb.png");
        png(&srgb, &[10; 6], ExtendedColorType::Rgb8, None);
        assert!(super::probe(&srgb).is_ok());
        // Not this probe's to refuse: opening says what is wrong with it.
        let damaged = dir.path().join("damaged.jpg");
        std::fs::write(&damaged, b"not a jpeg").unwrap();
        assert!(super::probe(&damaged).is_ok());
    }

    #[test]
    fn dimensions_are_upright_and_come_from_the_header() {
        assert_eq!(super::dimensions(&fixture("rotated.avif")), Some((12, 16)));
        assert_eq!(super::dimensions(&fixture("rotated.webp")), Some((12, 16)));
        assert_eq!(super::dimensions(&fixture("metadata.jxl")), Some((16, 12)));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rotated.jpg");
        rotated_jpeg(&path);
        assert_eq!(super::dimensions(&path), Some((1, 2)));
    }

    /// An AVIF whose image needs a property RAWmakase does not read (a crop,
    /// here) is not shown wrongly: it can't be opened.
    #[test]
    fn an_avif_needing_a_property_it_does_not_read_is_not_opened() {
        let error = decode(&fixture("cropped.avif")).unwrap_err().to_string();
        assert!(error.contains("clap"), "{error}");
        assert_eq!(super::dimensions(&fixture("cropped.avif")), None);
    }

    /// A crop not marked essential is ignored, as it was before RAWmakase
    /// read crops: the image opens uncropped.
    #[test]
    fn an_avif_crop_not_marked_essential_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("optional-crop.avif");
        let mut bytes = std::fs::read(fixture("cropped.avif")).unwrap();
        // The primary item's associations: ispe, pixi, av1C (essential), colr,
        // clap (essential).
        let at = bytes
            .windows(3)
            .position(|w| w == [0x83, 0x04, 0x85])
            .unwrap();
        bytes[at + 2] = 0x05;
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(decode(&path).unwrap().dimensions(), (16, 12));
    }

    /// The size and the refusal come from an AVIF's header: its AV1 data is
    /// read only to decode it.
    #[test]
    fn an_avif_header_is_read_without_its_image_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.avif");
        let bytes = std::fs::read(fixture("rotated.avif")).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 20]).unwrap();
        assert_eq!(super::dimensions(&path), Some((12, 16)));
        assert!(super::probe(&path).is_ok());
        assert!(decode(&path).is_err());
    }

    #[test]
    fn a_file_opens_by_its_content_whatever_its_extension() {
        let dir = tempfile::tempdir().unwrap();
        for (from, to, tolerance) in [
            ("metadata.webp", "a.jpg", 0),
            ("ten-bit.avif", "b.png", 0),
            // A JPEG transcode, so the pixels are only near.
            ("metadata.jxl", "c.webp", 8),
            // Lossy 4:2:0, where there is a HEVC decoder.
            ("grid.heic", "d.jpg", 4),
        ] {
            if from.ends_with(".heic") && !heic_decodes() {
                continue;
            }
            let path = dir.path().join(to);
            std::fs::copy(fixture(from), &path).unwrap();
            assert_corners_near(corners(&decode(&path).unwrap()), UPRIGHT, tolerance);
        }
    }

    #[test]
    fn dimensions_have_no_size_cap() {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = !0u32;
            for &b in bytes {
                crc ^= b as u32;
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
                }
            }
            !crc
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
            out.extend((body.len() as u32).to_be_bytes());
            let mut data = kind.to_vec();
            data.extend(body);
            out.extend(&data);
            out.extend(crc32(&data).to_be_bytes());
        }
        let mut ihdr = Vec::new();
        ihdr.extend(20000u32.to_be_bytes());
        ihdr.extend(10000u32.to_be_bytes());
        ihdr.extend([8, 2, 0, 0, 0]);
        let mut file = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(&mut file, b"IHDR", &ihdr);
        chunk(&mut file, b"IDAT", &[0x78, 0x01]);
        chunk(&mut file, b"IEND", &[]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.png");
        std::fs::write(&path, file).unwrap();
        assert_eq!(super::dimensions(&path), Some((20000, 10000)));
    }

    /// A 2x1 JPEG whose EXIF says Orientation 6 (rotate 90 clockwise).
    fn rotated_jpeg(path: &Path) {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .write_image(&[200, 0, 0, 0, 0, 200], 2, 1, ExtendedColorType::Rgb8)
            .unwrap();
        // Big-endian TIFF: one IFD entry, Orientation (0x0112), SHORT, 1, 6.
        let mut exif = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01".to_vec();
        exif.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0]);
        let mut app1 = vec![0xff, 0xe1];
        app1.extend(((exif.len() + 2) as u16).to_be_bytes());
        app1.extend(exif);
        jpeg.splice(2..2, app1);
        std::fs::write(path, jpeg).unwrap();
    }

    #[test]
    fn an_exif_rotated_jpeg_opens_upright() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("rotated.jpg");
        rotated_jpeg(&path);
        let im = super::open(&path, Cancel::NEVER)?;
        assert_eq!((im.width, im.height), (1, 2));
        assert_eq!((im.metadata.width, im.metadata.height), (1, 2));
        assert_eq!(im.metadata.flip, 0);
        assert_eq!(im.pixels.len(), 2);
        Ok(())
    }

    #[test]
    fn a_cancelled_open_stops() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("a.png");
        image::RgbImage::new(4, 4).save(&path)?;
        let cancel = std::sync::atomic::AtomicBool::new(true);
        assert!(super::open(&path, Cancel::new(&cancel)).is_err());
        Ok(())
    }

    #[test]
    fn an_avif_decodes_upright_by_its_rotation_and_mirror() {
        // irot 1: a quarter turn anticlockwise; the drawing's top-right (green)
        // becomes the top-left.
        let rotated = decode(&fixture("rotated.avif")).unwrap();
        assert_eq!(rotated.dimensions(), (12, 16));
        assert_eq!(
            corners(&rotated),
            [[0, 255, 0], [255, 255, 255], [255, 0, 0], [0, 0, 255]]
        );
        // Then imir 0: top and bottom swap, as libheif shows it.
        let mirrored = decode(&fixture("rotated-mirrored.avif")).unwrap();
        assert_eq!(
            corners(&mirrored),
            [[255, 0, 0], [0, 0, 255], [0, 255, 0], [255, 255, 255]]
        );
    }

    #[test]
    fn a_ten_bit_avif_keeps_its_depth_and_colours() -> anyhow::Result<()> {
        assert_eq!(corners(&decode(&fixture("ten-bit.avif"))?), UPRIGHT);
        let im = super::open(&fixture("ten-bit.avif"), Cancel::NEVER)?;
        let red = im.pixels[16 + 1];
        assert!(red[0] > 0.4 && red[1] < 0.1 && red[2] < 0.1, "{red:?}");
        Ok(())
    }

    #[test]
    fn a_flat_ten_bit_420_avif_is_flat_in_every_column() {
        // yuv 0.8.19's NEON bilinear 4:2:0 above 8 bits weights every other
        // pixel's chroma 22/16, so on ARM a flat colour came out striped.
        let image = decode(&fixture("ten-bit-420.avif")).unwrap();
        let want = [200, 80, 40];
        for (x, y, p) in image.enumerate_pixels() {
            assert!(
                p.0.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 3),
                "({x}, {y}) is {:?}, not near {want:?}",
                p.0
            );
        }
    }

    #[test]
    fn an_avifs_cicp_colours_are_converted() {
        // Lossless P3 (nclx 12/13/0, identity matrix): the green corner is P3
        // green, outside sRGB, so it comes out as the most saturated sRGB green
        // with no red; without CICP it would be (0, 255, 0) too, so check the
        // grey (128) instead, which P3 and sRGB share: and a P3 mid-red from the
        // red corner, whose red stays 255 but whose green must stay 0.
        // Both within 1: the CMS rounds.
        let image = decode(&fixture("p3.avif")).unwrap();
        let grey = image.get_pixel(8, 6).0;
        assert!(grey.iter().all(|v| v.abs_diff(128) <= 1), "{grey:?}");
        let [red, ..] = corners(&image);
        assert_corners_near([red; 4], [[255, 0, 0]; 4], 1);
        // And the profile Develop sees is P3, not sRGB.
        let super::Prepared { profile, .. } = super::prepare(&fixture("p3.avif")).unwrap();
        let p3 = moxcms::ColorProfile::new_display_p3();
        let profile = profile.expect("a profile from CICP");
        assert!((profile.red_colorant.x - p3.red_colorant.x).abs() < 1e-3);
    }

    // libavif writes an image sequence with a primary still item as well,
    // which the Library shows.
    #[test]
    fn animated_and_tiled_avifs_are_refused_for_develop_and_hdr_ones_open() {
        assert!(super::probe(&fixture("pq.avif")).is_ok());
        assert!(
            super::open(&fixture("pq.avif"), Cancel::NEVER)
                .unwrap()
                .hdr()
                .is_some()
        );
        for (name, reason) in [("animated.avif", "animated"), ("tiled.avif", "tiled AVIF")] {
            assert_eq!(
                super::probe(&fixture(name)).unwrap_err().0,
                reason,
                "{name}"
            );
        }
        assert!(decode(&fixture("pq.avif")).is_ok());
        assert!(decode(&fixture("animated.avif")).is_ok());
        assert!(decode(&fixture("tiled.avif")).is_err());
    }

    /// `imir` associated before `irot` mirrors first, then turns, as libheif
    /// shows it.
    #[test]
    fn an_avif_mirrored_before_its_rotation_is_turned_after() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mirrored-first.avif");
        let mut bytes = std::fs::read(fixture("rotated-mirrored.avif")).unwrap();
        // The primary item's last two associations: irot (5), imir (6), both
        // essential.
        let at = bytes.windows(2).rposition(|w| w == [0x85, 0x86]).unwrap();
        bytes[at..at + 2].copy_from_slice(&[0x86, 0x85]);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(
            corners(&decode(&path).unwrap()),
            [[255, 255, 255], [0, 255, 0], [0, 0, 255], [255, 0, 0]]
        );
    }

    /// BT.2020 constant luminance is not the BT.2020 matrix, and isn't
    /// converted: the image can't be opened rather than shown in wrong colours.
    #[test]
    fn an_avif_in_constant_luminance_is_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("constant-luminance.avif");
        let mut bytes = std::fs::read(fixture("p3.avif")).unwrap();
        // nclx: primaries 12, transfer 13, matrix 0 becomes 10.
        let at = bytes.windows(4).position(|w| w == b"nclx").unwrap() + 8;
        bytes[at..at + 2].copy_from_slice(&10u16.to_be_bytes());
        std::fs::write(&path, bytes).unwrap();
        let error = decode(&path).unwrap_err().to_string();
        assert!(error.contains("constant luminance"), "{error}");
    }

    /// YCgCo, ICtCp and other matrices `yuv` has no conversion for, and
    /// reserved codes, are not taken as BT.601: the image can't be opened
    /// rather than shown in wrong colours.
    #[test]
    fn an_avif_in_a_matrix_it_does_not_convert_is_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        for code in [3u16, 8, 11, 12, 14, 255, 256] {
            let path = dir.path().join(format!("matrix-{code}.avif"));
            let mut bytes = std::fs::read(fixture("p3.avif")).unwrap();
            // nclx: primaries 12, transfer 13, matrix 0 becomes `code`.
            let at = bytes.windows(4).position(|w| w == b"nclx").unwrap() + 8;
            bytes[at..at + 2].copy_from_slice(&code.to_be_bytes());
            std::fs::write(&path, bytes).unwrap();
            let error = decode(&path).unwrap_err().to_string();
            assert!(error.contains("matrix coefficients"), "{code}: {error}");
        }
    }

    /// An image sequence is refused whichever of its brands says so, not
    /// only the major one.
    #[test]
    fn an_avif_listing_the_sequence_brand_is_refused_as_animated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sequence.avif");
        let mut bytes = std::fs::read(fixture("rotated.avif")).unwrap();
        // Major brand avif; compatible avif, mif1, avis (in miaf's place), MA1A.
        let at = bytes.windows(4).position(|w| w == b"miaf").unwrap();
        bytes[at..at + 4].copy_from_slice(b"avis");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(super::probe(&path).unwrap_err().0, "animated");
    }

    /// A file cut short says what it is and what is wrong with it, once.
    #[test]
    fn a_truncated_file_says_what_is_wrong_once() {
        let dir = tempfile::tempdir().unwrap();
        for (name, message) in [
            (
                "metadata.jxl",
                "Could not read the JPEG XL file: reader ended before parsing first frame",
            ),
            (
                "metadata.webp",
                "Could not read the WebP file: An expected chunk was missing",
            ),
        ] {
            let bytes = std::fs::read(fixture(name)).unwrap();
            let path = dir.path().join(name);
            std::fs::write(&path, &bytes[..bytes.len() / 3]).unwrap();
            assert_eq!(format!("{:#}", decode(&path).unwrap_err()), message);
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let opened = super::open(&path, Cancel::new(&cancel)).err().unwrap();
            assert_eq!(format!("{opened:#}"), message);
        }
    }

    /// SMPTE ST 2084 and BT.2100 HLG, written out apart from RAWmakase's own: a
    /// neutral signal's display light, 1 = graphics white (203 cd/m²).
    fn bt2100_grey(transfer: &str, signal: f64) -> f64 {
        match transfer {
            "pq" => {
                let (m1, m2) = (2610. / 16384., 2523. / 4096. * 128.);
                let (c1, c2, c3) = (3424. / 4096., 2413. / 128., 2392. / 128.);
                let p = signal.powf(1. / m2);
                10_000. * ((p - c1).max(0.) / (c2 - c3 * p)).powf(1. / m1) / 203.
            }
            _ => {
                let (a, b, c) = (0.17883277, 0.28466892, 0.55991073);
                let scene = if signal <= 0.5 {
                    signal * signal / 3.
                } else {
                    (((signal - c) / a).exp() + b) / 12.
                };
                // The OOTF of a neutral: its luminance is itself.
                scene.powf(1.2) * 1000. / 203.
            }
        }
    }

    /// An HDR file's preview shows HDR reference white (203 nits, ITU-R BT.2408) as
    /// white, rather than at the 2% of PQ's 10,000 nits it is, which looked nearly
    /// black; brighter light rolls off into white. In Develop it opens as HDR, its
    /// light as BT.2100 decodes it.
    #[test]
    fn hdr_previews_show_reference_white_as_white() {
        // The fixtures' grey is code value 128/255: 95 nits in PQ, and in HLG
        // 5% of its nominal 1,000-nit peak; their white is the top code.
        for (name, grey) in [
            ("pq.avif", 183),
            ("pq.jxl", 183),
            ("hlg.avif", 138),
            ("hlg.jxl", 138),
        ] {
            let image = decode(&fixture(name)).unwrap();
            let (w, h) = image.dimensions();
            let centre = image.get_pixel(w / 2, h / 2).0;
            assert!(
                centre.iter().all(|v| v.abs_diff(grey) <= 3),
                "{name}: {centre:?}"
            );
            let white = corners(&image)[3];
            assert!(white.iter().all(|v| *v >= 250), "{name}: {white:?}");
            assert!(super::probe(&fixture(name)).is_ok(), "{name}");
            let opened = super::open(&fixture(name), Cancel::NEVER).unwrap();
            let light = opened.hdr().expect("an HDR light");
            let centre = light.pixels()[(h / 2 * w + w / 2) as usize];
            let want = bt2100_grey(&name[..name.find('.').unwrap()], 128. / 255.);
            // The channels' mean within 3%: the fixtures are lossy (JPEG XL at
            // distance 1, AVIF through 10-bit YUV), which moves each channel a little.
            let mean = centre.iter().map(|v| *v as f64).sum::<f64>() / 3.;
            assert!(
                (mean / want - 1.).abs() < 0.03,
                "{name}: {centre:?} against {want}"
            );
        }
    }

    /// A one-row TIFF of `samples` (native-endian bytes), with `icc` when given: a
    /// small image, whose profile the `image` crate alone would lose.
    fn tiff(path: &Path, samples: &[u8], color: ExtendedColorType, icc: Option<Vec<u8>>) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = image::codecs::tiff::TiffEncoder::new(file);
        if let Some(icc) = icc {
            encoder.set_icc_profile(icc).unwrap();
        }
        let bytes_per_pixel = match color {
            ExtendedColorType::Rgb16 => 6,
            ExtendedColorType::Rgb32F => 12,
            _ => unreachable!(),
        };
        let width = (samples.len() / bytes_per_pixel) as u32;
        encoder.write_image(samples, width, 1, color).unwrap();
    }

    /// RAWmakase's own HDR TIFFs (PQ and HLG in 16 bits, linear in 32-bit float),
    /// small ones too, and a float TIFF without a profile open in Develop as HDR:
    /// their light is what was encoded, within 16-bit quantisation (exactly in
    /// float), negative channels kept. Their previews show graphics white (203 nits)
    /// as white, brighter light rolled off rather than clipped, and a colour the same
    /// in every encoding: HLG's system gamma works on luminance, not on each channel.
    #[test]
    fn hdr_tiffs_open_as_hdr_and_preview_white_as_white() {
        use rawmakase_model::color::hdr_output::{HdrGamut, HdrOutput, HdrTransfer};
        let dir = tempfile::tempdir().unwrap();
        // Grey, graphics white and two stops above it, neutral; then a colour.
        let pixels = [[0.18f32; 3], [1.; 3], [4.; 3], [0.3, 0.15, 0.08]];
        let check = |path: &Path, what: &str, sixteen: bool| -> Vec<[u8; 3]> {
            assert!(super::probe(path).is_ok(), "{what}");
            let opened = super::open(path, Cancel::NEVER).unwrap();
            let light = opened.hdr().unwrap_or_else(|| panic!("{what}: no light"));
            let tolerance = if sixteen { 3e-3 } else { 1e-5 };
            for (got, want) in light.pixels().iter().zip(&pixels) {
                for c in 0..3 {
                    assert!(
                        (got[c] - want[c]).abs() <= tolerance * want[c].abs().max(0.05),
                        "{what}: {got:?} against {want:?}"
                    );
                }
            }
            let preview = decode(path).unwrap();
            let px: Vec<[u8; 3]> = (0..4).map(|x| preview.get_pixel(x, 0).0).collect();
            // 0.18 is below the preview's roll-off: sRGB 118.
            assert!(px[0].iter().all(|v| v.abs_diff(118) <= 3), "{what}: {px:?}");
            assert!(px[1][1] > px[0][1] + 100, "{what}: {px:?}");
            assert!(
                px[2][1] > px[1][1],
                "{what}: light above white clipped: {px:?}"
            );
            px
        };
        let mut colours = Vec::new();
        for transfer in HdrTransfer::ALL {
            let output = HdrOutput {
                gamut: HdrGamut::Rec2020,
                transfer,
                ..Default::default()
            };
            let encoded: Vec<f32> = pixels.iter().flat_map(|p| output.encode(*p, 16.)).collect();
            let path = dir.path().join(format!("{transfer:?}.tif"));
            let icc = Some(crate::color_management::hdr_profile(output).unwrap());
            let sixteen = transfer != HdrTransfer::Linear;
            if sixteen {
                let samples: Vec<u16> = encoded
                    .iter()
                    .map(|v| (v.clamp(0., 1.) * 65535. + 0.5) as u16)
                    .collect();
                tiff(
                    &path,
                    bytemuck::cast_slice(&samples),
                    ExtendedColorType::Rgb16,
                    icc,
                );
            } else {
                let bytes: &[u8] = bytemuck::cast_slice(&encoded);
                tiff(&path, bytes, ExtendedColorType::Rgb32F, icc);
            }
            colours.push((transfer, check(&path, &format!("{transfer:?}"), sixteen)[3]));
        }
        let (_, pq) = colours[0];
        for (transfer, colour) in &colours {
            assert!(
                colour.iter().zip(pq).all(|(a, b)| a.abs_diff(b) <= 4),
                "{transfer:?} {colour:?} against PQ {pq:?}"
            );
        }
        // A float TIFF without a profile: linear, sRGB primaries, as Photoshop's
        // 32-bit documents are read. Its light is in ProPhoto.
        let path = dir.path().join("float.tif");
        let to_srgb = HdrGamut::Rec709.prophoto_to_rgb();
        let samples: Vec<f32> = pixels
            .iter()
            .flat_map(|p| rawmakase_model::color::mul(to_srgb, *p))
            .collect();
        tiff(
            &path,
            bytemuck::cast_slice(&samples),
            ExtendedColorType::Rgb32F,
            None,
        );
        check(&path, "float without a profile", false);
        // Signed light stays signed.
        let path = dir.path().join("signed.tif");
        tiff(
            &path,
            bytemuck::cast_slice(&[-0.25f32, 0.5, 0.5]),
            ExtendedColorType::Rgb32F,
            None,
        );
        let opened = super::open(&path, Cancel::NEVER).unwrap();
        let light = opened.hdr().unwrap().pixels()[0];
        let back = rawmakase_model::color::mul(to_srgb, light);
        assert!((back[0] + 0.25).abs() < 1e-5, "{back:?}");
    }

    /// A float TIFF with a linear profile without CICP (Photoshop's 32-bit
    /// documents) keeps its light above white, as one without a profile does.
    #[test]
    fn a_float_tiff_with_a_linear_profile_keeps_light_above_white() {
        use moxcms::ToneReprCurve::{Lut, Parametric};
        let dir = tempfile::tempdir().unwrap();
        let samples = [1f32, 1., 1., 4., 4., 4., 2., 1., 0.5];
        let plain = dir.path().join("plain.tif");
        tiff(
            &plain,
            bytemuck::cast_slice(&samples),
            ExtendedColorType::Rgb32F,
            None,
        );
        let px = |path: &Path| {
            let preview = decode(path).unwrap();
            (0..3)
                .map(|x| preview.get_pixel(x, 0).0)
                .collect::<Vec<_>>()
        };
        let without = px(&plain);
        assert!(without[1][1] > without[0][1], "{without:?}");
        // The identity, as each table and parametric form writes it.
        let identities = [
            Lut(vec![]),
            Lut(vec![256]),
            Lut(vec![0, 65535]),
            Parametric(vec![1.]),
            Parametric(vec![1., 1., 0.]),
            Parametric(vec![1., 1., 0., 0.]),
            Parametric(vec![1., 1., 0., 1., 0.]),
            Parametric(vec![1., 1., 0., 1., 0., 0., 0.]),
            // Offsets that cancel.
            Parametric(vec![1., 1., 0.25, -0.25]),
            Parametric(vec![1., 1., 0.25, 1., 0.5, -0.25, 0.]),
        ];
        // Near the identity but not it: a gamma just above 1, and a table that
        // bends between samples.
        let mut bent: Vec<u16> = (0..4097).map(|i| (i * 65535 / 4096) as u16).collect();
        bent[128] += 1300;
        // A type-2 curve whose break is above 0: the constant c below it.
        let raised = Parametric(vec![1., 1., -0.25, 0.25]);
        for curve in [Parametric(vec![1.001]), Lut(bent), raised] {
            assert!(!super::is_identity(&curve), "{curve:?}");
        }
        for (i, curve) in identities.into_iter().enumerate() {
            let mut linear = moxcms::ColorProfile::new_srgb();
            for trc in [
                &mut linear.red_trc,
                &mut linear.green_trc,
                &mut linear.blue_trc,
            ] {
                *trc = Some(curve.clone());
            }
            linear.cicp = None;
            let path = dir.path().join(format!("linear-{i}.tif"));
            let icc = Some(linear.encode().unwrap());
            tiff(
                &path,
                bytemuck::cast_slice(&samples),
                ExtendedColorType::Rgb32F,
                icc,
            );
            let with = px(&path);
            for (a, b) in with.iter().zip(&without) {
                assert!(
                    a.iter().zip(b).all(|(a, b)| a.abs_diff(*b) <= 2),
                    "{curve:?}: {with:?} {without:?}"
                );
            }
        }
    }

    /// An HDR AVIF that is also an image sequence stays refused as one: lifting the
    /// HDR refusal doesn't lift the animation's.
    #[test]
    fn an_animated_hdr_avif_is_still_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(fixture("pq.avif")).unwrap();
        // The ftyp box's major brand, at bytes 8–12: `avif` made `avis`.
        assert_eq!(&bytes[8..12], b"avif");
        bytes[11] = b's';
        let path = dir.path().join("sequence.avif");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(super::probe(&path).unwrap_err().0, "animated");
    }

    /// An HDR file's SDR rendition is the same in the Library and in Develop, signed
    /// light included: both clip it at 0 before the tone map.
    #[test]
    fn an_hdr_files_sdr_rendition_is_the_librarys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("signed.tif");
        let to_srgb = rawmakase_model::color::hdr_output::HdrGamut::Rec709.prophoto_to_rgb();
        let light = [[-0.25f32, 0.3, 0.1], [2., 0.5, 0.1]];
        let samples: Vec<f32> = light
            .iter()
            .flat_map(|p| rawmakase_model::color::mul(to_srgb, *p))
            .collect();
        tiff(
            &path,
            bytemuck::cast_slice(&samples),
            ExtendedColorType::Rgb32F,
            None,
        );
        let preview = decode(&path).unwrap();
        let opened = super::open(&path, Cancel::NEVER).unwrap();
        let develop = super::linear_pro_photo_to_srgb(&opened.pixels, 2, 1);
        assert_eq!(preview.as_raw(), develop.as_raw());
    }

    /// A PNG whose cICP chunk says PQ, without an ICC profile, opens as HDR: graphics
    /// white is its light's 1.
    #[test]
    fn a_pq_png_marked_by_its_cicp_chunk_opens_as_hdr() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.png");
        // PQ's graphics white (203 cd/m²) is signal 0.5806: code 38050 of 65535.
        let samples: Vec<u8> = [38050u16; 3].iter().flat_map(|v| v.to_ne_bytes()).collect();
        png(&plain, &samples, ExtendedColorType::Rgb16, None);
        let mut bytes = std::fs::read(&plain).unwrap();
        // cICP: BT.2020, PQ, identity matrix, full range; after IHDR (8 + 25 bytes).
        let data = [9u8, 16, 0, 1];
        bytes.splice(33..33, png_chunk(b"cICP", &data));
        let path = dir.path().join("pq.png");
        std::fs::write(&path, bytes).unwrap();
        assert!(super::probe(&path).is_ok());
        let opened = super::open(&path, Cancel::NEVER).unwrap();
        let light = opened.hdr().expect("an HDR light").pixels()[0];
        assert!(light.iter().all(|v| (v - 1.).abs() < 2e-3), "{light:?}");
        // After many metadata chunks it is read all the same.
        let mut later = std::fs::read(&plain).unwrap();
        let mut texts = Vec::new();
        for i in 0..80 {
            texts.extend(png_chunk(b"tEXt", format!("k{i}\0v").as_bytes()));
        }
        texts.extend(png_chunk(b"cICP", &data));
        later.splice(33..33, texts);
        let path = dir.path().join("later.png");
        std::fs::write(&path, later).unwrap();
        assert!(super::open(&path, Cancel::NEVER).unwrap().hdr().is_some());
        // A text chunk larger than the png crate's own default budget (64 MiB) is
        // read within `image`'s, cICP and all.
        let mut large = std::fs::read(&plain).unwrap();
        let mut text = b"k\0".to_vec();
        text.resize(66 << 20, b'v');
        let mut chunks = png_chunk(b"tEXt", &text);
        chunks.extend(png_chunk(b"cICP", &data));
        large.splice(33..33, chunks);
        let path = dir.path().join("large.png");
        std::fs::write(&path, large).unwrap();
        assert!(super::open(&path, Cancel::NEVER).unwrap().hdr().is_some());
        // A damaged cICP is dropped, as the decoder drops it: the file is sRGB.
        let mut damaged = std::fs::read(&plain).unwrap();
        let mut chunk = png_chunk(b"cICP", &data);
        let last = chunk.len() - 1;
        chunk[last] ^= 0xff;
        damaged.splice(33..33, chunk);
        let path = dir.path().join("damaged.png");
        std::fs::write(&path, damaged).unwrap();
        assert!(super::open(&path, Cancel::NEVER).unwrap().hdr().is_none());
        // Narrow range isn't expanded: refused for Develop.
        let mut narrow = std::fs::read(&plain).unwrap();
        narrow.splice(33..33, png_chunk(b"cICP", &[1, 1, 0, 0]));
        let path = dir.path().join("narrow.png");
        std::fs::write(&path, narrow).unwrap();
        assert_eq!(super::probe(&path).unwrap_err().0, "narrow-range");
    }

    /// An APNG, HDR or not, is an animation: refused for Develop as other animated
    /// files are, though the Library shows its default image.
    #[test]
    fn an_apng_is_refused_for_develop() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.png");
        png(&plain, &[0, 0, 0], ExtendedColorType::Rgb8, None);
        let mut bytes = std::fs::read(&plain).unwrap();
        // acTL: two frames, played once; before the image data.
        let mut actl = 2u32.to_be_bytes().to_vec();
        actl.extend_from_slice(&0u32.to_be_bytes());
        bytes.splice(33..33, png_chunk(b"acTL", &actl));
        let path = dir.path().join("animated.png");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(super::probe(&path).unwrap_err().0, "animated");
    }

    /// A PNG chunk of `kind` holding `data`, with its length and CRC.
    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
        chunk.extend_from_slice(kind);
        chunk.extend_from_slice(data);
        chunk.extend_from_slice(&crc32(&chunk[4..]).to_be_bytes());
        chunk
    }

    /// PNG's CRC-32 (ISO 3309).
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for b in bytes {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A small BigTIFF's profile is read too: a PQ one opens as HDR.
    #[test]
    fn a_small_pq_bigtiff_is_hdr() {
        use rawmakase_model::color::hdr_output::HdrOutput;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pq.tif");
        let icc = crate::color_management::hdr_profile(HdrOutput::default()).unwrap();
        let mut encoder =
            tiff::encoder::TiffEncoder::new_big(std::fs::File::create(&path).unwrap()).unwrap();
        let mut image = encoder
            .new_image::<tiff::encoder::colortype::RGB16>(1, 1)
            .unwrap();
        image
            .encoder()
            .write_tag(tiff::tags::Tag::IccProfile, icc.as_slice())
            .unwrap();
        image.write_data(&[38050u16; 3]).unwrap();
        assert!(super::probe(&path).is_ok());
        assert!(super::open(&path, Cancel::NEVER).unwrap().hdr().is_some());
    }

    /// A float TIFF with a profile whose curve isn't linear is converted as the
    /// profile says: a bright pixel elsewhere doesn't darken the rest, as scaling
    /// through a curve would.
    #[test]
    fn a_float_tiff_with_a_curved_profile_keeps_each_pixel_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let srgb = moxcms::ColorProfile::new_srgb().encode().unwrap();
        let preview = |name: &str, samples: [f32; 6]| {
            let path = dir.path().join(name);
            tiff(
                &path,
                bytemuck::cast_slice(&samples),
                ExtendedColorType::Rgb32F,
                Some(srgb.clone()),
            );
            decode(&path).unwrap().get_pixel(0, 0).0
        };
        let alone = preview("alone.tif", [0.5; 6]);
        let beside_bright = preview("bright.tif", [0.5, 0.5, 0.5, 4., 4., 4.]);
        assert_eq!(alone, beside_bright);
    }

    #[test]
    fn an_odd_sized_420_avif_decodes_every_pixel() {
        let image = decode(&fixture("odd-420.avif")).unwrap();
        assert_eq!(image.dimensions(), (15, 11));
        // The bottom-right corner block reaches the last row and column.
        let last = image.get_pixel(14, 10).0;
        assert!(last.iter().all(|v| *v > 200), "{last:?}");
    }

    #[test]
    fn a_truncated_avif_is_an_error_not_a_hang() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = std::fs::read(fixture("ten-bit.avif")).unwrap();
        for keep in [bytes.len() - 10, bytes.len() * 3 / 4, 40] {
            let path = dir.path().join(format!("cut-{keep}.avif"));
            std::fs::write(&path, &bytes[..keep]).unwrap();
            assert!(decode(&path).is_err(), "{keep}");
        }
    }

    #[test]
    fn a_corrupt_avif_is_an_error_not_an_abort() {
        // One flipped bit in the AV1 data that made rav1d panic inside its C
        // interface, which aborted the whole app.
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(fixture("rotated.avif")).unwrap();
        assert_eq!(bytes[325], 0x5c, "rotated.avif was regenerated");
        bytes[325] = 0x54;
        let path = dir.path().join("corrupt.avif");
        std::fs::write(&path, bytes).unwrap();
        assert!(decode(&path).is_err());
    }

    #[test]
    fn an_avif_over_150_megapixels_is_refused_from_its_header() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(fixture("ten-bit.avif")).unwrap();
        // Rewrite the ispe property to 20000 x 10000.
        let at = bytes.windows(4).position(|w| w == b"ispe").unwrap() + 8;
        bytes[at..at + 4].copy_from_slice(&20_000u32.to_be_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&10_000u32.to_be_bytes());
        let path = dir.path().join("huge.avif");
        std::fs::write(&path, bytes).unwrap();
        let err = decode(&path).unwrap_err().to_string();
        assert!(err.contains("too large"), "{err}");
    }

    /// libheif's own HEIC: 4:2:0 in a 64×64 picture it crops (`clap`) to the
    /// 16×12 drawing.
    #[test]
    fn a_heic_decodes_cropped_with_its_colours() {
        if !heic_decodes() {
            return;
        }
        assert_eq!(super::dimensions(&fixture("metadata.heic")), Some((16, 12)));
        let image = decode(&fixture("metadata.heic")).unwrap();
        assert_eq!(image.dimensions(), (16, 12));
        assert_corners_near(corners(&image), UPRIGHT, 4);
        let grey = image.get_pixel(8, 6).0;
        assert!(grey.iter().all(|v| v.abs_diff(128) <= 2), "{grey:?}");
        assert!(super::probe(&fixture("metadata.heic")).is_ok());
    }

    /// A grid as an iPhone writes one: four tiles laid in place and cut to
    /// the grid's size, each converted by the matrix and range of its own
    /// `nclx` (BT.709, limited range), as the grid has only an ICC profile.
    #[test]
    fn a_heic_grid_is_laid_out_and_cut_to_its_size() {
        if !heic_decodes() {
            return;
        }
        assert_eq!(super::dimensions(&fixture("grid.heic")), Some((120, 100)));
        let image = decode(&fixture("grid.heic")).unwrap();
        assert_eq!(image.dimensions(), (120, 100));
        assert_corners_near(corners(&image), UPRIGHT, 4);
        // Grey either side of the tiles' edges, and the last pixel white.
        for (x, y) in [(63, 50), (64, 50), (60, 63), (60, 64)] {
            let grey = image.get_pixel(x, y).0;
            assert!(
                grey.iter().all(|v| v.abs_diff(128) <= 2),
                "{x},{y}: {grey:?}"
            );
        }
        let last = image.get_pixel(119, 99).0;
        assert!(last.iter().all(|v| *v >= 250), "{last:?}");
    }

    #[test]
    fn a_heic_decodes_upright_by_its_rotation_and_mirror() {
        if !heic_decodes() {
            return;
        }
        // As rotated.avif: irot 1 brings the top-right (green) to the top-left,
        // after the crop.
        assert_eq!(super::dimensions(&fixture("rotated.heic")), Some((12, 16)));
        // 4:2:0, as phones write, so near.
        let rotated = decode(&fixture("rotated.heic")).unwrap();
        assert_eq!(rotated.dimensions(), (12, 16));
        assert_corners_near(
            corners(&rotated),
            [[0, 255, 0], [255, 255, 255], [255, 0, 0], [0, 0, 255]],
            4,
        );
        let mirrored = decode(&fixture("rotated-mirrored.heic")).unwrap();
        assert_corners_near(
            corners(&mirrored),
            [[255, 0, 0], [0, 0, 255], [0, 255, 0], [255, 255, 255]],
            4,
        );
    }

    #[test]
    fn a_ten_bit_heic_keeps_its_depth_and_colours() -> anyhow::Result<()> {
        if !heic_decodes() {
            return Ok(());
        }
        assert_corners_near(corners(&decode(&fixture("ten-bit.heic"))?), UPRIGHT, 1);
        let super::Prepared { decoder, .. } = super::prepare(&fixture("ten-bit.heic"))?;
        assert_eq!(decoder.color_type(), image::ColorType::Rgb16);
        let im = super::open(&fixture("ten-bit.heic"), Cancel::NEVER)?;
        assert_eq!((im.width, im.height), (16, 12));
        let red = im.pixels[16 + 1];
        assert!(red[0] > 0.4 && red[1] < 0.1 && red[2] < 0.1, "{red:?}");
        Ok(())
    }

    #[test]
    fn a_heics_cicp_colours_are_converted() {
        if !heic_decodes() {
            return;
        }
        // As p3.avif, but 4:2:0.
        let image = decode(&fixture("p3.heic")).unwrap();
        let grey = image.get_pixel(8, 6).0;
        assert!(grey.iter().all(|v| v.abs_diff(128) <= 1), "{grey:?}");
        let [red, ..] = corners(&image);
        assert_corners_near([red; 4], [[255, 0, 0]; 4], 1);
        let super::Prepared { profile, .. } = super::prepare(&fixture("p3.heic")).unwrap();
        let p3 = moxcms::ColorProfile::new_display_p3();
        let profile = profile.expect("a profile from CICP");
        assert!((profile.red_colorant.x - p3.red_colorant.x).abs() < 1e-3);
    }

    /// A PQ HEIC is shown, its reference white as white, but not edited; nor
    /// is a still in a file whose brands say it holds a sequence. The
    /// refusals come from the header, wherever the pixels come from; how an
    /// HDR picture's preview looks, and whether a system decoder shows a still
    /// it takes for a sequence, are its own.
    #[test]
    fn hdr_and_sequence_heics_are_refused_for_develop() {
        assert_eq!(super::probe(&fixture("pq.heic")).unwrap_err().0, "HDR");
        if cfg!(target_os = "macos") {
            // ImageIO's HDR decoding is its own, but the grey is shown neither
            // black nor clipped.
            let image = decode(&fixture("pq.heic")).unwrap();
            let centre = image.get_pixel(8, 6).0;
            assert!(centre.iter().all(|v| (16..=250).contains(v)), "{centre:?}");
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sequence.heic");
        let mut bytes = std::fs::read(fixture("metadata.heic")).unwrap();
        let at = bytes.windows(4).position(|w| w == b"miaf").unwrap();
        bytes[at..at + 4].copy_from_slice(b"msf1");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(super::probe(&path).unwrap_err().0, "animated");
    }

    /// A HEIC with alpha (an auxiliary image) is listed by its size; on macOS
    /// it is decoded composited over black, a transparent one as black rather
    /// than refused as a decode that drew nothing.
    #[test]
    fn a_heic_with_alpha_is_shown_over_black() {
        for name in ["transparent.heic", "half-alpha.heic"] {
            assert_eq!(super::dimensions(&fixture(name)), Some((16, 12)), "{name}");
        }
        if cfg!(target_os = "macos") {
            let image = decode(&fixture("transparent.heic")).unwrap();
            assert!(image.pixels().all(|p| p.0 == [0; 3]));
            let image = decode(&fixture("half-alpha.heic")).unwrap();
            assert_corners_near([image.get_pixel(8, 6).0; 4], [[128, 0, 0]; 4], 6);
        } else if heic_decodes() {
            // libheif's alpha is dropped: the colour, as an AVIF's.
            for name in ["transparent.heic", "half-alpha.heic"] {
                assert_eq!(decode(&fixture(name)).unwrap().dimensions(), (16, 12));
            }
        }
    }

    /// The size and the refusal come from a HEIC's header: its HEVC data is
    /// read only to decode it, and a picture cut short is an error on every
    /// platform, whatever a system decoder would make of it.
    #[test]
    fn a_heic_header_is_read_without_its_image_data() {
        let dir = tempfile::tempdir().unwrap();
        for (name, size) in [("rotated.heic", (12, 16)), ("grid.heic", (120, 100))] {
            let path = dir.path().join(name);
            let bytes = std::fs::read(fixture(name)).unwrap();
            std::fs::write(&path, &bytes[..bytes.len() - 20]).unwrap();
            assert_eq!(super::dimensions(&path), Some(size), "{name}");
            assert!(super::probe(&path).is_ok(), "{name}");
            assert!(decode(&path).is_err(), "{name}");
        }
    }

    #[test]
    fn a_truncated_or_corrupt_heic_is_an_error_not_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = std::fs::read(fixture("grid.heic")).unwrap();
        for keep in [bytes.len() - 10, bytes.len() * 3 / 4, bytes.len() / 2, 40] {
            let path = dir.path().join(format!("cut-{keep}.heic"));
            std::fs::write(&path, &bytes[..keep]).unwrap();
            assert!(decode(&path).is_err(), "{keep}");
        }
        // Every byte of a tile's HEVC data flipped in turn: an error or an
        // image, never a panic out of the decoder.
        let mdat = bytes.windows(4).rposition(|w| w == b"mdat").unwrap() + 4;
        let path = dir.path().join("corrupt.heic");
        for at in (mdat..bytes.len()).step_by(7) {
            let mut corrupt = bytes.clone();
            corrupt[at] ^= 0xff;
            std::fs::write(&path, corrupt).unwrap();
            let _ = decode(&path);
        }
    }

    #[test]
    fn a_heic_over_150_megapixels_is_refused_from_its_header() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(fixture("metadata.heic")).unwrap();
        // Rewrite the ispe property to 20000 x 10000.
        let at = bytes.windows(4).position(|w| w == b"ispe").unwrap() + 8;
        bytes[at..at + 4].copy_from_slice(&20_000u32.to_be_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&10_000u32.to_be_bytes());
        let path = dir.path().join("huge.heic");
        std::fs::write(&path, bytes).unwrap();
        let err = decode(&path).unwrap_err().to_string();
        assert!(err.contains("too large"), "{err}");
    }

    /// A grid whose tiles don't add up to it is not laid out wrongly.
    #[test]
    fn a_heic_grid_its_tiles_dont_cover_is_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(fixture("grid.heic")).unwrap();
        // The grid's description in idat: 2 x 2 tiles of 64 for 120 x 100;
        // claim 130 wide.
        let at = bytes
            .windows(8)
            .position(|w| w == [0, 0, 1, 1, 0, 120, 0, 100])
            .unwrap();
        bytes[at + 5] = 130;
        let path = dir.path().join("short.heic");
        std::fs::write(&path, bytes).unwrap();
        let err = decode(&path).unwrap_err().to_string();
        assert!(err.contains("cover"), "{err}");
    }

    /// A tile is laid as it is coded: one that says it is turned is not
    /// laid out wrongly.
    #[test]
    fn a_heic_grid_whose_tiles_are_turned_is_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(fixture("grid.heic")).unwrap();
        // The tiles' colr (nclx) becomes an irot.
        let at = bytes.windows(8).position(|w| w == b"colrnclx").unwrap();
        bytes[at..at + 4].copy_from_slice(b"irot");
        let path = dir.path().join("turned-tiles.heic");
        std::fs::write(&path, bytes).unwrap();
        let err = decode(&path).unwrap_err().to_string();
        assert!(err.contains("turned"), "{err}");
    }

    /// A grid may list one tile several times; each place shows it.
    #[test]
    fn a_heic_grid_listing_a_tile_twice_shows_it_twice() {
        if !heic_decodes() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(fixture("grid.heic")).unwrap();
        // dimg from the grid (1) to its four tiles (2 to 5): all to tile 2.
        let at = bytes
            .windows(12)
            .position(|w| w == [0, 1, 0, 4, 0, 2, 0, 3, 0, 4, 0, 5])
            .unwrap();
        bytes[at + 4..at + 12].copy_from_slice(&[0, 2, 0, 2, 0, 2, 0, 2]);
        let path = dir.path().join("one-tile.heic");
        std::fs::write(&path, bytes).unwrap();
        let image = decode(&path).unwrap();
        // The top-left tile's red corner, and its grey where the top-right
        // tile's green was.
        assert_corners_near([corners(&image)[0]; 4], [UPRIGHT[0]; 4], 4);
        let grey = corners(&image)[1];
        assert!(grey.iter().all(|v| v.abs_diff(128) <= 2), "{grey:?}");
    }

    /// The system decoder decodes the file whose header was checked, not
    /// whatever is at its path by then: a file replaced in between is not
    /// decoded with the first one's crop, orientation and colour.
    #[test]
    fn a_heic_replaced_after_its_header_was_read_decodes_as_first_read() {
        if !heic_decodes() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("IMG_0001.HEIC");
        std::fs::copy(fixture("grid.heic"), &path).unwrap();
        let super::Prepared { decoder, .. } = super::prepare(&path).unwrap();
        // Replaced as a sync client does: written aside, then renamed over.
        let aside = dir.path().join("replacement");
        std::fs::copy(fixture("metadata.heic"), &aside).unwrap();
        std::fs::rename(&aside, &path).unwrap();
        assert_eq!(decoder.dimensions(), (120, 100));
        let mut rgb = vec![0; decoder.total_bytes() as usize];
        decoder.read_image_boxed(&mut rgb).unwrap();
        let image = image::RgbImage::from_raw(120, 100, rgb).unwrap();
        assert_corners_near(corners(&image), UPRIGHT, 4);
    }

    /// Elsewhere, as on Linux, a HEIC is decoded by the system's libheif,
    /// which a system (CI's Ubuntu runners among them) may lack: it is then
    /// listed with its size and Develop opens it, but decoding says what to
    /// install.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    #[test]
    fn with_libheif_a_heic_decodes_and_without_it_says_what_to_install() {
        assert_eq!(super::dimensions(&fixture("grid.heic")), Some((120, 100)));
        assert!(super::probe(&fixture("grid.heic")).is_ok());
        match decode(&fixture("grid.heic")) {
            Ok(image) => {
                assert_eq!(image.dimensions(), (120, 100));
                assert_corners_near(corners(&image), UPRIGHT, 4);
            }
            Err(error) => {
                let error = format!("{error:#}");
                assert!(error.contains("libheif"), "{error}");
                assert!(super::heif_decoder().is_err());
                assert!(
                    std::env::var_os("RAWMAKASE_REQUIRE_HEIF").is_none(),
                    "{error}"
                );
            }
        }
    }

    /// On Windows a HEIC is decoded by Microsoft's HEIF and HEVC extensions,
    /// which a PC (CI's among them) may lack: it is then refused with what to
    /// install, and otherwise decoded as elsewhere.
    #[cfg(windows)]
    #[test]
    fn on_windows_a_heic_decodes_or_says_what_to_install() {
        match decode(&fixture("grid.heic")) {
            Ok(image) => {
                assert_eq!(image.dimensions(), (120, 100));
                assert_corners_near(corners(&image), UPRIGHT, 4);
            }
            Err(error) => {
                let error = format!("{error:#}");
                assert!(error.contains("HEVC Video Extensions"), "{error}");
            }
        }
        // The header is RAWmakase's to read either way.
        assert_eq!(super::dimensions(&fixture("grid.heic")), Some((120, 100)));
    }

    /// A header saying a picture is smaller than it is coded is held to: the
    /// image comes out no larger than the header says, or not at all. Its
    /// coded size, within 150 megapixels, is not refused itself, as encoders
    /// pad (Apple's codes 64×64 as 160×64).
    #[test]
    fn a_heic_coded_larger_than_its_header_says_is_no_larger_shown() {
        if heic_decodes() {
            let image = decode(&fixture("wide.heic")).unwrap();
            assert_eq!(image.dimensions(), (192, 128));
            assert_corners_near(corners(&image), UPRIGHT, 4);
        }
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(fixture("wide.heic")).unwrap();
        // ispe: 192 x 128 becomes 64 x 64.
        let at = bytes.windows(4).position(|w| w == b"ispe").unwrap() + 8;
        bytes[at..at + 8].copy_from_slice(&[0, 0, 0, 64, 0, 0, 0, 64]);
        let path = dir.path().join("small-header.heic");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(super::dimensions(&path), Some((64, 64)));
        match decode(&path) {
            Ok(image) => assert_eq!(image.dimensions(), (64, 64)),
            Err(error) => {
                let error = format!("{error:#}");
                assert!(
                    ["not the size it says", "libheif", "HEVC Video Extensions"]
                        .iter()
                        .any(|e| error.contains(e)),
                    "{error}"
                );
                // libheif refuses it itself, by its own check of the size.
                assert!(!cfg!(target_os = "macos") || error.contains("not the size"));
            }
        }
    }
}
