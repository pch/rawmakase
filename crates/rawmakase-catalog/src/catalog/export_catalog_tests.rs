use super::*;
use crate::export_settings::ExportOptions;
use crate::model::recipe::Recipe;
use anyhow::Context as _;
use std::io::Write as _;
use std::path::PathBuf;

/// A catalog of root `photos` with folders `A` (a1, a2) and `B` (b1), and the
/// ids of a1, a2 and b1.
struct Shoots {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    catalog: Catalog,
    a: FolderId,
    b: FolderId,
    a1: PhotoId,
    a2: PhotoId,
    b1: PhotoId,
}
fn shoots() -> Result<Shoots> {
    let d = tempfile::tempdir()?;
    let dir = d.path().to_path_buf();
    let photos = dir.join("photos");
    for (folder, name) in [("A", "a1.ARW"), ("A", "a2.ARW"), ("B", "b1.ARW")] {
        std::fs::create_dir_all(photos.join(folder))?;
        std::fs::write(photos.join(folder).join(name), name.as_bytes())?;
    }
    let mut catalog = Catalog::create(&dir.join("Photos.rawmakase"))?;
    catalog.add_folder(&photos)?;
    let folder = |name: &str| -> Result<FolderId> {
        Ok(catalog
            .folders()?
            .into_iter()
            .find(|f| f.path.ends_with(name))
            .context("folder")?
            .id)
    };
    let (a, b) = (folder("A")?, folder("B")?);
    let photo = |name: &str| -> Result<PhotoId> {
        Ok(catalog
            .photos()?
            .into_iter()
            .find(|p| p.filename == name)
            .context("photo")?
            .id)
    };
    let (a1, a2, b1) = (photo("a1.ARW")?, photo("a2.ARW")?, photo("b1.ARW")?);
    Ok(Shoots {
        _dir: d,
        dir,
        catalog,
        a,
        b,
        a1,
        a2,
        b1,
    })
}
fn count(catalog: &Catalog, sql: &str) -> Result<i64> {
    Ok(catalog.db_for_tests().query_row(sql, [], |r| r.get(0))?)
}
fn edited(exposure: f32) -> Recipe {
    Recipe {
        exposure,
        ..Default::default()
    }
}

