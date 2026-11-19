//! Import Photos… (#806) in the app: copying photos from a card or any
//! folder into the library, as Lightroom Classic's Import (Copy) does.
//! `catalog::file_ops::import` owns the plan, the run and its recovery;
//! this is the dialog, its reads of the source ([`scan`]) and the run's
//! task.
//!
//! The dialog holds nothing while it is open. Its source is scanned on a
//! worker (a newer scan, or closing, cancels it, and an older scan's
//! results are dropped by their generation), then what it would do is
//! planned on a worker as its choices change, one plan at a time, the
//! latest input next, as Rename Photos plans. Import runs the plan shown
//! for exactly what is chosen: the open edit is saved first (a failed save
//! stops here, holding nothing), then `Activity::ChangingFiles` is taken for
//! the whole run, which waits navigation and every other file operation.
//! The run plans again under the operation lock: should that plan differ
//! (another import took its Import #, a name was taken meanwhile), nothing
//! is done and the dialog comes back with the new names to confirm. When
//! the run ends, fails, panics or is stopped, the catalog is let go and read
//! again, and Previous Import shows what it added.
//!
//! The choices are kept for the next import (`import-copy.json`) once
//! Import is asked for.
use super::State;
use crate::app::Editor;
use crate::app::export::naming::editor::{NameEditor, Outcome as EditorOutcome};
use crate::app::export::naming::menu::{self, Choice as NamingChoice};
use crate::app::export::naming::presets::{Library, Preset};
use crate::app::ui::design::{self, Tone, button, inset, radius, space, text};
use crate::app::ui::widgets::{self, plural, pretty_path};
use crate::catalog::file_ops::{
    self, DateFolders, Duplicate, ExtensionCase, ImportMetadata, ImportOutcome, ImportPlan,
    ImportRequest, ImportSource, Organize,
};
use crate::catalog::{CatalogLocation, Choice, NewPhotoDefaults};
use crate::export_settings::{Decimal, NameContext, NameTemplate};
use eframe::egui::{self, Vec2};
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod scan;
mod settings;
pub(in crate::app) use scan::FileReader;
pub(crate) use scan::{Scanned, Thumbnail};
pub(in crate::app) use settings::ImportCopySettings;

/// Tells each dialog, scan, plan and run of Import Photos from every other.
static TASKS: AtomicU64 = AtomicU64::new(1);
fn next_task() -> u64 {
    TASKS.fetch_add(1, Ordering::Relaxed)
}

/// An attached drive offered as a source; `card` is its `DCIM` folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Drive {
    pub(crate) name: String,
    pub(crate) mount: PathBuf,
    pub(crate) card: Option<PathBuf>,
}

/// One photo the scan found.
struct Row {
    source: ImportSource,
    duplicate: Option<Duplicate>,
    checked: bool,
    thumbnail: Option<egui::TextureHandle>,
}

/// Where the source's scan is.
enum Scan {
    /// No source yet.
    None,
    Reading {
        cancel: Arc<AtomicBool>,
        progress: scan::Progress,
    },
    Done,
    Failed(String),
}

/// A folder chooser open for the source or the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    Source,
    Destination,
}

/// Import Photos…, open. Holds nothing.
pub(super) struct Importing {
    id: u64,
    catalog: CatalogLocation,
    settings: ImportCopySettings,
    /// Preferences' defaults for photos added, read as the dialog opened.
    defaults: NewPhotoDefaults,
    /// The region's decimal separator, read as the dialog opened.
    decimal: Decimal,
    /// The attached drives, once listed.
    drives: Option<Vec<Drive>>,
    /// The source scanned: `settings.source` once chosen.
    generation: u64,
    scan: Scan,
    rows: Vec<Row>,
    /// Files and folders the scan couldn't read, with why.
    unreadable: Vec<(PathBuf, String)>,
    /// Stops the thumbnails being read.
    thumbnails: Arc<AtomicBool>,
    reader: FileReader,
    /// The Filename Template Editor, over the dialog.
    editor: Option<NameEditor>,
    saved: Library,
    /// Answers to where the destination is in the catalog.
    choices: Vec<Choice>,
    planned: Option<Planned>,
    planning: Option<(u64, Inputs)>,
    /// The catalog plans read, between plans.
    catalog_reader: Option<crate::catalog::Catalog>,
    /// The run found a plan other than the one shown, which is shown now.
    changed: bool,
    picked: Arc<Mutex<Option<(Pick, PathBuf)>>>,
    picking: Arc<AtomicBool>,
    /// The source is the folder last imported from, not one chosen here: a
    /// card found among the drives takes its place.
    remembered_source: bool,
}

/// What a plan is made from: the scan, the photos checked in it (by their
/// place in it), and where and how they go.
#[derive(Clone, Debug, PartialEq)]
struct Inputs {
    scan: u64,
    source: PathBuf,
    checked: Vec<usize>,
    destination: PathBuf,
    subfolder: String,
    organize: Organize,
    naming: Option<NameTemplate>,
    shoot_name: String,
    extensions: ExtensionCase,
    apply: ImportMetadata,
    skip_duplicates: bool,
    choices: Vec<Choice>,
}

struct Planned {
    inputs: Inputs,
    plan: Result<ImportPlan, String>,
}

/// A plan of the dialog's choices, back from its worker.
pub(crate) struct ImportPlanned {
    dialog: u64,
    request: u64,
    reader: Option<crate::catalog::Catalog>,
    plan: Result<ImportPlan, String>,
}

/// Import Photos running, with its dialog for a plan to confirm again.
pub(super) struct RunningImport {
    dialog: Box<Importing>,
    inputs: Inputs,
    run: u64,
    cancel: Arc<AtomicBool>,
    photos: usize,
}

impl Row {
    /// Whether it can be imported: not a suspected duplicate while Don't
    /// Import Suspected Duplicates is on (Lightroom greys those out).
    fn importable(&self, skip_duplicates: bool) -> bool {
        !(skip_duplicates && self.duplicate.is_some())
    }
}

impl Importing {
    /// What is chosen now, when there is anything to plan: a scan read,
    /// a destination, a photo checked that can be imported, keywords that
    /// read.
    fn inputs(&self) -> Option<Inputs> {
        if !matches!(self.scan, Scan::Done) {
            return None;
        }
        let s = &self.settings;
        let checked: Vec<usize> = (self.rows.iter().enumerate())
            .filter(|(_, r)| r.checked && r.importable(s.skip_duplicates))
            .map(|(i, _)| i)
            .collect();
        if checked.is_empty() {
            return None;
        }
        let keywords = crate::app::library::parse_keywords(&s.keywords).ok()?;
        Some(Inputs {
            scan: self.generation,
            source: s.source.clone()?,
            checked,
            destination: s.destination.clone()?,
            subfolder: match s.into_subfolder {
                true => s.subfolder.trim().to_string(),
                false => String::new(),
            },
            organize: s.organize.clone(),
            naming: s.rename.then(|| s.naming.name_template()),
            shoot_name: s.shoot_name.trim().to_string(),
            extensions: s.extensions,
            apply: ImportMetadata {
                creator: s.creator.trim().to_string(),
                copyright: s.copyright.trim().to_string(),
                keywords,
            },
            skip_duplicates: s.skip_duplicates,
            choices: self.choices.clone(),
        })
    }

