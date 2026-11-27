//! Slider Keys in the app, driven by key and wheel events and frames as a person
//! would: what a held letter moves, what History records, and what the mode takes.
use super::*;
use crate::app::Module;
use crate::camera_data::{CameraImage, Metadata};
use eframe::egui::{Modifiers, Pos2, Rect, Vec2};
use std::sync::Arc;

fn develop() -> Editor {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let metadata = Metadata {
        width: 8,
        height: 8,
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    e.document.metadata = Some(metadata.clone());
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64].into(),
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
    e
}

/// The text drawn in a frame, with its color and where it landed.
type Texts = Vec<(String, egui::Color32, Rect)>;

/// Runs a frame with `events` and returns the text it drew.
fn frame(e: &mut Editor, events: Vec<egui::Event>) -> Texts {
    let ctx = e.context.clone();
    let mut input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(2400., 900.))),
        events,
        ..Default::default()
    };
    // As eframe's `raw_input_hook`, where Slider Keys reads the keys.
    e.slider_key_input(&ctx, &mut input);
    let mut output = ctx.run_ui(input, |ui| e.draw(ui));
    output.textures_delta.clear();
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(t) => Some((
                t.galley.text().to_owned(),
                t.galley.job.sections.first()?.format.color,
                t.visual_bounding_rect(),
            )),
            _ => None,
        })
        .collect()
}

/// Whether a slider label `name` is drawn lit, as the keys' slider is.
fn lit(e: &Editor, texts: &Texts, name: &str) -> bool {
    let bright =
        crate::app::ui::theme::palette(&e.context).tone(crate::app::ui::design::Tone::BRIGHT);
    texts
        .iter()
        .any(|(t, color, _)| t == name && *color == bright)
}

fn drawn(texts: &Texts, name: &str) -> bool {
    texts.iter().any(|(t, ..)| t == name)
}

fn key(key: Key, pressed: bool, repeat: bool, modifiers: Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat,
        modifiers,
    }
}

fn press(k: Key, modifiers: Modifiers) -> egui::Event {
    key(k, true, false, modifiers)
}

fn release(k: Key) -> egui::Event {
    key(k, false, false, Modifiers::NONE)
}

fn wheel(lines: f32) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: Vec2::new(0., lines),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    }
}

fn steps(e: &Editor) -> Vec<(String, String)> {
    let (steps, applied) = e.document.edit.history().steps();
    steps[..applied]
        .iter()
        .map(|s| (s.name.clone(), s.value.clone()))
        .collect()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn a_held_letter_moves_its_slider_as_one_history_step() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    assert!(steps(&e).is_empty(), "the letter alone is no edit");
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(
        &mut e,
        vec![key(Key::ArrowUp, true, true, Modifiers::SHIFT)],
    );
    frame(&mut e, vec![press(Key::ArrowLeft, Modifiers::ALT)]);
    frame(&mut e, vec![wheel(2.)]);
    // 0.1 + 0.33 - 0.02 + 2 × 0.1
    assert!(close(e.document.edit.recipe().exposure, 0.61));
    assert_eq!(e.view.slider_keys.moved(), Some(ParameterId::Exposure));
    assert!(steps(&e).is_empty(), "still held");
    frame(&mut e, vec![release(Key::E)]);
    assert_eq!(steps(&e), [("Exposure".to_string(), "+0.61".to_string())]);
    e.undo();
    assert!(close(e.document.edit.recipe().exposure, 0.));
}

#[test]
fn the_mode_takes_its_letters_and_leaves_the_rest_when_off() {
    let mut e = develop();
    // Off: W is the White Balance tool, and arrows without a letter step nothing.
    frame(
        &mut e,
        vec![press(Key::W, Modifiers::NONE), release(Key::W)],
    );
    assert!(e.view.is(Tool::WhiteBalance));
    e.view.set_tool(Tool::None);
    e.view.slider_keys.on = true;
    frame(
        &mut e,
        vec![press(Key::W, Modifiers::NONE), release(Key::W)],
    );
    assert!(
        !e.view.is(Tool::WhiteBalance),
        "the mode's letter runs nothing"
    );
    let before = e.document.edit.recipe().clone();
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    assert_eq!(*e.document.edit.recipe(), before);
    // Cmd with a letter is never the mode's.
    frame(
        &mut e,
        vec![
            press(Key::E, Modifiers::COMMAND),
            press(Key::ArrowUp, Modifiers::COMMAND),
        ],
    );
    assert_eq!(*e.document.edit.recipe(), before);
}

