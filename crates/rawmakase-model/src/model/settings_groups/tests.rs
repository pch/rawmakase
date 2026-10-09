use super::*;
use crate::model::masks::{MaskComponent, MaskGroup, MaskShape};
use crate::model::retouch::{RetouchMode, RetouchOp, RetouchShape};
use crate::model::transform::UprightMode;
use crate::{model::effects::VignetteStyle, model::panels::PanelState};
use serde_json::Value;
use std::collections::BTreeMap;

fn from<'a>(recipe: &'a Recipe, metadata: &'a Metadata) -> Source<'a> {
    Source { recipe, metadata }
}

fn camera(make: &str, model: &str) -> Metadata {
    Metadata {
        make: make.into(),
        model: model.into(),
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[0.9, -0.3, -0.1], [-0.4, 1.2, 0.2], [-0.1, 0.2, 0.6]],
        ..Default::default()
    }
}

/// A recipe with every setting away from its default.
fn everything_changed() -> Recipe {
    let mut r = Recipe {
        lens_builtin: false,
        lens_profile: true,
        lens_profile_choice: crate::lens::choice::LensProfileChoice {
            setup: crate::lens::choice::LensProfileSetup::Custom,
            id: Some(crate::lens::choice::LensProfileId {
                name: "Adobe (Test 35mm)".into(),
                ..Default::default()
            }),
        },
        lens_distortion: 0.5,
        lens_vignetting: 1.5,
        lens_manual_distortion: -0.2,
        lens_ca: true,
        preset_name: "Film".into(),
        preset_settings: [("Exposure2012".to_string(), "1".to_string())].into(),
        sharpening_radius: 1.5,
        sharpening_detail: 0.6,
        sharpening_masking: 0.7,
        exposure: 0.7,
        camera_exposure: 0.3,
        temperature: 4000.,
        tint: 12.,
        wb: [1.5, 1., 0.8],
        auto_white_balance: Some([4000., 12.]),
        contrast: 0.1,
        highlights: -0.2,
        shadows: 0.3,
        whites: 0.15,
        blacks: -0.25,
        black_point: 0.05,
        white_point: 0.95,
        midtone: 1.2,
        saturation: 0.1,
        vibrance: 0.2,
        hsl: [[0.1, 0.2, 0.3]; 8],
        point_colors: vec![crate::model::point_color::PointColor {
            shift: [0.2, 0.3, -0.1],
            ..crate::model::point_color::PointColor::sampled([1., 0.5, 0.3])
        }],
        grading: [[0.5, 0.2, 0.1]; 3],
        noise_luma: 0.3,
        noise_chroma: 0.4,
        sharpening: 0.5,
        crop: [0.1, 0.1, 0.9, 0.9],
        straighten: 2.,
        constrain_crop: true,
        rotation: 1,
        flip_x: true,
        flip_y: true,
        ..Default::default()
    };
    r.profile = crate::camera_profiles::open::color(&camera("Fujifilm", "X100F")).map(Arc::new);
    r.curve.points = vec![[0., 0.1], [1., 1.]];
    r.transform.vertical = 0.2;
    r.upright.mode = UprightMode::Level;
    r.retouch.push(RetouchOp {
        mode: RetouchMode::Heal,
        shape: RetouchShape::Spot {
            center: [0.5, 0.5],
            radius: 0.01,
        },
        feather: 0.5,
        opacity: 1.,
        offset: [0.02, 0.],
    });
    r.red_eye.push(crate::model::red_eye::RedEyeOp {
        kind: Default::default(),
        center: [0.4, 0.4],
        radius: [0.01; 2],
        correlation: 0.,
        pupil_size: 0.5,
        darken: 0.5,
    });
    r.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Brush {
            strokes: Vec::new(),
        })],
        ..Default::default()
    });
    r.panels.set(Panel::Detail, PanelState::Off);
    r.curve_saturation = 0.4;
    r.profile_amount = 0.6;
    r.unknown
        .insert("from_a_newer_release".into(), Value::Bool(true));
    let e = &mut r.effects;
    e.channels[0].points = vec![[0., 0.2], [1., 1.]];
    e.parametric = [0.1; 4];
    e.splits = [0.2, 0.5, 0.8];
    e.calibration = [[0.1, 0.2]; 3];
    e.shadow_tint = 0.1;
    e.monochrome = true;
    e.gray_mix = [0.1; 8];
    e.balance = 0.2;
    e.blending = 0.6;
    e.global_grade = [0.2, 0.3, 0.1];
    e.clarity = 0.2;
    e.texture = 0.3;
    e.dehaze = 0.1;
    e.grain = 0.2;
    e.grain_size = 0.4;
    e.grain_roughness = 0.6;
    e.grain_seed = 7;
    e.vignette = -0.3;
    e.vignette_midpoint = 0.4;
    e.vignette_roundness = 0.2;
    e.vignette_feather = 0.7;
    e.vignette_highlights = 0.3;
    e.vignette_style = VignetteStyle::PaintOverlay;
    e.lens_vignette = 0.2;
    e.lens_vignette_midpoint = 0.4;
    e.defringe = [0.5, 0.5];
    e.defringe_ranges = [[0.2, 0.8], [0.3, 0.7]];
    e.luma_detail = 0.6;
    e.luma_contrast = 0.2;
    e.chroma_detail = 0.6;
    e.chroma_smoothness = 0.4;
    r
}

