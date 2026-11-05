//! Atomic JPEG, 16-bit TIFF and PNG export with the ICC profile of the pixels' colour
//! space (sRGB, Adobe RGB (1998) or ProPhoto RGB), the camera's EXIF and the edit as
//! Camera Raw XMP, as Lightroom embeds them.
pub mod assemble;
pub mod batch;
mod encode;
pub mod exif;
mod extended_xmp;
pub mod job;
mod metadata;
pub mod queue;
pub use crate::storage::Replace;
use crate::{
    camera_data::Metadata,
    develop::ExportImage,
    export_settings::{ExportOptions, Format},
    rendered::Rendered,
    storage::is_raw,
};
use anyhow::{Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};
use tempfile::NamedTempFile;

/// What an export embeds besides pixels.
#[derive(Clone, Debug)]
pub struct Embed {
    /// The camera's EXIF, read from the RAW; the decoder's capture settings stand in
    /// when it could not be read.
    pub camera: Option<crate::exif::CameraExif>,
    /// Make, model and lens model from the decoder's metadata where the camera's EXIF
    /// has none: the camera's information is included.
    pub camera_fallback: bool,
    /// An XMP packet, e.g. the edit as Camera Raw settings.
    pub xmp: Option<String>,
    /// Pixels per inch recorded in the file.
    pub ppi: u32,
}
impl Default for Embed {
    fn default() -> Self {
        Self {
            camera: None,
            camera_fallback: true,
            xmp: None,
            ppi: 240,
        }
    }
}

pub fn export(
    path: &Path,
    source: &Path,
    image: &Rendered,
    m: &Metadata,
    options: &ExportOptions,
    replace: Replace,
) -> Result<()> {
    export_with(path, source, image, m, options, &Embed::default(), replace)
}

/// Exports `image`, encoded sRGB, to `path`.
pub fn export_with(
    path: &Path,
    source: &Path,
    image: &Rendered,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
    replace: Replace,
) -> Result<()> {
    let image = ExportImage::srgb(image.clone());
    write(path, source, &image, m, options, embed, replace)
}

/// Exports `image` to `path` with the profile of its colour space.
pub fn export_image(
    path: &Path,
    source: &Path,
    image: &ExportImage,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
    replace: Replace,
) -> Result<()> {
    write(path, source, image, m, options, embed, replace)
}

fn write(
    path: &Path,
    source: &Path,
    image: &ExportImage,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
    replace: Replace,
) -> Result<()> {
    ensure!(!is_raw(path), "An export cannot overwrite a RAW file");
    options.validate()?;
    if path.exists() {
        ensure!(
            fs::canonicalize(path)? != fs::canonicalize(source)?,
            "Cannot overwrite source"
        );
        ensure!(replace == Replace::Overwrite, "Destination already exists");
    }
    let staged = stage_image(path, image, m, options, embed)?;
    crate::storage::persist(staged.file, path, replace)
}

/// An export encoded into a temporary file beside its destination. Should the
/// process end before it is put in place, [`remove_unfinished`] deletes it.
pub struct Staged {
    /// [`crate::storage::persist`] puts it in place; dropping it leaves nothing
    /// behind.
    pub file: NamedTempFile,
    /// Known to [`remove_unfinished`] until the export is done with, either way.
    _unfinished: Unfinished,
}

/// Temporary files being written, which a thread cut off at exit would leave behind:
/// dropping a [`NamedTempFile`] removes it, ending the process does not.
struct Writing(Mutex<Vec<PathBuf>>);
impl Writing {
    fn paths(&self) -> std::sync::MutexGuard<'_, Vec<PathBuf>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn track(&'static self, path: &Path) -> Unfinished {
        self.paths().push(path.to_owned());
        Unfinished(self, path.to_owned())
    }
    fn remove_all(&self) {
        for path in self.paths().drain(..) {
            let _ = fs::remove_file(path);
        }
    }
}
/// The temporary files of exports being written.
static WRITING: Writing = Writing(Mutex::new(Vec::new()));

/// A temporary file being written, for as long as this lives.
struct Unfinished(&'static Writing, PathBuf);
impl Drop for Unfinished {
    fn drop(&mut self) {
        let mut paths = self.0.paths();
        if let Some(at) = paths.iter().position(|p| *p == self.1) {
            paths.swap_remove(at);
        }
    }
}

/// Deletes the temporary files of exports still being written, as the process
/// ends without waiting for them. One put in place meanwhile is gone from its
/// temporary path already.
pub fn remove_unfinished() {
    WRITING.remove_all();
}

/// The export of `image`, encoded sRGB, for `path`, encoded into a synced temporary
/// file in `path`'s folder.
pub fn stage(
    path: &Path,
    image: &Rendered,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
) -> Result<Staged> {
    stage_image(path, &ExportImage::srgb(image.clone()), m, options, embed)
}

/// [`stage`] of `image` with its colour space's profile and EXIF colour space; a
/// format that can't hold the space is refused before anything is written.
pub fn stage_image(
    path: &Path,
    image: &ExportImage,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
) -> Result<Staged> {
    let space = image.space();
    ensure!(!is_raw(path), "An export cannot overwrite a RAW file");
    options.validate()?;
    let format = Format::from_path(path).ok_or_else(|| {
        anyhow::anyhow!("Export extension must be {}", Format::extensions_listed())
    })?;
    format.supports(space)?;
    let parent = crate::storage::parent_dir(path);
    let mut temp = NamedTempFile::new_in(parent)?;
    let unfinished = WRITING.track(temp.path());
    let profile = crate::color_management::output_profile(space).to_vec();
    let directories = metadata::directories(m, embed, image.width(), image.height(), space);
    match format {
        Format::Jpeg => encode::jpeg(
            &mut temp,
            image,
            options.quality,
            profile,
            directories,
            embed,
        )?,
        Format::Tiff => encode::tiff(&mut temp, image, profile, &directories, embed)?,
        Format::Png => encode::png(&mut temp, image, profile, directories, embed)?,
    }
    temp.as_file().sync_all()?;
    Ok(Staged {
        file: temp,
        _unfinished: unfinished,
    })
}

#[cfg(test)]
mod srgb_fixtures;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wide_gamut_tests;
