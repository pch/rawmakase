//! Sony's own sensor codings, knowing no container: ARW2's blocks of 16
//! samples (an 11-bit maximum and minimum with 7-bit steps between them) and
//! the tone curve its 11-bit codes are points on; ARW1's Huffman-coded
//! columns; packed 12-bit samples; small RAWs' luma and chroma.
//!
//! Sources: H. Dietz, "Sony ARW2 Compression: Artifacts And Credible Repair"
//! (IS&T Electronic Imaging 2016) for the block layout; ExifTool's Sony tag
//! documentation for the tone curve's tag (SonyToneCurve, 0x7010); ITU T.81
//! (difference coding) and T.871 (JFIF's YCbCr); our own measurements against
//! LibRaw's output on our files: the curve's segments, the packed samples'
//! bit order, and the YCbCr rounding. ARW1's code table and scan order are
//! written from the implementer's prior general knowledge of the format, no
//! decoder's source consulted; no permitted public description was found
//! (ExifTool's documentation only names the format, "ARW 1.0"). They are
//! validated by our own measurements: decoding matches LibRaw's output
//! exactly on all 10 ARW1 files of our dataset.
use super::bits::Msb;
use crate::error::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};

/// Samples in an ARW2 block, and bytes it is stored in.
const BLOCK: usize = 16;

/// Decodes one 16-byte ARW2 block into 16 codes of 11 bits: a little-endian
/// 128-bit word holding the maximum (bits 0–10), the minimum (11–21), the
/// maximum's and the minimum's positions (22–25, 26–29), then 14 steps of 7
/// bits above the minimum, in the order of the other positions, scaled by
/// the smallest power of two that makes 7 bits span the range.
pub(crate) fn arw2_block(block: &[u8; BLOCK], out: &mut [u16; BLOCK]) {
    let word = u128::from_le_bytes(*block);
    let max = (word & 0x7ff) as u16;
    let min = (word >> 11 & 0x7ff) as u16;
    let at_max = (word >> 22 & 15) as usize;
    let at_min = (word >> 26 & 15) as usize;
    let range = max.saturating_sub(min);
    let mut shift = 0;
    while shift < 4 && range >> shift > 127 {
        shift += 1;
    }
    let mut bit = 30;
    for (i, v) in out.iter_mut().enumerate() {
        *v = if i == at_max {
            max
        } else if i == at_min {
            min
        } else {
            // Corrupt blocks (both positions the same) run past the word.
            let step = if bit + 7 <= 128 {
                (word >> bit & 0x7f) as u16
            } else {
                0
            };
            bit += 7;
            (min + (step << shift)).min(0x7ff)
        };
    }
}

/// Decodes ARW2 rows of `width` bytes into `height` rows of `width` codes:
/// each 32 columns are two blocks, the even columns' then the odd columns'.
/// Rows past the end of `data` stay zero.
pub(crate) fn arw2(
    data: &[u8],
    width: usize,
    height: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    let mut codes = [0u16; BLOCK];
    for (y, (row, dst)) in data
        .chunks_exact(width)
        .zip(out.chunks_exact_mut(width))
        .take(height)
        .enumerate()
    {
        if y % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        for (group, dst) in row
            .as_chunks::<{ 2 * BLOCK }>()
            .0
            .iter()
            .zip(dst.as_chunks_mut::<{ 2 * BLOCK }>().0)
        {
            for (half, block) in group.as_chunks::<BLOCK>().0.iter().enumerate() {
                arw2_block(block, &mut codes);
                for (i, &c) in codes.iter().enumerate() {
                    dst[half + 2 * i] = c;
                }
            }
        }
    }
    Ok(())
}

/// Sony's tone curve as the values of its 4096 12-bit points: slope 1 up to
/// the first of the four thresholds (SonyToneCurve, in quarters), doubling at
/// each. An ARW2 code `c` is the point `2c`.
pub(crate) fn tone_curve(thresholds: [u16; 4]) -> Vec<u16> {
    let mut knots = [0u32; 6];
    for (k, &t) in knots[1..5].iter_mut().zip(&thresholds) {
        *k = (u32::from(t) / 4).min(4095);
    }
    knots[5] = 4095;
    let mut curve = Vec::with_capacity(4096);
    let mut value = 0u32;
    let mut segment = 0;
    curve.push(0);
    for x in 1..4096u32 {
        while segment < 4 && x > knots[segment + 1] {
            segment += 1;
        }
        value += 1 << segment;
        curve.push(value.min(u32::from(u16::MAX)) as u16);
    }
    curve
}