#[test]
fn in_masking_the_selected_masks_slider_moves() {
    let mut e = develop();
    e.document
        .edit
        .change(None, |r| r.masks.push(Default::default()));
    e.view.set_tool(Tool::Mask);
    e.view.masking.selected = Some(0);
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::C, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::C)]);
    let recipe = e.document.edit.recipe();
    assert!(close(recipe.masks[0].adjust.contrast, 0.05));
    assert!(close(recipe.contrast, 0.));
    assert_eq!(
        steps(&e).last(),
        Some(&("Mask 1 Contrast".to_string(), "+5".to_string()))
    );
    // A mask the user named goes by its name, over the photo and in History.
    e.document
        .edit
        .change(None, |r| r.masks[0].name = "Sky".into());
    frame(&mut e, vec![press(Key::C, Modifiers::NONE)]);
    let texts = frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    assert!(drawn(&texts, "Sky Contrast  +10"), "{texts:?}");
    frame(&mut e, vec![release(Key::C)]);
    assert_eq!(
        steps(&e).last(),
        Some(&("Sky Contrast".to_string(), "+10".to_string()))
    );
}

#[test]
fn a_letter_with_a_modifier_runs_its_command_while_the_mode_is_on() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(
        &mut e,
        vec![press(Key::W, Modifiers::SHIFT), release(Key::W)],
    );
    assert!(e.view.is(Tool::Mask), "Shift+W opens Masking");
    e.view.set_tool(Tool::None);
    frame(
        &mut e,
        vec![press(Key::T, Modifiers::SHIFT), release(Key::T)],
    );
    assert!(e.view.is(Tool::Guided), "Shift+T opens Guided Upright");
    e.view.set_tool(Tool::None);
    // Shift pressed while W is held: W's repeats stay the mode's, and Shift
    // makes the arrows' steps large.
    frame(&mut e, vec![press(Key::W, Modifiers::NONE)]);
    frame(
        &mut e,
        vec![
            key(Key::W, true, true, Modifiers::SHIFT),
            press(Key::ArrowUp, Modifiers::SHIFT),
        ],
    );
    assert!(!e.view.is(Tool::Mask), "a held W's repeat opens nothing");
    frame(&mut e, vec![release(Key::W)]);
    assert_eq!(
        steps(&e).last(),
        Some(&("Whites".to_string(), "+20".to_string()))
    );
}

#[test]
fn a_letter_alone_changes_nothing_and_the_slider_is_lit_only_while_it_moves() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    let before = e.document.edit.recipe().clone();
    frame(&mut e, vec![press(Key::C, Modifiers::NONE)]);
    let texts = frame(&mut e, vec![]);
    assert!(drawn(&texts, "Contrast"), "the Basic panel is drawn");
    assert!(
        !lit(&e, &texts, "Contrast"),
        "the letter alone lights nothing"
    );
    frame(&mut e, vec![release(Key::C)]);
    assert_eq!(*e.document.edit.recipe(), before);
    assert_eq!(e.view.selected_setting, None);
    assert!(steps(&e).is_empty());
    // An arrow with the letter held lights the slider until the letter is let go.
    frame(&mut e, vec![press(Key::C, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    let texts = frame(&mut e, vec![]);
    assert!(lit(&e, &texts, "Contrast"));
    assert!(drawn(&texts, "Contrast  +5"), "the label over the photo");
    frame(&mut e, vec![release(Key::C)]);
    let texts = frame(&mut e, vec![]);
    assert!(!lit(&e, &texts, "Contrast"));
    assert_eq!(e.view.selected_setting, None);
}

#[test]
fn in_masking_the_moving_masks_slider_is_lit() {
    let mut e = develop();
    e.document
        .edit
        .change(None, |r| r.masks.push(Default::default()));
    e.view.set_tool(Tool::Mask);
    e.view.masking.selected = Some(0);
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::H, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowDown, Modifiers::NONE)]);
    let texts = frame(&mut e, vec![]);
    assert!(
        lit(&e, &texts, "Highlights"),
        "the mask's Highlights slider"
    );
    assert!(!lit(&e, &texts, "Shadows"));
    frame(&mut e, vec![release(Key::H)]);
    let texts = frame(&mut e, vec![]);
    assert!(drawn(&texts, "Highlights"));
    assert!(!lit(&e, &texts, "Highlights"));
}

