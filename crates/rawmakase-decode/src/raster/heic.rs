//! A HEIC (a HEIF of HEVC) still: our reading of its container
//! ([`rawmakase_interop::isobmff`]) here, which says the size, crop, colour
//! and orientation, and what Develop refuses; its pixels decoded by
//! RAWmakase's own HEVC decoder (`software`), the same on every platform.
//!
//! A build without the decoder (without the `software-hevc` feature, for
//! packagers who leave software HEVC decoders out) reads the header, so the
//! photo is listed with its size and metadata, but refuses its pixels.
//!
//! A phone's photo is a grid (`grid`) of tiles, 512×512 on an iPhone, each its
//! own HEVC picture. A clean aperture (`clap`) crops the picture, before its
//! rotation and mirror, the order MIAF requires.
//!
//! The colour comes from the `colr` boxes of the image, or of its first tile
//! when the image has none of that kind (an iPhone's tiles carry the `nclx`):
//! an ICC profile, or one built from the CICP code points; a HEIC with neither
//! is taken as sRGB. The matrix and range come from the `nclx` box; without
//! one they are taken as BT.601 and full range, what phones and libheif write
//! (the HEVC header's own are not read). Alpha, depth maps, gain maps and
//! thumbnails are not read.
use super::avif::{self, Nclx};
use super::{Unsupported, hevc};
use anyhow::{Result, anyhow, bail, ensure};
use image::ImageDecoder;
use image::metadata::Orientation;
use rawmakase_interop::isobmff::{Heif, Property};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[cfg(feature = "software-hevc")]
mod software;
#[cfg(feature = "software-hevc")]
use software::pixels;

/// No decoder in this build.
#[cfg(not(feature = "software-hevc"))]
fn pixels(_: HeicFile) -> Result<Pixels> {
    bail!("This build of RAWmakase has no HEVC decoder, so it can't show HEIF images")
}

/// The decoded picture, before its crop: `width` × `height` of interleaved
/// RGB, 8-bit samples or 16-bit ones (native endian), as [`HeicFile`]'s colour
/// type says, in the colour space its profile says.
struct Pixels {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

/// The most pixels decoded for an image, each of its tiles whole, as many as
/// `prepare` lets an image have.
const MAX_PIXELS: u64 = 150_000_000;

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
    /// The pictures, row by row, `columns` to a row: each item and its HEVC
    /// configuration, one for each item a grid lists however often.
    tiles: Vec<(u32, Arc<hevc::Config>)>,
    #[cfg_attr(not(feature = "software-hevc"), allow(dead_code))]
    columns: u32,
    /// Each picture's size.
    tile: (u32, u32),
}

pub(super) struct HeicFile {
    crop: Crop,
    /// More than 8 bits per sample, from `hvcC`.
    deep: bool,
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
                    columns: 1,
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
        // RAWmakase's decoder reads 4:2:0 and 4:4:4: the others are refused
        // from the header, so Develop says why, as for a tiled AVIF.
        let refused = if cfg!(not(feature = "software-hevc")) {
            Some(Unsupported("HEIF".into()))
        } else if chroma == 0 {
            Some(Unsupported("monochrome HEIF".into()))
        } else if chroma == 2 {
            Some(Unsupported("4:2:2 HEIF".into()))
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

    /// Whether every picture's data is in the file, before any is decoded: a
    /// grid cut short is refused at once, not after its first tiles.
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
        columns,
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
        let Pixels { width, height, rgb } = pixels(self).map_err(failed)?;
        if (width, height) != full {
            return Err(failed(anyhow!("The HEIF image is not the size it says")));
        }
        // Bytes per pixel: three samples of one or two bytes.
        let pixel = if deep { 6 } else { 3 };
        let (row, kept) = (width as usize * pixel, crop.width as usize * pixel);
        if kept * crop.height as usize != buf.len() || rgb.len() != row * height as usize {
            return Err(failed(anyhow!("The HEIF image is not the size it says")));
        }
        for (y, out) in buf.chunks_exact_mut(kept).enumerate() {
            let start = (crop.top as usize + y) * row + crop.left as usize * pixel;
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
