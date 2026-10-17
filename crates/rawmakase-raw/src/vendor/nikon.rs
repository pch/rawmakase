//! Nikon maker notes: as-shot white balance and the presets, the black
//! level, the lens ID, the crop area, the NEF linearization table and the
//! embedded preview, and the decryption of the encrypted blocks.
//!
//! Sources: ExifTool's Nikon tag documentation (the tag numbers, and the
//! layouts of ColorBalanceA/B/C, LensData and the CropArea, BlackLevel,
//! SerialNumber and ShutterCount tags); our own measurements: LibRaw's output
//! on synthetic NEF files (the decryption keystream and its two 256-entry key
//! tables, which serial numbers and shutter counts select, and when lens data
//! is read) and on the test dataset's files (where each ColorBalance version
//! keeps the as-shot multipliers, which slots become presets), and Adobe DNG
//! Converter's output on them (where ColorBalance 0500 and 0502 keep the
//! black level).
//!
//! The maker note is a TIFF directory of its own: after "Nikon\0" and two
//! version bytes, a TIFF header whose offsets the note's tags use. The
//! earliest bodies (the D1 family) write a bare directory with the file's
//! offsets instead.
use crate::container::tiff::{Ifd, TEXT_MAX, TiffFile};
use crate::info::{Cfa, Presets};
use crate::preview::Candidate;

/// What the maker note says that reading the file needs.
#[derive(Clone, Debug, Default)]
pub(crate) struct Notes {
    /// As-shot multipliers: red, green, blue, second green.
    pub cam_mul: Option<[f32; 4]>,
    pub presets: Presets,
    /// BlackLevel (0x003d) per channel (red, green, blue, second green), at
    /// 14 bits.
    pub black: Option<[u32; 4]>,
    /// The black level a Coolpix's ColorBalanceC gives, at the data's bits.
    pub nrw_black: Option<u32>,
    /// The black levels (red, green, blue) a Coolpix's ColorBalance (0x0097)
    /// of version 0500 or 0502 gives, at 16 bits.
    pub cb_black: Option<[u32; 3]>,
    /// LensIDNumber; 0 when unknown.
    pub lens_id: u32,
    /// CropArea (0x0045): left, top, width, height.
    pub crop: Option<[u32; 4]>,
    /// NEFLinearizationTable (0x0096), or the D1X's in its 0x008c.
    pub curve: Option<Curve>,
    /// NEFCompression (0x0093).
    pub compression: Option<u32>,
    /// The PreviewIFD's JPEG.
    pub previews: Vec<Candidate>,
    /// Which Coolpix colour data (0x0014) the note has.
    pub coolpix: Coolpix,
    /// The ISO speed the note gives: ISO (0x0002), else ISOInfo's (0x0025).
    pub iso: Option<f32>,
}

/// A Coolpix's colour data, which also tells how its raw data is stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Coolpix {
    #[default]
    None,
    /// ColorBalanceA: the E-series.
    ColorBalanceA,
    /// ColorBalanceB or C of an NRW.
    Nrw,
}

/// The NEF linearization table: the compressed data's version, the first
/// predictions and the curve from decoded values to raw values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Curve {
    pub version: [u8; 2],
    /// The first value of each column of the first two rows, by row parity.
    pub predictors: [[u16; 2]; 2],
    /// The number of curve points, and the points stored.
    pub size: u16,
    pub points: Vec<u16>,
    /// The row from which the second Huffman table codes the data; 0 for none.
    pub split: u16,
}

/// The maker note of `t`'s EXIF directory, read.
pub(crate) fn read(t: &TiffFile, model: &str) -> Notes {
    match maker_note(t) {
        Some(note) => read_note(&note, note.base == t.base, model),
        None => Notes::default(),
    }
}

/// The maker note a DNG keeps in its DNGPrivateData at `at` in `file`:
/// Adobe's "MakN" record ("Adobe\0MakN", its length, the original byte
/// order and offset, then the note), read; `None` for another record or
/// maker. (DNG Specification 1.7, DNGPrivateData; Adobe's record layout
/// from our own reading of the dataset's DNGs.)
pub(crate) fn read_dng(file: &[u8], at: u64, model: &str) -> Option<Notes> {
    let head = file.get(usize::try_from(at).ok()?..)?;
    if !head.starts_with(b"Adobe\0MakN") {
        return None;
    }
    let note_at = at.checked_add(20)?;
    if !head.get(20..)?.starts_with(b"Nikon\0") {
        return None;
    }
    let note = TiffFile::parse(file, note_at.checked_add(10)?)?;
    Some(read_note(&note, false, model))
}

