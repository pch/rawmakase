use super::*;
use crate::develop::curve::ToneCurve;
use crate::develop::effects::EffectsRendering;
use rawmakase_cancel::Cancel;
fn adjust(p: [f32; 3], m: &Metadata, r: &Recipe) -> [f32; 3] {
    process_pixel(p, r, &CurveSet::new(r), camera_stage(m, r), [0., 0.], None)
}
fn fixture() -> CameraImage {
    let m = Metadata {
        width: 12,
        height: 8,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    CameraImage {
        recovered: Default::default(),
        width: 12,
        height: 8,
        pixels: (0..96)
            .map(|i| [0.02 + i as f32 / 200., 0.1 + i as f32 / 500., 0.04])
            .collect(),
        metadata: m,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }
}
/// A rendered file of one row of 8-bit sRGB pixels as `photo::open_source` gives it:
/// the pseudo-scene of these sRGB pixels, decoded to linear ProPhoto RGB.
pub(crate) fn rendered_fixture(srgb: &[[u8; 3]]) -> CameraImage {
    use rawmakase_model::camera_data::Source;
    use rawmakase_model::camera_profiles::RGB_TO_PRO;
    let decode = |v: u8| {
        let v = v as f32 / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let width = srgb.len() as u32;
    crate::develop::rendered::to_scene(CameraImage {
        recovered: Default::default(),
        width,
        height: 1,
        pixels: srgb
            .iter()
            .map(|p| crate::color::mul(RGB_TO_PRO, p.map(decode)))
            .collect(),
        metadata: Metadata {
            width,
            height: 1,
            source: Source::Rendered,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    })
}
/// An untouched edit of a rendered file reproduces it (on the CPU): deep shadows, mid
/// tones, saturated colors and near-white, where highlight recovery and the scene
/// tone stage's toe and default black would otherwise act.
#[test]
fn a_rendered_file_at_its_defaults_renders_as_it_is() -> anyhow::Result<()> {
    let mut srgb: Vec<[u8; 3]> = (0..=8).map(|v| [v, v, v]).collect();
    srgb.extend([
        [64, 64, 64],
        [118, 118, 118],
        [200, 30, 40],
        [20, 160, 60],
        [30, 60, 220],
    ]);
    srgb.extend((248..=255).map(|v| [v, v, v]));
    srgb.extend([[255, 250, 240], [255, 255, 200]]);
    let im = rendered_fixture(&srgb);
    let r = Recipe::for_rendered(&im.metadata);
    let out = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
    let mut off = Vec::new();
    for (got, want) in out.rgb8().chunks(3).zip(&srgb) {
        if (0..3).any(|c| (got[c] as i16 - want[c] as i16).abs() > 1) {
            off.push((*want, [got[0], got[1], got[2]]));
        }
    }
    assert!(off.is_empty(), "{off:?}");
    Ok(())
}
/// A grey ramp, every 8-bit sRGB level, as a rendered file.
fn grey_ramp() -> CameraImage {
    let srgb: Vec<[u8; 3]> = (0..=255u8).map(|v| [v; 3]).collect();
    rendered_fixture(&srgb)
}
fn rendered_levels(im: &CameraImage, r: &Recipe) -> anyhow::Result<Vec<[u8; 3]>> {
    let out = crate::develop::render(im, &r.checked()?, 0, Cancel::NEVER)?;
    Ok(out.rgb8().chunks(3).map(|p| [p[0], p[1], p[2]]).collect())
}
fn near(got: [u8; 3], want: [u8; 3], tolerance: i16) -> bool {
    (0..3).all(|c| (got[c] as i16 - want[c] as i16).abs() <= tolerance)
}

/// Lightroom Classic 15.6 (Camera Raw 18.7) renders of a grey JPEG chart: Exposure
/// brightens and darkens through the default rendering, so mid-tones move more than a
/// linear gain and white stays white at −1 EV.
#[test]
fn exposure_on_a_rendered_file_follows_lightroom() -> anyhow::Result<()> {
    let im = grey_ramp();
    for (exposure, levels) in [
        (
            1.,
            [
                (48u8, 79u8),
                (96, 146),
                (128, 181),
                (192, 229),
                (240, 251),
                (255, 255),
            ],
        ),
        (
            -1.,
            [
                (48, 27),
                (96, 60),
                (128, 83),
                (192, 141),
                (240, 213),
                (255, 255),
            ],
        ),
    ] {
        let r = Recipe {
            exposure,
            ..Recipe::for_rendered(&im.metadata)
        };
        let out = rendered_levels(&im, &r)?;
        for (input, want) in levels {
            let got = out[input as usize];
            assert!(
                near(got, [want; 3], 2),
                "Exposure {exposure}: {input} -> {got:?}, Lightroom {want}"
            );
        }
    }
    Ok(())
}

/// Camera Raw 18.7 renders of saturated sRGB primary, secondary and vivid patches in
/// a rendered JPEG at Exposure −1, +1, +2 and +3: the Embedded profile's ACR3 tone
/// curve (and `rendered::scene_from_rendered`'s inverse of it) uses PV2012's
/// hue-ratio-preserving channel mixing rather than PV2010 DNG RGBTone.
#[test]
fn exposure_on_a_rendered_files_saturated_colours_follows_lightroom() -> anyhow::Result<()> {
    let patches = [
        [143u8, 1, 0], // red mid
        [206, 0, 0],   // red bright
        [0, 143, 1],   // green mid
        [0, 0, 142],   // blue mid
        [144, 143, 0], // yellow mid
        [0, 143, 143], // cyan mid
        [143, 0, 142], // magenta mid
        [216, 108, 0], // orange vivid
        [65, 0, 128],  // violet vivid
    ];
    let im = rendered_fixture(&patches);
    for (exposure, expected) in [
        (
            -1.,
            [
                [92u8, 2, 2],
                [139, 0, 0],
                [0, 93, 0],
                [0, 0, 92],
                [94, 93, 0],
                [0, 93, 93],
                [93, 0, 92],
                [150, 71, 0],
                [39, 0, 82],
            ],
        ),
        (
            1.,
            [
                [211, 13, 13],
                [255, 44, 44],
                [48, 198, 48],
                [30, 30, 197],
                [197, 196, 46],
                [93, 194, 194],
                [199, 43, 198],
                [255, 146, 66],
                [100, 29, 184],
            ],
        ),
        (
            2.,
            [
                [255, 54, 54],
                [255, 96, 95],
                [117, 235, 117],
                [76, 77, 237],
                [232, 231, 112],
                [171, 229, 229],
                [238, 110, 236],
                [255, 181, 130],
                [138, 81, 229],
            ],
        ),
        (
            3.,
            [
                [255, 108, 108],
                [255, 152, 152],
                [179, 254, 179],
                [133, 133, 255],
                [250, 250, 176],
                [221, 247, 247],
                [255, 174, 255],
                [255, 213, 185],
                [178, 143, 253],
            ],
        ),
    ] {
        let r = Recipe {
            exposure,
            ..Recipe::for_rendered(&im.metadata)
        };
        let out = rendered_levels(&im, &r)?;
        for (i, want) in expected.into_iter().enumerate() {
            let got = out[i];
            assert!(
                near(got, want, 2),
                "Exposure {exposure}: patch {i} ({:?}) -> {got:?}, Camera Raw {want:?}",
                patches[i]
            );
        }
    }
    Ok(())
}

/// An HDR master render (`r.hdr.enabled = true`) keeps `CurveSet::rendered_tone == false`
/// and GPU `P_RENDERED_TONE == 0`, while the SDR base of an HDR rendered file
/// (`r.hdr.enabled = false`) matches opening the plain SDR image and reproduces its base
/// within ±1/255.
#[test]
fn hdr_master_leaves_rendered_tone_disabled() -> anyhow::Result<()> {
    use rawmakase_model::camera_data::{HdrSource, Source};
    use rawmakase_model::camera_profiles::RGB_TO_PRO;
    use rawmakase_model::color::hdr_output::HdrTransfer;
    let srgb = [[206u8, 0, 0], [0, 143, 1], [216, 108, 0]];
    let decode = |v: u8| {
        let v = v as f32 / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let linear_pro: Vec<[f32; 3]> = srgb
        .iter()
        .map(|p| crate::color::mul(RGB_TO_PRO, p.map(decode)))
        .collect();
    let im = crate::develop::rendered::to_scene(CameraImage {
        recovered: Default::default(),
        width: srgb.len() as u32,
        height: 1,
        pixels: linear_pro.into(),
        metadata: Metadata {
            width: srgb.len() as u32,
            height: 1,
            source: Source::Rendered,
            hdr_source: Some(HdrSource {
                peak: 4.0,
                transfer: HdrTransfer::Pq,
                gain_map: false,
            }),
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    // HDR master path: rendered_tone stays false and GPU P_RENDERED_TONE stays 0.
    let mut r_hdr = Recipe::for_rendered(&im.metadata);
    r_hdr.hdr.enabled = true;
    let camera_hdr = camera_stage(&im.metadata, &r_hdr);
    let lut_hdr = CurveSet::with_photo_measures((&im).into(), &r_hdr, camera_hdr);
    assert!(!lut_hdr.rendered_tone);
    assert!(lut_hdr.hdr.is_some());
    let gpu_hdr = super::pixel_params::pixel_params_hdr((&im).into(), &r_hdr).expect("supported");
    assert_eq!(gpu_hdr.get(super::pixel_params::slot::RENDERED_TONE), &[0.]);
    // SDR base rendition reproduces the SDR base within ±1/255.
    let r_sdr = Recipe::for_rendered(&im.metadata);
    let out = rendered_levels(&im, &r_sdr)?;
    for (got, want) in out.into_iter().zip(srgb) {
        assert!(near(got, want, 1), "{want:?} -> {got:?}");
    }
    Ok(())
}

/// Lightroom's renders of the same chart at Temp +25, +50 and Tint +25.
#[test]
fn white_balance_on_a_rendered_file_follows_lightroom() -> anyhow::Result<()> {
    let im = grey_ramp();
    for (temperature, tint, levels) in [
        (
            25.,
            0.,
            [
                (48u8, [70u8, 61, 45]),
                (96, [132, 117, 92]),
                (128, [170, 152, 124]),
            ],
        ),
        (
            50.,
            0.,
            [
                (48, [91, 73, 42]),
                (96, [165, 136, 87]),
                (128, [202, 170, 120]),
            ],
        ),
        (
            0.,
            25.,
            [
                (48, [52, 47, 54]),
                (96, [102, 95, 106]),
                (128, [136, 127, 140]),
            ],
        ),
    ] {
        let mut r = Recipe::for_rendered(&im.metadata);
        r.incremental_temperature = temperature;
        r.incremental_tint = tint;
        r.update_wb(&im.metadata);
        let out = rendered_levels(&im, &r)?;
        for (input, want) in levels {
            let got = out[input as usize];
            assert!(
                near(got, want, 3),
                "Temp {temperature} Tint {tint}: {input} -> {got:?}, Lightroom {want:?}"
            );
        }
    }
    Ok(())
}

/// Lightroom white-balances a rendered file's colors by a matrix, not channel gains
/// (sky at Temp −35 is 30 levels redder than gains give), and in the cool magenta
/// corner takes blue to 0. There red is 5 levels high: the matrix leaves ProPhoto blue
/// below 0 there, which the tone stage carries and Lightroom's apparently does not.
#[test]
fn white_balance_on_a_rendered_files_colours_follows_lightroom() -> anyhow::Result<()> {
    let (sky, foliage, skin, grey) = (0, 1, 2, 3);
    let im = rendered_fixture(&[[110, 150, 200], [80, 115, 55], [190, 140, 110], [39; 3]]);
    for (temperature, tint, levels) in [
        (
            -35.,
            0.,
            vec![
                (sky, [112u8, 160, 244]),
                (foliage, [35, 129, 101]),
                (skin, [172, 156, 171]),
            ],
        ),
        (
            75.,
            0.,
            vec![(sky, [203, 206, 193]), (foliage, [173, 175, 15])],
        ),
        (-75., 75., vec![(grey, [35, 47, 0])]),
    ] {
        let tolerance = if tint > 50. { 6 } else { 4 };
        let mut r = Recipe::for_rendered(&im.metadata);
        r.incremental_temperature = temperature;
        r.incremental_tint = tint;
        r.update_wb(&im.metadata);
        let out = rendered_levels(&im, &r)?;
        for (patch, want) in levels {
            let got = out[patch];
            assert!(
                near(got, want, tolerance),
                "Temp {temperature} Tint {tint}: patch {patch} -> {got:?}, Lightroom {want:?}"
            );
        }
    }
    Ok(())
}

/// Whites and Blacks act on the pseudo-scene as on a raw scene: Whites +50 lifts the
/// upper tones more than the mid-tones and keeps white white; Blacks −50 darkens shadows.
#[test]
fn whites_and_blacks_move_a_rendered_file() -> anyhow::Result<()> {
    let im = grey_ramp();
    let base = Recipe::for_rendered(&im.metadata);
    let whites = rendered_levels(
        &im,
        &Recipe {
            whites: 0.5,
            ..base.clone()
        },
    )?;
    assert!(
        whites[192][1] as i16 - 192 > whites[96][1] as i16 - 96,
        "{:?} {:?}",
        whites[96],
        whites[192]
    );
    assert_eq!(whites[255], [255; 3]);
    let blacks = rendered_levels(
        &im,
        &Recipe {
            blacks: -0.5,
            ..base
        },
    )?;
    assert!(
        blacks[32][1] < 32 && blacks[255] == [255; 3],
        "{:?} {:?}",
        blacks[32],
        blacks[255]
    );
    Ok(())
}
/// At a rendered file's defaults the render is the file, finer than 8-bit rounding, so
/// a 16-bit export reproduces it too.
#[test]
fn a_rendered_file_at_its_defaults_renders_as_it_is_in_16_bits() -> anyhow::Result<()> {
    let mut srgb: Vec<[u8; 3]> = [0u8, 1, 4, 8, 16, 32, 64, 118, 200, 250, 255]
        .iter()
        .map(|&v| [v; 3])
        .collect();
    srgb.extend([[200, 30, 40], [20, 160, 60], [30, 60, 220], [255, 250, 240]]);
    let im = rendered_fixture(&srgb);
    let r = Recipe::for_rendered(&im.metadata);
    let out = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
    for (got, want) in out.pixels.iter().zip(&srgb) {
        // Saturated colors get a wider tolerance than greys: the f32 rounding of the
        // round trip (about 1e-7 relative in ProPhoto, not the inverse table's error) is
        // amplified by the ProPhoto to sRGB conversion, whose large opposing
        // coefficients and sRGB's steep slope at dark channels turn it into up to ~3
        // 16-bit levels. Greys pass through that conversion unamplified.
        let grey = want[0] == want[1] && want[1] == want[2];
        let tolerance = if grey { 2. } else { 4. } / 65535.;
        for c in 0..3 {
            let want = want[c] as f32 / 255.;
            assert!((got[c] - want).abs() < tolerance, "{got:?} vs {want}");
        }
    }
    Ok(())
}
/// Settings earlier releases saved to choose a rendering engine and its operators are
/// dropped as they are read: one engine renders every recipe.
#[test]
fn engine_and_operator_settings_of_earlier_releases_are_dropped() -> anyhow::Result<()> {
    let im = fixture();
    let plain = Recipe::for_metadata(&im.metadata);
    let mut json = serde_json::to_value(&plain)?;
    let fields = serde_json::json!({
        "engine": 3,
        "profile_tone": false,
        "wide_gamut_curves": false,
        "reference_curves": false,
        "reference_calibration": false,
        "reference_color": false,
        "contrast_model": "Original",
        "sharpening_model": "Original",
        "gamut_model": "Compress",
        "a_later_setting": 1,
    });
    for (key, value) in fields.as_object().unwrap() {
        json[key] = value.clone();
    }
    let old: Recipe = serde_json::from_value(json)?;
    // Unrelated settings a later release wrote are kept.
    assert_eq!(old.unknown.keys().collect::<Vec<_>>(), ["a_later_setting"]);
    assert!(serde_json::to_value(&old)?.get("engine").is_none());
    assert_eq!(
        crate::develop::render(&im, &old.checked()?, 0, Cancel::NEVER)?.pixels,
        crate::develop::render(&im, &plain.checked()?, 0, Cancel::NEVER)?.pixels
    );
    // A recipe saved now has none of them.
    let saved: Recipe = serde_json::from_value(serde_json::to_value(&plain)?)?;
    assert!(saved.unknown.is_empty());
    Ok(())
}

#[test]
fn switched_off_panels_render_as_if_at_their_defaults() -> anyhow::Result<()> {
    use crate::model::panels::{Panel, PanelState};
    let im = fixture();
    let mut edited = Recipe::default();
    edited.effects.vignette = -0.8;
    edited.effects.grain = 0.6;
    edited.hsl[0] = [0.4, -0.6, 0.3];
    edited.exposure = 0.3;
    let mut off = edited.clone();
    off.panels.set(Panel::Effects, PanelState::Off);
    off.panels.set(Panel::ColorMixer, PanelState::Off);
    let plain = Recipe {
        exposure: 0.3,
        ..Default::default()
    };
    let pixels = |r: &Recipe| {
        crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER).map(|out| out.pixels)
    };
    assert_ne!(pixels(&edited)?, pixels(&plain)?);
    assert_eq!(pixels(&off)?, pixels(&plain)?);
    // Through the preview renderer too, which the editor and thumbnails use.
    let mut renderer = crate::develop::PreviewRenderer::with_processor(Err(anyhow::anyhow!("CPU")));
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let preview = renderer
        .render(&im, &off, 0, None, Cancel::new(&cancel))?
        .pixels;
    assert_eq!(preview, pixels(&plain)?);
    Ok(())
}
#[test]
fn panel_switches_round_trip_and_old_recipes_have_every_panel_on() {
    use crate::model::panels::{Panel, PanelState};
    let old: Recipe =
        serde_json::from_value(serde_json::to_value(Recipe::default()).unwrap()).unwrap();
    assert!(old.panels.all_on());
    assert!(!serde_json::to_string(&old).unwrap().contains("panels"));
    let mut r = Recipe::default();
    r.panels.set(Panel::Detail, PanelState::Off);
    let back: Recipe = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back.panels.state(Panel::Detail), PanelState::Off);
    assert!(back.unknown.is_empty());
}
/// Refine Saturation is left out of recipes at its default and kept outside `effects`,
/// whose older readers reject unknown fields, so releases that predate it still open
/// every recipe.
#[test]
fn refine_saturation_is_omitted_at_its_default_and_round_trips() {
    let json = serde_json::to_value(Recipe::default()).unwrap();
    assert!(json.get("curve_saturation").is_none());
    assert!(json["effects"].get("curve_saturation").is_none());
    let r = Recipe {
        curve_saturation: 0.25,
        ..Default::default()
    };
    let back: Recipe = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back.curve_saturation, 0.25);
    assert!(back.unknown.is_empty());
    let old: Recipe = serde_json::from_value(json).unwrap();
    assert_eq!(old.curve_saturation, 1.);
}
/// Constrain Crop is left out of recipes while off, so releases that predate it open
/// every recipe that does not use it.
#[test]
fn constrain_crop_is_omitted_while_off_and_round_trips() {
    let json = serde_json::to_value(Recipe::default()).unwrap();
    assert!(json.get("constrain_crop").is_none());
    let r = Recipe {
        constrain_crop: true,
        ..Default::default()
    };
    let back: Recipe = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert!(back.constrain_crop);
    assert!(back.unknown.is_empty());
    let old: Recipe = serde_json::from_value(json).unwrap();
    assert!(!old.constrain_crop);
}
#[test]
fn reference_color_extremes_stay_finite_and_in_gamut() {
    let im = fixture();
    let mut r = Recipe {
        ..Default::default()
    };
    for amount in [-1., 0., 1.] {
        r.saturation = amount;
        r.vibrance = amount;
        r.hsl = [[amount; 3]; 8];
        r.grading = [[0.62, 1., amount]; 3];
        r.effects.global_grade = [0.3, 1., amount];
        r.effects.balance = amount;
        r.effects.blending = (amount + 1.) * 0.5;
        for p in [
            [0.; 3],
            [1.; 3],
            [1., 0., 0.],
            [0., 0., 1.],
            [0.1, 0.5, 0.2],
        ] {
            let output = adjust(p, &im.metadata, &r);
            assert!(
                output
                    .iter()
                    .all(|v| v.is_finite() && (0. ..=1.).contains(v)),
                "{output:?}"
            );
        }
    }
}

#[test]
fn geometry_orientations_and_crop() {
    let im = fixture();
    let mut r = Recipe::default();
    for rotation in 0..4 {
        r.rotation = rotation;
        let g = Geometry::new(&im, &r, 0);
        let expected = if rotation % 2 == 0 { (12, 8) } else { (8, 12) };
        assert_eq!((g.width, g.height), expected);
        let p = g.source(0.5, 0.5);
        assert!((p[0] - 5.5).abs() < 1e-5 && (p[1] - 3.5).abs() < 1e-5);
    }
    r.rotation = 0;
    r.crop = [0.25, 0.25, 0.75, 0.75];
    let g = Geometry::new(&im, &r, 0);
    assert_eq!((g.width, g.height), (6, 4));
    assert_eq!(g.source(0., 0.), [2.5, 1.5]);
    r.crop = [0., 0., 1., 1.];
    r.straighten = 40.;
    let g = Geometry::new(&im, &r, 0);
    for u in [0., 1.] {
        for v in [0., 1.] {
            let [x, y] = g.source(u, v);
            assert!((-0.501..=11.501).contains(&x));
            assert!((-0.501..=7.501).contains(&y));
        }
    }
}
#[test]
fn point_curves_match_lightroom_ramp_references() {
    // Generated sRGB ramps, exported by Lightroom 15.5.1 as 16-bit TIFF.
    // See tests/data/README.md. These isolate curves from RAW/profile errors.
    let samples: Vec<[[u16; 3]; 5]> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/data/lightroom-point-curves.json"
    )))
    .unwrap();
    let curve = |points: &[[f32; 2]]| ToneCurve {
        points: points.iter().map(|p| p.map(|v| v / 255.)).collect(),
        ..Default::default()
    };
    let clipped = curve(&[[32., 16.], [96., 120.], [200., 224.]]);
    let mut recipes = [
        Recipe::default(),
        Recipe::default(),
        Recipe::default(),
        Recipe::default(),
    ];
    recipes[0].curve = curve(&[
        [0., 0.],
        [48., 24.],
        [128., 142.],
        [200., 224.],
        [255., 255.],
    ]);
    recipes[1].effects.channels[0] = curve(&[[0., 12.], [72., 52.], [170., 195.], [255., 244.]]);
    recipes[1].effects.channels[2] = curve(&[[0., 0.], [64., 86.], [192., 168.], [255., 255.]]);
    recipes[2].curve = clipped.clone();
    recipes[3].effects.channels[0] = clipped;
    recipes[3].effects.channels[2] = curve(&[[0., 0.], [64., 180.], [192., 64.], [255., 255.]]);
    for (case, recipe) in recipes.iter().enumerate() {
        let lut = CurveSet::new(recipe);
        let mut sum = 0.;
        let mut count = 0;
        let mut peak = 0f32;
        for (i, sample) in samples.iter().enumerate() {
            // Master curves still have a measured saturated-color residual;
            // only the neutral strip has the tight equivalence established here.
            if case % 2 == 0 && i >= 32 {
                continue;
            }
            let input = sample[0].map(|v| srgb_decode(v as f32 / 65535.));
            let actual = apply_reference_curves(input, recipe, &lut, None)
                .map(|v| srgb_encode(v).clamp(0., 1.));
            for (a, expected) in actual.into_iter().zip(sample[case + 1]) {
                let error = (a - expected as f32 / 65535.).abs();
                sum += error;
                peak = peak.max(error);
                count += 1;
            }
        }
        let mean = sum / count as f32;
        assert!(
            mean < 0.0001 && peak < 0.006,
            "case {case}: MAE {mean}, max {peak}"
        );
        if case % 2 == 0 {
            assert!(mean < 0.00003, "neutral MAE {mean}");
        }
    }
}

#[test]
fn viewport_matches_full_export_with_detail_and_geometry() -> Result<()> {
    let im = fixture();
    let mut r = Recipe {
        rotation: 1,
        straighten: 8.,
        noise_luma: 0.2,
        noise_chroma: 0.3,
        sharpening: 0.4,
        exposure: 0.5,
        camera_exposure: 0.15,
        vibrance: 0.6,
        grading: [[0.6, 0.3, 0.1]; 3],
        ..Default::default()
    };
    r.effects.calibration = [[0.2, -0.3], [-0.4, 0.5], [0.6, -0.7]];
    r.effects.shadow_tint = 0.3;
    r.curve.insert([0.4, 0.5]);
    r.effects.channels[0].insert([0.6, 0.7]);
    let full = render(&im, &r.checked()?, 0, Cancel::NEVER)?;
    let tile = render_region(&im, &r.checked()?, [1, 2, 4, 5])?;
    for y in 0..5 {
        for x in 0..4 {
            let a = full.pixels[(y + 2) * full.width as usize + x + 1];
            let b = tile.pixels[y * 4 + x];
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-6, "{a:?} vs {b:?}");
            }
        }
    }
    Ok(())
}
#[test]
fn wb_at_estimated_as_shot_is_continuous() {
    let m = fixture().metadata;
    let mut r = Recipe::for_metadata(&m);
    r.update_wb(&m);
    for v in r.wb {
        assert!((v - 1.).abs() < 1e-6);
    }
    r.temperature += 1.;
    r.update_wb(&m);
    for v in r.wb {
        assert!((v - 1.).abs() < 0.01);
    }
}
#[test]
fn neutral_picker_balances_channels() {
    let mut im = fixture();
    im.pixels.fill([0.4, 0.2, 0.1]);
    let r = Recipe::default();
    assert_eq!(neutral_pick(&im, &r, 0.5, 0.5), [0.5, 1., 2.]);
}
#[test]
fn clipped_channels_do_not_make_magenta_highlights() -> Result<()> {
    let mut im = fixture();
    im.metadata.wb = [2.5, 1., 1.4];
    im.metadata.matrix = [
        [2.0124, -0.9049, -0.1075],
        [-0.1196, 1.564, -0.4444],
        [0.0402, -0.4608, 1.4206],
    ];
    // Every channel clipped: highlight recovery makes the pixel neutral.
    im.pixels.fill([2.5, 1., 1.4]);
    for exposure in [-3., 0., 2.] {
        let r = Recipe {
            exposure,
            ..Recipe::for_metadata(&im.metadata)
        };
        let p = render(&im, &r.checked()?, 0, Cancel::NEVER)?.pixels[0];
        assert!(
            (p[0] - p[1]).abs() < 0.0001 && (p[1] - p[2]).abs() < 0.0001,
            "{p:?}"
        );
    }
    Ok(())
}
/// The photo's maximum is measured with the camera's as-shot color: a Temperature
/// change under a profile whose matrices follow the illuminant leaves it in place.
#[test]
fn temperature_leaves_the_photos_maximum_in_place() -> anyhow::Result<()> {
    let mut im = fixture();
    for p in im.pixels.iter_mut().step_by(3) {
        *p = [0.9, 0.5, 0.2];
    }
    let profile: crate::camera_profiles::CameraProfile =
        serde_json::from_value(serde_json::json!({
            "name": "dual", "camera": "", "copyright": "",
            "color1": [[1.2, -0.1, -0.1], [-0.2, 1.1, 0.1], [0.0, 0.2, 0.9]],
            "color2": [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            "forward1": [[0.5, 0.3, 0.2], [0.2, 0.7, 0.1], [0.0, 0.1, 0.8]],
            "forward2": [[0.8, 0.1, 0.1], [0.3, 0.6, 0.1], [0.0, 0.0, 0.9]],
            "kelvin1": 2856.0, "kelvin2": 6504.0,
            "hue1": null, "hue2": null, "look": null, "tone": [], "exposure": 0.0
        }))?;
    let maximum = |temperature: f32| {
        let r = Recipe {
            profile: Some(std::sync::Arc::new(profile.clone())),
            temperature,
            ..Default::default()
        };
        let camera = camera_stage(&im.metadata, &r);
        CurveSet::with_photo_measures((&im).into(), &r, camera)
            .measures
            .max
    };
    assert_eq!(maximum(3000.), maximum(7500.));
    Ok(())
}
/// Under a profile without forward matrices, whose matrix follows Tint as well as
/// Temperature (`CameraProfile::camera_matrix_at`), a Tint change leaves the photo's
/// maximum in place too.
#[test]
fn tint_leaves_the_photos_maximum_in_place_without_forward_matrices() {
    let mut im = fixture();
    for p in im.pixels.iter_mut().step_by(3) {
        *p = [0.9, 0.5, 0.2];
    }
    im.metadata.cam_xyz = [[1.2, -0.1, -0.1], [-0.2, 1.1, 0.1], [0.0, 0.2, 0.9]];
    let profile = crate::camera_profiles::open::standard(&im.metadata).unwrap();
    let maximum = |tint: f32| {
        let r = Recipe {
            profile: Some(std::sync::Arc::new(profile.clone())),
            tint,
            ..Recipe::for_metadata(&im.metadata)
        };
        let camera = camera_stage(&im.metadata, &r);
        CurveSet::with_photo_measures((&im).into(), &r, camera)
            .measures
            .max
    };
    assert_eq!(maximum(-60.), maximum(60.));
}
/// The level the default black follows is the photo's 0.1th percentile of luminance:
/// a few dark pixels (a darkened red-eye pupil) move the minimum, which Blacks reads,
/// but not it.
#[test]
fn a_few_dark_pixels_move_the_minimum_but_not_the_dark_level() {
    let (width, height) = (128, 96);
    let mut im = fixture();
    im.width = width;
    im.height = height;
    im.metadata.width = width;
    im.metadata.height = height;
    im.pixels = (0..(width * height) as usize)
        .map(|i| [0.05 + (i % 128) as f32 / 400.; 3])
        .collect();
    let r = Recipe::default();
    let camera = camera_stage(&im.metadata, &r);
    let measures = |im: &CameraImage| CurveSet::with_photo_measures(im.into(), &r, camera).measures;
    let plain = measures(&im);
    for (x, y) in [(60, 40), (61, 40), (60, 41), (61, 41)] {
        im.pixels[y * width as usize + x] = [0.001; 3];
    }
    let spotted = measures(&im);
    assert!(
        spotted.min < plain.min - 3.,
        "{} {}",
        spotted.min,
        plain.min
    );
    assert_eq!(spotted.dark, plain.dark);
}
/// Every scene stage control, and the settings after the stage that read the photo.
fn scene_stage_recipe() -> Recipe {
    let mut r = Recipe {
        exposure: 0.4,
        whites: 0.5,
        blacks: -0.3,
        shadows: 0.4,
        highlights: -0.5,
        contrast: 0.3,
        saturation: 0.2,
        ..Default::default()
    };
    r.effects.dehaze = 0.3;
    r
}
/// The display render is the stages after the scene stage replayed from its output,
/// with the same photo measures: nothing after the scene stage is folded into it.
#[test]
fn the_display_render_replays_the_stages_after_the_scene_stage() {
    let im = fixture();
    let r = scene_stage_recipe();
    let camera = camera_stage(&im.metadata, &r);
    let lut = CurveSet::with_photo_measures((&im).into(), &r, camera);
    let mut tap = CurveSet::with_photo_measures((&im).into(), &r, camera);
    tap.output = PixelOutput::SceneOutput;
    for (i, p) in im.pixels.iter().enumerate() {
        let pos = [(i % 12) as f32, (i / 12) as f32];
        let shown = process_pixel(*p, &r, &lut, camera, pos, None);
        let scene = process_pixel(*p, &r, &tap, camera, pos, None);
        assert_eq!(
            shown,
            color_stage(profile_stage(scene, &r, &lut), &r, &lut, None)
        );
    }
}
/// A camera without color data (no matrix, no profile) renders through no profile:
/// its camera RGB as sRGB through a luminance shoulder, on the CPU only. Pinned so
/// a fitted default (Camera Look) can only change it on purpose.
#[test]
fn a_camera_without_colour_data_renders_through_no_profile() {
    let im = fixture();
    let r = Recipe::with_profiles(&im.metadata, &[]);
    assert!(r.profile.is_none());
    assert!(!super::pixel_params::supported(&r));
    let pro = mul(crate::camera_profiles::RGB_TO_PRO, [0.4, 0.2, 0.1]);
    let lut = CurveSet::new(&r);
    let out = profile_stage(pro, &r, &lut);
    let y = crate::color::luminance([0.4, 0.2, 0.1]);
    let k = 1. / (y + 0.82);
    for (got, want) in out.iter().zip([0.4 * k, 0.2 * k, 0.1 * k]) {
        assert!((got - want).abs() < 1e-5, "{out:?}");
    }
}
/// The camera input stage is the decoder's samples times the white balance, through
/// the render's geometry: bright channels keep what the sensor recorded, where
/// highlight reconstruction pulls them (a fitted camera matrix must not learn it).
#[test]
fn the_camera_input_stage_is_before_highlight_reconstruction() -> anyhow::Result<()> {
    let mut im = fixture();
    im.pixels[13] = [0.8, 0.5, 0.4];
    let r = Recipe {
        wb: [1.5, 1., 0.8],
        ..Default::default()
    };
    let camera = crate::develop::quality::render_stage(
        &im,
        &r,
        crate::develop::quality::Stage::CameraInput,
        Cancel::NEVER,
    )?;
    assert_eq!([camera.width, camera.height], [12, 8]);
    for (got, p) in camera.pixels.iter().zip(&im.pixels) {
        let want: [f32; 3] = std::array::from_fn(|c| p[c] * r.wb[c]);
        assert!(
            got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6),
            "{got:?} {want:?}"
        );
    }
    let recovered = crate::develop::quality::recovered(&im, Cancel::NEVER)?;
    assert_ne!(
        recovered.pixels[13], im.pixels[13],
        "the bright color is reconstructed"
    );
    Ok(())
}
/// The scene stage reads nothing computed after it: settings applied after the stage
/// leave its output unchanged.
#[test]
fn settings_after_the_scene_stage_leave_it_unchanged() -> anyhow::Result<()> {
    let im = fixture();
    let scene = |r: &Recipe| {
        crate::develop::quality::render_stage(
            &im,
            r,
            crate::develop::quality::Stage::SceneOutput,
            Cancel::NEVER,
        )
    };
    let base = scene_stage_recipe();
    let expected = scene(&base)?.pixels;
    let edits: [&dyn Fn(&mut Recipe); 9] = [
        &|r| r.contrast = -0.6,
        &|r| r.curve.points = vec![[0., 0.], [0.5, 0.7], [1., 1.]],
        &|r| r.effects.parametric = [0.3, -0.2, 0.1, 0.],
        &|r| r.saturation = -0.5,
        &|r| r.vibrance = 0.6,
        &|r| r.hsl[2] = [0.2, -0.4, 0.3],
        &|r| r.grading[0] = [0.3, 0.4, 0.],
        &|r| r.effects.vignette = -0.5,
        &|r| r.effects.monochrome = true,
    ];
    for (i, edit) in edits.iter().enumerate() {
        let mut r = base.clone();
        edit(&mut r);
        assert_eq!(scene(&r)?.pixels, expected, "edit {i}");
    }
    Ok(())
}
#[test]
fn exposure_reveals_retained_highlights() {
    // A photo whose highlights were rebuilt to twice the sensor's white.
    let mut im = fixture();
    for p in im.pixels.iter_mut().step_by(2) {
        *p = [2.; 3];
    }
    let pixel = |exposure: f32, v: f32| {
        let r = Recipe {
            exposure,
            ..Default::default()
        };
        let camera = camera_stage(&im.metadata, &r);
        let lut = CurveSet::with_photo_measures((&im).into(), &r, camera);
        process_pixel([v; 3], &r, &lut, camera, [0., 0.], None)
    };
    // At Exposure 0 everything above the sensor's white rolls on to the top; lower
    // Exposure separates the rebuilt levels again. The photo's maximum stays at the
    // top, as the white point follows Exposure.
    let (a, b) = (pixel(0., 1.5)[1], pixel(0., 2.)[1]);
    assert!((a - b).abs() < 1e-4, "{a} {b}");
    let (a, b) = (pixel(-2., 1.5)[1], pixel(-2., 2.)[1]);
    assert!(a < b - 0.05 && a > 0.5, "{a} {b}");
    assert!(pixel(-2., 1.)[1] < a - 0.05);
    assert!(pixel(0., 2.).iter().all(|v| v.is_finite()));
}
#[test]
fn invalid_recipes_rejected() {
    let mut r = Recipe {
        exposure: f32::NAN,
        ..Default::default()
    };
    assert!(r.validate().is_err());
    r = Recipe::default();
    r.crop = [0.5, 0., 0.4, 1.];
    assert!(r.validate().is_err());
    r = Recipe::default();
    r.curve.points = vec![[0., 0.], [0.5, 0.8], [0.4, 0.5], [1., 1.]];
    assert!(r.validate().is_err());
}
#[test]
fn perceptual_roundtrip() {
    for p in [[0.1, 0.2, 0.4], [0.8, 0.2, 0.01], [-0.01, 0.3, 1.2]] {
        let q = lab_to_srgb(srgb_to_lab(p));
        for c in 0..3 {
            assert!((p[c] - q[c]).abs() < 1e-5);
        }
    }
}
#[test]
fn hue_bands_wrap_without_discontinuity() {
    let a = hue_weights(-0.00001);
    let b = hue_weights(0.00001);
    for c in 0..8 {
        assert!((a[c] - b[c]).abs() < 0.001);
    }
    for i in 0..1000 {
        assert!((hue_weights(i as f32 / 1000.).iter().sum::<f32>() - 1.).abs() < 1e-6);
    }
}
#[test]
fn neutral_color_fast_path_matches_general_processing() {
    let metadata = Metadata {
        wb: [2., 1., 1.5],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let recipe = Recipe {
        exposure: 0.4,
        ..Recipe::default()
    };
    let fast = CurveSet::new(&recipe);
    let mut general = CurveSet::new(&recipe);
    general.color_adjustments = true;
    let camera = camera_stage(&metadata, &recipe);
    for i in 0..4096 {
        let p = [
            (i % 16) as f32 / 8.,
            ((i / 16) % 16) as f32 / 8.,
            (i / 256) as f32 / 8.,
        ];
        let a = process_pixel(p, &recipe, &fast, camera, [0., 0.], None);
        let b = process_pixel(p, &recipe, &general, camera, [0., 0.], None);
        for c in 0..3 {
            assert!((a[c] - b[c]).abs() < 2e-5, "{p:?}: {a:?} vs {b:?}");
        }
    }
}
#[test]
fn builtin_lens_correction_brightens_corners_and_keeps_regions_consistent() {
    use crate::optics::{LensCorrection, Radial};
    let mut im = fixture();
    im.pixels = vec![[0.1; 3]; 96].into();
    im.metadata.lens = Some(LensCorrection {
        source: "test".into(),
        default_on: true,
        vignetting: Some(Radial {
            knots: vec![0., 1.],
            values: vec![1., 2.],
        }),
        distortion: Some(Radial {
            knots: vec![0., 1.],
            values: vec![1., 1.01],
        }),
        chromatic: None,
        gain_map: None,
        ..Default::default()
    });
    let on = Recipe::for_metadata(&im.metadata);
    assert!(on.lens_builtin);
    let off = Recipe {
        lens_builtin: false,
        ..on.clone()
    };
    let lum = |p: [f32; 3]| p.iter().sum::<f32>();
    let a = render(&im, &on.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    let b = render(&im, &off.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    let corner = |r: &Rendered| lum(r.pixels[0]);
    let centre = |r: &Rendered| lum(r.pixels[(4 * r.width + 6) as usize]);
    assert!(corner(&a) > corner(&b) + 0.01);
    assert!(corner(&a) > centre(&a) && (corner(&b) - centre(&b)).abs() < 1e-5);
    let region = render_region(&im, &on.checked().unwrap(), [3, 2, 4, 3]).unwrap();
    let full = render(&im, &on.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    for y in 0..3 {
        for x in 0..4 {
            let p = region.pixels[(y * 4 + x) as usize];
            let q = full.pixels[((y + 2) * full.width + x + 3) as usize];
            assert!((0..3).all(|c| (p[c] - q[c]).abs() < 2e-6));
        }
    }
}
/// A DNG's OpcodeList3 GainMap brightens each channel where its grid says, with the
/// vignetting, and follows the lens Vignetting amount.
#[test]
fn opcode_gain_maps_scale_each_channel_where_the_grid_says() {
    use crate::optics::{GainGrid, LensCorrection};
    let mut im = fixture();
    im.pixels = vec![[0.1; 3]; 96].into();
    let w = im.width as f32;
    // Two columns of points: gain 1 on the first column of pixels; on the last, red 2,
    // green 1.5, blue 1.
    im.metadata.lens = Some(LensCorrection {
        source: "DNG opcodes".into(),
        default_on: true,
        gain_map: Some(GainGrid {
            points: [1, 2],
            // Points on the first and last pixels' centres.
            origin: [0., 0.5 / w],
            spacing: [1., (w - 1.) / w],
            planes: 3,
            gains: vec![1., 1., 1., 2., 1.5, 1.],
            ..Default::default()
        }),
        ..Default::default()
    });
    let on = Recipe::for_metadata(&im.metadata);
    let field = |r: &Recipe| GainField::new(&im, r).unwrap();
    assert_eq!(field(&on).gain(0., 3.), [1.; 3]);
    let right = field(&on).gain(w - 1., 3.);
    assert!(
        right
            .iter()
            .zip([2., 1.5, 1.])
            .all(|(g, want)| (g - want).abs() < 1e-5),
        "{right:?}"
    );
    let half = field(&on).gain((w - 1.) / 2., 0.);
    assert!((half[0] - 1.5).abs() < 1e-5, "{half:?}");
    // Outside the image, nothing.
    assert_eq!(field(&on).gain(w, 0.), [1.; 3]);
    let amount = |a: f32| {
        field(&Recipe {
            lens_vignetting: a,
            ..on.clone()
        })
        .gain(w - 1., 0.)[0]
    };
    // Camera Raw applies a file's opcodes whatever the profile's Vignetting amount.
    assert!((amount(0.) - 2.).abs() < 1e-5);
    assert!((amount(2.) - 2.).abs() < 1e-5);
    // Rendered: the right edge is brighter than the left, and nothing changes with the
    // built-in correction off.
    let off = Recipe {
        lens_builtin: false,
        ..on.clone()
    };
    let lum = |p: [f32; 3]| p.iter().sum::<f32>();
    let a = render(&im, &on.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    let b = render(&im, &off.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    let at = |r: &Rendered, x: u32| lum(r.pixels[(4 * r.width + x) as usize]);
    assert!(at(&a, a.width - 1) > at(&a, 0) + 0.05);
    assert!((at(&b, b.width - 1) - at(&b, 0)).abs() < 1e-5);
}
/// The copy the photo is measured on carries its GainMap, as the render corrects it.
#[test]
fn measurement_copies_carry_the_gain_map() {
    use crate::optics::{GainGrid, LensCorrection};
    let mut im = fixture();
    im.pixels = vec![[0.1; 3]; 96].into();
    im.metadata.lens = Some(LensCorrection {
        source: "DNG opcodes".into(),
        default_on: true,
        gain_map: Some(GainGrid {
            points: [1, 2],
            spacing: [1., 1.],
            gains: vec![1., 1., 1., 3., 2., 1.],
            planes: 3,
            ..Default::default()
        }),
        ..Default::default()
    });
    let copy = measurement_source((&im).into(), 4, true);
    let (w, row) = (copy.width as usize, copy.width as usize);
    let (left, right) = (copy.pixels[row], copy.pixels[row + w - 1]);
    assert!(
        right[0] > left[0] * 2. && right[1] > left[1] * 1.5 && (right[2] - left[2]).abs() < 1e-6,
        "{left:?} {right:?}"
    );
    // Without a GainMap the copy is the photo reduced.
    im.metadata.lens = None;
    assert_eq!(
        measurement_source((&im).into(), 4, true).pixels,
        preview_source((&im).into(), 4).pixels
    );
}
/// The copy the photo is measured on carries a DNG's OpcodeList3 FixVignetteRadial
/// about its optical centre, with the GainMap when there is one. Vignetting Camera Raw
/// is not known to measure after (a maker's own table, OpcodeList2's opcode) is not on
/// the copy.
#[test]
fn measurement_copies_carry_a_dngs_opcode_list3_vignette() {
    use crate::optics::{GainGrid, LensCorrection, Radial};
    let mut im = fixture();
    im.pixels = vec![[0.1; 3]; 96].into();
    let lens = LensCorrection {
        source: "DNG opcodes".into(),
        default_on: true,
        vignetting: Radial::new(vec![0., 2.], vec![1., 3.]),
        vignetting_measured: true,
        centre: [0., 0.5],
        ..Default::default()
    };
    im.metadata.lens = Some(lens.clone());
    let copy = measurement_source((&im).into(), 4, true);
    let (w, h) = (copy.width as usize, copy.height as usize);
    let half = (w as f32).hypot(h as f32) * 0.5;
    for (i, p) in copy.pixels.iter().enumerate() {
        let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
        let gain = 1. + x.hypot(y - h as f32 * 0.5) / half;
        assert!(
            (p[0] - 0.1 * gain).abs() < 1e-6 && p[0] == p[2],
            "{i}: {p:?}"
        );
    }
    // With a GainMap both apply.
    let mut both = lens.clone();
    both.gain_map = Some(GainGrid {
        gains: vec![2.],
        ..Default::default()
    });
    im.metadata.lens = Some(both);
    let doubled = measurement_source((&im).into(), 4, true);
    for (p, q) in doubled.pixels.iter().zip(&copy.pixels) {
        assert!((p[1] - 2. * q[1]).abs() < 1e-6);
    }
    im.metadata.lens = Some(LensCorrection {
        vignetting_measured: false,
        ..lens
    });
    assert_eq!(
        measurement_source((&im).into(), 4, true).pixels,
        preview_source((&im).into(), 4).pixels
    );
}
/// The file's own correction switched off (`Recipe::lens_file_correction`) leaves
/// it off the measurement copy too: the copy is the photo's without it.
#[test]
fn measurement_copies_leave_the_files_correction_out_when_it_is_off() {
    use crate::optics::{GainGrid, LensCorrection, Radial};
    let mut im = fixture();
    im.metadata.lens = Some(LensCorrection {
        source: LensCorrection::DNG_OPCODES.into(),
        default_on: true,
        vignetting: Radial::new(vec![0., 2.], vec![1., 3.]),
        vignetting_measured: true,
        gain_map: Some(GainGrid {
            gains: vec![2.],
            ..Default::default()
        }),
        ..Default::default()
    });
    let plain = preview_source((&im).into(), 4).pixels;
    assert_ne!(measurement_source((&im).into(), 4, true).pixels, plain);
    assert_eq!(measurement_source((&im).into(), 4, false).pixels, plain);
}
/// Optical black corners that a camera's built-in barrel-distortion correction crops
/// away must not pollute the measurement copy (or drag Shadows/Highlights' dark floor
/// down), while in-frame pixels stay untouched.
#[test]
fn measurement_copies_clamp_corners_cropped_by_builtin_distortion() {
    use crate::optics::{LensCorrection, Radial};
    let (w, h) = (64u32, 48u32);
    // Strong barrel distortion (k < 1 at outer radii) pulls corners inward.
    let lens = LensCorrection {
        source: "Panasonic built-in".into(),
        default_on: true,
        distortion: Radial::new(vec![0., 0.5, 1., 1.5], vec![1., 0.92, 0.78, 0.65]),
        ..Default::default()
    };
    let mut pixels = vec![[0.2; 3]; (w * h) as usize];
    // Black optical corners outside the built-in crop.
    for y in 0..6u32 {
        for x in 0..6u32 {
            pixels[(y * w + x) as usize] = [0.; 3];
            pixels[(y * w + (w - 1 - x)) as usize] = [0.; 3];
            pixels[((h - 1 - y) * w + x) as usize] = [0.; 3];
            pixels[((h - 1 - y) * w + (w - 1 - x)) as usize] = [0.; 3];
        }
    }
    let im = CameraImage {
        width: w,
        height: h,
        pixels: pixels.into(),
        metadata: crate::camera_data::Metadata {
            width: w,
            height: h,
            wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            lens: Some(lens),
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
        recovered: Default::default(),
    };
    let copy = measurement_source((&im).into(), 64, true);
    assert!(
        copy.pixels[0][1] > 0.15,
        "optical black corner was not clamped away: {:?}",
        copy.pixels[0]
    );
    // Centre pixel stays untouched.
    assert_eq!(copy.pixels[(24 * w + 32) as usize], [0.2; 3]);
}
/// Optical-black margins outside the camera's default crop (`crop_left/top/width/height`,
/// e.g. Sony A7R VI raw borders) must not leak into the measurement copy.
#[test]
fn measurement_copies_clamp_optical_black_outside_default_crop() {
    let (w, h) = (64u32, 48u32);
    let mut pixels = vec![[0.; 3]; (w * h) as usize];
    for y in 2..40u32 {
        for x in 4..56u32 {
            pixels[(y * w + x) as usize] = [0.25; 3];
        }
    }
    let im = CameraImage {
        width: w,
        height: h,
        pixels: pixels.into(),
        metadata: crate::camera_data::Metadata {
            width: w,
            height: h,
            crop_left: 4,
            crop_top: 2,
            crop_width: 52,
            crop_height: 38,
            wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
        recovered: Default::default(),
    };
    for max in [64, 32] {
        let copy = measurement_source((&im).into(), max, true);
        for p in copy.pixels.iter() {
            assert!(
                (p[1] - 0.25).abs() < 1e-6,
                "optical black leaked at max={max}: {p:?}"
            );
        }
    }
    // Non-power-of-two full-res frame (`47x502`, where `(1/47)*47 < 1` in f32) and
    // reduced decode with odd crop offset must also never leak optical-black edges.
    let (nw, nh) = (47u32, 502u32);
    let mut npix = vec![[0.; 3]; (nw * nh) as usize];
    for y in 1..501u32 {
        for x in 1..46u32 {
            npix[(y * nw + x) as usize] = [0.25; 3];
        }
    }
    let non_pow2 = CameraImage {
        width: nw,
        height: nh,
        pixels: npix.into(),
        metadata: Metadata {
            width: nw,
            height: nh,
            crop_left: 1,
            crop_top: 1,
            crop_width: 45,
            crop_height: 500,
            wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
        recovered: Default::default(),
    };
    let copy_np = measurement_source((&non_pow2).into(), 512, true);
    assert!(
        copy_np.pixels.iter().all(|p| (p[1] - 0.25).abs() < 1e-6),
        "optical black leaked on non-power-of-two default crop"
    );
    // Half-size decode (`32x24` of `64x48`) with odd raw `crop_left = 11`: inner rect
    // must round inward to `x0 = 6`, not outward to `5`.
    let half = CameraImage {
        width: 32,
        height: 24,
        pixels: vec![[0.25; 3]; 32 * 24].into(),
        metadata: Metadata {
            width: 64,
            height: 48,
            crop_left: 11,
            crop_top: 5,
            crop_width: 42,
            crop_height: 38,
            ..Default::default()
        },
        fast: true,
        scale_factor: 0.5,
        scale_clipped: 0,
        hdr: Default::default(),
        recovered: Default::default(),
    };
    assert_eq!(
        crate::develop::local_tone::inner_crop_rect(&half),
        Some([6, 3, 26, 21])
    );
}
/// Built-in barrel-distortion corner clamping must only replace genuinely dark optical
/// black corners (>3 stops darker than the rectified edge), leaving illuminated outer
/// pixels and Sony's user-resettable `camera_crop` margins intact.
#[test]
fn measurement_copies_preserve_illuminated_outer_borders_and_sony_camera_crop() {
    use crate::optics::{LensCorrection, Radial};
    let (w, h) = (64u32, 48u32);
    let mut pixels = vec![[0.2; 3]; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            if x < 12 || x >= w - 12 || y < 6 || y >= h - 6 {
                pixels[(y * w + x) as usize] = [0.35; 3];
            }
        }
    }
    let im = CameraImage {
        width: w,
        height: h,
        pixels: pixels.clone().into(),
        metadata: crate::camera_data::Metadata {
            make: "SONY".into(),
            width: w,
            height: h,
            camera_crop: Some([1. / 6., 0., 5. / 6., 1.]),
            wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            lens: Some(LensCorrection {
                source: LensCorrection::SONY_BUILTIN.into(),
                default_on: true,
                distortion: Radial::new(vec![0., 0.5, 1., 1.5], vec![1., 0.96, 0.92, 0.88]),
                ..Default::default()
            }),
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
        recovered: Default::default(),
    };
    let copy = measurement_source((&im).into(), 64, true);
    assert_eq!(copy.pixels[0], [0.35; 3]);
    assert_eq!(copy.pixels[(24 * w + 2) as usize], [0.35; 3]);
}
/// A non-empty `CameraImage` with zero-sized metadata (`Metadata::default()`) must not
/// produce a `NaN` inset or panic in `measurement_source` or `active_crop_rect`.
#[test]
fn zero_metadata_dimensions_do_not_panic_in_crop_helpers() {
    let im = CameraImage {
        width: 16,
        height: 12,
        pixels: vec![[0.2; 3]; 16 * 12].into(),
        metadata: Metadata::default(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
        recovered: Default::default(),
    };
    assert_eq!(crate::develop::local_tone::active_crop_rect(&im), None);
    let copy = measurement_source((&im).into(), 16, true);
    assert_eq!((copy.width, copy.height), (16, 12));
}
/// Cached (`PreviewRenderer`) and uncached (`develop::render`) renders must agree on a
/// cropped-sensor photo with Shadows/Highlights active (`MapBase::with_crop`).
#[test]
fn cached_and_uncached_renders_match_on_cropped_sensor_with_shadows_and_highlights() {
    let (w, h) = (64u32, 48u32);
    let mut pixels = vec![[0.; 3]; (w * h) as usize];
    for y in 4..40u32 {
        for x in 6..54u32 {
            let v = 0.01 + 0.5 * ((x - 6) as f32 / 48.) * ((y - 4) as f32 / 36.);
            pixels[(y * w + x) as usize] = [v, v * 0.9, v * 0.8];
        }
    }
    let im = CameraImage {
        width: w,
        height: h,
        pixels: pixels.into(),
        metadata: Metadata {
            width: w,
            height: h,
            crop_left: 6,
            crop_top: 4,
            crop_width: 48,
            crop_height: 36,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
        recovered: Default::default(),
    };
    let r = Recipe {
        shadows: 0.7,
        highlights: -0.6,
        ..Default::default()
    };
    let valid = r.checked().unwrap();
    let uncached = crate::develop::render(&im, &valid, 0, Cancel::NEVER).unwrap();
    let mut renderer = crate::develop::PreviewRenderer::default();
    let cached = renderer.render(&im, &r, 0, None, Cancel::NEVER).unwrap();
    assert_eq!(cached.pixels, uncached.pixels);
}
/// Shadows' `default_black` key shift must sample map cells at their exact centers, so
/// the shifted black curve is invariant to the source image resolution.
#[test]
fn shadows_default_black_shift_is_resolution_invariant() {
    let make = |w: u32, h: u32| {
        let mut pixels = vec![[0.02; 3]; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / w as f32;
                let v = if u < 0.05 { 0.002 } else { 0.02 + 0.5 * u };
                pixels[(y * w + x) as usize] = [v; 3];
            }
        }
        CameraImage {
            width: w,
            height: h,
            pixels: pixels.into(),
            metadata: Metadata {
                width: w,
                height: h,
                wb: [1.; 3],
                daylight_wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
            recovered: Default::default(),
        }
    };
    let small_im = Arc::new(make(64, 48));
    let large_im = make(256, 192);
    let r = Recipe {
        shadows: 1.0,
        ..Default::default()
    };
    let c_small = CurveSet::for_image(
        Source {
            measured: Some(&small_im),
            ..Source::new(&small_im)
        },
        &r,
        camera_stage(&small_im.metadata, &r),
        false,
    );
    let c_large = CurveSet::for_image(
        Source {
            measured: Some(&small_im),
            ..Source::new(&large_im)
        },
        &r,
        camera_stage(&large_im.metadata, &r),
        false,
    );
    let a = c_small.scene.global.apply([0.005; 3])[0];
    let b = c_large.scene.global.apply([0.005; 3])[0];
    assert!(
        (a - b).abs() < 1e-5,
        "default_black shift varied with source resolution: {a} vs {b}"
    );
}
/// Before a per-channel warp (lateral chromatic aberration), each channel reads the
/// GainMap where it samples the photo, not where green does.
#[test]
fn gain_maps_before_a_warp_follow_each_channels_source() {
    use crate::optics::{GainGrid, LensCorrection, Radial};
    let mut im = fixture();
    im.pixels = vec![[0.1; 3]; 96].into();
    let w = im.width as f32;
    im.metadata.lens = Some(LensCorrection {
        source: "DNG opcodes".into(),
        default_on: true,
        // Red samples well inside green at the edges.
        chromatic: Some([
            Radial {
                knots: vec![0., 1.],
                values: vec![1., 0.8],
            },
            Radial {
                knots: vec![0., 1.],
                values: vec![1., 1.],
            },
        ]),
        // Every channel's gain rises from 1 on the left to 3 on the right.
        gain_map: Some(GainGrid {
            points: [1, 2],
            spacing: [1., 1.],
            gains: vec![1., 1., 1., 3., 3., 3.],
            planes: 3,
            ..Default::default()
        }),
        ..Default::default()
    });
    let r = Recipe::for_metadata(&im.metadata);
    let (x, y) = (w - 1., 3.);
    let red_before = {
        let warp = LensWarp::new(&im, &r).unwrap();
        let p = warp.sample(Source::new(&im), x, y, 0.);
        let m = &warp.map;
        let ([dx, dy], scale) = m.scales(x, y);
        let field = warp.gain_map.as_ref().unwrap();
        let want = |c: usize| {
            let at = [
                m.center[0] + dx * scale[c] - 0.5,
                m.center[1] + dy * scale[c] - 0.5,
            ];
            0.1 * field.gain(at[0], at[1])[c]
        };
        assert!(want(1) - want(0) > 0.01, "{} {}", want(0), want(1));
        for c in 0..3 {
            assert!((p[c] - want(c)).abs() < 1e-5, "{c}: {p:?} {}", want(c));
        }
        want(0) / 0.1
    };
    // After the warp: every channel at the corrected position, at the fill's scale.
    if let Some(l) = im.metadata.lens.as_mut() {
        l.gain_map.as_mut().unwrap().after_warp = true;
    }
    let warp = LensWarp::new(&im, &r).unwrap();
    let p = warp.sample(Source::new(&im), x, y, 0.);
    let m = &warp.map;
    let ([dx, dy], _) = m.scales(x, y);
    let g = warp
        .gain_map
        .as_ref()
        .unwrap()
        .gain(m.center[0] + dx - 0.5, m.center[1] + dy - 0.5);
    for c in 0..3 {
        assert!((p[c] - 0.1 * g[c]).abs() < 1e-5, "{c}: {p:?} {g:?}");
    }
    assert!(g[0] > red_before + 0.01);
}
#[test]
fn transform_scales_and_fills_uncovered_area_with_white() {
    let im = fixture();
    let mut r = Recipe::for_metadata(&im.metadata);
    let plain = render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    r.transform.scale = 0.5;
    let small = render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    assert_eq!((small.width, small.height), (plain.width, plain.height));
    // Halving the scale leaves the corners uncovered and keeps the centre.
    assert_eq!(small.pixels[0], [1.; 3]);
    let centre = |x: &Rendered| x.pixels[(4 * x.width + 6) as usize];
    assert!((0..3).all(|c| (centre(&small)[c] - centre(&plain)[c]).abs() < 0.05));
    r.transform = crate::model::transform::Transform {
        vertical: 0.6,
        horizontal: -0.3,
        rotate: 4.,
        aspect: 0.2,
        offset_x: 0.1,
        ..Default::default()
    };
    assert!(r.validate().is_ok());
    let full = render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    let region = render_region(&im, &r.checked().unwrap(), [2, 1, 5, 4]).unwrap();
    for y in 0..4 {
        for x in 0..5 {
            let p = region.pixels[(y * 5 + x) as usize];
            let q = full.pixels[((y + 1) * full.width + x + 2) as usize];
            assert!((0..3).all(|c| (p[c] - q[c]).abs() < 2e-6));
        }
    }
    r.transform.scale = 2.;
    assert!(r.validate().is_err());
}
/// Where a DNG's transparency mask says the file has no image (a panorama's
/// empty corners), the render is white whatever the edit, as Lightroom shows
/// it and as an area a transform leaves uncovered; the rest renders as
/// without a mask. A mask at another size than the image (the photo's, over a
/// reduced copy) marks the same area.
#[test]
fn a_transparency_mask_renders_its_empty_area_white() {
    let plain = fixture();
    let mut r = Recipe::for_metadata(&plain.metadata);
    r.exposure = -1.5;
    let r = r.checked().unwrap();
    let without = render(&plain, &r, 0, Cancel::NEVER).unwrap();
    for scale in [1, 2] {
        let (w, h) = (12 * scale, 8 * scale);
        let values = (0..w * h)
            .map(|i| if i % w < 4 * scale { 0 } else { 255 })
            .collect();
        let mut im = fixture();
        im.metadata.coverage = Some(std::sync::Arc::new(
            crate::camera_data::Coverage::new(w, h, values).unwrap(),
        ));
        let out = render(&im, &r, 0, Cancel::NEVER).unwrap();
        let region = render_region(&im, &r, [0, 0, 12, 8]).unwrap();
        for y in 0..8 {
            for x in 0..12 {
                let i = (y * 12 + x) as usize;
                if x < 4 {
                    assert_eq!(out.pixels[i], [1.; 3], "({x}, {y}) of a {w} × {h} mask");
                    assert_eq!(region.pixels[i], [1.; 3]);
                } else if x >= 6 {
                    let (p, q) = (out.pixels[i], without.pixels[i]);
                    assert!(
                        (0..3).all(|c| (p[c] - q[c]).abs() < 0.02),
                        "({x}, {y}): {p:?} {q:?}"
                    );
                }
            }
        }
    }
}
#[test]
fn fringe_selector_reaches_the_hue_defringe_tests() {
    // A purple and a green fringe as Defringe sees them.
    for (hue, range) in [(0.85f32, 0), (0.45, 1)] {
        let angle = hue * std::f32::consts::TAU;
        let lab = [0.6, angle.cos() * 0.08, angle.sin() * 0.08];
        let mut r = Recipe::default();
        let shown = finish_color(lab);
        let chroma = |lab: [f32; 3]| lab[1].hypot(lab[2]);
        assert_eq!(pick_fringe(&mut r, shown), Some(range));
        assert!(chroma(r.effects.defringe_color(lab, hue)) < chroma(lab) * 0.6);
    }
}

/// Camera Raw 18.7's master point curve on saturated colors uses a C1-smooth
/// hue-preserving secant (not plain RGBTone) and Refine Saturation 0 takes its
/// Photoshop luma (`0.30 R + 0.59 G + 0.11 B`) from the per-channel curved color
/// rather than the master-curved color.
#[test]
fn strong_s_master_curve_and_refine_saturation_match_camera_raw_18_7() {
    use crate::camera_profiles::{PRO_TO_RGB, RGB_TO_PRO};
    let recipe = Recipe {
        curve: ToneCurve {
            points: vec![
                [0., 0.],
                [64. / 255., 28. / 255.],
                [128. / 255., 128. / 255.],
                [192. / 255., 228. / 255.],
                [1., 1.],
            ],
            ..Default::default()
        },
        ..Default::default()
    };
    let render_pro = |enc_pro: [f32; 3], amount: f32| {
        let mut r = recipe.clone();
        r.curve_saturation = amount;
        let lin_rgb = mul(PRO_TO_RGB, enc_pro.map(srgb_decode));
        let out_rgb = apply_reference_curves(lin_rgb, &r, &CurveSet::new(&r), None);
        mul(RGB_TO_PRO, out_rgb).map(|v| srgb_encode(v.clamp(0., 1.)))
    };

    // Measured Camera Raw 18.7 encoded ProPhoto outputs on synthetic-d65 strong S-curve
    // at Refine Saturation = 100: #270 (ColorChecker 7), #92 (saturated purple),
    // #75 (dark orange), and #242 (wide-gamut red with clipped endpoints).
    let cases_100: [([f32; 3], [f32; 3]); 4] = [
        ([0.8330, 0.6445, 0.2894], [0.9909, 0.6985, 0.1475]),
        ([0.4987, 0.2109, 0.8231], [0.4860, 0.0488, 0.9788]),
        ([0.5186, 0.3253, 0.0246], [0.5044, 0.3070, 0.0000]),
        ([0.9126, 0.2808, 0.0000], [1.0000, 0.3077, 0.0000]),
    ];
    for (input, want) in cases_100 {
        let got = render_pro(input, 1.);
        for c in 0..3 {
            assert!(
                (got[c] - want[c]).abs() < 0.002,
                "100% {input:?}: got {got:?}, want {want:?}"
            );
        }
    }

    // Measured Camera Raw 18.7 encoded ProPhoto outputs at Refine Saturation = 0 and 50
    // for #270 and #75: 0% takes target luma [0.30, 0.59, 0.11] from per-channel f(RGB),
    // and 50% blends linearly between 0% and 100%.
    let cases_refine: [([f32; 3], [f32; 3], [f32; 3]); 2] = [
        (
            [0.8330, 0.6445, 0.2894],
            [0.9178, 0.7294, 0.3742],
            [0.9543, 0.7139, 0.2608],
        ),
        (
            [0.5186, 0.3253, 0.0246],
            [0.4228, 0.2573, 0.0000],
            [0.4636, 0.2821, 0.0000],
        ),
    ];
    let luma = |p: [f32; 3]| 0.30 * p[0] + 0.59 * p[1] + 0.11 * p[2];
    let lut = CurveLut::new(&recipe.curve);
    for (input, want_0, want_50) in cases_refine {
        let none = render_pro(input, 0.);
        let half = render_pro(input, 0.5);
        let full = render_pro(input, 1.);
        let per_ch = input.map(|v| lut.evaluate(v));
        assert!(
            (luma(none) - luma(per_ch)).abs() < 1e-3,
            "0% luma {input:?}: got {} vs per-channel {}",
            luma(none),
            luma(per_ch)
        );
        for c in 0..3 {
            assert!(
                (none[c] - want_0[c]).abs() < 0.002,
                "0% {input:?}: got {none:?}, want {want_0:?}"
            );
            assert!(
                (half[c] - want_50[c]).abs() < 0.002,
                "50% {input:?}: got {half:?}, want {want_50:?}"
            );
            assert!(
                (half[c] - 0.5 * (none[c] + full[c])).abs() < 1e-3,
                "50% blend {input:?}: got {half:?}, none={none:?}, full={full:?}"
            );
        }
    }
}

/// Refine Saturation 0 keeps each color's channel spread (encoded ProPhoto, before the
/// point curve) and takes its Photoshop luma (`0.30 R + 0.59 G + 0.11 B`) from the
/// per-channel curved color; other amounts blend linearly, and amounts above 1 render as
/// 1 (Camera Raw 18.7 on the synthetic chart).
#[test]
fn refine_saturation_zero_keeps_the_colours_saturation_through_the_point_curve() {
    let mut recipe = Recipe {
        ..Default::default()
    };
    recipe.curve = ToneCurve {
        points: vec![[0., 0.], [0.25, 0.1], [0.75, 0.9], [1., 1.]],
        ..Default::default()
    };
    let pro = |rgb: [f32; 3]| mul(crate::camera_profiles::RGB_TO_PRO, rgb).map(srgb_encode);
    let spread = |p: [f32; 3]| {
        p.iter().copied().fold(f32::MIN, f32::max) - p.iter().copied().fold(f32::MAX, f32::min)
    };
    let luma = |p: [f32; 3]| 0.30 * p[0] + 0.59 * p[1] + 0.11 * p[2];
    let input = [0.25, 0.08, 0.04];
    let lut = CurveLut::new(&recipe.curve);
    let per_channel = pro(input).map(|v| lut.evaluate(v));
    let render = |amount: f32| {
        let mut r = recipe.clone();
        r.curve_saturation = amount;
        pro(apply_reference_curves(input, &r, &CurveSet::new(&r), None))
    };
    let (full, none, half) = (render(1.), render(0.), render(0.5));
    assert!((spread(none) - spread(pro(input))).abs() < 1e-3, "{none:?}");
    assert!(spread(full) > spread(none) + 0.05, "{full:?} {none:?}");
    assert!((luma(none) - luma(per_channel)).abs() < 1e-3);
    for c in 0..3 {
        assert!(
            (half[c] - (full[c] + none[c]) / 2.).abs() < 1e-3,
            "{half:?}"
        );
    }
    assert_eq!(render(2.), full);
}
/// Constrain Crop renders without the white areas Vertical uncovers, at the crop's
/// aspect.
#[test]
fn constrain_crop_renders_no_white() {
    let m = Metadata {
        width: 90,
        height: 60,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let im = CameraImage {
        recovered: Default::default(),
        width: 90,
        height: 60,
        pixels: vec![[0.1; 3]; 90 * 60].into(),
        metadata: m,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    let mut r = Recipe::default();
    r.transform.vertical = 0.6;
    r.transform.rotate = 3.;
    let white = |out: &Rendered| {
        out.pixels
            .iter()
            .filter(|p| p.iter().all(|v| *v > 0.99))
            .count()
    };
    let free = render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    assert!(white(&free) > 100, "{}", white(&free));
    r.constrain_crop = true;
    let constrained = render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    assert_eq!(white(&constrained), 0);
    let aspect = constrained.width as f32 / constrained.height as f32;
    assert!((aspect - 1.5).abs() < 0.06, "{aspect}");
    assert!(constrained.width < free.width && constrained.width > 45);
}
/// A look's RGB table goes after the color controls: a fully desaturated gray still
/// takes the table's tint, and so does a black & white photo's gray, as Camera Raw 18.7
/// renders a color look under black and white (#1187).
#[test]
fn rgb_tables_follow_the_colour_controls() {
    let m = Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..Default::default()
    };
    let profile = crate::camera_profiles::CameraProfile::creative_for_test(&m)
        .with_test_rgb_tables()
        .remove(0);
    let r = Recipe {
        profile: Some(Arc::new(profile)),
        saturation: -1.,
        ..Default::default()
    };
    let spread = |r: &Recipe| {
        let lut = CurveSet::new(r);
        let rgb = profile_stage([0.05, 0.2, 0.1], r, &lut);
        let out = color_stage(rgb, r, &lut, None);
        out.iter().fold(0f32, |a, v| a.max(*v)) - out.iter().fold(1f32, |a, v| a.min(*v))
    };
    assert!(spread(&r) > 0.01);
    // Black & white comes before the table, which tints the gray.
    let mut mono = r;
    mono.effects.monochrome = true;
    assert!(spread(&mono) > 0.01, "{}", spread(&mono));
}
/// Black & white converts to gray before color grading, so grading tones the gray
/// in SDR and in an HDR master, as Camera Raw 18.7 renders split toning on black and
/// white (#1187).
#[test]
fn grading_tones_black_and_white() {
    let m = Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..Default::default()
    };
    let profile = crate::camera_profiles::CameraProfile::camera_matrix_default(&m).unwrap();
    let mut r = Recipe {
        profile: Some(Arc::new(profile)),
        ..Default::default()
    };
    r.effects.monochrome = true;
    let spread =
        |v: [f32; 3]| v.iter().fold(0f32, |a, x| a.max(*x)) - v.iter().fold(1f32, |a, x| a.min(*x));
    let sdr = |r: &Recipe| color_stage([0.05, 0.2, 0.1], r, &CurveSet::new(r), None);
    let hdr = |r: &Recipe| {
        let lut = CurveSet::new(r);
        lut.hdr
            .as_ref()
            .unwrap()
            .finish([0.05, 0.2, 0.1], r, &lut, None)
    };
    assert!(spread(sdr(&r)) < 1e-3, "{:?}", sdr(&r));
    // Shadows toward blue.
    r.grading[0] = [210. / 360., 0.5, 0.];
    let toned = sdr(&r);
    assert!(toned[2] > toned[0] + 0.01, "{toned:?}");
    r.hdr.enabled = true;
    r.hdr.limit = 4.;
    let toned = hdr(&r);
    assert!(toned[2] > toned[0] * 1.05, "{toned:?}");
    r.grading[0] = [0.; 3];
    assert!(spread(hdr(&r)) < 1e-4, "{:?}", hdr(&r));
}
/// A `.cube` imports as a look whose RGB table is the cube: an identity cube at 100%
/// renders as the base profile does, within the table's 16-bit rounding, at 0% exactly
/// as it does, and a cube that changes color changes it.
#[test]
fn an_identity_cube_renders_as_its_base_profile() {
    let m = Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..Default::default()
    };
    let base = crate::camera_profiles::open::standard(&m).unwrap();
    let cube = |size: usize, f: &dyn Fn([f32; 3]) -> [f32; 3]| {
        let last = (size - 1) as f32;
        let mut text = format!("LUT_3D_SIZE {size}\n");
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let [x, y, z] = f([r as f32 / last, g as f32 / last, b as f32 / last]);
                    text += &format!("{x} {y} {z}\n");
                }
            }
        }
        text
    };
    let identity = base.with_cube_for_test(&cube(33, &|p| p)).unwrap();
    let swap = base
        .with_cube_for_test(&cube(17, &|[r, g, b]| [g, b, r]))
        .unwrap();
    let recipe = |profile: &crate::camera_profiles::CameraProfile, amount: f32| Recipe {
        profile: Some(Arc::new(profile.clone())),
        profile_amount: amount,
        ..Default::default()
    };
    let plain = recipe(&base, 1.);
    // Profile Amount is folded into the look where rendering starts.
    let colour = |r: &Recipe, rgb: [f32; 3]| {
        let r = r.with_profile_adjustments();
        color_stage(rgb, &r, &CurveSet::new(&r), None)
    };
    let mut worst = 0f32;
    for i in 0..400u32 {
        let rgb = [
            (i * 37 % 101) as f32 / 101.,
            (i * 91 % 97) as f32 / 97.,
            (i * 53 % 89) as f32 / 89.,
        ]
        .map(|v| 0.6 * v);
        let want = colour(&plain, rgb);
        let got = colour(&recipe(&identity, 1.), rgb);
        worst = (0..3).fold(worst, |w, c| w.max((got[c] - want[c]).abs()));
        assert_eq!(colour(&recipe(&identity, 0.), rgb), want, "{rgb:?}");
        assert_eq!(colour(&recipe(&swap, 0.), rgb), want, "{rgb:?}");
    }
    assert!(worst < 2e-5, "{worst}");
    let rgb = [0.3, 0.1, 0.05];
    let (want, got) = (colour(&plain, rgb), colour(&recipe(&swap, 1.), rgb));
    assert!(
        got[2] > want[2] + 0.05 && got[0] < want[0] - 0.05,
        "{want:?} {got:?}"
    );
}
#[test]
fn point_colors_render_in_color_only_and_round_trip() -> anyhow::Result<()> {
    use crate::model::panels::{Panel, PanelState};
    use crate::model::point_color::PointColor;
    let im = fixture();
    let plain = Recipe {
        ..Default::default()
    };
    // A swatch whose ranges hold every color, so the fixture's greens change.
    let mut edited = plain.clone();
    edited.point_colors = vec![PointColor {
        shift: [0.5, -0.5, 0.3],
        hue_range: [0., 0., 1., 1.],
        saturation_range: [0., 0., 1., 1.],
        luminance_range: [0., 0., 1., 1.],
        range: 1.,
        ..PointColor::sampled([2., 0.5, 0.2])
    }];
    let pixels = |r: &Recipe| {
        crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER).map(|out| out.pixels)
    };
    assert_ne!(pixels(&edited)?, pixels(&plain)?);
    // Camera Raw leaves Point Color out of black & white.
    let mono = |r: &Recipe| {
        let mut r = r.clone();
        r.effects.monochrome = true;
        r
    };
    assert_eq!(pixels(&mono(&edited))?, pixels(&mono(&plain))?);
    // The Color Mixer's switch turns it off with the mixer.
    let mut off = edited.clone();
    off.panels.set(Panel::ColorMixer, PanelState::Off);
    assert_eq!(pixels(&off)?, pixels(&plain)?);
    // Saved with the recipe, and left out while there are none.
    let back: Recipe = serde_json::from_str(&serde_json::to_string(&edited)?)?;
    assert_eq!(back.point_colors, edited.point_colors);
    assert!(!serde_json::to_string(&plain)?.contains("point_colors"));
    Ok(())
}
#[test]
fn point_colors_dropper_samples_the_photo_as_rendered() -> anyhow::Result<()> {
    use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
    use crate::{develop::point_color::add_sample, model::point_color::PointColor};
    let im = fixture();
    let plain = Recipe {
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let pick = |r: &Recipe| {
        crate::develop::quality::point_color_pick(&im, r, 0.05, 0.5, Cancel::new(&cancel))
    };
    let before = pick(&plain)?;
    // A mask brightening the left of the photo: the dropper sees it, as the photo
    // shows it.
    let mut masked = plain.clone();
    masked.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
        })],
        adjust: LocalAdjust {
            exposure: 1.,
            ..Default::default()
        },
        ..Default::default()
    });
    let brighter = pick(&masked)?;
    assert!(brighter[2] > 1.3 * before[2], "{before:?} {brighter:?}");
    // A swatch picked there selects that color: Saturation −100 grays the spot.
    let mut edited = plain;
    let i = add_sample(&mut edited.point_colors, before).unwrap();
    edited.point_colors[i] = PointColor {
        shift: [0., -1., 0.],
        ..edited.point_colors[i]
    };
    assert!(pick(&edited)?[1] < 0.6 * before[1]);
    Ok(())
}
#[test]
fn visualize_range_leaves_what_it_does_not_select_gray_under_grading() -> anyhow::Result<()> {
    use crate::model::point_color::{PointColor, visualize_range};
    let im = fixture();
    let mut r = Recipe {
        ..Default::default()
    };
    // Color grading tints everything after Point Color.
    r.effects.global_grade = [0.6, 0.5, 0.];
    // A magenta swatch, which none of the fixture's greens is.
    r.point_colors = vec![PointColor::sampled([5., 0.8, 0.3])];
    r.point_colors = visualize_range(&r.point_colors, 0).unwrap();
    let out = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
    for p in &out.pixels {
        assert!(
            (p[0] - p[1]).abs() < 2e-3 && (p[1] - p[2]).abs() < 2e-3,
            "{p:?}"
        );
    }
    Ok(())
}
#[test]
fn visualize_range_leaves_color_range_masks_selecting_the_photo() -> anyhow::Result<()> {
    use crate::develop::point_color::visualize;
    use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
    use crate::model::point_color::{PointColor, visualize_range};
    let im = fixture();
    // Without sharpening, which works on luminance across neighbours and so differs
    // between a gray and the colors it grays.
    let mut r = Recipe {
        sharpening: 0.,
        ..Default::default()
    };
    // A magenta swatch, which none of the fixture's greens is.
    r.point_colors = vec![PointColor::sampled([5., 0.8, 0.3])];
    // A Color Range mask on the fixture's own green brightens it.
    let green = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?.pixels[40];
    r.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::ColorRange {
            samples: vec![srgb_to_lab(green.map(srgb_decode))],
            amount: 0.5,
        })],
        adjust: LocalAdjust {
            exposure: 1.,
            ..Default::default()
        },
        ..Default::default()
    });
    let shown = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
    let mut visualized = r.clone();
    visualized.point_colors = visualize_range(&r.point_colors, 0).unwrap();
    let gray = crate::develop::render(&im, &visualized.checked()?, 0, Cancel::NEVER)?;
    // The mask still brightens the greens: Visualize Range only grays them.
    for (a, b) in shown.pixels.iter().zip(&gray.pixels) {
        let expected = visualize(*a, 0.);
        assert!(
            (0..3).all(|c| (expected[c] - b[c]).abs() < 2e-3),
            "{a:?} {b:?}"
        );
    }
    Ok(())
}
#[test]
fn targeted_adjustments_sample_the_photo_where_each_control_sees_it() -> anyhow::Result<()> {
    use crate::develop::targeted::{HslChannel, Target, TargetWeights};
    use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
    // Four patches: a dark gray, an orange, a light gray and a blue.
    let patches = [
        [0.03, 0.03, 0.03],
        [0.5, 0.25, 0.08],
        [0.6, 0.6, 0.6],
        [0.05, 0.1, 0.4],
    ];
    let m = Metadata {
        width: 40,
        height: 10,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let im = CameraImage {
        recovered: Default::default(),
        width: 40,
        height: 10,
        pixels: (0..400).map(|i| patches[(i % 40) / 10]).collect(),
        metadata: m,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    let r = Recipe {
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let u = |patch: usize| (patch as f32 * 10. + 5.) / 40.;
    let sample = |r: &Recipe, patch| {
        crate::develop::quality::targeted_sample(&im, r, u(patch), 0.5, Cancel::new(&cancel))
    };
    // The rendered patch, encoded sRGB.
    let shown = |r: &Recipe, patch: usize| -> anyhow::Result<[f32; 3]> {
        let out = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
        Ok(out.pixels[5 * out.width as usize + patch * 10 + 5])
    };
    let luma = |p: [f32; 3]| crate::color::luminance(p);
    // Tone Curve: the region chosen is the one the patch's tone falls in, between the
    // splits, and raising it brightens the patch.
    for patch in [0, 2] {
        let s = sample(&r, patch)?;
        let w = TargetWeights::new(Target::ToneCurve, &s, &r);
        let chosen = (0..4).find(|i| w.shares[*i] == 1.).unwrap();
        assert_eq!(chosen, r.effects.parametric_region(s.tone), "patch {patch}");
        let mut moved = r.clone();
        moved.effects.parametric[chosen] = 1.;
        assert!(luma(shown(&moved, patch)?) > luma(shown(&r, patch)?) + 0.01);
    }
    let dark = sample(&r, 0)?.tone;
    assert!(dark < 0.25 && sample(&r, 2)?.tone > 0.5, "{dark}");
    // As rendered: a mask brightening the dark patch raises what the curve sees.
    let mut masked = r.clone();
    masked.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.2, 0.5],
            to: [0.3, 0.5],
        })],
        adjust: LocalAdjust {
            exposure: 2.,
            ..Default::default()
        },
        ..Default::default()
    });
    assert!(sample(&masked, 0)?.tone > dark + 0.1);
    // Color Mixer: orange leads on the orange patch, blue on the blue one, and a
    // drag up saturates the patch; the grays have no color to adjust.
    let saturation = |p: [f32; 3]| {
        let max = p.into_iter().fold(0f32, f32::max);
        (max - p.into_iter().fold(1f32, f32::min)) / max
    };
    let target = Target::Hsl(HslChannel::Saturation);
    for (patch, band) in [(1, 1), (3, 5)] {
        let w = TargetWeights::new(target, &sample(&r, patch)?, &r);
        assert_eq!(w.shares[band], 1., "patch {patch}: {:?}", w.shares);
        let mut moved = r.clone();
        w.apply(&r, 0.5, &mut moved);
        assert!(
            saturation(shown(&moved, patch)?) > saturation(shown(&r, patch)?) + 0.02,
            "patch {patch}"
        );
    }
    assert!(TargetWeights::new(target, &sample(&r, 2)?, &r).is_empty());
    // Black & white: the mix's own hue weights at the patch, and a drag up brightens it.
    let mut mono = r;
    mono.effects.monochrome = true;
    let s = sample(&mono, 1)?;
    let w = TargetWeights::new(Target::BlackWhite, &s, &mono);
    let hue = s.color[2]
        .atan2(s.color[1])
        .rem_euclid(std::f32::consts::TAU)
        / std::f32::consts::TAU;
    let weights = crate::develop::pipeline::hue_weights(hue);
    let main = (0..8)
        .max_by(|a, b| weights[*a].total_cmp(&weights[*b]))
        .unwrap();
    assert_eq!(w.shares[main], 1., "{:?} {weights:?}", w.shares);
    let mut moved = mono.clone();
    w.apply(&mono, 0.5, &mut moved);
    assert!(luma(shown(&moved, 1)?) > luma(shown(&mono, 1)?) + 0.02);
    Ok(())
}
/// A flat image of the synthetic lens photo, with its imported test profiles.
fn lens_photo() -> CameraImage {
    let mut m = crate::lens::choice::tests::photo();
    m.width = 90;
    m.height = 60;
    m.wb = [1.; 3];
    m.daylight_wb = [1.; 3];
    m.matrix = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
    // Listed for the photo's own size: corrections fit its frame.
    m.lens_profiles = crate::lens::choice::tests::library().for_photo(&m);
    CameraImage {
        recovered: Default::default(),
        width: 90,
        height: 60,
        pixels: vec![[0.1; 3]; 90 * 60].into(),
        metadata: m,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }
}
fn profile_recipe(setup: crate::lens::choice::LensProfileSetup, filename: &str) -> Recipe {
    Recipe {
        lens_profile: true,
        lens_profile_choice: crate::lens::choice::LensProfileChoice {
            setup,
            id: (!filename.is_empty()).then(|| {
                crate::lens::choice::LensProfileRef::Adobe(crate::lens::choice::LensProfileId {
                    name: String::new(),
                    filename: filename.into(),
                    digest: String::new(),
                    embedded: false,
                })
            }),
        },
        ..Default::default()
    }
}
/// The lens profile choice is left out of recipes at its default, so releases that
/// predate it open them, and round trips with the profile it names.
#[test]
fn lens_profile_choice_round_trips_and_is_omitted_by_default() {
    use crate::lens::choice::LensProfileSetup;
    let json = serde_json::to_value(Recipe::default()).unwrap();
    assert!(json.get("lens_profile_choice").is_none());
    let mut r = profile_recipe(LensProfileSetup::Custom, "Mine 35mm F2.lcp");
    r.lens_profile_choice.adobe_mut().unwrap().digest = "0123ABCD".into();
    let back: Recipe = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back.lens_profile_choice, r.lens_profile_choice);
    assert!(back.unknown.is_empty());
    let old: Recipe = serde_json::from_value(json).unwrap();
    assert!(old.lens_profile_choice.is_default());
}
/// The chosen profile is the one rendered; Default and Auto render the best match.
#[test]
fn the_chosen_lens_profile_renders() {
    use crate::lens::choice::{
        LensProfileSetup,
        tests::{ADOBE, MINE},
    };
    let im = lens_photo();
    let m = &im.metadata;
    let corner = |r: &Recipe| r.lens_correction(m).unwrap().vignetting_gain(1.);
    let auto = corner(&profile_recipe(LensProfileSetup::Default, ""));
    assert_eq!(auto, corner(&profile_recipe(LensProfileSetup::Auto, ADOBE)));
    let mine = corner(&profile_recipe(LensProfileSetup::Custom, MINE));
    assert!(auto > mine + 0.05 && mine > 1.01, "{auto} {mine}");
    // Off, nothing renders and nothing is reported.
    let off = Recipe {
        lens_profile: false,
        ..profile_recipe(LensProfileSetup::Custom, MINE)
    };
    assert!(off.lens_correction(m).is_none());
    assert_eq!(off.missing_lens_profile(m), None);
}
/// The Distortion and Vignetting amounts scale the chosen profile: 0 leaves the photo
/// as without it, 200 doubles the correction.
#[test]
fn lens_profile_amounts_scale_the_correction() {
    use crate::lens::choice::{LensProfileSetup, tests::OTHER};
    let im = lens_photo();
    let base = profile_recipe(LensProfileSetup::Custom, OTHER);
    let at = |distortion: f32, vignetting: f32| Recipe {
        lens_distortion: distortion,
        lens_vignetting: vignetting,
        ..base.clone()
    };
    let gain = |r: &Recipe| VignetteField::new(&im, r).unwrap().gain(0., 0.);
    let (g0, g1, g2) = (gain(&at(1., 0.)), gain(&at(1., 1.)), gain(&at(1., 2.)));
    assert_eq!(g0, 1.);
    assert!(g1 > 1.2 && (g2 - g1 * g1).abs() < 1e-4, "{g1} {g2}");
    let scale = |r: &Recipe| {
        let map = crate::develop::image_space::LensMap::new(&im, r).unwrap();
        map.lens.radial_scale_with(1., map.amount)[1]
    };
    let (s0, s1, s2) = (scale(&at(0., 1.)), scale(&at(1., 1.)), scale(&at(2., 1.)));
    assert_eq!(s0, 1.);
    assert!(
        s1 > 1.005 && ((s2 - 1.) - 2. * (s1 - 1.)).abs() < 1e-5,
        "{s1} {s2}"
    );
    // As rendered: corners brighten with the amount, and 0 matches no profile.
    let lum = |r: &Recipe| {
        let out = render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
        out.pixels[0].iter().sum::<f32>()
    };
    let none = lum(&Recipe::default());
    assert!((lum(&at(0., 0.)) - none).abs() < 1e-4);
    assert!(lum(&at(0., 2.)) > lum(&at(0., 1.)) + 0.01 && lum(&at(0., 1.)) > none + 0.01);
}
/// A profile's distortion never uncovers white, at any amount, and Constrain Crop
/// still crops out what the Transform sliders uncover through it.
#[test]
fn lens_profile_distortion_with_constrain_crop_renders_no_white() {
    use crate::lens::choice::{LensProfileSetup, tests::OTHER};
    let im = lens_photo();
    let white = |out: &Rendered| {
        out.pixels
            .iter()
            .filter(|p| p.iter().all(|v| *v > 0.99))
            .count()
    };
    let mut r = Recipe {
        lens_distortion: 2.,
        ..profile_recipe(LensProfileSetup::Custom, OTHER)
    };
    assert_eq!(
        white(&render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap()),
        0
    );
    r.transform.vertical = 0.6;
    assert!(white(&render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap()) > 100);
    r.constrain_crop = true;
    let constrained = render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    assert_eq!(white(&constrained), 0);
    let aspect = constrained.width as f32 / constrained.height as f32;
    assert!((aspect - 1.5).abs() < 0.06, "{aspect}");
}
/// Switching the Lens Corrections panel off renders no profile, keeps the choice and
/// reports nothing missing.
#[test]
fn lens_corrections_panel_off_bypasses_the_chosen_profile() {
    use crate::lens::choice::{LensProfileSetup, tests::MINE};
    use crate::model::panels::{Panel, PanelState};
    let im = lens_photo();
    let mut r = profile_recipe(LensProfileSetup::Custom, "Gone.lcp");
    assert!(r.missing_lens_profile(&im.metadata).is_some());
    r.lens_profile_choice = profile_recipe(LensProfileSetup::Custom, MINE).lens_profile_choice;
    r.panels.set(Panel::LensCorrections, PanelState::Off);
    assert!(r.as_rendered().lens_correction(&im.metadata).is_none());
    assert_eq!(r.lens_profile_choice.setup, LensProfileSetup::Custom);
    assert_eq!(
        render(&im, &r.checked().unwrap(), 0, Cancel::NEVER)
            .unwrap()
            .pixels,
        render(&im, &Recipe::default().checked().unwrap(), 0, Cancel::NEVER)
            .unwrap()
            .pixels
    );
    r.lens_profile_choice.id = profile_recipe(LensProfileSetup::Custom, "Gone.lcp")
        .lens_profile_choice
        .id;
    assert_eq!(r.missing_lens_profile(&im.metadata), None);
}
/// A profile the edit names that isn't imported is reported with what renders instead.
#[test]
fn a_missing_named_lens_profile_is_reported() {
    use crate::lens::choice::LensProfileSetup;
    let im = lens_photo();
    let m = &im.metadata;
    let mut custom = profile_recipe(LensProfileSetup::Custom, "Gone.lcp");
    custom.lens_profile_choice.adobe_mut().unwrap().name = "Adobe (Gone)".into();
    assert_eq!(
        custom.missing_lens_profile(m).as_deref(),
        Some("Lens profile \"Adobe (Gone)\" isn't imported; no lens correction")
    );
    assert!(custom.lens_correction(m).is_none());
    let auto = Recipe {
        lens_profile_choice: crate::lens::choice::LensProfileChoice {
            setup: LensProfileSetup::Auto,
            ..custom.lens_profile_choice.clone()
        },
        ..custom.clone()
    };
    assert_eq!(
        auto.missing_lens_profile(m).as_deref(),
        Some("Lens profile \"Adobe (Gone)\" isn't imported; using Adobe (Testcam 35mm F2)")
    );
    assert!(
        profile_recipe(LensProfileSetup::Auto, "")
            .missing_lens_profile(m)
            .is_none()
    );
}
/// Changing the lens profile choice is an edit of the Lens Corrections panel, so a
/// switched-off panel turns on with it.
#[test]
fn a_lens_profile_choice_belongs_to_the_lens_corrections_panel() {
    use crate::lens::choice::{LensProfileSetup, tests::MINE};
    use crate::model::panels::Panel;
    let before = profile_recipe(LensProfileSetup::Auto, "");
    let after = profile_recipe(LensProfileSetup::Custom, MINE);
    assert!(Panel::LensCorrections.holds_change(&before, &after));
}

