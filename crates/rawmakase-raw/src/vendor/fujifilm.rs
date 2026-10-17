//! Fujifilm (RAF): what the RAF header's directory, the raw image directory
//! and the maker notes in the embedded JPEG's EXIF mean. Tag names follow
//! ExifTool's FujiFilm tag documentation: header records 0x100
//! RawImageFullSize, 0x110 RawImageCropTopLeft, 0x111 RawImageCroppedSize,
//! 0x115 RawImageAspectRatio,
//! 0x120–0x121 the older sensors' image position and size, 0x130 FujiLayout,
//! 0x131 XTransLayout, 0x2100 / 0x2200 / 0x2301 white balance levels (GRGB),
//! 0x2FF0 WB_GRGBLevels, 0x9650 RawExposureBias, 0xC000 RAFData; raw image
//! directory tags 0xF001–0xF003 width, height and bits, 0xF005 the row stride
//! (32-bit words), 0xF006 the packing, 0xF007–0xF008 the data's offset and
//! length, 0xF009 the colour filter's phase, 0xF00A black levels, 0xF00C
//! the standard illuminants' white balance, 0xF00E as-shot white balance;
//! maker note 0x1403 DevelopmentDynamicRange.
//!
//! Where the tags leave a choice (which levels, which presets, where the
//! filter starts), the rules below are our own measurements of how RAWmakase
//! has always read these files, so saved edits keep their geometry and scale.
use crate::info::{Cfa, Presets};
use crate::io::{Bytes, Order};

pub(crate) const RAW_SIZE: u16 = 0x100;
pub(crate) const CROP_TOP_LEFT: u16 = 0x110;
pub(crate) const CROP_SIZE: u16 = 0x111;
pub(crate) const ASPECT_RATIO: u16 = 0x115;
pub(crate) const IMAGE_TOP_LEFT: u16 = 0x120;
pub(crate) const IMAGE_SIZE: u16 = 0x121;
pub(crate) const LAYOUT: u16 = 0x130;
pub(crate) const XTRANS_LAYOUT: u16 = 0x131;
const WB_DAYLIGHT: u16 = 0x2100;
const WB_CLOUDY: u16 = 0x2200;
const WB_DAY_WHITE_FLUORESCENT: u16 = 0x2301;
const WB_AS_SHOT: u16 = 0x2ff0;
const EXPOSURE_BIAS: u16 = 0x9650;
/// The older bodies' black levels (green, red, second green, blue; our own
/// measurement).
const BLACK: u16 = 0x4000;
/// Set in the older bodies whose white balance levels RAWmakase has always
/// read scaled (our own measurement).
const SCALED: u16 = 0x8200;
pub(crate) const RAF_DATA: u16 = 0xc000;

pub(crate) const RAW_WIDTH: u16 = 0xf001;
pub(crate) const RAW_HEIGHT: u16 = 0xf002;
pub(crate) const RAW_BITS: u16 = 0xf003;
pub(crate) const RAW_STRIDE: u16 = 0xf005;
pub(crate) const RAW_PACKING: u16 = 0xf006;
pub(crate) const RAW_OFFSET: u16 = 0xf007;
pub(crate) const RAW_LENGTH: u16 = 0xf008;
/// The phase of the Bayer filter: bit 0 shifts it a column, bit 1 a row.
pub(crate) const RAW_CFA_PHASE: u16 = 0xf009;
pub(crate) const RAW_BLACK: u16 = 0xf00a;
pub(crate) const RAW_WB_ILLUMINANTS: u16 = 0xf00c;
pub(crate) const RAW_WB_AS_SHOT: u16 = 0xf00e;

const DEVELOPMENT_DYNAMIC_RANGE: u16 = 0x1403;

/// The RAF header's directory: (tag, value) records, big-endian.
#[derive(Clone, Debug, Default)]
pub(crate) struct Records<'a>(Vec<(u16, &'a [u8])>);

