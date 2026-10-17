//! Panasonic and Leica (RW2, RWL and the older RAW): what the tags of the
//! RW2 header directory mean. Tag names follow ExifTool's PanasonicRaw tag
//! documentation: 0x02–0x07 the sensor's size and borders, 0x09 the colour
//! filter pattern, 0x0A bits per sample, 0x0B compression, 0x0E–0x10 the
//! linearity limits, 0x11–0x12 red and blue balance, 0x13 and 0x27 the white
//! balance presets, 0x17 and 0x37 ISO, 0x1C–0x1E black levels, 0x24–0x26
//! white balance levels, 0x2D the raw format, 0x2E the embedded JPEG,
//! 0x2F–0x32 the camera's crop, 0x40–0x48 raw format 8's codes and strips,
//! 0x118 the raw data offset, 0x120 the camera directory (CameraIFD).
//!
//! Where the tags leave a choice (which crop, which levels), the rules below
//! are our own measurements of how RAWmakase has always read these files,
//! so saved edits keep their geometry and scale.
use crate::container::tiff::{Ifd, TiffFile};
use crate::info::{Cfa, Presets, UNSET};

pub(crate) const SENSOR_WIDTH: u16 = 0x02;
pub(crate) const SENSOR_HEIGHT: u16 = 0x03;
const BORDERS: [u16; 4] = [0x04, 0x05, 0x06, 0x07];
const CFA_PATTERN: u16 = 0x09;
pub(crate) const BITS: u16 = 0x0a;
pub(crate) const COMPRESSION: u16 = 0x0b;
const LINEARITY_LIMITS: [u16; 3] = [0x0e, 0x0f, 0x10];
const RED_BALANCE: u16 = 0x11;
const BLUE_BALANCE: u16 = 0x12;
const ISO: u16 = 0x17;
const BLACK: [u16; 3] = [0x1c, 0x1d, 0x1e];
const WB_LEVELS: [u16; 3] = [0x24, 0x25, 0x26];
const WB_PRESETS: u16 = 0x27;
const WB_PRESETS_OLD: u16 = 0x13;
pub(crate) const RAW_FORMAT: u16 = 0x2d;
pub(crate) const JPEG: u16 = 0x2e;
const CROP: [u16; 4] = [0x2f, 0x30, 0x31, 0x32];
const ISO_LONG: u16 = 0x37;
pub(crate) const RAW_OFFSET: u16 = 0x118;

/// Whether `tag` is a single SHORT, and its value.
fn short(t: &TiffFile, ifd: &Ifd, tag: u16) -> Option<u32> {
    let e = ifd.entries.get(&tag)?;
    (e.kind == rawmakase_tiff::kind::SHORT)
        .then(|| t.int(ifd, tag))
        .flatten()
}

/// The sensor borders (tags 4 to 7: top, left, bottom, right) as `[left,
/// top, width, height]` in sensor coordinates.
pub(crate) fn borders(t: &TiffFile, ifd: &Ifd) -> Option<[u32; 4]> {
    let [top, left, bottom, right] = BORDERS.map(|tag| short(t, ifd, tag));
    let (top, left, bottom, right) = (top?, left?, bottom?, right?);
    (right > left && bottom > top).then(|| [left, top, right - left, bottom - top])
}

/// The default crop in sensor coordinates: the camera's crop (tags 0x2F to
/// 0x32, top, left, bottom, right) when the file has one, else the sensor
/// borders; starting on even rows and columns, keeping its far edges.
pub(crate) fn frame(t: &TiffFile, ifd: &Ifd) -> [u32; 4] {
    let rect = match CROP.map(|tag| t.int(ifd, tag)) {
        [Some(top), Some(left), Some(bottom), Some(right)] => Some([
            left,
            top,
            right.saturating_sub(left),
            bottom.saturating_sub(top),
        ]),
        _ => borders(t, ifd),
    };
    let Some([mut x, mut y, mut w, mut h]) = rect else {
        return [UNSET, UNSET, 0, 0];
    };
    if x % 2 == 1 {
        x = x.saturating_add(1);
        w = w.saturating_sub(1);
    }
    if y % 2 == 1 {
        y = y.saturating_add(1);
        h = h.saturating_sub(1);
    }
    [x, y, w, h]
}

