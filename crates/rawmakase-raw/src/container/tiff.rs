//! TIFF-based containers (DNG, CR2, NEF, ARW, PEF, ORF, RW2, SRW, 3FR…): every
//! image directory reachable from the header (the IFD chain and SubIFDs), the
//! EXIF directory and where the maker notes are.
use crate::io::{Bytes, Order};
use rawmakase_tiff::{Entry, Tiff, kind};
use std::collections::BTreeMap;
use std::io::Cursor;

/// An image file directory.
pub(crate) struct Ifd {
    pub entries: BTreeMap<u16, Entry>,
}

impl Ifd {
    pub fn has(&self, tag: u16) -> bool {
        self.entries.contains_key(&tag)
    }
}

/// A TIFF file in memory, its directories read.
pub(crate) struct TiffFile<'a> {
    /// The file from the TIFF header on, in its byte order: offsets in the
    /// directories are into it.
    pub bytes: Bytes<'a>,
    /// Where the TIFF header is in the file.
    pub base: u64,
    /// IFD0 and its chain, each followed by its SubIFDs (depth first).
    pub ifds: Vec<Ifd>,
    pub exif: Option<Ifd>,
}

/// The most directories followed: files with more are malformed or hostile.
const MAX_IFDS: usize = 64;

impl<'a> TiffFile<'a> {
    /// The TIFF whose header is at `base` in `file`.
    pub fn parse(file: &'a [u8], base: u64) -> Option<Self> {
        let mut t = Tiff::open(Cursor::new(file), base)?;
        let mut ifds = Vec::new();
        let mut seen = Vec::new();
        let mut next = t.first;
        while next != 0 && ifds.len() < MAX_IFDS && !seen.contains(&next) {
            seen.push(next);
            let Some(entries) = t.ifd(next) else { break };
            let count = t.bytes(next, 2).map(|b| u64::from(t.u16(&b)))?;
            let link = t
                .bytes(next + 2 + count * 12, 4)
                .map(|b| u64::from(t.u32(&b)))
                .unwrap_or(0);
            let ifd = Ifd { entries };
            let subs = ifd.entries.get(&330).and_then(|e| t.offsets(e));
            ifds.push(ifd);
            for sub in subs.into_iter().flatten() {
                walk(&mut t, sub, &mut ifds, &mut seen, 1);
            }
            next = link;
        }
        if ifds.is_empty() {
            return None;
        }
        let exif = ifds
            .first()
            .and_then(|i| i.entries.get(&34665))
            .and_then(|e| t.offset(e))
            .and_then(|at| t.ifd(at).map(|entries| Ifd { entries }));
        let order = if t.little { Order::Little } else { Order::Big };
        Some(Self {
            bytes: Bytes::new(file.get(usize::try_from(base).ok()?..)?, order),
            base,
            ifds,
            exif,
        })
    }
    pub fn little(&self) -> bool {
        self.bytes.order == Order::Little
    }
    /// The directory at `offset` (from the TIFF header) alone, read with this
    /// TIFF's byte order and offsets: a maker note's or a preview's.
    pub fn directory_at(&self, offset: u64) -> Option<TiffFile<'a>> {
        Some(Self {
            bytes: self.bytes,
            base: self.base,
            ifds: vec![self.ifd_at(offset)?],
            exif: None,
        })
    }
    /// An entry's value bytes.
    pub fn value(&self, e: &Entry) -> Option<&'a [u8]> {
        let len = u64::from(e.count).checked_mul(e.type_size()? as u64)?;
        if len <= 4 {
            // In the entry itself; not borrowed from the file.
            return None;
        }
        self.bytes.slice(u64::from(self.u32(&e.value)), len).ok()
    }
    /// Where an entry's value is, from the TIFF header, when it is not in the
    /// entry itself.
    pub fn offset_of(&self, e: &Entry) -> Option<u64> {
        let len = u64::from(e.count).checked_mul(e.type_size()? as u64)?;
        (len > 4).then(|| u64::from(self.u32(&e.value)))
    }
    pub fn u32(&self, b: &[u8]) -> u32 {
        match self.bytes.order {
            Order::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Order::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        }
    }
    pub fn u16(&self, b: &[u8]) -> u16 {
        match self.bytes.order {
            Order::Little => u16::from_le_bytes([b[0], b[1]]),
            Order::Big => u16::from_be_bytes([b[0], b[1]]),
        }
    }
    /// The directory at `offset` (from the TIFF header), outside the chain:
    /// a maker note's or a private one.
    pub fn ifd_at(&self, offset: u64) -> Option<Ifd> {
        let mut t = Tiff::open(Cursor::new(self.bytes.data), 0)?;
        t.ifd(offset).map(|entries| Ifd { entries })
    }
    /// A tag's bytes, wherever they are.
    pub fn raw(&self, ifd: &Ifd, tag: u16) -> Option<Vec<u8>> {
        let e = ifd.entries.get(&tag)?;
        let len = usize::try_from(e.count).ok()?.checked_mul(e.type_size()?)?;
        if len <= 4 {
            Some(e.value[..len].to_vec())
        } else {
            self.value(e).map(<[u8]>::to_vec)
        }
    }
    /// The values of an unsigned integer tag (BYTE, SHORT, LONG, IFD).
    pub fn ints(&self, ifd: &Ifd, tag: u16) -> Option<Vec<u32>> {
        let e = ifd.entries.get(&tag)?;
        if !matches!(
            e.kind,
            kind::BYTE
                | kind::UNDEFINED
                | kind::SHORT
                | kind::LONG
                | kind::IFD
                | kind::SSHORT
                | kind::SLONG
        ) {
            return None;
        }
        let size = e.type_size()?;
        let raw = self.raw(ifd, tag)?;
        Some(
            raw.chunks_exact(size)
                .map(|c| match size {
                    1 => u32::from(c[0]),
                    2 => u32::from(self.u16(c)),
                    _ => self.u32(c),
                })
                .collect(),
        )
    }
    pub fn int(&self, ifd: &Ifd, tag: u16) -> Option<u32> {
        self.ints(ifd, tag)?.first().copied()
    }
    /// The values of any numeric tag in double precision: rationals as their
    /// quotient, floats widened.
    pub fn reals(&self, ifd: &Ifd, tag: u16) -> Option<Vec<f64>> {
        let e = ifd.entries.get(&tag)?;
        let size = e.type_size()?;
        if matches!(e.kind, kind::ASCII | kind::UNDEFINED) {
            return None;
        }
        let raw = self.raw(ifd, tag)?;
        Some(
            raw.chunks_exact(size)
                .map(|c| match e.kind {
                    kind::BYTE => f64::from(c[0]),
                    kind::SBYTE => f64::from(c[0] as i8),
                    kind::SHORT => f64::from(self.u16(c)),
                    kind::SSHORT => f64::from(self.u16(c) as i16),
                    kind::LONG | kind::IFD => f64::from(self.u32(c)),
                    kind::SLONG => f64::from(self.u32(c) as i32),
                    kind::RATIONAL => f64::from(self.u32(&c[..4])) / f64::from(self.u32(&c[4..])),
                    kind::SRATIONAL => {
                        f64::from(self.u32(&c[..4]) as i32) / f64::from(self.u32(&c[4..]) as i32)
                    }
                    kind::FLOAT => f64::from(f32::from_bits(self.u32(c))),
                    _ => {
                        let (a, b) = (u64::from(self.u32(&c[..4])), u64::from(self.u32(&c[4..])));
                        f64::from_bits(if self.little() {
                            b << 32 | a
                        } else {
                            a << 32 | b
                        })
                    }
                })
                .collect(),
        )
    }
    pub fn real(&self, ifd: &Ifd, tag: u16) -> Option<f64> {
        self.reals(ifd, tag)?.first().copied()
    }
    /// An ASCII tag up to its first NUL.
    pub fn text(&self, ifd: &Ifd, tag: u16) -> Option<String> {
        let e = ifd.entries.get(&tag)?;
        if !matches!(e.kind, kind::ASCII | kind::BYTE | kind::UNDEFINED) {
            return None;
        }
        let raw = self.raw(ifd, tag)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Some(String::from_utf8_lossy(&raw[..end]).into_owned())
    }
}

