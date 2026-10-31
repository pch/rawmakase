use super::*;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

fn editor() -> Editor {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, Default::default(), None);
    // This fixture supplies its own defaults; ignore the startup preset scan.
    e.presets.next_scan();
    e
}

fn x_t20() -> Metadata {
    Metadata {
        make: "Fujifilm".into(),
        model: "X-T20".into(),
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    }
}

/// The catalogue's profile for the X-T20 (ART's public-domain file).
fn entry() -> &'static Entry {
    catalogue()
        .get("art/fujifilm-x-t20")
        .expect("in the catalogue")
}

/// A synthetic DCP for the catalogue X-T20 profile's camera, named `name`, with
/// the shipped profile's notice: no third-party bytes. `gain` sets its matrix, so
/// two profiles can share a name and camera and still differ.
fn dcp(name: &str, gain: i32) -> Vec<u8> {
    let text = |s: &str| format!("{s}\0").into_bytes();
    let entries = [
        (50708u16, 2u16, text(&entry().profile.camera)),
        (50936, 2, text(name)),
        (50942, 2, text(&entry().profile.notice)),
        (
            50964,
            10,
            (0..9)
                .flat_map(|i| [if i % 4 == 0 { gain } else { 0 }, 1])
                .flat_map(i32::to_le_bytes)
                .collect(),
        ),
    ];
    let mut at = 8 + 2 + entries.len() * 12 + 4;
    let mut bytes = b"IIRC".to_vec();
    bytes.extend(8u32.to_le_bytes());
    bytes.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, data) in &entries {
        bytes.extend(tag.to_le_bytes());
        bytes.extend(kind.to_le_bytes());
        bytes.extend((if *kind == 10 { 9 } else { data.len() as u32 }).to_le_bytes());
        bytes.extend((at as u32).to_le_bytes());
        at += data.len();
    }
    bytes.extend(0u32.to_le_bytes());
    for (_, _, data) in entries {
        bytes.extend(data);
    }
    bytes
}

/// A catalogue entry pinning `bytes`, from `source` (`art` or `rawtherapee`), for
/// the X-T20: what a reviewed profile is, without shipping one in the tests.
pub(in crate::app) fn synthetic(source: &str, name: &str, bytes: &[u8]) -> &'static Entry {
    use sha2::Digest;
    let sha256: String = sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let from = catalogue()
        .entries()
        .iter()
        .find(|e| e.source.id == source)
        .expect("a shipped source");
    let file = format!("{name}.dcp");
    Box::leak(Box::new(Entry {
        profile: crate::camera_profiles::catalogue::Profile {
            id: format!("{source}/synthetic-{}", &sha256[..12]),
            source: source.into(),
            filename: file.clone(),
            file,
            camera: entry().profile.camera.clone(),
            name: name.into(),
            notice: entry().profile.notice.clone(),
            terms: entry().profile.terms,
            size: bytes.len() as u64,
            sha256,
        },
        source: from.source.clone(),
    }))
}

/// The synthetic profile a download in these tests installs.
fn reviewed() -> (&'static Entry, Vec<u8>) {
    let bytes = reviewed_bytes();
    (synthetic("art", "Synthetic X-T20", &bytes), bytes)
}

fn reviewed_bytes() -> Vec<u8> {
    dcp("Synthetic X-T20", 1)
}

/// Writes `bytes` to `dir/name`, as an installed file.
fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Opens a photo of `m` as the loader does: a new load of a real file in `dir`,
/// identified as it was opened, its profiles listed and its defaults resolved.
fn open(e: &mut Editor, m: Metadata, dir: &Path) {
    let path = dir.join(format!("photo-{}.RAF", e.load.id() + 1));
    std::fs::write(&path, format!("a RAW from {path:?}")).unwrap();
    e.document.reset(None);
    e.load.start();
    e.document.file = crate::storage::Identity::read(&path).ok();
    e.document.path = Some(path);
    e.document.profiles = crate::camera_profiles::listed_from(&[], &m).profiles;
    e.document.metadata = Some(m);
    e.refresh_photo_defaults();
}

