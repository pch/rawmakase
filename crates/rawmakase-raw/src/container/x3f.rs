//! Sigma X3F (Foveon): "FOVb" and a header, then sections, each starting
//! with its own mark, and the directory ("SECd": a version, a count, then
//! each section's offset, length and type) at the offset the file's last
//! four bytes give. Image sections ("SECi", types `IMAG` and `IMA2`) have a
//! 28-byte header before their data: the sensor data, thumbnails and, on
//! the SD14 and later bodies, a JPEG preview with the camera's EXIF. The
//! property list ("SECp", type `PROP`) is a count of name and value
//! offsets into UTF-16 text, among them TIME, when the photo was taken in
//! seconds since 1970 on the camera's clock; the sd Quattro and later bodies
//! write none. RAWmakase decodes no X3F sensor data: only where the
//! preview's EXIF is, the capture time and, for the bodies without that
//! EXIF, the camera and exposure of the property list are read here.
//!
//! Sources: Foveon's public description of the X3F format (the header, the
//! directory, the sections and the property list), ExifTool's SigmaRaw tag
//! names, and our own measurements on raw.pixls.us's X3F files from the SD9
//! to the sd Quattro H (TIME and the preview's DateTimeOriginal agree on
//! every file that has both).
use super::tiff::TiffFile;
use crate::captured::exif_date;
use crate::exif_tiffs::ExifTiffs;
use crate::io::Pieces;
use crate::{X3fMetadata, X3fProperties};

/// The most sections and properties read: files with more are malformed.
const MAX_SECTIONS: u64 = 64;
const MAX_PROPERTIES: u64 = 4096;
/// An image section's header, before its data.
const IMAGE_HEADER: u64 = 28;
/// How much of a JPEG preview is read for its EXIF, which an APP1 segment
/// of at most 64 KB holds, after a JFIF segment at most as long.
const JPEG_HEAD: u64 = 1 << 17;

/// Whether `bytes` is an X3F file.
pub(crate) fn is_x3f(bytes: &[u8]) -> bool {
    bytes.starts_with(b"FOVb")
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// The camera's EXIF: the TIFF of the first JPEG preview with EXIF, the
/// directory and the image sections' starts read alone.
pub(crate) fn exif_tiffs(f: &mut Pieces) -> ExifTiffs {
    ExifTiffs {
        main: preview_exif(f, false).flatten(),
        ..Default::default()
    }
}

/// What RAWmakase reads of an X3F, which it doesn't decode: its preview's
/// EXIF and its property list; `None` when a section holding them can't
/// be read (a file still being copied), so that none is taken for absent.
pub(crate) fn metadata(f: &mut Pieces) -> Option<X3fMetadata> {
    Some(X3fMetadata {
        exif: preview_exif(f, true)?,
        properties: properties(f)?,
    })
}

/// The TIFF of the first JPEG preview with EXIF; `Some(None)` for a file
/// with none. `None` when the directory can't be read, or, when `strict`,
/// an image section's start; otherwise such a section gives way to the
/// next.
fn preview_exif(f: &mut Pieces, strict: bool) -> Option<Option<Vec<u8>>> {
    for (at, len, kind) in directory(f)? {
        if !matches!(&kind, b"IMAG" | b"IMA2") || len <= IMAGE_HEADER {
            continue;
        }
        // The section's start, all in the file: longer than its header.
        let take = (IMAGE_HEADER + JPEG_HEAD).min(len);
        let Some(head) = f.read(at, take) else {
            if strict {
                return None;
            }
            continue;
        };
        if head.starts_with(b"SECi")
            && let Some(exif) = super::tiff_exif::in_jpeg(&head, IMAGE_HEADER, take - IMAGE_HEADER)
        {
            let at = usize::try_from(exif.start).unwrap_or(usize::MAX);
            let end = usize::try_from(exif.end)
                .unwrap_or(usize::MAX)
                .min(head.len());
            // A preview whose EXIF is no TIFF gives way to the next.
            if let Some(tiff) = head.get(at..end)
                && TiffFile::parse(tiff, 0).is_some()
            {
                return Some(Some(tiff.to_vec()));
            }
        }
    }
    Some(None)
}

/// When the photo was taken, as the property list's TIME says: seconds
/// since 1970 on the camera's clock, as EXIF writes a date. A list that
/// can't be read gives way to the next.
pub(crate) fn time(f: &mut Pieces) -> Option<String> {
    let seconds = |v: &str| v.parse::<u64>().ok();
    let [time] = find(f, ["TIME"], |_, v| seconds(v).is_some(), false)?;
    exif_date(seconds(&time?)?)
}

/// What the property list says of the camera and the exposure, for the
/// bodies whose preview has no EXIF (the SD9's and SD10's); `None` for a
/// file whose directory or property lists can't be read (one still being
/// copied), as for one that is no X3F. The lens is a number (LENSMODEL),
/// named nowhere in the file, so it is not read.
fn properties(f: &mut Pieces) -> Option<X3fProperties> {
    let positive = |v: &str| v.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.);
    // The make and model are text, the others positive numbers.
    let [make, model, shutter, exposure, aperture, iso, focal] = find(
        f,
        [
            "CAMMANUF", "CAMMODEL", "SHUTTER", "EXPTIME", "APERTURE", "ISO", "FLENGTH",
        ],
        |i, v| i < 2 || positive(v).is_some(),
        true,
    )?;
    let number = |v: Option<String>| positive(&v?);
    Some(X3fProperties {
        make: make.unwrap_or_default(),
        model: model.unwrap_or_default(),
        // SHUTTER in seconds where the body writes it, else EXPTIME, the
        // integration time, in microseconds.
        exposure: number(shutter).or_else(|| Some(number(exposure)? / 1e6)),
        aperture: number(aperture),
        iso: number(iso),
        focal: number(focal),
    })
}

