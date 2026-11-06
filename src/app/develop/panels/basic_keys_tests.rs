//! The Basic slider keys in the app, driven by key events and frames as a person
//! would: what each key steps, what History records, and when they do nothing.
use super::*;
use crate::app::develop::state::Tool;
use crate::app::{Editor, Module};
use crate::camera_data::{CameraImage, Metadata, Source};
use eframe::egui::{self, Key, Modifiers, Pos2, Rect, Vec2};
use std::sync::Arc;

/// An editor in Develop with a decoded 8×8 photo described by `metadata`.
fn develop(metadata: Metadata) -> Editor {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    e.onboarding.visible = false;
    open(&mut e, metadata);
    e
}

/// Shows a decoded photo described by `metadata`, as opening one does.
fn open(e: &mut Editor, metadata: Metadata) {
    e.document.metadata = Some(metadata.clone());
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64],
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
}

fn raw() -> Metadata {
    Metadata {
        width: 8,
        height: 8,
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    }
}

fn jpeg() -> Metadata {
    Metadata {
        source: Source::Rendered,
        ..raw()
    }
}

fn input(events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
        events,
        ..Default::default()
    }
}

/// One whole frame of the editor with `events`.
fn frame(e: &mut Editor, events: Vec<egui::Event>) {
    frame_with(e, input(events));
}

fn frame_with(e: &mut Editor, input: egui::RawInput) {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(input, |ui| e.draw(ui));
    output.textures_delta.clear();
}

fn key_event(key: Key, physical: Key, repeat: bool, modifiers: Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(physical),
        pressed: true,
        repeat,
        modifiers,
    }
}

fn down(key: Key, modifiers: Modifiers) -> egui::Event {
    key_event(key, key, false, modifiers)
}

fn up(key: Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

/// `key` pressed and released in a frame of its own.
fn tap(e: &mut Editor, key: Key, modifiers: Modifiers) {
    frame(e, vec![down(key, modifiers), up(key)]);
}

/// Selects `id` with `.` from no selection, as a person would.
fn select(e: &mut Editor, id: ParameterId) {
    e.view.selected_setting = None;
    let presses = ORDER.iter().position(|s| *s == id).unwrap() + 1;
    for _ in 0..presses {
        tap(e, Key::Period, Modifiers::NONE);
    }
    assert_eq!(e.view.selected_setting, Some(id));
}

/// History's step names and values, oldest first.
fn steps(e: &Editor) -> Vec<(String, String)> {
    let (steps, applied) = e.document.edit.history().steps();
    steps[..applied]
        .iter()
        .map(|s| (s.name.clone(), s.value.clone()))
        .collect()
}

/// A file dropped on the window.
#[derive(Debug)]
struct Dropped(std::path::PathBuf);
impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Err("not read".into())
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn each_press_steps_the_selected_slider_as_one_named_history_step() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Exposure);
    assert!(steps(&e).is_empty(), "selecting is no edit");
    assert!(!e.document.edit.save_state().needs_save());
    tap(&mut e, Key::Equals, Modifiers::NONE);
    assert!(close(e.document.edit.recipe().exposure, 0.1));
    // Shift+= arrives as `+` typed on the `=` key.
    frame(
        &mut e,
        vec![
            key_event(Key::Plus, Key::Equals, false, Modifiers::SHIFT),
            up(Key::Equals),
        ],
    );
    assert!(close(e.document.edit.recipe().exposure, 0.43));
    tap(&mut e, Key::Minus, Modifiers::ALT);
    assert!(close(e.document.edit.recipe().exposure, 0.41));
    tap(&mut e, Key::Minus, Modifiers::SHIFT | Modifiers::ALT);
    assert!(close(e.document.edit.recipe().exposure, 0.39));
    tap(&mut e, Key::Minus, Modifiers::NONE);
    assert!(close(e.document.edit.recipe().exposure, 0.29));
    let named = |value: &str| ("Exposure".to_string(), value.to_string());
    assert_eq!(
        steps(&e),
        [
            named("+0.10"),
            named("+0.43"),
            named("+0.41"),
            named("+0.39"),
            named("+0.29")
        ]
    );
    assert!(e.document.edit.save_state().needs_save());
    // Undo takes back one press.
    e.undo();
    assert!(close(e.document.edit.recipe().exposure, 0.39));
}

