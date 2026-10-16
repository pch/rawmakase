//! Opening a photo: LibRaw's facts about the file ([`Raw::open_file`]), then what
//! the file says beyond them and what the user's libraries add: the lens tables
//! the camera embedded, a DNG's profile, baseline exposure, colour matrix, lens
//! and default crop, and the imported lens profiles that fit. Everything that
//! develops, previews or exports a photo opens it here.
use crate::raster;
use crate::raw::Raw;
use anyhow::Result;
use rawmakase_model::camera_data::{CameraImage, Metadata};
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// The imported lens profiles, shared by everything that opens photos (Develop's
/// loaders, the Library's previews, exports, Sync Settings) so they are read once
/// each time the files change, not once per thread.
static LENS_PROFILES: crate::lens::lcp::LibraryCache = crate::lens::lcp::LibraryCache::new();

/// A photo opened for developing: a RAW still to decode, or a rendered file
/// already decoded (it has no half-size or cached stage).
pub enum Opened {
    Raw(Raw),
    Rendered(CameraImage),
}
impl Opened {
    pub fn metadata(&self) -> &Metadata {
        match self {
            Opened::Raw(raw) => &raw.metadata,
            Opened::Rendered(image) => &image.metadata,
        }
    }
}

/// Opens `path` as a RAW (LibRaw) when its extension is a RAW's, otherwise as a
/// JPEG, PNG or TIFF. A rendered file opens as the scene its default rendering is of
/// (`develop::rendered`), which Develop renders as a raw scene.
pub fn open_source(path: &Path, cancel: &AtomicBool) -> Result<Opened> {
    if rawmakase_model::storage::is_raw(path) {
        open(path).map(Opened::Raw)
    } else {
        raster::open(path, cancel)
            .map(|image| Opened::Rendered(rawmakase_engine::develop::rendered::to_scene(image)))
    }
}

/// The photo at `path`, opened for developing.
pub fn open(path: &Path) -> Result<Raw> {
    open_with(path, crate::raw::Decoder::Libraw)
}

