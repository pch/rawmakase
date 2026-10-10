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
    Connection::open(&path)?.execute_batch("PRAGMA user_version=4")?;
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
    drop(catalog);
    other.execute_batch("ROLLBACK")?;
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
