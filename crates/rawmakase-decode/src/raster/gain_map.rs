//! Gain maps over a JPEG or JPEG XL base (docs/hdr.md#gain-map-files), read as
//! RAWmakase's own gain-map exports write them (`export::gain_map`, `export::jxl`):
//! - **JPEG:** the base carries an APP2 version marker naming ISO 21496-1 and an MPF
//!   index. The index's secondary image is the map, a JPEG whose own APP2 segment
//!   holds the metadata. This is the form libultrahdr, Android, Apple and Adobe write.
//! - **JPEG, XMP only:** Adobe's earlier gain-map format (`hdrgm`, which Android's
//!   Ultra HDR 1.0 adopted): the base's XMP says `hdrgm:Version`, the MPF index finds
//!   the map, and the map's own XMP holds the metadata.
//! - **JPEG XL:** a `jhgm` box (libjxl's gain-map bundle) holds the metadata, the
//!   alternate rendition's colour, and the map's own codestream.
//!
//! The map is reconstructed at its full weight, as a display with the alternate
//! rendition's whole headroom would show it. That makes the light the authored HDR
//! rendition, the same on every display: no monitor's headroom reaches the decode.
//!
//! Everything is read with bounds. Segment, box, MPF and metadata lengths are checked
//! against the file before anything is allocated from them. A map that is damaged, or
//! of a form RAWmakase doesn't apply, is [`Unsupported`] with the reason.
use super::Unsupported;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// The APP2 payload that names ISO 21496-1, before its versions or metadata.
const ISO_URN: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0";
/// The APP1 payload of a JPEG's XMP packet, before the packet.
const XMP_URN: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
/// The APP1 payload of a chunk of a JPEG's extended XMP, before its GUID.
const XMP_EXTENSION: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
/// Adobe's gain-map XMP namespace (`hdrgm`).
const HDRGM: &str = "http://ns.adobe.com/hdr-gain-map/1.0/";
/// The most a JPEG's segments before its image data may take, read in full.
const MAX_HEADER: u64 = 16 << 20;
/// The most segments a JPEG may have before its image data.
const MAX_SEGMENTS: usize = 4096;
/// The largest map read into memory, encoded.
const MAX_MAP: u64 = 256 << 20;
/// The most images an MPF index may list.
const MAX_IMAGES: u32 = 32;
/// The largest gain or headroom accepted, in stops either way. ISO 21496-1 sets no
/// bound; 16 stops is beyond any display, and keeps every gain finite in f32.
const MAX_STOPS: f64 = 16.;

fn damaged() -> Unsupported {
    Unsupported("damaged gain-map".into())
}

/// One colour channel's gain: log2 of the smallest and largest gain the map
/// encodes, the map's gamma, and the offsets added to the base and taken from the
/// alternate, in linear units where 1 is the base's white.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Channel {
    min: f32,
    max: f32,
    gamma: f32,
    base_offset: f32,
    alternate_offset: f32,
}

impl Channel {
    /// A channel of `[min, max, gamma, base offset, alternate offset]`, checked:
    /// gains within [`MAX_STOPS`] and the maximum not below the minimum, a gamma above
    /// 0, and offsets from 0 to `offset_max`. [`damaged`] otherwise.
    fn checked(
        [min, max, gamma, base_offset, alternate_offset]: [f64; 5],
        offset_max: f64,
    ) -> Result<Self, Unsupported> {
        let stops = |v: f64| v.abs() <= MAX_STOPS;
        let offset = |v: f64| (0. ..=offset_max).contains(&v);
        if !(stops(min) && stops(max) && max >= min)
            || !(gamma > 0. && gamma <= MAX_STOPS)
            || !(offset(base_offset) && offset(alternate_offset))
        {
            return Err(damaged());
        }
        Ok(Self {
            min: min as f32,
            max: max as f32,
            gamma: gamma as f32,
            base_offset: base_offset as f32,
            alternate_offset: alternate_offset as f32,
        })
    }

    /// The alternate rendition of linear base value `base`, where the map's sample
    /// is `sample` (0–1), at the map's full weight.
    fn apply(&self, base: f32, sample: f32) -> f32 {
        let mut v = sample.clamp(0., 1.);
        if self.gamma != 1. {
            v = v.powf(1. / self.gamma);
        }
        let stops = self.min + (self.max - self.min) * v;
        (base + self.base_offset) * stops.exp2() - self.alternate_offset
    }
}

/// A gain map's ISO 21496-1 metadata, validated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Metadata {
    /// Each channel's gain; a single-channel map's three are the same.
    channels: [Channel; 3],
    /// The map applies in the base image's colour space, not the alternate's.
    base_colour_space: bool,
}

/// A big-endian reader over a byte slice that fails as [`damaged`] past its end.
struct Bytes<'a>(&'a [u8]);
impl Bytes<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Unsupported> {
        let (head, rest) = self.0.split_first_chunk::<N>().ok_or_else(damaged)?;
        self.0 = rest;
        Ok(*head)
    }
    fn u8(&mut self) -> Result<u8, Unsupported> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, Unsupported> {
        Ok(u16::from_be_bytes(self.take()?))
    }
    fn u32(&mut self) -> Result<u32, Unsupported> {
        Ok(u32::from_be_bytes(self.take()?))
    }
    fn i32(&mut self) -> Result<i32, Unsupported> {
        Ok(i32::from_be_bytes(self.take()?))
    }
}

impl Metadata {
    /// The metadata ISO 21496-1 serialises, as libultrahdr 2.0 reads it:
    /// - minimum and writer versions (u16 each);
    /// - flags: multichannel (0x80), base colour space (0x40), common denominator
    ///   (0x08), and the base as the HDR rendition (0x04);
    /// - the base's and the alternate's headroom;
    /// - then for each channel the map's minimum, maximum and gamma and the two
    ///   offsets.
    ///
    /// Each value is a numerator over its own denominator, or over a common one that
    /// comes first. Bytes after the last field are left alone, as a later writer
    /// version may add some.
    ///
    /// Refused:
    /// - a minimum version above 0, by its number;
    /// - a base that is the HDR rendition, by its flag or by a headroom at or above
    ///   the alternate's;
    /// - as damaged: a short payload, a zero denominator, a maximum below its
    ///   minimum, a gamma not above 0, a negative offset or one above 1, and gains or
    ///   headrooms beyond [`MAX_STOPS`].
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, Unsupported> {
        let mut r = Bytes(bytes);
        let minimum = r.u16()?;
        if minimum != 0 {
            return Err(Unsupported(format!("version {minimum} gain-map")));
        }
        let _writer = r.u16()?;
        let flags = r.u8()?;
        if flags & 0x04 != 0 {
            return Err(hdr_base());
        }
        let count = if flags & 0x80 != 0 { 3 } else { 1 };
        let common = match flags & 0x08 != 0 {
            true => Some(r.u32()?),
            false => None,
        };
        let mut rational = |signed: bool| -> Result<f64, Unsupported> {
            let n = if signed {
                f64::from(r.i32()?)
            } else {
                f64::from(r.u32()?)
            };
            let d = match common {
                Some(d) => d,
                None => r.u32()?,
            };
            if d == 0 {
                return Err(damaged());
            }
            Ok(n / f64::from(d))
        };
        let base_headroom = rational(false)?;
        let alternate_headroom = rational(false)?;
        let mut channels = [None; 3];
        for channel in channels.iter_mut().take(count) {
            let min = rational(true)?;
            let max = rational(true)?;
            let gamma = rational(false)?;
            let base_offset = rational(true)?;
            let alternate_offset = rational(true)?;
            *channel = Some(Channel::checked(
                [min, max, gamma, base_offset, alternate_offset],
                1.,
            )?);
        }
        if !(base_headroom <= MAX_STOPS && alternate_headroom <= MAX_STOPS) {
            return Err(damaged());
        }
        if base_headroom >= alternate_headroom {
            return Err(hdr_base());
        }
        let first = channels[0].ok_or_else(damaged)?;
        Ok(Self {
            channels: channels.map(|c| c.unwrap_or(first)),
            base_colour_space: flags & 0x40 != 0,
        })
    }

