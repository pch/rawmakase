//! Import from Another Catalog: a trip catalog's photos joining the main one.
use super::*;
use anyhow::Context as _;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// What Import from Another Catalog does with each table of the other catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Carried {
    /// Copied, every row of the photos added.
    Copied,
    /// Found in this catalog by what it names, else made: folders, keywords
    /// and collections.
    Matched,
    /// Left in the other catalog: about the other catalog itself, or made
    /// again here (an import batch, a computer's locations, editable
    /// previews).
    Left,
}

/// Every table of the schema and what an import does with it. A table
/// added to the schema fails [`every_table_is_carried_or_left_on_purpose`]
/// until it is listed here, and copied by `import_catalog` when it is about
/// photos.
const TABLES: &[(&str, Carried)] = &[
    ("sources", Carried::Left),
    ("roots", Carried::Matched),
    ("folders", Carried::Matched),
    ("photos", Carried::Copied),
    ("collections", Carried::Matched),
    ("collection_photos", Carried::Copied),
    ("collection_options", Carried::Copied),
    ("smart_collection_rules", Carried::Left),
    ("keywords", Carried::Matched),
    ("keyword_synonyms", Carried::Copied),
    ("photo_keywords", Carried::Copied),
    ("folder_mappings", Carried::Left),
    ("lightroom_history", Carried::Copied),
    ("local_edits", Carried::Copied),
    ("develop_history", Carried::Copied),
    ("develop_snapshots", Carried::Copied),
    ("bitmaps", Carried::Copied),
    ("recipe_assets", Carried::Copied),
    ("photo_info", Carried::Copied),
    ("photo_checksums", Carried::Copied),
    ("editable_previews", Carried::Left),
    ("photo_sequences", Carried::Copied),
    ("import_batches", Carried::Left),
    // The photos added make an import batch of their own.
    ("photo_imports", Carried::Left),
    ("import_burst_stacks", Carried::Left),
    ("photo_import_facts", Carried::Copied),
    ("catalog_counters", Carried::Left),
    ("meta", Carried::Left),
    ("photo_fields", Carried::Copied),
    ("photo_text", Carried::Copied),
    ("photo_creators", Carried::Copied),
    ("photo_capture", Carried::Copied),
    ("photo_location", Carried::Copied),
    ("keyword_export", Carried::Copied),
    ("keyword_applied", Carried::Left),
    ("computers", Carried::Left),
    ("folder_paths", Carried::Matched),
    ("folder_locations", Carried::Matched),
    ("file_operations", Carried::Left),
    ("file_operation_members", Carried::Left),
    ("file_operation_copies", Carried::Left),
    ("stacks", Carried::Copied),
    ("stack_photos", Carried::Copied),
    ("photo_uuids", Carried::Copied),
];

#[test]
fn every_table_is_carried_or_left_on_purpose() {
    let schema: BTreeSet<&str> = include_str!("schema.sql")
        .lines()
        .filter_map(|l| l.trim().strip_prefix("CREATE TABLE IF NOT EXISTS "))
        .filter_map(|rest| rest.split([' ', '(']).next())
        .collect();
    let listed: BTreeSet<&str> = TABLES.iter().map(|(t, _)| *t).collect();
    assert_eq!(listed.len(), TABLES.len(), "a table is listed twice");
    assert_eq!(
        schema, listed,
        "say in TABLES what Import from Another Catalog does with each table"
    );
}

/// A main catalog with folder `home` (h1), and a trip catalog of root
/// `trip` (t1) and its folder `trip/day 2` (t2), in one temporary folder.
struct Setup {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    main: Catalog,
    trip: PathBuf,
}
fn setup() -> Result<Setup> {
    let d = tempfile::tempdir()?;
    let dir = d.path().canonicalize()?;
    for (folder, name) in [
        ("home", "h1.ARW"),
        ("trip", "t1.ARW"),
        ("trip/day 2", "t2.ARW"),
    ] {
        std::fs::create_dir_all(dir.join(folder))?;
        std::fs::write(dir.join(folder).join(name), name.as_bytes())?;
    }
    let mut main = Catalog::create(&dir.join("Main.rawmakase"))?;
    main.add_folder(&dir.join("home"))?;
    let trip = dir.join("Trip.rawmakase");
    Catalog::create(&trip)?.add_folder(&dir.join("trip"))?;
    Ok(Setup {
        _dir: d,
        dir,
        main,
        trip,
    })
}
fn id_of(catalog: &Catalog, name: &str) -> Result<PhotoId> {
    Ok(catalog
        .photos()?
        .into_iter()
        .find(|p| p.filename == name && p.master.is_none())
        .context("photo")?
        .id)
}
fn count(catalog: &Catalog, sql: &str) -> Result<i64> {
    Ok(catalog.db_for_tests().query_row(sql, [], |r| r.get(0))?)
}

