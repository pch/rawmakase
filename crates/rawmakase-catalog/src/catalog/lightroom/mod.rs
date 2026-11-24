//! Read-only Lightroom catalog import. Its Develop settings are converted by
//! `lr_develop`.
pub(super) mod history;
use super::Catalog;
use crate::catalog::db::{LightroomWrite, Reads, sql, sqlite_sql};
use anyhow::{Context, Result, ensure};
pub use history::HistoryStep;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};

/// A photo's filename in a Lightroom catalog, from its `AgLibraryFile` row
/// named `f`: SQL to `concat!` into a statement.
macro_rules! lightroom_filename {
    () => {
        "CASE WHEN f.idx_filename<>'' THEN f.idx_filename ELSE f.baseName||'.'||f.extension END"
    };
}
pub(in crate::catalog) use lightroom_filename;

impl Catalog {
    /// The Lightroom catalogs this catalog was imported from, by the paths
    /// they were imported from.
    pub fn lightroom_sources(&self) -> Result<Vec<PathBuf>> {
        let paths: Vec<String> = self.db.read(sql!("SELECT path FROM sources"), &[])?;
        Ok(paths.into_iter().map(PathBuf::from).collect())
    }
}

/// Copies Lightroom's Include on Export and Export Containing Keywords of
/// the keywords that have either off, from a catalog attached as `lr`.
fn copy_keyword_export(lr: &mut LightroomWrite<'_>) -> Result<usize> {
    let columns: Vec<String> = lr.read_sqlite(
        sqlite_sql!("SELECT name FROM pragma_table_info('AgLibraryKeyword', 'lr')"),
        &[],
    )?;
    let has = |c: &str| columns.iter().any(|n| n == c);
    if !has("includeOnExport") || !has("includeParents") {
        return Ok(0);
    }
    lr.execute_sqlite(
        sqlite_sql!(
            "INSERT INTO keyword_export(keyword, include, parents)
             SELECT id_local, COALESCE(includeOnExport, 1) <> 0, COALESCE(includeParents, 1) <> 0
             FROM lr.AgLibraryKeyword
             WHERE (includeOnExport = 0 OR includeParents = 0)
               AND id_local IN (SELECT id FROM keywords)
             ON CONFLICT(keyword) DO UPDATE SET include=excluded.include, parents=excluded.parents"
        ),
        &[],
    )
}

/// Copies what `AgLibraryCollectionContent` keeps of each collection
/// imported: a smart collection's rules (kept verbatim, also when they can't
/// be read) and a collection's sort, from a catalog attached as `lr`.
fn copy_collection_content(lr: &mut LightroomWrite<'_>) -> Result<()> {
    super::value::row! {
        struct Content {
            collection: crate::catalog::CollectionId,
            content: String,
        }
    }
    let rules: Vec<Content> = lr.read_sqlite(
        sqlite_sql!(
            "SELECT k.collection, k.content FROM lr.AgLibraryCollectionContent k
             JOIN collections c ON c.id = k.collection
             WHERE k.owningModule = 'ag.library.smart_collection'
               AND c.kind = 'com.adobe.ag.library.smart_collection'
               AND k.content IS NOT NULL
             ORDER BY k.id_local"
        ),
        &[],
    )?;
    let mut seen = std::collections::HashSet::new();
    for r in rules {
        // A collection with two rule rows keeps the first.
        if seen.insert(r.collection) {
            let rules = super::collections::SmartRules::from_lightroom(&r.content);
            super::collections::write_rules(lr.write(), r.collection, &rules)?;
        }
    }
    lr.execute_sqlite(
        sqlite_sql!(
            "INSERT INTO collection_options(collection, sort, direction)
             SELECT t.collection, t.content,
                    COALESCE((SELECT d.content FROM lr.AgLibraryCollectionContent d
                              WHERE d.collection = t.collection
                                AND d.owningModule = 'com.adobe.ag.library.sortDirection'
                                AND d.content IS NOT NULL
                              ORDER BY d.id_local LIMIT 1), 'ascending')
             FROM lr.AgLibraryCollectionContent t
             JOIN collections c ON c.id = t.collection
             WHERE t.owningModule = 'com.adobe.ag.library.sortType'
               AND t.content IS NOT NULL
               AND c.kind <> 'com.adobe.ag.library.group'
             ORDER BY t.id_local
             ON CONFLICT DO NOTHING"
        ),
        &[],
    )?;
    Ok(())
}

