//! Samsung SRW: a TIFF whose raw image is the IFD0 SubIFD with a private
//! compression: 32769 16-bit words, 32770 packed 12-bit rows or (when its
//! data is smaller) rows of blocks, 32772 Huffman-coded differences, 32773
//! tiles of 16. The maker note is `vendor::samsung`'s.
//!
//! Sources: TIFF 6 and TIFF/EP (SubIFDs, CFAPattern), ExifTool's Samsung
//! tag documentation, and our own measurements of the codings against
//! LibRaw's output on our files.
use super::tiff::{Ifd, NAME_MAX, TiffFile};
use crate::codec::packed::{self, Packing};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::samsung;
use std::sync::atomic::AtomicBool;

/// Where an SRW's sensor data is and how it is coded, and its levels.
#[derive(Clone, Debug)]
pub(crate) struct SrwPlan {
    pub offset: u64,
    /// Rows of blocks: where the table of row offsets is.
    pub rows_at: u64,
    pub width: u32,
    pub height: u32,
    pub packing: Packing,
    pub bits: u32,
    pub black: [u32; 4],
    pub white: u32,
}

/// The raw image's directory: a SubIFD with Samsung's private compression.
fn raw_ifd<'a>(t: &'a TiffFile) -> Option<&'a Ifd> {
    t.ifds.iter().find(|ifd| {
        ifd.has(256)
            && t.int(ifd, 259)
                .is_some_and(|c| (32769..=32773).contains(&c))
    })
}

/// Whether the TIFF `t` is a Samsung SRW.
pub(crate) fn is_srw(t: &TiffFile) -> bool {
    let make = t.text_upto(&t.ifds[0], 271, NAME_MAX).unwrap_or_default();
    make.trim().to_ascii_uppercase().starts_with("SAMSUNG") && raw_ifd(t).is_some()
}

/// The row offsets table tag of rows of blocks.
const ROW_OFFSETS: u16 = 0xa010;

/// Reads an SRW's header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let raw = raw_ifd(&t).ok_or_else(|| Error::Unsupported("SRW without raw data".into()))?;
    let width = t.int(raw, 256).ok_or_else(|| corrupt("SRW width"))?;
    let height = t.int(raw, 257).ok_or_else(|| corrupt("SRW height"))?;
    crate::io::samples(width, height, 1)?;
    let bits = t.int(raw, 258).unwrap_or(12);
    if !(8..=16).contains(&bits) {
        return Err(Error::Unsupported(format!("SRW of {bits} bits")));
    }
    let compression = t.int(raw, 259).unwrap_or(0);
    let offset = *t
        .ints_upto(raw, 273, 1)
        .unwrap_or_default()
        .first()
        .ok_or_else(|| corrupt("SRW data offset"))?;
    let bytes = t
        .pieces(raw, 279)
        .unwrap_or_default()
        .iter()
        .fold(0u64, |sum, &c| sum.saturating_add(u64::from(c)));
    let packed = (u64::from(width) * u64::from(bits)).div_ceil(8) * u64::from(height);
    let (variant, rows_at) = match compression {
        32769 => (Variant::SrwUnpacked, 0),
        32770 if bytes >= packed => (Variant::SrwPacked, 0),
        32770 => {
            let at = t
                .int(raw, ROW_OFFSETS)
                .ok_or_else(|| corrupt("SRW row offsets"))?;
            (Variant::SrwBlocks, u64::from(at))
        }
        32772 => (Variant::SrwHuffman, 0),
        32773 => (Variant::SrwTiles, 0),
        c => return Err(Error::Unsupported(format!("SRW compression {c}"))),
    };
    // Files whose raw directory says three samples a pixel (the NX20
    // generation's) pack pairs into little-endian 3-byte words, the pair
    // swapped; earlier ones plainly (our measurements).
    let packing = if t.int(raw, 277).unwrap_or(1) > 1 {
        Packing {
            word: 3,
            swap: true,
            ..Packing::msb(bits)
        }
    } else {
        Packing::msb(bits)
    };
    let ifd0 = &t.ifds[0];
    let mut info = crate::info::base(variant);
    info.make = t.text_upto(ifd0, 271, NAME_MAX).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, NAME_MAX).unwrap_or_default();
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
    info.cfa = Some(cfa(&t, raw));
    if let Some(colors) =
        crate::facts::camera(&info.make, &info.model, false, [width, height]).find_map(|c| c.cfa)
    {
        info.cfa = Some(crate::info::second_green(Cfa {
            rows: 2,
            cols: 2,
            colors: colors.to_vec(),
        }));
    }
    super::tiff_exif::read(&t, &mut info);
    info.previews = t.previews();
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    let notes = samsung::read(&t).unwrap_or_default();
    info.cam_mul = notes.cam_mul;
    info.presets = notes.presets;
    let size = [width, height];
    // Rows for files of this bit depth, or any.
    let rows = || {
        crate::facts::camera(&info.make, &info.model, false, size)
            .filter(move |c| c.bits.is_none_or(|b| b == bits))
    };
    // Levels that do not fit 16 bits are not levels.
    let level = |v: i64| {
        u32::try_from(v)
            .ok()
            .filter(|&v| v <= 0xffff)
            .ok_or_else(|| corrupt("SRW level"))
    };
    let black = match (rows().find_map(|c| c.black), notes.black) {
        (Some(b), _) => [b; 4],
        (None, Some([r, g, b, g2])) => [level(r)?, level(g)?, level(b)?, level(g2)?],
        (None, None) => [0; 4],
    };
    let white = match (rows().find_map(|c| c.white), notes.white) {
        (Some(w), _) => w,
        (None, Some(w)) => level(w)?,
        (None, None) => (1 << bits) - 1,
    };
    info.plan = Plan::Srw(SrwPlan {
        offset: t.base + u64::from(offset),
        rows_at: t.base + rows_at,
        width,
        height,
        packing,
        bits,
        black,
        white,
    });
    Ok(info)
}

