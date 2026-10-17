//! Phase One's IIQ L and IIQ S codings, knowing no container: each row
//! coded on its own, from an offset of its own, its bits read most
//! significant first from 32-bit words in the file's byte order. The even
//! and odd columns are two runs of differences; every eight columns a
//! short prefix code may change each run's difference length (one of ten),
//! and a length of 14 means a 16-bit value instead of a difference; the
//! columns past the last whole group of eight are 16-bit values. IIQ L
//! keeps 14-bit values (stored times 4) or 16-bit ones; IIQ S (the older
//! one) maps values below 256 through a square-law curve first. IIQ Sv2
//! (`row_sv2`) codes each group of eight columns' lengths and a width, and
//! quantised differences.
//!
//! Sources: IIQ L and S written from prior general knowledge of the format
//! (no decoder source consulted; no public description found), then
//! verified by an exact match with LibRaw's output on every IIQ L and IIQ S
//! file of the test dataset. IIQ Sv2 worked out black-box: LibRaw's output
//! for streams written to test each code (lengths, widths, the top length,
//! the tail, values kept to 14 bits), and for random streams of the
//! coding; then an exact match on every IIQ Sv2 file of the test dataset.
//! LibRaw's output for a width over its length by more than two (no camera
//! writes one) is not followed: such a width adds nothing.
use super::bits::Words;
use crate::error::{Error, Result, corrupt};
use crate::io::Order;
use std::sync::atomic::{AtomicBool, Ordering};

/// The difference lengths a prefix code selects.
const LENGTHS: [u32; 10] = [8, 7, 6, 9, 11, 10, 5, 12, 14, 13];
/// The length meaning a whole 16-bit value.
const LITERAL: u32 = 14;

/// What a row's decoded values become.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    /// IIQ L of 14-bit values: times 4.
    Times4,
    /// IIQ L16: as they are.
    Plain,
    /// IIQ S: values below 256 through the curve, then times 4.
    Curve,
}

/// IIQ S's curve over values below 256: the square of the value over
/// 3.969, rounded.
fn curve(v: u16) -> u16 {
    (f64::from(v) * f64::from(v) / 3.969 + 0.5) as u16
}

/// Decodes one row of `out.len()` columns from `src` (zeros past its end).
pub(crate) fn row(src: &[u8], order: Order, output: Output, out: &mut [u16]) {
    let mut pump = Words::new(src, order);
    let mut pred = [0i32; 2];
    let mut len = [0u32; 2];
    let whole = out.len() & !7;
    for (col, o) in out.iter_mut().enumerate() {
        if col >= whole {
            len = [LITERAL; 2];
        } else if col % 8 == 0 {
            for l in &mut len {
                // Up to four zeros then a one select a pair of lengths, the
                // next bit one of them; a one at once keeps the length.
                let mut zeros = 0;
                while zeros < 5 && pump.get(1) == 0 {
                    zeros += 1;
                }
                if zeros > 0 {
                    *l = LENGTHS[(zeros - 1) * 2 + pump.get(1) as usize];
                }
            }
        }
        let c = col & 1;
        pred[c] = match len[c] {
            LITERAL => pump.get(16) as i32,
            0 => pred[c],
            n => pred[c]
                .wrapping_add(pump.get(n) as i32)
                .wrapping_add(1 - (1 << (n - 1))),
        };
        // Values are 16 bits: a run past them wraps as it is stored.
        let v = pred[c] as u16;
        *o = match output {
            Output::Plain => v,
            Output::Times4 => v.wrapping_shl(2),
            Output::Curve if v < 256 => curve(v).wrapping_shl(2),
            Output::Curve => v.wrapping_shl(2),
        };
    }
}

