use super::*;
use rawmakase_interop::exif::Field;
use rawmakase_model::camera_data::{
    EmbeddedProfileStatus, HdrSource, HighlightTonePriority, MatrixSource, NeutralCalibration,
    Source, WhiteBalancePresets,
};
use rawmakase_model::camera_profiles::fitted;
use rawmakase_model::optics::{GainGrid, LensCorrection, Radial};
use std::sync::atomic::AtomicBool;

fn identity() -> Identity {
    Identity {
        size: 25_123_456,
        modified_ns: 1_700_000_000_123_456_789,
        prefix_hash: "0123456789abcdef".into(),
    }
}

fn radial(scale: f32) -> Radial {
    Radial::new(vec![0., 0.5, 1.2], vec![1., 1. + scale, 1. + 2. * scale]).unwrap()
}

/// A lens correction with every field set to something other than its default.
fn lens(scale: f32) -> LensCorrection {
    LensCorrection {
        source: format!("Test built-in {scale}"),
        default_on: true,
        vignetting: Some(radial(scale)),
        distortion: Some(radial(scale / 2.)),
        chromatic: Some([radial(0.001), radial(-0.001)]),
        chromatic_at_source: true,
        gain_map: Some(GainGrid {
            area: [0.1, 0.05, 0.9, 0.95],
            after_warp: true,
            points: [2, 3],
            origin: [0., 0.],
            spacing: [0.5, 0.25],
            planes: 3,
            gains: (0..18).map(|i| 1. + i as f32 / 100.).collect(),
        }),
        centre: [0.48, 0.52],
    }
}

/// Metadata with every field set to something other than its default, so a field
/// dropped on the way shows.
fn every_fact(width: u32, height: u32) -> Metadata {
    let m = Metadata {
        make: "Testmaker".into(),
        model: "T-1 Mark II".into(),
        exif_make: "TESTMAKER CORPORATION".into(),
        exif_model: "TESTMAKER T-1 MARK II".into(),
        sensor_crop: Some(1.6),
        width,
        height,
        raw_width: width + 16,
        raw_height: height + 8,
        crop_width: width.saturating_sub(1),
        crop_height: height.saturating_sub(1),
        crop_left: 1,
        crop_top: 1,
        camera_crop: Some([0.1, 0., 0.9, 1.]),
        flip: 6,
        xtrans: true,
        fuji_dynamic_range: 200,
        highlight_tone_priority: HighlightTonePriority::Enhanced,
        sraw: true,
        monochrome: true,
        fuji_exposure_shift: Some(-0.7),
        iso: 6400.,
        shutter: 1. / 250.,
        aperture: 2.8,
        focal: 35.,
        focal_35mm: 52.,
        wb: [2.1, 1., 1.6],
        daylight_wb: [2., 1., 1.5],
        wb_presets: WhiteBalancePresets {
            daylight: Some([2., 1., 1.5]),
            shade: Some([2.3, 1., 1.3]),
            d65: None,
            fine_weather: Some([1.9, 1., 1.6]),
        },
        matrix: [[1.1, -0.1, 0.], [-0.2, 1.2, 0.], [0., -0.3, 1.3]],
        cam_xyz: [[0.7, -0.2, -0.1], [-0.4, 1.2, 0.2], [-0.1, 0.2, 0.6]],
        matrix_source: MatrixSource::FileTungstenOnly,
        camera_look: Some("Velvia".into()),
        embedded_profile_status: EmbeddedProfileStatus::Usable,
        lens: Some(lens(0.2)),
        lens_stand_in: Some(lens(0.3)),
        lens_model: "FE 55mm F1.8 ZA".into(),
        lens_id: 4 << 8 | 252,
        lens_info: Some([24., 70., 2.8, 2.8]),
        adobe_names: vec!["Testmaker T1".into(), "Other Name".into()],
        baseline_exposure: Some(0.35),
        dng_neutral_calibration: Some(NeutralCalibration {
            gains: [1.01, 1., 0.99],
            signature: "com.example.calibration".into(),
        }),
        dng_analog_balance: Some([1.2, 1., 0.9]),
        dng_matrix_profile_signature: Some("matrix-signature".into()),
        source: Source::Rendered,
        hdr_source: None,
        levels: Some([512, 15000, 16383]),
        lens_profiles: Default::default(),
        lateral_ca: Default::default(),
        embedded_dcp: Some(Arc::from(&b"IIRC\x00profile bytes"[..])),
    };
    let _ = m.lateral_ca.set(Some([radial(0.0005), radial(-0.0004)]));
    m
}

fn exif() -> CameraExif {
    CameraExif {
        main: vec![
            Field::ascii(0x010f, "Testmaker"),
            Field::ascii(0x0110, "T-1"),
        ],
        exif: vec![
            Field::rational(0x829a, 1, 250),
            Field::short(0x8827, 6400),
            Field::ascii(0x9003, "2026:11:07 10:39:33"),
        ],
        gps: vec![Field::long(0x0000, 0x0203)],
    }
}

