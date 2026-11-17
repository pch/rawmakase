//! The `catalog` importer: a Lightroom Classic catalog (`.lrcat`) read once
//! into the import plan the app imports ([`rawmakase_extension::protocol::plan`]).
//!
//! The catalog is copied beside the plan and read from the copy, so nothing
//! opens Lightroom's file. A catalog with a live journal, or one that changes
//! while it is copied, is refused, and the copy must pass SQLite's
//! `quick_check`. The plan's rows are `INSERT … SELECT` statements from the
//! copy, attached to the plan's connection. Each root, folder, photo,
//! collection, stack and keyword keeps Lightroom's `id_local` as its id and
//! its row id, so the new catalog's ids are Lightroom's.
//!
//! What Lightroom may hold and the plan refuses is left out: a photo's
//! second develop settings row, memberships, keyword links and stack photos
//! naming a photo, collection, stack or keyword that isn't imported, and
//! duplicate links. History steps and snapshots are decoded to plain text.
use rawmakase_extension::protocol::plan::{PLAN_VERSION, QUICK_COLLECTION, SCHEMA};
use rawmakase_extension::{Context, ErrorCode, Importer, RemoteError};
use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::ValueRef;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Why a plan couldn't be written, as the user reads it.
pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// The importer the manifest declares as `catalog`.
pub struct Catalog;

