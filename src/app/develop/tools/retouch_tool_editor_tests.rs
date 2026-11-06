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
    editor.view.tool = crate::app::develop::state::Tool::Remove;
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                ctx.input(|i| {
                    if editor.view.is(crate::app::develop::state::Tool::Remove) {
                        editor.retouch_keys(i)
                    }
                });
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
        let mut output = ctx.run_ui(input, |ui| ui.input(|i| editor.retouch_keys(i)));
        output.textures_delta.clear();
        editor.view.retouch.visualize
    };
    assert!(!press(egui::Modifiers::COMMAND));
    assert!(press(egui::Modifiers::NONE));
}