#[test]
fn an_exported_folder_keeps_everything_of_its_photos_and_nothing_of_others() -> Result<()> {
    let mut s = shoots()?;
    let c = &mut s.catalog;
    c.set_metadata(s.a1, 4, 1, "Red")?;
    c.set_metadata(s.b1, 2, 0, "Blue")?;
    let a1_path = s.dir.join("photos/A/a1.ARW");
    c.save_edit(
        s.a1,
        &a1_path,
        &crate::storage::Identity::read(&a1_path)?,
        &edited(1.25),
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    let copy = c.create_virtual_copy(s.a1)?;
    c.add_keywords(&[s.a1], &[vec!["Places".into(), "Paris".into()]])?;
    c.add_keywords(&[s.b1], &[vec!["Secret".into()]])?;
    let quick = c.quick_collection()?;
    c.change_collection(quick, &[s.a1, s.b1], &[])?;
    let destination = s.dir.join("out/Shoot A.rawmakase");

    let export = c.export_catalog(&ExportScope::Folders(vec![s.a]), &destination)?;

    assert_eq!(export.photos, 3);
    assert_eq!(export.family, 0);
    assert!(export.offline.is_empty());
    assert_eq!(export.collection, "Shoot A");
    let out = Catalog::open(&destination)?;
    let mut names: Vec<_> = out
        .photos()?
        .into_iter()
        .map(|p| (p.filename, p.copy_name, p.rating, p.label))
        .collect();
    names.sort();
    assert_eq!(names.len(), 3);
    assert_eq!(names[0].0, "a1.ARW");
    assert_eq!((names[0].2, names[0].3.as_str()), (4, "Red"));
    assert!(names.iter().all(|n| n.0.starts_with('a')));
    assert_eq!(
        out.load_edit(s.a1, &a1_path)?.context("edit")?.recipe,
        edited(1.25)
    );
    assert!(out.photos()?.iter().any(|p| p.id == copy));
    let paths: Vec<Vec<String>> = out.keywords(s.a1)?.into_iter().map(|k| k.path).collect();
    assert_eq!(paths, [vec!["Places".to_string(), "Paris".to_string()]]);
    assert_eq!(count(&out, "SELECT count(*) FROM keywords")?, 2);
    // The Quick Collection keeps its own member; a new one holds them all.
    let members = out.collection_photos()?;
    assert_eq!(members[&quick].len(), 1);
    let all = out
        .collections()?
        .into_iter()
        .find(|c| c.name == "Shoot A")
        .context("collection")?;
    assert_eq!(members[&all.id].len(), 3);
    // Folder B, and anything of b1, is not there.
    let folders: Vec<FolderId> = out.folders()?.into_iter().map(|f| f.id).collect();
    assert!(folders.contains(&s.a) && !folders.contains(&s.b));
    assert_eq!(
        count(&out, "SELECT count(*) FROM photos WHERE filename='b1.ARW'")?,
        0
    );
    let bytes = std::fs::read(&destination)?;
    assert!(!bytes.windows(6).any(|w| w == b"Secret"));
    // This catalog is as it was, and nothing else was left beside the export.
    assert_eq!(c.photos()?.len(), 4);
    assert_eq!(std::fs::read_dir(s.dir.join("out"))?.count(), 1);
    Ok(())
}

#[test]
fn a_virtual_copy_chosen_takes_its_master_and_siblings() -> Result<()> {
    let mut s = shoots()?;
    let copy = s.catalog.create_virtual_copy(s.b1)?;
    let other = s.catalog.create_virtual_copy(s.b1)?;
    let destination = s.dir.join("copy.rawmakase");
    let export = s
        .catalog
        .export_catalog(&ExportScope::Photos(vec![copy]), &destination)?;
    assert_eq!((export.photos, export.family), (3, 2));
    assert!(export.removal.is_none());
    let out = Catalog::open(&destination)?;
    let mut ids: Vec<PhotoId> = out.photos()?.into_iter().map(|p| p.id).collect();
    ids.sort();
    let mut expected = vec![s.b1, copy, other];
    expected.sort();
    assert_eq!(ids, expected);
    Ok(())
}

#[test]
fn mask_rasters_go_only_with_the_edits_history_and_snapshots_that_name_them() -> Result<()> {
    let mut s = shoots()?;
    let raster = |seed: u8| crate::storage::bitmaps::Bitmap {
        width: 2,
        height: 1,
        channels: 1,
        depth: 1,
        data: vec![seed, 255 - seed],
    };
    let in_snapshot = s.catalog.put_bitmap(&raster(1))?;
    let in_history = s.catalog.put_bitmap(&raster(2))?;
    let elsewhere = s.catalog.put_bitmap(&raster(3))?;
    let db = s.catalog.db_for_tests();
    db.execute(
        "INSERT INTO develop_snapshots(photo, name, recipe) VALUES (?1, 'a', ?2), (?3, 'b', ?4)",
        rusqlite::params![
            s.a1.0,
            format!("{{\"mask\":\"{in_snapshot}\"}}"),
            s.b1.0,
            format!("{{\"mask\":\"{elsewhere}\"}}")
        ],
    )?;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    write!(z, "{{\"version\":99,\"steps\":[\"{in_history}\"]}}")?;
    db.execute(
        "INSERT INTO develop_history(photo, data) VALUES (?1, ?2)",
        rusqlite::params![s.a2.0, z.finish()?],
    )?;
    let destination = s.dir.join("masks.rawmakase");
    let export = s
        .catalog
        .export_catalog(&ExportScope::Folders(vec![s.a]), &destination)?;
    assert!(!export.kept_every_mask);
    let out = Catalog::open(&destination)?;
    assert!(out.bitmap(&in_snapshot)?.is_some());
    assert!(out.bitmap(&in_history)?.is_some());
    assert!(out.bitmap(&elsewhere)?.is_none());

    // A History that can't be read keeps every raster rather than lose one.
    s.catalog.db_for_tests().execute(
        "INSERT INTO develop_history(photo, data) VALUES (?1, X'00FF')",
        [s.a1.0],
    )?;
    let destination = s.dir.join("masks 2.rawmakase");
    let export = s
        .catalog
        .export_catalog(&ExportScope::Folders(vec![s.a]), &destination)?;
    assert!(export.kept_every_mask);
    assert!(Catalog::open(&destination)?.bitmap(&elsewhere)?.is_some());
    Ok(())
}

#[test]
fn an_exported_folder_is_removed_only_while_it_is_what_was_exported() -> Result<()> {
    let mut s = shoots()?;
    let export = s
        .catalog
        .export_catalog(&ExportScope::Folders(vec![s.a]), &s.dir.join("a.rawmakase"))?;
    let removal = export.removal.context("removal")?;
    assert_eq!(removal.folders(), [s.a]);

    // Rated since: the export doesn't have it.
    s.catalog.set_metadata(s.a2, 5, 0, "")?;
    assert!(s.catalog.remove_exported_folders(&removal).is_err());
    s.catalog.set_metadata(s.a2, 0, 0, "")?;
    assert_eq!(s.catalog.photos()?.len(), 3);

    let mut removed = s.catalog.remove_exported_folders(&removal)?;
    removed.sort();
    let mut expected = [s.a1, s.a2];
    expected.sort();
    assert_eq!(removed, expected);
    let left: Vec<PhotoId> = s.catalog.photos()?.into_iter().map(|p| p.id).collect();
    assert_eq!(left, [s.b1]);

    // A virtual copy made since, or a photo added to the folder.
    for change in 0..2 {
        let mut s = shoots()?;
        let export = s
            .catalog
            .export_catalog(&ExportScope::Folders(vec![s.a]), &s.dir.join("a.rawmakase"))?;
        match change {
            0 => drop(s.catalog.create_virtual_copy(s.a1)?),
            _ => {
                std::fs::write(s.dir.join("photos/A/a3.ARW"), b"a3")?;
                s.catalog.add_folder(&s.dir.join("photos"))?;
            }
        }
        let error = s
            .catalog
            .remove_exported_folders(&export.removal.context("removal")?)
            .unwrap_err();
        assert!(error.to_string().contains("changed since it was exported"));
        assert_eq!(s.catalog.photos()?.len(), 4);
    }
    Ok(())
}

#[test]
fn a_failed_export_writes_nothing() -> Result<()> {
    let s = shoots()?;
    let taken = s.dir.join("taken.rawmakase");
    std::fs::write(&taken, b"mine")?;
    assert!(
        s.catalog
            .export_catalog(&ExportScope::Folders(vec![s.a]), &taken)
            .is_err()
    );
    assert_eq!(std::fs::read(&taken)?, b"mine");

    // A table a later release added can't be told apart into photos kept
    // and left out.
    s.catalog
        .db_for_tests()
        .execute_batch("CREATE TABLE future_things(photo INTEGER, data TEXT)")?;
    let out = s.dir.join("out");
    let error = s
        .catalog
        .export_catalog(&ExportScope::Folders(vec![s.a]), &out.join("a.rawmakase"))
        .unwrap_err();
    assert!(format!("{error:#}").contains("future_things"));
    assert_eq!(std::fs::read_dir(&out)?.count(), 0);

    assert!(
        s.catalog
            .export_catalog(
                &ExportScope::Photos(vec![PhotoId(9999)]),
                &s.dir.join("none.rawmakase")
            )
            .is_err()
    );
    assert!(!s.dir.join("none.rawmakase").exists());
    Ok(())
}

#[test]
fn an_exported_lightroom_catalog_leaves_the_archived_lrcat_behind() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("trip.lrcat");
    super::tests::fixture(&source)?;
    let imported = d.path().join("trip.rawmakase");
    super::lightroom::import_lightroom(&source, &imported)?;
    let catalog = Catalog::open(&imported)?;
    assert_eq!(count(&catalog, "SELECT count(*) FROM sources")?, 1);
    let destination = d.path().join("one.rawmakase");
    let export = catalog.export_catalog(&ExportScope::Photos(vec![PhotoId(41)]), &destination)?;
    // The copy took its master.
    assert_eq!((export.photos, export.family), (2, 1));
    let out = Catalog::open(&destination)?;
    assert_eq!(count(&out, "SELECT count(*) FROM sources")?, 0);
    assert_eq!(
        count(
            &out,
            "SELECT count(*) FROM photos WHERE lightroom_develop IS NOT NULL"
        )?,
        2
    );
    let keywords: Vec<Vec<String>> = out
        .keywords(PhotoId(40))?
        .into_iter()
        .map(|k| k.path)
        .collect();
    assert_eq!(keywords, [vec!["City".to_string()]]);
    Ok(())
}

