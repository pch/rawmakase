//! Merge to HDR ([docs/hdr-merge.md](../../../docs/hdr-merge.md)): a bracket
//! of RAWs merged into one scene-referred linear floating-point DNG that opens
//! as an ordinary raw, here and in Lightroom and Camera Raw.
//!
//! [`merge()`] checks the bracket ([`bracket`]), decodes its frames from the
//! darkest to the brightest, measures each one's exposure against the one
//! before (`ratio`) and its noise (`noise`), and adds it to the noise-weighted
//! merge (`merge`); then it encodes the DNG ([`encode`]). [`write_dng`]
//! publishes it beside the sources, once they are checked unchanged.
//!
//! It never depends on the catalog, export, the app, the GUI or inference
//! (`scripts/deps-crates.txt`).
#![forbid(unsafe_code)]

mod align;
pub mod bracket;
mod dng;
mod merge;
mod noise;
mod ratio;
mod write;

pub use align::{Alignment, MAX_ROTATION, MAX_SHIFT, Matrix};
pub use dng::{Calibration, DngFacts, LinearImage, MAX_WHITE, MakerNote, TILE, encode};
pub use write::{Destination, write_dng};

use bracket::{Exposure, Refusal, Shot};
use rawmakase_cancel::Cancel;
use rawmakase_decode::photo;
use rawmakase_decode::raw::Frame;
use rawmakase_interop::exif::{CameraExif, Field, tag};
use rawmakase_model::camera_data::{Demosaic, Metadata};
use rawmakase_model::storage::Identity;
use std::path::{Path, PathBuf};

/// The merged DNG's WhiteLevel, which the darkest frame's saturation maps to,
/// as Lightroom's merges have it: a power of two, so the reference frame's
/// levels stay exact in half precision.
pub const WHITE: u32 = 32768;

/// The most BaselineExposure the merged DNG may have, in stops, either way:
/// what DNG readers take (`rawmakase_model::dng`).
pub const MAX_BASELINE: f64 = 5.;

/// A source of a merge, captured once: its path and its file's identity, which
/// [`write_dng`] checks again before publishing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameInput {
    pub path: PathBuf,
    pub identity: Identity,
}

impl FrameInput {
    /// The file at `path` as it is now.
    pub fn read(path: &Path) -> Result<Self, Error> {
        let identity = Identity::read(path).map_err(|e| Error::Unreadable {
            photo: name(path),
            why: e.to_string(),
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            identity,
        })
    }
}

/// How to merge, captured with the inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeOptions {
    pub demosaic: Demosaic,
    /// Auto Align: each frame aligned to the reference and warped into its
    /// geometry. Off, the frames are merged as they were taken.
    pub align: bool,
    /// The DNG's Software tag: the application and its version.
    pub software: String,
}

/// Where a merge is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Reading the photos' metadata and checking they are a bracket.
    Check,
    /// Decoding the `n`th frame (from 0) in merge order. With Auto Align
    /// the reference frame is decoded first.
    Decode(usize),
    /// Aligning the `n`th frame to the reference (Auto Align), the
    /// reference's own `Align` preparing it for the others.
    Align(usize),
    /// Measuring the `n`th frame's exposure against the one before.
    Measure(usize),
    /// Adding the `n`th frame to the merge.
    Merge(usize),
    Encode,
}

/// A merge, ready for [`write_dng`].
#[derive(Debug)]
pub struct Merged {
    /// The DNG file's bytes.
    pub dng: Vec<u8>,
    /// The sources, as captured.
    pub inputs: Vec<FrameInput>,
    /// The frames in merge order, from the darkest.
    pub frames: Vec<MergedFrame>,
    /// The reference frame's place in [`Self::frames`]: the DNG renders at its
    /// brightness and takes its white balance and crop.
    pub reference: usize,
    /// The reference had to move from the frame nearest the median
    /// exposure for the BaselineExposure to stay within ±[`MAX_BASELINE`].
    pub reference_moved: bool,
    pub baseline_exposure: f64,
}

