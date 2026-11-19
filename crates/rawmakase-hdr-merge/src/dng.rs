//! The merged DNG: a linear raw image (PhotometricInterpretation 34892,
//! demosaiced camera RGB) of half-precision floating-point samples, as
//! Lightroom's own HDR merges store theirs, written by a little-endian TIFF
//! writer of our own ([`ifd`]); `rawmakase-tiff` stays the bounded reader.
//!
//! Layout (DNG Specification 1.7.1.0): IFD0 is the raw image, with every DNG
//! tag and the XMP packet, when there is one; its SubIFDs hold the
//! transparency mask (NewSubFileType 4) and the preview (NewSubFileType 1),
//! when there are. [`encoded`] makes everything but IFD0 itself, the expensive
//! part; [`Encoded::with`] then makes the file with the XMP and preview it
//! is given, IFD0 last, so the same merge can be written with and without
//! them. The image is 256 × 256 tiles of three interleaved 16-bit floats a
//! pixel, Deflate-compressed under Adobe's floating-point predictor 34894
//! (FloatingPointX2: each row's samples as byte planes, most significant
//! first, each byte the difference from the same byte two pixels before;
//! TIFF Technical Note 3 and the DNG SDK's `DecodeFPDelta`). DNGVersion and
//! DNGBackwardVersion are 1.4.0.0, the version that defined floating-point
//! samples, the predictor and transparency masks.
//!
//! Samples are black-subtracted camera RGB (BlackLevel 0, not white balanced)
//! under [`DngFacts::white_level`]; AsShotNeutral is the white balance. The
//! camera's maker note is kept in DNGPrivateData as Adobe DNG Converter keeps it
//! (an Adobe `MakN` record), not in the EXIF directory, so readers find it
//! where they find a converted DNG's.
mod ifd;

use crate::Error;
use ifd::{Ifd, Tiff, Value, kind};
use rawmakase_cancel::Cancel;
use rawmakase_interop::exif::{CameraExif, Field, tag as exif_tag};
use rayon::prelude::*;
use std::io::Write;

/// The tile size: square, a multiple of 16 as TIFF asks, even as predictor
/// 34894 asks.
pub const TILE: u32 = 256;

/// The largest WhiteLevel half-precision samples can reach (65504 is the
/// largest finite half).
pub const MAX_WHITE: u32 = 65504;

/// The image a merged DNG holds.
pub struct LinearImage<'a> {
    pub width: u32,
    pub height: u32,
    /// Camera RGB, black-subtracted and not white balanced, row by row, under
    /// [`DngFacts::white_level`]. Values below 0 or that are not numbers are
    /// written as 0, values above white as white.
    pub pixels: &'a [[f32; 3]],
    /// How much of each pixel the image covers, 0 (none) to 255, row by row;
    /// written as the DNG's transparency mask. `None` covers it all.
    pub coverage: Option<&'a [u8]>,
}

/// One of the camera's two calibrations (DNG's ColorMatrix1/2 and their kin).
#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    /// The light it was made under, as an EXIF LightSource code (17 standard
    /// light A, 21 D65).
    pub illuminant: u16,
    /// XYZ to camera RGB, row by row.
    pub color_matrix: [f64; 9],
    /// Camera RGB to XYZ (D50), white balanced, row by row.
    pub forward_matrix: Option<[f64; 9]>,
    /// The individual camera's calibration, row by row.
    pub camera_calibration: Option<[f64; 9]>,
}

/// The camera's maker note, as Adobe's DNGPrivateData keeps it: its bytes,
/// the byte order its file was in and its offset in that file, which its own
/// offsets may count from.
#[derive(Clone, Debug, PartialEq)]
pub struct MakerNote {
    pub bytes: Vec<u8>,
    pub little_endian: bool,
    pub offset: u32,
}

