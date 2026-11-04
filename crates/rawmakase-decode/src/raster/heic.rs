//! A HEIC (a HEIF of HEVC) still: our reading of its container
//! ([`rawmakase_interop::isobmff`]) here, its pixels decoded by the system's
//! HEVC decoder: macOS's ImageIO (`imageio`), Windows Imaging Component
//! (`wic`), and elsewhere, as on Linux, the system's libheif (`libheif`),
//! opened when first needed. The header here says the size, crop, colour and
//! orientation, and what Develop refuses.
//!
//! RAWmakase has no HEVC decoder of its own, as HEVC is patented. Without the
//! system's, the header is still read, so the photo is listed with its size
//! and metadata, and opening it says what to install.
//!
//! A phone's photo is a grid (`grid`) of tiles, 512×512 on an iPhone, each its
//! own HEVC picture. A clean aperture (`clap`) crops the picture, before its
//! rotation and mirror, the order MIAF requires.
//!
//! The colour comes from the `colr` boxes of the image, or of its first tile
//! when the image has none of that kind (an iPhone's tiles carry the `nclx`):
//! an ICC profile, or one built from the CICP code points; a HEIC with neither
//! is taken as sRGB. The system decoder chooses the matrix and range itself.
//! Alpha, depth maps, gain maps and thumbnails are not shown; an alpha image,
//! which a system decoder decodes with the image, is held to the image's
//! limits with it.
// Every unsafe operation here crosses into a C API: each one says why it is
// sound, and an unsafe function's body is no unsafe block of its own.
#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]
use super::heif::{self, Nclx};
use super::{Unsupported, hevc};
use anyhow::{Result, anyhow, bail, ensure};
use image::ImageDecoder;
use image::metadata::Orientation;
use rawmakase_interop::isobmff::{Heif, Property};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod imageio;
#[cfg(target_os = "macos")]
use imageio::pixels;
#[cfg(windows)]
mod wic;
#[cfg(windows)]
use wic::pixels;
#[cfg(all(unix, not(target_vendor = "apple")))]
mod libheif;
#[cfg(all(unix, not(target_vendor = "apple")))]
use libheif::pixels;
/// No decoder on this system.
#[cfg(not(any(unix, windows)))]
fn pixels(_: HeicFile) -> Result<Pixels> {
    bail!("RAWmakase can't show HEIF images on this system")
}

/// Whether this system can decode a HEIC's pixels, or what to install:
/// macOS can; Windows' extensions are found only by decoding; elsewhere,
/// libheif with its HEVC decoder.
pub(super) fn decoder() -> std::result::Result<(), String> {
    #[cfg(all(unix, not(target_vendor = "apple")))]
    return libheif::available();
    #[cfg(any(target_os = "macos", windows))]
    return Ok(());
    #[cfg(not(any(unix, windows)))]
    Err("RAWmakase can't show HEIF images on this system".into())
}

/// Decoded pixels: `width` × `height` of interleaved RGB, 8-bit samples or
/// 16-bit ones (native endian), as [`HeicFile`]'s colour type says, in the
/// colour space its profile says: the whole picture, or (a system decoder
/// that crops by the `clap` itself) the clean aperture alone.
struct Pixels {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

/// The most pixels decoded for an image, each of its tiles whole, as many as
/// `prepare` lets an image have.
const MAX_PIXELS: u64 = 150_000_000;

/// Whether pictures placed with `areas`, each the largest any of its SPS
/// codes, take no more than [`MAX_PIXELS`] decoded together.
fn coded_fits(areas: impl Iterator<Item = u128>) -> bool {
    areas.sum::<u128>() <= u128::from(MAX_PIXELS)
}

/// Whether pictures laid out as `layouts`, each tile decoded whole, take no
/// more than [`MAX_PIXELS`] together.
fn within_limit<'a>(layouts: impl IntoIterator<Item = &'a Layout>) -> bool {
    let area = |l: &Layout| l.tiles.len() as u128 * u128::from(l.tile.0) * u128::from(l.tile.1);
    layouts.into_iter().map(area).sum::<u128>() <= u128::from(MAX_PIXELS)
}

/// The `auxC` types of an alpha image, as libheif reads them: MPEG's, and
/// HEVC's own.
const ALPHA: [&str; 2] = [
    "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha",
    "urn:mpeg:hevc:2015:auxid:1",
];

