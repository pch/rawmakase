//! Phase One's IIQ L and IIQ S codings, knowing no container: each row
//! coded on its own, from an offset of its own, its bits read most
//! significant first from 32-bit words in the file's byte order. The even
//! and odd columns are two runs of differences; every eight columns a
//! short prefix code may change each run's difference length (one of ten),
//! and a length of 14 means a 16-bit value instead of a difference; the
//! columns past the last whole group of eight are 16-bit values. IIQ L
//! keeps 14-bit values (stored times 4) or 16-bit ones; IIQ S (the older
//! one) maps values below 256 through a square-law curve first.
//!
//! Sources: written from prior general knowledge of the format (no decoder
//! source consulted; no public description found), then verified by an
//! exact match with LibRaw's output on every IIQ L and IIQ S file of the
//! test dataset.
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
