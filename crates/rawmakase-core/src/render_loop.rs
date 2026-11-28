//! A view's render loop: what an editor that renders while its controls move
//! runs on its render thread, so that each front end doesn't make its own.
//!
//! The newest edit replaces the one waiting, so a fast drag queues nothing,
//! and the render that is running finishes and shows, so a renderer slower
//! than the slider still shows frames during a drag. When the renderer says a
//! draft is due ([`Renderer::draft_due`](crate::Renderer::draft_due)), the
//! draft shows first and the frame follows once no newer edit has come for
//! the quiet time; a newer edit goes first. A frame older than the one shown,
//! and a draft after its own frame, are never given to the app.
//!
//! A render is cancelled only when what it was made for is gone (another
//! photo, another view), by the flag its job carries, and a cancelled render
//! is dropped without a word: it is not a failure. A render that panics is
//! reported as its edit's failure, and the renderer is made again, since what
//! the panic interrupted is suspect.
use crate::mailbox::{Mailbox, Numbered, Shown, Taken};
use crate::{Context, Error, Frame, Image, Recipe, Renderer, View};
use rawmakase_cancel::Cancel;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// One edit of a view to render.
#[derive(Clone, Debug)]
pub struct RenderJob {
    /// The app's number for the edit, which what is rendered for it carries
    /// back.
    pub tag: u64,
    /// The photo's decode.
    pub image: Image,
    /// The edit.
    pub recipe: Recipe,
    /// What of the photo is shown.
    pub view: View,
    /// Set by the app once the photo or the view the job was made for is
    /// gone: its render stops, and nothing of it is given to the app.
    pub cancel: Arc<AtomicBool>,
}

/// What a [`RenderLoop`] gives the app, on its render thread.
#[derive(Debug)]
pub enum Rendered {
    /// A frame to show: a draft ([`Frame::is_draft`]), or the job's frame.
    Frame {
        /// The job's [`RenderJob::tag`].
        tag: u64,
        /// The pixels.
        frame: Frame,
        /// How long the render took.
        took: Duration,
    },
    /// A render that failed, or panicked; never one that was cancelled.
    Failed {
        /// The job's [`RenderJob::tag`].
        tag: u64,
        /// Why.
        error: Error,
    },
}

/// The render loop of one view: jobs are given to it from any thread with
/// [`RenderLoop::submit`], and the app's render thread runs it with
/// [`RenderLoop::run`]. Cheap to clone; every clone is the same loop.
///
/// Shutdown: [`RenderLoop::close`] ends [`RenderLoop::run`] once the render it
/// is in finishes; a job waiting then never renders. Nothing in the library
/// waits for the thread or joins it.
#[derive(Clone, Debug)]
pub struct RenderLoop {
    jobs: Arc<Mailbox<RenderJob>>,
    quiet: Duration,
}

impl Default for RenderLoop {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderLoop {
    /// How long a draft stands, with no newer edit, before its frame renders,
    /// unless [`RenderLoop::quiet`] says otherwise: longer than the gap
    /// between a dragged slider's changes (a frame at 60 Hz is 17 ms), so a
    /// drag goes on in drafts.
    pub const QUIET: Duration = Duration::from_millis(50);

    /// A loop with nothing to render.
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mailbox::new()),
            quiet: Self::QUIET,
        }
    }

    /// This loop with a draft's frame rendered after `quiet` without a newer
    /// edit, in place of [`RenderLoop::QUIET`].
    #[must_use]
    pub fn quiet(mut self, quiet: Duration) -> Self {
        self.quiet = quiet;
        self
    }

    /// Makes `job` the next to render, in place of the one waiting, which
    /// then never renders.
    pub fn submit(&self, job: RenderJob) {
        self.jobs.put(job);
    }

    /// Ends [`RenderLoop::run`] once its current render is done; a job waiting
    /// never runs, nor the frame a draft was waiting for.
    pub fn close(&self) {
        self.jobs.close();
    }

    /// Renders the jobs submitted, with renderers made from `ctx`, giving
    /// `rendered` each frame to show and each failure, until the loop is
    /// closed. Run it on the thread that renders: it blocks between jobs.
    pub fn run(&self, ctx: &Context, rendered: impl FnMut(Rendered)) {
        let mut rendered = rendered;
        run_with(
            &self.jobs,
            self.quiet,
            || ctx.renderer(),
            |event| {
                rendered(event.into());
            },
        );
    }
}

