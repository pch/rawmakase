//! Olympus and OM Digital Solutions maker notes: white balance as shot and
//! its presets, black level and bit depth, the default crop and aspect
//! ratio and the camera's colour matrix, from the ImageProcessing directory
//! (0x2040), the older compacts' RawInfo (0x3000) or the main directory of
//! older notes; the lens's name (Equipment, 0x2010) and the embedded JPEGs
//! (the main directory's thumbnail, CameraSettings' preview).
//!
//! A note starts with `OLYMPUS\0` and a byte order mark (its offsets from the
//! note's start), `OM SYSTEM\0\0\0` and a byte order mark (likewise), or
//! `OLYMP\0` (its offsets from the TIFF header, the subdirectories stored
//! inline as UNDEFINED values).
//!
//! Sources: ExifTool's Olympus tag documentation (the note's headers, the
//! ImageProcessing directory's WB_RBLevels 0x0100 (red, blue and the two
//! greens' levels) and their colour temperature presets 0x0102–0x010D,
//! ColorMatrix 0x0200, BlackLevel2 0x0600, ValidBits 0x0611,
//! CropLeft/Top/Width/Height 0x0612–0x0615, AspectRatio 0x1112; Equipment's
//! LensModel 0x0203; CameraSettings' PreviewImageStart and Length; RawInfo
//! 0x3000 and its white balance levels (WB_RBLevelsDaylightFluor 0x0130)
//! and ColorMatrix2 0x0200; the main directory's ColorMatrix 0x1011,
//! BlackLevel 0x1012, RedBalance 0x1017, BlueBalance 0x1018, ValidBits
//! 0x102C); our own measurements of which values RAWmakase has always read.
use crate::container::tiff::{Ifd, TEXT_MAX, TiffFile};
use crate::info::Presets;
use crate::io::Order;
use rawmakase_tiff::kind;

/// The CameraSettings, ImageProcessing and (older notes') RawInfo
/// directories.
const EQUIPMENT: u16 = 0x2010;
const CAMERA_SETTINGS: u16 = 0x2020;
const IMAGE_PROCESSING: u16 = 0x2040;
const RAW_INFO: u16 = 0x3000;
/// The main directory's thumbnail; CameraSettings' preview start and length.
const THUMBNAIL: u16 = 0x0100;
/// Equipment's lens name.
const LENS_MODEL: u16 = 0x0203;
const PREVIEW: [u16; 2] = [0x0101, 0x0102];
/// ImageProcessing's and RawInfo's white balance as shot, and the presets
/// RAWmakase has always read as fine weather, D65 and shade: ImageProcessing's
/// 5300 K, 6600 K and 7500 K levels, RawInfo's fine weather, daylight
/// fluorescent and shade levels.
const WB_LEVELS: u16 = 0x0100;
const PROCESSING_PRESETS: [u16; 3] = [0x010a, 0x010c, 0x010d];
const RAW_INFO_PRESETS: [u16; 3] = [0x0122, 0x0131, 0x0120];
/// The camera's own colour matrix: camera to its output RGB, of white
/// balanced data, nine signed 16-bit values (rows summing to about 256).
const COLOR_MATRIX: u16 = 0x0200;
/// The levels of the camera's own D65 white balance: ImageProcessing's 6600
/// K levels, RawInfo's daylight fluorescent (6500 K) ones.
const PROCESSING_DAYLIGHT: u16 = 0x010c;
const RAW_INFO_DAYLIGHT: u16 = 0x0130;
const BLACK: u16 = 0x0600;
const VALID_BITS: u16 = 0x0611;
/// BlackLevel of 14-bit samples (red, green, green, blue on their scale).
const BLACK_14: u16 = 0x0606;
const CROP: [u16; 4] = [0x0612, 0x0613, 0x0614, 0x0615];
const ASPECT_RATIO: u16 = 0x1112;
/// The main directory's tags of the oldest notes.
const OLD_COLOR_MATRIX: u16 = 0x1011;
const OLD_BLACK: u16 = 0x1012;
const OLD_RED: u16 = 0x1017;
const OLD_BLUE: u16 = 0x1018;
const OLD_VALID_BITS: u16 = 0x102c;

