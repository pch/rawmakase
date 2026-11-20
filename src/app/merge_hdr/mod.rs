//! Merge to HDR… in the app: its window, the merge on a thread of its own,
//! and the merged DNG added to the catalog, stacked and shown.
//!
//! The Library's thumbnail menu, Photo › Merge to HDR… and the `merge_hdr`
//! command ask for it with the selection, which its window checks against the
//! catalog's photo info (2 to 9 photos, each file once, one camera, each
//! exposure its own) and sets Auto Align, Deghost, Create Stack and Open
//! after Merge for. Merge saves the open edit, then captures everything the
//! merge reads once: the photos' files, the active photo's edit as the
//! catalog has it, the options, the demosaic of Preferences and the raw
//! defaults. The work is [`crate::merge_hdr`]'s, as `rawmakase merge-hdr`
//! does it: the DNG is published beside the active photo, or nothing is.
//!
//! The merge holds nothing of the catalog: the Library stays usable, its
//! progress and Cancel show in the top bar, and a cancel, or quitting, stops
//! it within a pass over the pixels and publishes nothing. Once published,
//! the DNG is added as a source's report is (`app::extensions::sources`): the
//! catalog's folder-change activity is taken, retried each frame while other
//! work holds it, and a worker of its own imports it with its starting edit,
//! in one transaction on a connection of its own. Back on the interface, the
//! Library reads the catalog again in place, the merge is stacked on top of
//! the photos of its bracket still in its folder (one command, which Undo
//! takes back), and it is selected, or opened in Develop. When another
//! catalog was opened meanwhile, the DNG stays where it was saved, and the
//! message says where.
//!
//! A [`task::Task`]'s generation rejects every result but the running
//! merge's, and every ending, a panic included, clears what was running and
//! the folder-change activity.
use crate::app::{Editor, Module, task, worker::Event};
use crate::catalog::{CatalogLocation, PhotoId};
use crate::edits::EditRecord;
use crate::merge_hdr::{ActiveEdit, Published};
use crate::raw_defaults::DevelopDefaults;
use anyhow::{Context, Result};
use eframe::egui;
use rawmakase_cancel::Cancel;
use rawmakase_hdr_merge::{Deghost, Destination, FrameInput, MergeOptions, Stage, bracket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What the close guard calls a merge.
pub(in crate::app) const MERGE_HDR: &str = "Merge to HDR";

/// Merge to HDR's window, its merge and the DNG on its way into the catalog.
#[derive(Default)]
pub(in crate::app) struct MergeHdr {
    task: task::Task,
    dialog: Option<Dialog>,
    running: Option<Running>,
    /// The merge's thread, for quitting to wait for.
    thread: Option<std::thread::JoinHandle<()>>,
    /// Stands in for the merge and its publication, for a test.
    #[cfg(test)]
    pub(in crate::app) stub: Option<Stub>,
}

/// A test's merge: what it publishes, given the job and its cancel.
#[cfg(test)]
pub(in crate::app) type Stub = Arc<dyn Fn(&Job, Cancel<'_>) -> Result<Published> + Send + Sync>;

/// The window, while it is open.
struct Dialog {
    /// The catalog the photos are of: their ids name other photos in another.
    catalog: CatalogLocation,
    /// The photos, as the Library listed them.
    photos: Vec<PhotoId>,
    /// The one the merge starts from and is named after, Lightroom's active
    /// photo.
    active: PhotoId,
    frames: Vec<Frame>,
    /// Why these photos can't be merged; Merge waits for another selection.
    problem: Option<String>,
    align: bool,
    deghost: Deghost,
    stack: bool,
    open: bool,
}

/// A photo as the window lists it.
struct Frame {
    name: String,
    exposure: String,
}

/// What a merge reads, captured once when it starts.
pub(in crate::app) struct Job {
    /// The photos' files, in the Library's order.
    pub(in crate::app) sources: Vec<PathBuf>,
    /// The active photo's place in [`Self::sources`].
    pub(in crate::app) active: usize,
    /// The active photo's edit as the catalog had it.
    record: EditRecord,
    options: MergeOptions,
    raw_defaults: Arc<DevelopDefaults>,
}

/// A merge under way, from Merge until its DNG is in the catalog.
struct Running {
    generation: u64,
    catalog: CatalogLocation,
    /// The photos merged, to stack the DNG with.
    photos: Vec<PhotoId>,
    frames: usize,
    stack: bool,
    open: bool,
    progress: Arc<Mutex<Progress>>,
    phase: Phase,
}

#[derive(Clone, Copy, Default)]
enum Progress {
    #[default]
    Starting,
    Stage(Stage, f32),
    Saving,
}

enum Phase {
    Merging,
    /// Published, waiting for the catalog's folder-change activity.
    Waiting(Published),
    /// Its import worker is adding it.
    Adding(Published),
}

/// What the merge's and the import's workers send back.
pub(crate) enum Done {
    Merged {
        generation: u64,
        result: Result<Box<Published>, String>,
        cancelled: bool,
    },
    Added {
        generation: u64,
        /// False when the catalog had the DNG already.
        result: Result<bool, String>,
    },
}

impl MergeHdr {
    pub(in crate::app) fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }
    /// The photos the open window would merge.
    #[cfg(test)]
    pub(in crate::app) fn dialog_photos(&self) -> Option<&[PhotoId]> {
        self.dialog.as_ref().map(|d| d.photos.as_slice())
    }
    /// A merge is under way, or its DNG waits to be added: closing waits for
    /// it. Once its import has begun, the folder change it holds is what
    /// closing waits for.
    pub(in crate::app) fn running(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|r| !matches!(r.phase, Phase::Adding(_)))
    }
    /// Stops the merge, or forgets a DNG waiting to be added; returns the
    /// DNG left outside the catalog, if one was published. A merge stopped
    /// publishes nothing. An import under way is left to finish.
    pub(in crate::app) fn cancel(&mut self) -> Option<PathBuf> {
        if !self.running() {
            return None;
        }
        self.task.invalidate();
        match self.running.take()?.phase {
            Phase::Waiting(published) => Some(published.path),
            _ => None,
        }
    }
    /// Quitting's step: the merge is cancelled and its thread handed over to
    /// wait for, under the exit's deadline.
    pub(in crate::app) fn stop(&mut self) -> Vec<task::Stopping> {
        self.cancel();
        vec![task::Stopping::new(self.thread.take())]
    }
}

