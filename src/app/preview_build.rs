//! Build Standard-Sized Previews (issue #213): previews of the selected photos at
//! the Standard Preview Size, rendered in the background the way export renders
//! them and kept in the preview cache, for Develop to show while a photo opens.
//!
//! A request captures each photo's edit once. The build renders that edit and
//! stores its row under the edit's identity, so an edit made meanwhile only makes
//! the row obsolete. A result reaches the Library's grid only while the photo's
//! texture ticket and edit identity are still the ones captured.
//!
//! The build never makes the editor busy, so navigation is never blocked. It stops
//! between photos and inside the decode and the render when cancelled, and is
//! joined at exit within the shutdown deadline (docs/shutdown.md).
use super::Editor;
use crate::app::design::{Tone, radius, text};
use crate::app::theme;
use crate::app::widgets::plural;
use crate::camera_data::{Decode, Demosaic, Metadata};
use crate::catalog::preview_cache::{PreviewCache, PreviewKind, Stamp};
use crate::catalog::{CatalogLocation, PhotoId};
use crate::edits::EditRecord;
use crate::model::recipe::Recipe;
use crate::photo::Opened;
use crate::raw_defaults::DevelopDefaults;
use anyhow::{Context, ensure};
use eframe::egui::{self, Color32, Sense, Vec2};
use std::{
    collections::VecDeque,
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
};

/// Preferences > Standard Preview Size: the long edges offered.
pub(super) const STANDARD_SIZES: [u32; 3] = [1440, 2048, 2560];
pub(super) const DEFAULT_STANDARD_SIZE: u32 = 2048;
/// Preferences > Automatically Discard 1:1 Previews: the choices, in days
/// unused, as Lightroom offers them; `None` is Never.
pub(super) const DISCARD_CHOICES: [(Option<u32>, &str); 4] = [
    (Some(1), "After One Day"),
    (Some(7), "After One Week"),
    (Some(30), "After 30 Days"),
    (None, "Never"),
];
pub(super) const DEFAULT_DISCARD_DAYS: Option<u32> = Some(30);
/// The longest side a JPEG can hold.
const JPEG_LIMIT: u32 = 65_535;
/// The long edge of the Library thumbnail a build also gives the grid.
const THUMBNAIL_EDGE: u32 = 640;

