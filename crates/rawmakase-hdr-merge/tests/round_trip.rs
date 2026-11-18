//! A merged DNG written by [`rawmakase_hdr_merge::encode`] opens through the
//! app's decoder (`rawmakase_decode::photo::open`) with the samples and facts
//! it was written with.
use rawmakase_cancel::Cancel;
use rawmakase_hdr_merge::{Calibration, DngFacts, Error, LinearImage, MakerNote, encode};
use rawmakase_interop::exif::{CameraExif, Field, tag};
use rawmakase_model::camera_data::Decode;
use std::sync::atomic::AtomicBool;

/// Three tiles across and two down, the last of each partly outside.
const W: u32 = 600;
const H: u32 = 300;
const WHITE: u32 = 32768;

/// A 24-stop ramp across the image, its colour changing down it.
fn pixels() -> Vec<[f32; 3]> {
    (0..H)
        .flat_map(|y| {
            (0..W).map(move |x| {
                let v = WHITE as f32 * 2f32.powf(-24. * x as f32 / (W - 1) as f32);
                let t = y as f32 / H as f32;
                [v, v * (0.5 + 0.25 * t), v * (1. - 0.5 * t)]
            })
        })
        .collect()
}

/// The top-left 40 × 20 pixels uncovered, a soft edge beside them.
fn coverage() -> Vec<u8> {
    (0..H)
        .flat_map(|y| {
            (0..W).map(move |x| match (x, y) {
                (0..40, 0..20) => 0,
                (40..44, 0..20) => 128,
                _ => 255,
            })
        })
        .collect()
}

/// A Fujifilm maker note of one tag, DevelopmentDynamicRange (0x1403) = 200:
/// "FUJIFILM", the little-endian offset of its directory, the directory.
fn fujifilm_note() -> Vec<u8> {
    let mut n = b"FUJIFILM".to_vec();
    n.extend(12u32.to_le_bytes());
    n.extend(1u16.to_le_bytes());
    n.extend(0x1403u16.to_le_bytes());
    n.extend(3u16.to_le_bytes());
    n.extend(1u32.to_le_bytes());
    n.extend([200, 0, 0, 0]);
    n.extend(0u32.to_le_bytes());
    n
}

const D65: [f64; 9] = [
    1.1434, -0.4699, -0.0377, -0.3305, 1.1357, 0.2208, -0.0349, 0.1066, 0.6046,
];

/// The unit's calibration, as Adobe DNG Converter writes it for both
/// illuminants.
const CALIBRATION: [f64; 9] = [0.9785, 0., 0., 0., 1., 0., 0., 0., 1.0018];

fn facts() -> DngFacts {
    let exif = vec![
        Field::rational(tag::EXPOSURE_TIME, 1, 20),
        Field::rational(tag::F_NUMBER, 11, 1),
        Field::short(tag::ISO, 100),
        Field::ascii(tag::DATE_TIME_ORIGINAL, "2023:09:13 06:41:07"),
        Field::rational(tag::FOCAL_LENGTH, 23, 1),
        Field::ascii(tag::LENS_MODEL, "XF16-80mmF4 R OIS WR"),
        // Left out: the maker note goes in DNGPrivateData.
        Field {
            tag: tag::MAKER_NOTE,
            kind: 7,
            count: 4,
            bytes: vec![1, 2, 3, 4],
        },
    ];
    DngFacts {
        make: "FUJIFILM".into(),
        model: "X-T4".into(),
        unique_camera_model: "Fujifilm X-T4".into(),
        software: "RAWmakase test".into(),
        white_level: WHITE,
        baseline_exposure: 1.54,
        calibrations: vec![
            Calibration {
                illuminant: 17,
                color_matrix: [
                    1.3426, -0.6334, -0.0177, -0.4244, 1.2136, 0.2371, -0.0844, 0.1922, 0.7097,
                ],
                forward_matrix: None,
                camera_calibration: Some(CALIBRATION),
            },
            Calibration {
                illuminant: 21,
                color_matrix: D65,
                forward_matrix: Some([
                    0.4386, 0.3874, 0.1383, 0.1599, 0.7541, 0.0860, 0.0405, 0.0004, 0.7842,
                ]),
                camera_calibration: Some(CALIBRATION),
            },
        ],
        calibration_signature: Some("com.adobe".into()),
        analog_balance: None,
        as_shot_neutral: [0.5, 1., 0.625],
        orientation: 6,
        default_crop: Some([8, 4, 584, 290]),
        exif: CameraExif {
            main: vec![
                Field::ascii(tag::MAKE, "Ignored"),
                Field::ascii(tag::ARTIST, "lphilpot"),
            ],
            exif,
            gps: Vec::new(),
        },
        maker_note: Some(MakerNote {
            bytes: fujifilm_note(),
            little_endian: true,
            offset: 1234,
        }),
    }
}

fn write(pixels: &[[f32; 3]], coverage: Option<&[u8]>, facts: &DngFacts) -> tempfile::TempPath {
    let image = LinearImage {
        width: W,
        height: H,
        pixels,
        coverage,
    };
    let bytes = encode(&image, facts, Cancel::NEVER).unwrap();
    let file = tempfile::Builder::new().suffix(".dng").tempfile().unwrap();
    std::fs::write(file.path(), bytes).unwrap();
    file.into_temp_path()
}