impl Editor {
    /// Opens Merge to HDR's window for `ids`, the Library's selection.
    pub(in crate::app) fn open_merge_hdr(&mut self, ids: Vec<PhotoId>) {
        if self.merge_hdr.running.is_some() {
            self.messages
                .warn("A merge is already running: wait for it, or cancel it, first.");
            return;
        }
        // A merge cancelled in a stalled read still holds its frames.
        if self
            .merge_hdr
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
        {
            self.messages
                .warn("The merge cancelled is still stopping: try again in a moment.");
            return;
        }
        let Some(library) = &self.library else {
            return;
        };
        let ids: Vec<PhotoId> = ids
            .into_iter()
            .filter(|id| library.photo(*id).is_some())
            .collect();
        let active = library
            .selected()
            .filter(|id| ids.contains(id))
            .or(ids.first().copied());
        let Some(active) = active else {
            return;
        };
        let catalog = library.session.catalog();
        let photos: Vec<_> = ids.iter().filter_map(|id| library.photo(*id)).collect();
        // A selection too large to be a bracket reads nothing more.
        let infos: Vec<_> = if photos.len() <= bracket::MAX_FRAMES {
            ids.iter()
                .map(|id| catalog.photo_info(*id).ok().flatten().unwrap_or_default())
                .collect()
        } else {
            vec![Default::default(); photos.len()]
        };
        let frames = photos
            .iter()
            .take(bracket::MAX_FRAMES)
            .zip(&infos)
            .map(|(photo, info)| Frame {
                name: photo.filename.clone(),
                exposure: [info.exposure_text(), info.iso_text()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", "),
            })
            .collect();
        let problem = problem(
            &photos
                .iter()
                .map(|p| (p.filename.as_str(), p.path.as_path()))
                .collect::<Vec<_>>(),
            &infos,
        );
        let catalog = catalog.location().clone();
        self.merge_hdr.dialog = Some(Dialog {
            catalog,
            photos: ids,
            active,
            frames,
            problem,
            align: true,
            deghost: Deghost::None,
            stack: false,
            open: true,
        });
    }

