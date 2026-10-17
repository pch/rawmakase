//! Samsung's compressed SRW codings, knowing no container. Both read their
//! bits most significant first from little-endian 32-bit words.
//!
//! **Rows of blocks** (compression 32770, the NX300 generation): each row
//! starts where a table of 32-bit offsets says. A row is blocks of 16
//! samples: a direction bit, four 2-bit operations on the four code widths
//! (3: a new 4-bit width; 2: one less; 1: one more; 0: kept; the widths
//! start at 7 on the first two rows and 4 after), then the even samples and
//! the odd ones, each a signed difference of its width (the even samples of
//! the block's first half use width 0, its second half width 1; the odd
//! ones widths 2 and 3). Downwards, the even samples predict from the row
//! above and the odd ones from two up, the same column; else from the last
//! even and odd samples of the block before (128 for the first block). Then
//! the samples of every 2 × 2 cell at the top right and bottom left swap.
//!
//! **Tiles of 16** (compression 32773, NX1 and NX500): an option byte
//! and an initial value in a 14-byte header; each row starts at the next
//! 16-byte boundary. Every 64 samples, unless the options say otherwise, a
//! 2-bit code adjusts a scale `m` (by 0, −2 or +2, or a new 12-bit value).
//! Every 16 samples a prediction mode, and (unless the options fix them)
//! four code widths, each from a 2-bit code relative to the width two
//! groups back of the same kind (or a new 4-bit width). A tile's samples
//! are its columns interleaved by colour; each is the prediction plus a
//! signed difference `d` times `2m + 1`, plus `m`. Prediction mode 7 (and
//! the first two rows) predict from the same colour two columns left (the
//! initial value at a row's start); the others from the mean of two
//! samples of the same colour above, at offsets the mode picks. Samples are
//! kept within the bit depth's range.
//!
//! Sources: written from prior general knowledge of the codings (no decoder
//! source consulted), then verified and corrected by measurement against
//! LibRaw's output on our files (an exact match on every compressed SRW of
//! our dataset); the measurement found that tiles' samples are kept within
//! the bit depth.
use crate::error::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};

/// Bits most significant first from little-endian 32-bit words, read a
/// word at a time as needed; zeros past the end.
struct Words<'a> {
    data: &'a [u8],
    /// The next word's byte offset.
    pos: usize,
    buf: u64,
    bits: u32,
}

impl<'a> Words<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            buf: 0,
            bits: 0,
        }
    }
    #[inline]
    fn get(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.bits < n {
            let mut w = [0u8; 4];
            for (i, b) in w.iter_mut().enumerate() {
                *b = self
                    .pos
                    .checked_add(i)
                    .and_then(|p| self.data.get(p))
                    .copied()
                    .unwrap_or(0);
            }
            self.pos = self.pos.saturating_add(4);
            self.buf = self.buf << 32 | u64::from(u32::from_le_bytes(w));
            self.bits += 32;
        }
        let v = (self.buf << (64 - self.bits)) >> (64 - n);
        self.bits -= n;
        v as u32
    }
}

/// `v` as a signed `n`-bit number.
#[inline]
fn signed(v: u32, n: u32) -> i32 {
    if n == 0 || n >= 32 {
        v as i32
    } else {
        ((v << (32 - n)) as i32) >> (32 - n)
    }
}

