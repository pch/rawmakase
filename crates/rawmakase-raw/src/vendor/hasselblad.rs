//! Hasselblad's maker note (3FR and FFF): a TIFF directory in the EXIF
//! MakerNote, offsets from the TIFF header. RAWmakase reads the sensor code
//! (0x0011) and the sensor coating (0x0016), which name the camera's colour
//! as RAWmakase has always named it: the sensor's megapixels, the code of
//! the newer sensors, the coating, a second generation (" II" in the
//! model), and the file's kind (FFF, Phocus's, or the camera's 3FR):
//! "50-15-Coated5-3FR".
//!
//! Sources: our own measurements on the test dataset's files: which tags
//! hold the sensor code and coating, and the megapixels and coating names
//! the earlier decoder (LibRaw) gave each value (`compat-observed`; Adobe
//! Camera Raw's profiles use the same names without the file's kind).
use crate::container::tiff::TiffFile;

const SENSOR_CODE: u16 = 0x0011;
const COATING: u16 = 0x0016;

/// The megapixels of a sensor by its code, as observed.
const MEGAPIXELS: [(u32, u32); 9] = [
    (4, 16),
    (6, 22),
    (9, 39),
    (11, 50),
    (12, 60),
    (13, 40),
    (15, 50),
    (17, 100),
    (20, 100),
];

/// What the maker note says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Notes {
    pub sensor: Option<u32>,
    pub coating: Option<u32>,
}

/// The maker note of `t`, when it has one.
pub(crate) fn read(t: &TiffFile) -> Notes {
    let note = t
        .exif
        .as_ref()
        .and_then(|e| e.entries.get(&0x927c))
        .and_then(|e| t.offset_of(e))
        .and_then(|at| t.ifd_at(at));
    let Some(ifd) = note else {
        return Notes::default();
    };
    Notes {
        sensor: t.int(&ifd, SENSOR_CODE),
        coating: t.int(&ifd, COATING),
    }
}

impl Notes {
    /// The sensor's megapixels, when its code is known.
    pub(crate) fn megapixels(&self) -> Option<u32> {
        let code = self.sensor?;
        MEGAPIXELS.iter().find(|(c, _)| *c == code).map(|(_, m)| *m)
    }
    /// The colour's name for a file of `model` (as RAWmakase names it),
    /// FFF or 3FR, when the sensor and coating are known.
    pub(crate) fn colour_name(&self, model: &str, fff: bool) -> Option<String> {
        let megapixels = self.megapixels()?;
        let coating = match self.coating? {
            1 => "Uncoated",
            4 => "Coated",
            5 => "Coated5",
            6 => "Coated6",
            _ => return None,
        };
        let mut name = megapixels.to_string();
        let code = self.sensor?;
        if code >= 15 {
            name += &format!("-{code}");
        }
        name += "-";
        name += coating;
        if model.contains(" II") {
            name += "-II";
        }
        name += if fff { "-FFF" } else { "-3FR" };
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_colour_by_sensor_coating_and_kind() {
        let n = |sensor, coating| Notes {
            sensor: Some(sensor),
            coating: Some(coating),
        };
        assert_eq!(
            n(15, 5).colour_name("X1D-50c", false).as_deref(),
            Some("50-15-Coated5-3FR")
        );
        assert_eq!(
            n(15, 5).colour_name("X1D II 50C", false).as_deref(),
            Some("50-15-Coated5-II-3FR")
        );
        assert_eq!(
            n(13, 5).colour_name("H5D-40", true).as_deref(),
            Some("40-Coated5-FFF")
        );
        assert_eq!(
            n(4, 1).colour_name("CFV-16", false).as_deref(),
            Some("16-Uncoated-3FR")
        );
        // Codes and coatings not seen are not named.
        assert_eq!(n(3, 5).colour_name("X", false), None);
        assert_eq!(n(15, 2).colour_name("X", false), None);
        assert_eq!(Notes::default().colour_name("X", true), None);
    }
}
