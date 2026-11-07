//! JPEG XL exports (docs/hdr.md#hdr-export), lossless or lossy.
//!
//! Lossless exports: `zune-jpegxl` encodes the frame; RAWmakase writes the image
//! header in its place, because zune's always says sRGB: ours names the colour
//! encoding (sRGB, or an HDR export's primaries and PQ or HLG) and, for HDR, the tone
//! mapping's intensity target. The frame begins at a byte boundary after the header
//! (ISO/IEC 18181-1, the header ends with ZeroPadToByte), so zune's header is
//! replaced whole: its length is found by writing the same bits zune writes and
//! checked against zune's own bytes before anything is spliced.
//!
//! Lossy exports: `jixel` encodes a VarDCT frame in XYB. XYB holds absolute display
//! light, so the header's colour encoding only tells a decoder which signal to give
//! back. jixel reads PQ input as that light correctly, but takes HLG input for
//! normalised scene light (no OOTF, no intensity target), which decodes about two
//! stops dark. An HLG export is therefore given to jixel as the PQ of its display
//! light on HLG's reference display, and its header, checked the same way as zune's,
//! is replaced with one that says HLG.
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

/// The VarDCT distance of a lossy export at JPEG `quality` (1–100), as libjxl's
/// `cjxl` maps quality to distance: 90 is distance 1 (visually lossless), 100 is 0.1.
pub(super) fn distance(quality: u8) -> f32 {
    let q = f32::from(quality.clamp(1, 100));
    if q >= 30. {
        0.1 + (100. - q) * 0.09
    } else {
        53. / 3000. * q * q - 23. / 20. * q + 25.
    }
}

/// jixel's settings for a lossy frame at `distance` in `colour_encoding`: its fast
/// effort, which encodes a 45 MP photo in seconds (docs/records/hdr-stage1-results.md).
fn lossy_config(distance: f32, colour_encoding: jixel::ColorEncoding) -> jixel::EncodeConfig {
    jixel::EncodeConfig {
        distance,
        color_encoding: colour_encoding,
        speed: jixel::Speed::Fast,
        ..Default::default()
    }
}

/// A lossy codestream of 16-bit PQ samples (`pq`, interleaved RGB: absolute display
/// light) whose header names `colour`: PQ as encoded, or HLG, whose decoders give
/// back the HLG signal of that light for a display of `intensity_target`. jixel's
/// header is checked against the one RAWmakase writes for the PQ frame before it is
/// replaced, so a jixel that writes another one is refused, not spliced.
pub(super) fn lossy_hdr_codestream(
    width: u32,
    height: u32,
    pq: &[u16],
    colour: Colour,
    distance: f32,
) -> Result<Vec<u8>> {
    let Colour::Hdr {
        primaries,
        transfer,
        intensity_target,
    } = colour
    else {
        bail!("A lossy HDR JPEG XL needs an HDR colour encoding");
    };
    ensure!(
        transfer == 16 || transfer == 18,
        "Lossy HDR JPEG XL exports are PQ or HLG"
    );
    let jixel_primaries = match primaries {
        1 => jixel::Primaries::Bt709,
        9 => jixel::Primaries::Bt2020,
        // JPEG XL's P3 (11) with a D65 white point: Display P3.
        11 => jixel::Primaries::Smpte431,
        _ => bail!("Lossy HDR JPEG XL exports are Rec. 709, Rec. 2020 or Display P3"),
    };
    let encoding = jixel::ColorEncoding {
        white_point: jixel::WhitePoint::D65,
        primaries: jixel_primaries,
        transfer: jixel::TransferFunction::Smpte2084,
        rendering_intent: jixel::RenderingIntent::Relative,
    };
    let config = jixel::EncodeConfig {
        intensity_target: Some(intensity_target),
        ..lossy_config(distance, encoding)
    };
    let encoded = jixel::encode_image_16bit(pq, width as usize, height as usize, &config)
        .map_err(|e| anyhow::anyhow!("JPEG XL encoding failed: {e:?}"))?;
    let as_pq = Colour::Hdr {
        primaries,
        transfer: 16,
        intensity_target,
    };
    replace_header(
        &encoded,
        &header(width, height, 16, as_pq, Writer::Jixel),
        header(width, height, 16, colour, Writer::Jixel),
    )
}

