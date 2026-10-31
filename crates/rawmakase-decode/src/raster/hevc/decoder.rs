//! One HEVC picture decoded by rust_h265 (pure Rust, without `unsafe`): the
//! parameter sets of a HEIF item's `hvcC`, then the item's length-prefixed
//! NAL units.
//!
//! rust_h265 is young and may panic on malformed data rather than return an
//! error. Being Rust, the panic unwinds, and [`decode`] catches it, so a
//! corrupt file is an error, not a crash. A sequence parameter set coding a
//! picture wider or taller than the header says is refused before rust_h265
//! reads it, and its limits refuse one of over 512 MiB of state, so a header
//! claiming small tiles can't make it decode large ones.
use super::Config;
use anyhow::{Result, anyhow, bail};
use rust_h265::{
    DecodeLimits, Decoder, Frame, NalUnit, NalUnitType, PixelData, parse_hvcc, parse_nal,
};

/// A decoded picture: luma, then two chroma planes each `chroma_width` wide,
/// rows packed, 8-bit samples or deeper ones.
pub(in crate::raster) struct Picture {
    pub(in crate::raster) width: u32,
    pub(in crate::raster) height: u32,
    pub(in crate::raster) depth: u8,
    /// 4:2:0 (subsampled both ways) rather than 4:4:4.
    pub(in crate::raster) subsampled: bool,
    pub(in crate::raster) planes: Planes,
}

pub(in crate::raster) enum Planes {
    Bytes([Vec<u8>; 3]),
    Words([Vec<u16>; 3]),
}

impl Picture {
    /// The width of a chroma plane.
    pub(in crate::raster) fn chroma_width(&self) -> u32 {
        if self.subsampled {
            self.width.div_ceil(2)
        } else {
            self.width
        }
    }
    /// The height of a chroma plane.
    pub(in crate::raster) fn chroma_height(&self) -> u32 {
        if self.subsampled {
            self.height.div_ceil(2)
        } else {
            self.height
        }
    }
}

/// The picture coded in `data`, an item's NAL units, after `config`'s
/// parameter sets; refused, before it is allocated, if it is coded larger than
/// `size`, the size the header gives it, allows.
pub(in crate::raster) fn decode(config: &Config, data: &[u8], size: (u32, u32)) -> Result<Picture> {
    let frame = std::panic::catch_unwind(|| decode_frame(config, data, size))
        .map_err(|_| anyhow!("The HEVC image data is corrupt"))??;
    picture(frame)
}

/// A side the header says is `v` pixels long rounded up to whole coding
/// blocks, of at most 64 pixels a side: a picture is coded in whole blocks
/// and cropped to its size, so it is coded no larger than that.
fn blocks(v: u32) -> u64 {
    u64::from(v).div_ceil(64) * 64
}

/// rust_h265's limits for a picture the header says is `width` × `height`.
fn limits((width, height): (u32, u32)) -> DecodeLimits {
    DecodeLimits {
        max_picture_pixels: blocks(width).saturating_mul(blocks(height)),
        ..DecodeLimits::default()
    }
}

/// `nal` refused if it is a sequence parameter set coding a picture wider or
/// taller than `size` in whole blocks: rust_h265 holds only the picture's
/// area to its limit, then allocates it in whole blocks, so a picture of one
/// row would take 64.
fn check(nal: &NalUnit, (width, height): (u32, u32)) -> Result<()> {
    if nal.nal_unit_type != NalUnitType::Sps {
        return Ok(());
    }
    let (w, h) = coded_size(&nal.rbsp).ok_or_else(|| anyhow!("The HEVC image data is corrupt"))?;
    if u64::from(w) > blocks(width) || u64::from(h) > blocks(height) {
        bail!("The HEVC image is coded larger than the header says ({w} x {h})");
    }
    Ok(())
}

