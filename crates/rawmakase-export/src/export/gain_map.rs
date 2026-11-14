//! Gain-map exports (docs/hdr.md#gain-maps): an SDR base image, the edit's SDR
//! rendition, with a gain map that turns it into the HDR master on a display with
//! headroom, as ISO 21496-1 describes. The JPEG form is the one libultrahdr, Android,
//! Apple and Adobe read: the base JPEG with an ISO 21496-1 version marker and an MPF
//! index, followed by the gain map's own JPEG carrying the metadata.
//!
//! The same metadata is also written as Adobe's gain-map XMP (`hdrgm` 1.0, which
//! Android's Ultra HDR 1.0 adopted), for readers that know only that form: the base's
//! XMP gets `hdrgm:Version` and a GContainer directory of the two images, and the
//! map's own XMP the values. Both forms carry the same rationals, so a reader that
//! knows both gets the same light from either; ISO 21496-1 is the one that wins.
//!
//! The map is one channel, computed in the base's colour space (sRGB primaries, linear)
//! from luminance: log2 of (HDR + offset) / (SDR + offset), with both offsets 1/64,
//! scaled between its minimum and maximum over the image (gamma 1). The base's headroom
//! is 0 and the alternate's log2 of the master's peak, so a display with that headroom
//! shows the master and one with none the base.
use crate::xml::{self, ns::RDF};
use anyhow::{Context, Result, ensure};

/// Both offsets, in linear units where 1 is graphics white.
const OFFSET: f32 = 1. / 64.;
/// Rational fields' denominator: 2^16 holds the offsets exactly and log2 values to
/// 1/65536 of a stop.
const DENOMINATOR: u32 = 1 << 16;
/// Adobe's gain-map XMP namespace (`hdrgm`).
pub(super) const HDRGM: &str = "http://ns.adobe.com/hdr-gain-map/1.0/";
/// GContainer's namespaces, of the directory Ultra HDR lists a file's images in.
pub(super) const CONTAINER: &str = "http://ns.google.com/photos/1.0/container/";
const ITEM: &str = "http://ns.google.com/photos/1.0/container/item/";
/// Rec. 709 / sRGB luminance weights.
const LUMINANCE: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// ISO 21496-1 metadata of a single-channel map in the base's colour space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metadata {
    /// log2 of the smallest and largest gain the map encodes.
    pub min: f32,
    pub max: f32,
    /// log2 of the base's and the alternate's HDR headroom.
    pub base_headroom: f32,
    pub alternate_headroom: f32,
}

impl Metadata {
    /// The metadata as ISO 21496-1 serialises it: versions, flags (base colour space,
    /// one channel, a denominator per value), then the headrooms and the channel's
    /// minimum, maximum, gamma and offsets, each a numerator and a denominator.
    pub fn iso_21496(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(61);
        out.extend(0u16.to_be_bytes()); // minimum version
        out.extend(0u16.to_be_bytes()); // writer version
        out.push(0x40); // use the base colour space
        let unsigned = |out: &mut Vec<u8>, v: f32| {
            out.extend((numerator(v.max(0.)) as u32).to_be_bytes());
            out.extend(DENOMINATOR.to_be_bytes());
        };
        let signed = |out: &mut Vec<u8>, v: f32| {
            out.extend(numerator(v).to_be_bytes());
            out.extend(DENOMINATOR.to_be_bytes());
        };
        unsigned(&mut out, self.base_headroom);
        unsigned(&mut out, self.alternate_headroom);
        signed(&mut out, self.min);
        signed(&mut out, self.max);
        unsigned(&mut out, 1.); // gamma
        signed(&mut out, OFFSET); // base offset
        signed(&mut out, OFFSET); // alternate offset
        out
    }

