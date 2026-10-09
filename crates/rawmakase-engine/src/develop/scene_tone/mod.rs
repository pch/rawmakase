//! The scene tone stage (S3): the Basic panel's tone controls on scene-linear ProPhoto
//! RGB, after Exposure and before the camera profile's look and tone curve, where
//! Camera Raw applies them (docs/scene-tone-stage.md).
mod global;
mod measures;
pub(crate) use global::{
    GlobalTone, MASK_VALUES, SLIDER_STEP, SLIDERS, T_FIRST, T_SAMPLES, T_STEP, U_FIRST, U_SAMPLES,
    U_STEP, Y_FIRST, Y_SAMPLES, Y_STEP, gpu_mask_table,
};
pub(crate) use measures::{MAX_EDGE, PhotoMeasures};

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
    /// Dehaze and the level its response is relative to (log2).
    pub(crate) dehaze: f32,
    pub(crate) dehaze_key: f32,
}
impl SceneTone {
    pub(crate) fn new(r: &Recipe, measures: &PhotoMeasures) -> Self {
        Self {
            global: GlobalTone::new(
                [
                    measures.sensor_white + r.exposure,
                    measures.max + r.exposure,
                    measures.white_point(r.exposure),
                ],
                r.whites,
                r.blacks,
                measures.black_key(r.exposure),
                r.profile
                    .as_ref()
                    .is_none_or(|p| p.black_render() != crate::camera_profiles::BlackRender::None),
            ),
            dehaze: r.effects.dehaze,
            dehaze_key: measures.p99 + r.exposure,
        }
    }
    /// S3's Dehaze and global curves on one pixel of linear ProPhoto RGB, with a mask's
    /// Dehaze added to the recipe's and its Whites and Blacks as their own curves after
    /// the global ones, as Camera Raw renders them.
    pub(crate) fn apply(&self, pro: [f32; 3], local: Option<&LocalDelta>) -> [f32; 3] {
        let dehaze = self.dehaze + local.map_or(0., |d| d[slot::DEHAZE]);
        let pro = if dehaze != 0. {
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
