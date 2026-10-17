//! Nikon NEF: TIFF files whose raw image is the full-size directory with a
//! CFA photometric interpretation, its data Huffman-coded (TIFF compression
//! 34713) or High Efficiency (JPEG XS); the maker note says how it is coded,
//! and gives the white balance, levels, lens and crop.
//!
//! Sources: TIFF 6 and TIFF/EP (directories, strips, CFA tags); ExifTool's
//! Nikon tag documentation (the maker note's tags); our own measurements of
//! LibRaw's output on the test dataset's files (which data sizes are not
//! Huffman-coded, the preview it picks).
use super::tiff::{Ifd, TiffFile};
use crate::codec::{nikon, nikon_he};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo, UNSET};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::nikon::{Curve, Notes};
use std::sync::atomic::{AtomicBool, Ordering};

/// Where a NEF's raw image is and how it is coded.
#[derive(Clone, Debug)]
pub(crate) struct NefPlan {
    /// The stored image's size and its samples' bits.
    pub width: u32,
    pub height: u32,
    pub bits: u32,
    /// The strips' (offset, length), in order.
    pub strips: Vec<(u64, u64)>,
    /// The file's byte order.
    pub little: bool,
    pub curve: Option<Curve>,
    pub black: [u32; 4],
    pub white: u32,
}

/// Whether `t` is a Nikon camera's own TIFF.
pub(crate) fn is_nef(t: &TiffFile) -> bool {
    t.text(&t.ifds[0], 271)
        .is_some_and(|m| m.trim().to_ascii_uppercase().starts_with("NIKON"))
}

/// Reads a NEF's header facts.
pub(crate) fn open(bytes: &[u8], t: TiffFile) -> Result<RawInfo> {
    let raw =
        raw_directory(&t).ok_or_else(|| Error::Unsupported("NEF without a raw image".into()))?;
    let ifd = &t.ifds[raw];
    let ifd0 = &t.ifds[0];
    let make = t.text(ifd0, 271).unwrap_or_default();
    let file_model = t.text(ifd0, 272).unwrap_or_default();
    let model = crate::names::model(&crate::names::make(&make), &make, &file_model);
    let width = t.int(ifd, 256).ok_or_else(|| corrupt("NEF width"))?;
    let height = t.int(ifd, 257).ok_or_else(|| corrupt("NEF height"))?;
    crate::io::samples(width, height, 1)?;
    let bits = t.int(ifd, 258).unwrap_or(12);
    if !(8..=16).contains(&bits) {
        return Err(Error::Unsupported(format!("NEF of {bits}-bit samples")));
    }
    let compression = t.int(ifd, 259).unwrap_or(1);
    let strips = strips(&t, ifd)?;
    let size: u64 = strips.iter().map(|s| s.1).sum();
    let notes = crate::vendor::nikon::read(&t, &model);
    let first = strips
        .first()
        .and_then(|&(at, _)| bytes.get(at as usize..(at as usize).checked_add(4)?));
    let pixels = u64::from(width) * u64::from(height);
    let rows = u64::from(height);
    let row = |b: u64| (u64::from(width) * b).div_ceil(8);
    let little = t.little();
    // Data the size of uncompressed samples, packed or not, is not
    // Huffman-coded, whatever the compression tag says.
    let uncompressed = size == pixels * 2
        || [12, 14]
            .into_iter()
            .any(|b| size == row(b) * rows || size == row(b).next_multiple_of(16) * rows)
        || size == u64::from(width.div_ceil(10)) * 16 * rows;
    let variant = if first == Some(&[0xff, 0x10, 0xff, 0x50][..]) {
        // JPEG XS's SOC and CAP: High Efficiency, whatever the body.
        Variant::NefHighEfficiency
    } else if compression == 34713 && size == pixels * 3 {
        return Err(Error::Unsupported("Nikon small RAW".into()));
    } else if compression == 34713 && !uncompressed {
        Variant::NefHuffman
    } else {
        return Err(Error::Unsupported(format!(
            "NEF of {bits}-bit samples in {size} bytes"
        )));
    };
    let mut info = crate::info::base(variant);
    info.make = make;
    info.model = file_model;
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
    let cfa = cfa(&t, ifd)?;
    info.cfa = Some(crate::vendor::nikon::cfa(&model).unwrap_or(cfa));
    if let Some(mul) = notes.cam_mul {
        info.cam_mul = mul;
    }
    info.presets = notes.presets;
    info.lens_id = notes.lens_id;
    let inside =
        |start: u32, len: u32, total: u32| start.checked_add(len).is_some_and(|e| e <= total);
    info.frame = match notes.crop {
        // Crops start on even rows and columns, keeping their far edges; a
        // crop beyond the image is none.
        Some([mut x, mut y, mut w, mut h])
            if w > 0 && h > 0 && inside(x, w, info.raw_width) && inside(y, h, info.raw_height) =>
        {
            if x % 2 == 1 {
                x += 1;
                w = w.saturating_sub(1);
            }
            if y % 2 == 1 {
                y += 1;
                h = h.saturating_sub(1);
            }
            [x, y, w, h]
        }
        _ => [UNSET, UNSET, 0, 0],
    };
    super::tiff_exif::read(&t, &mut info);
    if info.iso == 0. {
        info.iso = notes.iso.unwrap_or(0.);
    }
    // The directories give no preview sizes: the JPEGs' own frames do.
    info.previews = t.previews();
    info.previews.extend(notes.previews.iter().copied());
    for p in &mut info.previews {
        if p.pixels == 0 {
            p.pixels = jpeg_pixels(bytes, p.offset, p.len);
        }
    }
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    let (black, white) = levels(&info, &notes, bits);
    info.plan = Plan::Nef(NefPlan {
        width,
        height,
        bits,
        strips,
        little,
        curve: notes.curve.clone(),
        black,
        white,
    });
    Ok(info)
}