/// A full-size decode, recovered, with values from deep shadow to 64 times white.
fn original(width: u32, height: u32) -> CameraImage {
    let pixels = (0..width * height)
        .map(|i| {
            let (x, y) = ((i % width) as f32, (i / width) as f32);
            let t = (x + y * 0.37) / (width as f32 + height as f32 * 0.37);
            // −16 to +6 stops around white.
            let v = (t * 22. - 16.).exp2();
            [v * 1.7, v, v * 0.6]
        })
        .collect();
    let image = CameraImage {
        width,
        height,
        recovered: Default::default(),
        pixels,
        metadata: every_fact(width, height),
        fast: false,
        scale_factor: 1.25,
        scale_clipped: 42,
        hdr: Default::default(),
    };
    let _ = image.recovered.set(Arc::new(image.clone()));
    image
}

fn built(width: u32, height: u32, long_edge: u32) -> EditablePreview {
    build(
        &original(width, height),
        &identity(),
        Some(exif()),
        long_edge,
        Precision::Lossless,
        Cancel::NEVER,
    )
    .unwrap()
}

fn written(dir: &Path, preview: &EditablePreview) -> std::path::PathBuf {
    let path = dir.join(format!("7-{}.rmep", preview.identity().key()));
    write(&path, preview).unwrap();
    path
}

/// Everything that makes the camera image, compared exactly.
fn assert_same(a: &EditablePreview, b: &EditablePreview) {
    assert_eq!(a.identity(), b.identity());
    assert_eq!(a.exif(), b.exif());
    assert_eq!(a.header(), b.header());
    let (a, b) = (a.image(), b.image());
    assert_eq!((a.width, a.height), (b.width, b.height));
    assert_eq!(a.pixels, b.pixels);
    assert_eq!((a.fast, b.fast), (true, true));
    assert_eq!(a.scale_factor, b.scale_factor);
    assert_eq!(a.scale_clipped, b.scale_clipped);
    assert_eq!(format!("{:?}", a.metadata), format!("{:?}", b.metadata));
    assert_eq!(a.recovered.get().unwrap().pixels, b.pixels);
}

#[test]
fn pixels_and_facts_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let preview = built(300, 200, 120);
    let image = preview.image();
    assert_eq!((image.width, image.height), (120, 80));
    assert!(image.fast, "a reduced decode");
    assert_eq!(
        (image.metadata.width, image.metadata.height),
        (300, 200),
        "the original's metadata, its full size among it"
    );
    let path = written(dir.path(), &preview);
    let back = read(&path, Cancel::NEVER).unwrap();
    assert_same(&preview, &back);
    let m = &back.image().metadata;
    assert_eq!(format!("{:?}", m.lens), format!("{:?}", Some(lens(0.2))));
    assert_eq!(
        m.lateral_ca.get().unwrap().as_ref().unwrap()[0],
        radial(0.0005)
    );
    assert_eq!(
        m.embedded_dcp.as_deref(),
        Some(&b"IIRC\x00profile bytes"[..])
    );
    // Every metadata field but the lens profiles, which are matched on open.
    let original = every_fact(300, 200);
    assert_eq!(format!("{:?}", m), format!("{:?}", original));
    // Writing what was read writes the same file.
    let again = dir.path().join("again.rmep");
    write(&again, &back).unwrap();
    assert_eq!(fs_bytes(&path), fs_bytes(&again));
}

fn fs_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

/// Each pixel is the mean of the pixels it covers, and a photo already smaller than
/// the long edge keeps its size.
#[test]
fn reduced_by_area_never_enlarged() {
    let mut im = original(4, 4);
    // Four 2 × 2 blocks, whose means are 4, 0.5, 2 and 1.
    let red = [
        [1., 7., 0.5, 0.5],
        [3., 5., 0.25, 0.75],
        [2., 2., 0., 2.],
        [1., 3., 1., 1.],
    ];
    im.pixels = red.as_flattened().iter().map(|r| [*r, 0., 0.]).collect();
    im.recovered = Default::default();
    let _ = im.recovered.set(Arc::new(im.clone()));
    let small = build(
        &im,
        &identity(),
        None,
        2,
        Precision::Lossless,
        Cancel::NEVER,
    )
    .unwrap();
    let small = small.image();
    assert_eq!((small.width, small.height), (2, 2));
    for (got, want) in small.pixels.iter().zip([4., 0.5, 2., 1.]) {
        assert!((got[0] / want - 1.).abs() < 1.3e-4, "{got:?} {want}");
    }
    let same = build(
        &im,
        &identity(),
        None,
        4000,
        Precision::Lossless,
        Cancel::NEVER,
    )
    .unwrap();
    assert_eq!((same.image().width, same.image().height), (4, 4));
    assert_eq!(same.original, [4, 4]);
}

