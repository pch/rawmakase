//! Removing the Lightroom catalog earlier releases archived inside ours.
use super::lightroom::import_lightroom;
use super::lightroom_archive::faults::FAIL_COMPACTION;
use super::tests::{compressed, fixture};
use super::*;
use crate::model::recipe::Recipe;
use rusqlite::{Connection, OptionalExtension};

/// How the releases with backfills found the archive (`backfill_once`): a
/// catalog that gives no row here has nothing for them to read.
const OLD_BACKFILL_QUERY: &str = "SELECT original_catalog FROM sources
     WHERE original_catalog IS NOT NULL AND length(original_catalog) > 0 LIMIT 1";

/// An imported catalog as an earlier release left it: a 4 MB archive of its
/// Lightroom catalog, History and snapshots stored compressed as well as
/// plain, a snapshot made in RAWmakase, and its backfills recorded as done.
struct Archived {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
}
fn archived() -> Result<Archived> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.lrcat");
    fixture(&source)?;
    Connection::open(&source)?.execute(
        "INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (40, 1, 100., 'Import', ?)",
        ["s = { Exposure2012 = 0.25 }"],
    )?;
    let path = dir.path().join("Photos.rawmakase");
    import_lightroom(&source, &path)?;
    let mut cat = Catalog::open(&path)?;
    cat.add_snapshot(
        PhotoId(40),
        "Mine",
        &Recipe {
            exposure: 0.75,
            ..Default::default()
        },
    )?;
    let db = cat.db_for_tests();
    db.execute(
        "INSERT INTO lightroom_history(photo, position, name, created, text)
         VALUES (40, 2, 'Compressed', 200., ?)",
        [compressed("s = { Contrast2012 = 10 }")],
    )?;
    db.execute(
        "INSERT INTO develop_snapshots(photo, name, recipe, lightroom)
         VALUES (40, 'Compressed', NULL, ?)",
        [compressed("s = { Exposure2012 = -1 }")],
    )?;
    db.execute_batch(
        "UPDATE sources SET original_catalog = randomblob(4000000);
         INSERT INTO meta(key, value) VALUES
             ('lightroom_history_backfilled', '1'), ('lightroom_snapshots_backfilled', '1'),
             ('lightroom_info_backfilled', '1'), ('lightroom_metadata_backfilled', '1'),
             ('lightroom_keyword_export_backfilled', '1');",
    )?;
    Ok(Archived { _dir: dir, path })
}

/// Everything a photo of the catalog shows: the photos, their Lightroom
/// develop text, History and snapshots.
fn contents(cat: &Catalog) -> Result<String> {
    let mut out = String::new();
    for p in cat.photos()? {
        out += &format!(
            "{p:?}\n{:?}\n{:?}\n{:?}\n",
            cat.lightroom_develop(p.id)?,
            cat.lightroom_history(p.id)?,
            cat.snapshots(p.id)?
        );
    }
    Ok(out)
}
fn archive_len(cat: &Catalog) -> Result<i64> {
    Ok(cat.db_for_tests().query_row(
        "SELECT COALESCE(sum(length(original_catalog)), 0) FROM sources",
        [],
        |r| r.get(0),
    )?)
}
fn meta_of(cat: &Catalog, key: &str) -> Option<String> {
    cat.meta(key).unwrap()
}