/// A maker note's facts; `headerless` for the D1 family's bare directory.
fn read_note(note: &TiffFile, headerless: bool, model: &str) -> Notes {
    let mut notes = Notes::default();
    let ifd = &note.ifds[0];
    let little = note.little();
    let u16_at = |b: &[u8], at: usize| -> Option<u16> {
        let v = b.get(at..at + 2)?;
        Some(if little {
            u16::from_le_bytes([v[0], v[1]])
        } else {
            u16::from_be_bytes([v[0], v[1]])
        })
    };
    // As shot: WB_RBLevels, except on the D1, whose levels are not
    // multipliers. Observed compatibility: LibRaw reads the D1's as unity, and
    // the file says nothing that tells the two apart, so the rule is the
    // model's.
    if headerless && model == "D1" {
        notes.cam_mul = Some([1.; 4]);
    } else if let Some(levels) = note.reals_upto(ifd, 0x000c, 2).filter(|l| l.len() >= 2)
        && levels[0] > 0.
        && levels[1] > 0.
    {
        notes.cam_mul = Some([levels[0] as f32, 1., levels[1] as f32, 1.]);
    }
    let keys = Keys::of(note, ifd, model);
    // The balance and its black are in the first 80 bytes (version 0100's
    // levels end there); the cipher's stream does not depend on the length.
    if let Some(cb) = note.raw_upto(ifd, 0x0097, 80) {
        let cb = match (cb.get(..2), keys) {
            (Some(b"02"), Some(k)) => k.decrypt(&cb),
            _ => cb,
        };
        if notes.cam_mul.is_none() {
            notes.cam_mul = color_balance(&cb, |b, at| u16_at(b, at));
        }
        notes.cb_black = color_balance_black(&cb, |b, at| u16_at(b, at));
    }
    if let Some(data) = note.bytes_of(ifd, 0x0014) {
        coolpix_color(data, &mut notes);
    }
    notes.black = note
        .ints_upto(ifd, 0x003d, 4)
        .filter(|b| b.len() >= 4)
        .map(|b| [b[0], b[1], b[3], b[2]]);
    // Lens data is read only alongside a shutter count (our own
    // measurements).
    if ifd.has(0x00a7)
        && let Some(lens) = note.raw_upto(ifd, 0x0098, LENS_DATA_USED)
    {
        notes.lens_id = lens_id(&lens, keys);
    }
    notes.crop = note
        .ints(ifd, 0x0045, 4)
        .filter(|c| c.len() == 4)
        .map(|c| [c[0], c[1], c[2], c[3]]);
    notes.compression = note.int(ifd, 0x0093);
    // ISOInfo's ISO byte is 100 × 2^(v / 12 − 5).
    let iso_info = note
        .raw_upto(ifd, 0x0025, 1)
        .and_then(|b| b.first().copied())
        .filter(|&v| v > 0)
        .map(|v| 100. * 2f32.powf(f32::from(v) / 12. - 5.));
    notes.iso = note
        .ints_upto(ifd, 0x0002, 2)
        .and_then(|v| v.get(1).copied())
        .filter(|&v| v > 0)
        .map(|v| v as f32)
        .or(iso_info);
    // The D1X writes its table in 0x008c, after 2110 bytes of other data.
    notes.curve = note
        .bytes_of(ifd, 0x0096)
        .and_then(|c| curve(c, little))
        .or_else(|| {
            note.bytes_of(ifd, 0x008c)
                .and_then(|c| curve(c, little))
                .filter(|c| c.version[0] == 0x49 || c.version[1] == 0x58)
        });
    if let Some(at) = note.int(ifd, 0x0011)
        && let Some(preview) = note.directory_at(u64::from(at))
    {
        let p = &preview.ifds[0];
        if let (Some(at), Some(len)) = (preview.int(p, 0x0201), preview.int(p, 0x0202)) {
            notes.previews.push(Candidate {
                offset: preview.base + u64::from(at),
                len: u64::from(len),
                pixels: 0,
            });
        }
    }
    notes
}

