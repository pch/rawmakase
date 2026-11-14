//! A catalog of an earlier format is upgraded as it opens: a verified backup beside
//! it, one transaction that stores each camera profile and Lensfun calibration once
//! and has every stored recipe name them (and drops the obsolete settings and the
//! copy of applied presets' settings), then `VACUUM`. Every stored recipe reads back
//! as it did; a failure leaves the file as it was and the catalog unopened.
use super::db::{NEWEST, ONE_ENGINE, PROFILE_ASSETS, faults::FAIL_BACKUP, opens};
use super::format_upgrade::faults::{DAMAGE_ASSETS, DROP_ASSETS, FAIL_AFTER_ROWS, MOST_VERIFIED};
use super::upgrade_fixtures::{Earlier, compress, decompress, inline, inline_history};
use super::*;
use crate::camera_data::{Metadata, Source};
use crate::camera_profiles::{CameraProfile, fitted};
use crate::model::saved_format::OBSOLETE_SETTINGS;
use crate::storage::recipe_assets::{self, Asset};
use crate::{export_settings::ExportOptions, model::recipe::Recipe};
use rusqlite::Connection;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn camera(seed: f32) -> Metadata {
    Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.9, -0.2, seed], [-0.4, 1.2, 0.15], [-0.05, 0.1, 0.6]],
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        ..Default::default()
    }
}
/// A profile with a full look table, a few hundred kilobytes stored inline, as an
/// imported camera profile is.
fn large_profile(seed: f32) -> Arc<CameraProfile> {
    Arc::new(
        CameraProfile::camera_matrix_default(&camera(seed))
            .unwrap()
            .with_test_srgb_look(),
    )
}
fn small_profile(seed: f32) -> Arc<CameraProfile> {
    Arc::new(CameraProfile::camera_matrix_default(&camera(seed)).unwrap())
}
/// A Camera Look fitted to one photo: every photo's is its own.
fn fitted_look(n: usize) -> Arc<CameraProfile> {
    let base = CameraProfile::camera_matrix_default(&camera(0.01)).unwrap();
    let mut look = fitted::FittedLook::identity();
    let at = n % look.table.len();
    look.table[at][2] = 1. + (n as f32 + 1.) / 1000.;
    Arc::new(fitted::with_look(&base, &look).unwrap())
}
/// The Embedded profile as releases before its tone curve stored it.
fn old_embedded() -> Arc<CameraProfile> {
    let m = Metadata {
        source: Source::Rendered,
        ..camera(0.)
    };
    let mut v = serde_json::to_value(CameraProfile::embedded(&m)).unwrap();
    v["tone"] = serde_json::json!([[0., 0.], [1., 1.]]);
    Arc::new(serde_json::from_value(v).unwrap())
}
fn fresh_embedded() -> Arc<CameraProfile> {
    Arc::new(CameraProfile::embedded(&Metadata {
        source: Source::Rendered,
        ..camera(0.)
    }))
}
fn cube_look() -> Arc<CameraProfile> {
    let cube = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n0.9 0.95 1\n";
    Arc::new(
        CameraProfile::camera_matrix_default(&camera(0.02))
            .unwrap()
            .with_cube_for_test(cube)
            .unwrap(),
    )
}
fn edit(profile: &Arc<CameraProfile>, exposure: f32) -> Recipe {
    Recipe {
        profile: Some(profile.clone()),
        exposure,
        preset_name: "Vivid".into(),
        ..Default::default()
    }
}
/// A recipe naming a Lensfun lens, with its calibration.
fn with_lensfun(on: bool) -> Recipe {
    let mut r = Recipe {
        lens_profile: on,
        ..Default::default()
    };
    r.lens_profile_choice
        .choose_lensfun(rawmakase_model::lens::lensfun::fixture::choice());
    r
}