/// Each setting's JSON, `effects.*` flattened, so recipes can be compared setting by
/// setting.
fn settings(r: &Recipe) -> BTreeMap<String, Value> {
    let Value::Object(all) = serde_json::to_value(r).unwrap() else {
        unreachable!()
    };
    let mut out = BTreeMap::new();
    for (key, value) in all {
        match (key.as_str(), value) {
            ("effects", Value::Object(effects)) => {
                for (k, v) in effects {
                    out.insert(format!("effects.{k}"), v);
                }
            }
            ("from_a_newer_release", v) => {
                out.insert("unknown".into(), v);
            }
            (_, v) => {
                out.insert(key, v);
            }
        }
    }
    out
}
fn changed(a: &Recipe, b: &Recipe) -> BTreeSet<String> {
    let (a, b) = (settings(a), settings(b));
    a.keys()
        .chain(b.keys())
        .filter(|k| a.get(*k) != b.get(*k))
        .cloned()
        .collect()
}

#[test]
fn every_setting_is_classified_and_the_source_changes_all_of_them() {
    let source = everything_changed();
    let listed: BTreeSet<String> = every_setting(&source)
        .into_iter()
        .map(|(k, _)| k.to_string())
        .collect();
    // Upright corrections are omitted when empty, so compare against the source.
    assert_eq!(changed(&Recipe::default(), &source), listed);
}

#[test]
fn each_group_transfers_exactly_its_settings() {
    let source = everything_changed();
    let m = camera("Fujifilm", "X100F");
    let target = Target {
        metadata: &m,
        profiles: &[],
    };
    let kinds = every_setting(&source);
    for group in SettingGroup::ALL {
        let mut selection = GroupSelection::none();
        selection.set(group, GroupInclusion::Included);
        let to = Recipe::default();
        let moved = changed(
            &to,
            &transfer(from(&source, &m), &to, &selection, target).recipe,
        );
        let mut expected: BTreeSet<String> = kinds
            .iter()
            .filter(|(_, kind)| *kind == Kind::Group(group))
            .map(|(k, _)| k.to_string())
            .collect();
        // What is worked out for the target follows its group.
        match group {
            SettingGroup::WhiteBalance => {
                expected.insert("wb".into());
            }
            // A new profile keeps the photo's white balance gains and shows them as
            // its Temperature and Tint, as choosing one in the Profile menu does.
            SettingGroup::TreatmentAndProfile => {
                expected.extend(["camera_exposure", "temperature", "tint", "wb"].map(String::from));
            }
            // The whole of Upright, its mode with it.
            SettingGroup::UprightTransforms => {
                expected.insert("upright".into());
            }
            // The source has Detail switched off.
            SettingGroup::Sharpening
            | SettingGroup::LuminanceNoiseReduction
            | SettingGroup::ColorNoiseReduction => {
                expected.insert("panels".into());
            }
            _ => {}
        }
        if group == SettingGroup::ProcessVersion {
            // One engine renders every edit: only the target's own baseline comes back.
            assert!(
                moved.is_subset(&BTreeSet::from(["camera_exposure".to_string()])),
                "{group:?}: {moved:?}"
            );
            continue;
        }
        if group == SettingGroup::TreatmentAndProfile {
            // Only what the profile maps differently moves with it.
            assert!(moved.is_subset(&expected), "{group:?}: {moved:?}");
            assert!(moved.contains("profile") && moved.contains("effects.monochrome"));
            continue;
        }
        assert_eq!(moved, expected, "{group:?}");
    }
}