    /// The map's own XMP packet: the same values as [`Metadata::iso_21496`] as Adobe's
    /// `hdrgm` 1.0 names them, each the same rational written as a decimal. hdrgm's
    /// capacities are ISO 21496-1's headrooms of an SDR base.
    pub fn hdrgm(&self) -> String {
        let value = |v: f32| f64::from(numerator(v)) / f64::from(DENOMINATOR);
        xml::packet(&format!(
            "  <rdf:Description rdf:about=\"\"\n    xmlns:hdrgm=\"{HDRGM}\"\n   \
             hdrgm:Version=\"1.0\"\n   hdrgm:GainMapMin=\"{}\"\n   hdrgm:GainMapMax=\"{}\"\n   \
             hdrgm:Gamma=\"1\"\n   hdrgm:OffsetSDR=\"{}\"\n   hdrgm:OffsetHDR=\"{}\"\n   \
             hdrgm:HDRCapacityMin=\"{}\"\n   hdrgm:HDRCapacityMax=\"{}\"\n   \
             hdrgm:BaseRenditionIsHDR=\"False\"/>\n",
            value(self.min),
            value(self.max),
            value(OFFSET),
            value(OFFSET),
            value(self.base_headroom.max(0.)),
            value(self.alternate_headroom.max(0.)),
        ))
    }
}

/// `v` in 1/[`DENOMINATOR`] units, as both metadata forms write it.
fn numerator(v: f32) -> i32 {
    (v * DENOMINATOR as f32).round() as i32
}

/// The gain map of `hdr` over `sdr`, both linear sRGB-primaries RGB of the same size
/// (1 = graphics white), as 8-bit samples, with its metadata; `peak` is the master's
/// brightest value.
pub fn compute(sdr: &[[f32; 3]], hdr: &[[f32; 3]], peak: f32) -> Result<(Vec<u8>, Metadata)> {
    ensure!(
        sdr.len() == hdr.len() && !sdr.is_empty(),
        "Gain map renditions differ in size"
    );
    let lum = |p: &[f32; 3]| (0..3).map(|c| LUMINANCE[c] * p[c].max(0.)).sum::<f32>();
    let gains: Vec<f32> = sdr
        .iter()
        .zip(hdr)
        .map(|(s, h)| ((lum(h) + OFFSET) / (lum(s) + OFFSET)).log2())
        .collect();
    let min = gains.iter().copied().fold(f32::INFINITY, f32::min).min(0.);
    let mut max = gains.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if max - min < 1. / 256. {
        max = min + 1. / 256.;
    }
    let map = gains
        .iter()
        .map(|g| (((g - min) / (max - min)).clamp(0., 1.) * 255. + 0.5) as u8)
        .collect();
    Ok((
        map,
        Metadata {
            min,
            max,
            base_headroom: 0.,
            alternate_headroom: peak.max(1.).log2(),
        },
    ))
}

/// The APP2 payload naming ISO 21496-1, before its metadata.
const ISO_URN: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0";

/// The gain map's JPEG: `gain_map` (a complete grayscale JPEG) with its `hdrgm` XMP
/// and then its ISO 21496-1 metadata, as libultrahdr writes them.
pub fn map_jpeg(gain_map: &[u8], metadata: &Metadata) -> Result<Vec<u8>> {
    ensure!(gain_map.starts_with(&[0xff, 0xd8]), "Not a JPEG");
    let xmp = [xml::ns::JPEG_HEADER, metadata.hdrgm().as_bytes()].concat();
    let (gain_map, _) = insert(gain_map, 0xe1, &[xmp])?;
    let (gain_map, _) = insert(
        &gain_map,
        0xe2,
        &[[ISO_URN, &metadata.iso_21496()].concat()],
    )?;
    Ok(gain_map)
}

