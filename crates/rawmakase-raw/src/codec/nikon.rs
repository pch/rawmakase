//! Nikon's Huffman-coded NEF data (TIFF compression 34713), lossless and
//! lossy: a bit stream, most significant bit first and continuous across
//! rows, of one Huffman-coded difference per sample. Each sample is
//! predicted from the one two columns to its left (same colour); the first
//! two of a row from the first two of the row two above, starting from the
//! linearization table's predictions. Lossy data's values index the table's
//! curve, and from the table's split row a second Huffman table codes the
//! rest.
//!
//! Sources: Laurent Clévy's public NEF notes (the linearization table's
//! layout: version, predictions, curve size and points, split row); ITU T.81
//! (canonical Huffman codes, difference categories and their additional
//! bits); our own measurements: the six Huffman tables and the difference
//! coding, recovered from LibRaw's output on the test dataset's files, and
//! the curve's interpolation, from Adobe DNG Converter's linearization tables
//! of the same files.
use super::bits::Msb;
use super::huffman::Huffman;
use crate::error::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};

/// The linearization table's version of lossless data.
const LOSSLESS: u8 = 0x46;

/// The data's coding, from the linearization table.
#[derive(Clone, Copy)]
pub(crate) struct Coding<'a> {
    pub version: [u8; 2],
    pub predictors: [[u16; 2]; 2],
    /// The curve's size and stored points.
    pub size: u16,
    pub points: &'a [u16],
    pub split: u16,
    pub bits: u32,
    /// A whole curve that replaces the points' (`table[v]`, its last value
    /// beyond it): the camera's, where the file's is not the one to read.
    pub table: Option<&'a [u16]>,
}

/// Decodes `width × height` samples of `bits` bits from `data`.
pub(crate) fn decode(
    data: &[u8],
    width: usize,
    height: usize,
    coding: &Coding,
    cancel: &AtomicBool,
) -> Result<Vec<u16>> {
    if !matches!(coding.bits, 12 | 14) {
        return Err(Error::Unsupported(format!(
            "Nikon compressed data of {} bits",
            coding.bits
        )));
    }
    let samples = crate::io::samples(width as u32, height as u32, 1)?;
    let curve = curve(coding);
    let mut table = if coding.version[0] == LOSSLESS { 2 } else { 0 };
    if coding.bits == 14 {
        table += 3;
    }
    // Only lossy data of the second kind switches tables; in the first kind
    // the split's place holds curve points.
    let split = matches!(coding.version, [0x44, 0x20 | 0x40])
        .then_some(usize::from(coding.split))
        .filter(|&s| s > 0);
    let (counts, symbols) = TABLES[table];
    let mut huffman = Huffman::new(&counts, symbols)?;
    let mut out = vec![0u16; samples];
    let mut bits = Msb::new(data);
    let mut first = coding.predictors.map(|row| row.map(i32::from));
    for (y, row) in out.chunks_exact_mut(width).enumerate() {
        if y % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if split == Some(y) {
            let (counts, symbols) = TABLES[table + 1];
            huffman = Huffman::new(&counts, symbols)?;
        }
        let mut left = [0i32; 2];
        for (x, v) in row.iter_mut().enumerate() {
            let symbol = u32::from(huffman.symbol(&mut bits)?);
            let diff = difference(&mut bits, symbol);
            let p = if x < 2 {
                let p = &mut first[y & 1][x];
                *p += diff;
                *p
            } else {
                left[x & 1] + diff
            };
            left[x & 1] = p;
            *v = curve[p.clamp(0, 0x3fff) as usize];
        }
    }
    Ok(out)
}

/// The difference a symbol and its additional bits code: the low four bits
/// of the symbol are the difference's length; the high four, how many of its
/// low bits the coder dropped (the difference is then the middle of the range
/// those bits span).
#[inline]
fn difference(bits: &mut Msb, symbol: u32) -> i32 {
    let len = symbol & 15;
    let dropped = symbol >> 4;
    if len == 0 {
        return 0;
    }
    let kept = len.saturating_sub(dropped);
    let mut diff = (((bits.get(kept) << 1) + 1) << dropped >> 1) as i32;
    if diff & (1 << (len - 1)) == 0 {
        diff -= (1 << len) - i32::from(dropped == 0);
    }
    diff
}