impl<'a> Records<'a> {
    /// The records of the directory `dir`: a count, then per record its tag,
    /// its length and its bytes.
    pub fn parse(dir: &'a [u8]) -> Self {
        let b = Bytes::new(dir, Order::Big);
        let count = b.u32(0).unwrap_or(0).min(4096);
        let mut at = 4u64;
        let mut out = Vec::new();
        for _ in 0..count {
            let (Ok(tag), Ok(len)) = (b.u16(at), b.u16(at + 2)) else {
                break;
            };
            let Ok(value) = b.slice(at + 4, u64::from(len)) else {
                break;
            };
            out.push((tag, value));
            at += 4 + u64::from(len);
        }
        Self(out)
    }
    pub fn get(&self, tag: u16) -> Option<&'a [u8]> {
        self.0.iter().find(|r| r.0 == tag).map(|r| r.1)
    }
    /// A record of two 16-bit values.
    pub fn pair(&self, tag: u16) -> Option<[u32; 2]> {
        let v = self.get(tag).filter(|v| v.len() == 4)?;
        Some([
            u32::from(u16::from_be_bytes([v[0], v[1]])),
            u32::from(u16::from_be_bytes([v[2], v[3]])),
        ])
    }
}

/// The model the header's camera name (bytes 28 to 60) is.
pub(crate) fn camera_name(header: &[u8]) -> String {
    let name = header.get(28..60).unwrap_or_default();
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    String::from_utf8_lossy(&name[..end]).trim().to_string()
}

/// How the older bodies' Super CCD stores its pixels, rotated by 45°: in
/// rows of the tall image (FujiLayout 0x80 to 0x83) or of the wide one (0 to
/// 3). `None` for an ordinary sensor (Bayer 8 to 11, X-Trans 12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SuperCcd {
    Tall,
    Wide,
}

pub(crate) fn super_ccd(records: &Records) -> Option<SuperCcd> {
    let first = *records.get(LAYOUT)?.first()?;
    match first {
        0x80..=0x83 => Some(SuperCcd::Tall),
        0..=3 => Some(SuperCcd::Wide),
        _ => None,
    }
}

/// The colour filter at the stored data's corner: X-Trans from XTransLayout
/// (36 colours, 0 red, 1 green, 2 blue, last first), a Super CCD's fixed
/// diagonal patterns, else RGGB moved by the filter's phase (the raw image
/// directory's 0xF009, our own measurement; FujiLayout's colours are not the
/// ones RAWmakase has always read).
pub(crate) fn cfa(records: &Records, phase: u32) -> Cfa {
    if let Some(x) = records.get(XTRANS_LAYOUT)
        && x.len() == 36
        && x.iter().all(|&c| c < 3)
    {
        return Cfa {
            rows: 6,
            cols: 6,
            colors: x.iter().rev().copied().collect(),
        };
    }
    let (rows, cols, colors) = match super_ccd(records) {
        // Our own measurements: RB/Gg/BR/gG and RGBg/BgRG from the corner of
        // the visible area.
        Some(SuperCcd::Tall) => (4, 2, vec![0, 2, 1, 3, 2, 0, 3, 1]),
        Some(SuperCcd::Wide) => (2, 4, vec![0, 1, 2, 3, 2, 3, 0, 1]),
        None => {
            let cfa = Cfa {
                rows: 2,
                cols: 2,
                colors: vec![0, 1, 3, 2],
            };
            return cfa.shifted((phase as usize >> 1) & 1, phase as usize & 1);
        }
    };
    Cfa { rows, cols, colors }
}

/// The older bodies' black level, every channel's: record 0x4000's first
/// green, the one Adobe DNG Converter's BlackLevel has for all (our own
/// measurements of its output on the dataset's S2Pro, S5000, S100FS and
/// S200EXR, whose other levels differ).
pub(crate) fn black(records: &Records) -> Option<[u32; 4]> {
    let v = records.get(BLACK).filter(|v| v.len() >= 8)?;
    Some([u32::from(u16::from_be_bytes([v[0], v[1]])); 4])
}