#[test]
fn the_first_key_selects_an_end_and_unselected_or_modified_keys_do_nothing() {
    let mut e = develop(raw());
    let before = e.document.edit.recipe().clone();
    tap(&mut e, Key::Equals, Modifiers::NONE);
    tap(&mut e, Key::Minus, Modifiers::SHIFT);
    assert_eq!(*e.document.edit.recipe(), before, "nothing selected yet");
    tap(&mut e, Key::Period, Modifiers::SHIFT);
    tap(&mut e, Key::Comma, Modifiers::ALT);
    assert_eq!(e.view.selected_setting, None);
    tap(&mut e, Key::Comma, Modifiers::NONE);
    assert_eq!(e.view.selected_setting, Some(ParameterId::Saturation));
    tap(&mut e, Key::Period, Modifiers::NONE);
    assert_eq!(e.view.selected_setting, Some(ParameterId::Temperature));
    // Ctrl and Cmd are never the slider keys.
    tap(&mut e, Key::Equals, Modifiers::CTRL);
    tap(&mut e, Key::Period, Modifiers::COMMAND);
    assert_eq!(e.view.selected_setting, Some(ParameterId::Temperature));
    assert_eq!(*e.document.edit.recipe(), before);
}

#[test]
fn cmd_equals_still_zooms_and_cmd_comma_still_opens_preferences() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Exposure);
    let before = e.document.edit.recipe().clone();
    // Cmd+- steps the zoom out of 400%, and moves no slider.
    e.view.zoom.set(4.);
    tap(&mut e, Key::Minus, Modifiers::COMMAND);
    assert!(!(e.view.zoom.on && e.view.zoom.level == 4.), "Cmd+- zooms");
    tap(&mut e, Key::Equals, Modifiers::COMMAND);
    assert_eq!(*e.document.edit.recipe(), before);
    tap(&mut e, Key::Comma, Modifiers::COMMAND);
    assert!(e.preferences.open);
    assert_eq!(e.view.selected_setting, Some(ParameterId::Exposure));
    // Preferences has the keys now.
    tap(&mut e, Key::Equals, Modifiers::NONE);
    assert_eq!(*e.document.edit.recipe(), before);
}

#[test]
fn held_key_repeats_and_presses_in_one_frame_are_a_step_each() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Contrast);
    frame(
        &mut e,
        vec![
            down(Key::Equals, Modifiers::NONE),
            key_event(Key::Equals, Key::Equals, true, Modifiers::NONE),
            key_event(Key::Equals, Key::Equals, true, Modifiers::NONE),
        ],
    );
    assert!(close(e.document.edit.recipe().contrast, 0.15));
    frame(
        &mut e,
        vec![
            up(Key::Equals),
            down(Key::Minus, Modifiers::NONE),
            up(Key::Minus),
            down(Key::Minus, Modifiers::NONE),
            up(Key::Minus),
        ],
    );
    assert!(close(e.document.edit.recipe().contrast, 0.05));
    let values: Vec<_> = steps(&e).into_iter().map(|(_, v)| v).collect();
    assert_eq!(values, ["+5", "+10", "+15", "+10", "+5"]);
    assert!(steps(&e).iter().all(|(name, _)| name == "Contrast"));
}

#[test]
fn a_step_at_the_end_of_the_slider_records_nothing_and_imported_values_survive() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Exposure);
    e.document.edit.setup_mut().exposure = 5.;
    tap(&mut e, Key::Equals, Modifiers::SHIFT);
    assert_eq!(e.document.edit.recipe().exposure, 5.);
    assert!(steps(&e).is_empty());
    // An imported +7 EV, beyond the slider: never pushed further, nor clamped.
    e.document.edit.setup_mut().exposure = 7.;
    tap(&mut e, Key::Equals, Modifiers::NONE);
    assert_eq!(e.document.edit.recipe().exposure, 7.);
    tap(&mut e, Key::Minus, Modifiers::NONE);
    assert!(close(e.document.edit.recipe().exposure, 6.9));
    assert_eq!(steps(&e).len(), 1);
}