/// The colour filter from the raw directory's CFAPattern, at the sensor's
/// corner; red first when it has none.
fn cfa(t: &TiffFile, raw: &Ifd) -> Cfa {
    let colors = t
        .bytes_of(raw, 33422)
        .filter(|p| p.len() == 4 && p.iter().all(|&c| c <= 2))
        .map_or(vec![0, 1, 1, 2], <[u8]>::to_vec);
    crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors,
    })
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &SrwPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let (width, height) = (plan.width as usize, plan.height as usize);
    let mut data = crate::io::zeroed(crate::io::samples(plan.width, plan.height, 1)?)?;
    let file = Bytes::new(bytes, Order::Little);
    // Truncated files keep what they have.
    let available = (bytes.len() as u64).saturating_sub(plan.offset);
    let src = file.slice(plan.offset, available)?;
    match info.variant {
        Variant::SrwUnpacked => {
            // 12-bit words hold samples of 11 bits (our measurements).
            let shift = u32::from(plan.bits == 12);
            packed::words(src, Order::Little, shift, &mut data, cancel)?;
        }
        Variant::SrwPacked => {
            let p = plan.packing;
            packed::rows(src, p, width, p.row_bytes(width), &mut data, cancel)?;
        }
        Variant::SrwBlocks => {
            let table = file.slice(plan.rows_at, 4 * height as u64)?;
            let rows: Vec<u32> = table
                .as_chunks::<4>()
                .0
                .iter()
                .map(|w| u32::from_le_bytes(*w))
                .collect();
            crate::codec::samsung::blocks(src, &rows, width, height, &mut data, cancel)?;
        }
        Variant::SrwHuffman => {
            let codes = crate::codec::pentax::Codes::samsung();
            crate::codec::pentax::decode(src, &codes, width, height, &mut data, cancel)?;
        }
        Variant::SrwTiles => {
            crate::codec::samsung::tiles(src, plan.bits, width, height, &mut data, cancel)?;
        }
        v => {
            return Err(Error::Unsupported(format!(
                "{} is not an SRW variant",
                v.name()
            )));
        }
    }
    let [left, top] = info.margins;
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area: [left, top, info.width, info.height],
        black: plan.black,
        black_pattern: (0, 0, Vec::new()),
        white: plan.white,
        linear_white: [0; 4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::test_tiff::{E, ascii, dir, long, longs, offsets, point, short, tiff};

    const BE: Order = Order::Big;
    const KEY: [u32; 11] = [305, 72, 737, 456, 282, 307, 519, 724, 13, 505, 193];

    /// `values` stored with the key added (`sign` 1) or taken away (−1),
    /// key element `from` first.
    fn keyed(values: &[i64], sign: i64, from: usize) -> Vec<u32> {
        values
            .iter()
            .enumerate()
            .map(|(i, &v)| (v + sign * i64::from(KEY[(i + from) % 11])) as u32)
            .collect()
    }

    /// A maker note: a big-endian directory of LONG vectors, its offsets
    /// from its own start.
    fn note(black: i64) -> Vec<u8> {
        let mut raw_data = vec![0i64; 4];
        raw_data.extend([4095; 8]);
        dir(
            &[
                long(BE, 0xa020, &KEY),
                long(BE, 0xa021, &keyed(&[8192, 4096, 4096, 6144], 1, 0)),
                long(BE, 0xa024, &keyed(&[731, 4096, 4096, 352], 1, 1)),
                long(BE, 0xa028, &keyed(&[black; 4], 1, 0)),
                long(BE, 0xa048, &keyed(&raw_data, -1, 0)),
            ],
            0,
            BE,
        )
    }

    /// A big-endian SRW of `width × height`: IFD0 with one SubIFD, the raw
    /// directory (`compression`, `bps`, samples per pixel `spp`, its data
    /// `strip` and `more` entries), and an EXIF maker note.
    fn srw(
        [width, height]: [u32; 2],
        [compression, bps, spp]: [u16; 3],
        strip: &[u8],
        more: Vec<E>,
        note: &[u8],
    ) -> Vec<u8> {
        // The raw directory's place: after the strip and the note.
        let at = offsets(&[strip, note, &[]]);
        let mut raw = vec![
            long(BE, 256, &[width]),
            long(BE, 257, &[height]),
            short(BE, 258, &[bps]),
            short(BE, 259, &[compression]),
            short(BE, 277, &[spp]),
            long(BE, 273, &[at[0]]),
            long(BE, 279, &[strip.len() as u32]),
            short(BE, 33421, &[2, 2]),
            E(33422, 1, vec![0, 1, 1, 2]),
        ];
        raw.extend(more);
        let raw = dir(&raw, at[2], BE);
        let ifd0 = vec![
            ascii(271, "SAMSUNG"),
            ascii(272, "NX-TEST"),
            long(BE, 330, &[at[2]]),
        ];
        let exif = vec![E(0x927c, 7, vec![0; note.len()])];
        let mut file = tiff(b"MM\0*", BE, ifd0, exif, &[strip, note, &raw]);
        point(&mut file, BE, 0x927c, note.len(), at[1]);
        file
    }

    fn decode(file: &[u8]) -> Result<(RawInfo, Sensor)> {
        let info = crate::open(file)?;
        let sensor = crate::decode(&info, file, &AtomicBool::new(false))?;
        Ok((info, sensor))
    }

    #[test]
    fn packed_rows_and_the_note_s_keyed_vectors() {
        let strip = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc];
        let file = srw([2, 2], [32770, 12, 1], &strip, vec![], &note(3));
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::SrwPacked);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Samsung", "NX-TEST")
        );
        assert_eq!(s.data, [0x123, 0x456, 0x789, 0xabc]);
        assert_eq!(info.cam_mul, [8192., 4096., 6144., 4096.]);
        // A D65 preset of red and blue below a quarter of green is on a
        // scale of 256.
        assert_eq!(info.presets.d65, [731., 256., 352.]);
        assert_eq!((s.black, s.white), ([3; 4], 4095));
        // Three samples a pixel: little-endian 3-byte words, pairs swapped.
        let file = srw([2, 2], [32770, 12, 3], &strip, vec![], &note(0));
        let (_, s) = decode(&file).unwrap();
        assert_eq!(s.data, [0x412, 0x563, 0xa78, 0xbc9]);
    }

    #[test]
    fn words_huffman_blocks_and_tiles() {
        // 12-bit words hold 11-bit samples.
        let words = longs(Order::Little, &[0x0004_0002, 0x0008_0006]);
        let (info, s) = decode(&srw([2, 2], [32769, 12, 1], &words, vec![], &note(0))).unwrap();
        assert_eq!(info.variant, Variant::SrwUnpacked);
        assert_eq!(s.data, [1, 2, 3, 4]);
        // Huffman: "1111" (2 bits) then "11": +3 for every sample.
        let (info, s) = decode(&srw([4, 1], [32772, 12, 1], &[0xff; 8], vec![], &note(0))).unwrap();
        assert_eq!(info.variant, Variant::SrwHuffman);
        assert_eq!(s.data, [3, 3, 6, 6]);
        // Tiles: options 7, all codes zero: mode 7, widths 7 (the first
        // rows), differences 0: the initial value everywhere.
        let mut tiles = vec![0u8; 16];
        tiles[9] = 7;
        tiles[12..14].copy_from_slice(&1000u16.to_le_bytes());
        tiles.extend([0; 64]);
        let (info, s) = decode(&srw([16, 1], [32773, 12, 1], &tiles, vec![], &note(0))).unwrap();
        assert_eq!(info.variant, Variant::SrwTiles);
        assert!(s.data.iter().all(|&v| v == 1000));
    }

    #[test]
    fn rows_of_blocks_start_where_their_table_says() {
        // The table of row offsets in the strip itself, both rows' codes
        // (all zero: 128 everywhere) after it.
        let mut strip = 8u32.to_le_bytes().to_vec();
        strip.extend(8u32.to_le_bytes());
        strip.extend([0; 64]);
        let at = offsets(&[&strip])[0];
        let more = vec![long(BE, ROW_OFFSETS, &[at])];
        let file = srw([64, 2], [32770, 12, 1], &strip, more, &note(0));
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::SrwBlocks);
        assert!(s.data.iter().all(|&v| v == 128));
    }

    #[test]
    fn unknown_compressions_are_unsupported() {
        let file = srw([16, 1], [32771, 12, 1], &[0; 64], vec![], &note(0));
        assert!(matches!(crate::open(&file), Err(Error::Unsupported(_))));
    }

    #[test]
    fn tiles_of_widths_grown_past_31_bits_decode() {
        // Options 5: no scale codes, widths every tile, each one more than
        // two tiles back (growing by one a tile), every difference all ones.
        let mut bits: Vec<bool> = Vec::new();
        let mut put = |v: u64, n: u32| (0..n).rev().for_each(|i| bits.push(v >> i & 1 == 1));
        for tile in 1..=50u32 {
            put(1, 1);
            for _ in 0..4 {
                put(0b01, 2);
            }
            let width = (7 + tile).min(32);
            for _ in 0..16 {
                put(u64::MAX, width);
            }
        }
        let words: Vec<u8> = bits
            .chunks(32)
            .flat_map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u32, |w, (i, &b)| w | u32::from(b) << (31 - i))
                    .to_le_bytes()
            })
            .collect();
        let mut data = vec![0u8; 16];
        data[9] = 5;
        data.extend(words);
        let file = srw([800, 1], [32773, 14, 1], &data, vec![], &note(0));
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::SrwTiles);
        assert!(s.data.iter().all(|&v| v <= 16383));
    }

    #[test]
    fn bad_levels_cancellation_and_garbage() {
        let strip = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc];
        let file = srw([2, 2], [32770, 12, 1], &strip, vec![], &note(1 << 20));
        assert!(matches!(crate::open(&file), Err(Error::Corrupt(_))));
        let mut tiles = vec![0u8; 16];
        tiles[9] = 7;
        tiles.extend([0x5a; 64]);
        let files = [
            srw([2, 2], [32770, 12, 3], &strip, vec![], &note(0)),
            srw([2, 2], [32769, 14, 1], &[7; 8], vec![], &note(0)),
            srw([4, 1], [32772, 12, 1], &[0xa5; 8], vec![], &note(0)),
            srw([16, 1], [32773, 14, 1], &tiles, vec![], &note(0)),
        ];
        for file in files {
            let info = crate::open(&file).unwrap();
            let cancelled = AtomicBool::new(true);
            assert_eq!(
                crate::decode(&info, &file, &cancelled).unwrap_err(),
                Error::Cancelled
            );
            for end in (0..file.len()).step_by(3) {
                let _ = decode(&file[..end]);
            }
            let mut noisy = file.clone();
            for (i, b) in noisy.iter_mut().enumerate().skip(8) {
                *b ^= (i * 7919 % 251) as u8;
            }
            let _ = decode(&noisy);
        }
    }
}
