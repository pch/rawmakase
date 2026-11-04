//! AI Denoise's service: one queue and one worker thread for every caller (Develop,
//! export and the CLI), the model loaded once, and the results' disk cache
//! (docs/rendering-quality.md#ai-denoise). Export reaches it as a
//! [`rawmakase_decode::ai_denoise::AiBase`]; Develop asks for a photo with
//! [`Service::request`] and receives its progress, a partial result after the tiles
//! covering the 100% view, and the final result as [`Update`]s.
//!
//! A job's inputs are captured once, as its [`key_of`]: the file's identity, the model's
//! digest, the adapter version, the provider and the build, which name its cache entry
//! too. A request for a key already queued or running waits for that job. Shutdown
//! follows docs/shutdown.md: [`Service::stop`] raises the stop flag and the running
//! run's cancel flag (ONNX Runtime terminates the run within one poll), and hands the
//! thread to the shared deadline; the thread owns its ONNX session.
use crate::camera_data::CameraImage;
use crate::develop::AiSource;
use anyhow::{Result, anyhow, bail};
use rawmakase_cancel::Cancel;
use rawmakase_decode::ai_cache::{AiCache, AiEntry};
use rawmakase_decode::ai_denoise::{AiBase, ModelOutput, PackedBayer, Unsupported, assemble};
use rawmakase_inference::denoise::{GainReference, Grid, Output, TileModel, fit_gain, run};
use rawmakase_inference::{InferenceError, Provider, RAWNIND};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// What Develop hears about the photo it asked for.
pub(crate) struct Update {
    /// The plain image the request was made for: an update for another image (the photo
    /// was replaced, or another one opened) is dropped by the receiver.
    pub plain: std::sync::Weak<CameraImage>,
    pub state: State,
}
#[derive(Clone)]
pub(crate) enum State {
    /// Waiting for another job.
    Queued,
    /// Running: the share of tiles done.
    Running(f32),
    /// The tiles covering the 100% view are done: the rest is the plain image.
    Partial(AiSource),
    Ready(AiSource),
    /// Why there is no result (an unsupported sensor, no model, a failed run).
    Failed(String),
}
impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queued => f.write_str("Queued"),
            Self::Running(p) => write!(f, "Running({p:.2})"),
            Self::Partial(_) => f.write_str("Partial"),
            Self::Ready(_) => f.write_str("Ready"),
            Self::Failed(why) => write!(f, "Failed({why})"),
        }
    }
}

/// Where Develop's updates go.
pub(crate) type Sink = Arc<dyn Fn(Update) + Send + Sync>;
/// Told the path of a photo whose result was just stored.
pub(crate) type Stored = Arc<dyn Fn(&Path) + Send + Sync>;
/// Opens a photo's mosaic for the model (the RAW decoders in the app, a stand-in in tests).
pub(crate) type Source = Arc<dyn Fn(&Path, &AtomicBool) -> Result<PackedBayer> + Send + Sync>;
/// Loads the model on a provider (ONNX Runtime in the app, a stand-in in tests).
pub(crate) type Loader =
    Arc<dyn Fn(&Path, Provider) -> Result<Box<dyn TileModel + Send>, String> + Send + Sync>;

/// One Develop request: the photo, its plain image, the 100% region shown (sensor
/// pixels) and whether Develop still wants it (cleared when the switch goes off or the
/// photo closes: a queued job is then dropped; a running one finishes and stores).
pub(crate) struct Request {
    pub path: PathBuf,
    /// The file as it was before `plain` was decoded from it: a file changed since is
    /// refused, never denoised into another file's image.
    pub file: crate::storage::Identity,
    pub plain: Arc<CameraImage>,
    pub region: Option<[u32; 4]>,
    pub wanted: Arc<AtomicBool>,
    pub sink: Sink,
}

/// The cache key of `path` with the model run on `provider`: everything the result
/// depends on, captured once.
#[cfg(test)]
pub(crate) fn key(path: &Path, provider: Provider) -> Result<String> {
    Ok(key_of(&crate::storage::Identity::read(path)?, provider))
}
/// The key of a file whose identity is `id`, run on `provider`.
fn key_of(id: &crate::storage::Identity, provider: Provider) -> String {
    format!(
        "{}-{}-{}-{}-a{}-{}-{}",
        id.prefix_hash,
        id.size,
        id.modified_ns,
        RAWNIND.model.sha256,
        RAWNIND.version,
        provider.name(),
        rawmakase_decode::decode_cache::build_identity(),
    )
}

/// The installed model, as a Library preview made from its result names it: a result
/// of another model makes another preview.
pub(crate) fn model_tag() -> String {
    format!("{}-a{}", &RAWNIND.model.sha256[..16], RAWNIND.version)
}

/// The key of a finished result of the installed model for the file `file` names, run
/// on any provider, when `cache` has one: what a Library preview of the photo shows.
pub(crate) fn finished(cache: &AiCache, file: &crate::storage::Identity) -> Option<String> {
    [Provider::Cpu, Provider::Cuda, Provider::CoreML]
        .into_iter()
        .map(|provider| key_of(file, provider))
        .find(|key| cache.contains(key))
}

/// A finished result the Library's Loupe shows: `key`'s entry in `cache`, assembled
/// over `plain`, the image it was found for.
pub(crate) struct CachedLoad {
    cache: AiCache,
    key: String,
    plain: Arc<CameraImage>,
    /// Set once the photo is left or another load replaces this one.
    cancel: Arc<AtomicBool>,
}

/// The worker that loads the Loupe's finished results, one at a time, the latest
/// first: a load asked for meanwhile replaces the one waiting. It hands each result
/// over as Develop's final result is (`Event::AiDenoise`), and stops between loads at
/// exit; a cancelled load stops between its stages and sends nothing.
pub(super) fn cached_loader(
    tx: std::sync::mpsc::Sender<super::worker::Event>,
    ctx: eframe::egui::Context,
) -> super::worker::Latest<CachedLoad> {
    super::worker::Latest::new(move |job: CachedLoad| {
        let cancelled = || job.cancel.load(Ordering::Relaxed);
        if cancelled() {
            return;
        }
        let Some(entry) = job.cache.load(&job.key) else {
            return;
        };
        if cancelled() {
            return;
        }
        let output = ModelOutput {
            layout: entry.layout,
            rgb: &entry.rgb,
            done: None,
            clipped: &entry.clipped,
        };
        let source = assemble(&output, &job.plain, &job.cancel)
            .and_then(|image| AiSource::recover(image, Cancel::new(&job.cancel)));
        if let Ok(source) = source
            && !cancelled()
        {
            let _ = tx.send(super::worker::Event::AiDenoise(Box::new(Update {
                plain: Arc::downgrade(&job.plain),
                state: State::Ready(source),
            })));
            ctx.request_repaint();
        }
    })
}

/// `sink`, handed a result (partial or final) only while the file at `path` is still
/// `identity`, else told why not. Called by the worker, so the file is read there.
fn checked(sink: Sink, path: PathBuf, identity: crate::storage::Identity) -> Sink {
    Arc::new(move |update: Update| {
        let result = matches!(update.state, State::Partial(_) | State::Ready(_));
        if result && crate::storage::Identity::read(&path).ok().as_ref() != Some(&identity) {
            sink(Update {
                plain: update.plain,
                state: State::Failed(CHANGED_SINCE_OPENED.into()),
            });
        } else {
            sink(update);
        }
    })
}

/// Why a job ends when its run, or the worker, panicked.
const STOPPED_UNEXPECTEDLY: &str = "AI Denoise stopped unexpectedly";

/// Hands `update` to `sink`; a sink that panics loses its update, and nothing else.
fn deliver(sink: &Sink, update: Update) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(update)));
}