/// A transport that never delivers: it waits until its job is cancelled (the
/// editor's drop cancels it too), so nothing is ever installed.
pub(super) fn waits(_: &Entry, cancel: &AtomicBool, _: &AtomicUsize) -> Result<Vec<u8>> {
    let until = Instant::now() + Duration::from_secs(10);
    while !cancel.load(Ordering::Relaxed) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    anyhow::bail!("offline")
}

/// Starts a download of `entry` for the open photo, as the offer does; returns
/// its job.
fn ask(e: &mut Editor, entry: &'static Entry) -> u64 {
    let ctx = e.context.clone();
    e.download_profile_using(entry, &ctx, waits);
    assert!(e.profile_download.task.is_running());
    e.profile_download.task.id()
}

fn steps(e: &Editor) -> usize {
    e.document.edit.history().steps().0.len()
}

#[test]
fn a_finished_download_applies_to_the_photo_it_was_asked_for_as_one_history_step() {
    for edited in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (reviewed, bytes) = reviewed();
        let mut e = editor();
        open(&mut e, x_t20(), dir.path());
        if edited {
            e.change_edit(None, |r| r.exposure = 1.25);
        }
        let before = steps(&e);
        let job = ask(&mut e, reviewed);
        let ctx = e.context.clone();
        let path = write(dir.path(), "installed.dcp", &bytes);
        e.profile_downloaded(job, Ok(path.clone()), &ctx);
        assert!(!e.profile_download.task.is_running());
        let r = e.document.edit.recipe();
        assert_eq!(r.profile.as_ref().unwrap().name, reviewed.profile.name);
        assert_eq!(r.profile_amount, 1.);
        assert_eq!(r.exposure, if edited { 1.25 } else { 0. });
        assert_eq!(steps(&e), before + 1);
        assert_eq!(
            e.document.edit.history().steps().0.last().unwrap().name,
            "Profile"
        );
        assert!(e.document.edit.save_state().needs_save());
        // The duplicate or late completion of the same job changes nothing more.
        e.profile_downloaded(job, Ok(path.clone()), &ctx);
        assert_eq!(steps(&e), before + 1);
        // It is in use now: the offer no longer shows unasked.
        e.assume_catalogue_profiles(vec![(reviewed, Status::Installed(path))]);
        assert!(e.profile_offer(false).is_none());
    }
}

#[test]
fn a_download_finishing_after_the_user_moved_on_only_installs() {
    type MovesOn = fn(&mut Editor, &Path);
    let cases: [(&str, MovesOn); 7] = [
        ("opened another photo", |e, dir| open(e, x_t20(), dir)),
        ("chose another profile", |e, _| {
            e.change_edit(None, |r| r.profile = None);
        }),
        ("asked for a camera-look fit", |e, _| {
            e.document.camera_look.start();
        }),
        ("replaced the photo with another camera's", |e, dir| {
            open(
                e,
                Metadata {
                    make: "Nikon".into(),
                    model: "Z 8".into(),
                    ..x_t20()
                },
                dir,
            )
        }),
        ("closed the photo", |e, _| {
            e.document.reset(None);
            e.load.start();
        }),
        // Outside RAWmakase, with no new load: another photo of the same camera
        // where the one asked for was.
        ("replaced the photo's file", |e, _| {
            let path = e.document.path.clone().unwrap();
            std::fs::write(&path, "another X-T20 RAW, written over the first").unwrap();
        }),
        ("deleted the photo's file", |e, _| {
            std::fs::remove_file(e.document.path.as_ref().unwrap()).unwrap();
        }),
    ];
    for (what, moves_on) in cases {
        let dir = tempfile::tempdir().unwrap();
        let (reviewed, bytes) = reviewed();
        let mut e = editor();
        open(&mut e, x_t20(), dir.path());
        e.change_edit(None, |r| r.exposure = 0.5);
        let job = ask(&mut e, reviewed);
        moves_on(&mut e, dir.path());
        let recipe = e.document.edit.recipe().clone();
        let before = steps(&e);
        let saving = e.document.edit.save_state().needs_save();
        let ctx = e.context.clone();
        let path = write(dir.path(), "installed.dcp", &bytes);
        e.profile_downloaded(job, Ok(path), &ctx);
        assert!(!e.profile_download.task.is_running(), "{what}");
        assert_eq!(*e.document.edit.recipe(), recipe, "{what}");
        assert_eq!(steps(&e), before, "{what}");
        assert_eq!(e.document.edit.save_state().needs_save(), saving, "{what}");
        assert!(e.messages.line().contains("installed"), "{what}");
    }
}

