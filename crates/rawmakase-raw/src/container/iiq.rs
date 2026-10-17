//! Phase One IIQ (and the TIFFs of Phase One's backs, Leaf's Credo IIQ): a
//! TIFF whose IFD0 is a thumbnail and whose EXIF maker note, eight bytes
//! into the file, is Phase One's own block (`vendor::phase_one`) with the
//! sensor data and its description: IIQ L (14 or 16-bit) and IIQ S coded
//! rows (`codec::phase_one`), IIQ Sv2 rows, or 16-bit words. The older
//! scrambled formats are `Unsupported`.
//!
//! Sources: TIFF 6, ExifTool's PhaseOne tag documentation, and our own
//! measurements against LibRaw's output and Adobe DNG Converter's on our
//! files: the visible area is the image area, an odd left or top edge
//! moved in by one; the colour filter at the sensor's corner is red first
//! (`compat-observed` rows give the older backs'); black is BlackLevel (0
//! without one), white 65535, Adobe DNG Converter's WhiteLevel for every
//! Phase One file. The black offsets per row and per column the block may
//! carry (BlackLevelData) are not applied: `Sensor` has no way to say them.
use super::tiff::TiffFile;
use crate::codec::phase_one::{self, Output};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::phase_one as notes;
use std::sync::atomic::AtomicBool;

/// Where the Phase One block is in the file.
const BLOCK: u64 = 8;
/// The white level of IIQ data: Adobe DNG Converter's WhiteLevel.
const WHITE: u32 = 0xffff;

/// Where an IIQ's sensor data is and how it is coded, and its levels.
#[derive(Clone, Debug)]
pub(crate) struct IiqPlan {
    pub data: (u64, u64),
    /// Each row's offset into the data (coded formats).
    pub rows: Vec<u32>,
    pub order: Order,
    pub output: Output,
    pub width: u32,
    pub height: u32,
    pub black: u32,
    pub white: u32,
}

/// Whether the file in `bytes` is a TIFF holding a Phase One block.
pub(crate) fn is_iiq(bytes: &[u8]) -> bool {
    matches!(bytes.get(8..12), Some(b"IIII" | b"MMMM")) && notes::read(bytes, BLOCK).is_some()
}

