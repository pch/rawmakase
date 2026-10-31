//! The camera's own EXIF, read from a RAW, JPEG, TIFF, PNG, WebP, AVIF, HEIF or
//! JPEG XL: for an export to carry as Lightroom's do (capture settings, lens, dates,
//! serial numbers and GPS), and for the catalog's capture time and photo info.
//! Maker notes, thumbnail offsets and other pointers into the RAW are left out;
//! they would not survive the move to a new file. `sequence` reads a few
//! maker-note tags apart, for Auto-Stack, and gives none of them to exports.
use crate::{
    jpeg::{APP1, Segments},
    tiff::{
        Entry, Tiff,
        kind::{ASCII, IFD, LONG, RATIONAL, SHORT, SRATIONAL},
    },
};
use std::{
    fs::File,
    io::{Read, Seek},
    path::Path,
};
use tag::*;

mod sequence;
#[cfg(any(test, feature = "test-support"))]
pub use sequence::fixture as sequence_fixture;
pub use sequence::{Mark, SEQUENCE_VERSION, Sequence, sequence};

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
    pub const FOCAL_LENGTH_IN_35MM: u16 = 0xa405;
    pub const MAKER_NOTE: u16 = 0x927c;
    pub const SUBSEC_TIME: u16 = 0x9290;
    pub const SUBSEC_TIME_ORIGINAL: u16 = 0x9291;
    pub const SUBSEC_TIME_DIGITIZED: u16 = 0x9292;
    pub const COLOR_SPACE: u16 = 0xa001;
    pub const PIXEL_X_DIMENSION: u16 = 0xa002;
    pub const PIXEL_Y_DIMENSION: u16 = 0xa003;
    pub const INTEROPERABILITY_IFD: u16 = 0xa005;
    pub const BODY_SERIAL_NUMBER: u16 = 0xa431;
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
    /// Capture settings the RAW decoder reports, for a RAW whose EXIF could not be read.
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

/// What a file's EXIF says of the colour of the images its camera rendered, and of
/// the software that wrote it: ColorSpace (1 sRGB, 0xFFFF uncalibrated), the
/// interoperability index ("R98" sRGB, "R03" Adobe RGB) and IFD0's Software.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColourFacts {
    pub color_space: Option<u16>,
    pub interop_index: Option<String>,
    pub software: Option<String>,
}
impl ColourFacts {
    /// `self`'s facts, each from `other` where `self` has none.
    pub fn or(self, other: Self) -> Self {
        Self {
            color_space: self.color_space.or(other.color_space),
            interop_index: self.interop_index.or(other.interop_index),
            software: self.software.or(other.software),
        }
    }
}

/// The [`ColourFacts`] of the file at `path` (a TIFF-based RAW, a JPEG…); `None`
/// when its EXIF can't be read.
pub fn colour_facts(path: &Path) -> Option<ColourFacts> {
    match open_tiff(path)? {
        Source::File(t) => colour_facts_in(t),
        Source::Memory(t) => colour_facts_in(t),
    }
}

/// The [`ColourFacts`] of the JPEG in `jpeg` (a RAW's embedded preview), from its
/// own EXIF segment; `None` without one.
pub fn colour_facts_of_jpeg(jpeg: &[u8]) -> Option<ColourFacts> {
    let mut cursor = std::io::Cursor::new(jpeg);
    let (at, len) = exif_in_jpeg(&mut cursor, 0, jpeg.len())?;
    colour_facts_in(Tiff::open_within(std::io::Cursor::new(jpeg), at, len)?)
}

