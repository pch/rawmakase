//! Where a photo falls in a sequence its camera shot (a burst in continuous
//! drive, a bracket), as the camera's maker note says: for Auto-Stack's
//! Group Camera Bursts. Read from public tag documentation (ExifTool's tag
//! names and tables, the DNG specification):
//!
//! - Sony's SequenceNumber (0xb04a), Fujifilm's (0x1101) and Panasonic's
//!   (0x002b) number the frames of each press from 1, one taken alone in
//!   continuous drive included, and are 0 for a photo taken in single drive.
//! - Apple's BurstUUID (0x000b) gives a burst's photos one id.
//! - Canon's ContinuousDrive (CameraSettings 0x0001, value 5) and
//!   BracketMode (FileInfo 0x0093, value 3), Nikon's ShootingMode (0x0089)
//!   and Olympus's DriveMode (CameraSettings 0x2020, tag 0x0600) say the drive
//!   mode. Canon's and Nikon's number no frames (Canon's SequenceNumber is 0 in
//!   most files); Olympus's numbers some.
//!
//! A DNG converted by Adobe keeps the camera's maker note in its
//! DNGPrivateData ("Adobe", then a "MakN" block with the note's byte order
//! and where it was in its file), or, converted from a Panasonic RW2, the
//! RW2's first directory in a "Pano" block, whose JpgFromRaw holds it. A CR3
//! keeps it in its `CMT3` box.
//! Older Sony bodies (DSLR-A, NEX, SLT, some RX) write no SequenceNumber and
//! keep their drive mode in model-specific blocks; they are not read.
use super::{
    CameraExif, Field, Source, Tiff, camera_exif, exif_in_jpeg, open_tiff, read_tiffs,
    tag::{BODY_SERIAL_NUMBER, EXIF_IFD, MAKE, MAKER_NOTE, MODEL},
};
use crate::isobmff::{BoxHeader, boxes, file_len, read_body};
use crate::tiff::{
    Entry,
    kind::{ASCII, BYTE, IFD, LONG, SHORT, SSHORT},
};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

/// A photo's place in its camera's sequences.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Sequence {
    /// The camera that numbered it: make, model and body serial number when
    /// the EXIF has one. Two bodies' frame numbers are never compared.
    pub camera: String,
    pub mark: Mark,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Mark {
    /// One of the photos of a burst, which share this id.
    Burst(String),
    /// The `n`th frame (from 1) of a press of the shutter in continuous
    /// drive or bracketing.
    Frame(u32),
    /// Taken in continuous drive or bracketing, the frame not numbered: which
    /// press it came from is left to its capture time.
    Continuous,
    /// Taken in single drive: of no sequence.
    Single,
}

/// The version of what [`sequence`] reads. A catalog keeps what it read
/// with the version, and reads the file again under another: one more
/// camera's notes read, the files that said nothing may now say something.
pub const SEQUENCE_VERSION: u32 = 2;

/// The sequence the photo at `path` is in, as its maker note says; `None`
/// when the file can't be read, has no maker note this reads, or the note
/// doesn't say.
pub fn sequence(path: &Path) -> Option<Sequence> {
    if is_cr3(path)? {
        return in_cr3(path);
    }
    match open_tiff(path)? {
        Source::File(t) => sequence_of(t)
            .or_else(|| in_jpg_from_raw(path))
            .or_else(|| in_adobe_pano(path)),
        Source::Memory(t) => sequence_of(t),
    }
}

/// Whether the file at `path` is a CR3; `None` when it can't be read.
fn is_cr3(path: &Path) -> Option<bool> {
    let mut head = Vec::with_capacity(12);
    File::open(path)
        .ok()?
        .take(12)
        .read_to_end(&mut head)
        .ok()?;
    Some(head.get(4..12) == Some(b"ftypcrx "))
}

/// [`sequence`] of a Panasonic RW2, which keeps its maker note in the EXIF
/// of the JPEG its first directory holds (JpgFromRaw, 0x002e), not in its
/// own EXIF directory.
fn in_jpg_from_raw(path: &Path) -> Option<Sequence> {
    let mut t = Tiff::open(File::open(path).ok()?, 0)?;
    let main = t.ifd(t.first)?;
    let e = main.get(&JPG_FROM_RAW)?;
    in_jpeg(path, t.u32(&e.value) as u64, e.count as u64)
}

/// [`sequence`] of the EXIF of the JPEG of `len` bytes `at` the start of
/// the file at `path`; none when its EXIF segment says it runs past them.
fn in_jpeg(path: &Path, at: u64, len: u64) -> Option<Sequence> {
    let (exif, exif_len) =
        exif_in_jpeg(&mut File::open(path).ok()?, at, usize::try_from(len).ok()?)?;
    if exif.checked_add(exif_len)? > len {
        return None;
    }
    sequence_of(Tiff::open_within(
        File::open(path).ok()?,
        at + exif,
        exif_len,
    )?)
}

/// [`sequence`] of a DNG Adobe converted from a Panasonic RW2, which keeps
/// the RW2's first directory in a "Pano" block of its private data: its byte
/// order, a big-endian count, then entries of a big-endian tag, type and
/// count, each followed by its value in that byte order. Its JpgFromRaw
/// holds the camera's EXIF, as in the RW2.
fn in_adobe_pano(path: &Path) -> Option<Sequence> {
    let mut t = Tiff::open(File::open(path).ok()?, 0)?;
    let main = t.ifd(t.first)?;
    let (start, size) = dng_block(&mut t, main.get(&DNG_PRIVATE_DATA)?, b"Pano")?;
    let end = start + size;
    let head = t.bytes(start, 4)?;
    if !matches!(&head[..2], b"II" | b"MM") {
        return None;
    }
    let count = u16::from_be_bytes([head[2], head[3]]);
    let mut at = start + 4;
    for _ in 0..count.min(MOST_PANO_ENTRIES) {
        if at.checked_add(8)? > end {
            return None;
        }
        let e = t.bytes(at, 8)?;
        let tag = u16::from_be_bytes([e[0], e[1]]);
        let entry = Entry {
            kind: u16::from_be_bytes([e[2], e[3]]),
            count: u32::from_be_bytes(e[4..8].try_into().ok()?),
            value: [0; 4],
        };
        let len = (entry.count as u64).checked_mul(entry.type_size()? as u64)?;
        let value = at + 8;
        if value.checked_add(len)? > end {
            return None;
        }
        if tag == JPG_FROM_RAW {
            return in_jpeg(path, value, len);
        }
        at = value + len;
    }
    None
}

/// How many entries of a "Pano" block are looked at for its JpgFromRaw: an
/// RW2's first directory has some fifty.
const MOST_PANO_ENTRIES: u16 = 256;

/// A Panasonic RW2's preview JPEG, with the camera's whole EXIF.
const JPG_FROM_RAW: u16 = 0x002e;

/// A DNG's private data (0xc634) in its first directory.
const DNG_PRIVATE_DATA: u16 = 0xc634;

/// How many bytes before the maker note Camera Raw copied some DNGs' "MakN"
/// blocks from (ExifTool documents its "00 00 00 01" form; others have
/// other bytes there).
const MISPLACED_MAKN: u64 = 12;