/// The Contrast pivot and the Whites curve are the photo's: the same for the GPU's map pass (whose
/// parameters also run the final pass) as for the plain per-pixel parameters, and not
/// moved by the measured Texture.
#[test]
fn contrast_and_the_scene_stage_are_measured_on_the_photo_alone() {
    use crate::develop::basic_tone::TYPICAL_PIVOT;
    let mut im = fixture();
    im.metadata.cam_xyz = [
        [1.1434, -0.4948, -0.121],
        [-0.3746, 1.2042, 0.1903],
        [-0.0666, 0.1479, 0.52],
    ];
    // Dark with a few bright pixels, so the pivot is not the typical one.
    for (i, p) in im.pixels.iter_mut().enumerate() {
        *p = if i % 9 == 0 {
            [0.9; 3]
        } else {
            p.map(|v| v * 0.3)
        };
    }
    let profile =
        crate::camera_profiles::CameraProfile::camera_matrix_default(&im.metadata).unwrap();
    let r = Recipe {
        profile: Some(std::sync::Arc::new(profile)),
        contrast: 0.6,
        whites: 0.5,
        shadows: 0.3,
        ..Default::default()
    };
    let camera = camera_stage(&im.metadata, &r);
    let plain = CurveSet::for_image((&im).into(), &r, camera, false);
    let pivot = plain.photo.contrast_pivot;
    assert!((pivot - TYPICAL_PIVOT).abs() > 0.01, "{pivot}");
    assert_ne!(plain.measures, Default::default());
    let gpu = pixel_params::pixel_params((&im).into(), &r).unwrap();
    assert_eq!(gpu.get(pixel_params::slot::LOCAL_PIVOT), [pivot]);
    let map_pass = CurveSet::with_photo_measures((&im).into(), &r, camera);
    assert_eq!(map_pass.photo, plain.photo);
    assert_eq!(map_pass.measures, plain.measures);
    assert_eq!(map_pass.basic.as_ref(), plain.basic.as_ref());
    // The measured Texture, which makes a new image, does not move them.
    let textured = crate::develop::texture::TextureDetail::of(&im, 1., Cancel::NEVER)
        .unwrap()
        .apply(&im, 1.);
    let source = Source {
        untextured: Some(&im),
        ..Source::new(&textured)
    };
    let textured = CurveSet::with_photo_measures(source, &r, camera);
    assert_eq!(textured.photo, plain.photo);
    assert_eq!(textured.measures, plain.measures);
}