/// The raw image: the largest full-size directory holding CFA data.
fn raw_directory(t: &TiffFile) -> Option<usize> {
    (0..t.ifds.len())
        .filter(|&i| {
            let ifd = &t.ifds[i];
            t.int(ifd, 254).unwrap_or(0) & 1 == 0 && t.int(ifd, 262) == Some(32803) && ifd.has(273)
        })
        .max_by_key(|&i| {
            u64::from(t.int(&t.ifds[i], 256).unwrap_or(0))
                * u64::from(t.int(&t.ifds[i], 257).unwrap_or(0))
        })
}

/// The pixels of the JPEG at `at` from its frame header (ITU T.81 B.2.2);
/// 0 when it has none before the scan.
fn jpeg_pixels(bytes: &[u8], at: u64, len: u64) -> u64 {
    let Some(jpeg) = usize::try_from(at)
        .ok()
        .zip(usize::try_from(at.saturating_add(len)).ok())
        .and_then(|(start, end)| bytes.get(start..end.min(bytes.len())))
    else {
        return 0;
    };
    let mut i = 2;
    while let Some(&[0xff, marker, hi, lo]) = jpeg.get(i..i + 4) {
        let size = usize::from(u16::from_be_bytes([hi, lo]));
        match marker {
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                return jpeg.get(i + 5..i + 9).map_or(0, |d| {
                    u64::from(u16::from_be_bytes([d[0], d[1]]))
                        * u64::from(u16::from_be_bytes([d[2], d[3]]))
                });
            }
            0xda | 0xd9 => return 0,
            _ => i += 2 + size,
        }
    }
    0
}

/// The strips' offsets and lengths.
fn strips(t: &TiffFile, ifd: &Ifd) -> Result<Vec<(u64, u64)>> {
    let offsets = t
        .ints(ifd, 273)
        .ok_or_else(|| corrupt("NEF strip offsets"))?;
    let counts = t
        .ints(ifd, 279)
        .ok_or_else(|| corrupt("NEF strip counts"))?;
    if offsets.is_empty() || offsets.len() != counts.len() {
        return Err(corrupt("NEF strips"));
    }
    Ok(offsets
        .iter()
        .zip(&counts)
        .map(|(&o, &c)| (t.base + u64::from(o), u64::from(c)))
        .collect())
}

/// The 2 × 2 colour filter from CFAPattern (red, green and blue as 0, 1, 2);
/// the usual red-first Bayer pattern when the file has none.
fn cfa(t: &TiffFile, ifd: &Ifd) -> Result<Cfa> {
    let pattern = match t.raw(ifd, 33422) {
        Some(p) if p.len() >= 4 && p[..4].iter().all(|&c| c < 3) => p[..4].to_vec(),
        Some(_) => return Err(Error::Unsupported("NEF with a four-colour CFA".into())),
        None => vec![0, 1, 1, 2],
    };
    Ok(crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors: pattern,
    }))
}