/// What a stored preview's pixels depend on that is known without opening the
/// photo: its saved edit, else its Lightroom edit, else the raw defaults and the
/// release that resolves them; the demosaic and the RAW decoder
/// (`raw::developed`). The build and Develop's lookup both name rows by it.
/// Whether the file changed is the row's `Stamp`'s to say.
pub(super) fn identity(
    record: &EditRecord,
    defaults: &DevelopDefaults,
    library: &str,
    demosaic: Demosaic,
) -> String {
    let mut h = DefaultHasher::new();
    "preview-identity-1".hash(&mut h);
    record.recipe.hash(&mut h);
    record.local.hash(&mut h);
    record.lightroom().hash(&mut h);
    if record.recipe.is_none() && record.lightroom().is_none() {
        format!("{defaults:?}").hash(&mut h);
        // Which profile a default picks, and a photo without colour data's fitted
        // look, depend on the profile library.
        library.hash(&mut h);
        rawmakase_export::build_info::SOFTWARE.hash(&mut h);
    }
    format!("{:?}", demosaic).hash(&mut h);
    crate::raw::developed().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Whether a photo has an edit of its own, saved or from Lightroom; the grid
/// shows the embedded preview of a photo without one.
pub(super) fn has_edit(record: &EditRecord) -> bool {
    record.recipe.is_some() || record.lightroom().is_some()
}

/// One photo to build, with everything the build reads captured when it was asked for.
#[derive(Clone, Debug)]
pub(super) struct Item {
    pub catalog: CatalogLocation,
    pub photo: PhotoId,
    /// As the progress report names it.
    pub name: String,
    pub path: PathBuf,
    pub record: EditRecord,
    pub defaults: Arc<DevelopDefaults>,
    pub demosaic: Demosaic,
    /// Where a full-size decode is looked up first.
    pub decode_cache: crate::decode_cache::DecodeCache,
    pub identity: String,
    pub kind: PreviewKind,
    /// The long edge asked for; 0 for full size.
    pub edge: u32,
    /// The Library's texture ticket for the photo when it was asked for, for a
    /// photo the grid shows with its edit; the grid takes the result only while it
    /// is unchanged.
    pub ticket: Option<u64>,
}

/// How building one photo ended.
#[derive(Debug)]
pub(super) enum Outcome {
    /// Stored, with a thumbnail for the grid when the item has a ticket.
    Built(Option<image::RgbImage>),
    /// A row for this edit and size was there already.
    Fresh,
    Failed(String),
    Cancelled,
}

pub(super) struct Finished {
    pub item: Item,
    pub outcome: Outcome,
}

/// Renders `item` at its edge, giving up once the flag is set.
pub(super) type Render = fn(&Item, &AtomicBool) -> anyhow::Result<image::RgbImage>;

/// Builds and stores one photo's preview, unless a fresh one is there.
fn build_one(
    item: &Item,
    cache: Option<&mut PreviewCache>,
    render: Render,
    cancel: &AtomicBool,
) -> Outcome {
    let Some(cache) = cache else {
        return Outcome::Failed("the preview cache can't be opened".into());
    };
    if cache
        .sized_fresh(&item.path, &item.identity, item.kind, item.edge)
        .unwrap_or(false)
    {
        // Asked for again: as good as built now, for the automatic discard.
        let _ = cache.touch_sized(&item.path, &item.identity, item.kind);
        return Outcome::Fresh;
    }
    let stamp = match Stamp::read(&item.path) {
        Ok(stamp) => stamp,
        Err(e) => return Outcome::Failed(format!("{e:#}")),
    };
    let image = match render(item, cancel) {
        Ok(image) => image,
        Err(_) if cancel.load(Ordering::Relaxed) => return Outcome::Cancelled,
        Err(e) => return Outcome::Failed(format!("{e:#}")),
    };
    // The last moment to stop: nothing is stored.
    if cancel.load(Ordering::Relaxed) {
        return Outcome::Cancelled;
    }
    if image.width().max(image.height()) > JPEG_LIMIT {
        return Outcome::Failed(format!(
            "it is larger than a preview can be ({JPEG_LIMIT} pixels a side)"
        ));
    }
    if let Err(e) = cache.store_sized(
        &item.path,
        &item.identity,
        item.kind,
        &stamp,
        item.edge,
        &image,
    ) {
        return Outcome::Failed(format!("it could not be kept: {e:#}"));
    }
    // Recorded again with the row: an automatic discard that ran while this
    // waited may have forgotten the request.
    let _ = cache.record_intent(&item.catalog, item.photo, &item.path, item.kind);
    Outcome::Built(item.ticket.map(|_| thumbnail(&image)))
}

fn thumbnail(image: &image::RgbImage) -> image::RgbImage {
    let (w, h) = image.dimensions();
    let scale = THUMBNAIL_EDGE as f32 / w.max(h) as f32;
    if scale >= 1. {
        return image.clone();
    }
    let size = |n: u32| ((n as f32 * scale).round() as u32).max(1);
    image::imageops::resize(
        image,
        size(w),
        size(h),
        image::imageops::FilterType::Triangle,
    )
}

/// Renders a catalog photo as batch export does: its edit resolved through the
/// identity-checked record (a protected or unreadable edit fails, never falling
/// back to the defaults), Upright completed, then a cancellable render.
pub(super) fn render(item: &Item, cancel: &AtomicBool) -> anyhow::Result<image::RgbImage> {
    let opened = crate::photo::open_source(&item.path, cancel)
        .with_context(|| format!("{} can't be read", item.path.display()))?;
    let (profiles, _) = crate::camera_profiles::installed(opened.metadata());
    let profiles =
        crate::camera_look::with_default(&item.path, opened.metadata(), profiles, cancel);
    let mut recipe = item_recipe(item, opened.metadata(), &profiles)?;
    let image = match opened {
        Opened::Raw(raw) if needs_full(&recipe, &raw.metadata, item.edge) => {
            crate::export::job::decode_full(
                raw,
                &item.path,
                item.demosaic,
                &item.decode_cache,
                cancel,
            )?
        }
        Opened::Raw(raw) => raw.develop(Decode::Half, cancel)?,
        // A JPEG, PNG or TIFF opens at full size: there is no smaller decode.
        Opened::Rendered(image) => image,
    };
    ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
    develop(&mut recipe, &image, item.edge, cancel)
}

/// The edit `item` captured, worked out for the photo as export does.
fn item_recipe(
    item: &Item,
    m: &Metadata,
    profiles: &[Arc<crate::camera_profiles::CameraProfile>],
) -> anyhow::Result<Recipe> {
    Ok(crate::edits::resolve(&item.record, &item.path, m, profiles, &item.defaults)?.recipe)
}

/// `recipe` rendered on the decoded photo at `edge`, its Upright and Straighten Auto
/// completed first.
fn develop(
    recipe: &mut Recipe,
    image: &crate::camera_data::CameraImage,
    edge: u32,
    cancel: &AtomicBool,
) -> anyhow::Result<image::RgbImage> {
    crate::develop::upright::complete(recipe, image);
    let out = crate::develop::render_cancellable(image, &recipe.checked()?, edge, cancel)?;
    image::RgbImage::from_raw(out.width, out.height, out.rgb8()).context("Invalid preview size")
}

/// Whether the photo needs its full-size decode for `edge`: the half-size one,
/// cropped and transformed as `recipe` says, falls short of it. Upright or a
/// Straighten Auto still to be worked out always takes the full decode, which
/// Develop measures on, and completing it can shrink the photo (Constrain Crop).
pub(super) fn needs_full(recipe: &Recipe, m: &Metadata, edge: u32) -> bool {
    if edge == 0 || recipe.upright.needs_analysis() || recipe.straighten_pending() {
        return true;
    }
    let g = crate::develop::Geometry::for_metadata(m, recipe);
    g.width.max(g.height) / 2 < edge
}

/// How far the builds asked for have got.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Progress {
    pub done: usize,
    pub total: usize,
    /// Photos that could not be built: their names and why.
    pub failed: Vec<(String, String)>,
}
impl Progress {
    pub(super) fn running(&self) -> bool {
        self.done < self.total
    }
}

