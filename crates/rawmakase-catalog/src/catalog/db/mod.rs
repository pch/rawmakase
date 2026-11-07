//! The catalog's database, behind one narrow boundary (issue #341).
//!
//! Domain code above this module reads and writes through [`Db`] in the
//! catalog's portable SQL ([`Sql`]) and its own values ([`ToValue`],
//! [`FromRow`]); nothing outside sees which backend holds the rows or its
//! connection. SQLite is the only backend. A second one would be a new
//! [`Backend`] variant, with each `match` on it showing where it differs.
//!
//! Every write goes through [`Db::write`], the one place a backend would
//! check ownership or record what it wrote. Reads refuse any statement that
//! isn't read-only, and [`Sql`]'s shape keeps pragmas, attachments and
//! transaction control inside this module.
mod sql;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod value_tests;

use super::CatalogLocation;
use super::value::{FromRow, FromValue, ToValue, ValueRef};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::{path::Path, time::Duration};

pub use sql::assert_shape;
pub(in crate::catalog) use sql::{Sql, SqliteSql, sql, sqlite_sql};

/// Marks a file as a RAWmakase catalog (`PRAGMA application_id`).
const APPLICATION_ID: i64 = 0x4f4d4152;
/// The catalog format version (`PRAGMA user_version`) new catalogs get; older
/// releases check it too. Version 2 may hold raster mask data, which a release
/// that cannot read it must not open and rewrite. Version 3 holds edits rendered by
/// the one engine, which releases before it would render with operators they no
/// longer choose.
pub(in crate::catalog) const VERSION: i64 = 3;
/// The first version, still read and written as it is until a catalog is upgraded.
pub(in crate::catalog) const FIRST_VERSION: i64 = 1;
/// The first version that may hold raster masks.
pub(in crate::catalog) const RASTER_MASKS: i64 = 2;
/// The first version that may hold Lensfun lens choices (an edit, a History state or
/// a snapshot naming a Lensfun lens), which releases before Lensfun would read as an
/// Adobe profile without a name and save without it. A catalog is raised to it by the
/// first write that stores one ([`Catalog::write_recipes`]); others stay at
/// [`VERSION`], which those releases open.
///
/// [`Catalog::write_recipes`]: super::Catalog
pub(in crate::catalog) const LENSFUN: i64 = 4;
/// The first version that may hold AI Denoise settings (an edit, a History state or a
/// snapshot with AI Denoise on or its Amount moved), which releases before it would
/// render without AI Denoise and save without the setting. Raised to by the first
/// write that stores one, as [`LENSFUN`] is.
pub(in crate::catalog) const AI_DENOISE: i64 = 5;
/// The first version that may hold HDR edits (an edit, a History state or a snapshot
/// with HDR on), which releases before it would render as SDR and save without the
/// setting. Values kept while HDR is off don't raise it. Raised to by the first write
/// that stores one, as [`LENSFUN`] is.
pub(in crate::catalog) const HDR: i64 = 6;
/// The newest version this release opens.
pub(in crate::catalog) const NEWEST: i64 = HDR;
/// The newest version the releases before the one engine open.
#[cfg(test)]
pub(in crate::catalog) const BEFORE_ONE_ENGINE: i64 = 2;

/// Whether a release that opens versions up to `newest` opens a catalog of
/// `version`: this one with [`VERSION`], an earlier one with its own (the check is
/// the same in every release).
pub(in crate::catalog) fn opens(version: i64, newest: i64) -> bool {
    (FIRST_VERSION..=newest).contains(&version)
}

/// The catalog's database. Opaque: nothing outside this module sees which
/// backend it is or its connection.
pub(in crate::catalog) struct Db {
    backend: Backend,
    /// Where the catalog is, for the files of its own beside it.
    location: CatalogLocation,
}

/// Where the catalog's rows live.
enum Backend {
    Sqlite(Connection),
}

