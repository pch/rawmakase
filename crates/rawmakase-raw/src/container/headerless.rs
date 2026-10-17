//! RAW files with no header at all (CHDK's from Canon PowerShots, action
//! cameras', the Pentax Optio S4's): only the file's size says what it is,
//! so the camera table lists each size with its camera, image size, bit
//! depth, visible area and colour filter. The samples are packed, most
//! significant bit first, in 16-bit little-endian words or (where the row
//! says) bytes or 32-bit words, rows padded where it gives their size, in
//! two fields or shifted down where it says; black is measured on the masked columns unless the row gives it.
//! A row may skip a header of the camera's and take the codes through a
//! curve, its own or one the header keeps (the Kodak C603's).
//!
//! Sources: our own measurements on the test dataset's files; the C603
//! "CCDI" curve measured from LibRaw's output, no public source giving it.
use crate::codec::packed;
use crate::error::Result;
use crate::facts::Headerless;
use crate::info::{Cfa, RawInfo};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use std::sync::atomic::{AtomicBool, Ordering};

/// A headerless file's table row.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HeaderlessPlan {
    pub row: &'static Headerless,
}

/// The header facts of a headerless file the table knows by its size.
pub(crate) fn open(bytes: &[u8]) -> Option<RawInfo> {
    let row = crate::facts::headerless(bytes.len() as u64)?;
    let [width, height] = row.raw_size;
    let [left, top, visible_width, visible_height] = row.visible;
    let colors: Vec<u8> = row
        .cfa
        .bytes()
        .map(|c| match c {
            b'R' => 0,
            b'G' => 1,
            _ => 2,
        })
        .collect();
    let cfa = crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors,
    });
    let mut info = crate::info::base(Variant::HeaderlessPacked);
    info.make = row.make.clone();
    info.model = row.model.clone();
    info.raw_width = width;
    info.raw_height = height;
    info.margins = [left, top];
    info.width = visible_width;
    info.height = visible_height;
    info.cfa = Some(cfa.shifted(top as usize, left as usize));
    info.plan = Plan::Headerless(HeaderlessPlan { row });
    Some(info)
}

