use super::*;
#[test]
fn matrix_signature_does_not_activate_the_legacy_x100f_profile_correction() {
    let m = Metadata {
        baseline_exposure: Some(0.),
        dng_matrix_profile_signature: Some("com.adobe".into()),
        dng_neutral_calibration: Some(crate::camera_data::NeutralCalibration {
            gains: [0.9, 1., 1.1],
            signature: "com.adobe".into(),
        }),
        ..x100f()
    };
    let unsigned = CameraProfile::camera_matrix_default(&x100f()).unwrap();
    let fallback = CameraProfile::camera_matrix_default(&m).unwrap();
    assert_eq!(
        fallback.legacy_white_balance(5600., 12., &m),
        unsigned.white_balance(5600., 12., &x100f())
    );
    assert_ne!(
        fallback.white_balance(5600., 12., &m),
        fallback.legacy_white_balance(5600., 12., &m)
    );
}

#[test]
fn calibration_upgrade_keeps_old_recipe_white_balance_continuous() {
    let m = Metadata {
        make: "Sony".into(),
        model: "ILCE-7M4".into(),
        sony_daylight_wb: Some([2456., 1024., 1691.]),
        ..x100f()
    };
    let mut p = CameraProfile::camera_matrix_default(&m).unwrap();
    p.color1 = Some(m.cam_xyz);
    p.color2 = Some(m.cam_xyz);
    p.calibration_signature.clear();
    let before = p.white_balance(5600., 0., &m).unwrap();
    p.calibration_signature = "com.adobe".into();
    let mut recipe = crate::model::recipe::Recipe {
        temperature: 5600.,
        tint: 0.,
        wb: before,
        profile: Some(std::sync::Arc::new(p)),
        ..Default::default()
    };
    recipe.update_wb(&m);
    assert_eq!(recipe.wb, before);
}
#[test]
fn a7iv_dng_neutral_solves_adobe_as_shot_and_custom_white_balance() {
    // Public raw.pixls.us A7 IV sample, independently converted with Adobe DNG
    // Converter. These are metadata measurements, not a fit to rendered pixels.
    let m = Metadata {
        make: "Sony".into(),
        model: "ILCE-7M4".into(),
        sony_daylight_wb: Some([2456., 1024., 1691.]),
        wb: [2380. / 1024., 1., 1920. / 1024.],
        ..x100f()
    };
    let mut p = CameraProfile::camera_matrix_default(&m).unwrap();
    p.color1 = Some([
        [0.8784, -0.4791, 0.1177],
        [-0.3468, 1.0693, 0.3213],
        [0.0009, 0.0507, 0.7395],
    ]);
    p.color2 = Some([
        [0.746, -0.2365, -0.0588],
        [-0.5687, 1.3442, 0.2474],
        [-0.0624, 0.1156, 0.6584],
    ]);
    p.kelvin1 = 2856.;
    p.kelvin2 = 6504.;
    p.calibration_signature = "com.adobe".into();
    let [t, tint] = p.as_shot_white_balance(&m).unwrap();
    assert!(
        (t - 4700.).abs() < 50. && (tint - 24.).abs() < 1.,
        "{t} / {tint}"
    );
    let gains = p.white_balance(t, tint, &m).unwrap();
    assert!(gains.iter().all(|v| (v - 1.).abs() < 0.0005));
    for t in [3200., 5600., 7500.] {
        let gains = p.white_balance(t, 0., &m).unwrap();
        let mut adjusted = m.clone();
        adjusted.wb = std::array::from_fn(|c| m.wb[c] * gains[c]);
        let actual = p.as_shot_white_balance(&adjusted).unwrap();
        assert!((actual[0] / t - 1.).abs() < 0.002 && actual[1].abs() < 0.15);
    }
    p.calibration_signature.clear();
    assert_eq!(p.neutral_calibration(&m), [1.; 3]);
}
#[test]
fn dng_without_calibration_does_not_inherit_a_native_camera_correction() {
    let mut m = Metadata {
        make: "Sony".into(),
        model: "ILCE-7M4".into(),
        sony_daylight_wb: Some([2456., 1024., 1691.]),
        baseline_exposure: Some(0.),
        ..x100f()
    };
    let mut p = CameraProfile::camera_matrix_default(&m).unwrap();
    p.calibration_signature = "com.adobe".into();
    assert_eq!(p.neutral_calibration(&m), [1.; 3]);
    m.dng_neutral_calibration = Some(crate::camera_data::NeutralCalibration {
        gains: [0.95, 1., 1.02],
        signature: "com.adobe".into(),
    });
    assert_eq!(p.neutral_calibration(&m), [0.95, 1., 1.02]);
    p.calibration_signature = "different".into();
    assert_eq!(p.neutral_calibration(&m), [1.; 3]);
}
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn x100f() -> Metadata {
    // X100F D65 ColorMatrix, as Adobe writes it into DNGs and LibRaw reports as cam_xyz.
    Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        wb: [2.0198677, 1., 1.8874172],
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    }
}
/// A published DCP for the tests that need a real one. None are bundled; point
/// RAWMAKASE_TEST_DCP at RawTherapee's `SONY ILCE-7M2.dcp` to run them.
fn sony_dcp() -> Vec<u8> {
    std::fs::read(std::env::var_os("RAWMAKASE_TEST_DCP").expect("Set RAWMAKASE_TEST_DCP")).unwrap()
}
#[test]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn dng_camera_neutral_matches_reference_and_roundtrips() {
    let mut p = CameraProfile::camera_matrix_default(&x100f()).unwrap();
    // Metadata from two Lightroom DNG exports, independent of rendered pixels.
    p.color1 = Some([
        [1.339, -0.731, 0.0216],
        [-0.3983, 1.1994, 0.2238],
        [-0.0435, 0.1035, 0.6328],
    ]);
    p.color2 = Some([
        [1.1434, -0.4948, -0.121],
        [-0.3746, 1.2042, 0.1903],
        [-0.0666, 0.1479, 0.5235],
    ]);
    p.calibration_signature = "com.adobe".into();
    p.kelvin1 = 2856.;
    p.kelvin2 = 6504.;
    let m = x100f();
    let [temperature, tint] = p.as_shot_white_balance(&m).unwrap();
    assert!((temperature - 5050.).abs() < 10.);
    assert!((tint - 21.).abs() < 0.2);
    let gains = p.white_balance(temperature, tint, &m).unwrap();
    assert!(gains.iter().all(|g| (g - 1.).abs() < 0.0005), "{gains:?}");
    for t in [2856., 4000., 6504., 10000.] {
        for tint in [-30., 0., 40.] {
            let gains = p.white_balance(t, tint, &m).unwrap();
            let mut adjusted = m.clone();
            adjusted.wb = std::array::from_fn(|c| m.wb[c] * gains[c]);
            let roundtrip = p.as_shot_white_balance(&adjusted).unwrap();
            assert!(
                (roundtrip[0] - t).abs() / t < 0.002,
                "{t} {tint}: {roundtrip:?}"
            );
            assert!((roundtrip[1] - tint).abs() < 0.15);
        }
    }
    p.calibration_signature.clear();
    assert_eq!(p.neutral_calibration(&m), [1.; 3]);
    // An old embedded profile has no new calibration matrices.
    p.color1 = None;
    p.color2 = None;
    assert!(p.white_balance(5000., 10., &m).is_none());
}
#[test]
fn reference_tone_preserves_neutrals_and_channel_order() {
    let mut p = CameraProfile::camera_matrix_default(&x100f()).unwrap();
    p.look = None;
    p.tone = crate::camera_profiles::dng_tone::DEFAULT_TONE
        .iter()
        .enumerate()
        .map(|(i, y)| [i as f32 / 1024., *y])
        .collect();
    let neutral = p.finish([0.18; 3], true);
    assert!(neutral.iter().all(|v| (v - 0.388).abs() < 0.004));
    let color = p.finish([0.3, 0.15, 0.05], true);
    assert!(color[0] > color[1] && color[1] > color[2]);
    assert!(color.iter().all(|v| v.is_finite()));
}

