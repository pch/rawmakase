//! Export commands. The dialog lives in `dialog`; the export itself (planning,
//! rendering, file writing) in `crate::export`. Every export, of one photo or the
//! whole selection, goes through the app's one export queue and runs while you keep
//! editing; the top bar shows its progress, and what could not be exported stays
//! there until it is dismissed.
mod dialog;
mod watermark_editor;

use crate::app::export::naming::editor::{self, NameEditor};
use crate::app::export::naming::read::{self as name_read, NameRead, Reads, Status, ToRead};
use crate::app::ui::design::{self, Tone, inset, radius, space, text};

use crate::app::Module;
use crate::catalog::PhotoId;

use crate::app::ui::theme;
use crate::app::ui::widgets::plural;
use crate::app::{Editor, worker::Event};
use crate::export::{
    assemble::Values,
    batch::{self, BatchPhoto, Edit, Outcome, Unplanned},
    job::Photo,
    queue::Queue,
};
use crate::export_settings::{Existing, ExportSettings, NameFields, NameTemplate};
use eframe::egui::{self, Color32, Sense, Vec2};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

/// One photo an Export acts on.
#[derive(Clone, Debug)]
struct Chosen {
    /// Its catalog id; `None` for a photo opened without a catalog.
    id: Option<PhotoId>,
    source: PathBuf,
    name: String,
    /// The photo open in Develop: exported with its edit as shown.
    open: bool,
    /// What its exported name is made from: the catalog's, read when Export
    /// was chosen, and the file's once read (`naming::read`).
    fields: NameFields,
    /// Its place among the photos chosen, those left out counted.
    place: usize,
}

/// The photos an Export acts on, fixed when it is chosen.
#[derive(Clone, Debug, Default)]
struct Scope {
    photos: Vec<Chosen>,
    /// How many photos were chosen, those left out counted.
    chosen: usize,
    /// Photos chosen that can't be exported: their names and why.
    left_out: Vec<(String, String)>,
}
impl Scope {
    /// Its photo has no catalog: what its name needs is all in its file.
    fn standalone(&self) -> bool {
        self.photos.iter().any(|p| p.id.is_none())
    }
    /// Some photo's title, caption, copyright or creator is its file's.
    fn lacks_descriptive(&self) -> bool {
        self.photos.iter().any(|p| p.fields.lacks_descriptive())
    }
}

/// A batch whose files already exist, waiting for the answer to Ask.
struct Pending {
    batch: Vec<BatchPhoto>,
    settings: ExportSettings,
    watermark: Option<crate::watermark::Watermark>,
    left_out: Vec<(String, String)>,
    conflicts: Vec<PathBuf>,
}

/// What a queued batch's outcomes are reported against.
struct Queued {
    names: Vec<String>,
    left_out: Vec<(String, String)>,
    /// What happens once it is written.
    after: crate::export_settings::AfterExport,
}

/// What finished exports could not do, kept until it is dismissed.
#[derive(Clone, Debug, Default)]
pub(in crate::app) struct Summary {
    exported: usize,
    total: usize,
    /// Photos exported with something to say.
    noted: usize,
    /// Photos not exported, with why.
    problems: Vec<(String, String)>,
    /// What exported photos had to say, each note on its own.
    notes: Vec<(String, String)>,
}
impl Summary {
    /// "Exported 148 of 150 · 2 not exported · 1 with notes".
    fn line(&self) -> String {
        let mut line = format!("Exported {} of {}", self.exported, self.total);
        if !self.problems.is_empty() {
            line.push_str(&format!(" · {} not exported", self.problems.len()));
        }
        if self.noted > 0 {
            line.push_str(&format!(" · {} with notes", self.noted));
        }
        line
    }
    /// What it has to say photo by photo, one per line.
    fn details(&self) -> String {
        let mut lines = Vec::new();
        for (title, rows) in [
            ("Not exported", &self.problems),
            ("Exported, with notes", &self.notes),
        ] {
            if !rows.is_empty() {
                lines.push(title.to_string());
                lines.extend(
                    rows.iter()
                        .map(|(name, reason)| format!("{name}: {reason}")),
                );
            }
        }
        lines.join(
            "
",
        )
    }
}

