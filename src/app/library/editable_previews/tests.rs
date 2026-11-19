//! Build and Discard Editable Previews through the editor, with the decoder
//! stood in for: what is recorded, what is dropped, and that every ending
//! clears the busy state.
use super::*;
use crate::app::testing::editor_with_catalog;
use crate::app::worker::editable_preview::stand_in::StandIn;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

/// Holds each build until it is opened, and counts the builds that reached it.
#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar, AtomicUsize)>);
impl Gate {
    fn wait(&self) {
        let (open, changed, reached) = &*self.0;
        reached.fetch_add(1, Ordering::SeqCst);
        let mut open = open.lock().unwrap();
        while !*open {
            open = changed.wait(open).unwrap();
        }
    }
    fn open(&self) {
        let (open, changed, _) = &*self.0;
        *open.lock().unwrap() = true;
        changed.notify_all();
    }
    /// Waits until `n` builds have reached the gate.
    fn reached(&self, n: usize) {
        let until = Instant::now() + Duration::from_secs(10);
        while self.0.2.load(Ordering::SeqCst) < n {
            assert!(Instant::now() < until, "no build reached the gate");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// An editor whose catalog holds `names`, building through `copies`.
fn editor(names: &[&str], copies: Copies) -> (tempfile::TempDir, Editor, Vec<PhotoId>) {
    let (dir, mut e, ids) = editor_with_catalog(names).unwrap();
    e.copies = copies;
    (dir, e, ids)
}

/// An editor whose builds wait at `gate`.
fn gated(names: &[&str]) -> (tempfile::TempDir, Editor, Vec<PhotoId>, Gate) {
    let gate = Gate::default();
    let held = gate.clone();
    let (dir, e, ids) = editor(
        names,
        StandIn::building(move |_| {
            held.wait();
            Ok(())
        }),
    );
    (dir, e, ids, gate)
}

/// Takes results until nothing is being built and none is left to take.
fn settle(e: &mut Editor) {
    let until = Instant::now() + Duration::from_secs(10);
    while e.editable_builds.running() {
        assert!(Instant::now() < until, "the builds never finished");
        std::thread::sleep(Duration::from_millis(2));
        e.poll_editable_builds();
    }
    e.poll_editable_builds();
}

fn recorded(e: &Editor, id: PhotoId) -> Option<EditablePreview> {
    let library = e.library.as_ref().unwrap();
    library.session.catalog().editable_preview(id).unwrap()
}

/// The files in the open catalog's copies folder.
fn files(e: &Editor) -> Vec<String> {
    let folder = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .editable_previews_folder();
    files_in(&folder)
}

fn files_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn path_of(e: &Editor, id: PhotoId) -> PathBuf {
    e.library.as_ref().unwrap().photo(id).unwrap().path.clone()
}

#[test]
fn a_build_records_each_masters_copy_once() {
    let (_dir, mut e, ids) = editor(&["a.ARW", "b.ARW", "c.png"], StandIn::copies());
    let queued = e.build_editable_previews(&ids).unwrap();
    // A PNG gets none.
    assert_eq!(
        queued,
        Queued {
            queued: 2,
            skipped: 1
        }
    );
    settle(&mut e);
    for &id in &ids[..2] {
        let copy = recorded(&e, id).expect("recorded");
        let identity = Identity::read(&path_of(&e, id)).unwrap();
        assert_eq!(copy.original, identity);
        assert_eq!(
            copy.file,
            crate::catalog::editable_preview_file_name(id, &identity)
        );
        assert!(e.library.as_ref().unwrap().editable_preview(id).is_some());
    }
    assert_eq!(files(&e).len(), 2);
    // Built again from the same originals: nothing to do.
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert_eq!(files(&e).len(), 2);
    assert!(e.editable_builds.progress().unwrap().failed.is_empty());
}

/// A photo built again after its original changed gets a new copy, and the
/// one it replaces is deleted once the new one is recorded.
#[test]
fn a_rebuild_replaces_the_copy_of_a_changed_original() {
    let (_dir, mut e, ids) = editor(&["a.ARW"], StandIn::copies());
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    let first = recorded(&e, ids[0]).unwrap();
    std::fs::write(path_of(&e, ids[0]), b"another version").unwrap();
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    let second = recorded(&e, ids[0]).unwrap();
    assert_ne!(first.file, second.file);
    assert_eq!(files(&e), vec![second.file]);
}

/// A recorded copy that no longer opens (damaged, or with a record this
/// release doesn't know) is built again, though its original is unchanged.
#[test]
fn a_copy_that_does_not_open_is_built_again() {
    let (_dir, mut e, ids) = editor(&["a.ARW"], StandIn::copies());
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    let copy = recorded(&e, ids[0]).unwrap();
    let folder = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .editable_previews_folder();
    let path = folder.join(&copy.file);
    std::fs::write(&path, b"truncated").unwrap();
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert!(
        e.copies.open(&path, Cancel::NEVER).is_ok(),
        "the damaged copy was replaced"
    );
    assert_eq!(recorded(&e, ids[0]).unwrap().file, copy.file);
    assert!(e.editable_builds.progress().unwrap().failed.is_empty());
}

/// A build that has finished, its file written, is counted in the step that
/// ends it, before its result is sent: until the UI takes it, Build's and
/// Discard's cleanup waits, so the file isn't deleted as an orphan in the
/// moment between the worker finishing and the result arriving.
#[test]
fn a_finished_build_not_yet_taken_keeps_cleanup_waiting() {
    let (tx, results) = mpsc::channel();
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        wake: Condvar::new(),
    });
    let builds = EditableBuilds {
        builder: Some(Builder {
            shared: shared.clone(),
            results,
            thread: None,
        }),
        catalog: None,
        discards: 0,
        discarded: HashMap::new(),
        swept: None,
    };
    assert!(builds.settled());
    let item = Item {
        catalog: CatalogLocation::File(PathBuf::from("a.rawmakase")),
        photo: PhotoId(1),
        name: "a.ARW".into(),
        path: PathBuf::from("a.ARW"),
        demosaic: Demosaic::default(),
        decode_cache: crate::decode_cache::DecodeCache::default(),
        folder: PathBuf::from("copies"),
        recorded: None,
        discards: 0,
    };
    shared.lock().enqueue(vec![item]);
    let (item, _cancel) = shared.next().unwrap();
    assert!(!builds.settled(), "under way");
    let finished = Finished {
        item,
        outcome: Outcome::Built {
            file: "1-a.dng".into(),
            original: Identity {
                size: 1,
                modified_ns: 0,
                prefix_hash: String::new(),
            },
        },
    };
    shared.finish(&finished);
    // Ended, and not sent yet: the worker is between the two.
    assert!(!builds.running(), "no longer running");
    assert!(!builds.settled(), "its file is not an orphan yet");
    tx.send(finished).unwrap();
    assert!(!builds.settled(), "sent, not taken");
}

#[test]
fn a_failing_or_panicking_build_clears_the_busy_state() {
    let (_dir, mut e, ids) = editor(
        &["fails.ARW", "panics.ARW", "works.ARW"],
        StandIn::building(|path| match path.file_name().unwrap().to_str().unwrap() {
            "fails.ARW" => anyhow::bail!("could not decode"),
            "panics.ARW" => panic!("the decoder panicked"),
            _ => Ok(()),
        }),
    );
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert!(!e.editable_builds.running());
    assert!(!e.work_slows_previews());
    let progress = e.editable_builds.progress().unwrap();
    assert_eq!(progress.failed.len(), 2, "{:?}", progress.failed);
    assert!(progress.failed[1].1.contains("stopped unexpectedly"));
    // The worker outlived the panic and built the last one.
    assert!(recorded(&e, ids[0]).is_none());
    assert!(recorded(&e, ids[1]).is_none());
    assert!(recorded(&e, ids[2]).is_some());
    assert_eq!(files(&e).len(), 1);
}

#[test]
fn a_cancelled_build_records_nothing_and_clears_the_busy_state() {
    let (_dir, mut e, ids, gate) = gated(&["a.ARW", "b.ARW"]);
    e.build_editable_previews(&ids).unwrap();
    gate.reached(1);
    assert!(e.editable_builds.running());
    assert!(e.work_slows_previews());
    e.editable_builds.builder.as_ref().unwrap().cancel();
    gate.open();
    settle(&mut e);
    assert!(!e.editable_builds.running());
    assert!(recorded(&e, ids[0]).is_none());
    assert!(recorded(&e, ids[1]).is_none());
    assert!(files(&e).is_empty(), "{:?}", files(&e));
}

/// A result is recorded only while it is still the photo's: not once the
/// photo was removed, its file relinked elsewhere, or its copy built again
/// meanwhile. Nothing it wrote is left behind but a file a row names.
#[test]
fn a_build_whose_photo_was_removed_relinked_or_rebuilt_meanwhile_records_nothing() {
    let (_dir, mut e, ids, gate) = gated(&["removed.ARW", "relinked.ARW", "rebuilt.ARW"]);
    let (removed, relinked, rebuilt) = (ids[0], ids[1], ids[2]);
    e.build_editable_previews(&ids).unwrap();
    gate.reached(1);
    let library = e.library.as_mut().unwrap();
    // Relinked: the photo's file is now elsewhere.
    for photo in library.session.photos_mut_for_tests() {
        if photo.id == relinked {
            photo.path = photo.path.with_file_name("elsewhere.ARW");
        }
    }
    library
        .session
        .photos_mut_for_tests()
        .retain(|p| p.id != removed);
    // Built again by another request, of the file as another time stamped it,
    // recorded meanwhile.
    let identity = Identity {
        modified_ns: 1,
        ..Identity::read(&path_of(&e, rebuilt)).unwrap()
    };
    let elsewhere = crate::catalog::editable_preview_file_name(rebuilt, &identity);
    let library = e.library.as_mut().unwrap();
    library
        .session
        .record_editable_preview(rebuilt, &identity)
        .unwrap();
    gate.open();
    settle(&mut e);
    assert!(recorded(&e, removed).is_none());
    assert!(recorded(&e, relinked).is_none());
    assert_eq!(recorded(&e, rebuilt).unwrap().file, elsewhere);
    assert!(files(&e).is_empty(), "{:?}", files(&e));
}

/// A rebuild of a master's copy that finishes after Set Copy as Master moved
/// the row to its virtual copy, and the former master was removed, records
/// nothing for the photo it was asked for, and keeps the file the new
/// master's row still names.
#[test]
fn a_rebuild_keeps_a_copy_another_master_now_names() {
    let (_dir, mut e, ids, gate) = gated(&["a.ARW"]);
    let a = ids[0];
    let identity = Identity::read(&path_of(&e, a)).unwrap();
    let library = e.library.as_mut().unwrap();
    let b = library.create_virtual_copy(a).unwrap().value;
    library
        .session
        .record_editable_preview(a, &identity)
        .unwrap();
    let folder = library.session.catalog().editable_previews_folder();
    let file = crate::catalog::editable_preview_file_name(a, &identity);
    // Damaged, so Build writes it again under the same name.
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(&file), b"damaged").unwrap();
    e.build_editable_previews(&[a]).unwrap();
    gate.reached(1);
    let library = e.library.as_mut().unwrap();
    library.set_copy_as_master(b).unwrap();
    library.remove_virtual_copy(a).unwrap();
    gate.open();
    settle(&mut e);
    assert_eq!(recorded(&e, b).unwrap().file, file);
    assert_eq!(files(&e), vec![file], "the new master's copy is kept");
}

