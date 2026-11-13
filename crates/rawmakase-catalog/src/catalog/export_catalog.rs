//! Export as Catalog (issue #738): a new catalog holding only some of this
//! one's photos, with everything the catalog holds about them, that opens on
//! its own; and removing an exported folder from this catalog only while it
//! is still what was exported.
//!
//! The export is a consistent copy of the whole catalog (SQLite's own), with
//! everything about other photos then deleted from it: every column, value
//! and format version of the photos kept stays exactly as stored, including
//! what a later release added to a value this one does not read. The copy is
//! made, pruned and checked in a folder beside the destination, then
//! published under its name without replacing anything there.
//!
//! The photos stay where they are: the export refers to them as this catalog
//! does, by the same roots and folders, so it finds them on this computer
//! and on the others this catalog knows.
use super::db::{Reads, Write, sql};
use super::folder_sync::digest::{self, FamilyDigest};
use super::value::row;
use super::{Catalog, FolderId, PhotoId};
use anyhow::{Context, Result, bail, ensure};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The kind of an ordinary collection, as Lightroom names it.
const COLLECTION_KIND: &str = "com.adobe.ag.library.collection";

/// The photos to export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportScope {
    /// The photos of these folders (a folder and its subfolders, as the
    /// Folders panel chose them), offline ones too.
    Folders(Vec<FolderId>),
    /// These photos.
    Photos(Vec<PhotoId>),
}

/// What an export wrote, and what it could not carry.
#[derive(Clone, Debug)]
pub struct CatalogExport {
    /// The catalog written.
    pub path: PathBuf,
    /// Photos in it, virtual copies included.
    pub photos: usize,
    /// Of those, the masters and virtual copies of the photos chosen that
    /// were not chosen themselves: a copy never goes without its master.
    pub family: usize,
    /// The collection of every exported photo the export was given.
    pub collection: String,
    /// Photos whose file is not where the catalog has it on this computer:
    /// exported, but nothing of the file itself was checked.
    pub offline: Vec<PathBuf>,
    /// Lens profiles the edits name, which render only on a computer that
    /// imported them (they live in RAWmakase's data folder, not a catalog).
    pub lens_profiles: Vec<String>,
    /// Some History could not be read, so every mask raster of the catalog
    /// was kept rather than one it still needs left out.
    pub kept_every_mask: bool,
    /// For a folder exported, what to check before removing it from this
    /// catalog.
    pub removal: Option<ExportedFolders>,
}

/// Folders as they were exported: removing them from the catalog is refused
/// once their photos or anything about them changed, or they were moved or
/// found elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportedFolders {
    folders: Vec<FolderId>,
    /// The photos removing the folders removes, sorted.
    photos: Vec<PhotoId>,
    /// Each of their families', by master: what the user made of it and
    /// where it is.
    families: BTreeMap<PhotoId, (FamilyDigest, FamilyDigest)>,
    /// Where the folders are, on every computer the catalog knows.
    placement: Vec<String>,
}
impl ExportedFolders {
    /// The folders exported.
    pub fn folders(&self) -> &[FolderId] {
        &self.folders
    }
    /// The folders as `db` has them now.
    fn read(db: &impl Reads, folders: &[FolderId]) -> Result<Self> {
        let photos = super::ingest::folder_rows(db, folders)?;
        let mut families = BTreeMap::new();
        for &photo in &photos {
            let master = master_of(db, photo)?;
            if families.contains_key(&master) {
                continue;
            }
            let user = digest::user_state(db, master)?.context("A photo left the catalog")?;
            let placed = digest::placement(db, master)?.context("A photo left the catalog")?;
            families.insert(master, (user, placed));
        }
        Ok(Self {
            folders: folders.to_vec(),
            photos,
            families,
            placement: super::locations::placement_of(db, folders)?,
        })
    }
}

