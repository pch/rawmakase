//! Atomic JPEG, 16-bit TIFF and PNG export with the ICC profile of the pixels' colour
//! space (sRGB, Adobe RGB (1998) or ProPhoto RGB), the camera's EXIF and the edit as
//! Camera Raw XMP, as Lightroom embeds them.
pub mod assemble;
pub mod batch;
mod encode;
pub mod exif;
mod extended_xmp;
pub mod gain_map;
pub mod job;
mod jxl;
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

/// An export's pixels: an SDR render in its colour space, or an HDR master encoded
/// for its HDR output.
#[derive(Clone)]
pub enum Pixels {
    Sdr(ExportImage),
    Hdr(HdrPixels),
    /// The SDR rendition (sRGB) with a gain map to the HDR master.
    GainMap(GainMapPixels),
}
/// A gain-map export's renditions: its sRGB base and the map over it.
#[derive(Clone)]
pub struct GainMapPixels {
    pub base: ExportImage,
    /// One 8-bit sample a pixel, the base's size.
    pub map: Vec<u8>,
    pub metadata: gain_map::Metadata,
}
impl GainMapPixels {
    /// The gain map from `base`, the SDR rendition in sRGB, to `master`, of the same
    /// size.
    pub fn new(base: ExportImage, master: &crate::rendered::HdrImage) -> Result<Self> {
        use crate::color::hdr_output::HdrGamut;
        ensure!(
            base.space() == crate::color::OutputSpace::Srgb
                && (base.width(), base.height()) == (master.width, master.height),
            "A gain map's base is the master's size, in sRGB"
        );
        let to_rgb = HdrGamut::Rec709.prophoto_to_rgb();
        let hdr: Vec<[f32; 3]> = master
            .pixels
            .iter()
            .map(|p| crate::color::mul(to_rgb, *p))
            .collect();
        let sdr: Vec<[f32; 3]> = base
            .pixels()
            .iter()
            .map(|p| p.map(|v| crate::color::srgb_decode(v.clamp(0., 1.))))
            .collect();
        let (map, metadata) = gain_map::compute(&sdr, &hdr, master.peak)?;
        Ok(Self {
            base,
            map,
            metadata,
        })
    }
}
/// An HDR master encoded by [`HdrOutput::encode`]: PQ and HLG signal values (0–1) or
/// linear light (1 = graphics white), in its gamut, row by row.
///
/// [`HdrOutput::encode`]: crate::color::hdr_output::HdrOutput::encode
#[derive(Clone)]
pub struct HdrPixels {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
    pub output: crate::color::hdr_output::HdrOutput,
}
impl HdrPixels {
    /// `master` encoded for `output`.
    pub fn encode(
        master: &crate::rendered::HdrImage,
        output: crate::color::hdr_output::HdrOutput,
    ) -> Self {
        Self {
            width: master.width,
            height: master.height,
            pixels: master
                .pixels
                .iter()
                .map(|p| output.encode(*p, master.peak))
                .collect(),
            output,
        }
    }
}
impl Pixels {
    pub fn width(&self) -> u32 {
        match self {
            Self::Sdr(image) => image.width(),
            Self::Hdr(image) => image.width,
            Self::GainMap(image) => image.base.width(),
        }
    }
    pub fn height(&self) -> u32 {
        match self {
            Self::Sdr(image) => image.height(),
            Self::Hdr(image) => image.height,
            Self::GainMap(image) => image.base.height(),
        }
    }
}

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

/// Exports `pixels` to `path`: [`export_image`] of an SDR render, an HDR TIFF of an
/// HDR master.
pub fn export_pixels(
    path: &Path,
    source: &Path,
    pixels: &Pixels,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
    replace: Replace,
) -> Result<()> {
    match pixels {
        Pixels::Sdr(image) => write(path, source, image, m, options, embed, replace),
        Pixels::Hdr(_) | Pixels::GainMap(_) => {
            check_destination(path, source, replace)?;
            let staged = stage_pixels(path, pixels, m, options, embed)?;
            crate::storage::persist(staged.file, path, replace)
        }
    }
}