#[test]
fn locations_of_folders_left_out_do_not_go_with_the_export() -> Result<()> {
    let s = shoots()?;
    let root = s.catalog.folders()?[0].root;
    // A folder located on its own with no catalog folder of its own, and a
    // root with no folder left.
    let db = s.catalog.db_for_tests();
    db.execute(
        "INSERT INTO folder_locations(root, relative_path, computer, path)
         VALUES (?1, 'Secret', 'test', '/Volumes/private-client')",
        [root.0],
    )?;
    db.execute(
        "INSERT INTO roots(original_path) VALUES ('/Volumes/Elsewhere')",
        [],
    )?;
    let destination = s.dir.join("a.rawmakase");
    s.catalog
        .export_catalog(&ExportScope::Folders(vec![s.a]), &destination)?;
    let out = Catalog::open(&destination)?;
    assert_eq!(
        count(
            &out,
            "SELECT count(*) FROM folder_locations WHERE relative_path<>''"
        )?,
        0
    );
    assert_eq!(count(&out, "SELECT count(*) FROM roots")?, 1);
    let bytes = std::fs::read(&destination)?;
    for secret in [&b"private-client"[..], b"Elsewhere"] {
        assert!(!bytes.windows(secret.len()).any(|w| w == secret));
    }
    Ok(())
}

