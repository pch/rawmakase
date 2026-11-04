//! Renaming (planned in #556): Rename Photo (F2 on one photo), Rename
//! Photos (F2 on several, numbered in the order they were taken, #342) and
//! Rename Folder…
//!
//! Renaming photos holds the catalog while its dialog is open, as a
//! confirmation does: the open edit is saved and `Activity::ChangingFiles`
//! taken first, and the photos' order is read once, on a worker, from the
//! catalog's capture times, then kept. What the dialog would do is planned
//! on a worker as it is typed, one plan at a time, the latest input next,
//! so a name taken, or one the system can't have, is said as soon as it is
//! known while typing goes on (#681); a plan for input since changed is
//! only shown until the next. Rename runs the plan of exactly what is
//! typed, which is its confirmation (a rename keeps every byte, and is
//! undone by another rename): asked before that plan is in, it runs once it
//! is, if nothing was typed meanwhile. Rename Folder… holds nothing until
//! Rename, which plans and runs.
//!
//! Several photos are named by a template, as Export names files: a preset,
//! one of RAWmakase's own, or one made in the Filename Template Editor
//! (File Naming > Edit…). Their names come from the catalog's fields, read
//! with their order, and from their files (the camera's make and model,
//! and a title, caption, copyright or creator the catalog has none of),
//! read in the background once a template needs them (`naming::read`), and
//! cancelled with the dialog. A plan names photos from the fields it was
//! given: one made before the files were read is replaced once they are,
//! and Rename waits for that read. The choice is remembered for the next
//! Rename Photos (`RenameSettings`) when Rename is asked for.
use super::{State, Verb};
use crate::app::export::naming::editor::{NameEditor, Outcome};
use crate::app::export::naming::menu::{self, Choice};
use crate::app::export::naming::presets::{Library, Preset};
use crate::app::export::naming::read::{self as name_read, NameRead, Reads, Status, ToRead};
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::{Editor, ui::widgets};
use crate::catalog::file_ops::{self, FileOp, Plan, RenameOrder, Skip};
use crate::catalog::{CatalogLocation, PhotoId, RootId};
use crate::export_settings::{NameContext, NameTemplate, RenameSettings};
use eframe::egui::{self, Vec2};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Rename Photo or Rename Photos, open.
pub(super) struct Renaming {
    catalog: CatalogLocation,
    /// The session's when it opened: a catalog read again since may have
    /// given its photos' ids to others.
    generation: u64,
    order: Arc<RenameOrder>,
    /// One photo: its new name before the extension, and the extension.
    stem: String,
    extension: String,
    /// Several: how they are named.
    settings: RenameSettings,
    /// The Filename Template Editor, over the dialog.
    editor: Option<NameEditor>,
    /// The saved file-name templates, read when the dialog opens.
    saved: Library,
    /// What the names need from the photos' files, read once a template
    /// needs it; then in `order`'s fields.
    read: NameRead,
    /// Counts the times `order`'s fields changed, so a plan made from
    /// earlier fields is not taken for one of these.
    fields: u64,
    /// The names last planned, which may be for input changed since.
    planned: Option<Planned>,
    /// The plan under way on a worker, and the input it plans.
    planning: Option<(u64, Inputs)>,
    /// The catalog plans read, between plans: each takes it to its worker
    /// and gives it back, so typing opens it once.
    reader: Option<crate::catalog::Catalog>,
    /// Rename asked for this input before its plan was in.
    submitted: Option<Inputs>,
    focused: bool,
}

/// What the dialog's names are planned from.
#[derive(Clone, Debug, PartialEq)]
struct Inputs {
    stem: String,
    template: NameTemplate,
    /// Which of `order`'s fields (see `Renaming::fields`) the names take,
    /// when they take any from the files; 0 when they don't.
    fields: u64,
}

/// What the dialog's names would do, planned for the input they came from.
struct Planned {
    inputs: Inputs,
    rows: Vec<Row>,
    /// The plan of exactly these names, when it renames anything and
    /// nothing blocks it.
    plan: Option<Plan>,
}

/// A plan of the dialog's names, back from its worker.
pub(crate) struct RenamePlanned {
    request: u64,
    reader: Option<crate::catalog::Catalog>,
    /// The rows and plan; `Err` when the catalog could not be opened or the
    /// worker stopped.
    planned: Result<(Vec<Row>, Option<Plan>), String>,
}

#[cfg(test)]
impl RenamePlanned {
    /// The answer of plan `request` when its worker failed with `message`.
    pub(super) fn failed(request: u64, message: &str) -> Self {
        Self {
            request,
            reader: None,
            planned: Err(message.into()),
        }
    }
}

