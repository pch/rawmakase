//! Editor tests of `messages`: the whole editor driven through its frames.
use crate::app::*;

/// A failure said while the Library's own message was showing used to be
/// hidden by it, and either was gone with the next progress.
#[test]
fn a_failed_action_stays_on_screen_past_the_librarys_messages_and_progress() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.onboarding.visible = false;
    e.messages
        .fail("No camera look fitted: The preview does not line up");
    e.messages.note("Reading RAW…");
    for time in [0., 3600.] {
        let input = egui::RawInput {
            time: Some(time),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| e.draw(ui));
        output.textures_delta.clear();
    }
    assert_eq!(e.messages.line(), "Reading RAW…");
    assert!(
        e.messages
            .has_entry("No camera look fitted: The preview does not line up")
    );
}
