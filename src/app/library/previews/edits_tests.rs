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
    rendering_with(shared, cache, wanted, render)
}

/// Renders with `render`, each on a thread of its own, for a job taken from
/// `shared`.
fn rendering_with(
    shared: &Arc<Shared>,
    cache: Option<PreviewCache>,
    wanted: Wanted,
    render: RenderEdited,
) -> (Renders, Receiver<EditResult>) {
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
        profiles: String::new,
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

/// Develop asks again for a photo's preview as its edit changes (the Crop tool,
/// #1330): a request a later one for the same photo replaced is answered
/// without a render.
#[test]
fn a_request_replaced_by_a_later_one_for_the_photo_is_not_rendered() {
    let directory = Path::new("/nonexistent");
    let shared = Shared::default();
    let wanted = Wanted::default();
    wanted.lock().unwrap().extend([PhotoId(1), PhotoId(2)]);
    let queued = |job| Queued {
        job,
        missed: false,
        dropped: false,
    };
    let again = |ticket| {
        let EditJob::Render {
            id,
            path,
            copy,
            source,
            ahead,
            display,
            ..
        } = render_job(1, directory, false)
        else {
            unreachable!()
        };
        EditJob::Render {
            id,
            ticket,
            path,
            copy,
            source,
            ahead,
            develop: true,
            display,
        }
    };
    shared.lock().jobs = vec![
        queued(render_job(1, directory, false)),
        queued(render_job(2, directory, false)),
        queued(again(7)),
    ];
    assert!(matches!(
        next_step(&shared, &wanted, 2),
        Step::Skip(PhotoId(1), 1)
    ));
    // The latest request for each photo is left, the latest first.
    assert!(matches!(
        next_step(&shared, &wanted, 2),
        Step::Look(1, true)
    ));
}

/// Develop's edit asked for with the Crop tool open is rendered though its
/// photo is out of view (the filmstrip hidden or scrolled away): the catalog
/// may not have that edit yet when the photo is next shown (#1330).
#[test]
fn develop_s_edit_is_rendered_out_of_view() {
    let directory = Path::new("/nonexistent");
    let shared = Shared::default();
    let wanted = Wanted::default();
    let queued = |job| Queued {
        job,
        missed: false,
        dropped: false,
    };
    let mut develop = render_job(1, directory, false);
    if let EditJob::Render { develop, .. } = &mut develop {
        *develop = true;
    }
    shared.lock().jobs = vec![queued(render_job(2, directory, false)), queued(develop)];
    assert!(matches!(
        next_step(&shared, &wanted, 2),
        Step::Skip(PhotoId(2), 2)
    ));
    assert!(matches!(
        next_step(&shared, &wanted, 2),
        Step::Look(0, true)
    ));
}

/// The results of the render started from `shared`'s only job, up to and
/// including its answer, which comes after any cache failure.
fn render_results(
    render: RenderEdited,
    path: &Path,
    cache: PreviewCache,
) -> anyhow::Result<(Vec<EditResult>, Option<PreviewCache>)> {
    let shared = Arc::<Shared>::default();
    shared.lock().jobs.push(Queued {
        job: render_job(1, path, false),
        missed: false,
        dropped: false,
    });
    let wanted = Wanted::default();
    wanted.lock().unwrap().insert(PhotoId(1));
    let (renders, rx) = rendering_with(&shared, Some(cache), wanted, render);
    renders.start(0);
    let mut results = Vec::new();
    loop {
        let result = rx.recv_timeout(std::time::Duration::from_secs(10))?;
        let answer = !matches!(result, EditResult::CacheError(_));
        results.push(result);
        if answer {
            break;
        }
    }
    let cache = renders.cache.lock().unwrap().take();
    Ok((results, cache))
}

/// A file changed while its edited preview renders (#1302): the render is
/// shown, but not kept, and the refusal is no cache failure.
#[test]
fn a_file_changed_while_its_edited_preview_renders_is_shown_not_kept() -> anyhow::Result<()> {
    /// Changes the file it renders, as a save to it mid-render does.
    fn render(path: &Path, _: &EditSource, _: Cancel<'_>) -> anyhow::Result<image::RgbImage> {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)?
            .write_all(b"saved while it rendered")?;
        Ok(image::RgbImage::new(4, 4))
    }
    let directory = tempfile::tempdir()?;
    let path = edited_png(directory.path(), "1.png")?;
    let (original, rendered) = (std::fs::read(&path)?, Stamp::read(&path)?);
    let modified = std::fs::metadata(&path)?.modified()?;
    let cache = PreviewCache::open(&directory.path().join("previews.sqlite3"))?;
    let (results, cache) = render_results(render, &path, cache)?;
    assert!(
        matches!(results[..], [EditResult::Ready(PhotoId(1), 1, _)]),
        "{:?}",
        results.iter().map(describe).collect::<Vec<_>>()
    );
    // Put back as it was rendered, the file would find a preview kept for it.
    std::fs::write(&path, original)?;
    std::fs::File::options()
        .write(true)
        .open(&path)?
        .set_modified(modified)?;
    assert_eq!(Stamp::read(&path)?, rendered);
    let EditJob::Render { source, .. } = render_job(1, &path, false) else {
        unreachable!()
    };
    let tag = source.preview_tag(plain_of(&source));
    assert!(cache.unwrap().load_tagged(&path, &tag)?.is_none(), "kept");
    Ok(())
}

/// A render the cache can't keep is still shown, and the failure reported.
#[test]
fn an_edited_preview_the_cache_fails_to_keep_is_reported() -> anyhow::Result<()> {
    fn render(_: &Path, _: &EditSource, _: Cancel<'_>) -> anyhow::Result<image::RgbImage> {
        // Larger than the cache keeps.
        Ok(image::RgbImage::new(2048, 8))
    }
    let directory = tempfile::tempdir()?;
    let path = edited_png(directory.path(), "1.png")?;
    let cache = PreviewCache::open(&directory.path().join("previews.sqlite3"))?;
    let (results, _) = render_results(render, &path, cache)?;
    assert!(
        matches!(
            results[..],
            [
                EditResult::CacheError(_),
                EditResult::Ready(PhotoId(1), 1, _)
            ]
        ),
        "{:?}",
        results.iter().map(describe).collect::<Vec<_>>()
    );
    Ok(())
}

/// Offers `image`, rendered from `path` as `stamp`, to [`Renders::store`],
/// and gives what it sent and whether the preview was kept.
fn stored(
    path: &Path,
    stamp: &Stamp,
    image: &image::RgbImage,
    cache: PreviewCache,
) -> anyhow::Result<(Vec<EditResult>, bool)> {
    let (renders, rx) = rendering_with(
        &Arc::<Shared>::default(),
        Some(cache),
        Wanted::default(),
        |_, _, _| unreachable!(),
    );
    renders.store(path, "edit-1", "edit-1", stamp, image);
    let sent = rx.try_iter().collect();
    let cache = renders.cache.lock().unwrap().take().unwrap();
    Ok((sent, cache.load_tagged(path, "edit-1")?.is_some()))
}

/// A file changed or removed since it rendered gets no preview, and that is
/// no cache failure.
#[test]
fn storing_an_edited_preview_of_a_changed_or_removed_file_is_no_failure() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = edited_png(directory.path(), "1.png")?;
    let image = image::RgbImage::new(4, 4);
    let open = || PreviewCache::open(&directory.path().join("previews.sqlite3"));
    let rendered = Stamp::read(&path)?;
    {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"saved while it rendered")?;
    }
    let (sent, kept) = stored(&path, &rendered, &image, open()?)?;
    assert!(
        sent.is_empty(),
        "{:?}",
        sent.iter().map(describe).collect::<Vec<_>>()
    );
    assert!(!kept);
    let rendered = Stamp::read(&path)?;
    std::fs::remove_file(&path)?;
    let (sent, kept) = stored(&path, &rendered, &image, open()?)?;
    assert!(
        sent.is_empty(),
        "{:?}",
        sent.iter().map(describe).collect::<Vec<_>>()
    );
    assert!(!kept);
    Ok(())
}