#[test]
#[ignore = "Needs a published DCP; set RAWMAKASE_TEST_DCP"]
fn published_sony_profile_parses_and_rejects_other_cameras() -> Result<()> {
    let p = from_bytes(&sony_dcp())?;
    assert!(p.copyright.contains("Dworak"));
    let m = Metadata {
        make: "Sony".into(),
        model: "ILCE-7M2".into(),
        ..Default::default()
    };
    p.ensure_camera(&m)?;
    assert!(p.ensure_camera(&Metadata::default()).is_err());
    for t in [2856., 5000., 6504.] {
        let rgb = p.camera_color([0.2; 3], p.camera_matrix(t), t);
        assert!((rgb[0] - rgb[1]).abs() < 0.01);
        assert!((rgb[1] - rgb[2]).abs() < 0.01);
    }
    Ok(())
}
#[test]
#[ignore = "Needs a published DCP; set RAWMAKASE_TEST_DCP"]
fn truncated_profiles_never_panic() {
    let b = sony_dcp();
    for n in [0, 1, 7, 8, 24, 256, 65536] {
        assert!(from_bytes(&b[..n]).is_err());
    }
}
#[test]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn camera_matrix_default_keeps_neutrals_and_uses_dng_tone() {
    let m = x100f();
    let p = CameraProfile::camera_matrix_default(&m).unwrap();
    p.ensure_camera(&m).unwrap();
    assert_eq!(p.tone.len(), 1025);
    let rgb = p.camera_color([0.18; 3], p.camera_matrix(5000.), 5000.);
    assert!(rgb.iter().all(|v| (v - 0.18).abs() < 0.001), "{rgb:?}");
    assert!(p.as_shot_white_balance(&m).is_some());
    assert!(CameraProfile::camera_matrix_default(&Metadata::default()).is_none());
}

