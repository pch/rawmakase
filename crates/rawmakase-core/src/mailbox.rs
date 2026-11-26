//! What the render loop ([`crate::RenderLoop`]) keeps beside a renderer: a
//! single-slot mailbox of edits, and the rules for which frame is shown.
//!
//! An editor keeps one render thread and gives it only the newest edit: the
//! one waiting is replaced, the one running finishes and shows. Each edit put
//! in the mailbox is numbered, and a frame carries the number of the edit it
//! renders, so [`Shown`] can drop a frame older than the one on screen.
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A single-slot mailbox: [`Mailbox::put`] replaces what waits, and
/// [`Mailbox::take`] gives the render thread the newest. `Send` and `Sync`,
/// to share behind an `Arc`.
#[derive(Debug)]
pub(crate) struct Mailbox<T> {
    state: Mutex<State<T>>,
    wake: Condvar,
}

#[derive(Debug)]
struct State<T> {
    waiting: Option<Numbered<T>>,
    /// The number the next edit gets: one counter for every photo, so a new
    /// photo's first frame is numbered after the edits made before it.
    next: u64,
    closed: bool,
}

/// What [`Mailbox::take_within`] gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Taken<T> {
    /// A newer edit, to render in place of the draft's frame.
    Job(Numbered<T>),
    /// None came: render the draft's frame now.
    Quiet,
    /// The mailbox was closed.
    Closed,
}

/// An edit taken from a [`Mailbox`], with its number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Numbered<T> {
    /// Its place among the edits put in the mailbox, from 1.
    pub(crate) number: u64,
    /// The edit.
    pub(crate) job: T,
}

impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                waiting: None,
                next: 1,
                closed: false,
            }),
            wake: Condvar::new(),
        }
    }
}

impl<T> Mailbox<T> {
    /// An empty mailbox.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Puts `job` in the mailbox in place of any waiting, and returns its
    /// number. Once the mailbox is closed, the job is dropped.
    pub(crate) fn put(&self, job: T) -> u64 {
        let mut state = self.lock();
        let number = state.next;
        state.next += 1;
        let displaced = if state.closed {
            Some(job)
        } else {
            let replaced = state.waiting.replace(Numbered { number, job });
            self.wake.notify_one();
            replaced.map(|n| n.job)
        };
        // Dropped once the lock is let go and the render thread is woken.
        drop(state);
        drop(displaced);
        number
    }

    /// The newest job, waiting until one is put; `None` once the mailbox is
    /// closed, which ends the render thread's loop.
    pub(crate) fn take(&self) -> Option<Numbered<T>> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some(job) = state.waiting.take() {
                return Some(job);
            }
            state = self
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The newest job put within `quiet`: what a render thread asks after
    /// showing a draft, so that the draft's frame is rendered only once no
    /// newer edit has come for that long, and a newer edit goes first.
    pub(crate) fn take_within(&self, quiet: Duration) -> Taken<T> {
        // A quiet time too long for the clock is no deadline.
        let deadline = Instant::now().checked_add(quiet);
        let mut state = self.lock();
        loop {
            if state.closed {
                return Taken::Closed;
            }
            if let Some(job) = state.waiting.take() {
                return Taken::Job(job);
            }
            let Some(deadline) = deadline else {
                state = self
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            };
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Taken::Quiet;
            }
            state = self
                .wake
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The newest job, if one waits, without waiting.
    #[cfg(test)]
    pub(crate) fn try_take(&self) -> Option<Numbered<T>> {
        self.lock().waiting.take()
    }

    /// Closes the mailbox: what waits is dropped, and [`Mailbox::take`]
    /// returns `None` from now on.
    pub(crate) fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        let dropped = state.waiting.take();
        self.wake.notify_all();
        drop(state);
        drop(dropped);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Which frame a view shows, by the number of the edit it renders
/// ([`Numbered::number`]): the rules every front end repeats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Shown {
    shown: Option<(u64, bool)>,
}

impl Shown {
    /// Whether a frame of edit `number` (`draft` for a reduced one) is shown
    /// in place of the frame on screen, and if so, records it. A frame older
    /// than the one shown is dropped, and a draft never replaces its own
    /// edit's full frame; the full frame replaces its draft.
    pub(crate) fn accept(&mut self, number: u64, draft: bool) -> bool {
        let taken = match self.shown {
            None => true,
            Some((shown, _)) if number > shown => true,
            Some((shown, shown_draft)) if number == shown => shown_draft && !draft,
            Some(_) => false,
        };
        if taken {
            self.shown = Some((number, draft));
        }
        taken
    }

    /// The number of the edit whose frame is shown, if any.
    #[cfg(test)]
    pub(crate) fn number(&self) -> Option<u64> {
        self.shown.map(|(n, _)| n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_edit_replaces_the_one_waiting() {
        let mailbox = Mailbox::new();
        assert_eq!(mailbox.put("a"), 1);
        assert_eq!(mailbox.put("b"), 2);
        assert_eq!(
            mailbox.take(),
            Some(Numbered {
                number: 2,
                job: "b"
            })
        );
        assert_eq!(mailbox.try_take(), None);
        mailbox.put("c");
        mailbox.close();
        assert_eq!(mailbox.take(), None, "closed: what waited is dropped");
        assert_eq!(mailbox.put("d"), 4, "numbers go on, the job is dropped");
        assert_eq!(mailbox.try_take(), None);
    }

    #[test]
    fn after_a_draft_a_newer_edit_goes_first_or_the_frame_follows() {
        let mailbox = Mailbox::new();
        let quiet = Duration::from_millis(20);
        assert_eq!(mailbox.take_within(quiet), Taken::Quiet);
        mailbox.put(1);
        assert_eq!(
            mailbox.take_within(quiet),
            Taken::Job(Numbered { number: 1, job: 1 })
        );
        mailbox.put(2);
        assert!(matches!(mailbox.take_within(Duration::MAX), Taken::Job(_)));
        mailbox.close();
        assert_eq!(mailbox.take_within(quiet), Taken::Closed);
    }

    #[test]
    fn a_render_thread_waits_for_an_edit_and_ends_when_closed() {
        let mailbox = std::sync::Arc::new(Mailbox::new());
        let worker = {
            let mailbox = mailbox.clone();
            std::thread::spawn(move || {
                let mut seen = Vec::new();
                while let Some(job) = mailbox.take() {
                    seen.push(job.job);
                    if seen.len() == 1 {
                        mailbox.close();
                    }
                }
                seen
            })
        };
        mailbox.put(7);
        assert_eq!(worker.join().unwrap(), [7]);
    }

    #[test]
    fn stale_frames_and_a_late_draft_are_dropped() {
        let mut shown = Shown::default();
        assert!(shown.accept(2, true));
        assert!(shown.accept(2, false), "the full frame replaces its draft");
        assert!(!shown.accept(2, true), "a draft never replaces its frame");
        assert!(!shown.accept(1, false), "an older edit's frame");
        assert!(shown.accept(3, true));
        assert_eq!(shown.number(), Some(3));
    }
}