    /// The engine's request for `inputs`: the photos checked.
    fn request(&self, inputs: &Inputs) -> ImportRequest {
        ImportRequest {
            sources: (inputs.checked.iter())
                .map(|&i| self.rows[i].source.clone())
                .collect(),
            source: inputs.source.clone(),
            destination: inputs.destination.clone(),
            subfolder: inputs.subfolder.clone(),
            organize: inputs.organize.clone(),
            naming: inputs.naming.clone(),
            shoot_name: inputs.shoot_name.clone(),
            extensions: inputs.extensions,
            choices: inputs.choices.clone(),
            defaults: self.defaults.clone(),
            apply: inputs.apply.clone(),
            skip_duplicates: inputs.skip_duplicates,
        }
    }

    /// Marks the photos `plan` takes for suspected duplicates that the scan
    /// didn't (the catalog gained them since, by another import): greyed
    /// out as the scan's are while Don't Import Suspected Duplicates is on.
    fn mark_duplicates(&mut self, plan: &ImportPlan) {
        for photo in &plan.photos {
            if let Some(found) = photo.duplicate
                && let Some(row) = self
                    .rows
                    .iter_mut()
                    .find(|r| r.duplicate.is_none() && r.source.file.path == photo.source)
            {
                row.duplicate = Some(found);
            }
        }
    }

    /// The plan of what is chosen now, once it is in.
    fn current(&self) -> Option<&Planned> {
        let inputs = self.inputs()?;
        self.planned.as_ref().filter(|p| p.inputs == inputs)
    }

    /// The plan Import would run: in, for what is chosen, and runnable.
    fn runnable(&self) -> Option<&ImportPlan> {
        self.current()
            .and_then(|p| p.plan.as_ref().ok())
            .filter(|p| p.can_run())
    }

