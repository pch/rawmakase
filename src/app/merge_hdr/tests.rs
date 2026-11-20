//! Merge to HDR's lifecycle in the editor, with a stub in place of the merge:
//! what it adds, stacks and shows, and how every other ending leaves the
//! catalog and the busy state.
use super::*;
use crate::app::testing::editor_with_catalog;
use crate::model::recipe::Recipe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const NAMES: [&str; 3] = ["IMG_1.CR3", "IMG_2.CR3", "IMG_3.CR3"];

/// The edit a stub's merge starts with.
fn starting() -> Recipe {
    Recipe {
        exposure: 0.5,
        crop: [0.01, 0.02, 0.98, 0.99],
        ..Default::default()
    }
}

/// Where the merge of `job` is published: beside its active photo.
fn beside(job: &Job) -> PathBuf {
    let active = &job.sources[job.active];
    let stem = active.file_stem().unwrap().to_string_lossy();
    active.with_file_name(format!("{stem}-HDR.dng"))
}

/// A merge that publishes a file beside the active photo, after `before`.
fn publishing(before: impl Fn(Cancel<'_>) -> Result<()> + Send + Sync + 'static) -> Stub {
    Arc::new(move |job, cancel| {
        before(cancel)?;
        let path = beside(job);
        std::fs::write(&path, b"fixture")?;
        Ok(Published {
            path,
            recipe: starting(),
            notes: Vec::new(),
        })
    })
}

/// A merge that waits for `cancel`, as a real one stops at its next check.
fn until_cancelled() -> Stub {
    Arc::new(|_, cancel| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cancel.is_cancelled() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(rawmakase_hdr_merge::Error::Cancelled.into())
    })
}

/// Opens the window on `ids`, sets Create Stack and Open after Merge, and
/// merges with `stub`.
fn merge(e: &mut Editor, ids: &[PhotoId], stack: bool, open: bool, stub: Stub) {
    e.merge_hdr.stub = Some(stub);
    e.open_merge_hdr(ids.to_vec());
    let mut dialog = e.merge_hdr.dialog.take().expect("the window opens");
    assert_eq!(dialog.problem, None);
    dialog.stack = stack;
    dialog.open = open;
    e.start_merge_hdr(dialog);
    assert!(e.merge_hdr.running.is_some(), "{}", e.messages.line());
}

/// Delivers events, as frames do, until the merge has ended.
fn settle(e: &mut Editor) {
    let ctx = e.context.clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    while e.merge_hdr.running.is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
        e.events(&ctx);
        e.add_merged();
    }
    assert!(e.merge_hdr.running.is_none(), "the merge never ended");
    assert!(!e.activity.is_busy());
    assert_eq!(e.closing_waits_for(), None);
}

fn listed(e: &Editor, name: &str) -> Option<PhotoId> {
    e.library
        .as_ref()?
        .session
        .photos()
        .iter()
        .find(|p| p.filename == name)
        .map(|p| p.id)
}

fn photos_in_catalog(e: &Editor) -> usize {
    let location = e.library.as_ref().unwrap().session.catalog().location();
    crate::catalog::Catalog::open(location)
        .unwrap()
        .photos()
        .unwrap()
        .len()
}

#[test]
fn a_merge_is_added_with_its_edit_stacked_on_its_photos_and_selected() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    // The active photo names the merge.
    e.library.as_mut().unwrap().make_active(ids[1]);
    merge(&mut e, &ids, true, false, publishing(|_| Ok(())));
    assert_eq!(e.closing_waits_for().as_deref(), Some(MERGE_HDR));
    settle(&mut e);
    let photo = listed(&e, "IMG_2-HDR.dng").expect("the merge is in the Library");
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.selected(), Some(photo));
    let path = &library.photo(photo).unwrap().path;
    assert_eq!(
        library
            .session
            .catalog()
            .load_edit(photo, path)?
            .map(|edit| edit.recipe),
        Some(starting())
    );
    let stack = library.photo(photo).unwrap().stack.expect("stacked");
    assert!(stack.is_top() && stack.collapsed);
    assert_eq!(stack.count, 4);
    for id in &ids {
        assert_eq!(
            library.photo(*id).unwrap().stack.map(|s| s.id),
            Some(stack.id)
        );
    }
    assert_eq!(e.messages.line(), "Merged to HDR: IMG_2-HDR.dng");

    // Undo takes the stack back; the merge stays in the catalog.
    e.sync_undo();
    e.undo();
    let library = e.library.as_ref().unwrap();
    assert!(library.photo(photo).unwrap().stack.is_none());
    assert!(
        ids.iter()
            .all(|id| library.photo(*id).unwrap().stack.is_none())
    );
    assert_eq!(photos_in_catalog(&e), 4);
    Ok(())
}

