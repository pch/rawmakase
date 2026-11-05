//! Lightroom's export watermark: a text or graphic mark, laid over the
//! exported pixels after the render and before they are quantized, so a
//! 16-bit TIFF keeps its precision outside the mark and under a partly
//! transparent one. Watermarks are saved as presets in the data folder's
//! `watermarks`, a graphic one with its own copy of the image.
pub mod fonts;
mod raster;
use crate::color::OutputSpace;
use crate::develop::ExportImage;
use crate::storage::RasterFormat;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use crate::export_settings::SIMPLE_COPYRIGHT;

/// The formats a graphic watermark's image can be in.
pub const IMAGE_FORMATS: [RasterFormat; 2] = [RasterFormat::Png, RasterFormat::Jpeg];
/// [`IMAGE_FORMATS`] by name: "PNG or JPEG".
pub fn image_formats_named() -> String {
    IMAGE_FORMATS.map(RasterFormat::name).join(" or ")
}
/// Every extension [`IMAGE_FORMATS`] have, for a file dialog.
pub fn image_extensions() -> Vec<&'static str> {
    IMAGE_FORMATS
        .into_iter()
        .flat_map(|f| f.extensions().iter().copied())
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Style {
    #[default]
    Text,
    Graphic,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}
/// How large the mark is: a fraction of the photo's width, as wide as the
/// photo (never taller than it), or covering it both ways, cut at its
/// edges. Insets are taken off first.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Size {
    Proportional(f32),
    Fit,
    Fill,
}
impl Default for Size {
    fn default() -> Self {
        Self::Proportional(0.2)
    }
}
/// One of nine points: (horizontal, vertical), each 0 start, 1 center, 2 end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor(pub u8, pub u8);
impl Default for Anchor {
    /// Lightroom's: bottom left.
    fn default() -> Self {
        Self(0, 2)
    }
}
/// A drop shadow under the text, as Lightroom's: offset and radius in
/// fractions of the text size, the angle in degrees (0 points right, 90 up).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    pub enabled: bool,
    pub opacity: f32,
    pub offset: f32,
    pub radius: f32,
    pub angle: f32,
}
impl Default for Shadow {
    fn default() -> Self {
        Self {
            enabled: false,
            opacity: 0.5,
            offset: 0.04,
            radius: 0.04,
            angle: -45.,
        }
    }
}

/// A watermark preset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watermark {
    pub name: String,
    pub style: Style,
    pub text: String,
    /// Font family and style, as the font menus name them.
    pub family: String,
    pub face: String,
    pub align: Align,
    /// sRGB, 0–1.
    pub color: [f32; 3],
    pub shadow: Shadow,
    /// The preset's own copy of its image, by file name in
    /// `watermarks/images`.
    pub image: Option<String>,
    pub opacity: f32,
    pub size: Size,
    /// Horizontal and vertical inset, fractions of the photo's width and
    /// height.
    pub inset: [f32; 2],
    pub anchor: Anchor,
    /// Quarter turns counter-clockwise.
    pub rotation: u8,
}
impl Default for Watermark {
    fn default() -> Self {
        Self {
            name: String::new(),
            style: Style::Text,
            text: "Copyright".into(),
            family: fonts::INTER.into(),
            face: "Regular".into(),
            align: Align::Left,
            color: [1., 1., 1.],
            shadow: Shadow::default(),
            image: None,
            opacity: 1.,
            size: Size::default(),
            inset: [0.02, 0.02],
            anchor: Anchor::default(),
            rotation: 0,
        }
    }
}

/// A mark ready to lay over a photo: straight-alpha RGBA in output-encoded
/// sRGB, and where its top left corner goes.
pub struct Placed {
    pub x: i64,
    pub y: i64,
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<[f32; 4]>,
}

/// A watermark with what it needs loaded: its image decoded or its font
/// read, so an export can fail before rendering when either is missing.
#[derive(Clone)]
pub struct Ready {
    watermark: Watermark,
    asset: Asset,
}

/// What a watermark draws: a graphic's image or a text's font.
#[derive(Clone)]
enum Asset {
    Image(std::sync::Arc<image::Rgba32FImage>),
    Font(fonts::Font),
}

pub fn dir() -> PathBuf {
    crate::storage::data_dir().join("watermarks")
}
fn images_dir() -> PathBuf {
    dir().join("images")
}

