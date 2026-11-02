//! A HEIF item's HEVC configuration (`hvcC`), read from the header: the depth
//! and chroma the system's decoder will give, and the size each sequence
//! parameter set (SPS) codes the picture at, there or in the item's data, so
//! a picture coded larger than RAWmakase opens is refused before the system's
//! decoder allocates it.

/// The largest `hvcC` read: its parameter sets take a few hundred bytes, and
/// an encoder's SEI banner a kilobyte or two.
const MAX_CONFIG: usize = 16 << 10;

/// What an `hvcC` (ISO/IEC 14496-15 HEVCDecoderConfigurationRecord) says.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Config {
    /// Bits per luma sample.
    pub(super) depth: u8,
    /// `chroma_format_idc`: 0 monochrome, 1 4:2:0, 2 4:2:2, 3 4:4:4.
    pub(super) chroma: u8,
    /// Bytes before each NAL unit of the item's data giving its length: 1, 2
    /// or 4.
    pub(super) length_size: u8,
    /// The size each of its SPS codes the picture at.
    pub(super) coded: Vec<(u32, u32)>,
}

impl Config {
    /// The record in `hvcc`; `None` when it is cut off, not version 1, over
    /// 16 KiB, or holds an SPS whose size can't be read.
    pub(super) fn read(hvcc: &[u8]) -> Option<Self> {
        if hvcc.len() < 23 || hvcc.len() > MAX_CONFIG || hvcc[0] != 1 {
            return None;
        }
        let length_size = (hvcc[21] & 3) + 1;
        if length_size == 3 {
            return None;
        }
        let mut rest = &hvcc[23..];
        let mut take = |n: usize| {
            let (head, tail) = rest.split_at_checked(n)?;
            rest = tail;
            Some(head)
        };
        let mut coded = Vec::new();
        for _ in 0..hvcc[22] {
            take(1)?; // completeness and NAL unit type
            let count = u16::from_be_bytes(take(2)?.try_into().ok()?);
            for _ in 0..count {
                let len = u16::from_be_bytes(take(2)?.try_into().ok()?);
                if let Some(size) = sps_size(take(len as usize)?)? {
                    coded.push(size);
                }
            }
        }
        Some(Self {
            depth: (hvcc[17] & 7) + 8,
            chroma: hvcc[16] & 3,
            length_size,
            coded,
        })
    }
}

/// The pixels in a picture coded at `size`.
pub(super) fn area((width, height): (u32, u32)) -> u128 {
    u128::from(width) * u128::from(height)
}

/// The largest [`area`] any SPS among an item's NAL units codes the picture
/// at, 0 with none, each unit after `length_size` bytes giving its length;
/// `None` when a unit is cut off or an SPS can't be read. Only the largest is
/// kept, however many SPS there are.
pub(super) fn largest_coded_in(mut data: &[u8], length_size: u8) -> Option<u128> {
    let mut largest = 0;
    while !data.is_empty() {
        let (len, rest) = data.split_at_checked(usize::from(length_size))?;
        let len = len.iter().fold(0usize, |n, b| n << 8 | usize::from(*b));
        let (nal, rest) = rest.split_at_checked(len)?;
        if let Some(size) = sps_size(nal)? {
            largest = largest.max(area(size));
        }
        data = rest;
    }
    Some(largest)
}

/// The size an SPS NAL unit codes the picture at; `Some(None)` for a NAL unit
/// of another type, `None` for an SPS that can't be read.
fn sps_size(nal: &[u8]) -> Option<Option<(u32, u32)>> {
    // The two-byte NAL unit header: its type is bits 1 to 6; an SPS is 33.
    if (nal.first()? >> 1) & 0x3f != 33 {
        return Some(None);
    }
    coded_size(&unescape(nal.get(2..)?)).map(Some)
}