/// A file whose folder became unreadable since it rendered gets no preview,
/// and that is no cache failure either.
#[cfg(unix)]
#[test]
fn storing_an_edited_preview_of_an_unreachable_file_is_no_failure() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    /// Opens the folder again, so the temporary directory can be removed.
    struct Reopen<'a>(&'a Path);
    impl Drop for Reopen<'_> {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
        }
    }
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("shoot");
    std::fs::create_dir(&folder)?;
    let path = edited_png(&folder, "1.png")?;
    let rendered = Stamp::read(&path)?;
    let cache = PreviewCache::open(&directory.path().join("previews.sqlite3"))?;
    let reopen = Reopen(&folder);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000))?;
    // Root reads the file regardless, and has nothing to check here.
    if Stamp::read(&path).is_err() {
        let (renders, rx) = rendering_with(
            &Arc::<Shared>::default(),
            Some(cache),
            Wanted::default(),
            |_, _, _| unreachable!(),
        );
        renders.store(
            &path,
            "edit-1",
            "edit-1",
            &rendered,
            &image::RgbImage::new(4, 4),
        );
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            sent.is_empty(),
            "{:?}",
            sent.iter().map(describe).collect::<Vec<_>>()
        );
        drop(reopen);
        let cache = renders.cache.lock().unwrap().take().unwrap();
        assert!(cache.load_tagged(&path, "edit-1")?.is_none(), "kept");
    } else {
        drop(reopen);
    }
    assert_eq!(
        std::fs::metadata(&folder)?.permissions().mode() & 0o777,
        0o755
    );
    Ok(())
}

/// A preview the cache can't keep is reported as a cache failure.
#[test]
fn storing_an_oversized_edited_preview_is_a_cache_failure() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = edited_png(directory.path(), "1.png")?;
    let cache = PreviewCache::open(&directory.path().join("previews.sqlite3"))?;
    let (sent, kept) = stored(
        &path,
        &Stamp::read(&path)?,
        &image::RgbImage::new(2048, 8),
        cache,
    )?;
    assert!(
        matches!(sent[..], [EditResult::CacheError(_)]),
        "{:?}",
        sent.iter().map(describe).collect::<Vec<_>>()
    );
    assert!(!kept);
    Ok(())
}

fn describe(result: &EditResult) -> String {
    match result {
        EditResult::Ready(id, ticket, _) => format!("Ready({id:?}, {ticket})"),
        EditResult::Kept(id, ticket) => format!("Kept({id:?}, {ticket})"),
        EditResult::Skipped(id, ticket) => format!("Skipped({id:?}, {ticket})"),
        EditResult::Failed(id, ticket) => format!("Failed({id:?}, {ticket})"),
        EditResult::CacheError(error) => format!("CacheError({error})"),
    }
}