    /// Scans `settings.source` (cancelling a scan under way), forgetting
    /// what the last one found.
    fn rescan(
        &mut self,
        events: &std::sync::mpsc::Sender<crate::app::worker::Event>,
        ctx: &egui::Context,
    ) {
        self.stop_reads();
        self.rows.clear();
        self.unreadable.clear();
        self.planned = None;
        self.changed = false;
        self.choices.clear();
        self.generation = next_task();
        let Some(folder) = self.settings.source.clone() else {
            self.scan = Scan::None;
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = scan::Progress::default();
        self.scan = Scan::Reading {
            cancel: cancel.clone(),
            progress: progress.clone(),
        };
        scan::start(
            scan::Source {
                generation: self.generation,
                folder,
                subfolders: self.settings.subfolders,
                catalog: self.catalog.clone(),
                reader: self.reader.clone(),
                cancel,
                progress,
            },
            events.clone(),
            ctx.clone(),
        );
    }

    /// Reads the thumbnails the rows lack.
    fn read_thumbnails(
        &mut self,
        events: &std::sync::mpsc::Sender<crate::app::worker::Event>,
        ctx: &egui::Context,
    ) {
        self.thumbnails.store(true, Ordering::Relaxed);
        self.thumbnails = Arc::new(AtomicBool::new(false));
        let missing: Vec<(usize, PathBuf)> = (self.rows.iter().enumerate())
            .filter(|(_, r)| r.thumbnail.is_none())
            .map(|(i, r)| (i, r.source.file.path.clone()))
            .collect();
        if !missing.is_empty() {
            scan::thumbnails(
                self.generation,
                missing,
                self.thumbnails.clone(),
                events.clone(),
                ctx.clone(),
            );
        }
    }

    /// Stops the scan and the thumbnails being read.
    fn stop_reads(&mut self) {
        if let Scan::Reading { cancel, .. } = &self.scan {
            cancel.store(true, Ordering::Relaxed);
        }
        self.thumbnails.store(true, Ordering::Relaxed);
    }

    /// The checked photos' count and bytes, companions included.
    fn checked(&self) -> (usize, u64) {
        let skip = self.settings.skip_duplicates;
        let rows = (self.rows.iter()).filter(|r| r.checked && r.importable(skip));
        rows.fold((0, 0), |(n, bytes), r| {
            let size = r.source.file.identity.size
                + r.source
                    .companions
                    .iter()
                    .map(|c| c.identity.size)
                    .sum::<u64>();
            (n + 1, bytes + size)
        })
    }

    /// The first checked photo named by `template`, for the editor's
    /// Example, with Import # as the plan has it.
    fn example(&self, template: &NameTemplate) -> (String, Vec<String>) {
        let Some(row) = self.rows.iter().find(|r| r.checked).or(self.rows.first()) else {
            return Default::default();
        };
        let path = &row.source.file.path;
        let number = self
            .planned
            .as_ref()
            .and_then(|p| p.plan.as_ref().ok())
            .map_or(1, |p| p.number);
        let fields = crate::export_settings::NameFields {
            captured: row.source.date().map(str::to_string),
            original_name: path.file_name().map(|n| n.to_string_lossy().into_owned()),
            import_number: Some(number),
            shoot_name: Some(self.settings.shoot_name.trim().to_string()),
            ..row.source.fields.clone()
        };
        // As the plan counts them: the photos imported.
        let total = self.checked().0.max(1);
        let at = NameContext {
            index: 0,
            total,
            fields: &fields,
        };
        let (stem, notes) = template.stem(path, &at);
        let name = match path.extension() {
            Some(extension) => {
                let extension = extension.to_string_lossy();
                let extension = match self.settings.extensions {
                    ExtensionCase::LeaveAsIs => extension.into_owned(),
                    ExtensionCase::Lowercase => extension.to_lowercase(),
                    ExtensionCase::Uppercase => extension.to_uppercase(),
                };
                format!("{stem}.{extension}")
            }
            None => stem,
        };
        (name, notes)
    }

    /// File Renaming's Sample: the first photo checked (else the first)
    /// named as Import would name it; `None` without photos.
    fn rename_sample(&self) -> Option<(String, Vec<String>)> {
        (!self.rows.is_empty()).then(|| self.example(&self.settings.naming.name_template()))
    }

    /// Opens the system folder chooser on its own thread, so the dialog
    /// keeps drawing.
    fn choose_folder(&self, pick: Pick, ctx: &egui::Context) {
        if self.picking.swap(true, Ordering::Relaxed) {
            return;
        }
        let (picked, picking, ctx) = (self.picked.clone(), self.picking.clone(), ctx.clone());
        let start = match pick {
            Pick::Source => self.settings.source.clone(),
            Pick::Destination => self.settings.destination.clone(),
        };
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title(match pick {
                Pick::Source => "Import photos from",
                Pick::Destination => "Import photos into",
            });
            if let Some(start) = start {
                dialog = dialog.set_directory(start);
            }
            if let Some(folder) = dialog.pick_folder()
                && let Ok(mut slot) = picked.lock()
            {
                *slot = Some((pick, folder));
            }
            picking.store(false, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }
}

impl Drop for Importing {
    /// Reads nobody can take any more stop.
    fn drop(&mut self) {
        self.stop_reads();
    }
}

impl Editor {
    /// Library ▸ Import Photos… (⌘⇧I): opens the dialog for the catalog
    /// shown. Its drives are listed on a worker, and a camera's card among
    /// them, else the folder last imported from, is scanned.
    pub(in crate::app) fn request_import(&mut self) {
        if self.activity.is_busy() || !matches!(self.file_ops.state, State::Idle) {
            self.messages.warn("Finish what is running first");
            return;
        }
        let Some(catalog) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone())
        else {
            self.messages
                .warn("Open or create a catalog to import photos");
            return;
        };
        let settings = self.remembered_import();
        let mut dialog = Importing {
            id: next_task(),
            catalog,
            settings,
            defaults: self.import_defaults(),
            decimal: Decimal(crate::platform::locale::decimal_separator()),
            drives: None,
            generation: next_task(),
            scan: Scan::None,
            rows: Vec::new(),
            unreadable: Vec::new(),
            thumbnails: Arc::new(AtomicBool::new(false)),
            reader: self.import_reader(),
            editor: None,
            saved: Library::load(),
            choices: Vec::new(),
            planned: None,
            planning: None,
            catalog_reader: None,
            changed: false,
            picked: Default::default(),
            picking: Default::default(),
            remembered_source: true,
        };
        // The folder last imported from is scanned at once: listing the
        // drives can wait on a stalled network mount.
        if dialog.settings.source.is_some() {
            dialog.rescan(&self.tx, &self.context);
        }
        // Tests choose their sources: this computer's drives are no part of them.
        #[cfg(test)]
        let attached = Vec::new;
        #[cfg(not(test))]
        let attached = crate::platform::volume::attached;
        scan::list_volumes(dialog.id, attached, self.tx.clone(), self.context.clone());
        self.file_ops.state = State::Importing(Box::new(dialog));
    }

    /// The attached drives were listed: a camera's card among them is the
    /// source, unless one was chosen meanwhile, in place of the folder last
    /// imported from.
    pub(in crate::app) fn import_drives(&mut self, dialog: u64, drives: Vec<Drive>) {
        let (events, ctx) = (self.tx.clone(), self.context.clone());
        let State::Importing(d) = &mut self.file_ops.state else {
            return;
        };
        if d.id != dialog || d.drives.is_some() {
            return;
        }
        let card = drives.iter().find_map(|d| d.card.clone());
        d.drives = Some(drives);
        if let Some(card) = card
            && d.remembered_source
            && d.settings.source.as_ref() != Some(&card)
        {
            d.settings.source = Some(card);
            d.rescan(&events, &ctx);
        }
    }

    /// The source was scanned: its photos are listed, checked, the suspected
    /// duplicates marked, and their thumbnails read. A scan of an earlier
    /// source is dropped.
    pub(in crate::app) fn import_scanned(&mut self, scanned: Scanned) {
        let (events, ctx) = (self.tx.clone(), self.context.clone());
        let State::Importing(d) = &mut self.file_ops.state else {
            return;
        };
        if scanned.generation != d.generation || !matches!(d.scan, Scan::Reading { .. }) {
            return;
        }
        match scanned.result {
            Ok((found, duplicates)) => {
                let mut photos = 0;
                for f in found {
                    match f {
                        scan::Found::Photo(mut source) => {
                            // The region's number format, as Rename Photos
                            // names with it.
                            source.fields.decimal = d.decimal;
                            let duplicate = duplicates.get(photos).copied().flatten();
                            photos += 1;
                            d.rows.push(Row {
                                source: *source,
                                duplicate,
                                checked: true,
                                thumbnail: None,
                            });
                        }
                        scan::Found::Unreadable { path, why } => d.unreadable.push((path, why)),
                    }
                }
                d.scan = Scan::Done;
                d.read_thumbnails(&events, &ctx);
            }
            Err(why) => d.scan = Scan::Failed(why),
        }
        self.plan_import();
    }

    /// A thumbnail came in for the scan shown.
    pub(in crate::app) fn import_thumbnail(&mut self, thumbnail: Thumbnail) {
        let ctx = self.context.clone();
        let State::Importing(d) = &mut self.file_ops.state else {
            return;
        };
        if thumbnail.generation != d.generation {
            return;
        }
        if let (Some(row), Some(image)) = (d.rows.get_mut(thumbnail.index), thumbnail.image) {
            let name = format!("import-thumbnail-{}-{}", d.generation, thumbnail.index);
            row.thumbnail = Some(ctx.load_texture(name, image, egui::TextureOptions::LINEAR));
        }
    }

    /// Plans what the dialog's choices are now on a worker, unless that is
    /// planned or another plan is under way: its choices are planned once
    /// that one is in.
    fn plan_import(&mut self) {
        let State::Importing(d) = &mut self.file_ops.state else {
            return;
        };
        let Some(inputs) = d.inputs() else {
            return;
        };
        if d.planning.is_some() || d.current().is_some() {
            return;
        }
        let request = next_task();
        d.planning = Some((request, inputs.clone()));
        #[cfg(test)]
        if self.file_ops.hold_plans {
            return;
        }
        let (dialog, location) = (d.id, d.catalog.clone());
        let import = d.request(&inputs);
        let reader = d.catalog_reader.take();
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let catalog = match reader {
                    Some(catalog) => Ok(catalog),
                    None => crate::catalog::Catalog::open(&location).map_err(|e| format!("{e:#}")),
                };
                let done = match catalog {
                    Ok(catalog) => ImportPlanned {
                        dialog,
                        request,
                        plan: file_ops::plan_import(&catalog, &import).map_err(|e| e.to_string()),
                        reader: Some(catalog),
                    },
                    Err(e) => ImportPlanned {
                        dialog,
                        request,
                        plan: Err(e),
                        reader: None,
                    },
                };
                let _ = tx.send(crate::app::worker::Event::ImportPlanned(Box::new(done)));
            },
            move |tx, message| {
                let failed = ImportPlanned {
                    dialog,
                    request,
                    plan: Err(message),
                    reader: None,
                };
                let _ = tx.send(crate::app::worker::Event::ImportPlanned(Box::new(failed)));
            },
        );
    }

    /// A plan of the dialog's choices came back: it is shown, unless the
    /// dialog it was for is gone; what was chosen meanwhile is planned next.
    pub(in crate::app) fn import_planned(&mut self, done: ImportPlanned) {
        let State::Importing(d) = &mut self.file_ops.state else {
            return;
        };
        if d.id != done.dialog {
            return;
        }
        let Some((_, inputs)) = d.planning.take_if(|(request, _)| *request == done.request) else {
            return;
        };
        d.catalog_reader = done.reader;
        // A plan of an earlier scan says nothing of this one's photos.
        if let Ok(plan) = &done.plan
            && inputs.scan == d.generation
        {
            d.mark_duplicates(plan);
        }
        d.planned = Some(Planned {
            inputs,
            plan: done.plan,
        });
        self.plan_import();
    }

    /// Import: runs the plan shown for what is chosen, once the open edit is
    /// saved, holding the catalog until the run is over.
    fn run_import(&mut self) {
        let State::Importing(mut d) = std::mem::take(&mut self.file_ops.state) else {
            return;
        };
        // A plan in flight comes back while the run holds the dialog and is
        // dropped: forgotten now, or a dialog shown again would wait on it.
        d.planning = None;
        let (Some(inputs), Some(plan)) = (d.inputs(), d.runnable().cloned()) else {
            self.file_ops.state = State::Importing(d);
            return;
        };
        if !self.showing_catalog(&d.catalog) {
            return;
        }
        if self.activity.is_busy() {
            self.messages.warn("Finish what is running first");
            self.file_ops.state = State::Importing(d);
            return;
        }
        // Saved first, with nothing processed in between: a failed save
        // stops here, and the dialog stays.
        if !self.flush() {
            self.messages
                .fail("The open photo's edit could not be saved; nothing was imported");
            self.file_ops.state = State::Importing(d);
            return;
        }
        if !self.activity.begin_file_change() {
            self.file_ops.state = State::Importing(d);
            return;
        }
        if let Some(library) = &mut self.library {
            library.suspend_background_reads();
        }
        if let Err(e) = self.remember_import(&d.settings) {
            self.messages.warn(format!(
                "Import Photos' choices not kept for next time: {e}"
            ));
        }
        // The card is read for the copies alone now.
        d.thumbnails.store(true, Ordering::Relaxed);
        let request = d.request(&inputs);
        let run = next_task();
        #[cfg(test)]
        let cancel = Arc::new(AtomicBool::new(self.file_ops.stop_first));
        #[cfg(not(test))]
        let cancel: Arc<AtomicBool> = Default::default();
        let photos = plan
            .photos
            .iter()
            .filter(|p| p.destination.is_some())
            .count();
        let location = d.catalog.clone();
        self.file_ops.state = State::ImportRunning(Box::new(RunningImport {
            dialog: d,
            inputs,
            run,
            cancel: cancel.clone(),
            photos,
        }));
        let trash = self.file_ops.trash.clone();
        #[cfg(test)]
        let panics = self.file_ops.import_panics;
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                #[cfg(test)]
                assert!(!panics, "the import's worker panicked");
                let done = crate::catalog::Catalog::open(&location)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|mut catalog| {
                        file_ops::execute_import(
                            &mut catalog,
                            &request,
                            &plan,
                            &*trash,
                            Cancel::new(&cancel),
                        )
                        .map_err(|e| e.to_string())
                    });
                let _ = tx.send(crate::app::worker::Event::ImportDone(run, Box::new(done)));
            },
            move |tx, message| {
                let _ = tx.send(crate::app::worker::Event::ImportDone(
                    run,
                    Box::new(Err(message)),
                ));
            },
        );
    }

    /// The run ended, failed or its thread stopped: the catalog is let go,
    /// read again and Previous Import shown, with what was left and why; or
    /// the plan made under the lock differed, and the dialog shows it.
    pub(in crate::app) fn import_done(&mut self, run: u64, done: Result<ImportOutcome, String>) {
        let State::ImportRunning(running) = std::mem::take(&mut self.file_ops.state) else {
            return;
        };
        if running.run != run {
            self.file_ops.state = State::ImportRunning(running);
            return;
        }
        let RunningImport {
            mut dialog, inputs, ..
        } = *running;
        if !self.showing_catalog(&dialog.catalog) {
            self.release_files();
            return;
        }
        let (imported, skipped, stopped) = match done {
            Ok(ImportOutcome::Changed(plan)) => {
                self.release_files();
                self.messages.warn(
                    "The names changed since they were shown (another import, or a file \
                     added there); check them and Import again",
                );
                dialog.mark_duplicates(&plan);
                dialog.planned = Some(Planned {
                    inputs,
                    plan: Ok(*plan),
                });
                dialog.changed = true;
                dialog.read_thumbnails(&self.tx, &self.context);
                self.file_ops.state = State::Importing(dialog);
                return;
            }
            Ok(ImportOutcome::Ran {
                imported,
                skipped,
                outcome,
                ..
            }) => (imported.len(), skipped, outcome.stopped),
            // Begun, its result unknown: the catalog read again says what
            // it added.
            Ok(ImportOutcome::Unread { error, .. }) => {
                drop(dialog);
                self.messages.fail(format!(
                    "The import stopped, and what it added couldn't be read: {error}. \
                     Check Previous Import; the catalog finishes the rest when it next \
                     changes files."
                ));
                self.reread_after_files(&[], &[]);
                if let Some(library) = &mut self.library {
                    library.show_previous_import();
                }
                self.release_files();
                return;
            }
            Err(e) => (0, Vec::new(), Some(e)),
        };
        drop(dialog);
        let destination = inputs.destination.file_name().map_or_else(
            || pretty_path(&inputs.destination),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut text = format!(
            "Imported {} into {destination}",
            plural(imported, "photo", "photos")
        );
        if let Some((first, why)) = skipped.first() {
            let name = first.file_name().unwrap_or_default().to_string_lossy();
            text.push_str(&format!(
                " · {} not imported: {name}, {why}",
                plural(skipped.len(), "photo", "photos")
            ));
        }
        if let Some(stopped) = &stopped {
            text.push_str(&format!(" · stopped: {stopped}"));
        }
        let detail = skipped
            .iter()
            .map(|(path, why)| format!("Not imported {}: {why}", path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        if stopped.is_some() || (imported == 0 && !skipped.is_empty()) {
            self.messages.fail_with_detail(text, detail);
        } else if !skipped.is_empty() {
            self.messages.warn_with_detail(text, detail);
        } else {
            self.messages.done(text);
        }
        // Whatever the run said, the catalog says what it added.
        self.reread_after_files(&[], &[]);
        if imported > 0
            && let Some(library) = &mut self.library
        {
            library.show_previous_import();
        }
        self.release_files();
    }

    /// The Filename Template Editor over Import Photos, offering Import #
    /// too; Done makes its template the dialog's.
    fn import_editor_window(&mut self, ctx: &egui::Context) {
        let State::Importing(d) = &mut self.file_ops.state else {
            return;
        };
        let Some(parsed) = d.editor.as_ref().map(NameEditor::parsed) else {
            return;
        };
        let example = match &parsed {
            Ok(template) => {
                let mut naming = d.settings.naming.clone();
                naming.choose_tokens(&template.to_lightroom());
                d.example(&naming.name_template())
            }
            Err(_) => Default::default(),
        };
        let Some(editor) = d.editor.as_mut() else {
            return;
        };
        match editor.show(ctx, &example, &mut d.saved) {
            EditorOutcome::Open => {}
            EditorOutcome::Done(template) => {
                d.settings.naming.choose_tokens(&template.to_lightroom());
                d.settings.rename = true;
                d.editor = None;
            }
            EditorOutcome::Cancel => d.editor = None,
        }
    }

    /// Import Photos: the source and its photos, where and how they go,
    /// what that comes to, and Import.
    pub(super) fn import_window(&mut self, ctx: &egui::Context) {
        let (events, context) = (self.tx.clone(), self.context.clone());
        let (State::Importing(d), Some(library)) = (&mut self.file_ops.state, &self.library) else {
            return;
        };
        if library.session.catalog().location() != &d.catalog {
            self.file_ops.state = State::Idle;
            return;
        }
        // A folder chosen in the system chooser.
        let picked = d.picked.lock().ok().and_then(|mut p| p.take());
        let mut rescan = false;
        match picked {
            Some((Pick::Source, folder)) => {
                d.settings.source = Some(folder);
                d.remembered_source = false;
                rescan = true;
            }
            Some((Pick::Destination, folder)) => {
                d.settings.destination = Some(folder);
                d.choices.clear();
                d.changed = false;
            }
            None => {}
        }
        let palette = crate::app::ui::theme::palette(ctx);
        let mut shown = Shown::default();
        let response = egui::Modal::new(egui::Id::new("import-photos"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| draw(ui, d, &palette, &mut shown));
        if shown.choose_source {
            d.choose_folder(Pick::Source, ctx);
        }
        if shown.choose_destination {
            d.choose_folder(Pick::Destination, ctx);
        }
        if let Some(source) = shown.source.take() {
            d.settings.source = Some(source);
            d.remembered_source = false;
            rescan = true;
        }
        if shown.subfolders_changed {
            rescan = true;
        }
        if rescan {
            d.rescan(&events, &context);
        }
        if shown.edit {
            let template = d.settings.naming.name_template();
            let tokens = (d.settings.naming.tokens.clone())
                .unwrap_or_else(|| template.naming.tokens().to_string());
            let preset = Preset::of(
                template.naming,
                d.settings.naming.tokens.as_deref(),
                &d.saved,
            );
            d.editor = Some(NameEditor::new(&tokens, preset).with_import_tokens());
        }
        if shown.changed_choice {
            d.changed = false;
        }
        // Escape closes one window at a time: the editor keeps it.
        let close = d.editor.is_none() && !shown.edit && response.should_close();
        if shown.cancel || close {
            self.file_ops.state = State::Idle;
            return;
        }
        self.import_editor_window(ctx);
        if shown.import {
            self.run_import();
            return;
        }
        self.plan_import();
    }
}

/// What a frame of the dialog asked for.
#[derive(Default)]
struct Shown {
    choose_source: bool,
    choose_destination: bool,
    /// An attached drive picked as the source.
    source: Option<PathBuf>,
    subfolders_changed: bool,
    edit: bool,
    /// A choice changed that the plan shown since the run doesn't follow.
    changed_choice: bool,
    import: bool,
    cancel: bool,
}

/// A section's heading.
fn heading(ui: &mut egui::Ui, palette: &crate::app::ui::theme::Palette, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(text::LABEL)
            .strong()
            .color(palette.tone(Tone::SOFT)),
    );
}

/// A line of small text in `tone`.
fn note(ui: &mut egui::Ui, text: impl Into<String>, color: egui::Color32) -> egui::Response {
    ui.add(egui::Label::new(egui::RichText::new(text).size(text::LABEL).color(color)).wrap())
}

/// The dialog's contents.
fn draw(
    ui: &mut egui::Ui,
    d: &mut Importing,
    palette: &crate::app::ui::theme::Palette,
    shown: &mut Shown,
) {
    ui.set_width(1000.);
    ui.label(
        egui::RichText::new("Import Photos")
            .size(text::TITLE)
            .color(palette.tone(Tone::STRONG)),
    );
    ui.add_space(space::L);
    // The photos on the left, Lightroom's panels on the right.
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(560.);
            source_section(ui, d, palette, shown);
            ui.add_space(space::M);
            photos_section(ui, d, palette);
            ui.add_space(space::M);
            preview_section(ui, d, palette, shown);
        });
        // A gap, not a separator: a vertical one takes all the height a
        // modal offers, and the modal grows with it.
        ui.add_space(space::XL);
        ui.vertical(|ui| {
            ui.set_width(410.);
            egui::ScrollArea::vertical()
                .id_salt("import-settings")
                .max_height(520.)
                .show(ui, |ui| settings_section(ui, d, palette, shown));
        });
    });
    ui.add_space(space::L);
    footer(ui, d, palette, shown);
}