/// What a merged DNG says beside its samples.
#[derive(Clone, Debug, PartialEq)]
pub struct DngFacts {
    /// The camera's Make and Model, as its own files write them: they choose
    /// the camera profiles and lens profiles that fit.
    pub make: String,
    pub model: String,
    /// DNG's UniqueCameraModel: Adobe's name for the camera.
    pub unique_camera_model: String,
    pub software: String,
    /// The level the samples clip at (WhiteLevel), at most [`MAX_WHITE`].
    pub white_level: u32,
    /// BaselineExposure in stops, the camera's own included.
    pub baseline_exposure: f64,
    /// One or two calibrations, by illuminant.
    pub calibrations: Vec<Calibration>,
    /// CameraCalibrationSignature: which profiles the calibrations'
    /// [`Calibration::camera_calibration`] is for ("com.adobe" for Adobe's);
    /// `None` writes none.
    pub calibration_signature: Option<String>,
    /// Gains the samples carry (AnalogBalance); `None` for none.
    pub analog_balance: Option<[f64; 3]>,
    /// The as-shot white balance as camera RGB of a neutral (AsShotNeutral).
    pub as_shot_neutral: [f64; 3],
    /// EXIF Orientation, 1 to 8.
    pub orientation: u16,
    /// DefaultCropOrigin and DefaultCropSize: left, top, width, height in
    /// pixels; `None` for the whole image.
    pub default_crop: Option<[u32; 4]>,
    /// The EXIF directories, as read from the camera's file. Make, Model and
    /// a maker note in them are left out: [`Self::make`], [`Self::model`] and
    /// [`Self::maker_note`] say them.
    pub exif: CameraExif,
    pub maker_note: Option<MakerNote>,
}

/// TIFF and DNG tags this writer sets.
mod tag {
    pub(super) const NEW_SUBFILE_TYPE: u16 = 254;
    pub(super) const IMAGE_WIDTH: u16 = 256;
    pub(super) const IMAGE_LENGTH: u16 = 257;
    pub(super) const BITS_PER_SAMPLE: u16 = 258;
    pub(super) const COMPRESSION: u16 = 259;
    pub(super) const PHOTOMETRIC: u16 = 262;
    pub(super) const MAKE: u16 = 271;
    pub(super) const MODEL: u16 = 272;
    pub(super) const ORIENTATION: u16 = 274;
    pub(super) const SAMPLES_PER_PIXEL: u16 = 277;
    pub(super) const PLANAR_CONFIGURATION: u16 = 284;
    pub(super) const SOFTWARE: u16 = 305;
    pub(super) const STRIP_OFFSETS: u16 = 273;
    pub(super) const ROWS_PER_STRIP: u16 = 278;
    pub(super) const STRIP_BYTE_COUNTS: u16 = 279;
    pub(super) const XMP: u16 = 700;
    pub(super) const YCBCR_SUB_SAMPLING: u16 = 530;
    pub(super) const PREDICTOR: u16 = 317;
    pub(super) const TILE_WIDTH: u16 = 322;
    pub(super) const TILE_LENGTH: u16 = 323;
    pub(super) const TILE_OFFSETS: u16 = 324;
    pub(super) const TILE_BYTE_COUNTS: u16 = 325;
    pub(super) const SUB_IFDS: u16 = 330;
    pub(super) const SAMPLE_FORMAT: u16 = 339;
    pub(super) const EXIF_IFD: u16 = 34665;
    pub(super) const GPS_IFD: u16 = 34853;
    pub(super) const DNG_VERSION: u16 = 50706;
    pub(super) const DNG_BACKWARD_VERSION: u16 = 50707;
    pub(super) const UNIQUE_CAMERA_MODEL: u16 = 50708;
    pub(super) const BLACK_LEVEL: u16 = 50714;
    pub(super) const WHITE_LEVEL: u16 = 50717;
    pub(super) const DEFAULT_CROP_ORIGIN: u16 = 50719;
    pub(super) const DEFAULT_CROP_SIZE: u16 = 50720;
    pub(super) const COLOR_MATRIX: [u16; 2] = [50721, 50722];
    pub(super) const CAMERA_CALIBRATION: [u16; 2] = [50723, 50724];
    pub(super) const ANALOG_BALANCE: u16 = 50727;
    pub(super) const AS_SHOT_NEUTRAL: u16 = 50728;
    pub(super) const BASELINE_EXPOSURE: u16 = 50730;
    pub(super) const DNG_PRIVATE_DATA: u16 = 50740;
    pub(super) const CAMERA_CALIBRATION_SIGNATURE: u16 = 50931;
    pub(super) const CALIBRATION_ILLUMINANT: [u16; 2] = [50778, 50779];
    pub(super) const FORWARD_MATRIX: [u16; 2] = [50964, 50965];
    pub(super) const PREVIEW_APPLICATION_NAME: u16 = 50966;
    pub(super) const PREVIEW_APPLICATION_VERSION: u16 = 50967;
    pub(super) const PREVIEW_COLOR_SPACE: u16 = 50970;
    pub(super) const PREVIEW_DATE_TIME: u16 = 50971;
}