/// ARW1's code lengths and the difference bits each code is followed by, in
/// the order the codes take the code space from all zeros.
const ARW1_CODES: [(u8, u8); 18] = [
    (15, 15),
    (15, 16),
    (14, 15),
    (13, 14),
    (12, 13),
    (11, 12),
    (10, 11),
    (9, 10),
    (8, 9),
    (7, 8),
    (6, 7),
    (5, 6),
    (4, 5),
    (3, 4),
    (3, 3),
    (3, 0),
    (2, 2),
    (2, 1),
];

/// Decodes ARW1: one running sum of Huffman-coded differences (each code
/// followed by its difference's bits, as JPEG codes them), column by column
/// from the right, each column's even rows then its odd rows. Data past the
/// end reads as zeros.
pub(crate) fn arw1(
    data: &[u8],
    width: usize,
    height: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    // The code of every 15-bit prefix: its length and difference bits.
    let mut table = Vec::with_capacity(1 << 15);
    for &(len, bits) in &ARW1_CODES {
        table.extend(std::iter::repeat_n((len, bits), 1 << (15 - len)));
    }
    let mut pump = Msb::new(data);
    let mut sum = 0i32;
    for (n, col) in (0..width).rev().enumerate() {
        if n % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut row = 0;
        while row < height + 1 {
            if row == height {
                row = 1;
            }
            let (len, bits) = table[pump.peek(15) as usize];
            pump.skip(u32::from(len));
            let diff = match bits {
                0 => 0,
                16 => -32768,
                b => super::huffman::extend(pump.get(u32::from(b)), u32::from(b)),
            };
            sum = sum.wrapping_add(diff);
            if row < height {
                out[row * width + col] = sum as u16;
            }
            row += 2;
        }
    }
    Ok(())
}

/// Unpacks 12-bit samples stored two in three bytes, least significant bits
/// first: `a = b0 | (b1 & 15) << 8`, `b = b1 >> 4 | b2 << 4`.
pub(crate) fn packed12(row: &[u8], out: &mut [u16]) {
    for (b, px) in row
        .as_chunks::<3>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<2>().0)
    {
        px[0] = u16::from(b[0]) | u16::from(b[1] & 15) << 8;
        px[1] = u16::from(b[1] >> 4) | u16::from(b[2]) << 4;
    }
}

