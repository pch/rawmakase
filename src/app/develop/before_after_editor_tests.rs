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

/// Before beside an HDR edit asks the GPU for its own master at the headroom due when
/// its settings have HDR on, and for none when they have it off.
#[test]
fn before_asks_for_its_own_hdr_master() {
    use crate::app::develop::before_after::{Axis, Compare};
    use crate::app::shell::display::HdrDisplay;
    let ctx = egui::Context::default();
    let (mut e, _) = crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), true);
    e.preview.last_fit_edge = 64;
    e.view.viewport = Vec2::new(16., 12.);
    e.view.hdr_display = HdrDisplay {
        headroom: 4.,
        potential: 16.,
        float_surface: true,
        ..HdrDisplay::default()
    };
    e.document.edit.setup_mut().hdr.enabled = true;
    let mut before = e.document.edit.recipe().clone();
    e.document.before = Some(before.clone());
    e.view.compare = Compare::SideBySide(Axis::LeftRight);
    e.schedule_before();
    let asked = e.preview.before.submitted.as_ref().unwrap().hdr;
    assert!(asked.is_some_and(|h| (h - 4.).abs() < 0.1), "{asked:?}");
    let id = e.preview.before.task.id();
    e.view.hdr_display.headroom = 8.;
    e.schedule_before();
    assert_ne!(
        e.preview.before.task.id(),
        id,
        "a new headroom renders again"
    );
    before.hdr.enabled = false;
    e.document.before = Some(before);
    e.schedule_before();
    assert_eq!(e.preview.before.submitted.as_ref().unwrap().hdr, None);
}
