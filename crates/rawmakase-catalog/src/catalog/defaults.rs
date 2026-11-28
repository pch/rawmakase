//! Default Creator and Copyright from Preferences, written for photos added
//! from folders where neither the file nor its sidecar has one: below an
//! edit or an imported value, above the file's EXIF, and never at export;
//! the location the camera's EXIF gives a photo whose XMP has none, as
//! Lightroom reads it; and Preferences' other choices for photos added
//! (Stack Camera Bursts).
use super::PhotoId;
use super::db::Write;
use crate::metadata::{LangAlt, Value};
use crate::xmp::descriptive::Read;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetadataDefaults {
    pub creator: String,
    pub copyright: String,
}
impl MetadataDefaults {
    fn path() -> PathBuf {
        crate::storage::data_dir().join("metadata-defaults.json")
    }
    pub fn load() -> Self {
        crate::storage::read_json_or_default(&Self::path())
    }
    pub fn save(&self) -> Result<()> {
        crate::storage::atomic_json(&Self::path(), self)
    }
    pub fn is_empty(&self) -> bool {
        self.creator.trim().is_empty() && self.copyright.trim().is_empty()
    }
}

/// What Preferences says to do with photos added from folders (Add Folder,
/// and Synchronize's new files), beside the metadata defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportOptions {
    /// Stack the bursts their cameras numbered or shot in continuous drive,
    /// once their files are read (see `Catalog::pending_burst_stacks`).
    pub stack_bursts: bool,
    /// Choices a later release added, saved back untouched.
    #[serde(flatten)]
    pub later_fields: rawmakase_interop::export_settings::LaterFields,
}
impl ImportOptions {
    fn path() -> PathBuf {
        crate::storage::data_dir().join("import-options.json")
    }
    pub fn load() -> Self {
        crate::storage::read_json_or_default(&Self::path())
    }
    pub fn save(&self) -> Result<()> {
        crate::storage::atomic_json(&Self::path(), self)
    }
}

/// Reads the camera's EXIF of a photo's file. The catalog can't read it
/// from every RAW itself (a CR3's, MRW's or X3F's is found by RAWmakase's
/// RAW decoders), so the app passes its reader in: `photo::camera_exif`.
pub type CameraExifReader = fn(&Path) -> Option<crate::exif::CameraExif>;

