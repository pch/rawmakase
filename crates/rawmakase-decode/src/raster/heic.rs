//! A HEIC (a HEIF of HEVC) still: our reading of its container
//! ([`rawmakase_interop::isobmff`]) here, its pixels decoded by the system's
//! HEVC decoder: macOS's ImageIO (`imageio`), Windows Imaging Component
//! (`wic`). The header here says the size, crop, colour and orientation, and
//! what Develop refuses.
//!
//! RAWmakase has no HEVC decoder of its own, as HEVC is patented. Elsewhere,
//! as on Linux, the header is still read, so the photo is listed with its size
//! and metadata, but its pixels are refused.
//!
//! A phone's photo is a grid (`grid`) of tiles, 512×512 on an iPhone, each its
//! own HEVC picture. A clean aperture (`clap`) crops the picture, before its
//! rotation and mirror, the order MIAF requires.
//!
//! The colour comes from the `colr` boxes of the image, or of its first tile
//! when the image has none of that kind (an iPhone's tiles carry the `nclx`):
//! an ICC profile, or one built from the CICP code points; a HEIC with neither
//! is taken as sRGB. The system decoder chooses the matrix and range itself.
//! Alpha, depth maps, gain maps and thumbnails are not read.
use super::avif::{self, Nclx};
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
/// No decoder on this system.
#[cfg(not(any(target_os = "macos", windows)))]
fn pixels(_: HeicFile) -> Result<Pixels> {
    bail!("RAWmakase shows HEIF images only on macOS and Windows, with their HEVC decoders")
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

/// Whether `count` tiles of `size` take no more than [`MAX_PIXELS`] decoded.
fn within_limit(count: usize, (w, h): (u32, u32)) -> bool {
    count as u128 * u128::from(w) * u128::from(h) <= u128::from(MAX_PIXELS)
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
    /// Where the file is, for a system decoder to open.
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    path: std::path::PathBuf,
    /// The file and its boxes, to read the pictures from when decoding.
    file: std::fs::File,
    heif: Heif,
    layout: Layout,
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
        let layout = match &primary.kind {
            b"hvc1" => {
                let (hvcc, (width, height)) = picture(&heif, heif.primary, false)?;
                Layout {
                    width,
                    height,
                    tiles: vec![(heif.primary, Arc::new(config(hvcc)?))],
                    tile: (width, height),
                }
            }
            b"grid" => grid(&heif, &mut file)?,
            _ => bail!("The HEIF image is not HEVC"),
        };
        // Every tile is decoded whole, so the tiles, which cover the image,
        // bound the work.
        ensure!(
            within_limit(layout.tiles.len(), layout.tile),
            "Image too large to preview"
        );
        let props = heif.properties(heif.primary);
        let crop = crop(&props, layout.width, layout.height)?;
        let (icc, nclx) = avif::colour(&props);
        let (tile_icc, tile_nclx) = avif::colour(&heif.properties(layout.tiles[0].0));
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
        // Without a system decoder, Develop says why from the header; with
        // one, the decoder decides what chroma and depth it reads.
        let refused = if cfg!(not(any(target_os = "macos", windows))) {
            Some(Unsupported("HEIF".into()))
        } else if nclx.is_some_and(Nclx::hdr) {
            Some(Unsupported("HDR".into()))
        } else if heif.brands.iter().any(|b| animated.contains(&b)) {
            Some(Unsupported("animated".into()))
        } else {
            None
        };
        Ok(Self {
            crop,
            deep: depth > 8,
            path: path.to_path_buf(),
            file,
            orientation: avif::transforms(&props),
            heif,
            layout,
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
        for (id, _) in &self.layout.tiles {
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
        // Each distinct tile's largest coded area, read once.
        let mut largest: HashMap<u32, u128> = HashMap::new();
        for (id, config) in &self.layout.tiles {
            if largest.contains_key(id) {
                continue;
            }
            let data = self
                .heif
                .data(&mut self.file, *id)
                .ok_or_else(|| anyhow!("The HEIF image data is missing or cut off"))?;
            let in_data = hevc::largest_coded_in(&data, config.length_size)
                .ok_or_else(|| anyhow!("The HEVC image data is corrupt"))?;
            let in_config = config.coded.iter().map(|s| hevc::area(*s)).max();
            largest.insert(*id, in_data.max(in_config.unwrap_or(0)));
        }
        ensure!(
            coded_fits(self.layout.tiles.iter().map(|(id, _)| largest[id])),
            "The HEVC image is coded at more than 150 megapixels"
        );
        Ok(())
    }

    /// Whether a picture of `width` × `height` is this image: the whole
    /// picture, or its crop alone. A system decoder's size is checked so
    /// before anything that size is allocated.
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
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

/// A `grid` item (ISO/IEC 23008-12 6.6.2.3): its size, and its tiles, all of
/// one size, which cover it without a row or column left over.
fn grid(heif: &Heif, file: &mut std::fs::File) -> Result<Layout> {
    let bad = || anyhow!("The HEIF image's grid can't be read");
    let data = heif.data_up_to(file, heif.primary, 64).ok_or_else(bad)?;
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
    let ids = heif.referenced(*b"dimg", heif.primary);
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
        Ok(avif::profile(self.icc.as_ref(), self.nclx))
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
        assert!(within_limit(192, (512, 512)));
        assert!(within_limit(1, (12_247, 12_247)));
        assert!(!within_limit(2, (8_661, 8_661)));
        assert!(!within_limit(4, (1 << 31, 1 << 31)));
        assert!(!within_limit(65_536, (u32::MAX, u32::MAX)));
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
