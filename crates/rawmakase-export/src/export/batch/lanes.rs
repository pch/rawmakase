//! How many photos of a batch [`run`](super::run) exports at once ([`Lanes`]),
//! and the memory their images may take together ([`Gate`]).
use rawmakase_cancel::Cancel;
use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex, PoisonError},
    time::Duration,
};

/// How a batch runs: up to `photos` photos at once, each on a pool of `threads`
/// threads at background priority, while their images fit in `memory` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lanes {
    pub photos: usize,
    pub threads: usize,
    pub memory: u64,
}

/// Logical CPUs per photo under way. One 24 MP photo kept 5.7 threads busy on
/// average, its decode and encode partly on one thread: 16 photos took 51 s one
/// at a time on 8 threads, 43 s on 16, and 35 s three at a time on 8 each (an
/// 8-core, 16-thread Ryzen 7 8745HS).
const CPUS_PER_PHOTO: usize = 6;
/// The most photos under way, however many CPUs there are.
const MOST_PHOTOS: usize = 8;
/// The most threads a lane has while every lane has a photo.
const MOST_THREADS: usize = 16;
/// The memory a photo is counted as taking, per pixel. Peaks measured 46-57
/// bytes per pixel for 24-66 MP photos exported as JPEG; this leaves room for a
/// 16-bit output, an AI Denoise source or a gain map's base.
pub(super) const BYTES_PER_PIXEL: u64 = 64;
/// The memory budget when the computer's isn't known.
const UNKNOWN_MEMORY_BUDGET: u64 = 4 << 30;

impl Lanes {
    /// For a computer with `logical` CPUs and `memory` bytes: one photo per
    /// [`CPUS_PER_PHOTO`] CPUs, to the nearest, each with half again its share
    /// of the CPUs so a photo's single-threaded stages leave none idle, and half
    /// the memory for their images.
    pub fn for_machine(logical: usize, memory: Option<u64>) -> Self {
        let logical = logical.max(1);
        let photos = ((logical + CPUS_PER_PHOTO / 2) / CPUS_PER_PHOTO).clamp(1, MOST_PHOTOS);
        let threads = (logical * 3)
            .div_ceil(2 * photos)
            .clamp(2, MOST_THREADS)
            .min(logical);
        Self {
            photos,
            threads,
            memory: memory.map_or(UNKNOWN_MEMORY_BUDGET, |m| m / 2),
        }
    }

    /// Threads for each of `under_way` photos, when a batch has fewer photos than
    /// lanes: the threads of the lanes left idle are shared out, up to about the
    /// CPUs [`for_machine`](Self::for_machine) had (two thirds of every lane's).
    pub(super) fn threads_for(&self, under_way: usize) -> usize {
        let all = self.threads * self.photos;
        if under_way >= self.photos {
            return self.threads;
        }
        all.div_ceil(under_way.max(1))
            .min((all * 2 / 3).max(self.threads))
    }
}

/// Admits photos while their memory fits the budget, first come, first served,
/// so a large photo isn't passed over by smaller ones behind it. A photo larger
/// than the whole budget runs alone.
pub(super) struct Gate {
    budget: u64,
    state: Mutex<State>,
    freed: Condvar,
}

#[derive(Default)]
struct State {
    /// The memory admitted photos take.
    used: u64,
    /// How many photos are admitted.
    admitted: usize,
    /// The photos waiting, by ticket, first come first.
    waiting: VecDeque<u64>,
    next_ticket: u64,
}

/// A photo's place in the budget, given back when it is dropped.
pub(super) struct Admitted<'a> {
    gate: &'a Gate,
    bytes: u64,
}

/// How often a waiting photo looks at Cancel.
const CANCEL_POLL: Duration = Duration::from_millis(50);

impl Gate {
    pub(super) fn new(budget: u64) -> Self {
        Self {
            budget,
            state: Mutex::new(State::default()),
            freed: Condvar::new(),
        }
    }