/// What photos added from folders get where their files have none: the
/// Preferences' Creator and Copyright, which give way to the camera's own,
/// read by the reader given; and the reader of the XMP a RAW's camera
/// embeds, which the defaults give way to as well. The default is none,
/// which reads nothing. Also the Preferences' choices for what is done with
/// the photos added, taken once when adding starts.
#[derive(Clone, Debug, Default)]
pub struct NewPhotoDefaults {
    metadata: MetadataDefaults,
    camera_exif: Option<CameraExifReader>,
    raw_xmp: Option<super::RawXmpReader>,
    options: ImportOptions,
}
impl NewPhotoDefaults {
    /// `metadata` where `camera_exif` finds no Artist or Copyright of the
    /// camera's; new photos' RAWs' own XMP read by `raw_xmp`.
    pub fn new(
        metadata: MetadataDefaults,
        camera_exif: CameraExifReader,
        raw_xmp: super::RawXmpReader,
    ) -> Self {
        Self {
            metadata,
            camera_exif: Some(camera_exif),
            raw_xmp: Some(raw_xmp),
            options: ImportOptions::default(),
        }
    }
    /// These defaults, with `options` for what is done with the photos added.
    pub fn with_options(self, options: ImportOptions) -> Self {
        Self { options, ..self }
    }
    /// What Preferences said, as an import journals it.
    pub(super) fn snapshot(&self) -> (MetadataDefaults, ImportOptions) {
        (self.metadata.clone(), self.options.clone())
    }
    /// These readers, with the Preferences an import journalled.
    pub(super) fn with_snapshot(&self, metadata: MetadataDefaults, options: ImportOptions) -> Self {
        Self {
            metadata,
            options,
            ..self.clone()
        }
    }
    /// The readers [`install_file_readers`] installed, with no metadata
    /// defaults: what recovery of an import starts from, which journalled
    /// its Preferences but can't journal the readers.
    pub(super) fn installed() -> Self {
        match READERS.get() {
            Some((camera_exif, raw_xmp)) => Self {
                camera_exif: Some(*camera_exif),
                raw_xmp: Some(*raw_xmp),
                ..Self::default()
            },
            None => Self::default(),
        }
    }
    /// Whether the bursts of the photos added are to be stacked.
    pub(super) fn stack_bursts(&self) -> bool {
        self.options.stack_bursts
    }
    /// What reads the XMP a new photo's RAW embeds; nothing by default.
    pub(super) fn raw_xmp(&self) -> super::RawXmpReader {
        self.raw_xmp.unwrap_or(|_| None)
    }
    /// What of the defaults a photo added from `file` gets: each one the
    /// metadata read from its files (`read`) and its camera's EXIF both lack;
    /// and the location its camera's EXIF gives, when its XMP has none.
    pub(super) fn for_file(&self, file: &Path, read: Option<&Read>) -> Fill {
        let defaults = &self.metadata;
        let (creator, copyright) = (defaults.creator.trim(), defaults.copyright.trim());
        let creator = !creator.is_empty() && read.is_none_or(|r| r.creator.is_none());
        let copyright = !copyright.is_empty() && read.is_none_or(|r| r.copyright.is_none());
        let location = read.is_none_or(|r| r.location.is_none());
        let Some(camera_exif) = self
            .camera_exif
            .filter(|_| creator || copyright || location)
        else {
            return Fill::default();
        };
        // Read once for all three.
        let exif = camera_exif(file);
        let has = |tag| {
            exif.as_ref()
                .and_then(|e| e.get(tag).and_then(crate::exif::Field::text))
                .is_some_and(|t| !t.is_empty())
        };
        Fill {
            creator: (creator && !has(crate::exif::tag::ARTIST))
                .then(|| Value::Set(vec![defaults.creator.trim().to_string()])),
            copyright: (copyright && !has(crate::exif::tag::COPYRIGHT))
                .then(|| Value::Set(LangAlt::new(defaults.copyright.trim()))),
            location: exif
                .as_ref()
                .filter(|_| location)
                .and_then(crate::exif::CameraExif::location),
        }
    }
}

/// The readers the app gives every photo added (see [`NewPhotoDefaults::new`]),
/// for what is added when no request carries them: an import recovery
/// finishes after a crash.
static READERS: std::sync::OnceLock<(CameraExifReader, super::RawXmpReader)> =
    std::sync::OnceLock::new();

/// Installs the app's readers of a photo's camera EXIF and a RAW's own XMP,
/// once per process, for imports recovery finishes. Later calls change
/// nothing.
pub fn install_file_readers(camera_exif: CameraExifReader, raw_xmp: super::RawXmpReader) {
    let _ = READERS.set((camera_exif, raw_xmp));
}

/// The defaults one added photo gets, worked out before it is written.
#[derive(Debug, Default)]
pub(super) struct Fill {
    creator: Option<Value<Vec<String>>>,
    copyright: Option<Value<LangAlt>>,
    location: Option<crate::metadata::Location>,
}

/// Writes `fill` for photo `id`, in `w`'s transaction, where it has no
/// value of its own.
pub(super) fn fill(w: &mut Write<'_>, id: PhotoId, fill: &Fill) -> Result<()> {
    if fill.creator.is_none() && fill.copyright.is_none() && fill.location.is_none() {
        return Ok(());
    }
    let mut d = super::descriptive::read(w, id)?;
    if d.creator.is_none() {
        d.creator.clone_from(&fill.creator);
    }
    if d.copyright.is_none() {
        d.copyright.clone_from(&fill.copyright);
    }
    if d.location.is_none() {
        d.location.clone_from(&fill.location);
    }
    super::descriptive::write(w, id, &d)
}

#[cfg(test)]
#[path = "defaults_tests.rs"]
mod tests;
