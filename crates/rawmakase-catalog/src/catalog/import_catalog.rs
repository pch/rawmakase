//! Import from Another Catalog: the photos of another RAWmakase catalog added to
//! this one, with everything that catalog holds about them, as Lightroom's
//! Import from Another Catalog adds them ([`Catalog::import_catalog`]).
//!
//! The other catalog is never changed. A private copy of it is made
//! (SQLite's own, as Export as Catalog makes one, staged in a hidden
//! temporary folder beside this catalog; a crash or force-quit mid-import
//! leaves at most that folder with the unfinished copy), upgraded to this
//! release's format when older, and reduced to the photos to add: those
//! this catalog has already, by UUID or by file, those whose file isn't
//! there on this computer and those whose folder can't be placed are
//! taken out of the copy, as Export as Catalog takes out the photos it leaves.
//! What is left is copied row by row, one statement per table, with the
//! copy attached in an immediate transaction; each row keeps its bytes, so an
//! edit, History or metadata a later release extends arrives as it was. Ids are
//! shifted above every id this catalog has given. Folders are placed as Add
//! Folder places them, by this computer's folder locations; when an anchor
//! would enclose a folder this catalog already has, the shallowest folder
//! below it that does not enclose one becomes the new root instead (and a
//! photo whose own folder still encloses one is left out). Keywords and
//! collections are found by their path and made where this catalog has none.
//! Smart collections stay with the other catalog.
//!
//! The tests list what happens to each table of the schema, and fail when
//! it gains one they don't list.
use super::db::{LightroomWrite, Reads, sql, sqlite_sql};
use super::ingest::{ImportKind, Matched, Matcher, Place, folder_id, known, new_batch};
use super::value::row;
use super::{Catalog, CollectionId, FolderId, PhotoId, RootId};
use crate::metadata::keyword_key;
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What an import added and what it left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogImport {
    /// Photos added, virtual copies included.
    pub added: usize,
    /// Photos left out because this catalog has them already, by their
    /// UUID or their master's file, virtual copies included. Nothing of them
    /// changes.
    pub present: usize,
    /// Masters whose file isn't where the other catalog has it on this
    /// computer, left out with their virtual copies.
    pub missing: Vec<PathBuf>,
    /// Masters whose folder could not be placed in this catalog (located
    /// elsewhere here, matching more than one location, enclosing an existing
    /// catalog folder, or split across paths), left out with their virtual
    /// copies.
    pub unplaced: Vec<PathBuf>,
}

/// Where a folder of the other catalog goes in this one.
struct Target {
    place: Place,
    /// The folder the root it makes is, for [`Place::New`].
    root: PathBuf,
    /// The canonical folder on disk.
    directory_path: PathBuf,
    /// The folder on disk, ending in a separator: the start of its photos'
    /// paths.
    directory: String,
}

/// Cached filenames per `(RootId, logical)` folder in this catalog, indexed
/// case-insensitively so checking whether a file is already present stays
/// linear while still catching case-differing aliases on case-insensitive
/// filesystems.
#[derive(Default)]
struct FolderNames {
    by_folder: HashMap<(RootId, String), HashMap<String, Vec<String>>>,
}

