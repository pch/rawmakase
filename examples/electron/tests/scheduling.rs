//! The render thread's scheduling, with renders the tests hold and release,
//! so nothing depends on how fast this computer renders: the newest edit
//! replaces the one waiting, the running render finishes and shows, a draft
//! comes before its frame when the renderer says one is due, and a render
//! cancelled because its photo or view is gone is dropped without being
//! reported.
use rawmakase_core::{Cancel, Context, Decode, Error, ErrorKind, Renderer, View};
use rawmakase_electron::editor::{self, Pixels, Work};
use rawmakase_electron::worker::{Event, Job, Render, Worker};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SOON: Duration = Duration::from_secs(10);
/// Long enough that a test's next step always comes first.
const NEVER_QUIET: Duration = Duration::from_secs(3600);

/// What a held render is given: a number, which is also its output, and
/// whether the renderer is to say a draft is due before its frame.
type Fake = (u32, bool);

/// A renderer whose renders report that they started (the work's number and
/// whether it is the draft) and wait to be released. Work 666 panics.
struct Held {
    started: Sender<(u32, bool)>,
    release: Arc<Mutex<Receiver<()>>>,
}

impl Render<Fake> for Held {
    type Output = u32;

    fn draft_due(&self, work: &Fake) -> bool {
        work.1
    }

    fn render(&mut self, work: &Fake, draft: bool, _: Cancel<'_>) -> Result<u32, Error> {
        self.started.send((work.0, draft)).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        assert_ne!(work.0, 666, "this render panics");
        Ok(work.0)
    }
}

struct Thread {
    worker: Worker<Fake>,
    started: Receiver<(u32, bool)>,
    release: Sender<()>,
    events: Receiver<Event<u32>>,
}

impl Thread {
    fn new(quiet: Duration) -> Self {
        let (started_tx, started) = channel();
        let (release, release_rx) = channel();
        let (events_tx, events) = channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let worker = Worker::spawn(
            quiet,
            move || Held {
                started: started_tx.clone(),
                release: release_rx.clone(),
            },
            move |event| events_tx.send(event).unwrap(),
        );
        Self {
            worker,
            started,
            release,
            events,
        }
    }