/// [`sequence`] of the EXIF in `t`: its maker note, or the one an Adobe
/// DNG keeps in its private data.
fn sequence_of<R: Read + Seek>(mut t: Tiff<R>) -> Option<Sequence> {
    let main = t.ifd(t.first)?;
    let make = main
        .get(&MAKE)
        .filter(|e| e.kind == ASCII)
        .and_then(|e| t.raw(e))
        .map(|b| text(&b))?;
    let in_exif = main
        .get(&EXIF_IFD)
        .and_then(|e| t.offset(e))
        .and_then(|at| t.ifd(at))
        .and_then(|exif| {
            let e = exif.get(&MAKER_NOTE)?;
            let len = (e.count as u64).checked_mul(e.type_size()? as u64)?;
            (len > 4).then(|| (t.u32(&e.value) as u64, len))
        });
    let mark = match in_exif {
        // Read in its TIFF, where what it points to may lie outside it (an
        // older Olympus's), its offsets counting from the TIFF's start.
        Some((at, len)) => maker_mark(
            &make,
            &mut Note {
                tiff: t.part(0, None, t.little)?,
                start: at,
                len,
                origin: at,
            },
        ),
        None => {
            let (at, len, origin) = in_dng_private_data(&mut t, main.get(&DNG_PRIVATE_DATA)?)?;
            let mut note = |skip: u64| {
                maker_mark(
                    &make,
                    &mut Note {
                        tiff: t.part(at + skip, Some(len.checked_sub(skip)?), t.little)?,
                        start: 0,
                        len: len - skip,
                        origin: origin + skip,
                    },
                )
            };
            // Camera Raw has copied some notes from 12 bytes before them.
            note(0).or_else(|| note(MISPLACED_MAKN))
        }
    }?;
    let exif = camera_exif(t, |tag| tag == MAKE || tag == MODEL)?;
    Some(Sequence {
        camera: camera(&exif),
        mark,
    })
}

/// Where in `t` the maker note an Adobe DNG keeps in its private data `e`
/// is, how long it is, and where it was in the file it came from: a "MakN"
/// block holds the note's byte order, its big-endian offset in its original
/// file, then the note.
fn in_dng_private_data<R: Read + Seek>(t: &mut Tiff<R>, e: &Entry) -> Option<(u64, u64, u64)> {
    let (body, size) = dng_block(t, e, b"MakN")?;
    let head = t.bytes(body, 6)?;
    let origin = u32::from_be_bytes(head[2..6].try_into().ok()?) as u64;
    Some((body + 6, size.checked_sub(6)?, origin))
}

/// Where in `t` the body of the first `kind` block of an Adobe DNG's private
/// data `e` is, and how long: the private data is "Adobe\0", then blocks of
/// a four-letter kind and a big-endian length, each padded to an even length
/// (at most [`MOST_DNG_BLOCKS`] are looked at).
fn dng_block<R: Read + Seek>(t: &mut Tiff<R>, e: &Entry, kind: &[u8; 4]) -> Option<(u64, u64)> {
    let len = e.count as u64;
    let start = if len > 4 {
        t.u32(&e.value) as u64
    } else {
        return None;
    };
    if t.bytes(start, 6)? != b"Adobe\0" {
        return None;
    }
    let end = start.checked_add(len)?;
    let mut at = start + 6;
    for _ in 0..MOST_DNG_BLOCKS {
        if at.checked_add(8)? > end {
            return None;
        }
        let header = t.bytes(at, 8)?;
        let size = u32::from_be_bytes(header[4..8].try_into().ok()?) as u64;
        let body = at + 8;
        if body.checked_add(size)? > end {
            return None;
        }
        if header[..4] == kind[..] {
            return Some((body, size));
        }
        at = body + size + (size & 1);
    }
    None
}

/// How many blocks of a DNG's private data are looked at: Adobe writes a
/// few.
const MOST_DNG_BLOCKS: usize = 64;

/// [`sequence`] of a CR3: make and model in the TIFF in its `CMT1` box, the
/// EXIF directory in `CMT2`'s, and the maker note as the first directory of
/// `CMT3`'s, its offsets from that TIFF's start; all three in the "uuid" box
/// of Canon's id in its "moov" box.
fn in_cr3(path: &Path) -> Option<Sequence> {
    let mut f = File::open(path).ok()?;
    let end = file_len(&mut f)?;
    let moov = boxes(&mut f, 0, end)
        .into_iter()
        .find(|b| &b.kind == b"moov")?;
    let canon = boxes(&mut f, moov.start, moov.start + moov.size)
        .into_iter()
        .filter(|b| &b.kind == b"uuid" && b.size >= 16)
        .find(|b| {
            let id = read_body(
                &mut f,
                &BoxHeader {
                    size: 16,
                    ..b.clone()
                },
            );
            id.as_deref() == Some(&CANON_UUID[..])
        })?;
    let inner = boxes(&mut f, canon.start + 16, canon.start + canon.size);
    let find = |kind: &[u8; 4]| inner.iter().find(|b| &b.kind == kind);
    let cmt1 = read_body(&mut f, find(b"CMT1")?)?;
    let cmt2 = find(b"CMT2").and_then(|b| read_body(&mut f, b));
    let exif = read_tiffs(Some(&cmt1), cmt2.as_deref(), None)?;
    let make = exif.get(MAKE).and_then(Field::text)?;
    let cmt3 = find(b"CMT3")?;
    let mut t = Tiff::open_within(f, cmt3.start, cmt3.size)?;
    let first = t.first;
    let mark = maker_mark(
        &make,
        &mut Note {
            tiff: t.part(0, None, t.little)?,
            start: first,
            len: cmt3.size.checked_sub(first)?,
            origin: first,
        },
    )?;
    Some(Sequence {
        camera: camera(&exif),
        mark,
    })
}

/// The id of a CR3's "uuid" box of Canon's metadata.
const CANON_UUID: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];

/// Make, model and body serial number, a line each.
fn camera(exif: &CameraExif) -> String {
    let text = |tag| exif.get(tag).and_then(Field::text).unwrap_or_default();
    [text(MAKE), text(MODEL), text(BODY_SERIAL_NUMBER)].join("\n")
}

/// An ASCII value as [`Field::text`] reads it.
fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b)
        .trim_end_matches('\0')
        .trim()
        .to_string()
}

/// A maker note, read in place: its directories and values are read as
/// they are asked for, never the whole note.
struct Note<'a, R> {
    /// What the note is read in: its file's TIFF, or its own bytes alone.
    tiff: Tiff<&'a mut R>,
    /// Where in that the note starts, and how long it is.
    start: u64,
    len: u64,
    /// Where the note starts in what offsets in it count from when they
    /// count from its file's TIFF (Canon's, older Olympus's and Nikon's):
    /// its offset in that TIFF, or in the file an Adobe DNG's note was
    /// copied from.
    origin: u64,
}

/// A directory of a maker note.
struct Directory {
    entries: BTreeMap<u16, Entry>,
    little: bool,
    /// Added to an offset in it for the place in the note it points to.
    shift: i64,
}

