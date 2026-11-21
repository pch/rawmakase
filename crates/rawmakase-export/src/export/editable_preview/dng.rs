//! An editable preview's file: a DNG linear raw (PhotometricInterpretation
//! 34892) of three 16-bit samples a pixel in lossless JPEG tiles (Compression
//! 7, [`super::ljpeg`]), which RAWmakase's DNG reader opens as it opens any.
//! DNG has stored 16-bit integer raw data this way since its first version.
//! DNGBackwardVersion is 1.1, the lowest Adobe's own writer gives: from DNG
//! 1.1 on, readers take a difference of length 16 to have no bits after its
//! code, as T.81 says, where DNG 1.0 files wrote 16 (DNG 1.7.1, Appendix A,
//! Compatibility Issue 2).
//!
//! Measured on the 13 RAWs below at 2560 px, the files average 16.1 MB
//! (12.8–20.8 MB). JPEG XL (DNG 1.7's 52546) made them smaller, 13.0 MB on
//! average, but zune-jpegxl 0.5.2's 16-bit lossless encoder writes codestreams
//! no decoder can read (neither jxl-oxide nor libjxl) when a gradient runs into
//! a flat area, as a clipped sky does. Deflate (Compression 8, with Predictor
//! 2) averaged 17.3 MB, and DNG allows it only for floating-point, 32-bit
//! integer, transparency mask and depth map data, not 16-bit integer samples.
//!
//! Integer linear raw develops as `(sample − black) / (white − black)` times the
//! white balance, with no clamp at 1, so a WhiteLevel below 65535 keeps
//! highlights above white. [`WHITE`] keeps 2 stops: measured on 13 RAWs of 9
//! makes (pch/rawmakase#1022), none clipped renders at +4 or −3 EV, where no
//! headroom clipped the brightest ones and 4 stops started to show the loss of
//! precision under white. Floating-point samples would be clamped at white by
//! the reader, so they are not used.
//!
//! The standard tags carry what another DNG reader needs: the size, the color
//! matrix, white balance, orientation, the default crop, the basic EXIF and
//! Adobe's original-size tags (51089–51091). RAWmakase's own facts are in
//! DNGPrivateData ([`super::record`]), which
//! [`rawmakase_decode::editable_preview::open`] reads in their place.
use super::ljpeg;
use anyhow::{Result, ensure};
use rawmakase_cancel::Cancel;
use rawmakase_decode::editable_preview::PRIVATE_DATA;
use rawmakase_model::camera_data::{CameraImage, Metadata};
use rayon::prelude::*;

/// The sample the sensor's white is written as: 2 stops of headroom.
pub(super) const WHITE: u16 = 16383;
/// Tiles of 512 × 512 pixels, coded in parallel; the edge tiles are padded.
const TILE: u32 = 512;
/// TIFF's compression code for JPEG, lossless JPEG in DNG's raw IFDs.
const JPEG: u16 = 7;
/// PhotometricInterpretation's linear raw.
const LINEAR_RAW: u16 = 34892;
/// The XYZ (D65) to linear sRGB matrix, written as the color matrix for other
/// readers when the original's decoder had none: such a photo renders from its
/// fitted Camera Look in RAWmakase, which takes its facts from the record.
const XYZ_TO_SRGB: [[f32; 3]; 3] = [
    [3.240_454_2, -1.537_138_5, -0.498_531_4],
    [-0.969_266, 1.876_010_8, 0.041_556],
    [0.055_643_4, -0.204_025_9, 1.057_225_2],
];

/// A TIFF entry: tag, type, count and its value's bytes, little-endian.
struct Entry(u16, u16, u32, Vec<u8>);