    /// The metadata of Adobe's gain-map XMP (`hdrgm`, version 1.0), from a map's XMP
    /// packet, as Android's Ultra HDR 1.0 specification lists it:
    /// - `Version`, which must be "1.0";
    /// - `BaseRenditionIsHDR`, "False" when absent;
    /// - `GainMapMin` (0 when absent) and `GainMapMax`, log2 of the gains;
    /// - `Gamma` (1), `OffsetSDR` and `OffsetHDR` (1/64 each), linear;
    /// - `HDRCapacityMin` (0) and `HDRCapacityMax`, log2 of the display headroom
    ///   between which a display weighs the map in. The map is applied at full weight
    ///   whatever they are, so they are only checked.
    ///
    /// The five per-channel values are each one value for every channel or three, one
    /// a channel. The map applies in the base's colour space: hdrgm has no other.
    ///
    /// Refused: a version other than 1.0 by its number, an HDR base, and as damaged:
    /// a missing `GainMapMax` or `HDRCapacityMax`, a value that isn't a number, two
    /// values or more than three, a capacity below 0 or a maximum not above its
    /// minimum, and the channel checks of [`Channel::checked`] (offsets only below 0:
    /// hdrgm sets them no upper bound, so theirs is [`MAX_STOPS`] stops).
    pub(super) fn from_hdrgm(xmp: &Hdrgm) -> Result<Self, Unsupported> {
        match xmp.get("Version").map(Vec::as_slice) {
            Some([v]) if v == "1.0" => {}
            Some([v]) => return Err(Unsupported(format!("version {v} gain-map"))),
            _ => return Err(damaged()),
        }
        match xmp.get("BaseRenditionIsHDR").map(Vec::as_slice) {
            None => {}
            Some([v]) if v.eq_ignore_ascii_case("false") => {}
            Some([v]) if v.eq_ignore_ascii_case("true") => return Err(hdr_base()),
            Some(_) => return Err(damaged()),
        }
        let values = |name: &str, default: Option<f64>| -> Result<Vec<f64>, Unsupported> {
            match xmp.get(name) {
                None => default.map(|d| vec![d]).ok_or_else(damaged),
                Some(v) if v.len() == 1 || v.len() == 3 => v
                    .iter()
                    .map(|v| v.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(damaged),
                Some(_) => Err(damaged()),
            }
        };
        let single = |name: &str, default: Option<f64>| -> Result<f64, Unsupported> {
            match values(name, default)?.as_slice() {
                [v] => Ok(*v),
                _ => Err(damaged()),
            }
        };
        let fields = [
            values("GainMapMin", Some(0.))?,
            values("GainMapMax", None)?,
            values("Gamma", Some(1.))?,
            values("OffsetSDR", Some(1. / 64.))?,
            values("OffsetHDR", Some(1. / 64.))?,
        ];
        let capacity_min = single("HDRCapacityMin", Some(0.))?;
        let capacity_max = single("HDRCapacityMax", None)?;
        if !(capacity_min >= 0. && capacity_max > capacity_min && capacity_max <= MAX_STOPS) {
            return Err(damaged());
        }
        let mut channels = [None; 3];
        for (c, channel) in channels.iter_mut().enumerate() {
            let v = fields
                .each_ref()
                .map(|f| if f.len() == 3 { f[c] } else { f[0] });
            *channel = Some(Channel::checked(v, MAX_STOPS.exp2())?);
        }
        Ok(Self {
            channels: channels.map(|c| c.unwrap_or_else(|| unreachable!("every channel is set"))),
            base_colour_space: true,
        })
    }
}

/// The key in [`Hdrgm`] of `xmpNote:HasExtendedXMP`, the GUID of the extended packet
/// that holds the rest of the properties; no hdrgm name has a colon.
const EXTENDED: &str = "xmpNote:HasExtendedXMP";

/// The `hdrgm` properties of an XMP packet: each one's value, or the items of its
/// ordered array, as text. Properties are attributes of an `rdf:Description` or
/// elements of their own, as XMP allows either.
pub(super) type Hdrgm = std::collections::HashMap<String, Vec<String>>;

/// The `hdrgm` properties of the XMP `packet`; `None` for a packet that isn't XML.
/// Only the image's own properties are read: those of each `rdf:Description` of the
/// packet's `rdf:RDF`, not those of a resource nested in one. An array is an
/// `rdf:Seq` of `rdf:li` items, ordered as hdrgm asks; any other structure (an
/// `rdf:Bag`, an `rdf:Alt`) is kept with no value, which no field accepts.
fn hdrgm(packet: &[u8]) -> Option<Hdrgm> {
    const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    const NOTE: &str = "http://ns.adobe.com/xmp/note/";
    let text = std::str::from_utf8(packet).ok()?;
    // An XMP packet's wrapper (`<?xpacket ...?>`) is a processing instruction, which
    // XML allows; trailing padding after it is whitespace.
    let doc = roxmltree::Document::parse(text.trim_end_matches('\0')).ok()?;
    let mut found = Hdrgm::new();
    let Some(rdf) = doc.descendants().find(|n| n.has_tag_name((RDF, "RDF"))) else {
        return Some(found);
    };
    // A literal: an element of text alone (its comments aside), no resource.
    let literal = |n: roxmltree::Node<'_, '_>| -> Option<String> {
        let plain = !n.children().any(|c| c.is_element())
            && !n.attributes().any(|a| a.namespace() == Some(RDF));
        plain.then(|| {
            n.children()
                .filter(|c| c.is_text())
                .filter_map(|c| c.text())
                .collect()
        })
    };
    let blank = |n: roxmltree::Node<'_, '_>| {
        n.children()
            .filter(|c| c.is_text())
            .all(|c| c.text().unwrap_or_default().trim().is_empty())
    };
    for description in rdf
        .children()
        .filter(|n| n.has_tag_name((RDF, "Description")))
    {
        for a in description.attributes() {
            if a.namespace() == Some(HDRGM) {
                found.insert(a.name().to_owned(), vec![a.value().to_owned()]);
            } else if a.namespace() == Some(NOTE) && a.name() == "HasExtendedXMP" {
                found.insert(EXTENDED.into(), vec![a.value().to_owned()]);
            }
        }
        for property in description.children().filter(|n| n.is_element()) {
            let name = property.tag_name();
            if name.namespace() == Some(NOTE) && name.name() == "HasExtendedXMP" {
                let guid = literal(property).unwrap_or_default();
                found.insert(EXTENDED.into(), vec![guid]);
                continue;
            }
            if name.namespace() != Some(HDRGM) {
                continue;
            }
            let inner: Vec<_> = property.children().filter(|n| n.is_element()).collect();
            let value = match inner.as_slice() {
                [] => literal(property).map(|v| vec![v]).unwrap_or_default(),
                [seq] if seq.has_tag_name((RDF, "Seq")) && blank(property) && blank(*seq) => seq
                    .children()
                    .filter(|n| n.is_element())
                    .map(|li| li.has_tag_name((RDF, "li")).then(|| literal(li)).flatten())
                    .collect::<Option<Vec<_>>>()
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            found.insert(name.name().to_owned(), value);
        }
    }
    Some(found)
}

/// The extended XMP packet `guid` of a JPEG from its APP1 `chunks` (each after its
/// URN): a 32-character GUID, the packet's length and the chunk's offset in it (u32
/// each, big-endian), then the chunk. Those of `guid` must agree on its length and
/// together fill it once, in any order; anything else, none among them, is
/// [`damaged`].
fn extended(chunks: &[Vec<u8>], guid: &[u8]) -> Result<Vec<u8>, Unsupported> {
    let mut parts = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let mut r = Bytes(chunk);
        let id = r.take::<32>()?;
        let (len, offset) = (r.u32()?, r.u32()?);
        if id.as_slice() == guid {
            parts.push((len, offset, r.0));
        }
    }
    let len = parts.first().ok_or_else(damaged)?.0;
    if u64::from(len) > MAX_HEADER || parts.iter().any(|p| p.0 != len) {
        return Err(damaged());
    }
    parts.sort_by_key(|p| p.1);
    let mut packet = Vec::with_capacity(len as usize);
    for (_, offset, data) in parts {
        if offset as usize != packet.len() {
            return Err(damaged());
        }
        packet.extend_from_slice(data);
    }
    if packet.len() != len as usize {
        return Err(damaged());
    }
    Ok(packet)
}

fn hdr_base() -> Unsupported {
    Unsupported("HDR-base gain-map".into())
}

/// Where a file's map is and what its header says.
#[derive(Clone, Copy, Debug)]
pub(super) struct GainMap {
    pub(super) metadata: Metadata,
    /// The map's encoded image: a whole JPEG, or a naked JPEG XL codestream.
    offset: u64,
    len: u64,
    jxl: bool,
    /// Its size, from its header, which the decoded map must have.
    width: u32,
    height: u32,
}

/// The gain map of the JPEG or JPEG XL `file`, whose base image is `base` (width,
/// height) as its decoder reads it, before any EXIF orientation. `None` for a file
/// without one, which opens as an SDR file. A file whose base says it has an ISO
/// 21496-1 map but whose map can't be applied is [`Unsupported`].
///
/// A JPEG's map is described by ISO 21496-1 or, without it, by Adobe's `hdrgm` XMP;
/// either way the MPF index locates it.
pub(super) fn find(file: &mut File, base: (u32, u32)) -> Result<Option<GainMap>, Unsupported> {
    let len = file.seek(SeekFrom::End(0)).map_err(|_| damaged())?;
    let mut head = [0u8; 12];
    file.seek(SeekFrom::Start(0)).map_err(|_| damaged())?;
    if file.read_exact(&mut head).is_err() {
        return Ok(None);
    }
    let found = if head.starts_with(&[0xff, 0xd8]) {
        jpeg(file, len)?
    } else if head == rawmakase_interop::isobmff::JXL_CONTAINER {
        jxl(file, len)?
    } else {
        None
    };
    let Some(found) = found else {
        return Ok(None);
    };
    // The map covers the base: at most its size, at least one sample, and the same
    // shape to within a map sample's rounding, as libultrahdr's downsampled maps are.
    let (w, h) = (u64::from(base.0), u64::from(base.1));
    let (mw, mh) = (u64::from(found.width), u64::from(found.height));
    let shaped = mw >= 1 && mh >= 1 && mw <= w && mh <= h && (w * mh).abs_diff(h * mw) <= w + h;
    if !shaped {
        return Err(damaged());
    }
    Ok(Some(found))
}

/// A JPEG segment before the image data: its marker and where its payload is.
struct Segment {
    marker: u8,
    start: u64,
    len: u64,
}

/// The segments of the JPEG at `start`, ending at `end`, before its first scan.
/// Read through a buffer: fill bytes before a marker are legal in any number, and a
/// read each would stall on megabytes of them.
fn segments(file: &mut File, start: u64, end: u64) -> Result<Vec<Segment>, Unsupported> {
    use std::io::BufReader;
    let mut found = Vec::new();
    let mut at = start + 2;
    file.seek(SeekFrom::Start(at)).map_err(|_| damaged())?;
    let mut r = BufReader::with_capacity(64 << 10, &mut *file);
    let byte = |r: &mut BufReader<&mut File>, at: &mut u64| -> Result<u8, Unsupported> {
        if *at >= end || *at - start > MAX_HEADER {
            return Err(damaged());
        }
        let mut b = [0u8; 1];
        r.read_exact(&mut b).map_err(|_| damaged())?;
        *at += 1;
        Ok(b[0])
    };
    loop {
        if found.len() >= MAX_SEGMENTS {
            return Err(damaged());
        }
        if byte(&mut r, &mut at)? != 0xff {
            return Err(damaged());
        }
        // Fill bytes before a marker.
        let mut marker = 0xff;
        while marker == 0xff {
            marker = byte(&mut r, &mut at)?;
        }
        match marker {
            // The first scan, or the end of an image without one.
            0xda | 0xd9 => return Ok(found),
            // Markers without a payload.
            0x01 | 0xd0..=0xd7 => continue,
            _ => {}
        }
        let size = u64::from(u16::from_be_bytes([
            byte(&mut r, &mut at)?,
            byte(&mut r, &mut at)?,
        ]));
        if size < 2 || at - 2 + size > end {
            return Err(damaged());
        }
        found.push(Segment {
            marker,
            start: at,
            len: size - 2,
        });
        let skip = size - 2;
        r.seek_relative(skip as i64).map_err(|_| damaged())?;
        at += skip;
    }
}

/// Fills `buf` from `file` at `at`, which must end by `end`.
fn read_at(file: &mut File, at: u64, end: u64, buf: &mut [u8]) -> Result<(), Unsupported> {
    if at.checked_add(buf.len() as u64).is_none_or(|e| e > end) {
        return Err(damaged());
    }
    file.seek(SeekFrom::Start(at)).map_err(|_| damaged())?;
    file.read_exact(buf).map_err(|_| damaged())
}

/// A segment's payload, at most 64 KiB as JPEG allows.
fn payload(file: &mut File, s: &Segment, end: u64) -> Result<Vec<u8>, Unsupported> {
    let mut buf = vec![0u8; s.len as usize];
    read_at(file, s.start, end, &mut buf)?;
    Ok(buf)
}

/// The gain map of a JPEG of `len` bytes, from the ISO 21496-1 version marker on its
/// primary image, or the `hdrgm:Version` of its XMP, and its MPF index: the first
/// secondary image whose own metadata of that form reads.
fn jpeg(file: &mut File, len: u64) -> Result<Option<GainMap>, Unsupported> {
    // Only a primary whose segments read can be a gain map's base; one whose don't
    // is left to the JPEG decoder to say what is wrong.
    let Ok(primary) = segments(file, 0, len) else {
        return Ok(None);
    };
    let mut marker = None;
    let mut mpf = None;
    let mut xmp_version = None;
    for s in primary.iter().filter(|s| [0xe1, 0xe2].contains(&s.marker)) {
        let p = payload(file, s, len)?;
        if s.marker == 0xe1 {
            if let Some(packet) = p.strip_prefix(XMP_URN) {
                xmp_version = xmp_version.or_else(|| hdrgm(packet)?.remove("Version"));
            }
        } else if let Some(versions) = p.strip_prefix(ISO_URN) {
            marker = Some(versions.to_vec());
        } else if p.starts_with(b"MPF\0") {
            // Its offsets count from the TIFF header, after "MPF\0".
            mpf = Some((p, s.start + 4));
        }
    }
    // ISO 21496-1 wins over the XMP that writers such as libultrahdr add beside it.
    let iso = match (marker, xmp_version) {
        (Some(versions), _) => {
            let minimum = Bytes(&versions).u16()?;
            if minimum != 0 {
                return Err(Unsupported(format!("version {minimum} gain-map")));
            }
            true
        }
        (None, Some(version)) => match version.as_slice() {
            [v] if v == "1.0" => false,
            [v] => return Err(Unsupported(format!("version {v} gain-map"))),
            _ => return Err(damaged()),
        },
        (None, None) => return Ok(None),
    };
    let (mpf, tiff) = mpf.ok_or_else(damaged)?;
    for (offset, size) in mpf_images(&mpf[4..])? {
        let Some(start) = tiff.checked_add(u64::from(offset)) else {
            continue;
        };
        let end = start + u64::from(size);
        if offset == 0 || size < 4 || end > len || u64::from(size) > MAX_MAP {
            continue;
        }
        let mut soi = [0u8; 2];
        read_at(file, start, end, &mut soi)?;
        if soi != [0xff, 0xd8] {
            continue;
        }
        let Ok(map) = segments(file, start, end) else {
            continue;
        };
        let mut metadata = None;
        let mut icc = false;
        let mut frame = None;
        // An hdrgm map's XMP: its standard packet's properties and its extended
        // packet's chunks.
        let mut xmp = Hdrgm::new();
        let mut extension = Vec::new();
        for s in &map {
            match s.marker {
                0xe2 => {
                    let p = payload(file, s, end)?;
                    if let Some(m) = p.strip_prefix(ISO_URN).filter(|_| iso) {
                        metadata = Some(Metadata::parse(m)?);
                    }
                    icc |= p.starts_with(b"ICC_PROFILE\0");
                }
                0xe1 if !iso => {
                    let p = payload(file, s, end)?;
                    if let Some(packet) = p.strip_prefix(XMP_URN) {
                        xmp.extend(hdrgm(packet).unwrap_or_default());
                    } else if let Some(chunk) = p.strip_prefix(XMP_EXTENSION) {
                        extension.push(chunk.to_vec());
                    }
                }
                // Start of frame, any but the DHT, JPG and DAC markers among them.
                0xc0..=0xcf if ![0xc4, 0xc8, 0xcc].contains(&s.marker) => {
                    let p = payload(file, s, end)?;
                    let mut r = Bytes(&p);
                    let _precision = r.u8()?;
                    let height = r.u16()?;
                    let width = r.u16()?;
                    let components = r.u8()?;
                    if ![1, 3].contains(&components) {
                        return Err(damaged());
                    }
                    frame = Some((u32::from(width), u32::from(height)));
                }
                _ => {}
            }
        }
        // The extended packet the standard one names, which must be there whole;
        // chunks it doesn't name are another's, as XMP's storage in JPEG says.
        if let Some(guid) = xmp.remove(EXTENDED) {
            let [guid] = guid.as_slice() else {
                return Err(damaged());
            };
            let packet = extended(&extension, guid.as_bytes())?;
            let mut more = hdrgm(&packet).ok_or_else(damaged)?;
            more.remove(EXTENDED);
            xmp.extend(more);
        }
        if !iso && !xmp.is_empty() {
            metadata = Some(Metadata::from_hdrgm(&xmp)?);
        }
        let Some(metadata) = metadata else {
            continue;
        };
        let (width, height) = frame.ok_or_else(damaged)?;
        // A map in the alternate's colour space, which its ICC profile describes. An
        // hdrgm map's profile isn't used: it applies in the base's.
        if !metadata.base_colour_space && icc {
            return Err(Unsupported("alternate-colour gain-map".into()));
        }
        return Ok(Some(GainMap {
            metadata,
            offset: start,
            len: u64::from(size),
            jxl: false,
            width,
            height,
        }));
    }
    Err(damaged())
}

/// The images an MPF index lists after the first: each one's offset from the
/// index's TIFF header and its size, as `tiff` (the index after "MPF\0") gives them.
fn mpf_images(tiff: &[u8]) -> Result<Vec<(u32, u32)>, Unsupported> {
    let big = match tiff.get(..4) {
        Some(b"MM\0*") => true,
        Some(b"II*\0") => false,
        _ => return Err(damaged()),
    };
    let u16_at = |at: usize| -> Result<u16, Unsupported> {
        let b: [u8; 2] = tiff
            .get(at..at + 2)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(damaged)?;
        Ok(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Result<u32, Unsupported> {
        let b: [u8; 4] = tiff
            .get(at..at + 4)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(damaged)?;
        Ok(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    let mut entries = None;
    for i in 0..count.min(64) {
        let at = ifd + 2 + i * 12;
        // MPEntry: 16 bytes an image, its count the bytes they take.
        if u16_at(at)? == 0xb002 {
            let bytes = u32_at(at + 4)?;
            if bytes % 16 != 0 || bytes / 16 > MAX_IMAGES {
                return Err(damaged());
            }
            entries = Some((u32_at(at + 8)? as usize, bytes as usize / 16));
        }
    }
    let (at, n) = entries.ok_or_else(damaged)?;
    (1..n)
        .map(|i| Ok((u32_at(at + i * 16 + 8)?, u32_at(at + i * 16 + 4)?)))
        .collect()
}

/// The gain map in a JPEG XL container's `jhgm` box (libjxl's bundle): a version
/// byte (0), the ISO 21496-1 metadata after its u16 length, the alternate's colour
/// encoding after its u8 length, its ICC profile after a u32 length, and the map's
/// naked codestream, the rest of the box.
fn jxl(file: &mut File, len: u64) -> Result<Option<GainMap>, Unsupported> {
    use rawmakase_interop::isobmff::boxes;
    let all = boxes(file, 0, len);
    // `boxes` ends its listing at a box that runs past the file's end. A gain map cut
    // off there is damaged, not absent.
    let listed = all.last().map_or(0, |b| b.start + b.size);
    if listed + 8 <= len {
        let mut header = [0u8; 8];
        read_at(file, listed, len, &mut header)?;
        if &header[4..] == b"jhgm" {
            return Err(damaged());
        }
    }
    let Some(b) = all.iter().find(|b| &b.kind == b"jhgm") else {
        // A bundle compressed in a `brob` box, which libjxl doesn't write.
        for b in all.iter().filter(|b| &b.kind == b"brob" && b.size >= 4) {
            let mut inner = [0u8; 4];
            read_at(file, b.start, b.start + b.size, &mut inner)?;
            if &inner == b"jhgm" {
                return Err(Unsupported("compressed gain-map".into()));
            }
        }
        return Ok(None);
    };
    let end = b.start + b.size;
    let mut at = b.start;
    let mut read = |n: u64| -> Result<Vec<u8>, Unsupported> {
        let mut buf = vec![0u8; n.min(end.saturating_sub(at)) as usize];
        if buf.len() as u64 != n {
            return Err(damaged());
        }
        read_at(file, at, end, &mut buf)?;
        at += n;
        Ok(buf)
    };
    let version = read(1)?[0];
    if version != 0 {
        return Err(Unsupported(format!("version {version} gain-map")));
    }
    let size = u16::from_be_bytes(read(2)?.try_into().unwrap_or_default());
    let metadata = Metadata::parse(&read(u64::from(size))?)?;
    let colour = read(1)?[0];
    read(u64::from(colour))?;
    let icc = u32::from_be_bytes(read(4)?.try_into().unwrap_or_default());
    // The ICC profile is skipped, not read: only its length is needed.
    if at + u64::from(icc) > end {
        return Err(damaged());
    }
    at += u64::from(icc);
    if !metadata.base_colour_space && (colour > 0 || icc > 0) {
        return Err(Unsupported("alternate-colour gain-map".into()));
    }
    let codestream = end - at;
    if codestream == 0 || codestream > MAX_MAP {
        return Err(damaged());
    }
    file.seek(SeekFrom::Start(at)).map_err(|_| damaged())?;
    let header =
        super::jxl::Header::read(&mut (&mut *file).take(codestream)).map_err(|_| damaged())?;
    if header.animated {
        return Err(damaged());
    }
    if header.turned {
        return Err(Unsupported("turned JPEG XL gain-map".into()));
    }
    Ok(Some(GainMap {
        metadata,
        offset: at,
        len: codestream,
        jxl: true,
        width: header.width,
        height: header.height,
    }))
}

/// A decoded map: `channels` (1 or 3) samples a pixel, 0–1.
pub(super) struct Samples {
    width: u32,
    height: u32,
    channels: usize,
    data: Vec<f32>,
}

impl GainMap {
    /// The map's samples from `file`, decoded and checked against its header, then
    /// turned by `orientation`, the base's.
    pub(super) fn decode(
        &self,
        file: &mut File,
        orientation: image::metadata::Orientation,
    ) -> Result<Samples, Unsupported> {
        let mut bytes = vec![0u8; self.len as usize];
        read_at(file, self.offset, self.offset + self.len, &mut bytes)?;
        let mut image = if self.jxl {
            let decoder = jxl_oxide::integration::JxlDecoder::new(std::io::Cursor::new(bytes))
                .map_err(|_| damaged())?;
            image::DynamicImage::from_decoder(decoder).map_err(|_| damaged())?
        } else {
            image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
                .map_err(|_| damaged())?
        };
        if (image.width(), image.height()) != (self.width, self.height) {
            return Err(damaged());
        }
        image.apply_orientation(orientation);
        let (width, height) = (image.width(), image.height());
        let (channels, data) = if image.color().has_color() {
            (3, image.into_rgb32f().into_raw())
        } else {
            (1, image.to_luma32f().into_raw())
        };
        Ok(Samples {
            width,
            height,
            channels,
            data,
        })
    }
}

impl Samples {
    /// Channel `c` of the map at the centre of base pixel (`x`, `y`) of a `width` ×
    /// `height` base: bilinear between the map's sample centres, which are spread
    /// over the base, the edges held. A map the base's size is read as it is.
    fn at(&self, x: u32, y: u32, width: u32, height: u32, c: usize) -> f32 {
        let c = c.min(self.channels - 1);
        let sample = |mx: u32, my: u32| {
            self.data[(my as usize * self.width as usize + mx as usize) * self.channels + c]
        };
        if (self.width, self.height) == (width, height) {
            return sample(x, y);
        }
        let position = |v: u32, base: u32, map: u32| {
            let p = ((v as f32 + 0.5) * map as f32 / base as f32 - 0.5).clamp(0., (map - 1) as f32);
            let low = p.floor() as u32;
            (low, (low + 1).min(map - 1), p - low as f32)
        };
        let (x0, x1, fx) = position(x, width, self.width);
        let (y0, y1, fy) = position(y, height, self.height);
        let top = sample(x0, y0) * (1. - fx) + sample(x1, y0) * fx;
        let bottom = sample(x0, y1) * (1. - fx) + sample(x1, y1) * fx;
        top * (1. - fy) + bottom * fy
    }
}

/// The alternate rendition of `base` (linear RGB of a `width` × `height` image in
/// the base's own primaries, 1 = its white, already upright), by `map` at its full
/// weight, in the same primaries.
pub(super) fn reconstruct(
    base: &[[f32; 3]],
    width: u32,
    height: u32,
    map: &Samples,
    metadata: &Metadata,
    cancel: rawmakase_cancel::Cancel<'_>,
) -> anyhow::Result<Vec<[f32; 3]>> {
    use rayon::prelude::*;
    let mut out = vec![[0f32; 3]; base.len()];
    if out.is_empty() {
        return Ok(out);
    }
    anyhow::ensure!(
        base.len() == width as usize * height as usize,
        "The gain map's base has the wrong size"
    );
    out.par_chunks_mut(width as usize)
        .zip(base.par_chunks(width as usize))
        .enumerate()
        .try_for_each(|(y, (to, from))| {
            anyhow::ensure!(!cancel.is_cancelled(), "Opening cancelled");
            for (x, (to, from)) in to.iter_mut().zip(from).enumerate() {
                for c in 0..3 {
                    let sample = map.at(x as u32, y as u32, width, height, c);
                    to[c] = metadata.channels[c].apply(from[c], sample);
                }
            }
            Ok(())
        })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A metadata payload: versions, `flags`, then `values` as big-endian u32s.
    fn payload(flags: u8, values: &[u32]) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 0, flags];
        for v in values {
            out.extend(v.to_be_bytes());
        }
        out
    }

    /// The common-denominator form libultrahdr may write reads as the per-value one.
    #[test]
    fn a_common_denominator_reads_as_separate_ones() {
        let d = 1 << 10;
        let shared = payload(0x40 | 0x08, &[d, 0, 3 * d, 0, 2 * d, d, d / 64, d / 64]);
        let own = payload(
            0x40,
            &[0, d, 3 * d, d, 0, d, 2 * d, d, d, d, d / 64, d, d / 64, d],
        );
        let (a, b) = (
            Metadata::parse(&shared).unwrap(),
            Metadata::parse(&own).unwrap(),
        );
        assert_eq!(a, b);
        assert_eq!(a.channels[0].max, 2.);
        assert_eq!(a.channels[2], a.channels[0], "one channel serves all three");
        assert!(a.base_colour_space);
    }

    /// A multichannel map has a gain per channel, in order.
    #[test]
    fn a_multichannel_map_has_a_gain_per_channel() {
        let d = 1u32;
        let mut values = vec![0, d, 2, d];
        for max in [1, 2, 3] {
            values.extend([0, d, max, d, 1, d, 0, d, 0, d]);
        }
        let m = Metadata::parse(&payload(0x80, &values)).unwrap();
        assert_eq!(m.channels.map(|c| c.max), [1., 2., 3.]);
        assert!(!m.base_colour_space);
    }

    /// Every field is read within the payload: each shorter prefix of a valid one is
    /// damaged, never a panic.
    #[test]
    fn every_truncation_is_damaged() {
        let d = 1u32;
        let valid = payload(0x40, &[0, d, 2, d, 0, d, 2, d, 1, d, 0, d, 0, d]);
        Metadata::parse(&valid).unwrap();
        for n in 0..valid.len() {
            assert_eq!(
                Metadata::parse(&valid[..n]).unwrap_err().0,
                "damaged gain-map"
            );
        }
    }

    /// Gains are applied as ISO 21496-1 writes them: log2 gain interpolated between
    /// the minimum and the maximum by the sample raised to 1/gamma, the offsets added
    /// to the base and taken from the result.
    #[test]
    fn a_channel_applies_its_gain_with_its_offsets_and_gamma() {
        let c = Channel {
            min: -1.,
            max: 3.,
            gamma: 2.,
            base_offset: 0.25,
            alternate_offset: 0.125,
        };
        assert_eq!(c.apply(0.75, 0.), 0.5 - 0.125);
        assert_eq!(c.apply(0.75, 1.), 8. - 0.125);
        // 0.25 raised to 1/2 is half way: 1 stop.
        assert!((c.apply(0.75, 0.25) - (2. - 0.125)).abs() < 1e-6);
    }

    /// A map half the base's size is sampled between its centres, and holds its
    /// edges; one the base's size is read sample for sample.
    #[test]
    fn a_smaller_map_is_interpolated_between_its_centres() {
        let map = Samples {
            width: 2,
            height: 1,
            channels: 1,
            data: vec![0., 1.],
        };
        let row: Vec<f32> = (0..4).map(|x| map.at(x, 0, 4, 1, 0)).collect();
        assert_eq!(row, [0., 0.25, 0.75, 1.]);
        assert_eq!(map.at(1, 0, 2, 1, 2), 1.);
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/formats")
            .join(name)
    }

    /// libultrahdr 2.0.2's decode of `ultrahdr.jpg` at its full headroom, as linear
    /// Display P3 half floats (scripts/make-gain-map-fixtures.py prints them).
    const LIBULTRAHDR: [(usize, usize, [f32; 3]); 8] = [
        (0, 24, [0.015213, 0.01535, 0.014]),
        (9, 24, [0.034729, 0.033234, 0.029892]),
        (18, 24, [0.16943, 0.17078, 0.151]),
        (27, 24, [0.5293, 0.55713, 0.48682]),
        (36, 24, [1.2695, 1.3271, 1.123]),
        (45, 24, [2.3594, 2.4512, 2.2168]),
        (54, 24, [4.0859, 4.25, 3.7988]),
        (63, 24, [6.5039, 6.793, 6.0391]),
    ];

    /// libultrahdr's own Ultra HDR JPEG (a Display P3 base, a three-channel map with
    /// its own ICC profile, in the base's colour space) opens with the light
    /// libultrahdr decodes at full headroom, to its half floats' and the two JPEG
    /// decoders' rounding. Its quarter-size map spreads over the base: the same
    /// pixels come within the map's interpolation of the full map's light.
    #[test]
    fn libultrahdrs_gain_map_opens_as_its_decoder_reads_it() {
        use rawmakase_model::color::{
            hdr_output::{HdrGamut, HdrTransfer},
            mul,
        };
        let open =
            |name| super::super::open(&fixture(name), rawmakase_cancel::Cancel::NEVER).unwrap();
        let (full, quarter) = (open("ultrahdr.jpg"), open("ultrahdr-quarter-map.jpg"));
        let to_p3 = HdrGamut::DisplayP3.prophoto_to_rgb();
        let k = 1. / 64.;
        let stops = |a: f32, b: f32| ((a + k) / (b + k)).log2().abs();
        for (x, y, want) in LIBULTRAHDR {
            let at = y * 64 + x;
            let got = mul(to_p3, full.hdr().unwrap().pixels()[at]);
            let spread = mul(to_p3, quarter.hdr().unwrap().pixels()[at]);
            // Measured: 0.019 stop at most against libultrahdr, and 0.04 between the
            // maps but at the right edge, where the ramp is steepest and the quarter
            // map's last sample is held (0.10).
            for c in 0..3 {
                assert!(
                    stops(got[c], want[c]) <= 0.025,
                    "({x}, {y}): {got:?} against {want:?}"
                );
                assert!(
                    stops(spread[c], got[c]) <= 0.12,
                    "({x}, {y}): {spread:?} against {got:?}"
                );
            }
        }
        assert_eq!(full.hdr().unwrap().source().transfer, HdrTransfer::Linear);
        assert!(full.hdr().unwrap().source().gain_map);
    }

    /// A JPEG segment: `marker`, its length and `payload`.
    fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0xff, marker];
        out.extend(((payload.len() + 2) as u16).to_be_bytes());
        out.extend(payload);
        out
    }

    /// An XMP APP1 segment whose `rdf:Description` holds `body` (attributes, then
    /// `>` and elements).
    fn xmp(body: &str) -> Vec<u8> {
        let packet = format!(
            r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/" {body}</rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#
        );
        segment(0xe1, &[XMP_URN, packet.as_bytes()].concat())
    }

    /// An 8-bit JPEG of `pixels`, `channels` (1 or 3) a pixel.
    fn encoded(pixels: &[u8], width: u32, height: u32, channels: usize) -> Vec<u8> {
        use image::ImageEncoder;
        let color = match channels {
            1 => image::ExtendedColorType::L8,
            _ => image::ExtendedColorType::Rgb8,
        };
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 100)
            .write_image(pixels, width, height, color)
            .unwrap();
        out
    }

