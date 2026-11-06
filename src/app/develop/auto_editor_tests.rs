//! Editor tests of `auto`: the whole editor driven through its frames.
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use std::sync::Arc;

#[test]
fn auto_is_one_undoable_step_that_keeps_edits_made_while_it_ran() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let (width, height) = (64u32, 48u32);
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width,
        height,
        // A dim, warm gradient: Auto brightens it, and the WB menu's Auto cools it.
        pixels: (0..width * height)
            .map(|i| {
                let v = 0.002 + 0.06 * (i % width) as f32 / width as f32;
                [v * 1.2, v, v * 0.8]
            })
            .collect(),
        metadata: Metadata {
            width,
            height,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
    editor.document.edit.setup_mut().saturation = 0.25;
    let before = editor.document.edit.recipe().clone();
    editor.start_auto(worker::AutoKind::Settings);
    assert!(editor.document.auto.is_running());
    // A second request while the first runs is ignored.
    editor.start_auto(worker::AutoKind::Settings);
    editor.document.edit.setup_mut().effects.clarity = 0.25;
    let start = std::time::Instant::now();
    while editor.document.auto.is_running() {
        assert!(start.elapsed().as_secs() < 60, "Auto did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
        editor.events(&ctx);
    }
    let auto = editor.document.edit.recipe().clone();
    assert!(auto.exposure > 1., "exposure {}", auto.exposure);
    // Auto sets the tone sliders, Vibrance and Saturation, as Lightroom's does; white
    // balance is the WB menu's Auto.
    assert!(auto.vibrance > 0., "vibrance {}", auto.vibrance);
    assert_ne!(auto.saturation, before.saturation);
    assert_eq!(
        (auto.wb, auto.temperature, auto.tint),
        (before.wb, before.temperature, before.tint)
    );
    // An edit made while it ran is kept.
    assert_eq!(auto.effects.clarity, 0.25);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 1);
    assert_eq!(steps[0].name, "Auto Settings");
    editor.undo();
    let mut expected = before;
    expected.effects.clarity = 0.25;
    assert_eq!(*editor.document.edit.recipe(), expected);
    editor.redo();
    assert_eq!(*editor.document.edit.recipe(), auto);

    editor.start_auto(worker::AutoKind::WhiteBalance);
    while editor.document.auto.is_running() {
        assert!(start.elapsed().as_secs() < 60, "Auto did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
        editor.events(&ctx);
    }
    let r = editor.document.edit.recipe();
    assert!(r.wb[0] < 1. && r.wb[2] > 1., "wb {:?}", r.wb);
    assert_eq!(r.auto_white_balance, Some([r.temperature, r.tint]));
    assert_eq!(r.exposure, auto.exposure);
    let (steps, _) = editor.document.edit.history().steps();
    assert_eq!(steps[1].name, "White Balance");
    // Pasted onto a photo, the values were not estimated for it: the WB menu says Custom.
    editor.copy_settings(crate::model::settings_groups::GroupSelection::default());
    editor.paste_settings();
    let r = editor.document.edit.recipe();
    assert!(r.wb[0] < 1. && r.wb[2] > 1., "wb {:?}", r.wb);
    assert_eq!(r.auto_white_balance, None);
}

#[test]
fn stale_auto_results_are_ignored_after_moving_on() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let before = editor.document.edit.recipe().clone();
    let mut auto = before.clone();
    auto.exposure = 2.;
    editor
        .tx
        .send(worker::Event::Auto {
            id: editor.document.load_id() + 1,
            kind: worker::AutoKind::Settings,
            result: Ok(Box::new(auto)),
        })
        .unwrap();
    editor.events(&ctx);
    assert_eq!(*editor.document.edit.recipe(), before);
    assert!(!editor.document.edit.history().can_undo());
}

#[test]
fn auto_shortcut_starts_auto_once_the_photo_is_decoded() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let modifiers = egui::Modifiers {
        command: true,
        mac_cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        shift: true,
        ..Default::default()
    };
    let press = |e: &mut Editor| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::U,
                    physical_key: Some(egui::Key::U),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
        output.textures_delta.clear();
    };
    press(&mut e);
    assert!(!e.document.auto.is_running(), "no photo yet");
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64],
        metadata: Metadata {
            width: 8,
            height: 8,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
    press(&mut e);
    assert!(e.document.auto.is_running());
}

#[test]
fn auto_arriving_mid_drag_lands_between_the_two_halves_of_the_drag() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let start = editor.document.edit.recipe().clone();
    // A Shadows drag is under way when the estimate arrives. Auto sets Shadows too,
    // so the drag does not make the estimate stale.
    editor.document.edit.setup_mut().shadows = 0.1;
    let mid = editor.document.edit.recipe().clone();
    editor
        .document
        .edit
        .history_mut()
        .observe(start.clone(), &mid, true);
    let mut auto = start.clone();
    auto.exposure = 1.;
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    let with_auto = editor.document.edit.recipe().clone();
    assert_eq!(with_auto.exposure, 1.);
    editor.document.edit.setup_mut().shadows = 0.2;
    let end = editor.document.edit.recipe().clone();
    editor
        .document
        .edit
        .history_mut()
        .observe(with_auto.clone(), &end, false);
    let names: Vec<_> = editor
        .document
        .edit
        .history()
        .steps()
        .0
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(names[1], "Auto Settings");
    // Undoing the rest of the drag keeps Auto; the next undo removes only Auto.
    editor.undo();
    assert_eq!(*editor.document.edit.recipe(), with_auto);
    editor.undo();
    assert_eq!(*editor.document.edit.recipe(), mid);
    editor.undo();
    assert_eq!(*editor.document.edit.recipe(), start);
}