/// Cache upkeep the worker does between photos, so the interface never waits
/// for the preview database.
enum Op {
    RecordIntent(CatalogLocation, Vec<(PhotoId, PathBuf)>, PreviewKind),
    ForgetIntent(CatalogLocation, Vec<PhotoId>),
    /// Photos whose file was found moved, with its new path.
    MoveIntent(CatalogLocation, Vec<(PhotoId, PathBuf)>),
    /// Discard Previews: the rows of these files and the requests of these
    /// photos, of these kinds.
    Discard(
        CatalogLocation,
        Vec<PhotoId>,
        Vec<PathBuf>,
        Vec<PreviewKind>,
    ),
    Clear(PreviewKind),
    /// Automatically Discard 1:1 Previews: those unused for this long.
    Expire(PreviewKind, std::time::Duration),
    /// Edits saved: each photo with previews asked for (built, waiting or being
    /// built) is built again, unless its preview is still fresh.
    Refresh(Vec<Item>),
}

#[derive(Default)]
struct State {
    /// One entry per photo and kind, holding the latest request.
    waiting: VecDeque<Item>,
    running: Option<(PhotoId, PreviewKind)>,
    /// The flag the running build stops on; replaced when it is set.
    cancel: Arc<AtomicBool>,
    ops: VecDeque<Op>,
    closed: bool,
    progress: Progress,
    /// Bumped by Cancel and Discard: a refresh taken before then queues nothing.
    generation: u64,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

/// The build queue and the thread that works through it.
pub(super) struct Builder {
    shared: Arc<Shared>,
    results: Receiver<Finished>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Builder {
    /// Starts the worker on the cache at `cache_path`; `changed` is called after
    /// each photo.
    pub(super) fn start(
        cache_path: PathBuf,
        render: Render,
        changed: impl Fn() + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let (tx, results) = mpsc::channel();
        let worker = shared.clone();
        let thread = crate::raw::spawn_background(move || {
            let pool = crate::raw::background_pool(2, "preview-build").ok();
            let mut cache = PreviewCache::open(&cache_path).ok();
            while let Some(next) = worker.next() {
                let (item, cancel) = match next {
                    Next::Op(Op::Refresh(items), generation) => {
                        if let Some(cache) = &cache {
                            worker.refresh(cache, items, generation);
                        }
                        continue;
                    }
                    Next::Op(op, _) => {
                        if let Some(cache) = &mut cache {
                            // Best effort, and never worth ending the worker for.
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                maintain(cache, op)
                            }));
                        }
                        continue;
                    }
                    Next::Build(item, cancel) => (item, cancel),
                };
                let held = cache.as_mut();
                let build = || build_one(&item, held, render, &cancel);
                let caught =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &pool {
                        Some(pool) => pool.install(build),
                        None => build(),
                    }));
                let outcome = caught.unwrap_or_else(|panic| {
                    // The cache may have been left mid-write.
                    cache = PreviewCache::open(&cache_path).ok();
                    let message = crate::app::worker::panic_message(&*panic);
                    Outcome::Failed(format!("stopped unexpectedly: {message}"))
                });
                let finished = Finished { item, outcome };
                worker.finish(&finished);
                if tx.send(finished).is_err() {
                    break;
                }
                changed();
            }
        });
        Self {
            shared,
            results,
            thread: Some(thread),
        }
    }

    /// Queues `items`; a photo already waiting takes the newer request in its
    /// place, and one being built queues behind itself. Returns how many were
    /// added.
    pub(super) fn submit(&self, items: Vec<Item>) -> usize {
        let added = self
            .shared
            .state
            .lock()
            .expect("preview builds")
            .enqueue(items);
        self.shared.wake.notify_one();
        added
    }

    /// Stops the build under way if it is of `kind` (and of one of `photos`,
    /// when given); the others carry on.
    fn cancel_running(&self, photos: Option<&[PhotoId]>, kind: PreviewKind) {
        let mut state = self.shared.state.lock().expect("preview builds");
        if state.running.is_some_and(|(photo, running)| {
            running == kind && photos.is_none_or(|p| p.contains(&photo))
        }) {
            state.cancel.store(true, Ordering::Relaxed);
            state.cancel = Arc::default();
        }
    }

    /// Queues `expire` in place of any expiry not run yet, which an earlier
    /// setting asked for; `None` (Never) only removes those.
    fn replace_expiry(&self, expire: Option<Op>) {
        let mut state = self.shared.state.lock().expect("preview builds");
        state.ops.retain(|op| !matches!(op, Op::Expire(..)));
        state.ops.extend(expire);
        drop(state);
        self.shared.wake.notify_one();
    }

    fn maintain(&self, op: Op) {
        self.shared
            .state
            .lock()
            .expect("preview builds")
            .ops
            .push_back(op);
        self.shared.wake.notify_one();
    }

    /// Cancel: nothing waiting starts, and the photo being built stops without
    /// storing anything.
    pub(super) fn cancel(&self) {
        let mut state = self.shared.state.lock().expect("preview builds");
        let dropped = state.waiting.len();
        state.waiting.clear();
        // A refresh not yet looked at, or being looked at, would queue builds again.
        state.ops.retain(|op| !matches!(op, Op::Refresh(_)));
        state.generation += 1;
        state.progress.total -= dropped;
        state.cancel.store(true, Ordering::Relaxed);
        state.cancel = Arc::default();
    }

    /// Drops the builds not started of `photos` (all of them when `None`), of
    /// `kinds`.
    fn drop_waiting(&self, photos: Option<&[PhotoId]>, kinds: &[PreviewKind]) {
        let mut state = self.shared.state.lock().expect("preview builds");
        let before = state.waiting.len();
        state.waiting.retain(|item| {
            !(kinds.contains(&item.kind) && photos.is_none_or(|p| p.contains(&item.photo)))
        });
        let dropped = before - state.waiting.len();
        state.progress.total -= dropped;
        state.generation += 1;
        for op in &mut state.ops {
            if let Op::Refresh(items) = op {
                items.retain(|item| {
                    !(kinds.contains(&item.kind) && photos.is_none_or(|p| p.contains(&item.photo)))
                });
            }
        }
    }

    pub(super) fn progress(&self) -> Progress {
        self.shared
            .state
            .lock()
            .expect("preview builds")
            .progress
            .clone()
    }

    /// A finished report is put away; a running one stays.
    fn dismiss(&self) {
        let mut state = self.shared.state.lock().expect("preview builds");
        if !state.progress.running() {
            state.progress = Progress::default();
        }
    }

    /// The next finished photo, or `Err` once the worker is gone.
    fn try_recv(&self) -> Result<Finished, TryRecvError> {
        self.results.try_recv()
    }

    /// Stops the worker at exit: the photo being built is cancelled and nothing
    /// else starts. Returns the thread, to wait for.
    pub(super) fn close(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.shared.close();
        self.thread.take()
    }
}

