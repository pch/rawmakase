//! Lensfun's lens database in the app: installing it (from Develop's Lens
//! Corrections, or Preferences), checking for and installing a newer one, and removing
//! it (Preferences). The work is `lensfun_install`'s, on a thread of its own, one job
//! at a time; this keeps its progress and outcome for the panels.
use crate::app::shell::messages::Level;
use crate::app::task::Stopping;
use crate::app::worker::Event;
use crate::lens::lensfun::installed::{self, Manifest};
use crate::lensfun_install::{self as install, Installed, Progress, Source};
use eframe::egui;
use std::sync::{Arc, atomic::Ordering, mpsc::Sender};
use std::time::{Duration, Instant};

/// What a job does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Job {
    /// Download and install the newest database.
    Install,
    /// Ask Lensfun for its newest database's timestamp.
    Check,
    Remove,
}

/// How a job ended.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Done {
    Installed(Installed),
    /// Lensfun's newest database's timestamp.
    Checked(i64),
    Removed(bool),
}

struct Running {
    job: Job,
    generation: u64,
    progress: Arc<Progress>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The installed database and the job under way, if any.
#[derive(Default)]
pub(crate) struct Lensfun {
    running: Option<Running>,
    generation: u64,
    /// Lensfun's newest database, as last checked.
    latest: Option<i64>,
    /// Why the last job failed, until the next one starts.
    failed: Option<(Job, String)>,
    /// What the folder held when last looked at: the files decide, not this
    /// process's own jobs (another window, or the CLI, may install or remove).
    seen: std::cell::Cell<Option<(Instant, Option<Manifest>)>>,
    /// The folder, `installed::dir()` unless a test sets another, and where
    /// installs come from (Lensfun's channel, or a test's archive).
    dir: Option<std::path::PathBuf>,
    source: Option<Source>,
    /// Lens Corrections suggests Lensfun profiles (a preference, on by default).
    pub(crate) suggest: bool,
}

impl Job {
    /// What the app is doing, while it does.
    pub(crate) fn running_text(self) -> &'static str {
        match self {
            Job::Install => "Installing Lensfun profiles…",
            Job::Check => "Checking for Lensfun updates…",
            Job::Remove => "Removing Lensfun profiles…",
        }
    }
}

impl Lensfun {
    pub(crate) fn suggesting(suggest: bool) -> Self {
        Self {
            suggest,
            ..Default::default()
        }
    }
    pub(crate) fn dir(&self) -> std::path::PathBuf {
        self.dir.clone().unwrap_or_else(installed::dir)
    }
    #[cfg(test)]
    pub(crate) fn in_folder(dir: std::path::PathBuf, archive: std::path::PathBuf) -> Self {
        Self {
            dir: Some(dir),
            source: Some(Source::File(archive)),
            suggest: true,
            ..Default::default()
        }
    }
    /// The version installed, looked at again after a second.
    pub(crate) fn installed(&self) -> Option<Manifest> {
        if let Some((at, m)) = self.seen.take()
            && at.elapsed() < Duration::from_secs(1)
        {
            self.seen.set(Some((at, m.clone())));
            return m;
        }
        let m = install::current(&self.dir());
        self.seen.set(Some((Instant::now(), m.clone())));
        m
    }
    /// The installed database, read, for matching and choosing lenses.
    pub(crate) fn database(&self) -> Option<Arc<installed::Installed>> {
        match &self.dir {
            None => installed::current(),
            Some(dir) => installed::load(dir).ok().map(Arc::new),
        }
    }
    pub(crate) fn running(&self) -> Option<Job> {
        self.running.as_ref().map(|r| r.job)
    }
    /// Bytes downloaded and the total, when known.
    pub(crate) fn progress(&self) -> Option<(u64, u64)> {
        self.running.as_ref().map(|r| {
            (
                r.progress.done.load(Ordering::Relaxed),
                r.progress.total.load(Ordering::Relaxed),
            )
        })
    }
    /// Why the last job failed, if it did and wasn't cancelled.
    pub(crate) fn failure(&self, job: Job) -> Option<&str> {
        self.failed
            .as_ref()
            .filter(|(j, _)| *j == job)
            .map(|(_, e)| e.as_str())
    }
    /// Lensfun's newest database, when checked and newer than the one installed.
    pub(crate) fn newer(&self) -> Option<i64> {
        let latest = self.latest?;
        self.installed()
            .is_none_or(|m| m.version < latest)
            .then_some(latest)
    }
    /// Whether the last check found the installed version current.
    pub(crate) fn up_to_date(&self) -> bool {
        self.latest
            .is_some_and(|l| self.installed().is_some_and(|m| m.version >= l))
    }
    /// Starts `job`; one at a time.
    pub(crate) fn start(&mut self, job: Job, tx: Sender<Event>, ctx: egui::Context) {
        if self.running.is_some() {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        let progress = Arc::new(Progress::default());
        let dir = self.dir();
        let source = self.source.clone().unwrap_or(Source::Lensfun);
        self.failed = None;
        let worker = Arc::clone(&progress);
        let spawned = std::thread::Builder::new()
            .name("lensfun".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                    Job::Install => install::install(&dir, &source, &worker).map(Done::Installed),
                    Job::Check => match &source {
                        Source::Lensfun => install::latest(&worker).map(Done::Checked),
                        Source::File(_) => Ok(Done::Checked(i64::MAX)),
                    },
                    Job::Remove => install::remove(&dir, &worker).map(Done::Removed),
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("it stopped unexpectedly")));
                let result = result.map_err(|e| {
                    if install::is_cancelled(&e) {
                        Failure::Cancelled
                    } else {
                        Failure::Error(format!("{e:#}"))
                    }
                });
                let _ = tx.send(Event::Lensfun {
                    generation,
                    job,
                    result,
                });
                ctx.request_repaint();
            });
        match spawned {
            Ok(thread) => {
                self.running = Some(Running {
                    job,
                    generation,
                    progress,
                    thread: Some(thread),
                })
            }
            Err(e) => self.failed = Some((job, format!("could not start: {e}"))),
        }
    }
    pub(crate) fn cancel(&self) {
        if let Some(r) = &self.running {
            r.progress.cancel.store(true, Ordering::Relaxed);
        }
    }
    /// A job's event. One from an earlier job (there is only ever one running) is
    /// ignored. Returns what to say.
    pub(crate) fn finished(
        &mut self,
        generation: u64,
        job: Job,
        result: Result<Done, Failure>,
    ) -> Option<(Level, String)> {
        if self
            .running
            .as_ref()
            .is_none_or(|r| r.generation != generation)
        {
            return None;
        }
        self.running = None;
        self.seen.set(None);
        Some(match result {
            Ok(Done::Installed(Installed::New(m))) => {
                self.latest = Some(self.latest.unwrap_or(m.version).max(m.version));
                (
                    Level::Done,
                    format!("Lensfun profiles installed ({})", m.date()),
                )
            }
            Ok(Done::Installed(Installed::Current(m))) => {
                self.latest = Some(m.version);
                (
                    Level::Done,
                    format!("Lensfun profiles are up to date ({})", m.date()),
                )
            }
            Ok(Done::Checked(latest)) => {
                self.latest = Some(latest);
                (
                    Level::Note,
                    match self.newer() {
                        Some(_) => "A newer Lensfun database is available".into(),
                        None => "Lensfun profiles are up to date".into(),
                    },
                )
            }
            Ok(Done::Removed(_)) => (
                Level::Done,
                "Lensfun profiles removed; edits keep their corrections".into(),
            ),
            Err(Failure::Cancelled) => (Level::Note, "Cancelled".into()),
            Err(Failure::Error(e)) => {
                let said = match job {
                    Job::Install => format!("Lensfun profiles not installed: {e}"),
                    Job::Check => format!("Could not check for Lensfun updates: {e}"),
                    Job::Remove => format!("Lensfun profiles not removed: {e}"),
                };
                self.failed = Some((job, e));
                (Level::Failure, said)
            }
        })
    }
    /// Cancels the job and hands its thread to the shared shutdown deadline.
    pub(crate) fn stop(&mut self) -> Vec<Stopping> {
        self.cancel();
        self.running
            .as_mut()
            .map(|r| Stopping::new(r.thread.take()))
            .into_iter()
            .collect()
    }
}

