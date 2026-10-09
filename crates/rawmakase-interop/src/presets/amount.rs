//! Lightroom's preset Amount: after a preset that supports it is applied, a slider
//! from 0 to 200% scales the preset's changes. Each position is computed again from
//! the settings before the preset and the preset's result at 100%, both kept once
//! when the preset is applied, so dragging back and forth never drifts.
//!
//! Adobe doesn't document how each setting scales, and Camera Raw only offers the
//! slider in its own window. RAWmakase's rule:
//! - sliders move linearly from their value before the preset to the preset's,
//!   continuing past it above 100%, within the slider's range;
//! - Temperature moves evenly in mireds, as its slider does;
//! - point curves blend their outputs;
//! - hues of Color Grading follow the shorter way round, or take the other side's hue
//!   where one side has no saturation;
//! - settings that are not numbers (profile, treatment, panel switches, vignette
//!   style, grain seed) take the preset's choice at any Amount above 0, so 0% is the
//!   photo as before and every other Amount keeps the preset's character; a look with
//!   a Profile Amount scales it from 0 instead;
//!
//! Point Color swatches scale their shifts. Lens corrections, chromatic aberration,
//! crop, geometry, Upright, spots, masks and Point Color swatches added or taken away
//! don't scale: a preset that changes any of them gets no Amount.
use crate::model::recipe::{Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN};
use crate::{
    camera_data::Metadata,
    color::curve::ToneCurve,
    model::{effects::Effects, params::ParameterId, settings_groups::SettingGroup},
    xmp::Preset,
};

/// The slider's range: 0–200%, 1 = 100%.
pub const RANGE: std::ops::RangeInclusive<f32> = 0. ..=2.;

/// A preset's changes, ready to be scaled. Holds the photo's settings before the
/// preset and the preset's result at 100%.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetAmount {
    before: Recipe,
    full: Recipe,
}

/// Why a preset offers no Amount.
#[derive(Clone, Debug, PartialEq)]
pub enum NoAmount {
    /// The preset doesn't say it supports one (`crs:SupportsAmount`).
    NotOffered,
    /// It changes a setting that doesn't scale.
    Changes(SettingGroup),
}
impl std::fmt::Display for NoAmount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoAmount::NotOffered => write!(f, "this preset has no Amount"),
            NoAmount::Changes(group) => write!(f, "{} doesn't scale", group.label()),
        }
    }
}