fn source_section(
    ui: &mut egui::Ui,
    d: &mut Importing,
    palette: &crate::app::ui::theme::Palette,
    shown: &mut Shown,
) {
    heading(ui, palette, "From");
    ui.horizontal(|ui| {
        let current = d
            .settings
            .source
            .as_deref()
            .map_or_else(|| "Choose a source".to_string(), pretty_path);
        egui::ComboBox::from_id_salt("import-source")
            .width(420.)
            .selected_text(current)
            .show_ui(ui, |ui| {
                match &d.drives {
                    None => {
                        ui.label("Looking for cards and drives…");
                    }
                    Some(drives) if drives.is_empty() => {
                        ui.label("No card or drive attached");
                    }
                    Some(drives) => {
                        for drive in drives {
                            let folder = drive.card.clone().unwrap_or(drive.mount.clone());
                            let on = d.settings.source.as_ref() == Some(&folder);
                            if ui.selectable_label(on, &drive.name).clicked() {
                                shown.source = Some(folder);
                            }
                        }
                    }
                }
                ui.separator();
                if ui.selectable_label(false, "Other Folder…").clicked() {
                    shown.choose_source = true;
                }
            });
        if ui.button("Choose…").clicked() {
            shown.choose_source = true;
        }
        if ui
            .checkbox(&mut d.settings.subfolders, "Include Subfolders")
            .changed()
        {
            shown.subfolders_changed = true;
        }
    });
}