/// Values above white, to 12 stops, come back within the curve's precision.
#[test]
fn headroom_above_white_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let mut im = original(3, 2);
    im.pixels = vec![
        [1.5, 8., 64.],
        [256., 1000., 4000.],
        [1e-5, 0.18, 1.],
        [2., 2., 2.],
        [32., 1., 1.],
        [3000., 4., 0.5],
    ];
    im.recovered = Default::default();
    let _ = im.recovered.set(Arc::new(im.clone()));
    let preview = build(
        &im,
        &identity(),
        None,
        LONG_EDGE,
        Precision::Lossless,
        Cancel::NEVER,
    )
    .unwrap();
    let back = read(&written(dir.path(), &preview), Cancel::NEVER).unwrap();
    for (got, want) in back.image().pixels.iter().zip(&im.pixels) {
        for c in 0..3 {
            let error = (got[c] - want[c]).abs();
            assert!(error <= 1.3e-4 * want[c] + 4e-8, "{got:?} {want:?}");
        }
    }
}

/// Highlights are recovered when the caller hasn't, and Auto CA is measured on the
/// full-size image.
#[test]
fn an_unrecovered_original_is_recovered_and_measured() {
    let mut im = original(80, 70);
    im.recovered = Default::default();
    im.metadata.lateral_ca = Default::default();
    let preview = build(
        &im,
        &identity(),
        None,
        40,
        Precision::Lossless,
        Cancel::NEVER,
    )
    .unwrap();
    assert!(im.recovered.get().is_some());
    assert!(im.metadata.lateral_ca.get().is_some());
    assert_eq!(
        format!("{:?}", preview.image().metadata.lateral_ca.get()),
        format!("{:?}", im.metadata.lateral_ca.get())
    );
}

#[test]
fn reduced_decodes_and_hdr_light_are_refused() {
    let mut im = original(8, 8);
    im.fast = true;
    assert!(
        build(
            &im,
            &identity(),
            None,
            4,
            Precision::Lossless,
            Cancel::NEVER
        )
        .is_err()
    );
    let mut im = original(8, 8);
    im.metadata.hdr_source = Some(HdrSource {
        transfer: Default::default(),
        peak: 4.,
    });
    assert!(
        build(
            &im,
            &identity(),
            None,
            4,
            Precision::Lossless,
            Cancel::NEVER
        )
        .is_err()
    );
    assert!(
        build(
            &original(8, 8),
            &identity(),
            None,
            0,
            Precision::Lossless,
            Cancel::NEVER
        )
        .is_err()
    );
    // The codec's smallest image is 2 × 2.
    assert!(
        build(
            &original(8, 1),
            &identity(),
            None,
            8,
            Precision::Lossless,
            Cancel::NEVER
        )
        .is_err()
    );
    assert!(
        build(
            &original(80, 4),
            &identity(),
            None,
            20,
            Precision::Lossless,
            Cancel::NEVER
        )
        .is_err()
    );
}

#[test]
fn read_header_agrees_with_read() {
    let dir = tempfile::tempdir().unwrap();
    let preview = built(90, 140, 70);
    let path = written(dir.path(), &preview);
    let header = read_header(&path).unwrap();
    assert_eq!(header, read(&path, Cancel::NEVER).unwrap().header());
    assert_eq!(
        header,
        Header {
            version: 1,
            codec: Codec::LosslessJxl16,
            identity: identity(),
            original: [90, 140],
            preview: [45, 70],
        }
    );
}

/// Every truncation is refused by both readers, and every damaged byte by `read`;
/// none panics.
#[test]
fn truncated_and_damaged_files_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = written(dir.path(), &built(40, 30, 40));
    let bytes = fs_bytes(&path);
    let damaged = dir.path().join("damaged.rmep");
    let check = |b: &[u8], header_too: bool| {
        let _ = std::fs::remove_file(&damaged);
        std::fs::write(&damaged, b).unwrap();
        assert!(read(&damaged, Cancel::NEVER).is_err());
        if header_too {
            assert!(read_header(&damaged).is_err());
        }
    };
    for len in 0..bytes.len() {
        check(&bytes[..len], true);
    }
    let mut longer = bytes.clone();
    longer.push(0);
    check(&longer, true);
    // A flipped bit anywhere: the hashes catch what the structure doesn't.
    for at in 0..bytes.len() {
        let mut b = bytes.clone();
        b[at] ^= 0x10;
        check(&b, false);
    }
}

#[test]
fn a_later_version_is_refused_with_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = written(dir.path(), &built(20, 10, 20));
    let mut bytes = fs_bytes(&path);
    bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    for error in [
        read(&path, Cancel::NEVER).err().unwrap(),
        read_header(&path).err().unwrap(),
    ] {
        let message = error.to_string();
        assert!(message.contains("version 2"), "{message}");
        assert!(message.contains("newer"), "{message}");
    }
    let message = Codec::of(3).unwrap_err().to_string();
    assert!(message.contains("later release"), "{message}");
}

