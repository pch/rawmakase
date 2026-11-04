use super::*;
use crate::ai_denoise::key;
use crate::ai_denoise::tests::*;
use rawmakase_cancel::Cancel;
use rawmakase_decode::ai::cache::AiEntry;
use rawmakase_decode::ai::denoise::PackedBayer;
use rawmakase_inference::Provider;
use std::sync::mpsc;
use std::time::Duration;

/// The Detail panel's switch and Amount are edits: one History step each, the edit
/// needing a save, undone like any other. A result arriving is not an edit, and a
/// result for an image no longer shown is dropped.
#[test]
fn the_switch_is_an_edit_and_a_result_is_not() {
    use crate::app::testing::{community_editor, labelled};
    use eframe::egui;
    // The Detail panel sits far down: a tall window keeps it on screen.
    fn controls_frame(
        ctx: &egui::Context,
        editor: &mut crate::app::Editor,
        events: Vec<egui::Event>,
    ) -> egui::accesskit::TreeUpdate {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320., 5000.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                egui::CentralPanel::default().show(ui, |ui| editor.controls(ui));
                editor.finish_edit_frame(frame, ui.ctx());
            },
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .expect("accessibility is on")
    }
    fn click(ctx: &egui::Context, editor: &mut crate::app::Editor, node: &egui::accesskit::Node) {
        let b = node.bounds().expect("placed");
        let at = egui::pos2(((b.x0 + b.x1) / 2.) as f32, ((b.y0 + b.y1) / 2.) as f32);
        for pressed in [true, false] {
            controls_frame(
                ctx,
                editor,
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }
    let ctx = egui::Context::default();
    let mut editor = community_editor(&ctx, "X-T20", false);
    let plain = fixture(Provider::Cpu, Behaviour::default()).plain();
    editor.document.set_image(plain.clone());
    let tree = controls_frame(&ctx, &mut editor, vec![]);
    let switch = labelled(&tree, "AI Denoise").expect("the AI Denoise switch");
    click(&ctx, &mut editor, switch);
    assert!(editor.document.edit.recipe().ai_denoise.enabled);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(
        (steps[applied - 1].name.as_str(), applied),
        ("AI Denoise", 1),
        "{steps:?}"
    );
    assert!(editor.document.edit.save_state().needs_save());
    // Without the model the panel says so, and nothing is asked for.
    let tree = controls_frame(&ctx, &mut editor, vec![]);
    assert!(
        tree.nodes.iter().any(|(_, n)| n
            .label()
            .is_some_and(|l| l.starts_with("Accept and download"))),
        "the install offer"
    );

    // A result for the image shown: kept, rendered, no History step.
    let source = AiSource::recover((*plain).clone(), Cancel::NEVER).unwrap();
    editor.ai_denoise_update(Update {
        plain: Arc::downgrade(&plain),
        state: State::Ready(source.clone()),
    });
    assert_eq!(
        editor
            .document
            .ai_source(&plain, &editor.document.edit.recipe().ai_denoise),
        Some(source.clone())
    );
    assert_eq!(editor.document.edit.history().steps().1, 1);
    // Off: renders without it, the result kept for switching back.
    assert!(editor.document.edit.undo());
    assert!(!editor.document.edit.recipe().ai_denoise.enabled);
    assert!(
        editor
            .document
            .ai_source(&plain, &editor.document.edit.recipe().ai_denoise)
            .is_none()
    );
    assert!(editor.document.ai.source().is_some());

    // A result for another image (the photo was replaced): dropped.
    let replaced = Arc::new((*plain).clone());
    editor.document.set_image(replaced);
    editor.ai_denoise_update(Update {
        plain: Arc::downgrade(&plain),
        state: State::Ready(source),
    });
    assert!(editor.document.ai.source().is_none());
}

/// A render of another photo (Reference View) never gets this photo's result.
#[test]
fn only_the_documents_own_image_gets_its_source() {
    let mut doc = crate::app::develop::state::Document::default();
    let plain = fixture(Provider::Cpu, Behaviour::default()).plain();
    doc.set_image(plain.clone());
    doc.ai.source = Some(AiSource::recover((*plain).clone(), Cancel::NEVER).unwrap());
    let on = crate::model::ai_denoise::AiDenoise {
        enabled: true,
        amount: 1.,
    };
    assert!(doc.ai_source(&plain, &on).is_some());
    let reference = Arc::new((*plain).clone());
    assert!(doc.ai_source(&reference, &on).is_none());
}

/// A partial result does not stop the photo asking again (switched off before the
/// final one arrived); a final result or a failure does.
#[test]
fn a_partial_result_asks_again_a_final_or_failed_one_does_not() {
    let plain = fixture(Provider::Cpu, Behaviour::default()).plain();
    let mut photo = Photo::default();
    photo.source = Some(AiSource::recover((*plain).clone(), Cancel::NEVER).unwrap());
    assert!(photo.needs_request(&plain));
    photo.complete = true;
    assert!(!photo.needs_request(&plain));
    photo.complete = false;
    photo.failed = true;
    assert!(!photo.needs_request(&plain));
}

/// A partial result is dropped when the final one fails: Develop renders plain.
#[test]
fn a_failure_after_a_partial_result_drops_it() {
    use crate::app::testing::community_editor;
    let ctx = eframe::egui::Context::default();
    let mut editor = community_editor(&ctx, "X-T20", false);
    let plain = fixture(Provider::Cpu, Behaviour::default()).plain();
    editor.document.set_image(plain.clone());
    editor.document.edit.setup_mut().ai_denoise.enabled = true;
    let partial = AiSource::recover((*plain).clone(), Cancel::NEVER).unwrap();
    editor.ai_denoise_update(Update {
        plain: Arc::downgrade(&plain),
        state: State::Partial(partial),
    });
    let on = editor.document.edit.recipe().ai_denoise;
    assert!(editor.document.ai_source(&plain, &on).is_some());
    editor.ai_denoise_update(Update {
        plain: Arc::downgrade(&plain),
        state: State::Failed("the run failed".into()),
    });
    assert!(editor.document.ai_source(&plain, &on).is_none());
    assert!(matches!(
        editor.document.ai.status(),
        Some(State::Failed(_))
    ));
}

/// The identity a request carries is the one read before its image was decoded (the
/// loader's), not the file as it is when the header arrives: a file replaced in
/// between is refused, never denoised into the first file's image.
#[test]
fn the_decoded_images_file_is_the_loaders_reading() {
    use crate::app::worker::Event;
    let ctx = eframe::egui::Context::default();
    let mut editor = crate::app::testing::community_editor(&ctx, "X-T20", false);
    let f = fixture(Provider::Cpu, Behaviour::default());
    let photo = f.photo("a.raw");
    let before = crate::storage::Identity::read(&photo).unwrap();
    let (generation, _) = editor.document.start_load();
    editor
        .tx
        .send(Event::Ready {
            id: generation,
            full: f.plain(),
            file: Some(before.clone()),
            status: "Developed".into(),
        })
        .unwrap();
    editor.events(&ctx);
    assert_eq!(editor.document.image_file, Some(before));
    // Replaced after decoding: the image's file is still the one decoded.
    std::fs::write(&photo, b"another photo entirely").unwrap();
    assert_ne!(
        editor.document.image_file,
        crate::storage::Identity::read(&photo).ok()
    );
    // A new image forgets it until the loader says.
    editor.document.set_image(f.plain());
    assert_eq!(editor.document.image_file, None);
}

/// Without the file's identity from before decoding (it could not be read), Develop asks
/// for nothing and says why, rather than trusting the file as it is now.
#[test]
fn an_image_without_its_files_identity_gets_no_ai_request() {
    let ctx = eframe::egui::Context::default();
    let mut editor = crate::app::testing::community_editor(&ctx, "X-T20", false);
    let f = fixture(Provider::Cpu, Behaviour::default());
    let plain = f.plain();
    editor.document.set_image(plain.clone());
    editor.document.opened_at(f.photo("a.raw"));
    editor.document.edit.setup_mut().ai_denoise.enabled = true;
    assert_eq!(editor.document.image_file, None);
    let geometry = crate::develop::Geometry::new(&plain, editor.document.edit.recipe(), 0);
    editor.request_ai_denoise(&plain, &geometry, None);
    assert!(!editor.document.ai.asked_for(&plain));
    assert!(
        matches!(editor.document.ai.status(), Some(State::Failed(why)) if why.contains("changed")),
        "{:?}",
        editor.document.ai.status()
    );
}

/// A job queued or running holds the Library's edited previews at background pace,
/// also once the photo it was asked for is left: the run goes on, on most cores.
#[test]
fn a_job_under_way_keeps_the_librarys_previews_in_the_background() {
    use crate::app::library::Pace;
    let ctx = eframe::egui::Context::default();
    let mut editor = crate::app::testing::community_editor(&ctx, "X-T20", false);
    let f = fixture(Provider::Cpu, Behaviour::default());
    editor.ai_denoise = f.service.clone();
    editor.module = crate::app::Module::Library;
    assert_eq!(editor.preview_pace(), Pace::Full);
    let plain = f.plain();
    let gate = f.behaviour.gate.lock().unwrap();
    let (states, wanted) = f.request(&f.photo("a.raw"), &plain, None);
    assert_eq!(editor.preview_pace(), Pace::Background);
    assert!(matches!(
        states.recv_timeout(Duration::from_secs(20)),
        Ok(State::Running(_))
    ));
    // Left for the Library: no longer wanted, but the run finishes.
    wanted.store(false, Ordering::Relaxed);
    assert_eq!(editor.preview_pace(), Pace::Background);
    drop(gate);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while editor.preview_pace() == Pace::Background {
        assert!(std::time::Instant::now() < deadline, "the run never ended");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(f.behaviour.runs.load(Ordering::SeqCst), tiles());
}

/// The result is assembled at full size and handed over after the job leaves the
/// queue: the Library's previews stay in the background until that is done too.
#[test]
fn a_job_handing_over_its_result_keeps_the_librarys_previews_in_the_background() {
    use crate::app::library::Pace;
    let ctx = eframe::egui::Context::default();
    let mut editor = crate::app::testing::community_editor(&ctx, "X-T20", false);
    let f = fixture(Provider::Cpu, Behaviour::default());
    editor.ai_denoise = f.service.clone();
    editor.module = crate::app::Module::Library;
    // The final result's sink waits here, inside the hand-over.
    let handing = Arc::new(Mutex::new(()));
    let held = handing.lock().unwrap();
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let sink_gate = handing.clone();
    let photo = f.photo("a.raw");
    f.service.request(Request {
        path: photo.clone(),
        file: crate::storage::Identity::read(&photo).unwrap(),
        plain: f.plain(),
        region: None,
        wanted: Arc::new(AtomicBool::new(true)),
        sink: Arc::new(move |u: Update| {
            let last = matches!(u.state, State::Ready(_) | State::Failed(_));
            if last {
                let _ = tx.lock().unwrap().send(());
                drop(sink_gate.lock().unwrap());
            }
        }),
    });
    rx.recv_timeout(Duration::from_secs(20))
        .expect("the result is handed over");
    assert_eq!(editor.preview_pace(), Pace::Background);
    drop(held);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while editor.preview_pace() == Pace::Background {
        assert!(
            std::time::Instant::now() < deadline,
            "the hand-over never ended"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Issue 808 (D5): command-line renders keep AI Denoise results where the app keeps
/// them, in the folder Preferences chose for decoded images.
#[test]
fn command_line_renders_use_the_cache_folder_preferences_chose() {
    let chosen = tempfile::tempdir().unwrap();
    let session = crate::app::preferences::session::Session {
        decoded_cache_folder: Some(chosen.path().to_path_buf()),
        ..Default::default()
    };
    assert!(user_cache(&session).dir().starts_with(chosen.path()));
    let default = crate::app::preferences::session::Session::default();
    assert_eq!(user_cache(&default).dir(), AiCache::default().dir());
}

/// The Library's Loupe shows a finished result of any provider's run, found once
/// one is stored, without ever loading the model.
#[test]
fn the_loupe_loads_a_finished_result_of_any_provider_without_running_the_model() {
    let ctx = eframe::egui::Context::default();
    let mut editor = crate::app::testing::community_editor(&ctx, "X-T20", false);
    // The service would run on CoreML; the result was made on the CPU.
    let f = fixture(Provider::CoreML, Behaviour::default());
    editor.ai_denoise = f.service.clone();
    let path = f.photo("a.raw");
    let plain = f.plain();
    editor.document.set_image(plain);
    editor.document.opened_at(path.clone());
    editor.document.image_file = editor.document.file.clone();
    editor.module = crate::app::Module::Library;
    crate::app::testing::in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.recipe_mut().switch_ai_denoise(true)
    });
    editor.schedule();
    assert!(editor.document.ai.source().is_none(), "none stored: plain");
    let layout = PackedBayer::pack(W, H, &mosaic(), [0, 0]).layout;
    let entry = AiEntry {
        layout,
        gain: 1.,
        rgb: vec![0.3; 3 * layout.output_pixels()],
        clipped: vec![false; layout.packed_width * layout.packed_height],
    };
    f.service
        .cache()
        .store(&key(&path, Provider::Cpu).unwrap(), &entry)
        .unwrap();
    editor.ai_result_stored(&path);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !editor.document.ai.is_complete() {
        assert!(std::time::Instant::now() < deadline, "never shown");
        std::thread::sleep(Duration::from_millis(10));
        editor.events(&ctx);
    }
    assert!(editor.document.ai.source().is_some());
    assert!(
        f.behaviour.loads.lock().unwrap().is_empty(),
        "the model ran"
    );
}

/// A final result is kept when a partial one for the same image comes after it, as
/// when the Loupe's cached load and a Develop run both answer: the photo never shows,
/// nor gives the Library, a partial result as final.
#[test]
fn a_partial_result_after_the_final_one_is_ignored() {
    let ctx = eframe::egui::Context::default();
    let mut editor = crate::app::testing::community_editor(&ctx, "X-T20", false);
    let f = fixture(Provider::Cpu, Behaviour::default());
    let plain = f.plain();
    editor.document.set_image(plain.clone());
    let final_one = AiSource::recover((*plain).clone(), Cancel::NEVER).unwrap();
    let mut other = (*plain).clone();
    other.pixels.iter_mut().for_each(|p| p[0] = 0.9);
    let partial = AiSource::recover(other, Cancel::NEVER).unwrap();
    editor.ai_denoise_update(Update {
        plain: Arc::downgrade(&plain),
        state: State::Ready(final_one.clone()),
    });
    editor.ai_denoise_update(Update {
        plain: Arc::downgrade(&plain),
        state: State::Partial(partial),
    });
    assert!(editor.document.ai.is_complete());
    assert_eq!(editor.document.ai.source(), Some(&final_one));
}

/// Leaving the photo, or switching AI Denoise off, cancels the Loupe's load of its
/// finished result; one cancelled before it was shown is looked for again next time.
#[test]
fn a_cancelled_loupe_load_is_looked_for_again() {
    let image = Arc::new(fixture(Provider::Cpu, Behaviour::default()).plain());
    let cancel = Arc::new(AtomicBool::new(false));
    let mut photo = Photo::default();
    photo.looked_up = Some(Arc::downgrade(&image));
    photo.loading = Some(cancel.clone());
    photo.release();
    assert!(cancel.load(Ordering::Relaxed));
    assert!(photo.looked_up.is_none(), "looked for again");
    let left = Arc::new(AtomicBool::new(false));
    photo.loading = Some(left.clone());
    drop(photo);
    assert!(left.load(Ordering::Relaxed));
}