#[test]
fn the_letters_go_by_their_place() {
    let placed = |k: Key, place: Key, pressed: bool| egui::Event::Key {
        key: k,
        physical_key: Some(place),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    let mut e = develop();
    e.view.slider_keys.on = true;
    // Dvorak types E on D's place: Dehaze moves, not Exposure.
    frame(&mut e, vec![placed(Key::E, Key::D, true)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![placed(Key::E, Key::D, false)]);
    let recipe = e.document.edit.recipe();
    assert!(recipe.effects.dehaze > 0.);
    assert!(close(recipe.exposure, 0.));
    // AZERTY types Q on A's place: Saturation moves, and Q opens no tool.
    frame(&mut e, vec![placed(Key::Q, Key::A, true)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![placed(Key::Q, Key::A, false)]);
    assert!(e.document.edit.recipe().saturation > 0.);
    assert!(!e.view.is(Tool::Remove));
    assert_eq!(steps(&e).len(), 2);
}

#[test]
fn a_trackpad_steps_once_per_twelve_points_and_forgets_the_rest_on_release() {
    let points = |p: f32| egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0., p),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    };
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![points(7.)]);
    assert!(
        close(e.document.edit.recipe().exposure, 0.),
        "short of a step"
    );
    frame(&mut e, vec![points(7.)]);
    assert!(
        close(e.document.edit.recipe().exposure, 0.1),
        "14 points, one step"
    );
    frame(&mut e, vec![points(-30.)]);
    // 2 left over, less 30: two steps down, 4 short of a third.
    assert!(close(e.document.edit.recipe().exposure, -0.1));
    frame(&mut e, vec![release(Key::E)]);
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![points(-8.)]);
    assert!(
        close(e.document.edit.recipe().exposure, -0.1),
        "the rest is gone"
    );
    frame(&mut e, vec![release(Key::E)]);
}

#[test]
fn in_crop_x_and_a_are_the_crops_and_the_other_letters_the_modes() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    e.document.edit.setup_mut().crop = [0.1, 0.2, 0.5, 0.6];
    e.view.toggle(Tool::Crop);
    frame(&mut e, vec![]);
    let aspect = e.view.chosen_aspect;
    let crop = e.document.edit.recipe().crop;
    frame(
        &mut e,
        vec![press(Key::A, Modifiers::NONE), release(Key::A)],
    );
    assert_ne!(e.view.chosen_aspect, aspect, "A locks the aspect");
    frame(
        &mut e,
        vec![press(Key::X, Modifiers::NONE), release(Key::X)],
    );
    assert_ne!(
        e.document.edit.recipe().crop,
        crop,
        "X swaps the orientation"
    );
    let (texture, saturation) = {
        let r = e.document.edit.recipe();
        (r.effects.texture, r.saturation)
    };
    frame(&mut e, vec![press(Key::X, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::X)]);
    assert_eq!(e.document.edit.recipe().effects.texture, texture);
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::E)]);
    assert!(close(e.document.edit.recipe().exposure, 0.1));
    assert_eq!(e.document.edit.recipe().saturation, saturation);
    assert!(e.view.is(Tool::Crop));
}

#[test]
fn another_mask_or_leaving_masking_mid_hold_is_a_step_of_its_own() {
    let mut e = develop();
    e.document.edit.change(None, |r| {
        r.masks.push(Default::default());
        r.masks.push(Default::default());
    });
    e.view.set_tool(Tool::Mask);
    e.view.masking.selected = Some(0);
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    e.view.masking.selected = Some(1);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    e.view.set_tool(Tool::None);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::E)]);
    let names: Vec<String> = steps(&e).into_iter().map(|s| s.0).collect();
    assert_eq!(
        names[names.len() - 3..],
        ["Mask 1 Exposure", "Mask 2 Exposure", "Exposure"]
    );
    let recipe = e.document.edit.recipe();
    assert!(close(recipe.masks[0].adjust.exposure, 0.1));
    assert!(close(recipe.masks[1].adjust.exposure, 0.1));
    assert!(close(recipe.exposure, 0.1));
}

#[test]
fn a_release_by_its_place_lets_go_though_it_types_another_letter() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    // Option changed what the place types: egui keeps the E it saw down.
    frame(
        &mut e,
        vec![egui::Event::Key {
            key: Key::Q,
            physical_key: Some(Key::E),
            pressed: false,
            repeat: false,
            modifiers: Modifiers::ALT,
        }],
    );
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    assert!(close(e.document.edit.recipe().exposure, 0.1));
    assert_eq!(e.view.slider_keys.moved(), None, "nothing lit");
    assert_eq!(steps(&e).len(), 1);
    // Pressed again, which egui calls a repeat: it takes the letter up again.
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::E)]);
    assert!(close(e.document.edit.recipe().exposure, 0.2));
}