/// Decodes rows of blocks: row `r` starts `rows[r]` bytes into `data`.
pub(crate) fn blocks(
    data: &[u8],
    rows: &[u32],
    width: usize,
    height: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    let at = |out: &[u16], r: usize, c: usize| i32::from(out[r * width + c]);
    for row in 0..height {
        if row % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let start = rows.get(row).map_or(usize::MAX, |&r| r as usize);
        let mut bits = Words::new(data, start);
        let mut len = [if row < 2 { 7i32 } else { 4 }; 4];
        for col in (0..width).step_by(16) {
            let down = bits.get(1) == 1;
            let ops: [u32; 4] = std::array::from_fn(|_| bits.get(2));
            for (l, op) in len.iter_mut().zip(ops) {
                match op {
                    3 => *l = bits.get(4) as i32,
                    2 => *l -= 1,
                    1 => *l += 1,
                    _ => {}
                }
            }
            for c in (0..16).step_by(2).chain((1..16).step_by(2)) {
                let n = len[((c & 1) << 1) | (c >> 3)].clamp(0, 32) as u32;
                let diff = signed(bits.get(n), n);
                let x = col + c;
                let pred = if down {
                    let up = if c & 1 == 0 { 1 } else { 2 };
                    if row >= up && x < width {
                        at(out, row - up, x)
                    } else {
                        0
                    }
                } else if col > 0 {
                    at(out, row, col - if c & 1 == 0 { 2 } else { 1 })
                } else {
                    128
                };
                if x < width {
                    out[row * width + x] = pred.wrapping_add(diff) as u16;
                }
            }
        }
    }
    for row in (0..height.saturating_sub(1)).step_by(2) {
        if row % 256 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        for col in (0..width.saturating_sub(1)).step_by(2) {
            out.swap(row * width + col + 1, (row + 1) * width + col);
        }
    }
    Ok(())
}

