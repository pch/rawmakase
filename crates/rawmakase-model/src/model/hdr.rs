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

/// Refuses the HDR master of an active HDR edit that this release can render only in
/// SDR: an adjustment, look or profile whose rendering above graphics white has not
/// been measured against Camera Raw (docs/records/hdr-stage1-results.md). An edit
/// with HDR off, or with nothing but measured adjustments, passes.
///
/// Measured: white balance, Exposure, negative Whites, Blacks, the HDR limit, camera profiles
/// (their curves within [`crate::camera_profiles::hdr_tone`]'s family), and what works
/// on the camera's samples before tone: lens corrections, geometry, noise reduction,
/// AI Denoise, Heal and Clone, and output sharpening.
pub fn check_recipe(r: &super::recipe::Recipe) -> Result<()> {
    // What renders: a switched-off panel's adjustments do not.
    let r = r.as_rendered();
    if !r.hdr.is_active() {
        return Ok(());
    }
    // Without one, the camera matrix with the ACR3 curve renders it (`Recipe::resolved`).
    if let Some(profile) = &r.profile {
        profile.hdr_tone()?;
    }
    ensure!(
        r.hdr.sdr == SdrRendition::default(),
        "The SDR rendition's adjustments are not supported in HDR yet"
    );
    // The identity over the whole domain: points on the diagonal from black to white.
    // A curve whose ends are inside 0–1 clamps the rest, so it is not one.
    let identity = |c: &crate::color::curve::ToneCurve| {
        c.points.first() == Some(&[0., 0.])
            && c.points.last() == Some(&[1., 1.])
            && c.points.iter().all(|p| p[0] == p[1])
    };
    let defaults = super::recipe::Recipe::default();
    let unmeasured: [(&str, bool); 15] = [
        ("Contrast", r.contrast != 0.),
        // Camera Raw stretches a photo whose brightest areas are below about twice
        // graphics white much further than the measured gain (+3.8 stops at scene 0.3
        // on a ramp peaking at 0.5); negative Whites follows no photo statistic.
        ("Positive Whites", r.whites > 0.),
        (
            "Highlights and Shadows",
            r.highlights != 0. || r.shadows != 0.,
        ),
        (
            "Levels",
            r.black_point != defaults.black_point
                || r.white_point != defaults.white_point
                || r.midtone != defaults.midtone,
        ),
        (
            "The Tone Curve",
            !identity(&r.curve)
                || !r.effects.channels.iter().all(identity)
                || r.effects.parametric != [0.; 4],
        ),
        (
            "Presence",
            r.saturation != 0.
                || r.vibrance != 0.
                || r.effects.clarity != 0.
                || r.effects.texture != 0.
                || r.effects.dehaze != 0.,
        ),
        ("The Color Mixer", r.hsl != [[0.; 3]; 8]),
        ("Point Color", !r.point_colors.is_empty()),
        (
            "Color Grading",
            r.grading != [[0.; 3]; 3] || r.effects.global_grade != [0.; 3],
        ),
        (
            "Calibration",
            r.effects.calibration != [[0.; 2]; 3] || r.effects.shadow_tint != 0.,
        ),
        ("Black & White", r.effects.monochrome),
        ("Defringe", r.effects.defringe != [0.; 2]),
        ("Effects", r.effects.grain != 0. || r.effects.vignette != 0.),
        (
            "Masks",
            r.masks
                .iter()
                .any(crate::model::masks::MaskGroup::is_active),
        ),
        ("Red Eye", !r.red_eye.is_blank()),
    ];
    if let Some((name, _)) = unmeasured.iter().find(|(_, used)| *used) {
        anyhow::bail!("{name} is not supported in HDR yet");
    }
    Ok(())
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
    let refused: [(&str, bool); 15] = [
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
        (
            "The SDR rendition's adjustments",
            r.hdr.sdr != SdrRendition::default(),
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
        assert!(check_for(&fine, &file).is_ok());
        type Change = fn(&mut super::super::recipe::Recipe);
        let refusals: [(&str, Change); 8] = [
            ("White balance", |r| r.incremental_temperature = 10.),
            ("Tone", |r| r.contrast = 10.),
            ("Detail", |r| r.sharpening = 40.),
            ("AI Denoise", |r| r.ai_denoise.enabled = true),
            ("Transform", |r| r.transform.vertical = 10.),
            ("Color", |r| r.saturation = 10.),
            ("Presence and Effects", |r| r.effects.clarity = 10.),
            ("The SDR rendition's adjustments", |r| {
                r.hdr.sdr.brightness = 10.
            }),
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
        assert!(check_recipe(&r).is_ok());
        let unmeasured: [fn(&mut Recipe); 6] = [
            |r| r.contrast = 0.1,
            |r| r.shadows = 0.1,
            |r| r.effects.clarity = 0.1,
            |r| r.saturation = 0.1,
            |r| r.curve.points = vec![[0., 0.], [0.5, 0.6], [1., 1.]],
            |r| r.hdr.sdr.brightness = 0.1,
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
        // A curve with extra points on the diagonal is still the identity; one whose
        // ends stop short of black and white clamps, so it is not.
        r.curve.points = vec![[0., 0.], [0.5, 0.5], [1., 1.]];
        assert!(check_recipe(&r).is_ok());
        r.curve.points = vec![[0.2, 0.2], [0.8, 0.8]];
        assert!(check_recipe(&r).is_err());
        r.curve = Default::default();
        r.effects.channels[1].points = vec![[0.2, 0.2], [0.8, 0.8]];
        assert!(check_recipe(&r).is_err());
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
