//! The scene tone stage (S3): the Basic panel's tone controls on scene-linear ProPhoto
//! RGB, after Exposure and before the camera profile's look and tone curve, where
//! Camera Raw applies them (docs/scene-tone-stage.md).
mod global;
mod haze;
mod measures;
pub(crate) use global::{
    GlobalTone, MASK_VALUES, SLIDER_STEP, SLIDERS, T_FIRST, T_SAMPLES, T_STEP, U_FIRST, U_SAMPLES,
    U_STEP, Y_FIRST, Y_SAMPLES, Y_STEP, gpu_mask_table,
};
pub(crate) use haze::{Dehazing, Haze};
pub(crate) use measures::{MAX_EDGE, Measured, PhotoMeasures};

use crate::develop::masks::{
    LocalDelta,
    local::{SCENE_SLOTS, slot, uses},
};
use crate::model::recipe::Recipe;

/// Luminance of linear ProPhoto RGB.
pub(crate) fn luminance(p: [f32; 3]) -> f32 {
    0.2880402 * p[0] + 0.7118741 * p[1] + 0.0000857 * p[2]
}

/// A render's scene tone stage, prepared from its recipe and the photo's measures.
pub(crate) struct SceneTone {
    pub(crate) global: GlobalTone,
    /// Dehaze and the level negative Dehaze's response is relative to (log2).
    pub(crate) dehaze: f32,
    pub(crate) dehaze_key: f32,
    /// Positive Dehaze's haze model, when the recipe or a mask has positive Dehaze
    /// ([`needs_haze`]); without it positive Dehaze leaves the pixel alone.
    pub(crate) haze: Option<Dehazing>,
}
/// Whether a render of `r` removes haze somewhere: positive Dehaze, globally or in an
/// active mask (a mask's Dehaze adds to the recipe's).
pub(crate) fn needs_haze(r: &Recipe) -> bool {
    r.effects.dehaze > 0.
        || r.masks
            .iter()
            .any(|m| m.is_active() && m.adjust.dehaze > 0.)
}
impl SceneTone {
    pub(crate) fn new(r: &Recipe, measures: &PhotoMeasures, haze: Option<Dehazing>) -> Self {
        Self {
            global: GlobalTone::new(
                [
                    measures.sensor_white + r.exposure,
                    measures.max + r.exposure,
                    measures.whites_top(r.exposure),
                    measures.white_point(r.exposure),
                ],
                r.whites,
                r.blacks,
                measures.black_key(r.exposure),
                measures.dark_key(r.exposure),
                r.profile
                    .as_ref()
                    .is_none_or(|p| p.black_render() != crate::camera_profiles::BlackRender::None),
            ),
            dehaze: r.effects.dehaze,
            dehaze_key: measures.p99 + r.exposure,
            haze,
        }
    }
    /// S3's Dehaze and global curves on one pixel of linear ProPhoto RGB at camera-image
    /// sample position `pos`, with a mask's Dehaze added to the recipe's and its Whites
    /// and Blacks as their own curves after the global ones, as Camera Raw renders them.
    pub(crate) fn apply(
        &self,
        pro: [f32; 3],
        pos: [f32; 2],
        local: Option<&LocalDelta>,
    ) -> [f32; 3] {
        let dehaze = self.dehaze + local.map_or(0., |d| d[slot::DEHAZE]);
        let pro = if dehaze > 0. {
            // The photo's haze, removed in proportion to the amount.
            self.haze
                .as_ref()
                .map_or(pro, |haze| haze.apply(pro, dehaze, pos))
        } else if dehaze < 0. {
            // Each channel by its level relative to the photo's bright end.
            pro.map(|v| {
                if v > 0. {
                    let level = crate::develop::local_tone::level(v);
                    v * crate::develop::local_tone::family(
                        &crate::develop::local_tone_data::DEHAZE,
                        dehaze,
                        self.dehaze_key,
                        level,
                    )
                    .exp2()
                } else {
                    v
                }
            })
        } else {
            pro
        };
        match local.filter(|d| uses(d, &SCENE_SLOTS)) {
            Some(d) => self.global.apply_at(pro, d[slot::WHITES], d[slot::BLACKS]),
            None => self.global.apply(pro),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mask's Dehaze adds to the recipe's: positive sums remove the photo's haze as
    /// the global slider at the sum does, negative ones use the measured tables, and a
    /// mask cancelling the slider leaves the pixel alone.
    #[test]
    fn mask_dehaze_sums_with_the_global_slider() {
        let (w, h) = (40usize, 30usize);
        let scene: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = 0.05 + 0.3 * (i % w) as f32 / w as f32;
                [v + 0.3, v + 0.32, v + 0.38]
            })
            .collect();
        let haze = std::sync::Arc::new(Haze::of(&scene, w, h));
        let measures = PhotoMeasures::default();
        let tone = |dehaze: f32| {
            let mut r = Recipe::default();
            r.effects.dehaze = dehaze;
            SceneTone::new(
                &r,
                &measures,
                Some(Dehazing::new(haze.clone(), [w as u32, h as u32], 0.)),
            )
        };
        let mask = |dehaze: f32| {
            let mut d = [0.; crate::develop::masks::local::LEN];
            d[slot::DEHAZE] = dehaze;
            d
        };
        let (p, pos) = (scene[12 * w + 7], [7., 12.]);
        for (global, local) in [(0.3, 0.3), (0., 0.6), (0.4, -0.6), (-0.2, -0.3)] {
            let masked = tone(global).apply(p, pos, Some(&mask(local)));
            assert_eq!(
                masked,
                tone(global + local).apply(p, pos, None),
                "{global} {local}"
            );
        }
        let removed = tone(0.6).apply(p, pos, None);
        assert!(removed[1] < tone(0.).apply(p, pos, None)[1] - 0.01);
        assert_eq!(
            tone(0.5).apply(p, pos, Some(&mask(-0.5))),
            tone(0.).apply(p, pos, None)
        );
    }

    /// A small specular highlight at the sensor's white leaves Whites' stretch as it is
    /// without it: Camera Raw stretches toward the bulk of the photo's highlights.
    #[test]
    fn a_specular_highlight_does_not_hold_whites_back() {
        let gray = |max: f32| {
            let measures = PhotoMeasures {
                sensor_white: 1.,
                max,
                min: -12.,
                dark: -12.,
                p99: -3.,
            };
            let r = Recipe {
                whites: 1.,
                ..Default::default()
            };
            SceneTone::new(&r, &measures, None).apply([0.05; 3], [0.; 2], None)[1]
        };
        let (specular, bulk) = (gray(1.), gray(-2.));
        assert!((specular / bulk - 1.).abs() < 0.01, "{specular} {bulk}");
    }
}