/// An HDR edit's Contrast pivots where its SDR rendition's does, as Camera Raw's does:
/// the pivot is measured on the default SDR rendering, never on the HDR master.
#[test]
fn hdr_contrast_pivots_where_its_sdr_rendition_does() {
    let mut im = fixture();
    im.metadata.cam_xyz = [
        [1.1434, -0.4948, -0.121],
        [-0.3746, 1.2042, 0.1903],
        [-0.0666, 0.1479, 0.52],
    ];
    for (i, p) in im.pixels.iter_mut().enumerate() {
        *p = if i % 7 == 0 {
            [0.95; 3]
        } else {
            p.map(|v| v * 0.4)
        };
    }
    let profile =
        crate::camera_profiles::CameraProfile::camera_matrix_default(&im.metadata).unwrap();
    let mut r = Recipe {
        profile: Some(std::sync::Arc::new(profile)),
        contrast: -0.6,
        ..Default::default()
    };
    let camera = camera_stage(&im.metadata, &r);
    let sdr = CurveSet::for_image((&im).into(), &r, camera, false);
    r.hdr.enabled = true;
    r.hdr.limit = 4.;
    let hdr = CurveSet::for_image((&im).into(), &r, camera, false);
    assert_eq!(hdr.photo, sdr.photo);
    assert!(hdr.hdr.as_ref().is_some_and(|h| h.has_contrast()));
}