#[test]
fn a_trip_catalog_joins_the_main_one_with_what_it_holds() -> Result<()> {
    let mut s = setup()?;
    let (uuid, copy) = {
        let mut trip = Catalog::open(s.trip.as_path())?;
        let t1 = id_of(&trip, "t1.ARW")?;
        let t2 = id_of(&trip, "t2.ARW")?;
        trip.db_mut_for_tests()
            .execute("UPDATE photos SET rating=4, label='Red' WHERE id=?", [t1.0])?;
        trip.add_keywords(&[t1], &[vec!["Places".into(), "Lisbon".into()]])?;
        trip.create_collection_of("Lisbon", None, &[t1, t2])?;
        let copy = trip.create_virtual_copy(t1)?;
        (
            trip.photo_uuid(t1)?.context("uuid")?,
            trip.photo_uuid(copy)?,
        )
    };
    let before = std::fs::read(&s.trip)?;

    let report = s.main.import_catalog(&s.trip, || true)?.context("began")?;

    assert_eq!(report.added, 3, "t1, its virtual copy and t2");
    assert_eq!(
        (report.present, report.missing.len(), report.unplaced.len()),
        (0, 0, 0)
    );
    assert_eq!(
        std::fs::read(&s.trip)?,
        before,
        "the trip catalog is only read"
    );
    let t1 = id_of(&s.main, "t1.ARW")?;
    let photo = s
        .main
        .photos()?
        .into_iter()
        .find(|p| p.id == t1)
        .context("t1")?;
    assert_eq!((photo.rating, photo.label.as_str()), (4, "Red"));
    assert_eq!(photo.path, s.dir.join("trip").join("t1.ARW"));
    let t2 = id_of(&s.main, "t2.ARW")?;
    let day2 = s
        .main
        .photos()?
        .into_iter()
        .find(|p| p.id == t2)
        .context("t2")?;
    assert_eq!(
        day2.path,
        s.dir.join("trip/day 2/t2.ARW"),
        "a folder below the root"
    );
    let roots = s.main.roots()?;
    assert_eq!(
        roots.len(),
        2,
        "home, and trip as Add Folder would add it: {roots:?}"
    );
    assert_eq!(
        s.main.photo_uuid(t1)?,
        Some(uuid),
        "the photo keeps its UUID"
    );
    let copies: Vec<_> = s
        .main
        .photos()?
        .into_iter()
        .filter(|p| p.master == Some(t1))
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(s.main.photo_uuid(copies[0].id)?, copy);
    let keywords = s.main.keywords(t1)?;
    assert!(keywords.iter().any(|k| k.name == "Lisbon"), "{keywords:?}");
    let lisbon = s
        .main
        .collections()?
        .into_iter()
        .find(|c| c.name == "Lisbon")
        .context("collection")?;
    let members = &s.main.collection_photos()?[&lisbon.id];
    assert_eq!(members.len(), 2);
    assert_eq!(count(&s.main, "SELECT count(*) FROM photos")?, 4);
    Ok(())
}

#[test]
fn importing_again_adds_nothing_and_changes_nothing() -> Result<()> {
    let mut s = setup()?;
    s.main.import_catalog(&s.trip, || true)?;
    let t1 = id_of(&s.main, "t1.ARW")?;
    s.main
        .db_mut_for_tests()
        .execute("UPDATE photos SET rating=5 WHERE id=?", [t1.0])?;

    let again = s.main.import_catalog(&s.trip, || true)?.context("began")?;

    assert_eq!((again.added, again.present), (0, 2));
    assert_eq!(count(&s.main, "SELECT count(*) FROM photos")?, 3);
    let rating: i64 =
        s.main
            .db_for_tests()
            .query_row("SELECT rating FROM photos WHERE id=?", [t1.0], |r| r.get(0))?;
    assert_eq!(rating, 5, "a photo the catalog has is left as it is");
    Ok(())
}

#[test]
fn a_photo_the_catalog_has_by_its_file_is_left() -> Result<()> {
    let mut s = setup()?;
    // The main catalog added the trip folder itself: other UUIDs, same files.
    s.main.add_folder(&s.dir.join("trip"))?;

    let report = s.main.import_catalog(&s.trip, || true)?.context("began")?;

    assert_eq!((report.added, report.present), (0, 2));
    assert_eq!(count(&s.main, "SELECT count(*) FROM photos")?, 3);
    Ok(())
}

