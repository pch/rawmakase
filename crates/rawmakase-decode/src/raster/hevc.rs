//! A HEIF item's HEVC configuration (`hvcC`), read from the header: the depth
//! and chroma the system's decoder will give.

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
}

impl Config {
    /// The record in `hvcc`; `None` when it is shorter than its fixed part,
    /// not version 1 or over 16 KiB.
    pub(super) fn read(hvcc: &[u8]) -> Option<Self> {
        if hvcc.len() < 23 || hvcc.len() > MAX_CONFIG || hvcc[0] != 1 {
            return None;
        }
        Some(Self {
            depth: (hvcc[17] & 7) + 8,
            chroma: hvcc[16] & 3,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `hvcC` of 8-bit 4:2:0 and `extra` bytes of parameter sets.
    fn hvcc(extra: usize) -> Vec<u8> {
        let mut record = vec![1, 1, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 0x5a];
        record.extend([0xf0, 0, 0xfc, 0xfd, 0xf8, 0xf8, 0, 0, 0x0f, 0]);
        record.resize(record.len() + extra, 0x4e);
        record
    }

    #[test]
    fn a_configuration_records_depth_and_chroma_are_read() {
        let config = Config::read(&hvcc(0)).unwrap();
        assert_eq!((config.depth, config.chroma), (8, 1));
        let mut ten_bit = hvcc(0);
        ten_bit[17] = 0xfa;
        assert_eq!(Config::read(&ten_bit).unwrap().depth, 10);
    }

    #[test]
    fn a_configuration_record_cut_off_or_of_another_version_is_not_read() {
        let record = hvcc(0);
        assert_eq!(Config::read(&record[..22]), None);
        let mut version = record;
        version[0] = 2;
        assert_eq!(Config::read(&version), None);
        // Nor is one larger than parameter sets and SEI take.
        assert!(Config::read(&hvcc(MAX_CONFIG - 23)).is_some());
        assert_eq!(Config::read(&hvcc(MAX_CONFIG - 22)), None);
    }
}
