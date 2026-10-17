//! Leaf MOS: a TIFF whose IFD0 is the raw image, one tile of 16-bit
//! samples, lossless JPEG coded (compression 99) or not (1), with Leaf's
//! private data (`vendor::leaf`) and no make. The lossless JPEG's MCU rows
//! are the image's rows.
//!
//! Sources: TIFF 6, ITU T.81, ExifTool's Leaf tag documentation, and our own
//! measurements on the test dataset's files: the camera named from the back
//! type (`[[name]]` rows of the names RAWmakase has always given), the row
//! order Rows_data names, and the levels: black 0, white the camera
//! table's (Adobe DNG Converter's 15000 for the Aptus 22, the one back it
//! converts), else 16383, what RAWmakase has always read.
use super::tiff::TiffFile;
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::leaf;
use std::sync::atomic::AtomicBool;

/// Leaf's private data.
const LEAF_DATA: u16 = 0x8606;
/// The white level of Leaf's 14-bit backs.
const WHITE: u32 = 16383;

/// Where a MOS's sensor data is.
#[derive(Clone, Debug)]
pub(crate) struct MosPlan {
    pub data: (u64, u64),
    /// The coded rows are the image's even rows, then its odd ones from
    /// the bottom up.
    pub interlaced: bool,
    pub order: Order,
    pub width: u32,
    pub height: u32,
    pub white: u32,
}

/// Whether the TIFF `t` is a Leaf MOS: Leaf's data in IFD0, which holds a
/// one-sample image.
pub(crate) fn is_mos(t: &TiffFile) -> bool {
    let ifd0 = &t.ifds[0];
    ifd0.has(LEAF_DATA)
        && t.int(ifd0, 277).unwrap_or(1) == 1
        && t.raw_upto(ifd0, LEAF_DATA, 4).as_deref() == Some(b"PKTS")
}

/// Reads the header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let width = t.int(ifd0, 256).ok_or_else(|| corrupt("MOS width"))?;
    let height = t.int(ifd0, 257).ok_or_else(|| corrupt("MOS height"))?;
    crate::io::samples(width, height, 1)?;
    let bits = t.int(ifd0, 258).unwrap_or(16);
    let compression = t.int(ifd0, 259).unwrap_or(1);
    let notes = ifd0
        .entries
        .get(&LEAF_DATA)
        .and_then(|e| Some((t.offset_of(e)?, u64::from(e.count))))
        .and_then(|(at, len)| leaf::read(t.bytes.data, at, len))
        .ok_or_else(|| corrupt("MOS without Leaf's data"))?;
    if notes.planes.unwrap_or(1) != 1 {
        return Err(Error::Unsupported("MOS of several planes".into()));
    }
    let variant = match (compression, bits) {
        (99, 16) => Variant::MosLosslessJpeg,
        (1, 16) => Variant::MosUncompressed,
        _ => {
            return Err(Error::Unsupported(format!(
                "MOS compression {compression} of {bits} bits"
            )));
        }
    };
    // One tile (or strip) of the whole image.
    let (offsets, counts) = match (t.ints_upto(ifd0, 324, 2), t.ints_upto(ifd0, 325, 2)) {
        (Some(o), Some(c)) => (o, c),
        _ => (
            t.ints_upto(ifd0, 273, 2).unwrap_or_default(),
            t.ints_upto(ifd0, 279, 2).unwrap_or_default(),
        ),
    };
    let ([offset], [count]) = (&offsets[..], &counts[..]) else {
        return Err(Error::Unsupported("MOS of several tiles".into()));
    };
    let mut info = crate::info::base(variant);
    info.make = t.text_upto(ifd0, 271, 64).unwrap_or_else(|| "Leaf".into());
    info.model = notes
        .back_type
        .map_or_else(String::new, |b| format!("back type {b}"));
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    info.flip = match notes.rotation {
        Some(90) => 6,
        Some(180) => 3,
        Some(270) => 5,
        _ => 0,
    };
    info.cfa = Some(crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors: notes.cfa.map_or(vec![0, 1, 1, 2], |c| c.to_vec()),
    }));
    if let Some([n, r, g, b]) = notes.neutrals
        && [r, g, b].iter().all(|v| v.is_finite() && *v > 0.)
        && n.is_finite()
    {
        info.cam_mul = [
            (n / r) as f32,
            (n / g) as f32,
            (n / b) as f32,
            (n / g) as f32,
        ];
    }
    if let Some((at, len)) = notes.preview {
        info.previews.push(crate::preview::Candidate {
            offset: t.base + at,
            len,
            pixels: 0,
        });
    }
    info.plan = Plan::Mos(MosPlan {
        data: (t.base + u64::from(*offset), u64::from(*count)),
        interlaced: match notes.rows {
            None | Some(0) => false,
            Some(1) => true,
            Some(r) => return Err(Error::Unsupported(format!("MOS rows data {r}"))),
        },
        order: if t.little() {
            Order::Little
        } else {
            Order::Big
        },
        width,
        height,
        // The camera table's white (Adobe DNG Converter's) where it has one.
        white: crate::facts::camera(&info.make, &info.model, false, [width, height])
            .find_map(|c| c.white)
            .unwrap_or(WHITE),
    });
    Ok(info)
}

