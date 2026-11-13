use super::*;
#[test]
fn a7ii_native_neutral_matches_adobe_as_shot() {
    // Anonymous native RAW metadata and Adobe DNG Converter 18.7 calibration.
    // Two daylight presets exercise the per-file correction, not a fixed gain.
    for (daylight_blue, calibration_blue) in [(1500., 0.9707), (1520., 0.9579)] {
        let m = Metadata {
            make: "Sony".into(),
            model: "ILCE-7M2".into(),
            wb_presets: daylight_preset([2412., 1024., daylight_blue]),
            wb: [2.453125, 1., 1.3945312],
            ..x100f()
        };
        let mut p = CameraProfile::camera_matrix_default(&m).unwrap();
        p.color1 = Some([
            [0.7491, -0.3027, 0.0351],
            [-0.4231, 1.1579, 0.3031],
            [-0.0818, 0.1721, 0.7466],
        ]);
        p.color2 = Some([
            [0.5271, -0.0712, -0.0347],
            [-0.6153, 1.3653, 0.2763],
            [-0.1601, 0.2366, 0.7242],
        ]);
        p.kelvin1 = 2856.;
        p.kelvin2 = 6504.;
        p.calibration_signature = "com.adobe".into();
        assert_eq!(p.neutral_calibration(&m), [1.01, 1., calibration_blue]);
        let actual = p.as_shot_white_balance(&m).unwrap();
        if daylight_blue == 1500. {
            assert!((actual[0] - 5420.98).abs() < 1., "{actual:?}");
            assert!((actual[1] - 4.4767).abs() < 0.01, "{actual:?}");
        }
        let gains = p.white_balance(actual[0], actual[1], &m).unwrap();
        assert!(gains.iter().all(|v| (v - 1.).abs() < 0.0005));
        let mut dng = m;
        dng.baseline_exposure = Some(0.35);
        dng.dng_neutral_calibration = Some(crate::camera_data::NeutralCalibration {
            gains: [1.01, 1., calibration_blue],
            signature: "com.adobe".into(),
        });
        assert_eq!(actual, p.as_shot_white_balance(&dng).unwrap());
    }
}