#[test]
fn a_photo_whose_file_is_not_there_is_left_and_reported() -> Result<()> {
    let mut s = setup()?;
    std::fs::remove_file(s.dir.join("trip/day 2/t2.ARW"))?;

    let report = s.main.import_catalog(&s.trip, || true)?.context("began")?;

    assert_eq!(report.added, 1);
    assert_eq!(report.missing, vec![s.dir.join("trip/day 2/t2.ARW")]);
    Ok(())
}

#[test]
fn keywords_and_collections_are_found_by_their_path() -> Result<()> {
    let mut s = setup()?;
    let h1 = id_of(&s.main, "h1.ARW")?;
    s.main
        .add_keywords(&[h1], &[vec!["places".into(), "LISBON".into()]])?;
    s.main.create_collection_of("Lisbon", None, &[h1])?;
    {
        let mut trip = Catalog::open(s.trip.as_path())?;
        let t1 = id_of(&trip, "t1.ARW")?;
        trip.add_keywords(&[t1], &[vec!["Places".into(), "Lisbon".into()]])?;
        trip.create_collection_of("Lisbon", None, &[t1])?;
    }

    s.main.import_catalog(&s.trip, || true)?;

    assert_eq!(count(&s.main, "SELECT count(*) FROM keywords")?, 2);
    let t1 = id_of(&s.main, "t1.ARW")?;
    let lisbon: Vec<_> = s
        .main
        .collections()?
        .into_iter()
        .filter(|c| c.name == "Lisbon")
        .collect();
    assert_eq!(lisbon.len(), 1);
    let members = &s.main.collection_photos()?[&lisbon[0].id];
    assert!(members.contains(&h1) && members.contains(&t1));
    Ok(())
}

#[test]
fn stacks_and_snapshots_come_with_their_photos() -> Result<()> {
    let mut s = setup()?;
    {
        let mut trip = Catalog::open(s.trip.as_path())?;
        let t1 = id_of(&trip, "t1.ARW")?;
        // A new virtual copy is stacked with its photo.
        let copy = trip.create_virtual_copy(t1)?;
        assert_eq!(trip.stack_of(copy)?, vec![t1, copy]);
        trip.db_for_tests().execute(
            "INSERT INTO develop_snapshots(photo, name, recipe) VALUES (?, 'Before', NULL)",
            [t1.0],
        )?;
    }

    s.main.import_catalog(&s.trip, || true)?;

    let t1 = id_of(&s.main, "t1.ARW")?;
    let copy = s
        .main
        .photos()?
        .into_iter()
        .find(|p| p.master == Some(t1))
        .context("copy")?
        .id;
    assert_eq!(s.main.stack_of(copy)?, vec![t1, copy]);
    let snapshot: String = s.main.db_for_tests().query_row(
        "SELECT name FROM develop_snapshots WHERE photo=?",
        [t1.0],
        |r| r.get(0),
    )?;
    assert_eq!(snapshot, "Before");
    Ok(())
}

#[test]
fn nothing_is_written_when_the_import_does_not_begin() -> Result<()> {
    let mut s = setup()?;

    assert!(s.main.import_catalog(&s.trip, || false)?.is_none());

    assert_eq!(count(&s.main, "SELECT count(*) FROM photos")?, 1);
    assert_eq!(count(&s.main, "SELECT count(*) FROM import_batches")?, 1);
    Ok(())
}

#[test]
fn the_open_catalog_cannot_import_itself() -> Result<()> {
    let mut s = setup()?;
    let own = s.dir.join("Main.rawmakase");

    let error = s.main.import_catalog(&own, || true).unwrap_err();

    assert!(error.to_string().contains("open now"), "{error}");
    Ok(())
}

#[test]
fn a_catalog_a_later_release_added_to_is_refused() -> Result<()> {
    let mut s = setup()?;
    {
        let trip = Catalog::open(s.trip.as_path())?;
        trip.db_for_tests()
            .execute_batch("CREATE TABLE later_release(x INTEGER)")?;
    }

    let error = s.main.import_catalog(&s.trip, || true).unwrap_err();

    assert!(error.to_string().contains("later release"), "{error}");
    assert_eq!(count(&s.main, "SELECT count(*) FROM photos")?, 1);
    Ok(())
}