/// The alpha images of item `id`, which a system decoder decodes with it.
/// Its other auxiliary images, such as depth and gain maps, it does not.
fn alpha(heif: &Heif, id: u32) -> Vec<u32> {
    heif.describing(*b"auxl", id)
        .into_iter()
        .filter(|aux| {
            heif.properties(*aux)
                .iter()
                .any(|p| matches!(p, Property::Auxiliary(kind) if ALPHA.contains(&kind.as_str())))
        })
        .collect()
}

/// Where the image is in the decoded picture: its clean aperture.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Crop {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
}

/// The pictures an image is decoded from: one, or a grid's tiles.
struct Layout {
    /// The whole picture's size, before its crop.
    width: u32,
    height: u32,
    /// The pictures, row by row: each item and its HEVC configuration, one
    /// for each item a grid lists however often.
    tiles: Vec<(u32, Arc<hevc::Config>)>,
    /// Each picture's size.
    tile: (u32, u32),
}

pub(super) struct HeicFile {
    crop: Crop,
    /// More than 8 bits per sample, from `hvcC`.
    deep: bool,
    /// The file and its boxes, to read the pictures from when decoding. The
    /// system decoder reads this open file too, never the path again, so
    /// what it decodes is what the header's checks passed.
    file: std::fs::File,
    heif: Heif,
    layout: Layout,
    /// Every picture the system decoder decodes, each item as often as it
    /// is placed: the image's tiles, then its alpha's.
    pictures: Vec<(u32, Arc<hevc::Config>)>,
    icc: Option<Vec<u8>>,
    nclx: Option<Nclx>,
    orientation: Orientation,
    refused: Option<Unsupported>,
}

