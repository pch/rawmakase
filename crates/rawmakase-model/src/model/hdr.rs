//! High Dynamic Range editing, as Lightroom and Camera Raw save it: whether the edit
//! renders in HDR, its HDR limit, and the adjustments of the SDR rendition derived
//! from the HDR master (docs/records/hdr-implementation-plan.md).
//!
//! These are saved settings only. Display headroom and the view-only SDR preview are
//! not part of an edit. Values set while HDR is off are kept, so switching HDR back
//! on restores them, as Camera Raw does.
//!
//! Rendering an active HDR edit is not supported yet. [`HdrSettings::check_renderable`]
//! is the one guard every entry point uses to refuse one (XMP, Lightroom's Develop
//! settings, presets, saved edits and automation all validate the recipe), so an active
//! HDR edit is preserved and reported rather than rendered as SDR.
//!
//! Where HDR renders, [`check_recipe`] refuses the HDR master of an edit that uses an
//! adjustment whose HDR rendering has not been measured, rather than rendering it
//! wrong. The edit stays valid and keeps HDR on: its SDR rendition, which every SDR
//! view and export shows, renders it whole, and the reason is shown where the HDR
//! master is asked for.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// The HDR limit's range, in stops above graphics white (`crs:HDRMaxValue`). Camera
/// Raw 18.7 renders a limit of `L` with nothing brighter than 2^`L` times graphics
/// white. It keeps 1 and 8 and replaces 0.99 and 10 with [`DEFAULT_LIMIT`]; values
/// between 8 and 10 were not measured and are refused.
pub const LIMIT_RANGE: std::ops::RangeInclusive<f32> = 1.0..=8.0;
/// The HDR limit Camera Raw 18.7 uses when a sidecar has none or an invalid one.
pub const DEFAULT_LIMIT: f32 = 2.3;

/// Whether this release renders active HDR edits. Raised only when the CPU HDR master
/// and its SDR rendition reach every caller that renders an edit.
const RENDERS_HDR: bool = true;

/// Whether this release renders HDR: active HDR edits and HDR camera profiles.
pub const fn renders() -> bool {
    RENDERS_HDR
}

pub use crate::color::hdr_output::roll_off;

/// The HDR limit a new HDR file starts with: the fewest whole stops whose roll-off
/// (from [`LIMIT_KNEE`](crate::color::hdr_output::LIMIT_KNEE) of the limit) starts
/// at or above its `peak`, so its own light renders as it is. A peak beyond the
/// largest limit's knee gets that limit, and its brightest light rolls off; a black
/// or non-finite peak gets the smallest.
pub fn limit_containing(peak: f32) -> f32 {
    let (low, high) = (*LIMIT_RANGE.start(), *LIMIT_RANGE.end());
    let knee = |limit: f32| crate::color::hdr_output::LIMIT_KNEE * limit.exp2();
    let stops = (peak / crate::color::hdr_output::LIMIT_KNEE).log2().ceil();
    if !stops.is_finite() {
        return low;
    }
    // `log2` can round a peak just past a knee down onto it: the knee is checked.
    let mut limit = stops.clamp(low, high);
    if knee(limit) < peak && limit < high {
        limit += 1.;
    }
    limit
}

/// An edit's HDR settings: its mode, limit and SDR rendition.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HdrSettings {
    /// Lightroom's HDR switch (`crs:HDREditMode`).
    pub enabled: bool,
    /// The HDR limit, in stops above graphics white (`crs:HDRMaxValue`).
    pub limit: f32,
    /// The SDR rendition's adjustments. They never change the HDR master.
    pub sdr: SdrRendition,
}

/// The SDR rendition's sliders, as Camera Raw's `crs:SDR*` values divided by 100
/// (−1 to 1, 0 by default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SdrRendition {
    pub brightness: f32,
    pub contrast: f32,
    pub clarity: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blend: f32,
}

impl Default for HdrSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            limit: DEFAULT_LIMIT,
            sdr: SdrRendition::default(),
        }
    }
}

impl SdrRendition {
    /// Why the SDR rendition doesn't render these sliders, if it doesn't: one is set
    /// beyond what was measured to render as Camera Raw 18.7 changes its own SDR
    /// rendition, as one change of luminance (docs/records/hdr-stage1-results.md#sdr-rendition).
    /// Within those ranges each slider's change of the rendition is the same curve of
    /// luminance on every photo measured; Clarity is a local operator and Blend
    /// ("Highlight Saturation") changes saturation, neither measured to render. All the
    /// sliders render or none does, so an edit's SDR rendition is never half of them.
    pub fn unrendered(&self) -> Option<&'static str> {
        let ranges: [(&'static str, f32, std::ops::RangeInclusive<f32>); 7] = [
            ("Brightness below −50", self.brightness, -0.5..=1.),
            ("Contrast above +50", self.contrast, -1. ..=0.5),
            ("Whites above +50", self.whites, -1. ..=0.5),
            ("Positive Shadows", self.shadows, -1. ..=0.),
            ("Negative Highlights", self.highlights, 0. ..=1.),
            ("Clarity", self.clarity, 0. ..=0.),
            ("Highlight Saturation", self.blend, 0. ..=0.),
        ];
        ranges
            .into_iter()
            .find_map(|(name, v, range)| (!range.contains(&v)).then_some(name))
    }
    /// The sliders in the order of Camera Raw's keys,
    /// [`crate::model::camera_raw::SDR_RENDITION`].
    pub fn values_mut(&mut self) -> [&mut f32; 7] {
        [
            &mut self.brightness,
            &mut self.contrast,
            &mut self.clarity,
            &mut self.highlights,
            &mut self.shadows,
            &mut self.whites,
            &mut self.blend,
        ]
    }
    /// [`Self::values_mut`], read.
    pub fn values(&self) -> [f32; 7] {
        let mut copy = *self;
        copy.values_mut().map(|v| *v)
    }
}