/// Sony's small RAWs' luma and chroma (chroma about 16383) as red, green and
/// blue: JFIF's conversion (ITU T.871's coefficients to five places) in
/// single precision with fused multiply-adds, rounded down, as RAWmakase has
/// always read them (our measurements: exact on every sample of our files).
pub(crate) fn ycbcr_rgb(y: u16, cb: u16, cr: u16) -> [u16; 3] {
    let (y, cb, cr) = (f32::from(y), f32::from(cb) - 16383., f32::from(cr) - 16383.);
    [
        1.402f32.mul_add(cr, y),
        (-0.71414f32).mul_add(cr, (-0.34414f32).mul_add(cb, y)),
        1.772f32.mul_add(cb, y),
    ]
    .map(|v| v.clamp(0., 65535.) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Packs a block as the paper describes it, independently of the decoder.
    fn pack(max: u16, min: u16, at_max: usize, at_min: usize, steps: &[u16]) -> [u8; 16] {
        let mut word = u128::from(max) | u128::from(min) << 11;
        word |= (at_max as u128) << 22 | (at_min as u128) << 26;
        for (i, &s) in steps.iter().enumerate() {
            word |= u128::from(s) << (30 + 7 * i);
        }
        word.to_le_bytes()
    }

    #[test]
    fn blocks_scale_steps_to_the_range() {
        // Range 100: steps are unscaled.
        let steps: Vec<u16> = (0..14).map(|i| i * 7).collect();
        let mut out = [0; 16];
        arw2_block(&pack(600, 500, 3, 9, &steps), &mut out);
        let mut want = vec![];
        let mut s = steps.iter();
        for i in 0..16 {
            want.push(match i {
                3 => 600,
                9 => 500,
                _ => 500 + *s.next().unwrap(),
            });
        }
        assert_eq!(out.to_vec(), want);
        // Range 1000: 7 bits span it at a shift of 3 (127 << 3 = 1016).
        arw2_block(&pack(1500, 500, 0, 1, &[127; 14]), &mut out);
        assert_eq!(&out[..3], &[1500, 500, 1516]);
        // Clamped to 11 bits.
        arw2_block(&pack(2047, 1000, 0, 1, &[127; 14]), &mut out);
        assert_eq!(out[2], 2047);
    }

    #[test]
    fn rows_interleave_two_blocks() {
        let even = pack(10, 0, 0, 1, &[1; 14]);
        let odd = pack(20, 5, 15, 14, &[2; 14]);
        let row: Vec<u8> = even.iter().chain(&odd).copied().collect();
        let mut out = vec![0; 32];
        arw2(&row, 32, 1, &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(&out[..6], &[10, 5 + 2, 0, 5 + 2, 1, 7]);
        assert_eq!(&out[28..], &[1, 5, 1, 20]);
    }

    #[test]
    fn the_tone_curve_doubles_its_slope_at_each_threshold() {
        let c = tone_curve([8000, 10400, 12900, 14100]);
        assert_eq!(c.len(), 4096);
        assert_eq!(c[2000], 2000);
        assert_eq!(c[2060], 2120);
        assert_eq!(c[2600], 3200);
        assert_eq!(c[3225], 5700);
        assert_eq!(c[3525], 8100);
        assert_eq!(c[4018], 15988);
    }

    #[test]
    fn packed_samples_are_least_significant_bits_first() {
        let mut out = [0u16; 4];
        packed12(&[0x21, 0x43, 0x65, 0xff, 0x0f, 0x00], &mut out);
        assert_eq!(out, [0x321, 0x654, 0xfff, 0x000]);
    }

    #[test]
    fn small_raws_convert_with_jfif_coefficients() {
        // Neutral chroma is luma itself.
        assert_eq!(ycbcr_rgb(1000, 16383, 16383), [1000; 3]);
        // Red: Y + 1.402 Cr; green: Y − 0.34414 Cb − 0.71414 Cr; blue:
        // Y + 1.772 Cb, rounded down.
        assert_eq!(ycbcr_rgb(1000, 16383 + 10, 16383 - 20), [971, 1010, 1017]);
        // Out of range clips.
        assert_eq!(ycbcr_rgb(10, 0, 16383), [10, 5648, 0]);
    }

    /// ARW1's stream for `samples` (`width × height`, rows even then odd,
    /// columns from the right), coded independently of the decoder: each
    /// code is the next run of the code space in table order.
    fn arw1_encode(samples: &[u16], width: usize, height: usize) -> Vec<u8> {
        let mut codes = vec![];
        let mut start = 0u32;
        for &(len, bits) in &ARW1_CODES {
            codes.push((start >> (15 - len), u32::from(len), u32::from(bits)));
            start += 1 << (15 - len);
        }
        let mut bits: Vec<bool> = vec![];
        let mut put = |v: u32, n: u32| (0..n).rev().for_each(|i| bits.push(v >> i & 1 == 1));
        let mut sum = 0i32;
        for col in (0..width).rev() {
            for row in (0..height).step_by(2).chain((1..height).step_by(2)) {
                let d = i32::from(samples[row * width + col]) - sum;
                sum += d;
                let n = 32 - d.unsigned_abs().leading_zeros();
                let &(code, len, _) = codes.iter().find(|c| c.2 == n).unwrap();
                put(code, len);
                put(
                    if d < 0 {
                        (d + (1 << n) - 1) as u32
                    } else {
                        d as u32
                    },
                    n,
                );
            }
        }
        bits.chunks(8)
            .map(|b| b.iter().fold(0u8, |a, &x| a << 1 | u8::from(x)) << (8 - b.len()))
            .collect()
    }

    #[test]
    fn arw1_sums_differences_down_columns_from_the_right() {
        let (w, h) = (5, 6);
        let samples: Vec<u16> = (0..w * h).map(|i| (i * 977 % 4096) as u16).collect();
        let data = arw1_encode(&samples, w, h);
        let mut out = vec![0; w * h];
        arw1(&data, w, h, &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(out, samples);
        // Truncated: zeros, no panic.
        arw1(
            &data[..data.len() / 2],
            w,
            h,
            &mut out,
            &AtomicBool::new(false),
        )
        .unwrap();
    }
}