/// A NAL unit's payload without its emulation prevention bytes: the 3 of
/// every 0, 0, 3.
fn unescape(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zeros = 0;
    for &b in payload {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An `hvcC` of 8-bit 4:2:0 with 4-byte lengths and one array of `nals`.
    fn hvcc(nals: &[&[u8]]) -> Vec<u8> {
        let mut record = vec![1, 1, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 0x5a];
        record.extend([0xf0, 0, 0xfc, 0xfd, 0xf8, 0xf8, 0, 0, 0x0f, 1]);
        record.extend([0xa1, 0, nals.len() as u8]);
        for nal in nals {
            record.extend((nal.len() as u16).to_be_bytes());
            record.extend(*nal);
        }
        record
    }

    /// An SPS NAL unit coding `width` × `height` in 4:2:0, with `sub_layers`
    /// sub-layers each giving its profile and level; cut off after the size,
    /// as only that is read, and escaped as an encoder would.
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
        let rbsp: Vec<u8> = bits
            .chunks(8)
            .map(|b| b.iter().fold(0, |a, b| a << 1 | b))
            .collect();
        let mut nal = vec![0x42, 0x01];
        let mut zeros = 0;
        for b in rbsp {
            if zeros >= 2 && b <= 3 {
                nal.push(3);
                zeros = 0;
            }
            zeros = if b == 0 { zeros + 1 } else { 0 };
            nal.push(b);
        }
        nal
    }

    #[test]
    fn a_configuration_records_depth_chroma_and_coded_sizes_are_read() {
        let config = Config::read(&hvcc(&[&sps(0, 4032, 3024), &[0x44, 1]])).unwrap();
        assert_eq!((config.depth, config.chroma, config.length_size), (8, 1, 4));
        assert_eq!(config.coded, [(4032, 3024)]);
        let mut ten_bit = hvcc(&[]);
        ten_bit[17] = 0xfa;
        assert_eq!(Config::read(&ten_bit).unwrap().depth, 10);
    }

    #[test]
    fn a_configuration_record_cut_off_or_of_another_version_is_not_read() {
        let record = hvcc(&[&sps(0, 64, 64)]);
        assert_eq!(Config::read(&record[..record.len() - 1]), None);
        assert_eq!(Config::read(&record[..20]), None);
        let mut version = record.clone();
        version[0] = 2;
        assert_eq!(Config::read(&version), None);
        // A length of 3 bytes is not one HEVC allows.
        let mut three = record;
        three[21] = 0x0e;
        assert_eq!(Config::read(&three), None);
        // Nor is one larger than parameter sets and SEI take.
        let large = vec![0x4e; 0xffff];
        assert!(Config::read(&hvcc(&[&large[..MAX_CONFIG - 30]])).is_some());
        assert_eq!(Config::read(&hvcc(&[&large[..MAX_CONFIG]])), None);
        // Nor one with an SPS cut off before its size.
        assert_eq!(Config::read(&hvcc(&[&sps(0, 64, 64)[..14]])), None);
    }

    #[test]
    fn an_sps_size_is_read_with_sub_layers_and_emulation_prevention() {
        for sub_layers in [0, 1, 6] {
            for size in [(4032, 3024), (64, 64), (4096, 1), (1, 1)] {
                let nal = sps(sub_layers, size.0, size.1);
                assert_eq!(sps_size(&nal), Some(Some(size)), "{sub_layers} {size:?}");
            }
        }
        // Another NAL unit type is not an SPS.
        assert_eq!(sps_size(&[0x44, 1, 0xc0]), Some(None));
    }

    /// The fixtures' own, from libheif's x265: 64×64 pictures cropped to
    /// 16×12, a grid's 64×64 tiles, and wide.heic's 192×128.
    #[test]
    fn the_fixtures_sps_sizes_are_read() {
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/formats");
        for (name, size) in [
            ("metadata.heic", (64, 64)),
            ("ten-bit.heic", (64, 64)),
            ("grid.heic", (64, 64)),
            ("wide.heic", (192, 128)),
        ] {
            let bytes = std::fs::read(fixtures.join(name)).unwrap();
            let at = bytes.windows(4).position(|w| w == b"hvcC").unwrap();
            let len = u32::from_be_bytes(bytes[at - 4..at].try_into().unwrap()) as usize;
            let config = Config::read(&bytes[at + 4..at - 4 + len]).unwrap();
            assert_eq!(config.coded, [size], "{name}");
        }
    }

    #[test]
    fn the_sps_in_an_items_nal_units_are_found() {
        let mut data = Vec::new();
        for nal in [&[0x26, 1, 0xaf][..], &sps(0, 192, 128), &[0x4e, 1, 5]] {
            data.extend((nal.len() as u32).to_be_bytes());
            data.extend(nal);
        }
        assert_eq!(largest_coded_in(&data, 4), Some(192 * 128));
        // The largest of several.
        let mut more = data.clone();
        for nal in [sps(0, 4032, 3024), sps(0, 64, 64)] {
            more.extend((nal.len() as u32).to_be_bytes());
            more.extend(nal);
        }
        assert_eq!(largest_coded_in(&more, 4), Some(4032 * 3024));
        // A unit cut off is no picture to decode.
        assert_eq!(largest_coded_in(&data[..data.len() - 1], 4), None);
        assert_eq!(largest_coded_in(&[], 4), Some(0));
    }
}