impl HeicFile {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let heif = Heif::read(&mut file).ok_or_else(|| anyhow!("Could not read the HEIF file"))?;
        let primary = heif
            .item(heif.primary)
            .ok_or_else(|| anyhow!("The HEIF file has no image"))?;
        if let Some(kind) = heif.unread_essential(heif.primary) {
            let kind = String::from_utf8_lossy(&kind);
            bail!("The HEIF image needs `{kind}`, which RAWmakase can't read");
        }
        ensure!(
            matches!(&primary.kind, b"hvc1" | b"grid"),
            "The HEIF image is not HEVC"
        );
        let layout = layout_of(&heif, &mut file, heif.primary)?;
        let alphas = alpha(&heif, heif.primary)
            .into_iter()
            .map(|id| layout_of(&heif, &mut file, id))
            .collect::<Result<Vec<_>>>()?;
        // Every tile is decoded whole, the alpha's too, so the tiles, which
        // cover the image, bound the work.
        ensure!(
            within_limit(std::iter::once(&layout).chain(&alphas)),
            "Image too large to preview"
        );
        let pictures = std::iter::once(&layout)
            .chain(&alphas)
            .flat_map(|l| l.tiles.iter().cloned())
            .collect();
        let props = heif.properties(heif.primary);
        let crop = crop(&props, layout.width, layout.height)?;
        let (icc, nclx) = heif::colour(&props);
        let (tile_icc, tile_nclx) = heif::colour(&heif.properties(layout.tiles[0].0));
        let (icc, nclx) = (icc.or(tile_icc), nclx.or(tile_nclx));
        let (depth, chroma) = (layout.tiles[0].1.depth, layout.tiles[0].1.chroma);
        ensure!(
            layout
                .tiles
                .iter()
                .all(|(_, c)| (c.depth, c.chroma) == (depth, chroma)),
            "The HEIF image's tiles differ in depth or chroma"
        );
        let animated = [b"msf1", b"hevc", b"hevx", b"hevm", b"hevs"];
        // The system's decoder decides what chroma and depth it reads, and
        // says what to install when there is none.
        let refused = if nclx.is_some_and(Nclx::hdr) {
            Some(Unsupported("HDR".into()))
        } else if heif.brands.iter().any(|b| animated.contains(&b)) {
            Some(Unsupported("animated".into()))
        } else {
            None
        };
        Ok(Self {
            crop,
            deep: depth > 8,
            file,
            orientation: heif::transforms(&props),
            heif,
            layout,
            pictures,
            icc,
            nclx,
            refused,
        })
    }

    pub(super) fn refusal(&self) -> Option<Unsupported> {
        self.refused.as_ref().map(|r| Unsupported(r.0.clone()))
    }

    /// Whether every picture's data is in the file, before any decoder reads
    /// it: the header, not the decoder, says a file cut short is one, whose
    /// pixels a system decoder might make up for.
    fn stored(&mut self) -> Result<()> {
        let mut checked = std::collections::HashSet::new();
        for (id, _) in &self.pictures {
            ensure!(
                !checked.insert(*id) || self.heif.stored_within(&mut self.file, *id),
                "The HEIF image data is missing or cut off"
            );
        }
        Ok(())
    }

    /// Whether the pictures, at the size their SPS code them, take no more
    /// than [`MAX_PIXELS`] together. The system's decoder decodes a picture
    /// at its coded size, padding and all, whatever the header says, so this,
    /// not the header, bounds its work. A coded size larger than the header's
    /// is not refused itself: encoders pad (Apple's codes a 64×64 picture as
    /// 160×64), and the decoded image is still held to the header's size.
    fn coded_within(&mut self) -> Result<()> {
        let too_large = || anyhow!("The HEVC image is coded at more than 150 megapixels");
        // Each distinct picture, and how often it is placed.
        let mut placed: Vec<(u32, u128, &hevc::Config)> = Vec::new();
        for (id, config) in &self.pictures {
            match placed.iter_mut().find(|(placed, ..)| placed == id) {
                Some((_, count, _)) => *count += 1,
                None => placed.push((*id, 1, config)),
            }
        }
        // The sizes the configurations code first, which read no data.
        let configured = |c: &hevc::Config| c.coded.iter().map(|s| hevc::area(*s)).max();
        ensure!(
            coded_fits(
                placed
                    .iter()
                    .map(|(_, n, c)| n * configured(c).unwrap_or(0))
            ),
            too_large()
        );
        // Then each picture's own SPS, read from its data. Pictures stored
        // apart lie apart in the file, so their data together is no larger
        // than it; more is extents repeating one stretch of it, refused
        // before it is read. Pictures with the same stored data, as a grid's
        // identical tiles may have, read and count it once.
        let larger = || anyhow!("The HEIF image data is larger than the file");
        let mut budget = self.file.metadata()?.len();
        let mut read = HashMap::new();
        let mut total = 0;
        for (id, count, config) in placed {
            let stored = self
                .heif
                .extents(id)
                .ok_or_else(|| anyhow!("The HEIF image data is missing or cut off"))?;
            let key = (stored, config.length_size);
            let in_data = match read.get(&key) {
                Some(largest) => *largest,
                None => {
                    let len = key.0.1.iter().try_fold(0u64, |n, (_, l)| n.checked_add(*l));
                    budget = len
                        .and_then(|len| budget.checked_sub(len))
                        .ok_or_else(larger)?;
                    let data = self
                        .heif
                        .data_up_to(&mut self.file, id, len.unwrap_or(0))
                        .ok_or_else(|| anyhow!("The HEIF image data is missing or cut off"))?;
                    let largest = hevc::largest_coded_in(&data, config.length_size)
                        .ok_or_else(|| anyhow!("The HEVC image data is corrupt"))?;
                    *read.entry(key).or_insert(largest)
                }
            };
            total += count * in_data.max(configured(config).unwrap_or(0));
            ensure!(coded_fits(std::iter::once(total)), too_large());
        }
        Ok(())
    }

    /// The whole file, from the open file the header was read from, for a
    /// system decoder that reads it from memory.
    #[cfg(any(target_os = "macos", all(unix, not(target_vendor = "apple"))))]
    fn bytes(&mut self) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut bytes = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;
        self.file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    /// Whether a picture of `width` × `height` is this image: the whole
    /// picture, or its crop alone. A system decoder's size is checked so
    /// before anything that size is allocated.
    #[cfg_attr(not(any(unix, windows)), allow(dead_code))]
    fn expects(&self, width: u32, height: u32) -> bool {
        (width, height) == (self.layout.width, self.layout.height)
            || (width, height) == (self.crop.width, self.crop.height)
    }
}

