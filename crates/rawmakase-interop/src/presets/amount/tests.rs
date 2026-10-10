use super::*;
use std::sync::Arc;

fn preset(supports: &str) -> Preset {
    Preset {
        id: "test".into(),
        name: "Test".into(),
        group: "Tests".into(),
        path: "test.xmp".into(),
        settings: [("SupportsAmount".to_string(), supports.to_string())].into(),
        curves: Default::default(),
        look: String::new(),
        blockers: Vec::new(),
        notes: Vec::new(),
        photo_settings: false,
        local: Default::default(),
        builtin: false,
    }
}
fn metadata() -> Metadata {
    Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..Default::default()
    }
}
fn amount(before: &Recipe, full: &Recipe) -> PresetAmount {
    PresetAmount::new(&preset("True"), before.clone(), full.clone()).unwrap()
}

#[test]
fn only_presets_that_offer_one_get_an_amount() {
    let before = Recipe::default();
    let full = Recipe {
        exposure: 1.,
        ..Default::default()
    };
    for (supports, offered) in [
        ("True", true),
        ("true", true),
        ("False", false),
        ("", false),
    ] {
        let result = PresetAmount::new(&preset(supports), before.clone(), full.clone());
        assert_eq!(result.is_ok(), offered, "{supports:?}");
    }
    assert_eq!(
        PresetAmount::new(&preset("False"), before, full).unwrap_err(),
        NoAmount::NotOffered
    );
}

#[test]
fn presets_changing_settings_that_dont_scale_get_no_amount() {
    let before = Recipe::default();
    type Change = (fn(&mut Recipe), SettingGroup);
    let changes: [Change; 6] = [
        (|r| r.crop = [0.1, 0.1, 0.9, 0.9], SettingGroup::Crop),
        (
            |r| r.lens_profile = true,
            SettingGroup::LensProfileCorrections,
        ),
        (|r| r.lens_ca = true, SettingGroup::ChromaticAberration),
        (
            |r| r.transform.vertical = 0.2,
            SettingGroup::TransformAdjustments,
        ),
        (
            |r| r.upright.mode = crate::model::transform::UprightMode::Level,
            SettingGroup::UprightMode,
        ),
        (|r| r.straighten = 2., SettingGroup::Crop),
    ];
    for (change, group) in changes {
        let mut full = before.clone();
        full.exposure = 1.;
        change(&mut full);
        assert_eq!(
            PresetAmount::new(&preset("True"), before.clone(), full),
            Err(NoAmount::Changes(group))
        );
        assert!(!group.scales_with_amount());
    }
    // The same settings left as they were don't matter.
    let mut cropped = before;
    cropped.crop = [0.1, 0.1, 0.9, 0.9];
    let mut full = cropped.clone();
    full.exposure = 1.;
    assert!(PresetAmount::new(&preset("True"), cropped, full).is_ok());
}

#[test]
fn sliders_move_linearly_from_before_through_the_preset() {
    let m = metadata();
    let before = Recipe {
        exposure: 0.2,
        contrast: 0.1,
        ..Default::default()
    };
    let mut full = before.clone();
    full.exposure = 1.2;
    full.contrast = 0.7;
    full.hsl[2][1] = -0.4;
    full.effects.grain = 0.3;
    let a = amount(&before, &full);
    let half = a.at(0.5, &m);
    assert!((half.exposure - 0.7).abs() < 1e-6);
    assert!((half.contrast - 0.4).abs() < 1e-6);
    assert!((half.hsl[2][1] + 0.2).abs() < 1e-6);
    assert!((half.effects.grain - 0.15).abs() < 1e-6);
    // Past 100% the change continues, within each slider's range.
    let double = a.at(2., &m);
    assert!((double.exposure - 2.2).abs() < 1e-6);
    assert_eq!(double.contrast, 1.);
    assert!((double.hsl[2][1] + 0.8).abs() < 1e-6);
    assert!((double.effects.grain - 0.6).abs() < 1e-6);
}

