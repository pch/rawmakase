//! A HEIF item's HEVC configuration (`hvcC`), read from the header, and the
//! decoder (`decoder`), built with the `software-hevc` feature.
#[cfg(feature = "software-hevc")]
mod decoder;
#[cfg(feature = "software-hevc")]
pub(super) use decoder::{Picture, Planes, decode};

/// The largest `hvcC` read: its parameter sets take a few hundred bytes, and
/// an encoder's SEI banner a kilobyte or two. Each tile's decoder reads it
/// again, so a larger one could make a small file of many tiles slow.
const MAX_CONFIG: usize = 16 << 10;

/// The most parameter sets and SEI an `hvcC` may hold: HEVC allows 16 VPS,
/// 16 SPS and 64 PPS, and encoders write one of each.
const MAX_PARAMETER_SETS: usize = 128;

/// What an `hvcC` (ISO/IEC 14496-15 HEVCDecoderConfigurationRecord) says.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Config {
    /// Bytes before each NAL unit giving its length: 1, 2 or 4.
    // This and the parameter sets are the decoder's.
    #[cfg_attr(not(feature = "software-hevc"), allow(dead_code))]
    length_size: u8,
    /// Bits per luma sample.
    pub(super) depth: u8,
    /// `chroma_format_idc`: 0 monochrome, 1 4:2:0, 2 4:2:2, 3 4:4:4.
    pub(super) chroma: u8,
    /// The VPS, SPS, PPS and SEI NAL units, in order.
    #[cfg_attr(not(feature = "software-hevc"), allow(dead_code))]
    parameter_sets: Vec<Vec<u8>>,
}

impl Config {
    /// The record in `hvcc`; `None` when it is cut off, not version 1 or
    /// over 16 KiB.
    pub(super) fn read(hvcc: &[u8]) -> Option<Self> {
        if hvcc.len() < 23 || hvcc.len() > MAX_CONFIG || hvcc[0] != 1 {
            return None;
        }
        let length_size = (hvcc[21] & 3) + 1;
        if length_size == 3 {
            return None;
        }
        let depth = (hvcc[17] & 7) + 8;
        let chroma = hvcc[16] & 3;
        let mut rest = &hvcc[23..];
        let mut take = |n: usize| {
            let (head, tail) = rest.split_at_checked(n)?;
            rest = tail;
            Some(head)
        };
        let mut parameter_sets = Vec::new();
        for _ in 0..hvcc[22] {
            take(1)?; // completeness and NAL unit type
            let count = u16::from_be_bytes(take(2)?.try_into().ok()?);
            for _ in 0..count {
                let len = u16::from_be_bytes(take(2)?.try_into().ok()?);
                if parameter_sets.len() == MAX_PARAMETER_SETS {
                    return None;
                }
                parameter_sets.push(take(len as usize)?.to_vec());
            }
        }
        Some(Self {
            length_size,
            depth,
            chroma,
            parameter_sets,
        })
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// An `hvcC` of 8-bit 4:2:0 with 4-byte lengths and one array of `nals`.
    pub(super) fn hvcc(nals: &[&[u8]]) -> Vec<u8> {
        let mut record = vec![1, 1, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 0x5a];
        record.extend([0xf0, 0, 0xfc, 0xfd, 0xf8, 0xf8, 0, 0, 0x0f, 1]);
        record.extend([0xa0, 0, nals.len() as u8]);
        for nal in nals {
            record.extend((nal.len() as u16).to_be_bytes());
            record.extend(*nal);
        }
        record
    }

    #[test]
    fn a_configuration_records_parameter_sets_and_sizes_are_read() {
        let config = Config::read(&hvcc(&[&[0x40, 1, 2], &[0x42, 1]])).unwrap();
        assert_eq!(config.length_size, 4);
        assert_eq!(config.depth, 8);
        assert_eq!(config.chroma, 1);
        assert_eq!(config.parameter_sets, [vec![0x40, 1, 2], vec![0x42, 1]]);
        let mut ten_bit = hvcc(&[]);
        ten_bit[17] = 0xfa;
        assert_eq!(Config::read(&ten_bit).unwrap().depth, 10);
    }

    #[test]
    fn a_configuration_record_cut_off_or_of_another_version_is_not_read() {
        let record = hvcc(&[&[0x40, 1, 2]]);
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
        // Nor one of more parameter sets than HEVC allows, however small.
        let many = vec![&[0x40, 1][..]; MAX_PARAMETER_SETS + 1];
        assert!(Config::read(&hvcc(&many[..MAX_PARAMETER_SETS])).is_some());
        assert_eq!(Config::read(&hvcc(&many)), None);
    }
}
