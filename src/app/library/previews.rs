//! Bounded, asynchronous disk-cache work and its UI progress.
use crate::app::widgets::plural;
use crate::catalog::PhotoId;
use crate::catalog::preview_cache::{PreviewCache, Stamp};
use crate::model::recipe::Recipe;
use eframe::egui;
use rawmakase_cancel::Cancel;
use rawmakase_decode::ai_cache::AiCache;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::JoinHandle,
};

pub(super) struct PreviewResult {
    pub path: PathBuf,
    pub image: Option<image::RgbImage>,
    pub cache_error: Option<String>,
}

/// A preview worker: where its jobs go, where its results come back, and its thread.
pub(super) type Worker<J, R> = (J, Receiver<R>, JoinHandle<()>);

/// The thumbnail workers, which end once their requests or their results
/// are dropped: a few side by side (see [`thumbnail_threads`]), so one slow
/// file or disk doesn't hold up the rest. They run at normal priority, as the
/// one worker did, so the grid fills before edited previews are rendered.
pub(super) fn spawn(
    cache_path: PathBuf,
    ctx: egui::Context,
) -> Worker<SyncSender<PathBuf>, PreviewResult> {
    spawn_with(cache_path, ctx, super::thumbnail)
}
/// Thumbnails made side by side: enough to keep a disk and a few cores busy,
/// leaving the rest to the edited previews and Develop.
fn thumbnail_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 4))
}
fn spawn_with(
    cache_path: PathBuf,
    ctx: egui::Context,
    thumbnail: fn(&Path) -> anyhow::Result<image::RgbImage>,
) -> Worker<SyncSender<PathBuf>, PreviewResult> {
    let (tx, rx) = mpsc::sync_channel::<PathBuf>(24);
    let (result_tx, result_rx) = mpsc::sync_channel(24);
    let thread = std::thread::spawn(move || {
        let threads = thumbnail_threads();
        let open = || match PreviewCache::open(&cache_path) {
            Ok(cache) => (Some(cache), None),
            Err(error) => (None, Some(error.to_string())),
        };
        // A connection each. The first is opened here, as the one worker's
        // was, and makes the database; the others on their first request, so
        // a worker that is never asked opens nothing and ends at once.
        let first = open();
        let rx = Mutex::new(rx);
        let work = |opened: Option<(Option<PreviewCache>, Option<String>)>| {
            let mut opened = opened;
            loop {
                let Ok(path) = rx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv()
                else {
                    break;
                };
                let (cache, open_error) = opened.get_or_insert_with(open);
                let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut cache_error = open_error.clone();
                    let cached = cache.as_ref().and_then(|cache| match cache.load(&path) {
                        Ok(image) => image,
                        Err(error) => {
                            cache_error = Some(error.to_string());
                            None
                        }
                    });
                    let image = cached.or_else(|| {
                        let stamp = Stamp::read(&path).ok()?;
                        let image = thumbnail(&path).ok()?;
                        if let Some(cache) = cache.as_mut()
                            && let Err(error) = cache.store(&path, &stamp, &image)
                        {
                            cache_error = Some(error.to_string());
                        }
                        Some(image)
                    });
                    (image, cache_error)
                }));
                // After a panic the preview is unavailable, and the cache, which it may
                // have left mid-write, is opened again for the next one.
                let (image, cache_error) = prepared.unwrap_or_else(|_| {
                    opened = Some(open());
                    (None, None)
                });
                if result_tx
                    .send(PreviewResult {
                        path,
                        image,
                        cache_error,
                    })
                    .is_err()
                {
                    break;
                }
                ctx.request_repaint();
            }
        };
        // One works on this thread, so a worker the system can't start
        // leaves the others; returns once every worker has ended.
        let work = &work;
        std::thread::scope(|scope| {
            for _ in 1..threads {
                let _ = std::thread::Builder::new()
                    .name("thumbnail".into())
                    .spawn_scoped(scope, move || work(None));
            }
            work(Some(first));
        });
    });
    (tx, result_rx, thread)
}

