//! Hasselblad's coding of 3FR and FFF data (TIFF compression 7), knowing no
//! container: a lossless JPEG's headers (its Huffman table and predictor),
//! then data read most significant bit first from little-endian 32-bit
//! words, without byte stuffing. Each pair of columns codes two difference
//! lengths, then the two differences as JPEG codes them (16 bits of 0xFFFF
//! being −32768). A sample is the one two columns to its left plus its
//! difference, the first two of a row 0x8000 plus theirs; predictor 11 adds
//! half the change two rows up.
//!
//! Sources: ITU T.81 (headers, Huffman codes, difference coding); the pairs,
//! words and predictors written from prior general knowledge of the format
//! (no decoder source consulted, no public description found) and verified
//! by an exact match with LibRaw's output on every compressed 3FR and FFF of
//! the test dataset.
use super::bits::Words;
use super::huffman::Huffman;
use crate::error::{Error, Result, corrupt};
use crate::io::Order;
use std::sync::atomic::{AtomicBool, Ordering};

/// The headers' Huffman table and predictor, and where the data starts.
struct Headers {
    huffman: Huffman,
    predictor: u8,
    data: usize,
}

/// Reads the JPEG headers up to the scan.
fn headers(src: &[u8]) -> Result<Headers> {
    if src.get(..2) != Some(&[0xff, 0xd8]) {
        return Err(corrupt("Hasselblad data without SOI"));
    }
    let mut at = 2usize;
    let mut huffman = None;
    loop {
        // Fill bytes before a marker are allowed.
        while src.get(at) == Some(&0xff) && src.get(at + 1) == Some(&0xff) {
            at += 1;
        }
        let head = src
            .get(at..at + 4)
            .ok_or_else(|| corrupt("Hasselblad headers end early"))?;
        if head[0] != 0xff {
            return Err(corrupt("Hasselblad header marker"));
        }
        let len = usize::from(u16::from_be_bytes([head[2], head[3]]));
        let body = src
            .get(at + 4..at + 2 + len)
            .filter(|_| len >= 2)
            .ok_or_else(|| corrupt("Hasselblad header segment"))?;
        match head[1] {
            0xc4 => {
                // The first table of class 0.
                if body.len() >= 17 && body[0] & 0xf0 == 0 && huffman.is_none() {
                    let counts: [u8; 16] = body[1..17]
                        .try_into()
                        .map_err(|_| corrupt("Hasselblad DHT"))?;
                    let n: usize = counts.iter().map(|&c| usize::from(c)).sum();
                    let symbols = body
                        .get(17..17 + n)
                        .ok_or_else(|| corrupt("Hasselblad DHT"))?;
                    huffman = Some(Huffman::new(&counts, symbols)?);
                }
            }
            0xda => {
                let n = usize::from(*body.first().ok_or_else(|| corrupt("SOS"))?);
                let predictor = *body.get(1 + 2 * n).ok_or_else(|| corrupt("SOS"))?;
                return Ok(Headers {
                    huffman: huffman.ok_or_else(|| corrupt("Hasselblad data without DHT"))?,
                    predictor,
                    data: at + 2 + len,
                });
            }
            _ => {}
        }
        at += 2 + len;
    }
}

