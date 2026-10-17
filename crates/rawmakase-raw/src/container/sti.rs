//! Sinar STI (CaptureShop's files of Sinarback digital backs): a TIFF whose
//! IFD0 is a thumbnail with Sinar's settings as text tags, and whose next
//! directory is the raw image, one tile of 16-bit words with its CFAPattern
//! and orientation. The white balance as shot is the Neutral line of the
//! last settings tag (0xC517, else 0xC516).
//!
//! Sources: TIFF 6, TIFF/EP (CFAPattern), and our own measurements on the
//! test dataset's file: which settings tag gives the white balance, and the
//! levels (black 0, white 16383, what RAWmakase has always read; Adobe DNG
//! Converter does not read the format).
use super::tiff::{Ifd, TiffFile};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use std::sync::atomic::AtomicBool;

/// Sinar's settings, text of `key value` lines.
const SETTINGS: [u16; 2] = [0xc517, 0xc516];
/// The most bytes of a settings tag read.
const MAX_SETTINGS: usize = 4096;
const WHITE: u32 = 16383;

/// Where an STI's sensor data is.
#[derive(Clone, Debug)]
pub(crate) struct StiPlan {
    pub data: (u64, u64),
    pub order: Order,
    pub width: u32,
    pub height: u32,
}

/// The raw image's directory: a 16-bit CFA image.
fn raw_ifd<'a>(t: &'a TiffFile) -> Option<&'a Ifd> {
    t.ifds
        .iter()
        .find(|ifd| t.int(ifd, 262) == Some(32803) && ifd.has(256))
}

/// Whether the TIFF `t` is a Sinar STI.
pub(crate) fn is_sti(t: &TiffFile) -> bool {
    let make = t.text_upto(&t.ifds[0], 271, 64).unwrap_or_default();
    make.trim().to_ascii_uppercase().starts_with("SINAR") && raw_ifd(t).is_some()
}

/// The Neutral line of Sinar's settings: red, green and blue levels.
fn neutral(t: &TiffFile) -> Option<[f32; 3]> {
    let ifd0 = &t.ifds[0];
    SETTINGS.iter().find_map(|&tag| {
        let text = t.text_upto(ifd0, tag, MAX_SETTINGS)?;
        let line = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("Neutral "))?;
        let v: Vec<f32> = line
            .split_whitespace()
            .take(3)
            .map_while(|s| s.parse().ok())
            .collect();
        match v[..] {
            [r, g, b] if [r, g, b].iter().all(|x| x.is_finite() && *x > 0.) => Some([r, g, b]),
            _ => None,
        }
    })
}

