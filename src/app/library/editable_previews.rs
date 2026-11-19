//! Build Editable Previews and Discard Editable Previews
//! (docs/editable-previews.md): a reduced copy of each chosen RAW, built while
//! its original is online, so that Develop can edit the photo while the original
//! is offline (`Library::develop_from`). Copies are not a cache: once the
//! original is offline one can't be built again, so the catalog keeps a row for
//! each (`catalog::editable_previews`). They are RAWmakase's own files, which
//! this module deletes outright when their row goes.
//!
//! A request captures each item once: the master photo (a virtual copy asks for
//! its master's), its path, the demosaic, the copies folder and the copy the
//! catalog recorded then. The worker reads the original's [`Identity`] before
//! anything else reads the file, builds the copy through the app's one seam to
//! a copy's file
//! ([`CopyFiles::build`](crate::app::worker::editable_preview::CopyFiles::build))
//! under the name the catalog gives that identity, and reads the file again at
//! the end: a build of a file that changed meanwhile is dropped.
//!
//! The worker only writes files. Its result is recorded on the interface's
//! connection, where it is checked first against what the Library holds now: a
//! result is dropped, and its file deleted unless a row names it, when the
//! catalog was switched, the photo was removed, relinked or is no longer a
//! master, or its copy was built again or discarded since the item was
//! captured. So a build never records a row for a photo or catalog it is no
//! longer for, whatever happened while it ran.
//!
//! One photo is built at a time on two background threads
//! (`raw::background_pool`), at one pace whatever else runs. The build never
//! makes the editor busy. It stops between photos and, when the seam allows,
//! inside one when cancelled, and quitting cancels it and waits within the
//! shutdown deadline (docs/shutdown.md): nothing is lost, as the original it
//! was built from is still there to build from again.
//!
//! This is a worker of its own rather than Build Standard-Sized Previews'
//! ([`preview_build`](super::preview_build)), whose thread holds the preview
//! cache and runs its upkeep between builds, and whose queue is keyed by
//! preview kind; it shares that build's [`Progress`] and progress bar.
use super::preview_build::{Clicked, Progress, ProgressText, progress_bar};
use crate::app::Editor;
use crate::app::ui::widgets::plural;
use crate::app::worker::editable_preview::{BuildInput, Copies};
use crate::camera_data::Demosaic;
use crate::catalog::{CatalogLocation, EditablePreview, PhotoId};
use crate::storage::Identity;
use anyhow::Context;
use eframe::egui;
use rawmakase_cancel::Cancel;
use std::{
    collections::{HashMap, VecDeque},
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
    /// The original's path as the catalog had it: the file read, and the path
    /// the result must still be the photo's for it to be recorded.
    pub path: PathBuf,
    pub demosaic: Demosaic,
    pub decode_cache: crate::decode_cache::DecodeCache,
    /// The catalog's copies folder, where the copy is written.
    pub folder: PathBuf,
    /// The copy the catalog recorded for the photo when it was asked for: a
    /// result is recorded only while it still does, so a copy built or
    /// discarded meanwhile is never replaced by an older request's.
    pub recorded: Option<EditablePreview>,
    /// The discards counted when it was asked for.
    pub discards: u64,
}

/// How building one photo ended.
#[derive(Debug, PartialEq)]
pub(in crate::app) enum Outcome {
    /// Written to the copies folder under `file`, of the original as
    /// `original` names it; not recorded yet.
    Built {
        file: String,
        original: Identity,
    },
    /// The copy recorded is of this original already.
    Unchanged,
    Failed(String),
    /// Cancelled: nothing written.
    Cancelled,
}

pub(in crate::app) struct Finished {
    pub item: Item,
    pub outcome: Outcome,
}