#[test]
fn nothing_selected_changes_nothing_and_the_photos_own_settings_never_move() {
    let source = everything_changed();
    let m = camera("Fujifilm", "X100F");
    let target = Target {
        metadata: &m,
        profiles: &[],
    };
    let to = Recipe::default();
    assert_eq!(
        transfer(from(&source, &m), &to, &GroupSelection::none(), target).recipe,
        to
    );
    let all = transfer(from(&source, &m), &to, &GroupSelection::all(), target).recipe;
    for key in [
        "rotation",
        "flip_x",
        "flip_y",
        "preset_name",
        "preset_settings",
        // Red eye corrections belong to their photo; Lightroom never copies them.
        "red_eye",
    ] {
        assert!(!changed(&to, &all).contains(key), "{key}");
    }
    assert!(all.unknown.is_empty());
    // Paste's default leaves spots and masks where they were made.
    let pasted = transfer(from(&source, &m), &to, &GroupSelection::default(), target).recipe;
    assert!(pasted.retouch.is_empty() && pasted.masks.is_empty());
    assert_eq!(pasted.exposure, source.exposure);
}

#[test]
fn profile_and_white_balance_are_resolved_for_the_target_camera() {
    use crate::camera_profiles::open;
    let fuji = camera("Fujifilm", "X100F");
    let m = fuji.clone();
    let sony = camera("Sony", "ILCE-7M2");
    let source = Recipe {
        profile: open::color(&fuji).map(Arc::new),
        profile_amount: 0.5,
        temperature: 4200.,
        tint: 8.,
        ..Default::default()
    };
    let sony_profiles: Vec<_> = open::color(&sony).map(Arc::new).into_iter().collect();
    let to = Recipe {
        profile_amount: 1.2,
        ..Default::default()
    };
    let out = transfer(
        from(&source, &m),
        &to,
        &GroupSelection::default(),
        Target {
            metadata: &sony,
            profiles: &sony_profiles,
        },
    );
    let profile = out.recipe.profile.as_ref().unwrap();
    assert_eq!(profile.name, open::COLOR);
    assert!(profile.ensure_camera(&sony).is_ok());
    assert!(out.notes.is_empty(), "{:?}", out.notes);
    // The same Temperature and Tint, with this camera's gains.
    assert_eq!((out.recipe.temperature, out.recipe.tint), (4200., 8.));
    let mut expected = out.recipe.clone();
    expected.update_wb(&sony);
    assert_eq!(out.recipe.wb, expected.wb);
    // Without that profile for the target, it keeps its own and says so.
    let out = transfer(
        from(&source, &m),
        &to,
        &GroupSelection::default(),
        Target {
            metadata: &sony,
            profiles: &[],
        },
    );
    // Its own profile keeps its own Amount.
    assert_eq!(
        (&out.recipe.profile, out.recipe.profile_amount),
        (&to.profile, 1.2)
    );
    assert!(
        out.notes.iter().any(|n| n.contains(open::COLOR)),
        "{:?}",
        out.notes
    );
}

