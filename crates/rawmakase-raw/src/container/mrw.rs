//! Minolta MRW: "\0MRM" and the size of a run of big-endian blocks (a
//! 4-byte name, a 4-byte size): PRD (the sensor's and image's sizes, bit
//! depths, storage and filter), WBG (the white balance levels), RIF and TTW
//! (a TIFF with the EXIF); the sensor data follows the blocks: 12-bit
//! samples packed most significant bit first ("linear", 0x59) or 16-bit
//! big-endian words ("padded", 0x52).
//!
//! Sources: ExifTool's MinoltaRaw tag documentation (the blocks, PRD's
//! fields, WBG's levels), and our own measurements on the test dataset's
//! files (the data after the blocks, the levels as the as-shot white
//! balance in the filter's order).
use super::tiff::TiffFile;
use crate::codec::packed::{self, Packing};
use crate::error::{Error, Result, corrupt};
use crate::exif_tiffs::ExifTiffs;
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order, Pieces};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use rawmakase_cancel::Cancel;

/// The most blocks read.
const MAX_BLOCKS: usize = 64;

/// Where an MRW's sensor data is, and its levels.
#[derive(Clone, Debug)]
pub(crate) struct MrwPlan {
    pub offset: u64,
    pub width: u32,
    pub height: u32,
    pub white: u32,
}

/// Whether `bytes` is an MRW.
pub(crate) fn is_mrw(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\0MRM")
}

/// The blocks read, and where the sensor data starts.
struct Blocks<'a> {
    data: u64,
    prd: Option<&'a [u8]>,
    wbg: Option<&'a [u8]>,
    rif: Option<&'a [u8]>,
    /// Where TTW's TIFF starts, and its block's size.
    ttw: Option<(u64, u64)>,
}

fn blocks(bytes: &[u8]) -> Result<Blocks<'_>> {
    let b = Bytes::new(bytes, Order::Big);
    let mut out = Blocks {
        data: 8u64
            .checked_add(u64::from(b.u32(4)?))
            .ok_or_else(|| corrupt("MRW size"))?,
        prd: None,
        wbg: None,
        rif: None,
        ttw: None,
    };
    let mut at = 8u64;
    for _ in 0..MAX_BLOCKS {
        if at.saturating_add(8) > out.data {
            break;
        }
        let name = b.slice(at, 4)?;
        let size = u64::from(b.u32(at + 4)?);
        let body = at + 8;
        match name {
            b"\0PRD" if size >= 24 => out.prd = Some(b.slice(body, 24)?),
            b"\0WBG" if size >= 12 => out.wbg = Some(b.slice(body, 12)?),
            b"\0RIF" if size >= 16 => out.rif = Some(b.slice(body, 16)?),
            b"\0TTW" => out.ttw = Some((body, size)),
            _ => {}
        }
        at = body.saturating_add(size);
    }
    Ok(out)
}

/// The camera's EXIF: TTW's TIFF, the blocks before the sensor data read
/// alone.
pub(crate) fn exif_tiffs(f: &mut Pieces) -> ExifTiffs {
    let head = f
        .read(4, 4)
        .and_then(|size| f.read(0, 8 + u64::from(u32::from_be_bytes(size.try_into().ok()?))));
    let main = head.as_deref().and_then(|head| {
        let (at, size) = blocks(head).ok()?.ttw?;
        let at = usize::try_from(at).ok()?;
        let end = at
            .saturating_add(usize::try_from(size).ok()?)
            .min(head.len());
        Some(head.get(at..end)?.to_vec())
    });
    ExifTiffs {
        main,
        ..Default::default()
    }
}