/// Why a photo whose file changed since it was decoded gets no result.
const CHANGED_SINCE_OPENED: &str =
    "The photo's file changed since it was opened; open it again to use AI Denoise.";

/// A request's inputs, captured once when it is made: the file's identity, the provider
/// and the key they make. The job runs only on that file (checked when it starts and
/// when it ends) and stores its result under that key.
#[derive(Clone)]
struct Keyed {
    key: String,
    provider: Provider,
    identity: crate::storage::Identity,
}

struct Job {
    key: String,
    provider: Provider,
    identity: crate::storage::Identity,
    path: PathBuf,
    region: Option<[u32; 4]>,
    /// Develop's requests for this job: each gets the updates while it wants them.
    develop: Vec<(Arc<CameraImage>, Arc<AtomicBool>, Sink)>,
    /// Waiters (export) for this key.
    waiters: usize,
}

/// How a job ended, for the waiters of its key.
#[derive(Clone)]
enum Outcome {
    Done(Arc<AiEntry>),
    Failed(String),
    /// Retried under another key (on the CPU after the GPU failed).
    Moved(String),
}

#[derive(Default)]
struct Queue {
    pending: VecDeque<Job>,
    /// The running job's key and its cancel flag.
    running: Option<(String, Arc<AtomicBool>)>,
    /// The worker has a job, from taking it until its result is handed over (which
    /// goes on after `running` is cleared).
    working: bool,
    /// Develop requests that joined the running job.
    joined: Vec<(Arc<CameraImage>, Arc<AtomicBool>, Sink)>,
    outcomes: HashMap<String, Outcome>,
    /// Export waiters per key: a finished result is kept in memory only for them.
    waiting: HashMap<String, usize>,
    /// The GPU produced a non-finite value: not used again in this session.
    gpu_off: bool,
    /// The worker is to drop its loaded models (the model is being removed).
    unload: bool,
    /// A worker thread serves the queue. Cleared by a worker leaving it, under the
    /// same lock as it takes the last of the queue, so a job queued after has a new
    /// worker started for it rather than wait for one that is gone.
    served: bool,
    stopped: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
    cache: AiCache,
    /// Told the photo's path each time a result is stored, whoever asked for it.
    stored: Mutex<Option<Stored>>,
    /// The installed model's folder, `None` when it is not installed.
    model: Mutex<Option<PathBuf>>,
    provider: Provider,
    source: Source,
    loader: Loader,
}

/// Clears [`Queue::working`] once a job is finished with, also when finishing panics.
struct Working<'a>(&'a Service);
impl Drop for Working<'_> {
    fn drop(&mut self) {
        self.0.queue().working = false;
    }
}

/// The service; clones share it.
#[derive(Clone)]
pub(crate) struct Service {
    shared: Arc<Shared>,
    thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
}

/// The provider AI Denoise asks for: CoreML's GPU on Apple Silicon (measured equal to
/// the CPU, docs/rendering-quality.md#ai-denoise), the CPU elsewhere.
pub(crate) fn preferred_provider() -> Provider {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Provider::CoreML
    } else {
        Provider::Cpu
    }
}

/// Opens the RAW at `path` and packs its mosaic.
fn open_mosaic(path: &Path, cancel: &AtomicBool) -> Result<PackedBayer> {
    if !crate::storage::is_raw(path) {
        return Err(anyhow::Error::new(Unsupported(
            "AI Denoise works on RAW files only.",
        )));
    }
    crate::photo::open(path)?.packed_bayer(Cancel::new(cancel))
}

/// ONNX Runtime threads: fewer than rendering's, so a running job leaves the UI and
/// renders room.
fn threads() -> usize {
    rawmakase_inference::runtime::default_threads()
        .saturating_sub(2)
        .max(2)
}

fn load_model(dir: &Path, provider: Provider) -> Result<Box<dyn TileModel + Send>, String> {
    let bundled = super::subject_mask::runtime_dir();
    let runtime_library = rawmakase_inference::runtime::runtime_file_names()
        .iter()
        .map(|name| bundled.join(name))
        .find(|path| path.is_file());
    rawmakase_inference::Denoiser::load_with(
        dir,
        &rawmakase_inference::LoadOptions {
            runtime_library,
            threads: Some(threads()),
            provider: Some(provider),
        },
    )
    .map(|d| Box::new(d) as Box<dyn TileModel + Send>)
    .map_err(|e| e.to_string())
}