#[test]
fn a_saved_edit_and_the_mask_rasters_it_names_come_with_their_photo() -> Result<()> {
    let mut s = setup()?;
    let file = s.dir.join("trip").join("t1.ARW");
    let recipe = crate::model::recipe::Recipe {
        exposure: 1.25,
        ..Default::default()
    };
    let raster = crate::storage::bitmaps::Bitmap {
        width: 2,
        height: 1,
        channels: 1,
        depth: 1,
        data: vec![1, 254],
    };
    let mask = {
        let mut trip = Catalog::open(s.trip.as_path())?;
        let t1 = id_of(&trip, "t1.ARW")?;
        trip.save_edit(
            t1,
            &file,
            &crate::storage::Identity::read(&file)?,
            &recipe,
            &crate::export_settings::ExportOptions::default(),
            HistoryUpdate::Keep,
        )?;
        let mask = trip.put_bitmap(&raster)?;
        trip.db_for_tests().execute(
            "INSERT INTO develop_snapshots(photo, name, recipe) VALUES (?1, 'Masked', ?2)",
            rusqlite::params![t1.0, format!("{{\"mask\":\"{mask}\"}}")],
        )?;
        mask
    };

    s.main.import_catalog(&s.trip, || true)?;

    let t1 = id_of(&s.main, "t1.ARW")?;
    let edit = s.main.load_edit(t1, &file)?.context("edit")?;
    assert_eq!(edit.recipe, recipe);
    assert!(
        s.main.bitmap(&mask)?.is_some(),
        "the snapshot's raster came"
    );
    Ok(())
}

#[test]
fn a_folder_located_on_its_own_comes_from_where_it_is() -> Result<()> {
    let mut s = setup()?;
    // The trip catalog has its folder `day 2` somewhere outside its root.
    let elsewhere = s.dir.join("elsewhere").join("second day");
    std::fs::create_dir_all(&elsewhere)?;
    std::fs::rename(s.dir.join("trip/day 2/t2.ARW"), elsewhere.join("t2.ARW"))?;
    // With a folder of its own below it, which is found through it.
    std::fs::create_dir_all(s.dir.join("trip/day 2/raw"))?;
    std::fs::write(s.dir.join("trip/day 2/raw/t3.ARW"), b"t3.ARW")?;
    Catalog::open(s.trip.as_path())?.add_folder(&s.dir.join("trip"))?;
    std::fs::create_dir_all(elsewhere.join("raw"))?;
    std::fs::rename(
        s.dir.join("trip/day 2/raw/t3.ARW"),
        elsewhere.join("raw/t3.ARW"),
    )?;
    {
        let mut trip = Catalog::open(s.trip.as_path())?;
        let root = trip.roots()?[0].0;
        trip.relink_folder_at(root, "day 2", &elsewhere, Overrides::Keep)?;
    }

    let report = s.main.import_catalog(&s.trip, || true)?.context("began")?;

    assert_eq!((report.added, report.missing.len()), (3, 0));
    let t3 = id_of(&s.main, "t3.ARW")?;
    let t3 = s
        .main
        .photos()?
        .into_iter()
        .find(|p| p.id == t3)
        .context("t3")?;
    assert_eq!(t3.path, elsewhere.join("raw/t3.ARW"));
    let t2 = id_of(&s.main, "t2.ARW")?;
    let t2 = s
        .main
        .photos()?
        .into_iter()
        .find(|p| p.id == t2)
        .context("t2")?;
    assert_eq!(t2.path, elsewhere.join("t2.ARW"));
    let roots: Vec<String> = s.main.roots()?.into_iter().map(|r| r.1).collect();
    assert_eq!(
        roots.len(),
        3,
        "home, trip and the folder on its own: {roots:?}"
    );
    assert!(
        roots.contains(&elsewhere.to_string_lossy().into_owned()),
        "the folder on its own is its root, not the folder above it: {roots:?}"
    );
    Ok(())
}

#[test]
fn a_collection_named_as_another_kind_here_gets_a_number() -> Result<()> {
    let mut s = setup()?;
    s.main
        .create_collection(CollectionKind::Set, "Lisbon", None)?;
    {
        let mut trip = Catalog::open(s.trip.as_path())?;
        let t1 = id_of(&trip, "t1.ARW")?;
        trip.create_collection_of("Lisbon", None, &[t1])?;
    }

    s.main.import_catalog(&s.trip, || true)?;

    let mut names: Vec<String> = s
        .main
        .collections()?
        .into_iter()
        .filter(|c| c.name.starts_with("Lisbon"))
        .map(|c| c.name)
        .collect();
    names.sort();
    assert_eq!(names, ["Lisbon", "Lisbon 2"]);
    Ok(())
}

