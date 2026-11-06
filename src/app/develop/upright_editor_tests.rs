//! Editor tests of `upright`: the whole editor driven through its frames.
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use std::sync::Arc;

#[test]
fn undoing_an_upright_mode_turns_it_off_once_analysed() {
    use crate::model::transform::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let original = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().upright.mode = UprightMode::Vertical;
    e.commit_edit(original, None);
    // The analysis arrives after the click that chose the mode.
    let (generation, _) = e.document.upright.start();
    let analysed = e.document.edit.recipe().clone();
    // Copied to Before while the analysis runs: Before gets it too.
    e.transfer(crate::app::develop::before_after::Transfer::AfterToBefore);
    let mut corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 6];
    corrections[4][6] = 0.1;
    e.upright_ready(generation, &analysed, Ok(corrections.clone()));
    assert_eq!(e.document.edit.recipe().upright.corrections, corrections);
    assert_eq!(
        e.document.before.as_ref().unwrap().upright.corrections,
        corrections
    );
    // It is not a step of its own: one undo leaves Upright off, redo brings it back
    // corrected.
    assert_eq!(e.document.edit.history().steps().1, 1);
    e.undo();
    assert_eq!(e.document.edit.recipe().upright.mode, UprightMode::Off);
    e.redo();
    assert_eq!(e.document.edit.recipe().upright.mode, UprightMode::Vertical);
    assert_eq!(e.document.edit.recipe().upright.corrections, corrections);
}

#[test]
fn upright_analysis_stays_with_its_photo_and_keeps_imported_guided() {
    use crate::model::transform::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let identity = [1., 0., 0., 0., 1., 0., 0., 0., 1.];
    let mut guided = identity;
    guided[2] = 0.05;
    e.document.edit.setup_mut().upright.mode = UprightMode::Guided;
    e.document.edit.setup_mut().upright.corrections =
        vec![identity, identity, identity, identity, identity, guided];
    // Update analyses the other modes; Lightroom's Guided correction survives.
    let (generation, _) = e.document.upright.start();
    let analysed = e.document.edit.recipe().clone();
    e.upright_ready(generation, &analysed, Ok(vec![identity; 5]));
    assert_eq!(e.document.edit.recipe().upright.corrections[5], guided);
    // Pasted onto another photo, the mode comes along but not the corrections: that
    // photo keeps its own, here none yet, for the editor to analyse.
    e.document.edit.setup_mut().upright.mode = UprightMode::Vertical;
    e.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    e.copy_settings(crate::model::settings_groups::GroupSelection::default());
    e.document.edit.replace(Recipe::default());
    e.paste_settings();
    assert_eq!(e.document.edit.recipe().upright.mode, UprightMode::Vertical);
    assert!(e.document.edit.recipe().upright.corrections.is_empty());
}

#[test]
fn upright_analysis_yields_to_corrections_applied_meanwhile() {
    use crate::model::transform::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.document.edit.setup_mut().upright.mode = UprightMode::Level;
    let (generation, _) = e.document.upright.start();
    let analysed = e.document.edit.recipe().clone();
    // A Lightroom preset with its own corrections lands before the analysis.
    let mut imported = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 6];
    imported[3][2] = 0.02;
    e.document.edit.setup_mut().upright.corrections = imported.clone();
    e.upright_ready(
        generation,
        &analysed,
        Ok(vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 5]),
    );
    assert_eq!(e.document.edit.recipe().upright.corrections, imported);
}

/// Stripes falling 3° to the right, which Auto straighten levels at about −3°.
fn falling_stripes() -> Arc<CameraImage> {
    let (w, h) = (600usize, 400usize);
    let (s, c) = 3f32.to_radians().sin_cos();
    let pixels = (0..w * h)
        .map(|i| {
            // Supersampled, so edges are smooth rather than pixel steps.
            let mut sum = 0.;
            for j in 0..4 {
                let x = (i % w) as f32 + (j % 2) as f32 * 0.5 - 299.75;
                let y = (i / w) as f32 + (j / 2) as f32 * 0.5 - 199.75;
                let across = -s * x + c * y;
                sum += if (across / 70.).rem_euclid(2.) < 1. {
                    0.78
                } else {
                    0.1
                };
            }
            [sum / 4.; 3]
        })
        .collect();
    Arc::new(CameraImage {
        recovered: Default::default(),
        width: w as u32,
        height: h as u32,
        pixels,
        metadata: Metadata {
            width: w as u32,
            height: h as u32,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    })
}

#[test]
fn auto_straighten_sets_the_level_angle_as_one_step_and_measures_again_after_a_turn() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.document.set_image(falling_stripes());
    e.document.edit.setup_mut().wb = [1.; 3];
    let upright = e.document.edit.recipe().upright.clone();
    let next = |e: &mut Editor| loop {
        match e.rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(worker::Event::Straighten {
                generation,
                analysed,
                result,
                ..
            }) => break (generation, analysed, result),
            Ok(_) => continue,
            Err(err) => panic!("no Auto straighten result: {err}"),
        }
    };
    e.start_auto_straighten();
    assert!(e.document.straighten.is_running());
    // Measured before the photo was turned: it is measured again, and nothing changes yet.
    let (generation, analysed, result) = next(&mut e);
    crate::develop::turn(
        e.document.edit.setup_mut(),
        crate::develop::QuarterTurn::Right,
    );
    crate::develop::turn(
        e.document.edit.setup_mut(),
        crate::develop::QuarterTurn::Left,
    );
    e.document.edit.setup_mut().flip_x = true;
    e.auto_straighten_ready(generation, &analysed, result);
    assert_eq!(e.document.edit.recipe().straighten, 0.);
    assert!(e.document.straighten.is_running(), "measured again");
    e.document.edit.setup_mut().flip_x = false;
    let (generation, analysed, result) = next(&mut e);
    e.auto_straighten_ready(generation, &analysed, result);
    // The stale analysis restarted once more for the flip back; take its result.
    let (generation, analysed, result) = next(&mut e);
    e.auto_straighten_ready(generation, &analysed, result);
    let angle = e.document.edit.recipe().straighten;
    assert!((angle + 3.).abs() < 0.2, "{angle}");
    let (steps, applied) = e.document.edit.history().steps();
    assert_eq!(applied, 1);
    assert_eq!(
        (steps[0].name.as_str(), steps[0].value.as_str()),
        ("Straighten", "Auto")
    );
    // Upright is left as it was.
    assert_eq!(e.document.edit.recipe().upright, upright);
    e.undo();
    assert_eq!(e.document.edit.recipe().straighten, 0.);
}

