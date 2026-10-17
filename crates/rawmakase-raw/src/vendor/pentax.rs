//! Pentax maker notes (and Samsung's GX bodies', which are Pentax's): white
//! balance as shot and its presets, black level, the lens's type and the
//! table of the sensor data's Huffman codes. A PEF's note starts with `AOC\0`
//! and a byte order mark (its offsets from the TIFF header); newer PEFs'
//! and DNGs' (in DNGPrivateData) with `PENTAX \0` (`SAMSUNG\0` in Samsung's
//! DNGs) and a byte order mark (its offsets from the note's start).
//!
//! Sources: ExifTool's Pentax tag documentation (the notes' headers,
//! ISO 0x0014 and its codes, ImageAreaOffset 0x0038, RawImageSize 0x0039, LensType 0x003F, BlackPoint 0x0200, WhitePoint 0x0201,
//! WB_RGGBLevelsDaylight 0x020D, Shade 0x020E, FluorescentN 0x0212,
//! WBLevels 0x022D,
//! HuffmanTable 0x0220); our own measurements of which values RAWmakase has
//! always read (the lens ID as the type's two bytes as one number).
use crate::container::tiff::{Ifd, TiffFile};
use crate::info::Presets;
use crate::io::Order;

/// ISO: a code for a nominal speed.
const ISO: u16 = 0x0014;
/// The nominal speeds of ISO codes 3 to 45, in thirds of a stop.
const ISO_SPEEDS: [u32; 43] = [
    50, 64, 80, 100, 125, 160, 200, 250, 320, 400, 500, 640, 800, 1000, 1250, 1600, 2000, 2500,
    3200, 4000, 5000, 6400, 8000, 10000, 12800, 16000, 20000, 25600, 32000, 40000, 51200, 64000,
    80000, 102400, 128000, 160000, 204800, 256000, 320000, 409600, 512000, 640000, 819200,
];
/// ImageAreaOffset and RawImageSize: the default crop's corner and size.
const AREA: [u16; 2] = [0x0038, 0x0039];
const LENS_TYPE: u16 = 0x003f;
const BLACK: u16 = 0x0200;
const WHITE_BALANCE: u16 = 0x0201;
/// The presets RAWmakase reads as daylight, shade and D65.
const PRESETS: [u16; 3] = [0x020d, 0x020e, 0x0212];
const HUFFMAN: u16 = 0x0220;
/// WBLevels: a count, then records of an ID byte and red, green, green and
/// blue levels; the IDs RAWmakase reads as daylight, shade and D65.
const WB_LEVELS: u16 = 0x022d;
const WB_LEVEL_IDS: [u8; 3] = [0, 1, 5];

/// What a Pentax maker note says.
#[derive(Clone, Debug, Default)]
pub(crate) struct Notes {
    /// As-shot multipliers: red, green, blue, second green.
    pub cam_mul: [f32; 4],
    pub presets: Presets,
    /// Black of red, green, blue and the second green.
    pub black: Option<[u32; 4]>,
    pub lens_id: u32,
    /// The ISO speed, when given.
    pub iso: Option<f32>,
    /// The default crop: left, top, width, height.
    pub crop: Option<[u32; 4]>,
    /// The HuffmanTable value and the note's byte order.
    pub huffman: Option<(Vec<u8>, Order)>,
}

/// The note at `at` (from the TIFF header) in `t`.
pub(crate) fn read_at(t: &TiffFile, at: u64) -> Option<Notes> {
    let (view, ifd) = directory(t, at)?;
    Some(notes(&view, &ifd))
}

/// The directory of the note at `at` (from the TIFF header) in `t`, and the
/// view its offsets count in.
fn directory<'a>(t: &TiffFile<'a>, at: u64) -> Option<(TiffFile<'a>, Ifd)> {
    let head = t.bytes.data.get(usize::try_from(at).ok()?..)?;
    let order = |mark: &[u8]| match mark {
        b"II" => Some(Order::Little),
        b"MM" => Some(Order::Big),
        _ => None,
    };
    let (view, ifd) = if head.starts_with(b"AOC\0") {
        let order = order(head.get(4..6)?).unwrap_or(t.bytes.order);
        let view = TiffFile::view(t.bytes.data, 0, order)?;
        let ifd = view.ifd_at(at + 6)?;
        (view, ifd)
    } else if head.starts_with(b"PENTAX \0") || head.starts_with(b"SAMSUNG\0") {
        let view = TiffFile::view(t.bytes.data, at, order(head.get(8..10)?)?)?;
        let ifd = view.ifd_at(10)?;
        (view, ifd)
    } else {
        return None;
    };
    Some((view, ifd))
}

/// The maker note of a Pentax (or Samsung GX) DNG, in its DNGPrivateData.
pub(crate) fn dng_notes(t: &TiffFile) -> Option<Notes> {
    let e = t.ifds.first()?.entries.get(&0xc634)?;
    read_at(t, t.offset_of(e)?)
}

/// The note the EXIF directory's MakerNote holds.
pub(crate) fn read(t: &TiffFile) -> Option<Notes> {
    let entry = t.exif.as_ref()?.entries.get(&0x927c)?;
    read_at(t, t.offset_of(entry)?)
}

fn notes(v: &TiffFile, ifd: &Ifd) -> Notes {
    let rggb = |tag| v.ints(ifd, tag).filter(|l| l.len() >= 4);
    let mut out = Notes::default();
    if let Some(l) = rggb(WHITE_BALANCE) {
        out.cam_mul = [l[0], l[1], l[3], l[2]].map(|x| x as f32);
    }
    let preset = |tag| {
        rggb(tag)
            .map(|l| [l[0] as f32, l[1] as f32, l[3] as f32])
            .unwrap_or_default()
    };
    let [daylight, shade, d65] = PRESETS;
    out.presets = Presets {
        daylight: preset(daylight),
        shade: preset(shade),
        d65: preset(d65),
        ..Presets::default()
    };
    if let Some(levels) = v.raw(ifd, WB_LEVELS) {
        let record = |id: u8| {
            levels
                .get(1..)?
                .as_chunks::<9>()
                .0
                .iter()
                .find(|r| r[0] == id)
                .map(|r| [1, 3, 7].map(|i| f32::from(v.u16(&r[i..i + 2]))))
        };
        let [daylight, shade, d65] = WB_LEVEL_IDS.map(|id| record(id).unwrap_or_default());
        out.presets = Presets {
            daylight,
            shade,
            d65,
            ..Presets::default()
        };
    }
    out.black = rggb(BLACK).map(|l| [l[0], l[1], l[3], l[2]]);
    if let Some(l) = v.ints(ifd, LENS_TYPE).filter(|l| l.len() >= 2) {
        out.lens_id = (l[0] & 0xff) << 8 | (l[1] & 0xff);
    }
    out.iso = v
        .int(ifd, ISO)
        .and_then(|c| ISO_SPEEDS.get((c as usize).checked_sub(3)?))
        .map(|&s| s as f32);
    if let [Some(origin), Some(size)] = AREA.map(|tag| v.ints(ifd, tag).filter(|l| l.len() >= 2)) {
        out.crop = Some([origin[0], origin[1], size[0], size[1]]);
    }
    out.huffman = v.raw(ifd, HUFFMAN).map(|b| (b, v.bytes.order));
    out
}
