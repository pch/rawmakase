//! Catalog › Import's items from extensions' importers: one
//! [`PlanImport`] at a time, driven from the UI thread.
//!
//! 1. The open edit is saved (`ready_for_catalog`) and the import holds the
//!    catalog dialog's busy state until it ends, however it ends. On a
//!    dialog thread the user picks the source and the destination, and the
//!    import's folder is made ([`Event::CatalogImportPrepared`]).
//! 2. `import.run` goes through the extension host
//!    ([`Host::run_import`](crate::app::extensions::Host::run_import)), whose
//!    outcomes come back each frame: the program's progress on the status
//!    line, then its answer, or its failure (disabled, gone, silent past the
//!    inactivity deadline).
//! 3. On the answer a worker of its own imports and publishes
//!    ([`Event::CatalogImported`]). A worker also drops an import that ends
//!    before the answer, which deletes its folder: that may wait on a
//!    network share.
//! 4. The published catalog opens through the ordinary catalog-open path,
//!    which saves the open edit first. When that save fails the catalog
//!    stays published and the app says where it is.
//!
//! Disabling the extension, or its importer going, cancels the import until
//! it publishes; the program stopping once it has answered (idle stop, a
//! crash) changes nothing. An outcome of another request, or of an import
//! that ended, is dropped. Quitting cancels the import in any phase before
//! it published and waits for its worker at most the shared deadline
//! ([`Editor::stop_catalog_import`]).
use crate::app::Editor;
use crate::app::extensions::importer::Offer;
use crate::app::worker::{Event, panic_message};
use crate::catalog::plan_import::{Cancelled, Gate, PlanImport};
use eframe::egui;
use rawmakase_extensions::protocol::RemoteError;
use rawmakase_extensions::protocol::method::{ImportDone, ImportRun};
use serde_json::Value;
use std::path::PathBuf;
use std::thread::JoinHandle;

/// An import's folder made on the dialog thread, or why not.
pub(crate) struct Prepared {
    pub(in crate::app) offer: Offer,
    pub(in crate::app) result: Result<PlanImport, String>,
}

/// How an import ended: its published catalog and the photos it holds, or
/// `Err(None)` when it was stopped and `Err(Some(why))` when it failed.
pub(crate) type Imported = Result<(PathBuf, u64), Option<String>>;

/// What a test hears of a worker's reports.
#[cfg(test)]
pub(in crate::app) type Hear = Box<dyn FnMut(&str) + Send>;

/// The import under way.
pub(in crate::app) struct Running {
    offer: Offer,
    /// Tells its worker's reports from another import's.
    generation: u64,
    gate: Gate,
    phase: Phase,
    /// What the status line says it is doing.
    text: String,
    /// Hears each of the worker's reports, on the worker: tests pause it.
    #[cfg(test)]
    pub(in crate::app) on_progress: Option<Hear>,
}

enum Phase {
    /// Waiting for the program's answer to request `ticket`.
    Program { job: Box<PlanImport>, ticket: u64 },
    /// A worker imports and publishes, or drops what an import that ended
    /// left; it reports [`Event::CatalogImported`].
    Worker(Option<JoinHandle<()>>),
}

/// How long an import ended before its program answered keeps trying to
/// delete its folder: as long as the host waits for a cancelled request's
/// buffers before it takes the program for hung.
const DISCARD_WITHIN: std::time::Duration = rawmakase_extensions::session::LEASE_GRACE;

impl Editor {
    /// The import menu's items: the enabled extensions' importers.
    pub(in crate::app) fn importer_items(&self) -> Vec<Offer> {
        self.extensions.importers()
    }

    /// Whether `extension`'s program is working on an import's request.
    pub(in crate::app) fn catalog_import_by(&self, extension: &str) -> bool {
        self.catalog_import.as_ref().is_some_and(|r| {
            r.offer.extension == extension && matches!(r.phase, Phase::Program { .. })
        })
    }