/// Switching catalogs while a build runs drops its results: no row in either
/// catalog, and no file left in the first one's copies folder.
#[test]
fn switching_catalogs_mid_build_drops_its_results() {
    let (_dir, mut e, ids, gate) = gated(&["a.ARW", "b.ARW"]);
    let first = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    let folder = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .editable_previews_folder();
    e.build_editable_previews(&ids).unwrap();
    gate.reached(1);
    let (_other_dir, other, _) = editor_with_catalog(&["c.ARW"]).unwrap();
    e.library = other.library;
    e.poll_editable_builds();
    gate.open();
    settle(&mut e);
    assert!(!e.editable_builds.running());
    let crate::catalog::CatalogLocation::File(first) = first;
    let first = crate::catalog::Catalog::open(&first).unwrap();
    assert!(first.editable_previews().unwrap().is_empty());
    let second = e.library.as_ref().unwrap().session.catalog();
    assert!(second.editable_previews().unwrap().is_empty());
    assert!(files_in(&folder).is_empty(), "{:?}", files_in(&folder));
}

/// Discard while a build of the photo runs: the catalog forgets its copy and
/// its file goes at once, and the build's result, arriving later, is dropped.
#[test]
fn discard_while_a_build_runs_leaves_no_copy() {
    let gate = Gate::default();
    let held = gate.clone();
    let hold = Arc::new(AtomicBool::new(false));
    let holding = hold.clone();
    let (_dir, mut e, ids) = editor(
        &["a.ARW"],
        StandIn::building(move |_| {
            if holding.load(Ordering::SeqCst) {
                held.wait();
            }
            Ok(())
        }),
    );
    // A copy recorded already, then the original changed and built again.
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert_eq!(files(&e).len(), 1);
    std::fs::write(path_of(&e, ids[0]), b"another version").unwrap();
    hold.store(true, Ordering::SeqCst);
    e.build_editable_previews(&ids).unwrap();
    gate.reached(1);
    e.discard_editable_previews(&ids);
    assert!(recorded(&e, ids[0]).is_none());
    assert!(files(&e).is_empty());
    gate.open();
    settle(&mut e);
    assert!(recorded(&e, ids[0]).is_none());
    assert!(files(&e).is_empty(), "{:?}", files(&e));
    assert!(
        e.library
            .as_ref()
            .unwrap()
            .editable_preview(ids[0])
            .is_none()
    );
}