    /// Merge to HDR's window: the photos, the options, Cancel and Merge.
    pub(in crate::app) fn merge_hdr_window(&mut self, ctx: &egui::Context) {
        use crate::app::ui::design::{self, Tone, button, inset, space, text};
        use crate::app::ui::widgets;
        let Some(mut dialog) = self.merge_hdr.dialog.take() else {
            return;
        };
        let palette = crate::app::ui::theme::palette(ctx);
        let mut merge = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("merge-hdr"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(400.);
                ui.label(
                    egui::RichText::new("Merge to HDR")
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::M);
                for frame in &dialog.frames {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&frame.name).size(text::LABEL));
                        ui.label(
                            egui::RichText::new(&frame.exposure)
                                .size(text::LABEL)
                                .color(palette.tone(Tone::MUTED)),
                        );
                    });
                }
                if let Some(problem) = &dialog.problem {
                    ui.add_space(space::S);
                    ui.label(
                        egui::RichText::new(problem)
                            .size(text::LABEL)
                            .color(palette.tone(Tone::STRONG)),
                    );
                }
                ui.add_space(space::M);
                ui.checkbox(&mut dialog.align, "Auto Align");
                ui.horizontal(|ui| {
                    ui.label("Deghost");
                    egui::ComboBox::from_id_salt("merge-hdr-deghost")
                        .selected_text(deghost_name(dialog.deghost))
                        .show_ui(ui, |ui| {
                            for level in
                                [Deghost::None, Deghost::Low, Deghost::Medium, Deghost::High]
                            {
                                ui.selectable_value(
                                    &mut dialog.deghost,
                                    level,
                                    deghost_name(level),
                                );
                            }
                        });
                });
                ui.checkbox(&mut dialog.stack, "Create Stack");
                ui.checkbox(&mut dialog.open, "Open after Merge");
                ui.label(
                    egui::RichText::new(
                        "The merge is saved as a DNG beside the active photo, named after it, \
                         and starts with its white balance, profile and lens corrections.",
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
                    merge = ui
                        .add_enabled_ui(dialog.problem.is_none(), |ui| {
                            widgets::primary_button(ui, "Merge")
                        })
                        .inner
                        .clicked();
                });
            });
        if cancel || response.should_close() {
            return;
        }
        if merge {
            self.start_merge_hdr(dialog);
        } else {
            self.merge_hdr.dialog = Some(dialog);
        }
    }

    /// Starts merging what `dialog` says, once the open edit is saved: the
    /// active photo's edit is read from the catalog.
    fn start_merge_hdr(&mut self, dialog: Dialog) {
        if !self.flush() {
            self.messages
                .warn("Merge to HDR didn't start: the open edit couldn't be saved first.");
            return;
        }
        let Some(library) = &self.library else {
            return;
        };
        if *library.session.catalog().location() != dialog.catalog {
            self.messages.warn(
                "Merge to HDR didn't start: another catalog is open now. Choose the photos again.",
            );
            return;
        }
        let sources: Vec<PathBuf> = dialog
            .photos
            .iter()
            .filter_map(|id| library.photo(*id).map(|p| p.path.clone()))
            .collect();
        let active = dialog.photos.iter().position(|id| *id == dialog.active);
        let (Some(active), true) = (active, sources.len() == dialog.photos.len()) else {
            self.messages
                .warn("Merge to HDR didn't start: its photos are no longer in the catalog.");
            return;
        };
        let record = match library.session.catalog().edit_record(dialog.active) {
            Ok(record) => record,
            Err(e) => {
                self.messages.fail(format!(
                    "Merge to HDR didn't start: the active photo's edit couldn't be read: {e:#}"
                ));
                return;
            }
        };
        let job = Job {
            active,
            record,
            options: MergeOptions {
                demosaic: self.session.demosaic,
                align: dialog.align,
                deghost: dialog.deghost,
                software: format!("RAWmakase {}", env!("CARGO_PKG_VERSION")),
            },
            raw_defaults: self.raw_defaults.clone(),
            sources,
        };
        let (generation, cancel) = self.merge_hdr.task.start();
        let progress = Arc::new(Mutex::new(Progress::default()));
        self.merge_hdr.running = Some(Running {
            generation,
            catalog: library.session.catalog().location().clone(),
            photos: dialog.photos,
            frames: job.sources.len(),
            stack: dialog.stack,
            open: dialog.open,
            progress: progress.clone(),
            phase: Phase::Merging,
        });
        #[cfg(test)]
        let stub = self.merge_hdr.stub.clone();
        let thread = task::spawn_joinable(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let cancel_now = Cancel::new(&cancel);
                #[cfg(test)]
                let result = match stub {
                    Some(stub) => stub(&job, cancel_now),
                    None => run(&job, &progress, cancel_now),
                };
                #[cfg(not(test))]
                let result = run(&job, &progress, cancel_now);
                let cancelled = result.as_ref().is_err_and(|e| {
                    matches!(
                        e.downcast_ref::<rawmakase_hdr_merge::Error>(),
                        Some(rawmakase_hdr_merge::Error::Cancelled)
                    )
                });
                let _ = tx.send(Event::MergeHdr(Box::new(Done::Merged {
                    generation,
                    result: result.map(Box::new).map_err(|e| format!("{e:#}")),
                    cancelled,
                })));
            },
            move |tx, why| {
                let _ = tx.send(Event::MergeHdr(Box::new(Done::Merged {
                    generation,
                    result: Err(why),
                    cancelled: false,
                })));
            },
        );
        // The last merge's thread had ended (`open_merge_hdr`).
        self.merge_hdr.thread = Some(thread);
    }

    /// The merge's progress and Cancel in the top bar.
    pub(in crate::app) fn merge_hdr_progress(&mut self, ui: &mut egui::Ui) {
        use crate::app::ui::design::{Tone, radius, text};
        let Some(running) = &self.merge_hdr.running else {
            return;
        };
        let (line, fraction) = match &running.phase {
            Phase::Merging => match *running.progress.lock().unwrap_or_else(|e| e.into_inner()) {
                Progress::Starting => ("Merge to HDR: starting".to_string(), 0.),
                Progress::Stage(stage, done) => (
                    format!(
                        "Merge to HDR: {}",
                        crate::merge_hdr::stage_text(stage, running.frames)
                            .unwrap_or_else(|| "merging".into())
                            .to_lowercase()
                    ),
                    done,
                ),
                Progress::Saving => ("Merge to HDR: saving".to_string(), 1.),
            },
            Phase::Waiting(_) | Phase::Adding(_) => {
                ("Merge to HDR: adding to the catalog".to_string(), 1.)
            }
        };
        let palette = crate::app::ui::theme::palette(ui.ctx());
        let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(230., 28.), egui::Sense::hover());
        let painter = ui.painter();
        painter.text(
            egui::pos2(rect.left(), rect.top() + 7.),
            egui::Align2::LEFT_CENTER,
            line,
            egui::FontId::proportional(text::SMALL),
            palette.tone(Tone::SOFT),
        );
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.top() + 17.),
            egui::Vec2::new(rect.width() - 26., 4.),
        );
        painter.rect_filled(bar, radius::SMALL, palette.tone(Tone::OUTLINE));
        painter.rect_filled(
            egui::Rect::from_min_size(
                bar.min,
                egui::Vec2::new(bar.width() * fraction.clamp(0., 1.), 4.),
            ),
            2.,
            egui::Color32::from_rgb(110, 150, 190),
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
        if !self.merge_hdr.running() {
            return;
        }
        let close = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 9., rect.center().y),
            egui::Vec2::splat(18.),
        );
        let cancel = ui
            .interact(
                close,
                ui.id().with("cancel-merge-hdr"),
                egui::Sense::click(),
            )
            .on_hover_text("Stop merging");
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
        if cancel.clicked() {
            self.cancel_merge_hdr();
        }
    }

    /// Cancel in the top bar, or in the close guard: the merge stops and
    /// publishes nothing; a DNG already saved stays outside the catalog.
    pub(in crate::app) fn cancel_merge_hdr(&mut self) {
        let running = self.merge_hdr.running();
        match self.merge_hdr.cancel() {
            Some(path) => self.messages.warn(format!(
                "Merge to HDR: saved at {}, not added to the catalog",
                path.display()
            )),
            None if running => self.messages.note("Merge to HDR cancelled"),
            None => {}
        }
    }

    /// A worker of the merge reported back.
    pub(in crate::app) fn merge_hdr_done(&mut self, done: Done) {
        match done {
            Done::Merged {
                generation,
                result,
                cancelled,
            } => self.merged(generation, result, cancelled),
            Done::Added { generation, result } => self.merge_added(generation, result),
        }
    }

    fn merged(&mut self, generation: u64, result: Result<Box<Published>, String>, cancelled: bool) {
        let current = self
            .merge_hdr
            .running
            .as_ref()
            .is_some_and(|r| r.generation == generation && matches!(r.phase, Phase::Merging));
        if !current {
            // Cancelled, but done just before it saw that.
            if let Ok(published) = result {
                self.messages.warn(format!(
                    "Merge to HDR: saved at {}, not added to the catalog",
                    published.path.display()
                ));
            }
            return;
        }
        self.merge_hdr.task.finish(generation);
        let published = match result {
            Ok(published) => *published,
            Err(why) => {
                self.merge_hdr.running = None;
                if cancelled {
                    self.messages.note("Merge to HDR cancelled");
                } else {
                    self.messages.fail(format!("Merge to HDR: {why}"));
                }
                return;
            }
        };
        for note in &published.notes {
            self.messages.warn(format!("Merge to HDR: {note}"));
        }
        if !self.merging_into_open_catalog() {
            let name = self
                .merge_hdr
                .running
                .take()
                .map(|r| r.catalog.name())
                .unwrap_or_default();
            self.messages.warn(format!(
                "Merge to HDR: saved at {}, but not added to {name}: another catalog is open now",
                published.path.display()
            ));
            return;
        }
        let running = self.merge_hdr.running.as_mut().expect("checked above");
        running.phase = Phase::Waiting(published);
        self.add_merged();
    }

    /// Adds the DNG waiting to the catalog once nothing else holds it;
    /// called each frame meanwhile.
    pub(in crate::app) fn add_merged(&mut self) {
        let waiting = self
            .merge_hdr
            .running
            .as_ref()
            .is_some_and(|r| matches!(r.phase, Phase::Waiting(_)));
        if !waiting {
            return;
        }
        if !self.merging_into_open_catalog() {
            if let Some(Phase::Waiting(published)) = self.merge_hdr.running.take().map(|r| r.phase)
            {
                self.messages.warn(format!(
                    "Merge to HDR: saved at {}, but not added: another catalog is open now",
                    published.path.display()
                ));
            }
            return;
        }
        // Excludes Add Folder, Synchronize, batch edits, file operations, a
        // source's report and a catalog change until the worker reports back.
        if !self.activity.begin_folder_change() {
            return;
        }
        let running = self.merge_hdr.running.as_mut().expect("checked above");
        let phase = std::mem::replace(&mut running.phase, Phase::Merging);
        let Phase::Waiting(published) = phase else {
            unreachable!("checked above");
        };
        let generation = running.generation;
        let location = running.catalog.clone();
        let job = Published {
            path: published.path.clone(),
            recipe: published.recipe.clone(),
            notes: Vec::new(),
        };
        running.phase = Phase::Adding(published);
        task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let result = crate::catalog::Catalog::open(&location)
                    .with_context(|| {
                        format!(
                            "The merge was saved at {}, but couldn't be added to the catalog",
                            job.path.display()
                        )
                    })
                    .and_then(|mut catalog| {
                        crate::merge_hdr::import(
                            &mut catalog,
                            &job,
                            &crate::catalog_session::new_photo_defaults(),
                        )
                    })
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::MergeHdr(Box::new(Done::Added {
                    generation,
                    result,
                })));
            },
            move |tx, why| {
                let _ = tx.send(Event::MergeHdr(Box::new(Done::Added {
                    generation,
                    result: Err(why),
                })));
            },
        );
    }

    /// Whether the catalog the running merge started in is still open.
    fn merging_into_open_catalog(&self) -> bool {
        let open = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location());
        self.merge_hdr
            .running
            .as_ref()
            .is_some_and(|r| open == Some(&r.catalog))
    }

    /// The DNG's import reported back: the catalog is free again, and the
    /// Library shows the merge, stacked as asked.
    fn merge_added(&mut self, generation: u64, result: Result<bool, String>) {
        let ours = self
            .merge_hdr
            .running
            .as_ref()
            .is_some_and(|r| r.generation == generation && matches!(r.phase, Phase::Adding(_)));
        if !ours {
            return;
        }
        self.activity.finish_dialog();
        let running = self.merge_hdr.running.take().expect("checked above");
        let Phase::Adding(published) = running.phase else {
            unreachable!("checked above");
        };
        let path = published.path;
        let added = match result {
            Ok(added) => added,
            Err(why) => {
                self.messages.fail(format!("Merge to HDR: {why}"));
                return;
            }
        };
        let same = self
            .library
            .as_ref()
            .is_some_and(|l| *l.session.catalog().location() == running.catalog);
        if !same {
            self.messages.warn(format!(
                "Merge to HDR: {} was added to {}, which is no longer open",
                path.display(),
                running.catalog.name()
            ));
            return;
        }
        // Reading the catalog again drops a Copy Name or metadata field being
        // typed: it is saved first, and when it can't be the merge shows with
        // the next read.
        if added
            && self.commit_library_drafts()
            && let Some(library) = &mut self.library
        {
            match library.reread_after_added() {
                Ok(()) => library.start_background_reads(),
                Err(e) => self.messages.fail(format!(
                    "Merge to HDR: added, but the catalog could not be read again: {e:#}"
                )),
            }
        }
        let Some(photo) = self.catalog_photo_at(&path) else {
            self.messages.warn(format!(
                "Merge to HDR: {} added, but not stacked or shown: it shows once the catalog \
                 is read again",
                file_name(&path)
            ));
            return;
        };
        if running.stack
            && let Some(library) = &mut self.library
        {
            match library.stack_merge(photo, running.photos) {
                Ok(true) => {}
                Ok(false) => self.messages.warn(format!(
                    "Merge to HDR: {} wasn't stacked: none of its photos is in its folder",
                    file_name(&path)
                )),
                Err(e) => self.messages.fail(format!(
                    "Merge to HDR: {} added, but not stacked: {e:#}",
                    file_name(&path)
                )),
            }
            self.messages.absorb(&mut library.messages);
        }
        if !added {
            self.messages.note(format!(
                "Merge to HDR: {} was in the catalog already, which keeps its edit there",
                file_name(&path)
            ));
        } else {
            self.messages
                .done(format!("Merged to HDR: {}", file_name(&path)));
        }
        if running.open {
            self.develop_catalog_photo(photo);
        } else if let Some(library) = &mut self.library {
            if self.module == Module::Library {
                library.reveal(photo);
            } else {
                library.show(photo);
            }
        }
    }
}

