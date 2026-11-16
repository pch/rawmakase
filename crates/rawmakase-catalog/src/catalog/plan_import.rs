//! Importing the plan an extension's `[[importer]]` wrote into a new catalog
//! (`docs/lightroom-extension-plan.md`, "The core's side"), as
//! [`import_lightroom`](super::lightroom::import_lightroom) imports a
//! Lightroom catalog: a folder beside the destination, a working catalog in
//! it, the plan attached and checked, every row copied in one transaction,
//! then the integrity check and an atomic publish.
//!
//! The plan is a SQLite file in the published format
//! ([`rawmakase_extension_protocol::plan`]), written by the extension's
//! program in the folder ([`PlanImport::plan`]). Its rules are checked here,
//! a few queries in all, before anything is copied. The catalog is new and empty,
//! so each row's catalog id is its row id in the plan, and a reference is
//! the row id of the row whose text id it names.
//!
//! [`Gate`] decides between cancelling and publishing. Until publishing
//! begins every failure or cancel deletes the folder and publishes nothing.
//! A crash leaves the folder behind, as [`import_lightroom`]'s does; a
//! crash during a copy on a volume without hard links leaves the partial
//! destination.
//!
//! [`import_lightroom`]: super::lightroom::import_lightroom
use super::db::{LightroomWrite, Reads, SqliteSql, sql, sqlite_sql};
use super::sidecar::{Merge, apply};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::xmp::descriptive::{self, Read as Descriptive};
use anyhow::{Context, Result, bail, ensure};
use rawmakase_extension_protocol::plan::{MAX_TEXT_BYTES, PLAN_VERSION, QUICK_COLLECTION, SCHEMA};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[cfg(test)]
#[path = "plan_import_tests.rs"]
mod tests;

/// The plan's name in the import's folder.
const PLAN: &str = "plan.sqlite";
/// The working catalog's name in the import's folder.
const WORKING: &str = "catalog.rawmakase";
/// How much a copy writes between looks at whether the app is quitting.
const COPY_BLOCK: usize = 1 << 20;

/// The error of an import that a canceller stopped, or quitting stopped
/// while it copied: it published nothing.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The import was stopped")
    }
}

impl std::error::Error for Cancelled {}

const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const PUBLISHING: u8 = 2;

/// Decides between cancelling an import and publishing it: one state that
/// moves once, from running to cancelled or to publishing, whichever comes
/// first. A clone shares it, so any thread may cancel.
#[derive(Clone, Debug, Default)]
pub struct Gate(Arc<GateState>);

#[derive(Debug, Default)]
struct GateState {
    phase: AtomicU8,
    /// Quitting: a copy under way stops at its next block.
    quitting: AtomicBool,
}

impl Gate {
    /// Cancels the import unless publishing has begun. True when it will
    /// publish nothing.
    pub fn cancel(&self) -> bool {
        match self
            .0
            .phase
            .compare_exchange(RUNNING, CANCELLED, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => true,
            Err(phase) => phase == CANCELLED,
        }
    }

    /// Cancels for quitting: before publishing as [`Gate::cancel`] does;
    /// after, a copy under way stops at its next block and deletes what it
    /// wrote. A hard link, being one step, is done or not.
    pub fn quit(&self) {
        self.0.quitting.store(true, Ordering::SeqCst);
        self.cancel();
    }

    /// Whether a canceller won.
    pub fn is_cancelled(&self) -> bool {
        self.0.phase.load(Ordering::SeqCst) == CANCELLED
    }