/// The black and white levels the camera table gives for `info`'s files of
/// `bits`-bit samples.
fn table_levels(info: &RawInfo, bits: u32) -> (Option<u32>, Option<u32>) {
    let make = crate::names::make(&info.make);
    let model = crate::names::model(&make, &info.make, &info.model);
    let size = [info.raw_width, info.raw_height];
    let rows = || {
        crate::facts::camera(&make, &model, false, size)
            .filter(|c| c.bits.is_none_or(|b| b == bits))
    };
    (rows().find_map(|c| c.black), rows().find_map(|c| c.white))
}

/// The black (per channel) and white levels: the maker note's black level
/// at the data's bits, a Coolpix's own, or the camera table's.
fn levels(info: &RawInfo, notes: &Notes, bits: u32) -> ([u32; 4], u32) {
    let (table_black, table_white) = table_levels(info, bits);
    let white = table_white.unwrap_or((1 << bits.min(16)) - 1);
    let mut black = [0; 4];
    if let Some(b) = notes.black {
        black = b.map(|v| v >> 14u32.saturating_sub(bits));
    } else if let Some(b) = notes.nrw_black.or(table_black) {
        black = [b; 4];
    }
    (black, white)
}

/// Decodes the raw image.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &NefPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let (width, height) = (plan.width as usize, plan.height as usize);
    let file = Bytes::new(
        bytes,
        if plan.little {
            Order::Little
        } else {
            Order::Big
        },
    );
    // No coding takes more than four bytes a sample (a 16-bit code word and
    // a 14-bit difference); strips beyond that are not this image's.
    let budget = (width as u64 * height as u64).saturating_mul(4) + 65_536;
    let data = strip_data(&file, &plan.strips, budget, cancel)?;
    let (channels, samples) = match info.variant {
        Variant::NefHuffman => {
            let coding = match &plan.curve {
                Some(curve) => nikon::Coding {
                    version: curve.version,
                    predictors: curve.predictors,
                    size: curve.size,
                    points: &curve.points,
                    split: curve.split,
                    bits: plan.bits,
                },
                // Without a table: no curve, predictions of zero.
                None => nikon::Coding {
                    version: [0; 2],
                    predictors: [[0; 2]; 2],
                    size: 0,
                    points: &[],
                    split: 0,
                    bits: plan.bits,
                },
            };
            (1, nikon::decode(&data, width, height, &coding, cancel)?)
        }
        Variant::NefHighEfficiency => {
            let mut out = vec![0u16; crate::io::samples(plan.width, plan.height, 1)?];
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            nikon_he::decode(&data, &mut out, width, height, width)?;
            (1, out)
        }
        v => return Err(Error::Unsupported(format!("{} in a NEF", v.name()))),
    };
    let [left, top] = info.margins;
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels,
        data: samples,
        area: [left, top, info.width, info.height],
        black: plan.black,
        black_pattern: (0, 0, Vec::new()),
        white: plan.white,
        linear_white: [0; 4],
    })
}

