//! Writing a developed photo to a file, as the desktop's Export does: JPEG,
//! TIFF, PNG, JPEG XL, an HDR master or a gain map, in the color space and
//! size the settings choose. With the `export` feature.
use crate::{Context, Error, Photo, Result};
use rawmakase_cancel::Cancel;
use rawmakase_export::export::{Replace, job};
use rawmakase_interop::export_settings::Format;
use rawmakase_model::model::recipe::Recipe;
use std::path::Path;

/// How a file is exported: its quality, color space, HDR encoding, size,
/// sharpening and metadata, as the desktop's Export dialog chooses them. Its
/// destination folder and file naming are the dialog's, and its format is
/// the file's: the library's [`Context::export`] is given the file to write,
/// and writes the format its extension names.
pub use rawmakase_interop::export_settings::ExportSettings;

impl Context {
    /// Develops `photo` with `recipe` at full quality and writes it to
    /// `target`, encoded beside it first and then put in place, so a failed
    /// export leaves the destination as it was. Returns what the export has to
    /// say about the file, if anything (a JPEG larger than its size limit, say),
    /// and reports progress from 0 to 1.
    ///
    /// The photo is decoded in full, from the context's cache when it holds
    /// the file. A long operation: `cancel` stops it between stages, and a file
    /// is only written by an export that was not cancelled before its render
    /// finished.
    ///
    /// Errors: [`ErrorKind::Exists`](crate::ErrorKind::Exists) when `target`
    /// is there and `replace` is false, before anything is decoded; the
    /// photo's own file and a RAW file are never replaced, and an extension
    /// that names no export format is [`ErrorKind::Invalid`](crate::ErrorKind::Invalid).
    /// [`ErrorKind::Invalid`](crate::ErrorKind::Invalid) for a recipe or
    /// settings that do not validate, or a format that can't hold them;
    /// [`ErrorKind::Unsupported`](crate::ErrorKind::Unsupported) for a recipe
    /// with AI Denoise on, which the library doesn't run yet;
    /// [`ErrorKind::Changed`](crate::ErrorKind::Changed) when the photo's file
    /// is no longer the one opened; [`ErrorKind::Cancelled`](crate::ErrorKind::Cancelled).
    #[allow(
        clippy::too_many_arguments,
        reason = "an export's inputs, each its own"
    )]
    pub fn export(
        &self,
        photo: &Photo,
        recipe: &Recipe,
        settings: &ExportSettings,
        target: impl AsRef<Path>,
        replace: bool,
        cancel: Cancel<'_>,
        progress: impl Fn(f32),
    ) -> Result<Option<String>> {
        let target = target.as_ref();
        let invalid =
            |message: String| Error::of(rawmakase_model::error::Invalid::new(message).into());
        // What the file is named decides its format, and what can be refused
        // without a render is refused first.
        let format = Format::from_path(target).ok_or_else(|| {
            invalid(format!(
                "Export extension must be {}",
                Format::extensions_listed()
            ))
        })?;
        let mut settings = settings.clone();
        settings.choose_format(format);
        let replace = if replace {
            Replace::Overwrite
        } else {
            Replace::NoClobber
        };
        rawmakase_export::export::check_destination(target, photo.path(), replace)
            .map_err(Error::of)?;
        if recipe.as_rendered().ai_denoise.renders() {
            return Err(Error::of(
                rawmakase_decode::ai::denoise::Unsupported(
                    "The library doesn't run AI Denoise yet; switch it off in the recipe",
                )
                .into(),
            ));
        }
        job::check(recipe, &settings, target).map_err(Error::of)?;
        let image = photo.decode_in(self, crate::Decode::Full, false, cancel)?;
        let ctx = self;
        let job = job::Photo {
            image: image.camera().clone(),
            source: photo.path().to_path_buf(),
            recipe: recipe.clone(),
            values: Default::default(),
            watermark: None,
            watermarks: Default::default(),
            demosaic: Default::default(),
            cache: ctx.decoded().cloned(),
            lenses: ctx.lenses().clone(),
            ai: rawmakase_decode::ai::denoise::AiService::refusing(
                "The library doesn't run AI Denoise yet",
            ),
            file: photo.identity().cloned(),
        };
        job::run(job, &settings, target, replace, cancel, progress).map_err(Error::of)
    }
}
