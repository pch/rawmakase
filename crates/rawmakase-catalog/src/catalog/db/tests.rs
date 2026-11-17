//! The boundary's guarantees: reads can't write, a snapshot sees one state,
//! failed writes leave nothing, and a Lightroom catalog is always detached.
use super::*;
use crate::catalog::value::row;

/// A new catalog file in a temporary folder, open, with one photo row.
fn catalog() -> Result<(tempfile::TempDir, std::path::PathBuf, Db)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("Boundary.rawmakase");
    Db::create(&path)?;
    let mut db = Db::open(&CatalogLocation::File(path.clone()))?;
    db.write(|w| {
        w.execute(
            sql!("INSERT INTO roots(id, original_path) VALUES (1, '/photos')"),
            &[],
        )?;
        w.execute(
            sql!("INSERT INTO folders(id, root, relative_path) VALUES (1, 1, '')"),
            &[],
        )?;
        w.execute(
            sql!(
                "INSERT INTO photos(id, folder, filename, original_path, rating)
                 VALUES (1, 1, 'a.dng', '/photos/a.dng', 1)"
            ),
            &[],
        )
    })?;
    Ok((dir, path, db))
}

fn rating(db: &impl Reads) -> Result<i64> {
    db.read_one(sql!("SELECT rating FROM photos WHERE id=1"), &[])
}

#[test]
fn reads_refuse_statements_that_write() -> Result<()> {
    let (_dir, _, db) = catalog()?;
    let update = sql!("UPDATE photos SET rating=5 WHERE id=1 RETURNING rating");
    let error = db.read::<i64>(update, &[]).unwrap_err();
    assert!(error.to_string().contains("A read can't write"), "{error}");
    assert!(db.read_optional::<i64>(update, &[]).is_err());
    let error = db.snapshot(|s| s.read::<i64>(update, &[])).unwrap_err();
    assert!(error.to_string().contains("A read can't write"), "{error}");
    assert_eq!(rating(&db)?, 1);
    Ok(())
}

#[test]
fn a_snapshot_sees_one_state_while_another_connection_writes() -> Result<()> {
    let (_dir, path, db) = catalog()?;
    // No busy timeout: a write that has to wait fails at once.
    let other = Connection::open(&path)?;
    other.busy_timeout(Duration::ZERO)?;
    let (first, attempt, second) = db.snapshot(|s| {
        let first = rating(s)?;
        let attempt = other.execute("UPDATE photos SET rating=4 WHERE id=1", []);
        Ok((first, attempt, rating(s)?))
    })?;
    let busy = attempt.unwrap_err();
    assert_eq!(
        busy.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy),
        "{busy}"
    );
    assert_eq!((first, second), (1, 1));
    // Once it ends, the other connection writes as usual.
    other.execute("UPDATE photos SET rating=4 WHERE id=1", [])?;
    assert_eq!(rating(&db)?, 4);
    Ok(())
}

#[test]
fn a_failed_write_leaves_nothing_and_a_failed_savepoint_only_its_own() -> Result<()> {
    let (_dir, _, mut db) = catalog()?;
    let failed: Result<()> = db.write(|w| {
        w.execute(sql!("UPDATE photos SET rating=2 WHERE id=1"), &[])?;
        anyhow::bail!("refused")
    });
    assert!(failed.is_err());
    assert_eq!(rating(&db)?, 1);

    db.write(|w| {
        w.execute(sql!("UPDATE photos SET rating=2 WHERE id=1"), &[])?;
        let inner: Result<()> = w.savepoint(|w| {
            w.execute(sql!("UPDATE photos SET rating=3 WHERE id=1"), &[])?;
            anyhow::bail!("refused")
        });
        assert!(inner.is_err());
        assert_eq!(rating(w)?, 2);
        w.savepoint(|w| w.execute(sql!("UPDATE photos SET label='kept' WHERE id=1"), &[]))?;
        Ok(())
    })?;
    assert_eq!(rating(&db)?, 2);
    let label: String = db.read_one(sql!("SELECT label FROM photos WHERE id=1"), &[])?;
    assert_eq!(label, "kept");
    Ok(())
}

#[test]
fn new_rows_return_their_ids() -> Result<()> {
    let (_dir, _, mut db) = catalog()?;
    let id: crate::ids::RootId = db.write(|w| {
        w.insert_returning_id(
            sql!("INSERT INTO roots(original_path) VALUES (?) RETURNING id"),
            &[&"/more"],
        )
    })?;
    row! {
        struct Root {
            id: crate::ids::RootId,
            path: String,
        }
    }
    let root: Root = db.read_one(
        sql!("SELECT id, original_path FROM roots WHERE id=?"),
        &[&id],
    )?;
    assert_eq!((root.id, root.path.as_str()), (id, "/more"));
    Ok(())
}

/// Whether a database is attached as `plan`.
fn attached(db: &Db) -> Result<bool> {
    Ok(db.connection().query_row(
        "SELECT count(*) FROM pragma_database_list WHERE name='plan'",
        [],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

#[test]
fn an_import_plan_is_detached_whatever_its_import_does() -> Result<()> {
    let (dir, _, mut db) = catalog()?;
    let plan = dir.path().join("plan.sqlite");
    let schema = "CREATE TABLE photo (id TEXT)";
    Connection::open(&plan)?.execute_batch(schema)?;
    let with_plan = |db: &mut Db, rating: i64, then: fn() -> Result<()>| {
        db.with_plan(
            &plan,
            schema,
            || false,
            |p| {
                p.write()
                    .execute(sql!("UPDATE photos SET rating=? WHERE id=1"), &[&rating])?;
                then()
            },
        )
    };
    with_plan(&mut db, 2, || Ok(()))?;
    assert!(!attached(&db)?);
    assert_eq!(rating(&db)?, 2);

    assert!(with_plan(&mut db, 3, || anyhow::bail!("refused")).is_err());
    assert!(!attached(&db)?);
    assert_eq!(rating(&db)?, 2);

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_plan(&mut db, 4, || panic!("import failed"))
    }));
    assert!(panicked.is_err());
    assert!(!attached(&db)?);
    assert_eq!(rating(&db)?, 2);
    Ok(())
}

#[test]
fn a_unit_test_catalog_does_not_flush_every_commit() -> Result<()> {
    // Fault matrices rebuild a catalog per injected fault. The fault is a
    // panic in this process, so synchronous=OFF (0) is enough on the
    // connection that recovery reopens. FULL is what made those matrices
    // take tens of minutes on NTFS. Shipping opens are compiled without
    // `cfg(test)` and stay FULL; this test only runs as a unit test.
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("Flush.rawmakase");
    Db::create(&path)?;
    let db = Db::open(&CatalogLocation::File(path))?;
    let mode: i64 = db
        .connection()
        .query_row("PRAGMA synchronous", [], |r| r.get(0))?;
    assert_eq!(mode, 0);
    Ok(())
}
