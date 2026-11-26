//! Installing and removing AI Denoise's model pack on its own thread, only on
//! the user's choice. What an install does is in [`crate::model_install`]. The
//! [`Installer`] is kept on the editor beside the AI Denoise service. Subject's
//! and Sky's models are the AI masks extension's to install.
use crate::app::task::Stopping;
use crate::app::worker::Event;
use crate::model_install::{InstallError, Progress};
use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, mpsc::Sender};
use std::time::Duration;

/// The pack on disk and the install in progress, if any.
pub(crate) struct Installer {
    /// What the folder held when last looked at, and when: the files on disk decide
    /// whether the models are installed (a copy made, or deleted, by hand counts), not
    /// a note of what this process did.
    seen: std::cell::Cell<Option<(std::time::Instant, bool)>>,
    progress: Option<Arc<Progress>>,
    thread: Option<std::thread::JoinHandle<()>>,
    removing: bool,
    /// Why the last install failed, until the next one starts.
    failed: Option<String>,
}

impl Installer {
    pub(in crate::app) fn new() -> Self {
        Self {
            seen: Default::default(),
            progress: None,
            thread: None,
            removing: false,
            failed: None,
        }
    }
    pub(in crate::app) fn installed(&self) -> bool {
        if self.removing {
            return false;
        }
        // Looked at again after a second, so the drawer does not stat on every frame.
        match self.seen.get() {
            Some((at, installed)) if at.elapsed() < Duration::from_secs(1) => installed,
            _ => {
                let installed = crate::model_install::is_installed();
                self.seen.set(Some((std::time::Instant::now(), installed)));
                installed
            }
        }
    }
    fn forget(&self) {
        self.seen.set(None);
    }
    /// The folder holding the model's files.
    pub(in crate::app) fn path(&self) -> Option<PathBuf> {
        self.installed().then(crate::model_install::dir)
    }
    pub(in crate::app) fn busy(&self) -> bool {
        self.progress.is_some() || self.removing
    }
    /// Bytes fetched so far and the total, while installing.
    pub(in crate::app) fn progress(&self) -> Option<(u64, u64)> {
        self.progress
            .as_ref()
            .map(|p| (p.fetched(), crate::model_install::bytes()))
    }
    pub(in crate::app) fn cancel(&mut self) {
        if let Some(p) = &self.progress {
            p.cancel();
        }
    }
    /// Why the last install failed, if it did and was not cancelled.
    pub(in crate::app) fn failure(&self) -> Option<&str> {
        self.failed.as_deref()
    }
    /// Called when the install's event arrives.
    pub(in crate::app) fn finished(&mut self, result: &Result<(), InstallError>) {
        self.progress = None;
        self.thread = None;
        self.failed = match result {
            Ok(()) | Err(InstallError::Cancelled) => None,
            Err(e) => Some(e.to_string()),
        };
        self.forget();
    }
    /// Starts fetching the model, or copying `import`; one install at a time.
    pub(in crate::app) fn install(
        &mut self,
        import: Option<PathBuf>,
        tx: Sender<Event>,
        ctx: egui::Context,
    ) {
        if self.busy() || self.installed() {
            return;
        }
        let progress = Arc::new(Progress::default());
        self.progress = Some(progress.clone());
        self.failed = None;
        let spawned = std::thread::Builder::new()
            .name("model-install".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::model_install::install(import.as_deref(), &progress)
                }))
                .unwrap_or_else(|_| {
                    Err(InstallError::Failed(
                        "the install stopped unexpectedly".into(),
                    ))
                });
                let _ = tx.send(Event::ModelInstalled(result));
                ctx.request_repaint();
            });
        match spawned {
            Ok(handle) => self.thread = Some(handle),
            Err(e) => {
                self.progress = None;
                eprintln!("Could not start the model install: {e}");
            }
        }
    }
    /// Cancels an install and hands its thread to the shared shutdown deadline.
    pub(in crate::app) fn stop(&mut self) -> Vec<Stopping> {
        self.cancel();
        vec![Stopping::new(self.thread.take())]
    }
    /// Removes the model from disk: the worker lets go of it first, off the UI
    /// thread, and the files go once it has. Refuses to delete them if the worker
    /// does not finish unloading within its deadline.
    pub(in crate::app) fn remove(
        &mut self,
        unload: impl FnOnce() -> bool + Send + 'static,
        tx: Sender<Event>,
        ctx: egui::Context,
    ) {
        if self.busy() || !self.installed() {
            return;
        }
        self.remove_dir(crate::model_install::dir(), unload, tx, ctx);
    }
    fn remove_dir(
        &mut self,
        dir: PathBuf,
        unload: impl FnOnce() -> bool + Send + 'static,
        tx: Sender<Event>,
        ctx: egui::Context,
    ) {
        self.removing = true;
        std::thread::spawn(move || {
            let result = if !unload() {
                Err("the denoiser did not finish unloading the model in time".into())
            } else {
                std::fs::remove_dir_all(dir)
                    .or_else(|e| {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            Ok(())
                        } else {
                            Err(e)
                        }
                    })
                    .map_err(|e| e.to_string())
            };
            let _ = tx.send(Event::ModelRemoved(result));
            ctx.request_repaint();
        });
    }
    pub(in crate::app) fn removed(&mut self) {
        self.removing = false;
        self.forget();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_failed_install_says_why_until_the_next_one() {
        let mut installer = Installer::new();
        installer.finished(&Err(InstallError::Failed(
            "could not download the model: timed out".into(),
        )));
        assert_eq!(
            installer.failure(),
            Some("could not download the model: timed out")
        );
        installer.finished(&Err(InstallError::Cancelled));
        assert_eq!(installer.failure(), None);
    }

    #[test]
    fn remove_keeps_files_and_reports_error_when_unload_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("model");
        std::fs::create_dir(&model_dir).unwrap();
        let model_file = model_dir.join("model.onnx");
        std::fs::write(&model_file, b"weights").unwrap();

        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let mut installer = Installer::new();
        installer.remove_dir(model_dir.clone(), || false, tx.clone(), ctx.clone());
        assert!(installer.busy());
        let event = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("ModelRemoved event");
        assert!(matches!(event, Event::ModelRemoved(Err(_))));
        assert!(
            model_file.is_file(),
            "model files must not be deleted when unload times out"
        );
        installer.removed();
        assert!(!installer.busy());

        installer.remove_dir(model_dir.clone(), || true, tx, ctx);
        assert!(installer.busy());
        let event = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("ModelRemoved event");
        assert!(matches!(event, Event::ModelRemoved(Ok(()))));
        assert!(!model_dir.exists(), "model directory should be deleted");
        installer.removed();
        assert!(!installer.busy());
    }
}