impl Drop for Builder {
    /// Signals the worker; joining is the exit hook's to decide.
    fn drop(&mut self) {
        self.shared.close();
    }
}

enum Next {
    /// Upkeep, with the generation it was taken in.
    Op(Op, u64),
    Build(Item, Arc<AtomicBool>),
}

impl State {
    /// Queues `items`, each in the place of a waiting request for its photo and
    /// kind; returns how many were added.
    fn enqueue(&mut self, items: Vec<Item>) -> usize {
        if !self.progress.running() {
            self.progress = Progress::default();
        }
        let mut added = 0;
        for item in items {
            let waiting = self
                .waiting
                .iter_mut()
                .find(|w| w.photo == item.photo && w.kind == item.kind);
            match waiting {
                Some(waiting) => *waiting = item,
                None => {
                    self.waiting.push_back(item);
                    added += 1;
                }
            }
        }
        self.progress.total += added;
        added
    }
    /// Whether a build of the photo is waiting or running.
    fn pending(&self, photo: PhotoId, kind: PreviewKind) -> bool {
        self.running == Some((photo, kind))
            || self
                .waiting
                .iter()
                .any(|item| item.photo == photo && item.kind == kind)
    }
}

impl Shared {
    /// The items of a refresh that are to be built again.
    /// The cache and file checks run without the lock the interface queues
    /// through, as a file on a stalled share can hold them up.
    fn refresh(&self, cache: &PreviewCache, items: Vec<Item>, generation: u64) {
        let pending: Vec<bool> = match self.state.lock() {
            Ok(state) => items
                .iter()
                .map(|item| state.pending(item.photo, item.kind))
                .collect(),
            Err(_) => return,
        };
        let stale: Vec<Item> = items
            .into_iter()
            .zip(pending)
            .filter(|(item, pending)| {
                let asked = *pending
                    || cache
                        .has_intent(&item.catalog, item.photo, &item.path, item.kind)
                        .unwrap_or(false);
                asked
                    && !cache
                        .sized_fresh(&item.path, &item.identity, item.kind, item.edge)
                        .unwrap_or(false)
            })
            .map(|(item, _)| item)
            .collect();
        if let Ok(mut state) = self.state.lock()
            && state.generation == generation
        {
            state.enqueue(stale);
        }
    }
    /// Upkeep first, then the next photo; `None` once closed.
    fn next(&self) -> Option<Next> {
        let mut state = self.state.lock().ok()?;
        loop {
            // Upkeep asked for goes through even at exit: a Discard already
            // reported must not come back on the next launch.
            if let Some(op) = state.ops.pop_front() {
                return Some(Next::Op(op, state.generation));
            }
            if state.closed {
                return None;
            }
            if let Some(item) = state.waiting.pop_front() {
                state.running = Some((item.photo, item.kind));
                let cancel = state.cancel.clone();
                return Some(Next::Build(item, cancel));
            }
            state = self.wake.wait(state).ok()?;
        }
    }
    fn finish(&self, finished: &Finished) {
        if let Ok(mut state) = self.state.lock() {
            state.running = None;
            state.progress.done += 1;
            if let Outcome::Failed(why) = &finished.outcome {
                state
                    .progress
                    .failed
                    .push((finished.item.name.clone(), why.clone()));
            }
        }
    }
    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.waiting.clear();
            state.cancel.store(true, Ordering::Relaxed);
        }
        self.wake.notify_all();
    }
}