#[test]
fn a_temp_step_recomputes_the_white_balance_and_ends_auto() {
    let mut e = develop(raw());
    let m = raw();
    e.document.edit.setup_mut().reset_white_balance(&m);
    let start = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().auto_white_balance = Some([start.temperature, start.tint]);
    select(&mut e, ParameterId::Temperature);
    tap(&mut e, Key::Minus, Modifiers::NONE);
    let r = e.document.edit.recipe();
    assert!(close(r.temperature, start.temperature - 50.));
    assert_eq!(r.auto_white_balance, None);
    let mut expected = start.clone();
    expected.temperature -= 50.;
    expected.update_wb(&m);
    assert_eq!(r.wb, expected.wb);
    assert_ne!(r.wb, start.wb);
    assert_eq!(steps(&e).last().unwrap().0, "Temp");
}

#[test]
fn temp_and_tint_stay_selected_between_raw_and_rendered_photos() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Temperature);
    let raw_temp = e.document.edit.recipe().temperature;
    tap(&mut e, Key::Equals, Modifiers::NONE);
    assert!(close(e.document.edit.recipe().temperature, raw_temp + 50.));
    // A JPEG: the same selection moves its relative Temp, by Tint's steps.
    open(&mut e, jpeg());
    assert_eq!(e.view.selected_setting, Some(ParameterId::Temperature));
    let temp = e.document.edit.recipe().temperature;
    tap(&mut e, Key::Equals, Modifiers::NONE);
    tap(&mut e, Key::Equals, Modifiers::SHIFT);
    assert!(close(e.document.edit.recipe().incremental_temperature, 25.));
    assert_eq!(e.document.edit.recipe().temperature, temp);
    // `.` to Tint on the JPEG, then back on a RAW file Tint stays selected.
    tap(&mut e, Key::Period, Modifiers::NONE);
    assert_eq!(e.view.selected_setting, Some(ParameterId::Tint));
    tap(&mut e, Key::Minus, Modifiers::ALT);
    assert!(close(e.document.edit.recipe().incremental_tint, -1.));
    open(&mut e, raw());
    let tint = e.document.edit.recipe().tint;
    tap(&mut e, Key::Minus, Modifiers::NONE);
    assert!(close(e.document.edit.recipe().tint, tint - 5.));
}

#[test]
fn a_monochrome_photo_skips_and_keeps_white_balance() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Tint);
    open(
        &mut e,
        Metadata {
            monochrome: true,
            ..raw()
        },
    );
    let before = e.document.edit.recipe().clone();
    // Tint stays selected but disabled: the keys leave it.
    tap(&mut e, Key::Equals, Modifiers::NONE);
    assert_eq!(*e.document.edit.recipe(), before);
    tap(&mut e, Key::Period, Modifiers::NONE);
    assert_eq!(e.view.selected_setting, Some(ParameterId::Exposure));
    tap(&mut e, Key::Comma, Modifiers::NONE);
    assert_eq!(e.view.selected_setting, Some(ParameterId::Saturation));
    // Back on a colour photo, Tint is there again.
    open(&mut e, raw());
    tap(&mut e, Key::Period, Modifiers::NONE);
    tap(&mut e, Key::Period, Modifiers::NONE);
    assert_eq!(e.view.selected_setting, Some(ParameterId::Tint));
}

#[test]
fn the_selection_stays_across_photos_and_modules_and_keys_rest_in_the_library() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Shadows);
    e.module = Module::Library;
    let before = e.document.edit.recipe().clone();
    tap(&mut e, Key::Equals, Modifiers::NONE);
    tap(&mut e, Key::Period, Modifiers::NONE);
    assert_eq!(*e.document.edit.recipe(), before);
    assert_eq!(e.view.selected_setting, Some(ParameterId::Shadows));
    e.module = Module::Develop;
    // Opening another photo keeps the selection too.
    e.open_raw(std::path::PathBuf::from("missing-basic-keys.ARW"), None);
    assert_eq!(e.view.selected_setting, Some(ParameterId::Shadows));
    // Still loading: nothing to step.
    tap(&mut e, Key::Equals, Modifiers::NONE);
    assert_eq!(e.document.edit.recipe().shadows, 0.);
    open(&mut e, raw());
    let loaded = e.document.load_id();
    e.document.finish_load(loaded);
    tap(&mut e, Key::Equals, Modifiers::NONE);
    assert!(close(e.document.edit.recipe().shadows, 0.05));
}

