//! Single-slot mailbox: pending work is replaced by the latest request. A mailbox
//! can have several lanes, each keeping its own latest request; the worker takes
//! the first lane's work first.
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
struct Slot<T> {
    /// The latest job of each lane.
    jobs: Mutex<Vec<Option<T>>>,
    wake: Condvar,
    stopped: AtomicBool,
}
pub(crate) struct Latest<T> {
    slot: Arc<Slot<T>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl<T: Send + 'static> Latest<T> {
    pub(crate) fn new(run: impl FnMut(T) + Send + 'static) -> Self {
        Self::with_lanes(1, run)
    }
    /// A mailbox with `lanes` lanes, which `submit_to` addresses.
    pub(crate) fn with_lanes(lanes: usize, mut run: impl FnMut(T) + Send + 'static) -> Self {
        let slot = Arc::new(Slot {
            jobs: Mutex::new((0..lanes.max(1)).map(|_| None).collect()),
            wake: Condvar::new(),
            stopped: AtomicBool::new(false),
        });
        let worker = slot.clone();
        let thread = std::thread::spawn(move || {
            loop {
                let mut lock = worker.jobs.lock().unwrap();
                while lock.iter().all(Option::is_none) && !worker.stopped.load(Ordering::Relaxed) {
                    lock = worker.wake.wait(lock).unwrap();
                }
                if worker.stopped.load(Ordering::Relaxed) {
                    break;
                }
                let job = lock.iter_mut().find_map(Option::take).unwrap();
                drop(lock);
                // A panic must not end the thread, or later jobs would wait forever.
                // Workers with state catch their own panics to report and rebuild it.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job)));
            }
        });
        Self {
            slot,
            thread: Some(thread),
        }
    }
    /// Stops the worker once its current job is done; pending jobs never run.
    pub(in crate::app) fn stop(&mut self) -> crate::app::task::Stopping {
        self.signal_stop();
        crate::app::task::Stopping::new(self.thread.take())
    }

    pub(crate) fn submit(&self, job: T) {
        self.submit_to(0, job);
    }
    /// Replaces the pending job of `lane`, leaving the other lanes' work queued.
    /// Returns the job it replaced, which never runs.
    pub(crate) fn submit_to(&self, lane: usize, job: T) -> Option<T> {
        self.slot.submit_to(lane, job)
    }
    /// Where another worker submits jobs to this one, which stays owned here.
    pub(crate) fn mailbox(&self) -> Mailbox<T> {
        Mailbox(self.slot.clone())
    }
}
/// Submits jobs to a [`Latest`] worker without owning it: stopping and waiting
/// for the worker stay with its owner. Jobs submitted once it has stopped never run.
pub(crate) struct Mailbox<T>(Arc<Slot<T>>);
impl<T> Mailbox<T> {
    pub(crate) fn submit(&self, job: T) {
        self.0.submit_to(0, job);
    }
}
impl<T> Slot<T> {
    fn submit_to(&self, lane: usize, job: T) -> Option<T> {
        let mut jobs = self.jobs.lock().unwrap();
        let last = jobs.len() - 1;
        let replaced = jobs[lane.min(last)].replace(job);
        drop(jobs);
        self.wake.notify_one();
        replaced
    }
}
/// The message a caught panic carries.
pub(crate) fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}
impl<T> Latest<T> {
    fn signal_stop(&self) {
        let _guard = self.slot.jobs.lock().unwrap();
        self.slot.stopped.store(true, Ordering::Relaxed);
        self.slot.wake.notify_one();
    }
}
impl<T> Drop for Latest<T> {
    fn drop(&mut self) {
        self.signal_stop();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_stopped_worker_finishes_its_job_and_runs_no_pending_one() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let mut worker = Latest::new(move |i: u32| {
            started_tx.send(i).unwrap();
            if i == 1 {
                let _ = release_rx.recv();
            }
        });
        worker.submit(1);
        assert_eq!(started_rx.recv().unwrap(), 1);
        worker.submit(2);
        let stopping = worker.stop();
        drop(release_tx);
        let waited = crate::app::task::wait_for(vec![stopping], std::time::Duration::from_secs(10));
        assert_eq!(waited.detached, 0);
        assert!(started_rx.try_recv().is_err(), "the pending job never ran");
    }
    #[test]
    fn pending_jobs_are_coalesced() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = Latest::new(move |i: u32| {
            started_tx.send(i).unwrap();
            if i == 1 {
                release_rx.recv().unwrap();
            }
        });
        worker.submit(1);
        assert_eq!(started_rx.recv().unwrap(), 1);
        worker.submit(2);
        worker.submit(3);
        release_tx.send(()).unwrap();
        assert_eq!(
            started_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            3
        );
    }
    #[test]
    fn each_lane_keeps_its_latest_job_and_the_first_lane_goes_first() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = Latest::with_lanes(2, move |i: u32| {
            started_tx.send(i).unwrap();
            if i == 1 {
                release_rx.recv().unwrap();
            }
        });
        worker.submit_to(0, 1);
        assert_eq!(started_rx.recv().unwrap(), 1);
        // Queued while 1 runs: lane 1's job is not replaced by lane 0's, and each
        // submission returns the job it replaced, never the running one.
        assert_eq!(worker.submit_to(1, 10), None);
        assert_eq!(worker.submit_to(0, 2), None);
        assert_eq!(worker.submit_to(0, 3), Some(2));
        release_tx.send(()).unwrap();
        let next = || {
            started_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
        };
        assert_eq!(next(), 3);
        assert_eq!(next(), 10);
    }
    #[test]
    fn a_panicking_job_does_not_stop_the_worker() {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = Latest::new(move |i: u32| {
            tx.send(i).unwrap();
            assert_ne!(i, 1, "job 1 panics");
        });
        let next = || rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        worker.submit(1);
        assert_eq!(next(), 1);
        worker.submit(2);
        assert_eq!(next(), 2);
    }
}