fn photos_section(ui: &mut egui::Ui, d: &mut Importing, palette: &crate::app::ui::theme::Palette) {
    match &d.scan {
        Scan::None => {
            note(
                ui,
                "Choose a card or folder to import from.",
                palette.tone(Tone::MUTED),
            );
            return;
        }
        Scan::Reading { progress, .. } => {
            let (read, found) = (
                progress.read.load(Ordering::Relaxed),
                progress.found.load(Ordering::Relaxed),
            );
            let text = if found == 0 {
                "Looking for photos…".to_string()
            } else {
                format!("Reading {} of {found} photos…", (read + 1).min(found))
            };
            note(ui, text, palette.tone(Tone::MUTED));
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        Scan::Failed(why) => {
            note(ui, why.clone(), palette.warning());
            return;
        }
        Scan::Done => {}
    }
    let (count, bytes) = d.checked();
    ui.horizontal(|ui| {
        note(
            ui,
            format!(
                "{} of {} checked · {}",
                count,
                plural(d.rows.len(), "photo", "photos"),
                super::size(bytes)
            ),
            palette.tone(Tone::LABEL),
        );
        if ui.small_button("Check All").clicked() {
            d.rows.iter_mut().for_each(|r| r.checked = true);
        }
        if ui.small_button("Uncheck All").clicked() {
            d.rows.iter_mut().for_each(|r| r.checked = false);
        }
    });
    if d.rows.is_empty() {
        note(ui, "No photos here.", palette.tone(Tone::MUTED));
    }
    egui::ScrollArea::vertical()
        .id_salt("import-grid")
        .max_height(250.)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let skip = d.settings.skip_duplicates;
                for row in &mut d.rows {
                    cell(ui, palette, row, skip);
                }
            });
            for (path, why) in &d.unreadable {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                note(ui, format!("{name}: {why}"), palette.warning());
            }
        });
}

