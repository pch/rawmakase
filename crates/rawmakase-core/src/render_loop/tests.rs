//! The loop's scheduling, with renders the tests hold and release, so
//! nothing depends on how fast this computer renders.
use super::*;
use crate::{Decode, ErrorKind, RawDefaults};
use std::path::Path;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

const SOON: Duration = Duration::from_secs(10);
/// Long enough that a test's next step always comes first.
const NEVER_QUIET: Duration = Duration::from_secs(3600);
/// The view of a job whose renderer says a draft is due.
const DRAFTED: View = View::Fit(1);

/// The chart, decoded, and where it starts: what every job carries.
fn photo() -> (Context, Image, Recipe) {
    let chart =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng");
    let ctx = Context::builder().build().unwrap();
    let photo = ctx.open(&chart, Cancel::NEVER).unwrap();
    let image = photo.decode(Decode::Half, Cancel::NEVER).unwrap();
    let recipe = photo
        .starting_recipe(&ctx.raw_defaults(&RawDefaults::default()))
        .unwrap();
    (ctx, image, recipe)
}

/// A renderer whose renders report that they started (the job's tag and
/// whether it is the draft) and wait to be released. Tag 666 panics, and
/// tag 777 fails.
struct Held {
    started: Sender<(u64, bool)>,
    release: Arc<Mutex<Receiver<()>>>,
}

impl Render for Held {
    type Output = u64;

    fn draft_due(&self, job: &RenderJob) -> bool {
        job.view == DRAFTED
    }

    fn render(&mut self, job: &RenderJob, draft: bool, _: Cancel<'_>) -> crate::Result<u64> {
        self.started.send((job.tag, draft)).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        assert_ne!(job.tag, 666, "this render panics");
        if job.tag == 777 {
            return Err(Error::new(ErrorKind::Other, "this render fails"));
        }
        // As the library's renderer stops once its flag is set.
        if job.cancel.load(Ordering::Relaxed) {
            return Err(Error::of(
                rawmakase_cancel::Cancelled::new("Render superseded").into(),
            ));
        }
        Ok(job.tag)
    }
}

struct Thread {
    jobs: Arc<Mailbox<RenderJob>>,
    started: Receiver<(u64, bool)>,
    release: Sender<()>,
    events: Receiver<Event<u64>>,
    image: Image,
    recipe: Recipe,
}

impl Thread {
    fn new(quiet: Duration) -> Self {
        let (_, image, recipe) = photo();
        let (started_tx, started) = channel();
        let (release, release_rx) = channel();
        let (events_tx, events) = channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let jobs = Arc::new(Mailbox::new());
        let taken = jobs.clone();
        std::thread::spawn(move || {
            run_with(
                &taken,
                quiet,
                move || Held {
                    started: started_tx.clone(),
                    release: release_rx.clone(),
                },
                move |event| events_tx.send(event).unwrap(),
            );
        });
        Self {
            jobs,
            started,
            release,
            events,
            image,
            recipe,
        }
    }

    fn submit(&self, tag: u64, drafted: bool) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs.put(RenderJob {
            tag,
            image: self.image.clone(),
            recipe: self.recipe.clone(),
            view: if drafted { DRAFTED } else { View::Fit(0) },
            cancel: cancel.clone(),
        });
        cancel
    }

    /// The render that started next: its job's tag and whether it is a
    /// draft.
    fn started(&self) -> (u64, bool) {
        self.started.recv_timeout(SOON).expect("a render to start")
    }

    fn release(&self) {
        self.release.send(()).unwrap();
    }

    /// What was shown next: the tag, and whether it is the job's draft, as
    /// the held render was told.
    fn shown(&self) -> u64 {
        match self.events.recv_timeout(SOON).expect("a render to show") {
            Event::Shown { tag, output, .. } => {
                assert_eq!(tag, output);
                tag
            }
            Event::Failed { tag, error } => panic!("job {tag} failed: {error:#}"),
        }
    }

    fn nothing_more(&self) {
        assert!(self.started.try_recv().is_err(), "no render started");
        assert!(self.events.try_recv().is_err(), "nothing was shown");
    }
}

