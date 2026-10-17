//! Panasonic's prefix-coded strips (RW2 raw format 8), from bytes to
//! samples. The image is cut into vertical strips, each an independent bit
//! stream whose bits are taken from each byte starting at its lowest bit.
//! Samples are coded as differences the way ITU T.81 codes lossless JPEG
//! differences (a prefix code for the category `c`, then `c` bits, a leading
//! 0 bit meaning a negative difference, F.1.2.1), with a table of 17 (length,
//! code) pairs for the categories 0 to 16; a shorter code shadows longer ones
//! it starts. Pixels are coded two rows at a time, by 2 × 2 cells from left
//! to right, each cell top left, bottom left, top right, bottom right; each
//! sample is the difference added to the same site of the cell before it,
//! the first cell of a row pair to the first cell of the pair above, 0 at
//! the top.
//!
//! Sources: ITU T.81 for the differences; our own measurements, comparing
//! the decoded samples with LibRaw's recorded output on our test dataset.
use crate::error::{Error, Result, corrupt};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// A strip's bits, each byte's from its lowest up, as one number read from
/// its highest bit: the bytes are reversed as they are read, not copied.
/// Past the end of the data it reads zeros.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    /// Bits not yet consumed, left aligned.
    cache: u64,
    bits: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            cache: 0,
            bits: 0,
        }
    }
    #[inline]
    fn refill(&mut self) {
        while self.bits <= 56 {
            let b = self.data.get(self.pos).map_or(0, |b| b.reverse_bits());
            self.pos += 1;
            self.cache |= u64::from(b) << (56 - self.bits);
            self.bits += 8;
        }
    }
    /// The next `n` bits (at most 32) without consuming them.
    #[inline]
    fn peek(&mut self, n: u32) -> u32 {
        if self.bits < n {
            self.refill();
        }
        (self.cache >> (64 - n)) as u32
    }
    #[inline]
    fn skip(&mut self, n: u32) {
        if self.bits < n {
            self.refill();
        }
        self.cache <<= n;
        self.bits -= n;
    }
    #[inline]
    fn get(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek(n);
        self.skip(n);
        v
    }
}

/// One strip: its coded bytes and its columns.
pub(crate) struct Strip<'a> {
    pub data: &'a [u8],
    pub left: usize,
    pub width: usize,
}

/// The prefix codes of the 17 categories, looked up by the next 16 bits.
pub(crate) struct Codes {
    /// By the next 16 bits: the code's length (0 for none) and category.
    table: Vec<(u8, u8)>,
}

impl Codes {
    /// From (length, code) per category 0 to 16; length 0 is no code.
    pub fn new(pairs: &[(u32, u32)]) -> Result<Self> {
        if pairs.len() != 17 {
            return Err(corrupt("Panasonic code table size"));
        }
        let mut table = vec![(0u8, 0u8); 1 << 16];
        // Shorter codes first, so a code another starts with wins.
        let mut order: Vec<usize> = (0..pairs.len()).collect();
        order.sort_by_key(|&c| pairs[c].0);
        for c in order {
            let (len, code) = pairs[c];
            if len == 0 {
                continue;
            }
            if len > 16 || code >> len != 0 {
                return Err(corrupt("Panasonic prefix code"));
            }
            let shift = 16 - len;
            let start = (code << shift) as usize;
            for e in &mut table[start..start + (1 << shift)] {
                if e.0 == 0 {
                    *e = (len as u8, c as u8);
                }
            }
        }
        Ok(Self { table })
    }
    /// The next difference.
    #[inline]
    fn difference(&self, bits: &mut Bits) -> Result<i32> {
        let (len, category) = self.table[bits.peek(16) as usize];
        if len == 0 {
            return Err(corrupt("invalid Panasonic prefix code"));
        }
        bits.skip(u32::from(len));
        let c = u32::from(category);
        if c == 0 {
            return Ok(0);
        }
        let v = bits.get(c) as i32;
        Ok(if v < 1 << (c - 1) {
            v - (1 << c) + 1
        } else {
            v
        })
    }
}

/// Decodes `width × height` samples from `strips`, which side by side cover
/// every column, each the full height.
pub(crate) fn decode(
    strips: &[Strip],
    codes: &Codes,
    width: u32,
    height: u32,
    cancel: &AtomicBool,
) -> Result<Vec<u16>> {
    let n = crate::io::samples(width, height, 1)?;
    let (width, height) = (width as usize, height as usize);
    let mut x = 0;
    for s in strips {
        if s.left != x || s.width == 0 {
            return Err(corrupt("Panasonic strips do not cover the image"));
        }
        x += s.width;
    }
    if x != width {
        return Err(corrupt("Panasonic strips do not cover the image"));
    }
    // A site takes at most a 16-bit code and 16 more bits, and every 2 × 2
    // cell is coded whole, also past an odd width or height: longer data is
    // not a strip of this size.
    for s in strips {
        let cells = s.width.div_ceil(2).saturating_mul(height.div_ceil(2));
        let most = cells.saturating_mul(16).saturating_add(16);
        if s.data.len() > most {
            return Err(corrupt("Panasonic strip longer than its samples"));
        }
    }
    let decoded = strips
        .par_iter()
        .map(|s| strip(s, codes, height, cancel))
        .collect::<Result<Vec<_>>>()?;
    let mut out = vec![0u16; n];
    for (s, samples) in strips.iter().zip(decoded) {
        for (row, src) in out
            .chunks_exact_mut(width)
            .zip(samples.chunks_exact(s.width))
        {
            row[s.left..s.left + s.width].copy_from_slice(src);
        }
    }
    Ok(out)
}

