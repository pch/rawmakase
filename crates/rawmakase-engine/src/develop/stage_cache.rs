//! Results of the stages before the per-pixel color pipeline, kept between preview
//! renders: local-tone blurs, the local-tone image, geometry/lens-warp samples and the
//! reduced image the engine 4 Shadows/Highlights map is built from.
//! Each key holds only the recipe fields its stage reads, so exposure, curve, HSL
//! and grading edits reuse all three and rerun only the per-pixel stage.
//!
//! [`stage_recipes`] places every recipe field: a new field does not compile until it
//! is added to the stages that read it, or to those that only later stages read.
use super::{
    Geometry,
    pipeline::{Samples, Toned},
    quality::LocalBlurs,
};
use crate::model::recipe::Recipe;
use crate::{camera_data::CameraImage, model::effects::Effects};
use anyhow::Result;
use std::sync::Arc;

/// Most recent entries kept per stage: Fit, the quick Fit shown when leaving 100%, and
/// the 100% view's reduced preview and full region, so switching views reuses them.
const ENTRIES: usize = 4;
/// Byte budget per stage. Larger results are computed but not kept.
const BUDGET: usize = 512 << 20;

#[derive(Default)]
pub(crate) struct StageCache {
    pub(crate) blurs: Lru<BlurKey, LocalBlurs>,
    pub(crate) local: Lru<LocalKey, Vec<f32>>,
    pub(crate) samples: Lru<SampleKey, Samples>,
    pub(crate) reduced: Lru<ReducedKey, CameraImage>,
    /// The measured Texture's detail of a camera image, and the image with an amount.
    pub(crate) texture_detail: Lru<TextureKey, super::texture::TextureDetail>,
    pub(crate) textured: Lru<TextureKey, CameraImage>,
    /// Mask weights of a region.
    pub(crate) masks: Lru<MaskKey, super::masks::MaskWeights>,
    /// Brush masks rasterised in image space.
    pub(crate) rasters: super::masks::RasterCache,
    /// The photo's measures and its Shadows/Highlights map before the sliders: what the
    /// engine 4 tone stage makes of the reduced photo (see [`ToneKey`]).
    pub(crate) pivots: Lru<ToneKey, f32>,
    pub(crate) highlights: Lru<ToneKey, f32>,
    pub(crate) maps: Lru<ToneKey, super::local_tone::MapBase>,
}