#[test]
fn rows_left_behind_by_a_removed_photo_stay_behind() -> Result<()> {
    let mut s = setup()?;
    {
        let trip = Catalog::open(s.trip.as_path())?;
        // An older release removed photo 999 without its rows.
        trip.db_for_tests().execute(
            "INSERT INTO local_edits(photo, data) VALUES (999, '{}')",
            [],
        )?;
        trip.db_for_tests().execute(
            "INSERT INTO photo_info(photo, camera) VALUES (999, 'Ghost')",
            [],
        )?;
    }

    s.main.import_catalog(&s.trip, || true)?;

    assert_eq!(count(&s.main, "SELECT count(*) FROM local_edits")?, 0);
    assert_eq!(
        count(
            &s.main,
            "SELECT count(*) FROM photo_info WHERE camera='Ghost'"
        )?,
        0
    );
    Ok(())
}

#[test]
fn importing_again_finds_the_collection_it_numbered() -> Result<()> {
    let mut s = setup()?;
    s.main
        .create_collection(CollectionKind::Set, "Lisbon", None)?;
    {
        let mut trip = Catalog::open(s.trip.as_path())?;
        let t1 = id_of(&trip, "t1.ARW")?;
        trip.create_collection_of("Lisbon", None, &[t1])?;
    }
    s.main.import_catalog(&s.trip, || true)?;
    // A photo added to the trip catalog since, and to its collection.
    std::fs::write(s.dir.join("trip/t4.ARW"), b"t4.ARW")?;
    {
        let mut trip = Catalog::open(s.trip.as_path())?;
        trip.add_folder(&s.dir.join("trip"))?;
        let t4 = id_of(&trip, "t4.ARW")?;
        let lisbon = trip
            .collections()?
            .into_iter()
            .find(|c| c.name == "Lisbon")
            .context("collection")?
            .id;
        trip.change_collection(lisbon, &[t4], &[])?;
    }

    s.main.import_catalog(&s.trip, || true)?;

    let lisbons: Vec<_> = s
        .main
        .collections()?
        .into_iter()
        .filter(|c| c.name.starts_with("Lisbon"))
        .collect();
    assert_eq!(lisbons.len(), 2, "the set and Lisbon 2: {lisbons:?}");
    let numbered = lisbons
        .iter()
        .find(|c| c.name == "Lisbon 2")
        .context("Lisbon 2")?;
    let t4 = id_of(&s.main, "t4.ARW")?;
    assert!(s.main.collection_photos()?[&numbered.id].contains(&t4));
    Ok(())
}

#[test]
fn a_photo_added_here_meanwhile_stops_the_import() -> Result<()> {
    let mut s = setup()?;
    // The main catalog's folder `trip` exists before the import reads.
    s.main.add_folder(&s.dir.join("trip"))?;
    s.main
        .db_mut_for_tests()
        .execute("DELETE FROM photos WHERE filename='t1.ARW'", [])?;
    let main = s.dir.join("Main.rawmakase");
    let trip = s.dir.join("trip");

    // Between the read and the write, another window adds t1 again.
    let error = s
        .main
        .import_catalog(&s.trip, || {
            Catalog::open(main.as_path())
                .and_then(|mut c| c.add_folder(&trip))
                .is_ok()
        })
        .unwrap_err();

    assert!(error.to_string().contains("meanwhile"), "{error}");
    assert_eq!(
        count(
            &s.main,
            "SELECT count(*) FROM photos WHERE filename='t1.ARW'"
        )?,
        1
    );
    Ok(())
}

#[test]
fn a_new_root_added_here_meanwhile_stops_the_import() -> Result<()> {
    let mut s = setup()?;
    let main = s.dir.join("Main.rawmakase");
    let trip = s.dir.join("trip");

    // `trip` is a new root when `reduce()` runs; another window adds it before
    // the transaction writes.
    let error = s
        .main
        .import_catalog(&s.trip, || {
            Catalog::open(main.as_path())
                .and_then(|mut c| c.add_folder(&trip))
                .is_ok()
        })
        .unwrap_err();

    assert!(error.to_string().contains("meanwhile"), "{error}");
    assert_eq!(s.main.roots()?.len(), 2, "home and the one trip root");
    assert_eq!(count(&s.main, "SELECT count(*) FROM photos")?, 3);
    Ok(())
}