/// Tells each plan of a rename dialog's names from every other.
static PLANS: AtomicU64 = AtomicU64::new(0);

/// A photo's name now and as renamed, and why it can't be.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    old: String,
    new: String,
    problem: Option<String>,
}

/// What a rename dialog shows, for tests: once the names typed are planned.
#[cfg(test)]
pub(super) struct RenameShown {
    pub title: String,
    pub rows: Vec<(String, String, Option<String>)>,
    pub can_rename: bool,
}

/// Rename Folder…, open. Holds nothing.
pub(super) struct RenamingFolder {
    catalog: CatalogLocation,
    root: RootId,
    /// Its logical path, and its name now.
    logical: String,
    old: String,
    name: String,
    focused: bool,
}

impl Renaming {
    fn one(&self) -> bool {
        self.order.photos().len() == 1
    }

    fn title(&self) -> String {
        match self.order.photos() {
            [one] => format!("Rename Photo “{}”", one.filename),
            photos => format!("Rename {} Photos", photos.len()),
        }
    }

    fn inputs(&self) -> Inputs {
        let template = self.settings.name_template();
        Inputs {
            stem: self.stem.clone(),
            // Names that take nothing from the files are the same whatever
            // they gave, so their plan stays.
            fields: if self.needs_files(&template) {
                self.fields
            } else {
                0
            },
            template,
        }
    }

    /// Whether names by `template` need what the photos' files give.
    fn needs_files(&self, template: &NameTemplate) -> bool {
        let lacks = self
            .order
            .photos()
            .iter()
            .any(|o| o.fields.lacks_descriptive());
        !self.one()
            && template
                .needs()
                .and(name_read::file_sourced(false, lacks))
                .any()
    }

    /// Whether names by `template` wait for the photos' files: they need
    /// them, and they are being read; started here the first time.
    fn names_wait(
        &mut self,
        template: &NameTemplate,
        events: &std::sync::mpsc::Sender<crate::app::worker::Event>,
        ctx: &egui::Context,
    ) -> bool {
        if !self.needs_files(template) {
            return false;
        }
        match self.read.status() {
            Status::Idle => {
                let photos = self
                    .order
                    .photos()
                    .iter()
                    .map(|o| ToRead {
                        path: o.path.clone(),
                        standalone: false,
                        descriptive: o.fields.lacks_descriptive(),
                    })
                    .collect();
                self.read.start(photos, events.clone(), ctx.clone());
                true
            }
            Status::Reading { .. } => true,
            Status::Ready | Status::Failed(_) => false,
        }
    }

    /// What `template`'s names wait for or lack from the files: `None` when
    /// they need nothing from them, or have it.
    fn read_status(&self, template: &NameTemplate) -> Option<Status> {
        match self.read.status() {
            Status::Ready => None,
            status if self.needs_files(template) => Some(status),
            _ => None,
        }
    }

    /// The first photo's name by `template`, as the dialog would name it,
    /// and what it has to say.
    fn example(&self, template: &NameTemplate) -> (String, Vec<String>) {
        match self.read_status(template) {
            Some(Status::Reading { .. }) => return ("Reading photo info…".into(), Vec::new()),
            Some(Status::Failed(why)) => {
                let (name, _) = self.example_name(template);
                return (name, vec![format!("photo info could not be read: {why}")]);
            }
            _ => {}
        }
        self.example_name(template)
    }
    fn example_name(&self, template: &NameTemplate) -> (String, Vec<String>) {
        let Some(first) = self.order.photos().first() else {
            return Default::default();
        };
        let at = NameContext {
            index: 0,
            total: self.order.photos().len(),
            fields: &first.fields,
        };
        let (stem, notes) = template.stem(&first.path, &at);
        let name = match first.path.extension() {
            Some(extension) => format!("{stem}.{}", extension.to_string_lossy()),
            None => stem,
        };
        (name, notes)
    }

    /// The plan of what is typed now, once it is in.
    fn current(&self) -> Option<&Planned> {
        let inputs = self.inputs();
        self.planned.as_ref().filter(|p| p.inputs == inputs)
    }

    /// Each photo with `problem`, as nothing could be planned.
    fn unplanned(&self, inputs: &Inputs, problem: &str) -> Vec<Row> {
        let new = |old: &str| match (self.one(), self.extension.is_empty()) {
            (false, _) => old.to_string(),
            (true, true) => inputs.stem.clone(),
            (true, false) => format!("{}.{}", inputs.stem, self.extension),
        };
        self.order
            .photos()
            .iter()
            .map(|o| Row {
                old: o.filename.clone(),
                new: new(&o.filename),
                problem: Some(problem.to_string()),
            })
            .collect()
    }
}

