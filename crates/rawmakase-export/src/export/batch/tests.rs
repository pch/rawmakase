use super::*;
use crate::{
    catalog::{Catalog, HistoryUpdate, PhotoId},
    export_settings::{Destination, Format},
};
use rawmakase_cancel::Cancel;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// A photo of a batch at `source`, without an edit.
fn photo(source: &Path) -> BatchPhoto {
    BatchPhoto {
        source: source.to_path_buf(),
        name: source.file_name().unwrap().to_string_lossy().into(),
        edit: Edit::Catalog(Default::default()),
        values: Default::default(),
        naming: Default::default(),
        place: None,
        catalog: Some(Default::default()),
    }
}
/// Export settings into `folder`, as small TIFFs.
fn settings(folder: &Path) -> ExportSettings {
    ExportSettings {
        destination: Destination::Folder,
        folder: Some(folder.to_path_buf()),
        format: Format::Tiff,
        resize: true,
        long_edge: 96,
        ..Default::default()
    }
}
fn names(plan: &Plan) -> Vec<(String, Write)> {
    plan.entries
        .iter()
        .map(|e| {
            (
                e.target.file_name().unwrap().to_string_lossy().into(),
                e.write,
            )
        })
        .collect()
}
fn s(name: &str, write: Write) -> (String, Write) {
    (name.into(), write)
}

#[test]
fn names_never_collide_within_a_batch() -> Result<()> {
    let d = tempfile::tempdir()?;
    let out = d.path().join("out");
    std::fs::create_dir(&out)?;
    // Already in the folder: a suffixed name, and one in other case.
    std::fs::write(out.join("a-2.tif"), b"")?;
    std::fs::write(out.join("C.TIF"), b"")?;
    let photos = [
        photo(Path::new("/one/a.dng")),
        // The same name from another folder, and a virtual copy of the first.
        photo(Path::new("/two/a.dng")),
        photo(Path::new("/one/a.dng")),
        photo(Path::new("/one/b.dng")),
    ];
    let plan = plan(&photos, &settings(&out), &Default::default(), None).unwrap();
    assert_eq!(
        names(&plan),
        [
            s("a.tif", Write::Create),
            // a-2.tif is taken on disk.
            s("a-3.tif", Write::Create),
            s("a-4.tif", Write::Create),
            s("b.tif", Write::Create),
        ]
    );
    // Same Folder: each next to its own original, so no collision.
    let mut same = settings(&out);
    same.destination = Destination::SameFolder;
    let plan = super::plan(&photos[..2], &same, &Default::default(), None).unwrap();
    assert_eq!(
        plan.entries
            .iter()
            .map(|e| e.target.clone())
            .collect::<Vec<_>>(),
        [PathBuf::from("/one/a.tif"), PathBuf::from("/two/a.tif")]
    );

    // A name differing only in case is the same name, and the new one keeps the
    // photo's own spelling.
    let mut unique = settings(&out);
    unique.existing = Existing::Unique;
    let plan = super::plan(
        &[photo(Path::new("/one/c.dng"))],
        &unique,
        &Default::default(),
        None,
    )
    .unwrap();
    assert_eq!(names(&plan), [s("c-2.tif", Write::Create)]);
    // Overwrite replaces the file that is there, as it is spelled, and Ask names it
    // so: never a second file beside it in other case.
    let ask = settings(&out);
    assert_eq!(
        super::plan(
            &[photo(Path::new("/one/c.dng"))],
            &ask,
            &Default::default(),
            None
        )
        .unwrap_err(),
        Unplanned::Conflicts(vec![out.join("C.TIF")])
    );
    let plan = super::plan(
        &[photo(Path::new("/one/c.dng"))],
        &ask,
        &Default::default(),
        Some(Existing::Overwrite),
    )
    .unwrap();
    assert_eq!(names(&plan), [s("C.TIF", Write::Overwrite)]);
    Ok(())
}

#[test]
fn ask_returns_the_files_that_exist_and_the_answer_applies_to_all() -> Result<()> {
    let d = tempfile::tempdir()?;
    let out = d.path().join("out");
    std::fs::create_dir(&out)?;
    std::fs::write(out.join("a.tif"), b"")?;
    std::fs::write(out.join("b.tif"), b"")?;
    let photos = [
        photo(Path::new("/p/a.dng")),
        photo(Path::new("/p/b.dng")),
        photo(Path::new("/p/c.dng")),
        // A second a: never over the first one's file.
        photo(Path::new("/q/a.dng")),
    ];
    let ask = settings(&out);
    assert_eq!(ask.existing, Existing::Ask);
    assert_eq!(
        plan(&photos, &ask, &Default::default(), None).unwrap_err(),
        Unplanned::Conflicts(vec![out.join("a.tif"), out.join("b.tif")])
    );
    let overwrite = plan(
        &photos,
        &ask,
        &Default::default(),
        Some(Existing::Overwrite),
    )
    .unwrap();
    assert_eq!(
        names(&overwrite),
        [
            s("a.tif", Write::Overwrite),
            s("b.tif", Write::Overwrite),
            s("c.tif", Write::Create),
            s("a-2.tif", Write::Create),
        ]
    );
    assert_eq!(overwrite.late, Existing::Overwrite);
    let skip = plan(&photos, &ask, &Default::default(), Some(Existing::Skip)).unwrap();
    assert_eq!(
        names(&skip),
        [
            s("a.tif", Write::Skip),
            s("b.tif", Write::Skip),
            s("c.tif", Write::Create),
            s("a.tif", Write::Skip),
        ]
    );
    let unique = plan(&photos, &ask, &Default::default(), Some(Existing::Unique)).unwrap();
    assert_eq!(
        names(&unique),
        [
            s("a-2.tif", Write::Create),
            s("b-2.tif", Write::Create),
            s("c.tif", Write::Create),
            s("a-3.tif", Write::Create),
        ]
    );
    // Ask with nothing to ask about: a file appearing later is not replaced.
    let fresh = plan(&photos[2..3], &ask, &Default::default(), None).unwrap();
    assert_eq!(fresh.late, Existing::Skip);
    // No folder chosen.
    let mut none = ask;
    none.folder = None;
    assert_eq!(
        plan(&photos, &none, &Default::default(), None).unwrap_err(),
        Unplanned::NoFolder
    );
    Ok(())
}

