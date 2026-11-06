//! RAWmakase JSON recipes, including migration of earlier pipeline versions.
use crate::{
    camera_data::Metadata,
    model::recipe::Recipe,
    model::saved_format::{migrate_recipe, schema_for},
    storage::atomic_json,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs::File, path::Path};

#[derive(Serialize, Deserialize)]
struct Preset {
    schema: u32,
    pipeline: u32,
    recipe: Recipe,
    /// Masks, kept outside the recipe so releases before them read the rest (the
    /// envelope accepts unknown keys in every release).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    masks: Vec<crate::model::masks::MaskGroup>,
    /// Made on a rendered file (JPEG, PNG, TIFF), whose white balance and profile a
    /// RAW's don't mix with. Absent in presets made on RAWs, and in every preset from
    /// before rendered files developed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    rendered: bool,
}
/// A preset file as it loads: its recipe, and whether it was made on a rendered file.
pub struct LoadedPreset {
    pub recipe: Recipe,
    rendered: bool,
}
/// Saves `r`, the edit of a photo `m` describes, as a preset.
pub fn save_preset(path: &Path, r: &Recipe, m: &Metadata) -> Result<()> {
    r.validate()?;
    anyhow::ensure!(
        !r.masks
            .iter()
            .any(crate::model::masks::MaskGroup::has_raster),
        "A preset cannot hold masks made from a selection; leave Masking out of it"
    );
    // Spot removal is specific to its photo; Lightroom presets never include it.
    let (mut recipe, local) = r.split_local();
    // Auto white balance was estimated for this photo; applied elsewhere its values are
    // just Custom.
    recipe.auto_white_balance = None;
    // Upright's corrections were analysed from this photo; the mode stays.
    recipe.upright.clear_analysis();
    atomic_json(
        path,
        &Preset {
            schema: schema_for(&recipe),
            pipeline: schema_for(&recipe),
            recipe,
            masks: local.masks,
            rendered: m.is_rendered(),
        },
    )
}
pub fn load_preset(path: &Path) -> Result<LoadedPreset> {
    let mut v: serde_json::Value = serde_json::from_reader(File::open(path)?)?;
    migrate_recipe(&mut v)?;
    let p: Preset = serde_json::from_value(v)?;
    let mut recipe = p.recipe.with_local(crate::model::recipe::LocalEdits {
        retouch: Vec::new(),
        red_eye: Default::default(),
        masks: p.masks,
    });
    // Checked after merging: development builds kept masks inside the recipe itself.
    anyhow::ensure!(
        !recipe
            .masks
            .iter()
            .any(crate::model::masks::MaskGroup::has_raster),
        "This preset refers to a photo's selection mask, which a preset cannot hold"
    );
    recipe.upright.clear_analysis();
    recipe.validate()?;
    Ok(LoadedPreset {
        recipe,
        rendered: p.rendered,
    })
}

