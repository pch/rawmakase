//! Each photo's own UUID: made once, for every photo and virtual copy,
//! never moved or given to another photo, and filled in for photos that
//! releases before it added.
use super::lightroom::import_lightroom;
use super::upgrade_fixtures::Earlier;
use super::*;
use crate::ids::PhotoUuid;
use rusqlite::Connection;
use std::path::PathBuf;

/// A catalog of folder `photos`, holding a.ARW and, in subfolder `b`,
/// b.ARW.
fn catalog() -> Result<(tempfile::TempDir, PathBuf, Catalog)> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir_all(photos.join("b"))?;
    std::fs::write(photos.join("a.ARW"), "synthetic a")?;
    std::fs::write(photos.join("b/b.ARW"), "synthetic b")?;
    let path = dir.path().join("Photos.rawmakase");
    let mut cat = Catalog::create(&path)?;
    cat.add_folder(&photos)?;
    Ok((dir, path, cat))
}

fn photo(cat: &Catalog, name: &str) -> PhotoId {
    cat.photos()
        .unwrap()
        .into_iter()
        .find(|p| p.filename == name && p.master.is_none())
        .unwrap()
        .id
}

fn uuid(cat: &Catalog, id: PhotoId) -> PhotoUuid {
    cat.photo_uuid(id).unwrap().expect("a UUID")
}

/// Every photo's UUID, by id, as the file at `path` holds them.
fn stored(path: &Path) -> Result<Vec<(i64, String)>> {
    let db = Connection::open(path)?;
    let mut statement = db.prepare(
        "SELECT p.id, u.uuid FROM photos p LEFT JOIN photo_uuids u ON u.photo=p.id ORDER BY p.id",
    )?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// A random (version 4) UUID, lowercase and hyphenated.
fn assert_random(uuid: &str) {
    assert_eq!(
        PhotoUuid::parse(uuid).map(|u| u.to_string()),
        Some(uuid.into())
    );
    let bytes = uuid.as_bytes();
    assert_eq!(bytes[14], b'4', "{uuid}");
    assert!(b"89ab".contains(&bytes[19]), "{uuid}");
}

#[test]
fn an_imported_photo_gets_a_random_lowercase_uuid() -> Result<()> {
    let (_dir, path, cat) = catalog()?;
    let a = uuid(&cat, photo(&cat, "a.ARW"));
    let b = uuid(&cat, photo(&cat, "b.ARW"));
    assert_random(a.as_str());
    assert_random(b.as_str());
    assert_ne!(a, b);
    assert!(stored(&path)?.iter().all(|(_, u)| !u.is_empty()));
    Ok(())
}

#[test]
fn a_virtual_copy_gets_its_own_uuid() -> Result<()> {
    let (_dir, _path, mut cat) = catalog()?;
    let a = photo(&cat, "a.ARW");
    let copy = cat.create_virtual_copy(a)?;
    assert_random(uuid(&cat, copy).as_str());
    assert_ne!(uuid(&cat, copy), uuid(&cat, a));
    Ok(())
}

#[test]
fn a_lightroom_import_publishes_a_uuid_for_every_photo_and_copy() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.lrcat");
    super::tests::fixture(&source)?;
    let output = dir.path().join("Photos.rawmakase");
    import_lightroom(&source, &output)?;
    // Read as published, before any open could fill them in.
    let rows = stored(&output)?;
    assert_eq!(rows.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [40, 41]);
    for (_, uuid) in &rows {
        assert_random(uuid);
    }
    assert_ne!(rows[0].1, rows[1].1);
    Ok(())
}

#[test]
fn set_copy_as_master_leaves_each_uuid_with_its_photo() -> Result<()> {
    let (_dir, _path, mut cat) = catalog()?;
    let a = photo(&cat, "a.ARW");
    let copy = cat.create_virtual_copy(a)?;
    let (master_uuid, copy_uuid) = (uuid(&cat, a), uuid(&cat, copy));
    cat.set_copy_as_master(copy)?;
    assert_eq!(
        cat.photos()?.iter().find(|p| p.id == a).unwrap().master,
        Some(copy)
    );
    assert_eq!(uuid(&cat, a), master_uuid);
    assert_eq!(uuid(&cat, copy), copy_uuid);
    Ok(())
}

#[test]
fn a_photo_reusing_a_removed_photos_id_gets_a_new_uuid() -> Result<()> {
    let (dir, path, mut cat) = catalog()?;
    // Added last, so it has the highest id.
    let later = dir.path().join("later");
    std::fs::create_dir(&later)?;
    std::fs::write(later.join("l.ARW"), "synthetic l")?;
    cat.add_folder(&later)?;
    let photos = cat.photos()?;
    let l = photos.iter().find(|p| p.filename == "l.ARW").unwrap();
    assert_eq!(Some(l.id), photos.iter().map(|p| p.id).max());
    let (b, folder) = (l.id, l.folder);
    let removed = uuid(&cat, b);
    cat.remove_folders(&[folder])?;
    assert_eq!(cat.photo_by_uuid(&removed)?, None);
    // An older release, which ignores `photo_id_high`, gives the id again.
    drop(cat);
    let db = Connection::open(&path)?;
    db.execute(
        "INSERT INTO photos(id, folder, filename, original_path)
         SELECT ?1, folder, 'c.ARW', 'c.ARW' FROM photos LIMIT 1",
        [b.0],
    )?;
    drop(db);
    let cat = Catalog::open(&path)?;
    let c = photo(&cat, "c.ARW");
    assert_eq!(c, b);
    assert_ne!(uuid(&cat, c), removed);
    assert_eq!(cat.photo_by_uuid(&removed)?, None);
    Ok(())
}

