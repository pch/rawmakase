//! Fujifilm RAF: a header ("FUJIFILMCCD-RAW", the format version, the camera
//! name at byte 28) whose big-endian pointers at byte 84 give the embedded
//! JPEG (with the EXIF and maker notes), the directory of header records
//! (`vendor::fujifilm`) and the raw data, and at byte 108 the compression.
//! Current bodies' raw data is a little-endian TIFF whose first directory
//! points (tag 0xF000) to the raw image directory; the older bodies' is the
//! samples themselves, their size in record 0x100.
//!
//! Sources: ExifTool's FujiFilm tag documentation, TIFF 6, Exif 2.32, and
//! our own measurements: sample packings (16-bit words, 12-bit samples least
//! significant bit first, or most significant first in little-endian 32-bit
//! words), the older layouts' byte order, rows and tiles, and the visible
//! areas, crops and levels RAWmakase has always read.
use super::tiff::{Ifd, TiffFile};
use crate::error::{Error, Result, corrupt};
use crate::info::{RawInfo, UNSET};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::fujifilm::{self as fuji, Records, SuperCcd};
use std::sync::atomic::{AtomicBool, Ordering};

/// How the sensor data is coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coding {
    /// 16-bit words, rows `stride` samples apart.
    Words { big: bool, stride: u32 },
    /// 16-bit big-endian words in vertical tiles `tile` columns wide, each
    /// stored whole, left to right.
    Tiles { tile: u32 },
    /// Samples of `bits` packed least significant bit first, rows `stride`
    /// bytes apart.
    Packed { bits: u32, stride: u32 },
    /// Samples of `bits` packed most significant bit first in little-endian
    /// 32-bit words, rows `stride` bytes apart.
    PackedWords { bits: u32, stride: u32 },
    /// Fujifilm's compressed stripes (lossless or lossy).
    Compressed { bits: u32 },
}

/// Where a RAF's sensor data is and how it is coded, and its levels.
#[derive(Clone, Debug)]
pub(crate) struct RafPlan {
    pub offset: u64,
    pub length: u64,
    pub coding: Coding,
    pub width: u32,
    pub height: u32,
    /// The samples' bit depth.
    pub bits: u32,
    pub black: [u32; 4],
    /// The black levels per filter position (rows, columns, values), at the
    /// stored data's corner.
    pub black_pattern: (u32, u32, Vec<u32>),
    pub white: u32,
}

/// Whether `bytes` is a RAF file.
pub(crate) fn is_raf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"FUJIFILMCCD-RAW ")
}

/// The header's pointer `i` (0 JPEG offset, 1 JPEG length, 2 records'
/// offset, 3 their length, 4 raw data offset, 5 its length).
fn pointer(header: &Bytes, i: u64) -> Result<u32> {
    header.u32(84 + 4 * i)
}

/// Where the TIFF of the JPEG's EXIF (APP1 "Exif") starts.
fn exif_at(bytes: &[u8], jpeg: u64, len: u64) -> Option<u64> {
    let b = Bytes::new(bytes, Order::Big);
    let end = jpeg.checked_add(len)?;
    if b.u16(jpeg).ok()? != 0xffd8 {
        return None;
    }
    let mut at = jpeg + 2;
    while at + 4 <= end {
        let marker = b.u16(at).ok()?;
        let size = u64::from(b.u16(at + 2).ok()?);
        if marker == 0xffe1 && b.slice(at + 4, 6).ok()? == b"Exif\0\0" {
            return Some(at + 10);
        }
        if marker & 0xff00 != 0xff00 || marker == 0xffda || size < 2 {
            return None;
        }
        at += 2 + size;
    }
    None
}

