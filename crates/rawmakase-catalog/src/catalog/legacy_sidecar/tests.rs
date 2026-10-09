use super::*;
use crate::presets::{load_preset, save_preset};
#[test]
fn legacy_sidecar_and_preset_migrate_without_curve_changes() -> Result<()> {
    let d = tempfile::tempdir()?;
    let raw = d.path().join("legacy.ARW");
    fs::write(&raw, b"fixture")?;
    let store = d.path().join("store");
    let p = save_at(&raw, &Recipe::default(), &ExportOptions::default(), &store)?;
    let mut v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    v["schema"] = 1.into();
    v["pipeline"] = 1.into();
    v["recipe"]["curve"] = serde_json::json!([0.1, 0.2, 0.5, 0.9, 1.]);
    fs::write(&p, serde_json::to_vec(&v)?)?;
    let loaded = load_at(&raw, &store)?.unwrap();
    assert!(!loaded.recipe.curve.smooth);
    save_at(&raw, &loaded.recipe, &loaded.export, &store)?;
    assert_eq!(load_at(&raw, &store)?.unwrap().recipe, loaded.recipe);
    let preset = d.path().join("preset.json");
    fs::write(
        &preset,
        serde_json::to_vec(&serde_json::json!({"schema":1,"pipeline":1,"recipe":v["recipe"]}))?,
    )?;
    assert_eq!(load_preset(&preset)?, loaded.recipe);
    save_preset(&preset, &loaded.recipe)?;
    assert_eq!(load_preset(&preset)?, loaded.recipe);
    Ok(())
}
// Unix directory permissions; Windows has no read-only folders in this sense.
#[cfg(unix)]
#[test]
fn readonly_folder_uses_fallback_and_restores() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir()?;
    let store = tempfile::tempdir()?;
    let p = dir.path().join("photo.ARW");
    fs::write(&p, b"raw")?;
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555))?;
    // Root (as in CI containers) ignores directory permissions; nothing to test then.
    if fs::write(dir.path().join("probe"), b"").is_ok() {
        return Ok(());
    }
    let result = save_at(
        &p,
        &Recipe::default(),
        &ExportOptions::default(),
        store.path(),
    );
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755))?;
    let output = result?;
    assert!(output.starts_with(store.path()));
    assert!(load_at(&p, store.path())?.is_some());
    Ok(())
}
#[test]
fn sidecar_roundtrip_and_protection() -> Result<()> {
    let d = tempfile::tempdir()?;
    let raw = d.path().join("test.ARW");
    fs::write(&raw, b"fixture")?;
    let r = Recipe {
        exposure: 1.25,
        ..Default::default()
    };
    let p = save(&raw, &r, &ExportOptions::default())?;
    assert_eq!(load(&raw)?.unwrap().recipe, r);
    assert!(p.ends_with("test.ARW.rawmakase.json"));
    let mut v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    v["schema"] = 999.into();
    fs::write(&p, serde_json::to_vec(&v)?)?;
    assert!(save(&raw, &r, &ExportOptions::default()).is_err());
    assert_eq!(
        serde_json::from_reader::<_, serde_json::Value>(File::open(&p)?)?["schema"],
        999
    );
    Ok(())
}
/// A sidecar is saved as the one engine's version, which releases before it refuse
/// as newer; one an earlier release saved loses the settings that chose its engine
/// and operators, and keeps its sliders and a newer release's setting.
#[test]
fn sidecars_migrate_to_the_one_engine_and_earlier_releases_refuse_new_ones() -> Result<()> {
    use crate::model::saved_format::{BEFORE_ONE_ENGINE, reads};
    let d = tempfile::tempdir()?;
    let raw = d.path().join("photo.ARW");
    fs::write(&raw, b"fixture")?;
    let store = d.path().join("store");
    let p = save_at(&raw, &Recipe::default(), &ExportOptions::default(), &store)?;
    let mut v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    assert!(!reads(&v, BEFORE_ONE_ENGINE));
    v["schema"] = 10.into();
    v["pipeline"] = 10.into();
    let recipe = v["recipe"].as_object_mut().unwrap();
    recipe.insert("exposure".into(), 0.75.into());
    recipe.insert("engine".into(), 3.into());
    recipe.insert("reference_color".into(), true.into());
    recipe.insert("sharpening_model".into(), "Original".into());
    recipe.insert("future_slider".into(), serde_json::json!([1]));
    fs::write(&p, serde_json::to_vec(&v)?)?;
    let loaded = load_at(&raw, &store)?.unwrap();
    assert_eq!(loaded.recipe.exposure, 0.75);
    assert_eq!(
        loaded.recipe.unknown,
        [("future_slider".to_string(), serde_json::json!([1]))].into()
    );
    save_at(&raw, &loaded.recipe, &loaded.export, &store)?;
    let v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    assert!(!reads(&v, BEFORE_ONE_ENGINE));
    for key in crate::model::saved_format::OBSOLETE_SETTINGS {
        assert!(v["recipe"].get(key).is_none(), "{key}");
    }
    assert_eq!(v["recipe"]["future_slider"], serde_json::json!([1]));
    Ok(())
}
#[test]
fn changed_source_refused() -> Result<()> {
    let d = tempfile::tempdir()?;
    let p = d.path().join("x.RAF");
    fs::write(&p, b"one")?;
    save(&p, &Recipe::default(), &ExportOptions::default())?;
    fs::write(&p, b"two longer")?;
    assert!(load(&p).is_err());
    Ok(())
}
/// Spots and masks go to the companion file, so the sidecar itself stays a recipe
/// without them; they load back into the recipe.
#[test]
fn spots_and_masks_save_beside_a_compatible_sidecar() -> Result<()> {
    let d = tempfile::tempdir()?;
    let raw = d.path().join("photo.ARW");
    fs::write(&raw, b"fixture")?;
    let store = d.path().join("store");
    let mut r = Recipe {
        exposure: 0.4,
        ..Default::default()
    };
    r.retouch.push(crate::model::retouch::RetouchOp {
        mode: crate::model::retouch::RetouchMode::Clone,
        shape: crate::model::retouch::RetouchShape::Spot {
            center: [0.4, 0.5],
            radius: 0.02,
        },
        feather: 0.5,
        opacity: 1.,
        offset: [0.1, 0.],
    });
    r.masks.push(crate::model::masks::MaskGroup {
        components: vec![crate::model::masks::MaskComponent::new(
            crate::model::masks::MaskShape::Linear {
                from: [0.5, 0.],
                to: [0.5, 0.5],
            },
        )],
        adjust: crate::model::masks::LocalAdjust {
            exposure: -1.,
            ..Default::default()
        },
        ..Default::default()
    });
    r.red_eye.push(crate::model::red_eye::RedEyeOp {
        kind: Default::default(),
        center: [0.3, 0.4],
        radius: [0.01, 0.012],
        correlation: -0.2,
        pupil_size: 0.6,
        darken: 0.4,
    });
    let p = save_at(&raw, &r, &ExportOptions::default(), &store)?;
    let v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    assert_eq!(
        (v["schema"].as_u64(), v["pipeline"].as_u64()),
        (Some(11), Some(11))
    );
    let recipe = v["recipe"].as_object().unwrap();
    assert!(
        !recipe.contains_key("retouch")
            && !recipe.contains_key("masks")
            && !recipe.contains_key("red_eye")
    );
    assert_eq!(recipe["exposure"], 0.4);
    assert!(local_path(&raw).exists());
    assert_eq!(load_at(&raw, &store)?.unwrap().recipe, r);
    // Unknown top-level fields of a newer release survive a save.
    let mut v = v;
    v["future"] = serde_json::json!({"x": 1});
    fs::write(&p, serde_json::to_vec(&v)?)?;
    let loaded = load_at(&raw, &store)?.unwrap();
    save_at(&raw, &loaded.recipe, &loaded.export, &store)?;
    let v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    assert_eq!(v["future"]["x"], 1);
    // Without spots and masks the companion goes away.
    save_at(&raw, &Recipe::default(), &ExportOptions::default(), &store)?;
    assert!(!local_path(&raw).exists());
    // A development build's schema 7 sidecar, with them in the recipe, still loads.
    let mut v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    v["schema"] = 7.into();
    v["pipeline"] = 7.into();
    v["recipe"] = serde_json::to_value(&r)?;
    fs::write(&p, serde_json::to_vec(&v)?)?;
    assert_eq!(load_at(&raw, &store)?.unwrap().recipe, r);
    Ok(())
}
/// The imported edit and its bitmaps come from the same store: the newer one.
#[test]
fn import_takes_the_edit_and_its_bitmaps_from_one_store() -> Result<()> {
    use crate::storage::bitmaps::{Bitmap, to_base64};
    let d = tempfile::tempdir()?;
    let raw = d.path().join("photo.ARW");
    fs::write(&raw, b"fixture")?;
    let store = d.path().join("store");
    let id = Identity::read(&raw)?;
    let bitmap = |value: u8| Bitmap {
        width: 2,
        height: 1,
        channels: 1,
        depth: 1,
        data: vec![value; 2],
    };
    let companion = |path: &Path, bitmap: &Bitmap| -> Result<()> {
        atomic_json(
            path,
            &Companion {
                schema: COMPANION_SCHEMA,
                source: id.clone(),
                local: Default::default(),
                bitmaps: [(bitmap.hash(), to_base64(&bitmap.compress()?))].into(),
            },
        )
    };
    let edit = |exposure| Recipe {
        exposure,
        ..Default::default()
    };
    // Beside the photo, older.
    let primary = save_at(&raw, &edit(0.1), &ExportOptions::default(), &store)?;
    companion(&local_path(&raw), &bitmap(1))?;
    // In the fallback store, newer.
    let backup = fallback_at(&id, &store, "json");
    let mut sidecar: serde_json::Value = serde_json::from_reader(File::open(&primary)?)?;
    sidecar["recipe"]["exposure"] = 0.9.into();
    atomic_json(&backup, &sidecar)?;
    companion(&fallback_at(&id, &store, "local.json"), &bitmap(2))?;
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    File::options()
        .write(true)
        .open(&primary)?
        .set_modified(old)?;
    let (imported, bitmaps) = import_at(&raw, &store)?.unwrap();
    assert_eq!(imported.recipe.exposure, 0.9);
    assert_eq!(bitmaps, [bitmap(2)]);
    assert_eq!(load_at(&raw, &store)?.unwrap().recipe, imported.recipe);
    Ok(())
}