/// A photo's thumbnail and checkbox, with what marks it: a suspected
/// duplicate is greyed out, with no checkbox, while `skip_duplicates`, as in
/// Lightroom.
fn cell(
    ui: &mut egui::Ui,
    palette: &crate::app::ui::theme::Palette,
    row: &mut Row,
    skip_duplicates: bool,
) {
    const EDGE: f32 = 104.;
    ui.allocate_ui(Vec2::new(EDGE, EDGE + 52.), |ui| {
        ui.set_width(EDGE);
        ui.vertical(|ui| {
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(EDGE), egui::Sense::click());
            ui.painter()
                .rect_filled(rect, radius::CONTROL, palette.tone(Tone::WELL));
            let importable = row.importable(skip_duplicates);
            let dimmed = !importable || !row.checked;
            if let Some(texture) = &row.thumbnail {
                let size = texture.size_vec2();
                let scale = (EDGE / size.x.max(size.y).max(1.)).min(1.);
                let image = egui::Rect::from_center_size(rect.center(), size * scale);
                let tint = if dimmed {
                    egui::Color32::from_white_alpha(70)
                } else {
                    egui::Color32::WHITE
                };
                ui.painter().image(
                    texture.id(),
                    image,
                    egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
                    tint,
                );
            }
            if response.clicked() && importable {
                row.checked = !row.checked;
            }
            let name = (row.source.file.path.file_name())
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if importable {
                ui.checkbox(
                    &mut row.checked,
                    egui::RichText::new(name).size(text::SMALL),
                );
            } else {
                note(ui, name, palette.tone(Tone::DISABLED));
                let why = match row.duplicate {
                    Some(Duplicate::InImport) => "Suspected duplicate (twice here)",
                    _ => "Suspected duplicate",
                };
                note(ui, why, palette.tone(Tone::MUTED));
            }
            if row.source.captured.is_none() {
                note(ui, "Dated by its file", palette.tone(Tone::MUTED)).on_hover_text(
                    "It has no capture time: its file's modification date files and names it",
                );
            }
        });
    });
}

/// A row label of the settings, of one width.
fn row_label(ui: &mut egui::Ui, palette: &crate::app::ui::theme::Palette, text: &str) {
    ui.add_sized(
        Vec2::new(100., 20.),
        egui::Label::new(egui::RichText::new(text).color(palette.tone(Tone::LABEL))),
    );
}

/// File Handling, File Renaming, Apply During Import and Destination, as
/// Lightroom's Import has them.
fn settings_section(
    ui: &mut egui::Ui,
    d: &mut Importing,
    palette: &crate::app::ui::theme::Palette,
    shown: &mut Shown,
) {
    let before = (d.settings.clone(), d.choices.clone());
    let label = |ui: &mut egui::Ui, text: &str| row_label(ui, palette, text);
    heading(ui, palette, "File Handling");
    ui.horizontal(|ui| {
        label(ui, "");
        ui.checkbox(
            &mut d.settings.skip_duplicates,
            "Don't Import Suspected Duplicates",
        )
        .on_hover_text(
            "Photos already in the catalog (by their original name, capture time and size) \
             are greyed out and not imported",
        );
    });
    ui.add_space(space::S);
    heading(ui, palette, "File Renaming");
    ui.horizontal(|ui| {
        label(ui, "");
        ui.checkbox(&mut d.settings.rename, "Rename Files");
    });
    ui.add_enabled_ui(d.settings.rename, |ui| {
        ui.horizontal(|ui| {
            label(ui, "Template");
            let s = &mut d.settings.naming;
            match menu::combo(
                ui,
                "import-naming",
                240.,
                s.naming,
                s.tokens.as_deref(),
                &d.saved,
                true,
            ) {
                Some(NamingChoice::Preset(naming)) => s.choose_naming(naming),
                Some(NamingChoice::Tokens(tokens)) => s.choose_tokens(&tokens),
                Some(NamingChoice::Edit) => shown.edit = true,
                None => {}
            }
        });
        let template = d.settings.naming.name_template();
        ui.horizontal(|ui| {
            label(ui, "Custom Text");
            ui.add_enabled(
                template.uses_text(),
                egui::TextEdit::singleline(&mut d.settings.naming.custom_text).desired_width(240.),
            );
        });
        ui.horizontal(|ui| {
            label(ui, "Shoot Name");
            ui.add_enabled(
                template.uses_shoot(),
                egui::TextEdit::singleline(&mut d.settings.shoot_name).desired_width(240.),
            );
        });
        ui.horizontal(|ui| {
            label(ui, "Start Number");
            ui.add_enabled(
                template.uses_sequence(),
                egui::DragValue::new(&mut d.settings.naming.start_number).range(0..=999_999),
            );
        });
        ui.horizontal(|ui| {
            label(ui, "Extensions");
            let shown_case = |c: ExtensionCase| match c {
                ExtensionCase::Lowercase => "Lowercase",
                ExtensionCase::Uppercase => "Uppercase",
                ExtensionCase::LeaveAsIs => "Leave as-is",
            };
            egui::ComboBox::from_id_salt("import-extensions")
                .width(160.)
                .selected_text(shown_case(d.settings.extensions))
                .show_ui(ui, |ui| {
                    for c in [
                        ExtensionCase::Lowercase,
                        ExtensionCase::Uppercase,
                        ExtensionCase::LeaveAsIs,
                    ] {
                        ui.selectable_value(&mut d.settings.extensions, c, shown_case(c));
                    }
                });
        });
        if let Some((sample, _)) = d.rename_sample() {
            ui.horizontal(|ui| {
                label(ui, "Sample");
                note(ui, sample, palette.tone(Tone::SECONDARY));
            });
        }
    });
    ui.add_space(space::S);
    heading(ui, palette, "Apply During Import");
    ui.horizontal(|ui| {
        label(ui, "Creator");
        ui.add(egui::TextEdit::singleline(&mut d.settings.creator).desired_width(240.));
    });
    ui.horizontal(|ui| {
        label(ui, "Copyright");
        ui.add(egui::TextEdit::singleline(&mut d.settings.copyright).desired_width(240.));
    });
    ui.horizontal(|ui| {
        label(ui, "Keywords");
        ui.add(
            egui::TextEdit::singleline(&mut d.settings.keywords)
                .hint_text("Comma-separated")
                .desired_width(280.),
        );
    });
    if let Err(e) = crate::app::library::parse_keywords(&d.settings.keywords) {
        ui.horizontal(|ui| {
            label(ui, "");
            note(ui, format!("{e:#}"), palette.warning());
        });
    }
    note(
        ui,
        "A Creator or Copyright typed here replaces the camera's and the sidecar's; \
         keywords are added to the photos' own.",
        palette.tone(Tone::MUTED),
    );
    ui.add_space(space::S);
    heading(ui, palette, "Destination");
    ui.horizontal(|ui| {
        label(ui, "To");
        let to = d
            .settings
            .destination
            .as_deref()
            .map_or_else(|| "Choose a folder".to_string(), pretty_path);
        ui.add(
            egui::Label::new(egui::RichText::new(to).color(palette.tone(Tone::TEXT))).truncate(),
        );
        if ui.button("Choose…").clicked() {
            shown.choose_destination = true;
        }
    });
    ui.horizontal(|ui| {
        label(ui, "");
        ui.checkbox(&mut d.settings.into_subfolder, "Into Subfolder");
        ui.add_enabled(
            d.settings.into_subfolder,
            egui::TextEdit::singleline(&mut d.settings.subfolder)
                .hint_text("Enter name")
                .desired_width(170.),
        );
    });
    ui.horizontal(|ui| {
        label(ui, "Organize");
        let shown_organize = |o: &Organize| match o {
            Organize::ByOriginalFolders => "By original folders",
            Organize::ByDate(_) => "By date",
            Organize::IntoOneFolder => "Into one folder",
        };
        let date = match &d.settings.organize {
            Organize::ByDate(format) => *format,
            _ => DateFolders::YearThenDate,
        };
        egui::ComboBox::from_id_salt("import-organize")
            .width(180.)
            .selected_text(shown_organize(&d.settings.organize))
            .show_ui(ui, |ui| {
                for o in [
                    Organize::ByOriginalFolders,
                    Organize::ByDate(date),
                    Organize::IntoOneFolder,
                ] {
                    let on =
                        std::mem::discriminant(&o) == std::mem::discriminant(&d.settings.organize);
                    if ui.selectable_label(on, shown_organize(&o)).clicked() && !on {
                        d.settings.organize = o;
                    }
                }
            });
    });
    if let Organize::ByDate(format) = &mut d.settings.organize {
        let today = today();
        ui.horizontal(|ui| {
            label(ui, "Date Format");
            egui::ComboBox::from_id_salt("import-date-format")
                .width(220.)
                .selected_text(format.label(&today))
                .show_ui(ui, |ui| {
                    for (i, group) in DateFolders::GROUPS.iter().enumerate() {
                        if i > 0 {
                            ui.separator();
                        }
                        for &f in *group {
                            ui.selectable_value(format, f, f.label(&today));
                        }
                    }
                });
        });
    }
    if (d.settings.clone(), d.choices.clone()) != before {
        shown.changed_choice = true;
    }
}

