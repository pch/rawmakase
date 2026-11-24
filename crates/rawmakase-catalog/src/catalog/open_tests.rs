//! What opening a catalog guarantees (issue #341): the format is checked
//! before anything is written, a ready catalog is opened without writing,
//! and one that isn't ready is readied.
use super::locations::Computer;
use super::*;
use rusqlite::Connection;
use std::path::PathBuf;

fn computer(id: &str) -> Computer {
    Computer {
        id: id.into(),
        name: id.into(),
    }
}

/// A catalog with a folder of one photo, opened once on `mac`.
fn ready() -> Result<(tempfile::TempDir, PathBuf)> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.ARW"), "synthetic")?;
    let path = dir.path().join("Open.rawmakase");
    Catalog::create(&path)?;
    Catalog::open_as(&path, &computer("mac"))?.add_folder(&photos)?;
    Ok((dir, path))
}

/// Another connection holding the write lock, as a second computer saving.
fn writing(path: &Path) -> Result<Connection> {
    let other = Connection::open(path)?;
    other.execute_batch("BEGIN IMMEDIATE")?;
    Ok(other)
}

#[test]
fn an_unsupported_version_is_refused_with_the_file_unchanged() -> Result<()> {
    let (_dir, path) = ready()?;
    // One past the newest this release opens (7, which stores profiles once).
    Connection::open(&path)?.execute_batch("PRAGMA user_version=8")?;
    let before = std::fs::read(&path)?;
    let error = Catalog::open_as(&path, &computer("linux")).err().unwrap();
    assert!(
        error
            .to_string()
            .contains("Unsupported RAWmakase catalog version"),
        "{error}"
    );
    assert!(std::fs::read(&path)? == before, "the file changed");
    Ok(())
}

#[test]
fn a_ready_catalog_opens_without_a_write_lock() -> Result<()> {
    let (_dir, path) = ready()?;
    let other = writing(&path)?;
    // Any write here would wait for the lock, then fail.
    let catalog = Catalog::open_as(&path, &computer("mac"))?;
    assert_eq!(catalog.photos()?.len(), 1);
    // Every photo has its UUID already.
    assert!(catalog.photo_uuid(catalog.photos()?[0].id)?.is_some());
    drop(catalog);
    other.execute_batch("ROLLBACK")?;
    Ok(())
}

/// Adds a photo as a release before photo UUIDs would: without one.
fn older_release_adds_a_photo(path: &Path) -> Result<PhotoId> {
    let db = Connection::open(path)?;
    db.execute(
        "INSERT INTO photos(folder, filename, original_path) SELECT folder, 'b.ARW', 'b.ARW'
         FROM photos LIMIT 1",
        [],
    )?;
    Ok(PhotoId(db.last_insert_rowid()))
}

#[test]
fn filling_in_uuids_waits_for_another_computers_write() -> Result<()> {
    let (_dir, path) = ready()?;
    let added = older_release_adds_a_photo(&path)?;
    let other = writing(&path)?;
    // While Linux is writing, Mac can't give the photo its UUID, and the
    // open fails rather than going on without it.
    let started = std::time::Instant::now();
    assert!(Catalog::open_as(&path, &computer("mac")).is_err());
    assert!(started.elapsed() >= std::time::Duration::from_secs(4));
    other.execute_batch("ROLLBACK")?;
    let catalog = Catalog::open_as(&path, &computer("mac"))?;
    assert!(catalog.photo_uuid(added)?.is_some());
    Ok(())
}

#[test]
fn an_open_waiting_for_another_fill_in_finds_nothing_left_to_do() -> Result<()> {
    let (_dir, path) = ready()?;
    let added = older_release_adds_a_photo(&path)?;
    // Another computer is giving it its UUID.
    let other = writing(&path)?;
    let theirs = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    other.execute(
        "INSERT INTO photo_uuids(photo, uuid) VALUES (?, ?)",
        rusqlite::params![added.0, theirs],
    )?;
    let opening = std::thread::spawn(move || -> Result<_> {
        let started = std::time::Instant::now();
        let catalog = Catalog::open_as(&path, &computer("mac"))?;
        Ok((started.elapsed(), catalog.photo_uuid(added)?))
    });
    // Long enough for the open to see the photo without a UUID and wait.
    std::thread::sleep(std::time::Duration::from_secs(1));
    other.execute_batch("COMMIT")?;
    let (took, found) = opening.join().unwrap()?;
    assert!(
        took >= std::time::Duration::from_millis(500),
        "the open didn't wait for the other write: {took:?}"
    );
    assert_eq!(found, PhotoUuid::parse(theirs));
    Ok(())
}

#[test]
fn a_catalog_not_ready_for_this_computer_is_readied() -> Result<()> {
    let (_dir, path) = ready()?;
    // A computer that never opened it, and a folder an older release added
    // since, without a logical path.
    let db = Connection::open(&path)?;
    db.execute(
        "INSERT INTO folders(root, relative_path) SELECT root, 'later' FROM folders",
        [],
    )?;
    let adopted = |db: &Connection| -> rusqlite::Result<Option<bool>> {
        db.query_row(
            "SELECT adopted_at IS NOT NULL FROM computers WHERE id='linux'",
            [],
            |r| r.get(0),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })
    };
    assert_eq!(adopted(&db)?, None);
    drop(db);
    Catalog::open_as(&path, &computer("linux"))?;
    let db = Connection::open(&path)?;
    assert_eq!(adopted(&db)?, Some(true));
    let unmapped: i64 = db.query_row(
        "SELECT count(*) FROM folders f LEFT JOIN folder_paths p ON p.folder=f.id
         WHERE p.folder IS NULL",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(unmapped, 0);
    Ok(())
}

#[test]
fn a_catalog_missing_newer_tables_gains_them() -> Result<()> {
    let (_dir, path) = ready()?;
    Connection::open(&path)?.execute_batch("DROP TABLE keyword_export")?;
    Catalog::open_as(&path, &computer("mac"))?;
    let tables: i64 = Connection::open(&path)?.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name='keyword_export'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(tables, 1);
    Ok(())
}

#[test]
fn readying_waits_for_another_computers_write() -> Result<()> {
    let (_dir, path) = ready()?;
    let other = writing(&path)?;
    // Linux has to register; while Mac is writing it can't, and the open
    // fails rather than going on unready.
    let started = std::time::Instant::now();
    assert!(Catalog::open_as(&path, &computer("linux")).is_err());
    assert!(started.elapsed() >= std::time::Duration::from_secs(4));
    other.execute_batch("ROLLBACK")?;
    Catalog::open_as(&path, &computer("linux"))?;
    Ok(())
}