/// A section only a later release writes is skipped, as long as its hash holds.
#[test]
fn an_unknown_section_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let preview = built(20, 10, 20);
    let path = written(dir.path(), &preview);
    let mut bytes = fs_bytes(&path);
    let payload = b"a later fact";
    bytes.extend(b"NEWS");
    bytes.extend((payload.len() as u64).to_le_bytes());
    bytes.extend(payload);
    bytes.extend(hash(payload).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert_same(&preview, &read(&path, Cancel::NEVER).unwrap());
    assert_eq!(read_header(&path).unwrap(), preview.header());
}

/// Facts missing from a record read as their defaults; the size defaults to the
/// original's.
#[test]
fn missing_facts_take_their_defaults() {
    let (m, scale) = facts::read_metadata(&Record::parse(&[]).unwrap(), [6000, 4000]).unwrap();
    let d = Metadata::default();
    assert_eq!((m.width, m.height), (6000, 4000));
    assert_eq!(
        format!(
            "{:?}",
            Metadata {
                width: 6000,
                height: 4000,
                ..d
            }
        ),
        format!("{m:?}")
    );
    assert_eq!(
        scale,
        facts::Scale {
            factor: 1.,
            clipped: 0
        }
    );
    assert!(m.lateral_ca.get().is_none(), "not measured");
}

#[test]
fn writing_never_replaces_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let preview = built(20, 10, 20);
    let path = dir.path().join("7-key.rmep");
    std::fs::write(&path, b"someone else's").unwrap();
    let error = write(&path, &preview).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().map(|e| e.kind()),
        Some(std::io::ErrorKind::AlreadyExists),
        "{error}"
    );
    assert_eq!(fs_bytes(&path), b"someone else's");
    // No temporary file is left behind.
    let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
    assert_eq!(names.len(), 1);
    // A folder that isn't there is an error, not a file elsewhere.
    assert!(write(&dir.path().join("missing/7.rmep"), &preview).is_err());
}

#[test]
fn reading_stops_when_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let path = written(dir.path(), &built(20, 10, 20));
    let cancelled = AtomicBool::new(true);
    assert!(read(&path, Cancel::new(&cancelled)).is_err());
}

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/editable-preview/v1.rmep"
);

/// The pixels of the golden file's source: 4 × 3, from black to 4,000 times white.
fn golden_pixels() -> Vec<[f32; 3]> {
    vec![
        [0., 0., 0.],
        [1e-6, 1e-4, 0.01],
        [0.18, 0.18, 0.18],
        [1., 1., 1.],
        [2.5, 1., 0.4],
        [16., 8., 4.],
        [100., 200., 300.],
        [4000., 4096., 3.],
        [0.5, 0.25, 0.125],
        [0.001, 0.002, 0.003],
        [0.9, 0.95, 0.99],
        [1.01, 1.02, 1.03],
    ]
}

fn golden_source() -> CameraImage {
    let mut im = original(4, 3);
    im.pixels = golden_pixels();
    im.recovered = Default::default();
    let _ = im.recovered.set(Arc::new(im.clone()));
    im
}

/// A version 1 file written once and checked in: every later release must keep
/// opening it, with these pixels and facts. Never write it again; a new version
/// gets a golden file of its own.
#[test]
fn the_golden_version_1_file_opens() {
    let path = Path::new(GOLDEN);
    if std::env::var_os("RAWMAKASE_WRITE_EDITABLE_PREVIEW_GOLDEN").is_some() && !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let preview = build(
            &golden_source(),
            &identity(),
            Some(exif()),
            LONG_EDGE,
            Precision::Lossless,
            Cancel::NEVER,
        )
        .unwrap();
        write(path, &preview).unwrap();
    }
    let golden = read(path, Cancel::NEVER).expect("the golden version 1 editable preview");
    assert!(golden.camera_look().is_none(), "written before LOOK");
    assert_eq!(
        golden.header(),
        Header {
            version: 1,
            codec: Codec::LosslessJxl16,
            identity: identity(),
            original: [4, 3],
            preview: [4, 3],
        }
    );
    assert_eq!(read_header(path).unwrap(), golden.header());
    assert_eq!(golden.exif(), Some(&exif()));
    let image = golden.image();
    assert!(image.fast);
    assert_eq!((image.scale_factor, image.scale_clipped), (1.25, 42));
    assert_eq!(
        format!("{:?}", image.metadata),
        format!("{:?}", every_fact(4, 3))
    );
    for (got, want) in image.pixels.iter().zip(golden_pixels()) {
        for c in 0..3 {
            let want = want[c].min(4096.);
            assert!(
                (got[c] - want).abs() <= 1.3e-4 * want + 4e-8,
                "{got:?} {want}"
            );
        }
    }
}

/// Codes over the whole range, different in every pixel and channel.
fn noisy_code(i: usize) -> u16 {
    (rawmakase_model::storage::fnv1a(rawmakase_model::storage::FNV_OFFSET, &i.to_le_bytes()) >> 17)
        as u16
}

/// An image of exactly `codes`, standing for itself.
fn of_codes(width: u32, height: u32, code: impl Fn(usize) -> u16) -> CameraImage {
    let table = curve::table();
    let mut im = original(width, height);
    im.pixels = (0..(width * height) as usize)
        .map(|i| std::array::from_fn(|c| table[code(i * 3 + c) as usize]))
        .collect();
    im.recovered = Default::default();
    let _ = im.recovered.set(Arc::new(im.clone()));
    im
}