/// A file already replaced when the download is asked for, or one that can't be
/// identified: the download only installs, whatever happens next.
#[test]
fn a_download_asked_for_a_file_no_longer_as_opened_only_installs() {
    type Before = fn(&mut Editor);
    let cases: [(&str, Before); 2] = [
        ("replaced before it was asked for", |e| {
            let path = e.document.path.clone().unwrap();
            std::fs::write(&path, "another X-T20 RAW, written over the first").unwrap();
        }),
        ("not identified when it was opened", |e| {
            e.document.file = None
        }),
    ];
    for (what, before) in cases {
        let dir = tempfile::tempdir().unwrap();
        let (reviewed, bytes) = reviewed();
        let mut e = editor();
        open(&mut e, x_t20(), dir.path());
        before(&mut e);
        let job = ask(&mut e, reviewed);
        let recipe = e.document.edit.recipe().clone();
        let ctx = e.context.clone();
        let path = write(dir.path(), "installed.dcp", &bytes);
        e.profile_downloaded(job, Ok(path), &ctx);
        assert!(!e.profile_download.task.is_running(), "{what}");
        assert_eq!(*e.document.edit.recipe(), recipe, "{what}");
        assert_eq!(steps(&e), 0, "{what}");
        assert!(!e.document.edit.save_state().needs_save(), "{what}");
        assert!(e.messages.line().contains("installed"), "{what}");
    }
}

/// The installed file is not what was verified when the completion arrives: a
/// valid profile for the same camera, with the same name, in its place. It is not
/// used or credited as the catalogue's.
#[test]
fn a_profile_file_replaced_after_installation_is_not_applied() {
    let dir = tempfile::tempdir().unwrap();
    let (reviewed, _) = reviewed();
    let mut e = editor();
    open(&mut e, x_t20(), dir.path());
    let job = ask(&mut e, reviewed);
    let recipe = e.document.edit.recipe().clone();
    let swapped = write(dir.path(), "installed.dcp", &dcp(&reviewed.profile.name, 2));
    assert!(crate::camera_profiles::load(&swapped, &x_t20()).is_ok());
    let ctx = e.context.clone();
    e.profile_downloaded(job, Ok(swapped), &ctx);
    assert!(!e.profile_download.task.is_running());
    assert_eq!(*e.document.edit.recipe(), recipe);
    assert_eq!(steps(&e), 0);
    assert!(!e.document.edit.save_state().needs_save());
    let line = e.messages.line();
    assert!(
        line.contains("changed") && !line.contains("applied, from"),
        "{line}"
    );
    // Not a failed download: no Retry.
    assert!(e.profile_download.failure.is_none());
}

/// Apply in the offer, clicked in Develop's own edit frame: one History step, as
/// choosing the profile in the menu is, that Undo and Redo take back and remake.
#[test]
fn applying_an_installed_profile_from_the_offer_is_one_undoable_history_step() {
    use crate::app::tests::{click, community_editor, controls_frame, labelled};
    let ctx = egui::Context::default();
    let mut e = community_editor(&ctx, "X-T20", false);
    let dir = tempfile::tempdir().unwrap();
    let (reviewed, bytes) = reviewed();
    let path = write(dir.path(), "installed.dcp", &bytes);
    e.assume_catalogue_profiles(vec![(reviewed, Status::Installed(path))]);
    let tree = controls_frame(&ctx, &mut e, vec![]);
    let apply = labelled(&tree, "Apply").expect("Apply").clone();
    let before = e.document.edit.recipe().clone();
    click(&ctx, &mut e, &apply);
    controls_frame(&ctx, &mut e, vec![]);
    let r = e.document.edit.recipe().clone();
    assert_eq!(r.profile.as_ref().unwrap().name, reviewed.profile.name);
    assert_eq!(r.profile_amount, 1.);
    let (history, applied) = e.document.edit.history().steps();
    let names: Vec<_> = history.iter().map(|s| s.name.as_str()).collect();
    assert_eq!((names, applied), (vec!["Profile"], 1));
    assert!(e.document.edit.save_state().needs_save());
    e.sync_undo();
    e.undo();
    assert_eq!(*e.document.edit.recipe(), before);
    assert_eq!(e.document.edit.history().steps().1, 0);
    e.redo();
    assert_eq!(*e.document.edit.recipe(), r);
    assert!(e.document.edit.save_state().needs_save());
}

