//! Lightroom's per-panel switches: a panel switched off keeps its settings but renders
//! as if they were at their defaults.
//!
//! Lightroom stores one `Enable*` flag per panel in each photo's develop settings. The
//! settings each switch covers follow the panel layout Lightroom Classic 15 names in
//! its own history (the Basic panel and crop have no switch).
use super::recipe::Recipe;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A Develop panel with a switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    ToneCurve,
    ColorMixer,
    BlackWhiteMix,
    ColorGrading,
    Detail,
    LensCorrections,
    Transform,
    Effects,
    Calibration,
    SpotRemoval,
    RedEye,
    Masks,
}

/// Whether a panel's settings are applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelState {
    On,
    Off,
}

impl Panel {
    pub const ALL: [Panel; 12] = [
        Panel::ToneCurve,
        Panel::ColorMixer,
        Panel::BlackWhiteMix,
        Panel::ColorGrading,
        Panel::Detail,
        Panel::LensCorrections,
        Panel::Transform,
        Panel::Effects,
        Panel::Calibration,
        Panel::SpotRemoval,
        Panel::RedEye,
        Panel::Masks,
    ];
    /// Lightroom's develop-settings keys for this switch; the first is the one written.
    /// Masks have one switch since Lightroom 11 and three for the older local tools,
    /// which RAWmakase converts into the same masks.
    pub fn lightroom_keys(self) -> &'static [&'static str] {
        match self {
            Panel::ToneCurve => &["EnableToneCurve"],
            Panel::ColorMixer => &["EnableColorAdjustments"],
            Panel::BlackWhiteMix => &["EnableGrayscaleMix"],
            Panel::ColorGrading => &["EnableSplitToning"],
            Panel::Detail => &["EnableDetail"],
            Panel::LensCorrections => &["EnableLensCorrections"],
            Panel::Transform => &["EnableTransform"],
            Panel::Effects => &["EnableEffects"],
            Panel::Calibration => &["EnableCalibration"],
            Panel::SpotRemoval => &["EnableRetouch"],
            Panel::RedEye => &["EnableRedEye"],
            Panel::Masks => &[
                "EnableMaskGroupBasedCorrections",
                "EnablePaintBasedCorrections",
                "EnableGradientBasedCorrections",
                "EnableCircularGradientBasedCorrections",
            ],
        }
    }
    /// Whether `after` differs from `before` only in this panel's settings, as an
    /// edit in the panel leaves it.
    pub fn holds_change(self, before: &Recipe, after: &Recipe) -> bool {
        if before == after {
            return false;
        }
        let defaults = Recipe::default();
        let (mut a, mut b) = (before.clone(), after.clone());
        self.bypass(&mut a, &defaults);
        self.bypass(&mut b, &defaults);
        // The camera's built-in correction stays through the bypass, but Enable
        // Profile Corrections sets it with the profile, so it is the panel's too.
        if self == Panel::LensCorrections {
            a.lens_builtin = b.lens_builtin;
        }
        a == b
    }
    /// Sets this panel's settings in `r` to the values of `defaults`.
    fn bypass(self, r: &mut Recipe, defaults: &Recipe) {
        let (e, d) = (&mut r.effects, &defaults.effects);
        match self {
            Panel::ToneCurve => {
                r.curve = defaults.curve.clone();
                r.curve_saturation = defaults.curve_saturation;
                e.channels = d.channels.clone();
                e.parametric = d.parametric;
                e.splits = d.splits;
            }
            Panel::ColorMixer => {
                r.hsl = defaults.hsl;
                r.point_colors = defaults.point_colors.clone();
            }
            Panel::BlackWhiteMix => e.gray_mix = d.gray_mix,
            Panel::ColorGrading => {
                r.grading = defaults.grading;
                e.balance = d.balance;
                e.blending = d.blending;
                e.global_grade = d.global_grade;
            }
            Panel::Detail => {
                r.sharpening = 0.;
                r.sharpening_radius = defaults.sharpening_radius;
                r.sharpening_detail = defaults.sharpening_detail;
                r.sharpening_masking = defaults.sharpening_masking;
                r.noise_luma = 0.;
                r.noise_chroma = 0.;
                e.luma_detail = d.luma_detail;
                e.luma_contrast = d.luma_contrast;
                e.chroma_detail = d.chroma_detail;
                e.chroma_smoothness = d.chroma_smoothness;
            }
            // The correction a camera stores in its RAW stays: Lightroom applies it
            // outside the panel's controls (inferred; not yet measured).
            Panel::LensCorrections => {
                r.lens_profile = false;
                r.lens_profile_choice = defaults.lens_profile_choice.clone();
                r.lens_ca = false;
                r.lens_distortion = defaults.lens_distortion;
                r.lens_vignetting = defaults.lens_vignetting;
                r.lens_manual_distortion = defaults.lens_manual_distortion;
                e.defringe = [0.; 2];
                e.defringe_ranges = d.defringe_ranges;
                e.lens_vignette = 0.;
                e.lens_vignette_midpoint = d.lens_vignette_midpoint;
            }
            Panel::Transform => {
                r.transform = defaults.transform;
                r.upright = defaults.upright.clone();
                r.constrain_crop = defaults.constrain_crop;
            }
            Panel::Effects => e.reset_post_crop(),
            Panel::Calibration => {
                e.calibration = d.calibration;
                e.shadow_tint = d.shadow_tint;
            }
            Panel::SpotRemoval => r.retouch.clear(),
            Panel::RedEye => r.red_eye.clear(),
            Panel::Masks => r.masks.clear(),
        }
    }
}

