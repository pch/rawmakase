//! Editor tests of `retouch_tool`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use eframe::egui::{Pos2, Rect};
use std::sync::Arc;

#[test]
fn remove_tool_adds_spots_paints_brushes_and_edits_the_selection() {
    use crate::model::retouch::RetouchShape;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: (0..160000)
            .map(|i| {
                let (x, y) = ((i % 400) as f32, (i / 400) as f32);
                [0.2 + 0.05 * (x * 0.1).sin() * (y * 0.13).cos(); 3]
            })
            .collect(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor
        .view
        .set_tool(crate::app::develop::state::Tool::Remove);
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                if editor.view.is(crate::app::develop::state::Tool::Remove) {
                    editor.retouch_keys(ui.ctx())
                }
                editor.viewport_ui(ui);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
        (
            editor.document.edit.recipe().retouch.clone(),
            editor.view.zoom.on,
            editor.view.retouch.selected,
        )
    };
    let button = |pos, pressed, modifiers| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    let none = egui::Modifiers::NONE;
    frame(vec![]);
    // Click: a spot with an automatic source, and no zoom.
    let p = Pos2::new(60., 60.);
    frame(vec![egui::Event::PointerMoved(p), button(p, true, none)]);
    let (ops, zoomed, _) = frame(vec![button(p, false, none)]);
    assert_eq!(ops.len(), 1);
    assert!(!zoomed);
    assert!(ops[0].offset != [0., 0.] && ops[0].offset.iter().all(|v| v.is_finite()));
    assert!(matches!(ops[0].shape, RetouchShape::Spot { center, .. }
        if (center[0] - 0.3).abs() < 0.02 && (center[1] - 0.3).abs() < 0.02));
    // A drag from empty space paints a brushed area.
    let (a, b) = (Pos2::new(150., 40.), Pos2::new(150., 150.));
    frame(vec![egui::Event::PointerMoved(a), button(a, true, none)]);
    for k in 1..=10 {
        frame(vec![egui::Event::PointerMoved(
            a + (b - a) * k as f32 / 10.,
        )]);
    }
    let (ops, _, selected) = frame(vec![button(b, false, none)]);
    assert_eq!(ops.len(), 2);
    assert!(matches!(&ops[1].shape, RetouchShape::Brush { points, .. } if points.len() > 3));
    assert_eq!(selected, Some(1));
    // ] grows the selected area; Delete removes it.
    let radius = ops[1].radius();
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let (ops, ..) = frame(vec![key(egui::Key::CloseBracket)]);
    assert!(ops[1].radius() > radius);
    let (ops, ..) = frame(vec![key(egui::Key::Delete)]);
    assert_eq!(ops.len(), 1);
    // Dragging the remaining spot moves it and keeps its source in place.
    let before = ops[0].clone();
    let q = Pos2::new(80., 70.);
    frame(vec![egui::Event::PointerMoved(p), button(p, true, none)]);
    frame(vec![egui::Event::PointerMoved(q)]);
    let (ops, ..) = frame(vec![button(q, false, none)]);
    let source = |op: &crate::model::retouch::RetouchOp| {
        [op.pin()[0] + op.offset[0], op.pin()[1] + op.offset[1]]
    };
    assert!((ops[0].pin()[0] - before.pin()[0] - 0.1).abs() < 0.01);
    assert!((source(&ops[0])[0] - source(&before)[0]).abs() < 1e-5);
}

#[test]
fn a_quick_cmd_a_in_remove_does_not_toggle_visualize_spots() {
    let ctx = egui::Context::default();
    let (mut editor, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    let mut press = |modifiers: egui::Modifiers| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: Some(egui::Key::A),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
                // Cmd released within the same frame: Select All's press.
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| editor.retouch_keys(ui.ctx()));
        output.textures_delta.clear();
        editor.view.retouch.visualize
    };
    assert!(!press(egui::Modifiers::COMMAND));
    assert!(press(egui::Modifiers::NONE));
}

