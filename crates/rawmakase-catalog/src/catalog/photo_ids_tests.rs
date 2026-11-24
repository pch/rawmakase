//! A photo's id is never given to another photo, whichever way photos are
//! added and removed: requests naming a photo that is gone (an extension's
//! proposal, an MCP `photo_ids`) must not land on a new one.
use super::lightroom::import_lightroom;
use super::*;

/// A catalog with one folder of `names`, and the folder.
fn catalog(names: &[&str]) -> Result<(tempfile::TempDir, Catalog, Folder)> {
    let dir = tempfile::tempdir()?;
    let mut catalog = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    let folder = add(&dir, &mut catalog, "first", names)?;
    Ok((dir, catalog, folder))
}

/// Adds folder `name` of `names` to `catalog`; returns the catalog folder.
fn add(
    dir: &tempfile::TempDir,
    catalog: &mut Catalog,
    name: &str,
    names: &[&str],
) -> Result<Folder> {
    let folder = dir.path().join(name);
    std::fs::create_dir(&folder)?;
    for file in names {
        std::fs::write(folder.join(file), file)?;
    }
    catalog.add_folder(&folder)?;
    Ok(catalog
        .folders()?
        .into_iter()
        .find(|f| f.path == folder)
        .unwrap())
}

fn ids(catalog: &Catalog) -> Result<Vec<PhotoId>> {
    let mut ids: Vec<PhotoId> = catalog.photos()?.iter().map(|p| p.id).collect();
    ids.sort();
    Ok(ids)
}

fn highest(catalog: &Catalog) -> Result<PhotoId> {
    Ok(*ids(catalog)?.last().unwrap())
}

#[test]
fn an_imported_photo_never_takes_a_removed_photos_id() -> Result<()> {
    let (dir, mut c, _) = catalog(&["a.ARW"])?;
    let later = add(&dir, &mut c, "second", &["b.ARW"])?;
    let gone = highest(&c)?;
    c.remove_folders(&[later.id])?;
    add(&dir, &mut c, "third", &["c.ARW"])?;
    assert!(highest(&c)? > gone, "{:?} after {gone:?}", ids(&c)?);
    Ok(())
}

#[test]
fn a_virtual_copy_never_takes_a_removed_photos_id() -> Result<()> {
    let (dir, mut c, _) = catalog(&["a.ARW"])?;
    let master = highest(&c)?;
    // After a removed virtual copy.
    let copy = c.create_virtual_copy(master)?;
    c.remove_virtual_copy(copy)?;
    let again = c.create_virtual_copy(master)?;
    assert!(again > copy, "{again:?} after {copy:?}");
    // After a removed imported photo.
    let later = add(&dir, &mut c, "second", &["b.ARW"])?;
    let gone = highest(&c)?;
    c.remove_folders(&[later.id])?;
    let copy = c.create_virtual_copy(master)?;
    assert!(copy > gone, "{copy:?} after {gone:?}");
    Ok(())
}

#[test]
fn a_photo_added_after_a_lightroom_import_takes_an_id_above_lightrooms() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("lightroom.lrcat");
    super::tests::fixture(&source)?;
    let destination = dir.path().join("Photos.rawmakase");
    import_lightroom(&source, &destination)?;
    let mut c = Catalog::open(&destination)?;
    // Lightroom's ids are kept: 40, and its virtual copy 41.
    assert_eq!(ids(&c)?, [PhotoId(40), PhotoId(41)]);
    c.remove_virtual_copy(PhotoId(41))?;
    let copy = c.create_virtual_copy(PhotoId(40))?;
    assert!(copy > PhotoId(41), "{copy:?}");

    // Removed by a release that keeps no mark, the import's own still holds.
    let destination = dir.path().join("Older.rawmakase");
    import_lightroom(&source, &destination)?;
    let mut c = Catalog::open(&destination)?;
    c.db_for_tests().execute_batch(
        "DELETE FROM collection_photos WHERE photo=41;
         DELETE FROM lightroom_history WHERE photo=41;
         DELETE FROM photos WHERE id=41;",
    )?;
    add(&dir, &mut c, "added", &["a.ARW"])?;
    assert!(highest(&c)? > PhotoId(41), "{:?}", ids(&c)?);
    Ok(())
}

#[test]
fn a_catalog_without_the_mark_starts_above_its_highest_photo() -> Result<()> {
    let (dir, mut c, _) = catalog(&["a.ARW", "b.ARW"])?;
    // As a catalog written before the mark existed.
    c.db_for_tests()
        .execute_batch("DELETE FROM meta WHERE key='photo_id_high'")?;
    let top = highest(&c)?;
    add(&dir, &mut c, "second", &["c.ARW"])?;
    assert!(highest(&c)? > top);
    // The first removal there keeps the highest id from being given again.
    c.db_for_tests()
        .execute_batch("DELETE FROM meta WHERE key='photo_id_high'")?;
    let copy = c.create_virtual_copy(top)?;
    c.db_for_tests()
        .execute_batch("DELETE FROM meta WHERE key='photo_id_high'")?;
    c.remove_virtual_copy(copy)?;
    let again = c.create_virtual_copy(top)?;
    assert!(again > copy, "{again:?} after {copy:?}");
    Ok(())
}

#[test]
fn an_older_release_reusing_an_id_does_not_lower_the_mark() -> Result<()> {
    let (dir, mut c, first) = catalog(&["a.ARW"])?;
    let master = highest(&c)?;
    let copy = c.create_virtual_copy(master)?;
    c.remove_virtual_copy(copy)?;
    // An older release gives the id out again, then removes that photo.
    c.db_for_tests().execute(
        "INSERT INTO photos(id, folder, filename, original_path) VALUES (?1, ?2, 'old.ARW', 'old')",
        rusqlite::params![copy.0, first.id.0],
    )?;
    c.db_for_tests()
        .execute("DELETE FROM photos WHERE id=?", [copy.0])?;
    add(&dir, &mut c, "second", &["b.ARW"])?;
    assert!(highest(&c)? > copy, "{:?} after {copy:?}", ids(&c)?);
    Ok(())
}

#[test]
fn an_exported_catalog_keeps_the_ids_its_source_gave() -> Result<()> {
    let (dir, mut c, first) = catalog(&["a.ARW"])?;
    add(&dir, &mut c, "second", &["b.ARW"])?;
    // The mark above every photo left, which only `meta` carries.
    let copy = c.create_virtual_copy(highest(&c)?)?;
    c.remove_virtual_copy(copy)?;
    let top = copy;
    let destination = dir.path().join("Export.rawmakase");
    c.export_catalog(&ExportScope::Folders(vec![first.id]), &destination)?;
    let mut exported = Catalog::open(&destination)?;
    assert!(highest(&exported)? < top);
    add(&dir, &mut exported, "third", &["c.ARW"])?;
    assert!(highest(&exported)? > top, "{:?}", ids(&exported)?);
    Ok(())
}
