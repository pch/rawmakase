//! Lightroom's Auto Sync: while it is on, each History step of the open photo goes
//! to the other photos selected with it. A step takes only the groups it changed
//! among those Synchronize Settings applied last time, at their new values, so the
//! other photos keep the rest of their settings; an Auto or Auto White Balance is
//! measured on each photo instead.
//!
//! The other photos are written off the UI thread, one batch at a time, with no
//! progress window and nothing disabled. Steps made while a batch runs wait, joined
//! with the step before them when they go the same way to the same photos, so a
//! held key or a run of quick changes makes one batch, not one per step. Each batch
//! joins the open photo's steps it carries into one undo command with what it wrote
//! (see `UndoLog::attach_synced`), so one Undo takes back both.
//!
//! A step goes to the photos selected when it was made, with the open photo's
//! settings of then, wherever the editor is when its batch runs. Leaving the open
//! photo, Undo and anything else that saves first (`Editor::flush`) wait until every
//! step has been written ([`Editor::settle_auto_sync`]), so no photo opens with an
//! edit a batch is about to replace. A wait that runs out refuses what asked for it,
//! with a note, rather than going on while a batch still writes; closing the window
//! waits for Auto Sync as for any batch (see `docs/shutdown.md`).
use crate::app::develop::settings_transfer::Settings;
use crate::app::history::Recorded;
use crate::app::library::sync::{
    AutoSyncChange, BatchChange, BatchProgress, Decoding, SyncResult, SyncSource, SyncTarget,
};
use crate::app::worker::AutoKind;
use crate::app::{Editor, Module};
use crate::catalog::{Catalog, CatalogLocation, PhotoId};
use crate::model::settings_groups::GroupSelection;
use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a batch runs before the Sync button shows that it is still working.
pub(in crate::app) const SLOW: Duration = Duration::from_secs(1);
/// How long leaving the open photo or Undo waits, on the UI thread, for Auto Sync
/// to write every step; past it, they are refused until it has.
const SETTLE_WAIT: Duration = Duration::from_secs(3);

/// Auto Sync's batches: the steps waiting, oldest first, and the batch writing.
#[derive(Default)]
pub(in crate::app) struct AutoSync {
    waiting: VecDeque<Waiting>,
    running: Option<Running>,
    /// An Auto estimate landing on the open photo: its step is measured on each
    /// other photo rather than copied.
    measured: Option<AutoKind>,
    /// Holds each batch until the test sends a value.
    #[cfg(test)]
    pub(in crate::app) gate: Option<std::sync::Arc<std::sync::Mutex<mpsc::Receiver<()>>>>,
    /// Batches started.
    #[cfg(test)]
    pub(in crate::app) started: usize,
}

/// Steps of the open photo still to go to `targets`.
struct Waiting {
    /// The catalog the steps were made in: a batch for another is dropped.
    catalog: CatalogLocation,
    photo: PhotoId,
    targets: Vec<SyncTarget>,
    change: AutoSyncChange,
    /// The open photo's settings after the latest of the steps.
    source: Settings,
    /// The steps' sequences, which name their undo commands. They follow one
    /// another in the photo's History: a step Auto Sync does not take closes them.
    steps: Vec<u64>,
    /// A step since the last of `steps` went nowhere, so none joins them.
    closed: bool,
}

