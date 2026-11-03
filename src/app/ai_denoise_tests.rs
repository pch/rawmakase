use super::*;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;

/// How the stand-in model behaves.
#[derive(Default)]
struct Behaviour {
    /// Tiles run, across all loads.
    runs: AtomicUsize,
    /// Loads, per provider.
    loads: Mutex<Vec<Provider>>,
    /// A non-finite value from this run on, on a provider other than the CPU.
    gpu_poison_after: Option<usize>,
    /// Panic on the first run.
    panic: bool,
    /// Never return from a run, whatever the cancel flag says.
    hang: bool,
    /// Held before each run.
    gate: Mutex<()>,
}

/// Nearest-neighbour stand-in for the network, at the model's own scale (×10⁶).
struct Fake {
    provider: Provider,
    behaviour: Arc<Behaviour>,
}
impl TileModel for Fake {
    fn run(&mut self, input: &[f32], cancel: &AtomicBool) -> Result<Vec<f32>, InferenceError> {
        let _gate = self.behaviour.gate.lock().unwrap();
        if cancel.load(Ordering::Relaxed) {
            return Err(InferenceError::Cancelled);
        }
        let n = self.behaviour.runs.fetch_add(1, Ordering::SeqCst) + 1;
        if self.behaviour.panic {
            panic!("fake model panicked");
        }
        if self.behaviour.hang {
            loop {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let s = RAWNIND.tile;
        let o = 2 * s;
        let mut out = vec![0.; 3 * o * o];
        for y in 0..s {
            for x in 0..s {
                let p = |c: usize| input[(c * s + y) * s + x];
                let rgb = [p(0), (p(1) + p(2)) / 2., p(3)];
                for (c, v) in rgb.into_iter().enumerate() {
                    for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                        out[(c * o + 2 * y + dy) * o + 2 * x + dx] = v * 1e6;
                    }
                }
            }
        }
        if self.provider != Provider::Cpu
            && self
                .behaviour
                .gpu_poison_after
                .is_some_and(|after| n > after)
        {
            out[5] = f32::INFINITY;
        }
        Ok(out)
    }
}

struct Fixture {
    service: Service,
    behaviour: Arc<Behaviour>,
    dir: tempfile::TempDir,
}

const W: usize = 1000;
const H: usize = 300;

fn mosaic() -> Vec<f32> {
    (0..W * H)
        .map(|i| 0.1 + 0.6 * ((i * 7919) % 997) as f32 / 997.)
        .collect()
}

fn fixture(provider: Provider, behaviour: Behaviour) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let behaviour = Arc::new(behaviour);
    let source: Source = Arc::new(|path: &Path, _: &AtomicBool| {
        anyhow::ensure!(path.exists(), "no such photo");
        if std::fs::read(path)?.starts_with(b"xtrans") {
            return Err(anyhow::Error::new(Unsupported("not Bayer")));
        }
        Ok(PackedBayer::pack(W, H, &mosaic(), [0, 0]))
    });
    let b = behaviour.clone();
    let loader: Loader = Arc::new(move |_: &Path, provider: Provider| {
        b.loads.lock().unwrap().push(provider);
        Ok(Box::new(Fake {
            provider,
            behaviour: b.clone(),
        }) as Box<dyn TileModel + Send>)
    });
    let service = Service::with(
        AiCache::new(dir.path().join("cache"), 1 << 30),
        provider,
        source,
        loader,
    );
    let model = dir.path().join("model");
    std::fs::create_dir(&model).unwrap();
    std::fs::write(model.join(RAWNIND.model.name), b"m").unwrap();
    service.set_model(Some(model));
    Fixture {
        service,
        behaviour,
        dir,
    }
}

