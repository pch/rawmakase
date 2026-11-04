//! Editor tests of `ai_denoise`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use crate::develop;
use std::sync::Arc;

/// The Library's Loupe shows AI Denoise only from a finished result, as the
/// Library's previews do: it never asks for one to be computed, and renders a
/// partial one plain.
#[test]
fn the_loupe_shows_only_a_finished_ai_denoise_result() -> anyhow::Result<()> {
    use crate::ai_denoise::{State, Update};
    let d = tempfile::tempdir()?;
    let raw = d.path().join("a.RAF");
    std::fs::write(&raw, b"fixture")?;
    let ctx = egui::Context::default();
    let (mut e, image) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    task::wait_for(vec![e.renderer.stop()], std::time::Duration::from_secs(5));
    let (taken_tx, _taken) = std::sync::mpsc::channel();
    let (_release, release) = std::sync::mpsc::channel();
    e.renderer = worker::holding(taken_tx, release);
    e.document.opened_at(raw);
    e.document.image_file = e.document.file.clone();
    e.view.viewport = Vec2::new(40., 40.);
    e.module = Module::Library;
    in_edit_frame(&ctx, &mut e, |e| {
        e.document.edit.recipe_mut().switch_ai_denoise(true)
    });
    e.schedule();
    assert!(e.document.ai.status().is_none(), "nothing asked for");
    let source = develop::AiSource::recover((*image).clone(), rawmakase_cancel::Cancel::NEVER)?;
    let rendered_ai = |e: &Editor| e.preview.request(e.preview.task.id()).map(|p| p.ai);
    e.ai_denoise_update(Update {
        plain: Arc::downgrade(&image),
        state: State::Partial(source.clone()),
    });
    e.schedule();
    assert_eq!(
        rendered_ai(&e),
        Some(crate::app::develop::state::RenderedAi::None),
        "a partial one"
    );
    e.ai_denoise_update(Update {
        plain: Arc::downgrade(&image),
        state: State::Ready(source),
    });
    e.schedule();
    assert_eq!(
        rendered_ai(&e),
        Some(crate::app::develop::state::RenderedAi::Complete)
    );
    Ok(())
}