/// What an edited preview is rendered from.
#[derive(Clone)]
pub(in crate::app) enum EditSource {
    /// A saved RAWmakase recipe, as JSON.
    Recipe(String),
    /// Lightroom develop settings from an imported catalog.
    Lightroom(String),
    /// No edit: the raw defaults, as Develop would open the photo.
    Defaults(Arc<crate::raw_defaults::DevelopDefaults>),
}
impl EditSource {
    /// The recipe the photo at `path` is rendered with: its edit, or the defaults
    /// Develop would open it with (Adobe Default unless given), a fitted camera look
    /// included for a photo without colour data.
    pub(crate) fn recipe(
        edit: Option<&Self>,
        m: &crate::camera_data::Metadata,
        path: &Path,
        cancel: &AtomicBool,
    ) -> anyhow::Result<Recipe> {
        let (profiles, _) = crate::camera_profiles::installed(m);
        let profiles = crate::camera_look::with_default(path, m, profiles, Cancel::new(cancel));
        Ok(match edit {
            Some(Self::Recipe(json)) => serde_json::from_str(json)?,
            Some(Self::Lightroom(text)) => {
                crate::lr_develop::convert_develop(text, m, &profiles, None)?.0
            }
            Some(Self::Defaults(defaults)) => defaults.resolve(m, &profiles).recipe,
            None => Recipe::with_profiles(m, &profiles),
        })
    }
    /// Identifies this edit, developed by this build's RAW decoder
    /// (`raw::developed`), in the preview cache.
    pub(crate) fn tag(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self {
            Self::Recipe(text) => ("recipe", text).hash(&mut h),
            Self::Lightroom(text) => ("lightroom", text).hash(&mut h),
            Self::Defaults(defaults) => ("defaults", format!("{defaults:?}")).hash(&mut h),
        }
        crate::raw::developed().hash(&mut h);
        format!("edit-{:016x}", h.finish())
    }
    /// Identifies this edit's Library preview in the preview cache: [`Self::tag`],
    /// how the preview is rendered (Develop's Fit path), so previews rendered
    /// another way are made again rather than shown beside new ones, and, for an
    /// edit with AI Denoise, whether it shows the installed model's result.
    pub(crate) fn preview_tag(&self, ai: PreviewAi) -> String {
        self.preview_tag_of(ai, &crate::app::ai_denoise::model_tag())
    }
    /// [`preview_tag`](Self::preview_tag) with AI Denoise's result from `model`.
    fn preview_tag_of(&self, ai: PreviewAi, model: &str) -> String {
        let tag = self.tag();
        match ai {
            PreviewAi::Off => format!("{tag}-fit"),
            // Not "-fit": before AI Denoise was told apart, Develop kept its renders
            // with the result under the edit's plain tag. They are not shown again.
            PreviewAi::Plain => format!("{tag}-fit-noai"),
            PreviewAi::Finished => format!("{tag}-fit-ai-{model}"),
        }
    }
    /// Whether this edit renders AI Denoise, as far as the Library tells without
    /// opening the file: a RAWmakase edit with it on. A Lightroom edit or the
    /// defaults render without it here, as they did before.
    pub(in crate::app) fn renders_ai(&self) -> bool {
        matches!(self, Self::Recipe(json) if serde_json::from_str::<Recipe>(json)
            .is_ok_and(|r| r.as_rendered().ai_denoise.renders()))
    }
}
/// What a Library preview shows of AI Denoise (docs/rendering-quality.md#ai-denoise).
/// A preview made with a partial result is never kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum PreviewAi {
    /// The edit has no AI Denoise.
    Off,
    /// The edit has AI Denoise, rendered without a result: none was finished.
    Plain,
    /// The edit rendered with a finished result of the installed model.
    Finished,
}
pub(super) enum EditJob {
    /// Render (or load from cache) catalog photo `id` with its edit. Virtual
    /// copies share a file but not an edit, so results go by photo.
    Render {
        id: PhotoId,
        /// Matches the result to this request, not to a later photo that
        /// reused a removed copy's id.
        ticket: u64,
        path: PathBuf,
        source: EditSource,
        /// Asked for before the photo is shown, to be kept in the cache: only
        /// at [`Pace::Full`], after every photo shown, and answered
        /// [`EditResult::Kept`] unless the photo is shown by then.
        ahead: bool,
    },
    /// Keep an already rendered preview, e.g. from Develop, of the file as
    /// `stamp` gives it when it was rendered: not kept once the file differs.
    Store {
        path: PathBuf,
        tag: String,
        stamp: Stamp,
        image: image::RgbImage,
    },
}
/// Photos shown in the grid or filmstrip in the last frame. Edited previews
/// are rendered only for these and the ones asked for ahead, so scrolling
/// past photos leaves no backlog.
pub(super) type Wanted = Arc<Mutex<HashSet<PhotoId>>>;
/// What an edited preview job came to, by photo and the ticket it was asked with.
pub(super) enum EditResult {
    Ready(PhotoId, u64, image::RgbImage),
    /// Asked ahead and not shown yet: rendered and kept in the cache, or not
    /// rendered, there being no cache to keep it in. Asked again when shown.
    Kept(PhotoId, u64),
    /// The photo scrolled out of view first, or its render gave way to
    /// Develop; request it again when shown.
    Skipped(PhotoId, u64),
    Failed(PhotoId, u64),
    /// A preview could not be kept in the cache; it comes besides any result.
    CacheError(String),
}
/// How much of the computer edited previews take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Pace {
    /// One render at a time on two background threads, leaving the rest to
    /// Develop and whatever else is rendering.
    #[default]
    Background,
    /// The Library in front with nothing else rendering: a few renders side by
    /// side on every core but one, and photos rendered ahead of being shown.
    Full,
}
/// Renders a 640 px edited preview, giving up once the flag is set.
type RenderEdited = fn(&Path, &EditSource, &AtomicBool) -> anyhow::Result<image::RgbImage>;
/// Renders a preview of an edit with AI Denoise from a finished result: the file's
/// version the result was made for, and the result.
type RenderWithAi = fn(
    &Path,
    &crate::storage::Identity,
    &EditSource,
    &rawmakase_decode::ai_cache::AiEntry,
    &AtomicBool,
) -> anyhow::Result<image::RgbImage>;
/// Where edited preview jobs go, and how fast they are rendered. Dropping it
/// closes the worker, as [`EditQueue::close`] does.
pub(super) struct EditQueue(Arc<Shared>);
/// The queue's state, which the UI, the worker and its renders share.
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}
#[derive(Default)]
struct State {
    /// Jobs not started, oldest first.
    jobs: Vec<Queued>,
    pace: Pace,
    closed: bool,
    /// Renders under way.
    running: Vec<Running>,
    next_slot: u64,
    /// Where AI Denoise's results are, which an edit with it renders from once
    /// finished; none until the app sets it.
    ai: Option<AiCache>,
}
struct Queued {
    job: EditJob,
    /// The cache had no preview for it when asked; it waits for a render.
    missed: bool,
    /// Asked ahead for a list no longer shown: answered without a render
    /// unless its photo is shown.
    dropped: bool,
}
struct Running {
    slot: u64,
    /// On the full pace's threads, so given up when the pace drops.
    full: bool,
    cancel: Arc<AtomicBool>,
    /// The file rendered: another render of it waits for this one.
    path: PathBuf,
}
impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        for running in &state.running {
            running.cancel.store(true, Ordering::Relaxed);
        }
        self.changed.notify_all();
    }
}
#[derive(Debug)]
pub(super) struct Closed;
impl std::fmt::Display for Closed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The edited preview worker is closed")
    }
}
impl std::error::Error for Closed {}
impl EditQueue {
    pub(super) fn send(&self, job: EditJob) -> Result<(), Closed> {
        let mut state = self.0.lock();
        if state.closed {
            return Err(Closed);
        }
        state.jobs.push(Queued {
            job,
            missed: false,
            dropped: false,
        });
        self.0.changed.notify_all();
        Ok(())
    }
    /// From now on renders at `pace`; dropping to [`Pace::Background`] gives up
    /// the renders under way at full pace (answered `Skipped`) and the photos
    /// asked for ahead.
    pub(super) fn set_pace(&self, pace: Pace) {
        let mut state = self.0.lock();
        if state.pace == pace {
            return;
        }
        state.pace = pace;
        if pace == Pace::Background {
            for running in state.running.iter().filter(|r| r.full) {
                running.cancel.store(true, Ordering::Relaxed);
            }
        }
        self.0.changed.notify_all();
    }
    /// Finished AI Denoise results come from `cache` from now on.
    pub(super) fn set_ai_cache(&self, cache: AiCache) {
        self.0.lock().ai = Some(cache);
    }
    /// Drops the photos asked ahead and not started, for a list shown no
    /// longer: each is answered `Skipped` unless it is shown by then. Renders
    /// already under way finish.
    pub(super) fn drop_ahead(&self) {
        let mut state = self.0.lock();
        for queued in &mut state.jobs {
            if matches!(queued.job, EditJob::Render { ahead: true, .. }) {
                queued.dropped = true;
            }
        }
        self.0.changed.notify_all();
    }
    /// Cancels the renders under way and ends the worker once they stop;
    /// jobs not started are dropped.
    pub(super) fn close(&self) {
        self.0.close();
    }
    /// A queue no worker takes from, and what is sent to it, for tests.
    #[cfg(test)]
    pub(super) fn detached() -> (Self, Sent) {
        let shared = Arc::<Shared>::default();
        (Self(shared.clone()), Sent(shared))
    }
}
impl Drop for EditQueue {
    fn drop(&mut self) {
        self.0.close();
    }
}
/// The jobs sent to a detached queue, oldest first.
#[cfg(test)]
pub(in crate::app) struct Sent(Arc<Shared>);
#[cfg(test)]
impl Sent {
    pub(super) fn try_recv(&self) -> Option<EditJob> {
        let mut state = self.0.lock();
        (!state.jobs.is_empty()).then(|| state.jobs.remove(0).job)
    }
    /// How many jobs sent and not taken are dropped.
    pub(super) fn dropped(&self) -> usize {
        self.0.lock().jobs.iter().filter(|q| q.dropped).count()
    }
    /// The tags of the previews sent to be kept and not taken, oldest first.
    pub(in crate::app) fn stored_tags(&self) -> Vec<String> {
        let state = self.0.lock();
        state
            .jobs
            .iter()
            .filter_map(|q| match &q.job {
                EditJob::Store { tag, .. } => Some(tag.clone()),
                EditJob::Render { .. } => None,
            })
            .collect()
    }
}
/// Threads the full pace renders on: every core but one.
fn full_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).max(2))
}
/// The most renders side by side at full pace. Each holds its file, its sensor
/// data and a half-size image, so a computer with many cores spends the rest
/// on each render's parallel stages rather than on more renders at once.
const FULL_RENDERS: usize = 4;
/// Memory per render side by side at full pace. With 46-102 MP RAWs, four at
/// once peaked at 4.7-5.25 GB, against 2.9 GB for the largest alone and 3.2-3.7
/// GB for one at a time: an 8 GB computer renders two at once, 16 GB or more
/// four.
const MEMORY_PER_RENDER: u64 = 4 << 30;
/// Renders side by side at full pace on a computer with `memory` bytes (to the
/// nearest [`MEMORY_PER_RENDER`], as systems keep some back) and
/// `full_threads`; two when its memory isn't known.
fn full_renders(memory: Option<u64>, full_threads: usize) -> usize {
    let by_memory = memory.map_or(2, |bytes| {
        (bytes.saturating_add(MEMORY_PER_RENDER / 2) / MEMORY_PER_RENDER) as usize
    });
    by_memory.clamp(1, FULL_RENDERS).min(full_threads)
}
/// Renders side by side at `pace`, at most `full_renders` at full pace.
fn renders_at(pace: Pace, full_renders: usize) -> usize {
    match pace {
        Pace::Background => 1,
        Pace::Full => full_renders,
    }
}
/// What the worker does next.
enum Step {
    Store(PathBuf, String, Stamp, image::RgbImage),
    /// Answer without rendering: the photo is out of view, or was asked for
    /// ahead and the pace dropped.
    Skip(PhotoId, u64),
    /// Ask the cache, then render on a miss when `render` (a free slot).
    Look(usize, bool),
    Render(usize),
    Exit,
}
/// Edited previews on their own worker, so slow renders never delay the
/// embedded previews that fill the grid first. Photos shown go first, the
/// latest request first, then the photos asked for ahead in the order asked.
/// At [`Pace::Background`] one render runs at a time on two background
/// threads; at [`Pace::Full`] up to [`FULL_RENDERS`] side by side on every
/// core but one. The
/// cache is read and written by the worker and its renders under one lock.
/// Closing the queue cancels the renders under way; the worker ends once
/// they have stopped.
pub(super) fn spawn_edited(
    cache_path: PathBuf,
    wanted: Wanted,
    ctx: egui::Context,
) -> Worker<EditQueue, EditResult> {
    spawn_edited_with(cache_path, wanted, ctx, render_edited)
}
fn spawn_edited_with(
    cache_path: PathBuf,
    wanted: Wanted,
    ctx: egui::Context,
    render: RenderEdited,
) -> Worker<EditQueue, EditResult> {
    spawn_edited_with_ai(cache_path, wanted, ctx, render, render_edited_with_ai)
}
fn spawn_edited_with_ai(
    cache_path: PathBuf,
    wanted: Wanted,
    ctx: egui::Context,
    render: RenderEdited,
    render_ai: RenderWithAi,
) -> Worker<EditQueue, EditResult> {
    let shared = Arc::<Shared>::default();
    let (tx, result_rx) = mpsc::channel();
    let results = Results {
        tx,
        shared: shared.clone(),
        ctx,
    };
    let thread = crate::raw::spawn_background(move || {
        let full_threads = full_threads();
        let full_renders = full_renders(crate::platform::memory::total(), full_threads);
        let renders = Renders {
            background: crate::raw::background_pool(2, "edited-preview").ok(),
            full: crate::raw::background_pool(full_threads, "edited-preview-full").ok(),
            cache: Arc::new(Mutex::new(PreviewCache::open(&cache_path).ok())),
            cache_path,
            wanted,
            results,
            render,
            render_ai,
        };
        loop {
            match next_step(&renders.results.shared, &renders.wanted, full_renders) {
                Step::Exit => break,
                Step::Skip(id, ticket) => renders.results.send(EditResult::Skipped(id, ticket)),
                Step::Store(path, tag, stamp, image) => renders.store(&path, &tag, &stamp, &image),
                Step::Look(index, render) => renders.look(index, render),
                Step::Render(index) => renders.start(index),
            }
        }
        // The pools' threads end as they are dropped, their renders done.
    });
    (EditQueue(shared), result_rx, thread)
}
/// Where results go; a closed receiver closes the queue.
#[derive(Clone)]
struct Results {
    tx: mpsc::Sender<EditResult>,
    shared: Arc<Shared>,
    ctx: egui::Context,
}
impl Results {
    fn send(&self, result: EditResult) {
        if self.tx.send(result).is_err() {
            self.shared.close();
        }
        self.ctx.request_repaint();
    }
}
/// What the worker renders with.
struct Renders {
    background: Option<rayon::ThreadPool>,
    full: Option<rayon::ThreadPool>,
    cache: Arc<Mutex<Option<PreviewCache>>>,
    cache_path: PathBuf,
    wanted: Wanted,
    results: Results,
    render: RenderEdited,
    render_ai: RenderWithAi,
}
/// The preview cache, held while it is read or written.
fn cache(cache: &Mutex<Option<PreviewCache>>) -> std::sync::MutexGuard<'_, Option<PreviewCache>> {
    cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
