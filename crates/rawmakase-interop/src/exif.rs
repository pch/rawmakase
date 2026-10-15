//! The camera's own EXIF, read from a RAW, JPEG, TIFF, PNG, WebP, AVIF or JPEG
//! XL: for an export to carry as Lightroom's do (capture settings, lens, dates,
//! serial numbers and GPS), and for the catalog's capture time and photo info.
//! Maker notes, thumbnail offsets and other pointers into the RAW are left out;
//! they would not survive the move to a new file.
use crate::{
    jpeg::{APP1, Segments},
    tiff::{
        Entry, Tiff,
        kind::{ASCII, IFD, LONG, RATIONAL, SHORT, SRATIONAL},
    },
};
use std::{fs::File, io::Read, path::Path};
use tag::*;

/// The tags RAWmakase reads or writes, by their EXIF names: of the main
/// directory, the EXIF directory and the GPS directory.
pub mod tag {
    pub const IMAGE_DESCRIPTION: u16 = 0x010e;
    pub const MAKE: u16 = 0x010f;
    pub const MODEL: u16 = 0x0110;
    pub const ORIENTATION: u16 = 0x0112;
    pub const X_RESOLUTION: u16 = 0x011a;
    pub const Y_RESOLUTION: u16 = 0x011b;
    pub const RESOLUTION_UNIT: u16 = 0x0128;
    pub const SOFTWARE: u16 = 0x0131;
    pub const DATE_TIME: u16 = 0x0132;
    pub const ARTIST: u16 = 0x013b;
    pub const YCBCR_POSITIONING: u16 = 0x0213;
    /// The XMP packet, in a TIFF.
    pub const XMP: u16 = 0x02bc;
    pub const COPYRIGHT: u16 = 0x8298;
    pub const ICC_PROFILE: u16 = 0x8773;
    /// Pointers to the EXIF and GPS directories.
    pub const EXIF_IFD: u16 = 0x8769;
    pub const GPS_IFD: u16 = 0x8825;

    pub const EXPOSURE_TIME: u16 = 0x829a;
    pub const F_NUMBER: u16 = 0x829d;
    pub const ISO: u16 = 0x8827;
    pub const SENSITIVITY_TYPE: u16 = 0x8830;
    pub const STANDARD_OUTPUT_SENSITIVITY: u16 = 0x8831;
    pub const RECOMMENDED_EXPOSURE_INDEX: u16 = 0x8832;
    pub const ISO_SPEED: u16 = 0x8833;
    pub const DATE_TIME_ORIGINAL: u16 = 0x9003;
    pub const DATE_TIME_DIGITIZED: u16 = 0x9004;
    pub const OFFSET_TIME: u16 = 0x9010;
    pub const OFFSET_TIME_ORIGINAL: u16 = 0x9011;
    pub const OFFSET_TIME_DIGITIZED: u16 = 0x9012;
    /// APEX values of the exposure time and aperture.
    pub const SHUTTER_SPEED_VALUE: u16 = 0x9201;
    pub const APERTURE_VALUE: u16 = 0x9202;
    pub const FOCAL_LENGTH: u16 = 0x920a;
    pub const MAKER_NOTE: u16 = 0x927c;
    pub const SUBSEC_TIME: u16 = 0x9290;
    pub const SUBSEC_TIME_ORIGINAL: u16 = 0x9291;
    pub const SUBSEC_TIME_DIGITIZED: u16 = 0x9292;
    pub const COLOR_SPACE: u16 = 0xa001;
    pub const PIXEL_X_DIMENSION: u16 = 0xa002;
    pub const PIXEL_Y_DIMENSION: u16 = 0xa003;
    pub const INTEROPERABILITY_IFD: u16 = 0xa005;
    pub const LENS_MODEL: u16 = 0xa434;

    pub const GPS_VERSION_ID: u16 = 0x0000;
    pub const GPS_LATITUDE_REF: u16 = 0x0001;
    pub const GPS_LATITUDE: u16 = 0x0002;
    pub const GPS_LONGITUDE_REF: u16 = 0x0003;
    pub const GPS_LONGITUDE: u16 = 0x0004;
    pub const GPS_ALTITUDE_REF: u16 = 0x0005;
    pub const GPS_ALTITUDE: u16 = 0x0006;
}