    /// An MPF APP2 segment, big-endian, listing the primary (`primary` bytes) and a
    /// map of `map` bytes at `offset` from its TIFF header.
    fn mpf(primary: u32, map: u32, offset: u32) -> Vec<u8> {
        let mut t = b"MPF\0MM\0\x2a\0\0\0\x08".to_vec();
        t.extend(3u16.to_be_bytes());
        // Version, number of images, then the MP entries after the IFD.
        t.extend([0xb0, 0x00, 0, 7, 0, 0, 0, 4]);
        t.extend(b"0100");
        t.extend([0xb0, 0x01, 0, 4, 0, 0, 0, 1, 0, 0, 0, 2]);
        t.extend([0xb0, 0x02, 0, 7, 0, 0, 0, 32]);
        t.extend((8u32 + 2 + 3 * 12 + 4).to_be_bytes());
        t.extend(0u32.to_be_bytes());
        for (size, at) in [(primary, 0), (map, offset)] {
            t.extend(0u32.to_be_bytes());
            t.extend(size.to_be_bytes());
            t.extend(at.to_be_bytes());
            t.extend([0; 4]);
        }
        segment(0xe2, &t)
    }

    /// A gain-map JPEG: `base` with `primary_xmp` (and `exif`, an APP1 payload, when
    /// given) and an MPF index, then `map` with `map_xmp`.
    fn hdrgm_jpeg(
        base: &[u8],
        map: &[u8],
        primary_xmp: &str,
        map_xmp: &str,
        exif: Option<&[u8]>,
    ) -> Vec<u8> {
        hdrgm_jpeg_with(base, map, primary_xmp, &xmp(map_xmp), exif)
    }