/// The visible width: the columns up to the right border and as many again
/// as the left border is wide, within the stored width.
pub(crate) fn visible_width(raw_width: u32, borders: Option<[u32; 4]>) -> u32 {
    match borders {
        Some([left, _, width, _]) => left.saturating_mul(2).saturating_add(width).min(raw_width),
        None => raw_width,
    }
}

/// The colour filter at the stored data's corner (tag 9: 1 RGGB, 2 GRBG, 3
/// GBRG, 4 BGGR).
pub(crate) fn cfa(t: &TiffFile, ifd: &Ifd) -> Cfa {
    let colors = match t.int(ifd, CFA_PATTERN) {
        Some(2) => vec![1, 0, 2, 1],
        Some(3) => vec![1, 2, 0, 1],
        Some(4) => vec![2, 1, 1, 0],
        _ => vec![0, 1, 1, 2],
    };
    crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors,
    })
}

/// As-shot white balance multipliers (red, green, blue, green): the levels
/// of tags 0x24 to 0x26 relative to green, else the red and blue balance of
/// older files (tags 0x11 and 0x12, 256 for one).
pub(crate) fn white_balance(t: &TiffFile, ifd: &Ifd) -> [f32; 4] {
    if let [Some(r), Some(g), Some(b)] = WB_LEVELS.map(|tag| t.int(ifd, tag))
        && r > 0
        && g > 0
        && b > 0
    {
        let g = g as f32;
        return [r as f32 / g, 1., b as f32 / g, 1.];
    }
    if let (Some(r), Some(b)) = (t.int(ifd, RED_BALANCE), t.int(ifd, BLUE_BALANCE))
        && r > 0
        && b > 0
    {
        return [r as f32 / 256., 1., b as f32 / 256., 1.];
    }
    [0.; 4]
}

/// The white balance presets of tag 0x27: a count, then per preset its EXIF
/// light source and red, green and blue levels (16-bit little-endian); in
/// older files, tag 0x13: a count, then per preset its light source and red
/// and blue levels against a green of 256.
pub(crate) fn presets(t: &TiffFile, ifd: &Ifd) -> Presets {
    let mut presets = Presets::default();
    let (raw, green_given) = match t.bytes_of(ifd, WB_PRESETS) {
        Some(raw) => (raw, true),
        None => match t.bytes_of(ifd, WB_PRESETS_OLD) {
            Some(raw) => (raw, false),
            None => return presets,
        },
    };
    // The count's presets of four words (three in older files) are read;
    // the words past them are not.
    let listed = raw
        .first_chunk()
        .map_or(0, |&n| usize::from(u16::from_le_bytes(n)));
    let used = 2 + 2 * listed * if green_given { 4 } else { 3 };
    let words: Vec<u16> = raw[..raw.len().min(used)]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| u16::from_le_bytes(*w))
        .collect();
    let count = words.first().map_or(0, |&n| usize::from(n));
    let rest = words.get(1..).unwrap_or_default();
    let entries: Vec<[u16; 4]> = if green_given {
        rest.as_chunks::<4>().0.to_vec()
    } else {
        let (entries, _) = rest.as_chunks::<3>();
        entries.iter().map(|&[s, r, b]| [s, r, 256, b]).collect()
    };
    for &[source, r, g, b] in entries.iter().take(count) {
        let levels = [r, g, b].map(f32::from);
        match source {
            9 => presets.fine_weather = levels,
            11 => presets.shade = levels,
            _ => {}
        }
    }
    presets
}

/// The 35 mm equivalent focal length: tag 0x1203 of the camera directory
/// (tag 0x120, a TIFF of its own; ExifTool's CameraIFD) when the file has
/// it; else the RW2's own EXIF FocalLengthIn35mmFormat, read with the other
/// EXIF facts. The embedded JPEG's EXIF value follows the in-camera aspect
/// ratio crop and is not used.
pub(crate) fn focal_35mm(t: &TiffFile, ifd: &Ifd) -> Option<f32> {
    let at = ifd.entries.get(&CAMERA_IFD).and_then(|e| t.offset_of(e))?;
    let camera = TiffFile::parse(t.bytes.data, at)?;
    camera.int(&camera.ifds[0], 0x1203).map(|v| v as f32)
}

const CAMERA_IFD: u16 = 0x120;

/// ISO: tag 0x17, or the 32-bit tag 0x37 of files whose ISO does not fit.
pub(crate) fn iso(t: &TiffFile, ifd: &Ifd) -> Option<f32> {
    match t.int(ifd, ISO) {
        Some(iso) if iso > 0 && iso < 65535 => Some(iso as f32),
        _ => t.int(ifd, ISO_LONG).filter(|&v| v > 0).map(|v| v as f32),
    }
}