#[test]
fn a_photo_added_by_uuid_meanwhile_stops_the_import() -> Result<()> {
    let mut s = setup()?;
    let main = s.dir.join("Main.rawmakase");
    let trip_uuid = {
        let trip = Catalog::open(s.trip.as_path())?;
        let t1 = id_of(&trip, "t1.ARW")?;
        trip.photo_uuid(t1)?.context("uuid")?
    };

    // Between `reduce()` and `copy_rows()`, another window assigns t1's UUID
    // to a photo in main.
    let error = s
        .main
        .import_catalog(&s.trip, || {
            let Ok(other) = Catalog::open(main.as_path()) else {
                return false;
            };
            let Ok(h1) = id_of(&other, "h1.ARW") else {
                return false;
            };
            other
                .db_for_tests()
                .execute(
                    "UPDATE photo_uuids SET uuid=?1 WHERE photo=?2",
                    rusqlite::params![trip_uuid.as_str(), h1.0],
                )
                .is_ok()
        })
        .unwrap_err();

    assert!(error.to_string().contains("meanwhile"), "{error}");
    assert_eq!(count(&s.main, "SELECT count(*) FROM photos")?, 1);
    Ok(())
}

#[test]
fn duplicate_source_folders_for_one_directory_do_not_duplicate_masters() -> Result<()> {
    let mut s = setup()?;
    {
        let trip = Catalog::open(s.trip.as_path())?;
        let root = trip.roots()?[0].0;
        let db = trip.db_for_tests();
        // A second folder row in the source catalog resolving to the same
        // directory with another master row for t1.ARW (different UUID).
        db.execute(
            "INSERT INTO folders(root, relative_path) VALUES (?, '')",
            [root.0],
        )?;
        let dup_folder: i64 = db.query_row("SELECT max(id) FROM folders", [], |r| r.get(0))?;
        db.execute(
            "INSERT INTO folder_paths(folder, path) VALUES (?, '')",
            [dup_folder],
        )?;
        db.execute(
            "INSERT INTO photos(folder, filename, original_path) VALUES (?1, 't1.ARW', ?2)",
            rusqlite::params![dup_folder, s.dir.join("trip/t1.ARW").to_string_lossy()],
        )?;
    }

    let report = s.main.import_catalog(&s.trip, || true)?.context("began")?;

    assert_eq!(report.added, 2, "t1 once and t2");
    assert_eq!(report.present, 1, "the duplicate t1 is left out");
    assert_eq!(
        count(
            &s.main,
            "SELECT count(*) FROM photos WHERE filename='t1.ARW'"
        )?,
        1
    );
    Ok(())
}

