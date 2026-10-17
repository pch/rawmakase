//! Hasselblad 3FR (the camera's) and FFF (Phocus's, with its private tag
//! 0xC519 in IFD0): a TIFF whose IFD0 is a thumbnail and whose SubIFD holds
//! the raw image, 16-bit words (compression 1) or Hasselblad's coding
//! (compression 7, `codec::hasselblad`); the levels in DNG's BlackLevel
//! and WhiteLevel tags, white taken as Adobe DNG Converter takes it
//! (`adobe_white`). Multi-shot files (several samples a pixel) are
//! `Unsupported`. The maker note is `vendor::hasselblad`'s.
//!
//! Sources: TIFF 6, DNG 1.7 (the level tags), and our own measurements on
//! the test dataset's files: the camera's name (the model without what
//! follows a "/", the sensor's megapixels appended to look up an older
//! body's name), the visible areas (`compat-observed` rows).
use super::tiff::{Ifd, TiffFile};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::hasselblad;
use std::sync::atomic::AtomicBool;

/// Phocus's private data, in FFF files only.
const PHOCUS: u16 = 0xc519;

/// Where a 3FR's or FFF's sensor data is, and its levels.
#[derive(Clone, Debug)]
pub(crate) struct FffPlan {
    pub data: (u64, u64),
    pub order: Order,
    pub width: u32,
    pub height: u32,
    pub black: u32,
    pub white: u32,
}

/// The raw image's directory: a CFA image.
fn raw_ifd<'a>(t: &'a TiffFile) -> Option<&'a Ifd> {
    t.ifds
        .iter()
        .find(|ifd| t.int(ifd, 262) == Some(32803) && ifd.has(256) && ifd.has(273))
}

/// Whether the TIFF `t` is a Hasselblad 3FR or FFF.
pub(crate) fn is_fff(t: &TiffFile) -> bool {
    let make = t.text_upto(&t.ifds[0], 271, 64).unwrap_or_default();
    make.trim().to_ascii_uppercase().starts_with("HASSELBLAD") && raw_ifd(t).is_some()
}

/// Reads the header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let raw = raw_ifd(&t).ok_or_else(|| Error::Unsupported("3FR without raw data".into()))?;
    let width = t.int(raw, 256).ok_or_else(|| corrupt("3FR width"))?;
    let height = t.int(raw, 257).ok_or_else(|| corrupt("3FR height"))?;
    crate::io::samples(width, height, 1)?;
    let bits = t.int(raw, 258).unwrap_or(16);
    let samples = t.int(raw, 277).unwrap_or(1);
    let compression = t.int(raw, 259).unwrap_or(1);
    let variant = match (compression, bits, samples) {
        (_, _, s) if s != 1 => {
            return Err(Error::Unsupported(format!(
                "3FR of {s} samples a pixel (multi-shot)"
            )));
        }
        (1, 16, _) => Variant::FffUncompressed,
        (7, 16, _) => Variant::FffCompressed,
        _ => {
            return Err(Error::Unsupported(format!(
                "3FR compression {compression} of {bits} bits"
            )));
        }
    };
    let offset = t.int(raw, 273).ok_or_else(|| corrupt("3FR data offset"))?;
    let count = t.int(raw, 279).ok_or_else(|| corrupt("3FR data size"))?;
    let notes = hasselblad::read(&t);
    let fff = ifd0.has(PHOCUS);
    let mut info = crate::info::base(variant);
    info.make = t.text_upto(ifd0, 271, 64).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, 64).unwrap_or_default();
    // The model without its maker, and without what a "/" adds (the shutter
    // or the flash sync).
    crate::names::apply(&mut info);
    let model = info
        .model
        .split('/')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    info.model = notes
        .megapixels()
        .and_then(|mp| crate::facts::name(&info.make, &format!("{model}-{mp}")))
        .map_or(model, str::to_string);
    crate::names::apply(&mut info);
    info.canonical = notes
        .colour_name(&info.model, fff)
        .map(|name| [info.make.clone(), name]);
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
    // AsShotNeutral, three values (the shape checked before reading).
    if t.shape(ifd0, 50728).is_some_and(|(_, n)| n == 3)
        && let Some(n) = t.reals(ifd0, 50728, 3)
        && n.iter().all(|v| v.is_finite() && *v > 0.)
    {
        let g = n[1];
        info.cam_mul = [(g / n[0]) as f32, 1., (g / n[2]) as f32, 1.];
    }
    info.previews = t.previews();
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    // BlackLevel: one value (a short tag, its shape checked before reading).
    let black = t
        .shape(raw, 50714)
        .filter(|&(_, n)| n <= 4)
        .and_then(|_| t.real(raw, 50714))
        .filter(|b| b.is_finite() && *b >= 0.)
        .unwrap_or(0.);
    let file_white = t.int(raw, 50717).unwrap_or(0xffff);
    if black > f64::from(file_white) || file_white > 0xffff {
        return Err(corrupt("3FR levels"));
    }
    let white = adobe_white(file_white);
    info.plan = Plan::Fff(FffPlan {
        data: (t.base + u64::from(offset), u64::from(count)),
        order: if t.little() {
            Order::Little
        } else {
            Order::Big
        },
        width,
        height,
        black: black as u32,
        white,
    });
    Ok(info)
}