/// The base's XMP packet: `edit` (the packet the export embeds, if any) with
/// `hdrgm:Version` and a GContainer directory of the base and its gain map, whose
/// JPEG ([`map_jpeg`]) is `map_len` bytes, as Ultra HDR 1.0 lists them.
pub fn primary_xmp(edit: Option<&str>, map_len: usize) -> Result<String> {
    let namespaces = format!(
        "\n    xmlns:hdrgm=\"{HDRGM}\"\n    xmlns:Container=\"{CONTAINER}\"\n    \
         xmlns:Item=\"{ITEM}\"\n   hdrgm:Version=\"1.0\""
    );
    let directory = format!(
        "   <Container:Directory>\n    <rdf:Seq>\n     <rdf:li rdf:parseType=\"Resource\">\n      \
         <Container:Item Item:Semantic=\"Primary\" Item:Mime=\"image/jpeg\"/>\n     </rdf:li>\n     \
         <rdf:li rdf:parseType=\"Resource\">\n      <Container:Item Item:Semantic=\"GainMap\" \
         Item:Mime=\"image/jpeg\" Item:Length=\"{map_len}\"/>\n     </rdf:li>\n    </rdf:Seq>\n   \
         </Container:Directory>\n"
    );
    let Some(edit) = edit else {
        return Ok(xml::packet(&format!(
            "  <rdf:Description rdf:about=\"\"{namespaces}>\n{directory}  </rdf:Description>\n"
        )));
    };
    // The edit's packet is `xmp::write`'s: one description, which takes the new
    // properties, under an `rdf` prefix.
    let doc = roxmltree::Document::parse(edit).context("XMP packet is not valid XML")?;
    let mut descriptions = doc
        .descendants()
        .filter(|n| n.has_tag_name((RDF, "RDF")))
        .flat_map(|n| n.children())
        .filter(|n| n.has_tag_name((RDF, "Description")));
    let description = descriptions
        .next()
        .context("XMP packet has no description")?;
    ensure!(
        descriptions.next().is_none(),
        "XMP packet has more than one description"
    );
    ensure!(
        description.lookup_prefix(RDF) == Some("rdf")
            && ["hdrgm", "Container", "Item"]
                .iter()
                .all(|p| description.lookup_namespace_uri(Some(p)).is_none()),
        "XMP packet's prefixes clash with the gain map's"
    );
    let range = description.range();
    let element = &edit[range.clone()];
    let name_end = range.start
        + element
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .context("XMP description is not an element")?;
    let mut out = String::with_capacity(edit.len() + namespaces.len() + directory.len() + 32);
    out.push_str(&edit[..name_end]);
    out.push_str(&namespaces);
    if element.ends_with("/>") {
        out.push_str(&edit[name_end..range.end - 2]);
        out.push_str(">\n");
        out.push_str(&directory);
        out.push_str("  </rdf:Description>");
    } else {
        let close = range.start
            + element
                .rfind("</")
                .context("XMP description is not closed")?;
        out.push_str(&edit[name_end..close]);
        out.push_str(&directory);
        out.push_str("  ");
        out.push_str(&edit[close..range.end]);
    }
    out.push_str(&edit[range.end..]);
    Ok(out)
}

/// The gain-map JPEG: `primary` (the base, a complete JPEG) with an ISO 21496-1
/// version marker and an MPF index, followed by `map` ([`map_jpeg`]), as libultrahdr
/// writes them.
pub fn assemble(primary: &[u8], map: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        primary.starts_with(&[0xff, 0xd8]) && map.starts_with(&[0xff, 0xd8]),
        "Not a JPEG"
    );
    // The primary's version marker: the versions alone.
    let marker = [ISO_URN, &[0, 0, 0, 0]].concat();
    let placeholder = mpf(0, 0, 0);
    let (mut out, at) = insert(primary, 0xe2, &[marker, placeholder])?;
    // The MPF index, now that the sizes and the map's offset are known, where it was
    // inserted (the same bytes may be in the primary's own metadata). Offsets count
    // from its TIFF header, four bytes into its payload ("MPF\0").
    let at = at[1];
    let tiff = at + 4;
    let primary_size = out.len();
    let index = mpf(
        u32::try_from(primary_size)?,
        u32::try_from(map.len())?,
        u32::try_from(primary_size - tiff)?,
    );
    out[at..at + index.len()].copy_from_slice(&index);
    out.extend_from_slice(map);
    Ok(out)
}

/// [`assemble`] with ISO 21496-1 metadata alone, already serialised: `iso` is the
/// map's payload after its URN, written as it is.
#[cfg(test)]
pub(super) fn assemble_iso(primary: &[u8], gain_map: &[u8], iso: &[u8]) -> Result<Vec<u8>> {
    ensure!(gain_map.starts_with(&[0xff, 0xd8]), "Not a JPEG");
    let (gain_map, _) = insert(gain_map, 0xe2, &[[ISO_URN, iso].concat()])?;
    assemble(primary, &gain_map)
}

