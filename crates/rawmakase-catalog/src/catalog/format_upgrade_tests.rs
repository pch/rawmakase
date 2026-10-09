//! A catalog of an earlier format opens as it is, and is upgraded, with a backup,
//! before a recipe is stored in it: its stored recipes lose the settings that chose
//! an engine or operator, and releases before the one engine refuse it.
use super::db::{BEFORE_ONE_ENGINE, VERSION, opens};
use super::*;
use crate::model::saved_format::OBSOLETE_SETTINGS;
use crate::{export_settings::ExportOptions, model::recipe::Recipe};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use rusqlite::{Connection, OptionalExtension};
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};

/// What an earlier release stored: sliders, a newer release's setting and every
/// obsolete one.
fn earlier_fields(exposure: f64) -> serde_json::Map<String, serde_json::Value> {
    let mut fields = serde_json::Map::new();
    fields.insert("exposure".into(), exposure.into());
    fields.insert("future_slider".into(), serde_json::json!([1, 2]));
    for key in OBSOLETE_SETTINGS {
        fields.insert((*key).into(), serde_json::json!("Original"));
    }
    fields.insert("engine".into(), 3.into());
    fields
}
fn compress(value: &serde_json::Value) -> Result<Vec<u8>> {
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(value.to_string().as_bytes())?;
    Ok(z.finish()?)
}
fn decompress(data: &[u8]) -> Result<serde_json::Value> {
    let mut text = String::new();
    ZlibDecoder::new(data).read_to_string(&mut text)?;
    Ok(serde_json::from_str(&text)?)
}

struct Stored {
    recipe: String,
    snapshot: String,
    history: Vec<u8>,
}
fn stored(db: &Connection, id: PhotoId) -> Result<Stored> {
    Ok(Stored {
        recipe: db.query_row("SELECT recipe FROM photos WHERE id=?1", [id.0], |r| {
            r.get(0)
        })?,
        snapshot: db.query_row(
            "SELECT recipe FROM develop_snapshots WHERE photo=?1",
            [id.0],
            |r| r.get(0),
        )?,
        history: db.query_row(
            "SELECT data FROM develop_history WHERE photo=?1",
            [id.0],
            |r| r.get(0),
        )?,
    })
}

/// A catalog of the second format whose photo has an edit, a History and a
/// snapshot as an earlier release stored them, closed; and the photo's file.
fn earlier_catalog() -> Result<(tempfile::TempDir, PathBuf, PathBuf, PhotoId)> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let chart =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng");
    let file = photos.join("a.dng");
    std::fs::copy(chart, &file)?;
    let path = dir.path().join("Earlier.rawmakase");
    let mut cat = Catalog::create(&path)?;
    cat.add_folder(&photos)?;
    let id = cat.photos()?[0].id;
    // Saved for real, so the edit's identity is the file's; then rewritten as an
    // earlier release stored it.
    let history = SavedHistory {
        origin: Recipe::default(),
        steps: vec![SavedStep {
            name: "Exposure".into(),
            value: "+0.50".into(),
            recipe: Recipe::default(),
        }],
        applied: 1,
    };
    cat.save_edit(
        id,
        &file,
        &Recipe::default(),
        &ExportOptions::default(),
        HistoryUpdate::Replace(&history),
    )?;
    cat.add_snapshot(id, "Before", &Recipe::default())?;
    let db = cat.db_for_tests();
    let recipe = serde_json::Value::Object(earlier_fields(0.5)).to_string();
    let snapshot = serde_json::Value::Object(earlier_fields(0.25)).to_string();
    let history = compress(&serde_json::json!({
        "version": 1,
        "pool": [],
        "origin": {"fields": earlier_fields(0.)},
        "steps": [{"name": "Exposure", "value": "+0.50", "state": {"fields": earlier_fields(0.5)}}],
        "applied": 1,
    }))?;
    db.execute(
        "UPDATE photos SET recipe=?1 WHERE id=?2",
        rusqlite::params![recipe, id.0],
    )?;
    db.execute(
        "UPDATE develop_snapshots SET recipe=?1 WHERE photo=?2",
        rusqlite::params![snapshot, id.0],
    )?;
    db.execute(
        "UPDATE develop_history SET data=?1 WHERE photo=?2",
        rusqlite::params![history, id.0],
    )?;
    db.execute_batch("PRAGMA user_version=2")?;
    drop(cat);
    Ok((dir, path, file, id))
}
fn version(path: &Path) -> Result<i64> {
    Ok(Connection::open(path)?.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}
fn backups(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().contains("before-upgrade-backup"))
        .collect();
    found.sort();
    Ok(found)
}
fn has_obsolete(fields: &serde_json::Value) -> bool {
    OBSOLETE_SETTINGS
        .iter()
        .any(|key| fields.get(key).is_some())
}