/// The strips' bytes, one after another (a single strip is borrowed), at
/// most `budget` bytes.
fn strip_data<'a>(
    file: &Bytes<'a>,
    strips: &[(u64, u64)],
    budget: u64,
    cancel: &AtomicBool,
) -> Result<std::borrow::Cow<'a, [u8]>> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    if let [(at, len)] = strips {
        // Truncated files are common: read what there is.
        let len = (*len).min((file.data.len() as u64).saturating_sub(*at));
        return Ok(std::borrow::Cow::Borrowed(file.slice(*at, len)?));
    }
    let total = strips
        .iter()
        .try_fold(0u64, |sum, &(_, len)| sum.checked_add(len))
        .filter(|&total| total <= budget)
        .ok_or_else(|| corrupt("NEF strips larger than the image needs"))?;
    let mut out = Vec::new();
    out.try_reserve_exact(usize::try_from(total).map_err(|_| Error::TooLarge("NEF strips"))?)
        .map_err(|_| Error::TooLarge("NEF strips"))?;
    for (i, &(at, len)) in strips.iter().enumerate() {
        if i % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        out.extend_from_slice(file.slice(at, len)?);
    }
    Ok(std::borrow::Cow::Owned(out))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A little-endian TIFF directory at `at` with its long values after it.
    fn directory(entries: &[(u16, u16, Vec<u8>)], at: usize) -> Vec<u8> {
        let mut entries = entries.to_vec();
        entries.sort_by_key(|e| e.0);
        let mut extra = at + 2 + 12 * entries.len() + 4;
        let (mut head, mut values) = ((entries.len() as u16).to_le_bytes().to_vec(), Vec::new());
        for (tag, kind, value) in &entries {
            let size = match kind {
                3 => 2,
                4 => 4,
                5 => 8,
                _ => 1,
            };
            head.extend(tag.to_le_bytes());
            head.extend(kind.to_le_bytes());
            head.extend(((value.len() / size) as u32).to_le_bytes());
            if value.len() <= 4 {
                head.extend(value);
                head.resize(head.len() + 4 - value.len(), 0);
            } else {
                head.extend((extra as u32).to_le_bytes());
                values.extend(value);
                extra += value.len();
            }
        }
        head.extend(0u32.to_le_bytes());
        [head, values].concat()
    }

    fn short(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn long(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// A NEF of `width × height` samples of `bits` bits stored as `data`
    /// with TIFF `compression`, and a maker note of `note` tags.
    pub(crate) fn nef(
        model: &str,
        width: u32,
        height: u32,
        bits: u16,
        compression: u16,
        note: &[(u16, u16, Vec<u8>)],
        data: &[u8],
    ) -> Vec<u8> {
        let text = |s: &str| [s.as_bytes(), &[0]].concat();
        let note = [
            &b"Nikon\0\x02\x10\0\0II*\0\x08\0\0\0"[..],
            &directory(note, 8),
        ]
        .concat();
        // The header, IFD0, the raw directory, EXIF, the note, the data;
        // placed in two passes.
        let (mut raw_at, mut exif_at, mut data_at) = (0, 0, 0);
        let mut parts = Vec::new();
        for _ in 0..2 {
            let ifd0 = directory(
                &[
                    (271, 2, text("NIKON CORPORATION")),
                    (272, 2, text(model)),
                    (330, 4, long(&[raw_at as u32])),
                    (34665, 4, long(&[exif_at as u32])),
                ],
                8,
            );
            raw_at = 8 + ifd0.len();
            let raw = directory(
                &[
                    (254, 4, long(&[0])),
                    (256, 4, long(&[width])),
                    (257, 4, long(&[height])),
                    (258, 3, short(&[bits])),
                    (259, 3, short(&[compression])),
                    (262, 3, short(&[32803])),
                    (273, 4, long(&[data_at as u32])),
                    (279, 4, long(&[data.len() as u32])),
                    (33422, 1, vec![0, 1, 1, 2]),
                ],
                raw_at,
            );
            exif_at = raw_at + raw.len();
            let exif = directory(&[(0x927c, 7, note.clone())], exif_at);
            data_at = exif_at + exif.len();
            parts = vec![b"II*\0\x08\0\0\0".to_vec(), ifd0, raw, exif];
        }
        parts.push(data.to_vec());
        parts.concat()
    }

    /// The lossless 12-bit stream of the codec's test, 4 × 2 samples.
    fn lossless_stream() -> Vec<u8> {
        let bits = "0000000001111111110111110111111111111110111100000001111100000000";
        let mut bytes: Vec<u8> = bits
            .as_bytes()
            .chunks(8)
            .map(|b| b.iter().fold(0u8, |a, &c| a << 1 | u8::from(c == b'1')))
            .collect();
        // Not the size of any uncompressed layout.
        bytes.resize(33, 0);
        bytes
    }

    fn linearization(version: [u8; 2], predictor: u16) -> Vec<u8> {
        let mut t = version.to_vec();
        t.extend(short(&[predictor; 4]));
        t.extend(short(&[0]));
        t.resize(46, 0);
        t
    }

    fn huffman_nef() -> Vec<u8> {
        let note = [
            (0x000c, 5, long(&[600, 256, 400, 256, 256, 256, 256, 256])),
            (0x003d, 3, short(&[600, 600, 600, 600])),
            (0x0045, 3, short(&[1, 0, 3, 2])),
            (0x0093, 3, short(&[3])),
            (0x0096, 7, linearization([0x46, 0x30], 2048)),
        ];
        nef("NIKON Z 7", 4, 2, 12, 34713, &note, &lossless_stream())
    }

    #[test]
    fn opens_and_decodes_a_huffman_nef() {
        let file = huffman_nef();
        let info = crate::open(&file).unwrap();
        assert_eq!(info.variant, Variant::NefHuffman);
        assert_eq!((info.make.as_str(), info.model.as_str()), ("Nikon", "Z 7"));
        assert_eq!(info.cam_mul, [600. / 256., 1., 400. / 256., 1.]);
        // The crop starts on an even column, keeping its far edge.
        assert_eq!(info.frame, [2, 0, 2, 2]);
        let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.data, [2017, 2079, 2017, 3102, 2048, 2048, 2018, 2048]);
        // BlackLevel is at 14 bits; the data at 12.
        assert_eq!((s.black, s.white), ([150; 4], 4095));
    }

    #[test]
    fn strips_beyond_the_image_are_refused() {
        // 1024 strips of one 65,536-byte range for a 4 × 2 image: refused
        // before anything is copied, and a cancelled decode stops first.
        let mut file = huffman_nef();
        let at = file.len() as u64;
        file.resize(file.len() + 65_536, 0);
        let mut info = crate::open(&huffman_nef()).unwrap();
        let Plan::Nef(plan) = &mut info.plan else {
            panic!("not a NEF plan")
        };
        plan.strips = vec![(at, 65_536); 1024];
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            crate::decode(&info, &file, &cancel),
            Err(Error::Corrupt(_))
        ));
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            crate::decode(&info, &file, &cancel).unwrap_err(),
            Error::Cancelled
        );
    }

    #[test]
    fn crops_beyond_the_image_are_none() {
        for crop in [
            [u32::MAX, 0, 2, 2],
            [0, u32::MAX, 2, 2],
            [1, 0, u32::MAX, 2],
            [0, 1, 2, u32::MAX],
            [3, 0, 2, 2],
        ] {
            let c: Vec<u8> = crop
                .iter()
                .flat_map(|&v| (v.min(65535) as u16).to_le_bytes())
                .collect();
            let tag = if crop.contains(&u32::MAX) {
                // CropArea as LONGs, past what a SHORT holds.
                (0x0045, 4, long(&crop))
            } else {
                (0x0045, 3, c)
            };
            let note = [tag, (0x0096, 7, linearization([0x46, 0x30], 2048))];
            let file = nef("NIKON Z 7", 4, 2, 12, 34713, &note, &lossless_stream());
            let info = crate::open(&file).unwrap();
            assert_eq!(info.frame, [UNSET, UNSET, 0, 0], "{crop:?}");
        }
    }

    #[test]
    fn high_efficiency_data_is_recognised_by_its_codestream() {
        // A Z 9's, or a body LibRaw reads as Huffman-coded: the codestream's
        // start says what it is.
        for model in ["NIKON Z 9", "NIKON Z5_2"] {
            let stream = crate::codec::nikon_he::tests::blank_codestream(64, 64);
            let note = [(0x003d, 3, short(&[1008; 4]))];
            let file = nef(model, 64, 64, 14, 34713, &note, &stream);
            let info = crate::open(&file).unwrap();
            assert_eq!(info.variant, Variant::NefHighEfficiency, "{model}");
            let s = crate::decode(&info, &file, &AtomicBool::new(false)).unwrap();
            let value = crate::codec::nikon_he::tests::blank_value();
            assert!(s.data.iter().all(|&v| v == value), "{model}");
            assert_eq!((s.black, s.white), ([1008; 4], 16383));
        }
    }

    #[test]
    fn truncated_and_garbage_nefs_are_errors_not_panics() {
        let file = huffman_nef();
        for len in 0..file.len() {
            if let Ok(info) = crate::open(&file[..len]) {
                let _ = crate::decode(&info, &file[..len], &AtomicBool::new(false));
            }
        }
        let mut damaged = file.clone();
        for at in (8..file.len()).step_by(3) {
            damaged[at] ^= 0x5a;
            if let Ok(info) = crate::open(&damaged) {
                let _ = crate::decode(&info, &damaged, &AtomicBool::new(false));
            }
        }
    }
}
