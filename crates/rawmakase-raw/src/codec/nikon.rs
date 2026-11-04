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
use super::speculative::{self, Stream};
use crate::error::{Error, Result};
use rawmakase_cancel::Cancel;
use rayon::prelude::*;

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
    cancel: Cancel<'_>,
) -> Result<Vec<u16>> {
    decode_with(data, width, height, coding, cancel, true)
}

/// [`decode`], on all threads where it can (`shared`).
fn decode_with(
    data: &[u8],
    width: usize,
    height: usize,
    coding: &Coding,
    cancel: Cancel<'_>,
    shared: bool,
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
    // Data that keeps one table is decoded on all threads.
    if shared && split.is_none_or(|s| s >= height) {
        let stream = Differences {
            data,
            huffman: &huffman,
        };
        if speculative::decode(&stream, &mut out, cancel)?.is_some() {
            predict(&mut out, width, coding, &curve, cancel)?;
            return Ok(out);
        }
    }
    let mut bits = Msb::new(data);
    let mut first = coding.predictors.map(|row| row.map(i32::from));
    for (y, row) in out.chunks_exact_mut(width).enumerate() {
        if y % 64 == 0 && cancel.is_cancelled() {
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

/// The data's differences, each as the 16 bits of an `i16`.
struct Differences<'a> {
    data: &'a [u8],
    huffman: &'a Huffman,
}

impl Stream for Differences<'_> {
    fn data(&self) -> &[u8] {
        self.data
    }
    fn next(&self, bits: &mut Msb, _: usize) -> Option<u16> {
        let Ok(symbol) = self.huffman.symbol(bits) else {
            bits.skip(1);
            return None;
        };
        // A difference of at most 15 bits.
        Some(difference(bits, u32::from(symbol)) as i16 as u16)
    }
}

/// The samples of the differences in `out` (rows of `width`), as [`decode`]
/// predicts them: the first two of each row in order, the rest of each row
/// on all threads.
fn predict(
    out: &mut [u16],
    width: usize,
    coding: &Coding,
    curve: &[u16],
    cancel: Cancel<'_>,
) -> Result<()> {
    let diff = |v: u16| i32::from(v as i16);
    let mut first = coding.predictors.map(|row| row.map(i32::from));
    let starts: Vec<[i32; 2]> = out
        .chunks_exact(width)
        .enumerate()
        .map(|(y, row)| {
            let p = &mut first[y & 1];
            for (p, &v) in p.iter_mut().zip(row) {
                *p += diff(v);
            }
            *p
        })
        .collect();
    out.par_chunks_exact_mut(width)
        .zip(starts)
        .try_for_each(|(row, start)| {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let mut left = start;
            for (x, v) in row.iter_mut().enumerate() {
                let p = if x < 2 {
                    start[x]
                } else {
                    left[x & 1] + diff(*v)
                };
                left[x & 1] = p;
                *v = curve[p.clamp(0, 0x3fff) as usize];
            }
            Ok(())
        })
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

/// Nikon's standard curve of lossy 12-bit data: the 683 points every NEF
/// with a table of version 0x44 0x10 stores (the D40X, D50, D60, D70, D70s,
/// D80, D200, D2H, D2X, D2Xs and D3000 files of our dataset, all equal). The
/// D3, D300 and D700's 12-bit lossy files (version 0x44 0x20) are read with
/// it instead of their own points, as Adobe DNG Converter reads them
/// ([`standard_curve`]).
#[rustfmt::skip]
static STANDARD_CURVE: [u16; 683] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
    12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
    24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35,
    36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
    48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59,
    60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71,
    72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83,
    84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95,
    96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107,
    108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119,
    120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131,
    132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143,
    144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155,
    156, 157, 158, 159, 160, 161, 162, 163, 164, 165, 166, 167,
    168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179,
    180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191,
    192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203,
    204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 215,
    217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228,
    230, 231, 232, 233, 234, 235, 236, 238, 239, 240, 241, 242,
    243, 245, 246, 247, 248, 249, 251, 252, 253, 254, 255, 257,
    258, 259, 261, 262, 263, 264, 266, 267, 268, 270, 271, 272,
    274, 275, 276, 278, 279, 280, 282, 283, 285, 286, 287, 289,
    290, 292, 293, 295, 296, 298, 300, 303, 307, 311, 315, 319,
    323, 328, 332, 336, 341, 345, 349, 354, 358, 362, 367, 371,
    376, 381, 385, 390, 394, 399, 404, 409, 413, 418, 423, 428,
    433, 437, 442, 447, 452, 457, 462, 467, 472, 477, 483, 488,
    493, 498, 503, 508, 514, 519, 524, 530, 535, 540, 546, 551,
    557, 562, 568, 574, 579, 585, 590, 596, 602, 608, 613, 619,
    625, 631, 637, 642, 648, 654, 660, 666, 672, 679, 685, 691,
    697, 703, 709, 716, 722, 728, 735, 741, 747, 754, 760, 767,
    773, 780, 786, 793, 799, 806, 813, 819, 826, 833, 840, 846,
    853, 860, 867, 874, 881, 888, 895, 902, 909, 916, 923, 930,
    937, 944, 952, 959, 966, 973, 981, 988, 995, 1003, 1010, 1017,
    1025, 1032, 1040, 1047, 1055, 1063, 1070, 1078, 1085, 1093, 1101, 1109,
    1116, 1124, 1132, 1140, 1148, 1156, 1164, 1172, 1180, 1188, 1196, 1204,
    1212, 1220, 1228, 1237, 1245, 1253, 1261, 1270, 1278, 1287, 1295, 1303,
    1312, 1320, 1329, 1337, 1346, 1355, 1363, 1372, 1381, 1389, 1398, 1407,
    1416, 1424, 1433, 1442, 1451, 1460, 1469, 1478, 1487, 1496, 1505, 1514,
    1523, 1532, 1541, 1551, 1560, 1569, 1578, 1588, 1597, 1606, 1616, 1625,
    1634, 1644, 1653, 1663, 1672, 1682, 1691, 1701, 1711, 1720, 1730, 1740,
    1750, 1759, 1769, 1779, 1789, 1799, 1809, 1819, 1829, 1839, 1849, 1859,
    1869, 1879, 1889, 1899, 1909, 1920, 1930, 1940, 1951, 1961, 1971, 1982,
    1992, 2003, 2013, 2024, 2034, 2045, 2055, 2066, 2077, 2087, 2098, 2109,
    2119, 2130, 2141, 2152, 2163, 2174, 2185, 2196, 2207, 2218, 2229, 2240,
    2251, 2262, 2273, 2284, 2295, 2306, 2318, 2329, 2340, 2351, 2363, 2374,
    2385, 2397, 2408, 2420, 2431, 2443, 2454, 2466, 2478, 2489, 2501, 2513,
    2524, 2536, 2548, 2560, 2571, 2583, 2595, 2607, 2619, 2631, 2643, 2655,
    2667, 2679, 2691, 2703, 2716, 2728, 2740, 2752, 2765, 2777, 2789, 2802,
    2814, 2826, 2839, 2851, 2864, 2876, 2889, 2902, 2914, 2927, 2939, 2952,
    2965, 2978, 2990, 3003, 3016, 3029, 3042, 3055, 3068, 3081, 3094, 3107,
    3120, 3133, 3146, 3159, 3172, 3185, 3198, 3212, 3225, 3238, 3251, 3265,
    3278, 3291, 3305, 3318, 3332, 3345, 3359, 3372, 3386, 3400, 3413, 3427,
    3440, 3454, 3468, 3482, 3495, 3509, 3523, 3537, 3551, 3565, 3579, 3593,
    3607, 3621, 3635, 3649, 3663, 3677, 3692, 3706, 3720, 3734, 3749, 3763,
    3777, 3792, 3806, 3821, 3835, 3850, 3864, 3879, 3893, 3908, 3923, 3937,
    3952, 3967, 3981, 3996, 4011, 4026, 4041, 4055, 4070, 4085, 4095,
];

/// The curve the `model`'s files of `bits`-bit data whose table is of
/// `version` are read with instead of their own: [`STANDARD_CURVE`] for the
/// D3, D300 and D700's lossy 12-bit files.
pub(crate) fn standard_curve(model: &str, bits: u32, version: [u8; 2]) -> Option<&'static [u16]> {
    (bits == 12 && version == [0x44, 0x20] && matches!(model, "D3" | "D300" | "D700"))
        .then_some(STANDARD_CURVE.as_slice())
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
    use std::sync::atomic::AtomicBool;

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
        decode(data, width, height, c, Cancel::NEVER).unwrap()
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

    /// Nikon's standard 12-bit curve is a curve (rising from 0 to 4095)
    /// and reads the D3, D300 and D700's lossy 12-bit files only.
    #[test]
    fn the_standard_curve_reads_only_the_d3_d300_and_d700s_12_bit_files() {
        let c = super::STANDARD_CURVE;
        assert!(c.is_sorted() && c[0] == 0 && c[682] == 4095);
        for model in ["D3", "D300", "D700"] {
            assert_eq!(super::standard_curve(model, 12, [0x44, 0x20]), Some(&c[..]));
            assert_eq!(super::standard_curve(model, 14, [0x44, 0x20]), None);
            assert_eq!(super::standard_curve(model, 12, [0x44, 0x40]), None);
        }
        assert_eq!(super::standard_curve("D300S", 12, [0x44, 0x20]), None);
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

    /// Long streams decode on all threads as they do in order, the tables
    /// switching at a split past the last row or not at all.
    #[test]
    fn parallel_decoding_is_the_sequential_one() {
        let data: Vec<u8> = (0..1_500_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let points: Vec<u16> = (0..257).map(|i| (i * 16).min(4095)).collect();
        for c in [
            coding([0x46, 0x30], 12, &[], 0),
            coding([0x46, 0x30], 14, &[], 0),
            coding([0x44, 0x20], 12, &points, 2000),
        ] {
            let cancel = AtomicBool::new(false);
            let (width, height) = (1500, 1000);
            let parallel =
                decode_with(&data, width, height, &c, Cancel::new(&cancel), true).unwrap();
            let sequential =
                decode_with(&data, width, height, &c, Cancel::new(&cancel), false).unwrap();
            assert!(parallel == sequential, "{:?}", c.version);
        }
    }

    #[test]
    fn cancelling_stops_the_prediction() {
        let c = coding([0x46, 0x30], 12, &[], 0);
        let curve: Vec<u16> = (0..0x4000).collect();
        let mut out = vec![0; 64];
        assert_eq!(
            predict(&mut out, 8, &c, &curve, Cancel::new(&AtomicBool::new(true))),
            Err(Error::Cancelled)
        );
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
                let _ = decode(&data, 16, 4, &c, Cancel::NEVER);
            }
        }
        let c = coding([0x46, 0x30], 16, &[], 0);
        assert!(decode(&[0; 64], 4, 4, &c, Cancel::NEVER).is_err());
        let c = coding([0x46, 0x30], 12, &[], 0);
        assert_eq!(
            decode(&[0; 64], 4, 4, &c, Cancel::new(&AtomicBool::new(true))),
            Err(Error::Cancelled)
        );
    }
}