impl<R: Read + Seek> Note<'_, R> {
    /// Up to `n` bytes from the note's start.
    fn head(&mut self, n: u64) -> Option<Vec<u8>> {
        self.tiff.bytes(self.start, n.min(self.len) as usize)
    }

    /// The shift of offsets that count from `at` bytes into the note.
    fn shift_from(&self, at: u64) -> Option<i64> {
        i64::try_from(self.start.checked_add(at)?).ok()
    }

    /// The shift of offsets that count from its file's TIFF.
    fn tiff_shift(&self) -> Option<i64> {
        i64::try_from(self.start)
            .ok()?
            .checked_sub(i64::try_from(self.origin).ok()?)
    }

    /// The directory `at` bytes into the note, `shift` added to its offsets
    /// for where they point in what the note is read in; little-endian or
    /// not as `little` says, or, when `None`, as its entry count tells, a
    /// count being under 256. One with an entry of no TIFF type (but 0) is
    /// none: not a directory.
    fn directory(&mut self, at: u64, little: Option<bool>, shift: i64) -> Option<Directory> {
        let at = self.start.checked_add(at)?;
        self.directory_at(at, little, shift)
    }

    /// [`Note::directory`] at `at` in what the note is read in.
    fn directory_at(&mut self, at: u64, little: Option<bool>, shift: i64) -> Option<Directory> {
        let little = match little {
            Some(little) => little,
            None => {
                let count = self.tiff.bytes(at, 2)?;
                count[0] != 0 || count[1] == 0
            }
        };
        self.tiff.little = little;
        let entries = self.tiff.ifd(at)?;
        // Canon pads its directory with an entry of type 0.
        if entries
            .values()
            .any(|e| e.kind != 0 && e.type_size().is_none())
        {
            return None;
        }
        Some(Directory {
            entries,
            little,
            shift,
        })
    }

    /// Where in what the note is read in `offset` from directory `d` points.
    fn at(d: &Directory, offset: u32) -> Option<u64> {
        u64::try_from(i64::from(offset).checked_add(d.shift)?).ok()
    }

    /// The directory entry `tag` of `d` points to: a sub-directory, or one
    /// held in its value (in the note's byte order, or as its count tells
    /// when it is not a pointer).
    fn sub(&mut self, d: &Directory, tag: u16) -> Option<Directory> {
        let e = d.entries.get(&tag)?;
        self.tiff.little = d.little;
        let at = Self::at(d, self.tiff.u32(&e.value))?;
        let little = matches!(e.kind, LONG | IFD).then_some(d.little);
        self.directory_at(at, little, d.shift)
    }

    /// The values of BYTE, SHORT, SSHORT or LONG entry `tag` of `d`.
    fn ints(&mut self, d: &Directory, tag: u16) -> Option<Vec<u32>> {
        let e = d.entries.get(&tag)?;
        let size = e.type_size()?;
        let len = (e.count as usize).checked_mul(size)?;
        self.tiff.little = d.little;
        let b = if len <= 4 {
            e.value[..len].to_vec()
        } else {
            let at = Self::at(d, self.tiff.u32(&e.value))?;
            self.tiff.bytes(at, len)?
        };
        let t = &self.tiff;
        match e.kind {
            BYTE => Some(b.into_iter().map(u32::from).collect()),
            SHORT | SSHORT => Some(
                b.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| t.u16(c) as u32)
                    .collect(),
            ),
            LONG => Some(b.as_chunks::<4>().0.iter().map(|c| t.u32(c)).collect()),
            _ => None,
        }
    }

    /// The first value of BYTE, SHORT, SSHORT or LONG entry `tag` of `d`.
    fn int(&mut self, d: &Directory, tag: u16) -> Option<u32> {
        self.ints(d, tag)?.first().copied()
    }

    /// The text of ASCII entry `tag` of `d`, when there is any.
    fn text(&mut self, d: &Directory, tag: u16) -> Option<String> {
        let e = d.entries.get(&tag).filter(|e| e.kind == ASCII)?;
        self.tiff.little = d.little;
        let b = if e.count <= 4 {
            e.value[..e.count as usize].to_vec()
        } else {
            let at = Self::at(d, self.tiff.u32(&e.value))?;
            self.tiff.bytes(at, e.count as usize)?
        };
        Some(text(&b)).filter(|t| !t.is_empty())
    }
}

/// The mark in maker note `note` of a camera by `make`.
fn maker_mark<R: Read + Seek>(make: &str, note: &mut Note<'_, R>) -> Option<Mark> {
    let make = make.trim().to_ascii_uppercase();
    if make.starts_with("SONY") {
        sony(note)
    } else if make.starts_with("FUJIFILM") {
        fujifilm(note)
    } else if make.starts_with("PANASONIC") {
        panasonic(note)
    } else if make.starts_with("APPLE") {
        apple(note)
    } else if make.starts_with("CANON") {
        canon(note)
    } else if make.starts_with("NIKON") {
        nikon(note)
    } else if make.starts_with("OLYMPUS") || make.starts_with("OM DIGITAL") {
        olympus(note)
    } else {
        None
    }
}

/// A frame number, 0 for single drive.
fn frame(n: u32) -> Mark {
    if n == 0 { Mark::Single } else { Mark::Frame(n) }
}

/// Frame `n` of a sequence, or one not numbered when `n` is 0.
fn in_sequence(n: u32) -> Mark {
    if n == 0 {
        Mark::Continuous
    } else {
        Mark::Frame(n)
    }
}

/// Sony: a directory at the note's start (a RAW's), or after a 12-byte
/// "SONY DSC " or "SONY CAM " header (a JPEG's), in the EXIF's byte order,
/// which its entry count tells; another header isn't read.
/// SequenceNumber (0xb04a): 0 in single drive, 65535 when the camera doesn't
/// say.
fn sony<R: Read + Seek>(note: &mut Note<'_, R>) -> Option<Mark> {
    let head = note.head(12)?;
    let at = if head.starts_with(b"SONY DSC \0\0\0") || head.starts_with(b"SONY CAM \0\0\0") {
        12
    } else if head.starts_with(b"SONY") {
        return None;
    } else {
        0
    };
    let d = note.directory(at, None, note.shift_from(0)?)?;
    let n = note.int(&d, 0xb04a)?;
    (n != 0xffff).then(|| frame(n))
}

/// Fujifilm: "FUJIFILM", then the little-endian offset of its directory from
/// the note's start. SequenceNumber (0x1101).
fn fujifilm<R: Read + Seek>(note: &mut Note<'_, R>) -> Option<Mark> {
    let head = note.head(12)?;
    let header = head.strip_prefix(b"FUJIFILM")?;
    let at = u32::from_le_bytes(header.get(..4)?.try_into().ok()?) as u64;
    let d = note.directory(at, Some(true), note.shift_from(0)?)?;
    Some(frame(note.int(&d, 0x1101)?))
}

/// Panasonic: "Panasonic\0\0\0", then a directory in the EXIF's byte order.
/// SequenceNumber (0x002b).
fn panasonic<R: Read + Seek>(note: &mut Note<'_, R>) -> Option<Mark> {
    if !note.head(10)?.starts_with(b"Panasonic\0") {
        return None;
    }
    let d = note.directory(12, None, note.shift_from(0)?)?;
    Some(frame(note.int(&d, 0x002b)?))
}

/// Apple: "Apple iOS\0", a version, "MM", then a big-endian directory
/// whose offsets are from the note's start. BurstUUID (0x000b); a photo
/// without one is no burst's.
fn apple<R: Read + Seek>(note: &mut Note<'_, R>) -> Option<Mark> {
    let head = note.head(14)?;
    if !head.starts_with(b"Apple iOS\0") || head.get(12..14) != Some(b"MM") {
        return None;
    }
    let d = note.directory(14, Some(false), note.shift_from(0)?)?;
    Some(note.text(&d, 0x000b).map_or(Mark::Single, Mark::Burst))
}