/// Reads a RAF's header facts.
pub(crate) fn open(bytes: &[u8]) -> Result<RawInfo> {
    let header = Bytes::new(bytes, Order::Big);
    let version = header.slice(16, 4)?;
    let (jpeg, jpeg_len) = (pointer(&header, 0)?, pointer(&header, 1)?);
    let (dir, dir_len) = (pointer(&header, 2)?, pointer(&header, 3)?);
    let (raw_at, raw_len) = (pointer(&header, 4)?, pointer(&header, 5)?);
    let records = Records::parse(header.slice(u64::from(dir), u64::from(dir_len))?);
    let mut info = crate::info::base(Variant::RafUnpacked);
    info.make = "Fujifilm".into();
    info.model = fuji::camera_name(bytes);

    // The embedded JPEG: the preview, and the EXIF with the exposure, lens,
    // orientation and maker notes.
    info.previews.push(crate::preview::Candidate {
        offset: u64::from(jpeg),
        len: u64::from(jpeg_len),
        pixels: 0,
    });
    let mut orientation = 1;
    if let Some(at) = exif_at(bytes, u64::from(jpeg), u64::from(jpeg_len))
        && let Some(t) = TiffFile::parse(bytes, at)
    {
        super::tiff_exif::read(&t, &mut info);
        orientation = t.int(&t.ifds[0], 274).unwrap_or(1);
        if let Some(dr) = t
            .exif
            .as_ref()
            .and_then(|e| t.bytes_of(e, 0x927c))
            .and_then(fuji::dynamic_range)
        {
            info.fuji_dynamic_range = dr;
        }
    }
    info.flip = crate::geometry::flip(orientation);
    if let Some(shift) = fuji::exposure_shift(&records) {
        info.fuji_exposure_shift = shift;
    }
    // The camera table is keyed by RAWmakase's names.
    crate::names::apply(&mut info);

    let raw = header.slice(u64::from(raw_at), u64::from(raw_len))?;
    let mut plan = if raw.starts_with(b"II*\0") {
        tiff_plan(bytes, u64::from(raw_at), &header, &records, &mut info)?
    } else {
        older_plan(
            bytes,
            version,
            u64::from(raw_at),
            u64::from(raw_len),
            &header,
            &records,
            &mut info,
        )?
    };
    info.raw_width = plan.width;
    info.raw_height = plan.height;
    let super_ccd = fuji::super_ccd(&records);

    // The visible area: the whole stored data, a tall Super CCD's image
    // records, or the camera table's.
    info.width = plan.width;
    info.height = plan.height;
    if super_ccd == Some(SuperCcd::Tall)
        && let (Some([top, left]), Some([_, width])) = (
            records.pair(fuji::IMAGE_TOP_LEFT),
            records.pair(fuji::IMAGE_SIZE),
        )
    {
        // In stored rows: columns half the image's, rows twice.
        let (left, top) = (left / 2, top * 2);
        if left + width / 2 <= plan.width && 2 * top < plan.height {
            info.margins = [left, top];
            info.width = width / 2;
            info.height = plan.height - 2 * top;
        }
    }
    let (make, model) = (info.make.clone(), info.model.clone());
    let rows = || crate::facts::camera(&make, &model, false, [plan.width, plan.height]);
    if let Some([left, top, w, h]) = rows().find_map(|c| c.visible)
        && left.checked_add(w).is_some_and(|r| r <= plan.width)
        && top.checked_add(h).is_some_and(|b| b <= plan.height)
    {
        info.margins = [left, top];
        info.width = w;
        info.height = h;
    }
    if let Some(layout) = super_ccd {
        info.super_ccd = Some(crate::info::SuperCcd {
            stored: [info.width, info.height],
            wide: layout == SuperCcd::Wide,
        });
    }
    let cfa = info.cfa.take();
    info.cfa = cfa.map(|c| c.shifted(info.margins[1] as usize, info.margins[0] as usize));

    // The camera's crop: the recommended one as the header gives it, and the
    // default crop from it, starting on the filter's period (not a Super
    // CCD's, whose crop is of its turned image).
    if let (Some([top, left]), Some([height, width])) = (
        records.pair(fuji::CROP_TOP_LEFT),
        records.pair(fuji::CROP_SIZE),
    ) && width > 0
        && height > 0
    {
        info.recommended_crop = Some([left, top, width, height]);
        let period = info.cfa.as_ref().map_or(1, |c| c.rows.max(c.cols) as u32);
        let period = if super_ccd.is_some() { 1 } else { period };
        let up = |v: u32| v.div_ceil(period) * period;
        let (l, t) = (up(left), up(top));
        info.frame = [
            l,
            t,
            width.saturating_sub(l - left),
            height.saturating_sub(t - top),
        ];
    }
    // The camera's aspect ratio (height to width) where it is wider than the
    // default crop by more than 1%: the crop's height cut, centred.
    if let Some([a, b]) = records.pair(fuji::ASPECT_RATIO)
        && let [left, top, width, height] = info.frame
        && left != UNSET
        && a > 0
        && b > 0
    {
        let cut = u64::from(width) * u64::from(a) / u64::from(b);
        if cut * 100 < u64::from(height) * 99 {
            let cut = cut as u32;
            info.aspect = [left, top + (height - cut) / 2, width, cut];
        }
    }
    if let Some((scale, observed)) = rows().find_map(|c| {
        c.wb_scale
            .map(|s| (s, c.source == crate::facts::Source::CompatObserved))
    }) && let Some((wb, presets)) = fuji::scaled_levels(&records, scale, observed)
    {
        info.cam_mul = wb;
        info.presets = presets;
    }
    if let Some(flip) = rows().find_map(|c| c.flip) {
        info.flip = flip;
    }
    if let Some(frame) = rows().find_map(|c| c.frame) {
        info.frame = frame;
    }
    // 16-bit samples use their whole range.
    if let Some(w) = rows().find_map(|c| c.white)
        && plan.bits < 16
    {
        plan.white = w;
    }
    // A camera's black level replaces the file's, its pattern too.
    if let Some(b) = rows().find_map(|c| c.black) {
        plan.black = [b; 4];
        plan.black_pattern = (0, 0, Vec::new());
    }
    info.plan = Plan::Raf(plan);
    Ok(info)
}