/// A batch writing the other photos.
struct Running {
    steps: Vec<u64>,
    started: Instant,
    result: mpsc::Receiver<SyncResult>,
    /// Its worker, for the exit sequence to wait for.
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Editor {
    /// The groups Synchronize Settings applied last time; until it has been used,
    /// the ones its dialog opens with.
    pub(in crate::app) fn sync_groups(&self) -> GroupSelection {
        self.session
            .sync_groups
            .clone()
            .unwrap_or_else(|| self.session.copy_groups.clone())
    }
    /// Turns Auto Sync on or off, as Lightroom's switch on the Sync button; the
    /// session keeps it. Steps made while it was on still go.
    pub(in crate::app) fn toggle_auto_sync(&mut self) {
        self.session.auto_sync = !self.session.auto_sync;
        let _ = self.save_session();
        self.messages.note(if self.session.auto_sync {
            "Auto Sync on: edits go to the selected photos too"
        } else {
            "Auto Sync off"
        });
    }
    /// The photos Auto Sync writes a step of the open photo to: those selected with
    /// it, when it is selected and its settings are its own (not a protected edit).
    fn auto_sync_targets(&self) -> Vec<SyncTarget> {
        let (Some(library), Some(open)) = (&self.library, self.document.catalog_photo) else {
            return Vec::new();
        };
        if !library.is_selected(open)
            || self.document.metadata.is_none()
            || self.document.edit.save_state().is_protected()
        {
            return Vec::new();
        }
        self.selected_besides(open)
    }
    /// The next step recorded is Auto's estimate of `kind` landing, which each
    /// other photo measures for itself.
    pub(in crate::app) fn auto_sync_measure(&mut self, kind: AutoKind) {
        self.auto_sync.measured = Some(kind);
    }
    /// Auto's step has been taken, or there was none.
    pub(in crate::app) fn auto_sync_measure_done(&mut self) {
        self.auto_sync.measured = None;
    }
    /// Takes a step of the open photo, as the shared undo log takes it, for Auto
    /// Sync. A History click moves within the photo's own History and is not an
    /// edit to synchronize. A step that goes nowhere (a click, an edit made with
    /// Auto Sync off or of groups Synchronize did not choose) ends the run of steps
    /// waiting, so their one undo command never takes it in.
    pub(in crate::app) fn note_for_auto_sync(&mut self, step: &Recorded) {
        let measure = self.auto_sync.measured.take();
        if !self.queue_auto_sync(step, measure)
            && let Some(last) = self.auto_sync.waiting.back_mut()
        {
            last.closed = true;
        }
    }
    /// [`Self::note_for_auto_sync`]'s step queued, or false when it goes nowhere.
    fn queue_auto_sync(&mut self, step: &Recorded, measure: Option<AutoKind>) -> bool {
        let Some(photo) = self.document.catalog_photo else {
            return false;
        };
        if !self.session.auto_sync || self.module != Module::Develop || step.click {
            return false;
        }
        let chosen = self.sync_groups();
        let change = AutoSyncChange {
            groups: chosen.changed(&step.before, &step.after),
            // An Auto's step sets sliders: a panel it turned on is turned on by each
            // photo's own Auto.
            switches: match measure {
                Some(_) => Default::default(),
                None => chosen.switches_changed(&step.before, &step.after),
            },
            measure,
        };
        if change.groups.is_empty() && change.switches.is_empty() {
            return false;
        }
        let targets = self.auto_sync_targets();
        let (Some(library), Some(metadata)) = (&self.library, self.document_metadata()) else {
            return false;
        };
        if targets.is_empty() {
            return false;
        }
        let catalog = library.session.catalog().location().clone();
        // The step's own settings: a later step of the same frame (a History click,
        // say) may have moved the open photo on since.
        let source = Settings {
            recipe: step.after.clone(),
            metadata,
        };
        let joins = |w: &Waiting| {
            !w.closed
                && w.catalog == catalog
                && w.photo == photo
                && w.targets.len() == targets.len()
                && w.targets.iter().zip(&targets).all(|(a, b)| a.id == b.id)
        };
        // Joined with the step before when it goes the same way to the same photos;
        // an Auto measures each photo and is never joined.
        if let Some(last) = self.auto_sync.waiting.back_mut()
            && joins(last)
            && last.change.measure.is_none()
            && change.measure.is_none()
        {
            last.change.groups.add(&change.groups);
            last.change.switches.extend(change.switches);
            last.source = source;
            last.steps.push(step.sequence);
            return true;
        }
        self.auto_sync.waiting.push_back(Waiting {
            catalog,
            photo,
            targets,
            change,
            source,
            steps: vec![step.sequence],
            closed: false,
        });
        true
    }
    /// Takes in a batch that finished and starts the next waiting one, once nothing
    /// else is writing the catalog's edits. Called every frame.
    pub(in crate::app) fn run_auto_sync(&mut self) {
        if let Some(running) = &self.auto_sync.running {
            match running.result.try_recv() {
                Ok(result) => self.auto_synced(result),
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => self.auto_sync.running = None,
            }
        }
        if !self.auto_sync_held() {
            self.start_auto_sync();
        }
    }
    /// Whether something else writes the catalog's edits, or decides what they
    /// mean, so no batch starts: a Sync or file operation, profiles being
    /// imported, Read Metadata from Files.
    fn auto_sync_held(&self) -> bool {
        self.activity.holds_catalog_writes()
            || self.importing.is_some()
            || self.library.as_ref().is_none_or(|l| l.rereading())
    }
    /// Whether a batch has been writing for longer than [`SLOW`].
    pub(in crate::app) fn auto_sync_slow(&self) -> bool {
        self.auto_sync
            .running
            .as_ref()
            .is_some_and(|r| r.started.elapsed() >= SLOW)
    }
    /// Whether steps are waiting or a batch is writing.
    pub(in crate::app) fn auto_sync_busy(&self) -> bool {
        self.auto_sync.running.is_some() || !self.auto_sync.waiting.is_empty()
    }
    /// Writes every step still waiting and waits for it, so the other photos'
    /// edits are final: before the open photo is left or saved, and before Undo.
    /// False, with a note, when they are not within [`SETTLE_WAIT`] or something
    /// else holds the catalog: what asked must not go on as if they were.
    pub(in crate::app) fn settle_auto_sync(&mut self) -> bool {
        let until = Instant::now() + SETTLE_WAIT;
        loop {
            if let Some(running) = &self.auto_sync.running {
                let left = until.saturating_duration_since(Instant::now());
                match running.result.recv_timeout(left) {
                    Ok(result) => self.auto_synced(result),
                    Err(mpsc::RecvTimeoutError::Disconnected) => self.auto_sync.running = None,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                }
            } else if self.auto_sync.waiting.is_empty() {
                return true;
            } else if self.auto_sync_held() {
                break;
            } else {
                self.start_auto_sync();
            }
        }
        self.messages
            .note("Auto Sync is still writing the other selected photos: try again in a moment");
        false
    }
    /// Forgets every step not written yet, and a batch writing, whose result is no
    /// longer taken in: for closing the window anyway. The batch's worker goes on
    /// alone, writing in one transaction, which the process ending rolls back.
    pub(in crate::app) fn abandon_auto_sync(&mut self) {
        self.auto_sync.waiting.clear();
        self.auto_sync.running = None;
    }
    /// The exit sequence's step: no step waiting starts, and the batch writing is
    /// handed over to be waited for under the deadline.
    pub(in crate::app) fn stop_auto_sync(&mut self) -> Vec<crate::app::task::Stopping> {
        self.auto_sync.waiting.clear();
        let worker = self.auto_sync.running.take().and_then(|r| r.worker);
        vec![crate::app::task::Stopping::new(worker)]
    }
    /// Starts the oldest waiting batch, dropping any made in another catalog.
    fn start_auto_sync(&mut self) {
        if self.auto_sync.running.is_some() {
            return;
        }
        let Some(library) = &self.library else {
            self.auto_sync.waiting.clear();
            return;
        };
        let catalog = library.session.catalog().location().clone();
        while self
            .auto_sync
            .waiting
            .front()
            .is_some_and(|w| w.catalog != catalog)
        {
            self.auto_sync.waiting.pop_front();
        }
        let Some(waiting) = self.auto_sync.waiting.pop_front() else {
            return;
        };
        let defaults = self.raw_defaults.clone();
        let decoding = Decoding {
            demosaic: self.session.demosaic,
            cache: Some(self.decode_cache().clone()),
            lenses: self.lens_profiles().clone(),
            profiles: self.camera_profiles().clone(),
            lensfun: self.lensfun_db().clone(),
        };
        let change = BatchChange::AutoSync(Box::new(waiting.change));
        let source = SyncSource::Open(Box::new(waiting.source));
        let targets = waiting.targets;
        let (tx, result) = mpsc::channel();
        let ctx = self.context.clone();
        #[cfg(test)]
        let gate = self.auto_sync.gate.clone();
        #[cfg(test)]
        {
            self.auto_sync.started += 1;
        }
        let worker = std::thread::spawn(move || {
            #[cfg(test)]
            if let Some(gate) = gate {
                let _ = gate.lock().map(|g| g.recv());
            }
            let progress = BatchProgress::new(change.name(), targets.len());
            // A panic (in Upright's analysis, say) still finishes the batch.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Catalog::open(&catalog).map(|mut c| {
                    crate::app::library::sync::synchronize(
                        &mut c,
                        Some(&source),
                        &change,
                        &targets,
                        &defaults,
                        &decoding,
                        &progress,
                    )
                })
            }));
            let _ = tx.send(crate::app::library::sync::finished(
                outcome, &change, &catalog,
            ));
            ctx.request_repaint();
        });
        self.auto_sync.running = Some(Running {
            steps: waiting.steps,
            started: Instant::now(),
            result,
            worker: Some(worker),
        });
    }
    /// A finished batch: the photos it wrote show their new edits, the steps it
    /// carried become one undo command with it, and only a photo it could not
    /// write is mentioned.
    fn auto_synced(&mut self, result: SyncResult) {
        let Some(running) = self.auto_sync.running.take() else {
            return;
        };
        // A result for a catalog no longer open must not reach this one.
        if self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location())
            != Some(&result.catalog)
        {
            return;
        }
        let ids: Vec<PhotoId> = result.synced.iter().map(|e| e.id).collect();
        if let Some(library) = &mut self.library {
            library.edits_changed(ids.iter().copied());
        }
        self.refresh_previews(&ids);
        self.reopen_synced(&ids);
        self.load_reference();
        if !result.synced.is_empty() {
            let command = crate::app::library::sync::SyncCommand {
                sequence: crate::edit_session::sequence(),
                change: result.change.clone(),
                edits: result.synced,
            };
            if let Err(command) = self.undo_log.attach_synced(&running.steps, command) {
                self.undo_log
                    .push(crate::app::shell::undo::Command::Sync(Box::new(command)));
            }
        }
        for failure in &result.failed {
            self.messages.note(format!(
                "Auto Sync: {} not changed: {}",
                failure.name, failure.reason
            ));
        }
    }
}

#[cfg(test)]
#[path = "auto_sync_tests.rs"]
mod tests;
