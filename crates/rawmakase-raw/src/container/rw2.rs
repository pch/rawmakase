//! Panasonic RW2 and Leica RWL, and the older Panasonic and Leica RAW files
//! of the same structure: a TIFF with its own magic number (`IIU`) whose
//! first directory holds Panasonic's tags (`vendor::panasonic`), the EXIF
//! directory, the embedded JPEG (tag 0x2E) and where the sensor data is (tag
//! 0x118, else the strip offset). The raw format (tag 0x2D) and compression
//! (tag 0x0B) say how the data is coded.
//!
//! Sources: TIFF 6, Exif 2.32, ExifTool's PanasonicRaw tag documentation,
//! and our own measurements.
use super::tiff::{NAME_MAX, TiffFile};
use crate::codec::panasonic::{self, Block};
use crate::codec::panasonic_strips as strips;
use crate::error::{Error, Result, corrupt};
use crate::info::RawInfo;
use crate::io::Bytes;
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::panasonic as tags;
use std::sync::atomic::AtomicBool;

/// How the sensor data is coded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Coding {
    /// 16-byte blocks, whole 0x4000-byte chunks rotated when `rotated`.
    Blocks { block: Block, rotated: bool },
    /// 16-bit words (the oldest files, without a raw format).
    Words,
    /// Prefix-coded strips (raw format 8).
    Strips(Box<tags::Strips>),
}

/// Where an RW2's sensor data is and how it is coded, and its levels.
#[derive(Clone, Debug)]
pub(crate) struct Rw2Plan {
    pub offset: u64,
    pub coding: Coding,
    pub width: u32,
    pub height: u32,
    pub black: [u32; 4],
    pub white: u32,
    pub linear_white: [u32; 4],
    /// The rows read from the data; the rest are 0.
    pub rows: u32,
}

/// Whether `bytes` is an RW2-structured file.
pub(crate) fn is_rw2(bytes: &[u8]) -> bool {
    bytes.starts_with(b"IIU\0")
}

