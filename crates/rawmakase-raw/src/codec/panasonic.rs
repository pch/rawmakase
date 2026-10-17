//! Panasonic's block encodings of RW2, RWL and the older RAW files, from
//! bytes to samples: a continuous stream of 16-byte blocks, each holding a
//! fixed number of samples of consecutive pixels in raster order.
//!
//! - Packed blocks hold ⌊128 / bits⌋ samples of `bits` bits, the block read as
//!   a little-endian 128-bit number from its lowest bit up.
//! - Delta blocks (raw format 4 and older) hold 14 samples, the block read
//!   from its highest bit down. Pixels alternate between two colours. Before
//!   pixels 2, 5, 8 and 11 a 2-bit scale `s` sets the step `m` (1, 2, 4 or
//!   16) of the next three. A colour's first pixel is a seed (8 high bits, 4
//!   low bits); a zero high part of a seed before pixel 12 is a pixel of 0
//!   without low bits, and the colour's next pixel is its seed instead. Every
//!   later pixel is an 8-bit code `j` against the colour's last value `p`:
//!   `j = 0` keeps `p`; else `p + (j − 128)·m` when `s < 3` and `p ≥ 128·m`,
//!   else `j·m + p mod m`.
//! - Seeded delta blocks (raw format 6) are read from the highest bit down
//!   too: two seeds of `bits` bits (one per colour), then groups of a 2-bit
//!   scale and three codes, as in delta blocks with the codes' midpoint `h`
//!   for 128: 12-bit samples have four groups of 8-bit codes (14 samples),
//!   14-bit samples three groups of 10-bit codes (11 samples, the last 4 bits
//!   unused). A code `j` is `p + (j − h)·m` when `s < 3` and `p ≥ h·m`, else
//!   `j·m` (without the low bits of `p`); `j = 0` keeps `p`.
//!
//! Some files (RW2 raw formats 4 and 5) store every whole 0x4000-byte chunk
//! rotated: its content is the chunk's bytes from 0x1FF8 on, then its first
//! 0x1FF8 bytes. The last chunk is rotated whole when the file holds it.
//!
//! The oldest files store 16-bit little-endian words instead, each sample in
//! the word's high 12 bits.
//!
//! Sources: our own measurements, comparing the decoded samples with
//! LibRaw's recorded output on our test dataset.
use crate::error::{Error, Result, corrupt};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

const CHUNK: usize = 0x4000;
const ROTATION: usize = 0x1ff8;

/// What one 16-byte block holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Block {
    /// ⌊128 / bits⌋ samples of `bits` bits from the lowest bit up.
    Packed { bits: u32 },
    /// 14 samples coded as differences (raw format 4).
    Delta,
    /// Two seeds of `bits` (12 or 14) bits and codes against them (raw
    /// format 6).
    Seeded { bits: u32 },
}

impl Block {
    /// Samples per block.
    pub fn per(self) -> usize {
        match self {
            Block::Packed { bits } => (128 / bits.clamp(1, 16)) as usize,
            Block::Delta => 14,
            Block::Seeded { bits: 14 } => 11,
            Block::Seeded { .. } => 14,
        }
    }
    fn decode(self, block: &[u8; 16], out: &mut [u16]) {
        match self {
            Block::Packed { bits } => packed(block, bits, out),
            Block::Delta => delta(block, out),
            Block::Seeded { bits } => seeded(block, bits, out),
        }
    }
}

/// The bytes the blocks of `width × height` samples take.
pub(crate) fn len(block: Block, width: u32, height: u32) -> u64 {
    (u64::from(width) * u64::from(height)).div_ceil(block.per() as u64) * 16
}

/// Decodes `width × height` samples from the blocks in `src`, whole chunks
/// rotated when `rotated`. `src` must hold every block ([`len`]).
pub(crate) fn decode(
    src: &[u8],
    block: Block,
    rotated: bool,
    width: u32,
    height: u32,
    cancel: &AtomicBool,
) -> Result<Vec<u16>> {
    let n = crate::io::samples(width, height, 1)?;
    let need = usize::try_from(len(block, width, height)).map_err(|_| corrupt("RAW size"))?;
    if src.len() < need {
        return Err(corrupt(format!(
            "Panasonic RAW data ends after {} of {need} bytes",
            src.len()
        )));
    }
    let per = block.per();
    let mut out = vec![0u16; n];
    out.par_chunks_mut(CHUNK / 16 * per)
        .enumerate()
        .try_for_each(|(c, out)| {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let start = c * CHUNK;
            let stored = &src[start.min(src.len())..(start + CHUNK).min(src.len())];
            let mut buf = [0u8; CHUNK];
            if rotated {
                // Bytes past the end of the file read as zeros.
                for (i, b) in buf.iter_mut().enumerate() {
                    let at = if i < CHUNK - ROTATION {
                        ROTATION + i
                    } else {
                        i - (CHUNK - ROTATION)
                    };
                    *b = stored.get(at).copied().unwrap_or(0);
                }
            } else {
                buf[..stored.len()].copy_from_slice(stored);
            }
            let (blocks, _) = buf.as_chunks::<16>();
            for (b, px) in blocks.iter().zip(out.chunks_mut(per)) {
                block.decode(b, px);
            }
            Ok(())
        })?;
    Ok(out)
}