/// RawTherapee and ART have profiles with one name for one camera, and different
/// contents. Using one leaves the other offered, to download or apply.
#[test]
fn a_profile_sharing_another_sources_name_and_camera_is_not_taken_for_it() {
    use crate::app::tests::{click, community_editor, controls_frame, labelled};
    let name = "Synthetic X-T20";
    let (a, b) = (dcp(name, 1), dcp(name, 2));
    let (first, second) = (
        synthetic("rawtherapee", name, &a),
        synthetic("art", name, &b),
    );
    let dir = tempfile::tempdir().unwrap();
    // Each line of text once: its label, not the text run inside.
    let count = |tree: &egui::accesskit::TreeUpdate, text: &str| {
        tree.nodes
            .iter()
            .filter(|(_, n)| n.role() == egui::accesskit::Role::Label && n.value() == Some(text))
            .count()
    };
    for installed in [false, true] {
        let ctx = egui::Context::default();
        let mut e = community_editor(&ctx, "X-T20", false);
        let m = e.document.metadata.clone().unwrap();
        let used = Arc::new(crate::camera_profiles::from_bytes(&a).unwrap());
        e.document.edit.setup_mut().profile = Some(used);
        let second_status = if installed {
            Status::Installed(write(dir.path(), "b.dcp", &b))
        } else {
            Status::Available
        };
        let measured = vec![
            (first, Status::Installed(write(dir.path(), "a.dcp", &a))),
            (second, second_status),
        ];
        e.assume_catalogue_profiles(measured.clone());
        // The photo uses one: the offer stays quiet until asked for again.
        assert!(e.profile_offer(false).is_none());
        e.profile_download.request(&m);
        // Request rechecks the real catalogue; supply this test's two-source
        // measurement again instead of this computer's installed profiles.
        e.assume_catalogue_profiles(measured);
        let offer = e.profile_offer(false).expect("asked for");
        let in_use: Vec<_> = offer.profiles.iter().map(|p| p.in_use).collect();
        assert_eq!(in_use, [true, false], "installed {installed}");
        let tree = controls_frame(&ctx, &mut e, vec![]);
        assert_eq!(count(&tree, "This photo uses it."), 1);
        if !installed {
            let download = tree
                .nodes
                .iter()
                .map(|(_, n)| n)
                .find(|n| n.label().is_some_and(|l| l.starts_with("Download & Apply")))
                .expect("the other source's download");
            assert!(!download.is_disabled());
            continue;
        }
        let apply = labelled(&tree, "Apply")
            .expect("the other source's Apply")
            .clone();
        click(&ctx, &mut e, &apply);
        let used = e.document.edit.recipe().profile.clone().unwrap();
        assert_eq!(*used, crate::camera_profiles::from_bytes(&b).unwrap());
        assert_eq!(steps(&e), 1);
        let tree = controls_frame(&ctx, &mut e, vec![]);
        assert_eq!(count(&tree, "This photo uses it."), 1);
        assert!(
            labelled(&tree, "Apply").is_some(),
            "the first source's, now"
        );
    }
}