/// A TIFF field with its value in little-endian byte order.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub tag: u16,
    pub kind: u16,
    pub count: u32,
    pub bytes: Vec<u8>,
}
impl Field {
    pub fn ascii(tag: u16, value: &str) -> Self {
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        Self {
            tag,
            kind: ASCII,
            count: bytes.len() as u32,
            bytes,
        }
    }
    pub fn short(tag: u16, value: u16) -> Self {
        Self {
            tag,
            kind: SHORT,
            count: 1,
            bytes: value.to_le_bytes().to_vec(),
        }
    }
    pub fn long(tag: u16, value: u32) -> Self {
        Self {
            tag,
            kind: LONG,
            count: 1,
            bytes: value.to_le_bytes().to_vec(),
        }
    }
    pub fn rational(tag: u16, n: u32, d: u32) -> Self {
        let mut bytes = n.to_le_bytes().to_vec();
        bytes.extend(d.to_le_bytes());
        Self {
            tag,
            kind: RATIONAL,
            count: 1,
            bytes,
        }
    }
    /// The first value of a numeric field: SHORT, LONG or (S)RATIONAL.
    pub fn number(&self) -> Option<f64> {
        let word = |at: usize| {
            Some(u32::from_le_bytes(
                self.bytes.get(at..at + 4)?.try_into().ok()?,
            ))
        };
        match self.kind {
            SHORT => Some(u16::from_le_bytes(self.bytes.get(..2)?.try_into().ok()?) as f64),
            LONG => word(0).map(f64::from),
            RATIONAL => Some(word(0)? as f64 / word(4).filter(|d| *d != 0)? as f64),
            SRATIONAL => {
                Some(word(0)? as i32 as f64 / (word(4).filter(|d| *d != 0)? as i32) as f64)
            }
            _ => None,
        }
    }
    /// Text of an ASCII field, without the terminator.
    pub fn text(&self) -> Option<String> {
        (self.kind == ASCII).then(|| {
            String::from_utf8_lossy(&self.bytes)
                .trim_end_matches('\0')
                .trim()
                .to_string()
        })
    }
}

/// The directories an export copies from the camera.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CameraExif {
    /// Make, model, artist and copyright from the main directory.
    pub main: Vec<Field>,
    /// The EXIF directory: exposure, lens, dates, serial numbers…
    pub exif: Vec<Field>,
    pub gps: Vec<Field>,
}
impl CameraExif {
    pub fn get(&self, tag: u16) -> Option<&Field> {
        self.main
            .iter()
            .chain(&self.exif)
            .find(|field| field.tag == tag)
    }
    /// DateTimeOriginal as "2018:08:26 10:39:33".
    pub fn captured(&self) -> Option<String> {
        self.get(DATE_TIME_ORIGINAL)
            .and_then(Field::text)
            .filter(|s| !s.is_empty())
    }
    /// Capture settings LibRaw reports, for a RAW whose EXIF could not be read.
    pub fn from_libraw(m: &crate::camera_data::Metadata) -> Self {
        let rational =
            |tag, v: f32| Field::rational(tag, (v.max(0.) * 1_000_000.).round() as u32, 1_000_000);
        let main = vec![Field::ascii(MAKE, &m.make), Field::ascii(MODEL, &m.model)];
        let mut exif = vec![
            rational(EXPOSURE_TIME, m.shutter),
            rational(F_NUMBER, m.aperture),
            Field::short(ISO, m.iso.min(65535.) as u16),
            rational(FOCAL_LENGTH, m.focal),
        ];
        if !m.lens_model.is_empty() {
            exif.push(Field::ascii(LENS_MODEL, &m.lens_model));
        }
        Self {
            main,
            exif,
            gps: Vec::new(),
        }
    }
}