/// Decodes `width × height` 12-bit samples from the high bits of 16-bit
/// little-endian words in `src`.
pub(crate) fn words(src: &[u8], width: u32, height: u32, cancel: &AtomicBool) -> Result<Vec<u16>> {
    let n = crate::io::samples(width, height, 1)?;
    let (words, _) = src.as_chunks::<2>();
    let words = words
        .get(..n)
        .ok_or_else(|| corrupt("Panasonic RAW data ends early"))?;
    let mut out = vec![0u16; n];
    let row = width as usize;
    out.par_chunks_mut(row * 64)
        .zip(words.par_chunks(row * 64))
        .try_for_each(|(out, words)| {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            for (o, w) in out.iter_mut().zip(words) {
                *o = u16::from_le_bytes(*w) >> 4;
            }
            Ok(())
        })?;
    Ok(out)
}

fn packed(block: &[u8; 16], bits: u32, out: &mut [u16]) {
    let word = u128::from_le_bytes(*block);
    let mask = (1u128 << bits.clamp(1, 16)) - 1;
    for (i, o) in out.iter_mut().enumerate() {
        *o = ((word >> (i as u32 * bits)) & mask) as u16;
    }
}

/// A 16-byte block's bits from its highest down.
struct Down {
    word: u128,
    left: u32,
}

impl Down {
    fn new(block: &[u8; 16]) -> Self {
        Self {
            word: u128::from_le_bytes(*block),
            left: 128,
        }
    }
    /// The next `n` (1 to 16) bits; zeros once the block is used up.
    #[inline]
    fn get(&mut self, n: u32) -> i32 {
        if self.left < n {
            self.left = 0;
            return 0;
        }
        self.left -= n;
        ((self.word >> self.left) & ((1 << n) - 1)) as i32
    }
}

fn delta(block: &[u8; 16], out: &mut [u16]) {
    let mut bits = Down::new(block);
    let (mut last, mut seeded, mut scale) = ([0i32; 2], [false; 2], 0);
    for (i, o) in out.iter_mut().enumerate().take(14) {
        if i % 3 == 2 {
            scale = bits.get(2);
        }
        let c = i & 1;
        if seeded[c] {
            last[c] = step(last[c], bits.get(8), scale, 128, true);
        } else {
            let high = bits.get(8);
            if high != 0 || i >= 12 {
                last[c] = high << 4 | bits.get(4);
                seeded[c] = true;
            }
        }
        *o = last[c].clamp(0, 0xffff) as u16;
    }
}

/// `p` moved by code `j` at scale `scale` with the codes' midpoint `half`;
/// an absolute level keeps the low bits of `p` when `low`.
#[inline]
fn step(p: i32, j: i32, scale: i32, half: i32, low: bool) -> i32 {
    let m = [1, 2, 4, 16][(scale & 3) as usize];
    if j == 0 {
        p
    } else if scale < 3 && p >= half * m {
        p + (j - half) * m
    } else if low {
        j * m + p % m
    } else {
        j * m
    }
}