/// `value` written and read again, as a stored value reads.
fn reparsed(value: &Value) -> Value {
    serde_json::from_str(&value.to_string()).unwrap()
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
fn column<T: rusqlite::types::FromSql>(path: &Path, sql: &str, id: i64) -> Result<T> {
    Ok(Connection::open(path)?.query_row(sql, [id], |r| r.get(0))?)
}
fn asset_ids(path: &Path) -> Result<Vec<(String, String)>> {
    let db = Connection::open(path)?;
    let mut s = db.prepare("SELECT id, kind FROM recipe_assets ORDER BY id")?;
    let rows = s
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}
fn file_size(path: &Path) -> Result<u64> {
    Ok(std::fs::metadata(path)?.len())
}
fn snapshot_recipes(cat: &Catalog, id: PhotoId) -> Result<Vec<Recipe>> {
    Ok(cat
        .snapshots(id)?
        .into_iter()
        .map(|s| match s.settings {
            SnapshotSettings::Recipe(r) => *r,
            SnapshotSettings::Lightroom(_) => panic!("a recipe snapshot"),
        })
        .collect())
}
/// `r` as an earlier release's stored text reads now.
fn read_inline(r: &Recipe) -> Recipe {
    serde_json::from_value(inline(r)).unwrap()
}
fn history(states: &[Recipe]) -> SavedHistory {
    SavedHistory {
        origin: states[0].clone(),
        steps: states[1..]
            .iter()
            .enumerate()
            .map(|(i, r)| SavedStep {
                name: format!("Step {i}"),
                value: format!("{i}"),
                recipe: r.clone(),
            })
            .collect(),
        applied: states.len() - 1,
    }
}

/// The golden upgrade: every kind of profile, a Lensfun lens, a copy of an applied
/// preset's settings, a History whose pooled profiles differ from the edit's,
/// snapshots, a virtual copy, a damaged edit and a newer release's History. After
/// the upgrade every edit, History state and snapshot reads as the same recipe, the
/// rows it couldn't read are as they were, the format is 7 and the backup opens.
#[test]
fn the_upgrade_keeps_every_stored_recipe() -> Result<()> {
    let shared = large_profile(0.03);
    let edits = [
        edit(&shared, 0.1),
        edit(&shared, 0.2),
        edit(&old_embedded(), 0.3),
        edit(
            &Arc::new(small_profile(0.04).as_ref().clone().as_hdr_for_test()),
            0.4,
        ),
        edit(&fitted_look(1), 0.5),
        edit(&cube_look(), 0.6),
        Recipe {
            exposure: 0.7,
            ..with_lensfun(true)
        },
    ];
    let mut f = Earlier::new(6, edits.len() + 3)?;
    for (n, r) in edits.iter().enumerate() {
        f.set_edit(n, r)?;
    }
    // A History whose states hold other profiles than the edit's, and a Lensfun lens.
    let states = [
        edit(&small_profile(0.05), 0.),
        edit(&large_profile(0.06), 0.1),
        Recipe {
            exposure: 0.2,
            ..with_lensfun(true)
        },
        edit(&shared, 0.1),
    ];
    f.set_history(0, &states)?;
    f.add_snapshot(0, "Before", &edit(&large_profile(0.07), -0.5))?;
    f.add_snapshot(6, "Lens", &with_lensfun(false))?;
    let copy = edits.len();
    f.make_copy(copy, 0)?;
    // A damaged edit, and a History of a newer release.
    let damaged = r#"{"exposure":0.5,"exposure":1.5}"#;
    f.set_edit_text(copy + 1, damaged)?;
    let newer = compress(&serde_json::json!({"version": 99, "anything": [1]}));
    f.set_history_data(copy + 1, &newer)?;
    f.set_edit(copy + 2, &edit(&shared, 0.9))?;
    f.set_history_data(
        copy + 2,
        &compress(&serde_json::json!({"version": 1, "garbage": true})),
    )?;
    let unreadable_history: Vec<u8> = f.with_db(|db| {
        Ok(db.query_row(
            "SELECT data FROM develop_history WHERE photo=?1",
            [f.photos[copy + 2].0.0],
            |r| r.get(0),
        )?)
    })?;
    let before = file_size(&f.path)?;

    let cat = f.open()?;
    assert_eq!(cat.format_version()?, PROFILE_ASSETS);
    let upgraded = cat.upgraded().unwrap().clone();
    assert_eq!(upgraded.from, 6);
    assert_eq!(upgraded.unreadable, 2, "the damaged edit and History");
    assert_eq!(upgraded.vacuum_error, None);
    for (n, r) in edits.iter().enumerate() {
        let (id, file) = &f.photos[n];
        assert_eq!(
            cat.load_edit(*id, file)?.unwrap().recipe,
            read_inline(r),
            "edit {n}"
        );
        // Stored by reference.
        let text: String = column(&f.path, "SELECT recipe FROM photos WHERE id=?1", id.0)?;
        assert!(!text.contains("preset_settings"), "{n}");
        let stored: Value = serde_json::from_str(&text)?;
        if r.profile.is_some() {
            assert!(recipe_assets::is_asset_id(
                stored["profile"].as_str().unwrap()
            ));
        }
    }
    let (copy_id, copy_file) = &f.photos[copy];
    assert_eq!(
        cat.load_edit(*copy_id, copy_file)?.unwrap().recipe,
        read_inline(&edits[0])
    );
    let read = cat.load_history(f.photos[0].0)?.unwrap();
    let expected: Vec<Recipe> = states.iter().map(read_inline).collect();
    assert_eq!(read.origin, expected[0]);
    assert_eq!(
        read.steps
            .iter()
            .map(|s| s.recipe.clone())
            .collect::<Vec<_>>(),
        expected[1..]
    );
    assert_eq!(
        snapshot_recipes(&cat, f.photos[0].0)?,
        [read_inline(&edit(&large_profile(0.07), -0.5))]
    );
    assert_eq!(
        snapshot_recipes(&cat, f.photos[6].0)?,
        [read_inline(&with_lensfun(false))]
    );
    // What it couldn't read is as it was.
    let id = f.photos[copy + 1].0.0;
    let text: String = column(&f.path, "SELECT recipe FROM photos WHERE id=?1", id)?;
    assert_eq!(text, damaged);
    let data: Vec<u8> = column(
        &f.path,
        "SELECT data FROM develop_history WHERE photo=?1",
        id,
    )?;
    assert_eq!(data, newer);
    let id = f.photos[copy + 2].0.0;
    let data: Vec<u8> = column(
        &f.path,
        "SELECT data FROM develop_history WHERE photo=?1",
        id,
    )?;
    assert_eq!(data, unreadable_history);
    // One asset per distinct profile and calibration.
    let kinds: Vec<String> = asset_ids(&f.path)?.into_iter().map(|(_, k)| k).collect();
    assert_eq!(kinds.iter().filter(|k| *k == "camera_profile").count(), 8);
    assert_eq!(
        kinds.iter().filter(|k| *k == "lensfun_calibration").count(),
        1
    );
    // The backup is the catalog as it was, and opens.
    assert_eq!(
        backups(f.dir.path())?,
        std::slice::from_ref(&upgraded.backup)
    );
    assert_eq!(version(&upgraded.backup)?, 6);
    assert!(file_size(&upgraded.backup)? > 0 && before > 0);
    drop(cat);
    let reopened_backup = f.dir.path().join("Backup copy.rawmakase");
    std::fs::copy(&upgraded.backup, &reopened_backup)?;
    let backup = Catalog::open(&reopened_backup)?;
    let (id, file) = &f.photos[1];
    assert_eq!(
        backup.load_edit(*id, file)?.unwrap().recipe,
        read_inline(&edits[1])
    );
    Ok(())
}

/// A second open finds the catalog current: it writes nothing and makes no backup.
#[test]
fn a_second_open_writes_nothing() -> Result<()> {
    let mut f = Earlier::new(6, 2)?;
    f.set_edit(0, &edit(&large_profile(0.08), 0.5))?;
    drop(f.open()?);
    let bytes = std::fs::read(&f.path)?;
    let modified = std::fs::metadata(&f.path)?.modified()?;
    let backups_before = backups(f.dir.path())?;
    let cat = Catalog::open(&f.path)?;
    assert!(cat.upgraded().is_none());
    let (id, file) = &f.photos[0];
    assert!(cat.load_edit(*id, file)?.is_some());
    drop(cat);
    assert_eq!(std::fs::read(&f.path)?, bytes);
    assert_eq!(std::fs::metadata(&f.path)?.modified()?, modified);
    assert_eq!(backups(f.dir.path())?, backups_before);
    Ok(())
}

/// A catalog of `version` whose two photos have edits sharing a large profile, a
/// History and a snapshot.
fn upgradable(version: i64) -> Result<Earlier> {
    let f = Earlier::new(version, 2)?;
    let profile = large_profile(0.09);
    f.set_edit(0, &edit(&profile, 0.5))?;
    f.set_edit(1, &edit(&profile, 0.25))?;
    f.set_history(0, &[edit(&profile, 0.), edit(&profile, 0.5)])?;
    f.add_snapshot(1, "Before", &edit(&profile, 0.))?;
    Ok(f)
}
/// Opening `f` fails with an error containing `says`, and leaves its file as it was.
fn refused(f: &mut Earlier, says: &str) -> Result<()> {
    f.close();
    let bytes = std::fs::read(&f.path)?;
    let error = format!("{:#}", Catalog::open(&f.path).err().expect("refused"));
    assert!(error.contains(says), "{error}");
    assert_eq!(std::fs::read(&f.path)?, bytes, "the catalog changed");
    assert_eq!(version(&f.path)?, 6);
    Ok(())
}

/// When the backup can't be written, the catalog is not opened and stays as it
/// was; the error says how much space the backup needs.
#[test]
fn a_failed_backup_leaves_the_catalog_unchanged() -> Result<()> {
    let mut f = upgradable(6)?;
    FAIL_BACKUP.with(|b| b.set(true));
    let result = refused(&mut f, "free beside it");
    FAIL_BACKUP.with(|b| b.set(false));
    result?;
    assert!(backups(f.dir.path())?.is_empty());
    // Once there is room, it opens.
    assert_eq!(Catalog::open(&f.path)?.format_version()?, PROFILE_ASSETS);
    Ok(())
}

/// A failure in the middle of the upgrade leaves the catalog as it was, the backup
/// beside it.
#[test]
fn a_failed_upgrade_leaves_the_catalog_unchanged() -> Result<()> {
    let mut f = upgradable(6)?;
    FAIL_AFTER_ROWS.with(|r| r.set(Some(2)));
    let result = refused(&mut f, "injected");
    FAIL_AFTER_ROWS.with(|r| r.set(None));
    result?;
    assert_eq!(backups(f.dir.path())?.len(), 1);
    Ok(())
}

/// The upgrade reads what it wrote back through its own transaction: a damaged or
/// missing asset row is caught even while the right profile is in this process's
/// cache and stored in another open catalog, and the catalog stays as it was.
#[test]
fn the_upgrade_checks_what_it_stored_past_the_cache() -> Result<()> {
    let profile = large_profile(0.09);
    // The same profile stored in another open catalog, and warm in the cache.
    let mut other = Earlier::new(6, 1)?;
    other.set_edit(0, &edit(&profile, 0.5))?;
    let other_catalog = other.open()?;
    let (id, file) = &other.photos[0];
    let held = other_catalog.load_edit(*id, file)?.unwrap().recipe;
    for fault in [&DAMAGE_ASSETS, &DROP_ASSETS] {
        let mut f = upgradable(6)?;
        fault.with(|d| d.set(true));
        let result = refused(&mut f, "the catalog was not upgraded");
        fault.with(|d| d.set(false));
        result?;
    }
    assert_eq!(held.profile.as_deref(), Some(profile.as_ref()));
    drop(other_catalog);
    Ok(())
}

/// An Embedded profile saved with an old tone curve is stored once, under the id a
/// fresh save of it has, whether the upgrade or a save stored it, with nothing
/// cached.
#[test]
fn an_old_embedded_profile_is_stored_once_under_a_fresh_saves_id() -> Result<()> {
    let mut f = Earlier::new(6, 1)?;
    f.set_edit(0, &edit(&old_embedded(), 0.5))?;
    drop(f.open()?);
    recipe_assets::clear_cache();
    let mut cat = Catalog::open(&f.path)?;
    let (id, file) = f.photos[0].clone();
    let loaded = cat.load_edit(id, &file)?.unwrap();
    assert_eq!(
        loaded.recipe.profile.as_deref(),
        Some(fresh_embedded().as_ref())
    );
    cat.save_edit(
        id,
        &file,
        &crate::storage::Identity::read(&file)?,
        &edit(&fresh_embedded(), 0.6),
        &loaded.export,
        HistoryUpdate::Keep,
    )?;
    assert_eq!(
        asset_ids(&f.path)?,
        [(
            Asset::profile(&fresh_embedded()).id,
            "camera_profile".into()
        )]
    );
    Ok(())
}

/// Fields of a later History format, and settings this release doesn't know,
/// stay where they were; the profile becomes a reference, the copy of the preset's
/// settings goes and the pool is renumbered.
#[test]
fn the_upgrade_keeps_what_it_does_not_know_in_a_history() -> Result<()> {
    let mut f = Earlier::new(2, 1)?;
    f.set_edit(0, &Recipe::default())?;
    let profile = large_profile(0.1);
    let profile_value = serde_json::to_value(profile.as_ref())?;
    let mut origin = serde_json::Map::new();
    origin.insert("exposure".into(), 0.into());
    origin.insert("future_slider".into(), serde_json::json!([1, 2]));
    for key in OBSOLETE_SETTINGS {
        origin.insert((*key).into(), "Original".into());
    }
    let history = serde_json::json!({
        "version": 1,
        "pool": [{"long": "value"}, profile_value],
        "origin": {"fields": origin, "pooled": {"big_setting": 0, "profile": 1}, "state_extra": 1},
        "steps": [{"name": "Exposure", "value": "+0.50",
                   "state": {"fields": {"exposure": 0.5}, "pooled": {"profile": 1}},
                   "step_extra": [1]}],
        "applied": 1,
        "history_extra": {"a": true},
    });
    f.set_history_data(0, &compress(&history))?;
    let cat = f.open()?;
    let id = f.photos[0].0;
    let after = decompress(&column::<Vec<u8>>(
        &f.path,
        "SELECT data FROM develop_history WHERE photo=?1",
        id.0,
    )?);
    let reference = Value::String(Asset::profile(&profile).id);
    let mut expected = history;
    let fields = expected
        .pointer_mut("/origin/fields")
        .unwrap()
        .as_object_mut()
        .unwrap();
    crate::model::saved_format::drop_obsolete_settings(fields);
    expected["pool"] = serde_json::json!([{"long": "value"}, reference]);
    assert_eq!(after, reparsed(&expected));
    let read = cat.load_history(id)?.unwrap();
    assert_eq!(read.origin.profile.as_deref(), Some(profile.as_ref()));
    assert_eq!(
        read.origin.unknown.keys().collect::<Vec<_>>(),
        ["big_setting", "future_slider"]
    );
    assert_eq!(read.steps[0].recipe.exposure, 0.5);
    Ok(())
}

/// The History's encoder pools values by content alone, so one entry may serve the
/// profile and a setting this release doesn't know that holds the same object (and
/// likewise the lens choice). The recognised field takes the reference; the other
/// keeps the object as it was.
#[test]
fn a_pool_entry_shared_with_an_unknown_setting_stays_for_it() -> Result<()> {
    let mut f = Earlier::new(6, 1)?;
    f.set_edit(0, &Recipe::default())?;
    let profile = large_profile(0.11);
    let profile_value = serde_json::to_value(profile.as_ref())?;
    let choice_value = serde_json::to_value(&with_lensfun(true).lens_profile_choice)?;
    let history = serde_json::json!({
        "version": 1,
        "pool": [profile_value, choice_value],
        "origin": {"fields": {"exposure": 0.0},
                   "pooled": {"profile": 0, "my_profile_copy": 0,
                              "lens_profile_choice": 1, "my_lens": 1}},
        "steps": [],
        "applied": 0,
    });
    f.set_history_data(0, &compress(&history))?;
    let cat = f.open()?;
    let id = f.photos[0].0;
    let after = decompress(&column::<Vec<u8>>(
        &f.path,
        "SELECT data FROM develop_history WHERE photo=?1",
        id.0,
    )?);
    let pool = after["pool"].as_array().unwrap();
    let at = |key: &str| pool[after["origin"]["pooled"][key].as_u64().unwrap() as usize].clone();
    assert_eq!(at("my_profile_copy"), reparsed(&profile_value));
    assert_eq!(at("my_lens"), reparsed(&choice_value));
    // Kept to the byte, not written again.
    let text = String::from_utf8(super::develop_history::text(&column::<Vec<u8>>(
        &f.path,
        "SELECT data FROM develop_history WHERE photo=?1",
        id.0,
    )?)?)?;
    assert!(text.contains(&profile_value.to_string()));
    assert!(text.contains(&choice_value.to_string()));
    assert_eq!(at("profile"), Value::String(Asset::profile(&profile).id));
    assert!(at("lens_profile_choice")["id"]["lensfun"]["calibration"].is_string());
    assert_eq!(pool.len(), 4);
    let read = cat.load_history(id)?.unwrap();
    assert_eq!(read.origin.profile.as_deref(), Some(profile.as_ref()));
    assert_eq!(
        read.origin.unknown["my_profile_copy"],
        reparsed(&profile_value)
    );
    assert_eq!(
        read.origin.lens_profile_choice,
        with_lensfun(true).lens_profile_choice
    );
    Ok(())
}

/// A damaged History (here giving a setting twice, which reading would collapse)
/// and a damaged snapshot are left as they were.
#[test]
fn the_upgrade_leaves_damaged_rows_as_they_were() -> Result<()> {
    let mut f = Earlier::new(2, 1)?;
    f.set_edit(0, &Recipe::default())?;
    let damaged = r#"{"engine":3,"exposure":0.5,"exposure":1.5}"#;
    let history = format!(
        r#"{{"version":1,"pool":[],"origin":{{"fields":{damaged}}},"steps":[],"applied":0}}"#
    );
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut z, history.as_bytes())?;
    let history = z.finish()?;
    f.set_history_data(0, &history)?;
    f.add_snapshot_text(0, "Damaged", damaged)?;
    let cat = f.open()?;
    assert_eq!(cat.upgraded().unwrap().unreadable, 2);
    let id = f.photos[0].0.0;
    let data: Vec<u8> = column(
        &f.path,
        "SELECT data FROM develop_history WHERE photo=?1",
        id,
    )?;
    let text: String = column(
        &f.path,
        "SELECT recipe FROM develop_snapshots WHERE photo=?1",
        id,
    )?;
    assert_eq!((text.as_str(), data), (damaged, history));
    Ok(())
}