#[test]
fn orphaned_keywords_collections_stacks_and_virtual_copies_stay_behind() -> Result<()> {
    let mut s = setup()?;
    // Put t1 in main already so a stack [t1, t1b] in trip loses t1 and becomes
    // a single-photo stack during reduce().
    std::fs::write(s.dir.join("trip/t1b.ARW"), b"t1b.ARW")?;
    let ghost_dir = s.dir.join("trip/ghost");
    std::fs::create_dir_all(&ghost_dir)?;
    {
        let mut trip = Catalog::open(s.trip.as_path())?;
        trip.add_folder(&s.dir.join("trip"))?;
        let t1 = id_of(&trip, "t1.ARW")?;
        let t1b = id_of(&trip, "t1b.ARW")?;
        trip.change_stacks(&StackChange::Group(vec![t1, t1b]))?;
        let root = trip.roots()?[0].0;
        let db = trip.db_for_tests();
        db.execute_batch("PRAGMA foreign_keys=OFF;")?;
        // Orphaned keyword, collection, and stack rows for removed photo 999.
        db.execute(
            "INSERT INTO keywords(id, name, parent) VALUES (900, 'GhostKw', NULL)",
            [],
        )?;
        db.execute(
            "INSERT INTO photo_keywords(photo, keyword) VALUES (999, 900)",
            [],
        )?;
        db.execute(
            "INSERT INTO collections(id, name, parent, kind)
             VALUES (900, 'GhostCol', NULL, 'com.adobe.ag.library.collection')",
            [],
        )?;
        db.execute(
            "INSERT INTO collection_photos(collection, photo) VALUES (900, 999)",
            [],
        )?;
        db.execute("INSERT INTO stacks(id, collapsed) VALUES (900, 1)", [])?;
        db.execute(
            "INSERT INTO stack_photos(photo, stack, position) VALUES (999, 900, 0)",
            [],
        )?;
        // Orphaned virtual copy in its own folder whose master 999 is gone.
        db.execute(
            "INSERT INTO folders(root, relative_path) VALUES (?, 'ghost')",
            [root.0],
        )?;
        let ghost_folder: i64 = db.query_row("SELECT max(id) FROM folders", [], |r| r.get(0))?;
        db.execute(
            "INSERT INTO folder_paths(folder, path) VALUES (?, 'ghost')",
            [ghost_folder],
        )?;
        db.execute(
            "INSERT INTO photos(folder, filename, original_path, copy_name, master_id)
             VALUES (?1, 'gone.ARW', ?2, 'Copy 1', 999)",
            rusqlite::params![ghost_folder, ghost_dir.join("gone.ARW").to_string_lossy()],
        )?;
        db.execute_batch("PRAGMA foreign_keys=ON;")?;
    }
    // Import once with t1 already in main by UUID.
    let t1_uuid = {
        let trip = Catalog::open(s.trip.as_path())?;
        let t1 = id_of(&trip, "t1.ARW")?;
        trip.photo_uuid(t1)?.context("uuid")?
    };
    let h1 = id_of(&s.main, "h1.ARW")?;
    s.main.db_mut_for_tests().execute(
        "UPDATE photo_uuids SET uuid=?1 WHERE photo=?2",
        rusqlite::params![t1_uuid.as_str(), h1.0],
    )?;

    let report = s.main.import_catalog(&s.trip, || true)?.context("began")?;

    assert_eq!(report.added, 2, "t1b and t2 only; orphan copy left behind");
    assert_eq!(
        count(
            &s.main,
            "SELECT count(*) FROM keywords WHERE name='GhostKw'"
        )?,
        0
    );
    assert_eq!(
        count(
            &s.main,
            "SELECT count(*) FROM collections WHERE name='GhostCol'"
        )?,
        0
    );
    assert_eq!(
        count(&s.main, "SELECT count(*) FROM stacks")?,
        0,
        "orphan and single-photo stacks are not copied"
    );
    assert_eq!(count(&s.main, "SELECT count(*) FROM stack_photos")?, 0);
    Ok(())
}

#[test]
fn a_new_root_does_not_enclose_an_existing_root() -> Result<()> {
    let mut s = setup()?;
    // Main already has `trip/day 2` as its own root.
    s.main.add_folder(&s.dir.join("trip/day 2"))?;
    // Trip also has a sibling subfolder `trip/day 3` with a nested `sub` folder
    // alongside `trip/t1.ARW`.
    std::fs::create_dir_all(s.dir.join("trip/day 3/sub"))?;
    std::fs::write(s.dir.join("trip/day 3/t3.ARW"), b"t3.ARW")?;
    std::fs::write(s.dir.join("trip/day 3/sub/t4.ARW"), b"t4.ARW")?;
    Catalog::open(s.trip.as_path())?.add_folder(&s.dir.join("trip"))?;

    let report = s.main.import_catalog(&s.trip, || true)?.context("began")?;

    assert_eq!(report.added, 2, "t3 in trip/day 3 and t4 in trip/day 3/sub");
    assert_eq!(report.present, 1, "t2 in trip/day 2 is already in main");
    assert_eq!(
        report.unplaced,
        vec![s.dir.join("trip/t1.ARW")],
        "trip/ itself holds existing root trip/day 2, so it cannot become a root"
    );
    let roots: Vec<PathBuf> = s
        .main
        .roots()?
        .into_iter()
        .map(|r| PathBuf::from(r.1))
        .collect();
    assert!(
        !roots.contains(&s.dir.join("trip")),
        "enclosing root was not created: {roots:?}"
    );
    assert!(
        roots.contains(&s.dir.join("trip/day 3")),
        "shallowest non-enclosing folder became one root: {roots:?}"
    );
    assert!(
        !roots.contains(&s.dir.join("trip/day 3/sub")),
        "subfolder stays under trip/day 3 instead of creating a nested root: {roots:?}"
    );
    let t4 = id_of(&s.main, "t4.ARW")?;
    let t4_photo = s
        .main
        .photos()?
        .into_iter()
        .find(|p| p.id == t4)
        .context("t4")?;
    assert_eq!(t4_photo.path, s.dir.join("trip/day 3/sub/t4.ARW"));
    Ok(())
}