/// Whether `preset` says it supports an Amount, as Lightroom writes it.
pub fn offered(preset: &Preset) -> bool {
    preset
        .settings
        .get("SupportsAmount")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

impl PresetAmount {
    /// The Amount for `preset`, applied over `before` as `full`; refused when the
    /// preset doesn't offer one or changes a setting that doesn't scale.
    pub fn new(preset: &Preset, before: Recipe, full: Recipe) -> Result<Self, NoAmount> {
        if !offered(preset) {
            return Err(NoAmount::NotOffered);
        }
        if let Some(group) = fixed_change(&before, &full) {
            return Err(NoAmount::Changes(group));
        }
        Ok(Self { before, full })
    }
    /// The settings at `amount` (0–2, 1 = the preset as applied).
    pub fn at(&self, amount: f32, m: &Metadata) -> Recipe {
        let t = amount.clamp(*RANGE.start(), *RANGE.end());
        if t == 1. {
            return self.full.clone();
        }
        if t == 0. {
            return self.before.clone();
        }
        blend(&self.before, &self.full, t, m)
    }
}

/// The first setting group that doesn't scale and differs between `a` and `b`.
fn fixed_change(a: &Recipe, b: &Recipe) -> Option<SettingGroup> {
    use SettingGroup::*;
    let lens = |r: &Recipe| {
        (
            r.lens_builtin,
            r.lens_profile,
            // As it renders: Default and Auto alike, and Adobe's digest left out.
            r.lens_profile_choice.rendering(),
            r.lens_distortion,
            r.lens_vignetting,
            r.lens_manual_distortion,
        )
    };
    let ca = |r: &Recipe| (r.lens_ca, r.effects.defringe, r.effects.defringe_ranges);
    let crop = |r: &Recipe| {
        (
            r.crop,
            r.straighten,
            r.constrain_crop,
            r.rotation,
            r.flip_x,
            r.flip_y,
        )
    };
    [
        (lens(a) != lens(b), LensProfileCorrections),
        (ca(a) != ca(b), ChromaticAberration),
        (crop(a) != crop(b), Crop),
        (a.transform != b.transform, TransformAdjustments),
        (a.upright != b.upright, UprightMode),
        (
            a.retouch != b.retouch || a.red_eye != b.red_eye,
            SpotRemoval,
        ),
        (a.masks != b.masks, Masking),
        (!same_swatches(a, b), ColorAdjustments),
        // Settings from a newer release: their kind is unknown.
        (a.unknown != b.unknown, ProcessVersion),
    ]
    .into_iter()
    .find_map(|(changed, group)| changed.then_some(group))
}

/// Whether both have the same Point Color swatches, apart from their shifts, which
/// scale. Swatches added or taken away don't.
fn same_swatches(a: &Recipe, b: &Recipe) -> bool {
    let unshifted = |r: &Recipe| {
        r.point_colors
            .iter()
            .map(|p| crate::model::point_color::PointColor {
                shift: [0.; 3],
                ..*p
            })
            .collect::<Vec<_>>()
    };
    unshifted(a) == unshifted(b)
}

/// A slider between `a` and `b`, kept within `lo..=hi` above 100%. Imported values may
/// lie outside the slider's range (an Exposure of +7): the range then reaches them, so
/// the Amount moves smoothly up to them and a setting left alone stays.
fn lerp(a: f32, b: f32, t: f32, lo: f32, hi: f32) -> f32 {
    if a == b {
        return a;
    }
    (a + (b - a) * t).clamp(lo.min(a).min(b), hi.max(a).max(b))
}
/// A named setting interpolated as [`lerp`] does, within its slider's range.
fn lerp_setting(id: ParameterId, a: f32, b: f32, t: f32) -> f32 {
    let range = &id.descriptor().interactive;
    lerp(a, b, t, *range.start(), *range.end())
}
fn lerp_all<const N: usize>(a: [f32; N], b: [f32; N], t: f32, lo: f32, hi: f32) -> [f32; N] {
    std::array::from_fn(|i| lerp(a[i], b[i], t, lo, hi))
}
/// A hue (0–1, wrapping) with the saturation that shows it: the shorter way round,
/// or the other side's hue where one side has none.
fn hue(a: [f32; 2], b: [f32; 2], t: f32) -> f32 {
    let ([ha, sa], [hb, sb]) = (a, b);
    if sa == 0. {
        return hb;
    }
    if sb == 0. {
        return ha;
    }
    let delta = (hb - ha + 0.5).rem_euclid(1.) - 0.5;
    (ha + delta * t).rem_euclid(1.)
}
/// A Color Grading wheel: [hue, saturation, luminance], luminance −1 to 1.
fn wheel(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        hue([a[0], a[1]], [b[0], b[1]], t),
        lerp(a[1], b[1], t, 0., 1.),
        lerp(a[2], b[2], t, -1., 1.),
    ]
}

