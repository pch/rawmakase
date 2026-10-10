use super::worker::{LoadJob, RenderJob};
use super::{Editor, state::Picture};
use crate::model::recipe::Recipe;
use crate::{
    app::Module,
    catalog::{CatalogLocation, PhotoId},
    develop::Geometry,
};
use eframe::egui::{self, Vec2};
use std::{path::PathBuf, sync::Arc, time::Instant};

/// How saving the edit before closing ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Flushed {
    Saved,
    /// Not saved: the error is on the status line.
    Failed,
    /// Still being saved when time ran out.
    Late,
}

impl Editor {
    pub(super) fn open(&mut self, path: PathBuf) {
        let catalog = path
            .extension()
            .is_some_and(|e| e == "rawmakase" || e == "lrcat");
        if !catalog {
            // Photos are edited through the Library only.
            self.add_to_library(path);
            return;
        }
        if self.activity.is_busy() {
            return;
        }
        if path.extension().is_some_and(|e| e == "rawmakase") {
            self.load_catalog(path, &self.context.clone());
        } else if path.extension().is_some_and(|e| e == "lrcat") {
            self.status =
                "Use Library → Import Lightroom catalog to select a new RAWmakase catalog destination"
                    .into();
            self.module = Module::Library;
        }
    }
    pub(super) fn open_raw(&mut self, path: PathBuf, photo: Option<PhotoId>) {
        if self.load_raw(path, photo) {
            self.module = Module::Develop;
        }
    }
    /// Starts loading a RAW as the document, staying in the module shown:
    /// the Library's Loupe shows it through the same pipeline as Develop.
    /// False when work in progress or an unsaved edit prevents it.
    pub(super) fn load_raw(&mut self, path: PathBuf, photo: Option<PhotoId>) -> bool {
        if self.activity.is_busy() {
            return false;
        }
        if !self.flush() {
            return false;
        }
        // The photo left, saved with another edit: its built previews follow it.
        if let Some(left) = self.document.catalog_photo
            && self.left_edited(left)
        {
            self.refresh_previews(&[left]);
        }
        // Moving on cancels the previous photo's prefetch.
        self.prefetch_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.prefetch_cancel = Default::default();
        let neighbour = photo.and_then(|id| self.neighbour_of(id));
        let prefetch = photo
            .and_then(|id| self.prefetch_neighbour(id))
            .map(|path| super::worker::Prefetch {
                path,
                cancel: self.prefetch_cancel.clone(),
                demosaic: self.demosaic.effective(),
            });
        // The photo being left is Paste from Previous's source; opening the same photo
        // again (as a new demosaic setting does) leaves Previous as it was.
        let another =
            self.document.catalog_photo != photo || self.document.path.as_ref() != Some(&path);
        if another && let Some(settings) = self.current_settings() {
            self.previous_settings = Some(settings);
        }
        self.document.reset(photo);
        let (id, cancel) = self.load.start();
        // The reference photo stays on screen from photo to photo; Before goes.
        let reference = self
            .reference_view()
            .then(|| std::mem::take(&mut self.preview.before));
        self.preview.clear_document();
        if let Some(reference) = reference {
            self.preview.before = reference;
        }
        self.presets.clear_document();
        self.view.clear_document();
        self.selection.clear_document();
        if let Some(photo) = photo {
            self.request_stand_ins(id, photo, neighbour);
        }
        self.status = "Reading RAW…".into();
        self.loader.submit(LoadJob {
            id,
            path,
            cancel,
            prefetch,
            defaults: self.raw_defaults.clone(),
            demosaic: self.demosaic.effective(),
        });
        // The photo left may be the reference, or its edit may have changed.
        self.load_reference();
        true
    }
    /// The photo to decode ahead of time while `id` is shown: the next one in
    /// the filmstrip, or the previous one after stepping back.
    pub(super) fn prefetch_neighbour(&self, id: PhotoId) -> Option<PathBuf> {
        let photo = self.library.as_ref()?.photo(self.neighbour_of(id)?)?;
        (photo.path.is_file() && crate::storage::is_raw(&photo.path)).then(|| photo.path.clone())
    }
    /// The photo next to `id` in the direction of travel.
    fn neighbour_of(&self, id: PhotoId) -> Option<PhotoId> {
        let library = self.library.as_ref()?;
        let step = match self.document.catalog_photo {
            Some(previous) if previous != id && library.navigate(previous, -1) == Some(id) => -1,
            _ => 1,
        };
        library.navigate(id, step).filter(|n| *n != id)
    }
    /// Saves a Copy Name or metadata field still being typed in the Library;
    /// false, with the error on the status line, if it could not be saved.
    pub(super) fn commit_library_drafts(&mut self) -> bool {
        if let Some(library) = &mut self.library
            && let Err(e) = library.commit_drafts()
        {
            self.status = format!("Not saved: {e}");
            return false;
        }
        true
    }
    /// Saves the edit now, after any background save in flight; false if
    /// it could not be saved.
    pub(super) fn flush(&mut self) -> bool {
        if !self.commit_library_drafts() {
            return false;
        }
        // A snapshot name still being typed, as leaving the photo any way commits it.
        self.commit_snapshot_rename();
        if let Some(completion) = self.autosave.wait() {
            self.background_saved(completion);
        }
        // A slider or histogram drag still held when the photo is left (Left or Right
        // with the button down) is saved as a step of its own; History records it
        // once the save succeeds.
        if self.document.edit.history().in_gesture() {
            self.document.edit.save_state_mut().mark_changed();
        }
        if !self.document.edit.save_state().needs_save() {
            self.finish_saved_gesture();
            return true;
        }
        if let (Some(path), Some(l), Some(id)) = (
            &self.document.path,
            &mut self.library,
            self.document.catalog_photo,
        ) {
            let history = self
                .document
                .edit
                .history()
                .saved(self.document.edit.recipe());
            let saved = l
                .session
                .catalog
                .save_edit(
                    id,
                    path,
                    self.document.edit.recipe(),
                    &self.document.export,
                    crate::catalog::HistoryUpdate::of(&history),
                )
                .map(|()| l.session.catalog.location().clone());
            match saved {
                Ok(p) => {
                    self.saved_to(&p);
                    self.document.edit.save_state_mut().saved();
                }
                Err(e) => {
                    self.document.edit.save_state_mut().failed(e.to_string());
                    self.status = format!("Edits not saved: {e}");
                    return false;
                }
            }
        }
        // Its state was just saved with History: nothing new to save.
        self.finish_saved_gesture();
        true
    }
    /// Autosave: collects a finished background save and, once the edit
    /// has settled, starts the next.
    pub(super) fn autosave(&mut self, ctx: &egui::Context) {
        if let Some(completion) = self.autosave.poll() {
            self.background_saved(completion);
        }
        if !self.document.edit.save_state().ready()
            || self.document.edit.history().in_gesture()
            || self.autosave.busy()
        {
            return;
        }
        let Some(job) = self.save_job() else {
            return;
        };
        match self.autosave.submit(job, ctx) {
            Ok(()) => self.document.edit.save_state_mut().saving(),
            // No saver thread: save here, as before.
            Err(_) => {
                self.flush();
            }
        }
    }
    /// The edit as it stands, to save to the catalog photo it belongs to.
    fn save_job(&self) -> Option<super::autosave::Job> {
        let raw = self.document.path.clone()?;
        let (Some(l), Some(photo)) = (&self.library, self.document.catalog_photo) else {
            return None;
        };
        Some(super::autosave::Job {
            catalog: l.session.catalog.location().clone(),
            photo,
            raw,
            recipe: self.document.edit.recipe().clone(),
            export: self.document.export.clone(),
            history: self
                .document
                .edit
                .history()
                .saved(self.document.edit.recipe()),
        })
    }
    /// Saves the edit as [`flush`](Self::flush) does, but on the autosave thread,
    /// giving up at `until`: a catalog on a stalled network share must not hold up
    /// closing. A save still running then goes on, and the edit stays unsaved.
    pub(super) fn flush_by(&mut self, until: Instant) -> Flushed {
        if !self.commit_library_drafts() {
            return Flushed::Failed;
        }
        self.commit_snapshot_rename();
        if self.document.edit.history().in_gesture() {
            self.document.edit.save_state_mut().mark_changed();
        }
        // An edit closed without saving leaves the save in flight to finish alone.
        if self.document.edit.save_state().needs_save() {
            if let Some(completion) = self.autosave.wait_until(until) {
                self.background_saved(completion);
            }
            if self.autosave.busy() {
                return Flushed::Late;
            }
        }
        if self.document.edit.save_state().needs_save()
            && let Some(job) = self.save_job()
        {
            // A saver lost since the last save is started again by a second
            // submit; saving here instead could stall as long as the catalog does.
            let submitted = self
                .autosave
                .submit(job, &self.context)
                .or_else(|job| self.autosave.submit(*job, &self.context));
            if submitted.is_err() {
                let error = "the autosave thread could not start".to_string();
                self.document.edit.save_state_mut().failed(error.clone());
                self.status = format!("Edits not saved: {error}");
                return Flushed::Failed;
            }
            self.document.edit.save_state_mut().saving();
            if let Some(completion) = self.autosave.wait_until(until) {
                self.background_saved(completion);
            }
            if self.autosave.busy() {
                return Flushed::Late;
            }
            if self.document.edit.save_state().needs_save() {
                return Flushed::Failed;
            }
        }
        // Its state was just saved with History: nothing new to save.
        self.finish_saved_gesture();
        Flushed::Saved
    }
    fn background_saved(&mut self, completion: super::autosave::Completion) {
        let saved = completion.into_result();
        let result = saved.as_ref().map(|_| ()).map_err(Clone::clone);
        if !self.document.edit.save_state_mut().finished(result) {
            return;
        }
        match saved {
            Ok(p) => self.saved_to(&p),
            Err(e) => self.status = format!("Edits not saved: {e}"),
        }
    }
    fn saved_to(&mut self, location: &CatalogLocation) {
        let CatalogLocation::File(path) = location;
        self.status = format!(
            "Saved {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    pub(super) fn effective_recipe(&self) -> Recipe {
        let mut r = if self.view.compare.before_only() {
            self.before_settings()
        } else {
            self.presets
                .preview
                .as_ref()
                .unwrap_or(self.document.edit.recipe())
                .clone()
        };
        if self.view.is(super::state::Tool::Crop) {
            // The whole photo, white areas included, around the crop being drawn.
            r.crop = [0., 0., 1., 1.];
            r.constrain_crop = false;
        }
        // Switched-off panels as rendered, so viewport geometry matches the photo shown.
        if !r.panels.all_on() {
            r = r.as_rendered().into_owned();
        }
        r
    }
    /// The swatch Point Color's Visualize Range shows, while its tab is open on a color
    /// photo in Develop.
    /// Not while an eyedropper is out, which samples the photo as it renders, nor in
    /// Before, which shows the photo's defaults.
    pub(super) fn visualized_swatch(&self) -> Option<usize> {
        let pc = &self.view.point_color;
        let shown = pc.visualize
            && self.point_color_tab_shown()
            && !self.view.picks_color()
            && !self.view.compare.before_only();
        pc.selected
            .filter(|i| shown && *i < self.document.edit.recipe().point_colors.len())
    }
    /// What the active tool draws into the rendered preview.
    pub(super) fn overlay(&self) -> super::worker::Overlay {
        use super::{state::Tool, worker::Overlay};
        match self.view.tool {
            Tool::Remove if self.view.retouch.visualize => {
                Overlay::Spots(self.view.retouch.threshold)
            }
            Tool::Mask if self.view.masking.overlay => match self.view.masking.selected {
                Some(index) if index < self.document.edit.recipe().masks.len() => Overlay::Mask {
                    index,
                    color: [230, 40, 40],
                    opacity: 0.5,
                },
                _ => Overlay::None,
            },
            _ => Overlay::None,
        }
    }
    /// The 1:1 region to render when zoomed to 100% or more; below 100% the
    /// whole photo is rendered at the zoomed size instead.
    pub(super) fn region(&self) -> Option<[u32; 4]> {
        let im = self.document.full()?;
        self.region_in(&Geometry::new(im, &self.effective_recipe(), 0))
    }
    /// The 1:1 region of a photo with geometry `g` that the view shows, as `region`.
    pub(super) fn region_in(&self, g: &Geometry) -> Option<[u32; 4]> {
        self.view.zoom.region(self.view.viewport, g.width, g.height)
    }
    pub(super) fn schedule(&mut self) {
        self.schedule_render(false);
    }
    /// Renders an edit. While a slider moves, each change starts a render; one of
    /// the same view already running finishes and shows on the way instead of being
    /// cancelled, or a GPU slower than the changes would show nothing until the
    /// slider stops.
    pub(super) fn schedule_edit(&mut self) {
        self.schedule_render(true);
    }
    fn schedule_render(&mut self, edit: bool) {
        let image = self.document.full().cloned();
        if let Some(image) = image {
            self.yield_before();
            let region = self.region();
            self.preview.last_region = region;
            let geometry = Geometry::new(&image, &self.effective_recipe(), 0);
            let RenderEdges { fit, max_edge } = self.render_edges(&geometry);
            self.preview.last_fit_edge = fit;
            // Visualize Range renders the selected swatch's selection instead of its
            // adjustment; never as the photo's thumbnail.
            let mut recipe = self.effective_recipe();
            let visualize = self
                .visualized_swatch()
                .and_then(|i| crate::model::point_color::visualize_range(&recipe.point_colors, i));
            let thumbnail = region.is_none() && self.shows_library_edit() && visualize.is_none();
            if let Some(list) = visualize {
                recipe.point_colors = list;
            }
            let view = super::state::RenderView {
                image: Arc::downgrade(&image),
                path: self.document.path.clone(),
                photo: self.document.catalog_photo,
                region,
                max_edge,
            };
            let same_view = self
                .preview
                .last_view
                .as_ref()
                .is_some_and(|v| v.same(&view));
            self.preview.last_view = Some(view);
            let (id, cancel) = self.preview.start(
                edit && same_view,
                super::state::Pending {
                    recipe: Some(recipe.clone()),
                    mode: region.map_or(
                        super::state::TextureMode::Whole,
                        super::state::TextureMode::Region,
                    ),
                    crop: geometry.crop(),
                },
            );
            self.renderer.submit(RenderJob {
                pane: super::worker::Pane::After,
                max_edge,
                cancel,
                id,
                image,
                recipe,
                region,
                monitor: self.view.monitor.clone(),
                clipping: self.view.clipping.overlay(),
                navigator: !self.view.zoom.on || self.preview.navigator.is_none(),
                thumbnail,
                samples: self.wants_samples(),
                overlay: self.overlay(),
                drawn: self.preview.presented(),
            });
        }
        self.schedule_before();
    }
    /// The long edge a Fit render of a photo with geometry `g` needs in the view,
    /// and the one to render at the current zoom.
    pub(super) fn render_edges(&self, g: &Geometry) -> RenderEdges {
        render_edges(&self.view.zoom, self.view.viewport, g)
    }
    /// A CPU render: the whole photo, or a 100% region drawn over it.
    pub(super) fn set_pixels(
        &mut self,
        ctx: &egui::Context,
        region: bool,
        [w, h]: [u32; 2],
        data: &[u8],
        navigator: Option<image::RgbImage>,
    ) {
        let image = egui::ColorImage::from_rgb([w as usize, h as usize], data);
        if region {
            Picture::upload(&mut self.preview.region, ctx, "photo region", image);
            // Zoomed in before the photo was decoded: the region is the live render.
            self.preview.stand_in = None;
            return;
        }
        // Zoomed in, a whole render still fills a Navigator that has none,
        // e.g. after moving on to the next photo at the same zoom.
        if (!self.view.zoom.on || self.preview.navigator.is_none())
            && let Some(small) = navigator
        {
            let small = egui::ColorImage::from_rgb(
                [small.width() as usize, small.height() as usize],
                small.as_raw(),
            );
            Picture::upload(&mut self.preview.navigator, ctx, "navigator", small);
        }
        Picture::upload(&mut self.preview.texture, ctx, "photo", image);
        // A live render replaces the stored preview; the embedded JPEG sets this
        // again once shown.
        self.preview.embedded = false;
        self.preview.stand_in = None;
    }
    /// A GPU render, presented into textures the renderer registered.
    pub(super) fn set_presented(
        &mut self,
        region: bool,
        (id, size): (egui::TextureId, [usize; 2]),
        navigator: Option<(egui::TextureId, [usize; 2])>,
    ) {
        let picture = Some(Picture::presented(id, size));
        if region {
            self.preview.region = picture;
            self.preview.stand_in = None;
            return;
        }
        if (!self.view.zoom.on || self.preview.navigator.is_none())
            && let Some((id, size)) = navigator
        {
            self.preview.navigator = Some(Picture::presented(id, size));
        }
        self.preview.texture = picture;
        self.preview.embedded = false;
        self.preview.stand_in = None;
    }
    pub(super) fn navigate(&mut self, delta: isize) {
        if let (Some(l), Some(id)) = (&self.library, self.document.catalog_photo)
            && let Some(next) = l.navigate(id, delta as i32)
        {
            self.develop_catalog_photo(next);
        }
    }
}

/// The long edges a render of a photo with geometry `g` needs in a view of
/// `viewport` pixels at `zoom`.
pub(super) fn render_edges(
    zoom: &super::navigator::Zoom,
    viewport: Vec2,
    g: &Geometry,
) -> RenderEdges {
    let fit = crate::develop::quality::fit_edge(
        g.width,
        g.height,
        [viewport.x as u32, viewport.y as u32],
    );
    let max_edge = if zoom.on && zoom.level < 1. {
        (g.width.max(g.height) as f32 * zoom.level).round() as u32
    } else {
        fit
    };
    RenderEdges { fit, max_edge }
}
/// The long edges of a render, from `Editor::render_edges`.
pub(super) struct RenderEdges {
    /// The view's Fit size.
    pub(super) fit: u32,
    /// What to render at the current zoom: Fit, or smaller zoomed out.
    pub(super) max_edge: u32,
}
