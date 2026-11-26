//! Writing a developed photo to a file, as the desktop's Export does: JPEG,
//! TIFF, PNG, JPEG XL, an HDR master or a gain map, in the color space and
//! size the settings choose. With the `export` feature.
use crate::{Context, Error, Photo, Result};
use rawmakase_cancel::Cancel;
use rawmakase_export::export::{Replace, job};
use rawmakase_interop::export_settings::{Format, Include};
use rawmakase_model::color::OutputSpace;
use rawmakase_model::color::hdr_output::HdrOutput;
use rawmakase_model::model::recipe::Recipe;
use std::path::Path;

/// How a file is encoded: its color space or HDR encoding, quality, size and
/// metadata, as the desktop's Export dialog chooses them. The format is the
/// file's: [`Context::export`] writes the one its target's extension names.
/// What else the dialog chooses (the folder, the file's name, a watermark,
/// what happens after) is the app's own.
///
/// [`Default`] is the dialog's first settings: sRGB, quality 92, full size,
/// all of the metadata in the photo's file and the edit.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSettings {
    /// The color space of the pixels and the embedded profile, for an SDR
    /// export. A format that can't hold it is refused.
    pub color_space: OutputSpace,
    /// An HDR export's encoding, in place of the SDR rendition in
    /// `color_space`: a TIFF, a JPEG XL, or a JPEG with a gain map. The edit
    /// must have HDR on.
    pub hdr: Option<HdrOutput>,
    /// A JPEG's quality, and a lossy JPEG XL's, from 1 to 100.
    pub quality: u8,
    /// A JPEG XL is lossy, at `quality`, rather than lossless.
    pub jxl_lossy: bool,
    /// A JPEG's largest size, in K of 1000 bytes with its metadata, at least 1: the
    /// highest quality whose file fits is chosen in place of `quality`, as
    /// Lightroom's Limit File Size To does, and the smallest file is written,
    /// with a notice, when none fits.
    pub size_limit: Option<u32>,
    /// The long edge, in pixels, the photo is reduced to; never enlarged.
    pub long_edge: Option<u32>,
    /// The resolution written in the file, in pixels per inch: 1 to 10000.
    pub ppi: u32,
    /// Which metadata goes in, as Lightroom's Include chooses it. With
    /// [`Include::Custom`], `capture`, `location` and `develop` choose.
    pub include: Include,
    /// The camera, capture settings, lens and dates (EXIF), with
    /// [`Include::Custom`].
    pub capture: bool,
    /// Where the photo was taken, with [`Include::Custom`] and `capture`.
    pub location: bool,
    /// The edit as Camera Raw settings (XMP), with [`Include::Custom`].
    pub develop: bool,
    /// Lightroom's Remove Location Info, for each choice but
    /// [`Include::Custom`].
    pub remove_location: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        let d = rawmakase_interop::export_settings::ExportSettings::default();
        Self {
            color_space: d.color_space,
            hdr: d.hdr,
            quality: d.quality,
            jxl_lossy: d.jxl_lossy,
            size_limit: d.limit_size.then_some(d.size_limit),
            long_edge: d.resize.then_some(d.long_edge),
            ppi: d.ppi,
            include: d.include,
            capture: d.capture,
            location: d.location,
            develop: d.develop,
            remove_location: d.remove_location,
        }
    }
}

impl ExportSettings {
    /// Refuses a value the desktop's settings would clamp, as invalid.
    fn check(&self) -> Result<()> {
        let limit = rawmakase_interop::export_settings::SIZE_LIMIT_MAX;
        let refused = if !(1..=100).contains(&self.quality) {
            Some("Quality must be 1–100".to_string())
        } else if self.size_limit.is_some_and(|k| !(1..=limit).contains(&k)) {
            Some(format!("A file size limit must be 1–{limit} K"))
        } else if self.long_edge == Some(0) {
            Some("A long edge must be above 0 pixels; None exports the full size".into())
        } else if !(1..=10_000).contains(&self.ppi) {
            Some("The resolution must be 1–10000 pixels per inch".into())
        } else {
            None
        };
        refused.map_or(Ok(()), |message| {
            Err(Error::of(
                rawmakase_model::error::Invalid::new(message).into(),
            ))
        })
    }

    /// The desktop's settings that export a file in `format` as these do.
    fn desktop(&self, format: Format) -> rawmakase_interop::export_settings::ExportSettings {
        let mut s = rawmakase_interop::export_settings::ExportSettings {
            quality: self.quality,
            jxl_lossy: self.jxl_lossy,
            limit_size: self.size_limit.is_some(),
            resize: self.long_edge.is_some(),
            ppi: self.ppi,
            include: self.include,
            capture: self.capture,
            location: self.location,
            develop: self.develop,
            // The ratings, keywords, title and copyright a catalog holds:
            // the library has none, and with Custom the switch would only
            // write an XMP packet the other switches left out.
            descriptive: false,
            remove_location: self.remove_location,
            ..Default::default()
        };
        if let Some(limit) = self.size_limit {
            s.size_limit = limit;
        }
        if let Some(edge) = self.long_edge {
            s.long_edge = edge;
        }
        s.choose_format(format);
        s.choose_color_space(self.color_space);
        s.choose_hdr(self.hdr);
        s
    }
}

impl Context {
    /// Develops `photo` with `recipe` at full quality and writes it to
    /// `target`, encoded beside it first and then put in place, so a failed
    /// export leaves the destination as it was. Returns what the export has to
    /// say about the file, if anything (a JPEG larger than its size limit, say),
    /// and reports progress from 0 to 1.
    ///
    /// The photo's full decode is the one an [`Image`](crate::Image) the app
    /// holds of it, else the context's cache's copy, else decoded here and
    /// not kept. A long operation: `cancel` stops it between stages, and a file
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
        settings.check()?;
        let settings = settings.desktop(format);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The library's defaults are the desktop dialog's, and each setting
    /// reaches the desktop's own.
    #[test]
    fn the_settings_are_the_desktops() {
        let mut desktop = rawmakase_interop::export_settings::ExportSettings {
            descriptive: false,
            ..Default::default()
        };
        desktop.choose_format(Format::Png);
        assert_eq!(ExportSettings::default().desktop(Format::Png), desktop);
        for refused in [
            ExportSettings {
                quality: 0,
                ..Default::default()
            },
            ExportSettings {
                quality: 101,
                ..Default::default()
            },
            ExportSettings {
                size_limit: Some(0),
                ..Default::default()
            },
            ExportSettings {
                long_edge: Some(0),
                ..Default::default()
            },
            ExportSettings {
                ppi: 0,
                ..Default::default()
            },
            ExportSettings {
                ppi: 10_001,
                ..Default::default()
            },
        ] {
            let e = refused.check().unwrap_err();
            assert_eq!(e.kind(), crate::ErrorKind::Invalid, "{refused:?}");
        }
        ExportSettings::default().check().unwrap();
        let s = ExportSettings {
            color_space: OutputSpace::AdobeRgb,
            quality: 70,
            size_limit: Some(300),
            long_edge: Some(1600),
            ..Default::default()
        }
        .desktop(Format::Jpeg);
        let options = s.options();
        assert_eq!(s.output_space().unwrap(), OutputSpace::AdobeRgb);
        assert_eq!(options.quality, 70);
        assert_eq!(options.max_edge, 1600);
        assert_eq!(options.max_bytes, Some(300_000));
    }
}