/// The colour filter a body's files are read with where its CFAPattern says
/// otherwise: the COOLPIX P6000's NRWs, read red first. Observed
/// compatibility: LibRaw's reading of the dataset's file, which saved edits
/// keep; nothing in the file says it, so the rule is the model's (the camera
/// table has no CFA facts).
pub(crate) fn cfa(model: &str) -> Option<Cfa> {
    (model == "COOLPIX P6000").then(|| {
        crate::info::second_green(Cfa {
            rows: 2,
            cols: 2,
            colors: vec![0, 1, 1, 2],
        })
    })
}

/// The maker note's directory, with the offsets its tags use.
fn maker_note<'a>(t: &TiffFile<'a>) -> Option<TiffFile<'a>> {
    let exif = t.exif.as_ref()?;
    let at = t.offset_of(exif.entries.get(&0x927c)?)?;
    let head = t.bytes.data.get(usize::try_from(at).ok()?..)?;
    if head.starts_with(b"Nikon\0") {
        // The note's own TIFF, its offsets from its header; a camera's own
        // TIFF starts the file.
        (t.base == 0)
            .then_some(t.bytes.data)
            .and_then(|file| TiffFile::parse(file, at.checked_add(10)?))
    } else {
        t.directory_at(at)
    }
}

/// The as-shot multipliers of an unencrypted (or decrypted) ColorBalance
/// block, by its version (our own measurements).
fn color_balance(cb: &[u8], u16_at: impl Fn(&[u8], usize) -> Option<u16>) -> Option<[f32; 4]> {
    let (at, order): (usize, [usize; 4]) = match cb.get(..4)? {
        // Red, blue, green, green.
        b"0100" => (72, [0, 2, 3, 1]),
        // Red, green, green, blue.
        b"0102" => (10, [0, 1, 3, 2]),
        // Red, green, blue, green.
        b"0103" => (20, [0, 1, 2, 3]),
        b"0205" => (18, [0, 1, 3, 2]),
        _ => return None,
    };
    let mut mul = [0f32; 4];
    for (i, &c) in order.iter().enumerate() {
        mul[c] = f32::from(u16_at(cb, at + 2 * i)?);
    }
    (mul[0] > 0. && mul[1] > 0. && mul[2] > 0.).then_some(mul)
}

/// The black levels (red, green, blue) at 16 bits of a Coolpix's
/// ColorBalance of version 0500 (at byte 54) or 0502 (at 56): Adobe DNG
/// Converter's BlackLevel of the dataset's COOLPIX P7700, P7800 (0500),
/// P330 and P340 (0502), at their data's scale (our own measurements).
fn color_balance_black(
    cb: &[u8],
    u16_at: impl Fn(&[u8], usize) -> Option<u16>,
) -> Option<[u32; 3]> {
    let at = match cb.get(..4)? {
        b"0500" => 54,
        b"0502" => 56,
        _ => return None,
    };
    let level = |i: usize| u16_at(cb, at + 2 * i).map(u32::from);
    Some([level(0)?, level(1)?, level(2)?])
}

/// A Coolpix's colour data (0x0014): ColorBalanceA, big-endian 16-bit red
/// and blue levels over a green of 256, or an NRW's ColorBalanceB or C,
/// little-endian 32-bit red, green, green, blue, whose red and blue count
/// twice.
fn coolpix_color(data: &[u8], notes: &mut Notes) {
    let be16 = |at: usize| {
        data.get(at..at + 2)
            .map(|b| f32::from(u16::from_be_bytes([b[0], b[1]])))
    };
    let le32 = |at: usize| {
        data.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32)
    };
    // Red, green, blue from red, green, green, blue at `at`.
    let rggb = |at: usize| -> Option<[f32; 3]> {
        Some([2. * le32(at)?, le32(at + 4)?, 2. * le32(at + 12)?])
    };
    let rb = |at: usize| -> Option<[f32; 3]> { Some([be16(at)?, 256., be16(at + 2)?]) };
    let as_shot = |v: Option<[f32; 3]>| v.map(|[r, g, b]| [r, g, b, g]);
    let preset = |v: Option<[f32; 3]>| v.unwrap_or_default();
    match data.get(..8) {
        Some(b"NRW 0100") => {
            notes.coolpix = Coolpix::Nrw;
            notes.cam_mul = notes.cam_mul.or(as_shot(rggb(5096)));
            notes.presets.daylight = preset(rggb(5112));
        }
        Some(v) if v.starts_with(b"NRW ") => {
            notes.coolpix = Coolpix::Nrw;
            notes.cam_mul = notes.cam_mul.or(as_shot(rggb(56)));
            notes.presets.daylight = preset(rggb(76));
            notes.presets.shade = preset(rggb(116));
            notes.presets.d65 = preset(rggb(176));
            notes.nrw_black = le32(32).map(|b| b as u32).filter(|&b| b > 0);
        }
        Some(_) if data.len() >= 1384 => {
            notes.coolpix = Coolpix::ColorBalanceA;
            notes.cam_mul = notes.cam_mul.or(as_shot(rb(1248)));
            notes.presets.daylight = preset(rb(1256));
            notes.presets.d65 = preset(rb(1316));
            notes.presets.shade = preset(rb(1380));
        }
        _ => {}
    }
}