/// After a panic the cache, which it may have left mid-write, is opened again.
fn reopen(at: &Mutex<Option<PreviewCache>>, path: &Path) {
    *cache(at) = PreviewCache::open(path).ok();
}
impl Renders {
    /// Keeps `image`, rendered from the file at `path` as `stamp` gives it,
    /// as its preview under `tag`; a file changed since gets none of it, and
    /// that is not a cache failure.
    fn store(&self, path: &Path, tag: &str, stamp: &Stamp, image: &image::RgbImage) {
        let stored = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match &mut *cache(&self.cache) {
                Some(cache) if Stamp::read(path).ok().as_ref() == Some(stamp) => cache
                    .store_tagged(path, tag, stamp, image)
                    .err()
                    .map(|error| error.to_string()),
                _ => None,
            }
        }));
        match stored {
            Ok(Some(error)) => self.results.send(EditResult::CacheError(error)),
            Ok(None) => {}
            Err(_) => reopen(&self.cache, &self.cache_path),
        }
    }
    /// Answers the job at `index` from the cache, else renders it when
    /// `render`, else marks it to wait for a render.
    fn look(&self, index: usize, render: bool) {
        let (id, ticket, path, source, ahead, ai) = {
            let state = self.results.shared.lock();
            match &state.jobs[index].job {
                EditJob::Render {
                    id,
                    ticket,
                    path,
                    source,
                    ahead,
                } => (
                    *id,
                    *ticket,
                    path.clone(),
                    source.clone(),
                    *ahead,
                    state.ai.clone(),
                ),
                EditJob::Store { .. } => unreachable!("stores are not looked up"),
            }
        };
        let tags = preview_tags(&path, &source, ai.as_ref());
        let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let cache = cache(&self.cache);
            let cache = cache.as_ref()?;
            tags.iter()
                .find_map(|tag| cache.load_tagged(&path, tag).ok().flatten())
        }))
        .unwrap_or_else(|_| {
            reopen(&self.cache, &self.cache_path);
            None
        });
        let mut state = self.results.shared.lock();
        match hit {
            Some(image) => {
                state.jobs.remove(index);
                drop(state);
                self.results.send(self.answer(id, ticket, ahead, image));
            }
            None if render => {
                drop(state);
                self.start(index);
            }
            None => state.jobs[index].missed = true,
        }
    }
    /// A preview for photo `id`: shown, unless it was asked ahead and the
    /// photo is not shown yet.
    fn answer(&self, id: PhotoId, ticket: u64, ahead: bool, image: image::RgbImage) -> EditResult {
        answer(&self.wanted, id, ticket, ahead, image)
    }
    /// Renders the job at `index` on the pool of the pace it starts at. A
    /// photo asked ahead and not shown is not rendered once the pace has
    /// dropped or its list has gone since it was looked up, nor with no cache
    /// to keep it in (rendered when shown).
    fn start(&self, index: usize) {
        let shared = &self.results.shared;
        let no_cache = cache(&self.cache).is_none();
        let mut state = shared.lock();
        // Closed since it was looked up: nothing is rendered, nor answered.
        if state.closed {
            return;
        }
        let queued = state.jobs.remove(index);
        let EditJob::Render {
            id,
            ticket,
            path,
            source,
            ahead,
        } = queued.job
        else {
            unreachable!("stores are not rendered")
        };
        if ahead && !shown(&self.wanted, id) {
            let answer = if queued.dropped || state.pace == Pace::Background {
                Some(EditResult::Skipped(id, ticket))
            } else {
                no_cache.then_some(EditResult::Kept(id, ticket))
            };
            if let Some(answer) = answer {
                drop(state);
                self.results.send(answer);
                return;
            }
        }
        let full = state.pace == Pace::Full;
        let ai = state.ai.clone();
        let slot = state.next_slot;
        state.next_slot += 1;
        let cancel = Arc::<AtomicBool>::default();
        state.running.push(Running {
            slot,
            full,
            cancel: cancel.clone(),
            path: path.clone(),
        });
        drop(state);
        let (cache_at, cache_path, wanted, results, render, render_ai) = (
            self.cache.clone(),
            self.cache_path.clone(),
            self.wanted.clone(),
            self.results.clone(),
            self.render,
            self.render_ai,
        );
        let task = move || {
            let _finished = Finished(results.shared.clone(), slot);
            let mut cache_error = None;
            let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Given up before it began: not even the file is read.
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let stamp = Stamp::read(&path).ok()?;
                let (image, shows) = match finished_entry(&path, &source, ai.as_ref()) {
                    Some((file, entry)) => (
                        render_ai(&path, &file, &source, &entry, &cancel).ok()?,
                        PreviewAi::Finished,
                    ),
                    None => (render(&path, &source, &cancel).ok()?, plain_of(&source)),
                };
                if let Some(cache) = &mut *cache(&cache_at)
                    && let Err(error) =
                        cache.store_tagged(&path, &source.preview_tag(shows), &stamp, &image)
                {
                    cache_error = Some(error.to_string());
                }
                Some(image)
            }));
            let image = rendered.unwrap_or_else(|_| {
                reopen(&cache_at, &cache_path);
                None
            });
            if let Some(error) = cache_error {
                results.send(EditResult::CacheError(error));
            }
            results.send(if cancel.load(Ordering::Relaxed) {
                EditResult::Skipped(id, ticket)
            } else {
                match image {
                    Some(image) => answer(&wanted, id, ticket, ahead, image),
                    None => EditResult::Failed(id, ticket),
                }
            });
        };
        match if full { &self.full } else { &self.background } {
            Some(pool) => pool.spawn(task),
            None => drop(std::thread::spawn(task)),
        }
    }
}
fn answer(
    wanted: &Wanted,
    id: PhotoId,
    ticket: u64,
    ahead: bool,
    image: image::RgbImage,
) -> EditResult {
    if ahead && !shown(wanted, id) {
        EditResult::Kept(id, ticket)
    } else {
        EditResult::Ready(id, ticket, image)
    }
}
/// Whether photo `id` was shown last frame.
fn shown(wanted: &Wanted, id: PhotoId) -> bool {
    wanted
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&id)
}
/// Ends a render's slot however the render ends, panics included.
struct Finished(Arc<Shared>, u64);
impl Drop for Finished {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.running.retain(|r| r.slot != self.1);
        self.0.changed.notify_all();
    }
}
/// The worker's next step, waiting while there is none. A job leaves the
/// queue only by the step that does it, and only the worker removes jobs
/// (the UI appends), so an index stays valid until its step.
fn next_step(shared: &Shared, wanted: &Wanted, full_renders: usize) -> Step {
    let mut state = shared.lock();
    loop {
        if state.closed {
            // Once the renders under way have stopped, as their cancel asks.
            if state.running.is_empty() {
                return Step::Exit;
            }
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
            continue;
        }
        // Stores first: they are cheap and keep Develop's renders.
        if let Some(index) = state
            .jobs
            .iter()
            .position(|q| matches!(q.job, EditJob::Store { .. }))
        {
            let EditJob::Store {
                path,
                tag,
                stamp,
                image,
            } = state.jobs.remove(index).job
            else {
                unreachable!()
            };
            return Step::Store(path, tag, stamp, image);
        }
        let shown = wanted.lock().unwrap().clone();
        let full = state.pace == Pace::Full;
        // Out of view, or asked ahead at a pace that renders none ahead, or for
        // a list no longer shown.
        if let Some(index) = state.jobs.iter().position(|q| match &q.job {
            EditJob::Render { id, ahead, .. } => {
                !shown.contains(id) && (!ahead || !full || q.dropped)
            }
            EditJob::Store { .. } => false,
        }) {
            let EditJob::Render { id, ticket, .. } = state.jobs.remove(index).job else {
                unreachable!()
            };
            return Step::Skip(id, ticket);
        }
        // Every job left is a render, of a photo shown or asked ahead.
        let visible =
            |q: &Queued| matches!(&q.job, EditJob::Render { id, .. } if shown.contains(id));
        let free = state.running.len() < renders_at(state.pace, full_renders);
        // One render of a file at a time, so its virtual copies share one Camera
        // Look fit (`camera_look::default_profile`): renders on the pool don't
        // wait for each other's fits, as that could deadlock, but fit a file
        // themselves. A second waits here instead, outside the pool.
        let rendering = |q: &Queued| match &q.job {
            EditJob::Render { path, .. } => state.running.iter().any(|r| &r.path == path),
            EditJob::Store { .. } => false,
        };
        // Photos shown, the latest first: from the cache whether or not a
        // render could start, rendered when one can.
        if let Some(index) = state.jobs.iter().rposition(|q| visible(q) && !q.missed) {
            return Step::Look(index, free && !rendering(&state.jobs[index]));
        }
        if free {
            if let Some(index) = state.jobs.iter().rposition(|q| visible(q) && !rendering(q)) {
                return Step::Render(index);
            }
            // Then the ones asked ahead, in the order asked.
            if let Some(index) = state.jobs.iter().position(|q| !visible(q) && !rendering(q)) {
                return if state.jobs[index].missed {
                    Step::Render(index)
                } else {
                    Step::Look(index, true)
                };
            }
        }
        state = shared
            .changed
            .wait(state)
            .unwrap_or_else(|e| e.into_inner());
    }
}
/// A 640 px preview of `path` developed with `source`, as Develop's Fit draws it
/// (`develop::render_fitted`): from a reduced copy of the photo, several times
/// faster than rendering it whole and reducing the result.
fn render_edited(
    path: &Path,
    source: &EditSource,
    cancel: &AtomicBool,
) -> anyhow::Result<image::RgbImage> {
    let (image, recipe) = open_quickly(path, Some(source), cancel)?;
    rgb(crate::develop::render_fitted(
        &image,
        &recipe,
        640,
        Cancel::new(cancel),
    )?)
}
/// [`develop_with_ai`] at the Library's preview size.
fn render_edited_with_ai(
    path: &Path,
    file: &crate::storage::Identity,
    source: &EditSource,
    entry: &rawmakase_decode::ai_cache::AiEntry,
    cancel: &AtomicBool,
) -> anyhow::Result<image::RgbImage> {
    develop_with_ai(path, file, source, entry, 640, cancel)
}
/// What a render of `source` without an AI Denoise result shows.
fn plain_of(source: &EditSource) -> PreviewAi {
    if source.renders_ai() {
        PreviewAi::Plain
    } else {
        PreviewAi::Off
    }
}
/// The file at `path` as it is now and the finished AI Denoise result for it in
/// `ai`, when `source` renders AI Denoise and there is one.
fn finished_entry(
    path: &Path,
    source: &EditSource,
    ai: Option<&AiCache>,
) -> Option<(
    crate::storage::Identity,
    rawmakase_decode::ai_cache::AiEntry,
)> {
    if !source.renders_ai() {
        return None;
    }
    let (file, key) = finished_ai(path, ai)?;
    Some((file, ai?.load(&key)?))
}
/// The tags a preview of `source` at `path` may be kept under, in the order it is
/// looked for: the plain render of an edit with AI Denoise only while no finished
/// result is there to render from instead. An offline photo shows whichever it has.
fn preview_tags(path: &Path, source: &EditSource, ai: Option<&AiCache>) -> Vec<String> {
    if !source.renders_ai() {
        return vec![source.preview_tag(PreviewAi::Off)];
    }
    let mut tags = vec![source.preview_tag(PreviewAi::Finished)];
    if finished_ai(path, ai).is_none() {
        tags.push(source.preview_tag(PreviewAi::Plain));
    }
    tags
}
/// The file at `path` as it is now and the key of a finished AI Denoise result
/// for it in `cache`, if there is one.
fn finished_ai(path: &Path, ai: Option<&AiCache>) -> Option<(crate::storage::Identity, String)> {
    let file = crate::storage::Identity::read(path).ok()?;
    let key = crate::app::ai_denoise::finished(ai?, &file)?;
    Some((file, key))
}
/// `path` developed with `source` within `edge` pixels for a preview, and what it
/// shows of AI Denoise: an edit with it on renders from a finished result of the
/// installed model when `ai` has one for the file, decoding the photo at full size;
/// otherwise from the half-size decode, without it, as [`develop_quickly`] does.
pub(super) fn develop_for_preview(
    path: &Path,
    source: &EditSource,
    edge: u32,
    ai: Option<&AiCache>,
    cancel: &AtomicBool,
) -> anyhow::Result<(image::RgbImage, PreviewAi)> {
    if let Some((file, entry)) = finished_entry(path, source, ai) {
        return Ok((
            develop_with_ai(path, &file, source, &entry, edge, cancel)?,
            PreviewAi::Finished,
        ));
    }
    Ok((
        develop_quickly(path, Some(source), edge, cancel)?,
        plain_of(source),
    ))
}
/// The RAW at `path`, the version `file` names, decoded at full size and rendered
/// with `source` and AI Denoise's finished result `entry` within `edge` pixels. A
/// file changed meanwhile fails: the result is of the version it was made for.
fn develop_with_ai(
    path: &Path,
    file: &crate::storage::Identity,
    source: &EditSource,
    entry: &rawmakase_decode::ai_cache::AiEntry,
    edge: u32,
    cancel: &AtomicBool,
) -> anyhow::Result<image::RgbImage> {
    use rawmakase_decode::ai_denoise::{ModelOutput, assemble};
    let raw = crate::photo::open(path)?;
    let recipe = EditSource::recipe(Some(source), &raw.metadata, path, cancel)?;
    let plain = raw.develop(
        crate::camera_data::Decode::Full(Default::default()),
        Cancel::new(cancel),
    )?;
    let denoised = assemble(
        &ModelOutput {
            layout: entry.layout,
            rgb: &entry.rgb,
            done: None,
            clipped: &entry.clipped,
        },
        &plain,
        Cancel::new(cancel),
    )?;
    let ai = crate::develop::AiSource::recover(denoised, Cancel::new(cancel))?;
    let mut renderer = crate::develop::PreviewRenderer::default();
    renderer.set_ai(Some(ai));
    let rendered = renderer.render(&plain, &recipe, edge, None, Cancel::new(cancel))?;
    anyhow::ensure!(
        crate::storage::Identity::read(path)? == *file,
        "The photo's file changed while its preview was made"
    );
    rgb(rendered)
}
/// `path` developed with `edit` (see [`EditSource::recipe`]) within `edge` pixels:
/// a RAW from the fast half-size decode, a JPEG, PNG or TIFF from the file.
pub(super) fn develop_quickly(
    path: &Path,
    edit: Option<&EditSource>,
    edge: u32,
    cancel: &AtomicBool,
) -> anyhow::Result<image::RgbImage> {
    let (image, recipe) = open_quickly(path, edit, cancel)?;
    finish_quickly(&image, &recipe, edge, cancel)
}
/// `path` opened for a quick render, with the recipe `edit` gives it: a RAW from
/// the fast half-size decode, a JPEG, PNG or TIFF from the file.
fn open_quickly(
    path: &Path,
    edit: Option<&EditSource>,
    cancel: &AtomicBool,
) -> anyhow::Result<(crate::camera_data::CameraImage, Recipe)> {
    let opened = crate::photo::open_source(path, Cancel::new(cancel))?;
    let recipe = EditSource::recipe(edit, opened.metadata(), path, cancel)?;
    let image = match opened {
        crate::photo::Opened::Raw(raw) => {
            raw.develop(crate::camera_data::Decode::Half, Cancel::new(cancel))?
        }
        crate::photo::Opened::Rendered(image) => image,
    };
    anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
    Ok((image, recipe))
}
fn rgb(out: crate::rendered::Rendered) -> anyhow::Result<image::RgbImage> {
    image::RgbImage::from_raw(out.width, out.height, out.rgb8())
        .ok_or_else(|| anyhow::anyhow!("Invalid preview size"))
}
/// `image` rendered with `recipe` within `edge` pixels, given up once `cancel`
/// is set: a render that gives way to Develop or a close stops in its stages,
/// not only between them.
fn finish_quickly(
    image: &crate::camera_data::CameraImage,
    recipe: &Recipe,
    edge: u32,
    cancel: &AtomicBool,
) -> anyhow::Result<image::RgbImage> {
    rgb(crate::develop::render(
        image,
        &recipe.checked()?,
        edge,
        Cancel::new(cancel),
    )?)
}