/// The plan of a current body's raw data: a TIFF at `raw_at` whose first
/// directory points (0xF000) to the raw image directory.
fn tiff_plan(
    bytes: &[u8],
    raw_at: u64,
    header: &Bytes,
    records: &Records,
    info: &mut RawInfo,
) -> Result<RafPlan> {
    let t = TiffFile::parse(bytes, raw_at).ok_or_else(|| corrupt("RAF raw directory"))?;
    let at = t
        .int(&t.ifds[0], 0xf000)
        .ok_or_else(|| corrupt("RAF raw directory"))?;
    let mut tiff = rawmakase_tiff::Tiff::open(std::io::Cursor::new(bytes), raw_at)
        .ok_or_else(|| corrupt("RAF raw directory"))?;
    let ifd = Ifd {
        entries: tiff
            .ifd(u64::from(at))
            .ok_or_else(|| corrupt("RAF raw directory"))?,
    };
    let int = |tag| t.int(&ifd, tag);
    let width = int(fuji::RAW_WIDTH).ok_or_else(|| corrupt("RAF width"))?;
    let height = int(fuji::RAW_HEIGHT).ok_or_else(|| corrupt("RAF height"))?;
    let bits = int(fuji::RAW_BITS).unwrap_or(12);
    crate::io::samples(width, height, 1)?;
    if !(8..=16).contains(&bits) {
        return Err(Error::Unsupported(format!("RAF of {bits}-bit samples")));
    }
    let offset = raw_at + u64::from(int(fuji::RAW_OFFSET).ok_or_else(|| corrupt("RAF data"))?);
    let length = u64::from(int(fuji::RAW_LENGTH).ok_or_else(|| corrupt("RAF data"))?);
    let pixels = u64::from(width) * u64::from(height);
    let row_bytes = |bits: u32| {
        let packed = (u64::from(width) * u64::from(bits)).div_ceil(8);
        let stride = u64::from(int(fuji::RAW_STRIDE).unwrap_or(0)) * 4;
        u32::try_from(stride.max(packed)).map_err(|_| Error::TooLarge("RAF row"))
    };
    let compression = header.u32(108).unwrap_or(0);
    let (variant, coding) = if matches!(compression, 2 | 3) {
        (Variant::RafCompressed, Coding::Compressed { bits })
    } else if int(fuji::RAW_PACKING) == Some(1) {
        let stride = row_bytes(bits)?;
        (
            Variant::RafPackedWords,
            Coding::PackedWords { bits, stride },
        )
    } else if length >= pixels * 2 {
        (
            Variant::RafUnpacked,
            Coding::Words {
                big: false,
                stride: width,
            },
        )
    } else if bits == 12 {
        let stride = row_bytes(bits)?;
        (Variant::RafPacked, Coding::Packed { bits, stride })
    } else {
        return Err(Error::Unsupported(format!(
            "RAF of {bits}-bit samples in {length} bytes"
        )));
    };
    info.variant = variant;
    let phase = int(fuji::RAW_CFA_PHASE).filter(|&p| p < 4).unwrap_or(0);
    let cfa = fuji::cfa(records, phase);
    // Black per filter position, from the stored data's corner: of a 6 × 6
    // filter at most.
    let black = t.ints(&ifd, fuji::RAW_BLACK, 36).unwrap_or_default();
    if black.iter().any(|&b| b > 0xffff) {
        return Err(corrupt("RAF black level"));
    }
    let black_pattern = if black.len() == cfa.colors.len() && black.len() > 1 {
        (cfa.rows as u32, cfa.cols as u32, black)
    } else if let Some(&b) = black.first() {
        (cfa.rows as u32, cfa.cols as u32, vec![b; cfa.colors.len()])
    } else {
        (0, 0, Vec::new())
    };
    info.cfa = Some(crate::info::second_green(cfa));
    let as_shot = t
        .ints_upto(&ifd, fuji::RAW_WB_AS_SHOT, 3)
        .filter(|v| v.len() >= 3)
        .map(|v| [v[0], v[1], v[2]]);
    info.cam_mul = fuji::white_balance(as_shot, records);
    // Four values per illuminant, of a few dozen at most.
    let illuminants = t
        .ints(&ifd, fuji::RAW_WB_ILLUMINANTS, 4 * 64)
        .unwrap_or_default();
    info.presets = fuji::presets(
        records,
        &illuminants,
        records.get(fuji::RAF_DATA).unwrap_or_default(),
        as_shot,
    );
    Ok(RafPlan {
        offset,
        length,
        coding,
        width,
        height,
        bits,
        black: [0; 4],
        black_pattern,
        white: (1u32 << bits) - 1,
    })
}

