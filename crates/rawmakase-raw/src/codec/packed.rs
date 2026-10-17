//! Packed samples, knowing no container: rows of `bits`-bit samples read
//! most significant bit first from a stream of little-endian words of one to
//! four bytes (one byte: plain big-endian bit packing), optionally with a
//! pad byte after every ten samples, the two samples of each pair swapped,
//! or each row's words in reverse; or 16-bit words, one sample each.
//!
//! Sources: TIFF 6 (bit packing of samples, most significant first) and our
//! own measurements of each camera's word size, padding and order against
//! LibRaw's output on our files.
use crate::error::{Error, Result};
use crate::io::Order;
use std::sync::atomic::{AtomicBool, Ordering};

/// How a row's samples are packed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Packing {
    pub bits: u32,
    /// Bytes per word (1 to 4), each word little-endian.
    pub word: usize,
    /// A pad byte after every ten samples.
    pub pad: bool,
    /// The samples of each pair of columns swapped.
    pub swap: bool,
}

impl Packing {
    /// Plain bit packing, most significant bit first.
    pub fn msb(bits: u32) -> Self {
        Self {
            bits,
            word: 1,
            pad: false,
            swap: false,
        }
    }
    /// The bytes a row of `width` samples takes, its padding included.
    pub fn row_bytes(&self, width: usize) -> usize {
        let bytes = (width * self.bits as usize).div_ceil(8);
        if self.pad { bytes + width / 10 } else { bytes }
    }
}

/// Unpacks one row of `out.len()` samples from `src` (zeros past its end).
pub(crate) fn row(src: &[u8], p: Packing, out: &mut [u16]) {
    let word = p.word.clamp(1, 4);
    let mut bytes = Vec::with_capacity(src.len());
    // The pad bytes dropped: each group of ten samples is followed by one.
    let group = if p.pad {
        (10 * p.bits as usize).div_ceil(8)
    } else {
        usize::MAX
    };
    let mut at = 0;
    while at < src.len() {
        let end = at.saturating_add(group).min(src.len());
        bytes.extend_from_slice(&src[at..end]);
        at = end.saturating_add(usize::from(p.pad));
    }
    // Each little-endian word's bytes, most significant first.
    for w in bytes.chunks_mut(word) {
        w.reverse();
    }
    let mut pump = crate::codec::bits::Msb::new(&bytes);
    for x in 0..out.len() {
        let v = pump.get(p.bits) as u16;
        let to = if p.swap { x ^ 1 } else { x };
        if let Some(o) = out.get_mut(to) {
            *o = v;
        }
    }
}

/// Unpacks `out.len() / width` rows of `width` samples, each `stride`
/// bytes from the last, the row `r` of the image at `rows(r)` bytes into
/// `src` (rows past its end left zero).
pub(crate) fn rows(
    src: &[u8],
    p: Packing,
    width: usize,
    stride: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    for (y, dst) in out.chunks_exact_mut(width).enumerate() {
        if y % 256 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let Some(start) = y.checked_mul(stride).filter(|&s| s < src.len()) else {
            break;
        };
        let end = start.saturating_add(stride).min(src.len());
        row(&src[start..end], p, dst);
    }
    Ok(())
}

/// Rows of `width` samples stored as two fields, `stride` bytes a row: the
/// even rows, then from the next 2 KiB boundary the odd ones (Olympus's and
/// Minolta's compacts; our measurements).
pub(crate) fn fields(
    src: &[u8],
    p: Packing,
    width: usize,
    stride: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    let height = out.len() / width.max(1);
    let even = height.div_ceil(2);
    let second = (even * stride).next_multiple_of(2048);
    let mut field = crate::io::zeroed(width * even)?;
    for (start, first) in [(0, 0), (second, 1)] {
        field.fill(0);
        let part = src.get(start..).unwrap_or(&[]);
        rows(part, p, width, stride, &mut field, cancel)?;
        for (i, row) in field.chunks_exact(width).enumerate() {
            if i % 256 == 0 && cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let y = 2 * i + first;
            if y < height {
                out[y * width..(y + 1) * width].copy_from_slice(row);
            }
        }
    }
    Ok(())
}

/// 16-bit words, one sample each, shifted right by `shift`.
pub(crate) fn words(
    src: &[u8],
    order: Order,
    shift: u32,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    for (i, (o, w)) in out.iter_mut().zip(src.as_chunks::<2>().0).enumerate() {
        if i % (1 << 20) == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let v = match order {
            Order::Little => u16::from_le_bytes(*w),
            Order::Big => u16::from_be_bytes(*w),
        };
        *o = v >> shift.min(15);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_packing_is_most_significant_first() {
        let mut out = [0u16; 2];
        row(&[0xab, 0xcd, 0xef], Packing::msb(12), &mut out);
        assert_eq!(out, [0xabc, 0xdef]);
    }

    #[test]
    fn words_are_little_endian_and_read_from_their_top() {
        // Two 12-bit samples in a 3-byte little-endian word 0xefcdab.
        let p = Packing {
            word: 3,
            ..Packing::msb(12)
        };
        let mut out = [0u16; 2];
        row(&[0xab, 0xcd, 0xef], p, &mut out);
        assert_eq!(out, [0xefc, 0xdab]);
        // Swapped pairs.
        row(&[0xab, 0xcd, 0xef], Packing { swap: true, ..p }, &mut out);
        assert_eq!(out, [0xdab, 0xefc]);
        // 32-bit words.
        let p = Packing {
            word: 4,
            ..Packing::msb(16)
        };
        row(&[1, 2, 3, 4], p, &mut out);
        assert_eq!(out, [0x0403, 0x0201]);
    }

    #[test]
    fn a_pad_byte_follows_every_ten_samples() {
        // Ten 12-bit samples of 0xfff (15 bytes), a pad byte 0, then two
        // samples 0x123, 0x456.
        let mut src = vec![0xff; 15];
        src.push(0);
        src.extend([0x12, 0x34, 0x56]);
        let p = Packing {
            pad: true,
            ..Packing::msb(12)
        };
        assert_eq!(p.row_bytes(20), 32);
        let mut out = [0u16; 12];
        row(&src, p, &mut out);
        assert_eq!(&out[..10], &[0xfff; 10]);
        assert_eq!(&out[10..], &[0x123, 0x456]);
    }

    #[test]
    fn short_data_leaves_zeros() {
        let mut out = vec![7u16; 8];
        rows(
            &[0xff; 3],
            Packing::msb(12),
            2,
            3,
            &mut out,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(out, [0xfff, 0xfff, 7, 7, 7, 7, 7, 7]);
        let mut out = [0u16; 3];
        words(
            &[1, 2, 3, 4, 5],
            Order::Big,
            4,
            &mut out,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(out, [0x0102 >> 4, 0x0304 >> 4, 0]);
    }
}
