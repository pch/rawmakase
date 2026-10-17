//! Lightroom's Loupe in the Library: the active photo large, with the
//! filmstrip below. An online RAW is loaded and drawn by Develop's own
//! pipeline (see `Editor::loupe_viewport`); this module shows the rest. A
//! JPEG, TIFF or PNG is decoded in the background and shown no larger than
//! the view, and the next one in the direction of travel is prepared ahead
//! and kept, within a budget, until it is shown. An offline photo shows its
//! cached preview.
use super::{Action, Library, thumbnails};
use crate::app::navigator::Zoom;
use crate::app::theme;
use crate::app::worker::Latest;
use crate::catalog::{Photo, PhotoId};
use eframe::egui::{self, Color32, Vec2};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, channel},
};

/// Views are rendered in steps of this many pixels, so resizing the window
/// a little does not render the photo again.
const EDGE_STEP: u32 = 512;
/// Prepared neighbours kept, in bytes; the oldest go first.
const PREFETCHED_BYTES: usize = 64 << 20;

/// A preview as asked for: photo, file (photo ids can be reused) and edge.
type Key = (PhotoId, PathBuf, u32);

struct Job {
    ticket: u64,
    /// A neighbour prepared ahead, kept under this key.
    ahead: Option<Key>,
    path: PathBuf,
    /// Longest side, in pixels, of the image wanted.
    edge: u32,
    /// For an offline RAW: the identity of its stored Standard preview, read
    /// from the preview cache instead of the file.
    stored: Option<String>,
    cancel: Arc<AtomicBool>,
}
struct Done {
    ticket: u64,
    ahead: Option<Key>,
    result: Result<Prepared, String>,
}
/// A preview for the view, and the size of the image it was made from.
struct Prepared {
    image: image::RgbImage,
    full: (u32, u32),
}
/// What the Loupe shows under the photo.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum State {
    Loading,
    Ready,
    Failed(String),
}

