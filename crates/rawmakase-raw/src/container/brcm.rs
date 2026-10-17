//! Raspberry Pi camera files (raspistill's and picamera's `--raw`): a JPEG
//! whose EXIF maker note is the ISP's settings as text, followed by
//! Broadcom's raw block: "BRCM", a 32768-byte header (the sensor's name, the
//! row size, the image size, the bits) and rows of 10-bit samples packed
//! four in five bytes (the four samples' high bits, then their low bits two
//! at a time from the least significant), rows padded.
//!
//! Sources: our own measurements on the test dataset's files (the header's
//! fields, the packing, the white balance as the maker note's gain_r and
//! gain_b); the names, filter and levels RAWmakase has always read are a
//! `compat-observed` row per sensor, and a sensor without one is
//! `Unsupported`.
use super::tiff::TiffFile;
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use std::sync::atomic::{AtomicBool, Ordering};

/// The raw block's header size, after which the rows start.
const HEADER: u64 = 32768;
/// The most of the maker note's text read.
const MAX_NOTE: usize = 2048;

/// Where a Raspberry Pi file's rows are, and its levels.
#[derive(Clone, Debug)]
pub(crate) struct BrcmPlan {
    pub data: u64,
    pub stride: u32,
    pub width: u32,
    pub height: u32,
    pub black: u32,
    pub white: u32,
}

/// How far after the JPEG's end the raw block may start (the dataset's
/// files: one byte).
const MAX_GAP: usize = 16;

/// Where a JPEG ends: past its end of image marker, found by walking its
/// segments (a scan's data up to the next marker other than a stuffed byte
/// or a restart). `None` for a JPEG whose segments do not read.
pub(crate) fn jpeg_end(bytes: &[u8]) -> Option<usize> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut at = 2usize;
    loop {
        if *bytes.get(at)? != 0xff {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        match marker {
            // Fill bytes.
            0xff => at += 1,
            0xd9 => return Some(at + 2),
            0x01 | 0xd0..=0xd7 => at += 2,
            _ => {
                let len = usize::from(u16::from_be_bytes([
                    *bytes.get(at + 2)?,
                    *bytes.get(at + 3)?,
                ]));
                if len < 2 {
                    return None;
                }
                at = at.checked_add(2 + len)?;
                if marker == 0xda {
                    // The scan's data: to the next 0xFF not followed by 0 or
                    // a restart marker.
                    loop {
                        let next = bytes.get(at..)?.iter().position(|&b| b == 0xff)?;
                        at += next;
                        match *bytes.get(at + 1)? {
                            0x00 | 0xd0..=0xd7 => at += 2,
                            _ => break,
                        }
                    }
                }
            }
        }
    }
}

/// The raw block's header fields: where it starts, the row size, the image
/// size and the bits.
struct Header {
    at: u64,
    stride: u32,
    width: u32,
    height: u32,
    bits: u8,
}

/// The raw block: "BRCM" just after the JPEG's end, its header's sizes
/// non-zero, a row at least a byte per sample, and its rows in the file.
fn block(bytes: &[u8]) -> Option<Header> {
    let end = jpeg_end(bytes)?;
    let window = bytes.get(end..bytes.len().min(end + MAX_GAP + 4))?;
    let at = (end + window.windows(4).position(|w| w == b"BRCM")?) as u64;
    let b = crate::io::Bytes::new(bytes, crate::io::Order::Little);
    let h = Header {
        at,
        stride: u32::from(b.u16(at + 0xa0).ok()?),
        width: u32::from(b.u16(at + 0xd0).ok()?),
        height: u32::from(b.u16(at + 0xd2).ok()?),
        bits: *b.slice(at + 0xf6, 1).ok()?.first()?,
    };
    let rows = u64::from(h.stride) * u64::from(h.height);
    (h.width > 0 && h.height > 0 && h.stride >= h.width && at + HEADER + rows <= bytes.len() as u64)
        .then_some(h)
}

/// Whether `bytes` is a JPEG followed by Broadcom's raw block.
pub(crate) fn is_brcm(bytes: &[u8]) -> bool {
    block(bytes).is_some()
}

/// The `key=` value of the ISP's settings text.
fn setting(text: &str, key: &str) -> Option<f32> {
    text.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
        .and_then(|v| v.parse().ok())
        .filter(|v: &f32| v.is_finite() && *v > 0.)
}