/// Reads the header facts. `t` is the parsed TIFF.
pub(crate) fn open(bytes: &[u8], t: TiffFile) -> Result<RawInfo> {
    let n = notes::read(bytes, BLOCK).ok_or_else(|| corrupt("Phase One directory"))?;
    let [width, height] = n.raw_size;
    crate::io::samples(width, height, 1)?;
    let (variant, output) = match n.format {
        0 => (Variant::IiqUncompressed, Output::Plain),
        3 => (Variant::IiqL, Output::Times4),
        8 => (Variant::IiqL, Output::Plain),
        5 => (Variant::IiqS, Output::Curve),
        6 => (Variant::IiqSv2, Output::Times4),
        f => {
            return Err(Error::Unsupported(format!("IIQ raw format {f}")));
        }
    };
    let order = n.order.unwrap_or(Order::Little);
    let rows = if variant == Variant::IiqUncompressed {
        Vec::new()
    } else {
        let (at, size) = n.strips.ok_or_else(|| corrupt("IIQ without row offsets"))?;
        if size / 4 < u64::from(height) {
            return Err(corrupt("IIQ row offsets"));
        }
        let b = Bytes::new(bytes, order);
        (0..u64::from(height))
            .map(|r| b.u32(at + 4 * r))
            .collect::<Result<Vec<u32>>>()?
    };
    if n.data.1 == 0 {
        return Err(corrupt("IIQ without raw data"));
    }
    let ifd0 = &t.ifds[0];
    let mut info = crate::info::base(variant);
    info.make = t.text_upto(ifd0, 271, 64).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, 64).unwrap_or_default();
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);
    info.raw_width = width;
    info.raw_height = height;
    // The image area, an odd left or top edge moved in by one.
    let [left, top, w, h] = crate::geometry::even(n.image);
    let fits = |start: u32, len: u32, total: u32| {
        len > 0 && start.checked_add(len).is_some_and(|end| end <= total)
    };
    if fits(left, w, width) && fits(top, h, height) {
        info.margins = [left, top];
        info.width = w;
        info.height = h;
    } else {
        info.width = width;
        info.height = height;
    }
    // CameraOrientation's quarter turns.
    info.flip = [0, 6, 5, 3][(n.orientation & 3) as usize];
    let corner = crate::facts::camera(&info.make, &info.model, false, [width, height])
        .find_map(|c| c.cfa)
        .map_or(vec![0, 1, 1, 2], |c| c.to_vec());
    let cfa = crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors: corner,
    });
    info.cfa = Some(cfa.shifted(info.margins[1] as usize, info.margins[0] as usize));
    super::tiff_exif::read(&t, &mut info);
    if info.lens.is_empty() {
        info.lens = n.lens.clone();
    }
    if let Some([r, g, b]) = n.wb {
        info.cam_mul = [r, g, b, g];
    }
    info.previews = t.previews();
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    let black = n.black.unwrap_or(0);
    if black > 0xffff {
        return Err(corrupt("IIQ black level"));
    }
    info.plan = Plan::Iiq(IiqPlan {
        data: n.data,
        rows,
        order,
        output,
        width,
        height,
        black,
        white: WHITE,
    });
    Ok(info)
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &IiqPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let mut data = crate::io::zeroed(crate::io::samples(plan.width, plan.height, 1)?)?;
    let (at, size) = plan.data;
    let src = Bytes::new(bytes, plan.order).slice(at, size)?;
    match info.variant {
        Variant::IiqL | Variant::IiqS => phase_one::decode(
            src,
            &plan.rows,
            plan.order,
            plan.output,
            plan.width as usize,
            &mut data,
            cancel,
        )?,
        Variant::IiqSv2 => phase_one::decode_sv2(
            src,
            &plan.rows,
            plan.order,
            plan.width as usize,
            &mut data,
            cancel,
        )?,
        Variant::IiqUncompressed => {
            crate::codec::packed::words(src, plan.order, 0, &mut data, cancel)?
        }
        v => {
            return Err(Error::Unsupported(format!(
                "{} is not an IIQ variant",
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
        black: [plan.black; 4],
        black_pattern: (0, 0, Vec::new()),
        white: plan.white,
        linear_white: [0; 4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::test_tiff::{ascii, tiff};

    /// A little-endian IIQ by `model` of 4 × 2 samples in `format`: rows of
    /// literal 16-bit values (rows narrower than eight columns are all
    /// literals), the image area from column 1, and a quarter turn.
    fn iiq(model: &str, format: u32, rows: [[u16; 4]; 2]) -> Vec<u8> {
        let mut data = Vec::new();
        for r in rows {
            for pair in r.chunks(2) {
                let word = (u32::from(pair[0]) << 16) | u32::from(pair[1]);
                data.extend(word.to_le_bytes());
            }
        }
        // The block's values after its directory of nine entries (16 + 8 +
        // 9 × 16 = 168 bytes): the row offsets, then the data.
        let (strips, raw) = (168u32, 176u32);
        let mut block = crate::vendor::phase_one::tests::block(&[
            (0x0108, 4, 4),
            (0x0109, 4, 2),
            (0x010a, 4, 1),
            (0x010c, 4, 2),
            (0x010d, 4, 2),
            (0x010e, 4, format),
            (0x010f, data.len() as u32, raw),
            (0x021c, 8, strips),
            (0x0100, 4, 1),
        ]);
        assert_eq!(block.len(), strips as usize);
        block.extend(0u32.to_le_bytes());
        block.extend(8u32.to_le_bytes());
        block.extend(data);
        block.extend(0u32.to_le_bytes());
        tiff(
            b"II*\0",
            Order::Little,
            vec![ascii(271, "Phase One A/S"), ascii(272, model)],
            vec![],
            &[&block],
        )
    }

    fn decode(file: &[u8]) -> Result<(RawInfo, Sensor)> {
        let info = crate::open(file)?;
        let sensor = crate::decode(&info, file, &AtomicBool::new(false))?;
        Ok((info, sensor))
    }

    #[test]
    fn reads_iiq_l_and_its_block() {
        let rows = [[1, 2, 3, 4], [0x3fff, 5, 6, 7]];
        let (info, s) = decode(&iiq("IQ3 100MP", 3, rows)).unwrap();
        assert_eq!(info.variant, Variant::IiqL);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Phase One", "IQ3 100MP")
        );
        // IIQ L of 14 bits: values times 4.
        assert_eq!(s.data, [4, 8, 12, 16, 0xfffc, 20, 24, 28]);
        // The image area's odd left edge moves in by one.
        assert_eq!((info.margins, info.width, info.height), ([2, 0], 1, 2));
        assert_eq!(s.area, [2, 0, 1, 2]);
        assert_eq!(info.flip, 6);
        assert_eq!((s.black, s.white), ([0; 4], 0xffff));
        // L16 keeps its values, the first IIQ S curves those below 256.
        let (_, s) = decode(&iiq("IQ3 100MP", 8, rows)).unwrap();
        assert_eq!(s.data[..4], [1, 2, 3, 4]);
        let (info, s) = decode(&iiq("IQ3 100MP", 5, rows)).unwrap();
        assert_eq!(info.variant, Variant::IiqS);
        assert_eq!(s.data[4], 0xfffc);
        assert_eq!(s.data[0], 0);
        // An older back's filter comes from the camera table: GRBG at the
        // sensor's corner, seen from column 2.
        let (info, _) = decode(&iiq("P45+", 3, rows)).unwrap();
        let cfa = info.cfa.unwrap();
        assert_eq!((cfa.color(0, 0), cfa.color(0, 1)), (1, 0));
    }

    #[test]
    fn unknown_formats_are_unsupported_and_bad_files_do_not_panic() {
        let rows = [[1, 2, 3, 4], [5, 6, 7, 8]];
        for format in [1, 2, 7] {
            assert!(matches!(
                crate::open(&iiq("IQ3 100MP", format, rows)),
                Err(Error::Unsupported(_))
            ));
        }
        for format in [0, 3, 5, 6, 8] {
            let file = iiq("IQ3 100MP", format, rows);
            let info = crate::open(&file).unwrap();
            assert_eq!(
                crate::decode(&info, &file, &AtomicBool::new(true)).unwrap_err(),
                Error::Cancelled
            );
            for end in 0..file.len() {
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