impl HdrSettings {
    /// The brightest value an HDR render of these settings may have: 2^limit times
    /// graphics white.
    pub fn peak(&self) -> f32 {
        self.limit.exp2()
    }
    /// HDR off with every value at its default: not saved, so releases that predate HDR
    /// read the recipe unchanged.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// Whether the edit renders in HDR. Values kept while HDR is off do not count: a
    /// release that predates HDR keeps them as unknown settings and renders the edit
    /// correctly as SDR.
    pub fn is_active(&self) -> bool {
        self.enabled
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.limit.is_finite() && LIMIT_RANGE.contains(&self.limit),
            "HDR limit out of bounds"
        );
        ensure!(
            self.sdr
                .values()
                .iter()
                .all(|v| v.is_finite() && (-1. ..=1.).contains(v)),
            "Invalid SDR rendition adjustment"
        );
        Ok(())
    }
    /// Refuses an active HDR edit while this release cannot render one. The edit is
    /// kept as it was: callers preserve it and report the error.
    pub fn check_renderable(&self) -> Result<()> {
        ensure!(
            RENDERS_HDR || !self.is_active(),
            "HDR editing is preserved but not supported yet"
        );
        Ok(())
    }
}

/// The highest Whites (−1 to 1) the HDR master renders.
const POSITIVE_WHITES_MEASURED: f32 = 0.5;
/// The largest positive band Luminance (0 to 1) the HDR master renders; negative renders
/// down to −100. Camera Raw's positive HDR Luminance strengthens with brightness where
/// its SDR one rolls off, which the SDR-fitted grids can't follow at +100
/// (docs/records/hdr-stage1-results.md#band-luminance).
const BAND_LUMINANCE_MEASURED: f32 = 0.4;
/// The largest Clarity, either sign (−1 to 1), the HDR master renders: up to ±50 the
/// master's (the SDR operator, stronger beside light above white) departs from Camera
/// Raw's HDR effect no further than the SDR rendition from its SDR one; at +100 it does
/// (docs/records/hdr-stage1-results.md#presence).
const CLARITY_MEASURED: f32 = 0.5;
/// The most negative Dehaze (−1 to 1) the HDR master renders: positive Dehaze renders
/// up to +100, where its HDR error on photos is its SDR one; at −50 it is too, and at
/// −70 and −100 it is further from Camera Raw below white than in SDR
/// (docs/records/hdr-stage1-results.md#presence).
const DEHAZE_MEASURED: f32 = -0.5;

/// Refuses the HDR master of an active HDR edit that this release can render only in
/// SDR: an adjustment, look or profile whose rendering above graphics white has not
/// been measured against Camera Raw (docs/records/hdr-stage1-results.md). An edit
/// with HDR off, or with nothing but measured adjustments, passes.
///
/// Measured: white balance, Exposure, Contrast, Shadows, Whites up to +50, Blacks, the HDR limit,
/// Color Grading (the SDR curves in a domain that compresses the HDR range),
/// the post-crop vignette (without a darkening vignette's Highlights) and grain,
/// the master point curve, the channels' point curves and the parametric curve, and
/// Levels as a point curve of its shape ([`crate::color::hdr_curve`]), Texture (on the
/// camera's samples, as in SDR), Clarity from −50 to +50 and Dehaze from −50 to +100
/// (the SDR operators, unclipped),
/// Point Color (its swatches in colour grading's HDR domain),
/// masks whose sliders are Temp, Tint, Exposure, Contrast, Shadows, Texture, Color,
/// Sharpness and Noise, Saturation within [`masked_saturation_measured`], Clarity and Dehaze within [`masked_clarity_measured`] and
/// [`masked_dehaze_measured`],
/// camera profiles
/// (their curves within [`crate::camera_profiles::hdr_tone`]'s family), looks (their
/// tables and curve, [`crate::camera_profiles::hdr_look`], at any Profile Amount, and
/// their own settings checked as the user's), and what works
/// on the camera's samples before tone: lens corrections, geometry, noise reduction,
/// AI Denoise, Red Eye, Heal and Clone, and output sharpening. The SDR rendition's sliders never
/// change the master, so they are not checked.
pub fn check_recipe(r: &super::recipe::Recipe) -> Result<()> {
    check(r, true)
}

/// [`check_recipe`] of a raw's recipe already resolved for its master
/// ([`super::recipe::Recipe::resolved_hdr`]): its look's settings are in its own sliders
/// already, so they aren't added a second time.
pub fn check_resolved_recipe(r: &super::recipe::Recipe) -> Result<()> {
    check(r, false)
}