/// An MPF APP2 payload for a primary image of `primary` bytes and a second image of
/// `second` bytes at `offset` from the TIFF header (big endian, as libultrahdr's).
fn mpf(primary: u32, second: u32, offset: u32) -> Vec<u8> {
    let mut p = b"MPF\0".to_vec();
    p.extend(b"MM\0*");
    p.extend(8u32.to_be_bytes()); // first IFD
    p.extend(3u16.to_be_bytes());
    // MPFVersion "0100".
    p.extend([0xb0, 0x00, 0, 7]);
    p.extend(4u32.to_be_bytes());
    p.extend(b"0100");
    // NumberOfImages 2.
    p.extend([0xb0, 0x01, 0, 4]);
    p.extend(1u32.to_be_bytes());
    p.extend(2u32.to_be_bytes());
    // MPEntry: two 16-byte entries, after the IFD.
    p.extend([0xb0, 0x02, 0, 7]);
    p.extend(32u32.to_be_bytes());
    p.extend(50u32.to_be_bytes());
    p.extend(0u32.to_be_bytes()); // no next IFD
    // The primary: baseline MP primary image, at offset 0.
    p.extend(0x0003_0000u32.to_be_bytes());
    p.extend(primary.to_be_bytes());
    p.extend(0u32.to_be_bytes());
    p.extend([0, 0, 0, 0]);
    // The gain map.
    p.extend(0u32.to_be_bytes());
    p.extend(second.to_be_bytes());
    p.extend(offset.to_be_bytes());
    p.extend([0, 0, 0, 0]);
    p
}