/// Reads the header facts.
pub(crate) fn open(bytes: &[u8]) -> Result<RawInfo> {
    let Blocks {
        data,
        prd,
        wbg,
        rif,
        ttw,
    } = blocks(bytes)?;
    let prd = prd.ok_or_else(|| corrupt("MRW without PRD"))?;
    let be16 = |o: usize| u32::from(u16::from_be_bytes([prd[o], prd[o + 1]]));
    let (height, width) = (be16(8), be16(10));
    crate::io::samples(width, height, 1)?;
    // RawDepth (the stored bits) and BitDepth (the samples').
    let (bits, depth) = (u32::from(prd[16]), u32::from(prd[17]));
    let variant = match (prd[18], bits) {
        (0x59, 12) => Variant::MrwPacked,
        (0x52, 12 | 16) => Variant::MrwUnpacked,
        (storage, _) => {
            return Err(Error::Unsupported(format!(
                "MRW storage {storage:#x} of {bits} bits"
            )));
        }
    };
    // The filter at the sensor's corner: RGGB (1) or GBRG (4).
    let colors = match prd[23] {
        1 => vec![0, 1, 1, 2],
        4 => vec![1, 2, 0, 1],
        p => return Err(Error::Unsupported(format!("MRW Bayer pattern {p}"))),
    };
    let mut info = crate::info::base(variant);
    if let Some(t) = ttw.and_then(|(at, _)| TiffFile::parse(bytes, at)) {
        info.make = t.text_upto(&t.ifds[0], 271, 64).unwrap_or_default();
        info.model = t.text_upto(&t.ifds[0], 272, 64).unwrap_or_default();
        info.flip = crate::geometry::flip(t.int(&t.ifds[0], 274).unwrap_or(1));
        super::tiff_exif::read(&t, &mut info);
        info.previews = t.previews();
        // Exposure as APEX values where the times are not given.
        if let Some(exif) = &t.exif {
            if info.shutter == 0.
                && let Some(v) = t
                    .real(exif, 37377)
                    .filter(|v| v.is_finite() && v.abs() < 64.)
            {
                info.shutter = (-v).exp2() as f32;
            }
            if info.aperture == 0.
                && let Some(v) = t
                    .real(exif, 37378)
                    .filter(|v| v.is_finite() && v.abs() < 64.)
            {
                info.aperture = (v / 2.).exp2() as f32;
            }
        }
        if let Some(preview) = minolta_preview(&t) {
            info.previews.push(preview);
        }
    }
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);
    // The as-shot levels, in the filter's order.
    if let Some(w) = wbg {
        let level = |i: usize| f32::from(u16::from_be_bytes([w[4 + 2 * i], w[5 + 2 * i]]));
        let [a, b0, c, d] = [level(0), level(1), level(2), level(3)];
        let (r, g, bl, g2) = if prd[23] == 4 {
            (c, a, b0, d)
        } else {
            (a, b0, d, c)
        };
        if [r, g, bl, g2].iter().all(|&v| v > 0.) {
            info.cam_mul = [r, g, bl, g2];
        }
    }
    // RIF's daylight red and blue levels, over a green of 256.
    if let Some(r) = rif {
        let level = |o: usize| f32::from(u16::from_be_bytes([r[o], r[o + 1]]));
        let (red, blue) = (level(12), level(14));
        if red > 0. && blue > 0. {
            info.presets.daylight = [red, 256., blue];
        }
    }
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    info.cfa = Some(crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors,
    }));
    let white = crate::facts::camera(&info.make, &info.model, false, [width, height])
        .find_map(|c| c.white)
        .unwrap_or((1 << depth.clamp(1, 16)) - 1);
    info.plan = Plan::Mrw(MrwPlan {
        offset: data,
        width,
        height,
        white,
    });
    Ok(info)
}