/// [`check_recipe`], adding a look's settings to the user's when `fold`.
fn check(r: &super::recipe::Recipe, fold: bool) -> Result<()> {
    // What renders: a switched-off panel's adjustments do not.
    let r = r.as_rendered();
    if !r.hdr.is_active() {
        return Ok(());
    }
    // Without one, the camera matrix with the ACR3 curve renders it (`Recipe::resolved`).
    if let Some(profile) = &r.profile {
        profile.hdr_tone()?;
    }
    if let Some(name) = unmeasured(&r) {
        anyhow::bail!("{name} is not supported in HDR yet");
    }
    if let Some(name) = r
        .masks
        .iter()
        .filter(|g| g.is_active())
        .find_map(|g| unrendered_mask_slider(&g.adjust))
    {
        anyhow::bail!("A mask's {name} is not supported in HDR yet");
    }
    if !masked_saturation_measured(&r) {
        anyhow::bail!("A mask's Saturation outside 0 to +50 is not supported in HDR yet");
    }
    if !masked_clarity_measured(&r) {
        anyhow::bail!(
            "A mask's Clarity outside 0 to +50, with the photo's own, is not supported in HDR yet"
        );
    }
    if !masked_dehaze_measured(&r) {
        anyhow::bail!(
            "A mask's Dehaze outside 0 to +30, or above +100 with the photo's own, is not supported in HDR yet"
        );
    }
    // A look's own settings render as the user's sliders do, added to them, and its
    // table and curve at the Profile Amount (`Recipe::with_profile_adjustments`): each
    // is checked as that slider is.
    if !fold {
        return Ok(());
    }
    let folded = r.with_profile_adjustments();
    if let std::borrow::Cow::Owned(folded) = &folded {
        if let Some(profile) = &folded.profile {
            profile.hdr_tone()?;
        }
        if let Some(name) = unmeasured(folded) {
            anyhow::bail!("{name}, which the look sets, is not supported in HDR yet");
        }
        // A look's Clarity adds to the photo's own, so a mask's Clarity is bounded by
        // their sum.
        if !masked_clarity_measured(folded) {
            anyhow::bail!(
                "A mask's Clarity outside 0 to +50, with the photo's and the look's own, is not supported in HDR yet"
            );
        }
        if !masked_dehaze_measured(folded) {
            anyhow::bail!(
                "A mask's Dehaze above +100 with the photo's and the look's own is not supported in HDR yet"
            );
        }
    }
    Ok(())
}

/// The first adjustment of `r` (its panels as rendered) whose HDR rendering is not
/// measured, by name.
fn unmeasured(r: &super::recipe::Recipe) -> Option<&'static str> {
    // The SDR rendition's sliders are not here: they never change the master (Camera
    // Raw 18.7's is bit-identical with each at ±50), so it renders with them set.
    let unmeasured: [(&'static str, bool); 9] = [
        // The master's Defringe follows its colour stage, after the black & white mix,
        // where it would change nothing; SDR's precedes the mix
        // (docs/records/hdr-stage1-results.md#defringe).
        (
            "Defringe with Black & White",
            r.effects.defringe != [0.; 2] && r.effects.monochrome,
        ),
        // Camera Raw's positive Whites follows the photo's level, which twice the 99th
        // percentile predicts to 0.17 stop; above +50 that error grows past the HDR
        // tolerances (docs/records/hdr-stage1-results.md#whites).
        ("Whites above +50", r.whites > POSITIVE_WHITES_MEASURED),
        // Camera Raw's HDR Highlights levels off (negative) or fades (positive) above
        // white, unlike the SDR-trained rule; Shadows renders as in SDR.
        ("Highlights", r.highlights != 0.),
        // Point curves were measured ending at white; one whose last point is left
        // of it holds its value beyond, which Camera Raw was not seen to do above
        // white.
        (
            "A point curve that ends left of white",
            std::iter::once(&r.curve)
                .chain(&r.effects.channels)
                .any(|c| c.points.last().is_some_and(|p| p[0] < 1.)),
        ),
        (
            "Refine Saturation",
            r.curve_saturation != 1. && !r.curve.is_identity(),
        ),
        (
            "Clarity beyond ±50",
            r.effects.clarity.abs() > CLARITY_MEASURED,
        ),
        ("Dehaze below −50", r.effects.dehaze < DEHAZE_MEASURED),
        (
            "The Color Mixer's Luminance above +40",
            r.hsl.iter().any(|band| band[2] > BAND_LUMINANCE_MEASURED),
        ),
        // Camera Raw protects highlights from a darkening vignette in HDR only from
        // about four times white up (docs/records/hdr-stage1-results.md#vignette-and-grain).
        (
            "The vignette's Highlights",
            r.effects.vignette < 0.
                && r.effects.vignette_highlights > 0.
                && r.effects.vignette_style != super::effects::VignetteStyle::PaintOverlay,
        ),
    ];
    unmeasured
        .into_iter()
        .find_map(|(name, used)| used.then_some(name))
}

/// The first slider of a mask's adjustment that the HDR master doesn't render, by its
/// Lightroom name (docs/hdr.md#masks). A mask renders in the master with Temp and Tint,
/// Exposure, Contrast, Shadows, Texture, Color, Sharpness and Noise. Camera Raw 18.7's
/// HDR Whites and Blacks in a mask follow neither the SDR mask's curves nor the global
/// sliders' (docs/records/hdr-stage1-results.md#masks); SDR's Hue rotation is further
/// from Camera Raw's above white than the SDR rendition is below it; Highlights has no
/// HDR rendering globally. A mask's Saturation, Clarity and Dehaze are bounded
/// ([`masked_saturation_measured`], [`masked_clarity_measured`],
/// [`masked_dehaze_measured`]).
fn unrendered_mask_slider(a: &crate::model::masks::LocalAdjust) -> Option<&'static str> {
    [
        ("Highlights", a.highlights),
        ("Blacks", a.blacks),
        ("Hue", a.hue),
    ]
    .into_iter()
    .find_map(|(name, v)| (v != 0.).then_some(name))
}

/// The most Saturation (−1 to 1) the masks add to a pixel in the HDR master, which
/// raises a colour's saturation keeping its value, as Camera Raw's HDR mask Saturation
/// does: at +50 on six photos it is within the acceptance rule; at +100 on the colour
/// patches, and below 0, it is further from Camera Raw above white than the SDR
/// rendition is below it (docs/records/hdr-stage1-results.md#masks).
const MASK_SATURATION_MEASURED: f32 = 0.5;