fn shorts(tag: u16, v: &[u16]) -> Entry {
    Entry(
        tag,
        3,
        v.len() as u32,
        v.iter().flat_map(|v| v.to_le_bytes()).collect(),
    )
}
fn longs(tag: u16, v: &[u32]) -> Entry {
    Entry(
        tag,
        4,
        v.len() as u32,
        v.iter().flat_map(|v| v.to_le_bytes()).collect(),
    )
}
fn ascii(tag: u16, s: &str) -> Entry {
    let mut b: Vec<u8> = s.bytes().filter(|b| *b != 0).collect();
    b.push(0);
    Entry(tag, 2, b.len() as u32, b)
}
fn bytes(tag: u16, v: &[u8]) -> Entry {
    Entry(tag, 1, v.len() as u32, v.to_vec())
}
/// Unsigned rationals, each over the largest power of two up to 2^24 that keeps
/// its numerator within 32 bits: 1 in 16 million below 256, and a crop size or a
/// long focal length kept whole. Negative and non-finite values are 0.
fn rationals(tag: u16, v: &[f64]) -> Entry {
    let b = v
        .iter()
        .flat_map(|v| {
            let v = if v.is_finite() { v.max(0.) } else { 0. };
            let mut d = 1u32 << 24;
            while d > 1 && v * f64::from(d) > f64::from(u32::MAX) {
                d >>= 1;
            }
            let n = (v * f64::from(d)).round().min(f64::from(u32::MAX)) as u32;
            [n.to_le_bytes(), d.to_le_bytes()].concat()
        })
        .collect();
    Entry(tag, 5, v.len() as u32, b)
}
/// Signed rationals over 10^6.
fn srationals(tag: u16, v: &[f64]) -> Entry {
    let d = 1_000_000i32;
    let b = v
        .iter()
        .flat_map(|v| {
            let n = (v * f64::from(d))
                .round()
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
            [n.to_le_bytes(), d.to_le_bytes()].concat()
        })
        .collect();
    Entry(tag, 10, v.len() as u32, b)
}

/// The IFD of `entries` at byte `at` of the file, followed by the values that
/// don't fit in an entry; no next IFD.
fn ifd(mut entries: Vec<Entry>, at: u32) -> Result<Vec<u8>> {
    entries.sort_by_key(|e| e.0);
    let head = 2 + 12 * entries.len() + 4;
    let mut out = (entries.len() as u16).to_le_bytes().to_vec();
    let mut values = Vec::new();
    for Entry(tag, kind, count, value) in &entries {
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        if value.len() <= 4 {
            let mut v = value.clone();
            v.resize(4, 0);
            out.extend(v);
        } else {
            let offset = u32::try_from(at as usize + head + values.len())?;
            out.extend(offset.to_le_bytes());
            values.extend(value);
            if values.len() % 2 == 1 {
                values.push(0);
            }
        }
    }
    out.extend(0u32.to_le_bytes());
    out.extend(values);
    Ok(out)
}

/// TIFF's Orientation for RAWmakase's `flip`, the inverse of the DNG reader's.
fn orientation(flip: i32) -> u16 {
    match flip {
        1 => 2,
        3 => 3,
        2 => 4,
        4 => 5,
        6 => 6,
        7 => 7,
        5 => 8,
        _ => 1,
    }
}

/// The samples of one tile of `codes` (three samples a pixel, `width` pixels a
/// row): the pixels of the box at `x0, y0`, `w × h` pixels, at the top left of
/// a whole tile, three samples a pixel, row by row.
///
/// Every tile is TileWidth × TileLength, as TIFF requires: one at the right or
/// bottom edge is padded with its last column and row repeated, which readers
/// crop away, and which predict as well as the samples next to them.
fn padded(codes: &[u16], width: u32, [x0, y0, w, h]: [u32; 4]) -> Vec<u16> {
    let row_samples = (TILE * 3) as usize;
    let mut samples = Vec::with_capacity(row_samples * TILE as usize);
    for dy in 0..TILE {
        let y = y0 + dy.min(h - 1);
        let source = &codes[((y * width + x0) * 3) as usize..][..(w * 3) as usize];
        samples.extend_from_slice(source);
        let last = &source[((w - 1) * 3) as usize..];
        for _ in w..TILE {
            samples.extend_from_slice(last);
        }
    }
    samples
}

/// One tile of `codes`, padded ([`padded`]), as lossless JPEG.
fn tile(codes: &[u16], width: u32, b: [u32; 4]) -> Vec<u8> {
    ljpeg::encode(&padded(codes, width, b), TILE as u16, TILE as u16, 3)
}

/// The samples of `reduced`, a recovered image in its camera space, white
/// balanced by `wb` with 1 the sensor's white: each value divided by its white
/// balance (DNG's samples are before it), [`WHITE`] at white, rounded.
pub(super) fn samples(reduced: &CameraImage, wb: [f32; 3]) -> Vec<u16> {
    reduced
        .pixels
        .par_iter()
        .flat_map_iter(|p| {
            (0..3).map(move |c| (p[c] / wb[c] * f32::from(WHITE)).round().clamp(0., 65535.) as u16)
        })
        .collect()
}