/// Imports a closed or exported Lightroom catalog into a new, atomically
/// published file. What the import doesn't read stays in the Lightroom
/// catalog only: the new catalog doesn't depend on it.
pub fn import_lightroom(source: &Path, destination: &Path) -> Result<PathBuf> {
    ensure!(
        !destination.exists(),
        "Destination exists; choose a new catalog filename"
    );
    ensure_no_live_journal(
        source,
        "Lightroom catalog has a live journal. Close Lightroom and copy/export the catalog with its companion files first",
    )?;
    let (snapshot, size) = take_snapshot(source)?;
    verify_snapshot(snapshot.path())?;
    let parent = crate::storage::parent_dir(destination);
    std::fs::create_dir_all(parent)?;
    let tmpdir = tempfile::tempdir_in(parent)?;
    let working = tmpdir.path().join("import.rawmakase");
    let mut catalog = Catalog::create(&working)?;
    catalog.db.with_lightroom(snapshot.path(), |lr| {
        // `original_catalog` stays empty: the schema keeps the column, and
        // releases that read it take an empty one as nothing to read.
        lr.write().execute(
            sql!("INSERT INTO sources(path,original_size,original_catalog) VALUES(?,?,?)"),
            &[&source.to_string_lossy(), &(size as i64), &Vec::<u8>::new()],
        )?;
        copy_tables(lr)
    })?;
    ensure!(
        catalog.db.is_intact()?,
        "Imported catalog failed integrity check"
    );
    drop(catalog);
    crate::storage::rename_noclobber(&working, destination)
        .context("Publish imported catalog without overwriting")?;
    Ok(destination.into())
}

/// Fails with `message` while Lightroom has `source` open: its WAL or
/// rollback journal is not empty.
fn ensure_no_live_journal(source: &Path, message: &str) -> Result<()> {
    for suffix in ["-wal", "-journal"] {
        let p = PathBuf::from(format!("{}{suffix}", source.display()));
        ensure!(!p.exists() || p.metadata()?.len() == 0, "{message}");
    }
    Ok(())
}

/// A copy of `source`, refused if it changed while being copied, and its size.
fn take_snapshot(source: &Path) -> Result<(tempfile::NamedTempFile, u64)> {
    let before = source.metadata()?;
    let snapshot = tempfile::NamedTempFile::new()?;
    std::fs::copy(source, snapshot.path())?;
    let after = source.metadata()?;
    ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "Source catalog changed during import; retry after closing Lightroom"
    );
    ensure_no_live_journal(
        source,
        "Source catalog became active during import; retry after closing Lightroom",
    )?;
    Ok((snapshot, before.len()))
}

/// Whether the Lightroom catalog open as `db` has table `name`.
fn has_table(db: &Connection, name: &str) -> Result<bool> {
    Ok(db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
            [name],
            |r| r.get::<_, i32>(0),
        )
        .optional()?
        .is_some())
}

/// Checks the snapshot is an intact Lightroom catalog with the tables an
/// import needs.
fn verify_snapshot(snapshot: &Path) -> Result<()> {
    let db = Connection::open_with_flags(snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    ensure!(
        db.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))? == "ok",
        "Source catalog failed SQLite integrity check"
    );
    for table in [
        "Adobe_images",
        "AgLibraryFile",
        "AgLibraryFolder",
        "AgLibraryRootFolder",
    ] {
        ensure!(
            has_table(&db, table)?,
            "Not a supported Lightroom catalog: missing {table}"
        );
    }
    Ok(())
}