#[test]
fn a7cr_native_units_use_their_own_daylight_metadata() {
    for (daylight, expected) in [
        ([2569., 1024., 1800.], [0.9432, 1., 0.9994]),
        ([2610., 1024., 1771.], [0.9284, 1., 1.0158]),
    ] {
        let mut m = Metadata {
            make: "Sony".into(),
            model: "ILCE-7CR".into(),
            sony_daylight_wb: Some(daylight),
            ..x100f()
        };
        let mut profile = CameraProfile::camera_matrix_default(&m).unwrap();
        profile.calibration_signature = "com.adobe".into();
        assert_eq!(profile.neutral_calibration(&m), expected);
        m.model = "A7CR".into();
        assert_eq!(profile.neutral_calibration(&m), expected);
        m.sony_daylight_wb = None;
        assert_eq!(profile.neutral_calibration(&m), [1.; 3]);
        m.sony_daylight_wb = Some([0., 1024., 1800.]);
        assert_eq!(profile.neutral_calibration(&m), [1.; 3]);
        m.sony_daylight_wb = Some(daylight);
        profile.calibration_signature.clear();
        assert_eq!(profile.neutral_calibration(&m), [1.; 3]);
    }
}

#[test]
fn every_film_look_file_is_listed_and_parses() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/looks");
    let mut on_disk: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|f| f.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".xmp"))
        .collect();
    let mut listed: Vec<_> = film::FILES.iter().map(|(p, _)| p.to_string()).collect();
    on_disk.sort();
    listed.sort();
    assert_eq!(on_disk, listed, "assets/looks and film::FILES differ");
    assert_eq!(
        film::looks().len(),
        film::FILES.len(),
        "a film look fails to parse"
    );
}

/// Film looks are listed for any camera with a colour matrix, always over RAWmakase
/// Standard, with Lightroom's Profile Amount.
#[test]
fn film_looks_go_over_rawmakase_standard() {
    let looks = film::profiles(&x100f());
    assert_eq!(looks.len(), film::FILES.len());
    let mut uuids = std::collections::BTreeSet::new();
    for p in &looks {
        assert!(p.name.starts_with("RMKS Film: "), "{}", p.name);
        let look = p.enhanced.as_ref().unwrap();
        assert_eq!(look.base_name, open::STANDARD, "{}", p.name);
        assert!(look.amount.is_some() && look.rgb().is_some(), "{}", p.name);
        assert!(uuids.insert(look.uuid.clone()), "{}", p.name);
    }
    let no_matrix = Metadata {
        cam_xyz: [[0.; 3]; 3],
        ..x100f()
    };
    assert!(film::profiles(&no_matrix).is_empty());
}

/// A film look keeps a correctly exposed gray near neutral and near mid gray: the
/// film is balanced on it, as a lab balances a print or a scan. Not exactly: the
/// models leave casts (Kodachrome's slide cool, blue 18% over red in linear light;
/// Portra's print warm, 5%), which this allows; a broken table does not pass.
#[test]
fn film_looks_keep_mid_gray_neutral() {
    let gray = 0.1842;
    for p in film::profiles(&x100f()) {
        let out = p.enhanced.as_ref().unwrap().rgb().unwrap().apply([gray; 3]);
        let (lo, hi) = (
            out.iter().copied().fold(f32::INFINITY, f32::min),
            out.iter().copied().fold(0., f32::max),
        );
        assert!((hi - lo) / gray < 0.25, "{}: {out:?}", p.name);
        assert!((out[1] / gray - 1.).abs() < 0.15, "{}: {out:?}", p.name);
    }
}
