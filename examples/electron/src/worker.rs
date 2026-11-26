//! The render thread: the library's [`Mailbox`] of edits, its draft rule and
//! its stale-frame rule ([`Shown`]), around a renderer that is synchronous.
//!
//! The newest edit replaces the one waiting, so a fast drag queues nothing,
//! and the render that is running finishes and shows, so a renderer slower
//! than the slider still shows frames during a drag. When the renderer says a
//! draft is due, the draft shows first and the frame follows once no newer
//! edit has come for the quiet time; a newer edit goes first. A frame older
//! than the one shown, and a draft after its own frame, never reach the
//! interface.
//!
//! A render is cancelled only when what it was made for is gone (another
//! photo, another view), by the flag its job carries, and a cancelled render
//! is dropped without a word: it is not a failure. A render that panics is
//! reported as its edit's failure, and the renderer is made again.
//!
//! Shutdown: [`Worker::stop`], and dropping the worker, close the mailbox.
//! The thread ends after the render it is in, pending work never runs, and
//! nothing waits for it or joins it.
#![forbid(unsafe_code)]
use rawmakase_core::{Cancel, Error, Mailbox, Numbered, Shown, Taken};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// What the render thread renders with: the library's renderer in the app,
/// a fake one in the tests.
pub trait Render<W> {
    type Output;
    /// Whether `work`'s frame is to be preceded by a draft.
    fn draft_due(&self, work: &W) -> bool;
    /// Renders `work`'s draft, or its frame.
    fn render(&mut self, work: &W, draft: bool, cancel: Cancel<'_>) -> Result<Self::Output, Error>;
}

/// One edit to render.
pub struct Job<W> {
    /// The interface's number for the edit, which what is shown for it
    /// carries back.
    pub edit: u32,
    /// What the render is given.
    pub work: W,
    /// Set by the job's owner once the photo or the view it was made for is
    /// gone: its render stops, and nothing of it is shown.
    pub cancel: Arc<AtomicBool>,
}

/// What the render thread reports.
pub enum Event<T> {
    /// A render to show, with how long it took.
    Shown {
        edit: u32,
        draft: bool,
        output: T,
        took: Duration,
    },
    /// A render that failed, or panicked. Never one that was cancelled.
    Failed { edit: u32, message: String },
}

/// The render thread. `W` is what a render is given: the photo, the edit and
/// the view, captured when the job was made.
pub struct Worker<W> {
    mailbox: Arc<Mailbox<Job<W>>>,
}

impl<W: Send + 'static> Worker<W> {
    /// Starts the thread. `make` builds the renderer on it, as a renderer
    /// belongs to the thread that renders, and again after a render panicked,
    /// since what the panic interrupted is suspect. `quiet` is how long a
    /// draft stands before its frame is rendered. `sink` is called on the
    /// render thread.
    pub fn spawn<R: Render<W>>(
        quiet: Duration,
        make: impl Fn() -> R + Send + 'static,
        mut sink: impl FnMut(Event<R::Output>) + Send + 'static,
    ) -> Self {
        let mailbox = Arc::new(Mailbox::<Job<W>>::new());
        let jobs = mailbox.clone();
        std::thread::spawn(move || {
            let mut renderer = make();
            let mut shown = Shown::default();
            let mut run = |renderer: &mut R, job: &Numbered<Job<W>>, draft: bool| {
                let Numbered { number, job } = job;
                if job.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let started = Instant::now();
                let cancel = Cancel::new(&job.cancel);
                let rendered = catch_unwind(AssertUnwindSafe(|| {
                    renderer.render(&job.work, draft, cancel)
                }));
                match rendered {
                    // A result for a photo or a view that is gone is stale.
                    Ok(Ok(_)) if cancel.is_cancelled() => {}
                    Ok(Ok(output)) => {
                        if shown.accept(*number, draft) {
                            sink(Event::Shown {
                                edit: job.edit,
                                draft,
                                output,
                                took: started.elapsed(),
                            });
                        }
                    }
                    Ok(Err(error)) if error.is_cancelled() => {}
                    Ok(Err(error)) => sink(Event::Failed {
                        edit: job.edit,
                        message: format!("{error:#}"),
                    }),
                    Err(_) => {
                        *renderer = make();
                        sink(Event::Failed {
                            edit: job.edit,
                            message: "Rendering failed unexpectedly".into(),
                        });
                    }
                }
            };
            let mut next = None;
            loop {
                let Some(job) = next.take().or_else(|| jobs.take()) else {
                    break;
                };
                let due = match catch_unwind(AssertUnwindSafe(|| renderer.draft_due(&job.job.work)))
                {
                    Ok(due) => due,
                    // Rendered as a frame, by a renderer made again.
                    Err(_) => {
                        renderer = make();
                        false
                    }
                };
                if due && !job.job.cancel.load(Ordering::Relaxed) {
                    run(&mut renderer, &job, true);
                    match jobs.take_within(quiet) {
                        Taken::Job(newer) => {
                            next = Some(newer);
                            continue;
                        }
                        Taken::Closed => break,
                        Taken::Quiet => {}
                    }
                }
                run(&mut renderer, &job, false);
            }
        });
        Self { mailbox }
    }

    /// Makes `job` the next to render, in place of the one waiting, which
    /// then never renders. Returns the number the mailbox gave it.
    pub fn submit(&self, job: Job<W>) -> u64 {
        self.mailbox.put(job)
    }
}

impl<W> Worker<W> {
    /// Asks the thread to end once its current render is done; pending work
    /// never runs.
    pub fn stop(&self) {
        self.mailbox.close();
    }
}

impl<W> Drop for Worker<W> {
    fn drop(&mut self) {
        self.stop();
    }
}
