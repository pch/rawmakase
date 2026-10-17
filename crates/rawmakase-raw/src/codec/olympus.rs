//! Olympus's compressed sensor coding (ORF and ORI), knowing no container:
//! after a 7-byte header, one bit stream (most significant bit first) codes
//! every row's samples in raster order.
//!
//! Each sample is a difference from a prediction. Each colour of a row (the
//! even and the odd columns) keeps three running values, all zero at the
//! start of a row: the last code's magnitude `m`, a bias `b` and a count `r`
//! of small codes in a row. A code is read as:
//!
//! - its width `n`: with `k` = 2 while `r` < 3 and 0 after, the least `n` ≥
//!   2 + `k` for which `m` (as 16 bits) has no bit at or above `n + k`;
//! - 3 bits: a sign (the highest) and the two low bits of the sample's
//!   difference;
//! - the high part `h`: the number of zero bits before a one (the one
//!   consumed), at most 12; at 12 (no one) it is the next `16 − n` bits
//!   halved instead;
//! - the magnitude `m' = h·2ⁿ` plus the next `n` bits; the difference is
//!   `4·(b + (m' or, when negative, its complement)) + low bits`.
//!
//! Then `b` becomes `(3·d + b) >> 5` for `d = b ± m'`, `r` counts up (or
//! restarts at 0 when `m'` > 16) and `m = m'`. The prediction from the same
//! colour's neighbours `w` (two to the left), `n` (two up) and `nw`: 0 for
//! the first two rows and columns' corner, `w` along the first two rows, `n`
//! down the first two columns; else, when `nw` lies strictly between `w` and
//! `n`, `w + n − nw` if `w` or `n` differ from `nw` by more than 32 and their
//! mean otherwise; else whichever of `w` and `n` differs more from `nw`.
//! Samples are 16-bit words (a sum past them wraps).
//!
//! Sources: written from prior general knowledge of the format (no decoder
//! source consulted), then verified and corrected by measurement against
//! LibRaw's output on our files (an exact match on every compressed ORF and
//! ORI of our dataset); the measurement found that 14-bit samples' codes
//! start a byte later than 12-bit samples'.
use crate::codec::bits::Msb;
use crate::error::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};

/// Bytes before the first row's codes of 12-bit samples (14-bit samples
/// have one more).
pub(crate) const HEADER: usize = 7;

/// One colour's running values in a row.
#[derive(Clone, Copy, Default)]
struct Carry {
    magnitude: i32,
    bias: i32,
    run: i32,
}

/// Decodes `height` rows of `width` samples into `out` (`width × height`),
/// the codes starting `header` bytes into `data`.
pub(crate) fn decode(
    data: &[u8],
    header: usize,
    width: usize,
    height: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    let mut bits = Msb::new(data.get(header..).unwrap_or(&[]));
    for row in 0..height {
        if row % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut carry = [Carry::default(); 2];
        for col in 0..width {
            let c = &mut carry[col & 1];
            let k = if c.run < 3 { 2 } else { 0 };
            let mut n = 2 + k;
            while (c.magnitude as u16 as u32) >> (n + k) != 0 {
                n += 1;
            }
            let three = bits.get(3) as i32;
            let low = three & 3;
            let negative = three & 4 != 0;
            let zeros = bits.peek(12).leading_zeros().saturating_sub(20);
            let high = if zeros >= 12 {
                bits.skip(12);
                bits.get(16u32.saturating_sub(n)) >> 1
            } else {
                bits.skip(zeros + 1);
                zeros
            };
            let magnitude = ((high << n) | bits.get(n)) as i32;
            let signed = if negative { !magnitude } else { magnitude };
            let diff = signed.wrapping_add(c.bias);
            c.bias = diff.wrapping_mul(3).wrapping_add(c.bias) >> 5;
            c.run = if magnitude > 16 { 0 } else { c.run + 1 };
            c.magnitude = magnitude;
            let at = |r: usize, x: usize| i32::from(out[r * width + x]);
            let pred = match (row < 2, col < 2) {
                (true, true) => 0,
                (true, false) => at(row, col - 2),
                (false, true) => at(row - 2, col),
                (false, false) => predict(at(row, col - 2), at(row - 2, col), at(row - 2, col - 2)),
            };
            out[row * width + col] = pred.wrapping_add(diff.wrapping_shl(2) | low) as u16;
        }
    }
    Ok(())
}