/// The white balance the samples are divided by: the original's, kept away
/// from 0.
pub(super) fn white_balance(original: &Metadata) -> [f32; 3] {
    original.wb.map(|v| if v > 1e-6 { v } else { 1. })
}

/// The DNG of `reduced` (a recovered image of the photo `original` describes,
/// in its camera space, white balanced, 1 the sensor's white), with `private`
/// as its DNGPrivateData.
pub(super) fn file(
    reduced: &CameraImage,
    original: &Metadata,
    private: &[u8],
    cancel: Cancel<'_>,
) -> Result<Vec<u8>> {
    let (w, h) = (reduced.width, reduced.height);
    ensure!(
        w >= 2 && h >= 2 && reduced.pixels.len() == w as usize * h as usize,
        "A photo this small can't have an editable preview"
    );
    let wb = white_balance(original);
    let codes = samples(reduced, wb);
    let mut boxes = Vec::new();
    for y in (0..h).step_by(TILE as usize) {
        for x in (0..w).step_by(TILE as usize) {
            boxes.push([x, y, TILE.min(w - x), TILE.min(h - y)]);
        }
    }
    let tiles: Vec<Vec<u8>> = boxes
        .par_iter()
        .map(|b| {
            // Checked before each tile, so a cancelled build stops within one.
            ensure!(!cancel.is_cancelled(), "Editable preview cancelled");
            Ok(tile(&codes, w, *b))
        })
        .collect::<Result<_>>()?;
    // The header, then the tiles, the EXIF IFD and IFD0.
    let mut file = b"II*\0\0\0\0\0".to_vec();
    let mut offsets = Vec::new();
    for t in &tiles {
        offsets.push(u32::try_from(file.len())?);
        file.extend(t);
        if file.len() % 2 == 1 {
            file.push(0);
        }
    }
    let positive = |v: f32| f64::from(if v.is_finite() { v.max(0.) } else { 0. });
    let mut exif = vec![
        rationals(33434, &[positive(original.shutter)]),
        rationals(33437, &[positive(original.aperture)]),
        shorts(34855, &[original.iso.clamp(0., 65535.) as u16]),
        rationals(37386, &[positive(original.focal)]),
    ];
    if original.focal_35mm > 0. {
        exif.push(shorts(
            41989,
            &[original.focal_35mm.clamp(0., 65535.) as u16],
        ));
    }
    if !original.lens_model.is_empty() {
        exif.push(ascii(42036, &original.lens_model));
    }
    let exif_at = u32::try_from(file.len())?;
    file.extend(ifd(exif, exif_at)?);
    let ifd0_at = u32::try_from(file.len())?;
    let s = f64::from(w) / f64::from(original.width.max(1));
    // The default crop, when the decoder gave one inside the frame; the whole
    // frame otherwise, as DNG's defaults have it.
    let [left, top, crop_w, crop_h] = [
        original.crop_left,
        original.crop_top,
        original.crop_width,
        original.crop_height,
    ];
    let crop = if crop_w > 0
        && crop_h > 0
        && left
            .checked_add(crop_w)
            .is_some_and(|r| r <= original.width)
        && top
            .checked_add(crop_h)
            .is_some_and(|b| b <= original.height)
    {
        [left, top, crop_w, crop_h]
    } else {
        [0, 0, original.width, original.height]
    };
    let matrix = if original.cam_xyz.iter().flatten().any(|v| *v != 0.) {
        original.cam_xyz
    } else {
        XYZ_TO_SRGB
    };
    let mut entries = vec![
        longs(254, &[0]),
        longs(256, &[w]),
        longs(257, &[h]),
        shorts(258, &[16, 16, 16]),
        shorts(259, &[JPEG]),
        shorts(262, &[LINEAR_RAW]),
        ascii(271, &original.make),
        ascii(272, &original.model),
        shorts(274, &[orientation(original.flip)]),
        shorts(277, &[3]),
        shorts(284, &[1]),
        ascii(305, "RAWmakase"),
        longs(322, &[TILE]),
        longs(323, &[TILE]),
        longs(324, &offsets),
        longs(
            325,
            &tiles
                .iter()
                .map(|t| u32::try_from(t.len()))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        shorts(339, &[1, 1, 1]),
        longs(34665, &[exif_at]),
        // DNG 1.4 for the original-size tags, which earlier readers ignore;
        // readable since DNG 1.1, as the module's documentation says.
        bytes(50706, &[1, 4, 0, 0]),
        bytes(50707, &[1, 1, 0, 0]),
        ascii(50708, &format!("{} {}", original.make, original.model)),
        longs(50714, &[0, 0, 0]),
        longs(50717, &[u32::from(WHITE); 3]),
        rationals(50719, &[f64::from(crop[0]) * s, f64::from(crop[1]) * s]),
        rationals(50720, &[f64::from(crop[2]) * s, f64::from(crop[3]) * s]),
        srationals(
            50721,
            &matrix
                .iter()
                .flatten()
                .map(|v| f64::from(*v))
                .collect::<Vec<_>>(),
        ),
        rationals(50728, &wb.map(|v| 1. / f64::from(v))),
        bytes(PRIVATE_DATA, private),
        shorts(50778, &[21]),
        // The tags Adobe added for Smart Previews: the original's sizes.
        longs(51089, &[crop[2], crop[3]]),
        longs(51090, &[original.width, original.height]),
        rationals(51091, &[f64::from(crop[2]), f64::from(crop[3])]),
    ];
    if let Some(b) = original.baseline_exposure {
        entries.push(srationals(50730, &[f64::from(b)]));
    }
    file.extend(ifd(entries, ifd0_at)?);
    file[4..8].copy_from_slice(&ifd0_at.to_le_bytes());
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a rational entry's values read back as.
    fn read(e: &Entry) -> Vec<f64> {
        e.3.chunks(8)
            .map(|c| {
                let n = u32::from_le_bytes(c[..4].try_into().unwrap());
                let d = u32::from_le_bytes(c[4..].try_into().unwrap());
                f64::from(n) / f64::from(d)
            })
            .collect()
    }

    /// A cancel set while the tiles are being encoded stops the build at the
    /// next tile, with nothing returned.
    #[test]
    fn a_cancelled_build_stops_before_encoding_its_tiles() {
        let (width, height) = (600, 400);
        let reduced = CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: vec![[0.2, 0.3, 0.4]; (width * height) as usize].into(),
            metadata: Metadata {
                width,
                height,
                wb: [2., 1., 1.5],
                ..Default::default()
            },
            fast: true,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let stop = std::sync::atomic::AtomicBool::new(true);
        let built = file(&reduced, &reduced.metadata, b"", Cancel::new(&stop));
        assert!(built.is_err());
        let stop = std::sync::atomic::AtomicBool::new(false);
        assert!(file(&reduced, &reduced.metadata, b"", Cancel::new(&stop)).is_ok());
    }

    /// Every tile is TileWidth × TileLength, as TIFF requires: in a tile at
    /// the right and bottom edges every sample past the image repeats the last
    /// column's, then the last row's, and the rest are the image's own.
    #[test]
    fn edge_tiles_are_full_tiles_padded_with_their_edge() {
        let (width, height) = (600u32, 530u32);
        let codes: Vec<u16> = (0..width * height * 3)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 16) as u16)
            .collect();
        let (x0, y0) = (TILE, TILE);
        let (w, h) = (width - x0, height - y0);
        let tile = padded(&codes, width, [x0, y0, w, h]);
        assert_eq!(tile.len(), (TILE * TILE * 3) as usize);
        let source = |x: u32, y: u32, c: u32| codes[(((y0 + y) * width + x0 + x) * 3 + c) as usize];
        for y in 0..TILE {
            for x in 0..TILE {
                for c in 0..3 {
                    let want = source(x.min(w - 1), y.min(h - 1), c);
                    assert_eq!(
                        tile[((y * TILE + x) * 3 + c) as usize],
                        want,
                        "{x}, {y}, {c}"
                    );
                }
            }
        }
    }

    /// The DNG of `codes`, `width × height` pixels of three samples, as the
    /// writer makes it, read back by the DNG reader, and the samples it read:
    /// with the white balance and color matrix the identity, the image the
    /// reader develops is each sample over [`WHITE`].
    fn read_back(codes: &[u16], width: u32, height: u32) -> Vec<u16> {
        let white = f32::from(WHITE);
        let reduced = CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: codes
                .chunks(3)
                .map(|p| {
                    p.iter()
                        .map(|&v| f32::from(v) / white)
                        .collect::<Vec<_>>()
                        .try_into()
                        .unwrap()
                })
                .collect(),
            metadata: Metadata {
                width,
                height,
                wb: [1.; 3],
                cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: true,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        assert!(
            samples(&reduced, [1.; 3]) == codes,
            "the samples written are `codes`"
        );
        let bytes = file(&reduced, &reduced.metadata, b"", Cancel::NEVER).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("copy.dng");
        std::fs::write(&path, bytes).unwrap();
        let read = rawmakase_decode::photo::open(&path)
            .and_then(|raw| {
                raw.develop(
                    rawmakase_model::camera_data::Decode::Full(Default::default()),
                    Cancel::NEVER,
                )
            })
            .unwrap();
        assert_eq!((read.width, read.height), (width, height));
        read.pixels
            .iter()
            .flatten()
            .map(|&v| {
                // Each value is a sample over white, to within a few of the
                // float's steps, so which sample it is is never in doubt.
                let scaled = f64::from(v) * f64::from(WHITE);
                assert!((scaled - scaled.round()).abs() < 0.05, "{v}");
                scaled.round() as u16
            })
            .collect()
    }

    /// Pixels of every 16-bit value, the differences between neighbours of
    /// every length up to 32768, read back exactly: in whole tiles, in tiles
    /// cut short at the right and bottom edges, and in an image smaller than
    /// one tile.
    #[test]
    fn every_tile_reads_back_exactly() {
        for (width, height) in [(1100u32, 700u32), (1024, 512), (300, 200), (2, 2)] {
            let codes: Vec<u16> = (0..width * height * 3)
                .map(|i| match i % 7 {
                    // Large jumps, 32768 among them.
                    0 => (i % 2) as u16 * 32768,
                    // A smooth ramp.
                    1 | 2 => (i / 3 % 65536) as u16,
                    _ => (i.wrapping_mul(2_654_435_761) >> 16) as u16,
                })
                .collect();
            let read = read_back(&codes, width, height);
            assert!(read == codes, "{width} × {height}");
        }
    }

    /// A gradient running into a flat area, the shape zune-jpegxl's 16-bit
    /// lossless encoder writes undecodable streams for, reads back exactly.
    #[test]
    fn a_gradient_into_a_flat_area_reads_back_exactly() {
        let (width, height) = (1100u32, 700u32);
        let codes: Vec<u16> = (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                if y >= 171 {
                    [63_896; 3]
                } else {
                    let v = x * 20 + y * 7;
                    [v, v / 2, v / 3].map(|v| v as u16)
                }
            })
            .collect();
        assert!(read_back(&codes, width, height) == codes);
    }

    /// A tile whose differences' lengths are counted as the Fibonacci numbers
    /// are, as many of length 1 as the 16th, one of length 16, which an
    /// unlimited Huffman code would give codes longer than 16 bits, reads back
    /// exactly through its limited codes.
    #[test]
    fn a_tile_needing_long_codes_reads_back_exactly() {
        let (width, height) = (TILE, 8);
        // Differences of each length 16 down to 1: 2^(length − 1), 1, 1, 2, 3,
        // 5... times.
        let mut differences = Vec::new();
        let (mut a, mut b) = (1, 1);
        for length in (1..=16).rev() {
            differences.extend(std::iter::repeat_n(1u32 << (length - 1), a));
            (a, b) = (b, a + b);
        }
        let mut differences = differences.into_iter();
        // The first sample of the first component is 32768, its prediction;
        // the differences are along the rows of the first component, after
        // their first column, and the last row is flat, as its padding is.
        let mut red = vec![32768u32; (width * height) as usize];
        for y in 0..height {
            let row = (y * width) as usize;
            if y > 0 {
                red[row] = red[row - width as usize];
            }
            for x in 1..width as usize {
                let d = if y < height - 1 {
                    differences.next().unwrap_or(0)
                } else {
                    0
                };
                red[row + x] = (red[row + x - 1] + d) % 65536;
            }
        }
        assert_eq!(differences.next(), None, "every difference is in the tile");
        let codes: Vec<u16> = red.iter().flat_map(|&r| [r as u16, 32768, 32768]).collect();
        assert!(read_back(&codes, width, height) == codes);
    }

    /// Crop sizes and focal lengths above 256 keep their value; small ones keep
    /// their precision.
    #[test]
    fn rationals_keep_large_values_and_small_ones_precise() {
        let values = [2560., 1707., 6000., 600., 1. / 3., 0., -1., f64::NAN];
        let back = read(&rationals(50720, &values));
        for (v, b) in values.iter().zip(&back) {
            let v = if v.is_finite() { v.max(0.) } else { 0. };
            assert!((v - b).abs() <= v * 1e-7 + 1e-7, "{v} read back as {b}");
        }
    }
}
