//! Independent, versioned SQLite catalogs. Lightroom sources are never opened writable.
//!
//! `Catalog` owns the connection; `schema.sql` owns every table. The catalog's
//! operations are grouped by what they change: browsing queries and relinking
//! here, collections in `collections`, edits in `edits`, virtual copies in
//! `copies`, adding folders in `ingest`, and everything Lightroom-specific
//! under `lightroom`.
use anyhow::{Result, ensure};
use db::{Db, Reads, Write, sql};
use std::path::Path;
use value::row;

pub struct Catalog {
    location: CatalogLocation,
    db: Db,
    /// The computer it is open on, whose folder locations apply.
    computer: locations::Computer,
    /// The file object the catalog was when it opened, if that could be
    /// told; file operations check it is still the one at its path.
    file: Option<crate::fsops::FileId>,
    /// Where values read from it resolve their stored profiles and calibrations.
    assets: crate::storage::recipe_assets::SharedSource,
    /// What upgrading it as it opened did.
    upgraded: Option<format_upgrade::Upgraded>,
}

mod checksums;
mod collections;
mod copies;
mod db;
mod defaults;
mod descriptive;
mod develop_history;
mod edit_records;
mod edit_rows;
mod edits;
mod export_catalog;
pub mod file_ops;
mod fingerprint;
pub mod folder_sync;
mod format_upgrade;
mod info;
mod ingest;
pub mod legacy_sidecar;
pub mod lightroom;
mod lightroom_archive;
mod location;
pub mod locations;
mod mask_assets;
mod models;
mod naming;
mod recipe_assets;
mod rule_inputs;
#[cfg(any(test, feature = "test-support"))]
pub mod upgrade_fixtures;
// XMP metadata sidecars; `legacy_sidecar` is the old `*.rawmakase.json` edits.
mod sequences;
mod sidecar;
mod snapshots;
mod stacks;
mod value;
pub use crate::ids::{CollectionId, FolderId, PhotoId, RootId, StackId};
pub use crate::xmp::descriptive::Read as FileMetadata;
#[doc(hidden)]
pub use checksums::ReadChecksum;
pub use collections::{
    CollectionOptions, Member, MembershipChange, Position, RemovedCollections, SmartRules,
};
pub use db::assert_shape as assert_sql_shape;
pub use defaults::{CameraExifReader, ImportOptions, MetadataDefaults, NewPhotoDefaults};
pub use descriptive::MetadataSnapshot;
pub use develop_history::{HistoryUpdate, SavedHistory, SavedStep};
pub use edits::PreviewEdit;
pub use edits::{EditChange, EditToSave};
pub use export_catalog::{CatalogExport, ExportScope, ExportedFolders};
pub use fingerprint::Conditional;
pub use format_upgrade::{OpeningProgress, Upgraded};
pub use ingest::{
    Ambiguity, Choice, Conflict, FileRefusal, FilesAdded, IMPORT_BURST_SECONDS, PendingBurstStacks,
};
pub use lightroom::HistoryStep;
pub use location::CatalogLocation;
pub use locations::{Override, Overrides, RootLocations};
pub use models::{Collection, CollectionKind, Folder, Photo, QUICK_COLLECTION};
pub use rule_inputs::{KeywordNode, RuleTexts};
pub use sequences::SavedSequence;
pub use sidecar::{RawXmpReader, SidecarReport, read_file as read_file_metadata, sidecars};
pub use snapshots::{Snapshot, SnapshotSettings};
pub use stacks::{
    BurstCandidate, SavedStack, StackChange, StackPlace, StackState, StacksChanged, auto_stacks,
    auto_stacks_with_bursts,
};
impl Catalog {
    pub fn create(path: &Path) -> Result<Self> {
        ensure!(!path.exists(), "Catalog already exists: {}", path.display());
        let parent = crate::storage::parent_dir(path);
        std::fs::create_dir_all(parent)?;
        let file = crate::storage::staging_file(parent)?;
        Db::create(file.path())?;
        crate::storage::persist_noclobber(file, path)?;
        Self::open(path)
    }
    /// Opens a catalog on this computer.
    pub fn open(location: impl Into<CatalogLocation>) -> Result<Self> {
        Self::open_as(location, &locations::Computer::this())
    }
    /// Opens a catalog on `computer`, whose folder locations apply.
    ///
    /// The format is checked before anything is written, and a catalog this
    /// release can't read is left as it was. A catalog of an earlier format is
    /// upgraded first, after a verified backup beside it
    /// ([`Catalog::upgraded`]); when that fails, it is left unchanged and not
    /// opened. An open writes only what is missing for this computer, and a
    /// catalog that can't be readied isn't opened.
    pub fn open_as(
        location: impl Into<CatalogLocation>,
        computer: &locations::Computer,
    ) -> Result<Self> {
        Self::open_reporting(location, computer, &mut |_| {})
    }
    /// [`Catalog::open_as`], telling `progress` how far an upgrade has got.
    pub fn open_reporting(
        location: impl Into<CatalogLocation>,
        computer: &locations::Computer,
        progress: &mut dyn FnMut(OpeningProgress),
    ) -> Result<Self> {
        let location = location.into();
        let (db, file) = file_ops::opening(&location, || Db::open(&location));
        let mut catalog = Self {
            db: db?,
            assets: recipe_assets::shared_source(&location),
            location,
            computer: computer.clone(),
            file,
            upgraded: None,
        };
        catalog.upgrade_on_open(progress)?;
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
                has_lightroom_edits: bool,
            }
        }
        row! {
            struct KeywordRow {
                photo: PhotoId,
                name: String,
            }
        }
        let mappings = self.folders()?;
        let stacks = self.stack_places()?;
        let paths: std::collections::HashMap<_, _> =
            mappings.into_iter().map(|f| (f.id, f.path)).collect();
        let rows: Vec<PhotoRow> = self.db.read(
            sql!(
                "SELECT p.id,p.folder,p.filename,p.captured,p.rating,p.flag,p.label,p.format,p.copy_name,p.master_id,length(COALESCE(p.lightroom_develop,''))>0
                 FROM photos p
                 ORDER BY p.captured,p.filename,p.id"
            ),
            &[],
        )?;
        let mut keywords: std::collections::HashMap<PhotoId, Vec<String>> =
            std::collections::HashMap::new();
        let names: Vec<KeywordRow> = self.db.read(
            sql!(
                "SELECT pk.photo, k.name FROM photo_keywords pk JOIN keywords k ON k.id=pk.keyword
                 WHERE k.name<>'' ORDER BY pk.photo, k.name, k.id"
            ),
            &[],
        )?;
        for k in names {
            keywords.entry(k.photo).or_default().push(k.name);
        }
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
                stack: stacks.get(&r.id).copied(),
                keywords: keywords.remove(&r.id).unwrap_or_default(),
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
mod checksums_tests;
#[cfg(test)]
mod collections_tests;
#[cfg(test)]
mod descriptive_tests;
#[cfg(test)]
mod edit_rows_tests;
#[cfg(test)]
mod export_catalog_tests;
#[cfg(test)]
mod fingerprint_tests;
#[cfg(test)]
mod format_upgrade_tests;
#[cfg(test)]
mod import_bursts_tests;
#[cfg(test)]
mod ingest_tests;
#[cfg(test)]
mod lightroom_archive_tests;
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
mod sequences_tests;
#[cfg(test)]
mod smart_rules_tests;
#[cfg(test)]
mod sql_scan;
#[cfg(test)]
mod stacks_tests;
#[cfg(test)]
mod tests;