#[test]
fn a_frames_keys_count_in_the_order_they_came() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(
        &mut e,
        vec![
            press(Key::E, Modifiers::NONE),
            press(Key::ArrowUp, Modifiers::NONE),
            press(Key::C, Modifiers::NONE),
            press(Key::ArrowUp, Modifiers::NONE),
        ],
    );
    frame(&mut e, vec![]);
    frame(&mut e, vec![release(Key::E), release(Key::C)]);
    frame(&mut e, vec![]);
    let recipe = e.document.edit.recipe();
    assert!(close(recipe.exposure, 0.1));
    assert!(close(recipe.contrast, 0.05));
    assert_eq!(
        steps(&e),
        [
            ("Exposure".to_string(), "+0.10".to_string()),
            ("Contrast".to_string(), "+5".to_string())
        ]
    );
    // An arrow before the letter's release in the same frame is still its step.
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(
        &mut e,
        vec![press(Key::ArrowUp, Modifiers::NONE), release(Key::E)],
    );
    assert!(close(e.document.edit.recipe().exposure, 0.2));
}

#[test]
fn a_pointer_press_ends_the_step_under_way() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    let at = Pos2::new(5., 5.);
    frame(
        &mut e,
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    assert_eq!(steps(&e).len(), 1, "a drag starting now is not part of it");
    frame(
        &mut e,
        vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::E)]);
    assert_eq!(steps(&e).len(), 2);
}

#[test]
fn a_commands_repeat_without_its_modifier_takes_up_no_letter() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::W, Modifiers::SHIFT)]);
    frame(&mut e, vec![key(Key::W, true, true, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::W)]);
    assert!(close(e.document.edit.recipe().whites, 0.));
}

#[test]
fn a_wheel_notch_steps_by_its_own_modifiers() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(
        &mut e,
        vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0., 1.),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::SHIFT,
        }],
    );
    frame(&mut e, vec![release(Key::E)]);
    assert!(close(e.document.edit.recipe().exposure, 0.33));
}

#[test]
fn moving_a_masks_slider_hides_its_overlay() {
    let mut e = develop();
    e.document
        .edit
        .change(None, |r| r.masks.push(Default::default()));
    e.view.set_tool(Tool::Mask);
    e.view.masking.selected = Some(0);
    e.view.masking.overlay = true;
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::S, Modifiers::NONE)]);
    assert!(e.view.masking.overlay, "the letter alone");
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::S)]);
    assert!(!e.view.masking.overlay);
}

#[test]
fn a_step_for_a_mask_left_in_its_frame_moves_nothing() {
    let mut e = develop();
    e.document
        .edit
        .change(None, |r| r.masks.push(Default::default()));
    e.view.set_tool(Tool::Mask);
    e.view.masking.selected = Some(0);
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    // Up, then Shift+W closing Masking, in one frame.
    frame(
        &mut e,
        vec![
            press(Key::ArrowUp, Modifiers::NONE),
            press(Key::W, Modifiers::SHIFT),
        ],
    );
    frame(&mut e, vec![]);
    frame(&mut e, vec![release(Key::E)]);
    assert!(!e.view.is(Tool::Mask));
    let recipe = e.document.edit.recipe();
    assert!(close(recipe.exposure, 0.), "not the photo's");
    assert!(close(recipe.masks[0].adjust.exposure, 0.));
}

#[test]
fn an_undo_in_the_frame_of_a_step_keeps_history_and_settings_in_step() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::E)]);
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(
        &mut e,
        vec![
            press(Key::ArrowUp, Modifiers::NONE),
            press(Key::Z, Modifiers::COMMAND),
        ],
    );
    frame(&mut e, vec![]);
    frame(&mut e, vec![release(Key::E)]);
    // Undone to 0, then a step of its own.
    assert!(close(e.document.edit.recipe().exposure, 0.1));
    assert_eq!(steps(&e), [("Exposure".to_string(), "+0.10".to_string())]);
    e.undo();
    assert!(close(e.document.edit.recipe().exposure, 0.));
}

#[test]
fn a_tools_key_while_a_letter_is_held_is_a_step_of_its_own() {
    let mut e = develop();
    e.document.edit.setup_mut().crop = [0.1, 0.2, 0.5, 0.6];
    e.view.toggle(Tool::Crop);
    e.view.slider_keys.on = true;
    frame(&mut e, vec![]);
    let base = steps(&e).len();
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(
        &mut e,
        vec![press(Key::X, Modifiers::NONE), release(Key::X)],
    );
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::E)]);
    let names: Vec<String> = steps(&e).into_iter().map(|s| s.0).skip(base).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(names[0], "Exposure");
    assert_ne!(names[1], "Exposure");
    assert_eq!(names[2], "Exposure");
}

