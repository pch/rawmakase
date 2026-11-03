//! The previews the grid and filmstrip draw, as GPU textures: each file's
//! embedded preview, and each photo's edited preview once rendered. Both are
//! bounded; requests go to the workers in `previews`, and results are
//! matched to the latest request so a late or stale render never shows.
use super::previews::{
    self, EditJob, EditQueue, EditResult, EditSource, Pace, PreviewResult, Progress, Wanted,
};
use crate::catalog::{Photo, PhotoId};
use eframe::egui;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender},
    thread::JoinHandle,
};

/// Textures kept per cache; older ones are dropped first.
const KEPT: usize = 192;
/// Photos asked for ahead at once; looked at for it in a frame; whose edit
/// is read from the catalog for it in a frame, on the UI thread; and asked
/// for ahead from one view, a small part of what the preview cache keeps, so
/// none pushes out another or the embedded previews.
const AHEAD: usize = 32;
const AHEAD_SCAN: usize = 256;
const AHEAD_LOOKUPS: usize = 16;
const AHEAD_WINDOW: usize = 500;

pub(super) struct PreviewTextures {
    /// Embedded previews by file. Virtual copies share a file, and so this.
    pub(super) thumbs: HashMap<PathBuf, egui::TextureHandle>,
    pub(super) thumb_order: VecDeque<PathBuf>,
    pub(super) pending: HashSet<PathBuf>,
    pub(super) failed: HashSet<PathBuf>,
    pub(super) thumb_tx: SyncSender<PathBuf>,
    pub(super) thumb_rx: Receiver<PreviewResult>,
    /// Edited previews: rendered from each photo's edit on a second worker.
    pub(super) edit_tx: EditQueue,
    pub(super) edit_rx: Receiver<EditResult>,
    /// Photos with an edited preview requested, by the ticket of the latest
    /// request; results of earlier requests are dropped.
    pub(super) edited_requested: HashMap<PhotoId, u64>,
    next_ticket: u64,
    /// Photos asking for an edited preview this frame, and last frame's
    /// as the worker sees them.
    pub(super) edit_seen: HashSet<PhotoId>,
    edit_wanted: Wanted,
    /// Edited previews of photos shown queued and not yet back.
    pub(super) edits_pending: usize,
    /// How fast edited previews are rendered, as last told the worker.
    pace: Pace,
    /// Tickets of the edited previews asked for ahead and not yet back.
    pub(super) ahead: HashSet<u64>,
    /// Photos asked for ahead and answered (kept in the cache, or failed),
    /// asked again only when shown, until forgotten.
    pub(super) kept: HashSet<PhotoId>,
    /// Where asking ahead got to: the list (by its version) and position it
    /// starts from, and how far past it the photos have been asked about.
    ahead_scan: ((u64, usize), usize),
    /// Previews showing each photo's edit, by photo.
    pub(super) edited: HashMap<PhotoId, egui::TextureHandle>,
    pub(super) edited_order: VecDeque<PhotoId>,
    pub(super) progress: Progress,
    /// Photos one view shows at once, e.g. a large survey; never fewer
    /// textures are kept, so none is dropped while it is shown.
    pub(super) shown_at_once: usize,
    /// The two workers, to wait for at exit.
    threads: [Option<JoinHandle<()>>; 2],
}
impl PreviewTextures {
    /// Starts both preview workers on the shared disk cache.
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let cache = crate::catalog::preview_cache::PreviewCache::path();
        let (thumb_tx, thumb_rx, thumbs) = previews::spawn(cache.clone(), ctx.clone());
        let edit_wanted = Wanted::default();
        // A library replaced by another catalog closes its queue as it is
        // dropped, which cancels its edited previews; the thumbnail workers
        // end once their channels are dropped with it.
        let (edit_tx, edit_rx, edits) =
            previews::spawn_edited(cache, edit_wanted.clone(), ctx.clone());
        Self {
            thumbs: HashMap::new(),
            thumb_order: VecDeque::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            thumb_tx,
            thumb_rx,
            edit_tx,
            edit_rx,
            edited_requested: HashMap::new(),
            next_ticket: 0,
            edit_seen: HashSet::new(),
            edit_wanted,
            edits_pending: 0,
            pace: Pace::Background,
            ahead: HashSet::new(),
            kept: HashSet::new(),
            ahead_scan: ((0, 0), 0),
            edited: HashMap::new(),
            edited_order: VecDeque::new(),
            progress: Progress::default(),
            shown_at_once: 0,
            threads: [Some(thumbs), Some(edits)],
        }
    }
    /// Closes the edited previews' queue, which cancels the renders under way,
    /// and drops the thumbnail workers' channels, so neither waits for the grid;
    /// the workers are then waited for at exit.
    pub(super) fn close(&mut self) -> [crate::app::task::Stopping; 2] {
        self.edit_tx.close();
        self.thumb_tx = mpsc::sync_channel(0).0;
        self.thumb_rx = mpsc::sync_channel(0).1;
        self.edit_rx = mpsc::channel().1;
        self.threads
            .each_mut()
            .map(|thread| crate::app::task::Stopping::new(thread.take()))
    }
    /// Takes every finished preview. Drain in every workspace so the bounded
    /// worker never waits for the grid.
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.thumb_rx.try_recv() {
            self.progress.finish(&result);
            let PreviewResult {
                path, image: im, ..
            } = result;
            self.pending.remove(&path);
            match im {
                Some(im) => self.insert_thumb(ctx, path, &im),
                None => {
                    self.failed.insert(path);
                }
            }
        }
        while let Ok(result) = self.edit_rx.try_recv() {
            let (id, ticket) = match &result {
                EditResult::CacheError(error) => {
                    self.progress.cache_failed(error.clone());
                    continue;
                }
                EditResult::Ready(id, ticket, _)
                | EditResult::Kept(id, ticket)
                | EditResult::Skipped(id, ticket)
                | EditResult::Failed(id, ticket) => (*id, *ticket),
            };
            let ahead = self.ahead.remove(&ticket);
            if !ahead {
                self.edits_pending = self.edits_pending.saturating_sub(1);
            }
            let latest = self.edited_requested.get(&id) == Some(&ticket);
            match result {
                EditResult::Ready(_, _, im) if latest => self.insert_edited(ctx, id, &im),
                // Asked again when shown, from the cache; one that failed ahead
                // (its file offline, say) is tried again then, not before.
                EditResult::Kept(..) if latest => {
                    self.edited_requested.remove(&id);
                    self.kept.insert(id);
                }
                EditResult::Failed(..) if latest && ahead => {
                    self.edited_requested.remove(&id);
                    self.kept.insert(id);
                }
                EditResult::Skipped(..) if latest => {
                    self.edited_requested.remove(&id);
                    // Given up for Develop, or out of view: ask ahead again later.
                    self.ahead_scan.1 = 0;
                }
                _ => {}
            }
        }
    }
    /// Hands the worker the photos shown last frame. Call once per frame.
    pub(super) fn publish_shown(&mut self) {
        let shown = std::mem::take(&mut self.edit_seen);
        *self.edit_wanted.lock().unwrap() = shown;
    }
    pub(super) fn insert_thumb(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        im: &image::RgbImage,
    ) {
        if !self.thumbs.contains_key(&path) {
            while self.thumbs.len() >= KEPT.max(self.shown_at_once) {
                let Some(old) = self.thumb_order.pop_front() else {
                    break;
                };
                self.thumbs.remove(&old);
            }
            self.thumb_order.push_back(path.clone());
        }
        self.thumbs
            .insert(path.clone(), texture(ctx, path.display().to_string(), im));
    }
    pub(super) fn insert_edited(&mut self, ctx: &egui::Context, id: PhotoId, im: &image::RgbImage) {
        if !self.edited.contains_key(&id) {
            while self.edited.len() >= KEPT.max(self.shown_at_once) {
                let Some(old) = self.edited_order.pop_front() else {
                    break;
                };
                self.edited.remove(&old);
                self.edited_requested.remove(&old);
            }
            self.edited_order.push_back(id);
        }
        self.edited
            .insert(id, texture(ctx, format!("edited-{id}"), im));
    }
    /// The ticket of the photo's latest edited preview request, if any.
    pub(super) fn edited_ticket(&self, id: PhotoId) -> Option<u64> {
        self.edited_requested.get(&id).copied()
    }
    fn ticket(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }
    /// Forgets a removed photo's previews, so a later photo given its id
    /// starts afresh.
    pub(super) fn forget(&mut self, id: PhotoId) {
        self.edited.remove(&id);
        self.edited_order.retain(|other| *other != id);
        self.edited_requested.remove(&id);
        self.edit_seen.remove(&id);
        self.kept.remove(&id);
        self.ahead_scan.1 = 0;
    }
    /// The photo's preview: its edit once rendered, else the embedded one.
    pub(super) fn texture(&self, photo: &Photo) -> Option<&egui::TextureHandle> {
        self.edited
            .get(&photo.id)
            .or_else(|| self.thumbs.get(&photo.path))
    }
    /// Photos whose thumbnail shows an edit.
    pub(super) fn edited_ids(&self) -> impl Iterator<Item = PhotoId> + '_ {
        self.edited.keys().copied()
    }
    /// Whether the photo's thumbnail already shows its edit (crop included).
    pub(super) fn has_edited(&self, id: PhotoId) -> bool {
        self.edited.contains_key(&id)
    }
    /// Queues the previews a shown photo needs: the embedded one until its
    /// edited one is in, and the edited one for a saved or Lightroom edit,
    /// which `edit` looks up when it is first asked for.
    pub(super) fn request(
        &mut self,
        photo: &Photo,
        ctx: &egui::Context,
        edit: impl FnOnce() -> Option<EditSource>,
    ) {
        if !self.edited.contains_key(&photo.id) {
            self.request_thumbnail(&photo.path, ctx);
        }
        self.request_edited(photo, edit);
    }
    pub(super) fn request_thumbnail(&mut self, path: &Path, ctx: &egui::Context) {
        if !self.thumbs.contains_key(path)
            && !self.pending.contains(path)
            && !self.failed.contains(path)
            && self.thumb_tx.try_send(path.to_path_buf()).is_ok()
        {
            self.pending.insert(path.to_path_buf());
            self.progress.queued();
            ctx.request_repaint();
        }
    }
    /// Queues an edited preview for a photo with a saved or Lightroom edit.
    /// A photo without one is asked about once, until it is forgotten.
    pub(super) fn request_edited(
        &mut self,
        photo: &Photo,
        edit: impl FnOnce() -> Option<EditSource>,
    ) {
        self.edit_seen.insert(photo.id);
        if self.edited_requested.contains_key(&photo.id) {
            return;
        }
        let ticket = self.ticket();
        self.edited_requested.insert(photo.id, ticket);
        if let Some(source) = edit()
            && self
                .edit_tx
                .send(EditJob::Render {
                    id: photo.id,
                    ticket,
                    path: photo.path.clone(),
                    source,
                    ahead: false,
                })
                .is_ok()
        {
            self.edits_pending += 1;
        }
    }
    /// Renders edited previews at `pace` from now on.
    pub(super) fn set_pace(&mut self, pace: Pace) {
        if self.pace != pace {
            self.pace = pace;
            self.edit_tx.set_pace(pace);
            self.ahead_scan.1 = 0;
        }
    }
    /// At [`Pace::Full`], once the embedded previews asked for are in, asks
    /// for the edited previews of the `photos` at `order` not shown yet, a few
    /// at a time from position `start` on (and then from the first), up to
    /// [`AHEAD_WINDOW`] of them, so they come from the cache once shown;
    /// `version` changes with `order`, which starts it over and drops the
    /// ones asked for the last list and not started. `edit`
    /// looks up each photo's edit when it is first asked about, as for a
    /// photo shown.
    pub(super) fn request_ahead(
        &mut self,
        photos: &[Photo],
        (version, order): (u64, &[usize]),
        start: usize,
        mut edit: impl FnMut(PhotoId) -> Option<EditSource>,
    ) {
        // Another list: what was asked ahead for the last one and not started
        // goes, so this one's come next.
        if self.ahead_scan.0.0 != version {
            if !self.ahead.is_empty() {
                self.edit_tx.drop_ahead();
            }
            self.ahead_scan = ((version, start), 0);
        }
        if self.pace != Pace::Full || order.is_empty() || !self.pending.is_empty() {
            return;
        }
        // A view not drawn yet, or past the end of a list grown shorter,
        // starts from the first.
        let start = if start < order.len() { start } else { 0 };
        if self.ahead_scan.0 != (version, start) {
            self.ahead_scan = ((version, start), 0);
        }
        let (mut scanned, mut looked_up) = (0, 0);
        while self.ahead.len() < AHEAD
            && scanned < AHEAD_SCAN
            && looked_up < AHEAD_LOOKUPS
            && self.ahead_scan.1 < order.len().min(AHEAD_WINDOW)
        {
            let Some(photo) = photos.get(order[(start + self.ahead_scan.1) % order.len()]) else {
                break;
            };
            self.ahead_scan.1 += 1;
            scanned += 1;
            if self.edited.contains_key(&photo.id)
                || self.edited_requested.contains_key(&photo.id)
                || self.kept.contains(&photo.id)
            {
                continue;
            }
            // A photo without an edit is asked about once, as when shown.
            looked_up += 1;
            let ticket = self.ticket();
            self.edited_requested.insert(photo.id, ticket);
            if let Some(source) = edit(photo.id)
                && self
                    .edit_tx
                    .send(EditJob::Render {
                        id: photo.id,
                        ticket,
                        path: photo.path.clone(),
                        source,
                        ahead: true,
                    })
                    .is_ok()
            {
                self.ahead.insert(ticket);
            }
        }
    }
    /// Shows Develop's latest render as the photo's thumbnail and caches it
    /// under the edit it was rendered with. Renders still in flight are older.
    pub(super) fn store_edited(
        &mut self,
        ctx: &egui::Context,
        id: PhotoId,
        path: PathBuf,
        image: image::RgbImage,
        recipe_json: String,
    ) {
        let tag = EditSource::Recipe(recipe_json).preview_tag();
        let ticket = self.ticket();
        self.edited_requested.insert(id, ticket);
        self.insert_edited(ctx, id, &image);
        let _ = self.edit_tx.send(EditJob::Store { path, tag, image });
    }
    /// Worth a status line: still working, or something could not be prepared.
    pub(super) fn progress_active(&self) -> bool {
        self.progress.active() || self.edits_pending > 0 || !self.ahead.is_empty()
    }
    pub(super) fn show_progress(&self, ui: &mut egui::Ui) {
        self.progress
            .show(ui, self.edits_pending, !self.ahead.is_empty());
    }
}
fn texture(ctx: &egui::Context, name: String, im: &image::RgbImage) -> egui::TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgb([im.width() as usize, im.height() as usize], im.as_raw()),
        egui::TextureOptions::LINEAR,
    )
}