#[derive(Default)]
pub(in crate::app) struct Exports {
    /// The dialog is open, editing `draft` for `scope`.
    dialog: bool,
    draft: ExportSettings,
    scope: Scope,
    /// The folder chooser's answer, from its own thread.
    picked: Arc<Mutex<Option<PathBuf>>>,
    picking: Arc<AtomicBool>,
    /// The Ask question for a batch's existing files.
    pending: Option<Pending>,
    queue: Option<Queue>,
    queued: HashMap<u64, Queued>,
    /// The Watermark Editor, over the dialog.
    watermark_editor: Option<watermark_editor::WatermarkEditor>,
    /// Saved watermarks, read when the dialog opens and after the editor
    /// saves one.
    watermarks: Vec<crate::watermark::Watermark>,
    /// What the scope's names need from its files, read in the background.
    read: NameRead,
    /// Export with Previous's settings, waiting for `read`.
    waiting: Option<ExportSettings>,
    /// The Filename Template Editor, over the dialog.
    name_editor: Option<NameEditor>,
    /// The saved file-name templates, read when the dialog opens.
    saved: crate::app::export::naming::presets::Library,
    /// Tests: an export doesn't save its settings as the last export's.
    unsaved_settings: bool,
}

impl Exports {
    /// Stops the export queue at exit, cancelling the running batch; its thread,
    /// to wait for.
    pub(in crate::app) fn close(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.queue.as_mut().and_then(Queue::close)
    }
}

impl Editor {
    /// Export…: the dialog for the photos chosen now, starting from the last
    /// export's choices.
    pub(in crate::app) fn open_export_dialog(&mut self) {
        let Some(scope) = self.export_scope() else {
            return;
        };
        self.exports.read.cancel();
        self.exports.scope = scope;
        self.exports.draft = ExportSettings::load().unwrap_or_default();
        self.exports.watermarks = crate::watermark::presets();
        self.exports.saved = crate::app::export::naming::presets::Library::load();
        self.exports.dialog = true;
    }
    /// Export with Previous: the last export's choices, without the dialog.
    pub(in crate::app) fn export_with_previous(&mut self) {
        self.export_previous(ExportSettings::load());
    }
    /// Export with Previous with the last export's settings, if any.
    fn export_previous(&mut self, previous: Option<ExportSettings>) {
        match previous {
            Some(settings) => {
                if let Some(scope) = self.export_scope() {
                    self.exports.read.cancel();
                    self.exports.scope = scope;
                    // Names that need the files wait for them, in a small
                    // window that can cancel.
                    if self.names_wait(&settings.name_template()) {
                        self.exports.waiting = Some(settings);
                    } else {
                        self.export(settings);
                    }
                }
            }
            None => self.open_export_dialog(),
        }
    }
    /// The Export dialog or its existing-file question is showing.
    pub(in crate::app) fn export_modal(&self) -> bool {
        self.exports.dialog
            || self.exports.pending.is_some()
            || self.exports.watermark_editor.is_some()
            || self.exports.name_editor.is_some()
            || self.exports.waiting.is_some()
    }
    /// An export is running or waiting.
    pub(in crate::app) fn exporting(&self) -> bool {
        self.exports.queue.as_ref().is_some_and(Queue::busy)
    }