/// RawExposureBias (record 0x9650): a signed numerator and a denominator.
pub(crate) fn exposure_shift(records: &Records) -> Option<f32> {
    let [n, d] = records.pair(EXPOSURE_BIAS)?;
    (d != 0).then(|| f32::from(n as u16 as i16) / d as f32)
}

/// The white balance multipliers (red, green, blue, green) of levels as
/// (green, red, blue); zero when a level is.
fn multipliers([g, r, b]: [u32; 3]) -> [f32; 4] {
    if g == 0 || r == 0 || b == 0 {
        return [0.; 4];
    }
    let g = g as f32;
    [r as f32 / g, 1., b as f32 / g, 1.]
}

/// A GRGB record's (green, red, blue).
fn grgb(records: &Records, tag: u16) -> Option<[u32; 3]> {
    let v = records.get(tag).filter(|v| v.len() >= 8)?;
    let w = |i: usize| u32::from(u16::from_be_bytes([v[2 * i], v[2 * i + 1]]));
    Some([w(0), w(1), w(3)])
}

/// The older bodies' as-shot white balance and presets with red and blue
/// scaled by `[red, blue]`, rounded down to whole levels where `floor`
/// (as RAWmakase has always read the factors observed from LibRaw's output),
/// else kept whole (Adobe DNG Converter's as-shot neutral); `None` unless
/// record 0x8200 is set (FinePix F700, S5000, S7000, S2Pro, the GX680 back),
/// whose levels are the ones scaled.
pub(crate) fn scaled_levels(
    records: &Records,
    [red, blue]: [f64; 2],
    floor: bool,
) -> Option<([f32; 4], Presets)> {
    if !records
        .get(SCALED)
        .is_some_and(|v| v.iter().any(|&b| b != 0))
    {
        return None;
    }
    let round = |v: f64| if floor { v.floor() } else { v };
    let scaled = |tag| {
        grgb(records, tag).map(|[g, r, b]| {
            [
                f64::from(g),
                round(f64::from(r) * red),
                round(f64::from(b) * blue),
            ]
        })
    };
    let rgb = |[g, r, b]: [f64; 3]| [r as f32, g as f32, b as f32];
    let mut p = Presets::default();
    if let Some(v) = scaled(WB_DAYLIGHT) {
        p.fine_weather = rgb(v);
    }
    if let Some(v) = scaled(WB_CLOUDY) {
        p.shade = rgb(v);
    }
    if let Some(v) = scaled(WB_DAY_WHITE_FLUORESCENT) {
        p.d65 = rgb(v);
    }
    let wb = scaled(WB_AS_SHOT).map_or([0.; 4], |[g, r, b]| {
        if g > 0. && r > 0. && b > 0. {
            [(r / g) as f32, 1., (b / g) as f32, 1.]
        } else {
            [0.; 4]
        }
    });
    Some((wb, p))
}

/// As-shot white balance: the raw image directory's levels (green, red,
/// blue), else the header's GRGB record.
pub(crate) fn white_balance(raw: Option<[u32; 3]>, records: &Records) -> [f32; 4] {
    raw.or_else(|| grgb(records, WB_AS_SHOT))
        .map_or([0.; 4], multipliers)
}