/// Main-directory tags an export keeps; it writes its own orientation, size and
/// software.
const MAIN: [u16; 5] = [IMAGE_DESCRIPTION, MAKE, MODEL, ARTIST, COPYRIGHT];
/// EXIF tags an export replaces or drops: pixel dimensions (the export's own),
/// the interoperability pointer and the maker note, whose internal offsets point
/// into the RAW.
const SKIP: [u16; 4] = [
    PIXEL_X_DIMENSION,
    PIXEL_Y_DIMENSION,
    INTEROPERABILITY_IFD,
    MAKER_NOTE,
];
/// A value this long is not capture metadata (and would crowd the 64 KB JPEG
/// segment the EXIF must fit in).
pub const MAX_VALUE: usize = 4096;

/// Reads the camera EXIF of a TIFF-based RAW (ARW, NEF, CR2, DNG, ORF, RW2, PEF…),
/// a JPEG, TIFF, PNG, WebP, AVIF or JPEG XL, or a Fujifilm RAF, whose EXIF lives in its preview JPEG.
/// `None` when the file has none that can be read, e.g. a CR3.
pub fn read(path: &Path) -> Option<CameraExif> {
    directories(path, |tag| MAIN.contains(&tag))
}

/// When the photo was taken, in the form Lightroom stores it: the camera's
/// local time with no zone and three-digit subseconds,
/// "2018-08-26T10:39:33.120". Falls back from DateTimeOriginal to
/// DateTimeDigitized, then DateTime, each with its own subseconds.
pub fn capture_time(path: &Path) -> Option<String> {
    let exif = directories(path, |tag| tag == DATE_TIME)?;
    [
        (DATE_TIME_ORIGINAL, SUBSEC_TIME_ORIGINAL),
        (DATE_TIME_DIGITIZED, SUBSEC_TIME_DIGITIZED),
        (DATE_TIME, SUBSEC_TIME),
    ]
    .into_iter()
    .find_map(|(date, subseconds)| {
        let date = exif.get(date).and_then(Field::text)?;
        lightroom_time(&date, exif.get(subseconds).and_then(Field::text).as_deref())
    })
}

/// Camera, lens and exposure settings of a JPEG, TIFF, PNG, WebP, AVIF, JPEG XL or TIFF-based RAW
/// from its EXIF; `None` when it has none. Dimensions are not read here.
pub fn photo_info(path: &Path) -> Option<crate::metadata::PhotoInfo> {
    let exif = directories(path, |tag| tag == MAKE || tag == MODEL)?;
    let text = |tag| {
        exif.get(tag)
            .and_then(Field::text)
            .filter(|t| !t.is_empty())
    };
    let number = |tag| exif.get(tag).and_then(Field::number).filter(|n| *n > 0.);
    // Lightroom shows the model, which usually names the make too.
    let camera = match (text(MAKE), text(MODEL)) {
        (Some(make), Some(model)) if !model.starts_with(make.split(' ').next().unwrap_or("")) => {
            Some(format!("{make} {model}"))
        }
        (make, model) => model.or(make),
    };
    let info = crate::metadata::PhotoInfo {
        camera,
        lens: text(LENS_MODEL),
        focal: number(FOCAL_LENGTH),
        // FNumber and ExposureTime, else their APEX values.
        aperture: number(F_NUMBER).or_else(|| {
            let av = exif.get(APERTURE_VALUE).and_then(Field::number)?;
            Some(2f64.powf(av / 2.))
        }),
        exposure: number(EXPOSURE_TIME).or_else(|| {
            let tv = exif.get(SHUTTER_SPEED_VALUE).and_then(Field::number)?;
            Some(2f64.powf(-tv))
        }),
        iso: iso(&exif),
        dimensions: None,
    };
    (info != Default::default()).then_some(info)
}

/// The EXIF Make and Model of a JPEG, TIFF, PNG or TIFF-based RAW, trimmed, each
/// empty when missing; `None` when the file has no EXIF at all.
pub fn camera_names(path: &Path) -> Option<(String, String)> {
    let exif = directories(path, |tag| tag == MAKE || tag == MODEL)?;
    let text = |tag| exif.get(tag).and_then(Field::text).unwrap_or_default();
    Some((text(MAKE), text(MODEL)))
}