/// An HDR rendered file's Contrast pivot still matches its SDR rendition under strong
/// white balance, even though the HDR master leaves the camera stage's pre-matrix clip
/// unclipped (`camera_stage`).
#[test]
fn hdr_rendered_contrast_pivots_where_its_sdr_rendition_does() {
    let srgb: Vec<[u8; 3]> = (0..96u8)
        .map(|i| if i % 5 == 0 { [240; 3] } else { [50 + i; 3] })
        .collect();
    let im = rendered_fixture(&srgb);
    let mut sdr_recipe = Recipe {
        incremental_temperature: -50.,
        incremental_tint: 100.,
        contrast: 0.6,
        ..Recipe::for_rendered(&im.metadata)
    };
    sdr_recipe.update_wb(&im.metadata);
    let sdr_camera = camera_stage(&im.metadata, &sdr_recipe);
    let sdr = CurveSet::for_image((&im).into(), &sdr_recipe, sdr_camera, false);

    let mut hdr_recipe = sdr_recipe.clone();
    hdr_recipe.hdr.enabled = true;
    hdr_recipe.hdr.limit = 4.;
    let hdr_camera = camera_stage(&im.metadata, &hdr_recipe);
    assert_ne!(hdr_camera.clip, sdr_camera.clip);
    let hdr = CurveSet::for_image((&im).into(), &hdr_recipe, hdr_camera, false);
    assert_eq!(hdr.photo, sdr.photo);
}