/// A preview the size of a real one spans several of the codec's groups (256
/// pixels) and DC groups (2048): every code comes back exactly.
#[test]
fn codes_round_trip_across_the_codecs_groups() {
    let dir = tempfile::tempdir().unwrap();
    let im = of_codes(2100, 260, noisy_code);
    let preview = build(
        &im,
        &identity(),
        None,
        2100,
        Precision::Lossless,
        Cancel::NEVER,
    )
    .unwrap();
    assert_eq!(preview.image().pixels, im.pixels);
    let back = read(&written(dir.path(), &preview), Cancel::NEVER).unwrap();
    assert!(back.image().pixels == im.pixels, "a code changed");
}

/// Nothing is written that `read` would refuse as too large.
#[test]
fn sizes_beyond_what_reads_are_refused_when_building() {
    assert!(check_sizes([16384, 4096], [60000, 40000]).is_ok());
    assert!(check_sizes([16385, 2], [16385, 2]).is_err());
    assert!(check_sizes([16384, 4097], [16384, 4097]).is_err());
    assert!(check_sizes([2560, 1707], [1 << 18 | 1, 2]).is_err());
    let im = of_codes(16, 2, noisy_code);
    assert!(
        build(
            &im,
            &identity(),
            None,
            1 << 15,
            Precision::Lossless,
            Cancel::NEVER
        )
        .is_ok()
    );
}

const GOLDEN_GROUPS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/editable-preview/v1-groups.rmep"
);

/// The golden groups file's codes: smooth, so the file stays small, over 2100 × 260
/// pixels, which are several of the codec's groups and two of its DC groups.
fn groups_code(i: usize) -> u16 {
    let (p, c) = (i / 3, i % 3);
    let (x, y) = (p % 2100, p / 2100);
    ((x * 30 + y * 3 + c * 20000) % 65536) as u16
}

/// A version 1 file of a real preview's width, written once and checked in: the
/// codec's decoder must keep reading codestreams of many groups exactly.
#[test]
fn the_golden_version_1_groups_file_opens() {
    let path = Path::new(GOLDEN_GROUPS);
    let im = of_codes(2100, 260, groups_code);
    if std::env::var_os("RAWMAKASE_WRITE_EDITABLE_PREVIEW_GOLDEN").is_some() && !path.exists() {
        let preview = build(
            &im,
            &identity(),
            None,
            2100,
            Precision::Lossless,
            Cancel::NEVER,
        )
        .unwrap();
        write(path, &preview).unwrap();
    }
    let golden = read(path, Cancel::NEVER).expect("the golden version 1 groups file");
    assert!(golden.camera_look().is_none());
    assert_eq!(golden.header().preview, [2100, 260]);
    assert!(golden.image().pixels == im.pixels, "a code changed");
}

/// A Camera Look (fitted) for [`every_fact`]'s camera, over a fitted matrix as a
/// camera without colour data has: what the default fit of such a photo makes.
fn fitted_look() -> CameraProfile {
    let m = every_fact(4, 3);
    let d50 = [0.9642, 1., 0.8249];
    let raw = [[0.6, 0.3, 0.1], [0.25, 0.7, 0.05], [0.0, 0.1, 0.9]];
    let forward = std::array::from_fn(|i| {
        let sum: f32 = raw[i].iter().sum();
        std::array::from_fn(|j| raw[i][j] * d50[i] / sum)
    });
    let base = fitted::matrix_base(&m, forward).unwrap();
    let mut look = fitted::FittedLook::identity();
    // Something other than the identity, still small once compressed.
    for node in look.table.iter_mut().step_by(7) {
        *node = [4., 1.05, 0.98];
    }
    fitted::with_look(&base, &look).unwrap()
}

fn with_look(mut preview: EditablePreview) -> EditablePreview {
    preview.set_camera_look(&fitted_look()).unwrap();
    preview
}

/// The default Camera Look written with an editable preview comes back whole, and
/// a file without one reads as before.
#[test]
fn the_camera_look_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let preview = with_look(built(30, 20, 30));
    assert_eq!(preview.camera_look(), Some(&fitted_look()));
    let path = written(dir.path(), &preview);
    let back = read(&path, Cancel::NEVER).unwrap();
    assert_same(&preview, &back);
    assert_eq!(back.camera_look(), Some(&fitted_look()));
    assert_eq!(read_header(&path).unwrap(), preview.header());
    let again = dir.path().join("again.rmep");
    write(&again, &back).unwrap();
    assert_eq!(fs_bytes(&path), fs_bytes(&again));
    let plain = built(30, 20, 30);
    let path = dir.path().join("plain.rmep");
    write(&path, &plain).unwrap();
    assert!(read(&path, Cancel::NEVER).unwrap().camera_look().is_none());
}