/// The JPEG preview Minolta's maker note gives: PreviewImageStart and
/// PreviewImageLength (0x0088, 0x0089), from the TIFF's header, or the
/// older PreviewImage (0x0081).
fn minolta_preview(t: &TiffFile) -> Option<crate::preview::Candidate> {
    let entry = t.exif.as_ref()?.entries.get(&0x927c)?;
    let note = t.ifd_at(t.offset_of(entry)?)?;
    let (at, len) = match (t.int(&note, 0x0088), t.int(&note, 0x0089)) {
        (Some(at), Some(len)) => (u64::from(at), u64::from(len)),
        _ => {
            let e = note.entries.get(&0x0081)?;
            (t.offset_of(e)?, u64::from(e.count))
        }
    };
    Some(crate::preview::Candidate {
        offset: t.base + at,
        len,
        pixels: 0,
    })
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &MrwPlan,
    bytes: &[u8],
    cancel: Cancel<'_>,
) -> Result<Sensor> {
    let width = plan.width as usize;
    let mut data = crate::io::zeroed(crate::io::samples(plan.width, plan.height, 1)?)?;
    // Truncated files keep what they have.
    let src = usize::try_from(plan.offset)
        .ok()
        .and_then(|at| bytes.get(at..))
        .ok_or_else(|| corrupt("MRW data past the end of the file"))?;
    match info.variant {
        Variant::MrwPacked => {
            let p = Packing::msb(12);
            packed::rows(src, p, width, p.row_bytes(width), &mut data, cancel)?;
        }
        Variant::MrwUnpacked => packed::words(src, Order::Big, 0, &mut data, cancel)?,
        v => {
            return Err(Error::Unsupported(format!(
                "{} is not an MRW variant",
                v.name()
            )));
        }
    }
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
    use std::sync::atomic::AtomicBool;

    /// A block named `name` holding `body`.
    fn block(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = name.to_vec();
        out.extend((body.len() as u32).to_be_bytes());
        out.extend(body);
        out
    }

    /// An MRW of 4 × 2 samples stored as `storage` with Bayer pattern
    /// `bayer`, then `data`.
    fn mrw(storage: u8, bayer: u8, data: &[u8]) -> Vec<u8> {
        mrw_with(storage, bayer, data, None)
    }

    /// As `mrw`, with the TTW block `ttw` after the others.
    fn mrw_with(storage: u8, bayer: u8, data: &[u8], ttw: Option<&[u8]>) -> Vec<u8> {
        let mut prd = b"21810002".to_vec();
        for v in [2u16, 4, 2, 4] {
            prd.extend(v.to_be_bytes());
        }
        prd.extend([
            if storage == 0x52 { 16 } else { 12 },
            12,
            storage,
            0,
            0,
            0,
            0,
            bayer,
        ]);
        let mut wbg = vec![2; 4];
        for v in [400u16, 256, 256, 300] {
            wbg.extend(v.to_be_bytes());
        }
        let mut rif = vec![0; 12];
        rif.extend([1, 0x7e, 1, 0x7c]);
        let mut blocks = [
            block(b"\0PRD", &prd),
            block(b"\0WBG", &wbg),
            block(b"\0RIF", &rif),
        ]
        .concat();
        if let Some(ttw) = ttw {
            blocks.extend(block(b"\0TTW", ttw));
        }
        let mut out = block(b"\0MRM", &blocks);
        out.extend(data);
        out
    }

    fn decode(file: &[u8]) -> Result<(RawInfo, Sensor)> {
        let info = crate::open(file)?;
        let sensor = crate::decode(&info, file, Cancel::NEVER)?;
        Ok((info, sensor))
    }

    #[test]
    fn reads_the_blocks_and_both_storages() {
        // Packed: 0x123 0x456 0x789 0xabc, then 0x111 four times.
        let packed = [
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
        ];
        let (info, s) = decode(&mrw(0x59, 1, &packed)).unwrap();
        assert_eq!(info.variant, Variant::MrwPacked);
        assert_eq!(
            s.data,
            [0x123, 0x456, 0x789, 0xabc, 0x111, 0x111, 0x111, 0x111]
        );
        assert_eq!((s.black, s.white), ([0; 4], 4095));
        // WBG's levels in the filter's order (RGGB): red 400, blue 300.
        assert_eq!(info.cam_mul, [400., 256., 300., 256.]);
        assert_eq!(info.presets.daylight, [382., 256., 380.]);
        // Padded: 16-bit big-endian words; GBRG's levels are G, B, R, G.
        let words: Vec<u8> = (1u16..=8).flat_map(|v| v.to_be_bytes()).collect();
        let (info, s) = decode(&mrw(0x52, 4, &words)).unwrap();
        assert_eq!(info.variant, Variant::MrwUnpacked);
        assert_eq!(s.data, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(info.cam_mul, [256., 400., 256., 300.]);
    }

    /// TTW's TIFF dates an MRW; the sensor data after the blocks is not
    /// read.
    #[test]
    fn the_tiff_block_dates_an_mrw() {
        use super::super::test_tiff::{ascii, tiff};
        let ttw = tiff(
            b"MM\0*",
            Order::Big,
            vec![
                ascii(271, "Minolta Co., Ltd."),
                ascii(306, "2005:03:24 14:56:53"),
            ],
            vec![
                ascii(36867, "2005:03:24 14:56:52"),
                ascii(36868, "2005:03:24 14:56:52"),
            ],
            &[],
        );
        let file = mrw_with(0x59, 1, &[0; 12], Some(&ttw));
        let dates = crate::capture_dates(&mut std::io::Cursor::new(&file));
        let stamp = |date: &str| crate::Stamp {
            date: Some(date.into()),
            subseconds: None,
        };
        assert_eq!(
            dates,
            crate::CaptureDates {
                original: stamp("2005:03:24 14:56:52"),
                digitized: stamp("2005:03:24 14:56:52"),
                modified: stamp("2005:03:24 14:56:53"),
            }
        );
        // The decoder reads the same block, which is the camera's EXIF.
        assert_eq!(crate::open(&file).unwrap().make, "Minolta");
        assert_eq!(
            crate::exif_tiffs(&mut std::io::Cursor::new(&file)).main,
            Some(ttw)
        );
        // Without TTW, no dates.
        assert_eq!(
            crate::capture_dates(&mut std::io::Cursor::new(&mrw(0x59, 1, &[0; 12]))),
            crate::CaptureDates::default()
        );
        for end in 0..file.len() {
            let _ = crate::capture_dates(&mut std::io::Cursor::new(&file[..end]));
            let _ = crate::exif_tiffs(&mut std::io::Cursor::new(&file[..end]));
        }
    }

    #[test]
    fn other_storage_is_unsupported_and_bad_files_do_not_panic() {
        assert!(matches!(
            crate::open(&mrw(0x40, 1, &[0; 12])),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            crate::open(&mrw(0x59, 2, &[0; 12])),
            Err(Error::Unsupported(_))
        ));
        let file = mrw(0x59, 1, &[0x5a; 12]);
        let info = crate::open(&file).unwrap();
        assert_eq!(
            crate::decode(&info, &file, Cancel::new(&AtomicBool::new(true))).unwrap_err(),
            Error::Cancelled
        );
        for end in 0..file.len() {
            let _ = decode(&file[..end]);
        }
        let mut noisy = file;
        for (i, b) in noisy.iter_mut().enumerate().skip(4) {
            *b ^= (i * 7919 % 251) as u8;
        }
        let _ = decode(&noisy);
    }
}