#[derive(Default)]
pub(super) struct Progress {
    total: usize,
    completed: usize,
    failed: usize,
    cache_error: Option<String>,
}

impl Progress {
    pub(crate) fn queued(&mut self) {
        if self.completed == self.total {
            *self = Self::default();
        }
        self.total += 1;
    }

    pub(crate) fn finish(&mut self, result: &PreviewResult) {
        self.completed += 1;
        self.failed += usize::from(result.image.is_none());
        if let Some(error) = &result.cache_error {
            self.cache_error = Some(error.clone());
        }
    }

    /// A preview could not be kept in the cache.
    pub(crate) fn cache_failed(&mut self, error: String) {
        self.cache_error = Some(error);
    }

    /// Worth a status line: still working, or something could not be prepared.
    pub(crate) fn active(&self) -> bool {
        self.completed < self.total || self.failed > 0 || self.cache_error.is_some()
    }
    /// One line of small text, the same height as the status row it sits in.
    /// In a right-to-left layout, so the spinner comes first and sits right.
    /// `ahead`: edited previews of photos not shown yet are being rendered.
    pub(crate) fn show(&self, ui: &mut egui::Ui, edits_pending: usize, ahead: bool) {
        let building = self.completed < self.total || edits_pending > 0 || ahead;
        if building {
            crate::app::widgets::spinner(ui, 11.);
        }
        if edits_pending > 0 {
            ui.small(format!(
                "Rendering {}",
                plural(edits_pending, "edited preview", "edited previews")
            ))
            .on_hover_text(
                "Photos with Lightroom or RAWmakase edits are rendered with them, \
                 visible ones first. Each is kept, so this happens once.",
            );
        }
        if ahead && edits_pending == 0 {
            ui.small("Rendering edited previews ahead").on_hover_text(
                "While the Library is in front, edited previews of photos not shown yet \
                 are rendered and kept, so they are ready when you scroll to them.",
            );
        }
        if self.completed < self.total {
            ui.small(format!("Preparing previews {} / {}", self.completed, self.total))
                .on_hover_text("Cached previews load first; missing ones are built in the background as you browse.");
        }
        if self.failed > 0 {
            ui.small(format!("{} previews unavailable", self.failed))
                .on_hover_text(
                    "The original may be offline, damaged, or have no usable embedded preview.",
                );
        }
        if let Some(error) = &self.cache_error {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                egui::RichText::new("Preview cache could not be updated").small(),
            )
            .on_hover_text(error);
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use rawmakase_cancel::Cancel;
    use std::time::Duration;

    /// A grey JPEG at `path`, and a saved edit of it one stop brighter.
    pub(in crate::app::library) fn edited_jpeg(path: &Path) -> anyhow::Result<EditSource> {
        image::RgbImage::from_pixel(64, 48, image::Rgb([60, 60, 60])).save(path)?;
        let m = crate::camera_data::Metadata {
            source: crate::camera_data::Source::Rendered,
            ..Default::default()
        };
        let recipe = crate::model::recipe::Recipe {
            exposure: 1.,
            ..crate::model::recipe::Recipe::for_rendered(&m)
        };
        Ok(EditSource::Recipe(serde_json::to_string(&recipe)?))
    }

    #[test]
    fn an_edited_jpeg_renders_with_its_edit() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("photo.jpg");
        let edit = edited_jpeg(&path)?;
        let image = render_edited(&path, &edit, &AtomicBool::new(false))?;
        assert_eq!(image.dimensions(), (64, 48));
        // One stop brighter than the file's 60.
        assert!(
            image.pixels().all(|p| p.0[1] > 75),
            "{:?}",
            image.get_pixel(0, 0)
        );
        Ok(())
    }