/// PhotometricInterpretation LinearRaw.
const LINEAR_RAW: u16 = 34892;
/// PhotometricInterpretation TransparencyMask (DNG 1.4).
const TRANSPARENCY_MASK: u16 = 4;
/// Compression Deflate (Adobe's code).
const DEFLATE: u16 = 8;
/// Predictor FloatingPointX2 (DNG 1.4).
const FLOATING_POINT_X2: u16 = 34894;
/// Compression JPEG (TIFF Technical Note 2) and PhotometricInterpretation
/// YCbCr, as previews are written.
const JPEG: u16 = 7;
const YCBCR: u16 = 6;
/// PreviewColorSpace sRGB.
const SRGB: u32 = 2;

/// A merged DNG, encoded but for IFD0, which [`Self::with`] adds.
#[derive(Debug)]
pub struct Encoded {
    /// The header, the image's and the mask's tiles, the mask's directory and
    /// the EXIF and GPS directories.
    body: Vec<u8>,
    /// IFD0's entries but the XMP and the SubIFDs.
    raw: Ifd,
    /// The mask's directory, when there is one.
    mask: Option<u32>,
}

/// What a merged DNG may hold besides the merge: what Lightroom's merges hold.
#[derive(Clone, Copy, Debug, Default)]
pub struct Extras<'a> {
    /// The XMP packet: the photo's starting settings and Lightroom's
    /// `aux:IsMergedHDR`.
    pub xmp: Option<&'a str>,
    pub preview: Option<Preview<'a>>,
}

/// A preview of the merge as it renders: a baseline sRGB JPEG.
#[derive(Clone, Copy, Debug)]
pub struct Preview<'a> {
    pub jpeg: &'a [u8],
    pub width: u32,
    pub height: u32,
    /// The application that rendered it and its version.
    pub application: (&'a str, &'a str),
    /// When it was rendered, as XMP writes dates ("2026-11-19T10:15:00Z").
    pub rendered: &'a str,
}

impl Encoded {
    /// The DNG with `extras`, ready to write.
    pub fn with(&self, extras: &Extras<'_>) -> Result<Dng<'_>, Error> {
        let mut tail = Tiff::after(self.body.len());
        let mut raw = self.raw.clone();
        let mut subs: Vec<u32> = self.mask.into_iter().collect();
        if let Some(preview) = &extras.preview {
            subs.push(preview_directory(&mut tail, preview)?);
        }
        if !subs.is_empty() {
            raw.set(tag::SUB_IFDS, Value::longs(&subs));
        }
        if let Some(xmp) = extras.xmp {
            raw.set(tag::XMP, Value::bytes(kind::BYTE, xmp.as_bytes().to_vec()));
        }
        let first = tail.directory(&raw)?;
        Ok(Dng {
            body: &self.body,
            first,
            tail: tail.into_bytes(),
        })
    }
}

/// A merged DNG as [`Encoded::with`] makes it: written without copying the
/// encoded image.
pub struct Dng<'a> {
    body: &'a [u8],
    /// IFD0's offset.
    first: u32,
    /// The preview, the XMP and IFD0, which follow the body.
    tail: Vec<u8>,
}

