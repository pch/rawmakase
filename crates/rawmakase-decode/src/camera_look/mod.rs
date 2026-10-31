//! Camera Look (fitted): a profile fitted to one photo's embedded JPEG, the maker's
//! in-camera rendering (docs/lightroom-profiles.md#camera-look-fitted).
//!
//! The photo is rendered small (192 pixels on the long edge, from stages four
//! times that reduced in boxes) at its default crop and orientation, with the
//! camera's built-in lens corrections, which its JPEG has, and no others. The
//! preview is reduced to the same pixels, aligned, and masked where it can't be
//! learned from ([`pairs`]). A third of the photo, in whole tiles, is held out.
//!
//! - With a usable colour matrix, a look over RAWmakase Standard is fitted from
//!   the scene tone stage's output ([`solve`]).
//! - Without one (or for the no-matrix trial, `FitInputs::drop_matrix`), a forward
//!   matrix is fitted first, from the camera input stage (before highlight
//!   reconstruction), and the look over it.
//!
//! A look is used only when its full render beats the baseline on the held-out
//! tiles ([`gate`]): a colour look, else one of the curve alone, else none. Fitting
//! is deterministic; nothing here reads or writes the catalog or a cache.
pub mod cache;
mod gate;
mod pairs;
mod solve;

pub use gate::Scores;
pub use pairs::Alignment;
use rawmakase_engine::develop;
use rawmakase_model::{
    camera_data::{Decode, Metadata},
    camera_profiles::{CameraProfile, PRO_TO_RGB, fitted},
    color::{mul, srgb_decode, srgb_encode},
    model::recipe::Recipe,
};
use serde::Serialize;
use std::{
    path::Path,
    sync::{Arc, Condvar, Mutex, atomic::AtomicBool},
};

type Rgb = [f32; 3];

/// The fit's version: part of a fit cache's key, raised whenever what a fit makes
/// of a photo changes.
pub const FIT_VERSION: u32 = 6;
/// The proxy's long edge, and how much larger its renders are.
const PROXY_EDGE: u32 = 192;
const RENDER_FACTOR: u32 = 4;
/// Linear ProPhoto RGB to XYZ D50.
const PRO_TO_XYZ: [[f32; 3]; 3] = [
    [0.797_674_9, 0.135_191_7, 0.031_353_4],
    [0.288_040_2, 0.711_874_1, 0.000_085_7],
    [0., 0., 0.825_210],
];

/// What a fit is asked to do.
#[derive(Clone, Copy, Debug, Default)]
pub struct FitInputs {
    /// Fit a matrix even for a camera that has one, as for a camera without (the
    /// no-matrix trial); its own then only serves for comparison.
    pub drop_matrix: bool,
}

/// Which look a fit settled on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Colour,
    ToneOnly,
    Monochrome,
}

/// A look a fit tried, how it scored on the held-out tiles, and why it was not
/// used, if it was not.
#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub kind: Kind,
    pub scores: Scores,
    pub refused: Option<String>,
}

/// What a fit measured, used or refused: for `rawmakase fit-look --report`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub camera: String,
    pub matrix_source: String,
    pub preview: [u32; 2],
    pub space: &'static str,
    /// The preview was cut to the photo's aspect ratio (an in-camera crop).
    pub preview_cut: bool,
    pub alignment: Alignment,
    pub fit_pairs: usize,
    pub holdout_pairs: usize,
    /// The share of held-out pairs in colour.
    pub coloured: f32,
    pub monochrome: bool,
    /// The render the look replaces: RAWmakase Standard, else no profile.
    pub baseline: Scores,
    pub candidates: Vec<Candidate>,
    /// The fitted colour look and an unconstrained reference fit (the look plus a
    /// free 3D correction), both applied to the scene tone stage's output directly:
    /// what the look's representation loses.
    pub direct: Option<Direct>,
    /// The no-matrix trial: the fitted matrix base alone, and how far the fitted
    /// matrix's colour is from the camera's own matrix's on the fitted pixels.
    pub trial: Option<Trial>,
    pub kind: Option<Kind>,
    pub refusal: Option<String>,
    pub seconds: f32,
    /// What the base's grey was tinted by to match the camera's.
    pub grey_gains: Option<[f32; 3]>,
}
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Direct {
    pub fitted: Scores,
    pub unconstrained: Scores,
}
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Trial {
    pub base_only: Scores,
    pub matrix_de00: f32,
}