#[test]
fn the_ends_are_the_photo_before_and_the_preset() {
    let m = metadata();
    let before = Recipe {
        exposure: 0.4,
        ..Default::default()
    };
    let full = Recipe {
        exposure: -0.6,
        temperature: 4300.,
        tint: 12.,
        preset_name: "Cool".into(),
        ..Default::default()
    };
    let a = amount(&before, &full);
    assert_eq!(a.at(1., &m), full);
    // 0% is the photo as before.
    assert_eq!(a.at(0., &m), before);
    // Each Amount is computed from the same two recipes, so going back and forth
    // ends exactly where a single move would.
    let mut shown = a.at(0.3, &m);
    for amount in [1.7, 0.1, 2., 0.65] {
        shown = a.at(amount, &m);
    }
    assert_eq!(shown, a.at(0.65, &m));
}

#[test]
fn temperature_moves_in_mireds_and_tint_linearly() {
    let m = metadata();
    let before = Recipe {
        temperature: 5000.,
        tint: -10.,
        ..Default::default()
    };
    let full = Recipe {
        temperature: 10000.,
        tint: 30.,
        ..Default::default()
    };
    let half = amount(&before, &full).at(0.5, &m);
    // (200 + 100) / 2 mireds.
    assert!((half.temperature - 1e6 / 150.).abs() < 0.1);
    assert!((half.tint - 10.).abs() < 1e-5);
    let mut expected = half.clone();
    expected.update_wb(&m);
    assert_eq!(half.wb, expected.wb);
    half.validate().unwrap();
}

#[test]
fn point_curves_blend_their_outputs() {
    let m = metadata();
    let before = Recipe::default();
    let mut full = before.clone();
    full.curve.points = vec![[0., 0.1], [0.25, 0.15], [0.75, 0.9], [1., 0.95]];
    full.effects.channels[0].points = vec![[0., 0.], [0.5, 0.6], [1., 1.]];
    let a = amount(&before, &full);
    for t in [0.25, 0.5, 1.5] {
        let r = a.at(t, &m);
        r.validate().unwrap();
        for x in [0.1, 0.3, 0.5, 0.8] {
            let expected = (x + (full.curve.evaluate(x) - x) * t).clamp(0., 1.);
            assert!(
                (r.curve.evaluate(x) - expected).abs() < 2e-3,
                "{t} {x}: {} vs {expected}",
                r.curve.evaluate(x)
            );
        }
        let red = r.effects.channels[0].evaluate(0.5);
        assert!((red - (0.5 + 0.1 * t)).abs() < 1e-4);
    }
    // Curves with different inputs blend at the inputs of both.
    let mut bent = before;
    bent.curve.points = vec![[0., 0.], [0.4, 0.3], [1., 1.]];
    let r = amount(&bent, &full).at(0.5, &m);
    assert_eq!(r.curve.points.len(), 5);
    r.validate().unwrap();
}

#[test]
fn choices_that_arent_numbers_follow_the_preset_above_zero() {
    use crate::{
        model::effects::VignetteStyle,
        model::panels::{Panel, PanelState},
    };
    let m = metadata();
    let before = Recipe::default();
    let mut full = before.clone();
    full.effects.monochrome = true;
    full.effects.gray_mix[0] = 0.4;
    full.effects.vignette = -0.5;
    full.effects.vignette_style = VignetteStyle::PaintOverlay;
    full.panels.set(Panel::ColorMixer, PanelState::Off);
    let a = amount(&before, &full);
    let low = a.at(0.01, &m);
    assert!(low.effects.monochrome);
    assert_eq!(low.effects.vignette_style, VignetteStyle::PaintOverlay);
    assert_eq!(low.panels.state(Panel::ColorMixer), PanelState::Off);
    assert!((low.effects.gray_mix[0] - 0.004).abs() < 1e-6);
    let zero = a.at(0., &m);
    assert!(!zero.effects.monochrome);
    assert_eq!(zero.panels.state(Panel::ColorMixer), PanelState::On);
}