#[test]
fn without_create_stack_the_merge_is_opened_unstacked() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    merge(&mut e, &ids, false, true, publishing(|_| Ok(())));
    settle(&mut e);
    let photo = listed(&e, "IMG_1-HDR.dng").unwrap();
    assert!(
        e.library
            .as_ref()
            .unwrap()
            .photo(photo)
            .unwrap()
            .stack
            .is_none()
    );
    assert_eq!(e.module, Module::Develop);
    assert_eq!(e.document.catalog_photo, Some(photo));
    Ok(())
}

#[test]
fn a_failed_merge_adds_nothing_and_says_why() -> Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    let folder = dir.path().join("photos");
    let why = format!(
        "The merged DNG couldn't be saved in {}: disk full",
        folder.display()
    );
    let failure = why.clone();
    merge(
        &mut e,
        &ids,
        true,
        true,
        Arc::new(move |_, _| Err(anyhow::anyhow!(failure.clone()))),
    );
    settle(&mut e);
    assert_eq!(e.messages.line(), format!("Merge to HDR: {why}"));
    assert_eq!(photos_in_catalog(&e), 3);
    assert_eq!(e.module, Module::Library);
    Ok(())
}

#[test]
fn a_merge_that_panics_ends_and_clears_busy() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    merge(
        &mut e,
        &ids,
        true,
        true,
        Arc::new(|_, _| panic!("decoder broke")),
    );
    settle(&mut e);
    assert_eq!(
        e.messages.line(),
        "Merge to HDR: Stopped unexpectedly: decoder broke"
    );
    assert_eq!(photos_in_catalog(&e), 3);
    Ok(())
}

#[test]
fn a_cancelled_merge_publishes_and_adds_nothing() -> Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    merge(&mut e, &ids, true, true, until_cancelled());
    e.cancel_merge_hdr();
    // Nothing waits for it from now on, closing included.
    assert!(!e.merge_hdr.running());
    assert_eq!(e.closing_waits_for(), None);
    assert_eq!(e.messages.line(), "Merge to HDR cancelled");
    let thread = e.merge_hdr.thread.take().unwrap();
    let waited = task::wait_for(
        vec![task::Stopping::new(Some(thread))],
        Duration::from_secs(10),
    );
    assert_eq!(waited.detached, 0);
    e.events(&e.context.clone());
    assert!(!dir.path().join("photos/IMG_1-HDR.dng").exists());
    assert_eq!(photos_in_catalog(&e), 3);
    assert!(!e.activity.is_busy());
    Ok(())
}

/// A merge that publishes just as it is cancelled: its result is stale,
/// and the file it saved is named rather than added.
#[test]
fn a_merge_done_as_it_was_cancelled_is_named_not_added() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    merge(
        &mut e,
        &ids,
        true,
        true,
        publishing(move |_| {
            let _ = wait.lock().unwrap().recv();
            Ok(())
        }),
    );
    e.cancel_merge_hdr();
    release.send(())?;
    let thread = e.merge_hdr.thread.take().unwrap();
    task::wait_for(
        vec![task::Stopping::new(Some(thread))],
        Duration::from_secs(10),
    );
    e.events(&e.context.clone());
    e.add_merged();
    assert!(
        e.messages
            .line()
            .ends_with("IMG_1-HDR.dng, not added to the catalog")
    );
    assert_eq!(photos_in_catalog(&e), 3);
    assert!(!e.activity.is_busy());
    Ok(())
}

#[test]
fn a_merge_into_a_catalog_closed_meanwhile_is_left_where_it_was_saved() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    merge(
        &mut e,
        &ids,
        true,
        true,
        publishing(move |_| {
            let _ = wait.lock().unwrap().recv();
            Ok(())
        }),
    );
    let first = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    let (_other_dir, other, _) = editor_with_catalog(&["other.CR3"])?;
    e.library = other.library;
    release.send(())?;
    settle(&mut e);
    let line = e.messages.line().to_string();
    assert!(line.contains("IMG_1-HDR.dng"), "{line}");
    assert!(line.contains("another catalog is open now"), "{line}");
    assert_eq!(photos_in_catalog(&e), 1);
    assert_eq!(crate::catalog::Catalog::open(&first)?.photos()?.len(), 3);
    Ok(())
}

