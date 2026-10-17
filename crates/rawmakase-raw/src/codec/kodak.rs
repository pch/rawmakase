//! Kodak's coding of DCR data (TIFF compression 65000), knowing no
//! container: each row in blocks of up to 256 samples. A block starts with
//! a length (0 to 12 bits) per sample, two to a byte (low nibble first),
//! then each sample's difference in that many bits, read least significant
//! first from 16-bit big-endian halves of 32-bit groups (JPEG's sign rule),
//! summed per colour (even and odd columns) from 0. A block whose lengths
//! do not read (one over 12) is stored plainly instead: groups of eight
//! 12-bit samples in six 16-bit words, the top four bits of the first six
//! words holding the first two samples.
//!
//! Sources: written from prior general knowledge of the format (no decoder
//! source consulted, no public description found), and verified by an exact
//! match with LibRaw's output on every DCR of the test dataset.
use crate::error::{Error, Result};
use crate::io::Order;
use std::sync::atomic::{AtomicBool, Ordering};

/// The samples of a block.
const BLOCK: usize = 256;

/// A reader of the stream's bytes, zeros past its end.
struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bytes<'_> {
    fn byte(&mut self) -> u8 {
        let b = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos = self.pos.saturating_add(1);
        b
    }
}

/// Decodes one block of `out.len()` samples (at most 256); whether it was
/// stored plainly (its samples are values, not differences).
fn block(src: &mut Bytes, order: Order, out: &mut [i32]) -> bool {
    let n = out.len();
    let size = (n + 3) & !3;
    let start = src.pos;
    let mut lengths = [0u8; BLOCK];
    let mut plain = false;
    for [low, high] in lengths[..size].as_chunks_mut::<2>().0 {
        let c = src.byte();
        *low = c & 15;
        *high = c >> 4;
        let pair = [*low, *high];
        if pair[0] > 12 || pair[1] > 12 {
            plain = true;
            break;
        }
    }
    if plain {
        src.pos = start;
        let mut words = [0u16; 6];
        for group in (0..size).step_by(8) {
            for w in &mut words {
                let b = [src.byte(), src.byte()];
                *w = match order {
                    Order::Little => u16::from_le_bytes(b),
                    Order::Big => u16::from_be_bytes(b),
                };
            }
            let top = |a: usize, b: usize, c: usize| {
                i32::from(words[a] >> 12) << 8
                    | i32::from(words[b] >> 12) << 4
                    | i32::from(words[c] >> 12)
            };
            let mut values = [0i32; 8];
            values[0] = top(0, 2, 4);
            values[1] = top(1, 3, 5);
            for (v, w) in values[2..].iter_mut().zip(words) {
                *v = i32::from(w & 0xfff);
            }
            for (i, v) in values.into_iter().enumerate() {
                if let Some(o) = out.get_mut(group + i) {
                    *o = v;
                }
            }
        }
        return true;
    }
    let mut buf: u64 = 0;
    let mut bits = 0u32;
    if size & 7 == 4 {
        buf = u64::from(src.byte()) << 8 | u64::from(src.byte());
        bits = 16;
    }
    for (o, &len) in out.iter_mut().zip(&lengths) {
        let len = u32::from(len);
        if bits < len {
            // Four bytes: two 16-bit big-endian halves, the first low.
            let b = [src.byte(), src.byte(), src.byte(), src.byte()];
            let word = u64::from(b[0]) << 8
                | u64::from(b[1])
                | u64::from(b[2]) << 24
                | u64::from(b[3]) << 16;
            buf |= word << bits;
            bits += 32;
        }
        let v = (buf & ((1u64 << len) - 1)) as i32;
        buf >>= len;
        bits -= len;
        *o = if len == 0 {
            0
        } else if v & (1 << (len - 1)) == 0 {
            v - ((1 << len) - 1)
        } else {
            v
        };
    }
    false
}

/// Decodes `out.len() / width` rows of `width` samples from `src` (codes; a
/// container takes them through its curve).
pub(crate) fn decode(
    src: &[u8],
    order: Order,
    width: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    if width == 0 {
        return Err(crate::error::corrupt("Kodak rows of no samples"));
    }
    let mut bytes = Bytes { data: src, pos: 0 };
    let mut buf = [0i32; BLOCK];
    for (y, row) in out.chunks_exact_mut(width).enumerate() {
        if y % 16 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        for dst in row.chunks_mut(BLOCK) {
            let n = dst.len();
            let plain = block(&mut bytes, order, &mut buf[..n]);
            let mut pred = [0i32; 2];
            for (i, (o, &v)) in dst.iter_mut().zip(&buf[..n]).enumerate() {
                let value = if plain {
                    v
                } else {
                    pred[i & 1] = pred[i & 1].wrapping_add(v);
                    pred[i & 1]
                };
                // A corrupt stream's values wrap.
                *o = value as u16;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_of_differences_and_a_plain_one() {
        // Four samples: lengths 2, 0, 3, 1 (bytes 0x02, 0x13), then the
        // bits least significant first: 0b11 (+3), none, 0b010 (-5), 0b1 (+1)
        // in one 32-bit group (the low half big-endian).
        let bits: u32 = 0b11 | (0b010 << 2) | (0b1 << 5);
        let mut s = vec![0x02, 0x13];
        // size 4 & 7 == 4: a 16-bit prefetch.
        s.extend([(bits >> 8) as u8, bits as u8]);
        let mut out = [0u16; 4];
        decode(&s, Order::Big, 4, &mut out, &AtomicBool::new(false)).unwrap();
        // Even columns: 3, 3 - 5 = -2; odd: 0, 1.
        assert_eq!(out, [3, 0, (-2i32) as u16, 1]);
        // A length over 12: plain groups of six words.
        let words: [u16; 6] = [0x1abc, 0x2def, 0x3123, 0x4456, 0x5789, 0x6aaa];
        let mut s = Vec::new();
        for w in words {
            s.extend(w.to_be_bytes());
        }
        let mut out = [0u16; 8];
        decode(&s, Order::Big, 8, &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            out,
            [0x135, 0x246, 0xabc, 0xdef, 0x123, 0x456, 0x789, 0xaaa]
        );
    }

    #[test]
    fn truncated_streams_and_cancellation() {
        let mut out = vec![0u16; 600];
        for len in 0..40 {
            let s = vec![0x5a; len];
            let _ = decode(&s, Order::Big, 300, &mut out, &AtomicBool::new(false));
        }
        assert_eq!(
            decode(&[0; 8], Order::Big, 300, &mut out, &AtomicBool::new(true)),
            Err(Error::Cancelled)
        );
    }
}
