//! Build and Discard Editable Previews, through the editor as the menu and
//! automation reach them, with the decode stood in for (`test_open`) and
//! everything after it (the format, the store folder, the catalog's record) real.
use super::*;
use crate::app::testing::editor_with_catalog;
use crate::camera_profiles::fitted;
use eframe::egui;
use std::collections::HashSet;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

/// Every original the test steps opened, by file.
static OPENED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// The thread each opening ran on, by file: which pool, so which pace.
static OPENED_ON: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());
/// "slow" builds that have started, and those let go.
static STARTED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static RELEASED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
/// "absent" photos' fits are refused while set.
static REFUSE_FIT: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
/// Photos whose opening fails while set, whatever their names.
static FAIL: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// The camera of the stand-in decodes, with a colour matrix unless `bare`.
fn metadata(width: u32, height: u32, bare: bool) -> Metadata {
    let mut m = Metadata {
        make: "Testmaker".into(),
        model: "T-1".into(),
        width,
        height,
        wb: [2., 1., 1.5],
        ..Default::default()
    };
    if !bare {
        m.matrix_source = crate::camera_data::MatrixSource::File;
        m.cam_xyz = [[0.8, -0.2, -0.1], [-0.4, 1.2, 0.2], [-0.05, 0.2, 0.6]];
    }
    m
}

