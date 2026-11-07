//! Autosave writes on a thread of its own, with its own catalog connection:
//! a catalog commit waits for the disk (fsync), which on a busy disk took long
//! enough to stall the interface mid-edit. Saves before navigation stay
//! synchronous, after waiting for the one in flight.
use crate::{
    catalog::{Catalog, CatalogLocation, PhotoId, SavedHistory},
    export_settings::ExportOptions,
    model::recipe::Recipe,
    storage::Identity,
};
use eframe::egui;
use std::{
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    time::Instant,
};

/// An edit to write, as it was when the save started.
pub(in crate::app) struct Job {
    /// The catalog the edit goes to, and the photo in it.
    pub catalog: CatalogLocation,
    pub photo: PhotoId,
    pub raw: PathBuf,
    /// The file as it was when the photo was opened: a file replaced since gets
    /// none of the edit.
    pub source: Identity,
    /// Whether the photo was opened from its editable preview: `source` is then
    /// the identity that recorded, standing in for `raw` while it is not there.
    pub from_editable_preview: bool,
    pub recipe: Recipe,
    pub export: ExportOptions,
    /// Its Develop History, saved in the same transaction.
    pub history: SavedHistory,
}
/// How a background save ended.
#[derive(Debug)]
pub(in crate::app) enum Completion {
    /// Written to the catalog at this path.
    Saved(CatalogLocation),
    /// The save failed, or panicked; the saver keeps running.
    Failed(String),
    /// The saver thread is gone without reporting. The next save starts a new one.
    WorkerLost,
}
impl Completion {
    /// Where the edit was saved, or the error to show.
    pub(crate) fn into_result(self) -> Result<CatalogLocation, String> {
        match self {
            Self::Saved(path) => Ok(path),
            Self::Failed(error) => Err(error),
            Self::WorkerLost => Err("The autosave thread stopped".into()),
        }
    }
}

/// Writes one job, reusing the catalog connection between jobs.
type Saver = fn(&mut Option<Catalog>, &Job) -> anyhow::Result<CatalogLocation>;

/// The saver thread's two ends, held while it runs.
struct Worker {
    jobs: Sender<Job>,
    completions: Receiver<Completion>,
    /// None in tests that stand in for a worker.
    thread: Option<std::thread::JoinHandle<()>>,
}

pub(in crate::app) struct Autosave {
    worker: Option<Worker>,
    in_flight: bool,
    saver: Saver,
}
impl Default for Autosave {
    fn default() -> Self {
        Self {
            worker: None,
            in_flight: false,
            saver: save,
        }
    }
}
impl Autosave {
    /// Starts saving `job`, or hands it back if the saver cannot run.
    pub(crate) fn submit(&mut self, job: Job, ctx: &egui::Context) -> Result<(), Box<Job>> {
        debug_assert!(!self.in_flight);
        if self.worker.is_none() {
            let (jobs, rx) = mpsc::channel();
            let (tx, completions) = mpsc::channel();
            let ctx = ctx.clone();
            let saver = self.saver;
            let spawned = std::thread::Builder::new()
                .name("autosave".into())
                .spawn(move || run(rx, tx, ctx, saver));
            let Ok(thread) = spawned else {
                return Err(Box::new(job));
            };
            self.worker = Some(Worker {
                jobs,
                completions,
                thread: Some(thread),
            });
        }
        let worker = self.worker.as_ref().expect("started above");
        if let Err(mpsc::SendError(job)) = worker.jobs.send(job) {
            self.worker = None;
            return Err(Box::new(job));
        }
        self.in_flight = true;
        Ok(())
    }
    pub(crate) fn busy(&self) -> bool {
        self.in_flight
    }
    /// Drops the saver's channels at exit: it ends once the save in flight, if any,
    /// is written, and is then waited for.
    pub(in crate::app) fn close(&mut self) -> crate::app::task::Stopping {
        self.in_flight = false;
        let thread = self.worker.take().and_then(|worker| worker.thread);
        crate::app::task::Stopping::new(thread)
    }
    /// The finished save, if one finished.
    pub(crate) fn poll(&mut self) -> Option<Completion> {
        self.receive(|completions| match completions.try_recv() {
            Ok(completion) => Some(completion),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Completion::WorkerLost),
        })
    }
    /// The save in flight, once it finishes.
    pub(crate) fn wait(&mut self) -> Option<Completion> {
        self.receive(|completions| Some(completions.recv().unwrap_or(Completion::WorkerLost)))
    }
    /// The save in flight, if it finishes before `until`; still [`busy`](Self::busy)
    /// if it does not.
    pub(crate) fn wait_until(&mut self, until: Instant) -> Option<Completion> {
        self.receive(|completions| {
            match completions.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(completion) => Some(completion),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => Some(Completion::WorkerLost),
            }
        })
    }
    fn receive(
        &mut self,
        get: impl FnOnce(&Receiver<Completion>) -> Option<Completion>,
    ) -> Option<Completion> {
        if !self.in_flight {
            return None;
        }
        let Some(worker) = &self.worker else {
            self.in_flight = false;
            return Some(Completion::WorkerLost);
        };
        let completion = get(&worker.completions)?;
        self.in_flight = false;
        if matches!(completion, Completion::WorkerLost) {
            self.worker = None;
        }
        Some(completion)
    }
}