/// Decodes `out.len() / width` rows of `width` columns, row `r` at
/// `offsets[r]` bytes into `data`.
pub(crate) fn decode(
    data: &[u8],
    offsets: &[u32],
    order: Order,
    output: Output,
    width: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    if width == 0 {
        return Err(corrupt("IIQ rows of no columns"));
    }
    // The most a row can take: two 5-bit codes and eight 16-bit values per
    // eight columns, and a word.
    let most = width.div_ceil(8).saturating_mul(36).saturating_add(8);
    for (r, dst) in out.chunks_exact_mut(width).enumerate() {
        if r % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let at = *offsets.get(r).ok_or_else(|| corrupt("IIQ row offsets"))? as usize;
        let src = data
            .get(at..)
            .ok_or_else(|| corrupt("IIQ row past the end of its data"))?;
        row(&src[..src.len().min(most)], order, output, dst);
    }
    Ok(())
}

/// IIQ Sv2's top length, whose values are stored as they are: 14 bits.
const SV2_LITERAL: u32 = 12;
/// IIQ Sv2's values: 14 bits.
const SV2_MAX: i32 = (1 << 14) - 1;

/// Reads an IIQ Sv2 length code: a run of up to seven zeros, then (fewer
/// than seven) a one; the run and a bit after it select the length,
/// relative to `prev` (the run's own last length) or absolute; seven zeros
/// mean the top length.
fn sv2_length(pump: &mut Words, prev: u32) -> Result<u32> {
    let mut zeros = 0;
    while zeros < 7 && pump.get(1) == 0 {
        zeros += 1;
    }
    let n = match zeros {
        0 => prev.checked_add(pump.get(1)),
        1 => prev.checked_sub(1),
        2 => Some([6, 5][pump.get(1) as usize]),
        3 => Some([4, 7][pump.get(1) as usize]),
        4 => Some([9, 8][pump.get(1) as usize]),
        5 => Some([3, 10][pump.get(1) as usize]),
        6 => Some(11),
        _ => Some(SV2_LITERAL),
    };
    // Lengths past 16 bits (or of none) are no coding of 14-bit values.
    n.filter(|n| (1..=16).contains(n))
        .ok_or_else(|| corrupt("IIQ Sv2 length"))
}

/// Decodes one IIQ Sv2 row of `out.len()` columns from `src` (zeros past
/// its end): a word, a bit, the first eight columns' 14-bit values; then
/// each whole group of eight columns a length code for the even columns,
/// one for the odd, and a width code ("1" 3 bits, else "0" and two bits
/// over 4); a column of the top length a 14-bit value, any other `width`
/// bits `b` added to the value two columns left: `b` shifted up by the
/// length less the width (nothing when the width is the greater), less
/// half the length's range, plus one. Values are kept to 14 bits (the
/// run goes on from the unkept one), then stored times 4. The columns past
/// the last whole group of eight are 14-bit values.
pub(crate) fn row_sv2(src: &[u8], order: Order, out: &mut [u16]) -> Result<()> {
    let mut pump = Words::new(src, order);
    pump.skip(32);
    pump.skip(1);
    let first = out.len().min(8);
    let whole = out.len() & !7;
    let mut pred = [0i32; 2];
    let mut len = [SV2_LITERAL; 2];
    let mut width = 3;
    for (col, o) in out.iter_mut().enumerate() {
        let c = col & 1;
        let literal = if col < first || col >= whole.max(8) {
            true
        } else {
            if col % 8 == 0 {
                len[0] = sv2_length(&mut pump, len[0])?;
                len[1] = sv2_length(&mut pump, len[1])?;
                width = if pump.get(1) == 1 { 3 } else { 4 + pump.get(2) };
            }
            len[c] == SV2_LITERAL
        };
        pred[c] = if literal {
            pump.get(14) as i32
        } else {
            let n = len[c];
            let b = pump.get(width) as i32;
            let up = if n >= width { b << (n - width) } else { 0 };
            pred[c]
                .wrapping_add(up)
                .wrapping_sub(1 << (n - 1))
                .wrapping_add(1)
        };
        *o = (pred[c].clamp(0, SV2_MAX) as u16) << 2;
    }
    Ok(())
}

