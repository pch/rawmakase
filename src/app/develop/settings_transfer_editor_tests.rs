//! Editor tests of `settings_transfer`: the whole editor driven through its frames.
use crate::app::*;
use crate::camera_data::Metadata;
use eframe::egui::{Pos2, Rect};

#[test]
fn paste_works_out_white_balance_for_this_camera_and_previous_pastes_the_last_photo() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let camera = |wb: [f32; 3]| Metadata {
        wb,
        daylight_wb: wb,
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let (first, second) = (camera([2., 1., 1.8]), camera([2.6, 1., 1.3]));
    // A photo from one camera, its settings copied.
    editor.document.metadata = Some(first.clone());
    let mut copied = Recipe {
        temperature: 4200.,
        tint: 8.,
        exposure: 0.4,
        ..Default::default()
    };
    copied.update_wb(&first);
    editor.document.edit.replace(copied.clone());
    editor.copy_settings(crate::model::settings_groups::GroupSelection::default());
    // Pasted onto a photo from another camera.
    editor.document.metadata = Some(second.clone());
    editor.document.edit.replace(Recipe::default());
    editor.paste_settings();
    let pasted = editor.document.edit.recipe().clone();
    assert_eq!((pasted.temperature, pasted.exposure), (4200., 0.4));
    let mut expected = pasted.clone();
    expected.update_wb(&second);
    assert_eq!(pasted.wb, expected.wb);
    assert_ne!(pasted.wb, copied.wb);
    // Moving to another photo makes this one's settings the Previous; opening that
    // photo again leaves it.
    let other = std::path::PathBuf::from("missing-previous-fixture.ARW");
    editor.open_raw(other.clone(), None);
    editor.document.metadata = Some(first);
    editor.document.opened_at(other.clone());
    editor.document.edit.setup_mut().exposure = -1.;
    editor.open_raw(other, None);
    editor.document.metadata = Some(second);
    editor.paste_previous();
    assert_eq!(editor.document.edit.recipe().exposure, 0.4);
}

#[test]
fn copy_settings_copies_the_chosen_groups_and_remembers_them() {
    use crate::model::settings_groups::{GroupInclusion, GroupSelection, SettingGroup};
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    editor.document.edit.setup_mut().exposure = 0.6;
    editor.document.edit.setup_mut().contrast = 0.3;
    editor.open_copy_dialog(crate::app::develop::settings_transfer::Transfer::Copy);
    // The dialog draws, with its buttons in view in the smallest window.
    for _ in 0..2 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000., 650.))),
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
    }
    let copy = ctx.memory(|m| m.area_rect(egui::Id::new("copy-settings")));
    assert!(copy.is_some_and(|r| r.bottom() <= 650.), "{copy:?}");
    let Some(Modal::CopySettings(dialog)) = &mut editor.modal else {
        panic!("Copy Settings is open");
    };
    dialog.groups = GroupSelection::none();
    dialog
        .groups
        .set(SettingGroup::Exposure, GroupInclusion::Included);
    editor.close_copy_dialog(crate::app::develop::settings_transfer::CopyChoice::Confirm);
    assert!(editor.modal.is_none());
    editor.document.edit.replace(Recipe::default());
    editor.paste_settings();
    assert_eq!(editor.document.edit.recipe().exposure, 0.6);
    assert_eq!(editor.document.edit.recipe().contrast, 0.);
    // The next Copy Settings starts from that choice; Cancel copies nothing.
    editor.open_copy_dialog(crate::app::develop::settings_transfer::Transfer::Copy);
    let Some(Modal::CopySettings(dialog)) = &editor.modal else {
        panic!("Copy Settings is open");
    };
    assert!(dialog.groups.contains(SettingGroup::Exposure));
    assert!(!dialog.groups.contains(SettingGroup::Contrast));
    editor.document.edit.setup_mut().exposure = -1.;
    editor.close_copy_dialog(crate::app::develop::settings_transfer::CopyChoice::Cancel);
    editor.document.edit.replace(Recipe::default());
    editor.paste_settings();
    assert_eq!(editor.document.edit.recipe().exposure, 0.6);
}

#[test]
fn return_confirms_copy_settings_and_escape_cancels_it() {
    use crate::model::settings_groups::GroupSelection;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    editor.document.edit.setup_mut().exposure = 0.6;
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000., 650.))),
                events,
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
    };
    // Escape cancels: nothing is copied.
    editor.open_copy_dialog(crate::app::develop::settings_transfer::Transfer::Copy);
    frame(&mut editor, vec![]);
    frame(&mut editor, vec![key(egui::Key::Escape)]);
    assert!(editor.modal.is_none());
    assert!(editor.clipboard.is_none());
    // Return confirms, even with a checkbox focused, which keeps its state.
    editor.open_copy_dialog(crate::app::develop::settings_transfer::Transfer::Copy);
    let chosen = GroupSelection::default();
    frame(&mut editor, vec![]);
    frame(&mut editor, vec![key(egui::Key::Tab)]);
    assert!(
        ctx.memory(|m| m.focused().is_some()),
        "a checkbox has focus"
    );
    frame(&mut editor, vec![key(egui::Key::Enter)]);
    frame(&mut editor, vec![]);
    assert!(editor.modal.is_none());
    let clipboard = editor.clipboard.as_ref().expect("settings copied");
    assert_eq!(clipboard.groups, chosen);
}