impl Db {
    /// Makes the empty file at `path` a catalog with every table; the caller
    /// publishes it under its final name.
    pub(in crate::catalog) fn create(path: &Path) -> Result<()> {
        let db = Connection::open(path)?;
        // Unit tests of this crate rebuild a catalog per injected fault. The
        // fault is an in-process panic, so a commit only has to reach the
        // operating system. `synchronous=FULL` (SQLite's default, and what a
        // shipping catalog uses below) flushes every statement; on NTFS that
        // flush is most of the Windows fault-matrix time. `cfg(test)` is off
        // when this crate is compiled as a dependency, so the app's tests
        // and the shipping binary stay `FULL`.
        #[cfg(test)]
        db.execute_batch("PRAGMA synchronous=OFF;")?;
        db.execute_batch(&format!(
            "PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version={VERSION};"
        ))?;
        db.execute_batch(include_str!("../schema.sql"))?;
        Ok(())
    }

    /// Connects to the catalog at `location` and checks it is one this
    /// release reads, before anything is written.
    pub(in crate::catalog) fn open(location: &CatalogLocation) -> Result<Self> {
        let CatalogLocation::File(path) = location;
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        ensure!(
            db.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0))? == APPLICATION_ID,
            "Not a RAWmakase catalog"
        );
        ensure!(
            opens(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                NEWEST
            ),
            "Unsupported RAWmakase catalog version; file left unchanged"
        );
        db.busy_timeout(Duration::from_secs(5))?;
        // See `create`: this crate's unit tests skip the durability flush.
        // Split so a shipping open cannot compile the OFF pragma.
        #[cfg(test)]
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=OFF;")?;
        #[cfg(not(test))]
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
        Ok(Self {
            backend: Backend::Sqlite(db),
            location: location.clone(),
        })
    }

    /// The catalog's format version.
    pub(in crate::catalog) fn version(&self) -> Result<i64> {
        Ok(self
            .connection()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?)
    }

    /// Upgrades a catalog of an earlier version to `target` ([`VERSION`] or
    /// [`LENSFUN`]), after writing a consistent copy of it to `backup` (a path nothing
    /// exists at). The copy is SQLite's own (`VACUUM INTO`), not a copy of the live
    /// file, and is checked before the upgrade commits. `migrate` brings the stored
    /// rows of a catalog before [`VERSION`] up to date and `then` writes what needed
    /// the upgrade, both in the upgrade's transaction: when either fails, nothing is
    /// changed and the backup stays. On a catalog already at `target` or later, only
    /// `then` runs, as a [`write`](Self::write).
    pub(in crate::catalog) fn upgrade<R>(
        &mut self,
        target: i64,
        backup: &Path,
        migrate: impl FnOnce(&mut Write<'_>) -> Result<()>,
        then: impl FnOnce(&mut Write<'_>) -> Result<R>,
    ) -> Result<R> {
        debug_assert!((VERSION..=NEWEST).contains(&target));
        if self.version()? >= target {
            return self.write(then);
        }
        let Backend::Sqlite(db) = &mut self.backend;
        let location = &self.location;
        ensure!(
            !backup.exists(),
            "A backup already exists: {}",
            backup.display()
        );
        if let Err(error) = copy_into(db, backup) {
            let _ = std::fs::remove_file(backup);
            return Err(error.context("Could not back up the catalog; it was not upgraded"));
        }
        // The upgrade is one immediate transaction that rechecks what it read.
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let found = tx.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?;
        if found < target {
            if found < VERSION {
                migrate(&mut Write::new(&tx, location))?;
            }
            tx.execute_batch(&format!("PRAGMA user_version={target}"))?;
        }
        let mut write = Write::new(&tx, location);
        let result = then(&mut write)?;
        let discarded = std::mem::take(&mut write.discarded);
        tx.commit()?;
        delete_discarded(discarded);
        Ok(result)
    }

    /// Writes a consistent copy of the catalog, every row and its format as
    /// they are, to `path` (a path nothing exists at), and checks it. The
    /// copy is SQLite's own (`VACUUM INTO`), not a copy of the live file.
    pub(in crate::catalog) fn copy_to(&self, path: &Path) -> Result<()> {
        ensure!(!path.exists(), "A file already exists: {}", path.display());
        copy_into(self.connection(), path)
    }

    /// Rewrites the file without the space deleted rows leave behind, so
    /// none of their bytes stay in it, then checks it is intact and every
    /// reference between its rows holds.
    pub(in crate::catalog) fn compact(&mut self) -> Result<()> {
        let db = self.connection();
        db.execute_batch("VACUUM")?;
        ensure!(self.is_intact()?, "The catalog failed its integrity check");
        let broken: Option<String> = db
            .query_row("PRAGMA foreign_key_check", [], |r| r.get(0))
            .optional()?;
        ensure!(
            broken.is_none(),
            "A row of {} refers to one that is not there",
            broken.unwrap_or_default()
        );
        Ok(())
    }

    /// The names of the catalog's tables, SQLite's own left out.
    pub(in crate::catalog) fn table_names(&self) -> Result<Vec<String>> {
        let mut statement = self.connection().prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'",
        )?;
        let names = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(names)
    }

    /// Gives a catalog from an earlier release the tables added since. On
    /// a current catalog it changes nothing and takes no write lock.
    pub(in crate::catalog) fn apply_schema(&mut self) -> Result<()> {
        match &self.backend {
            Backend::Sqlite(db) => db.execute_batch(include_str!("../schema.sql"))?,
        }
        Ok(())
    }

    /// Whether the stored data is intact, as far as the backend can tell.
    pub(in crate::catalog) fn is_intact(&self) -> Result<bool> {
        match &self.backend {
            Backend::Sqlite(db) => {
                Ok(db.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))? == "ok")
            }
        }
    }

    /// Several reads that see one consistent state, with no writes. Not
    /// subject to anything a backend checks before writing.
    pub(in crate::catalog) fn snapshot<R>(
        &self,
        f: impl FnOnce(&Snapshot<'_>) -> Result<R>,
    ) -> Result<R> {
        match &self.backend {
            Backend::Sqlite(db) => {
                // BEGIN DEFERRED: the first read takes a shared lock, which
                // the rollback journal catalogs use keeps until it ends.
                let tx = db.unchecked_transaction()?;
                let result = f(&Snapshot { db: &tx })?;
                tx.commit()?;
                Ok(result)
            }
        }
    }

    /// The only way to write: `f`'s writes are committed together, or none
    /// of them when it fails or panics. The files `f` asked to delete
    /// ([`Write::delete_after_commit`]) are deleted once the commit succeeds,
    /// and none of them otherwise.
    pub(in crate::catalog) fn write<R>(
        &mut self,
        f: impl FnOnce(&mut Write<'_>) -> Result<R>,
    ) -> Result<R> {
        self.write_with(TransactionBehavior::Deferred, f)
    }

    /// [`write`](Self::write), locking out other writers from the start, for
    /// readying a catalog as it opens: it rechecks what it read before.
    pub(in crate::catalog) fn write_immediate<R>(
        &mut self,
        f: impl FnOnce(&mut Write<'_>) -> Result<R>,
    ) -> Result<R> {
        self.write_with(TransactionBehavior::Immediate, f)
    }

    fn write_with<R>(
        &mut self,
        behavior: TransactionBehavior,
        f: impl FnOnce(&mut Write<'_>) -> Result<R>,
    ) -> Result<R> {
        match &mut self.backend {
            Backend::Sqlite(db) => {
                // Dropped without a commit, the transaction rolls back.
                let tx = db.transaction_with_behavior(behavior)?;
                let mut write = Write::new(&tx, &self.location);
                let result = f(&mut write)?;
                let discarded = std::mem::take(&mut write.discarded);
                tx.commit()?;
                delete_discarded(discarded);
                Ok(result)
            }
        }
    }

    /// Runs `f` in one write with the Lightroom catalog at `lightroom`
    /// attached as `lr`, for importing from it. It is detached again when
    /// `f` returns, fails or panics.
    pub(in crate::catalog) fn with_lightroom<R>(
        &mut self,
        lightroom: &Path,
        f: impl FnOnce(&mut LightroomWrite<'_>) -> Result<R>,
    ) -> Result<R> {
        match &mut self.backend {
            Backend::Sqlite(db) => {
                // SQLite attaches only outside a transaction.
                db.execute("ATTACH DATABASE ? AS lr", [lightroom.to_string_lossy()])?;
                let attached = Attached { db };
                let tx = attached.db.transaction()?;
                let mut write = LightroomWrite {
                    write: Write::new(&tx, &self.location),
                };
                let result = f(&mut write)?;
                let discarded = std::mem::take(&mut write.write.discarded);
                tx.commit()?;
                drop(attached);
                delete_discarded(discarded);
                Ok(result)
            }
        }
    }
}

/// Deletes the files a committed write discarded. They are RAWmakase's own
/// derived files, never a user's (see [`Write::delete_after_commit`]). One
/// that can't be deleted is left: no row names it any more, and
/// [`Catalog::editable_preview_orphans`](super::Catalog::editable_preview_orphans)
/// finds it again.
fn delete_discarded(files: Vec<std::path::PathBuf>) {
    for file in files {
        let _ = std::fs::remove_file(file);
    }
}

/// SQLite's consistent copy of `db` at `path`, checked to be an intact catalog.
fn copy_into(db: &Connection, path: &Path) -> Result<()> {
    // SQLite takes the name as text; a path that is not valid UTF-8 cannot be
    // passed to it unchanged, and a changed one would put the copy elsewhere.
    let name = path
        .to_str()
        .context("The folder's name cannot be used for a copy of the catalog")?;
    db.execute("VACUUM INTO ?", [name])?;
    let copy = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    ensure!(
        copy.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))? == "ok"
            && copy.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0))?
                == APPLICATION_ID,
        "The copy of the catalog could not be verified"
    );
    Ok(())
}