/// Plans the names `inputs` give the photos of `order`, one photo's with
/// `extension`, in `catalog`.
fn plan_names(
    catalog: &crate::catalog::Catalog,
    order: &RenameOrder,
    extension: &str,
    inputs: &Inputs,
) -> (Vec<Row>, Option<Plan>) {
    match order.photos() {
        [one] => plan_one(catalog, one, extension, &inputs.stem),
        _ => plan_several(catalog, order, &inputs.template),
    }
}

fn plan_one(
    catalog: &crate::catalog::Catalog,
    photo: &file_ops::Ordered,
    extension: &str,
    stem: &str,
) -> (Vec<Row>, Option<Plan>) {
    let new = if extension.is_empty() {
        stem.to_string()
    } else {
        format!("{stem}.{extension}")
    };
    let row = |problem: Option<String>| Row {
        old: photo.filename.clone(),
        new: new.clone(),
        problem,
    };
    if let Some(problem) = file_ops::photo_name_problem(stem) {
        return (vec![row(Some(problem.to_string()))], None);
    }
    let op = FileOp::RenamePhotos {
        renames: vec![(photo.photo, stem.to_string())],
    };
    match file_ops::plan(catalog, op) {
        Ok(plan) => match plan.skipped().first() {
            Some(s) => (vec![row(Some(s.reason.to_string()))], None),
            None if plan.is_empty() => (vec![row(None)], None),
            None => (vec![row(None)], Some(plan)),
        },
        Err(e) => (vec![row(Some(e.to_string()))], None),
    }
}

fn plan_several(
    catalog: &crate::catalog::Catalog,
    order: &RenameOrder,
    template: &NameTemplate,
) -> (Vec<Row>, Option<Plan>) {
    match file_ops::rename_preview(catalog, order, template) {
        Ok(preview) => {
            let rows: Vec<Row> = preview
                .rows
                .iter()
                .map(|r| Row {
                    old: r.old.clone(),
                    new: r.new.clone(),
                    problem: r
                        .problem
                        .as_ref()
                        .filter(|p| **p != Skip::SameName)
                        .map(ToString::to_string)
                        .or_else(|| r.note.clone()),
                })
                .collect();
            let blocked = preview
                .rows
                .iter()
                .any(|r| r.problem.as_ref().is_some_and(|p| *p != Skip::SameName));
            let plan = (!blocked && !preview.plan.is_empty()).then_some(preview.plan);
            (rows, plan)
        }
        Err(e) => {
            let rows = order
                .photos()
                .iter()
                .map(|o| Row {
                    old: o.filename.clone(),
                    new: o.filename.clone(),
                    problem: Some(e.to_string()),
                })
                .collect();
            (rows, None)
        }
    }
}