#[test]
fn photos_an_older_release_adds_get_a_uuid_on_the_next_open() -> Result<()> {
    let (_dir, path, cat) = catalog()?;
    drop(cat);
    let db = Connection::open(&path)?;
    db.execute(
        "INSERT INTO photos(folder, filename, original_path) SELECT folder, 'c.ARW', 'c.ARW'
         FROM photos LIMIT 1",
        [],
    )?;
    let added = PhotoId(db.last_insert_rowid());
    drop(db);
    let cat = Catalog::open(&path)?;
    assert_random(uuid(&cat, added).as_str());
    assert_eq!(cat.photo_by_uuid(&uuid(&cat, added))?, Some(added));
    drop(cat);
    // An older release removes it on a connection of its own, which turns
    // foreign keys on as every release does: its UUID goes with it.
    let db = Connection::open(&path)?;
    db.execute_batch("PRAGMA foreign_keys=ON")?;
    db.execute("DELETE FROM photos WHERE id=?", [added.0])?;
    let left: i64 = db.query_row(
        "SELECT count(*) FROM photo_uuids WHERE photo=?",
        [added.0],
        |r| r.get(0),
    )?;
    assert_eq!(left, 0);
    Ok(())
}

#[test]
fn a_photo_without_a_uuid_yet_is_reported_missing_by_reads() -> Result<()> {
    let (_dir, path, cat) = catalog()?;
    let a = photo(&cat, "a.ARW");
    // An older release adds one while this catalog is open.
    cat.db_for_tests().execute(
        "INSERT INTO photos(folder, filename, original_path) SELECT folder, 'c.ARW', 'c.ARW'
         FROM photos LIMIT 1",
        [],
    )?;
    let added = PhotoId(cat.db_for_tests().last_insert_rowid());
    assert_eq!(cat.photo_uuid(added)?, None);
    let read = cat.photo_uuids(&[a, added, PhotoId(9999)])?;
    assert_eq!(read, [Some(uuid(&cat, a)), None, None]);
    let uuids: Vec<String> = stored(&path)?.into_iter().map(|(_, u)| u).collect();
    assert_eq!(
        uuids.iter().filter(|u| u.is_empty()).count(),
        1,
        "a read writes nothing"
    );
    assert!(cat.photos()?.iter().any(|p| p.id == added));
    Ok(())
}

#[test]
fn photos_of_every_earlier_format_get_a_uuid_as_they_open() -> Result<()> {
    for version in [1, 2, 6] {
        let mut earlier = Earlier::new(version, 3)?;
        earlier.close();
        let cat = Catalog::open(&earlier.path)?;
        let uuids =
            cat.photo_uuids(&earlier.photos.iter().map(|(id, _)| *id).collect::<Vec<_>>())?;
        assert!(uuids.iter().all(Option::is_some), "format {version}");
        let distinct: std::collections::HashSet<_> = uuids.iter().collect();
        assert_eq!(distinct.len(), 3, "format {version}");
        assert_eq!(cat.db.version()?, db::NEWEST, "format {version}");
        drop(cat);
        // Opened again, they stay.
        let again = Catalog::open(&earlier.path)?;
        assert_eq!(
            again.photo_uuids(&earlier.photos.iter().map(|(id, _)| *id).collect::<Vec<_>>())?,
            uuids
        );
    }
    Ok(())
}

#[test]
fn an_exported_catalog_keeps_its_photos_uuids() -> Result<()> {
    let (dir, _path, mut cat) = catalog()?;
    let a = photo(&cat, "a.ARW");
    let copy = cat.create_virtual_copy(a)?;
    let (a_uuid, copy_uuid) = (uuid(&cat, a), uuid(&cat, copy));
    let b_uuid = uuid(&cat, photo(&cat, "b.ARW"));
    let destination = dir.path().join("Exported.rawmakase");
    cat.export_catalog(&ExportScope::Photos(vec![a]), &destination)?;
    let exported = Catalog::open(&destination)?;
    assert_eq!(exported.photo_by_uuid(&a_uuid)?, Some(a));
    assert_eq!(exported.photo_by_uuid(&copy_uuid)?, Some(copy));
    assert_eq!(exported.photo_by_uuid(&b_uuid)?, None);
    assert_eq!(
        exported.photo_uuids(&[a, copy])?,
        [Some(a_uuid), Some(copy_uuid)]
    );
    Ok(())
}

#[test]
fn photo_by_uuid_finds_the_photo_until_it_is_removed() -> Result<()> {
    let (_dir, _path, mut cat) = catalog()?;
    let a = photo(&cat, "a.ARW");
    let copy = cat.create_virtual_copy(a)?;
    let copy_uuid = uuid(&cat, copy);
    assert_eq!(cat.photo_by_uuid(&uuid(&cat, a))?, Some(a));
    let upper = PhotoUuid::parse(&copy_uuid.as_str().to_ascii_uppercase()).unwrap();
    assert_eq!(cat.photo_by_uuid(&upper)?, Some(copy));
    cat.remove_virtual_copy(copy)?;
    assert_eq!(cat.photo_by_uuid(&copy_uuid)?, None);
    Ok(())
}
