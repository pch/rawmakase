//! The catalog's SQL is written so that SQLite and Postgres both accept it
//! (issue #341). Each rewrite whose meaning could differ from the SQLite-only
//! form it replaced is pinned here against what that form did.
use super::locations::Computer;
use super::*;
use crate::{export_settings::ExportOptions, model::recipe::Recipe};
use rusqlite::params;
use std::path::PathBuf;

/// A catalog with `n` photos added from a folder, their ids and the folder.
fn catalog(n: usize) -> Result<(tempfile::TempDir, Catalog, Vec<PhotoId>, PathBuf)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for i in 0..n {
        std::fs::write(
            folder.join(format!("image{i}.ARW")),
            format!("synthetic {i}"),
        )?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let ids = cat.photos()?.iter().map(|p| p.id).collect();
    Ok((dir, cat, ids, folder))
}

#[test]
fn no_table_an_upsert_replaces_rows_of_has_rows_depending_on_it() -> Result<()> {
    // `INSERT OR REPLACE` deleted the old row, firing `ON DELETE CASCADE`
    // on rows referring to it; `ON CONFLICT DO UPDATE` keeps it. Both did
    // the same only while nothing refers to these tables.
    let (_dir, cat, _, _) = catalog(0)?;
    let upserted = [
        "develop_history",
        "folder_locations",
        "keyword_export",
        "local_edits",
        "meta",
        "photo_info",
        "photo_text",
    ];
    let tables: Vec<String> = cat
        .db_for_tests()
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for table in &tables {
        let targets: Vec<String> = cat
            .db_for_tests()
            .prepare("SELECT \"table\" FROM pragma_foreign_key_list(?)")?
            .query_map([table], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for target in targets {
            assert!(
                !upserted.contains(&target.as_str()),
                "{table} refers to {target}"
            );
        }
    }
    Ok(())
}

#[test]
fn upserts_replace_every_column_they_write() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(1)?;
    let id = ids[0];
    let info = |camera: &str, width| crate::metadata::PhotoInfo {
        camera: Some(camera.into()),
        lens: Some(format!("{camera} lens")),
        focal: Some(50.),
        aperture: Some(2.8),
        exposure: Some(0.01),
        iso: Some(400.),
        dimensions: Some((width, 4000)),
    };
    cat.fill_photo_info(&[(id, Some(info("First", 6000)))])?;
    cat.fill_photo_info(&[(id, Some(info("Second", 5000)))])?;
    assert_eq!(cat.photo_info(id)?, Some(info("Second", 5000)));
    // A file without info replaces every column with NULL.
    cat.fill_photo_info(&[(id, None)])?;
    assert_eq!(cat.photo_info(id)?, Some(Default::default()));

    cat.set_meta("key", "first")?;
    cat.set_meta("key", "second")?;
    assert_eq!(cat.meta("key")?.as_deref(), Some("second"));
    Ok(())
}

#[test]
fn locating_a_root_or_folder_again_moves_it() -> Result<()> {
    let (dir, mut cat, _, _) = catalog(1)?;
    let root = cat.roots()?[0].0;
    let (first, second) = (dir.path().join("first"), dir.path().join("second"));
    for place in [&first, &second] {
        std::fs::create_dir_all(place.join("sub"))?;
    }
    cat.relink_root(root, &first)?;
    cat.relink_root(root, &second)?;
    assert_eq!(cat.roots()?[0].2, Some(second.to_string_lossy().into()));

    let folder = cat.folders()?[0].clone();
    cat.relink_folder(folder.id, &first.join("sub"))?;
    cat.relink_folder(folder.id, &second.join("sub"))?;
    let rows = cat.location_rows()?.remove(&root).unwrap_or_default();
    let own: Vec<_> = rows.iter().filter(|(r, _)| *r == folder.relative).collect();
    assert_eq!(own, [&(folder.relative.clone(), second.join("sub"))]);
    assert_eq!(cat.folders()?[0].path, second.join("sub"));
    let mappings: i64 =
        cat.db_for_tests()
            .query_row("SELECT count(*) FROM folder_mappings", [], |r| r.get(0))?;
    assert_eq!(mappings, 1);
    Ok(())
}

#[test]
fn adopting_legacy_mappings_of_duplicate_folders_keeps_the_last_added() -> Result<()> {
    let (dir, cat, _, _) = catalog(1)?;
    let CatalogLocation::File(path) = cat.location().clone();
    let folder = cat.folders()?[0].clone();
    drop(cat);
    // An older release added the same folder twice and mapped each copy.
    let db = rusqlite::Connection::open(&path)?;
    db.execute(
        "INSERT INTO folders(root, relative_path) SELECT root, relative_path FROM folders",
        [],
    )?;
    let duplicate = FolderId(db.query_row("SELECT max(id) FROM folders", [], |r| r.get(0))?);
    db.execute(
        "INSERT INTO folder_paths(folder, path) VALUES (?, ?)",
        params![duplicate.0, folder.relative],
    )?;
    let (older, newer) = (dir.path().join("older"), dir.path().join("newer"));
    // Mapped newest folder first, so row order can't decide.
    for (id, place) in [(duplicate, &newer), (folder.id, &older)] {
        db.execute(
            "INSERT INTO folder_mappings(folder, path) VALUES (?, ?)",
            params![id.0, place.to_string_lossy()],
        )?;
    }
    drop(db);
    let cat = Catalog::open_as(
        &path,
        &Computer {
            id: "new".into(),
            name: "New".into(),
        },
    )?;
    let rows = cat
        .location_rows()?
        .remove(&folder.root)
        .unwrap_or_default();
    assert_eq!(rows, vec![(folder.relative, newer)]);
    Ok(())
}