/// Unpacks the samples, with the black level measured on the masked borders.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &HeaderlessPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let [width, height] = plan.row.raw_size;
    let count = crate::io::samples(width, height, 1)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(crate::error::Error::Cancelled);
    }
    let mut data = crate::io::zeroed(count)?;
    // Packed most significant bit first in words of the row's (16-bit
    // little-endian when it says none), each row a whole number of words.
    let row = plan.row;
    let packing = packed::Packing {
        bits: row.bits,
        word: row.word.unwrap_or(2),
        pad: false,
        swap: false,
    };
    let stride = usize::try_from(row.stride()).map_err(|_| crate::error::corrupt("row size"))?;
    if !stride.is_multiple_of(packing.word) || stride < packing.row_bytes(width as usize) {
        return Err(crate::error::Error::Unsupported(
            "headerless rows of part of a word".into(),
        ));
    }
    let offset =
        usize::try_from(row.offset.unwrap_or(0)).map_err(|_| crate::error::corrupt("offset"))?;
    let samples = bytes.get(offset..).unwrap_or(&[]);
    if row.fields == Some(true) {
        packed::fields(samples, packing, width as usize, stride, &mut data, cancel)?;
    } else {
        packed::rows(samples, packing, width as usize, stride, &mut data, cancel)?;
    }
    // The codes' values: the row's curve, or the one the file keeps.
    let kept;
    let curve = match (&row.curve, row.curve_at) {
        (Some(c), _) => Some(c.as_slice()),
        (None, Some(at)) => {
            let n = 1usize << row.bits.min(16);
            let b = crate::io::Bytes::new(bytes, crate::io::Order::Little);
            let raw = b.slice(at, 2 * n as u64)?;
            kept = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect::<Vec<_>>();
            Some(kept.as_slice())
        }
        (None, None) => None,
    };
    if let Some(curve) = curve.filter(|c| !c.is_empty()) {
        for (i, v) in data.iter_mut().enumerate() {
            if i % (1 << 20) == 0 && cancel.load(Ordering::Relaxed) {
                return Err(crate::error::Error::Cancelled);
            }
            *v = curve[usize::from(*v).min(curve.len() - 1)];
        }
    }
    if let Some(shift) = row.shift.filter(|&s| s > 0) {
        for (i, v) in data.iter_mut().enumerate() {
            if i % (1 << 20) == 0 && cancel.load(Ordering::Relaxed) {
                return Err(crate::error::Error::Cancelled);
            }
            *v >>= shift.min(15);
        }
    }
    let area = [info.margins[0], info.margins[1], info.width, info.height];
    let cfa = info
        .cfa
        .as_ref()
        .ok_or_else(|| crate::error::corrupt("no colour filter"))?;
    let black = match row.black {
        Some(b) => [b; 4],
        None => crate::levels::masked_black(&data, width as usize, area, cfa, row.black_columns, 0)
            .unwrap_or([0; 4]),
    };
    let bits = row.bits - row.shift.unwrap_or(0).min(row.bits - 1);
    Ok(Sensor {
        width,
        height,
        channels: 1,
        data,
        area,
        black,
        black_pattern: (0, 0, Vec::new()),
        // The row's white, else the curve's top, else the samples' range.
        white: row
            .white
            .or(curve.and_then(|c| c.iter().max()).map(|&m| u32::from(m)))
            .unwrap_or((1 << bits) - 1),
        linear_white: [0; 4],
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    #[test]
    fn files_are_known_by_their_size() {
        // The size of a PowerShot S2 IS file: 2672 × 1968 samples of 10 bits,
        // every sample 0x155 (bits 0101010101 repeating: the words 0x5555).
        let bytes = vec![0x55u8; 6_573_120];
        let info = crate::open(&bytes).unwrap();
        assert_eq!(info.variant, crate::Variant::HeaderlessPacked);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Canon", "PowerShot S2 IS")
        );
        assert_eq!((info.raw_width, info.raw_height), (2672, 1968));
        assert_eq!(info.margins, [12, 8]);
        let sensor = crate::decode(&info, &bytes, &AtomicBool::new(false)).unwrap();
        assert!(sensor.data.iter().all(|&v| v == 0x155));
        assert_eq!(sensor.black, [0x155; 4]);
        assert_eq!(sensor.white, 1023);
        // One byte more or less is no known file.
        assert!(crate::open(&bytes[1..]).is_err());
        assert_eq!(
            crate::decode(&info, &bytes, &AtomicBool::new(true)).unwrap_err(),
            crate::Error::Cancelled
        );
    }

    #[test]
    fn rows_may_be_padded_byte_packed_or_shifted() {
        // A Pentax Optio S4 file: rows of 2346 12-bit samples packed in
        // bytes (0x12 0x34 0x56: 0x123 and 0x456), padded to 3520 bytes.
        let mut row = [0x12u8, 0x34, 0x56].repeat(1173);
        row.push(0xff);
        let bytes = row.repeat(1737);
        let info = crate::open(&bytes).unwrap();
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Pentax", "Optio S4")
        );
        assert_eq!((info.width, info.height), (2324, 1737));
        let s = crate::decode(&info, &bytes, &AtomicBool::new(false)).unwrap();
        assert_eq!(&s.data[..4], &[0x123, 0x456, 0x123, 0x456]);
        assert_eq!(s.data[2346], 0x123);
        assert_eq!((s.black, s.white), ([0; 4], 3968));
        // 10-bit samples in the top bits of 16-bit little-endian words.
        let bytes = [0xc0u8, 0xff].repeat(12_000_000);
        let info = crate::open(&bytes).unwrap();
        let s = crate::decode(&info, &bytes, &AtomicBool::new(false)).unwrap();
        assert!(s.data.iter().all(|&v| v == 1023));
        assert_eq!(s.white, 992);
        assert_eq!(
            crate::decode(&info, &bytes, &AtomicBool::new(true)).unwrap_err(),
            crate::Error::Cancelled
        );
    }

    #[test]
    fn eight_bit_codes_are_taken_through_a_curve() {
        // A Kodak C603 "CCDI" file: no header, its curve the row's (code 3
        // is 198, the top 65535).
        let bytes = vec![3u8; 6_163_328];
        let info = crate::open(&bytes).unwrap();
        assert_eq!((info.make.as_str(), info.model.as_str()), ("Kodak", "C603"));
        let s = crate::decode(&info, &bytes, &AtomicBool::new(false)).unwrap();
        assert!(s.data.iter().all(|&v| v == 198));
        assert_eq!((s.black, s.white), ([0; 4], 65535));
        // A "CFAI" file: a 3160-byte header whose curve at byte 168 is the
        // code times 4, top 1020.
        let mut bytes = vec![0u8; 3160];
        for c in 0..256u16 {
            let at = 168 + 2 * usize::from(c);
            bytes[at..at + 2].copy_from_slice(&(c * 4).to_le_bytes());
        }
        bytes.extend((0..2864 * 2152).map(|i| (i % 256) as u8));
        let info = crate::open(&bytes).unwrap();
        let s = crate::decode(&info, &bytes, &AtomicBool::new(false)).unwrap();
        assert_eq!(&s.data[..3], &[0, 4, 8]);
        assert_eq!(s.white, 1020);
        assert_eq!(
            crate::decode(&info, &bytes, &AtomicBool::new(true)).unwrap_err(),
            crate::Error::Cancelled
        );
    }

    #[test]
    fn files_of_a_known_container_are_never_known_by_size() {
        // A little-endian TIFF of one directory (an image width), one whose
        // directory does not read, and an ISO base media file, each padded
        // to a PowerShot S2 IS file's size.
        let mut tiff = b"II*\0\x08\0\0\0\x01\0\0\x01\x03\0\x01\0\0\0\x10\0\0\0\0\0\0\0".to_vec();
        tiff.resize(6_573_120, 0x55);
        let mut broken = b"II*\0\xff\xff\xff\x7f".to_vec();
        broken.resize(6_573_120, 0x55);
        let mut bmff = b"\0\0\0\x18ftypheic".to_vec();
        bmff.resize(6_573_120, 0x55);
        assert!(matches!(
            crate::open(&tiff),
            Err(crate::Error::Unsupported(_))
        ));
        assert!(matches!(
            crate::open(&broken),
            Err(crate::Error::Corrupt(_))
        ));
        assert!(matches!(
            crate::open(&bmff),
            Err(crate::Error::Unsupported(_))
        ));
    }
}
