//! The catalog reaches its database through `catalog::db` alone (issue #341):
//! no other module names `rusqlite`, except those that are SQLite by what
//! they are, and tests.
use std::path::{Path, PathBuf};

/// Modules that may use SQLite directly, and why.
const SQLITE_ONLY: [(&str, &str); 4] = [
    ("catalog/db", "the boundary itself"),
    (
        "catalog/preview_cache.rs",
        "a per-machine cache file shared by all catalogs",
    ),
    (
        "catalog/lightroom/mod.rs",
        "Lightroom catalogs are SQLite files, checked before import",
    ),
    (
        "catalog/lightroom/library_db.rs",
        "a Lightroom library's own SQLite catalog: copied to read, and Store Album Locally written into it",
    ),
];

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// Whether `relative` holds only tests.
fn is_test_file(relative: &str) -> bool {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    name == "tests.rs" || name.ends_with("_tests.rs")
}

#[test]
fn only_catalog_db_reaches_the_database() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    let mut offenders = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if is_test_file(&relative) || SQLITE_ONLY.iter().any(|(m, _)| relative.starts_with(m)) {
            continue;
        }
        let source = std::fs::read_to_string(&file)
            .unwrap()
            .replace("\r\n", "\n");
        if super::sql_scan::without_tests(&source).contains("rusqlite") {
            offenders.push(relative);
        }
    }
    assert!(
        offenders.is_empty(),
        "go through catalog::db instead of rusqlite in {offenders:?}"
    );
}