impl Fixture {
    fn photo(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        if !path.exists() {
            std::fs::write(&path, name.as_bytes()).unwrap();
        }
        path
    }
    fn plain(&self) -> Arc<CameraImage> {
        let metadata = crate::camera_data::Metadata {
            wb: [2., 1., 1.5],
            ..Default::default()
        };
        Arc::new(CameraImage {
            recovered: Default::default(),
            width: W as u32,
            height: H as u32,
            pixels: vec![[0.2; 3]; W * H],
            metadata,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        })
    }
    /// A Develop request whose updates arrive on the returned channel.
    fn request(
        &self,
        path: &Path,
        plain: &Arc<CameraImage>,
        region: Option<[u32; 4]>,
    ) -> (mpsc::Receiver<State>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let wanted = Arc::new(AtomicBool::new(true));
        self.service.request(Request {
            path: path.to_path_buf(),
            file: crate::storage::Identity::read(path).unwrap(),
            plain: plain.clone(),
            region,
            wanted: wanted.clone(),
            sink: Arc::new(move |u: Update| {
                let _ = tx.lock().unwrap().send(u.state);
            }),
        });
        (rx, wanted)
    }
}

/// An export's request for `photo`, as decoded now.
fn export(
    service: &Service,
    photo: &Path,
    plain: &Arc<CameraImage>,
    cancel: &AtomicBool,
) -> Result<Arc<CameraImage>> {
    service.get(
        photo,
        &crate::storage::Identity::read(photo)?,
        plain,
        cancel,
    )
}

/// The states until the first final one.
fn until_final(rx: &mpsc::Receiver<State>) -> Vec<State> {
    let mut states = Vec::new();
    loop {
        let state = rx.recv_timeout(Duration::from_secs(20)).expect("an update");
        let last = matches!(state, State::Ready(_) | State::Failed(_));
        states.push(state);
        if last {
            return states;
        }
    }
}

fn tiles() -> usize {
    Grid::new(W.div_ceil(2), H.div_ceil(2)).tiles().len()
}