/// The bytes of a LensData block [`lens_id`] reads: its version and the
/// LensIDNumber, at byte 13 at most.
const LENS_DATA_USED: usize = 14;

/// The LensIDNumber of a LensData block, by its version: unencrypted
/// versions 0100 and 0101, encrypted 02xx and 08xx (whose first four bytes,
/// the version, are not); 0 for others.
fn lens_id(lens: &[u8], keys: Option<Keys>) -> u32 {
    let at = match lens.get(..4) {
        Some(b"0100") => 6,
        Some(b"0101" | b"0201" | b"0202" | b"0203") => 11,
        Some(b"0204") => 12,
        Some(b"0800" | b"0801" | b"0802") => 13,
        _ => return 0,
    };
    let lens = match (lens.get(..2), keys) {
        (Some(b"01"), _) => lens.to_vec(),
        (_, Some(k)) => k.decrypt(lens),
        (_, None) => return 0,
    };
    lens.get(at).map_or(0, |&v| u32::from(v))
}

/// The NEF linearization table, its values in the maker note's byte order.
fn curve(raw: &[u8], little: bool) -> Option<Curve> {
    let u16_at = |at: usize| {
        raw.get(at..at + 2).map(|v| {
            if little {
                u16::from_le_bytes([v[0], v[1]])
            } else {
                u16::from_be_bytes([v[0], v[1]])
            }
        })
    };
    let version = [*raw.first()?, *raw.get(1)?];
    // Versions 0x49 and 0x58 keep the predictions 2110 bytes further on.
    let at = if version[0] == 0x49 || version[1] == 0x58 {
        2110
    } else {
        0
    };
    let predictors = [
        [u16_at(at + 2)?, u16_at(at + 4)?],
        [u16_at(at + 6)?, u16_at(at + 8)?],
    ];
    let size = u16_at(at + 10).unwrap_or(0);
    // Points are indexed by a 16-bit size: those past 65536 are never read.
    let points = (at + 12..raw.len().saturating_sub(1))
        .step_by(2)
        .take(1 << 16)
        .filter_map(u16_at)
        .collect();
    let split = u16_at(562).unwrap_or(0);
    Some(Curve {
        version,
        predictors,
        size,
        points,
        split,
    })
}

/// The keys of the maker note's encrypted blocks: from the serial number
/// (SerialNumber, 0x001d) and the shutter count (ShutterCount, 0x00a7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Keys {
    /// The keystream's step and its start.
    step: u8,
    start: u8,
}

impl Keys {
    fn of(note: &TiffFile, ifd: &Ifd, model: &str) -> Option<Keys> {
        let count = note.int(ifd, 0x00a7)?;
        let serial = match note.text_upto(ifd, 0x001d, TEXT_MAX) {
            // The D50 writes its model name, not a serial number; LibRaw
            // keys it as 34 (observed compatibility, measured on synthetic
            // files; the file carries no other key).
            _ if model == "D50" => 34,
            Some(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
                s.bytes().fold(0u32, |n, b| {
                    n.wrapping_mul(10).wrapping_add(u32::from(b - b'0'))
                })
            }
            Some(s) if !s.is_empty() => 96,
            _ => 0,
        };
        let key = count.to_le_bytes().iter().fold(0u8, |k, b| k ^ b);
        Some(Keys {
            step: SERIAL_KEYS[(serial & 0xff) as usize],
            start: COUNT_KEYS[usize::from(key)],
        })
    }
    /// `data` decrypted after its four version bytes: each byte XORed with a
    /// running sum that grows by `step` times a counter starting at 0x60.
    fn decrypt(self, data: &[u8]) -> Vec<u8> {
        let (mut sum, mut counter) = (self.start, 0x60u8);
        let mut out = data.to_vec();
        for b in out.iter_mut().skip(4) {
            sum = sum.wrapping_add(self.step.wrapping_mul(counter));
            counter = counter.wrapping_add(1);
            *b ^= sum;
        }
        out
    }
}

