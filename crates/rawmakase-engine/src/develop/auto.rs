//! One-click Auto: the Basic tone sliders, and white balance, chosen for a photo.
//!
//! White balance is estimated from the camera pixels. The tone sliders, Vibrance and
//! Saturation are predicted from a reduced render of the photo before its adjustments,
//! by linear fits to Lightroom's own Auto values.
use super::{
    pipeline::{preview, render},
    quality::recovered,
};
use crate::model::recipe::Recipe;
use crate::{
    camera_data::{CameraImage, Metadata},
    color::srgb_decode,
};
use anyhow::{Result, bail, ensure};
use std::sync::atomic::{AtomicBool, Ordering};

/// Long edge of the crop white balance is sampled on.
const WHITE_BALANCE_EDGE: u32 = 256;
/// Long edge of the cropped render the tone estimates are measured on. The fit to
/// Lightroom was measured at this size: the brightest and darkest percentiles depend on
/// how much small highlights and shadows are averaged away.
const TONE_EDGE: u32 = 1024;
/// Largest long edge of the reduced photo either is cropped from, so a tight crop never
/// copies a full-size photo (about 34 MB at 2048 × 1365).
const ANALYSIS_SOURCE_MAX: u32 = 2048;
/// Vibrance Lightroom's Auto gives nearly every photo (+15 for most, nearly all within
/// ±2; see docs/tone-controls.md).
const AUTO_VIBRANCE: f32 = 0.15;
/// Contrast Lightroom's current Auto gives every photo (+5 to +7).
const AUTO_CONTRAST: f32 = 0.06;
/// Below this brightest channel a pixel counts as without colour, as its hue is mostly
/// noise.
const CHROMA_FLOOR: f32 = 0.02;

/// `base` with white balance chosen so the photo's near-neutral areas render neutral.
pub fn auto_white_balance(im: &CameraImage, base: &Recipe) -> Result<Recipe> {
    auto_white_balance_cancellable(im, base, &AtomicBool::new(false))
}

/// [`auto_white_balance`] that stops with an error once `cancel` is set.
pub fn auto_white_balance_cancellable(
    im: &CameraImage,
    base: &Recipe,
    cancel: &AtomicBool,
) -> Result<Recipe> {
    check_cancel(cancel)?;
    // Camera pixels as decoded: highlight recovery invents colour where a channel
    // clipped, which must not count as a neutral.
    let small = preview(im, analysis_edge(im, base, WHITE_BALANCE_EDGE));
    check_cancel(cancel)?;
    let mut r = base.clone();
    fit_white_balance(&small, &mut r)?;
    r.validate()?;
    Ok(r)
}

/// `base` with Exposure, Contrast, Highlights, Shadows, Whites, Blacks and Vibrance
/// estimated from the photo before its adjustments (see [`auto_tone_basis`]), as the
/// Basic panel's Auto button applies them. White balance and every other setting of
/// `base` are kept, as in Lightroom.
pub fn auto_tone(im: &CameraImage, base: &Recipe) -> Result<Recipe> {
    auto_tone_cancellable(im, base, &AtomicBool::new(false))
}

/// [`auto_tone`] that stops with an error once `cancel` is set.
pub fn auto_tone_cancellable(
    im: &CameraImage,
    base: &Recipe,
    cancel: &AtomicBool,
) -> Result<Recipe> {
    let t = auto_tone_basis(base);
    let small = tone_copy(im, &t, cancel)?;
    let mut r = base.clone();
    AutoTone::predict(&Measure::of(&small, &t, cancel)?).apply(&mut r);
    r.validate()?;
    Ok(r)
}

/// The photo Auto tone measures: `r` as its profile, white balance, calibration, lens
/// corrections and geometry render it, without the adjustments made on top (the tone
/// sliders, curves and Levels, presence, color mixer, B&W, grading, detail, effects,
/// spots and masks). Lightroom's Auto ignores them too: a point curve that lifts black
/// far above zero still leaves its Auto Blacks at ordinary values, which measuring
/// through the curve could not give. Auto therefore gives the same result whatever
/// those adjustments are.
pub fn auto_tone_basis(r: &Recipe) -> Recipe {
    let d = Recipe::default();
    let e = &r.effects;
    Recipe {
        exposure: 0.,
        contrast: 0.,
        highlights: 0.,
        shadows: 0.,
        whites: 0.,
        blacks: 0.,
        black_point: d.black_point,
        white_point: d.white_point,
        midtone: d.midtone,
        curve: d.curve,
        saturation: d.saturation,
        vibrance: d.vibrance,
        hsl: d.hsl,
        grading: d.grading,
        noise_luma: d.noise_luma,
        noise_chroma: d.noise_chroma,
        sharpening: d.sharpening,
        sharpening_radius: d.sharpening_radius,
        sharpening_detail: d.sharpening_detail,
        sharpening_masking: d.sharpening_masking,
        retouch: Vec::new(),
        red_eye: Default::default(),
        masks: Vec::new(),
        // Bookkeeping that does not render.
        preset_name: d.preset_name,
        preset_settings: d.preset_settings,
        auto_white_balance: None,
        unknown: d.unknown,
        effects: crate::model::effects::Effects {
            calibration: e.calibration,
            shadow_tint: e.shadow_tint,
            lens_vignette: e.lens_vignette,
            lens_vignette_midpoint: e.lens_vignette_midpoint,
            defringe: e.defringe,
            defringe_ranges: e.defringe_ranges,
            ..d.effects
        },
        ..r.clone()
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Cancelled");
    }
    Ok(())
}