/// Raw format 8's strips and prefix codes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Strips {
    /// Per strip: where its bytes are, its left column and top row, its
    /// length in bits, width and height.
    pub offsets: Vec<u32>,
    pub lefts: Vec<u32>,
    pub tops: Vec<u32>,
    pub bits: Vec<u32>,
    pub widths: Vec<u32>,
    pub heights: Vec<u32>,
    /// (length, code) of the prefix code of each difference category.
    pub codes: Vec<(u32, u32)>,
}

/// A tag of 16-bit little-endian words: a count, then the values, of two
/// words each (low, high) when `wide`.
fn list(t: &TiffFile, ifd: &Ifd, tag: u16, wide: bool) -> Option<Vec<u32>> {
    let raw = t.bytes_of(ifd, tag)?;
    // The count's values are read; the words past them are not.
    let listed = usize::from(u16::from_le_bytes(*raw.first_chunk()?));
    let used = 2 + 2 * listed * if wide { 2 } else { 1 };
    let words: Vec<u32> = raw[..raw.len().min(used)]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| u32::from(u16::from_le_bytes(*w)))
        .collect();
    let (&count, rest) = words.split_first()?;
    let count = count as usize;
    if wide {
        let (pairs, _) = rest.as_chunks::<2>();
        (pairs.len() >= count).then(|| {
            pairs[..count]
                .iter()
                .map(|&[lo, hi]| lo | hi << 16)
                .collect()
        })
    } else {
        rest.get(..count).map(<[u32]>::to_vec)
    }
}

/// The strips of raw format 8 (tags 0x44 offsets, 0x45 left columns with
/// the top row in the high word, 0x46 lengths in bits, 0x47 widths and 0x48
/// heights, each a count then the values) and its prefix codes (tag 0x40:
/// the count, then length and code per category).
pub(crate) fn strips(t: &TiffFile, ifd: &Ifd) -> Option<Strips> {
    let places = list(t, ifd, 0x45, true)?;
    let codes = list(t, ifd, 0x40, true)?;
    Some(Strips {
        offsets: list(t, ifd, 0x44, true)?,
        lefts: places.iter().map(|p| p & 0xffff).collect(),
        tops: places.iter().map(|p| p >> 16).collect(),
        bits: list(t, ifd, 0x46, true)?,
        widths: list(t, ifd, 0x47, false)?,
        heights: list(t, ifd, 0x48, false)?,
        codes: codes.iter().map(|c| (c & 0xffff, c >> 16)).collect(),
    })
}

/// The levels: black per channel (red, green, blue, second green), white,
/// and the linearity limits as the file records them.
///
/// Black is tags 0x1C to 0x1E, 15 above them in raw formats 3 and 4 (the
/// delta-coded blocks). White is the bit depth's full range scaled by 256
/// over the green white balance level (tag 0x25), rounded half up; raw
/// formats 6 to 8 of fewer than 16 bits clip at the linearity limit
/// (green's), else 1/256 of the range below it. Both are Adobe DNG
/// Converter's WhiteLevel (exactly on all our files of 256 over a level
/// above 256, to within three levels on the linearity limits).
pub(crate) fn levels(
    t: &TiffFile,
    ifd: &Ifd,
    format: Option<u32>,
    bits: u32,
) -> ([u32; 4], u32, [u32; 4]) {
    let offset = if matches!(format, Some(3 | 4)) { 15 } else { 0 };
    let [r, g, b] = BLACK.map(|tag| t.int(ifd, tag).unwrap_or(0).saturating_add(offset));
    let full = (1u32 << bits.clamp(1, 16)) - 1;
    let [lr, lg, lb] = LINEARITY_LIMITS.map(|tag| t.int(ifd, tag).unwrap_or(0));
    let white = match format {
        Some(6..=8) if bits < 16 && lg > 0 && lg <= full => lg,
        Some(6..=8) if bits < 16 => full.saturating_sub(1 << bits.saturating_sub(8)),
        _ => match t.int(ifd, WB_LEVELS[1]) {
            Some(g) if g > 256 => {
                ((u64::from(full) * 512 + u64::from(g)) / (2 * u64::from(g))) as u32
            }
            _ => full,
        },
    };
    ([r, g, b, g], white, [lr, lg, lb, lg])
}