/// A Lensfun lens's calibration is stored once and named by id in edits, pooled
/// History values and snapshots, whether the upgrade or a save stored them.
#[test]
fn a_lensfun_calibration_is_stored_once_and_named() -> Result<()> {
    let lens = with_lensfun(true);
    let mut f = Earlier::new(6, 2)?;
    f.set_edit(0, &lens)?;
    f.set_history(0, &[Recipe::default(), lens.clone()])?;
    f.add_snapshot(0, "Lens", &lens)?;
    let mut cat = f.open()?;
    let pointer = "/lens_profile_choice/id/lensfun/calibration";
    let id = f.photos[0].0;
    let check = |text: &Value| {
        let calibration = text.pointer(pointer).and_then(Value::as_str).unwrap();
        assert!(recipe_assets::is_asset_id(calibration));
    };
    check(&serde_json::from_str(&column::<String>(
        &f.path,
        "SELECT recipe FROM photos WHERE id=?1",
        id.0,
    )?)?);
    check(&serde_json::from_str(&column::<String>(
        &f.path,
        "SELECT recipe FROM develop_snapshots WHERE photo=?1",
        id.0,
    )?)?);
    let history = decompress(&column::<Vec<u8>>(
        &f.path,
        "SELECT data FROM develop_history WHERE photo=?1",
        id.0,
    )?);
    let pooled = history["steps"][0]["state"]["pooled"]["lens_profile_choice"].as_u64();
    let choice = match pooled {
        Some(n) => history["pool"][n as usize].clone(),
        None => history["steps"][0]["state"]["fields"]["lens_profile_choice"].clone(),
    };
    check(&serde_json::json!({"lens_profile_choice": choice}));
    assert_eq!(
        cat.load_edit(id, &f.photos[0].1)?.unwrap().recipe,
        read_inline(&lens)
    );
    assert_eq!(
        cat.load_history(id)?.unwrap().steps[0].recipe,
        read_inline(&lens)
    );
    // A save of the same lens on another photo adds nothing.
    let rows = asset_ids(&f.path)?;
    let (other, file) = f.photos[1].clone();
    cat.save_edit(
        other,
        &file,
        &crate::storage::Identity::read(&file)?,
        &lens,
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    check(&serde_json::from_str(&column::<String>(
        &f.path,
        "SELECT recipe FROM photos WHERE id=?1",
        other.0,
    )?)?);
    assert_eq!(asset_ids(&f.path)?, rows);
    Ok(())
}

/// Catalogs of the first formats, made with the schemas of their releases, which
/// lack the tables added since, are upgraded in one go.
#[test]
fn catalogs_of_the_first_formats_upgrade() -> Result<()> {
    for format in [1, 2, 6] {
        let mut f = upgradable(format)?;
        let mut obsolete = inline(&Recipe::default());
        obsolete["engine"] = 3.into();
        obsolete["profile_tone"] = "Original".into();
        obsolete["exposure"] = 0.75.into();
        f.add_snapshot_text(0, "Obsolete", &obsolete.to_string())?;
        let cat = f.open()?;
        assert_eq!(cat.format_version()?, PROFILE_ASSETS, "{format}");
        assert_eq!(cat.upgraded().unwrap().from, format);
        let (id, file) = &f.photos[0];
        assert_eq!(
            cat.load_edit(*id, file)?.unwrap().recipe,
            read_inline(&edit(&large_profile(0.09), 0.5))
        );
        assert_eq!(cat.load_history(*id)?.unwrap().steps.len(), 1);
        let snapshot: String = column(
            &f.path,
            "SELECT recipe FROM develop_snapshots WHERE name='Obsolete' AND photo=?1",
            id.0,
        )?;
        assert!(
            !snapshot.contains("engine") && !snapshot.contains("profile_tone"),
            "{snapshot}"
        );
        assert_eq!(asset_ids(&f.path)?.len(), 1, "{format}");
        // Tables of later releases are there.
        Connection::open(&f.path)?.query_row("SELECT COUNT(*) FROM photo_checksums", [], |r| {
            r.get::<_, i64>(0)
        })?;
    }
    Ok(())
}

/// What an upgrade holds at once is bounded by a batch, not by the catalog: with
/// hundreds of distinct profiles (each photo's fitted look) or one shared by
/// every photo, as format 1, 2 or 6, with History and snapshots.
#[test]
fn the_upgrade_holds_a_batch_at_a_time() -> Result<()> {
    let count = 2 * super::format_upgrade::BATCH as usize + 20;
    for distinct in [true, false] {
        for format in [1, 2, 6] {
            let mut f = Earlier::new(format, count)?;
            let shared = small_profile(0.12);
            for n in 0..count {
                let profile = if distinct {
                    small_profile(0.2 + n as f32 / 1000.)
                } else {
                    shared.clone()
                };
                f.set_edit(n, &edit(&profile, 0.1))?;
                if n % 3 == 0 {
                    f.set_history(n, &[edit(&profile, 0.), edit(&profile, 0.1)])?;
                    f.add_snapshot(n, "Before", &edit(&profile, 0.))?;
                }
            }
            MOST_VERIFIED.with(|m| m.set(0));
            let cat = f.open()?;
            let most = MOST_VERIFIED.with(std::cell::Cell::get);
            assert!(
                most <= super::format_upgrade::BATCH as usize + 1,
                "{format} {distinct}: {most} assets held"
            );
            assert!(most >= 1);
            let rows = asset_ids(&f.path)?.len();
            assert_eq!(rows, if distinct { count } else { 1 });
            let (id, file) = &f.photos[count - 1];
            assert!(cat.load_edit(*id, file)?.is_some());
        }
    }
    // Fitted looks: every photo's own, each its own asset.
    let mut f = Earlier::new(6, 3)?;
    for n in 0..3 {
        f.set_edit(n, &edit(&fitted_look(n + 10), 0.))?;
    }
    drop(f.open()?);
    assert_eq!(asset_ids(&f.path)?.len(), 3);
    Ok(())
}

/// A catalog of many edits holding one large profile each shrinks to a small
/// fraction once the profile is stored once and the space given back.
#[test]
fn the_upgraded_catalog_is_a_fraction_of_its_size() -> Result<()> {
    let count = 40;
    let mut f = Earlier::new(6, count)?;
    let profile = large_profile(0.13);
    let other = large_profile(0.14);
    for n in 0..count {
        f.set_edit(n, &edit(&profile, n as f32 / 100.))?;
        f.set_history(n, &[edit(&other, 0.), edit(&profile, n as f32 / 100.)])?;
    }
    f.close();
    let before = file_size(&f.path)?;
    let cat = Catalog::open(&f.path)?;
    drop(cat);
    let after = file_size(&f.path)?;
    println!("catalog of {count} edits: {before} bytes before, {after} after the upgrade");
    assert!(after * 20 < before, "{before} → {after}");
    Ok(())
}

/// Edits naming one profile share it in memory, read cold.
#[test]
fn edits_naming_one_profile_share_it() -> Result<()> {
    let mut f = Earlier::new(6, 4)?;
    let profile = large_profile(0.15);
    for n in 0..4 {
        f.set_edit(n, &edit(&profile, n as f32 / 10.))?;
    }
    let cat = f.open()?;
    recipe_assets::clear_cache();
    let loaded: Vec<Recipe> = f
        .photos
        .iter()
        .map(|(id, file)| Ok(cat.load_edit(*id, file)?.unwrap().recipe))
        .collect::<Result<_>>()?;
    let first = loaded[0].profile.as_ref().unwrap();
    assert!(
        loaded
            .iter()
            .all(|r| Arc::ptr_eq(r.profile.as_ref().unwrap(), first))
    );
    Ok(())
}

/// Read cold through the catalog's public reads: an edit, a History and a snapshot;
/// an export's record after the catalog is closed; two catalogs at once. A record
/// with no catalog behind it says so.
#[test]
fn stored_references_resolve_through_the_public_reads() -> Result<()> {
    let mut a = upgradable(6)?;
    let mut b = Earlier::new(6, 1)?;
    let b_profile = large_profile(0.16);
    b.set_edit(0, &edit(&b_profile, 0.3))?;
    let cat_a = a.open()?;
    let cat_b = b.open()?;
    recipe_assets::clear_cache();
    let (id, file) = a.photos[0].clone();
    let profile = large_profile(0.09);
    assert_eq!(
        cat_a.load_edit(id, &file)?.unwrap().recipe,
        read_inline(&edit(&profile, 0.5))
    );
    assert_eq!(
        cat_a.load_history(id)?.unwrap().steps[0].recipe,
        read_inline(&edit(&profile, 0.5))
    );
    assert_eq!(
        snapshot_recipes(&cat_a, a.photos[1].0)?,
        [read_inline(&edit(&profile, 0.))]
    );
    let (b_id, b_file) = &b.photos[0];
    assert_eq!(
        cat_b.load_edit(*b_id, b_file)?.unwrap().recipe,
        read_inline(&edit(&b_profile, 0.3))
    );
    // A record read for an export, resolved on another thread after its catalog closed.
    let record = cat_a.edit_record(id)?;
    drop(cat_a);
    recipe_assets::clear_cache();
    let resolved = std::thread::spawn(move || record.saved(&file).map(|s| s.unwrap().recipe))
        .join()
        .unwrap()?;
    assert_eq!(resolved, read_inline(&edit(&profile, 0.5)));
    // Without its catalog, the reference is reported.
    recipe_assets::clear_cache();
    let mut orphan = cat_b.edit_record(*b_id)?;
    orphan.assets = Default::default();
    let error = format!("{:#}", orphan.saved(b_file).unwrap_err());
    assert!(error.contains("is missing from the catalog"), "{error}");
    Ok(())
}

/// A save stores references and each profile once; a damaged stored profile is
/// repaired by the next save naming it.
#[test]
fn saves_store_each_profile_once_and_repair_a_damaged_one() -> Result<()> {
    let mut f = Earlier::new(6, 2)?;
    let mut cat = f.open()?;
    let profile = large_profile(0.17);
    let save = |cat: &mut Catalog, (id, file): &(PhotoId, PathBuf), r: &Recipe| {
        cat.save_edit(
            *id,
            file,
            &crate::storage::Identity::read(file)?,
            r,
            &ExportOptions::default(),
            HistoryUpdate::Replace(&history(&[Recipe::default(), r.clone()])),
        )
    };
    save(&mut cat, &f.photos[0], &edit(&profile, 0.1))?;
    let id = Asset::profile(&profile).id;
    assert_eq!(
        asset_ids(&f.path)?,
        [(id.clone(), "camera_profile".to_string())]
    );
    let text: String = column(
        &f.path,
        "SELECT recipe FROM photos WHERE id=?1",
        f.photos[0].0.0,
    )?;
    assert!(
        text.len() < 4096 && text.contains(&id),
        "{} bytes",
        text.len()
    );
    save(&mut cat, &f.photos[1], &edit(&profile, 0.2))?;
    assert_eq!(asset_ids(&f.path)?.len(), 1);
    // Damaged on disk: the next save naming it stores it again.
    cat.db_for_tests().execute(
        "UPDATE recipe_assets SET data=?1 WHERE id=?2",
        rusqlite::params![recipe_assets::compress(b"{}"), id],
    )?;
    save(&mut cat, &f.photos[1], &edit(&profile, 0.3))?;
    recipe_assets::clear_cache();
    let (photo, file) = &f.photos[0];
    assert_eq!(
        cat.load_edit(*photo, file)?
            .unwrap()
            .recipe
            .profile
            .as_deref(),
        Some(profile.as_ref())
    );
    Ok(())
}

/// A profile removed from disk after its edit was saved: the edit reads back with
/// it from the catalog, with nothing cached.
#[test]
fn an_edit_keeps_its_profile_after_the_file_is_gone() -> Result<()> {
    let mut f = Earlier::new(6, 1)?;
    let mut cat = f.open()?;
    let dir = tempfile::tempdir()?;
    let dcp = dir.path().join("Imported.dcp");
    std::fs::write(&dcp, serde_json::to_vec(large_profile(0.18).as_ref())?)?;
    let imported: CameraProfile = serde_json::from_slice(&std::fs::read(&dcp)?)?;
    let r = edit(&Arc::new(imported), 0.4);
    let (id, file) = f.photos[0].clone();
    cat.save_edit(
        id,
        &file,
        &crate::storage::Identity::read(&file)?,
        &r,
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    std::fs::remove_file(&dcp)?;
    drop(cat);
    recipe_assets::clear_cache();
    let cat = Catalog::open(&f.path)?;
    assert_eq!(cat.load_edit(id, &file)?.unwrap().recipe, r);
    Ok(())
}

/// Export as Catalog keeps exactly the profiles and calibrations its photos name,
/// and every one when a History can't be read.
#[test]
fn an_exported_catalog_keeps_the_profiles_it_names() -> Result<()> {
    let mut f = Earlier::new(6, 3)?;
    let (a, b, c) = (large_profile(0.19), large_profile(0.2), large_profile(0.21));
    f.set_edit(0, &edit(&a, 0.))?;
    f.set_history(0, &[edit(&b, 0.), edit(&a, 0.)])?;
    f.set_edit(1, &edit(&c, 0.))?;
    f.set_edit(2, &with_lensfun(true))?;
    let cat = f.open()?;
    let destination = f.dir.path().join("Exported.rawmakase");
    cat.export_catalog(
        &super::ExportScope::Photos(vec![f.photos[0].0]),
        &destination,
    )?;
    let mut expected = vec![Asset::profile(&a).id, Asset::profile(&b).id];
    expected.sort();
    let kept: Vec<String> = asset_ids(&destination)?
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(kept, expected);
    assert_eq!(version(&destination)?, PROFILE_ASSETS);
    let exported = Catalog::open(&destination)?;
    let (id, file) = &f.photos[0];
    assert_eq!(
        exported.load_edit(*id, file)?.unwrap().recipe,
        read_inline(&edit(&a, 0.))
    );
    drop(exported);
    // A History it can't read: every profile is kept.
    cat.db_for_tests().execute(
        "UPDATE develop_history SET data=x'00' WHERE photo=?1",
        [f.photos[0].0.0],
    )?;
    let destination = f.dir.path().join("Exported again.rawmakase");
    cat.export_catalog(
        &super::ExportScope::Photos(vec![f.photos[0].0]),
        &destination,
    )?;
    assert_eq!(asset_ids(&destination)?.len(), 4);
    Ok(())
}

/// Version 7 stores profiles once: releases that open up to 6 refuse it, and this
/// one creates and opens it.
#[test]
fn releases_before_stored_profiles_refuse_their_format() -> Result<()> {
    use super::db::{AI_DENOISE, HDR, LENSFUN};
    assert!(!opens(PROFILE_ASSETS, HDR));
    assert!(opens(PROFILE_ASSETS, NEWEST));
    assert!(!opens(NEWEST + 1, NEWEST));
    // Each earlier mark was refused by the releases before it.
    assert!(!opens(LENSFUN, ONE_ENGINE) && !opens(AI_DENOISE, LENSFUN) && !opens(HDR, AI_DENOISE));
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("New.rawmakase");
    let cat = Catalog::create(&path)?;
    assert_eq!(cat.format_version()?, PROFILE_ASSETS);
    assert!(cat.upgraded().is_none());
    assert!(backups(dir.path())?.is_empty());
    Ok(())
}

/// An empty catalog of an earlier format is upgraded too, with its backup.
#[test]
fn an_empty_catalog_is_upgraded_too() -> Result<()> {
    let mut f = Earlier::new(6, 0)?;
    let cat = f.open()?;
    assert_eq!(cat.format_version()?, PROFILE_ASSETS);
    assert_eq!(backups(f.dir.path())?.len(), 1);
    Ok(())
}

/// Export as Catalog of a Lensfun edit keeps the calibration with its source and
/// license.
#[test]
fn an_exported_lensfun_edit_keeps_its_attribution() -> Result<()> {
    let mut f = Earlier::new(6, 1)?;
    let mut cat = f.open()?;
    let (id, file) = f.photos[0].clone();
    cat.save_edit(
        id,
        &file,
        &crate::storage::Identity::read(&file)?,
        &with_lensfun(true),
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    let destination = f.dir.path().join("Exported.rawmakase");
    let export = cat.export_catalog(&super::ExportScope::Photos(vec![id]), &destination)?;
    assert!(
        export.lens_profiles.is_empty(),
        "{:?}",
        export.lens_profiles
    );
    recipe_assets::clear_cache();
    let exported = Catalog::open(&destination)?;
    let edit = exported.load_edit(id, &file)?.unwrap();
    let provenance = &edit
        .recipe
        .lens_profile_choice
        .lensfun()
        .unwrap()
        .provenance;
    assert_eq!(provenance.license, "CC BY-SA 3.0");
    assert_eq!(provenance.attribution, "Lensfun contributors");
    assert_eq!(
        provenance.source,
        "https://lensfun.github.io/db/version_2.tar.bz2"
    );
    Ok(())
}

/// An HDR edit with masks and the SDR rendition's sliders survives closing and
/// reopening the catalog wherever it is stored: as the photo's edit, in a virtual
/// copy, in a History state only and in a snapshot.
#[test]
fn an_hdr_edit_survives_reopening_wherever_it_is_stored() -> Result<()> {
    use rawmakase_model::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
    let mut hdr = Recipe::default();
    hdr.hdr.enabled = true;
    hdr.hdr.limit = 3.;
    hdr.hdr.sdr.highlights = -0.3;
    hdr.hdr.sdr.brightness = 0.2;
    hdr.exposure = 0.7;
    hdr.profile = Some(large_profile(0.22));
    hdr.masks = vec![MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0., 0.2],
            to: [0., 0.8],
        })],
        adjust: LocalAdjust {
            exposure: 0.5,
            ..Default::default()
        },
        ..Default::default()
    }];
    let mut f = Earlier::new(6, 2)?;
    let mut cat = f.open()?;
    let (id, file) = f.photos[0].clone();
    cat.save_edit(
        id,
        &file,
        &crate::storage::Identity::read(&file)?,
        &hdr,
        &ExportOptions::default(),
        HistoryUpdate::Replace(&history(&[Recipe::default(), hdr.clone()])),
    )?;
    let copy = cat.create_virtual_copy(id)?;
    cat.add_snapshot(f.photos[1].0, "HDR", &hdr)?;
    drop(cat);
    recipe_assets::clear_cache();
    let cat = Catalog::open(&f.path)?;
    assert_eq!(cat.load_edit(id, &file)?.unwrap().recipe, hdr);
    assert_eq!(cat.load_edit(copy, &file)?.unwrap().recipe, hdr);
    assert_eq!(cat.load_history(id)?.unwrap().steps[0].recipe, hdr);
    assert_eq!(snapshot_recipes(&cat, f.photos[1].0)?, [hdr]);
    Ok(())
}

