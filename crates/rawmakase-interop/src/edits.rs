//! The edit a photo is developed with, worked out the same way wherever it is
//! needed: its saved RAWmakase edit, else its Lightroom edit, else the raw
//! defaults. Develop, Sync Settings and Export all start from here.
//!
//! The catalog reads the stored edit (`Catalog::edit_record` in `rawmakase_catalog`)
//! and [`resolve`] works it out after, so a caller can read many photos at once
//! and resolve them later, off the UI thread. Upright's analysis needs the
//! developed photo, so it is a step of its own: `rawmakase_engine::develop::upright::complete`.
use crate::{
    camera_data::Metadata, camera_profiles::CameraProfile, export_settings::ExportOptions,
    model::recipe::Recipe, raw_defaults::DevelopDefaults, storage::Identity,
};
use anyhow::{Context, Result, ensure};
use std::{path::Path, sync::Arc};

/// A saved RAWmakase edit: the recipe, with its spots and masks, and how the
/// photo exports.
#[derive(Debug)]
pub struct SavedEdit {
    pub recipe: Recipe,
    pub export: ExportOptions,
}

/// One photo's edit as the catalog stores it, not yet read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditRecord {
    pub recipe: Option<String>,
    pub export: Option<String>,
    /// The file the recipe was saved for.
    pub identity: Option<String>,
    /// Spots and masks, saved apart from the recipe.
    pub local: Option<String>,
    /// Lightroom's develop settings, from an imported catalog.
    pub lightroom: Option<String>,
}

/// Where a photo's edit came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Saved,
    Lightroom,
    Defaults,
}

/// A photo's edit, worked out.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub recipe: Recipe,
    pub export: ExportOptions,
    pub origin: Origin,
    /// What the edit could not bring along: Lightroom settings not rendered yet, or
    /// why the chosen raw defaults could not be used.
    pub warnings: Vec<String>,
}

/// What exporting one photo needs from the catalog: its edit and the metadata the
/// file carries.
#[derive(Clone, Debug)]
pub struct PhotoRecord {
    pub edit: EditRecord,
    pub descriptive: crate::metadata::Descriptive,
    pub keywords: Vec<crate::metadata::Keyword>,
    pub rating: i32,
    pub label: String,
    /// The capture time the catalog sorts by ("2026-05-04 10:21:33.000"), empty
    /// when unknown.
    pub captured: String,
}

impl EditRecord {
    /// The saved RAWmakase edit, if there is one. An error when it can't be read, or
    /// when the file at `path` is not the one it was saved for: the edit is then
    /// protected, never replaced by another.
    pub fn saved(&self, path: &Path) -> Result<Option<SavedEdit>> {
        let Some(recipe) = &self.recipe else {
            return Ok(None);
        };
        let saved: Identity =
            serde_json::from_str(self.identity.as_deref().context("Missing photo identity")?)?;
        ensure!(
            saved == Identity::read(path)?,
            "Photo changed since this catalog edit was saved; catalog edit protected"
        );
        let recipe: Recipe = serde_json::from_str(recipe)?;
        let recipe = recipe.with_local(local_edits(self.local.as_deref())?);
        recipe.validate()?;
        let export: ExportOptions =
            serde_json::from_str(self.export.as_deref().context("Missing export settings")?)?;
        export.validate()?;
        Ok(Some(SavedEdit { recipe, export }))
    }
    /// Lightroom's develop settings, when the photo has any.
    pub fn lightroom(&self) -> Option<&str> {
        self.lightroom.as_deref().filter(|text| !text.is_empty())
    }
}

/// A Lightroom edit as RAWmakase renders it, converted from Adobe Default as
/// Lightroom stores it, and the settings it can't render yet.
pub fn lightroom_edit(
    text: &str,
    m: &Metadata,
    profiles: &[Arc<CameraProfile>],
) -> Result<(Recipe, Vec<String>)> {
    crate::lr_develop::convert_develop(text, m, profiles, None)
}

/// The edit photo `record` at `path` is developed with: its saved edit, else its
/// Lightroom edit, else `defaults`. An edit that is there but can't be read is an
/// error, never the defaults.
pub fn resolve(
    record: &EditRecord,
    path: &Path,
    m: &Metadata,
    profiles: &[Arc<CameraProfile>],
    defaults: &DevelopDefaults,
) -> Result<Resolved> {
    if let Some(saved) = record.saved(path)? {
        return Ok(Resolved {
            recipe: saved.recipe,
            export: saved.export,
            origin: Origin::Saved,
            warnings: Vec::new(),
        });
    }
    if let Some(text) = record.lightroom() {
        let (recipe, warnings) =
            lightroom_edit(text, m, profiles).context("Its Lightroom edit can't be read")?;
        return Ok(Resolved {
            recipe,
            export: ExportOptions::default(),
            origin: Origin::Lightroom,
            warnings,
        });
    }
    let resolved = defaults.resolve(m, profiles);
    Ok(Resolved {
        recipe: resolved.recipe,
        export: ExportOptions::default(),
        origin: Origin::Defaults,
        warnings: resolved.note.into_iter().collect(),
    })
}

/// Spots and masks from their stored text; none when there is none. The catalog reads
/// them with this too.
pub fn local_edits(text: Option<&str>) -> Result<crate::model::recipe::LocalEdits> {
    let local: crate::model::recipe::LocalEdits = match text {
        Some(d) => serde_json::from_str(d)?,
        None => Default::default(),
    };
    local.validate()?;
    Ok(local)
}