/// Canon: a directory at the note's start, its offsets from its file's TIFF.
/// ContinuousDrive, value 5 of CameraSettings (0x0001): 0 single, 6 silent
/// single and 9 single silent; 1 continuous, 3 speed priority, 4 low, 5
/// high, 8 high+ and 10 silent continuous; 2, a movie, and others are not
/// read. BracketMode, value 3 of FileInfo (0x0093), other than 0 (off): a
/// bracket, in either drive mode. Both start with their length in bytes, which
/// tells them from what a misplaced note's offsets lead to.
fn canon<R: Read + Seek>(note: &mut Note<'_, R>) -> Option<Mark> {
    let d = note.directory(0, None, note.tiff_shift()?)?;
    let mut values = |tag| {
        note.ints(&d, tag)
            .filter(|v| v.first() == Some(&(2 * v.len() as u32)))
    };
    let bracket = values(0x0093).and_then(|v| v.get(3).copied());
    if bracket.is_some_and(|mode| mode != 0) {
        return Some(Mark::Continuous);
    }
    match values(0x0001)?.get(5)? {
        0 | 6 | 9 => Some(Mark::Single),
        1 | 3 | 4 | 5 | 8 | 10 => Some(Mark::Continuous),
        _ => None,
    }
}

/// Nikon: "Nikon\0\x02", then at byte 10 a TIFF header, the offsets in its
/// directories from it; or, in a D1's, a directory at the note's start, its
/// offsets from its file's TIFF. Other notes ("Nikon\0\x01") are not read. ShootingMode (0x0089), bit
/// flags: 0 continuous, 4 exposure, 6 white balance and 8 D-Lighting
/// bracketing, 11 pre-capture are a sequence; none but 2 (PC control) and 5
/// (Auto ISO) is single frame. Other flags (delay, self-timer, IR control)
/// don't say.
fn nikon<R: Read + Seek>(note: &mut Note<'_, R>) -> Option<Mark> {
    let head = note.head(18)?;
    let d = if head.starts_with(b"Nikon\0\x02") {
        let little = match head.get(10..14)? {
            b"II*\0" => true,
            b"MM\0*" => false,
            _ => return None,
        };
        note.tiff.little = little;
        let first = note.tiff.u32(head.get(14..18)?) as u64;
        note.directory(first.checked_add(10)?, Some(little), note.shift_from(10)?)?
    } else if head.starts_with(b"Nikon") {
        return None;
    } else {
        note.directory(0, None, note.tiff_shift()?)?
    };
    let mode = note.int(&d, 0x0089)?;
    if mode & (1 | 1 << 4 | 1 << 6 | 1 << 8 | 1 << 11) != 0 {
        Some(Mark::Continuous)
    } else if mode & !(1 << 2 | 1 << 5) == 0 {
        Some(Mark::Single)
    } else {
        None
    }
}

/// Olympus and OM System: "OLYMP\0" with a directory at byte 8, its offsets
/// from its file's TIFF; "OLYMPUS\0" and a byte order, the directory at byte
/// 12; "OM SYSTEM\0\0\0" and a byte order, at byte 16; the offsets of these
/// two from the note's start. DriveMode (0x0600) in CameraSettings (0x2020,
/// a directory it points to or holds) is two to six values: the mode, the
/// shot number, the bracketed settings' bit flags, -, the shutter, and (E-M1
/// Mark II and later) the shooting mode.
fn olympus<R: Read + Seek>(note: &mut Note<'_, R>) -> Option<Mark> {
    let head = note.head(16)?;
    let order = |b: Option<&[u8]>| match b? {
        b"II" => Some(true),
        b"MM" => Some(false),
        _ => None,
    };
    let d = if head.starts_with(b"OLYMP\0") {
        note.directory(8, None, note.tiff_shift()?)?
    } else if head.starts_with(b"OLYMPUS\0") {
        note.directory(12, Some(order(head.get(8..10))?), note.shift_from(0)?)?
    } else if head.starts_with(b"OM SYSTEM\0") {
        note.directory(16, Some(order(head.get(12..14))?), note.shift_from(0)?)?
    } else {
        return None;
    };
    let settings = note.sub(&d, 0x2020)?;
    olympus_drive(&note.ints(&settings, 0x0600)?)
}

/// The mark of Olympus DriveMode `values`, read in the order ExifTool's
/// table does: bracketing by bit flags (mode 5), then the shooting mode when
/// there is one, then the mode.
fn olympus_drive(values: &[u32]) -> Option<Mark> {
    let (mode, shot) = (*values.first()?, values.get(1).copied().unwrap_or(0));
    let shooting = values.get(5).copied().unwrap_or(0);
    if mode == 5 && values.len() > 2 {
        return Some(in_sequence(shot));
    }
    if shooting != 0 {
        return match shooting {
            // Single shot with each shutter, and High Res Shot, one photo
            // of many exposures.
            0x01 | 0x11 | 0x21 | 0x30 => Some(Mark::Single),
            // Sequential L, H, SH1, SH2 with each shutter, and Pro Capture.
            0x02 | 0x03 | 0x07 | 0x12 | 0x13 | 0x17 | 0x22 | 0x23 | 0x27 | 0x28 | 0x29 | 0x41
            | 0x42 | 0x43 | 0x48 | 0x49 => Some(in_sequence(shot)),
            _ => None,
        };
    }
    match mode {
        0 => Some(Mark::Single),
        // Continuous, and exposure, white balance or both bracketed.
        1..=4 => Some(in_sequence(shot)),
        _ => None,
    }
}