/// An `hvc1` item's HEVC configuration (`hvcC`) and size; the item may be a
/// grid's `tile`, which may not be cropped or turned itself.
fn picture(heif: &Heif, id: u32, tile: bool) -> Result<(&[u8], (u32, u32))> {
    let item = heif
        .item(id)
        .ok_or_else(|| anyhow!("The HEIF image is missing a tile"))?;
    ensure!(&item.kind == b"hvc1", "The HEIF image is not HEVC");
    if let Some(kind) = heif.unread_essential(id) {
        let kind = String::from_utf8_lossy(&kind);
        bail!("The HEIF image needs `{kind}`, which RAWmakase can't read");
    }
    let props = heif.properties(id);
    let transformed = props.iter().any(|p| {
        matches!(
            p,
            Property::CleanAperture(_) | Property::Rotation(_) | Property::Mirror(_)
        )
    });
    ensure!(
        !(tile && transformed),
        "The HEIF image's tiles are cropped or turned, which RAWmakase can't read"
    );
    let size = props
        .iter()
        .find_map(|p| match p {
            Property::Size(w, h) => Some((*w, *h)),
            _ => None,
        })
        .filter(|(w, h)| *w > 0 && *h > 0)
        .ok_or_else(|| anyhow!("The HEIF image has no size"))?;
    let hvcc = props
        .iter()
        .find_map(|p| match p {
            Property::HevcConfig(c) => Some(c.as_slice()),
            _ => None,
        })
        .ok_or_else(|| anyhow!("The HEIF image has no HEVC configuration"))?;
    Ok((hvcc, size))
}

fn config(hvcc: &[u8]) -> Result<hevc::Config> {
    hevc::Config::read(hvcc)
        .ok_or_else(|| anyhow!("The HEIF image's HEVC configuration can't be read"))
}

/// The pictures item `id` is decoded from: itself, an `hvc1`, or a grid's
/// tiles.
fn layout_of(heif: &Heif, file: &mut std::fs::File, id: u32) -> Result<Layout> {
    match heif.item(id).map(|i| &i.kind) {
        Some(b"grid") => grid(heif, file, id),
        _ => {
            let (hvcc, (width, height)) = picture(heif, id, false)?;
            Ok(Layout {
                width,
                height,
                tiles: vec![(id, Arc::new(config(hvcc)?))],
                tile: (width, height),
            })
        }
    }
}

/// A `grid` item (ISO/IEC 23008-12 6.6.2.3): its size, and its tiles, all of
/// one size, which cover it without a row or column left over.
fn grid(heif: &Heif, file: &mut std::fs::File, id: u32) -> Result<Layout> {
    let bad = || anyhow!("The HEIF image's grid can't be read");
    let data = heif.data_up_to(file, id, 64).ok_or_else(bad)?;
    let (rows, columns) = match data.get(..4) {
        Some([0, _, rows, columns]) => (u32::from(*rows) + 1, u32::from(*columns) + 1),
        _ => return Err(bad()),
    };
    let (width, height) = if data[1] & 1 == 0 {
        let size = data.get(4..8).ok_or_else(bad)?;
        (
            u32::from(u16::from_be_bytes([size[0], size[1]])),
            u32::from(u16::from_be_bytes([size[2], size[3]])),
        )
    } else {
        let size = data.get(4..12).ok_or_else(bad)?;
        (
            u32::from_be_bytes(size[..4].try_into()?),
            u32::from_be_bytes(size[4..].try_into()?),
        )
    };
    let ids = heif.referenced(*b"dimg", id);
    ensure!(
        ids.len() as u32 == rows * columns,
        "The HEIF image's grid has {} tiles, not {}",
        ids.len(),
        rows * columns
    );
    // A grid may list a tile more than once, and its tiles share one
    // configuration as a rule: each tile and each configuration is read once.
    let (mut read, mut configs) = (HashMap::new(), HashMap::new());
    for id in &ids {
        if !read.contains_key(id) {
            let (hvcc, size) = picture(heif, *id, true)?;
            let parsed = match configs.get(hvcc) {
                Some(parsed) => Arc::clone(parsed),
                None => Arc::clone(configs.entry(hvcc).or_insert(Arc::new(config(hvcc)?))),
            };
            read.insert(*id, (parsed, size));
        }
    }
    let tile = read[&ids[0]].1;
    ensure!(
        read.values().all(|(_, size)| *size == tile),
        "The HEIF image's tiles differ in size"
    );
    let covers = |count: u32, size: u32, full: u32| {
        full > 0
            && u64::from(count) * u64::from(size) >= u64::from(full)
            && u64::from(count - 1) * u64::from(size) < u64::from(full)
    };
    ensure!(
        covers(columns, tile.0, width) && covers(rows, tile.1, height),
        "The HEIF image's tiles don't cover it"
    );
    Ok(Layout {
        width,
        height,
        tiles: ids
            .iter()
            .map(|id| (*id, Arc::clone(&read[id].0)))
            .collect(),
        tile,
    })
}

