//! Epson's maker notes (R-D1, R-D1s, R-D1x): `EPSON\0` and a version, then
//! a directory whose offsets count from the TIFF header, as older Olympus
//! notes. It gives the black levels, the image size the camera crops to and
//! the white balance as shot, and the embedded JPEG (PreviewImage, 0x0280).
//!
//! Sources: ExifTool's Olympus and Epson tag documentation (the note's
//! layout, 0x020B and 0x020C the image's width and height); our own
//! measurements: BlackLevel 0x0401 (red, green, green, blue) as Adobe DNG
//! Converter's BlackLevel, and the red and blue levels 48 bytes into 0x0E80
//! scaled as Adobe DNG Converter's AsShotNeutral on our files.
use crate::container::tiff::TiffFile;
use rawmakase_tiff::kind;

const IMAGE_SIZE: [u16; 2] = [0x020b, 0x020c];
const PREVIEW: u16 = 0x0280;
const BLACK: u16 = 0x0401;
const WB_BLOCK: u16 = 0x0e80;
/// The red and blue levels' scale to multipliers (our measurement against
/// Adobe DNG Converter's AsShotNeutral on three bodies' files).
const RED_SCALE: f64 = 567.0101 / 65536.;
const BLUE_SCALE: f64 = 430.9281 / 65536.;

/// What an Epson maker note says.
#[derive(Clone, Debug, Default)]
pub(crate) struct Notes {
    /// As-shot multipliers: red, green, blue, second green.
    pub cam_mul: [f32; 4],
    /// Black of red, green, blue and the second green.
    pub black: Option<[u32; 4]>,
    /// The image the camera keeps: width and height.
    pub size: Option<[u32; 2]>,
    /// The embedded JPEG: offset from the TIFF header and length.
    pub preview: Option<(u64, u64)>,
}

/// The note the EXIF directory's MakerNote holds.
pub(crate) fn read(t: &TiffFile) -> Option<Notes> {
    let entry = t.exif.as_ref()?.entries.get(&0x927c)?;
    let at = t.offset_of(entry)?;
    let head = t.bytes.data.get(usize::try_from(at).ok()?..)?;
    if !head.starts_with(b"EPSON\0") {
        return None;
    }
    let ifd = t.ifd_at(at + 8)?;
    let size = match IMAGE_SIZE.map(|tag| t.int(&ifd, tag)) {
        [Some(w), Some(h)] => Some([w, h]),
        _ => None,
    };
    let mut out = Notes {
        black: t
            .ints_upto(&ifd, BLACK, 4)
            .filter(|l| l.len() >= 4)
            .map(|l| [l[0], l[1], l[3], l[2]]),
        size,
        preview: ifd
            .entries
            .get(&PREVIEW)
            .and_then(|e| Some((t.offset_of(e)?, u64::from(e.count)))),
        ..Notes::default()
    };
    if let Some(block) = ifd
        .entries
        .get(&WB_BLOCK)
        .filter(|e| e.kind == kind::UNDEFINED)
        .and_then(|_| t.bytes_of(&ifd, WB_BLOCK))
        && let Some(levels) = block.get(48..52)
    {
        let red = f64::from(t.u16(&levels[..2])) * RED_SCALE;
        let blue = f64::from(t.u16(&levels[2..])) * BLUE_SCALE;
        if red > 0. && blue > 0. {
            out.cam_mul = [red as f32, 1., blue as f32, 1.];
        }
    }
    Some(out)
}