#[test]
fn develop_gets_a_partial_then_the_final_result_and_the_cache_serves_it_next() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let photo = f.photo("a.raw");
    let plain = f.plain();
    assert!(tiles() >= 2);
    // The 100% view at the right edge: its tile runs first, then one partial result.
    let (rx, _wanted) = f.request(&photo, &plain, Some([900, 100, 50, 50]));
    let states = until_final(&rx);
    let partial: Vec<_> = states
        .iter()
        .filter_map(|s| match s {
            State::Partial(p) => Some(p.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(partial.len(), 1, "{states:?}");
    let State::Ready(ready) = states.last().unwrap() else {
        panic!("{states:?}");
    };
    // A new source each time, so render caches keyed by it see the change.
    assert!(partial[0] != *ready);
    // Not computed yet in the partial: the plain image (white balanced 0.2) at the left.
    let p = &partial[0].image().pixels;
    assert_eq!(p[10], [0.2; 3]);
    // Computed: the model's output at the mosaic's scale, white balanced.
    let r = &ready.image().pixels;
    let m = mosaic();
    assert!(
        (r[10][0] - m[10] * 2.).abs() < 1e-3,
        "{:?} {}",
        r[10],
        m[10]
    );
    assert_eq!(f.behaviour.runs.load(Ordering::SeqCst), tiles());
    // Asked again: from the cache, no model run.
    let (rx, _wanted) = f.request(&photo, &plain, None);
    let states = until_final(&rx);
    assert!(matches!(states.last(), Some(State::Ready(_))), "{states:?}");
    assert_eq!(f.behaviour.runs.load(Ordering::SeqCst), tiles());
}

#[test]
fn concurrent_requests_for_one_photo_run_one_job() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let photo = f.photo("a.raw");
    let plain = f.plain();
    let gate = f.behaviour.gate.lock().unwrap();
    let (rx, _wanted) = f.request(&photo, &plain, None);
    let exports: Vec<_> = (0..2)
        .map(|_| {
            let (service, photo, plain) = (f.service.clone(), photo.clone(), plain.clone());
            std::thread::spawn(move || export(&service, &photo, &plain, &AtomicBool::new(false)))
        })
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    drop(gate);
    for export in exports {
        let image = export.join().unwrap().unwrap();
        assert_eq!(image.width, W as u32);
    }
    assert!(matches!(until_final(&rx).last(), Some(State::Ready(_))));
    assert_eq!(f.behaviour.runs.load(Ordering::SeqCst), tiles());
}

#[test]
fn a_gpu_failure_after_good_tiles_reruns_on_the_cpu_under_the_cpus_key() {
    let f = fixture(
        Provider::CoreML,
        Behaviour {
            gpu_poison_after: Some(1),
            ..Default::default()
        },
    );
    let photo = f.photo("a.raw");
    let plain = f.plain();
    let (rx, _wanted) = f.request(&photo, &plain, None);
    let export = {
        let (service, photo, plain) = (f.service.clone(), photo.clone(), plain.clone());
        std::thread::spawn(move || export(&service, &photo, &plain, &AtomicBool::new(false)))
    };
    assert!(matches!(until_final(&rx).last(), Some(State::Ready(_))));
    export.join().unwrap().unwrap();
    assert_eq!(
        *f.behaviour.loads.lock().unwrap(),
        [Provider::CoreML, Provider::Cpu]
    );
    // The cached result is the CPU's, and a later request hits it without the GPU.
    assert!(
        f.service
            .cache()
            .contains(&key(&photo, Provider::Cpu).unwrap())
    );
    assert!(
        !f.service
            .cache()
            .contains(&key(&photo, Provider::CoreML).unwrap())
    );
    let runs = f.behaviour.runs.load(Ordering::SeqCst);
    let (rx, _wanted) = f.request(&photo, &plain, None);
    assert!(matches!(until_final(&rx).last(), Some(State::Ready(_))));
    assert_eq!(f.behaviour.runs.load(Ordering::SeqCst), runs);
}

#[test]
fn failures_and_panics_are_reported_and_the_worker_carries_on() {
    let f = fixture(
        Provider::Cpu,
        Behaviour {
            panic: true,
            ..Default::default()
        },
    );
    let plain = f.plain();
    let (rx, _wanted) = f.request(&f.photo("a.raw"), &plain, None);
    let states = until_final(&rx);
    assert!(
        matches!(states.last(), Some(State::Failed(why)) if why.contains("unexpectedly")),
        "{states:?}"
    );
    // An unsupported sensor says why; an export of it fails with that reason.
    let xtrans = f.photo("xtrans.raf");
    let (rx, _wanted) = f.request(&xtrans, &plain, None);
    assert!(matches!(until_final(&rx).last(), Some(State::Failed(why)) if why == "not Bayer"));
    let error = f
        .service
        .get(
            &xtrans,
            &crate::storage::Identity::read(&xtrans).unwrap(),
            &plain,
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "not Bayer");
    // No model: the reason, before anything runs.
    f.service.set_model(None);
    let (rx, _wanted) = f.request(&f.photo("b.raw"), &plain, None);
    assert!(
        matches!(until_final(&rx).last(), Some(State::Failed(why)) if why.contains("not installed"))
    );
}

#[test]
fn a_queued_request_no_longer_wanted_is_dropped() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let plain = f.plain();
    let gate = f.behaviour.gate.lock().unwrap();
    let (first, _a) = f.request(&f.photo("a.raw"), &plain, None);
    std::thread::sleep(Duration::from_millis(50));
    let (second, wanted) = f.request(&f.photo("b.raw"), &plain, None);
    assert!(matches!(
        second.recv_timeout(Duration::from_secs(5)),
        Ok(State::Queued)
    ));
    wanted.store(false, Ordering::Relaxed);
    drop(gate);
    assert!(matches!(until_final(&first).last(), Some(State::Ready(_))));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(f.behaviour.runs.load(Ordering::SeqCst), tiles());
    assert!(second.try_recv().is_err());
}