    fn submit(&self, edit: u32, drafted: bool) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        self.worker.submit(Job {
            edit,
            work: (edit, drafted),
            cancel: cancel.clone(),
        });
        cancel
    }

    /// The render that started next: its work's number and whether it is a
    /// draft.
    fn started(&self) -> (u32, bool) {
        self.started.recv_timeout(SOON).expect("a render to start")
    }

    fn release(&self) {
        self.release.send(()).unwrap();
    }

    /// What was shown next: the edit, whether it is a draft, and the output.
    fn shown(&self) -> (u32, bool, u32) {
        match self.events.recv_timeout(SOON).expect("a render to show") {
            Event::Shown {
                edit,
                draft,
                output,
                ..
            } => (edit, draft, output),
            Event::Failed { edit, message } => panic!("edit {edit} failed: {message}"),
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
    for edit in 2..=4 {
        t.submit(edit, false);
    }
    t.release();
    assert_eq!(t.shown(), (1, false, 1));
    assert_eq!(t.started(), (4, false));
    t.release();
    assert_eq!(t.shown(), (4, false, 4));
    t.nothing_more();
}

/// The same edit rendered again, as for a new view, shows again: the frames
/// are told apart by the mailbox's numbers, not by the interface's.
#[test]
fn the_same_edit_for_a_new_view_shows_again() {
    let t = Thread::new(NEVER_QUIET);
    t.submit(1, false);
    assert_eq!(t.started(), (1, false));
    t.release();
    assert_eq!(t.shown(), (1, false, 1));
    t.submit(1, false);
    assert_eq!(t.started(), (1, false));
    t.release();
    assert_eq!(t.shown(), (1, false, 1));
}

/// When the renderer says a draft is due, the draft shows, then, once
/// nothing newer has come for the quiet time, its frame.
#[test]
fn a_due_draft_shows_first_and_its_frame_follows_when_quiet() {
    let t = Thread::new(Duration::from_millis(20));
    t.submit(1, true);
    assert_eq!(t.started(), (1, true));
    t.release();
    assert_eq!(t.shown(), (1, true, 1));
    assert_eq!(t.started(), (1, false));
    t.release();
    assert_eq!(t.shown(), (1, false, 1));
    // No draft due: the frame alone.
    t.submit(2, false);
    assert_eq!(t.started(), (2, false));
    t.release();
    assert_eq!(t.shown(), (2, false, 2));
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
    assert_eq!(t.shown(), (1, true, 1));
    assert_eq!(t.started(), (2, true));
    t.release();
    assert_eq!(t.shown(), (2, true, 2));
    // Edit 3's last full frame was quick, say: no draft.
    t.submit(3, false);
    assert_eq!(t.started(), (3, false));
    t.release();
    assert_eq!(t.shown(), (3, false, 3));
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
    assert_eq!(t.shown(), (2, false, 2));
    t.nothing_more();

    // An edit with a draft due, its view gone while it waits behind a
    // running render: neither its draft nor its frame renders.
    t.submit(3, false);
    assert_eq!(t.started(), (3, false));
    let gone = t.submit(4, true);
    gone.store(true, Ordering::Relaxed);
    t.release();
    assert_eq!(t.shown(), (3, false, 3));
    t.submit(5, false);
    assert_eq!(t.started(), (5, false), "edit 4 rendered nothing");
    t.release();
    assert_eq!(t.shown(), (5, false, 5));
    t.nothing_more();
}

/// A panicking render is reported as that edit's failure, and the thread
/// goes on rendering with a renderer made again.
#[test]
fn a_panicking_render_fails_its_edit_alone() {
    let t = Thread::new(NEVER_QUIET);
    t.submit(666, false);
    assert_eq!(t.started(), (666, false));
    t.release();
    match t.events.recv_timeout(SOON).unwrap() {
        Event::Failed { edit, .. } => assert_eq!(edit, 666),
        Event::Shown { .. } => panic!("a panicking render showed something"),
    }
    t.submit(2, false);
    assert_eq!(t.started(), (2, false));
    t.release();
    assert_eq!(t.shown(), (2, false, 2));
}

/// A stopped worker finishes the render it is in and runs nothing pending,
/// nor the frame a draft was waiting for.
#[test]
fn a_stopped_worker_runs_no_pending_work() {
    let t = Thread::new(NEVER_QUIET);
    t.submit(1, false);
    assert_eq!(t.started(), (1, false));
    t.submit(2, false);
    t.worker.stop();
    t.release();
    assert_eq!(t.shown(), (1, false, 1));
    // The thread ends, dropping its side of the channels.
    assert!(
        t.started.recv_timeout(SOON).is_err(),
        "edit 2 never rendered"
    );

    let t = Thread::new(NEVER_QUIET);
    t.submit(1, true);
    assert_eq!(t.started(), (1, true));
    t.release();
    assert_eq!(t.shown(), (1, true, 1));
    drop(t.worker);
    assert!(
        t.started.recv_timeout(SOON).is_err(),
        "the frame never rendered"
    );
}

/// The library's renderer, held before each render and never drafting.
struct Library {
    renderer: Renderer,
    started: Sender<View>,
    release: Arc<Mutex<Receiver<()>>>,
    kinds: Arc<Mutex<Vec<Result<(), ErrorKind>>>>,
}

impl Render<Work> for Library {
    type Output = Pixels;

    fn draft_due(&self, _: &Work) -> bool {
        false
    }

    fn render(&mut self, work: &Work, draft: bool, cancel: Cancel<'_>) -> Result<Pixels, Error> {
        self.started.send(work.view).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        let rendered = editor::render(&mut self.renderer, work, draft, cancel);
        let kind = rendered.as_ref().map(|_| ()).map_err(Error::kind);
        self.kinds.lock().unwrap().push(kind);
        rendered
    }
}

/// With the library's own renderer: a render whose flag is set while it
/// waits to start returns the library's cancelled kind, which the worker
/// drops without reporting, and the next render of the new view shows.
#[test]
fn a_cancelled_render_of_the_library_is_never_shown_as_an_error() {
    let chart =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng");
    let home = tempfile::tempdir().unwrap();
    let ctx = Context::builder()
        .data_dirs([home.path().join("data")])
        .build()
        .unwrap();
    let photo = ctx.open(&chart, Cancel::NEVER).unwrap();
    let image = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
    let recipe = photo.starting_recipe().unwrap();
    let work = |edge| Work {
        image: image.clone(),
        recipe: recipe.clone(),
        view: View::Fit(edge),
    };
    let (started_tx, started) = channel();
    let (release, release_rx) = channel::<()>();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let (events_tx, events) = channel();
    let kinds = Arc::new(Mutex::new(Vec::new()));
    let seen = kinds.clone();
    let worker = Worker::spawn(
        NEVER_QUIET,
        move || Library {
            renderer: ctx.renderer(),
            started: started_tx.clone(),
            release: release_rx.clone(),
            kinds: seen.clone(),
        },
        move |event| events_tx.send(event).unwrap(),
    );
    let cancel = Arc::new(AtomicBool::new(false));
    worker.submit(Job {
        edit: 1,
        work: work(300),
        cancel: cancel.clone(),
    });
    assert_eq!(started.recv_timeout(SOON).unwrap(), View::Fit(300));
    // The view changes while edit 1 renders: its flag is set, and the same
    // edit is asked for at the new size.
    cancel.store(true, Ordering::Relaxed);
    worker.submit(Job {
        edit: 1,
        work: work(200),
        cancel: Arc::default(),
    });
    release.send(()).unwrap();
    assert_eq!(started.recv_timeout(SOON).unwrap(), View::Fit(200));
    release.send(()).unwrap();
    match events.recv_timeout(SOON).unwrap() {
        Event::Shown { edit, output, .. } => {
            assert_eq!(edit, 1);
            assert_eq!(output.width.max(output.height), 200);
        }
        Event::Failed { message, .. } => panic!("the cancelled render was reported: {message}"),
    }
    assert!(
        events.try_recv().is_err(),
        "one render showed, nothing failed"
    );
    assert_eq!(
        *kinds.lock().unwrap(),
        [Err(ErrorKind::Cancelled), Ok(())],
        "the library returned the cancelled kind for the first render"
    );
}