#[test]
fn failures_cancellation_and_stale_completions_clear_busy_state_and_leave_a_retry() {
    let mut e = editor();
    let dir = tempfile::tempdir().unwrap();
    open(&mut e, x_t20(), dir.path());
    e.assume_catalogue_profiles_available();
    let ctx = e.context.clone();
    let old = ask(&mut e, entry());
    e.profile_offer_action(OfferAction::Cancel, &ctx);
    assert!(!e.profile_download.task.is_running());
    let current = ask(&mut e, entry());
    let downloading = |e: &Editor| e.running_work().iter().any(|w| w.key == "profile download");
    // Shown as running work wherever the user goes, not only in its offer.
    assert!(downloading(&e));
    let recipe = e.document.edit.recipe().clone();
    let (_, bytes) = reviewed();
    e.profile_downloaded(old, Ok(write(dir.path(), "installed.dcp", &bytes)), &ctx);
    e.profile_downloaded(old, Err("stale failure".into()), &ctx);
    assert!(e.profile_download.task.is_running());
    assert_eq!(*e.document.edit.recipe(), recipe);
    e.profile_downloaded(current, Err("connection lost".into()), &ctx);
    assert!(!e.profile_download.task.is_running());
    assert!(!downloading(&e));
    assert!(e.messages.line().contains("connection lost"));
    assert_eq!(*e.document.edit.recipe(), recipe);
    e.assume_catalogue_profiles_available();
    let offer = e.profile_offer(false).expect("the failed download's offer");
    let offered = offer
        .profiles
        .iter()
        .find(|p| p.entry.profile.id == entry().profile.id)
        .unwrap();
    assert_eq!(offered.failure.as_deref(), Some("connection lost"));
    assert!(offer.running.is_none());
    // The retry is an ordinary new download.
    ask(&mut e, entry());
    assert!(e.profile_download.failure.is_none());
}

#[test]
fn publication_cannot_be_cancelled_and_cancellation_cannot_be_published() {
    let mut e = editor();
    let dir = tempfile::tempdir().unwrap();
    open(&mut e, x_t20(), dir.path());
    ask(&mut e, entry());
    e.profile_download.publication.store(1, Ordering::SeqCst);
    assert!(!e.profile_download.cancellable());
    e.profile_download.cancel();
    assert!(e.profile_download.task.is_running());
    e.profile_download.publication.store(0, Ordering::SeqCst);
    e.profile_download.cancel();
    assert_eq!(e.profile_download.publication.load(Ordering::SeqCst), 2);
    assert!(!e.profile_download.task.is_running());
}

#[test]
fn a_panicking_profile_worker_reports_failure_and_clears_busy_state() {
    let mut e = editor();
    let dir = tempfile::tempdir().unwrap();
    open(&mut e, x_t20(), dir.path());
    let ctx = e.context.clone();
    // Holds the worker until other events are queued ahead of its completion, as
    // the startup preset scan's can be. Released however the test ends: a failed
    // assertion poisons it, and the worker then goes on to its panic all the same.
    static GATE: Mutex<()> = Mutex::new(());
    let held = GATE.lock().unwrap();
    e.download_profile_using(entry(), &ctx, |_, _, _| {
        drop(GATE.lock().unwrap_or_else(PoisonError::into_inner));
        panic!("lost profile worker")
    });
    let id = e.profile_download.task.id();
    assert!(e.profile_download.task.is_running());
    // An earlier job's failure, before and after the worker is let go: neither
    // finishes this one.
    let stale = || Event::ProfileDownloaded {
        id: id - 1,
        result: Err("an earlier job's failure".into()),
    };
    e.tx.send(stale()).unwrap();
    e.events(&ctx);
    assert!(e.profile_download.task.is_running());
    assert!(!e.messages.line().contains("an earlier job's failure"));
    e.tx.send(stale()).unwrap();
    drop(held);
    // Waits for this job's completion alone; whatever arrives first is passed on
    // to the editor with it, in order.
    let until = Instant::now() + Duration::from_secs(10);
    let mut received = Vec::new();
    loop {
        let event =
            e.rx.recv_timeout(until.saturating_duration_since(Instant::now()))
                .expect("the panicking worker reports its job");
        let done = match &event {
            Event::ProfileDownloaded { id: done, result } if *done == id => {
                let why = result.as_ref().expect_err("the worker panicked");
                assert!(why.contains("lost profile worker"), "{why}");
                true
            }
            _ => false,
        };
        received.push(event);
        if done {
            break;
        }
    }
    assert!(received.len() > 1, "the stale failure came first");
    for event in received {
        e.tx.send(event).unwrap();
    }
    e.events(&ctx);
    assert!(!e.profile_download.task.is_running());
    assert!(e.messages.line().contains("lost profile worker"));
}