/// A fitted profile and what was measured.
pub struct Fitted {
    pub profile: CameraProfile,
    pub report: Report,
}

/// Why a photo gets no fitted look.
#[derive(Debug)]
pub struct Refusal {
    pub reason: String,
    pub report: Option<Box<Report>>,
    pub alignment: Option<Alignment>,
}
fn refuse(reason: impl Into<String>) -> Refusal {
    Refusal {
        reason: reason.into(),
        report: None,
        alignment: None,
    }
}

/// The settings every pairing and gate render uses, whatever the user's defaults:
/// Adobe Default with `profile`, white balance as shot, the camera's built-in lens
/// correction and no lens profile.
fn pairing_recipe(m: &Metadata, profile: Option<CameraProfile>) -> Recipe {
    let mut r = Recipe::with_profiles(m, &[]);
    r.profile = profile.map(Arc::new);
    r.profile_amount = 1.;
    r.reset_white_balance(m);
    r.lens_profile = false;
    r.lens_builtin = m.lens.is_some();
    r
}

/// The render of `r` at the proxy's size, as linear sRGB.
fn full_render(
    im: &rawmakase_model::camera_data::CameraImage,
    r: &Recipe,
    size: [u32; 2],
    cancel: &AtomicBool,
) -> Result<Vec<Rgb>, Refusal> {
    let valid = r.checked().map_err(|e| refuse(format!("{e:#}")))?;
    let out = develop::render_cancellable(im, &valid, PROXY_EDGE * RENDER_FACTOR, cancel)
        .map_err(|e| refuse(format!("{e:#}")))?;
    let linear: Vec<Rgb> = out.pixels.iter().map(|p| p.map(srgb_decode)).collect();
    let (w, h, reduced) = pairs::reduce(&linear, out.width, out.height);
    if [w, h] == size {
        return Ok(reduced);
    }
    // A pixel or two of rounding, from the render's whole-pixel crop: the same picture.
    if w.abs_diff(size[0]) > 2 || h.abs_diff(size[1]) > 2 {
        return Err(refuse(format!(
            "The render is {w} × {h}, its stages {} × {}",
            size[0], size[1]
        )));
    }
    let at = |i: u32, from: u32, to: u32| {
        ((u64::from(i) * 2 + 1) * u64::from(from) / (2 * u64::from(to))) as u32
    };
    Ok((0..size[1])
        .flat_map(|y| (0..size[0]).map(move |x| (x, y)))
        .map(|(x, y)| reduced[(at(y, h, size[1]) * w + at(x, w, size[0])) as usize])
        .collect())
}

/// `im` with each pixel 1 where the sensor reached 0.99 of its white in any channel
/// (the image is white balanced by `Metadata::wb`), else 0. Sampled through the
/// render's geometry, a sample above 0 touched a clipped pixel, whatever gains the
/// lens correction or a gain map multiply it by.
fn clip_indicator(
    im: &rawmakase_model::camera_data::CameraImage,
) -> rawmakase_model::camera_data::CameraImage {
    let wb = im.metadata.wb.map(|v| v.max(1e-6));
    rawmakase_model::camera_data::CameraImage {
        width: im.width,
        height: im.height,
        recovered: Default::default(),
        pixels: im
            .pixels
            .iter()
            .map(|p| [f32::from(u8::from((0..3).any(|c| p[c] / wb[c] >= 0.99))); 3])
            .collect(),
        metadata: im.metadata.clone(),
        fast: im.fast,
        scale_factor: im.scale_factor,
        scale_clipped: im.scale_clipped,
    }
}

/// A refusal once `cancel` is set: the fit's caller no longer wants it.
fn stopped(cancel: &AtomicBool) -> Result<(), Refusal> {
    match cancel.load(std::sync::atomic::Ordering::Relaxed) {
        true => Err(refuse("Fitting cancelled")),
        false => Ok(()),
    }
}

