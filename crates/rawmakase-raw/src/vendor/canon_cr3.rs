//! What Canon's maker notes in CR3 files say: the as-shot white balance and
//! the daylight and shade presets (ColorData, in the timed metadata), the
//! camera's aspect-ratio crop (AspectInfo), Highlight Tone Priority
//! (LightingOpt) and the lens.
//!
//! Sources: ExifTool's Canon tag documentation (ColorData9 to ColorData12,
//! AspectInfo, LightingOpt, LensModel), and our own measurements.
use crate::container::tiff::{Ifd, TiffFile};
use crate::info::Presets;

const ASPECT_INFO: u16 = 0x9a;
const COLOR_DATA: u16 = 0x4001;
const LIGHTING_OPT: u16 = 0x4018;

/// Where a ColorData version keeps the as-shot, daylight and shade levels
/// (red, green, green, blue each), in 16-bit words.
fn color_data_layout(version: u32) -> Option<(usize, usize, usize)> {
    match version {
        16..=19 => Some((71, 136, 141)),
        32 | 33 => Some((85, 150, 155)),
        34 | 48 => Some((105, 205, 210)),
        // Version 64's arrays are longer and their presets earlier; versions
        // 65 and 66 keep version 48's layout (our measurement).
        64 => Some((105, 110, 115)),
        65 | 66 => Some((105, 205, 210)),
        _ => None,
    }
}

/// White balance as shot (red, green, blue, second green) and the presets,
/// from a maker note's ColorData.
pub(crate) fn white_balance(t: &TiffFile, ifd: &Ifd) -> Option<([f32; 4], Presets)> {
    let cd = t.ints(ifd, COLOR_DATA)?;
    let (shot, daylight, shade) = color_data_layout(*cd.first()?)?;
    let rggb = |at: usize| -> Option<[f32; 4]> {
        let v = cd.get(at..at + 4)?;
        Some([v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32])
    };
    let [r, g, g2, b] = rggb(shot)?;
    let preset = |at| rggb(at).map_or([0.; 3], |[r, g, _, b]| [r, g, b]);
    let presets = Presets {
        daylight: preset(daylight),
        shade: preset(shade),
        ..Presets::default()
    };
    Some(([r, g, b, g2], presets))
}

/// ColorData's per-channel black levels (PerChannelBlackLevel), as red,
/// green, blue and second green.
pub(crate) fn black(t: &TiffFile, ifd: &Ifd) -> Option<[u32; 4]> {
    let cd = t.ints(ifd, COLOR_DATA)?;
    let at = match *cd.first()? {
        16..=19 => 329,
        32 | 33 => 343,
        34 | 48 => 363,
        64..=66 => 383,
        _ => return None,
    };
    let v = cd.get(at..at + 4)?;
    // Red, green, green, blue.
    Some([v[0], v[1], v[3], v[2]]).filter(|v| v.iter().all(|&b| b < 0x10000))
}

/// ColorData's SpecularWhiteLevel: the level highlights clip at, which
/// Adobe DNG Converter's WhiteLevel follows for most cameras.
pub(crate) fn white(t: &TiffFile, ifd: &Ifd) -> Option<u32> {
    let cd = t.ints(ifd, COLOR_DATA)?;
    let at = match *cd.first()? {
        16..=19 => 797,
        32 | 33 => 811,
        34 | 48 => 641,
        64..=66 => 661,
        _ => return None,
    };
    cd.get(at).copied()
}

/// The camera's aspect-ratio crop, `[left, top, width, height]` from the
/// image's crop corner.
pub(crate) fn aspect(t: &TiffFile, ifd: &Ifd) -> Option<[u32; 4]> {
    let a = t.ints(ifd, ASPECT_INFO)?;
    Some([*a.get(3)?, *a.get(4)?, *a.get(1)?, *a.get(2)?])
}

/// Highlight Tone Priority: 0 off, 1 on, 2 enhanced.
pub(crate) fn highlight_tone_priority(t: &TiffFile, ifd: &Ifd) -> i32 {
    t.ints(ifd, LIGHTING_OPT)
        .and_then(|v| v.get(3).copied())
        .map_or(0, |v| v as i32)
}
