//! Results of the stages before the per-pixel color pipeline, kept between preview
//! renders: geometry/lens-warp samples, the measured Texture, the photo's measurement
//! copy the Shadows/Highlights map is built from, and the scene tone stage's measures
//! of that copy (with the haze positive Dehaze removes).
//! Each key holds only the recipe fields its stage reads, so exposure, curve, HSL
//! and grading edits reuse them and rerun only the per-pixel stage.
//!
//! [`stage_recipes`] places every recipe field: a new field does not compile until it
//! is added to the stages that read it, or to those that only later stages read.
use super::{
    Geometry,
    pipeline::{Samples, Toned},
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
    pub(crate) samples: Lru<SampleKey, Samples>,
    /// A photo's measurement copy (`Toned::measured`), per full-resolution image.
    pub(crate) measured: Lru<Same<CameraImage>, CameraImage>,
    /// The measured Texture's detail of a camera image, and the image with an amount.
    pub(crate) texture_detail: Lru<TextureKey, super::texture::TextureDetail>,
    pub(crate) textured: Lru<TextureKey, CameraImage>,
    /// Mask weights of a region.
    pub(crate) masks: Lru<MaskKey, super::masks::MaskWeights>,
    /// Brush masks rasterised in image space.
    pub(crate) rasters: super::masks::RasterCache,
    /// The scene tone stage's measures of a measurement copy, shared with the renders
    /// that read them (`Toned::measures`).
    pub(crate) measures: Arc<MeasuresCache>,
}
/// The scene tone stage's measures (`pipeline::tone::photo_measures`) and, once a render
/// needs it, the photo's haze (`pipeline::tone::photo_haze`), per measurement copy and
/// the settings they read: both read the same scene values.
pub(crate) type MeasuresCache =
    std::sync::Mutex<Lru<MeasuresKey, crate::develop::scene_tone::Measured>>;

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
    /// The kept values, most recently used first.
    #[cfg(test)]
    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|e| e.1.as_ref())
    }
}

/// Identity of a shared value. Keys hold the value, so its address stays unique.
pub(crate) struct Same<T>(pub(crate) Arc<T>);
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