fn run(jobs: Receiver<Job>, completions: Sender<Completion>, ctx: egui::Context, saver: Saver) {
    let mut catalog = None;
    for job in jobs {
        let saved = panic::catch_unwind(AssertUnwindSafe(|| saver(&mut catalog, &job)));
        let completion = match saved {
            Ok(Ok(path)) => Completion::Saved(path),
            Ok(Err(e)) => Completion::Failed(e.to_string()),
            Err(_) => {
                // The connection may be mid-transaction; the next job reopens it.
                catalog = None;
                Completion::Failed("Saving stopped unexpectedly".into())
            }
        };
        if completions.send(completion).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Keeps the catalog open between saves.
fn save(catalog: &mut Option<Catalog>, job: &Job) -> anyhow::Result<CatalogLocation> {
    let location = &job.catalog;
    if catalog
        .as_ref()
        .is_none_or(|c: &Catalog| c.location() != location)
    {
        *catalog = None;
        *catalog = Some(Catalog::open(location)?);
    }
    let c = catalog.as_mut().expect("opened above");
    let original = crate::edits::Original {
        path: &job.raw,
        recorded: job.from_editable_preview.then_some(&job.source),
    };
    c.save_edit(
        job.photo,
        original,
        &job.source,
        &job.recipe,
        &job.export,
        crate::catalog::HistoryUpdate::of(&job.history),
    )?;
    Ok(location.clone())
}

#[cfg(test)]
impl Autosave {
    /// An autosave whose saves take a minute, as on a stalled network share.
    pub(in crate::app) fn stalled() -> Self {
        fn stall(_: &mut Option<Catalog>, job: &Job) -> anyhow::Result<CatalogLocation> {
            std::thread::sleep(std::time::Duration::from_secs(60));
            Ok(job.catalog.clone())
        }
        Self {
            saver: stall,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn job(photo: PhotoId) -> Job {
        Job {
            catalog: CatalogLocation::File("test.rawmakase".into()),
            photo,
            raw: PathBuf::from("image.ARW"),
            source: Identity {
                size: 0,
                modified_ns: 0,
                prefix_hash: String::new(),
            },
            from_editable_preview: false,
            recipe: Recipe::default(),
            export: ExportOptions::default(),
            history: SavedHistory {
                origin: Recipe::default(),
                steps: Vec::new(),
                applied: 0,
            },
        }
    }

    /// Panics on photo 1 and saves every other photo.
    fn panics_on_photo_one(_: &mut Option<Catalog>, job: &Job) -> anyhow::Result<CatalogLocation> {
        assert_ne!(job.photo, PhotoId(1), "save panicked");
        Ok(job.catalog.clone())
    }

    fn finish(autosave: &mut Autosave) -> Completion {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(completion) = autosave.poll() {
                return completion;
            }
            assert!(Instant::now() < deadline, "the save never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_panicking_save_fails_and_the_next_save_runs() {
        let ctx = egui::Context::default();
        let mut autosave = Autosave {
            saver: panics_on_photo_one,
            ..Autosave::default()
        };
        assert!(autosave.submit(job(PhotoId(1)), &ctx).is_ok());
        let completion = finish(&mut autosave);
        assert!(
            matches!(completion, Completion::Failed(_)),
            "{completion:?}"
        );
        assert!(!autosave.busy());

        assert!(autosave.submit(job(PhotoId(2)), &ctx).is_ok());
        let completion = finish(&mut autosave);
        assert!(matches!(completion, Completion::Saved(_)), "{completion:?}");
    }

    #[test]
    fn a_lost_worker_clears_busy_and_is_replaced() {
        let (jobs, _) = mpsc::channel();
        let (_, completions) = mpsc::channel();
        let mut autosave = Autosave {
            worker: Some(Worker {
                jobs,
                completions,
                thread: None,
            }),
            in_flight: true,
            saver: panics_on_photo_one,
        };
        assert!(matches!(autosave.poll(), Some(Completion::WorkerLost)));
        assert!(!autosave.busy());
        assert!(autosave.worker.is_none());

        let ctx = egui::Context::default();
        assert!(autosave.submit(job(PhotoId(2)), &ctx).is_ok());
        assert!(matches!(finish(&mut autosave), Completion::Saved(_)));
    }

    #[test]
    fn waiting_on_a_lost_worker_reports_it() {
        let (jobs, _) = mpsc::channel();
        let (_, completions) = mpsc::channel();
        let mut autosave = Autosave {
            worker: Some(Worker {
                jobs,
                completions,
                thread: None,
            }),
            in_flight: true,
            saver: save,
        };
        assert!(matches!(autosave.wait(), Some(Completion::WorkerLost)));
        assert!(!autosave.busy());
    }

    #[test]
    fn waiting_for_a_stalled_save_gives_up_at_the_deadline() {
        let ctx = egui::Context::default();
        let mut autosave = Autosave::stalled();
        assert!(autosave.submit(job(PhotoId(1)), &ctx).is_ok());
        let started = Instant::now();
        let until = started + Duration::from_millis(50);
        assert!(autosave.wait_until(until).is_none());
        assert!(Instant::now() >= until);
        assert!(started.elapsed() < Duration::from_secs(30));
        assert!(autosave.busy(), "the save is still in flight");
    }
}

#[cfg(test)]
#[path = "autosave_editor_tests.rs"]
mod editor_tests;
