use super::*;
use crate::app::library::previews::tests::{edited_png, render_job};
use std::sync::mpsc::Receiver;

/// Renders that only count themselves, for a job taken from `shared`.
fn counting(
    shared: &Arc<Shared>,
    cache: Option<PreviewCache>,
    wanted: Wanted,
) -> (Renders, Receiver<EditResult>) {
    fn render(_: &Path, _: &EditSource, _: Cancel<'_>) -> anyhow::Result<image::RgbImage> {
        RENDERED.fetch_add(1, Ordering::SeqCst);
        Ok(image::RgbImage::new(4, 4))
    }
    let (tx, rx) = mpsc::channel();
    let renders = Renders {
        background: None,
        full: None,
        cache: Arc::new(Mutex::new(cache)),
        cache_path: PathBuf::new(),
        wanted,
        results: Results {
            tx,
            shared: shared.clone(),
            ctx: egui::Context::default(),
        },
        render,
        render_ai: render_edited_with_ai,
    };
    (renders, rx)
}

static RENDERED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[test]
fn photos_asked_ahead_are_not_rendered_once_they_lost_their_reason() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = edited_png(directory.path(), "1.png")?;
    let queued = |job| Queued {
        job,
        missed: false,
        dropped: false,
    };
    // No cache to keep it in: rendered when shown.
    let shared = Arc::<Shared>::default();
    shared.lock().pace = Pace::Full;
    shared.lock().jobs.push(queued(render_job(1, &path, true)));
    let (renders, rx) = counting(&shared, None, Wanted::default());
    renders.start(0);
    assert!(matches!(rx.try_recv(), Ok(EditResult::Kept(PhotoId(1), 1))));
    // Looked up at full pace, started once Develop came to the front.
    let shared = Arc::<Shared>::default();
    shared.lock().jobs.push(queued(render_job(2, &path, true)));
    let cache = PreviewCache::open(&directory.path().join("previews.sqlite3"))?;
    let (renders, rx) = counting(&shared, Some(cache), Wanted::default());
    renders.start(0);
    assert!(matches!(
        rx.try_recv(),
        Ok(EditResult::Skipped(PhotoId(2), 2))
    ));
    assert!(shared.lock().running.is_empty());
    // Its list gone while it was looked up (a miss), before its render began.
    let shared = Arc::<Shared>::default();
    let queue = EditQueue(shared.clone());
    queue.set_pace(Pace::Full);
    queue.send(render_job(4, &path, true))?;
    assert!(matches!(
        next_step(&shared, &Wanted::default(), 2),
        Step::Look(0, true)
    ));
    queue.drop_ahead();
    let cache = PreviewCache::open(&directory.path().join("empty.sqlite3"))?;
    let (renders, rx) = counting(&shared, Some(cache), Wanted::default());
    renders.look(0, true);
    assert!(matches!(
        rx.try_recv(),
        Ok(EditResult::Skipped(PhotoId(4), 4))
    ));
    assert!(shared.lock().running.is_empty());
    drop(queue);
    // Closed since it was looked up.
    let shared = Arc::<Shared>::default();
    shared.lock().jobs.push(queued(render_job(3, &path, false)));
    let (renders, rx) = counting(&shared, None, Wanted::default());
    shared.close();
    renders.start(0);
    assert!(rx.try_recv().is_err());
    assert!(shared.lock().running.is_empty());
    assert_eq!(RENDERED.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn photos_asked_ahead_for_a_list_gone_are_dropped_unless_shown() {
    let directory = Path::new("/nonexistent");
    let (queue, _) = EditQueue::detached();
    let wanted = Wanted::default();
    wanted.lock().unwrap().insert(PhotoId(2));
    queue.set_pace(Pace::Full);
    for (id, ahead) in [(1, true), (2, true)] {
        queue.send(render_job(id, directory, ahead)).unwrap();
    }
    queue.drop_ahead();
    // Sent after the list changed: kept.
    queue.send(render_job(3, directory, true)).unwrap();
    // Not rendered, even at full pace; one shown since is, as a photo shown.
    assert!(matches!(
        next_step(&queue.0, &wanted, 2),
        Step::Skip(PhotoId(1), 1)
    ));
    assert!(matches!(
        next_step(&queue.0, &wanted, 2),
        Step::Look(0, true)
    ));
    queue.0.lock().jobs.remove(0);
    assert!(matches!(
        next_step(&queue.0, &wanted, 2),
        Step::Look(0, true)
    ));
}

/// A file being rendered isn't rendered again beside it, so the two share
/// one Camera Look fit rather than each making it. It is still looked up,
/// and other files render.
#[test]
fn computers_with_less_memory_render_fewer_side_by_side() {
    const GIB: u64 = 1 << 30;
    let renders = |gib: f64| full_renders(Some((gib * GIB as f64) as u64), 17);
    assert_eq!(renders(8.), 2);
    // Less than the label says, as systems keep some back.
    assert_eq!(renders(7.6), 2);
    assert_eq!(renders(12.), 3);
    assert_eq!(renders(16.), 4);
    assert_eq!(renders(64.), 4);
    assert_eq!(renders(4.), 1);
    assert_eq!(renders(1.), 1);
    // Unknown: as an 8 GB computer.
    assert_eq!(full_renders(None, 17), 2);
    // Never more than the full pace's threads.
    assert_eq!(full_renders(Some(64 * GIB), 2), 2);
}

#[test]
fn a_file_is_rendered_once_at_a_time() {
    let (one, other) = (Path::new("/photos/one.NEF"), Path::new("/photos/other.NEF"));
    let wanted = Wanted::default();
    wanted
        .lock()
        .unwrap()
        .extend([PhotoId(1), PhotoId(2), PhotoId(3)]);
    let queued = |job| Queued {
        job,
        missed: false,
        dropped: false,
    };
    let shared = Shared::default();
    {
        let mut state = shared.lock();
        state.pace = Pace::Full;
        // A virtual copy of `one` is rendering.
        state.running.push(Running {
            slot: 0,
            full: true,
            cancel: Arc::default(),
            path: one.into(),
        });
        state.jobs = vec![
            queued(render_job(2, other, false)),
            queued(render_job(3, one, false)),
        ];
    }
    // The latest, another copy of `one`: from the cache only.
    assert!(matches!(
        next_step(&shared, &wanted, 4),
        Step::Look(1, false)
    ));
    shared.lock().jobs[1].missed = true;
    // `other` renders beside it.
    assert!(matches!(
        next_step(&shared, &wanted, 4),
        Step::Look(0, true)
    ));
    shared.lock().jobs[0].missed = true;
    assert!(matches!(next_step(&shared, &wanted, 4), Step::Render(0)));
    // Once `one`'s render is done, its other copy renders.
    shared.lock().jobs.remove(0);
    shared.lock().running.clear();
    assert!(matches!(next_step(&shared, &wanted, 4), Step::Render(0)));
}

#[test]
fn photos_shown_go_before_the_ones_asked_ahead() {
    let directory = Path::new("/nonexistent");
    let shared = Shared::default();
    let wanted = Wanted::default();
    wanted.lock().unwrap().extend([PhotoId(2), PhotoId(3)]);
    let queued = |job| Queued {
        job,
        missed: false,
        dropped: false,
    };
    {
        let mut state = shared.lock();
        state.pace = Pace::Full;
        state.jobs = vec![
            queued(render_job(1, directory, true)),
            queued(render_job(2, directory, false)),
            // Asked ahead and shown since: as a photo shown.
            queued(render_job(3, directory, true)),
            queued(render_job(4, directory, true)),
        ];
    }
    // The latest photo shown first, then the one before it.
    assert!(matches!(
        next_step(&shared, &wanted, 2),
        Step::Look(2, true)
    ));
    shared.lock().jobs[2].missed = true;
    assert!(matches!(
        next_step(&shared, &wanted, 2),
        Step::Look(1, true)
    ));
    shared.lock().jobs[1].missed = true;
    assert!(matches!(next_step(&shared, &wanted, 2), Step::Render(2)));
    shared.lock().jobs.remove(2);
    shared.lock().jobs.remove(1);
    // Then the ones asked ahead, in the order asked.
    assert!(matches!(
        next_step(&shared, &wanted, 2),
        Step::Look(0, true)
    ));
    // A photo out of view is answered without a render; at the background
    // pace so are the ones asked ahead.
    shared
        .lock()
        .jobs
        .push(queued(render_job(5, directory, false)));
    assert!(matches!(
        next_step(&shared, &wanted, 2),
        Step::Skip(PhotoId(5), 5)
    ));
    shared.lock().pace = Pace::Background;
    assert!(matches!(
        next_step(&shared, &wanted, 2),
        Step::Skip(PhotoId(1), 1)
    ));
    // With every slot taken a photo shown is still looked up.
    let mut state = shared.lock();
    state.jobs.clear();
    state.jobs.push(queued(render_job(2, directory, false)));
    state.running.push(Running {
        slot: 0,
        full: false,
        cancel: Arc::default(),
        path: PathBuf::from("/elsewhere.png"),
    });
    drop(state);
    assert!(matches!(
        next_step(&shared, &wanted, 2),
        Step::Look(0, false)
    ));
}
