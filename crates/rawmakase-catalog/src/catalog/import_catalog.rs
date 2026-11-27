//! Import from Catalog: the photos of another RAWmakase catalog added to
//! this one, with everything that catalog holds about them, as Lightroom's
//! Import from Another Catalog adds them ([`Catalog::import_catalog`]).
//!
//! The other catalog is never changed. A private copy of it is made
//! (SQLite's own, as Export as Catalog makes one), upgraded to this
//! release's format when older, and reduced to the photos to add: those
//! this catalog has already, by UUID or by file, those whose file isn't
//! there on this computer and those whose folder can't be placed are taken
//! out of the copy, as Export as Catalog takes out the photos it leaves.
//! What is left is copied row by row, one statement per table, with the
//! copy attached; each row keeps its bytes, so an edit, History or
//! metadata a later release extends arrives as it was. Ids are shifted
//! above every id this catalog has given. Folders are placed as Add Folder
//! places them, by this computer's folder locations; keywords and
//! collections are found by their path and made where this catalog has
//! none.
//!
//! The tests list what happens to each table of the schema, and fail when
//! it gains one they don't list.
use super::db::{LightroomWrite, Reads, sql, sqlite_sql};
use super::ingest::{ImportKind, Place, folder_id, known, new_batch};
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
    /// Masters whose folder this catalog has elsewhere on this computer, or
    /// can't tell apart from another, left out with their virtual copies.
    pub unplaced: Vec<PathBuf>,
}

/// Where a folder of the other catalog goes in this one.
struct Target {
    place: Place,
    /// The folder the root it makes is, for [`Place::New`].
    root: PathBuf,
    /// The folder on disk, ending in a separator: the start of its photos'
    /// paths.
    directory: String,
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
            source.canonicalize().ok() != own.canonicalize().ok(),
            "That is the catalog open now; choose another"
        );
        let staging = tempfile::tempdir()?;
        let working = staging.path().join("import.rawmakase");
        super::db::Db::open(&source.to_path_buf().into())?
            .copy_to(&working)
            .context("Could not read the catalog")?;
        let mut copy = Catalog::open_as(working.as_path(), &self.computer)?;
        copy.ensure_known_tables()?;
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
        report.added = self.db.with_catalog(&working, |w| copy_rows(w, &targets))?;
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
        let mut families: BTreeMap<PhotoId, Vec<PhotoId>> = BTreeMap::new();
        for p in &photos {
            families
                .entry(p.master.unwrap_or(p.id))
                .or_default()
                .push(p.id);
        }
        let folders: HashMap<FolderId, super::Folder> =
            copy.folders()?.into_iter().map(|f| (f.id, f)).collect();
        let mut leave: BTreeSet<PhotoId> = BTreeSet::new();
        // Each master's file as it is on disk, by the folder its root is.
        let mut by_root: BTreeMap<PathBuf, Vec<(PhotoId, PathBuf)>> = BTreeMap::new();
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
            let found = p.path.canonicalize().ok().filter(|f| f.is_file());
            let root = folders.get(&p.folder).and_then(root_of);
            match (found, root) {
                (Some(file), Some(root)) => by_root.entry(root).or_default().push((p.id, file)),
                _ => {
                    report.missing.push(p.path.clone());
                    leave.extend(family);
                }
            }
        }
        // Placed as Add Folder places the folder each root is.
        let mut targets: HashMap<FolderId, Target> = HashMap::new();
        for (root, files) in by_root {
            let paths: Vec<PathBuf> = files.iter().map(|(_, f)| f.clone()).collect();
            let placed = self.place(&paths, &root, &[])?;
            for (id, file) in files {
                let family = &families[&id];
                let directory = file.parent().expect("a file is in a folder");
                let place = placed.places.get(directory);
                let Some(place) = place else {
                    report.unplaced.push(file);
                    leave.extend(family);
                    continue;
                };
                if let Place::Folder(root_id, logical) = place
                    && self.db.snapshot(|s| known(s, *root_id, logical, &file))?
                {
                    report.present += family.len();
                    leave.extend(family);
                    continue;
                }
                let folder = photos
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.folder)
                    .context("A photo left the catalog")?;
                let mut directory = directory.to_string_lossy().into_owned();
                directory.push(std::path::MAIN_SEPARATOR);
                // Photos are copied folder by folder, so a folder's photos
                // all go to the first one's place; one found elsewhere (by
                // a link) is left out.
                let target = targets.entry(folder).or_insert(Target {
                    place: place.clone(),
                    root: root.clone(),
                    directory: directory.clone(),
                });
                if target.directory != directory {
                    report.unplaced.push(file);
                    leave.extend(family);
                }
            }
        }
        copy.db.write(|w| {
            for id in &leave {
                super::copies::delete_photo(w, *id)?;
            }
            super::export_catalog::prune_unreferenced(w)?;
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

/// The folder on disk the root of `folder` is: its path without the names
/// of its logical path.
fn root_of(folder: &super::Folder) -> Option<PathBuf> {
    let depth = super::locations::names(&folder.relative)
        .filter(|n| !n.is_empty())
        .count();
    let root = folder.path.ancestors().nth(depth)?;
    root.canonicalize().ok()
}

/// Copies every photo of the attached catalog `src`, and what is about
/// them, into this one, the folders going to `targets`. Returns how many
/// photos were added.
fn copy_rows(w: &mut LightroomWrite<'_>, targets: &HashMap<FolderId, Target>) -> Result<usize> {
    let count: i64 = w.read_sqlite::<i64>(sqlite_sql!("SELECT count(*) FROM src.photos"), &[])?[0];
    if count == 0 {
        return Ok(0);
    }
    let highest: i64 = w.read_sqlite::<i64>(sqlite_sql!("SELECT max(id) FROM src.photos"), &[])?[0];
    let photo = super::photo_ids::reserve_block(w.write(), highest)?;
    let batch = new_batch(w.write(), ImportKind::Add)?;
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
                     label, format, copy_name, master_id, orientation, lightroom_develop, recipe,
                     export_options, identity, edited_at)
                 SELECT id+?1, ?2, filename, ?3 || filename, captured, rating, flag, label,
                     format, copy_name, master_id+?1, orientation, lightroom_develop, recipe,
                     export_options, identity, edited_at
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
    // Each row of the photos added, with the ids it names shifted.
    for statement in [
        sqlite_sql!(
            "INSERT INTO photo_uuids(photo, uuid) SELECT photo+?1, uuid FROM src.photo_uuids"
        ),
        sqlite_sql!(
            "INSERT INTO lightroom_history(photo, position, name, created, text)
             SELECT photo+?1, position, name, created, text FROM src.lightroom_history"
        ),
        sqlite_sql!(
            "INSERT INTO local_edits(photo, data) SELECT photo+?1, data FROM src.local_edits"
        ),
        sqlite_sql!(
            "INSERT INTO develop_history(photo, data)
             SELECT photo+?1, data FROM src.develop_history"
        ),
        sqlite_sql!(
            "INSERT INTO photo_info(photo, camera, lens, focal, aperture, exposure, iso, width,
                 height)
             SELECT photo+?1, camera, lens, focal, aperture, exposure, iso, width, height
             FROM src.photo_info"
        ),
        sqlite_sql!(
            "INSERT INTO photo_checksums(photo, size, quick_hash, full_hash, identity, version)
             SELECT photo+?1, size, quick_hash, full_hash, identity, version
             FROM src.photo_checksums"
        ),
        sqlite_sql!(
            "INSERT INTO photo_sequences(photo, camera, burst, frame, size, modified_ns, version)
             SELECT photo+?1, camera, burst, frame, size, modified_ns, version
             FROM src.photo_sequences"
        ),
        sqlite_sql!(
            "INSERT INTO photo_import_facts(photo, original_name, exif_captured, size, exif_read)
             SELECT photo+?1, original_name, exif_captured, size, exif_read
             FROM src.photo_import_facts"
        ),
        sqlite_sql!(
            "INSERT INTO photo_fields(photo, field, state)
             SELECT photo+?1, field, state FROM src.photo_fields"
        ),
        sqlite_sql!(
            "INSERT INTO photo_text(photo, field, lang, position, value)
             SELECT photo+?1, field, lang, position, value FROM src.photo_text"
        ),
        sqlite_sql!(
            "INSERT INTO photo_creators(photo, position, name)
             SELECT photo+?1, position, name FROM src.photo_creators"
        ),
        sqlite_sql!(
            "INSERT INTO photo_capture(photo, captured, subsec, \"offset\")
             SELECT photo+?1, captured, subsec, \"offset\" FROM src.photo_capture"
        ),
        sqlite_sql!(
            "INSERT INTO photo_location(photo, lat, lon, alt, cleared)
             SELECT photo+?1, lat, lon, alt, cleared FROM src.photo_location"
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
             SELECT id+?2, photo+?1, name, recipe, lightroom FROM src.develop_snapshots"
        ),
        &[&photo, &snapshot],
    )?;
    w.execute_sqlite(
        sqlite_sql!("INSERT INTO stacks(id, collapsed) SELECT id+?1, collapsed FROM src.stacks"),
        &[&stack],
    )?;
    w.execute_sqlite(
        sqlite_sql!(
            "INSERT INTO stack_photos(photo, stack, position)
             SELECT photo+?1, stack+?2, position FROM src.stack_photos"
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
                 SELECT photo+?1, ?2 FROM src.photo_keywords WHERE keyword=?3
                 ON CONFLICT DO NOTHING"
            ),
            &[&photo, &id, &row.id],
        )?;
    }
    Ok(())
}