#[test]
fn a_photo_removed_meanwhile_leaves_the_merge_stacked_with_the_rest() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    // A virtual copy of the third, which can go while the merge runs.
    let library = e.library.as_mut().unwrap();
    let copy = library.create_virtual_copy(ids[2])?.value;
    // Making the copy selected it: the first photo names the merge.
    library.make_active(ids[0]);
    let sources = [ids[0], ids[1], copy];
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    merge(
        &mut e,
        &sources,
        true,
        false,
        publishing(move |_| {
            let _ = wait.lock().unwrap().recv();
            Ok(())
        }),
    );
    e.remove_virtual_copy(copy);
    assert!(listed(&e, "IMG_3.CR3").is_some());
    release.send(())?;
    settle(&mut e);
    let library = e.library.as_ref().unwrap();
    let photo = listed(&e, "IMG_1-HDR.dng").unwrap();
    let stack = library.photo(photo).unwrap().stack.expect("stacked");
    assert_eq!(stack.count, 3);
    assert!(library.photo(ids[2]).unwrap().stack.is_none());
    Ok(())
}

#[test]
fn a_merge_waits_for_the_catalog_while_other_work_holds_it() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    merge(
        &mut e,
        &ids,
        false,
        false,
        publishing(move |_| {
            let _ = wait.lock().unwrap().recv();
            Ok(())
        }),
    );
    // A file dialog holds the catalog's foreground activity.
    assert!(e.activity.begin_dialog());
    release.send(())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !matches!(
        e.merge_hdr.running.as_ref().map(|r| &r.phase),
        Some(Phase::Waiting(_))
    ) && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(5));
        e.events(&e.context.clone());
    }
    for _ in 0..3 {
        e.add_merged();
    }
    assert!(matches!(
        e.merge_hdr.running.as_ref().map(|r| &r.phase),
        Some(Phase::Waiting(_))
    ));
    // Closing waits for it, and offers to give up on adding it.
    assert_eq!(e.closing_waits_for().as_deref(), Some(MERGE_HDR));
    assert_eq!(photos_in_catalog(&e), 3);
    e.activity.finish_dialog();
    settle(&mut e);
    assert!(listed(&e, "IMG_1-HDR.dng").is_some());
    Ok(())
}

#[test]
fn a_merge_the_catalog_refuses_names_where_it_was_saved() -> Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    // Saved as a file the catalog doesn't take for a photo.
    let path = dir.path().join("photos/IMG_1-HDR.txt");
    let saved = path.clone();
    merge(
        &mut e,
        &ids,
        true,
        true,
        Arc::new(move |_, _| {
            std::fs::write(&saved, b"fixture")?;
            Ok(Published {
                path: saved.clone(),
                recipe: starting(),
                notes: Vec::new(),
            })
        }),
    );
    settle(&mut e);
    assert_eq!(
        e.messages.line(),
        format!(
            "Merge to HDR: The merge was saved at {}, but couldn't be added to the catalog: \
             the catalog doesn't read it as a photo",
            path.display()
        )
    );
    assert_eq!(photos_in_catalog(&e), 3);
    assert_eq!(e.module, Module::Library);
    Ok(())
}

#[test]
fn quitting_cancels_the_merge_and_waits_for_it_within_the_deadline() -> Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    let stopped = Arc::new(AtomicBool::new(false));
    let saw = stopped.clone();
    merge(
        &mut e,
        &ids,
        true,
        true,
        Arc::new(move |_, cancel| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !cancel.is_cancelled() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            // Ending takes a moment after the cancel is seen.
            std::thread::sleep(Duration::from_millis(50));
            saw.store(true, Ordering::SeqCst);
            Err(rawmakase_hdr_merge::Error::Cancelled.into())
        }),
    );
    let started = Instant::now();
    e.exit_within(Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(5));
    // Waited for, not left running.
    assert!(stopped.load(Ordering::SeqCst));
    assert!(!dir.path().join("photos/IMG_1-HDR.dng").exists());
    Ok(())
}

#[test]
fn a_merge_that_ignores_its_cancel_is_left_at_the_deadline() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    merge(
        &mut e,
        &ids,
        true,
        true,
        Arc::new(move |_, _| {
            // A read stalled on a network share.
            let _ = wait.lock().unwrap().recv();
            Err(rawmakase_hdr_merge::Error::Cancelled.into())
        }),
    );
    let started = Instant::now();
    let waited = e.exit_within(Duration::from_millis(200));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(waited.detached >= 1);
    drop(release);
    Ok(())
}