    /// What the import under way is doing, if one is.
    pub(in crate::app) fn catalog_importing(&self) -> Option<&str> {
        self.catalog_import.as_ref().map(|r| r.text.as_str())
    }

    /// Catalog › Import › ‹importer›…: saves the open edit, asks for the
    /// source unless it is given and where the new catalog goes, and starts
    /// the import. Setup gives `beside`, a catalog the new one is named
    /// beside without asking ("Catalog.rawmakase", "Catalog 2.rawmakase"…).
    pub(in crate::app) fn import_with(
        &mut self,
        offer: Offer,
        source: Option<PathBuf>,
        beside: Option<PathBuf>,
        ctx: &egui::Context,
    ) {
        if self.catalog_import.is_some()
            || !self.ready_for_catalog()
            || !self.activity.begin_dialog()
        {
            return;
        }
        let failed_offer = offer.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            ctx.clone(),
            move |tx| {
                let extensions: Vec<&str> = offer.extensions.iter().map(String::as_str).collect();
                let Some(source) = source.or_else(|| {
                    rfd::FileDialog::new()
                        .add_filter(&offer.name, &extensions)
                        .pick_file()
                }) else {
                    let _ = tx.send(Event::DialogClosed);
                    return;
                };
                let stem = source.file_stem().unwrap_or_default().to_string_lossy();
                let destination = match beside {
                    Some(place) => (1..)
                        .map(|n| match n {
                            1 => place.with_file_name(format!("{stem}.rawmakase")),
                            n => place.with_file_name(format!("{stem} {n}.rawmakase")),
                        })
                        .find(|p| !p.exists()),
                    None => super::catalog_file_dialog()
                        .set_file_name(format!("{stem}.rawmakase"))
                        .save_file(),
                };
                let Some(destination) = destination else {
                    let _ = tx.send(Event::DialogClosed);
                    return;
                };
                let result =
                    PlanImport::prepare(&source, &destination).map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::CatalogImportPrepared(Box::new(Prepared {
                    offer,
                    result,
                })));
            },
            move |tx, error| {
                let _ = tx.send(Event::CatalogImportPrepared(Box::new(Prepared {
                    offer: failed_offer,
                    result: Err(error),
                })));
            },
        );
    }

    /// The dialog thread made an import's folder, or failed to.
    pub(in crate::app) fn catalog_import_prepared(&mut self, prepared: Prepared) {
        match prepared.result {
            Ok(job) => self.start_catalog_import(prepared.offer, job),
            Err(why) => {
                self.activity.finish_dialog();
                self.messages
                    .fail(format!("{} not imported: {why}", prepared.offer.name));
            }
        }
    }

    /// Sends `job`'s `import.run` to `offer`'s extension. The catalog
    /// dialog's busy state is held from here until the import ends.
    pub(in crate::app) fn start_catalog_import(&mut self, offer: Offer, job: PlanImport) {
        self.next_import += 1;
        let source = job.source().to_path_buf();
        let request = ImportRun {
            importer: offer.importer.clone(),
            source: source.clone(),
            plan: job.plan(),
        };
        let sent = self.extensions.run_import(&offer, request);
        self.catalog_import = Some(Running {
            offer,
            generation: self.next_import,
            gate: job.gate(),
            phase: Phase::Worker(None),
            text: format!(
                "Importing {}…",
                source.file_name().unwrap_or_default().to_string_lossy()
            ),
            #[cfg(test)]
            on_progress: None,
        });
        let job = Box::new(job);
        match sent {
            Ok(ticket) => {
                if let Some(running) = &mut self.catalog_import {
                    running.phase = Phase::Program { job, ticket };
                }
            }
            Err(why) => self.end_before_answer(job, Some(why)),
        }
    }

    /// The ticket of the import's request while the program works on it.
    #[cfg(test)]
    pub(in crate::app) fn catalog_import_ticket(&self) -> Option<u64> {
        match self.catalog_import.as_ref()?.phase {
            Phase::Program { ticket, .. } => Some(ticket),
            Phase::Worker(_) => None,
        }
    }

    /// Stops the import under way, unless it has begun publishing.
    pub(in crate::app) fn cancel_catalog_import(&mut self) {
        let Some(running) = &self.catalog_import else {
            return;
        };
        if !running.gate.cancel() {
            self.messages
                .note("The imported catalog is being saved; it can't be stopped now");
            return;
        }
        self.cancelled_before_answer();
    }

    /// An import cancelled while the program works on it: the request is
    /// cancelled, and a worker deletes the folder. Once the worker has the
    /// import, the gate stops it instead.
    fn cancelled_before_answer(&mut self) {
        let Some(running) = &mut self.catalog_import else {
            return;
        };
        let Phase::Program { ticket, .. } = running.phase else {
            return;
        };
        let Phase::Program { job, .. } = std::mem::replace(&mut running.phase, Phase::Worker(None))
        else {
            unreachable!("matched above");
        };
        let extension = running.offer.extension.clone();
        self.extensions.cancel_import(&extension, ticket);
        self.end_before_answer(job, None);
    }

    /// Ends an import before its program answered: a worker drops it,
    /// deleting its folder, and reports `why` (none: it was stopped).
    fn end_before_answer(&mut self, job: Box<PlanImport>, why: Option<String>) {
        job.gate().cancel();
        self.spawn_import_worker(move |_| {
            // The program may still have the plan open (it is told to stop,
            // and takes a moment), which Windows won't delete meanwhile.
            let Err(folder) = job.discard(DISCARD_WITHIN) else {
                return Err(why);
            };
            let left = format!(
                "{} was left behind; delete it once the extension has stopped",
                crate::app::ui::widgets::pretty_path(&folder)
            );
            Err(Some(match why {
                Some(why) => format!("{why}. {left}"),
                None => format!("Import stopped, but {left}"),
            }))
        });
    }

    /// The import request's `progress`; another request's is dropped.
    pub(in crate::app) fn import_progressed(&mut self, extension: &str, ticket: u64, text: String) {
        if let Some(running) = &mut self.catalog_import
            && running.offer.extension == extension
            && matches!(running.phase, Phase::Program { ticket: t, .. } if t == ticket)
        {
            running.text = text;
        }
    }

    /// The program's answer to the import's request, or its failure; an
    /// answer to another request, or from an import that ended, is dropped.
    pub(in crate::app) fn import_answered(
        &mut self,
        extension: &str,
        ticket: u64,
        result: Result<Value, RemoteError>,
    ) {
        let Some(running) = &mut self.catalog_import else {
            return;
        };
        if running.offer.extension != extension
            || !matches!(running.phase, Phase::Program { ticket: t, .. } if t == ticket)
        {
            return;
        }
        let Phase::Program { job, .. } = std::mem::replace(&mut running.phase, Phase::Worker(None))
        else {
            unreachable!("matched above");
        };
        let name = running.offer.extension_name.clone();
        let answer = result
            .map_err(|e| crate::catalog_import::failure(&name, &e))
            .and_then(|v| {
                serde_json::from_value::<ImportDone>(v)
                    .map_err(|e| Some(format!("{name} answered something unusable: {e}")))
            });
        match answer {
            Ok(done) => self.spawn_import_worker(move |progress| {
                job.run(done.photos, progress)
                    .map(|destination| (destination, done.photos))
                    .map_err(|e| (!e.is::<Cancelled>()).then(|| format!("{e:#}")))
            }),
            Err(why) => self.end_before_answer(job, why),
        }
    }

    /// Runs `work` on the import's worker, which reports its progress and
    /// its one outcome, a panic included.
    fn spawn_import_worker(
        &mut self,
        work: impl FnOnce(&mut dyn FnMut(&str)) -> Imported + Send + 'static,
    ) {
        let Some(running) = &mut self.catalog_import else {
            return;
        };
        let generation = running.generation;
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        #[cfg(test)]
        let mut on_progress = running.on_progress.take();
        let thread = std::thread::Builder::new()
            .name("catalog-import".into())
            .spawn(move || {
                let mut progress = |text: &str| {
                    #[cfg(test)]
                    if let Some(hear) = &mut on_progress {
                        hear(text);
                    }
                    let _ = tx.send(Event::CatalogImportProgress {
                        generation,
                        text: text.to_string(),
                    });
                    ctx.request_repaint();
                };
                // A panic drops the import, which deletes its folder.
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&mut progress)))
                        .unwrap_or_else(|panic| {
                            Err(Some(format!(
                                "Stopped unexpectedly: {}",
                                panic_message(&*panic)
                            )))
                        });
                let _ = tx.send(Event::CatalogImported { generation, result });
                ctx.request_repaint();
            });
        match thread {
            Ok(handle) => running.phase = Phase::Worker(Some(handle)),
            // The work, with the import it holds, was dropped with the
            // closure: its folder is deleted here.
            Err(e) => {
                let _ = self.tx.send(Event::CatalogImported {
                    generation,
                    result: Err(Some(format!("The import couldn't start: {e}"))),
                });
            }
        }
    }

    /// The worker's progress.
    pub(in crate::app) fn catalog_import_progress(&mut self, generation: u64, text: String) {
        if let Some(running) = &mut self.catalog_import
            && running.generation == generation
        {
            running.text = text;
        }
    }

    /// The import ended: its busy state ends, and a published catalog opens.
    pub(in crate::app) fn catalog_imported(&mut self, generation: u64, result: Imported) {
        if self
            .catalog_import
            .as_ref()
            .is_none_or(|r| r.generation != generation)
        {
            return;
        }
        let running = self.catalog_import.take().expect("checked above");
        self.activity.finish_dialog();
        let name = running.offer.name;
        match result {
            Ok((destination, photos)) => {
                let path = crate::app::ui::widgets::pretty_path(&destination);
                // Opening saves the open edit first; a failed save keeps
                // this catalog, saying where it is.
                if self.ready_for_catalog() {
                    self.messages
                        .note(format!("Imported {photos} photos into {path}"));
                    self.load_catalog(destination, &self.context.clone());
                } else {
                    self.messages.fail(format!(
                        "Imported {photos} photos into {path}, but it wasn't opened: the open edit couldn't be saved"
                    ));
                }
            }
            Err(None) => self.messages.note(format!("{name}: import stopped")),
            Err(Some(why)) => {
                self.messages.fail(format!("{name} not imported: {why}"));
                self.onboarding.catalog_error = Some(why);
            }
        }
    }

    /// Cancels the import when its extension or importer stopped being
    /// available (disabled, uninstalled), until it publishes.
    pub(in crate::app) fn check_catalog_import(&mut self) {
        let Some(running) = &self.catalog_import else {
            return;
        };
        if !self.extensions.importer_enabled(&running.offer) && running.gate.cancel() {
            self.cancelled_before_answer();
        }
    }

    /// Quitting: the import is cancelled in any phase before it published,
    /// and its worker returned to be waited for, until the shared deadline.
    /// A copy under way stops at its next block and deletes what it wrote.
    pub(in crate::app) fn stop_catalog_import(&mut self) -> Vec<crate::app::task::Stopping> {
        let Some(running) = &self.catalog_import else {
            return Vec::new();
        };
        running.gate.quit();
        self.cancelled_before_answer();
        match self.catalog_import.as_mut().map(|r| &mut r.phase) {
            Some(Phase::Worker(handle)) => vec![crate::app::task::Stopping::new(handle.take())],
            _ => Vec::new(),
        }
    }
}
