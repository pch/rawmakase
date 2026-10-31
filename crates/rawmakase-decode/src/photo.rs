//! Opening a photo: the RAW decoder's facts about the file ([`Raw::open_file`]), then what
//! the file says beyond them and what the user's libraries add: the lens tables
//! the camera embedded, a DNG's profile, baseline exposure, colour matrix, lens
//! and default crop, and the imported lens profiles that fit. Everything that
//! develops, previews or exports a photo opens it here; the Library's
//! thumbnails, which need only the file's facts and its embedded preview,
//! use [`Raw::open_file`] and `raw::thumbnail_reaching`.
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

/// Opens `path` as a RAW when its extension is a RAW's, otherwise as a
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

/// The camera's EXIF of the photo at `path`, which exports carry and the
/// catalog's defaults give way to: `exif::read`'s, else, for a CR3, MRW or
/// X3F, what RAWmakase's RAW decoders find in the container
/// (`raw::camera_exif`). `None` when the file has none, e.g. a CRW.
pub fn camera_exif(path: &Path) -> Option<rawmakase_interop::exif::CameraExif> {
    rawmakase_interop::exif::read(path).or_else(|| crate::raw::camera_exif(path))
}

/// The XMP a camera embeds in a RAW the catalog can't read it from itself,
/// a CR3's (an in-camera rating, title or description), found by
/// RAWmakase's RAW decoders; nothing decoded. `None` for other files.
pub fn raw_xmp(path: &Path) -> Option<Vec<u8>> {
    crate::raw::embedded_xmp(path)
}

