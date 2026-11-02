//! Bounded reading of ISO base media files: the boxes of a HEIF (an AVIF or a
//! HEIC) or a JPEG XL container, and a HEIF's items with their locations,
//! properties and references. Only headers and the bytes asked for are read.
use std::io::{Read, Seek, SeekFrom};

const MAX_BODY: u64 = 16 << 20;
const MAX_ITEM: u64 = 256 << 20;
const MAX_BOXES: usize = 4096;

/// A JPEG XL container's first box, its signature.
pub const JXL_CONTAINER: [u8; 12] = [
    0, 0, 0, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a,
];

/// A box: its kind and its body's offset and length.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxHeader {
    pub kind: [u8; 4],
    pub start: u64,
    pub size: u64,
}

pub fn file_len<R: Seek>(r: &mut R) -> Option<u64> {
    r.seek(SeekFrom::End(0)).ok()
}

/// The boxes between `start` and `end`, in order. A box that claims to run
/// past `end` ends the listing.
pub fn boxes<R: Read + Seek>(r: &mut R, start: u64, end: u64) -> Vec<BoxHeader> {
    let mut found = Vec::new();
    let mut at = start;
    while at + 8 <= end && found.len() < MAX_BOXES {
        let Some((header, total)) = box_at(r, at, end) else {
            break;
        };
        found.push(header);
        at += total;
    }
    found
}

fn box_at<R: Read + Seek>(r: &mut R, at: u64, end: u64) -> Option<(BoxHeader, u64)> {
    r.seek(SeekFrom::Start(at)).ok()?;
    let mut h = [0u8; 8];
    r.read_exact(&mut h).ok()?;
    let kind: [u8; 4] = h[4..8].try_into().ok()?;
    let (header_len, total) = match u32::from_be_bytes(h[0..4].try_into().ok()?) {
        0 => (8, end - at),
        1 => {
            let mut large = [0u8; 8];
            r.read_exact(&mut large).ok()?;
            (16, u64::from_be_bytes(large))
        }
        n => (8, n as u64),
    };
    if total < header_len || at.checked_add(total)? > end {
        return None;
    }
    Some((
        BoxHeader {
            kind,
            start: at + header_len,
            size: total - header_len,
        },
        total,
    ))
}

/// A box's body, when not larger than 16 MiB.
pub fn read_body<R: Read + Seek>(r: &mut R, b: &BoxHeader) -> Option<Vec<u8>> {
    if b.size > MAX_BODY {
        return None;
    }
    r.seek(SeekFrom::Start(b.start)).ok()?;
    let mut body = vec![0u8; b.size as usize];
    r.read_exact(&mut body).ok()?;
    Some(body)
}

/// The body of a JPEG XL container's first `kind` box ("Exif", "xml "), or of
/// a "brob" box holding that kind brotli-compressed, as cjxl writes them by
/// default; `None` when there is neither, or the body, compressed or not, is
/// over 16 MiB.
pub fn jxl_metadata<R: Read + Seek>(r: &mut R, kind: [u8; 4]) -> Option<Vec<u8>> {
    let len = file_len(r)?;
    for b in boxes(r, 0, len) {
        if b.kind == kind {
            return read_body(r, &b);
        }
        if &b.kind != b"brob" || b.size < 4 || b.size > MAX_BODY {
            continue;
        }
        r.seek(SeekFrom::Start(b.start)).ok()?;
        let mut inner = [0u8; 4];
        r.read_exact(&mut inner).ok()?;
        if inner == kind {
            let compressed = (&mut *r).take(b.size - 4);
            let mut body = Vec::new();
            brotli_decompressor::Decompressor::new(compressed, 4096)
                .take(MAX_BODY + 1)
                .read_to_end(&mut body)
                .ok()?;
            return (body.len() as u64 <= MAX_BODY).then_some(body);
        }
    }
    None
}

/// An item of a HEIF file, e.g. an AV1 image ("av01"), an HEVC one ("hvc1"),
/// a grid of them ("grid"), its EXIF ("Exif") or XMP ("mime", content type
/// "application/rdf+xml").
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: u32,
    pub kind: [u8; 4],
    pub content_type: String,
}

/// An item property this reader knows; others are kept by kind.
#[derive(Clone, Debug, PartialEq)]
pub enum Property {
    /// `av1C`: the AV1 configuration record.
    Av1Config(Vec<u8>),
    /// `hvcC`: the HEVC configuration record, with its parameter sets.
    HevcConfig(Vec<u8>),
    /// `colr`.
    Colour(Colour),
    /// `ispe`: width and height.
    Size(u32, u32),
    /// `pixi`: bits per channel.
    Depth(Vec<u8>),
    /// `irot`: anticlockwise quarter turns, 0–3.
    Rotation(u8),
    /// `imir`: 0 swaps the top and bottom, 1 the left and right (ISO/IEC
    /// 23008-12:2022).
    Mirror(u8),
    /// `auxC`: an auxiliary image's type, e.g. alpha.
    Auxiliary(String),
    /// `clap`: a crop.
    CleanAperture(CleanAperture),
    Other([u8; 4]),
}