/// The point curve whose outputs are `a`'s and `b`'s blended, at the inputs of both.
/// A natural spline is linear in its outputs, so when `a` is straight (as before most
/// presets) this is exactly the blend of the two curves.
pub(crate) fn curve(a: &ToneCurve, b: &ToneCurve, t: f32) -> ToneCurve {
    if a == b {
        return b.clone();
    }
    let xs_a: Vec<f32> = a.points.iter().map(|p| p[0]).collect();
    let xs_b: Vec<f32> = b.points.iter().map(|p| p[0]).collect();
    let same_shape = xs_a == xs_b && (a.smooth, a.natural) == (b.smooth, b.natural);
    let mut xs = if same_shape {
        xs_b
    } else {
        let mut xs: Vec<f32> = xs_a.into_iter().chain(xs_b).collect();
        xs.sort_by(f32::total_cmp);
        // Inputs closer than validation allows are one point.
        xs.dedup_by(|x, kept| *x - *kept < 0.001);
        xs
    };
    // Too many for a curve: the preset's own, so 100% is exact and nothing jumps there.
    if xs.len() > 32 {
        xs = b.points.iter().map(|p| p[0]).collect();
    }
    let points = xs
        .into_iter()
        .map(|x| {
            let (ya, yb) = if same_shape {
                let i = b.points.iter().position(|p| p[0] == x).unwrap_or(0);
                (a.points[i][1], b.points[i][1])
            } else {
                (a.evaluate(x), b.evaluate(x))
            };
            [x, lerp(ya, yb, t, 0., 1.)]
        })
        .collect();
    let blended = ToneCurve {
        points,
        smooth: b.smooth,
        natural: b.natural,
    };
    // A curve that can't be shown (a non-natural one not spanning 0–1) takes the
    // nearer side.
    if blended.validate().is_ok() {
        blended
    } else if t < 0.5 {
        a.clone()
    } else {
        b.clone()
    }
}

fn effects(a: &Effects, b: &Effects, t: f32) -> Effects {
    // Taken apart without `..`, so a new setting is a compile error until it is placed.
    let Effects {
        channels,
        parametric,
        splits,
        calibration,
        shadow_tint,
        monochrome,
        gray_mix,
        balance,
        blending,
        global_grade,
        clarity,
        texture,
        dehaze,
        grain,
        grain_size,
        grain_roughness,
        grain_seed,
        vignette,
        vignette_midpoint,
        vignette_roundness,
        vignette_feather,
        vignette_highlights,
        vignette_style,
        lens_vignette,
        lens_vignette_midpoint,
        // Chromatic aberration doesn't scale; see `fixed_change`.
        defringe,
        defringe_ranges,
        luma_detail,
        luma_contrast,
        chroma_detail,
        chroma_smoothness,
    } = b;
    // Region boundaries stay in order: past the Amount where two would meet, they stop
    // there rather than jump back.
    let ordered = |s: [f32; 3]| s[0] < s[1] && s[1] < s[2];
    let splits_at = |t: f32| lerp_all(a.splits, *splits, t, 0.01, 0.99);
    let splits = if ordered(splits_at(t)) {
        splits_at(t)
    } else {
        let (mut good, mut bad) = (t.min(1.), t);
        for _ in 0..24 {
            let mid = (good + bad) / 2.;
            if ordered(splits_at(mid)) {
                good = mid;
            } else {
                bad = mid;
            }
        }
        splits_at(good)
    };
    Effects {
        channels: std::array::from_fn(|i| curve(&a.channels[i], &channels[i], t)),
        parametric: lerp_all(a.parametric, *parametric, t, -1., 1.),
        splits,
        calibration: std::array::from_fn(|i| {
            lerp_all(a.calibration[i], calibration[i], t, -1., 1.)
        }),
        shadow_tint: lerp(a.shadow_tint, *shadow_tint, t, -1., 1.),
        monochrome: *monochrome,
        gray_mix: lerp_all(a.gray_mix, *gray_mix, t, -1., 1.),
        balance: lerp(a.balance, *balance, t, -1., 1.),
        blending: lerp(a.blending, *blending, t, 0., 1.),
        global_grade: wheel(a.global_grade, *global_grade, t),
        clarity: lerp_setting(ParameterId::Clarity, a.clarity, *clarity, t),
        texture: lerp_setting(ParameterId::Texture, a.texture, *texture, t),
        dehaze: lerp_setting(ParameterId::Dehaze, a.dehaze, *dehaze, t),
        grain: lerp(a.grain, *grain, t, 0., 1.),
        grain_size: lerp(a.grain_size, *grain_size, t, 0., 1.),
        grain_roughness: lerp(a.grain_roughness, *grain_roughness, t, 0., 1.),
        grain_seed: *grain_seed,
        vignette: lerp(a.vignette, *vignette, t, -1., 1.),
        vignette_midpoint: lerp(a.vignette_midpoint, *vignette_midpoint, t, 0., 1.),
        vignette_roundness: lerp(a.vignette_roundness, *vignette_roundness, t, -1., 1.),
        vignette_feather: lerp(a.vignette_feather, *vignette_feather, t, 0., 1.),
        vignette_highlights: lerp(a.vignette_highlights, *vignette_highlights, t, 0., 1.),
        vignette_style: *vignette_style,
        lens_vignette: lerp(a.lens_vignette, *lens_vignette, t, -1., 1.),
        lens_vignette_midpoint: lerp(a.lens_vignette_midpoint, *lens_vignette_midpoint, t, 0., 1.),
        defringe: *defringe,
        defringe_ranges: *defringe_ranges,
        luma_detail: lerp(a.luma_detail, *luma_detail, t, 0., 1.),
        luma_contrast: lerp(a.luma_contrast, *luma_contrast, t, 0., 1.),
        chroma_detail: lerp(a.chroma_detail, *chroma_detail, t, 0., 1.),
        chroma_smoothness: lerp(a.chroma_smoothness, *chroma_smoothness, t, 0., 1.),
    }
}