/// Decodes `out.len() / width` rows of `width` samples from `src`, the
/// stream from its SOI.
pub(crate) fn decode(src: &[u8], width: usize, out: &mut [u16], cancel: &AtomicBool) -> Result<()> {
    if width == 0 || !width.is_multiple_of(2) {
        return Err(Error::Unsupported("Hasselblad rows of an odd width".into()));
    }
    let h = headers(src)?;
    if !matches!(h.predictor, 8 | 11) {
        return Err(Error::Unsupported(format!(
            "Hasselblad predictor {}",
            h.predictor
        )));
    }
    let mut bits = Words::new(src.get(h.data..).unwrap_or(&[]), Order::Little);
    // The rows decoded: this one, the last, and the one before.
    let mut rows = [vec![0i32; width], vec![0i32; width], vec![0i32; width]];
    for (y, dst) in out.chunks_exact_mut(width).enumerate() {
        if y % 32 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        rows.rotate_left(1);
        let [two_up, _, row] = &mut rows;
        for col in (0..width).step_by(2) {
            let mut len = [0u32; 2];
            for l in &mut len {
                let (n, symbol) = h.huffman.lookup(bits.peek(16))?;
                bits.skip(n);
                *l = u32::from(symbol);
                if *l > 16 {
                    return Err(corrupt("Hasselblad difference length"));
                }
            }
            let mut diff = [0i32; 2];
            for (d, &l) in diff.iter_mut().zip(&len) {
                let v = bits.get(l) as i32;
                *d = if l == 0 {
                    0
                } else if v & (1 << (l - 1)) == 0 {
                    v - ((1 << l) - 1)
                } else {
                    v
                };
                if *d == 65535 {
                    *d = -32768;
                }
            }
            for (i, d) in diff.into_iter().enumerate() {
                let s = col + i;
                let mut pred = if col == 0 { 0x8000 } else { row[s - 2] };
                // Wrapping: a corrupt stream's sums run past 32 bits; the
                // output keeps their low 16 bits either way.
                if col > 0 && y > 1 && h.predictor == 11 {
                    pred = pred.wrapping_add((two_up[s] / 2).wrapping_sub(two_up[s - 2] / 2));
                }
                pred = pred.wrapping_add(d);
                row[s] = pred;
                dst[s] = pred as u16;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream of SOI, a DHT of three codes (0 → length 0, 10 → 1, 11 →
    /// 16) and an SOS of `predictor`, then `bits` packed into little-endian
    /// words.
    fn stream(predictor: u8, bits: &[(u32, u32)]) -> Vec<u8> {
        let mut out = vec![0xff, 0xd8, 0xff, 0xc4, 0, 22, 0];
        let mut counts = [0u8; 16];
        counts[0] = 1;
        counts[1] = 2;
        out.extend(counts);
        out.extend([0, 1, 16]);
        out.extend([0xff, 0xda, 0, 8, 1, 0, 0, predictor, 0, 0]);
        let mut acc: Vec<bool> = Vec::new();
        for &(v, n) in bits {
            for i in (0..n).rev() {
                acc.push(v >> i & 1 == 1);
            }
        }
        for chunk in acc.chunks(32) {
            let mut w = 0u32;
            for (i, &b) in chunk.iter().enumerate() {
                w |= u32::from(b) << (31 - i);
            }
            out.extend(w.to_le_bytes());
        }
        out
    }

    #[test]
    fn pairs_of_differences_from_two_columns_left() {
        // A row of four: lengths (1, 0) then differences +1 and none; then
        // lengths (16, 1): 16 bits 0xFFFF (−32768) and −1.
        let s = stream(
            8,
            &[
                (0b10, 2),
                (0, 1),
                (1, 1),
                (0b11, 2),
                (0b10, 2),
                (0xffff, 16),
                (0, 1),
            ],
        );
        let mut out = vec![0u16; 4];
        decode(&s, 4, &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(out, [0x8001, 0x8000, 0x8001 - 0x8000, 0x8000 - 1]);
    }

    #[test]
    fn predictor_11_sums_past_32_bits_do_not_panic() {
        // 65,534 × 5 samples, every difference +65,534 (length 16): a
        // row's sums climb past 2^31 and predictor 11 adds their change.
        let (width, height) = (65_534usize, 5usize);
        let bits: Vec<(u32, u32)> = (0..width * height / 2)
            .flat_map(|_| [(0b11, 2), (0b11, 2), (0xfffe, 16), (0xfffe, 16)])
            .collect();
        let s = stream(11, &bits);
        let mut out = vec![0u16; width * height];
        decode(&s, width, &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(out[..2], [0x8000u16.wrapping_add(0xfffe); 2]);
    }

    #[test]
    fn bad_streams_are_errors() {
        let cancel = AtomicBool::new(false);
        let mut out = vec![0u16; 4];
        assert!(decode(&[0, 1, 2], 4, &mut out, &cancel).is_err());
        assert!(decode(&stream(1, &[]), 4, &mut out, &cancel).is_err());
        assert!(decode(&stream(8, &[]), 3, &mut out[..3], &cancel).is_err());
        let s = stream(8, &[(0b10, 2), (0, 1), (1, 1)]);
        for end in 0..s.len() {
            let _ = decode(&s[..end], 4, &mut out, &cancel);
        }
        assert_eq!(
            decode(&s, 4, &mut out, &AtomicBool::new(true)),
            Err(Error::Cancelled)
        );
    }
}