fn seeded(block: &[u8; 16], bits: u32, out: &mut [u16]) {
    let (code, groups) = if bits == 14 { (10, 3) } else { (8, 4) };
    let half = 1 << (code - 1);
    let mut b = Down::new(block);
    let mut last = [b.get(bits.min(16)), b.get(bits.min(16))];
    let mut values = [0i32; 14];
    values[..2].copy_from_slice(&last);
    for g in 0..groups {
        let scale = b.get(2);
        for k in 0..3 {
            let i = 2 + 3 * g + k;
            last[i & 1] = step(last[i & 1], b.get(code), scale, half, false);
            values[i] = last[i & 1];
        }
    }
    for (o, v) in out.iter_mut().zip(values) {
        *o = v.clamp(0, 0xffff) as u16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block written from its highest bit down: (value, bits) fields.
    fn block_down(fields: &[(u32, u32)]) -> [u8; 16] {
        let (mut word, mut at) = (0u128, 128u32);
        for &(v, n) in fields {
            at -= n;
            word |= u128::from(v) << at;
        }
        word.to_le_bytes()
    }

    #[test]
    fn delta_blocks_follow_each_colour() {
        // Seeds 0x123 and 0x456, then scale 0 (step 1): +1, -2, +3; scale 1
        // (step 2): +1 → +2 ... and a zero code keeping the value.
        let mut f = vec![(0x12, 8), (0x3, 4), (0x45, 8), (0x6, 4)];
        f.extend([(0, 2), (129, 8), (126, 8), (131, 8)]);
        f.extend([(1, 2), (129, 8), (0, 8), (127, 8)]);
        // Scale 3 (step 16) reads absolute levels keeping the low bits.
        f.extend([(3, 2), (10, 8), (20, 8), (30, 8)]);
        f.extend([(2, 2), (128, 8), (128, 8), (128, 8)]);
        let mut out = [0u16; 14];
        delta(&block_down(&f), &mut out);
        // Worked by hand: colour a at even pixels, b at odd ones.
        let want = [
            0x123,
            0x456,
            0x123 + 1,
            0x456 - 2,
            0x123 + 4,
            0x456,
            0x123 + 4,
            0x456 - 2,
            10 * 16 + 0x127 % 16,
            20 * 16 + 0x454 % 16,
            30 * 16 + 0x127 % 16,
            // 324 < 128 × 4: absolute, then 487 < 512 too, then 512 ≥ 512.
            128 * 4,
            128 * 4 + 487 % 4,
            128 * 4,
        ];
        assert_eq!(out, want);
    }

    #[test]
    fn a_zero_seed_is_a_dark_pixel_and_defers_the_seed() {
        // Colour a's first seed is 0: pixel 0 is 0 and pixel 2 is its seed.
        let f = [(0, 8), (0x45, 8), (0x6, 4), (0, 2), (0x12, 8), (0x3, 4)];
        let mut out = [0u16; 14];
        delta(&block_down(&f), &mut out);
        assert_eq!(&out[..3], &[0, 0x456, 0x123]);
    }

    #[test]
    fn seeded_blocks_have_wider_seeds_and_codes() {
        // 14 bits: seeds 5000 and 9000, then scale 1 (step 2) codes 513,
        // 0, 510 against the midpoint 512; scale 3 (step 16) code 100 reads
        // an absolute level; scale 0 code 512 is +0.
        let mut f = vec![(5000, 14), (9000, 14)];
        f.extend([(1, 2), (513, 10), (0, 10), (510, 10)]);
        f.extend([(3, 2), (100, 10), (300, 10), (200, 10)]);
        f.extend([(0, 2), (512, 10), (513, 10), (511, 10)]);
        let mut out = [0u16; 11];
        seeded(&block_down(&f), 14, &mut out);
        let want = [
            5000, 9000, 5002, 9000, 4998, 1600, 4800, 3200, 4800, 3201, 4799,
        ];
        assert_eq!(out, want.map(|v| v as u16));
        // 12 bits: 14 samples, codes of 8 bits.
        let mut f = vec![(1000, 12), (2000, 12)];
        for _ in 0..4 {
            f.extend([(0, 2), (129, 8), (127, 8), (128, 8)]);
        }
        let mut out = [0u16; 14];
        seeded(&block_down(&f), 12, &mut out);
        assert_eq!(&out[..6], &[1000, 2000, 1001, 1999, 1001, 2000]);
    }

    #[test]
    fn packed_blocks_read_from_the_lowest_bit() {
        let mut word = 0u128;
        for i in 0..10u128 {
            word |= (0x100 + i) << (12 * i);
        }
        let mut out = [0u16; 10];
        packed(&word.to_le_bytes(), 12, &mut out);
        assert_eq!(out, std::array::from_fn(|i| 0x100 + i as u16));
    }

    #[test]
    fn rotated_chunks_start_at_0x1ff8() {
        // One chunk of packed 16-bit samples (8 per block): the first sample
        // decoded is the one stored at byte 0x1FF8.
        let mut src = vec![0u8; CHUNK];
        for (i, c) in src.chunks_mut(2).enumerate() {
            c.copy_from_slice(&(i as u16).to_le_bytes());
        }
        let block = Block::Packed { bits: 16 };
        let out = decode(&src, block, true, 64, 128, &AtomicBool::new(false)).unwrap();
        assert_eq!(out[0], (ROTATION / 2) as u16);
        assert_eq!(out[(CHUNK - ROTATION) / 2], 0);
        let plain = decode(&src, block, false, 64, 128, &AtomicBool::new(false)).unwrap();
        assert_eq!(plain[5], 5);
    }

    #[test]
    fn words_hold_samples_in_their_high_bits() {
        let src = [0x30, 0x12, 0xf0, 0xff, 0x00];
        let out = words(&src, 2, 1, &AtomicBool::new(false)).unwrap();
        assert_eq!(out, [0x123, 0xfff]);
        assert!(words(&src, 3, 1, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn short_data_is_an_error() {
        let cancel = AtomicBool::new(false);
        assert!(decode(&[0; 100], Block::Delta, true, 14, 10, &cancel).is_err());
        assert!(decode(&[0; 160], Block::Delta, true, 14, 10, &cancel).is_ok());
        assert!(decode(&[], Block::Delta, true, 0, 10, &cancel).is_err());
    }
}
