//! A Lightroom library's own SQLite files: a consistent copy of one to read,
//! taken only while Lightroom has it closed, and Store Album Locally written
//! into its catalog as Lightroom writes it, with an album made as Lightroom
//! makes one for photos in no album.
//!
//! Lightroom keeps its catalog in WAL mode and leaves the `-wal` file, often
//! megabytes of it, when it quits, so neither a leftover WAL nor the main
//! file alone says anything: whether Lightroom has it open is asked of the
//! lock every connection holds ([`rawmakase_fs::sqlite_in_use`]), and a copy
//! is the main file and its WAL together.
use anyhow::{Context, Result, bail, ensure};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::{Path, PathBuf};

/// The schema version of the library catalogs whose writes were seen, the
/// only one written to.
const KNOWN_SCHEMA: &str = "1.2";

/// Fails with `message` while a program has the SQLite file `db` open.
pub(super) fn ensure_closed(db: &Path, message: &str) -> Result<()> {
    let open = rawmakase_fs::sqlite_in_use(db)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("Can't tell whether {} is open", db.display()))?;
    ensure!(!open, "{message}");
    Ok(())
}

/// A copy of a Lightroom SQLite file with what its WAL holds, in a folder of
/// its own that goes with it.
pub(super) struct Snapshot {
    _dir: tempfile::TempDir,
    pub(super) path: PathBuf,
    /// The bytes of the file and its WAL that were copied.
    pub(super) bytes: u64,
}

/// What [`snapshot`] checks has not changed: each file's size and time.
fn stamps(db: &Path) -> Result<Vec<Option<(u64, std::time::SystemTime)>>> {
    ["", "-wal"]
        .iter()
        .map(|suffix| match std::fs::metadata(with_suffix(db, suffix)) {
            Ok(m) => Ok(Some((m.len(), m.modified()?))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        })
        .collect()
}

fn with_suffix(db: &Path, suffix: &str) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push(suffix);
    PathBuf::from(p)
}

/// A copy of `db` and its WAL, taken while no program has it open, as one
/// file: refused when it is open (`message`), or changed or opened while it
/// was copied. `db` and its files are only read.
pub(super) fn snapshot(db: &Path, message: &str) -> Result<Snapshot> {
    ensure_closed(db, message)?;
    let before = stamps(db)?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("snapshot.db");
    let mut bytes = std::fs::copy(db, &path)?;
    if before[1].is_some() {
        bytes += std::fs::copy(with_suffix(db, "-wal"), with_suffix(&path, "-wal"))?;
    }
    ensure!(
        stamps(db)? == before,
        "{} changed while it was read; {message}",
        db.display()
    );
    ensure_closed(db, message)?;
    // The copy's WAL into the copy, so it is one file to read.
    let copy = Connection::open(&path)?;
    copy.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    copy.query_row("PRAGMA journal_mode=DELETE", [], |_| Ok(()))?;
    drop(copy);
    Ok(Snapshot {
        _dir: dir,
        path,
        bytes,
    })
}

/// What [`mark_for_download`] did: the albums marked and their photos now
/// marked, and photos Lightroom hasn't settled, left as they were; the album
/// photos in no album were put in, if they were, with how many were marked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marked {
    pub albums: usize,
    pub photos: usize,
    pub unsettled: usize,
    pub new_album: Option<(String, usize)>,
}

/// An album to make in the library for photos in no album, so Lightroom
/// downloads them as it does an album's.
pub(super) struct NewAlbum<'a> {
    pub name: &'a str,
    /// The photos' asset ids.
    pub assets: &'a [String],
}