#[test]
fn auto_runs_again_when_the_crop_changed_while_it_ran() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64],
        metadata: Metadata {
            width: 8,
            height: 8,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
    // An estimate for the uncropped photo arrives after the photo was cropped.
    let mut auto = editor.document.edit.recipe().clone();
    auto.exposure = 1.;
    editor.document.edit.setup_mut().crop = [0.1, 0.1, 0.9, 0.9];
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    assert_eq!(editor.document.edit.recipe().exposure, 0.);
    assert!(!editor.document.edit.history().can_undo());
    assert!(
        editor.document.auto.is_running(),
        "Auto runs again for the crop"
    );
}

#[test]
fn auto_runs_again_when_it_failed_on_settings_changed_since() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64],
        metadata: Metadata {
            width: 8,
            height: 8,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
    // An estimate that failed on the uncropped photo arrives after the photo was cropped.
    editor.document.auto_input = Some(editor.document.edit.recipe().clone());
    editor.document.edit.setup_mut().crop = [0.1, 0.1, 0.9, 0.9];
    editor.auto_ready(
        worker::AutoKind::Settings,
        Err("Auto: no usable pixels".into()),
    );
    assert!(
        !editor.messages.line().contains("usable"),
        "{}",
        editor.messages.line()
    );
    assert!(
        editor.document.auto.is_running(),
        "Auto runs again for the crop"
    );
}

#[test]
fn auto_is_off_while_its_settings_stand() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    assert!(!editor.auto_in_effect());
    let mut auto = editor.document.edit.recipe().clone();
    auto.exposure = 1.;
    auto.vibrance = 0.15;
    auto.saturation = 0.02;
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    assert_eq!(editor.document.edit.recipe().vibrance, 0.15);
    assert_eq!(editor.document.edit.recipe().saturation, 0.02);
    assert!(editor.auto_in_effect());
    // Any change, to a slider Auto sets or to what it measured, turns it back on, and
    // so does undoing Auto.
    editor.document.edit.setup_mut().vibrance = 0.;
    assert!(!editor.auto_in_effect());
    editor.document.edit.setup_mut().vibrance = 0.15;
    assert!(editor.auto_in_effect());
    editor.document.edit.setup_mut().saturation = 0.;
    assert!(!editor.auto_in_effect());
    editor.document.edit.setup_mut().saturation = 0.02;
    assert!(editor.auto_in_effect());
    editor.document.edit.setup_mut().exposure = 0.5;
    assert!(!editor.auto_in_effect());
    editor.document.edit.setup_mut().exposure = 1.;
    assert!(editor.auto_in_effect());
    editor.document.edit.setup_mut().crop[0] = 0.1;
    assert!(!editor.auto_in_effect());
    editor.document.edit.setup_mut().crop[0] = 0.;
    assert!(editor.auto_in_effect());
    // Adjustments Auto does not measure leave it off.
    editor.document.edit.setup_mut().effects.clarity = 0.3;
    editor.document.edit.setup_mut().curve.points[0] = [0., 0.2];
    editor.document.edit.setup_mut().preset_name = "Curve only".into();
    assert!(editor.auto_in_effect());
    editor.document.edit.setup_mut().effects.clarity = 0.;
    editor.document.edit.setup_mut().curve.points[0] = [0., 0.];
    editor.undo();
    assert!(!editor.auto_in_effect());
}

#[test]
fn auto_white_balance_on_a_jpeg_sets_its_relative_temp_and_tint() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let (width, height) = (64u32, 48u32);
    let metadata = Metadata {
        source: crate::camera_data::Source::Rendered,
        width,
        height,
        ..Default::default()
    };
    // A warm gradient, which Auto cools.
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: (0..width * height)
            .map(|i| {
                let v = 0.05 + 0.5 * (i % width) as f32 / width as f32;
                [v * 1.2, v, v * 0.8]
            })
            .collect(),
        metadata: metadata.clone(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
    *editor.document.edit.setup_mut() = Recipe::for_rendered(&metadata);
    editor.start_auto(worker::AutoKind::WhiteBalance);
    let start = std::time::Instant::now();
    while editor.document.auto.is_running() {
        assert!(start.elapsed().as_secs() < 60, "Auto did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
        editor.events(&ctx);
    }
    let r = editor.document.edit.recipe();
    assert!(
        r.incremental_temperature < 0.,
        "{}",
        r.incremental_temperature
    );
    assert_eq!((r.temperature, r.tint), (6500., 0.));
    // The sliders carry it (the gains stay 1), and the WB menu shows Auto.
    assert_eq!(r.wb, [1.; 3]);
    assert_eq!(
        r.auto_white_balance,
        Some([r.incremental_temperature, r.incremental_tint])
    );
}