#[test]
fn rendered_unmatched_or_unidentified_photos_never_start_a_download_or_show_an_offer() {
    let rendered = Metadata {
        source: crate::camera_data::Source::Rendered,
        ..x_t20()
    };
    let unmatched = Metadata {
        model: "X-T5".into(),
        ..x_t20()
    };
    for m in [None, Some(rendered), Some(unmatched)] {
        let mut e = editor();
        e.document.metadata = m;
        let ctx = e.context.clone();
        e.download_profile_using(entry(), &ctx, waits);
        assert!(!e.profile_download.task.is_running());
        assert!(!e.has_catalogue_profiles());
        assert!(e.profile_offer(false).is_none());
    }
}

#[test]
fn a_dismissed_offer_stays_away_for_its_camera_only_until_asked_for() {
    let mut e = editor();
    let ctx = e.context.clone();
    let dir = tempfile::tempdir().unwrap();
    open(&mut e, x_t20(), dir.path());
    assert!(e.profile_offer(false).is_some());
    e.profile_offer_action(OfferAction::Dismiss, &ctx);
    assert!(e.profile_offer(false).is_none());
    // Another photo of the same camera, under the decoder's other spelling.
    open(
        &mut e,
        Metadata {
            make: "FUJIFILM".into(),
            ..x_t20()
        },
        dir.path(),
    );
    assert!(e.profile_offer(false).is_none());
    // Another covered camera keeps its offer.
    let other = catalogue()
        .entries()
        .iter()
        .find(|e| !e.profile.camera.to_lowercase().contains("x-t20"))
        .unwrap();
    let (make, model) = other.profile.camera.split_once(' ').unwrap();
    open(
        &mut e,
        Metadata {
            make: make.into(),
            model: model.into(),
            ..x_t20()
        },
        dir.path(),
    );
    assert!(e.profile_offer(false).is_some());
    // Back to the X-T20: Profile › Show community profiles brings it back.
    open(&mut e, x_t20(), dir.path());
    assert!(e.has_catalogue_profiles());
    let m = e.document.metadata.clone().unwrap();
    e.profile_download.request(&m);
    assert!(e.profile_offer(false).is_some());
}

#[test]
fn adobe_profiles_take_precedence_unless_the_offer_is_asked_for() {
    let mut e = editor();
    let dir = tempfile::tempdir().unwrap();
    open(&mut e, x_t20(), dir.path());
    e.assume_catalogue_profiles_available();
    // Adobe's profiles for this camera can be imported from Lightroom's installation.
    assert!(e.profile_offer(true).is_none());
    // Or Adobe Standard is imported already.
    let mut standard = (*e.document.profiles[0]).clone();
    standard.name = "Adobe Standard".into();
    e.document.profiles.push(Arc::new(standard));
    assert!(e.profile_offer(false).is_none());
    let m = e.document.metadata.clone().unwrap();
    e.profile_download.request(&m);
    assert!(e.profile_offer(true).is_some());
}

#[test]
fn an_installed_profile_is_checked_again_before_it_applies_without_a_download() {
    let dir = tempfile::tempdir().unwrap();
    let (reviewed, _) = reviewed();
    let mut e = editor();
    open(&mut e, x_t20(), dir.path());
    let ctx = e.context.clone();
    // As measured: the exact file at this path. Then another valid profile with
    // its name and camera takes its place, so the check before applying refuses.
    let path = write(dir.path(), "installed.dcp", &reviewed_bytes());
    e.assume_catalogue_profiles(vec![(reviewed, Status::Installed(path))]);
    let offer = e.profile_offer(false).unwrap();
    assert_eq!(offer.profiles.len(), 1);
    write(dir.path(), "installed.dcp", &dcp(&reviewed.profile.name, 2));
    let recipe = e.document.edit.recipe().clone();
    crate::app::tests::in_edit_frame(&ctx, &mut e, |e| {
        e.profile_offer_action(OfferAction::Apply(&reviewed.profile.id), &ctx);
    });
    assert!(!e.profile_download.task.is_running(), "no download");
    assert_eq!(steps(&e), 0);
    assert_eq!(*e.document.edit.recipe(), recipe);
    assert!(e.messages.line().contains("no longer installed"));
}