/// A rendered file's Contrast pivots higher than a raw photo's of the same scene, as
/// Lightroom's does on JPEGs (`basic_tone::RENDERED_PIVOT_OFFSET`).
#[test]
fn a_rendered_photos_contrast_pivots_higher_than_a_raws() {
    use crate::develop::basic_tone::RENDERED_PIVOT_OFFSET;
    // Dark with a few bright pixels, so neither pivot is at its bounds.
    let srgb: Vec<[u8; 3]> = (0..96u8)
        .map(|i| if i % 9 == 0 { [240; 3] } else { [40 + i; 3] })
        .collect();
    let rendered = rendered_fixture(&srgb);
    let mut raw = rendered.clone();
    raw.metadata.source = rawmakase_model::camera_data::Source::Raw;
    let r = Recipe {
        contrast: 0.6,
        ..Recipe::for_rendered(&rendered.metadata)
    };
    let camera = camera_stage(&rendered.metadata, &r);
    let measures = CurveSet::with_photo_measures((&rendered).into(), &r, camera).measures;
    let pivot = |im: &CameraImage| tone::contrast_pivot(im.into(), &r, camera, &measures, None);
    let (rendered, raw) = (pivot(&rendered), pivot(&raw));
    assert!(
        (rendered - raw - RENDERED_PIVOT_OFFSET).abs() < 1e-6,
        "rendered {rendered}, raw {raw}"
    );
}