/// Adds the photos added to the attached catalog's collections, each found
/// by its path and kind, else made with its sort and rules. In a collection
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
    let existing: Vec<Row> = w
        .write()
        .read(sql!("SELECT id, name, parent, kind FROM collections"), &[])?;
    let mut mapped: HashMap<i64, CollectionId> = HashMap::new();
    for row in rows {
        let parent = row.parent.and_then(|p| mapped.get(&p).copied());
        let found = existing
            .iter()
            .find(|c| c.parent == parent.map(|p| p.0) && c.name == row.name && c.kind == row.kind)
            .map(|c| CollectionId(c.id));
        let (id, made) = match found {
            Some(id) => (id, false),
            None => {
                let id = super::collections::insert(w.write(), &row.name, parent, &row.kind)?;
                for statement in [
                    sqlite_sql!(
                        "INSERT INTO collection_options(collection, sort, direction)
                         SELECT ?1, sort, direction FROM src.collection_options
                         WHERE collection=?2"
                    ),
                    sqlite_sql!(
                        "INSERT INTO smart_collection_rules(collection, rules, format, lightroom)
                         SELECT ?1, rules, format, lightroom FROM src.smart_collection_rules
                         WHERE collection=?2"
                    ),
                ] {
                    w.execute_sqlite(statement, &[&id, &row.id])?;
                }
                (id, true)
            }
        };
        mapped.insert(row.id, id);
        w.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO collection_photos(collection, photo, position)
                 SELECT ?1, photo+?2, CASE WHEN ?3 THEN position END
                 FROM src.collection_photos WHERE collection=?4
                 ON CONFLICT DO NOTHING"
            ),
            &[&id, &photo, &made, &row.id],
        )?;
    }
    Ok(())
}