#[test]
fn a7iv_dng_neutral_solves_adobe_as_shot_and_custom_white_balance() {
    // Public raw.pixls.us A7 IV sample, independently converted with Adobe DNG
    // Converter. These are metadata measurements, not a fit to rendered pixels.
    let m = Metadata {
        make: "Sony".into(),
        model: "ILCE-7M4".into(),
        wb_presets: daylight_preset([2456., 1024., 1691.]),
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
        wb_presets: daylight_preset([2456., 1024., 1691.]),
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
fn daylight_preset(v: [f32; 3]) -> crate::camera_data::WhiteBalancePresets {
    crate::camera_data::WhiteBalancePresets {
        daylight: Some(v),
        ..Default::default()
    }
}
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn x100f() -> Metadata {
    // X100F D65 ColorMatrix, as Adobe writes it into DNGs and LibRaw reports as cam_xyz.
    Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        wb: [2.0198677, 1., 1.8874172],
        // This unit's stored D65 preset gives the calibration in Lightroom's DNGs.
        wb_presets: crate::camera_data::WhiteBalancePresets {
            d65: Some([681., 302., 484.]),
            ..Default::default()
        },
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
    let neutral = p.finish([0.18; 3]);
    assert!(neutral.iter().all(|v| (v - 0.388).abs() < 0.004));
    let color = p.finish([0.3, 0.15, 0.05]);
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
            wb_presets: daylight_preset(daylight),
            ..x100f()
        };
        let mut profile = CameraProfile::camera_matrix_default(&m).unwrap();
        profile.calibration_signature = "com.adobe".into();
        assert_eq!(profile.neutral_calibration(&m), expected);
        m.model = "A7CR".into();
        assert_eq!(profile.neutral_calibration(&m), expected);
        m.wb_presets.daylight = None;
        assert_eq!(profile.neutral_calibration(&m), [1.; 3]);
        m.wb_presets.daylight = Some([0., 1024., 1800.]);
        assert_eq!(profile.neutral_calibration(&m), [1.; 3]);
        m.wb_presets.daylight = Some(daylight);
        profile.calibration_signature.clear();
        assert_eq!(profile.neutral_calibration(&m), [1.; 3]);
    }
}
#[test]
fn phone_module_profiles_fit_the_module_that_took_the_photo() {
    // Camera Raw's picks, checked against its CameraProfileDigest for raw.pixls.us DNGs.
    let fits = |make: &str, model: &str, focal_35mm: f32, lens: &str, camera: &str| {
        let m = Metadata {
            make: make.into(),
            model: model.into(),
            focal_35mm,
            lens_model: lens.into(),
            ..x100f()
        };
        let mut p = CameraProfile::camera_matrix_default(&m).unwrap();
        p.camera = camera.into();
        p.ensure_camera(&m).is_ok()
    };
    let pixel = |f, lens, module: &str| {
        fits(
            "Google",
            "Pixel 7 Pro",
            f,
            lens,
            &format!("Google Pixel 7 Pro {module} Camera"),
        )
    };
    assert!(pixel(117., "", "Telephoto"));
    assert!(!pixel(117., "", "Wide"));
    assert!(pixel(24., "", "Wide"));
    assert!(!pixel(24., "", "Front"));
    assert!(pixel(13., "", "Ultrawide"));
    assert!(pixel(24., "Pixel 7 Pro front camera 2.7mm f/2.2", "Front"));
    assert!(!pixel(0., "", "Wide"));
    // Pixel 4 XL at 43 mm took its Rear Telephoto profile; Pixel 6 Pro its Rear Main.
    assert!(fits(
        "Google",
        "Pixel 4 XL",
        43.,
        "",
        "Google Pixel 4 XL Rear Telephoto Camera"
    ));
    assert!(fits(
        "Google",
        "Pixel 6 Pro",
        24.,
        "",
        "Google Pixel 6 Pro Rear Main Camera"
    ));
    let galaxy = |model: &str, f, camera: &str| fits("samsung", model, f, "", camera);
    assert!(galaxy(
        "Galaxy S23 Ultra",
        230.,
        "Samsung Galaxy S23 Ultra Rear Super Telephoto Camera"
    ));
    assert!(!galaxy(
        "Galaxy S23 Ultra",
        230.,
        "Samsung Galaxy S23 Ultra Rear Telephoto Camera"
    ));
    assert!(galaxy(
        "Galaxy S23 Ultra",
        69.,
        "Samsung Galaxy S23 Ultra Rear Telephoto Camera"
    ));
    // Another phone whose name extends this one's is not a module.
    assert!(!galaxy(
        "Galaxy S23",
        23.,
        "Samsung Galaxy S23 Ultra Rear Wide Camera"
    ));
    assert!(galaxy(
        "Galaxy S23",
        23.,
        "Samsung Galaxy S23 Rear Wide Camera"
    ));
    // Nor is any other suffix.
    assert!(!fits("Ricoh", "GXR", 28., "", "Ricoh GXR MOUNT A12"));
}