impl Catalog {
    /// Writes a new catalog at `destination` holding the photos of `scope`,
    /// with the masters and virtual copies they have, and everything the
    /// catalog holds about them: edits, History, snapshots, mask rasters,
    /// metadata, keywords, collections, stacks and where their folders are.
    /// Nothing of other photos goes with them. Nothing is written at
    /// `destination` unless the whole catalog is, and this catalog is not
    /// changed.
    pub fn export_catalog(&self, scope: &ExportScope, destination: &Path) -> Result<CatalogExport> {
        ensure!(
            !destination.exists(),
            "A file already exists there; choose another name"
        );
        let parent = crate::storage::parent_dir(destination);
        std::fs::create_dir_all(parent)?;
        let staging = tempfile::tempdir_in(parent)?;
        let working = staging.path().join("export.rawmakase");
        self.db.copy_to(&working)?;
        let collection = destination
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Exported".into());
        let mut copy = Catalog::open_as(working.as_path(), &self.computer)?;
        let mut export = copy.keep_only(scope, &collection)?;
        let kept: i64 = copy.db.read_one(sql!("SELECT count(*) FROM photos"), &[])?;
        drop(copy);
        // Opened again as anyone would open it, on this computer.
        let opened = Catalog::open_as(working.as_path(), &self.computer)?;
        ensure!(
            opened.photos()?.len() as i64 == kept,
            "The exported catalog does not list every photo exported"
        );
        drop(opened);
        crate::storage::rename_noclobber(&working, destination)
            .context("Could not save the exported catalog under its name")?;
        export.path = destination.to_path_buf();
        Ok(export)
    }

    /// Removes folders exported as [`CatalogExport::removal`] from the
    /// catalog, as [`Catalog::remove_folders`] does, unless a photo of them
    /// came, went, moved or changed since: the export would then not hold
    /// what is removed. Returns the photos removed.
    pub fn remove_exported_folders(&mut self, exported: &ExportedFolders) -> Result<Vec<PhotoId>> {
        self.db.write(|w| {
            ensure!(
                ExportedFolders::read(w, &exported.folders)? == *exported,
                "The folder changed since it was exported; export it again to remove it"
            );
            super::ingest::remove_folders(w, &exported.folders)
        })
    }

    /// Reduces this catalog, a copy just made, to the photos of `scope`, their
    /// families and what refers to them, adding collection `collection` of
    /// them all.
    fn keep_only(&mut self, scope: &ExportScope, collection: &str) -> Result<CatalogExport> {
        self.ensure_known_tables()?;
        ensure!(
            !super::file_ops::any_unresolved(&self.db)?,
            "A file operation is unfinished; finish it before exporting"
        );
        // Read before anything below changes the copy, so it is this
        // catalog's as it was copied.
        let removal = match scope {
            ExportScope::Folders(folders) => Some(ExportedFolders::read(&self.db, folders)?),
            ExportScope::Photos(_) => None,
        };
        let chosen = self.chosen(scope)?;
        ensure!(!chosen.is_empty(), "There are no photos to export");
        let keep = self.families(&chosen)?;
        // Lightroom data still only in the archived Lightroom catalog goes
        // with the photos as rows before the archive, which holds every
        // other photo too, is left out.
        self.backfill_keyword_export()?;
        self.backfill_lightroom_metadata()?;
        self.backfill_lightroom_info()?;
        self.backfill_lightroom_snapshots()?;
        self.backfill_lightroom_history()?;

        let photos = self.photos()?;
        let offline = photos
            .iter()
            .filter(|p| keep.contains(&p.id) && p.master.is_none())
            .filter(|p| p.path.as_os_str().is_empty() || !p.path.exists())
            .map(|p| p.path.clone())
            .collect();
        let mut folders: BTreeSet<FolderId> = photos
            .iter()
            .filter(|p| keep.contains(&p.id))
            .map(|p| p.folder)
            .collect();
        if let ExportScope::Folders(chosen) = scope {
            folders.extend(chosen);
        }
        // The folders above them stay too: a location of one of those is
        // where they are found.
        let folders = super::locations::with_ancestors(&self.db, &folders)?;
        let others: Vec<FolderId> = self
            .folders()?
            .into_iter()
            .map(|f| f.id)
            .filter(|f| !folders.contains(f))
            .collect();
        let lens_profiles = self.lens_profiles(&keep)?;

        let kept_every_mask = self.db.write(|w| {
            let all: Vec<PhotoId> = w.read(sql!("SELECT id FROM photos"), &[])?;
            for id in all.into_iter().filter(|id| !keep.contains(id)) {
                super::copies::delete_photo(w, id)?;
            }
            super::ingest::remove_folders(w, &others)?;
            super::locations::forget_unused(w)?;
            prune_unreferenced(w)?;
            let kept_every_mask = prune_bitmaps(w)?;
            prune_recipe_assets(w)?;
            keep_reread(w, &keep)?;
            add_collection(w, collection)?;
            Ok(kept_every_mask)
        })?;
        self.db.compact()?;
        // Each photo is found where this catalog finds it.
        let before: BTreeMap<PhotoId, PathBuf> = photos
            .into_iter()
            .filter(|p| keep.contains(&p.id))
            .map(|p| (p.id, p.path))
            .collect();
        let after: BTreeMap<PhotoId, PathBuf> =
            self.photos()?.into_iter().map(|p| (p.id, p.path)).collect();
        ensure!(
            before == after,
            "The exported catalog would not find its photos where this one does"
        );
        Ok(CatalogExport {
            path: PathBuf::new(),
            photos: keep.len(),
            family: keep.len() - chosen.len(),
            collection: collection.to_string(),
            offline,
            lens_profiles,
            kept_every_mask,
            removal,
        })
    }