impl Service {
    /// The app's service: the RAW decoders, ONNX Runtime and `cache`.
    pub(crate) fn new(cache: AiCache) -> Self {
        Self::with(
            cache,
            preferred_provider(),
            Arc::new(open_mosaic),
            Arc::new(load_model),
        )
    }
    pub(crate) fn with(cache: AiCache, provider: Provider, source: Source, loader: Loader) -> Self {
        Self {
            shared: Arc::new(Shared {
                queue: Mutex::default(),
                changed: Condvar::new(),
                cache,
                stored: Mutex::new(None),
                model: Mutex::new(None),
                provider,
                source,
                loader,
            }),
            thread: Arc::default(),
        }
    }
    /// The installed model's folder, or `None`: jobs then fail with the reason.
    pub(crate) fn set_model(&self, dir: Option<PathBuf>) {
        *self.shared.model.lock().unwrap_or_else(|e| e.into_inner()) = dir;
    }
    pub(crate) fn cache(&self) -> &AiCache {
        &self.shared.cache
    }
    /// Whether a job is queued, running or handing over its result. A running job
    /// finishes even once nobody wants it, so this outlasts the photo it was asked for.
    pub(crate) fn busy(&self) -> bool {
        let queue = self.queue();
        queue.working || !queue.pending.is_empty()
    }
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.shared.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn provider(&self, queue: &Queue) -> Provider {
        if queue.gpu_off {
            Provider::Cpu
        } else {
            self.shared.provider
        }
    }
    /// The inputs of a request for the file `identity`, captured once, with the
    /// provider in use. Nothing is read from the file: the worker checks it before and
    /// after the job. Returns the queue locked.
    fn keyed(
        &self,
        identity: crate::storage::Identity,
    ) -> Result<(Keyed, std::sync::MutexGuard<'_, Queue>)> {
        let queue = self.queue();
        ensure_running(&queue)?;
        let provider = self.provider(&queue);
        let key = key_of(&identity, provider);
        Ok((
            Keyed {
                key,
                provider,
                identity,
            },
            queue,
        ))
    }
    /// Starts the worker if it is not running. When the thread cannot be started, every
    /// queued job fails with the reason: nothing would ever run them.
    fn ensure_worker(&self) {
        {
            let mut queue = self.queue();
            if queue.served || queue.stopped {
                return;
            }
            queue.served = true;
        }
        let mut thread = self.thread.lock().unwrap_or_else(|e| e.into_inner());
        let service = self.clone();
        match std::thread::Builder::new()
            .name("ai-denoise".into())
            .spawn(move || {
                let lived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    service.work();
                }));
                if lived.is_err() {
                    service.worker_lost();
                } else {
                    service.queue().served = false;
                }
            }) {
            // A worker that left the queue may still be ending: it is let go, as it
            // runs nothing more.
            Ok(handle) => *thread = Some(handle),
            Err(e) => {
                *thread = None;
                drop(thread);
                self.queue().served = false;
                self.fail_pending(&format!("AI Denoise could not start: {e}"));
            }
        }
    }
    /// Fails every queued job with `why`: Develop is told, export's waiters woken.
    fn fail_pending(&self, why: &str) {
        let jobs: Vec<Job> = {
            let mut queue = self.queue();
            let jobs: Vec<Job> = queue.pending.drain(..).collect();
            for job in &jobs {
                queue
                    .outcomes
                    .insert(job.key.clone(), Outcome::Failed(why.into()));
            }
            jobs
        };
        self.shared.changed.notify_all();
        for job in jobs {
            for (plain, wanted, sink) in job.develop {
                if wanted.load(Ordering::Relaxed) {
                    deliver(
                        &sink,
                        Update {
                            plain: Arc::downgrade(&plain),
                            state: State::Failed(why.into()),
                        },
                    );
                }
            }
        }
    }
    /// The worker thread died outside a job's own guard: the job it held and every
    /// queued one fail, so no Develop request or export waits for a worker that is
    /// gone. The next request starts a new worker.
    fn worker_lost(&self) {
        // The queue is left in one step: a request after it starts a new worker.
        let (joined, jobs) = {
            let mut queue = self.queue();
            if let Some((key, _)) = queue.running.take() {
                queue
                    .outcomes
                    .entry(key)
                    .or_insert_with(|| Outcome::Failed(STOPPED_UNEXPECTEDLY.into()));
            }
            let jobs: Vec<Job> = queue.pending.drain(..).collect();
            for job in &jobs {
                queue.outcomes.insert(
                    job.key.clone(),
                    Outcome::Failed(STOPPED_UNEXPECTEDLY.into()),
                );
            }
            queue.working = false;
            queue.served = false;
            (std::mem::take(&mut queue.joined), jobs)
        };
        self.shared.changed.notify_all();
        let develop = jobs.into_iter().flat_map(|job| job.develop);
        for (plain, wanted, sink) in joined.into_iter().chain(develop) {
            if wanted.load(Ordering::Relaxed) {
                deliver(
                    &sink,
                    Update {
                        plain: Arc::downgrade(&plain),
                        state: State::Failed(STOPPED_UNEXPECTEDLY.into()),
                    },
                );
            }
        }
    }
    /// Tells `stored` the path of each photo whose result is stored from now on, so
    /// previews shown without one can show it.
    pub(crate) fn on_stored(&self, stored: Stored) {
        *self.shared.stored.lock().unwrap_or_else(|e| e.into_inner()) = Some(stored);
    }
    /// Asks for `request`'s photo in Develop. Its updates follow on its sink; a result
    /// already cached arrives without running anything.
    pub(crate) fn request(&self, request: Request) {
        let Request {
            path,
            file,
            plain,
            region,
            wanted,
            sink,
        } = request;
        if self.queue().stopped {
            return;
        }
        let (
            Keyed {
                key,
                provider,
                identity,
            },
            mut queue,
        ) = match self.keyed(file) {
            Ok(keyed) => keyed,
            // Only refused when quitting.
            Err(_) => return,
        };
        let running = queue.running.as_ref().is_some_and(|(k, _)| *k == key);
        if running || queue.pending.iter().any(|j| j.key == key) {
            // Joined to a job of the same file that may run under another path (a
            // hard link): its results come only while this request's own file is
            // still the one it asked for.
            let entry = (plain.clone(), wanted, checked(sink.clone(), path, identity));
            if running {
                queue.joined.push(entry);
                return;
            }
            let job = queue
                .pending
                .iter_mut()
                .find(|j| j.key == key)
                .expect("found");
            job.develop.push(entry);
            // The photo on screen goes first, with its region.
            job.region = region.or(job.region);
            if let Some(i) = queue.pending.iter().position(|j| j.key == key) {
                let job = queue.pending.remove(i).expect("found");
                queue.pending.push_front(job);
            }
        } else {
            queue.pending.push_front(Job {
                key,
                provider,
                identity,
                path,
                region,
                develop: vec![(plain.clone(), wanted, sink.clone())],
                waiters: 0,
            });
        }
        let running = queue.running.is_some();
        drop(queue);
        if running {
            sink(Update {
                plain: Arc::downgrade(&plain),
                state: State::Queued,
            });
        }
        self.shared.changed.notify_all();
        self.ensure_worker();
    }

    /// The worker: one job at a time until stopped.
    fn work(&self) {
        let mut models: HashMap<(PathBuf, Provider), Box<dyn TileModel + Send>> = HashMap::new();
        loop {
            let job = {
                let mut queue = self.queue();
                loop {
                    if queue.stopped {
                        return;
                    }
                    if queue.unload {
                        models.clear();
                        queue.unload = false;
                        self.shared.changed.notify_all();
                    }
                    // A Develop job nobody wants any more, and no export waits for, goes.
                    queue.pending.retain_mut(|job| {
                        job.develop
                            .retain(|(_, wanted, _)| wanted.load(Ordering::Relaxed));
                        !job.develop.is_empty() || job.waiters > 0
                    });
                    if let Some(job) = queue.pending.pop_front() {
                        let cancel = Arc::new(AtomicBool::new(false));
                        queue.running = Some((job.key.clone(), cancel));
                        queue.working = true;
                        break job;
                    }
                    queue = self
                        .shared
                        .changed
                        .wait(queue)
                        .unwrap_or_else(|e| e.into_inner());
                }
            };
            let cancel = self
                .queue()
                .running
                .as_ref()
                .map(|(_, c)| c.clone())
                .unwrap_or_default();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.run(&job, &cancel, &mut models)
            }))
            .unwrap_or_else(|_| Err(anyhow!(STOPPED_UNEXPECTEDLY)));
            let _working = Working(self);
            self.finish(job, result);
        }
    }

    /// Develop's requests for the running job, `job`'s own and those that joined it.
    fn listeners(&self, job: &Job) -> Vec<(Arc<CameraImage>, Sink)> {
        let queue = self.queue();
        job.develop
            .iter()
            .chain(&queue.joined)
            .filter(|(_, wanted, _)| wanted.load(Ordering::Relaxed))
            .map(|(plain, _, sink)| (plain.clone(), sink.clone()))
            .collect()
    }
    fn tell(&self, job: &Job, state: impl Fn(&Arc<CameraImage>) -> Option<State>) {
        for (plain, sink) in self.listeners(job) {
            if let Some(state) = state(&plain) {
                sink(Update {
                    plain: Arc::downgrade(&plain),
                    state,
                });
            }
        }
    }

    /// Runs `job`: from the cache, or the model over every tile.
    fn run(
        &self,
        job: &Job,
        cancel: &AtomicBool,
        models: &mut HashMap<(PathBuf, Provider), Box<dyn TileModel + Send>>,
    ) -> Result<Outcome> {
        // The file it was asked for, from the cache too.
        if crate::storage::Identity::read(&job.path).ok().as_ref() != Some(&job.identity) {
            bail!("The photo's file changed since AI Denoise was asked for it.");
        }
        if let Some(entry) = self.shared.cache.load(&job.key) {
            // Checked again: the file may have been replaced during a slow read.
            if crate::storage::Identity::read(&job.path).ok().as_ref() != Some(&job.identity) {
                bail!("The photo's file changed since AI Denoise was asked for it.");
            }
            return Ok(Outcome::Done(Arc::new(entry)));
        }
        let model_dir = self
            .shared
            .model
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| anyhow!("The AI Denoise model is not installed."))?;
        self.tell(job, |_| Some(State::Running(0.)));
        let packed = (self.shared.source)(&job.path, cancel)?;
        let layout = packed.layout;
        let model = match models.entry((model_dir.clone(), job.provider)) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let model = (self.shared.loader)(&model_dir, job.provider)
                    .map_err(|why| anyhow!("The AI Denoise model could not be loaded: {why}"))?;
                e.insert(model)
            }
        };
        let grid = Grid::new(layout.packed_width, layout.packed_height).with_origin(layout.pad);
        let region = job.region.map(|[x, y, w, h]| {
            let [ax, ay] = layout.pad;
            let x0 = (x as usize + ax) / 2;
            let y0 = (y as usize + ay) / 2;
            [
                x0,
                y0,
                (w as usize).div_ceil(2).max(1),
                (h as usize).div_ceil(2).max(1),
            ]
        });
        let (tiles, first) = grid.order(region);
        let total = tiles.len();
        let mut out = Output::new(&grid);
        let reference = GainReference {
            width: layout.packed_width,
            height: layout.packed_height,
            rgb: &packed.reference,
            clipped: &packed.clipped,
        };
        let planes = rawmakase_inference::denoise::Planes {
            width: layout.packed_width,
            height: layout.packed_height,
            data: &packed.planes,
        };
        let mut last = 0usize;
        run(
            planes,
            &grid,
            &tiles,
            model.as_mut(),
            &mut out,
            cancel,
            |done, out| {
                if first > 0 && done == first && done < total {
                    // The 100% view's tiles: one partial result, at a provisional gain.
                    if let Ok(gain) = fit_gain(out, &reference) {
                        let done_mask: Vec<bool> = (0..layout.packed_height)
                            .flat_map(|y| (0..layout.packed_width).map(move |x| (x, y)))
                            .map(|(x, y)| out.is_done(x, y))
                            .collect();
                        let mut rgb: Vec<f32> = out.data.iter().map(|v| v * gain).collect();
                        rawmakase_inference::denoise::correct_dark(&mut rgb, &reference);
                        self.tell(job, |plain| {
                            let image = assemble(
                                &ModelOutput {
                                    layout,
                                    rgb: &rgb,
                                    done: Some(&done_mask),
                                    clipped: &packed.clipped,
                                },
                                plain,
                                Cancel::new(cancel),
                            )
                            .ok()?;
                            AiSource::recover(image, Cancel::new(cancel))
                                .ok()
                                .map(State::Partial)
                        });
                    }
                }
                // Progress at most every 5%.
                if done * 20 / total != last * 20 / total {
                    last = done;
                    self.tell(job, |_| Some(State::Running(done as f32 / total as f32)));
                }
            },
        )
        .map_err(|e| match e {
            InferenceError::OutputInvalid(why) if job.provider != Provider::Cpu => {
                anyhow::Error::new(GpuFailed(why))
            }
            e => anyhow!("AI Denoise failed: {e}"),
        })?;
        let gain = fit_gain(&out, &reference).map_err(|why| anyhow!("AI Denoise failed: {why}"))?;
        let mut rgb = out.data;
        for v in &mut rgb {
            *v *= gain;
        }
        rawmakase_inference::denoise::correct_dark(&mut rgb, &reference);
        // Rejected when the file or the model changed during the run.
        let now = crate::storage::Identity::read(&job.path).ok();
        if now.as_ref() != Some(&job.identity) {
            bail!("The photo's file changed while AI Denoise ran.");
        }
        if self
            .shared
            .model
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            != Some(&model_dir)
            || !model_dir.join(RAWNIND.model.name).is_file()
        {
            bail!("The AI Denoise model was removed while it ran.");
        }
        let entry = AiEntry {
            layout,
            gain,
            rgb,
            clipped: packed.clipped,
        };
        // A result that cannot be stored at half precision is not one.
        match self.shared.cache.store(&job.key, &entry) {
            Ok(()) => {
                let stored = self
                    .shared
                    .stored
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if let Some(stored) = stored {
                    stored(&job.path);
                }
            }
            Err(e) if e.to_string().contains("half precision") => return Err(e),
            Err(e) => eprintln!("Could not cache the AI Denoise result: {e:#}"),
        }
        Ok(Outcome::Done(Arc::new(entry)))
    }

    /// Ends `job`: Develop gets its result, export's waiters are woken.
    fn finish(&self, job: Job, result: Result<Outcome>) {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(e) if e.downcast_ref::<GpuFailed>().is_some() => {
                // Retried on the CPU under the CPU's key; the GPU is not used again.
                let mut queue = self.queue();
                queue.gpu_off = true;
                let joined = std::mem::take(&mut queue.joined);
                queue.running = None;
                let cpu = key_of(&job.identity, Provider::Cpu);
                queue
                    .outcomes
                    .insert(job.key.clone(), Outcome::Moved(cpu.clone()));
                let mut develop = job.develop;
                develop.extend(joined);
                queue.pending.push_front(Job {
                    key: cpu,
                    provider: Provider::Cpu,
                    develop,
                    ..job
                });
                // Queued jobs move to the CPU too, under the CPU's keys (their waiters
                // follow), merged where two now name the same result.
                let pending: Vec<Job> = queue.pending.drain(..).collect();
                for mut job in pending {
                    if job.provider != Provider::Cpu {
                        let cpu = key_of(&job.identity, Provider::Cpu);
                        queue
                            .outcomes
                            .insert(job.key.clone(), Outcome::Moved(cpu.clone()));
                        job.key = cpu;
                        job.provider = Provider::Cpu;
                    }
                    if let Some(same) = queue.pending.iter_mut().find(|j| j.key == job.key) {
                        same.develop.extend(job.develop);
                        same.waiters += job.waiters;
                        same.region = same.region.or(job.region);
                    } else {
                        queue.pending.push_back(job);
                    }
                }
                drop(queue);
                self.shared.changed.notify_all();
                return;
            }
            Err(e) => Outcome::Failed(
                e.downcast_ref::<Unsupported>()
                    .map_or_else(|| format!("{e:#}"), |u| u.0.to_string()),
            ),
        };
        // The job ends here, in one step: every request accepted for it so far gets its
        // outcome below; a request after this starts a job of its own (a cache hit).
        let (cancelled, joined) = {
            let mut queue = self.queue();
            let cancelled = queue.stopped
                || queue
                    .running
                    .as_ref()
                    .is_some_and(|(_, c)| c.load(Ordering::Relaxed));
            let joined = std::mem::take(&mut queue.joined);
            queue.running = None;
            // A result is kept in memory only while exports wait for it (the cache
            // keeps it otherwise); failures are small.
            let waited = queue.waiting.get(&job.key).is_some_and(|n| *n > 0);
            if waited || !matches!(outcome, Outcome::Done(_)) {
                queue.outcomes.insert(job.key.clone(), outcome.clone());
            }
            (cancelled, joined)
        };
        self.shared.changed.notify_all();
        let listeners: Vec<(Arc<CameraImage>, Sink)> = job
            .develop
            .iter()
            .chain(&joined)
            .filter(|(_, wanted, _)| wanted.load(Ordering::Relaxed))
            .map(|(plain, _, sink)| (plain.clone(), sink.clone()))
            .collect();
        // Each listener's result is made and handed over on its own: a panic there
        // fails that listener alone, never the jobs queued behind this one.
        let tell = |state: &dyn Fn(&Arc<CameraImage>) -> State| {
            for (plain, sink) in &listeners {
                let state = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state(plain)))
                    .unwrap_or_else(|_| State::Failed(STOPPED_UNEXPECTEDLY.into()));
                deliver(
                    sink,
                    Update {
                        plain: Arc::downgrade(plain),
                        state,
                    },
                );
            }
        };
        match &outcome {
            Outcome::Done(entry) => tell(&|plain| {
                let image = assemble(
                    &ModelOutput {
                        layout: entry.layout,
                        rgb: &entry.rgb,
                        done: None,
                        clipped: &entry.clipped,
                    },
                    plain,
                    Cancel::NEVER,
                );
                match image.and_then(|i| AiSource::recover(i, Cancel::NEVER)) {
                    Ok(source) => State::Ready(source),
                    Err(e) => State::Failed(format!("{e:#}")),
                }
            }),
            Outcome::Failed(why) if !cancelled => tell(&|_| State::Failed(why.clone())),
            _ => {}
        }
    }

    /// Lets go of the model before it is removed: no job uses it from now on, the
    /// running one is cancelled, and the worker drops its ONNX sessions between jobs.
    /// Waits for that up to five seconds (a run stops within one cancel poll).
    pub(crate) fn unload(&self) {
        self.set_model(None);
        let alive = self
            .thread
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|t| !t.is_finished());
        let mut queue = self.queue();
        if !alive {
            return;
        }
        queue.unload = true;
        if let Some((_, cancel)) = &queue.running {
            cancel.store(true, Ordering::Relaxed);
        }
        self.shared.changed.notify_all();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while queue.unload && !queue.stopped && std::time::Instant::now() < until {
            queue = self
                .shared
                .changed
                .wait_timeout(queue, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
    /// Stops the worker for good: the stop flag and the running run's cancel flag are
    /// raised, and its thread is handed to the shared deadline.
    pub(in crate::app) fn stop(&self) -> super::task::Stopping {
        {
            let mut queue = self.queue();
            queue.stopped = true;
            if let Some((_, cancel)) = &queue.running {
                cancel.store(true, Ordering::Relaxed);
            }
            queue.pending.clear();
        }
        self.shared.changed.notify_all();
        super::task::Stopping::new(self.thread.lock().unwrap_or_else(|e| e.into_inner()).take())
    }

    /// The result entry for `path`, computed if need be, for a caller that blocks
    /// (export): coalesced with a queued or running job of the same key.
    fn entry(
        &self,
        path: &Path,
        file: &crate::storage::Identity,
        cancel: &AtomicBool,
    ) -> Result<Arc<AiEntry>> {
        // Read without the queue's lock: a stalled drive must not hold up quitting.
        if crate::storage::Identity::read(path)? != *file {
            bail!("{CHANGED_SINCE_OPENED}");
        }
        let (keyed, queue) = self.keyed(file.clone())?;
        drop(queue);
        // Read without the queue's lock; the file checked again after a slow read.
        if let Some(entry) = self.shared.cache.load(&keyed.key) {
            if crate::storage::Identity::read(path).ok().as_ref() != Some(&keyed.identity) {
                bail!("{CHANGED_SINCE_OPENED}");
            }
            return Ok(Arc::new(entry));
        }
        let mut queue = self.queue();
        ensure_running(&queue)?;
        // The GPU switched off meanwhile: the CPU's key, for the same file.
        let provider = self.provider(&queue);
        let keyed = if provider == keyed.provider {
            keyed
        } else {
            Keyed {
                key: key_of(&keyed.identity, provider),
                provider,
                identity: keyed.identity,
            }
        };
        let mut key = keyed.key.clone();
        // A failure of an earlier attempt is not this one's.
        if matches!(queue.outcomes.get(&key), Some(Outcome::Failed(_))) {
            queue.outcomes.remove(&key);
        }
        let mut waiting = Waiting::new(self, &mut queue, key.clone());
        let job = |key: &str| Job {
            key: key.to_string(),
            provider: keyed.provider,
            identity: keyed.identity.clone(),
            path: path.to_path_buf(),
            region: None,
            develop: Vec::new(),
            waiters: 1,
        };
        let running = queue.running.as_ref().is_some_and(|(k, _)| *k == key);
        if !running {
            if let Some(pending) = queue.pending.iter_mut().find(|j| j.key == key) {
                pending.waiters += 1;
            } else {
                queue.pending.push_back(job(&key));
            }
        }
        drop(queue);
        self.shared.changed.notify_all();
        self.ensure_worker();
        let mut queue = self.queue();
        loop {
            ensure_running(&queue)?;
            if cancel.load(Ordering::Relaxed) {
                bail!("Export cancelled");
            }
            match queue.outcomes.get(&key).cloned() {
                Some(Outcome::Done(entry)) => {
                    waiting.leave(&mut queue);
                    return Ok(entry);
                }
                Some(Outcome::Failed(why)) => bail!("{why}"),
                Some(Outcome::Moved(next)) => {
                    waiting.leave(&mut queue);
                    key = next;
                    waiting = Waiting::new(self, &mut queue, key.clone());
                }
                None => {
                    // Not in memory (kept only while waited for, and this waiter came
                    // after): the cache has it, or it runs again.
                    let known = queue.running.as_ref().is_some_and(|(k, _)| *k == key)
                        || queue.pending.iter().any(|j| j.key == key);
                    if !known {
                        drop(queue);
                        if let Some(entry) = self.shared.cache.load(&key) {
                            if crate::storage::Identity::read(path).ok().as_ref()
                                != Some(&keyed.identity)
                            {
                                bail!("{CHANGED_SINCE_OPENED}");
                            }
                            return Ok(Arc::new(entry));
                        }
                        queue = self.queue();
                        ensure_running(&queue)?;
                        let known = queue.running.as_ref().is_some_and(|(k, _)| *k == key)
                            || queue.pending.iter().any(|j| j.key == key)
                            || queue.outcomes.contains_key(&key);
                        if !known {
                            let mut retry = job(&key);
                            retry.provider = if key == keyed.key {
                                keyed.provider
                            } else {
                                Provider::Cpu
                            };
                            queue.pending.push_back(retry);
                            drop(queue);
                            self.shared.changed.notify_all();
                            self.ensure_worker();
                            queue = self.queue();
                        }
                        continue;
                    }
                }
            }
            queue = self
                .shared
                .changed
                .wait_timeout(queue, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// One export waiting for `key`: counted while it waits, so the finished result is kept
/// in memory until the last waiter has it.
struct Waiting<'a> {
    service: &'a Service,
    key: Option<String>,
}
impl<'a> Waiting<'a> {
    fn new(service: &'a Service, queue: &mut Queue, key: String) -> Self {
        *queue.waiting.entry(key.clone()).or_default() += 1;
        Self {
            service,
            key: Some(key),
        }
    }
    fn leave(&mut self, queue: &mut Queue) {
        let Some(key) = self.key.take() else {
            return;
        };
        let left = queue.waiting.get_mut(&key).map_or(0, |n| {
            *n = n.saturating_sub(1);
            *n
        });
        if left == 0 {
            queue.waiting.remove(&key);
            if matches!(queue.outcomes.get(&key), Some(Outcome::Done(_))) {
                queue.outcomes.remove(&key);
            }
        }
    }
}
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if self.key.is_some() {
            let service = self.service;
            let mut queue = service.queue();
            self.leave(&mut queue);
        }
    }
}

fn ensure_running(queue: &Queue) -> Result<()> {
    if queue.stopped {
        bail!("RAWmakase is quitting");
    }
    Ok(())
}

/// A non-finite value from a GPU provider: the attempt is discarded and rerun on the CPU.
#[derive(Debug)]
struct GpuFailed(String);
impl std::fmt::Display for GpuFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the GPU produced an invalid result: {}", self.0)
    }
}
impl std::error::Error for GpuFailed {}