/// The merge: the sources read, the active photo's edit resolved, merged and
/// published beside the active photo.
fn run(job: &Job, progress: &Mutex<Progress>, cancel: Cancel<'_>) -> Result<Published> {
    let inputs = job
        .sources
        .iter()
        .map(|path| FrameInput::read(path))
        .collect::<Result<Vec<_>, _>>()?;
    let active_path = &job.sources[job.active];
    let active = ActiveEdit::resolve(&job.record, active_path, &job.raw_defaults, cancel)?;
    let set = |p: Progress| *progress.lock().unwrap_or_else(|e| e.into_inner()) = p;
    let merged = rawmakase_hdr_merge::merge(
        &inputs,
        &job.options,
        &|stage, done| set(Progress::Stage(stage, done)),
        cancel,
    )?;
    set(Progress::Saving);
    let mut published = crate::merge_hdr::publish(
        &merged,
        active.active(),
        &job.raw_defaults,
        &Destination::beside(active_path),
        &job.options.software,
        cancel,
    )?;
    published.notes.splice(0..0, active.warnings);
    Ok(published)
}

/// Why `photos` (their names and files, with their `infos`) aren't a
/// bracket, as far as the catalog tells; the merge checks the files
/// themselves.
fn problem(photos: &[(&str, &Path)], infos: &[crate::metadata::PhotoInfo]) -> Option<String> {
    let n = photos.len();
    if !(bracket::MIN_FRAMES..=bracket::MAX_FRAMES).contains(&n) {
        return Some(format!(
            "A bracket is {} to {} photos, not {}.",
            bracket::MIN_FRAMES,
            bracket::MAX_FRAMES,
            crate::app::ui::widgets::plural(n, "photo", "photos")
        ));
    }
    for (i, (name, path)) in photos.iter().enumerate() {
        if photos[..i].iter().any(|(_, other)| other == path) {
            return Some(format!(
                "{name} is chosen twice, with a virtual copy: choose each photo once."
            ));
        }
    }
    let mut cameras: Vec<&str> = infos.iter().filter_map(|i| i.camera.as_deref()).collect();
    cameras.dedup();
    if let [first, second, ..] = cameras[..]
        && first != second
    {
        return Some(format!(
            "The photos were taken with more than one camera: {first} and {second}."
        ));
    }
    // t · ISO / N² where every photo records its time, as the merge's own
    // check reads it: the f-number and ISO only when every photo records
    // them, else as the same in all.
    let all = |f: fn(&crate::metadata::PhotoInfo) -> Option<f64>| {
        infos.iter().all(|i| f(i).is_some_and(|v| v > 0.))
    };
    let (f_number, iso) = (all(|i| i.aperture), all(|i| i.iso));
    let ev: Option<Vec<f64>> = infos
        .iter()
        .map(|i| {
            let t = i.exposure.filter(|t| *t > 0.)?;
            let n = if f_number { i.aperture? } else { 1. };
            let s = if iso { i.iso? } else { 100. };
            Some((t * s / (n * n)).log2())
        })
        .collect();
    if let Some(ev) = ev {
        for (i, a) in ev.iter().enumerate() {
            if let Some(j) = ev[..i]
                .iter()
                .position(|b| (a - b).abs() < bracket::MIN_STEP)
            {
                return Some(format!(
                    "{} and {} have the same exposure: a bracket's photos each have their own.",
                    photos[j].0, photos[i].0
                ));
            }
        }
    }
    None
}

fn deghost_name(level: Deghost) -> &'static str {
    match level {
        Deghost::None => "None",
        Deghost::Low => "Low",
        Deghost::Medium => "Medium",
        Deghost::High => "High",
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests;