/// A look's parametric curve: a second curve after the user's, as Camera Raw 18.7
/// renders it.
#[test]
fn a_looks_parametric_curve_follows_the_users() {
    use crate::develop::parametric::curve;
    let mut m = fixture().metadata;
    m.cam_xyz = [
        [1.1434, -0.4948, -0.121],
        [-0.3746, 1.2042, 0.1903],
        [-0.0666, 0.1479, 0.52],
    ];
    let mut look = crate::camera_profiles::CameraProfile::creative_for_test(&m);
    let settings = &mut look.enhanced.as_mut().unwrap().settings;
    settings.parametric = [0., -0.15, -0.2, -0.33];
    settings.splits = [0.25, 0.5, 0.75];
    let mut r = Recipe {
        profile: Some(std::sync::Arc::new(look)),
        ..Default::default()
    };
    r.effects.parametric = [0., 0.3, 0., 0.];
    let user = curve([0., 0.3, 0., 0.], [0.25, 0.5, 0.75]).unwrap();
    let own = curve([0., -0.15, -0.2, -0.33], [0.25, 0.5, 0.75]).unwrap();
    let layered = r.with_profile_adjustments().into_owned();
    assert_eq!(layered.effects.parametric, [0., 0.3, 0., 0.]);
    let curve = CurveSet::new(&layered).parametric.unwrap();
    for x in [0.1, 0.3, 0.5, 0.7, 0.9] {
        assert!((curve.eval(x) - own.eval(user.eval(x))).abs() < 1e-3, "{x}");
    }
}