/// Upkeep is best effort: a failure leaves rows for the budget to evict, or a
/// request that only costs a rebuild.
fn maintain(cache: &mut PreviewCache, op: Op) {
    let _ = match op {
        Op::RecordIntent(catalog, photos, kind) => photos
            .iter()
            .try_for_each(|(photo, path)| cache.record_intent(&catalog, *photo, path, kind)),
        Op::ForgetIntent(catalog, photos) => cache.forget_intent(&catalog, &photos, None),
        Op::MoveIntent(catalog, photos) => photos
            .iter()
            .try_for_each(|(photo, path)| cache.move_intent(&catalog, *photo, path)),
        Op::Discard(catalog, photos, paths, kinds) => kinds.iter().try_for_each(|kind| {
            cache.forget_intent(&catalog, &photos, Some(*kind))?;
            cache.discard(&paths, *kind)
        }),
        Op::Clear(kind) => cache.clear(kind),
        Op::Expire(kind, unused) => cache.expire(kind, unused).map(|_| ()),
        Op::Refresh(_) => Ok(()),
    };
}

/// The editor's preview builds.
pub(super) struct PreviewBuilds {
    builder: Option<Builder>,
    /// The catalog the queued builds are for; another one cancels them.
    catalog: Option<CatalogLocation>,
    /// Preferences > Standard Preview Size.
    pub(super) standard_size: u32,
    /// Preferences > Automatically Discard 1:1 Previews, after this many days
    /// unused; `None` keeps them.
    pub(super) discard_one_to_one_after: Option<u32>,
    render: Render,
    cache_path: PathBuf,
}

impl PreviewBuilds {
    pub(super) fn new(standard_size: u32, discard_one_to_one_after: Option<u32>) -> Self {
        Self {
            builder: None,
            catalog: None,
            standard_size,
            discard_one_to_one_after,
            render,
            cache_path: PreviewCache::path(),
        }
    }
    /// Builds in the cache at `cache_path`, rendered by `render`.
    #[cfg(test)]
    pub(super) fn for_tests(cache_path: PathBuf, render: Render) -> Self {
        Self {
            render,
            cache_path,
            ..Self::new(DEFAULT_STANDARD_SIZE, DEFAULT_DISCARD_DAYS)
        }
    }
    fn builder(&mut self, ctx: &egui::Context) -> &Builder {
        let (path, render, ctx) = (self.cache_path.clone(), self.render, ctx.clone());
        self.builder
            .get_or_insert_with(|| Builder::start(path, render, move || ctx.request_repaint()))
    }
    pub(super) fn progress(&self) -> Option<Progress> {
        self.builder
            .as_ref()
            .map(Builder::progress)
            .filter(|p| p.total > 0 || !p.failed.is_empty())
    }
    /// Stops the worker at exit; its thread, to wait for.
    pub(super) fn close(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.builder.as_mut().and_then(Builder::close)
    }
    /// Where previews go, and how large, for `kind`.
    fn edge(&self, kind: PreviewKind) -> u32 {
        match kind {
            PreviewKind::Standard => self.standard_size,
            PreviewKind::OneToOne => 0,
        }
    }
}

