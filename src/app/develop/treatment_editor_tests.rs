//! Editor tests of `treatment`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use eframe::egui::{Pos2, Rect};

#[test]
fn v_converts_to_black_and_white_with_the_auto_mix_as_one_step() {
    let ctx = egui::Context::default();
    let (mut editor, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    let before = editor.document.edit.recipe().clone();
    // V, through a whole frame of the Develop module.
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events: vec![egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| editor.draw(ui),
    );
    output.textures_delta.clear();
    // Held down, V repeats; the repeats change nothing.
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events: vec![egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| editor.draw(ui),
    );
    output.textures_delta.clear();
    let auto = editor
        .photo_colors()
        .unwrap()
        .auto_mix()
        .for_recipe(editor.document.edit.recipe());
    let r = editor.document.edit.recipe();
    assert!(r.effects.monochrome);
    assert_ne!(auto, [0.; 8]);
    assert_eq!(r.effects.gray_mix, auto);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(
        (applied, steps[0].name.as_str()),
        (1, "Convert to Black & White")
    );
    // Back to color keeps the mix for the next conversion, as Lightroom does.
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(!editor.document.edit.recipe().effects.monochrome);
    assert_eq!(editor.document.edit.recipe().effects.gray_mix, auto);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!((applied, steps[1].name.as_str()), (2, "Convert to Color"));
    editor.undo();
    editor.undo();
    assert_eq!(*editor.document.edit.recipe(), before);

    // The B&W panel's Auto brings back the Auto mix after a slider moved, as one step.
    editor.redo();
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.setup_mut().effects.gray_mix[3] = 0.6
    });
    in_edit_frame(&ctx, &mut editor, Editor::auto_black_white_mix);
    assert_eq!(editor.document.edit.recipe().effects.gray_mix, auto);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 3);
    assert_eq!(
        (steps[2].name.as_str(), steps[2].value.as_str()),
        ("Black & White Mix", "Auto")
    );
    // With the preference off, the first conversion keeps the mix at zero.
    let session = crate::app::preferences::session::Session {
        no_auto_black_white_mix: true,
        ..Default::default()
    };
    let (mut editor, _) = editor_with_blue_photo(&ctx, session, true);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(editor.document.edit.recipe().effects.monochrome);
    assert_eq!(editor.document.edit.recipe().effects.gray_mix, [0.; 8]);
}

#[test]
fn converting_while_the_photo_decodes_waits_for_its_auto_mix() {
    let ctx = egui::Context::default();
    // V twice while decoding: the second cancels the first.
    let (mut editor, image) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        false,
    );
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, Editor::finish_pending_treatment);
    assert!(!editor.document.edit.recipe().effects.monochrome);
    assert_eq!(editor.document.edit.history().steps().1, 0);
    // Once: the conversion waits for the photo.
    let (mut editor, image) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        false,
    );
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(!editor.document.edit.recipe().effects.monochrome);
    assert_eq!(editor.document.edit.history().steps().1, 0);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, Editor::finish_pending_treatment);
    let r = editor.document.edit.recipe();
    assert!(r.effects.monochrome);
    assert_ne!(r.effects.gray_mix, [0.; 8]);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(
        (applied, steps[0].name.as_str()),
        (1, "Convert to Black & White")
    );
    // An edit in the frame the photo decodes in drops the request too.
    let (mut editor, image) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        false,
    );
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.setup_mut().exposure = 0.3
    });
    assert!(!editor.document.edit.recipe().effects.monochrome);
    in_edit_frame(&ctx, &mut editor, |_| {});
    assert!(!editor.document.edit.recipe().effects.monochrome);
    // A request lapses when the recipe changes otherwise before the photo decodes.
    let (mut editor, image) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        false,
    );
    let before_exposure = editor.document.edit.recipe().exposure;
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.setup_mut().exposure = 0.5
    });
    // Undone again before it decodes: the request still lapsed.
    in_edit_frame(&ctx, &mut editor, Editor::undo);
    assert_eq!(editor.document.edit.recipe().exposure, before_exposure);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, Editor::finish_pending_treatment);
    assert!(!editor.document.edit.recipe().effects.monochrome);
}

#[test]
fn auto_ends_a_conversion_waiting_for_the_photo() {
    let ctx = egui::Context::default();
    let (mut editor, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        false,
    );
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(editor.document.pending_treatment.is_some());
    let mut auto = editor.document.edit.recipe().clone();
    auto.exposure = 1.;
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    // Any other edit drops the waiting conversion, as an edit made with a slider does.
    assert_eq!(editor.document.edit.recipe().exposure, 1.);
    assert!(editor.document.pending_treatment.is_none());
}