/// Whether `error` came from the database itself, as a full disk or a lost
/// connection does, rather than from a value refused before it was written.
pub(in crate::catalog) fn is_storage_error(error: &anyhow::Error) -> bool {
    error.chain().any(|e| e.is::<rusqlite::Error>())
}

/// Direct access to the stored state, for tests that set it up or break it.
#[cfg(any(test, feature = "test-support"))]
impl super::Catalog {
    /// The SQLite connection.
    pub fn db_for_tests(&self) -> &Connection {
        self.db.connection()
    }
    /// The SQLite connection, for tests that hook into it.
    pub fn db_mut_for_tests(&mut self) -> &mut Connection {
        self.db.connection_mut()
    }
    /// Makes every write of descriptive metadata fail, as on a full disk.
    pub fn fail_metadata_writes(&self) -> Result<()> {
        self.db_for_tests().execute_batch(
            "CREATE TEMP TRIGGER fail_metadata BEFORE INSERT ON photo_fields
             BEGIN SELECT RAISE(FAIL, 'disk full'); END;",
        )?;
        Ok(())
    }
}

/// Detaches the Lightroom catalog when dropped, whatever happened.
struct Attached<'a> {
    db: &'a mut Connection,
}

impl Drop for Attached<'_> {
    fn drop(&mut self) {
        // Nothing to do if it fails: the connection is dropped with the
        // catalog, and a later attach would report it.
        let _ = self.db.execute_batch("DETACH DATABASE lr");
    }
}