#[test]
fn profile_transport_rejects_truncated_oversized_and_failed_responses() {
    let cancel = AtomicBool::new(false);
    let progress = AtomicUsize::new(0);
    assert_eq!(
        read_body(&b"abcd"[..], 4, &cancel, &progress).unwrap(),
        b"abcd"
    );
    assert_eq!(progress.load(Ordering::Relaxed), 4);
    assert!(read_body(&b"abc"[..], 4, &cancel, &progress).is_err());
    assert!(read_body(&b"abcde"[..], 4, &cancel, &progress).is_err());
    assert!(read_body(&b""[..], MAX_BYTES + 1, &cancel, &progress).is_err());
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("lost connection"))
        }
    }
    assert!(
        read_body(Broken, 4, &cancel, &progress)
            .unwrap_err()
            .to_string()
            .contains("interrupted")
    );
}

#[test]
fn profile_transport_cancels_before_and_during_a_response() {
    let cancel = AtomicBool::new(true);
    let progress = AtomicUsize::new(0);
    assert!(read_body(&b"abcd"[..], 4, &cancel, &progress).is_err());
    assert_eq!(progress.load(Ordering::Relaxed), 0);
    struct Cancels<'a>(&'a AtomicBool);
    impl Read for Cancels<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.store(true, Ordering::Relaxed);
            buf[0] = b'x';
            Ok(1)
        }
    }
    cancel.store(false, Ordering::Relaxed);
    assert!(read_body(Cancels(&cancel), 4, &cancel, &progress).is_err());
}

/// Fetches every catalogue profile from its pinned URL with the app's transport and
/// verifies it as a download is (size, digest, parser, camera, name and notice),
/// and that each source's license link exists. Installs nothing.
#[test]
#[ignore = "Downloads every catalogue profile over HTTPS (about 5.9 MB)"]
fn published_catalogue_files_match_their_pins() {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut licenses = std::collections::BTreeSet::new();
    for e in catalogue().entries() {
        licenses.insert(e.repository_license_url().replace("/blob/", "/raw/"));
        let (make, model) = e.profile.camera.split_once(' ').unwrap();
        let m = Metadata {
            make: make.into(),
            model: model.into(),
            ..Default::default()
        };
        let bytes = fetch(e, &AtomicBool::new(false), &AtomicUsize::new(0))
            .unwrap_or_else(|why| panic!("{}: {why:#}", e.profile.id));
        e.verify(&bytes, &m)
            .unwrap_or_else(|why| panic!("{}: {why:#}", e.profile.id));
    }
    for license in licenses {
        let response = agent.get(&license).call().expect(&license);
        assert_eq!(response.status(), 200, "{license}");
    }
}

/// Reopening the finder must recheck files changed outside RAWmakase, even
/// though the camera and the application's last library fingerprint stayed put.
#[test]
fn reopening_the_offer_rechecks_a_resolved_file_conflict() {
    const CHILD: &str = "RAWMAKASE_TEST_PROFILE_RECHECK";
    if std::env::var_os(CHILD).is_none() {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app::profile_download::tests::reopening_the_offer_rechecks_a_resolved_file_conflict",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("RAWMAKASE_DATA_DIR", dir.path())
            .env("XDG_DATA_HOME", dir.path().join("xdg"))
            .status()
            .unwrap();
        assert!(status.success(), "isolated profile conflict recovery");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut e = editor();
    open(&mut e, x_t20(), dir.path());
    let library = crate::storage::data_dir().join("camera-profiles");
    std::fs::create_dir_all(&library).unwrap();
    let conflict = library.join(&entry().profile.filename);
    std::fs::write(&conflict, b"a different profile already here").unwrap();
    let first = e.profile_offer(false).unwrap();
    assert_eq!(first.profiles[0].status, Status::Conflict);
    let fingerprint = e.profile_library.clone();
    std::fs::rename(&conflict, library.join("moved-aside.dcp")).unwrap();
    let m = e.document.metadata.clone().unwrap();
    e.profile_download.dismiss(&m);
    e.profile_download.request(&m);
    let refreshed = e.profile_offer(false).unwrap();
    assert_eq!(refreshed.profiles[0].status, Status::Available);
    assert_eq!(
        e.profile_library, fingerprint,
        "an explicit recheck suffices"
    );
    assert!(e.document.edit.history().steps().0.is_empty());
    assert!(!e.document.edit.save_state().needs_save());
}