fn check_destination(path: &Path, source: &Path, replace: Replace) -> Result<()> {
    ensure!(!is_raw(path), "An export cannot overwrite a RAW file");
    if path.exists() {
        ensure!(
            fs::canonicalize(path)? != fs::canonicalize(source)?,
            "Cannot overwrite source"
        );
        ensure!(replace == Replace::Overwrite, "Destination already exists");
    }
    Ok(())
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
    check_destination(path, source, replace)?;
    options.validate()?;
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

/// [`stage_image`] or, for an HDR master, [`stage_hdr`].
pub fn stage_pixels(
    path: &Path,
    pixels: &Pixels,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
) -> Result<Staged> {
    match pixels {
        Pixels::Sdr(image) => stage_image(path, image, m, options, embed),
        Pixels::Hdr(image) => stage_hdr(path, image, m, embed),
        Pixels::GainMap(image) => stage_gain_map(path, image, m, options, embed),
    }
}

/// A gain-map JPEG: the sRGB base as an SDR JPEG export is, with its ICC profile, EXIF
/// and XMP, the gain map's grayscale JPEG after it, and their ISO 21496-1 metadata and
/// MPF index. Any other format is refused before anything is written.
pub fn stage_gain_map(
    path: &Path,
    image: &GainMapPixels,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
) -> Result<Staged> {
    ensure!(!is_raw(path), "An export cannot overwrite a RAW file");
    options.validate()?;
    let format = Format::from_path(path)
        .filter(|f| matches!(f, Format::Jpeg | Format::Jxl))
        .ok_or_else(|| {
            anyhow::anyhow!("A gain-map export is a JPEG or JPEG XL: name it .jpg, .jpeg or .jxl")
        })?;
    let parent = crate::storage::parent_dir(path);
    let mut temp = NamedTempFile::new_in(parent)?;
    let unfinished = WRITING.track(temp.path());
    let base = &image.base;
    let directories = metadata::directories(m, embed, base.width(), base.height(), base.space());
    if format == Format::Jxl {
        encode::jxl_gain_map(&mut temp, image, directories, embed)?;
        temp.as_file().sync_all()?;
        return Ok(Staged {
            file: temp,
            _unfinished: unfinished,
        });
    }
    let profile = crate::color_management::output_profile(base.space()).to_vec();
    let mut primary = Vec::new();
    encode::jpeg(
        &mut primary,
        base,
        options.quality,
        profile,
        directories,
        embed,
    )?;
    let mut map = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut map, options.quality).encode(
        &image.map,
        base.width(),
        base.height(),
        image::ExtendedColorType::L8,
    )?;
    std::io::Write::write_all(
        &mut temp,
        &gain_map::assemble(&primary, &map, &image.metadata)?,
    )?;
    temp.as_file().sync_all()?;
    Ok(Staged {
        file: temp,
        _unfinished: unfinished,
    })
}

/// An HDR master as a TIFF, with its encoding's ICC profile (CICP and colorants):
/// 16-bit PQ or HLG signal, or 32-bit float linear light; or as a lossless 16-bit
/// JPEG XL naming its encoding in its header. Any other format is refused before
/// anything is written.
pub fn stage_hdr(path: &Path, image: &HdrPixels, m: &Metadata, embed: &Embed) -> Result<Staged> {
    ensure!(!is_raw(path), "An export cannot overwrite a RAW file");
    let format = Format::from_path(path)
        .filter(|f| matches!(f, Format::Tiff | Format::Jxl))
        .ok_or_else(|| {
            anyhow::anyhow!("An HDR export is a TIFF or JPEG XL: name it .tif, .tiff or .jxl")
        })?;
    ensure!(
        image.pixels.len() == image.width as usize * image.height as usize,
        "HDR export has the wrong size"
    );
    let parent = crate::storage::parent_dir(path);
    let mut temp = NamedTempFile::new_in(parent)?;
    let unfinished = WRITING.track(temp.path());
    let directories = metadata::directories_uncalibrated(m, embed, image.width, image.height);
    match format {
        Format::Jxl => encode::jxl_hdr(&mut temp, image, directories, embed)?,
        _ => {
            let profile = crate::color_management::hdr_profile(image.output)?;
            encode::tiff_hdr(&mut temp, image, profile, &directories, embed)?
        }
    }
    temp.as_file().sync_all()?;
    Ok(Staged {
        file: temp,
        _unfinished: unfinished,
    })
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
        Format::Jxl => encode::jxl(&mut temp, image, directories, embed)?,
    }
    temp.as_file().sync_all()?;
    Ok(Staged {
        file: temp,
        _unfinished: unfinished,
    })
}

#[cfg(test)]
mod hdr_tests;
#[cfg(test)]
mod srgb_fixtures;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wide_gamut_tests;