#[test]
fn profiles_fit_under_the_names_adobe_uses() {
    let fits =
        |make: &str, model: &str, normalized: (&str, &str), focal_35mm: f32, camera: &str| {
            let m = Metadata {
                make: make.into(),
                model: model.into(),
                adobe_names: crate::cameras::adobe_names(make, model, normalized),
                focal_35mm,
                ..x100f()
            };
            let mut p = CameraProfile::camera_matrix_default(&m).unwrap();
            p.camera = camera.into();
            p.ensure_camera(&m).is_ok()
        };
    // Spaces and underscores.
    assert!(fits(
        "Nikon",
        "Z 6_2",
        ("Nikon", "Z 6_2"),
        0.,
        "Nikon Z 6 2"
    ));
    assert!(fits(
        "Nikon",
        "Z50_2",
        ("Nikon", "Z50_2"),
        0.,
        "Nikon Z 50 2"
    ));
    assert!(fits(
        "Fujifilm",
        "GFX100RF",
        ("Fujifilm", "GFX100RF"),
        0.,
        "Fujifilm GFX 100RF"
    ));
    assert!(!fits("Nikon", "Z 6_2", ("Nikon", "Z 6_2"), 0., "Nikon Z 6"));
    // LibRaw's canonical name for a regional twin.
    assert!(fits(
        "Panasonic",
        "DC-G95",
        ("Panasonic", "DC-G99"),
        0.,
        "Panasonic DC-G99"
    ));
    assert!(fits(
        "Panasonic",
        "DC-GX880",
        ("Panasonic", "DC-GF10"),
        0.,
        "Panasonic DC-GF10"
    ));
    assert!(!fits(
        "Panasonic",
        "DC-G95",
        ("Panasonic", "DC-G95"),
        0.,
        "Panasonic DC-G99"
    ));
    // Twins LibRaw names as themselves, which Camera Raw profiles as the G99 (#664).
    for model in ["DC-G97", "DC-G95D", "DC-G99D", "DC-G99M2"] {
        assert!(
            fits(
                "Panasonic",
                model,
                ("Panasonic", model),
                0.,
                "Panasonic DC-G99"
            ),
            "{model}"
        );
    }
    // The make Adobe spells otherwise, and the table.
    assert!(fits(
        "OM Digital",
        "OM-5",
        ("OM Digital", "OM-5"),
        0.,
        "OM Digital Solutions OM-5"
    ));
    assert!(fits(
        "Olympus",
        "E-M10MarkIIIS",
        ("Olympus", "E-M10MarkIIIS"),
        0.,
        "Olympus E-M10 Mark III"
    ));
    assert!(fits(
        "Samsung",
        "SM-G965F",
        ("Samsung", "SM-G965F"),
        0.,
        "Samsung Galaxy S9+"
    ));
    assert!(!fits(
        "Samsung",
        "SM-G965F",
        ("Samsung", "SM-G965F"),
        0.,
        "Samsung Galaxy S9"
    ));
    // Older Google phones, and a phone's module under its Adobe name.
    assert!(fits(
        "Google",
        "Pixel 2 XL",
        ("Google", "Pixel 2 XL"),
        0.,
        "Pixel 2 XL-Google-google"
    ));
    assert!(fits(
        "Huawei",
        "VOG-L29",
        ("Huawei", "VOG-L29"),
        27.,
        "HUAWEI P30 Pro Rear Main Camera"
    ));
    assert!(!fits(
        "Huawei",
        "VOG-L29",
        ("Huawei", "VOG-L29"),
        125.,
        "HUAWEI P30 Pro Rear Main Camera"
    ));
    // Huawei's "Rear Wide" is its ultrawide module; Google's "Wide" is the main one.
    let huawei = |focal, camera| fits("Huawei", "VOG-L29", ("Huawei", "VOG-L29"), focal, camera);
    assert!(!huawei(27., "HUAWEI P30 Pro Rear Wide Camera"));
    assert!(huawei(16., "HUAWEI P30 Pro Rear Wide Camera"));
    assert!(fits(
        "Google",
        "Pixel 7 Pro",
        ("Google", "Pixel 7 Pro"),
        24.,
        "Google Pixel 7 Pro Wide Camera"
    ));
}

#[test]
fn the_embedded_profile_renders_with_camera_raws_default_tone_curve() {
    use crate::camera_data::Source;
    for m in [
        Metadata {
            source: Source::Rendered,
            ..Default::default()
        },
        Metadata {
            source: Source::Rendered,
            make: "Apple".into(),
            model: "iPhone 15".into(),
            ..Default::default()
        },
    ] {
        let p = CameraProfile::embedded(&m);
        assert!(p.is_embedded());
        assert!(p.ensure_camera(&m).is_ok());
        for v in [
            [0.0, 0.0, 0.0],
            [0.18, 0.18, 0.18],
            [0.9, 0.2, 0.05],
            [1.0, 1.0, 1.0],
        ] {
            // camera_color, as pixel.rs runs S1: the colour is linear ProPhoto as it is.
            let rgb = p.camera_color(v, p.camera_matrix(6500.), 6500.);
            let pro = mul(RGB_TO_PRO, rgb);
            for c in 0..3 {
                assert!((pro[c] - v[c]).abs() < 1e-5, "{v:?} -> {pro:?}");
            }
        }
        // finish converts to the working RGB, which leaves greys grey.
        let out = mul(RGB_TO_PRO, p.finish([0.5; 3]));
        let want = crate::camera_profiles::default_tone_at(0.5);
        for c in 0..3 {
            assert!((out[c] - want).abs() < 1e-5, "{out:?} vs {want}");
        }
    }
}

