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

/// If removing the AI Denoise model fails while the files remain installed (for
/// example when unloading times out), Develop restores the model path, clears the
/// temporary install warning, and automatically resumes AI Denoise for the open photo.
#[test]
fn failed_model_removal_resumes_ai_denoise_in_develop_when_model_stays_installed()
-> anyhow::Result<()> {
    use crate::ai_denoise::State;
    use crate::ai_denoise::tests::{Behaviour, fixture};
    use rawmakase_inference::Provider;
    use std::time::{Duration, Instant};

    let f = fixture(Provider::Cpu, Behaviour::default());
    let model_dir = f.dir.path().join("model");
    let photo = f.photo("a.raw");
    let plain = f.plain();

    let ctx = egui::Context::default();
    let (mut e, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    e.ai_denoise_model.set_test_dir(model_dir.clone());
    task::wait_for(vec![e.renderer.stop()], Duration::from_secs(5));
    let (taken_tx, _taken) = std::sync::mpsc::channel();
    let (_release, release) = std::sync::mpsc::channel();
    e.renderer = worker::holding(taken_tx, release);
    e.ai_denoise = f.service.clone();
    e.document.set_image(plain);
    e.document.opened_at(photo);
    e.document.image_file = e.document.file.clone();
    e.view.viewport = Vec2::new(40., 40.);
    e.module = Module::Develop;
    e.document.edit.setup_mut().switch_ai_denoise(true);

    // Simulate clicking "Remove model" whose unload times out and returns false.
    let (unload_started_tx, unload_started_rx) = std::sync::mpsc::channel();
    let (release_unload_tx, release_unload_rx) = std::sync::mpsc::channel();
    let service = e.ai_denoise.clone();
    e.ai_denoise_model.remove(
        move || {
            service.set_model(None);
            let _ = unload_started_tx.send(());
            let _ = release_unload_rx.recv();
            false
        },
        e.tx.clone(),
        ctx.clone(),
    );
    e.document.ai = Default::default();
    e.schedule();

    unload_started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("model removal started");
    assert!(e.ai_denoise_model.busy());
    assert!(
        matches!(
            e.document.ai.status(),
            Some(State::Failed(why)) if why.contains("Install the AI Denoise model")
        ),
        "pending removal should show the install warning: {:?}",
        e.document.ai.status()
    );

    assert_eq!(
        f.behaviour.runs.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no inference should have run before removal failed"
    );

    // Let the removal fail and deliver Event::ModelRemoved(Err(...)).
    let _ = release_unload_tx.send(());
    drop(release_unload_tx);

    let deadline = Instant::now() + Duration::from_secs(5);
    while e.ai_denoise_model.busy() {
        e.events(&ctx);
        assert!(
            Instant::now() < deadline,
            "ModelRemoved event never arrived"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(
        e.messages.line().contains("Model not removed"),
        "expected removal failure message, got {:?}",
        e.messages.line()
    );
    assert!(model_dir.is_dir(), "model files should remain on disk");
    assert!(
        !matches!(e.document.ai.status(), Some(State::Failed(_))),
        "stale install warning should be cleared after failed removal: {:?}",
        e.document.ai.status()
    );

    // The rescheduled Develop request should run with the restored model path and complete.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !e.document.ai.is_complete() {
        e.events(&ctx);
        assert!(
            Instant::now() < deadline,
            "resumed AI Denoise request never completed; status={:?}",
            e.document.ai.status()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(e.document.ai.source().is_some());
    assert!(
        f.behaviour.runs.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "resumed request should have run the model using the restored model path"
    );

    if let Some(handle) = e.ai_denoise.stop() {
        handle.join().unwrap();
    }
    Ok(())
}
