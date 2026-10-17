//! Sequential DCT JPEG (ITU T.81, baseline and extended, Huffman coded, 8 or
//! 12-bit samples) of one component, as some DNG writers (Blackmagic) store
//! raw tiles under the lossless JPEG compression code. The output is the
//! frame's samples in raster order. The inverse DCT is computed exactly (in
//! double precision) and rounded.
use super::bits::Msb;
use super::huffman::{Huffman, extend};
use crate::error::{Error, Result, corrupt};
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether `data` is a DCT JPEG this module decodes (its first frame is
/// SOF0 or SOF1).
pub(crate) fn is_dct(data: &[u8]) -> bool {
    frame_marker(data).is_some_and(|m| m == 0xc0 || m == 0xc1)
}

fn frame_marker(data: &[u8]) -> Option<u8> {
    let mut at = 2;
    while at + 4 <= data.len() && data[at] == 0xff {
        let m = data[at + 1];
        if (0xc0..=0xcf).contains(&m) && m != 0xc4 && m != 0xc8 && m != 0xcc {
            return Some(m);
        }
        at += 2 + usize::from(u16::from_be_bytes([data[at + 2], data[at + 3]]));
    }
    None
}

/// The natural position of each zig-zag index.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Decodes `data` into `out`: `height` rows of `width` samples; returns the
/// frame's width and height.
pub(crate) fn decode_into(
    data: &[u8],
    cancel: &AtomicBool,
    out: &mut Vec<u16>,
) -> Result<(usize, usize)> {
    if data.len() < 4 || data[..2] != [0xff, 0xd8] {
        return Err(corrupt("DCT JPEG without SOI"));
    }
    let mut quant: [Option<[u16; 64]>; 4] = [None; 4];
    let mut dc: Vec<Option<Huffman>> = (0..4).map(|_| None).collect();
    let mut ac: Vec<Option<Huffman>> = (0..4).map(|_| None).collect();
    let mut frame: Option<(u8, usize, usize, usize)> = None;
    let mut restart = 0usize;
    let mut at = 2;
    let scan = loop {
        if at + 4 > data.len() || data[at] != 0xff {
            return Err(corrupt("DCT JPEG headers end early"));
        }
        let marker = data[at + 1];
        let len = usize::from(u16::from_be_bytes([data[at + 2], data[at + 3]]));
        let body = data
            .get(at + 4..at + 2 + len)
            .filter(|_| len >= 2)
            .ok_or_else(|| corrupt("DCT JPEG segment"))?;
        match marker {
            0xdb => {
                let mut b = body;
                while let Some(&pq) = b.first() {
                    let (wide, id) = (pq >> 4 == 1, usize::from(pq & 15));
                    let size = if wide { 128 } else { 64 };
                    let t = b.get(1..1 + size).ok_or_else(|| corrupt("DQT"))?;
                    let mut table = [0u16; 64];
                    for (k, z) in ZIGZAG.iter().enumerate() {
                        table[*z] = if wide {
                            u16::from_be_bytes([t[2 * k], t[2 * k + 1]])
                        } else {
                            u16::from(t[k])
                        };
                    }
                    *quant.get_mut(id).ok_or_else(|| corrupt("DQT id"))? = Some(table);
                    b = &b[1 + size..];
                }
            }
            0xc4 => {
                let mut b = body;
                while b.len() >= 17 {
                    let (class, id) = (b[0] >> 4, usize::from(b[0] & 15));
                    let counts: [u8; 16] = b[1..17].try_into().map_err(|_| corrupt("DHT"))?;
                    let n: usize = counts.iter().map(|&c| usize::from(c)).sum();
                    let symbols = b.get(17..17 + n).ok_or_else(|| corrupt("DHT"))?;
                    let table = Some(Huffman::new(&counts, symbols)?);
                    match (class, id) {
                        (0, 0..=3) => dc[id] = table,
                        (1, 0..=3) => ac[id] = table,
                        _ => return Err(corrupt("DHT class")),
                    }
                    b = &b[17 + n..];
                }
            }
            0xc0 | 0xc1 => {
                if body.len() < 9 || body[5] != 1 {
                    return Err(Error::Unsupported(
                        "DCT JPEG with several components".into(),
                    ));
                }
                let precision = body[0];
                let height = usize::from(u16::from_be_bytes([body[1], body[2]]));
                let width = usize::from(u16::from_be_bytes([body[3], body[4]]));
                if !matches!(precision, 8 | 12) || body[7] != 0x11 {
                    return Err(Error::Unsupported("DCT JPEG precision or sampling".into()));
                }
                frame = Some((precision, width, height, usize::from(body[8] & 3)));
            }
            0xdd => {
                let [a, b] = <[u8; 2]>::try_from(body).map_err(|_| corrupt("DRI"))?;
                restart = usize::from(u16::from_be_bytes([a, b]));
            }
            0xda => {
                if body.first() != Some(&1) || body.len() < 6 {
                    return Err(Error::Unsupported("DCT JPEG scan".into()));
                }
                let tables = body[2];
                break (
                    usize::from(tables >> 4),
                    usize::from(tables & 15),
                    at + 2 + len,
                );
            }
            0xc2 | 0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                return Err(Error::Unsupported(format!("JPEG process {marker:#x}")));
            }
            _ => {}
        }
        at += 2 + len;
    };
    let (precision, width, height, q) = frame.ok_or_else(|| corrupt("SOS before SOF"))?;
    let (dc_id, ac_id, start) = scan;
    let quant = quant[q].ok_or_else(|| corrupt("missing quantization table"))?;
    let dc = dc
        .get(dc_id)
        .and_then(Option::as_ref)
        .ok_or_else(|| corrupt("DC table"))?;
    let ac = ac
        .get(ac_id)
        .and_then(Option::as_ref)
        .ok_or_else(|| corrupt("AC table"))?;
    crate::io::samples(width as u32, height as u32, 1)?;
    let (bw, bh) = (width.div_ceil(8), height.div_ceil(8));
    out.clear();
    out.resize(width * height, 0);
    let basis = basis();
    let max = (1i64 << precision) - 1;
    let center = 1f64 * (1u32 << (precision - 1)) as f64;
    let mut bits = Msb::jpeg(&data[start.min(data.len())..]);
    let mut predictor = 0i32;
    let mut left = restart;
    for by in 0..bh {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        for bx in 0..bw {
            if restart > 0 {
                if left == 0 {
                    bits.restart();
                    predictor = 0;
                    left = restart;
                }
                left -= 1;
            }
            let mut block = [0f64; 64];
            let size = u32::from(dc.symbol(&mut bits)?);
            if size > 16 {
                return Err(corrupt("DC difference"));
            }
            predictor += extend(bits.get(size), size);
            block[0] = f64::from(predictor) * f64::from(quant[0]);
            let mut k = 1;
            while k < 64 {
                let rs = ac.symbol(&mut bits)?;
                let (run, size) = (usize::from(rs >> 4), u32::from(rs & 15));
                if size == 0 {
                    if run == 15 {
                        k += 16;
                        continue;
                    }
                    break;
                }
                k += run;
                if k >= 64 {
                    return Err(corrupt("AC run past the block"));
                }
                let z = ZIGZAG[k];
                block[z] = f64::from(extend(bits.get(size), size)) * f64::from(quant[z]);
                k += 1;
            }
            // Samples: Σ basis(x,u) basis(y,v) F(v,u), rows first.
            let rows: [[f64; 8]; 8] = std::array::from_fn(|v| {
                std::array::from_fn(|x| (0..8).map(|u| basis[x][u] * block[v * 8 + u]).sum())
            });
            for (y, weights) in basis.iter().enumerate().take(height - by * 8) {
                let line = &mut out[(by * 8 + y) * width..][..width];
                for x in 0..8.min(width - bx * 8) {
                    let s: f64 = (0..8).map(|v| weights[v] * rows[v][x]).sum();
                    line[bx * 8 + x] = (s + center).round().clamp(0., max as f64) as u16;
                }
            }
        }
    }
    Ok((width, height))
}