/// `preset` as applied to a photo edited as `photo`: presets never carry spot removal
/// or red eye, so the photo keeps its own and their panel switches; the preset's masks
/// replace the photo's only when it has any, as in Lightroom. Between a RAW and a
/// rendered file the photo keeps its white balance and profile, as with Copy and Sync.
pub fn applied_to(preset: LoadedPreset, photo: &Recipe, m: &Metadata) -> Recipe {
    applied_to_with_lensfun(
        preset,
        photo,
        m,
        &crate::lens::lensfun::installed::Handle::Installed,
    )
}
/// [`applied_to`], with a Lensfun lens taken to this photo's camera against `lensfun`.
pub(crate) fn applied_to_with_lensfun(
    preset: LoadedPreset,
    photo: &Recipe,
    m: &Metadata,
    lensfun: &crate::lens::lensfun::installed::Handle,
) -> Recipe {
    use crate::model::panels::Panel;
    let LoadedPreset {
        recipe: mut preset,
        rendered,
    } = preset;
    preset.retouch = photo.retouch.clone();
    preset.red_eye = photo.red_eye.clone();
    for panel in [Panel::SpotRemoval, Panel::RedEye] {
        preset.panels.set(panel, photo.panels.state(panel));
    }
    if preset.masks.is_empty() {
        preset.masks = photo.masks.clone();
    }
    if rendered != m.is_rendered() {
        preset.keep_white_balance_and_profile(photo);
    }
    // A Lensfun lens takes this photo's camera.
    preset.lens_profile_choice.for_photo(m, lensfun);
    preset
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A native preset's Lensfun lens takes the photo's camera it is applied to.
    #[test]
    fn a_preset_s_lensfun_lens_takes_the_photo_s_camera() {
        use crate::lens::lensfun::fixture;
        let mut r = Recipe::default();
        r.lens_profile_choice.choose_lensfun(fixture::choice());
        let m = fixture::other_camera();
        let applied = applied_to_with_lensfun(
            LoadedPreset {
                recipe: r,
                rendered: false,
            },
            &Recipe::default(),
            &m,
            &fixture::installed(-0.02, 1_793_180_205),
        );
        let camera = applied
            .lens_profile_choice
            .lensfun()
            .unwrap()
            .camera
            .clone()
            .unwrap();
        assert_eq!(camera.model, "Canon EOS 6D");
    }
    use crate::model::panels::{Panel, PanelState};
    use crate::model::red_eye::RedEyeOp;
    #[test]
    fn a_preset_leaves_the_photos_red_eye_and_its_switch() {
        let mut photo = Recipe::default();
        photo.red_eye.push(RedEyeOp {
            kind: Default::default(),
            center: [0.4, 0.4],
            radius: [0.01; 2],
            correlation: 0.,
            pupil_size: 0.5,
            darken: 0.5,
        });
        let mut preset = Recipe {
            exposure: 0.5,
            ..Default::default()
        };
        preset.panels.set(Panel::RedEye, PanelState::Off);
        preset.panels.set(Panel::SpotRemoval, PanelState::Off);
        let applied = applied_to(
            LoadedPreset {
                recipe: preset,
                rendered: false,
            },
            &photo,
            &Metadata::default(),
        );
        assert_eq!(applied.exposure, 0.5);
        assert_eq!(applied.red_eye, photo.red_eye);
        assert_eq!(applied.panels.state(Panel::RedEye), PanelState::On);
        assert_eq!(applied.panels.state(Panel::SpotRemoval), PanelState::On);
    }
    fn rendered() -> Metadata {
        Metadata {
            source: crate::camera_data::Source::Rendered,
            ..Default::default()
        }
    }
    /// A RAW's preset, from before rendered files developed, leaves a JPEG its white
    /// balance and profile (a RAW's gains and camera profile would render it wrong)
    /// and applies the rest.
    #[test]
    fn a_raw_preset_leaves_a_rendered_photo_its_white_balance_and_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let text = serde_json::json!({
            "schema": 11,
            "pipeline": 11,
            "recipe": {
                "exposure": 0.5,
                "camera_exposure": 0.35,
                "temperature": 3000.0,
                "wb": [2.0, 1.0, 0.5],
            },
        });
        std::fs::write(&path, text.to_string()).unwrap();
        let m = rendered();
        let mut photo = Recipe::for_rendered(&m);
        photo.incremental_temperature = 20.;
        let applied = applied_to(load_preset(&path).unwrap(), &photo, &m);
        assert_eq!(applied.exposure, 0.5);
        assert_eq!(applied.wb, [1.; 3]);
        assert_eq!(applied.incremental_temperature, 20.);
        assert_eq!(applied.temperature, photo.temperature);
        // The RAW camera's baseline exposure is its profile's, not the JPEG's.
        assert_eq!(applied.camera_exposure, 0.);
        assert!(applied.profile.is_some_and(|p| p.is_embedded()));
    }
    /// A rendered file's preset carries its relative white balance to another
    /// rendered file, and leaves a RAW its own.
    #[test]
    fn a_rendered_preset_keeps_relative_white_balance_to_rendered_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let m = rendered();
        let mut r = Recipe::for_rendered(&m);
        r.incremental_temperature = 30.;
        r.exposure = 0.5;
        save_preset(&path, &r, &m).unwrap();
        let other = Recipe::for_rendered(&m);
        let applied = applied_to(load_preset(&path).unwrap(), &other, &m);
        assert_eq!(applied.incremental_temperature, 30.);
        let raw = Recipe {
            wb: [2., 1., 1.5],
            temperature: 4000.,
            camera_exposure: 0.35,
            ..Default::default()
        };
        let applied = applied_to(load_preset(&path).unwrap(), &raw, &Metadata::default());
        assert_eq!(applied.exposure, 0.5);
        assert_eq!(applied.incremental_temperature, 0.);
        assert_eq!((applied.wb, applied.temperature), (raw.wb, raw.temperature));
        assert_eq!(applied.profile, raw.profile);
        assert_eq!(applied.camera_exposure, 0.35);
    }
    /// A preset is saved as the one engine's version, which releases before it
    /// refuse as newer.
    #[test]
    fn a_saved_preset_is_refused_by_releases_before_the_one_engine() {
        use crate::model::saved_format::{BEFORE_ONE_ENGINE, reads};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let r = Recipe {
            exposure: 0.5,
            ..Default::default()
        };
        save_preset(&path, &r, &Metadata::default()).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            (&saved["schema"], &saved["pipeline"]),
            (&11.into(), &11.into())
        );
        assert!(!reads(&saved, BEFORE_ONE_ENGINE));
        assert_eq!(load_preset(&path).unwrap().recipe.exposure, 0.5);
    }
    /// HDR values kept while HDR is off save in the version earlier releases read and
    /// load back; a preset with HDR on saves in the HDR schema and loads back.
    #[test]
    fn a_preset_keeps_hdr_values_and_saves_hdr_on_in_its_schema() {
        use crate::model::saved_format::{AI_DENOISE_SCHEMA, HDR_SCHEMA, reads};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let mut r = Recipe::default();
        r.hdr.limit = 6.;
        r.hdr.sdr.whites = -0.25;
        save_preset(&path, &r, &Metadata::default()).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(reads(&saved, AI_DENOISE_SCHEMA));
        assert_eq!(load_preset(&path).unwrap().recipe.hdr, r.hdr);

        // HDR on saves as the HDR schema, which earlier releases refuse, and loads back.
        r.hdr.enabled = true;
        save_preset(&path, &r, &Metadata::default()).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!reads(&saved, AI_DENOISE_SCHEMA));
        assert!(reads(&saved, HDR_SCHEMA));
        assert_eq!(load_preset(&path).unwrap().recipe.hdr, r.hdr);
    }
    /// A preset an earlier release saved loses the settings that chose its engine
    /// and operators, and keeps its sliders and a newer release's setting.
    #[test]
    fn an_earlier_preset_loads_without_its_engine_and_operators() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let text = serde_json::json!({
            "schema": 10,
            "pipeline": 10,
            "recipe": {
                "exposure": 0.5,
                "effects": {"clarity": 0.3},
                "engine": 3,
                "profile_tone": true,
                "clarity_model": "Original",
                "white_balance_model": "Calibrated",
                "future_slider": 2,
            },
        });
        std::fs::write(&path, text.to_string()).unwrap();
        let r = load_preset(&path).unwrap().recipe;
        assert_eq!((r.exposure, r.effects.clarity), (0.5, 0.3));
        assert_eq!(
            r.unknown,
            [("future_slider".to_string(), serde_json::json!(2))].into()
        );
    }
    #[test]
    fn a_preset_refuses_masks_made_from_a_selection() {
        use crate::model::masks::{
            BITMAP_SAMPLING, BitmapMask, MaskComponent, MaskGroup, MaskShape,
        };
        let r = Recipe {
            masks: vec![MaskGroup {
                components: vec![MaskComponent::new(MaskShape::Bitmap(BitmapMask {
                    id: format!("sha256:{}", "ab".repeat(32)),
                    width: 2,
                    height: 2,
                    sampling: BITMAP_SAMPLING,
                    source: None,
                }))],
                ..Default::default()
            }],
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        assert!(save_preset(&path, &r, &Metadata::default()).is_err());
        assert!(!path.exists());
    }
    #[test]
    fn a_preset_with_a_selection_mask_inside_its_recipe_is_refused() {
        use crate::model::masks::{
            BITMAP_SAMPLING, BitmapMask, MaskComponent, MaskGroup, MaskShape,
        };
        let recipe = Recipe {
            masks: vec![MaskGroup {
                components: vec![MaskComponent::new(MaskShape::Bitmap(BitmapMask {
                    id: format!("sha256:{}", "ab".repeat(32)),
                    width: 2,
                    height: 2,
                    sampling: BITMAP_SAMPLING,
                    source: None,
                }))],
                ..Default::default()
            }],
            ..Default::default()
        };
        // As a development build wrote it: masks in the recipe, none beside it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let value = serde_json::json!({
            "schema": crate::model::saved_format::SCHEMA,
            "pipeline": crate::model::saved_format::SCHEMA,
            "recipe": recipe,
        });
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(load_preset(&path).is_err());
    }
}