    /// Refuses a catalog with a table this release does not know: a later
    /// release added it, and whether its rows are about photos left out, and
    /// must go, can't be told here.
    fn ensure_known_tables(&self) -> Result<()> {
        let known = known_tables();
        let unknown: Vec<String> = self
            .db
            .table_names()?
            .into_iter()
            .filter(|t| !known.contains(t.as_str()))
            .collect();
        if !unknown.is_empty() {
            bail!(
                "A later release of RAWmakase added to this catalog ({}); export it with that release",
                unknown.join(", ")
            );
        }
        Ok(())
    }

    /// The photos `scope` chose, each one checked to be in the catalog.
    fn chosen(&self, scope: &ExportScope) -> Result<BTreeSet<PhotoId>> {
        Ok(match scope {
            ExportScope::Folders(folders) => {
                for folder in folders {
                    let found: Option<i64> = self
                        .db
                        .read_optional(sql!("SELECT 1 FROM folders WHERE id=?"), &[folder])?;
                    ensure!(
                        found.is_some(),
                        "A folder chosen is no longer in the catalog"
                    );
                }
                super::ingest::folder_rows(&self.db, folders)?
                    .into_iter()
                    .collect()
            }
            ExportScope::Photos(photos) => {
                for photo in photos {
                    let found: Option<i64> = self
                        .db
                        .read_optional(sql!("SELECT 1 FROM photos WHERE id=?"), &[photo])?;
                    ensure!(
                        found.is_some(),
                        "A photo chosen is no longer in the catalog"
                    );
                }
                photos.iter().copied().collect()
            }
        })
    }

    /// `chosen` with the master and every virtual copy of each.
    fn families(&self, chosen: &BTreeSet<PhotoId>) -> Result<BTreeSet<PhotoId>> {
        let mut keep = BTreeSet::new();
        for &photo in chosen {
            let master = master_of(&self.db, photo)?;
            let members: Vec<PhotoId> = self.db.read(
                sql!("SELECT id FROM photos WHERE id=? OR master_id=?"),
                &[&master, &master],
            )?;
            keep.extend(members);
        }
        Ok(keep)
    }