/// A `clap` box: the size of the image kept, and how far its centre is from
/// the full image's, each a fraction (numerator, denominator).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CleanAperture {
    pub width: (u32, u32),
    pub height: (u32, u32),
    pub horizontal_offset: (i32, u32),
    pub vertical_offset: (i32, u32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Colour {
    Icc(Vec<u8>),
    /// CICP code points (ITU-T H.273).
    Nclx {
        primaries: u16,
        transfer: u16,
        matrix: u16,
        full_range: bool,
    },
}

#[derive(Clone, Debug, Default)]
struct Location {
    id: u32,
    method: u8,
    base: u64,
    extents: Vec<(u64, u64)>,
}

/// An item's properties, as 1-based indexes into `Heif::properties`, and
/// whether each is essential.
type Association = (u32, Vec<(u16, bool)>);

/// What a HEIF (AVIF) file's `ftyp` and `meta` say.
#[derive(Clone, Debug, Default)]
pub struct Heif {
    /// Major brand first, then the compatible ones.
    pub brands: Vec<[u8; 4]>,
    pub primary: u32,
    pub items: Vec<Item>,
    locations: Vec<Location>,
    properties: Vec<Property>,
    associations: Vec<Association>,
    /// (kind, from, to).
    references: Vec<([u8; 4], u32, u32)>,
    /// The `idat` box's body: the data of items stored in the `meta` box.
    idat: Vec<u8>,
}

/// A big-endian cursor over a box body; every read is bounds-checked.
struct Bytes<'a>(&'a [u8]);
impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        (self.0.len() >= n).then(|| {
            let (head, rest) = self.0.split_at(n);
            self.0 = rest;
            head
        })
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
    }
    /// A fraction: a numerator then a denominator other than 0.
    fn fraction(&mut self) -> Option<(u32, u32)> {
        let (n, d) = (self.u32()?, self.u32()?);
        (d != 0).then_some((n, d))
    }
    fn sized(&mut self, size: u8) -> Option<u64> {
        match size {
            0 => Some(0),
            4 => self.u32().map(u64::from),
            8 => self
                .take(8)
                .map(|b| u64::from_be_bytes(b.try_into().unwrap())),
            _ => None,
        }
    }
    fn kind(&mut self) -> Option<[u8; 4]> {
        self.take(4).map(|b| b.try_into().unwrap())
    }
    /// A NUL-terminated string.
    fn string(&mut self) -> Option<String> {
        let end = self.0.iter().position(|b| *b == 0)?;
        let s = String::from_utf8_lossy(&self.0[..end]).into_owned();
        self.0 = &self.0[end + 1..];
        Some(s)
    }
    /// The boxes in the rest of the body.
    fn children(&self) -> Vec<([u8; 4], &'a [u8])> {
        let mut out = Vec::new();
        let mut rest = self.0;
        while rest.len() >= 8 && out.len() < MAX_BOXES {
            let size = u32::from_be_bytes(rest[0..4].try_into().unwrap()) as usize;
            if size < 8 || size > rest.len() {
                break;
            }
            out.push((rest[4..8].try_into().unwrap(), &rest[8..size]));
            rest = &rest[size..];
        }
        out
    }
}

impl Heif {
    /// The `ftyp` and `meta` of a HEIF file; `None` when it isn't one.
    pub fn read<R: Read + Seek>(r: &mut R) -> Option<Heif> {
        let end = file_len(r)?;
        let top = boxes(r, 0, end);
        let ftyp = top.first().filter(|b| &b.kind == b"ftyp")?;
        let ftyp = read_body(r, ftyp)?;
        let mut heif = Heif::default();
        let mut f = Bytes(&ftyp);
        heif.brands.push(f.kind()?);
        f.take(4)?;
        while let Some(brand) = f.kind() {
            heif.brands.push(brand);
        }
        let meta = top.iter().find(|b| &b.kind == b"meta")?;
        let meta = read_body(r, meta)?;
        let mut m = Bytes(&meta);
        m.take(4)?; // version and flags
        for (kind, body) in m.children() {
            let mut b = Bytes(body);
            match &kind {
                b"pitm" => {
                    let version = b.u8()?;
                    b.take(3)?;
                    heif.primary = if version == 0 {
                        b.u16()? as u32
                    } else {
                        b.u32()?
                    };
                }
                b"iinf" => heif.items = items(b)?,
                b"iloc" => heif.locations = locations(b)?,
                b"iprp" => heif.read_properties(b)?,
                b"iref" => heif.references = references(b)?,
                b"idat" => heif.idat = body.to_vec(),
                _ => {}
            }
        }
        Some(heif)
    }