#[test]
fn linear_dng_analog_balance_scales_the_camera_neutral() {
    // Samples stored with AnalogBalance 2, 1, ½ (and `wb` of them): a white
    // set by temperature balances them as the profile's neutral times those
    // gains, and the as-shot temperature is the same as without them.
    let plain = Metadata {
        dng_analog_balance: None,
        ..x100f()
    };
    let p = CameraProfile::camera_matrix_default(&plain).unwrap();
    let gains = [2., 1., 0.5];
    let stored = Metadata {
        wb: std::array::from_fn(|c| plain.wb[c] / gains[c]),
        dng_analog_balance: Some(gains),
        ..plain.clone()
    };
    let as_shot = p.as_shot_white_balance(&plain).unwrap();
    let with = p.as_shot_white_balance(&stored).unwrap();
    assert!((as_shot[0] - with[0]).abs() < 1. && (as_shot[1] - with[1]).abs() < 0.01);
    // The balanced samples are the same: stored samples (plain ones times the
    // gains) times their `wb` (plain over the gains) are the plain ones times
    // theirs, so the white's gains on top must be the same too.
    let a = p.white_balance(5500., 0., &plain).unwrap();
    let b = p.white_balance(5500., 0., &stored).unwrap();
    for c in 0..3 {
        assert!((a[c] - b[c]).abs() < 1e-4, "{a:?} {b:?}");
    }
}

/// A look whose curve is the identity applies no curve in HDR, as Camera Raw 18.7
/// renders a `.cube` look's: light above four stops over white is not held at 16, as
/// a curve that ends at white holds it.
#[test]
fn an_identity_look_curve_leaves_hdr_light_above_sixteen() {
    let base = CameraProfile::camera_matrix_default(&x100f()).unwrap();
    let identity = base
        .clone()
        .with_test_look(1, |_| 1., &[[0., 0.], [1., 1.]]);
    assert_eq!(identity.hdr_look(), None);
    let tone = identity.hdr_tone().unwrap();
    let plain = base.finish_hdr([40.; 3], &tone, None);
    let looked = identity.finish_hdr([40.; 3], &tone, identity.hdr_look().as_ref());
    assert!(
        looked
            .iter()
            .zip(plain)
            .all(|(a, b)| (a - b).abs() < 1e-3 * b),
        "{looked:?} {plain:?}"
    );
    assert!(looked.iter().all(|v| *v > 20.), "{looked:?}");
    let lifting = base.with_test_look(1, |_| 1., &[[0., 0.], [0.5, 0.6], [1., 1.]]);
    assert!(lifting.hdr_look().is_some());
}

/// An ordinary profile's sRGB-encoded look table renders in HDR as in SDR below white,
/// and keeps light above it (Camera Raw 18.7's HDR render of the Leica D-Lux 7's Adobe
/// Standard); an HDR profile's looks up the sRGB encoding of the overrange-encoded
/// value (its render of Nikon's Camera Standard HDR on the Z5 II and Z50 II,
/// docs/records/hdr-stage1-results.md#srgb-encoded-tables).
#[test]
fn srgb_encoded_tables_render_in_hdr() {
    let profile = CameraProfile::camera_matrix_default(&x100f())
        .unwrap()
        .with_test_srgb_look();
    let tone = profile.hdr_tone().unwrap();
    let below = [0.2, 0.15, 0.1];
    let hdr = profile.finish_hdr(below, &tone, None);
    assert!(hdr.iter().all(|v| v.is_finite() && *v > 0.), "{hdr:?}");
    let bright = profile.finish_hdr([6., 5., 4.], &tone, None);
    assert!(
        bright.iter().all(|v| v.is_finite() && *v > 2.),
        "{bright:?}"
    );
    let hdr = profile.as_hdr_for_test();
    let tone = hdr.hdr_tone().unwrap();
    let plain = CameraProfile {
        look: None,
        ..hdr.clone()
    };
    for p in [below, [6., 5., 4.]] {
        let table = hdr.look.as_ref().unwrap();
        // The table's own lookup (of the sRGB-encoded value) on the overrange encoding,
        // then the rest of the finish.
        let looked = table
            .apply(p.map(overrange::encode), None, 0.)
            .map(overrange::decode);
        let expected = plain.finish_hdr(looked, &tone, None);
        let got = hdr.finish_hdr(p, &tone, None);
        assert!(
            got.iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() <= 1e-4 * b.abs().max(1.)),
            "{p:?}: {got:?} {expected:?}"
        );
    }
}