#[test]
fn grading_hues_take_the_shorter_way_or_the_side_with_color() {
    let m = metadata();
    let mut before = Recipe::default();
    before.grading[0] = [0.95, 0.2, 0.];
    before.grading[2] = [0.3, 0., 0.];
    let mut full = before.clone();
    full.grading[0] = [0.05, 0.4, 0.2];
    full.grading[2] = [0.6, 0.5, 0.];
    full.effects.global_grade = [0.1, 0.3, 0.];
    let half = amount(&before, &full).at(0.5, &m);
    // Through red, not round through green and blue.
    assert!(half.grading[0][0].min(1. - half.grading[0][0]) < 1e-5);
    assert!((half.grading[0][1] - 0.3).abs() < 1e-6);
    assert!((half.grading[0][2] - 0.1).abs() < 1e-6);
    // No saturation before: the hue is the preset's from the start.
    assert_eq!(half.grading[2][0], 0.6);
    assert_eq!(half.effects.global_grade[0], 0.1);
}

#[test]
fn a_new_look_fades_in_by_its_profile_amount() {
    let m = metadata();
    let look = Arc::new(crate::camera_profiles::CameraProfile::creative_for_test(&m));
    let before = Recipe::default();
    let full = Recipe {
        profile: Some(look.clone()),
        profile_amount: 0.8,
        ..Default::default()
    };
    let a = amount(&before, &full);
    let r = a.at(0.5, &m);
    assert_eq!(r.profile.as_ref(), Some(&look));
    assert!((r.profile_amount - 0.4).abs() < 1e-6);
    assert!((a.at(2., &m).profile_amount - 1.6).abs() < 1e-6);
    // The same look already chosen: its Amount moves from the photo's.
    let before = Recipe {
        profile_amount: 0.5,
        ..full.clone()
    };
    let r = amount(&before, &full).at(2., &m);
    assert!((r.profile_amount - 1.1).abs() < 1e-6);
}

#[test]
fn every_amount_is_a_valid_recipe_at_the_extremes() {
    let m = metadata();
    let mut low = Recipe::default();
    let mut high = Recipe::default();
    for (r, sign) in [(&mut low, -1f32), (&mut high, 1.)] {
        r.exposure = 5. * sign;
        r.contrast = sign;
        r.shadows = sign;
        r.temperature = if sign > 0. { 50000. } else { 2000. };
        r.tint = 150. * sign;
        r.hsl = [[sign; 3]; 8];
        r.grading = [[0.5, 1., sign]; 3];
        r.effects.parametric = [sign; 4];
        r.effects.gray_mix = [sign; 8];
        r.effects.clarity = sign;
        r.effects.grain = (sign + 1.) / 2.;
        r.effects.vignette = sign;
        r.sharpening_radius = if sign > 0. { 3. } else { 0.5 };
        r.midtone = if sign > 0. { 4. } else { 0.1 };
        r.effects.splits = if sign > 0. {
            [0.6, 0.7, 0.9]
        } else {
            [0.1, 0.2, 0.3]
        };
    }
    for (before, full) in [(&low, &high), (&high, &low)] {
        let a = amount(before, full);
        for t in [0., 0.25, 0.5, 1., 1.5, 2.] {
            a.at(t, &m).validate().unwrap();
        }
    }
}

/// A setting the preset leaves alone keeps its value, even outside the slider's range.
#[test]
fn settings_the_preset_leaves_alone_stay_as_they_were() {
    let m = metadata();
    let before = Recipe {
        exposure: 6.,
        ..Default::default()
    };
    let full = Recipe {
        contrast: 0.4,
        ..before.clone()
    };
    for t in [0.5, 2.] {
        assert_eq!(amount(&before, &full).at(t, &m).exposure, 6.);
    }
    // One the preset brings from outside the slider's range is reached smoothly.
    let seven = Recipe {
        exposure: 7.,
        ..Default::default()
    };
    let a = amount(&Recipe::default(), &seven);
    assert!((a.at(0.9, &m).exposure - 6.3).abs() < 1e-5);
    assert_eq!(a.at(1.5, &m).exposure, 7.);
}