/// The reads available on [`Db`], [`Snapshot`] and [`Write`] alike, so a
/// helper runs inside whichever it is given. A statement that isn't
/// read-only is refused before it runs.
pub(in crate::catalog) trait Reads {
    /// Every row `sql` returns.
    fn read<T: FromRow>(&self, sql: Sql, params: &[&dyn ToValue]) -> Result<Vec<T>>;

    /// The first row `sql` returns, if any.
    fn read_optional<T: FromRow>(&self, sql: Sql, params: &[&dyn ToValue]) -> Result<Option<T>>;

    /// The first row `sql` returns; an error if there is none.
    fn read_one<T: FromRow>(&self, sql: Sql, params: &[&dyn ToValue]) -> Result<T> {
        self.read_optional(sql, params)?
            .context("Query returned no rows")
    }
}

/// Reads on a connection, refusing statements that would write.
fn query<T: FromRow>(
    db: &Connection,
    sql: &str,
    params: &[&dyn ToValue],
    limit: Option<usize>,
) -> Result<Vec<T>> {
    let mut statement = db.prepare_cached(sql)?;
    ensure!(statement.readonly(), "A read can't write: {sql}");
    let mut rows = statement.query(params_of(params))?;
    let mut read = Vec::new();
    while limit.is_none_or(|limit| read.len() < limit) {
        let Some(row) = rows.next()? else { break };
        read.push(T::from_row(&Row { row })?);
    }
    Ok(read)
}