/// The stage `stage` of `r` at the proxy's size.
fn stage(
    im: &rawmakase_model::camera_data::CameraImage,
    r: &Recipe,
    stage: develop::quality::Stage,
    cancel: &AtomicBool,
) -> Result<rawmakase_model::rendered::Rendered, Refusal> {
    develop::quality::render_stage_sized(im, r, stage, PROXY_EDGE * RENDER_FACTOR, cancel)
        .map_err(|e| refuse(format!("{e:#}")))
}

/// `profiles`, the photo's listed profiles, with Camera Look (fitted) added when
/// it is the photo's default: a photo without colour data
/// (`fitted::Eligibility::Absent`) whose fit is accepted. From the fit cache when
/// it holds the file's result, else fitted now and stored (a failure to store is
/// logged, the fit still used). Any other photo's list is returned as it is, at the
/// cost of looking at its metadata. `Recipe::with_profile_preference` takes the
/// fitted look only when nothing else applies, so the raw defaults, Reset,
/// previews and exports all start from it alike.
/// A fit stopped by `cancel` is neither used nor stored.
pub fn with_default(
    path: &Path,
    m: &Metadata,
    mut profiles: Vec<Arc<CameraProfile>>,
    cancel: &AtomicBool,
) -> Vec<Arc<CameraProfile>> {
    if fitted::eligibility(m, &profiles) != fitted::Eligibility::Absent {
        return profiles;
    }
    if let Some(p) = default_profile(path, m, &cache::dir(), cancel) {
        profiles.push(Arc::new(p));
    }
    profiles
}

/// The default fit of an `Absent` photo, through the fit cache in `dir`.
fn default_profile(
    path: &Path,
    m: &Metadata,
    dir: &Path,
    cancel: &AtomicBool,
) -> Option<CameraProfile> {
    let key = cache::FitKey::of(path).ok()?;
    if let Some(result) = cache::load(dir, &key, m) {
        return result.ok();
    }
    // One fit of a file at a time: another caller's (a virtual copy rendered
    // beside this one, say) is waited for, and its result read from the cache.
    let _claim = claim(&key, cancel)?;
    if let Some(result) = cache::load(dir, &key, m) {
        return result.ok();
    }
    // A fit that panics must not stop the photo opening: it starts from what it
    // would have without one, and is tried again next time.
    let fitted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fit(path, &FitInputs::default(), cancel)
    }));
    let Ok(fitted) = fitted else {
        eprintln!(
            "Camera Look (fitted): fitting {} stopped unexpectedly",
            path.display()
        );
        return None;
    };
    let result = fitted.map(|f| f.profile).map_err(|e| e.reason);
    // A file replaced or changed while it was fitted: the result may be of either
    // file, so it is neither used nor stored.
    if cancel.load(std::sync::atomic::Ordering::Relaxed)
        || cache::FitKey::of(path).ok().as_ref() != Some(&key)
    {
        return None;
    }
    if let Err(e) = cache::store(dir, &key, &result) {
        eprintln!(
            "Camera Look (fitted): the fit of {} isn't cached: {e:#}",
            path.display()
        );
    }
    result.ok()
}

/// The files being fitted, by fit key, and a wake-up for each fit that ends.
static FITTING: (Mutex<Vec<cache::FitKey>>, Condvar) = (Mutex::new(Vec::new()), Condvar::new());
/// The fit of a file, claimed by one caller until dropped.
struct Claim(cache::FitKey);
impl Drop for Claim {
    fn drop(&mut self) {
        let mut fitting = FITTING.0.lock().unwrap_or_else(|e| e.into_inner());
        fitting.retain(|key| *key != self.0);
        FITTING.1.notify_all();
    }
}
/// Claims the fit of `key`, waiting while another caller fits the same file;
/// `None` once `cancel` is set, also when the key comes free: both jobs are
/// often cancelled together, and a cancelled one must not start a fit.
fn claim(key: &cache::FitKey, cancel: &AtomicBool) -> Option<Claim> {
    let mut fitting = FITTING.0.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        if !fitting.contains(key) {
            break;
        }
        // Bounded, so a cancel is seen without the fit ending.
        fitting = FITTING
            .1
            .wait_timeout(fitting, std::time::Duration::from_millis(50))
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    fitting.push(key.clone());
    Some(Claim(key.clone()))
}