#[test]
fn the_wheel_is_taken_only_while_a_letter_is_held_and_not_with_cmd() {
    let wheels = |e: &Editor| {
        e.context.input(|i| {
            i.events
                .iter()
                .filter(|e| matches!(e, egui::Event::MouseWheel { .. }))
                .count()
        })
    };
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    // egui never sees a notch taken: none of it scrolls in the frames after,
    // the letter let go.
    frame(&mut e, vec![wheel(1.), release(Key::E)]);
    assert_eq!(wheels(&e), 0);
    for _ in 0..3 {
        frame(&mut e, Vec::new());
        assert_eq!(e.context.input(|i| i.smooth_scroll_delta), Vec2::ZERO);
    }
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![wheel(1.), release(Key::E), wheel(1.)]);
    assert_eq!(wheels(&e), 1, "the notch after the release is the view's");
    assert!(close(e.document.edit.recipe().exposure, 0.2));
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(
        &mut e,
        vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0., 1.),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::COMMAND,
        }],
    );
    assert_eq!(wheels(&e), 1, "Cmd with the wheel is the view's");
    frame(&mut e, vec![release(Key::E)]);
    assert!(close(e.document.edit.recipe().exposure, 0.2));
}

#[test]
fn each_hold_of_a_letter_is_its_own_step_though_in_one_frame() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(
        &mut e,
        vec![
            press(Key::ArrowUp, Modifiers::NONE),
            release(Key::E),
            press(Key::E, Modifiers::NONE),
            press(Key::ArrowUp, Modifiers::NONE),
        ],
    );
    frame(&mut e, vec![]);
    frame(&mut e, vec![release(Key::E)]);
    assert_eq!(
        steps(&e),
        [
            ("Exposure".to_string(), "+0.10".to_string()),
            ("Exposure".to_string(), "+0.20".to_string())
        ]
    );
}

#[test]
fn a_masks_steps_stop_at_its_ends_one_by_one() {
    let mut e = develop();
    e.document.edit.change(None, |r| {
        r.masks.push(Default::default());
        r.masks[0].adjust.exposure = 4.;
    });
    e.view.set_tool(Tool::Mask);
    e.view.masking.selected = Some(0);
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(
        &mut e,
        vec![
            press(Key::ArrowUp, Modifiers::NONE),
            press(Key::ArrowDown, Modifiers::NONE),
        ],
    );
    frame(&mut e, vec![release(Key::E)]);
    assert!(close(
        e.document.edit.recipe().masks[0].adjust.exposure,
        3.9
    ));
}

/// `e` with its key map in `dir`, where `set` changes it as Preferences does.
fn with_keymap(e: &mut Editor, dir: &tempfile::TempDir) {
    e.keymap = crate::app::shell::keymap::file::KeymapFile::load(dir.path().join("keymap.toml"));
}