/// A look that wouldn't read back (another camera's) is not kept.
#[test]
fn a_look_that_would_not_read_back_is_refused() {
    let mut preview = with_look(built(30, 20, 30));
    let mut other = every_fact(4, 3);
    other.model = "Other".into();
    let base = fitted::matrix_base(&other, [[0.4, 0.4, 0.2], [0.2, 0.7, 0.1], [0., 0.1, 0.8]]);
    let foreign = fitted::with_look(&base.unwrap(), &fitted::FittedLook::identity()).unwrap();
    assert!(preview.set_camera_look(&foreign).is_err());
    assert!(preview.camera_look().is_none(), "the earlier one goes too");
}

/// A `LOOK` section whose hash holds but whose profile doesn't read (a later
/// carried version, or one altered before it was hashed) refuses the file, saying
/// why: the photo must not quietly render with other colours.
#[test]
fn a_camera_look_that_does_not_read_refuses_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = written(dir.path(), &with_look(built(20, 10, 20)));
    let mut bytes = fs_bytes(&path);
    let at = find(&bytes, b"LOOK");
    let len = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize;
    let payload = at + 12;
    let version = payload + find(&bytes[payload..payload + len], b"v1:");
    bytes[version + 1] = b'9';
    let hash = hash(&bytes[payload..payload + len]);
    bytes[payload + len..payload + len + 8].copy_from_slice(&hash.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let message = format!("{:#}", read(&path, Cancel::NEVER).err().unwrap());
    assert!(message.contains("Camera Look"), "{message}");
    // A later carried version says that updating opens it.
    assert!(message.contains("update RAWmakase to open it"), "{message}");
    // Its structure is sound: the header reads.
    assert!(read_header(&path).is_ok());
}

/// A later release's look says that updating opens it; one that is damaged
/// doesn't.
#[test]
fn a_camera_look_from_a_later_release_is_told_apart() {
    let damaged = anyhow::anyhow!("The fitted profile does not match its UUID");
    assert!(look_from_a_later_release("v2:abc", &damaged));
    assert!(look_from_a_later_release("v10:abc", &damaged));
    assert!(!look_from_a_later_release("v1:abc", &damaged));
    assert!(!look_from_a_later_release("abc", &damaged));
    // A field this release doesn't know, as serde refuses it.
    let unknown = serde_json::from_str::<CameraProfile>(r#"{"later_field": 1}"#).unwrap_err();
    assert!(look_from_a_later_release(
        "v1:abc",
        &anyhow::Error::new(unknown)
    ));
    let malformed = serde_json::from_str::<CameraProfile>("{").unwrap_err();
    assert!(!look_from_a_later_release(
        "v1:abc",
        &anyhow::Error::new(malformed)
    ));
}

fn find(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .unwrap()
}

const GOLDEN_LOOK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/editable-preview/v1-camera-look.rmep"
);

/// A version 1 file with a Camera Look, written once and checked in: every later
/// release must keep opening it with its look. Never write it again.
#[test]
fn the_golden_version_1_camera_look_file_opens() {
    let path = Path::new(GOLDEN_LOOK);
    if std::env::var_os("RAWMAKASE_WRITE_EDITABLE_PREVIEW_GOLDEN").is_some() && !path.exists() {
        let preview = build(
            &golden_source(),
            &identity(),
            Some(exif()),
            LONG_EDGE,
            Precision::Lossless,
            Cancel::NEVER,
        )
        .unwrap();
        write(path, &with_look(preview)).unwrap();
    }
    let golden = read(path, Cancel::NEVER).expect("the golden version 1 Camera Look file");
    // Read, its look passed `fitted::from_carried`: of a known version, whole (its
    // UUID its content's hash) and for the photo's camera. What it holds is
    // frozen here as written, not as today's fit would make it.
    let look = golden.camera_look().expect("its Camera Look");
    assert_eq!(look.name, "Camera Look (fitted)");
    assert_eq!(look.camera, "Testmaker T-1 Mark II");
    let enhanced = look.enhanced.as_ref().unwrap();
    assert_eq!(enhanced.uuid, "F0369FC82A0BCFD409E2E1F8C00CF909");
    assert_eq!(enhanced.camera_look, None);
    look.ensure_camera(&golden.image().metadata).unwrap();
    let plain = read(Path::new(GOLDEN), Cancel::NEVER).unwrap();
    assert_same(&plain, &golden);
    assert_eq!(read_header(path).unwrap(), plain.header());
}

/// What a version 1 reader from before `LOOK` makes of the golden file with one:
/// it skips the section as one it doesn't know (its tag renamed here to one no
/// release knows, which this reader skips the same way), and reads the rest
/// exactly as the file without it.
#[test]
fn a_reader_from_before_the_camera_look_skips_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = fs_bytes(Path::new(GOLDEN_LOOK));
    let at = find(&bytes, b"LOOK");
    bytes[at..at + 4].copy_from_slice(b"ZZZZ");
    let path = dir.path().join("old-reader.rmep");
    std::fs::write(&path, &bytes).unwrap();
    let skipped = read(&path, Cancel::NEVER).unwrap();
    assert!(skipped.camera_look().is_none());
    let plain = read(Path::new(GOLDEN), Cancel::NEVER).unwrap();
    assert_same(&plain, &skipped);
    assert_eq!(read_header(&path).unwrap(), plain.header());
}