/// The upload question's card is placed where it can be seen: beside the
/// area when that is in view, else at the edge of the visible part of the
/// photo, and never larger than it.
#[test]
fn the_upload_question_stays_in_view() {
    use super::card_rect;
    let visible = Rect::from_min_max(Pos2::ZERO, Pos2::new(800., 600.));
    let size = egui::Vec2::new(250., 66.);
    let beside = card_rect(Pos2::new(100., 100.), size, visible);
    assert_eq!(beside.min, Pos2::new(114., 90.));
    // Zoomed in: the area is near the right edge, or out of view altogether.
    for pin in [
        Pos2::new(790., 300.),
        Pos2::new(1500., -400.),
        Pos2::new(-900., 900.),
    ] {
        let card = card_rect(pin, size, visible);
        assert!(visible.contains_rect(card), "{pin:?}: {card:?}");
        assert_eq!(card.size(), size);
    }
    // A viewport smaller than the card: the card fits it.
    let small = Rect::from_min_max(Pos2::new(10., 10.), Pos2::new(200., 50.));
    let card = card_rect(Pos2::new(100., 30.), size, small);
    assert!(small.contains_rect(card), "{card:?}");
}

/// However narrow the view, the question's card is tall enough for its
/// wrapped text: Send and Cancel are inside the visible part of the photo.
#[test]
fn the_upload_question_s_buttons_stay_in_view() {
    let ctx = egui::Context::default();
    let question = "Send this area and its surroundings to the service set up in AI Remove? \
                    Asked once.";
    for (width, pin) in [
        (800., Pos2::new(100., 100.)),
        (160., Pos2::new(80., 2000.)),
        (100., Pos2::new(-500., 900.)),
    ] {
        let visible = Rect::from_min_size(Pos2::ZERO, egui::Vec2::new(width, 600.));
        let mut buttons = Rect::NOTHING;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::Vec2::new(800., 600.))),
                ..Default::default()
            },
            |ui| {
                buttons = super::consent_card(ui, pin, visible, question).1;
            },
        );
        output.textures_delta.clear();
        assert!(
            visible.contains_rect(buttons),
            "{width}: buttons {buttons:?} outside {visible:?}"
        );
    }
}

/// Painting on the photo takes the keyboard from a text field, as a click
/// does, so Cmd/Ctrl+Z after a stroke undoes in History, not in the field.
#[test]
fn painting_takes_the_keyboard_from_a_text_field() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: (0..160000)
            .map(|i| {
                let (x, y) = ((i % 400) as f32, (i / 400) as f32);
                [0.2 + 0.05 * (x * 0.1).sin() * (y * 0.13).cos(); 3]
            })
            .collect(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor
        .view
        .set_tool(crate::app::develop::state::Tool::Remove);
    let mut text = String::new();
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                if editor.view.is(crate::app::develop::state::Tool::Remove) {
                    editor.retouch_keys(ui.ctx())
                }
                editor.viewport_ui(ui);
                // A text field in a corner, as the AI Retouch drawer's.
                egui::Area::new(egui::Id::new("field area"))
                    .fixed_pos(Pos2::new(0., 185.))
                    .show(ui.ctx(), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut text)
                                .id(egui::Id::new("a text field"))
                                .desired_width(40.),
                        );
                    });
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
        (
            editor.document.edit.recipe().retouch.clone(),
            editor.view.zoom.on,
            editor.view.retouch.selected,
        )
    };
    let button = |pos, pressed, modifiers| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    let none = egui::Modifiers::NONE;
    frame(vec![]);
    let field = egui::Id::new("a text field");
    ctx.memory_mut(|m| m.request_focus(field));
    frame(vec![]);
    assert_eq!(ctx.memory(|m| m.focused()), Some(field));
    let (a, b) = (Pos2::new(150., 40.), Pos2::new(150., 150.));
    frame(vec![egui::Event::PointerMoved(a), button(a, true, none)]);
    for k in 1..=10 {
        frame(vec![egui::Event::PointerMoved(
            a + (b - a) * k as f32 / 10.,
        )]);
    }
    let (ops, ..) = frame(vec![button(b, false, none)]);
    assert_eq!(ops.len(), 1, "the stroke was painted");
    assert_eq!(ctx.memory(|m| m.focused()), None);
}

/// The draft's tint keeps its proportions when its texture is capped, so a
/// round brush stays round.
#[test]
fn the_draft_tint_keeps_its_proportions() {
    use super::tint_size;
    assert_eq!(tint_size(1000., 100.), (512, 52));
    assert_eq!(tint_size(100., 1000.), (52, 512));
    assert_eq!(tint_size(300.4, 20.), (301, 20));
    assert_eq!(tint_size(0.2, 0.2), (1, 1));
}
