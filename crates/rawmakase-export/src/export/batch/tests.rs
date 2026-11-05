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
    let plan = plan(&photos, &settings(&out), None).unwrap();
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
    let plan = super::plan(&photos[..2], &same, None).unwrap();
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
    let plan = super::plan(&[photo(Path::new("/one/c.dng"))], &unique, None).unwrap();
    assert_eq!(names(&plan), [s("c-2.tif", Write::Create)]);
    // Overwrite replaces the file that is there, as it is spelled, and Ask names it
    // so: never a second file beside it in other case.
    let ask = settings(&out);
    assert_eq!(
        super::plan(&[photo(Path::new("/one/c.dng"))], &ask, None).unwrap_err(),
        Unplanned::Conflicts(vec![out.join("C.TIF")])
    );
    let plan = super::plan(
        &[photo(Path::new("/one/c.dng"))],
        &ask,
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
        plan(&photos, &ask, None).unwrap_err(),
        Unplanned::Conflicts(vec![out.join("a.tif"), out.join("b.tif")])
    );
    let overwrite = plan(&photos, &ask, Some(Existing::Overwrite)).unwrap();
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
    let skip = plan(&photos, &ask, Some(Existing::Skip)).unwrap();
    assert_eq!(
        names(&skip),
        [
            s("a.tif", Write::Skip),
            s("b.tif", Write::Skip),
            s("c.tif", Write::Create),
            s("a.tif", Write::Skip),
        ]
    );
    let unique = plan(&photos, &ask, Some(Existing::Unique)).unwrap();
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
    let fresh = plan(&photos[2..3], &ask, None).unwrap();
    assert_eq!(fresh.late, Existing::Skip);
    // No folder chosen.
    let mut none = ask;
    none.folder = None;
    assert_eq!(plan(&photos, &none, None).unwrap_err(), Unplanned::NoFolder);
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
                BatchPhoto::from_record(record, path.clone(), name, naming)
            })
            .collect())
    }
    fn batch(&self, photos: Vec<BatchPhoto>, settings: ExportSettings) -> Batch {
        Batch {
            plan: plan(&photos, &settings, Some(Existing::Unique)).unwrap(),
            photos,
            settings,
            defaults: Arc::new(DevelopDefaults::with_presets(Default::default(), |_| None)),
            watermark: None,
            demosaic: Default::default(),
            cache: Default::default(),
            ai: crate::ai_denoise::AiService::refusing("no AI Denoise in tests"),
        }
    }
}
fn run_all(batch: &Batch) -> Vec<Outcome> {
    run(batch, Cancel::NEVER, |_| {})
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
    let m = crate::photo::open(a_path)?.metadata;
    let (profiles, _) = crate::camera_profiles::installed(&m);
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
            plan: plan(&photos, &settings, answer).unwrap(),
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
            plan: plan(&photos, &settings, None).unwrap(),
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
        let outcomes = run(&batch, Cancel::new(&cancel), |p| {
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
    let m = crate::photo::open(path)?.metadata;
    let (profiles, _) = crate::camera_profiles::installed(&m);
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
        plan: plan(&before.photos, &settings(&f.out().join("after")), None).unwrap(),
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
fn closing_the_queue_stops_its_thread_and_starts_no_waiting_batch() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng"])?;
    let photos = f.batch_photos()?;
    let mut queue = crate::export::queue::Queue::new(|_, _| {}, || {});
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
        ..Default::default()
    };
    let batch = Batch {
        watermark: Some(gone),
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
        ..Default::default()
    };
    let marked = run_all(&Batch {
        watermark: Some(text),
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
            plan: plan(&photos, &settings, Some(existing)).unwrap(),
            photos: photos.clone(),
            settings,
            defaults: Arc::new(DevelopDefaults::with_presets(Default::default(), |_| None)),
            watermark: None,
            demosaic: Default::default(),
            cache: Default::default(),
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
    let plan = plan(&photos, &s, None).unwrap();
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
    let m = crate::photo::open(a_path)?.metadata;
    let (profiles, _) = crate::camera_profiles::installed(&m);
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
