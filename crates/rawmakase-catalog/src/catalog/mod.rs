//! Independent, versioned SQLite catalogs. Lightroom sources are never opened writable.
//!
//! `Catalog` owns the connection; `schema.sql` owns every table. The catalog's
//! operations are grouped by what they change: browsing queries and relinking
//! here, edits in `edits`, virtual copies in `copies`, adding folders in
//! `ingest`, and everything Lightroom-specific under `lightroom`.
use anyhow::{Result, ensure};
use db::{Db, Reads, Write, sql};
use std::path::Path;
use value::row;

pub struct Catalog {
    location: CatalogLocation,
    db: Db,
    /// The computer it is open on, whose folder locations apply.
    computer: locations::Computer,
}

mod copies;
mod db;
mod defaults;
mod descriptive;
mod develop_history;
mod edit_records;
mod edit_rows;
mod edits;
mod format_upgrade;
mod info;
mod ingest;
pub mod legacy_sidecar;
pub mod lightroom;
mod location;
pub mod locations;
mod mask_assets;
mod models;
// XMP metadata sidecars; `legacy_sidecar` is the old `*.rawmakase.json` edits.
mod sidecar;
mod snapshots;
mod value;
pub use crate::ids::{CollectionId, FolderId, PhotoId, RootId};
pub use crate::xmp::descriptive::Read as FileMetadata;
#[doc(hidden)]
pub use db::assert_shape as assert_sql_shape;
pub use defaults::MetadataDefaults;
pub use descriptive::MetadataSnapshot;
pub use develop_history::{HistoryUpdate, SavedHistory, SavedStep};
pub use edits::{EditChange, EditToSave};
pub use ingest::{Ambiguity, Choice, Conflict};
pub use lightroom::HistoryStep;
pub use location::CatalogLocation;
pub use locations::{Override, Overrides, RootLocations};
pub use models::{Collection, CollectionKind, Folder, Photo, QUICK_COLLECTION};
pub use sidecar::{SidecarReport, read_file as read_file_metadata, sidecars};
pub use snapshots::{Snapshot, SnapshotSettings};
impl Catalog {
    pub fn create(path: &Path) -> Result<Self> {
        ensure!(!path.exists(), "Catalog already exists: {}", path.display());
        let parent = crate::storage::parent_dir(path);
        std::fs::create_dir_all(parent)?;
        let file = tempfile::NamedTempFile::new_in(parent)?;
        Db::create(file.path())?;
        file.persist_noclobber(path)?;
        Self::open(path)
    }
    /// Opens a catalog on this computer.
    pub fn open(location: impl Into<CatalogLocation>) -> Result<Self> {
        Self::open_as(location, &locations::Computer::this())
    }
    /// Opens a catalog on `computer`, whose folder locations apply.
    ///
    /// The format is checked before anything is written, and a catalog this
    /// release can't read is left as it was. An open writes only what is
    /// missing for this computer, and a catalog that can't be readied isn't
    /// opened.
    pub fn open_as(
        location: impl Into<CatalogLocation>,
        computer: &locations::Computer,
    ) -> Result<Self> {
        let location = location.into();
        let mut catalog = Self {
            db: Db::open(&location)?,
            location,
            computer: computer.clone(),
        };
        catalog.prepare()?;
        Ok(catalog)
    }
    /// Where the catalog is, to name it and open it again.
    pub fn location(&self) -> &CatalogLocation {
        &self.location
    }
    /// Readies an open catalog for this computer. Usually there is nothing
    /// to do, and nothing is written.
    fn prepare(&mut self) -> Result<()> {
        // The schema is idempotent: a catalog from an earlier release gains the
        // tables added since.
        self.db.apply_schema()?;
        locations::prepare(&mut self.db, &self.computer)
    }
    pub fn photos(&self) -> Result<Vec<Photo>> {
        row! {
            struct PhotoRow {
                id: PhotoId,
                folder: FolderId,
                filename: String,
                captured: String,
                rating: i32,
                flag: i32,
                label: String,
                format: String,
                copy_name: String,
                master: Option<PhotoId>,
                keywords: String,
                has_lightroom_edits: bool,
            }
        }
        let mappings = self.folders()?;
        let paths: std::collections::HashMap<_, _> =
            mappings.into_iter().map(|f| (f.id, f.path)).collect();
        let rows: Vec<PhotoRow> = self.db.read(
            sql!(
                "SELECT p.id,p.folder,p.filename,p.captured,p.rating,p.flag,p.label,p.format,p.copy_name,p.master_id, COALESCE((SELECT string_agg(k.name, ', ')
                 FROM photo_keywords pk JOIN keywords k ON k.id=pk.keyword WHERE pk.photo=p.id),''),length(COALESCE(p.lightroom_develop,''))>0
                 FROM photos p
                 ORDER BY p.captured,p.filename,p.id"
            ),
            &[],
        )?;
        Ok(rows
            .into_iter()
            .map(|r| Photo {
                id: r.id,
                folder: r.folder,
                // No path where its folder can't be on this computer.
                path: paths
                    .get(&r.folder)
                    .filter(|path| !path.as_os_str().is_empty())
                    .map(|path| path.join(&r.filename))
                    .unwrap_or_default(),
                filename: r.filename,
                captured: r.captured,
                rating: r.rating,
                flag: r.flag,
                label: r.label,
                format: r.format,
                copy_name: r.copy_name,
                master: r.master,
                keywords: r.keywords,
                has_lightroom_edits: r.has_lightroom_edits,
            })
            .collect())
    }
    /// Every folder where it is on this computer (see `locations`); an
    /// empty path where it can't be here.
    pub fn folders(&self) -> Result<Vec<Folder>> {
        row! {
            struct FolderRow {
                id: FolderId,
                root: RootId,
                original: String,
                relative: String,
                logical: Option<String>,
                count: i64,
            }
        }
        let rows = self.location_rows()?;
        let folders: Vec<FolderRow> = self.db.read(
            sql!(
                "SELECT f.id,f.root,r.original_path,f.relative_path,p.path,(SELECT count(*)
                 FROM photos p WHERE p.folder=f.id)
                 FROM folders f JOIN roots r ON r.id=f.root LEFT JOIN folder_paths p ON p.folder=f.id
                 ORDER BY r.original_path,COALESCE(p.path,f.relative_path)"
            ),
            &[],
        )?;
        Ok(folders
            .into_iter()
            .map(|f| {
                // An older release may have added it since this catalog opened.
                let relative = f
                    .logical
                    .unwrap_or_else(|| locations::logical_from_legacy(&f.original, &f.relative));
                let own = rows.get(&f.root).map_or(&[][..], |r| &r[..]);
                let path = locations::resolve_in(&f.original, own, &relative, cfg!(windows))
                    .unwrap_or_default();
                Folder {
                    relative,
                    id: f.id,
                    root: f.root,
                    path,
                    count: f.count as usize,
                }
            })
            .collect())
    }
    pub fn collections(&self) -> Result<Vec<Collection>> {
        row! {
            struct CollectionRow {
                id: CollectionId,
                name: String,
                parent: Option<CollectionId>,
                kind: String,
            }
        }
        let rows: Vec<CollectionRow> = self.db.read(
            sql!("SELECT id, name, parent, kind FROM collections ORDER BY name"),
            &[],
        )?;
        Ok(rows
            .into_iter()
            .map(|r| Collection {
                id: r.id,
                kind: CollectionKind::from_lightroom(&r.kind, &r.name),
                name: r.name,
                parent: r.parent,
            })
            .collect())
    }
    /// Lightroom's Quick Collection: the one imported with the catalog, or a
    /// new one made the same way.
    pub fn quick_collection(&mut self) -> Result<CollectionId> {
        const KIND: &str = "com.adobe.ag.library.collection";
        let found = self.db.read_optional(
            sql!("SELECT id FROM collections WHERE name=?1 AND kind=?2 AND parent IS NULL"),
            &[&models::QUICK_COLLECTION, &KIND],
        )?;
        if let Some(id) = found {
            return Ok(id);
        }
        self.db.write(|w| {
            w.insert_returning_id(
                sql!(
                    "INSERT INTO collections(name, parent, kind) VALUES (?, NULL, ?) RETURNING id"
                ),
                &[&models::QUICK_COLLECTION, &KIND],
            )
        })
    }
    /// Adds `add` to and removes `remove` from a collection, in one transaction.
    pub fn change_collection(
        &mut self,
        collection: CollectionId,
        add: &[PhotoId],
        remove: &[PhotoId],
    ) -> Result<()> {
        self.db.write(|w| {
            for photo in add {
                w.execute(
                    sql!(
                        "INSERT INTO collection_photos(collection, photo) VALUES (?, ?)
                         ON CONFLICT DO NOTHING"
                    ),
                    &[&collection, photo],
                )?;
            }
            for photo in remove {
                w.execute(
                    sql!("DELETE FROM collection_photos WHERE collection=? AND photo=?"),
                    &[&collection, photo],
                )?;
            }
            Ok(())
        })
    }
    /// Every collection's photos, by collection.
    pub fn collection_photos(
        &self,
    ) -> Result<std::collections::HashMap<CollectionId, std::collections::HashSet<PhotoId>>> {
        row! {
            struct Member {
                collection: CollectionId,
                photo: PhotoId,
            }
        }
        let mut members: std::collections::HashMap<_, std::collections::HashSet<_>> =
            Default::default();
        let rows: Vec<Member> = self
            .db
            .read(sql!("SELECT collection, photo FROM collection_photos"), &[])?;
        for row in rows {
            members.entry(row.collection).or_default().insert(row.photo);
        }
        Ok(members)
    }
    #[cfg(test)]
    pub(crate) fn collection_members(
        &self,
        id: CollectionId,
    ) -> Result<std::collections::HashSet<PhotoId>> {
        let members: Vec<PhotoId> = self.db.read(
            sql!("SELECT photo FROM collection_photos WHERE collection=?"),
            &[&id],
        )?;
        Ok(members.into_iter().collect())
    }
    /// Every root: where it was added and this computer's location of it.
    pub fn roots(&self) -> Result<Vec<(RootId, String, Option<String>)>> {
        row! {
            struct RootRow {
                id: RootId,
                original: String,
                location: Option<String>,
            }
        }
        let rows: Vec<RootRow> = self.db.read(
            sql!(
                "SELECT r.id,r.original_path,l.path FROM roots r LEFT JOIN folder_locations l
                 ON l.root=r.id AND l.relative_path='' AND l.computer=? ORDER BY r.id"
            ),
            &[&self.computer.id],
        )?;
        Ok(rows
            .into_iter()
            .map(|r| (r.id, r.original, r.location))
            .collect())
    }
    /// A fact about the catalog itself, from the `meta` table.
    fn meta(&self, key: &str) -> Result<Option<String>> {
        self.db
            .read_optional(sql!("SELECT value FROM meta WHERE key=?"), &[&key])
    }
    fn set_meta(&mut self, key: &str, value: &str) -> Result<()> {
        self.db.write(|w| set_meta(w, key, value))
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_metadata(&mut self, id: PhotoId, rating: i32, flag: i32, label: &str) -> Result<()> {
        self.set_metadata_of(&[(id, rating, flag, label.into())])
    }
    /// Sets rating, flag and label of several photos in one transaction:
    /// all of them are saved, or none.
    pub fn set_metadata_of(&mut self, changes: &[(PhotoId, i32, i32, String)]) -> Result<()> {
        self.db.write(|w| {
            for (id, rating, flag, label) in changes {
                ensure!((0..=5).contains(rating), "Rating must be between 0 and 5");
                ensure!((-1..=1).contains(flag), "Invalid pick/reject flag");
                ensure!(
                    w.execute(
                        sql!("UPDATE photos SET rating=?,flag=?,label=? WHERE id=?"),
                        &[rating, flag, label, id]
                    )? == 1,
                    "Unknown photo"
                );
            }
            Ok(())
        })
    }
}

/// Records a fact about the catalog in its `meta` table.
fn set_meta(w: &mut Write<'_>, key: &str, value: &str) -> Result<()> {
    w.execute(
        sql!(
            "INSERT INTO meta(key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value"
        ),
        &[&key, &value],
    )?;
    Ok(())
}

pub use sidecar::Merge;
#[cfg(test)]
mod boundary_tests;
#[cfg(test)]
mod descriptive_tests;
#[cfg(test)]
mod edit_rows_tests;
#[cfg(test)]
mod format_upgrade_tests;
#[cfg(test)]
mod locations_tests;
#[cfg(test)]
mod mask_assets_tests;
#[cfg(test)]
mod open_tests;
#[cfg(test)]
mod portability_tests;
#[cfg(test)]
mod portable_sql_tests;
pub mod preview_cache;
#[cfg(test)]
mod private_tests;
#[cfg(test)]
mod sql_scan;
#[cfg(test)]
mod tests;