/// Adobe DNG Converter's white level for a file whose WhiteLevel is
/// `white`: 96% of it, to the nearest (our measurement: on every body of
/// the test dataset, Adobe's white fitted to RAWmakase's samples, and its
/// WhiteLevel where it keeps the samples as they are, 62914 and 31456).
fn adobe_white(white: u32) -> u32 {
    ((u64::from(white) * 96 + 50) / 100) as u32
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &FffPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let mut data = crate::io::zeroed(crate::io::samples(plan.width, plan.height, 1)?)?;
    let (at, size) = plan.data;
    // Truncated files keep what they have.
    let size = size.min((bytes.len() as u64).saturating_sub(at));
    let src = Bytes::new(bytes, plan.order).slice(at, size)?;
    match info.variant {
        Variant::FffUncompressed => {
            crate::codec::packed::words(src, plan.order, 0, &mut data, cancel)?
        }
        Variant::FffCompressed => {
            crate::codec::hasselblad::decode(src, plan.width as usize, &mut data, cancel)?
        }
        v => {
            return Err(Error::Unsupported(format!(
                "{} is not a 3FR variant",
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
    use crate::container::test_tiff::{E, ascii, dir, long, offsets, short, tiff};

    const LE: Order = Order::Little;

    /// A little-endian 3FR by `model` of 4 × 2 samples: the data `strip`
    /// (compression `compression`) in a SubIFD with black 256 and white
    /// 65535, an FFF when `fff`.
    fn fff(model: &str, compression: u16, strip: &[u8], fff: bool) -> Vec<u8> {
        let at = offsets(&[strip, &[]]);
        let raw = dir(
            &[
                long(LE, 256, &[4]),
                long(LE, 257, &[2]),
                short(LE, 258, &[16]),
                short(LE, 259, &[compression]),
                short(LE, 262, &[32803]),
                long(LE, 273, &[at[0]]),
                long(LE, 279, &[strip.len() as u32]),
                E(
                    50714,
                    5,
                    [256u32, 1].iter().flat_map(|v| v.to_le_bytes()).collect(),
                ),
                short(LE, 50717, &[65535]),
            ],
            at[1],
            LE,
        );
        let mut ifd0 = vec![
            ascii(271, "Hasselblad"),
            ascii(272, model),
            long(LE, 330, &[at[1]]),
        ];
        if fff {
            ifd0.push(E(PHOCUS, 1, vec![0; 8]));
        }
        tiff(b"II*\0", LE, ifd0, vec![], &[strip, &raw])
    }

    fn decode(file: &[u8]) -> Result<(RawInfo, Sensor)> {
        let info = crate::open(file)?;
        let sensor = crate::decode(&info, file, &AtomicBool::new(false))?;
        Ok((info, sensor))
    }

    /// A coded row pair: SOI, a DHT whose one-bit code 0 is length 0, an
    /// SOS of predictor 8, and zero bits: every sample 0x8000.
    fn coded() -> Vec<u8> {
        let mut s = vec![0xff, 0xd8, 0xff, 0xc4, 0, 20, 0, 1];
        s.extend([0; 15]);
        s.push(0);
        s.extend([0xff, 0xda, 0, 8, 1, 0, 0, 8, 0, 0]);
        s.extend([0; 8]);
        s
    }

    #[test]
    fn reads_3fr_and_fff_data_and_levels() {
        let words: Vec<u8> = (1u16..=8).flat_map(|v| v.to_le_bytes()).collect();
        let (info, s) = decode(&fff("Hasselblad X1D", 1, &words, false)).unwrap();
        assert_eq!(info.variant, Variant::FffUncompressed);
        assert_eq!(s.data, [1, 2, 3, 4, 5, 6, 7, 8]);
        // Black as the file says, white as Adobe DNG Converter takes it.
        assert_eq!((s.black, s.white), ([256; 4], 62914));
        // The body's name, and its colour's from the maker note (none here).
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Hasselblad", "X1D-50c")
        );
        let (info, s) = decode(&fff("Hasselblad X2D 100C", 7, &coded(), true)).unwrap();
        assert_eq!(info.variant, Variant::FffCompressed);
        assert_eq!(s.data, [0x8000; 8]);
        assert_eq!(adobe_white(32767), 31456);
    }

    #[test]
    fn other_data_is_unsupported_and_bad_files_do_not_panic() {
        let words = vec![0u8; 16];
        assert!(matches!(
            crate::open(&fff("Hasselblad X1D", 8, &words, false)),
            Err(Error::Unsupported(_))
        ));
        for file in [
            fff("Hasselblad X1D", 1, &words, false),
            fff("Hasselblad X1D", 7, &coded(), true),
        ] {
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