#[test]
fn keys_step_the_global_setting_with_a_tool_open_or_a_mask_selected() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Exposure);
    e.view.tool = Tool::Crop;
    tap(&mut e, Key::Equals, Modifiers::NONE);
    assert!(close(e.document.edit.recipe().exposure, 0.1));
    e.view.tool = Tool::Mask;
    e.document
        .edit
        .setup_mut()
        .masks
        .push(crate::model::masks::MaskGroup::default());
    e.view.masking.selected = Some(0);
    let mask = e.document.edit.recipe().masks[0].clone();
    tap(&mut e, Key::Equals, Modifiers::NONE);
    assert!(close(e.document.edit.recipe().exposure, 0.2));
    assert_eq!(e.document.edit.recipe().masks[0], mask);
}

#[test]
fn keys_wait_while_the_pointer_is_down_or_a_gesture_is_held() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Exposure);
    let mut pressed = input(vec![
        egui::Event::PointerMoved(Pos2::new(5., 5.)),
        egui::Event::PointerButton {
            pos: Pos2::new(5., 5.),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    frame_with(&mut e, pressed.clone());
    pressed.events = vec![down(Key::Equals, Modifiers::NONE), up(Key::Equals)];
    frame_with(&mut e, pressed);
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    frame(
        &mut e,
        vec![egui::Event::PointerButton {
            pos: Pos2::new(5., 5.),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    // A dial still turning on a control surface holds a gesture too.
    let start = e.document.edit.recipe().clone();
    let held = e.document.edit.begin();
    e.document.edit.recipe_mut().contrast = 0.3;
    e.document
        .edit
        .finish(held, crate::edit_session::Gesture::Held);
    assert!(e.document.edit.history().in_gesture());
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(input(vec![down(Key::Equals, Modifiers::NONE)]), |ui| {
        e.develop_key_steps(ui.ctx(), false)
    });
    output.textures_delta.clear();
    assert_eq!(e.document.edit.recipe().exposure, start.exposure);
}

#[test]
fn keys_rest_while_typing_in_a_dialog_or_closing() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Exposure);
    let ctx = e.context.clone();
    let step_with = |e: &mut Editor, input: egui::RawInput, before: &dyn Fn(&egui::Context)| {
        let mut output = ctx.run_ui(input, |ui| {
            before(ui.ctx());
            e.develop_key_steps(ui.ctx(), false);
        });
        output.textures_delta.clear();
    };
    let press = || input(vec![down(Key::Equals, Modifiers::NONE)]);
    // A text field has the keys.
    step_with(&mut e, press(), &|ctx| {
        egui::Window::new("field").show(ctx, |ui| {
            let mut text = String::new();
            ui.text_edit_singleline(&mut text).request_focus();
        });
    });
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    // A dialog over the photo.
    e.view.shortcuts = true;
    step_with(&mut e, press(), &|_| {});
    e.view.shortcuts = false;
    // A busy task.
    assert!(e.activity.begin_dialog());
    step_with(&mut e, press(), &|_| {});
    e.activity = Default::default();
    // Closing, or about to.
    for set in [
        |e: &mut Editor| e.close_confirm = true,
        |e: &mut Editor| e.close_after_work = true,
        |e: &mut Editor| e.quit_by = Some(std::time::Instant::now()),
    ] {
        set(&mut e);
        step_with(&mut e, press(), &|_| {});
        e.close_confirm = false;
        e.close_after_work = false;
        e.quit_by = None;
    }
    let mut closing = press();
    closing.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            events: vec![egui::ViewportEvent::Close],
            ..Default::default()
        },
    );
    step_with(&mut e, closing, &|_| {});
    // Files dropped on the window.
    let mut dropped = press();
    dropped
        .dropped_files
        .push(Arc::new(Dropped("dropped.ARW".into())));
    step_with(&mut e, dropped, &|_| {});
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    assert!(steps(&e).is_empty());
    // And with none of that, it steps.
    step_with(&mut e, press(), &|_| {});
    assert!(close(e.document.edit.recipe().exposure, 0.1));
}

#[test]
fn a_close_requested_in_the_same_frame_is_not_cut_short_by_a_step() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Exposure);
    let mut closing = input(vec![down(Key::Equals, Modifiers::NONE)]);
    closing.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            events: vec![egui::ViewportEvent::Close],
            ..Default::default()
        },
    );
    frame_with(&mut e, closing);
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    assert!(!e.document.edit.save_state().needs_save());
}

