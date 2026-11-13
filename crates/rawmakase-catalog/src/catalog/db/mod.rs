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
/// Failures tests inject into the upgrade, on this thread.
#[cfg(test)]
pub(in crate::catalog) mod faults {
    thread_local! {
        /// The upgrade's backup fails, as on a full disk.
        pub(in crate::catalog) static FAIL_BACKUP: std::cell::Cell<bool> =
            const { std::cell::Cell::new(false) };
    }
}
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
/// The first catalog format version (`PRAGMA user_version`) holding only edits
/// rendered by the one engine, which releases before it would render with
/// operators they no longer choose; older releases check the version too. Version 2
/// may hold raster mask data, which a release that cannot read it must not open and
/// rewrite. Catalogs below it lose the settings that chose an engine or operator
/// when they are upgraded.
pub(in crate::catalog) const ONE_ENGINE: i64 = 3;
/// The first version, the oldest this release opens; it is upgraded as it opens.
pub(in crate::catalog) const FIRST_VERSION: i64 = 1;
/// The first version that could hold Lensfun lens choices (an edit, a History state
/// or a snapshot naming a Lensfun lens), which releases before Lensfun would read as
/// an Adobe profile without a name and save without it. This release no longer
/// writes it; tests use it to check that each earlier release refused the next
/// format.
#[cfg(test)]
pub(in crate::catalog) const LENSFUN: i64 = 4;
/// The first version that could hold AI Denoise settings, which releases before it
/// would render without AI Denoise. Kept for tests, as [`LENSFUN`] is.
#[cfg(test)]
pub(in crate::catalog) const AI_DENOISE: i64 = 5;
/// The first version that could hold HDR edits, which releases before it would
/// render as SDR. Kept for tests, as [`LENSFUN`] is.
#[cfg(test)]
pub(in crate::catalog) const HDR: i64 = 6;
/// The first version that stores each camera profile and Lensfun calibration once,
/// in `recipe_assets`, and has stored recipes, History states and snapshots name
/// them by content id: releases before it would read such a recipe as damaged.
/// Every catalog below it is upgraded to it when it opens
/// ([`Catalog::open_as`](super::Catalog::open_as)), and new ones are created at it.
pub(in crate::catalog) const PROFILE_ASSETS: i64 = 7;
/// The newest version this release opens, and the one it creates and upgrades
/// catalogs to.
pub(in crate::catalog) const NEWEST: i64 = PROFILE_ASSETS;

/// Whether a release that opens versions up to `newest` opens a catalog of
/// `version`: this one with [`NEWEST`], an earlier one with its own (the check is
/// the same in every release).
pub(in crate::catalog) fn opens(version: i64, newest: i64) -> bool {
    (FIRST_VERSION..=newest).contains(&version)
}

/// The catalog's database. Opaque: nothing outside this module sees which
/// backend it is or its connection.
pub(in crate::catalog) struct Db {
    backend: Backend,
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
            "PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version={NEWEST};"
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
        })
    }

    /// The catalog's format version.
    pub(in crate::catalog) fn version(&self) -> Result<i64> {
        Ok(self
            .connection()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?)
    }

    /// Upgrades a catalog of an earlier version to `target`, after writing a
    /// consistent copy of it to `backup` (a path nothing exists at). The copy is
    /// SQLite's own (`VACUUM INTO`), not a copy of the live file, and is checked
    /// before anything is written. Then one immediate transaction gives the catalog
    /// the tables added since, runs `migrate` with the version found and raises the
    /// version: when any of it fails, nothing is changed and the backup stays. A
    /// catalog already at `target` or later is left as it is.
    pub(in crate::catalog) fn upgrade(
        &mut self,
        target: i64,
        backup: &Path,
        migrate: impl FnOnce(&mut Write<'_>, i64) -> Result<()>,
    ) -> Result<()> {
        debug_assert!((ONE_ENGINE..=NEWEST).contains(&target));
        if self.version()? >= target {
            return Ok(());
        }
        let Backend::Sqlite(db) = &mut self.backend;
        ensure!(
            !backup.exists(),
            "A backup already exists: {}",
            backup.display()
        );
        if let Err(error) = copy_into(db, backup) {
            let _ = std::fs::remove_file(backup);
            let size = std::fs::metadata(path_of(db)).map_or(0, |m| m.len());
            return Err(error.context(format!(
                "Could not back up the catalog before upgrading it; it was left unchanged. \
                 The backup needs about {} free beside it",
                megabytes(size)
            )));
        }
        // The upgrade is one immediate transaction that rechecks what it read.
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let found = tx.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?;
        if found < target {
            // The tables added since, before any row is touched; created in the
            // transaction, so a failure leaves the file as it was.
            tx.execute_batch(include_str!("../schema.sql"))?;
            migrate(&mut Write { db: &tx }, found)?;
            tx.execute_batch(&format!("PRAGMA user_version={target}"))?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Gives the disk back the space deleted and rewritten rows leave in the file.
    pub(in crate::catalog) fn vacuum(&mut self) -> Result<()> {
        self.connection().execute_batch("VACUUM")?;
        Ok(())
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
    /// of them when it fails or panics.
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
                let result = f(&mut Write { db: &tx })?;
                tx.commit()?;
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
                let result = f(&mut LightroomWrite {
                    write: Write { db: &tx },
                })?;
                tx.commit()?;
                drop(attached);
                Ok(result)
            }
        }
    }
}

/// The file `db` is, as SQLite names it.
fn path_of(db: &Connection) -> std::path::PathBuf {
    db.path().map(std::path::PathBuf::from).unwrap_or_default()
}

/// `bytes` as people read a file's size.
fn megabytes(bytes: u64) -> String {
    format!("{:.0} MB", (bytes as f64 / 1e6).ceil())
}

/// SQLite's consistent copy of `db` at `path`, checked to be an intact catalog.
fn copy_into(db: &Connection, path: &Path) -> Result<()> {
    #[cfg(test)]
    if faults::FAIL_BACKUP.with(std::cell::Cell::get) {
        anyhow::bail!("disk full (injected)");
    }
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
}

impl Write<'_> {
    fn connection(&self) -> &Connection {
        self.db
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
        match f(self) {
            Ok(value) => {
                self.db.execute_batch("RELEASE write")?;
                Ok(value)
            }
            Err(error) => {
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
