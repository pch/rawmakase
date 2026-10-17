//! Kodak DCS TIFFs (the DCS520C and DCS560C, Canon's EOS D2000C and D6000C
//! of the same backs): a thumbnail IFD0 whose SubIFD holds the raw image,
//! one lossless JPEG strip (compression 7) whose MCU rows are the image's
//! rows, 12-bit codes on the strip's GrayResponseCurve (0x0123); and the
//! KodakIFD (IFD0 tag 0x8290, offsets from the TIFF header) with the black
//! level (BlackLevelBottom, 0x03F0) and the neutral levels as shot
//! (0x0848, red, green, blue). Kodak's DCR files (the DCS760C and DCS Pro
//! bodies) are the same TIFFs, the strip in Kodak's own coding (compression
//! 65000, `codec::kodak`) and the KodakIFD's curve up to 4096 entries (1024,
//! of 10-bit codes, on the DCS Pro 14N, 14nx and SLR/c).
//!
//! Sources: TIFF 6 (GrayResponseCurve, SubIFDs), TIFF/EP (CFAPattern), ITU
//! T.81, ExifTool's Kodak tag documentation (KodakIFD, BlackLevelBottom), and
//! our own measurements on the test dataset's files (which neutral levels
//! are the white balance, the codes taken through the curve).
use super::tiff::{Ifd, TiffFile};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use std::sync::atomic::{AtomicBool, Ordering};

const KODAK_IFD: u16 = 0x8290;
/// BlackLevelTop and BlackLevelBottom.
const BLACK_LEVELS: [u16; 2] = [0x03ef, 0x03f0];
/// WhiteBalance (the setting as shot), and the neutral levels of each
/// setting from WB_RGBLevelsDaylight on (red, green, blue).
const WHITE_BALANCE: u16 = 0x03fc;
const DAYLIGHT: u16 = 0x0848;
const GRAY_RESPONSE_CURVE: u16 = 0x0123;
/// ImageExpansionTable: the KodakIFD's curve, the one the codes are on.
const EXPANSION_TABLE: u16 = 0x090d;
const BORDERS: [u16; 4] = [0x03eb, 0x03ec, 0x03ed, 0x03ee];
/// The curve's entries: one per 12-bit code.
const CURVE_LEN: usize = 4096;

/// Where a DCS TIFF's strip is, its curve and levels.
#[derive(Clone, Debug)]
pub(crate) struct DcsPlan {
    pub strip: (u64, u64),
    pub width: u32,
    pub height: u32,
    /// The value of each 12-bit code, when the strip has a curve.
    pub curve: Option<Vec<u16>>,
    pub black: u32,
    pub white: u32,
}

/// The raw image's directory: a 12-bit lossless JPEG CFA image.
fn raw_ifd<'a>(t: &'a TiffFile) -> Option<&'a Ifd> {
    t.ifds.iter().find(|ifd| {
        t.int(ifd, 262) == Some(32803)
            && matches!(t.int(ifd, 259), Some(7 | 65000))
            && t.int(ifd, 258) == Some(12)
    })
}

/// Whether the TIFF `t` is a Kodak DCS TIFF: Kodak's IFD, and the raw image.
pub(crate) fn is_dcs(t: &TiffFile) -> bool {
    t.ifds[0].has(KODAK_IFD) && raw_ifd(t).is_some()
}