/// The keystream step for each low byte of the serial number (our own
/// measurements).
const SERIAL_KEYS: [u8; 256] = [
    0xc1, 0xbf, 0x6d, 0x0d, 0x59, 0xc5, 0x13, 0x9d, 0x83, 0x61, 0x6b, 0x4f, 0xc7, 0x7f, 0x3d, 0x3d,
    0x53, 0x59, 0xe3, 0xc7, 0xe9, 0x2f, 0x95, 0xa7, 0x95, 0x1f, 0xdf, 0x7f, 0x2b, 0x29, 0xc7, 0x0d,
    0xdf, 0x07, 0xef, 0x71, 0x89, 0x3d, 0x13, 0x3d, 0x3b, 0x13, 0xfb, 0x0d, 0x89, 0xc1, 0x65, 0x1f,
    0xb3, 0x0d, 0x6b, 0x29, 0xe3, 0xfb, 0xef, 0xa3, 0x6b, 0x47, 0x7f, 0x95, 0x35, 0xa7, 0x47, 0x4f,
    0xc7, 0xf1, 0x59, 0x95, 0x35, 0x11, 0x29, 0x61, 0xf1, 0x3d, 0xb3, 0x2b, 0x0d, 0x43, 0x89, 0xc1,
    0x9d, 0x9d, 0x89, 0x65, 0xf1, 0xe9, 0xdf, 0xbf, 0x3d, 0x7f, 0x53, 0x97, 0xe5, 0xe9, 0x95, 0x17,
    0x1d, 0x3d, 0x8b, 0xfb, 0xc7, 0xe3, 0x67, 0xa7, 0x07, 0xf1, 0x71, 0xa7, 0x53, 0xb5, 0x29, 0x89,
    0xe5, 0x2b, 0xa7, 0x17, 0x29, 0xe9, 0x4f, 0xc5, 0x65, 0x6d, 0x6b, 0xef, 0x0d, 0x89, 0x49, 0x2f,
    0xb3, 0x43, 0x53, 0x65, 0x1d, 0x49, 0xa3, 0x13, 0x89, 0x59, 0xef, 0x6b, 0xef, 0x65, 0x1d, 0x0b,
    0x59, 0x13, 0xe3, 0x4f, 0x9d, 0xb3, 0x29, 0x43, 0x2b, 0x07, 0x1d, 0x95, 0x59, 0x59, 0x47, 0xfb,
    0xe5, 0xe9, 0x61, 0x47, 0x2f, 0x35, 0x7f, 0x17, 0x7f, 0xef, 0x7f, 0x95, 0x95, 0x71, 0xd3, 0xa3,
    0x0b, 0x71, 0xa3, 0xad, 0x0b, 0x3b, 0xb5, 0xfb, 0xa3, 0xbf, 0x4f, 0x83, 0x1d, 0xad, 0xe9, 0x2f,
    0x71, 0x65, 0xa3, 0xe5, 0x07, 0x35, 0x3d, 0x0d, 0xb5, 0xe9, 0xe5, 0x47, 0x3b, 0x9d, 0xef, 0x35,
    0xa3, 0xbf, 0xb3, 0xdf, 0x53, 0xd3, 0x97, 0x53, 0x49, 0x71, 0x07, 0x35, 0x61, 0x71, 0x2f, 0x43,
    0x2f, 0x11, 0xdf, 0x17, 0x97, 0xfb, 0x95, 0x3b, 0x7f, 0x6b, 0xd3, 0x25, 0xbf, 0xad, 0xc7, 0xc5,
    0xc5, 0xb5, 0x8b, 0xef, 0x2f, 0xd3, 0x07, 0x6b, 0x25, 0x49, 0x95, 0x25, 0x49, 0x6d, 0x71, 0xc7,
];

