//! Pentax's Huffman-coded differences (PEF compression 65535; Samsung's
//! SRW compression 32772 the same with a fixed table), knowing no container:
//! one bit stream, most significant bit first, codes every sample in raster
//! order as a difference whose length is a prefix code (ITU T.81's
//! difference coding). The first two samples of a row are differences from
//! the first two of the row two up (0 for the first two rows); every later
//! sample from the sample two to its left.
//!
//! Pentax's table (maker note tag 0x0220): a 16-bit count `d` (`d + 12`
//! codes, at most 15), 12 bytes, the codes as 16-bit values left-aligned in
//! 12 bits, then their lengths as bytes. A code of length `n` covers the
//! 2¹²⁻ⁿ 12-bit values from it on (wrapping), a later code's over an
//! earlier's. Values no code covers are a difference of 0 taking no bits.
//!
//! Sources: ITU T.81 (difference coding, F.1.2.1 and H.1.2.2) and ExifTool's
//! Pentax tag documentation (HuffmanTable, 0x0220); the table's layout, the
//! prediction and the bit order written from prior general knowledge of the
//! format (no decoder source consulted), then verified by an exact match with
//! LibRaw's output on every PEF of our dataset.
use crate::codec::bits::Msb;
use crate::codec::huffman::extend;
use crate::codec::speculative::{self, Stream};
use crate::error::{Error, Result, corrupt};
use crate::io::Order;
use rawmakase_cancel::Cancel;
use rayon::prelude::*;

/// Prefix codes by the next 12 bits: (length, difference length).
pub(crate) struct Codes {
    table: Vec<(u8, u8)>,
}

impl Codes {
    /// From each symbol's code (left-aligned in 12 bits) and length; the
    /// symbol is its index.
    pub(crate) fn new(codes: &[(u16, u8)]) -> Result<Self> {
        let mut table = vec![(0u8, 0u8); 4096];
        for (symbol, &(code, len)) in codes.iter().enumerate() {
            if !(1..=12).contains(&len) {
                return Err(corrupt("Pentax Huffman code length"));
            }
            let code = usize::from(code & 0xfff);
            for i in 0..4096 >> len {
                table[(code + i) & 4095] = (len, symbol as u8);
            }
        }
        Ok(Self { table })
    }
    /// From (length, symbol) pairs, each taking the next code of its
    /// length in list order.
    fn in_order(list: &[(u8, u8)]) -> Result<Self> {
        let mut table = vec![(0u8, 0u8); 4096];
        let mut at = 0usize;
        for &(len, symbol) in list {
            if !(1..=12).contains(&len) {
                return Err(corrupt("Huffman code length"));
            }
            let span = 4096 >> len;
            table
                .get_mut(at..at + span)
                .ok_or_else(|| corrupt("Huffman table overfull"))?
                .fill((len, symbol));
            at += span;
        }
        Ok(Self { table })
    }
    /// Samsung's fixed table (SRW compression 32772): (length, difference
    /// length) in code order, written from prior general knowledge of the
    /// format (no decoder source consulted) and verified by an exact match
    /// with LibRaw's output on our files.
    pub(crate) fn samsung() -> Self {
        const LIST: [(u8, u8); 14] = [
            (3, 4),
            (3, 7),
            (2, 6),
            (2, 5),
            (4, 3),
            (6, 0),
            (7, 9),
            (8, 10),
            (9, 11),
            (10, 12),
            (10, 13),
            (5, 1),
            (4, 8),
            (4, 2),
        ];
        // The list fills the 12-bit table exactly.
        Self::in_order(&LIST).unwrap_or(Self {
            table: vec![(0, 0); 4096],
        })
    }
    /// The table in a Pentax maker note's HuffmanTable value, in the note's
    /// byte order.
    pub(crate) fn from_note(b: &[u8], order: Order) -> Result<Self> {
        let u16_at = |i: usize| {
            b.get(i..i + 2).map(|s| match order {
                Order::Little => u16::from_le_bytes([s[0], s[1]]),
                Order::Big => u16::from_be_bytes([s[0], s[1]]),
            })
        };
        let n = (usize::from(u16_at(0).ok_or_else(|| corrupt("Pentax Huffman table"))?) + 12) & 15;
        let codes = (0..n)
            .map(|i| Some((u16_at(14 + 2 * i)?, *b.get(14 + 2 * n + i)?)))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| corrupt("Pentax Huffman table"))?;
        Self::new(&codes)
    }
    /// The next difference.
    #[inline]
    fn difference(&self, bits: &mut Msb) -> i32 {
        let (len, symbol) = self.table[bits.peek(12) as usize];
        bits.skip(u32::from(len));
        match symbol {
            0 => 0,
            16.. => -32768,
            n => extend(bits.get(u32::from(n)), u32::from(n)),
        }
    }
}

/// Bytes past the strip's stated length the parallel decoding still reads:
/// the stream can end a few bytes past it (40 symbols on a K-3 Mark III).
const STRIP_SLACK: usize = 64 << 10;