    #[test]
    fn an_edited_preview_is_rendered_as_develops_fit_draws_it() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("textured.png");
        // Larger than the preview, with edges and gradients a few pixels wide
        // that a reduced copy keeps, as a photo's.
        image::RgbImage::from_fn(1800, 1200, |x, y| {
            let v = (((x / 9) * 5 + (y / 7) * 3) % 200 + 20) as u8;
            image::Rgb([v, 255 - v, (x / 4 % 128 + 64) as u8])
        })
        .save(&path)?;
        let m = crate::camera_data::Metadata {
            source: crate::camera_data::Source::Rendered,
            ..Default::default()
        };
        let mut recipe = Recipe {
            exposure: 0.5,
            shadows: 0.4,
            highlights: -0.3,
            ..Recipe::for_rendered(&m)
        };
        recipe.effects.clarity = 0.2;
        let edit = EditSource::Recipe(serde_json::to_string(&recipe)?);
        let cancel = AtomicBool::new(false);
        let preview = render_edited(&path, &edit, &cancel)?;
        let (image, recipe) = open_quickly(&path, Some(&edit), &cancel)?;
        let fitted = rgb(crate::develop::render_fitted(
            &image,
            &recipe,
            640,
            Cancel::new(&cancel),
        )?)?;
        assert_eq!(preview, fitted);
        // Close to the photo rendered whole and reduced, as before.
        let whole = finish_quickly(&image, &recipe, 640, &cancel)?;
        assert_eq!(preview.dimensions(), whole.dimensions());
        let difference = preview
            .as_raw()
            .iter()
            .zip(whole.as_raw())
            .map(|(a, b)| f64::from(a.abs_diff(*b)))
            .sum::<f64>()
            / preview.as_raw().len() as f64;
        assert!(difference < 4., "{difference}");
        Ok(())
    }

    #[test]
    fn an_edited_render_stops_once_cancelled() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("photo.png");
        let EditSource::Recipe(json) = edited_jpeg(&path)? else {
            unreachable!()
        };
        let recipe: Recipe = serde_json::from_str(&json)?;
        let crate::photo::Opened::Rendered(image) =
            crate::photo::open_source(&path, Cancel::NEVER)?
        else {
            unreachable!("a PNG opens rendered")
        };
        assert!(finish_quickly(&image, &recipe, 640, &AtomicBool::new(false)).is_ok());
        // Set as the render begins, as when the pace drops.
        assert!(finish_quickly(&image, &recipe, 640, &AtomicBool::new(true)).is_err());
        Ok(())
    }

    #[test]
    fn worker_persists_previews_and_reuses_them_when_original_is_offline() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("photo.png");
        image::RgbImage::new(720, 480).save(&source)?;
        let cache_path = directory.path().join("previews.sqlite3");
        let (tx, rx, _) = spawn(cache_path.clone(), egui::Context::default());
        tx.try_send(source.clone())?;
        let result = rx.recv_timeout(Duration::from_secs(10))?;
        assert_eq!(result.image.unwrap().dimensions(), (640, 427));
        assert!(result.cache_error.is_none());
        // Reopen through another worker: a memory-only result cannot pass this.
        std::fs::remove_file(&source)?;
        let (tx, rx, _) = spawn(cache_path, egui::Context::default());
        tx.try_send(source.clone())?;
        let result = rx.recv_timeout(Duration::from_secs(10))?;
        assert_eq!(result.path, source);
        assert!(result.image.is_some());
        assert!(result.cache_error.is_none());
        Ok(())
    }

    #[test]
    fn a_panicking_preview_is_unavailable_and_the_next_one_is_made() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let [panics, works] = ["panics.png", "works.png"].map(|name| directory.path().join(name));
        for path in [&panics, &works] {
            image::RgbImage::new(16, 16).save(path)?;
        }
        fn thumbnail(path: &Path) -> anyhow::Result<image::RgbImage> {
            assert!(!path.ends_with("panics.png"), "thumbnail panics");
            Ok(image::open(path)?.to_rgb8())
        }
        let (tx, rx, _) = spawn_with(
            directory.path().join("previews.sqlite3"),
            egui::Context::default(),
            thumbnail,
        );
        tx.try_send(panics.clone())?;
        tx.try_send(works.clone())?;
        // Made side by side, so in either order.
        let mut results = [
            rx.recv_timeout(Duration::from_secs(10))?,
            rx.recv_timeout(Duration::from_secs(10))?,
        ];
        results.sort_by(|a, b| a.path.cmp(&b.path));
        let [first, second] = results;
        assert_eq!(first.path, panics);
        assert!(first.image.is_none());
        assert_eq!(second.path, works);
        assert!(second.image.is_some());
        Ok(())
    }

    #[test]
    fn a_panicking_edited_preview_fails_and_the_next_one_is_rendered() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let [panics, works] = ["panics.png", "works.png"].map(|name| directory.path().join(name));
        for path in [&panics, &works] {
            image::RgbImage::new(16, 16).save(path)?;
        }
        fn render(path: &Path, _: &EditSource, _: &AtomicBool) -> anyhow::Result<image::RgbImage> {
            assert!(!path.ends_with("panics.png"), "render panics");
            Ok(image::open(path)?.to_rgb8())
        }
        let wanted = Wanted::default();
        wanted.lock().unwrap().extend([PhotoId(1), PhotoId(2)]);
        let (tx, rx, _) = spawn_edited_with(
            directory.path().join("previews.sqlite3"),
            wanted,
            egui::Context::default(),
            render,
        );
        let source = EditSource::Recipe("{}".into());
        let job = |id, path: &PathBuf| EditJob::Render {
            id,
            ticket: 0,
            path: path.clone(),
            source: source.clone(),
            ahead: false,
        };
        tx.send(job(PhotoId(1), &panics))?;
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10))?,
            EditResult::Failed(PhotoId(1), 0)
        ));
        tx.send(job(PhotoId(2), &works))?;
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10))?,
            EditResult::Ready(PhotoId(2), 0, _)
        ));
        Ok(())
    }

    /// Develop's render of a file replaced since is not kept as the new file's
    /// preview: it is stored under the file as it was rendered, or not at all.
    #[test]
    fn a_stored_render_of_a_file_replaced_since_is_not_kept() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("photo.png");
        image::RgbImage::new(16, 16).save(&source)?;
        let rendered = Stamp::read(&source)?;
        image::RgbImage::new(24, 24).save(&source)?;
        let cache = directory.path().join("previews.sqlite3");
        let (tx, rx, _) = spawn_edited(cache.clone(), Wanted::default(), egui::Context::default());
        tx.send(EditJob::Store {
            path: source.clone(),
            tag: "edit".into(),
            stamp: rendered,
            image: image::RgbImage::new(4, 4),
        })?;
        // Stores go first: once a later job is answered, the store is done.
        tx.send(EditJob::Render {
            id: PhotoId(1),
            ticket: 0,
            path: source.clone(),
            source: EditSource::Recipe("{}".into()),
            ahead: false,
        })?;
        match rx.recv_timeout(Duration::from_secs(10))? {
            EditResult::Skipped(PhotoId(1), 0) => {}
            EditResult::CacheError(error) => panic!("reported as a failure: {error}"),
            _ => panic!("unexpected result"),
        }
        assert!(
            PreviewCache::open(&cache)?
                .load_tagged(&source, "edit")?
                .is_none(),
            "kept as the replaced file's preview"
        );
        Ok(())
    }

    /// Stand-ins for the edited-preview renders: a plain one dark, one from AI
    /// Denoise's result bright, each counted by file and kind.
    static RENDERS: Mutex<Vec<(PathBuf, PreviewAi)>> = Mutex::new(Vec::new());
    fn renders_of(path: &Path, kind: PreviewAi) -> usize {
        let renders = RENDERS.lock().unwrap();
        renders
            .iter()
            .filter(|(p, k)| p == path && *k == kind)
            .count()
    }
    fn plain(path: &Path, _: &EditSource, _: &AtomicBool) -> anyhow::Result<image::RgbImage> {
        RENDERS
            .lock()
            .unwrap()
            .push((path.into(), PreviewAi::Plain));
        Ok(image::RgbImage::from_pixel(8, 8, image::Rgb([10; 3])))
    }
    fn with_ai(
        path: &Path,
        _: &crate::storage::Identity,
        _: &EditSource,
        _: &rawmakase_decode::ai_cache::AiEntry,
        _: &AtomicBool,
    ) -> anyhow::Result<image::RgbImage> {
        RENDERS
            .lock()
            .unwrap()
            .push((path.into(), PreviewAi::Finished));
        Ok(image::RgbImage::from_pixel(8, 8, image::Rgb([200; 3])))
    }
    /// An edit with AI Denoise on.
    fn with_ai_denoise() -> anyhow::Result<EditSource> {
        let mut recipe = Recipe::default();
        recipe.switch_ai_denoise(true);
        Ok(EditSource::Recipe(serde_json::to_string(&recipe)?))
    }
    /// Stores a finished AI Denoise result for the file at `path` in `cache`.
    fn finish_ai(cache: &AiCache, path: &Path) -> anyhow::Result<()> {
        let layout = rawmakase_decode::ai_denoise::Layout::new(4, 4, [0, 0]);
        let entry = rawmakase_decode::ai_cache::AiEntry {
            layout,
            gain: 1.,
            rgb: vec![0.5; 3 * layout.output_pixels()],
            clipped: vec![false; layout.packed_width * layout.packed_height],
        };
        let key = crate::app::ai_denoise::key(path, rawmakase_inference::Provider::Cpu)?;
        cache.store(&key, &entry)
    }
    /// What photo 1's preview, asked for with `ticket`, comes back as: whether it
    /// is bright (rendered from AI Denoise's result).
    fn preview_of(
        tx: &EditQueue,
        rx: &Receiver<EditResult>,
        path: &Path,
        source: &EditSource,
        ticket: u64,
    ) -> anyhow::Result<bool> {
        tx.send(EditJob::Render {
            id: PhotoId(1),
            ticket,
            path: path.into(),
            source: source.clone(),
            ahead: false,
        })?;
        match rx.recv_timeout(Duration::from_secs(10))? {
            EditResult::Ready(PhotoId(1), t, image) if t == ticket => {
                Ok(image.get_pixel(0, 0).0[0] > 100)
            }
            _ => anyhow::bail!("no preview"),
        }
    }

    /// An edit with AI Denoise shows its plain render until a finished result is
    /// cached, then the result's, kept apart from the plain one; offline, the one
    /// kept is still shown.
    #[test]
    fn an_ai_denoise_preview_follows_its_finished_result_and_stays_offline() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("photo.png");
        image::RgbImage::new(16, 16).save(&path)?;
        let ai = AiCache::new(directory.path().join("ai"), u64::MAX);
        let wanted = Wanted::default();
        wanted.lock().unwrap().insert(PhotoId(1));
        let (tx, rx, _) = spawn_edited_with_ai(
            directory.path().join("previews.sqlite3"),
            wanted,
            egui::Context::default(),
            plain,
            with_ai,
        );
        tx.set_ai_cache(ai.clone());
        let source = with_ai_denoise()?;
        assert!(
            !preview_of(&tx, &rx, &path, &source, 0)?,
            "plain without a result"
        );
        assert!(!preview_of(&tx, &rx, &path, &source, 1)?);
        assert_eq!(renders_of(&path, PreviewAi::Plain), 1, "the plain one kept");
        finish_ai(&ai, &path)?;
        assert!(
            preview_of(&tx, &rx, &path, &source, 2)?,
            "the result's once finished"
        );
        assert!(preview_of(&tx, &rx, &path, &source, 3)?);
        assert_eq!(
            renders_of(&path, PreviewAi::Finished),
            1,
            "the result's kept"
        );
        // Offline: no file to check a result against, the preview kept is shown.
        std::fs::rename(&path, directory.path().join("away.png"))?;
        assert!(preview_of(&tx, &rx, &path, &source, 4)?, "shown offline");
        assert_eq!(renders_of(&path, PreviewAi::Finished), 1);
        Ok(())
    }

    /// A finished result renders a RAW's preview from it: decoded at full size and
    /// the result assembled over it, as Develop shows it.
    #[test]
    fn a_raw_renders_its_preview_from_a_finished_result() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("photo.dng");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-a.dng"),
            &path,
        )?;
        let cancel = AtomicBool::new(false);
        let packed = crate::photo::open(&path)?.packed_bayer(Cancel::new(&cancel))?;
        let layout = packed.layout;
        // A flat result: the preview shows it, not the chart.
        let entry = rawmakase_decode::ai_cache::AiEntry {
            layout,
            gain: 1.,
            rgb: vec![0.3; 3 * layout.output_pixels()],
            clipped: packed.clipped,
        };
        let ai = AiCache::new(directory.path().join("ai"), u64::MAX);
        let key = crate::app::ai_denoise::key(&path, rawmakase_inference::Provider::Cpu)?;
        ai.store(&key, &entry)?;
        let mut recipe = Recipe::default();
        recipe.switch_ai_denoise(true);
        recipe.ai_denoise.amount = 1.;
        let source = EditSource::Recipe(serde_json::to_string(&recipe)?);
        let (plain, shows) = develop_for_preview(&path, &source, 256, None, &cancel)?;
        assert_eq!(shows, PreviewAi::Plain);
        let (denoised, shows) = develop_for_preview(&path, &source, 256, Some(&ai), &cancel)?;
        assert_eq!(shows, PreviewAi::Finished);
        assert_eq!(plain.dimensions(), denoised.dimensions());
        // Rendered from the flat result: far from the chart's own colours.
        let difference = plain
            .as_raw()
            .iter()
            .zip(denoised.as_raw())
            .map(|(a, b)| f64::from(a.abs_diff(*b)))
            .sum::<f64>()
            / plain.as_raw().len() as f64;
        assert!(difference > 30., "{difference}");
        Ok(())
    }

    /// A preview made from another model's result is not shown for this one's.
    #[test]
    fn a_preview_of_another_models_result_is_made_again() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("photo.png");
        image::RgbImage::new(16, 16).save(&path)?;
        let ai = AiCache::new(directory.path().join("ai"), u64::MAX);
        finish_ai(&ai, &path)?;
        let source = with_ai_denoise()?;
        let previews = directory.path().join("previews.sqlite3");
        PreviewCache::open(&previews)?.store_tagged(
            &path,
            &source.preview_tag_of(PreviewAi::Finished, "an-earlier-model"),
            &Stamp::read(&path)?,
            &image::RgbImage::from_pixel(8, 8, image::Rgb([90; 3])),
        )?;
        let wanted = Wanted::default();
        wanted.lock().unwrap().insert(PhotoId(1));
        let (tx, rx, _) =
            spawn_edited_with_ai(previews, wanted, egui::Context::default(), plain, with_ai);
        tx.set_ai_cache(ai);
        assert!(
            preview_of(&tx, &rx, &path, &source, 0)?,
            "rendered from this model's"
        );
        assert_eq!(renders_of(&path, PreviewAi::Finished), 1);
        Ok(())
    }

    #[test]
    fn edited_preview_cache_failures_are_reported() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("photo.png");
        image::RgbImage::new(16, 16).save(&source)?;
        let (tx, rx, _) = spawn_edited(
            directory.path().join("previews.sqlite3"),
            Wanted::default(),
            egui::Context::default(),
        );
        // Larger than the cache keeps.
        tx.send(EditJob::Store {
            stamp: Stamp::read(&source)?,
            path: source,
            tag: "edit".into(),
            image: image::RgbImage::new(2048, 8),
        })?;
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10))?,
            EditResult::CacheError(_)
        ));
        Ok(())
    }

    #[test]
    fn cache_failure_does_not_stop_previews_or_completion() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("photo.png");
        image::RgbImage::new(16, 16).save(&source)?;
        // A directory cannot be opened as a SQLite database.
        let (tx, rx, _) = spawn(directory.path().into(), egui::Context::default());
        let mut progress = Progress::default();
        for path in [source.clone(), directory.path().join("missing.ARW")] {
            tx.try_send(path)?;
            progress.queued();
        }
        // Made side by side, so in either order.
        let first = rx.recv_timeout(Duration::from_secs(10))?;
        let second = rx.recv_timeout(Duration::from_secs(10))?;
        let (made, missing) = if first.path == source {
            (first, second)
        } else {
            (second, first)
        };
        assert!(made.image.is_some());
        assert!(made.cache_error.is_some());
        assert!(missing.image.is_none());
        progress.finish(&made);
        assert_eq!((progress.completed, progress.total), (1, 2));
        progress.finish(&missing);
        assert_eq!(
            (progress.completed, progress.total, progress.failed),
            (2, 2, 1)
        );
        assert!(progress.cache_error.is_some());
        progress.queued();
        assert_eq!(
            (progress.completed, progress.total, progress.failed),
            (0, 1, 0)
        );
        Ok(())
    }

    /// Waits up to 20 s for `done`.
    fn until(done: impl Fn() -> bool) {
        let started = std::time::Instant::now();
        while !done() {
            assert!(started.elapsed() < Duration::from_secs(20), "timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn edited_png(directory: &Path, name: &str) -> anyhow::Result<PathBuf> {
        let path = directory.join(name);
        image::RgbImage::new(16, 16).save(&path)?;
        Ok(path)
    }

    fn render_job(id: i64, path: &Path, ahead: bool) -> EditJob {
        EditJob::Render {
            id: PhotoId(id),
            ticket: id as u64,
            path: path.into(),
            source: EditSource::Recipe(format!("{{\"photo\":{id}}}")),
            ahead,
        }
    }

    #[test]
    fn edited_previews_render_side_by_side_at_full_pace() -> anyhow::Result<()> {
        static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        // Ends once the other render is under way too.
        fn render(path: &Path, _: &EditSource, _: &AtomicBool) -> anyhow::Result<image::RgbImage> {
            RUNNING.fetch_add(1, Ordering::SeqCst);
            until(|| RUNNING.load(Ordering::SeqCst) >= 2);
            Ok(image::open(path)?.to_rgb8())
        }
        let directory = tempfile::tempdir()?;
        let wanted = Wanted::default();
        wanted.lock().unwrap().extend([PhotoId(1), PhotoId(2)]);
        let (tx, rx, _) = spawn_edited_with(
            directory.path().join("previews.sqlite3"),
            wanted,
            egui::Context::default(),
            render,
        );
        tx.set_pace(Pace::Full);
        for id in [1, 2] {
            tx.send(render_job(
                id,
                &edited_png(directory.path(), &format!("{id}.png"))?,
                false,
            ))?;
        }
        for _ in 0..2 {
            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(30))?,
                EditResult::Ready(..)
            ));
        }
        Ok(())
    }

    #[test]
    fn edited_previews_render_one_at_a_time_in_the_background() -> anyhow::Result<()> {
        static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        static MOST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        fn render(path: &Path, _: &EditSource, _: &AtomicBool) -> anyhow::Result<image::RgbImage> {
            let running = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
            MOST.fetch_max(running, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            RUNNING.fetch_sub(1, Ordering::SeqCst);
            Ok(image::open(path)?.to_rgb8())
        }
        let directory = tempfile::tempdir()?;
        let wanted = Wanted::default();
        wanted.lock().unwrap().extend((1..=3).map(PhotoId));
        let (tx, rx, _) = spawn_edited_with(
            directory.path().join("previews.sqlite3"),
            wanted,
            egui::Context::default(),
            render,
        );
        for id in 1..=3 {
            tx.send(render_job(
                id,
                &edited_png(directory.path(), &format!("{id}.png"))?,
                false,
            ))?;
        }
        for _ in 0..3 {
            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(30))?,
                EditResult::Ready(..)
            ));
        }
        assert_eq!(MOST.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn full_pace_renders_give_way_when_the_pace_drops() -> anyhow::Result<()> {
        static STARTED: AtomicBool = AtomicBool::new(false);
        fn render(
            _: &Path,
            _: &EditSource,
            cancel: &AtomicBool,
        ) -> anyhow::Result<image::RgbImage> {
            STARTED.store(true, Ordering::SeqCst);
            until(|| cancel.load(Ordering::Relaxed));
            anyhow::bail!("Cancelled")
        }
        let directory = tempfile::tempdir()?;
        let wanted = Wanted::default();
        wanted.lock().unwrap().insert(PhotoId(1));
        let (tx, rx, _) = spawn_edited_with(
            directory.path().join("previews.sqlite3"),
            wanted,
            egui::Context::default(),
            render,
        );
        tx.set_pace(Pace::Full);
        tx.send(render_job(
            1,
            &edited_png(directory.path(), "1.png")?,
            false,
        ))?;
        until(|| STARTED.load(Ordering::SeqCst));
        // Develop came to the front: asked again when shown.
        tx.set_pace(Pace::Background);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(30))?,
            EditResult::Skipped(PhotoId(1), 1)
        ));
        Ok(())
    }

    #[test]
    fn photos_asked_ahead_are_kept_and_then_shown_from_the_cache() -> anyhow::Result<()> {
        static RENDERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        fn render(path: &Path, _: &EditSource, _: &AtomicBool) -> anyhow::Result<image::RgbImage> {
            RENDERS.fetch_add(1, Ordering::SeqCst);
            Ok(image::open(path)?.to_rgb8())
        }
        let directory = tempfile::tempdir()?;
        let path = edited_png(directory.path(), "1.png")?;
        let wanted = Wanted::default();
        let (tx, rx, _) = spawn_edited_with(
            directory.path().join("previews.sqlite3"),
            wanted.clone(),
            egui::Context::default(),
            render,
        );
        // Not rendered ahead in the background.
        tx.send(render_job(1, &path, true))?;
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(30))?,
            EditResult::Skipped(PhotoId(1), 1)
        ));
        assert_eq!(RENDERS.load(Ordering::SeqCst), 0);
        tx.set_pace(Pace::Full);
        tx.send(render_job(1, &path, true))?;
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(30))?,
            EditResult::Kept(PhotoId(1), 1)
        ));
        // Shown, it comes from the cache.
        wanted.lock().unwrap().insert(PhotoId(1));
        tx.send(render_job(1, &path, false))?;
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(30))?,
            EditResult::Ready(PhotoId(1), 1, _)
        ));
        assert_eq!(RENDERS.load(Ordering::SeqCst), 1);
        // Asked ahead again, it is kept without a render.
        wanted.lock().unwrap().clear();
        tx.send(render_job(1, &path, true))?;
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(30))?,
            EditResult::Kept(PhotoId(1), 1)
        ));
        assert_eq!(RENDERS.load(Ordering::SeqCst), 1);
        Ok(())
    }

    /// Renders that only count themselves, for a job taken from `shared`.
    fn counting(
        shared: &Arc<Shared>,
        cache: Option<PreviewCache>,
        wanted: Wanted,
    ) -> (Renders, Receiver<EditResult>) {
        fn render(_: &Path, _: &EditSource, _: &AtomicBool) -> anyhow::Result<image::RgbImage> {
            RENDERED.fetch_add(1, Ordering::SeqCst);
            Ok(image::RgbImage::new(4, 4))
        }
        let (tx, rx) = mpsc::channel();
        let renders = Renders {
            background: None,
            full: None,
            cache: Arc::new(Mutex::new(cache)),
            cache_path: PathBuf::new(),
            wanted,
            results: Results {
                tx,
                shared: shared.clone(),
                ctx: egui::Context::default(),
            },
            render,
            render_ai: render_edited_with_ai,
        };
        (renders, rx)
    }
    static RENDERED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    #[test]
    fn photos_asked_ahead_are_not_rendered_once_they_lost_their_reason() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = edited_png(directory.path(), "1.png")?;
        let queued = |job| Queued {
            job,
            missed: false,
            dropped: false,
        };
        // No cache to keep it in: rendered when shown.
        let shared = Arc::<Shared>::default();
        shared.lock().pace = Pace::Full;
        shared.lock().jobs.push(queued(render_job(1, &path, true)));
        let (renders, rx) = counting(&shared, None, Wanted::default());
        renders.start(0);
        assert!(matches!(rx.try_recv(), Ok(EditResult::Kept(PhotoId(1), 1))));
        // Looked up at full pace, started once Develop came to the front.
        let shared = Arc::<Shared>::default();
        shared.lock().jobs.push(queued(render_job(2, &path, true)));
        let cache = PreviewCache::open(&directory.path().join("previews.sqlite3"))?;
        let (renders, rx) = counting(&shared, Some(cache), Wanted::default());
        renders.start(0);
        assert!(matches!(
            rx.try_recv(),
            Ok(EditResult::Skipped(PhotoId(2), 2))
        ));
        assert!(shared.lock().running.is_empty());
        // Its list gone while it was looked up (a miss), before its render began.
        let shared = Arc::<Shared>::default();
        let queue = EditQueue(shared.clone());
        queue.set_pace(Pace::Full);
        queue.send(render_job(4, &path, true))?;
        assert!(matches!(
            next_step(&shared, &Wanted::default(), 2),
            Step::Look(0, true)
        ));
        queue.drop_ahead();
        let cache = PreviewCache::open(&directory.path().join("empty.sqlite3"))?;
        let (renders, rx) = counting(&shared, Some(cache), Wanted::default());
        renders.look(0, true);
        assert!(matches!(
            rx.try_recv(),
            Ok(EditResult::Skipped(PhotoId(4), 4))
        ));
        assert!(shared.lock().running.is_empty());
        drop(queue);
        // Closed since it was looked up.
        let shared = Arc::<Shared>::default();
        shared.lock().jobs.push(queued(render_job(3, &path, false)));
        let (renders, rx) = counting(&shared, None, Wanted::default());
        shared.close();
        renders.start(0);
        assert!(rx.try_recv().is_err());
        assert!(shared.lock().running.is_empty());
        assert_eq!(RENDERED.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn photos_asked_ahead_for_a_list_gone_are_dropped_unless_shown() {
        let directory = Path::new("/nonexistent");
        let (queue, _) = EditQueue::detached();
        let wanted = Wanted::default();
        wanted.lock().unwrap().insert(PhotoId(2));
        queue.set_pace(Pace::Full);
        for (id, ahead) in [(1, true), (2, true)] {
            queue.send(render_job(id, directory, ahead)).unwrap();
        }
        queue.drop_ahead();
        // Sent after the list changed: kept.
        queue.send(render_job(3, directory, true)).unwrap();
        // Not rendered, even at full pace; one shown since is, as a photo shown.
        assert!(matches!(
            next_step(&queue.0, &wanted, 2),
            Step::Skip(PhotoId(1), 1)
        ));
        assert!(matches!(
            next_step(&queue.0, &wanted, 2),
            Step::Look(0, true)
        ));
        queue.0.lock().jobs.remove(0);
        assert!(matches!(
            next_step(&queue.0, &wanted, 2),
            Step::Look(0, true)
        ));
    }

    /// A file being rendered isn't rendered again beside it, so the two share
    /// one Camera Look fit rather than each making it. It is still looked up,
    /// and other files render.
    #[test]
    fn computers_with_less_memory_render_fewer_side_by_side() {
        const GIB: u64 = 1 << 30;
        let renders = |gib: f64| full_renders(Some((gib * GIB as f64) as u64), 17);
        assert_eq!(renders(8.), 2);
        // Less than the label says, as systems keep some back.
        assert_eq!(renders(7.6), 2);
        assert_eq!(renders(12.), 3);
        assert_eq!(renders(16.), 4);
        assert_eq!(renders(64.), 4);
        assert_eq!(renders(4.), 1);
        assert_eq!(renders(1.), 1);
        // Unknown: as an 8 GB computer.
        assert_eq!(full_renders(None, 17), 2);
        // Never more than the full pace's threads.
        assert_eq!(full_renders(Some(64 * GIB), 2), 2);
    }

    #[test]
    fn a_file_is_rendered_once_at_a_time() {
        let (one, other) = (Path::new("/photos/one.NEF"), Path::new("/photos/other.NEF"));
        let wanted = Wanted::default();
        wanted
            .lock()
            .unwrap()
            .extend([PhotoId(1), PhotoId(2), PhotoId(3)]);
        let queued = |job| Queued {
            job,
            missed: false,
            dropped: false,
        };
        let shared = Shared::default();
        {
            let mut state = shared.lock();
            state.pace = Pace::Full;
            // A virtual copy of `one` is rendering.
            state.running.push(Running {
                slot: 0,
                full: true,
                cancel: Arc::default(),
                path: one.into(),
            });
            state.jobs = vec![
                queued(render_job(2, other, false)),
                queued(render_job(3, one, false)),
            ];
        }
        // The latest, another copy of `one`: from the cache only.
        assert!(matches!(
            next_step(&shared, &wanted, 4),
            Step::Look(1, false)
        ));
        shared.lock().jobs[1].missed = true;
        // `other` renders beside it.
        assert!(matches!(
            next_step(&shared, &wanted, 4),
            Step::Look(0, true)
        ));
        shared.lock().jobs[0].missed = true;
        assert!(matches!(next_step(&shared, &wanted, 4), Step::Render(0)));
        // Once `one`'s render is done, its other copy renders.
        shared.lock().jobs.remove(0);
        shared.lock().running.clear();
        assert!(matches!(next_step(&shared, &wanted, 4), Step::Render(0)));
    }

    #[test]
    fn photos_shown_go_before_the_ones_asked_ahead() {
        let directory = Path::new("/nonexistent");
        let shared = Shared::default();
        let wanted = Wanted::default();
        wanted.lock().unwrap().extend([PhotoId(2), PhotoId(3)]);
        let queued = |job| Queued {
            job,
            missed: false,
            dropped: false,
        };
        {
            let mut state = shared.lock();
            state.pace = Pace::Full;
            state.jobs = vec![
                queued(render_job(1, directory, true)),
                queued(render_job(2, directory, false)),
                // Asked ahead and shown since: as a photo shown.
                queued(render_job(3, directory, true)),
                queued(render_job(4, directory, true)),
            ];
        }
        // The latest photo shown first, then the one before it.
        assert!(matches!(
            next_step(&shared, &wanted, 2),
            Step::Look(2, true)
        ));
        shared.lock().jobs[2].missed = true;
        assert!(matches!(
            next_step(&shared, &wanted, 2),
            Step::Look(1, true)
        ));
        shared.lock().jobs[1].missed = true;
        assert!(matches!(next_step(&shared, &wanted, 2), Step::Render(2)));
        shared.lock().jobs.remove(2);
        shared.lock().jobs.remove(1);
        // Then the ones asked ahead, in the order asked.
        assert!(matches!(
            next_step(&shared, &wanted, 2),
            Step::Look(0, true)
        ));
        // A photo out of view is answered without a render; at the background
        // pace so are the ones asked ahead.
        shared
            .lock()
            .jobs
            .push(queued(render_job(5, directory, false)));
        assert!(matches!(
            next_step(&shared, &wanted, 2),
            Step::Skip(PhotoId(5), 5)
        ));
        shared.lock().pace = Pace::Background;
        assert!(matches!(
            next_step(&shared, &wanted, 2),
            Step::Skip(PhotoId(1), 1)
        ));
        // With every slot taken a photo shown is still looked up.
        let mut state = shared.lock();
        state.jobs.clear();
        state.jobs.push(queued(render_job(2, directory, false)));
        state.running.push(Running {
            slot: 0,
            full: false,
            cancel: Arc::default(),
            path: PathBuf::from("/elsewhere.png"),
        });
        drop(state);
        assert!(matches!(
            next_step(&shared, &wanted, 2),
            Step::Look(0, false)
        ));
    }

    #[test]
    fn thumbnails_are_made_side_by_side() -> anyhow::Result<()> {
        static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        // Ends once the other thumbnail is under way too.
        fn thumbnail(path: &Path) -> anyhow::Result<image::RgbImage> {
            RUNNING.fetch_add(1, Ordering::SeqCst);
            until(|| RUNNING.load(Ordering::SeqCst) >= 2);
            Ok(image::open(path)?.to_rgb8())
        }
        let directory = tempfile::tempdir()?;
        let (tx, rx, _) = spawn_with(
            directory.path().join("previews.sqlite3"),
            egui::Context::default(),
            thumbnail,
        );
        for name in ["1.png", "2.png"] {
            tx.try_send(edited_png(directory.path(), name)?)?;
        }
        for _ in 0..2 {
            assert!(rx.recv_timeout(Duration::from_secs(30))?.image.is_some());
        }
        Ok(())
    }

    #[test]
    fn closing_cancels_the_edited_preview_under_way() -> anyhow::Result<()> {
        static STARTED: AtomicBool = AtomicBool::new(false);
        fn render(
            _: &Path,
            _: &EditSource,
            cancel: &AtomicBool,
        ) -> anyhow::Result<image::RgbImage> {
            STARTED.store(true, Ordering::Relaxed);
            // A render that only a cancel ends.
            while !cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1));
            }
            anyhow::bail!("Cancelled")
        }
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("photo.png");
        image::RgbImage::new(16, 16).save(&source)?;
        let wanted = Wanted::default();
        wanted.lock().unwrap().insert(PhotoId(1));
        let (tx, rx, thread) = spawn_edited_with(
            directory.path().join("previews.sqlite3"),
            wanted,
            egui::Context::default(),
            render,
        );
        let job = |ticket| EditJob::Render {
            id: PhotoId(1),
            ticket,
            path: source.clone(),
            source: EditSource::Recipe("{}".into()),
            ahead: false,
        };
        tx.send(job(0))?;
        let started = std::time::Instant::now();
        while !STARTED.load(Ordering::Relaxed) {
            assert!(started.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(1));
        }
        // Queued behind it, and never rendered.
        tx.send(job(1))?;
        tx.close();
        let stopping = crate::app::task::Stopping::new(Some(thread));
        let waited = crate::app::task::wait_for(vec![stopping], Duration::from_secs(20));
        assert_eq!(waited.detached, 0);
        assert!(matches!(
            rx.try_recv(),
            Ok(EditResult::Skipped(PhotoId(1), 0))
        ));
        assert!(rx.try_recv().is_err());
        Ok(())
    }
}