/// The camera's embedded JPEG of a photo, upright, as sRGB, and the look the camera
/// names for it.
pub struct CameraPreview {
    pub image: image::RgbImage,
    pub look: Option<String>,
}

/// The [`CameraPreview`] of the photo at `path`: converted from the colour space the
/// file states; one that states none is taken to be sRGB already.
pub fn preview_srgb(path: &Path) -> anyhow::Result<CameraPreview> {
    let mut raw = crate::photo::open(path)?;
    let look = raw.metadata.camera_look.clone();
    let image = crate::raw::thumbnail(&mut raw)?;
    let image = match pairs::preview_pro(path, &mut raw, &image) {
        Some(pro) => {
            let bytes = pro
                .iter()
                .flat_map(|q| {
                    mul(PRO_TO_RGB, *q).map(|v| (srgb_encode(v.clamp(0., 1.)) * 255. + 0.5) as u8)
                })
                .collect();
            image::RgbImage::from_raw(image.width(), image.height(), bytes)
                .ok_or_else(|| anyhow::anyhow!("The preview's pixels don't fit its size"))?
        }
        None => image,
    };
    Ok(CameraPreview { image, look })
}

/// Fits a camera look to the photo at `path`, or says why it can't.
pub fn fit(path: &Path, inputs: &FitInputs, cancel: &AtomicBool) -> Result<Fitted, Refusal> {
    let start = std::time::Instant::now();
    let mut raw = crate::photo::open(path).map_err(|e| refuse(format!("{e:#}")))?;
    let m = raw.metadata.clone();
    let mut report = Report {
        camera: format!("{} {}", m.make, m.model),
        matrix_source: format!("{:?}", m.matrix_source),
        ..Default::default()
    };
    let with_report = |mut e: Refusal, report: &Report| {
        let mut report = report.clone();
        report.refusal = Some(e.reason.clone());
        if let Some(a) = e.alignment {
            report.alignment = a;
        }
        report.seconds = start.elapsed().as_secs_f32();
        e.report = Some(Box::new(report));
        e
    };
    let preview = pairs::preview(path, &mut raw).map_err(|e| with_report(e, &report))?;
    report.preview = [preview.width, preview.height];
    report.space = preview.space;
    let im = raw
        .develop(Decode::Half, cancel)
        .map_err(|e| with_report(refuse(format!("{e:#}")), &report))?;
    let known = fitted::standard_base(&m);
    let result = fit_pairs(&im, &m, preview, known, inputs, cancel, &mut report);
    report.seconds = start.elapsed().as_secs_f32();
    match result {
        Ok(profile) => Ok(Fitted { profile, report }),
        Err(e) => Err(with_report(e, &report)),
    }
}