/// `params` as rusqlite binds them.
fn params_of<'a>(params: &'a [&'a dyn ToValue]) -> rusqlite::ParamsFromIter<Vec<Param<'a>>> {
    rusqlite::params_from_iter(params.iter().map(|p| Param(*p)).collect::<Vec<_>>())
}

/// Implements [`Reads`] for a type with a `db: &Connection` field.
macro_rules! reads_on_connection {
    ($($ty:ty),*) => {$(
        impl Reads for $ty {
            fn read<T: FromRow>(&self, sql: Sql, params: &[&dyn ToValue]) -> Result<Vec<T>> {
                query(self.connection(), sql.text(), params, None)
            }

            fn read_optional<T: FromRow>(
                &self,
                sql: Sql,
                params: &[&dyn ToValue],
            ) -> Result<Option<T>> {
                Ok(query(self.connection(), sql.text(), params, Some(1))?.pop())
            }
        }
    )*};
}

reads_on_connection!(Db, Snapshot<'_>, Write<'_>);

impl Db {
    fn connection(&self) -> &Connection {
        match &self.backend {
            Backend::Sqlite(db) => db,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn connection_mut(&mut self) -> &mut Connection {
        match &mut self.backend {
            Backend::Sqlite(db) => db,
        }
    }
}

/// A consistent view for several reads (see [`Db::snapshot`]).
pub(in crate::catalog) struct Snapshot<'db> {
    db: &'db Connection,
}

impl Snapshot<'_> {
    fn connection(&self) -> &Connection {
        self.db
    }
}

/// An open write transaction (see [`Db::write`]).
pub(in crate::catalog) struct Write<'db> {
    db: &'db Connection,
    location: &'db CatalogLocation,
    /// Files to delete once the transaction commits.
    discarded: Vec<std::path::PathBuf>,
}

impl<'db> Write<'db> {
    fn new(db: &'db Connection, location: &'db CatalogLocation) -> Self {
        Self {
            db,
            location,
            discarded: Vec::new(),
        }
    }
}

impl Write<'_> {
    fn connection(&self) -> &Connection {
        self.db
    }

    /// Where the catalog being written is.
    pub(in crate::catalog) fn location(&self) -> &CatalogLocation {
        self.location
    }

    /// Deletes `file` once this transaction commits, and never if it rolls
    /// back, so no committed row names a file that is gone. Only for
    /// RAWmakase's own derived files, which the rows being written stop
    /// naming (an editable preview): never a user's file, which only
    /// `catalog::file_ops` moves or puts in the Trash.
    pub(in crate::catalog) fn delete_after_commit(&mut self, file: std::path::PathBuf) {
        self.discarded.push(file);
    }

    /// Runs `sql`; returns how many rows it changed.
    pub(in crate::catalog) fn execute(
        &mut self,
        sql: Sql,
        params: &[&dyn ToValue],
    ) -> Result<usize> {
        Ok(self
            .db
            .prepare_cached(sql.text())?
            .execute(params_of(params))?)
    }

    /// Runs an `INSERT … RETURNING id`; returns the new row's id.
    pub(in crate::catalog) fn insert_returning_id<T: FromValue>(
        &mut self,
        sql: Sql,
        params: &[&dyn ToValue],
    ) -> Result<T> {
        let mut statement = self.db.prepare_cached(sql.text())?;
        let mut rows = statement.query(params_of(params))?;
        let row = rows.next()?.context("The insert returned no id")?;
        Row { row }.get(0)
    }

    /// Runs `f` so that its writes are kept only if it succeeds; the rest
    /// of this transaction goes on either way.
    pub(in crate::catalog) fn savepoint<R>(
        &mut self,
        f: impl FnOnce(&mut Write<'_>) -> Result<R>,
    ) -> Result<R> {
        // One name serves nested savepoints too: SQLite releases and rolls
        // back to the most recent savepoint of a name.
        self.db.execute_batch("SAVEPOINT write")?;
        // Files the savepoint's writes discarded go only if they are kept.
        let discarded = self.discarded.len();
        match f(self) {
            Ok(value) => {
                self.db.execute_batch("RELEASE write")?;
                Ok(value)
            }
            Err(error) => {
                self.discarded.truncate(discarded);
                self.db.execute_batch("ROLLBACK TO write; RELEASE write")?;
                Err(error)
            }
        }
    }
}