/// Reads the header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let raw = raw_ifd(&t).ok_or_else(|| Error::Unsupported("DCS without raw data".into()))?;
    let width = t.int(raw, 256).ok_or_else(|| corrupt("DCS width"))?;
    let height = t.int(raw, 257).ok_or_else(|| corrupt("DCS height"))?;
    crate::io::samples(width, height, 1)?;
    let offsets = t.ints_upto(raw, 273, 2).unwrap_or_default();
    let counts = t.ints_upto(raw, 279, 2).unwrap_or_default();
    let ([offset], [count]) = (&offsets[..], &counts[..]) else {
        return Err(Error::Unsupported("DCS of several strips".into()));
    };
    let (offset, count) = (*offset, *count);
    let kodak = t
        .int(ifd0, KODAK_IFD)
        .and_then(|at| t.ifd_at(u64::from(at)));
    // The curve: up to 4096 non-decreasing values (the DCS Pro 14N's has
    // 1024, its codes 10-bit), its shape checked before it is read; the
    // KodakIFD's, else the strip's GrayResponseCurve.
    let table = |ifd: &Ifd, tag: u16| -> Option<Vec<u16>> {
        t.shape(ifd, tag)
            .filter(|&(_, n)| (2..=CURVE_LEN).contains(&(n as usize)))
            .and_then(|_| t.ints(ifd, tag, CURVE_LEN))
            .filter(|c| c.windows(2).all(|p| p[0] <= p[1]) && c.iter().all(|&v| v < 1 << 16))
            .map(|c| c.into_iter().map(|v| v as u16).collect())
    };
    let curve = kodak
        .as_ref()
        .and_then(|k| table(k, EXPANSION_TABLE))
        .or_else(|| table(raw, GRAY_RESPONSE_CURVE));
    // Black: the mean of the top and bottom levels, rounding up, as Adobe
    // DNG Converter takes it (our measurement).
    let black = match kodak
        .as_ref()
        .map(|k| BLACK_LEVELS.map(|tag| t.int(k, tag)))
    {
        Some([Some(top), Some(bottom)]) => top.saturating_add(bottom).div_ceil(2),
        Some([None, Some(level)] | [Some(level), None]) => level,
        _ => 0,
    };
    if black >= 4095 {
        return Err(corrupt("DCS black level"));
    }
    let variant = match t.int(raw, 259) {
        Some(65000) => Variant::DcsKodak,
        _ => Variant::DcsLosslessJpeg,
    };
    let mut info = crate::info::base(variant);
    info.make = t.text_upto(ifd0, 271, 64).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, 64).unwrap_or_default();
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
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
    // The exposure index where no ISO speed is given.
    if info.iso == 0.
        && let Some(v) = t.real(ifd0, 37397).filter(|v| v.is_finite() && *v > 0.)
    {
        info.iso = v as f32;
    }
    // The sensor's borders: the camera's image inside the sensor's.
    if let Some(k) = &kodak
        && let [Some(left), Some(top), Some(w), Some(h)] = BORDERS.map(|tag| t.int(k, tag))
        && left.checked_add(w).is_some_and(|r| r <= width)
        && top.checked_add(h).is_some_and(|b| b <= height)
        && w > 0
        && h > 0
    {
        info.frame = [left, top, w, h];
    }
    if let Some(k) = &kodak {
        let levels = |tag: u16| {
            t.shape(k, tag)
                .filter(|&(_, n)| n == 3)
                .and_then(|_| t.reals(k, tag, 3))
                .and_then(|v| <[f64; 3]>::try_from(v).ok())
                .filter(|l| l.iter().all(|v| v.is_finite() && *v > 0.))
        };
        // The as-shot setting's levels: green over each colour's.
        let setting = t.int(k, WHITE_BALANCE).filter(|&s| s <= 5).unwrap_or(0);
        if let Some([r, g, b]) = levels(DAYLIGHT + setting as u16) {
            info.cam_mul = [(g / r) as f32, 1., (g / b) as f32, 1.];
        }
        // The daylight preset: the levels green over each colour's, scaled
        // by green, whole.
        if let Some([r, g, b]) = levels(DAYLIGHT) {
            info.presets.daylight = [
                (g * g / r).floor() as f32,
                g as f32,
                (g * g / b).floor() as f32,
            ];
        }
    }
    info.previews = t.previews();
    // DCR files' previews are RGB or YCbCr in Kodak's coding: not JPEG.
    info.uncompressed_preview = t.has_uncompressed_thumbnail()
        || t.ifds
            .iter()
            .any(|ifd| t.int(ifd, 259) == Some(65000) && matches!(t.int(ifd, 262), Some(2 | 6)));
    info.plan = Plan::Dcs(DcsPlan {
        strip: (t.base + u64::from(offset), u64::from(count)),
        width,
        height,
        curve,
        black,
        // The camera table's white (Adobe DNG Converter's), else the codes'
        // 12-bit range.
        white: crate::facts::camera(&info.make, &info.model, false, [width, height])
            .find_map(|c| c.white)
            .unwrap_or(4095),
    });
    Ok(info)
}