/// The prediction from the same colour's neighbours to the left (`w`),
/// above (`n`) and above left (`nw`).
fn predict(w: i32, n: i32, nw: i32) -> i32 {
    if (w < nw && nw < n) || (n < nw && nw < w) {
        if (w - nw).abs() > 32 || (n - nw).abs() > 32 {
            w + n - nw
        } else {
            (w + n) >> 1
        }
    } else if (w - nw).abs() > (n - nw).abs() {
        w
    } else {
        n
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Bits written most significant first.
    #[derive(Default)]
    struct Writer {
        bytes: Vec<u8>,
        bits: u32,
    }

    impl Writer {
        fn put(&mut self, value: u32, n: u32) {
            for i in (0..n).rev() {
                if self.bits.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                let bit = (value >> i & 1) as u8;
                *self.bytes.last_mut().unwrap() |= bit << (7 - self.bits % 8);
                self.bits += 1;
            }
        }
    }

    /// A header and one row of four samples, every code spelled out by hand
    /// from the description above: they decode to [21, 65527, 97, 65525].
    pub(crate) fn row_of_four() -> Vec<u8> {
        let mut w = Writer::default();
        w.bytes.extend([0; HEADER]);
        w.bits = HEADER as u32 * 8;
        // Col 0: width 4 (m = 0, r = 0 → k = 2); sign 0, low 1; high 0
        // ("1"); 4 bits 0101 → m' = 5, d = 5 → 4·5 + 1 = 21.
        w.put(0b001, 3);
        w.put(1, 1);
        w.put(0b0101, 4);
        // Col 1: m' = 2 (high 0, "0010"), low 3, negative → d = !2 = −3:
        // 4·(−3) + 3 = −9 → wraps to 65527.
        w.put(0b111, 3);
        w.put(1, 1);
        w.put(0b0010, 4);
        // Col 2 (colour 0: m = 5, b = (15 + 0) >> 5 = 0, r = 1): width 4
        // (5 >> 6 = 0); high 1 ("01"), 4 bits 0011 → m' = 19; d = 19:
        // pred w = 21 → 21 + 76 + 0 = 97.
        w.put(0b000, 3);
        w.put(0b01, 2);
        w.put(0b0011, 4);
        // Col 3 (colour 1: m = 2, b = (−9 + 0) >> 5 = −1, r = 1): m' = 0,
        // low 2, positive → d = −1 + 0 = −1; pred 65527 → 65527 − 4 + 2.
        w.put(0b010, 3);
        w.put(1, 1);
        w.put(0, 4);
        w.bytes
    }

    #[test]
    fn codes_are_differences_from_the_neighbours() {
        let mut out = [0u16; 4];
        decode(
            &row_of_four(),
            HEADER,
            4,
            1,
            &mut out,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(out, [21, 65527, 97, 65525]);
    }

    #[test]
    fn a_missing_one_reads_the_high_part_from_the_next_bits() {
        let mut w = Writer::default();
        w.bytes.extend([0; HEADER]);
        w.bits = HEADER as u32 * 8;
        // Twelve zeros, then 16 − 4 = 12 bits halved (0b100 → high 2), then
        // 4 low bits 0001: m' = 2·16 + 1 = 33, d = 33 → 4·33 = 132.
        w.put(0b000, 3);
        w.put(0, 12);
        w.put(0b100, 12);
        w.put(0b0001, 4);
        let mut out = [0u16; 1];
        decode(&w.bytes, HEADER, 1, 1, &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(out, [132]);
    }

    #[test]
    fn predictions_follow_the_neighbours() {
        // A gradient through the corner: the plane through the three.
        assert_eq!(predict(100, 200, 150), 150);
        assert_eq!(predict(100, 300, 150), 250);
        // Close to the corner: their mean.
        assert_eq!(predict(100, 120, 110), 110);
        assert_eq!(predict(101, 120, 110), 110);
        // The corner outside them: the one farther from it.
        assert_eq!(predict(100, 120, 90), 120);
        assert_eq!(predict(100, 120, 130), 100);
        assert_eq!(predict(100, 120, 110 + 100), 100);
    }
}