    pub fn item(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// The properties of item `id`, in the order its association lists them
    /// (the order transformations apply in).
    pub fn properties(&self, id: u32) -> Vec<&Property> {
        self.associations
            .iter()
            .filter(|(item, _)| *item == id)
            .flat_map(|(_, indexes)| indexes.iter())
            .filter_map(|(i, _)| self.properties.get((*i as usize).checked_sub(1)?))
            .collect()
    }

    /// The properties of item `id` it marks essential, which a reader must
    /// understand to show it.
    pub fn essential_properties(&self, id: u32) -> Vec<&Property> {
        self.associations
            .iter()
            .filter(|(item, _)| *item == id)
            .flat_map(|(_, indexes)| indexes.iter())
            .filter(|(_, essential)| *essential)
            .filter_map(|(i, _)| self.properties.get((*i as usize).checked_sub(1)?))
            .collect()
    }

    /// An essential property of item `id` that is not read here, e.g. `lsel`
    /// (a layer selection).
    pub fn unread_essential(&self, id: u32) -> Option<[u8; 4]> {
        self.essential_properties(id)
            .into_iter()
            .find_map(|p| match p {
                Property::Other(kind) => Some(*kind),
                _ => None,
            })
    }

    /// Where item `id`'s data is in the file, when in one piece there.
    pub fn extent(&self, id: u32) -> Option<(u64, u64)> {
        let l = self
            .locations
            .iter()
            .find(|l| l.id == id && l.method == 0)?;
        match l.extents.as_slice() {
            [(offset, len)] => Some((l.base.checked_add(*offset)?, *len)),
            _ => None,
        }
    }

    /// Item `id`'s data from the file, or from the `idat` box (construction
    /// method 1), its extents joined; up to 256 MiB, and `None` when an extent
    /// runs past the file or the box. Length 0 ("to the end of the file") is
    /// not read.
    pub fn data<R: Read + Seek>(&self, r: &mut R, id: u32) -> Option<Vec<u8>> {
        self.data_up_to(r, id, MAX_ITEM)
    }

    /// Item `id`'s data as [`Heif::data`] reads it, but `None`, before any
    /// of it is read, when its extents add up to more than `max` bytes; for
    /// metadata items, which are never as large as an image.
    pub fn data_up_to<R: Read + Seek>(&self, r: &mut R, id: u32, max: u64) -> Option<Vec<u8>> {
        let l = self
            .locations
            .iter()
            .find(|l| l.id == id && l.method <= 1)?;
        if l.method == 1 {
            let mut data = Vec::new();
            for (offset, len) in &l.extents {
                let start = usize::try_from(l.base.checked_add(*offset)?).ok()?;
                let end = start.checked_add(usize::try_from(*len).ok()?)?;
                data.extend_from_slice(self.idat.get(start..end)?);
                if data.len() as u64 > max.min(MAX_ITEM) {
                    return None;
                }
            }
            return Some(data);
        }
        let file_len = file_len(r)?;
        let mut total = 0u64;
        for (offset, len) in &l.extents {
            let end = l.base.checked_add(*offset)?.checked_add(*len)?;
            total = total.checked_add(*len)?;
            if end > file_len || total > max.min(MAX_ITEM) {
                return None;
            }
        }
        let mut data = Vec::with_capacity(usize::try_from(total).ok()?);
        for (offset, len) in &l.extents {
            r.seek(SeekFrom::Start(l.base + offset)).ok()?;
            let at = data.len();
            data.resize(at + usize::try_from(*len).ok()?, 0);
            r.read_exact(&mut data[at..]).ok()?;
        }
        Some(data)
    }

    /// Whether item `id`'s data is all there, in the file or the `idat` box,
    /// without reading it: what a decoder that reads the file itself would
    /// otherwise find cut off.
    pub fn stored_within<R: Seek>(&self, r: &mut R, id: u32) -> bool {
        let Some(l) = self.locations.iter().find(|l| l.id == id && l.method <= 1) else {
            return false;
        };
        let Some(end) = (match l.method {
            0 => file_len(r),
            _ => Some(self.idat.len() as u64),
        }) else {
            return false;
        };
        !l.extents.is_empty()
            && l.extents.iter().all(|(offset, len)| {
                l.base
                    .checked_add(*offset)
                    .and_then(|at| at.checked_add(*len))
                    .is_some_and(|e| *len > 0 && e <= end)
            })
    }

    /// The items with a `kind` reference to item `of`: for "cdsc", the
    /// metadata that describes it.
    pub fn describing(&self, kind: [u8; 4], of: u32) -> Vec<u32> {
        self.references
            .iter()
            .filter(|(k, _, to)| *k == kind && *to == of)
            .map(|(_, from, _)| *from)
            .collect()
    }

    /// The items item `from` has a `kind` reference to, in the order it lists
    /// them: for "dimg" from a grid, its tiles row by row.
    pub fn referenced(&self, kind: [u8; 4], from: u32) -> Vec<u32> {
        self.references
            .iter()
            .filter(|(k, f, _)| *k == kind && *f == from)
            .map(|(_, _, to)| *to)
            .collect()
    }

    fn read_properties(&mut self, b: Bytes) -> Option<()> {
        for (kind, body) in b.children() {
            match &kind {
                b"ipco" => {
                    self.properties = Bytes(body)
                        .children()
                        .into_iter()
                        .map(|(kind, body)| property(kind, body).unwrap_or(Property::Other(kind)))
                        .collect()
                }
                // One per version and flags, so there may be several; an
                // item has one association at most, so there are no more
                // associations than items can be listed. Each lookup scans
                // them all.
                b"ipma" if self.associations.len() < MAX_BOXES => {
                    self.associations.extend(associations(Bytes(body))?);
                    self.associations.truncate(MAX_BOXES);
                }
                _ => {}
            }
        }
        Some(())
    }
}

fn items(mut b: Bytes) -> Option<Vec<Item>> {
    let version = b.u8()?;
    b.take(3)?;
    let count = if version == 0 {
        b.u16()? as u32
    } else {
        b.u32()?
    };
    let mut out = Vec::new();
    for (kind, body) in b.children().into_iter().take(count as usize) {
        if &kind != b"infe" {
            continue;
        }
        let mut e = Bytes(body);
        let version = e.u8()?;
        e.take(3)?;
        if version < 2 {
            continue;
        }
        let id = if version == 2 {
            e.u16()? as u32
        } else {
            e.u32()?
        };
        e.u16()?; // protection index
        let kind = e.kind()?;
        e.string()?; // name
        let content_type = if &kind == b"mime" {
            e.string().unwrap_or_default()
        } else {
            String::new()
        };
        out.push(Item {
            id,
            kind,
            content_type,
        });
    }
    Some(out)
}

fn locations(mut b: Bytes) -> Option<Vec<Location>> {
    let version = b.u8()?;
    b.take(3)?;
    let sizes = b.u16()?;
    let (offset_size, length_size, base_size) = (
        (sizes >> 12) as u8,
        (sizes >> 8 & 15) as u8,
        (sizes >> 4 & 15) as u8,
    );
    let index_size = if version >= 1 { (sizes & 15) as u8 } else { 0 };
    let count = if version < 2 {
        b.u16()? as u32
    } else {
        b.u32()?
    };
    let mut out = Vec::new();
    for _ in 0..count.min(MAX_BOXES as u32) {
        let id = if version < 2 {
            b.u16()? as u32
        } else {
            b.u32()?
        };
        let method = if version >= 1 {
            (b.u16()? & 15) as u8
        } else {
            0
        };
        b.u16()?; // data reference index
        let base = b.sized(base_size)?;
        let extents = b.u16()?;
        let mut l = Location {
            id,
            method,
            base,
            extents: Vec::new(),
        };
        // Every extent must consume bytes, so the count cannot outgrow the body.
        if extents > 1 && index_size == 0 && offset_size == 0 && length_size == 0 {
            return None;
        }
        for _ in 0..extents {
            b.sized(index_size)?;
            let offset = b.sized(offset_size)?;
            let length = b.sized(length_size)?;
            l.extents.push((offset, length));
        }
        out.push(l);
    }
    Some(out)
}

fn associations(mut b: Bytes) -> Option<Vec<Association>> {
    let version = b.u8()?;
    let flags = b.take(3)?[2];
    let count = b.u32()?;
    let mut out = Vec::new();
    for _ in 0..count.min(MAX_BOXES as u32) {
        let id = if version < 1 {
            b.u16()? as u32
        } else {
            b.u32()?
        };
        let n = b.u8()?;
        let mut indexes = Vec::new();
        for _ in 0..n {
            // The top bit says whether the property is essential.
            indexes.push(if flags & 1 == 1 {
                let i = b.u16()?;
                (i & 0x7fff, i & 0x8000 != 0)
            } else {
                let i = b.u8()?;
                ((i & 0x7f) as u16, i & 0x80 != 0)
            });
        }
        out.push((id, indexes));
    }
    Some(out)
}

fn references(mut b: Bytes) -> Option<Vec<([u8; 4], u32, u32)>> {
    let version = b.u8()?;
    b.take(3)?;
    let mut out = Vec::new();
    for (kind, body) in b.children() {
        let mut r = Bytes(body);
        let id = |r: &mut Bytes| {
            if version == 0 {
                r.u16().map(u32::from)
            } else {
                r.u32()
            }
        };
        let from = id(&mut r)?;
        let n = r.u16()?;
        for _ in 0..n {
            out.push((kind, from, id(&mut r)?));
        }
    }
    Some(out)
}

fn property(kind: [u8; 4], body: &[u8]) -> Option<Property> {
    let mut b = Bytes(body);
    Some(match &kind {
        b"av1C" => Property::Av1Config(body.to_vec()),
        b"hvcC" => Property::HevcConfig(body.to_vec()),
        b"colr" => match &b.kind()? {
            b"nclx" => {
                let (primaries, transfer, matrix) = (b.u16()?, b.u16()?, b.u16()?);
                Property::Colour(Colour::Nclx {
                    primaries,
                    transfer,
                    matrix,
                    full_range: b.u8()? & 0x80 != 0,
                })
            }
            b"prof" | b"rICC" => Property::Colour(Colour::Icc(b.0.to_vec())),
            _ => Property::Other(kind),
        },
        b"ispe" => {
            b.take(4)?;
            Property::Size(b.u32()?, b.u32()?)
        }
        b"pixi" => {
            b.take(4)?;
            let n = b.u8()? as usize;
            Property::Depth(b.take(n)?.to_vec())
        }
        b"irot" => Property::Rotation(b.u8()? & 3),
        b"imir" => Property::Mirror(b.u8()? & 1),
        b"auxC" => {
            b.take(4)?;
            Property::Auxiliary(b.string()?)
        }
        b"clap" => {
            let (width, height) = (b.fraction()?, b.fraction()?);
            let (horizontal, vertical) = (b.fraction()?, b.fraction()?);
            Property::CleanAperture(CleanAperture {
                width,
                height,
                // The offsets are signed.
                horizontal_offset: (horizontal.0 as i32, horizontal.1),
                vertical_offset: (vertical.0 as i32, vertical.1),
            })
        }
        _ => Property::Other(kind),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn fixture(name: &str) -> std::fs::File {
        std::fs::File::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/data/formats")
                .join(name),
        )
        .unwrap()
    }
    /// A box of `kind` around `body`.
    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend(kind);
        b.extend(body);
        b
    }

    #[test]
    fn a_jpeg_xl_metadata_box_is_read_plain_or_brotli_compressed() {
        let mut plain = fixture("metadata.jxl");
        let mut compressed = fixture("metadata-brob.jxl");
        for kind in [*b"Exif", *b"xml "] {
            let expected = jxl_metadata(&mut plain, kind).unwrap();
            assert_eq!(jxl_metadata(&mut compressed, kind), Some(expected));
        }
        assert_eq!(jxl_metadata(&mut compressed, *b"jumb"), None);
    }

    #[test]
    fn a_compressed_jpeg_xl_box_unpacking_to_over_16_mib_is_not_read() {
        // Brotli streams of 16 MiB and of 16 MiB + 1 zero bytes.
        let at_limit = [
            0xcf, 0xff, 0xff, 0x7f, 0xf8, 0x27, 0x00, 0xe2, 0xb1, 0x40, 0x20, 0xf7, 0xfe, 0x7f,
        ];
        let over = [
            0xcf, 0xff, 0xff, 0x7f, 0xf8, 0x27, 0x00, 0xe2, 0xb1, 0x40, 0x20, 0xf7, 0xfe, 0x1f,
            0x00, 0x00, 0x01, 0x00, 0x03,
        ];
        let file = |stream: &[u8]| {
            let body = [b"xml ".as_slice(), stream].concat();
            [JXL_CONTAINER.to_vec(), bx(b"brob", &body)].concat()
        };
        let read = jxl_metadata(&mut Cursor::new(file(&at_limit)), *b"xml ").unwrap();
        assert_eq!(read.len(), 16 << 20);
        assert_eq!(jxl_metadata(&mut Cursor::new(file(&over)), *b"xml "), None);
    }

    #[test]
    fn a_corrupt_compressed_jpeg_xl_box_is_not_read() {
        let body = [b"Exif".as_slice(), &[0xff; 32]].concat();
        let file = [JXL_CONTAINER.to_vec(), bx(b"brob", &body)].concat();
        assert_eq!(jxl_metadata(&mut Cursor::new(file), *b"Exif"), None);
    }

    #[test]
    fn boxes_are_listed_with_their_bodies() {
        let file = [bx(b"ftyp", b"avif\0\0\0\0avifmif1"), bx(b"free", &[0; 3])].concat();
        let mut r = Cursor::new(&file);
        let found = boxes(&mut r, 0, file.len() as u64);
        let kinds: Vec<_> = found.iter().map(|b| (b.kind, b.start, b.size)).collect();
        assert_eq!(kinds, [(*b"ftyp", 8, 16), (*b"free", 32, 3)]);
    }

    #[test]
    fn a_box_claiming_more_than_the_file_ends_the_listing() {
        let mut file = bx(b"ftyp", b"avif\0\0\0\0");
        file.extend(1_000_000u32.to_be_bytes());
        file.extend(b"mdat");
        let found = boxes(&mut Cursor::new(&file), 0, file.len() as u64);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn an_avifs_primary_item_and_properties_are_read() {
        let heif = Heif::read(&mut fixture("rotated.avif")).unwrap();
        assert!(heif.brands.contains(b"avif"));
        let primary = heif.item(heif.primary).unwrap();
        assert_eq!(&primary.kind, b"av01");
        let props = heif.properties(heif.primary);
        assert!(props.iter().any(|p| matches!(p, Property::Av1Config(_))));
        assert!(props.iter().any(|p| matches!(p, Property::Size(16, 12))));
        assert!(props.iter().any(|p| matches!(p, Property::Rotation(1))));
        assert!(
            heif.data(&mut fixture("rotated.avif"), heif.primary)
                .is_some_and(|d| !d.is_empty())
        );
    }

    #[test]
    fn an_essential_property_it_does_not_read_is_named() {
        // Item 1 has an essential `lsel` (a layer selection, not read here)
        // and a non-essential `free`.
        let ipco = bx(b"ipco", &[bx(b"lsel", &[0, 0]), bx(b"free", &[])].concat());
        let ipma = bx(b"ipma", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 2, 0x81, 0x02]);
        let meta = [vec![0; 4], bx(b"iprp", &[ipco, ipma].concat())].concat();
        let file = [bx(b"ftyp", b"heic\0\0\0\0mif1heic"), bx(b"meta", &meta)].concat();
        let heif = Heif::read(&mut Cursor::new(&file)).unwrap();
        assert_eq!(heif.unread_essential(1), Some(*b"lsel"));
        // irot and clap are essential too, and read.
        for name in ["rotated.avif", "cropped.avif"] {
            let heif = Heif::read(&mut fixture(name)).unwrap();
            assert_eq!(heif.unread_essential(heif.primary), None, "{name}");
        }
    }

    #[test]
    fn a_clean_aperture_is_read_as_fractions() {
        // avifenc --crop 2,2,8,8 of a 16x12 image: 8x8, its centre 2 left of
        // the image's and level with it.
        let heif = Heif::read(&mut fixture("cropped.avif")).unwrap();
        let clap = heif
            .properties(heif.primary)
            .into_iter()
            .find_map(|p| match p {
                Property::CleanAperture(c) => Some(*c),
                _ => None,
            })
            .unwrap();
        let value = |(n, d): (i32, u32)| f64::from(n) / f64::from(d);
        assert_eq!(value((clap.width.0 as i32, clap.width.1)), 8.);
        assert_eq!(value((clap.height.0 as i32, clap.height.1)), 8.);
        assert_eq!(value(clap.horizontal_offset), -2.);
        assert_eq!(value(clap.vertical_offset), 0.);
        // A zero denominator is no fraction: the box is not read.
        let zero = [[0, 0, 0, 8], [0; 4], [0, 0, 0, 8], [0, 0, 0, 1]].concat();
        assert_eq!(property(*b"clap", &[zero.clone(), zero].concat()), None);
    }

    #[test]
    fn an_item_in_the_meta_box_is_read_from_idat() {
        // iloc version 1, 4-byte offsets and lengths: item 1, construction
        // method 1, two extents of the idat body ("bc" then "e").
        let mut iloc = vec![1, 0, 0, 0, 0x44, 0x00, 0, 1];
        iloc.extend([0, 1, 0, 1, 0, 0, 0, 2]);
        iloc.extend([0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 4, 0, 0, 0, 1]);
        let idat = bx(b"idat", b"abcdef");
        let meta = [vec![0; 4], bx(b"iloc", &iloc), idat].concat();
        let file = [bx(b"ftyp", b"heic\0\0\0\0mif1"), bx(b"meta", &meta)].concat();
        let mut r = Cursor::new(&file);
        let heif = Heif::read(&mut r).unwrap();
        assert_eq!(heif.data(&mut r, 1).as_deref(), Some(b"bce".as_slice()));
        assert_eq!(heif.data_up_to(&mut r, 1, 2), None);
        // An extent past the end of the box is not read.
        let mut past = iloc.clone();
        past[23] = 9;
        let meta = [vec![0; 4], bx(b"iloc", &past), bx(b"idat", b"abcdef")].concat();
        let file = [bx(b"ftyp", b"heic\0\0\0\0mif1"), bx(b"meta", &meta)].concat();
        let mut r = Cursor::new(&file);
        assert_eq!(Heif::read(&mut r).unwrap().data(&mut r, 1), None);
    }

    #[test]
    fn an_item_is_stored_within_the_file_only_when_all_its_extents_are() {
        let mut f = fixture("rotated.avif");
        let heif = Heif::read(&mut f).unwrap();
        assert!(heif.stored_within(&mut f, heif.primary));
        assert!(!heif.stored_within(&mut f, 999));
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/data/formats/rotated.avif"),
        )
        .unwrap();
        let mut cut = Cursor::new(bytes[..bytes.len() - 1].to_vec());
        assert!(!heif.stored_within(&mut cut, heif.primary));
        // In idat: within the box, and past it.
        let mut iloc = vec![1, 0, 0, 0, 0x44, 0x00, 0, 1];
        iloc.extend([0, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 4]);
        let meta = [vec![0; 4], bx(b"iloc", &iloc), bx(b"idat", b"abcdef")].concat();
        let file = [bx(b"ftyp", b"heic\0\0\0\0mif1"), bx(b"meta", &meta)].concat();
        let mut r = Cursor::new(&file);
        let heif = Heif::read(&mut r).unwrap();
        assert!(heif.stored_within(&mut r, 1));
        let mut past = iloc.clone();
        past[23] = 5;
        let meta = [vec![0; 4], bx(b"iloc", &past), bx(b"idat", b"abcdef")].concat();
        let file = [bx(b"ftyp", b"heic\0\0\0\0mif1"), bx(b"meta", &meta)].concat();
        let mut r = Cursor::new(&file);
        assert!(!Heif::read(&mut r).unwrap().stored_within(&mut r, 1));
    }