/// Files with maker notes, for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod fixture {
    /// A TIFF with Make, Model and an EXIF directory holding `note` as its
    /// maker note (and a body serial number when given), in either byte
    /// order.
    pub fn tiff(little: bool, make: &str, model: &str, serial: &str, note: &[u8]) -> Vec<u8> {
        let u16b = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32b = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let text = |s: &str| [s.as_bytes(), b"\0"].concat();
        let (make, model, serial) = (text(make), text(model), text(serial));
        let main_at = 8u32;
        let exif_at = main_at + 2 + 3 * 12 + 4;
        let data_at = exif_at + 2 + 2 * 12 + 4;
        let make_at = data_at;
        let model_at = make_at + make.len() as u32;
        let serial_at = model_at + model.len() as u32;
        let note_at = serial_at + serial.len() as u32;
        let mut t = if little {
            b"II*\0".to_vec()
        } else {
            b"MM\0*".to_vec()
        };
        t.extend(u32b(main_at));
        let entry = |t: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32| {
            t.extend(u16b(tag));
            t.extend(u16b(kind));
            t.extend(u32b(count));
            t.extend(u32b(value));
        };
        t.extend(u16b(3));
        entry(&mut t, 0x010f, 2, make.len() as u32, make_at);
        entry(&mut t, 0x0110, 2, model.len() as u32, model_at);
        entry(&mut t, 0x8769, 4, 1, exif_at);
        t.extend([0; 4]);
        t.extend(u16b(2));
        entry(&mut t, 0x927c, 7, note.len() as u32, note_at);
        // A value of four bytes or less is held in the entry itself.
        if serial.len() <= 4 {
            t.extend(u16b(0xa431));
            t.extend(u16b(2));
            t.extend(u32b(serial.len() as u32));
            let mut inline = serial.clone();
            inline.resize(4, 0);
            t.extend(inline);
        } else {
            entry(&mut t, 0xa431, 2, serial.len() as u32, serial_at);
        }
        t.extend([0; 4]);
        t.extend(make.iter().chain(&model).chain(&serial).chain(note));
        t
    }

    /// A maker-note directory of `entries` (tag, kind, count, and value or
    /// offset from the directory's start), then `data`.
    pub fn note_ifd(little: bool, entries: &[(u16, u16, u32, u32)], data: &[u8]) -> Vec<u8> {
        let u16b = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32b = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let mut d = u16b(entries.len() as u16).to_vec();
        for (tag, kind, count, value) in entries {
            d.extend(u16b(*tag));
            d.extend(u16b(*kind));
            d.extend(u32b(*count));
            // A SHORT is left-justified in its value field.
            if *kind == 3 && *count == 1 {
                d.extend(u16b(*value as u16));
                d.extend([0, 0]);
            } else {
                d.extend(u32b(*value));
            }
        }
        d.extend([0; 4]);
        d.extend(data);
        d
    }

    /// A Sony maker note saying frame `n` of a continuous burst, in either
    /// byte order, with or without the "SONY DSC " header.
    pub fn sony_note(header: bool, little: bool, n: u16) -> Vec<u8> {
        let mut note = if header {
            b"SONY DSC \0\0\0".to_vec()
        } else {
            Vec::new()
        };
        note.extend(note_ifd(
            little,
            &[(0xb049, 3, 1, 2), (0xb04a, 3, 1, n as u32)],
            &[],
        ));
        note
    }

    /// A Sony RAW (a TIFF) of `serial`'s body, frame `n` of its burst; 0 for
    /// a single shot.
    pub fn sony_raw(serial: &str, n: u16) -> Vec<u8> {
        tiff(true, "SONY", "ILCE-7M4", serial, &sony_note(false, true, n))
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{note_ifd, sony_note, tiff};
    use super::{Mark, Sequence, sequence};

    /// `tiff` in a JPEG's APP1 segment.
    fn jpeg(tiff: &[u8]) -> Vec<u8> {
        let mut j = vec![0xff, 0xd8, 0xff, 0xe1];
        j.extend(((tiff.len() + 8) as u16).to_be_bytes());
        j.extend(b"Exif\0\0");
        j.extend(tiff);
        j.extend([0xff, 0xd9]);
        j
    }

    fn read(file: &[u8], name: &str) -> Option<Sequence> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, file).unwrap();
        sequence(&path)
    }

    #[test]
    fn sony_frames_are_read_in_either_byte_order_with_or_without_a_header() {
        for (little, header) in [(true, false), (true, true), (false, true), (false, false)] {
            let file = tiff(
                little,
                "SONY",
                "ILCE-7M4",
                "",
                &sony_note(header, little, 3),
            );
            let read = read(&file, "a.arw").unwrap();
            assert_eq!(read.mark, Mark::Frame(3), "little {little} header {header}");
            assert_eq!(read.camera, "SONY\nILCE-7M4\n");
        }
        let single = tiff(true, "SONY", "ILCE-7M4", "", &sony_note(false, true, 0));
        assert_eq!(read(&single, "a.arw").unwrap().mark, Mark::Single);
        let unknown = tiff(
            true,
            "SONY",
            "ILCE-7M4",
            "",
            &sony_note(false, true, 0xffff),
        );
        assert_eq!(read(&unknown, "a.arw"), None);
        let mut other = b"SONY MOBILE\0".to_vec();
        other.extend(&sony_note(false, true, 3));
        assert_eq!(read(&tiff(true, "SONY", "X", "", &other), "a.jpg"), None);
    }

    #[test]
    fn fujifilm_frames_are_read_from_a_jpeg_and_a_raf_preview() {
        let mut note = b"FUJIFILM".to_vec();
        note.extend(12u32.to_le_bytes());
        note.extend(note_ifd(true, &[(0x1100, 3, 1, 1), (0x1101, 3, 1, 2)], &[]));
        // Fujifilm's EXIF is big-endian in a RAF's preview, its note not.
        let exif = jpeg(&tiff(false, "FUJIFILM", "X-T5", "", &note));
        assert_eq!(read(&exif, "a.jpg").unwrap().mark, Mark::Frame(2));
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(84, 0);
        raf.extend(108u32.to_be_bytes());
        raf.extend((exif.len() as u32).to_be_bytes());
        raf.resize(108, 0);
        raf.extend(&exif);
        assert_eq!(read(&raf, "a.raf").unwrap().mark, Mark::Frame(2));
    }

    #[test]
    fn panasonic_frames_and_single_drive() {
        let note = |n| {
            let mut note = b"Panasonic\0\0\0".to_vec();
            note.extend(note_ifd(true, &[(0x002a, 3, 1, 1), (0x002b, 4, 1, n)], &[]));
            note
        };
        let file = tiff(true, "Panasonic", "DC-S5", "", &note(4));
        assert_eq!(read(&file, "a.rw2").unwrap().mark, Mark::Frame(4));
        // An RW2: its own EXIF without the note, which is in its JpgFromRaw's.
        let preview = jpeg(&file);
        let mut rw2 = b"IIU\0\x08\0\0\0".to_vec();
        rw2.extend(1u16.to_le_bytes());
        rw2.extend(0x002eu16.to_le_bytes());
        rw2.extend(7u16.to_le_bytes());
        rw2.extend((preview.len() as u32).to_le_bytes());
        rw2.extend(26u32.to_le_bytes());
        rw2.extend([0; 4]);
        rw2.extend(&preview);
        assert_eq!(read(&rw2, "a.rw2").unwrap().mark, Mark::Frame(4));
        let file = tiff(true, "Panasonic", "DC-S5", "", &note(0));
        assert_eq!(read(&file, "a.rw2").unwrap().mark, Mark::Single);
    }

    #[test]
    fn an_iphone_burst_is_its_uuid_and_other_photos_are_single() {
        let note = |uuid: Option<&str>| {
            let mut note = b"Apple iOS\0\0\x01MM".to_vec();
            let entries_len = 2 + 2 * 12 + 4;
            let at = (14 + entries_len) as u32;
            let mut entries = vec![(0x0001, 9, 1, 14)];
            let mut data = Vec::new();
            if let Some(uuid) = uuid {
                data = [uuid.as_bytes(), b"\0"].concat();
                entries.push((0x000b, 2, data.len() as u32, at));
            }
            note.extend(note_ifd(false, &entries, &data));
            note
        };
        let uuid = "D1E2C3B4-0000-4A4A-8B8B-123456789ABC";
        let burst = jpeg(&tiff(
            false,
            "Apple",
            "iPhone 15 Pro",
            "",
            &note(Some(uuid)),
        ));
        assert_eq!(
            read(&burst, "a.jpg").unwrap().mark,
            Mark::Burst(uuid.into())
        );
        let single = jpeg(&tiff(false, "Apple", "iPhone 15 Pro", "", &note(None)));
        assert_eq!(read(&single, "a.jpg").unwrap().mark, Mark::Single);
    }

    #[test]
    fn the_body_serial_number_tells_two_bodies_apart() {
        let a = tiff(true, "SONY", "ILCE-1", "111", &sony_note(false, true, 1));
        let b = tiff(true, "SONY", "ILCE-1", "222", &sony_note(false, true, 1));
        assert_ne!(
            read(&a, "a.arw").unwrap().camera,
            read(&b, "b.arw").unwrap().camera
        );
    }

    #[test]
    fn other_makes_and_broken_notes_read_as_none() {
        let note = sony_note(false, true, 2);
        assert_eq!(
            read(&tiff(true, "Canon", "EOS R5", "", &note), "a.cr2"),
            None
        );
        for broken in [
            Vec::new(),
            b"SONY DSC \0\0\0".to_vec(),
            note[..note.len() / 2].to_vec(),
            b"FUJIFILM\xff\xff\xff\xff".to_vec(),
            b"Apple iOS\0\0\x01II".to_vec(),
        ] {
            for make in ["SONY", "FUJIFILM", "Apple"] {
                assert_eq!(read(&tiff(true, make, "X", "", &broken), "a.tif"), None);
            }
        }
        // A directory said to hold more entries than the note does.
        let mut huge = vec![0xe7, 0x03];
        huge.extend([0; 20]);
        assert_eq!(read(&tiff(true, "SONY", "X", "", &huge), "a.arw"), None);
    }

    /// A value's bytes in either byte order.
    fn u16b(little: bool, v: u16) -> [u8; 2] {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    }
    fn u32b(little: bool, v: u32) -> [u8; 4] {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    }

    /// An entry of a file's directory: tag, type, count, and its value's
    /// bytes, made from where they will be in the file (bytes of four or
    /// fewer are held in the entry).
    type Made = Box<dyn Fn(u32) -> Vec<u8>>;
    fn entry(
        tag: u16,
        kind: u16,
        count: u32,
        value: impl Fn(u32) -> Vec<u8> + 'static,
    ) -> (u16, u16, u32, Made) {
        (tag, kind, count, Box::new(value))
    }
    fn ascii(tag: u16, s: &str) -> (u16, u16, u32, Made) {
        let b = [s.as_bytes(), b"\0"].concat();
        entry(tag, 2, b.len() as u32, move |_| b.clone())
    }

    /// A TIFF of IFD0 `main` and, when given, an EXIF directory `exif`
    /// it points to, their values after them.
    fn file(
        little: bool,
        mut main: Vec<(u16, u16, u32, Made)>,
        exif: Vec<(u16, u16, u32, Made)>,
    ) -> Vec<u8> {
        let size = |n: usize| 2 + n as u32 * 12 + 4;
        let has_exif = !exif.is_empty();
        let main_len = main.len() + has_exif as usize;
        let exif_at = 8 + size(main_len);
        let mut at = exif_at + if has_exif { size(exif.len()) } else { 0 };
        if has_exif {
            main.push(entry(0x8769, 4, 1, move |_| u32b(little, exif_at).to_vec()));
            main.sort_by_key(|e| e.0);
        }
        let mut data = Vec::new();
        let mut ifd = |entries: &[(u16, u16, u32, Made)]| {
            let mut d = u16b(little, entries.len() as u16).to_vec();
            for (tag, kind, count, value) in entries {
                d.extend(u16b(little, *tag));
                d.extend(u16b(little, *kind));
                d.extend(u32b(little, *count));
                let bytes = value(at);
                if bytes.len() <= 4 {
                    let mut inline = bytes.clone();
                    inline.resize(4, 0);
                    d.extend(inline);
                } else {
                    d.extend(u32b(little, at));
                    at += bytes.len() as u32;
                    data.extend(bytes);
                }
            }
            d.extend([0; 4]);
            d
        };
        let main = ifd(&main);
        let exif = if has_exif { ifd(&exif) } else { Vec::new() };
        let mut t = if little {
            b"II*\0".to_vec()
        } else {
            b"MM\0*".to_vec()
        };
        t.extend(u32b(little, 8));
        t.extend(main);
        t.extend(exif);
        t.extend(data);
        t
    }

    /// A Canon maker note, little-endian, its offsets counting from `at`
    /// before its start: CameraSettings with ContinuousDrive `drive`, and
    /// FileInfo with BracketMode `bracket`; each array's first value is its
    /// length in bytes unless `bad_length`.
    fn canon_note(at: u32, drive: u16, bracket: Option<u16>, bad_length: bool) -> Vec<u8> {
        let shorts = |v: &[u16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let mut settings = vec![0u16; 49];
        settings[0] = if bad_length { 3 } else { 98 };
        settings[1] = 2;
        settings[5] = drive;
        let mut info = vec![0u16; 20];
        info[0] = 40;
        let mut entries = vec![(0x0000, 0, 0, 0)];
        let ifd_len = 2 + (2 + bracket.is_some() as u32) * 12 + 4;
        entries.push((0x0001, 3, 49, at + ifd_len));
        let mut data = shorts(&settings);
        if let Some(mode) = bracket {
            info[3] = mode;
            entries.push((0x0093, 3, 20, at + ifd_len + 98));
            data.extend(shorts(&info));
        }
        note_ifd(true, &entries, &data)
    }

    /// A CR2-like file: Canon's note in its EXIF.
    fn cr2(drive: u16, bracket: Option<u16>) -> Vec<u8> {
        // The note's length is its count: make it once to know it.
        let len = canon_note(0, drive, bracket, false).len() as u32;
        file(
            true,
            vec![
                ascii(0x010f, "Canon"),
                ascii(0x0110, "Canon EOS 5D Mark IV"),
            ],
            vec![
                entry(0x927c, 7, len, move |at| {
                    canon_note(at, drive, bracket, false)
                }),
                ascii(0xa431, "012345678901"),
            ],
        )
    }

    #[test]
    fn canon_drive_modes_and_brackets() {
        for (drive, bracket, mark) in [
            (0, None, Some(Mark::Single)),
            (9, Some(0), Some(Mark::Single)),
            (1, Some(0), Some(Mark::Continuous)),
            (5, None, Some(Mark::Continuous)),
            (10, None, Some(Mark::Continuous)),
            (0, Some(1), Some(Mark::Continuous)),
            (2, None, None),
            (11, None, None),
        ] {
            let read = read(&cr2(drive, bracket), "a.cr2");
            assert_eq!(
                read.as_ref().map(|s| &s.mark),
                mark.as_ref(),
                "{drive} {bracket:?}"
            );
        }
        assert_eq!(
            read(&cr2(5, None), "a.cr2").unwrap().camera,
            "Canon\nCanon EOS 5D Mark IV\n012345678901"
        );
        // Offsets that don't lead to a CameraSettings array say nothing.
        let len = canon_note(0, 5, None, true).len() as u32;
        let bad = file(
            true,
            vec![ascii(0x010f, "Canon"), ascii(0x0110, "X")],
            vec![entry(0x927c, 7, len, move |at| {
                canon_note(at, 5, None, true)
            })],
        );
        assert_eq!(read(&bad, "a.cr2"), None);
    }

    /// A DNG of Adobe's, its camera's note in a "MakN" block of its private
    /// data after another block, `before` bytes before the note copied with
    /// it.
    fn dng(make: &str, note: &[u8], origin: u32, before: &[u8]) -> Vec<u8> {
        let mut private = b"Adobe\0".to_vec();
        // A block of an odd length is padded.
        private.extend(b"Othr");
        private.extend(3u32.to_be_bytes());
        private.extend(b"abc\0");
        private.extend(b"MakN");
        private.extend(((6 + before.len() + note.len()) as u32).to_be_bytes());
        private.extend(b"II");
        private.extend((origin - before.len() as u32).to_be_bytes());
        private.extend(before);
        private.extend(note);
        file(
            true,
            vec![
                ascii(0x010f, make),
                ascii(0x0110, "Model"),
                entry(0xc634, 1, private.len() as u32, move |_| private.clone()),
            ],
            vec![ascii(0xa431, "42")],
        )
    }

    #[test]
    fn an_adobe_dngs_note_is_read_from_its_private_data() {
        // Canon's offsets count from where the note was in its CR2.
        let note = canon_note(0x3e4, 4, None, false);
        let read = read(&dng("Canon", &note, 0x3e4, b""), "a.dng").unwrap();
        assert_eq!(read.mark, Mark::Continuous);
        assert_eq!(read.camera, "Canon\nModel\n42");
        // Copied from 12 bytes before the note, which look like a directory.
        let before = [3, 0, 0, 0, 0x87, 0, 0, 0, 1, 0, 0, 0];
        let misplaced = dng("Canon", &canon_note(0x3e4, 0, None, false), 0x3e4, &before);
        assert_eq!(read_mark(&misplaced), Some(Mark::Single));
        let sony = dng("SONY", &sony_note(false, true, 4), 0x1000, b"");
        assert_eq!(read_mark(&sony), Some(Mark::Frame(4)));
        // A note said to run past the private data, or none at all.
        let mut short = dng("SONY", &sony_note(false, true, 4), 0x1000, b"");
        let at = short.windows(4).position(|w| w == b"MakN").unwrap();
        short[at + 4..at + 8].copy_from_slice(&9999u32.to_be_bytes());
        assert_eq!(read_mark(&short), None);
        let other = file(
            true,
            vec![
                ascii(0x010f, "SONY"),
                entry(0xc634, 1, 14, |_| b"Adobe\0Pano\0\0\0\0".to_vec()),
            ],
            vec![],
        );
        assert_eq!(read_mark(&other), None);
        // Only so many blocks are looked at.
        let mut many = b"Adobe\0".to_vec();
        for _ in 0..100 {
            many.extend(b"Zero\0\0\0\0");
        }
        let note = sony_note(false, true, 4);
        many.extend(b"MakN");
        many.extend(((6 + note.len()) as u32).to_be_bytes());
        many.extend(b"II\0\0\x10\0");
        many.extend(&note);
        let many = file(
            true,
            vec![
                ascii(0x010f, "SONY"),
                entry(0xc634, 1, many.len() as u32, move |_| many.clone()),
            ],
            vec![],
        );
        assert_eq!(read_mark(&many), None);
    }

    #[test]
    fn a_panasonic_dngs_note_is_read_from_the_jpeg_in_its_pano_block() {
        let mut note = b"Panasonic\0\0\0".to_vec();
        note.extend(note_ifd(true, &[(0x002b, 4, 1, 3)], &[]));
        let preview = jpeg(&tiff(true, "Panasonic", "DC-S1", "9", &note));
        // The RW2's first directory: big-endian entries, each value after
        // its entry in the block's byte order.
        let pano = |count: u16, preview: &[u8]| {
            let mut p = b"II".to_vec();
            p.extend(count.to_be_bytes());
            p.extend(0x0002u16.to_be_bytes());
            p.extend(3u16.to_be_bytes());
            p.extend(1u32.to_be_bytes());
            p.extend(6000u16.to_le_bytes());
            p.extend(0x002eu16.to_be_bytes());
            p.extend(7u16.to_be_bytes());
            p.extend((preview.len() as u32).to_be_bytes());
            p.extend(preview);
            p
        };
        let dng = |pano: Vec<u8>| {
            let mut private = b"Adobe\0Pano".to_vec();
            private.extend((pano.len() as u32).to_be_bytes());
            private.extend(&pano);
            file(
                true,
                vec![
                    ascii(0x010f, "Panasonic"),
                    entry(0xc634, 1, private.len() as u32, move |_| private.clone()),
                ],
                vec![],
            )
        };
        let read = read(&dng(pano(2, &preview)), "a.dng").unwrap();
        assert_eq!(read.mark, Mark::Frame(3));
        assert_eq!(read.camera, "Panasonic\nDC-S1\n9");
        // A preview said to run past the block, or entries that aren't there.
        let mut cut = pano(2, &preview);
        cut.truncate(cut.len() - 10);
        assert_eq!(read_mark(&dng(cut)), None);
        // A preview said to end inside its EXIF segment: what follows the
        // block is not read as its EXIF.
        let mut short = pano(2, &preview);
        let at = short.len() - preview.len() - 4;
        short[at..at + 4].copy_from_slice(&12u32.to_be_bytes());
        assert_eq!(read_mark(&dng(short)), None);
        let mut many = pano(2, &preview);
        many[2..4].copy_from_slice(&3u16.to_be_bytes());
        many.truncate(18);
        assert_eq!(read_mark(&dng(many)), None);
    }

    fn read_mark(file: &[u8]) -> Option<Mark> {
        read(file, "a.dng").map(|s| s.mark)
    }

    #[test]
    fn a_cr3s_note_is_read_from_its_cmt3_box() {
        let bx = |kind: &[u8; 4], body: &[u8]| {
            [&((body.len() + 8) as u32).to_be_bytes()[..], kind, body].concat()
        };
        let cmt1 = file(
            true,
            vec![ascii(0x010f, "Canon"), ascii(0x0110, "Canon EOS R5")],
            vec![],
        );
        let cmt2 = file(true, vec![ascii(0xa431, "777")], vec![]);
        // The note is the first directory of CMT3's TIFF, its offsets from
        // that TIFF's start.
        let mut cmt3 = b"II*\0\x08\0\0\0".to_vec();
        cmt3.extend(canon_note(8, 8, None, false));
        let canon = [
            &super::CANON_UUID[..],
            &bx(b"CMT1", &cmt1),
            &bx(b"CMT2", &cmt2),
            &bx(b"CMT3", &cmt3),
        ]
        .concat();
        let cr3 = [
            bx(b"ftyp", b"crx \0\0\0\x01crx isom"),
            bx(b"moov", &bx(b"uuid", &canon)),
        ]
        .concat();
        let read = read(&cr3, "a.cr3").unwrap();
        assert_eq!(read.mark, Mark::Continuous);
        assert_eq!(read.camera, "Canon\nCanon EOS R5\n777");
    }

    /// A Nikon note: "Nikon\0\x02\x10\0\0" and a TIFF of ShootingMode `mode`
    /// of type `kind`.
    fn nikon_note(little: bool, kind: u16, mode: u16) -> Vec<u8> {
        let mut note = b"Nikon\0\x02\x10\0\0".to_vec();
        note.extend(if little { b"II*\0" } else { b"MM\0*" });
        note.extend(u32b(little, 8));
        let mut d = note_ifd(
            little,
            &[
                (0x0001, 7, 4, u32::from_be_bytes(*b"0210")),
                (0x0089, kind, 1, mode as u32),
            ],
            &[],
        );
        if kind == 1 {
            // A BYTE is the value field's first byte.
            let at = 2 + 12 + 8;
            d[at..at + 4].copy_from_slice(&[mode as u8, 0, 0, 0]);
        }
        note.extend(d);
        note
    }

    #[test]
    fn nikon_shooting_modes() {
        let nef = |note: &[u8]| tiff(false, "NIKON CORPORATION", "NIKON Z 8", "", note);
        for (mode, mark) in [
            (0, Some(Mark::Single)),
            (0x20, Some(Mark::Single)),
            (0x04, Some(Mark::Single)),
            (0x01, Some(Mark::Continuous)),
            (0x21, Some(Mark::Continuous)),
            (0x10, Some(Mark::Continuous)),
            (0x800, Some(Mark::Continuous)),
            (0x08, None),
            (0x02, None),
        ] {
            for little in [true, false] {
                let got = read(&nef(&nikon_note(little, 3, mode)), "a.nef").map(|s| s.mark);
                assert_eq!(got, mark, "{mode:#x} little {little}");
            }
        }
        // A D2's mode is a BYTE.
        assert_eq!(
            read_mark(&nef(&nikon_note(false, 1, 1))),
            Some(Mark::Continuous)
        );
        // A D1's note has no header; its offsets count from the TIFF.
        let d1 = note_ifd(false, &[(0x0089, 3, 1, 0)], &[]);
        assert_eq!(read_mark(&nef(&d1)), Some(Mark::Single));
        // Coolpix notes ("Nikon\0\x01") aren't read.
        let mut coolpix = b"Nikon\0\x01\0".to_vec();
        coolpix.extend(note_ifd(false, &[(0x0089, 3, 1, 0)], &[]));
        assert_eq!(read_mark(&nef(&coolpix)), None);
    }

    /// An Olympus note with header `header` (its directory right after it),
    /// whose CameraSettings is a directory it points to holding DriveMode
    /// `drive`; its offsets count from the note's start.
    fn olympus_note(header: &[u8], little: bool, drive: &[u16]) -> Vec<u8> {
        let at = header.len() as u32;
        let main_len = 2 + 12 + 4;
        let sub_at = at + main_len;
        let sub_len = 2 + 12 + 4;
        let values: Vec<u8> = drive.iter().flat_map(|v| u16b(little, *v)).collect();
        let mut note = header.to_vec();
        note.extend(note_ifd(little, &[(0x2020, 13, 1, sub_at)], &[]));
        let value = if values.len() <= 4 {
            let mut v = values.clone();
            v.resize(4, 0);
            u32::from_be_bytes(v.try_into().unwrap())
        } else {
            sub_at + sub_len
        };
        let mut sub = note_ifd(little, &[(0x0600, 3, drive.len() as u32, value)], &[]);
        if values.len() <= 4 {
            // Held in the entry as it is.
            let mut v = values.clone();
            v.resize(4, 0);
            sub[10..14].copy_from_slice(&v);
        } else {
            sub.extend(&values);
        }
        note.extend(sub);
        note
    }

    #[test]
    fn olympus_and_om_drive_modes() {
        let orf = |make, note: &[u8]| tiff(true, make, "E-M1MarkIII", "", note);
        let olympus = |drive: &[u16]| olympus_note(b"OLYMPUS\0II\x03\0", true, drive);
        assert_eq!(
            read_mark(&orf(
                "OLYMPUS CORPORATION",
                &olympus(&[0, 0, 0, 0, 4, 0x21])
            )),
            Some(Mark::Single)
        );
        assert_eq!(
            read_mark(&orf("OLYMPUS CORPORATION", &olympus(&[1, 3, 0]))),
            Some(Mark::Frame(3))
        );
        let om = olympus_note(b"OM SYSTEM\0\0\0MM\x04\0", false, &[0, 2, 0, 0, 4, 0x23]);
        assert_eq!(
            read_mark(&orf("OM Digital Solutions", &om)),
            Some(Mark::Frame(2))
        );
        let other = olympus_note(b"OLYMPUX\0II\x03\0", true, &[1, 1]);
        assert_eq!(read_mark(&orf("OLYMPUS CORPORATION", &other)), None);
    }

    #[test]
    fn an_older_olympus_note_may_point_outside_itself() {
        // "OLYMP\0", its CameraSettings held in its value, DriveMode's
        // values after the note, offsets counting from the TIFF's start.
        let drive: Vec<u8> = [1u16, 2, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let note = |at: u32| {
            let mut note = b"OLYMP\0\x02\0".to_vec();
            let settings_at = at + 8 + 2 + 12 + 4;
            note.extend(note_ifd(true, &[(0x2020, 7, 18, settings_at)], &[]));
            // The settings directory, then the note ends.
            let after = settings_at + 18;
            note.extend(note_ifd(true, &[(0x0600, 3, 3, after)], &[]));
            note
        };
        let len = note(0).len() as u32;
        let drive2 = drive;
        let orf = file(
            true,
            vec![
                ascii(0x010f, "OLYMPUS IMAGING CORP."),
                ascii(0x0110, "E-330"),
            ],
            vec![
                entry(0x927c, 7, len, note),
                entry(0xa000, 7, 6, move |_| drive2.clone()),
            ],
        );
        assert_eq!(read_mark(&orf), Some(Mark::Frame(2)));
    }

    #[test]
    fn olympus_drive_values_in_exiftools_order() {
        use super::olympus_drive as d;
        assert_eq!(d(&[0, 0]), Some(Mark::Single));
        assert_eq!(d(&[1, 1, 0]), Some(Mark::Frame(1)));
        assert_eq!(d(&[1, 0, 0]), Some(Mark::Continuous));
        assert_eq!(d(&[2, 3, 0, 0, 0]), Some(Mark::Frame(3)));
        // Bracketing by bit flags comes before the shooting mode.
        assert_eq!(d(&[5, 2, 1, 0, 0, 0x01]), Some(Mark::Frame(2)));
        // A shooting mode of 0 leaves it to the mode.
        assert_eq!(d(&[1, 4, 0, 0, 4, 0]), Some(Mark::Frame(4)));
        assert_eq!(d(&[0, 0, 0, 0, 4, 0x21]), Some(Mark::Single));
        assert_eq!(d(&[0, 0, 0, 0, 4, 0x30]), Some(Mark::Single));
        assert_eq!(d(&[0, 0, 0, 0, 4, 0x42]), Some(Mark::Continuous));
        // Self-timers and modes ExifTool doesn't name don't say.
        assert_eq!(d(&[0, 0, 0, 0, 4, 0x26]), None);
        assert_eq!(d(&[0, 0, 0, 0, 2, 5]), None);
        assert_eq!(d(&[7, 0, 0]), None);
        assert_eq!(d(&[]), None);
    }

    #[test]
    fn exports_still_leave_the_maker_note_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.arw");
        std::fs::write(
            &path,
            tiff(true, "SONY", "ILCE-7M4", "", &sony_note(false, true, 2)),
        )
        .unwrap();
        let exif = super::super::read(&path).unwrap();
        assert!(exif.get(super::MAKER_NOTE).is_none());
        assert!(sequence(&path).is_some());
    }
}
