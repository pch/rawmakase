//! Import from Catalog: a trip catalog's photos joining the main one.
use super::*;
use anyhow::Context as _;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// What Import from Catalog does with each table of the other catalog.
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
    ("smart_collection_rules", Carried::Copied),
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
        "say in TABLES what Import from Catalog does with each table"
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
