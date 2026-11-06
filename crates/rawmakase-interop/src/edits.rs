//! The edit a photo is developed with, worked out the same way wherever it is
//! needed: its saved RAWmakase edit, else its Lightroom edit, else the raw
//! defaults. Develop, Library previews, Sync Settings and Export all start from
//! here.
//!
//! The catalog reads the stored edit (`Catalog::edit_record` in `rawmakase_catalog`)
//! and [`resolve`] works it out after, so a caller can read many photos at once
//! and resolve them later, off the UI thread. A caller that chooses the source at
//! one moment and develops it at another (Develop installs a saved edit when the
//! photo's header arrives and converts a Lightroom edit once its camera profiles
//! are known; Library previews render the source their cache entry is tagged with)
//! holds a [`Start`]: the chosen source, read once, resolved when the metadata and
//! profiles are known. Each caller keeps its own policy for what an error or a
//! warning does. Upright's analysis needs the developed photo, so it is a step of
//! its own: `rawmakase_engine::develop::upright::complete`.
use crate::{
    camera_data::Metadata, camera_profiles::CameraProfile, export_settings::ExportOptions,
    model::recipe::Recipe, raw_defaults::DevelopDefaults, storage::Identity,
};
use anyhow::{Context, Result, ensure};
use std::{path::Path, sync::Arc};

/// A saved RAWmakase edit: the recipe, with its spots and masks, and how the
/// photo exports.
#[derive(Clone, Debug)]
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
    /// when the file at `path` is not the photo it was saved for
    /// ([`Identity::same_photo`]): the edit is then protected, never replaced by
    /// another.
    pub fn saved(&self, path: &Path) -> Result<Option<SavedEdit>> {
        let Some(recipe) = &self.recipe else {
            return Ok(None);
        };
        let saved: Identity =
            serde_json::from_str(self.identity.as_deref().context("Missing photo identity")?)?;
        ensure!(
            saved.same_photo(&Identity::read(path)?),
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
        return Ok(of_saved(saved));
    }
    if let Some(text) = record.lightroom() {
        return of_lightroom(text, m, profiles).context("Its Lightroom edit can't be read");
    }
    Ok(of_defaults(defaults, m, profiles))
}

/// Where a photo's starting edit comes from, chosen once and resolved when the
/// photo's metadata and camera profiles are known. The source's text and the raw
/// defaults' presets are read when it is chosen; [`Start::resolve`] reads no
/// catalog, saved edit or preset file. Settings that name a Lensfun lens still
/// resolve it against the database installed when they are applied, as everywhere.
#[derive(Clone, Debug)]
pub enum Start {
    /// A saved RAWmakase edit, read and checked against its file
    /// ([`EditRecord::saved`]).
    Saved(Box<SavedEdit>),
    /// Lightroom's develop settings, as the catalog stores them.
    Lightroom(String),
    /// No edit: these raw defaults.
    Defaults(Arc<DevelopDefaults>),
    /// A recipe as JSON, read as it is: what a Library preview's cache entry is
    /// tagged with. Unlike [`Start::Saved`] it is not checked against its file.
    RecipeJson(String),
    /// Adobe Default with the photo's profiles (`Recipe::with_profiles`), whatever
    /// the raw defaults are.
    AdobeDefault,
}
impl Start {
    /// Photo `record` at `path`'s source, as [`resolve`] chooses it: its saved edit
    /// (read and checked now; an error when it can't be), else its Lightroom edit,
    /// else `defaults`.
    pub fn capture(
        record: &EditRecord,
        path: &Path,
        defaults: Arc<DevelopDefaults>,
    ) -> Result<Self> {
        if let Some(saved) = record.saved(path)? {
            return Ok(Self::Saved(Box::new(saved)));
        }
        Ok(match record.lightroom() {
            Some(text) => Self::Lightroom(text.to_owned()),
            None => Self::Defaults(defaults),
        })
    }
    /// The edit this source gives a photo with metadata `m` and camera profiles
    /// `profiles`, with what it could not bring along. An error only for a
    /// Lightroom edit or a recipe that can't be read.
    pub fn resolve(&self, m: &Metadata, profiles: &[Arc<CameraProfile>]) -> Result<Resolved> {
        Ok(match self {
            Self::Saved(saved) => of_saved((**saved).clone()),
            Self::Lightroom(text) => of_lightroom(text, m, profiles)?,
            Self::Defaults(defaults) => of_defaults(defaults, m, profiles),
            Self::RecipeJson(json) => Resolved {
                recipe: serde_json::from_str(json)?,
                export: ExportOptions::default(),
                origin: Origin::Saved,
                warnings: Vec::new(),
            },
            Self::AdobeDefault => Resolved {
                recipe: Recipe::with_profiles(m, profiles),
                export: ExportOptions::default(),
                origin: Origin::Defaults,
                warnings: Vec::new(),
            },
        })
    }
}

fn of_saved(saved: SavedEdit) -> Resolved {
    Resolved {
        recipe: saved.recipe,
        export: saved.export,
        origin: Origin::Saved,
        warnings: Vec::new(),
    }
}
fn of_lightroom(text: &str, m: &Metadata, profiles: &[Arc<CameraProfile>]) -> Result<Resolved> {
    let (recipe, warnings) = lightroom_edit(text, m, profiles)?;
    Ok(Resolved {
        recipe,
        export: ExportOptions::default(),
        origin: Origin::Lightroom,
        warnings,
    })
}
fn of_defaults(
    defaults: &DevelopDefaults,
    m: &Metadata,
    profiles: &[Arc<CameraProfile>],
) -> Resolved {
    let resolved = defaults.resolve(m, profiles);
    Resolved {
        recipe: resolved.recipe,
        export: ExportOptions::default(),
        origin: Origin::Defaults,
        warnings: resolved.note.into_iter().collect(),
    }
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

#[cfg(test)]
mod hdr_tests {
    use super::*;
    use crate::camera_data::{HdrSource, Source};
    use crate::color::hdr_output::HdrTransfer;

    fn hdr_file() -> Metadata {
        Metadata {
            source: Source::Rendered,
            hdr_source: Some(HdrSource {
                transfer: HdrTransfer::Pq,
                peak: 6.,
            }),
            ..Default::default()
        }
    }

    /// A new HDR file starts as an HDR edit; a Lightroom edit of one that says
    /// nothing of HDR keeps HDR off, as it read before HDR files opened, and one that
    /// sets the limit alone does not turn HDR on either.
    #[test]
    fn only_a_new_hdr_file_starts_with_hdr_on() -> Result<()> {
        let m = hdr_file();
        let defaults = Recipe::for_new_rendered(&m);
        assert!(defaults.hdr.enabled);
        assert_eq!(defaults.hdr.limit, 3.);
        for text in ["s = { Exposure2012 = 0.5 }", "s = { HDRMaxValue = 3 }"] {
            let (recipe, _) = lightroom_edit(text, &m, &[])?;
            assert!(!recipe.hdr.enabled, "{text}");
        }
        let (on, _) = lightroom_edit("s = { HDREditMode = 1 }", &m, &[])?;
        assert!(on.hdr.enabled);
        Ok(())
    }
}