    /// The photos Export acts on: the Library's selection, or in Develop the
    /// Filmstrip's when the open photo is part of it, else the open photo. Says why
    /// and returns `None` when there is nothing to export, or the open photo is
    /// among them and its edit is not final yet.
    fn export_scope(&mut self) -> Option<Scope> {
        let open = self.document.catalog_photo;
        let ids = match &self.library {
            Some(l) if self.module == Module::Library => l.selected_photos(),
            Some(l) => match open {
                Some(open) if l.is_selected(open) => l.selected_photos(),
                Some(open) => vec![open],
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let shown = self.document.path.is_some() && self.document.full().is_some();
        let mut scope = Scope::default();
        match &self.library {
            Some(l) if !ids.is_empty() => {
                for id in ids {
                    let Some(photo) = l.photo(id) else {
                        continue;
                    };
                    let place = scope.chosen;
                    scope.chosen += 1;
                    let name = format!(
                        "{}{}",
                        photo.filename,
                        crate::app::library::copy_suffix(photo)
                    );
                    let open = open == Some(id) && shown;
                    // The open photo too: it is decoded again from its file.
                    match l.export_refusal(id) {
                        Some(refusal) => scope.left_out.push((name, refusal.label())),
                        None => scope.photos.push(Chosen {
                            id: Some(id),
                            source: photo.path.clone(),
                            name,
                            open,
                            fields: NameFields::default(),
                            place,
                        }),
                    }
                }
            }
            // A photo opened without a catalog, still where it was opened from.
            _ if shown && open.is_none() => {
                let source = self.document.path.clone().unwrap_or_default();
                if !source.is_file() {
                    self.messages
                        .warn(format!("{} can't be exported: Offline", source.display()));
                    return None;
                }
                scope.chosen = 1;
                scope.photos.push(Chosen {
                    id: None,
                    // Its capture time and info are its file's: read in the
                    // background when its name needs them.
                    fields: NameFields::default(),
                    place: 0,
                    name: source
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    source,
                    open: true,
                });
            }
            _ => {}
        }
        if scope.photos.is_empty() {
            let text: String = match scope.left_out.as_slice() {
                [] => "Select a photo to export".into(),
                [(name, why)] => format!("{name} can't be exported: {why}"),
                left_out => format!("None of the {} photos can be exported", left_out.len()),
            };
            self.messages.warn(text);
            return None;
        }
        if scope.photos.iter().any(|p| p.open)
            && let Some(why) = self.open_photo_unready()
        {
            self.messages
                .warn(format!("Export waits for the open photo: {why}"));
            return None;
        }
        // Metadata typed in the Library goes in the names and the files.
        if !self.commit_library_drafts() {
            self.messages
                .fail(format!("Export stopped: {}", self.messages.line()));
            return None;
        }
        // What the names are made from, as the catalog has it now, in one
        // reading: the Example and the files named agree whatever changes.
        let ids: Vec<PhotoId> = scope.photos.iter().filter_map(|p| p.id).collect();
        if let Some(l) = &self.library
            && !ids.is_empty()
        {
            match l.session.catalog().name_fields(&ids) {
                Ok(fields) => {
                    let chosen = scope.photos.iter_mut().filter(|p| p.id.is_some());
                    for (chosen, fields) in chosen.zip(fields) {
                        chosen.fields = fields;
                    }
                }
                Err(e) => {
                    self.messages.fail(format!(
                        "Export stopped: the catalog could not be read: {e:#}"
                    ));
                    return None;
                }
            }
        }
        // The region's number format, read once with the fields.
        let decimal = crate::export_settings::Decimal(crate::platform::locale::decimal_separator());
        for photo in &mut scope.photos {
            photo.fields.decimal = decimal;
        }
        Some(scope)
    }

    /// Whether `template` needs what the scope's files give.
    fn names_need_files(&self, template: &NameTemplate) -> bool {
        let scope = &self.exports.scope;
        !scope.photos.is_empty()
            && template
                .needs()
                .and(name_read::file_sourced(
                    scope.standalone(),
                    scope.lacks_descriptive(),
                ))
                .any()
    }
    /// Whether names by `template` wait for the scope's files: it needs
    /// them, and they are being read; started here the first time.
    fn names_wait(&mut self, template: &NameTemplate) -> bool {
        if !self.names_need_files(template) {
            return false;
        }
        match self.exports.read.status() {
            Status::Idle => {
                let photos = self
                    .exports
                    .scope
                    .photos
                    .iter()
                    .map(|p| ToRead {
                        path: p.source.clone(),
                        standalone: p.id.is_none(),
                        descriptive: p.fields.lacks_descriptive(),
                    })
                    .collect();
                self.exports
                    .read
                    .start(photos, self.tx.clone(), self.context.clone());
                true
            }
            Status::Reading { .. } => true,
            Status::Ready | Status::Failed(_) => false,
        }
    }
    /// What `template`'s names wait for or lack from the files: `None` when
    /// they need nothing from them, or have it.
    fn names_read_status(&self, template: &NameTemplate) -> Option<Status> {
        match self.exports.read.status() {
            Status::Ready => None,
            status if self.names_need_files(template) => Some(status),
            _ => None,
        }
    }

    /// The scope's files were read: their fields go in its photos, and Export
    /// with Previous goes on, or (when they could not be read) opens the
    /// dialog to say so. A read of another scope is dropped.
    pub(in crate::app) fn names_read(&mut self, request: u64, read: Result<Reads, String>) {
        let Some((read, failed)) = self.exports.read.finished(request, read) else {
            return;
        };
        // What could be read goes in, whether or not all could.
        for (chosen, read) in self.exports.scope.photos.iter_mut().zip(read) {
            if let Some(read) = read {
                name_read::fill(&mut chosen.fields, read, chosen.id.is_none());
            }
        }
        let Some(settings) = self.exports.waiting.take() else {
            return;
        };
        match failed {
            None => self.export(settings),
            Some(why) => {
                self.messages.warn(format!(
                    "Photo info could not be read for the file names: {why}"
                ));
                self.exports.draft = settings;
                self.exports.watermarks = crate::watermark::presets();
                self.exports.dialog = true;
            }
        }
    }

    /// The Filename Template Editor over the dialog: its Example is the
    /// first photo named by the template being edited, as the export would
    /// name it, its files read first if it needs them; Done makes the
    /// template the dialog's.
    fn name_editor_window(&mut self, ctx: &egui::Context) {
        let Some(parsed) = self.exports.name_editor.as_ref().map(NameEditor::parsed) else {
            return;
        };
        let mut settings = self.exports.draft.clone();
        let example = match &parsed {
            Ok(template) => {
                settings.choose_tokens(&template.to_lightroom());
                let named = settings.name_template();
                self.names_wait(&named);
                let status = self.names_read_status(&named);
                let first = self.exports.scope.photos.first();
                let (source, fields, place) = first
                    .map(|p| (p.source.clone(), p.fields.clone(), p.place))
                    .unwrap_or_default();
                let at = crate::export_settings::NameContext {
                    index: place,
                    total: self.exports.scope.chosen.max(1),
                    fields: &fields,
                };
                let (name, notes) = settings.file_name_for(&source, &at);
                match status {
                    Some(Status::Reading { .. }) => ("Reading photo info…".to_string(), Vec::new()),
                    Some(Status::Failed(why)) => {
                        (name, vec![format!("photo info could not be read: {why}")])
                    }
                    _ => (name, notes),
                }
            }
            Err(_) => (String::new(), Vec::new()),
        };
        let Some(name_editor) = self.exports.name_editor.as_mut() else {
            return;
        };
        match name_editor.show(ctx, &example, &mut self.exports.saved) {
            editor::Outcome::Open => {}
            editor::Outcome::Done(template) => {
                self.exports.draft.choose_tokens(&template.to_lightroom());
                self.exports.name_editor = None;
            }
            editor::Outcome::Cancel => self.exports.name_editor = None,
        }
    }

    /// Export with Previous waiting for the files its names need: how far,
    /// and Cancel.
    fn export_waiting(&mut self, ctx: &egui::Context) {
        if self.exports.waiting.is_none() {
            return;
        }
        let palette = theme::palette(ctx);
        let line = match self.exports.read.status() {
            Status::Reading { done, total } => {
                format!("Reading photo info for the file names… {done} of {total}")
            }
            _ => "Reading photo info for the file names…".into(),
        };
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("export-names-reading"))
            .frame(
                crate::app::ui::widgets::modal_frame(&palette)
                    .inner_margin(design::margin_same(inset::DIALOG)),
            )
            .show(ctx, |ui| {
                ui.set_width(360.);
                ui.label(
                    egui::RichText::new(line)
                        .size(text::LARGE)
                        .color(palette.tone(Tone::TEXT)),
                );
                ui.add_space(space::XL);
                cancel = ui.button("Cancel").clicked();
            });
        if cancel || response.should_close() {
            self.exports.read.cancel();
            self.exports.waiting = None;
            self.exports.scope = Scope::default();
            self.messages.note("Export cancelled");
        }
    }

    /// Why the open photo's edit is not final yet, if it is not: what it shows
    /// would not be what is exported.
    fn open_photo_unready(&self) -> Option<&'static str> {
        let d = &self.document;
        if d.metadata.is_none() || d.full().is_none() {
            Some("it is still loading")
        } else if d.pending_lightroom.is_some() {
            Some("its Lightroom edit is still being applied")
        } else if d.auto.is_running() {
            Some("Auto is still working on it")
        } else if d.edit.save_state().is_protected() {
            Some("its edit is protected, as the file changed since it was saved")
        } else if self.importing.is_some() {
            Some("camera profiles are still being imported")
        } else if d.upright.is_running() || d.edit.recipe().upright.needs_analysis() {
            Some("Upright is still analysing it")
        } else if d.straighten.is_running() || d.edit.recipe().straighten_pending() {
            Some("Auto straighten is still measuring it")
        } else {
            None
        }
    }