fn fit_pairs(
    im: &rawmakase_model::camera_data::CameraImage,
    m: &Metadata,
    preview: pairs::Preview,
    known: Option<CameraProfile>,
    inputs: &FitInputs,
    cancel: &AtomicBool,
    report: &mut Report,
) -> Result<CameraProfile, Refusal> {
    use develop::quality::Stage;
    let neutral = pairing_recipe(m, None);
    let camera = stage(im, &neutral, Stage::CameraInput, cancel)?;
    let clipped = stage(&clip_indicator(im), &neutral, Stage::CameraInput, cancel)?;
    let proxy = pairs::proxy(&camera, &clipped);
    let (preview, cut) = pairs::to_aspect(preview, proxy.width, proxy.height);
    report.preview_cut = cut;
    let (sampled, alignment) = pairs::align(
        &preview,
        &proxy,
        known.is_none() || inputs.drop_matrix,
        cancel,
    )?;
    report.alignment = alignment;
    let (w, h) = (proxy.width, proxy.height);
    let usable = pairs::usable(&proxy, &sampled);
    let held = pairs::holdout(w, h);
    let target = sampled.pro;
    let fitting: Vec<bool> = usable.iter().zip(&held).map(|(u, h)| *u && !h).collect();
    let testing: Vec<bool> = usable.iter().zip(&held).map(|(u, h)| *u && *h).collect();
    report.fit_pairs = fitting.iter().filter(|v| **v).count();
    report.holdout_pairs = testing.iter().filter(|v| **v).count();
    let target_srgb: Vec<Rgb> = target.iter().map(|p| mul(PRO_TO_RGB, *p)).collect();
    report.coloured = gate::coloured(&target_srgb, &testing);
    report.monochrome = pairs::monochrome(&target, &usable);
    if report.fit_pairs < 256 {
        return Err(refuse(format!("Only {} pairs to fit", report.fit_pairs)));
    }
    let pick = |v: &[Rgb], mask: &[bool]| -> Vec<Rgb> {
        v.iter()
            .zip(mask)
            .filter(|(_, m)| **m)
            .map(|(p, _)| *p)
            .collect()
    };
    let fit_target = pick(&target, &fitting);
    // The base: the camera's matrix, else one fitted from the camera input stage.
    let (base, fitted_matrix) = match known.clone().filter(|_| !inputs.drop_matrix) {
        Some(base) => (base, false),
        None => {
            let xyz: Vec<Rgb> = fit_target.iter().map(|p| mul(PRO_TO_XYZ, *p)).collect();
            let forward =
                solve::forward_matrix(&pick(&proxy.camera, &fitting), &xyz).map_err(refuse)?;
            let base = fitted::matrix_base(m, forward).map_err(|e| refuse(format!("{e:#}")))?;
            (base, true)
        }
    };
    let scene_of = |base: &CameraProfile| -> Result<Vec<Rgb>, Refusal> {
        let rendered = stage(
            im,
            &pairing_recipe(m, Some(base.clone())),
            Stage::SceneOutput,
            cancel,
        )?;
        let (sw, sh, scene) = pairs::reduce(&rendered.pixels, rendered.width, rendered.height);
        if [sw, sh] != [w, h] {
            return Err(refuse("The photo's stages differ in size"));
        }
        Ok(scene)
    };
    let mut scene = scene_of(&base)?;
    // The camera's grey, which a look's table can't move, is the base's to match.
    let mut base = base;
    if let Some(gains) = solve::grey_gains(&pick(&scene, &fitting), &fit_target) {
        base = fitted::with_grey_gains(&base, gains);
        scene = scene_of(&base)?;
        report.grey_gains = Some(gains);
    }
    // Renders judge a look at the proxy's size: a small copy serves, at a fraction of
    // the half-size photo's cost.
    let small = develop::preview(im, PROXY_EDGE * RENDER_FACTOR * 2);
    let baseline = full_render(&small, &pairing_recipe(m, known.clone()), [w, h], cancel)?;
    report.baseline = gate::score(&baseline, &target_srgb, &testing, w, h);
    if fitted_matrix {
        let base_only = full_render(
            &small,
            &pairing_recipe(m, Some(base.clone())),
            [w, h],
            cancel,
        )?;
        let matrix_de00 = known.as_ref().map_or(0., |k| {
            matrix_distance(k, &base, &pick(&proxy.camera, &fitting))
        });
        report.trial = Some(Trial {
            base_only: gate::score(&base_only, &target_srgb, &testing, w, h),
            matrix_de00,
        });
    }
    stopped(cancel)?;
    let fit_scene = pick(&scene, &fitting);
    let kinds: &[Kind] = if report.monochrome {
        &[Kind::Monochrome]
    } else {
        &[Kind::Colour, Kind::ToneOnly]
    };
    let mut chosen = None;
    stopped(cancel)?;
    // The tone-only look is the fallback: only a refused colour look needs it fitted
    // and rendered.
    for &kind in kinds {
        stopped(cancel)?;
        let mut look = match kind {
            Kind::Colour => solve::colour_look(&base, &fit_scene, &fit_target),
            Kind::ToneOnly => solve::tone_look(&base, &fit_scene, &fit_target),
            Kind::Monochrome => solve::monochrome_look(&fit_scene, &fit_target),
        };
        look.camera_look = m.camera_look.clone();
        let profile = fitted::with_look(&base, &look).map_err(|e| refuse(format!("{e:#}")))?;
        let render = full_render(
            &small,
            &pairing_recipe(m, Some(profile.clone())),
            [w, h],
            cancel,
        )?;
        let scores = gate::score(&render, &target_srgb, &testing, w, h);
        let refused = match kind {
            Kind::Colour => gate::colour_refusal(&scores, &report.baseline, report.coloured),
            Kind::ToneOnly | Kind::Monochrome => gate::tone_refusal(&scores, &report.baseline),
        };
        if kind == Kind::Colour && !cancel.load(std::sync::atomic::Ordering::Relaxed) {
            report.direct = Some(direct(
                &profile,
                &scene,
                &target_srgb,
                &fitting,
                &testing,
                w,
                h,
            ));
        }
        let accepted = refused.is_none();
        report.candidates.push(Candidate {
            kind,
            scores,
            refused,
        });
        if accepted {
            report.kind = Some(kind);
            chosen = Some(profile);
            break;
        }
    }
    chosen.ok_or_else(|| {
        let why = report
            .candidates
            .iter()
            .filter_map(|c| c.refused.clone())
            .collect::<Vec<_>>()
            .join("; ");
        refuse(format!("No fitted look beats the baseline: {why}"))
    })
}