/// Opens by the file's name: "panic…" panics, "fail…" (or one in [`FAIL`])
/// fails, "slow…" waits until released or cancelled, "stubborn…" waits until
/// released and only then sees a cancel, "absent…" has no colour data; anything
/// else is a small full-size decode of a camera with a matrix.
fn test_open(item: &Item, _: &Identity, cancel: Cancel<'_>) -> anyhow::Result<Original> {
    OPENED.lock().unwrap().push(item.path.clone());
    let thread = std::thread::current().name().unwrap_or_default().to_owned();
    OPENED_ON.lock().unwrap().push((item.path.clone(), thread));
    let name = item.path.file_name().unwrap().to_string_lossy().to_string();
    if name.starts_with("panic") {
        panic!("test panic");
    }
    anyhow::ensure!(
        !name.starts_with("fail") && !in_set(&FAIL, &item.path),
        "test failure"
    );
    if name.starts_with("slow") {
        STARTED.lock().unwrap().push(item.path.clone());
        while !in_set(&RELEASED, &item.path) {
            anyhow::ensure!(!cancel.is_cancelled(), "Cancelled");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    // Sees a cancel only once released, as a long decode does between its stages,
    // so a test can order what happens while it runs.
    if name.starts_with("stubborn") {
        STARTED.lock().unwrap().push(item.path.clone());
        while !in_set(&RELEASED, &item.path) {
            std::thread::sleep(Duration::from_millis(2));
        }
        anyhow::ensure!(!cancel.is_cancelled(), "Cancelled");
    }
    let (w, h) = (48, 32);
    let image = CameraImage {
        width: w,
        height: h,
        recovered: Default::default(),
        pixels: (0..w * h).map(|i| [i as f32 / 900., 0.5, 0.25]).collect(),
        metadata: metadata(w, h, name.starts_with("absent")),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    Ok(Original {
        image,
        exif: None,
        profiles: Vec::new(),
    })
}

/// A fitted look for an `Absent` photo, as the real fit's eligibility decides,
/// unless the test refuses it.
fn test_fit(
    path: &Path,
    m: &Metadata,
    profiles: &[Arc<CameraProfile>],
    _: Cancel<'_>,
) -> DefaultFit {
    if fitted::eligibility(m, profiles) != fitted::Eligibility::Absent {
        return DefaultFit::NotDefault;
    }
    if in_set(&REFUSE_FIT, path) {
        return DefaultFit::Missing("No embedded preview".into());
    }
    let base = fitted::matrix_base(m, [[0.4, 0.4, 0.2], [0.2, 0.7, 0.1], [0., 0.1, 0.8]]).unwrap();
    DefaultFit::Fitted(Box::new(
        fitted::with_look(&base, &fitted::FittedLook::identity()).unwrap(),
    ))
}

const STEPS: Steps = Steps {
    open: test_open,
    fit: test_fit,
};

fn in_set(set: &Mutex<Option<HashSet<PathBuf>>>, path: &Path) -> bool {
    set.lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.contains(path))
}
fn add(set: &Mutex<Option<HashSet<PathBuf>>>, path: &Path) {
    set.lock()
        .unwrap()
        .get_or_insert_with(HashSet::new)
        .insert(path.to_path_buf());
}
fn release(path: &Path) {
    add(&RELEASED, path);
}
fn opens_of(path: &Path) -> usize {
    OPENED.lock().unwrap().iter().filter(|p| *p == path).count()
}
/// Whether each opening of `path` so far ran on the full pace's pool.
fn opened_at_full(path: &Path) -> Vec<bool> {
    OPENED_ON
        .lock()
        .unwrap()
        .iter()
        .filter(|(p, _)| p == path)
        .map(|(_, thread)| thread.starts_with("editable-previews-full"))
        .collect()
}
fn wait_opens(path: &Path, n: usize) {
    let until = Instant::now() + Duration::from_secs(10);
    while opens_of(path) < n {
        assert!(
            Instant::now() < until,
            "opened {} times, not {n}",
            opens_of(path)
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn wait_started(path: &Path) {
    let until = Instant::now() + Duration::from_secs(10);
    while !STARTED.lock().unwrap().iter().any(|p| p == path) {
        assert!(Instant::now() < until, "the build never started");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn editor(names: &[&str]) -> (tempfile::TempDir, Editor, Vec<PhotoId>) {
    let (dir, mut e, ids) = editor_with_catalog(names).unwrap();
    e.editable_builds = EditableBuilds::for_tests(STEPS);
    // Every fixture different, so no two photos are the same file.
    for id in &ids {
        let path = path_of(&e, *id);
        std::fs::write(&path, format!("fixture of {}", path.display())).unwrap();
    }
    (dir, e, ids)
}
fn path_of(e: &Editor, id: PhotoId) -> PathBuf {
    e.library.as_ref().unwrap().photo(id).unwrap().path.clone()
}
fn id_named(e: &Editor, ids: &[PhotoId], name: &str) -> PhotoId {
    *ids.iter()
        .find(|id| path_of(e, **id).ends_with(name))
        .unwrap()
}
fn catalog(e: &Editor) -> &Catalog {
    e.library.as_ref().unwrap().session.catalog()
}
fn recorded(e: &Editor, id: PhotoId) -> Option<crate::catalog::EditablePreview> {
    catalog(e).editable_preview(id).unwrap()
}
/// The editable preview files in the store folder.
fn files(e: &Editor) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(catalog(e).editable_preview_folder())
        .map(|d| {
            d.flatten()
                .map(|f| f.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
/// Polls until every build asked for has ended.
fn settle(e: &mut Editor) {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        e.poll_editable_builds();
        if !e.editable_builds.running() {
            std::thread::sleep(Duration::from_millis(5));
            e.poll_editable_builds();
            return;
        }
        assert!(Instant::now() < until, "builds never ended");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn progress(e: &Editor) -> Progress {
    e.editable_builds.progress().unwrap_or_default()
}
fn copy_of(e: &mut Editor, master: PhotoId) -> PhotoId {
    e.library
        .as_mut()
        .unwrap()
        .create_virtual_copy(master)
        .unwrap()
        .value
}

#[test]
fn offline_and_unsupported_photos_are_skipped_and_copies_build_their_master_once() {
    let (_dir, mut e, ids) = editor(&["a.ARW", "b.ARW", "c.JPG", "d.ARW"]);
    let [a, b, c, d] = [
        id_named(&e, &ids, "a.ARW"),
        id_named(&e, &ids, "b.ARW"),
        id_named(&e, &ids, "c.JPG"),
        id_named(&e, &ids, "d.ARW"),
    ];
    let copy = copy_of(&mut e, a);
    std::fs::remove_file(path_of(&e, d)).unwrap();
    let queued = e.build_editable_previews(&[copy, a, b, c, d]).unwrap();
    // The JPEG and the offline RAW are skipped; the copy and its master are one.
    assert_eq!(
        queued,
        Queued {
            queued: 2,
            skipped: 2
        }
    );
    settle(&mut e);
    assert_eq!(opens_of(&path_of(&e, a)), 1);
    assert_eq!(opens_of(&path_of(&e, c)), 0);
    let p = progress(&e);
    assert_eq!((p.done, p.total), (2, 2));
    assert!(p.failed.is_empty() && p.without_look.is_empty(), "{p:?}");
    for id in [a, b] {
        let row = recorded(&e, id).expect("recorded");
        assert_eq!(row.photo, id);
        let identity = Identity::read(&path_of(&e, id)).unwrap();
        assert_eq!(row.original, identity);
        let name = row.path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name, editable_preview_file_name(id, &identity));
        let stem = name.strip_suffix(".rmep").unwrap();
        assert!(
            stem.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-'),
            "{name}"
        );
        let file = editable_preview::read(&row.path, Cancel::NEVER).unwrap();
        assert_eq!(file.identity(), &identity);
        assert!(file.camera_look().is_none(), "the camera has colour data");
    }
    assert_eq!(
        recorded(&e, copy),
        recorded(&e, a),
        "the copy shares its master's"
    );
    assert_eq!(files(&e).len(), 2);
}

#[test]
fn an_unchanged_original_is_skipped_and_a_stale_one_rebuilt() {
    let (_dir, mut e, ids) = editor(&["a.ARW"]);
    let a = ids[0];
    let path = path_of(&e, a);
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    let first = recorded(&e, a).unwrap();
    // Built again from the same file: skipped, without opening it.
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert_eq!(opens_of(&path), 1);
    assert_eq!(recorded(&e, a).unwrap().path, first.path);
    // Touched, the same photo still: skipped too.
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    drop(f);
    assert!(!first.is_stale(&Identity::read(&path).unwrap()));
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert_eq!(opens_of(&path), 1);
    // Another photo in the file: the stale one goes, a new one is built.
    std::fs::write(&path, b"another photo, another size").unwrap();
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert_eq!(opens_of(&path), 2);
    let second = recorded(&e, a).unwrap();
    assert_ne!(second.path, first.path);
    assert!(!first.path.exists(), "the stale file is deleted");
    assert_eq!(
        files(&e),
        [second.path.file_name().unwrap().to_string_lossy()]
    );
    // Its file gone (deleted outside RAWmakase): built again.
    std::fs::remove_file(&second.path).unwrap();
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert_eq!(opens_of(&path), 3);
    assert!(recorded(&e, a).unwrap().path.is_file());
}

/// A file a failed write or an earlier build left at the name: a whole one of
/// this original is recorded as it is; anything else is removed and written over.
#[test]
fn a_file_left_at_the_name_is_recorded_when_whole_and_replaced_when_not() {
    let (_dir, mut e, ids) = editor(&["absent-a.ARW", "b.ARW"]);
    let (a, b) = (
        id_named(&e, &ids, "absent-a.ARW"),
        id_named(&e, &ids, "b.ARW"),
    );
    let path = path_of(&e, a);
    // Built once without its look, then left unrecorded.
    add(&REFUSE_FIT, &path);
    e.build_editable_previews(&[a]).unwrap();
    settle(&mut e);
    let row = recorded(&e, a).unwrap();
    let left = std::fs::read(&row.path).unwrap();
    e.discard_editable_previews(Some(&[a])).unwrap();
    assert!(!row.path.exists());
    std::fs::write(&row.path, &left).unwrap();
    // Now the fit would succeed, but the whole file there is kept, and says so.
    REFUSE_FIT.lock().unwrap().as_mut().unwrap().remove(&path);
    e.build_editable_previews(&[a]).unwrap();
    settle(&mut e);
    assert_eq!(recorded(&e, a).unwrap().path, row.path);
    assert_eq!(std::fs::read(&row.path).unwrap(), left);
    let p = progress(&e);
    assert!(p.failed.is_empty(), "{p:?}");
    assert_eq!(p.without_look.len(), 1, "{p:?}");
    assert!(p.without_look[0].1.contains("kept"), "{p:?}");
    // Junk at b's name: removed, and the build written in its place.
    let identity = Identity::read(&path_of(&e, b)).unwrap();
    let name = editable_preview_file_name(b, &identity);
    let junk = catalog(&e).editable_preview_folder().join(&name);
    std::fs::write(&junk, b"a partial copy").unwrap();
    e.build_editable_previews(&[b]).unwrap();
    settle(&mut e);
    assert!(progress(&e).failed.is_empty());
    assert_eq!(recorded(&e, b).unwrap().path, junk);
    assert!(editable_preview::read(&junk, Cancel::NEVER).is_ok());
}

/// Waits for `name`'s build to start, does `change`, lets it go and settles;
/// returns why it failed.
fn changed_mid_build(name: &str, change: impl FnOnce(&mut Editor, PhotoId)) -> String {
    let (_dir, mut e, ids) = editor(&[name]);
    let id = ids[0];
    let path = path_of(&e, id);
    e.build_editable_previews(&ids).unwrap();
    wait_started(&path);
    change(&mut e, id);
    release(&path);
    settle(&mut e);
    let p = progress(&e);
    assert_eq!(p.failed.len(), 1, "{p:?}");
    assert!(files(&e).is_empty(), "a stray file: {:?}", files(&e));
    assert_eq!(catalog(&e).editable_previews_size().unwrap(), 0);
    p.failed[0].1.clone()
}

#[test]
fn a_photo_removed_replaced_or_moved_meanwhile_is_refused_leaving_no_file() {
    let why = changed_mid_build("slow-removed.ARW", |e, _| {
        let folders: Vec<_> = catalog(e).folders().unwrap().iter().map(|f| f.id).collect();
        let session = &mut e.library.as_mut().unwrap().session;
        session
            .catalog_mut_for_tests()
            .remove_folders(&folders)
            .unwrap();
    });
    assert!(why.contains("no longer in the catalog"), "{why}");
    let why = changed_mid_build("slow-replaced.ARW", |e, id| {
        std::fs::write(path_of(e, id), b"another photo in its place").unwrap();
    });
    assert!(why.contains("The photo's file changed"), "{why}");
    // Moved: the catalog finds the photo in a copy of its folder; the file read
    // is still where it was, unchanged, but is no longer the photo's.
    let why = changed_mid_build("slow-moved.ARW", |e, id| {
        let from = path_of(e, id).parent().unwrap().to_path_buf();
        let to = from.with_file_name("moved");
        std::fs::create_dir(&to).unwrap();
        for f in std::fs::read_dir(&from).unwrap().flatten() {
            std::fs::copy(f.path(), to.join(f.file_name())).unwrap();
        }
        let folder = catalog(e).folders().unwrap()[0].id;
        let session = &mut e.library.as_mut().unwrap().session;
        session
            .catalog_mut_for_tests()
            .relink_folder(folder, &to)
            .unwrap();
    });
    assert!(why.contains("The photo changed"), "{why}");
}

#[test]
fn cancel_stops_the_build_under_way_and_drops_the_rest() {
    let (_dir, mut e, ids) = editor(&["slow-cancel.ARW", "z-after.ARW"]);
    let (slow, after) = (
        id_named(&e, &ids, "slow-cancel.ARW"),
        id_named(&e, &ids, "z-after.ARW"),
    );
    e.build_editable_previews(&[slow, after]).unwrap();
    wait_started(&path_of(&e, slow));
    assert!(e.editable_builds.running());
    e.editable_builds.cancel();
    settle(&mut e);
    assert!(!e.editable_builds.running(), "busy cleared");
    assert_eq!(opens_of(&path_of(&e, after)), 0);
    assert!(recorded(&e, slow).is_none() && files(&e).is_empty());
    assert!(progress(&e).failed.is_empty());
}

#[test]
fn a_failure_or_a_panic_fails_one_photo_and_the_next_still_builds() {
    let (_dir, mut e, ids) = editor(&["fail.ARW", "good.ARW", "panic.ARW"]);
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    let p = progress(&e);
    let mut failed: Vec<&str> = p.failed.iter().map(|(n, _)| n.as_str()).collect();
    failed.sort();
    assert_eq!(failed, ["fail.ARW", "panic.ARW"]);
    assert!(!e.editable_builds.running());
    assert!(recorded(&e, id_named(&e, &ids, "good.ARW")).is_some());
    assert_eq!(files(&e).len(), 1);
    // The worker carries on after the panic.
    e.build_editable_previews(&[id_named(&e, &ids, "fail.ARW")])
        .unwrap();
    settle(&mut e);
    assert_eq!(progress(&e).failed.len(), 1);
}

/// The worker gone (its queue closed under it, as a thread that died would leave
/// it): busy clears and the editor says so.
#[test]
fn a_worker_that_is_gone_clears_busy() {
    let (_dir, mut e, ids) = editor(&["slow-lost.ARW"]);
    e.build_editable_previews(&ids).unwrap();
    wait_started(&path_of(&e, ids[0]));
    e.editable_builds.builder.as_ref().unwrap().shared.close();
    // The worker ends once its build sees the cancel; until then nothing is lost.
    let until = Instant::now() + Duration::from_secs(10);
    while e.editable_builds.builder.is_some() {
        assert!(Instant::now() < until, "the worker's loss was never seen");
        e.poll_editable_builds();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!e.editable_builds.running());
    assert!(
        e.messages.line().contains("stopped being built"),
        "{}",
        e.messages.line()
    );
    assert!(files(&e).is_empty());
}

#[test]
fn closing_does_not_wait_and_quitting_ends_in_time() {
    let (_dir, mut e, ids) = editor(&["slow-exit.ARW"]);
    e.build_editable_previews(&ids).unwrap();
    wait_started(&path_of(&e, ids[0]));
    assert!(e.closing_waits_for().is_none());
    assert!(e.work_slows_previews() && e.work_pauses_checksums());
    let waited = e.exit_within(Duration::from_secs(3));
    assert_eq!(waited.detached, 0);
    assert!(recorded(&e, ids[0]).is_none() && files(&e).is_empty());
}

#[test]
fn discarding_removes_the_selection_s_and_stops_their_builds() {
    let (_dir, mut e, ids) = editor(&["a.ARW", "b.ARW", "slow-c.ARW"]);
    let [a, b, c] = [
        id_named(&e, &ids, "a.ARW"),
        id_named(&e, &ids, "b.ARW"),
        id_named(&e, &ids, "slow-c.ARW"),
    ];
    let copy = copy_of(&mut e, a);
    e.build_editable_previews(&[a, b]).unwrap();
    settle(&mut e);
    let file_a = recorded(&e, a).unwrap().path;
    // A copy discards its master's.
    let done = e.discard_editable_previews(Some(&[copy])).unwrap();
    assert_eq!(
        done,
        Discarded {
            discarded: 1,
            stopped: 0
        }
    );
    assert!(recorded(&e, a).is_none() && !file_a.exists());
    assert!(recorded(&e, b).is_some());
    // Under way and waiting: both stop, nothing recorded.
    e.build_editable_previews(&[c, a]).unwrap();
    wait_started(&path_of(&e, c));
    let done = e.discard_editable_previews(Some(&[a, c])).unwrap();
    assert_eq!(done.stopped, 2);
    settle(&mut e);
    assert_eq!(
        opens_of(&path_of(&e, a)),
        1,
        "the waiting build never started"
    );
    assert!(recorded(&e, c).is_none() && recorded(&e, a).is_none());
    assert_eq!(files(&e).len(), 1, "b's alone");
    // Discard All.
    let done = e.discard_editable_previews(None).unwrap();
    assert_eq!(done.discarded, 1);
    assert!(files(&e).is_empty());
    assert_eq!(catalog(&e).editable_previews_size().unwrap(), 0);
}

/// A discard whose catalog write ran while a build was recording: the build sees
/// it after its record, and discards what it recorded.
#[test]
fn a_record_that_raced_a_discard_is_discarded_again() {
    let (_dir, e, ids) = editor(&["a.ARW"]);
    let mut catalog = Catalog::open(catalog(&e).location().clone()).unwrap();
    let item = Item {
        catalog: catalog.location().clone(),
        photo: ids[0],
        name: "a.ARW".into(),
        path: path_of(&e, ids[0]),
        demosaic: Demosaic::default(),
        precision: editable_preview::Precision::Lossless,
        decode_cache: Default::default(),
        discards: 0,
    };
    // Not yet discarded when recording, discarded once recorded.
    let asked = AtomicUsize::new(0);
    let discarded = || asked.fetch_add(1, Ordering::Relaxed) > 0;
    let outcome = build_one(&item, &mut catalog, &discarded, STEPS, Cancel::NEVER);
    assert_eq!(outcome, Outcome::Cancelled);
    assert!(catalog.editable_preview(ids[0]).unwrap().is_none());
    assert!(files(&e).is_empty());
}

#[test]
fn only_a_photo_without_colour_data_keeps_a_camera_look() {
    let (_dir, mut e, ids) = editor(&["absent-look.ARW", "absent-refused.ARW", "usable.ARW"]);
    let [look, refused, usable] = [
        id_named(&e, &ids, "absent-look.ARW"),
        id_named(&e, &ids, "absent-refused.ARW"),
        id_named(&e, &ids, "usable.ARW"),
    ];
    add(&REFUSE_FIT, &path_of(&e, refused));
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    let read = |id| editable_preview::read(&recorded(&e, id).unwrap().path, Cancel::NEVER).unwrap();
    let stored = read(look);
    let expected = match test_fit(
        &path_of(&e, look),
        &stored.image().metadata,
        &[],
        Cancel::NEVER,
    ) {
        DefaultFit::Fitted(p) => *p,
        other => panic!("{other:?}"),
    };
    assert_eq!(stored.camera_look(), Some(&expected));
    assert!(read(usable).camera_look().is_none());
    // A refused fit fails nothing: built without it, and the report says so.
    assert!(read(refused).camera_look().is_none());
    let p = progress(&e);
    assert!(p.failed.is_empty(), "{p:?}");
    assert_eq!(p.without_look.len(), 1);
    assert!(p.without_look[0].1.contains("No embedded preview"), "{p:?}");
    let (line, reasons) = report(&p).unwrap();
    assert_eq!(line, "1 built without its Camera Look");
    assert!(reasons.starts_with("absent-refused.ARW: "), "{reasons}");
}

#[test]
fn preferences_shows_what_they_take_and_discards_them_all() {
    let (_dir, mut e, ids) = editor(&["a.ARW", "b.ARW"]);
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    let total: u64 = ids
        .iter()
        .map(|id| {
            std::fs::metadata(recorded(&e, *id).unwrap().path)
                .unwrap()
                .len()
        })
        .sum();
    e.open_preferences(crate::app::preferences::Tab::Catalog);
    assert_eq!(e.shown_editable_previews_size(), Some(total));
    e.discard_editable_previews(None).unwrap();
    assert_eq!(e.shown_editable_previews_size(), Some(0));
    assert!(files(&e).is_empty());
}

/// A stale editable preview is discarded before it is built again, so a rebuild
/// that fails still changes what Preferences shows.
#[test]
fn preferences_shows_a_stale_preview_gone_when_its_rebuild_fails() {
    let (_dir, mut e, ids) = editor(&["a.ARW"]);
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    e.open_preferences(crate::app::preferences::Tab::Catalog);
    assert!(
        e.shown_editable_previews_size()
            .is_some_and(|size| size > 0)
    );
    let path = path_of(&e, ids[0]);
    std::fs::write(&path, "the original, changed since").unwrap();
    add(&FAIL, &path);
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    assert_eq!(progress(&e).failed.len(), 1);
    assert!(recorded(&e, ids[0]).is_none() && files(&e).is_empty());
    assert_eq!(e.shown_editable_previews_size(), Some(0));
}

#[test]
fn automation_builds_and_discards_the_selection() {
    use crate::app::shell::commands::{Command, Operation, Outcome as Reply};
    let (_dir, mut e, ids) = editor(&["slow-a.ARW", "slow-b.ARW", "c.JPG"]);
    let raws = ["slow-a.ARW", "slow-b.ARW"].map(|name| path_of(&e, id_named(&e, &ids, name)));
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[2]);
    let ctx = e.context.clone();
    let reply = e
        .execute_command(Command::new(Operation::BuildEditablePreviews), &ctx)
        .unwrap();
    assert!(matches!(
        reply,
        Reply::Previews {
            queued: 2,
            skipped: 1
        }
    ));
    // One builds at a time: while the first waits, neither is done.
    let until = Instant::now() + Duration::from_secs(10);
    while !STARTED.lock().unwrap().iter().any(|p| raws.contains(p)) {
        assert!(Instant::now() < until, "the build never started");
        std::thread::sleep(Duration::from_millis(2));
    }
    let state = serde_json::to_value(e.command_state()).unwrap();
    assert_eq!(
        state["building_editable_previews"],
        serde_json::json!({"done": 0, "total": 2})
    );
    for raw in &raws {
        release(raw);
    }
    settle(&mut e);
    let state = serde_json::to_value(e.command_state()).unwrap();
    assert!(state["building_editable_previews"].is_null());
    let discard = Operation::DiscardEditablePreviews {
        include_offline: false,
    };
    let reply = e.execute_command(Command::new(discard), &ctx).unwrap();
    assert!(matches!(
        reply,
        Reply::EditablePreviewsDiscarded {
            discarded: 2,
            stopped: 0,
            kept_offline: 0
        }
    ));
    assert!(files(&e).is_empty());
    let reply = serde_json::to_value(&reply).unwrap();
    assert_eq!(
        reply,
        serde_json::json!({"discarded": 2, "stopped": 0, "kept_offline": 0})
    );
}

/// Automation's discard keeps offline photos' editable previews, counting
/// masters, unless `include_offline`.
#[test]
fn automation_keeps_offline_photos_editable_previews_unless_asked() {
    use crate::app::shell::commands::{Command, Operation};
    let (_dir, mut e, ids) = editor(&["a.ARW", "b.ARW"]);
    let [a, b] = ["a.ARW", "b.ARW"].map(|name| id_named(&e, &ids, name));
    let copy = copy_of(&mut e, a);
    e.build_editable_previews(&[a, b]).unwrap();
    settle(&mut e);
    std::fs::remove_file(path_of(&e, a)).unwrap();
    let ctx = e.context.clone();
    let discard = |e: &mut Editor, include_offline| {
        let library = e.library.as_mut().unwrap();
        library.select(Some(copy));
        library.select_range_to(b);
        let operation = Operation::DiscardEditablePreviews { include_offline };
        let reply = e.execute_command(Command::new(operation), &ctx).unwrap();
        serde_json::to_value(&reply).unwrap()
    };
    // a (by its copy, and perhaps itself) offline and kept; b discarded.
    assert_eq!(
        discard(&mut e, false),
        serde_json::json!({"discarded": 1, "stopped": 0, "kept_offline": 1})
    );
    assert!(recorded(&e, a).is_some() && recorded(&e, b).is_none());
    assert!(
        e.messages.line().contains("kept those of 1 photo offline"),
        "{}",
        e.messages.line()
    );
    assert_eq!(
        discard(&mut e, true),
        serde_json::json!({"discarded": 1, "stopped": 0, "kept_offline": 0})
    );
    assert!(recorded(&e, a).is_none() && files(&e).is_empty());
}

/// The real steps, on the synthetic chart: decoded in full, reduced and recorded.
#[test]
fn a_real_raw_is_built_from_its_full_decode() {
    let (_dir, mut e, ids) = editor_with_catalog(&["chart.dng"]).unwrap();
    let chart = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-d65.dng");
    let path = path_of(&e, ids[0]);
    std::fs::copy(&chart, &path).unwrap();
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    let p = progress(&e);
    assert!(p.failed.is_empty() && p.without_look.is_empty(), "{p:?}");
    let row = recorded(&e, ids[0]).unwrap();
    let file = editable_preview::read(&row.path, Cancel::NEVER).unwrap();
    let raw = crate::photo::open(&path).unwrap();
    assert_eq!(
        file.header().original,
        [raw.metadata.width, raw.metadata.height]
    );
    assert_eq!(row.original_size, (raw.metadata.width, raw.metadata.height));
    assert!(file.image().fast);
}

/// The thumbnail menu's commands, as the grid hands them to the editor: the
/// selection when the photo clicked is in it, else that photo.
#[test]
fn the_menu_builds_and_discards_the_selection_or_the_photo_clicked() {
    use crate::app::library::cell::PhotoAction;
    let (_dir, mut e, ids) = editor(&["a.ARW", "b.ARW", "c.ARW"]);
    let choose = |e: &mut Editor, id: PhotoId, action: PhotoAction| {
        let ctx = e.context.clone();
        let library = e.library.as_mut().unwrap();
        let photo = library.photo(id).unwrap().clone();
        library.photo_action(&ctx, &photo, action, true);
        e.library_requests(&ctx);
    };
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[1]);
    choose(&mut e, ids[2], PhotoAction::BuildEditablePreviews);
    settle(&mut e);
    assert!(
        e.messages.line().contains("Building 1 editable preview"),
        "{}",
        e.messages.line()
    );
    assert_eq!(files(&e).len(), 1);
    choose(&mut e, ids[0], PhotoAction::BuildEditablePreviews);
    settle(&mut e);
    assert_eq!(files(&e).len(), 3);
    choose(&mut e, ids[1], PhotoAction::DiscardEditablePreviews);
    assert!(
        e.messages
            .line()
            .contains("Discarded the editable previews of 2 photos"),
        "{}",
        e.messages.line()
    );
    assert!(recorded(&e, ids[2]).is_some());
    assert_eq!(files(&e).len(), 1);
}

/// Draws the Discard Editable Previews confirmation, with `events` (a window
/// shown first is only sized, and paints from its second frame).
fn draw_offline_discard(
    e: &mut Editor,
    events: Vec<egui::Event>,
) -> Vec<egui::epaint::ClippedShape> {
    let ctx = e.context.clone();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200., 800.),
        )),
        events,
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| e.discard_editable_previews_window(ui.ctx()));
    out.textures_delta.clear();
    out.shapes
}
fn text_at(shapes: &[egui::epaint::ClippedShape], text: &str) -> Option<egui::Pos2> {
    shapes.iter().find_map(|s| match &s.shape {
        egui::Shape::Text(t) if t.galley.text() == text => Some(t.visual_bounding_rect().center()),
        _ => None,
    })
}
fn shown(shapes: &[egui::epaint::ClippedShape], part: &str) -> bool {
    shapes.iter().any(|s| match &s.shape {
        egui::Shape::Text(t) => t.galley.text().contains(part),
        _ => false,
    })
}
fn key(key: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

/// The thumbnail menu's Discard Editable Previews asks first when an offline
/// photo's would go, counting masters; Escape and Return cancel, Discard
/// discards. A selection whose photos are all online discards at once.
#[test]
fn discarding_an_offline_photo_s_from_the_menu_asks_first() {
    use crate::app::library::cell::PhotoAction;
    let (_dir, mut e, ids) = editor(&["a.ARW", "b.ARW", "c.ARW", "d.ARW"]);
    let [a, b, c, d] = ["a.ARW", "b.ARW", "c.ARW", "d.ARW"].map(|name| id_named(&e, &ids, name));
    let copy = copy_of(&mut e, a);
    e.build_editable_previews(&[a, b, c]).unwrap();
    settle(&mut e);
    let choose = |e: &mut Editor, id: PhotoId| {
        let ctx = e.context.clone();
        let library = e.library.as_mut().unwrap();
        let photo = library.photo(id).unwrap().clone();
        library.photo_action(&ctx, &photo, PhotoAction::DiscardEditablePreviews, true);
        e.library_requests(&ctx);
    };
    // Online: at once.
    choose(&mut e, c);
    assert!(e.modal.is_none());
    assert!(recorded(&e, c).is_none());
    std::fs::remove_file(path_of(&e, a)).unwrap();
    // Offline without one, d is not counted: its photo has none to lose.
    std::fs::remove_file(path_of(&e, d)).unwrap();
    e.discard_editable_previews_from_menu(&[d]);
    assert!(e.modal.is_none());
    e.discard_editable_previews_from_menu(&[copy, d]);
    draw_offline_discard(&mut e, vec![]);
    let shapes = draw_offline_discard(&mut e, vec![]);
    assert!(shown(&shapes, "This photo is offline."));
    draw_offline_discard(
        &mut e,
        vec![key(egui::Key::Escape, true), key(egui::Key::Escape, false)],
    );
    assert!(e.modal.is_none());
    // A virtual copy's is its master's, offline: asked, nothing discarded yet.
    choose(&mut e, copy);
    assert!(matches!(
        e.modal,
        Some(crate::app::Modal::DiscardEditablePreviews(_))
    ));
    draw_offline_discard(&mut e, vec![]);
    let shapes = draw_offline_discard(&mut e, vec![]);
    assert!(shown(&shapes, "This photo is offline."));
    draw_offline_discard(
        &mut e,
        vec![key(egui::Key::Escape, true), key(egui::Key::Escape, false)],
    );
    assert!(e.modal.is_none());
    assert!(recorded(&e, a).is_some());
    // The selection, a and b, from b's menu: 1 of 2 offline. Return is Cancel's.
    let library = e.library.as_mut().unwrap();
    library.select(Some(a));
    library.select_range_to(b);
    choose(&mut e, b);
    draw_offline_discard(&mut e, vec![]);
    let shapes = draw_offline_discard(&mut e, vec![]);
    assert!(shown(&shapes, "1 of these photos is offline."));
    draw_offline_discard(
        &mut e,
        vec![key(egui::Key::Enter, true), key(egui::Key::Enter, false)],
    );
    draw_offline_discard(&mut e, vec![]);
    assert!(e.modal.is_none());
    assert!(recorded(&e, a).is_some() && recorded(&e, b).is_some());
    choose(&mut e, b);
    draw_offline_discard(&mut e, vec![]);
    let shapes = draw_offline_discard(&mut e, vec![]);
    let at = text_at(&shapes, "Discard").expect("the Discard button");
    let click = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    draw_offline_discard(&mut e, vec![egui::Event::PointerMoved(at), click(true)]);
    draw_offline_discard(&mut e, vec![click(false)]);
    assert!(e.modal.is_none());
    assert!(recorded(&e, a).is_none() && recorded(&e, b).is_none());
    assert!(
        e.messages
            .line()
            .contains("Discarded the editable previews of 2 photos"),
        "{}",
        e.messages.line()
    );
    assert!(files(&e).is_empty());
}

/// Discard All Editable Previews… says how many are of offline photos.
#[test]
fn discard_all_says_how_many_are_of_offline_photos() {
    let (_dir, mut e, ids) = editor(&["a.ARW", "b.ARW"]);
    e.build_editable_previews(&ids).unwrap();
    settle(&mut e);
    e.open_preferences(crate::app::preferences::Tab::Catalog);
    e.ask_discard_all_editable_previews();
    assert_eq!(e.asked_discard_all_offline(), Some(0));
    std::fs::remove_file(path_of(&e, ids[0])).unwrap();
    e.open_preferences(crate::app::preferences::Tab::Catalog);
    assert_eq!(e.asked_discard_all_offline(), None, "opening forgets it");
    e.ask_discard_all_editable_previews();
    assert_eq!(e.asked_discard_all_offline(), Some(1));
}

fn codec(e: &Editor, id: PhotoId) -> editable_preview::Codec {
    editable_preview::read_header(&recorded(e, id).unwrap().path)
        .unwrap()
        .codec
}

/// Preferences › Catalog's precision is captured when a build is asked for: a
/// build under way or waiting keeps the precision it was asked at, unless asked
/// again for a photo still waiting, which replaces its request; the next one
/// takes the new choice, and an editable preview of an unchanged original isn't
/// built again for it.
#[test]
fn a_build_uses_the_precision_chosen_when_it_was_asked_for() {
    use editable_preview::{Codec, Precision};
    let names = ["slow-a.ARW", "b.ARW", "c.ARW", "d.ARW"];
    let (_dir, mut e, ids) = editor(&names);
    let [a, b, c, d] = names.map(|name| id_named(&e, &ids, name));
    assert_eq!(e.editable_builds.precision, Precision::Lossless);
    e.editable_builds.precision = Precision::Smaller;
    e.build_editable_previews(&[a, b, d]).unwrap();
    wait_started(&path_of(&e, a));
    // Changed while `a` builds and `b` and `d` wait: neither `a` nor `b` changes;
    // `d`, asked for again while waiting, is asked at the new choice.
    e.editable_builds.precision = Precision::Lossless;
    e.build_editable_previews(&[d]).unwrap();
    release(&path_of(&e, a));
    settle(&mut e);
    assert_eq!(codec(&e, a), Codec::DitheredJxl12);
    assert_eq!(codec(&e, b), Codec::DitheredJxl12);
    assert_eq!(codec(&e, d), Codec::LosslessJxl16);
    assert_eq!(opens_of(&path_of(&e, d)), 1);
    // And it reads, at the precision it was written at.
    editable_preview::read(&recorded(&e, a).unwrap().path, Cancel::NEVER).unwrap();
    // The next build takes the new choice; the up-to-date ones are skipped.
    e.build_editable_previews(&[a, b, c]).unwrap();
    settle(&mut e);
    assert_eq!(codec(&e, c), Codec::LosslessJxl16);
    assert_eq!(
        (opens_of(&path_of(&e, a)), opens_of(&path_of(&e, b))),
        (1, 1)
    );
    assert_eq!(codec(&e, a), Codec::DitheredJxl12);
    // Discarded and built again, it is at the new choice.
    e.discard_editable_previews(Some(&[b])).unwrap();
    e.build_editable_previews(&[b]).unwrap();
    settle(&mut e);
    assert_eq!(codec(&e, b), Codec::LosslessJxl16);
}

/// Builds go to every core but one with the Library in front and nothing else
/// rendering. Unlike edited previews, they don't count themselves as such work.
#[test]
fn builds_take_the_computer_only_with_the_library_in_front_and_nothing_else_rendering() {
    use crate::app::testing::{FocusedButton, panel_frame};
    let (_dir, mut e, ids) = editor(&["slow-rule.ARW"]);
    e.module = crate::app::Module::Library;
    panel_frame(&mut e, Vec::new(), FocusedButton::No);
    assert_eq!(e.editable_builds.pace, Pace::Full);
    e.build_editable_previews(&ids).unwrap();
    wait_started(&path_of(&e, ids[0]));
    // Running, they send edited previews to the background but stay at full pace.
    assert_eq!(e.preview_pace(), Pace::Background);
    assert_eq!(e.editable_build_pace(), Pace::Full);
    e.module = crate::app::Module::Develop;
    assert_eq!(e.editable_build_pace(), Pace::Background);
    e.module = crate::app::Module::Library;
    e.automation.hold_running_output();
    assert_eq!(e.editable_build_pace(), Pace::Background);
    panel_frame(&mut e, Vec::new(), FocusedButton::No);
    assert_eq!(e.editable_builds.pace, Pace::Background);
    e.automation.cancel_outputs();
    assert_eq!(e.editable_build_pace(), Pace::Full);
    e.editable_builds.cancel();
    settle(&mut e);
}

/// A photo under way at full pace when the pace drops stops and is built again
/// on the background pool, counted once, failing nothing.
#[test]
fn a_photo_at_full_pace_gives_way_and_is_built_again_in_the_background() {
    let (_dir, mut e, ids) = editor(&["slow-way.ARW", "z-next.ARW"]);
    let (slow, next) = (
        id_named(&e, &ids, "slow-way.ARW"),
        id_named(&e, &ids, "z-next.ARW"),
    );
    let path = path_of(&e, slow);
    e.editable_builds.set_pace(Pace::Full);
    e.build_editable_previews(&[slow, next]).unwrap();
    wait_started(&path);
    assert_eq!(opened_at_full(&path), [true]);
    e.editable_builds.set_pace(Pace::Background);
    wait_opens(&path, 2);
    assert_eq!(opened_at_full(&path), [true, false]);
    assert!(e.editable_builds.running());
    assert_eq!(
        progress(&e).done,
        0,
        "the photo that gave way isn't counted done"
    );
    release(&path);
    settle(&mut e);
    assert!(recorded(&e, slow).is_some() && recorded(&e, next).is_some());
    assert_eq!(files(&e).len(), 2);
    assert_eq!(opened_at_full(&path_of(&e, next)), [false]);
    let p = progress(&e);
    assert_eq!((p.done, p.total), (2, 2));
    assert!(p.failed.is_empty());
}

/// Giving way never brings back a photo the queue was told to drop: one
/// discarded, or builds cancelled, while it was under way.
#[test]
fn a_photo_that_gave_way_after_a_discard_or_a_cancel_is_not_built_again() {
    let (_dir, mut e, ids) = editor(&["slow-gone.ARW"]);
    let path = path_of(&e, ids[0]);
    e.editable_builds.set_pace(Pace::Full);
    e.build_editable_previews(&ids).unwrap();
    wait_started(&path);
    e.discard_editable_previews(Some(&ids)).unwrap();
    e.editable_builds.set_pace(Pace::Background);
    settle(&mut e);
    assert_eq!(opens_of(&path), 1);
    assert!(recorded(&e, ids[0]).is_none() && files(&e).is_empty());

    let (_dir, mut e, ids) = editor(&["slow-stop.ARW"]);
    let path = path_of(&e, ids[0]);
    e.editable_builds.set_pace(Pace::Full);
    e.build_editable_previews(&ids).unwrap();
    wait_started(&path);
    e.editable_builds.set_pace(Pace::Background);
    e.editable_builds.cancel();
    settle(&mut e);
    assert!(!e.editable_builds.running());
    // At most the one restart that may begin before the cancel arrives, which
    // the cancel then stops.
    assert!(opens_of(&path) <= 2);
    assert!(recorded(&e, ids[0]).is_none() && files(&e).is_empty());
    assert!(progress(&e).failed.is_empty());
}

/// Stop wins over giving way, whichever comes first while the photo is still
/// running: the pace dropping after Stop doesn't bring the photo back, and nor
/// does one discarded after it gave way.
#[test]
fn a_photo_stopped_or_discarded_around_giving_way_is_not_built_again() {
    let (_dir, mut e, ids) = editor(&["stubborn-stop.ARW"]);
    let path = path_of(&e, ids[0]);
    e.editable_builds.set_pace(Pace::Full);
    e.build_editable_previews(&ids).unwrap();
    wait_started(&path);
    e.editable_builds.cancel();
    e.editable_builds.set_pace(Pace::Background);
    release(&path);
    settle(&mut e);
    assert_eq!(opens_of(&path), 1, "Stop was undone by the pace dropping");
    assert!(recorded(&e, ids[0]).is_none() && files(&e).is_empty());

    let (_dir, mut e, ids) = editor(&["stubborn-gone.ARW"]);
    let path = path_of(&e, ids[0]);
    e.editable_builds.set_pace(Pace::Full);
    e.build_editable_previews(&ids).unwrap();
    wait_started(&path);
    e.editable_builds.set_pace(Pace::Background);
    e.discard_editable_previews(Some(&ids)).unwrap();
    release(&path);
    settle(&mut e);
    assert_eq!(opens_of(&path), 1, "a discarded photo was built again");
    assert!(recorded(&e, ids[0]).is_none() && files(&e).is_empty());
}
