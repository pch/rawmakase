//! A HEIC's pixels decoded in pure Rust: each HEVC picture by rust_h265
//! ([`super::super::hevc`]) and turned into RGB with the matrix and range the
//! file says.
//!
//! A grid's tiles are decoded in parallel, a few at a time so a large grid
//! never holds them all, and laid into one picture before it is turned into
//! RGB, so subsampled chroma is interpolated across their edges as within
//! them: bilinearly, as an AVIF's is (libheif repeats each sample instead, so
//! the two differ along sharp colour edges, and elsewhere agree to within one
//! level).
use super::super::avif::{self, Nclx};
use super::super::hevc;
use super::{HeicFile, Layout, Pixels};
use anyhow::{Result, anyhow, bail, ensure};
use rayon::prelude::*;
use std::sync::Arc;
use yuv::{YuvPlanarImage, YuvRange};

/// The most bytes a picture of `width` × `height` is read from: HEVC codes
/// one in fewer than its samples take at 16 bits, with room for the SEI an
/// encoder adds. More is not a picture of that size, and is not read.
fn coded_budget(width: u32, height: u32) -> u64 {
    (u64::from(width) * u64::from(height))
        .saturating_mul(6)
        .saturating_add(64 << 10)
}

/// The whole picture `file` holds, before its crop.
pub(super) fn pixels(file: HeicFile) -> Result<Pixels> {
    let (deep, nclx) = (file.deep, file.nclx);
    let picture = decode(file)?;
    ensure!(
        (picture.depth > 8) == deep,
        "The HEIF image is not the depth it says"
    );
    Ok(Pixels {
        width: picture.width,
        height: picture.height,
        rgb: to_rgb(&picture, nclx)?,
    })
}

/// The whole picture: each tile decoded and laid in its place.
fn decode(mut file: HeicFile) -> Result<hevc::Picture> {
    let Layout {
        width,
        height,
        tiles,
        columns,
        tile,
    } = &file.layout;
    let mut canvas: Option<hevc::Picture> = None;
    let batch = rayon::current_num_threads().max(1);
    // No more than the file holds either, however its extents overlap.
    let budget = coded_budget(tile.0, tile.1).min(file.file.metadata()?.len());
    // A tile listed more than once is decoded once, and laid at each of
    // its places.
    let mut distinct: Vec<(u32, &Arc<hevc::Config>, Vec<u32>)> = Vec::new();
    let mut index = std::collections::HashMap::new();
    for (at, (id, config)) in tiles.iter().enumerate() {
        let i = *index.entry(*id).or_insert_with(|| {
            distinct.push((*id, config, Vec::new()));
            distinct.len() - 1
        });
        distinct[i].2.push(at as u32);
    }
    for chunk in distinct.chunks(batch) {
        let data = chunk
            .iter()
            .map(|(id, _, _)| {
                file.heif
                    .data_up_to(&mut file.file, *id, budget)
                    .ok_or_else(|| anyhow!("The HEIF image data is missing, cut off or too large"))
            })
            .collect::<Result<Vec<_>>>()?;
        let pictures: Vec<_> = chunk
            .par_iter()
            .zip(&data)
            .map(|((_, config, _), data)| hevc::decode(config, data, *tile))
            .collect();
        for ((_, _, places), picture) in chunk.iter().zip(pictures) {
            let picture = picture?;
            ensure!(
                (picture.width, picture.height) == *tile,
                "The HEIF image is not the size it says"
            );
            if tiles.len() == 1 {
                return Ok(picture);
            }
            let canvas = canvas.get_or_insert_with(|| blank(&picture, *width, *height));
            for at in places {
                place(
                    canvas,
                    &picture,
                    at % columns * tile.0,
                    at / columns * tile.1,
                )?;
            }
        }
    }
    canvas.ok_or_else(|| anyhow!("The HEIF image has no tiles"))
}