#[test]
fn an_earlier_catalog_reads_without_the_obsolete_settings_and_stays_as_it_was() -> Result<()> {
    let (dir, path, file, id) = earlier_catalog()?;
    let before = stored(&Connection::open(&path)?, id)?;
    let cat = Catalog::open(&path)?;
    // Every entry point reads the recipe without them, keeping the rest.
    let edit = cat.load_edit(id, &file)?.unwrap().recipe;
    assert_eq!(edit.exposure, 0.5);
    assert_eq!(edit.unknown.keys().collect::<Vec<_>>(), ["future_slider"]);
    let history = cat.load_history(id)?.unwrap();
    assert_eq!(history.steps[0].recipe.exposure, 0.5);
    for r in [&history.origin, &history.steps[0].recipe] {
        assert_eq!(r.unknown.keys().collect::<Vec<_>>(), ["future_slider"]);
    }
    let SnapshotSettings::Recipe(snapshot) = &cat.snapshots(id)?[0].settings else {
        panic!("a recipe snapshot");
    };
    assert_eq!(snapshot.exposure, 0.25);
    assert_eq!(
        snapshot.unknown.keys().collect::<Vec<_>>(),
        ["future_slider"]
    );
    // Reading changes nothing: an earlier release still opens it.
    drop(cat);
    let after = stored(&Connection::open(&path)?, id)?;
    assert_eq!(
        (after.recipe, after.snapshot, after.history),
        (before.recipe, before.snapshot, before.history)
    );
    assert_eq!(version(&path)?, 2);
    assert!(backups(dir.path())?.is_empty());
    Ok(())
}

#[test]
fn storing_an_edit_upgrades_the_catalog_first_and_earlier_releases_refuse_it() -> Result<()> {
    let (dir, path, file, id) = earlier_catalog()?;
    let before = stored(&Connection::open(&path)?, id)?;
    let mut cat = Catalog::open(&path)?;
    let mut edit = cat.load_edit(id, &file)?.unwrap();
    edit.recipe.contrast = 0.1;
    cat.save_edit(id, &file, &edit.recipe, &edit.export, HistoryUpdate::Keep)?;
    drop(cat);
    assert_eq!(version(&path)?, VERSION);
    // A release before the one engine refuses the upgraded catalog...
    assert!(!opens(version(&path)?, BEFORE_ONE_ENGINE));
    // ...and opens the backup, which is the catalog as it was.
    let [backup] = backups(dir.path())?.try_into().unwrap();
    assert!(opens(version(&backup)?, BEFORE_ONE_ENGINE));
    let kept = stored(&Connection::open(&backup)?, id)?;
    assert_eq!(
        (kept.recipe, kept.snapshot, kept.history),
        (before.recipe, before.snapshot, before.history)
    );
    // Every stored recipe lost the obsolete settings and kept the rest.
    let after = stored(&Connection::open(&path)?, id)?;
    let recipe: serde_json::Value = serde_json::from_str(&after.recipe)?;
    assert!(!has_obsolete(&recipe));
    assert_eq!(
        (recipe["contrast"].as_f64(), recipe["exposure"].as_f64()),
        (Some(0.1), Some(0.5))
    );
    assert_eq!(recipe["future_slider"], serde_json::json!([1, 2]));
    let snapshot: serde_json::Value = serde_json::from_str(&after.snapshot)?;
    assert!(!has_obsolete(&snapshot));
    assert_eq!(
        (snapshot["exposure"].as_f64(), &snapshot["future_slider"]),
        (Some(0.25), &serde_json::json!([1, 2]))
    );
    let history = decompress(&after.history)?;
    for fields in [
        &history["origin"]["fields"],
        &history["steps"][0]["state"]["fields"],
    ] {
        assert!(!has_obsolete(fields), "{fields}");
        assert_eq!(fields["future_slider"], serde_json::json!([1, 2]));
    }
    assert_eq!(history["steps"][0]["state"]["fields"]["exposure"], 0.5);
    let cat = Catalog::open(&path)?;
    assert_eq!(cat.load_history(id)?.unwrap().steps.len(), 1);
    Ok(())
}