/// Dragging fast queues nothing: of the edits made while a render runs only
/// the newest is rendered next, and the running render is not cancelled by
/// them but finishes and shows.
#[test]
fn the_newest_edit_replaces_the_one_waiting_and_the_running_render_shows() {
    let t = Thread::new(NEVER_QUIET);
    t.submit(1, false);
    assert_eq!(t.started(), (1, false));
    // Three edits while edit 1 renders: each replaces the one waiting, never
    // the one running.
    for tag in 2..=4 {
        t.submit(tag, false);
    }
    t.release();
    assert_eq!(t.shown(), 1);
    assert_eq!(t.started(), (4, false));
    t.release();
    assert_eq!(t.shown(), 4);
    t.nothing_more();
}

/// The same edit rendered again, as for a new view, shows again: the frames
/// are told apart by the loop's own count, not by the app's tags.
#[test]
fn the_same_edit_for_a_new_view_shows_again() {
    let t = Thread::new(NEVER_QUIET);
    t.submit(1, false);
    assert_eq!(t.started(), (1, false));
    t.release();
    assert_eq!(t.shown(), 1);
    t.submit(1, false);
    assert_eq!(t.started(), (1, false));
    t.release();
    assert_eq!(t.shown(), 1);
}

/// When the renderer says a draft is due, the draft shows, then, once
/// nothing newer has come for the quiet time, its frame.
#[test]
fn a_due_draft_shows_first_and_its_frame_follows_when_quiet() {
    let t = Thread::new(Duration::from_millis(20));
    t.submit(1, true);
    assert_eq!(t.started(), (1, true));
    t.release();
    assert_eq!(t.shown(), 1);
    assert_eq!(t.started(), (1, false));
    t.release();
    assert_eq!(t.shown(), 1);
    // No draft due: the frame alone.
    t.submit(2, false);
    assert_eq!(t.started(), (2, false));
    t.release();
    assert_eq!(t.shown(), 2);
    t.nothing_more();
}

/// While a control moves, a newer edit takes the place of the frame a draft
/// was waiting for.
#[test]
fn a_newer_edit_comes_before_a_drafts_frame() {
    let t = Thread::new(NEVER_QUIET);
    t.submit(1, true);
    assert_eq!(t.started(), (1, true));
    t.submit(2, true);
    t.release();
    assert_eq!(t.shown(), 1);
    assert_eq!(t.started(), (2, true));
    t.release();
    assert_eq!(t.shown(), 2);
    // Edit 3's last full frame was quick, say: no draft.
    t.submit(3, false);
    assert_eq!(t.started(), (3, false));
    t.release();
    assert_eq!(t.shown(), 3);
    t.nothing_more();
}

/// A render that finishes after its photo or view is gone is stale: nothing
/// of it is shown, and it is not a failure. A cancelled job's draft is not
/// rendered either.
#[test]
fn a_result_for_a_view_that_is_gone_is_dropped() {
    let t = Thread::new(NEVER_QUIET);
    let gone = t.submit(1, false);
    assert_eq!(t.started(), (1, false));
    gone.store(true, Ordering::Relaxed);
    t.submit(2, false);
    t.release();
    assert_eq!(t.started(), (2, false));
    t.release();
    assert_eq!(t.shown(), 2);
    t.nothing_more();

    // An edit with a draft due, its view gone while it waits behind a
    // running render: neither its draft nor its frame renders.
    t.submit(3, false);
    assert_eq!(t.started(), (3, false));
    let gone = t.submit(4, true);
    gone.store(true, Ordering::Relaxed);
    t.release();
    assert_eq!(t.shown(), 3);
    t.submit(5, false);
    assert_eq!(t.started(), (5, false), "edit 4 rendered nothing");
    t.release();
    assert_eq!(t.shown(), 5);
    t.nothing_more();
}