/// Decodes `height` rows of `width` samples into `out` (`width × height`).
/// `strip` is how many bytes the file says the stream takes, if it says:
/// what follows (a JPEG, other frames) is not decoded on spare threads.
pub(crate) fn decode(
    data: &[u8],
    strip: Option<usize>,
    codes: &Codes,
    width: usize,
    height: usize,
    out: &mut [u16],
    cancel: Cancel<'_>,
) -> Result<()> {
    decode_with(data, strip, codes, width, height, out, cancel, true).map(drop)
}

/// [`decode`], on all threads where it can (`shared`); whether it did.
#[allow(clippy::too_many_arguments)]
fn decode_with(
    data: &[u8],
    strip: Option<usize>,
    codes: &Codes,
    width: usize,
    height: usize,
    out: &mut [u16],
    cancel: Cancel<'_>,
    shared: bool,
) -> Result<bool> {
    let samples = width * height;
    if shared && width > 0 && out.len() >= samples {
        let out = &mut out[..samples];
        let span = strip.map_or(data.len(), |n| {
            n.saturating_add(STRIP_SLACK).min(data.len())
        });
        let stream = Differences {
            data: &data[..span],
            codes,
        };
        if let Some(end) = speculative::decode(&stream, out, cancel)?
            && reads_within(end, span, data.len())
        {
            return predict(out, width, cancel).map(|()| true);
        }
    }
    let mut bits = Msb::new(data);
    // The first two samples of the last row of each parity.
    let mut first = [[0i32; 2]; 2];
    for (row, dst) in out.chunks_exact_mut(width).take(height).enumerate() {
        if row % 64 == 0 && cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut last = [0i32; 2];
        for (col, d) in dst.iter_mut().enumerate() {
            let diff = codes.difference(&mut bits);
            let v = if col < 2 {
                let v = first[row & 1][col].wrapping_add(diff);
                first[row & 1][col] = v;
                v
            } else {
                last[col & 1].wrapping_add(diff)
            };
            last[col & 1] = v;
            *d = v as u16;
        }
    }
    Ok(false)
}

/// Whether a stream decoded from the first `span` of `len` bytes, ending
/// at bit `end`, is the one decoded from all of them. A code is looked up
/// by the 12 bits where it starts, so every bit read lies before
/// `end + 12`; past the span the decoding read zeros, not the data.
fn reads_within(end: u64, span: usize, len: usize) -> bool {
    span == len || end + 12 <= span as u64 * 8
}

/// The data's differences, each as the 16 bits of an `i16`.
struct Differences<'a> {
    data: &'a [u8],
    codes: &'a Codes,
}

impl Stream for Differences<'_> {
    fn data(&self) -> &[u8] {
        self.data
    }
    fn next(&self, bits: &mut Msb, _: usize) -> Option<u16> {
        // Every code is valid: values no code covers take no bits.
        Some(self.codes.difference(bits) as u16)
    }
}