#[test]
fn a_result_is_rejected_when_the_file_or_the_model_changes_during_the_run() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let photo = f.photo("a.raw");
    let plain = f.plain();
    let gate = f.behaviour.gate.lock().unwrap();
    let (rx, _wanted) = f.request(&photo, &plain, None);
    std::thread::sleep(Duration::from_millis(100));
    std::fs::write(&photo, b"replaced with another photo").unwrap();
    drop(gate);
    let states = until_final(&rx);
    assert!(
        matches!(states.last(), Some(State::Failed(why)) if why.contains("changed")),
        "{states:?}"
    );
    assert_eq!(
        std::fs::read_dir(f.dir.path().join("cache")).map_or(0, |d| d.count()),
        0
    );

    let other = f.photo("b.raw");
    let gate = f.behaviour.gate.lock().unwrap();
    let (rx, _wanted) = f.request(&other, &plain, None);
    std::thread::sleep(Duration::from_millis(100));
    f.service.set_model(None);
    drop(gate);
    let states = until_final(&rx);
    assert!(
        matches!(states.last(), Some(State::Failed(why)) if why.contains("removed")),
        "{states:?}"
    );
}

#[test]
fn a_run_that_never_returns_does_not_hold_quitting_past_the_deadline() {
    let f = fixture(
        Provider::Cpu,
        Behaviour {
            hang: true,
            ..Default::default()
        },
    );
    let plain = f.plain();
    let (_rx, _wanted) = f.request(&f.photo("a.raw"), &plain, None);
    while f.behaviour.runs.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = std::time::Instant::now();
    let waited = super::super::task::wait_for(vec![f.service.stop()], Duration::from_millis(200));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(waited.detached, 1);
    // After stopping, an export fails at once rather than waiting.
    assert!(
        f.service
            .get(
                &f.photo("b.raw"),
                &crate::storage::Identity::read(&f.photo("b.raw")).unwrap(),
                &plain,
                &AtomicBool::new(false),
            )
            .is_err()
    );
}

#[test]
fn a_cancelled_export_stops_waiting() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let plain = f.plain();
    let _gate = f.behaviour.gate.lock().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let export = {
        let (service, photo, plain, cancel) =
            (f.service.clone(), f.photo("a.raw"), plain, cancel.clone());
        std::thread::spawn(move || export(&service, &photo, &plain, &cancel))
    };
    std::thread::sleep(Duration::from_millis(100));
    cancel.store(true, Ordering::Relaxed);
    assert!(export.join().unwrap().is_err());
}