/// The plan of an older body's raw data: 16-bit words at `raw_at`, the size
/// in record 0x100 (height, width). The first format version (0200) stores
/// them big-endian. A pointer at byte 112 shorter than the data gives the
/// bytes of a row's run: shorter than a row, the data is in vertical tiles
/// of that width; a whole row, and the file holds twice the data, a Super
/// CCD SR's rows hold their S pixels, then their R pixels, of which the S
/// are read.
#[allow(clippy::too_many_arguments)]
fn older_plan(
    bytes: &[u8],
    version: &[u8],
    raw_at: u64,
    raw_len: u64,
    header: &Bytes,
    records: &Records,
    info: &mut RawInfo,
) -> Result<RafPlan> {
    let [height, width] = records
        .pair(fuji::RAW_SIZE)
        .ok_or_else(|| corrupt("RAF raw size"))?;
    crate::io::samples(width, height, 1)?;
    let big = version == b"0200";
    let run = if big {
        0
    } else {
        header.u32(112).unwrap_or(0) / 2
    };
    let available = (bytes.len() as u64).saturating_sub(raw_at);
    let pixels = u64::from(width) * u64::from(height);
    let (variant, coding) = if run > 0 && run < width && width % run == 0 {
        (Variant::RafTiled, Coding::Tiles { tile: run })
    } else if run == width && available >= 4 * pixels {
        let stride = width.checked_mul(2).ok_or(Error::TooLarge("RAF row"))?;
        (Variant::RafSuperCcdSr, Coding::Words { big, stride })
    } else {
        (Variant::RafUnpacked, Coding::Words { big, stride: width })
    };
    info.variant = variant;
    info.cfa = Some(crate::info::second_green(fuji::cfa(records, 0)));
    info.cam_mul = fuji::white_balance(None, records);
    info.presets = fuji::presets(records, &[], &[], None);
    Ok(RafPlan {
        offset: raw_at,
        // A Super CCD SR's R pixels are not counted.
        length: if variant == Variant::RafSuperCcdSr {
            available
        } else {
            raw_len
        },
        coding,
        width,
        height,
        // The older bodies' samples are of 14 bits.
        bits: 14,
        black: fuji::black(records).unwrap_or_default(),
        black_pattern: (0, 0, Vec::new()),
        white: 0x3fff,
    })
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &RafPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let (w, h) = (plan.width as usize, plan.height as usize);
    let n = crate::io::samples(plan.width, plan.height, 1)?;
    let available = (bytes.len() as u64).saturating_sub(plan.offset);
    let src = Bytes::new(bytes, Order::Little).slice(plan.offset, plan.length.min(available))?;
    let cancelled = || cancel.load(Ordering::Relaxed);
    if cancelled() {
        return Err(Error::Cancelled);
    }
    // The data must hold every row before the image is allocated.
    if (src.len() as u64) < needed(plan.coding, plan.width, plan.height) {
        return Err(corrupt("RAF data ends early"));
    }
    // The codec checks its header and stripes before allocating its own.
    let mut data = match plan.coding {
        Coding::Compressed { .. } => Vec::new(),
        _ => crate::io::zeroed(n)?,
    };
    match plan.coding {
        Coding::Words { big, stride } => {
            let stride = stride as usize * 2;
            for (y, row) in data.chunks_exact_mut(w).enumerate() {
                if y % 64 == 0 && cancelled() {
                    return Err(Error::Cancelled);
                }
                let at = y * stride;
                let line = src
                    .get(at..at + 2 * w)
                    .ok_or_else(|| corrupt("RAF data ends early"))?;
                for (v, b) in row.iter_mut().zip(line.as_chunks::<2>().0) {
                    *v = if big {
                        u16::from_be_bytes(*b)
                    } else {
                        u16::from_le_bytes(*b)
                    };
                }
            }
        }
        Coding::Tiles { tile } => {
            let tile = tile as usize;
            for x0 in (0..w).step_by(tile) {
                if cancelled() {
                    return Err(Error::Cancelled);
                }
                let at = x0 * h * 2;
                let block = src
                    .get(at..at + tile * h * 2)
                    .ok_or_else(|| corrupt("RAF data ends early"))?;
                for (y, line) in block.chunks_exact(tile * 2).enumerate() {
                    if y % 256 == 0 && cancelled() {
                        return Err(Error::Cancelled);
                    }
                    for (x, b) in line.as_chunks::<2>().0.iter().enumerate() {
                        data[y * w + x0 + x] = u16::from_be_bytes(*b);
                    }
                }
            }
        }
        Coding::Packed { bits, stride } | Coding::PackedWords { bits, stride } => {
            let words = matches!(plan.coding, Coding::PackedWords { .. });
            let stride = stride as usize;
            let row_bytes = (w * bits as usize).div_ceil(8);
            // The word-swapped row, for the 32-bit word packing only.
            let mut line = Vec::new();
            if words {
                line.try_reserve_exact(row_bytes.next_multiple_of(4))
                    .map_err(|_| Error::TooLarge("RAF row"))?;
            }
            for (y, row) in data.chunks_exact_mut(w).enumerate() {
                if y % 64 == 0 && cancelled() {
                    return Err(Error::Cancelled);
                }
                let at = y * stride;
                let packed = src
                    .get(at..at + row_bytes)
                    .ok_or_else(|| corrupt("RAF data ends early"))?;
                if words {
                    // Each 32-bit word's bytes, most significant first.
                    line.clear();
                    let whole = src.get(at..at + packed.len().next_multiple_of(4));
                    let whole = whole.ok_or_else(|| corrupt("RAF data ends early"))?;
                    line.extend(
                        whole
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .flat_map(|c| [c[3], c[2], c[1], c[0]]),
                    );
                    let mut pump = crate::codec::bits::Msb::new(&line);
                    for v in row.iter_mut() {
                        *v = pump.get(bits) as u16;
                    }
                } else {
                    unpack_lsb(packed, bits, row);
                }
            }
        }
        Coding::Compressed { bits } => {
            let xtrans = info.cfa.as_ref().is_some_and(|c| c.is_xtrans());
            data = crate::codec::fuji_compressed::decode(
                src,
                plan.width,
                plan.height,
                bits,
                xtrans,
                cancel,
            )?;
        }
    }
    let [left, top] = info.margins;
    // The black pattern from the visible area's corner. The raw image
    // directory's starts at the camera's crop (as Adobe DNG Converter's
    // BlackLevel does from its active area, the crop: the GFX 100's rows of
    // 62 and 63 start at its crop's odd row).
    let (rows, cols, values) = &plan.black_pattern;
    let [ax, ay] = info
        .recommended_crop
        .filter(|_| info.super_ccd.is_none())
        .map_or([0, 0], |[l, t, _, _]| [i64::from(l), i64::from(t)]);
    let black_pattern = if *rows > 0 && *cols > 0 {
        let (r, c) = (*rows as usize, *cols as usize);
        let shifted = (0..r * c)
            .map(|i| {
                let y = ((i / c) as i64 + i64::from(top) - ay).rem_euclid(r as i64) as usize;
                let x = ((i % c) as i64 + i64::from(left) - ax).rem_euclid(c as i64) as usize;
                values.get(y * c + x).copied().unwrap_or(0)
            })
            .collect();
        (*rows, *cols, shifted)
    } else {
        (0, 0, Vec::new())
    };
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area: [left, top, info.width, info.height],
        black: plan.black,
        black_pattern,
        white: plan.white,
        linear_white: [0; 4],
    })
}