/// The ISO speed. Above 65535 the EXIF 2.3 tag that SensitivityType
/// names holds it: standard output sensitivity, recommended
/// exposure index or ISO speed.
fn iso(exif: &CameraExif) -> Option<f64> {
    let number = |tag| exif.get(tag).and_then(Field::number).filter(|n| *n > 0.);
    let iso = number(ISO);
    if iso.is_some_and(|iso| iso < 65535.) {
        return iso;
    }
    let extended = match number(SENSITIVITY_TYPE).map(|t| t as u32) {
        Some(1 | 4 | 5 | 7) => STANDARD_OUTPUT_SENSITIVITY,
        Some(2 | 6) => RECOMMENDED_EXPOSURE_INDEX,
        Some(3) => ISO_SPEED,
        _ => {
            return [
                ISO_SPEED,
                RECOMMENDED_EXPOSURE_INDEX,
                STANDARD_OUTPUT_SENSITIVITY,
            ]
            .into_iter()
            .find_map(number)
            .or(iso);
        }
    };
    number(extended).or(iso)
}

/// "YYYY:MM:DD HH:MM:SS" and optional subsecond digits as
/// "YYYY-MM-DDTHH:MM:SS.fff"; `None` for a blank or zeroed date.
pub fn lightroom_time(date: &str, subseconds: Option<&str>) -> Option<String> {
    let digits: Vec<u8> = date.bytes().filter(u8::is_ascii_digit).collect();
    let shape = date.trim().len() >= 19
        && date
            .trim()
            .bytes()
            .take(19)
            .enumerate()
            .all(|(i, b)| match i {
                4 | 7 => matches!(b, b':' | b'-'),
                10 => matches!(b, b' ' | b'T'),
                13 | 16 => b == b':',
                _ => b.is_ascii_digit(),
            });
    if !shape || digits[..8].iter().all(|d| *d == b'0') {
        return None;
    }
    let d = |range: std::ops::Range<usize>| std::str::from_utf8(&digits[range]).unwrap_or("");
    let mut fraction: String = subseconds
        .unwrap_or("")
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .take(3)
        .collect();
    while fraction.len() < 3 {
        fraction.push('0');
    }
    Some(format!(
        "{}-{}-{}T{}:{}:{}.{fraction}",
        d(0..4),
        d(4..6),
        d(6..8),
        d(8..10),
        d(10..12),
        d(12..14)
    ))
}

fn directories(path: &Path, keep_main: impl Fn(u16) -> bool) -> Option<CameraExif> {
    let mut f = File::open(path).ok()?;
    // Enough to tell the containers apart and find a RAF's preview; a small
    // PNG or JPEG can be shorter.
    let mut head = Vec::with_capacity(108);
    (&mut f).take(108).read_to_end(&mut head).ok()?;
    // Where the TIFF starts, and how long it is when embedded in another
    // container, whose other bytes are not read as part of it.
    let (base, len) = if head.starts_with(b"FUJIFILMCCD-RAW") {
        // Where its preview JPEG is, and how long.
        let preview = head.get(84..92)?;
        let jpeg = u32::from_be_bytes(preview[..4].try_into().ok()?) as u64;
        let length = u32::from_be_bytes(preview[4..].try_into().ok()?) as usize;
        let (at, len) = exif_in_jpeg(&mut File::open(path).ok()?, jpeg, length)?;
        (jpeg + at, Some(len))
    } else if head.starts_with(&[0xff, 0xd8]) {
        let length = f.metadata().ok()?.len().min(1 << 16) as usize;
        let (at, len) = exif_in_jpeg(&mut File::open(path).ok()?, 0, length)?;
        (at, Some(len))
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        let (at, len) = exif_in_png(&mut File::open(path).ok()?)?;
        (at, Some(len))
    } else if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP") {
        (exif_in_webp(&mut File::open(path).ok()?)?, None)
    } else if head.get(4..8) == Some(b"ftyp") && has_heif_brand(&head) {
        (exif_in_heif(&mut File::open(path).ok()?)?, None)
    } else if head.starts_with(&crate::isobmff::JXL_CONTAINER) {
        (exif_in_jxl(&mut File::open(path).ok()?)?, None)
    } else {
        (0, None)
    };
    let f = File::open(path).ok()?;
    let mut t = match len {
        Some(len) => Tiff::open_within(f, base, len)?,
        None => Tiff::open(f, base)?,
    };
    let main = t.ifd(t.first)?;
    let mut out = CameraExif {
        main: fields(&mut t, &main, keep_main),
        ..Default::default()
    };
    if let Some(ifd) = main
        .get(&EXIF_IFD)
        .and_then(|e| t.offset(e))
        .and_then(|at| t.ifd(at))
    {
        out.exif = fields(&mut t, &ifd, |tag| !SKIP.contains(&tag) && tag < 0xc000);
    }
    if let Some(ifd) = main
        .get(&GPS_IFD)
        .and_then(|e| t.offset(e))
        .and_then(|at| t.ifd(at))
    {
        out.gps = fields(&mut t, &ifd, |tag| tag <= 0x1f);
    }
    (!out.exif.is_empty() || !out.main.is_empty()).then_some(out)
}