/// The recipe as the cached stages read it: the samples, and the photo's measures.
struct StageRecipes {
    samples: Recipe,
    measures: Recipe,
}
/// Splits `r` into what each cached stage reads. `Recipe` and `Effects` are taken
/// apart without `..`, so adding a field to either is a compile error here until it is
/// placed: a stage that reads a field it does not key on would reuse stale results.
fn stage_recipes(r: &Recipe) -> StageRecipes {
    let Recipe {
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
        effects,
        // White balance, the profile and the camera's exposure render in the per-pixel
        // stage, and the photo's measures read them (Temperature and Tint set the
        // white balance; the look's strength changes the profile once
        // `Recipe::resolved` has applied it; kept anyway, so they can only
        // over-invalidate).
        wb,
        temperature,
        tint,
        auto_white_balance,
        profile,
        profile_amount,
        camera_exposure,
        // The measures are taken with Exposure at 0. Shadows and Highlights render per
        // pixel, from a map built per render; spot removal changes the source image,
        // which every key holds.
        exposure: _,
        shadows: _,
        highlights: _,
        retouch: _,
        red_eye: _,
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
        // Clarity is in the map, built per render, and Texture makes its own image,
        // keyed by `TextureKey`.
        clarity: _,
        texture: _,
        // Camera Calibration renders in the per-pixel stage, and the photo's measures
        // read it.
        calibration,
        shadow_tint,
        // Read only by the per-pixel stage and the finishing stages after these.
        channels: _,
        parametric: _,
        splits: _,
        monochrome: _,
        gray_mix: _,
        balance: _,
        blending: _,
        global_grade: _,
        dehaze: _,
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
        // Manual Vignetting scales the camera image.
        lens_vignette,
        lens_vignette_midpoint,
        defringe: _,
        defringe_ranges: _,
    } = effects;
    StageRecipes {
        // Geometry, lens correction and noise reduction.
        samples: Recipe {
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
        // White balance, the profile's colour and calibration, at the camera's exposure.
        measures: Recipe {
            wb: *wb,
            temperature: *temperature,
            tint: *tint,
            auto_white_balance: *auto_white_balance,
            profile: profile.clone(),
            profile_amount: *profile_amount,
            camera_exposure: *camera_exposure,
            effects: Effects {
                calibration: *calibration,
                shadow_tint: *shadow_tint,
                ..Default::default()
            },
            ..Default::default()
        },
    }
}

/// The photo's measures: the measurement copy they are taken on (whose metadata they
/// read), the profile matrix and the recipe fields they read.
#[derive(PartialEq)]
pub(crate) struct MeasuresKey {
    copy: Same<CameraImage>,
    matrix: [[u32; 3]; 3],
    recipe: Recipe,
}
impl MeasuresKey {
    pub(crate) fn new(copy: &Arc<CameraImage>, r: &Recipe, matrix: [[f32; 3]; 3]) -> Self {
        Self {
            copy: Same(copy.clone()),
            matrix: matrix.map(|row| row.map(f32::to_bits)),
            recipe: stage_recipes(r).measures,
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
/// Samples of an output region: geometry, lens correction and noise reduction.
#[derive(PartialEq)]
pub(crate) struct SampleKey {
    image: Same<CameraImage>,
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
        let changes = |r: &Recipe| stage_recipes(&base).samples != stage_recipes(r).samples;
        // (edit, changes the samples)
        let cases: [(&str, Recipe, bool); 15] = [
            ("temperature", edit(&|r| r.temperature = 3000.), false),
            ("lens vignetting", edit(&|r| r.lens_vignetting = 0.5), true),
            (
                "manual vignetting",
                edit(&|r| r.effects.lens_vignette = -0.5),
                true,
            ),
            ("lens CA", edit(&|r| r.lens_ca = true), true),
            ("distortion", edit(&|r| r.lens_distortion = 0.5), true),
            (
                "manual distortion",
                edit(&|r| r.lens_manual_distortion = -0.3),
                true,
            ),
            ("crop", edit(&|r| r.crop = [0.1, 0.1, 0.9, 0.9]), true),
            ("straighten", edit(&|r| r.straighten = 2.), true),
            ("constrain crop", edit(&|r| r.constrain_crop = true), true),
            ("noise", edit(&|r| r.noise_luma = 0.3), true),
            (
                "chroma detail",
                edit(&|r| r.effects.chroma_detail = 0.1),
                true,
            ),
            ("exposure", edit(&|r| r.exposure = 1.), false),
            ("curve", edit(&|r| r.contrast = 0.4), false),
            ("defringe", edit(&|r| r.effects.defringe = [0.5, 0.]), false),
            ("sharpening", edit(&|r| r.sharpening = 0.9), false),
        ];
        for (name, r, samples) in cases {
            assert_eq!(changes(&r), samples, "{name}");
        }
        // (edit, changes the photo's measures)
        let measures = |r: &Recipe| stage_recipes(&base).measures != stage_recipes(r).measures;
        let cases: [(&str, Recipe, bool); 12] = [
            ("white balance", edit(&|r| r.wb = [1.2, 1., 0.8]), true),
            ("temperature", edit(&|r| r.temperature = 3000.), true),
            ("tint", edit(&|r| r.tint = 10.), true),
            ("profile amount", edit(&|r| r.profile_amount = 0.5), true),
            ("camera exposure", edit(&|r| r.camera_exposure = 0.3), true),
            (
                "calibration",
                edit(&|r| r.effects.calibration[1] = [0.2, 0.]),
                true,
            ),
            ("shadow tint", edit(&|r| r.effects.shadow_tint = 0.2), true),
            ("exposure", edit(&|r| r.exposure = 1.), false),
            ("whites", edit(&|r| r.whites = 0.5), false),
            ("shadows", edit(&|r| r.shadows = 0.5), false),
            ("dehaze", edit(&|r| r.effects.dehaze = 0.5), false),
            ("crop", edit(&|r| r.crop = [0.1, 0.1, 0.9, 0.9]), false),
        ];
        for (name, r, changed) in cases {
            assert_eq!(measures(&r), changed, "{name}");
        }
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