impl Watermark {
    /// Lightroom's Simple Copyright Watermark: the copyright as text, small,
    /// bottom left.
    pub fn simple_copyright(copyright: &str) -> Self {
        Self {
            name: "Simple Copyright Watermark".into(),
            text: copyright.into(),
            ..Default::default()
        }
    }
    /// Loads its image or font, from `images` for a graphic preset.
    pub fn ready_in(&self, images: &Path) -> Result<Ready> {
        let asset = match self.style {
            Style::Graphic => {
                let name = self.image.clone().unwrap_or_default();
                let path = images.join(&name);
                ensure!(
                    !name.is_empty() && path.is_file(),
                    "Watermark image not found: {name}"
                );
                Asset::Image(std::sync::Arc::new(
                    decode(&path)
                        .with_context(|| format!("Watermark image not readable: {name}"))?,
                ))
            }
            Style::Text => Asset::Font(fonts::load(&self.family, &self.face)?),
        };
        Ok(Ready {
            watermark: self.clone(),
            asset,
        })
    }
    pub fn ready(&self) -> Result<Ready> {
        self.ready_in(&images_dir())
    }
    /// A graphic watermark whose image is gone, without reading it.
    pub fn image_missing(&self) -> bool {
        self.style == Style::Graphic
            && !self
                .image
                .as_ref()
                .is_some_and(|name| images_dir().join(name).is_file())
    }
}

impl Ready {
    /// The same image or font with other settings: a preview changing its
    /// size or opacity doesn't read them again.
    pub fn with(&self, watermark: &Watermark) -> Self {
        Self {
            watermark: watermark.clone(),
            asset: self.asset.clone(),
        }
    }
    /// The mark for a photo `width` × `height`, placed.
    pub fn place(&self, width: u32, height: u32) -> Option<Placed> {
        let w = &self.watermark;
        let (pw, ph) = (width as f32, height as f32);
        let room = (
            (pw * (1. - 2. * w.inset[0])).max(1.),
            (ph * (1. - 2. * w.inset[1])).max(1.),
        );
        let turned = w.rotation % 2 == 1;
        // The mark's own aspect, as shown after its rotation.
        let aspect = |mw: f32, mh: f32| if turned { mh / mw } else { mw / mh };
        let fit = |aspect: f32| -> (f32, f32) {
            match w.size {
                // Never larger than the photo, a turned mark included.
                Size::Proportional(p) => {
                    let mw = (pw * p.clamp(0.01, 1.)).min(room.0).min(room.1 * aspect);
                    (mw, mw / aspect)
                }
                // As wide as the photo, unless that makes it taller.
                Size::Fit => {
                    let mw = room.0.min(room.1 * aspect);
                    (mw, mw / aspect)
                }
                // Covers the photo both ways, cut at its edges; at most twice
                // its size, so a thin mark can't grow without bound.
                Size::Fill => {
                    let mw = room
                        .0
                        .max(room.1 * aspect)
                        .min(2. * room.0)
                        .min(2. * room.1 * aspect);
                    (mw, mw / aspect)
                }
            }
        };
        let mut rgba;
        let (mut mw, mut mh);
        match &self.asset {
            Asset::Image(image) => {
                let (iw, ih) = (image.width() as f32, image.height() as f32);
                let (tw, th) = fit(aspect(iw, ih));
                let (sw, sh) = if turned { (th, tw) } else { (tw, th) };
                let (sw, sh) = (sw.round().max(1.) as u32, sh.round().max(1.) as u32);
                let scaled = image::imageops::resize(
                    image.as_ref(),
                    sw,
                    sh,
                    image::imageops::FilterType::Lanczos3,
                );
                (mw, mh) = (sw as usize, sh as usize);
                // Back to straight alpha from the premultiplied scaling.
                rgba = scaled
                    .pixels()
                    .map(|p| {
                        let [r, g, b, a] = p.0;
                        let a = a.clamp(0., 1.);
                        if a <= 0. {
                            return [0.; 4];
                        }
                        [
                            (r / a).clamp(0., 1.),
                            (g / a).clamp(0., 1.),
                            (b / a).clamp(0., 1.),
                            a,
                        ]
                    })
                    .collect();
            }
            Asset::Font(font) => {
                // Measured at a reference size, then drawn at the one that
                // gives the mark its width.
                let reference = raster::measure(font, &w.text, 100.)?;
                let (tw, _) = fit(aspect(reference.0, reference.1));
                let target = if turned {
                    tw * reference.0 / reference.1
                } else {
                    tw
                };
                let px = 100. * target / reference.0.max(1e-3);
                let limit = if turned { (ph, pw) } else { (pw, ph) };
                let text = raster::text(font, &w.text, px, w.align, w.color, &w.shadow, limit)?;
                (mw, mh, rgba) = (text.width, text.height, text.rgba);
            }
        }
        for _ in 0..w.rotation % 4 {
            (rgba, mw, mh) = rotate(&rgba, mw, mh);
        }
        let opacity = w.opacity.clamp(0., 1.);
        for p in &mut rgba {
            p[3] *= opacity;
        }
        let along = |slot: u8, size: f32, mark: usize, inset: f32| -> i64 {
            (match slot {
                0 => size * inset,
                1 => (size - mark as f32) / 2.,
                _ => size - mark as f32 - size * inset,
            })
            .round() as i64
        };
        Some(Placed {
            x: along(w.anchor.0, pw, mw, w.inset[0]),
            y: along(w.anchor.1, ph, mh, w.inset[1]),
            width: mw,
            height: mh,
            rgba,
        })
    }
    /// Lays the mark over `image`'s pixels; false when there was nothing
    /// to draw (text the font has none of). The mark's colours, its text, shadow
    /// and graphic, are sRGB; in another space they are converted to it first, so
    /// a red mark is the same red in every space. A graphic's own profile is
    /// ignored, as it always has been.
    pub fn apply(&self, image: &mut ExportImage) -> bool {
        let space = image.space();
        let (width, height) = (image.width(), image.height());
        let Some(mut mark) = self.place(width, height) else {
            return false;
        };
        if space != OutputSpace::Srgb {
            for p in &mut mark.rgba {
                let [r, g, b] = space.from_srgb([p[0], p[1], p[2]]);
                [p[0], p[1], p[2]] = [r, g, b];
            }
        }
        composite(image.pixels_mut(), width, height, &mark);
        true
    }
}