/// Whether an `ftyp` box lists the AVIF/HEIF-image brands (`avif`, `avis`,
/// `mif1`) as major or compatible brand; other ISOBMFF files (Canon CR3, MP4)
/// are left to the TIFF path.
fn has_heif_brand(head: &[u8]) -> bool {
    let Some(brands) = head.get(8..head.len().min(64)) else {
        return false;
    };
    let (brands, _) = brands.as_chunks::<4>();
    // brands[1] is the minor version, not a brand.
    brands
        .iter()
        .enumerate()
        .any(|(i, b)| i != 1 && matches!(b, b"avif" | b"avis" | b"mif1"))
}

/// Offset of the TIFF header in a WebP's EXIF chunk (some writers keep
/// JPEG's "Exif" header in it).
fn exif_in_webp(f: &mut File) -> Option<u64> {
    let chunk = crate::riff::webp_chunks(f)?
        .into_iter()
        .find(|c| &c.kind == b"EXIF")?;
    tiff_after_header(f, chunk.start, chunk.start.checked_add(chunk.size)?)
}

/// Offset of the TIFF header in an AVIF's Exif item, which starts with the
/// 4-byte offset to it.
fn exif_in_heif(f: &mut File) -> Option<u64> {
    let heif = crate::isobmff::Heif::read(f)?;
    let id = heif
        .describing(*b"cdsc", heif.primary)
        .into_iter()
        .find(|id| heif.item(*id).is_some_and(|i| &i.kind == b"Exif"))?;
    let (start, len) = heif.extent(id)?;
    after_offset(f, start, len)
}

/// Offset of the TIFF header in a JPEG XL container's Exif box (also with a
/// 4-byte offset first). Brotli-compressed ("brob") boxes are not read.
fn exif_in_jxl(f: &mut File) -> Option<u64> {
    let len = crate::isobmff::file_len(f)?;
    let exif = crate::isobmff::boxes(f, 0, len)
        .into_iter()
        .find(|b| &b.kind == b"Exif")?;
    after_offset(f, exif.start, exif.size)
}

/// The TIFF header of the `len` bytes at `start`, after the big-endian u32
/// offset they begin with; `None` when the offset leaves them.
fn after_offset(f: &mut File, start: u64, len: u64) -> Option<u64> {
    use std::io::{Seek, SeekFrom};
    if len < 4 {
        return None;
    }
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut offset = [0u8; 4];
    f.read_exact(&mut offset).ok()?;
    let at = start
        .checked_add(4)?
        .checked_add(u64::from(u32::from_be_bytes(offset)))?;
    tiff_after_header(f, at, start.checked_add(len)?)
}