/// The directory's sections: offset, length and type.
fn directory(f: &mut Pieces) -> Option<Vec<(u64, u64, [u8; 4])>> {
    let at = u64::from(u32_at(&f.read(f.len.checked_sub(4)?, 4)?, 0)?);
    let head = f.read(at, 12)?;
    if !head.starts_with(b"SECd") {
        return None;
    }
    let count = u64::from(u32_at(&head, 8)?).min(MAX_SECTIONS);
    let entries = f.read(at + 12, 12 * count)?;
    Some(
        entries
            .as_chunks::<12>()
            .0
            .iter()
            .filter_map(|e| {
                let kind = e[8..12].try_into().ok()?;
                Some((u64::from(u32_at(e, 0)?), u64::from(u32_at(e, 4)?), kind))
            })
            .collect(),
    )
}

/// The longest property value read, in characters: numbers and names.
const MAX_VALUE: usize = 64;

/// The values of `names`, each trimmed, not empty and one `valid` takes
/// (given the name's index), from the first property that has one, in the
/// directory's order, each list read in turn. `None` when the directory
/// can't be read, or, when `strict`, a property list. Only these values are
/// decoded.
fn find<const N: usize>(
    f: &mut Pieces,
    names: [&str; N],
    valid: impl Fn(usize, &str) -> bool,
    strict: bool,
) -> Option<[Option<String>; N]> {
    let mut found: [Option<String>; N] = std::array::from_fn(|_| None);
    for (at, len, kind) in directory(f)? {
        if &kind != b"PROP" {
            continue;
        }
        let Some(list) = PropertyList::read(f, at, len) else {
            if strict {
                return None;
            }
            continue;
        };
        for (i, (name, value)) in names.iter().zip(&mut found).enumerate() {
            if value.is_none() {
                *value = list.get(name, |v| valid(i, v));
            }
        }
    }
    Some(found)
}

/// A property list: the mark, a version, the count, the text's format (0,
/// UTF-16), a reserved word and the text's length in characters, then each
/// property's name and value offsets in characters, then the text.
struct PropertyList {
    section: Vec<u8>,
    count: usize,
    /// Where the text starts in `section`.
    text: usize,
}