/// Marks the albums `albums` (Lightroom ids) of the catalog `db` and their
/// photos Store Album Locally, as Lightroom marks them, and puts the photos
/// of `new_album` in an album marked the same way ([`make_album`]), after
/// writing a copy
/// of the catalog as it was to `backup`; Lightroom downloads their originals
/// when it next runs. Refused while a program has `db` open (`message`), for
/// a catalog of a schema whose writes weren't seen, and when an album isn't
/// there. Nothing is changed when it fails.
///
/// Per document, as Lightroom does it: a new current revision, a copy of the
/// one it was (the same `revId` and content: nothing new goes to Adobe), the
/// revision it was no longer current and its content dropped, its labels
/// moved to the new one, and `_localOnly.isMarkedForOfflineUse` set in the
/// document's annotation. Lightroom's own workers pick up a revision newer
/// than the change counters they keep and start the download.
pub(super) fn mark_for_download(
    db: &Path,
    albums: &[String],
    new_album: Option<NewAlbum<'_>>,
    backup: &Path,
    message: &str,
) -> Result<Marked> {
    ensure_closed(db, message)?;
    let mut conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.busy_timeout(std::time::Duration::ZERO)?;
    // Exclusive before anything is read: the catalog file is locked against
    // every other connection until this one closes, so Lightroom started
    // meanwhile waits rather than reads or writes alongside, and the WAL is
    // read without the `-shm` file, whose locks Lightroom's connections
    // share. Asking about those locks now would see this connection's own
    // (Windows), or, by closing a descriptor of that file, drop them
    // (POSIX), so it is asked only before.
    conn.query_row("PRAGMA locking_mode=EXCLUSIVE", [], |_| Ok(()))?;
    conn.execute_batch("BEGIN EXCLUSIVE; COMMIT")
        .map_err(|e| match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                anyhow::anyhow!("{message}")
            }
            _ => e.into(),
        })?;
    let version: Option<String> = conn
        .query_row("SELECT currentVersion FROM schemaVersion", [], |r| {
            r.get::<_, rusqlite::types::Value>(0)
        })
        .optional()?
        .map(|v| match v {
            rusqlite::types::Value::Text(t) => t,
            rusqlite::types::Value::Real(r) => r.to_string(),
            rusqlite::types::Value::Integer(i) => i.to_string(),
            _ => String::new(),
        });
    ensure!(
        version.as_deref() == Some(KNOWN_SCHEMA),
        "This Lightroom version keeps its library in a way RAWmakase doesn't know ({}); \
         choose the albums in Lightroom: right-click each, Store Album Locally",
        version.unwrap_or_default()
    );
    ensure!(
        !backup.exists(),
        "{} exists; a backup is never written over",
        backup.display()
    );
    conn.execute("VACUUM INTO ?1", [backup.to_string_lossy()])
        .context("Back up Lightroom's catalog")?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut marked = Marked::default();
    for album in albums {
        let id: Option<i64> = tx
            .query_row(
                "SELECT localDocId FROM docs
                 WHERE fullDocId=?1 AND type='album' AND deleted=0",
                [album],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else {
            bail!("Lightroom has no album {album} any more; nothing was changed");
        };
        let photos = album_photos(&tx, album)?;
        match touch(&tx, id)? {
            Touch::Unsettled => bail!(
                "Lightroom hasn't finished syncing an album; open Lightroom until it has, \
                 then try again"
            ),
            Touch::Marked | Touch::Already => marked.albums += 1,
        }
        for photo in photos {
            match touch(&tx, photo)? {
                Touch::Marked => marked.photos += 1,
                Touch::Already => {}
                Touch::Unsettled => marked.unsettled += 1,
            }
        }
    }
    if let Some(new) = new_album {
        let (album, photos, unsettled) = make_album(&tx, &new)?;
        marked.unsettled += unsettled;
        marked.new_album = Some((album, photos));
    }
    let check: String = tx.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure!(
        check == "ok",
        "Lightroom's catalog failed SQLite's integrity check"
    );
    tx.commit()?;
    Ok(marked)
}