/// Whether the masks' Saturation stays in the range measured for the master, 0 to +50,
/// overlapping masks added. As for Clarity, each active mask's Saturation at its Amount
/// is taken as reaching every pixel.
fn masked_saturation_measured(r: &super::recipe::Recipe) -> bool {
    let masks: Vec<f32> = r
        .masks
        .iter()
        .filter(|g| g.is_active() && g.adjust.saturation != 0.)
        .map(|g| g.adjust.saturation * g.amount)
        .collect();
    masks.iter().all(|v| *v >= 0.) && masks.iter().sum::<f32>() <= MASK_SATURATION_MEASURED
}

/// Whether every pixel's Clarity stays in the range the global slider renders in, 0 to
/// +50, when masks add theirs to the photo's own. A mask's Clarity works in the global
/// slider's stage, so a pixel renders the global slider at the sum
/// (docs/records/hdr-stage1-results.md#masks). The bound is conservative: each active
/// mask's Clarity at its Amount is taken as reaching every pixel, positive masks and
/// negative ones apart, whatever its shape covers.
fn masked_clarity_measured(r: &super::recipe::Recipe) -> bool {
    let masks: Vec<_> = r
        .masks
        .iter()
        .filter(|g| g.is_active() && g.adjust.clarity != 0.)
        .collect();
    if masks.is_empty() {
        return true;
    }
    let (lo, hi) = masks
        .iter()
        .fold((r.effects.clarity, r.effects.clarity), |(lo, hi), g| {
            let v = g.adjust.clarity * g.amount;
            (lo + v.min(0.), hi + v.max(0.))
        });
    lo >= 0. && hi <= CLARITY_MEASURED
}

/// The most Dehaze (−1 to 1) the masks add to a pixel in the HDR master: Camera Raw
/// 18.7's HDR render of a mask's Dehaze +30 on six photos is as close to the master as
/// its SDR render to the SDR rendition, and −30's is further
/// (docs/records/hdr-stage1-results.md#masks).
const MASK_DEHAZE_MEASURED: f32 = 0.3;

/// Whether the masks' Dehaze stays in the range measured for them, 0 to +30, and, with
/// the photo's own, within the global slider's, up to +100. A mask's Dehaze works in
/// the global slider's stage, so a pixel renders the global slider at the sum. As for
/// Clarity, each active mask's Dehaze at its Amount is taken as reaching every pixel.
fn masked_dehaze_measured(r: &super::recipe::Recipe) -> bool {
    let masks: Vec<f32> = r
        .masks
        .iter()
        .filter(|g| g.is_active() && g.adjust.dehaze != 0.)
        .map(|g| g.adjust.dehaze * g.amount)
        .collect();
    if masks.is_empty() {
        return true;
    }
    let added: f32 = masks.iter().sum();
    masks.iter().all(|v| *v >= 0.)
        && added <= MASK_DEHAZE_MEASURED
        && r.effects.dehaze + added <= 1.
}

/// Whether `r` renders an HDR master of the photo `m` describes, and if not, why;
/// every caller asks this, so the Basic panel's note, Develop's HDR preview,
/// export and the renderer agree. With HDR off there is nothing to check.
/// - A raw: [`check_recipe`].
/// - An HDR file: [`check_hdr_file`].
/// - An SDR JPEG, PNG or TIFF: refused; its pseudo-scene inverts an SDR rendering,
///   which has no HDR to give.
pub fn check_for(r: &super::recipe::Recipe, m: &crate::camera_data::Metadata) -> Result<()> {
    if !r.as_rendered().hdr.is_active() {
        return Ok(());
    }
    match (m.hdr_source, m.is_rendered()) {
        (Some(_), _) => check_hdr_file(r, m),
        (None, true) => anyhow::bail!(
            "HDR editing of SDR JPEG, PNG, TIFF and other rendered files is not supported"
        ),
        (None, false) => check_recipe(r),
    }
}

