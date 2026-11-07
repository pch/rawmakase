//! Build Editable Previews and Discard Editable Previews: a smaller copy of each
//! chosen photo (2560 pixels on its long edge, [`editable_preview`]), built while
//! its original is online, so that a later release can edit the photo while the
//! original is offline. Editable previews are not a cache: once the original is
//! offline one can't be built again, so the catalog keeps a row for each
//! (`catalog::editable_previews`), and only RAWmakase's own files are ever deleted.
//!
//! A request captures each item once: the master photo (a virtual copy asks for
//! its master's), the path read and the demosaic. The worker reads the original's
//! [`Identity`] once, before anything else reads the file, and the same identity
//! keys the decode cache lookup, names the file, goes in its header and is what
//! the catalog records; the file is read again at the end, and a build of a file
//! that changed meanwhile is dropped. The catalog's record refuses a photo that
//! left the catalog or was moved, and a refused build's file is removed through
//! the catalog, which never deletes a file a row names.
//!
//! The worker has its own catalog connection, as autosave has, so the interface
//! never waits on a build's catalog reads and writes. Discard runs on the
//! interface's connection; it counts itself in the queue's state first, and a
//! build of a photo discarded after it was queued records nothing, or, when its
//! record raced the discard, discards it again (see [`State::discarded_since`]).
//!
//! The build never makes the editor busy. It stops between photos and inside the
//! decode and the build when cancelled, and is joined at exit within the shutdown
//! deadline (docs/shutdown.md). Quitting cancels it: nothing is lost, as the
//! original it was built from is still there to build from again.
use super::{Progress as PreviewProgress, Queued};
use crate::app::Editor;
use crate::app::library::Library;
use crate::app::ui::widgets::plural;
use crate::camera_data::{CameraImage, Demosaic, Metadata};
use crate::camera_look::DefaultFit;
use crate::camera_profiles::CameraProfile;
use crate::catalog::{
    BuiltEditablePreview, Catalog, CatalogLocation, PhotoId, editable_preview_file_name,
};
use crate::editable_preview;
use crate::exif::CameraExif;
use crate::storage::Identity;
use anyhow::{Context, ensure};
use rawmakase_cancel::Cancel;
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
};

/// One photo to build, with what the build reads captured when it was asked for.
#[derive(Clone, Debug)]
pub(in crate::app) struct Item {
    pub catalog: CatalogLocation,
    /// The master photo: a virtual copy's request is its master's.
    pub photo: PhotoId,
    /// As the progress report names it.
    pub name: String,
    /// The original's path as the catalog had it when asked: the file read, and
    /// the path the build is recorded for.
    pub path: PathBuf,
    /// The demosaic the original is decoded with. An editable preview keeps the
    /// demosaic it was built with, and its identity doesn't include it, as
    /// [`Demosaic`] has a single variant; revisit both if a second one is added.
    pub demosaic: Demosaic,
    /// Where the full-size decode is looked up first.
    pub decode_cache: crate::decode_cache::DecodeCache,
    /// The discards counted when it was queued.
    discards: u64,
}

/// The original, opened and decoded in full, with what the build keeps of it.
pub(in crate::app) struct Original {
    pub image: CameraImage,
    pub exif: Option<CameraExif>,
    /// The profiles listed for the photo, which decide whether it starts from a
    /// fitted Camera Look.
    pub profiles: Vec<Arc<CameraProfile>>,
}