/// A write with a Lightroom catalog attached as `lr` (see
/// [`Db::with_lightroom`]): the only place [`SqliteSql`] runs.
pub(in crate::catalog) struct LightroomWrite<'db> {
    write: Write<'db>,
}

impl<'db> LightroomWrite<'db> {
    /// The ordinary write underneath, for statements on the catalog alone.
    pub(in crate::catalog) fn write(&mut self) -> &mut Write<'db> {
        &mut self.write
    }

    /// Runs `sql`; returns how many rows it changed.
    pub(in crate::catalog) fn execute_sqlite(
        &mut self,
        sql: SqliteSql,
        params: &[&dyn ToValue],
    ) -> Result<usize> {
        Ok(self
            .write
            .db
            .prepare_cached(sql.text())?
            .execute(params_of(params))?)
    }

    /// Every row `sql` returns.
    pub(in crate::catalog) fn read_sqlite<T: FromRow>(
        &self,
        sql: SqliteSql,
        params: &[&dyn ToValue],
    ) -> Result<Vec<T>> {
        query(self.write.db, sql.text(), params, None)
    }

    /// Whether table `table` of the attached Lightroom catalog has column
    /// `column`.
    pub(in crate::catalog) fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        Ok(!self
            .read_sqlite::<i64>(
                sqlite_sql!("SELECT 1 FROM pragma_table_info(?, 'lr') WHERE name=?"),
                &[&table, &column],
            )?
            .is_empty())
    }
    /// Whether the attached Lightroom catalog has table `name`.
    pub(in crate::catalog) fn has_table(&self, name: &str) -> Result<bool> {
        Ok(!self
            .read_sqlite::<i64>(
                sqlite_sql!("SELECT 1 FROM lr.sqlite_master WHERE type='table' AND name=?"),
                &[&name],
            )?
            .is_empty())
    }
}

/// One row of a query's result, read by column index.
pub(in crate::catalog) struct Row<'a> {
    row: &'a rusqlite::Row<'a>,
}

impl Row<'_> {
    /// How many columns the row has.
    pub(in crate::catalog) fn len(&self) -> usize {
        self.row.as_ref().column_count()
    }
    /// Column `index`, read as `T`.
    pub(in crate::catalog) fn get<T: FromValue>(&self, index: usize) -> Result<T> {
        let value = match self.row.get_ref(index)? {
            rusqlite::types::ValueRef::Null => ValueRef::Null,
            rusqlite::types::ValueRef::Integer(i) => ValueRef::Integer(i),
            rusqlite::types::ValueRef::Real(f) => ValueRef::Real(f),
            rusqlite::types::ValueRef::Text(bytes) => ValueRef::Text(bytes),
            rusqlite::types::ValueRef::Blob(bytes) => ValueRef::Blob(bytes),
        };
        T::from_value(value).with_context(|| format!("column {index}"))
    }
}

/// A value bound to a SQLite statement.
struct Param<'a>(&'a dyn ToValue);

impl rusqlite::ToSql for Param<'_> {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, ValueRef as Sqlite};
        Ok(ToSqlOutput::Borrowed(match self.0.to_value() {
            ValueRef::Null => Sqlite::Null,
            ValueRef::Integer(i) => Sqlite::Integer(i),
            ValueRef::Real(f) => Sqlite::Real(f),
            ValueRef::Text(bytes) => Sqlite::Text(bytes),
            ValueRef::Blob(bytes) => Sqlite::Blob(bytes),
        }))
    }
}
