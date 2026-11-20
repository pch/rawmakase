//! What Canon's maker notes in CR3 files say: the as-shot white balance and
//! the daylight and shade presets (ColorData, in the timed metadata), the
//! camera's aspect-ratio crop (AspectInfo), Highlight Tone Priority
//! (LightingOpt) and the lens.
//!
//! Sources: ExifTool's Canon tag documentation (ColorData9 to ColorData12,
//! AspectInfo, LightingOpt, LensModel), and our own measurements (ColorData
//! version 67, which ExifTool 13.55 does not decode, against Adobe DNG
//! Converter 18.7's AsShotNeutral, BlackLevel and CameraCalibration).
use crate::container::tiff::{Ifd, TiffFile};
use crate::info::Presets;

const ASPECT_INFO: u16 = 0x9a;
const COLOR_DATA: u16 = 0x4001;
const LIGHTING_OPT: u16 = 0x4018;

/// The ColorData values read: up to version 32's SpecularWhiteLevel, the
/// furthest (811).
const COLOR_DATA_USED: usize = 812;

/// Where a ColorData version keeps the as-shot, daylight and shade levels
/// (red, green, green, blue each), in 16-bit words.
fn color_data_layout(version: u32) -> Option<(usize, usize, usize)> {
    match version {
        16..=19 => Some((71, 136, 141)),
        32 | 33 => Some((85, 150, 155)),
        34 | 48 => Some((105, 205, 210)),
        // Version 64's arrays are longer and their presets earlier; versions
        // 65 and 66 keep version 48's layout, version 67 (EOS R8 Mark II)
        // version 64's (our measurement).
        64 | 67 => Some((105, 110, 115)),
        65 | 66 => Some((105, 205, 210)),
        _ => None,
    }
}

/// White balance as shot (red, green, blue, second green) and the presets,
/// from a maker note's ColorData.
pub(crate) fn white_balance(t: &TiffFile, ifd: &Ifd) -> Option<([f32; 4], Presets)> {
    let cd = t.ints_upto(ifd, COLOR_DATA, COLOR_DATA_USED)?;
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
    let cd = t.ints_upto(ifd, COLOR_DATA, COLOR_DATA_USED)?;
    let at = match *cd.first()? {
        16..=19 => 329,
        32 | 33 => 343,
        34 | 48 => 363,
        64..=67 => 383,
        _ => return None,
    };
    let v = cd.get(at..at + 4)?;
    // Red, green, green, blue.
    Some([v[0], v[1], v[3], v[2]]).filter(|v| v.iter().all(|&b| b < 0x10000))
}

/// ColorData's SpecularWhiteLevel: the level highlights clip at, which
/// Adobe DNG Converter's WhiteLevel follows for most cameras.
pub(crate) fn white(t: &TiffFile, ifd: &Ifd) -> Option<u32> {
    let cd = t.ints_upto(ifd, COLOR_DATA, COLOR_DATA_USED)?;
    let at = match *cd.first()? {
        16..=19 => 797,
        32 | 33 => 811,
        34 | 48 => 641,
        64..=67 => 661,
        _ => return None,
    };
    cd.get(at).copied()
}

/// The camera's aspect-ratio crop, `[left, top, width, height]` from the
/// image's crop corner.
pub(crate) fn aspect(t: &TiffFile, ifd: &Ifd) -> Option<[u32; 4]> {
    let a = t.ints_upto(ifd, ASPECT_INFO, 5)?;
    Some([*a.get(3)?, *a.get(4)?, *a.get(1)?, *a.get(2)?])
}

/// The crop mode AspectInfo's ratio code names (`canon::crop_mode`).
pub(crate) fn sensor_crop(t: &TiffFile, ifd: &Ifd) -> Option<f32> {
    super::canon::crop_mode(*t.ints_upto(ifd, ASPECT_INFO, 1)?.first()?)
}

/// Highlight Tone Priority: 0 off, 1 on, 2 enhanced.
pub(crate) fn highlight_tone_priority(t: &TiffFile, ifd: &Ifd) -> i32 {
    t.ints_upto(ifd, LIGHTING_OPT, 4)
        .and_then(|v| v.get(3).copied())
        .map_or(0, |v| v as i32)
}