    /// The lens profiles the saved edits of `photos` name, sorted: an edit
    /// with profile corrections on and a profile chosen that is not the one
    /// the camera stored.
    fn lens_profiles(&self, photos: &BTreeSet<PhotoId>) -> Result<Vec<String>> {
        row! {
            struct Saved {
                id: PhotoId,
                recipe: String,
            }
        }
        let rows: Vec<Saved> = self.db.read(
            sql!("SELECT id, recipe FROM photos WHERE recipe IS NOT NULL"),
            &[],
        )?;
        let mut names = BTreeSet::new();
        for saved in rows.iter().filter(|s| photos.contains(&s.id)) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&saved.recipe) else {
                continue;
            };
            if value.get("lens_profile").and_then(|v| v.as_bool()) != Some(true) {
                continue;
            }
            let Some(id) = value.pointer("/lens_profile_choice/id") else {
                continue;
            };
            // The camera's own profile, and a Lensfun lens, whose calibration the
            // edit carries, need nothing imported.
            if id.get("embedded").and_then(|v| v.as_bool()) == Some(true)
                || id.get("lensfun").is_some()
            {
                continue;
            }
            let text = |key: &str| id.get(key).and_then(|v| v.as_str()).unwrap_or_default();
            let label = if text("name").is_empty() {
                text("filename")
            } else {
                text("name")
            };
            if !label.is_empty() {
                names.insert(label.to_string());
            }
        }
        Ok(names.into_iter().collect())
    }
}

/// The master of `photo`: its own, or itself.
fn master_of(db: &impl Reads, photo: PhotoId) -> Result<PhotoId> {
    let master: Option<Option<PhotoId>> =
        db.read_optional(sql!("SELECT master_id FROM photos WHERE id=?"), &[&photo])?;
    Ok(master.context("A photo left the catalog")?.unwrap_or(photo))
}

/// The tables `schema.sql` makes.
fn known_tables() -> BTreeSet<&'static str> {
    include_str!("schema.sql")
        .lines()
        .filter_map(|l| l.trim().strip_prefix("CREATE TABLE IF NOT EXISTS "))
        .filter_map(|rest| rest.split([' ', '(']).next())
        .collect()
}

/// Deletes what no photo kept refers to: keywords (but a used one's parents),
/// collections (but a used one's sets), stacks, import batches (and every
/// batch's bursts still to stack), the Lightroom
/// catalog it was imported from (every other photo's too), and the journal of
/// file operations, all finished.
fn prune_unreferenced(w: &mut Write<'_>) -> Result<()> {
    for statement in [
        sql!(
            "WITH RECURSIVE used(id) AS (
                 SELECT keyword FROM photo_keywords
                 UNION SELECT k.parent FROM keywords k JOIN used u ON k.id=u.id
                 WHERE k.parent IS NOT NULL)
             DELETE FROM keyword_export WHERE keyword NOT IN (SELECT id FROM used)"
        ),
        sql!(
            "WITH RECURSIVE used(id) AS (
                 SELECT keyword FROM photo_keywords
                 UNION SELECT k.parent FROM keywords k JOIN used u ON k.id=u.id
                 WHERE k.parent IS NOT NULL)
             DELETE FROM keywords WHERE id NOT IN (SELECT id FROM used)"
        ),
        sql!(
            "WITH RECURSIVE used(id) AS (
                 SELECT collection FROM collection_photos
                 UNION SELECT c.parent FROM collections c JOIN used u ON c.id=u.id
                 WHERE c.parent IS NOT NULL)
             DELETE FROM collections WHERE id NOT IN (SELECT id FROM used)"
        ),
        sql!("DELETE FROM stacks WHERE id NOT IN (SELECT stack FROM stack_photos)"),
        // Bursts still to stack are this catalog's to stack, not the copy's.
        sql!("DELETE FROM import_burst_stacks"),
        sql!("DELETE FROM import_batches WHERE id NOT IN (SELECT batch FROM photo_imports)"),
        sql!("DELETE FROM sources"),
        sql!("DELETE FROM file_operation_copies"),
        sql!("DELETE FROM file_operation_members"),
        sql!("DELETE FROM file_operations"),
    ] {
        w.execute(statement, &[])?;
    }
    Ok(())
}