/// Today, as the catalog keeps a time: the date Lightroom's Date Format
/// menu shows its formats with.
fn today() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    match crate::platform::clock::local_time(now) {
        Some([y, m, d, ..]) => format!("{y:04}-{m:02}-{d:02}T00:00:00.000"),
        None => "2026-11-19T00:00:00.000".into(),
    }
}

/// Where each photo goes, as planned, and what stops the import.
fn preview_section(
    ui: &mut egui::Ui,
    d: &mut Importing,
    palette: &crate::app::ui::theme::Palette,
    shown: &mut Shown,
) {
    if d.settings.destination.is_none() || !matches!(d.scan, Scan::Done) {
        return;
    }
    let skip = d.settings.skip_duplicates;
    if !d.rows.iter().any(|r| r.checked && r.importable(skip)) {
        let all_duplicates = !d.rows.is_empty() && d.rows.iter().all(|r| !r.importable(skip));
        let text = match all_duplicates {
            true => {
                "Every photo here is a suspected duplicate: turn off Don't Import Suspected \
                     Duplicates to import them anyway."
            }
            false => "Check the photos to import.",
        };
        note(ui, text, palette.tone(Tone::MUTED));
        return;
    }
    if crate::app::library::parse_keywords(&d.settings.keywords).is_err() {
        note(
            ui,
            "Correct the keywords to see where the photos go.",
            palette.warning(),
        );
        return;
    }
    let Some(planned) = d.current() else {
        note(
            ui,
            "Checking where the photos go…",
            palette.tone(Tone::MUTED),
        );
        return;
    };
    let plan = match &planned.plan {
        Ok(plan) => plan,
        Err(e) => {
            note(
                ui,
                format!("Nothing can be imported: {e}"),
                palette.warning(),
            );
            return;
        }
    };
    if d.changed {
        note(
            ui,
            "These names changed since they were shown: check them, then Import again.",
            palette.warning(),
        );
    }
    if let Some(refused) = &plan.refused {
        note(
            ui,
            format!("Nothing can be imported there: {refused}"),
            palette.warning(),
        );
    }
    if let Some(ambiguity) = plan.ambiguous.clone() {
        let shown_dirs: Vec<String> = ambiguity
            .directories
            .iter()
            .take(3)
            .map(|p| pretty_path(p))
            .collect();
        note(
            ui,
            format!(
                "{} matches more than one folder of the catalog on this computer. Choose the one these photos belong to:",
                shown_dirs.join(", ")
            ),
            palette.warning(),
        );
        ui.horizontal_wrapped(|ui| {
            for option in &ambiguity.options {
                let label = if option.relative.is_empty() {
                    format!("Root {} ({})", option.root, pretty_path(&option.path))
                } else {
                    format!("{} in root {}", option.relative, option.root)
                };
                if ui.button(label).clicked() {
                    d.choices.push(Choice {
                        options: ambiguity.options.clone(),
                        chosen: option.clone(),
                    });
                    shown.changed_choice = true;
                }
            }
        });
        return;
    }
    let destination = d.settings.destination.clone().unwrap_or_default();
    let shown_path = |path: &Path| {
        path.strip_prefix(&destination)
            .map_or_else(|_| pretty_path(path), |p| p.display().to_string())
    };
    egui::ScrollArea::vertical()
        .id_salt("import-preview")
        .max_height(200.)
        .show(ui, |ui| {
            for photo in &plan.photos {
                let name = photo
                    .source
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                ui.horizontal(|ui| {
                    note(ui, name.to_string(), palette.tone(Tone::SECONDARY));
                    note(ui, "→", palette.tone(Tone::DISABLED));
                    match (&photo.destination, &photo.skipped) {
                        (Some(to), _) => note(ui, shown_path(to), palette.tone(Tone::TEXT)),
                        (None, Some(why)) => {
                            note(ui, format!("not imported: {why}"), palette.warning())
                        }
                        (None, None) => note(ui, "not imported", palette.warning()),
                    }
                });
            }
        });
    if !plan.folders.is_empty() {
        let folders: Vec<String> = plan.folders.iter().take(4).map(|f| shown_path(f)).collect();
        let more = plan.folders.len().saturating_sub(folders.len());
        let mut text = format!("New folders: {}", folders.join(", "));
        if more > 0 {
            text.push_str(&format!(" and {more} more"));
        }
        note(ui, text, palette.tone(Tone::MUTED));
    }
}