#[test]
fn storing_a_snapshot_upgrades_the_catalog_first() -> Result<()> {
    let (dir, path, _file, id) = earlier_catalog()?;
    let mut cat = Catalog::open(&path)?;
    cat.add_snapshot(id, "After", &Recipe::default())?;
    assert_eq!(cat.format_version()?, VERSION);
    assert_eq!(backups(dir.path())?.len(), 1);
    Ok(())
}

/// A failure inside the upgrade leaves the catalog and the backup as they were,
/// and the edit is not stored.
#[test]
fn a_failed_upgrade_leaves_the_catalog_and_its_backup_intact() -> Result<()> {
    let (dir, path, file, id) = earlier_catalog()?;
    let before = stored(&Connection::open(&path)?, id)?;
    let mut cat = Catalog::open(&path)?;
    // History is rewritten after the edits: fail there, midway.
    cat.db_for_tests().execute_batch(
        "CREATE TEMP TRIGGER fail_history BEFORE UPDATE ON develop_history
         BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
    )?;
    let mut edit = cat.load_edit(id, &file)?.unwrap();
    edit.recipe.contrast = 0.1;
    let error = cat
        .save_edit(id, &file, &edit.recipe, &edit.export, HistoryUpdate::Keep)
        .unwrap_err();
    assert!(format!("{error:#}").contains("disk full"), "{error:#}");
    assert_eq!(cat.format_version()?, 2);
    drop(cat);
    let after = stored(&Connection::open(&path)?, id)?;
    assert_eq!(
        (after.recipe, after.snapshot, after.history),
        (
            before.recipe.clone(),
            before.snapshot.clone(),
            before.history.clone()
        )
    );
    assert_eq!(version(&path)?, 2);
    let [backup] = backups(dir.path())?.try_into().unwrap();
    let copy = Connection::open(&backup)?;
    assert_eq!(
        copy.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))?,
        "ok"
    );
    let kept = stored(&copy, id)?;
    assert_eq!(
        (kept.recipe, kept.snapshot, kept.history),
        (before.recipe, before.snapshot, before.history)
    );
    assert_eq!(version(&backup)?, 2);
    // Without the failure, the next save upgrades it, beside the first backup.
    let mut cat = Catalog::open(&path)?;
    cat.save_edit(id, &file, &edit.recipe, &edit.export, HistoryUpdate::Keep)?;
    assert_eq!(cat.format_version()?, VERSION);
    assert_eq!(backups(dir.path())?.len(), 2);
    assert!(
        copy.query_row("SELECT 1 FROM photos WHERE id=?1", [id.0], |r| r
            .get::<_, i64>(0))
            .optional()?
            .is_some()
    );
    Ok(())
}