/// An inline History, as earlier releases encoded it, reads as its states.
#[test]
fn the_fixtures_history_is_what_earlier_releases_stored() {
    let states = [
        edit(&large_profile(0.23), 0.),
        edit(&large_profile(0.23), 0.5),
    ];
    let stored = inline_history(&states);
    let profile = serde_json::to_value(states[0].profile.as_deref().unwrap()).unwrap();
    let pool = stored["pool"].as_array().unwrap();
    assert_eq!(pool.iter().filter(|v| **v == profile).count(), 1);
    let at = stored["origin"]["pooled"]["profile"].as_u64().unwrap() as usize;
    assert_eq!(pool[at], profile);
    assert_eq!(
        stored["steps"][0]["state"]["pooled"]["profile"].as_u64(),
        Some(at as u64)
    );
}

/// A History the upgrade can read but not rewrite (its own fields name a lens
/// choice with an empty calibration that a pooled choice shadows) is left as it
/// was, and what the upgrade began for it doesn't leak into the next row: a later
/// History with the same profile still stores it, and the catalog opens.
#[test]
fn a_row_left_as_it_was_leaves_nothing_behind() -> Result<()> {
    let mut f = Earlier::new(6, 2)?;
    let profile = large_profile(0.24);
    let p = serde_json::to_value(profile.as_ref())?;
    let shadowed = serde_json::json!({
        "version": 1,
        "pool": [{}],
        "origin": {"fields": {"profile": p, "lens_profile_choice": {"id": {"lensfun": {"calibration": {}}}}},
                   "pooled": {"lens_profile_choice": 0}},
        "steps": [],
        "applied": 0,
    });
    f.set_edit(0, &Recipe::default())?;
    f.set_history_data(0, &compress(&shadowed))?;
    f.set_edit(1, &Recipe::default())?;
    f.set_history(1, &[edit(&profile, 0.), edit(&profile, 0.5)])?;
    let cat = f.open()?;
    assert_eq!(cat.upgraded().unwrap().unreadable, 1);
    assert_eq!(
        cat.load_history(f.photos[1].0)?.unwrap().steps[0].recipe,
        read_inline(&edit(&profile, 0.5))
    );
    let data: Vec<u8> = column(
        &f.path,
        "SELECT data FROM develop_history WHERE photo=?1",
        f.photos[0].0.0,
    )?;
    assert_eq!(data, compress(&shadowed), "left byte for byte");
    Ok(())
}

/// An edit or snapshot giving a setting this release doesn't know twice is left as
/// it was: rewriting it would keep one of the two.
#[test]
fn a_recipe_giving_a_key_twice_is_left_as_it_was() -> Result<()> {
    let mut f = Earlier::new(6, 1)?;
    let mut text = inline(&edit(&large_profile(0.25), 0.5)).to_string();
    text.insert_str(1, r#""future_setting":1,"future_setting":2,"#);
    f.set_edit_text(0, &text)?;
    f.add_snapshot_text(0, "Twice", &text)?;
    let cat = f.open()?;
    assert_eq!(cat.upgraded().unwrap().unreadable, 2);
    let id = f.photos[0].0.0;
    let edit: String = column(&f.path, "SELECT recipe FROM photos WHERE id=?1", id)?;
    let snapshot: String = column(
        &f.path,
        "SELECT recipe FROM develop_snapshots WHERE photo=?1",
        id,
    )?;
    assert_eq!(
        (edit.as_str(), snapshot.as_str()),
        (text.as_str(), text.as_str())
    );
    Ok(())
}