/// Puts the photos of `new` in album `new.name` of the library, marked Store
/// Album Locally, and marks them as [`mark_for_download`] does. The album is
/// the one of that name already there, so downloading again adds to it
/// rather than making another of the same name; or made as Lightroom makes
/// one on this computer: a document for the album and one for each photo in
/// it, each with one current revision and no label, as Lightroom's are until
/// it has uploaded them (it labels them then). Photos the library no longer
/// has are left out. Returns the album's id, how many photos were put in it
/// and marked, and how many were put in it but not marked, as Lightroom
/// hasn't settled them.
fn make_album(
    tx: &rusqlite::Transaction<'_>,
    new: &NewAlbum<'_>,
) -> Result<(String, usize, usize)> {
    let now = lightroom_now();
    let album = match album_named(tx, new.name)? {
        Some((album, id)) => {
            touch(tx, id)?;
            album
        }
        None => {
            let album = new_id()?;
            insert_document(
                tx,
                &album,
                "album",
                Some("collection"),
                &crate::lr_library::new_album_annotation(&now),
                &crate::lr_library::new_album(new.name, &now),
            )?;
            album
        }
    };
    let (mut photos, mut unsettled) = (0, 0);
    for asset in new.assets {
        let id: Option<i64> = tx
            .query_row(
                "SELECT localDocId FROM docs WHERE fullDocId=?1 AND type='asset' AND deleted=0",
                [asset],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else {
            continue;
        };
        insert_document(
            tx,
            &new_id()?,
            "album_asset",
            None,
            crate::lr_library::NEW_ALBUM_ASSET_ANNOTATION,
            &crate::lr_library::new_album_asset(&album, asset, &now),
        )?;
        match touch(tx, id)? {
            Touch::Unsettled => unsettled += 1,
            Touch::Marked | Touch::Already => photos += 1,
        }
    }
    Ok((album, photos, unsettled))
}

/// The album named `name` the library has, with its row: its id and
/// `localDocId`.
fn album_named(tx: &rusqlite::Transaction<'_>, name: &str) -> Result<Option<(String, i64)>> {
    let mut albums = tx.prepare(
        "SELECT d.fullDocId, d.localDocId, r.content FROM docs d
         JOIN revs r ON r.sequence=d.winningRevSequence
         WHERE d.type='album' AND d.subtype='collection' AND d.deleted=0 AND r.deleted=0",
    )?;
    let mut rows = albums.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let Some(content) = bytes(row.get_ref(2)?) else {
            continue;
        };
        if let Ok(Some(crate::lr_library::Document::Album(a))) =
            crate::lr_library::document("album", "collection", &id, content)
            && a.name == name
        {
            return Ok(Some((id, row.get(1)?)));
        }
    }
    Ok(None)
}

/// A new document `id` of `kind`/`subtype` with its annotation and one
/// current revision of `content`, both stored as TEXT, as Lightroom stores
/// them.
fn insert_document(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    kind: &str,
    subtype: Option<&str>,
    annotation: &[u8],
    content: &[u8],
) -> Result<()> {
    tx.execute(
        "INSERT INTO docs(fullDocId, deleted, hasConflicts, annotation, spaceId, type, subtype)
         VALUES(?1, 0, 0, CAST(?2 AS TEXT), NULL, ?3, ?4)",
        params![id, annotation, kind, subtype],
    )?;
    let doc = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO revs(localDocId, revId, parentRevSequence, current, deleted, content)
         VALUES(?1, ?2, NULL, 1, 0, CAST(?3 AS TEXT))",
        params![doc, new_id()?, content],
    )?;
    let rev = tx.last_insert_rowid();
    tx.execute(
        "UPDATE docs SET winningRevSequence=?1, mostRecentRevSequence=?1 WHERE localDocId=?2",
        params![rev, doc],
    )?;
    Ok(())
}

/// A new document or revision id as Lightroom makes them: 128 random bits in
/// lowercase hex.
fn new_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Now in UTC as Lightroom writes times: `2026-11-08T10:22:05.786Z`.
fn lightroom_now() -> String {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let [y, mo, d, h, mi, s] = crate::time::utc(since.as_secs() as i64);
    format!(
        "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{:03}Z",
        since.subsec_millis()
    )
}

