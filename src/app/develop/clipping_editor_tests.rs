//! Editor tests of `clipping`: the whole editor driven through its frames.
use crate::app::*;

#[test]
fn j_toggles_both_clipping_warnings_but_not_while_typing() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let mut text = String::new();
    let j = || egui::Event::Key {
        key: egui::Key::J,
        physical_key: Some(egui::Key::J),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let mut frame = |events: Vec<egui::Event>, typing: bool, e: &mut Editor| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                if typing {
                    ui.text_edit_singleline(&mut text).request_focus();
                }
                e.develop_shortcuts(&ctx);
            },
        );
        output.textures_delta.clear();
    };
    frame(vec![], false, &mut e);
    frame(vec![j()], false, &mut e);
    let both = crate::rendered::ClipOverlay {
        shadows: true,
        highlights: true,
    };
    assert_eq!(e.view.clipping.overlay(), both);
    // A J typed into a field stays there.
    frame(vec![], true, &mut e);
    frame(vec![j()], true, &mut e);
    assert_eq!(e.view.clipping.overlay(), both);
    frame(vec![], false, &mut e);
    frame(vec![j()], false, &mut e);
    assert_eq!(
        e.view.clipping.overlay(),
        crate::rendered::ClipOverlay::NONE
    );
}

#[test]
fn a_hovered_clipping_triangle_shows_its_warning_until_the_pointer_leaves() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let frame = e.begin_edit_frame();
    // As the histogram does while its highlight triangle is under the pointer.
    e.view
        .clipping
        .set_hover(Some(crate::app::develop::clipping::ClipSide::Highlights));
    e.finish_edit_frame(frame, &ctx);
    assert!(e.view.clipping.overlay().highlights);
    // The next frame draws no hovered triangle: the warning goes, and nothing
    // was turned on.
    let frame = e.begin_edit_frame();
    e.finish_edit_frame(frame, &ctx);
    assert_eq!(
        e.view.clipping.overlay(),
        crate::rendered::ClipOverlay::NONE
    );
}