fn colour_facts_in<R: Read + Seek>(mut t: Tiff<R>) -> Option<ColourFacts> {
    let text = |t: &mut Tiff<R>, e: &Entry| {
        (e.kind == ASCII && e.count as usize <= MAX_VALUE)
            .then(|| t.raw(e))
            .flatten()
            .map(|b| {
                String::from_utf8_lossy(&b)
                    .trim_end_matches('\0')
                    .trim()
                    .to_string()
            })
    };
    let main = t.ifd(t.first)?;
    let mut out = ColourFacts {
        software: main.get(&SOFTWARE).and_then(|e| text(&mut t, e)),
        ..Default::default()
    };
    if let Some(exif) = main
        .get(&EXIF_IFD)
        .and_then(|e| t.offset(e))
        .and_then(|at| t.ifd(at))
    {
        out.color_space = exif
            .get(&COLOR_SPACE)
            .filter(|e| e.kind == SHORT && e.count == 1)
            .map(|e| t.u16(&e.value));
        // The interoperability directory's first tag, InteroperabilityIndex.
        out.interop_index = exif
            .get(&INTEROPERABILITY_IFD)
            .and_then(|e| t.offset(e))
            .and_then(|at| t.ifd(at))
            .and_then(|ifd| ifd.get(&1).and_then(|e| text(&mut t, e)));
    }
    Some(out)
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
/// a JPEG, TIFF, PNG, WebP, AVIF, HEIF or JPEG XL, or a Fujifilm RAF, whose EXIF lives in its preview JPEG.
/// `None` when the file has none that can be read, e.g. a CR3, whose EXIF
/// RAWmakase's RAW decoders find for [`read_tiffs`].
pub fn read(path: &Path) -> Option<CameraExif> {
    directories(path, |tag| MAIN.contains(&tag))
}

/// [`read`] of the TIFFs a RAW container keeps its camera's EXIF in, each
/// on its own, as RAWmakase's RAW decoders find them: `main`'s first
/// directory is IFD0 (a CR3's `CMT1`, an MRW's or X3F's whole TIFF), whose
/// pointers lead to the EXIF and GPS directories unless `exif` or `gps` is
/// a TIFF whose first directory is that directory (a CR3's `CMT2` and
/// `CMT4`). `None` when they hold neither IFD0's tags nor EXIF's.
pub fn read_tiffs(
    main: Option<&[u8]>,
    exif: Option<&[u8]>,
    gps: Option<&[u8]>,
) -> Option<CameraExif> {
    fn open(b: &[u8]) -> Option<Tiff<std::io::Cursor<&[u8]>>> {
        Tiff::open_within(std::io::Cursor::new(b), 0, b.len() as u64)
    }
    let mut out = main
        .and_then(open)
        .and_then(|t| camera_exif(t, |tag| MAIN.contains(&tag)))
        .unwrap_or_default();
    for (tiff, kept, keep) in [
        (exif, &mut out.exif, exif_tag as fn(u16) -> bool),
        (gps, &mut out.gps, gps_tag),
    ] {
        if let Some(mut t) = tiff.and_then(open)
            && let Some(ifd) = t.ifd(t.first)
        {
            *kept = fields(&mut t, &ifd, keep);
        }
    }
    (!out.exif.is_empty() || !out.main.is_empty()).then_some(out)
}

/// When the photo was taken, in the form Lightroom stores it: the camera's
/// local time with no zone and three-digit subseconds,
/// "2018-08-26T10:39:33.120". Falls back from DateTimeOriginal to
/// DateTimeDigitized, then DateTime, each with its own subseconds.
pub fn capture_time(path: &Path) -> Option<String> {
    let exif = directories(path, |tag| tag == DATE_TIME)?;
    let text = |tag| exif.get(tag).and_then(Field::text);
    capture_time_of([
        (text(DATE_TIME_ORIGINAL), text(SUBSEC_TIME_ORIGINAL)),
        (text(DATE_TIME_DIGITIZED), text(SUBSEC_TIME_DIGITIZED)),
        (text(DATE_TIME), text(SUBSEC_TIME)),
    ])
}

/// [`capture_time`] of dates read elsewhere (a RAW container's own
/// records): DateTimeOriginal, DateTimeDigitized and DateTime as EXIF
/// writes them, each with its subseconds, the first that is a date.
pub fn capture_time_of(dates: [(Option<String>, Option<String>); 3]) -> Option<String> {
    dates
        .into_iter()
        .find_map(|(date, subseconds)| lightroom_time(&date?, subseconds.as_deref()))
}

/// Camera, lens and exposure settings of a JPEG, TIFF, PNG, WebP, AVIF, HEIF, JPEG XL or TIFF-based RAW
/// from its EXIF; `None` when it has none. Dimensions are not read here.
pub fn photo_info(path: &Path) -> Option<crate::metadata::PhotoInfo> {
    directories(path, |tag| tag == MAKE || tag == MODEL)?.photo_info()
}

/// [`read`] of what file names take from the EXIF: also the software that
/// wrote the file, which an export writes its own of and so leaves out.
pub fn read_for_names(path: &Path) -> Option<CameraExif> {
    directories(path, |tag| MAIN.contains(&tag) || tag == SOFTWARE)
}

/// The EXIF Make and Model of a JPEG, TIFF, PNG or TIFF-based RAW, trimmed, each
/// empty when missing; `None` when the file has no EXIF at all.
pub fn camera_names(path: &Path) -> Option<(String, String)> {
    Some(directories(path, |tag| tag == MAKE || tag == MODEL)?.camera_names())
}

impl CameraExif {
    /// The camera, lens and exposure settings [`photo_info`] reads, of
    /// these directories; `None` when they have none.
    pub fn photo_info(&self) -> Option<crate::metadata::PhotoInfo> {
        let exif = self;
        let text = |tag| {
            exif.get(tag)
                .and_then(Field::text)
                .filter(|t| !t.is_empty())
        };
        let number = |tag| exif.get(tag).and_then(Field::number).filter(|n| *n > 0.);
        // Lightroom shows the model, which usually names the make too.
        let camera = match (text(MAKE), text(MODEL)) {
            (Some(make), Some(model))
                if !model.starts_with(make.split(' ').next().unwrap_or("")) =>
            {
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
            iso: iso(exif),
            dimensions: None,
        };
        (info != Default::default()).then_some(info)
    }
    /// Make and Model, as [`camera_names`] reads them.
    pub fn camera_names(&self) -> (String, String) {
        let text = |tag| self.get(tag).and_then(Field::text).unwrap_or_default();
        (text(MAKE), text(MODEL))
    }
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
    match open_tiff(path)? {
        Source::File(t) => camera_exif(t, keep_main),
        Source::Memory(t) => camera_exif(t, keep_main),
    }
}

/// The TIFF holding a file's EXIF, read where it is or, when its container
/// may store it in pieces or compressed, from memory.
enum Source {
    File(Tiff<File>),
    Memory(Tiff<std::io::Cursor<Vec<u8>>>),
}

/// The TIFF holding the EXIF of the RAW, JPEG, TIFF, PNG, WebP, AVIF, HEIF
/// or JPEG XL at `path`; `None` when there is none to read.
fn open_tiff(path: &Path) -> Option<Source> {
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
        let (at, len) = exif_in_webp(&mut File::open(path).ok()?)?;
        (at, Some(len))
    } else if head.get(4..8) == Some(b"ftyp") && has_heif_brand(&head) {
        // Its Exif item may be stored in pieces, so the TIFF is read from the
        // joined item in memory.
        return exif_in_memory(exif_in_heif(&mut f)?).map(Source::Memory);
    } else if head.starts_with(&crate::isobmff::JXL_CONTAINER) {
        // Its Exif box may be brotli-compressed ("brob"), so the TIFF is read
        // from the box's body in memory.
        return exif_in_memory(crate::isobmff::jxl_metadata(&mut f, *b"Exif")?).map(Source::Memory);
    } else {
        (0, None)
    };
    let f = File::open(path).ok()?;
    match len {
        Some(len) => Tiff::open_within(f, base, len),
        None => Tiff::open(f, base),
    }
    .map(Source::File)
}

/// The main, EXIF and GPS directories' fields of `t`, keeping the main
/// directory's tags `keep_main` accepts.
fn camera_exif<R: Read + Seek>(
    mut t: Tiff<R>,
    keep_main: impl Fn(u16) -> bool,
) -> Option<CameraExif> {
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
        out.exif = fields(&mut t, &ifd, exif_tag);
    }
    if let Some(ifd) = main
        .get(&GPS_IFD)
        .and_then(|e| t.offset(e))
        .and_then(|at| t.ifd(at))
    {
        out.gps = fields(&mut t, &ifd, gps_tag);
    }
    (!out.exif.is_empty() || !out.main.is_empty()).then_some(out)
}