/// `at`, or past an "Exif\0\0" header there; `None` when `at` is not before
/// `end` by at least a TIFF header. The "Exif" header is looked for only if
/// a TIFF header still fits after it.
fn tiff_after_header(f: &mut File, at: u64, end: u64) -> Option<u64> {
    use std::io::{Seek, SeekFrom};
    if at >= end {
        return None;
    }
    // A TIFF header is 8 bytes.
    if end - at < 8 {
        return None;
    }
    if end - at >= 14 {
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut signature = [0u8; 6];
        f.read_exact(&mut signature).ok()?;
        if signature == *b"Exif\0\0" {
            return Some(at + 6);
        }
    }
    Some(at)
}

/// Offset of the TIFF header inside a JPEG's EXIF segment, relative to the
/// JPEG, and the length of the TIFF the segment holds. Only the first `length`
/// bytes, at most 64 KB, are read.
fn exif_in_jpeg(f: &mut File, jpeg: u64, length: usize) -> Option<(u64, u64)> {
    use std::io::{Cursor, Seek, SeekFrom};
    f.seek(SeekFrom::Start(jpeg)).ok()?;
    let mut head = vec![0u8; length.min(1 << 16)];
    f.read_exact(&mut head).ok()?;
    let mut segments = Segments::new(Cursor::new(head)).ok()??;
    while let Some(segment) = segments.next_segment().ok()? {
        let mut signature = [0u8; 6];
        if segment.marker == APP1
            && segments.reader().read_exact(&mut signature).is_ok()
            && signature == *b"Exif\0\0"
        {
            return Some((segment.offset + 6, segment.length.checked_sub(6)? as u64));
        }
    }
    None
}

/// Offset of the TIFF header inside a PNG's eXIf chunk and the length of the
/// TIFF the chunk holds, found by its chunks' headers alone, before or after
/// the image data. A chunk said to run past the file's end is not read.
fn exif_in_png(f: &mut File) -> Option<(u64, u64)> {
    use std::io::{Seek, SeekFrom};
    let size = f.metadata().ok()?.len();
    let mut at = 8u64;
    loop {
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut header = [0u8; 8];
        f.read_exact(&mut header).ok()?;
        let length = u32::from_be_bytes(header[..4].try_into().ok()?) as u64;
        match &header[4..] {
            b"eXIf" => {
                if at + 8 + length > size {
                    return None;
                }
                // Some writers keep JPEG's "Exif" header in the chunk too.
                let mut signature = [0u8; 6];
                f.read_exact(&mut signature).ok()?;
                let skip = if length >= 6 && signature == *b"Exif\0\0" {
                    6
                } else {
                    0
                };
                return Some((at + 8 + skip, length - skip));
            }
            b"IEND" => return None,
            _ => at += 12 + length,
        }
    }
}

fn fields(
    t: &mut Tiff,
    ifd: &std::collections::BTreeMap<u16, Entry>,
    keep: impl Fn(u16) -> bool,
) -> Vec<Field> {
    ifd.iter()
        .filter(|(tag, e)| keep(**tag) && e.kind != IFD)
        .filter_map(|(tag, e)| {
            let size = e.type_size()?;
            if e.count as usize * size > MAX_VALUE {
                return None;
            }
            let mut bytes = t.raw(e)?;
            if !t.little && size > 1 {
                // Rationals are two 4-byte numbers; every other type is one number.
                for chunk in bytes.chunks_mut(size.min(4)) {
                    chunk.reverse();
                }
            }
            Some(Field {
                tag: *tag,
                kind: e.kind,
                count: e.count,
                bytes,
            })
        })
        .collect()
}

#[cfg(test)]
mod camera_names_tests {
    use super::camera_names;

    /// A little-endian TIFF whose only directory holds Make and Model.
    fn tiff(make: &str, model: &str) -> Vec<u8> {
        let mut t = b"II*\0\x08\0\0\0".to_vec();
        t.extend(directory(8, make, model));
        t
    }