#[test]
fn keywords_are_found_at_the_top_and_nested() -> Result<()> {
    let (_dir, mut cat, _, _) = catalog(0)?;
    let path = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    let top = cat.keyword_at(&path(&["Places"]))?;
    let nested = cat.keyword_at(&path(&["Places", "City"]))?;
    let city = cat.keyword_at(&path(&["City"]))?;
    assert_ne!(nested, city, "a top-level keyword is not a nested one");
    assert_eq!(cat.keyword_at(&path(&["Places"]))?, top);
    assert_eq!(cat.keyword_at(&path(&["Places", "City"]))?, nested);
    assert_eq!(cat.keyword_at(&path(&["City"]))?, city);
    let count: i64 = cat
        .db_for_tests()
        .query_row("SELECT count(*) FROM keywords", [], |r| r.get(0))?;
    assert_eq!(count, 3);
    Ok(())
}

#[test]
fn reused_numbered_parameters_bind_one_value() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(1)?;
    let master = ids[0];
    let first = cat.create_virtual_copy(master)?;
    let second = cat.create_virtual_copy(master)?;
    // `master_id=?1 ... id<>?1`: one value for both.
    cat.set_copy_as_master(first)?;
    let masters: std::collections::HashMap<PhotoId, Option<PhotoId>> = cat
        .photos()?
        .into_iter()
        .map(|p| (p.id, p.master))
        .collect();
    assert_eq!(masters[&first], None);
    assert_eq!(masters[&master], Some(first));
    assert_eq!(masters[&second], Some(first));
    Ok(())
}

#[test]
fn edit_times_keep_their_text_form() -> Result<()> {
    let (_dir, mut cat, ids, folder) = catalog(2)?;
    let before = rawmakase_model::time::now_text();
    cat.save_edit(
        ids[0],
        &folder.join("image0.ARW"),
        &crate::storage::Identity::read(&folder.join("image0.ARW"))?,
        &Recipe::default(),
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    let after = rawmakase_model::time::now_text();
    // Lightroom counts seconds from 2001, with fractions.
    cat.db_for_tests().execute(
        "INSERT INTO lightroom_history(photo, position, created, text) VALUES (?, 1, ?, '')",
        params![ids[1].0, 491_026_045.75],
    )?;
    let times = cat.edit_times()?;
    let saved = &times[&ids[0]];
    assert!(before <= *saved && *saved <= after, "{saved}");
    // Compared as text, "2016-07-24 04:07:25" sorts as the time it is.
    assert_eq!(saved.len(), "2016-07-24 04:07:25".len());
    assert_eq!(times[&ids[1]], "2016-07-24 04:07:25");
    // The adoption time of this computer has the same form.
    let adopted: String =
        cat.db_for_tests()
            .query_row("SELECT adopted_at FROM computers", [], |r| r.get(0))?;
    assert_eq!(adopted.len(), saved.len());
    assert_eq!(&adopted[10..11], " ");
    Ok(())
}

#[test]
fn lightroom_edit_times_round_as_sqlite_did() -> Result<()> {
    let db = rusqlite::Connection::open_in_memory()?;
    let mut created: Vec<f64> = vec![
        0.,
        0.4994,
        0.4995,
        0.9995,
        -0.0004,
        -0.0006,
        -1.5,
        59.9999,
        491_026_045.75,
        -978_307_200.,
        -978_307_200.000_6,
        // 0000-01-01 and the last millisecond of 9999.
        -63_145_526_400.,
        -63_145_526_400.001,
        252_423_993_599.999,
        252_423_993_600.,
        f64::MAX,
        f64::MIN,
    ];
    let mut seed = 1_u64;
    for _ in 0..2000 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let unit = (seed >> 11) as f64 / (1_u64 << 53) as f64;
        created.push((unit - 0.3) * 2e9);
    }
    for created in created {
        let sqlite: Option<String> = db.query_row(
            "SELECT datetime(? + 978307200, 'unixepoch')",
            [created],
            |r| r.get(0),
        )?;
        assert_eq!(super::edits::lightroom_time(created), sqlite, "{created}");
    }
    Ok(())
}

#[test]
fn cameras_sort_ignoring_ascii_case_only() -> Result<()> {
    let (_dir, cat, _, _) = catalog(0)?;
    let names = [
        "nikon Z 8",
        "Canon EOS R5",
        "canon eos r5",
        "_Prototype",
        "Zeiss",
        "Éclair",
        "apple iPhone",
        "[Scanner]",
        "Leica M11",
        "LEICA M10",
        "",
    ];
    for (i, name) in names.into_iter().enumerate() {
        cat.db_for_tests().execute(
            "INSERT INTO photo_info(photo, camera) VALUES (?, ?)",
            params![i as i64 + 1, name],
        )?;
    }
    let nocase: Vec<String> = cat
        .db_for_tests()
        .prepare(
            "SELECT DISTINCT camera FROM photo_info
             WHERE camera IS NOT NULL AND camera != ''
             ORDER BY camera COLLATE NOCASE, camera",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    assert_eq!(cat.cameras()?, nocase);
    Ok(())
}