/// Builds `item`'s copy through `copies`, unless the recorded one is of this
/// original already and opens: a damaged copy, or one whose record this
/// release doesn't know, is built again. A file written and then not wanted (cancelled, or the
/// original changed meanwhile) is deleted, unless the recorded copy names it.
fn build_one(item: &Item, copies: &Copies, cancel: Cancel<'_>) -> Outcome {
    let identity = match Identity::read(&item.path)
        .with_context(|| format!("{} can't be read", item.path.display()))
    {
        Ok(identity) => identity,
        Err(e) => return Outcome::Failed(format!("{e:#}")),
    };
    if let Some(recorded) = &item.recorded
        && !recorded.out_of_date
        && recorded.original.same_photo(&identity)
        && recorded
            .path_in(&item.folder)
            .is_some_and(|path| copies.open(&path, cancel).is_ok())
    {
        return Outcome::Unchanged;
    }
    let file = crate::catalog::editable_preview_file_name(item.photo, &identity);
    let dest = item.folder.join(&file);
    let unwanted = || {
        if item.recorded.as_ref().is_none_or(|r| r.file != file) {
            let _ = std::fs::remove_file(&dest);
        }
    };
    let input = BuildInput {
        path: &item.path,
        identity: &identity,
        demosaic: item.demosaic,
        decode_cache: &item.decode_cache,
    };
    let built = std::fs::create_dir_all(&item.folder)
        .with_context(|| format!("{} can't be made", item.folder.display()))
        .and_then(|()| copies.build(&input, &dest, cancel));
    if cancel.is_cancelled() {
        if built.is_ok() {
            unwanted();
        }
        return Outcome::Cancelled;
    }
    if let Err(e) = built {
        return Outcome::Failed(format!("{e:#}"));
    }
    // Read again: a build of a file replaced while it ran is of neither.
    if Identity::read(&item.path).ok().as_ref() != Some(&identity) {
        unwanted();
        return Outcome::Failed("its file changed while its editable preview was built".into());
    }
    Outcome::Built {
        file,
        original: identity,
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
    /// Builds finished with a file written that the UI hasn't taken yet, so
    /// not recorded: their files are not orphans.
    unrecorded: usize,
}

impl State {
    /// Queues `items`, each in the place of a waiting request for its photo;
    /// returns how many were added.
    fn enqueue(&mut self, items: Vec<Item>) -> usize {
        if !self.progress.running() {
            self.progress = Progress::default();
        }
        let mut added = 0;
        for item in items {
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
    /// Stops the build under way and drops those waiting, of `photos` (all of
    /// them when `None`).
    fn drop_builds(&mut self, photos: Option<&[PhotoId]>) {
        let wanted = |photo: &PhotoId| photos.is_none_or(|p| p.contains(photo));
        let before = self.waiting.len();
        self.waiting.retain(|item| !wanted(&item.photo));
        self.progress.total -= before - self.waiting.len();
        if self.running.as_ref().is_some_and(wanted) {
            self.cancel.store(true, Ordering::Relaxed);
            self.cancel = Arc::default();
        }
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
    /// The next photo to build and its cancel flag; `None` once closed.
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
        if matches!(finished.outcome, Outcome::Built { .. }) {
            state.unrecorded += 1;
        }
        if let Outcome::Failed(why) = &finished.outcome {
            state
                .progress
                .failed
                .push((finished.item.name.clone(), why.clone()));
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
    /// Starts the worker, building through `copies`; `changed` is called after
    /// each photo.
    fn start(copies: Copies, changed: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let (tx, results) = mpsc::channel();
        let worker = shared.clone();
        let thread = crate::raw::spawn_background(move || {
            let pool = crate::raw::background_pool(2, "editable-previews").ok();
            while let Some((item, cancel)) = worker.next() {
                let build = || build_one(&item, &copies, Cancel::new(&cancel));
                let caught =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &pool {
                        Some(pool) => pool.install(build),
                        None => build(),
                    }));
                let outcome = caught.unwrap_or_else(|panic| {
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
    /// Cancel: nothing waiting starts, and the photo being built stops; what
    /// it wrote is dropped.
    fn cancel(&self) {
        self.shared.lock().drop_builds(None);
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

/// The editor's editable preview builds.
#[derive(Default)]
pub(in crate::app) struct EditableBuilds {
    builder: Option<Builder>,
    /// The catalog the queued builds are for; another one cancels them.
    catalog: Option<CatalogLocation>,
    /// Discards counted, and the count at each master photo's last one: a
    /// result for a photo discarded after it was asked for is dropped.
    discards: u64,
    discarded: HashMap<PhotoId, u64>,
    /// The catalog session's generation whose files no row names were last
    /// deleted ([`Editor::sweep_editable_previews`]).
    swept: Option<u64>,
}

impl EditableBuilds {
    fn builder(&mut self, copies: &Copies, ctx: &egui::Context) -> &Builder {
        let (copies, ctx) = (copies.clone(), ctx.clone());
        self.builder
            .get_or_insert_with(|| Builder::start(copies, move || ctx.request_repaint()))
    }
    pub(in crate::app) fn progress(&self) -> Option<Progress> {
        self.builder
            .as_ref()
            .map(Builder::progress)
            .filter(|p| p.total > 0 || !p.failed.is_empty())
    }
    /// Whether a build is waiting or under way.
    pub(in crate::app) fn running(&self) -> bool {
        self.progress().is_some_and(|p| p.running())
    }
    /// Whether no file a build wrote can still be recorded: nothing waiting or
    /// under way, and every finished build's file taken and recorded. Only
    /// then are files no row names orphans. The worker counts a finished
    /// build in the same step that ends it, so there is no moment between.
    fn settled(&self) -> bool {
        self.builder.as_ref().is_none_or(|b| {
            let state = b.shared.lock();
            state.running.is_none() && state.waiting.is_empty() && state.unrecorded == 0
        })
    }
    /// Stops the worker at exit: the photo being built is cancelled and nothing
    /// else starts. Returns the thread, to wait for.
    pub(in crate::app) fn close(&mut self) -> Option<std::thread::JoinHandle<()>> {
        let builder = self.builder.as_mut()?;
        builder.shared.close();
        builder.thread.take()
    }
    /// Whether `item`'s photo was discarded after it was asked for.
    fn discarded_since(&self, item: &Item) -> bool {
        self.discarded
            .get(&item.photo)
            .is_some_and(|at| *at > item.discards)
    }
}

/// What a Build Editable Previews request did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct Queued {
    pub queued: usize,
    /// Offline photos and files that aren't RAWs.
    pub skipped: usize,
}

impl Editor {
    /// Build Editable Previews for `ids`: each online RAW's master, once.
    /// Offline photos and other files are skipped and counted; a copy already
    /// of the original as it is costs only reading its identity.
    pub(in crate::app) fn build_editable_previews(
        &mut self,
        ids: &[PhotoId],
    ) -> Result<Queued, String> {
        // Results already in are recorded first, so none is taken for a file
        // no row names below.
        self.poll_editable_builds();
        let Some(library) = &self.library else {
            return Err("No catalog is open".into());
        };
        let catalog = library.session.catalog();
        let location = catalog.location().clone();
        let folder = catalog.editable_previews_folder();
        let mut items: Vec<Item> = Vec::new();
        let mut skipped = 0;
        for &id in ids {
            let Some(photo) = library.photo(id) else {
                continue;
            };
            let master = photo.master.unwrap_or(photo.id);
            if items.iter().any(|item| item.photo == master) {
                continue;
            }
            if library.export_refusal(id).is_some() || !crate::storage::is_raw(&photo.path) {
                skipped += 1;
                continue;
            }
            let recorded = catalog
                .editable_preview(master)
                .map_err(|e| format!("The catalog could not be read: {e:#}"))?;
            items.push(Item {
                catalog: location.clone(),
                photo: master,
                name: photo.filename.clone(),
                path: photo.path.clone(),
                demosaic: self.session.demosaic,
                decode_cache: self.decode_cache.clone(),
                folder: folder.clone(),
                recorded,
                discards: self.editable_builds.discards,
            });
        }
        // Files no row names, left by a removal or a build or discard whose
        // deletion failed, go while nothing is being built that could still
        // record one.
        if self.editable_builds.settled() {
            delete_orphans(library);
        }
        let ctx = self.context.clone();
        let copies = self.copies.clone();
        let builds = &mut self.editable_builds;
        if builds.catalog.as_ref() != Some(&location) {
            if let Some(builder) = &builds.builder {
                builder.cancel();
            }
            builds.catalog = Some(location);
        }
        let queued = builds.builder(&copies, &ctx).submit(items);
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

    /// Discard Editable Previews for `ids`: their masters' builds not started
    /// are dropped and the one under way stopped, the catalog forgets their
    /// copies in one transaction, and the files go once it has committed. A
    /// catalog that refuses leaves everything as it was; a file that can't be
    /// deleted is reported, and is deleted by the next sweep, Build or Discard.
    pub(in crate::app) fn discard_editable_previews(&mut self, ids: &[PhotoId]) {
        self.poll_editable_builds();
        let Some(library) = &mut self.library else {
            return;
        };
        let masters: Vec<PhotoId> = ids
            .iter()
            .filter_map(|id| library.photo(*id))
            .map(|photo| photo.master.unwrap_or(photo.id))
            .collect();
        let builds = &mut self.editable_builds;
        builds.discards += 1;
        for master in &masters {
            builds.discarded.insert(*master, builds.discards);
        }
        if let Some(builder) = &builds.builder {
            builder.shared.lock().drop_builds(Some(&masters));
        }
        let files = match library.session.discard_editable_previews(ids) {
            Ok(files) => files,
            Err(e) => {
                self.messages
                    .fail(format!("Editable previews not discarded: {e:#}"));
                return;
            }
        };
        let folder = library.session.catalog().editable_previews_folder();
        let mut failed: Vec<String> = files
            .iter()
            .filter_map(|file| delete(&folder, file).err())
            .collect();
        if builds.settled() {
            failed.extend(delete_orphans(library));
        }
        library.editable_previews_changed();
        match failed.first() {
            None => self.messages.note(format!(
                "Discarded the editable previews of {}",
                plural(files.len(), "photo", "photos")
            )),
            Some(first) => self.messages.fail(format!(
                "Discarded the editable previews of {}, but {} could not be deleted ({first}); \
                 the next Build or Discard deletes them",
                plural(files.len(), "photo", "photos"),
                plural(failed.len(), "file", "files")
            )),
        }
    }

    /// Every frame: takes finished builds and records those that still apply,
    /// then deletes the files of photos removed since.
    pub(in crate::app) fn poll_editable_builds(&mut self) {
        self.take_editable_builds();
        self.sweep_editable_previews();
    }

    /// Deletes the files no row names once a change that may have removed
    /// photos has committed: the catalog session starts a new generation for
    /// each such change, whichever workflow made it. While a build is under way
    /// or its file not yet recorded, it waits until nothing could still record
    /// one. A file that can't be deleted stays for the next change, Build or
    /// Discard.
    fn sweep_editable_previews(&mut self) {
        let Some(library) = &self.library else {
            return;
        };
        let generation = library.session.generation();
        let builds = &mut self.editable_builds;
        if builds.swept == Some(generation) || !builds.settled() {
            return;
        }
        builds.swept = Some(generation);
        delete_orphans(library);
    }

    /// Takes finished builds and records those that still apply. Another
    /// catalog cancels the builds of the last one, whose late results are
    /// dropped.
    fn take_editable_builds(&mut self) {
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
        let mut finished = Vec::new();
        loop {
            match builder.results.try_recv() {
                Ok(done) => finished.push(done),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // The worker is gone: nothing more will finish.
                    builds.builder = None;
                    builds.catalog = None;
                    self.messages.fail("Editable previews stopped being built");
                    break;
                }
            }
        }
        let built = finished
            .iter()
            .filter(|done| matches!(done.outcome, Outcome::Built { .. }))
            .count();
        let mut changed = false;
        for done in finished {
            if let Outcome::Built { file, original } = done.outcome {
                changed |= self.record_build(&done.item, &file, &original);
            }
        }
        // Recorded (or deleted as no longer wanted) on this thread, before any
        // cleanup can run: their files are not orphans until now.
        if built > 0
            && let Some(builder) = &self.editable_builds.builder
        {
            let mut state = builder.shared.lock();
            state.unrecorded = state.unrecorded.saturating_sub(built);
        }
        if changed && let Some(library) = &mut self.library {
            library.editable_previews_changed();
        }
    }

    /// Records the copy `file` built of `item`'s original as `original` names
    /// it, if it still applies; else deletes the file, unless a row names it.
    /// Returns whether the catalog changed.
    fn record_build(&mut self, item: &Item, file: &str, original: &Identity) -> bool {
        let discarded = self.editable_builds.discarded_since(item);
        let Some(library) = self
            .library
            .as_mut()
            .filter(|l| *l.session.catalog().location() == item.catalog)
        else {
            // Another catalog: its rows can't be read here, so only the copy
            // recorded when asked is known to name a file.
            if item.recorded.as_ref().is_none_or(|r| r.file != file) {
                let _ = delete(&item.folder, file);
            }
            return false;
        };
        let now = library.session.catalog().editable_preview(item.photo);
        // Still the master, of the same file, with the same copy recorded: a
        // photo Set Copy as Master made a virtual copy since has its master's
        // copy, which another build makes.
        let applies = !discarded
            && library
                .photo(item.photo)
                .is_some_and(|p| p.master.is_none() && p.path == item.path)
            && now.as_ref().ok() == Some(&item.recorded);
        // Recorded under the name the catalog makes from the identity, which is
        // the name the copy was written under (`build_one`).
        let recorded = applies
            .then(|| {
                library
                    .session
                    .record_editable_preview(item.photo, original)
            })
            .transpose();
        match recorded {
            Ok(Some(replaced)) => {
                if let Some(replaced) = replaced {
                    let _ = delete(&item.folder, &replaced);
                }
                true
            }
            other => {
                if let Err(e) = other {
                    self.messages.fail(format!(
                        "The editable preview of {} could not be recorded: {e:#}",
                        item.name
                    ));
                }
                // Any row naming the file keeps it, not only this photo's: Set
                // Copy as Master moves a row to another photo with its file's
                // name. A catalog that can't be read keeps it too; a file no
                // row names goes with the next cleanup.
                let named = library
                    .session
                    .catalog()
                    .editable_previews()
                    .map_or(true, |rows| rows.iter().any(|r| r.file == file));
                if !named {
                    let _ = delete(&item.folder, file);
                }
                false
            }
        }
    }

    /// "Building editable previews 3 / 12" with its Cancel in the top bar, then
    /// what could not be built until it is dismissed.
    pub(in crate::app) fn editable_build_progress(&mut self, ui: &mut egui::Ui) {
        let Some(progress) = self.editable_builds.progress() else {
            return;
        };
        let clicked = progress_bar(
            ui,
            &progress,
            ProgressText {
                running: "Building editable previews",
                failed: ("editable preview", "editable previews"),
                cancel: "Stop building editable previews",
                id: "cancel-editable-build",
            },
        );
        if let Some(builder) = &self.editable_builds.builder {
            match clicked {
                Some(Clicked::Cancel) => builder.cancel(),
                Some(Clicked::Dismiss) => builder.dismiss(),
                None => {}
            }
        }
    }
}

/// Deletes `file` from the copies `folder`; a file already gone is deleted.
fn delete(folder: &Path, file: &str) -> Result<(), String> {
    match std::fs::remove_file(folder.join(file)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{file}: {e}")),
    }
}

/// Deletes the files in `library`'s copies folder no row names; returns what
/// could not be deleted.
fn delete_orphans(library: &crate::app::library::Library) -> Vec<String> {
    match library.session.catalog().editable_preview_orphans() {
        Ok(orphans) => orphans
            .iter()
            .filter_map(|path| {
                std::fs::remove_file(path)
                    .err()
                    .filter(|e| e.kind() != std::io::ErrorKind::NotFound)
                    .map(|e| format!("{}: {e}", path.display()))
            })
            .collect(),
        Err(e) => vec![format!("{e:#}")],
    }
}

/// "Building 12 editable previews · 3 skipped (offline, or not a RAW)".
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
            " · {} skipped (offline, or not a RAW)",
            queued.skipped
        ));
    }
    line
}

#[cfg(test)]
mod tests;
