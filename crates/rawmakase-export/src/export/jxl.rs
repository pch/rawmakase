//! JPEG XL exports, lossless (docs/hdr.md#hdr-export). `zune-jpegxl` encodes the frame;
//! RAWmakase writes the image header in its place, because zune's always says sRGB:
//! ours names the colour encoding (sRGB, or an HDR export's primaries and PQ or HLG)
//! and, for HDR, the tone mapping's intensity target. The frame begins at a byte
//! boundary after the header (ISO/IEC 18181-1, the header ends with ZeroPadToByte), so
//! zune's header is replaced whole: its length is found by writing the same bits zune
//! writes and checked against zune's own bytes before anything is spliced.
//!
//! The codestream goes in a container (ISO/IEC 18181-2) with `Exif` and `xml ` boxes
//! when the export carries metadata.
use anyhow::{Result, bail, ensure};

/// The colour a JPEG XL export's header names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Colour {
    /// sRGB: the header's `all_default` encoding.
    Srgb,
    /// An HDR export: enumerated primaries (JPEG XL's code, D65) and transfer, with the
    /// display luminance its maximum value stands for (`intensity_target`, cd/m²).
    Hdr {
        primaries: u8,
        transfer: u8,
        intensity_target: f32,
    },
}

/// A lossless JPEG XL codestream of interleaved RGB `samples` (8- or 16-bit) with
/// `colour` in its header.
pub(super) fn codestream(
    width: u32,
    height: u32,
    samples: Samples<'_>,
    colour: Colour,
) -> Result<Vec<u8>> {
    use zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions};
    let (bytes, depth, bits) = match samples {
        Samples::Eight(v) => (std::borrow::Cow::Borrowed(v), BitDepth::Eight, 8),
        Samples::Sixteen(v) => (
            std::borrow::Cow::Owned(v.iter().flat_map(|s| s.to_ne_bytes()).collect()),
            BitDepth::Sixteen,
            16,
        ),
    };
    ensure!(
        bytes.len() == width as usize * height as usize * 3 * (bits / 8),
        "JPEG XL export has the wrong size"
    );
    let options = EncoderOptions::new(width as usize, height as usize, ColorSpace::RGB, depth);
    let mut encoded = Vec::new();
    zune_jpegxl::JxlSimpleEncoder::new(&bytes, options)
        .encode(&mut encoded)
        .map_err(|e| anyhow::anyhow!("JPEG XL encoding failed: {e:?}"))?;
    let theirs = header(width, height, bits as u32, Colour::Srgb, false);
    ensure!(
        encoded.starts_with(&theirs),
        "The JPEG XL encoder's header is not the one RAWmakase replaces"
    );
    let mut out = header(width, height, bits as u32, colour, true);
    out.extend_from_slice(&encoded[theirs.len()..]);
    Ok(out)
}

/// A lossless 8-bit grayscale codestream of `samples`, as zune writes it (its header
/// names gray with the sRGB transfer): a gain map's.
pub(super) fn gray_codestream(width: u32, height: u32, samples: &[u8]) -> Result<Vec<u8>> {
    use zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions};
    ensure!(
        samples.len() == width as usize * height as usize,
        "JPEG XL gain map has the wrong size"
    );
    let options = EncoderOptions::new(
        width as usize,
        height as usize,
        ColorSpace::Luma,
        BitDepth::Eight,
    );
    let mut encoded = Vec::new();
    zune_jpegxl::JxlSimpleEncoder::new(samples, options)
        .encode(&mut encoded)
        .map_err(|e| anyhow::anyhow!("JPEG XL encoding failed: {e:?}"))?;
    Ok(encoded)
}

/// A `jhgm` box's payload (libjxl's gain-map bundle): version 0, the ISO 21496-1
/// `metadata` (big-endian u16 length), no alternate colour encoding (a zero length:
/// the map applies in the base's colour space), no alternate ICC profile (a zero u32
/// length), then the map's naked `codestream`.
pub(super) fn gain_map_bundle(metadata: &[u8], codestream: &[u8]) -> Result<Vec<u8>> {
    let len = u16::try_from(metadata.len())?;
    let mut out = vec![0];
    out.extend(len.to_be_bytes());
    out.extend_from_slice(metadata);
    out.push(0);
    out.extend(0u32.to_be_bytes());
    out.extend_from_slice(codestream);
    Ok(out)
}