/// The samples of the differences in `out` (rows of `width`), as [`decode`]
/// predicts them: the first two of each row in order, the rest of each row
/// on all threads.
fn predict(out: &mut [u16], width: usize, cancel: Cancel<'_>) -> Result<()> {
    let mut first = [[0u16; 2]; 2];
    let starts: Vec<[u16; 2]> = out
        .chunks_exact(width)
        .enumerate()
        .map(|(y, row)| {
            let p = &mut first[y & 1];
            for (p, &d) in p.iter_mut().zip(row) {
                *p = p.wrapping_add(d);
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
            let mut last = start;
            for (col, v) in row.iter_mut().enumerate() {
                let p = if col < 2 {
                    start[col]
                } else {
                    last[col & 1].wrapping_add(*v)
                };
                last[col & 1] = p;
                *v = p;
            }
            Ok(())
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn samsung_s_table_is_complete_and_prefix_free() {
        let codes = Codes::samsung();
        assert!(codes.table.iter().all(|&(len, _)| len > 0));
        // "000" → 4 bits, "1111" → 2 bits, "110100" → none.
        assert_eq!(codes.table[0], (3, 4));
        assert_eq!(codes.table[0xf00], (4, 2));
        assert_eq!(codes.table[0xd00], (6, 0));
    }

    /// Long streams decode on all threads as they do in order, with
    /// Samsung's complete table and with one leaving values uncovered.
    #[test]
    fn parallel_decoding_is_the_sequential_one() {
        let data: Vec<u8> = (0..1_500_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let sparse = Codes::new(&[(0, 2), (0x400, 3), (0x800, 4), (0xa00, 4)]).unwrap();
        for codes in [Codes::samsung(), sparse] {
            let cancel = AtomicBool::new(false);
            let (width, height) = (1500, 1200);
            let mut parallel = vec![0; width * height + 7];
            let mut sequential = parallel.clone();
            decode_with(
                &data,
                None,
                &codes,
                width,
                height,
                &mut parallel,
                Cancel::new(&cancel),
                true,
            )
            .unwrap();
            decode_with(
                &data,
                None,
                &codes,
                width,
                height,
                &mut sequential,
                Cancel::new(&cancel),
                false,
            )
            .unwrap();
            assert!(parallel == sequential);
        }
    }

    /// Where `n` differences coded from the start of `data` end, in bits.
    fn stream_end(data: &[u8], codes: &Codes, n: usize) -> u64 {
        let mut bits = Msb::new(data);
        for _ in 0..n {
            codes.difference(&mut bits);
        }
        bits.consumed()
    }

    /// The strip's stated length bounds what is decoded on spare threads
    /// (pixel-shift PEFs keep three frames after it, #725), and the
    /// samples stay the sequential decoder's however short it is.
    #[test]
    fn the_strip_bounds_the_parallel_decoding() {
        let codes = Codes::samsung();
        let (width, height) = (1000, 1000);
        let stream: Vec<u8> = (0..4_000_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let end = stream_end(&stream, &codes, width * height);
        let bytes = usize::try_from(end.div_ceil(8)).unwrap();
        assert!(bytes > 4 * STRIP_SLACK && stream.len() > 2 * bytes);
        let mut sequential = vec![0; width * height];
        let cancel = Cancel::NEVER;
        let used = decode_with(
            &stream,
            None,
            &codes,
            width,
            height,
            &mut sequential,
            cancel,
            false,
        );
        assert_eq!(used, Ok(false));
        let cases = [
            // The stream ends just past the stated length, within the slack.
            (Some(bytes - 100), true),
            (Some(bytes), true),
            // Far past it: decoded in order, from all the data.
            (Some(bytes / 2), false),
            (None, true),
        ];
        // Two workers, so that the stream is shared on any machine.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        pool.install(|| {
            for (strip, shared) in cases {
                let mut out = vec![0; width * height];
                let used = decode_with(
                    &stream, strip, &codes, width, height, &mut out, cancel, true,
                );
                assert_eq!(used, Ok(shared), "strip {strip:?}");
                assert!(out == sequential, "strip {strip:?}");
            }
        });
    }

    #[test]
    fn decodings_reading_past_the_span_are_not_the_stream_s() {
        assert!(reads_within(8 * 100 - 12, 100, 200));
        assert!(!reads_within(8 * 100 - 11, 100, 200));
        // All of the data: past it every decoder reads zeros.
        assert!(reads_within(8 * 300, 100, 100));
    }

    #[test]
    fn cancelling_stops_the_prediction() {
        let mut out = vec![0; 64];
        assert_eq!(
            predict(&mut out, 8, Cancel::new(&AtomicBool::new(true))),
            Err(Error::Cancelled)
        );
    }

    #[test]
    fn note_tables_give_codes_and_lengths() {
        // d = 7: (7 + 12) & 15 = 3 codes, "0", "10" and "11", for
        // differences of 0, 1 and 2 bits.
        let mut t = vec![0, 7];
        t.extend([0; 12]);
        t.extend([0x00, 0x00, 0x08, 0x00, 0x0c, 0x00]);
        t.extend([1, 2, 2]);
        let codes = Codes::from_note(&t, Order::Big).unwrap();
        // 0 | 10 1 | 11 01 | 0: lengths 0, 1 (+1), 2 (−2), 0.
        let mut bits = Msb::new(&[0b0101_1101, 0]);
        let d: Vec<i32> = (0..4).map(|_| codes.difference(&mut bits)).collect();
        assert_eq!(d, [0, 1, -2, 0]);
    }

    #[test]
    fn rows_predict_from_the_same_colour() {
        // "0" → 0 bits, "10" → 2 bits, "11" → 4 bits (symbols 1 and 3 are
        // covered by the later codes).
        let codes =
            Codes::new(&[(0, 1), (0x800, 12), (0x800, 2), (0xc00, 12), (0xc00, 2)]).unwrap();
        // Row 0: +3, +2, then 0 (→ 3) and +12 (→ 14); row 1: −10 and
        // zeros; row 2: −2 from row 0's first sample.
        let mut w = Vec::new();
        let mut put = |v: u32, n: u32| (0..n).rev().for_each(|i| w.push((v >> i & 1) as u8));
        put(0b10, 2); // len 2
        put(0b11, 2); // +3
        put(0b10, 2);
        put(0b10, 2); // +2
        put(0, 1); // col 2: 0 → 3
        put(0b11, 2);
        put(0b1100, 4); // col 3: +12 → 14
        put(0b11, 2);
        put(0b0101, 4); // row 1 col 0: −10 → −10 wraps
        put(0, 1);
        put(0, 1);
        put(0, 1);
        put(0b10, 2);
        put(0b01, 2); // row 2 col 0: −2 from row 0's 3 → 1
        let bytes: Vec<u8> = w
            .chunks(8)
            .map(|c| c.iter().enumerate().fold(0, |a, (i, b)| a | b << (7 - i)))
            .collect();
        let mut out = [0u16; 12];
        decode(&bytes, None, &codes, 4, 3, &mut out, Cancel::NEVER).unwrap();
        assert_eq!(&out[..4], &[3, 2, 3, 14]);
        assert_eq!(&out[4..8], &[(-10i32) as u16, 0, (-10i32) as u16, 0]);
        assert_eq!(out[8], 1);
    }
}
