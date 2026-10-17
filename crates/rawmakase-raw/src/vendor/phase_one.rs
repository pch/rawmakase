//! Phase One's directory (IIQ, and the Phase One backs' TIFFs, Leaf's
//! Credo too): a block in the EXIF maker note, eight bytes into the file,
//! of its own layout. `IIII` (or `MMMM`) and a 4-byte signature, then the
//! offset of a directory: an entry count, four bytes, and 16-byte entries
//! (tag, type, size in bytes, value or offset), offsets counting from the
//! block's start. It gives the sensor and image sizes, the raw format and
//! data, each row's offset, the black level, the white balance and the
//! orientation.
//!
//! Sources: ExifTool's PhaseOne tag documentation (tag names and the raw
//! format values), and our own measurements on the test dataset's files
//! (the entry layout, which tags are integers and which floats).
use crate::io::{Bytes, Order};

const ORIENTATION: u32 = 0x0100;
const WB_LEVELS: u32 = 0x0107;
const SENSOR_WIDTH: u32 = 0x0108;
const SENSOR_HEIGHT: u32 = 0x0109;
const LEFT_MARGIN: u32 = 0x010a;
const TOP_MARGIN: u32 = 0x010b;
const IMAGE_WIDTH: u32 = 0x010c;
const IMAGE_HEIGHT: u32 = 0x010d;
const RAW_FORMAT: u32 = 0x010e;
const RAW_DATA: u32 = 0x010f;
const STRIP_OFFSETS: u32 = 0x021c;
const BLACK_LEVEL: u32 = 0x021d;
const LENS_MODEL: u32 = 0x0412;

/// The most entries a directory is read with.
const MAX_ENTRIES: u32 = 4096;
/// The most bytes of a text tag read.
const MAX_TEXT: u64 = 64;

/// One entry: its tag, its size in bytes and its value (or offset from the
/// block's start).
#[derive(Clone, Copy, Debug)]
struct Entry {
    tag: u32,
    size: u32,
    value: u32,
}

/// What a Phase One directory says, the values RAWmakase reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Notes {
    /// The byte order of the block (and of the raw data's words).
    pub order: Option<Order>,
    pub raw_size: [u32; 2],
    /// The image area: left, top, width, height in sensor coordinates.
    pub image: [u32; 4],
    /// RawFormat: 0 uncompressed, 3 IIQ L, 5 IIQ S, 6 IIQ Sv2, 8 IIQ L16.
    pub format: u32,
    /// The raw data's offset in the file and its size.
    pub data: (u64, u64),
    /// Where each row's offset (from the data's start) is in the file, and
    /// the size of that table.
    pub strips: Option<(u64, u64)>,
    pub black: Option<u32>,
    /// As-shot white balance levels: red, green, blue.
    pub wb: Option<[f32; 3]>,
    /// CameraOrientation: 0 normal, 1 a quarter turn clockwise, 2 a quarter
    /// turn counter-clockwise, 3 a half turn.
    pub orientation: u32,
    pub lens: String,
}

/// The directory of the block at `base` in `file`, when there is one.
pub(crate) fn read(file: &[u8], base: u64) -> Option<Notes> {
    let order = match file.get(usize::try_from(base).ok()?..)?.get(..4)? {
        b"IIII" => Order::Little,
        b"MMMM" => Order::Big,
        _ => return None,
    };
    let b = Bytes::new(file, order);
    let at = base.checked_add(u64::from(b.u32(base + 8).ok()?))?;
    let count = b.u32(at).ok()?;
    if count == 0 || count > MAX_ENTRIES {
        return None;
    }
    let mut entries = Vec::new();
    for i in 0..u64::from(count) {
        let e = at + 8 + i * 16;
        entries.push(Entry {
            tag: b.u32(e).ok()?,
            size: b.u32(e + 8).ok()?,
            value: b.u32(e + 12).ok()?,
        });
    }
    let get = |tag: u32| entries.iter().find(|e| e.tag == tag).copied();
    let int = |tag: u32| get(tag).filter(|e| e.size == 4).map(|e| e.value);
    // A value longer than four bytes, where it lies inside the file.
    let block = |tag: u32| {
        let e = get(tag)?;
        let start = base.checked_add(u64::from(e.value))?;
        let end = start.checked_add(u64::from(e.size))?;
        (end <= file.len() as u64).then_some((start, u64::from(e.size)))
    };
    let wb = block(WB_LEVELS).and_then(|(at, size)| {
        let level = |i: u64| b.u32(at + 4 * i).ok().map(f32::from_bits);
        let levels = [level(0)?, level(1)?, level(2)?];
        (size >= 12 && levels.iter().all(|v| v.is_finite() && *v > 0.)).then_some(levels)
    });
    let lens = block(LENS_MODEL)
        .and_then(|(at, size)| b.slice(at, size.min(MAX_TEXT)).ok())
        .map(|text| {
            let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
            String::from_utf8_lossy(&text[..end]).trim().to_string()
        })
        .unwrap_or_default();
    let int0 = |tag| int(tag).unwrap_or(0);
    Some(Notes {
        order: Some(order),
        raw_size: [int0(SENSOR_WIDTH), int0(SENSOR_HEIGHT)],
        image: [
            int0(LEFT_MARGIN),
            int0(TOP_MARGIN),
            int0(IMAGE_WIDTH),
            int0(IMAGE_HEIGHT),
        ],
        format: int0(RAW_FORMAT),
        data: block(RAW_DATA).unwrap_or((0, 0)),
        strips: block(STRIP_OFFSETS),
        black: int(BLACK_LEVEL),
        wb,
        orientation: int0(ORIENTATION),
        lens,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A little-endian Phase One block of `entries` (tag, size, value; a
    /// value is an offset from the block's start when the size is over
    /// four), its directory at 16.
    pub(crate) fn block(entries: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut out = b"IIII\0\0\0\0".to_vec();
        out.extend(16u32.to_le_bytes());
        out.extend([0; 4]);
        out.extend((entries.len() as u32).to_le_bytes());
        out.extend([0; 4]);
        for &(tag, size, value) in entries {
            for v in [tag, 1, size, value] {
                out.extend(v.to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn reads_the_directory_and_its_blocks() {
        let mut b = block(&[
            (SENSOR_WIDTH, 4, 12),
            (SENSOR_HEIGHT, 4, 8),
            (LEFT_MARGIN, 4, 3),
            (RAW_FORMAT, 4, 3),
            (BLACK_LEVEL, 4, 1024),
            (WB_LEVELS, 12, 200),
            (LENS_MODEL, 6, 212),
            (ORIENTATION, 4, 1),
        ]);
        b.resize(200, 0);
        for v in [1.5f32, 1., 2.25] {
            b.extend(v.to_bits().to_le_bytes());
        }
        b.extend(b"Lens\0x");
        let notes = read(&b, 0).unwrap();
        assert_eq!(notes.raw_size, [12, 8]);
        assert_eq!(notes.image, [3, 0, 0, 0]);
        assert_eq!(notes.format, 3);
        assert_eq!(notes.black, Some(1024));
        assert_eq!(notes.wb, Some([1.5, 1., 2.25]));
        assert_eq!(notes.lens, "Lens");
        assert_eq!(notes.orientation, 1);
        // A block past the end of the file is not read.
        let short = &b[..205];
        assert_eq!(read(short, 0).unwrap().wb, None);
        // No block, or one whose directory is past the end.
        assert!(read(&b"IIIJ"[..], 0).is_none());
        assert!(read(&b[..20], 0).is_none());
    }
}
