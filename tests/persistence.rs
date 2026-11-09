//! Public API regressions, with relative-path work isolated in a child process.
use anyhow::Result;
use rawmakase::{
    catalog::{Catalog, legacy_sidecar},
    export_settings::ExportOptions,
    model::recipe::Recipe,
    presets, storage,
};
use std::{fs, path::Path, process::Command};

#[test]
fn relative_paths_roundtrip() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let output = Command::new(std::env::current_exe()?)
        .args(["--exact", "relative_paths_child", "--nocapture"])
        .current_dir(dir.path())
        .env("RAWMAKASE_RELATIVE_PATH_TEST", "1")
        .env("RAWMAKASE_DATA_DIR", dir.path().join("data"))
        .output()?;
    // libtest exits 0 when `--exact` matches nothing, so a renamed child would
    // pass without running. Require that it ran.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(" 1 passed;"),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn relative_paths_child() -> Result<()> {
    if std::env::var_os("RAWMAKASE_RELATIVE_PATH_TEST").is_none() {
        return Ok(());
    }
    let recipe = Recipe {
        exposure: 0.75,
        ..Default::default()
    };
    presets::save_preset(Path::new("preset.json"), &recipe, &Default::default())?;
    assert_eq!(
        presets::load_preset(Path::new("preset.json"))?.recipe,
        recipe
    );
    fs::write("photo.ARW", b"identity fixture")?;
    legacy_sidecar::save(Path::new("photo.ARW"), &recipe, &ExportOptions::default())?;
    assert_eq!(
        legacy_sidecar::load(Path::new("photo.ARW"))?
            .unwrap()
            .recipe,
        recipe
    );
    assert_eq!(storage::list_raws(Path::new("photo.ARW"))?.len(), 1);
    let catalog = Catalog::create(Path::new("photos.rawmakase"))?;
    assert!(catalog.photos()?.is_empty());
    drop(catalog);
    assert!(
        Catalog::open(Path::new("photos.rawmakase"))?
            .photos()?
            .is_empty()
    );
    Ok(())
}

#[test]
fn malformed_legacy_recipes_are_errors_and_original_bytes_survive() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let raw = dir.path().join("photo.ARW");
    fs::write(&raw, b"identity fixture")?;
    let preset = dir.path().join("preset.json");
    let sidecar = legacy_sidecar::sidecar_path(&raw);
    for version in [1, 2, 3, 4, 999] {
        for recipe in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!(false),
            serde_json::json!("invalid"),
        ] {
            let bytes = serde_json::to_vec(
                &serde_json::json!({"schema": version, "pipeline": version, "recipe": recipe}),
            )?;
            fs::write(&preset, &bytes)?;
            fs::write(&sidecar, &bytes)?;
            assert!(presets::load_preset(&preset).is_err());
            assert!(legacy_sidecar::load(&raw).is_err());
            assert!(
                legacy_sidecar::save(&raw, &Recipe::default(), &ExportOptions::default()).is_err()
            );
            assert_eq!(fs::read(&sidecar)?, bytes);
        }
    }
    Ok(())
}

#[test]
fn invalid_export_defaults_never_replace_saved_edits() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let raw = dir.path().join("photo.ARW");
    fs::write(&raw, b"identity fixture")?;
    let recipe = Recipe::default();
    let sidecar = legacy_sidecar::save(&raw, &recipe, &ExportOptions::default())?;
    let original = fs::read(&sidecar)?;
    let mut catalog = Catalog::create(&dir.path().join("photos.rawmakase"))?;
    catalog.add_folder(dir.path())?;
    let id = catalog.photos()?[0].id;
    catalog.save_edit(
        id,
        &raw,
        &rawmakase::storage::Identity::read(&raw)?,
        &recipe,
        &ExportOptions::default(),
        rawmakase::catalog::HistoryUpdate::Keep,
    )?;
    for options in [
        ExportOptions {
            quality: 0,
            max_edge: 0,
            ..Default::default()
        },
        ExportOptions {
            quality: 101,
            max_edge: 0,
            ..Default::default()
        },
        ExportOptions {
            quality: 92,
            max_edge: 30_001,
            ..Default::default()
        },
    ] {
        assert!(legacy_sidecar::save(&raw, &recipe, &options).is_err());
        assert_eq!(fs::read(&sidecar)?, original);
        assert!(
            catalog
                .save_edit(
                    id,
                    &raw,
                    &rawmakase::storage::Identity::read(&raw)?,
                    &recipe,
                    &options,
                    rawmakase::catalog::HistoryUpdate::Keep
                )
                .is_err()
        );
        let saved = catalog.load_edit(id, &raw)?.unwrap();
        assert_eq!(saved.export.quality, 92);
        assert_eq!(saved.export.max_edge, 0);
    }
    let rawmakase::catalog::CatalogLocation::File(path) = catalog.location().clone();
    let db = rusqlite::Connection::open(&path)?;
    let corrupt_options = r#"{"quality":0,"max_edge":0}"#;
    db.execute(
        "UPDATE photos SET export_options=? WHERE id=?",
        rusqlite::params![corrupt_options, id.0],
    )?;
    assert!(catalog.load_edit(id, &raw).is_err());
    assert!(
        catalog
            .save_edit(
                id,
                &raw,
                &rawmakase::storage::Identity::read(&raw)?,
                &recipe,
                &ExportOptions::default(),
                rawmakase::catalog::HistoryUpdate::Keep
            )
            .is_err()
    );
    let preserved: String = db.query_row(
        "SELECT export_options FROM photos WHERE id=?",
        [id.0],
        |row| row.get(0),
    )?;
    assert_eq!(preserved, corrupt_options);
    Ok(())
}
