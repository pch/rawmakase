//! RAW files with no header at all (CHDK's from Canon PowerShots): only the
//! file's size says what it is, so the camera table lists each size with its
//! camera, image size, bit depth, visible area and colour filter. The samples
//! are packed, most significant bit first, in 16-bit little-endian words.
//!
//! Sources: our own measurements on the test dataset's files.
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
    // Packed most significant bit first in 16-bit little-endian words, one
    // run over the rows (each row a whole number of words).
    let packing = packed::Packing {
        bits: plan.row.bits,
        word: 2,
        pad: false,
        swap: false,
    };
    if !(u64::from(width) * u64::from(plan.row.bits)).is_multiple_of(16) {
        return Err(crate::error::Error::Unsupported(
            "headerless rows of part of a word".into(),
        ));
    }
    let stride = packing.row_bytes(width as usize);
    packed::rows(bytes, packing, width as usize, stride, &mut data, cancel)?;
    let area = [info.margins[0], info.margins[1], info.width, info.height];
    let cfa = info
        .cfa
        .as_ref()
        .ok_or_else(|| crate::error::corrupt("no colour filter"))?;
    let black =
        crate::levels::masked_black(&data, width as usize, area, cfa, plan.row.black_columns, 0)
            .unwrap_or([0; 4]);
    Ok(Sensor {
        width,
        height,
        channels: 1,
        data,
        area,
        black,
        black_pattern: (0, 0, Vec::new()),
        white: (1 << plan.row.bits) - 1,
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