/// The EXIF directory's tags an export copies: not [`SKIP`]'s, nor the
/// private ones.
fn exif_tag(tag: u16) -> bool {
    !SKIP.contains(&tag) && tag < 0xc000
}

/// The GPS directory's tags, those of Exif 2.32.
fn gps_tag(tag: u16) -> bool {
    tag <= 0x1f
}

/// Whether an `ftyp` box lists the AVIF/HEIF-image brands (`avif`, `avis`,
/// `mif1`, and HEIC's `heic` and `heix`) as major or compatible brand; other
/// ISOBMFF files (Canon CR3, MP4) are left to the TIFF path.
fn has_heif_brand(head: &[u8]) -> bool {
    let Some(brands) = head.get(8..head.len().min(64)) else {
        return false;
    };
    let (brands, _) = brands.as_chunks::<4>();
    // brands[1] is the minor version, not a brand.
    brands
        .iter()
        .enumerate()
        .any(|(i, b)| i != 1 && matches!(b, b"avif" | b"avis" | b"mif1" | b"heic" | b"heix"))
}

/// Offset of the TIFF header in a WebP's EXIF chunk (some writers keep
/// JPEG's "Exif" header in it), and the length of the TIFF the chunk holds.
fn exif_in_webp(f: &mut File) -> Option<(u64, u64)> {
    let chunk = crate::riff::webp_chunks(f)?
        .into_iter()
        .find(|c| &c.kind == b"EXIF")?;
    tiff_after_header(f, chunk.start, chunk.start.checked_add(chunk.size)?)
}