/// What an Olympus maker note says about the sensor data.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Notes {
    /// As-shot multipliers: red, green, blue, second green.
    pub cam_mul: [f32; 4],
    pub presets: Presets,
    /// Black of red, green, blue and the second green, on the scale of
    /// `bits`.
    pub black: Option<[u32; 4]>,
    /// Black of red, green, blue and the second green of 14-bit samples, on
    /// their scale.
    pub black14: Option<[u32; 4]>,
    /// The bit depth of the samples (12 or 14).
    pub bits: Option<u32>,
    /// The default crop: left, top, width, height.
    pub crop: Option<[u32; 4]>,
    /// The lens's name.
    pub lens: Option<String>,
    /// The camera's aspect ratio (across, down), when set.
    pub aspect: Option<[u32; 2]>,
    /// The embedded JPEGs: offset from the TIFF header and length.
    pub previews: Vec<(u64, u64)>,
    /// The camera's colour matrix, white balanced camera to sRGB, rows
    /// red, green, blue, as the note writes it (unscaled).
    pub color_matrix: Option<[[f64; 3]; 3]>,
    /// The red, green and blue levels of the camera's own D65 white
    /// balance, which the colour matrix is of.
    pub daylight: Option<[f32; 3]>,
}

/// The maker note's directories: the main one and ImageProcessing, and the
/// view their offsets count in.
struct Note<'a> {
    view: TiffFile<'a>,
    main: Ifd,
    /// ImageProcessing, or RawInfo, and which it is.
    processing: Option<(Ifd, bool)>,
    settings: Option<Ifd>,
    equipment: Option<Ifd>,
}

/// A subdirectory of the main one: its offset (an IFD or LONG value) in the
/// view, or (older notes) the directory itself inline.
fn sub(view: &TiffFile, main: &Ifd, tag: u16) -> Option<Ifd> {
    let e = main.entries.get(&tag)?;
    let at = match e.kind {
        kind::UNDEFINED => view.offset_of(e)?,
        _ => u64::from(*view.ints_upto(main, tag, 1)?.first()?),
    };
    view.ifd_at(at)
}

impl<'a> Note<'a> {
    fn new(view: TiffFile<'a>, main: Ifd) -> Self {
        let processing = sub(&view, &main, IMAGE_PROCESSING)
            .map(|d| (d, false))
            .or_else(|| sub(&view, &main, RAW_INFO).map(|d| (d, true)));
        let settings = sub(&view, &main, CAMERA_SETTINGS);
        let equipment = sub(&view, &main, EQUIPMENT);
        Self {
            view,
            main,
            processing,
            settings,
            equipment,
        }
    }
}

fn note<'a>(t: &TiffFile<'a>) -> Option<Note<'a>> {
    let entry = t.exif.as_ref()?.entries.get(&0x927c)?;
    let at = t.offset_of(entry)?;
    let head = t.bytes.data.get(usize::try_from(at).ok()?..)?;
    let order = |mark: &[u8]| match mark {
        b"II" => Some(Order::Little),
        b"MM" => Some(Order::Big),
        _ => None,
    };
    if let Some(start) = [&b"OLYMPUS\0"[..], b"OM SYSTEM\0\0\0"]
        .into_iter()
        .find_map(|magic| head.starts_with(magic).then_some(magic.len()))
    {
        let view = TiffFile::view(t.bytes.data, at, order(head.get(start..start + 2)?)?)?;
        let main = view.ifd_at(start as u64 + 4)?;
        return Some(Note::new(view, main));
    }
    if head.starts_with(b"OLYMP\0") {
        // Offsets from the TIFF header.
        let view = TiffFile::view(t.bytes.data, 0, t.bytes.order)?;
        let main = view.ifd_at(at + 8)?;
        return Some(Note::new(view, main));
    }
    None
}

