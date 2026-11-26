//! Develop settings some looks carry beyond their tables and the tone adjustments in
//! `Enhanced`: Exposure, Saturation, the color mixer, the parametric curve, split
//! toning and a post-crop vignette (Lightroom's Vintage 07, Modern 03 and 04), and
//! Whites, Vibrance, Texture and calibration (third-party looks such as RNI's). They
//! render with the user's settings without moving the user's sliders (see
//! `Recipe::with_profile_adjustments`).
use crate::model::camera_raw as crs;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

#[cfg(test)]
const BANDS: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];
const DEFAULT_SPLITS: [f32; 3] = [0.25, 0.5, 0.75];

/// A look's split toning: hue (0–1) and saturation (0–1) for shadows and
/// highlights, and the balance between them (−1–1).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Toning {
    pub shadows: [f32; 2],
    pub highlights: [f32; 2],
    pub balance: f32,
}
/// A look's post-crop vignette, in the recipe's units.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Vignette {
    pub amount: f32,
    pub midpoint: f32,
    pub feather: f32,
    pub roundness: f32,
    pub highlights: f32,
    /// Lightroom's style code: 1 highlight priority, 2 color priority, 3 paint.
    pub style: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LookSettings {
    /// Stops.
    pub exposure: f32,
    pub saturation: f32,
    /// Hue, saturation and luminance per color band, as `Recipe::hsl`.
    pub hsl: [[f32; 3]; 8],
    pub parametric: [f32; 4],
    pub splits: [f32; 3],
    pub toning: Option<Toning>,
    pub vignette: Option<Vignette>,
    /// Whites, Vibrance and Texture, in the recipe's units. Omitted at zero, as are the
    /// calibration settings below, so a recipe whose look has none of them reads in
    /// releases that predate them.
    #[serde(skip_serializing_if = "is_zero")]
    pub whites: f32,
    #[serde(skip_serializing_if = "is_zero")]
    pub vibrance: f32,
    #[serde(skip_serializing_if = "is_zero")]
    pub texture: f32,
    /// Calibration's hue and saturation of the red, green and blue primaries, as
    /// `Effects::calibration`.
    #[serde(skip_serializing_if = "is_zero_calibration")]
    pub calibration: [[f32; 2]; 3],
    /// Calibration's Shadows Tint.
    #[serde(skip_serializing_if = "is_zero")]
    pub shadow_tint: f32,
}
fn is_zero(v: &f32) -> bool {
    *v == 0.
}
fn is_zero_calibration(c: &[[f32; 2]; 3]) -> bool {
    c.iter().flatten().all(|v| *v == 0.)
}
impl Default for LookSettings {
    fn default() -> Self {
        Self {
            exposure: 0.,
            saturation: 0.,
            hsl: [[0.; 3]; 8],
            parametric: [0.; 4],
            splits: DEFAULT_SPLITS,
            toning: None,
            vignette: None,
            whites: 0.,
            vibrance: 0.,
            texture: 0.,
            calibration: [[0.; 2]; 3],
            shadow_tint: 0.,
        }
    }
}
impl LookSettings {
    /// Whether `key` is one of the settings this reads.
    pub(super) fn reads(key: &str) -> bool {
        // A look's own subset of Camera Raw's settings; others in a look are refused.
        let named = [
            crs::EXPOSURE,
            crs::SATURATION,
            crs::WHITES,
            crs::VIBRANCE,
            crs::TEXTURE,
            crs::SHADOW_TINT,
            crs::SPLIT_TONING_BALANCE,
            crs::POST_CROP_VIGNETTE_AMOUNT,
            crs::POST_CROP_VIGNETTE_MIDPOINT,
            crs::POST_CROP_VIGNETTE_FEATHER,
            crs::POST_CROP_VIGNETTE_ROUNDNESS,
            crs::POST_CROP_VIGNETTE_HIGHLIGHT_CONTRAST,
        ];
        key == "PostCropVignetteStyle"
            || named
                .into_iter()
                .chain(crs::PARAMETRIC)
                .chain(crs::PARAMETRIC_SPLITS)
                .chain(crs::SPLIT_TONING_HUE)
                .chain(crs::SPLIT_TONING_SATURATION)
                .chain(crs::HSL.into_iter().flatten())
                .chain(crs::CALIBRATION.into_iter().flatten())
                .any(|s| s.key == key)
    }
    /// The settings among a look's attributes (`attr` gives "" when absent).
    pub(super) fn parse<'a>(attr: impl Fn(&str) -> &'a str) -> Result<Self> {
        // In recipe units: Lightroom's divided by the slider's units, and checked
        // against the look's own range.
        let number = |slider: crs::Slider, range: [f32; 2]| -> Result<Option<f32>> {
            let key = slider.key;
            let text = attr(key);
            if text.is_empty() {
                return Ok(None);
            }
            let v = text
                .trim_start_matches('+')
                .parse::<f32>()
                .with_context(|| format!("Invalid profile setting {key}"))?
                / slider.units;
            ensure!(
                v.is_finite() && (range[0]..=range[1]).contains(&v),
                "Profile setting {key} is out of range"
            );
            Ok(Some(v))
        };
        let slider = |s: crs::Slider| number(s, [-1., 1.]).map(Option::unwrap_or_default);
        let mut s = Self {
            exposure: number(crs::EXPOSURE, [-5., 5.])?.unwrap_or_default(),
            saturation: slider(crs::SATURATION)?,
            whites: slider(crs::WHITES)?,
            vibrance: slider(crs::VIBRANCE)?,
            texture: slider(crs::TEXTURE)?,
            shadow_tint: slider(crs::SHADOW_TINT)?,
            ..Self::default()
        };
        for (i, primary) in crs::CALIBRATION.into_iter().enumerate() {
            for (j, control) in primary.into_iter().enumerate() {
                s.calibration[i][j] = slider(control)?;
            }
        }
        for (i, band) in crs::HSL.into_iter().enumerate() {
            for (j, control) in band.into_iter().enumerate() {
                s.hsl[i][j] = slider(control)?;
            }
        }
        for (i, region) in crs::PARAMETRIC.into_iter().enumerate() {
            s.parametric[i] = slider(region)?;
        }
        for (i, split) in crs::PARAMETRIC_SPLITS.into_iter().enumerate() {
            if let Some(v) = number(split, [0., 1.])? {
                s.splits[i] = v;
            }
        }
        let part = |s: crs::Slider| number(s, [0., 1.]).map(Option::unwrap_or_default);
        let toning = Toning {
            shadows: [
                part(crs::SPLIT_TONING_HUE[0])?,
                part(crs::SPLIT_TONING_SATURATION[0])?,
            ],
            highlights: [
                part(crs::SPLIT_TONING_HUE[1])?,
                part(crs::SPLIT_TONING_SATURATION[1])?,
            ],
            balance: slider(crs::SPLIT_TONING_BALANCE)?,
        };
        if toning.shadows[1] != 0. || toning.highlights[1] != 0. {
            s.toning = Some(toning);
        }
        if let Some(amount) = number(crs::POST_CROP_VIGNETTE_AMOUNT, [-1., 1.])?
            && amount != 0.
        {
            let style = match attr("PostCropVignetteStyle") {
                // 0 is how older Lightroom versions store highlight priority.
                "" | "0" | "1" => 1,
                "2" => 2,
                "3" => 3,
                other => bail!("Unsupported profile vignette style {other}"),
            };
            let or = |v: Option<f32>, default: f32| v.unwrap_or(default);
            s.vignette = Some(Vignette {
                amount,
                midpoint: or(number(crs::POST_CROP_VIGNETTE_MIDPOINT, [0., 1.])?, 0.5),
                feather: or(number(crs::POST_CROP_VIGNETTE_FEATHER, [0., 1.])?, 0.5),
                roundness: or(number(crs::POST_CROP_VIGNETTE_ROUNDNESS, [-1., 1.])?, 0.),
                highlights: or(
                    number(crs::POST_CROP_VIGNETTE_HIGHLIGHT_CONTRAST, [0., 1.])?,
                    0.,
                ),
                style,
            });
        }
        s.validate()?;
        Ok(s)
    }
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// Scales a look slider `value` (normalized to `[-1, 1]`, or `[0, 1]`) by Profile
    /// Amount `amount` (`0..=2`, where `1` is 100%), as Camera Raw 18.7 renders it:
    /// linearly up to 100%, and via `a * v / (1 + (a - 1) * |v|)` above 100%.
    ///
    /// Measured in Camera Raw 18.7 across Contrast (10–100), Exposure (±0.25–±5 EV,
    /// normalized by 5 EV), Parametric regions, Split Toning saturations, Vignette,
    /// Shadows, Highlights, Whites, Blacks, Clarity, Texture, Vibrance, Saturation,
    /// HSL and Calibration at Amounts 100–200% (matching Camera Raw's 1-unit / 0.01-EV
    /// rounded direct sliders within 0.004 out of 255 MAE in gamma-1.8 ProPhoto RGB).
    pub(super) fn scale_slider(value: f32, amount: f32) -> f32 {
        if amount > 1. {
            let v = value.clamp(-1., 1.);
            amount * v / (1. + (amount - 1.) * v.abs())
        } else {
            value * amount
        }
    }
    /// These settings at Profile Amount `amount` (`0..=2`, where `1` is 100%). At 0
    /// the look's split toning and vignette are left out, so they don't replace the
    /// user's either.
    pub(super) fn scaled(&self, amount: f32) -> Self {
        let present = amount != 0.;
        let s = |v: f32| Self::scale_slider(v, amount);
        Self {
            // Exposure's slider range is ±5 EV; normalize by 5 before applying the
            // shared Amount > 100% curve (measured exact at +0.25..+5.0 and -2.0 EV).
            exposure: 5. * s(self.exposure / 5.),
            saturation: s(self.saturation),
            whites: s(self.whites),
            vibrance: s(self.vibrance),
            texture: s(self.texture),
            calibration: self.calibration.map(|p| p.map(s)),
            shadow_tint: s(self.shadow_tint),
            hsl: self.hsl.map(|band| band.map(s)),
            parametric: self.parametric.map(s),
            splits: self.splits,
            toning: self.toning.filter(|_| present).map(|t| Toning {
                shadows: [t.shadows[0], s(t.shadows[1]).clamp(0., 1.)],
                highlights: [t.highlights[0], s(t.highlights[1]).clamp(0., 1.)],
                balance: t.balance,
            }),
            vignette: self.vignette.filter(|_| present).map(|v| Vignette {
                amount: s(v.amount),
                ..v
            }),
        }
    }
    pub(super) fn validate(&self) -> Result<()> {
        let within = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
        ensure!(
            self.toning.is_none_or(|t| {
                [t.shadows, t.highlights]
                    .iter()
                    .flatten()
                    .all(|v| within(*v, 0., 1.))
                    && within(t.balance, -1., 1.)
            }),
            "Invalid profile split toning"
        );
        // Unscaled looks stay within ±1 (±5 EV for Exposure) even at 200% under the
        // odds curve; the wider ±2 / ±10 bounds stay for compatibility with recipes
        // stored by earlier releases.
        ensure!(
            self.vignette.is_none_or(|v| within(v.amount, -2., 2.)
                && [v.midpoint, v.feather, v.highlights]
                    .iter()
                    .all(|x| within(*x, 0., 1.))
                && within(v.roundness, -1., 1.)
                && (1..=3).contains(&v.style)),
            "Invalid profile vignette"
        );
        // As `Effects` requires: increasing, strictly inside 0–1.
        ensure!(
            self.splits[0] > 0.
                && self.splits[2] < 1.
                && self.splits.windows(2).all(|p| p[0] < p[1]),
            "Invalid profile parametric splits"
        );
        let sliders = [
            self.saturation,
            self.whites,
            self.vibrance,
            self.texture,
            self.shadow_tint,
        ]
        .into_iter()
        .chain(self.calibration.iter().flatten().copied())
        .chain(self.hsl.iter().flatten().copied())
        .chain(self.parametric);
        ensure!(
            self.exposure.is_finite()
                && self.exposure.abs() <= 10.
                && sliders.into_iter().all(|v| v.is_finite() && v.abs() <= 2.),
            "Invalid profile settings"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{camera_data::Metadata, camera_profiles::CameraProfile, model::recipe::Recipe};
    use std::{collections::BTreeMap, sync::Arc};

    fn parse(pairs: &[(&str, &str)]) -> Result<LookSettings> {
        let map: BTreeMap<&str, &str> = pairs.iter().copied().collect();
        LookSettings::parse(|key| map.get(key).copied().unwrap_or(""))
    }
    /// Settings like Vintage 07's and Modern 03's, made up.
    fn vintage() -> LookSettings {
        parse(&[
            ("Exposure2012", "+0.20"),
            ("Saturation", "-20"),
            ("SaturationAdjustmentBlue", "-30"),
            ("ParametricLights", "-20"),
            ("ParametricHighlightSplit", "80"),
            ("SplitToningShadowHue", "180"),
            ("SplitToningShadowSaturation", "10"),
            ("SplitToningHighlightHue", "54"),
            ("SplitToningHighlightSaturation", "20"),
            ("SplitToningBalance", "+40"),
            ("PostCropVignetteAmount", "-10"),
            ("PostCropVignetteStyle", "1"),
        ])
        .unwrap()
    }

    #[test]
    fn look_settings_parse_in_recipe_units() -> Result<()> {
        let s = vintage();
        assert_eq!((s.exposure, s.saturation), (0.2, -0.2));
        assert_eq!(s.hsl[5], [0., -0.3, 0.]);
        assert_eq!(s.parametric, [0., 0., -0.2, 0.]);
        assert_eq!(s.splits, [0.25, 0.5, 0.8]);
        let t = s.toning.unwrap();
        assert_eq!(
            (t.shadows, t.highlights, t.balance),
            ([0.5, 0.1], [0.15, 0.2], 0.4)
        );
        assert_eq!(s.vignette.unwrap().amount, -0.1);
        assert!(LookSettings::reads("LuminanceAdjustmentOrange"));
        assert!(LookSettings::reads("ParametricMidtoneSplit"));
        assert!(!LookSettings::reads("Dehaze"));
        // Nothing set is the default; out-of-range and unknown values are refused.
        assert!(parse(&[])?.is_default());
        assert!(parse(&[("Saturation", "-120")]).is_err());
        assert!(parse(&[("SplitToningShadowHue", "400")]).is_err());
        assert!(
            parse(&[
                ("PostCropVignetteAmount", "-10"),
                ("PostCropVignetteStyle", "7")
            ])
            .is_err()
        );
        let style0 = parse(&[
            ("PostCropVignetteAmount", "-10"),
            ("PostCropVignetteStyle", "0"),
        ])?;
        assert_eq!(style0.vignette.unwrap().style, 1);
        // Splits must stay strictly inside 0–1 and increasing.
        assert!(parse(&[("ParametricShadowSplit", "0")]).is_err());
        assert!(parse(&[("ParametricMidtoneSplit", "75")]).is_err());
        Ok(())
    }
    #[test]
    fn look_settings_render_with_the_users_as_camera_raw_does() {
        let m = Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        };
        let mut profile = CameraProfile::creative_for_test(&m);
        profile.enhanced.as_mut().unwrap().settings = vintage();
        let mut user = Recipe {
            profile: Some(Arc::new(profile)),
            saturation: 0.1,
            ..Default::default()
        };
        user.grading[0] = [0.08, 0.5, 0.];
        user.effects.vignette = -0.3;
        let r = user.with_profile_adjustments();
        // Added to the user's sliders, which keep their values.
        assert!((r.saturation - -0.1).abs() < 1e-6);
        assert!((r.exposure - 0.2).abs() < 1e-6);
        assert_eq!(r.hsl[5][1], -0.3);
        // The look's parametric curve and split toning render as passes of their
        // own, leaving the user's curve and grading as they are.
        assert_eq!(r.effects.parametric, user.effects.parametric);
        assert_eq!(r.effects.splits, user.effects.splits);
        assert_eq!(r.grading, user.grading);
        // The look's vignette replaces the user's.
        assert_eq!(r.effects.vignette, -0.1);
        assert_eq!(user.saturation, 0.1);
        // At 50% the settings are half as strong, toning hues unchanged.
        user.profile_amount = 0.5;
        let r = user.with_profile_adjustments();
        assert!((r.saturation - 0.).abs() < 1e-6);
        assert_eq!(r.effects.vignette, -0.05);
        // Strong toning at 200% follows Camera Raw's odds curve and stays valid.
        let mut toned = vintage();
        toned.toning.as_mut().unwrap().highlights[1] = 0.9;
        let scaled = toned.scaled(2.);
        assert!((scaled.toning.unwrap().highlights[1] - 1.8 / 1.9).abs() < 1e-6);
        assert_eq!(scaled.toning.unwrap().balance, 0.4);
        assert!(scaled.validate().is_ok());
        // At 0% the look leaves the user's vignette alone.
        user.profile_amount = 0.;
        let r = user.with_profile_adjustments();
        assert_eq!(r.effects.vignette, -0.3);
        user.profile_amount = 0.5;
        // A full-strength vignette at 200% stays within the slider's range.
        let mut strong = user.clone();
        let mut profile = (**strong.profile.as_ref().unwrap()).clone();
        let look = profile.enhanced.as_mut().unwrap();
        look.settings.vignette.as_mut().unwrap().amount = -1.;
        strong.profile = Some(Arc::new(profile));
        strong.profile_amount = 2.;
        assert_eq!(strong.with_profile_adjustments().effects.vignette, -1.);
        // Stored settings outside what the parser accepts are refused.
        let mut bad = vintage();
        bad.vignette.as_mut().unwrap().midpoint = 100.;
        assert!(bad.validate().is_err());
        let mut bad = vintage();
        bad.toning.as_mut().unwrap().balance = 3.;
        assert!(bad.validate().is_err());
    }

    /// Whites, Vibrance, Texture and calibration in a look render as the user's
    /// sliders at those values and add to them (measured with Camera Raw 18.7 on
    /// synthetic looks, ΔE00 0.00 at 50% and 100%), and are left out of a recipe
    /// when zero, so recipes with looks that have none read in earlier releases.
    #[test]
    fn look_whites_vibrance_texture_and_calibration_add_to_the_users() -> Result<()> {
        let s = parse(&[
            ("Whites2012", "-10"),
            ("Vibrance", "+40"),
            ("Texture", "+60"),
            ("GreenHue", "-40"),
            ("BlueSaturation", "+20"),
            ("ShadowTint", "+40"),
        ])?;
        let m = Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        };
        let mut profile = CameraProfile::creative_for_test(&m);
        profile.enhanced.as_mut().unwrap().settings = s.clone();
        let mut user = Recipe {
            profile: Some(Arc::new(profile)),
            whites: 0.3,
            vibrance: -0.2,
            ..Default::default()
        };
        user.effects.calibration[1][0] = 0.1;
        let r = user.with_profile_adjustments();
        assert!((r.whites - 0.2).abs() < 1e-6);
        assert!((r.vibrance - 0.2).abs() < 1e-6);
        assert!((r.effects.texture - 0.6).abs() < 1e-6);
        assert!((r.effects.calibration[1][0] - -0.3).abs() < 1e-6);
        assert!((r.effects.calibration[2][1] - 0.2).abs() < 1e-6);
        assert!((r.effects.shadow_tint - 0.4).abs() < 1e-6);
        // At 50% half as strong; above 100% Camera Raw's odds curve scales
        // `a * v / (1 + (a - 1) * |v|)` (+60 Texture at 200% is +75, not +90).
        user.profile_amount = 0.5;
        assert!((user.with_profile_adjustments().effects.texture - 0.3).abs() < 1e-6);
        user.profile_amount = 2.;
        assert!((user.with_profile_adjustments().effects.texture - 0.75).abs() < 1e-6);
        // Saved and read back; a look without them saves as before.
        let copy: LookSettings = serde_json::from_value(serde_json::to_value(&s)?)?;
        assert_eq!(copy, s);
        let old = serde_json::to_value(vintage())?;
        for key in [
            "whites",
            "vibrance",
            "texture",
            "calibration",
            "shadow_tint",
        ] {
            assert!(old.get(key).is_none(), "{key}");
        }
        Ok(())
    }

    /// Above 100% Profile Amount, Camera Raw 18.7 scales every look slider via
    /// `a * v / (1 + (a - 1) * |v|)` over its full range (`[-1, 1]`, or `[-5, 5]` EV
    /// for Exposure), while keeping parametric splits, split-toning hues/balance and
    /// vignette geometry fixed.
    #[test]
    fn amount_above_100_scales_sliders_by_camera_raw_odds_curve() -> Result<()> {
        // Continuity at 100%, linearity below, exact values above, symmetry and bounds.
        for v in [-1., -0.6, -0.3, -0.1, 0., 0.1, 0.3, 0.6, 1.] {
            assert!((LookSettings::scale_slider(v, 0.)).abs() < 1e-6);
            assert!((LookSettings::scale_slider(v, 0.5) - 0.5 * v).abs() < 1e-6);
            assert!((LookSettings::scale_slider(v, 1.) - v).abs() < 1e-6);
            let at_150 = LookSettings::scale_slider(v, 1.5);
            let at_200 = LookSettings::scale_slider(v, 2.);
            assert!((at_150 + LookSettings::scale_slider(-v, 1.5)).abs() < 1e-6);
            assert!((at_200 + LookSettings::scale_slider(-v, 2.)).abs() < 1e-6);
            assert!(at_200.abs() <= 1. + 1e-6);
            if v > 0. {
                assert!(v <= at_150 && at_150 <= at_200);
            }
        }
        // Measured Camera Raw 18.7 checkpoints (Exposure normalized by 5 EV,
        // parametric combo D-15/L-20/H-33, split toning sh=40/hi=30/bal=+20, vignette -30).
        let s = parse(&[
            ("Exposure2012", "+0.50"),
            ("ParametricDarks", "-15"),
            ("ParametricLights", "-20"),
            ("ParametricHighlights", "-33"),
            ("ParametricShadowSplit", "20"),
            ("ParametricMidtoneSplit", "45"),
            ("ParametricHighlightSplit", "80"),
            ("SplitToningShadowHue", "210"),
            ("SplitToningShadowSaturation", "40"),
            ("SplitToningHighlightHue", "50"),
            ("SplitToningHighlightSaturation", "30"),
            ("SplitToningBalance", "+20"),
            ("PostCropVignetteAmount", "-30"),
            ("PostCropVignetteMidpoint", "40"),
        ])?;
        let at_150 = s.scaled(1.5);
        assert!((at_150.exposure - 0.75 / 1.05).abs() < 1e-5);
        assert!((at_150.parametric[1] - (-0.225 / 1.075)).abs() < 1e-5);
        assert!((at_150.parametric[2] - (-0.30 / 1.10)).abs() < 1e-5);
        assert!((at_150.parametric[3] - (-0.495 / 1.165)).abs() < 1e-5);
        let at_200 = s.scaled(2.);
        assert!((at_200.exposure - 1. / 1.1).abs() < 1e-5);
        assert!((at_200.parametric[1] - (-0.30 / 1.15)).abs() < 1e-5);
        assert!((at_200.parametric[2] - (-0.40 / 1.20)).abs() < 1e-5);
        assert!((at_200.parametric[3] - (-0.66 / 1.33)).abs() < 1e-5);
        assert_eq!(at_200.splits, [0.20, 0.45, 0.80]);
        let t = at_200.toning.unwrap();
        assert!((t.shadows[1] - 0.80 / 1.40).abs() < 1e-5);
        assert!((t.highlights[1] - 0.60 / 1.30).abs() < 1e-5);
        assert_eq!(
            (t.shadows[0], t.highlights[0], t.balance),
            (210. / 360., 50. / 360., 0.2)
        );
        let v = at_200.vignette.unwrap();
        assert!((v.amount - (-0.60 / 1.30)).abs() < 1e-5);
        assert_eq!(v.midpoint, 0.40);
        // Exposure +2.0 EV at 150% -> +2.50 EV exactly; +5.0 EV at 200% -> +5.00 EV.
        let exp2 = parse(&[("Exposure2012", "+2.00")])?;
        assert!((exp2.scaled(1.5).exposure - 2.5).abs() < 1e-5);
        assert!((exp2.scaled(2.).exposure - 4. / 1.4).abs() < 1e-5);
        let exp5 = parse(&[("Exposure2012", "+5.00")])?;
        assert!((exp5.scaled(2.).exposure - 5.).abs() < 1e-5);
        Ok(())
    }

    /// What a look's settings read, pinned in a golden file: each key alone at values
    /// across and beyond its range, the vignette's dependent settings with and without
    /// an amount, and which keys a look accepts at all. Compared with
    /// `look_settings_golden.txt`, written by this test with `RAWMAKASE_BLESS=1`;
    /// floats in full, so a one-bit change fails.
    #[test]
    fn every_look_setting_reads_as_before() {
        use std::fmt::Write as _;
        const VALUES: &[&str] = &[
            "", "0", "-0", "+0.5", "-0.5", "1", "2.4", "4.99", "5", "5.01", "-5", "-5.01", "5.5",
            "-12.5", "33.3", "50", "99.99", "-100", "100", "100.01", "101", "180", "359", "360",
            "361", "1e-7", "-1e-7", "inf", "x", "NaN",
        ];
        let mut keys: Vec<String> = [
            "Exposure2012",
            "Saturation",
            "ParametricShadows",
            "ParametricDarks",
            "ParametricLights",
            "ParametricHighlights",
            "ParametricShadowSplit",
            "ParametricMidtoneSplit",
            "ParametricHighlightSplit",
            "SplitToningShadowHue",
            "SplitToningShadowSaturation",
            "SplitToningHighlightHue",
            "SplitToningHighlightSaturation",
            "SplitToningBalance",
            "PostCropVignetteAmount",
            "PostCropVignetteMidpoint",
            "PostCropVignetteFeather",
            "PostCropVignetteRoundness",
            "PostCropVignetteStyle",
            "PostCropVignetteHighlightContrast",
            "Dehaze",
            "Contrast2012",
            "Exposure",
            "HueAdjustment",
            "HueAdjustmentTeal",
        ]
        .map(String::from)
        .to_vec();
        for band in BANDS {
            for control in ["Hue", "Saturation", "Luminance"] {
                keys.push(format!("{control}Adjustment{band}"));
            }
        }
        keys.extend(
            [
                "Whites2012",
                "Vibrance",
                "Texture",
                "ShadowTint",
                "RedHue",
                "RedSaturation",
                "GreenHue",
                "GreenSaturation",
                "BlueHue",
                "BlueSaturation",
            ]
            .map(String::from),
        );
        // The settings' fields that differ from a look without any, floats in full.
        // Fields a look without them leaves out (zero) compare as zero.
        fn changed(result: &Result<LookSettings>) -> String {
            fn walk(prefix: &str, v: &serde_json::Value, d: &serde_json::Value, out: &mut String) {
                match (v, d) {
                    (serde_json::Value::Object(m), serde_json::Value::Object(dm)) => {
                        for (k, v) in m {
                            let d = dm.get(k).unwrap_or(&serde_json::Value::Null);
                            walk(&format!("{prefix}.{k}"), v, d, out);
                        }
                    }
                    (
                        serde_json::Value::Array(a),
                        serde_json::Value::Array(_) | serde_json::Value::Null,
                    ) => {
                        for (i, v) in a.iter().enumerate() {
                            let d = d.get(i).unwrap_or(&serde_json::Value::Null);
                            walk(&format!("{prefix}[{i}]"), v, d, out);
                        }
                    }
                    _ => {
                        // Compared as written: JSON numbers hold -0 equal to 0.
                        let text = |v: &serde_json::Value| match v.as_f64() {
                            Some(f) if v.is_f64() => format!("{:?}", f as f32),
                            _ => v.to_string(),
                        };
                        let default = match d {
                            serde_json::Value::Null if v.is_f64() => "0.0".into(),
                            d => text(d),
                        };
                        if text(v) != default {
                            let _ = write!(out, " {prefix}={}", text(v));
                        }
                    }
                }
            }
            match result {
                Ok(s) => {
                    let mut out = String::from("ok");
                    let base = serde_json::to_value(LookSettings::default()).unwrap();
                    walk("", &serde_json::to_value(s).unwrap(), &base, &mut out);
                    out
                }
                Err(e) => format!("error {e:#}"),
            }
        }
        let mut out = String::new();
        for key in &keys {
            let _ = writeln!(out, "# {key} read {}", LookSettings::reads(key));
            // Alone, and with the setting that makes it count: a vignette's amount, the
            // toning's saturations.
            let context: &[(&str, &str)] = match key.as_str() {
                k if k.starts_with("PostCropVignette") && k != "PostCropVignetteAmount" => {
                    &[("PostCropVignetteAmount", "-20")]
                }
                k if k.starts_with("SplitToning") && !k.ends_with("Saturation") => &[
                    ("SplitToningShadowSaturation", "20"),
                    ("SplitToningHighlightSaturation", "30"),
                ],
                _ => &[],
            };
            for (name, with) in [("alone", &[][..]), ("with", context)] {
                if name == "with" && with.is_empty() {
                    continue;
                }
                for value in VALUES {
                    let result = LookSettings::parse(|k| {
                        if k == key {
                            value
                        } else {
                            with.iter().find(|(c, _)| *c == k).map_or("", |(_, v)| *v)
                        }
                    });
                    let _ = writeln!(out, "{key}={value:?} {name} {}", changed(&result));
                }
            }
        }
        // The vignette's other settings, with an amount and without one.
        for amount in ["", "0", "-20"] {
            for (key, value) in [
                ("PostCropVignetteStyle", "x"),
                ("PostCropVignetteStyle", "3"),
                ("PostCropVignetteMidpoint", "150"),
                ("PostCropVignetteFeather", "-1"),
            ] {
                let result = LookSettings::parse(|k| match k {
                    "PostCropVignetteAmount" => amount,
                    k if k == key => value,
                    _ => "",
                });
                let _ = writeln!(
                    out,
                    "amount {amount:?} {key}={value:?} {}",
                    changed(&result)
                );
            }
        }
        if std::env::var_os("RAWMAKASE_BLESS").is_some() {
            std::fs::write(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/camera_profiles/look_settings_golden.txt"),
                &out,
            )
            .unwrap();
            return;
        }
        let golden = include_str!("look_settings_golden.txt");
        for (i, (want, got)) in golden.lines().zip(out.lines()).enumerate() {
            assert_eq!(got, want, "line {}", i + 1);
        }
        assert_eq!(out.lines().count(), golden.lines().count(), "line count");
    }
}