/// The curve from decoded values (clamped to 14 bits) to raw values: the
/// values themselves, except where the table's points say otherwise. Lossy
/// data of the second kind places point `i` at `i·m / 255` (version 0x20:
/// the first 256 points span every value) or `i·m / 1024` (0x40: they span
/// a quarter), `m` the largest value of the data's bits, linearly
/// interpolated and rounded half up; of the first kind, point `i` at `i`.
/// Past the last point, its value. This is Adobe DNG Converter's
/// linearization table of our dataset's lossy files (but for the D3, D300
/// and D700's of 12 bits, which it reads with the first kind's curve).
fn curve(coding: &Coding) -> Vec<u16> {
    let mut curve: Vec<u16> = (0..=0x3fff).collect();
    let size = usize::from(coding.size);
    let point = |i: usize| i64::from(coding.points.get(i).copied().unwrap_or(0));
    if let Some(table) = coding.table.filter(|t| !t.is_empty()) {
        for (i, v) in curve.iter_mut().enumerate() {
            *v = table[i.min(table.len() - 1)];
        }
        return curve;
    }
    match coding.version {
        [LOSSLESS, _] => {}
        [0x44, v @ (0x20 | 0x40)] if size > 1 => {
            let m = (1i64 << coding.bits) - 1;
            let per = if v == 0x40 { 1024 } else { 255 };
            for (x, out) in curve.iter_mut().enumerate() {
                let t = x as i64 * per;
                let (i, f) = ((t / m) as usize, t % m);
                let y = if i + 1 < size {
                    let (lo, hi) = (point(i), point(i + 1));
                    // lo + (hi − lo)·f/m, rounded half up.
                    (2 * (lo * m + (hi - lo) * f) + m).div_euclid(2 * m)
                } else {
                    point(size - 1)
                };
                *out = y.clamp(0, 0xffff) as u16;
            }
        }
        _ if size > 0 => {
            for (i, v) in curve.iter_mut().enumerate() {
                *v = point(i.min(size - 1)) as u16;
            }
        }
        _ => {}
    }
    curve
}