    /// The import's side: true when it may publish.
    fn publish(&self) -> bool {
        self.0
            .phase
            .compare_exchange(RUNNING, PUBLISHING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    fn running(&self) -> Result<()> {
        if self.is_cancelled() {
            bail!(Cancelled);
        }
        Ok(())
    }
}

/// One import into a new catalog at its destination. Its folder, beside
/// the destination, holds the plan and the working catalog, and is deleted
/// when the import is dropped, however it ended.
#[derive(Debug)]
pub struct PlanImport {
    folder: tempfile::TempDir,
    source: PathBuf,
    destination: PathBuf,
    gate: Gate,
    /// The schema the plan must have: tests give one without constraints,
    /// to write plans that break them.
    #[cfg(test)]
    schema: &'static str,
}

impl PlanImport {
    /// Makes the folder for importing `source` into a new catalog at
    /// `destination`. Both are made absolute here, once: the program runs
    /// in its own folder. Refuses a destination that exists.
    pub fn prepare(source: &Path, destination: &Path) -> Result<Self> {
        let source = std::path::absolute(source)?;
        let destination = std::path::absolute(destination)?;
        ensure!(
            std::fs::symlink_metadata(&destination).is_err(),
            "{} already exists; choose a new catalog filename",
            destination.display()
        );
        // Not a dot-name, as `storage::staging_file` explains.
        let folder = tempfile::Builder::new()
            .prefix("rawmakase-import-")
            .suffix(".partial")
            .tempdir_in(crate::storage::parent_dir(&destination))
            .with_context(|| format!("Couldn't make a folder beside {}", destination.display()))?;
        Ok(Self {
            folder,
            source,
            destination,
            gate: Gate::default(),
            #[cfg(test)]
            schema: SCHEMA,
        })
    }

    /// Where the program writes the plan.
    pub fn plan(&self) -> PathBuf {
        self.folder.path().join(PLAN)
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// What cancels this import, from any thread.
    pub fn gate(&self) -> Gate {
        self.gate.clone()
    }

    /// Deletes the import's folder, trying again for up to `within`: on
    /// Windows a file the program still has open, such as a plan it is
    /// still writing after a cancel, can't be deleted until it closes it.
    /// Returns the folder when it is still there. Runs on a worker.
    pub fn discard(self, within: std::time::Duration) -> Result<(), PathBuf> {
        self.discard_with(within, |folder| std::fs::remove_dir_all(folder))
    }

    fn discard_with(
        self,
        within: std::time::Duration,
        mut remove: impl FnMut(&Path) -> std::io::Result<()>,
    ) -> Result<(), PathBuf> {
        let folder = self.folder.keep();
        let until = std::time::Instant::now() + within;
        loop {
            match remove(&folder) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
                Err(_) if std::time::Instant::now() >= until => return Err(folder),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
    }

    /// Imports the plan, once the program answered that it holds `photos`
    /// photos, and publishes the new catalog at the destination, which it
    /// returns. Reports what it is doing through `progress`. Fails with
    /// [`Cancelled`] when a canceller won the gate. Runs on a worker: it
    /// reads and writes files, possibly on a network share.
    pub fn run(self, photos: u64, progress: &mut dyn FnMut(&str)) -> Result<PathBuf> {
        let working = self.folder.path().join(WORKING);
        let result = self.import(&working, photos, progress).and_then(|()| {
            // Durable before it is linked, which shares its contents. Opened
            // for writing: Windows syncs only a file it may write.
            std::fs::OpenOptions::new()
                .write(true)
                .open(&working)
                .and_then(|f| f.sync_all())?;
            if !self.gate.publish() {
                bail!(Cancelled);
            }
            progress("Saving the new catalog…");
            publish(
                &working,
                &self.destination,
                &|| self.gate.0.quitting.load(Ordering::SeqCst),
                |from, to| std::fs::hard_link(from, to),
            )
        });
        match result {
            // An interrupted statement fails with SQLite's own error.
            Err(_) if self.gate.is_cancelled() => Err(Cancelled.into()),
            Err(e) => Err(e),
            Ok(()) => Ok(self.destination.clone()),
        }
    }

    /// The working catalog written from the plan and checked. It is closed
    /// when this returns, so the folder can be deleted.
    fn import(&self, working: &Path, photos: u64, progress: &mut dyn FnMut(&str)) -> Result<()> {
        self.gate.running()?;
        let size = std::fs::metadata(&self.source).map_or(0, |m| m.len());
        let mut catalog = Catalog::create(working)?;
        let gate = self.gate.clone();
        #[cfg(test)]
        let schema = self.schema;
        #[cfg(not(test))]
        let schema = SCHEMA;
        catalog.db.with_plan(
            &self.plan(),
            schema,
            move || gate.is_cancelled(),
            |plan| {
                progress("Checking the import plan…");
                check(plan, photos, &self.gate)?;
                progress("Importing photos…");
                copy(plan, &self.source, size, &self.gate)
            },
        )?;
        progress("Checking the new catalog…");
        ensure!(
            catalog.db.is_intact()?,
            "Imported catalog failed integrity check"
        );
        Ok(())
    }
}

// The 16000000 and "quick collection" the queries below name.
const _: () = assert!(
    MAX_TEXT_BYTES == 16_000_000 && matches!(QUICK_COLLECTION.as_bytes(), b"quick collection")
);

/// The first row, by table and row id, that has a row id below 1 or a value
/// of a storage class its column doesn't take, or develop text or XMP of
/// [`MAX_TEXT_BYTES`] or more, as their readers refuse.
const MALFORMED: SqliteSql = sqlite_sql!(
    "SELECT 'plan', rowid FROM plan.plan WHERE rowid < 1 OR typeof(version) <> 'integer'
         OR typeof(source_photos) <> 'integer'
     UNION ALL SELECT 'root', rowid FROM plan.root WHERE rowid < 1 OR typeof(id) <> 'text'
         OR typeof(path) <> 'text'
     UNION ALL SELECT 'folder', rowid FROM plan.folder WHERE rowid < 1
         OR typeof(id) <> 'text' OR typeof(root) <> 'text' OR typeof(path) <> 'text'
     UNION ALL SELECT 'photo', rowid FROM plan.photo WHERE rowid < 1
         OR typeof(id) <> 'text' OR typeof(folder) <> 'text' OR typeof(file_name) <> 'text'
         OR typeof(original_path) <> 'text' OR typeof(captured) <> 'text'
         OR typeof(rating) <> 'integer' OR typeof(flag) <> 'integer' OR typeof(label) <> 'text'
         OR typeof(format) <> 'text' OR typeof(orientation) NOT IN ('text', 'null')
         OR typeof(master) NOT IN ('text', 'null') OR typeof(copy_name) <> 'text'
         OR typeof(original_name) NOT IN ('text', 'null')
     UNION ALL SELECT 'info', rowid FROM plan.info WHERE rowid < 1
         OR typeof(photo) <> 'text' OR typeof(camera) NOT IN ('text', 'null')
         OR typeof(lens) NOT IN ('text', 'null')
         OR typeof(focal) NOT IN ('real', 'integer', 'null')
         OR typeof(aperture) NOT IN ('real', 'integer', 'null')
         OR typeof(exposure) NOT IN ('real', 'integer', 'null')
         OR typeof(iso) NOT IN ('real', 'integer', 'null')
         OR typeof(width) NOT IN ('integer', 'null')
         OR typeof(height) NOT IN ('integer', 'null')
     UNION ALL SELECT 'metadata', rowid FROM plan.metadata WHERE rowid < 1
         OR typeof(photo) <> 'text' OR typeof(xmp) <> 'text'
         OR length(CAST(xmp AS BLOB)) >= 16000000
     UNION ALL SELECT 'edit', rowid FROM plan.edit WHERE rowid < 1
         OR typeof(photo) <> 'text' OR typeof(format) <> 'text' OR typeof(text) <> 'text'
         OR length(CAST(text AS BLOB)) >= 16000000
     UNION ALL SELECT 'history', rowid FROM plan.history WHERE rowid < 1
         OR typeof(photo) <> 'text' OR typeof(position) <> 'integer' OR typeof(name) <> 'text'
         OR typeof(created) NOT IN ('real', 'integer', 'null') OR typeof(text) <> 'text'
         OR length(CAST(text AS BLOB)) >= 16000000
     UNION ALL SELECT 'snapshot', rowid FROM plan.snapshot WHERE rowid < 1
         OR typeof(photo) <> 'text' OR typeof(name) <> 'text' OR typeof(text) <> 'text'
         OR length(CAST(text AS BLOB)) >= 16000000
     UNION ALL SELECT 'collection', rowid FROM plan.collection WHERE rowid < 1
         OR typeof(id) <> 'text' OR typeof(name) <> 'text'
         OR typeof(parent) NOT IN ('text', 'null') OR typeof(kind) <> 'text'
     UNION ALL SELECT 'collection_photo', rowid FROM plan.collection_photo WHERE rowid < 1
         OR typeof(collection) <> 'text' OR typeof(photo) <> 'text'
         OR typeof(position) NOT IN ('text', 'null')
     UNION ALL SELECT 'stack', rowid FROM plan.stack WHERE rowid < 1
         OR typeof(id) <> 'text' OR typeof(collapsed) <> 'integer'
     UNION ALL SELECT 'stack_photo', rowid FROM plan.stack_photo WHERE rowid < 1
         OR typeof(stack) <> 'text' OR typeof(photo) <> 'text' OR typeof(position) <> 'integer'
     UNION ALL SELECT 'keyword', rowid FROM plan.keyword WHERE rowid < 1
         OR typeof(id) <> 'text' OR typeof(name) <> 'text'
         OR typeof(parent) NOT IN ('text', 'null') OR typeof(include_on_export) <> 'integer'
         OR typeof(export_containing) <> 'integer'
     UNION ALL SELECT 'photo_keyword', rowid FROM plan.photo_keyword WHERE rowid < 1
         OR typeof(photo) <> 'text' OR typeof(keyword) <> 'text' LIMIT 1"
);

/// The plan's rules once its values are well formed, each query finding the
/// first row that breaks one, as what it is and the id it names. Each group
/// relies on the ones before: ids unique, then references resolving.
const RULES: [SqliteSql; 4] = [
    sqlite_sql!(
        "SELECT 'two roots with one id', id FROM plan.root GROUP BY id HAVING count(*) > 1
         UNION ALL SELECT 'two folders with one id', id FROM plan.folder GROUP BY id HAVING count(*) > 1
         UNION ALL SELECT 'two photos with one id', id FROM plan.photo GROUP BY id HAVING count(*) > 1
         UNION ALL SELECT 'two collections with one id', id FROM plan.collection
             GROUP BY id HAVING count(*) > 1
         UNION ALL SELECT 'two stacks with one id', id FROM plan.stack GROUP BY id HAVING count(*) > 1
         UNION ALL SELECT 'two keywords with one id', id FROM plan.keyword
             GROUP BY id HAVING count(*) > 1
         LIMIT 1"
    ),
    sqlite_sql!(
        "SELECT 'folder in a root it doesn''t have', root FROM plan.folder
             WHERE root NOT IN (SELECT id FROM plan.root)
         UNION ALL SELECT 'photo in a folder it doesn''t have', folder FROM plan.photo
             WHERE folder NOT IN (SELECT id FROM plan.folder)
         UNION ALL SELECT 'virtual copy of a photo it doesn''t have', master FROM plan.photo
             WHERE master NOT IN (SELECT id FROM plan.photo)
         UNION ALL SELECT 'info for a photo it doesn''t have', photo FROM plan.info
             WHERE photo NOT IN (SELECT id FROM plan.photo)
         UNION ALL SELECT 'metadata for a photo it doesn''t have', photo FROM plan.metadata
             WHERE photo NOT IN (SELECT id FROM plan.photo)
         UNION ALL SELECT 'edit for a photo it doesn''t have', photo FROM plan.edit
             WHERE photo NOT IN (SELECT id FROM plan.photo)
         UNION ALL SELECT 'History step for a photo it doesn''t have', photo FROM plan.history
             WHERE photo NOT IN (SELECT id FROM plan.photo)
         UNION ALL SELECT 'snapshot for a photo it doesn''t have', photo FROM plan.snapshot
             WHERE photo NOT IN (SELECT id FROM plan.photo)
         UNION ALL SELECT 'collection in a set it doesn''t have', parent FROM plan.collection
             WHERE parent NOT IN (SELECT id FROM plan.collection)
         UNION ALL SELECT 'photo in a collection it doesn''t have', collection
             FROM plan.collection_photo WHERE collection NOT IN (SELECT id FROM plan.collection)
         UNION ALL SELECT 'collection holding a photo it doesn''t have', photo
             FROM plan.collection_photo WHERE photo NOT IN (SELECT id FROM plan.photo)
         UNION ALL SELECT 'photo in a stack it doesn''t have', stack FROM plan.stack_photo
             WHERE stack NOT IN (SELECT id FROM plan.stack)
         UNION ALL SELECT 'stack holding a photo it doesn''t have', photo FROM plan.stack_photo
             WHERE photo NOT IN (SELECT id FROM plan.photo)
         UNION ALL SELECT 'keyword under a keyword it doesn''t have', parent FROM plan.keyword
             WHERE parent NOT IN (SELECT id FROM plan.keyword)
         UNION ALL SELECT 'keyword on a photo it doesn''t have', photo FROM plan.photo_keyword
             WHERE photo NOT IN (SELECT id FROM plan.photo)
         UNION ALL SELECT 'photo with a keyword it doesn''t have', keyword
             FROM plan.photo_keyword WHERE keyword NOT IN (SELECT id FROM plan.keyword)
         LIMIT 1"
    ),
    sqlite_sql!(
        "SELECT 'two info rows for one photo', photo FROM plan.info
             GROUP BY photo HAVING count(*) > 1
         UNION ALL SELECT 'two metadata rows for one photo', photo FROM plan.metadata
             GROUP BY photo HAVING count(*) > 1
         UNION ALL SELECT 'two edits for one photo', photo FROM plan.edit
             GROUP BY photo HAVING count(*) > 1
         UNION ALL SELECT 'two History steps at one position of a photo', photo FROM plan.history
             GROUP BY photo, position HAVING count(*) > 1
         UNION ALL SELECT 'photo twice in one collection', photo FROM plan.collection_photo
             GROUP BY collection, photo HAVING count(*) > 1
         UNION ALL SELECT 'keyword twice on one photo', photo FROM plan.photo_keyword
             GROUP BY photo, keyword HAVING count(*) > 1
         UNION ALL SELECT 'stack with two photos at one position', stack FROM plan.stack_photo
             GROUP BY stack, position HAVING count(*) > 1
         UNION ALL SELECT 'photo in two stacks', photo FROM plan.stack_photo
             GROUP BY photo HAVING count(*) > 1
         LIMIT 1"
    ),
    sqlite_sql!(
        "SELECT 'photo with a rating outside 0 to 5', id FROM plan.photo
             WHERE rating NOT BETWEEN 0 AND 5
         UNION ALL SELECT 'keyword with an export option other than 0 and 1', id FROM plan.keyword
             WHERE include_on_export NOT IN (0, 1) OR export_containing NOT IN (0, 1)
         UNION ALL SELECT 'stack collapsed other than 0 and 1', id FROM plan.stack
             WHERE collapsed NOT IN (0, 1)
         UNION ALL SELECT 'photo with a flag other than -1, 0 and 1', id FROM plan.photo
             WHERE flag NOT BETWEEN -1 AND 1
         UNION ALL SELECT 'collection of an unknown kind', id FROM plan.collection
             WHERE kind NOT IN ('set', 'collection', 'smart', 'quick', 'system')
         UNION ALL SELECT 'edit in an unknown format', photo FROM plan.edit
             WHERE format <> 'lightroom-develop'
         UNION ALL SELECT 'Quick Collection not named quick collection at the top', id
             FROM plan.collection WHERE kind = 'quick'
                 AND (name <> 'quick collection' OR parent IS NOT NULL)
         UNION ALL SELECT 'collection set holding photos', id FROM plan.collection
             WHERE kind = 'set' AND id IN (SELECT collection FROM plan.collection_photo)
         UNION ALL SELECT 'stack of photos in different folders', s.stack
             FROM plan.stack_photo s JOIN plan.photo p ON p.id = s.photo
             GROUP BY s.stack HAVING count(DISTINCT p.folder) > 1
         UNION ALL SELECT 'virtual copy of a virtual copy', c.id
             FROM plan.photo c JOIN plan.photo m ON m.id = c.master WHERE m.master IS NOT NULL
         UNION ALL SELECT 'virtual copy in another folder than its master', c.id
             FROM plan.photo c JOIN plan.photo m ON m.id = c.master WHERE m.folder <> c.folder
         LIMIT 1"
    ),
];

/// Fails, saying why, unless the plan keeps every rule of its format and
/// holds the `answered` photos the program said it did, which its source
/// holds too.
fn check(plan: &mut LightroomWrite<'_>, answered: u64, gate: &Gate) -> Result<()> {
    row! {
        struct Malformed {
            table: String,
            row: i64,
        }
    }
    if let Some(bad) = plan.read_sqlite::<Malformed>(MALFORMED, &[])?.pop() {
        bail!(
            "The import plan's {} row {} is malformed: a value of the wrong type, develop \
             text or XMP of {MAX_TEXT_BYTES} bytes or more, or a row id below 1",
            bad.table,
            bad.row
        );
    }
    row! {
        struct Header {
            version: i64,
            source_photos: i64,
        }
    }
    // Two at most: a plan with more is refused without reading them all.
    let header = plan.read_sqlite::<Header>(
        sqlite_sql!("SELECT version, source_photos FROM plan.plan LIMIT 2"),
        &[],
    )?;
    let [
        Header {
            version,
            source_photos,
        },
    ] = header[..]
    else {
        ensure!(
            !header.is_empty(),
            "The import plan has 0 plan rows, not one"
        );
        bail!("The import plan has more than one plan row");
    };
    ensure!(
        version == PLAN_VERSION,
        "The import plan is version {version}; this version of RAWmakase reads version {PLAN_VERSION}"
    );
    for query in RULES {
        gate.running()?;
        row! {
            struct Broken {
                what: String,
                id: String,
            }
        }
        if let Some(broken) = plan.read_sqlite::<Broken>(query, &[])?.pop() {
            let id: String = broken.id.chars().take(200).collect();
            bail!("The import plan has a {}: \"{id}\"", broken.what);
        }
    }
    let photos: i64 = plan.read_sqlite(sqlite_sql!("SELECT count(*) FROM plan.photo"), &[])?[0];
    // A photo of the source the program couldn't place, as Lightroom import
    // has always said of one.
    ensure!(
        photos == source_photos,
        "Catalog has orphaned image records ({photos}/{source_photos}); import rolled back"
    );
    ensure!(
        u64::try_from(photos) == Ok(answered),
        "The importer answered {answered} photos, but its plan holds {photos}"
    );
    Ok(())
}

/// The `collections.kind` each plan kind is stored as: the `creationId`
/// Lightroom gives it, as [`CollectionKind::from_lightroom`] reads it. The
/// Quick Collection is a plain collection by its name, and any unsaved
/// creation of Lightroom's is stored as `com.adobe.ag.library.unsaved`.
///
/// [`CollectionKind::from_lightroom`]: super::CollectionKind::from_lightroom
const COPY: [SqliteSql; 14] = [
    sqlite_sql!("INSERT INTO roots(id, original_path) SELECT rowid, path FROM plan.root"),
    sqlite_sql!(
        "INSERT INTO folders(id, root, relative_path)
         SELECT f.rowid, r.rowid, f.path FROM plan.folder f JOIN plan.root r ON r.id = f.root"
    ),
    sqlite_sql!(
        "INSERT INTO photos(id, folder, filename, original_path, captured, rating, flag, label,
             format, copy_name, master_id, orientation, lightroom_develop)
         SELECT p.rowid, f.rowid, p.file_name, p.original_path, p.captured, p.rating, p.flag,
             p.label, p.format, p.copy_name, m.rowid, p.orientation, e.text
         FROM plan.photo p JOIN plan.folder f ON f.id = p.folder
         LEFT JOIN plan.photo m ON m.id = p.master LEFT JOIN plan.edit e ON e.photo = p.id"
    ),
    // The name a master's file had when it was first imported, for Import
    // Photos' suspected duplicates and the Original filename token. Its EXIF
    // capture time is read from the file in the background.
    sqlite_sql!(
        "INSERT INTO photo_import_facts(photo, original_name, exif_captured, size, exif_read)
         SELECT rowid, original_name, NULL, NULL, 0 FROM plan.photo
         WHERE master IS NULL AND original_name <> ''"
    ),
    sqlite_sql!(
        "INSERT INTO lightroom_history(photo, position, name, created, text)
         SELECT p.rowid, h.position, h.name, h.created, h.text
         FROM plan.history h JOIN plan.photo p ON p.id = h.photo"
    ),
    sqlite_sql!(
        "INSERT INTO develop_snapshots(photo, name, recipe, lightroom)
         SELECT p.rowid, s.name, NULL, s.text FROM plan.snapshot s JOIN plan.photo p ON p.id = s.photo"
    ),
    sqlite_sql!(
        "INSERT INTO collections(id, name, parent, kind)
         SELECT c.rowid, c.name, p.rowid, CASE c.kind
             WHEN 'set' THEN 'com.adobe.ag.library.group'
             WHEN 'smart' THEN 'com.adobe.ag.library.smart_collection'
             WHEN 'system' THEN 'com.adobe.ag.library.unsaved'
             ELSE 'com.adobe.ag.library.collection' END
         FROM plan.collection c LEFT JOIN plan.collection p ON p.id = c.parent"
    ),
    sqlite_sql!(
        "INSERT INTO collection_photos(collection, photo, position)
         SELECT c.rowid, p.rowid, m.position FROM plan.collection_photo m
         JOIN plan.collection c ON c.id = m.collection JOIN plan.photo p ON p.id = m.photo"
    ),
    // Lightroom's top is position 1. A stack without photos is none.
    sqlite_sql!(
        "INSERT INTO stacks(id, collapsed) SELECT rowid, collapsed <> 0 FROM plan.stack
         WHERE id IN (SELECT stack FROM plan.stack_photo)"
    ),
    sqlite_sql!(
        "INSERT INTO stack_photos(photo, stack, position)
         SELECT p.rowid, s.rowid, ROW_NUMBER() OVER (PARTITION BY s.rowid ORDER BY m.position)
         FROM plan.stack_photo m JOIN plan.stack s ON s.id = m.stack
         JOIN plan.photo p ON p.id = m.photo"
    ),
    sqlite_sql!(
        "INSERT INTO keywords(id, name, parent)
         SELECT k.rowid, k.name, p.rowid FROM plan.keyword k LEFT JOIN plan.keyword p ON p.id = k.parent"
    ),
    // A keyword with no row exports with its parents.
    sqlite_sql!(
        "INSERT INTO keyword_export(keyword, include, parents)
         SELECT rowid, include_on_export <> 0, export_containing <> 0 FROM plan.keyword
         WHERE include_on_export = 0 OR export_containing = 0"
    ),
    sqlite_sql!(
        "INSERT INTO photo_info(photo, camera, lens, focal, aperture, exposure, iso, width, height)
         SELECT p.rowid, i.camera, i.lens, i.focal, i.aperture, i.exposure, i.iso, i.width, i.height
         FROM plan.info i JOIN plan.photo p ON p.id = i.photo"
    ),
    sqlite_sql!(
        "INSERT INTO photo_keywords(photo, keyword)
         SELECT p.rowid, k.rowid FROM plan.photo_keyword m
         JOIN plan.photo p ON p.id = m.photo JOIN plan.keyword k ON k.id = m.keyword"
    ),
];

/// Copies the checked plan into the catalog, with a `sources` row for
/// `source` and its `size`.
fn copy(plan: &mut LightroomWrite<'_>, source: &Path, size: u64, gate: &Gate) -> Result<()> {
    // `original_catalog` stays empty, as Lightroom import leaves it, bound as
    // an empty blob so the statement prepares on every backend.
    plan.write().execute(
        sql!("INSERT INTO sources(path,original_size,original_catalog) VALUES(?,?,?)"),
        &[
            &source.to_string_lossy(),
            &i64::try_from(size)?,
            &Vec::<u8>::new(),
        ],
    )?;
    for statement in COPY {
        gate.running()?;
        plan.execute_sqlite(statement, &[])?;
    }
    // Descriptive metadata from XMP, a row at a time, for photos that have
    // none yet, as Lightroom import reads it; the plan has its own keywords,
    // rating, label and flag. A packet that can't be read is left out.
    row! {
        struct Packet {
            row: i64,
            photo: PhotoId,
            xmp: String,
        }
    }
    let mut after = 0;
    loop {
        gate.running()?;
        let Some(packet) = plan
            .read_sqlite::<Packet>(
                sqlite_sql!(
                    "SELECT m.rowid, p.rowid, m.xmp FROM plan.metadata m
                     JOIN plan.photo p ON p.id = m.photo
                     WHERE m.rowid > ? ORDER BY m.rowid LIMIT 1"
                ),
                &[&after],
            )?
            .pop()
        else {
            break;
        };
        after = packet.row;
        if let Ok(found) = descriptive::read(&packet.xmp) {
            let descriptive_only = Descriptive {
                keywords: None,
                rating: None,
                label: None,
                flag: None,
                ..found
            };
            apply(
                plan.write(),
                packet.photo,
                &descriptive_only,
                Merge::FillEmpty,
            )?;
        }
    }
    let written: i64 = plan
        .write()
        .read_one(sql!("SELECT count(*) FROM photos"), &[])?;
    let rows: i64 = plan.read_sqlite(sqlite_sql!("SELECT count(*) FROM plan.photo"), &[])?[0];
    ensure!(
        written == rows,
        "The import wrote {written} photos of the plan's {rows}"
    );
    Ok(())
}

/// Puts the working catalog at `destination`, never replacing a file:
/// `link` (a hard link, which is atomic); when the volume refuses one
/// (exFAT), a copy into a file created only if none has the name. The copy
/// stops when `quitting` says so, and then deletes the file it created, as
/// it does when it fails.
fn publish(
    working: &Path,
    destination: &Path,
    quitting: &dyn Fn() -> bool,
    link: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    let taken = || {
        format!(
            "{} already exists; the import was not saved over it",
            destination.display()
        )
    };
    // The file put at `destination`, so a failure deletes it only while the
    // name is still it, never a file another writer put there meanwhile.
    let made = match link(working, destination) {
        // The link is the working catalog's file.
        Ok(()) => std::fs::File::open(working)?,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => bail!(taken()),
        // Any other refusal may be a volume without hard links. One for
        // another reason fails the exclusive create just the same.
        Err(_) => {
            let mut out = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)
            {
                Err(e) if e.kind() == ErrorKind::AlreadyExists => bail!(taken()),
                created => {
                    created.with_context(|| format!("Couldn't create {}", destination.display()))?
                }
            };
            if let Err(e) = copy_blocks(working, &mut out, quitting) {
                remove_if_ours(out, destination);
                return Err(e);
            }
            out
        }
    };
    if let Err(e) = crate::storage::sync_dir(crate::storage::parent_dir(destination)) {
        // Not durably in place: the import fails, so the file it made goes.
        remove_if_ours(made, destination);
        return Err(e);
    }
    Ok(())
}

/// Deletes `path` if it is still the file `made` is, by the file ids the
/// file system gives (Windows included), never a file another writer put at
/// that name meanwhile. Where the ids can't be read, the file is left.
fn remove_if_ours(made: std::fs::File, path: &Path) {
    let ours = crate::fsops::handle_id(&made).ok();
    drop(made);
    if ours.is_some() && crate::fsops::entry(path).ok().map(|e| e.id) == ours {
        let _ = std::fs::remove_file(path);
    }
}

/// Copies `from` into `out` a block at a time, then syncs it.
fn copy_blocks(from: &Path, out: &mut std::fs::File, quitting: &dyn Fn() -> bool) -> Result<()> {
    let mut from = std::fs::File::open(from)?;
    let mut block = vec![0u8; COPY_BLOCK];
    loop {
        if quitting() {
            bail!(Cancelled);
        }
        let n = match from.read(&mut block) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        out.write_all(&block[..n])?;
    }
    out.sync_all()?;
    Ok(())
}