#[test]
fn a_second_merge_waits_for_the_first() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    merge(&mut e, &ids, true, true, until_cancelled());
    e.open_merge_hdr(ids.clone());
    assert!(!e.merge_hdr.dialog_open());
    assert!(e.messages.line().starts_with("A merge is already running"));
    Ok(())
}

#[test]
fn a_new_merge_waits_for_a_cancelled_one_to_stop() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    merge(
        &mut e,
        &ids,
        true,
        true,
        Arc::new(move |_, _| {
            // A read stalled on a network share, which the cancel can't stop.
            let _ = wait.lock().unwrap().recv();
            Err(rawmakase_hdr_merge::Error::Cancelled.into())
        }),
    );
    e.cancel_merge_hdr();
    e.open_merge_hdr(ids.clone());
    assert!(!e.merge_hdr.dialog_open());
    assert!(
        e.messages
            .line()
            .starts_with("The merge cancelled is still stopping")
    );
    release.send(())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !e.merge_hdr.thread.as_ref().unwrap().is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    e.open_merge_hdr(ids);
    assert!(e.merge_hdr.dialog_open());
    Ok(())
}

#[test]
fn the_window_says_why_photos_are_no_bracket() {
    use crate::metadata::PhotoInfo;
    let shot = |camera: &str, exposure: f64| PhotoInfo {
        camera: Some(camera.into()),
        exposure: Some(exposure),
        aperture: Some(8.),
        iso: Some(100.),
        ..Default::default()
    };
    let (a, b, c) = (
        Path::new("/p/a.CR3"),
        Path::new("/p/b.CR3"),
        Path::new("/p/c.CR3"),
    );
    let photos = [("a.CR3", a), ("b.CR3", b), ("c.CR3", c)];
    let bracket = [
        shot("X", 1. / 250.),
        shot("X", 1. / 60.),
        shot("X", 1. / 15.),
    ];
    assert_eq!(problem(&photos, &bracket), None);
    // What the catalog doesn't know is left to the merge.
    assert_eq!(
        problem(
            &photos,
            &[Default::default(), Default::default(), Default::default()]
        ),
        None
    );
    assert_eq!(
        problem(&photos[..1], &bracket[..1]).as_deref(),
        Some("A bracket is 2 to 9 photos, not 1 photo.")
    );
    assert_eq!(
        problem(&[("a.CR3", a), ("a.CR3", a)], &bracket[..2]).as_deref(),
        Some("a.CR3 is chosen twice, with a virtual copy: choose each photo once.")
    );
    assert_eq!(
        problem(
            &photos,
            &[
                shot("X", 1. / 250.),
                shot("Y", 1. / 60.),
                shot("X", 1. / 15.)
            ]
        )
        .as_deref(),
        Some("The photos were taken with more than one camera: X and Y.")
    );
    // An ISO one photo doesn't record counts as the same in all, as in the
    // merge's check: these differ by their times alone.
    let no_iso = PhotoInfo {
        iso: None,
        ..shot("X", 1. / 15.)
    };
    let iso_400 = |exposure| PhotoInfo {
        iso: Some(400.),
        ..shot("X", exposure)
    };
    // Filled in with ISO 100, the third would be the second's exposure.
    let mixed = [iso_400(1. / 250.), iso_400(1. / 60.), no_iso];
    assert_eq!(problem(&photos, &mixed), None);
    assert_eq!(
        problem(
            &photos,
            &[
                shot("X", 1. / 250.),
                shot("X", 1. / 60.),
                shot("X", 1. / 240.)
            ]
        )
        .as_deref(),
        Some("a.CR3 and c.CR3 have the same exposure: a bracket's photos each have their own.")
    );
}

/// A catalog dropped on the window while it is open: its photo ids name
/// other photos, so the window merges nothing.
#[test]
fn the_window_merges_nothing_once_another_catalog_is_open() -> Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&NAMES)?;
    e.merge_hdr.stub = Some(publishing(|_| Ok(())));
    e.open_merge_hdr(ids);
    let dialog = e.merge_hdr.dialog.take().unwrap();
    // The same ids, other photos.
    let (_other_dir, other, _) = editor_with_catalog(&["A.CR3", "B.CR3", "C.CR3"])?;
    e.library = other.library;
    e.start_merge_hdr(dialog);
    assert!(e.merge_hdr.running.is_none());
    assert!(
        e.messages.line().contains("another catalog is open now"),
        "{}",
        e.messages.line()
    );
    assert!(listed(&e, "A-HDR.dng").is_none());
    Ok(())
}