#[test]
fn undo_in_the_same_frame_as_a_step_keeps_history_consistent() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Exposure);
    tap(&mut e, Key::Equals, Modifiers::NONE);
    let cmd = Modifiers {
        command: true,
        mac_cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Default::default()
    };
    frame(
        &mut e,
        vec![
            egui::Event::ModifiersChanged(cmd),
            down(Key::Z, cmd),
            up(Key::Z),
            down(Key::Minus, Modifiers::NONE),
            up(Key::Minus),
        ],
    );
    // Undone to 0, then stepped down from there: the step is applied after the
    // frame's Undo, as one step of its own, and nothing is left to redo.
    assert!(close(e.document.edit.recipe().exposure, -0.1));
    assert_eq!(steps(&e), [("Exposure".to_string(), "-0.10".to_string())]);
    assert!(!e.document.edit.history().can_redo());
    e.undo();
    assert_eq!(e.document.edit.recipe().exposure, 0.);
}

#[test]
fn a_label_click_selects_its_slider_and_lights_it() {
    let ctx = egui::Context::default();
    let mut value = 0.;
    let mut event = crate::app::ui::widgets::SliderEvent::None;
    let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(300., 600.));
    let mut run = |events: Vec<egui::Event>, value: &mut f32| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(rect),
                events,
                ..Default::default()
            },
            |ui| {
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    let e = crate::app::ui::widgets::keyed_setting_slider(
                        ui,
                        &mut Default::default(),
                        ParameterId::Exposure,
                        value,
                        0.,
                        false,
                    );
                    if e != crate::app::ui::widgets::SliderEvent::None {
                        event = e;
                    }
                });
            },
        );
        output.textures_delta.clear();
    };
    // The label sits at the row's left, before the rail.
    let at = Pos2::new(10., 12.);
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    run(vec![], &mut value);
    run(
        vec![egui::Event::PointerMoved(at), button(true)],
        &mut value,
    );
    run(vec![button(false)], &mut value);
    assert_eq!(event, crate::app::ui::widgets::SliderEvent::LabelClicked);
    assert_eq!(value, 0., "a click on the label changes nothing");
}

#[test]
fn a_minus_typed_into_a_field_that_return_then_leaves_is_no_step() {
    let mut e = develop(raw());
    select(&mut e, ParameterId::Exposure);
    let ctx = e.context.clone();
    let mut text = String::new();
    let run = |e: &mut Editor, events: Vec<egui::Event>, text: &mut String| {
        let mut output = ctx.run_ui(input(events), |ui| {
            // As `draw` does: whether a field has the keys, before any takes them.
            let typing = ui.ctx().text_edit_focused();
            ui.add(egui::TextEdit::singleline(text).id(egui::Id::new("field")));
            e.develop_key_steps(ui.ctx(), typing);
        });
        output.textures_delta.clear();
    };
    ctx.memory_mut(|m| m.request_focus(egui::Id::new("field")));
    run(&mut e, vec![], &mut text);
    assert!(ctx.text_edit_focused());
    // The `-`, its text, and Return, which takes the keys from the field, all in
    // one frame.
    run(
        &mut e,
        vec![
            down(Key::Minus, Modifiers::NONE),
            egui::Event::Text("-".into()),
            down(Key::Enter, Modifiers::NONE),
        ],
        &mut text,
    );
    assert_eq!(text, "-");
    assert!(!ctx.text_edit_focused(), "Return left the field");
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    assert!(steps(&e).is_empty());
}
