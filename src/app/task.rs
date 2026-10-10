//! Generation and cancellation belong to the operation that owns them; [`spawn`]
//! runs one-off background work so that a panic still reaches the UI.
use super::worker::{Event, panic_message};
use eframe::egui;
use std::{
    panic::{self, AssertUnwindSafe},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

/// Runs `work` on a thread of its own; it reports through the events it sends.
/// Should it panic, `failed` sends whatever the UI needs to stop waiting for it,
/// given the panic's message. Either way the UI repaints afterwards to read them.
pub(super) fn spawn(
    events: Sender<Event>,
    ctx: egui::Context,
    work: impl FnOnce(&Sender<Event>) + Send + 'static,
    failed: impl FnOnce(&Sender<Event>, String) + Send + 'static,
) {
    std::thread::spawn(move || {
        if let Err(panic) = panic::catch_unwind(AssertUnwindSafe(|| work(&events))) {
            let message = format!("Stopped unexpectedly: {}", panic_message(&*panic));
            failed(&events, message);
        }
        ctx.request_repaint();
    });
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Running,
}

/// A worker asked to stop, to wait for at exit (see docs/shutdown.md).
pub(super) struct Stopping(Option<std::thread::JoinHandle<()>>);
impl Stopping {
    pub(crate) fn new(thread: Option<std::thread::JoinHandle<()>>) -> Self {
        Self(thread)
    }
    fn finished(&self) -> bool {
        self.0
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
    }
}
/// How waiting for stopping workers ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Waited {
    pub finished: usize,
    /// Still running at the deadline, and left to end with the process.
    pub detached: usize,
}
/// Waits until every worker has finished, or `deadline` passes. A worker blocked
/// on a stalled network share must not hold up quitting, so none is joined
/// without one.
pub(super) fn wait_for(workers: Vec<Stopping>, deadline: std::time::Duration) -> Waited {
    let until = std::time::Instant::now() + deadline;
    while !workers.iter().all(Stopping::finished) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let finished = workers.iter().filter(|w| w.finished()).count();
    Waited {
        finished,
        detached: workers.len() - finished,
    }
}

#[derive(Default)]
pub(super) struct Task {
    generation: u64,
    /// The oldest generation whose results still count: the last `start`. Jobs
    /// `supersede` started after it share its cancel flag and may still report.
    floor: u64,
    cancel: Arc<AtomicBool>,
    phase: Phase,
}
impl Task {
    pub(crate) fn id(&self) -> u64 {
        self.generation
    }
    pub(crate) fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running)
    }
    pub(crate) fn start(&mut self) -> (u64, Arc<AtomicBool>) {
        self.invalidate();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.phase = Phase::Running;
        (self.generation, self.cancel.clone())
    }
    /// Starts a newer job without cancelling the running ones, whose results then
    /// still count (see `counts`) until the next `start` or `invalidate` cancels them
    /// all. For a change whose earlier results are still worth showing on the way.
    pub(crate) fn supersede(&mut self) -> (u64, Arc<AtomicBool>) {
        self.generation += 1;
        self.phase = Phase::Running;
        (self.generation, self.cancel.clone())
    }
    /// Whether a result of job `generation` is still wanted: the latest job's, or an
    /// earlier one `supersede` let run.
    pub(crate) fn counts(&self, generation: u64) -> bool {
        (self.floor..=self.generation).contains(&generation)
    }
    pub(crate) fn invalidate(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.generation += 1;
        self.floor = self.generation;
        self.phase = Phase::Idle;
    }
    pub(crate) fn finish(&mut self, generation: u64) {
        if generation == self.generation {
            self.phase = Phase::Idle;
        }
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waiting_for_workers_stops_at_the_deadline() {
        use std::time::Duration;
        let quick = Stopping::new(Some(std::thread::spawn(|| {})));
        let (release, stalled) = std::sync::mpsc::channel::<()>();
        // A read stalled on a network share.
        let slow = Stopping::new(Some(std::thread::spawn(move || {
            let _ = stalled.recv();
        })));
        let started = std::time::Instant::now();
        let waited = wait_for(
            vec![quick, slow, Stopping::new(None)],
            Duration::from_millis(50),
        );
        assert_eq!(
            waited,
            Waited {
                finished: 2,
                detached: 1
            }
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(release);
    }
    use std::time::Duration;

    #[test]
    fn spawned_work_reports_its_panic() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn(
            tx,
            egui::Context::default(),
            |_| panic!("scan failed"),
            |events, message| {
                let _ = events.send(Event::CatalogWorking(message));
            },
        );
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Event::CatalogWorking(message)) => {
                assert_eq!(message, "Stopped unexpectedly: scan failed");
            }
            _ => panic!("no failure reported"),
        }
    }
    #[test]
    fn spawned_work_that_finishes_reports_nothing_more() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn(
            tx,
            egui::Context::default(),
            |events| {
                let _ = events.send(Event::DialogClosed);
            },
            |_, message| panic!("{message}"),
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(Event::DialogClosed)
        ));
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_err());
    }
    #[test]
    fn superseded_and_dropped_tasks_are_cancelled_and_stale_completion_is_ignored() {
        let mut task = Task::default();
        let (first, cancelled) = task.start();
        let (second, active) = task.start();
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(!active.load(Ordering::Relaxed));
        task.finish(first);
        assert!(task.is_running());
        task.finish(second);
        assert!(!task.is_running());
        let (_, active) = task.start();
        drop(task);
        assert!(active.load(Ordering::Relaxed));
    }

    #[test]
    fn a_superseded_job_runs_on_and_counts_until_the_next_start() {
        let mut task = Task::default();
        let (first, cancel) = task.start();
        let (second, _) = task.supersede();
        assert!(second > first);
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(task.counts(first) && task.counts(second));
        let (third, _) = task.start();
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!task.counts(first) && !task.counts(second));
        assert!(task.counts(third));
        task.invalidate();
        assert!(!task.counts(third));
    }
}
