//! Upgrading a catalog of an earlier format to the current one: a verified backup
//! beside it, then one transaction that brings the stored rows up to date and
//! raises the version, so releases that cannot read the result refuse it.
//!
//! Version 3 is the one engine's: every stored recipe (edits, History, snapshots)
//! loses the settings earlier releases chose an engine or operator with
//! (`saved_format::OBSOLETE_SETTINGS`), and a catalog is upgraded before this
//! release stores any recipe in it, in the same transaction as that write
//! ([`Catalog::write_recipes`]),
//! so a release that would render those recipes with other operators never opens
//! it. Opening, browsing and rating leave an older catalog as it is.
//!
//! Version 4 may hold Lensfun lens choices. The first write storing one (an edit or
//! any state of its History, a snapshot, a legacy sidecar's edit) raises the catalog
//! to it in the same way, so releases before Lensfun, which would read the choice as
//! an Adobe profile without a name and save without it, refuse it. Catalogs without
//! one stay at version 3.
use super::db::Write;
use super::{Catalog, CatalogLocation};
use anyhow::Result;
use std::path::{Path, PathBuf};

impl Catalog {
    #[cfg(any(test, feature = "test-support"))]
    /// The catalog's format version.
    pub fn format_version(&self) -> Result<i64> {
        self.db.version()
    }
    /// Upgrades a catalog of an earlier format to the current one, after writing
    /// a consistent backup next to it. Returns the backup's path, or `None` when it
    /// already was current. Older releases cannot open it afterwards; it must not
    /// be open in one meanwhile.
    pub fn upgrade_format(&mut self) -> Result<Option<PathBuf>> {
        if self.db.version()? >= super::db::VERSION {
            return Ok(None);
        }
        let CatalogLocation::File(path) = &self.location;
        let backup = backup_path(path)?;
        self.db
            .upgrade(super::db::VERSION, &backup, migrate_recipes, |_| Ok(()))?;
        Ok(Some(backup))
    }
    /// The write that stores a recipe (an edit, its History, a snapshot), checked
    /// beforehand by the caller. A recipe this release saves is for the one engine,
    /// so a catalog of an earlier format is upgraded in the same transaction, after
    /// its backup: when `write` fails, the catalog stays as it was (the backup is
    /// left beside it). `stores` says what the write stores: a Lensfun lens choice
    /// raises the catalog to version 4, an AI Denoise setting to version 5, an HDR edit
    /// to version 6.
    pub(super) fn write_recipes<R>(
        &mut self,
        stores: Stores,
        write: impl FnOnce(&mut Write<'_>) -> Result<R>,
    ) -> Result<R> {
        let target = stores.version();
        if self.db.version()? >= target {
            return self.db.write(write);
        }
        let CatalogLocation::File(path) = &self.location;
        let backup = backup_path(path)?;
        self.db.upgrade(target, &backup, migrate_recipes, write)
    }
}

/// What a write of recipes stores, for the format it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stores {
    Recipes,
    /// A Lensfun lens choice, in use or not, resolved or not.
    Lensfun,
    /// AI Denoise not at its default (on, or its Amount moved), with or without a
    /// Lensfun lens choice.
    AiDenoise,
    /// HDR on, with or without the others.
    Hdr,
}
impl Stores {
    /// For a write of `recipes`.
    pub(super) fn of<'r>(
        recipes: impl IntoIterator<Item = &'r crate::model::recipe::Recipe>,
    ) -> Self {
        recipes
            .into_iter()
            .map(|r| {
                if r.uses_hdr() {
                    Self::Hdr
                } else if !r.ai_denoise.is_default() {
                    Self::AiDenoise
                } else if r.uses_lensfun() {
                    Self::Lensfun
                } else {
                    Self::Recipes
                }
            })
            .max_by_key(|s| s.version())
            .unwrap_or(Self::Recipes)
    }
    pub(super) fn version(self) -> i64 {
        match self {
            Self::Recipes => super::db::VERSION,
            Self::Lensfun => super::db::LENSFUN,
            Self::AiDenoise => super::db::AI_DENOISE,
            Self::Hdr => super::db::HDR,
        }
    }
}

/// A free name for the backup of the catalog at `path`.
fn backup_path(path: &Path) -> Result<PathBuf> {
    let name = path.file_name().map_or_else(
        || "catalog".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    (0..1000)
        .map(|n| {
            let suffix = if n == 0 {
                String::new()
            } else {
                format!("-{n}")
            };
            path.with_file_name(format!("{name}.before-upgrade-backup{suffix}"))
        })
        .find(|candidate| !candidate.exists())
        .ok_or_else(|| anyhow::anyhow!("Too many backups next to the catalog"))
}

/// Removes the obsolete settings from every stored recipe, inside the upgrade.
/// Rows this release cannot read (damaged, or a History from a newer release) are
/// left as they are: an edit that cannot be loaded stays protected.
fn migrate_recipes(w: &mut Write<'_>) -> Result<()> {
    super::edit_rows::migrate_edits(w)?;
    super::snapshots::migrate_snapshots(w)
}