/// `encoded` with its header `theirs` replaced by `ours`, refused when it doesn't
/// start with exactly `theirs`.
fn replace_header(encoded: &[u8], theirs: &[u8], mut ours: Vec<u8>) -> Result<Vec<u8>> {
    ensure!(
        encoded.starts_with(theirs),
        "The JPEG XL encoder's header is not the one RAWmakase replaces"
    );
    ours.extend_from_slice(&encoded[theirs.len()..]);
    Ok(ours)
}

/// A JPEG XL codestream of interleaved RGB `samples` (8- or 16-bit) with `colour`
/// in its header: lossless, or lossy at `distance` when one is given (8-bit sRGB
/// only; HDR goes through [`lossy_hdr_codestream`]).
pub(super) fn codestream(
    width: u32,
    height: u32,
    samples: Samples<'_>,
    colour: Colour,
    distance: Option<f32>,
) -> Result<Vec<u8>> {
    if let Some(distance) = distance {
        let (Samples::Eight(rgb), Colour::Srgb) = (samples, colour) else {
            bail!("Lossy JPEG XL of these samples goes through lossy_hdr_codestream");
        };
        let config = lossy_config(distance, jixel::ColorEncoding::srgb());
        return jixel::encode_image(rgb, width as usize, height as usize, &config)
            .map_err(|e| anyhow::anyhow!("JPEG XL encoding failed: {e:?}"));
    }
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
    let theirs = header(width, height, bits as u32, Colour::Srgb, Writer::Zune);
    ensure!(
        encoded.starts_with(&theirs),
        "The JPEG XL encoder's header is not the one RAWmakase replaces"
    );
    let mut out = header(width, height, bits as u32, colour, Writer::Ours);
    out.extend_from_slice(&encoded[theirs.len()..]);
    Ok(out)
}

/// An 8-bit grayscale codestream of `samples` (its header names gray with the sRGB
/// transfer): a gain map's. Lossless as zune writes it, or lossy at `distance`.
pub(super) fn gray_codestream(
    width: u32,
    height: u32,
    samples: &[u8],
    distance: Option<f32>,
) -> Result<Vec<u8>> {
    use zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions};
    ensure!(
        samples.len() == width as usize * height as usize,
        "JPEG XL gain map has the wrong size"
    );
    if let Some(distance) = distance {
        let config = lossy_config(distance, jixel::ColorEncoding::srgb());
        return jixel::encode_image_gray(samples, width as usize, height as usize, &config)
            .map_err(|e| anyhow::anyhow!("JPEG XL encoding failed: {e:?}"));
    }
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

/// Whose image header [`header`] writes.
#[derive(Clone, Copy, PartialEq)]
enum Writer {
    /// zune's lossless one: sRGB, no extra fields, whatever `colour` says.
    Zune,
    /// RAWmakase's for zune's lossless frame: `colour` and, for HDR, tone mapping.
    Ours,
    /// jixel's for a lossy 16-bit HDR frame, as [`Writer::Ours`] but XYB-encoded and
    /// with the modular 16-bit buffer flag jixel sets.
    Jixel,
}