/// Long edge of a reduced copy of `im` whose crop under `r` has a long edge of about
/// `edge`, so a tight crop is still measured on enough pixels, up to
/// [`ANALYSIS_SOURCE_MAX`] for the whole copy.
fn analysis_edge(im: &CameraImage, r: &Recipe, edge: u32) -> u32 {
    let [x0, y0, x1, y1] = r.crop;
    // The crop's sides may be in rotated (oriented) coordinates, so the shorter source
    // side gives a crop edge that is never overestimated.
    let crop_edge = ((x1 - x0).max(y1 - y0) * im.width.min(im.height) as f32).max(1.);
    let long_edge = im.width.max(im.height);
    let scale = (edge as f32 / crop_edge).min(1.);
    ((long_edge as f32 * scale).ceil() as u32).clamp(1, long_edge.min(ANALYSIS_SOURCE_MAX))
}

/// The reduced copy the tone sliders are fitted on (see [`analysis_edge`]).
///
/// The copy is reduced from the recovered image, as the app's preview and exports
/// are, so a small clipped highlight is recovered before averaging hides it. The copy
/// is marked as already recovered, so rendering it does not recover it again.
fn tone_copy(im: &CameraImage, r: &Recipe, cancel: &AtomicBool) -> Result<CameraImage> {
    let edge = analysis_edge(im, r, TONE_EDGE);
    let small = preview(&*recovered(im, cancel)?, edge);
    let _ = small.recovered.set(std::sync::Arc::new(small.clone()));
    Ok(small)
}

/// Lightroom's Auto white balance, as measured on 133 photos from three cameras (see
/// docs/tone-controls.md): gray world, then this many mired warmer…
const WARM_MIRED: f32 = 23.;
/// …and this much greener,
const TINT_SHIFT: f32 = -3.;
/// within these Temperature and Tint limits.
const AUTO_TEMPERATURE: (f32, f32) = (2850., 7500.);
const AUTO_TINT: (f32, f32) = (0., 30.);

fn fit_white_balance(im: &CameraImage, r: &mut Recipe) -> Result<()> {
    fit_white_balance_to(&crop_samples(im, r), &im.metadata, r)
}

fn fit_white_balance_to(samples: &[[f32; 3]], m: &Metadata, r: &mut Recipe) -> Result<()> {
    r.wb = gray_world(samples)?;
    r.sync_white_balance_controls(m);
    let (lo, hi) = AUTO_TEMPERATURE;
    let mired = (1e6 / r.temperature - WARM_MIRED).max(1e6 / hi);
    r.temperature = (1e6 / mired).clamp(lo, hi);
    r.tint = (r.tint + TINT_SHIFT).clamp(AUTO_TINT.0, AUTO_TINT.1);
    r.update_wb(m);
    r.auto_white_balance = Some([r.temperature, r.tint]);
    Ok(())
}

/// Camera pixels inside the crop of `r`, sampled on a grid of the output through the
/// recipe's geometry (orientation, straighten, Transform) and lens distortion
/// correction, as rendering samples them, so areas cropped away do not pull white
/// balance.
fn crop_samples(im: &CameraImage, r: &Recipe) -> Vec<[f32; 3]> {
    let g = super::Geometry::new(im, r, WHITE_BALANCE_EDGE);
    let lens = super::image_space::LensMap::new(im, r);
    let (columns, rows) = (g.width.max(1), g.height.max(1));
    let mut samples = Vec::with_capacity((columns * rows) as usize);
    for row in 0..rows {
        for column in 0..columns {
            let u = (column as f32 + 0.5) / columns as f32;
            let v = (row as f32 + 0.5) / rows as f32;
            let [x, y] = g.source(u, v);
            let [x, y] = lens.as_ref().map_or([x, y], |l| l.forward(x, y));
            let (x, y) = (x.round(), y.round());
            if x >= 0. && y >= 0. && x < im.width as f32 && y < im.height as f32 {
                samples.push(im.pixels[y as usize * im.width as usize + x as usize]);
            }
        }
    }
    samples
}

