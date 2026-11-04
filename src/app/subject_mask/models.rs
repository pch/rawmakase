//! Installing and removing the model packs (Select Subject's and Sky's, AI Denoise's)
//! on their own threads, only on the user's choice. What an install does is in
//! [`crate::model_install`].
use crate::app::task::Stopping;
use crate::app::worker::Event;
use crate::model_install::{InstallError, Pack, Progress};
use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, mpsc::Sender};
use std::time::Duration;

/// The installs of each pack.
pub(crate) struct Models {
    pub(in crate::app) subject: Installer,
    pub(in crate::app) sky: Installer,
    pub(in crate::app) denoise: Installer,
}
impl Default for Models {
    fn default() -> Self {
        Self {
            subject: Installer::default(),
            sky: Installer {
                pack: Pack::Sky,
                ..Installer::default()
            },
            denoise: Installer {
                pack: Pack::Denoise,
                ..Installer::default()
            },
        }
    }
}
impl Models {
    pub(in crate::app) fn of(&self, pack: Pack) -> &Installer {
        match pack {
            Pack::Subject => &self.subject,
            Pack::Sky => &self.sky,
            Pack::Denoise => &self.denoise,
        }
    }
    pub(in crate::app) fn of_mut(&mut self, pack: Pack) -> &mut Installer {
        match pack {
            Pack::Subject => &mut self.subject,
            Pack::Sky => &mut self.sky,
            Pack::Denoise => &mut self.denoise,
        }
    }
}

/// One pack on disk and the install in progress, if any.
#[derive(Default)]
pub(crate) struct Installer {
    pack: Pack,
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
    pub(in crate::app) fn installed(&self) -> bool {
        if self.removing {
            return false;
        }
        // Looked at again after a second, so the drawer does not stat on every frame.
        match self.seen.get() {
            Some((at, installed)) if at.elapsed() < Duration::from_secs(1) => installed,
            _ => {
                let installed = self.pack.is_installed();
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
        self.installed().then(|| self.pack.dir())
    }
    pub(in crate::app) fn busy(&self) -> bool {
        self.progress.is_some() || self.removing
    }
    /// Bytes fetched so far and the total, while installing.
    pub(in crate::app) fn progress(&self) -> Option<(u64, u64)> {
        self.progress
            .as_ref()
            .map(|p| (p.fetched(), self.pack.bytes()))
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
        let pack = self.pack;
        let progress = Arc::new(Progress::default());
        self.progress = Some(progress.clone());
        self.failed = None;
        let spawned = std::thread::Builder::new()
            .name("model-install".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::model_install::install(pack, import.as_deref(), &progress)
                }))
                .unwrap_or_else(|_| {
                    Err(InstallError::Failed(
                        "the install stopped unexpectedly".into(),
                    ))
                });
                let _ = tx.send(Event::ModelInstalled(pack, result));
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
    /// thread, and the files go once it has. Saved masks keep their pixels.
    pub(in crate::app) fn remove(
        &mut self,
        unload: impl FnOnce() + Send + 'static,
        tx: Sender<Event>,
        ctx: egui::Context,
    ) {
        if self.busy() || !self.installed() {
            return;
        }
        self.removing = true;
        let pack = self.pack;
        std::thread::spawn(move || {
            unload();
            let result = std::fs::remove_dir_all(pack.dir())
                .or_else(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(e)
                    }
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::ModelRemoved(pack, result));
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

    #[test]
    fn a_failed_install_says_why_until_the_next_one() {
        let mut installer = Installer::default();
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
}