#[test]
fn upright_moves_its_mode_and_guided_needs_this_photos_guides() {
    let m = camera("Fujifilm", "X100F");
    let target = Target {
        metadata: &m,
        profiles: &[],
    };
    let mut source = Recipe::default();
    source.upright.mode = UprightMode::Guided;
    let out = transfer(
        from(&source, &m),
        &Recipe::default(),
        &GroupSelection::default(),
        target,
    );
    assert_eq!(out.recipe.upright.mode, UprightMode::Off);
    assert!(!out.notes.is_empty());
    // The target's own analysis stays.
    source.upright.mode = UprightMode::Level;
    let mut to = Recipe::default();
    to.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4];
    let out = transfer(from(&source, &m), &to, &GroupSelection::default(), target);
    assert_eq!(out.recipe.upright.mode, UprightMode::Level);
    assert_eq!(out.recipe.upright.corrections, to.upright.corrections);
}

#[test]
fn transform_keeps_what_it_shows_on_photos_turned_differently() {
    let m = camera("Fujifilm", "X100F");
    let mut source = Recipe {
        rotation: 1,
        ..Default::default()
    };
    let turned = |r: &Recipe| {
        crate::model::transform::display_axes(
            (crate::model::image_frame::ImageFrame::for_metadata(&m).turns + r.rotation) % 4,
            r.flip_x,
            r.flip_y,
        )
    };
    // Vertical +50 as shown on the turned photo.
    let mut shown = source.transform.displayed(turned(&source));
    shown.vertical = 0.5;
    source.transform = shown.recorded(turned(&source));
    let mut selection = GroupSelection::none();
    selection.set(SettingGroup::TransformAdjustments, GroupInclusion::Included);
    let target = Target {
        metadata: &m,
        profiles: &[],
    };
    let out = transfer(from(&source, &m), &Recipe::default(), &selection, target).recipe;
    let pasted = out.transform.displayed(turned(&out));
    assert_eq!((pasted.vertical, pasted.horizontal), (0.5, 0.));
}

#[test]
fn unselected_groups_and_unchanged_lenses_leave_upright_alone() {
    let m = camera("Fujifilm", "X100F");
    let target = Target {
        metadata: &m,
        profiles: &[],
    };
    let mut to = Recipe::default();
    to.upright.mode = UprightMode::Guided;
    let out = transfer(
        from(&Recipe::default(), &m),
        &to,
        &GroupSelection::none(),
        target,
    );
    assert_eq!(out.recipe, to);
    // New lens corrections call for a new analysis.
    let mut to = Recipe::default();
    to.upright.mode = UprightMode::Level;
    to.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4];
    to.upright.lightroom = [("UprightVersion".into(), "151388160".into())].into();
    let mut source = Recipe {
        lens_profile: true,
        ..Default::default()
    };
    source.upright.mode = UprightMode::Level;
    let out = transfer(from(&source, &m), &to, &GroupSelection::default(), target);
    assert_eq!(out.recipe.upright.mode, UprightMode::Level);
    assert!(out.recipe.upright.corrections.is_empty());
    // Lightroom's analysis details went with its corrections.
    assert!(out.recipe.upright.lightroom.is_empty());
}

#[test]
fn a_profile_resolves_to_the_targets_own_file_of_that_name() {
    use crate::camera_profiles::open;
    let m = camera("Fujifilm", "X100F");
    let source = Recipe {
        profile: open::color(&m).map(Arc::new),
        ..Default::default()
    };
    // The target's own profile of the same name (as a DNG's embedded one would be).
    let own = Arc::new(open::color(&m).unwrap());
    let profiles = [own.clone()];
    let out = transfer(
        from(&source, &m),
        &Recipe::default(),
        &GroupSelection::default(),
        Target {
            metadata: &m,
            profiles: &profiles,
        },
    );
    assert!(Arc::ptr_eq(out.recipe.profile.as_ref().unwrap(), &own));
}