    /// [`hdrgm_jpeg`] with the map's XMP segments given whole.
    fn hdrgm_jpeg_with(
        base: &[u8],
        map: &[u8],
        primary_xmp: &str,
        map_segments: &[u8],
        exif: Option<&[u8]>,
    ) -> Vec<u8> {
        let map = [&map[..2], map_segments, &map[2..]].concat();
        let mut head = exif.map(|e| segment(0xe1, e)).unwrap_or_default();
        head.extend(xmp(primary_xmp));
        // The MPF segment's own size doesn't depend on its values.
        let mpf_len = mpf(0, 0, 0).len();
        let primary_len = 2 + head.len() + mpf_len + base.len() - 2;
        // The TIFF header is 4 bytes into the MPF segment's payload.
        let tiff = 2 + head.len() + 4 + 4;
        let index = mpf(
            primary_len as u32,
            map.len() as u32,
            (primary_len - tiff) as u32,
        );
        [&base[..2], &head, &index, &base[2..], &map].concat()
    }

    /// Adobe's gain-map decoding at full weight, as the Ultra HDR 1.0 specification
    /// writes it, for one channel: the sample linear 0–1, then raised to 1/gamma,
    /// the gain interpolated in stops between the map's minimum and maximum, applied
    /// to the base with the offsets.
    fn adobe(sdr: f64, sample: u8, min: f64, max: f64, gamma: f64, offsets: (f64, f64)) -> f64 {
        let recovery = (f64::from(sample) / 255.).powf(1. / gamma);
        let boost = min * (1. - recovery) + max * recovery;
        (sdr + offsets.0) * boost.exp2() - offsets.1
    }

