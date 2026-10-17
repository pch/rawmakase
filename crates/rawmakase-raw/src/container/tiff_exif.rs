//! The exposure and lens facts every TIFF-based RAW keeps in its EXIF
//! directory (and, for some, IFD0).
use super::tiff::TiffFile;
use crate::info::RawInfo;

pub(crate) fn read(t: &TiffFile, info: &mut RawInfo) {
    for ifd in [Some(&t.ifds[0]), t.exif.as_ref()].into_iter().flatten() {
        if let Some(v) = t.real(ifd, 33434) {
            info.shutter = v as f32;
        }
        if let Some(v) = t.real(ifd, 33437) {
            info.aperture = v as f32;
        }
        if let Some(v) = t.int(ifd, 34855) {
            info.iso = v as f32;
        }
        if let Some(v) = t.real(ifd, 37386) {
            info.focal = v as f32;
        }
        if let Some(v) = t.int(ifd, 41989) {
            info.focal_35mm = v as f32;
        }
        if let Some(v) = t.text(ifd, 42036).filter(|v| !v.trim().is_empty()) {
            info.lens = v;
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
        .and_then(|e| t.raw(e, 41730))
        .filter(|p| p.len() == 8 && two(&p[..2]) && two(&p[2..4]))
        .map(|p| p[4..].to_vec())
        .filter(|p| p.iter().all(|&c| c <= 2))
}