#[test]
fn measured_manual_vignetting_darkens_the_photo_not_the_crop() {
    let mut im = fixture();
    im.pixels = vec![[0.1; 3]; 96].into();
    let mut r = Recipe::for_metadata(&im.metadata);
    r.effects.lens_vignette = -0.5;
    let lum = |p: [f32; 3]| p.iter().sum::<f32>();
    let full = render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    let at = |im: &Rendered, x: u32, y: u32| lum(im.pixels[(y * im.width + x) as usize]);
    let (corner, centre) = (at(&full, 11, 0), at(&full, 6, 4));
    assert!(corner < centre * 0.9, "{corner} {centre}");
    // Lightroom's positive amounts lighten the corners.
    let lighter = Recipe {
        effects: crate::model::effects::Effects {
            lens_vignette: 0.5,
            ..r.effects.clone()
        },
        ..r.clone()
    };
    assert!(
        at(
            &render(&im, &lighter.checked().unwrap(), 0, Cancel::NEVER).unwrap(),
            11,
            0
        ) > centre * 1.1
    );
    // The gain belongs to the whole photo: a crop keeps each pixel's.
    let cropped = Recipe {
        crop: [0.5, 0., 1., 1.],
        ..r
    };
    let half = render(&im, &cropped.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    assert!((at(&half, half.width - 1, 0) - corner).abs() < 1e-3);
}

#[test]
fn out_of_gamut_channels_clip_as_camera_raw() {
    // Clipping keeps the channels inside sRGB as they are.
    let lab = srgb_to_lab([1.3, 0.2, -0.1]);
    let out = finish_color(lab).map(srgb_decode);
    assert!(
        (out[0] - 1.).abs() < 1e-5 && (out[1] - 0.2).abs() < 1e-3 && out[2] == 0.,
        "{out:?}"
    );
}
/// New edits render Grain as strong as Camera Raw 18.7 does: Amount 50, Size 25 on a
/// flat mid gray 1500 pixels wide measured an L* standard deviation of 7.3 there.
#[test]
fn new_edits_render_grain_at_camera_raw_strength() -> anyhow::Result<()> {
    let (width, height) = (1500, 40);
    let m = Metadata {
        width,
        height,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let im = CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: vec![[0.18; 3]; (width * height) as usize].into(),
        metadata: m.clone(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    let mut r = Recipe::with_profiles(&m, &[]);
    r.sharpening = 0.;
    r.noise_chroma = 0.;
    let flat = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
    r.effects.grain = 0.5;
    let grain = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
    let lightness = |p: &[f32; 3]| {
        let v = p[1];
        let y = if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        };
        116. * y.cbrt() - 16.
    };
    let n = flat.pixels.len() as f32;
    let deltas: Vec<f32> = (flat.pixels.iter().zip(&grain.pixels))
        .map(|(a, b)| lightness(b) - lightness(a))
        .collect();
    let mean = deltas.iter().sum::<f32>() / n;
    let deviation = (deltas.iter().map(|d| (d - mean).powi(2)).sum::<f32>() / n).sqrt();
    let gray = flat.pixels.iter().map(lightness).sum::<f32>() / n;
    assert!((35. ..75.).contains(&gray), "{gray}");
    assert!((5.8..8.8).contains(&deviation), "{deviation}");
    Ok(())
}
/// New edits start at Lightroom's Color noise reduction of 25, which takes out a
/// flat's color noise as Camera Raw does on photos.
#[test]
fn new_edits_reduce_colour_noise_as_camera_raw() -> anyhow::Result<()> {
    let (width, height) = (160, 120);
    let m = Metadata {
        width,
        height,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let noise = |x: u32, y: u32, c: u32| {
        let mut v =
            x.wrapping_mul(0x9e3779b9) ^ y.wrapping_mul(0x85ebca6b) ^ c.wrapping_mul(0xc2b2ae35);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2c1b3c6d);
        v ^= v >> 12;
        (v as f32 / u32::MAX as f32) * 2. - 1.
    };
    let im = CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: (0..width * height)
            .map(|i| {
                let (x, y) = (i % width, i / width);
                std::array::from_fn(|c| 0.18 * (1. + 0.08 * noise(x, y, c as u32)))
            })
            .collect(),
        metadata: m.clone(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    let mut r = Recipe::with_profiles(&m, &[]);
    assert_eq!(r.noise_chroma, 0.25);
    r.sharpening = 0.;
    let chroma_noise = |r: &Recipe| -> anyhow::Result<f32> {
        let out = crate::develop::render(&im, &r.checked()?, 0, Cancel::NEVER)?;
        let ab: Vec<[f32; 2]> = out
            .pixels
            .iter()
            .map(|p| [p[0] - p[1], p[2] - p[1]])
            .collect();
        let n = ab.len() as f32;
        let mean = ab
            .iter()
            .fold([0.; 2], |a, v| [a[0] + v[0] / n, a[1] + v[1] / n]);
        Ok((ab
            .iter()
            .map(|v| (v[0] - mean[0]).powi(2) + (v[1] - mean[1]).powi(2))
            .sum::<f32>()
            / n)
            .sqrt())
    };
    let on = chroma_noise(&r)?;
    let mut off = r;
    off.noise_chroma = 0.;
    let off = chroma_noise(&off)?;
    let ratio = on / off;
    assert!(ratio < 0.15, "{ratio}");
    Ok(())
}
/// Issue 465: at ISO 2000 a photo's color noise, demosaiced, forms blotches several
/// pixels across, which Lightroom's default Color noise reduction takes out. They are
/// taken out; the noise is measured in the photo, so a photo four times as noisy loses
/// as much of it.
#[test]
fn colour_blotches_are_taken_out() -> anyhow::Result<()> {
    let (width, height) = (256, 256);
    let m = Metadata {
        width,
        height,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let hash = |x: u32, y: u32, c: u32| {
        let mut v =
            x.wrapping_mul(0x9e3779b9) ^ y.wrapping_mul(0x85ebca6b) ^ c.wrapping_mul(0xc2b2ae35);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2c1b3c6d);
        v ^= v >> 12;
        (v as f32 / u32::MAX as f32) * 2. - 1.
    };
    // Fine noise plus blotches: noise on a 4-pixel grid, interpolated.
    let blotch = |x: u32, y: u32, c: u32| {
        let (fx, fy) = ((x % 4) as f32 / 4., (y % 4) as f32 / 4.);
        let (gx, gy) = (x / 4, y / 4);
        let at = |dx, dy| hash(gx + dx, gy + dy, c + 7);
        (at(0, 0) * (1. - fx) + at(1, 0) * fx) * (1. - fy)
            + (at(0, 1) * (1. - fx) + at(1, 1) * fx) * fy
    };
    let image = |amplitude: f32| CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: (0..width * height)
            .map(|i| {
                let (x, y) = (i % width, i / width);
                std::array::from_fn(|c| {
                    let c = c as u32;
                    0.18 * (1. + amplitude * (0.5 * hash(x, y, c) + blotch(x, y, c)))
                })
            })
            .collect(),
        metadata: m.clone(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    // The color noise of 8×8 block averages: the blotches, without the finest noise.
    let blotches = |im: &CameraImage, r: &Recipe| -> anyhow::Result<f32> {
        let out = crate::develop::render(im, &r.checked()?, 0, Cancel::NEVER)?;
        let blocks: Vec<[f32; 2]> = (0..(width / 8) * (height / 8))
            .map(|b| {
                let (bx, by) = (b % (width / 8) * 8, b / (width / 8) * 8);
                let mut s = [0.; 2];
                for y in by..by + 8 {
                    for x in bx..bx + 8 {
                        let p = out.pixels[(y * width + x) as usize];
                        s = [s[0] + (p[0] - p[1]) / 64., s[1] + (p[2] - p[1]) / 64.];
                    }
                }
                s
            })
            .collect();
        let n = blocks.len() as f32;
        let mean = blocks
            .iter()
            .fold([0.; 2], |a, v| [a[0] + v[0] / n, a[1] + v[1] / n]);
        Ok((blocks
            .iter()
            .map(|v| (v[0] - mean[0]).powi(2) + (v[1] - mean[1]).powi(2))
            .sum::<f32>()
            / n)
            .sqrt())
    };
    let mut r = Recipe::with_profiles(&m, &[]);
    r.sharpening = 0.;
    let mut off = r.clone();
    off.noise_chroma = 0.;
    for amplitude in [0.04, 0.16] {
        let im = image(amplitude);
        let before = blotches(&im, &off)?;
        let kept = blotches(&im, &r)? / before;
        assert!(kept < 0.3, "{amplitude}: {kept}");
    }
    Ok(())
}

fn noisy_luminance_fixture() -> CameraImage {
    let mut im = fixture();
    im.width = 128;
    im.height = 128;
    im.metadata.width = 128;
    im.metadata.height = 128;
    let hash = |x: u32, y: u32| {
        let mut v = x.wrapping_mul(0x9e3779b9) ^ y.wrapping_mul(0x85ebca6b);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2c1b3c6d);
        v ^= v >> 12;
        v as f32 / u32::MAX as f32 - 0.5
    };
    im.pixels = (0..128 * 128)
        .map(|i| {
            let (x, y) = (i % 128, i / 128);
            [0.1 + 0.02 * hash(x, y) + 0.025 * hash(x / 4, y / 4); 3]
        })
        .collect();
    im
}

/// Issue #386: luminance NR must suppress multi-pixel blotches, not only soften
/// single-pixel grain. Measure the rendered flat in 4×4 blocks, as in the photo
/// investigation, relative to the same render at Amount 0.
#[test]
fn luminance_noise_reduces_coarse_capture_noise() -> Result<()> {
    let im = noisy_luminance_fixture();
    let mut r = Recipe {
        sharpening: 0.,
        noise_chroma: 0.,
        ..Recipe::for_metadata(&im.metadata)
    };
    let energy = |out: &Rendered| {
        let mut means = Vec::new();
        for by in 4..28 {
            for bx in 4..28 {
                let mut sum = 0.;
                for y in by * 4..by * 4 + 4 {
                    for x in bx * 4..bx * 4 + 4 {
                        sum += out.pixels[y * 128 + x][1] / 16.;
                    }
                }
                means.push(sum);
            }
        }
        let mean = means.iter().sum::<f32>() / means.len() as f32;
        means.iter().map(|v| (v - mean).powi(2)).sum::<f32>()
    };
    let baseline = energy(&render(&im, &r.checked()?, 0, Cancel::NEVER)?);
    r.noise_luma = 0.5;
    let reduced = energy(&render(&im, &r.checked()?, 0, Cancel::NEVER)?);
    let retained = (reduced / baseline).sqrt();
    assert!(retained < 0.65, "coarse noise retained: {retained}");
    Ok(())
}

#[test]
fn color_detail_does_not_control_luminance_noise_reduction() -> Result<()> {
    let im = noisy_luminance_fixture();
    let mut r = Recipe {
        noise_luma: 0.5,
        noise_chroma: 0.,
        sharpening: 0.,
        ..Recipe::for_metadata(&im.metadata)
    };
    r.effects.chroma_detail = 0.;
    let low = render(&im, &r.checked()?, 0, Cancel::NEVER)?;
    r.effects.chroma_detail = 1.;
    let high = render(&im, &r.checked()?, 0, Cancel::NEVER)?;
    assert_eq!(low.pixels, high.pixels);
    Ok(())
}

#[test]
fn lens_ca_protects_finite_stripes_at_every_phase() {
    for period in [3.03, 3.6, 3.8, 4.] {
        for phase in [0., std::f32::consts::FRAC_PI_4, std::f32::consts::FRAC_PI_2] {
            let error = lens_ca_stripe_error(period, phase);
            assert!(error < 0.001, "period={period} phase={phase}: {error}");
        }
    }
}

fn lens_ca_stripe_error(period: f32, phase: f32) -> f32 {
    use crate::optics::{LensCorrection, Radial};
    let mut im = fixture();
    im.width = 192;
    im.height = 192;
    im.metadata.width = im.width;
    im.metadata.height = im.height;
    im.metadata.xtrans = true;
    im.pixels = (0..192 * 192)
        .map(|i| {
            let l = 0.1
                + if (76..112).contains(&(i / 192)) {
                    0.06 * (std::f32::consts::TAU * (i % 192) as f32 / period + phase).sin()
                } else {
                    0.
                };
            [1.08 * l, l, 0.92 * l]
        })
        .collect();
    im.metadata.lens = Some(LensCorrection {
        source: "test".into(),
        default_on: true,
        vignetting: None,
        distortion: None,
        chromatic: Some([
            Radial {
                knots: vec![0., 1.],
                values: vec![1.004; 2],
            },
            Radial {
                knots: vec![0., 1.],
                values: vec![1.; 2],
            },
        ]),
        gain_map: None,
        ..Default::default()
    });
    let r = Recipe::for_metadata(&im.metadata);
    let warp = LensWarp::new(&im, &r).unwrap();
    let mut error = 0f32;
    for y in 75..113 {
        for x in 30..162 {
            let p = warp.sample((&im).into(), x as f32, y as f32, 0.);
            error = error.max((p[0] / p[1] - 1.08).abs());
        }
    }
    error
}

#[test]
fn stripe_ca_guard_keeps_isolated_edges_resolved_colour_and_noise() {
    use crate::optics::{LensCorrection, Radial};
    let mut im = fixture();
    im.width = 192;
    im.height = 64;
    im.metadata.width = im.width;
    im.metadata.height = im.height;
    im.metadata.xtrans = true;
    im.metadata.lens = Some(LensCorrection {
        source: "test".into(),
        default_on: true,
        vignetting: None,
        distortion: None,
        chromatic: Some([
            Radial {
                knots: vec![0., 1.],
                values: vec![1.004; 2],
            },
            Radial {
                knots: vec![0., 1.],
                values: vec![0.997; 2],
            },
        ]),
        gain_map: None,
        ..Default::default()
    });
    for case in 0..5 {
        let mut seed = 0x23456789u32;
        im.pixels = (0..192 * 64)
            .map(|i| {
                let x = (i % 192) as f32;
                if case == 1 {
                    let v = (std::f32::consts::TAU * x / 3.03).sin();
                    [0.1 + 0.07 * v, 0.1 - 0.06 * v, 0.1 + 0.04 * v]
                } else if case == 3 {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    let l = 0.08 + 0.04 * (seed >> 8) as f32 / 16777216.;
                    [1.08 * l, l, 0.92 * l]
                } else if case == 2 {
                    std::array::from_fn(|_| {
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        0.1 + 0.04 * ((seed >> 8) as f32 / 16777216. - 0.5)
                    })
                } else {
                    let edge = |x: f32| 0.03 + 0.2 / (1. + (-(x - 70.) / 2.).exp());
                    let slat = if case == 4 && x < 68. {
                        1. + 0.25 * (std::f32::consts::TAU * (i / 192) as f32 / 3.03).sin()
                    } else {
                        1.
                    };
                    [edge(x + 0.3) * slat, edge(x) * slat, edge(x - 0.2) * slat]
                }
            })
            .collect();
        let mut unguarded = im.clone();
        unguarded.metadata.xtrans = false;
        let r = Recipe::for_metadata(&im.metadata);
        let a = LensWarp::new(&im, &r).unwrap();
        let b = LensWarp::new(&unguarded, &r).unwrap();
        let mut changed = 0;
        for y in 10..54 {
            for x in 10..182 {
                // The textured side may need alias protection; the ordinary
                // CA edge beside it must retain the original correction.
                if case == 4 && x < 68 {
                    continue;
                }
                let p = a.sample((&im).into(), x as f32, y as f32, 0.);
                let q = b.sample((&unguarded).into(), x as f32, y as f32, 0.);
                if case == 3 {
                    changed += usize::from(p.iter().zip(q).any(|(a, b)| (a - b).abs() > 1e-5));
                } else {
                    assert_eq!(p, q, "changed case={case} at {x},{y}");
                }
            }
        }
        assert!(changed < 44 * 172 / 20, "noise pixels changed: {changed}");
    }
}

/// A Lensfun lens the edit chose renders from its own copy of the calibration: the
/// warp carries it, with lateral CA read at green's source radius, on the CPU and in
/// the GPU's parameters, and with nothing installed.
#[test]
fn a_lensfun_choice_reaches_the_warp_and_the_gpu() {
    use crate::lens::lensfun::fixture;
    let metadata = fixture::photo();
    let im = CameraImage {
        width: metadata.width,
        height: metadata.height,
        pixels: vec![[0.2; 3]; (metadata.width * metadata.height) as usize].into(),
        metadata,
        recovered: Default::default(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    let mut r = Recipe {
        lens_profile: true,
        lens_distortion: 2.,
        ..Default::default()
    };
    r.lens_profile_choice.choose_lensfun(fixture::choice());
    let warp = LensWarp::new(&im, &r).unwrap();
    assert!(warp.map.lens.source.starts_with("Lensfun"));
    assert!(warp.map.lens.chromatic_at_source);
    assert_eq!(warp.map.amount, 2.);
    assert!(warp.vignetting.is_some());
    let mut tables = Vec::new();
    let params = super::gpu_params::lens_gpu_params(&im, &r, 0, &mut tables);
    assert_eq!(params[0], 2., "lateral CA at the source radius");
    assert!(params[8] >= 0. && params[10] >= 0., "red and blue tables");
    // Switched off, the edit renders no Lensfun correction.
    r.lens_profile = false;
    assert!(LensWarp::new(&im, &r).is_none_or(|w| !w.map.lens.source.starts_with("Lensfun")));
}
/// A render uses a small part of a worker's stack. Rayon can run another render's
/// task on a thread that waits inside a render, so renders nest on one stack: with
/// each render's curve tables on its stack (over 768 KiB for this one), two or three
/// of them overflowed Rayon's 2 MiB threads in the color tests. An overflow aborts
/// the test binary and names this test's thread.
#[test]
fn a_render_fits_in_an_eighth_of_a_worker_stack() -> anyhow::Result<()> {
    let curve = ToneCurve {
        points: vec![[0., 0.], [0.25, 0.3], [0.75, 0.7], [1., 1.]],
        ..Default::default()
    };
    // An HDR edit with every point curve holds the most curve tables: its master
    // and its SDR rendition (which `render` gives) are both checked.
    let mut im = fixture();
    im.metadata.cam_xyz = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
    let mut r = Recipe {
        profile: crate::camera_profiles::CameraProfile::camera_matrix_default(&im.metadata)
            .map(std::sync::Arc::new),
        exposure: 0.4,
        contrast: 0.3,
        ..Default::default()
    };
    r.hdr.enabled = true;
    r.curve = curve.clone();
    r.effects.channels = [curve.clone(), curve.clone(), curve];
    let r = r.checked()?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .stack_size(256 * 1024)
        .thread_name(|_| "a_render_fits_in_an_eighth_of_a_worker_stack".into())
        .build()?;
    pool.install(|| {
        crate::develop::render(&im, &r, 0, Cancel::NEVER)?;
        crate::develop::render_hdr(&im, &r, None, 0, Cancel::NEVER)
    })?;
    Ok(())
}

/// Masked Texture applies the same positive-channel guard as global `TextureDetail::apply`
/// so a whole-photo Texture mask renders signed below-black pixels identically to the
/// global Texture slider without amplifying negative noise.
#[test]
fn masked_texture_matches_global_texture_on_signed_pixels() -> anyhow::Result<()> {
    use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};

    let (w, h) = (32u32, 32u32);
    let mut im = fixture();
    im.width = w;
    im.height = h;
    im.metadata.width = w;
    im.metadata.height = h;
    im.pixels = (0..(w * h) as usize)
        .map(|i| {
            let (x, y) = (i % w as usize, i / w as usize);
            let checker = if (x + y) % 2 == 0 { 0.04 } else { 0.002 };
            if y < 12 {
                [-0.015 * ((x % 3) as f32 + 1.), checker - 0.01, -0.008]
            } else {
                [checker, checker * 0.8, checker * 1.1]
            }
        })
        .collect::<Vec<_>>()
        .into();

    let plain = crate::develop::render(&im, &Recipe::default().checked()?, 0, Cancel::NEVER)?;

    let mut global_recipe = Recipe::default();
    global_recipe.effects.texture = 0.8;
    let global = crate::develop::render(&im, &global_recipe.checked()?, 0, Cancel::NEVER)?;
    assert_ne!(global.pixels, plain.pixels);

    let mut masked_recipe = Recipe::default();
    masked_recipe.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.5, 2.0],
            to: [0.5, 3.0],
        })],
        adjust: LocalAdjust {
            texture: 0.8,
            ..Default::default()
        },
        ..Default::default()
    });
    let masked = crate::develop::render(&im, &masked_recipe.checked()?, 0, Cancel::NEVER)?;

    assert_eq!(global.pixels, masked.pixels);
    Ok(())
}