// The Smaller choice: 12-bit codes on curve 1, dithered when built (codec 2).

/// One and a half of the curve's 12-bit steps (0.41% of the value plus the toe,
/// where the curve turns linear): the triangular dither moves a value at most one
/// step, and the bucket's middle is half a step from its edges.
const SMALLER_TOLERANCE: f32 = 0.0062;

/// Whether `got` is within [`SMALLER_TOLERANCE`] of `want`.
fn within_smaller(got: f32, want: f32) -> bool {
    (got - want).abs() <= SMALLER_TOLERANCE * (want + curve::TOE as f32)
}

fn smaller(im: &CameraImage, long_edge: u32) -> EditablePreview {
    build(
        im,
        &identity(),
        Some(exif()),
        long_edge,
        Precision::Smaller,
        Cancel::NEVER,
    )
    .unwrap()
}

#[test]
fn lossless_is_the_default_precision() {
    assert_eq!(Precision::default(), Precision::Lossless);
    assert_eq!(Codec::LosslessJxl16.precision(), Precision::Lossless);
    assert_eq!(Codec::DitheredJxl12.precision(), Precision::Smaller);
    assert_eq!(built(30, 20, 30).header().codec, Codec::LosslessJxl16);
}

/// A Smaller preview reads back exactly as built, within the 12-bit precision of
/// the lossless one, and its header says which it is.
#[test]
fn a_smaller_preview_round_trips_within_its_precision() {
    let dir = tempfile::tempdir().unwrap();
    let im = original(300, 200);
    let small = smaller(&im, 120);
    let lossless = built(300, 200, 120);
    assert_eq!(small.header().codec, Codec::DitheredJxl12);
    let path = written(dir.path(), &small);
    let back = read(&path, Cancel::NEVER).unwrap();
    assert_same(&small, &back);
    assert_eq!(read_header(&path).unwrap().codec, Codec::DitheredJxl12);
    for (a, b) in back.image().pixels.iter().zip(&lossless.image().pixels) {
        for c in 0..3 {
            assert!(within_smaller(a[c], b[c]), "{a:?} {b:?}");
        }
    }
    // Writing what was read writes the same file.
    let again = dir.path().join("again.rmep");
    write(&again, &back).unwrap();
    assert_eq!(fs_bytes(&path), fs_bytes(&again));
    // And it is smaller than the lossless file.
    let lossless_path = dir.path().join("lossless.rmep");
    write(&lossless_path, &lossless).unwrap();
    assert!(fs_bytes(&path).len() < fs_bytes(&lossless_path).len());
}

/// The same photo always builds the same file: the dither is seeded by position.
#[test]
fn the_dither_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let im = original(200, 150);
    let a = written(dir.path(), &smaller(&im, 200));
    let b = dir.path().join("b.rmep");
    write(&b, &smaller(&im, 200)).unwrap();
    assert_eq!(fs_bytes(&a), fs_bytes(&b));
}

/// Over a flat area the dithered codes average to the value, where plain 12-bit
/// rounding would be off by up to half a step everywhere.
#[test]
fn the_dither_keeps_flat_areas_unbiased() {
    let table = curve::table();
    for v in [0.0123f32, 0.18, 0.7, 3.3] {
        // Values that fall between two 12-bit buckets' middles.
        let at = curve::position(v);
        let mut im = original(64, 64);
        im.pixels = vec![[v; 3]; 64 * 64];
        im.recovered = Default::default();
        let _ = im.recovered.set(Arc::new(im.clone()));
        let p = smaller(&im, 64);
        let mean = p
            .image()
            .pixels
            .iter()
            .map(|p| f64::from(p[1]))
            .sum::<f64>()
            / 4096.;
        let plain = table[(((at / 16.).floor() as usize) << 4) | 8];
        let error = (mean / f64::from(v) - 1.).abs();
        let plain_error = (f64::from(plain) / f64::from(v) - 1.).abs();
        assert!(
            error < 3e-4 && error < plain_error.max(3e-4),
            "{v}: dithered {error}, plain {plain_error}"
        );
    }
}

/// Values above white, to 12 stops, survive the Smaller choice within its precision.
#[test]
fn a_smaller_preview_keeps_headroom() {
    let dir = tempfile::tempdir().unwrap();
    let mut im = original(3, 2);
    im.pixels = vec![
        [1.5, 8., 64.],
        [256., 1000., 4000.],
        [0.01, 0.18, 1.],
        [2., 2., 2.],
        [32., 1., 1.],
        [3000., 4., 0.5],
    ];
    im.recovered = Default::default();
    let _ = im.recovered.set(Arc::new(im.clone()));
    let back = read(
        &written(dir.path(), &smaller(&im, LONG_EDGE)),
        Cancel::NEVER,
    )
    .unwrap();
    for (got, want) in back.image().pixels.iter().zip(&im.pixels) {
        for c in 0..3 {
            assert!(within_smaller(got[c], want[c]), "{got:?} {want:?}");
        }
    }
}