/// The bytes a coding's `width × height` samples take at least.
fn needed(coding: Coding, width: u32, height: u32) -> u64 {
    let (w, h) = (u64::from(width), u64::from(height));
    let rows = |stride: u64, last: u64| (h - 1) * stride + last;
    match coding {
        Coding::Words { stride, .. } => rows(2 * u64::from(stride), 2 * w),
        Coding::Tiles { .. } => 2 * w * h,
        Coding::Packed { bits, stride } => {
            rows(u64::from(stride), (w * u64::from(bits)).div_ceil(8))
        }
        Coding::PackedWords { bits, stride } => {
            rows(u64::from(stride), (w * u64::from(bits)).div_ceil(32) * 4)
        }
        // The codec checks its stripes before allocating.
        Coding::Compressed { .. } => 16,
    }
}

/// Samples of `bits` packed least significant bit first.
fn unpack_lsb(src: &[u8], bits: u32, out: &mut [u16]) {
    let mask = (1u32 << bits) - 1;
    let (mut acc, mut have, mut at) = (0u32, 0u32, 0usize);
    for v in out.iter_mut() {
        while have < bits {
            acc |= u32::from(src.get(at).copied().unwrap_or(0)) << have;
            at += 1;
            have += 8;
        }
        *v = (acc & mask) as u16;
        acc >>= bits;
        have -= bits;
    }
}

#[cfg(test)]
mod tests {
    use crate::info::UNSET;
    use std::sync::atomic::AtomicBool;

    /// A big-endian record directory.
    fn directory(records: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut d = (records.len() as u32).to_be_bytes().to_vec();
        for (tag, v) in records {
            d.extend(tag.to_be_bytes());
            d.extend((v.len() as u16).to_be_bytes());
            d.extend(v);
        }
        d
    }
    fn pair(a: u16, b: u16) -> Vec<u8> {
        [a.to_be_bytes(), b.to_be_bytes()].concat()
    }