pub(crate) struct Lru<K, V> {
    /// Most recently used first.
    entries: Vec<(K, Arc<V>, usize)>,
}
impl<K, V> Default for Lru<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}
impl<K: PartialEq, V> Lru<K, V> {
    pub(crate) fn get_or_try(
        &mut self,
        key: K,
        bytes: impl Fn(&V) -> usize,
        make: impl FnOnce() -> Result<V>,
    ) -> Result<Arc<V>> {
        if let Some(i) = self.entries.iter().position(|(k, ..)| *k == key) {
            let entry = self.entries.remove(i);
            let value = entry.1.clone();
            self.entries.insert(0, entry);
            return Ok(value);
        }
        let value = Arc::new(make()?);
        let size = bytes(&value);
        if size <= BUDGET {
            self.entries.insert(0, (key, value.clone(), size));
            self.entries.truncate(ENTRIES);
            while self.entries.iter().map(|e| e.2).sum::<usize>() > BUDGET {
                self.entries.pop();
            }
        }
        Ok(value)
    }
    /// The value for `key`, made most recently used.
    pub(crate) fn get(&mut self, key: &K) -> Option<Arc<V>> {
        let i = self.entries.iter().position(|(k, ..)| k == key)?;
        let entry = self.entries.remove(i);
        let value = entry.1.clone();
        self.entries.insert(0, entry);
        Some(value)
    }
    pub(crate) fn insert(&mut self, key: K, value: Arc<V>, size: usize) {
        if size <= BUDGET {
            self.entries.retain(|(k, ..)| *k != key);
            self.entries.insert(0, (key, value, size));
            self.entries.truncate(ENTRIES);
            while self.entries.iter().map(|e| e.2).sum::<usize>() > BUDGET {
                self.entries.pop();
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Identity of a shared value. Keys hold the value, so its address stays unique.
pub(crate) struct Same<T>(Arc<T>);
impl<T> Clone for Same<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> PartialEq for Same<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// The recipe as the cached stages read it: the local-tone blurs, the samples and the
/// engine 4 tone stage.
struct StageRecipes {
    blurs: Recipe,
    samples: Recipe,
    tone: Recipe,
}
/// Splits `r` into what each cached stage reads. `Recipe` and `Effects` are taken
/// apart without `..`, so adding a field to either is a compile error here until it is
/// placed: a stage that reads a field it does not key on would reuse stale results.
fn stage_recipes(r: &Recipe) -> StageRecipes {
    let Recipe {
        engine,
        wb,
        temperature,
        profile,
        lens_builtin,
        lens_profile,
        lens_profile_choice,
        lens_distortion,
        lens_vignetting,
        lens_manual_distortion,
        lens_ca,
        crop,
        rotation,
        straighten,
        constrain_crop,
        flip_x,
        flip_y,
        transform,
        upright,
        noise_luma,
        noise_chroma,
        // The measured operator changes the source image, which every key holds.
        noise_model: _,
        lens_vignette_model,
        effects,
        // Shadows/Highlights and their exposure are keyed by `LocalKey`; spot removal
        // changes the source image, which every key holds. The tone stage reads the
        // exposure; engine 4 applies Shadows and Highlights after it, from the map.
        exposure,
        camera_exposure,
        shadows: _,
        highlights: _,
        retouch: _,
        retouch_model: _,
        red_eye: _,
        // The profile tone curve ends the tone stage.
        profile_tone,
        // The look's strength; its Shadows, Highlights and Clarity are keyed by
        // `LocalKey` once `Recipe::resolved` has added them. The tone stage applies the
        // look's tables.
        profile_amount,
        tint,
        auto_white_balance: _,
        wide_gamut_curves: _,
        // Which tone stage and calibration render.
        reference_curves,
        reference_calibration,
        reference_color,
        parametric_model: _,
        grain_model: _,
        // The measured Clarity is in the map, built per render; the original one is
        // keyed by `LocalKey` through `effects.clarity`.
        clarity_model: _,
        // The measured Texture makes its own image, keyed by `TextureKey`.
        texture_model: _,
        contrast_model: _,
        grading_model: _,
        mixer_model: _,
        saturation_model: _,
        vibrance_model: _,
        black_white_model: _,
        calibration_model,
        whites_model: _,
        white_balance_model: _,
        gamut_model: _,
        contrast: _,
        whites: _,
        blacks: _,
        black_point: _,
        white_point: _,
        midtone: _,
        curve: _,
        curve_saturation: _,
        saturation: _,
        vibrance: _,
        hsl: _,
        point_colors: _,
        grading: _,
        sharpening: _,
        sharpening_radius: _,
        sharpening_detail: _,
        sharpening_masking: _,
        sharpening_model: _,
        masks: _,
        // Not read when rendering: switched-off panels are bypassed before the stages
        // (see `Recipe::as_rendered`).
        panels: _,
        preset_name: _,
        preset_settings: _,
        unknown: _,
    } = r;
    let Effects {
        luma_detail,
        luma_contrast,
        chroma_detail,
        chroma_smoothness,
        // Keyed by `LocalKey`.
        clarity: _,
        texture: _,
        // Read only by the per-pixel stage and the finishing stages after these, but
        // for the calibration the tone stage applies.
        channels: _,
        parametric: _,
        splits: _,
        calibration,
        shadow_tint,
        monochrome: _,
        gray_mix: _,
        balance: _,
        blending: _,
        global_grade: _,
        // The tone stage applies it before engine 4's measured curve.
        dehaze,
        grain: _,
        grain_size: _,
        grain_roughness: _,
        grain_seed: _,
        vignette: _,
        vignette_midpoint: _,
        vignette_roundness: _,
        vignette_feather: _,
        vignette_highlights: _,
        vignette_style: _,
        // Manual Vignetting, measured, scales the camera image; the original
        // operator's only finishes it, so it is keyed on without effect.
        lens_vignette,
        lens_vignette_midpoint,
        defringe: _,
        defringe_ranges: _,
    } = effects;
    StageRecipes {
        // Engine 4's tone stage (`pipeline::tone_stage`): white balance, camera profile,
        // calibration, exposure and its black ramp, and the profile's look and tone
        // curve. Contrast, Whites, Blacks, Shadows and Highlights come after it.
        tone: Recipe {
            engine: *engine,
            wb: *wb,
            temperature: *temperature,
            tint: *tint,
            profile: profile.clone(),
            profile_tone: *profile_tone,
            profile_amount: *profile_amount,
            exposure: *exposure,
            camera_exposure: *camera_exposure,
            reference_curves: *reference_curves,
            reference_calibration: *reference_calibration,
            reference_color: *reference_color,
            calibration_model: *calibration_model,
            effects: Effects {
                calibration: *calibration,
                shadow_tint: *shadow_tint,
                dehaze: *dehaze,
                ..Default::default()
            },
            ..Default::default()
        },
        // Log luminance after white balance, profile matrix and lens vignetting.
        blurs: Recipe {
            engine: *engine,
            wb: *wb,
            temperature: *temperature,
            // The blurs read the camera matrices and tables, not the look.
            profile: profile.as_ref().map(|p| p.camera_part()),
            lens_builtin: *lens_builtin,
            lens_profile: *lens_profile,
            lens_profile_choice: lens_profile_choice.clone(),
            lens_vignetting: *lens_vignetting,
            lens_vignette_model: *lens_vignette_model,
            effects: Effects {
                lens_vignette: *lens_vignette,
                lens_vignette_midpoint: *lens_vignette_midpoint,
                ..Default::default()
            },
            ..Default::default()
        },
        // Geometry, lens correction and noise reduction.
        samples: Recipe {
            engine: *engine,
            crop: *crop,
            rotation: *rotation,
            straighten: *straighten,
            constrain_crop: *constrain_crop,
            flip_x: *flip_x,
            flip_y: *flip_y,
            transform: *transform,
            upright: upright.clone(),
            lens_builtin: *lens_builtin,
            lens_profile: *lens_profile,
            lens_profile_choice: lens_profile_choice.clone(),
            lens_distortion: *lens_distortion,
            lens_vignetting: *lens_vignetting,
            lens_manual_distortion: *lens_manual_distortion,
            lens_ca: *lens_ca,
            lens_vignette_model: *lens_vignette_model,
            noise_luma: *noise_luma,
            noise_chroma: *noise_chroma,
            effects: Effects {
                luma_detail: *luma_detail,
                luma_contrast: *luma_contrast,
                chroma_detail: *chroma_detail,
                chroma_smoothness: *chroma_smoothness,
                lens_vignette: *lens_vignette,
                lens_vignette_midpoint: *lens_vignette_midpoint,
                ..Default::default()
            },
            ..Default::default()
        },
    }
}

/// Local-tone blurs: log luminance after white balance, profile matrix and lens
/// vignetting, before exposure.
#[derive(Clone, PartialEq)]
pub(crate) struct BlurKey {
    image: Same<CameraImage>,
    scale: u32,
    texture: bool,
    recipe: Recipe,
}
impl BlurKey {
    pub(crate) fn new(image: &Arc<CameraImage>, r: &Recipe, scale: f32, texture: bool) -> Self {
        Self {
            image: Same(image.clone()),
            scale: scale.to_bits(),
            texture,
            recipe: stage_recipes(r).blurs,
        }
    }
}
/// The local-tone gain: blurs plus the sliders applied to them. Exposure only
/// matters to Shadows and Highlights. Keyed by the blurs' inputs rather than the blurs
/// themselves, so the gain is found again even when the blurs were too large to keep
/// (a 61-megapixel photo's), and exposure edits at 100% do not recompute them.
#[derive(Clone, PartialEq)]
pub(crate) struct LocalKey {
    blurs: BlurKey,
    sliders: [u32; 5],
}
impl LocalKey {
    pub(crate) fn new(blurs: BlurKey, r: &Recipe) -> Self {
        let exposure = if r.shadows != 0. || r.highlights != 0. {
            r.exposure + r.camera_exposure
        } else {
            0.
        };
        Self {
            blurs,
            sliders: [
                exposure,
                r.shadows,
                r.highlights,
                r.effects.clarity,
                r.effects.texture,
            ]
            .map(f32::to_bits),
        }
    }
}
/// The measured Texture's detail and image: the source image, its scale and (for the
/// image) the amount.
#[derive(Clone, PartialEq)]
pub(crate) struct TextureKey {
    image: Same<CameraImage>,
    scale: u32,
    amount: u32,
}
impl TextureKey {
    pub(crate) fn new(image: &Arc<CameraImage>, scale: f32, amount: f32) -> Self {
        Self {
            image: Same(image.clone()),
            scale: scale.to_bits(),
            amount: amount.to_bits(),
        }
    }
}
/// The toned image reduced for the engine 4 Shadows/Highlights map: the camera image
/// and its local-tone gain.
#[derive(PartialEq)]
pub(crate) struct ReducedKey {
    image: Same<CameraImage>,
    gain: Option<LocalKey>,
}
impl ReducedKey {
    pub(crate) fn new(toned: &Toned) -> Self {
        Self {
            image: Same(toned.image.clone()),
            gain: toned.gain_key.clone(),
        }
    }
}
/// The recipe as the engine 4 tone stage reads it.
pub(crate) fn tone_recipe(r: &Recipe) -> Recipe {
    stage_recipes(r).tone
}
/// What the engine 4 tone stage makes of the reduced photo: its Contrast pivot (measured
/// at the camera's exposure), the highlights positive Whites follows and the
/// Shadows/Highlights map's base. The photo, its local-tone gain and its image before
/// Texture, and the recipe as the tone stage reads it.
#[derive(Clone, PartialEq)]
pub(crate) struct ToneKey {
    image: Same<CameraImage>,
    gain: Option<LocalKey>,
    untextured: Option<Same<CameraImage>>,
    recipe: Recipe,
}
impl ToneKey {
    /// For measures at the recipe's exposure.
    pub(crate) fn new(toned: &Toned, r: &Recipe) -> Self {
        Self {
            image: Same(toned.image.clone()),
            gain: toned.gain_key.clone(),
            untextured: toned.untextured.clone().map(Same),
            recipe: stage_recipes(r).tone,
        }
    }
    /// For the Contrast pivot, which the user's Exposure does not move.
    pub(crate) fn without_exposure(toned: &Toned, r: &Recipe) -> Self {
        Self::new(
            toned,
            &Recipe {
                exposure: 0.,
                ..r.clone()
            },
        )
    }
}
/// Samples of an output region: geometry, lens correction and noise reduction.
#[derive(PartialEq)]
pub(crate) struct SampleKey {
    image: Same<CameraImage>,
    gain: Option<LocalKey>,
    size: [u32; 2],
    region: [u32; 4],
    spread: u32,
    recipe: Recipe,
}
impl SampleKey {
    pub(crate) fn new(
        toned: &Toned,
        r: &Recipe,
        g: &Geometry,
        region: [u32; 4],
        spread: f32,
    ) -> Self {
        Self {
            image: Same(toned.image.clone()),
            gain: toned.gain_key.clone(),
            size: [g.width, g.height],
            region,
            spread: spread.to_bits(),
            recipe: stage_recipes(r).samples,
        }
    }
}

/// Mask weights of a region: where its samples come from, and the shapes of the
/// masks that render (not their sliders, which apply later). Range components read
/// the developed colours, so then the rest of the recipe counts too.
#[derive(PartialEq)]
pub(crate) struct MaskKey {
    samples: SampleKey,
    masks: Vec<(Vec<crate::model::masks::MaskComponent>, bool)>,
    recipe: Option<Recipe>,
}
impl MaskKey {
    pub(crate) fn new(
        toned: &Toned,
        r: &Recipe,
        g: &Geometry,
        region: [u32; 4],
        spread: f32,
        ranges: bool,
    ) -> Self {
        Self {
            samples: SampleKey::new(toned, r, g, region, spread),
            masks: r
                .masks
                .iter()
                .filter(|m| m.is_active())
                .map(|m| (m.components.clone(), m.invert))
                .collect(),
            recipe: ranges.then(|| Recipe {
                masks: Vec::new(),
                ..r.clone()
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Which edits each cached stage recomputes for: the stages before the per-pixel
    /// one must change with their own settings and must not with later ones.
    #[test]
    fn stage_keys_follow_the_settings_each_stage_reads() {
        let base = Recipe::default();
        let edit = |f: &dyn Fn(&mut Recipe)| {
            let mut r = base.clone();
            f(&mut r);
            r
        };
        let changes = |r: &Recipe| {
            let (a, b) = (stage_recipes(&base), stage_recipes(r));
            (a.blurs != b.blurs, a.samples != b.samples, a.tone != b.tone)
        };
        // (edit, changes the blurs, changes the samples, changes the tone stage)
        let cases: [(&str, Recipe, bool, bool, bool); 23] = [
            (
                "temperature",
                edit(&|r| r.temperature = 3000.),
                true,
                false,
                true,
            ),
            (
                "lens vignetting",
                edit(&|r| r.lens_vignetting = 0.5),
                true,
                true,
                false,
            ),
            (
                "manual vignetting",
                edit(&|r| {
                    r.lens_vignette_model = crate::model::operators::LensVignetteModel::Measured;
                    r.effects.lens_vignette = -0.5;
                }),
                true,
                true,
                false,
            ),
            ("lens CA", edit(&|r| r.lens_ca = true), false, true, false),
            (
                "distortion",
                edit(&|r| r.lens_distortion = 0.5),
                false,
                true,
                false,
            ),
            (
                "manual distortion",
                edit(&|r| r.lens_manual_distortion = -0.3),
                false,
                true,
                false,
            ),
            (
                "crop",
                edit(&|r| r.crop = [0.1, 0.1, 0.9, 0.9]),
                false,
                true,
                false,
            ),
            (
                "straighten",
                edit(&|r| r.straighten = 2.),
                false,
                true,
                false,
            ),
            (
                "constrain crop",
                edit(&|r| r.constrain_crop = true),
                false,
                true,
                false,
            ),
            ("noise", edit(&|r| r.noise_luma = 0.3), false, true, false),
            (
                "chroma detail",
                edit(&|r| r.effects.chroma_detail = 0.1),
                false,
                true,
                false,
            ),
            ("exposure", edit(&|r| r.exposure = 1.), false, false, true),
            ("curve", edit(&|r| r.contrast = 0.4), false, false, false),
            (
                "defringe",
                edit(&|r| r.effects.defringe = [0.5, 0.]),
                false,
                false,
                false,
            ),
            (
                "sharpening",
                edit(&|r| r.sharpening = 0.9),
                false,
                false,
                false,
            ),
            ("whites", edit(&|r| r.whites = 0.5), false, false, false),
            ("blacks", edit(&|r| r.blacks = -0.5), false, false, false),
            ("shadows", edit(&|r| r.shadows = 0.5), false, false, false),
            (
                "highlights",
                edit(&|r| r.highlights = -0.5),
                false,
                false,
                false,
            ),
            ("hsl", edit(&|r| r.hsl[0][1] = 0.5), false, false, false),
            (
                "calibration",
                edit(&|r| r.effects.calibration[0] = [0.2, 0.1]),
                false,
                false,
                true,
            ),
            ("tint", edit(&|r| r.tint = 20.), false, false, true),
            (
                "camera exposure",
                edit(&|r| r.camera_exposure = 0.3),
                false,
                false,
                true,
            ),
        ];
        for (name, r, blurs, samples, tone) in cases {
            assert_eq!(changes(&r), (blurs, samples, tone), "{name}");
        }
    }
    /// The blurs read the camera part of the profile only: a look's Profile Amount,
    /// which `Recipe::resolved` puts into the profile, reuses them.
    #[test]
    fn profile_amount_reuses_the_blurs() {
        let m = crate::camera_data::Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        };
        let look = Arc::new(crate::camera_profiles::CameraProfile::creative_for_test(&m));
        let at = |amount: f32| {
            Recipe {
                engine: 4,
                profile: Some(look.clone()),
                profile_amount: amount,
                ..Default::default()
            }
            .resolved(&m)
            .into_owned()
        };
        let (full, half) = (at(1.), at(0.5));
        assert_ne!(full.profile, half.profile);
        assert!(stage_recipes(&full).blurs == stage_recipes(&half).blurs);
    }
    #[test]
    fn lru_keeps_recent_entries_within_budget() -> Result<()> {
        let mut lru: Lru<u32, Vec<u8>> = Lru::default();
        let mut made = 0;
        let mut get = |lru: &mut Lru<u32, Vec<u8>>, key: u32, size: usize| {
            lru.get_or_try(key, Vec::len, || {
                made += 1;
                Ok(vec![0; size])
            })
            .map(|_| made)
        };
        let n = ENTRIES as u32;
        for key in 1..=n {
            assert_eq!(get(&mut lru, key, 10)?, key);
        }
        // Hits make nothing; key 1 becomes the most recently used.
        assert_eq!(get(&mut lru, 1, 10)?, n);
        // A new key evicts the least recently used one, key 2.
        assert_eq!(get(&mut lru, n + 1, 10)?, n + 1);
        assert_eq!(get(&mut lru, 1, 10)?, n + 1);
        assert_eq!(get(&mut lru, 2, 10)?, n + 2);
        // Oversized results are returned but not kept.
        assert_eq!(get(&mut lru, n + 2, BUDGET + 1)?, n + 3);
        assert_eq!(lru.len(), ENTRIES);
        assert!(
            lru.get_or_try(0, Vec::len, || anyhow::bail!("cancelled"))
                .is_err()
        );
        assert_eq!(lru.len(), ENTRIES);
        Ok(())
    }
}