    /// The reader's error against the formulas in f64, relative: moxcms's f32 sRGB
    /// curve, measured at 3e-4 at most, f32 arithmetic and the ProPhoto round trip.
    const TOLERANCE: f64 = 1e-3;

    fn srgb_eotf(v: u8) -> f64 {
        let v = f64::from(v) / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    /// The light of `file` in linear sRGB primaries.
    fn light_of(path: &std::path::Path) -> anyhow::Result<(u32, u32, Vec<[f64; 3]>)> {
        use rawmakase_model::color::{hdr_output::HdrGamut, mul};
        let image = super::super::open(path, rawmakase_cancel::Cancel::NEVER)?;
        let hdr = image.hdr().ok_or_else(|| anyhow::anyhow!("no HDR"))?;
        assert!(hdr.source().gain_map);
        let to_srgb = HdrGamut::Rec709.prophoto_to_rgb();
        Ok((
            image.width,
            image.height,
            hdr.pixels()
                .iter()
                .map(|p| mul(to_srgb, *p).map(f64::from))
                .collect(),
        ))
    }

    /// A ramp of grey and colour patches a channel each, with the decoded samples the
    /// file holds (the JPEG decoder's, as the reader's).
    fn base_ramp(width: u32, height: u32) -> Vec<u8> {
        (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                let v = (x * 255 / (width - 1)) as u8;
                match y * 3 / height {
                    0 => [v, v, v],
                    1 => [v, v / 2, 40],
                    _ => [30, v / 3, v],
                }
            })
            .collect()
    }