/// The signature and image header, padded to a byte, as `writer` writes it.
fn header(width: u32, height: u32, bits: u32, colour: Colour, writer: Writer) -> Vec<u8> {
    let ours = writer != Writer::Zune;
    let xyb = writer == Writer::Jixel;
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
    w.put(1, u64::from(bits <= 14 || xyb)); // modular 16-bit buffer sufficient
    w.put(2, 0); // no extra channels
    w.put(1, u64::from(xyb));
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
            &codestream(w, h, Samples::Sixteen(&samples), colour, None)?,
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
        let codestream = gray_codestream(16, 9, &samples, None)?;
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
    fn quality_maps_to_libjxls_distance() {
        assert!((distance(90) - 1.).abs() < 1e-6);
        assert!((distance(100) - 0.1).abs() < 1e-6);
        assert!((distance(92) - 0.82).abs() < 1e-6);
        assert!((distance(30) - 6.4).abs() < 1e-5);
        assert!((distance(1) - 23.8677).abs() < 1e-3);
    }

    /// A ramp from black to 16× graphics white with saturated patches, as PQ signal.
    fn hdr_scene(w: u32, h: u32) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let light = 2f32.powf(x as f32 / (w - 1) as f32 * 12. - 8.);
                match y * 3 / h {
                    0 => [light; 3],
                    1 => [light, light * 0.25, light * 0.1],
                    _ => [light * 0.2, light * 0.5, light],
                }
            })
            .collect()
    }

    fn decode(bytes: &[u8]) -> Result<(jxl_oxide::JxlImage, Vec<f32>)> {
        let image = jxl_oxide::JxlImage::builder()
            .read(std::io::Cursor::new(bytes))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let frame = image.render_frame(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        let samples = frame.image_all_channels().buf().to_vec();
        Ok((image, samples))
    }

    /// The median and 95th percentile error, in stops, of decoded luminance against
    /// the scene's (Rec. 2020 weights for both), over pixels brighter than a twentieth
    /// of graphics white.
    fn stops(scene: &[[f32; 3]], decoded: &[[f32; 3]]) -> (f32, f32) {
        let weights = crate::color::hdr_output::HdrGamut::Rec2020.luminance();
        let y = |p: &[f32; 3]| (0..3).map(|c| weights[c] * p[c]).sum::<f32>();
        let mut errors: Vec<f32> = scene
            .iter()
            .zip(decoded)
            .map(|(a, b)| (y(a), y(b)))
            .filter(|(a, _)| *a > 0.05)
            .map(|(a, b)| (b.max(1e-6) / a).log2().abs())
            .collect();
        errors.sort_by(f32::total_cmp);
        (errors[errors.len() / 2], errors[errors.len() * 95 / 100])
    }

    /// A lossy PQ codestream: jixel's frame under the header RAWmakase checks, read
    /// by jxl-oxide as P3 PQ with its intensity target, and the light within a few
    /// hundredths of a stop of the scene's at quality 92.
    #[test]
    fn a_lossy_pq_codestream_keeps_its_light_and_encoding() -> Result<()> {
        use crate::color::hdr_output::{GRAPHICS_WHITE_NITS, pq_decode, pq_encode};
        let (w, h) = (96u32, 66u32);
        let scene = hdr_scene(w, h);
        let pq: Vec<u16> = scene
            .iter()
            .flatten()
            .map(|v| (pq_encode(v * GRAPHICS_WHITE_NITS) * 65535. + 0.5) as u16)
            .collect();
        let colour = Colour::Hdr {
            primaries: 11,
            transfer: 16,
            intensity_target: 10_000.,
        };
        let bytes = lossy_hdr_codestream(w, h, &pq, colour, distance(92))?;
        let (image, samples) = decode(&bytes)?;
        let header = image.image_header();
        assert!(header.metadata.xyb_encoded);
        assert_eq!(header.metadata.bit_depth.bits_per_sample(), 16);
        assert!((header.metadata.tone_mapping.intensity_target - 10_000.).abs() < 1.);
        let encoding = format!("{:?}", header.metadata.colour_encoding);
        assert!(
            encoding.contains("P3") && encoding.contains("Pq"),
            "{encoding}"
        );
        let decoded: Vec<[f32; 3]> = samples
            .chunks(3)
            .map(|p| [0, 1, 2].map(|c| pq_decode(p[c]) / GRAPHICS_WHITE_NITS))
            .collect();
        let (median, p95) = stops(&scene, &decoded);
        assert!(median < 0.03 && p95 < 0.1, "{median} {p95}");
        Ok(())
    }

    /// A lossy HLG codestream: the display light (HLG's reference display, 1,000
    /// cd/m²) encoded as PQ, under a header that says HLG. Read back by jxl-oxide as
    /// Rec. 2100 HLG with its intensity target; its light, and its HLG signal where
    /// that is neutral, are the export's, not two stops darker as jixel's own HLG
    /// input gives (it leaves out the OOTF and the intensity target).
    ///
    /// The light is read as PQ in BT.709 primaries, in which XYB decodes: jxl-oxide
    /// clips that light to the intensity target before converting it to wider
    /// primaries, so a saturated colour near HLG's peak would read back clipped in
    /// Rec. 2020 (libjxl's `djxl` does not clip it).
    #[test]
    fn a_lossy_hlg_codestream_decodes_to_its_hlg_signal() -> Result<()> {
        use crate::color::hdr_output::{
            GRAPHICS_WHITE_NITS, HdrGamut, hlg_decode, hlg_encode, pq_decode, pq_encode,
        };
        let (w, h) = (96u32, 66u32);
        let gamut = HdrGamut::Rec2020;
        let weights = gamut.luminance();
        // The scene as an HLG export holds it: clipped at the reference display's peak.
        let signal: Vec<[f32; 3]> = hdr_scene(w, h)
            .iter()
            .map(|p| hlg_encode(*p, weights))
            .collect();
        let scene: Vec<[f32; 3]> = signal.iter().map(|s| hlg_decode(*s, weights)).collect();
        let pq: Vec<u16> = scene
            .iter()
            .flatten()
            .map(|v| (pq_encode(v * GRAPHICS_WHITE_NITS) * 65535. + 0.5) as u16)
            .collect();
        let colour = Colour::Hdr {
            primaries: 9,
            transfer: 18,
            intensity_target: 1000.,
        };
        let bytes = lossy_hdr_codestream(w, h, &pq, colour, distance(92))?;
        let (image, samples) = decode(&bytes)?;
        let header = image.image_header();
        assert!((header.metadata.tone_mapping.intensity_target - 1000.).abs() < 1.);
        let encoding = format!("{:?}", header.metadata.colour_encoding);
        assert!(
            encoding.contains("Bt2100") && encoding.contains("Hlg"),
            "{encoding}"
        );
        // Neutral pixels (the first third of the rows) as HLG signal.
        let neutral = (w * h / 3) as usize;
        let decoded: Vec<[f32; 3]> = samples
            .chunks(3)
            .take(neutral)
            .map(|p| hlg_decode([p[0], p[1], p[2]].map(|v| v.clamp(0., 1.)), weights))
            .collect();
        let (median, p95) = stops(&scene[..neutral], &decoded);
        assert!(median < 0.03 && p95 < 0.1, "neutral HLG: {median} {p95}");
        // Every pixel as light.
        let mut image = jxl_oxide::JxlImage::builder()
            .read(std::io::Cursor::new(&bytes))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut bt709_pq = jxl_oxide::EnumColourEncoding::bt2100_pq(Default::default());
        bt709_pq.primaries = jxl_oxide::color::Primaries::Srgb;
        image.request_color_encoding(bt709_pq);
        let frame = image.render_frame(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        let to_rec2020 = crate::color::mul_matrix(
            gamut.prophoto_to_rgb(),
            crate::color::inverse(HdrGamut::Rec709.prophoto_to_rgb()),
        );
        let decoded: Vec<[f32; 3]> = frame
            .image_all_channels()
            .buf()
            .chunks(3)
            .map(|p| {
                let bt709 = [0, 1, 2].map(|c| pq_decode(p[c]) / GRAPHICS_WHITE_NITS);
                crate::color::mul(to_rec2020, bt709)
            })
            .collect();
        let (median, p95) = stops(&scene, &decoded);
        assert!(median < 0.03 && p95 < 0.1, "light: {median} {p95}");
        Ok(())
    }

    /// A header jixel writes differently from the one RAWmakase expects (here the
    /// lossless writer's, whose XYB and modular 16-bit buffer flags differ) is refused
    /// rather than replaced, and one it writes as expected is replaced whole. A colour
    /// RAWmakase never encodes lossy as HDR is refused before encoding.
    #[test]
    fn a_lossy_header_that_is_not_jixels_is_refused() -> Result<()> {
        let pq = Colour::Hdr {
            primaries: 9,
            transfer: 16,
            intensity_target: 1000.,
        };
        let hlg = Colour::Hdr {
            primaries: 9,
            transfer: 18,
            intensity_target: 1000.,
        };
        let theirs = header(64, 48, 16, pq, Writer::Jixel);
        let frame = [0xAA, 0x55];
        let encoded = [theirs.as_slice(), &frame].concat();
        let ours = header(64, 48, 16, hlg, Writer::Jixel);
        let replaced = replace_header(&encoded, &theirs, ours.clone())?;
        assert_eq!(replaced, [ours.as_slice(), &frame].concat());
        let other = header(64, 48, 16, pq, Writer::Ours);
        assert_ne!(other, theirs);
        let encoded = [other.as_slice(), &frame].concat();
        assert!(replace_header(&encoded, &theirs, ours).is_err());
        let samples = vec![30_000u16; 8 * 8 * 3];
        assert!(lossy_hdr_codestream(8, 8, &samples, Colour::Srgb, 1.).is_err());
        Ok(())
    }

    /// A lossy 8-bit sRGB codestream and a lossy gray one (a gain map's): read back
    /// as sRGB and gray, near the samples given.
    #[test]
    fn lossy_sdr_and_gray_codestreams_decode_near_their_samples() -> Result<()> {
        let (w, h) = (64u32, 40u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| ((i / 3 % w) * 4) as u8).collect();
        let (image, samples) = decode(&codestream(
            w,
            h,
            Samples::Eight(&rgb),
            Colour::Srgb,
            Some(distance(92)),
        )?)?;
        let encoding = format!("{:?}", image.image_header().metadata.colour_encoding);
        assert!(encoding.contains("Srgb"), "{encoding}");
        let worst = rgb
            .iter()
            .zip(&samples)
            .map(|(a, b)| (f32::from(*a) - b * 255.).abs())
            .fold(0f32, f32::max);
        assert!(worst < 12., "{worst}");
        let gray: Vec<u8> = (0..w * h).map(|i| (i % w * 4) as u8).collect();
        let (image, samples) = decode(&gray_codestream(w, h, &gray, Some(distance(92)))?)?;
        let encoding = format!("{:?}", image.image_header().metadata.colour_encoding);
        assert!(encoding.contains("Grey"), "{encoding}");
        assert_eq!(samples.len(), gray.len());
        let worst = gray
            .iter()
            .zip(&samples)
            .map(|(a, b)| (f32::from(*a) - b * 255.).abs())
            .fold(0f32, f32::max);
        assert!(worst < 6., "{worst}");
        Ok(())
    }

    #[test]
    fn an_srgb_codestream_keeps_zunes_header() -> Result<()> {
        let samples: Vec<u8> = (0..4 * 3 * 3).map(|i| i as u8).collect();
        let bytes = codestream(4, 3, Samples::Eight(&samples), Colour::Srgb, None)?;
        let image = jxl_oxide::JxlImage::builder()
            .read(std::io::Cursor::new(&bytes))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let encoding = format!("{:?}", image.image_header().metadata.colour_encoding);
        assert!(encoding.contains("Srgb"), "{encoding}");
        Ok(())
    }
}