impl Dng<'_> {
    pub fn write_to(&self, out: &mut impl Write) -> std::io::Result<()> {
        out.write_all(&self.body[..4])?;
        out.write_all(&self.first.to_le_bytes())?;
        out.write_all(&self.body[8..])?;
        out.write_all(&self.tail)
    }
    /// The file's bytes.
    pub fn to_vec(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + self.tail.len());
        // Writing to memory doesn't fail.
        let _ = self.write_to(&mut out);
        out
    }
}

/// The preview's directory (one strip of JPEG), added to `tiff`; its offset.
fn preview_directory(tiff: &mut Tiff, preview: &Preview<'_>) -> Result<u32, Error> {
    if preview.width == 0 || preview.height == 0 || preview.jpeg.is_empty() {
        return Err(Error::Invalid("an empty preview"));
    }
    let at = tiff.block(preview.jpeg)?;
    let count = u32::try_from(preview.jpeg.len()).map_err(|_| Error::TooLarge)?;
    let mut ifd = Ifd::default();
    ifd.set(tag::NEW_SUBFILE_TYPE, Value::longs(&[1]));
    ifd.set(tag::IMAGE_WIDTH, Value::longs(&[preview.width]));
    ifd.set(tag::IMAGE_LENGTH, Value::longs(&[preview.height]));
    ifd.set(tag::BITS_PER_SAMPLE, Value::shorts(&[8; 3]));
    ifd.set(tag::COMPRESSION, Value::shorts(&[JPEG]));
    ifd.set(tag::PHOTOMETRIC, Value::shorts(&[YCBCR]));
    // The `image` crate's baseline JPEGs keep every chroma sample (4:4:4);
    // TIFF takes a missing tag for 2 × 2.
    ifd.set(tag::YCBCR_SUB_SAMPLING, Value::shorts(&[1, 1]));
    ifd.set(tag::SAMPLES_PER_PIXEL, Value::shorts(&[3]));
    ifd.set(tag::PLANAR_CONFIGURATION, Value::shorts(&[1]));
    ifd.set(tag::STRIP_OFFSETS, Value::longs(&[at]));
    ifd.set(tag::ROWS_PER_STRIP, Value::longs(&[preview.height]));
    ifd.set(tag::STRIP_BYTE_COUNTS, Value::longs(&[count]));
    ifd.set(
        tag::PREVIEW_APPLICATION_NAME,
        Value::ascii(preview.application.0),
    );
    ifd.set(
        tag::PREVIEW_APPLICATION_VERSION,
        Value::ascii(preview.application.1),
    );
    ifd.set(tag::PREVIEW_COLOR_SPACE, Value::longs(&[SRGB]));
    ifd.set(tag::PREVIEW_DATE_TIME, Value::ascii(preview.rendered));
    tiff.directory(&ifd)
}

/// The merged DNG of `image` and `facts`, as file bytes, without
/// [`Extras`].
pub fn encode(image: &LinearImage, facts: &DngFacts, cancel: Cancel<'_>) -> Result<Vec<u8>, Error> {
    Ok(encoded(image, facts, cancel)?
        .with(&Extras::default())?
        .to_vec())
}