/// The Detail panel's switch and Amount are edits: one History step each, the edit
/// needing a save, undone like any other. A result arriving is not an edit, and a
/// result for an image no longer shown is dropped.
#[test]
fn the_switch_is_an_edit_and_a_result_is_not() {
    use super::super::tests::{community_editor, labelled};
    use eframe::egui;
    // The Detail panel sits far down: a tall window keeps it on screen.
    fn controls_frame(
        ctx: &egui::Context,
        editor: &mut super::super::Editor,
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
    fn click(ctx: &egui::Context, editor: &mut super::super::Editor, node: &egui::accesskit::Node) {
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
    let source = AiSource::recover((*plain).clone(), &AtomicBool::new(false)).unwrap();
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

/// Removing the model: the running job is cancelled, the worker drops its sessions,
/// and later requests fail with the reason rather than using the removed files.
#[test]
fn unloading_cancels_the_run_drops_the_model_and_refuses_later_jobs() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let plain = f.plain();
    let gate = f.behaviour.gate.lock().unwrap();
    let (rx, _wanted) = f.request(&f.photo("a.raw"), &plain, None);
    std::thread::sleep(Duration::from_millis(100));
    let service = f.service.clone();
    let unloading = std::thread::spawn(move || service.unload());
    std::thread::sleep(Duration::from_millis(50));
    drop(gate);
    unloading.join().unwrap();
    assert!(!f.service.queue().unload, "the worker dropped its models");
    // The cancelled run reported nothing as a failure; nothing was cached.
    while let Ok(state) = rx.recv_timeout(Duration::from_millis(300)) {
        assert!(!matches!(state, State::Ready(_)), "{state:?}");
    }
    let (rx, _wanted) = f.request(&f.photo("b.raw"), &plain, None);
    assert!(
        matches!(until_final(&rx).last(), Some(State::Failed(why)) if why.contains("not installed"))
    );
}

/// A file replaced while its job waited in the queue is not run: the identity the
/// request captured no longer holds, and nothing is cached under its key.
#[test]
fn a_file_replaced_while_its_job_waits_is_refused() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let plain = f.plain();
    let gate = f.behaviour.gate.lock().unwrap();
    let (first, _a) = f.request(&f.photo("a.raw"), &plain, None);
    std::thread::sleep(Duration::from_millis(50));
    let b = f.photo("b.raw");
    let original_key = key(&b, Provider::Cpu).unwrap();
    let (second, _b) = f.request(&b, &plain, None);
    std::fs::write(&b, b"another photo of other length").unwrap();
    drop(gate);
    assert!(matches!(until_final(&first).last(), Some(State::Ready(_))));
    let states = until_final(&second);
    assert!(
        matches!(states.last(), Some(State::Failed(why)) if why.contains("changed")),
        "{states:?}"
    );
    assert!(!f.service.cache().contains(&original_key));
}

/// A request for the same photo made while the finished job is still handing out its
/// result is not lost: it gets the result too (from the cache).
#[test]
fn a_request_while_the_result_is_handed_out_still_gets_it() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let photo = f.photo("a.raw");
    let plain = f.plain();
    let (entered_tx, entered) = mpsc::channel::<()>();
    let (release_tx, release) = mpsc::channel::<()>();
    let (entered_tx, release) = (Mutex::new(entered_tx), Mutex::new(release));
    let wanted = Arc::new(AtomicBool::new(true));
    f.service.request(Request {
        path: photo.clone(),
        file: crate::storage::Identity::read(&photo).unwrap(),
        plain: plain.clone(),
        region: None,
        wanted,
        sink: Arc::new(move |u: Update| {
            if matches!(u.state, State::Ready(_)) {
                let _ = entered_tx.lock().unwrap().send(());
                let _ = release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10));
            }
        }),
    });
    entered
        .recv_timeout(Duration::from_secs(20))
        .expect("the first result");
    let (second, _wanted) = f.request(&photo, &plain, None);
    release_tx.send(()).unwrap();
    assert!(matches!(until_final(&second).last(), Some(State::Ready(_))));
}

/// A render of another photo (Reference View) never gets this photo's result.
#[test]
fn only_the_documents_own_image_gets_its_source() {
    let mut doc = super::super::state::Document::default();
    let plain = fixture(Provider::Cpu, Behaviour::default()).plain();
    doc.set_image(plain.clone());
    doc.ai.source = Some(AiSource::recover((*plain).clone(), &AtomicBool::new(false)).unwrap());
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
    photo.source = Some(AiSource::recover((*plain).clone(), &AtomicBool::new(false)).unwrap());
    assert!(photo.needs_request(&plain));
    photo.complete = true;
    assert!(!photo.needs_request(&plain));
    photo.complete = false;
    photo.failed = true;
    assert!(!photo.needs_request(&plain));
}

/// A photo decoded from one file is never denoised from another: a request or an
/// export whose file changed since it was decoded is refused.
#[test]
fn a_file_changed_since_it_was_decoded_is_refused() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let photo = f.photo("a.raw");
    let plain = f.plain();
    let decoded = crate::storage::Identity::read(&photo).unwrap();
    std::fs::write(&photo, b"a different photo, same name").unwrap();
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    f.service.request(Request {
        path: photo.clone(),
        file: decoded.clone(),
        plain: plain.clone(),
        region: None,
        wanted: Arc::new(AtomicBool::new(true)),
        sink: Arc::new(move |u: Update| {
            let _ = tx.lock().unwrap().send(u.state);
        }),
    });
    assert!(matches!(until_final(&rx).last(), Some(State::Failed(why)) if why.contains("changed")));
    let error = f
        .service
        .get(&photo, &decoded, &plain, &AtomicBool::new(false))
        .err()
        .unwrap();
    assert!(error.to_string().contains("changed"), "{error}");
    assert_eq!(f.behaviour.runs.load(Ordering::SeqCst), 0);
}

