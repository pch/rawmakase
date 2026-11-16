use super::*;
use rawmakase_cancel::Cancel;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;

/// How the stand-in model behaves.
#[derive(Default)]
pub struct Behaviour {
    /// Tiles run, across all loads.
    pub runs: AtomicUsize,
    /// Loads, per provider.
    pub loads: Mutex<Vec<Provider>>,
    /// A non-finite value from this run on, on a provider other than the CPU.
    gpu_poison_after: Option<usize>,
    /// Panic on the first run.
    panic: bool,
    /// Never return from a run, whatever the cancel flag says.
    hang: bool,
    /// Held before each run.
    pub gate: Mutex<()>,
    /// Panic when a loaded model is dropped, outside any job.
    panic_on_drop: bool,
}

/// Nearest-neighbour stand-in for the network, at the model's own scale (×10⁶).
struct Fake {
    provider: Provider,
    behaviour: Arc<Behaviour>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        if self.behaviour.panic_on_drop && !std::thread::panicking() {
            panic!("fake model panicked when dropped");
        }
    }
}

impl TileModel for Fake {
    fn run(&mut self, input: &[f32], cancel: Cancel<'_>) -> Result<Vec<f32>, InferenceError> {
        let _gate = self.behaviour.gate.lock().unwrap();
        if cancel.is_cancelled() {
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

pub struct Fixture {
    pub service: Service,
    pub behaviour: Arc<Behaviour>,
    pub dir: tempfile::TempDir,
}

pub const W: usize = 1000;

pub const H: usize = 300;

pub fn mosaic() -> Vec<f32> {
    (0..W * H)
        .map(|i| 0.1 + 0.6 * ((i * 7919) % 997) as f32 / 997.)
        .collect()
}

pub fn fixture(provider: Provider, behaviour: Behaviour) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let behaviour = Arc::new(behaviour);
    let source: Source = Arc::new(|path: &Path, _: Cancel<'_>| {
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
    pub fn photo(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        if !path.exists() {
            std::fs::write(&path, name.as_bytes()).unwrap();
        }
        path
    }
    pub fn plain(&self) -> Arc<CameraImage> {
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
            hdr: Default::default(),
        })
    }
    /// A Develop request whose updates arrive on the returned channel.
    pub fn request(
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
#[cfg(test)]
fn export(
    service: &Service,
    photo: &Path,
    plain: &Arc<CameraImage>,
    cancel: Cancel<'_>,
) -> Result<Arc<CameraImage>> {
    service.get(
        photo,
        &crate::storage::Identity::read(photo)?,
        plain,
        cancel,
    )
}

/// The states until the first final one.
#[cfg(test)]
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

pub fn tiles() -> usize {
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
            std::thread::spawn(move || export(&service, &photo, &plain, Cancel::NEVER))
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
        std::thread::spawn(move || export(&service, &photo, &plain, Cancel::NEVER))
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
            Cancel::NEVER,
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
    let finished = wait_within(f.service.stop(), Duration::from_millis(200));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!finished);
    // After stopping, an export fails at once rather than waiting.
    assert!(
        f.service
            .get(
                &f.photo("b.raw"),
                &crate::storage::Identity::read(&f.photo("b.raw")).unwrap(),
                &plain,
                Cancel::NEVER,
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
        std::thread::spawn(move || export(&service, &photo, &plain, Cancel::new(&cancel)))
    };
    std::thread::sleep(Duration::from_millis(100));
    cancel.store(true, Ordering::Relaxed);
    assert!(export.join().unwrap().is_err());
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
        .get(&photo, &decoded, &plain, Cancel::NEVER)
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

/// Issue 808 (D3): Develop's request is keyed by the file's identity captured before
/// decoding, without reading the file on the calling thread (the UI's): a file on a
/// stalled drive must not freeze the window. The worker still checks the file.
#[cfg(unix)]
#[test]
fn a_request_does_not_read_the_file_on_the_calling_thread() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let photo = f.photo("a.raw");
    let file = crate::storage::Identity::read(&photo).unwrap();
    // Replaced by a pipe nobody writes to: opening it blocks, as a stalled drive does.
    std::fs::remove_file(&photo).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&photo)
        .status()
        .unwrap();
    assert!(made.success());
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let (returned, returned_rx) = mpsc::channel();
    let service = f.service.clone();
    let plain = f.plain();
    let path = photo.clone();
    std::thread::spawn(move || {
        service.request(Request {
            path,
            file,
            plain,
            region: None,
            wanted: Arc::new(AtomicBool::new(true)),
            sink: Arc::new(move |u: Update| {
                let _ = tx.lock().unwrap().send(u.state);
            }),
        });
        let _ = returned.send(());
    });
    returned_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the request read the file on the calling thread");
    // The drive comes back with something else there: the worker refuses it.
    drop(
        std::fs::OpenOptions::new()
            .write(true)
            .open(&photo)
            .unwrap(),
    );
    let states = until_final(&rx);
    assert!(
        matches!(states.last(), Some(State::Failed(why)) if why.contains("changed")),
        "{states:?}"
    );
    assert_eq!(f.behaviour.runs.load(Ordering::SeqCst), 0);
}

/// Issue 808 (D3, review): a request that joins a job for the same file under another
/// path (a hard link) gets no result once its own file was replaced, though the job's
/// file is unchanged: the request is checked against its own file, by the worker.
#[cfg(unix)]
#[test]
fn a_request_joining_another_paths_job_is_refused_when_its_file_changed() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let plain = f.plain();
    let a = f.photo("a.raw");
    let b = f.dir.path().join("b.raw");
    std::fs::hard_link(&a, &b).unwrap();
    let decoded_b = crate::storage::Identity::read(&b).unwrap();
    for joins in ["running", "pending"] {
        let gate = f.behaviour.gate.lock().unwrap();
        // A job ahead holds the worker while the one asked for in Develop is pending.
        let (ahead, _x) = (joins == "pending")
            .then(|| f.request(&f.photo("x.raw"), &plain, None))
            .unzip();
        let (first, _a) = f.request(&a, &plain, None);
        std::fs::remove_file(&b).unwrap();
        std::fs::write(&b, b"another photo under the linked name").unwrap();
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        f.service.request(Request {
            path: b.clone(),
            file: decoded_b.clone(),
            plain: plain.clone(),
            region: None,
            wanted: Arc::new(AtomicBool::new(true)),
            sink: Arc::new(move |u: Update| {
                let _ = tx.lock().unwrap().send(u.state);
            }),
        });
        drop(gate);
        if let Some(ahead) = ahead {
            until_final(&ahead);
        }
        assert!(
            matches!(until_final(&first).last(), Some(State::Ready(_))),
            "{joins}"
        );
        let states = until_final(&rx);
        assert!(
            matches!(states.last(), Some(State::Failed(why)) if why.contains("changed")),
            "{joins}: {states:?}"
        );
        assert!(
            !states
                .iter()
                .any(|s| matches!(s, State::Partial(_) | State::Ready(_))),
            "{joins}: {states:?}"
        );
        // Linked again for the next round.
        std::fs::remove_file(&b).unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        f.service.cache().purge();
    }
}

/// Issue 808 (D4): a sink that panics on the final result, after the job's outcome is
/// published, must not strand the job queued behind it: its export waiter and its
/// Develop request each get a final outcome, and the service is idle afterwards.
#[test]
fn a_sink_panicking_on_the_final_result_strands_no_queued_job() {
    let f = fixture(Provider::Cpu, Behaviour::default());
    let plain = f.plain();
    let gate = f.behaviour.gate.lock().unwrap();
    let a = f.photo("a.raw");
    f.service.request(Request {
        path: a.clone(),
        file: crate::storage::Identity::read(&a).unwrap(),
        plain: plain.clone(),
        region: None,
        wanted: Arc::new(AtomicBool::new(true)),
        sink: Arc::new(|u: Update| {
            if matches!(u.state, State::Ready(_) | State::Failed(_)) {
                panic!("a sink panicked on the final result");
            }
        }),
    });
    // A's job is running (held at its first tile); B's is queued behind it.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while f.service.queue().running.is_none() {
        assert!(std::time::Instant::now() < deadline, "A's job never ran");
        std::thread::sleep(Duration::from_millis(5));
    }
    let b = f.photo("b.raw");
    let (develop, _wanted) = f.request(&b, &plain, None);
    let (exported, exported_rx) = mpsc::channel();
    let (service, waiter_plain, waiter_photo) = (f.service.clone(), plain, b);
    std::thread::spawn(move || {
        let _ =
            exported.send(export(&service, &waiter_photo, &waiter_plain, Cancel::NEVER).is_ok());
    });
    while f.service.queue().waiting.is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the export never waited"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(gate);
    let states = until_final(&develop);
    assert!(
        matches!(states.last(), Some(State::Ready(_) | State::Failed(_))),
        "{states:?}"
    );
    exported_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the export waiting for the queued job gets an outcome");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while f.service.busy() {
        assert!(
            std::time::Instant::now() < deadline,
            "the service stays busy"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Issue 808 (D4): when the worker thread itself dies, between jobs, the jobs queued
/// fail rather than wait for it, the service is idle, and the next request starts a
/// new worker.
#[test]
fn queued_jobs_fail_when_the_worker_dies_and_the_next_request_restarts_it() {
    let f = fixture(
        Provider::Cpu,
        Behaviour {
            panic_on_drop: true,
            ..Default::default()
        },
    );
    let plain = f.plain();
    let (first, _a) = f.request(&f.photo("a.raw"), &plain, None);
    assert!(matches!(until_final(&first).last(), Some(State::Ready(_))));
    // Queued while the worker is to drop its loaded model, whose drop panics.
    let b = f.photo("b.raw");
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    {
        let mut queue = f.service.queue();
        let identity = crate::storage::Identity::read(&b).unwrap();
        queue.pending.push_back(Job {
            key: key_of(&identity, Provider::Cpu),
            provider: Provider::Cpu,
            identity,
            path: b.clone(),
            region: None,
            develop: vec![(
                plain.clone(),
                Arc::new(AtomicBool::new(true)),
                Arc::new(move |u: Update| {
                    let _ = tx.lock().unwrap().send(u.state);
                }),
            )],
            waiters: 0,
        });
        queue.unload = true;
    }
    f.service.shared.changed.notify_all();
    let states = until_final(&rx);
    assert!(
        matches!(states.last(), Some(State::Failed(why)) if why.contains("unexpectedly")),
        "{states:?}"
    );
    assert!(!f.service.busy());
    // A new worker takes the next request.
    let (again, _b) = f.request(&b, &plain, None);
    assert!(matches!(until_final(&again).last(), Some(State::Ready(_))));
}

/// Issue 808 (D4, review): a request made while the dying worker is still telling the
/// jobs it failed is not left to it: it starts a new worker, which runs it.
#[test]
fn a_request_while_the_dying_worker_reports_its_failures_starts_a_new_worker() {
    let f = fixture(
        Provider::Cpu,
        Behaviour {
            panic_on_drop: true,
            ..Default::default()
        },
    );
    let plain = f.plain();
    let (first, _a) = f.request(&f.photo("a.raw"), &plain, None);
    assert!(matches!(until_final(&first).last(), Some(State::Ready(_))));
    // B's failure is reported to a sink that holds the dying worker until released.
    let (told, told_rx) = mpsc::channel();
    let told = Mutex::new(told);
    let (release, released) = mpsc::channel::<()>();
    let released = Mutex::new(released);
    let b = f.photo("b.raw");
    {
        let mut queue = f.service.queue();
        let identity = crate::storage::Identity::read(&b).unwrap();
        queue.pending.push_back(Job {
            key: key_of(&identity, Provider::Cpu),
            provider: Provider::Cpu,
            identity,
            path: b,
            region: None,
            develop: vec![(
                plain.clone(),
                Arc::new(AtomicBool::new(true)),
                Arc::new(move |u: Update| {
                    if matches!(u.state, State::Failed(_)) {
                        told.lock().unwrap().send(()).unwrap();
                        let _ = released.lock().unwrap().recv();
                    }
                }),
            )],
            waiters: 0,
        });
        queue.unload = true;
    }
    f.service.shared.changed.notify_all();
    told_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("B's failure is told");
    let (c, _c) = f.request(&f.photo("c.raw"), &plain, None);
    release.send(()).unwrap();
    let states = until_final(&c);
    assert!(matches!(states.last(), Some(State::Ready(_))), "{states:?}");
}

/// A result is filed under the file's identity, the model and its assembly version,
/// the provider and the build, in the form the results already on disk have.
#[test]
fn a_result_key_keeps_its_form() {
    let id = crate::storage::Identity {
        size: 12,
        modified_ns: 34,
        prefix_hash: "ab".into(),
    };
    assert_eq!(
        key_of(&id, Provider::CoreML),
        format!(
            "ab-12-34-{}-a{}-CoreML-{}",
            RAWNIND.model.sha256,
            RAWNIND.version,
            rawmakase_decode::decode_cache::build_identity()
        )
    );
}
