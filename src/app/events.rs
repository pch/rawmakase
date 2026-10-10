//! Accept worker results at a single generation-checked boundary.
use super::{
    Editor,
    worker::{self, Event, LoadedHeader, Pane, RenderStage, TaskKind},
};
use crate::app::Module;
use crate::export_settings::ExportOptions;
use eframe::egui;

impl Editor {
    pub(super) fn events(&mut self, ctx: &egui::Context) {
        self.import_progress();
        // A Point Color sample stops as soon as its tab is no longer where the photo is
        // edited (the Library, Before, another tab), whichever way that happened.
        if self.document.point_color_pick.is_running() && !self.point_color_tab_shown() {
            self.document.point_color_pick.invalidate();
        }
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::CatalogWorking(message) => {
                    self.status = message.clone();
                    self.catalog_work = Some(message);
                }
                Event::CatalogReady(result) => {
                    self.catalog_work = None;
                    self.catalog_ready(result);
                }
                // The dialog's own DialogClosed follows.
                Event::FolderQuestion(question) => {
                    self.modal = Some(super::Modal::FolderQuestion(*question))
                }

                Event::Monitor(p) => {
                    self.activity.finish_dialog();
                    self.view.monitor = Some(p);
                    let _ = self.save_session();
                    self.schedule();
                }
                Event::PresetLoad(p) => {
                    self.activity.finish_dialog();
                    match crate::presets::load_preset(&p) {
                        Ok(r) => {
                            let r = crate::presets::applied_to(r, self.document.edit.recipe());
                            let old = self.document.edit.replace(r);
                            // Before the step is taken, as the Presets panel does.
                            self.ensure_upright();
                            self.commit_edit(old.clone(), None);
                        }
                        Err(e) => self.status = e.to_string(),
                    }
                }
                Event::Auto { id, kind, result } if id == self.load.id() => {
                    self.auto_ready(kind, result)
                }
                Event::PointColorSample {
                    id,
                    generation,
                    sampled,
                    result,
                } if id == self.load.id()
                    && self.document.point_color_pick.is_running()
                    && self.document.point_color_pick.id() == generation =>
                {
                    self.point_color_sample_ready(&sampled, result)
                }
                Event::TargetedSample {
                    id,
                    generation,
                    sampled,
                    result,
                } if id == self.load.id()
                    && self.document.targeted_pick.is_running()
                    && self.document.targeted_pick.id() == generation =>
                {
                    self.targeted_sample_ready(&sampled, result)
                }
                Event::Upright {
                    id,
                    generation,
                    analysed,
                    result,
                } if id == self.load.id() => self.upright_ready(generation, &analysed, result),
                Event::Straighten {
                    id,
                    generation,
                    analysed,
                    result,
                } if id == self.load.id() => {
                    self.auto_straighten_ready(generation, &analysed, result)
                }
                Event::Selection(done) => self.selection_done(*done),
                Event::ModelInstalled(result) => {
                    self.selection.models.finished(&result);
                    if result.as_ref().err().is_some_and(|e| e == "Cancelled") {
                        self.status = "Model download cancelled".into();
                    } else {
                        self.model_installed(result);
                    }
                }
                Event::ModelRemoved(result) => {
                    self.selection.models.removed(result.is_ok());
                    self.status = match result {
                        Ok(()) => "Selection model removed".into(),
                        Err(e) => format!("Model not removed: {e}"),
                    };
                }
                Event::CatalogUpgraded { generation, result } => {
                    self.catalog_upgraded(generation, result)
                }
                Event::XmpLibrary { scan, library } => self.presets_scanned(scan, library),
                Event::PresetScanFailed { scan, error } => self.preset_scan_failed(scan, error),
                Event::Profiles {
                    id,
                    profiles,
                    errors,
                } if id == self.load.id() => {
                    self.document.profiles = profiles;
                    self.document.profile_errors = errors;
                    self.refresh_photo_defaults();
                    self.refresh_preset_support();
                    if let Some(text) = self.document.pending_lightroom.take() {
                        self.apply_lightroom_edits(&text);
                        // The Lightroom edit is the starting point, not an unsaved change.
                        self.document.edit.save_state_mut().saved();
                    }
                }
                Event::Import(kind, paths) => {
                    self.activity.finish_dialog();
                    self.import(kind, paths, ctx);
                }
                Event::OnboardingScanned { generation, found } => {
                    self.onboarding_scanned(generation, *found)
                }
                Event::Imported(summary) => self.imported(*summary, ctx),
                Event::Synced(result) => self.synced(*result),
                Event::PresetSave(p) => {
                    self.activity.finish_dialog();
                    match crate::presets::save_preset(&p, self.document.edit.recipe()) {
                        Ok(()) => self.status = "Preset saved".into(),
                        Err(e) => self.status = e.to_string(),
                    }
                }
                Event::Header(header) if header.id == self.load.id() => self.header_ready(*header),
                Event::StandIn(stand_in) => self.stand_in_ready(ctx, *stand_in),
                Event::Embedded { id, image: im } if id == self.load.id() => {
                    // The stored preview, which shows the edit, stays until the
                    // first render.
                    if self.preview.stand_in.is_some() {
                        continue;
                    }
                    // The camera JPEG is uncropped and unedited; when the Library
                    // already has the edited thumbnail, keep showing that until
                    // the first render instead of flashing the original.
                    let edited = self
                        .document
                        .catalog_photo
                        .zip(self.library.as_ref())
                        .is_some_and(|(id, l)| l.has_edited_thumbnail(id));
                    if edited {
                        continue;
                    }
                    let k = (360. / im.width().max(im.height()) as f32).min(1.);
                    let navigator = image::imageops::thumbnail(
                        &im,
                        ((im.width() as f32 * k) as u32).max(1),
                        ((im.height() as f32 * k) as u32).max(1),
                    );
                    self.set_pixels(
                        ctx,
                        false,
                        [im.width(), im.height()],
                        im.as_raw(),
                        Some(navigator),
                    );
                    self.preview.embedded = true;
                    self.preview.mode = super::state::TextureMode::Whole;
                    // Uncropped, so it is placed by the photo's crop once that is known.
                    self.preview.crop = Some([0., 0., 1., 1.]);
                    self.preview.status = "Camera preview • developing RAW…".into();
                }
                Event::Ready { id, full, status } if id == self.load.id() => {
                    self.document.set_image(full);
                    self.load.finish(id);
                    // An Upright mode chosen before the photo decoded still needs analysing.
                    self.ensure_upright();
                    if !self.document.edit.save_state().is_protected() {
                        // A raw default that could not be used stays explained.
                        self.status = match self.defaults_note() {
                            Some(note) => format!("{status} · {note}"),
                            None => status,
                        };
                    }
                    self.schedule();
                }
                Event::Reference { ticket, result } => self.reference_ready(ticket, result),
                Event::Rendered {
                    id,
                    pane: Pane::Before,
                    preview,
                    stage,
                    ..
                } if id == self.preview.before.task.id() => {
                    self.before_rendered(ctx, preview, stage, id);
                }
                Event::Rendered {
                    id,
                    pane: Pane::After,
                    preview,
                    histogram,
                    thumbnail,
                    samples,
                    stage,
                    status,
                } => {
                    let Some(request) = self.preview.request(id) else {
                        continue;
                    };
                    self.preview.showing(id);
                    let region = matches!(request.mode, super::state::TextureMode::Region(_));
                    // A region's own histogram would describe only what is
                    // visible; the whole photo's follows as `Histogram`.
                    if !region {
                        self.preview.histogram = *histogram;
                    }
                    match preview {
                        worker::Preview::Pixels {
                            image,
                            display_rgb,
                            navigator,
                        } => self.set_pixels(
                            ctx,
                            region,
                            [image.width, image.height],
                            &display_rgb,
                            navigator,
                        ),
                        worker::Preview::Texture {
                            id,
                            size,
                            navigator,
                        } => self.set_presented(region, (id, size), navigator),
                    }
                    // Pixels asked for by a hover or loupe that has since ended are
                    // not kept.
                    self.preview.keep_samples(region, samples);
                    self.preview.samples_recipe = request.recipe;
                    self.preview.mode = request.mode;
                    if !region {
                        self.preview.crop = Some(request.crop);
                    }
                    if stage != RenderStage::Draft {
                        self.preview.task.finish(id);
                        if let Some(small) = thumbnail {
                            self.refresh_library_thumbnail(ctx, small);
                        }
                    }
                    self.preview.status = status;
                }
                Event::Histogram { id, histogram } if self.preview.request(id).is_some() => {
                    self.preview.histogram = *histogram;
                }
                Event::Failed {
                    id,
                    task: TaskKind::Load,
                    error,
                } if id == self.load.id() => {
                    self.status = error;
                    self.load.finish(id);
                    // No render is coming to take over from the stored preview.
                    self.preview.stand_in = None;
                }
                Event::Failed {
                    id,
                    task: TaskKind::Render(Pane::After),
                    error,
                } if id == self.preview.task.id() => {
                    self.status = error;
                    self.preview.task.finish(id);
                    self.preview.stand_in = None;
                    // A failed render on the GPU retires Before's textures too: render
                    // Before again, not the edit that failed.
                    let before = &mut self.preview.before;
                    if before.texture.is_none() && before.region.is_none() {
                        before.forget_job();
                        self.schedule_before();
                    }
                }
                Event::Failed {
                    id,
                    task: TaskKind::Render(Pane::Before),
                    error,
                } if id == self.preview.before.task.id() => {
                    self.status = format!("Before: {error}");
                    self.preview.before.task.finish(id);
                    // A failed render on the GPU retires the edit's textures too: render
                    // the edit again. Before keeps its failed job, so it is not retried.
                    if self.preview.texture.is_none() && self.preview.region.is_none() {
                        self.schedule();
                    }
                }
                Event::RendererReset(retired) => {
                    self.preview.forget_presented();
                    drop(retired);
                }
                Event::DialogClosed => {
                    self.activity.finish_dialog();
                }
                Event::BatchExported { ticket, outcomes } => self.batch_exported(ticket, outcomes),
                _ => {}
            }
        }
    }
    fn catalog_ready(&mut self, result: Result<Box<super::library::Library>, String>) {
        self.activity.finish_dialog();
        // Folder locations may have changed with it.
        self.save_computer_name();
        self.preferences.locations = None;
        match result {
            Ok(mut l) => {
                self.onboarding.catalog_error = None;
                self.load.invalidate();
                // The photo being left is Previous, as when moving between photos.
                if let Some(settings) = self.current_settings() {
                    self.previous_settings = Some(settings);
                }
                self.document.reset(None);
                self.preview.clear_document();
                self.presets.clear_document();
                self.view.clear_document();
                self.selection.clear_catalog();
                self.status = if !l.message.is_empty() {
                    l.message.clone()
                } else if l.session.photos.is_empty() {
                    "Catalog ready. Add a folder of photos to begin.".into()
                } else {
                    "Catalog ready. Offline photos remain in the library; locate their folders to develop them.".into()
                };
                // Commands never cross catalogs; reloading this one (after
                // adding or relinking a folder) keeps them.
                let reloaded = self.library.as_ref().is_some_and(|old| {
                    old.session.catalog.location() == l.session.catalog.location()
                });
                if !reloaded {
                    // A folder chosen for removal belongs to the catalog left.
                    if matches!(self.modal, Some(super::Modal::RemoveFolder(_))) {
                        self.modal = None;
                    }
                    self.undo_log.clear();
                    // Photo ids belong to their catalog, and so does the reference.
                    self.clear_reference();
                }
                l.set_defaults(self.raw_defaults.clone());
                self.library = Some(l);
                self.module = Module::Library;
                // On launch, return to the folder, photo and module of last time.
                let restore = self.restore.take();
                if let Some(library) = &mut self.library {
                    if let Some(place) = &restore {
                        library.restore_source(&place.source, place.photo);
                    }
                    // The Library as it was shown, on launch and when this
                    // catalog is loaded again (a folder added or relinked);
                    // another catalog starts with every photo shown.
                    if restore.is_some() || reloaded {
                        library.apply_layout(&self.saved_layout);
                    }
                }
                // Develop reopens on its photo, even one the filters now hide.
                if let Some(place) = restore
                    && place.module == super::library::Module::Develop
                    && let Some(id) = place.photo.or_else(|| self.library.as_ref()?.selected())
                {
                    self.develop_catalog_photo(id);
                    // On launch, a photo gone offline leaves the Library shown
                    // without a dialog; the status bar says why.
                    self.not_editable = None;
                }
                self.open_pending_photo();
                let _ = self.save_session();
            }
            Err(e) => {
                self.pending_photo = None;
                self.onboarding.catalog_error = Some(e.clone());
                self.status = format!("Catalog operation failed: {e}");
            }
        }
    }

    fn header_ready(&mut self, header: LoadedHeader) {
        let LoadedHeader {
            path: p,
            metadata: m,
            recipe: r,
            export: ex,
            status,
            ..
        } = header;
        self.document.file = crate::storage::Identity::read(&p).ok();
        self.document.metadata = Some(m);
        self.document.edit.replace(r);
        self.document.export = ex;
        self.document.edit.save_state_mut().saved();
        self.status = status;
        if let (Some(l), Some(photo)) = (&self.library, self.document.catalog_photo) {
            self.document.lightroom_history = l
                .session
                .catalog
                .lightroom_history(photo)
                .unwrap_or_default();
            self.document.snapshots.list = l.session.catalog.snapshots(photo).unwrap_or_default();
            // The edit as the catalog stores it, read once: the Lightroom settings
            // applied below are the ones read with it.
            let record = l.session.catalog.edit_record(photo);
            let saved = record
                .as_ref()
                .map_err(|e| anyhow::anyhow!("{e:#}"))
                .and_then(|r| r.saved(&p));
            match saved {
                Ok(Some(saved)) => {
                    self.document.origin = super::state::EditOrigin::Saved;
                    self.document.edit.replace(saved.recipe);
                    self.document.export = saved.export;
                    // A History that cannot be read leaves the edit as it is.
                    if let Ok(Some(history)) = l.session.catalog.load_history(photo) {
                        self.document.edit.restore_history(history);
                    }
                    self.document.edit.save_state_mut().saved();
                    self.document.lightroom_notice.clear();
                    // Masks made from a selection are read from the catalog now. One
                    // that cannot be leaves the edit and its History as they are, but
                    // protected: saving it would lose the mask for good.
                    let refs: Vec<(String, u32, u32)> =
                        crate::model::masks::bitmap_refs(&self.document.edit.recipe().masks)
                            .map(|(id, w, h)| (id.to_owned(), w, h))
                            .collect();
                    if let Err(e) = crate::storage::mask_assets::ensure_shaped(
                        refs.iter().map(|(id, w, h)| (id.as_str(), *w, *h)),
                    ) {
                        self.document.edit.save_state_mut().protect(e.to_string());
                        self.document.lightroom_notice = e.to_string();
                    }
                }
                Ok(None) => {
                    self.document.export = ExportOptions::default();
                    self.document.edit.save_state_mut().saved();
                    self.document.lightroom_notice.clear();
                    // No RAWmakase edit yet: start from the Lightroom edit, as
                    // Lightroom shows it, once camera profiles are known.
                    self.document.pending_lightroom =
                        record.ok().and_then(|r| r.lightroom().map(str::to_owned));
                    if self.document.pending_lightroom.is_some() {
                        self.document.origin = super::state::EditOrigin::Lightroom;
                    }
                }
                Err(e) => {
                    // Unreadable is not unedited: it must not follow the defaults.
                    self.document.origin = super::state::EditOrigin::Saved;
                    self.document.edit.save_state_mut().protect(e.to_string());
                    self.document.lightroom_notice = e.to_string();
                }
            }
        }
        self.document.path = Some(p);
        let _ = self.save_session();
    }
}
impl Editor {
    /// After a finished whole-photo render of the current edit, show it as
    /// the photo's Library and filmstrip thumbnail.
    /// Whether the viewport shows the catalog photo's edit as the library would.
    pub(super) fn shows_library_edit(&self) -> bool {
        self.library.is_some()
            && self.document.catalog_photo.is_some()
            && self.document.path.is_some()
            && !self.view.zoom.on
            && !self.view.compare.shows_before()
            && !self.view.is(super::state::Tool::Crop)
            && self.presets.preview.is_none()
    }
    fn refresh_library_thumbnail(&mut self, ctx: &egui::Context, small: image::RgbImage) {
        if self.preview.mode != super::state::TextureMode::Whole || !self.shows_library_edit() {
            return;
        }
        let Ok(json) = serde_json::to_string(self.document.edit.recipe()) else {
            return;
        };
        let (Some(library), Some(id)) = (&mut self.library, self.document.catalog_photo) else {
            return;
        };
        library.update_edited(ctx, id, small, json);
    }
}