impl Editor {
    /// F2 and Rename Photo…: renames `photos`, one by its name, several by a
    /// template in the order they were taken. Saves what is open and holds
    /// the catalog while their order is read and the dialog is open.
    pub(in crate::app) fn request_rename(&mut self, photos: Vec<PhotoId>) {
        if photos.is_empty() {
            return;
        }
        if self.activity.is_busy() || !matches!(self.file_ops.state, State::Idle) {
            self.messages.warn("Finish what is running first");
            return;
        }
        let Some(location) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone())
        else {
            return;
        };
        if !self.flush() {
            self.messages
                .fail("The open photo's edit could not be saved; nothing was renamed");
            return;
        }
        if !self.activity.begin_file_change() {
            return;
        }
        if let Some(library) = &mut self.library {
            library.suspend_background_reads();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.file_ops.state = State::Ordering {
            catalog: location.clone(),
            cancel: cancel.clone(),
        };
        let failed = (location.clone(), cancel.clone());
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let ordered = crate::catalog::Catalog::open(&location)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|catalog| {
                        file_ops::rename_order(&catalog, &photos).map_err(|e| e.to_string())
                    });
                let _ = tx.send(crate::app::worker::Event::RenameOrdered(
                    location,
                    cancel,
                    Box::new(ordered),
                ));
            },
            move |tx, message| {
                let _ = tx.send(crate::app::worker::Event::RenameOrdered(
                    failed.0,
                    failed.1,
                    Box::new(Err(message)),
                ));
            },
        );
    }

    /// The photos' order came back: the dialog opens, or nothing can be
    /// renamed.
    pub(in crate::app) fn rename_ordered(
        &mut self,
        location: CatalogLocation,
        read_with: Arc<AtomicBool>,
        ordered: Result<RenameOrder, String>,
    ) {
        // Only the reading under way, not one cancelled before it.
        let ours = matches!(
            &self.file_ops.state,
            State::Ordering { cancel, .. } if Arc::ptr_eq(cancel, &read_with)
        );
        if !ours {
            return;
        }
        let State::Ordering { catalog, .. } = std::mem::take(&mut self.file_ops.state) else {
            return;
        };
        if location != catalog || !self.showing_catalog(&catalog) {
            self.release_files();
            return;
        }
        let order = match ordered {
            Ok(order) if !order.photos().is_empty() => order,
            Ok(_) => {
                self.messages.warn("Nothing to rename");
                self.release_files();
                return;
            }
            Err(e) => {
                self.messages.fail(format!("Photos not renamed: {e}"));
                self.release_files();
                return;
            }
        };
        let generation = self.library.as_ref().map_or(0, |l| l.session.generation());
        let first = std::path::Path::new(&order.photos()[0].filename);
        let stem = first
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let extension = first
            .extension()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // The region's number format, read once with the fields.
        let decimal = crate::export_settings::Decimal(crate::platform::locale::decimal_separator());
        let order = order.with_fields(|_, fields| fields.decimal = decimal);
        let settings = self.remembered_rename();
        #[allow(unused_mut)]
        let mut read = NameRead::default();
        #[cfg(test)]
        if let Some(reader) = &self.file_ops.name_reader {
            read.set_reader(reader.clone());
        }
        self.file_ops.state = State::Renaming(Box::new(Renaming {
            catalog,
            generation,
            order: Arc::new(order),
            stem,
            extension,
            settings,
            editor: None,
            saved: Library::load(),
            read,
            fields: 0,
            planned: None,
            planning: None,
            reader: None,
            submitted: None,
            focused: false,
        }));
    }

    /// Plans what the rename dialog's names are now on a worker, unless
    /// that is planned, or another plan is under way: its input is planned
    /// once that one is in.
    fn plan_rename(&mut self) {
        let State::Renaming(r) = &mut self.file_ops.state else {
            return;
        };
        let inputs = r.inputs();
        if r.planning.is_some() || r.current().is_some() {
            return;
        }
        let request = PLANS.fetch_add(1, Ordering::Relaxed);
        r.planning = Some((request, inputs.clone()));
        let (location, order, extension) =
            (r.catalog.clone(), r.order.clone(), r.extension.clone());
        #[cfg(test)]
        if self.file_ops.hold_plans {
            return;
        }
        let reader = r.reader.take();
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let catalog = match reader {
                    Some(catalog) => Ok(catalog),
                    None => crate::catalog::Catalog::open(&location).map_err(|e| format!("{e:#}")),
                };
                let done = match catalog {
                    Ok(catalog) => RenamePlanned {
                        request,
                        planned: Ok(plan_names(&catalog, &order, &extension, &inputs)),
                        reader: Some(catalog),
                    },
                    Err(e) => RenamePlanned {
                        request,
                        planned: Err(e),
                        reader: None,
                    },
                };
                let _ = tx.send(crate::app::worker::Event::RenamePlanned(Box::new(done)));
            },
            move |tx, message| {
                let failed = RenamePlanned {
                    request,
                    planned: Err(message),
                    reader: None,
                };
                let _ = tx.send(crate::app::worker::Event::RenamePlanned(Box::new(failed)));
            },
        );
    }

    /// A plan of the rename dialog's names came back: it is shown, unless
    /// the dialog it was for is gone.
    pub(in crate::app) fn rename_planned(&mut self, done: RenamePlanned) {
        let State::Renaming(r) = &mut self.file_ops.state else {
            return;
        };
        let Some((_, inputs)) = r.planning.take_if(|(request, _)| *request == done.request) else {
            return;
        };
        r.reader = done.reader;
        let (rows, plan) = match done.planned {
            Ok(planned) => planned,
            Err(e) => (r.unplanned(&inputs, &e), None),
        };
        // Rename asked for it runs in the dialog's next frame, after what
        // that frame types or cancels: never from here.
        r.planned = Some(Planned { inputs, rows, plan });
        // What was typed meanwhile.
        self.plan_rename();
    }

    /// The answer to the plan under way, planned here, for tests that
    /// hold plans back from their workers.
    #[cfg(test)]
    pub(super) fn rename_answer_now(&self) -> Option<RenamePlanned> {
        let State::Renaming(r) = &self.file_ops.state else {
            return None;
        };
        let (request, inputs) = r.planning.clone()?;
        let catalog = &self.library.as_ref()?.session.catalog();
        Some(RenamePlanned {
            request,
            reader: None,
            planned: Ok(plan_names(catalog, &r.order, &r.extension, &inputs)),
        })
    }

    /// Rename Photos' choices from the last rename.
    fn remembered_rename(&self) -> RenameSettings {
        #[cfg(test)]
        return self.file_ops.rename_settings.clone();
        #[cfg(not(test))]
        RenameSettings::load()
    }
    /// Keeps Rename Photos' choices for the next one: what is named now
    /// does not depend on it, so a failure is only said.
    fn remember_rename(&mut self, settings: &RenameSettings) -> Result<(), String> {
        #[cfg(test)]
        {
            self.file_ops.rename_settings = settings.clone();
            Ok(())
        }
        #[cfg(not(test))]
        settings.save().map_err(|e| format!("{e:#}"))
    }

    /// The rename dialog's files were read for their names: what they gave
    /// goes in its photos' fields, which replaces the plan of the earlier
    /// ones. A read of another dialog, or one cancelled, is dropped.
    pub(in crate::app) fn rename_names_read(&mut self, request: u64, read: Result<Reads, String>) {
        let State::Renaming(r) = &mut self.file_ops.state else {
            return;
        };
        let Some((read, _failed)) = r.read.finished(request, read) else {
            return;
        };
        // What could be read goes in, whether or not all could; the dialog
        // says what could not.
        r.order = Arc::new(r.order.with_fields(|index, fields| {
            if let Some(Some(read)) = read.get(index) {
                name_read::fill(fields, read.clone(), false);
            }
        }));
        // Plans of names by the files' fields are of earlier ones now.
        r.fields += 1;
        self.plan_rename();
    }

    /// The Filename Template Editor over Rename Photos: its Example is the
    /// first photo named by the template being edited, its files read first
    /// if it needs them; Done makes it the dialog's template.
    fn rename_editor_window(&mut self, ctx: &egui::Context) {
        let (tx, context) = (self.tx.clone(), self.context.clone());
        let State::Renaming(r) = &mut self.file_ops.state else {
            return;
        };
        let Some(parsed) = r.editor.as_ref().map(NameEditor::parsed) else {
            return;
        };
        let example = match &parsed {
            Ok(template) => {
                let mut settings = r.settings.clone();
                settings.choose_tokens(&template.to_lightroom());
                let named = settings.name_template();
                r.names_wait(&named, &tx, &context);
                r.example(&named)
            }
            Err(_) => Default::default(),
        };
        let Some(editor) = r.editor.as_mut() else {
            return;
        };
        match editor.show(ctx, &example, &mut r.saved) {
            Outcome::Open => {}
            Outcome::Done(template) => {
                r.settings.choose_tokens(&template.to_lightroom());
                r.editor = None;
            }
            Outcome::Cancel => r.editor = None,
        }
    }

    /// Rename: runs the plan of what is typed, which is in and can be had.
    fn run_rename(&mut self) {
        let State::Renaming(r) = std::mem::take(&mut self.file_ops.state) else {
            return;
        };
        let inputs = r.inputs();
        let several = !r.one();
        let Some(Planned {
            plan: Some(plan),
            rows,
            ..
        }) = r.planned.filter(|p| p.inputs == inputs)
        else {
            // Not reached (callers check); the catalog is let go anyway.
            self.release_files();
            return;
        };
        if several && let Err(e) = self.remember_rename(&r.settings) {
            self.messages.warn(format!(
                "Rename Photos' choices not kept for next time: {e}"
            ));
        }
        let verb = Verb::Rename {
            one: match rows.as_slice() {
                [one] => Some((one.old.clone(), one.new.clone())),
                _ => None,
            },
        };
        self.file_ops.state = State::Confirming {
            plan: Box::new(plan),
            trash: None,
            catalog: r.catalog,
            generation: r.generation,
            verb,
        };
        self.confirm_file_op();
    }

    /// While the photos' order is read: Cancel (or Escape) stops it and
    /// lets the catalog go.
    pub(super) fn ordering_window(&mut self, ctx: &egui::Context) {
        let palette = crate::app::ui::theme::palette(ctx);
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("rename-ordering"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(360.);
                ui.label(
                    egui::RichText::new("Reading when the photos were taken…")
                        .size(text::LARGE)
                        .color(palette.tone(Tone::TEXT)),
                );
                ui.add_space(space::XL);
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        if cancel || response.should_close() {
            self.cancel_ordering();
        }
    }

    /// Stops reading the photos' order, and lets the catalog go.
    pub(in crate::app) fn cancel_ordering(&mut self) {
        if let State::Ordering { cancel, .. } = &self.file_ops.state {
            cancel.store(true, Ordering::Relaxed);
            self.file_ops.state = State::Idle;
            self.release_files();
        }
    }

    /// Rename Photo / Rename Photos: the name or the template, each photo's
    /// new name with what is wrong with it, and Rename.
    pub(super) fn rename_window(&mut self, ctx: &egui::Context) {
        let (State::Renaming(r), Some(library)) = (&mut self.file_ops.state, &self.library) else {
            return;
        };
        if library.session.catalog().location() != &r.catalog {
            self.file_ops.state = State::Idle;
            self.release_files();
            return;
        }
        let palette = crate::app::ui::theme::palette(ctx);
        let mut rename = false;
        let mut cancel = false;
        let mut edit = false;
        // What the names need from the files is read once they need it
        // (here, or once the Filename Template Editor adds it); Rename waits
        // for it, and the dialog says when it could not be read.
        let template = r.settings.name_template();
        r.names_wait(&template, &self.tx, &self.context);
        let read = r.read_status(&template);
        let reading = matches!(read, Some(Status::Reading { .. }));
        let example = r.example(&template);
        let can_rename = !reading && r.current().is_some_and(|p| p.plan.is_some());
        let checking = r.current().is_none();
        let title = r.title();
        // Several photos: Return is Rename, as in Lightroom, whichever
        // control has focus (Custom Text may be off); taken before the
        // dialog draws so a focused control does not also act on it. An open
        // menu, or the editor on top, keeps it.
        if !r.one()
            && r.editor.is_none()
            && !egui::Popup::is_any_open(ctx)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            rename = true;
        }
        let response = egui::Modal::new(egui::Id::new("rename-photos"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(if r.one() { 420. } else { 520. });
                ui.label(egui::RichText::new(&title).size(text::TITLE).color(palette.tone(Tone::STRONG)));
                ui.add_space(space::L);
                let field = if r.one() {
                    let field = ui.horizontal(|ui| {
                        let output = egui::TextEdit::singleline(&mut r.stem)
                            .desired_width(330.)
                            .show(ui);
                        if !r.extension.is_empty() {
                            ui.label(
                                egui::RichText::new(format!(".{}", r.extension))
                                    .color(palette.tone(Tone::SECONDARY)),
                            );
                        }
                        output
                    });
                    field.inner
                } else {
                    let label = |ui: &mut egui::Ui, text: &str| {
                        ui.add_sized(
                            Vec2::new(96., 20.),
                            egui::Label::new(egui::RichText::new(text).color(palette.tone(Tone::LABEL))),
                        );
                    };
                    ui.horizontal(|ui| {
                        label(ui, "File Naming");
                        let s = &mut r.settings;
                        let choice = menu::combo(
                            ui,
                            "rename-naming",
                            260.,
                            s.naming,
                            s.tokens.as_deref(),
                            &r.saved,
                        );
                        match choice {
                            Some(Choice::Preset(naming)) => s.choose_naming(naming),
                            Some(Choice::Tokens(tokens)) => s.choose_tokens(&tokens),
                            Some(Choice::Edit) => edit = true,
                            None => {}
                        }
                    });
                    let output = ui
                        .horizontal(|ui| {
                            label(ui, "Custom Text");
                            ui.add_enabled_ui(template.uses_text(), |ui| {
                                egui::TextEdit::singleline(&mut r.settings.custom_text)
                                    .desired_width(260.)
                                    .show(ui)
                            })
                            .inner
                        })
                        .inner;
                    ui.horizontal(|ui| {
                        label(ui, "Start Number");
                        ui.add_enabled(
                            template.uses_sequence(),
                            egui::DragValue::new(&mut r.settings.start_number).range(0..=999_999),
                        );
                    });
                    ui.horizontal(|ui| {
                        label(ui, "Example");
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&example.0).color(palette.tone(Tone::TEXT)),
                            )
                            .truncate(),
                        );
                    });
                    if let Some(Status::Failed(why)) = &read {
                        ui.label(
                            egui::RichText::new(format!("Photo info could not be read: {why}"))
                                .size(text::LABEL)
                                .color(palette.warning()),
                        );
                    }
                    output
                };
                if !r.focused {
                    field.response.request_focus();
                    // The name is typed over, not added to.
                    let mut state = field.state.clone();
                    let end = egui::text::CCursor::new(field.galley.text().chars().count());
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        end,
                    )));
                    state.store(ui.ctx(), field.response.id);
                    r.focused = true;
                }
                if field.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    rename = true;
                }
                ui.add_space(space::M);
                if checking {
                    ui.label(
                        egui::RichText::new("Checking the names…")
                            .size(text::LABEL)
                            .color(palette.tone(Tone::MUTED)),
                    );
                }
                if let Some(planned) = &r.planned {
                    let rows = |ui: &mut egui::Ui| {
                        for row in &planned.rows {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&row.old)
                                        .size(text::LABEL)
                                        .color(palette.tone(Tone::SECONDARY)),
                                );
                                ui.label(egui::RichText::new("→").size(text::LABEL).color(palette.tone(Tone::DISABLED)));
                                ui.label(
                                    egui::RichText::new(&row.new)
                                        .size(text::LABEL)
                                        .color(palette.tone(Tone::TEXT)),
                                );
                            });
                            if let Some(problem) = &row.problem {
                                ui.label(
                                    egui::RichText::new(problem)
                                        .size(text::LABEL)
                                        .color(palette.warning()),
                                );
                            }
                        }
                    };
                    if r.order.photos().len() > 1 {
                        egui::ScrollArea::vertical().max_height(260.).show(ui, rows);
                        ui.add_space(space::XS);
                        ui.label(
                            egui::RichText::new(
                                "Numbered in the order the photos were taken; those without a capture time come last.",
                            )
                            .size(text::LABEL)
                            .color(palette.tone(Tone::MUTED)),
                        );
                    } else if planned.rows.iter().any(|r| r.problem.is_some()) {
                        rows(ui);
                    }
                }
                ui.add_space(space::XS);
                ui.label(
                    egui::RichText::new("Their sidecar files are renamed to match.")
                        .size(text::LABEL)
                        .color(palette.tone(Tone::MUTED)),
                );
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui
                        .add_enabled_ui(can_rename, |ui| widgets::primary_button(ui, "Rename"))
                        .inner
                        .clicked()
                    {
                        rename = true;
                    }
                });
            });
        // Escape closes one window at a time: the editor keeps it, also on
        // the frame it opens, while the dialog is still the top one.
        let close = r.editor.is_none() && !edit && response.should_close();
        if cancel || close {
            self.file_ops.state = State::Idle;
            self.release_files();
            return;
        }
        let State::Renaming(r) = &mut self.file_ops.state else {
            return;
        };
        if edit {
            let template = r.settings.name_template();
            let tokens = r
                .settings
                .tokens
                .clone()
                .unwrap_or_else(|| template.naming.tokens().to_string());
            let preset = Preset::of(template.naming, r.settings.tokens.as_deref(), &r.saved);
            r.editor = Some(NameEditor::new(&tokens, preset));
        }
        self.rename_editor_window(ctx);
        let State::Renaming(r) = &mut self.file_ops.state else {
            return;
        };
        // Typed since Rename was asked: it waits for another.
        if r.submitted.as_ref().is_some_and(|s| *s != r.inputs()) {
            r.submitted = None;
        }
        // Rename is off while the files are read for the names, as in
        // Export: Return then does nothing.
        let reading = matches!(
            r.read_status(&r.settings.name_template()),
            Some(Status::Reading { .. })
        );
        if rename && !reading {
            // Return may come with the last of the text: what this frame
            // typed is what is renamed, once planned.
            r.submitted = Some(r.inputs());
        }
        // Asked for what is typed, and its plan is in.
        if r.submitted.is_some()
            && let Some(runs) = r.current().map(|p| p.plan.is_some())
        {
            r.submitted = None;
            // What can't be had keeps the dialog, which says why.
            if runs {
                return self.run_rename();
            }
        }
        self.plan_rename();
    }

    /// Rename Folder… for the folder of `root` at logical path `logical`:
    /// asks its new name.
    pub(in crate::app) fn rename_folder(&mut self, root: RootId, logical: String) {
        if self.activity.is_busy() || !matches!(self.file_ops.state, State::Idle) {
            self.messages.warn("Finish what is running first");
            return;
        }
        let Some(catalog) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone())
        else {
            return;
        };
        let old = logical.rsplit('/').next().unwrap_or_default().to_string();
        self.file_ops.state = State::RenamingFolder(Box::new(RenamingFolder {
            catalog,
            root,
            logical,
            name: old.clone(),
            old,
            focused: false,
        }));
    }

    /// Rename Folder…: its new name, checked as it is typed.
    pub(super) fn rename_folder_window(&mut self, ctx: &egui::Context) {
        let State::RenamingFolder(f) = &mut self.file_ops.state else {
            return;
        };
        let palette = crate::app::ui::theme::palette(ctx);
        let problem = file_ops::folder_name_problem(&f.name);
        let valid = problem.is_none() && f.name != f.old;
        let mut rename = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("rename-folder"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(420.);
                ui.label(
                    egui::RichText::new(format!("Rename Folder “{}”", f.old))
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::L);
                let field = egui::TextEdit::singleline(&mut f.name)
                    .desired_width(f32::INFINITY)
                    .show(ui);
                if !f.focused {
                    field.response.request_focus();
                    let mut state = field.state.clone();
                    let end = egui::text::CCursor::new(field.galley.text().chars().count());
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            end,
                        )));
                    state.store(ui.ctx(), field.response.id);
                    f.focused = true;
                }
                if field.response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && valid
                {
                    rename = true;
                }
                if let Some(problem) = problem.filter(|_| f.name != f.old) {
                    ui.add_space(space::XS);
                    ui.label(
                        egui::RichText::new(problem)
                            .size(text::LABEL)
                            .color(palette.warning()),
                    );
                }
                ui.add_space(space::XS);
                ui.label(
                    egui::RichText::new(
                        "The folder is renamed on disk; its photos and the folders in it follow.",
                    )
                    .size(text::LABEL)
                    .color(palette.tone(Tone::MUTED)),
                );
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui
                        .add_enabled_ui(valid, |ui| widgets::primary_button(ui, "Rename"))
                        .inner
                        .clicked()
                    {
                        rename = true;
                    }
                });
            });
        if cancel || response.should_close() {
            self.file_ops.state = State::Idle;
            return;
        }
        if rename {
            let State::RenamingFolder(f) = std::mem::take(&mut self.file_ops.state) else {
                return;
            };
            if !self.showing_catalog(&f.catalog) {
                return;
            }
            let parent = f
                .logical
                .rsplit_once('/')
                .map_or(String::new(), |(p, _)| p.to_string());
            let op = FileOp::RenameFolder {
                root: f.root,
                folder: f.logical.clone(),
                name: f.name.clone(),
            };
            let verb = Verb::RenameFolder {
                root: f.root,
                from: f.logical,
                to: if parent.is_empty() {
                    f.name.clone()
                } else {
                    format!("{parent}/{}", f.name)
                },
            };
            self.request_file_op(op, None, verb);
        }
    }
}