/// Pasting the Crop of a photo whose angle was Auto measures this photo, as Sync does
/// on the photos it changes; the angle found belongs to the Paste's step, which Undo
/// and Redo take back and forth without measuring again.
#[test]
fn pasting_an_auto_straighten_measures_this_photo_within_the_paste() {
    use crate::model::transform::AutoStraighten;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = falling_stripes();
    e.document.metadata = Some(image.metadata.clone());
    e.document.set_image(image);
    e.document.edit.setup_mut().wb = [1.; 3];
    // The Crop copied from a photo Auto turned by 5°.
    e.document.edit.setup_mut().set_auto_straighten(Some(5.));
    let mut crop = crate::model::settings_groups::GroupSelection::none();
    crop.set(
        crate::model::settings_groups::SettingGroup::Crop,
        crate::model::settings_groups::GroupInclusion::Included,
    );
    e.copy_settings(crop);
    *e.document.edit.setup_mut() = Recipe {
        wb: [1.; 3],
        ..Default::default()
    };
    // Pasted as the menu does, within a frame, which records the step.
    let frame = e.begin_edit_frame();
    e.paste_settings();
    e.finish_edit_frame(frame, &ctx);
    let r = e.document.edit.recipe();
    assert_eq!(r.straighten_auto, Some(AutoStraighten::Pending(0.)));
    assert!(e.document.straighten.is_running());
    let (generation, analysed, result) = loop {
        match e.rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(worker::Event::Straighten {
                generation,
                analysed,
                result,
                ..
            }) => break (generation, analysed, result),
            Ok(_) => continue,
            Err(err) => panic!("no Auto straighten result: {err}"),
        }
    };
    e.auto_straighten_ready(generation, &analysed, result);
    let r = e.document.edit.recipe();
    let angle = r.straighten;
    assert!((angle + 3.).abs() < 0.2, "{angle}");
    assert_eq!(r.auto_straighten(), Some(AutoStraighten::Measured(angle)));
    assert!(!e.document.straighten.is_running());
    let (steps, applied) = e.document.edit.history().steps();
    assert_eq!(applied, 1);
    assert_eq!(steps[0].name, "Paste Settings");
    e.undo();
    assert_eq!(e.document.edit.recipe().straighten, 0.);
    assert_eq!(e.document.edit.recipe().auto_straighten(), None);
    e.redo();
    assert_eq!(e.document.edit.recipe().straighten, angle);
    assert!(!e.document.straighten.is_running());
}

/// A photo flipped while its pasted Auto straighten is being measured is measured
/// again as it is now, rather than left waiting or given the stale angle.
#[test]
fn a_pasted_auto_straighten_flipped_before_it_is_measured_is_measured_again() {
    use crate::model::transform::AutoStraighten;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = falling_stripes();
    e.document.metadata = Some(image.metadata.clone());
    e.document.set_image(image);
    e.document.edit.setup_mut().set_auto_straighten(Some(5.));
    let mut crop = crate::model::settings_groups::GroupSelection::none();
    crop.set(
        crate::model::settings_groups::SettingGroup::Crop,
        crate::model::settings_groups::GroupInclusion::Included,
    );
    e.copy_settings(crop);
    *e.document.edit.setup_mut() = Recipe {
        wb: [1.; 3],
        straighten: 1.,
        ..Default::default()
    };
    e.paste_settings();
    assert!(e.document.straighten.is_running());
    crate::develop::mirror(
        e.document.edit.setup_mut(),
        crate::develop::Mirror::Horizontal,
    );
    assert_eq!(
        e.document.edit.recipe().straighten_auto,
        Some(AutoStraighten::Pending(-1.))
    );
    let next = |e: &mut Editor| loop {
        match e.rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(worker::Event::Straighten {
                generation,
                analysed,
                result,
                ..
            }) => break e.auto_straighten_ready(generation, &analysed, result),
            Ok(_) => continue,
            Err(err) => panic!("no Auto straighten result: {err}"),
        }
    };
    // Measured before the flip: it doesn't fit, and the photo is measured again.
    next(&mut e);
    assert!(e.document.edit.recipe().straighten_pending());
    assert!(e.document.straighten.is_running(), "measured again");
    next(&mut e);
    // Flipped, the stripes rise to the right.
    let r = e.document.edit.recipe();
    assert!((r.straighten - 3.).abs() < 0.2, "{}", r.straighten);
    assert_eq!(
        r.auto_straighten(),
        Some(AutoStraighten::Measured(r.straighten))
    );
}
