//! The version a saved recipe is written with, and the migration of older ones.
//! Catalog edits, native presets and legacy sidecars all save recipes this way.
use anyhow::{Context, Result, ensure};
/// The version written. Spots and masks are saved apart from the recipe (see
/// `LocalEdits`), so recipes stay readable by releases that predate them.
pub const SCHEMA: u32 = 6;
#[cfg(test)]
pub const PIPELINE: u32 = 6;
/// The version written for a recipe whose look has a Profile Amount or internal
/// Contrast or Blacks: releases that read 6 and 7 reject those profile fields, so
/// they are told the file is newer instead.
const LOOK_AMOUNT: u32 = 8;
/// The version written for a recipe whose look has an RGB table (or no HSV table)
/// or carries develop settings, which releases that read 8 reject.
const RGB_TABLE: u32 = 9;
/// Matrix-only DNG signatures add a profile field rejected by older readers.
const MATRIX_SIGNATURE: u32 = 10;

/// The schema and pipeline version to write `r` with.
pub fn saved_version(r: &super::recipe::Recipe) -> u32 {
    if r.profile
        .as_ref()
        .is_some_and(|p| p.has_matrix_calibration_signature())
    {
        return MATRIX_SIGNATURE;
    }
    let Some(look) = r.profile.as_ref().and_then(|p| p.enhanced.as_ref()) else {
        return SCHEMA;
    };
    if look.has_rgb_table_or_settings() {
        RGB_TABLE
    } else if look.amount.is_some() || look.contrast != 0. || look.blacks != 0. {
        LOOK_AMOUNT
    } else {
        SCHEMA
    }
}

