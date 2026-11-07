//! The edited-preview scheduler: the queue, its pace, the photos asked ahead and the
//! worker rendering them.
use super::render::{finished_entry, plain_of, preview_tags, render_edited, render_edited_with_ai};
use super::source::{EditSource, PreviewAi};
use super::thumbnails::Worker;
use crate::catalog::PhotoId;
use crate::catalog::preview_cache::{PreviewCache, Stamp};
use eframe::egui;
use rawmakase_cancel::Cancel;
use rawmakase_decode::ai::cache::AiCache;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub(in crate::app::library) enum EditJob {
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
pub(in crate::app::library) type Wanted = Arc<Mutex<HashSet<PhotoId>>>;
/// What an edited preview job came to, by photo and the ticket it was asked with.
pub(in crate::app::library) enum EditResult {
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
type RenderEdited = fn(&Path, &EditSource, Cancel<'_>) -> anyhow::Result<image::RgbImage>;
/// Renders a preview of an edit with AI Denoise from a finished result: the file's
/// version the result was made for, and the result.
type RenderWithAi = fn(
    &Path,
    &crate::storage::Identity,
    &EditSource,
    &rawmakase_decode::ai::cache::AiEntry,
    Cancel<'_>,
) -> anyhow::Result<image::RgbImage>;
/// Where edited preview jobs go, and how fast they are rendered. Dropping it
/// closes the worker, as [`EditQueue::close`] does.
pub(in crate::app::library) struct EditQueue(Arc<Shared>);
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
pub(in crate::app::library) struct Closed;
impl std::fmt::Display for Closed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The edited preview worker is closed")
    }
}
impl std::error::Error for Closed {}
impl EditQueue {
    pub(in crate::app::library) fn send(&self, job: EditJob) -> Result<(), Closed> {
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
    pub(in crate::app::library) fn set_pace(&self, pace: Pace) {
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
    pub(in crate::app::library) fn set_ai_cache(&self, cache: AiCache) {
        self.0.lock().ai = Some(cache);
    }
    /// Drops the photos asked ahead and not started, for a list shown no
    /// longer: each is answered `Skipped` unless it is shown by then. Renders
    /// already under way finish.
    pub(in crate::app::library) fn drop_ahead(&self) {
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
    pub(in crate::app::library) fn close(&self) {
        self.0.close();
    }
    /// A queue no worker takes from, and what is sent to it, for tests.
    #[cfg(test)]
    pub(in crate::app::library) fn detached() -> (Self, Sent) {
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
    pub(in crate::app::library) fn try_recv(&self) -> Option<EditJob> {
        let mut state = self.0.lock();
        (!state.jobs.is_empty()).then(|| state.jobs.remove(0).job)
    }
    /// How many jobs sent and not taken are dropped.
    pub(in crate::app::library) fn dropped(&self) -> usize {
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
/// Threads the full pace renders on: every core but one. Editable preview
/// builds take the same at their full pace.
pub(in crate::app::library) fn full_threads() -> usize {
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
pub(in crate::app::library) fn spawn_edited(
    cache_path: PathBuf,
    wanted: Wanted,
    ctx: egui::Context,
) -> Worker<EditQueue, EditResult> {
    spawn_edited_with(cache_path, wanted, ctx, render_edited)
}
pub(super) fn spawn_edited_with(
    cache_path: PathBuf,
    wanted: Wanted,
    ctx: egui::Context,
    render: RenderEdited,
) -> Worker<EditQueue, EditResult> {
    spawn_edited_with_ai(cache_path, wanted, ctx, render, render_edited_with_ai)
}
pub(super) fn spawn_edited_with_ai(
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
                        render_ai(&path, &file, &source, &entry, Cancel::new(&cancel)).ok()?,
                        PreviewAi::Finished,
                    ),
                    None => (
                        render(&path, &source, Cancel::new(&cancel)).ok()?,
                        plain_of(&source),
                    ),
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

#[cfg(test)]
#[path = "edits_tests.rs"]
mod tests;