/// What the loop renders with: the library's renderer, or a test's.
pub(crate) trait Render {
    /// What a render gives.
    type Output;
    /// Whether `job`'s frame is to be preceded by a draft.
    fn draft_due(&self, job: &RenderJob) -> bool;
    /// Renders `job`'s draft, or its frame.
    fn render(
        &mut self,
        job: &RenderJob,
        draft: bool,
        cancel: Cancel<'_>,
    ) -> crate::Result<Self::Output>;
}

impl Render for Renderer {
    type Output = Frame;

    fn draft_due(&self, job: &RenderJob) -> bool {
        Renderer::draft_due(self, &job.image, &job.recipe, job.view).is_some()
    }

    fn render(&mut self, job: &RenderJob, draft: bool, cancel: Cancel<'_>) -> crate::Result<Frame> {
        if draft {
            self.render_draft(&job.image, &job.recipe, job.view, cancel)
        } else {
            Renderer::render(self, &job.image, &job.recipe, job.view, cancel)
        }
    }
}

/// What [`run_with`] gives, with the renderer's output.
pub(crate) enum Event<T> {
    Shown { tag: u64, output: T, took: Duration },
    Failed { tag: u64, error: Error },
}

impl From<Event<Frame>> for Rendered {
    fn from(event: Event<Frame>) -> Self {
        match event {
            Event::Shown { tag, output, took } => Rendered::Frame {
                tag,
                frame: output,
                took,
            },
            Event::Failed { tag, error } => Rendered::Failed { tag, error },
        }
    }
}

/// The loop itself, over any renderer: until `jobs` is closed.
pub(crate) fn run_with<R: Render>(
    jobs: &Mailbox<RenderJob>,
    quiet: Duration,
    make: impl Fn() -> R,
    mut sink: impl FnMut(Event<R::Output>),
) {
    let mut renderer = make();
    let mut shown = Shown::default();
    // Whether the render was shown: a cancelled, failed or panicked one has
    // no frame to follow it.
    let mut render = |renderer: &mut R, job: &Numbered<RenderJob>, draft: bool| -> bool {
        let Numbered { number, job } = job;
        if job.cancel.load(Ordering::Relaxed) {
            return false;
        }
        let started = Instant::now();
        let cancel = Cancel::new(&job.cancel);
        let rendered = catch_unwind(AssertUnwindSafe(|| renderer.render(job, draft, cancel)));
        match rendered {
            // A result for a photo or a view that is gone is stale.
            Ok(Ok(_)) if cancel.is_cancelled() => false,
            Ok(Ok(output)) => {
                if shown.accept(*number, draft) {
                    sink(Event::Shown {
                        tag: job.tag,
                        output,
                        took: started.elapsed(),
                    });
                }
                true
            }
            // Nothing of a job whose photo or view is gone is given, its
            // failures included.
            Ok(Err(error)) if error.is_cancelled() || cancel.is_cancelled() => false,
            Ok(Err(error)) => {
                sink(Event::Failed {
                    tag: job.tag,
                    error,
                });
                false
            }
            Err(_) => {
                *renderer = make();
                if cancel.is_cancelled() {
                    return false;
                }
                sink(Event::Failed {
                    tag: job.tag,
                    error: Error::new(crate::ErrorKind::Other, "Rendering failed unexpectedly"),
                });
                false
            }
        }
    };
    let mut next = None;
    loop {
        let Some(job) = next.take().or_else(|| jobs.take()) else {
            break;
        };
        let due = match catch_unwind(AssertUnwindSafe(|| renderer.draft_due(&job.job))) {
            Ok(due) => due,
            // Rendered as a frame, by a renderer made again.
            Err(_) => {
                renderer = make();
                false
            }
        };
        if due {
            if !render(&mut renderer, &job, true) {
                continue;
            }
            match jobs.take_within(quiet) {
                Taken::Job(newer) => {
                    next = Some(newer);
                    continue;
                }
                Taken::Closed => break,
                Taken::Quiet => {}
            }
        }
        let _ = render(&mut renderer, &job, false);
    }
}

#[cfg(test)]
mod tests;