impl FolderNames {
    fn has_file(
        &mut self,
        r: &impl Reads,
        root: RootId,
        logical: &str,
        directory: &Path,
        filename: &str,
    ) -> Result<bool> {
        let file = directory.join(filename);
        if known(r, root, logical, &file)? {
            return Ok(true);
        }
        let key = (root, logical.to_string());
        let names = match self.by_folder.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let stored: Vec<String> = r.read(
                    sql!(
                        "SELECT DISTINCT filename FROM photos WHERE folder IN
                         (SELECT f.id FROM folders f JOIN folder_paths p ON p.folder=f.id
                          WHERE f.root=? AND p.path=?)"
                    ),
                    &[&root, &logical],
                )?;
                let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
                for name in stored {
                    let folded = super::sidecar::caseless(std::ffi::OsStr::new(&name));
                    grouped.entry(folded).or_default().push(name);
                }
                entry.insert(grouped)
            }
        };
        let want = super::sidecar::caseless(std::ffi::OsStr::new(filename));
        let Some(candidates) = names.get(&want) else {
            return Ok(false);
        };
        let real = file.canonicalize().ok();
        for existing in candidates {
            if existing == filename {
                return Ok(true);
            }
            if real.is_some() && directory.join(existing).canonicalize().ok() == real {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// `directory` ending in a path separator without doubling a drive root's.
fn dir_prefix(directory: &Path) -> String {
    let mut text = directory.to_string_lossy().into_owned();
    if !text.ends_with(std::path::MAIN_SEPARATOR) && !(cfg!(windows) && text.ends_with('/')) {
        text.push(std::path::MAIN_SEPARATOR);
    }
    text
}

/// The shallowest ancestor of `directory` strictly below `anchor` that does
/// not hold any existing catalog location below it, paired with `directory`'s
/// logical path inside that root. Returns `None` when `directory` itself holds
/// an existing catalog location.
fn shallowest_free_root(
    matcher: &Matcher,
    directory: &Path,
    anchor: &Path,
) -> Option<(PathBuf, String)> {
    if matcher.holds_below(directory) {
        return None;
    }
    let mut best = directory;
    for ancestor in directory.ancestors().skip(1) {
        if ancestor == anchor || !ancestor.starts_with(anchor) || matcher.holds_below(ancestor) {
            break;
        }
        best = ancestor;
    }
    let rest = directory.strip_prefix(best).unwrap_or(Path::new(""));
    Some((best.to_path_buf(), super::locations::logical_from_os(rest)))
}

/// A deduplication key for `filename` in canonical `directory`: uses the
/// disk's canonical filename when the file is not a symlink to another
/// directory, so case-differing spellings on a case-insensitive filesystem
/// collapse to one key while symlinks keep their link name in `directory`.
fn file_dedup_key(directory: &Path, filename: &str) -> (PathBuf, String) {
    let file = directory.join(filename);
    let canonical_in_dir = file
        .canonicalize()
        .ok()
        .filter(|real| real.parent() == Some(directory))
        .and_then(|real| real.file_name().map(|n| n.to_string_lossy().into_owned()));
    (
        directory.to_path_buf(),
        canonical_in_dir.unwrap_or_else(|| filename.to_string()),
    )
}

impl Catalog {
    /// Adds the photos of the catalog at `source` to this one, as described
    /// in the [module](self) docs, in one transaction. `source` is only
    /// read. Returns `None`, with nothing written, when `begin`, asked once
    /// the other catalog is read and before the transaction, says no.
    pub fn import_catalog(
        &mut self,
        source: &Path,
        begin: impl FnOnce() -> bool,
    ) -> Result<Option<CatalogImport>> {
        let super::CatalogLocation::File(own) = &self.location;
        ensure!(
            !matches!(
                (source.canonicalize(), own.canonicalize()),
                (Ok(s), Ok(o)) if s == o
            ),
            "That is the catalog open now; choose another"
        );
        let parent = crate::storage::parent_dir(own);
        let staging = tempfile::tempdir_in(parent)?;
        let working = staging.path().join("import.rawmakase");
        super::db::Db::copy_file(source, &working).context("Could not read the catalog")?;
        let mut copy = Catalog::open_as(working.as_path(), &self.computer)?;
        let unknown = copy.unknown_tables()?;
        ensure!(
            unknown.is_empty(),
            "A later release of RAWmakase added to that catalog ({}); import it with that release",
            unknown.join(", ")
        );
        ensure!(
            !super::file_ops::any_unresolved(&copy.db)?,
            "A file operation in that catalog is unfinished; open it to finish it first"
        );
        let (report, targets) = self.reduce(&mut copy)?;
        drop(copy);
        if !begin() {
            return Ok(None);
        }
        let mut report = report;
        let computer = self.computer.id.clone();
        report.added = self
            .db
            .with_catalog(&working, |w| copy_rows(w, &targets, &computer))?;
        Ok(Some(report))
    }

    /// Takes out of `copy` the photos this catalog won't add, and what
    /// only they referred to. Returns what was left out and where each
    /// folder of what is left goes.
    fn reduce(&self, copy: &mut Catalog) -> Result<(CatalogImport, HashMap<FolderId, Target>)> {
        let mut report = CatalogImport::default();
        let photos = copy.photos()?;
        row! {
            struct Uuid {
                photo: PhotoId,
                uuid: String,
            }
        }
        let uuids: HashMap<PhotoId, String> = copy
            .db
            .read::<Uuid>(sql!("SELECT photo, uuid FROM photo_uuids"), &[])?
            .into_iter()
            .map(|u| (u.photo, u.uuid))
            .collect();
        let masters: HashSet<PhotoId> = photos
            .iter()
            .filter(|p| p.master.is_none())
            .map(|p| p.id)
            .collect();
        let mut families: BTreeMap<PhotoId, Vec<PhotoId>> = BTreeMap::new();
        let mut leave: BTreeSet<PhotoId> = BTreeSet::new();
        for p in &photos {
            match p.master {
                Some(master) if !masters.contains(&master) => {
                    // An orphaned virtual copy whose master is missing stays behind.
                    leave.insert(p.id);
                }
                _ => {
                    families
                        .entry(p.master.unwrap_or(p.id))
                        .or_default()
                        .push(p.id);
                }
            }
        }
        let folder_of: HashMap<PhotoId, FolderId> =
            photos.iter().map(|p| (p.id, p.folder)).collect();
        // Where the other catalog's roots, and folders located on their own,
        // are on this computer.
        let mut anchors: Vec<PathBuf> = copy
            .roots()?
            .into_iter()
            .map(|(_, original, here)| PathBuf::from(here.unwrap_or(original)))
            .chain(
                copy.location_rows()?
                    .into_values()
                    .flatten()
                    .map(|(_, path)| path),
            )
            .filter_map(|path| path.canonicalize().ok())
            .collect();
        anchors.sort();
        anchors.dedup();
        // Each master's canonical folder and stored filename, grouped by the
        // anchor chosen for it as Add Folder is given one: the most specific of
        // those that holds it, else its own folder.
        let mut by_root: BTreeMap<PathBuf, Vec<(PhotoId, PathBuf, String)>> = BTreeMap::new();
        for p in photos.iter().filter(|p| p.master.is_none()) {
            let family = &families[&p.id];
            let present = self.db.snapshot(|s| {
                for member in family {
                    let Some(uuid) = uuids.get(member) else {
                        continue;
                    };
                    let found: Option<i64> = s.read_optional(
                        sql!("SELECT photo FROM photo_uuids WHERE uuid=?"),
                        &[uuid],
                    )?;
                    if found.is_some() {
                        return Ok(true);
                    }
                }
                Ok(false)
            })?;
            if present {
                report.present += family.len();
                leave.extend(family);
                continue;
            }
            // Canonicalize only the parent directory, not the file itself, so
            // stored filenames keep their spelling and file symlinks keep their
            // link directory and name instead of pointing to a non-existent path.
            let directory = p
                .path
                .is_file()
                .then(|| p.path.parent().and_then(|d| d.canonicalize().ok()))
                .flatten();
            match directory {
                Some(directory) => {
                    let chosen = anchors
                        .iter()
                        .filter(|a| directory.starts_with(a))
                        .max_by_key(|a| a.components().count())
                        .cloned()
                        .unwrap_or_else(|| directory.clone());
                    by_root
                        .entry(chosen)
                        .or_default()
                        .push((p.id, directory, p.filename.clone()));
                }
                None => {
                    let missing = if p.path.as_os_str().is_empty() {
                        PathBuf::from(&p.filename)
                    } else {
                        p.path.clone()
                    };
                    report.missing.push(missing);
                    leave.extend(family);
                }
            }
        }
        // Placed as Add Folder places the folder each root is, refusing any new
        // root that would enclose a root or location this catalog already has.
        let matcher = Matcher::new(self)?;
        let mut existing_names = FolderNames::default();
        let mut seen_files: HashSet<(PathBuf, String)> = HashSet::new();
        let mut targets: HashMap<FolderId, Target> = HashMap::new();
        for (root, files) in by_root {
            for (id, directory, filename) in files {
                let family = &families[&id];
                let file = directory.join(&filename);
                let (place, target_root) = match matcher.place(&directory, &root, &[]) {
                    Matched::Place(Place::Folder(root_id, logical)) => {
                        (Place::Folder(root_id, logical), root.clone())
                    }
                    Matched::Place(Place::New(logical)) if !matcher.holds_below(&root) => {
                        (Place::New(logical), root.clone())
                    }
                    Matched::Place(Place::New(_)) => {
                        // The chosen anchor holds an existing catalog folder,
                        // so use the shallowest ancestor below it that does not
                        // enclose one, keeping siblings and subfolders together
                        // under one root.
                        match shallowest_free_root(&matcher, &directory, &root) {
                            Some((free_root, logical)) => (Place::New(logical), free_root),
                            None => {
                                report.unplaced.push(file);
                                leave.extend(family);
                                continue;
                            }
                        }
                    }
                    Matched::Conflict(_) | Matched::Ambiguous(_) => {
                        report.unplaced.push(file);
                        leave.extend(family);
                        continue;
                    }
                };
                if let Place::Folder(root_id, logical) = &place
                    && self.db.snapshot(|s| {
                        existing_names.has_file(s, *root_id, logical, &directory, &filename)
                    })?
                {
                    report.present += family.len();
                    leave.extend(family);
                    continue;
                }
                // Two source folders resolving to the same directory with the
                // same file on disk must not insert duplicate master rows.
                if !seen_files.insert(file_dedup_key(&directory, &filename)) {
                    report.present += family.len();
                    leave.extend(family);
                    continue;
                }
                let folder = folder_of[&id];
                let prefix = dir_prefix(&directory);
                // Photos are copied folder by folder, so a folder's photos
                // all go to the first one's place; one found elsewhere is left out.
                let target = targets.entry(folder).or_insert(Target {
                    place,
                    root: target_root,
                    directory_path: directory,
                    directory: prefix.clone(),
                });
                if target.directory != prefix {
                    report.unplaced.push(file);
                    leave.extend(family);
                }
            }
        }
        copy.db.write(|w| {
            for id in &leave {
                super::copies::delete_photo(w, *id)?;
            }
            // Drop any rows left behind by photos removed in earlier releases
            // and degenerate single-photo stacks before pruning references.
            for statement in [
                sql!("DELETE FROM photo_keywords WHERE photo NOT IN (SELECT id FROM photos)"),
                sql!("DELETE FROM collection_photos WHERE photo NOT IN (SELECT id FROM photos)"),
                sql!("DELETE FROM stack_photos WHERE photo NOT IN (SELECT id FROM photos)"),
                sql!(
                    "DELETE FROM stack_photos WHERE stack IN
                     (SELECT stack FROM stack_photos GROUP BY stack HAVING count(*) < 2)"
                ),
            ] {
                w.execute(statement, &[])?;
            }
            super::export_catalog::prune_unreferenced(w)?;
            // Smart collections belong to the other catalog, not to its photos:
            // leaving them behind avoids re-adding deleted defaults on import.
            for statement in [
                sql!("DELETE FROM smart_collection_rules"),
                sql!(
                    "WITH RECURSIVE used(id) AS (
                         SELECT collection FROM collection_photos
                         UNION SELECT c.parent FROM collections c JOIN used u ON c.id=u.id
                         WHERE c.parent IS NOT NULL)
                     DELETE FROM collection_options WHERE collection NOT IN (SELECT id FROM used)"
                ),
                sql!(
                    "WITH RECURSIVE used(id) AS (
                         SELECT collection FROM collection_photos
                         UNION SELECT c.parent FROM collections c JOIN used u ON c.id=u.id
                         WHERE c.parent IS NOT NULL)
                     DELETE FROM collections WHERE id NOT IN (SELECT id FROM used)"
                ),
            ] {
                w.execute(statement, &[])?;
            }
            if let Some(unnamed) = super::mask_assets::unnamed(w)? {
                super::mask_assets::delete(w, &unnamed)?;
            }
            super::export_catalog::prune_recipe_assets(w)
        })?;
        let kept: BTreeSet<FolderId> = photos
            .iter()
            .filter(|p| !leave.contains(&p.id))
            .map(|p| p.folder)
            .collect();
        targets.retain(|folder, _| kept.contains(folder));
        Ok((report, targets))
    }
}