/// A queued job whose file was replaced is refused even when the cache holds the
/// result of the file it was asked for.
#[test]
fn a_replaced_file_is_refused_on_a_cache_hit_too() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let plain = f.plain();
    let b = f.photo("b.raw");
    let (warm, _w) = f.request(&b, &plain, None);
    assert!(matches!(until_final(&warm).last(), Some(State::Ready(_))));
    let gate = f.behaviour.gate.lock().unwrap();
    let (first, _a) = f.request(&f.photo("a.raw"), &plain, None);
    std::thread::sleep(Duration::from_millis(50));
    let (second, _b) = f.request(&b, &plain, None);
    std::fs::write(&b, b"replaced by another photo").unwrap();
    drop(gate);
    assert!(matches!(until_final(&first).last(), Some(State::Ready(_))));
    let states = until_final(&second);
    assert!(
        matches!(states.last(), Some(State::Failed(why)) if why.contains("changed")),
        "{states:?}"
    );
}

/// After the GPU failed, jobs already queued run on the CPU under the CPU's keys.
#[test]
fn jobs_queued_when_the_gpu_fails_move_to_the_cpu() {
    let f = fixture(
        Provider::CoreML,
        Behaviour {
            gpu_poison_after: Some(1),
            ..Default::default()
        },
    );
    let plain = f.plain();
    let gate = f.behaviour.gate.lock().unwrap();
    let (first, _a) = f.request(&f.photo("a.raw"), &plain, None);
    std::thread::sleep(Duration::from_millis(50));
    let b = f.photo("b.raw");
    let (second, _b) = f.request(&b, &plain, None);
    drop(gate);
    assert!(matches!(until_final(&first).last(), Some(State::Ready(_))));
    assert!(matches!(until_final(&second).last(), Some(State::Ready(_))));
    assert!(f.service.cache().contains(&key(&b, Provider::Cpu).unwrap()));
    assert!(
        !f.service
            .cache()
            .contains(&key(&b, Provider::CoreML).unwrap())
    );
}

/// A partial result is dropped when the final one fails: Develop renders plain.
#[test]
fn a_failure_after_a_partial_result_drops_it() {
    use super::super::tests::community_editor;
    let ctx = eframe::egui::Context::default();
    let mut editor = community_editor(&ctx, "X-T20", false);
    let plain = fixture(Provider::Cpu, Behaviour::default()).plain();
    editor.document.set_image(plain.clone());
    editor.document.edit.setup_mut().ai_denoise.enabled = true;
    let partial = AiSource::recover((*plain).clone(), &AtomicBool::new(false)).unwrap();
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
    use super::super::worker::Event;
    let ctx = eframe::egui::Context::default();
    let mut editor = super::super::tests::community_editor(&ctx, "X-T20", false);
    let f = fixture(Provider::Cpu, Behaviour::default());
    let photo = f.photo("a.raw");
    let before = crate::storage::Identity::read(&photo).unwrap();
    let (generation, _) = editor.load.start();
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
    let mut editor = super::super::tests::community_editor(&ctx, "X-T20", false);
    let f = fixture(Provider::Cpu, Behaviour::default());
    let plain = f.plain();
    editor.document.set_image(plain.clone());
    editor.document.path = Some(f.photo("a.raw"));
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
    let mut editor = super::super::tests::community_editor(&ctx, "X-T20", false);
    let f = fixture(Provider::Cpu, Behaviour::default());
    editor.ai_denoise = f.service.clone();
    editor.module = super::super::Module::Library;
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
    let mut editor = super::super::tests::community_editor(&ctx, "X-T20", false);
    let f = fixture(Provider::Cpu, Behaviour::default());
    editor.ai_denoise = f.service.clone();
    editor.module = super::super::Module::Library;
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
