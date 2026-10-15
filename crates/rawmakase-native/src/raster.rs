//! Opening a JPEG, PNG or TIFF, the files RAWmakase shows without developing.
use anyhow::{Result, anyhow};
use image::{DynamicImage, ImageDecoder, RgbImage};
use moxcms::{
    ColorProfile, DataColorSpace, Layout, ToneReprCurve, TransformExecutor, TransformOptions,
};
use rawmakase_model::camera_data::{CameraImage, Metadata, Source};
use rayon::prelude::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

mod av1;
mod avif;
mod jxl;

/// What a file's first bytes say it is.
#[derive(Debug, PartialEq)]
enum Kind {
    Avif,
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
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brands = &head[8..head.len().min(64)];
        if brands
            .as_chunks::<4>()
            .0
            .iter()
            .any(|b| b == b"avif" || b == b"avis")
        {
            return Kind::Avif;
        }
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
}

fn prepare(path: &Path) -> Result<Prepared> {
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
    let (mut decoder, refused): (Box<dyn ImageDecoder>, Option<Unsupported>) =
        match sniff(&head[..n]) {
            Kind::Webp { animated } => (
                Box::new(
                    image::ImageReader::open(path)?
                        .with_guessed_format()?
                        .into_decoder()?,
                ),
                animated.then(|| Unsupported("animated".into())),
            ),
            Kind::Jxl => {
                let file = jxl::JxlFile::open(path)?;
                let refused = file.refusal();
                (Box::new(file), refused)
            }
            Kind::Avif => {
                let file = avif::AvifFile::open(path)?;
                let refused = file.refusal();
                (Box::new(file), refused)
            }
            _ => (
                Box::new(
                    image::ImageReader::open(path)?
                        .with_guessed_format()?
                        .into_decoder()?,
                ),
                None,
            ),
        };
    let (w, h) = decoder.dimensions();
    anyhow::ensure!(
        (w as u64) * (h as u64) <= 150_000_000,
        "Image too large to preview"
    );
    let orientation = decoder.orientation()?;
    let profile = decoder
        .icc_profile()
        .ok()
        .flatten()
        .and_then(|icc| ColorProfile::new_from_slice(&icc).ok());
    Ok(Prepared {
        decoder,
        orientation,
        profile,
        refused,
    })
}

