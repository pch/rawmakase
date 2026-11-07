use crate::app::shell::messages::Topic;
use crate::app::worker::{LoadJob, RenderJob};
use crate::app::{Editor, develop::state::Picture};
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
pub(in crate::app) enum Flushed {
    Saved,
    /// Not saved: the error is on the top bar.
    Failed,
    /// Still being saved when time ran out.
    Late,
}

/// Why the open photo's edit can't be saved: with no identity read when it
/// opened, there is nothing to check its file against.
const UNIDENTIFIED: &str = "the photo's file could not be read when it was opened";

/// What a render is scheduled for, which decides whether the one running is
/// cancelled or let finish and show on the way.
#[derive(Clone, Copy)]
enum Change {
    /// Anything else: a new zoom level, photo or display setting.
    View,
    Edit,
    /// The 100% region panned to another part of the photo.
    Pan,
}

impl Editor {
    pub(in crate::app) fn open(&mut self, path: PathBuf) {
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
            self.messages.warn(
                "Use Library → Import Lightroom catalog to select a new RAWmakase catalog destination",
            );
            self.module = Module::Library;
        }
    }
    pub(in crate::app) fn open_raw(&mut self, path: PathBuf, photo: Option<PhotoId>) {
        if self.load_raw(path, photo) {
            self.module = Module::Develop;
        }
    }
    /// Starts loading a RAW as the document, staying in the module shown:
    /// the Library's Loupe shows it through the same pipeline as Develop.
    /// False when work in progress or an unsaved edit prevents it.
    pub(in crate::app) fn load_raw(&mut self, path: PathBuf, photo: Option<PhotoId>) -> bool {
        if self.activity.is_busy() {
            return false;
        }
        if !self.flush() {
            return false;
        }
        // An offline photo opens from its editable preview, when it has a usable
        // one. One without opens from its file, whose load says it can't be
        // read; one whose editable preview can't be used says why instead.
        let from = match photo {
            Some(id) if !path.is_file() => {
                let chosen = self
                    .library
                    .as_ref()
                    .and_then(|l| l.photo(id).map(|p| (l, p)))
                    .filter(|(_, p)| p.path == path)
                    .map(|(l, p)| l.develop_from(p, false));
                match chosen {
                    Some(Ok(from)) => from,
                    Some(Err(refusal @ crate::app::library::Refusal::EditablePreview(_))) => {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        self.messages
                            .warn(format!("{name} can't be opened: {}", refusal.detail()));
                        return false;
                    }
                    _ => crate::app::worker::OpenedFrom::Original,
                }
            }
            _ => crate::app::worker::OpenedFrom::Original,
        };
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
            .map(|path| crate::app::worker::Prefetch {
                path,
                cancel: self.prefetch_cancel.clone(),
                demosaic: self.session.demosaic,
            });
        // The photo being left is Paste from Previous's source; opening the same photo
        // again (as a new demosaic setting does) leaves Previous as it was.
        let another =
            self.document.catalog_photo != photo || self.document.path.as_ref() != Some(&path);
        if another && let Some(settings) = self.current_settings() {
            self.previous_settings = Some(settings);
        }
        let (id, cancel) = self.document.open(photo);
        // The reference photo stays on screen from photo to photo; Before goes.
        let reference = self
            .reference_view()
            .then(|| std::mem::take(&mut self.preview.before));
        self.preview.clear_document();
        if let Some(reference) = reference {
            self.preview.before = reference;
        }
        self.presets.clear_document();
        self.view.next_document(another);
        self.selection.clear_document();
        if let Some(photo) = photo {
            self.request_stand_ins(id, photo, neighbour);
        }
        // What failed for the photo left says nothing of this one.
        self.messages.settle(Topic::Load);
        self.messages.settle(Topic::Render);
        let text: String = if from.is_final_reduced() {
            "Reading editable preview…"
        } else if crate::storage::is_raw(&path) {
            "Reading RAW…"
        } else {
            "Reading photo…"
        }
        .into();
        self.messages.note(text);
        self.loader.submit(LoadJob {
            id,
            path,
            from,
            cancel,
            prefetch,
            defaults: self.raw_defaults.clone(),
            demosaic: self.session.demosaic,
        });
        // The photo left may be the reference, or its edit may have changed.
        self.load_reference();
        true
    }
    /// The photo to decode ahead of time while `id` is shown: the next one in
    /// the filmstrip, or the previous one after stepping back.
    pub(in crate::app) fn prefetch_neighbour(&self, id: PhotoId) -> Option<PathBuf> {
        let photo = self.library.as_ref()?.photo(self.neighbour_of(id)?)?;
        photo.path.is_file().then(|| photo.path.clone())
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
    /// false, with the error on the top bar, if it could not be saved.
    pub(in crate::app) fn commit_library_drafts(&mut self) -> bool {
        if let Some(library) = &mut self.library
            && let Err(e) = library.commit_drafts()
        {
            self.messages.fail(format!("Not saved: {e}"));
            return false;
        }
        true
    }
    /// Saves the edit now, after any background save in flight; false if
    /// it could not be saved.
    pub(in crate::app) fn flush(&mut self) -> bool {
        // A Sync may be writing the open photo: nothing is saved over it until it is
        // done (see `autosave`).
        if self.activity.holds_catalog_writes() {
            return false;
        }
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
            let saved = self
                .document
                .file
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!(UNIDENTIFIED))
                .and_then(|source| {
                    l.session.save_edit(
                        id,
                        self.document.original(path),
                        source,
                        self.document.edit.recipe(),
                        &self.document.export,
                        crate::catalog::HistoryUpdate::of(&history),
                    )
                })
                .map(|()| l.session.catalog().location().clone());
            match saved {
                Ok(p) => {
                    self.saved_to(&p);
                    self.document.edit.save_state_mut().saved();
                }
                Err(e) => {
                    self.document.edit.save_state_mut().failed(e.to_string());
                    self.messages
                        .fail_on(Topic::Save, format!("Edits not saved: {e}"));
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
    pub(in crate::app) fn autosave(&mut self, ctx: &egui::Context) {
        if let Some(completion) = self.autosave.poll() {
            self.background_saved(completion);
        }
        // A Sync may be writing the open photo (a target, from the Library): what an
        // analysis landing meanwhile changed is not saved over it. The photo opens
        // again once the Sync is done, or saves then if it was not a target.
        if !self.document.edit.save_state().ready()
            || self.document.edit.history().in_gesture()
            || self.autosave.busy()
            || self.activity.holds_catalog_writes()
        {
            return;
        }
        let job = match self.save_job() {
            None => return,
            Some(Ok(job)) => job,
            Some(Err(error)) => return self.unidentified(error),
        };
        match self.autosave.submit(job, ctx) {
            Ok(()) => self.document.edit.save_state_mut().saving(),
            // No saver thread: save here, as before.
            Err(_) => {
                self.flush();
            }
        }
    }
    /// Notes that the open photo's edit can't be saved, as its file could not be
    /// identified when it opened.
    fn unidentified(&mut self, error: &'static str) {
        self.document.edit.save_state_mut().failed(error.into());
        self.messages
            .fail_on(Topic::Save, format!("Edits not saved: {error}"));
    }
    /// The edit as it stands, to save to the catalog photo it belongs to; `None`
    /// when it belongs to none. A file that could not be identified when it
    /// opened gets none: the edit can't be checked against it.
    fn save_job(&self) -> Option<Result<crate::app::develop::autosave::Job, &'static str>> {
        let raw = self.document.path.clone()?;
        let (Some(l), Some(photo)) = (&self.library, self.document.catalog_photo) else {
            return None;
        };
        let Some(source) = self.document.file.clone() else {
            return Some(Err(UNIDENTIFIED));
        };
        Some(Ok(crate::app::develop::autosave::Job {
            catalog: l.session.catalog().location().clone(),
            photo,
            raw,
            source,
            from_editable_preview: self.document.opened_from_preview(),
            recipe: self.document.edit.recipe().clone(),
            export: self.document.export.clone(),
            history: self
                .document
                .edit
                .history()
                .saved(self.document.edit.recipe()),
        }))
    }
    /// Saves the edit as [`flush`](Self::flush) does, but on the autosave thread,
    /// giving up at `until`: a catalog on a stalled network share must not hold up
    /// closing. A save still running then goes on, and the edit stays unsaved.
    pub(in crate::app) fn flush_by(&mut self, until: Instant) -> Flushed {
        // Saved once a Sync, which may be writing the open photo, is done.
        if self.activity.holds_catalog_writes() {
            return Flushed::Late;
        }
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
        let job = if self.document.edit.save_state().needs_save() {
            match self.save_job() {
                Some(Err(error)) => {
                    self.unidentified(error);
                    return Flushed::Failed;
                }
                job => job.and_then(Result::ok),
            }
        } else {
            None
        };
        if let Some(job) = job {
            // A saver lost since the last save is started again by a second
            // submit; saving here instead could stall as long as the catalog does.
            let submitted = self
                .autosave
                .submit(job, &self.context)
                .or_else(|job| self.autosave.submit(*job, &self.context));
            if submitted.is_err() {
                let error = "the autosave thread could not start".to_string();
                self.document.edit.save_state_mut().failed(error.clone());
                self.messages
                    .fail_on(Topic::Save, format!("Edits not saved: {error}"));
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
    fn background_saved(&mut self, completion: crate::app::develop::autosave::Completion) {
        let saved = completion.into_result();
        let result = saved.as_ref().map(|_| ()).map_err(Clone::clone);
        if !self.document.edit.save_state_mut().finished(result) {
            return;
        }
        match saved {
            Ok(p) => self.saved_to(&p),
            Err(e) => self
                .messages
                .fail_on(Topic::Save, format!("Edits not saved: {e}")),
        }
    }
    fn saved_to(&mut self, location: &CatalogLocation) {
        self.messages.settle(Topic::Save);
        let CatalogLocation::File(path) = location;
        self.messages.note(format!(
            "Saved {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    pub(in crate::app) fn effective_recipe(&self) -> Recipe {
        let mut r = if self.view.compare.renders_before_alone() {
            self.before_settings()
        } else {
            self.presets
                .preview
                .as_ref()
                .unwrap_or(self.document.edit.recipe())
                .clone()
        };
        if self.view.is(crate::app::develop::state::Tool::Crop) {
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
    pub(in crate::app) fn visualized_swatch(&self) -> Option<usize> {
        let pc = &self.view.point_color;
        let shown = pc.visualize
            && self.point_color_tab_shown()
            && !self.view.picks_color()
            && !self.view.compare.before_only();
        pc.selected
            .filter(|i| shown && *i < self.document.edit.recipe().point_colors.len())
    }
    /// What the active tool draws into the rendered preview.
    pub(in crate::app) fn overlay(&self) -> crate::app::worker::Overlay {
        use crate::app::{develop::state::Tool, worker::Overlay};
        match self.view.tool {
            Tool::Remove if self.view.retouch.visualize => {
                Overlay::Spots(self.view.retouch.threshold)
            }
            Tool::Mask => match self.view.masking.hovered().or(self
                .view
                .masking
                .selected
                .filter(|_| self.view.masking.overlay))
            {
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
    /// The 1:1 region to render when zoomed to 100% or more, with a margin
    /// around the view; below 100% the whole photo is rendered at the zoomed
    /// size instead. See
    /// [`Zoom::render_region`](crate::app::view::navigator::Zoom::render_region).
    pub(in crate::app) fn region(&self) -> Option<[u32; 4]> {
        let im = self.document.full()?;
        self.region_in(&Geometry::new(im, &self.effective_recipe(), 0))
    }
    /// The 1:1 region of a photo with geometry `g` that the view shows, as `region`.
    pub(in crate::app) fn region_in(&self, g: &Geometry) -> Option<[u32; 4]> {
        self.view.zoom.render_region(
            self.view.viewport,
            g.width,
            g.height,
            self.preview.last_region,
        )
    }
    pub(in crate::app) fn schedule(&mut self) {
        self.schedule_render(Change::View);
    }
    /// Renders an edit. While a slider moves, each change starts a render; one of
    /// the same view already running finishes and shows on the way instead of being
    /// cancelled, or a GPU slower than the changes would show nothing until the
    /// slider stops.
    pub(in crate::app) fn schedule_edit(&mut self) {
        self.schedule_render(Change::Edit);
    }
    /// Renders the 100% region the view was panned to. As for an edit, a region
    /// of the same edit already running finishes and shows where it was, or a
    /// drag (on the minimap, where the photo moves fast) on a GPU slower than the
    /// frames would show no sharp pixels until it stops.
    pub(in crate::app) fn schedule_pan(&mut self) {
        self.schedule_render(Change::Pan);
    }
    fn schedule_render(&mut self, change: Change) {
        let image = self.document.full().cloned();
        if let Some(image) = image {
            self.yield_before();
            self.yield_magnifier();
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
            let view = crate::app::develop::state::RenderView {
                image: Arc::downgrade(&image),
                path: self.document.path.clone(),
                photo: self.document.catalog_photo,
                region,
                max_edge,
            };
            let last = self.preview.last_view.as_ref();
            let let_finish = match change {
                Change::View => false,
                Change::Edit => last.is_some_and(|v| v.same(&view)),
                Change::Pan => {
                    last.is_some_and(|v| v.panned(&view))
                        && self.preview.pending.recipe.as_ref() == Some(&recipe)
                }
            };
            self.preview.last_view = Some(view);
            let ai = self.shown_ai_source(&image, &recipe.ai_denoise);
            let rendered_ai = match &ai {
                None => crate::app::develop::state::RenderedAi::None,
                Some(_) if self.document.ai.is_complete() => {
                    crate::app::develop::state::RenderedAi::Complete
                }
                Some(_) => crate::app::develop::state::RenderedAi::Partial,
            };
            let (id, cancel) = self.preview.start(
                let_finish,
                crate::app::develop::state::Pending {
                    recipe: Some(recipe.clone()),
                    mode: region.map_or(
                        crate::app::develop::state::TextureMode::Whole,
                        crate::app::develop::state::TextureMode::Region,
                    ),
                    crop: geometry.crop(),
                    ai: rendered_ai,
                },
            );
            self.request_ai_denoise(&image, &geometry, region);
            let replaced = self.renderer.submit(RenderJob {
                pane: crate::app::worker::Pane::After,
                max_edge,
                cancel,
                id,
                image,
                recipe,
                region,
                monitor: self.view.display_profile(),
                clipping: self.view.clipping.overlay(),
                navigator: !self.view.zoom.on || self.preview.navigator.is_none(),
                thumbnail,
                samples: self.wants_samples(),
                overlay: self.overlay(),
                ai,
                drawn: self.preview.presented(),
            });
            if let Some(replaced) = replaced {
                self.preview.replaced(replaced);
            }
        }
        self.schedule_before();
    }
    /// The long edge a Fit render of a photo with geometry `g` needs in the view,
    /// and the one to render at the current zoom.
    pub(in crate::app) fn render_edges(&self, g: &Geometry) -> RenderEdges {
        render_edges(&self.view.zoom, self.view.viewport, g)
    }
    /// A CPU render: the whole photo, or a 100% region drawn over it.
    pub(in crate::app) fn set_pixels(
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
        // Zoomed in, a whole render still fills a minimap that has none,
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
    pub(in crate::app) fn set_presented(
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
    pub(in crate::app) fn navigate(&mut self, delta: isize) {
        if let Some(id) = self.document.catalog_photo
            && let Some(next) = self.develop_step(id, delta as i32)
        {
            self.develop_catalog_photo(next);
        }
    }
    /// The photo `step` photos from `id` in the filmstrip that Develop can
    /// open, passing over the ones it can't (offline without a usable editable
    /// preview, HDR, animated, CMYK):
    /// stopping on them would leave every photo beyond out of reach of the
    /// arrow keys. With none in that direction, the first one, so opening it
    /// says why it can't be.
    pub(in crate::app) fn develop_step(&self, id: PhotoId, step: i32) -> Option<PhotoId> {
        let library = self.library.as_ref()?;
        let first = library.navigate(id, step)?;
        let mut at = first;
        loop {
            let photo = library.photo(at)?;
            // An offline photo without an editable preview recorded is passed
            // over by its file alone, as before; only the others are asked.
            let online = photo.path.is_file();
            if at == id
                || ((online || library.has_editable_preview(at))
                    && library.develop_from(photo, online).is_ok())
            {
                return Some(at);
            }
            match library.navigate(at, step) {
                Some(next) if next != at => at = next,
                _ => return Some(first),
            }
        }
    }
}

/// The long edges a render of a photo with geometry `g` needs in a view of
/// `viewport` pixels at `zoom`.
pub(in crate::app) fn render_edges(
    zoom: &crate::app::view::navigator::Zoom,
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
pub(in crate::app) struct RenderEdges {
    /// The view's Fit size.
    pub(in crate::app) fit: u32,
    /// What to render at the current zoom: Fit, or smaller zoomed out.
    pub(in crate::app) max_edge: u32,
}

#[cfg(test)]
#[path = "workflow_editor_tests.rs"]
mod editor_tests;

#[cfg(test)]
#[path = "editable_preview_editor_tests.rs"]
mod editable_preview_tests;