/// The white balance presets: from the header's records in the older files
/// (daylight as fine weather, cloudy as shade, day white fluorescent as
/// D65); else D65 from the raw image directory's illuminants (EXIF light
/// source 21), and the presets RAFData lists. `as_shot` is the raw image
/// directory's (green, red, blue).
pub(crate) fn presets(
    records: &Records,
    illuminants: &[u32],
    raf_data: &[u8],
    as_shot: Option<[u32; 3]>,
) -> Presets {
    let rgb = |[g, r, b]: [u32; 3]| [r as f32, g as f32, b as f32];
    let mut p = Presets::default();
    if let Some(v) = grgb(records, WB_DAYLIGHT) {
        p.fine_weather = rgb(v);
    }
    if let Some(v) = grgb(records, WB_CLOUDY) {
        p.shade = rgb(v);
    }
    if let Some(v) = grgb(records, WB_DAY_WHITE_FLUORESCENT) {
        p.d65 = rgb(v);
    }
    // (green, red, blue, light source) per illuminant.
    for e in illuminants.as_chunks::<4>().0 {
        if e[3] == 21 {
            p.d65 = rgb([e[0], e[1], e[2]]);
        }
    }
    if let Some(as_shot) = as_shot {
        if let Some([fine, shade]) = raf_data_table(raf_data, as_shot) {
            p.fine_weather = rgb(fine);
            p.shade = rgb(shade);
        } else if let Some(listed) = raf_data_list(raf_data) {
            for (source, levels) in listed {
                match source {
                    1 => p.fine_weather = rgb(levels),
                    8 => p.shade = rgb(levels),
                    83 => p.d65 = rgb(levels),
                    _ => {}
                }
            }
        }
    }
    p
}

/// RAFData's white balance table (our own measurements): (green, red, blue)
/// little-endian 16-bit levels, one preset after another, found as the first
/// run of six of the as-shot green. Fine weather is the first and shade the
/// second, after the as-shot levels in the first X bodies' RAFData (whose
/// first 32-bit word is below 65536, the image width).
fn raf_data_table(data: &[u8], [green, ..]: [u32; 3]) -> Option<[[u32; 3]; 2]> {
    if green == 0 || green > 0xffff {
        return None;
    }
    let words: Vec<u32> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| u32::from(u16::from_le_bytes(*w)))
        .collect();
    let first = Bytes::new(data, Order::Little).u32(0).ok()?;
    let skip = if first < 0x10000 { 3 } else { 0 };
    const RUN: usize = 6;
    let at = words.windows(3 * RUN).position(|run| {
        run.as_chunks::<3>()
            .0
            .iter()
            .all(|t| t[0] == green && t[1] > 0 && t[2] > 0)
    })?;
    let t = words.get(at + skip..at + skip + 6)?;
    Some([[t[0], t[1], t[2]], [t[3], t[4], t[5]]])
}

/// The X-A bodies' RAFData list (our own measurements): from byte 512,
/// little-endian 32-bit (light source, colour temperature, red, green,
/// second green, blue) per preset, the first the camera's automatic levels
/// (light source and temperature 0), every green the same; red and blue are
/// on half green's scale. Returns (light source, (green, red, blue)) per
/// preset, red and blue doubled.
fn raf_data_list(data: &[u8]) -> Option<Vec<(u32, [u32; 3])>> {
    let b = Bytes::new(data, Order::Little);
    let entry = |i: u64| -> Option<[u32; 6]> {
        let at = 512 + 24 * i;
        let mut e = [0; 6];
        for (k, v) in e.iter_mut().enumerate() {
            *v = b.u32(at + 4 * k as u64).ok()?;
        }
        Some(e)
    };
    let auto = entry(0)?;
    let green = auto[3];
    if auto[..2] != [0, 0] || green == 0 || green > 0xffff {
        return None;
    }
    Some(
        (1..)
            .map_while(entry)
            .take_while(|e| e[3] == green)
            .map(|e| (e[0], [e[3], e[2].saturating_mul(2), e[5].saturating_mul(2)]))
            .collect(),
    )
}

/// The header directory of the RAF header Adobe DNG Converter keeps in a
/// DNG's private data ("RAF " record): the byte order, the length of the
/// header's bytes from its format version on, those bytes, the directory's
/// length and the directory (our own reading of the dataset's DNGs).
pub(crate) fn dng_directory(raf: &[u8]) -> Option<&[u8]> {
    let b = Bytes::new(raf, Order::Big);
    if raf.get(..2)? != b"MM" {
        return None;
    }
    let header = u64::from(b.u32(2).ok()?);
    let at = 6u64.checked_add(header)?;
    let len = u64::from(b.u32(at).ok()?);
    b.slice(at + 4, len).ok()
}