/// Discard forgets the copies in the catalog, then deletes their files and
/// any file no row names; a virtual copy's discards its master's.
#[test]
fn discard_deletes_the_files_after_the_catalog_forgets_them() {
    let (_dir, mut e, ids) = editor(&["a.ARW", "b.ARW"], StandIn::copies());
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    let folder = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .editable_previews_folder();
    std::fs::write(folder.join("9999-left-by-a-removal.dng"), b"orphan").unwrap();
    e.discard_editable_previews(&ids[..1]);
    assert!(recorded(&e, ids[0]).is_none());
    let kept = recorded(&e, ids[1]).unwrap();
    assert_eq!(files(&e), vec![kept.file]);
    assert!(
        e.messages.line().starts_with("Discarded"),
        "{}",
        e.messages.line()
    );
}

/// Removes every folder of the open catalog through the editor, as the
/// Folders panel's Remove does.
fn remove_all_folders(e: &mut Editor) {
    let library = e.library.as_ref().unwrap();
    let removal = crate::app::library::FolderRemoval {
        catalog: library.session.catalog().location().clone(),
        name: "photos".into(),
        folders: library.session.folders().iter().map(|f| f.id).collect(),
        photos: library.session.photos().len(),
        empty_root: None,
        exported: None,
    };
    e.remove_folders(&removal);
    assert!(library_photos(e).is_empty(), "{}", e.messages.line());
}