/// Per-channel gains, relative to As Shot and normalised to green, that make the
/// average of the photo neutral (gray world). Pixels near clipping or in the noise
/// floor are ignored.
fn gray_world(pixels: &[[f32; 3]]) -> Result<[f32; 3]> {
    let mut sum = [0f64; 3];
    let mut count = 0usize;
    for p in pixels {
        if p.iter().all(|v| *v > 0.002 && *v < 0.9) {
            for c in 0..3 {
                sum[c] += p[c] as f64;
            }
            count += 1;
        }
    }
    ensure!(count >= 16, "No usable pixels for automatic white balance");
    Ok(std::array::from_fn(|c| (sum[1] / sum[c]) as f32))
}

/// Sorted luminance, brightest-channel and chroma values of a render, in display
/// encoding. Chroma is the spread of a pixel's channels relative to its brightest one,
/// 0 for gray and 1 for a fully saturated colour.
struct Measure {
    luma: Vec<f32>,
    peak: Vec<f32>,
    chroma: Vec<f32>,
}
impl Measure {
    fn of(im: &CameraImage, r: &Recipe, cancel: &AtomicBool) -> Result<Self> {
        check_cancel(cancel)?;
        let out = render(im, &r.checked()?, TONE_EDGE)?;
        ensure!(!out.pixels.is_empty(), "Nothing to measure for Auto");
        let mut luma = Vec::with_capacity(out.pixels.len());
        let mut peak = Vec::with_capacity(out.pixels.len());
        let mut chroma = Vec::with_capacity(out.pixels.len());
        for p in &out.pixels {
            let p = p.map(|v| if v.is_finite() { v.clamp(0., 1.) } else { 0. });
            let (max, min) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
            luma.push(crate::color::luminance(p));
            peak.push(max);
            chroma.push(if max > CHROMA_FLOOR {
                (max - min) / max
            } else {
                0.
            });
        }
        luma.sort_by(f32::total_cmp);
        peak.sort_by(f32::total_cmp);
        chroma.sort_by(f32::total_cmp);
        Ok(Self { luma, peak, chroma })
    }
    fn luma(&self, q: f32) -> f32 {
        percentile(&self.luma, q)
    }
    fn peak(&self, q: f32) -> f32 {
        percentile(&self.peak, q)
    }
    fn chroma(&self, q: f32) -> f32 {
        percentile(&self.chroma, q)
    }
}
fn percentile(sorted: &[f32], q: f32) -> f32 {
    sorted[((sorted.len() - 1) as f32 * q.clamp(0., 1.)).round() as usize]
}

/// The settings Auto chooses, in slider units (Exposure in EV, the rest −1 to 1): the
/// six Tone sliders, Vibrance and Saturation, which Lightroom's Auto sets too.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoTone {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
    pub saturation: f32,
}
impl AutoTone {
    /// The settings of `r` Auto chooses.
    pub fn of(r: &Recipe) -> Self {
        Self {
            exposure: r.exposure,
            contrast: r.contrast,
            highlights: r.highlights,
            shadows: r.shadows,
            whites: r.whites,
            blacks: r.blacks,
            vibrance: r.vibrance,
            saturation: r.saturation,
        }
    }

    /// Sets these in `r`, leaving every other setting as it is.
    pub fn apply(self, r: &mut Recipe) {
        r.exposure = self.exposure;
        r.contrast = self.contrast;
        r.highlights = self.highlights;
        r.shadows = self.shadows;
        r.whites = self.whites;
        r.blacks = self.blacks;
        r.vibrance = self.vibrance;
        r.saturation = self.saturation;
    }

    /// Lightroom's Auto values predicted from the photo rendered with its tone sliders
    /// at 0. Each slider is a linear fit, to Lightroom Classic's own Auto results, of
    /// the one or two percentiles that predicted it best on held-out photos, with the
    /// level of Lightroom's Auto since mid-2019 (see docs/tone-controls.md). Lightroom's
    /// Auto is a learned estimate rather than a target it solves for: it lifts a dark
    /// photo only part of the way to middle gray, and nearly always pulls Highlights
    /// down and opens Shadows.
    fn predict(m: &Measure) -> Self {
        // Brighter midtones and highlights both take Exposure down.
        let exposure = 2.22 - 2.13 * m.luma(0.4) - 1.52 * m.luma(0.99);
        // About −65; more for bright highlights, less for photos with bright shadows.
        let highlights = -36. - 51.9 * m.luma(0.9) + 26.9 * m.peak(0.25);
        // About +50; less as the shadows brighten.
        let shadows = 54.4 - 41.5 * m.peak(0.1);
        // Raised the more the brightest channel falls short of white.
        let whites = 67.5 - 52.2 * m.peak(0.998);
        // About −20, from the darkest 1% in stops: deep shadows are darkened less.
        let blacks = -38. - 1.86 * srgb_decode(m.luma(0.01)).max(1e-4).log2();
        // +2 for a photo without colour, less the more colourful its duller part is.
        let saturation = 2.07 - 10.2 * m.chroma(0.35);
        Self {
            exposure: round(exposure.clamp(-5., 5.), 100.),
            contrast: AUTO_CONTRAST,
            highlights: round(highlights.clamp(-100., 0.), 1.) / 100.,
            shadows: round(shadows.clamp(0., 100.), 1.) / 100.,
            whites: round(whites.clamp(-100., 100.), 1.) / 100.,
            blacks: round(blacks.clamp(-100., 0.), 1.) / 100.,
            vibrance: AUTO_VIBRANCE,
            saturation: round(saturation.clamp(-100., 100.), 1.) / 100.,
        }
    }
}