    /// The open photo as it is now, with its catalog rating, label and keywords.
    pub(in crate::app) fn export_photo(&mut self) -> Option<Photo> {
        let catalog = self
            .document
            .catalog_photo
            .and_then(|id| self.library.as_ref()?.photo(id));
        let values = match (catalog, self.library.as_ref()) {
            (Some(p), Some(library)) => {
                let read = library
                    .session
                    .catalog()
                    .descriptive(p.id)
                    .and_then(|descriptive| {
                        let keywords = library.session.catalog().keywords(p.id)?;
                        Ok(Values {
                            descriptive,
                            keywords: keywords
                                .into_iter()
                                .map(|k| crate::xmp::write::KeywordPath {
                                    path: k.path,
                                    exported: k.exported,
                                })
                                .collect(),
                            rating: p.rating,
                            label: p.label.clone(),
                        })
                    });
                read.map_err(|e| format!("Metadata could not be read for export: {e}"))
            }
            _ => Ok(Values::default()),
        };
        let values = match values {
            Ok(values) => values,
            Err(e) => {
                self.messages.fail(e);
                return None;
            }
        };
        Some(Photo {
            image: self.document.full()?.clone(),
            source: self.document.path.clone()?,
            recipe: self.document.edit.recipe().clone(),
            values,
            watermark: None,
            demosaic: self.session.demosaic,
            cache: self.decode_cache.clone(),
            ai: self.ai_service(),
            file: self.document.image_file.clone(),
        })
    }