/// `a` moved toward `b` by `t` (0 < t ≤ 2; above 1 past `b`), for settings that may differ;
/// `fixed_change` has made sure the others are equal.
fn blend(a: &Recipe, b: &Recipe, t: f32, m: &Metadata) -> Recipe {
    // Taken apart without `..`, so a new setting is a compile error until it is placed.
    let Recipe {
        lens_builtin,
        lens_profile,
        lens_profile_choice,
        lens_distortion,
        lens_vignetting,
        lens_manual_distortion,
        lens_ca,
        effects: b_effects,
        preset_name,
        preset_settings,
        profile,
        profile_amount,
        sharpening_radius,
        sharpening_detail,
        sharpening_masking,
        exposure,
        camera_exposure,
        temperature,
        tint,
        wb,
        auto_white_balance,
        contrast,
        highlights,
        shadows,
        whites,
        blacks,
        black_point,
        white_point,
        midtone,
        curve: b_curve,
        curve_saturation,
        saturation,
        vibrance,
        hsl,
        point_colors,
        grading,
        noise_luma,
        noise_chroma,
        sharpening,
        crop,
        straighten,
        constrain_crop,
        transform,
        upright,
        rotation,
        flip_x,
        flip_y,
        retouch,
        red_eye,
        masks,
        panels,
        unknown,
    } = b;
    // Settings that aren't numbers are the preset's: this runs for Amounts above 0
    // only. A look with a Profile Amount the photo didn't have fades in from 0.
    let profile_amount = if a.profile == *profile {
        lerp(a.profile_amount, *profile_amount, t, 0., 2.)
    } else if profile.as_ref().is_some_and(|p| p.supports_amount()) {
        lerp(0., *profile_amount, t, 0., 2.)
    } else {
        *profile_amount
    };
    // Like Lightroom's slider, Temperature moves evenly in mireds.
    let mired = |k: f32| 1e6 / k;
    let temperature = (1e6
        / lerp(
            mired(a.temperature),
            mired(*temperature),
            t,
            mired(TEMPERATURE_MAX),
            mired(TEMPERATURE_MIN),
        ))
    .clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
    let tint = lerp_setting(ParameterId::Tint, a.tint, *tint, t);
    let levels = {
        let black = lerp(a.black_point, *black_point, t, 0., 0.99);
        let white = lerp(a.white_point, *white_point, t, 0.01, 1.);
        if white - black >= 0.01 {
            (black, white)
        } else {
            (*black_point, *white_point)
        }
    };
    let mut r = Recipe {
        lens_builtin: *lens_builtin,
        lens_profile: *lens_profile,
        lens_profile_choice: lens_profile_choice.clone(),
        lens_distortion: *lens_distortion,
        lens_vignetting: *lens_vignetting,
        lens_manual_distortion: *lens_manual_distortion,
        lens_ca: *lens_ca,
        effects: effects(&a.effects, b_effects, t),
        preset_name: preset_name.clone(),
        preset_settings: preset_settings.clone(),
        profile: profile.clone(),
        profile_amount,
        sharpening_radius: lerp(a.sharpening_radius, *sharpening_radius, t, 0.5, 3.),
        sharpening_detail: lerp(a.sharpening_detail, *sharpening_detail, t, 0., 1.),
        sharpening_masking: lerp(a.sharpening_masking, *sharpening_masking, t, 0., 1.),
        exposure: lerp_setting(ParameterId::Exposure, a.exposure, *exposure, t),
        camera_exposure: *camera_exposure,
        temperature,
        tint,
        wb: *wb,
        auto_white_balance: None,
        contrast: lerp_setting(ParameterId::Contrast, a.contrast, *contrast, t),
        highlights: lerp_setting(ParameterId::Highlights, a.highlights, *highlights, t),
        shadows: lerp_setting(ParameterId::Shadows, a.shadows, *shadows, t),
        whites: lerp_setting(ParameterId::Whites, a.whites, *whites, t),
        blacks: lerp_setting(ParameterId::Blacks, a.blacks, *blacks, t),
        black_point: levels.0,
        white_point: levels.1,
        midtone: lerp(a.midtone, *midtone, t, 0.1, 4.),
        curve: curve(&a.curve, b_curve, t),
        curve_saturation: lerp(a.curve_saturation, *curve_saturation, t, 0., 2.),
        saturation: lerp_setting(ParameterId::Saturation, a.saturation, *saturation, t),
        vibrance: lerp_setting(ParameterId::Vibrance, a.vibrance, *vibrance, t),
        hsl: std::array::from_fn(|i| lerp_all(a.hsl[i], hsl[i], t, -1., 1.)),
        // `same_swatches` holds: only the shifts differ.
        point_colors: a
            .point_colors
            .iter()
            .zip(point_colors)
            .map(|(a, b)| crate::model::point_color::PointColor {
                shift: lerp_all(a.shift, b.shift, t, -1., 1.),
                ..*b
            })
            .collect(),
        grading: std::array::from_fn(|i| wheel(a.grading[i], grading[i], t)),
        noise_luma: lerp(a.noise_luma, *noise_luma, t, 0., 1.),
        noise_chroma: lerp(a.noise_chroma, *noise_chroma, t, 0., 1.),
        sharpening: lerp(a.sharpening, *sharpening, t, 0., 1.),
        crop: *crop,
        straighten: *straighten,
        constrain_crop: *constrain_crop,
        transform: *transform,
        upright: upright.clone(),
        rotation: *rotation,
        flip_x: *flip_x,
        flip_y: *flip_y,
        retouch: retouch.clone(),
        red_eye: red_eye.clone(),
        masks: masks.clone(),
        panels: panels.clone(),
        unknown: unknown.clone(),
    };
    // The gains follow the controls, unless they are one side's own.
    if (r.temperature, r.tint) == (b.temperature, b.tint) && r.profile == b.profile {
        r.wb = *wb;
        r.auto_white_balance = *auto_white_balance;
    } else if (r.temperature, r.tint) == (a.temperature, a.tint) && r.profile == a.profile {
        r.wb = a.wb;
        r.auto_white_balance = a.auto_white_balance;
    } else {
        r.update_wb(m);
    }
    r
}

#[cfg(test)]
mod tests;