/// A catalog of copies of the synthetic chart DNG, and a file that isn't a photo.
struct Fixture {
    dir: tempfile::TempDir,
    catalog: Catalog,
    photos: Vec<(PhotoId, PathBuf)>,
}
fn fixture(names: &[&str]) -> Result<Fixture> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    let chart =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng");
    for name in names {
        if name.ends_with(".dng") {
            std::fs::copy(&chart, folder.join(name))?;
        } else {
            std::fs::write(folder.join(name), b"not a photo")?;
        }
    }
    let mut catalog = Catalog::create(&dir.path().join("batch.rawmakase"))?;
    catalog.add_folder(&folder)?;
    let photos = catalog
        .photos()?
        .into_iter()
        .map(|p| (p.id, p.path))
        .collect();
    Ok(Fixture {
        dir,
        catalog,
        photos,
    })
}
impl Fixture {
    fn out(&self) -> PathBuf {
        self.dir.path().join("out")
    }
    /// The batch photos of every photo, as Export reads them.
    fn batch_photos(&self) -> Result<Vec<BatchPhoto>> {
        let ids: Vec<PhotoId> = self.photos.iter().map(|(id, _)| *id).collect();
        Ok(self
            .catalog
            .photo_records(&ids)?
            .into_iter()
            .zip(self.catalog.name_fields(&ids)?)
            .zip(&self.photos)
            .map(|((record, naming), (_, path))| {
                let name = path.file_name().unwrap().to_string_lossy().into();
                BatchPhoto::from_record(record, None, path.clone(), name, naming)
            })
            .collect())
    }
    fn batch(&self, photos: Vec<BatchPhoto>, settings: ExportSettings) -> Batch {
        Batch {
            plan: plan(
                &photos,
                &settings,
                &Default::default(),
                Some(Existing::Unique),
            )
            .unwrap(),
            photos,
            settings,
            defaults: Arc::new(DevelopDefaults::with_presets(Default::default(), |_| None)),
            watermark: None,
            watermarks: Default::default(),
            demosaic: Default::default(),
            cache: Default::default(),
            lenses: crate::photo::LensProfiles::none(),
            profiles: crate::camera_profiles::ProfileLibrary::for_tests(),
            fits: Default::default(),
            ai: crate::ai_denoise::AiService::refusing("no AI Denoise in tests"),
        }
    }
}
/// One photo at a time, as the tests that stop part-way through the first photo
/// expect.
const ONE: Lanes = Lanes {
    photos: 1,
    threads: 2,
    memory: u64::MAX,
};
fn run_all(batch: &Batch) -> Vec<Outcome> {
    run(batch, ONE, Cancel::NEVER, |_| {})
}
fn exported(outcome: &Outcome) -> &Path {
    match outcome {
        Outcome::Exported { path, .. } => path,
        other => panic!("not exported: {other:?}"),
    }
}
fn pixels(path: &Path) -> Vec<u16> {
    image::open(path).unwrap().into_rgb16().into_raw()
}
/// Files in `dir`, sorted.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn every_photo_is_exported_with_its_own_edit_or_said_why_not() -> Result<()> {
    let mut f = fixture(&["a.dng", "b.dng", "c.dng", "d.dng", "e.dng"])?;
    let [(a, a_path), (b, b_path), _, (d, d_path), (_, e_path)] = &f.photos[..] else {
        panic!("five photos");
    };
    let m = crate::photo::open(a_path, &crate::photo::LensProfiles::none())?.metadata;
    let (profiles, _) = crate::camera_profiles::ProfileLibrary::for_tests().installed(&m);
    let mut brighter = Recipe::with_profiles(&m, &profiles);
    brighter.exposure = 1.;
    let mut save = |id: PhotoId, path: &Path| {
        f.catalog.save_edit(
            id,
            path,
            &crate::storage::Identity::read(path)?,
            &brighter,
            &Default::default(),
            HistoryUpdate::Keep,
        )
    };
    save(*a, a_path)?;
    save(*d, d_path)?;
    f.catalog.db_for_tests().execute(
        "UPDATE photos SET lightroom_develop='s = { Exposure2012 = 0.5 }' WHERE id=?",
        [b.0],
    )?;
    let photos = f.batch_photos()?;
    // Its file changed since the edit was saved: protected, not exported unedited.
    let mut bytes = std::fs::read(d_path)?;
    bytes.extend_from_slice(b"changed");
    std::fs::write(d_path, bytes)?;
    // Offline since Export was pressed.
    std::fs::remove_file(e_path)?;
    let _ = b_path;
    let outcomes = run_all(&f.batch(photos, settings(&f.out())));
    let mean = |o: &Outcome| {
        let p = pixels(exported(o));
        p.iter().map(|&v| v as f64).sum::<f64>() / p.len() as f64
    };
    let (saved, lightroom, unedited) = (mean(&outcomes[0]), mean(&outcomes[1]), mean(&outcomes[2]));
    assert!(
        saved > lightroom && lightroom > unedited,
        "{saved} {lightroom} {unedited}"
    );
    match &outcomes[3] {
        Outcome::Failed(reason) => assert!(reason.contains("protected"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(&outcomes[4], Outcome::Failed(_)),
        "{:?}",
        outcomes[4]
    );
    assert_eq!(listing(&f.out()), ["a.tif", "b.tif", "c.tif"]);
    // Each written file's size, its long edge as the settings resize it.
    let Outcome::Exported { path, size, .. } = &outcomes[0] else {
        panic!("exported");
    };
    assert_eq!(size.0.max(size.1), 96);
    assert_eq!(*size, image::image_dimensions(path)?);
    Ok(())
}

/// Issue 546: exFAT on macOS refuses both an exclusive rename and a hard link.
/// Runs on the folder RAWMAKASE_TEST_EXFAT names, on such a volume; without it
/// there is nothing to test against. To make one on macOS:
/// `mkfile -n 64m x.img; hdiutil attach -imagekey diskimage-class=CRawDiskImage
/// -nomount x.img`, then `newfs_exfat /dev/rdiskN` and `mount -t exfat
/// /dev/diskN <folder>`.
#[test]
fn a_volume_without_exclusive_rename_takes_exports_without_replacing_files() -> Result<()> {
    let Some(volume) = std::env::var_os("RAWMAKASE_TEST_EXFAT") else {
        eprintln!("RAWMAKASE_TEST_EXFAT unset: skipped");
        return Ok(());
    };
    let f = fixture(&["a.dng", "b.dng"])?;
    let out = tempfile::tempdir_in(volume)?;
    std::fs::write(out.path().join("a.tif"), b"previous")?;
    let batch = f.batch(f.batch_photos()?, settings(out.path()));
    // Appears after planning, before the photo is written.
    std::fs::write(out.path().join("b.tif"), b"late")?;
    let outcomes = run_all(&batch);
    assert_eq!(exported(&outcomes[0]), out.path().join("a-2.tif"));
    assert_eq!(exported(&outcomes[1]), out.path().join("b-2.tif"));
    assert_eq!(std::fs::read(out.path().join("a.tif"))?, b"previous");
    assert_eq!(std::fs::read(out.path().join("b.tif"))?, b"late");
    assert!(!pixels(&out.path().join("a-2.tif")).is_empty());
    // Without the "._" files macOS keeps beside each file there.
    let files: Vec<_> = listing(out.path())
        .into_iter()
        .filter(|n| !n.starts_with("._"))
        .collect();
    assert_eq!(files, ["a-2.tif", "a.tif", "b-2.tif", "b.tif"]);
    Ok(())
}

#[test]
fn a_file_that_appears_during_the_export_gets_the_batchs_answer() -> Result<()> {
    let f = fixture(&["a.dng"])?;
    let photos = f.batch_photos()?;
    let late = |existing: Existing, answer: Option<Existing>| -> Result<(Outcome, Vec<String>)> {
        let out = f.out().join(format!("{existing:?}-{}", answer.is_some()));
        let mut settings = settings(&out);
        settings.existing = existing;
        let batch = Batch {
            plan: plan(&photos, &settings, &Default::default(), answer).unwrap(),
            ..f.batch(photos.clone(), settings)
        };
        assert_eq!(batch.plan.entries[0].write, Write::Create);
        // Appears after planning, before the photo is written.
        std::fs::create_dir_all(&out)?;
        std::fs::write(out.join("a.tif"), b"late")?;
        let outcome = run_all(&batch).remove(0);
        Ok((outcome, listing(&out)))
    };
    let (outcome, files) = late(Existing::Unique, None)?;
    assert_eq!(exported(&outcome), f.out().join("Unique-false/a-2.tif"));
    assert_eq!(files, ["a-2.tif", "a.tif"]);
    let (outcome, files) = late(Existing::Skip, None)?;
    assert!(matches!(outcome, Outcome::Skipped(_)), "{outcome:?}");
    assert_eq!(files, ["a.tif"]);
    // Overwrite WITHOUT WARNING covers it too, and says so.
    let (outcome, files) = late(Existing::Overwrite, None)?;
    match &outcome {
        Outcome::Exported { notes, .. } => assert!(notes[0].contains("replaced"), "{notes:?}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(files, ["a.tif"]);
    assert_ne!(
        std::fs::read(f.out().join("Overwrite-false/a.tif"))?,
        b"late"
    );
    // Ask had nothing to ask about: no answer, so nothing is replaced.
    let (outcome, files) = late(Existing::Ask, None)?;
    assert!(matches!(outcome, Outcome::Skipped(_)), "{outcome:?}");
    assert_eq!(files, ["a.tif"]);

    // The same name in other case, as an earlier batch may write it: the same
    // answers, on any volume, and an overwrite replaces it as it is spelled.
    let other_case = |existing: Existing| -> Result<(Outcome, Vec<String>)> {
        let out = f.out().join(format!("case-{existing:?}"));
        let mut settings = settings(&out);
        settings.existing = existing;
        let batch = Batch {
            plan: plan(&photos, &settings, &Default::default(), None).unwrap(),
            ..f.batch(photos.clone(), settings)
        };
        std::fs::create_dir_all(&out)?;
        std::fs::write(out.join("A.TIF"), b"late")?;
        let outcome = run_all(&batch).remove(0);
        Ok((outcome, listing(&out)))
    };
    let (outcome, files) = other_case(Existing::Unique)?;
    assert_eq!(exported(&outcome), f.out().join("case-Unique/a-2.tif"));
    assert_eq!(files, ["A.TIF", "a-2.tif"]);
    let (outcome, files) = other_case(Existing::Overwrite)?;
    assert_eq!(exported(&outcome), f.out().join("case-Overwrite/A.TIF"));
    assert_eq!(files, ["A.TIF"]);
    let (outcome, files) = other_case(Existing::Skip)?;
    assert!(matches!(outcome, Outcome::Skipped(_)), "{outcome:?}");
    assert_eq!(files, ["A.TIF"]);
    Ok(())
}

#[test]
fn cancel_before_a_commit_leaves_nothing_and_after_it_counts_as_exported() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng"])?;
    let photos = f.batch_photos()?;
    // Cancel at `at` of the first photo: 0.95 is staged and not yet committed, 1
    // is committed.
    let cancelled_at = |at: f32| -> Result<(Vec<Outcome>, Vec<String>)> {
        let out = f.out().join(format!("{at}"));
        let batch = f.batch(photos.clone(), settings(&out));
        let cancel = AtomicBool::new(false);
        let outcomes = run(&batch, ONE, Cancel::new(&cancel), |p| {
            if p.done == 0 && p.fraction >= at {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        Ok((outcomes, listing(&out)))
    };
    let (outcomes, files) = cancelled_at(0.95)?;
    assert_eq!(outcomes, [Outcome::Cancelled, Outcome::NotStarted]);
    // No export and no temporary file.
    assert!(files.is_empty(), "{files:?}");
    let (outcomes, files) = cancelled_at(1.)?;
    assert!(
        matches!(outcomes[0], Outcome::Exported { .. }),
        "{outcomes:?}"
    );
    assert_eq!(outcomes[1], Outcome::NotStarted);
    assert_eq!(files, ["a.tif"]);
    // Before the photo renders.
    let (outcomes, files) = cancelled_at(0.)?;
    assert_eq!(outcomes, [Outcome::Cancelled, Outcome::NotStarted]);
    assert!(files.is_empty(), "{files:?}");
    Ok(())
}

#[test]
fn a_batch_exports_the_edits_as_they_were_when_export_was_pressed() -> Result<()> {
    let mut f = fixture(&["a.dng"])?;
    let (id, path) = &f.photos[0];
    let m = crate::photo::open(path, &crate::photo::LensProfiles::none())?.metadata;
    let (profiles, _) = crate::camera_profiles::ProfileLibrary::for_tests().installed(&m);
    let mut edit = Recipe::with_profiles(&m, &profiles);
    edit.exposure = 0.5;
    f.catalog.save_edit(
        *id,
        path,
        &crate::storage::Identity::read(path)?,
        &edit,
        &Default::default(),
        HistoryUpdate::Keep,
    )?;
    let before = f.batch(f.batch_photos()?, settings(&f.out().join("before")));
    let expected = run_all(&before);
    // Edited again after Export was pressed.
    edit.exposure = -1.;
    f.catalog.save_edit(
        *id,
        path,
        &crate::storage::Identity::read(path)?,
        &edit,
        &Default::default(),
        HistoryUpdate::Keep,
    )?;
    let snapshot = Batch {
        settings: settings(&f.out().join("after")),
        plan: plan(
            &before.photos,
            &settings(&f.out().join("after")),
            &Default::default(),
            None,
        )
        .unwrap(),
        ..before
    };
    let outcomes = run_all(&snapshot);
    assert_eq!(
        pixels(exported(&outcomes[0])),
        pixels(exported(&expected[0]))
    );
    Ok(())
}

#[test]
fn the_queue_runs_one_batch_at_a_time_and_a_waiting_one_can_be_removed() -> Result<()> {
    let f = fixture(&["a.dng"])?;
    let photos = f.batch_photos()?;
    let done = Arc::new(Mutex::new(Vec::new()));
    let finished = done.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let changed = changes.clone();
    let queue = crate::export::queue::Queue::new(
        ONE,
        move |ticket, outcomes| {
            finished.lock().unwrap().push((ticket, outcomes));
            let _ = tx.send(ticket);
        },
        move || {
            changed.fetch_add(1, Ordering::Relaxed);
        },
    );
    let batch = |name: &str| f.batch(photos.clone(), settings(&f.out().join(name)));
    let first = queue.submit(batch("first"));
    let second = queue.submit(batch("second"));
    let third = queue.submit(batch("third"));
    // Nothing runs beside the first; the second waits and is removed, which those
    // showing the queue hear about.
    let heard = changes.load(Ordering::Relaxed);
    assert!(queue.remove(second));
    assert!(changes.load(Ordering::Relaxed) > heard);
    assert!(!queue.cancel(second));
    let mut finished = vec![rx.recv_timeout(std::time::Duration::from_secs(120))?];
    finished.push(rx.recv_timeout(std::time::Duration::from_secs(120))?);
    assert_eq!(finished, [first, third]);
    assert!(!queue.remove(first));
    assert!(!queue.busy());
    assert!(listing(&f.out().join("second")).is_empty());
    assert_eq!(listing(&f.out().join("third")), ["a.tif"]);
    assert!(
        done.lock()
            .unwrap()
            .iter()
            .all(|(_, o)| matches!(o[0], Outcome::Exported { .. }))
    );
    Ok(())
}

#[test]
fn the_queue_tells_each_photo_as_its_file_is_in_place_before_the_batch_ends() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng"])?;
    let photos = f.batch_photos()?;
    let heard = Arc::new(Mutex::new(Vec::new()));
    let each = heard.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let ended = heard.clone();
    let queue = crate::export::queue::Queue::with_each(
        lanes(2),
        move |ticket, index, outcome| {
            // The file is there when the photo is told.
            let Outcome::Exported { path, .. } = outcome else {
                panic!("{outcome:?}");
            };
            assert!(path.is_file());
            each.lock().unwrap().push((ticket, index));
        },
        move |ticket, outcomes| {
            let _ = tx.send((ticket, outcomes, ended.lock().unwrap().len()));
        },
        || {},
    );
    let ticket = queue.submit(f.batch(photos, settings(&f.out())));
    let (finished, outcomes, told) = rx.recv_timeout(std::time::Duration::from_secs(120))?;
    assert_eq!((finished, outcomes.len(), told), (ticket, 2, 2));
    let mut heard = heard.lock().unwrap().clone();
    heard.sort_unstable();
    assert_eq!(heard, [(ticket, 0), (ticket, 1)]);
    Ok(())
}

#[test]
fn a_batch_that_panics_outside_its_photos_fails_and_the_queue_runs_the_next() -> Result<()> {
    let f = fixture(&["a.dng"])?;
    let photos = f.batch_photos()?;
    let (tx, rx) = std::sync::mpsc::channel();
    // The status callback panics once on the queue's thread, as the batch
    // starts: outside the panic catch each photo has of its own.
    let submitter = std::thread::current().id();
    let panicked = Arc::new(AtomicBool::new(false));
    let queue = crate::export::queue::Queue::new(
        ONE,
        move |ticket, outcomes| {
            let _ = tx.send((ticket, outcomes));
        },
        move || {
            if std::thread::current().id() != submitter && !panicked.swap(true, Ordering::Relaxed) {
                panic!("status callback");
            }
        },
    );
    let batch = |name: &str| f.batch(photos.clone(), settings(&f.out().join(name)));
    let first = queue.submit(batch("first"));
    let second = queue.submit(batch("second"));
    let wait = std::time::Duration::from_secs(120);
    let (ticket, outcomes) = rx.recv_timeout(wait)?;
    assert_eq!(ticket, first);
    assert!(
        matches!(&outcomes[..], [Outcome::Failed(_)]),
        "{outcomes:?}"
    );
    let (ticket, outcomes) = rx.recv_timeout(wait)?;
    assert_eq!(ticket, second);
    assert!(
        matches!(&outcomes[..], [Outcome::Exported { .. }]),
        "{outcomes:?}"
    );
    assert!(!queue.busy());
    Ok(())
}

#[test]
fn closing_the_queue_stops_its_thread_and_starts_no_waiting_batch() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng"])?;
    let photos = f.batch_photos()?;
    let mut queue = crate::export::queue::Queue::new(ONE, |_, _| {}, || {});
    let batch = |name: &str| f.batch(photos.clone(), settings(&f.out().join(name)));
    queue.submit(batch("running"));
    queue.submit(batch("waiting"));
    let start = std::time::Instant::now();
    while queue.status().running.is_none() {
        assert!(start.elapsed().as_secs() < 60, "the batch did not start");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let thread = queue.close().expect("the queue's thread");
    // The running batch stops after the stage it is in.
    thread.join().unwrap();
    assert!(listing(&f.out().join("waiting")).is_empty());
    assert!(queue.close().is_none(), "closed once");
    Ok(())
}

#[test]
fn a_preset_watermark_is_read_once_for_the_whole_batch() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng"])?;
    let photos = f.batch_photos()?;
    // Its image is gone: read once, so every photo says why, and nothing is
    // written.
    let gone = crate::watermark::Watermark {
        name: "Gone".into(),
        style: crate::watermark::Style::Graphic,
        image: Some("rawmakase-test-missing-watermark.png".into()),
        ..crate::watermark::fonts::test_watermark()
    };
    let batch = Batch {
        watermark: Some(gone),
        watermarks: Default::default(),
        ..f.batch(photos.clone(), settings(&f.out().join("gone")))
    };
    let outcomes = run_all(&batch);
    assert!(
        outcomes.iter().all(
            |o| matches!(o, Outcome::Failed(reason) if reason.contains("Watermark image not found"))
        ),
        "{outcomes:?}"
    );
    assert!(listing(&f.out().join("gone")).is_empty());
    // A text preset marks every photo, as an export of one photo does.
    let text = crate::watermark::Watermark {
        name: "Text".into(),
        text: "RAWmakase".into(),
        ..crate::watermark::fonts::test_watermark()
    };
    let marked = run_all(&Batch {
        watermark: Some(text),
        watermarks: Default::default(),
        ..f.batch(photos.clone(), settings(&f.out().join("marked")))
    });
    let plain = run_all(&f.batch(photos, settings(&f.out().join("plain"))));
    for (marked, plain) in marked.iter().zip(&plain) {
        assert_ne!(pixels(exported(marked)), pixels(exported(plain)));
    }
    Ok(())
}

#[test]
fn the_open_photo_is_not_exported_over_a_file_replaced_since() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng"])?;
    let mut photos = f.batch_photos()?;
    let (a, b) = (&f.photos[0].1, &f.photos[1].1);
    let shown = |file| Edit::Shown {
        recipe: Box::new(Recipe::default()),
        unsaved: false,
        file,
    };
    photos[0].edit = shown(Some(crate::storage::Identity::read(a)?));
    // Only its modification time changed (issue #870): still the photo shown.
    let file = std::fs::File::options().write(true).open(a)?;
    file.set_modified(file.metadata()?.modified()? + std::time::Duration::from_secs(2))?;
    drop(file);
    // Another file than the one shown.
    photos[1].edit = shown(Some(crate::storage::Identity::read(b)?));
    let mut bytes = std::fs::read(b)?;
    bytes.extend_from_slice(b"replaced");
    std::fs::write(b, bytes)?;
    let outcomes = run_all(&f.batch(photos, settings(&f.out())));
    exported(&outcomes[0]);
    match &outcomes[1] {
        Outcome::Failed(reason) => assert!(reason.contains("changed"), "{reason}"),
        other => panic!("{other:?}"),
    }
    Ok(())
}

#[test]
fn an_overwrite_takes_the_exact_name_over_one_in_other_case() {
    // Both, as a case-sensitive volume can hold them.
    let dir = PathBuf::from("/out");
    let mut reserved = Reservations::default();
    reserved.on_disk.insert(
        dir.clone(),
        [(
            "a.tif".to_string(),
            vec![OsString::from("A.TIF"), OsString::from("a.tif")],
        )]
        .into(),
    );
    assert_eq!(
        reserved.existing(&dir.join("a.tif")),
        Some(dir.join("a.tif"))
    );
    assert_eq!(
        reserved.existing(&dir.join("A.TIF")),
        Some(dir.join("A.TIF"))
    );
    // No exact match: the one in other case.
    assert_eq!(
        reserved.existing(&dir.join("A.tif")),
        Some(dir.join("A.TIF"))
    );
}

#[test]
fn sequence_numbers_follow_the_photos_chosen_and_a_failure_keeps_its_number() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng", "c.dng"])?;
    let mut photos = f.batch_photos()?;
    // b is offline when its turn comes: it keeps 11, and c is still 12.
    std::fs::remove_file(&f.photos[1].1)?;
    let mut s = settings(&f.out());
    s.naming = Some(crate::export_settings::Naming::CustomNameSequence);
    s.custom_text = "Concert".into();
    s.start_number = 10;
    let outcomes = run_all(&f.batch(photos.clone(), s));
    assert!(matches!(outcomes[1], Outcome::Failed(_)), "{outcomes:?}");
    assert_eq!(listing(&f.out()), ["Concert-10.tif", "Concert-12.tif"]);
    // A photo left out before the export (offline, not RAW) keeps its number too.
    let mut placed = vec![photos[0].clone(), photos[2].clone()];
    placed[0].place = Some((0, 3));
    placed[1].place = Some((2, 3));
    let mut s = settings(&f.out().join("placed"));
    s.naming = Some(crate::export_settings::Naming::CustomNameOf);
    s.custom_text = "Show".into();
    run_all(&f.batch(placed, s));
    assert_eq!(
        listing(&f.out().join("placed")),
        ["Show (1 of 3).tif", "Show (3 of 3).tif"]
    );
    // Date - Filename: the capture date, or "undated" with a note, never the file's
    // modified date.
    photos[0].naming.captured = Some("2026-05-04 10:21:33.000".into());
    photos[2].naming.captured = None;
    let photos = vec![photos[0].clone(), photos[2].clone()];
    let mut s = settings(&f.out().join("dated"));
    s.naming = Some(crate::export_settings::Naming::DateFilename);
    let outcomes = run_all(&f.batch(photos, s));
    assert_eq!(
        listing(&f.out().join("dated")),
        ["20260504-a.tif", "undated-c.tif"]
    );
    match &outcomes[1] {
        Outcome::Exported { notes, .. } => assert!(notes.iter().any(|n| n.contains("undated"))),
        other => panic!("{other:?}"),
    }
    Ok(())
}

#[test]
fn exporting_a_jpeg_never_replaces_it() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("photo.jpg");
    image::RgbImage::from_fn(32, 24, |x, y| {
        image::Rgb([(x * 8) as u8, (y * 10) as u8, 128])
    })
    .save(&source)?;
    let original = std::fs::read(&source)?;
    let photos = vec![photo(&source)];
    let export = |existing: Existing| {
        let settings = ExportSettings {
            destination: Destination::SameFolder,
            naming: Some(crate::export_settings::Naming::Filename),
            format: Format::Jpeg,
            existing,
            ..Default::default()
        };
        let batch = Batch {
            plan: plan(&photos, &settings, &Default::default(), Some(existing)).unwrap(),
            photos: photos.clone(),
            settings,
            defaults: Arc::new(DevelopDefaults::with_presets(Default::default(), |_| None)),
            watermark: None,
            watermarks: Default::default(),
            demosaic: Default::default(),
            cache: Default::default(),
            lenses: crate::photo::LensProfiles::none(),
            profiles: crate::camera_profiles::ProfileLibrary::for_tests(),
            fits: Default::default(),
            ai: crate::ai_denoise::AiService::refusing("no AI Denoise in tests"),
        };
        run_all(&batch)
    };

    match &export(Existing::Overwrite)[..] {
        [Outcome::Failed(reason)] => {
            assert!(reason.contains("Cannot overwrite source"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read(&source)?, original);
    assert_eq!(listing(dir.path()), ["photo.jpg"]);

    // Unique names it as every export does: the next free "name-2", "name-3"…
    let outcomes = export(Existing::Unique);
    assert_eq!(exported(&outcomes[0]), dir.path().join("photo-2.jpg"));
    assert_eq!(std::fs::read(&source)?, original);
    assert_eq!(listing(dir.path()), ["photo-2.jpg", "photo.jpg"]);
    Ok(())
}

/// A template can give a photo another photo's file name: here a JPEG exported
/// earlier with the same template, chosen again beside the photo it came from.
/// Overwrite must not replace that other photo's original.
#[test]
fn exporting_never_replaces_another_photos_original() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (png, jpeg) = (
        dir.path().join("DSC01.png"),
        dir.path().join("2026-04-24 18.43.24.jpg"),
    );
    let image = image::RgbImage::from_fn(32, 24, |x, y| {
        image::Rgb([(x * 8) as u8, (y * 10) as u8, 128])
    });
    image.save(&png)?;
    image.save(&jpeg)?;
    let original = std::fs::read(&jpeg)?;
    let mut photos = vec![photo(&png), photo(&jpeg)];
    for p in &mut photos {
        p.naming.captured = Some("2026-04-24T18:43:24.000".into());
    }
    let mut settings = ExportSettings {
        destination: Destination::SameFolder,
        format: Format::Jpeg,
        existing: Existing::Overwrite,
        ..Default::default()
    };
    settings.choose_tokens(crate::export_settings::BUILTIN[0].tokens);
    let batch = Batch {
        plan: plan(&photos, &settings, &Default::default(), None).unwrap(),
        photos: photos.clone(),
        settings,
        defaults: Arc::new(DevelopDefaults::with_presets(Default::default(), |_| None)),
        watermark: None,
        watermarks: Default::default(),
        demosaic: Default::default(),
        cache: Default::default(),
        lenses: crate::photo::LensProfiles::none(),
        profiles: crate::camera_profiles::ProfileLibrary::for_tests(),
        fits: Default::default(),
        ai: crate::ai_denoise::AiService::refusing("no AI Denoise in tests"),
    };
    assert_eq!(batch.plan.entries[0].target, jpeg);
    let outcomes = run_all(&batch);
    match &outcomes[0] {
        Outcome::Failed(reason) => assert!(reason.contains("Cannot overwrite source"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        exported(&outcomes[1]),
        dir.path().join("2026-04-24 18.43.24-2.jpg")
    );
    assert_eq!(std::fs::read(&jpeg)?, original);
    Ok(())
}

/// Names found taken while the batch runs are numbered on from the photo's own
/// name, as names taken when it was planned are.
#[test]
fn a_name_taken_twice_during_the_export_gets_the_next_number() -> Result<()> {
    let f = fixture(&["a.dng"])?;
    let photos = f.batch_photos()?;
    let mut settings = settings(&f.out().join("twice"));
    settings.existing = Existing::Unique;
    let out = f.out().join("twice");
    let batch = Batch {
        plan: plan(&photos, &settings, &Default::default(), None).unwrap(),
        ..f.batch(photos.clone(), settings.clone())
    };
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("a.tif"), b"late")?;
    std::fs::write(out.join("a-2.tif"), b"late")?;
    let outcome = run_all(&batch).remove(0);
    assert_eq!(exported(&outcome), out.join("a-3.tif"));

    // Planned as a-4, beside a, a-2 and a-3: a-4 and a-5 then appear.
    let batch = Batch {
        plan: plan(&photos, &settings, &Default::default(), None).unwrap(),
        ..f.batch(photos, settings)
    };
    assert_eq!(batch.plan.entries[0].target, out.join("a-4.tif"));
    std::fs::write(out.join("a-4.tif"), b"late")?;
    std::fs::write(out.join("a-5.tif"), b"late")?;
    let outcome = run_all(&batch).remove(0);
    assert_eq!(exported(&outcome), out.join("a-6.tif"));
    Ok(())
}

/// A template from the Filename Template Editor names each photo from the
/// fields taken when Export was chosen, and says what each name lacks.
#[test]
fn a_custom_template_names_from_the_fields_taken_at_export() {
    use crate::export_settings::{CameraNames, NameFields};
    let d = tempfile::tempdir().unwrap();
    let mut photos = vec![
        photo(Path::new("/one/DSC01.ARW")),
        photo(Path::new("/one/DSC02.ARW")),
    ];
    photos[0].naming = NameFields {
        captured: Some("2026-04-24T18:43:24.000".into()),
        camera: CameraNames::Read {
            make: Some("Sony".into()),
            model: Some("ILCE-7RM6".into()),
        },
        ..Default::default()
    };
    // The same second, and no camera read: told apart, and said.
    photos[1].naming = NameFields {
        captured: Some("2026-04-24T18:43:24.000".into()),
        ..Default::default()
    };
    let mut s = settings(d.path());
    s.choose_tokens(crate::export_settings::BUILTIN[0].tokens);
    let plan = plan(&photos, &s, &Default::default(), None).unwrap();
    assert_eq!(
        names(&plan),
        [
            (
                String::from("2026-04-24 18.43.24 ILCE-7RM6.tif"),
                Write::Create
            ),
            (String::from("2026-04-24 18.43.24.tif"), Write::Create),
        ]
    );
    assert!(plan.entries[0].notes.is_empty());
    assert_eq!(
        plan.entries[1].notes,
        ["the camera's make and model were not read"]
    );
}

/// A photo with AI Denoise on and no AI result to be had fails with the reason; the
/// batch carries on with the others, and never writes that photo without AI Denoise.
#[test]
fn a_photo_without_its_ai_result_fails_and_the_batch_carries_on() -> Result<()> {
    let mut f = fixture(&["a.dng", "b.dng"])?;
    let [(a, a_path), _] = &f.photos[..] else {
        panic!("two photos");
    };
    let m = crate::photo::open(a_path, &crate::photo::LensProfiles::none())?.metadata;
    let (profiles, _) = crate::camera_profiles::ProfileLibrary::for_tests().installed(&m);
    let mut denoised = Recipe::with_profiles(&m, &profiles);
    denoised.ai_denoise = crate::model::ai_denoise::AiDenoise {
        enabled: true,
        amount: 0.5,
    };
    f.catalog.save_edit(
        *a,
        a_path,
        &crate::storage::Identity::read(a_path)?,
        &denoised,
        &Default::default(),
        HistoryUpdate::Keep,
    )?;
    let outcomes = run_all(&f.batch(f.batch_photos()?, settings(&f.out())));
    match &outcomes[0] {
        Outcome::Failed(reason) => {
            assert!(reason.contains("no AI Denoise in tests"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    exported(&outcomes[1]);
    assert_eq!(listing(&f.out()), ["b.tif"]);
    Ok(())
}

/// HDR JPEG XL exports (PQ, and a gain map over the SDR rendition) go through the
/// queue as SDR ones do: a cancel before the commit leaves neither the export nor a
/// temporary file, one after it counts as exported; a photo whose edit has HDR off
/// fails with the reason and the next one is still exported; a destination that can't
/// be made fails every photo and writes nothing; what is published is whole (it decodes
/// at the export's size with its HDR encoding); a preset watermark marks HDR TIFFs.
#[test]
fn hdr_jpeg_xl_exports_cancel_fail_and_publish_as_sdr_ones_do() -> Result<()> {
    use crate::color::hdr_output::{HdrGamut, HdrOutput, HdrTransfer};
    let mut f = fixture(&["a.dng", "b.dng"])?;
    let m = crate::photo::open(&f.photos[0].1, &crate::photo::LensProfiles::none())?.metadata;
    let (profiles, _) = crate::camera_profiles::ProfileLibrary::for_tests().installed(&m);
    let mut hdr = Recipe::with_profiles(&m, &profiles);
    hdr.hdr.enabled = true;
    hdr.hdr.limit = 4.;
    for (id, path) in f.photos.clone() {
        f.catalog.save_edit(
            id,
            &path,
            &crate::storage::Identity::read(&path)?,
            &hdr,
            &Default::default(),
            HistoryUpdate::Keep,
        )?;
    }
    let photos = f.batch_photos()?;
    let jxl = |out: &Path, output: HdrOutput| {
        let mut s = settings(out);
        s.format = Format::Jxl;
        s.jxl_lossy = true;
        s.hdr = Some(output);
        s
    };
    let pq = HdrOutput {
        gamut: HdrGamut::Rec2020,
        transfer: HdrTransfer::Pq,
        gain_map: false,
    };
    let gain_map = HdrOutput {
        gamut: HdrGamut::Rec709,
        gain_map: true,
        ..Default::default()
    };
    for (name, output, transfer) in [("pq", pq, "Pq"), ("gain-map", gain_map, "Srgb")] {
        let cancelled_at = |at: f32| -> Result<(Vec<Outcome>, Vec<String>)> {
            let out = f.out().join(format!("{name}-{at}"));
            let batch = f.batch(photos.clone(), jxl(&out, output));
            let cancel = AtomicBool::new(false);
            let outcomes = run(&batch, ONE, Cancel::new(&cancel), |p| {
                if p.done == 0 && p.fraction >= at {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
            Ok((outcomes, listing(&out)))
        };
        let (outcomes, files) = cancelled_at(0.95)?;
        assert_eq!(
            outcomes,
            [Outcome::Cancelled, Outcome::NotStarted],
            "{name}"
        );
        assert!(files.is_empty(), "{name}: {files:?}");
        let (outcomes, files) = cancelled_at(1.)?;
        let path = exported(&outcomes[0]).to_path_buf();
        assert_eq!(outcomes[1], Outcome::NotStarted, "{name}");
        assert_eq!(files, ["a.jxl"], "{name}");
        let decoded = jxl_oxide::JxlImage::builder()
            .read(std::io::Cursor::new(std::fs::read(&path)?))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let header = decoded.image_header();
        assert_eq!(header.size.width.max(header.size.height), 96, "{name}");
        let encoding = format!("{:?}", header.metadata.colour_encoding);
        assert!(encoding.contains(transfer), "{name}: {encoding}");
        decoded
            .render_frame(0)
            .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
    }
    // HDR off for the first photo: refused, and the second still exported.
    let mut off = photos.clone();
    let Edit::Catalog(record) = &mut off[0].edit else {
        panic!("a catalog edit");
    };
    record.recipe = None;
    let out = f.out().join("hdr-off");
    let outcomes = run_all(&f.batch(off, jxl(&out, pq)));
    match &outcomes[0] {
        Outcome::Failed(reason) => assert!(reason.contains("HDR off"), "{reason}"),
        other => panic!("{other:?}"),
    }
    exported(&outcomes[1]);
    assert_eq!(listing(&out), ["b.jxl"]);
    // A preset watermark marks HDR exports as it marks SDR ones.
    let text = crate::watermark::Watermark {
        name: "Text".into(),
        text: "RAWmakase".into(),
        ..crate::watermark::fonts::test_watermark()
    };
    let tiff = |out: &Path| {
        let mut s = settings(out);
        s.hdr = Some(pq);
        s
    };
    let marked = run_all(&Batch {
        watermark: Some(text),
        watermarks: Default::default(),
        ..f.batch(photos.clone(), tiff(&f.out().join("hdr-marked")))
    });
    let plain = run_all(&f.batch(photos.clone(), tiff(&f.out().join("hdr-plain"))));
    for (marked, plain) in marked.iter().zip(&plain) {
        assert_ne!(pixels(exported(marked)), pixels(exported(plain)));
    }
    // A destination folder that is a file.
    let blocked = f.out().join("blocked");
    std::fs::create_dir_all(f.out())?;
    std::fs::write(&blocked, b"a file")?;
    let outcomes = run_all(&f.batch(photos, jxl(&blocked.join("in"), pq)));
    assert!(
        outcomes.iter().all(|o| matches!(o, Outcome::Failed(_))),
        "{outcomes:?}"
    );
    assert_eq!(std::fs::read(&blocked)?, b"a file");
    Ok(())
}

/// AI Denoise that holds each photo until released, counting how many it holds
/// at once, and panics for the photo named `panics_on`: a way to keep photos of
/// a batch under way.
#[derive(Default)]
struct Holding {
    /// Photos held now, the most held at once, and whether they're released.
    state: Mutex<(usize, usize, bool)>,
    changed: std::sync::Condvar,
    panics_on: Option<&'static str>,
    /// The only photo held, when set; the others pass straight through.
    holds: Option<&'static str>,
}
impl crate::ai_denoise::AiBase for Holding {
    fn get(
        &self,
        path: &Path,
        _: &crate::storage::Identity,
        plain: &Arc<crate::camera_data::CameraImage>,
        cancel: Cancel<'_>,
    ) -> Result<Arc<crate::camera_data::CameraImage>> {
        let named = |name: Option<&str>| {
            name.is_some_and(|name| path.file_name().is_some_and(|n| n == name))
        };
        if named(self.panics_on) {
            panic!("AI Denoise of {}", path.display());
        }
        let mut image = (**plain).clone();
        image.recovered = Default::default();
        if self.holds.is_some() && !named(self.holds) {
            return Ok(Arc::new(image));
        }
        let mut state = self.state.lock().unwrap();
        state.0 += 1;
        state.1 = state.1.max(state.0);
        self.changed.notify_all();
        while !state.2 && !cancel.is_cancelled() {
            state = self
                .changed
                .wait_timeout(state, std::time::Duration::from_millis(10))
                .unwrap()
                .0;
        }
        state.0 -= 1;
        Ok(Arc::new(image))
    }
}
impl Holding {
    /// Waits until `n` photos are held. Should they never be, it releases those
    /// held before failing, so the batch ends rather than the test hanging.
    fn wait_for(&self, n: usize) {
        let start = std::time::Instant::now();
        let mut state = self.state.lock().unwrap();
        while state.0 < n {
            if start.elapsed().as_secs() >= 120 {
                state.2 = true;
                self.changed.notify_all();
                panic!("{} held, not {n}", state.0);
            }
            state = self
                .changed
                .wait_timeout(state, std::time::Duration::from_millis(10))
                .unwrap()
                .0;
        }
    }
    fn release(&self) {
        self.state.lock().unwrap().2 = true;
        self.changed.notify_all();
    }
    fn most(&self) -> usize {
        self.state.lock().unwrap().1
    }
}
impl Fixture {
    /// A batch of every photo with AI Denoise on, answered by `ai`.
    fn held(&self, ai: &Arc<Holding>, out: &Path) -> Result<Batch> {
        let photos = self
            .batch_photos()?
            .into_iter()
            .map(|mut p| {
                let mut recipe = crate::model::recipe::Recipe::default();
                recipe.ai_denoise.enabled = true;
                p.edit = Edit::Shown {
                    recipe: Box::new(recipe),
                    unsaved: false,
                    file: None,
                };
                p
            })
            .collect();
        Ok(Batch {
            ai: crate::ai_denoise::AiService(ai.clone()),
            ..self.batch(photos, settings(out))
        })
    }
}
fn lanes(photos: usize) -> Lanes {
    Lanes {
        photos,
        threads: 2,
        memory: u64::MAX,
    }
}

#[test]
fn photos_run_side_by_side_up_to_the_lanes_and_keep_their_order() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng", "c.dng", "d.dng", "e.dng"])?;
    let ai = Arc::new(Holding::default());
    let batch = f.held(&ai, &f.out().join("lanes"))?;
    let outcomes = std::thread::scope(|s| {
        let running = s.spawn(|| run(&batch, lanes(2), Cancel::NEVER, |_| {}));
        ai.wait_for(2);
        // A third would be held by now if the lanes let it start.
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(ai.most(), 2);
        ai.release();
        running.join().unwrap()
    });
    let paths: Vec<_> = outcomes.iter().map(|o| exported(o).to_path_buf()).collect();
    assert_eq!(
        paths,
        batch
            .plan
            .entries
            .iter()
            .map(|p| p.target.clone())
            .collect::<Vec<_>>()
    );
    // The same files as one photo at a time.
    let one = Batch {
        plan: plan(
            &batch.photos,
            &settings(&f.out().join("one")),
            &Default::default(),
            None,
        )
        .unwrap(),
        settings: settings(&f.out().join("one")),
        ..batch.clone()
    };
    let one = run(&one, ONE, Cancel::NEVER, |_| {});
    for (a, b) in outcomes.iter().zip(&one) {
        assert_eq!(pixels(exported(a)), pixels(exported(b)));
    }
    Ok(())
}

#[test]
fn cancel_stops_the_photos_under_way_and_starts_no_other() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng", "c.dng", "d.dng"])?;
    let ai = Arc::new(Holding::default());
    let out = f.out().join("cancel");
    let batch = f.held(&ai, &out)?;
    let cancel = AtomicBool::new(false);
    let outcomes = std::thread::scope(|s| {
        let running = s.spawn(|| run(&batch, lanes(2), Cancel::new(&cancel), |_| {}));
        ai.wait_for(2);
        cancel.store(true, Ordering::Relaxed);
        running.join().unwrap()
    });
    assert_eq!(
        outcomes,
        [
            Outcome::Cancelled,
            Outcome::Cancelled,
            Outcome::NotStarted,
            Outcome::NotStarted
        ]
    );
    assert_eq!(ai.most(), 2);
    // No export and no temporary file.
    assert!(listing(&out).is_empty(), "{:?}", listing(&out));
    Ok(())
}

#[test]
fn photos_finishing_out_of_order_keep_their_places_and_a_panic_fails_its_own() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng", "c.dng", "d.dng", "e.dng"])?;
    // The first photo is held while the other lane exports the rest, one of
    // which panics.
    let ai = Arc::new(Holding {
        holds: Some("a.dng"),
        panics_on: Some("c.dng"),
        ..Default::default()
    });
    let batch = f.held(&ai, &f.out().join("order"))?;
    let reports = Mutex::new(Vec::new());
    let outcomes = std::thread::scope(|s| {
        let running = s.spawn(|| {
            run(&batch, lanes(2), Cancel::NEVER, |p| {
                reports.lock().unwrap().push(p.done)
            })
        });
        ai.wait_for(1);
        // Every other photo is done while the first is still held.
        let start = std::time::Instant::now();
        let mut others_done = false;
        while !others_done && start.elapsed().as_secs() < 120 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            others_done = reports.lock().unwrap().last() == Some(&4);
        }
        // Released before any assertion, so a failure can't leave it waiting.
        ai.release();
        let outcomes = running.join().unwrap();
        assert!(others_done, "the other photos did not finish first");
        outcomes
    });
    for (i, outcome) in outcomes.iter().enumerate() {
        match i {
            2 => assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}"),
            _ => assert_eq!(exported(outcome), batch.plan.entries[i].target, "{i}"),
        }
    }
    Ok(())
}

#[test]
fn photos_over_the_memory_budget_run_one_at_a_time() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng", "c.dng"])?;
    let ai = Arc::new(Holding::default());
    let batch = f.held(&ai, &f.out().join("budget"))?;
    let tight = Lanes {
        memory: 1,
        ..lanes(3)
    };
    let outcomes = std::thread::scope(|s| {
        let running = s.spawn(|| run(&batch, tight, Cancel::NEVER, |_| {}));
        ai.wait_for(1);
        std::thread::sleep(std::time::Duration::from_millis(200));
        ai.release();
        running.join().unwrap()
    });
    assert_eq!(ai.most(), 1);
    assert!(
        outcomes
            .iter()
            .all(|o| matches!(o, Outcome::Exported { .. })),
        "{outcomes:?}"
    );
    Ok(())
}

#[test]
fn cancel_stops_a_photo_waiting_for_memory() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng", "c.dng"])?;
    let ai = Arc::new(Holding::default());
    let out = f.out().join("waiting");
    let batch = f.held(&ai, &out)?;
    let tight = Lanes {
        memory: 1,
        ..lanes(2)
    };
    let cancel = AtomicBool::new(false);
    let outcomes = std::thread::scope(|s| {
        let running = s.spawn(|| run(&batch, tight, Cancel::new(&cancel), |_| {}));
        ai.wait_for(1);
        // The other lane's photo is waiting for the first one's memory.
        std::thread::sleep(std::time::Duration::from_millis(200));
        cancel.store(true, Ordering::Relaxed);
        running.join().unwrap()
    });
    assert_eq!(
        outcomes,
        [Outcome::Cancelled, Outcome::Cancelled, Outcome::NotStarted]
    );
    assert_eq!(ai.most(), 1);
    assert!(listing(&out).is_empty(), "{:?}", listing(&out));
    Ok(())
}

#[test]
fn progress_with_photos_side_by_side_never_goes_back() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng", "c.dng", "d.dng", "e.dng"])?;
    let batch = f.batch(f.batch_photos()?, settings(&f.out().join("progress")));
    let reports = Mutex::new(Vec::new());
    run(&batch, lanes(3), Cancel::NEVER, |p| {
        reports.lock().unwrap().push(p)
    });
    let reports = reports.into_inner().unwrap();
    let at: Vec<f32> = reports.iter().map(|p| p.done as f32 + p.fraction).collect();
    assert!(at.windows(2).all(|w| w[1] >= w[0] - 1e-6), "{at:?}");
    assert!(
        reports
            .iter()
            .all(|p| p.done as f32 + p.fraction <= 5. + 1e-6)
    );
    assert_eq!(reports.last().map(|p| p.done), Some(5));
    Ok(())
}

#[test]
fn files_appearing_under_photos_side_by_side_get_them_distinct_names() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng"])?;
    let ai = Arc::new(Holding::default());
    let out = f.out().join("late");
    let batch = f.held(&ai, &out)?;
    let batch = Batch {
        plan: Plan {
            late: Existing::Unique,
            ..batch.plan.clone()
        },
        ..batch
    };
    let outcomes = std::thread::scope(|s| {
        let running = s.spawn(|| run(&batch, lanes(2), Cancel::NEVER, |_| {}));
        ai.wait_for(2);
        // Both planned names, and the first unique ones, are taken meanwhile.
        std::fs::create_dir_all(&out).unwrap();
        for name in ["a.tif", "b.tif", "a-2.tif", "b-2.tif"] {
            std::fs::write(out.join(name), b"not ours").unwrap();
        }
        ai.release();
        running.join().unwrap()
    });
    let names: Vec<_> = outcomes
        .iter()
        .map(|o| {
            exported(o)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert_eq!(names, ["a-3.tif", "b-3.tif"]);
    assert_eq!(listing(&out).len(), 6, "{:?}", listing(&out));
    // Nothing of theirs was replaced.
    for name in ["a.tif", "b.tif", "a-2.tif", "b-2.tif"] {
        assert_eq!(std::fs::read(out.join(name))?, b"not ours");
    }
    Ok(())
}

/// Limit File Size To in a batch: a limit the photo fits exports it within the limit
/// without a note; one even the lowest quality can't meet exports the smallest file,
/// with the note the export's summary shows.
#[test]
fn a_size_limit_fits_each_jpeg_or_notes_why_not() -> Result<()> {
    let f = fixture(&["a.dng"])?;
    let photos = f.batch_photos()?;
    for (name, size_limit, noted) in [("fits", 1_000, false), ("too-small", 1, true)] {
        let out = f.out().join(name);
        let mut s = settings(&out);
        s.format = Format::Jpeg;
        s.limit_size = true;
        s.size_limit = size_limit;
        let outcomes = run_all(&f.batch(photos.clone(), s));
        let Outcome::Exported { path, notes, .. } = &outcomes[0] else {
            panic!("{name}: not exported: {outcomes:?}");
        };
        let size = std::fs::metadata(path)?.len();
        let limit_noted = notes.iter().any(|n| n.contains("lowest quality"));
        assert_eq!(limit_noted, noted, "{name}: {notes:?}");
        if !noted {
            assert!(size <= u64::from(size_limit) * 1000, "{name}: {size}");
        }
        image::open(path)?;
    }
    Ok(())
}

/// `batch::run_with_backend` with `ExportBackend::auto()` matches the CPU reference
/// across zero-copy `Rgb8` (PNG/JPEG), zero-copy `Rgb16` (TIFF), resized exports,
/// and watermarked exports (which must keep `F32` pixels for floating-point blending).
#[test]
fn batch_with_backend_matches_cpu_across_formats_and_watermarks() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng"])?;
    let photos = f.batch_photos()?;
    let cpu_backend = crate::develop::ExportBackend::cpu_only();
    let auto_backend = crate::develop::ExportBackend::auto();

    for (label, format, resize, watermark) in [
        ("png-full", Format::Png, false, false),
        ("tif-resized", Format::Tiff, true, false),
        ("jpg-full", Format::Jpeg, false, false),
        ("png-watermark", Format::Png, false, true),
    ] {
        let mut s_cpu = settings(&f.out().join(format!("{label}-cpu")));
        s_cpu.format = format;
        s_cpu.resize = resize;
        s_cpu.long_edge = 96;
        let mut s_gpu = s_cpu.clone();
        s_gpu.folder = Some(f.out().join(format!("{label}-gpu")));

        let mut b_cpu = f.batch(photos.clone(), s_cpu);
        let mut b_gpu = f.batch(photos.clone(), s_gpu);
        if watermark {
            let wm = crate::watermark::Watermark {
                name: "BenchMark".into(),
                text: "RAWmakase".into(),
                ..crate::watermark::fonts::test_watermark()
            };
            b_cpu.watermark = Some(wm.clone());
            b_gpu.watermark = Some(wm);
        }

        let out_cpu = run_with_backend(&b_cpu, lanes(2), &cpu_backend, Cancel::NEVER, |_| {});
        let out_gpu = run_with_backend(&b_gpu, lanes(2), &auto_backend, Cancel::NEVER, |_| {});
        assert_eq!(out_cpu.len(), 2);
        assert_eq!(out_gpu.len(), 2);

        for (i, (c, g)) in out_cpu.iter().zip(&out_gpu).enumerate() {
            let pc = exported(c);
            let pg = exported(g);
            match format {
                Format::Png => {
                    let ic = image::open(pc)?.to_rgb8();
                    let ig = image::open(pg)?.to_rgb8();
                    assert_eq!(ic.dimensions(), ig.dimensions(), "{label}[{i}]");
                    let max_diff = ic
                        .as_raw()
                        .iter()
                        .zip(ig.as_raw())
                        .map(|(&x, &y)| x.abs_diff(y))
                        .max()
                        .unwrap_or(0);
                    assert!(max_diff <= 1, "{label}[{i}]: max_diff={max_diff}");
                }
                Format::Jpeg => {
                    // Lossy 8×8 DCT + 4:2:0 subsampling can amplify a 1-LSB input
                    // difference locally across a block, so bound both RMS and peak.
                    let ic = image::open(pc)?.to_rgb8();
                    let ig = image::open(pg)?.to_rgb8();
                    assert_eq!(ic.dimensions(), ig.dimensions(), "{label}[{i}]");
                    let mut sum_sq = 0u64;
                    let mut max_diff = 0u8;
                    for (&x, &y) in ic.as_raw().iter().zip(ig.as_raw()) {
                        let d = x.abs_diff(y);
                        max_diff = max_diff.max(d);
                        sum_sq += u64::from(d) * u64::from(d);
                    }
                    let rms = (sum_sq as f64 / ic.as_raw().len() as f64).sqrt();
                    assert!(
                        rms <= 1.5 && max_diff <= 16,
                        "{label}[{i}]: rms={rms:.3}, max_diff={max_diff}"
                    );
                }
                Format::Tiff => {
                    let ic = image::open(pc)?.to_rgb16();
                    let ig = image::open(pg)?.to_rgb16();
                    assert_eq!(ic.dimensions(), ig.dimensions(), "{label}[{i}]");
                    let max_diff = ic
                        .as_raw()
                        .iter()
                        .zip(ig.as_raw())
                        .map(|(&x, &y)| x.abs_diff(y))
                        .max()
                        .unwrap_or(0);
                    assert!(max_diff <= 6, "{label}[{i}]: max_diff={max_diff}");
                }
                Format::Jxl => unreachable!(),
            }
        }
    }

    let status = auto_backend.status();
    assert_eq!(status.gpu_renders + status.cpu_renders, 8);
    Ok(())
}

/// `Queue::with_backend` shares one `ExportBackend` across consecutive batches,
/// reports non-blocking backend status, releases buffers when idle, and closes cleanly.
#[test]
fn queue_with_backend_runs_batches_and_closes_cleanly() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng"])?;
    let photos = f.batch_photos()?;
    let backend = Arc::new(crate::develop::ExportBackend::auto());
    let (tx, rx) = std::sync::mpsc::channel();
    let mut queue = crate::export::queue::Queue::with_backend(
        lanes(2),
        backend,
        |_, _, _| {},
        move |ticket, outcomes| {
            let _ = tx.send((ticket, outcomes));
        },
        || {},
    );
    let mut s1 = settings(&f.out().join("q1"));
    s1.format = Format::Png;
    let mut s2 = settings(&f.out().join("q2"));
    s2.format = Format::Tiff;
    let t1 = queue.submit(f.batch(photos.clone(), s1));
    let t2 = queue.submit(f.batch(photos, s2));
    let wait = std::time::Duration::from_secs(120);
    let (done1, o1) = rx.recv_timeout(wait)?;
    let (done2, o2) = rx.recv_timeout(wait)?;
    assert_eq!((done1, done2), (t1, t2));
    assert!(o1.iter().all(|o| matches!(o, Outcome::Exported { .. })));
    assert!(o2.iter().all(|o| matches!(o, Outcome::Exported { .. })));
    let status = queue.backend_status();
    assert_eq!(status.gpu_renders + status.cpu_renders, 4);
    let thread = queue.close().expect("queue worker thread");
    thread.join().unwrap();
    Ok(())
}

/// The batch-scoped `ProfileCache` produces identical profile lists to
/// `ProfileLibrary::installed` and keeps distinct cameras / lenses / matrices apart.
#[test]
fn profile_cache_matches_installed_and_separates_distinct_cameras() {
    let cache = ProfileCache::default();
    let library = crate::camera_profiles::ProfileLibrary::for_tests();
    let base = crate::camera_data::Metadata {
        make: "TestMake".into(),
        model: "TestModel".into(),
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let expected = library.installed(&base).0;
    assert_eq!(cache.installed(&library, &base), expected);
    // Second lookup hits the cached slot and returns the exact same profiles.
    assert_eq!(cache.installed(&library, &base), expected);

    // Changing cam_xyz must produce a distinct cache entry and match `installed`.
    let mut different_matrix = base.clone();
    different_matrix.cam_xyz = [[0.9, 0.1, 0.], [0., 1., 0.], [0., 0.1, 0.9]];
    assert_eq!(
        cache.installed(&library, &different_matrix),
        library.installed(&different_matrix).0
    );
    assert_ne!(cache.installed(&library, &different_matrix), expected);

    // Changing only dng_matrix_profile_signature must produce a distinct entry
    // so matrix-only DNGs keep their own calibration signature separate from
    // native RAWs with the same camera matrix.
    let mut different_sig = base.clone();
    different_sig.dng_matrix_profile_signature = Some("org.example.dng-writer".into());
    let expected_sig = library.installed(&different_sig).0;
    assert_eq!(cache.installed(&library, &different_sig), expected_sig);
    assert_ne!(expected_sig, expected);

    // Rendered metadata bypasses the cache without panicking or colliding.
    let mut rendered = base;
    rendered.source = crate::camera_data::Source::Rendered;
    assert_eq!(
        cache.installed(&library, &rendered),
        library.installed(&rendered).0
    );
}
