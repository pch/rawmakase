//! Kodak KDC of the EasyShare and P-series compacts: a TIFF with a JPEG
//! preview and two private directories: the KDC IFD (IFD0 tag 0xFE00) with
//! the raw image's size (0xFA13, 0xFA14), its filter as text (0xFA15) and
//! the as-shot levels (0xFA12, red, green and blue in 65536ths), and a
//! SubIFD whose tag 0xFD04 holds, among numbers, where the sensor data is:
//! 12-bit samples packed most significant bit first.
//!
//! Sources: TIFF 6, ExifTool's Kodak tag documentation (the KDC IFD, its
//! white balance levels and presets), and our own measurements: the data
//! starts 64 bytes after the first of the 16-row strips whose offsets tag
//! 0xFD04 lists from its seventeenth value on, an odd height is read a row
//! taller, the as-shot levels are those of the WhiteBalance setting
//! (0xFA0D: 0 Auto, 0xFA25; 3 Daylight, 0xFA29; 6 Shade, 0xFA2A); the data, its packing and the levels decode exactly as LibRaw's on
//! every KDC of the test dataset, and the white balance is Adobe DNG
//! Converter's AsShotNeutral.
use super::tiff::{Ifd, TiffFile};
use crate::codec::packed::{self, Packing};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use std::sync::atomic::AtomicBool;

const KDC_IFD: u16 = 0xfe00;
const WB_LEVELS: u16 = 0xfa12;
const RAW_SIZE: [u16; 2] = [0xfa13, 0xfa14];
const FILTER: u16 = 0xfa15;
const DATA: u16 = 0xfd04;
/// The bytes before a strip's rows.
const STRIP_HEADER: u32 = 64;
/// WhiteBalance, and the Daylight and Shade levels.
const WHITE_BALANCE: u16 = 0xfa0d;
const AUTO: u16 = 0xfa25;
const DAYLIGHT: u16 = 0xfa29;
const SHADE: u16 = 0xfa2a;
/// The camera's image: its size, then its left and top edges.
const IMAGE: [u16; 4] = [0xfa3e, 0xfa3f, 0xfa31, 0xfa32];

/// Where a KDC's sensor data is, and its white.
#[derive(Clone, Debug)]
pub(crate) struct KdcPlan {
    pub offset: u64,
    pub width: u32,
    pub height: u32,
    pub white: u32,
}

/// The directory holding tag 0xFD04.
fn data_ifd<'a>(t: &'a TiffFile) -> Option<&'a Ifd> {
    t.ifds.iter().find(|ifd| ifd.has(DATA))
}

/// The KDC IFD.
fn kdc_ifd(t: &TiffFile) -> Option<Ifd> {
    t.ifd_at(u64::from(t.int(&t.ifds[0], KDC_IFD)?))
}

/// Whether the TIFF `t` is a KDC of this layout.
pub(crate) fn is_kdc(t: &TiffFile) -> bool {
    t.ifds[0].has(KDC_IFD) && data_ifd(t).is_some()
}

