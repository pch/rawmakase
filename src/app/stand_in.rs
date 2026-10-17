//! The stored Standard preview Develop shows while a photo opens (issue #213),
//! until its first live render. It is only something to look at: nothing that
//! reads the photo (samples, the readout, white balance picking, the histogram,
//! tools, Before/After, export) ever reads it.
//!
//! Workers read the preview cache off the UI thread, decode the JPEG and
//! convert it through the monitor profile: one for the photo being opened and
//! one for its neighbour, so preparing the neighbour never replaces or holds up
//! the photo's own read. The last two neighbours prepared are kept, so moving
//! on can show one in the first frame.
use super::Editor;
use super::worker::{Event, Latest};
use crate::catalog::PhotoId;
use crate::catalog::preview_cache::{PreviewCache, PreviewKind};
use eframe::egui;
use std::{collections::VecDeque, path::PathBuf, sync::mpsc::Sender};

/// Prepared neighbours kept.
const KEPT: usize = 2;

/// Which stored preview, for which photo, as which load asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Wanted {
    pub photo: PhotoId,
    pub path: PathBuf,
    pub identity: String,
    pub monitor: Option<PathBuf>,
}

struct Job {
    /// The load it is for; `None` for a neighbour prepared ahead.
    load: Option<u64>,
    wanted: Wanted,
}

/// A stored preview read, ready to upload.
#[derive(Clone)]
pub(crate) struct StandIn {
    pub load: Option<u64>,
    pub wanted: Wanted,
    pub image: egui::ColorImage,
}

pub(super) struct StandIns {
    /// Reads the photo being opened.
    opening: Latest<Job>,
    /// Reads the neighbour, on its own thread so a read stalled on a network
    /// share never holds up the photo being opened.
    ahead: Latest<Job>,
    /// Neighbours prepared ahead, oldest first.
    prepared: VecDeque<StandIn>,
    /// The neighbour last asked for, so it is not asked for every time.
    asked: Option<Wanted>,
    /// The photo opened last and its preview identity then, to tell on leaving
    /// it whether its edit changed.
    opened: Option<(PhotoId, String)>,
}

/// A reader of stored previews, its cache opened on first use.
fn reader(tx: Sender<Event>, ctx: egui::Context, cache_path: PathBuf) -> Latest<Job> {
    let mut cache: Option<PreviewCache> = None;
    Latest::new(move |job: Job| {
        if cache.is_none() {
            cache = PreviewCache::open(&cache_path).ok();
        }
        let Some(image) = cache.as_ref().and_then(|c| {
            c.load_sized(
                &job.wanted.path,
                &job.wanted.identity,
                PreviewKind::Standard,
            )
            .ok()
            .flatten()
        }) else {
            return;
        };
        let size = [image.width() as usize, image.height() as usize];
        let mut rgb = image.into_raw();
        // As the renderer does: a failed transform shows the sRGB pixels.
        if let Some(monitor) = &job.wanted.monitor {
            let _ = crate::color_management::display_transform(monitor, &mut rgb);
        }
        let image = egui::ColorImage::from_rgb(size, &rgb);
        let _ = tx.send(Event::StandIn(Box::new(StandIn {
            load: job.load,
            wanted: job.wanted,
            image,
        })));
        ctx.request_repaint();
    })
}

impl StandIns {
    pub(super) fn new(tx: Sender<Event>, ctx: egui::Context, cache_path: PathBuf) -> Self {
        Self {
            opening: reader(tx.clone(), ctx.clone(), cache_path.clone()),
            ahead: reader(tx, ctx, cache_path),
            prepared: VecDeque::new(),
            asked: None,
            opened: None,
        }
    }
    /// Stops both readers at exit; each ends after the read in hand.
    pub(super) fn stop(&mut self) -> [super::task::Stopping; 2] {
        [self.opening.stop(), self.ahead.stop()]
    }
    /// A neighbour already prepared, taken out to be shown.
    fn take_prepared(&mut self, wanted: &Wanted) -> Option<StandIn> {
        let at = self.prepared.iter().position(|p| p.wanted == *wanted)?;
        self.prepared.remove(at)
    }
    fn keep(&mut self, stand_in: StandIn) {
        self.prepared
            .retain(|p| p.wanted.photo != stand_in.wanted.photo);
        self.prepared.push_back(stand_in);
        while self.prepared.len() > KEPT {
            self.prepared.pop_front();
        }
    }
}