impl AiBase for Service {
    fn get(
        &self,
        path: &Path,
        file: &crate::storage::Identity,
        plain: &Arc<CameraImage>,
        cancel: &AtomicBool,
    ) -> Result<Arc<CameraImage>> {
        let entry = self.entry(path, file, cancel)?;
        assemble(
            &ModelOutput {
                layout: entry.layout,
                rgb: &entry.rgb,
                done: None,
                clipped: &entry.clipped,
            },
            plain,
            Cancel::new(cancel),
        )
        .map(Arc::new)
    }
}

/// AI Denoise for the open photo's decoded image: the request in flight, the latest
/// result (partial or final) and what the Detail panel says. Replaced with the image, and
/// dropping it tells the service the photo is no longer wanted.
#[derive(Default)]
pub(crate) struct Photo {
    /// The image asked for and whether it is still wanted.
    requested: Option<(std::sync::Weak<CameraImage>, Arc<AtomicBool>)>,
    source: Option<AiSource>,
    /// `source` is the final result, not a partial one.
    complete: bool,
    /// The last request failed: not asked again for this image (a reinstalled model
    /// or a reopened photo asks again).
    failed: bool,
    status: Option<State>,
    /// The image the Library's Loupe looked for a finished result for, found or not:
    /// not looked for again until one is stored.
    looked_up: Option<std::sync::Weak<CameraImage>>,
    /// Cancels the Loupe's load of a finished result, once the photo is left.
    loading: Option<Arc<AtomicBool>>,
}
impl Drop for Photo {
    fn drop(&mut self) {
        self.release();
    }
}
impl Photo {
    /// The latest result.
    pub(crate) fn source(&self) -> Option<&AiSource> {
        self.source.as_ref()
    }
    /// Whether [`source`](Self::source) is the final result, not a partial one.
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }
    /// What happened last: running, ready, or why there is none.
    pub(crate) fn status(&self) -> Option<&State> {
        self.status.as_ref()
    }
    /// Tells the service a queued request is no longer wanted. A result already
    /// received is kept, so switching back on shows it at once.
    fn release(&mut self) {
        if let Some((_, wanted)) = self.requested.take() {
            wanted.store(false, Ordering::Relaxed);
        }
        // A load given up before it was shown is looked for again next time.
        if let Some(cancel) = self.loading.take() {
            cancel.store(true, Ordering::Relaxed);
            self.looked_up = None;
        }
    }
    /// Whether `image` should be asked for: not a draft, not asked for already, and
    /// without a final result or a failure. A partial result switched off before the
    /// final one arrived asks again (the job finished and stored: a cache hit).
    fn needs_request(&self, image: &Arc<CameraImage>) -> bool {
        !image.fast && !self.asked_for(image) && !self.complete && !self.failed
    }
    fn asked_for(&self, image: &Arc<CameraImage>) -> bool {
        self.requested
            .as_ref()
            .is_some_and(|(asked, _)| std::ptr::eq(asked.as_ptr(), Arc::as_ptr(image)))
    }
}