pub fn migrate_recipe(value: &mut serde_json::Value) -> Result<()> {
    let schema = value["schema"].as_u64();
    let pipeline = value["pipeline"].as_u64();
    ensure!(
        matches!(
            (schema, pipeline),
            (Some(1), Some(1))
                | (Some(2), Some(2))
                | (Some(3), Some(3))
                | (Some(4), Some(4))
                | (Some(5), Some(5))
                | (Some(6), Some(6))
                // Development builds of the retouch tools wrote 7 with the spots and
                // masks inside the recipe; they load as they are.
                | (Some(7), Some(7))
                | (Some(8), Some(8))
                | (Some(9), Some(9))
                | (Some(10), Some(10))
        ),
        "Unsupported saved recipe version: preserved without changes"
    );
    let recipe = value
        .get_mut("recipe")
        .and_then(serde_json::Value::as_object_mut)
        .context("Saved recipe must be an object")?;
    if pipeline.is_some_and(|p| p < 3) {
        // The first pipelines saved neither a camera profile in today's form nor
        // sharpening.
        recipe.insert("profile".into(), serde_json::Value::Null);
        recipe.entry("sharpening").or_insert(0.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Every version this build reads migrates; recipes keep the settings they
    /// were saved with and render with the one engine.
    #[test]
    fn saved_versions_migrate_without_an_engine() {
        for version in 1..=10 {
            let mut v = serde_json::json!({"schema": version, "pipeline": version, "recipe": {"exposure": 0.5}});
            super::migrate_recipe(&mut v).unwrap();
            assert!(v["recipe"].get("engine").is_none(), "{version}");
            assert_eq!(v["recipe"]["exposure"], 0.5);
        }
        let mut v = serde_json::json!({"schema": 11, "pipeline": 11, "recipe": {}});
        assert!(super::migrate_recipe(&mut v).is_err());
        let mut v = serde_json::json!({"schema": 5, "pipeline": 6, "recipe": {}});
        assert!(super::migrate_recipe(&mut v).is_err());
    }
    /// A look with a Profile Amount saves as version 8 and one with an RGB table as
    /// 9, which releases that reject their fields refuse as newer; everything else
    /// stays at 6.
    #[test]
    fn only_looks_with_new_fields_raise_the_saved_version() {
        use crate::{camera_data::Metadata, camera_profiles::CameraProfile, model::recipe::Recipe};
        let m = Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        };
        let mut r = Recipe::default();
        assert_eq!(super::saved_version(&r), super::SCHEMA);
        r.profile = crate::camera_profiles::open::color(&m).map(std::sync::Arc::new);
        assert_eq!(super::saved_version(&r), super::SCHEMA);
        r.profile = Some(std::sync::Arc::new(CameraProfile::creative_for_test(&m)));
        assert_eq!(super::saved_version(&r), 8);
        // A look with an RGB table saves as 9, which releases that read 8 refuse.
        let rgb = CameraProfile::creative_for_test(&m).with_test_rgb_tables();
        r.profile = Some(std::sync::Arc::new(rgb[0].clone()));
        assert_eq!(super::saved_version(&r), 9);
        // So does one carrying develop settings.
        let mut look = CameraProfile::creative_for_test(&m);
        look.enhanced.as_mut().unwrap().settings.saturation = -0.2;
        r.profile = Some(std::sync::Arc::new(look));
        assert_eq!(super::saved_version(&r), 9);
        let mut v = serde_json::json!({"schema": 9, "pipeline": 9, "recipe": {}});
        super::migrate_recipe(&mut v).unwrap();
    }
    #[test]
    fn matrix_profile_signature_raises_saved_version_and_round_trips() {
        let m = crate::camera_data::Metadata {
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            dng_matrix_profile_signature: Some("test.profile".into()),
            ..Default::default()
        };
        let r = super::super::recipe::Recipe {
            profile: crate::camera_profiles::open::color(&m).map(std::sync::Arc::new),
            ..Default::default()
        };
        assert!(r.profile.is_some());
        let version = super::saved_version(&r);
        assert_eq!(version, 10);
        let mut saved = serde_json::json!({"schema": version, "pipeline": version, "recipe": r});
        super::migrate_recipe(&mut saved).unwrap();
        let back: super::super::recipe::Recipe =
            serde_json::from_value(saved["recipe"].clone()).unwrap();
        assert_eq!(back, r);
    }

    /// The fields a freshly saved recipe writes, as of schema 6. Releases that read
    /// schema 6 keep fields they don't know, so adding a field is safe only when it
    /// is skipped at its default (`skip_serializing_if`) or when every release that
    /// may still read the file can be shown to tolerate it; either way, changing this
    /// list is a deliberate compatibility decision, not a side effect.
    const SAVED_FIELDS: &[&str] = &[
        "black_point",
        "blacks",
        "camera_exposure",
        "contrast",
        "crop",
        "curve",
        "effects",
        "exposure",
        "flip_x",
        "flip_y",
        "grading",
        "highlights",
        "hsl",
        "lens_builtin",
        "lens_ca",
        "lens_distortion",
        "lens_profile",
        "lens_vignetting",
        "midtone",
        "noise_chroma",
        "noise_luma",
        "preset_name",
        "preset_settings",
        "profile",
        "rotation",
        "saturation",
        "shadows",
        "sharpening",
        "sharpening_detail",
        "sharpening_masking",
        "sharpening_radius",
        "straighten",
        "temperature",
        "tint",
        "transform",
        "vibrance",
        "wb",
        "white_point",
        "whites",
    ];
    /// Spots and masks never enter the saved recipe, which writes exactly the known
    /// fields; and fields from newer releases survive a round trip.
    #[test]
    fn saved_recipes_write_only_the_known_fields() {
        use crate::model::recipe::Recipe;
        let mut r = Recipe::default();
        r.retouch.push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.01,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.05, 0.],
        });
        let (saved, local) = r.split_local();
        let json = serde_json::to_value(&saved).unwrap();
        let written: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            written, SAVED_FIELDS,
            "a saved recipe's fields changed; see SAVED_FIELDS before updating it"
        );
        let mut v = serde_json::json!({"schema": super::SCHEMA, "pipeline": super::PIPELINE, "recipe": json});
        super::migrate_recipe(&mut v).unwrap();
        assert_eq!(v["schema"], 6);
        let back: Recipe = serde_json::from_value(v["recipe"].clone()).unwrap();
        assert_eq!(back.with_local(local), r);
        // A setting from a newer release is kept.
        let newer: Recipe =
            serde_json::from_value(serde_json::json!({"exposure": 0.5, "future_slider": [1, 2]}))
                .unwrap();
        assert_eq!(newer.exposure, 0.5);
        let again = serde_json::to_value(&newer).unwrap();
        assert_eq!(again["future_slider"], serde_json::json!([1, 2]));
        // A development build's schema 7, with spots in the recipe, still loads them.
        let mut v = serde_json::json!({"schema": 7, "pipeline": 7, "recipe": serde_json::to_value(&r).unwrap()});
        super::migrate_recipe(&mut v).unwrap();
        let old: Recipe = serde_json::from_value(v["recipe"].clone()).unwrap();
        assert_eq!(old.retouch, r.retouch);
    }
}