/// The photo at `path`, opened for developing.
pub fn open(path: &Path) -> Result<Raw> {
    let mut raw = Raw::open_file(path)?;
    let metadata = &mut raw.metadata;
    let builtin = crate::lens::embedded::read_builtin(path, metadata);
    (metadata.lens, metadata.lens_stand_in) = (builtin.lens, builtin.stand_in);
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
        let fits = profile
            .as_ref()
            .is_some_and(|p| p.ensure_camera(metadata).is_ok());
        metadata.embedded_profile_status = embedded_profile_status(
            dng.profile.is_some() || dng.profile_unreadable,
            profile.is_some(),
            fits,
            color_matrix.is_some(),
        );
        metadata.embedded_dcp = dng.profile.filter(|_| fits).map(std::sync::Arc::from);
        // A DNG the decoder found no XYZ-to-camera matrix for, written with
        // colour matrices but no profile, would render without a profile at
        // all. Take the file's D65 matrix then: the same matrix in the same
        // direction, so nothing downstream has to know where it came from. A
        // camera the decoder has a matrix for keeps it.
        if metadata.embedded_dcp.is_none()
            && metadata.cam_xyz.iter().flatten().all(|v| *v == 0.)
            && let Some(matrix) = color_matrix
            && matrix.iter().flatten().any(|v| *v != 0.)
        {
            metadata.cam_xyz = matrix;
            metadata.matrix_source = rawmakase_model::camera_data::MatrixSource::File;
        }
        if dng.lens.is_some() {
            metadata.lens = dng.lens;
            metadata.lens_stand_in = None;
        }
        if let Some(crop) = dng.crop {
            metadata.apply_default_crop(crop);
        }
        // Exact where the decoder rounds it to whole, even pixels.
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

/// What a DNG's embedded profile tags are to its photo: none, a profile that
/// parses and fits the camera, one for another camera, colour matrices alone, or
/// tags that are neither (an error that must not read as no profile).
fn embedded_profile_status(
    present: bool,
    parses: bool,
    fits: bool,
    matrix_only: bool,
) -> rawmakase_model::camera_data::EmbeddedProfileStatus {
    use rawmakase_model::camera_data::EmbeddedProfileStatus as S;
    match (present, parses, fits, matrix_only) {
        (false, ..) => S::None,
        (true, true, true, _) => S::Usable,
        (true, true, false, _) => S::OtherCamera,
        (true, false, _, true) => S::MatrixOnly,
        (true, false, _, false) => S::Unreadable,
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn an_embedded_profile_is_told_from_none_and_from_an_unreadable_one() {
        use rawmakase_model::camera_data::{EmbeddedProfileStatus as S, MatrixSource};
        let chart = |name: &str| {
            let path = format!(
                "{}/../../tests/corpus/charts/{name}.dng",
                env!("CARGO_MANIFEST_DIR")
            );
            super::open(std::path::Path::new(&path)).unwrap().metadata
        };
        let named = chart("synthetic-d65");
        assert_eq!(named.embedded_profile_status, S::Usable);
        let matrix = chart("synthetic-d65-matrix-only");
        assert_eq!(matrix.embedded_profile_status, S::MatrixOnly);
        assert_eq!(matrix.matrix_source, MatrixSource::File);
        // The chart's IFD0 entries rewritten by `patch` (tag, then the entry's 12
        // bytes), in a copy.
        let dir = tempfile::tempdir().unwrap();
        let patched = |name: &str, patch: &dyn Fn(u16, &mut [u8])| {
            let path = format!(
                "{}/../../tests/corpus/charts/synthetic-d65.dng",
                env!("CARGO_MANIFEST_DIR")
            );
            let mut bytes = std::fs::read(path).unwrap();
            let first = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
            let count = u16::from_le_bytes(bytes[first..first + 2].try_into().unwrap()) as usize;
            for i in 0..count {
                let at = first + 2 + i * 12;
                let tag = u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
                patch(tag, &mut bytes[at..at + 12]);
            }
            let out = dir.path().join(name);
            std::fs::write(&out, bytes).unwrap();
            super::open(&out).unwrap().metadata
        };
        // A profile tag whose value lies past the file's end: unreadable, not absent.
        let unreadable = patched("unreadable.dng", &|tag, entry| {
            if tag == 50936 {
                entry[8..].copy_from_slice(&0x7fff_ff00u32.to_le_bytes());
            }
        });
        assert_eq!(unreadable.embedded_profile_status, S::Unreadable);
        // No colour tags at all (UniqueCameraModel alone): no profile.
        let colourless = patched("colourless.dng", &|tag, entry| {
            if [50721, 50722, 50938, 50939, 50964, 50965, 50982].contains(&tag) {
                entry[..2].copy_from_slice(&(tag + 10000).to_le_bytes());
            }
        });
        assert_eq!(colourless.embedded_profile_status, S::None);
        // Tags that are there but say nothing usable are not "no profile".
        assert_eq!(
            super::embedded_profile_status(true, false, false, false),
            S::Unreadable
        );
        assert_eq!(
            super::embedded_profile_status(true, true, false, false),
            S::OtherCamera
        );
        assert_eq!(
            super::embedded_profile_status(false, false, false, false),
            S::None
        );
    }
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

    /// A little-endian monochrome DNG (linear raw of one sample, a Leica M
    /// Monochrom's levels) of `samples`, `width` a row, written to a closed file.
    fn monochrome_dng(width: u32, samples: &[u16]) -> (tempfile::TempDir, std::path::PathBuf) {
        let height = samples.len() as u32 / width;
        let short = |v: u16| v.to_le_bytes().to_vec();
        let long = |v: u32| v.to_le_bytes().to_vec();
        let mut entries: Vec<(u16, u16, u32, Vec<u8>)> = vec![
            (254, 4, 1, long(0)),
            (256, 4, 1, long(width)),
            (257, 4, 1, long(height)),
            (258, 3, 1, short(16)),
            (259, 3, 1, short(1)),
            (262, 3, 1, short(34892)),
            (271, 2, 6, b"Leica\0".to_vec()),
            (272, 2, 12, b"M Monochrom\0".to_vec()),
            (273, 4, 1, Vec::new()),
            (277, 3, 1, short(1)),
            (278, 4, 1, long(height)),
            (279, 4, 1, long(samples.len() as u32 * 2)),
            (50706, 1, 4, vec![1, 4, 0, 0]),
            (50714, 3, 1, short(220)),
            (50717, 3, 1, short(16383)),
        ];
        let ifd_len = 2 + 12 * entries.len() + 4;
        let mut data_at = 8 + ifd_len;
        let mut data = Vec::new();
        for (tag, _, _, value) in &mut entries {
            if *tag == 273 {
                *value = long((data_at + data.len()) as u32);
                data.extend(samples.iter().flat_map(|v| v.to_le_bytes()));
            }
        }
        let mut bytes = b"II*\0".to_vec();
        bytes.extend(long(8));
        bytes.extend(short(entries.len() as u16));
        let mut values: Vec<u8> = Vec::new();
        data_at += data.len();
        for (tag, kind, count, value) in &entries {
            bytes.extend(short(*tag));
            bytes.extend(short(*kind));
            bytes.extend(long(*count));
            if value.len() <= 4 {
                bytes.extend(value.iter().copied().chain([0; 4]).take(4));
            } else {
                bytes.extend(long((data_at + values.len()) as u32));
                values.extend(value);
            }
        }
        bytes.extend(long(0));
        bytes.extend(data);
        bytes.extend(values);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("monochrome.dng");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    /// A monochrome DNG develops as grey, its one sample every channel's (a
    /// sample below black as 0), and
    /// renders grey whatever the white balance: through the default profile
    /// and RAWmakase Color, with Temp and Tint changed (which leave its gains
    /// at 1, as it has no white balance), an eyedropper's pick, and gains an
    /// edit of another photo brought.
    #[test]
    fn monochrome_dngs_develop_and_render_grey() {
        use rawmakase_model::model::recipe::Recipe;
        // 120 lies below the black level: noise there develops as 0.
        let (_dir, path) = monochrome_dng(4, &[220, 2236, 8301, 16383, 1000, 4000, 12000, 120]);
        let raw = super::open(&path).unwrap();
        let m = raw.metadata.clone();
        assert!(m.monochrome);
        assert_eq!((m.wb, m.daylight_wb), ([1.; 3], [1.; 3]));
        let im = raw
            .develop(
                rawmakase_model::camera_data::Decode::Half,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!((im.width, im.height), (4, 2));
        for (p, v) in im
            .pixels
            .iter()
            .zip([220, 2236, 8301, 16383, 1000, 4000, 12000, 120])
        {
            let want = ((v as f32 - 220.) / (16383. - 220.)).max(0.);
            assert_eq!(*p, [want; 3]);
        }
        let color = std::sync::Arc::new(crate::camera_profiles::open::color(&m).unwrap());
        for profiles in [vec![], vec![color]] {
            let start = Recipe::with_profiles(&m, &profiles);
            let mut warmer = start.clone();
            warmer.temperature = 3000.;
            warmer.tint = 40.;
            warmer.update_wb(&m);
            assert_eq!(warmer.wb, [1.; 3]);
            let mut picked = start.clone();
            picked.pick_white_balance([1.5, 1., 0.6], &m);
            assert_eq!(picked.wb, [1.; 3]);
            let mut synced = start.clone();
            synced.wb = [1.5, 1., 0.6];
            for r in [start, warmer, picked, synced] {
                let out = rawmakase_engine::develop::render(&im, &r.checked().unwrap(), 0).unwrap();
                for p in &out.pixels {
                    assert!(
                        (p[0] - p[1]).abs() < 1e-3 && (p[2] - p[1]).abs() < 1e-3,
                        "{p:?} with {:?}",
                        r.wb
                    );
                }
                assert!(out.pixels.iter().any(|p| p[1] > 0.5));
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
        // A closed file, written and closed before it is opened.
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
    fn open_source_opens_webp_avif_and_jxl_as_rendered() {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        for name in ["metadata.webp", "ten-bit.avif", "metadata.jxl"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/data/formats")
                .join(name);
            match super::open_source(&path, &cancel).unwrap() {
                super::Opened::Rendered(image) => assert!(image.metadata.is_rendered(), "{name}"),
                super::Opened::Raw(_) => panic!("{name} must open as rendered"),
            }
        }
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
    fn open_source_sends_a_raw_path_to_the_raw_decoder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("garbage.ARW");
        std::fs::write(&path, b"not a raw file").unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let raw = super::open(&path)
            .err()
            .expect("the decoder refuses garbage");
        let opened = super::open_source(&path, &cancel)
            .err()
            .expect("the decoder refuses garbage");
        assert_eq!(opened.to_string(), raw.to_string());
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