impl super::FileOps {
    /// The cancel flag of the order being read, while it is.
    #[cfg(test)]
    pub(super) fn ordering_cancel(&self) -> Option<Arc<AtomicBool>> {
        match &self.state {
            State::Ordering { cancel, .. } => Some(cancel.clone()),
            _ => None,
        }
    }
    /// Whether the open rename dialog's names are being planned.
    #[cfg(test)]
    pub(super) fn rename_planning(&self) -> bool {
        self.rename_request().is_some()
    }
    /// The plan of the open rename dialog's names under way.
    #[cfg(test)]
    pub(super) fn rename_request(&self) -> Option<u64> {
        match &self.state {
            State::Renaming(r) => r.planning.as_ref().map(|(request, _)| *request),
            _ => None,
        }
    }
    /// Whether the open rename dialog is reading its photos' files.
    #[cfg(test)]
    pub(super) fn rename_reading(&self) -> bool {
        matches!(&self.state, State::Renaming(r) if matches!(r.read.status(), Status::Reading { .. }))
    }
    /// Opens the Filename Template Editor over the rename dialog with
    /// `tokens` (Lightroom's form), as File Naming > Edit… would.
    #[cfg(test)]
    pub(super) fn rename_edit(&mut self, tokens: &str) {
        if let State::Renaming(r) = &mut self.state {
            let preset = Preset::of(r.settings.naming, Some(tokens), &r.saved);
            r.editor = Some(NameEditor::new(tokens, preset));
        }
    }
    /// Picks `tokens` (Lightroom's form) in the rename dialog's File Naming.
    #[cfg(test)]
    pub(super) fn rename_choose(&mut self, tokens: &str) {
        if let State::Renaming(r) = &mut self.state {
            r.settings.choose_tokens(tokens);
        }
    }
    /// Types `text` in the rename dialog's Custom Text.
    #[cfg(test)]
    pub(super) fn rename_custom_text(&mut self, text: &str) {
        if let State::Renaming(r) = &mut self.state {
            r.settings.custom_text = text.into();
        }
    }
    #[cfg(test)]
    pub(super) fn rename_editor_open(&self) -> bool {
        matches!(&self.state, State::Renaming(r) if r.editor.is_some())
    }
    /// The open rename dialog, as it shows once what is typed is planned.
    #[cfg(test)]
    pub(super) fn rename_shown(&self) -> Option<RenameShown> {
        let State::Renaming(r) = &self.state else {
            return None;
        };
        let planned = r.current()?;
        Some(RenameShown {
            title: r.title(),
            rows: planned
                .rows
                .iter()
                .map(|row| (row.old.clone(), row.new.clone(), row.problem.clone()))
                .collect(),
            can_rename: planned.plan.is_some(),
        })
    }
}