/// The documents of the photos of album `album`, as its `album_asset`
/// documents name them.
fn album_photos(tx: &rusqlite::Transaction<'_>, album: &str) -> Result<Vec<i64>> {
    let mut members = tx.prepare(
        "SELECT d.fullDocId, r.content FROM docs d
         JOIN revs r ON r.sequence=d.winningRevSequence
         WHERE d.type='album_asset' AND d.deleted=0 AND r.deleted=0",
    )?;
    let mut assets = Vec::new();
    let mut rows = members.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let Some(content) = bytes(row.get_ref(1)?) else {
            continue;
        };
        if let Ok(Some(crate::lr_library::Document::AlbumAsset(m))) =
            crate::lr_library::document("album_asset", "", &id, content)
            && m.album == album
        {
            assets.push(m.asset);
        }
    }
    let mut ids = Vec::new();
    for asset in assets {
        let id: Option<i64> = tx
            .query_row(
                "SELECT localDocId FROM docs WHERE fullDocId=?1 AND type='asset' AND deleted=0",
                [&asset],
                |r| r.get(0),
            )
            .optional()?;
        ids.extend(id);
    }
    Ok(ids)
}

enum Touch {
    Marked,
    Already,
    /// Lightroom has revisions of it it hasn't settled: conflicts, or a
    /// newest revision that isn't the winning one.
    Unsettled,
}

/// Marks document `id` Store Album Locally as Lightroom does (see
/// [`mark_for_download`]).
fn touch(tx: &rusqlite::Transaction<'_>, id: i64) -> Result<Touch> {
    let (winning, newest, conflicts, annotation) = tx.query_row(
        "SELECT winningRevSequence, mostRecentRevSequence, hasConflicts, annotation
         FROM docs WHERE localDocId=?1",
        [id],
        |r| {
            Ok((
                r.get::<_, Option<i64>>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                bytes(r.get_ref(3)?).map(<[u8]>::to_vec),
            ))
        },
    )?;
    let Some(winning) = winning.filter(|w| Some(*w) == newest && conflicts == 0) else {
        return Ok(Touch::Unsettled);
    };
    let Some(annotation) = crate::lr_library::mark_for_download(annotation.as_deref())? else {
        return Ok(Touch::Already);
    };
    let current: bool = tx.query_row(
        "SELECT current FROM revs WHERE sequence=?1",
        [winning],
        |r| r.get(0),
    )?;
    ensure!(current, "Lightroom's catalog is not as expected");
    // The copy keeps the content's storage class: Lightroom's is TEXT.
    tx.execute(
        "INSERT INTO revs(localDocId, revId, parentRevSequence, current, deleted, content)
         SELECT localDocId, revId, sequence, 1, deleted, content FROM revs WHERE sequence=?1",
        [winning],
    )?;
    let new = tx.last_insert_rowid();
    tx.execute(
        "UPDATE revs SET current=0, content=NULL WHERE sequence=?1",
        [winning],
    )?;
    tx.execute(
        "UPDATE labels SET revSequence=?1 WHERE localDocId=?2 AND revSequence=?3",
        params![new, id, winning],
    )?;
    tx.execute(
        "UPDATE docs SET winningRevSequence=?1, mostRecentRevSequence=?1,
         annotation=CAST(?2 AS TEXT) WHERE localDocId=?3",
        params![new, annotation, id],
    )?;
    Ok(Touch::Marked)
}

/// A column's bytes, as Lightroom stores them: TEXT that needn't be UTF-8,
/// or a BLOB.
fn bytes(v: ValueRef<'_>) -> Option<&[u8]> {
    match v {
        ValueRef::Text(b) | ValueRef::Blob(b) => Some(b),
        _ => None,
    }
}