fn footer(
    ui: &mut egui::Ui,
    d: &mut Importing,
    palette: &crate::app::ui::theme::Palette,
    shown: &mut Shown,
) {
    let plan = d.current().and_then(|p| p.plan.as_ref().ok());
    if let Some(plan) = plan {
        let photos = plan
            .photos
            .iter()
            .filter(|p| p.destination.is_some())
            .count();
        let mut line = format!(
            "{} · {} · {} free there",
            plural(photos, "photo", "photos"),
            super::size(plan.bytes),
            super::size(plan.available)
        );
        if plan.refused.is_none() && plan.bytes > plan.available {
            line.push_str(" · not enough space");
        }
        note(ui, line, palette.tone(Tone::LABEL));
    }
    note(
        ui,
        "The photos are copied and checked; the originals stay where they are. While they \
         are imported, other file operations and moving between photos wait.",
        palette.tone(Tone::MUTED),
    );
    ui.add_space(space::M);
    let can_import = d.runnable().is_some();
    ui.horizontal(|ui| {
        ui.spacing_mut().button_padding = button::DIALOG;
        if ui.button("Cancel").clicked() {
            shown.cancel = true;
        }
        if ui
            .add_enabled_ui(can_import, |ui| widgets::primary_button(ui, "Import"))
            .inner
            .clicked()
        {
            shown.import = true;
        }
    });
}

impl Editor {
    /// Import Photos' choices from the last import.
    fn remembered_import(&self) -> ImportCopySettings {
        #[cfg(test)]
        return self.file_ops.import_settings.clone();
        #[cfg(not(test))]
        ImportCopySettings::load()
    }
    /// Keeps Import Photos' choices for the next one: what is imported now
    /// doesn't depend on it, so a failure is only said.
    fn remember_import(&mut self, settings: &ImportCopySettings) -> Result<(), String> {
        #[cfg(test)]
        {
            self.file_ops.import_settings = settings.clone();
            Ok(())
        }
        #[cfg(not(test))]
        settings.save().map_err(|e| format!("{e:#}"))
    }
    /// What photos imported get: Preferences' defaults and readers, as Add
    /// Folder gives them.
    fn import_defaults(&self) -> NewPhotoDefaults {
        #[cfg(test)]
        return NewPhotoDefaults::default();
        #[cfg(not(test))]
        crate::catalog_session::new_photo_defaults()
    }
    /// What reads the scan's files for their names and capture times.
    fn import_reader(&self) -> FileReader {
        #[cfg(test)]
        if let Some(reader) = &self.file_ops.import_reader {
            return reader.clone();
        }
        scan::file_reader()
    }

    /// The top bar's text while an import runs; `None` when none does.
    pub(super) fn import_progress(&self) -> Option<(String, bool)> {
        let State::ImportRunning(r) = &self.file_ops.state else {
            return None;
        };
        let stopping = r.cancel.load(Ordering::Relaxed);
        let text = if stopping {
            "Stopping after the photo under way…".to_string()
        } else {
            format!("Importing {}…", plural(r.photos, "photo", "photos"))
        };
        Some((text, !stopping))
    }
}

impl RunningImport {
    pub(super) fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// For tests: the dialog driven as the user would drive it.
#[cfg(test)]
impl super::FileOps {
    fn importing(&mut self) -> &mut Importing {
        match &mut self.state {
            State::Importing(d) => d,
            _ => panic!("Import Photos isn't open"),
        }
    }
    pub(in crate::app) fn import_open(&self) -> bool {
        matches!(self.state, State::Importing(_))
    }
    pub(in crate::app) fn import_running(&self) -> bool {
        matches!(self.state, State::ImportRunning(_))
    }
    /// Chooses `folder` as the source, as the folder chooser would.
    pub(in crate::app) fn import_source(&mut self, folder: &Path) {
        *self.importing().picked.lock().unwrap() = Some((Pick::Source, folder.to_path_buf()));
    }
    /// Chooses `folder` as the destination (at once: a source may be
    /// waiting in the chooser's slot).
    pub(in crate::app) fn import_destination(&mut self, folder: &Path) {
        let d = self.importing();
        d.settings.destination = Some(folder.to_path_buf());
        d.choices.clear();
    }
    pub(in crate::app) fn import_settings_mut(&mut self) -> &mut ImportCopySettings {
        &mut self.importing().settings
    }
    /// Checks or unchecks the photo named `name`.
    pub(in crate::app) fn import_check(&mut self, name: &str, checked: bool) {
        let row = (self.importing().rows.iter_mut())
            .find(|r| r.source.file.path.file_name() == Some(std::ffi::OsStr::new(name)))
            .expect("no such photo");
        row.checked = checked;
    }
    /// The photos listed, by name: checked to be imported (not greyed
    /// out), suspected duplicate, dated.
    pub(in crate::app) fn import_rows(&mut self) -> Vec<(String, bool, bool, bool)> {
        let d = self.importing();
        let skip = d.settings.skip_duplicates;
        (d.rows.iter())
            .map(|r| {
                let name = r.source.file.path.file_name().unwrap().to_string_lossy();
                (
                    name.into_owned(),
                    r.checked && r.importable(skip),
                    r.duplicate.is_some(),
                    r.source.captured.is_some(),
                )
            })
            .collect()
    }
    pub(in crate::app) fn import_unreadable(&mut self) -> Vec<(PathBuf, String)> {
        self.importing().unreadable.clone()
    }
    /// The engine's request for what is chosen now.
    pub(in crate::app) fn import_request(&mut self) -> Option<ImportRequest> {
        let d = self.importing();
        Some(d.request(&d.inputs()?))
    }
    /// The plan shown for what is chosen now, once it is in.
    pub(in crate::app) fn import_plan(&mut self) -> Option<anyhow::Result<ImportPlan>> {
        let plan = self.importing().current()?.plan.clone();
        Some(plan.map_err(anyhow::Error::msg))
    }
    pub(in crate::app) fn import_changed(&mut self) -> bool {
        self.importing().changed
    }
    pub(in crate::app) fn import_scanning(&self) -> bool {
        matches!(&self.state, State::Importing(d) if matches!(d.scan, Scan::Reading { .. }))
            || matches!(&self.state, State::Importing(d) if d.planning.is_some())
    }
    pub(in crate::app) fn import_generation(&mut self) -> u64 {
        self.importing().generation
    }
}

#[cfg(test)]
impl Editor {
    /// Import, as its button asks for it.
    pub(in crate::app) fn press_import(&mut self) {
        self.run_import();
    }
}