/// The maker note's facts, `None` when the file has no Olympus note.
pub(crate) fn read(t: &TiffFile) -> Option<Notes> {
    let n = note(t)?;
    let v = &n.view;
    let mut out = Notes::default();
    let pair = |ifd: &Ifd, tag| {
        v.ints_upto(ifd, tag, 2)
            .filter(|l| l.len() >= 2)
            .map(|l| [l[0] as f32, 256., l[1] as f32])
    };
    let quad = |ifd: &Ifd, tag| {
        // Red, green, green, blue.
        v.ints_upto(ifd, tag, 4)
            .filter(|b| b.len() >= 4)
            .map(|b| [b[0], b[1], b[3], b[2]])
    };
    // The main directory's balance first, where both have one.
    let red = v.int(&n.main, OLD_RED);
    let blue = v.int(&n.main, OLD_BLUE);
    if let (Some(r), Some(b)) = (red, blue) {
        out.cam_mul = [r as f32, 256., b as f32, 256.];
    }
    let matrix = |ifd: &Ifd, tag| {
        let m = v.ints_upto(ifd, tag, 9).filter(|m| m.len() == 9)?;
        // SHORT or SSHORT: the low 16 bits, signed.
        Some(std::array::from_fn(|r| {
            std::array::from_fn(|c| f64::from(m[r * 3 + c] as u16 as i16))
        }))
    };
    out.color_matrix = matrix(&n.main, OLD_COLOR_MATRIX);
    out.black = quad(&n.main, OLD_BLACK);
    out.bits = v.int(&n.main, OLD_VALID_BITS);
    if let Some((ip, raw_info)) = &n.processing {
        if out.cam_mul == [0.; 4]
            && let Some(l) = v.ints_upto(ip, WB_LEVELS, 4).filter(|l| l.len() >= 2)
        {
            // Red and blue over the mean of the two greens' levels where
            // the tag has them (Adobe DNG Converter's AsShotNeutral), else
            // over 256.
            let green = match l[..] {
                [_, _, g1, g2, ..] => ((f64::from(g1) + f64::from(g2)) / 2.) as f32,
                _ => 256.,
            };
            out.cam_mul = [l[0] as f32, green, l[1] as f32, green];
        }
        let [fine, d65, shade] = if *raw_info {
            RAW_INFO_PRESETS
        } else {
            PROCESSING_PRESETS
        };
        out.presets = Presets {
            fine_weather: pair(ip, fine).unwrap_or_default(),
            d65: pair(ip, d65).unwrap_or_default(),
            shade: pair(ip, shade).unwrap_or_default(),
            ..Presets::default()
        };
        out.color_matrix = matrix(ip, COLOR_MATRIX).or(out.color_matrix);
        let daylight = if *raw_info {
            RAW_INFO_DAYLIGHT
        } else {
            PROCESSING_DAYLIGHT
        };
        out.daylight = pair(ip, daylight);
        out.black = quad(ip, BLACK).or(out.black);
        out.bits = v.int(ip, VALID_BITS).or(out.bits);
        // 14-bit samples' own black, as Adobe DNG Converter reads it.
        out.black14 = quad(ip, BLACK_14);
        let crop = CROP.map(|tag| v.int(ip, tag));
        if let [Some(l), Some(t), Some(w), Some(h)] = crop {
            out.crop = Some([l, t, w, h]);
        }
        // AspectRatio's first byte: 1 the sensor's 4:3, then 3:2, 16:9,
        // 6:6, 5:4, 7:6, 6:5, 7:5, 3:4.
        const RATIOS: [[u32; 2]; 9] = [
            [4, 3],
            [3, 2],
            [16, 9],
            [1, 1],
            [5, 4],
            [7, 6],
            [6, 5],
            [7, 5],
            [3, 4],
        ];
        out.aspect = v
            .int(ip, ASPECT_RATIO)
            .and_then(|r| RATIOS.get((r as usize).checked_sub(1)?))
            .copied();
    }
    out.lens = n
        .equipment
        .as_ref()
        .and_then(|e| v.text_upto(e, LENS_MODEL, TEXT_MAX))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty());
    let base = v.base;
    if let Some(e) = n.main.entries.get(&THUMBNAIL)
        && e.kind == kind::UNDEFINED
        && let Some(at) = v.offset_of(e)
    {
        out.previews.push((base + at, u64::from(e.count)));
    }
    if let Some(s) = &n.settings
        && let [Some(at), Some(len)] = PREVIEW.map(|tag| v.int(s, tag))
    {
        out.previews.push((base + u64::from(at), u64::from(len)));
    }
    Some(out)
}