/// The photo at `path` read by `decoder`, for the commands that compare
/// decoders; see [`crate::raw::Decoder`].
pub fn open_with(path: &Path, decoder: crate::raw::Decoder) -> Result<Raw> {
    let mut raw = Raw::open_with(path, decoder)?;
    let metadata = &mut raw.metadata;
    metadata.lens = crate::lens::embedded::read(path);
    if let Some(dng) = crate::dng::read(path) {
        // 0 is the DNG default; the camera table is for other raw formats.
        metadata.baseline_exposure = Some(dng.baseline_exposure.unwrap_or(0.));
        metadata.dng_neutral_calibration = dng.neutral_calibration;
        // A profile needs a forward matrix; one written as colour matrices alone
        // still describes the camera's colour, so keep that when it is all there is.
        let profile = dng
            .profile
            .as_deref()
            .and_then(|dcp| crate::camera_profiles::from_bytes(dcp).ok());
        let color_matrix = match profile {
            Some(_) => None,
            None => dng
                .profile
                .as_deref()
                .and_then(|dcp| crate::camera_profiles::d65_color_matrix(dcp).ok().flatten()),
        };
        if profile.is_none() && color_matrix.is_some() {
            metadata.dng_matrix_profile_signature = dng
                .profile_calibration_signature
                .filter(|signature| !signature.is_empty());
        }
        let fits = profile.is_some_and(|p| p.ensure_camera(metadata).is_ok());
        metadata.embedded_dcp = dng.profile.filter(|_| fits).map(std::sync::Arc::from);
        // LibRaw has no XYZ-to-camera matrix for a DNG from a camera it does not
        // know, so one written with colour matrices but no profile would render
        // without a profile at all. Take the file's D65 matrix then: the same
        // matrix in the same direction, so nothing downstream has to know where
        // it came from. A camera LibRaw knows keeps LibRaw's matrix.
        if metadata.embedded_dcp.is_none()
            && metadata.cam_xyz.iter().flatten().all(|v| *v == 0.)
            && let Some(matrix) = color_matrix
            && matrix.iter().flatten().any(|v| *v != 0.)
        {
            metadata.cam_xyz = matrix;
        }
        if dng.lens.is_some() {
            metadata.lens = dng.lens;
        }
        if let Some(crop) = dng.crop {
            metadata.apply_default_crop(crop);
        }
        // Exact where LibRaw rounds it to whole, even pixels.
        if dng.user_crop.is_some() {
            metadata.camera_crop = dng.user_crop.filter(|c| *c != [0., 0., 1., 1.]);
        }
    }
    metadata.lens_profiles = LENS_PROFILES.current().for_photo(metadata);
    // Pentax bodies record no lens name, only the lens ID: name the lens as the
    // profile matched by that ID does. Matched first, as the ID only counts for a
    // file without a lens name.
    if let Some(lens) = metadata.lens_profiles.lens_by_id(metadata) {
        metadata.lens_model = lens.to_string();
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    #[test]
    fn matrix_only_dng_keeps_its_profile_calibration_signature() {
        // Append a new IFD without moving the existing compressed mosaic tiles.
        let chart = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/corpus/charts/synthetic-d65-matrix-only.dng"
        );
        let original = std::fs::read(chart).unwrap();
        assert_eq!(&original[..4], b"II*\0");
        let first = u32::from_le_bytes(original[4..8].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes(original[first..first + 2].try_into().unwrap()) as usize;
        let gains = [0.9f32, 1., 1.1];
        let mut diagonal = Vec::new();
        for n in [9000i32, 0, 0, 0, 10000, 0, 0, 0, 11000] {
            diagonal.extend(n.to_le_bytes());
            diagonal.extend(10000i32.to_le_bytes());
        }
        let dir = tempfile::tempdir().unwrap();
        for (signature, calibrated) in [("test.profile", true), ("other.profile", false)] {
            let mut bytes = original.clone();
            let mut entries: Vec<Vec<u8>> = original[first + 2..first + 2 + count * 12]
                .as_chunks::<12>()
                .0
                .iter()
                .map(|entry| entry.to_vec())
                .collect();
            for (tag, kind, count, data) in [
                (50723u16, 10u16, 9u32, diagonal.clone()),
                (50724, 10, 9, diagonal.clone()),
                (50931, 2, 13, b"test.profile\0".to_vec()),
                (
                    50932,
                    2,
                    signature.len() as u32 + 1,
                    [signature.as_bytes(), b"\0"].concat(),
                ),
            ] {
                let mut entry = tag.to_le_bytes().to_vec();
                entry.extend(kind.to_le_bytes());
                entry.extend(count.to_le_bytes());
                entry.extend((bytes.len() as u32).to_le_bytes());
                bytes.extend(data);
                if !bytes.len().is_multiple_of(2) {
                    bytes.push(0);
                }
                entries.push(entry);
            }
            entries.sort_by_key(|e| u16::from_le_bytes(e[..2].try_into().unwrap()));
            let offset = (bytes.len() as u32).to_le_bytes();
            bytes[4..8].copy_from_slice(&offset);
            bytes.extend((entries.len() as u16).to_le_bytes());
            for entry in entries {
                bytes.extend(entry);
            }
            bytes.extend(0u32.to_le_bytes());
            let path = dir.path().join("matrix-only.dng");
            std::fs::write(&path, bytes).unwrap();
            let m = super::open(&path).unwrap().metadata;
            assert!(
                m.embedded_dcp.is_none(),
                "fixture must exercise the matrix-only fallback"
            );
            assert_eq!(m.dng_neutral_calibration.as_ref().unwrap().gains, gains);
            let profile = crate::camera_profiles::CameraProfile::camera_matrix_default(&m).unwrap();
            let mut identity = m.clone();
            identity.dng_neutral_calibration = None;
            let expected = profile.white_balance(5600., 12., &identity).unwrap();
            // Exercise both the unnamed fallback and the actual default profile.
            let color = std::sync::Arc::new(crate::camera_profiles::open::color(&m).unwrap());
            for profiles in [vec![], vec![color]] {
                let recipe = rawmakase_model::model::recipe::Recipe::with_profiles(&m, &profiles);
                let mut edited = recipe.clone();
                edited.temperature = 5600.;
                edited.tint = 12.;
                edited.update_wb(&m);
                let mut restored: rawmakase_model::model::recipe::Recipe =
                    serde_json::from_str(&serde_json::to_string(&edited).unwrap()).unwrap();
                restored.update_wb(&m);
                assert_eq!(restored.wb, edited.wb);
                for c in 0..3 {
                    let want = if calibrated {
                        expected[c] / gains[c]
                    } else {
                        expected[c]
                    };
                    assert!(
                        (edited.wb[c] - want).abs() < 1e-5,
                        "{signature}: {:?} expected channel {c} = {want}",
                        edited.wb
                    );
                }
            }
        }
    }

    #[test]
    fn dng_without_baseline_exposure_uses_the_dng_default() {
        let chart = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/corpus/charts/synthetic-d65.dng"
        );
        let mut bytes = std::fs::read(chart).unwrap();
        // BaselineExposure, SRATIONAL, count 1: give it an invalid type so it is unread.
        let entry = [0x2a, 0xc6, 10, 0, 1, 0, 0, 0];
        let at = bytes.windows(8).position(|w| w == entry).unwrap();
        bytes[at + 2] = 0;
        // A closed file: Windows' LibRaw cannot open one a NamedTempFile holds open.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-baseline.dng");
        std::fs::write(&path, bytes).unwrap();
        let m = super::open(&path).unwrap().metadata;
        // A camera without a table row would otherwise take the table's median.
        assert_eq!(m.baseline_exposure, Some(0.));
        assert_eq!(crate::camera_profiles::reference::baseline_exposure(&m), 0.);
    }
    /// `chart` with the IFD entries `tags` (tag, type, count, data) added, written to
    /// a closed file.
    fn chart_with(
        chart: &str,
        tags: &[(u16, u16, u32, Vec<u8>)],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let path = format!(
            "{}/../../tests/corpus/charts/{chart}",
            env!("CARGO_MANIFEST_DIR")
        );
        let original = std::fs::read(path).unwrap();
        let first = u32::from_le_bytes(original[4..8].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes(original[first..first + 2].try_into().unwrap()) as usize;
        let mut bytes = original.clone();
        let mut entries: Vec<Vec<u8>> = original[first + 2..first + 2 + count * 12]
            .as_chunks::<12>()
            .0
            .iter()
            .map(|entry| entry.to_vec())
            .collect();
        for (tag, kind, count, data) in tags {
            let mut entry = tag.to_le_bytes().to_vec();
            entry.extend(kind.to_le_bytes());
            entry.extend(count.to_le_bytes());
            entry.extend((bytes.len() as u32).to_le_bytes());
            bytes.extend(data);
            entries.push(entry);
        }
        entries.sort_by_key(|e| u16::from_le_bytes(e[..2].try_into().unwrap()));
        let offset = (bytes.len() as u32).to_le_bytes();
        bytes[4..8].copy_from_slice(&offset);
        bytes.extend((entries.len() as u16).to_le_bytes());
        for entry in entries {
            bytes.extend(entry);
        }
        bytes.extend(0u32.to_le_bytes());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(chart);
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }
    fn longs(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|n| n.to_le_bytes()).collect()
    }
    /// A DNG's DefaultUserCrop is the crop Camera Raw starts from (its in-camera
    /// aspect ratio, as Adobe DNG Converter writes it), inside the DefaultCrop frame
    /// that image space and Lightroom's crop values are relative to.
    #[test]
    fn dng_default_user_crop_is_the_starting_crop_inside_the_default_crop() {
        // DefaultCropOrigin and DefaultCropSize (LONG), DefaultUserCrop (RATIONAL top,
        // left, bottom, right, in tenths and quarters).
        let (_dir, path) = chart_with(
            "synthetic-d65.dng",
            &[
                (50719, 4, 2, longs(&[10, 10])),
                (50720, 4, 2, longs(&[900, 700])),
                (51125, 5, 4, longs(&[1, 10, 1, 4, 9, 10, 3, 4])),
            ],
        );
        let m = super::open(&path).unwrap().metadata;
        let frame = [m.crop_left, m.crop_top, m.crop_width, m.crop_height];
        assert_eq!(frame, [10, 10, 900, 700]);
        let crop = m.camera_crop.expect("the DNG's default user crop");
        for (got, want) in crop.iter().zip([0.25, 0.1, 0.75, 0.9]) {
            assert!((got - want).abs() < 1e-6, "{crop:?}");
        }
        let recipe = rawmakase_model::model::recipe::Recipe::with_profiles(&m, &[]);
        assert_eq!(recipe.crop, crop);
    }

    #[test]
    fn open_source_opens_a_png_as_rendered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.png");
        // A mid-grey.
        image::RgbImage::from_pixel(2, 2, image::Rgb([118, 118, 118]))
            .save(&path)
            .unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let raw_pixel = crate::raster::open(&path, &cancel).unwrap().pixels[0];
        match super::open_source(&path, &cancel).unwrap() {
            super::Opened::Rendered(image) => {
                assert!(image.metadata.is_rendered());
                // The pixels are the pseudo-scene of the file's linear values.
                assert_eq!(
                    image.pixels[0],
                    rawmakase_engine::develop::rendered::scene_from_rendered(raw_pixel)
                );
            }
            super::Opened::Raw(_) => panic!("a PNG must open as rendered"),
        }
    }

    #[test]
    fn open_source_sends_a_raw_path_to_libraw() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("garbage.ARW");
        std::fs::write(&path, b"not a raw file").unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let libraw = super::open(&path).err().expect("LibRaw refuses garbage");
        let opened = super::open_source(&path, &cancel)
            .err()
            .expect("LibRaw refuses garbage");
        assert_eq!(opened.to_string(), libraw.to_string());
    }

    /// A Sony DNG's DefaultCrop is its frame at any ratio, unlike an ARW's default
    /// crop, which may be the camera's aspect ratio inside the sensor's frame.
    #[test]
    fn a_sony_dng_default_crop_is_the_frame() {
        let (_dir, path) = chart_with(
            "sony-ilce-7m2-d65.dng",
            &[
                (50719, 4, 2, longs(&[20, 100])),
                (50720, 4, 2, longs(&[900, 500])),
            ],
        );
        let m = super::open(&path).unwrap().metadata;
        assert_eq!(m.make, "Sony");
        let frame = [m.crop_left, m.crop_top, m.crop_width, m.crop_height];
        assert_eq!(frame, [20, 100, 900, 500]);
        assert_eq!(m.camera_crop, None);
    }
}