    /// Exports the scope chosen with `settings`: each photo as the catalog has it
    /// now, the open one as shown. Asks once about files that already exist.
    fn export(&mut self, settings: ExportSettings) {
        if !self.exports.unsaved_settings
            && let Err(e) = settings.save()
        {
            self.messages
                .fail(format!("Export settings not saved: {e:#}"));
        }
        let scope = std::mem::take(&mut self.exports.scope);
        // Its names are made from what the scope has now.
        self.exports.read.cancel();
        let watermark = if settings.watermark {
            match watermark_for(&settings.watermark_name) {
                Some(w) => Some(w),
                None => {
                    self.messages
                        .fail(format!("Watermark not found: {}", settings.watermark_name));
                    return;
                }
            }
        } else {
            None
        };
        // Metadata typed in the Library was committed when the scope was
        // chosen (`export_scope`): nothing can be typed while it is open.
        // The open photo is exported as shown; if its edit can't be saved, it is
        // still exported, and the report says so.
        let unsaved = scope.photos.iter().any(|p| p.open) && !self.flush();
        let batch = match self.batch_photos(&scope, unsaved) {
            Ok(batch) => batch,
            Err(e) => {
                self.messages.fail(format!(
                    "Export stopped: the catalog could not be read: {e:#}"
                ));
                return;
            }
        };
        self.plan_export(
            Pending {
                batch,
                settings,
                watermark,
                left_out: scope.left_out,
                conflicts: Vec::new(),
            },
            None,
        );
    }

    /// The scope's photos as the catalog has them now, read in one transaction.
    fn batch_photos(&self, scope: &Scope, unsaved: bool) -> anyhow::Result<Vec<BatchPhoto>> {
        let ids: Vec<PhotoId> = scope.photos.iter().filter_map(|p| p.id).collect();
        let mut records = match &self.library {
            Some(l) if !ids.is_empty() => l.session.catalog().photo_records(&ids)?.into_iter(),
            _ => Vec::new().into_iter(),
        };
        Ok(scope
            .photos
            .iter()
            .map(|chosen| {
                let mut photo = match chosen.id {
                    Some(_) => BatchPhoto::from_record(
                        records.next().expect("a record for each photo"),
                        chosen.source.clone(),
                        chosen.name.clone(),
                        chosen.fields.clone(),
                    ),
                    None => BatchPhoto {
                        source: chosen.source.clone(),
                        name: chosen.name.clone(),
                        edit: Edit::Catalog(Default::default()),
                        values: Values::default(),
                        naming: chosen.fields.clone(),
                        place: None,
                    },
                };
                photo.place = Some((chosen.place, scope.chosen));
                if chosen.open {
                    photo.edit = Edit::Shown {
                        recipe: Box::new(self.document.edit.recipe().clone()),
                        unsaved,
                        file: self.document.file.clone(),
                    };
                }
                photo
            })
            .collect())
    }