impl Editor {
    /// What a catalog photo's stored preview would be named by now.
    fn stand_in_for(&self, photo: PhotoId) -> Option<Wanted> {
        let library = self.library.as_ref()?;
        let path = library.photo(photo)?.path.clone();
        let record = library.session.catalog.edit_record(photo).ok()?;
        // A JPEG, TIFF or PNG without an edit is shown as its file is, and never
        // stored (see `build_items`).
        if !crate::storage::is_raw(&path) && !super::preview_build::has_edit(&record) {
            return None;
        }
        Some(Wanted {
            photo,
            path,
            identity: super::preview_build::identity(&record, &self.raw_defaults, self.demosaic),
            monitor: self.view.monitor.clone(),
        })
    }
    /// Asks for the stored preview of the photo load `load` opens, showing a
    /// prepared one at once, and prepares the neighbour's.
    pub(super) fn request_stand_ins(
        &mut self,
        load: u64,
        photo: PhotoId,
        neighbour: Option<PhotoId>,
    ) {
        // A preview asked for and left stale (its edit saved as the app quit or
        // the catalog closed) is built again once the photo is opened.
        self.refresh_previews(&[photo]);
        let wanted = self.stand_in_for(photo);
        self.stand_ins.opened = wanted.as_ref().map(|w| (photo, w.identity.clone()));
        if let Some(wanted) = wanted {
            match self.stand_ins.take_prepared(&wanted) {
                Some(ready) => self.show_stand_in(&self.context.clone(), ready.image),
                None => self.stand_ins.opening.submit(Job {
                    load: Some(load),
                    wanted,
                }),
            }
        }
        let Some(next) = neighbour.and_then(|n| self.stand_in_for(n)) else {
            return;
        };
        let known = self.stand_ins.asked.as_ref() == Some(&next)
            || self.stand_ins.prepared.iter().any(|p| p.wanted == next);
        if !known {
            self.stand_ins.asked = Some(next.clone());
            self.stand_ins.ahead.submit(Job {
                load: None,
                wanted: next,
            });
        }
    }
    /// Whether the photo open, `photo`, is left with an edit other than the one
    /// it opened with.
    pub(super) fn left_edited(&self, photo: PhotoId) -> bool {
        match &self.stand_ins.opened {
            Some((opened, identity)) if *opened == photo => self
                .stand_in_for(photo)
                .is_some_and(|now| now.identity != *identity),
            _ => false,
        }
    }
    /// A stored preview read: shown when it is for the load under way and no live
    /// render has arrived; a neighbour's is kept for when it opens.
    pub(super) fn stand_in_ready(&mut self, ctx: &egui::Context, stand_in: StandIn) {
        match stand_in.load {
            Some(load) => {
                if load == self.load.id()
                    && self.document.catalog_photo == Some(stand_in.wanted.photo)
                    && !self.preview.live()
                {
                    self.show_stand_in(ctx, stand_in.image);
                }
            }
            None => {
                if self.stand_ins.asked.as_ref() == Some(&stand_in.wanted) {
                    self.stand_ins.asked = None;
                }
                self.stand_ins.keep(stand_in);
            }
        }
    }
    fn show_stand_in(&mut self, ctx: &egui::Context, image: egui::ColorImage) {
        super::state::Picture::upload(&mut self.preview.stand_in, ctx, "stand-in", image);
    }
}

impl Editor {
    /// An offline RAW in the Loupe shows its stored Standard preview: its
    /// identity, worked out once each time the Loupe comes to it.
    pub(super) fn loupe_stored_preview(&mut self) {
        let Some(library) = &self.library else {
            return;
        };
        let offline = library.loupe_offline_developed();
        if offline == library.loupe_stored().map(|(id, _)| *id) {
            return;
        }
        let stored = offline
            .and_then(|id| self.stand_in_for(id))
            .map(|w| (w.photo, w.identity));
        if let Some(library) = &mut self.library {
            // A photo without one is asked about once, as `None` under its id.
            library.set_loupe_stored(stored.or(offline.map(|id| (id, String::new()))));
        }
    }
}

#[cfg(test)]
mod tests;