/// The merged DNG of `image` and `facts`, all but IFD0 (see [`Encoded`]).
/// Stops with [`Error::Cancelled`] soon after `cancel` is set.
pub fn encoded(
    image: &LinearImage,
    facts: &DngFacts,
    cancel: Cancel<'_>,
) -> Result<Encoded, Error> {
    let (w, h) = (image.width as usize, image.height as usize);
    if w == 0 || h == 0 || w.checked_mul(h) != Some(image.pixels.len()) {
        return Err(Error::Invalid("the image's size does not match its pixels"));
    }
    if image
        .coverage
        .is_some_and(|c| c.len() != image.pixels.len())
    {
        return Err(Error::Invalid(
            "the coverage's size does not match the image's",
        ));
    }
    if !(1..=MAX_WHITE).contains(&facts.white_level) {
        return Err(Error::Invalid("WhiteLevel out of half-precision range"));
    }
    if !(1..=2).contains(&facts.calibrations.len()) {
        return Err(Error::Invalid("a DNG takes one or two calibrations"));
    }
    let finite = |v: &[f64]| v.iter().all(|v| v.is_finite());
    let positive = |v: &[f64]| v.iter().all(|&v| v.is_finite() && v > 0.);
    if !positive(&facts.as_shot_neutral) || facts.analog_balance.is_some_and(|a| !positive(&a)) {
        return Err(Error::Invalid(
            "AsShotNeutral and AnalogBalance must be positive",
        ));
    }
    if !facts.baseline_exposure.is_finite()
        || !facts.calibrations.iter().all(|c| {
            finite(&c.color_matrix)
                && c.forward_matrix.is_none_or(|m| finite(&m))
                && c.camera_calibration.is_none_or(|m| finite(&m))
        })
    {
        return Err(Error::Invalid(
            "a matrix or BaselineExposure that is not a number",
        ));
    }
    if !(1..=8).contains(&facts.orientation) {
        return Err(Error::Invalid("Orientation out of range"));
    }
    if let Some([x, y, cw, ch]) = facts.default_crop
        && (cw == 0
            || ch == 0
            || u64::from(x) + u64::from(cw) > image.width.into()
            || u64::from(y) + u64::from(ch) > image.height.into())
    {
        return Err(Error::Invalid("the default crop leaves the image"));
    }
    let mut tiff = Tiff::new();
    let tiles = predicted(tiles(image, facts.white_level as f32, cancel)?, cancel)?;
    let (offsets, counts) = blocks(&mut tiff, &tiles)?;
    let mut raw = Ifd::default();
    raw.set(tag::NEW_SUBFILE_TYPE, Value::longs(&[0]));
    raw.set(tag::IMAGE_WIDTH, Value::longs(&[image.width]));
    raw.set(tag::IMAGE_LENGTH, Value::longs(&[image.height]));
    raw.set(tag::BITS_PER_SAMPLE, Value::shorts(&[16; 3]));
    raw.set(tag::COMPRESSION, Value::shorts(&[DEFLATE]));
    raw.set(tag::PHOTOMETRIC, Value::shorts(&[LINEAR_RAW]));
    raw.set(tag::SAMPLES_PER_PIXEL, Value::shorts(&[3]));
    raw.set(tag::PLANAR_CONFIGURATION, Value::shorts(&[1]));
    raw.set(tag::PREDICTOR, Value::shorts(&[FLOATING_POINT_X2]));
    raw.set(tag::SAMPLE_FORMAT, Value::shorts(&[3; 3]));
    set_tiles(&mut raw, offsets, counts);
    raw.set(tag::BLACK_LEVEL, Value::longs(&[0; 3]));
    raw.set(tag::WHITE_LEVEL, Value::longs(&[facts.white_level; 3]));
    if let Some([x, y, cw, ch]) = facts.default_crop {
        raw.set(tag::DEFAULT_CROP_ORIGIN, Value::longs(&[x, y]));
        raw.set(tag::DEFAULT_CROP_SIZE, Value::longs(&[cw, ch]));
    }
    let mask = match image.coverage {
        Some(coverage) => Some(mask(&mut tiff, image, coverage, cancel)?),
        None => None,
    };
    describe(&mut tiff, &mut raw, facts)?;
    Ok(Encoded {
        body: tiff.into_bytes(),
        raw,
        mask,
    })
}