    /// A directory holding Make and Model, then their text, for `at` bytes
    /// into its TIFF; each is longer than the 4 bytes a value holds itself.
    fn directory(at: usize, make: &str, model: &str) -> Vec<u8> {
        let (make, model) = (format!("{make}\0"), format!("{model}\0"));
        let data = at + 2 + 2 * 12 + 4;
        let mut t = b"\x02\0".to_vec();
        for (tag, len, at) in [
            (0x010f_u16, make.len(), data),
            (0x0110, model.len(), data + make.len()),
        ] {
            t.extend(tag.to_le_bytes());
            t.extend(2u16.to_le_bytes());
            t.extend((len as u32).to_le_bytes());
            t.extend((at as u32).to_le_bytes());
        }
        t.extend([0; 4]);
        t.extend(make.bytes().chain(model.bytes()));
        t
    }

    #[test]
    fn make_and_model_are_read_apart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        std::fs::write(&path, tiff("FUJIFILM", "X-T5")).unwrap();
        assert_eq!(
            camera_names(&path),
            Some(("FUJIFILM".into(), "X-T5".into()))
        );
    }

    /// A PNG of `chunks` (type and data) between its header and end; the
    /// reader checks no CRCs, so they are left zero.
    fn png(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        let ihdr = [0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0];
        for (kind, data) in [(b"IHDR", &ihdr[..])]
            .into_iter()
            .chain(chunks.iter().copied())
            .chain([(b"IEND", &[][..])])
        {
            p.extend((data.len() as u32).to_be_bytes());
            p.extend(kind);
            p.extend(data);
            p.extend([0; 4]);
        }
        p
    }

    #[test]
    fn a_pngs_exif_is_read_wherever_its_chunk_is() {
        let dir = tempfile::tempdir().unwrap();
        let exif = tiff("FUJIFILM", "X-T5");
        let prefixed = [&b"Exif\0\0"[..], &exif].concat();
        let idat: &[u8] = &[0; 300];
        for (name, chunks) in [
            ("before.png", vec![(b"eXIf", &exif[..]), (b"IDAT", idat)]),
            ("after.png", vec![(b"IDAT", idat), (b"eXIf", &exif[..])]),
            (
                "prefixed.png",
                vec![(b"eXIf", &prefixed[..]), (b"IDAT", idat)],
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, png(&chunks)).unwrap();
            assert_eq!(
                camera_names(&path),
                Some(("FUJIFILM".into(), "X-T5".into())),
                "{name}"
            );
        }
        let path = dir.path().join("none.png");
        std::fs::write(&path, png(&[(b"IDAT", idat)])).unwrap();
        assert_eq!(camera_names(&path), None);
    }

    #[test]
    fn a_pngs_exif_is_read_from_its_chunk_alone() {
        let dir = tempfile::tempdir().unwrap();
        let idat: &[u8] = &[0; 300];
        // An 8-byte eXIf chunk whose directory is said to be in the next
        // chunk's data: 8 bytes, the chunk's CRC and the next one's header
        // into the TIFF.
        let mut exif = b"II*\0".to_vec();
        exif.extend(20u32.to_le_bytes());
        let other = directory(20, "BAD make", "BAD model");
        let path = dir.path().join("outside.png");
        let chunks = [(b"eXIf", &exif[..]), (b"tEXt", &other[..]), (b"IDAT", idat)];
        std::fs::write(&path, png(&chunks)).unwrap();
        assert_eq!(camera_names(&path), None);
        // A chunk longer than the file.
        let mut long = png(&[(b"eXIf", &tiff("FUJIFILM", "X-T5")[..])]);
        long[33..37].copy_from_slice(&u32::MAX.to_be_bytes());
        let path = dir.path().join("long.png");
        std::fs::write(&path, long).unwrap();
        assert_eq!(camera_names(&path), None);
    }

    #[test]
    fn a_jpegs_exif_is_read_from_its_segment_alone() {
        let dir = tempfile::tempdir().unwrap();
        // An APP1 segment of 8 TIFF bytes whose directory is said to be in
        // the next segment's data: after its marker and length.
        let mut exif = b"Exif\0\0II*\0".to_vec();
        exif.extend(12u32.to_le_bytes());
        let other = directory(12, "BAD make", "BAD model");
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend(((exif.len() + 2) as u16).to_be_bytes());
        jpeg.extend(&exif);
        jpeg.extend([0xff, 0xe2]);
        jpeg.extend(((other.len() + 2) as u16).to_be_bytes());
        jpeg.extend(&other);
        jpeg.extend([0xff, 0xd9]);
        jpeg.resize(jpeg.len().max(108), 0);
        let path = dir.path().join("outside.jpg");
        std::fs::write(&path, jpeg).unwrap();
        assert_eq!(camera_names(&path), None);
    }

    #[test]
    fn a_tiny_pngs_exif_is_read() {
        // A directory holding Model alone, its text in the entry itself.
        let mut exif = b"II*\0\x08\0\0\0\x01\0".to_vec();
        exif.extend(0x0110_u16.to_le_bytes());
        exif.extend(2u16.to_le_bytes());
        exif.extend(4u32.to_le_bytes());
        exif.extend(b"X-T\0");
        exif.extend([0; 4]);
        let file = png(&[(b"eXIf", &exif[..])]);
        assert!(file.len() < 108);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.png");
        std::fs::write(&path, file).unwrap();
        assert_eq!(camera_names(&path), Some((String::new(), "X-T".into())));
    }

    #[test]
    fn a_file_without_exif_has_no_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, [0u8; 200]).unwrap();
        assert_eq!(camera_names(&path), None);
    }
}

