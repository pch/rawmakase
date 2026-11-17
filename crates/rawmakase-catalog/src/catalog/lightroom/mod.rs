//! What a catalog keeps of the Lightroom catalogs it was imported from: the
//! source's path, and Lightroom's History steps. The import itself is the
//! bundled Lightroom extension's, through [`plan_import`](super::plan_import);
//! Lightroom's develop settings are converted by `lr_develop`.
pub(super) mod history;
use super::Catalog;
use crate::catalog::db::{Reads, sql};
use anyhow::Result;
pub use history::HistoryStep;
use std::path::PathBuf;

impl Catalog {
    /// The Lightroom catalogs this catalog was imported from, by the paths
    /// they were imported from.
    pub fn lightroom_sources(&self) -> Result<Vec<PathBuf>> {
        let paths: Vec<String> = self.db.read(sql!("SELECT path FROM sources"), &[])?;
        Ok(paths.into_iter().map(PathBuf::from).collect())
    }
}
