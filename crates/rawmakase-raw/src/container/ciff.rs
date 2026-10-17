//! Canon CRW: a CIFF file (Camera Image File Format), a heap of records in
//! directory blocks that may nest, each block's directory at the offset its
//! last four bytes give, records' values either in the block or in the entry
//! itself. Holds Canon's compressed sensor data (`codec::crw`).
//!
//! Sources: ExifTool's documentation of the CRW format and of the CanonRaw
//! tag names (the header, the directory blocks and entries, the record IDs,
//! SensorInfo, DecoderTable, ImageInfo, ShotInfo and the camera settings),
//! and our own measurements on the test dataset's files (where the low bits
//! and the coded data sit in the raw data record, and the levels).
use crate::codec::crw;
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use std::ops::Range;
use std::sync::atomic::AtomicBool;

/// The most records and the deepest nesting read: files with more are
/// malformed or hostile.
const MAX_RECORDS: usize = 4096;
const MAX_DEPTH: u32 = 8;

/// The most words read from one record: its leading fields.
const MAX_FIELDS: usize = 64;

/// The longest make and model names read.
const MAX_NAME: usize = 64;

/// The bodies whose CRW sensor has a cyan, magenta, yellow and green filter
/// (as Adobe DNG Converter writes their CFA), which RAWmakase does not
/// develop.
const FOUR_COLOUR: &[&str] = &["PowerShot G1", "PowerShot Pro70", "PowerShot Pro90 IS"];

/// Whether `bytes` is a CIFF file: "II", the header's length, "HEAPCCDR".
pub(crate) fn is_ciff(bytes: &[u8]) -> bool {
    bytes.get(..2) == Some(b"II") && bytes.get(6..14) == Some(b"HEAPCCDR")
}

/// The records of a CIFF file by their ID (data format and index), the first
/// of each ID found depth first: where each value is in the file (in its
/// block, or in its directory entry).
struct Records<'a> {
    bytes: &'a [u8],
    records: Vec<(u16, Range<usize>)>,
}