/// The picture size a sequence parameter set's RBSP codes (H.265 7.3.2.2):
/// `pic_width_in_luma_samples` and `pic_height_in_luma_samples`, after the
/// profile, tier and level and the chroma format.
fn coded_size(rbsp: &[u8]) -> Option<(u32, u32)> {
    let mut bits = Bits { data: rbsp, at: 0 };
    bits.skip(4)?; // sps_video_parameter_set_id
    let sub_layers = bits.read(3)? as usize;
    bits.skip(1)?; // sps_temporal_id_nesting_flag
    bits.skip(96)?; // the general profile, tier and level
    if sub_layers > 0 {
        let mut present = [(false, false); 7];
        for p in &mut present[..sub_layers] {
            *p = (bits.read(1)? == 1, bits.read(1)? == 1);
        }
        bits.skip(2 * (8 - sub_layers))?; // reserved_zero_2bits
        for (profile, level) in &present[..sub_layers] {
            bits.skip(if *profile { 88 } else { 0 } + if *level { 8 } else { 0 })?;
        }
    }
    bits.ue()?; // sps_seq_parameter_set_id
    if bits.ue()? == 3 {
        bits.skip(1)?; // separate_colour_plane_flag
    }
    Some((bits.ue()?, bits.ue()?))
}

/// A reader of an RBSP's bits, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn skip(&mut self, n: usize) -> Option<()> {
        self.at += n;
        (self.at <= self.data.len() * 8).then_some(())
    }

    /// `n` bits, at most 32, as a number.
    fn read(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u64;
        for _ in 0..n {
            let byte = self.data.get(self.at / 8)?;
            v = v << 1 | u64::from(byte >> (7 - self.at % 8) & 1);
            self.at += 1;
        }
        u32::try_from(v).ok()
    }

    /// An unsigned Exp-Golomb number (`ue(v)`) of at most 32 bits.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.read(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let v = (1u64 << zeros) - 1 + u64::from(self.read(zeros)?);
        u32::try_from(v).ok()
    }
}

fn decode_frame(config: &Config, data: &[u8], size: (u32, u32)) -> Result<Frame> {
    let failed = |e: rust_h265::DecodeError| anyhow!("Could not decode the HEVC image: {e}");
    let mut decoder = Decoder::with_limits(limits(size));
    for nal in config.parameter_sets.iter().filter_map(|n| parse_nal(n)) {
        check(&nal, size)?;
        decoder.decode_nal(&nal).map_err(failed)?;
    }
    for nal in parse_hvcc(data, config.length_size) {
        check(&nal, size)?;
        // A still is one picture: the first one finished is it.
        if let Some(frame) = decoder.decode_nal(&nal).map_err(failed)? {
            return Ok(frame);
        }
    }
    decoder
        .flush()
        .ok_or_else(|| anyhow!("The HEVC image data is missing or cut off"))
}

/// `frame` checked: 4:2:0 or 4:4:4, of 8 to 12 bits, each plane as large as
/// its size says.
fn picture(frame: Frame) -> Result<Picture> {
    let subsampled = match frame.chroma_format_idc {
        1 => true,
        3 => false,
        n => bail!("HEVC images in chroma format {n} can't be read"),
    };
    if !matches!(frame.bit_depth, 8 | 10 | 12) {
        bail!("{}-bit HEVC images are not supported", frame.bit_depth);
    }
    let Frame {
        y,
        u,
        v,
        width,
        height,
        bit_depth,
        ..
    } = frame;
    let planes = match (y, u, v) {
        (PixelData::U8(y), PixelData::U8(u), PixelData::U8(v)) if bit_depth == 8 => {
            Planes::Bytes([y, u, v])
        }
        (PixelData::U16(y), PixelData::U16(u), PixelData::U16(v)) if bit_depth > 8 => {
            Planes::Words([y, u, v])
        }
        _ => bail!("The HEVC image's samples are not as deep as it says"),
    };
    let picture = Picture {
        width,
        height,
        depth: bit_depth,
        subsampled,
        planes,
    };
    let luma = width as usize * height as usize;
    let chroma = picture.chroma_width() as usize * picture.chroma_height() as usize;
    let lengths = match &picture.planes {
        Planes::Bytes(p) => p.each_ref().map(Vec::len),
        Planes::Words(p) => p.each_ref().map(Vec::len),
    };
    if lengths != [luma, chroma, chroma] {
        bail!("The HEVC image is not the size it says");
    }
    Ok(picture)
}

#[cfg(test)]
mod tests {
    use super::super::tests::hvcc;
    use super::*;
    use std::path::Path;

    #[test]
    fn data_without_a_picture_is_an_error() {
        let config = Config::read(&hvcc(&[])).unwrap();
        let error = decode(&config, &[0, 0, 0, 2, 0x4e, 1], (8, 8))
            .err()
            .unwrap();
        assert!(error.to_string().contains("missing"), "{error}");
        assert!(decode(&config, &[], (8, 8)).is_err());
    }