    #[test]
    fn a_grids_tiles_are_its_dimg_references_in_order() {
        // iref version 0: dimg from 1 to 4, 2, 3; thmb from 5 to 1.
        let dimg = bx(b"dimg", &[0, 1, 0, 3, 0, 4, 0, 2, 0, 3]);
        let thmb = bx(b"thmb", &[0, 5, 0, 1, 0, 1]);
        let iref = bx(b"iref", &[vec![0; 4], dimg, thmb].concat());
        let meta = [vec![0; 4], iref].concat();
        let file = [bx(b"ftyp", b"heic\0\0\0\0mif1"), bx(b"meta", &meta)].concat();
        let heif = Heif::read(&mut Cursor::new(&file)).unwrap();
        assert_eq!(heif.referenced(*b"dimg", 1), [4, 2, 3]);
        assert_eq!(heif.referenced(*b"dimg", 5), [] as [u32; 0]);
        assert_eq!(heif.describing(*b"thmb", 1), [5]);
    }

    #[test]
    fn an_avifs_nclx_colour_is_read() {
        let heif = Heif::read(&mut fixture("pq.avif")).unwrap();
        let colour = heif
            .properties(heif.primary)
            .into_iter()
            .find_map(|p| match p {
                Property::Colour(c) => Some(c.clone()),
                _ => None,
            });
        assert!(matches!(
            colour,
            Some(Colour::Nclx {
                primaries: 9,
                transfer: 16,
                matrix: 9,
                ..
            })
        ));
    }