#[test]
fn a_lens_panel_switched_off_or_another_process_version_needs_a_new_analysis() {
    let m = camera("Fujifilm", "X100F");
    let target = Target {
        metadata: &m,
        profiles: &[],
    };
    // With profile corrections on, so switching the panel off changes the rendering
    // (a camera's built-in correction stays either way).
    let mut to = Recipe {
        lens_profile: true,
        ..Default::default()
    };
    to.upright.mode = UprightMode::Level;
    to.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4];
    let mut source = to.clone();
    source.upright.corrections.clear();
    source.panels.set(Panel::LensCorrections, PanelState::Off);
    let out = transfer(from(&source, &m), &to, &GroupSelection::default(), target);
    assert!(out.recipe.upright.corrections.is_empty());
}

/// Guides belong to the photo they were drawn on: Upright Mode leaves them behind, a
/// target keeps its own (solving them again after new lens corrections), and only
/// Upright Transforms copies a solved correction, guides and all.
#[test]
fn guides_stay_with_their_photo_unless_upright_transforms_copies_the_correction() {
    use crate::model::transform::UprightGuide;
    let m = camera("Fujifilm", "X100F");
    let target = Target {
        metadata: &m,
        profiles: &[],
    };
    let guides = vec![
        UprightGuide {
            a: [0.3, 0.1],
            b: [0.32, 0.9],
        },
        UprightGuide {
            a: [0.7, 0.1],
            b: [0.68, 0.9],
        },
    ];
    let mut solved = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 6];
    solved[5][7] = 0.1;
    let mut source = Recipe::default();
    source.upright.mode = UprightMode::Guided;
    source.upright.guides = guides.clone();
    source.upright.corrections = solved.clone();
    // Upright Mode alone: the guides don't move, so another photo's Guided is left Off.
    let out = transfer(
        from(&source, &m),
        &Recipe::default(),
        &GroupSelection::default(),
        target,
    );
    assert_eq!(out.recipe.upright.mode, UprightMode::Off);
    assert!(out.recipe.upright.guides.is_empty());
    // A photo with guides of its own keeps them, and solves them again once its new
    // lens corrections are analysed.
    let to = Recipe {
        upright: source.upright.clone(),
        ..Default::default()
    };
    let lens = Recipe {
        lens_profile: true,
        ..source.clone()
    };
    let out = transfer(from(&lens, &m), &to, &GroupSelection::default(), target);
    assert_eq!(out.recipe.upright.mode, UprightMode::Guided);
    assert_eq!(out.recipe.upright.guides, guides);
    assert!(out.recipe.upright.corrections.is_empty());
    assert!(out.recipe.upright.needs_analysis());
    assert!(out.notes.is_empty(), "{:?}", out.notes);
    // Upright Transforms: the correction exactly as solved on the source.
    let mut exact = GroupSelection::none();
    exact.set(SettingGroup::UprightTransforms, GroupInclusion::Included);
    for lens_profile in [false, true] {
        let to = Recipe {
            lens_profile,
            ..Default::default()
        };
        let out = transfer(from(&source, &m), &to, &exact, target);
        assert_eq!(out.recipe.upright, source.upright, "{lens_profile}");
    }
    // Paste leaves it out unless asked, as Lightroom does.
    assert!(!GroupSelection::default().contains(SettingGroup::UprightTransforms));
}
#[test]
fn a_black_and_white_profile_carries_its_treatment_to_another_camera() {
    use crate::camera_profiles::open;
    let fuji = camera("Fujifilm", "X100F");
    let sony = camera("Sony", "ILCE-7M2");
    let mut mono = open::color(&fuji).unwrap();
    mono.enhanced.as_mut().unwrap().monochrome = true;
    let source = Recipe {
        profile: Some(Arc::new(mono)),
        ..Default::default()
    };
    assert!(!source.effects.monochrome);
    assert_eq!(
        source.treatment(),
        crate::model::recipe::Treatment::BlackWhite
    );
    // The target camera has no such profile and keeps its own color one.
    let out = transfer(
        from(&source, &fuji),
        &Recipe::default(),
        &GroupSelection::default(),
        Target {
            metadata: &sony,
            profiles: &[],
        },
    );
    assert_eq!(
        out.recipe.treatment(),
        crate::model::recipe::Treatment::BlackWhite
    );
}