fn placed(k: Key, place: Key, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key: k,
        physical_key: Some(place),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

/// Holds the key at `place`, presses Up and lets go.
fn hold_up(e: &mut Editor, place: Key) {
    frame(e, vec![placed(place, place, true)]);
    frame(e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(e, vec![placed(place, place, false)]);
}

#[test]
fn a_slider_rebound_in_the_key_map_moves_by_its_new_key_alone() {
    use crate::app::shell::keymap::{Chord, Mods};
    let dir = tempfile::tempdir().expect("a folder");
    let mut e = develop();
    with_keymap(&mut e, &dir);
    e.keymap
        .set("slider_keys:exposure", &[Chord::new(Key::K, Mods::NONE)])
        .expect("set");
    e.view.slider_keys.on = true;
    hold_up(&mut e, Key::E);
    assert!(
        close(e.document.edit.recipe().exposure, 0.),
        "E is no longer Exposure's"
    );
    hold_up(&mut e, Key::K);
    assert!(close(e.document.edit.recipe().exposure, 0.1), "K is");
}

#[test]
fn a_slider_unbound_in_the_key_map_has_no_key() {
    let dir = tempfile::tempdir().expect("a folder");
    let mut e = develop();
    with_keymap(&mut e, &dir);
    e.keymap.set("slider_keys:dehaze", &[]).expect("set");
    e.view.slider_keys.on = true;
    hold_up(&mut e, Key::D);
    assert!(close(e.document.edit.recipe().effects.dehaze, 0.));
    // The others keep theirs.
    hold_up(&mut e, Key::H);
    assert!(e.document.edit.recipe().highlights > 0.);
}

#[test]
fn the_default_keys_are_the_letters_by_their_place() {
    use crate::app::shell::keymap::{KeyMatch, Mods};
    let defaults: Vec<(ParameterId, Key)> = KEYS
        .iter()
        .map(|(b, id)| {
            let [d] = b.defaults else {
                panic!("one chord for {id:?}")
            };
            assert_eq!(
                (d.mods, d.matching),
                (Mods::NONE, KeyMatch::Place),
                "{id:?}"
            );
            (*id, d.keys[0])
        })
        .collect();
    use ParameterId as P;
    assert_eq!(
        defaults,
        [
            (P::Exposure, Key::E),
            (P::Contrast, Key::C),
            (P::Highlights, Key::H),
            (P::Shadows, Key::S),
            (P::Whites, Key::W),
            (P::Blacks, Key::B),
            (P::Temperature, Key::T),
            (P::Tint, Key::G),
            (P::Texture, Key::X),
            (P::Clarity, Key::L),
            (P::Dehaze, Key::D),
            (P::Vibrance, Key::V),
            (P::Saturation, Key::A),
        ]
    );
}

#[test]
fn a_key_bound_with_cmd_steps_with_cmd_held() {
    use crate::app::shell::keymap::{Chord, Mods};
    let dir = tempfile::tempdir().expect("a folder");
    let mut e = develop();
    with_keymap(&mut e, &dir);
    e.keymap
        .set("slider_keys:exposure", &[Chord::new(Key::F9, Mods::CMD)])
        .expect("set");
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::F9, Modifiers::COMMAND)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::COMMAND)]);
    frame(
        &mut e,
        vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0., 1.),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::COMMAND,
        }],
    );
    frame(&mut e, vec![release(Key::F9)]);
    assert!(close(e.document.edit.recipe().exposure, 0.2));
}

#[test]
fn an_arrow_bound_to_a_slider_steps_the_held_one() {
    use crate::app::shell::keymap::{Chord, Mods};
    let dir = tempfile::tempdir().expect("a folder");
    let mut e = develop();
    with_keymap(&mut e, &dir);
    e.keymap
        .set(
            "slider_keys:contrast",
            &[Chord::new(Key::ArrowRight, Mods::NONE)],
        )
        .expect("set");
    e.view.slider_keys.on = true;
    hold_up(&mut e, Key::E);
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowRight, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::E)]);
    let recipe = e.document.edit.recipe();
    assert!(close(recipe.exposure, 0.2));
    assert!(close(recipe.contrast, 0.));
}

#[test]
fn a_held_keys_repeat_that_is_a_tools_key_is_the_tools() {
    use crate::app::shell::keymap::{Chord, Mods};
    let dir = tempfile::tempdir().expect("a folder");
    let mut e = develop();
    with_keymap(&mut e, &dir);
    e.keymap
        .set("crop:swap_orientation", &[Chord::new(Key::E, Mods::SHIFT)])
        .expect("set");
    e.view.slider_keys.on = true;
    e.document.edit.setup_mut().crop = [0.1, 0.2, 0.5, 0.6];
    e.view.toggle(Tool::Crop);
    frame(&mut e, vec![]);
    let crop = e.document.edit.recipe().crop;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![key(Key::E, true, true, Modifiers::SHIFT)]);
    frame(&mut e, vec![release(Key::E)]);
    assert_ne!(
        e.document.edit.recipe().crop,
        crop,
        "Shift+E swapped the crop"
    );
}

#[test]
fn slider_keys_hold_no_keys_while_they_cannot_act() {
    use crate::app::shell::keymap::Context;
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![]);
    let ctx = e.context.clone();
    assert!(e.live_keys(&ctx).has(Context::SliderKeys));
    e.onboarding.visible = true;
    assert!(!e.live_keys(&ctx).has(Context::SliderKeys));
}

#[test]
fn an_arrow_bound_to_a_slider_held_alone_moves_nothing() {
    use crate::app::shell::keymap::{Chord, Mods};
    let dir = tempfile::tempdir().expect("a folder");
    let mut e = develop();
    with_keymap(&mut e, &dir);
    e.keymap
        .set(
            "slider_keys:exposure",
            &[Chord::new(Key::ArrowRight, Mods::NONE)],
        )
        .expect("set");
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::ArrowRight, Modifiers::NONE)]);
    frame(
        &mut e,
        vec![key(Key::ArrowRight, true, true, Modifiers::NONE)],
    );
    assert!(
        close(e.document.edit.recipe().exposure, 0.),
        "its own repeat"
    );
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::ArrowRight)]);
    assert!(close(e.document.edit.recipe().exposure, 0.1));
}

