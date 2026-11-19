use super::*;
use crate::camera_data::Metadata;
use crate::lens::choice::{LensProfileChoice, LensProfileSetup};
use crate::lens::lensfun::{LensfunChoice, LensfunLens, Provenance};
use crate::storage::recipe_assets::{AssetError, AssetKind, NoAssets};

/// A catalog's assets in memory.
#[derive(Default)]
struct Assets(HashMap<String, (AssetKind, Vec<u8>)>);
impl Assets {
    fn of(assets: &[Asset]) -> Self {
        Self(
            assets
                .iter()
                .map(|a| (a.id.clone(), (a.kind(), a.compressed())))
                .collect(),
        )
    }
}
impl AssetSource for Assets {
    fn stored(&self, id: &str) -> Result<Option<(AssetKind, Vec<u8>)>, String> {
        Ok(self.0.get(id).cloned())
    }
}

fn profile(seed: f32) -> Arc<CameraProfile> {
    let m = Metadata {
        cam_xyz: [[0.9, -0.2, seed], [-0.4, 1.2, 0.15], [-0.05, 0.1, 0.6]],
        ..Default::default()
    };
    Arc::new(CameraProfile::camera_matrix_default(&m).unwrap())
}
fn calibration(crop: f32) -> Arc<Calibration> {
    Arc::new(Calibration {
        crop,
        aspect: 1.5,
        focal: [24., 70.],
        ..Default::default()
    })
}
fn edit(seed: f32) -> Recipe {
    Recipe {
        profile: Some(profile(seed)),
        exposure: 0.25,
        lens_profile_choice: LensProfileChoice {
            setup: LensProfileSetup::Custom,
            id: Some(LensProfileRef::Lensfun(Box::new(LensfunChoice {
                lens: LensfunLens {
                    maker: "Maker".into(),
                    model: "Zoom".into(),
                    crop: 1.5,
                    aspect: 1.5,
                    ..Default::default()
                },
                label: "Maker Zoom".into(),
                camera: None,
                calibration: Some(calibration(1.5)),
                provenance: Provenance::lensfun(1),
            }))),
        },
        ..Recipe::default()
    }
}

/// The stored form names the profile and the calibration; read back with the
/// catalog's assets it is the same recipe.
#[test]
fn a_stored_recipe_names_its_profile_and_calibration_and_reads_back() {
    let r = edit(0.021);
    let (stored, assets) = to_stored(&r);
    let stored: serde_json::Value = serde_json::from_str(&stored.to_text()).unwrap();
    assert_eq!(assets.len(), 2);
    let profile_id = stored["profile"].as_str().unwrap();
    let calibration_id = stored
        .pointer("/lens_profile_choice/id/lensfun/calibration")
        .and_then(serde_json::Value::as_str)
        .unwrap();
    assert!(
        assets
            .iter()
            .any(|a| a.id == profile_id && a.kind() == AssetKind::Profile)
    );
    assert!(
        assets
            .iter()
            .any(|a| a.id == calibration_id && a.kind() == AssetKind::Calibration)
    );
    let text = stored.to_string();
    assert!(text.len() < 4096, "{} bytes", text.len());
    let back = from_text(&Assets::of(&assets), &text).unwrap();
    assert_eq!(back, r);
    // Read without them (and past the cache, which now holds them), the reference
    // is reported, not dropped.
    let error =
        recipe_assets::reading_uncached(&NoAssets, &text, || serde_json::from_str::<Recipe>(&text))
            .unwrap_err()
            .to_string();
    // The lens choice is read before the profile.
    let missing = AssetError::Missing(AssetKind::Calibration, calibration_id.into());
    assert!(error.contains(&missing.to_string()), "{error}");
    let mut without_lens = stored.clone();
    without_lens["lens_profile_choice"] = serde_json::json!({});
    let text = without_lens.to_string();
    let error =
        recipe_assets::reading_uncached(&NoAssets, &text, || serde_json::from_str::<Recipe>(&text))
            .unwrap_err()
            .to_string();
    let missing = AssetError::Missing(AssetKind::Profile, profile_id.into());
    assert!(error.contains(&missing.to_string()), "{error}");
    // A recipe with neither stores neither.
    let (plain, none) = to_stored(&Recipe::default());
    assert!(none.is_empty());
    assert_eq!(
        plain.to_text(),
        serde_json::to_string(&Recipe::default()).unwrap()
    );
}

/// Everything outside a catalog keeps writing the profile inline, and an inline
/// recipe reads as before, with or without a catalog in scope.
#[test]
fn inline_recipes_are_written_and_read_as_before() {
    let r = edit(0.022);
    let inline = serde_json::to_value(&r).unwrap();
    assert!(inline["profile"].is_object());
    assert!(
        inline
            .pointer("/lens_profile_choice/id/lensfun/calibration")
            .unwrap()
            .is_object()
    );
    let text = inline.to_string();
    assert_eq!(serde_json::from_str::<Recipe>(&text).unwrap(), r);
    assert_eq!(from_text(&NoAssets, &text).unwrap(), r);
}