/// Alpha-over in the image's encoded values: pixels the mark doesn't cover keep
/// their exact values.
fn composite(pixels: &mut [[f32; 3]], width: u32, height: u32, mark: &Placed) {
    let (w, h) = (i64::from(width), i64::from(height));
    for my in 0..mark.height as i64 {
        let y = mark.y + my;
        if y < 0 || y >= h {
            continue;
        }
        for mx in 0..mark.width as i64 {
            let x = mark.x + mx;
            if x < 0 || x >= w {
                continue;
            }
            let [r, g, b, a] = mark.rgba[(my as usize) * mark.width + mx as usize];
            if a <= 0. {
                continue;
            }
            let p = &mut pixels[(y * w + x) as usize];
            for (c, m) in p.iter_mut().zip([r, g, b]) {
                *c = m * a + *c * (1. - a);
            }
        }
    }
}

/// A quarter turn counter-clockwise.
fn rotate(rgba: &[[f32; 4]], w: usize, h: usize) -> (Vec<[f32; 4]>, usize, usize) {
    let mut out = vec![[0.; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            // (x, y) goes to (y, w - 1 - x) in an h-wide image.
            out[(w - 1 - x) * h + y] = rgba[y * w + x];
        }
    }
    (out, h, w)
}

/// An image as shown: its orientation applied, as premultiplied RGBA so
/// scaling never bleeds the color of transparent pixels into the edges.
fn decode(path: &Path) -> Result<image::Rgba32FImage> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    // The decoder's default size limits, which reading through it skips.
    decoder.set_limits(image::Limits::default())?;
    // Converted to 16 bytes a pixel below: larger than any logo needs.
    let (w, h) = image::ImageDecoder::dimensions(&decoder);
    ensure!(
        u64::from(w) * u64::from(h) <= 40_000_000,
        "The image is too large for a watermark ({w} × {h})"
    );
    let orientation = decoder.orientation()?;
    let mut image = image::DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let mut rgba = image.into_rgba32f();
    for p in rgba.pixels_mut() {
        let a = p.0[3].clamp(0., 1.);
        for c in &mut p.0[..3] {
            *c *= a;
        }
    }
    Ok(rgba)
}

/// A preset's file name as disks compare them: without case, and in one
/// Unicode normalization (APFS ignores both).
fn disk_name(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    file_name(name).to_lowercase().nfc().collect()
}

/// Whether two paths are one file, however the disk compares names.
fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    }
}