/// Copies every photo of the attached catalog `src`, and what is about
/// them, into this one, the folders going to `targets`. Returns how many
/// photos were added.
fn copy_rows(
    w: &mut LightroomWrite<'_>,
    targets: &HashMap<FolderId, Target>,
    computer: &str,
) -> Result<usize> {
    let count: i64 = w.read_sqlite::<i64>(sqlite_sql!("SELECT count(*) FROM src.photos"), &[])?[0];
    if count == 0 {
        return Ok(0);
    }
    // Check if any photo was added here by UUID since the other catalog was read.
    let uuid_conflict: Vec<i64> = w.read_sqlite(
        sqlite_sql!(
            "SELECT 1 FROM src.photo_uuids s
             JOIN photo_uuids m ON m.uuid = s.uuid
             WHERE s.photo IN (SELECT id FROM src.photos)
             LIMIT 1"
        ),
        &[],
    )?;
    ensure!(
        uuid_conflict.is_empty(),
        "Some of these photos were added to this catalog meanwhile; import the catalog again"
    );
    // A file an unfinished Import Photos of this computer is copying is
    // that import's to add, as Add Folder leaves it.
    let reserved = super::file_ops::reserved_for_import(w.write(), computer)?;
    let live_matcher = Matcher::read(w.write(), computer)?;
    let mut existing_names = FolderNames::default();
    for (from, target) in targets {
        // Re-check placement for new roots so a folder added concurrently
        // cannot produce a duplicate root or duplicate master rows.
        if let Place::New(_) = &target.place {
            ensure!(
                matches!(
                    live_matcher.place(&target.directory_path, &target.root, &[]),
                    Matched::Place(Place::New(_))
                ) && !live_matcher.holds_below(&target.root),
                "{} was added to this catalog meanwhile; import the catalog again",
                target.root.display()
            );
        }
        let names: Vec<String> = w.read_sqlite(
            sqlite_sql!("SELECT filename FROM src.photos WHERE folder=? AND master_id IS NULL"),
            &[from],
        )?;
        for name in names {
            let file = format!("{}{name}", target.directory);
            ensure!(
                !reserved.contains(&super::sidecar::caseless(std::ffi::OsStr::new(&file))),
                "{file} is being imported by Import Photos; finish or recover that import first"
            );
            // Added here since the other catalog was read.
            if let Place::Folder(root, logical) = &target.place {
                ensure!(
                    !existing_names.has_file(
                        w.write(),
                        *root,
                        logical,
                        &target.directory_path,
                        &name
                    )?,
                    "{file} was added to this catalog meanwhile; import the catalog again"
                );
            }
        }
    }
    let highest: i64 = w.read_sqlite::<i64>(sqlite_sql!("SELECT max(id) FROM src.photos"), &[])?[0];
    let photo = super::photo_ids::reserve_block(w.write(), highest)?;
    let batch = new_batch(w.write(), ImportKind::Catalog)?;
    let mut roots: HashMap<PathBuf, RootId> = HashMap::new();
    let mut folders: BTreeMap<FolderId, FolderId> = BTreeMap::new();
    for (from, target) in targets {
        let (root, logical) = match &target.place {
            Place::Folder(root, logical) => (*root, logical.clone()),
            Place::New(logical) => {
                let root = match roots.get(&target.root) {
                    Some(root) => *root,
                    None => {
                        let root = w.write().insert_returning_id(
                            sql!("INSERT INTO roots(original_path) VALUES(?) RETURNING id"),
                            &[&target.root.to_string_lossy()],
                        )?;
                        roots.insert(target.root.clone(), root);
                        root
                    }
                };
                (root, logical.clone())
            }
        };
        let to = folder_id(w.write(), root, &logical)?;
        folders.insert(*from, to);
        w.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO photos(id, folder, filename, original_path, captured, rating, flag,
                     label, format, copy_name, master_id, orientation, lightroom_develop)
                 SELECT id+?1, ?2, filename, ?3 || filename, captured, rating, flag, label,
                     format, copy_name, master_id+?1, orientation, lightroom_develop
                 FROM src.photos WHERE folder=?4"
            ),
            &[&photo, &to, &target.directory, from],
        )?;
    }
    let added: i64 = w.read_sqlite::<i64>(
        sqlite_sql!("SELECT count(*) FROM photos WHERE id IN (SELECT id+? FROM src.photos)"),
        &[&photo],
    )?[0];
    ensure!(
        added == count,
        "Not every photo of the catalog found its folder"
    );
    let snapshot: i64 = w
        .write()
        .read_one::<Option<i64>>(sql!("SELECT max(id) FROM develop_snapshots"), &[])?
        .unwrap_or(0);
    let stack: i64 = w
        .write()
        .read_one::<Option<i64>>(sql!("SELECT max(id) FROM stacks"), &[])?
        .unwrap_or(0);
    // Their edits, as edit_rows alone writes them.
    super::edit_rows::copy_attached(w, photo)?;
    // Each row of the photos added, with the ids it names shifted.
    for statement in [
        sqlite_sql!(
            "INSERT INTO photo_uuids(photo, uuid) SELECT photo+?1, uuid FROM src.photo_uuids
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        sqlite_sql!(
            "INSERT INTO lightroom_history(photo, position, name, created, text)
             SELECT photo+?1, position, name, created, text FROM src.lightroom_history
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        sqlite_sql!(
            "INSERT INTO photo_info(photo, camera, lens, focal, aperture, exposure, iso, width,
                 height)
             SELECT photo+?1, camera, lens, focal, aperture, exposure, iso, width, height
             FROM src.photo_info
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        sqlite_sql!(
            "INSERT INTO photo_checksums(photo, size, quick_hash, full_hash, identity, version)
             SELECT photo+?1, size, quick_hash, full_hash, identity, version
             FROM src.photo_checksums
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        sqlite_sql!(
            "INSERT INTO photo_sequences(photo, camera, burst, frame, size, modified_ns, version)
             SELECT photo+?1, camera, burst, frame, size, modified_ns, version
             FROM src.photo_sequences
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        sqlite_sql!(
            "INSERT INTO photo_import_facts(photo, original_name, exif_captured, size, exif_read)
             SELECT photo+?1, original_name, exif_captured, size, exif_read
             FROM src.photo_import_facts
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        sqlite_sql!(
            "INSERT INTO photo_fields(photo, field, state)
             SELECT photo+?1, field, state FROM src.photo_fields
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        sqlite_sql!(
            "INSERT INTO photo_text(photo, field, lang, position, value)
             SELECT photo+?1, field, lang, position, value FROM src.photo_text
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        sqlite_sql!(
            "INSERT INTO photo_creators(photo, position, name)
             SELECT photo+?1, position, name FROM src.photo_creators
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        sqlite_sql!(
            "INSERT INTO photo_capture(photo, captured, subsec, \"offset\")
             SELECT photo+?1, captured, subsec, \"offset\" FROM src.photo_capture
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        sqlite_sql!(
            "INSERT INTO photo_location(photo, lat, lon, alt, cleared)
             SELECT photo+?1, lat, lon, alt, cleared FROM src.photo_location
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
    ] {
        w.execute_sqlite(statement, &[&photo])?;
    }
    w.execute_sqlite(
        sqlite_sql!("INSERT INTO photo_imports(photo, batch) SELECT id+?1, ?2 FROM src.photos"),
        &[&photo, &batch],
    )?;
    w.execute_sqlite(
        sqlite_sql!(
            "INSERT INTO develop_snapshots(id, photo, name, recipe, lightroom)
             SELECT id+?2, photo+?1, name, recipe, lightroom FROM src.develop_snapshots
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        &[&photo, &snapshot],
    )?;
    w.execute_sqlite(
        sqlite_sql!(
            "INSERT INTO stacks(id, collapsed) SELECT id+?1, collapsed FROM src.stacks
             WHERE id IN (SELECT stack FROM src.stack_photos WHERE photo IN (SELECT id FROM src.photos))"
        ),
        &[&stack],
    )?;
    w.execute_sqlite(
        sqlite_sql!(
            "INSERT INTO stack_photos(photo, stack, position)
             SELECT photo+?1, stack+?2, position FROM src.stack_photos
             WHERE photo IN (SELECT id FROM src.photos)"
        ),
        &[&photo, &stack],
    )?;
    // Named by their content, so one this catalog has is the same.
    for statement in [
        sqlite_sql!(
            "INSERT INTO bitmaps(hash, data) SELECT hash, data FROM src.bitmaps WHERE true
             ON CONFLICT DO NOTHING"
        ),
        sqlite_sql!(
            "INSERT INTO recipe_assets(id, kind, data) SELECT id, kind, data
             FROM src.recipe_assets WHERE true ON CONFLICT DO NOTHING"
        ),
    ] {
        w.execute_sqlite(statement, &[])?;
    }
    copy_keywords(w, photo)?;
    copy_collections(w, photo)?;
    Ok(count as usize)
}

/// A keyword or collection row of the attached catalog.
#[derive(Clone)]
struct Named {
    id: i64,
    name: String,
    parent: Option<i64>,
    kind: String,
}

/// `rows` with every parent before its children.
fn parents_first(rows: Vec<Named>) -> Vec<Named> {
    let mut placed: HashSet<i64> = HashSet::new();
    let mut ordered = Vec::with_capacity(rows.len());
    let mut left = rows;
    while !left.is_empty() {
        let before = left.len();
        let (ready, rest): (Vec<Named>, Vec<Named>) = left
            .into_iter()
            .partition(|r| r.parent.is_none_or(|p| placed.contains(&p)));
        placed.extend(ready.iter().map(|r| r.id));
        ordered.extend(ready);
        left = rest;
        // A parent that isn't there: the rest go at the top.
        if left.len() == before {
            for r in &mut left {
                r.parent = None;
            }
        }
    }
    ordered
}

/// Gives the photos added the attached catalog's keywords, each found by
/// its path, ignoring case as the Keyword List does, else made with its
/// synonyms and export options.
fn copy_keywords(w: &mut LightroomWrite<'_>, photo: i64) -> Result<()> {
    row! {
        struct Row {
            id: i64,
            name: String,
            parent: Option<i64>,
        }
    }
    let rows: Vec<Row> = w.read_sqlite(
        sqlite_sql!("SELECT id, name, parent FROM src.keywords"),
        &[],
    )?;
    // Lightroom's hidden root is this catalog's, where it has one.
    let hidden: BTreeSet<i64> = rows
        .iter()
        .filter(|r| super::keywords::is_root(&r.name, r.parent))
        .map(|r| r.id)
        .collect();
    let rows = parents_first(
        rows.into_iter()
            .filter(|r| !hidden.contains(&r.id))
            .map(|r| Named {
                id: r.id,
                parent: r.parent.filter(|p| !hidden.contains(p)),
                name: r.name,
                kind: String::new(),
            })
            .collect(),
    );
    let mut existing = super::keywords::keyword_list(w.write())?;
    let mut mapped: HashMap<i64, i64> = HashMap::new();
    for row in rows {
        let parent = row.parent.and_then(|p| mapped.get(&p).copied());
        let key = keyword_key(&row.name);
        let found = existing
            .iter()
            .find(|k| k.parent == parent && keyword_key(&k.name) == key)
            .map(|k| k.id);
        let id = match found {
            Some(id) => id,
            None => {
                let under = match parent {
                    Some(p) => Some(p),
                    None => super::keywords::root(w.write())?,
                };
                let id = super::keywords::new_keyword(w.write(), &row.name, under)?;
                w.execute_sqlite(
                    sqlite_sql!(
                        "INSERT INTO keyword_export(keyword, include, parents)
                         SELECT ?1, include, parents FROM src.keyword_export WHERE keyword=?2"
                    ),
                    &[&id, &row.id],
                )?;
                existing.push(super::keywords::ListedKeyword {
                    id,
                    name: row.name.clone(),
                    parent,
                    photos: 0,
                });
                id
            }
        };
        mapped.insert(row.id, id);
        w.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO keyword_synonyms(keyword, name)
                 SELECT ?1, name FROM src.keyword_synonyms WHERE keyword=?2
                 ON CONFLICT DO NOTHING"
            ),
            &[&id, &row.id],
        )?;
        w.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO photo_keywords(photo, keyword)
                 SELECT photo+?1, ?2 FROM src.photo_keywords
                 WHERE keyword=?3 AND photo IN (SELECT id FROM src.photos)
                 ON CONFLICT DO NOTHING"
            ),
            &[&photo, &id, &row.id],
        )?;
    }
    Ok(())
}

/// Adds the photos added to the attached catalog's collections, each found
/// by its path and kind, else made with its sort. In a collection
/// this catalog had, they go after its photos, in no order of their own.
fn copy_collections(w: &mut LightroomWrite<'_>, photo: i64) -> Result<()> {
    row! {
        struct Row {
            id: i64,
            name: String,
            parent: Option<i64>,
            kind: String,
        }
    }
    let rows: Vec<Row> = w.read_sqlite(
        sqlite_sql!("SELECT id, name, parent, kind FROM src.collections"),
        &[],
    )?;
    let rows = parents_first(
        rows.into_iter()
            .map(|r| Named {
                id: r.id,
                name: r.name,
                parent: r.parent,
                kind: r.kind,
            })
            .collect(),
    );
    let before: Vec<Row> = w
        .write()
        .read(sql!("SELECT id, name, parent, kind FROM collections"), &[])?;
    let mut made_here: Vec<Row> = Vec::new();
    let mut mapped: HashMap<i64, CollectionId> = HashMap::new();
    for row in rows {
        let parent = row.parent.and_then(|p| mapped.get(&p).copied());
        // The first of `name`, `name 2`, … that is a collection of the same
        // kind this catalog had before, or free: a name is unique among a
        // set's children, whatever their kind.
        let mut n = 1;
        let (id, made) = loop {
            let name = match n {
                1 => row.name.clone(),
                n => format!("{} {n}", row.name),
            };
            let here = |c: &Row| c.parent == parent.map(|p| p.0) && c.name == name;
            if let Some(c) = before.iter().find(|c| here(c) && c.kind == row.kind) {
                break (CollectionId(c.id), false);
            }
            if before.iter().chain(&made_here).any(here) {
                n += 1;
                continue;
            }
            let id = super::collections::insert(w.write(), &name, parent, &row.kind)?;
            w.execute_sqlite(
                sqlite_sql!(
                    "INSERT INTO collection_options(collection, sort, direction)
                     SELECT ?1, sort, direction FROM src.collection_options
                     WHERE collection=?2"
                ),
                &[&id, &row.id],
            )?;
            made_here.push(Row {
                id: id.0,
                name,
                parent: parent.map(|p| p.0),
                kind: row.kind.clone(),
            });
            break (id, true);
        };
        mapped.insert(row.id, id);
        w.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO collection_photos(collection, photo, position)
                 SELECT ?1, photo+?2, CASE WHEN ?3 THEN position END
                 FROM src.collection_photos
                 WHERE collection=?4 AND photo IN (SELECT id FROM src.photos)
                 ON CONFLICT DO NOTHING"
            ),
            &[&id, &photo, &made, &row.id],
        )?;
    }
    Ok(())
}
