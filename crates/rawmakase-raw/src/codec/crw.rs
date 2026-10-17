//! Canon's compressed CRW sensor data: blocks of 64 differences, each block's
//! first a DC-style difference (a size code, then that many bits; the
//! differences add up from block to block) and the rest AC-style run and size
//! codes (an end of block, sixteen zeros, or a run of zeros then a
//! difference), in JPEG's byte-stuffed bit order and its sign extension. A
//! sample is the one two to its left in the row plus its difference, the
//! first two of each row 512 plus theirs; the files with 12-bit samples keep
//! the two low bits of every sample, four to a byte, ahead of the coded data.
//!
//! Sources: our own measurements only. The three pairs of Huffman tables were
//! recovered by black-box probing of the earlier decoder with synthetic coded
//! data (every code's run, size and length), then checked on every CRW file of
//! the test dataset; ITU T.81 for the canonical code construction, the
//! difference coding and the byte stuffing.
use super::bits::Msb;
use super::huffman::{Huffman, extend};
use crate::error::{Error, Result, corrupt};
use std::sync::atomic::{AtomicBool, Ordering};

/// Per decoder table (the CRW's DecoderTable 0 to 2), the DC and the AC code
/// lengths (codes of each length 1 to 16) and symbols in code order, as a
/// JPEG Huffman table lists them. DC symbols are sizes; AC symbols are a run
/// (high nibble) and a size, 0x00 ending the block and 0xF0 sixteen zeros.
type Table = (&'static [u8; 16], &'static [u8]);
const TABLES: [[Table; 2]; 3] = [
    [
        // Table 0, DC: 12 codes.
        (
            &[0, 1, 4, 2, 3, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            &[
                0x04, 0x03, 0x05, 0x06, 0x02, 0x07, 0x01, 0x08, 0x09, 0x00, 0x0a, 0x0b,
            ],
        ),
        // Table 0, AC: 162 codes.
        (
            &[0, 2, 2, 2, 1, 4, 2, 1, 2, 5, 1, 1, 0, 0, 0, 139],
            &[
                0x03, 0x04, 0x02, 0x05, 0x01, 0x06, 0x07, 0x08, 0x12, 0x13, 0x11, 0x14, 0x09, 0x15,
                0x22, 0x00, 0x21, 0x16, 0x0a, 0xf0, 0x23, 0x17, 0x24, 0x31, 0x32, 0x18, 0x19, 0x33,
                0x25, 0x41, 0x34, 0x42, 0x35, 0x51, 0x36, 0x37, 0x38, 0x29, 0x79, 0x26, 0x1a, 0x39,
                0x56, 0x57, 0x28, 0x27, 0x52, 0x55, 0x58, 0x43, 0x76, 0x59, 0x77, 0x54, 0x61, 0xf9,
                0x71, 0x78, 0x75, 0x96, 0x97, 0x49, 0xb7, 0x53, 0xd7, 0x74, 0xb6, 0x98, 0x47, 0x48,
                0x95, 0x69, 0x99, 0x91, 0xfa, 0xb8, 0x68, 0xb5, 0xb9, 0xd6, 0xf7, 0xd8, 0x67, 0x46,
                0x45, 0x94, 0x89, 0xf8, 0x81, 0xd5, 0xf6, 0xb4, 0x88, 0xb1, 0x2a, 0x44, 0x72, 0xd9,
                0x87, 0x66, 0xd4, 0xf5, 0x3a, 0xa7, 0x73, 0xa9, 0xa8, 0x86, 0x62, 0xc7, 0x65, 0xc8,
                0xc9, 0xa1, 0xf4, 0xd1, 0xe9, 0x5a, 0x92, 0x85, 0xa6, 0xe7, 0x93, 0xe8, 0xc1, 0xc6,
                0x7a, 0x64, 0xe1, 0x4a, 0x6a, 0xe6, 0xb3, 0xf1, 0xd3, 0xa5, 0x8a, 0xb2, 0x9a, 0xba,
                0x84, 0xa4, 0x63, 0xe5, 0xc5, 0xf3, 0xd2, 0xc4, 0x82, 0xaa, 0xda, 0xe4, 0xf2, 0xca,
                0x83, 0xa3, 0xa2, 0xc3, 0xea, 0xc2, 0xe2, 0xe3,
            ],
        ),
    ],
    [
        // Table 1, DC: 12 codes.
        (
            &[0, 2, 2, 3, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
            &[
                0x03, 0x02, 0x04, 0x01, 0x05, 0x00, 0x06, 0x07, 0x09, 0x08, 0x0a, 0x0b,
            ],
        ),
        // Table 1, AC: 162 codes.
        (
            &[0, 2, 2, 1, 4, 1, 4, 1, 3, 3, 1, 0, 0, 0, 0, 140],
            &[
                0x02, 0x03, 0x01, 0x04, 0x05, 0x12, 0x11, 0x06, 0x13, 0x07, 0x08, 0x14, 0x22, 0x09,
                0x21, 0x00, 0x23, 0x15, 0x31, 0x32, 0x0a, 0x16, 0xf0, 0x24, 0x33, 0x41, 0x42, 0x19,
                0x17, 0x25, 0x18, 0x51, 0x34, 0x43, 0x52, 0x29, 0x35, 0x61, 0x39, 0x71, 0x62, 0x36,
                0x53, 0x26, 0x38, 0x1a, 0x37, 0x81, 0x27, 0x91, 0x79, 0x55, 0x45, 0x28, 0x72, 0x59,
                0xa1, 0xb1, 0x44, 0x69, 0x54, 0x58, 0xd1, 0xfa, 0x57, 0xe1, 0xf1, 0xb9, 0x49, 0x47,
                0x63, 0x6a, 0xf9, 0x56, 0x46, 0xa8, 0x2a, 0x4a, 0x78, 0x99, 0x3a, 0x75, 0x74, 0x86,
                0x65, 0xc1, 0x76, 0xb6, 0x96, 0xd6, 0x89, 0x85, 0xc9, 0xf5, 0x95, 0xb4, 0xc7, 0xf7,
                0x8a, 0x97, 0xb8, 0x73, 0xb7, 0xd8, 0xd9, 0x87, 0xa7, 0x7a, 0x48, 0x82, 0x84, 0xea,
                0xf4, 0xa6, 0xc5, 0x5a, 0x94, 0xa4, 0xc6, 0x92, 0xc3, 0x68, 0xb5, 0xc8, 0xe4, 0xe5,
                0xe6, 0xe9, 0xa2, 0xa3, 0xe3, 0xc2, 0x66, 0x67, 0x93, 0xaa, 0xd4, 0xd5, 0xe7, 0xf8,
                0x88, 0x9a, 0xd7, 0x77, 0xc4, 0x64, 0xe2, 0x98, 0xa5, 0xca, 0xda, 0xe8, 0xf3, 0xf6,
                0xa9, 0xb2, 0xb3, 0xf2, 0xd2, 0x83, 0xba, 0xd3,
            ],
        ),
    ],
    [
        // Table 2, DC: 12 codes.
        (
            &[0, 0, 6, 3, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            &[
                0x06, 0x05, 0x07, 0x04, 0x08, 0x03, 0x09, 0x02, 0x00, 0x0a, 0x01, 0x0b,
            ],
        ),
        // Table 2, AC: 162 codes.
        (
            &[0, 0, 6, 2, 1, 3, 3, 2, 5, 1, 2, 2, 8, 10, 0, 117],
            &[
                0x04, 0x05, 0x03, 0x06, 0x02, 0x07, 0x01, 0x08, 0x09, 0x12, 0x13, 0x14, 0x11, 0x15,
                0x0a, 0x16, 0x17, 0xf0, 0x00, 0x22, 0x21, 0x18, 0x23, 0x19, 0x24, 0x32, 0x31, 0x25,
                0x33, 0x38, 0x37, 0x34, 0x35, 0x36, 0x39, 0x79, 0x57, 0x58, 0x59, 0x28, 0x56, 0x78,
                0x27, 0x41, 0x29, 0x77, 0x26, 0x42, 0x76, 0x99, 0x1a, 0x55, 0x98, 0x97, 0xf9, 0x48,
                0x54, 0x96, 0x89, 0x47, 0xb7, 0x49, 0xfa, 0x75, 0x68, 0xb6, 0x67, 0x69, 0xb9, 0xb8,
                0xd8, 0x52, 0xd7, 0x88, 0xb5, 0x74, 0x51, 0x46, 0xd9, 0xf8, 0x3a, 0xd6, 0x87, 0x45,
                0x7a, 0x95, 0xd5, 0xf6, 0x86, 0xb4, 0xa9, 0x94, 0x53, 0x2a, 0xa8, 0x43, 0xf5, 0xf7,
                0xd4, 0x66, 0xa7, 0x5a, 0x44, 0x8a, 0xc9, 0xe8, 0xc8, 0xe7, 0x9a, 0x6a, 0x73, 0x4a,
                0x61, 0xc7, 0xf4, 0xc6, 0x65, 0xe9, 0x72, 0xe6, 0x71, 0x91, 0x93, 0xa6, 0xda, 0x92,
                0x85, 0x62, 0xf3, 0xc5, 0xb2, 0xa4, 0x84, 0xba, 0x64, 0xa5, 0xb3, 0xd2, 0x81, 0xe5,
                0xd3, 0xaa, 0xc4, 0xca, 0xf2, 0xb1, 0xe4, 0xd1, 0x83, 0x63, 0xea, 0xc3, 0xe2, 0x82,
                0xf1, 0xa3, 0xc2, 0xa1, 0xc1, 0xe3, 0xa2, 0xe1,
            ],
        ),
    ],
];

/// What a CRW's sensor data holds besides its codes.
pub(crate) struct Image<'a> {
    pub width: usize,
    pub height: usize,
    /// DecoderTable's table number, 0 to 2.
    pub table: usize,
    /// The coded differences, JPEG byte-stuffed.
    pub coded: &'a [u8],
    /// The two low bits of every sample, four to a byte from the least
    /// significant pair, for the 12-bit files.
    pub low_bits: Option<&'a [u8]>,
}

/// Decodes `image` into `out` (`width × height` samples, already sized):
/// 10-bit samples, or 12-bit ones with their low bits. Arithmetic wraps at 16
/// bits, as the samples RAWmakase has always read do on malformed data.
pub(crate) fn decode(image: &Image, out: &mut [u16], cancel: &AtomicBool) -> Result<()> {
    let [dc, ac] = TABLES
        .get(image.table)
        .ok_or_else(|| Error::Unsupported(format!("CRW decoder table {}", image.table)))?;
    let dc = Huffman::new(dc.0, dc.1)?;
    let ac = Huffman::new(ac.0, ac.1)?;
    let (width, count) = (image.width, image.width * image.height);
    if width < 2 || out.len() != count {
        return Err(corrupt("CRW image size"));
    }
    let mut bits = Msb::jpeg(image.coded);
    let mut carry = 0i32;
    let mut block = [0i32; 64];
    for (n, start) in (0..count).step_by(64).enumerate() {
        if n % 1024 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        block.fill(0);
        let size = u32::from(dc.symbol(&mut bits)?);
        if size > 16 {
            return Err(corrupt("CRW difference size"));
        }
        carry = carry.wrapping_add(extend(bits.get(size), size));
        block[0] = carry;
        let mut k = 1;
        while k < 64 {
            let symbol = ac.symbol(&mut bits)?;
            match symbol {
                0x00 => break,
                0xf0 => k += 16,
                _ => {
                    k += usize::from(symbol >> 4);
                    let size = u32::from(symbol & 15);
                    let v = extend(bits.get(size), size);
                    // A run past the block ends it.
                    if k < 64 {
                        block[k] = v;
                    }
                    k += 1;
                }
            }
        }
        for (j, &d) in block.iter().enumerate().take(count - start) {
            let p = start + j;
            let prediction = if p % width < 2 {
                512
            } else {
                i32::from(out[p - 2])
            };
            out[p] = prediction.wrapping_add(d) as u16;
        }
    }
    if let Some(low) = image.low_bits {
        for (chunk, &byte) in out.chunks_mut(4).zip(low) {
            for (i, v) in chunk.iter_mut().enumerate() {
                *v = (*v << 2) | u16::from(byte >> (2 * i) & 3);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::AtomicBool;

    /// Writes `codes` (bit strings) as a byte-stuffed stream.
    pub(crate) fn stream(codes: &[&str]) -> Vec<u8> {
        let bits: Vec<u8> = codes.concat().bytes().map(|b| b - b'0').collect();
        let mut out = Vec::new();
        for chunk in bits.chunks(8) {
            let mut v = 0u8;
            for (i, &b) in chunk.iter().enumerate() {
                v |= b << (7 - i);
            }
            out.push(v);
            if v == 0xff {
                out.push(0);
            }
        }
        out
    }

    #[test]
    fn decodes_blocks_of_differences() {
        // Table 0: DC size 3 is "010", AC (run 0, size 1) "1100", (0, 3)
        // "00", end of block "111111011", (1, 2) "111011", sixteen zeros
        // "1111111011". Block 0: DC 5 ("101"), then +1, -7, end. Block 1: DC
        // -2 (size 2 "101" + "01"), a zero run of 1 then +2 ("10"), end.
        let data = stream(&[
            "010",
            "101",
            "1100",
            "1",
            "00",
            "000",
            "111111011", // block 0
            "101",
            "01",
            "111011",
            "10",
            "111111011", // block 1
        ]);
        let image = super::Image {
            width: 128,
            height: 1,
            table: 0,
            coded: &data,
            low_bits: None,
        };
        let mut out = vec![0u16; 128];
        super::decode(&image, &mut out, &AtomicBool::new(false)).unwrap();
        // Block 0: 512 + 5, 512 + 1, then 517 - 7, then copies two apart.
        assert_eq!(&out[..6], &[517, 513, 510, 513, 510, 513]);
        // Block 1: its first difference is the running DC (5 - 2 = 3).
        assert_eq!(out[64], out[62] + 3);
        assert_eq!(out[65], out[63]);
        assert_eq!(out[66], out[64] + 2);
        // Low bits: four to a byte, the first sample's in the lowest pair.
        let low = [0b11_10_01_00u8; 32];
        let image = super::Image {
            low_bits: Some(&low),
            ..image
        };
        super::decode(&image, &mut out, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            &out[..4],
            &[517 << 2, 513 << 2 | 1, 510 << 2 | 2, 513 << 2 | 3]
        );
    }

    #[test]
    fn hostile_streams_are_errors_or_zeros_not_panics() {
        let mut out = vec![0u16; 64 * 3];
        for table in 0..3 {
            for data in [&[0xffu8; 40][..], &[0x00; 3], &[], &[0xff, 0xd9, 0x12]] {
                let image = super::Image {
                    width: 64,
                    height: 3,
                    table,
                    coded: data,
                    low_bits: Some(&[0xff; 7]),
                };
                let _ = super::decode(&image, &mut out, &AtomicBool::new(false));
            }
        }
        let image = super::Image {
            width: 64,
            height: 3,
            table: 3,
            coded: &[],
            low_bits: None,
        };
        assert!(super::decode(&image, &mut out, &AtomicBool::new(false)).is_err());
        let image = super::Image { table: 0, ..image };
        assert_eq!(
            super::decode(&image, &mut out, &AtomicBool::new(true)),
            Err(crate::Error::Cancelled)
        );
    }
}
