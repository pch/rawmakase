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

/// The longest Make and Model read: Exif names are short.
pub(crate) const NAME_MAX: usize = 64;

/// The longest free text read: lens names, UniqueCameraModel, serial
/// numbers.
pub(crate) const TEXT_MAX: usize = 256;

/// Values collected into a vector allocated fallibly: none when memory
/// runs out.
fn collect<T>(values: impl ExactSizeIterator<Item = T>) -> Option<Vec<T>> {
    let mut out = Vec::new();
    out.try_reserve_exact(values.len()).ok()?;
    out.extend(values);
    Some(out)
}

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
        let mut t = Tiff::headless(Cursor::new(self.bytes.data), 0, self.little());
        t.ifd(offset).map(|entries| Ifd { entries })
    }
    /// A maker note whose offsets count from `base` in `file`, in `order`:
    /// it has no directories of its own (`ifds` is empty); read them with
    /// [`TiffFile::ifd_at`].
    pub fn view(file: &'a [u8], base: u64, order: Order) -> Option<Self> {
        Some(Self {
            bytes: Bytes::new(file.get(usize::try_from(base).ok()?..)?, order),
            base,
            ifds: Vec::new(),
            exif: None,
        })
    }
    /// A tag's first `max` bytes at most, where they are (in the entry or
    /// in the file): borrowed, not copied. `usize::MAX` takes the whole
    /// value, which must then be in the file.
    fn slice_upto<'s>(&'s self, e: &'s Entry, max: usize) -> Option<&'s [u8]> {
        let len = usize::try_from(e.count).ok()?.checked_mul(e.type_size()?)?;
        let take = len.min(max);
        if len <= 4 {
            e.value.get(..take)
        } else {
            self.bytes
                .slice(u64::from(self.u32(&e.value)), take as u64)
                .ok()
        }
    }
    /// A tag's bytes where they are, borrowed: a record read in place
    /// costs nothing however long the file says it is.
    pub fn bytes_of<'s>(&'s self, ifd: &'s Ifd, tag: u16) -> Option<&'s [u8]> {
        self.slice_upto(ifd.entries.get(&tag)?, usize::MAX)
    }
    /// A tag's type and count of values, when the directory has it: whether
    /// it is there, apart from whether its value reads.
    pub fn shape(&self, ifd: &Ifd, tag: u16) -> Option<(u16, u32)> {
        ifd.entries.get(&tag).map(|e| (e.kind, e.count))
    }
    /// A copy of a tag's first `max` bytes at most: a value is as long as
    /// the file says, so a reader that needs a record's leading fields
    /// copies only those.
    pub fn raw_upto(&self, ifd: &Ifd, tag: u16, max: usize) -> Option<Vec<u8>> {
        let b = self.slice_upto(ifd.entries.get(&tag)?, max)?;
        let mut out = Vec::new();
        out.try_reserve_exact(b.len()).ok()?;
        out.extend_from_slice(b);
        Some(out)
    }
    /// The values of an unsigned integer tag (BYTE, SHORT, LONG, IFD) of
    /// at most `max` values; a longer one is none (malformed or hostile).
    pub fn ints(&self, ifd: &Ifd, tag: u16, max: usize) -> Option<Vec<u32>> {
        let e = ifd.entries.get(&tag)?;
        (usize::try_from(e.count).ok()? <= max)
            .then(|| self.ints_upto(ifd, tag, max))
            .flatten()
    }
    /// The first `n` values at most of an unsigned integer tag.
    pub fn ints_upto(&self, ifd: &Ifd, tag: u16, n: usize) -> Option<Vec<u32>> {
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
        let raw = self.slice_upto(e, n.saturating_mul(size))?;
        collect(raw.chunks_exact(size).map(|c| match size {
            1 => u32::from(c[0]),
            2 => u32::from(self.u16(c)),
            _ => self.u32(c),
        }))
    }
    pub fn int(&self, ifd: &Ifd, tag: u16) -> Option<u32> {
        self.ints_upto(ifd, tag, 1)?.first().copied()
    }
    /// The values of a numeric tag of at most `max` values (a longer one is
    /// none) in double precision: rationals as their quotient, floats
    /// widened.
    pub fn reals(&self, ifd: &Ifd, tag: u16, max: usize) -> Option<Vec<f64>> {
        let e = ifd.entries.get(&tag)?;
        (usize::try_from(e.count).ok()? <= max)
            .then(|| self.reals_upto(ifd, tag, max))
            .flatten()
    }
    /// The first `n` values at most of a numeric tag, as [`TiffFile::reals`].
    pub fn reals_upto(&self, ifd: &Ifd, tag: u16, n: usize) -> Option<Vec<f64>> {
        let e = ifd.entries.get(&tag)?;
        let size = e.type_size()?;
        if matches!(e.kind, kind::ASCII | kind::UNDEFINED) {
            return None;
        }
        let raw = self.slice_upto(e, n.saturating_mul(size))?;
        collect(raw.chunks_exact(size).map(|c| match e.kind {
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
        }))
    }
    /// A numeric tag's first value.
    pub fn real(&self, ifd: &Ifd, tag: u16) -> Option<f64> {
        self.reals_upto(ifd, tag, 1)?.first().copied()
    }
    /// A text tag's first `max` bytes at most, up to its first NUL.
    pub fn text_upto(&self, ifd: &Ifd, tag: u16, max: usize) -> Option<String> {
        let e = ifd.entries.get(&tag)?;
        if !matches!(e.kind, kind::ASCII | kind::BYTE | kind::UNDEFINED) {
            return None;
        }
        let raw = self.slice_upto(e, max)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Some(String::from_utf8_lossy(&raw[..end]).into_owned())
    }
    /// The values of StripOffsets or StripByteCounts (273, 279),
    /// TileOffsets or TileByteCounts (324, 325): at most as many as the
    /// directory's image can have (TIFF 6: a strip holds a row or more,
    /// tiles cover the image, once per sample plane when planar; its sides
    /// at most `io::MAX_SIDE`); more is none. The values are in the file,
    /// so no more are read than it holds.
    pub fn pieces(&self, ifd: &Ifd, tag: u16) -> Option<Vec<u32>> {
        let side = |tag| {
            self.int(ifd, tag)
                .map(|v| v.min(crate::io::MAX_SIDE) as usize)
        };
        let height = side(257)?;
        let per_plane = match tag {
            324 | 325 => {
                let (width, tw, th) = (side(256)?, side(322)?, side(323)?);
                if tw == 0 || th == 0 {
                    return None;
                }
                width.div_ceil(tw) * height.div_ceil(th)
            }
            _ => height,
        };
        let planes = match self.int(ifd, 284) {
            Some(2) => self.int(ifd, 277).unwrap_or(1).clamp(1, 16) as usize,
            _ => 1,
        };
        self.ints(ifd, tag, per_plane.saturating_mul(planes))
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
                && let (Some(offsets), Some(counts)) =
                    (self.ints(ifd, 273, 1), self.ints(ifd, 279, 1))
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
    /// RGB or YCbCr image whose first samples are 8-bit (a thumbnail; others,
    /// as a phone's 12-bit one or one whose bits are garbage, are none).
    pub fn has_uncompressed_thumbnail(&self) -> bool {
        self.ifds.iter().any(|ifd| {
            self.int(ifd, 259).unwrap_or(1) == 1
                && matches!(self.int(ifd, 262), Some(2 | 6))
                && self.ints_upto(ifd, 258, 1).and_then(|b| b.first().copied()) == Some(8)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian TIFF of one directory of SHORT `entries` (tag,
    /// values), values over two stored after it.
    fn tiff(entries: &[(u16, &[u16])]) -> Vec<u8> {
        let mut values_at = 8 + 2 + 12 * entries.len() as u32 + 4;
        let (mut f, mut values) = (b"II*\0\x08\0\0\0".to_vec(), Vec::new());
        f.extend((entries.len() as u16).to_le_bytes());
        for (tag, v) in entries {
            f.extend(tag.to_le_bytes());
            f.extend(3u16.to_le_bytes());
            f.extend((v.len() as u32).to_le_bytes());
            let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            if bytes.len() <= 4 {
                f.extend(bytes.iter().copied().chain([0; 4]).take(4));
            } else {
                f.extend(values_at.to_le_bytes());
                values_at += bytes.len() as u32;
                values.extend(bytes);
            }
        }
        f.extend(0u32.to_le_bytes());
        f.extend(values);
        f
    }

    #[test]
    fn a_record_longer_than_its_reader_reads_is_none_or_cut() {
        let f = tiff(&[(1, &[1, 2, 3]), (2, &[7])]);
        let t = TiffFile::parse(&f, 0).unwrap();
        let ifd = &t.ifds[0];
        assert_eq!(t.ints(ifd, 1, 3), Some(vec![1, 2, 3]));
        assert_eq!(t.ints(ifd, 1, 2), None);
        assert_eq!(t.ints_upto(ifd, 1, 2), Some(vec![1, 2]));
        assert_eq!(t.reals(ifd, 1, 2), None);
        assert_eq!(t.reals_upto(ifd, 1, 1), Some(vec![1.]));
        assert_eq!(t.real(ifd, 1), Some(1.));
        assert_eq!(t.bytes_of(ifd, 1), Some(&[1, 0, 2, 0, 3, 0][..]));
        assert_eq!(t.bytes_of(ifd, 2), Some(&[7, 0][..]));
        assert_eq!(t.raw_upto(ifd, 1, 3), Some(vec![1, 0, 2]));
    }

    /// Strips hold a row or more; tiles cover the image once per plane.
    #[test]
    fn strips_and_tiles_are_at_most_what_the_image_can_have() {
        let rows: Vec<u16> = (0..5).collect();
        let f = tiff(&[(256, &[10]), (257, &[4]), (273, &rows[..4]), (279, &rows)]);
        let t = TiffFile::parse(&f, 0).unwrap();
        assert_eq!(t.pieces(&t.ifds[0], 273), Some(vec![0, 1, 2, 3]));
        assert_eq!(t.pieces(&t.ifds[0], 279), None);
        // 10 × 4 in 4 × 2 tiles: three across, two down, in two planes.
        let six: Vec<u16> = (0..6).collect();
        let twelve: Vec<u16> = (0..12).collect();
        let tiles = |planar: u16, n: &[u16]| {
            tiff(&[
                (256, &[10]),
                (257, &[4]),
                (277, &[2]),
                (284, &[planar]),
                (322, &[4]),
                (323, &[2]),
                (324, n),
            ])
        };
        for (planar, n, fits) in [(1, &six, true), (1, &twelve, false), (2, &twelve, true)] {
            let f = tiles(planar, n);
            let t = TiffFile::parse(&f, 0).unwrap();
            assert_eq!(t.pieces(&t.ifds[0], 324).is_some(), fits, "{planar} {n:?}");
        }
        // A large image in small tiles keeps every one: 20000 × 20000 in
        // 16 × 16 tiles is 1,562,500 of them.
        let n = 1250 * 1250;
        let zeros = vec![0u16; n];
        let f = tiff(&[
            (254, &[0]),
            (256, &[20000]),
            (257, &[20000]),
            (258, &[16]),
            (259, &[1]),
            (262, &[32803]),
            (277, &[1]),
            (322, &[16]),
            (323, &[16]),
            (324, &zeros),
            (325, &zeros),
            (33421, &[2, 2]),
            (33422, &[0x0100, 0x0201]),
            (50706, &[0x0401, 0]),
        ]);
        let t = TiffFile::parse(&f, 0).unwrap();
        assert_eq!(t.pieces(&t.ifds[0], 324).map(|v| v.len()), Some(n));
        let info = crate::open(&f).unwrap();
        let crate::variant::Plan::Dng(plan) = &info.plan else {
            panic!("not a DNG plan");
        };
        assert_eq!(plan.tiles.len(), n);
        // Without the image's height, none.
        let f = tiff(&[(256, &[10]), (273, &[0])]);
        let t = TiffFile::parse(&f, 0).unwrap();
        assert_eq!(t.pieces(&t.ifds[0], 273), None);
    }
}
