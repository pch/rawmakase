//! The version a saved recipe is written with, and the migration of older ones.
//! Catalog edits, native presets and legacy sidecars all save recipes this way.
//!
//! This module also owns the settings earlier releases used to choose how a recipe
//! rendered ([`OBSOLETE_SETTINGS`]). Every recipe now renders with the one engine,
//! so they are dropped wherever a recipe enters: reading any `Recipe` (catalog
//! edits, History, snapshots, presets, sidecars, the session) leaves them out of its
//! unknown settings, and the catalog's upgrade removes them from the stored rows.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The version written, for every recipe: the first rendered by the one engine.
/// Releases before it read up to [`BEFORE_ONE_ENGINE`] and refuse anything newer,
/// so they never render a recipe with the operators it no longer chooses.
pub const SCHEMA: u32 = 11;
/// The newest version the releases before the one engine read. Look Amounts,
/// RGB-table looks and matrix-profile signatures were written as 8, 9 and 10.
pub const BEFORE_ONE_ENGINE: u32 = 10;

/// Recipe settings of earlier releases that chose a rendering engine or an
/// operator for a control: the process version, five opt-in flags and the
/// eighteen per-control operator choices. They mean nothing to the one engine and
/// are dropped; every other setting, known or not, is kept.
pub const OBSOLETE_SETTINGS: &[&str] = &[
    "engine",
    "profile_tone",
    "wide_gamut_curves",
    "reference_curves",
    "reference_color",
    "reference_calibration",
    "grain_model",
    "clarity_model",
    "texture_model",
    "sharpening_model",
    "parametric_model",
    "contrast_model",
    "lens_vignette_model",
    "retouch_model",
    "grading_model",
    "mixer_model",
    "saturation_model",
    "vibrance_model",
    "black_white_model",
    "calibration_model",
    "whites_model",
    "white_balance_model",
    "gamut_model",
    "noise_model",
];

/// Whether `key` is one of the [`OBSOLETE_SETTINGS`].
pub fn is_obsolete(key: &str) -> bool {
    OBSOLETE_SETTINGS.contains(&key)
}

/// Removes the [`OBSOLETE_SETTINGS`] from a recipe object; whether it had any.
pub fn drop_obsolete_settings(recipe: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let before = recipe.len();
    recipe.retain(|key, _| !is_obsolete(key));
    recipe.len() != before
}

/// A recipe stored as JSON text without its [`OBSOLETE_SETTINGS`]; `None` when it
/// has none, or is not a JSON object (a damaged edit stays as it is, protected).
pub fn recipe_text_without_obsolete_settings(text: &str) -> Option<String> {
    let serde_json::Value::Object(mut recipe) = serde_json::from_str(text).ok()? else {
        return None;
    };
    drop_obsolete_settings(&mut recipe).then(|| serde_json::Value::Object(recipe).to_string())
}

/// Reads a recipe's unknown settings, leaving out the [`OBSOLETE_SETTINGS`]
/// (`Recipe::unknown`'s deserializer).
pub(crate) fn unknown_settings<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error> {
    let mut unknown = BTreeMap::<String, serde_json::Value>::deserialize(d)?;
    unknown.retain(|key, _| !is_obsolete(key));
    Ok(unknown)
}

/// Whether a reader of versions up to `newest` reads the envelope in `value`: this
/// release with [`SCHEMA`], the releases before the one engine with
/// [`BEFORE_ONE_ENGINE`] (their check, exactly).
pub fn reads(value: &serde_json::Value, newest: u32) -> bool {
    let schema = value["schema"].as_u64();
    // Development builds of the retouch tools wrote 7 with the spots and masks
    // inside the recipe; they load as they are.
    schema.is_some_and(|s| (1..=u64::from(newest)).contains(&s))
        && value["pipeline"].as_u64() == schema
}