/// The panels switched off in a recipe. Every panel is on by default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelSwitches {
    off: BTreeSet<Panel>,
}

impl PanelSwitches {
    pub fn state(&self, panel: Panel) -> PanelState {
        if self.off.contains(&panel) {
            PanelState::Off
        } else {
            PanelState::On
        }
    }
    pub fn set(&mut self, panel: Panel, state: PanelState) {
        match state {
            PanelState::On => self.off.remove(&panel),
            PanelState::Off => self.off.insert(panel),
        };
    }
    pub fn all_on(&self) -> bool {
        self.off.is_empty()
    }
    pub fn switched_off(&self) -> impl Iterator<Item = Panel> + '_ {
        self.off.iter().copied()
    }
}

impl Recipe {
    /// The recipe with every switched-off panel at its defaults, as preview, export and
    /// thumbnails render it. The stored settings are unchanged.
    pub fn as_rendered(&self) -> std::borrow::Cow<'_, Recipe> {
        if self.panels.all_on() {
            return std::borrow::Cow::Borrowed(self);
        }
        let defaults = Recipe::default();
        let mut r = self.clone();
        for panel in self.panels.switched_off() {
            panel.bypass(&mut r, &defaults);
        }
        // The switches stay, so `Recipe::resolved`, which knows the camera, can also
        // turn off lens data that is only on because of the panel.
        std::borrow::Cow::Owned(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_inside_one_panel_is_held_by_that_panel_alone() {
        let before = Recipe::default();
        let mut after = before.clone();
        after.effects.gray_mix[0] = 0.3;
        assert!(Panel::BlackWhiteMix.holds_change(&before, &after));
        assert!(!Panel::Detail.holds_change(&before, &after));
        after.exposure = 0.5;
        assert!(!Panel::BlackWhiteMix.holds_change(&before, &after));
        // Enable Profile Corrections sets the camera's built-in correction with it.
        let mut after = before.clone();
        after.lens_profile = !before.lens_profile;
        after.lens_builtin = !before.lens_builtin;
        assert!(Panel::LensCorrections.holds_change(&before, &after));
        let mut after = before.clone();
        after.effects.lens_vignette = -0.4;
        assert!(Panel::LensCorrections.holds_change(&before, &after));
        let mut after = before.clone();
        after.effects.grain = 0.4;
        assert!(Panel::Effects.holds_change(&before, &after));
        let mut after = before.clone();
        after.add_retouch(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Clone,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.3, 0.3],
                radius: 0.05,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.2, 0.],
        });
        assert!(Panel::SpotRemoval.holds_change(&before, &after));
        // Detail's reset sets the default sharpening.
        let mut before = before;
        before.sharpening = 0.35;
        let mut after = before.clone();
        after.set_sharpening_defaults();
        assert!(Panel::Detail.holds_change(&before, &after));
    }
}