    /// Waits until `bytes` fit beside the photos admitted, or none is, and admits
    /// them; `None` once `cancel` is seen.
    pub(super) fn admit(&self, bytes: u64, cancel: Cancel<'_>) -> Option<Admitted<'_>> {
        let mut state = self.lock();
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.waiting.push_back(ticket);
        loop {
            let first = state.waiting.front() == Some(&ticket);
            if first && (state.admitted == 0 || state.used + bytes <= self.budget) {
                state.waiting.pop_front();
                state.used += bytes;
                state.admitted += 1;
                drop(state);
                // The next in line may fit too.
                self.freed.notify_all();
                return Some(Admitted { gate: self, bytes });
            }
            if cancel.is_cancelled() {
                state.waiting.retain(|t| *t != ticket);
                drop(state);
                self.freed.notify_all();
                return None;
            }
            state = self
                .freed
                .wait_timeout(state, CANCEL_POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Admitted<'_> {
    fn drop(&mut self) {
        let mut state = self.gate.lock();
        state.used -= self.bytes;
        state.admitted -= 1;
        drop(state);
        self.gate.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn lanes_follow_the_cpus_and_memory() {
        let gib = 1u64 << 30;
        let lanes = |logical, memory| {
            let l = Lanes::for_machine(logical, memory);
            (l.photos, l.threads, l.memory)
        };
        // As many CPUs as one photo uses or fewer: one at a time, on all of them.
        assert_eq!(lanes(1, Some(8 * gib)), (1, 1, 4 * gib));
        assert_eq!(lanes(4, Some(8 * gib)), (1, 4, 4 * gib));
        assert_eq!(lanes(8, Some(16 * gib)), (1, 8, 8 * gib));
        assert_eq!(lanes(16, Some(32 * gib)), (3, 8, 16 * gib));
        assert_eq!(lanes(32, Some(64 * gib)), (5, 10, 32 * gib));
        assert_eq!(lanes(128, None), (8, 16, UNKNOWN_MEMORY_BUDGET));
        assert_eq!(lanes(0, None).0, 1);
    }

    #[test]
    fn a_batch_smaller_than_the_lanes_shares_their_threads() {
        let threads = |logical, under_way| Lanes::for_machine(logical, None).threads_for(under_way);
        // 32 CPUs: five lanes of 10 threads; one photo alone gets about all CPUs.
        assert_eq!(threads(32, 5), 10);
        assert_eq!(threads(32, 9), 10);
        assert_eq!(threads(32, 1), 33);
        assert_eq!(threads(32, 2), 25);
        assert_eq!(threads(16, 1), 16);
        assert_eq!(threads(8, 1), 8);
        assert_eq!(threads(4, 1), 4);
    }

    #[test]
    fn a_photo_waits_until_it_fits_and_one_over_the_budget_runs_alone() {
        let gate = Gate::new(100);
        let a = gate.admit(60, Cancel::NEVER).unwrap();
        let b = gate.admit(40, Cancel::NEVER).unwrap();
        std::thread::scope(|s| {
            let waiting = s.spawn(|| gate.admit(300, Cancel::NEVER).map(|p| p.bytes));
            std::thread::sleep(Duration::from_millis(100));
            assert!(!waiting.is_finished(), "admitted beside others over budget");
            drop(a);
            std::thread::sleep(Duration::from_millis(100));
            assert!(!waiting.is_finished(), "admitted beside another");
            drop(b);
            assert_eq!(waiting.join().unwrap(), Some(300));
        });
        assert_eq!(gate.lock().used, 0);
    }

    #[test]
    fn photos_are_admitted_in_the_order_they_came() {
        let gate = Gate::new(100);
        let first = gate.admit(80, Cancel::NEVER).unwrap();
        let (release, released) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|s| {
            // The large one comes first, and stays in until released.
            let gate = &gate;
            let large = s.spawn(move || {
                let _p = gate.admit(90, Cancel::NEVER);
                let _ = released.recv();
            });
            while gate.lock().waiting.is_empty() {
                std::thread::yield_now();
            }
            // The small one would fit beside the first, but waits behind the large
            // one, and goes in beside it.
            let small = s.spawn(|| {
                let _p = gate.admit(10, Cancel::NEVER);
                gate.lock().used
            });
            std::thread::sleep(Duration::from_millis(100));
            assert!(!small.is_finished(), "admitted ahead of the large photo");
            drop(first);
            assert_eq!(small.join().unwrap(), 100);
            drop(release);
            large.join().unwrap();
        });
    }

    #[test]
    fn a_waiting_photo_gives_up_its_place_on_cancel() {
        let gate = Gate::new(100);
        let held = gate.admit(100, Cancel::NEVER).unwrap();
        let cancel = AtomicBool::new(false);
        std::thread::scope(|s| {
            let waiting = s.spawn(|| gate.admit(50, Cancel::new(&cancel)).is_none());
            std::thread::sleep(Duration::from_millis(60));
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            assert!(waiting.join().unwrap(), "admitted after cancel");
        });
        assert!(gate.lock().waiting.is_empty());
        drop(held);
        // The place it left isn't kept: the next photo goes straight in.
        assert!(gate.admit(100, Cancel::NEVER).is_some());
    }

    #[test]
    fn a_panic_gives_the_memory_back() {
        let gate = Gate::new(100);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _admitted = gate.admit(100, Cancel::NEVER);
            panic!("in the photo");
        }));
        let state = gate.lock();
        assert_eq!((state.used, state.admitted), (0, 0));
    }
}