/// Checks a saved envelope (`schema`, `pipeline`, `recipe`) is one this release
/// reads and brings its recipe up to date.
pub fn migrate_recipe(value: &mut serde_json::Value) -> Result<()> {
    ensure!(
        reads(value, SCHEMA),
        "Unsupported saved recipe version: preserved without changes"
    );
    let pipeline = value["pipeline"].as_u64();
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
    drop_obsolete_settings(recipe);
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Every version this build reads migrates; recipes keep the settings they
    /// were saved with and render with the one engine.
    #[test]
    fn saved_versions_migrate_without_an_engine() {
        for version in 1..=11 {
            let mut v = serde_json::json!({"schema": version, "pipeline": version, "recipe": {"exposure": 0.5}});
            super::migrate_recipe(&mut v).unwrap();
            assert!(v["recipe"].get("engine").is_none(), "{version}");
            assert_eq!(v["recipe"]["exposure"], 0.5);
        }
        let mut v = serde_json::json!({"schema": 12, "pipeline": 12, "recipe": {}});
        assert!(super::migrate_recipe(&mut v).is_err());
        let mut v = serde_json::json!({"schema": 5, "pipeline": 6, "recipe": {}});
        assert!(super::migrate_recipe(&mut v).is_err());
    }
    /// A recipe saved by an earlier release, with every setting that chose its engine
    /// or an operator: they are dropped, its sliders and a newer release's setting kept.
    fn earlier_recipe() -> serde_json::Value {
        let mut recipe = serde_json::json!({
            "exposure": 0.5,
            "contrast": -0.25,
            "sharpening": 0.4,
            "effects": {"clarity": 0.3, "grain": 0.2},
            "future_slider": [1, 2],
        });
        let fields = recipe.as_object_mut().unwrap();
        for key in super::OBSOLETE_SETTINGS {
            let value = match *key {
                "engine" => serde_json::json!(3),
                "profile_tone"
                | "wide_gamut_curves"
                | "reference_curves"
                | "reference_color"
                | "reference_calibration" => serde_json::json!(true),
                _ => serde_json::json!("Original"),
            };
            fields.insert((*key).into(), value);
        }
        recipe
    }
    #[test]
    fn the_obsolete_settings_are_the_eighteen_operators_and_six_selectors() {
        let models = super::OBSOLETE_SETTINGS
            .iter()
            .filter(|key| key.ends_with("_model"))
            .count();
        assert_eq!((models, super::OBSOLETE_SETTINGS.len()), (18, 24));
        // None of them is a setting this build reads.
        let known = serde_json::to_value(crate::model::recipe::Recipe::default()).unwrap();
        for key in super::OBSOLETE_SETTINGS {
            assert!(known.get(key).is_none(), "{key}");
        }
    }
    #[test]
    fn reading_any_recipe_drops_the_obsolete_settings_and_keeps_the_rest() {
        use crate::model::recipe::Recipe;
        let r: Recipe = serde_json::from_value(earlier_recipe()).unwrap();
        assert_eq!((r.exposure, r.contrast, r.sharpening), (0.5, -0.25, 0.4));
        assert_eq!((r.effects.clarity, r.effects.grain), (0.3, 0.2));
        assert_eq!(
            r.unknown.keys().collect::<Vec<_>>(),
            ["future_slider"],
            "only the newer release's setting is unknown"
        );
        let saved = serde_json::to_value(&r).unwrap();
        for key in super::OBSOLETE_SETTINGS {
            assert!(saved.get(key).is_none(), "{key} written back");
        }
        assert_eq!(saved["future_slider"], serde_json::json!([1, 2]));
        // A saved envelope of any earlier version migrates the same way.
        for version in 1..=super::BEFORE_ONE_ENGINE {
            let mut v = serde_json::json!({"schema": version, "pipeline": version, "recipe": earlier_recipe()});
            super::migrate_recipe(&mut v).unwrap();
            for key in super::OBSOLETE_SETTINGS {
                assert!(v["recipe"].get(key).is_none(), "{version}: {key}");
            }
            assert_eq!(v["recipe"]["future_slider"], serde_json::json!([1, 2]));
            assert_eq!(v["recipe"]["exposure"], 0.5);
        }
    }
    #[test]
    fn stored_recipe_text_loses_only_the_obsolete_settings() {
        let text = earlier_recipe().to_string();
        let migrated = super::recipe_text_without_obsolete_settings(&text).unwrap();
        let mut expected = earlier_recipe();
        assert!(super::drop_obsolete_settings(
            expected.as_object_mut().unwrap()
        ));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&migrated).unwrap(),
            expected
        );
        // Nothing to change, or nothing it can read: left as it is.
        assert_eq!(
            super::recipe_text_without_obsolete_settings(&migrated),
            None
        );
        assert_eq!(
            super::recipe_text_without_obsolete_settings("{not json"),
            None
        );
        assert_eq!(super::recipe_text_without_obsolete_settings("[1]"), None);
    }
    /// Every recipe is written as the first version of the one engine, which the
    /// releases before it refuse as newer.
    #[test]
    fn every_recipe_saves_as_a_version_earlier_releases_refuse() {
        let saved =
            serde_json::json!({"schema": super::SCHEMA, "pipeline": super::SCHEMA, "recipe": {}});
        assert!(super::reads(&saved, super::SCHEMA));
        assert!(!super::reads(&saved, super::BEFORE_ONE_ENGINE));
        let earlier = serde_json::json!({"schema": 10, "pipeline": 10, "recipe": {}});
        assert!(super::reads(&earlier, super::BEFORE_ONE_ENGINE));
    }
    #[test]
    fn matrix_profile_signature_round_trips() {
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
        let mut saved =
            serde_json::json!({"schema": super::SCHEMA, "pipeline": super::SCHEMA, "recipe": r});
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
        let mut v =
            serde_json::json!({"schema": super::SCHEMA, "pipeline": super::SCHEMA, "recipe": json});
        super::migrate_recipe(&mut v).unwrap();
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