/// The samples come back as they were written, to half precision, through
/// the tiles, the predictor and the levels; and so do the facts.
#[test]
fn a_merged_dng_opens_with_what_it_was_written_with() {
    let pixels = pixels();
    let coverage = coverage();
    let facts = facts();
    let path = write(&pixels, Some(&coverage), &facts);
    let raw = rawmakase_decode::photo::open(&path).unwrap();
    let m = raw.metadata.clone();
    assert_eq!((m.make.as_str(), m.model.as_str()), ("Fujifilm", "X-T4"));
    assert_eq!((m.width, m.height), (W, H));
    assert_eq!(
        [m.crop_left, m.crop_top, m.crop_width, m.crop_height],
        [8, 4, 584, 290]
    );
    assert_eq!(m.flip, 6);
    assert!((m.baseline_exposure.unwrap() - 1.54).abs() < 1e-6);
    assert_eq!((m.iso, m.aperture, m.focal), (100., 11., 23.));
    assert!((m.shutter - 0.05).abs() < 1e-7);
    assert_eq!(m.lens_model, "XF16-80mmF4 R OIS WR");
    assert_eq!(m.fuji_dynamic_range, 200);
    // As shot: the neutral's inverse, relative to green.
    for (got, want) in m.wb.iter().zip([2., 1., 1.6]) {
        assert!((got - want).abs() < 1e-5, "{:?}", m.wb);
    }
    // Linear raw's colour is the file's: its D65 ColorMatrix.
    for (got, want) in m.cam_xyz.iter().flatten().zip(D65) {
        assert!((f64::from(*got) - want).abs() < 1e-5, "{:?}", m.cam_xyz);
    }
    let calibration = m.dng_neutral_calibration.as_ref().unwrap();
    assert_eq!(calibration.signature, "com.adobe");
    assert_eq!(calibration.gains, [0.9785, 1., 1.0018]);
    let covered = m.coverage.as_ref().unwrap();
    assert_eq!((covered.width, covered.height), (W, H));
    assert_eq!(covered.values, coverage);

    let exif = rawmakase_interop::exif::read(&path).unwrap();
    assert_eq!(exif.captured().as_deref(), Some("2023:09:13 06:41:07"));
    let text = |t| exif.get(t).and_then(Field::text);
    assert_eq!(text(tag::ARTIST).as_deref(), Some("lphilpot"));
    assert_eq!(text(tag::MAKE).as_deref(), Some("FUJIFILM"));
    assert!(exif.get(tag::MAKER_NOTE).is_none());

    let wb = m.wb;
    let image = raw
        .develop(Decode::Full(Default::default()), Cancel::NEVER)
        .unwrap();
    assert_eq!((image.width, image.height), (W, H));
    for (i, (got, written)) in image.pixels.iter().zip(&pixels).enumerate() {
        for c in 0..3 {
            let stored = half::f16::from_f32(written[c]).to_f32();
            let want = stored / WHITE as f32 * wb[c];
            assert!(
                (got[c] - want).abs() <= want * 1e-6,
                "pixel {i} channel {c}: {} for {want}",
                got[c]
            );
        }
    }
}

/// Without coverage the file has no transparency mask; without a maker note,
/// no DNGPrivateData.
#[test]
fn coverage_and_maker_note_are_optional() {
    let mut facts = facts();
    facts.maker_note = None;
    let path = write(&pixels(), None, &facts);
    let raw = rawmakase_decode::photo::open(&path).unwrap();
    assert!(raw.metadata.coverage.is_none());
    // Unknown, as for a file without the note.
    assert_eq!(raw.metadata.fuji_dynamic_range, 65535);
}

/// A set flag stops encoding; inputs that break the contract are refused.
#[test]
fn cancelled_and_invalid_inputs_write_nothing() {
    let pixels = pixels();
    let image = LinearImage {
        width: W,
        height: H,
        pixels: &pixels,
        coverage: None,
    };
    let flag = AtomicBool::new(true);
    assert_eq!(
        encode(&image, &facts(), Cancel::new(&flag)),
        Err(Error::Cancelled)
    );
    let short = LinearImage {
        height: H + 1,
        ..image
    };
    assert!(matches!(
        encode(&short, &facts(), Cancel::NEVER),
        Err(Error::Invalid(_))
    ));
    for change in [
        |f: &mut DngFacts| f.white_level = 70000,
        |f: &mut DngFacts| f.calibrations.clear(),
        |f: &mut DngFacts| f.orientation = 9,
        |f: &mut DngFacts| f.default_crop = Some([590, 0, 20, 10]),
        |f: &mut DngFacts| f.as_shot_neutral = [0.5, 0., 0.6],
        |f: &mut DngFacts| f.as_shot_neutral[2] = f64::NAN,
        |f: &mut DngFacts| f.analog_balance = Some([1., -1., 1.]),
        |f: &mut DngFacts| f.calibrations[0].color_matrix[4] = f64::INFINITY,
        |f: &mut DngFacts| f.calibrations[1].forward_matrix = Some([f64::NAN; 9]),
        |f: &mut DngFacts| f.baseline_exposure = f64::NAN,
    ] {
        let mut facts = facts();
        change(&mut facts);
        let image = LinearImage {
            width: W,
            height: H,
            pixels: &pixels,
            coverage: None,
        };
        assert!(matches!(
            encode(&image, &facts, Cancel::NEVER),
            Err(Error::Invalid(_))
        ));
    }
}