impl Importer for Catalog {
    /// Writes the plan on a thread of its own, reporting what it is doing
    /// every two seconds and stopping it when the app cancels.
    fn import(&mut self, cx: &Context, source: &Path, plan: &Path) -> Result<u64, RemoteError> {
        let stop = Arc::new(AtomicBool::new(false));
        let phase = Mutex::new("Copying the catalog…");
        let written = std::thread::scope(|s| {
            let work = s.spawn(|| write_plan(source, plan, &stop, &|p| *phase.lock().unwrap() = p));
            let mut said: Option<Instant> = None;
            while !work.is_finished() {
                if cx.is_cancelled() {
                    stop.store(true, Ordering::SeqCst);
                }
                if said.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
                    cx.progress(None, Some(*phase.lock().unwrap()));
                    said = Some(Instant::now());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            work.join()
        });
        match written {
            _ if cx.is_cancelled() => Err(RemoteError::new(ErrorCode::Cancelled, "cancelled")),
            Ok(Ok(photos)) => Ok(photos),
            Ok(Err(e)) => Err(RemoteError::new(ErrorCode::Unavailable, e.to_string())),
            Err(_) => Err(RemoteError::new(
                ErrorCode::Unavailable,
                "the catalog's reader stopped unexpectedly",
            )),
        }
    }
}

/// Writes the plan of the Lightroom catalog `source` at `plan`, a file that
/// doesn't exist yet, and closes it; returns its photos. `phase` hears what
/// it is doing; `stop` interrupts it. The private copy of `source` is deleted
/// however it ends.
pub fn write_plan(
    source: &Path,
    plan: &Path,
    stop: &Arc<AtomicBool>,
    phase: &(dyn Fn(&'static str) + Sync),
) -> Result<u64, Error> {
    let copy = plan.with_file_name("source.lrcat");
    let result = Connection::open(plan)
        .map_err(Into::into)
        .and_then(|mut db| {
            let written = write(&mut db, source, &copy, stop, phase);
            db.close().map_err(|(_, e)| e)?;
            written
        });
    let _ = std::fs::remove_file(&copy);
    result
}

fn write(
    db: &mut Connection,
    source: &Path,
    copy: &Path,
    stop: &Arc<AtomicBool>,
    phase: &(dyn Fn(&'static str) + Sync),
) -> Result<u64, Error> {
    ensure_no_live_journal(
        source,
        "Lightroom catalog has a live journal. Close Lightroom and copy/export the catalog with its companion files first",
    )?;
    take_snapshot(source, copy, stop)?;
    let stopped = stop.clone();
    db.progress_handler(1000, Some(move || stopped.load(Ordering::SeqCst)))?;
    functions(db)?;
    db.execute("ATTACH DATABASE ? AS lr", [copy.to_string_lossy()])?;
    phase("Checking the catalog…");
    let check = db.query_row("PRAGMA lr.quick_check", [], |r| r.get::<_, String>(0));
    ensure(
        check.is_ok_and(|c| c == "ok"),
        "Source catalog failed SQLite integrity check",
    )?;
    let has = |table: &str, column: &str| -> rusqlite::Result<bool> {
        db.query_row(
            "SELECT count(*) > 0 FROM lr.sqlite_master m JOIN pragma_table_info(m.name, 'lr') c
             WHERE m.type = 'table' AND m.name = ?1 AND (c.name = ?2 OR ?2 = '')",
            [table, column],
            |r| r.get(0),
        )
    };
    for table in [
        "Adobe_images",
        "AgLibraryFile",
        "AgLibraryFolder",
        "AgLibraryRootFolder",
    ] {
        ensure(
            has(table, "")?,
            &format!("Not a supported Lightroom catalog: missing {table}"),
        )?;
    }
    phase("Reading photos…");
    db.execute_batch(SCHEMA)?;
    db.execute_batch("BEGIN")?;
    let original = if has("AgLibraryFile", "originalFilename")? {
        "f.originalFilename"
    } else {
        "NULL"
    };
    let export = if has("AgLibraryKeyword", "includeOnExport")?
        && has("AgLibraryKeyword", "includeParents")?
    {
        "COALESCE(includeOnExport, 1) <> 0, COALESCE(includeParents, 1) <> 0"
    } else {
        "1, 1"
    };
    let file_name = "CASE WHEN f.idx_filename <> '' THEN f.idx_filename
        ELSE f.baseName || '.' || f.extension END";
    let statements = [
        (&[][..], format!("INSERT INTO plan SELECT {PLAN_VERSION}, count(*) FROM lr.Adobe_images")),
        (&[], "INSERT INTO root(rowid, id, path) SELECT id_local, id_local, absolutePath
             FROM lr.AgLibraryRootFolder".into()),
        (&[], "INSERT INTO folder(rowid, id, root, path)
             SELECT id_local, id_local, rootFolder, pathFromRoot FROM lr.AgLibraryFolder".into()),
        (&[], format!(
            "INSERT INTO photo(rowid, id, folder, file_name, original_path, captured, rating, flag,
                 label, format, orientation, master, copy_name, original_name)
             SELECT i.id_local, i.id_local, f.folder, {file_name},
                 r.absolutePath || d.pathFromRoot || {file_name}, COALESCE(i.captureTime, ''),
                 COALESCE(i.rating, 0), COALESCE(i.pick, 0), COALESCE(i.colorLabels, ''),
                 COALESCE(i.fileFormat, ''), i.orientation, i.masterImage, COALESCE(i.copyName, ''),
                 {original}
             FROM lr.Adobe_images i JOIN lr.AgLibraryFile f ON f.id_local = i.rootFile
             JOIN lr.AgLibraryFolder d ON d.id_local = f.folder
             JOIN lr.AgLibraryRootFolder r ON r.id_local = d.rootFolder"
        )),
        // A master, set or parent Lightroom no longer has is none, as the
        // old import's dangling reference read: the copy is a master, the
        // collection or keyword at the top.
        (&[], "UPDATE photo SET master = NULL WHERE master NOT IN (SELECT id FROM photo)".into()),
        // A photo's first settings row, as Lightroom import always read it.
        (&["Adobe_imageDevelopSettings"], "INSERT INTO edit
             SELECT image, 'lightroom-develop', text FROM lr.Adobe_imageDevelopSettings
             WHERE text IS NOT NULL AND image IN (SELECT id FROM photo)
               AND rowid IN (SELECT min(rowid) FROM lr.Adobe_imageDevelopSettings GROUP BY image)".into()),
        // Numbered by every step with text, so a step that can't be decoded
        // leaves the others where they were.
        (&["Adobe_libraryImageDevelopHistoryStep"], "INSERT INTO history
             SELECT image, position, name, created, text FROM (
                 SELECT image, row_number() OVER (PARTITION BY image ORDER BY dateCreated, id_local)
                     AS position, COALESCE(name, '') AS name, dateCreated AS created,
                     decoded(text) AS text
                 FROM lr.Adobe_libraryImageDevelopHistoryStep
                 WHERE text IS NOT NULL AND image IN (SELECT id FROM photo))
             WHERE text IS NOT NULL".into()),
        (&["Adobe_libraryImageDevelopSnapshot"], "INSERT INTO snapshot
             SELECT image, COALESCE(name, ''), decoded(text) FROM lr.Adobe_libraryImageDevelopSnapshot
             WHERE image IN (SELECT id FROM photo) AND decoded(text) IS NOT NULL".into()),
        // As `CollectionKind::from_lightroom` reads `creationId`.
        (&["AgLibraryCollection"], format!(
            "INSERT INTO collection(rowid, id, name, parent, kind)
             SELECT id_local, id_local, name, parent, CASE
                 WHEN creationId = 'com.adobe.ag.library.group' THEN 'set'
                 WHEN creationId = 'com.adobe.ag.library.smart_collection' THEN 'smart'
                 WHEN creationId = 'com.adobe.ag.library.collection'
                     AND name = '{QUICK_COLLECTION}' AND parent IS NULL THEN 'quick'
                 WHEN substr(creationId, -8) = '.unsaved' THEN 'system'
                 ELSE 'collection' END
             FROM lr.AgLibraryCollection"
        )),
        (&[], "UPDATE collection SET parent = NULL
             WHERE parent NOT IN (SELECT id FROM collection)".into()),
        (&["AgLibraryCollection", "AgLibraryCollectionImage"], "INSERT INTO collection_photo
             SELECT collection, image, positionInCollection FROM lr.AgLibraryCollectionImage
             WHERE collection IN (SELECT id FROM collection) AND image IN (SELECT id FROM photo)
               AND rowid IN (SELECT min(rowid) FROM lr.AgLibraryCollectionImage
                   GROUP BY collection, image)".into()),
        (&["AgLibraryFolderStack", "AgLibraryFolderStackImage"], "INSERT INTO stack(rowid, id, collapsed)
             SELECT id_local, id_local, collapsed <> 0 FROM lr.AgLibraryFolderStack".into()),
        // Lightroom's top is position 1; ties go by row id. A photo in two
        // stacks stays in the first.
        (&["AgLibraryFolderStack", "AgLibraryFolderStackImage"], "INSERT INTO stack_photo
             SELECT stack, image, position FROM (
                 SELECT stack, image, row_number() OVER (PARTITION BY stack
                         ORDER BY CAST(position AS REAL), id_local) AS position,
                     row_number() OVER (PARTITION BY image
                         ORDER BY stack, CAST(position AS REAL), id_local) AS nth
                 FROM lr.AgLibraryFolderStackImage
                 WHERE image IN (SELECT id FROM photo) AND stack IN (SELECT id FROM stack))
             WHERE nth = 1".into()),
        (&["AgLibraryKeyword"], format!(
            "INSERT INTO keyword(rowid, id, name, parent, include_on_export, export_containing)
             SELECT id_local, id_local, COALESCE(name, ''), parent, {export} FROM lr.AgLibraryKeyword"
        )),
        (&[], "UPDATE keyword SET parent = NULL WHERE parent NOT IN (SELECT id FROM keyword)".into()),
        (&["AgLibraryKeyword", "AgLibraryKeywordImage"], "INSERT INTO photo_keyword
             SELECT DISTINCT image, tag FROM lr.AgLibraryKeywordImage
             WHERE image IN (SELECT id FROM photo) AND tag IN (SELECT id FROM keyword)".into()),
        // Lightroom keeps aperture and shutter speed as APEX values, and the
        // size as stored: a quarter turn shows it the other way round.
        (&["AgHarvestedExifMetadata", "AgInternedExifCameraModel", "AgInternedExifLens"], "INSERT INTO info
             SELECT i.id_local, c.value, l.value, e.focalLength, f_number(e.aperture),
                 seconds(e.shutterSpeed), e.isoSpeedRating,
                 CAST(CASE WHEN i.orientation IN ('BC', 'DA', 'AD', 'CB') THEN i.fileHeight
                      ELSE i.fileWidth END AS INTEGER),
                 CAST(CASE WHEN i.orientation IN ('BC', 'DA', 'AD', 'CB') THEN i.fileWidth
                      ELSE i.fileHeight END AS INTEGER)
             FROM lr.Adobe_images i
             LEFT JOIN lr.AgHarvestedExifMetadata e ON e.image = i.id_local
             LEFT JOIN lr.AgInternedExifCameraModel c ON c.id_local = e.cameraModelRef
             LEFT JOIN lr.AgInternedExifLens l ON l.id_local = e.lensRef
             WHERE i.id_local IN (SELECT id FROM photo)".into()),
        // Text that isn't UTF-8 is left out rather than imported mangled.
        (&["Adobe_AdditionalMetadata"], "INSERT INTO metadata SELECT image, utf8(xmp)
             FROM lr.Adobe_AdditionalMetadata
             WHERE image IN (SELECT id FROM photo) AND utf8(xmp) IS NOT NULL".into()),
    ];
    for (tables, sql) in statements {
        // Between statements; within one, the progress handler stops it.
        ensure(!stop.load(Ordering::SeqCst), "cancelled")?;
        if tables
            .iter()
            .try_fold(true, |all, t| Ok::<_, rusqlite::Error>(all && has(t, "")?))?
        {
            db.execute(&sql, [])?;
        }
    }
    db.execute_batch("COMMIT")?;
    let count = |sql| db.query_row(sql, [], |r| r.get::<_, i64>(0));
    let (photos, expected) = (
        count("SELECT count(*) FROM photo")?,
        count("SELECT source_photos FROM plan")?,
    );
    ensure(
        photos == expected,
        &format!("Catalog has orphaned image records ({photos}/{expected}); import rolled back"),
    )?;
    db.execute_batch("DETACH DATABASE lr")?;
    Ok(photos as u64)
}

fn ensure(ok: bool, message: &str) -> Result<(), Error> {
    if ok { Ok(()) } else { Err(message.into()) }
}

/// The plan's SQL functions: Lightroom text decoded, and APEX values turned
/// into an f-number and seconds.
fn functions(db: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    let bytes = |v: ValueRef<'_>| match v {
        ValueRef::Text(b) | ValueRef::Blob(b) => Some(b.to_vec()),
        _ => None,
    };
    db.create_scalar_function("decoded", 1, flags, move |f| {
        Ok(bytes(f.get_raw(0)).and_then(|b| decode_history_text(&b)))
    })?;
    db.create_scalar_function("utf8", 1, flags, move |f| {
        Ok(bytes(f.get_raw(0)).and_then(|b| String::from_utf8(b).ok()))
    })?;
    db.create_scalar_function("f_number", 1, flags, |f| {
        Ok(f.get::<Option<f64>>(0)?.map(|av| 2f64.powf(av / 2.)))
    })?;
    db.create_scalar_function("seconds", 1, flags, |f| {
        Ok(f.get::<Option<f64>>(0)?.map(|tv| 2f64.powf(-tv)))
    })
}

/// Largest History step or snapshot decoded. Develop text is a few
/// kilobytes; the cap only keeps a corrupt catalog from naming a length the
/// allocation would follow.
const MAX_TEXT_BYTES: usize = 1 << 20;

/// Lightroom stores History steps and snapshots either as text or as a
/// 4-byte big-endian length followed by a zlib stream. The app keeps the
/// same reader for catalogs imported before the text arrived decoded.
pub fn decode_history_text(bytes: &[u8]) -> Option<String> {
    if bytes.len() > 6 && bytes[4] == 0x78 {
        use std::io::Read;
        // The prefix declares the decompressed length, so it bounds the read,
        // and text that expands past it is corrupt and refused.
        let expected = u32::from_be_bytes(bytes[..4].try_into().ok()?) as usize;
        if expected > MAX_TEXT_BYTES {
            return None;
        }
        let mut text = String::new();
        flate2::read::ZlibDecoder::new(&bytes[4..])
            .take(expected as u64 + 1)
            .read_to_string(&mut text)
            .ok()?;
        return (text.len() <= expected).then_some(text);
    }
    String::from_utf8(bytes.to_vec()).ok()
}

/// Fails with `message` while Lightroom has `source` open: its WAL or
/// rollback journal is not empty.
fn ensure_no_live_journal(source: &Path, message: &str) -> Result<(), Error> {
    for suffix in ["-wal", "-journal"] {
        let p = PathBuf::from(format!("{}{suffix}", source.display()));
        ensure(!p.exists() || p.metadata()?.len() == 0, message)?;
    }
    Ok(())
}

/// Copies `source` to `copy`, a block at a time until `stop`; refused if
/// the source changed while it was copied.
fn take_snapshot(source: &Path, copy: &Path, stop: &AtomicBool) -> Result<(), Error> {
    use std::io::Read;
    let before = source.metadata()?;
    let mut from = std::fs::File::open(source)?;
    let mut to = std::fs::File::create(copy)?;
    while std::io::copy(&mut (&mut from).take(1 << 20), &mut to)? > 0 {
        ensure(!stop.load(Ordering::SeqCst), "cancelled")?;
    }
    drop(to);
    let after = source.metadata()?;
    ensure(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "Source catalog changed during import; retry after closing Lightroom",
    )?;
    ensure_no_live_journal(
        source,
        "Source catalog became active during import; retry after closing Lightroom",
    )
}