/// What a build does that needs a real photo, apart, so tests can stand in for it.
#[derive(Clone, Copy)]
pub(in crate::app) struct Steps {
    /// Opens the original and decodes it in full ([`open`]).
    pub open: fn(&Item, &Identity, Cancel<'_>) -> anyhow::Result<Original>,
    /// The photo's default Camera Look ([`crate::camera_look::default_fit`]).
    pub fit: fn(&Path, &Metadata, &[Arc<CameraProfile>], Cancel<'_>) -> DefaultFit,
}

impl Steps {
    pub(in crate::app) const REAL: Steps = Steps {
        open,
        fit: crate::camera_look::default_fit,
    };
}

/// Opens the RAW at `item`'s path and decodes it in full, as Develop's image is
/// decoded: the decode cache's copy of the version `identity` names (highlights
/// recovered) when it has one, else a decode, which is not stored, so a batch
/// never fills the cache, as exports don't.
pub(in crate::app) fn open(
    item: &Item,
    identity: &Identity,
    cancel: Cancel<'_>,
) -> anyhow::Result<Original> {
    let raw = crate::photo::open(&item.path)
        .with_context(|| format!("{} can't be read", item.path.display()))?;
    let (profiles, _) = crate::camera_profiles::installed(&raw.metadata);
    let exif = crate::photo::camera_exif(&item.path);
    let full = crate::decode::FullSize::of_version(
        Some(identity),
        item.demosaic,
        item.decode_cache.clone(),
    );
    let image = full
        .get(|| Ok(raw), crate::decode::DecodePolicy::Export, cancel)?
        .context("The photo wasn't decoded")?;
    Ok(Original {
        image,
        exif,
        profiles,
    })
}

/// How building one photo ended.
#[derive(Debug, PartialEq)]
pub(in crate::app) enum Outcome {
    /// Built and recorded; for a photo that starts from a fitted Camera Look and
    /// has none stored, why.
    Built {
        look_missing: Option<String>,
    },
    /// Its editable preview was built from this original already.
    Unchanged,
    Failed(String),
    /// Cancelled, or discarded while it was built: nothing recorded.
    Cancelled,
}

pub(in crate::app) struct Finished {
    pub item: Item,
    pub outcome: Outcome,
}

/// How far the builds asked for have got.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::app) struct Progress {
    pub done: usize,
    pub total: usize,
    /// Photos that could not be built: their names and why.
    pub failed: Vec<(String, String)>,
    /// Photos built without the Camera Look they start from: names and why.
    pub without_look: Vec<(String, String)>,
}
impl Progress {
    pub(in crate::app) fn running(&self) -> bool {
        self.done < self.total
    }
}

#[derive(Default)]
struct State {
    waiting: VecDeque<Item>,
    running: Option<PhotoId>,
    /// The flag the running build stops on; replaced when it is set.
    cancel: Arc<AtomicBool>,
    closed: bool,
    progress: Progress,
    /// Discards counted, the count at each photo's last one, and at the last
    /// Discard All.
    discards: u64,
    discarded: HashMap<PhotoId, u64>,
    all_discarded: u64,
}

impl State {
    /// Whether `photo` was discarded after its build was queued, when the queue
    /// had counted `discards`.
    fn discarded_since(&self, photo: PhotoId, discards: u64) -> bool {
        self.all_discarded > discards || self.discarded.get(&photo).is_some_and(|d| *d > discards)
    }
    /// Queues `items`, each in the place of a waiting request for its photo;
    /// returns how many were added.
    fn enqueue(&mut self, items: Vec<Item>) -> usize {
        if !self.progress.running() {
            self.progress = Progress::default();
        }
        let mut added = 0;
        for mut item in items {
            item.discards = self.discards;
            match self.waiting.iter_mut().find(|w| w.photo == item.photo) {
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
    /// Counts a discard of `photos` (all when `None`), drops their builds not
    /// started and stops the one under way; returns how many builds it stopped.
    fn discard(&mut self, photos: Option<&BTreeSet<PhotoId>>) -> usize {
        self.discards += 1;
        match photos {
            Some(photos) => {
                for photo in photos {
                    self.discarded.insert(*photo, self.discards);
                }
            }
            None => self.all_discarded = self.discards,
        }
        let wanted = |photo: &PhotoId| photos.is_none_or(|p| p.contains(photo));
        let before = self.waiting.len();
        self.waiting.retain(|item| !wanted(&item.photo));
        let mut stopped = before - self.waiting.len();
        self.progress.total -= stopped;
        if self.running.as_ref().is_some_and(wanted) {
            self.cancel.store(true, Ordering::Relaxed);
            self.cancel = Arc::default();
            stopped += 1;
        }
        stopped
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// The next photo to build; `None` once closed.
    fn next(&self) -> Option<(Item, Arc<AtomicBool>)> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some(item) = state.waiting.pop_front() {
                state.running = Some(item.photo);
                return Some((item, state.cancel.clone()));
            }
            state = self.wake.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
    fn finish(&self, finished: &Finished) {
        let mut state = self.lock();
        state.running = None;
        state.progress.done += 1;
        let name = || finished.item.name.clone();
        match &finished.outcome {
            Outcome::Failed(why) => state.progress.failed.push((name(), why.clone())),
            Outcome::Built {
                look_missing: Some(why),
            } => state.progress.without_look.push((name(), why.clone())),
            _ => {}
        }
    }
    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.waiting.clear();
        state.cancel.store(true, Ordering::Relaxed);
        drop(state);
        self.wake.notify_all();
    }
}

/// The build queue and the thread that works through it.
struct Builder {
    shared: Arc<Shared>,
    results: Receiver<Finished>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Builder {
    /// Starts the worker; `changed` is called after each photo.
    fn start(steps: Steps, changed: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let (tx, results) = mpsc::channel();
        let worker = shared.clone();
        let thread = crate::raw::spawn_background(move || {
            let pool = crate::raw::background_pool(2, "editable-previews").ok();
            let mut catalog: Option<Catalog> = None;
            while let Some((item, cancel)) = worker.next() {
                let discarded = || {
                    let state = worker.lock();
                    state.discarded_since(item.photo, item.discards)
                };
                let held = &mut catalog;
                let mut build = || match connection(held, &item.catalog) {
                    Ok(catalog) => {
                        build_one(&item, catalog, &discarded, steps, Cancel::new(&cancel))
                    }
                    Err(e) => Outcome::Failed(format!("the catalog can't be opened: {e:#}")),
                };
                let caught =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &pool {
                        Some(pool) => pool.install(build),
                        None => build(),
                    }));
                let outcome = caught.unwrap_or_else(|panic| {
                    // The connection may have been left mid-write.
                    catalog = None;
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

    fn submit(&self, items: Vec<Item>) -> usize {
        let added = self.shared.lock().enqueue(items);
        self.shared.wake.notify_one();
        added
    }

    /// Cancel: nothing waiting starts, and the photo being built stops without
    /// recording anything.
    fn cancel(&self) {
        let mut state = self.shared.lock();
        let dropped = state.waiting.len();
        state.waiting.clear();
        state.progress.total -= dropped;
        state.cancel.store(true, Ordering::Relaxed);
        state.cancel = Arc::default();
    }

    fn progress(&self) -> Progress {
        self.shared.lock().progress.clone()
    }

    /// A finished report is put away; a running one stays.
    fn dismiss(&self) {
        let mut state = self.shared.lock();
        if !state.progress.running() {
            state.progress = Progress::default();
        }
    }
}

impl Drop for Builder {
    /// Signals the worker; joining is the exit hook's to decide.
    fn drop(&mut self) {
        self.shared.close();
    }
}

/// The worker's connection to `at`, opened again for another catalog.
fn connection<'a>(
    held: &'a mut Option<Catalog>,
    at: &CatalogLocation,
) -> anyhow::Result<&'a mut Catalog> {
    if held.as_ref().is_none_or(|c| c.location() != at) {
        *held = None;
        *held = Some(Catalog::open(at.clone())?);
    }
    Ok(held.as_mut().expect("opened above"))
}

/// Builds `item`'s editable preview and records it, unless its recorded one is of
/// this original already. `discarded` says whether the photo was discarded since
/// the item was queued.
fn build_one(
    item: &Item,
    catalog: &mut Catalog,
    discarded: &dyn Fn() -> bool,
    steps: Steps,
    cancel: Cancel<'_>,
) -> Outcome {
    match build(item, catalog, discarded, steps, cancel) {
        Ok(outcome) => outcome,
        Err(_) if cancel.is_cancelled() => Outcome::Cancelled,
        Err(e) => Outcome::Failed(format!("{e:#}")),
    }
}

fn build(
    item: &Item,
    catalog: &mut Catalog,
    discarded: &dyn Fn() -> bool,
    steps: Steps,
    cancel: Cancel<'_>,
) -> anyhow::Result<Outcome> {
    let identity = Identity::read(&item.path)
        .with_context(|| format!("{} can't be read", item.path.display()))?;
    if let Some(recorded) = catalog.editable_preview(item.photo)? {
        // Built again from the same original, it would have the same name, and
        // a write never replaces a file.
        if !recorded.is_stale(&identity) && recorded.path.is_file() {
            return Ok(Outcome::Unchanged);
        }
        // Of another file (or its file gone): it can stand for nothing now,
        // and goes before the new one is built.
        catalog.discard_editable_previews(&[item.photo])?;
    }
    let Original {
        image,
        exif,
        profiles,
    } = (steps.open)(item, &identity, cancel)?;
    ensure!(!cancel.is_cancelled(), "Cancelled");
    let look = (steps.fit)(&item.path, &image.metadata, &profiles, cancel);
    // Lossless until Preferences offers the Smaller choice.
    let mut preview = editable_preview::build(
        &image,
        &identity,
        exif,
        editable_preview::LONG_EDGE,
        editable_preview::Precision::Lossless,
        cancel,
    )?;
    drop(image);
    // A fit that failed or was refused doesn't fail the build: the editable
    // preview is kept without it, and the report says so.
    let starts_from_look = !matches!(look, DefaultFit::NotDefault);
    let mut look_missing = match look {
        DefaultFit::NotDefault => None,
        DefaultFit::Fitted(p) => preview
            .set_camera_look(&p)
            .err()
            .map(|e| format!("its Camera Look can't be kept: {e:#}")),
        DefaultFit::Missing(why) => Some(format!("its Camera Look wasn't fitted: {why}")),
    };
    ensure!(!cancel.is_cancelled(), "Cancelled");
    let folder = catalog.editable_preview_folder();
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("{} can't be made", folder.display()))?;
    let file = editable_preview_file_name(item.photo, &identity);
    let path = folder.join(&file);
    if let Some(kept) = write_or_keep(&path, &file, &preview, &identity, catalog)? {
        look_missing = match kept.camera_look() {
            Some(_) => None,
            None if starts_from_look => look_missing
                .or_else(|| Some("an editable preview built earlier without it was kept".into())),
            None => None,
        };
    }
    drop(preview);
    match record(item, &identity, &file, &path, catalog, discarded) {
        Ok(true) => {}
        Ok(false) => {
            let _ = catalog.discard_unrecorded_editable_preview(&file);
            return Ok(Outcome::Cancelled);
        }
        Err(e) => {
            let _ = catalog.discard_unrecorded_editable_preview(&file);
            return Err(e);
        }
    }
    // A discard that came while this was recorded may have run before it: this
    // record must not outlive it.
    if discarded() {
        catalog.discard_editable_previews(&[item.photo])?;
        return Ok(Outcome::Cancelled);
    }
    Ok(Outcome::Built { look_missing })
}

/// Writes `preview` to `path` (named `file`), or keeps the file a failed write
/// leaves there when it is complete and of this original: returned, read, when
/// it is kept.
///
/// A write fails with a complete file in place when the folder's sync fails after
/// the rename, and finds one there (`AlreadyExists`) when an earlier build of
/// this original left it unrecorded (refused, then not removed, or a crash before
/// recording). Such a file is read whole, every section's hash checked, before
/// anything removes it. One that reads and names this original is kept and
/// recorded: it is what this build would write, and removing it would leave the
/// photo without one, should writing again fail. Anything else there (a partial
/// copy, another original's) is removed through the catalog, and a write that
/// found it is tried once more.
fn write_or_keep(
    path: &Path,
    file: &str,
    preview: &editable_preview::EditablePreview,
    identity: &Identity,
    catalog: &mut Catalog,
) -> anyhow::Result<Option<editable_preview::EditablePreview>> {
    let error = match editable_preview::write(path, preview) {
        Ok(()) => return Ok(None),
        Err(e) => e,
    };
    if let Ok(there) = editable_preview::read(path, Cancel::NEVER)
        && there.identity() == identity
    {
        return Ok(Some(there));
    }
    let found = error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists);
    catalog.discard_unrecorded_editable_preview(file)?;
    if found {
        editable_preview::write(path, preview)?;
        return Ok(None);
    }
    Err(error.context("The editable preview couldn't be written"))
}

/// Records the editable preview at `path` for `item`, unless its photo was
/// discarded meanwhile (`false`). Refused when the file isn't whole, the original
/// is no longer the file read, or the catalog refuses it (the photo left the
/// catalog or was moved).
fn record(
    item: &Item,
    identity: &Identity,
    file: &str,
    path: &Path,
    catalog: &mut Catalog,
    discarded: &dyn Fn() -> bool,
) -> anyhow::Result<bool> {
    let header = editable_preview::read_header(path)?;
    ensure!(
        header.identity == *identity,
        "The editable preview written isn't of the file read"
    );
    ensure!(
        Identity::read(&item.path).ok().as_ref() == Some(identity),
        "The photo's file changed while its editable preview was built"
    );
    if discarded() {
        return Ok(false);
    }
    catalog.record_editable_preview(
        item.photo,
        &item.path,
        &BuiltEditablePreview {
            file: file.to_owned(),
            format_version: header.version,
            original: header.identity,
            original_size: (header.original[0], header.original[1]),
            size: (header.preview[0], header.preview[1]),
        },
    )?;
    Ok(true)
}

/// The editor's editable preview builds.
pub(in crate::app) struct EditableBuilds {
    builder: Option<Builder>,
    /// The catalog the queued builds are for; another one cancels them.
    catalog: Option<CatalogLocation>,
    steps: Steps,
}

impl Default for EditableBuilds {
    fn default() -> Self {
        Self {
            builder: None,
            catalog: None,
            steps: Steps::REAL,
        }
    }
}

impl EditableBuilds {
    /// Builds through `steps`.
    #[cfg(test)]
    pub(in crate::app) fn for_tests(steps: Steps) -> Self {
        Self {
            steps,
            ..Self::default()
        }
    }
    fn builder(&mut self, ctx: &eframe::egui::Context) -> &Builder {
        let (steps, ctx) = (self.steps, ctx.clone());
        self.builder
            .get_or_insert_with(|| Builder::start(steps, move || ctx.request_repaint()))
    }
    pub(in crate::app) fn progress(&self) -> Option<Progress> {
        self.builder
            .as_ref()
            .map(Builder::progress)
            .filter(|p| p.total > 0 || !p.failed.is_empty() || !p.without_look.is_empty())
    }
    pub(in crate::app) fn running(&self) -> bool {
        self.progress().is_some_and(|p| p.running())
    }
    /// Stops the worker at exit: the photo being built is cancelled and nothing
    /// else starts. Returns the thread, to wait for.
    pub(in crate::app) fn close(&mut self) -> Option<std::thread::JoinHandle<()>> {
        let builder = self.builder.as_mut()?;
        builder.shared.close();
        builder.thread.take()
    }
    /// Stops the builds asked for (the status line's button).
    pub(in crate::app) fn cancel(&self) {
        if let Some(builder) = &self.builder {
            builder.cancel();
        }
    }
    pub(in crate::app) fn dismiss(&self) {
        if let Some(builder) = &self.builder {
            builder.dismiss();
        }
    }
}

/// What a Discard Editable Previews did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct Discarded {
    /// The photos (masters) whose editable previews went.
    pub discarded: usize,
    /// Builds of them stopped, waiting or under way.
    pub stopped: usize,
}

impl Editor {
    /// Build Editable Previews for `ids`: each available RAW's master, once.
    /// Offline photos and other files are skipped and counted; a photo whose
    /// editable preview is of its original as it is costs nothing.
    pub(in crate::app) fn build_editable_previews(
        &mut self,
        ids: &[PhotoId],
    ) -> Result<Queued, String> {
        let Some(library) = &self.library else {
            return Err("No catalog is open".into());
        };
        let catalog = library.session.catalog().location().clone();
        let mut masters = BTreeSet::new();
        let mut items = Vec::new();
        let mut skipped = 0;
        for &id in ids {
            let Some(photo) = library.photo(id) else {
                continue;
            };
            let master = photo.master.unwrap_or(id);
            if !masters.insert(master) {
                continue;
            }
            match library.photo(master) {
                // Only a RAW: a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL
                // offline shows its Standard preview, and its file is no larger
                // than one.
                Some(m)
                    if crate::storage::is_raw(&m.path)
                        && library.export_refusal(master).is_none() =>
                {
                    items.push(Item {
                        catalog: catalog.clone(),
                        photo: master,
                        name: m.filename.clone(),
                        path: m.path.clone(),
                        demosaic: self.session.demosaic,
                        decode_cache: self.decode_cache.clone(),
                        discards: 0,
                    });
                }
                _ => skipped += 1,
            }
        }
        let ctx = self.context.clone();
        let builds = &mut self.editable_builds;
        if builds.catalog.as_ref() != Some(&catalog) {
            if let Some(builder) = &builds.builder {
                builder.cancel();
            }
            builds.catalog = Some(catalog);
        }
        let queued = builds.builder(&ctx).submit(items);
        Ok(Queued { queued, skipped })
    }

    /// The thumbnail menu's Build Editable Previews.
    pub(in crate::app) fn build_editable_previews_from_menu(&mut self, ids: &[PhotoId]) {
        match self.build_editable_previews(ids) {
            Ok(queued) => self.messages.note(queued_message(queued)),
            Err(e) => self
                .messages
                .fail(format!("Editable previews not built: {e}")),
        }
    }

    /// Discard Editable Previews for `ids` (their masters', for virtual copies),
    /// or for every photo when `None`: builds of them waiting or under way stop,
    /// and their rows go, the files after the catalog commits.
    pub(in crate::app) fn discard_editable_previews(
        &mut self,
        ids: Option<&[PhotoId]>,
    ) -> Result<Discarded, String> {
        let Some(library) = &self.library else {
            return Err("No catalog is open".into());
        };
        let masters = ids.map(|ids| masters_of(library, ids));
        // Counted before the rows go, so a build recording meanwhile drops its
        // own record (see `State::discarded_since`).
        let stopped = match &self.editable_builds.builder {
            Some(builder) => builder.shared.lock().discard(masters.as_ref()),
            None => 0,
        };
        let library = self.library.as_mut().expect("checked above");
        let discarded = match &masters {
            Some(masters) => library
                .session
                .discard_editable_previews(&masters.iter().copied().collect::<Vec<_>>()),
            None => library.session.discard_all_editable_previews(),
        }
        .map_err(|e| format!("The catalog could not be changed: {e:#}"))?;
        self.editable_previews_changed();
        Ok(Discarded {
            discarded: discarded.len(),
            stopped,
        })
    }

    /// The photos (masters) of `ids`, or of the whole catalog when `None`, whose
    /// editable previews a discard would remove while their originals are
    /// offline, as a build finds them ([`Library::export_refusal`]): none of
    /// these can be built again until its original is back.
    pub(in crate::app) fn offline_editable_previews(
        &self,
        ids: Option<&[PhotoId]>,
    ) -> Result<BTreeSet<PhotoId>, String> {
        let mut recorded = self.editable_previews_of(ids)?;
        if let Some(library) = &self.library {
            recorded.retain(|m| library.export_refusal(*m).is_some());
        }
        Ok(recorded)
    }

    /// The photos (masters) of `ids`, or of the whole catalog when `None`, that
    /// have editable previews: those a discard of them removes.
    fn editable_previews_of(&self, ids: Option<&[PhotoId]>) -> Result<BTreeSet<PhotoId>, String> {
        let Some(library) = &self.library else {
            return Ok(BTreeSet::new());
        };
        let mut recorded = library
            .session
            .catalog()
            .photos_with_editable_previews()
            .map_err(|e| format!("The catalog could not be read: {e:#}"))?;
        if let Some(ids) = ids {
            let masters = masters_of(library, ids);
            recorded.retain(|m| masters.contains(m));
        }
        Ok(recorded)
    }

    /// Discard Editable Previews for `ids` (their masters'), keeping those of
    /// photos whose originals are offline ([`Self::offline_editable_previews`])
    /// unless `include_offline`: automation's discard. Returns what went and how
    /// many photos' were kept.
    pub(in crate::app) fn discard_editable_previews_online(
        &mut self,
        ids: &[PhotoId],
        include_offline: bool,
    ) -> Result<(Discarded, usize), String> {
        let kept = if include_offline {
            BTreeSet::new()
        } else {
            self.offline_editable_previews(Some(ids))?
        };
        let Some(library) = &self.library else {
            return Err("No catalog is open".into());
        };
        let masters: Vec<PhotoId> = masters_of(library, ids)
            .into_iter()
            .filter(|m| !kept.contains(m))
            .collect();
        let done = self.discard_editable_previews(Some(&masters))?;
        Ok((done, kept.len()))
    }

    /// The thumbnail menu's Discard Editable Previews: at once when every photo
    /// whose editable preview it removes is online, else after asking, as none
    /// of an offline photo's can be built again until its original is back.
    pub(in crate::app) fn discard_editable_previews_from_menu(&mut self, ids: &[PhotoId]) {
        let recorded = match self.editable_previews_of(Some(ids)) {
            Ok(recorded) => recorded,
            Err(e) => {
                self.messages
                    .fail(format!("Editable previews not discarded: {e}"));
                return;
            }
        };
        let Some(library) = &self.library else {
            return;
        };
        let offline = recorded
            .iter()
            .filter(|m| library.export_refusal(**m).is_some())
            .count();
        if offline == 0 {
            return self.discard_editable_previews_now(ids);
        }
        self.modal = Some(crate::app::Modal::DiscardEditablePreviews(OfflineDiscard {
            catalog: library.session.catalog().location().clone(),
            photos: recorded.len(),
            ids: ids.to_vec(),
            offline,
        }));
    }

    fn discard_editable_previews_now(&mut self, ids: &[PhotoId]) {
        match self.discard_editable_previews(Some(ids)) {
            Ok(done) => self.messages.note(discarded_message(done)),
            Err(e) => self
                .messages
                .fail(format!("Editable previews not discarded: {e}")),
        }
    }

    /// Asks before the thumbnail menu's Discard Editable Previews removes those
    /// of offline photos. Cancel has the focus; Return, Escape and a click
    /// outside cancel.
    pub(in crate::app) fn discard_editable_previews_window(&mut self, ctx: &eframe::egui::Context) {
        let Some(crate::app::Modal::DiscardEditablePreviews(asked)) = &self.modal else {
            return;
        };
        // Asked of another catalog, its photos' ids name others here.
        let current = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location());
        if current != Some(&asked.catalog) {
            self.modal = None;
            return;
        }
        let (title, line) = offline_question(asked.offline, asked.photos);
        let choice = crate::app::ui::widgets::destructive_choice(
            ctx,
            "discard-editable-previews",
            &title,
            &[line],
            "Cancel",
            &["Discard"],
        );
        let Some(choice) = choice else {
            return;
        };
        let Some(crate::app::Modal::DiscardEditablePreviews(asked)) = self.modal.take() else {
            return;
        };
        if choice.is_some() {
            self.discard_editable_previews_now(&asked.ids);
        }
    }

    /// Takes finished builds, every frame. Another catalog cancels the builds of
    /// the last one; a worker that is gone ends them.
    pub(in crate::app) fn poll_editable_builds(&mut self) {
        let current = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone());
        let builds = &mut self.editable_builds;
        if builds.catalog.is_some() && builds.catalog != current {
            if let Some(builder) = &builds.builder {
                builder.cancel();
            }
            builds.catalog = None;
        }
        let Some(builder) = &builds.builder else {
            return;
        };
        // Any result can change what they take: a stale one is discarded before
        // its rebuild, which may then fail, and a record that raced a discard is
        // discarded again.
        let mut finished = false;
        loop {
            match builder.results.try_recv() {
                Ok(_) => finished = true,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Nothing more will finish; the progress went with it.
                    builds.builder = None;
                    builds.catalog = None;
                    self.messages.fail("Editable previews stopped being built");
                    break;
                }
            }
        }
        if finished {
            self.editable_previews_changed();
        }
    }