/// Decodes tiles of 16 of `bits`-bit samples, the stream's header at the
/// start of `data`.
pub(crate) fn tiles(
    data: &[u8],
    bits: u32,
    width: usize,
    height: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    let opt = data.get(9).copied().unwrap_or(0);
    let init = data
        .get(12..14)
        .map_or(0, |b| i32::from(u16::from_le_bytes([b[0], b[1]])));
    // The samples by flat index; out of the image reads as 0.
    let get = |out: &[u16], i: i64| {
        usize::try_from(i)
            .ok()
            .and_then(|i| out.get(i))
            .map_or(0, |&v| i32::from(v))
    };
    let w = width as i64;
    // Samples are kept within the bit depth.
    let max = (1i32 << bits.clamp(1, 16)) - 1;
    let mut pos = 14usize;
    // The code widths carry over from row to row.
    let mut len = [0i32; 4];
    for row in 0..height {
        if row % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        // Rows start on 16-byte boundaries.
        pos = pos.next_multiple_of(16);
        let mut bits = Words::new(data, pos);
        let mut mag = 0i32;
        let mut pmode = 7u32;
        let mut lent = [[if row < 2 { 7i32 } else { 4 }; 2]; 3];
        let r = row as i64;
        // Where the two colours' rows above start, by column parity.
        let base: [i64; 2] = if row & 1 == 0 {
            [(r - 1) * w + 1, (r - 2) * w]
        } else {
            [(r - 2) * w, (r - 1) * w - 1]
        };
        let mut tab = 0;
        while tab + 15 < width {
            if opt & 4 == 0 && tab % 64 == 0 {
                mag = match bits.get(2) {
                    0 => mag,
                    1 => mag - 2,
                    2 => mag + 2,
                    _ => bits.get(12) as i32,
                };
            }
            if opt & 2 != 0 {
                pmode = 7 - 4 * bits.get(1);
            } else if bits.get(1) == 0 {
                pmode = bits.get(3);
            }
            if opt & 1 != 0 || bits.get(1) == 0 {
                let codes: [u32; 4] = std::array::from_fn(|_| bits.get(2));
                for (c, code) in codes.into_iter().enumerate() {
                    let i = (((row & 1) << 1) | (c & 1)) % 3;
                    len[c] = match code {
                        0 => lent[i][0],
                        1 => lent[i][0] + 1,
                        2 => lent[i][0] - 1,
                        _ => bits.get(4) as i32,
                    };
                    lent[i] = [lent[i][1], len[c]];
                }
            }
            for c in 0..16 {
                let col = tab + ((((c & 7) << 1) ^ (c >> 3)) ^ (row & 1));
                let pred = if pmode == 7 || row < 2 {
                    if tab > 0 {
                        get(out, r * w + (tab - 2 + (col & 1)) as i64)
                    } else {
                        init
                    }
                } else {
                    const A: [i64; 7] = [-4, -2, -2, 0, 0, 2, 4];
                    const B: [i64; 7] = [-4, -2, 0, 0, 2, 2, 4];
                    let p = pmode as usize;
                    let b = base[col & 1] + col as i64;
                    (get(out, b + A[p]) + get(out, b + B[p]) + 1) >> 1
                };
                let n = len[c >> 2].clamp(0, 32) as u32;
                let diff = signed(bits.get(n), n)
                    .wrapping_mul(mag.wrapping_mul(2).wrapping_add(1))
                    .wrapping_add(mag);
                if col < width {
                    out[row * width + col] = pred.wrapping_add(diff).clamp(0, max) as u16;
                }
            }
            tab += 16;
        }
        pos = bits.pos;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bits most significant first, packed into little-endian 32-bit words.
    #[derive(Default)]
    struct Writer {
        bits: Vec<u8>,
    }

    impl Writer {
        fn put(&mut self, v: u32, n: u32) {
            for i in (0..n).rev() {
                self.bits.push((v >> i & 1) as u8);
            }
        }
        fn words(&self) -> Vec<u8> {
            self.bits
                .chunks(32)
                .flat_map(|c| {
                    let w = c
                        .iter()
                        .enumerate()
                        .fold(0u32, |a, (i, &b)| a | u32::from(b) << (31 - i));
                    w.to_le_bytes()
                })
                .collect()
        }
    }

    #[test]
    fn words_are_little_endian_read_from_their_top() {
        let data = 0x8000_0001u32.to_le_bytes();
        let mut w = Words::new(&data, 0);
        assert_eq!(w.get(1), 1);
        assert_eq!(w.get(30), 0);
        assert_eq!(w.get(1), 1);
        assert_eq!(w.get(8), 0);
        assert_eq!(signed(0b111, 3), -1);
        assert_eq!(signed(0b011, 3), 3);
    }

    #[test]
    fn blocks_predict_from_the_last_block_and_swap_cells() {
        // One row of 16 (and a second, read from the same offset): widths
        // 7 kept; the even samples +1..+8 over 128, the odd ones −1…
        let mut w = Writer::default();
        w.put(0, 1);
        w.put(0, 8);
        for k in 0..8 {
            w.put(k + 1, 7);
        }
        for _ in 0..8 {
            w.put(0x7f, 7);
        }
        let data = w.words();
        let mut out = vec![0u16; 32];
        blocks(&data, &[0, 0], 16, 2, &mut out, &AtomicBool::new(false)).unwrap();
        // Before the swap row 0 is 129, 127, 130, 127, …; the swap moves
        // row 0's odd samples down and row 1's even ones up.
        assert_eq!(out[0], 129);
        assert_eq!(out[1], 129);
        assert_eq!(out[16], 127);
        assert_eq!(out[2], 130);
    }

    #[test]
    fn tiles_scale_their_differences() {
        // Header: options 4 (no scale codes) and 2 (modes by one bit), 1
        // (widths every tile); initial value 1000.
        let mut data = vec![0u8; 16];
        data[9] = 7;
        data[12..14].copy_from_slice(&1000u16.to_le_bytes());
        let mut w = Writer::default();
        w.put(1, 1); // mode 3, not 7 — but row 0 predicts from the left
        // Four width codes "new", then the four new widths: 2 bits each.
        for _ in 0..4 {
            w.put(3, 2);
        }
        for _ in 0..4 {
            w.put(2, 4);
        }
        for k in 0..16 {
            w.put(k % 4, 2);
        }
        data.extend(w.words());
        let mut out = vec![0u16; 16];
        tiles(&data, 12, 16, 1, &mut out, &AtomicBool::new(false)).unwrap();
        // Codes 0, 1, −2, −1 (2-bit signed) over 1000, the scale 0.
        assert_eq!(out[0], 1000);
        assert_eq!(out[2], 1001);
        assert_eq!(out[4], 998);
        assert_eq!(out[6], 999);
    }
}