/// How a request finds out which photos are online.
#[derive(Clone, Copy)]
enum Files {
    /// Asks the file system, as a request the user made does.
    Check,
    /// Goes by what the Library last found, as work done on the way to another
    /// photo does: a stalled share must not hold up navigation. The worker reads
    /// each file's stamp anyway.
    Known,
}

/// What a build request did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct Queued {
    pub queued: usize,
    /// Offline photos, files Develop can't open, and JPEG, PNG, TIFF, WebP,
    /// AVIF, HEIF or JPEG XL files without an edit, which are their own
    /// previews.
    pub skipped: usize,
}

impl Editor {
    /// Build Standard-Sized Previews (or 1:1) for `ids`: each available RAW with
    /// its edit as the catalog has it now, the open photo's saved first. Offline
    /// photos and other files are skipped and counted; fresh previews are found
    /// by the worker and cost nothing.
    pub(in crate::app) fn build_previews(
        &mut self,
        ids: &[PhotoId],
        kind: PreviewKind,
    ) -> Result<Queued, String> {
        if self.library.is_none() {
            return Err("No catalog is open".into());
        }
        if self
            .document
            .catalog_photo
            .is_some_and(|open| ids.contains(&open))
            // Unsaved, the build would show the open photo's edit as last saved.
            && !self.flush()
        {
            return Err(format!(
                "the open photo's edit could not be saved: {}",
                self.messages.line()
            ));
        }
        let (items, skipped) = self.build_items(ids, kind, Files::Check)?;
        let catalog = self
            .library
            .as_ref()
            .expect("checked above")
            .session
            .catalog
            .location()
            .clone();
        let intent = items
            .iter()
            .map(|item| (item.photo, item.path.clone()))
            .collect();
        let builder = self.catalog_builder(&catalog);
        builder.maintain(Op::RecordIntent(catalog, intent, kind));
        let queued = builder.submit(items);
        Ok(Queued { queued, skipped })
    }

    /// The builder, for builds in `catalog`; those of another catalog are cancelled.
    fn catalog_builder(&mut self, catalog: &CatalogLocation) -> &Builder {
        let ctx = self.context.clone();
        let builds = &mut self.preview_builds;
        if builds.catalog.as_ref() != Some(catalog) {
            if let Some(builder) = &builds.builder {
                builder.cancel();
            }
            builds.catalog = Some(catalog.clone());
        }
        builds.builder(&ctx)
    }

    /// What building `ids` would capture now: an item per available RAW or edited
    /// JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL, and how many others were skipped.
    fn build_items(
        &self,
        ids: &[PhotoId],
        kind: PreviewKind,
        files: Files,
    ) -> Result<(Vec<Item>, usize), String> {
        let Some(library) = &self.library else {
            return Err("No catalog is open".into());
        };
        let catalog = library.session.catalog.location().clone();
        let mut chosen = Vec::new();
        let mut skipped = 0;
        for &id in ids {
            match library.photo(id) {
                Some(photo)
                    if match files {
                        Files::Check => library.export_refusal(id).is_none(),
                        Files::Known => library.known_developable(id),
                    } =>
                {
                    let name = format!(
                        "{}{}",
                        photo.filename,
                        crate::app::library::copy_suffix(photo)
                    );
                    chosen.push((id, name, photo.path.clone()));
                }
                Some(_) => skipped += 1,
                None => {}
            }
        }
        let photo_ids: Vec<PhotoId> = chosen.iter().map(|(id, ..)| *id).collect();
        let records = library
            .session
            .catalog
            .photo_records(&photo_ids)
            .map_err(|e| format!("The catalog could not be read: {e:#}"))?;
        let edge = self.preview_builds.edge(kind);
        let mut items = Vec::with_capacity(chosen.len());
        for ((photo, name, path), record) in chosen.into_iter().zip(records) {
            let edited = has_edit(&record.edit);
            // A JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL without an edit is its own
            // preview.
            if !edited && !crate::storage::is_raw(&path) {
                skipped += 1;
                continue;
            }
            items.push(Item {
                catalog: catalog.clone(),
                photo,
                name,
                identity: identity(
                    &record.edit,
                    &self.raw_defaults,
                    &self.profile_library,
                    self.demosaic,
                ),
                ticket: edited.then(|| library.edited_ticket(photo)).flatten(),
                record: record.edit,
                path,
                defaults: self.raw_defaults.clone(),
                demosaic: self.demosaic,
                decode_cache: self.decode_cache.clone(),
                kind,
                edge,
            });
        }
        Ok((items, skipped))
    }

    /// Edits of `ids` were saved: the photos whose previews were asked for, and
    /// are no longer fresh, are built again with their new edits.
    pub(super) fn refresh_previews(&mut self, ids: &[PhotoId]) {
        let Some(library) = &self.library else {
            return;
        };
        let catalog = library.session.catalog.location().clone();
        let mut items = Vec::new();
        for kind in PreviewKind::ALL {
            match self.build_items(ids, kind, Files::Known) {
                Ok((more, _)) => items.extend(more),
                Err(_) => return,
            }
        }
        if !items.is_empty() {
            self.catalog_builder(&catalog).maintain(Op::Refresh(items));
        }
    }