/// `file` with its HEAD codec field set to `code` and the section's hash made good
/// again: what a release with yet another codec would write.
fn with_codec(file: &[u8], code: u8) -> Vec<u8> {
    let mut b = file.to_vec();
    let len = u64::from_le_bytes(b[16..24].try_into().unwrap()) as usize;
    let payload = 24..24 + len;
    // The codec is HEAD's first field: id 1, length 1, its value.
    assert_eq!(&b[24..30], &[1, 0, 1, 0, 0, 0]);
    b[30] = code;
    let h = hash(&b[payload.clone()]);
    b[payload.end..payload.end + 8].copy_from_slice(&h.to_le_bytes());
    b
}

/// A codec this release doesn't know is refused by both readers, saying a later
/// release reads it; the release before codec 2 refused codec 2 the same way
/// (`Codec::of` at version 1's first release).
#[test]
fn an_unknown_codec_is_refused_with_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = written(dir.path(), &smaller(&original(20, 10), 20));
    let unknown = dir.path().join("codec3.rmep");
    std::fs::write(&unknown, with_codec(&fs_bytes(&path), 3)).unwrap();
    for error in [
        read(&unknown, Cancel::NEVER).err().unwrap(),
        read_header(&unknown).err().unwrap(),
    ] {
        let message = error.to_string();
        assert!(
            message.contains("codec 3") && message.contains("later release"),
            "{message}"
        );
    }
    // A lossless file's codestream relabelled as codec 2 holds codes beyond 12
    // bits, which is damage, not a preview.
    let lossless = dir.path().join("lossless.rmep");
    write(&lossless, &built(20, 10, 20)).unwrap();
    let relabelled = dir.path().join("relabelled.rmep");
    std::fs::write(&relabelled, with_codec(&fs_bytes(&lossless), 2)).unwrap();
    assert!(read(&relabelled, Cancel::NEVER).is_err());
}

const GOLDEN_SMALLER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/editable-preview/v1-smaller.rmep"
);

/// A version 1 Smaller file written once and checked in: every later release must
/// keep opening codec 2 with these pixels and facts.
#[test]
fn the_golden_version_1_smaller_file_opens() {
    let path = Path::new(GOLDEN_SMALLER);
    if std::env::var_os("RAWMAKASE_WRITE_EDITABLE_PREVIEW_GOLDEN").is_some() && !path.exists() {
        write(path, &smaller(&golden_source(), LONG_EDGE)).unwrap();
    }
    let golden = read(path, Cancel::NEVER).expect("the golden version 1 Smaller file");
    assert_eq!(
        golden.header(),
        Header {
            version: 1,
            codec: Codec::DitheredJxl12,
            identity: identity(),
            original: [4, 3],
            preview: [4, 3],
        }
    );
    assert_eq!(read_header(path).unwrap(), golden.header());
    let image = golden.image();
    assert!(image.fast);
    assert_eq!((image.scale_factor, image.scale_clipped), (1.25, 42));
    // The file's 12-bit codes, exactly, each read as the middle of its 16 codes.
    let codes: [[u16; 3]; 12] = [
        [0, 0, 0],
        [0, 84, 920],
        [1626, 1625, 1626],
        [2048, 2047, 2047],
        [2273, 2048, 1822],
        [2730, 2560, 2388],
        [3180, 3353, 3451],
        [4089, 4095, 2318],
        [1877, 1707, 1536],
        [400, 546, 637],
        [2022, 2035, 2046],
        [2050, 2053, 2055],
    ];
    let table = curve::table();
    let read_as: Vec<[f32; 3]> = codes
        .iter()
        .map(|p| p.map(|q| table[(usize::from(q) << 4) + 8]))
        .collect();
    assert_eq!(image.pixels, read_as);
    assert_eq!(golden.exif(), Some(&exif()));
    assert_eq!(
        format!("{:?}", golden.image().metadata),
        format!("{:?}", every_fact(4, 3))
    );
    for (got, want) in golden.image().pixels.iter().zip(golden_pixels()) {
        for c in 0..3 {
            let want = want[c].min(4096.);
            assert!(within_smaller(got[c], want), "{got:?} {want}");
        }
    }
}

/// The Smaller choice keeps a Camera Look as the lossless one does: `LOOK` and
/// codec 2 are independent sections of the same layout.
#[test]
fn a_smaller_editable_preview_keeps_its_camera_look() {
    let dir = tempfile::tempdir().unwrap();
    let preview = with_look(smaller(&original(30, 20), 30));
    let back = read(&written(dir.path(), &preview), Cancel::NEVER).unwrap();
    assert_eq!(back.header().codec.precision(), Precision::Smaller);
    assert_eq!(back.camera_look(), Some(&fitted_look()));
    assert_same(&preview, &back);
}
