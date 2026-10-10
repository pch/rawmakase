//! Develop settings some of Lightroom's looks carry beyond their tables and the tone
//! adjustments in `Enhanced` (Vintage 07, Modern 03 and 04): Exposure, Saturation,
//! the colour mixer, the parametric curve, split toning and a post-crop vignette.
//! They render with the user's settings without moving the user's sliders (see
//! `Recipe::with_profile_adjustments`).
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

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
    /// Hue, saturation and luminance per colour band, as `Recipe::hsl`.
    pub hsl: [[f32; 3]; 8],
    pub parametric: [f32; 4],
    pub splits: [f32; 3],
    pub toning: Option<Toning>,
    pub vignette: Option<Vignette>,
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
        }
    }
}
impl LookSettings {
    /// Whether `key` is one of the settings this reads.
    pub(super) fn reads(key: &str) -> bool {
        const KEYS: &[&str] = &[
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
        ];
        KEYS.contains(&key)
            || ["Hue", "Saturation", "Luminance"].iter().any(|control| {
                key.strip_prefix(control)
                    .and_then(|k| k.strip_prefix("Adjustment"))
                    .is_some_and(|band| BANDS.contains(&band))
            })
    }
    /// The settings among a look's attributes (`attr` gives "" when absent).
    pub(super) fn parse<'a>(attr: impl Fn(&str) -> &'a str) -> Result<Self> {
        // Lightroom's units per recipe unit: 100 for sliders, 360 for hues.
        let number = |key: &str, units: f32, range: [f32; 2]| -> Result<Option<f32>> {
            let text = attr(key);
            if text.is_empty() {
                return Ok(None);
            }
            let v = text
                .trim_start_matches('+')
                .parse::<f32>()
                .with_context(|| format!("Invalid profile setting {key}"))?
                / units;
            ensure!(
                v.is_finite() && (range[0]..=range[1]).contains(&v),
                "Profile setting {key} is out of range"
            );
            Ok(Some(v))
        };
        let slider = |key: &str| number(key, 100., [-1., 1.]).map(Option::unwrap_or_default);
        let mut s = Self {
            exposure: number("Exposure2012", 1., [-5., 5.])?.unwrap_or_default(),
            saturation: slider("Saturation")?,
            ..Self::default()
        };
        for (i, band) in BANDS.iter().enumerate() {
            for (j, control) in ["Hue", "Saturation", "Luminance"].iter().enumerate() {
                s.hsl[i][j] = slider(&format!("{control}Adjustment{band}"))?;
            }
        }
        for (i, name) in ["Shadows", "Darks", "Lights", "Highlights"]
            .iter()
            .enumerate()
        {
            s.parametric[i] = slider(&format!("Parametric{name}"))?;
        }
        for (i, name) in ["Shadow", "Midtone", "Highlight"].iter().enumerate() {
            if let Some(v) = number(&format!("Parametric{name}Split"), 100., [0., 1.])? {
                s.splits[i] = v;
            }
        }
        let hue = |key: &str| number(key, 360., [0., 1.]);
        let saturation = |key: &str| number(key, 100., [0., 1.]);
        let toning = Toning {
            shadows: [
                hue("SplitToningShadowHue")?.unwrap_or_default(),
                saturation("SplitToningShadowSaturation")?.unwrap_or_default(),
            ],
            highlights: [
                hue("SplitToningHighlightHue")?.unwrap_or_default(),
                saturation("SplitToningHighlightSaturation")?.unwrap_or_default(),
            ],
            balance: slider("SplitToningBalance")?,
        };
        if toning.shadows[1] != 0. || toning.highlights[1] != 0. {
            s.toning = Some(toning);
        }
        if let Some(amount) = number("PostCropVignetteAmount", 100., [-1., 1.])?
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
                midpoint: or(number("PostCropVignetteMidpoint", 100., [0., 1.])?, 0.5),
                feather: or(number("PostCropVignetteFeather", 100., [0., 1.])?, 0.5),
                roundness: or(number("PostCropVignetteRoundness", 100., [-1., 1.])?, 0.),
                highlights: or(
                    number("PostCropVignetteHighlightContrast", 100., [0., 1.])?,
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
    /// These settings at `strength` of their effect. At 0 the look's split toning
    /// and vignette are left out, so they don't replace the user's either.
    pub(super) fn scaled(&self, strength: f32) -> Self {
        let present = strength != 0.;
        Self {
            exposure: self.exposure * strength,
            saturation: self.saturation * strength,
            hsl: self.hsl.map(|band| band.map(|v| v * strength)),
            parametric: self.parametric.map(|v| v * strength),
            splits: self.splits,
            toning: self.toning.filter(|_| present).map(|t| Toning {
                shadows: [t.shadows[0], (t.shadows[1] * strength).min(1.)],
                highlights: [t.highlights[0], (t.highlights[1] * strength).min(1.)],
                balance: t.balance,
            }),
            vignette: self.vignette.filter(|_| present).map(|v| Vignette {
                amount: v.amount * strength,
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
        // The amount may reach ±2 at a Profile Amount of 200%; it renders clamped.
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
        let sliders = [self.saturation]
            .into_iter()
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
        // Strong toning at 200% stays a valid saturation.
        let mut toned = vintage();
        toned.toning.as_mut().unwrap().highlights[1] = 0.9;
        let scaled = toned.scaled(1.5);
        assert_eq!(scaled.toning.unwrap().highlights[1], 1.);
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
}