    /// Plans where each file goes and queues the batch, or asks about the files
    /// that exist first.
    fn plan_export(&mut self, mut pending: Pending, answer: Option<Existing>) {
        match batch::plan(&pending.batch, &pending.settings, answer) {
            Ok(plan) => {
                let names = pending.batch.iter().map(|p| p.name.clone()).collect();
                let after = pending.settings.after_export;
                let batch = batch::Batch {
                    photos: pending.batch,
                    plan,
                    settings: pending.settings,
                    defaults: self.raw_defaults.clone(),
                    watermark: pending.watermark,
                    demosaic: self.session.demosaic,
                    cache: self.decode_cache.clone(),
                    ai: self.ai_service(),
                };
                let ticket = self.export_queue().submit(batch);
                self.exports.queued.insert(
                    ticket,
                    Queued {
                        names,
                        left_out: pending.left_out,
                        after,
                    },
                );
            }
            Err(Unplanned::NoFolder) => self.messages.warn("Choose a folder to export to"),
            Err(Unplanned::Settings(reason)) => {
                self.messages.fail(format!("Export stopped: {reason}"))
            }
            Err(Unplanned::Conflicts(conflicts)) => {
                pending.conflicts = conflicts;
                self.exports.pending = Some(pending);
            }
        }
    }

    /// The export queue, started with the first export.
    fn export_queue(&mut self) -> &Queue {
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        let repaint = self.context.clone();
        self.exports.queue.get_or_insert_with(|| {
            Queue::new(
                move |ticket, outcomes| {
                    let _ = tx.send(Event::BatchExported { ticket, outcomes });
                    ctx.request_repaint();
                },
                move || repaint.request_repaint(),
            )
        })
    }

    /// After Export: Show in Finder, for the files a batch wrote.
    fn show_exported(&mut self, exported: &[PathBuf]) {
        let shown = to_show(exported);
        let folders = folders(exported);
        for path in &shown {
            if let Err(e) = crate::platform::reveal::reveal(path) {
                self.messages
                    .warn(format!("Exported files not shown: {e:#}"));
                return;
            }
        }
        if folders > shown.len() {
            let line = format!(
                "{} · showing {} of the {folders} folders exported to",
                self.messages.line(),
                shown.len()
            );
            self.messages.note(line);
        }
    }