#[cfg(test)]
mod container_tests {
    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/formats")
            .join(name)
    }
    #[test]
    fn webp_avif_and_jxl_exif_is_read() {
        for name in ["metadata.webp", "metadata.avif", "metadata.jxl"] {
            let path = fixture(name);
            assert_eq!(
                super::capture_time(&path).as_deref(),
                Some("2024-05-04T10:21:33.000"),
                "{name}"
            );
            assert_eq!(
                super::camera_names(&path),
                Some(("Fixture".into(), "Synthetic 1".into())),
                "{name}"
            );
        }
    }
    #[test]
    fn files_without_exif_have_none() {
        for name in ["ten-bit.avif", "animated.webp", "sixteen-bit.jxl"] {
            assert_eq!(super::camera_names(&fixture(name)), None, "{name}");
        }
    }
    #[test]
    fn other_isobmff_files_are_not_read_as_heif() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.cr3");
        let mut bytes = vec![0, 0, 0, 24];
        bytes.extend(b"ftypcrx \0\0\0\x01crx isom");
        bytes.resize(200, 0);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(super::camera_names(&path), None);
    }
    /// A copy of a fixture with the 4-byte Exif offset before its TIFF header
    /// set to `offset`.
    fn with_offset(name: &str, offset: u32) -> Vec<u8> {
        let mut bytes = std::fs::read(fixture(name)).unwrap();
        let tiff = bytes.windows(4).position(|w| w == b"MM\0*").unwrap();
        // The AVIF item has "Exif\0\0" between its offset and the TIFF header.
        let at = if name.ends_with("avif") {
            tiff - 6
        } else {
            tiff
        };
        bytes[at - 4..at].copy_from_slice(&offset.to_be_bytes());
        bytes
    }
    #[test]
    fn an_exif_offset_past_the_item_or_box_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["metadata.avif", "metadata.jxl"] {
            // The AVIF fixture's own offset is 6 ("Exif\0\0"), the JXL's 0.
            for offset in [0xffff_ffff, 0x7fff_ffff, 1000] {
                let path = dir.path().join(name);
                std::fs::write(&path, with_offset(name, offset)).unwrap();
                assert_eq!(super::camera_names(&path), None, "{name} {offset}");
            }
        }
    }
    #[test]
    fn a_webp_exif_chunk_shorter_than_its_header_is_not_read_past() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = std::fs::read(fixture("metadata.webp")).unwrap();
        let at = bytes.windows(4).position(|w| w == b"EXIF").unwrap();
        bytes[at + 4..at + 8].copy_from_slice(&3u32.to_le_bytes());
        let path = dir.path().join("short.webp");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(super::camera_names(&path), None);
    }
}