/// Copies photos, folders, history, collections, keywords, info and
/// descriptive metadata from the Lightroom catalog attached as `lr`, in the
/// caller's transaction, and gives each photo its UUID. Fails if any image
/// was left out.
fn copy_tables(lr: &mut LightroomWrite<'_>) -> Result<()> {
    for statement in [
        sqlite_sql!(
            "INSERT INTO roots(id,original_path)
             SELECT id_local,absolutePath FROM lr.AgLibraryRootFolder"
        ),
        sqlite_sql!(
            "INSERT INTO folders(id,root,relative_path)
             SELECT id_local,rootFolder,pathFromRoot FROM lr.AgLibraryFolder"
        ),
        sqlite_sql!(concat!(
            "INSERT INTO photos(id,folder,filename,original_path,captured,rating,flag,label,format,copy_name,master_id,orientation)
             SELECT i.id_local,f.folder,",
            lightroom_filename!(),
            ",r.absolutePath||d.pathFromRoot||",
            lightroom_filename!(),
            ",COALESCE(i.captureTime,''),COALESCE(i.rating,0),COALESCE(i.pick,0),COALESCE(i.colorLabels,''),COALESCE(i.fileFormat,''),COALESCE(i.copyName,''),i.masterImage,i.orientation
             FROM lr.Adobe_images i JOIN lr.AgLibraryFile f ON f.id_local=i.rootFile JOIN lr.AgLibraryFolder d ON d.id_local=f.folder JOIN lr.AgLibraryRootFolder r ON r.id_local=d.rootFolder"
        )),
    ] {
        lr.execute_sqlite(statement, &[])?;
    }
    // Lightroom's ids are kept; new photos take ids above them.
    super::photo_ids::reserve(lr.write())?;
    // The name a master's file had when Lightroom imported it, for Import
    // Photos' suspected duplicates and the Original filename token. Its EXIF
    // capture time is read from the file in the background; Lightroom keeps
    // no file size.
    if lr.has_column("AgLibraryFile", "originalFilename")? {
        lr.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO photo_import_facts(photo, original_name, exif_captured, size, exif_read)
                 SELECT i.id_local, f.originalFilename, NULL, NULL, 0
                 FROM lr.Adobe_images i JOIN lr.AgLibraryFile f ON f.id_local=i.rootFile
                 WHERE i.masterImage IS NULL AND COALESCE(f.originalFilename, '')<>''"
            ),
            &[],
        )?;
    }
    if lr.has_table("Adobe_imageDevelopSettings")? {
        lr.execute_sqlite(
            sqlite_sql!(
                "UPDATE photos SET lightroom_develop=(SELECT text FROM lr.Adobe_imageDevelopSettings
                 WHERE image=photos.id LIMIT 1)"
            ),
            &[],
        )?;
    }
    if lr.has_table("Adobe_libraryImageDevelopHistoryStep")? {
        lr.execute_sqlite(history::COPY_LIGHTROOM_HISTORY, &[])?;
    }
    if lr.has_table("Adobe_libraryImageDevelopSnapshot")? {
        lr.execute_sqlite(crate::catalog::snapshots::COPY_LIGHTROOM_SNAPSHOTS, &[])?;
    }
    if lr.has_table("AgLibraryCollection")? {
        lr.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO collections SELECT id_local,name,parent,creationId
                 FROM lr.AgLibraryCollection"
            ),
            &[],
        )?;
    }
    if lr.has_table("AgLibraryCollectionImage")? {
        lr.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO collection_photos SELECT collection,image,positionInCollection
                 FROM lr.AgLibraryCollectionImage
                 WHERE collection IN(SELECT id FROM collections) AND image IN(SELECT id FROM photos)
                 ON CONFLICT DO NOTHING"
            ),
            &[],
        )?;
    }
    if lr.has_table("AgLibraryCollectionContent")? {
        copy_collection_content(lr)?;
    }
    if lr.has_table("Adobe_variablesTable")? {
        let target: Vec<String> = lr.read_sqlite(
            sqlite_sql!(
                "SELECT CAST(value AS TEXT) FROM lr.Adobe_variablesTable
                 WHERE name='targetCollection' AND value IS NOT NULL LIMIT 1"
            ),
            &[],
        )?;
        if let Some(target) = target.first() {
            super::collections::import_target(lr.write(), target)?;
        }
    }
    // Folder stacks: Lightroom's top is position 1, and it keeps whether a
    // stack is collapsed on the stack.
    if lr.has_table("AgLibraryFolderStack")? && lr.has_table("AgLibraryFolderStackImage")? {
        for statement in [
            sqlite_sql!(
                "INSERT INTO stacks(id,collapsed) SELECT id_local,collapsed<>0
                 FROM lr.AgLibraryFolderStack"
            ),
            sqlite_sql!(
                "INSERT INTO stack_photos(photo,stack,position)
                 SELECT image,stack,ROW_NUMBER() OVER (PARTITION BY stack
                     ORDER BY CAST(position AS REAL),id_local)
                 FROM lr.AgLibraryFolderStackImage
                 WHERE image IN(SELECT id FROM photos) AND stack IN(SELECT id FROM stacks)
                 ORDER BY stack,CAST(position AS REAL),id_local
                 ON CONFLICT DO NOTHING"
            ),
            sqlite_sql!("DELETE FROM stacks WHERE id NOT IN(SELECT stack FROM stack_photos)"),
        ] {
            lr.execute_sqlite(statement, &[])?;
        }
    }
    if lr.has_table("AgLibraryKeyword")? {
        lr.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO keywords SELECT id_local,COALESCE(name,''),parent
                 FROM lr.AgLibraryKeyword"
            ),
            &[],
        )?;
        copy_keyword_export(lr)?;
        if lr.has_table("AgLibraryKeywordSynonym")? {
            lr.execute_sqlite(
                sqlite_sql!(
                    "INSERT INTO keyword_synonyms(keyword, name)
                     SELECT keyword, name FROM lr.AgLibraryKeywordSynonym
                     WHERE keyword IN (SELECT id FROM keywords) AND trim(COALESCE(name, '')) <> ''
                     ON CONFLICT DO NOTHING"
                ),
                &[],
            )?;
        }
    }
    super::info::copy_lightroom_info(lr)?;
    super::sidecar::copy_lightroom_metadata(lr)?;
    if lr.has_table("AgLibraryKeywordImage")? {
        lr.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO photo_keywords SELECT image,tag FROM lr.AgLibraryKeywordImage
                 WHERE image IN(SELECT id FROM photos) AND tag IN(SELECT id FROM keywords)
                 ON CONFLICT DO NOTHING"
            ),
            &[],
        )?;
    }
    let imported: i64 = lr
        .write()
        .read_one(sql!("SELECT count(*) FROM photos"), &[])?;
    let expected: i64 = lr
        .read_sqlite(sqlite_sql!("SELECT count(*) FROM lr.Adobe_images"), &[])?
        .pop()
        .unwrap_or_default();
    ensure!(
        imported == expected,
        "Catalog has orphaned image records ({imported}/{expected}); import rolled back"
    );
    // Random, as every photo's: Lightroom's own `id_global` is not taken.
    // Here rather than on the next open, so the catalog published is whole.
    super::photo_uuids::give_missing(lr.write())
}