    /// The photos `build_previews` acts on, as Export: the Library's selection,
    /// or in Develop the Filmstrip's when the open photo is part of it, else the
    /// open photo.
    pub(in crate::app) fn preview_command_scope(&self) -> Vec<PhotoId> {
        let open = self.document.catalog_photo;
        match &self.library {
            Some(l) if self.module == super::Module::Library => l.selected_photos(),
            Some(l) => match open {
                Some(open) if l.is_selected(open) => l.selected_photos(),
                Some(open) => vec![open],
                None => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    /// The thumbnail menu's Build Standard-Sized Previews.
    pub(super) fn build_previews_from_menu(&mut self, ids: &[PhotoId], kind: PreviewKind) {
        match self.build_previews(ids, kind) {
            Ok(queued) => self.messages.note(queued_message(queued)),
            Err(e) => self.messages.fail(format!("Previews not built: {e}")),
        }
    }

    /// Discard 1:1 Previews, or Standard and 1:1: the files' built previews of
    /// `kinds` and the photos' requests go, and their builds not yet started.
    pub(super) fn discard_previews(&mut self, ids: &[PhotoId], kinds: &[PreviewKind]) {
        let Some(library) = &self.library else {
            return;
        };
        let catalog = library.session.catalog.location().clone();
        let paths = ids
            .iter()
            .filter_map(|id| Some(library.photo(*id)?.path.clone()))
            .collect();
        let ctx = self.context.clone();
        let builder = self.preview_builds.builder(&ctx);
        builder.drop_waiting(Some(ids), kinds);
        for kind in kinds {
            builder.cancel_running(Some(ids), *kind);
        }
        builder.maintain(Op::Discard(catalog, ids.to_vec(), paths, kinds.to_vec()));
        let which = if kinds == [PreviewKind::OneToOne] {
            "1:1 previews"
        } else {
            "previews"
        };
        self.messages.note(format!(
            "Discarded the {which} of {}",
            plural(ids.len(), "photo", "photos")
        ));
    }

    /// A photo leaving the catalog takes its requests along, so a later photo
    /// given its id starts without any.
    /// Photos whose file was found moved: their queued builds of the old
    /// file are dropped, and their requests follow the file.
    pub(super) fn move_preview_intents(
        &mut self,
        catalog: &CatalogLocation,
        moved: &[(PhotoId, PathBuf)],
    ) {
        if moved.is_empty() {
            return;
        }
        let ids: Vec<PhotoId> = moved.iter().map(|(id, _)| *id).collect();
        let ctx = self.context.clone();
        let builder = self.preview_builds.builder(&ctx);
        builder.drop_waiting(Some(&ids), &PreviewKind::ALL);
        for kind in PreviewKind::ALL {
            builder.cancel_running(Some(&ids), kind);
        }
        builder.maintain(Op::MoveIntent(catalog.clone(), moved.to_vec()));
    }
    pub(super) fn forget_preview_intent(&mut self, id: PhotoId) {
        let Some(library) = &self.library else {
            return;
        };
        let catalog = library.session.catalog.location().clone();
        let ctx = self.context.clone();
        let builder = self.preview_builds.builder(&ctx);
        builder.drop_waiting(Some(&[id]), &PreviewKind::ALL);
        builder.maintain(Op::ForgetIntent(catalog, vec![id]));
    }

    /// Preferences' Clear Standard Previews and Clear 1:1 Previews; the
    /// builds of that kind not started go too.
    pub(super) fn clear_previews(&mut self, kind: PreviewKind) {
        let ctx = self.context.clone();
        let builder = self.preview_builds.builder(&ctx);
        builder.drop_waiting(None, &[kind]);
        builder.cancel_running(None, kind);
        builder.maintain(Op::Clear(kind));
    }

    /// Automatically Discard 1:1 Previews, as Preferences sets it: run when the
    /// app starts and when the setting changes.
    pub(super) fn expire_previews(&mut self) {
        let expire = self.preview_builds.discard_one_to_one_after.map(|days| {
            Op::Expire(
                PreviewKind::OneToOne,
                std::time::Duration::from_secs(u64::from(days) * 86_400),
            )
        });
        let ctx = self.context.clone();
        self.preview_builds.builder(&ctx).replace_expiry(expire);
    }

    /// Takes finished builds, every frame. Another catalog cancels the builds of
    /// the last one, and their late results are dropped.
    pub(super) fn poll_preview_builds(&mut self) {
        let current = self
            .library
            .as_ref()
            .map(|l| l.session.catalog.location().clone());
        let builds = &mut self.preview_builds;
        if builds.catalog.is_some() && builds.catalog != current {
            if let Some(builder) = &builds.builder {
                builder.cancel();
            }
            builds.catalog = None;
        }
        let Some(builder) = &builds.builder else {
            return;
        };
        let mut finished = Vec::new();
        loop {
            match builder.try_recv() {
                Ok(done) => finished.push(done),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // The worker is gone: nothing more will finish.
                    builds.builder = None;
                    builds.catalog = None;
                    self.messages.fail("Previews stopped being built");
                    break;
                }
            }
        }
        for done in finished {
            if let Outcome::Built(Some(thumbnail)) = done.outcome {
                self.show_built(&done.item, &thumbnail);
            }
        }
    }

    /// Shows a build in the grid while it is still the photo's latest: the
    /// texture ticket it was given is current, and the photo's edit is still the
    /// one it was built from.
    fn show_built(&mut self, item: &Item, thumbnail: &image::RgbImage) {
        let Some(library) = &mut self.library else {
            return;
        };
        // A relinked folder reloads the Library, whose tickets start again: the
        // photo must still be the file built.
        if *library.session.catalog.location() != item.catalog
            || item.ticket.is_none()
            || library.edited_ticket(item.photo) != item.ticket
            || library.photo(item.photo).map(|p| &p.path) != Some(&item.path)
        {
            return;
        }
        let Ok(record) = library.session.catalog.edit_record(item.photo) else {
            return;
        };
        if identity(
            &record,
            &self.raw_defaults,
            &self.profile_library,
            self.demosaic,
        ) == item.identity
        {
            library.show_built_preview(&self.context, item.photo, thumbnail);
        }
    }

    /// "Building previews 12 / 150" over a bar, with a button that cancels; after
    /// a build that could not do everything, what it could not do, until clicked.
    pub(super) fn preview_build_progress(&mut self, ui: &mut egui::Ui) {
        let Some(progress) = self.preview_builds.progress() else {
            return;
        };
        let palette = theme::palette(ui.ctx());
        if !progress.running() {
            if progress.failed.is_empty() {
                return;
            }
            let line = egui::RichText::new(format!(
                "{} could not be built",
                plural(progress.failed.len(), "preview", "previews")
            ))
            .size(text::SMALL)
            .color(Color32::from_rgb(230, 170, 100));
            let reasons = progress
                .failed
                .iter()
                .map(|(name, why)| format!("{name}: {why}"))
                .collect::<Vec<_>>()
                .join("\n");
            if ui
                .add(egui::Button::new(line).frame(false))
                .on_hover_text(format!("{reasons}\n\nClick to dismiss"))
                .clicked()
                && let Some(builder) = &self.preview_builds.builder
            {
                builder.dismiss();
            }
            return;
        }
        let (rect, _) = ui.allocate_exact_size(Vec2::new(190., 28.), Sense::hover());
        let painter = ui.painter();
        let current = (progress.done + 1).min(progress.total);
        painter.text(
            egui::pos2(rect.left(), rect.top() + 7.),
            egui::Align2::LEFT_CENTER,
            format!("Building previews {current} / {}", progress.total),
            egui::FontId::proportional(text::SMALL),
            palette.tone(Tone::SOFT),
        );
        let fraction = progress.done as f32 / progress.total.max(1) as f32;
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.top() + 17.),
            Vec2::new(rect.width() - 26., 4.),
        );
        painter.rect_filled(bar, radius::SMALL, palette.tone(Tone::OUTLINE));
        painter.rect_filled(
            egui::Rect::from_min_size(bar.min, Vec2::new(bar.width() * fraction.min(1.), 4.)),
            2.,
            Color32::from_rgb(110, 150, 190),
        );
        let close = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 9., rect.center().y),
            Vec2::splat(18.),
        );
        let cancel = ui
            .interact(close, ui.id().with("cancel-preview-build"), Sense::click())
            .on_hover_text("Stop building previews");
        let color = palette.tone(if cancel.hovered() {
            Tone::STRONG
        } else {
            Tone::SECONDARY
        });
        crate::app::icons::paint_at(
            ui.painter(),
            crate::app::icons::Icon::Close,
            close.center(),
            13.,
            color,
        );
        if cancel.clicked()
            && let Some(builder) = &self.preview_builds.builder
        {
            builder.cancel();
        }
    }
}

/// "Building 12 previews · 3 skipped (offline, not editable, or an unedited
/// file that is its own preview)".
pub(super) fn queued_message(queued: Queued) -> String {
    let mut line = match queued.queued {
        0 => "No previews to build".to_string(),
        n => format!("Building {}", plural(n, "preview", "previews")),
    };
    if queued.skipped > 0 {
        line.push_str(&format!(
            " · {} skipped (offline, not editable, or an unedited file that is its own preview)",
            queued.skipped
        ));
    }
    line
}

#[cfg(test)]
mod tests;