/// The DCT basis: `basis[x][u]` = C(u)/2 · cos((2x + 1)uπ/16).
fn basis() -> [[f64; 8]; 8] {
    std::array::from_fn(|x| {
        std::array::from_fn(|u| {
            let c = if u == 0 {
                std::f64::consts::FRAC_1_SQRT_2
            } else {
                1.
            };
            c / 2. * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.).cos()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-block 8-bit JPEG whose only coefficient is DC: every sample is
    /// the level shift plus DC / 8 · quantizer.
    #[test]
    fn a_flat_block_decodes_to_its_dc() {
        let mut f = vec![0xff, 0xd8];
        let seg = |f: &mut Vec<u8>, m: u8, body: &[u8]| {
            f.extend([0xff, m]);
            f.extend(((body.len() + 2) as u16).to_be_bytes());
            f.extend(body);
        };
        let mut dqt = vec![0];
        dqt.extend([2u8; 64]);
        seg(&mut f, 0xdb, &dqt);
        // DC: symbol 4 (a 4-bit difference) coded `0`; AC: EOB coded `0`.
        let mut dht = vec![0x00, 1];
        dht.extend([0u8; 15]);
        dht.push(4);
        dht.extend([0x10, 1]);
        dht.extend([0u8; 15]);
        dht.push(0);
        seg(&mut f, 0xc4, &dht);
        seg(&mut f, 0xc0, &[8, 0, 8, 0, 8, 1, 1, 0x11, 0]);
        seg(&mut f, 0xda, &[1, 1, 0x00, 0, 63, 0]);
        // DC difference 12 (`1100`), then EOB `0`: 0 1100 0, padded with ones.
        f.extend([0b0110_0011, 0xff, 0x00]);
        f.extend([0xff, 0xd9]);
        assert!(is_dct(&f));
        let mut out = Vec::new();
        let (w, h) = decode_into(&f, &AtomicBool::new(false), &mut out).unwrap();
        assert_eq!((w, h), (8, 8));
        // 12 · 2 / 8 = 3 above the level shift of 128.
        assert!(out.iter().all(|&v| v == 131), "{out:?}");
        for end in 0..f.len() {
            let _ = decode_into(&f[..end], &AtomicBool::new(false), &mut out);
        }
        // A restart interval segment too short to hold its value.
        let mut bad = f[..2].to_vec();
        bad.extend([0xff, 0xdd, 0, 2]);
        bad.extend(&f[2..]);
        assert!(decode_into(&bad, &AtomicBool::new(false), &mut out).is_err());
    }
}