    fn decoded(jpeg: &[u8]) -> image::DynamicImage {
        image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).unwrap()
    }

    /// A JPEG whose map is described only by Adobe's `hdrgm` XMP (Lightroom's and
    /// Android's earlier form) opens with the light the specification's formula gives,
    /// whether its values are attributes or elements, one for every channel or three.
    #[test]
    fn an_hdrgm_gain_map_opens_with_the_light_its_formula_gives() -> anyhow::Result<()> {
        let (w, h) = (32, 24);
        let base = encoded(&base_ramp(w, h), w, h, 3);
        let map_pixels: Vec<u8> = (0..w * h)
            .map(|i| ((i % w) * 255 / (w - 1)) as u8)
            .collect();
        let map = encoded(&map_pixels, w, h, 1);
        let (sdr, gains) = (decoded(&base).into_rgb8(), decoded(&map).into_luma8());
        let dir = tempfile::tempdir()?;
        let forms = [
            // Lightroom's attributes, one value each, the defaults left out but two.
            (
                r#"hdrgm:Version="1.0" hdrgm:GainMapMin="-0.5" hdrgm:GainMapMax="2.5" hdrgm:Gamma="1.4" hdrgm:HDRCapacityMax="2.5">"#,
                [(-0.5, 2.5, 1.4, (1. / 64., 1. / 64.)); 3],
            ),
            // Elements, an ordered array a channel.
            (
                r#"hdrgm:Version="1.0" hdrgm:HDRCapacityMin="0" hdrgm:HDRCapacityMax="3"><hdrgm:GainMapMin><rdf:Seq><rdf:li>0</rdf:li><rdf:li>-1</rdf:li><rdf:li>0.25</rdf:li></rdf:Seq></hdrgm:GainMapMin><hdrgm:GainMapMax><rdf:Seq><rdf:li>3</rdf:li><rdf:li>2</rdf:li><rdf:li>1</rdf:li></rdf:Seq></hdrgm:GainMapMax><hdrgm:OffsetSDR>0</hdrgm:OffsetSDR><hdrgm:OffsetHDR>0.03125</hdrgm:OffsetHDR>"#,
                [
                    (0., 3., 1., (0., 0.03125)),
                    (-1., 2., 1., (0., 0.03125)),
                    (0.25, 1., 1., (0., 0.03125)),
                ],
            ),
        ];
        for (i, (metadata, channels)) in forms.into_iter().enumerate() {
            let path = dir.path().join(format!("{i}.jpg"));
            std::fs::write(
                &path,
                hdrgm_jpeg(&base, &map, r#"hdrgm:Version="1.0">"#, metadata, None),
            )?;
            let (width, height, light) = light_of(&path)?;
            assert_eq!((width, height), (w, h));
            for (at, got) in light.iter().enumerate() {
                let (x, y) = (at as u32 % w, at as u32 / w);
                let px = sdr.get_pixel(x, y).0;
                for c in 0..3 {
                    let (min, max, gamma, offsets) = channels[c];
                    let want = adobe(
                        srgb_eotf(px[c]),
                        gains.get_pixel(x, y).0[0],
                        min,
                        max,
                        gamma,
                        offsets,
                    );
                    assert!(
                        (got[c] - want).abs() <= TOLERANCE * want.abs().max(1.),
                        "form {i} ({x}, {y}) channel {c}: {} against {want}",
                        got[c]
                    );
                }
            }
        }
        Ok(())
    }

    /// The map turns with the base's EXIF orientation: it is in the base's stored
    /// frame, as a JPEG's ISO 21496-1 map is. A map smaller than the base spreads over
    /// it, and a flat one gives every pixel its gain.
    #[test]
    fn an_hdrgm_map_turns_with_its_base_and_spreads_over_it() -> anyhow::Result<()> {
        let (w, h) = (16, 8);
        let base = encoded(&base_ramp(w, h), w, h, 3);
        // Left half of the stored frame gains nothing, the right half its maximum.
        let map_pixels: Vec<u8> = (0..w * h)
            .map(|i| if i % w < w / 2 { 0 } else { 255 })
            .collect();
        let map = encoded(&map_pixels, w, h, 1);
        let metadata = r#"hdrgm:Version="1.0" hdrgm:GainMapMax="2" hdrgm:OffsetSDR="0" hdrgm:OffsetHDR="0" hdrgm:HDRCapacityMax="2">"#;
        // Orientation 6: the stored frame turned 90° clockwise.
        let mut exif = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01".to_vec();
        exif.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0]);
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("turned.jpg");
        std::fs::write(
            &path,
            hdrgm_jpeg(
                &base,
                &map,
                r#"hdrgm:Version="1.0">"#,
                metadata,
                Some(&exif),
            ),
        )?;
        let turn = |mut image: image::DynamicImage| {
            image.apply_orientation(image::metadata::Orientation::Rotate90);
            image
        };
        let sdr = turn(decoded(&base)).into_rgb8();
        let gains = turn(decoded(&map)).into_luma8();
        let (width, height, light) = light_of(&path)?;
        assert_eq!((width, height), (h, w));
        for (at, got) in light.iter().enumerate() {
            let (x, y) = (at as u32 % width, at as u32 / width);
            let sample = gains.get_pixel(x, y).0[0];
            // The stored frame's right half is the turned image's bottom half.
            assert_eq!(sample > 128, y >= height / 2, "({x}, {y})");
            for c in 0..3 {
                let want = adobe(
                    srgb_eotf(sdr.get_pixel(x, y).0[c]),
                    sample,
                    0.,
                    2.,
                    1.,
                    (0., 0.),
                );
                assert!(
                    (got[c] - want).abs() <= TOLERANCE * want.max(1.),
                    "({x}, {y}): {got:?}"
                );
            }
        }
        // A flat map a quarter of the base's size.
        let small = encoded(&[128; 4 * 2], 4, 2, 1);
        let path = dir.path().join("small.jpg");
        std::fs::write(
            &path,
            hdrgm_jpeg(&base, &small, r#"hdrgm:Version="1.0">"#, metadata, None),
        )?;
        let sample = decoded(&small).into_luma8().get_pixel(0, 0).0[0];
        let sdr = decoded(&base).into_rgb8();
        let (_, _, light) = light_of(&path)?;
        for (at, got) in light.iter().enumerate() {
            let (x, y) = (at as u32 % w, at as u32 / w);
            for c in 0..3 {
                let want = adobe(
                    srgb_eotf(sdr.get_pixel(x, y).0[c]),
                    sample,
                    0.,
                    2.,
                    1.,
                    (0., 0.),
                );
                assert!(
                    (got[c] - want).abs() <= TOLERANCE * want.max(1.),
                    "({x}, {y}): {got:?}"
                );
            }
        }
        Ok(())
    }

    /// What isn't hdrgm 1.0, or isn't whole, is refused by reason; a JPEG whose XMP
    /// names no hdrgm version is an SDR file.
    #[test]
    fn hdrgm_maps_that_cannot_be_applied_are_refused_by_reason() -> anyhow::Result<()> {
        let (w, h) = (8, 8);
        let base = encoded(&base_ramp(w, h), w, h, 3);
        let map = encoded(&[200; 64], w, h, 1);
        let ok = r#"hdrgm:Version="1.0" hdrgm:GainMapMax="2" hdrgm:HDRCapacityMax="2">"#;
        let primary = r#"hdrgm:Version="1.0">"#;
        let cases = [
            (
                primary,
                r#"hdrgm:Version="1.0" hdrgm:HDRCapacityMax="2">"#,
                "damaged gain-map",
            ),
            (
                primary,
                r#"hdrgm:Version="1.0" hdrgm:GainMapMax="2">"#,
                "damaged gain-map",
            ),
            (
                primary,
                r#"hdrgm:Version="1.0" hdrgm:GainMapMax="two" hdrgm:HDRCapacityMax="2">"#,
                "damaged gain-map",
            ),
            (
                primary,
                r#"hdrgm:Version="1.0" hdrgm:GainMapMin="3" hdrgm:GainMapMax="2" hdrgm:HDRCapacityMax="2">"#,
                "damaged gain-map",
            ),
            (
                primary,
                r#"hdrgm:Version="1.0" hdrgm:Gamma="0" hdrgm:GainMapMax="2" hdrgm:HDRCapacityMax="2">"#,
                "damaged gain-map",
            ),
            (
                primary,
                r#"hdrgm:Version="1.0" hdrgm:OffsetSDR="-0.1" hdrgm:GainMapMax="2" hdrgm:HDRCapacityMax="2">"#,
                "damaged gain-map",
            ),
            (
                primary,
                r#"hdrgm:Version="1.0" hdrgm:GainMapMax="2" hdrgm:HDRCapacityMin="2" hdrgm:HDRCapacityMax="2">"#,
                "damaged gain-map",
            ),
            (
                primary,
                r#"hdrgm:Version="1.0" hdrgm:HDRCapacityMax="2"><hdrgm:GainMapMax><rdf:Seq><rdf:li>1</rdf:li><rdf:li>2</rdf:li></rdf:Seq></hdrgm:GainMapMax>"#,
                "damaged gain-map",
            ),
            (
                primary,
                r#"hdrgm:Version="1.0" hdrgm:GainMapMax="20" hdrgm:HDRCapacityMax="2">"#,
                "damaged gain-map",
            ),
            (
                primary,
                r#"hdrgm:Version="1.0" hdrgm:BaseRenditionIsHDR="True" hdrgm:GainMapMax="2" hdrgm:HDRCapacityMax="2">"#,
                "HDR-base gain-map",
            ),
            (
                primary,
                r#"hdrgm:Version="2.0" hdrgm:GainMapMax="2" hdrgm:HDRCapacityMax="2">"#,
                "version 2.0 gain-map",
            ),
            (r#"hdrgm:Version="2.0">"#, ok, "version 2.0 gain-map"),
            // A map whose hdrgm XMP has no version.
            (primary, r#"hdrgm:Other="1">"#, "damaged gain-map"),
        ];
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("refused.jpg");
        for (primary, metadata, why) in cases {
            std::fs::write(&path, hdrgm_jpeg(&base, &map, primary, metadata, None))?;
            assert_eq!(super::super::probe(&path).unwrap_err().0, why, "{metadata}");
        }
        // No MPF index.
        std::fs::write(&path, [&base[..2], &xmp(primary), &base[2..]].concat())?;
        assert_eq!(
            super::super::probe(&path).unwrap_err().0,
            "damaged gain-map"
        );
        // XMP without an hdrgm version: an SDR JPEG, as before.
        std::fs::write(
            &path,
            [
                &base[..2],
                &xmp(r#"xmlns:dc="http://purl.org/dc/elements/1.1/">"#),
                &base[2..],
            ]
            .concat(),
        )?;
        super::super::probe(&path).unwrap();
        assert!(
            super::super::open(&path, rawmakase_cancel::Cancel::NEVER)?
                .hdr()
                .is_none()
        );
        Ok(())
    }

    /// Only the image's own XMP properties count: an `hdrgm:Version` of a resource
    /// nested in its description doesn't make an SDR JPEG a gain-map one, nor does a
    /// nested map field change a map. An array of channels must be ordered: a Bag or
    /// an Alt of three is refused.
    #[test]
    fn hdrgm_properties_are_the_images_own_and_arrays_are_ordered() -> anyhow::Result<()> {
        let (w, h) = (8, 8);
        let base = encoded(&base_ramp(w, h), w, h, 3);
        let map = encoded(&[255; 64], w, h, 1);
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("nested.jpg");
        let nested = r#"xmlns:test="urn:test:"><test:source rdf:parseType="Resource"><hdrgm:Version>1.0</hdrgm:Version></test:source>"#;
        std::fs::write(&path, [&base[..2], &xmp(nested), &base[2..]].concat())?;
        super::super::probe(&path).unwrap();
        assert!(
            super::super::open(&path, rawmakase_cancel::Cancel::NEVER)?
                .hdr()
                .is_none()
        );
        // A nested GainMapMax of 4 stops is not the map's 2.
        let metadata = r#"xmlns:test="urn:test:" hdrgm:Version="1.0" hdrgm:GainMapMax="2" hdrgm:OffsetSDR="0" hdrgm:OffsetHDR="0" hdrgm:HDRCapacityMax="2"><test:other rdf:parseType="Resource"><hdrgm:GainMapMax>4</hdrgm:GainMapMax></test:other>"#;
        std::fs::write(
            &path,
            hdrgm_jpeg(&base, &map, r#"hdrgm:Version="1.0">"#, metadata, None),
        )?;
        let sdr = decoded(&base).into_rgb8();
        let (_, _, light) = light_of(&path)?;
        let want = srgb_eotf(sdr.get_pixel(3, 3).0[0]) * 4.;
        assert!((light[3 * 8 + 3][0] - want).abs() <= TOLERANCE * want.max(1.));
        // A literal's text is all of it, a comment aside.
        let split = r#"hdrgm:Version="1.0" hdrgm:OffsetSDR="0" hdrgm:OffsetHDR="0" hdrgm:HDRCapacityMax="2"><hdrgm:GainMapMax>1<!-- -->.5</hdrgm:GainMapMax>"#;
        std::fs::write(
            &path,
            hdrgm_jpeg(&base, &map, r#"hdrgm:Version="1.0">"#, split, None),
        )?;
        let (_, _, light) = light_of(&path)?;
        let want = srgb_eotf(sdr.get_pixel(3, 3).0[0]) * 1.5f64.exp2();
        assert!((light[3 * 8 + 3][0] - want).abs() <= TOLERANCE * want.max(1.));
        // Items that aren't literals, and text beside an array.
        for items in [
            "<rdf:Seq><rdf:li>2<rdf:Description/></rdf:li></rdf:Seq>",
            r#"<rdf:Seq><rdf:li rdf:resource="urn:x">2</rdf:li></rdf:Seq>"#,
            "3<rdf:Seq><rdf:li>2</rdf:li></rdf:Seq>",
        ] {
            let metadata = format!(
                r#"hdrgm:Version="1.0" hdrgm:HDRCapacityMax="2"><hdrgm:GainMapMax>{items}</hdrgm:GainMapMax>"#
            );
            std::fs::write(
                &path,
                hdrgm_jpeg(&base, &map, r#"hdrgm:Version="1.0">"#, &metadata, None),
            )?;
            assert_eq!(
                super::super::probe(&path).unwrap_err().0,
                "damaged gain-map",
                "{items}"
            );
        }
        for container in ["Bag", "Alt"] {
            let metadata = format!(
                r#"hdrgm:Version="1.0" hdrgm:HDRCapacityMax="2"><hdrgm:GainMapMax><rdf:{container}><rdf:li>1</rdf:li><rdf:li>2</rdf:li><rdf:li>3</rdf:li></rdf:{container}></hdrgm:GainMapMax>"#
            );
            std::fs::write(
                &path,
                hdrgm_jpeg(&base, &map, r#"hdrgm:Version="1.0">"#, &metadata, None),
            )?;
            assert_eq!(
                super::super::probe(&path).unwrap_err().0,
                "damaged gain-map",
                "{container}"
            );
        }
        Ok(())
    }

    /// A map whose hdrgm fields are partly in its extended XMP (split across APP1
    /// chunks, in any order) reads them, as one packet: its gamma applies. Chunks
    /// that don't fill the packet are damaged.
    #[test]
    fn an_hdrgm_maps_extended_xmp_is_read() -> anyhow::Result<()> {
        let (w, h) = (8, 8);
        let base = encoded(&base_ramp(w, h), w, h, 3);
        let map = encoded(&[64; 64], w, h, 1);
        let guid = b"0123456789ABCDEF0123456789ABCDEF";
        let standard = xmp(&format!(
            r#"xmlns:xmpNote="http://ns.adobe.com/xmp/note/" xmpNote:HasExtendedXMP="{}" hdrgm:Version="1.0" hdrgm:GainMapMax="2" hdrgm:OffsetSDR="0" hdrgm:OffsetHDR="0" hdrgm:HDRCapacityMax="2">"#,
            std::str::from_utf8(guid)?
        ));
        let packet = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/" hdrgm:Gamma="2"/></rdf:RDF></x:xmpmeta>"#;
        let chunk = |from: usize, to: usize, len: usize| {
            let payload = [
                XMP_EXTENSION,
                guid,
                &(len as u32).to_be_bytes(),
                &(from as u32).to_be_bytes(),
                &packet[from..to],
            ]
            .concat();
            segment(0xe1, &payload)
        };
        let half = packet.len() / 2;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("extended.jpg");
        // The second half first.
        let segments = [
            standard.clone(),
            chunk(half, packet.len(), packet.len()),
            chunk(0, half, packet.len()),
        ]
        .concat();
        std::fs::write(
            &path,
            hdrgm_jpeg_with(&base, &map, r#"hdrgm:Version="1.0">"#, &segments, None),
        )?;
        let sample = decoded(&map).into_luma8().get_pixel(0, 0).0[0];
        let sdr = decoded(&base).into_rgb8();
        let (_, _, light) = light_of(&path)?;
        let want = adobe(
            srgb_eotf(sdr.get_pixel(3, 3).0[0]),
            sample,
            0.,
            2.,
            2.,
            (0., 0.),
        );
        assert!(
            (light[3 * 8 + 3][0] - want).abs() <= TOLERANCE * want.max(1.),
            "{} against {want}",
            light[3 * 8 + 3][0]
        );
        // A chunk missing, or every chunk: the standard packet names one.
        for segments in [
            [standard.clone(), chunk(0, half, packet.len())].concat(),
            standard,
        ] {
            std::fs::write(
                &path,
                hdrgm_jpeg_with(&base, &map, r#"hdrgm:Version="1.0">"#, &segments, None),
            )?;
            assert_eq!(
                super::super::probe(&path).unwrap_err().0,
                "damaged gain-map"
            );
        }
        Ok(())
    }
}