    #[test]
    fn item_data_over_the_cap_is_refused_before_reading() {
        /// Counts the bytes read through it.
        struct Counting(Cursor<Vec<u8>>, usize);
        impl Read for Counting {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.read(buf)?;
                self.1 += n;
                Ok(n)
            }
        }
        impl Seek for Counting {
            fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
                self.0.seek(to)
            }
        }
        // Three overlapping 8-byte extents of a 16-byte file: 24 bytes.
        let heif = Heif {
            locations: vec![Location {
                id: 1,
                method: 0,
                base: 0,
                extents: vec![(0, 8), (4, 8), (8, 8)],
            }],
            ..Heif::default()
        };
        let mut r = Counting(Cursor::new((0..16).collect()), 0);
        assert_eq!(heif.data_up_to(&mut r, 1, 16), None);
        assert_eq!(r.1, 0);
        assert_eq!(heif.data_up_to(&mut r, 1, 24).map(|d| d.len()), Some(24));
        assert_eq!(heif.data(&mut r, 1).map(|d| d.len()), Some(24));
    }

    #[test]
    fn an_avifs_exif_and_xmp_items_describe_the_primary_one() {
        let mut f = fixture("metadata.avif");
        let heif = Heif::read(&mut f).unwrap();
        let described = heif.describing(*b"cdsc", heif.primary);
        let kinds: Vec<_> = described
            .iter()
            .map(|id| heif.item(*id).unwrap().kind)
            .collect();
        assert!(kinds.contains(b"Exif"));
        let xmp = described
            .iter()
            .find(|id| heif.item(**id).unwrap().content_type == "application/rdf+xml")
            .copied()
            .unwrap();
        let data = heif.data(&mut f, xmp).unwrap();
        assert!(String::from_utf8_lossy(&data).contains("Fixture title"));
    }

    #[test]
    fn a_jxl_containers_boxes_include_exif_and_xml() {
        let mut f = fixture("metadata.jxl");
        let len = file_len(&mut f).unwrap();
        let kinds: Vec<_> = boxes(&mut f, 0, len).iter().map(|b| b.kind).collect();
        assert!(
            kinds.contains(b"Exif") && kinds.contains(b"xml "),
            "{kinds:?}"
        );
    }

    /// A file whose `iloc` (version 0) holds `items`, each (id, extents), with
    /// the given offset and length field sizes.
    fn with_iloc(offset_size: u16, length_size: u16, items: &[(u16, Vec<(u64, u64)>)]) -> Vec<u8> {
        let field = |v: u64, size: u16| match size {
            0 => vec![],
            4 => (v as u32).to_be_bytes().to_vec(),
            _ => v.to_be_bytes().to_vec(),
        };
        let mut iloc = vec![0, 0, 0, 0];
        iloc.extend((offset_size << 12 | length_size << 8).to_be_bytes());
        iloc.extend((items.len() as u16).to_be_bytes());
        for (id, extents) in items {
            iloc.extend(id.to_be_bytes());
            iloc.extend([0, 0]);
            iloc.extend((extents.len() as u16).to_be_bytes());
            for (offset, len) in extents {
                iloc.extend(field(*offset, offset_size));
                iloc.extend(field(*len, length_size));
            }
        }
        let meta = [vec![0; 4], bx(b"iloc", &iloc)].concat();
        [bx(b"ftyp", b"avif\0\0\0\0avif"), bx(b"meta", &meta)].concat()
    }

    #[test]
    fn associations_from_every_ipma_box_are_kept() {
        let ispe = bx(b"ispe", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2]);
        let ipco = bx(b"ipco", &[ispe, bx(b"irot", &[1])].concat());
        // Version 0 for item 1, version 1 (32-bit item IDs) for item 2.
        let first = bx(b"ipma", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 1]);
        let second = bx(b"ipma", &[1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2, 1, 2]);
        let meta = [vec![0; 4], bx(b"iprp", &[ipco, first, second].concat())].concat();
        let file = [bx(b"ftyp", b"avif\0\0\0\0avif"), bx(b"meta", &meta)].concat();
        let heif = Heif::read(&mut Cursor::new(&file)).unwrap();
        assert_eq!(heif.properties(1), [&Property::Size(1, 2)]);
        assert_eq!(heif.properties(2), [&Property::Rotation(1)]);
    }

    /// An item has one association at most, so no more are kept than items
    /// can be listed: each lookup scans them all, and a grid looks up every
    /// tile.
    #[test]
    fn associations_beyond_as_many_as_items_are_not_kept() {
        let ipco = bx(b"ipco", &bx(b"irot", &[1]));
        let ipma = |ids: std::ops::Range<u32>| {
            let mut body = vec![1, 0, 0, 0];
            body.extend((ids.len() as u32).to_be_bytes());
            for id in ids {
                body.extend(id.to_be_bytes());
                body.extend([1, 0x81]);
            }
            bx(b"ipma", &body)
        };
        let full = MAX_BOXES as u32;
        let iprp = bx(
            b"iprp",
            &[ipco, ipma(1..full + 1), ipma(full + 1..full + 3)].concat(),
        );
        let meta = [vec![0; 4], iprp].concat();
        let file = [bx(b"ftyp", b"heic\0\0\0\0mif1"), bx(b"meta", &meta)].concat();
        let heif = Heif::read(&mut Cursor::new(&file)).unwrap();
        assert_eq!(heif.associations.len(), MAX_BOXES);
        assert_eq!(heif.essential_properties(full), [&Property::Rotation(1)]);
        assert!(heif.properties(full + 1).is_empty());
    }

    #[test]
    fn extents_that_take_no_bytes_cannot_multiply() {
        // Repeated over 4096 items this would allocate gigabytes from 24 KB.
        let file = with_iloc(0, 0, &[(1, vec![(0, 0); 65535])]);
        assert!(Heif::read(&mut Cursor::new(&file)).is_none());
    }

    #[test]
    fn extent_lengths_that_overflow_are_refused() {
        let file = with_iloc(4, 8, &[(1, vec![(0, u64::MAX), (0, 2)])]);
        let heif = Heif::read(&mut Cursor::new(&file)).unwrap();
        assert!(heif.data(&mut Cursor::new(&file), 1).is_none());
    }

    #[test]
    fn an_extent_past_the_end_of_the_file_is_refused_before_allocating() {
        let file = with_iloc(4, 8, &[(1, vec![(0, 200 << 20)])]);
        let heif = Heif::read(&mut Cursor::new(&file)).unwrap();
        assert!(heif.data(&mut Cursor::new(&file), 1).is_none());
    }
}