/// An AVIF's or a HEIF's Exif item, up to 16 MiB: the 4-byte offset to the
/// TIFF header, then the TIFF.
fn exif_in_heif(f: &mut File) -> Option<Vec<u8>> {
    let heif = crate::isobmff::Heif::read(f)?;
    let id = heif
        .describing(*b"cdsc", heif.primary)
        .into_iter()
        .find(|id| heif.item(*id).is_some_and(|i| &i.kind == b"Exif"))?;
    heif.data_up_to(f, id, 16 << 20)
}

/// The TIFF in `body`, an Exif item or box: the big-endian u32 offset to
/// the TIFF header, then the TIFF.
fn exif_in_memory(body: Vec<u8>) -> Option<Tiff<std::io::Cursor<Vec<u8>>>> {
    let len = body.len() as u64;
    let mut body = std::io::Cursor::new(body);
    let (at, len) = after_offset(&mut body, 0, len)?;
    Tiff::open_within(body, at, len)
}

/// The TIFF header of the `len` bytes at `start`, after the big-endian u32
/// offset they begin with, and the bytes left to the TIFF from there; `None`
/// when the offset leaves them.
fn after_offset<R: Read + Seek>(f: &mut R, start: u64, len: u64) -> Option<(u64, u64)> {
    use std::io::SeekFrom;
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

/// `at`, or past an "Exif\0\0" header there, and the bytes from it to `end`,
/// which the TIFF may not leave; `None` when `at` is not before `end` by at
/// least a TIFF header. The "Exif" header is looked for only if a TIFF header
/// still fits after it.
fn tiff_after_header<R: Read + Seek>(f: &mut R, at: u64, end: u64) -> Option<(u64, u64)> {
    use std::io::SeekFrom;
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
            return Some((at + 6, end - at - 6));
        }
    }
    Some((at, end - at))
}