/// IFD0's camera, colour and EXIF tags, its EXIF and GPS directories added
/// to `tiff`.
fn describe(tiff: &mut Tiff, raw: &mut Ifd, facts: &DngFacts) -> Result<(), Error> {
    raw.set(tag::DNG_VERSION, Value::bytes(kind::BYTE, vec![1, 4, 0, 0]));
    raw.set(
        tag::DNG_BACKWARD_VERSION,
        Value::bytes(kind::BYTE, vec![1, 4, 0, 0]),
    );
    // The camera's directories first; the DNG's own tags replace any they hold.
    for field in facts.exif.main.iter().filter(|f| copied(f.tag)) {
        raw.set(field.tag, field_value(field));
    }
    raw.set(tag::MAKE, Value::ascii(&facts.make));
    raw.set(tag::MODEL, Value::ascii(&facts.model));
    raw.set(
        tag::UNIQUE_CAMERA_MODEL,
        Value::ascii(&facts.unique_camera_model),
    );
    raw.set(tag::SOFTWARE, Value::ascii(&facts.software));
    raw.set(tag::ORIENTATION, Value::shorts(&[facts.orientation]));
    for (i, c) in facts.calibrations.iter().enumerate() {
        raw.set(
            tag::CALIBRATION_ILLUMINANT[i],
            Value::shorts(&[c.illuminant]),
        );
        raw.set(tag::COLOR_MATRIX[i], Value::srationals(&c.color_matrix));
        if let Some(m) = &c.forward_matrix {
            raw.set(tag::FORWARD_MATRIX[i], Value::srationals(m));
        }
        if let Some(m) = &c.camera_calibration {
            raw.set(tag::CAMERA_CALIBRATION[i], Value::srationals(m));
        }
    }
    if let Some(signature) = &facts.calibration_signature {
        raw.set(tag::CAMERA_CALIBRATION_SIGNATURE, Value::ascii(signature));
    }
    if let Some(ab) = facts.analog_balance {
        raw.set(tag::ANALOG_BALANCE, Value::rationals(&ab));
    }
    raw.set(
        tag::AS_SHOT_NEUTRAL,
        Value::rationals(&facts.as_shot_neutral),
    );
    raw.set(
        tag::BASELINE_EXPOSURE,
        Value::srationals(&[facts.baseline_exposure]),
    );
    if let Some(note) = &facts.maker_note {
        raw.set(
            tag::DNG_PRIVATE_DATA,
            Value::bytes(kind::BYTE, private_data(note)?),
        );
    }
    let exif: Vec<&Field> = facts
        .exif
        .exif
        .iter()
        .filter(|f| f.tag != exif_tag::MAKER_NOTE)
        .collect();
    if !exif.is_empty() {
        let at = sub_directory(tiff, &exif)?;
        raw.set(tag::EXIF_IFD, Value::longs(&[at]));
    }
    if !facts.exif.gps.is_empty() {
        let gps: Vec<&Field> = facts.exif.gps.iter().collect();
        let at = sub_directory(tiff, &gps)?;
        raw.set(tag::GPS_IFD, Value::longs(&[at]));
    }
    Ok(())
}

/// Whether a field of the camera's main directory is copied into IFD0: what
/// describes the photo (ImageDescription, Artist, Copyright), not the camera
/// or the image, which the DNG's own tags say.
fn copied(tag: u16) -> bool {
    matches!(
        tag,
        exif_tag::IMAGE_DESCRIPTION | exif_tag::ARTIST | exif_tag::COPYRIGHT
    )
}

fn field_value(field: &Field) -> Value {
    Value {
        kind: field.kind,
        count: field.count,
        bytes: field.bytes.clone(),
    }
}

/// A directory of `fields`, added to `tiff`; its offset.
fn sub_directory(tiff: &mut Tiff, fields: &[&Field]) -> Result<u32, Error> {
    let mut ifd = Ifd::default();
    for field in fields {
        ifd.set(field.tag, field_value(field));
    }
    tiff.directory(&ifd)
}

