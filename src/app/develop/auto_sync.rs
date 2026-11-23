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
//! edit a batch is about to replace.
use crate::app::develop::settings_transfer::Settings;
use crate::app::history::Recorded;
use crate::app::library::sync::{
    BatchChange, BatchProgress, Decoding, SyncResult, SyncSource, SyncTarget,
};
use crate::app::worker::AutoKind;
use crate::app::{Editor, Module};
use crate::catalog::{Catalog, PhotoId};
use crate::model::settings_groups::GroupSelection;
use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a batch runs before the Sync button shows that it is still working.
pub(in crate::app) const SLOW: Duration = Duration::from_secs(1);
/// How long leaving the open photo or Undo waits for a batch; one that takes longer
/// is taken in by a later frame.
const SETTLE_WAIT: Duration = Duration::from_secs(120);

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

/// What a batch does to each photo.
#[derive(Clone, Debug, PartialEq)]
enum Change {
    /// The open photo's settings in these groups.
    Groups(GroupSelection),
    /// An Auto, measured on each photo.
    Auto(AutoKind),
}
impl Change {
    fn batch(&self) -> BatchChange {
        match self {
            Change::Groups(groups) => BatchChange::Settings(groups.clone()),
            Change::Auto(AutoKind::Settings) => BatchChange::AutoSettings,
            Change::Auto(AutoKind::WhiteBalance) => BatchChange::AutoWhiteBalance,
        }
    }
}

/// Steps of the open photo still to go to `targets`.
struct Waiting {
    photo: PhotoId,
    targets: Vec<SyncTarget>,
    change: Change,
    /// The open photo's settings after the latest of the steps.
    source: Settings,
    /// The steps' sequences, which name their undo commands.
    steps: Vec<u64>,
}

/// A batch writing the other photos.
struct Running {
    steps: Vec<u64>,
    started: Instant,
    result: mpsc::Receiver<SyncResult>,
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
    /// edit to synchronize.
    pub(in crate::app) fn note_for_auto_sync(&mut self, step: &Recorded) {
        let measured = self.auto_sync.measured.take();
        let Some(photo) = self.document.catalog_photo else {
            return;
        };
        if !self.session.auto_sync || self.module != Module::Develop || step.click {
            return;
        }
        let change = match measured {
            Some(kind) => Change::Auto(kind),
            None => {
                let groups = self.sync_groups().changed(&step.before, &step.after);
                if groups.is_empty() {
                    return;
                }
                Change::Groups(groups)
            }
        };
        let targets = self.auto_sync_targets();
        let Some(source) = self.current_settings().filter(|_| !targets.is_empty()) else {
            return;
        };
        let same_photos = |w: &Waiting| {
            w.photo == photo
                && w.targets.len() == targets.len()
                && w.targets.iter().zip(&targets).all(|(a, b)| a.id == b.id)
        };
        // Joined with the step before when it goes the same way to the same photos;
        // an Auto measures each photo and is never joined.
        if let Some(last) = self.auto_sync.waiting.back_mut()
            && same_photos(last)
            && let (Change::Groups(groups), Change::Groups(more)) = (&mut last.change, &change)
        {
            groups.add(more);
            last.source = source;
            last.steps.push(step.sequence);
            return;
        }
        self.auto_sync.waiting.push_back(Waiting {
            photo,
            targets,
            change,
            source,
            steps: vec![step.sequence],
        });
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
        let held = self.activity.holds_catalog_writes()
            || self.importing.is_some()
            || self.library.as_ref().is_none_or(|l| l.rereading());
        if !held {
            self.start_auto_sync();
        }
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
    pub(in crate::app) fn settle_auto_sync(&mut self) {
        loop {
            if let Some(running) = &self.auto_sync.running {
                match running.result.recv_timeout(SETTLE_WAIT) {
                    Ok(result) => self.auto_synced(result),
                    Err(mpsc::RecvTimeoutError::Disconnected) => self.auto_sync.running = None,
                    Err(mpsc::RecvTimeoutError::Timeout) => return,
                }
            } else if !self.start_auto_sync() {
                return;
            }
        }
    }
    /// Starts the oldest waiting batch; false when there was none to start.
    fn start_auto_sync(&mut self) -> bool {
        if self.auto_sync.running.is_some() {
            return false;
        }
        let Some(library) = &self.library else {
            self.auto_sync.waiting.clear();
            return false;
        };
        let Some(waiting) = self.auto_sync.waiting.pop_front() else {
            return false;
        };
        let catalog = library.session.catalog().location().clone();
        let defaults = self.raw_defaults.clone();
        let decoding = Decoding {
            demosaic: self.session.demosaic,
            cache: Some(self.decode_cache.clone()),
        };
        let change = waiting.change.batch();
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
        std::thread::spawn(move || {
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
        });
        true
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