fn strip(s: &Strip, codes: &Codes, height: usize, cancel: &AtomicBool) -> Result<Vec<u16>> {
    let width = s.width;
    let mut bits = Bits::new(s.data);
    let mut out = vec![0u16; width * height];
    let mut first = [0i32; 4];
    for pair in 0..height.div_ceil(2) {
        if pair % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut cell = first;
        for cx in 0..width.div_ceil(2) {
            for (k, v) in cell.iter_mut().enumerate() {
                *v = v
                    .checked_add(codes.difference(&mut bits)?)
                    .ok_or_else(|| corrupt("Panasonic strip sample out of range"))?;
                let (y, x) = (2 * pair + k % 2, 2 * cx + k / 2);
                if y < height && x < width {
                    out[y * width + x] = (*v).clamp(0, 0xffff) as u16;
                }
            }
            if cx == 0 {
                first = cell;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Codes: category 0 "0", 1 "10", 2 "110", 3 "1110", the rest none.
    fn codes() -> Codes {
        let mut pairs = vec![(1, 0b0), (2, 0b10), (3, 0b110), (4, 0b1110)];
        pairs.resize(17, (0, 0));
        Codes::new(&pairs).unwrap()
    }

    /// Bits written most significant first, then each byte's bits reversed
    /// (the stream takes them from the lowest bit up).
    fn stream(bits: &str) -> Vec<u8> {
        let bits: Vec<u8> = bits
            .bytes()
            .filter(|b| *b != b' ')
            .map(|b| b - b'0')
            .collect();
        bits.chunks(8)
            .map(|c| {
                let byte = c
                    .iter()
                    .enumerate()
                    .fold(0u8, |a, (i, &b)| a | b << (7 - i));
                byte.reverse_bits()
            })
            .collect()
    }

    #[test]
    fn cells_follow_the_cell_before_and_the_row_pair_above() {
        // A 4 × 4 strip: row pair 0 cells (+3, +1, −1, +2) then each +1
        // (category 1, bit 1); row pair 1's first cell from pair 0's first:
        // all 0 differences, its second all −1 (category 1, bit 0).
        let d3 = "110 11"; // +3: category 2, bits 11
        let d1 = "10 1";
        let dm1 = "10 0";
        let d2 = "110 10";
        let zero = "0";
        let pair0 = [d3, d1, dm1, d2, d1, d1, d1, d1].join(" ");
        let pair1 = [zero, zero, zero, zero, dm1, dm1, dm1, dm1].join(" ");
        let data = stream(&format!("{pair0} {pair1}"));
        let s = Strip {
            data: &data,
            left: 0,
            width: 4,
        };
        let out = decode(&[s], &codes(), 4, 4, &AtomicBool::new(false)).unwrap();
        // Cell sites: TL, BL, TR, BR = rows (0,1,0,1), columns (0,0,1,1).
        assert_eq!(
            out,
            [
                3, 0, 4, 0, //
                1, 2, 2, 3, //
                3, 0, 2, 0, //
                1, 2, 0, 1, //
            ]
        );
    }

    #[test]
    fn overflowing_predictions_and_overlong_strips_are_errors() {
        // Category 16 with all ones: +65535 at every sample of one site,
        // which would overflow 32 bits after some 32769 cells.
        let mut pairs = vec![(1, 0b0); 16];
        pairs.push((1, 0b1));
        let codes = Codes::new(&pairs).unwrap();
        let (width, height) = (65_534u32, 6u32);
        let data = vec![0xffu8; (width * height) as usize * 4];
        let s = Strip {
            data: &data,
            left: 0,
            width: width as usize,
        };
        let r = decode(&[s], &codes, width, height, &AtomicBool::new(false));
        assert!(matches!(r, Err(Error::Corrupt(_))), "{r:?}");
        // A strip far longer than its samples could take.
        let data = vec![0u8; 1000];
        let s = Strip {
            data: &data,
            left: 0,
            width: 2,
        };
        let r = decode(&[s], &codes, 2, 2, &AtomicBool::new(false));
        assert!(matches!(r, Err(Error::Corrupt(_))), "{r:?}");
        // Odd sizes code whole cells: a 1 × 3 strip may hold two cells' worst
        // case (16 bytes each) and the slack.
        let data = vec![0u8; 2 * 16 + 16];
        let s = Strip {
            data: &data,
            left: 0,
            width: 1,
        };
        let r = decode(&[s], &codes, 1, 3, &AtomicBool::new(false));
        assert!(
            !matches!(&r, Err(Error::Corrupt(m)) if m.contains("longer")),
            "{r:?}"
        );
    }

    #[test]
    fn shorter_codes_shadow_longer_ones_and_bad_streams_fail() {
        // Category 4's code "1100" starts with category 2's "110".
        let mut pairs = vec![(1, 0b0), (2, 0b10), (3, 0b110), (4, 0b1110), (4, 0b1100)];
        pairs.resize(17, (0, 0));
        let codes = Codes::new(&pairs).unwrap();
        let data = stream("110 01 0 0 0");
        let s = Strip {
            data: &data,
            left: 0,
            width: 2,
        };
        let out = decode(&[s], &codes, 2, 2, &AtomicBool::new(false)).unwrap();
        // −2, clamped to 0; read as category 4 it would be +8.
        assert_eq!(out, [0, 0, 0, 0]);
        // "1111" is no code: an error, not a panic.
        let data = stream("1111 1111");
        let s = Strip {
            data: &data,
            left: 0,
            width: 2,
        };
        assert!(decode(&[s], &codes, 2, 2, &AtomicBool::new(false)).is_err());
        // Strips that do not cover the image.
        let s = Strip {
            data: &[],
            left: 1,
            width: 2,
        };
        assert!(decode(&[s], &codes, 2, 2, &AtomicBool::new(false)).is_err());
        assert!(Codes::new(&pairs[..5]).is_err());
    }
}