/// The inline path stays strict: a profile with a duplicate or unknown field is
/// refused, as before.
#[test]
fn an_inline_profile_is_read_strictly() {
    let mut inline = serde_json::to_value(edit(0.023)).unwrap();
    inline["profile"]["not_a_profile_field"] = 1.into();
    assert!(serde_json::from_value::<Recipe>(inline).is_err());
    let text = serde_json::to_string(&edit(0.023)).unwrap();
    let duplicated = text.replacen("\"profile\":{", "\"profile\":{\"name\":\"x\",", 1);
    assert_ne!(duplicated, text);
    let error = serde_json::from_str::<Recipe>(&duplicated).unwrap_err();
    assert!(error.to_string().contains("duplicate field"), "{error}");
    let mut wrong = serde_json::to_value(edit(0.023)).unwrap();
    wrong["profile"] = 5.into();
    assert!(serde_json::from_value::<Recipe>(wrong).is_err());
}

/// A History of many states holding one profile hashes and stores it once.
#[test]
fn one_write_stores_a_shared_profile_once() {
    let r = edit(0.024);
    let mut storer = Storer::new();
    let states: Vec<_> = (0..50)
        .map(|i| {
            let mut s = r.clone();
            s.exposure = i as f32 / 10.;
            storer.to_stored(&s)
        })
        .collect();
    assert_eq!(storer.assets().count(), 2);
    let profile = |s: &RawObject| s.get("profile").unwrap().get().to_string();
    assert!(states.windows(2).all(|w| profile(&w[0]) == profile(&w[1])));
    // An equal profile in another Arc is the same asset.
    let mut other = r.clone();
    other.profile = Some(Arc::new((**r.profile.as_ref().unwrap()).clone()));
    storer.to_stored(&other);
    assert_eq!(storer.assets().count(), 2);
}

/// Edits naming one profile share one value in memory.
#[test]
fn loaded_edits_share_their_profile() {
    let (stored, assets) = to_stored(&edit(0.025));
    let catalog = Assets::of(&assets);
    let text = stored.to_text();
    let loaded: Vec<Recipe> = (0..4)
        .map(|_| from_text(&catalog, &text).unwrap())
        .collect();
    let first = loaded[0].profile.as_ref().unwrap();
    assert!(
        loaded
            .iter()
            .all(|r| Arc::ptr_eq(r.profile.as_ref().unwrap(), first))
    );
}

/// An edit's stored text is the same whichever form it was read from, so it can
/// identify the edit (a preview's tag).
#[test]
fn the_stored_text_is_the_same_from_either_form() {
    let r = edit(0.026);
    let (stored, assets) = to_stored(&r);
    let from_stored = from_text(&Assets::of(&assets), &stored.to_text()).unwrap();
    let from_inline: Recipe = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(to_text(&from_stored), to_text(&from_inline));
    assert_eq!(to_text(&from_stored), stored.to_text());
    // Settings keep the text a recipe writes them with.
    assert!(
        stored.to_text().contains("\"exposure\":0.25"),
        "{}",
        stored.to_text()
    );
    let mut changed = from_inline.clone();
    changed.profile = Some(profile(0.027));
    assert_ne!(to_text(&changed), to_text(&from_inline));
}

/// A look with known Lightroom settings is stored with them, once, named under
/// `look_source`; reading drops the reference from the recipe, and the edit's
/// identity leaves it out.
#[test]
fn a_looks_lightroom_settings_are_stored_beside_its_profile() {
    use crate::camera_profiles::look_source::{self, testing};
    let look = Arc::new(testing::look_profile(
        &testing::look_xmp(
            "Stored look",
            "66600000000000000000000000000010",
            "Someone",
            "128, 170",
        ),
        &profile(0.1),
    ));
    let source = look_source::find(&look).unwrap();
    let r = Recipe {
        profile: Some(look.clone()),
        ..Recipe::default()
    };
    let (stored, assets) = to_stored(&r);
    let source_asset = Asset::look_source(&source);
    let named = stored.get(LOOK_SOURCE).expect("the reference is stored");
    assert_eq!(named.get(), format!("\"{}\"", source_asset.id));
    assert!(assets.iter().any(|a| a.id == source_asset.id));
    // Read back: the same recipe, without the reference among its unknown settings.
    let read = from_text(&Assets::of(&assets), &stored.to_text()).unwrap();
    assert_eq!(read, r);
    assert!(!read.unknown.contains_key(LOOK_SOURCE));
    // What identifies the edit doesn't depend on whether its look's settings are known.
    assert!(!to_text(&r).contains(LOOK_SOURCE));
    let unknown = Arc::new({
        let mut p = (*look).clone();
        p.enhanced.as_mut().unwrap().uuid = "66600000000000000000000000000011".into();
        p
    });
    let without = Recipe {
        profile: Some(unknown),
        ..Recipe::default()
    };
    assert!(to_stored(&without).0.get(LOOK_SOURCE).is_none());
    // A stale reference kept from another release's unknown settings is not kept.
    let mut stale = without;
    stale
        .unknown
        .insert(LOOK_SOURCE.into(), serde_json::json!(source_asset.id));
    assert!(to_stored(&stale).0.get(LOOK_SOURCE).is_none());
}
