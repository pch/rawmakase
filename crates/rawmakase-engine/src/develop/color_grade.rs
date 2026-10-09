//! Color grading: per-channel curves measured in Camera Raw 18.7 on the synthetic
//! chart, for every hue, saturation, Luminance, Blending and Balance (see
//! `color_grade_curves`).
use super::color_grade_curves::ChannelCurves;
use crate::color::mul;
use crate::model::recipe::Recipe;

/// Grading, applied to linear display RGB after the color mixer.
pub(crate) struct ColorGrade(pub(crate) ChannelCurves);
impl ColorGrade {
    /// `None` when grading (the user's, and a look's split toning) is inactive.
    pub(crate) fn new(r: &Recipe) -> Option<Self> {
        ChannelCurves::new(r).map(Self)
    }
    /// `rgb` is linear display RGB (sRGB primaries).
    pub(crate) fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let p = mul(crate::camera_profiles::RGB_TO_PRO, rgb);
        mul(crate::camera_profiles::PRO_TO_RGB, self.0.apply(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_grading_has_no_curves() {
        assert!(ColorGrade::new(&Recipe::default()).is_none());
        let mut r = Recipe::default();
        r.grading[0] = [240. / 360., 0.5, 0.];
        assert!(ColorGrade::new(&r).is_some());
        // The measured curves cover every Blending and Balance.
        r.effects.balance = 0.3;
        assert!(ColorGrade::new(&r).is_some());
    }
    #[test]
    fn blue_shadow_tint_cools_shadows_more_than_highlights() {
        let mut r = Recipe::default();
        r.effects.blending = 0.5;
        r.grading[0] = [240. / 360., 0.5, 0.];
        let g = ColorGrade::new(&r).unwrap();
        let cool = |p: [f32; 3]| p[2] / p[0].max(1e-6);
        let dark = g.apply([0.02; 3]);
        let bright = g.apply([0.8; 3]);
        assert!(cool(dark) > 1.05, "{dark:?}");
        assert!(cool(dark) > cool(bright));
    }
}