/// DevelopmentDynamicRange from the maker notes (EXIF tag 0x927C: "FUJIFILM",
/// then the little-endian offset of a directory whose offsets are from the
/// notes' start).
pub(crate) fn dynamic_range(notes: &[u8]) -> Option<u32> {
    if !notes.starts_with(b"FUJIFILM") {
        return None;
    }
    let b = Bytes::new(notes, Order::Little);
    let at = u64::from(b.u32(8).ok()?);
    let count = b.u16(at).ok()?;
    (0..u64::from(count)).find_map(|i| {
        let e = at + 2 + 12 * i;
        let tag = b.u16(e).ok()?;
        let kind = b.u16(e + 2).ok()?;
        (tag == DEVELOPMENT_DYNAMIC_RANGE && kind == rawmakase_tiff::kind::SHORT)
            .then(|| b.u16(e + 8).ok().map(u32::from))
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header directory of `records`.
    pub(crate) fn directory(records: &[(u16, &[u8])]) -> Vec<u8> {
        let mut d = (records.len() as u32).to_be_bytes().to_vec();
        for (tag, v) in records {
            d.extend(tag.to_be_bytes());
            d.extend((v.len() as u16).to_be_bytes());
            d.extend(*v);
        }
        d
    }

    #[test]
    fn reads_records_and_their_pairs() {
        let d = directory(&[(0x110, &[0, 16, 0, 24]), (0x9650, &[0xff, 0xb8, 0, 100])]);
        let r = Records::parse(&d);
        assert_eq!(r.pair(CROP_TOP_LEFT), Some([16, 24]));
        assert_eq!(exposure_shift(&r), Some(-0.72));
        assert_eq!(r.pair(CROP_SIZE), None);
        // A record running past the directory ends it.
        let mut cut = d.clone();
        cut.truncate(d.len() - 2);
        assert_eq!(Records::parse(&cut).pair(0x9650), None);
        assert!(Records::parse(&[0xff; 3]).0.is_empty());
    }

    #[test]
    fn colour_filters_from_the_layouts() {
        let names = |c: &Cfa| -> String {
            (0..c.rows)
                .map(|r| {
                    (0..c.cols)
                        .map(|x| b"RGBg"[c.color(r, x) as usize] as char)
                        .collect()
                })
                .collect::<Vec<String>>()
                .join("/")
        };
        let d = directory(&[(0x130, &[0x0a, 0x0b, 0x09, 0x08])]);
        assert_eq!(names(&cfa(&Records::parse(&d), 0)), "RG/gB");
        // Phase 3: a row and a column on.
        assert_eq!(names(&cfa(&Records::parse(&d), 3)), "Bg/GR");
        assert_eq!(names(&cfa(&Records::parse(&d), 1)), "GR/Bg");
        let d = directory(&[(0x130, &[0x80, 0x81, 0x82, 0x83])]);
        assert_eq!(names(&cfa(&Records::parse(&d), 0)), "RB/Gg/BR/gG");
        // X-Trans: the layout's last colour first.
        let mut x = [1u8; 36];
        x[35] = 0;
        x[34] = 2;
        let d = directory(&[(0x130, &[12; 4]), (0x131, &x)]);
        let c = cfa(&Records::parse(&d), 0);
        assert!(c.is_xtrans());
        assert_eq!(&c.colors[..3], &[0, 2, 1]);
    }

    #[test]
    fn white_balance_and_presets() {
        let d = directory(&[
            (0x2ff0, &[0, 80, 0, 136, 0, 80, 0, 105]),
            (0x2100, &[0, 80, 0, 130, 0, 80, 0, 100]),
        ]);
        let r = Records::parse(&d);
        assert_eq!(white_balance(None, &r), [1.7, 1., 1.3125, 1.]);
        assert_eq!(white_balance(Some([256, 512, 384]), &r), [2., 1., 1.5, 1.]);
        // Scaled only when record 0x8200 is set: 136 · 0.9 = 122.4 → 122,
        // or 122.4 kept whole.
        assert!(scaled_levels(&r, [0.9, 0.5], true).is_none());
        let d = directory(&[
            (0x2ff0, &[0, 80, 0, 136, 0, 80, 0, 105]),
            (0x2100, &[0, 80, 0, 130, 0, 80, 0, 100]),
            (0x8200, &[0, 0x78, 0, 0x78]),
        ]);
        let (wb, p) = scaled_levels(&Records::parse(&d), [0.9, 0.5], true).unwrap();
        assert_eq!(wb, [122. / 80., 1., 52. / 80., 1.]);
        assert_eq!(p.fine_weather, [117., 80., 50.]);
        let (wb, p) = scaled_levels(&Records::parse(&d), [0.9, 0.5], false).unwrap();
        assert_eq!(wb, [(122.4 / 80.) as f32, 1., (52.5 / 80.) as f32, 1.]);
        assert_eq!(p.fine_weather, [117., 80., 50.]);
        let p = presets(&r, &[256, 300, 700, 17, 256, 500, 400, 21], &[], None);
        assert_eq!(p.fine_weather, [130., 80., 100.]);
        assert_eq!(p.d65, [500., 256., 400.]);
        // RAFData: six presets of the as-shot green in a row.
        let table: Vec<u8> = [0u16, 1000, 302, 555, 493, 302, 613, 428]
            .into_iter()
            .chain([302, 600, 500].repeat(4))
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let as_shot = Some([302, 600, 500]);
        let p = presets(&Records::default(), &[], &table, as_shot);
        assert_eq!(p.fine_weather, [555., 302., 493.]);
        assert_eq!(p.shade, [613., 302., 428.]);
        assert_eq!(
            presets(&Records::default(), &[], &table[..20], as_shot),
            Presets::default()
        );
        // The first X bodies' RAFData (a width first) lists the as-shot
        // levels before the presets.
        let mut first = 4952u32.to_le_bytes().to_vec();
        first.extend([302u16, 600, 500].into_iter().flat_map(|v| v.to_le_bytes()));
        first.extend(&table[4..]);
        let p = presets(&Records::default(), &[], &first, as_shot);
        assert_eq!(p.fine_weather, [555., 302., 493.]);
        // The X-A bodies' list: automatic, then fine weather (1), shade (8)
        // and D65 (83), red and blue on half green's scale.
        let mut list = vec![0u8; 512];
        for e in [
            [0u32, 0, 545, 512, 510, 371],
            [1, 0, 432, 512, 510, 445],
            [8, 0, 488, 512, 510, 384],
            [83, 0, 547, 512, 510, 381],
        ] {
            list.extend(e.iter().flat_map(|v| v.to_le_bytes()));
        }
        let p = presets(&Records::default(), &[], &list, Some([256, 545, 371]));
        assert_eq!(p.fine_weather, [864., 512., 890.]);
        assert_eq!(p.shade, [976., 512., 768.]);
        assert_eq!(p.d65, [1094., 512., 762.]);
        // Not a list: the first entry is not the automatic one.
        list[512] = 1;
        let p = presets(&Records::default(), &[], &list, Some([256, 545, 371]));
        assert_eq!(p, Presets::default());
    }

    #[test]
    fn dynamic_range_from_the_maker_notes() {
        let mut n = b"FUJIFILM".to_vec();
        n.extend(12u32.to_le_bytes());
        n.extend(1u16.to_le_bytes());
        n.extend(0x1403u16.to_le_bytes());
        n.extend(3u16.to_le_bytes());
        n.extend(1u32.to_le_bytes());
        n.extend(400u32.to_le_bytes());
        assert_eq!(dynamic_range(&n), Some(400));
        assert_eq!(dynamic_range(&n[..20]), None);
        assert_eq!(dynamic_range(b"FUJIFILM\xff\xff\xff\xff"), None);
    }
}