/// Adobe's DNGPrivateData with the maker note: "Adobe\0", then the record
/// "MakN" of a big-endian length and its data: the note's byte order ("II"
/// or "MM"), its big-endian offset in its original file, then the note.
fn private_data(note: &MakerNote) -> Result<Vec<u8>, Error> {
    let len = u32::try_from(note.bytes.len() + 6).map_err(|_| Error::TooLarge)?;
    let mut out = b"Adobe\0MakN".to_vec();
    out.extend(len.to_be_bytes());
    out.extend(if note.little_endian { b"II" } else { b"MM" });
    out.extend(note.offset.to_be_bytes());
    out.extend(&note.bytes);
    Ok(out)
}

/// A sample as IEEE half precision, rounded to nearest: 0 below 0 and for
/// NaN, `white` above it.
fn half_bits(v: f32, white: f32) -> u16 {
    let v = if v.is_nan() { 0. } else { v.clamp(0., white) };
    half::f16::from_f32(v).to_bits()
}

/// The image's tiles of half-precision samples under `white`, row by row
/// within each tile, the tiles in raster order; the parts past the image's
/// right and bottom edges are zeros.
fn tiles(image: &LinearImage, white: f32, cancel: Cancel<'_>) -> Result<Vec<Vec<u16>>, Error> {
    let (w, h) = (image.width as usize, image.height as usize);
    let tile = TILE as usize;
    let (across, down) = (w.div_ceil(tile), h.div_ceil(tile));
    (0..across * down)
        .into_par_iter()
        .map(|i| {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let (x0, y0) = ((i % across) * tile, (i / across) * tile);
            let mut out = vec![0u16; tile * tile * 3];
            for (dy, dst) in out.chunks_exact_mut(tile * 3).enumerate() {
                let y = y0 + dy;
                if y >= h {
                    break;
                }
                let src = &image.pixels[y * w + x0..y * w + (x0 + tile).min(w)];
                for (d, pixel) in dst.as_chunks_mut::<3>().0.iter_mut().zip(src) {
                    *d = pixel.map(|v| half_bits(v, white));
                }
            }
            Ok(out)
        })
        .collect()
}

/// Half-precision tiles under the floating-point predictor X2, each Deflated.
fn predicted(tiles: Vec<Vec<u16>>, cancel: Cancel<'_>) -> Result<Vec<Vec<u8>>, Error> {
    let row = TILE as usize * 3;
    tiles
        .into_par_iter()
        .map(|samples| {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let mut bytes = Vec::with_capacity(samples.len() * 2);
            for r in samples.chunks_exact(row) {
                let start = bytes.len();
                // Byte planes: every sample's high byte, then every low byte.
                bytes.extend(r.iter().map(|v| (v >> 8) as u8));
                bytes.extend(r.iter().map(|v| *v as u8));
                predict(&mut bytes[start..], 2 * 3);
            }
            deflate(&bytes)
        })
        .collect()
}

/// Each byte of `row` less the byte `stride` before it (the predictor's
/// differences), from the end so each takes the original before it.
fn predict(row: &mut [u8], stride: usize) {
    for i in (stride..row.len()).rev() {
        row[i] = row[i].wrapping_sub(row[i - stride]);
    }
}

fn deflate(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(bytes)
        .map_err(|_| Error::Invalid("Deflate failed"))?;
    z.finish().map_err(|_| Error::Invalid("Deflate failed"))
}

/// Adds the tiles to `tiff`: their offsets and byte counts.
fn blocks(tiff: &mut Tiff, tiles: &[Vec<u8>]) -> Result<(Vec<u32>, Vec<u32>), Error> {
    let mut offsets = Vec::with_capacity(tiles.len());
    let mut counts = Vec::with_capacity(tiles.len());
    for tile in tiles {
        offsets.push(tiff.block(tile)?);
        counts.push(u32::try_from(tile.len()).map_err(|_| Error::TooLarge)?);
    }
    Ok((offsets, counts))
}