    /// "Building editable previews 3 / 12" over a bar, with a button that stops
    /// them; then what could not be built, until clicked.
    pub(in crate::app) fn editable_build_progress(&mut self, ui: &mut eframe::egui::Ui) {
        let Some(progress) = self.editable_builds.progress() else {
            return;
        };
        if progress.running() {
            let general = PreviewProgress {
                done: progress.done,
                total: progress.total,
                failed: Vec::new(),
            };
            if super::progress_bar(
                ui,
                &general,
                "Building editable previews",
                "cancel-editable-build",
                "Stop building editable previews",
            ) {
                self.editable_builds.cancel();
            }
            return;
        }
        let Some((line, reasons)) = report(&progress) else {
            return;
        };
        if super::report_line(ui, &line, &reasons) {
            self.editable_builds.dismiss();
        }
    }
}

/// What a finished build could not do, and why, photo by photo.
fn report(progress: &Progress) -> Option<(String, String)> {
    let mut parts = Vec::new();
    if !progress.failed.is_empty() {
        parts.push(format!(
            "{} could not be built",
            plural(
                progress.failed.len(),
                "editable preview",
                "editable previews"
            )
        ));
    }
    if !progress.without_look.is_empty() {
        let n = progress.without_look.len();
        parts.push(format!(
            "{n} built without {} Camera Look",
            if n == 1 { "its" } else { "their" }
        ));
    }
    if parts.is_empty() {
        return None;
    }
    let reasons = progress
        .failed
        .iter()
        .chain(&progress.without_look)
        .map(|(name, why)| format!("{name}: {why}"))
        .collect::<Vec<_>>()
        .join("\n");
    Some((parts.join(" · "), reasons))
}

/// "Building 12 editable previews · 3 skipped (offline or not RAW)".
pub(in crate::app) fn queued_message(queued: Queued) -> String {
    let mut line = match queued.queued {
        0 => "No editable previews to build".to_string(),
        n => format!(
            "Building {}",
            plural(n, "editable preview", "editable previews")
        ),
    };
    if queued.skipped > 0 {
        line.push_str(&format!(
            " · {} skipped (offline or not RAW)",
            queued.skipped
        ));
    }
    line
}

/// A thumbnail menu's Discard Editable Previews waiting for the user's answer,
/// as some of the photos whose editable previews it removes are offline.
#[derive(Debug)]
pub(in crate::app) struct OfflineDiscard {
    /// The catalog it was asked of.
    catalog: CatalogLocation,
    /// The photos the menu was chosen for, as it would discard them.
    ids: Vec<PhotoId>,
    /// Their masters with editable previews: those it would remove.
    photos: usize,
    /// The masters whose editable previews it would remove while offline.
    offline: usize,
}

/// The masters of `ids`, a virtual copy's request being its master's.
fn masters_of(library: &Library, ids: &[PhotoId]) -> BTreeSet<PhotoId> {
    ids.iter()
        .filter_map(|id| library.photo(*id))
        .map(|p| p.master.unwrap_or(p.id))
        .collect()
}

/// The question, and why it is asked, for discarding editable previews of
/// `photos` photos, `offline` of them with their originals offline.
fn offline_question(offline: usize, photos: usize) -> (String, String) {
    let line = match (offline, photos) {
        (1, 1) => "This photo is offline. Its editable preview can't be built again until its original is back.".to_string(),
        (1, _) => "1 of these photos is offline. Its editable preview can't be built again until its original is back.".to_string(),
        (n, p) if n == p => "These photos are offline. Their editable previews can't be built again until the originals are back.".to_string(),
        (n, _) => format!("{n} of these photos are offline. Their editable previews can't be built again until the originals are back."),
    };
    ("Discard editable previews anyway?".into(), line)
}

/// "Discarded the editable previews of 3 photos".
pub(in crate::app) fn discarded_message(done: Discarded) -> String {
    match done.discarded {
        0 => "No editable previews to discard".into(),
        n => format!(
            "Discarded the editable previews of {}",
            plural(n, "photo", "photos")
        ),
    }
}

/// [`discarded_message`], and how many photos' were kept as their originals
/// are offline.
pub(in crate::app) fn discarded_online_message(done: Discarded, kept_offline: usize) -> String {
    match kept_offline {
        0 => discarded_message(done),
        n => format!(
            "{}; kept those of {} offline",
            discarded_message(done),
            plural(n, "photo", "photos")
        ),
    }
}

#[cfg(test)]
mod tests;
