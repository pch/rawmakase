//! The app's exports, one batch at a time: an Export chosen while another runs
//! waits its turn rather than running beside it, so however many are queued, only
//! the running batch's photos have images in memory, within its
//! [`Lanes`]' budget. A waiting batch holds edits, not images, and can be removed
//! before it starts.
use super::batch::{self, Batch, Lanes, Outcome, Progress};
use crate::develop::{ExportBackend, ExportBackendStatus};
use rawmakase_cancel::Cancel;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// The queue of export batches and the thread that runs them.
pub struct Queue {
    shared: Arc<Shared>,
    backend: Arc<ExportBackend>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    changed: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct State {
    waiting: VecDeque<(u64, Batch)>,
    running: Option<Running>,
    next: u64,
    closed: bool,
}

struct Running {
    ticket: u64,
    cancel: Arc<AtomicBool>,
    progress: Progress,
}

/// What the queue is doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// The batch running, and how far it is.
    pub running: Option<(u64, Progress)>,
    /// The batches waiting, by ticket, with how many photos each has.
    pub waiting: Vec<(u64, usize)>,
}

impl Queue {
    /// Starts the queue's thread, which runs each batch on `lanes`; `finished`
    /// gets each batch's outcomes, in its photos' order, and `changed` is called
    /// whenever its status changes.
    pub fn new(
        lanes: Lanes,
        finished: impl Fn(u64, Vec<Outcome>) + Send + 'static,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::with_each(lanes, |_, _, _| {}, finished, changed)
    }

    /// [`Queue::new`], also telling `each` every photo's outcome as soon as
    /// it has one (see [`batch::run_each`]), by ticket and index, before its
    /// batch's `finished`. Runs on the CPU reference backend.
    pub fn with_each(
        lanes: Lanes,
        each: impl Fn(u64, usize, &Outcome) + Send + Sync + 'static,
        finished: impl Fn(u64, Vec<Outcome>) + Send + 'static,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::with_backend(
            lanes,
            Arc::new(ExportBackend::cpu_only()),
            each,
            finished,
            changed,
        )
    }

    /// [`Queue::with_each`], rendering SDR exports through `backend`, shared across
    /// every batch and lane of the queue. Releases reusable GPU working buffers once
    /// the queue drains to idle so finished exports do not hold VRAM.
    pub fn with_backend(
        lanes: Lanes,
        backend: Arc<ExportBackend>,
        each: impl Fn(u64, usize, &Outcome) + Send + Sync + 'static,
        finished: impl Fn(u64, Vec<Outcome>) + Send + 'static,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            changed: Box::new(changed),
        });
        let worker = shared.clone();
        let worker_backend = backend.clone();
        let thread = std::thread::spawn(move || {
            while let Some((ticket, batch, cancel)) = worker.next() {
                // Each photo's panic is caught by the batch, which keeps the others'
                // outcomes. A panic outside them (here, the status callback) fails the
                // whole batch, so the queue still clears it and runs the next one.
                let outcomes = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (worker.changed)();
                    batch::run_each_with_backend(
                        &batch,
                        lanes,
                        &worker_backend,
                        Cancel::new(&cancel),
                        |progress| {
                            if let Ok(mut state) = worker.state.lock()
                                && let Some(running) = &mut state.running
                            {
                                running.progress = progress;
                            }
                            (worker.changed)();
                        },
                        |index, outcome| each(ticket, index, outcome),
                    )
                }))
                .unwrap_or_else(|_| {
                    let failed = Outcome::Failed("the export failed unexpectedly".into());
                    vec![failed; batch.photos.len()]
                });
                let idle = if let Ok(mut state) = worker.state.lock() {
                    state.running = None;
                    state.waiting.is_empty()
                } else {
                    true
                };
                finished(ticket, outcomes);
                (worker.changed)();
                if idle {
                    worker_backend.release_buffers();
                }
            }
            worker_backend.release_buffers();
        });
        Self {
            shared,
            backend,
            thread: Some(thread),
        }
    }

    /// Snapshot of this queue's export backend state and cumulative telemetry.
    /// Never blocks on an in-flight GPU export.
    pub fn backend_status(&self) -> ExportBackendStatus {
        self.backend.status()
    }

    /// Queues `batch`, returning its ticket.
    pub fn submit(&self, batch: Batch) -> u64 {
        let mut state = self.shared.state.lock().expect("export queue");
        state.next += 1;
        let ticket = state.next;
        state.waiting.push_back((ticket, batch));
        drop(state);
        self.shared.wake.notify_one();
        (self.shared.changed)();
        ticket
    }

    /// Removes a waiting batch; false when it has already started or finished.
    pub fn remove(&self, ticket: u64) -> bool {
        let mut state = self.shared.state.lock().expect("export queue");
        let before = state.waiting.len();
        state.waiting.retain(|(t, _)| *t != ticket);
        let removed = state.waiting.len() != before;
        drop(state);
        if removed {
            (self.shared.changed)();
        }
        removed
    }

    /// Cancels the running batch if it is `ticket`; false otherwise.
    pub fn cancel(&self, ticket: u64) -> bool {
        let state = self.shared.state.lock().expect("export queue");
        match &state.running {
            Some(running) if running.ticket == ticket => {
                running.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    pub fn status(&self) -> Status {
        let state = self.shared.state.lock().expect("export queue");
        Status {
            running: state.running.as_ref().map(|r| (r.ticket, r.progress)),
            waiting: state
                .waiting
                .iter()
                .map(|(ticket, batch)| (*ticket, batch.photos.len()))
                .collect(),
        }
    }

    /// Whether a batch is running or waiting.
    pub fn busy(&self) -> bool {
        let state = self.shared.state.lock().expect("export queue");
        state.running.is_some() || !state.waiting.is_empty()
    }
}

impl Shared {
    /// The next batch to run, once there is one; `None` once the queue is dropped.
    fn next(&self) -> Option<(u64, Batch, Arc<AtomicBool>)> {
        let mut state = self.state.lock().ok()?;
        loop {
            if state.closed {
                return None;
            }
            if let Some((ticket, batch)) = state.waiting.pop_front() {
                let cancel = Arc::new(AtomicBool::new(false));
                state.running = Some(Running {
                    ticket,
                    cancel: cancel.clone(),
                    progress: Progress {
                        done: 0,
                        total: batch.photos.len(),
                        fraction: 0.,
                    },
                });
                return Some((ticket, batch, cancel));
            }
            state = self.wake.wait(state).ok()?;
        }
    }
}

impl Queue {
    /// Stops the queue at exit: the running batch is cancelled after the stage it
    /// is in, and waiting ones never start. Returns the thread, to wait for.
    pub fn close(&mut self) -> Option<std::thread::JoinHandle<()>> {
        if let Ok(state) = self.shared.state.lock()
            && let Some(running) = &state.running
        {
            running.cancel.store(true, Ordering::Relaxed);
        }
        self.stop_after_running();
        self.thread.take()
    }
    fn stop_after_running(&self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.closed = true;
            state.waiting.clear();
        }
        self.shared.wake.notify_all();
    }
}

impl Drop for Queue {
    /// Stops the thread once the running batch is done; waiting ones never start.
    fn drop(&mut self) {
        self.stop_after_running();
    }
}