    /// A sequence parameter set NAL unit coding `width` × `height` in 4:2:0,
    /// with `sub_layers` sub-layers each giving its profile and level; cut off
    /// after the size, as only that is read before rust_h265 is given it.
    fn sps(sub_layers: u32, width: u32, height: u32) -> Vec<u8> {
        let mut bits = Vec::new();
        let mut put = |v: u64, n: u32| bits.extend((0..n).rev().map(|i| (v >> i & 1) as u8));
        put(0, 4);
        put(sub_layers.into(), 3);
        put(1, 1);
        put(0x0160_0000_0000, 48); // profile Main, then flags
        put(0, 48);
        if sub_layers > 0 {
            put(0b11_1111_1111_1111 >> (14 - 2 * sub_layers), 2 * sub_layers);
            put(0, 2 * (8 - sub_layers));
            for _ in 0..sub_layers {
                put(0, 44);
                put(0, 44);
                put(0x5a, 8);
            }
        }
        let mut ue = |v: u32| {
            let v = u64::from(v) + 1;
            let n = 64 - v.leading_zeros();
            put(0, n - 1);
            put(v, n);
        };
        ue(0);
        ue(1);
        ue(width);
        ue(height);
        put(1, 1);
        bits.resize(bits.len().next_multiple_of(8), 0);
        let mut nal = vec![0x42, 0x01];
        nal.extend(bits.chunks(8).map(|b| b.iter().fold(0, |a, b| a << 1 | b)));
        nal
    }

    #[test]
    fn a_sequence_parameter_sets_size_is_read() {
        for sub_layers in [0, 1, 6] {
            let nal = sps(sub_layers, 4032, 3024);
            assert_eq!(coded_size(&nal[2..]), Some((4032, 3024)), "{sub_layers}");
            assert_eq!(coded_size(&nal[2..14]), None);
        }
        // The fixtures' own, from libheif's x265.
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/formats");
        for (name, size) in [("wide.heic", (192, 128)), ("ten-bit.heic", (64, 64))] {
            let bytes = std::fs::read(fixtures.join(name)).unwrap();
            let at = bytes.windows(4).position(|w| w == b"hvcC").unwrap() + 4;
            let config = Config::read(&bytes[at..]).unwrap();
            let sps = config.parameter_sets.iter().filter_map(|n| parse_nal(n));
            let sizes: Vec<_> = sps
                .filter(|n| n.nal_unit_type == NalUnitType::Sps)
                .collect();
            assert_eq!(sizes.len(), 1);
            assert_eq!(coded_size(&sizes[0].rbsp), Some(size), "{name}");
        }
    }

    /// rust_h265 holds only a picture's area to its limit: one 4096 pixels
    /// wide and one tall passes for 64 × 64, then takes 64 rows.
    #[test]
    fn a_picture_coded_wider_or_taller_than_its_size_is_refused_unread() {
        for (width, height) in [(4096, 1), (1, 4096), (128, 64)] {
            let config = Config::read(&hvcc(&[&sps(0, width, height)])).unwrap();
            let error = decode(&config, &[], (64, 64)).err().unwrap();
            assert!(error.to_string().contains("coded larger"), "{error}");
            // In the image data too.
            let config = Config::read(&hvcc(&[])).unwrap();
            let mut data = (sps(0, width, height).len() as u32).to_be_bytes().to_vec();
            data.extend(sps(0, width, height));
            let error = decode(&config, &data, (64, 64)).err().unwrap();
            assert!(error.to_string().contains("coded larger"), "{error}");
        }
        // Within whole blocks of its size it is read on.
        let config = Config::read(&hvcc(&[&sps(0, 64, 64)])).unwrap();
        let error = decode(&config, &[], (16, 12)).err().unwrap();
        assert!(!error.to_string().contains("coded larger"), "{error}");
    }

    #[test]
    fn a_picture_may_be_coded_no_larger_than_its_size_in_whole_blocks() {
        assert_eq!(limits((64, 64)).max_picture_pixels, 64 * 64);
        assert_eq!(limits((16, 12)).max_picture_pixels, 64 * 64);
        assert_eq!(limits((4032, 3024)).max_picture_pixels, 4032 * 3072);
        // However large the header says it is.
        assert_eq!(limits((u32::MAX, u32::MAX)).max_picture_pixels, u64::MAX);
    }
}