/// A save or snapshot that fails its checks, or fails as it writes, leaves an
/// earlier catalog as it was: the upgrade belongs to the write that needs it.
#[test]
fn a_failed_save_or_snapshot_does_not_upgrade_the_catalog() -> Result<()> {
    let (dir, path, file, id) = earlier_catalog()?;
    let before = stored(&Connection::open(&path)?, id)?;
    let mut cat = Catalog::open(&path)?;
    let edit = cat.load_edit(id, &file)?.unwrap();
    // Checks first: a missing file, a changed one, a snapshot without a name.
    let missing = dir.path().join("photos/missing.dng");
    assert!(
        cat.save_edit(
            id,
            &missing,
            &edit.recipe,
            &edit.export,
            HistoryUpdate::Keep
        )
        .is_err()
    );
    let changed = dir.path().join("photos/changed.dng");
    std::fs::write(&changed, b"another file")?;
    assert!(
        cat.save_edit(
            id,
            &changed,
            &edit.recipe,
            &edit.export,
            HistoryUpdate::Keep
        )
        .is_err()
    );
    assert!(cat.add_snapshot(id, "  ", &edit.recipe).is_err());
    assert_eq!(cat.format_version()?, 2);
    assert!(backups(dir.path())?.is_empty());
    // A write that fails: the upgrade rolls back with it.
    assert!(cat.update_snapshot(i64::MAX, &edit.recipe).is_err());
    assert_eq!(cat.format_version()?, 2);
    drop(cat);
    let after = stored(&Connection::open(&path)?, id)?;
    assert_eq!(
        (after.recipe, after.snapshot, after.history),
        (before.recipe, before.snapshot, before.history)
    );
    Ok(())
}

/// The upgrade removes the obsolete settings from History's recipes and nothing
/// else: fields of a later History format stay where they were.
#[test]
fn the_upgrade_keeps_what_it_does_not_know_in_a_history() -> Result<()> {
    let (_dir, path, file, id) = earlier_catalog()?;
    let mut origin = earlier_fields(0.);
    origin.insert("big_setting".into(), 0.into());
    let history = serde_json::json!({
        "version": 1,
        "pool": [{"long": "value"}],
        "origin": {"fields": origin, "pooled": {"big_setting": 0}, "state_extra": 1},
        "steps": [{"name": "Exposure", "value": "+0.50", "state": {"fields": earlier_fields(0.5)},
                   "step_extra": [1]}],
        "applied": 1,
        "history_extra": {"a": true},
    });
    let db = Connection::open(&path)?;
    db.execute(
        "UPDATE develop_history SET data=?1 WHERE photo=?2",
        rusqlite::params![compress(&history)?, id.0],
    )?;
    drop(db);
    let mut cat = Catalog::open(&path)?;
    let edit = cat.load_edit(id, &file)?.unwrap();
    cat.save_edit(id, &file, &edit.recipe, &edit.export, HistoryUpdate::Keep)?;
    drop(cat);
    let after = decompress(&stored(&Connection::open(&path)?, id)?.history)?;
    let mut expected = history;
    for pointer in ["/origin/fields", "/steps/0/state/fields"] {
        let fields = expected
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap();
        crate::model::saved_format::drop_obsolete_settings(fields);
    }
    assert_eq!(after, expected);
    Ok(())
}

/// A damaged edit (here a setting given twice) is not one the upgrade reads, so it
/// stays as it was, still refused, rather than made readable by the rewrite.
#[test]
fn the_upgrade_leaves_a_damaged_edit_as_it_was() -> Result<()> {
    let (_dir, path, file, id) = earlier_catalog()?;
    let damaged = r#"{"engine":3,"exposure":0.5,"exposure":1.5}"#;
    // A History giving a setting twice: reading collapses it, so it isn't rewritten.
    let history = format!(
        r#"{{"version":1,"pool":[],"origin":{{"fields":{damaged}}},"steps":[],"applied":0}}"#
    );
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(history.as_bytes())?;
    let history = z.finish()?;
    let db = Connection::open(&path)?;
    db.execute(
        "UPDATE develop_snapshots SET recipe=?1 WHERE photo=?2",
        rusqlite::params![damaged, id.0],
    )?;
    db.execute(
        "UPDATE develop_history SET data=?1 WHERE photo=?2",
        rusqlite::params![history, id.0],
    )?;
    drop(db);
    let mut cat = Catalog::open(&path)?;
    let edit = cat.load_edit(id, &file)?.unwrap();
    cat.save_edit(id, &file, &edit.recipe, &edit.export, HistoryUpdate::Keep)?;
    assert_eq!(cat.format_version()?, VERSION);
    drop(cat);
    let after = stored(&Connection::open(&path)?, id)?;
    assert_eq!((after.snapshot.as_str(), after.history), (damaged, history));
    Ok(())
}