fn walk(
    t: &mut Tiff<Cursor<&[u8]>>,
    at: u64,
    ifds: &mut Vec<Ifd>,
    seen: &mut Vec<u64>,
    depth: u32,
) {
    if depth > 3 || ifds.len() >= MAX_IFDS || seen.contains(&at) {
        return;
    }
    seen.push(at);
    let Some(entries) = t.ifd(at) else { return };
    let ifd = Ifd { entries };
    let subs = ifd.entries.get(&330).and_then(|e| t.offsets(e));
    ifds.push(ifd);
    for sub in subs.into_iter().flatten() {
        walk(t, sub, ifds, seen, depth + 1);
    }
}

impl TiffFile<'_> {
    /// The JPEG previews the directories list: JPEGInterchangeFormat (513,
    /// 514) and JPEG-compressed strips of images that are not raw data.
    pub fn previews(&self) -> Vec<crate::preview::Candidate> {
        let mut out = Vec::new();
        for ifd in self.ifds.iter().chain(&self.exif) {
            let pixels = u64::from(self.int(ifd, 256).unwrap_or(0))
                * u64::from(self.int(ifd, 257).unwrap_or(0));
            if let (Some(at), Some(len)) = (self.int(ifd, 513), self.int(ifd, 514)) {
                out.push(crate::preview::Candidate {
                    offset: self.base + u64::from(at),
                    len: u64::from(len),
                    pixels,
                });
            }
            let photometric = self.int(ifd, 262).unwrap_or(0);
            if matches!(self.int(ifd, 259), Some(6 | 7))
                && matches!(photometric, 2 | 6)
                && let (Some(offsets), Some(counts)) = (self.ints(ifd, 273), self.ints(ifd, 279))
                && offsets.len() == 1
                && counts.len() == 1
            {
                out.push(crate::preview::Candidate {
                    offset: self.base + u64::from(offsets[0]),
                    len: u64::from(counts[0]),
                    pixels,
                });
            }
        }
        out
    }
    /// Whether a directory other than the raw image holds an uncompressed
    /// RGB or YCbCr image (a thumbnail).
    pub fn has_uncompressed_thumbnail(&self) -> bool {
        self.ifds.iter().any(|ifd| {
            self.int(ifd, 259).unwrap_or(1) == 1 && matches!(self.int(ifd, 262), Some(2 | 6))
        })
    }
}
