//! The catalog's SQL statements, checked for their shape when they are
//! compiled.

/// One statement of the catalog's portable SQL: what SQLite and Postgres
/// both accept, with `?` or numbered `?N` placeholders. Write it with
/// [`sql!`](macro@crate::catalog::db::sql), which checks it at compile time.
///
/// It must be a single statement starting with `SELECT`, `WITH`, `INSERT`,
/// `UPDATE` or `DELETE` and contain no `;`, so domain SQL can't change the
/// connection: pragmas, attachments and transaction control are issued by
/// `catalog::db` alone.
///
/// The check is [`assert_shape`]; these don't build:
///
/// ```compile_fail
/// const _: () = rawmakase_catalog::catalog::assert_sql_shape("PRAGMA foreign_keys=OFF");
/// ```
/// ```compile_fail
/// const _: () = rawmakase_catalog::catalog::assert_sql_shape("EXPLAIN PRAGMA foreign_keys=OFF");
/// ```
/// ```compile_fail
/// const _: () = rawmakase_catalog::catalog::assert_sql_shape("SELECT 1; PRAGMA foreign_keys=OFF");
/// ```
/// ```compile_fail
/// const _: () = rawmakase_catalog::catalog::assert_sql_shape("-- a comment\nSELECT 1");
/// ```
///
/// while this does:
///
/// ```
/// const _: () = rawmakase_catalog::catalog::assert_sql_shape("  SELECT 1");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::catalog) struct Sql(&'static str);

impl Sql {
    /// `sql`, refused unless it has the allowed shape; called in a constant
    /// by [`sql!`](macro@crate::catalog::db::sql), so a refusal fails the build.
    pub(in crate::catalog) const fn new(sql: &'static str) -> Self {
        assert_shape(sql);
        Self(sql)
    }

    pub(in crate::catalog) const fn text(self) -> &'static str {
        self.0
    }
}

/// Panics unless `sql` is one statement starting with `SELECT`, `WITH`,
/// `INSERT`, `UPDATE` or `DELETE` with no `;`: the check [`Sql`] and
/// [`SqliteSql`] make when they are built.
pub const fn assert_shape(sql: &str) {
    assert!(
        has_allowed_shape(sql),
        "not one SELECT, WITH, INSERT, UPDATE or DELETE statement"
    );
}

/// A statement that only SQLite runs: one reading a Lightroom catalog
/// attached to the catalog, or SQLite's own tables. Same shape as [`Sql`];
/// only `LightroomWrite` runs it, and the portability check skips it.
/// Write it with [`sqlite_sql!`](crate::catalog::db::sqlite_sql).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::catalog) struct SqliteSql(&'static str);

impl SqliteSql {
    pub(in crate::catalog) const fn new(sql: &'static str) -> Self {
        assert_shape(sql);
        Self(sql)
    }

    pub(in crate::catalog) const fn text(self) -> &'static str {
        self.0
    }
}

/// A [`Sql`] statement, checked at compile time.
macro_rules! sql {
    ($sql:expr) => {{
        const SQL: $crate::catalog::db::Sql = $crate::catalog::db::Sql::new($sql);
        SQL
    }};
}

/// A [`SqliteSql`] statement, checked at compile time.
macro_rules! sqlite_sql {
    ($sql:expr) => {{
        const SQL: $crate::catalog::db::SqliteSql = $crate::catalog::db::SqliteSql::new($sql);
        SQL
    }};
}

pub(in crate::catalog) use {sql, sqlite_sql};

/// Whether `sql` is one statement starting with an allowed keyword, after
/// leading whitespace, with no `;` anywhere.
const fn has_allowed_shape(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b';' {
            return false;
        }
        i += 1;
    }
    let mut start = 0;
    while start < bytes.len() && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    const ALLOWED: [&str; 5] = ["SELECT", "WITH", "INSERT", "UPDATE", "DELETE"];
    let mut k = 0;
    while k < ALLOWED.len() {
        if starts_with_keyword(bytes, start, ALLOWED[k].as_bytes()) {
            return true;
        }
        k += 1;
    }
    false
}

/// Whether `bytes` has `keyword` at `start`, in any case, as a whole word.
const fn starts_with_keyword(bytes: &[u8], start: usize, keyword: &[u8]) -> bool {
    if bytes.len() - start < keyword.len() {
        return false;
    }
    let mut i = 0;
    while i < keyword.len() {
        if bytes[start + i].to_ascii_uppercase() != keyword[i] {
            return false;
        }
        i += 1;
    }
    let end = start + keyword.len();
    end == bytes.len() || !(bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
}

#[cfg(test)]
mod tests {
    use super::has_allowed_shape;

    #[test]
    fn only_single_data_statements_are_allowed() {
        for allowed in [
            "SELECT 1",
            "  \n\tselect 1",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "INSERT INTO t VALUES (?)",
            "UPDATE t SET a=?",
            "DELETE FROM t",
            "SELECT(1)",
        ] {
            assert!(has_allowed_shape(allowed), "{allowed}");
        }
        for refused in [
            "",
            "PRAGMA foreign_keys=OFF",
            "EXPLAIN PRAGMA foreign_keys=OFF",
            "EXPLAIN SELECT 1",
            "SELECT 1; PRAGMA foreign_keys=OFF",
            "SELECT 1;",
            "ATTACH DATABASE 'x' AS lr",
            "DETACH DATABASE lr",
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
            "SAVEPOINT a",
            "-- comment\nSELECT 1",
            "/* comment */ SELECT 1",
            "SELECTED",
            "VACUUM",
        ] {
            assert!(!has_allowed_shape(refused), "{refused}");
        }
    }
}