/// The part of a `width` × `height` picture a `clap` keeps: its size rounded,
/// its left and top edges rounded down, as libheif crops; the whole picture
/// without one.
fn crop(props: &[&Property], width: u32, height: u32) -> Result<Crop> {
    let Some(clap) = props.iter().find_map(|p| match p {
        Property::CleanAperture(c) => Some(*c),
        _ => None,
    }) else {
        return Ok(Crop {
            left: 0,
            top: 0,
            width,
            height,
        });
    };
    let round = |(n, d): (u32, u32)| (i128::from(n) + i128::from(d) / 2) / i128::from(d);
    // The centre's offset, plus half of what the crop leaves out:
    // offset + (full - size) / 2, over the denominator 2 × od × sd.
    let edge = |(on, od): (i32, u32), full: u32, (sn, sd): (u32, u32)| {
        let (on, od, full) = (i128::from(on), i128::from(od), i128::from(full));
        let (sn, sd) = (i128::from(sn), i128::from(sd));
        (2 * on * sd + full * od * sd - sn * od).div_euclid(2 * od * sd)
    };
    let (w, h) = (round(clap.width), round(clap.height));
    let left = edge(clap.horizontal_offset, width, clap.width);
    let top = edge(clap.vertical_offset, height, clap.height);
    ensure!(
        w > 0
            && h > 0
            && left >= 0
            && top >= 0
            && left + w <= width.into()
            && top + h <= height.into(),
        "The HEIF image's crop is outside it"
    );
    // Each is within `width` or `height`, so fits.
    Ok(Crop {
        left: left as u32,
        top: top as u32,
        width: w as u32,
        height: h as u32,
    })
}

