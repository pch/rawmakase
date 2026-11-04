//! Editor tests of `before_after`: the whole editor driven through its frames.
use crate::app::*;
use eframe::egui::{Pos2, Rect};

#[test]
fn compact_inspector_keeps_canvas_and_before_preserves_edits() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.document.edit.setup_mut().exposure = 1.25;
    editor.document.edit.setup_mut().crop = [0.1, 0.1, 0.9, 0.9];
    let saved = editor.document.edit.recipe().clone();
    for frame in 0..30 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                events: if frame == 28 {
                    vec![egui::Event::Key {
                        key: egui::Key::Backslash,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }]
                } else {
                    vec![]
                },
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
        assert!(
            editor.view.viewport.x > 400.,
            "Inspector consumed canvas on frame {frame}"
        );
    }
    assert_eq!(
        editor.view.compare,
        crate::app::develop::before_after::Compare::BeforeOnly
    );
    assert_eq!(*editor.document.edit.recipe(), saved);
    assert_eq!(editor.effective_recipe().crop, saved.crop);
    assert_eq!(editor.effective_recipe().exposure, 0.);
}