/// Every stored text that may name a mask raster, profile or calibration: edits,
/// Lightroom settings, spots and masks, snapshots and inflated Histories, whatever
/// release wrote them. `None` when a History can't be read.
fn naming_texts(w: &Write<'_>) -> Result<Option<Vec<String>>> {
    let mut texts: Vec<String> = Vec::new();
    for statement in [
        sql!("SELECT recipe FROM photos WHERE recipe IS NOT NULL"),
        sql!("SELECT lightroom_develop FROM photos WHERE lightroom_develop IS NOT NULL"),
        sql!("SELECT data FROM local_edits"),
        sql!("SELECT recipe FROM develop_snapshots WHERE recipe IS NOT NULL"),
    ] {
        texts.extend(w.read::<String>(statement, &[])?);
    }
    let histories: Vec<Vec<u8>> = w.read(sql!("SELECT data FROM develop_history"), &[])?;
    for data in histories {
        match super::develop_history::text(&data) {
            Ok(text) => texts.push(String::from_utf8_lossy(&text).into_owned()),
            Err(_) => return Ok(None),
        }
    }
    Ok(Some(texts))
}

/// Deletes the mask rasters no edit, History or snapshot left names. A
/// raster is kept if its id appears anywhere in them, whatever release wrote
/// them; if a History can't be read, every raster is kept. Returns whether
/// that happened.
fn prune_bitmaps(w: &mut Write<'_>) -> Result<bool> {
    let hashes: Vec<String> = w.read(sql!("SELECT hash FROM bitmaps"), &[])?;
    if hashes.is_empty() {
        return Ok(false);
    }
    let Some(texts) = naming_texts(w)? else {
        return Ok(true);
    };
    for hash in hashes {
        if !texts.iter().any(|t| t.contains(hash.as_str())) {
            w.execute(sql!("DELETE FROM bitmaps WHERE hash=?"), &[&hash])?;
        }
    }
    Ok(false)
}

/// Deletes the camera profiles and Lensfun calibrations no edit, History or
/// snapshot left names, by the rule [`prune_bitmaps`] follows: one is kept if its id
/// appears anywhere in them, and every one is kept if a History can't be read.
fn prune_recipe_assets(w: &mut Write<'_>) -> Result<()> {
    let ids: Vec<String> = w.read(sql!("SELECT id FROM recipe_assets"), &[])?;
    if ids.is_empty() {
        return Ok(());
    }
    let Some(texts) = naming_texts(w)? else {
        return Ok(());
    };
    for id in ids {
        if !texts.iter().any(|t| t.contains(id.as_str())) {
            w.execute(sql!("DELETE FROM recipe_assets WHERE id=?"), &[&id])?;
        }
    }
    Ok(())
}

/// Leaves only photos kept in the PNGs whose info is still to be read again.
fn keep_reread(w: &mut Write<'_>, keep: &BTreeSet<PhotoId>) -> Result<()> {
    let key = super::info::PNG_INFO_REREAD;
    let Some(ids): Option<String> =
        w.read_optional(sql!("SELECT value FROM meta WHERE key=?"), &[&key])?
    else {
        return Ok(());
    };
    let kept: Vec<&str> = ids
        .split(',')
        .filter(|id| id.parse().is_ok_and(|id| keep.contains(&PhotoId(id))))
        .collect();
    super::set_meta(w, key, &kept.join(","))
}

/// Adds collection `name` of every photo.
fn add_collection(w: &mut Write<'_>, name: &str) -> Result<()> {
    let id: i64 = w.insert_returning_id(
        sql!("INSERT INTO collections(name, parent, kind) VALUES (?, NULL, ?) RETURNING id"),
        &[&name, &COLLECTION_KIND],
    )?;
    w.execute(
        sql!(
            "INSERT INTO collection_photos(collection, photo)
             SELECT c.id, p.id FROM collections c, photos p WHERE c.id=?"
        ),
        &[&id],
    )?;
    Ok(())
}