/// Offset of the TIFF header inside a JPEG's EXIF segment, relative to the
/// JPEG, and the length of the TIFF the segment holds. Only the first `length`
/// bytes, at most 64 KB, are read.
fn exif_in_jpeg(f: &mut (impl Read + Seek), jpeg: u64, length: usize) -> Option<(u64, u64)> {
    use std::io::{Cursor, SeekFrom};
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
    use std::io::SeekFrom;
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

fn fields<R: Read + Seek>(
    t: &mut Tiff<R>,
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
    use super::{ColourFacts, camera_names, colour_facts, colour_facts_of_jpeg};

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

    /// A JPEG whose EXIF says it is Adobe RGB (ColorSpace uncalibrated, the
    /// interoperability index R03), written by `software`.
    fn adobe_rgb_jpeg(software: &str) -> Vec<u8> {
        let software = format!("{software}\0");
        let entry = |tag: u16, kind: u16, count: u32, value: u32| {
            let mut e = tag.to_le_bytes().to_vec();
            e.extend(kind.to_le_bytes());
            e.extend(count.to_le_bytes());
            e.extend(value.to_le_bytes());
            e
        };
        // IFD0 at 8 (two entries), the EXIF IFD at 38 (two), the interoperability
        // IFD at 68 (one), then the software's text at 86.
        let mut t = b"II*\0\x08\0\0\0".to_vec();
        t.extend(2u16.to_le_bytes());
        t.extend(entry(0x0131, 2, software.len() as u32, 86));
        t.extend(entry(0x8769, 4, 1, 38));
        t.extend([0; 4]);
        t.extend(2u16.to_le_bytes());
        t.extend(entry(0xa001, 3, 1, 0xffff));
        t.extend(entry(0xa005, 4, 1, 68));
        t.extend([0; 4]);
        t.extend(1u16.to_le_bytes());
        t.extend(entry(0x0001, 2, 4, u32::from_le_bytes(*b"R03\0")));
        t.extend([0; 4]);
        assert_eq!(t.len(), 86);
        t.extend(software.bytes());
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend(((t.len() + 8) as u16).to_be_bytes());
        jpeg.extend(b"Exif\0\0");
        jpeg.extend(t);
        jpeg.extend([0xff, 0xd9]);
        jpeg
    }

    #[test]
    fn a_previews_colour_space_and_writer_are_read() {
        let facts = colour_facts_of_jpeg(&adobe_rgb_jpeg("Adobe Photoshop Lightroom")).unwrap();
        assert_eq!(
            facts,
            ColourFacts {
                color_space: Some(0xffff),
                interop_index: Some("R03".into()),
                software: Some("Adobe Photoshop Lightroom".into()),
            }
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jpg");
        std::fs::write(&path, adobe_rgb_jpeg("Camera")).unwrap();
        assert_eq!(
            colour_facts(&path).unwrap().software.as_deref(),
            Some("Camera")
        );
        assert_eq!(colour_facts_of_jpeg(&[0xff, 0xd8, 0xff, 0xd9]), None);
        let none = ColourFacts::default();
        assert_eq!(none.clone().or(facts.clone()), facts);
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
    fn webp_avif_heif_and_jxl_exif_is_read() {
        // metadata-brob.jxl has its Exif box brotli-compressed, as cjxl
        // writes by default; split-exif.avif has its Exif item in two pieces.
        for name in [
            "metadata.webp",
            "metadata.avif",
            "split-exif.avif",
            "metadata.heic",
            "metadata.jxl",
            "metadata-brob.jxl",
        ] {
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
        for name in [
            "ten-bit.avif",
            "grid.heic",
            "animated.webp",
            "sixteen-bit.jxl",
        ] {
            assert_eq!(super::camera_names(&fixture(name)), None, "{name}");
        }
    }
    /// A HEIC's EXIF is read by its own brand, without MIAF's `mif1`.
    #[test]
    fn a_heic_listing_only_its_own_brand_has_its_exif_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("only-heic.heic");
        let mut bytes = std::fs::read(fixture("metadata.heic")).unwrap();
        // Major brand heic; compatible mif1, heic, miaf.
        for brand in [b"mif1", b"miaf"] {
            let at = bytes.windows(4).position(|w| w == brand).unwrap();
            bytes[at..at + 4].copy_from_slice(b"isom");
        }
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(
            super::camera_names(&path),
            Some(("Fixture".into(), "Synthetic 1".into()))
        );
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
    /// A copy of a fixture followed by a unit of its own container (a WebP
    /// chunk, a box) holding a big-endian directory with Make "BAD make" and
    /// Model "BAD model", and where that directory and the Model text are
    /// from the start of the fixture's TIFF.
    fn with_bad_directory_after(name: &str) -> (Vec<u8>, usize, usize, usize) {
        let mut bytes = std::fs::read(fixture(name)).unwrap();
        let tiff = bytes.windows(4).position(|w| w == b"MM\0*").unwrap();
        let (make, model) = (b"BAD make\0", b"BAD model\0");
        let ifd = bytes.len() + 8 - tiff;
        let text = ifd + 2 + 2 * 12 + 4;
        let mut body = 2u16.to_be_bytes().to_vec();
        for (tag, len, at) in [
            (0x010f_u16, make.len(), text),
            (0x0110, model.len(), text + make.len()),
        ] {
            body.extend(tag.to_be_bytes());
            body.extend(2u16.to_be_bytes());
            body.extend((len as u32).to_be_bytes());
            body.extend((at as u32).to_be_bytes());
        }
        body.extend([0; 4]);
        body.extend(make.iter().chain(model));
        if name.ends_with("webp") {
            body.resize(body.len().next_multiple_of(2), 0);
            bytes.extend(b"JUNK");
            bytes.extend((body.len() as u32).to_le_bytes());
            bytes.extend(&body);
            let riff = (bytes.len() - 8) as u32;
            bytes[4..8].copy_from_slice(&riff.to_le_bytes());
        } else {
            bytes.extend(((body.len() + 8) as u32).to_be_bytes());
            bytes.extend(b"free");
            bytes.extend(&body);
        }
        (bytes, tiff, ifd, text + make.len())
    }
    #[test]
    fn an_exif_directory_or_value_past_its_chunk_item_or_box_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let u32_at = |b: &[u8], at: usize| u32::from_be_bytes(b[at..at + 4].try_into().unwrap());
        for name in ["metadata.webp", "metadata.avif", "metadata.jxl"] {
            let path = dir.path().join(name);
            let (bytes, tiff, ifd, bad_model) = with_bad_directory_after(name);
            // The added chunk or box alone leaves the camera as it was.
            std::fs::write(&path, &bytes).unwrap();
            assert_eq!(
                super::camera_names(&path),
                Some(("Fixture".into(), "Synthetic 1".into())),
                "{name}"
            );
            // The first directory said to be in the added chunk or box.
            let mut outside = bytes.clone();
            outside[tiff + 4..tiff + 8].copy_from_slice(&(ifd as u32).to_be_bytes());
            std::fs::write(&path, &outside).unwrap();
            assert_eq!(super::camera_names(&path), None, "{name} directory");
            // The Model's text said to be there.
            let mut outside = bytes;
            let first = tiff + u32_at(&outside, tiff + 4) as usize;
            let count = u16::from_be_bytes([outside[first], outside[first + 1]]) as usize;
            let entry = (0..count)
                .map(|i| first + 2 + 12 * i)
                .find(|&e| outside[e..e + 2] == 0x0110_u16.to_be_bytes())
                .unwrap();
            outside[entry + 4..entry + 8].copy_from_slice(&10u32.to_be_bytes());
            outside[entry + 8..entry + 12].copy_from_slice(&(bad_model as u32).to_be_bytes());
            std::fs::write(&path, &outside).unwrap();
            let model = super::camera_names(&path).map(|(_, model)| model);
            assert!(
                model.as_deref().is_none_or(str::is_empty),
                "{name} value: {model:?}"
            );
        }
    }
}

#[cfg(test)]
mod tiffs_tests {
    use super::*;

    /// A TIFF in `order` ("II" or "MM") whose one directory holds `fields`
    /// (tag, type, count, value as the file writes it), values longer than
    /// four bytes after it.
    fn tiff(order: &[u8; 2], fields: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
        let big = order == b"MM";
        let u16b = |v: u16| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let u32b = |v: u32| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let mut out = order.to_vec();
        out.extend(u16b(42));
        out.extend(u32b(8));
        out.extend(u16b(fields.len() as u16));
        let mut data: Vec<u8> = Vec::new();
        let start = 8 + 2 + 12 * fields.len() + 4;
        for (tag, kind, count, value) in fields {
            out.extend(u16b(*tag));
            out.extend(u16b(*kind));
            out.extend(u32b(*count));
            if value.len() <= 4 {
                let mut v = value.clone();
                v.resize(4, 0);
                out.extend(v);
            } else {
                out.extend(u32b((start + data.len()) as u32));
                data.extend(value);
            }
        }
        out.extend([0; 4]);
        out.extend(data);
        out
    }

    fn ascii(tag: u16, text: &str) -> (u16, u16, u32, Vec<u8>) {
        let mut v = text.as_bytes().to_vec();
        v.push(0);
        (tag, ASCII, v.len() as u32, v)
    }

    /// The software that wrote a file is left out of what exports keep,
    /// and read for file names.
    #[test]
    fn names_read_the_software_exports_leave_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        let fields = [ascii(MAKE, "SONY"), ascii(SOFTWARE, "DSLR-A230 v1.10")];
        std::fs::write(&path, tiff(b"II", &fields)).unwrap();
        assert!(read(&path).unwrap().get(SOFTWARE).is_none());
        let named = read_for_names(&path).unwrap();
        assert_eq!(
            named.get(SOFTWARE).and_then(Field::text).as_deref(),
            Some("DSLR-A230 v1.10")
        );
        assert!(named.get(MAKE).is_some());
    }

    /// A CR3's three TIFFs, each holding one directory, give the
    /// directories `read` gives, by the same rules: IFD0's tags an export
    /// keeps, EXIF's but the maker note, private tags and long values, and
    /// GPS's own tags, numbers in little-endian order.
    #[test]
    fn a_containers_tiffs_are_read_by_the_rules_of_a_files_exif() {
        let main = tiff(
            b"MM",
            &[
                ascii(MAKE, "Canon"),
                (ORIENTATION, SHORT, 1, 6u16.to_be_bytes().to_vec()),
                ascii(ARTIST, "Ann Example"),
                // A pointer CMT1 may have: CMT2 is the EXIF directory.
                (EXIF_IFD, LONG, 1, 4096u32.to_be_bytes().to_vec()),
            ],
        );
        let exif = tiff(
            b"MM",
            &[
                (0x8822, SHORT, 1, 3u16.to_be_bytes().to_vec()),
                ascii(DATE_TIME_ORIGINAL, "2024:08:20 15:24:34"),
                ascii(MAKER_NOTE, "notes"),
                (0x9286, 7, 5000, vec![b'x'; 5000]),
                ascii(0xc5f0, "private"),
            ],
        );
        let gps = tiff(
            b"II",
            &[
                ascii(GPS_LATITUDE_REF, "N"),
                (0x0020, ASCII, 2, b"x\0".to_vec()),
            ],
        );
        let read = read_tiffs(Some(&main), Some(&exif), Some(&gps)).unwrap();
        let tags = |fields: &[Field]| fields.iter().map(|f| f.tag).collect::<Vec<_>>();
        assert_eq!(tags(&read.main), [MAKE, ARTIST]);
        assert_eq!(tags(&read.exif), [0x8822, DATE_TIME_ORIGINAL]);
        assert_eq!(tags(&read.gps), [GPS_LATITUDE_REF]);
        assert_eq!(read.get(0x8822).and_then(Field::number), Some(3.));
        assert_eq!(read.captured().as_deref(), Some("2024:08:20 15:24:34"));
        // An MRW's or X3F's one TIFF, its EXIF directory found by IFD0's
        // pointer, as in a file.
        let whole = read_tiffs(Some(&tiff(b"II", &[ascii(MAKE, "SIGMA")])), None, None).unwrap();
        assert_eq!(tags(&whole.main), [MAKE]);
        assert!(whole.exif.is_empty() && whole.gps.is_empty());
        // The EXIF directory alone is enough; GPS alone, or nothing, is none.
        assert_eq!(
            tags(&read_tiffs(None, Some(&exif), None).unwrap().exif),
            [0x8822, DATE_TIME_ORIGINAL]
        );
        assert_eq!(read_tiffs(None, None, Some(&gps)), None);
        assert_eq!(read_tiffs(None, None, None), None);
        assert_eq!(read_tiffs(Some(b"not a TIFF"), Some(&[]), None), None);
        // Cut short anywhere, a TIFF is read as far as it goes.
        for end in 0..exif.len() {
            let _ = read_tiffs(Some(&main[..end.min(main.len())]), Some(&exif[..end]), None);
        }
    }
}