/// The six Huffman tables, as T.81 writes them (the number of codes of each
/// length 1 to 16, then the symbols in code order): lossy 12-bit data, its
/// table from the split row on, lossless 12-bit, and the same three at 14
/// bits (our own measurements).
type Table = ([u8; 16], &'static [u8]);
static TABLES: [Table; 6] = [
    (
        [0, 1, 5, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
        &[5, 4, 3, 6, 2, 7, 1, 0, 8, 9, 11, 10, 12],
    ),
    (
        [0, 1, 5, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
        &[0x39, 0x5a, 0x38, 0x27, 0x16, 5, 4, 3, 2, 1, 0, 11, 12, 12],
    ),
    (
        [0, 1, 4, 2, 3, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        &[5, 4, 6, 3, 7, 2, 8, 1, 9, 0, 10, 11, 12],
    ),
    (
        [0, 1, 4, 3, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
        &[5, 6, 4, 7, 8, 3, 9, 2, 1, 0, 10, 11, 12, 13, 14],
    ),
    (
        [0, 1, 5, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0],
        &[8, 0x5c, 0x4b, 0x3a, 0x29, 7, 6, 5, 4, 3, 2, 1, 0, 13, 14],
    ),
    (
        [0, 1, 4, 2, 2, 3, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
        &[7, 6, 8, 5, 9, 4, 10, 3, 11, 12, 2, 0, 1, 13, 14],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Bits written as a string of 0s and 1s, zeros after.
    fn stream(bits: &str) -> Vec<u8> {
        let mut bits = bits.to_string();
        while !bits.len().is_multiple_of(8) || bits.len() < 256 {
            bits.push('0');
        }
        bits.as_bytes()
            .chunks(8)
            .map(|b| b.iter().fold(0u8, |a, &c| a << 1 | u8::from(c == b'1')))
            .collect()
    }

    fn coding(version: [u8; 2], bits: u32, points: &[u16], split: u16) -> Coding<'_> {
        Coding {
            version,
            predictors: [[2048; 2]; 2],
            size: points.len() as u16,
            points,
            split,
            bits,
            table: None,
        }
    }

    fn run(data: &[u8], width: usize, height: usize, c: &Coding) -> Vec<u16> {
        decode(data, width, height, c, &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn lossless_differences_decode_as_measured() {
        // Lossless 12-bit codes (T.81 categories): 00 is category 5, 11110
        // category 0, 111110 category 10. The values LibRaw reported for
        // such streams, worked out by hand: −31, +31, 0, +1023 on the
        // predictions of 2048 two columns apart, then a second row from the
        // second row's own predictions.
        let bits = [
            "0000000",
            "0011111",
            "11110",
            "1111101111111111",
            "11110",
            "11110",
            "0000001",
            "11110",
        ]
        .concat();
        let c = coding([0x46, 0x30], 12, &[], 0);
        assert_eq!(
            run(&stream(&bits), 4, 2, &c),
            [2017, 2079, 2017, 3102, 2048, 2048, 2048 - 30, 2048]
        );
    }

    #[test]
    fn the_split_row_switches_to_the_second_table() {
        // Lossy 12-bit data of the second kind over an identity curve (points
        // every 16 values). Row 0 is coded with the first table (11110,
        // category 0); from the split row, the second table, whose 00 drops
        // three low bits of a 9-bit difference: six zero bits are −508.
        let points: Vec<u16> = (0..257).map(|i| (i * 16).min(4095)).collect();
        let bits = [
            "11110".repeat(4),
            "00000000".to_string(),
            "00000000".to_string(),
        ]
        .concat();
        let c = coding([0x44, 0x20], 12, &points, 1);
        let out = run(&stream(&bits), 4, 2, &c);
        let curve = curve(&c);
        assert_eq!(out[..4], [curve[2048]; 4]);
        assert_eq!(out[4..6], [curve[2048 - 508]; 2]);
        // Without a split, or in data of the first kind, the first table
        // codes every row: 00 is category 5 there.
        let points: Vec<u16> = (0..4096).collect();
        for c in [
            coding([0x44, 0x20], 12, &points[..257], 0),
            coding([0x44, 0x10], 12, &points, 1),
        ] {
            let out = run(&stream(&bits), 4, 2, &c);
            assert_eq!(out[4], super::curve(&c)[2048 - 31], "{:?}", c.version);
        }
    }

    #[test]
    fn curves_interpolate_their_points_and_clamp_at_14_bits() {
        // Version 0x20 places point i at i·4095/255 in 12-bit data: 1000 at
        // 16.06, 4000 at 32.12, and 4000 beyond; between them, rounded half
        // up: 8 is 1000·2040/4095 = 498.2, 16 is 996.3, 17 is 1000 +
        // 3000·240/4095 = 1175.8.
        let c = coding([0x44, 0x20], 12, &[0, 1000, 4000], 0);
        let curve = curve(&c);
        assert_eq!(
            [curve[8], curve[16], curve[17], curve[33], curve[5000]],
            [498, 996, 1176, 4000, 4000]
        );
        // A 14-bit curve of 257 points every 64: 16383 is 255·64 = 16320,
        // and 2056 is 2048 + 64·(2056·255 − 32·16383)/16383 = 2048.1.
        let points: Vec<u16> = (0..257).map(|i| i * 64).collect();
        let c = coding([0x44, 0x20], 14, &points, 0);
        let curve = super::curve(&c);
        assert_eq!([curve[16383], curve[2056]], [16320, 2048]);
        // 0x40 places it at i·4095/1024: 2 is 1000·2048/4095 = 500.1, 4 is
        // 1000 + 3000·1/4095.
        let c = coding([0x44, 0x40], 12, &[0, 1000, 4000], 0);
        let curve = super::curve(&c);
        assert_eq!(
            [curve[2], curve[4], curve[8], curve[1025]],
            [500, 1001, 4000, 4000]
        );
        // The first kind's points are the curve's start, its last beyond.
        let c = coding([0x44, 0x10], 12, &[0, 7, 9], 0);
        assert_eq!(super::curve(&c)[..5], [0, 7, 9, 9, 9]);
        // A camera's table replaces the points, its last value beyond it.
        let mut c = coding([0x44, 0x20], 12, &[0, 1000, 4000], 0);
        c.table = Some(&[0, 3, 5]);
        assert_eq!(super::curve(&c)[..5], [0, 3, 5, 5, 5]);
        // Lossless 14-bit: 16000 + 8192 is beyond 14 bits and clamps
        // (LibRaw reported 16383); category 14 is 11111111.
        let mut c = coding([0x46, 0x30], 14, &[], 0);
        c.predictors = [[16000; 2]; 2];
        let bits = ["11111111", "1", &"0".repeat(13)].concat();
        assert_eq!(run(&stream(&bits), 2, 1, &c)[0], 16383);
    }

    #[test]
    fn every_table_is_complete() {
        for (counts, symbols) in TABLES {
            let kraft: f64 = counts
                .iter()
                .enumerate()
                .map(|(i, &n)| f64::from(n) / f64::from(1u32 << (i + 1)))
                .sum();
            assert_eq!(kraft, 1.0);
            let n: usize = counts.iter().map(|&n| usize::from(n)).sum();
            assert_eq!(n, symbols.len());
        }
    }

    #[test]
    fn truncated_and_garbage_streams_do_not_panic() {
        let points: Vec<u16> = (0..257).map(|i| i * 16).collect();
        for (version, bits) in [([0x46, 0x30], 12), ([0x44, 0x20], 14), ([0x44, 0x40], 12)] {
            let c = coding(version, bits, &points, 2);
            for len in 0..40 {
                let data: Vec<u8> = (0..len).map(|i| (i * 151 + 7) as u8).collect();
                let _ = decode(&data, 16, 4, &c, &AtomicBool::new(false));
            }
        }
        let c = coding([0x46, 0x30], 16, &[], 0);
        assert!(decode(&[0; 64], 4, 4, &c, &AtomicBool::new(false)).is_err());
        let c = coding([0x46, 0x30], 12, &[], 0);
        assert_eq!(
            decode(&[0; 64], 4, 4, &c, &AtomicBool::new(true)),
            Err(Error::Cancelled)
        );
    }
}