pub(super) struct Loupe {
    pub open: bool,
    pub regions: super::zoom::Regions,
    worker: Latest<Job>,
    /// Prepares the neighbour, on its own so it never holds up the photo shown.
    ahead: Latest<Job>,
    ahead_cancel: Arc<AtomicBool>,
    /// The neighbour asked for last, and those ready, oldest first.
    ahead_requested: Option<Key>,
    prefetched: std::collections::VecDeque<(Key, Prepared)>,
    results: Receiver<Done>,
    ticket: u64,
    /// What the current ticket asked for.
    requested: Option<Key>,
    cancel: Arc<AtomicBool>,
    texture: Option<egui::TextureHandle>,
    /// The size of the image shown, and of the view in pixels, for its Fit.
    full: Option<(u32, u32)>,
    pub(super) view: Vec2,
    /// The zoom before the last click toggled it, and when, for a
    /// double-click to undo.
    before_click: Option<(bool, f64, Option<PhotoId>)>,
    pub state: State,
}
impl Loupe {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let (tx, results) = channel();
        let worker = |tx: std::sync::mpsc::Sender<Done>, ctx: egui::Context| {
            Latest::new(move |job: Job| {
                if job.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let send = |result| {
                    let _ = tx.send(Done {
                        ticket: job.ticket,
                        ahead: job.ahead.clone(),
                        result,
                    });
                    ctx.request_repaint();
                };
                let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let image = match &job.stored {
                        Some(identity) => stored(&job.path, identity),
                        None => crate::raster::decode(&job.path),
                    };
                    image.map(|image| Prepared {
                        full: image.dimensions(),
                        image: thumbnails::downscale(&image, job.edge),
                    })
                }));
                match prepared {
                    _ if job.cancel.load(Ordering::Relaxed) => {}
                    Ok(Ok(image)) => send(Ok(image)),
                    Ok(Err(e)) => send(Err(format!("{e:#}"))),
                    Err(_) => send(Err("The preview could not be built".into())),
                }
            })
        };
        Self {
            open: false,
            regions: super::zoom::Regions::new(ctx),
            worker: worker(tx.clone(), ctx.clone()),
            ahead: worker(tx, ctx.clone()),
            ahead_cancel: Default::default(),
            ahead_requested: None,
            prefetched: Default::default(),
            results,
            ticket: 0,
            requested: None,
            cancel: Default::default(),
            texture: None,
            full: None,
            view: Vec2::ZERO,
            before_click: None,
            state: State::Loading,
        }
    }
    /// Asks for `photo` at `edge` pixels, unless that is already on its way;
    /// from its stored preview with that identity when `stored` says so.
    fn request(&mut self, ctx: &egui::Context, photo: &Photo, edge: u32, stored: Option<String>) {
        let wanted = key(photo, edge);
        let asked = self.requested.as_ref() == Some(&wanted);
        if asked && self.state == State::Ready {
            return;
        }
        // Prepared ahead, perhaps finishing after it was asked for again:
        // shown at once.
        if let Some(at) = self.prefetched.iter().position(|(k, _)| *k == wanted) {
            let (_, prepared) = self.prefetched.remove(at).unwrap();
            self.cancel.store(true, Ordering::Relaxed);
            self.ticket += 1;
            self.requested = Some(wanted);
            self.show(ctx, prepared);
            return;
        }
        if asked {
            return;
        }
        let edge = wanted.2;
        let same_photo = self.requested.as_ref().is_some_and(|r| r.0 == photo.id);
        self.requested = Some(wanted);
        // Moving on cancels the photo being prepared; its result is dropped.
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Default::default();
        self.ticket += 1;
        if !same_photo {
            self.texture = None;
            self.full = None;
            self.state = State::Loading;
        }
        self.worker.submit(Job {
            ticket: self.ticket,
            ahead: None,
            path: photo.path.clone(),
            edge,
            stored,
            cancel: self.cancel.clone(),
        });
    }
    /// Prepares `photo`, the next one along, unless it is ready or on its way.
    fn prepare_ahead(&mut self, photo: &Photo, edge: u32) {
        let wanted = key(photo, edge);
        if self.ahead_requested.as_ref() == Some(&wanted)
            || self.prefetched.iter().any(|(k, _)| *k == wanted)
        {
            return;
        }
        self.ahead_cancel.store(true, Ordering::Relaxed);
        self.ahead_cancel = Default::default();
        self.ahead_requested = Some(wanted.clone());
        self.ahead.submit(Job {
            ticket: 0,
            ahead: Some(wanted.clone()),
            path: photo.path.clone(),
            edge: wanted.2,
            stored: None,
            cancel: self.ahead_cancel.clone(),
        });
    }
    fn show(&mut self, ctx: &egui::Context, Prepared { image, full }: Prepared) {
        self.full = Some(full);
        let size = [image.width() as usize, image.height() as usize];
        self.texture = Some(ctx.load_texture(
            "library-loupe",
            egui::ColorImage::from_rgb(size, image.as_raw()),
            egui::TextureOptions::LINEAR,
        ));
        self.state = State::Ready;
    }
    /// Lets go of what the Loupe's own preview holds, unless already idle.
    fn idle(&mut self) {
        if self.requested.is_some() || self.texture.is_some() || self.regions.region.is_some() {
            self.reset();
        }
    }
    /// Forgets the photo shown and lets go of what is held for it.
    pub(super) fn reset(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.ahead_cancel.store(true, Ordering::Relaxed);
        self.ahead_requested = None;
        self.prefetched.clear();
        self.regions.release();
        self.requested = None;
        self.full = None;
        self.texture = None;
        self.state = State::Loading;
    }
    fn poll(&mut self, ctx: &egui::Context) {
        self.regions.poll(ctx);
        while let Ok(done) = self.results.try_recv() {
            if let Some(key) = done.ahead {
                if let Ok(image) = done.result {
                    self.prefetched.push_back((key, image));
                    let bytes = |p: &std::collections::VecDeque<(Key, Prepared)>| {
                        p.iter().map(|(_, p)| p.image.as_raw().len()).sum::<usize>()
                    };
                    while bytes(&self.prefetched) > PREFETCHED_BYTES {
                        self.prefetched.pop_front();
                    }
                }
                continue;
            }
            if done.ticket != self.ticket {
                continue;
            }
            match done.result {
                Ok(prepared) => self.show(ctx, prepared),
                Err(e) => self.state = State::Failed(e),
            }
        }
    }
    #[cfg(test)]
    pub(super) fn wait(&mut self, ctx: &egui::Context) {
        let started = std::time::Instant::now();
        while self.state == State::Loading && started.elapsed() < std::time::Duration::from_secs(20)
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll(ctx);
        }
    }
    #[cfg(test)]
    pub(super) fn wait_ahead(&mut self, ctx: &egui::Context) {
        let started = std::time::Instant::now();
        while self.prefetched.is_empty() && started.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll(ctx);
        }
    }
    #[cfg(test)]
    pub(super) fn texture_size(&self) -> Option<[usize; 2]> {
        self.texture.as_ref().map(|t| t.size())
    }
}