/// Decodes `out.len() / width` IIQ Sv2 rows of `width` columns, row `r` at
/// `offsets[r]` bytes into `data`.
pub(crate) fn decode_sv2(
    data: &[u8],
    offsets: &[u32],
    order: Order,
    width: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    if width == 0 {
        return Err(corrupt("IIQ rows of no columns"));
    }
    // The most a row can take: a word and a bit, then per eight columns two
    // 7-bit codes, a 3-bit one and eight 14-bit values.
    let most = width
        .div_ceil(8)
        .saturating_mul(129)
        .div_ceil(8)
        .saturating_add(8);
    for (r, dst) in out.chunks_exact_mut(width).enumerate() {
        if r % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let at = *offsets.get(r).ok_or_else(|| corrupt("IIQ row offsets"))? as usize;
        let src = data
            .get(at..)
            .ok_or_else(|| corrupt("IIQ row past the end of its data"))?;
        row_sv2(&src[..src.len().min(most)], order, dst)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bits written most significant first into little-endian 32-bit words.
    struct Writer {
        bits: Vec<bool>,
    }
    impl Writer {
        fn put(&mut self, v: u32, n: u32) {
            for i in (0..n).rev() {
                self.bits.push(v >> i & 1 == 1);
            }
        }
        fn words(&self) -> Vec<u8> {
            let mut out = Vec::new();
            for chunk in self.bits.chunks(32) {
                let mut w = 0u32;
                for (i, &b) in chunk.iter().enumerate() {
                    w |= u32::from(b) << (31 - i);
                }
                out.extend(w.to_le_bytes());
            }
            out
        }
    }

    #[test]
    fn a_row_of_differences_and_literals() {
        // Ten columns: a group of eight, then two literal columns. The even
        // run's code "0001" + "1" selects LENGTHS[(3 - 1) * 2 + 1] = 10 bits, the
        // odd run's "1" keeps length 0 (no bits: the value stays 0); then
        // values follow.
        let mut w = Writer { bits: Vec::new() };
        w.put(0b0001, 4);
        w.put(1, 1);
        w.put(1, 1);
        // Even columns: differences of 10 bits, value v + 1 - 512.
        let mut expect = vec![0u16; 10];
        let mut pred = 0i32;
        for (i, d) in [700u32, 511, 520, 0].into_iter().enumerate() {
            pred += d as i32 + 1 - 512;
            expect[2 * i] = pred as u16;
            // An odd column of length 0 between them reads no bits.
            w.put(d, 10);
        }
        // The two literal columns.
        w.put(0x1234, 16);
        w.put(0xfedc, 16);
        expect[8] = 0x1234;
        expect[9] = 0xfedc;
        let mut out = vec![0; 10];
        row(&w.words(), Order::Little, Output::Plain, &mut out);
        assert_eq!(out, expect);
        let mut out4 = vec![0; 10];
        row(&w.words(), Order::Little, Output::Times4, &mut out4);
        assert_eq!(out4[0], expect[0].wrapping_shl(2));
    }

    #[test]
    fn the_older_s_coding_curves_low_values() {
        assert_eq!(curve(0), 0);
        assert_eq!(curve(100), 2520);
        assert_eq!(curve(255), 16383);
        let mut w = Writer { bits: Vec::new() };
        for v in [100u32, 300] {
            w.put(v, 16);
        }
        // Two columns: both literal.
        let mut out = vec![0; 2];
        row(&w.words(), Order::Little, Output::Curve, &mut out);
        assert_eq!(out, [2520 * 4, 1200]);
    }

    #[test]
    fn an_sv2_row_of_lengths_widths_and_literals() {
        let mut w = Writer { bits: Vec::new() };
        w.put(0x0003_0001, 32);
        w.put(1, 1);
        // Columns 0 to 7: 14-bit values.
        for i in 0..8 {
            w.put(1000 + 100 * i, 14);
        }
        // Columns 8 to 15: even length "0011" (5), odd "10" (the top
        // length kept: 14-bit values), width "1" (3). An even value adds
        // b << 2, less 16, plus 1.
        w.put(0b0011, 4);
        w.put(0b10, 2);
        w.put(1, 1);
        for (b, odd) in [(4, 10), (7, 11), (0, 12), (4, 13)] {
            w.put(b, 3);
            w.put(odd, 14);
        }
        // Columns 16 to 23: even seven zeros (the top length), odd
        // "0000010" (3), width "011" (7): a width over the length adds
        // nothing, so each odd value is 4 less 1 below the last.
        w.put(0, 7);
        w.put(0b0000010, 7);
        w.put(0b011, 3);
        for even in [5, 6, 7, 8] {
            w.put(even, 14);
            w.put(0x7f, 7);
        }
        // Columns 24 to 31: even "01" (11), odd "01" (2), width "1" (3).
        // Values below 0 are kept at 0, the run going on from them.
        w.put(0b01, 2);
        w.put(0b01, 2);
        w.put(1, 1);
        for b in [0, 7, 7, 4] {
            w.put(b, 3);
            w.put(5, 3);
        }
        // Columns 32 to 35, past the last group of eight: 14-bit values.
        for v in 9000..9004 {
            w.put(v, 14);
        }
        let mut out = vec![0u16; 36];
        row_sv2(&w.words(), Order::Little, &mut out).unwrap();
        let expect: Vec<u16> = [
            1000, 1100, 1200, 1300, 1400, 1500, 1600, 1700, // literals
            1601, 10, 1614, 11, 1599, 12, 1600, 13, // lengths 5 and top
            5, 10, 6, 7, 7, 4, 8, 1, // top and 3, width 7
            0, 0, 0, 0, 523, 0, 524, 0, // 11 and 2, kept at 0
            9000, 9001, 9002, 9003,
        ]
        .iter()
        .map(|v| v << 2)
        .collect();
        assert_eq!(out, expect);
        // A length run below 1 or past 16 is corrupt.
        let below = [0u8, 0, 0, 0x40];
        assert!(sv2_length(&mut Words::new(&below, Order::Little), 1).is_err());
        let past = [0u8, 0, 0, 0xc0];
        assert!(sv2_length(&mut Words::new(&past, Order::Little), 16).is_err());
        // Truncated and garbage rows decode or fail without panicking.
        let words = w.words();
        for end in 0..words.len() {
            let _ = row_sv2(&words[..end], Order::Little, &mut out);
        }
        let garbage: Vec<u8> = (0..200u32).map(|i| (i * 7919 % 251) as u8).collect();
        let _ = row_sv2(&garbage, Order::Little, &mut out);
    }

    #[test]
    fn rows_need_their_offsets_and_stop_when_cancelled() {
        let data = [0u8; 64];
        let mut out = vec![0; 16];
        let cancel = AtomicBool::new(false);
        assert!(
            decode(
                &data,
                &[0],
                Order::Little,
                Output::Plain,
                8,
                &mut out,
                &cancel
            )
            .is_err()
        );
        assert!(
            decode(
                &data,
                &[0, 100],
                Order::Little,
                Output::Plain,
                8,
                &mut out,
                &cancel
            )
            .is_err()
        );
        assert!(
            decode(
                &data,
                &[0, 4],
                Order::Little,
                Output::Plain,
                8,
                &mut out,
                &cancel
            )
            .is_ok()
        );
        assert_eq!(
            decode(
                &data,
                &[0, 4],
                Order::Little,
                Output::Plain,
                8,
                &mut out,
                &AtomicBool::new(true)
            ),
            Err(Error::Cancelled)
        );
    }
}
