//! Quitting, however it happens: the steps of docs/shutdown.md's exit sequence.
use super::{Editor, task, workflow::Flushed};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

/// How long quitting waits for the edit to save and the stopped workers to end. The
/// preview renderer can wait up to 10 s for one GPU submission, but a quit that
/// hangs is worse than a render cut off; the loaders and exports write through
/// temporary files.
pub(super) const DEADLINE: Duration = Duration::from_secs(3);
/// How long quitting then waits for the temporary files of cut-off exports to be
/// deleted: they are on the folder an export stalled on, which can stall this too.
const CLEANUP: Duration = Duration::from_millis(500);
/// Deletes the temporary files of every export the process is still writing. Tests
/// run many editors in one process, so their exits leave other tests' exports alone;
/// `export`'s own tests cover the deletion.
#[cfg(not(test))]
const REMOVE_UNFINISHED: fn() = crate::export::remove_unfinished;
#[cfg(test)]
const REMOVE_UNFINISHED: fn() = || {};

impl Editor {
    /// Quit on macOS closes the window without a close request, so the close
    /// guard never sees it: the edit and the place in the catalog are saved here
    /// too. After a window close the guard has already flushed, and this finds
    /// nothing to do. Then the workers that write or hold the GPU are stopped and
    /// waited for, before eframe drops the device. A close the guard let through
    /// keeps the deadline it started with its save.
    pub(super) fn exit(&mut self) -> task::Waited {
        let until = self
            .quit_by
            .take()
            .unwrap_or_else(|| Instant::now() + DEADLINE);
        self.exit_by(until)
    }
    /// The exit sequence, with `deadline` for all of it.
    #[cfg(test)]
    pub(super) fn exit_within(&mut self, deadline: Duration) -> task::Waited {
        self.exit_by(Instant::now() + deadline)
    }
    /// The exit sequence, done by `until`. The workers wind down while the edit saves.
    fn exit_by(&mut self, until: Instant) -> task::Waited {
        self.cancel_jobs();
        self.close_channels();
        let mut stopping = self.stop_workers();
        // Nothing but the terminal is left to say so in: the edit stays as autosave
        // last saved it.
        match self.flush_by(until) {
            Flushed::Saved => {}
            Flushed::Failed => eprintln!("{}", self.messages.line()),
            Flushed::Late => eprintln!("Edits not saved: the catalog did not answer in time"),
        }
        self.remember_place(super::workspace::LayoutEdit::Settled);
        // Only now: the save went through it.
        stopping.push(self.autosave.close());
        let waited = task::wait_for(stopping, until.saturating_duration_since(Instant::now()));
        // Exports cut off at the deadline end with the process, which leaves their
        // temporary files behind.
        let removing = std::thread::Builder::new()
            .name("remove-unfinished".into())
            .spawn(REMOVE_UNFINISHED)
            .ok();
        task::wait_for(vec![task::Stopping::new(removing)], CLEANUP);
        waited
    }
    /// Every job in progress stops at its next check.
    fn cancel_jobs(&mut self) {
        self.profile_download.cancel();
        self.load.invalidate();
        self.preview.task.invalidate();
        self.preview.before.task.invalidate();
        self.reference.cancel_load();
        self.prefetch_cancel.store(true, Ordering::Relaxed);
        self.automation.cancel_outputs();
    }
    /// Drops the channel ends workers wait on, so none waits for the interface.
    fn close_channels(&mut self) {
        self.updates.close();
        self.controls.close_requests();
    }
    /// The workers the exit sequence waits for, asked to stop.
    fn stop_workers(&mut self) -> Vec<task::Stopping> {
        let mut stopping = Vec::from(self.loader.stop());
        stopping.extend([
            self.renderer.stop(),
            self.reference_loader.stop(),
            task::Stopping::new(self.exports.close()),
            task::Stopping::new(self.preview_builds.close()),
        ]);
        stopping.extend(self.stand_ins.stop());
        stopping.extend(self.selection.stop());
        stopping.extend(self.lensfun.stop());
        stopping.extend(self.automation.stop_outputs());
        stopping.extend(self.controls.stop());
        stopping.extend(self.stop_folder_sync());
        if let Some(library) = &mut self.library {
            stopping.extend(library.close_previews());
        }
        stopping
    }
}