/// Reads the header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let kdc = kdc_ifd(&t).ok_or_else(|| corrupt("KDC IFD"))?;
    // An odd height is read one row taller (as RAWmakase has always read
    // it; the data has the row).
    let [width, height] = RAW_SIZE.map(|tag| t.int(&kdc, tag).unwrap_or(0));
    let height = height.saturating_add(height & 1);
    crate::io::samples(width, height, 1)?;
    let at = data_ifd(&t)
        .and_then(|ifd| t.ints_upto(ifd, DATA, 17))
        .filter(|v| v.len() == 17)
        .and_then(|v| v[16].checked_add(STRIP_HEADER))
        .ok_or_else(|| corrupt("KDC data offset"))?;
    let colors: Vec<u8> = t
        .text_upto(&kdc, FILTER, 8)
        .filter(|f| f.len() == 4)
        .map(|f| {
            f.bytes()
                .map(|c| match c {
                    b'R' => 0,
                    b'G' => 1,
                    b'B' => 2,
                    _ => 3,
                })
                .collect()
        })
        .filter(|c: &Vec<u8>| {
            let mut s = c.clone();
            s.sort();
            s == [0, 1, 1, 2]
        })
        .ok_or_else(|| Error::Unsupported("KDC filter".into()))?;
    let mut info = crate::info::base(Variant::KdcPacked);
    info.make = t.text_upto(ifd0, 271, 64).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, 64).unwrap_or_default();
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
    // The camera table's filter where the file's is not the one RAWmakase
    // has always read (Adobe DNG Converter's agrees with the table).
    let colors = crate::facts::camera(&info.make, &info.model, false, [width, height])
        .find_map(|c| c.cfa)
        .map_or(colors, |c| c.to_vec());
    info.cfa = Some(crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors,
    }));
    super::tiff_exif::read(&t, &mut info);
    // The camera's image, starting on even rows and columns, as the file
    // gives it (on some bodies it runs past the sensor's right and bottom
    // edges; RAWmakase has always taken it so).
    if let [Some(left), Some(top), Some(w), Some(h)] = IMAGE.map(|tag| t.int(&kdc, tag))
        && w > 0
        && h > 0
        && left < width
        && top < height
        && w <= width
        && h <= height
    {
        info.frame = crate::geometry::even([left, top, w, h]);
    }
    // Red, green and blue levels in 65536ths.
    let levels = |tag: u16| {
        t.ints_upto(&kdc, tag, 3)
            .filter(|l| l.len() == 3 && l.iter().all(|&v| v > 0))
            .map(|l| l.iter().map(|&v| v as f32).collect::<Vec<f32>>())
    };
    // The levels of the WhiteBalance setting as shot: Auto (0), Daylight
    // (3), Shade (6); else the as-shot levels.
    let setting = match t.raw_upto(&kdc, WHITE_BALANCE, 1).as_deref() {
        Some([0]) => AUTO,
        Some([3]) => DAYLIGHT,
        Some([6]) => SHADE,
        _ => WB_LEVELS,
    };
    if let Some(l) = levels(setting).or_else(|| levels(WB_LEVELS)) {
        info.cam_mul = [l[0] / 65536., l[1] / 65536., l[2] / 65536., l[1] / 65536.];
    }
    if let Some(l) = levels(DAYLIGHT) {
        info.presets.daylight = [l[0], l[1], l[2]];
    }
    if let Some(l) = levels(SHADE) {
        info.presets.shade = [l[0], l[1], l[2]];
    }
    info.previews = t.previews();
    info.plan = Plan::Kdc(KdcPlan {
        offset: t.base + u64::from(at),
        width,
        height,
        // The camera table's white (Adobe DNG Converter's), else the
        // samples' 12-bit range.
        white: crate::facts::camera(&info.make, &info.model, false, [width, height])
            .find_map(|c| c.white)
            .unwrap_or(4095),
    });
    Ok(info)
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &KdcPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let width = plan.width as usize;
    let mut data = crate::io::zeroed(crate::io::samples(plan.width, plan.height, 1)?)?;
    // Truncated files keep what they have.
    let src = usize::try_from(plan.offset)
        .ok()
        .and_then(|at| bytes.get(at..))
        .ok_or_else(|| corrupt("KDC data past the end of the file"))?;
    let p = Packing::msb(12);
    packed::rows(src, p, width, p.row_bytes(width), &mut data, cancel)?;
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area: [0, 0, info.width, info.height],
        black: [0; 4],
        black_pattern: (0, 0, Vec::new()),
        white: plan.white,
        linear_white: [0; 4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::test_tiff::{E, ascii, dir, long, offsets, short, tiff};
    use crate::io::Order;

    const LE: Order = Order::Little;

    /// A little-endian KDC of 4 × 3 (read 4 × 4) samples `data` after a
    /// 64-byte strip header, WhiteBalance `setting`.
    fn kdc(data: &[u8], setting: u8) -> Vec<u8> {
        let strip = [vec![0xaa; 64], data.to_vec()].concat();
        let kdc_at = offsets(&[&strip, &[]])[1];
        let kdc_ifd = dir(
            &[
                long(LE, 0xfa12, &[65536, 65536, 65536]),
                short(LE, 0xfa13, &[4]),
                short(LE, 0xfa14, &[3]),
                ascii(0xfa15, "BGGR"),
                E(0xfa0d, 1, vec![setting]),
                long(LE, 0xfa25, &[131072, 65536, 98304]),
                long(LE, 0xfa29, &[98304, 65536, 131072]),
                short(LE, 0xfa3e, &[0]),
                short(LE, 0xfa3f, &[1]),
                short(LE, 0xfa31, &[4]),
                short(LE, 0xfa32, &[2]),
            ],
            kdc_at,
            LE,
        );
        let sub_at = offsets(&[&strip, &kdc_ifd, &[]])[2];
        let mut list = vec![0u32; 17];
        list[16] = 8;
        let sub = dir(&[long(LE, 0xfd04, &list)], sub_at, LE);
        let ifd0 = vec![
            ascii(271, "EASTMAN KODAK COMPANY"),
            ascii(272, "KODAK P880 ZOOM DIGITAL CAMERA"),
            long(LE, 330, &[sub_at]),
            long(LE, KDC_IFD, &[kdc_at]),
        ];
        tiff(b"II*\0", LE, ifd0, vec![], &[&strip, &kdc_ifd, &sub])
    }

    #[test]
    fn reads_the_kdc_ifd_and_packed_rows() {
        let data = [0x12, 0x34, 0x56].repeat(8);
        let file = kdc(&data, 0);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, Variant::KdcPacked);
        // An odd height is read a row taller.
        assert_eq!((info.raw_width, info.raw_height), (4, 4));
        assert_eq!(info.frame, [0, 2, 4, 1]);
        // WhiteBalance 0: the Auto levels.
        assert_eq!(info.cam_mul, [2., 1., 1.5, 1.]);
        assert_eq!(info.presets.daylight, [98304., 65536., 131072.]);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert!(s.data.chunks(2).all(|p| p == [0x123, 0x456]));
        // Another setting: the as-shot levels.
        assert_eq!(crate::open(&kdc(&data, 9)).unwrap().cam_mul, [1.; 4]);
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