impl ImageDecoder for HeicFile {
    fn dimensions(&self) -> (u32, u32) {
        (self.crop.width, self.crop.height)
    }
    fn color_type(&self) -> image::ColorType {
        if self.deep {
            image::ColorType::Rgb16
        } else {
            image::ColorType::Rgb8
        }
    }
    fn icc_profile(&mut self) -> image::ImageResult<Option<Vec<u8>>> {
        Ok(heif::profile(self.icc.as_ref(), self.nclx))
    }
    fn orientation(&mut self) -> image::ImageResult<Orientation> {
        Ok(self.orientation)
    }
    fn read_image(mut self, buf: &mut [u8]) -> image::ImageResult<()> {
        let failed = |e: anyhow::Error| {
            image::ImageError::Decoding(image::error::DecodingError::new(
                image::error::ImageFormatHint::Name("HEIF".into()),
                e.to_string(),
            ))
        };
        let (crop, deep) = (self.crop, self.deep);
        let full = (self.layout.width, self.layout.height);
        self.stored().map_err(failed)?;
        self.coded_within().map_err(failed)?;
        let Pixels { width, height, rgb } = pixels(self).map_err(failed)?;
        // The whole picture, cropped here, or the crop alone: any other size
        // is not the image the header describes (a system decoder that turned
        // it, say), and is not shown.
        let (left, top) = if (width, height) == full {
            (crop.left, crop.top)
        } else if (width, height) == (crop.width, crop.height) {
            (0, 0)
        } else {
            return Err(failed(anyhow!("The HEIF image is not the size it says")));
        };
        // Bytes per pixel: three samples of one or two bytes.
        let pixel = if deep { 6 } else { 3 };
        let (row, kept) = (width as usize * pixel, crop.width as usize * pixel);
        if kept * crop.height as usize != buf.len() || rgb.len() != row * height as usize {
            return Err(failed(anyhow!("The HEIF image is not the size it says")));
        }
        for (y, out) in buf.chunks_exact_mut(kept).enumerate() {
            let start = (top as usize + y) * row + left as usize * pixel;
            out.copy_from_slice(&rgb[start..start + kept]);
        }
        Ok(())
    }
    fn read_image_boxed(self: Box<Self>, buf: &mut [u8]) -> image::ImageResult<()> {
        (*self).read_image(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawmakase_interop::isobmff::CleanAperture;

    fn clap(width: (u32, u32), height: (u32, u32), h: (i32, u32), v: (i32, u32)) -> Property {
        Property::CleanAperture(CleanAperture {
            width,
            height,
            horizontal_offset: h,
            vertical_offset: v,
        })
    }

    #[test]
    fn a_clean_aperture_crops_around_its_centre() {
        let crop_of = |p: Property, w, h| crop(&[&p], w, h);
        // libheif's 16x12 coded as 64x64: centred, offset half the difference.
        assert_eq!(
            crop_of(clap((16, 1), (12, 1), (-24, 1), (-26, 1)), 64, 64).unwrap(),
            Crop {
                left: 0,
                top: 0,
                width: 16,
                height: 12
            }
        );
        // avifenc's --crop 2,2,8,8 of 16x12: a centre 2 left of the picture's.
        assert_eq!(
            crop_of(clap((8, 1), (8, 1), (-4, 2), (0, 1)), 16, 12).unwrap(),
            Crop {
                left: 2,
                top: 2,
                width: 8,
                height: 8
            }
        );
        // An odd difference: the left edge rounds down.
        assert_eq!(
            crop_of(clap((3, 1), (4, 1), (0, 1), (0, 1)), 4, 4)
                .unwrap()
                .left,
            0
        );
        // Without a clap, the whole picture.
        assert_eq!(
            crop(&[], 5, 7).unwrap(),
            Crop {
                left: 0,
                top: 0,
                width: 5,
                height: 7
            }
        );
    }

    /// The tiles, decoded whole, are held to 150 MP, without overflowing
    /// however large a tile says it is.
    #[test]
    fn tiles_are_held_to_150_megapixels() {
        let tiles = |count: usize, tile: (u32, u32)| Layout {
            width: 0,
            height: 0,
            tiles: vec![(1, Arc::new(config_of(&[]))); count],
            tile,
        };
        let fits = |layouts: &[Layout]| within_limit(layouts);
        assert!(fits(&[tiles(192, (512, 512))]));
        assert!(fits(&[tiles(1, (12_247, 12_247))]));
        assert!(!fits(&[tiles(2, (8_661, 8_661))]));
        assert!(!fits(&[tiles(4, (1 << 31, 1 << 31))]));
        assert!(!fits(&[tiles(65_536, (u32::MAX, u32::MAX))]));
        // An alpha counts with its image.
        assert!(fits(&[tiles(1, (8_661, 8_661))]));
        assert!(!fits(&[tiles(1, (8_661, 8_661)), tiles(1, (8_661, 8_661))]));
    }

    fn config_of(coded: &[(u32, u32)]) -> hevc::Config {
        hevc::Config {
            depth: 8,
            chroma: 1,
            length_size: 4,
            coded: coded.to_vec(),
        }
    }

    #[test]
    fn pictures_are_held_to_150_megapixels_as_coded() {
        let fits = |sizes: &[(u32, u32)]| coded_fits(sizes.iter().map(|s| hevc::area(*s)));
        // Apple's encoder codes a 64×64 picture as 160×64.
        assert!(fits(&[(160, 64)]));
        assert!(fits(&[(12_247, 12_247)]));
        assert!(!fits(&[(12_248, 12_248)]));
        // A tile counts as often as it is placed.
        assert!(fits(&[(8_661, 8_661)]));
        assert!(!fits(&[(8_661, 8_661); 2]));
        assert!(!fits(&[(u32::MAX, u32::MAX); 4096]));
        // No SPS: nothing the system's decoder can decode.
        assert!(fits(&[(0, 0)]));
    }

    /// A box of `kind` around `body`.
    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = (8 + body.len() as u32).to_be_bytes().to_vec();
        out.extend(kind);
        out.extend(body);
        out
    }

    /// A full box: version 0 and no flags before `body`.
    fn full(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        bx(kind, &[&[0, 0, 0, 0], body].concat())
    }

    /// An `hvc1` item of a HEIC: its id, size, `auxC` type when it is an
    /// auxiliary image, and its data as extents of the `mdat`'s bytes
    /// (offset, length).
    struct Picture {
        id: u16,
        size: (u32, u32),
        auxiliary: Option<&'static str>,
        extents: Vec<(u32, u32)>,
    }

    /// A HEIC of `pictures`, the first the primary image and each other one
    /// its alpha (an `auxl` reference to it), all with `hvcc`, and `mdat`.
    fn heic(pictures: &[Picture], hvcc: &[u8], mdat: &[u8]) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
        let pitm = full(b"pitm", &pictures[0].id.to_be_bytes());
        let mut iinf = (pictures.len() as u16).to_be_bytes().to_vec();
        for p in pictures {
            let infe = [
                &[2, 0, 0, 0][..],
                &p.id.to_be_bytes(),
                &[0, 0],
                b"hvc1",
                &[0],
            ]
            .concat();
            iinf.extend(bx(b"infe", &infe));
        }
        // Properties: hvcC, then each picture's ispe, then each auxC.
        let mut ipco = bx(b"hvcC", hvcc);
        let mut ipma = (pictures.len() as u32).to_be_bytes().to_vec();
        for (i, p) in pictures.iter().enumerate() {
            let size = [p.size.0.to_be_bytes(), p.size.1.to_be_bytes()].concat();
            ipco.extend(full(b"ispe", &size));
            let mut associated = vec![0x81, 2 + i as u8];
            if let Some(kind) = p.auxiliary {
                ipco.extend(full(b"auxC", &[kind.as_bytes(), &[0]].concat()));
                associated.push(1 + pictures.len() as u8 + i as u8);
            }
            ipma.extend(p.id.to_be_bytes());
            ipma.push(associated.len() as u8);
            ipma.extend(associated);
        }
        let iprp = bx(
            b"iprp",
            &[bx(b"ipco", &ipco), full(b"ipma", &ipma)].concat(),
        );
        let mut iref = Vec::new();
        for p in &pictures[1..] {
            let to = [p.id.to_be_bytes(), [0, 1], pictures[0].id.to_be_bytes()].concat();
            iref.extend(bx(b"auxl", &to));
        }
        let iref = full(b"iref", &iref);
        // iloc version 1, 4-byte offsets and lengths, offsets from the
        // file's start: the mdat comes after the meta box, whose size the
        // offsets don't change.
        let iloc = |at: u32| {
            let mut body = vec![1, 0, 0, 0, 0x44, 0x00];
            body.extend((pictures.len() as u16).to_be_bytes());
            for p in pictures {
                body.extend(p.id.to_be_bytes());
                body.extend([0, 0, 0, 0]);
                body.extend((p.extents.len() as u16).to_be_bytes());
                for (offset, length) in &p.extents {
                    body.extend((at + offset).to_be_bytes());
                    body.extend(length.to_be_bytes());
                }
            }
            bx(b"iloc", &body)
        };
        let meta = |at| {
            let boxes = [
                pitm.clone(),
                full(b"iinf", &iinf),
                iprp.clone(),
                iref.clone(),
                iloc(at),
            ];
            full(b"meta", &boxes.concat())
        };
        let at = (ftyp.len() + meta(0).len() + 8) as u32;
        [ftyp, meta(at), bx(b"mdat", mdat)].concat()
    }