impl super::Editor {
    /// Asks for the open photo's AI Denoise result when its edit renders AI Denoise and
    /// the full-size image is decoded; withdraws the request when it no longer does.
    /// `region` is the 100% view (in the render's pixels of `geometry`): its tiles run
    /// first.
    pub(super) fn request_ai_denoise(
        &mut self,
        image: &Arc<CameraImage>,
        geometry: &crate::develop::Geometry,
        region: Option<[u32; 4]>,
    ) {
        let renders = self.document.edit.recipe().ai_denoise.renders();
        if !renders {
            self.document.ai.release();
            return;
        }
        if !self.document.ai.needs_request(image) {
            return;
        }
        let Some(path) = self.document.path.clone() else {
            return;
        };
        // Without the file's identity from before decoding, nothing proves the result
        // would be of the same file: refused, as a changed file is.
        let Some(file) = self.document.image_file.clone() else {
            self.document.ai.status = Some(State::Failed(CHANGED_SINCE_OPENED.into()));
            return;
        };
        // The Library's Loupe shows a finished result, as its previews do, and never
        // runs the model: without one, the photo shows plain.
        if self.module == super::library::Module::Library {
            self.load_finished_ai(image, &file);
            return;
        }
        let model = self.selection.models.denoise.path();
        if model.is_none() {
            self.document.ai.status = Some(State::Failed(
                "Install the AI Denoise model to use it.".into(),
            ));
            return;
        }
        self.ai_denoise.set_model(model);
        let source_region = region.map(|[x, y, w, h]| {
            let (gw, gh) = (geometry.width.max(1) as f32, geometry.height.max(1) as f32);
            let corners = [(x, y), (x + w, y), (x, y + h), (x + w, y + h)]
                .map(|(u, v)| geometry.source(u as f32 / gw, v as f32 / gh));
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for [cx, cy] in corners {
                (x0, y0, x1, y1) = (x0.min(cx), y0.min(cy), x1.max(cx), y1.max(cy));
            }
            let clamp = |v: f32, max: u32| v.clamp(0., max as f32) as u32;
            let (x0, y0) = (clamp(x0, image.width), clamp(y0, image.height));
            let (x1, y1) = (clamp(x1, image.width), clamp(y1, image.height));
            [x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)]
        });
        let wanted = Arc::new(AtomicBool::new(true));
        self.document.ai.requested = Some((Arc::downgrade(image), wanted.clone()));
        self.document.ai.status = Some(State::Queued);
        let (tx, ctx) = (Mutex::new(self.tx.clone()), self.context.clone());
        self.ai_denoise.request(Request {
            path,
            file,
            plain: image.clone(),
            region: source_region,
            wanted,
            sink: Arc::new(move |update| {
                let _ = tx
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send(super::worker::Event::AiDenoise(Box::new(update)));
                ctx.request_repaint();
            }),
        });
    }
    /// For the Library's Loupe: a finished result in the cache for the file `file`
    /// names (of any provider), loaded and assembled over `image` off the UI thread,
    /// and handed over as Develop's final result is. Never runs the model; without a
    /// result the photo stays plain. Looked for once per image, until one is stored.
    fn load_finished_ai(&mut self, image: &Arc<CameraImage>, file: &crate::storage::Identity) {
        let looked = self.document.ai.looked_up.as_ref();
        if looked.is_some_and(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(image))) {
            return;
        }
        self.document.ai.looked_up = Some(Arc::downgrade(image));
        let Some(key) = finished(self.ai_denoise_cache(), file) else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(earlier) = self.document.ai.loading.replace(cancel.clone()) {
            earlier.store(true, Ordering::Relaxed);
        }
        self.ai_loupe.submit(CachedLoad {
            cache: self.ai_denoise_cache().clone(),
            key,
            plain: image.clone(),
            cancel,
        });
    }
    /// A result was stored for the file at `path`: the Library's Loupe showing it
    /// plain looks again.
    pub(super) fn ai_result_stored(&mut self, path: &Path) {
        if let Some(library) = &mut self.library {
            library.ai_result_stored(path);
        }
        if self.module == super::library::Module::Library
            && self.document.path.as_deref() == Some(path)
            && !self.document.ai.complete
        {
            self.document.ai.looked_up = None;
            self.schedule();
        }
    }
    /// The AI Denoise source a render of `image` with `setting` shows: Develop's latest
    /// result, partial or final; the Library's Loupe only a final one.
    pub(super) fn shown_ai_source(
        &self,
        image: &Arc<CameraImage>,
        setting: &crate::model::ai_denoise::AiDenoise,
    ) -> Option<crate::develop::AiSource> {
        self.document
            .ai_source(image, setting)
            .filter(|_| self.module != super::library::Module::Library || self.document.ai.complete)
    }
    /// Removes AI Denoise's model: the service lets go of it first, off the UI thread,
    /// and the photo shown asks again (and is told to install it) when next rendered.
    pub(super) fn remove_ai_denoise_model(&mut self) {
        let service = self.ai_denoise.clone();
        self.selection
            .models
            .of_mut(super::subject_mask::Pack::Denoise)
            .remove(
                move || service.unload(),
                self.tx.clone(),
                self.context.clone(),
            );
        self.document.ai = Default::default();
        self.schedule();
    }
    /// The service as exports reach it, with the installed model (or none, when every
    /// photo with AI Denoise on fails with the reason).
    pub(super) fn ai_service(&self) -> rawmakase_decode::ai_denoise::AiService {
        self.ai_denoise
            .set_model(self.selection.models.denoise.path());
        rawmakase_decode::ai_denoise::AiService(Arc::new(self.ai_denoise.clone()))
    }
    /// The results' disk cache, which follows the decode cache's folder.
    pub(super) fn ai_denoise_cache(&self) -> &AiCache {
        self.ai_denoise.cache()
    }
    /// An update from the service: kept when it is for the image on screen, which is
    /// rendered again with a new result.
    pub(super) fn ai_denoise_update(&mut self, update: Update) {
        let current = self
            .document
            .full()
            .is_some_and(|image| std::ptr::eq(update.plain.as_ptr(), Arc::as_ptr(image)));
        if !current {
            return;
        }
        // A final result is not replaced by a partial one: the Loupe's cached load
        // and a Develop run can both answer for one image.
        if matches!(update.state, State::Partial(_)) && self.document.ai.complete {
            return;
        }
        let shown = matches!(update.state, State::Partial(_) | State::Ready(_));
        if let State::Partial(source) | State::Ready(source) = &update.state {
            self.document.ai.source = Some(source.clone());
        }
        match &update.state {
            State::Ready(_) => {
                self.document.ai.complete = true;
                self.document.ai.requested = None;
            }
            State::Failed(_) => {
                self.document.ai.failed = true;
                self.document.ai.requested = None;
            }
            _ => {}
        }
        // A partial result is not shown once the final one will not come: the photo
        // renders plain, as without a result.
        let dropped = matches!(update.state, State::Failed(_))
            && !self.document.ai.complete
            && self.document.ai.source.take().is_some();
        self.document.ai.status = Some(update.state);
        if (shown || dropped) && self.document.edit.recipe().ai_denoise.renders() {
            self.schedule();
        }
    }
}