/// What the merge found of one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct MergedFrame {
    /// Its place in [`Merged::inputs`].
    pub input: usize,
    /// Its exposure over the darkest frame's, in stops: from the EXIF, and as
    /// measured (the EXIF's when it could not be measured).
    pub nominal_ev: f64,
    pub ev: f64,
    /// Whether [`Self::ev`] was measured.
    pub measured: bool,
    /// How Auto Align moved it onto the reference; `None` for the
    /// reference and without Auto Align.
    pub alignment: Option<Alignment>,
}

/// Why a merge or its DNG failed.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The work was cancelled.
    Cancelled,
    /// The DNG would pass the 4 GiB that TIFF's 32-bit offsets reach.
    TooLarge,
    /// The input breaks the writer's contract.
    Invalid(&'static str),
    /// The photos are not a bracket.
    Refused(Refusal),
    /// A photo could not be read or decoded.
    Unreadable { photo: String, why: String },
    /// A source changed after the merge read it: nothing is published.
    SourceChanged(String),
    /// The DNG could not be written in its folder.
    Write { folder: PathBuf, why: String },
    /// The file to write already exists.
    Exists(PathBuf),
    /// Auto Align couldn't align a frame to the reference: nothing is
    /// merged.
    Align { photo: String, why: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Merge to HDR was cancelled"),
            Self::TooLarge => f.write_str("The merged DNG would be larger than 4 GB"),
            Self::Invalid(why) => write!(f, "Invalid merged DNG: {why}"),
            Self::Refused(refusal) => refusal.fmt(f),
            Self::Unreadable { photo, why } => write!(f, "{photo} can't be read: {why}"),
            Self::SourceChanged(photo) => write!(
                f,
                "{photo} changed while it was being merged; nothing was saved"
            ),
            Self::Write { folder, why } => write!(
                f,
                "The merged DNG couldn't be saved in {}: {why}",
                folder.display()
            ),
            Self::Exists(path) => write!(f, "{} already exists", path.display()),
            Self::Align { photo, why } => write!(
                f,
                "Couldn't align {photo}: {why}. Try with Auto Align off, or a bracket taken on a tripod"
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<Refusal> for Error {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

/// A frame's measured exposure may differ from its EXIF's by this many stops;
/// more, and the frames don't show one scene.
const MAX_MEASURED_OFF: f64 = 0.5;

/// Merges the bracket `inputs` into a DNG, reporting each [`Stage`] with
/// its progress (0 to 1) to `progress`. Stops with [`Error::Cancelled`] soon
/// after `cancel` is set: between frames and within each pass over the pixels.
pub fn merge(
    inputs: &[FrameInput],
    options: &MergeOptions,
    progress: &(dyn Fn(Stage, f32) + Sync),
    cancel: Cancel<'_>,
) -> Result<Merged, Error> {
    let cancelled = || {
        if cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    };
    progress(Stage::Check, 0.);
    let metadata: Vec<Metadata> = inputs
        .iter()
        .map(|input| {
            cancelled()?;
            photo::header_metadata(&input.path)
                .or_else(|_| photo::open(&input.path).map(|raw| raw.metadata))
                .map_err(|e| unreadable(&input.path, &e))
        })
        .collect::<Result<_, _>>()?;
    let exif: Vec<Option<CameraExif>> = inputs
        .iter()
        .map(|input| photo::camera_exif(&input.path))
        .collect();
    let shots: Vec<Shot> = inputs
        .iter()
        .zip(&metadata)
        .zip(&exif)
        .map(|((input, m), exif)| shot(&input.path, m, exif.as_ref()))
        .collect();
    let bracket = bracket::check(&shots)?;
    progress(Stage::Check, 1.);
    // The `k`th frame in merge order, decoded, and its noise.
    let decoded = |k: usize| -> Result<(Frame, noise::Noise), Error> {
        cancelled()?;
        progress(Stage::Decode(k), 0.);
        let path = &inputs[bracket.order[k]].path;
        let frame = decode(path, options.demosaic, cancel)?;
        if frame.metadata.cam_xyz.iter().flatten().all(|v| *v == 0.) {
            return Err(Refusal::Unreadable(
                name(path),
                "RAWmakase has no colour matrix for its camera".into(),
            )
            .into());
        }
        // Measured before any warp: resampling smooths the noise.
        let noise = noise::measure(&frame);
        progress(Stage::Decode(k), 1.);
        Ok((frame, noise))
    };

    let n = bracket.order.len();
    let mut frames: Vec<MergedFrame> = Vec::with_capacity(n);
    let mut kept: Vec<Kept> = Vec::with_capacity(n);
    let mut accumulator: Option<merge::Accumulator> = None;
    let mut previous: Option<(Frame, noise::Noise)> = None;
    // With Auto Align the reference comes first: every other frame is
    // aligned to it. It is kept until its turn to be merged.
    let mut reference_frame: Option<(Frame, noise::Noise)> = None;
    let mut template: Option<align::Template> = None;
    if options.align {
        let k = bracket.reference;
        let (frame, noise) = decoded(k)?;
        progress(Stage::Align(k), 0.);
        template = Some(align::Template::new(align::Pyramid::of_frame(
            &frame, noise,
        )));
        progress(Stage::Align(k), 1.);
        reference_frame = Some((frame, noise));
    }
    // Which pixels every frame covers so far, once a frame was warped.
    let mut coverage: Option<Vec<bool>> = None;
    for (k, &input) in bracket.order.iter().enumerate() {
        let path = &inputs[input].path;
        let (frame, noise) = match reference_frame.take_if(|_| k == bracket.reference) {
            Some(kept) => kept,
            None => decoded(k)?,
        };
        let (frame, covered, alignment) = match &template {
            Some(template) if k != bracket.reference => {
                cancelled()?;
                progress(Stage::Align(k), 0.);
                let pyramid = align::Pyramid::of_frame(&frame, noise);
                let alignment =
                    align::align(template, &pyramid, cancel).map_err(|why| match why {
                        align::Failure::Cancelled => Error::Cancelled,
                        why => Error::Align {
                            photo: name(path),
                            why: why.to_string(),
                        },
                    })?;
                drop(pyramid);
                let (frame, covered) = if alignment.warped {
                    let (warped, covered) = align::warp(&frame, &alignment.matrix, cancel)?;
                    (warped, Some(covered))
                } else {
                    (frame, None)
                };
                progress(Stage::Align(k), 1.);
                (frame, covered, Some(alignment))
            }
            _ => (frame, None, None),
        };
        let nominal_ev = bracket.ev[k];
        let (ev, measured) = match &previous {
            None => (0., true),
            Some((dark, dark_noise)) => {
                cancelled()?;
                progress(Stage::Measure(k), 0.);
                let last = frames.last().map_or(0., |f: &MergedFrame| f.ev);
                let step = nominal_ev - bracket.ev[k - 1];
                let measured = ratio::measure(dark, &frame, *dark_noise);
                progress(Stage::Measure(k), 1.);
                match measured {
                    Some(r) => {
                        let off = r.log2() - step;
                        if off.abs() > MAX_MEASURED_OFF {
                            return Err(Refusal::AnotherScene(
                                name(&inputs[bracket.order[k - 1]].path),
                                name(path),
                                off.abs(),
                            )
                            .into());
                        }
                        (last + r.log2(), true)
                    }
                    None => (last + step, false),
                }
            }
        };
        progress(Stage::Merge(k), 0.);
        let acc =
            accumulator.get_or_insert_with(|| merge::Accumulator::new(frame.width, frame.height));
        acc.add(&frame, ev.exp2(), noise, k == 0, covered.as_deref(), cancel)?;
        progress(Stage::Merge(k), 1.);
        if let Some(covered) = covered {
            match &mut coverage {
                None => coverage = Some(covered),
                Some(all) => all.iter_mut().zip(covered).for_each(|(a, c)| *a &= c),
            }
        }
        frames.push(MergedFrame {
            input,
            nominal_ev,
            ev,
            measured,
            alignment,
        });
        kept.push(Kept {
            saturation: frame.saturation,
            width: frame.width,
            height: frame.height,
            metadata: frame.metadata.clone(),
        });
        previous = Some((frame, noise));
    }
    drop(previous);
    cancelled()?;
    let light = accumulator.ok_or(Error::Invalid("no frames"))?.finish();

    // The darkest frame's saturation, in its units, is the DNG's white.
    let white = f64::from(kept[0].saturation.iter().fold(1f32, |a, b| a.min(*b)));
    let baseline = |k: usize| {
        f64::from(rawmakase_model::camera_profiles::reference::baseline_exposure(&kept[k].metadata))
            + frames[k].ev
            + white.log2()
    };
    let mut reference = bracket.reference;
    while baseline(reference) > MAX_BASELINE && reference > 0 {
        reference -= 1;
    }
    while baseline(reference) < -MAX_BASELINE && reference + 1 < n {
        reference += 1;
    }
    let baseline_exposure = baseline(reference);
    if baseline_exposure.abs() > MAX_BASELINE {
        return Err(Error::Invalid("BaselineExposure out of range"));
    }
    let reference_moved = reference != bracket.reference;

    progress(Stage::Encode, 0.);
    let scale = (f64::from(WHITE) / white) as f32;
    let mut pixels = light;
    {
        use rayon::prelude::*;
        pixels
            .par_iter_mut()
            .for_each(|p| *p = p.map(|v| v * scale));
    }
    let r = &kept[reference];
    let facts = facts(
        &r.metadata,
        &exif[frames[0].input],
        &kept[0].metadata,
        baseline_exposure,
        options,
    )?;
    let coverage: Option<Vec<u8>> = coverage
        .filter(|c| c.contains(&false))
        .map(|c| c.into_iter().map(|c| if c { 255 } else { 0 }).collect());
    let image = LinearImage {
        width: r.width,
        height: r.height,
        pixels: &pixels,
        coverage: coverage.as_deref(),
    };
    let dng = encode(&image, &facts, cancel)?;
    progress(Stage::Encode, 1.);
    Ok(Merged {
        dng,
        inputs: inputs.to_vec(),
        frames,
        reference,
        reference_moved,
        baseline_exposure,
    })
}

/// What the merge keeps of a frame after merging it.
struct Kept {
    saturation: [f32; 3],
    width: u32,
    height: u32,
    metadata: Metadata,
}

fn decode(path: &Path, demosaic: Demosaic, cancel: Cancel<'_>) -> Result<Frame, Error> {
    let frame = photo::open(path).and_then(|raw| raw.develop_frame(demosaic, cancel));
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    frame.map_err(|e| unreadable(path, &e))
}

fn unreadable(path: &Path, e: &anyhow::Error) -> Error {
    Error::Unreadable {
        photo: name(path),
        why: e.to_string(),
    }
}

/// The file name of `path`, as messages name a photo.
fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// What the bracket check reads of the photo at `path`.
fn shot(path: &Path, m: &Metadata, exif: Option<&CameraExif>) -> Shot {
    let text = |t| exif.and_then(|e| e.get(t)).and_then(Field::text);
    let captured = text(tag::DATE_TIME_ORIGINAL)
        .and_then(|date| bracket::seconds(&date, text(tag::SUBSEC_TIME_ORIGINAL).as_deref()));
    Shot {
        name: name(path),
        camera: [m.make.clone(), m.model.clone()],
        serial: text(tag::BODY_SERIAL_NUMBER).filter(|s| !s.is_empty()),
        size: [m.width, m.height],
        flip: m.flip,
        xtrans: m.xtrans,
        exposure: Exposure {
            time: f64::from(m.shutter),
            f_number: f64::from(m.aperture),
            iso: f64::from(m.iso),
        },
        captured,
    }
}

/// The merged DNG's facts: the camera, colour, white balance and frame of
/// the reference frame `r`, the EXIF of the darkest frame (`darkest_exif`,
/// or what its `darkest` metadata says without one), as Lightroom's merges
/// take them.
fn facts(
    r: &Metadata,
    darkest_exif: &Option<CameraExif>,
    darkest: &Metadata,
    baseline_exposure: f64,
    options: &MergeOptions,
) -> Result<DngFacts, Error> {
    let or = |file: &str, named: &str| {
        if file.trim().is_empty() {
            named.to_string()
        } else {
            file.to_string()
        }
    };
    let unique_camera_model = r
        .adobe_names
        .first()
        .cloned()
        .unwrap_or_else(|| format!("{} {}", r.make, r.model));
    // A DNG's own calibration and its signature; Adobe's calibration of the
    // camera unit for other RAWs, as Camera Raw applies it.
    let (gains, signature) = match (&r.baseline_exposure, &r.dng_neutral_calibration) {
        (Some(_), Some(c)) => (c.gains, Some(c.signature.clone())),
        (Some(_), None) => ([1.; 3], None),
        (None, _) => (
            rawmakase_model::camera_profiles::reference::neutral_calibration(r),
            Some("com.adobe".to_string()),
        ),
    };
    let calibrated = gains != [1.; 3];
    let diagonal = |g: [f32; 3]| {
        let mut m = [0.; 9];
        for c in 0..3 {
            m[c * 4] = f64::from(g[c]);
        }
        m
    };
    let color_matrix: [f64; 9] = std::array::from_fn(|i| f64::from(r.cam_xyz[i / 3][i % 3]));
    let wb = r.wb;
    if !wb.iter().all(|v| v.is_finite() && *v > 0.) {
        return Err(Error::Invalid("the reference frame has no white balance"));
    }
    let neutral = [1. / wb[0], 1. / wb[1], 1. / wb[2]].map(|v| f64::from(v * wb[1]));
    let full = [0, 0, r.width, r.height];
    let crop = [r.crop_left, r.crop_top, r.crop_width, r.crop_height];
    let default_crop = (crop != full
        && crop[2] > 0
        && crop[3] > 0
        && crop[0] + crop[2] <= r.width
        && crop[1] + crop[3] <= r.height)
        .then_some(crop);
    Ok(DngFacts {
        make: or(&r.exif_make, &r.make),
        model: or(&r.exif_model, &r.model),
        unique_camera_model,
        software: options.software.clone(),
        white_level: WHITE,
        baseline_exposure,
        calibrations: vec![Calibration {
            // D65: the decoder's camera matrices are Adobe's D65 ones.
            illuminant: 21,
            color_matrix,
            forward_matrix: None,
            camera_calibration: calibrated.then(|| diagonal(gains)),
        }],
        calibration_signature: signature.filter(|_| calibrated),
        analog_balance: None,
        as_shot_neutral: neutral,
        orientation: orientation(r.flip),
        default_crop,
        exif: darkest_exif
            .clone()
            .unwrap_or_else(|| CameraExif::from_libraw(darkest)),
        // Not yet carried over: each container keeps its note its own way.
        maker_note: None,
    })
}

/// The EXIF orientation of RAWmakase's flip code: bit 0 mirrors columns, bit 1
/// rows, bit 2 swaps them (applied last); the inverse of
/// the RAW decoders' (`geometry::flip` in `rawmakase-raw`).
fn orientation(flip: i32) -> u16 {
    match flip {
        1 => 2,
        2 => 4,
        3 => 3,
        4 => 5,
        5 => 8,
        6 => 6,
        7 => 7,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn orientation_inverts_the_flip_code() {
        // The RAW decoders' flip codes of EXIF orientations 1 to 8.
        for (exif, flip) in [
            (1, 0),
            (2, 1),
            (3, 3),
            (4, 2),
            (5, 4),
            (6, 6),
            (7, 7),
            (8, 5),
        ] {
            assert_eq!(super::orientation(flip), exif);
        }
    }
}