impl PropertyList {
    fn read(f: &mut Pieces, at: u64, len: u64) -> Option<Self> {
        let section = f.read(at, len)?;
        let count = u64::from(u32_at(&section, 8)?);
        if !section.starts_with(b"SECp") || u32_at(&section, 12)? != 0 || count > MAX_PROPERTIES {
            return None;
        }
        let text = usize::try_from(24 + 8 * count).ok()?;
        (text <= section.len()).then_some(Self {
            section,
            count: count as usize,
            text,
        })
    }
    /// The character at `index` of the text.
    fn char_at(&self, index: usize) -> Option<u16> {
        let at = self.text.checked_add(index.checked_mul(2)?)?;
        Some(u16::from_le_bytes(
            self.section.get(at..at.checked_add(2)?)?.try_into().ok()?,
        ))
    }
    /// The value of the first property named `name` that `valid` takes,
    /// trimmed; none that is empty or longer than [`MAX_VALUE`].
    fn get(&self, name: &str, valid: impl Fn(&str) -> bool) -> Option<String> {
        (0..self.count).find_map(|i| {
            let entry = 24 + 8 * i;
            let at = usize::try_from(u32_at(&self.section, entry)?).ok()?;
            // The name, compared where it is.
            let named = name
                .encode_utf16()
                .chain([0])
                .enumerate()
                .all(|(k, c)| self.char_at(at + k) == Some(c));
            if !named {
                return None;
            }
            let at = usize::try_from(u32_at(&self.section, entry + 4)?).ok()?;
            // Its length first: a value too long is passed over unread.
            let len = (0..=MAX_VALUE)
                .take_while(|&k| self.char_at(at + k).is_some_and(|c| c != 0))
                .count();
            if len > MAX_VALUE || self.char_at(at + len) != Some(0) {
                return None;
            }
            let value: Vec<u16> = (0..len).filter_map(|k| self.char_at(at + k)).collect();
            Some(String::from_utf16(&value).ok()?.trim().to_string())
                .filter(|v| !v.is_empty() && valid(v))
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::Stamp;
    use std::io::Cursor;

    /// An X3F's properties, as `x3f_metadata` reads them.
    fn properties(file: &mut (impl std::io::Read + std::io::Seek)) -> Option<crate::X3fProperties> {
        crate::x3f_metadata(file).map(|m| m.properties)
    }

    /// A property list of `properties`.
    fn prop(properties: &[(&str, &str)]) -> Vec<u8> {
        let mut text: Vec<u16> = Vec::new();
        let mut entries = Vec::new();
        for (name, value) in properties {
            for s in [name, value] {
                entries.extend((text.len() as u32).to_le_bytes());
                text.extend(s.encode_utf16());
                text.push(0);
            }
        }
        let mut out = b"SECp".to_vec();
        for v in [
            0x0001_0000,
            properties.len() as u32,
            0,
            0,
            text.len() as u32,
        ] {
            out.extend(v.to_le_bytes());
        }
        out.extend(entries);
        out.extend(text.iter().flat_map(|c| c.to_le_bytes()));
        out
    }

    /// An image section of `data`.
    fn image(data: &[u8]) -> Vec<u8> {
        let mut out = b"SECi".to_vec();
        for v in [0x0001_0000u32, 2, 18, 4, 4, 0] {
            out.extend(v.to_le_bytes());
        }
        out.extend(data);
        out
    }

    /// A JPEG of a JFIF segment and the EXIF `tiff`.
    fn jpeg(tiff: &[u8]) -> Vec<u8> {
        let mut out = vec![0xff, 0xd8, 0xff, 0xe0, 0, 16];
        out.extend(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        out.extend([0xff, 0xe1]);
        out.extend(((tiff.len() + 8) as u16).to_be_bytes());
        out.extend(b"Exif\0\0");
        out.extend(tiff);
        out.extend([0xff, 0xd9]);
        out
    }

    /// An X3F of `sections`, typed, then the directory.
    fn x3f(sections: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut file = b"FOVb".to_vec();
        file.extend(0x0002_0003u32.to_le_bytes());
        file.resize(232, 0);
        let mut entries = Vec::new();
        for (kind, body) in sections {
            entries.extend((file.len() as u32).to_le_bytes());
            entries.extend((body.len() as u32).to_le_bytes());
            entries.extend(*kind);
            file.extend(body);
        }
        let dir = file.len() as u32;
        file.extend(b"SECd");
        file.extend(0x0002_0000u32.to_le_bytes());
        file.extend((sections.len() as u32).to_le_bytes());
        file.extend(entries);
        file.extend(dir.to_le_bytes());
        file
    }

    fn stamp(date: &str, subseconds: Option<&str>) -> Stamp {
        Stamp {
            date: Some(date.into()),
            subseconds: subseconds.map(Into::into),
        }
    }

    #[test]
    fn the_preview_s_exif_dates_an_x3f_and_time_stands_in_for_its_original() {
        use super::super::test_tiff::{E, ascii, tiff};
        use crate::io::Order;
        let exif = tiff(
            b"II*\0",
            Order::Little,
            vec![ascii(271, "SIGMA"), ascii(306, "2021:09:04 17:20:46")],
            vec![
                ascii(36867, "2021:09:04 17:20:45"),
                ascii(36868, "2021:09:04 17:20:44"),
                ascii(37521, "5"),
                E(37520, 2, b"7\0".to_vec()),
            ],
            &[],
        );
        let raw = image(&[0; 64]);
        // An SD1 Merrill's: the preview, then the properties. Without a
        // date in the EXIF, TIME is the original.
        let file = x3f(&[
            (b"IMA2", raw.clone()),
            (b"IMA2", image(&jpeg(&exif))),
            (
                b"PROP",
                prop(&[("CAMMODEL", "SD1"), ("TIME", "1630776000")]),
            ),
        ]);
        let dates = crate::capture_dates(&mut Cursor::new(&file));
        assert_eq!(dates.original, stamp("2021:09:04 17:20:45", Some("5")));
        // The preview's EXIF is the camera's.
        assert_eq!(
            crate::exif_tiffs(&mut Cursor::new(&file)).main,
            Some(exif.clone())
        );
        assert_eq!(dates.digitized, stamp("2021:09:04 17:20:44", None));
        assert_eq!(dates.modified, stamp("2021:09:04 17:20:46", Some("7")));
        // An SD9's: no preview, TIME alone.
        let file = x3f(&[
            (b"IMAG", raw),
            (
                b"PROP",
                prop(&[("CAMMANUF", "SIGMA"), ("TIME", "1050151205")]),
            ),
        ]);
        let dates = crate::capture_dates(&mut Cursor::new(&file));
        assert_eq!(dates.original, stamp("2003:04:12 12:40:05", None));
        assert_eq!(dates.digitized, Stamp::default());
        assert_eq!(
            crate::exif_tiffs(&mut Cursor::new(&file)),
            crate::ExifTiffs::default()
        );
        // A preview whose EXIF is no TIFF gives way to the next one's.
        let file = x3f(&[
            (b"IMA2", image(&jpeg(b"not a TIFF"))),
            (b"IMA2", image(&jpeg(&exif))),
        ]);
        let dates = crate::capture_dates(&mut Cursor::new(&file));
        assert_eq!(dates.original, stamp("2021:09:04 17:20:45", Some("5")));
    }

    /// The camera and exposure of an SD9's and an SD10's property list, the
    /// SD10's SHUTTER over its integration time.
    #[test]
    fn the_property_list_names_the_camera_and_its_exposure() {
        let sd9 = x3f(&[(
            b"PROP",
            prop(&[
                ("ISO", "100"),
                ("CAMMANUF", "SIGMA"),
                ("CAMMODEL", "SIGMA SD9"),
                ("EXPTIME", "41408"),
                ("APERTURE", "5.18736"),
                ("FLENGTH", "53"),
                ("LENSMODEL", "16"),
            ]),
        )]);
        assert_eq!(
            properties(&mut Cursor::new(&sd9)),
            Some(crate::X3fProperties {
                make: "SIGMA".into(),
                model: "SIGMA SD9".into(),
                exposure: Some(0.041408),
                aperture: Some(5.18736),
                iso: Some(100.),
                focal: Some(53.),
            })
        );
        let sd10 = x3f(&[(
            b"PROP",
            prop(&[
                ("EXPTIME", "20940"),
                ("SHUTTER", "0.006024"),
                ("APERTURE", " "),
                ("ISO", "0"),
            ]),
        )]);
        let p = properties(&mut Cursor::new(&sd10)).unwrap();
        assert_eq!(p.exposure, Some(0.006024));
        assert_eq!((p.aperture, p.iso), (None, None));
        // An X3F without a property list says nothing; another file is none.
        assert_eq!(
            properties(&mut Cursor::new(&x3f(&[]))),
            Some(Default::default())
        );
        assert_eq!(properties(&mut Cursor::new(b"II*\0")), None);
    }

    /// Reads fail within `section`, as on a bad sector or a drive gone.
    struct SectionFails {
        file: Cursor<Vec<u8>>,
        section: std::ops::Range<u64>,
    }
    impl std::io::Read for SectionFails {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let at = self.file.position();
            if at < self.section.end && at + buf.len() as u64 > self.section.start {
                return Err(std::io::Error::other("unreadable"));
            }
            self.file.read(buf)
        }
    }
    impl std::io::Seek for SectionFails {
        fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
            self.file.seek(to)
        }
    }

    /// A property list that can't be read leaves the properties unknown,
    /// not empty, so the photo's info is read again later; its capture
    /// time gives way, as before (#671).
    #[test]
    fn unreadable_properties_are_unknown_not_empty() {
        let list = prop(&[("CAMMANUF", "SIGMA"), ("TIME", "1050151205")]);
        let file = x3f(&[(b"PROP", list)]);
        assert_eq!(
            properties(&mut Cursor::new(&file)).map(|p| p.make),
            Some("SIGMA".into())
        );
        // The section, after the 232-byte header and before the directory
        // of one entry (28 bytes).
        let section = 232..file.len() as u64 - 28;
        let mut failing = SectionFails {
            file: Cursor::new(file.clone()),
            section: section.clone(),
        };
        assert_eq!(properties(&mut failing), None);
        let mut failing = SectionFails {
            file: Cursor::new(file.clone()),
            section,
        };
        assert_eq!(
            crate::capture_dates(&mut failing),
            crate::CaptureDates::default()
        );
        // A section said to run past the file's end: its length is the
        // directory entry's second word.
        let mut short = file;
        let at = short.len() - 12;
        short[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(properties(&mut Cursor::new(&short)), None);
    }

    /// A preview that can't be read leaves the X3F's metadata unknown,
    /// though its property list reads: the property list alone would name
    /// no lens (#671).
    #[test]
    fn an_unreadable_preview_leaves_the_metadata_unknown() {
        use super::super::test_tiff::{ascii, tiff};
        let exif = tiff(
            b"II*\0",
            crate::io::Order::Little,
            vec![ascii(271, "SIGMA")],
            vec![ascii(42036, "30mm F2.8")],
            &[],
        );
        let preview = image(&jpeg(&exif));
        let file = x3f(&[
            (b"IMA2", preview.clone()),
            (b"PROP", prop(&[("CAMMANUF", "SIGMA")])),
        ]);
        let read = crate::x3f_metadata(&mut Cursor::new(&file)).unwrap();
        assert_eq!(
            (read.exif, read.properties.make.as_str()),
            (Some(exif), "SIGMA")
        );
        let mut failing = SectionFails {
            file: Cursor::new(file.clone()),
            section: 232..232 + preview.len() as u64,
        };
        assert_eq!(crate::x3f_metadata(&mut failing), None);
        // The camera's EXIF for export and dates still reads what it can.
        let mut failing = SectionFails {
            file: Cursor::new(file),
            section: 232..232 + preview.len() as u64,
        };
        assert_eq!(crate::exif_tiffs(&mut failing), crate::ExifTiffs::default());
    }

    /// A value that isn't one gives way to the next property's, in the
    /// same list or a later one, as #665's TIME did (#671).
    #[test]
    fn a_value_that_is_no_number_gives_way_to_the_next() {
        let file = x3f(&[
            (b"PROP", prop(&[("TIME", "noon"), ("APERTURE", "wide")])),
            (
                b"PROP",
                prop(&[("TIME", "?"), ("TIME", "1050151205"), ("APERTURE", "5.6")]),
            ),
        ]);
        assert_eq!(
            crate::capture_dates(&mut Cursor::new(&file)).original,
            stamp("2003:04:12 12:40:05", None)
        );
        assert_eq!(
            properties(&mut Cursor::new(&file)).and_then(|p| p.aperture),
            Some(5.6)
        );
    }

    /// Only the values asked for are decoded, each at most 64 characters:
    /// thousands of properties naming one long value cost no more than
    /// the list itself (#671).
    #[test]
    fn many_properties_naming_one_long_value_decode_nothing() {
        let long: String = "9".repeat(100_000);
        let mut named = vec![("CAMMODEL", long.as_str())];
        named.extend(vec![("CAMMODEL", "x"); 4000]);
        named.push(("CAMMANUF", "SIGMA"));
        let mut list = prop(&named);
        // Every model's value offset points at the long one.
        let first = u32::from_le_bytes(list[28..32].try_into().unwrap());
        for i in 1..=4000 {
            let at = 24 + 8 * i + 4;
            list[at..at + 4].copy_from_slice(&first.to_le_bytes());
        }
        let file = x3f(&[(b"PROP", list)]);
        let p = properties(&mut Cursor::new(&file)).unwrap();
        assert_eq!((p.make.as_str(), p.model.as_str()), ("SIGMA", ""));
    }

    #[test]
    fn damaged_x3f_files_have_no_dates_and_do_not_panic() {
        // An image section said to be 100 bytes, 4 bytes from the end.
        let short: Vec<u8> = [
            0x6256_4f46u32,
            0x6443_4553,
            0x0002_0000,
            1,
            28,
            100,
            u32::from_le_bytes(*b"IMAG"),
            4,
        ]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
        assert_eq!(
            crate::capture_dates(&mut Cursor::new(&short)),
            crate::CaptureDates::default()
        );
        let exif = super::super::test_tiff::tiff(
            b"II*\0",
            crate::io::Order::Little,
            vec![super::super::test_tiff::ascii(306, "2021:09:04 17:20:46")],
            Vec::new(),
            &[],
        );
        let file = x3f(&[
            (b"IMA2", image(&jpeg(&exif))),
            (b"PROP", prop(&[("TIME", "1050151205")])),
        ]);
        let good = crate::capture_dates(&mut Cursor::new(&file));
        assert!(good.original.date.is_some() && good.modified.date.is_some());
        for end in 0..file.len() {
            let _ = crate::capture_dates(&mut Cursor::new(&file[..end]));
            let _ = crate::exif_tiffs(&mut Cursor::new(&file[..end]));
            let _ = properties(&mut Cursor::new(&file[..end]));
        }
        for i in 0..file.len() {
            for v in [0, 0x7f, 0xff] {
                let mut d = file.clone();
                d[i] = v;
                let _ = crate::capture_dates(&mut Cursor::new(&d));
                let _ = crate::exif_tiffs(&mut Cursor::new(&d));
                let _ = properties(&mut Cursor::new(&d));
            }
        }
        // A TIME that is not a number is none.
        let file = x3f(&[(b"PROP", prop(&[("TIME", "noon")]))]);
        assert_eq!(
            crate::capture_dates(&mut Cursor::new(&file)),
            crate::CaptureDates::default()
        );
    }
}
