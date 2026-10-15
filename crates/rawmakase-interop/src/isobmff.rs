//! Bounded reading of ISO base media files: the boxes of an AVIF (HEIF) or a
//! JPEG XL container, and an AVIF's items with their locations, properties and
//! references. Only headers and the bytes asked for are read.
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

/// An item of a HEIF file, e.g. the AV1 image ("av01"), a grid ("grid"),
/// its EXIF ("Exif") or XMP ("mime", content type "application/rdf+xml").
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
    /// `colr`.
    Colour(Colour),
    /// `ispe`: width and height.
    Size(u32, u32),
    /// `pixi`: bits per channel.
    Depth(Vec<u8>),
    /// `irot`: anticlockwise quarter turns, 0–3.
    Rotation(u8),
    /// `imir`: 0 mirrors about the vertical axis (left and right swap), 1
    /// about the horizontal one.
    Mirror(u8),
    /// `auxC`: an auxiliary image's type, e.g. alpha.
    Auxiliary(String),
    Other([u8; 4]),
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

/// What a HEIF (AVIF) file's `ftyp` and `meta` say.
#[derive(Clone, Debug, Default)]
pub struct Heif {
    /// Major brand first, then the compatible ones.
    pub brands: Vec<[u8; 4]>,
    pub primary: u32,
    pub items: Vec<Item>,
    locations: Vec<Location>,
    properties: Vec<Property>,
    /// Each item's properties, as 1-based indexes into `properties`.
    associations: Vec<(u32, Vec<u16>)>,
    /// (kind, from, to).
    references: Vec<([u8; 4], u32, u32)>,
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
            .filter_map(|i| self.properties.get((*i as usize).checked_sub(1)?))
            .collect()
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

    /// Item `id`'s data from the file, its extents joined; up to 256 MiB, and
    /// `None` when an extent runs past the file. Construction method 1 (`idat`)
    /// and length 0 ("to the end of the file") are not read.
    pub fn data<R: Read + Seek>(&self, r: &mut R, id: u32) -> Option<Vec<u8>> {
        let l = self
            .locations
            .iter()
            .find(|l| l.id == id && l.method == 0)?;
        let file_len = file_len(r)?;
        let mut total = 0u64;
        for (offset, len) in &l.extents {
            let end = l.base.checked_add(*offset)?.checked_add(*len)?;
            total = total.checked_add(*len)?;
            if end > file_len || total > MAX_ITEM {
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

    /// The items with a `kind` reference to item `of`: for "cdsc", the
    /// metadata that describes it.
    pub fn describing(&self, kind: [u8; 4], of: u32) -> Vec<u32> {
        self.references
            .iter()
            .filter(|(k, _, to)| *k == kind && *to == of)
            .map(|(_, from, _)| *from)
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
                b"ipma" => self.associations = associations(Bytes(body))?,
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

fn associations(mut b: Bytes) -> Option<Vec<(u32, Vec<u16>)>> {
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
                b.u16()? & 0x7fff
            } else {
                (b.u8()? & 0x7f) as u16
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