/// Saved presets, by name.
pub fn presets() -> Vec<Watermark> {
    presets_in(&dir())
}
pub fn presets_in(dir: &Path) -> Vec<Watermark> {
    let mut found: Vec<Watermark> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    found.sort_by_key(|w: &Watermark| w.name.to_lowercase());
    found
}
/// A preset's file name for its name.
fn file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{}.json", safe.trim())
}
/// Saves `watermark` as a new preset, or (`replacing`) in place of the
/// preset of that name, which it renames when its name differs. A graphic
/// one takes its own copy of `source`, the image chosen for it, or keeps
/// the image it has under its own name. Everything is checked before any
/// file changes, and the old files go only once the new ones are written.
pub fn save_in(
    dir: &Path,
    watermark: &Watermark,
    replacing: Option<&str>,
    source: Option<&Path>,
) -> Result<Watermark> {
    ensure!(!watermark.name.trim().is_empty(), "Name the watermark");
    ensure!(
        watermark.style == Style::Graphic || !watermark.text.trim().is_empty(),
        "Type the watermark's text"
    );
    ensure!(
        watermark.name != SIMPLE_COPYRIGHT,
        "That name is reserved; choose another"
    );
    let presets = presets_in(dir);
    // Another preset of this name, or of one that makes the same file,
    // would be overwritten.
    if let Some(other) = presets.iter().find(|p| {
        Some(p.name.as_str()) != replacing
            // Case too: most Mac and Windows disks don't tell names apart by it.
            && disk_name(&p.name) == disk_name(&watermark.name)
    }) {
        anyhow::bail!(
            "A watermark named \"{}\" exists; choose another name",
            other.name
        );
    }
    let previous = replacing.and_then(|r| presets.iter().find(|p| p.name == r).cloned());
    let extension = |path: &Path| {
        path.extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    };
    if let Some(source) = source {
        ensure!(
            RasterFormat::of(source).is_some_and(|f| IMAGE_FORMATS.contains(&f)),
            "Choose a {} image",
            image_formats_named()
        );
        decode(source).context("The image can't be read")?;
    }
    // Checked once installed fonts are listed; until then the export checks.
    if watermark.style == Style::Text
        && (watermark.family == fonts::INTER || fonts::families_if_listed().is_some())
    {
        let font = fonts::load(&watermark.family, &watermark.face)?;
        ensure!(
            raster::measure(&font, &watermark.text, 100.).is_some(),
            "The font has none of the text's characters; choose another"
        );
    }
    let images = dir.join("images");
    std::fs::create_dir_all(&images)?;
    let stem = file_name(&watermark.name)
        .trim_end_matches(".json")
        .to_string();
    let mut saved = watermark.clone();
    // The image, named after its preset, is copied beside first; it
    // replaces the live one only once the preset is written.
    let stage = |from: &Path| {
        crate::storage::stage(&images, |f| {
            std::fs::copy(from, f.path())?;
            Ok(())
        })
    };
    let mut staged = None;
    if let Some(source) = source {
        let name = format!("{stem}.{}", extension(source));
        staged = Some((stage(source)?, name.clone()));
        saved.image = Some(name);
    } else if let Some(image) = &watermark.image {
        // The image it keeps must still be there and readable.
        if watermark.style == Style::Graphic {
            decode(&images.join(image))
                .context("The watermark's image can't be read; choose it again")?;
        }
        let name = format!("{stem}.{}", extension(Path::new(image)));
        if *image != name && images.join(image).is_file() {
            staged = Some((stage(&images.join(image))?, name.clone()));
            saved.image = Some(name);
        }
    }
    crate::storage::atomic_json(&dir.join(file_name(&saved.name)), &saved)?;
    if let Some((file, name)) = staged {
        crate::storage::persist(file, &images.join(name), crate::storage::Replace::Overwrite)?;
    }
    if let Some(previous) = previous {
        // A rename by case alone is one file on most Mac and Windows disks.
        let old = dir.join(file_name(&previous.name));
        if !same_file(&old, &dir.join(file_name(&saved.name))) {
            std::fs::remove_file(old)
                .context("Saved under the new name, but the old one could not be removed")?;
        }
        if let Some(old) = previous.image.map(|i| images.join(i)) {
            let kept = saved
                .image
                .as_ref()
                .is_some_and(|s| same_file(&old, &images.join(s)));
            if !kept {
                let _ = std::fs::remove_file(old);
            }
        }
    }
    Ok(saved)
}
pub fn save(
    watermark: &Watermark,
    replacing: Option<&str>,
    source: Option<&Path>,
) -> Result<Watermark> {
    save_in(&dir(), watermark, replacing, source)
}
/// Deletes a preset and the image it owns.
pub fn delete_in(dir: &Path, watermark: &Watermark) -> Result<()> {
    let path = dir.join(file_name(&watermark.name));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    // The preset is gone; an image that can't be removed is only an unused
    // file, not a failed delete.
    if let Some(image) = &watermark.image {
        let _ = std::fs::remove_file(dir.join("images").join(image));
    }
    Ok(())
}
pub fn delete(watermark: &Watermark) -> Result<()> {
    delete_in(&dir(), watermark)
}
#[cfg(test)]
mod tests;