/// The user's AI Denoise cache as the app keeps it: in the folder `session`'s
/// Preferences chose for decoded images, or the default one.
fn user_cache(session: &super::session::Session) -> AiCache {
    let cache = AiCache::default();
    cache.set_folder(session.decoded_cache_folder.as_deref());
    cache
}

/// AI Denoise for command-line renders (`render`, `render-batch`): one service for the
/// whole command, so the model loads once, with the user's cache where the app keeps
/// it. Its worker is stopped when this is dropped, waiting at most three seconds.
pub struct CommandLine {
    service: Service,
}
impl CommandLine {
    pub fn new() -> Self {
        Self::with(Service::new(user_cache(&super::session::load_session())))
    }
    fn with(service: Service) -> Self {
        Self { service }
    }
    /// The AI Denoise source for a render of `path` (its full-size plain `image`,
    /// decoded from the file as `file` identifies it, read before decoding) when
    /// `recipe` renders AI Denoise, from the installed model, waiting for it. Never the
    /// plain image in its place: without one, the error says why.
    pub fn source(
        &self,
        path: &Path,
        file: &crate::storage::Identity,
        image: &Arc<CameraImage>,
        recipe: &crate::model::recipe::Recipe,
    ) -> Result<Option<AiSource>> {
        if !recipe.as_rendered().ai_denoise.renders() {
            return Ok(None);
        }
        anyhow::ensure!(
            !image.fast,
            "AI Denoise needs the full-size image; render without --fast"
        );
        let pack = super::subject_mask::Pack::Denoise;
        anyhow::ensure!(
            pack.is_installed(),
            "AI Denoise is on but its model is not installed"
        );
        self.service.set_model(Some(pack.installed_dir()));
        self.denoise(path, file, image).map(Some)
    }
    fn denoise(
        &self,
        path: &Path,
        file: &crate::storage::Identity,
        image: &Arc<CameraImage>,
    ) -> Result<AiSource> {
        let cancel = AtomicBool::new(false);
        self.service
            .get(path, file, image, &cancel)
            .and_then(|ai| AiSource::recover(Arc::unwrap_or_clone(ai), Cancel::new(&cancel)))
            .map_err(|e| anyhow!("AI Denoise: {e:#}"))
    }
}
impl Default for CommandLine {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for CommandLine {
    fn drop(&mut self) {
        super::task::wait_for(vec![self.service.stop()], Duration::from_secs(3));
    }
}

/// Options of `rawmakase denoise`, the development command.
pub struct Command {
    pub input: PathBuf,
    /// The model's folder; the installed model by default.
    pub model: Option<PathBuf>,
    pub provider: Provider,
    /// AI Denoise's Amount for `output` (0–1).
    pub amount: f32,
    /// A JPEG or PNG of the photo with its default settings and AI Denoise at `amount`.
    pub output: Option<PathBuf>,
    /// The same without AI Denoise.
    pub plain_output: Option<PathBuf>,
    pub max_edge: u32,
    /// The model's output with the gain applied, three little-endian f32 planes at the
    /// padded sensor size, and a JSON beside it with the layout and gain.
    pub dump: Option<PathBuf>,
    /// Use the user's AI Denoise cache rather than a throwaway one.
    pub cache: bool,
    /// Time Develop's renders while Amount moves: a Fit view of this long edge and a
    /// 100% region of 1600 × 1000.
    pub bench_fit: Option<u32>,
    /// Writes centre crops of `camera_crop` pixels of the camera image (white balanced
    /// camera RGB, highlights recovered) here: `plain.f32`, `ai.f32` and `nr<L>.f32` for
    /// each Luminance noise reduction amount in `nr_luma` (Color at its default), with
    /// `crop.json`. Phase 0's quality measures read them.
    pub camera_dump: Option<PathBuf>,
    pub camera_crop: u32,
    pub nr_luma: Vec<f32>,
    /// With `bench_fit`: the same renders without AI Denoise (Exposure moving instead),
    /// for the memory and time baseline; nothing is denoised.
    pub bench_plain: bool,
}

/// The process's resident memory in MB, as `ps` reports it (development command only).
fn resident_mb() -> f64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map_or(0., |kb| kb / 1024.)
}