fn set_tiles(ifd: &mut Ifd, offsets: Vec<u32>, counts: Vec<u32>) {
    ifd.set(tag::TILE_WIDTH, Value::longs(&[TILE]));
    ifd.set(tag::TILE_LENGTH, Value::longs(&[TILE]));
    ifd.set(tag::TILE_OFFSETS, Value::longs(&offsets));
    ifd.set(tag::TILE_BYTE_COUNTS, Value::longs(&counts));
}

/// The transparency mask's directory (8-bit, Deflate tiles), added to
/// `tiff`; its offset.
fn mask(
    tiff: &mut Tiff,
    image: &LinearImage,
    coverage: &[u8],
    cancel: Cancel<'_>,
) -> Result<u32, Error> {
    let w = image.width as usize;
    let tile = TILE as usize;
    let (across, down) = (w.div_ceil(tile), (image.height as usize).div_ceil(tile));
    let tiles: Vec<Vec<u8>> = (0..across * down)
        .into_par_iter()
        .map(|i| {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let (x0, y0) = ((i % across) * tile, (i / across) * tile);
            let mut out = vec![0u8; tile * tile];
            for (dy, dst) in out.chunks_exact_mut(tile).enumerate() {
                let y = y0 + dy;
                if y >= image.height as usize {
                    break;
                }
                let src = &coverage[y * w + x0..y * w + (x0 + tile).min(w)];
                dst[..src.len()].copy_from_slice(src);
            }
            deflate(&out)
        })
        .collect::<Result<_, _>>()?;
    let (offsets, counts) = blocks(tiff, &tiles)?;
    let mut ifd = Ifd::default();
    ifd.set(tag::NEW_SUBFILE_TYPE, Value::longs(&[4]));
    ifd.set(tag::IMAGE_WIDTH, Value::longs(&[image.width]));
    ifd.set(tag::IMAGE_LENGTH, Value::longs(&[image.height]));
    ifd.set(tag::BITS_PER_SAMPLE, Value::shorts(&[8]));
    ifd.set(tag::COMPRESSION, Value::shorts(&[DEFLATE]));
    ifd.set(tag::PHOTOMETRIC, Value::shorts(&[TRANSPARENCY_MASK]));
    ifd.set(tag::SAMPLES_PER_PIXEL, Value::shorts(&[1]));
    ifd.set(tag::PLANAR_CONFIGURATION, Value::shorts(&[1]));
    set_tiles(&mut ifd, offsets, counts);
    tiff.directory(&ifd)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The predictor's differences undo to the row they were made of, with
    /// the stride the DNG SDK undoes them with for three samples (2 pixels
    /// × 3 samples).
    #[test]
    fn prediction_differences_bytes_two_pixels_apart() {
        let row: Vec<u8> = (0..48u8).map(|v| v.wrapping_mul(37)).collect();
        let mut coded = row.clone();
        predict(&mut coded, 6);
        assert_eq!(coded[..6], row[..6]);
        assert_eq!(coded[10], row[10].wrapping_sub(row[4]));
        for i in 6..coded.len() {
            coded[i] = coded[i].wrapping_add(coded[i - 6]);
        }
        assert_eq!(coded, row);
    }

    /// Half precision rounds to nearest; out-of-range values are clamped.
    #[test]
    fn samples_round_to_half_precision() {
        assert_eq!(half_bits(1., 32768.), 0x3c00);
        assert_eq!(half_bits(-3., 32768.), 0);
        assert_eq!(half_bits(f32::NAN, 32768.), 0);
        assert_eq!(half_bits(1e9, 32768.), 0x7800);
        // 1 + 2^-11 is halfway between 1 and the next half: even, so 1.
        assert_eq!(half_bits(1. + 2f32.powi(-11), 32768.), 0x3c00);
        assert_eq!(half_bits(2f32.powi(-24), 32768.), 0x0001);
    }
}