#[test]
fn photos_below_a_folder_found_elsewhere_are_found_there_in_the_export() -> Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir_all(photos.join("Parent/Kept"))?;
    std::fs::write(photos.join("Parent/p.ARW"), b"p")?;
    std::fs::write(photos.join("Parent/Kept/k.ARW"), b"k")?;
    let mut catalog = Catalog::create(&d.path().join("c.rawmakase"))?;
    catalog.add_folder(&photos)?;
    // Parent moved, and found where it went.
    let moved = d.path().join("moved");
    std::fs::create_dir_all(&moved)?;
    std::fs::rename(photos.join("Parent"), moved.join("Parent"))?;
    let root = catalog.folders()?[0].root;
    catalog.relink_folder_at(
        root,
        "Parent",
        &moved.join("Parent"),
        crate::catalog::Overrides::Keep,
    )?;
    let k = catalog
        .photos()?
        .into_iter()
        .find(|p| p.filename == "k.ARW")
        .context("k")?;
    assert!(k.path.exists());
    let destination = d.path().join("k.rawmakase");
    let export = catalog.export_catalog(&ExportScope::Photos(vec![k.id]), &destination)?;
    assert!(export.offline.is_empty());
    let out = Catalog::open(&destination)?;
    let paths: Vec<PathBuf> = out.photos()?.into_iter().map(|p| p.path).collect();
    assert_eq!(paths, [k.path]);
    Ok(())
}

#[test]
fn an_exported_folder_found_elsewhere_since_is_not_removed() -> Result<()> {
    let mut s = shoots()?;
    let export = s
        .catalog
        .export_catalog(&ExportScope::Folders(vec![s.a]), &s.dir.join("a.rawmakase"))?;
    let root = s.catalog.folders()?[0].root;
    let moved = s.dir.join("moved-A");
    std::fs::rename(s.dir.join("photos/A"), &moved)?;
    s.catalog
        .relink_folder_at(root, "A", &moved, crate::catalog::Overrides::Keep)?;
    assert!(
        s.catalog
            .remove_exported_folders(&export.removal.context("removal")?)
            .is_err()
    );
    assert_eq!(s.catalog.photos()?.len(), 3);
    Ok(())
}