    /// A RAF: header (version 0201, `name`, `compression` at byte 108), a
    /// JPEG, the records, then `raw` (a TIFF or older samples).
    fn raf(name: &str, records: &[(u16, Vec<u8>)], raw: &[u8], compression: u32) -> Vec<u8> {
        let jpeg = [0xff, 0xd8, 0xff, 0xd9];
        let dir = directory(records);
        let mut out = b"FUJIFILMCCD-RAW 0201FF000000".to_vec();
        out.extend(name.as_bytes());
        out.resize(160, 0);
        let (j, d) = (160u32, 160 + jpeg.len() as u32);
        let r = d + dir.len() as u32;
        let pointers = [
            j,
            jpeg.len() as u32,
            d,
            dir.len() as u32,
            r,
            raw.len() as u32,
        ];
        for (i, v) in pointers.into_iter().enumerate() {
            out[84 + 4 * i..88 + 4 * i].copy_from_slice(&v.to_be_bytes());
        }
        out[108..112].copy_from_slice(&compression.to_be_bytes());
        out.extend(jpeg);
        out.extend(dir);
        out.extend(raw);
        out
    }

    /// A raw block: a little-endian TIFF whose first directory points
    /// (0xF000) to a directory of LONG `entries`, then `data`.
    fn block(entries: &[(u16, Vec<u32>)], data: &[u8]) -> Vec<u8> {
        let mut entries = entries.to_vec();
        let n = entries.len() + 2;
        let ifd_at = 26u32;
        let values_at = ifd_at + 2 + 12 * n as u32 + 4;
        let extra: u32 = entries
            .iter()
            .filter(|e| e.1.len() > 1)
            .map(|e| 4 * e.1.len() as u32)
            .sum();
        let data_at = values_at + extra;
        entries.push((0xf007, vec![data_at]));
        entries.push((0xf008, vec![data.len() as u32]));
        entries.sort_by_key(|e| e.0);
        let mut b = b"II*\0\x08\0\0\0\x01\0\x00\xf0\x0d\0\x01\0\0\0".to_vec();
        b.extend(ifd_at.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend((n as u16).to_le_bytes());
        let mut values = Vec::new();
        for (tag, v) in &entries {
            b.extend(tag.to_le_bytes());
            b.extend(4u16.to_le_bytes());
            b.extend((v.len() as u32).to_le_bytes());
            if v.len() == 1 {
                b.extend(v[0].to_le_bytes());
            } else {
                b.extend((values_at + values.len() as u32).to_le_bytes());
                values.extend(v.iter().flat_map(|x| x.to_le_bytes()));
            }
        }
        b.extend(0u32.to_le_bytes());
        b.extend(values);
        b.extend(data);
        b
    }

    fn records(xtrans: bool) -> Vec<(u16, Vec<u8>)> {
        let mut r = vec![
            (0x110, pair(3, 2)),
            (0x111, pair(4, 6)),
            (0x9650, pair(0xffb8, 100)),
        ];
        if xtrans {
            let mut x = [1u8; 36];
            x[35] = 0;
            x[34] = 2;
            r.push((0x131, x.to_vec()));
        }
        r
    }

    /// The common raw image directory: `width` × `height` of `bits`.
    fn entries(width: u32, height: u32, bits: u32) -> Vec<(u16, Vec<u32>)> {
        vec![
            (0xf001, vec![width]),
            (0xf002, vec![height]),
            (0xf003, vec![bits]),
            (0xf00a, vec![64, 64, 65, 65]),
            (0xf00e, vec![300, 600, 450]),
            (0xf00c, vec![300, 330, 700, 17, 300, 610, 420, 21]),
        ]
    }

    fn decode(file: &[u8]) -> (crate::RawInfo, crate::Sensor) {
        let info = crate::open(file).unwrap();
        let s = crate::decode(&info, file, &AtomicBool::new(false)).unwrap();
        (info, s)
    }

    #[test]
    fn reads_the_header_facts_and_16_bit_words() {
        let samples: Vec<u16> = (0..8 * 6).map(|i| 1000 + i).collect();
        let data: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        let file = raf(
            "X-T9",
            &records(false),
            &block(&entries(8, 6, 14), &data),
            0,
        );
        let (info, s) = decode(&file);
        assert_eq!(info.variant, crate::Variant::RafUnpacked);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Fujifilm", "X-T9")
        );
        assert_eq!(
            [info.raw_width, info.raw_height, info.width, info.height],
            [8, 6, 8, 6]
        );
        // The recommended crop as given; the default crop from even rows.
        assert_eq!(info.recommended_crop, Some([2, 3, 6, 4]));
        assert_eq!(info.frame, [2, 4, 6, 3]);
        assert_eq!(info.aspect, [UNSET, UNSET, 0, 0]);
        assert_eq!(info.fuji_exposure_shift, -0.72);
        assert_eq!(info.cam_mul, [2., 1., 1.5, 1.]);
        assert_eq!(info.presets.d65, [610., 300., 420.]);
        assert_eq!(info.cfa.as_ref().unwrap().colors, vec![0, 1, 3, 2]);
        assert_eq!(info.previews[0].len, 4);
        assert_eq!(s.data, samples);
        assert_eq!(s.white, 16383);
        // The black pattern (64, 64 over 65, 65) starts at the crop's row 3:
        // from the visible area's row 0, its second row comes first.
        assert_eq!(s.black_pattern, (2, 2, vec![65, 65, 64, 64]));
        assert_eq!(s.area, [0, 0, 8, 6]);
    }