fn round(v: f32, steps: f32) -> f32 {
    (v * steps).round() / steps
}

/// The measurements applying XMP settings asks of a decoded photo (see
/// [`crate::xmp::PhotoMeasures`]).
pub struct Measures<'a>(pub &'a CameraImage);

impl crate::xmp::PhotoMeasures for Measures<'_> {
    fn camera_image(&self) -> &CameraImage {
        self.0
    }
    fn auto_white_balance(&self, base: &Recipe) -> Result<Recipe> {
        auto_white_balance(self.0, base)
    }
    fn auto_gray_mix(&self, r: &Recipe, m: &Metadata) -> [f32; 8] {
        let spread = super::ColorSpread::measure(self.0);
        super::AutoMix {
            spread: &spread,
            metadata: m,
        }
        .for_recipe(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::Metadata;

    /// A 96 × 64 scene of smooth gradients, all channels scaled by `cast`.
    fn scene(cast: [f32; 3], level: f32) -> CameraImage {
        let (width, height) = (96u32, 64u32);
        CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: (0..width * height)
                .map(|i| {
                    let (x, y) = ((i % width) as f32 / width as f32, (i / width) as f32);
                    let v = level * (0.02 + 0.98 * x * x) * (1. + 0.1 * (y * 0.3).sin());
                    std::array::from_fn(|c| (v * cast[c]).min(1.))
                })
                .collect(),
            metadata: Metadata {
                width,
                height,
                wb: [1.; 3],
                daylight_wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        }
    }
    /// Auto measures every photo with the same sharpening, whichever Detail settings
    /// the edit uses.
    #[test]
    fn auto_measures_with_the_default_sharpening() {
        let mut r = Recipe::default();
        r.set_sharpening_defaults();
        r.sharpening_detail = 0.9;
        assert_eq!(auto_tone_basis(&r), auto_tone_basis(&Recipe::default()));
    }
    #[test]
    fn white_balance_neutralises_a_colour_cast() {
        let cast = [1.4, 1., 0.6];
        let gains = gray_world(&scene(cast, 0.5).pixels).unwrap();
        for c in 0..3 {
            assert!(
                (gains[c] * cast[c] - 1.).abs() < 0.02,
                "{gains:?} does not undo {cast:?}"
            );
        }
    }

    /// Temperature and Tint Lightroom's Auto gives a photo whose gray world is `r`'s.
    fn lightroom_auto(r: &Recipe) -> (f32, f32) {
        let temperature = 1e6 / (1e6 / r.temperature - WARM_MIRED).max(1e6 / AUTO_TEMPERATURE.1);
        (
            temperature.clamp(AUTO_TEMPERATURE.0, AUTO_TEMPERATURE.1),
            (r.tint + TINT_SHIFT).clamp(AUTO_TINT.0, AUTO_TINT.1),
        )
    }

    #[test]
    fn white_balance_is_gray_world_made_warmer_as_in_lightroom() {
        let mut im = scene([1.1, 1., 0.9], 0.5);
        // A camera matrix gives the recipe the default camera-matrix profile.
        im.metadata.cam_xyz = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let base = Recipe::for_metadata(&im.metadata);
        let auto = auto_white_balance(&im, &base).unwrap();
        let mut gray = base;
        gray.wb = gray_world(&im.pixels).unwrap();
        gray.sync_white_balance_controls(&im.metadata);
        let (temperature, tint) = lightroom_auto(&gray);
        assert!(
            (auto.temperature - temperature).abs() < 1. && (auto.tint - tint).abs() < 0.01,
            "{} {} vs {temperature} {tint}",
            auto.temperature,
            auto.tint
        );
        assert!(auto.temperature > gray.temperature, "{}", auto.temperature);
        assert_eq!(auto.auto_white_balance, Some([auto.temperature, auto.tint]));
    }

    #[test]
    fn white_balance_stays_within_lightroom_auto_limits() {
        for cast in [[0.3, 1., 3.], [3., 1., 0.3], [1., 0.5, 1.], [1., 2., 1.]] {
            let mut im = scene(cast, 0.3);
            im.metadata.cam_xyz = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
            let auto = auto_white_balance(&im, &Recipe::for_metadata(&im.metadata)).unwrap();
            assert!(
                (AUTO_TEMPERATURE.0..=AUTO_TEMPERATURE.1).contains(&auto.temperature)
                    && (AUTO_TINT.0..=AUTO_TINT.1).contains(&auto.tint),
                "{cast:?}: {} {}",
                auto.temperature,
                auto.tint
            );
        }
    }

    #[test]
    fn tight_crops_are_measured_on_enough_pixels() {
        let im = scene([1.; 3], 0.5);
        let r = Recipe {
            crop: [0.45, 0.45, 0.5, 0.5],
            ..Default::default()
        };
        // A 960 × 640 photo, whose 5% crop is 48 × 32 source pixels: a 256 px copy of
        // the whole photo would leave about 13 × 9 of them.
        let mut big = scene([1.; 3], 0.5);
        big.width *= 10;
        big.height *= 10;
        big.metadata.width = big.width;
        big.metadata.height = big.height;
        big.pixels = (0..big.width * big.height)
            .map(|i| im.pixels[((i / big.width / 10) * im.width + i % big.width / 10) as usize])
            .collect();
        let out = render(
            &tone_copy(&big, &r, &AtomicBool::new(false)).unwrap(),
            &r.checked().unwrap(),
            0,
        )
        .unwrap();
        // Under TONE_EDGE, so every source pixel of the crop is measured.
        assert!(
            out.width >= 48 && out.height >= 32,
            "{}x{}",
            out.width,
            out.height
        );
    }

    #[test]
    fn tight_crops_of_large_photos_are_measured_on_a_bounded_copy() {
        // A thin 3000 px photo keeps the test small; a 1% crop of it would ask for the
        // full-size photo.
        let (width, height) = (3000u32, 60u32);
        let mut im = scene([1.; 3], 0.5);
        im.width = width;
        im.height = height;
        im.metadata.width = width;
        im.metadata.height = height;
        im.pixels = vec![[0.2; 3]; (width * height) as usize];
        let r = Recipe {
            crop: [0.5, 0.5, 0.51, 0.51],
            ..Default::default()
        };
        let copy = tone_copy(&im, &r, &AtomicBool::new(false)).unwrap();
        assert_eq!(copy.width.max(copy.height), ANALYSIS_SOURCE_MAX);
    }

    #[test]
    fn white_balance_beyond_the_profile_controls_follows_the_clamped_controls() {
        let mut im = scene([1.; 3], 0.5);
        // A camera matrix gives the recipe the default camera-matrix profile.
        im.metadata.cam_xyz = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let mut r = Recipe {
            wb: [0.5, 1., 0.5],
            ..Default::default()
        };
        assert!(r.color_profile(&im.metadata).is_some());
        r.sync_white_balance_controls(&im.metadata);
        let mut replayed = r.clone();
        replayed.update_wb(&im.metadata);
        for c in 0..3 {
            assert!(
                (replayed.wb[c] / r.wb[c] - 1.).abs() < 1e-3,
                "{:?} vs {:?} (temperature {} tint {})",
                replayed.wb,
                r.wb,
                r.temperature,
                r.tint
            );
        }
    }

    #[test]
    fn highlights_are_recovered_before_the_photo_is_reduced() {
        // Red clips across the bright side, so highlight recovery changes those pixels.
        let im = scene([1.4, 1., 1.], 1.);
        let r = Recipe::default();
        let recovered = super::super::quality::recover_highlights(&im);
        assert_ne!(recovered.pixels, im.pixels);
        let copy = tone_copy(&im, &r, &AtomicBool::new(false)).unwrap();
        let expected = preview(&recovered, copy.width.max(copy.height));
        assert_eq!(copy.pixels, expected.pixels);
        // Rendering the copy does not recover it a second time.
        assert_eq!(copy.recovered.get().unwrap().pixels, copy.pixels);
    }

    #[test]
    fn fallback_white_balance_sync_under_a_profile_matches_the_gains() {
        let mut im = scene([1.; 3], 0.5);
        im.metadata.cam_xyz = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        for wb in [
            [0.8, 1., 1.3],
            [1.3, 1., 0.8],
            [0.7, 1., 0.9],
            [1.1, 1., 1.4],
        ] {
            let mut r = Recipe {
                wb,
                ..Default::default()
            };
            assert!(r.color_profile(&im.metadata).is_some());
            r.sync_fallback_white_balance_controls(&im.metadata);
            let mut replayed = r.clone();
            replayed.update_wb(&im.metadata);
            for c in 0..3 {
                assert!(
                    (replayed.wb[c] / r.wb[c] - 1.).abs() < 1e-3,
                    "{wb:?}: {:?} vs {:?}",
                    replayed.wb,
                    r.wb
                );
            }
        }
    }

    #[test]
    fn a_cancelled_white_balance_estimate_stops_with_an_error() {
        let im = scene([1.; 3], 0.5);
        let cancel = AtomicBool::new(true);
        assert!(auto_white_balance_cancellable(&im, &Recipe::default(), &cancel).is_err());
    }

    #[test]
    fn white_balance_ignores_colour_invented_by_highlight_recovery() {
        // A warm left half where every other pixel clipped in red, and a cool right
        // half. Recovery rebuilds the clipped pixels as warm, below the clipping cutoff,
        // which would weigh the warm half double.
        let mut im = scene([1.; 3], 0.5);
        let width = im.width as usize;
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = (i % width, i / width);
            *p = if x >= width / 2 {
                [0.4, 0.5, 0.6]
            } else if (x + y) % 2 == 0 {
                [1., 0.5, 0.4]
            } else {
                [0.6, 0.5, 0.4]
            };
        }
        let r = Recipe::for_metadata(&im.metadata);
        let recovered = super::super::quality::recover_highlights(&im);
        assert_ne!(recovered.pixels, im.pixels);
        let mut decoded = r.clone();
        fit_white_balance_to(&crop_samples(&im, &r), &im.metadata, &mut decoded).unwrap();
        let decoded = decoded.wb;
        let estimated = auto_white_balance(&im, &r).unwrap().wb;
        for c in 0..3 {
            assert!(
                (estimated[c] / decoded[c] - 1.).abs() < 1e-4,
                "{estimated:?} vs {decoded:?}"
            );
        }
    }

    #[test]
    fn white_balance_samples_through_lens_distortion_correction() {
        use crate::optics::{LensCorrection, Radial};
        // A neutral photo with a coloured strip down each side, which the distortion
        // correction pulls out of the frame.
        let mut im = scene([1.; 3], 0.5);
        let width = im.width as usize;
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let x = i % width;
            *p = if x < 10 || x >= width - 10 {
                [0.46, 0.4, 0.34]
            } else {
                [0.4; 3]
            };
        }
        im.metadata.lens = Some(LensCorrection {
            source: "test".into(),
            default_on: true,
            vignetting: None,
            distortion: Some(Radial {
                knots: vec![0., 1.],
                values: vec![1., 0.7],
            }),
            chromatic: None,
        });
        let r = Recipe::for_metadata(&im.metadata);
        assert!(r.lens_correction(&im.metadata).is_some());
        let samples = crop_samples(&im, &r);
        assert!(!samples.is_empty());
        assert!(
            samples.iter().all(|p| *p == [0.4; 3]),
            "white balance sampled the strips the correction removes"
        );
    }

    #[test]
    fn a_cancelled_estimate_stops_with_an_error() {
        let im = scene([1.; 3], 0.5);
        let cancel = AtomicBool::new(true);
        assert!(auto_tone_cancellable(&im, &Recipe::default(), &cancel).is_err());
    }

    #[test]
    fn white_balance_comes_from_the_crop() {
        // The left half is under a warm light, the right half under a cool one.
        let mut im = scene([1.; 3], 0.5);
        let width = im.width as usize;
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let cast = if i % width < width / 2 {
                [1.3, 1., 0.7]
            } else {
                [0.7, 1., 1.3]
            };
            *p = std::array::from_fn(|c| p[c] * cast[c]);
        }
        let base = Recipe {
            crop: [0., 0., 0.45, 1.],
            ..Default::default()
        };
        let auto = auto_white_balance(&im, &base).unwrap();
        // As if the whole photo were under the warm light.
        let warm = auto_white_balance(&scene([1.3, 1., 0.7], 0.5), &Recipe::default()).unwrap();
        for c in 0..3 {
            assert!(
                (auto.wb[c] / warm.wb[c] - 1.).abs() < 0.02,
                "{:?} vs {:?}",
                auto.wb,
                warm.wb
            );
        }
    }

    #[test]
    fn neutral_photos_need_no_gray_world_correction() {
        let gains = gray_world(&scene([1.; 3], 0.5).pixels).unwrap();
        assert!(gains.iter().all(|g| (g - 1.).abs() < 1e-3), "{gains:?}");
    }

    #[test]
    fn exposure_raises_dark_photos_and_lowers_bright_ones() {
        for (level, brighter) in [(0.04, true), (3., false)] {
            let im = scene([1.; 3], level);
            let auto = auto_tone(&im, &Recipe::default()).unwrap();
            assert_eq!(auto.exposure > 0., brighter, "exposure {}", auto.exposure);
        }
    }

    #[test]
    fn auto_sets_sliders_as_lightrooms_auto_does() {
        // An evenly lit mid-tone photo. Lightroom's Auto pulls Highlights well down,
        // opens Shadows, darkens Blacks, sets Contrast to +6 and adds Vibrance and, for a
        // photo without colour, Saturation (see docs/tone-controls.md); these are the
        // fitted model's values, so a change to it shows here.
        let auto = auto_tone(&scene([1.; 3], 0.5), &Recipe::default()).unwrap();
        let got = [
            auto.exposure,
            auto.contrast,
            auto.highlights,
            auto.shadows,
            auto.whites,
            auto.blacks,
            auto.vibrance,
            auto.saturation,
        ];
        let expected = [0.5, 0.06, -0.61, 0.49, 0.33, -0.26, AUTO_VIBRANCE, 0.02];
        assert!(
            got.iter().zip(expected).all(|(g, e)| (g - e).abs() < 0.015),
            "{got:?} vs {expected:?}"
        );
    }

    #[test]
    fn contrast_is_the_same_for_every_photo_as_in_lightroom() {
        // Lightroom's current Auto gives Contrast +5 to +7 whatever the photo.
        for (cast, level) in [([1.; 3], 0.04), ([1.; 3], 3.), ([1.6, 1., 0.4], 0.5)] {
            let auto = auto_tone(&scene(cast, level), &Recipe::default()).unwrap();
            assert_eq!(auto.contrast, AUTO_CONTRAST, "{cast:?} at {level}");
        }
    }

    #[test]
    fn muted_photos_get_more_saturation_than_colourful_ones() {
        let saturation = |cast| {
            auto_tone(&scene(cast, 0.5), &Recipe::default())
                .unwrap()
                .saturation
        };
        let gray = saturation([1.; 3]);
        let colourful = saturation([1.6, 1., 0.4]);
        assert_eq!(gray, 0.02);
        assert!(colourful < 0., "{colourful}");
    }

    #[test]
    fn bright_highlights_hold_exposure_back() {
        let clean = scene([1.; 3], 0.25);
        let mut bright = clean.clone();
        let n = bright.pixels.len();
        // A light source far beyond the sensor's range covers 3% of the photo.
        for p in &mut bright.pixels[n - n * 3 / 100..] {
            *p = [8.; 3];
        }
        let base = Recipe::default();
        let clean = auto_tone(&clean, &base).unwrap().exposure;
        let bright = auto_tone(&bright, &base).unwrap().exposure;
        assert!(bright < clean - 0.1, "{bright} vs {clean}");
    }

    #[test]
    fn crushed_shadows_are_darkened_less() {
        let open = scene([1.; 3], 0.5);
        let mut crushed = open.clone();
        // Shadows crushed in the camera: 2% of the photo is black.
        let n = crushed.pixels.len();
        for p in &mut crushed.pixels[..n / 50] {
            *p = [0.; 3];
        }
        let base = Recipe::default();
        let open = auto_tone(&open, &base).unwrap().blacks;
        let crushed = auto_tone(&crushed, &base).unwrap().blacks;
        assert!(crushed > open, "{crushed} vs {open}");
        assert!(crushed < 0., "{crushed}");
    }

    #[test]
    fn auto_tone_sets_only_the_tone_sliders_vibrance_and_saturation() {
        let im = scene([1.2, 1., 0.8], 0.1);
        let mut base = Recipe {
            saturation: 0.3,
            crop: [0.1, 0.1, 0.9, 0.9],
            exposure: -3.,
            contrast: 0.9,
            temperature: 4300.,
            tint: 12.,
            wb: [1.1, 1., 0.7],
            ..Default::default()
        };
        base.effects.clarity = -0.2;
        let auto = auto_tone(&im, &base).unwrap();
        auto.validate().unwrap();
        assert_ne!(auto.exposure, base.exposure);
        // White balance, even a manual one, and everything else is as it was.
        let mut expected = auto.clone();
        expected.exposure = base.exposure;
        expected.contrast = base.contrast;
        expected.highlights = base.highlights;
        expected.shadows = base.shadows;
        expected.whites = base.whites;
        expected.blacks = base.blacks;
        expected.vibrance = base.vibrance;
        expected.saturation = base.saturation;
        assert_eq!(expected, base);
        // Estimates are independent of the tone sliders they replace.
        let untouched = Recipe {
            exposure: 0.,
            contrast: 0.,
            ..base
        };
        let fresh = auto_tone(&im, &untouched).unwrap();
        assert_eq!(fresh.exposure, auto.exposure);
        assert_eq!(fresh.whites, auto.whites);
    }

    #[test]
    fn auto_tone_measures_the_photo_before_its_adjustments() {
        let im = scene([1.2, 1., 0.8], 0.1);
        let plain = Recipe::default();
        // A lifted point curve, color and presence edits, as a film-look preset makes.
        let mut adjusted = Recipe {
            saturation: -0.2,
            vibrance: 0.3,
            ..Default::default()
        };
        adjusted.curve.points = vec![[0., 42. / 255.], [1., 1.]];
        adjusted.hsl[1] = [0.1, -0.3, -0.2];
        adjusted.effects.clarity = 0.4;
        adjusted.effects.dehaze = 0.3;
        adjusted.effects.channels[2].points = vec![[0., 0.], [0.3, 0.2], [1., 1.]];
        adjusted.effects.vignette = -0.5;
        let a = auto_tone(&im, &plain).unwrap();
        let b = auto_tone(&im, &adjusted).unwrap();
        assert_eq!(AutoTone::of(&a), AutoTone::of(&b));
        // The adjustments themselves are kept.
        assert_eq!(b.curve, adjusted.curve);
        assert_eq!(b.effects, adjusted.effects);
        assert_eq!(b.hsl, adjusted.hsl);
    }

    #[test]
    fn empty_or_black_photos_are_errors() {
        let mut im = scene([1.; 3], 0.5);
        im.pixels.iter_mut().for_each(|p| *p = [0.; 3]);
        assert!(auto_white_balance(&im, &Recipe::default()).is_err());
    }

    /// An XMP packet with `attrs` on its settings.
    fn packet(attrs: &str) -> String {
        use crate::xml::ns::{CRS, RDF};
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{RDF}"><r:Description xmlns:c="{CRS}" {attrs}></r:Description></r:RDF></x:xmpmeta>"#
        )
    }

    #[test]
    fn xmp_auto_white_balance_is_the_wb_menus_auto_on_the_presets_crop() -> Result<()> {
        let (width, height) = (32u32, 24u32);
        let m = Metadata {
            width,
            height,
            wb: [2., 1., 1.8],
            daylight_wb: [2., 1., 1.8],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        };
        let im = CameraImage {
            recovered: Default::default(),
            width,
            height,
            // A warm left half and a cool right half.
            pixels: (0..width * height)
                .map(|i| {
                    let x = i % width;
                    let v = 0.05 + 0.4 * x as f32 / width as f32;
                    if x < width / 2 {
                        [v * 1.3, v, v * 0.7]
                    } else {
                        [v * 0.7, v, v * 1.3]
                    }
                })
                .collect(),
            metadata: m.clone(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        };
        // Measured on the preset's crop: the warm half.
        let attrs = r#"c:WhiteBalance="Auto" c:CropLeft="0" c:CropTop="0" c:CropRight="0.45" c:CropBottom="1""#;
        let preset = crate::xmp::parse(std::path::Path::new("preset.xmp"), &packet(attrs))?;
        let result = preset.apply(&Recipe::default(), &m, &[], Some(&Measures(&im)))?;
        let cropped = Recipe {
            crop: result.crop,
            ..Default::default()
        };
        let auto = auto_white_balance(&im, &cropped)?;
        let whole = auto_white_balance(&im, &Recipe::default())?;
        assert_ne!(auto.wb, whole.wb);
        assert_eq!(
            (result.wb, result.temperature, result.tint),
            (auto.wb, auto.temperature, auto.tint)
        );
        assert_eq!(result.auto_white_balance, auto.auto_white_balance);
        Ok(())
    }

    #[test]
    fn xmp_auto_black_and_white_mix_is_measured_on_the_photo() -> Result<()> {
        let (width, height) = (16u32, 8u32);
        let m = Metadata {
            width,
            height,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        };
        let im = CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: (0..width * height)
                .map(|i| {
                    let v = 0.05 + 0.3 * (i % width) as f32 / width as f32;
                    [v, v, 2. * v]
                })
                .collect(),
            metadata: m.clone(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        };
        let attrs = r#"c:ConvertToGrayscale="True" c:AutoGrayscaleMix="True""#;
        let preset = crate::xmp::parse(std::path::Path::new("auto.xmp"), &packet(attrs))?;
        let base = Recipe::default();
        let r = preset.apply(&base, &m, &[], Some(&Measures(&im)))?;
        let spread = super::super::ColorSpread::measure(&im);
        let expected = super::super::AutoMix {
            spread: &spread,
            metadata: &m,
        }
        .for_recipe(&r);
        assert_ne!(expected, base.effects.gray_mix);
        assert_eq!(r.effects.gray_mix, expected);
        Ok(())
    }
}
