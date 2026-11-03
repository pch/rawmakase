//! Minimal bounded TIFF directory reading for RAW containers (ARW, DNG, the TIFF
//! inside RAF) and the EXIF embedded in JPEGs, PNGs, WebPs, AVIFs, HEIFs and
//! JPEG XL files. Reads only the directories and tags it is asked for.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

/// TIFF field types.
pub mod kind {
    pub const BYTE: u16 = 1;
    pub const ASCII: u16 = 2;
    pub const SHORT: u16 = 3;
    pub const LONG: u16 = 4;
    pub const RATIONAL: u16 = 5;
    pub const SBYTE: u16 = 6;
    pub const UNDEFINED: u16 = 7;
    pub const SSHORT: u16 = 8;
    pub const SLONG: u16 = 9;
    pub const SRATIONAL: u16 = 10;
    pub const FLOAT: u16 = 11;
    pub const DOUBLE: u16 = 12;
    /// A directory offset, as a LONG.
    pub const IFD: u16 = 13;
}
use kind::*;

pub struct Entry {
    pub kind: u16,
    pub count: u32,
    pub value: [u8; 4],
}
impl Entry {
    /// Bytes per value of its type; `None` for a type this does not know.
    pub fn type_size(&self) -> Option<usize> {
        match self.kind {
            BYTE | ASCII | SBYTE | UNDEFINED => Some(1),
            SHORT | SSHORT => Some(2),
            LONG | SLONG | FLOAT | IFD => Some(4),
            RATIONAL | SRATIONAL | DOUBLE => Some(8),
            _ => None,
        }
    }
}
/// Minimal bounded TIFF directory reader. Offsets are relative to `base`. It
/// reads a file, or any other seekable source such as a RAW held in memory.
pub struct Tiff<R = File> {
    f: R,
    base: u64,
    /// How many bytes from `base` the TIFF may span; `None` to the file's end.
    len: Option<u64>,
    pub little: bool,
    pub first: u64,
}
impl<R: Read + Seek> Tiff<R> {
    /// The TIFF at `base`, reaching to the end of the file, as a RAW's or a
    /// TIFF's own does.
    pub fn open(f: R, base: u64) -> Option<Self> {
        Self::start(f, base, None)
    }
    /// The TIFF of `len` bytes at `base`, embedded in another file's container
    /// (a JPEG's APP1 segment, a PNG's eXIf chunk, a WebP's EXIF chunk, an
    /// AVIF's or HEIF's Exif item, a JPEG XL's Exif box): nothing outside it is
    /// read.
    pub fn open_within(f: R, base: u64, len: u64) -> Option<Self> {
        Self::start(f, base, Some(len))
    }
    /// Directories at offsets from `base` without a TIFF header there, in
    /// the given byte order: a maker note's.
    pub fn headless(f: R, base: u64, little: bool) -> Self {
        Self {
            f,
            base,
            len: None,
            little,
            first: 0,
        }
    }
    /// Headless directories in the `len` bytes at `offset` from this TIFF's
    /// base (to its end when `None`), read through its source: a maker note
    /// too large to read whole. Nothing outside them is read; `None` when
    /// they run past this TIFF.
    pub fn part(&mut self, offset: u64, len: Option<u64>, little: bool) -> Option<Tiff<&mut R>> {
        let len = match (len, self.len) {
            (Some(len), max) => {
                let end = offset.checked_add(len)?;
                if max.is_some_and(|max| end > max) {
                    return None;
                }
                Some(len)
            }
            (None, Some(max)) => Some(max.checked_sub(offset)?),
            (None, None) => None,
        };
        Some(Tiff {
            f: &mut self.f,
            base: self.base.checked_add(offset)?,
            len,
            little,
            first: 0,
        })
    }
    fn start(f: R, base: u64, len: Option<u64>) -> Option<Self> {
        let mut t = Self {
            f,
            base,
            len,
            little: false,
            first: 0,
        };
        let h = t.bytes(0, 8)?;
        let little = match &h[..4] {
            // Olympus ORF and Panasonic RW2 use their own magic numbers.
            b"II*\0" | b"IIRO" | b"IIRS" | b"IIU\0" => true,
            b"MM\0*" | b"MMOR" => false,
            _ => return None,
        };
        t.little = little;
        t.first = t.u32(&h[4..8]) as u64;
        Some(t)
    }
    pub fn u16(&self, b: &[u8]) -> u16 {
        let a = [b[0], b[1]];
        if self.little {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        }
    }
    pub fn u32(&self, b: &[u8]) -> u32 {
        let a = [b[0], b[1], b[2], b[3]];
        if self.little {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    }
    /// `len` bytes (at most 1 MiB) at `offset` from `base`.
    pub fn bytes(&mut self, offset: u64, len: usize) -> Option<Vec<u8>> {
        let end = offset.checked_add(len as u64)?;
        if len > 1 << 20 || self.len.is_some_and(|max| end > max) {
            return None;
        }
        let mut b = vec![0; len];
        self.f
            .seek(SeekFrom::Start(self.base.checked_add(offset)?))
            .ok()?;
        self.f.read_exact(&mut b).ok()?;
        Some(b)
    }
    pub fn ifd(&mut self, offset: u64) -> Option<BTreeMap<u16, Entry>> {
        let count = self.bytes(offset, 2)?;
        let n = self.u16(&count) as usize;
        if n == 0 || n > 1000 {
            return None;
        }
        let b = self.bytes(offset + 2, n * 12)?;
        Some(
            b.as_chunks::<12>()
                .0
                .iter()
                .map(|e| {
                    (
                        self.u16(&e[0..2]),
                        Entry {
                            kind: self.u16(&e[2..4]),
                            count: self.u32(&e[4..8]),
                            value: [e[8], e[9], e[10], e[11]],
                        },
                    )
                })
                .collect(),
        )
    }
    /// The first value of a LONG or IFD entry, used as a directory offset.
    pub fn offset(&self, e: &Entry) -> Option<u64> {
        matches!(e.kind, LONG | IFD).then(|| self.u32(&e.value) as u64)
    }
    /// The values of an integer or rational entry.
    pub fn numbers(&mut self, e: &Entry) -> Option<Vec<f32>> {
        if !matches!(e.kind, SHORT | SSHORT | LONG | SLONG | RATIONAL | SRATIONAL) {
            return None;
        }
        let size = e.type_size()?;
        let b = self.raw(e)?;
        let v: Vec<f32> = b
            .chunks_exact(size)
            .map(|c| match e.kind {
                SHORT => self.u16(c) as f32,
                SSHORT => self.u16(c) as i16 as f32,
                LONG => self.u32(c) as f32,
                SLONG => self.u32(c) as i32 as f32,
                RATIONAL => self.u32(&c[..4]) as f32 / self.u32(&c[4..]) as f32,
                _ => self.u32(&c[..4]) as i32 as f32 / self.u32(&c[4..]) as i32 as f32,
            })
            .collect();
        v.iter().all(|x| x.is_finite()).then_some(v)
    }
    /// An entry's raw value bytes in file byte order.
    pub fn raw(&mut self, e: &Entry) -> Option<Vec<u8>> {
        let len = (e.count as usize).checked_mul(e.type_size()?)?;
        if len <= 4 {
            Some(e.value[..len].to_vec())
        } else {
            let at = self.u32(&e.value) as u64;
            self.bytes(at, len)
        }
    }
    /// All values of a LONG or IFD entry, e.g. the SubIFDs array.
    pub fn offsets(&mut self, e: &Entry) -> Option<Vec<u64>> {
        if !matches!(e.kind, LONG | IFD) {
            return None;
        }
        let b = self.raw(e)?;
        Some(
            b.as_chunks::<4>()
                .0
                .iter()
                .map(|c| self.u32(c) as u64)
                .collect(),
        )
    }
}
