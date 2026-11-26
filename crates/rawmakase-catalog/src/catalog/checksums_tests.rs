//! Checksums are kept per master, only for the file they were read from.
use super::*;
use crate::storage::{ChecksumRead, FileChecksum};

fn catalog() -> Result<(tempfile::TempDir, Catalog, Vec<Photo>)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("a.dng"), b"synthetic raw a")?;
    std::fs::write(folder.join("b.dng"), b"synthetic raw b")?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let photos = cat.photos()?;
    Ok((dir, cat, photos))
}
fn read(photo: &Photo) -> Result<ReadChecksum> {
    let ChecksumRead::Done(checksum) = FileChecksum::read_full(&photo.path, |_| true)? else {
        panic!("not read");
    };
    Ok(ReadChecksum {
        photo: photo.id,
        folder: photo.folder,
        filename: photo.filename.clone(),
        path: photo.path.clone(),
        checksum,
    })
}

#[test]
fn a_checksum_is_saved_for_the_photo_it_was_read_from() -> Result<()> {
    let (_dir, mut cat, photos) = catalog()?;
    let (a, b) = (read(&photos[0])?, read(&photos[1])?);
    assert_eq!(cat.checksum_count()?, (0, 2));
    assert_eq!(cat.save_checksums(std::slice::from_ref(&a))?, [a.photo]);
    assert_eq!(cat.checksums()?[&a.photo], a.checksum);
    assert_eq!(cat.checksum_count()?, (1, 2));
    // Renamed outside RAWmakase meanwhile, as an older release would: dropped.
    cat.db_for_tests().execute(
        "UPDATE photos SET filename='renamed.dng' WHERE id=?",
        [b.photo.0],
    )?;
    assert!(cat.save_checksums(std::slice::from_ref(&b))?.is_empty());
    // Removed meanwhile, its id perhaps given to another photo: dropped.
    cat.db_for_tests()
        .execute("DELETE FROM photos WHERE id=?", [b.photo.0])?;
    assert!(cat.save_checksums(&[b])?.is_empty());
    assert_eq!(cat.checksums()?.len(), 1);
    Ok(())
}

#[test]
fn a_photo_made_a_copy_meanwhile_keeps_no_checksum() -> Result<()> {
    let (_dir, mut cat, photos) = catalog()?;
    let a = read(&photos[0])?;
    let copy = cat.create_virtual_copy(a.photo)?;
    cat.set_copy_as_master(copy)?;
    assert!(cat.save_checksums(&[a])?.is_empty());
    Ok(())
}

#[test]
fn set_copy_as_master_takes_the_checksum_over() -> Result<()> {
    let (_dir, mut cat, photos) = catalog()?;
    let a = read(&photos[0])?;
    cat.save_checksums(std::slice::from_ref(&a))?;
    let copy = cat.create_virtual_copy(a.photo)?;
    cat.set_copy_as_master(copy)?;
    let checksums = cat.checksums()?;
    assert_eq!(checksums.get(&copy), Some(&a.checksum));
    assert!(!checksums.contains_key(&a.photo));
    assert_eq!(cat.checksum_count()?, (1, 2));
    Ok(())
}

#[test]
fn removing_a_photo_removes_its_checksum() -> Result<()> {
    let (_dir, mut cat, photos) = catalog()?;
    cat.save_checksums(&[read(&photos[0])?, read(&photos[1])?])?;
    cat.remove_folders(&[photos[0].folder])?;
    let left: i64 =
        cat.db_for_tests()
            .query_row("SELECT count(*) FROM photo_checksums", [], |r| r.get(0))?;
    assert_eq!(left, 0);
    Ok(())
}

#[test]
fn a_checksum_of_another_version_is_read_again() -> Result<()> {
    let (_dir, mut cat, photos) = catalog()?;
    cat.save_checksums(&[read(&photos[0])?])?;
    cat.db_for_tests()
        .execute_batch("UPDATE photo_checksums SET version = version + 1")?;
    assert!(cat.checksums()?.is_empty());
    assert_eq!(cat.checksum_count()?, (0, 2));
    Ok(())
}

#[test]
fn a_checksum_read_before_its_folder_was_located_elsewhere_is_dropped() -> Result<()> {
    let (dir, mut cat, photos) = catalog()?;
    let a = read(&photos[0])?;
    // The folder found elsewhere, as Find Missing Folder does while the
    // read is under way.
    let moved = dir.path().join("moved");
    std::fs::rename(dir.path().join("photos"), &moved)?;
    let root = cat.roots()?[0].0;
    cat.relink_root(root, &moved)?;
    assert!(cat.save_checksums(std::slice::from_ref(&a))?.is_empty());
    // Read again where it is now, it is kept.
    let a = read(&cat.photos()?.into_iter().find(|p| p.id == a.photo).unwrap())?;
    assert_eq!(cat.save_checksums(std::slice::from_ref(&a))?, [a.photo]);
    Ok(())
}

/// A catalog is never created over a file that is already there.
#[test]
fn a_catalog_is_not_created_over_an_existing_file() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("Photos.rawmakase");
    std::fs::write(&path, b"something else")?;
    let Err(e) = Catalog::create(&path) else {
        panic!("a catalog was created over a file");
    };
    assert!(e.is::<rawmakase_model::error::Exists>(), "{e:#}");
    assert!(e.to_string().starts_with("Catalog already exists: "));
    assert_eq!(std::fs::read(&path)?, b"something else");
    Ok(())
}