/// Reads the header facts.
pub(crate) fn open(bytes: &[u8]) -> Result<RawInfo> {
    let Header {
        at,
        stride,
        width,
        height,
        bits,
    } = block(bytes).ok_or(Error::NotRaw)?;
    let b = crate::io::Bytes::new(bytes, crate::io::Order::Little);
    let name = b.slice(at + 16, 32)?;
    let name = &name[..name.iter().position(|&c| c == 0).unwrap_or(32)];
    let sensor = String::from_utf8_lossy(name)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    crate::io::samples(width, height, 1)?;
    if bits != 10 {
        return Err(Error::Unsupported(format!("Broadcom raw of {bits} bits")));
    }
    if u64::from(stride) < (u64::from(width) * 10).div_ceil(8) || width % 4 != 0 {
        return Err(corrupt("Broadcom row size"));
    }
    let mut info = crate::info::base(Variant::BrcmPacked);
    info.make = "Broadcom".into();
    info.model = format!("RPi {sensor}");
    let row = crate::facts::camera(&info.make, &info.model, false, [width, height])
        .find(|c| c.cfa.is_some())
        .ok_or_else(|| Error::Unsupported(format!("Raspberry Pi sensor {sensor}")))?;
    let cfa = row.cfa.map(|c| c.to_vec()).unwrap_or_default();
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    info.cfa = Some(crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors: cfa,
    }));
    // The JPEG's EXIF: an APP1 segment first.
    if bytes.get(2..4) == Some(&[0xff, 0xe1])
        && bytes.get(6..12) == Some(b"Exif\0\0")
        && let Some(t) = TiffFile::parse(bytes, 12)
    {
        super::tiff_exif::read(&t, &mut info);
        let note = t
            .exif
            .as_ref()
            .and_then(|e| t.raw_upto(e, 0x927c, MAX_NOTE));
        if let Some(note) = note {
            let text = String::from_utf8_lossy(&note);
            if let (Some(r), Some(bl)) = (setting(&text, "gain_r"), setting(&text, "gain_b")) {
                info.cam_mul = [r, 1., bl, 1.];
            }
        }
    }
    // The JPEG: up to its end of image marker before the raw block.
    info.previews.push(crate::preview::Candidate {
        offset: 0,
        len: jpeg_end(bytes).map_or(at, |e| e as u64),
        pixels: 0,
    });
    let black = row.black.unwrap_or(0);
    let white = row.white.unwrap_or(1023);
    info.plan = Plan::Brcm(BrcmPlan {
        data: at + HEADER,
        stride,
        width,
        height,
        black,
        white,
    });
    Ok(info)
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &BrcmPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let width = plan.width as usize;
    let mut data = crate::io::zeroed(crate::io::samples(plan.width, plan.height, 1)?)?;
    let start = usize::try_from(plan.data).map_err(|_| corrupt("Broadcom data"))?;
    let rows = bytes.get(start..).unwrap_or(&[]);
    for (y, out) in data.chunks_exact_mut(width).enumerate() {
        if y % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        // Truncated files keep what they have.
        let Some(row) = rows.get(y * plan.stride as usize..) else {
            break;
        };
        for (group, o) in row
            .as_chunks::<5>()
            .0
            .iter()
            .zip(out.as_chunks_mut::<4>().0)
        {
            for (k, v) in o.iter_mut().enumerate() {
                *v = u16::from(group[k]) << 2 | u16::from(group[4] >> (2 * k) & 3);
            }
        }
    }
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area: [0, 0, info.width, info.height],
        black: [plan.black; 4],
        black_pattern: (0, 0, Vec::new()),
        white: plan.white,
        linear_white: [0; 4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty JPEG, then a raw block of `sensor` of 8 × 2.
    fn file(sensor: &str, bits: u8) -> Vec<u8> {
        with_jpeg(&[0xff, 0xd8, 0xff, 0xd9], sensor, bits)
    }

    /// `jpeg`, a byte, then a raw block of `sensor` of 8 × 2.
    fn with_jpeg(jpeg: &[u8], sensor: &str, bits: u8) -> Vec<u8> {
        let mut out = jpeg.to_vec();
        out.push(b'@');
        let at = out.len();
        out.extend(b"BRCMo\0\0\0");
        out.resize(at + 16, 0);
        out.extend(sensor.as_bytes());
        out.resize(at + 0xa0, 0);
        out.extend(16u16.to_le_bytes());
        out.resize(at + 0xd0, 0);
        out.extend(8u16.to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.resize(at + 0xf6, 0);
        out.push(bits);
        out.resize(at + HEADER as usize, 0);
        // Each row: samples 4, 5, 6, 7 (high bits 1, low bits 0, 1, 2, 3),
        // twice, then padding.
        for _ in 0..2 {
            for _ in 0..2 {
                out.extend([1, 1, 1, 1, 0b11_10_01_00]);
            }
            out.extend([0xaa; 6]);
        }
        out
    }

    #[test]
    fn reads_the_raw_block_after_the_jpeg() {
        let f = file("imx219 version 1.0", 10);
        let info = crate::open(&f).unwrap();
        assert_eq!(info.variant, Variant::BrcmPacked);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Broadcom", "RPi IMX219")
        );
        let s = crate::decode(&info, &f, &AtomicBool::new(false)).unwrap();
        assert_eq!(&s.data[..8], &[4, 5, 6, 7, 4, 5, 6, 7]);
        assert_eq!((s.black, s.white), ([66; 4], 1023));
        assert_eq!(&crate::preview(&info, &f).unwrap()[..], &f[..4]);
        assert_eq!(
            crate::decode(&info, &f, &AtomicBool::new(true)).unwrap_err(),
            Error::Cancelled
        );
        assert!(matches!(
            crate::open(&file("ov5647 version 1.0", 10)),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            crate::open(&file("imx219", 12)),
            Err(Error::Unsupported(_))
        ));
        for end in 0..f.len().min(40000) {
            if let Ok(info) = crate::open(&f[..end]) {
                let _ = crate::decode(&info, &f[..end], &AtomicBool::new(false));
            }
        }
        assert_eq!(
            setting("ag=1 gain_r=1.707 gain_b=1.6", "gain_r"),
            Some(1.707)
        );
        assert_eq!(setting("gain_r=x", "gain_r"), None);
    }

    /// A 1 × 1 baseline JPEG's segments with an APP2 of `app2`, and a scan
    /// of one byte.
    fn jpeg_with_app2(app2: &[u8]) -> Vec<u8> {
        let mut j = vec![0xff, 0xd8, 0xff, 0xe2];
        j.extend(((app2.len() + 2) as u16).to_be_bytes());
        j.extend(app2);
        j.extend([0xff, 0xc0, 0, 11, 8, 0, 1, 0, 1, 1, 1, 0x11, 0]);
        j.extend([0xff, 0xda, 0, 8, 1, 1, 0, 0, 0x3f, 0]);
        // Scan data with a stuffed 0xFF and a restart marker.
        j.extend([0x12, 0xff, 0x00, 0xff, 0xd0, 0x34, 0xff, 0xd9]);
        j
    }

    #[test]
    fn only_a_block_after_the_jpegs_end_is_broadcoms() {
        // The 32768-byte header's fields inside a 256-byte APP2 do not make
        // the JPEG a Raspberry Pi file.
        let mut app2 = b"BRCMo\0\0\0".to_vec();
        app2.resize(256, 0);
        app2[0xd0..0xd4].copy_from_slice(&[8, 0, 2, 0]);
        app2[0xa0] = 16;
        app2[0xf6] = 10;
        let jpeg = jpeg_with_app2(&app2);
        assert!(!is_brcm(&jpeg));
        assert_eq!(crate::open(&jpeg).unwrap_err(), Error::NotRaw);
        // Nor does it hide a real block after the JPEG's end.
        let f = with_jpeg(&jpeg, "imx219 version 1.0", 10);
        let info = crate::open(&f).unwrap();
        assert_eq!(info.variant, Variant::BrcmPacked);
        let s = crate::decode(&info, &f, &AtomicBool::new(false)).unwrap();
        assert_eq!(&s.data[..4], &[4, 5, 6, 7]);
        assert_eq!(&crate::preview(&info, &f).unwrap()[..], &jpeg[..]);
        // A block whose rows are not in the file is none.
        assert!(!is_brcm(&f[..f.len() - 1]));
    }

    #[test]
    fn a_jpeg_of_a_headerless_raws_size_is_not_raw() {
        // 24,000,000 bytes: the size of a Samsung WB550 headerless RAW.
        let mut f = jpeg_with_app2(b"x");
        f.resize(24_000_000, 0);
        assert_eq!(crate::open(&f).unwrap_err(), Error::NotRaw);
    }
}