/// Whether `r` renders an HDR master of an HDR file (docs/records/hdr-stage4-design.md):
/// its light renders as it is, with Exposure, crop, straighten, Rotate and Flip, and
/// the HDR limit. Anything else that renders differently from a new HDR file's
/// settings (`Recipe::for_rendered`) is refused, by name, until it is measured on
/// display-referred light; the edit keeps it, and its SDR rendition renders it.
pub fn check_hdr_file(r: &super::recipe::Recipe, m: &crate::camera_data::Metadata) -> Result<()> {
    let r = r.as_rendered();
    let d = super::recipe::Recipe::for_rendered(m);
    let identity_curve = |c: &crate::color::curve::ToneCurve| {
        c.points.first() == Some(&[0., 0.])
            && c.points.last() == Some(&[1., 1.])
            && c.points.iter().all(|p| p[0] == p[1])
    };
    let refused: [(&str, bool); 14] = [
        (
            "White balance",
            r.temperature != d.temperature
                || r.tint != d.tint
                || r.incremental_temperature != d.incremental_temperature
                || r.incremental_tint != d.incremental_tint
                || r.wb != d.wb
                || r.auto_white_balance.is_some(),
        ),
        (
            "A profile",
            !r.profile.as_ref().is_none_or(|p| p.is_embedded())
                || r.profile_amount != d.profile_amount,
        ),
        (
            "Tone",
            r.contrast != d.contrast
                || r.highlights != d.highlights
                || r.shadows != d.shadows
                || r.whites != d.whites
                || r.blacks != d.blacks
                || r.black_point != d.black_point
                || r.white_point != d.white_point
                || r.midtone != d.midtone,
        ),
        (
            "The Tone Curve",
            !identity_curve(&r.curve) || r.curve_saturation != d.curve_saturation,
        ),
        (
            "Color",
            r.saturation != d.saturation
                || r.vibrance != d.vibrance
                || r.hsl != d.hsl
                || !r.point_colors.is_empty()
                || r.grading != d.grading,
        ),
        ("Presence and Effects", r.effects != d.effects),
        (
            "Detail",
            r.sharpening != d.sharpening
                || r.noise_luma != d.noise_luma
                || r.noise_chroma != d.noise_chroma,
        ),
        ("AI Denoise", r.ai_denoise.enabled),
        (
            "Lens corrections",
            r.lens_builtin != d.lens_builtin
                || r.lens_profile != d.lens_profile
                || r.lens_distortion != d.lens_distortion
                || r.lens_vignetting != d.lens_vignetting
                || r.lens_manual_distortion != d.lens_manual_distortion
                || r.lens_ca != d.lens_ca,
        ),
        ("Transform", r.transform != d.transform),
        (
            "Upright",
            r.upright.mode != super::transform::UprightMode::Off,
        ),
        ("Heal and Clone", !r.retouch.is_empty()),
        ("Red Eye", !r.red_eye.is_blank()),
        (
            "Masks",
            r.masks
                .iter()
                .any(crate::model::masks::MaskGroup::is_active),
        ),
    ];
    if let Some((name, _)) = refused.iter().find(|(_, used)| *used) {
        anyhow::bail!("{name} is not supported in HDR for HDR files yet");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new HDR file's limit contains its peak below the knee, from just below a
    /// knee to just above it, clamped to the limits there are.
    #[test]
    fn the_default_limit_contains_the_peak_below_its_knee() {
        let knee = |limit: f32| crate::color::hdr_output::LIMIT_KNEE * limit.exp2();
        for limit in 1..=8 {
            let limit = limit as f32;
            assert_eq!(limit_containing(knee(limit) * 0.999), limit);
            assert_eq!(limit_containing(knee(limit)), limit);
            if limit < 8. {
                assert_eq!(limit_containing(knee(limit) * 1.001), limit + 1.);
            }
        }
        // The next float past a knee, which `log2` rounds onto it, is past it.
        for limit in 1..8 {
            let knee = knee(limit as f32);
            assert_eq!(
                limit_containing(f32::from_bits(knee.to_bits() + 1)),
                limit as f32 + 1.
            );
        }
        // 16 is past limit 4's knee (15.52): limit 5, so 16 renders as 16.
        assert_eq!(limit_containing(16.), 5.);
        assert!(roll_off(16., limit_containing(16.).exp2()) == 16.);
        assert_eq!(limit_containing(0.), 1.);
        assert_eq!(limit_containing(0.5), 1.);
        assert_eq!(limit_containing(f32::NAN), 1.);
        assert_eq!(limit_containing(1e6), 8.);
    }

    /// An HDR file renders its master with Exposure, the geometry and the limit;
    /// anything else refuses the master by name and leaves the edit as it is. An SDR
    /// rendered file has no master; a raw is checked as raws are.
    #[test]
    fn an_hdr_files_master_takes_exposure_geometry_and_the_limit() {
        use crate::camera_data::{HdrSource, Metadata, Source};
        use crate::color::hdr_output::HdrTransfer;
        let file = Metadata {
            source: Source::Rendered,
            hdr_source: Some(HdrSource {
                transfer: HdrTransfer::Pq,
                gain_map: false,
                peak: 4.,
            }),
            ..Default::default()
        };
        let mut r = super::super::recipe::Recipe::for_rendered(&file);
        r.hdr.enabled = true;
        assert!(check_for(&r, &file).is_ok());
        let mut fine = r.clone();
        fine.exposure = 1.;
        fine.crop = [0.1, 0.1, 0.9, 0.9];
        fine.straighten = 3.;
        fine.rotation = 1;
        fine.flip_x = true;
        fine.hdr.limit = 2.;
        // The SDR rendition's sliders never change the master.
        fine.hdr.sdr.brightness = 0.1;
        assert!(check_for(&fine, &file).is_ok());
        type Change = fn(&mut super::super::recipe::Recipe);
        let refusals: [(&str, Change); 7] = [
            ("White balance", |r| r.incremental_temperature = 10.),
            ("Tone", |r| r.contrast = 10.),
            ("Detail", |r| r.sharpening = 40.),
            ("AI Denoise", |r| r.ai_denoise.enabled = true),
            ("Transform", |r| r.transform.vertical = 10.),
            ("Color", |r| r.saturation = 10.),
            ("Presence and Effects", |r| r.effects.clarity = 10.),
        ];
        for (name, change) in refusals {
            let mut edit = r.clone();
            change(&mut edit);
            let error = check_for(&edit, &file).unwrap_err().to_string();
            assert!(error.starts_with(name), "{name}: {error}");
        }
        let sdr_file = Metadata {
            hdr_source: None,
            ..file.clone()
        };
        assert!(check_for(&r, &sdr_file).is_err());
        let mut off = r;
        off.hdr.enabled = false;
        off.contrast = 50.;
        assert!(check_for(&off, &file).is_ok(), "HDR off checks nothing");
    }

    #[test]
    fn values_kept_while_off_are_not_active_but_are_not_default() {
        let mut hdr = HdrSettings::default();
        assert!(hdr.is_default() && !hdr.is_active());
        hdr.limit = 3.;
        hdr.sdr.brightness = 0.3;
        assert!(!hdr.is_default() && !hdr.is_active());
        assert!(hdr.validate().is_ok() && hdr.check_renderable().is_ok());
        hdr.enabled = true;
        assert!(hdr.is_active());
        assert!(hdr.validate().is_ok());
        assert!(hdr.check_renderable().is_ok());
    }

    /// An HDR master renders with measured adjustments only. The edit itself stays
    /// valid either way, and its SDR rendition renders the rest.
    #[test]
    fn only_measured_adjustments_render_in_hdr() {
        use crate::model::recipe::Recipe;
        let mut r = Recipe::default();
        r.hdr.enabled = true;
        r.exposure = 1.;
        r.whites = -0.5;
        r.blacks = -0.3;
        r.sharpening = 0.4;
        r.effects.texture = 0.5;
        assert!(check_recipe(&r).is_ok());
        r.contrast = -0.4;
        r.shadows = 0.5;
        assert!(check_recipe(&r).is_ok());
        // Positive Whites follows the photo's level (`scene_tone::hdr_whites`), up to +50.
        r.whites = 0.5;
        assert!(check_recipe(&r).is_ok());
        r.whites = 0.51;
        assert!(
            check_recipe(&r)
                .unwrap_err()
                .to_string()
                .contains("Whites above +50")
        );
        r.whites = -0.5;
        // The colour stage renders in HDR (docs/hdr.md#the-colour-stage).
        let mut colour = r.clone();
        colour.saturation = 0.3;
        colour.vibrance = -0.2;
        colour.hsl[2] = [0.1, -0.4, 0.];
        // Band Luminance too, per channel by the scene's level, up to ±40.
        colour.hsl[5][2] = -0.4;
        colour.hsl[1][2] = 0.4;
        colour.effects.calibration[1] = [0.2, 0.3];
        colour.effects.shadow_tint = 0.1;
        colour.effects.monochrome = true;
        colour.effects.gray_mix = [0.2; 8];
        // Colour grading renders too (docs/hdr.md#color-grading).
        colour.grading = [[0.6, 0.3, 0.1], [0.1, 0.2, -0.2], [0.1, 0.5, 0.3]];
        colour.effects.global_grade = [0.5, 0.2, -0.1];
        colour.effects.balance = 0.3;
        assert!(check_recipe(&colour).is_ok());
        // The SDR rendition's sliders never change the master, which renders with them.
        let mut sdr = r.clone();
        for v in sdr.hdr.sdr.values_mut() {
            *v = -0.5;
        }
        assert!(check_recipe(&sdr).is_ok());
        // Vignette and grain render (docs/hdr.md#vignette-and-grain), and so does
        // Highlights where it does nothing: Paint Overlay, or a lightening vignette.
        let mut effects = r.clone();
        effects.effects.grain = 0.4;
        effects.effects.vignette = -0.5;
        assert!(check_recipe(&effects).is_ok());
        effects.effects.vignette_highlights = 0.6;
        effects.effects.vignette_style = crate::model::effects::VignetteStyle::PaintOverlay;
        assert!(check_recipe(&effects).is_ok());
        effects.effects.vignette_style = crate::model::effects::VignetteStyle::HighlightPriority;
        effects.effects.vignette = 0.5;
        assert!(check_recipe(&effects).is_ok());
        // A mask renders with Temp, Tint, Exposure, Contrast, Shadows, Texture, Color,
        // Sharpness, Noise and Clarity (in the global slider's range); any other slider
        // refuses the master by its name (docs/hdr.md#masks).
        let mut masked = r.clone();
        masked.masks = vec![crate::model::masks::MaskGroup {
            components: vec![crate::model::masks::MaskComponent::new(
                crate::model::masks::MaskShape::Linear {
                    from: [0., 0.],
                    to: [0., 1.],
                },
            )],
            adjust: crate::model::masks::LocalAdjust {
                temperature: 0.2,
                tint: -0.1,
                exposure: 0.7,
                shadows: 0.4,
                texture: -0.3,
                contrast: 0.3,
                whites: 0.4,
                color: [0.6, 0.4],
                sharpness: 0.5,
                noise: 0.4,
                ..Default::default()
            },
            ..Default::default()
        }];
        assert!(check_recipe(&masked).is_ok());
        type Slider = fn(&mut crate::model::masks::LocalAdjust);
        let refused: [(&str, Slider); 6] = [
            ("Highlights", |a| a.highlights = -0.2),
            ("Blacks", |a| a.blacks = -0.2),
            ("Clarity", |a| a.clarity = -0.2),
            ("Dehaze", |a| a.dehaze = -0.2),
            ("Hue", |a| a.hue = 10.),
            ("Saturation", |a| a.saturation = -0.2),
        ];
        for (name, set) in refused {
            let mut edit = masked.clone();
            set(&mut edit.masks[0].adjust);
            let error = check_recipe(&edit).unwrap_err().to_string();
            assert!(error.contains(&format!("A mask's {name}")), "{error}");
        }
        // Saturation renders from 0 to +50, overlapping masks added.
        let mut saturation = masked.clone();
        saturation.masks[0].adjust.saturation = 0.5;
        assert!(check_recipe(&saturation).is_ok());
        let mut more = saturation.clone();
        more.masks[0].adjust.saturation = 0.6;
        let error = check_recipe(&more).unwrap_err().to_string();
        assert!(
            error.contains("A mask's Saturation outside 0 to +50"),
            "{error}"
        );
        let mut second = saturation.masks[0].clone();
        second.adjust.saturation = 0.2;
        second.amount = 0.5;
        saturation.masks.push(second);
        assert!(check_recipe(&saturation).is_err());
        // Clarity renders where, with the photo's own, it stays from 0 to +50, as the
        // global slider does; overlapping masks add.
        let mut clarity = masked.clone();
        clarity.masks[0].adjust.clarity = 0.3;
        clarity.effects.clarity = 0.2;
        assert!(check_recipe(&clarity).is_ok());
        let mut second = clarity.masks[0].clone();
        second.adjust = crate::model::masks::LocalAdjust {
            clarity: 0.2,
            ..Default::default()
        };
        second.amount = 0.5;
        clarity.masks.push(second);
        let error = check_recipe(&clarity).unwrap_err().to_string();
        assert!(error.contains("A mask's Clarity"), "{error}");
        // Dehaze renders from 0 to +30 in masks, overlapping masks added, and up to
        // +100 with the photo's own.
        let mut dehaze = masked.clone();
        dehaze.masks[0].adjust.dehaze = 0.3;
        dehaze.effects.dehaze = 0.7;
        assert!(check_recipe(&dehaze).is_ok());
        for (mask, own) in [(0.4, 0.), (0.2, 0.9)] {
            let mut edit = dehaze.clone();
            edit.masks[0].adjust.dehaze = mask;
            edit.effects.dehaze = own;
            let error = check_recipe(&edit).unwrap_err().to_string();
            assert!(error.contains("A mask's Dehaze"), "{mask} {own}: {error}");
        }
        let mut overlapping = dehaze.clone();
        let mut second = overlapping.masks[0].clone();
        second.adjust.dehaze = 0.2;
        second.amount = 0.5;
        overlapping.masks.push(second);
        assert!(check_recipe(&overlapping).is_err());
        // A look's Clarity adds to the sum.
        clarity.masks.pop();
        let mut look = crate::camera_profiles::open::color(&crate::camera_data::Metadata {
            make: "Fujifilm".into(),
            model: "X100F".into(),
            wb: [2., 1., 1.8],
            daylight_wb: [2., 1., 1.8],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            cam_xyz: [[0.9, -0.3, -0.1], [-0.4, 1.2, 0.2], [-0.1, 0.2, 0.6]],
            ..Default::default()
        })
        .unwrap();
        look.enhanced.as_mut().unwrap().clarity = 0.1;
        clarity.profile = Some(std::sync::Arc::new(look));
        let error = check_recipe(&clarity).unwrap_err().to_string();
        assert!(error.contains("the look's own"), "{error}");
        masked.masks[0].adjust.clarity = -0.2;
        masked.masks[0].hidden = true;
        assert!(
            check_recipe(&masked).is_ok(),
            "a hidden mask renders nothing"
        );
        // Defringe renders, except with Black & White.
        let mut defringe = r.clone();
        defringe.effects.defringe = [0.3, 0.2];
        assert!(check_recipe(&defringe).is_ok());
        let unmeasured: [fn(&mut Recipe); 10] = [
            |r| {
                r.effects.defringe = [0.3, 0.];
                r.effects.monochrome = true;
            },
            |r| r.highlights = 0.1,
            |r| r.hsl[1][2] = 0.41,
            |r| r.hsl[5][2] = 0.6,
            |r| {
                r.effects.vignette = -0.3;
                r.effects.vignette_highlights = 0.4;
            },
            |r| r.effects.clarity = 0.6,
            |r| r.effects.clarity = -0.6,
            |r| r.effects.dehaze = -0.51,
            |r| r.effects.dehaze = -0.7,
            |r| {
                r.curve.points = vec![[0., 0.], [0.5, 0.6], [1., 1.]];
                r.curve_saturation = 0.5;
            },
        ];
        for change in unmeasured {
            let mut edit = r.clone();
            change(&mut edit);
            let error = check_recipe(&edit).unwrap_err().to_string();
            assert!(error.contains("HDR"), "{error}");
            assert!(edit.validate().is_ok());
            edit.hdr.enabled = false;
            assert!(check_recipe(&edit).is_ok());
        }
        // Point and parametric curves render, with Refine Saturation at 100%, and so
        // does a curve whose first point is right of black; one whose last point is
        // left of white does not.
        r.curve.points = vec![[0., 0.], [0.5, 0.6], [1., 1.]];
        r.effects.parametric = [0.1, -0.2, 0.3, -0.4];
        r.black_point = 0.05;
        r.white_point = 0.9;
        r.midtone = 1.2;
        assert!(check_recipe(&r).is_ok());
        r.curve.points = vec![[0.2, 0.], [1., 1.]];
        assert!(check_recipe(&r).is_ok());
        r.curve.points = vec![[0.2, 0.2], [0.8, 0.8]];
        assert!(check_recipe(&r).is_err());
        // A channel's own curve renders too.
        r.curve = Default::default();
        r.effects.channels[1].points = vec![[0., 0.], [0.5, 0.6], [1., 1.]];
        assert!(check_recipe(&r).is_ok());
        r.effects.channels[1].points = vec![[0., 0.], [0.8, 0.8]];
        assert!(check_recipe(&r).is_err());
    }

    /// Looks render their master: an HSV table and a curve, as Adobe Color is, an RGB
    /// table, B&W, a fitted Camera Look, any Profile Amount, and their own sliders and
    /// settings where the same sliders render. A look's setting whose HDR rendering is
    /// not measured refuses the master by name, saying the look sets it; the edit stays
    /// valid. Split toning renders, as colour grading does.
    #[test]
    fn looks_render_in_hdr_as_their_parts_do() {
        use crate::camera_profiles::{CameraProfile, open};
        use crate::model::recipe::Recipe;
        use std::sync::Arc;
        let m = crate::camera_data::Metadata {
            make: "Fujifilm".into(),
            model: "X100F".into(),
            wb: [2., 1., 1.8],
            daylight_wb: [2., 1., 1.8],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            cam_xyz: [[0.9, -0.3, -0.1], [-0.4, 1.2, 0.2], [-0.1, 0.2, 0.6]],
            ..Default::default()
        };
        let mut r = Recipe::default();
        r.hdr.enabled = true;
        let color = open::color(&m).unwrap();
        r.profile = Some(Arc::new(color.clone()));
        check_recipe(&r).unwrap();
        let rendered: [fn(&mut CameraProfile); 9] = [
            |p| p.enhanced.as_mut().unwrap().monochrome = true,
            |p| {
                p.enhanced.as_mut().unwrap().settings.toning =
                    Some(crate::camera_profiles::Toning::default())
            },
            |p| p.enhanced.as_mut().unwrap().settings.hsl[1][2] = -0.2,
            |p| p.enhanced.as_mut().unwrap().settings.exposure = 0.3,
            |p| p.enhanced.as_mut().unwrap().contrast = 0.2,
            |p| p.enhanced.as_mut().unwrap().shadows = 0.25,
            |p| p.enhanced.as_mut().unwrap().clarity = 0.1,
            |p| p.enhanced.as_mut().unwrap().camera_look = Some("Velvia".into()),
            |p| *p = p.clone().with_test_rgb_tables().remove(0),
        ];
        for change in rendered {
            let mut edit = r.clone();
            let mut p = color.clone();
            change(&mut p);
            edit.profile = Some(Arc::new(p));
            check_recipe(&edit).unwrap();
        }
        type Change = fn(&mut CameraProfile);
        let refused: [(Change, &str); 2] = [
            (
                |p| p.enhanced.as_mut().unwrap().clarity = 0.6,
                "Clarity beyond ±50, which the look sets",
            ),
            (
                |p| p.enhanced.as_mut().unwrap().highlights = -0.12,
                "Highlights, which the look sets",
            ),
        ];
        for (change, reason) in refused {
            let mut edit = r.clone();
            let mut p = color.clone();
            change(&mut p);
            edit.profile = Some(Arc::new(p));
            let error = check_recipe(&edit).unwrap_err().to_string();
            assert!(error.contains(reason) && error.contains("HDR"), "{error}");
            edit.hdr.enabled = false;
            assert!(check_recipe(&edit).is_ok());
        }
        // A look's Clarity is added to the user's once: checked before the recipe is
        // resolved, or after it without adding it again.
        let mut both = r.clone();
        let mut p = color;
        p.enhanced.as_mut().unwrap().clarity = 0.1;
        both.profile = Some(Arc::new(p));
        both.effects.clarity = 0.35;
        check_recipe(&both).unwrap();
        let resolved = both.with_profile_adjustments();
        assert_eq!(resolved.effects.clarity, 0.45);
        check_resolved_recipe(&resolved).unwrap();
        both.effects.clarity = 0.45;
        assert!(check_recipe(&both).is_err());
        // The user's own Highlights is named as the user's.
        let mut own = r.clone();
        own.highlights = 0.1;
        let error = check_recipe(&own).unwrap_err().to_string();
        assert_eq!(error, "Highlights is not supported in HDR yet");
        // A look with Profile Amount renders at every amount.
        r.profile = Some(Arc::new(CameraProfile::creative_for_test(&m)));
        for amount in [0., 0.5, 1., 1.5, 2.] {
            r.profile_amount = amount;
            check_recipe(&r).unwrap();
        }
    }

    /// The SDR rendition renders its sliders within the measured ranges, all or none;
    /// an HDR edit's SDR rendition keeps them, an edit with HDR off has none to render.
    #[test]
    fn the_sdr_rendition_keeps_its_sliders_only_with_hdr_on() {
        let within = SdrRendition {
            brightness: 1.,
            contrast: -1.,
            whites: 0.5,
            shadows: -0.6,
            highlights: 0.8,
            ..Default::default()
        };
        assert_eq!(within.unrendered(), None);
        for (beyond, name) in [
            (
                SdrRendition {
                    brightness: -0.51,
                    ..within
                },
                "Brightness below −50",
            ),
            (
                SdrRendition {
                    contrast: 0.6,
                    ..within
                },
                "Contrast above +50",
            ),
            (
                SdrRendition {
                    whites: 0.6,
                    ..within
                },
                "Whites above +50",
            ),
            (
                SdrRendition {
                    shadows: 0.1,
                    ..within
                },
                "Positive Shadows",
            ),
            (
                SdrRendition {
                    highlights: -0.1,
                    ..within
                },
                "Negative Highlights",
            ),
            (
                SdrRendition {
                    clarity: 0.1,
                    ..within
                },
                "Clarity",
            ),
            (
                SdrRendition {
                    blend: -0.1,
                    ..within
                },
                "Highlight Saturation",
            ),
        ] {
            assert_eq!(beyond.unrendered(), Some(name));
        }
        let m = crate::camera_data::Metadata::default();
        let mut r = crate::model::recipe::Recipe::default();
        r.hdr.sdr = within;
        r.hdr.enabled = true;
        let sdr = r.resolved(&m);
        assert!(!sdr.hdr.enabled);
        assert_eq!(sdr.hdr.sdr, within);
        r.hdr.enabled = false;
        assert_eq!(r.resolved(&m).hdr.sdr, SdrRendition::default());
        assert_eq!(r.hdr.sdr, within, "the edit keeps them");
    }

    #[test]
    fn the_limit_rolls_off_only_at_the_top_and_never_exceeds_the_peak() {
        assert_eq!(roll_off(1., 16.), 1.);
        assert_eq!(roll_off(15., 16.), 15.);
        // Camera Raw 18.7: scene 16 at limit 4 renders as 15.84.
        assert!((roll_off(16., 16.) - 15.84).abs() < 0.03);
        for v in [16.5, 100., 1e9, f32::INFINITY] {
            assert!(roll_off(v, 16.) <= 16.);
        }
        assert_eq!(roll_off(f32::NAN, 16.), 0.);
        let mut last = 0.;
        for i in 0..400 {
            let y = roll_off(i as f32 / 10., 16.);
            assert!(y >= last);
            last = y;
        }
    }

    #[test]
    fn limits_and_sliders_outside_camera_raws_ranges_are_invalid() {
        for limit in [0.99, 8.01, f32::NAN, f32::INFINITY] {
            let hdr = HdrSettings {
                limit,
                ..Default::default()
            };
            assert!(hdr.validate().is_err(), "{limit}");
        }
        for value in [1.01, -1.01, f32::NAN] {
            let mut hdr = HdrSettings::default();
            hdr.sdr.blend = value;
            assert!(hdr.validate().is_err(), "{value}");
        }
    }
}