    #[test]
    fn reads_the_older_layouts() {
        // 4 × 2 little-endian words after the header, black from record
        // 0x4000 (green, red, second green, blue): its first green's.
        let samples = [1u16, 2, 3, 4, 5, 6, 7, 8];
        let data: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        let r = vec![
            (0x100, pair(2, 4)),
            (0x4000, [pair(20, 10), pair(21, 30)].concat()),
            (0x2ff0, [pair(80, 160), pair(80, 120)].concat()),
        ];
        let file = raf("FinePix S1000", &r, &data, 0);
        let (info, s) = decode(&file);
        assert_eq!(info.model, "S1000");
        assert_eq!(s.data, samples);
        assert_eq!(s.black, [20; 4]);
        assert_eq!(info.cam_mul, [2., 1., 1.5, 1.]);
        // Vertical tiles of 2 columns (the pointer at byte 112: 4 bytes),
        // big-endian, each stored whole.
        let tiles: Vec<u8> = [1u16, 2, 5, 6, 3, 4, 7, 8]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        let mut file = raf("DBP", &r, &tiles, 0);
        file[112..116].copy_from_slice(&4u32.to_be_bytes());
        let (info, s) = decode(&file);
        assert_eq!(info.variant, crate::Variant::RafTiled);
        assert_eq!(s.data, samples);
        // A tall Super CCD SR: rows of S then R pixels, twice the data
        // declared.
        let mut sr: Vec<u8> = Vec::new();
        for row in samples.chunks(4) {
            sr.extend(row.iter().flat_map(|v| v.to_le_bytes()));
            sr.extend([0xffu8; 8]);
        }
        let mut r2 = r.clone();
        r2.push((0x120, pair(0, 2)));
        r2.push((0x121, pair(2, 6)));
        r2.push((0x130, vec![0x80, 0x81, 0x82, 0x83]));
        let mut file = raf("FinePix F700", &r2, &sr[..16], 0);
        file.extend(&sr[16..]);
        file[112..116].copy_from_slice(&8u32.to_be_bytes());
        let (info, s) = decode(&file);
        assert_eq!(info.variant, crate::Variant::RafSuperCcdSr);
        assert_eq!(s.data, samples);
        // Stored area: half the image's columns from half its offset; the
        // image turned by 45°: 2 + 3 / 2 square, less a row.
        assert_eq!(s.area, [1, 0, 3, 2]);
        assert_eq!(info.super_ccd.map(|s| s.size()), Some([3, 2]));
        assert_eq!(info.cfa.as_ref().map(|c| (c.rows, c.cols)), Some((4, 2)));
    }