#[cfg(test)]
mod tests {
    use crate::container::cr2::tests::{put_ifd, shorts};
    use crate::container::tiff::{Ifd, TiffFile};
    use crate::info::Presets;

    /// `read` applied to a maker note holding the ColorData record `cd`.
    fn with_color_data<R>(cd: &[u16], read: impl FnOnce(&TiffFile, &Ifd) -> R) -> R {
        let mut f = b"II*\0\0\0\0\0".to_vec();
        let ifd = put_ifd(
            &mut f,
            &[(super::COLOR_DATA, 3, cd.len() as u32, shorts(cd))],
            0,
        );
        f[4..8].copy_from_slice(&ifd.to_le_bytes());
        let t = TiffFile::parse(&f, 0).unwrap();
        read(&t, &t.ifds[0])
    }

    /// The black of a ColorData `version` whose ColorData11 place (word 363)
    /// holds `black11` and whose ColorData12 place (383) holds `black12`,
    /// red, green, green, blue.
    fn black(version: u16, black11: [u16; 4], black12: [u16; 4]) -> Option<[u32; 4]> {
        let mut cd = vec![0u16; super::COLOR_DATA_USED];
        cd[0] = version;
        cd[363..367].copy_from_slice(&black11);
        cd[383..387].copy_from_slice(&black12);
        with_color_data(&cd, super::black)
    }

    /// The EOS R6 Mark III (version 66) and R50 V (65) keep ColorData12's
    /// levels in a record as long as ColorData11; read where ColorData11
    /// has them, they are small wrong values (#517).
    #[test]
    fn reads_the_black_where_each_color_data_version_keeps_it() {
        let wrong = [0, 58, 113, 145];
        assert_eq!(black(66, wrong, [512; 4]), Some([512; 4]));
        assert_eq!(
            black(65, wrong, [2044, 2047, 2046, 2043]),
            Some([2044, 2047, 2043, 2046])
        );
        // ColorData11 itself (the R6 Mark II's version 48).
        assert_eq!(
            black(48, [511, 512, 513, 514], wrong),
            Some([511, 512, 514, 513])
        );
        assert_eq!(black(99, [511; 4], [512; 4]), None);
    }

    /// The EOS R8 Mark II's version 67 keeps version 64's white balance
    /// (as shot, then Daylight and Shade) and version 66's levels. Values
    /// from one of its files at ISO 25600, which Adobe DNG Converter reads
    /// as AsShotNeutral 0.620982 1 0.563566 and BlackLevel 2047 2048 2048
    /// 2048. Words 205 and 210, where versions 65 and 66 keep their presets,
    /// hold other entries of the white balance table.
    #[test]
    fn reads_color_data_version_67() {
        let mut cd = vec![0u16; super::COLOR_DATA_USED];
        cd[0] = 67;
        cd[105..110].copy_from_slice(&[1649, 1024, 1024, 1817, 4103]);
        cd[110..115].copy_from_slice(&[1931, 1024, 1024, 1533, 5200]);
        cd[115..120].copy_from_slice(&[2241, 1024, 1024, 1322, 7000]);
        cd[205..215].copy_from_slice(&[1827, 1024, 1024, 1686, 4273, 1824, 1024, 1024, 1683, 4273]);
        cd[383..387].copy_from_slice(&[2047, 2048, 2048, 2048]);
        cd[660..662].copy_from_slice(&[13535, 14888]);
        let (shot, presets) = with_color_data(&cd, super::white_balance).unwrap();
        assert_eq!(shot, [1649., 1024., 1817., 1024.]);
        assert_eq!(
            presets,
            Presets {
                daylight: [1931., 1024., 1533.],
                shade: [2241., 1024., 1322.],
                ..Presets::default()
            }
        );
        assert_eq!(
            with_color_data(&cd, super::black),
            Some([2047, 2048, 2048, 2048])
        );
        assert_eq!(with_color_data(&cd, super::white), Some(14888));
    }
}