#[test]
fn smart_collections_of_the_other_catalog_stay_behind() -> Result<()> {
    let mut s = setup()?;
    s.main.add_default_smart_collections()?;
    let video = s
        .main
        .collections()?
        .into_iter()
        .find(|c| c.name == "Video Files")
        .context("Video Files")?
        .id;
    s.main.remove_collections(&[video])?;
    Catalog::open(s.trip.as_path())?.add_default_smart_collections()?;

    s.main.import_catalog(&s.trip, || true)?;

    assert!(
        !s.main
            .collections()?
            .iter()
            .any(|c| c.name == "Video Files"),
        "deleted default smart collection is not recreated by importing another catalog"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_file_symlink_keeps_its_link_directory_and_name() -> Result<()> {
    let mut s = setup()?;
    let archive = s.dir.join("archive");
    std::fs::create_dir_all(&archive)?;
    let real = archive.join("real.ARW");
    std::fs::write(&real, b"real.ARW")?;
    let link = s.dir.join("trip/link.ARW");
    std::os::unix::fs::symlink(&real, &link)?;
    {
        let trip = Catalog::open(s.trip.as_path())?;
        let folder = trip.folders()?[0].id;
        trip.db_for_tests().execute(
            "INSERT INTO photos(folder, filename, original_path) VALUES (?1, 'link.ARW', ?2)",
            rusqlite::params![folder.0, link.to_string_lossy()],
        )?;
    }

    let report = s.main.import_catalog(&s.trip, || true)?.context("began")?;

    assert_eq!(report.added, 3);
    assert!(report.unplaced.is_empty(), "{:?}", report.unplaced);
    let imported = s
        .main
        .photos()?
        .into_iter()
        .find(|p| p.filename == "link.ARW")
        .context("link.ARW")?;
    assert_eq!(imported.path, link);
    assert!(imported.path.is_file());
    Ok(())
}

#[test]
fn a_case_differing_filename_already_present_is_skipped() -> Result<()> {
    let mut s = setup()?;
    // Only applies on case-insensitive filesystems where `t1.arw` aliases `t1.ARW`.
    if !s.dir.join("trip/t1.arw").is_file() {
        return Ok(());
    }
    s.main.add_folder(&s.dir.join("trip"))?;
    // Main stores `t1.ARW` (disk case) and `t2.arw` (differing case), while trip
    // stores the opposite casing for each so both directions exercise the
    // case-folded lookup rather than the exact-name fast path.
    s.main.db_mut_for_tests().execute(
        "UPDATE photos SET filename='t2.arw' WHERE filename='t2.ARW'",
        [],
    )?;
    {
        let trip = Catalog::open(s.trip.as_path())?;
        trip.db_for_tests().execute(
            "UPDATE photos SET filename='t1.arw' WHERE filename='t1.ARW'",
            [],
        )?;
    }

    let report = s.main.import_catalog(&s.trip, || true)?.context("began")?;

    assert_eq!((report.added, report.present), (0, 2));
    assert_eq!(count(&s.main, "SELECT count(*) FROM photos")?, 3);
    Ok(())
}

/// A photo that came from Lightroom into the other catalog (no import
/// batch there) and whose file it hadn't read yet gets no camera GPS once
/// imported here, as it wouldn't there: it is the Previous Import, not a
/// photo added from files.
#[test]
fn a_photo_imported_from_another_catalog_gets_no_camera_gps_in_the_background() -> Result<()> {
    let mut s = setup()?;
    {
        let trip = Catalog::open(s.trip.as_path())?;
        let t1 = id_of(&trip, "t1.ARW")?;
        // As a Lightroom import leaves it: no batch, its file not read.
        trip.db_for_tests()
            .execute("DELETE FROM photo_imports WHERE photo=?", [t1.0])?;
        trip.db_for_tests()
            .execute("UPDATE photo_import_facts SET exif_read=0", [])?;
    }
    s.main.import_catalog(&s.trip, || true)?.context("began")?;
    let t1 = id_of(&s.main, "t1.ARW")?;
    let previous = s.main.previous_import()?;
    assert!(previous.contains(&t1), "{previous:?}");
    assert!(s.main.photos_to_read_exif_capture()?.contains(&t1));
    let location = crate::metadata::Location::At {
        lat: 40.7,
        lon: -74.,
        alt: None,
    };
    let located = s
        .main
        .record_capture_reads(&[(t1, None)], &[(t1, location)])?;
    assert!(located.is_empty(), "{located:?}");
    assert_eq!(s.main.descriptive(t1)?.location, None);
    // Its import facts came as they were, still to read, and that read
    // completes them.
    assert!(!s.main.photos_to_read_exif_capture()?.contains(&t1));
    Ok(())
}
