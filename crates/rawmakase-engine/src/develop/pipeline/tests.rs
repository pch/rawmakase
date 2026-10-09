use super::*;
use crate::develop::curve::ToneCurve;
use crate::develop::effects::EffectsRendering;
fn adjust(p: [f32; 3], m: &Metadata, r: &Recipe) -> [f32; 3] {
    process_pixel(
        p,
        r,
        &CurveSet::new(r),
        profile_matrix(m, r),
        [0., 0.],
        None,
    )
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
    }
}
/// Settings earlier releases saved to choose a rendering engine and its operators are
/// kept as they were, unread: one engine renders every recipe.
#[test]
fn engine_and_operator_settings_of_earlier_releases_are_kept_unread() -> anyhow::Result<()> {
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
    });
    for (key, value) in fields.as_object().unwrap() {
        json[key] = value.clone();
    }
    let old: Recipe = serde_json::from_value(json)?;
    assert_eq!(
        old.unknown.keys().collect::<Vec<_>>(),
        fields.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    assert_eq!(serde_json::to_value(&old)?["engine"], 3);
    assert_eq!(
        crate::develop::render(&im, &old.checked()?, 0)?.pixels,
        crate::develop::render(&im, &plain.checked()?, 0)?.pixels
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
    let pixels = |r: &Recipe| crate::develop::render(&im, &r.checked()?, 0).map(|out| out.pixels);
    assert_ne!(pixels(&edited)?, pixels(&plain)?);
    assert_eq!(pixels(&off)?, pixels(&plain)?);
    // Through the preview renderer too, which the editor and thumbnails use.
    let mut renderer = crate::develop::PreviewRenderer::with_processor(Err(anyhow::anyhow!("CPU")));
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let preview = renderer.render(&im, &off, 0, None, &cancel)?.pixels;
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
    let full = render(&im, &r.checked()?, 0)?;
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
        let p = render(&im, &r.checked()?, 0)?.pixels[0];
        assert!(
            (p[0] - p[1]).abs() < 0.0001 && (p[1] - p[2]).abs() < 0.0001,
            "{p:?}"
        );
    }
    Ok(())
}
#[test]
fn exposure_reveals_retained_highlights() {
    let im = fixture();
    let a = adjust([2.; 3], &im.metadata, &Recipe::default());
    let b = adjust(
        [2.; 3],
        &im.metadata,
        &Recipe {
            exposure: -2.,
            ..Default::default()
        },
    );
    assert!(a[0] > b[0] && b[0] > 0.5);
    assert!(a.iter().all(|v| v.is_finite()));
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
fn monotonic_curve() {
    let k = ToneCurve {
        points: vec![[0., 0.], [0.25, 0.1], [0.5, 0.4], [0.75, 0.8], [1., 1.]],
        smooth: true,
        natural: false,
    };
    let mut prev = 0.;
    for i in 0..=1000 {
        let v = k.evaluate(i as f32 / 1000.);
        assert!(v >= prev);
        prev = v;
    }
}
#[test]
fn matrix_preserves_negative_and_headroom() {
    let p = mul([[2., -1., 0.], [0., 1., 0.], [0., 0., 1.]], [0.1, 1.3, 2.]);
    assert!(p[0] < 0. && p[2] > 1.);
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
    let matrix = profile_matrix(&metadata, &recipe);
    for i in 0..4096 {
        let p = [
            (i % 16) as f32 / 8.,
            ((i / 16) % 16) as f32 / 8.,
            (i / 256) as f32 / 8.,
        ];
        let a = process_pixel(p, &recipe, &fast, matrix, [0., 0.], None);
        let b = process_pixel(p, &recipe, &general, matrix, [0., 0.], None);
        for c in 0..3 {
            assert!((a[c] - b[c]).abs() < 2e-5, "{p:?}: {a:?} vs {b:?}");
        }
    }
}
#[test]
fn builtin_lens_correction_brightens_corners_and_keeps_regions_consistent() {
    use crate::optics::{LensCorrection, Radial};
    let mut im = fixture();
    im.pixels = vec![[0.1; 3]; 96];
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
    });
    let on = Recipe::for_metadata(&im.metadata);
    assert!(on.lens_builtin);
    let off = Recipe {
        lens_builtin: false,
        ..on.clone()
    };
    let lum = |p: [f32; 3]| p.iter().sum::<f32>();
    let a = render(&im, &on.checked().unwrap(), 0).unwrap();
    let b = render(&im, &off.checked().unwrap(), 0).unwrap();
    let corner = |r: &Rendered| lum(r.pixels[0]);
    let centre = |r: &Rendered| lum(r.pixels[(4 * r.width + 6) as usize]);
    assert!(corner(&a) > corner(&b) + 0.01);
    assert!(corner(&a) > centre(&a) && (corner(&b) - centre(&b)).abs() < 1e-5);
    let region = render_region(&im, &on.checked().unwrap(), [3, 2, 4, 3]).unwrap();
    let full = render(&im, &on.checked().unwrap(), 0).unwrap();
    for y in 0..3 {
        for x in 0..4 {
            let p = region.pixels[(y * 4 + x) as usize];
            let q = full.pixels[((y + 2) * full.width + x + 3) as usize];
            assert!((0..3).all(|c| (p[c] - q[c]).abs() < 2e-6));
        }
    }
}
#[test]
fn transform_scales_and_fills_uncovered_area_with_white() {
    let im = fixture();
    let mut r = Recipe::for_metadata(&im.metadata);
    let plain = render(&im, &r.checked().unwrap(), 0).unwrap();
    r.transform.scale = 0.5;
    let small = render(&im, &r.checked().unwrap(), 0).unwrap();
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
    let full = render(&im, &r.checked().unwrap(), 0).unwrap();
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

/// Refine Saturation 0 keeps each colour's channel spread (encoded ProPhoto, before the
/// point curve) and takes its luma from the curved colour; other amounts blend, and
/// amounts above 1 render as 1 (Camera Raw 18.7 on the synthetic chart).
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
    let luma = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    let input = [0.25, 0.08, 0.04];
    let render = |amount: f32| {
        let mut r = recipe.clone();
        r.curve_saturation = amount;
        pro(apply_reference_curves(input, &r, &CurveSet::new(&r), None))
    };
    let (full, none, half) = (render(1.), render(0.), render(0.5));
    assert!((spread(none) - spread(pro(input))).abs() < 1e-3, "{none:?}");
    assert!(spread(full) > spread(none) + 0.05, "{full:?} {none:?}");
    assert!((luma(none) - luma(full)).abs() < 1e-3);
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
        pixels: vec![[0.1; 3]; 90 * 60],
        metadata: m,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
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
    let free = render(&im, &r.checked().unwrap(), 0).unwrap();
    assert!(white(&free) > 100, "{}", white(&free));
    r.constrain_crop = true;
    let constrained = render(&im, &r.checked().unwrap(), 0).unwrap();
    assert_eq!(white(&constrained), 0);
    let aspect = constrained.width as f32 / constrained.height as f32;
    assert!((aspect - 1.5).abs() < 0.06, "{aspect}");
    assert!(constrained.width < free.width && constrained.width > 45);
}
/// A look's RGB table goes after the colour controls: a fully desaturated gray still
/// takes the table's tint, and Monochrome still makes it gray.
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
        let out = color_stage([0.05, 0.2, 0.1], r, &CurveSet::new(r), None);
        out.iter().fold(0f32, |a, v| a.max(*v)) - out.iter().fold(1f32, |a, v| a.min(*v))
    };
    assert!(spread(&r) > 0.01);
    // Monochrome comes after the table and stays gray.
    let mut mono = r;
    mono.effects.monochrome = true;
    assert!(spread(&mono) < 1e-3);
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
    let pixels = |r: &Recipe| crate::develop::render(&im, &r.checked()?, 0).map(|out| out.pixels);
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
    let pick = |r: &Recipe| crate::develop::quality::point_color_pick(&im, r, 0.05, 0.5, &cancel);
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
    let out = crate::develop::render(&im, &r.checked()?, 0)?;
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
    // between a gray and the colours it grays.
    let mut r = Recipe {
        sharpening: 0.,
        ..Default::default()
    };
    // A magenta swatch, which none of the fixture's greens is.
    r.point_colors = vec![PointColor::sampled([5., 0.8, 0.3])];
    // A Color Range mask on the fixture's own green brightens it.
    let green = crate::develop::render(&im, &r.checked()?, 0)?.pixels[40];
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
    let shown = crate::develop::render(&im, &r.checked()?, 0)?;
    let mut visualized = r.clone();
    visualized.point_colors = visualize_range(&r.point_colors, 0).unwrap();
    let gray = crate::develop::render(&im, &visualized.checked()?, 0)?;
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
    };
    let r = Recipe {
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let u = |patch: usize| (patch as f32 * 10. + 5.) / 40.;
    let sample = |r: &Recipe, patch| {
        crate::develop::quality::targeted_sample(&im, r, u(patch), 0.5, &cancel)
    };
    // The rendered patch, encoded sRGB.
    let shown = |r: &Recipe, patch: usize| -> anyhow::Result<[f32; 3]> {
        let out = crate::develop::render(&im, &r.checked()?, 0)?;
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
        pixels: vec![[0.1; 3]; 90 * 60],
        metadata: m,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }
}
fn profile_recipe(setup: crate::lens::choice::LensProfileSetup, filename: &str) -> Recipe {
    Recipe {
        lens_profile: true,
        lens_profile_choice: crate::lens::choice::LensProfileChoice {
            setup,
            id: (!filename.is_empty()).then(|| crate::lens::choice::LensProfileId {
                name: String::new(),
                filename: filename.into(),
                digest: String::new(),
                embedded: false,
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
    r.lens_profile_choice.id.as_mut().unwrap().digest = "0123ABCD".into();
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
        let out = render(&im, &r.checked().unwrap(), 0).unwrap();
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
    assert_eq!(white(&render(&im, &r.checked().unwrap(), 0).unwrap()), 0);
    r.transform.vertical = 0.6;
    assert!(white(&render(&im, &r.checked().unwrap(), 0).unwrap()) > 100);
    r.constrain_crop = true;
    let constrained = render(&im, &r.checked().unwrap(), 0).unwrap();
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
        render(&im, &r.checked().unwrap(), 0).unwrap().pixels,
        render(&im, &Recipe::default().checked().unwrap(), 0)
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
    custom.lens_profile_choice.id.as_mut().unwrap().name = "Adobe (Gone)".into();
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
/// moved by Clarity's or Texture's gain.
#[test]
fn contrast_and_whites_are_measured_on_the_photo_alone() {
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
    let matrix = profile_matrix(&im.metadata, &r);
    let plain = CurveSet::for_image((&im).into(), &r, matrix, false);
    let pivot = plain.photo.contrast_pivot;
    assert!((pivot - TYPICAL_PIVOT).abs() > 0.01, "{pivot}");
    assert_ne!(
        plain.photo.whites,
        crate::develop::basic_tone::WhitesTable::original()
    );
    let tone = pixel_params::tone_params((&im).into(), &r).unwrap();
    assert_eq!(tone.get("LOCAL_PIVOT"), [pivot]);
    let map_pass = CurveSet::with_photo_measures((&im).into(), &r, matrix);
    assert_eq!(map_pass.photo, plain.photo);
    assert_eq!(
        map_pass.basic.as_ref().map(|b| &b.lut),
        plain.basic.as_ref().map(|b| &b.lut)
    );
    let gain: Vec<f32> = (0..im.pixels.len())
        .map(|i| 0.5 + (i % 7) as f32 * 0.2)
        .collect();
    let gained = CurveSet::with_photo_measures(Source::new(&im, Some(&gain)), &r, matrix);
    assert_eq!(gained.photo, plain.photo);
    // Nor the measured Texture, which makes a new image.
    let textured = crate::develop::texture::TextureDetail::of(
        &im,
        1.,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap()
    .apply(&im, 1.);
    let source = Source {
        untextured: Some(&im),
        ..Source::new(&textured, None)
    };
    let textured = CurveSet::with_photo_measures(source, &r, matrix);
    assert_eq!(textured.photo, plain.photo);
}

/// A look's parametric curve: a second curve after the user's, as Camera Raw 18.7
/// renders it.
#[test]
fn a_looks_parametric_curve_follows_the_users() {
    use crate::develop::parametric::ParametricCurve;
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
    let user = ParametricCurve::new([0., 0.3, 0., 0.], [0.25, 0.5, 0.75]).unwrap();
    let own = ParametricCurve::new([0., -0.15, -0.2, -0.33], [0.25, 0.5, 0.75]).unwrap();
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
    im.pixels = vec![[0.1; 3]; 96];
    let mut r = Recipe::for_metadata(&im.metadata);
    r.effects.lens_vignette = -0.5;
    let lum = |p: [f32; 3]| p.iter().sum::<f32>();
    let full = render(&im, &r.checked().unwrap(), 0).unwrap();
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
    assert!(at(&render(&im, &lighter.checked().unwrap(), 0).unwrap(), 11, 0) > centre * 1.1);
    // The gain belongs to the whole photo: a crop keeps each pixel's.
    let cropped = Recipe {
        crop: [0.5, 0., 1., 1.],
        ..r
    };
    let half = render(&im, &cropped.checked().unwrap(), 0).unwrap();
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
        pixels: vec![[0.18; 3]; (width * height) as usize],
        metadata: m.clone(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    };
    let mut r = Recipe::with_profiles(&m, &[]);
    r.sharpening = 0.;
    r.noise_chroma = 0.;
    let flat = crate::develop::render(&im, &r.checked()?, 0)?;
    r.effects.grain = 0.5;
    let grain = crate::develop::render(&im, &r.checked()?, 0)?;
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
/// New edits start at Lightroom's Color noise reduction of 25 with the measured
/// operator, which reduces a flat's colour noise as Camera Raw does: its chart's
/// chroma noise dropped from 5.0 to 2.7 at that setting.
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
    };
    let mut r = Recipe::with_profiles(&m, &[]);
    assert_eq!(r.noise_chroma, 0.25);
    r.sharpening = 0.;
    let chroma_noise = |r: &Recipe| -> anyhow::Result<f32> {
        let out = crate::develop::render(&im, &r.checked()?, 0)?;
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
    assert!((0.35..0.7).contains(&ratio), "{ratio}");
    Ok(())
}