/// The older DCS TIFFs' raw image (the DCS460, the EOS DCS 1 and 3): the
/// largest directory of 8-bit single samples, uncompressed, the codes on
/// IFD0's 256-entry GrayResponseCurve.
fn eight_bit_ifd<'a>(t: &'a TiffFile) -> Option<&'a Ifd> {
    t.ifds
        .iter()
        .filter(|ifd| {
            t.int(ifd, 258) == Some(8)
                && t.int(ifd, 277).unwrap_or(1) == 1
                && t.int(ifd, 259).unwrap_or(1) == 1
        })
        .max_by_key(|ifd| {
            u64::from(t.int(ifd, 256).unwrap_or(0)) * u64::from(t.int(ifd, 257).unwrap_or(0))
        })
}

/// Whether the TIFF `t` is an older Kodak DCS TIFF of 8-bit codes on a
/// curve: Kodak's make, IFD0's curve and an 8-bit image of a sensor's size.
pub(crate) fn is_dcs_eight_bit(t: &TiffFile) -> bool {
    let ifd0 = &t.ifds[0];
    t.text_upto(ifd0, 271, 16)
        .is_some_and(|m| m.trim().eq_ignore_ascii_case("KODAK"))
        && t.shape(ifd0, GRAY_RESPONSE_CURVE)
            .is_some_and(|(_, n)| n == 256)
        && eight_bit_ifd(t).is_some_and(|ifd| t.int(ifd, 256).unwrap_or(0) >= 1024)
}