/// The fitted colour look and the unconstrained reference fit, each on the scene
/// stage's output directly, scored on the held-out pixels.
fn direct(
    profile: &CameraProfile,
    scene: &[Rgb],
    target_srgb: &[Rgb],
    fitting: &[bool],
    testing: &[bool],
    w: u32,
    h: u32,
) -> Direct {
    let fitted: Vec<Rgb> = scene.iter().map(|s| profile.finish(*s)).collect();
    let (fs, ft): (Vec<Rgb>, Vec<Rgb>) = scene
        .iter()
        .zip(target_srgb)
        .zip(fitting)
        .filter(|(_, m)| **m)
        .map(|((s, t), _)| (*s, *t))
        .unzip();
    let lattice = solve::Lattice::fit(profile, &fs, &ft);
    let free: Vec<Rgb> = scene.iter().map(|s| lattice.apply(*s)).collect();
    Direct {
        fitted: gate::score(&fitted, target_srgb, testing, w, h),
        unconstrained: gate::score(&free, target_srgb, testing, w, h),
    }
}

/// The mean CIEDE2000 between the colours two profiles' matrices give `camera`
/// (white-balanced camera RGB), at D65.
fn matrix_distance(a: &CameraProfile, b: &CameraProfile, camera: &[Rgb]) -> f32 {
    let (ma, mb) = (a.camera_matrix(6504.), b.camera_matrix(6504.));
    let to_lab = |m, c: Rgb| {
        let srgb = mul(PRO_TO_RGB, mul(m, c)).map(|v| f64::from(v.clamp(0., 1.)));
        crate::half_check::lab(srgb)
    };
    let total: f64 = camera
        .iter()
        .map(|c| crate::half_check::delta_e2000(to_lab(ma, *c), to_lab(mb, *c)))
        .sum();
    (total / camera.len().max(1) as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chart(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(format!(
            "{}/../../tests/corpus/charts/{name}.dng",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    /// Only a photo without colour data gets a default fit, through the cache: a
    /// photo with a profile is never fitted, and a photo whose fit is refused (no
    /// preview here) is not fitted again.
    #[test]
    fn only_a_photo_without_colour_data_is_fitted_by_default_and_only_once() {
        let named = chart("synthetic-d65");
        let m = crate::photo::open(&named).unwrap().metadata;
        let (profiles, _) = rawmakase_model::camera_profiles::installed(&m);
        let cancel = AtomicBool::new(false);
        assert_eq!(
            with_default(&named, &m, profiles.clone(), &cancel),
            profiles
        );
        // The chart with its colour tags renamed: no colour data at all.
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(&named).unwrap();
        let first = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes(bytes[first..first + 2].try_into().unwrap()) as usize;
        for i in 0..count {
            let at = first + 2 + i * 12;
            let tag = u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
            if [50721, 50722, 50938, 50939, 50964, 50965, 50982].contains(&tag) {
                bytes[at..at + 2].copy_from_slice(&(tag + 10000).to_le_bytes());
            }
        }
        let bare = dir.path().join("bare.dng");
        std::fs::write(&bare, &bytes).unwrap();
        let m = crate::photo::open(&bare).unwrap().metadata;
        assert_eq!(fitted::eligibility(&m, &[]), fitted::Eligibility::Absent);
        let cache = dir.path().join("cache");
        assert!(default_profile(&bare, &m, &cache, &cancel).is_none());
        let key = cache::FitKey::of(&bare).unwrap();
        assert!(
            matches!(cache::load(&cache, &key, &m), Some(Err(_))),
            "the refusal is kept"
        );
        // Cancelled: nothing used, nothing stored (another file: the cache is keyed
        // by content, so a copy would share the first one's entry).
        let other = dir.path().join("other.dng");
        bytes.push(0);
        std::fs::write(&other, &bytes).unwrap();
        assert!(default_profile(&other, &m, &cache, &AtomicBool::new(true)).is_none());
        assert!(cache::load(&cache, &cache::FitKey::of(&other).unwrap(), &m).is_none());
    }

    /// While a file is fitted, another caller for it waits instead of fitting it
    /// too, gives up once cancelled, and goes on once the first has finished.
    #[test]
    fn a_file_is_fitted_by_one_caller_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.dng");
        std::fs::write(&path, b"one file, fitted once").unwrap();
        let key = cache::FitKey::of(&path).unwrap();
        let first = claim(&key, &AtomicBool::new(false)).unwrap();
        // Cancelled while it waits.
        assert!(claim(&key, &AtomicBool::new(true)).is_none());
        let waiting = std::thread::spawn({
            let key = key.clone();
            move || claim(&key, &AtomicBool::new(false)).is_some()
        });
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(
            !waiting.is_finished(),
            "a second fit started beside the first"
        );
        drop(first);
        assert!(waiting.join().unwrap());
        // Released again: the same file is free, but not to a cancelled job,
        // as when the holder and its waiter are cancelled at once.
        assert!(claim(&key, &AtomicBool::new(true)).is_none());
        assert!(claim(&key, &AtomicBool::new(false)).is_some());
    }

    /// A photo whose lens correction brightens the corners: bright but unclipped
    /// samples there are not taken for clipped, and a clipped one still is.
    #[test]
    fn clipping_is_the_sensor_s_whatever_the_lens_gain() {
        use rawmakase_model::camera_data::CameraImage;
        let wb = [2., 1., 1.5];
        let (w, h) = (96u32, 64u32);
        let mut pixels: Vec<Rgb> = vec![[0.6 * wb[0], 0.6, 0.6 * wb[2]]; (w * h) as usize];
        let clipped_at = (h / 2 * w + w / 2) as usize;
        pixels[clipped_at] = [1.0 * wb[0], 0.6, 0.6 * wb[2]];
        let im = CameraImage {
            width: w,
            height: h,
            pixels,
            metadata: Metadata {
                width: w,
                height: h,
                wb,
                lens: Some(rawmakase_model::optics::LensCorrection {
                    default_on: true,
                    vignetting: Some(rawmakase_model::optics::Radial {
                        knots: vec![0., 1.2],
                        values: vec![1., 3.],
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            recovered: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        };
        let r = pairing_recipe(&im.metadata, None);
        assert!(r.lens_builtin);
        let cancel = AtomicBool::new(false);
        let at = |stage: &rawmakase_model::rendered::Rendered, x: u32, y: u32| {
            stage.pixels[(y * stage.width + x) as usize]
        };
        let camera =
            develop::quality::render_stage(&im, &r, develop::quality::Stage::CameraInput, &cancel)
                .unwrap();
        // The corner is brighter than the white once the lens gain is in.
        assert!(at(&camera, 0, 0)[1] > 0.99, "{:?}", at(&camera, 0, 0));
        let clipped = develop::quality::render_stage(
            &clip_indicator(&im),
            &r,
            develop::quality::Stage::CameraInput,
            &cancel,
        )
        .unwrap();
        assert_eq!(at(&clipped, 0, 0), [0.; 3]);
        assert!(at(&clipped, w / 2, h / 2)[0] > 0.);
    }
}