#[test]
fn a_tools_arrow_stays_the_tools_while_a_key_is_held() {
    use crate::app::shell::keymap::{Chord, Mods};
    let dir = tempfile::tempdir().expect("a folder");
    let mut e = develop();
    with_keymap(&mut e, &dir);
    e.keymap
        .set(
            "crop:swap_orientation",
            &[Chord::new(Key::ArrowRight, Mods::NONE)],
        )
        .expect("set");
    e.view.slider_keys.on = true;
    e.document.edit.setup_mut().crop = [0.1, 0.2, 0.5, 0.6];
    e.view.toggle(Tool::Crop);
    frame(&mut e, vec![]);
    let crop = e.document.edit.recipe().crop;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowRight, Modifiers::NONE)]);
    frame(&mut e, vec![release(Key::E)]);
    assert!(close(e.document.edit.recipe().exposure, 0.));
    assert_ne!(
        e.document.edit.recipe().crop,
        crop,
        "Right swapped the crop"
    );
}

#[test]
fn a_held_letters_repeat_with_cmd_runs_no_command_and_ends_the_step() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    frame(&mut e, vec![press(Key::ArrowUp, Modifiers::NONE)]);
    assert!(e.view.slider_keys.held());
    // egui never saw E's press: it would take this repeat as Cmd+E's first.
    frame(&mut e, vec![press(Key::E, Modifiers::COMMAND)]);
    let left = e.context.input(|i| {
        i.events
            .iter()
            .any(|ev| matches!(ev, egui::Event::Key { key: Key::E, .. }))
    });
    assert!(!left, "the repeat is the mode's");
    assert!(!e.view.slider_keys.held(), "the step under way ended");
    frame(&mut e, vec![release(Key::E)]);
    assert!(close(e.document.edit.recipe().exposure, 0.1));
}

#[test]
fn two_sliders_on_one_key_move_neither() {
    use crate::app::shell::keymap::{Chord, Mods};
    let dir = tempfile::tempdir().expect("a folder");
    let mut e = develop();
    with_keymap(&mut e, &dir);
    e.keymap
        .set("slider_keys:exposure", &[Chord::new(Key::C, Mods::NONE)])
        .expect("set");
    e.view.slider_keys.on = true;
    hold_up(&mut e, Key::C);
    let recipe = e.document.edit.recipe();
    assert!(close(recipe.exposure, 0.) && close(recipe.contrast, 0.));
}

#[test]
fn a_window_opened_before_the_steps_apply_leaves_the_photo_alone() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    frame(&mut e, vec![press(Key::E, Modifiers::NONE)]);
    // The hook reads Up, then the frame opens Preferences (a native menu's
    // action) before the steps apply.
    let ctx = e.context.clone();
    let mut input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(2400., 900.))),
        events: vec![press(Key::ArrowUp, Modifiers::NONE)],
        ..Default::default()
    };
    e.slider_key_input(&ctx, &mut input);
    e.open_preferences(crate::app::preferences::Tab::General);
    let mut output = ctx.run_ui(input, |ui| e.draw(ui));
    output.textures_delta.clear();
    assert!(close(e.document.edit.recipe().exposure, 0.));
}

/// Develop with a photo and Slider Keys on, as the app draws it.
fn shot_window(e: &mut Editor, ctx: &egui::Context) {
    let mut ui = egui::Ui::new(
        ctx.clone(),
        egui::Id::new("slider-keys-shot"),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.content_rect()),
    );
    e.draw(&mut ui);
}

fn shot_setup(e: &mut Editor, shown: Option<(&str, &str)>) {
    e.module = crate::app::Module::Develop;
    e.onboarding.visible = false;
    e.document.metadata = Some(crate::app::testing::blue_image().metadata.clone());
    e.document.set_image(crate::app::testing::blue_image());
    e.view.slider_keys.on = true;
    // E held and moving Exposure: the label stays as set.
    if shown.is_some() {
        let keys = &mut e.view.slider_keys;
        keys.held = Some(Held {
            id: ParameterId::Exposure,
            place: Key::E,
            chord: false,
        });
        keys.moving = Some(Target {
            id: ParameterId::Exposure,
            photo: None,
            mask: None,
            hold: 0,
        });
    }
    e.view.slider_keys.shown = shown.map(|(l, v)| (l.to_string(), v.to_string()));
}