/// The image at `path` decoded, turned upright and converted from its embedded
/// ICC profile to 8-bit sRGB; refused above 150 MP. A file without a profile,
/// or with one that cannot be read or does not describe its pixels (RGB, or
/// grey for a grey image), is taken to be sRGB already.
pub fn decode(path: &Path) -> Result<RgbImage> {
    let Prepared {
        decoder,
        orientation,
        profile,
        ..
    } = prepare(path)?;
    let transform = profile.and_then(|profile| ToSrgb::new(&profile, decoder.color_type()));
    let mut image = DynamicImage::from_decoder(decoder)?;
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
pub(crate) fn open(path: &Path, cancel: &AtomicBool) -> Result<CameraImage> {
    let Prepared {
        decoder,
        orientation,
        profile,
        refused,
    } = prepare(path)?;
    if let Some(refused) = refused {
        return Err(refused.into());
    }
    let profile = profile.unwrap_or_else(ColorProfile::new_srgb);
    let linear = to_linear(&profile, decoder.color_type())?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let (width, height) = (image.width(), image.height());
    let pixels = linear.apply(image, cancel)?;
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
    Ok(CameraImage {
        width,
        height,
        recovered: Default::default(),
        pixels,
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    })
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

/// A conversion from a file's profile to linear ProPhoto, in f32.
struct ToLinear {
    transform: std::sync::Arc<moxcms::TransformF32Executor>,
    gray: bool,
}

/// The conversion for pixels of type `color` in `profile`; RGB, or grey for a
/// colourless image. As in [`decode`], a profile that does not describe the
/// pixels (a grey one on a colour image) or that has no transform is taken to be
/// sRGB. Any other colour space is [`Unsupported`].
fn to_linear(profile: &ColorProfile, color: image::ColorType) -> Result<ToLinear> {
    // An empty curve is the identity in ICC: ProPhoto's primaries, no gamma.
    let mut linear_pro = ColorProfile::new_pro_photo_rgb();
    linear_pro.red_trc = Some(ToneReprCurve::Lut(vec![]));
    linear_pro.green_trc = Some(ToneReprCurve::Lut(vec![]));
    linear_pro.blue_trc = Some(ToneReprCurve::Lut(vec![]));
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
    })
}
impl ToLinear {
    /// `image`'s pixels as linear ProPhoto, a row at a time.
    fn apply(&self, image: DynamicImage, cancel: &AtomicBool) -> Result<Vec<[f32; 3]>> {
        let (width, height) = (image.width() as usize, image.height() as usize);
        let mut out = vec![[0f32; 3]; width * height];
        if out.is_empty() {
            return Ok(out);
        }
        let (samples, per_pixel) = if self.gray {
            (image.to_luma32f().into_raw(), 1)
        } else {
            (image.into_rgb32f().into_raw(), 3)
        };
        samples
            .par_chunks(width * per_pixel)
            .zip(out.par_chunks_mut(width))
            .try_for_each(|(from, to)| {
                anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Opening cancelled");
                self.transform
                    .transform(from, bytemuck::cast_slice_mut(to))
                    .map_err(|e| anyhow!("Could not convert the image to ProPhoto: {e}"))
            })?;
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
        let im = super::open(&fixture("sixteen-bit.jxl"), &Default::default())?;
        assert_eq!((im.width, im.height), (16, 12));
        // Red corner, linear ProPhoto: a strong red, no green or blue left.
        let red = im.pixels[16 + 1];
        assert!(red[0] > 0.4 && red[1] < 0.1 && red[2] < 0.1, "{red:?}");
        Ok(())
    }

    #[test]
    fn hdr_and_animated_jxls_show_but_are_refused_for_develop() {
        for (name, reason) in [("pq.jxl", "HDR"), ("animated.jxl", "animated")] {
            let path = fixture(name);
            assert!(decode(&path).is_ok(), "{name}");
            assert_eq!(super::probe(&path).unwrap_err().0, reason, "{name}");
        }
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
        let err = super::open(&path, &Default::default()).err().unwrap();
        assert_eq!(
            err.downcast_ref::<super::Unsupported>().unwrap().0,
            "animated"
        );
    }

    #[test]
    fn a_still_webp_opens_for_develop() -> anyhow::Result<()> {
        let im = super::open(&fixture("metadata.webp"), &Default::default())?;
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
        let im = super::open(&path, &Default::default())?;
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
        let im = super::open(&path, &Default::default())?;
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
        let im = super::open(&path, &Default::default())?;
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
        let opened = super::open(&path, &Default::default())?;
        let srgb = super::open(&plain, &Default::default())?;
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
        let im = super::open(&path, &Default::default())?;
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
        assert!(super::open(&path, &cancel).is_err());
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
        // Then mirrored left to right.
        let mirrored = decode(&fixture("rotated-mirrored.avif")).unwrap();
        assert_eq!(
            corners(&mirrored),
            [[255, 255, 255], [0, 255, 0], [0, 0, 255], [255, 0, 0]]
        );
    }

    #[test]
    fn a_ten_bit_avif_keeps_its_depth_and_colours() -> anyhow::Result<()> {
        assert_eq!(corners(&decode(&fixture("ten-bit.avif"))?), UPRIGHT);
        let im = super::open(&fixture("ten-bit.avif"), &Default::default())?;
        let red = im.pixels[16 + 1];
        assert!(red[0] > 0.4 && red[1] < 0.1 && red[2] < 0.1, "{red:?}");
        Ok(())
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
    fn hdr_animated_and_tiled_avifs_are_refused_for_develop() {
        for (name, reason) in [
            ("pq.avif", "HDR"),
            ("animated.avif", "animated"),
            ("tiled.avif", "tiled AVIF"),
        ] {
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
}