/// The keystream start for each XOR of the shutter count's four bytes (our
/// own measurements).
const COUNT_KEYS: [u8; 256] = [
    0xa7, 0xbc, 0xc9, 0xad, 0x91, 0xdf, 0x85, 0xe5, 0xd4, 0x78, 0xd5, 0x17, 0x46, 0x7c, 0x29, 0x4c,
    0x4d, 0x03, 0xe9, 0x25, 0x68, 0x11, 0x86, 0xb3, 0xbd, 0xf7, 0x6f, 0x61, 0x22, 0xa2, 0x26, 0x34,
    0x2a, 0xbe, 0x1e, 0x46, 0x14, 0x68, 0x9d, 0x44, 0x18, 0xc2, 0x40, 0xf4, 0x7e, 0x5f, 0x1b, 0xad,
    0x0b, 0x94, 0xb6, 0x67, 0xb4, 0x0b, 0xe1, 0xea, 0x95, 0x9c, 0x66, 0xdc, 0xe7, 0x5d, 0x6c, 0x05,
    0xda, 0xd5, 0xdf, 0x7a, 0xef, 0xf6, 0xdb, 0x1f, 0x82, 0x4c, 0xc0, 0x68, 0x47, 0xa1, 0xbd, 0xee,
    0x39, 0x50, 0x56, 0x4a, 0xdd, 0xdf, 0xa5, 0xf8, 0xc6, 0xda, 0xca, 0x90, 0xca, 0x01, 0x42, 0x9d,
    0x8b, 0x0c, 0x73, 0x43, 0x75, 0x05, 0x94, 0xde, 0x24, 0xb3, 0x80, 0x34, 0xe5, 0x2c, 0xdc, 0x9b,
    0x3f, 0xca, 0x33, 0x45, 0xd0, 0xdb, 0x5f, 0xf5, 0x52, 0xc3, 0x21, 0xda, 0xe2, 0x22, 0x72, 0x6b,
    0x3e, 0xd0, 0x5b, 0xa8, 0x87, 0x8c, 0x06, 0x5d, 0x0f, 0xdd, 0x09, 0x19, 0x93, 0xd0, 0xb9, 0xfc,
    0x8b, 0x0f, 0x84, 0x60, 0x33, 0x1c, 0x9b, 0x45, 0xf1, 0xf0, 0xa3, 0x94, 0x3a, 0x12, 0x77, 0x33,
    0x4d, 0x44, 0x78, 0x28, 0x3c, 0x9e, 0xfd, 0x65, 0x57, 0x16, 0x94, 0x6b, 0xfb, 0x59, 0xd0, 0xc8,
    0x22, 0x36, 0xdb, 0xd2, 0x63, 0x98, 0x43, 0xa1, 0x04, 0x87, 0x86, 0xf7, 0xa6, 0x26, 0xbb, 0xd6,
    0x59, 0x4d, 0xbf, 0x6a, 0x2e, 0xaa, 0x2b, 0xef, 0xe6, 0x78, 0xb6, 0x4e, 0xe0, 0x2f, 0xdc, 0x7c,
    0xbe, 0x57, 0x19, 0x32, 0x7e, 0x2a, 0xd0, 0xb8, 0xba, 0x29, 0x00, 0x3c, 0x52, 0x7d, 0xa8, 0x49,
    0x3b, 0x2d, 0xeb, 0x25, 0x49, 0xfa, 0xa3, 0xaa, 0x39, 0xa7, 0xc5, 0xa7, 0x50, 0x11, 0x36, 0xfb,
    0xc6, 0x67, 0x4a, 0xf5, 0xa5, 0x12, 0x65, 0x7e, 0xb0, 0xdf, 0xaf, 0x4e, 0xb3, 0x61, 0x7f, 0x2f,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Adobe's MakN record of a Nikon note (its own TIFF, ISO 200 in
    /// 0x0002) is read; other records, and the record cut anywhere, are
    /// none or read without panicking.
    #[test]
    fn reads_the_note_a_dng_keeps() {
        let mut note = b"Nikon\0\x02\x10\0\0MM\0\x2a\0\0\0\x08".to_vec();
        note.extend(1u16.to_be_bytes());
        note.extend([0, 2, 0, 3, 0, 0, 0, 2, 0, 0, 0, 200]);
        note.extend(0u32.to_be_bytes());
        let mut file = b"junk".to_vec();
        file.extend(b"Adobe\0MakN");
        file.extend((note.len() as u32 + 6).to_be_bytes());
        file.extend(b"MM\0\0\x01\0");
        file.extend(&note);
        let notes = read_dng(&file, 4, "D80").unwrap();
        assert_eq!((notes.iso, notes.lens_id), (Some(200.), 0));
        assert!(read_dng(&file, 0, "D80").is_none());
        for n in 0..file.len() {
            let _ = read_dng(&file[..n], 4, "D80");
        }
    }

    /// The keys of serial number 1234 and shutter count 5678.
    fn keys() -> Keys {
        let count = 5678u32.to_le_bytes().iter().fold(0u8, |k, b| k ^ b);
        Keys {
            step: SERIAL_KEYS[1234 & 0xff],
            start: COUNT_KEYS[usize::from(count)],
        }
    }

    #[test]
    fn lens_ids_decrypt_as_measured() {
        // LensData of each version over the bytes 1, 2, 3…, with serial
        // number 1234 and shutter count 5678: the lens IDs LibRaw reported.
        let cipher: Vec<u8> = (1..60).collect();
        for (version, want) in [(b"0201", 33), (b"0204", 232), (b"0800", 70)] {
            let lens = [&version[..], &cipher].concat();
            assert_eq!(lens_id(&lens, Some(keys())), want, "{version:?}");
        }
        // Unencrypted versions are read as they are; encrypted ones need keys.
        let lens = [&b"0100"[..], &cipher].concat();
        assert_eq!(lens_id(&lens, None), 3);
        let lens = [&b"0204"[..], &cipher].concat();
        assert_eq!(lens_id(&lens, None), 0);
        assert_eq!(lens_id(b"02", Some(keys())), 0);
    }

    #[test]
    fn colour_balance_versions_place_their_multipliers() {
        let mut cb = b"0103".to_vec();
        cb.resize(40, 0);
        // Red, green, blue, green at 20, big-endian.
        for (i, v) in [555u16, 256, 388, 256].iter().enumerate() {
            cb[20 + 2 * i..22 + 2 * i].copy_from_slice(&v.to_be_bytes());
        }
        let be = |b: &[u8], at: usize| b.get(at..at + 2).map(|v| u16::from_be_bytes([v[0], v[1]]));
        assert_eq!(color_balance(&cb, be), Some([555., 256., 388., 256.]));
        assert_eq!(color_balance(&cb[..22], be), None);
        assert_eq!(color_balance(b"0999", be), None);
    }

    #[test]
    fn coolpix_colour_balance_gives_its_black_levels() {
        // Version 0500 keeps red, green, blue at byte 54; 0502 at 56.
        let le = |b: &[u8], at: usize| b.get(at..at + 2).map(|v| u16::from_le_bytes([v[0], v[1]]));
        for (version, at) in [(b"0500", 54), (b"0502", 56)] {
            let mut cb = version.to_vec();
            cb.resize(at + 6, 0);
            for (i, v) in [3208u16, 3209, 3210].iter().enumerate() {
                cb[at + 2 * i..at + 2 * i + 2].copy_from_slice(&v.to_le_bytes());
            }
            assert_eq!(color_balance_black(&cb, le), Some([3208, 3209, 3210]));
            assert_eq!(color_balance_black(&cb[..at + 5], le), None);
        }
        assert_eq!(color_balance_black(b"0103", le), None);
    }

    #[test]
    fn the_d1x_table_keeps_its_predictions_further_on() {
        // Version 0x44 0x58 (the D1X's, in tag 0x008c): 2110 bytes of other
        // data, then the predictions, the size and the points.
        let mut raw = vec![0x44, 0x58];
        raw.resize(2112, 0xee);
        for v in [212u16, 212, 212, 212, 3, 0, 5, 9] {
            raw.extend(v.to_be_bytes());
        }
        let c = curve(&raw, false).unwrap();
        assert_eq!((c.predictors, c.size), ([[212; 2]; 2], 3));
        assert_eq!(c.points[..3], [0, 5, 9]);
        // Other versions keep them at the start.
        let c = curve(&[0x46, 0x30, 0, 1, 0, 2, 0, 3, 0, 4, 0, 0], false).unwrap();
        assert_eq!(c.predictors, [[1, 2], [3, 4]]);
    }

    #[test]
    fn short_and_garbage_notes_do_not_panic() {
        for len in 0..40 {
            let data: Vec<u8> = (0..len).map(|i| (i * 37) as u8).collect();
            let _ = curve(&data, true);
            let _ = lens_id(&data, Some(keys()));
            let mut notes = Notes::default();
            coolpix_color(&data, &mut notes);
        }
    }
}