    /// metadata.heic's HEVC configuration and its picture's data.
    fn coded() -> (Vec<u8>, Vec<u8>) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/formats/metadata.heic");
        let mut file = std::fs::File::open(path).unwrap();
        let heif = Heif::read(&mut file).unwrap();
        let hvcc = heif
            .properties(heif.primary)
            .into_iter()
            .find_map(|p| match p {
                Property::HevcConfig(c) => Some(c.clone()),
                _ => None,
            })
            .unwrap();
        (hvcc, heif.data(&mut file, heif.primary).unwrap())
    }

    const ALPHA: &str = "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha";

    /// A system decoder decodes an image's alpha with it, so the alpha is
    /// held to the 150 MP its image is, together with it: an alpha of 150 MP
    /// alone would fit, but not with its 64×64 image.
    #[test]
    fn an_alpha_image_counts_towards_150_megapixels() {
        let (hvcc, data) = coded();
        let extents = vec![(0, data.len() as u32)];
        let file = heic(
            &[
                Picture {
                    id: 1,
                    size: (64, 64),
                    auxiliary: None,
                    extents: extents.clone(),
                },
                Picture {
                    id: 2,
                    size: (10_000, 15_000),
                    auxiliary: Some(ALPHA),
                    extents,
                },
            ],
            &hvcc,
            &data,
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge-alpha.heic");
        std::fs::write(&path, file).unwrap();
        let error = HeicFile::open(&path).err().map(|e| e.to_string());
        assert_eq!(error.as_deref(), Some("Image too large to preview"));
    }

    /// libheif writes its alpha as an `auxl` image of MPEG's alpha type.
    #[test]
    fn a_heics_alpha_is_among_the_pictures_decoded() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/formats/half-alpha.heic");
        assert_eq!(HeicFile::open(&path).unwrap().pictures.len(), 2);
    }

    /// An alpha whose data is cut off is refused before any decoder reads
    /// it, as the image's own would be.
    #[test]
    fn an_alpha_image_cut_off_is_refused_before_decoding() {
        let (hvcc, data) = coded();
        let len = data.len() as u32;
        let file = heic(
            &[
                Picture {
                    id: 1,
                    size: (64, 64),
                    auxiliary: None,
                    extents: vec![(0, len)],
                },
                Picture {
                    id: 2,
                    size: (64, 64),
                    auxiliary: Some(ALPHA),
                    extents: vec![(len, len)],
                },
            ],
            &hvcc,
            &data,
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut-alpha.heic");
        std::fs::write(&path, file).unwrap();
        let mut heic = HeicFile::open(&path).unwrap();
        let error = heic.stored().unwrap_err().to_string();
        assert!(error.contains("missing or cut off"), "{error}");
    }

    /// Pictures may share their stored data, as a grid's identical tiles may:
    /// it is read, and counted against the file's size, once.
    #[test]
    fn pictures_sharing_their_data_are_read_once() {
        let (hvcc, mut data) = coded();
        // A filler NAL unit (type 38) makes the data most of the file.
        data.extend(4002u32.to_be_bytes());
        data.extend([38 << 1, 1]);
        data.extend([0xff; 4000]);
        let extents = vec![(0, data.len() as u32)];
        let file = heic(
            &[
                Picture {
                    id: 1,
                    size: (64, 64),
                    auxiliary: None,
                    extents: extents.clone(),
                },
                Picture {
                    id: 2,
                    size: (64, 64),
                    auxiliary: Some(ALPHA),
                    extents,
                },
            ],
            &hvcc,
            &data,
        );
        assert!(2 * data.len() > file.len());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.heic");
        std::fs::write(&path, file).unwrap();
        let mut heic = HeicFile::open(&path).unwrap();
        heic.stored().unwrap();
        heic.coded_within().unwrap();
    }

    /// Extents that repeat one stretch of the file make an item larger than
    /// the file: refused once more than the file is read, not after reading
    /// gigabytes.
    #[test]
    fn a_picture_whose_data_repeats_past_the_files_size_is_refused() {
        let (hvcc, data) = coded();
        let extents = vec![(0, data.len() as u32); 4096];
        let file = heic(
            &[Picture {
                id: 1,
                size: (64, 64),
                auxiliary: None,
                extents,
            }],
            &hvcc,
            &data,
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repeats.heic");
        std::fs::write(&path, file).unwrap();
        let mut heic = HeicFile::open(&path).unwrap();
        let error = heic.coded_within().unwrap_err().to_string();
        assert!(error.contains("larger than the file"), "{error}");
    }

    #[test]
    fn a_clean_aperture_outside_the_picture_is_refused() {
        for p in [
            clap((17, 1), (12, 1), (0, 1), (0, 1)),
            clap((8, 1), (8, 1), (5, 1), (0, 1)),
            clap((8, 1), (8, 1), (0, 1), (-3, 1)),
            clap((0, 1), (8, 1), (0, 1), (0, 1)),
        ] {
            assert!(crop(&[&p], 16, 12).is_err(), "{p:?}");
        }
    }
}