    /// A finished batch: the top bar says how it went, and what could not be
    /// exported stays in the top bar until it is read.
    pub(in crate::app) fn batch_exported(&mut self, ticket: u64, outcomes: Vec<Outcome>) {
        let Some(queued) = self.exports.queued.remove(&ticket) else {
            return;
        };
        let (summary, exported) = summarize(&queued, &outcomes);
        // One photo exported with nothing to say is named in the top bar.
        let single = (summary.total == 1 && summary.noted == 0)
            .then(|| exported.first().cloned())
            .flatten();
        match single {
            Some(path) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                self.messages
                    .done_with_detail(format!("Exported {name}"), path.display().to_string());
            }
            None if !summary.problems.is_empty() => self
                .messages
                .fail_with_detail(summary.line(), summary.details()),
            None if !summary.notes.is_empty() => self
                .messages
                .warn_with_detail(summary.line(), summary.details()),
            None => self.messages.done(summary.line()),
        };
        // After the summary, so a file manager that can't be opened is said.
        if queued.after == crate::export_settings::AfterExport::Show {
            self.show_exported(&exported);
        }
    }

    /// Lightroom's activity indicator: while exports run, "Exporting 24 / 150"
    /// over a bar, with a button that cancels the running one and a menu of those
    /// waiting; after one that could not export everything, what it could not do,
    /// until it is read.
    pub(in crate::app) fn export_progress(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let status = self.exports.queue.as_ref().map(Queue::status);
        let Some((ticket, progress)) = status.as_ref().and_then(|s| s.running) else {
            return;
        };
        let waiting = status.map(|s| s.waiting).unwrap_or_default();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(210., 28.), Sense::click());
        let painter = ui.painter();
        let current = (progress.done + 1).min(progress.total);
        let mut label = format!("Exporting {current} / {}", progress.total);
        if !waiting.is_empty() {
            label.push_str(&format!(" · {} waiting", waiting.len()));
        }
        painter.text(
            egui::pos2(rect.left(), rect.top() + 7.),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(text::SMALL),
            palette.tone(Tone::SOFT),
        );
        let fraction = (progress.done as f32 + progress.fraction) / progress.total.max(1) as f32;
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
        // The exports waiting, each of which can be removed before it starts.
        if !waiting.is_empty() {
            egui::Popup::menu(&response).show(|ui| {
                for (queued, photos) in &waiting {
                    let text = format!("Waiting: {}", plural(*photos, "photo", "photos"));
                    if ui.button(format!("Remove · {text}")).clicked()
                        && let Some(queue) = &self.exports.queue
                        && queue.remove(*queued)
                    {
                        self.exports.queued.remove(queued);
                        ui.close();
                    }
                }
            });
        }
        let close = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 9., rect.center().y),
            Vec2::splat(18.),
        );
        let cancel = ui
            .interact(close, ui.id().with("cancel-export"), Sense::click())
            .on_hover_text("Cancel export");
        let color = palette.tone(if cancel.hovered() {
            Tone::STRONG
        } else {
            Tone::SECONDARY
        });
        crate::app::ui::icons::paint_at(
            ui.painter(),
            crate::app::ui::icons::Icon::Close,
            close.center(),
            13.,
            color,
        );
        if cancel.clicked()
            && let Some(queue) = &self.exports.queue
        {
            queue.cancel(ticket);
        }
    }
}

/// How a batch went: the report, and the file when it was one photo exported
/// with nothing to say.
fn summarize(queued: &Queued, outcomes: &[Outcome]) -> (Summary, Vec<PathBuf>) {
    let mut summary = Summary::default();
    let mut exported = Vec::new();
    for (name, outcome) in queued.names.iter().zip(outcomes) {
        let problem = match outcome {
            Outcome::Exported { path, notes } => {
                exported.push(path.clone());
                summary.noted += usize::from(!notes.is_empty());
                for note in notes {
                    summary.notes.push((name.clone(), note.clone()));
                }
                continue;
            }
            Outcome::Skipped(reason) => format!("skipped, {reason}"),
            Outcome::Failed(reason) => reason.clone(),
            Outcome::Cancelled => "export cancelled".into(),
            Outcome::NotStarted => "not started, export cancelled".into(),
        };
        summary.problems.push((name.clone(), problem));
    }
    for (name, why) in &queued.left_out {
        summary
            .problems
            .push((name.clone(), format!("can't be exported: {why}")));
    }
    summary.exported = exported.len();
    summary.total = queued.names.len() + queued.left_out.len();
    (summary, exported)
}

/// How many folders `exported` went to.
fn folders(exported: &[PathBuf]) -> usize {
    let mut folders: Vec<_> = exported.iter().map(|p| p.parent()).collect();
    folders.sort();
    folders.dedup();
    folders.len()
}

/// The first file of each folder exported to, at most five: one file manager
/// window for each folder, not one for each photo, and never a screenful. The
/// status line says when there were more folders.
fn to_show(exported: &[PathBuf]) -> Vec<PathBuf> {
    let mut shown: Vec<PathBuf> = Vec::new();
    for path in exported {
        if shown.len() == 5 {
            break;
        }
        if !shown.iter().any(|s| s.parent() == path.parent()) {
            shown.push(path.clone());
        }
    }
    shown
}

/// The watermark named in the Export dialog: a saved preset, or the Simple
/// Copyright Watermark, whose text comes from each photo at export.
fn watermark_for(name: &str) -> Option<crate::watermark::Watermark> {
    use crate::watermark::{SIMPLE_COPYRIGHT, Watermark, presets};
    if name == SIMPLE_COPYRIGHT {
        return Some(Watermark {
            name: SIMPLE_COPYRIGHT.into(),
            ..Default::default()
        });
    }
    presets().into_iter().find(|w| w.name == name)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod editor_tests;

pub(in crate::app) mod naming;