/// Curves with too many inputs between them keep the preset's, so 100% is exact.
#[test]
fn dense_curves_keep_the_presets_points() {
    let m = metadata();
    let dense = |offset: f32| {
        let mut r = Recipe::default();
        r.curve.points = (0..20)
            .map(|i| {
                let x = (i as f32 + offset) / 20.;
                [x.min(1.), x.min(1.)]
            })
            .collect();
        r.curve.points[0][0] = 0.;
        r.curve.points[19] = [1., 0.9];
        r
    };
    let before = dense(0.5);
    let full = dense(0.);
    let r = amount(&before, &full).at(0.99, &m);
    let xs = |r: &Recipe| r.curve.points.iter().map(|p| p[0]).collect::<Vec<_>>();
    assert_eq!(xs(&r), xs(&full));
    r.validate().unwrap();
}

/// Parametric region boundaries stop where two would meet rather than jump back.
#[test]
fn parametric_splits_keep_growing_until_they_would_cross() {
    let m = metadata();
    let before = Recipe::default();
    let mut full = before.clone();
    full.effects.splits = [0.1, 0.2, 0.3];
    let a = amount(&before, &full);
    let mut last = full.effects.splits[2];
    for t in [1., 1.2, 1.5, 1.7, 1.9, 2.] {
        let r = a.at(t, &m);
        r.validate().unwrap();
        let top = r.effects.splits[2];
        assert!(top <= last + 1e-6, "{t}: {top} after {last}");
        last = top;
    }
    assert!(last < 0.2);
}

/// Point Color swatches scale their shifts; swatches added or taken away don't scale.
#[test]
fn point_color_shifts_scale_and_new_swatches_dont() {
    use crate::model::point_color::PointColor;
    let m = metadata();
    let before = Recipe {
        point_colors: vec![PointColor::sampled([0.6, 0.4, 0.2])],
        ..Default::default()
    };
    let mut full = before.clone();
    full.point_colors[0].shift = [0.2, -0.4, 0.];
    let half = amount(&before, &full).at(0.5, &m);
    assert_eq!(half.point_colors[0].shift, [0.1, -0.2, 0.]);
    half.validate().unwrap();
    let cleared = Recipe::default();
    assert_eq!(
        PresetAmount::new(&preset("True"), before, cleared),
        Err(NoAmount::Changes(SettingGroup::ColorAdjustments))
    );
}

#[test]
fn lens_profile_setup_changes_that_render_alike_keep_amount() {
    use crate::lens::choice::{LensProfileChoice, LensProfileId, LensProfileSetup};
    let id = |digest: &str| {
        Some(LensProfileId {
            name: "Adobe (Testcam 35mm F2)".into(),
            digest: digest.into(),
            ..Default::default()
        })
    };
    let at = |setup, digest| Recipe {
        lens_profile_choice: LensProfileChoice {
            setup,
            id: id(digest),
        },
        ..Default::default()
    };
    let before = at(LensProfileSetup::Default, "");
    assert_eq!(
        super::fixed_change(&before, &at(LensProfileSetup::Auto, "0123ABCD")),
        None
    );
    assert_eq!(
        super::fixed_change(&before, &at(LensProfileSetup::Custom, "")),
        Some(SettingGroup::LensProfileCorrections)
    );
}

#[test]
fn an_amount_rounded_back_to_before_keeps_its_white_balance() {
    // 5000 K is exactly 200 mired, avoiding reciprocal round-off in this fixture.
    let before = Recipe {
        temperature: 5000.,
        ..Recipe::default()
    };
    let full = Recipe {
        temperature: 7000.,
        ..before.clone()
    };
    let tiny = amount(&before, &full).at(1e-9, &metadata());
    assert_eq!(tiny.temperature, before.temperature);
    assert_eq!(tiny.wb, before.wb);
}