/// Reads the header facts of an older DCS TIFF. `t` is the parsed TIFF.
pub(crate) fn open_eight_bit(t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let raw = eight_bit_ifd(&t).ok_or_else(|| corrupt("DCS raw image"))?;
    let width = t.int(raw, 256).ok_or_else(|| corrupt("DCS width"))?;
    let height = t.int(raw, 257).ok_or_else(|| corrupt("DCS height"))?;
    crate::io::samples(width, height, 1)?;
    let offsets = t.ints_upto(raw, 273, 2).unwrap_or_default();
    let ([offset], Some(size)) = (
        &offsets[..],
        u64::from(width).checked_mul(u64::from(height)),
    ) else {
        return Err(Error::Unsupported("DCS of several strips".into()));
    };
    let curve: Vec<u16> = t
        .ints(ifd0, GRAY_RESPONSE_CURVE, 256)
        .filter(|c| c.len() == 256 && c.iter().all(|&v| v < 1 << 16))
        .ok_or_else(|| corrupt("DCS curve"))?
        .into_iter()
        .map(|v| v as u16)
        .collect();
    let mut info = crate::info::base(Variant::DcsEightBit);
    info.make = t.text_upto(ifd0, 271, 64).unwrap_or_default();
    // The model's first word: "DCS460D         FILE VERSION 3".
    info.model = t
        .text_upto(ifd0, 272, 64)
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    crate::names::apply(&mut info);
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    let colors = crate::facts::camera(&info.make, &info.model, false, [width, height])
        .find_map(|c| c.cfa)
        .map_or(vec![0, 1, 1, 2], |c| c.to_vec());
    info.cfa = Some(crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors,
    }));
    info.previews = t.previews();
    // Its thumbnails (grey and RGB) are uncompressed.
    info.uncompressed_preview = true;
    info.plan = Plan::Dcs(DcsPlan {
        strip: (t.base + u64::from(*offset), size),
        width,
        height,
        // The curve's top is the white.
        white: u32::from(curve.iter().copied().max().unwrap_or(255)),
        curve: Some(curve),
        black: 0,
    });
    Ok(info)
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &DcsPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let count = crate::io::samples(plan.width, plan.height, 1)?;
    let (at, size) = plan.strip;
    // Truncated files keep what they have.
    let size = size.min((bytes.len() as u64).saturating_sub(at));
    let src = Bytes::new(bytes, Order::Big).slice(at, size)?;
    let data = match info.variant {
        Variant::DcsLosslessJpeg => {
            let h = crate::codec::ljpeg::header(src)?;
            let (mcus, rows) = h.mcus();
            if mcus.checked_mul(h.samples_per_mcu()) != Some(plan.width as usize)
                || rows != plan.height as usize
            {
                return Err(Error::Unsupported(
                    "DCS lossless JPEG of another size than the image".into(),
                ));
            }
            let mut data = Vec::new();
            data.try_reserve_exact(count)
                .map_err(|_| Error::TooLarge("image allocation"))?;
            crate::codec::ljpeg::decode_into(&h, src, cancel, &mut data)?;
            data.resize(count, 0);
            data
        }
        Variant::DcsKodak => {
            let mut data = crate::io::zeroed(count)?;
            let order = if bytes.starts_with(b"II") {
                Order::Little
            } else {
                Order::Big
            };
            crate::codec::kodak::decode(src, order, plan.width as usize, &mut data, cancel)?;
            data
        }
        Variant::DcsEightBit => {
            let mut data = crate::io::zeroed(count)?;
            let p = crate::codec::packed::Packing::msb(8);
            let width = plan.width as usize;
            crate::codec::packed::rows(src, p, width, width, &mut data, cancel)?;
            data
        }
        v => {
            return Err(Error::Unsupported(format!(
                "{} is not a DCS variant",
                v.name()
            )));
        }
    };
    let mut data = data;
    if let Some(curve) = &plan.curve {
        for (i, v) in data.iter_mut().enumerate() {
            if i % (1 << 20) == 0 && cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            *v = curve[usize::from(*v).min(curve.len().saturating_sub(1))];
        }
    }
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area: [info.margins[0], info.margins[1], info.width, info.height],
        black: [plan.black; 4],
        black_pattern: (0, 0, Vec::new()),
        white: plan.white,
        linear_white: [0; 4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::test_tiff::{E, ascii, dir, long, offsets, short, shorts, tiff};

    const BE: Order = Order::Big;

    fn rationals(v: &[u32]) -> Vec<u8> {
        v.iter()
            .flat_map(|x| [x.to_be_bytes(), 1u32.to_be_bytes()].concat())
            .collect()
    }

    /// A big-endian DCS TIFF of 4 × 2 samples coded as `jpeg`, the curve
    /// doubling codes, and a KodakIFD with black levels 10 and 13, the
    /// Daylight levels and WhiteBalance 0.
    fn dcs(jpeg: &[u8]) -> Vec<u8> {
        dcs_coded(jpeg, 7, 4096)
    }

    /// `dcs`, the strip of `compression` and the curve of `curve_len`
    /// entries.
    fn dcs_coded(jpeg: &[u8], compression: u16, curve_len: u32) -> Vec<u8> {
        let curve: Vec<u16> = (0..curve_len).map(|v| (2 * v).min(4095) as u16).collect();
        let curve = shorts(BE, &curve);
        let kodak_at = offsets(&[jpeg, &[]])[1];
        let kodak = dir(
            &[
                short(BE, 0x03ef, &[10]),
                short(BE, 0x03f0, &[13]),
                short(BE, 0x03fc, &[0]),
                E(0x0848, 10, rationals(&[2000, 1000, 500])),
                E(0x090d, 3, curve),
            ],
            kodak_at,
            BE,
        );
        let raw_at = offsets(&[jpeg, &kodak, &[]])[2];
        let raw = dir(
            &[
                long(BE, 256, &[4]),
                long(BE, 257, &[2]),
                short(BE, 258, &[12]),
                short(BE, 259, &[compression]),
                short(BE, 262, &[32803]),
                long(BE, 273, &[8]),
                long(BE, 279, &[jpeg.len() as u32]),
                E(33422, 1, vec![1, 0, 2, 1]),
            ],
            raw_at,
            BE,
        );
        let ifd0 = vec![
            ascii(271, "Kodak"),
            ascii(272, "DCS520C"),
            long(BE, 330, &[raw_at]),
            long(BE, KODAK_IFD, &[kodak_at]),
        ];
        tiff(b"MM\0*", BE, ifd0, vec![], &[jpeg, &kodak, &raw])
    }

    #[test]
    fn reads_the_strip_on_the_kodak_curve() {
        let samples: Vec<u16> = (0..8).map(|v| 100 + v * 10).collect();
        let jpeg = crate::codec::ljpeg::tests::encode(&samples, 4, 12, 1);
        let file = dcs(&jpeg);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, Variant::DcsLosslessJpeg);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.data, samples.iter().map(|v| v * 2).collect::<Vec<_>>());
        // Black: the mean of the top and bottom levels, rounding up.
        assert_eq!((s.black, s.white), ([12; 4], 4095));
        // WhiteBalance 0: the Daylight levels, green over each.
        assert_eq!(info.cam_mul, [0.5, 1., 2., 1.]);
        assert_eq!(info.presets.daylight, [500., 1000., 2000.]);
        assert_eq!(
            crate::decode(&info, &file, &AtomicBool::new(true)).unwrap_err(),
            Error::Cancelled
        );
    }

    #[test]
    fn dcr_files_are_kodak_coded_on_a_short_curve() {
        // Each row one block: lengths 4, 4, 4, 4, then 16 bits (prefetched,
        // big-endian): 10, 12, 8, 9 least significant first, each with its
        // top bit set (positive). Even columns 10, 18; odd 12, 21.
        let row = [0x44, 0x44, 0x98, 0xca];
        let file = dcs_coded(&[row, row].concat(), 65000, 1024);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, Variant::DcsKodak);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        // The codes through the 1024-entry curve (doubling).
        assert_eq!(s.data, [20, 24, 36, 42, 20, 24, 36, 42]);
        assert_eq!(
            crate::decode(&info, &file, &AtomicBool::new(true)).unwrap_err(),
            Error::Cancelled
        );
        for end in 0..file.len() {
            if let Ok(info) = crate::open(&file[..end]) {
                let _ = crate::decode(&info, &file[..end], &AtomicBool::new(false));
            }
        }
    }

    /// An older big-endian DCS TIFF: IFD0's curve (code times 3) and a
    /// SubIFD of 1024 × 2 8-bit codes `codes`.
    fn eight_bit(codes: &[u8]) -> Vec<u8> {
        let curve: Vec<u16> = (0..256u16).map(|v| v * 3).collect();
        let raw_at = offsets(&[codes, &[]])[1];
        let raw = dir(
            &[
                long(BE, 256, &[1024]),
                long(BE, 257, &[2]),
                short(BE, 258, &[8]),
                short(BE, 262, &[1]),
                long(BE, 273, &[8]),
                long(BE, 279, &[codes.len() as u32]),
            ],
            raw_at,
            BE,
        );
        let ifd0 = vec![
            ascii(271, "KODAK           "),
            ascii(272, "DCS460D         FILE VERSION 3  "),
            E(GRAY_RESPONSE_CURVE, 3, shorts(BE, &curve)),
            long(BE, 330, &[raw_at]),
        ];
        tiff(b"MM\0*", BE, ifd0, vec![], &[codes, &raw])
    }

    #[test]
    fn older_dcs_tiffs_are_8_bit_codes_on_a_curve() {
        let codes: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
        let file = eight_bit(&codes);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, Variant::DcsEightBit);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Kodak", "DCS460D")
        );
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.data[..4], [0, 3, 6, 9]);
        assert_eq!(s.white, 765);
        for end in 0..file.len() {
            if let Ok(info) = crate::open(&file[..end]) {
                let _ = crate::decode(&info, &file[..end], &AtomicBool::new(false));
            }
        }
    }

    #[test]
    fn bad_files_do_not_panic() {
        let samples: Vec<u16> = (0..8).collect();
        let file = dcs(&crate::codec::ljpeg::tests::encode(&samples, 4, 12, 1));
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