/// `jpeg` with `marker` (APP1 or APP2) segments of `payloads` after the APPn segments
/// it has, and where each payload begins in it.
fn insert(jpeg: &[u8], marker: u8, payloads: &[Vec<u8>]) -> Result<(Vec<u8>, Vec<usize>)> {
    let mut at = 2;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xff && (0xe0..=0xef).contains(&jpeg[at + 1]) {
        at += 2 + u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
    }
    ensure!(at <= jpeg.len(), "A JPEG segment runs past its end");
    let mut out =
        Vec::with_capacity(jpeg.len() + payloads.iter().map(|p| p.len() + 4).sum::<usize>());
    out.extend_from_slice(&jpeg[..at]);
    let mut starts = Vec::with_capacity(payloads.len());
    for p in payloads {
        ensure!(p.len() + 2 <= 65_535, "A JPEG segment is too large");
        out.extend([0xff, marker]);
        out.extend(((p.len() + 2) as u16).to_be_bytes());
        starts.push(out.len());
        out.extend_from_slice(p);
    }
    out.extend_from_slice(&jpeg[at..]);
    Ok((out, starts))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serialisation libultrahdr 2.0.2 writes for its own linear test map, field
    /// for field (its denominators differ; the values are the same).
    #[test]
    fn the_metadata_serialises_as_iso_21496_says() {
        let m = Metadata {
            min: 0.,
            max: 6.712_5,
            base_headroom: 0.,
            alternate_headroom: 5.622,
        };
        let bytes = m.iso_21496();
        assert_eq!(bytes.len(), 61);
        assert_eq!(&bytes[..5], &[0, 0, 0, 0, 0x40]);
        let rational = |at: usize| {
            let n = i32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
            let d = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap());
            n as f32 / d as f32
        };
        assert_eq!(rational(5), 0.);
        assert!((rational(13) - 5.622).abs() < 1e-4);
        assert!((rational(29) - 6.7125).abs() < 1e-4);
        assert_eq!(rational(37), 1.);
        assert_eq!(rational(45), OFFSET);
    }

    /// The MPF index is patched where it was inserted, even when the primary's own
    /// metadata holds the same bytes (a binary EXIF comment, say).
    #[test]
    fn the_mpf_index_is_patched_where_it_was_inserted() -> Result<()> {
        let decoy = mpf(0, 0, 0);
        let mut app1 = vec![0xff, 0xe1];
        app1.extend(((decoy.len() + 2) as u16).to_be_bytes());
        app1.extend_from_slice(&decoy);
        let primary = [&[0xff, 0xd8][..], &app1, &[0xff, 0xd9]].concat();
        let map = vec![0xff, 0xd8, 0xff, 0xd9];
        let m = Metadata {
            min: 0.,
            max: 1.,
            base_headroom: 0.,
            alternate_headroom: 1.,
        };
        let out = assemble(&primary, &map_jpeg(&map, &m)?)?;
        assert_eq!(
            &out[6..6 + decoy.len()],
            &decoy[..],
            "the EXIF payload is untouched"
        );
        let at = out[app1.len() + 2..]
            .windows(4)
            .position(|w| w == b"MPF\0")
            .unwrap()
            + app1.len()
            + 2;
        let tiff = at + 4;
        let field = |o: usize| u32::from_be_bytes(out[o..o + 4].try_into().unwrap()) as usize;
        let (size, offset) = (field(tiff + 50 + 20), field(tiff + 50 + 24));
        assert_eq!(field(tiff + 50 + 4), out.len() - size);
        assert_eq!(&out[tiff + offset..tiff + offset + 2], &[0xff, 0xd8]);
        assert_eq!(tiff + offset + size, out.len());
        Ok(())
    }

    /// The `hdrgm` values are the ISO 21496-1 rationals, as decimals that parse back
    /// to the same numbers.
    #[test]
    fn the_hdrgm_xmp_carries_the_iso_21496_values() -> Result<()> {
        let m = Metadata {
            min: -0.123_45,
            max: 6.712_5,
            base_headroom: 0.,
            alternate_headroom: 5.622,
        };
        let iso = m.iso_21496();
        let rational = |at: usize| {
            let n = i32::from_be_bytes(iso[at..at + 4].try_into().unwrap());
            let d = u32::from_be_bytes(iso[at + 4..at + 8].try_into().unwrap());
            f64::from(n) / f64::from(d)
        };
        let packet = m.hdrgm();
        let doc = roxmltree::Document::parse(&packet)?;
        let d = doc
            .descendants()
            .find(|n| n.has_tag_name((RDF, "Description")))
            .unwrap();
        let get = |name: &str| d.attribute((HDRGM, name)).unwrap();
        let number = |name: &str| get(name).parse::<f64>().unwrap();
        assert_eq!(get("Version"), "1.0");
        assert_eq!(get("BaseRenditionIsHDR"), "False");
        assert_eq!(number("HDRCapacityMin"), rational(5));
        assert_eq!(number("HDRCapacityMax"), rational(13));
        assert_eq!(number("GainMapMin"), rational(21));
        assert_eq!(number("GainMapMax"), rational(29));
        assert_eq!(number("Gamma"), rational(37));
        assert_eq!(number("OffsetSDR"), rational(45));
        assert_eq!(number("OffsetHDR"), rational(53));
        // The map's JPEG: XMP, then the ISO 21496-1 metadata, after its own APPn.
        let map = map_jpeg(&[0xff, 0xd8, 0xff, 0xd9], &m)?;
        let xmp = [xml::ns::JPEG_HEADER, packet.as_bytes()].concat();
        let mut expected = vec![0xff, 0xd8, 0xff, 0xe1];
        expected.extend(((xmp.len() + 2) as u16).to_be_bytes());
        expected.extend(&xmp);
        expected.extend([0xff, 0xe2]);
        expected.extend(((ISO_URN.len() + iso.len() + 2) as u16).to_be_bytes());
        expected.extend(ISO_URN);
        expected.extend(&iso);
        expected.extend([0xff, 0xd9]);
        assert_eq!(map, expected);
        Ok(())
    }

    /// The base's XMP is the edit's packet with `hdrgm:Version` and the directory
    /// added to its one top-level description; its own properties and nested
    /// descriptions stay as they were. Without an edit's packet it is a packet of
    /// its own.
    #[test]
    fn the_base_xmp_adds_the_directory_to_the_edits_packet() -> Result<()> {
        const ITEMS: (&str, &str) = (CONTAINER, "Item");
        let check = |packet: &str| -> Result<()> {
            let doc = roxmltree::Document::parse(packet)?;
            let rdf = doc
                .descendants()
                .find(|n| n.has_tag_name((RDF, "RDF")))
                .unwrap();
            let d = rdf.children().find(|n| n.is_element()).unwrap();
            assert_eq!(d.attribute((HDRGM, "Version")), Some("1.0"));
            let items: Vec<_> = d
                .descendants()
                .filter(|n| n.has_tag_name(ITEMS))
                .map(|n| {
                    (
                        n.attribute((ITEM, "Semantic")).unwrap(),
                        n.attribute((ITEM, "Mime")).unwrap(),
                        n.attribute((ITEM, "Length")),
                    )
                })
                .collect();
            assert_eq!(
                items,
                [
                    ("Primary", "image/jpeg", None),
                    ("GainMap", "image/jpeg", Some("1234"))
                ]
            );
            Ok(())
        };
        check(&primary_xmp(None, 1234)?)?;
        let edit = xml::packet(
            "  <rdf:Description rdf:about=\"\"\n    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    \
             xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\n   crs:Exposure2012=\"+1.00\">\n   \
             <crs:Look>\n    <rdf:Description\n     crs:Name=\"Look\"/>\n   </crs:Look>\n   \
             <dc:title>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">Title</rdf:li>\n    \
             </rdf:Alt>\n   </dc:title>\n  </rdf:Description>\n",
        );
        let merged = primary_xmp(Some(&edit), 1234)?;
        check(&merged)?;
        let doc = roxmltree::Document::parse(&merged)?;
        let d = doc
            .descendants()
            .find(|n| n.has_tag_name((RDF, "Description")))
            .unwrap();
        assert_eq!(
            d.attribute((
                "http://ns.adobe.com/camera-raw-settings/1.0/",
                "Exposure2012"
            )),
            Some("+1.00")
        );
        assert!(merged.contains("<crs:Look>\n    <rdf:Description\n     crs:Name=\"Look\"/>"));
        assert!(merged.contains("<rdf:li xml:lang=\"x-default\">Title</rdf:li>"));
        // A self-closed description takes the directory too.
        let empty = xml::packet("  <rdf:Description rdf:about=\"\"/>\n");
        check(&primary_xmp(Some(&empty), 1234)?)?;
        Ok(())
    }

    /// When the base's XMP is too large for one segment, the gain map's properties
    /// stay in the standard packet, which Ultra HDR readers read.
    #[test]
    fn the_gain_maps_xmp_stays_in_the_standard_packet() -> Result<()> {
        let long = "x".repeat(70_000);
        let edit = xml::packet(&format!(
            "  <rdf:Description rdf:about=\"\"\n    xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n   \
             <dc:description>{long}</dc:description>\n  </rdf:Description>\n"
        ));
        let split = super::super::extended_xmp::split(&primary_xmp(Some(&edit), 1234)?)?;
        assert!(split.extended.is_some());
        assert!(split.standard.contains("hdrgm:Version=\"1.0\""));
        assert!(split.standard.contains("Item:Length=\"1234\""));
        assert!(!split.standard.contains(&long));
        Ok(())
    }

    #[test]
    fn a_map_spans_its_gains_and_is_flat_without_highlights() {
        let sdr = vec![[0.5; 3], [1.; 3], [1.; 3]];
        let hdr = vec![[0.5; 3], [4.; 3], [1.; 3]];
        let (map, m) = compute(&sdr, &hdr, 4.).unwrap();
        assert_eq!(map, [0, 255, 0]);
        assert_eq!(m.min, 0.);
        assert!((m.max - ((4. + OFFSET) / (1. + OFFSET)).log2()).abs() < 1e-5);
        assert_eq!(m.alternate_headroom, 2.);
        let (flat, _) = compute(&sdr, &sdr, 1.).unwrap();
        assert!(flat.iter().all(|v| *v == 0));
    }
}
