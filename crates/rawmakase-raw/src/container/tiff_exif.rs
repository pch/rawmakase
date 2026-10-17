//! The exposure and lens facts every TIFF-based RAW keeps in its EXIF
//! directory (and, for some, IFD0).
use super::tiff::{TEXT_MAX, TiffFile};
use crate::info::RawInfo;

pub(crate) fn read(t: &TiffFile, info: &mut RawInfo) {
    for ifd in [Some(&t.ifds[0]), t.exif.as_ref()].into_iter().flatten() {
        // A rational of 0/0 is not a value.
        if let Some(v) = t.real(ifd, 33434).filter(|v| v.is_finite()) {
            info.shutter = v as f32;
        }
        // A rational of 0/0 is not a value.
        if let Some(v) = t.real(ifd, 33437).filter(|v| v.is_finite()) {
            info.aperture = v as f32;
        }
        if let Some(v) = t.int(ifd, 34855) {
            info.iso = v as f32;
        }
        // A rational of 0/0 is not a value.
        if let Some(v) = t.real(ifd, 37386).filter(|v| v.is_finite()) {
            info.focal = v as f32;
        }
        if let Some(v) = t.int(ifd, 41989) {
            info.focal_35mm = v as f32;
        }
        if let Some(v) = t
            .text_upto(ifd, 42036, TEXT_MAX)
            .filter(|v| !v.trim().is_empty())
        {
            info.lens = v;
        }
    }
    // Without an FNumber or ExposureTime, the APEX values (Exif 2.3:
    // ApertureValue = 2 log2 F, ShutterSpeedValue = −log2 t), as Adobe DNG
    // Converter derives them (Blackmagic's DNGs, Leica M's, the Sigma fp's
    // FNumber of 0).
    for ifd in [Some(&t.ifds[0]), t.exif.as_ref()].into_iter().flatten() {
        if !(info.aperture.is_finite() && info.aperture > 0.)
            && let Some(av) = t
                .real(ifd, 37378)
                .filter(|v| v.is_finite() && v.abs() < 64.)
        {
            info.aperture = 2f64.powf(av / 2.) as f32;
        }
        if !(info.shutter.is_finite() && info.shutter > 0.)
            && let Some(sv) = t
                .real(ifd, 37377)
                .filter(|v| v.is_finite() && v.abs() < 64.)
        {
            info.shutter = 2f64.powf(-sv) as f32;
        }
    }
}

/// EXIF's CFAPattern of a 2 × 2 filter: its colours (0 red, 1 green, 2
/// blue). Its dimensions are read in either byte order, as some files write
/// them in the other.
pub(crate) fn cfa_pattern(t: &TiffFile) -> Option<Vec<u8>> {
    let two = |d: &[u8]| d == [0, 2] || d == [2, 0];
    t.exif
        .as_ref()
        .and_then(|e| t.bytes_of(e, 41730))
        .filter(|p| p.len() == 8 && two(&p[..2]) && two(&p[2..4]))
        .map(|p| p[4..].to_vec())
        .filter(|p| p.iter().all(|&c| c <= 2))
}

#[cfg(test)]
mod tests {
    /// APEX values stand in for a missing FNumber and ExposureTime.
    #[test]
    fn apex_values_stand_in_for_missing_exposure_facts() {
        let srational = |n: i32, d: i32| [n.to_le_bytes(), d.to_le_bytes()].concat();
        let f = crate::container::dng::tests::dng(
            2,
            1,
            &[
                (33437, 5, 1, srational(0, 10)),
                (37377, 10, 1, srational(7, 1)),
                (37378, 5, 1, srational(6, 1)),
            ],
        );
        let info = crate::open(&f).unwrap();
        assert_eq!((info.aperture, info.shutter), (8., 1. / 128.));
    }
}
