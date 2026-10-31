//! Bounded output jobs capture a recipe and its identity before leaving the UI
//! thread. Polling a job confirms the file was published, not just queued.
use super::{Error, Result};
use crate::export::{Replace, job};
use crate::export_settings::{ExportSettings, Format};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Running,
    Completed,
    Failed,
}
#[derive(Debug, Clone, Serialize)]
pub(in crate::app) struct OutputState {
    job_id: u64,
    status: Status,
    generation: u64,
    revision: u64,
    path: PathBuf,
    progress: f32,
    notice: Option<String>,
    error: Option<String>,
}
struct Job {
    state: Arc<Mutex<OutputState>>,
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
#[derive(Default)]
pub(super) struct Outputs {
    next: u64,
    jobs: BTreeMap<u64, Job>,
}
impl Drop for Outputs {
    fn drop(&mut self) {
        self.cancel_all();
    }
}
impl Outputs {
    /// Whether job `id` has finished: completed, failed, or unknown.
    pub(crate) fn finished(&self, id: u64) -> bool {
        self.jobs.get(&id).is_none_or(|j| {
            j.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .status
                != Status::Running
        })
    }
    /// The jobs still running, cancelled or not.
    fn running(&self) -> impl Iterator<Item = &Job> {
        self.jobs.values().filter(|j| {
            j.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .status
                == Status::Running
        })
    }
    /// Whether any job not cancelled is still running.
    pub(crate) fn any_running(&self) -> bool {
        self.running()
            .any(|job| !job.cancel.load(Ordering::Relaxed))
    }
    /// A job that stays running until cancelled, with no thread, for tests.
    #[cfg(test)]
    pub(crate) fn hold_running(&mut self) {
        self.next += 1;
        let state = OutputState {
            job_id: self.next,
            status: Status::Running,
            generation: 0,
            revision: 0,
            path: PathBuf::new(),
            progress: 0.,
            notice: None,
            error: None,
        };
        self.jobs.insert(
            self.next,
            Job {
                state: Arc::new(Mutex::new(state)),
                cancel: Arc::default(),
                thread: None,
            },
        );
    }
    /// Cancels every job still running, after the stage it is in.
    pub(crate) fn cancel_all(&self) {
        for job in self.jobs.values() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
    /// Cancels every job still running, to wait for at exit.
    pub(in crate::app) fn stop(&mut self) -> Vec<crate::app::task::Stopping> {
        self.cancel_all();
        self.jobs
            .values_mut()
            .map(|job| crate::app::task::Stopping::new(job.thread.take()))
            .collect()
    }
    pub(crate) fn state(&self, id: u64) -> Result<OutputState> {
        self.jobs
            .get(&id)
            .map(|j| {
                j.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            })
            .ok_or_else(|| {
                Error::new(
                    "unknown_job",
                    "The output job does not exist or has expired",
                )
            })
    }
    pub(crate) fn start(
        &mut self,
        photo: job::Photo,
        path: PathBuf,
        edge: u32,
        generation: u64,
        revision: u64,
        ctx: eframe::egui::Context,
    ) -> Result<OutputState> {
        if self.running().count() >= 2 {
            return Err(Error::new("busy", "Two output jobs are already running"));
        }
        let format = Format::from_path(&path).ok_or_else(|| {
            Error::new(
                "invalid_path",
                format!("Use a {} output path", Format::extensions_listed()),
            )
        })?;
        if !path.is_absolute() {
            return Err(Error::new("invalid_path", "Output path must be absolute"));
        }
        if path.exists() {
            return Err(Error::new(
                "already_exists",
                "Output already exists; choose a new path",
            ));
        }
        if !path.parent().is_some_and(|p| p.is_dir()) {
            return Err(Error::new(
                "invalid_path",
                "Output directory does not exist",
            ));
        }
        while self.jobs.len() >= 32 {
            let old = self
                .jobs
                .iter()
                .find(|(_, j)| {
                    j.state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .status
                        != Status::Running
                })
                .map(|(id, _)| *id);
            if let Some(id) = old {
                self.jobs.remove(&id);
            } else {
                break;
            }
        }
        self.next += 1;
        let id = self.next;
        let initial = OutputState {
            job_id: id,
            status: Status::Running,
            generation,
            revision,
            path: path.clone(),
            progress: 0.,
            notice: None,
            error: None,
        };
        let state = Arc::new(Mutex::new(initial.clone()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (report, stop) = (state.clone(), cancel.clone());
        let thread = std::thread::Builder::new()
            .name("control-output".into())
            .spawn(move || {
                let settings = ExportSettings {
                    format,
                    resize: edge > 0,
                    long_edge: edge.max(1),
                    ..Default::default()
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    job::run(photo, &settings, &path, Replace::NoClobber, &stop, |p| {
                        report
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .progress = p;
                    })
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Output rendering failed unexpectedly")));
                let mut report = report
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match result {
                    Ok(notice) => {
                        report.status = Status::Completed;
                        report.notice = notice;
                        report.progress = 1.;
                    }
                    Err(e) => {
                        report.status = Status::Failed;
                        report.error = Some(format!("{e:#}"));
                    }
                }
                ctx.request_repaint();
            })
            .map_err(|e| Error::new("start_failed", e.to_string()))?;
        self.jobs.insert(
            id,
            Job {
                state,
                cancel,
                thread: Some(thread),
            },
        );
        Ok(initial)
    }
}