/// The held slider's label over the photo, for a Basic slider and for a mask
/// with a long name, for `RAWMAKASE_UI_SHOTS` ([`crate::app::ui::shots`]).
#[test]
fn shots_of_the_slider_keys_label() {
    for (name, label, value) in [
        ("slider-keys-label", "Exposure", "+0.33"),
        (
            "slider-keys-label-long-mask",
            "Evening sky over the old harbour, warmer Highlights",
            "-25",
        ),
    ] {
        crate::app::ui::shots::shot(
            name,
            Vec2::new(1100., 700.),
            |e, _| shot_setup(e, Some((label, value))),
            shot_window,
        );
    }
}

/// The photo's menu with Slider Keys on, for `RAWMAKASE_UI_SHOTS`. The menu
/// opens on a click, which a shot can't make, so it is drawn here: the click
/// goes where the toolbar drew "Photo".
#[test]
fn shots_of_the_photo_menu_with_slider_keys_on() {
    let Some(dir) = std::env::var_os("RAWMAKASE_UI_SHOTS").map(std::path::PathBuf::from) else {
        return;
    };
    let mut harness = egui_kittest::Harness::builder()
        .with_size(Vec2::new(1700., 900.))
        .with_pixels_per_point(2.)
        .wgpu()
        .build_ui_state(
            |ui, editor: &mut Option<Editor>| {
                let editor = editor.get_or_insert_with(|| {
                    crate::app::install_fonts(ui.ctx(), &fastframe_text::detect());
                    crate::app::ui::icons::install(ui.ctx());
                    let mut e = Editor::with_context(ui.ctx(), None, Default::default(), None);
                    shot_setup(&mut e, None);
                    // Room in the toolbar for the menu's "Photo", as with F7.
                    e.change_panels(crate::app::shell::panels::PanelChange::Toggle(
                        crate::app::shell::panels::WorkspacePanel::Left,
                    ));
                    e
                });
                shot_window(editor, ui.ctx());
            },
            None,
        );
    harness.run_steps(6);
    let photo = harness
        .output()
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(t) if t.galley.text() == "Photo" => {
                Some(t.visual_bounding_rect().center())
            }
            _ => None,
        })
        .expect("the photo's menu button");
    harness.hover_at(photo);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: photo,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.run_steps(1);
    }
    harness.run_steps(4);
    let image = harness.render().expect("wgpu could not draw the shot");
    std::fs::create_dir_all(&dir).expect("the shots folder");
    image
        .save(dir.join("slider-keys-photo-menu.png"))
        .expect("the shot");
}

#[test]
fn a_held_key_shows_its_slider_before_it_moves() {
    let mut e = develop();
    e.view.slider_keys.on = true;
    let label = |texts: &Texts| texts.iter().any(|(t, ..)| t.starts_with("Whites  "));
    frame(&mut e, vec![press(Key::W, Modifiers::NONE)]);
    // egui sizes a new area in its first frame and draws it from the next.
    let texts = frame(&mut e, vec![]);
    assert!(label(&texts), "shown from the press");
    assert!(!e.view.slider_keys.held(), "no step under way");
    let texts = frame(&mut e, vec![release(Key::W)]);
    assert!(!label(&texts), "gone with the key");
    // In Masking, the mask's slider, by the mask's name.
    e.document.edit.change(None, |r| {
        r.masks.push(Default::default());
        r.masks[0].name = "Sky".into();
    });
    e.view.set_tool(Tool::Mask);
    e.view.masking.selected = Some(0);
    frame(&mut e, vec![press(Key::C, Modifiers::NONE)]);
    let texts = frame(&mut e, vec![]);
    assert!(drawn(&texts, "Sky Contrast  0"), "{texts:?}");
    frame(&mut e, vec![release(Key::C)]);
}

#[test]
fn the_status_bar_says_the_mode_is_on_and_nothing_once_off() {
    let mut e = develop();
    let said = |texts: &Texts, text: &str| texts.iter().any(|(t, ..)| t.contains(text));
    assert!(
        !said(&frame(&mut e, vec![]), "Slider Keys"),
        "nothing while off"
    );
    frame(&mut e, vec![press(Key::K, Modifiers::SHIFT)]);
    let texts = frame(&mut e, vec![release(Key::K)]);
    assert!(e.view.slider_keys.on);
    assert!(
        texts.iter().any(|(t, ..)| t == "Slider Keys On"),
        "{texts:?}"
    );
    frame(&mut e, vec![press(Key::K, Modifiers::SHIFT)]);
    let texts = frame(&mut e, vec![release(Key::K)]);
    assert!(!e.view.slider_keys.on);
    assert!(!said(&texts, "Slider Keys"), "{texts:?}");
}