/// Reads the header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let raw = raw_ifd(&t).ok_or_else(|| Error::Unsupported("STI without raw data".into()))?;
    let width = t.int(raw, 256).ok_or_else(|| corrupt("STI width"))?;
    let height = t.int(raw, 257).ok_or_else(|| corrupt("STI height"))?;
    crate::io::samples(width, height, 1)?;
    let bits = t.int(raw, 258).unwrap_or(16);
    let compression = t.int(raw, 259).unwrap_or(1);
    if (compression, bits) != (1, 16) || t.int(raw, 277).unwrap_or(1) != 1 {
        return Err(Error::Unsupported(format!(
            "STI compression {compression} of {bits} bits"
        )));
    }
    let (offsets, counts) = match (t.ints_upto(raw, 324, 2), t.ints_upto(raw, 325, 2)) {
        (Some(o), Some(c)) => (o, c),
        _ => (
            t.ints_upto(raw, 273, 2).unwrap_or_default(),
            t.ints_upto(raw, 279, 2).unwrap_or_default(),
        ),
    };
    let ([offset], [count]) = (&offsets[..], &counts[..]) else {
        return Err(Error::Unsupported("STI of several tiles".into()));
    };
    let mut info = crate::info::base(Variant::StiUncompressed);
    info.make = t.text_upto(ifd0, 271, 64).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, 128).unwrap_or_default();
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    info.flip = crate::geometry::flip(t.int(raw, 274).or(t.int(ifd0, 274)).unwrap_or(1));
    let colors = t
        .raw_upto(raw, 33422, 4)
        .filter(|p| p.len() == 4 && p.iter().all(|&c| c <= 2))
        .unwrap_or(vec![0, 1, 1, 2]);
    info.cfa = Some(crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors,
    }));
    super::tiff_exif::read(&t, &mut info);
    if let Some([r, g, b]) = neutral(&t) {
        info.cam_mul = [r, g, b, g];
    }
    info.previews = t.previews();
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    info.plan = Plan::Sti(StiPlan {
        data: (t.base + u64::from(*offset), u64::from(*count)),
        order: if t.little() {
            Order::Little
        } else {
            Order::Big
        },
        width,
        height,
    });
    Ok(info)
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &StiPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let mut data = crate::io::zeroed(crate::io::samples(plan.width, plan.height, 1)?)?;
    let (at, size) = plan.data;
    // Truncated files keep what they have.
    let size = size.min((bytes.len() as u64).saturating_sub(at));
    let src = Bytes::new(bytes, plan.order).slice(at, size)?;
    crate::codec::packed::words(src, plan.order, 0, &mut data, cancel)?;
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area: [0, 0, info.width, info.height],
        black: [0; 4],
        black_pattern: (0, 0, Vec::new()),
        white: WHITE,
        linear_white: [0; 4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::test_tiff::{E, ascii, dir, long, offsets, short, tiff};

    const LE: Order = Order::Little;

    /// A little-endian STI of 2 × 2 samples `words`, turned by half, with
    /// Sinar's settings `settings`.
    fn sti(words: &[u8], settings: &str) -> Vec<u8> {
        let at = offsets(&[words, &[]]);
        let raw = dir(
            &[
                long(LE, 256, &[2]),
                long(LE, 257, &[2]),
                short(LE, 258, &[16]),
                short(LE, 259, &[1]),
                short(LE, 262, &[32803]),
                short(LE, 274, &[3]),
                long(LE, 324, &[at[0]]),
                long(LE, 325, &[words.len() as u32]),
                E(33422, 1, vec![1, 0, 2, 1]),
            ],
            at[1],
            LE,
        );
        let ifd0 = vec![
            ascii(271, "Sinar AG"),
            ascii(272, "Sinarback eVolution 75"),
            long(LE, 330, &[at[1]]),
            ascii(0xc517, settings),
        ];
        tiff(b"II*\0", LE, ifd0, vec![], &[words, &raw])
    }

    #[test]
    fn reads_sinar_s_raw_and_settings() {
        let words: Vec<u8> = [1u16, 2, 3, 4]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let file = sti(
            &words,
            "ColorEngine 2\r\nNeutral 1.5 1 1.25 0 0\r\nTint 1 1 1",
        );
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, Variant::StiUncompressed);
        assert_eq!(info.make, "Sinar");
        assert_eq!(info.flip, 3);
        assert_eq!(info.cam_mul, [1.5, 1., 1.25, 1.]);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.data, [1, 2, 3, 4]);
        assert_eq!(s.white, 16383);
        // No Neutral line: no white balance.
        let info = crate::open(&sti(&words, "Tint 1 1 1")).unwrap();
        assert_eq!(info.cam_mul, [0.; 4]);
    }

    #[test]
    fn bad_files_do_not_panic() {
        let file = sti(&[0u8; 8], "Neutral 1 1 1");
        let info = crate::open(&file).unwrap();
        assert_eq!(
            crate::decode(&info, &file, &AtomicBool::new(true)).unwrap_err(),
            Error::Cancelled
        );
        for end in 0..file.len() {
            if let Ok(info) = crate::open(&file[..end]) {
                let _ = crate::decode(&info, &file[..end], &AtomicBool::new(false));
            }
        }
        let mut noisy = file.clone();
        for (i, b) in noisy.iter_mut().enumerate().skip(8) {
            *b ^= (i * 7919 % 251) as u8;
        }
        if let Ok(info) = crate::open(&noisy) {
            let _ = crate::decode(&info, &noisy, &AtomicBool::new(false));
        }
    }
}