fn library_photos(e: &Editor) -> Vec<PhotoId> {
    let library = e.library.as_ref().unwrap();
    library.session.photos().iter().map(|p| p.id).collect()
}

/// Removing photos deletes their copies' files on the next frame once the
/// removal has committed, with no Build or Discard after it.
#[test]
fn removing_photos_deletes_their_copies() {
    let (_dir, mut e, ids) = editor(&["a.ARW", "b.ARW"], StandIn::copies());
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert_eq!(files(&e).len(), 2);
    remove_all_folders(&mut e);
    e.poll_editable_builds();
    assert!(files(&e).is_empty(), "{:?}", files(&e));
}

/// A removal while a build is under way leaves the files until nothing being
/// built could still record one, then deletes them, the build's own included.
#[test]
fn a_removal_during_a_build_deletes_the_copies_once_it_finishes() {
    let gate = Gate::default();
    let held = gate.clone();
    let hold = Arc::new(AtomicBool::new(false));
    let holding = hold.clone();
    let (_dir, mut e, ids) = editor(
        &["a.ARW", "b.ARW"],
        StandIn::building(move |_| {
            if holding.load(Ordering::SeqCst) {
                held.wait();
            }
            Ok(())
        }),
    );
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert_eq!(files(&e).len(), 2);
    // `b` built again from a changed original, held under way.
    std::fs::write(path_of(&e, ids[1]), b"another version").unwrap();
    hold.store(true, Ordering::SeqCst);
    e.build_editable_previews(&ids[1..]).unwrap();
    gate.reached(1);
    remove_all_folders(&mut e);
    e.poll_editable_builds();
    assert_eq!(files(&e).len(), 2, "deleted while a build was under way");
    gate.open();
    settle(&mut e);
    assert!(files(&e).is_empty(), "{:?}", files(&e));
}

/// Quitting stops the worker within the deadline, whatever it was building.
#[test]
fn closing_stops_the_worker() {
    let (_dir, mut e, ids, gate) = gated(&["a.ARW"]);
    e.build_editable_previews(&ids).unwrap();
    gate.reached(1);
    let thread = e.editable_builds.close().expect("a worker");
    gate.open();
    let until = Instant::now() + Duration::from_secs(10);
    while !thread.is_finished() {
        assert!(Instant::now() < until, "the worker never stopped");
        std::thread::sleep(Duration::from_millis(2));
    }
    thread.join().unwrap();
}