/// Camera Raw 18.7 runs RAW SDR Saturation before `ProfileLookTable` and RAW SDR
/// Vibrance between `ProfileLookTable` and `ProfileToneCurve`.
#[test]
fn raw_sdr_saturation_precedes_profile_look_and_vibrance_follows_it() {
    let m = Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..Default::default()
    };
    let profile = Arc::new(
        crate::camera_profiles::CameraProfile::camera_matrix_default(&m)
            .unwrap()
            .with_test_tables(),
    );
    let pro = [0.42, 0.24, 0.14];
    let r = Recipe {
        profile: Some(profile.clone()),
        saturation: 0.6,
        vibrance: -0.5,
        ..Default::default()
    };
    let lut = CurveSet::new(&r);
    let actual = profile_stage(pro, &r, &lut);
    let sat = crate::develop::color_mixer::apply_raw_saturation(pro, 0.6);
    let vib = crate::develop::color_mixer::RawVibrance::new(-0.5).unwrap();
    let expected = profile.finish_with(sat, |p| vib.apply(p));
    assert_eq!(actual, expected);
    // Swapping Saturation after the profile's look tables produces a visibly different color.
    let wrong_order = profile.finish_with(pro, |p| {
        vib.apply(crate::develop::color_mixer::apply_raw_saturation(p, 0.6))
    });
    let diff: f32 = actual
        .iter()
        .zip(&wrong_order)
        .map(|(a, b)| (a - b).abs())
        .fold(0., f32::max);
    assert!(
        diff > 1e-3,
        "expected look-table ordering difference, got {diff}"
    );
}

/// A positive photo of scene exposures within the synthetic film's range: smooth
/// gradients across about 1.9 decades, some saturated, and a grey patch.
fn film_scene() -> CameraImage {
    crate::develop::negative::tests::image(48, 32, |x, y| {
        if (20..28).contains(&x) && (12..20).contains(&y) {
            return [0.18; 3];
        }
        let (x, y) = (x as f32 / 47., y as f32 / 31.);
        [
            10f32.powf(-1.9 + 1.85 * x),
            10f32.powf(-1.9 + 1.85 * y),
            10f32.powf(-1.9 + 1.85 * (1. - x) * (0.5 + 0.5 * y)),
        ]
    })
}
/// The synthetic colour film's scan of [`film_scene`], and the recipes that render
/// the positive and the scan with its conversion: the same but for it.
fn scanned_scene(plain: Recipe) -> (CameraImage, CameraImage, Recipe, Recipe) {
    use crate::develop::negative::synthetic::COLOUR;
    use crate::model::negative::FilmType;
    let positive = film_scene();
    let negative = COLOUR.scanned(&positive);
    let converted = Recipe {
        negative: Some(COLOUR.conversion(FilmType::Colour)),
        ..plain.clone()
    };
    (positive, negative, plain, converted)
}

/// A scanned negative with its conversion renders as the positive it was made from:
/// the scene tone stage's input, the photo's measures and white point, and the
/// finished render with every control that reads the photo (Shadows and Highlights'
/// map, Contrast's pivot, Dehaze's haze), as the conversion gives the scene back
/// within the film's range.
#[test]
fn a_converted_negative_renders_as_its_positive() {
    let (positive, negative, plain, converted) = scanned_scene(scene_stage_recipe());
    let close = |a: f32, b: f32, tolerance: f32| (a - b).abs() <= tolerance;
    let tap = |im: &CameraImage, r: &Recipe| {
        let camera = camera_stage(&im.metadata, r);
        let mut lut = CurveSet::with_photo_measures(im.into(), r, camera);
        lut.output = PixelOutput::SceneInput;
        let pixels: Vec<[f32; 3]> = im
            .pixels
            .iter()
            .map(|p| process_pixel(*p, r, &lut, camera, [0., 0.], None))
            .collect();
        (lut.measures, pixels)
    };
    let (want, scene) = tap(&positive, &plain);
    let (got, converted_scene) = tap(&negative, &converted);
    for (a, b) in scene.iter().zip(&converted_scene) {
        assert!(
            (0..3).all(|c| close(a[c], b[c], 2e-3 * a[c].abs().max(1e-3))),
            "{a:?} {b:?}"
        );
    }
    // In stops. The white is the conversion's, at 1 as the positive's sensor clips:
    // the scan's own clip is its film base.
    for (name, a, b) in [
        ("sensor white", want.sensor_white, got.sensor_white),
        ("max", want.max, got.max),
        ("min", want.min, got.min),
        ("dark", want.dark, got.dark),
        ("p99", want.p99, got.p99),
        ("whites p99", want.whites_p99, got.whites_p99),
    ] {
        assert!(close(a, b, 0.01), "{name}: {a} {b}");
    }
    let shown =
        |im: &CameraImage, r: &Recipe| render(im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    let (a, b) = (shown(&positive, &plain), shown(&negative, &converted));
    let worst = a
        .pixels
        .iter()
        .zip(&b.pixels)
        .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs()))
        .fold(0f32, f32::max);
    assert!(worst < 2. / 255., "{worst}");
    // And without its conversion, the scan renders as the negative it is.
    assert_ne!(shown(&negative, &plain).pixels, a.pixels);
}

/// Black & white film converts to equal channels before white balance: a scan of a
/// toned base renders as greys at neutral white balance.
#[test]
fn a_converted_black_and_white_negative_renders_grey() {
    use crate::develop::negative::synthetic::BLACK_WHITE;
    use crate::model::negative::FilmType;
    let im = BLACK_WHITE.scanned(&film_scene());
    let r = Recipe {
        negative: Some(BLACK_WHITE.conversion(FilmType::BlackWhite)),
        ..Default::default()
    };
    let camera = camera_stage(&im.metadata, &r);
    let mut lut = CurveSet::with_photo_measures((&im).into(), &r, camera);
    lut.output = PixelOutput::SceneInput;
    for p in &im.pixels {
        let out = process_pixel(*p, &r, &lut, camera, [0., 0.], None);
        let spread =
            out.iter().copied().fold(0., f32::max) - out.iter().copied().fold(1., f32::min);
        assert!(spread < 1e-4 * out[1].max(1e-3), "{out:?}");
    }
}

/// The white balance picker reads a scanned negative's positive: picking a grey of the
/// scene gives the gains picking it on the positive gives.
#[test]
fn the_neutral_picker_reads_the_converted_negative() {
    let (mut positive, _, plain, converted) = scanned_scene(Recipe::default());
    // A grey under warm light.
    for (i, p) in positive.pixels.iter_mut().enumerate() {
        if (8..24).contains(&(i / 48)) {
            *p = [0.3, 0.2, 0.12];
        }
    }
    let negative = crate::develop::negative::synthetic::COLOUR.scanned(&positive);
    let want = neutral_pick(&positive, &plain, 0.5, 0.5);
    let got = neutral_pick(&negative, &converted, 0.5, 0.5);
    for c in 0..3 {
        assert!((got[c] / want[c] - 1.).abs() < 2e-3, "{got:?} {want:?}");
    }
    assert!((want[0] - 2. / 3.).abs() < 1e-3 && (want[2] - 0.2 / 0.12).abs() < 1e-3);
}

/// Auto white balance and Auto tone measure a scanned negative through its
/// conversion: they choose for it what they choose for its positive.
#[test]
fn auto_reads_a_converted_negatives_positive() {
    let (mut positive, _, plain, converted) = scanned_scene(Recipe::default());
    // A cast for Auto white balance to find.
    for p in positive.pixels.iter_mut() {
        *p = [p[0] * 1.2, p[1], p[2] * 0.8];
    }
    let negative = crate::develop::negative::synthetic::COLOUR.scanned(&positive);
    let want = crate::develop::auto_white_balance(&positive, &plain, Cancel::NEVER).unwrap();
    let got = crate::develop::auto_white_balance(&negative, &converted, Cancel::NEVER).unwrap();
    assert!(
        (got.temperature - want.temperature).abs() < 20. && (got.tint - want.tint).abs() < 1.,
        "{} {} against {} {}",
        got.temperature,
        got.tint,
        want.temperature,
        want.tint
    );
    assert_eq!(got.negative, converted.negative);
    let want = crate::develop::auto_tone(&positive, &plain, Cancel::NEVER).unwrap();
    let got = crate::develop::auto_tone(&negative, &converted, Cancel::NEVER).unwrap();
    for (name, a, b) in [
        ("exposure", want.exposure, got.exposure),
        ("contrast", want.contrast, got.contrast),
        ("highlights", want.highlights, got.highlights),
        ("shadows", want.shadows, got.shadows),
        ("whites", want.whites, got.whites),
        ("blacks", want.blacks, got.blacks),
        ("vibrance", want.vibrance, got.vibrance),
    ] {
        assert!((a - b).abs() < 0.011, "{name}: {a} {b}");
    }
    assert_eq!(got.negative, converted.negative);
}

/// Point Color's dropper and the Targeted Adjustment Tool sample the positive.
#[test]
fn panel_samples_read_a_converted_negatives_positive() {
    use crate::develop::quality::{point_color_pick, targeted_sample};
    let (positive, negative, plain, converted) = scanned_scene(Recipe::default());
    for (u, v) in [(0.5, 0.5), (0.2, 0.7), (0.8, 0.3)] {
        let want = point_color_pick(&positive, &plain, u, v, Cancel::NEVER).unwrap();
        let got = point_color_pick(&negative, &converted, u, v, Cancel::NEVER).unwrap();
        assert!(
            (0..3).all(|c| (got[c] - want[c]).abs() < 0.01),
            "{got:?} {want:?}"
        );
        let want = targeted_sample(&positive, &plain, u, v, Cancel::NEVER).unwrap();
        let got = targeted_sample(&negative, &converted, u, v, Cancel::NEVER).unwrap();
        assert!(
            (got.tone - want.tone).abs() < 0.01,
            "{} {}",
            got.tone,
            want.tone
        );
    }
}

/// A scanned TIFF or JPEG (a rendered file) converts its own linear values, as the
/// conversion's film base is measured in them: a grey ramp's negative renders as a
/// neutral ramp, brighter for a brighter scene.
#[test]
fn a_rendered_scan_renders_a_neutral_positive() {
    use crate::develop::negative::synthetic::BLACK_WHITE;
    use crate::model::negative::FilmType;
    let greys: Vec<[f32; 3]> = (0..=20)
        .map(|i| [10f32.powf(-2. * (1. - i as f32 / 20.)); 3])
        .collect();
    let mut im = crate::develop::negative::tests::image(greys.len() as u32, 1, |x, _| {
        BLACK_WHITE.scan(greys[x as usize])
    });
    im.metadata.source = crate::camera_data::Source::Rendered;
    let im = crate::develop::rendered::to_scene(im);
    let mut r = Recipe::for_rendered(&im.metadata);
    r.negative = Some(BLACK_WHITE.conversion(FilmType::BlackWhite));
    let out = render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    let mut last = -1f32;
    for p in &out.pixels {
        assert!(
            (p[0] - p[1]).abs() < 0.01 && (p[1] - p[2]).abs() < 0.01,
            "{p:?}"
        );
        assert!(p[1] >= last, "{:?}", out.pixels);
        last = p[1];
    }
    assert!(
        out.pixels[0][1] < 0.1 && out.pixels[20][1] > 0.8,
        "{:?}",
        out.pixels
    );
}

/// The GPU port renders a RAW's conversion (`gpu::tests::gpu_develop_matches_cpu_pixel_stage`
/// compares it with the CPU). A rendered file's, which reads the file's linear values,
/// renders on the CPU; without a conversion the port's slot is off.
#[test]
fn the_gpu_port_takes_a_raws_conversion_and_leaves_a_rendered_files_to_the_cpu() {
    use crate::develop::negative::synthetic::COLOUR;
    use crate::model::negative::FilmType;
    let mut im = film_scene();
    im.metadata.cam_xyz = [[0.9, -0.3, -0.1], [-0.4, 1.2, 0.2], [-0.1, 0.2, 0.6]];
    let profile =
        crate::camera_profiles::CameraProfile::camera_matrix_default(&im.metadata).unwrap();
    let positive = Recipe {
        profile: Some(std::sync::Arc::new(profile)),
        ..Default::default()
    };
    let params = pixel_params::pixel_params((&im).into(), &positive).unwrap();
    assert_eq!(params.get(pixel_params::slot::NEGATIVE), [0.; 11]);
    let converted = Recipe {
        negative: Some(COLOUR.conversion(FilmType::Colour)),
        ..positive
    };
    let params = pixel_params::pixel_params((&im).into(), &converted).unwrap();
    assert_eq!(params.get(pixel_params::slot::NEGATIVE)[0], 1.);
    let rendered = rendered_fixture(&[[200, 120, 80], [20, 10, 5]]);
    let mut r = Recipe::for_rendered(&rendered.metadata);
    assert!(pixel_params::pixel_params((&rendered).into(), &r).is_some());
    r.negative = converted.negative;
    assert!(pixel_params::pixel_params((&rendered).into(), &r).is_none());
}