/// A panicking render is reported as that edit's failure, and the loop goes
/// on rendering with a renderer made again.
#[test]
fn a_panicking_render_fails_its_edit_alone() {
    let t = Thread::new(NEVER_QUIET);
    t.submit(666, false);
    assert_eq!(t.started(), (666, false));
    t.release();
    match t.events.recv_timeout(SOON).unwrap() {
        Event::Failed { tag, error } => {
            assert_eq!(tag, 666);
            assert_eq!(error.kind(), ErrorKind::Other);
        }
        Event::Shown { .. } => panic!("a panicking render showed something"),
    }
    t.submit(2, false);
    assert_eq!(t.started(), (2, false));
    t.release();
    assert_eq!(t.shown(), 2);
}

/// A job whose photo or view went while it rendered gives nothing, even
/// when its render fails or panics.
#[test]
fn a_gone_job_s_failure_is_not_reported() {
    let t = Thread::new(NEVER_QUIET);
    for tag in [777, 666] {
        let gone = t.submit(tag, false);
        assert_eq!(t.started(), (tag, false));
        gone.store(true, Ordering::Relaxed);
        t.release();
    }
    t.submit(2, false);
    assert_eq!(t.started(), (2, false));
    t.release();
    assert_eq!(t.shown(), 2, "the next job's frame, and nothing before it");
    t.nothing_more();
}

/// A closed loop finishes the render it is in and runs nothing pending, nor
/// the frame a draft was waiting for.
#[test]
fn a_closed_loop_runs_no_pending_work() {
    let t = Thread::new(NEVER_QUIET);
    t.submit(1, false);
    assert_eq!(t.started(), (1, false));
    t.submit(2, false);
    t.jobs.close();
    t.release();
    assert_eq!(t.shown(), 1);
    // The loop ends, dropping its side of the channels.
    assert!(
        t.started.recv_timeout(SOON).is_err(),
        "edit 2 never rendered"
    );

    let t = Thread::new(NEVER_QUIET);
    t.submit(1, true);
    assert_eq!(t.started(), (1, true));
    t.release();
    assert_eq!(t.shown(), 1);
    t.jobs.close();
    assert!(
        t.started.recv_timeout(SOON).is_err(),
        "the frame never rendered"
    );
}

/// With the library's own renderer, through the loop an app runs: a render
/// whose flag is set while it renders returns the cancelled kind, which the
/// loop drops without reporting, and the next render of the new view shows.
#[test]
fn a_cancelled_render_of_the_library_is_never_reported() {
    let (ctx, image, recipe) = photo();
    let renders = RenderLoop::new().quiet(NEVER_QUIET);
    let cancel = Arc::new(AtomicBool::new(true));
    let job = |view, cancel: &Arc<AtomicBool>| RenderJob {
        tag: 1,
        image: image.clone(),
        recipe: recipe.clone(),
        view,
        cancel: cancel.clone(),
    };
    // Cancelled before it starts: nothing of it renders.
    renders.submit(job(View::Fit(300), &cancel));
    let (rendered_tx, rendered) = channel();
    let running = renders.clone();
    let thread = std::thread::spawn(move || {
        running.run(&ctx, move |r| rendered_tx.send(r).unwrap());
    });
    renders.submit(job(View::Fit(200), &Arc::default()));
    match rendered.recv_timeout(SOON).unwrap() {
        Rendered::Frame { tag, frame, .. } => {
            assert_eq!(tag, 1);
            assert_eq!(frame.width().max(frame.height()), 200);
            assert!(!frame.is_draft());
        }
        Rendered::Failed { error, .. } => panic!("reported: {error:#}"),
    }
    renders.close();
    thread.join().unwrap();
    assert!(rendered.try_recv().is_err(), "one frame, and no failure");
}