/// A `width` × `height` picture of `like`'s depth and chroma, all zero.
fn blank(like: &hevc::Picture, width: u32, height: u32) -> hevc::Picture {
    let mut picture = hevc::Picture {
        width,
        height,
        depth: like.depth,
        subsampled: like.subsampled,
        planes: hevc::Planes::Bytes(Default::default()),
    };
    let luma = width as usize * height as usize;
    let chroma = picture.chroma_width() as usize * picture.chroma_height() as usize;
    let sizes = [luma, chroma, chroma];
    picture.planes = match like.planes {
        hevc::Planes::Bytes(_) => hevc::Planes::Bytes(sizes.map(|n| vec![0; n])),
        hevc::Planes::Words(_) => hevc::Planes::Words(sizes.map(|n| vec![0; n])),
    };
    picture
}

/// `tile` copied into `canvas` with its top left corner at (`x`, `y`), as
/// much of it as fits.
fn place(canvas: &mut hevc::Picture, tile: &hevc::Picture, x: u32, y: u32) -> Result<()> {
    ensure!(
        (tile.depth, tile.subsampled) == (canvas.depth, canvas.subsampled),
        "The HEIF image's tiles differ in depth or chroma"
    );
    // A subsampled tile's chroma starts at an even pixel.
    ensure!(
        !canvas.subsampled || (x.is_multiple_of(2) && y.is_multiple_of(2)),
        "The HEIF image's tiles are an odd size"
    );
    let (cx, cy) = if canvas.subsampled {
        (x / 2, y / 2)
    } else {
        (x, y)
    };
    // The tile's width and height, the canvas's, and where the tile goes: of
    // the luma plane, then of the chroma ones.
    let sizes = [
        [tile.width, tile.height, canvas.width, canvas.height, x, y],
        [
            tile.chroma_width(),
            tile.chroma_height(),
            canvas.chroma_width(),
            canvas.chroma_height(),
            cx,
            cy,
        ],
    ]
    .map(|s| s.map(|v| v as usize));
    fn copy<T: Copy>(to: &mut [T], from: &[T], [w, h, cw, ch, x, y]: [usize; 6]) {
        let (cols, rows) = (w.min(cw.saturating_sub(x)), h.min(ch.saturating_sub(y)));
        for row in 0..rows {
            to[(y + row) * cw + x..][..cols].copy_from_slice(&from[row * w..][..cols]);
        }
    }
    match (&mut canvas.planes, &tile.planes) {
        (hevc::Planes::Bytes(to), hevc::Planes::Bytes(from)) => {
            for (plane, (to, from)) in to.iter_mut().zip(from).enumerate() {
                copy(to, from, sizes[plane.min(1)]);
            }
        }
        (hevc::Planes::Words(to), hevc::Planes::Words(from)) => {
            for (plane, (to, from)) in to.iter_mut().zip(from).enumerate() {
                copy(to, from, sizes[plane.min(1)]);
            }
        }
        _ => bail!("The HEIF image's tiles differ in depth"),
    }
    Ok(())
}