/// Reads an RW2's header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let width = t
        .int(ifd0, tags::SENSOR_WIDTH)
        .ok_or_else(|| corrupt("RW2 sensor width"))?;
    let height = t
        .int(ifd0, tags::SENSOR_HEIGHT)
        .ok_or_else(|| corrupt("RW2 sensor height"))?;
    crate::io::samples(width, height, 1)?;
    let bits = t.int(ifd0, tags::BITS).unwrap_or(12);
    let format = t.int(ifd0, tags::RAW_FORMAT);
    let (variant, coding) = match (format, bits) {
        (Some(3 | 4), 12) => (
            Variant::Rw2Delta,
            Coding::Blocks {
                block: Block::Delta,
                rotated: format == Some(4),
            },
        ),
        (Some(5), 12 | 14) => (
            Variant::Rw2PackedRotated,
            Coding::Blocks {
                block: Block::Packed { bits },
                rotated: true,
            },
        ),
        (Some(6), 12 | 14) => (
            Variant::Rw2Seeded,
            Coding::Blocks {
                block: Block::Seeded { bits },
                rotated: false,
            },
        ),
        (Some(7), 14) => (
            Variant::Rw2Packed,
            Coding::Blocks {
                block: Block::Packed { bits },
                rotated: false,
            },
        ),
        (Some(8), 12 | 14 | 16) => (
            Variant::Rw2Huffman,
            Coding::Strips(Box::new(
                tags::strips(&t, ifd0).ok_or_else(|| corrupt("RW2 strips"))?,
            )),
        ),
        (Some(2), 12) => (
            Variant::Rw2Packed,
            Coding::Blocks {
                block: Block::Packed { bits },
                rotated: false,
            },
        ),
        (Some(f), _) => {
            return Err(Error::Unsupported(format!(
                "Panasonic raw format {f} of {bits} bits"
            )));
        }
        (None, _) => match t.int(ifd0, tags::COMPRESSION) {
            Some(34826 | 34828 | 34830) => (Variant::Rw2Unpacked, Coding::Words),
            c => {
                return Err(Error::Unsupported(format!(
                    "Panasonic compression {}",
                    c.unwrap_or(0)
                )));
            }
        },
    };
    let offset = t
        .int(ifd0, tags::RAW_OFFSET)
        .or_else(|| t.int(ifd0, 273))
        .ok_or_else(|| corrupt("RW2 data offset"))?;
    let mut info = crate::info::base(variant);
    info.make = t.text_upto(ifd0, 271, NAME_MAX).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, NAME_MAX).unwrap_or_default();
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);
    info.raw_width = width;
    info.raw_height = height;
    let borders = tags::borders(&t, ifd0);
    info.sensor_borders = borders;
    info.width = tags::visible_width(width, borders);
    info.height = height;
    info.frame = tags::frame(&t, ifd0);
    info.cfa = Some(tags::cfa(&t, ifd0));
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
    info.cam_mul = tags::white_balance(&t, ifd0);
    info.presets = tags::presets(&t, ifd0);
    if let Some(iso) = tags::iso(&t, ifd0) {
        info.iso = iso;
    }
    if let Some(e) = ifd0.entries.get(&tags::JPEG)
        && let Some(at) = t.offset_of(e)
    {
        info.previews.push(crate::preview::Candidate {
            offset: t.base + at,
            len: u64::from(e.count),
            pixels: 0,
        });
    }
    super::tiff_exif::read(&t, &mut info);
    if let Some(focal) = tags::focal_35mm(&t, ifd0) {
        info.focal_35mm = focal;
    }
    // Words hold 12-bit samples whatever the bit depth tag says.
    let sample_bits = if coding == Coding::Words { 12 } else { bits };
    let (mut black, mut white, linear_white) = tags::levels(&t, ifd0, format, sample_bits);
    // What the file does not say: the visible area and levels RAWmakase has
    // always read some cameras' files with.
    let (make, model) = (info.make.clone(), info.model.clone());
    let rows = || crate::facts::camera(&make, &model, false, [width, height]);
    if let Some([left, top, w, h]) = rows().find_map(|c| c.visible)
        && left.checked_add(w).is_some_and(|r| r <= width)
        && top.checked_add(h).is_some_and(|b| b <= height)
    {
        info.margins = [left, top];
        info.width = w;
        info.height = h;
        let cfa = info.cfa.take();
        info.cfa = cfa.map(|c| c.shifted(top as usize, left as usize));
    }
    if let Some(b) = rows().find_map(|c| c.black) {
        black = [b; 4];
    }
    // A white for the file's ISO speed before the camera's.
    if let Some(w) = crate::facts::iso_white(&make, &model, false, [width, height], info.iso)
        .or_else(|| {
            rows()
                .filter(|c| c.bits.is_none_or(|b| b == sample_bits))
                .find_map(|c| c.white)
        })
    {
        white = w;
    }
    info.plan = Plan::Rw2(Rw2Plan {
        offset: t.base + u64::from(offset),
        coding,
        width,
        height,
        black,
        white,
        linear_white,
        // Raw format 7 is read in whole groups of four rows (our own
        // measurement: a 12096 × 8018 file's last two rows are 0).
        rows: if format == Some(7) {
            height & !3
        } else {
            height
        },
    });
    Ok(info)
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &Rw2Plan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let file = Bytes::new(bytes, crate::io::Order::Little);
    let data = match &plan.coding {
        &Coding::Blocks { block, rotated } => {
            // To the end of the file: the last chunk is rotated whole.
            let available = (bytes.len() as u64).saturating_sub(plan.offset);
            let src = file.slice(plan.offset, available)?;
            let mut data = panasonic::decode(src, block, rotated, plan.width, plan.height, cancel)?;
            if let Block::Seeded { .. } = block {
                // Raw format 6 is read 15 below its coded levels, where its
                // black level tags are (formats 3 and 4 keep the coded levels
                // and put black 15 above the tags).
                data.iter_mut().for_each(|v| *v = v.saturating_sub(15));
            }
            let read = plan.rows as usize * plan.width as usize;
            data.iter_mut().skip(read).for_each(|v| *v = 0);
            data
        }
        Coding::Words => {
            let available = (bytes.len() as u64).saturating_sub(plan.offset);
            panasonic::words(
                file.slice(plan.offset, available)?,
                plan.width,
                plan.height,
                cancel,
            )?
        }
        Coding::Strips(s) => {
            let n = s.offsets.len();
            if [&s.lefts, &s.tops, &s.bits, &s.widths, &s.heights]
                .iter()
                .any(|v| v.len() != n)
                || n == 0
            {
                return Err(corrupt("RW2 strip lists"));
            }
            // Strips stacked in more than one row are not decoded.
            if s.tops.iter().any(|&t| t != 0) || s.heights.iter().any(|&h| h != plan.height) {
                return Err(Error::Unsupported("RW2 strips in several rows".into()));
            }
            let strips = (0..n)
                .map(|i| {
                    Ok(strips::Strip {
                        data: file
                            .slice(u64::from(s.offsets[i]), u64::from(s.bits[i]).div_ceil(8))?,
                        left: s.lefts[i] as usize,
                        width: s.widths[i] as usize,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let codes = strips::Codes::new(&s.codes)?;
            strips::decode(&strips, &codes, plan.width, plan.height, cancel)?
        }
    };
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
        linear_white: plan.linear_white,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::info::UNSET;
    use std::sync::atomic::AtomicBool;

    /// A little-endian RW2 file: IFD0 with `entries` (tag, type, values as
    /// bytes), then `data` at the offset tag 0x118 names.
    pub(crate) fn rw2(entries: &[(u16, u16, Vec<u8>)], data: &[u8]) -> Vec<u8> {
        let mut entries = entries.to_vec();
        let n = entries.len() + 1;
        let ifd_end = 8 + 2 + 12 * n + 4;
        let extra: usize = entries.iter().map(|e| e.2.len()).filter(|&l| l > 4).sum();
        let data_at = (ifd_end + extra).next_multiple_of(16) as u32;
        entries.push((0x118, 4, data_at.to_le_bytes().to_vec()));
        entries.sort_by_key(|e| e.0);
        let mut out = b"IIU\0".to_vec();
        out.extend(8u32.to_le_bytes());
        out.extend((n as u16).to_le_bytes());
        let mut values: Vec<u8> = Vec::new();
        for (tag, kind, v) in &entries {
            let size = match kind {
                3 => 2,
                4 => 4,
                _ => 1,
            };
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(((v.len() / size) as u32).to_le_bytes());
            if v.len() <= 4 {
                let mut inline = v.clone();
                inline.resize(4, 0);
                out.extend(inline);
            } else {
                out.extend(((ifd_end + values.len()) as u32).to_le_bytes());
                values.extend(v);
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(values);
        out.resize(data_at as usize, 0);
        out.extend(data);
        out
    }
    pub(crate) fn short(tag: u16, v: u16) -> (u16, u16, Vec<u8>) {
        (tag, 3, v.to_le_bytes().to_vec())
    }
    fn text(tag: u16, v: &str) -> (u16, u16, Vec<u8>) {
        let mut b = v.as_bytes().to_vec();
        b.push(0);
        (tag, 2, b)
    }
    /// A block of 14 delta-coded samples: seeds `a` and `b` (12 bits, high
    /// part non-zero) for the two colours, every later code 0 (keep).
    fn flat_block(a: u16, b: u16) -> [u8; 16] {
        let word = (u128::from(a) << 116) | (u128::from(b) << 104);
        word.to_le_bytes()
    }
    /// One 0x4000-byte chunk stored rotated, holding `logical`.
    fn rotated(logical: &[u8]) -> Vec<u8> {
        let mut chunk = logical.to_vec();
        chunk.resize(0x4000, 0);
        chunk.rotate_right(0x1ff8);
        chunk
    }
    pub(crate) fn header(format: u16, bits: u16, size: [u16; 2]) -> Vec<(u16, u16, Vec<u8>)> {
        vec![
            short(0x02, size[0]),
            short(0x03, size[1]),
            // Borders: top 1, left 2, bottom size − 1, right size − 2.
            short(0x04, 1),
            short(0x05, 2),
            short(0x06, size[1] - 1),
            short(0x07, size[0] - 2),
            short(0x09, 3),
            short(0x0a, bits),
            short(0x1c, 100),
            short(0x1d, 101),
            short(0x1e, 102),
            short(0x24, 512),
            short(0x25, 256),
            short(0x26, 384),
            short(0x17, 400),
            short(0x2d, format),
            text(271, "Panasonic"),
            text(272, "DMC-GX80"),
        ]
    }

    #[test]
    fn reads_the_header_facts() {
        let mut entries = header(4, 12, [28, 4]);
        // White balance presets: fine weather (9) and cloudy (10).
        let presets: Vec<u8> = [2u16, 9, 600, 256, 400, 10, 650, 256, 380]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        entries.push((0x27, 7, presets));
        // The camera's crop: top 1, left 3, bottom 3, right 25.
        entries.extend([
            short(0x2f, 1),
            short(0x30, 3),
            short(0x31, 3),
            short(0x32, 25),
        ]);
        let file = rw2(&entries, &[0; 0x4000]);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, crate::Variant::Rw2Delta);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Panasonic", "DMC-GX80")
        );
        assert_eq!([info.raw_width, info.raw_height], [28, 4]);
        // The left border once more past the right one: 26 + 2.
        assert_eq!([info.width, info.height, info.margins[0]], [28, 4, 0]);
        assert_eq!(info.sensor_borders, Some([2, 1, 24, 2]));
        // The crop starts on even rows and columns, keeping its far edges.
        assert_eq!(info.frame, [4, 2, 21, 1]);
        assert_eq!(info.aspect, [UNSET, UNSET, 0, 0]);
        // GBRG, the green beside blue the second.
        assert_eq!(info.cfa.as_ref().unwrap().colors, vec![3, 2, 0, 1]);
        assert_eq!(info.cam_mul, [2., 1., 1.5, 1.]);
        assert_eq!(info.presets.fine_weather, [600., 256., 400.]);
        assert_eq!(info.presets.shade, [0.; 3]);
        assert_eq!(info.iso, 400.);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        // Raw format 4: 15 above the tags; 12 bits.
        assert_eq!(s.black, [115, 116, 117, 116]);
        assert_eq!(s.white, 4095);
    }

    #[test]
    fn decodes_delta_blocks_of_rotated_chunks() {
        // 28 × 2 samples: four blocks, each colour flat.
        let logical: Vec<u8> = [0x123, 0x456, 0x789, 0x9ab]
            .iter()
            .flat_map(|&a| flat_block(a, a + 1))
            .collect();
        let file = rw2(&header(4, 12, [28, 2]), &rotated(&logical));
        let info = crate::open(&file).unwrap();
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(&s.data[..4], &[0x123, 0x124, 0x123, 0x124]);
        assert_eq!(s.data[14..16], [0x456, 0x457]);
        assert_eq!(s.data[55], 0x9ac);
        // Format 3 is not rotated.
        let file = rw2(&header(3, 12, [28, 2]), &logical);
        let info = crate::open(&file).unwrap();
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.data[28..30], [0x789, 0x78a]);
        assert_eq!(s.black[0], 115);
        // White is the range over the green level (0x25) by 256: 4095 ·
        // 256 / 264 = 3970.9.
        let entries: Vec<_> = header(3, 12, [28, 2])
            .into_iter()
            .map(|e| if e.0 == 0x25 { short(0x25, 264) } else { e })
            .collect();
        let file = rw2(&entries, &logical);
        let info = crate::open(&file).unwrap();
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.white, 3971);
    }

    #[test]
    fn decodes_packed_blocks_of_rotated_chunks() {
        // Raw format 5, 12 bits: ten samples per block from the lowest bit.
        let mut word = 0u128;
        for i in 0..10u128 {
            word |= (0x800 + i) << (12 * i);
        }
        let logical: Vec<u8> = (0..2).flat_map(|_| word.to_le_bytes()).collect();
        let file = rw2(&header(5, 12, [10, 2]), &rotated(&logical));
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, crate::Variant::Rw2PackedRotated);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        let want: [u16; 10] = std::array::from_fn(|i| 0x800 + i as u16);
        assert_eq!(s.data[..10], want);
        // Raw format 5 records black as it is.
        assert_eq!(s.black, [100, 101, 102, 101]);
    }

    #[test]
    fn decodes_seeded_and_unrotated_packed_blocks() {
        // Raw format 6, 14 bits: seeds 1000 and 2000, every code 0 (keep);
        // read 15 below the coded levels.
        let word = (1000u128 << 114) | (2000u128 << 100);
        let file = rw2(&header(6, 14, [11, 1]), &word.to_le_bytes());
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, crate::Variant::Rw2Seeded);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.data, [985, 1985].repeat(6)[..11]);
        // Black as the tags say; white 1/256 of the range below it.
        assert_eq!(s.black, [100, 101, 102, 101]);
        assert_eq!(s.white, 16383 - 64);
        // With a linearity limit (green's, 0x0F), white is that.
        let mut entries = header(6, 14, [11, 1]);
        entries.extend([short(0x0e, 16000), short(0x0f, 16380), short(0x10, 16000)]);
        let file = rw2(&entries, &word.to_le_bytes());
        let info = crate::open(&file).unwrap();
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.white, 16380);
        // Raw format 7: packed 14-bit samples, nine a block, not rotated.
        let mut word = 0u128;
        for i in 0..9u128 {
            word |= (0x2000 + i) << (14 * i);
        }
        let data = word.to_le_bytes().repeat(5);
        let file = rw2(&header(7, 14, [9, 5]), &data);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, crate::Variant::Rw2Packed);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        let row: [u16; 9] = std::array::from_fn(|i| 0x2000 + i as u16);
        assert_eq!(s.data[..36], row.repeat(4));
        // Read in whole groups of four rows: the fifth is 0.
        assert_eq!(s.data[36..], [0; 9]);
    }

    /// A tag of 16-bit words: the count, then the values (low and high
    /// words when `wide`).
    fn words(tag: u16, values: &[u32], wide: bool) -> (u16, u16, Vec<u8>) {
        let mut w = vec![values.len() as u16];
        for &v in values {
            w.push(v as u16);
            if wide {
                w.push((v >> 16) as u16);
            }
        }
        (tag, 7, w.iter().flat_map(|v| v.to_le_bytes()).collect())
    }

    /// A raw format 8 file of `width` × 2 samples in strips of `widths`,
    /// every strip's stream `stream` (bytes, bits from the lowest up).
    fn format8(width: u16, widths: &[u32], tops: &[u32], stream: &[u8]) -> Vec<u8> {
        let lefts: Vec<u32> = widths
            .iter()
            .scan(0, |x, w| {
                *x += w;
                Some(*x - w)
            })
            .zip(tops)
            .map(|(l, t)| l | t << 16)
            .collect();
        // Category 0 "0", 1 "10", 2 "110"; the others none.
        let mut codes = vec![1, 0b0, 2, 0b10, 3, 0b110];
        codes.resize(34, 0);
        let codes: Vec<u32> = codes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[l, c]| l | c << 16)
            .collect();
        let build = |at: u32| {
            let mut e = header(8, 16, [width, 2]);
            let n = widths.len();
            e.extend([
                words(0x40, &codes, true),
                words(0x44, &vec![at; n], true),
                words(0x45, &lefts, true),
                words(0x46, &vec![stream.len() as u32 * 8; n], true),
                words(0x47, widths, false),
                words(0x48, &vec![2; n], false),
            ]);
            rw2(&e, stream)
        };
        let at = build(0).len() - stream.len();
        build(at as u32)
    }

    #[test]
    fn decodes_prefix_coded_strips() {
        // Each strip 2 × 2: one cell, differences +1, +2, 0, −1 written most
        // significant bit first (10 1, 110 10, 0, 10 0), stored reversed.
        let bits = [1, 0, 1, 1, 1, 0, 1, 0, 0, 1, 0, 0];
        let mut stream = vec![0u8; 2];
        for (i, b) in bits.iter().enumerate() {
            stream[i / 8] |= b << (i % 8);
        }
        let file = format8(4, &[2, 2], &[0, 0], &stream);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, crate::Variant::Rw2Huffman);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        // Top left 1, bottom left 2, top right 0, bottom right −1 (0).
        assert_eq!(s.data, [1, 0, 1, 0, 2, 0, 2, 0]);
        assert_eq!(s.white, 65535);
        // Strips stacked in two rows are not decoded.
        let file = format8(2, &[2, 2], &[0, 1], &stream);
        let info = crate::open(&file).unwrap();
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            crate::decode(&info, &file, &cancel),
            Err(crate::Error::Unsupported(_))
        ));
    }

    #[test]
    fn decodes_the_oldest_files_words_and_packed_blocks() {
        // No raw format: compression 34828, 12-bit samples in the high bits
        // of 16-bit words, whatever the bit depth tag says.
        let mut entries = header(4, 14, [2, 2]);
        entries.retain(|e| e.0 != 0x2d && !(0x24..=0x26).contains(&e.0));
        entries.extend([short(0x0b, 34828), short(0x11, 512), short(0x12, 384)]);
        // Old presets: count, then (light source, red, blue).
        let presets: Vec<u8> = [1u16, 9, 423, 363]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        entries.push((0x13, 7, presets));
        let data: Vec<u8> = [0x1230u16, 0x4560, 0x7890, 0xfff0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let file = rw2(&entries, &data);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, crate::Variant::Rw2Unpacked);
        assert_eq!(info.cam_mul, [2., 1., 1.5, 1.]);
        assert_eq!(info.presets.fine_weather, [423., 256., 363.]);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.data, [0x123, 0x456, 0x789, 0xfff]);
        assert_eq!(s.white, 4095);
        // Raw format 2: packed 12-bit blocks, not rotated, black as tagged.
        let mut word = 0u128;
        for i in 0..10u128 {
            word |= (0x100 + i) << (12 * i);
        }
        let file = rw2(&header(2, 12, [10, 1]), &word.to_le_bytes());
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, crate::Variant::Rw2Packed);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        let want: [u16; 10] = std::array::from_fn(|i| 0x100 + i as u16);
        assert_eq!(s.data, want);
        assert_eq!(s.black, [100, 101, 102, 101]);
    }

    /// A little-endian TIFF whose IFD0 holds SHORT `entries` (and, for a tag
    /// 0x8769, a second directory with `exif` at the offset that follows).
    fn tiff(entries: &[(u16, u16)], exif: &[(u16, u16)]) -> Vec<u8> {
        let ifd = |entries: &[(u16, u16)], kind: u16| {
            let mut b = (entries.len() as u16).to_le_bytes().to_vec();
            for &(tag, v) in entries {
                b.extend(tag.to_le_bytes());
                b.extend(kind.to_le_bytes());
                b.extend(1u32.to_le_bytes());
                b.extend(u32::from(v).to_le_bytes());
            }
            b.extend(0u32.to_le_bytes());
            b
        };
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        if exif.is_empty() {
            out.extend(ifd(entries, 3));
        } else {
            // IFD0: one LONG entry, the EXIF directory right after it.
            out.extend(ifd(&[(0x8769, 8 + 18)], 4));
            out.extend(ifd(exif, 3));
        }
        out
    }

    #[test]
    fn reads_the_35mm_focal_length_from_the_camera_directory() {
        let jpeg_exif = tiff(&[], &[(41989, 30)]);
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend(((jpeg_exif.len() + 8) as u16).to_be_bytes());
        jpeg.extend(b"Exif\0\0");
        jpeg.extend(&jpeg_exif);
        jpeg.extend([0xff, 0xd9]);
        let mut entries = header(4, 12, [28, 2]);
        entries.push((0x2e, 7, jpeg));
        // The embedded JPEG's EXIF value (cropped to the aspect ratio) is
        // not used.
        let file = rw2(&entries, &[0; 0x4000]);
        assert_eq!(crate::open(&file).unwrap().focal_35mm, 0.);
        // The camera directory (tag 0x120) wins: tag 0x1203.
        entries.push((0x120, 7, tiff(&[(0x1203, 25)], &[])));
        let file = rw2(&entries, &[0; 0x4000]);
        assert_eq!(crate::open(&file).unwrap().focal_35mm, 25.);
        // Damaged directories and JPEGs are errors or ignored, not panics.
        for at in 4..file.len() - 0x4000 {
            let mut bad = file.clone();
            bad[at] ^= 0xff;
            let _ = crate::open(&bad);
        }
    }

    #[test]
    fn truncated_and_garbage_files_are_errors_not_panics() {
        let file = rw2(&header(4, 12, [28, 2]), &[0x5a; 0x4000]);
        let cancel = AtomicBool::new(false);
        for len in (0..file.len()).step_by(7) {
            if let Ok(info) = crate::open(&file[..len]) {
                let _ = crate::decode(&info, &file[..len], &cancel);
            }
        }
        // The data cut short is an error.
        let short = &file[..file.len() - 0x3fe0];
        let info = crate::open(short).unwrap();
        assert!(crate::decode(&info, short, &cancel).is_err());
        // Every byte of the header changed in turn.
        for at in 4..file.len() - 0x4000 {
            let mut bad = file.clone();
            bad[at] ^= 0xff;
            if let Ok(info) = crate::open(&bad) {
                let _ = crate::decode(&info, &bad, &cancel);
            }
        }
        // The same for raw format 8's strip tags and streams.
        let file = format8(4, &[2, 2], &[0, 0], &[0xa5, 0x5a]);
        for at in 4..file.len() {
            let mut bad = file.clone();
            bad[at] ^= 0xff;
            if let Ok(info) = crate::open(&bad) {
                let _ = crate::decode(&info, &bad, &cancel);
            }
            if let Ok(info) = crate::open(&file[..at]) {
                let _ = crate::decode(&info, &file[..at], &cancel);
            }
        }
        // An unknown raw format is unsupported.
        let file = rw2(&header(9, 12, [28, 2]), &[0; 64]);
        assert!(matches!(
            crate::open(&file),
            Err(crate::Error::Unsupported(_))
        ));
    }
}