/// Samples the resident memory until dropped, keeping the peak.
struct Peak {
    peak: Arc<std::sync::atomic::AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Peak {
    fn start() -> Self {
        let peak = Arc::new(std::sync::atomic::AtomicU64::new(resident_mb() as u64));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, s) = (peak.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                p.fetch_max(resident_mb() as u64, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            peak,
            stop,
            thread: Some(thread),
        }
    }
    fn finish(mut self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.peak.load(Ordering::Relaxed).max(resident_mb() as u64)
    }
}

/// Runs AI Denoise on one photo and reports times and the fit as JSON.
pub fn command(c: Command) -> Result<()> {
    use std::time::Instant;
    let started = Instant::now();
    // Read before decoding: the result must be of the same file.
    let file = crate::storage::Identity::read(&c.input)?;
    let plain = Arc::new(crate::photo::open(&c.input)?.develop(
        crate::camera_data::Decode::full(Default::default()),
        Cancel::NEVER,
    )?);
    let decoded = started.elapsed();
    if c.bench_plain {
        let recipe = crate::model::recipe::Recipe::for_metadata(&plain.metadata);
        let before = resident_mb();
        let peak = Peak::start();
        bench(&plain, None, &recipe, c.bench_fit.unwrap_or(2000))?;
        println!(
            "{}",
            serde_json::json!({"resident_before_mb": before, "drag_peak_mb": peak.finish()})
        );
        return Ok(());
    }
    let before_job = resident_mb();
    let job_peak = Peak::start();
    let scratch = tempfile::tempdir()?;
    let cache = if c.cache {
        user_cache(&super::session::load_session())
    } else {
        AiCache::new(scratch.path().to_path_buf(), 1 << 40)
    };
    let service = Service::with(
        cache,
        c.provider,
        Arc::new(open_mosaic),
        Arc::new(load_model),
    );
    let model = c
        .model
        .clone()
        .unwrap_or_else(|| super::subject_mask::Pack::Denoise.installed_dir());
    service.set_model(Some(model));
    let t = Instant::now();
    let entry = service.entry(&c.input, &file, &AtomicBool::new(false))?;
    let inferred = t.elapsed();
    let t = Instant::now();
    let image = assemble(
        &ModelOutput {
            layout: entry.layout,
            rgb: &entry.rgb,
            done: None,
            clipped: &entry.clipped,
        },
        &plain,
        Cancel::NEVER,
    )?;
    let source = AiSource::recover(image, Cancel::NEVER)?;
    let assembled = t.elapsed();
    // The worker and its ONNX session gone, so what remains is what Develop keeps.
    super::task::wait_for(vec![service.stop()], Duration::from_secs(10));
    let job_peak = job_peak.finish();
    if let Some(dump) = &c.dump {
        let bytes: Vec<u8> = entry.rgb.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(dump, bytes)?;
        let packed = open_mosaic(&c.input, &AtomicBool::new(false))?;
        let planes: Vec<u8> = packed.planes.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(dump.with_extension("planes"), planes)?;
        std::fs::write(
            dump.with_extension("json"),
            serde_json::json!({
                "width": entry.layout.width,
                "height": entry.layout.height,
                "pad": entry.layout.pad,
                "packed": [entry.layout.packed_width, entry.layout.packed_height],
                "gain": entry.gain,
            })
            .to_string(),
        )?;
    }
    // A raw file's defaults, as the app opens it (Color noise reduction 25, as Lightroom).
    let mut recipe = crate::model::recipe::Recipe::for_metadata(&plain.metadata);
    recipe.set_color_noise_defaults();
    if let Some(dir) = &c.camera_dump {
        camera_dump(dir, &plain, &source, &recipe, c.camera_crop, &c.nr_luma)?;
    }
    if let Some(fit) = c.bench_fit {
        let entry_kept = Arc::strong_count(&entry);
        let held = (entry, service);
        drop(held);
        let after_job = resident_mb();
        let peak = Peak::start();
        bench(&plain, Some(&source), &recipe, fit)?;
        println!(
            "{}",
            serde_json::json!({
                "resident_before_job_mb": before_job,
                "job_peak_mb": job_peak,
                "resident_after_job_mb": after_job,
                "drag_peak_mb": peak.finish(),
                "entry_refs": entry_kept,
            })
        );
        return Ok(());
    }
    let save =
        |path: &Path, ai: Option<&AiSource>, recipe: &crate::model::recipe::Recipe| -> Result<()> {
            let out = crate::develop::render_with_ai(
                &plain,
                &recipe.checked()?,
                ai,
                c.max_edge,
                Cancel::NEVER,
            )?;
            image::RgbImage::from_raw(out.width, out.height, out.rgb8())
                .ok_or_else(|| anyhow!("bad render"))?
                .save(path)?;
            Ok(())
        };
    if let Some(path) = &c.plain_output {
        save(path, None, &recipe)?;
    }
    if let Some(path) = &c.output {
        // Switched on as the panel switches it (manual noise reduction to 0).
        recipe.switch_ai_denoise(true);
        recipe.ai_denoise.amount = c.amount;
        save(path, Some(&source), &recipe)?;
    }
    println!(
        "{}",
        serde_json::json!({
            "file": c.input,
            "provider": c.provider.name(),
            "width": plain.width,
            "height": plain.height,
            "decode_s": decoded.as_secs_f64(),
            "denoise_s": inferred.as_secs_f64(),
            "assemble_recover_s": assembled.as_secs_f64(),
            "gain": entry.gain,
        })
    );
    Ok(())
}

/// Centre crops of the camera image without noise reduction, with AI Denoise, and with
/// each Luminance amount (see [`Command::camera_dump`]).
fn camera_dump(
    dir: &Path,
    plain: &Arc<CameraImage>,
    source: &AiSource,
    recipe: &crate::model::recipe::Recipe,
    side: u32,
    nr_luma: &[f32],
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let (w, h) = (side.min(plain.width), side.min(plain.height));
    let (x0, y0) = ((plain.width - w) / 2, (plain.height - h) / 2);
    let cancel = AtomicBool::new(false);
    let write = |name: &str, image: &CameraImage| -> Result<()> {
        let mut bytes = Vec::with_capacity((w * h * 12) as usize);
        for y in y0..y0 + h {
            let row = (y * image.width) as usize;
            for p in &image.pixels[row + x0 as usize..row + (x0 + w) as usize] {
                for v in p {
                    bytes.extend(v.to_le_bytes());
                }
            }
        }
        std::fs::write(dir.join(format!("{name}.f32")), bytes)?;
        Ok(())
    };
    let mut off = recipe.clone();
    off.noise_luma = 0.;
    off.noise_chroma = 0.;
    write(
        "plain",
        &*crate::develop::quality::denoised_camera(plain, &off, None, Cancel::new(&cancel))?,
    )?;
    let mut ai = off;
    ai.ai_denoise = crate::model::ai_denoise::AiDenoise {
        enabled: true,
        amount: 1.,
    };
    write(
        "ai",
        &*crate::develop::quality::denoised_camera(plain, &ai, Some(source), Cancel::new(&cancel))?,
    )?;
    for luma in nr_luma {
        let mut nr = recipe.clone();
        nr.noise_luma = *luma;
        write(
            &format!("nr{:.0}", luma * 100.),
            &*crate::develop::quality::denoised_camera(plain, &nr, None, Cancel::new(&cancel))?,
        )?;
    }
    std::fs::write(
        dir.join("crop.json"),
        serde_json::json!({
            "x": x0, "y": y0, "width": w, "height": h,
            "wb": plain.metadata.wb, "matrix": plain.metadata.matrix,
            "cam_xyz": plain.metadata.cam_xyz, "chroma_default": recipe.noise_chroma,
        })
        .to_string(),
    )?;
    Ok(())
}

/// Develop's renders while AI Denoise's Amount moves, as the preview renderer runs them.
fn bench(
    plain: &Arc<CameraImage>,
    source: Option<&AiSource>,
    recipe: &crate::model::recipe::Recipe,
    fit: u32,
) -> Result<()> {
    use std::time::Instant;
    let mut renderer = if std::env::var_os("RAWMAKASE_BENCH_CPU").is_some() {
        crate::develop::PreviewRenderer::with_processor(Err(anyhow!("CPU bench")))
    } else {
        crate::develop::PreviewRenderer::with_gpu()
    };
    renderer.set_ai(source.cloned());
    let mut recipe = recipe.clone();
    recipe.ai_denoise.enabled = source.is_some();
    let cancel = AtomicBool::new(false);
    let region = [
        plain.width / 2 - 800,
        plain.height / 2 - 500,
        1600.min(plain.width),
        1000.min(plain.height),
    ];
    let mut report = Vec::new();
    for (label, region, edge) in [("fit", None, fit), ("region", Some(region), 0)] {
        let mut times = Vec::new();
        for (i, amount) in [0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8]
            .into_iter()
            .enumerate()
        {
            match source {
                Some(_) => recipe.ai_denoise.amount = amount,
                None => recipe.exposure = amount - 0.5,
            }
            let t = Instant::now();
            let _ = renderer.render(plain, &recipe, edge, region, Cancel::new(&cancel))?;
            if i > 0 {
                times.push(t.elapsed().as_secs_f64() * 1000.);
            }
        }
        times.sort_by(f64::total_cmp);
        report.push(serde_json::json!({
            "view": label,
            "gpu": renderer.used_gpu(),
            "median_ms": times[times.len() / 2],
            "max_ms": times[times.len() - 1],
        }));
    }
    println!("{}", serde_json::Value::Array(report));
    Ok(())
}

#[cfg(test)]
#[path = "ai_denoise_tests.rs"]
mod tests;