impl<'a> Records<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self> {
        let header = u32_at(bytes, 2).ok_or_else(|| corrupt("CRW header"))? as usize;
        let mut r = Records {
            bytes,
            records: Vec::new(),
        };
        if header < 14 || header >= bytes.len() {
            return Err(corrupt("CRW header"));
        }
        r.block(header..bytes.len(), 0)?;
        Ok(r)
    }

    fn block(&mut self, block: Range<usize>, depth: u32) -> Result<()> {
        if depth > MAX_DEPTH || block.end < block.start + 6 {
            return Err(corrupt("CRW directory"));
        }
        let start = block.start;
        let dir = start
            .checked_add(
                u32_at(self.bytes, block.end - 4).ok_or_else(|| corrupt("CRW directory"))? as usize,
            )
            .filter(|&d| d < block.end)
            .ok_or_else(|| corrupt("CRW directory"))?;
        let count = usize::from(u16_at(self.bytes, dir).ok_or_else(|| corrupt("CRW directory"))?);
        for i in 0..count {
            let at = dir + 2 + 10 * i;
            let entry = self
                .bytes
                .get(at..at + 10)
                .filter(|_| at + 10 <= block.end)
                .ok_or_else(|| corrupt("CRW directory entry"))?;
            if self.records.len() >= MAX_RECORDS {
                return Err(corrupt("CRW records"));
            }
            let tag = u16::from_le_bytes([entry[0], entry[1]]);
            let id = tag & 0x3fff;
            match tag & 0xc000 {
                0x0000 => {
                    let size = u32_at(entry, 2).unwrap_or(0) as usize;
                    let offset = u32_at(entry, 6).unwrap_or(0) as usize;
                    let range = start
                        .checked_add(offset)
                        .and_then(|s| Some(s..s.checked_add(size)?))
                        .filter(|r| r.end <= dir)
                        .ok_or_else(|| corrupt("CRW record"))?;
                    // Subdirectories (data formats 0x2800 and 0x3000).
                    if matches!(id >> 11, 5 | 6) {
                        self.block(range.clone(), depth + 1)?;
                    }
                    self.records.push((id, range));
                }
                // A value of up to eight bytes in the entry's size and offset
                // fields.
                0x4000 => self.records.push((id, at + 2..at + 10)),
                _ => {}
            }
        }
        Ok(())
    }

    fn get(&self, id: u16) -> Option<&'a [u8]> {
        self.bytes.get(self.range(id)?)
    }

    fn range(&self, id: u16) -> Option<Range<usize>> {
        self.records
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, r)| r.clone())
    }

    /// A record's first `n` 16-bit words at most (the fields RAWmakase
    /// reads): a record is as long as the file says, so it is never copied
    /// whole.
    fn words(&self, id: u16, n: usize) -> Vec<u16> {
        self.get(id)
            .map(|b| {
                b.as_chunks::<2>()
                    .0
                    .iter()
                    .take(n.min(MAX_FIELDS))
                    .map(|c| u16::from_le_bytes(*c))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A record's first `n` 32-bit words at most.
    fn longs(&self, id: u16, n: usize) -> Vec<u32> {
        self.get(id)
            .map(|b| {
                b.as_chunks::<4>()
                    .0
                    .iter()
                    .take(n.min(MAX_FIELDS))
                    .map(|c| u32::from_le_bytes(*c))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// Where a CRW's sensor data is.
#[derive(Clone, Debug)]
pub(crate) struct CrwPlan {
    /// The raw data record.
    pub raw: Range<usize>,
    pub width: u32,
    pub height: u32,
    /// DecoderTable's table number.
    pub table: u32,
}

/// The raw data record's parts: the low bits (12-bit files) and the coded
/// data, after a 514-byte header (our measurements). The low bits are there
/// when the first 0xFF past the header is not a stuffed 0xFF 0x00, as it would
/// be in coded data, or when there is none before the low bits would end.
fn parts(raw: &[u8], samples: usize) -> (Option<&[u8]>, &[u8]) {
    let low = samples / 4;
    let window = raw.get(514..low.min(raw.len())).unwrap_or(&[]);
    let low_bits = match window.iter().position(|&b| b == 0xff) {
        Some(i) => window.get(i + 1).is_none_or(|&b| b != 0),
        None => true,
    };
    if low_bits {
        let coded = raw.get(low.saturating_add(514)..).unwrap_or(&[]);
        (raw.get(..low), coded)
    } else {
        (None, raw.get(514..).unwrap_or(&[]))
    }
}

/// Reads a CRW's header facts.
pub(crate) fn open(bytes: &[u8]) -> Result<RawInfo> {
    let r = Records::parse(bytes)?;
    let raw = r
        .range(0x2005)
        .ok_or_else(|| Error::Unsupported("CRW without raw data".into()))?;
    let sensor = r.words(0x1031, 9);
    let [width, height] = match sensor.as_slice() {
        [_, w, h, ..] => [u32::from(*w), u32::from(*h)],
        _ => return Err(corrupt("CRW SensorInfo")),
    };
    crate::io::samples(width, height, 1)?;
    let table = *r
        .longs(0x1835, 1)
        .first()
        .ok_or_else(|| Error::Unsupported("CRW without a decoder table".into()))?;
    let mut info = crate::info::base(Variant::CrwCompressed);
    let names = r.get(0x080a).unwrap_or(&[]);
    let mut names = names.split(|&b| b == 0).map(|s| {
        String::from_utf8_lossy(&s[..s.len().min(MAX_NAME)])
            .trim()
            .to_string()
    });
    info.make = names.next().unwrap_or_default();
    info.model = names.next().unwrap_or_default();
    if FOUR_COLOUR.contains(&info.model.trim_start_matches("Canon ")) {
        return Err(Error::Unsupported(
            "CRW of a cyan, magenta, yellow and green filter".into(),
        ));
    }
    info.raw_width = width;
    info.raw_height = height;
    let notes = crate::vendor::canon::crw_notes(&sensor, r.longs(0x1834, 1).first().copied());
    // The visible area: SensorInfo's borders, unless the camera table says
    // otherwise.
    let [left, top, right, bottom] = notes
        .sensor
        .map(|s| [s[2], s[3], s[4], s[5]])
        .filter(|b| b[2] > b[0] && b[3] > b[1] && b[2] < width && b[3] < height)
        .unwrap_or([0, 0, width - 1, height - 1]);
    info.margins = [left, top];
    info.width = right - left + 1;
    info.height = bottom - top + 1;
    let cfa = crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors: vec![0, 1, 1, 2],
    });
    info.cfa = Some(cfa.shifted(top as usize, left as usize));
    notes.apply(&mut info, 1);
    // ImageInfo's rotation in degrees.
    let image_info = r.longs(0x1810, 4);
    info.flip = match image_info.get(3).map(|&v| v as i32) {
        Some(90) => 6,
        Some(180) => 3,
        Some(270) | Some(-90) => 5,
        _ => 0,
    };
    crate::vendor::canon::crw_exposure(
        &mut info,
        &r.words(0x102a, 6),
        &r.words(0x1029, 2),
        &r.words(0x102d, 26),
    );
    crate::vendor::canon::crw_white_balance(
        &mut info,
        &r.words(0x10a9, 13),
        &r.words(0x102c, 54),
        // ColorInfo1's levels, of the records of 2048 bytes only (plain
        // levels; our measurements).
        &if r.get(0x0032).is_some_and(|b| b.len() == 2048) {
            r.words(0x0032, 52)
        } else {
            Vec::new()
        },
    );
    for id in [0x2007, 0x2008] {
        if let Some(range) = r.range(id) {
            info.previews.push(crate::preview::Candidate {
                offset: range.start as u64,
                len: range.len() as u64,
                pixels: 0,
            });
        }
    }
    info.plan = Plan::Crw(CrwPlan {
        raw,
        width,
        height,
        table,
    });
    Ok(info)
}

/// Decodes the sensor data, with the black level measured on its masked
/// borders.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &CrwPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let raw = bytes
        .get(plan.raw.clone())
        .ok_or_else(|| corrupt("CRW raw data"))?;
    let count = crate::io::samples(plan.width, plan.height, 1)?;
    let (low_bits, coded) = parts(raw, count);
    // Every coded block of 64 samples takes two bits at least.
    if (count as u64).div_ceil(64) * 2 > (coded.len() as u64).saturating_mul(8) {
        return Err(corrupt("CRW data too short for its image"));
    }
    let mut data = crate::io::zeroed(count)?;
    let image = crw::Image {
        width: plan.width as usize,
        height: plan.height as usize,
        table: plan.table as usize,
        coded,
        low_bits,
    };
    crw::decode(&image, &mut data, cancel)?;
    let width = plan.width as usize;
    let camera = || crate::facts::camera(&info.make, &info.model, false, [plan.width, plan.height]);
    let white = camera()
        .find_map(|c| c.white)
        .unwrap_or(if low_bits.is_some() { 4095 } else { 1023 });
    let area = [info.margins[0], info.margins[1], info.width, info.height];
    let cfa = info
        .cfa
        .as_ref()
        .ok_or_else(|| corrupt("CRW without a colour filter"))?;
    let black = crate::levels::masked_black(&data, width, area, cfa, None, 2).unwrap_or([0; 4]);
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area,
        black,
        black_pattern: (0, 0, Vec::new()),
        white,
        linear_white: [0; 4],
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    /// A record: its tag (data format, storage and index) and its value.
    type Record = (u16, Vec<u8>);

    /// A heap block: the values of `records` (stored in the block, or in
    /// the entry for tags of storage 0x4000), then the directory and its
    /// offset.
    fn block(records: &[Record]) -> Vec<u8> {
        let mut data = Vec::new();
        let mut entries = Vec::new();
        for (tag, value) in records {
            entries.extend(tag.to_le_bytes());
            if tag & 0x4000 != 0 {
                let mut inline = value.clone();
                inline.resize(8, 0);
                entries.extend(inline);
            } else {
                entries.extend((value.len() as u32).to_le_bytes());
                entries.extend((data.len() as u32).to_le_bytes());
                data.extend(value);
            }
        }
        let dir = data.len() as u32;
        data.extend((records.len() as u16).to_le_bytes());
        data.extend(entries);
        data.extend(dir.to_le_bytes());
        data
    }

    /// A CIFF file of the heap block `heap`.
    fn ciff(heap: Vec<u8>) -> Vec<u8> {
        let mut f = b"II".to_vec();
        f.extend(26u32.to_le_bytes());
        f.extend(b"HEAPCCDR");
        f.extend([0; 12]);
        f.extend(heap);
        f
    }

    fn shorts(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn longs(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// A CRW of decoder table 0 whose first 128 samples are two blocks of
    /// coded differences (those `codec::crw`'s test decodes), after their
    /// low bits and the 514-byte header.
    fn sample(width: u16, height: u16, rotation: u32) -> Vec<u8> {
        let coded = crate::codec::crw::tests::stream(&[
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
        let mut raw = vec![0b11_10_01_00u8; usize::from(width) * usize::from(height) / 4];
        raw.extend([0; 514]);
        raw.extend(coded);
        ciff(block(&[
            (0x080a, b"Canon\0PowerShot T1\0".to_vec()),
            (
                0x1031,
                shorts(&[0, width, height, 0, 0, 0, 0, width - 1, height - 1]),
            ),
            (0x1835, longs(&[0])),
            (0x1810, longs(&[0, 0, 0, rotation])),
            // FocalLength, in its entry: 50 mm.
            (0x5029, shorts(&[0, 50])),
            (0x2005, raw),
        ]))
    }

    #[test]
    fn reads_records_and_decodes_the_sensor() {
        let bytes = sample(128, 1, 90);
        let info = crate::open(&bytes).unwrap();
        assert_eq!(info.variant, crate::Variant::CrwCompressed);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Canon", "PowerShot T1")
        );
        assert_eq!((info.raw_width, info.raw_height), (128, 1));
        assert_eq!((info.width, info.height), (128, 1));
        assert_eq!(info.flip, 6);
        assert_eq!(info.focal, 50.);
        let sensor = crate::decode(&info, &bytes, &AtomicBool::new(false)).unwrap();
        // The 10-bit values, then their low bits, four to a byte.
        assert_eq!(
            &sensor.data[..4],
            &[517 << 2, 513 << 2 | 1, 510 << 2 | 2, 513 << 2 | 3]
        );
        assert_eq!(sensor.white, 4095);
    }

    #[test]
    fn records_as_long_as_the_file_are_read_by_their_fields() {
        // SensorInfo, ColorInfo1 and the names spanning 16 MB each (a file
        // may claim any length): their leading fields are read, not copies.
        let mut sensor = shorts(&[0, 128, 1, 0, 0, 0, 0, 127, 0]);
        sensor.resize(16 << 20, 0x41);
        let mut names = b"Canon\0PowerShot T1\0".to_vec();
        names.resize(16 << 20, b'x');
        let coded = crate::codec::crw::tests::stream(&[
            "010",
            "101",
            "1100",
            "1",
            "00",
            "000",
            "111111011",
            "101",
            "01",
            "111011",
            "10",
            "111111011",
        ]);
        let mut raw = vec![0u8; 32];
        raw.extend([0; 514]);
        raw.extend(coded);
        let bytes = ciff(block(&[
            (0x080a, names),
            (0x1031, sensor),
            (0x1835, longs(&[0])),
            (0x0032, vec![0xff; 16 << 20]),
            (0x2005, raw),
        ]));
        let info = crate::open(&bytes).unwrap();
        assert_eq!((info.raw_width, info.raw_height), (128, 1));
        assert_eq!(info.model, "PowerShot T1");
        assert!(crate::decode(&info, &bytes, &AtomicBool::new(false)).is_ok());
    }

    #[test]
    fn four_colour_bodies_are_unsupported() {
        let bytes = sample(128, 1, 0);
        let at = bytes
            .windows(12)
            .position(|w| w == b"PowerShot T1")
            .unwrap();
        let mut g1 = bytes.clone();
        g1[at..at + 12].copy_from_slice(b"PowerShot G1");
        assert!(matches!(
            crate::open(&g1),
            Err(crate::Error::Unsupported(_))
        ));
    }

    #[test]
    fn decoding_stops_when_cancelled() {
        let bytes = sample(128, 1, 0);
        let info = crate::open(&bytes).unwrap();
        assert_eq!(
            crate::decode(&info, &bytes, &AtomicBool::new(true)).unwrap_err(),
            crate::Error::Cancelled
        );
    }

    #[test]
    fn hostile_files_are_errors_not_panics() {
        // A sensor far larger than its coded data: refused before the
        // samples are allocated.
        let bytes = sample(0xfffe, 0xfffe, 0);
        if let Ok(info) = crate::open(&bytes) {
            assert!(crate::decode(&info, &bytes, &AtomicBool::new(false)).is_err());
        }
        // Directories nested deeper than any file's.
        let mut nested = block(&[(0x1835, longs(&[0]))]);
        for _ in 0..12 {
            nested = block(&[(0x300a, nested)]);
        }
        assert!(crate::open(&ciff(nested)).is_err());
        // A directory offset past the block.
        let mut bad = sample(128, 1, 0);
        let end = bad.len();
        bad[end - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(crate::open(&bad).is_err());
        // Every truncation and a scrambled copy.
        let bytes = sample(128, 1, 0);
        for end in 0..bytes.len() {
            if let Ok(info) = crate::open(&bytes[..end]) {
                let _ = crate::decode(&info, &bytes[..end], &AtomicBool::new(false));
            }
        }
        let mut garbage = bytes.clone();
        for (i, b) in garbage.iter_mut().enumerate().skip(14) {
            *b ^= (i * 131 % 251) as u8;
        }
        if let Ok(info) = crate::open(&garbage) {
            let _ = crate::decode(&info, &garbage, &AtomicBool::new(false));
        }
    }
}