/// The picture's pixels as interleaved RGB: 8-bit samples, or 16-bit ones
/// (native endian) scaled up from its depth.
fn to_rgb(picture: &hevc::Picture, nclx: Option<Nclx>) -> Result<Vec<u8>> {
    let (code, full) = match nclx {
        Some(n) => (u8::try_from(n.matrix).unwrap_or(2), n.full_range),
        None => (6, true),
    };
    // BT.2020 constant luminance (10) and chroma-derived constant luminance
    // (13) need a reconstruction no matrix gives.
    ensure!(
        !matches!(code, 10 | 13),
        "HEIF images in constant luminance can't be read"
    );
    let (w, h) = (picture.width, picture.height);
    let (cw, depth) = (picture.chroma_width(), picture.depth);
    // Identity (GBR): the planes are green, blue and red.
    if code == 0 {
        ensure!(!picture.subsampled, "A GBR HEIF image must be 4:4:4");
        return Ok(match &picture.planes {
            hevc::Planes::Bytes([g, b, r]) => interleave(r, g, b),
            hevc::Planes::Words([g, b, r]) => avif::widen(&interleave(r, g, b), depth),
        });
    }
    let range = if full {
        YuvRange::Full
    } else {
        YuvRange::Limited
    };
    let m = avif::matrix(code);
    let stride = w * 3;
    let failed = |e: yuv::YuvError| anyhow!("Could not convert the HEIF image: {e:?}");
    fn planar<'a, T: Copy + std::fmt::Debug>(
        planes: &'a [Vec<T>; 3],
        w: u32,
        h: u32,
        cw: u32,
    ) -> YuvPlanarImage<'a, T> {
        YuvPlanarImage {
            y_plane: &planes[0],
            y_stride: w,
            u_plane: &planes[1],
            u_stride: cw,
            v_plane: &planes[2],
            v_stride: cw,
            width: w,
            height: h,
        }
    }
    match &picture.planes {
        hevc::Planes::Bytes(planes) => {
            let mut rgb = vec![0u8; stride as usize * h as usize];
            let image = planar(planes, w, h, cw);
            if picture.subsampled {
                yuv::yuv420_to_rgb_bilinear(&image, &mut rgb, stride, range, m)
            } else {
                yuv::yuv444_to_rgb(&image, &mut rgb, stride, range, m)
            }
            .map_err(failed)?;
            Ok(rgb)
        }
        hevc::Planes::Words(planes) => {
            let mut rgb = vec![0u16; stride as usize * h as usize];
            let image = planar(planes, w, h, cw);
            match (depth, picture.subsampled) {
                (10 | 12, true) => avif::deep420_to_rgb(&image, depth, &mut rgb, range, m),
                (10, false) => yuv::i410_to_rgb10(&image, &mut rgb, stride, range, m),
                (12, false) => yuv::i412_to_rgb12(&image, &mut rgb, stride, range, m),
                _ => bail!("{depth}-bit HEIF images are not supported"),
            }
            .map_err(failed)?;
            Ok(avif::widen(&rgb, depth))
        }
    }
}

/// Three planes as interleaved samples.
fn interleave<T: Copy>(r: &[T], g: &[T], b: &[T]) -> Vec<T> {
    r.iter()
        .zip(g)
        .zip(b)
        .flat_map(|((r, g), b)| [*r, *g, *b])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_coded_budget_saturates() {
        assert_eq!(coded_budget(u32::MAX, u32::MAX), u64::MAX);
    }

    /// A picture of `w` × `h` whose samples are `value`.
    fn flat(w: u32, h: u32, subsampled: bool, value: u8) -> hevc::Picture {
        let like = hevc::Picture {
            width: 0,
            height: 0,
            depth: 8,
            subsampled,
            planes: hevc::Planes::Bytes(Default::default()),
        };
        let mut picture = blank(&like, w, h);
        if let hevc::Planes::Bytes(planes) = &mut picture.planes {
            planes.iter_mut().for_each(|p| p.fill(value));
        }
        picture
    }

    #[test]
    fn tiles_are_laid_in_place_and_cut_at_the_edge() {
        let mut canvas = flat(6, 5, true, 0);
        place(&mut canvas, &flat(4, 4, true, 1), 0, 0).unwrap();
        place(&mut canvas, &flat(4, 4, true, 2), 4, 0).unwrap();
        place(&mut canvas, &flat(4, 4, true, 3), 0, 4).unwrap();
        place(&mut canvas, &flat(4, 4, true, 4), 4, 4).unwrap();
        let hevc::Planes::Bytes([y, u, _]) = &canvas.planes else {
            unreachable!()
        };
        assert_eq!(&y[..6], [1, 1, 1, 1, 2, 2]);
        assert_eq!(&y[24..], [3, 3, 3, 3, 4, 4]);
        // Chroma is 3 × 3.
        assert_eq!(u, &[1, 1, 2, 1, 1, 2, 3, 3, 4]);
        // A subsampled tile at an odd column is refused.
        assert!(place(&mut canvas, &flat(3, 4, true, 1), 3, 0).is_err());
        // As is a tile of other chroma.
        assert!(place(&mut canvas, &flat(4, 4, false, 1), 0, 0).is_err());
    }
}