    #[test]
    fn sizes_levels_and_crops_from_the_file_are_checked() {
        let cancel = AtomicBool::new(false);
        // 40000 × 40000 samples in 96 bytes: refused before allocating.
        let file = raf("FinePix S1", &[(0x100, pair(40000, 40000))], &[0; 96], 0);
        let info = crate::open(&file).unwrap();
        assert!(matches!(
            crate::decode(&info, &file, &cancel),
            Err(crate::Error::Corrupt(_))
        ));
        // A black level beyond 16 bits.
        let mut e = entries(8, 6, 14);
        e.retain(|e| e.0 != 0xf00a);
        e.push((0xf00a, vec![u32::MAX; 4]));
        let file = raf("X-T9", &records(false), &block(&e, &[0; 96]), 0);
        assert!(matches!(crate::open(&file), Err(crate::Error::Corrupt(_))));
        // The largest crop and aspect ratio records.
        let mut r = records(false);
        r.retain(|e| e.0 != 0x111);
        r.push((0x111, pair(u16::MAX, u16::MAX)));
        r.push((0x115, pair(u16::MAX, 1)));
        let file = raf("X-T9", &r, &block(&entries(8, 6, 14), &[0; 96]), 0);
        let info = crate::open(&file).unwrap();
        assert_eq!(info.aspect, [UNSET, UNSET, 0, 0]);
        r.retain(|e| e.0 != 0x115);
        r.push((0x115, pair(1, u16::MAX)));
        let file = raf("X-T9", &r, &block(&entries(8, 6, 14), &[0; 96]), 0);
        assert_eq!(crate::open(&file).unwrap().aspect[3], 1);
        // Cancelled before decoding.
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            crate::decode(&info, &file, &cancelled).unwrap_err(),
            crate::Error::Cancelled
        );
    }

    #[test]
    fn reads_packed_samples() {
        // 12-bit samples least significant bit first.
        let packed = [0x23, 0x61, 0x45, 0x89, 0xc7, 0xab];
        let file = raf(
            "X-A9",
            &records(false),
            &block(&entries(4, 1, 12), &packed),
            0,
        );
        let (info, s) = decode(&file);
        assert_eq!(info.variant, crate::Variant::RafPacked);
        assert_eq!(s.data, [0x123, 0x456, 0x789, 0xabc]);
        // 14-bit samples most significant first in little-endian words.
        let mut e = entries(2, 1, 14);
        e.push((0xf006, vec![1]));
        let word: u32 = (0x1234 << 18) | (0x2abc << 4);
        let file = raf("X-A9", &records(false), &block(&e, &word.to_le_bytes()), 0);
        let (info, s) = decode(&file);
        assert_eq!(info.variant, crate::Variant::RafPackedWords);
        assert_eq!(s.data, [0x1234, 0x2abc]);
        // A row stride of 4 GB on a one-row image: no scratch row of it.
        let mut e = entries(2, 1, 14);
        e.push((0xf006, vec![1]));
        e.push((0xf005, vec![0x3fff_ffff]));
        let file = raf("X-A9", &records(false), &block(&e, &word.to_le_bytes()), 0);
        let (_, s) = decode(&file);
        assert_eq!(s.data, [0x1234, 0x2abc]);
        // 40000 × 40000 packed samples in 8 bytes: refused before allocating.
        let cancel = AtomicBool::new(false);
        for packing in [0, 1] {
            let mut e = entries(40000, 40000, 12);
            e.push((0xf006, vec![packing]));
            let file = raf("X-A9", &records(false), &block(&e, &[0; 8]), 0);
            let info = crate::open(&file).unwrap();
            assert!(matches!(
                crate::decode(&info, &file, &cancel),
                Err(crate::Error::Corrupt(_))
            ));
        }
    }

    #[test]
    fn reads_compressed_x_trans_data() {
        use crate::codec::fuji_compressed::tests::{stream, zeros};
        let data = stream(14, true, 780, 1, &zeros(14, true, 1), None);
        let file = raf(
            "X-T9",
            &records(true),
            &block(&entries(780, 6, 14), &data),
            2,
        );
        let (info, s) = decode(&file);
        assert_eq!(info.variant, crate::Variant::RafCompressed);
        assert!(info.cfa.as_ref().unwrap().is_xtrans());
        // X-Trans: crops start on whole periods of 6.
        assert_eq!(info.frame, [6, 6, 2, 1]);
        assert_eq!(s.data, vec![0; 780 * 6]);
        assert_eq!(s.black_pattern.0, 6);
        // 40000 × 40000 compressed samples in 16 bytes: refused before the
        // image is allocated.
        let file = raf(
            "X-T9",
            &records(true),
            &block(&entries(40000, 40000, 14), &data[..16]),
            2,
        );
        let info = crate::open(&file).unwrap();
        assert!(matches!(
            crate::decode(&info, &file, &AtomicBool::new(false)),
            Err(crate::Error::Corrupt(_))
        ));
    }

    #[test]
    fn truncated_and_garbage_files_are_errors_not_panics() {
        let data: Vec<u8> = (0..96u8).collect();
        let mut e = entries(8, 6, 14);
        e.push((0xf006, vec![1]));
        let files = [
            raf("X-T9", &records(true), &block(&entries(8, 6, 14), &data), 0),
            raf("X-A9", &records(false), &block(&e, &data), 0),
            raf("FinePix S1", &[(0x100, pair(6, 8))], &data, 0),
        ];
        let cancel = AtomicBool::new(false);
        for file in &files {
            for len in 0..file.len() {
                if let Ok(info) = crate::open(&file[..len]) {
                    let _ = crate::decode(&info, &file[..len], &cancel);
                }
            }
            for at in 16..file.len() {
                let mut bad = file.clone();
                bad[at] ^= 0x5a;
                if let Ok(info) = crate::open(&bad) {
                    let _ = crate::decode(&info, &bad, &cancel);
                }
            }
        }
        // A width beyond the limits is refused before allocating.
        let file = raf(
            "X-T9",
            &records(false),
            &block(&entries(1 << 20, 6, 14), &data),
            0,
        );
        assert!(crate::open(&file).is_err());
    }
}