#[test]
fn opening_removes_the_archive_keeps_everything_else_and_compacts() -> Result<()> {
    let a = archived()?;
    let mut cat = Catalog::open(&a.path)?;
    let before = contents(&cat)?;
    assert!(before.contains("Compressed"));
    let size = std::fs::metadata(&a.path)?.len();
    let mut steps = Vec::new();
    assert_eq!(cat.remove_unused_data(&mut |p| steps.push(p)), None);
    assert_eq!(steps, [OpeningProgress::Compacting]);
    assert_eq!(archive_len(&cat)?, 0);
    assert_eq!(contents(&cat)?, before);
    assert_eq!(
        meta_of(&cat, "lightroom_archive_removed").as_deref(),
        Some("1")
    );
    assert_eq!(meta_of(&cat, "compact_pending"), None);
    drop(cat);
    assert!(std::fs::metadata(&a.path)?.len() < size - 3_000_000);
    // Opened again, there is nothing left to do.
    let mut cat = Catalog::open(&a.path)?;
    assert_eq!(contents(&cat)?, before);
    let mut steps = Vec::new();
    assert_eq!(cat.remove_unused_data(&mut |p| steps.push(p)), None);
    assert!(steps.is_empty());
    // A release with the backfills finds nothing to read.
    let old: Option<Vec<u8>> = cat
        .db_for_tests()
        .query_row(OLD_BACKFILL_QUERY, [], |r| r.get(0))
        .optional()?;
    assert_eq!(old, None);
    Ok(())
}

#[test]
fn a_failed_compaction_still_opens_and_is_tried_again() -> Result<()> {
    let a = archived()?;
    let size = std::fs::metadata(&a.path)?.len();
    let mut cat = Catalog::open(&a.path)?;
    let before = contents(&cat)?;
    FAIL_COMPACTION.with(|f| f.set(true));
    let failed = cat.remove_unused_data(&mut |_| {});
    FAIL_COMPACTION.with(|f| f.set(false));
    assert!(failed.unwrap().contains("No space left"));
    // The archive is gone; the space it took is still in the file.
    assert_eq!(archive_len(&cat)?, 0);
    assert_eq!(meta_of(&cat, "compact_pending").as_deref(), Some("1"));
    drop(cat);
    assert!(std::fs::metadata(&a.path)?.len() >= size);
    let mut cat = Catalog::open(&a.path)?;
    assert_eq!(contents(&cat)?, before);
    let mut steps = Vec::new();
    assert_eq!(cat.remove_unused_data(&mut |p| steps.push(p)), None);
    assert_eq!(steps, [OpeningProgress::Compacting]);
    assert_eq!(meta_of(&cat, "compact_pending"), None);
    drop(cat);
    assert!(std::fs::metadata(&a.path)?.len() < size - 3_000_000);
    Ok(())
}

#[test]
fn a_catalog_without_an_archive_is_left_alone() -> Result<()> {
    let d = tempfile::tempdir()?;
    let path = d.path().join("Photos.rawmakase");
    let mut cat = Catalog::create(&path)?;
    let mut steps = Vec::new();
    assert_eq!(cat.remove_unused_data(&mut |p| steps.push(p)), None);
    assert!(steps.is_empty());
    assert_eq!(meta_of(&cat, "lightroom_archive_removed"), None);
    Ok(())
}

#[test]
fn export_as_catalog_copies_what_the_catalog_holds() -> Result<()> {
    // An archive holding History the catalog never copied, with no record of
    // a backfill: the releases with backfills would have copied it into the
    // export. The export now holds what the catalog does.
    let d = tempfile::tempdir()?;
    let source = d.path().join("source.lrcat");
    fixture(&source)?;
    let path = d.path().join("Photos.rawmakase");
    import_lightroom(&source, &path)?;
    Connection::open(&source)?.execute(
        "INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (40, 1, 100., 'Archived only', ?)",
        ["s = { Exposure2012 = 0.25 }"],
    )?;
    let cat = Catalog::open(&path)?;
    cat.db_for_tests().execute(
        "UPDATE sources SET original_catalog = ?",
        [std::fs::read(&source)?],
    )?;
    let exported = d.path().join("Export.rawmakase");
    cat.export_catalog(&ExportScope::Photos(vec![PhotoId(40)]), &exported)?;
    assert!(cat.lightroom_history(PhotoId(40))?.is_empty());
    let copy = Catalog::open(&exported)?;
    assert!(copy.lightroom_history(PhotoId(40))?.is_empty());
    assert_eq!(archive_len(&copy)?, 0);
    Ok(())
}