#[test]
fn a_lens_profile_the_target_cannot_use_is_reported() {
    use crate::lens::choice::{LensProfileSetup, tests};
    let m = tests::photo();
    let mut source = Recipe {
        lens_profile: true,
        ..Default::default()
    };
    let other = &m
        .lens_profiles
        .all()
        .iter()
        .find(|c| c.profile.filename == tests::OTHER)
        .unwrap()
        .profile;
    source.lens_profile_choice.choose(other);
    let target = |m| Target {
        metadata: m,
        profiles: &[],
    };
    let out = transfer(
        from(&source, &m),
        &Recipe::default(),
        &GroupSelection::default(),
        target(&m),
    );
    assert!(out.notes.is_empty(), "{:?}", out.notes);
    // A photo without that profile keeps the choice and says so.
    let mut bare = m.clone();
    bare.lens_profiles = Default::default();
    let out = transfer(
        from(&source, &m),
        &Recipe::default(),
        &GroupSelection::default(),
        target(&bare),
    );
    assert_eq!(
        out.recipe.lens_profile_choice.setup,
        LensProfileSetup::Custom
    );
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("Adobe (Lensco 50mm F1.4)")),
        "{:?}",
        out.notes
    );
    // So is the camera's own profile on a photo whose RAW has none.
    let mut embedded = source.clone();
    embedded.lens_profile_choice.id = Some(crate::lens::choice::LensProfileId {
        name: "Camera Settings".into(),
        embedded: true,
        ..Default::default()
    });
    let out = transfer(
        from(&embedded, &m),
        &Recipe::default(),
        &GroupSelection::default(),
        target(&m),
    );
    assert!(
        out.notes.iter().any(|n| n.contains("Camera Settings")),
        "{:?}",
        out.notes
    );
}

#[test]
fn masks_made_from_a_selection_stay_on_their_photo() {
    use crate::model::masks::{BITMAP_SAMPLING, BitmapMask};
    let raster = MaskGroup {
        name: "Subject".into(),
        components: vec![MaskComponent::new(MaskShape::Bitmap(BitmapMask {
            id: format!("sha256:{}", "ab".repeat(32)),
            width: 4,
            height: 4,
            sampling: BITMAP_SAMPLING,
            source: None,
        }))],
        ..Default::default()
    };
    let gradient = MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.; 2],
            to: [1.; 2],
        })],
        ..Default::default()
    };
    let source = Recipe {
        masks: vec![raster.clone(), gradient.clone()],
        ..Default::default()
    };
    let m = camera("Sony", "ILCE-7M4");
    let mut selection = GroupSelection::none();
    selection.set(SettingGroup::Masking, GroupInclusion::Included);
    let target = Target {
        metadata: &m,
        profiles: &[],
    };
    let out = transfer(from(&source, &m), &Recipe::default(), &selection, target);
    assert_eq!(out.recipe.masks, vec![gradient.clone()]);
    assert_eq!(out.notes.len(), 1);
    // A target that has its own selection keeps it, under the source's other masks.
    let mut own = raster;
    own.name = "Own subject".into();
    let to = Recipe {
        masks: vec![own.clone()],
        ..Default::default()
    };
    let out = transfer(from(&source, &m), &to, &selection, target);
    assert_eq!(out.recipe.masks, vec![gradient.clone(), own.clone()]);
    // Even when the source fills every slot, the target's own selection survives.
    let full = Recipe {
        masks: vec![gradient.clone(); crate::model::masks::MAX_GROUPS],
        ..Default::default()
    };
    let out = transfer(from(&full, &m), &to, &selection, target);
    assert_eq!(out.recipe.masks.len(), crate::model::masks::MAX_GROUPS);
    assert_eq!(out.recipe.masks.last(), Some(&own));
}