/// The stored Standard preview of an offline RAW, else its 1:1 preview.
fn stored(path: &std::path::Path, identity: &str) -> anyhow::Result<image::RgbImage> {
    use crate::catalog::preview_cache::{PreviewCache, PreviewKind};
    let cache = PreviewCache::open(&PreviewCache::path())?;
    match cache.load_sized(path, identity, PreviewKind::Standard)? {
        Some(image) => Ok(image),
        None => cache
            .load_sized(path, identity, PreviewKind::OneToOne)?
            .ok_or_else(|| anyhow::anyhow!("No stored preview")),
    }
}
fn key(photo: &Photo, edge: u32) -> Key {
    (
        photo.id,
        photo.path.clone(),
        edge.div_ceil(EDGE_STEP).max(1) * EDGE_STEP,
    )
}

impl Library {
    pub(crate) fn loupe_open(&self) -> bool {
        self.loupe.open
    }
    /// E, Return or a double-click: the active photo, large.
    pub(crate) fn open_loupe(&mut self) {
        if self.selection.active.is_none() {
            self.select(self.visible.first().map(|i| self.session.photos[*i].id));
        }
        self.loupe.open = self.selection.active.is_some();
        if self.loupe.open {
            self.compare.open = false;
            self.survey.open = false;
        }
        self.loupe.before_click = None;
        self.scroll_to_active = true;
    }
    /// The offline photo the Loupe shows that a stored preview can stand in for:
    /// one it would develop (see [`Self::loupe_develops`]).
    pub(in crate::app) fn loupe_offline_developed(&self) -> Option<PhotoId> {
        let photo = self.selection.active.and_then(|id| self.photo(id))?;
        (self.loupe.open && !self.is_available(&photo.path) && self.develops(photo))
            .then_some(photo.id)
    }
    /// The identity of the stored preview the Loupe shows for an offline photo it
    /// would develop.
    pub(in crate::app) fn set_loupe_stored(&mut self, stored: Option<(PhotoId, String)>) {
        self.loupe_stored = stored;
    }
    pub(in crate::app) fn loupe_stored(&self) -> Option<&(PhotoId, String)> {
        self.loupe_stored.as_ref()
    }
    /// The photo the Loupe shows through Develop's pipeline: the active photo,
    /// when it is online and a RAW or an edited JPEG, TIFF or PNG. An unedited
    /// one, which its defaults reproduce, and offline photos are shown by the
    /// Loupe's own, quicker preview instead.
    pub(crate) fn loupe_develops(&self) -> Option<PhotoId> {
        let photo = self.selection.active.and_then(|id| self.photo(id))?;
        (self.loupe.open && self.is_available(&photo.path) && self.develops(photo))
            .then_some(photo.id)
    }
    /// Whether `photo` is shown developed: a RAW always, a JPEG, TIFF or PNG once
    /// it has an edit of its own.
    pub(super) fn develops(&self, photo: &Photo) -> bool {
        crate::storage::is_raw(&photo.path)
            || self.session.catalog.has_edit(photo.id).unwrap_or(false)
    }
    /// The zoom level at which the photo in the Loupe's own view fits, for
    /// stepping through the levels from it.
    pub(in crate::app) fn loupe_fit(&self) -> Option<f32> {
        let (w, h) = self.loupe.full?;
        let view = self.loupe.view;
        Some((view.x / w as f32).min(view.y / h as f32))
    }
    /// What the Navigator shows for a photo in the Loupe's own view: its
    /// preview, and the part in view when zoomed.
    pub(in crate::app) fn loupe_navigator(
        &self,
    ) -> Option<(crate::app::navigator::Photo, Option<[f32; 4]>)> {
        let photo = self.selection.active.and_then(|id| self.photo(id))?;
        // The Loupe's own preview once it is this photo's; until then the
        // grid's.
        let texture = self
            .loupe
            .texture
            .as_ref()
            .filter(|_| {
                self.loupe
                    .requested
                    .as_ref()
                    .is_some_and(|r| r.0 == photo.id)
            })
            .filter(|_| self.is_available(&photo.path))
            .or_else(|| self.texture(photo))?;
        let shown = self
            .loupe
            .regions
            .region
            .as_ref()
            .filter(|_| self.loupe.regions.photo == Some(photo.id))
            .map(|(_, rect)| *rect);
        Some(((texture.id(), texture.size_vec2()), shown))
    }
    /// A click in the Loupe toggled the zoom away from `before`.
    pub(in crate::app) fn loupe_zoom_toggled(&mut self, before: bool) {
        let at = self.ctx.input(|i| i.time);
        self.loupe.before_click = Some((before, at, self.selection.active));
    }
    /// A double-click: back to the grid, as in Lightroom. Returns the zoom
    /// from before its first click, which is undone.
    pub(in crate::app) fn loupe_double_click(&mut self) -> Option<bool> {
        // Only a toggle by this double-click's own first click counts.
        let now = self.ctx.input(|i| i.time);
        let before = self
            .loupe
            .before_click
            .take()
            .filter(|(_, at, photo)| now - at < 1. && *photo == self.selection.active)
            .map(|(on, ..)| on);
        self.close_loupe();
        before
    }
    /// G or Esc: back to the grid, at the active photo.
    pub(crate) fn close_loupe(&mut self) {
        self.loupe.before_click = None;
        if self.loupe.open {
            self.loupe.open = false;
            self.loupe.reset();
            self.scroll_to_active = true;
        }
    }
    /// The Loupe in place of the grid, with its toolbar below.
    pub(super) fn loupe(&mut self, ui: &mut egui::Ui, zoom: &mut Zoom, chrome: bool) -> Action {
        let palette = theme::palette(ui.ctx());
        self.loupe.poll(ui.ctx());
        let Some(id) = self.selection.active else {
            self.close_loupe();
            return Action::None;
        };
        // On the photo's backdrop, as the view above it.
        if chrome {
            egui::Panel::bottom("library-loupe-toolbar")
                .frame(
                    egui::Frame::new()
                        .fill(palette.photo_backdrop())
                        .inner_margin(egui::Margin::symmetric(10, 4)),
                )
                .show_separator_line(false)
                .show(ui, |ui| ui.horizontal(|ui| self.view_buttons(ui)));
        }
        let Some(photo) = self.photo(id).cloned() else {
            return Action::None;
        };
        // A photo developed is drawn by the editor, through Develop's viewport.
        if self.loupe_develops().is_some() {
            self.loupe.idle();
            return Action::None;
        }
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        ui.painter().rect_filled(rect, 0., palette.photo_backdrop());
        let ppp = ui.ctx().pixels_per_point();
        // The photo fits within the margin, as drawn below.
        self.loupe.view = rect.shrink(crate::app::workspace::PHOTO_MARGIN).size() * ppp;
        let available = self.is_available(&photo.path);
        let edge = (rect.width().max(rect.height()) * ppp) as u32;
        // An offline RAW with a stored Standard preview shows it.
        let stored = self
            .loupe_stored
            .as_ref()
            .filter(|(id, _)| !available && *id == photo.id)
            .map(|(_, identity)| identity.clone());
        if let Some(identity) = stored.clone() {
            self.loupe.request(ui.ctx(), &photo, edge, Some(identity));
        } else if available {
            self.loupe.request(ui.ctx(), &photo, edge, None);
            // Once this photo is ready, the next one along is prepared.
            if self.loupe.state == State::Ready
                && let Some(next) = self
                    .navigate(photo.id, self.loupe_direction)
                    .filter(|n| *n != photo.id)
                    .and_then(|n| self.photo(n).cloned())
                && self.is_available(&next.path)
                // A photo developed opens through Develop's pipeline instead.
                && !self.develops(&next)
            {
                self.loupe.prepare_ahead(&next, edge);
            }
        } else if self.loupe.requested.is_some() {
            self.loupe.reset();
        }
        self.request_previews(&photo, ui.ctx());
        let inset = rect.shrink(crate::app::workspace::PHOTO_MARGIN);
        // Until its own preview is in, the grid's stands in, enlarged.
        let own = available
            || stored.is_some()
                && self.loupe.state == State::Ready
                && self
                    .loupe
                    .requested
                    .as_ref()
                    .is_some_and(|r| r.0 == photo.id);
        let texture = self
            .loupe
            .texture
            .clone()
            .filter(|_| own)
            .or_else(|| self.texture(&photo).cloned());
        let fit = texture.as_ref().map(|t| {
            let size = t.size_vec2();
            let scale = (inset.width() / size.x).min(inset.height() / size.y);
            egui::Rect::from_center_size(inset.center(), size * scale)
        });
        // A click zooms in at the point clicked, and back; a drag pans. The
        // second click of a double-click is not another toggle, and zooming
        // in starts on the photo, not in the margin around it.
        let on_photo = response
            .interact_pointer_pos()
            .zip(fit)
            .is_some_and(|(pos, fit)| fit.contains(pos));
        let double = response.double_clicked() || response.triple_clicked();
        // An offline RAW zooms into its stored 1:1 preview.
        let zoomable = available || stored.is_some();
        if response.clicked() && !double && zoomable && (zoom.on || on_photo) {
            if let Some((pos, fit)) = response
                .interact_pointer_pos()
                .zip(fit)
                .filter(|_| !zoom.on)
            {
                zoom.pan = [
                    ((pos.x - fit.left()) / fit.width()).clamp(0., 1.),
                    ((pos.y - fit.top()) / fit.height()).clamp(0., 1.),
                ];
            }
            self.loupe_zoom_toggled(zoom.on);
            zoom.on = !zoom.on;
        }
        // As in Lightroom, a double-click goes back to the grid; its first
        // click's zoom is undone.
        if double {
            if let Some(on) = self.loupe_double_click() {
                zoom.on = on;
            }
            return Action::None;
        }
        if !zoom.on && self.loupe.regions.region.is_some() {
            self.loupe.regions.release();
        }
        let regions = &mut self.loupe.regions;
        let full = regions
            .full
            .filter(|_| zoom.on && zoomable && regions.photo == Some(photo.id));
        let shown = match full {
            Some(full) => {
                // `level` screen pixels per image pixel.
                let size = egui::vec2(full[0] as f32, full[1] as f32) * zoom.level / ppp;
                if response.dragged() {
                    let delta = response.drag_delta();
                    zoom.pan[0] -= delta.x / size.x;
                    zoom.pan[1] -= delta.y / size.y;
                }
                // The photo covers the view where it is larger, and is
                // centred where it is smaller.
                for i in 0..2 {
                    let half = rect.size()[i] / 2. / size[i];
                    zoom.pan[i] = if half >= 0.5 {
                        0.5
                    } else {
                        zoom.pan[i].clamp(half, 1. - half)
                    };
                }
                Some(egui::Rect::from_min_size(
                    rect.center() - egui::vec2(zoom.pan[0] * size.x, zoom.pan[1] * size.y),
                    size,
                ))
            }
            None => fit,
        };
        if zoom.on && zoomable {
            // The image pixels that fill the view at this level.
            let size =
                [rect.width(), rect.height()].map(|side| (side * ppp / zoom.level).ceil() as u32);
            let stored = stored.as_deref().filter(|_| !available);
            regions.request(photo.id, &photo.path, stored, zoom.pan, size, zoom.level);
        }
        let painter = ui.painter().with_clip_rect(rect);
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.));
        if let (Some(texture), Some(at)) = (&texture, shown) {
            painter.image(texture.id(), at, uv, Color32::WHITE);
        }
        // The zoomed region over the enlarged Fit preview.
        if let (Some(at), true) = (shown, full.is_some())
            && let Some((region, [x, y, w, h])) = &regions.region
        {
            let place = egui::Rect::from_min_size(
                at.min + egui::vec2(x * at.width(), y * at.height()),
                egui::vec2(w * at.width(), h * at.height()),
            );
            painter.image(region.id(), place, uv, Color32::WHITE);
        }
        if zoom.on && zoomable {
            ui.ctx().set_cursor_icon(if response.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        let note = match (&self.loupe.state, available, texture.is_some()) {
            _ if zoom.on && stored.is_some() => match &self.loupe.regions.error {
                Some(e) => format!("Zoom unavailable offline: {e}"),
                None if self.loupe.regions.pending || full.is_none() => "Loading…".into(),
                None => "Offline: showing the 1:1 preview".into(),
            },
            (_, false, true) => {
                "Offline: showing the cached preview. Full detail needs the original.".into()
            }
            (_, false, false) => "Offline, and there is no cached preview".into(),
            _ if zoom.on && self.loupe.regions.error.is_some() => format!(
                "Zoom unavailable: {}",
                self.loupe.regions.error.clone().unwrap_or_default()
            ),
            _ if zoom.on && (self.loupe.regions.pending || full.is_none()) => "Loading…".into(),
            (State::Loading, ..) => "Loading…".into(),
            (State::Ready, ..) => String::new(),
            (State::Failed(e), ..) => format!("Preview unavailable: {e}"),
        };
        if !note.is_empty() {
            ui.painter().text(
                rect.left_bottom() + Vec2::new(12., -10.),
                egui::Align2::LEFT_BOTTOM,
                note,
                egui::FontId::proportional(11.),
                palette.gray(170),
            );
        }
        self.loupe_overlay(ui.painter(), rect);
        Action::None
    }
}