/// The image of `coded` rows of `width`, whose even rows come first, then
/// its odd rows from the bottom up (our measurement against LibRaw's output).
fn deinterlace(coded: &[u16], width: usize, cancel: &AtomicBool) -> Result<Vec<u16>> {
    let mut out = crate::io::zeroed(coded.len())?;
    let height = coded.len() / width.max(1);
    for (j, row) in coded.chunks_exact(width).enumerate() {
        if j % 256 == 0 && cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let y = if j % 2 == 0 {
            j / 2
        } else {
            height - 1 - j / 2
        };
        out[y * width..(y + 1) * width].copy_from_slice(row);
    }
    Ok(out)
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &MosPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let count = crate::io::samples(plan.width, plan.height, 1)?;
    let (at, size) = plan.data;
    // Truncated files keep what they have.
    let size = size.min((bytes.len() as u64).saturating_sub(at));
    let src = Bytes::new(bytes, plan.order).slice(at, size)?;
    let data = match info.variant {
        Variant::MosUncompressed => {
            let mut data = crate::io::zeroed(count)?;
            crate::codec::packed::words(src, plan.order, 0, &mut data, cancel)?;
            data
        }
        Variant::MosLosslessJpeg => {
            let h = crate::codec::ljpeg::header(src)?;
            let (mcus, rows) = h.mcus();
            let samples = mcus.checked_mul(h.samples_per_mcu());
            if samples != Some(plan.width as usize) || rows != plan.height as usize {
                return Err(Error::Unsupported(
                    "MOS lossless JPEG of another size than the image".into(),
                ));
            }
            let mut data = Vec::new();
            data.try_reserve_exact(count)
                .map_err(|_| Error::TooLarge("image allocation"))?;
            crate::codec::ljpeg::decode_into(&h, src, cancel, &mut data)?;
            data.resize(count, 0);
            data
        }
        v => {
            return Err(Error::Unsupported(format!(
                "{} is not a MOS variant",
                v.name()
            )));
        }
    };
    let data = if plan.interlaced {
        deinterlace(&data, plan.width as usize, cancel)?
    } else {
        data
    };
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
    use crate::container::test_tiff::{E, long, offsets, short, tiff};
    use crate::vendor::leaf::tests::packet;

    const LE: Order = Order::Little;

    /// A little-endian MOS of 4 × 4 samples: one tile `tile` (compression
    /// `compression`) and Leaf's data with `rows` as Rows_data.
    fn mos(compression: u16, tile: &[u8], rows: u32) -> Vec<u8> {
        let leaf = [
            packet(
                "camera_profile",
                &[
                    packet("ShootObj_back_type", b"12\0"),
                    packet("NeutObj_neutrals", b"3000\n1500\n3000\n2000\0"),
                    packet("CaptProf_mosaic_pattern", b"2\n0\n1\n3\0"),
                ]
                .concat(),
            ),
            packet("ImgProf_rotation_angle", b"90\0"),
            packet("JPEG_preview_data", &[0xff, 0xd8, 0xff, 0xd9]),
            packet("Rows_data", &rows.to_be_bytes()),
        ]
        .concat();
        let at = offsets(&[tile, &leaf]);
        let ifd0 = vec![
            long(LE, 256, &[4]),
            long(LE, 257, &[4]),
            short(LE, 258, &[16]),
            short(LE, 259, &[compression]),
            short(LE, 262, &[1]),
            long(LE, 322, &[4]),
            long(LE, 323, &[4]),
            long(LE, 324, &[at[0]]),
            long(LE, 325, &[tile.len() as u32]),
            E(LEAF_DATA, 7, vec![0; leaf.len()]),
        ];
        let mut file = tiff(b"II*\0", LE, ifd0, vec![], &[tile, &leaf]);
        crate::container::test_tiff::point(&mut file, LE, LEAF_DATA, leaf.len(), at[1]);
        file
    }

    fn decode(file: &[u8]) -> Result<(RawInfo, Sensor)> {
        let info = crate::open(file)?;
        let sensor = crate::decode(&info, file, &AtomicBool::new(false))?;
        Ok((info, sensor))
    }

    #[test]
    fn reads_leaf_s_data_and_rows() {
        let samples: Vec<u16> = (0..16).collect();
        let words: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        let (info, s) = decode(&mos(1, &words, 0)).unwrap();
        assert_eq!(info.variant, Variant::MosUncompressed);
        assert_eq!(s.data, samples);
        // The back type names the body, whose white the camera table gives.
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Leaf", "Aptus 22")
        );
        assert_eq!(s.white, 15000);
        assert_eq!(info.flip, 6);
        assert_eq!(info.cam_mul, [2., 1., 1.5, 1.]);
        let cfa = info.cfa.clone().unwrap();
        // Green, blue over red, green: the green beside blue is the second.
        assert_eq!(
            (cfa.color(0, 0), cfa.color(0, 1), cfa.color(1, 0)),
            (3, 2, 0)
        );
        assert_eq!(
            &crate::preview(&info, &mos(1, &words, 0)).unwrap()[..],
            [0xff, 0xd8, 0xff, 0xd9]
        );
        // Rows_data 1: even rows first, then the odd ones bottom up.
        let (_, s) = decode(&mos(1, &words, 1)).unwrap();
        assert_eq!(&s.data[..4], &samples[..4]);
        assert_eq!(&s.data[4..8], &samples[8..12]);
        assert_eq!(&s.data[8..12], &samples[12..]);
        assert_eq!(&s.data[12..], &samples[4..8]);
        // A lossless JPEG tile of the image's rows.
        let jpeg = crate::codec::ljpeg::tests::encode(&samples, 4, 16, 1);
        let (info, s) = decode(&mos(99, &jpeg, 0)).unwrap();
        assert_eq!(info.variant, Variant::MosLosslessJpeg);
        assert_eq!(s.data, samples);
    }

    #[test]
    fn other_data_is_unsupported_and_bad_files_do_not_panic() {
        let words = vec![0u8; 32];
        assert!(matches!(
            crate::open(&mos(7, &words, 0)),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            crate::open(&mos(1, &words, 2)),
            Err(Error::Unsupported(_))
        ));
        let samples: Vec<u16> = (0..16).collect();
        let jpeg = crate::codec::ljpeg::tests::encode(&samples, 4, 16, 1);
        for file in [mos(1, &words, 1), mos(99, &jpeg, 0)] {
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