/// An export's samples.
pub(super) enum Samples<'a> {
    Eight(&'a [u8]),
    Sixteen(&'a [u16]),
}

/// The signature and image header, padded to a byte: what zune writes when `ours` is
/// false (sRGB, no extra fields), else with `colour` and, for HDR, tone mapping.
fn header(width: u32, height: u32, bits: u32, colour: Colour, ours: bool) -> Vec<u8> {
    let mut w = Bits::default();
    w.put(16, 0x0AFF);
    w.put(1, 0); // not small
    let size = |w: &mut Bits, v: u32| {
        let v = u64::from(v - 1);
        for (selector, width) in [(0, 9), (1, 13), (2, 18), (3, 30)] {
            if v < (1 << width) {
                w.put(2, selector);
                w.put(width, v);
                return;
            }
        }
    };
    size(&mut w, height);
    w.put(3, 0); // no aspect ratio: width follows
    size(&mut w, width);
    let hdr = ours && matches!(colour, Colour::Hdr { .. });
    w.put(1, 0); // metadata not all default
    w.put(1, u64::from(hdr)); // extra fields: tone mapping
    if hdr {
        w.put(3, 0); // orientation 1
        w.put(1, 0); // no intrinsic size
        w.put(1, 0); // no preview
        w.put(1, 0); // no animation
    }
    w.put(1, 0); // integer samples
    if bits == 8 {
        w.put(2, 0);
    } else {
        w.put(2, 0b11);
        w.put(6, u64::from(bits - 1));
    }
    w.put(1, u64::from(bits <= 14)); // modular 16-bit buffer sufficient
    w.put(2, 0); // no extra channels
    w.put(1, 0); // not XYB
    match (ours, colour) {
        (
            true,
            Colour::Hdr {
                primaries,
                transfer,
                ..
            },
        ) => {
            w.put(1, 0); // colour encoding not all default
            w.put(1, 0); // no ICC
            enumeration(&mut w, 0); // RGB
            enumeration(&mut w, 1); // D65
            enumeration(&mut w, u32::from(primaries));
            w.put(1, 0); // no gamma: an enumerated transfer
            enumeration(&mut w, u32::from(transfer));
            enumeration(&mut w, 1); // relative colorimetric
        }
        _ => w.put(1, 1), // all default: sRGB
    }
    if hdr
        && let Colour::Hdr {
            intensity_target, ..
        } = colour
    {
        w.put(1, 0); // tone mapping not all default
        w.put(16, u64::from(f16(intensity_target)));
        w.put(16, u64::from(f16(0.))); // min_nits
        w.put(1, 0); // relative_to_max_display
        w.put(16, u64::from(f16(0.))); // linear_below
    }
    w.put(2, 0); // no extensions
    w.put(1, 1); // default transform data
    w.bytes()
}

/// A JPEG XL `Enum`: U32(Val(0), Val(1), BitsOffset(4, 2), BitsOffset(6, 18)).
fn enumeration(w: &mut Bits, v: u32) {
    match v {
        0 | 1 => w.put(2, u64::from(v)),
        2..=17 => {
            w.put(2, 2);
            w.put(4, u64::from(v - 2));
        }
        _ => {
            w.put(2, 3);
            w.put(6, u64::from(v - 18));
        }
    }
}

/// `v` as IEEE 754 half precision bits, for the header's F16 fields; `v` must be a
/// normal half (between 2^-14 and 65504) or 0.
fn f16(v: f32) -> u16 {
    if v == 0. {
        return 0;
    }
    let bits = v.to_bits();
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = (bits >> 13) & 0x3ff;
    ((exponent as u32) << 10 | mantissa) as u16
}

/// Bits written least significant first, as JPEG XL's bit stream is.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}
impl Bits {
    fn put(&mut self, n: u32, v: u64) {
        for i in 0..n {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let bit = (v >> i) & 1;
            *self.bytes.last_mut().expect("a byte") |= (bit as u8) << (self.used % 8);
            self.used += 1;
        }
    }
    fn bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// `codestream` in a JPEG XL container with the export's EXIF (a TIFF stream), XMP and
/// gain-map bundle ([`gain_map_bundle`]).
pub(super) fn container(
    codestream: &[u8],
    exif: Option<&[u8]>,
    xmp: Option<&str>,
    gain_map: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut out = vec![
        0, 0, 0, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a,
    ];
    let mut push = |kind: &[u8; 4], parts: &[&[u8]]| -> Result<()> {
        let len: usize = 8 + parts.iter().map(|p| p.len()).sum::<usize>();
        let Ok(size) = u32::try_from(len) else {
            bail!("A JPEG XL box is too large");
        };
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(kind);
        for p in parts {
            out.extend_from_slice(p);
        }
        Ok(())
    };
    push(b"ftyp", &[b"jxl ", &[0, 0, 0, 0], b"jxl "])?;
    if let Some(exif) = exif {
        // The offset of the TIFF header within the payload.
        push(b"Exif", &[&[0, 0, 0, 0], exif])?;
    }
    if let Some(xmp) = xmp {
        push(b"xml ", &[xmp.as_bytes()])?;
    }
    if let Some(bundle) = gain_map {
        push(b"jhgm", &[bundle])?;
    }
    push(b"jxlc", &[codestream])?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_and_enums_encode_as_the_format_says() {
        assert_eq!(f16(10_000.), 0x70e2);
        assert_eq!(f16(1000.), 0x63d0);
        assert_eq!(f16(255.), 0x5bf8);
        let mut w = Bits::default();
        enumeration(&mut w, 18);
        assert_eq!(w.bytes(), [0b11]);
    }

    /// The header RAWmakase writes in zune's place is read back by jxl-oxide, an
    /// independent decoder, with its colour encoding and tone mapping; the samples are
    /// lossless.
    #[test]
    fn an_hdr_codestream_decodes_with_its_colour_encoding() -> Result<()> {
        let (w, h) = (37u32, 21u32);
        let samples: Vec<u16> = (0..w * h * 3).map(|i| (i * 977 % 65536) as u16).collect();
        let colour = Colour::Hdr {
            primaries: 9,
            transfer: 16,
            intensity_target: 10_000.,
        };
        let bytes = container(
            &codestream(w, h, Samples::Sixteen(&samples), colour)?,
            None,
            Some("<x:xmpmeta/>"),
            None,
        )?;
        let image = jxl_oxide::JxlImage::builder()
            .read(std::io::Cursor::new(&bytes))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let header = image.image_header();
        assert_eq!((header.size.width, header.size.height), (w, h));
        assert_eq!(header.metadata.bit_depth.bits_per_sample(), 16);
        assert!((header.metadata.tone_mapping.intensity_target - 10_000.).abs() < 1.);
        let encoding = format!("{:?}", header.metadata.colour_encoding);
        assert!(
            encoding.contains("Bt2100") && encoding.contains("Pq"),
            "{encoding}"
        );
        let frame = image.render_frame(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        let decoded = frame.image_all_channels();
        let back: Vec<u16> = decoded
            .buf()
            .iter()
            .map(|v| (v * 65535.).round() as u16)
            .collect();
        assert_eq!(back, samples);
        Ok(())
    }

    /// A gain-map bundle is laid out as libjxl's `JxlGainMapReadBundle` reads it, and
    /// its codestream decodes as the map's gray samples.
    #[test]
    fn a_gain_map_bundle_reads_back_as_libjxl_reads_it() -> Result<()> {
        let samples: Vec<u8> = (0..16 * 9).map(|i| (i * 7 % 256) as u8).collect();
        let codestream = gray_codestream(16, 9, &samples)?;
        let metadata = [7u8; 61];
        let bundle = gain_map_bundle(&metadata, &codestream)?;
        // libjxl's reader: version, BE16 metadata size, metadata, u8 colour encoding
        // size (0: none), BE32 ICC size (0), then the codestream to the end.
        assert_eq!(bundle[0], 0);
        assert_eq!(u16::from_be_bytes([bundle[1], bundle[2]]), 61);
        assert_eq!(&bundle[3..64], &metadata);
        assert_eq!(bundle[64], 0);
        assert_eq!(&bundle[65..69], &[0, 0, 0, 0]);
        assert_eq!(&bundle[69..], &codestream[..]);
        let image = jxl_oxide::JxlImage::builder()
            .read(std::io::Cursor::new(&codestream))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let frame = image.render_frame(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        let back: Vec<u8> = frame
            .image_all_channels()
            .buf()
            .iter()
            .map(|v| (v * 255.).round() as u8)
            .collect();
        assert_eq!(back, samples);
        Ok(())
    }

    #[test]
    fn an_srgb_codestream_keeps_zunes_header() -> Result<()> {
        let samples: Vec<u8> = (0..4 * 3 * 3).map(|i| i as u8).collect();
        let bytes = codestream(4, 3, Samples::Eight(&samples), Colour::Srgb)?;
        let image = jxl_oxide::JxlImage::builder()
            .read(std::io::Cursor::new(&bytes))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let encoding = format!("{:?}", image.image_header().metadata.colour_encoding);
        assert!(encoding.contains("Srgb"), "{encoding}");
        Ok(())
    }
}
