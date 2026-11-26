//! The previews the grid and filmstrip draw, as GPU textures: each file's
//! embedded preview, and each photo's edited preview once rendered. Both are
//! bounded; requests go to the workers in `previews`, and results are
//! matched to the latest request so a late or stale render never shows.
use super::previews::{
    self, EditJob, EditQueue, EditResult, EditSource, Pace, PreviewResult, Progress, ThumbJob,
    Wanted,
};
use crate::app::preferences::monitor::DisplayProfile;
use crate::catalog::{Photo, PhotoId};
use eframe::egui;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender},
    thread::JoinHandle,
};

/// Textures kept per cache; older ones not shown are dropped first.
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
    /// Files saved again since their thumbnail was made: it stays shown and
    /// is made again the next time it is shown.
    /// Each with whether a request made before the save is still out: what
    /// it brings back may be of the version before, so it is dropped.
    pub(super) renew: HashMap<PathBuf, bool>,
    pub(super) thumb_tx: SyncSender<ThumbJob>,
    pub(super) thumb_rx: Receiver<PreviewResult>,
    /// Edited previews: rendered from each photo's edit on a second worker.
    pub(super) edit_tx: EditQueue,
    pub(super) edit_rx: Receiver<EditResult>,
    /// Photos with an edited preview requested, by the ticket of the latest
    /// request; results of earlier requests are dropped.
    pub(super) edited_requested: HashMap<PhotoId, u64>,
    /// Photos whose latest request found no edit, so nothing was rendered.
    /// Should the file's embedded preview fail, the photo is asked about
    /// again and developed with the defaults instead.
    unedited: HashSet<PhotoId>,
    /// Requests for previews developed with the defaults in place of the
    /// embedded preview their file can't give, by ticket, with the file,
    /// until their results come back.
    stand_ins: HashMap<u64, PathBuf>,
    /// Offline photos' up-to-date editable previews, by photo: where Develop's
    /// renders of a photo opened from its copy are looked up too.
    copies: HashMap<PhotoId, PathBuf>,
    next_ticket: u64,
    /// Photos asking for an edited preview this frame, and last frame's
    /// as the worker sees them.
    pub(super) edit_seen: HashSet<PhotoId>,
    edit_wanted: Wanted,
    /// Last frame's [`Self::thumb_seen`] as the thumbnail workers see them:
    /// a request for a file no longer shown is skipped.
    pub(super) thumb_shown: previews::ShownFiles,
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
    /// Files whose thumbnail is shown this frame, by every view together.
    pub(super) thumb_seen: HashSet<PathBuf>,
    /// The files and photos shown in the last frame, by every view together
    /// (the grid and the filmstrip below it). Never fewer textures are kept,
    /// and these are never the ones dropped, so a grid of more cells than
    /// [`KEPT`] doesn't drop thumbnails it shows.
    shown_files: HashSet<PathBuf>,
    shown_photos: HashSet<PhotoId>,
    /// The two workers, to wait for at exit.
    threads: [Option<JoinHandle<()>>; 2],
    /// The monitor profile every texture here is converted to, which each
    /// request captures; a result converted for another is dropped.
    pub(super) display: DisplayProfile,
}
impl PreviewTextures {
    /// Starts both preview workers on the shared disk cache.
    pub(super) fn new(ctx: &egui::Context) -> Self {
        Self::with_cache(ctx, crate::catalog::preview_cache::PreviewCache::path())
    }
    /// Starts both preview workers on the disk cache at `cache`.
    pub(super) fn with_cache(ctx: &egui::Context, cache: PathBuf) -> Self {
        let thumb_shown = previews::ShownFiles::default();
        let (thumb_tx, thumb_rx, thumbs) =
            previews::spawn(cache.clone(), ctx.clone(), thumb_shown.clone());
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
            renew: HashMap::new(),
            thumb_tx,
            thumb_rx,
            edit_tx,
            edit_rx,
            edited_requested: HashMap::new(),
            unedited: HashSet::new(),
            stand_ins: HashMap::new(),
            copies: HashMap::new(),
            next_ticket: 0,
            edit_seen: HashSet::new(),
            edit_wanted,
            thumb_shown,
            edits_pending: 0,
            pace: Pace::Background,
            ahead: HashSet::new(),
            kept: HashSet::new(),
            ahead_scan: ((0, 0), 0),
            edited: HashMap::new(),
            edited_order: VecDeque::new(),
            progress: Progress::default(),
            shown_at_once: 0,
            thumb_seen: HashSet::new(),
            shown_files: HashSet::new(),
            shown_photos: HashSet::new(),
            threads: [Some(thumbs), Some(edits)],
            display: DisplayProfile::default(),
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
    /// Converts to `display` from now on: the textures converted for another
    /// go, and are asked for again when shown. Results on their way were
    /// converted for the old one and are dropped: thumbnails by the profile
    /// they carry, edited previews by their tickets, no longer the latest.
    pub(super) fn set_display(&mut self, display: DisplayProfile) {
        if self.display == display {
            return;
        }
        self.display = display;
        self.thumbs.clear();
        self.thumb_order.clear();
        self.edited.clear();
        self.edited_order.clear();
        self.edited_requested.clear();
        self.unedited.clear();
        self.ahead_scan.1 = 0;
    }
    /// Takes every finished preview. Drain in every workspace so the bounded
    /// worker never waits for the grid.
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.thumb_rx.try_recv() {
            self.progress.finish(&result);
            let PreviewResult {
                path,
                image: im,
                display,
                skipped,
                ..
            } = result;
            self.pending.remove(&path);
            if let Some(before) = self.renew.get_mut(&path) {
                if std::mem::take(before) {
                    // Asked for before the save: asked for again once shown.
                    continue;
                }
                if !skipped {
                    self.renew.remove(&path);
                }
            }
            // Converted for a monitor profile no longer used: asked for
            // again once shown, as one skipped is.
            let stale = display != self.display;
            match im.filter(|_| !stale) {
                Some(im) => self.insert_thumb(ctx, path, &im),
                // Asked for again once it is shown again.
                None if skipped || stale => {}
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
            self.stand_ins.remove(&ticket);
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
        self.shown_files = std::mem::take(&mut self.thumb_seen);
        *self
            .thumb_shown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(self.shown_files.clone());
        self.shown_photos = std::mem::take(&mut self.edit_seen);
        *self.edit_wanted.lock().unwrap() = self.shown_photos.clone();
    }
    /// How many textures of each kind are kept: [`KEPT`], or as many as are
    /// shown at once.
    fn kept(&self) -> usize {
        KEPT.max(self.shown_at_once).max(self.shown_photos.len())
    }
    pub(super) fn insert_thumb(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        im: &image::RgbImage,
    ) {
        if !self.thumbs.contains_key(&path) {
            while self.thumbs.len() >= self.kept() {
                match oldest_not_shown(&mut self.thumb_order, &self.shown_files) {
                    Some(old) => {
                        self.thumbs.remove(&old);
                    }
                    // Every one kept is shown and this one isn't: it is the
                    // one not kept, asked for again once shown.
                    None if !self.shown_files.contains(&path) => return,
                    None => match self.thumb_order.pop_front() {
                        Some(old) => {
                            self.thumbs.remove(&old);
                        }
                        None => break,
                    },
                }
            }
            self.thumb_order.push_back(path.clone());
        }
        self.thumbs
            .insert(path.clone(), texture(ctx, path.display().to_string(), im));
    }
    pub(super) fn insert_edited(&mut self, ctx: &egui::Context, id: PhotoId, im: &image::RgbImage) {
        if !self.edited.contains_key(&id) {
            while self.edited.len() >= self.kept() {
                let old = match oldest_not_shown(&mut self.edited_order, &self.shown_photos) {
                    Some(old) => old,
                    // As for thumbnails: not kept, so asked for again.
                    None if !self.shown_photos.contains(&id) => {
                        self.edited_requested.remove(&id);
                        return;
                    }
                    None => match self.edited_order.pop_front() {
                        Some(old) => old,
                        None => break,
                    },
                };
                self.edited.remove(&old);
                self.edited_requested.remove(&old);
            }
            self.edited_order.push_back(id);
        }
        self.edited
            .insert(id, texture(ctx, format!("edited-{id}"), im));
    }
    /// Shows `image`, an sRGB preview of photo `id`'s edit answering its
    /// request `ticket`: at once without a monitor profile, else once the
    /// worker has converted it, the one shown until then staying.
    fn show_edited(
        &mut self,
        ctx: &egui::Context,
        id: PhotoId,
        ticket: u64,
        image: &image::RgbImage,
    ) {
        if !self.display.converts() {
            self.insert_edited(ctx, id, image);
        } else if self
            .edit_tx
            .send(EditJob::Convert {
                id,
                ticket,
                image: image.clone(),
                display: self.display.clone(),
            })
            .is_ok()
        {
            self.edits_pending += 1;
        }
    }
    /// Shows a preview built for photo `id` (Build Standard-Sized Previews)
    /// as its edited thumbnail, answering its latest request.
    pub(super) fn show_built(&mut self, ctx: &egui::Context, id: PhotoId, image: &image::RgbImage) {
        match self.edited_requested.get(&id).copied() {
            Some(ticket) => self.show_edited(ctx, id, ticket, image),
            None if !self.display.converts() => self.insert_edited(ctx, id, image),
            None => {}
        }
    }
    /// The ticket of the photo's latest edited preview request, if any.
    pub(super) fn edited_ticket(&self, id: PhotoId) -> Option<u64> {
        self.edited_requested.get(&id).copied()
    }
    fn ticket(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }
    /// Asks for the photo's edited preview again the next time it is shown,
    /// keeping the one shown until then.
    pub(super) fn ask_again(&mut self, id: PhotoId) {
        self.edited_requested.remove(&id);
        self.unedited.remove(&id);
    }
    /// The file at `path` was saved again: its thumbnail is made again the
    /// next time it is shown, the one shown staying until the new one comes.
    pub(super) fn renew_file(&mut self, path: &std::path::Path) {
        self.failed.remove(path);
        self.renew
            .insert(path.to_path_buf(), self.pending.contains(path));
    }
    /// Forgets a removed photo's previews, so a later photo given its id
    /// starts afresh.
    pub(super) fn forget(&mut self, id: PhotoId) {
        self.edited.remove(&id);
        self.edited_order.retain(|other| *other != id);
        self.edited_requested.remove(&id);
        self.unedited.remove(&id);
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
    /// which `edit` looks up when it is first asked for. A photo without an
    /// edit whose file has no embedded preview to show, such as a film
    /// scanner's DNG, whose previews are not JPEG, is developed with
    /// `defaults` instead, as Develop would open it.
    pub(super) fn request(
        &mut self,
        photo: &Photo,
        ctx: &egui::Context,
        edit: impl FnOnce() -> Option<EditSource>,
        defaults: impl FnOnce() -> EditSource,
    ) {
        if !self.edited.contains_key(&photo.id) {
            self.request_thumbnail(&photo.path, ctx);
        }
        self.request_edited(photo, edit);
        // Found without an edit, now or before its thumbnail failed.
        if self.failed.contains(&photo.path) && self.unedited.remove(&photo.id) {
            self.request_stand_in(photo, defaults());
        }
    }
    /// Queues the photo's preview developed with `defaults`, in place of the
    /// embedded preview its file can't give.
    fn request_stand_in(&mut self, photo: &Photo, defaults: EditSource) {
        let ticket = self.ticket();
        self.edited_requested.insert(photo.id, ticket);
        if self
            .edit_tx
            .send(EditJob::Render {
                id: photo.id,
                ticket,
                path: photo.path.clone(),
                copy: self.copies.get(&photo.id).cloned(),
                source: defaults,
                ahead: false,
                display: self.display.clone(),
            })
            .is_ok()
        {
            self.edits_pending += 1;
            self.stand_ins.insert(ticket, photo.path.clone());
        }
    }
    /// The defaults changed: previews still being developed with the old ones
    /// in place of an embedded preview are no longer the latest, so what they
    /// bring is dropped, and their photos are developed again with the new
    /// ones the next time they are shown.
    pub(super) fn defaults_changed(&mut self) {
        let stand_ins = &self.stand_ins;
        self.edited_requested
            .retain(|_, ticket| !stand_ins.contains_key(ticket));
    }
    pub(super) fn request_thumbnail(&mut self, path: &Path, ctx: &egui::Context) {
        self.thumb_seen.insert(path.to_path_buf());
        if (self.thumbs.contains_key(path) && !self.renew.contains_key(path))
            || self.failed.contains(path)
        {
            return;
        }
        // The files shown this frame are published only at the next: a
        // worker taking this one before then, queued now or earlier, makes
        // it, not skips it.
        if let Some(shown) = self
            .thumb_shown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            shown.insert(path.to_path_buf());
        }
        let job = ThumbJob {
            path: path.to_path_buf(),
            display: self.display.clone(),
        };
        if !self.pending.contains(path) && self.thumb_tx.try_send(job).is_ok() {
            self.pending.insert(path.to_path_buf());
            self.progress.queued();
            ctx.request_repaint();
        }
    }
    /// Queues an edited preview for a photo with a saved or Lightroom edit.
    /// A photo without one is asked about once, until it is forgotten or (see
    /// [`Self::request`]) its file's embedded preview fails.
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
        let Some(source) = edit() else {
            self.unedited.insert(photo.id);
            return;
        };
        if self
            .edit_tx
            .send(EditJob::Render {
                id: photo.id,
                ticket,
                path: photo.path.clone(),
                copy: self.copies.get(&photo.id).cloned(),
                source,
                ahead: false,
                display: self.display.clone(),
            })
            .is_ok()
        {
            self.edits_pending += 1;
        }
    }
    /// Where offline photos' renders from their editable previews are looked
    /// up, from now on. A photo whose source changes (its original back
    /// online, or offline again with a copy) is asked for again the next time
    /// it is shown, and a request made before is no longer the latest, so its
    /// result is dropped: a render from the copy never stays as the
    /// original's.
    pub(super) fn set_copies(&mut self, copies: HashMap<PhotoId, PathBuf>) {
        let changed: Vec<PhotoId> = self
            .copies
            .iter()
            .filter(|(id, path)| copies.get(id) != Some(*path))
            .map(|(id, _)| *id)
            .chain(
                copies
                    .keys()
                    .filter(|id| !self.copies.contains_key(id))
                    .copied(),
            )
            .collect();
        for id in changed {
            self.ask_again(id);
            self.kept.remove(&id);
        }
        self.copies = copies;
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
            let Some(source) = edit(photo.id) else {
                self.unedited.insert(photo.id);
                continue;
            };
            if self
                .edit_tx
                .send(EditJob::Render {
                    id: photo.id,
                    ticket,
                    path: photo.path.clone(),
                    copy: self.copies.get(&photo.id).cloned(),
                    source,
                    ahead: true,
                    display: self.display.clone(),
                })
                .is_ok()
            {
                self.ahead.insert(ticket);
            }
        }
    }
    /// Shows Develop's latest render as the photo's thumbnail and caches it
    /// under the edit `source` it was rendered with and what it shows of AI
    /// Denoise (`ai`), for the file as `stamp` gives it when it was rendered
    /// (none: it is shown, not cached). Renders still in flight are older.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn store_edited(
        &mut self,
        ctx: &egui::Context,
        id: PhotoId,
        path: PathBuf,
        stamp: Option<crate::catalog::preview_cache::Stamp>,
        source: &EditSource,
        ai: previews::PreviewAi,
        image: image::RgbImage,
    ) {
        let ticket = self.ticket();
        self.edited_requested.insert(id, ticket);
        self.unedited.remove(&id);
        self.show_edited(ctx, id, ticket, &image);
        if let Some(stamp) = stamp {
            let _ = self.edit_tx.send(EditJob::Store {
                path,
                family: source.tag(),
                tag: source.preview_tag(ai),
                stamp,
                image,
            });
        }
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
/// Takes the oldest of `order` not shown in the last frame; `None` when all
/// are. Dropping one shown would have it asked for again at once.
fn oldest_not_shown<K: Eq + std::hash::Hash>(
    order: &mut VecDeque<K>,
    shown: &HashSet<K>,
) -> Option<K> {
    let at = order.iter().position(|k| !shown.contains(k))?;
    order.remove(at)
}
fn texture(ctx: &egui::Context, name: String, im: &image::RgbImage) -> egui::TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgb([im.width() as usize, im.height() as usize], im.as_raw()),
        egui::TextureOptions::LINEAR,
    )
}