/// Why a job didn't finish.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Failure {
    Cancelled,
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/lensfun")
            .join(name)
    }
    /// Runs `job` and hands back its event, as the window would receive it.
    fn run(l: &mut Lensfun, job: Job) -> (u64, Job, Result<Done, Failure>) {
        let (tx, rx) = std::sync::mpsc::channel();
        l.start(job, tx, egui::Context::default());
        assert_eq!(l.running(), Some(job));
        match rx.recv_timeout(Duration::from_secs(30)).unwrap() {
            Event::Lensfun {
                generation,
                job,
                result,
            } => (generation, job, result),
            _ => panic!("another event"),
        }
    }

    /// An install, a failed one and a removal each end the job, whatever happened,
    /// and say so; the installed version is what the folder holds.
    #[test]
    fn every_job_ends_and_clears_busy() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = Lensfun::in_folder(dir.path().to_path_buf(), archive("good.tar.bz2"));
        assert!(l.installed().is_none());
        let (g, job, result) = run(&mut l, Job::Install);
        let (level, said) = l.finished(g, job, result).unwrap();
        assert_eq!(level, Level::Done);
        assert!(said.starts_with("Lensfun profiles installed"), "{said}");
        assert_eq!(l.running(), None);
        assert_eq!(l.installed().unwrap().version, 1_793_180_205);
        assert!(l.database().is_some());
        // A bad archive fails and leaves the version installed.
        l.source = Some(Source::File(archive("symlink.tar.bz2")));
        let (g, job, result) = run(&mut l, Job::Install);
        let (level, said) = l.finished(g, job, result).unwrap();
        assert_eq!(level, Level::Failure, "{said}");
        assert!(said.contains("not installed"), "{said}");
        assert_eq!(l.running(), None);
        assert!(l.failure(Job::Install).is_some());
        assert_eq!(l.installed().unwrap().version, 1_793_180_205);
        let (g, job, result) = run(&mut l, Job::Remove);
        assert!(l.finished(g, job, result).unwrap().1.contains("removed"));
        assert!(l.installed().is_none() && l.failure(Job::Install).is_none());
    }

    /// A completion that isn't the running job's (an earlier generation) changes
    /// nothing; a cancelled job ends quietly.
    #[test]
    fn stale_and_cancelled_completions() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = Lensfun::in_folder(dir.path().to_path_buf(), archive("good.tar.bz2"));
        let (g, job, result) = run(&mut l, Job::Install);
        assert_eq!(l.finished(g - 1, job, result.clone()), None);
        assert!(l.running().is_some());
        assert!(l.finished(g, job, Err(Failure::Cancelled)).is_some());
        assert_eq!(l.running(), None);
        assert_eq!(l.failure(Job::Install), None);
        // Nothing runs: a late event is ignored.
        assert_eq!(l.finished(g, job, result), None);
        // Stopping hands over the thread of the one running, cancelled.
        let (tx, _rx) = std::sync::mpsc::channel();
        l.start(Job::Install, tx, egui::Context::default());
        let stopping = l.stop();
        assert_eq!(stopping.len(), 1);
    }
}

#[cfg(test)]
#[path = "lensfun_editor_tests.rs"]
mod editor_tests;
