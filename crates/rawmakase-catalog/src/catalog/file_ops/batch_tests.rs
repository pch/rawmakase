//! What a batch of items moved or renamed within their drive decides
//! (#681, #692): as each item would on its own, in the order it is
//! journalled, whatever an earlier item of its batch does, a step fails or
//! another program changes meanwhile, and whatever a crash interrupts.
use super::journal;
use super::recover_tests;
use super::tests::*;
use super::*;
use crate::fsops;
use rawmakase_cancel::Cancel;

/// Photos `IMG_0000.NEF`… in one folder, every third with an XMP sidecar.
fn many(n: usize) -> Vec<String> {
    (0..n)
        .flat_map(|i| {
            let photo = format!("IMG_{i:04}.NEF");
            let sidecar = (i % 3 == 0).then(|| format!("IMG_{i:04}.xmp"));
            std::iter::once(photo).chain(sidecar)
        })
        .collect()
}

#[test]
fn a_name_an_earlier_item_takes_in_the_catalog_leaves_the_later_one_untouched() {
    // Two photos whose names differ only in case, moved into one folder of
    // a volume that tells them apart: the disk would take both, but the
    // catalog holds one photo of a name in a folder (by
    // `sidecar::caseless`), so the second is left where it is, as it was
    // when items went one at a time.
    let mut fx = fixture(&["x/a.NEF", "y/A.NEF", "sub/"]);
    let sub = fx.root.join("sub");
    let names = crate::fsops::list(&sub).unwrap();
    if crate::fsops::probe(&sub, &names).case != crate::fsops::Folding::Exact {
        return;
    }
    let photos = vec![fx.photo("x/a.NEF"), fx.photo("y/A.NEF")];
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos: photos.clone(),
            root: fx.root_id(),
            to: "sub".into(),
        },
        &NoTrash,
        &Faulty::default(),
    );
    let results = results(&outcome);
    assert!(
        matches!(
            &results[..],
            [ItemResult::Moved { .. }, ItemResult::Unchanged(Some(why))] if why.contains("is named")
        ),
        "{results:?}"
    );
    let rows = fx.rows();
    assert!(
        photos.iter().all(|p| fx.root.join(&rows[p]).is_file()),
        "{rows:?}"
    );
}

#[test]
fn a_sidecar_an_earlier_item_leaves_to_a_later_one_keeps_it_from_going() {
    // a.CR2 and a.NEF share a.xmp, so neither carries it. Once the first is
    // renamed, a.xmp is the other's alone: that one is left as it is, as it
    // was when items went one at a time, rather than leave a.xmp behind.
    let mut fx = fixture(&["a.NEF", "a.CR2", "a.xmp"]);
    let mut ids = [fx.photo("a.NEF"), fx.photo("a.CR2")];
    ids.sort();
    let outcome = fx.run(
        FileOp::RenamePhotos {
            renames: vec![(ids[0], "first".into()), (ids[1], "later".into())],
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(
        matches!(
            &results(&outcome)[..],
            [ItemResult::Renamed { .. }, ItemResult::Unchanged(Some(why))] if why.contains("sidecars changed")
        ),
        "{:?}",
        results(&outcome)
    );
    assert!(fx.root.join("a.xmp").is_file());
}

#[test]
fn a_sidecar_made_while_the_batch_renames_puts_its_item_back() {
    // Another program writes later.NEF.xmp during the first rename: the
    // second item, renamed to later.NEF, would read it over its own
    // later.xmp, so it is put back before the catalog follows.
    let mut fx = fixture(&["a.NEF", "a.xmp", "b.NEF", "b.xmp"]);
    let mut ids = [fx.photo("a.NEF"), fx.photo("b.NEF")];
    ids.sort();
    let raced = fx.root.join("later.NEF.xmp");
    let written = std::cell::Cell::new(false);
    let at = raced.clone();
    let disk = Faulty {
        watch: Some(Box::new(move |call: &Call| {
            if matches!(call, Call::Rename(..)) && !written.replace(true) {
                std::fs::write(&at, b"another program's").unwrap();
            }
        })),
        ..Faulty::default()
    };
    let outcome = fx.run(
        FileOp::RenamePhotos {
            renames: vec![(ids[0], "first".into()), (ids[1], "later".into())],
        },
        &NoTrash,
        &disk,
    );
    assert!(
        matches!(
            &results(&outcome)[..],
            [ItemResult::Renamed { .. }, ItemResult::Unchanged(Some(why))] if why.contains("share an XMP sidecar")
        ),
        "{:?}",
        results(&outcome)
    );
    let later = fx.rows()[&ids[1]].clone();
    assert!(fx.root.join(&later).is_file(), "{later}");
    assert_eq!(std::fs::read(&raced).unwrap(), b"another program's");
}

/// A volume that stores names decomposed (as HFS+ does): asked for
/// "café.NEF", it names the file "cafe\u{301}.NEF".
struct Decomposing(Faulty);

impl disk::Disk for Decomposing {
    fn entry(&self, p: &std::path::Path) -> Result<fsops::Entry, fsops::FsError> {
        self.0.entry(p)
    }
    fn list(&self, d: &std::path::Path) -> Result<Vec<std::ffi::OsString>, fsops::FsError> {
        self.0.list(d)
    }
    fn identity(&self, p: &std::path::Path) -> anyhow::Result<crate::storage::Identity> {
        self.0.identity(p)
    }
    fn rename(&self, a: &std::path::Path, b: &std::path::Path) -> Result<(), fsops::FsError> {
        use unicode_normalization::UnicodeNormalization;
        let name: String = b.file_name().unwrap().to_string_lossy().nfd().collect();
        self.0.rename(a, &b.with_file_name(name))
    }
    fn create_dir(&self, p: &std::path::Path) -> Result<(), fsops::FsError> {
        self.0.create_dir(p)
    }
    fn sync_dir(&self, d: &std::path::Path) -> Result<(), fsops::FsError> {
        self.0.sync_dir(d)
    }
    fn canonical(&self, p: &std::path::Path) -> Result<std::path::PathBuf, fsops::FsError> {
        self.0.canonical(p)
    }
    passes_on!(0);
}

#[test]
fn a_failed_sidecar_rename_puts_back_a_photo_the_disk_spelled_otherwise() {
    // The photo is renamed (and spelled decomposed by the disk), its
    // sidecar's rename fails: the photo is found by the name the disk gave
    // it and put back, and the journal names it so too.
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let a = fx.photo("a.NEF");
    let outcome = fx.run(
        FileOp::RenamePhotos {
            renames: vec![(a, "caf\u{e9}".into())],
        },
        &NoTrash,
        &Decomposing(Faulty::at(Op::Rename, 1, Fault::Fail)),
    );
    assert!(
        matches!(&results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
        "{:?}",
        results(&outcome)
    );
    assert!(fx.root.join("a.NEF").is_file() && fx.root.join("a.xmp").is_file());
    assert_eq!(fx.rows()[&a], "a.NEF");
}

/// Two batches' worth of photos in one folder, and their rename.
fn two_batches() -> (Fx, FileOp) {
    let photos = execute::BATCH + 2;
    let names = many(photos);
    let files: Vec<&str> = names.iter().map(String::as_str).collect();
    let fx = fixture(&files);
    let renames = (0..photos)
        .map(|i| (fx.photo(&format!("IMG_{i:04}.NEF")), format!("trip_{i:04}")))
        .collect();
    (fx, FileOp::RenamePhotos { renames })
}

/// Every photo of `fx` with its sidecar, by its old name or its new one,
/// never split between them.
fn whole(fx: &Fx) {
    for i in 0..execute::BATCH + 2 {
        let old = fx.root.join(format!("IMG_{i:04}.NEF")).is_file();
        let new = fx.root.join(format!("trip_{i:04}.NEF")).is_file();
        assert!(old != new, "photo {i}");
        if i % 3 == 0 {
            let stem = if new { "trip" } else { "IMG" };
            assert!(
                fx.root.join(format!("{stem}_{i:04}.xmp")).is_file(),
                "sidecar {i}"
            );
        }
    }
}

#[test]
fn a_fault_in_either_batch_of_a_rename_ends_proven() {
    // Where the second batch's renames start: after the first batch's
    // items' members, each renamed once.
    let (mut clean, op) = two_batches();
    let counting = Faulty::default();
    let outcome = clean.run(op, &NoTrash, &counting);
    let renames = counting.count(Op::Rename);
    let second: usize = outcome.items[..execute::BATCH]
        .iter()
        .map(|i| {
            journal::item(&clean.cat.db, i.operation)
                .unwrap()
                .unwrap()
                .members
                .len()
        })
        .sum();
    assert!(0 < second && second < renames);
    // Every sync: each batch's rounds, its photos and then their sidecars.
    let syncs = counting.count(Op::Sync);
    let steps = [
        (Op::Rename, 0),
        (Op::Rename, second - 1),
        (Op::Rename, second),
        (Op::Rename, renames - 1),
    ]
    .into_iter()
    .chain((0..syncs).map(|nth| (Op::Sync, nth)));
    for (at, nth) in steps {
        for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
            let (mut fx, op) = two_batches();
            let was = recover_tests::before(&fx);
            let plan = fx.plan(op);
            let batch = plan.batch().clone();
            let disk = Faulty::at(at, nth, fault);
            crashes(|| {
                drop(execute::execute_with(
                    &mut fx.cat,
                    plan.confirm(),
                    &NoTrash,
                    Cancel::NEVER,
                    &disk,
                ));
            });
            fx.reopen();
            let recovery = fx.recover(&disk::Real);
            recover_tests::check(&fx, &was, &recovery, &batch);
            for u in &recovery.unfinished {
                let back =
                    resolve(&mut fx.cat, u.operation, Resolution::PutBack, &NoTrash).unwrap();
                assert!(
                    matches!(back.result, ItemResult::Unchanged(_)),
                    "{at:?} {nth} {fault:?}: {back:?}"
                );
            }
            recover_tests::check(&fx, &was, &Recovery::default(), &batch);
            whole(&fx);
        }
    }
}

#[test]
fn a_failed_completion_in_a_batch_leaves_only_its_item_and_a_restart_finishes_it() {
    // A batch's completion fails on one photo's row: the batch's other
    // items are completed one by one, that one left for recovery.
    let (mut fx, op) = two_batches();
    let was = recover_tests::before(&fx);
    let failing = fx.photo("IMG_0001.NEF");
    fx.fail_journal("BEFORE UPDATE ON photos WHEN NEW.filename = 'trip_0001.NEF'");
    let plan = fx.plan(op);
    let batch = plan.batch().clone();
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )
    .unwrap();
    let left: Vec<&ItemOutcome> = outcome
        .items
        .iter()
        .filter(|i| !matches!(i.result, ItemResult::Renamed { .. }))
        .collect();
    assert!(
        matches!(&left[..], [i] if i.photos == [failing] && matches!(i.result, ItemResult::NeedsRecovery(_))),
        "{left:?}"
    );
    fx.reopen();
    let recovery = fx.recover(&disk::Real);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    recover_tests::check(&fx, &was, &recovery, &batch);
    assert!(
        super::outcome(&fx.cat, &batch)
            .unwrap()
            .items
            .iter()
            .all(|i| matches!(i.result, ItemResult::Renamed { .. }))
    );
}

#[test]
fn a_failed_journal_commit_in_the_second_batch_puts_only_that_batch_back() {
    let (mut fx, op) = two_batches();
    let was = recover_tests::before(&fx);
    fx.fail_journal(&format!(
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'moved' AND NEW.operation > {}",
        execute::BATCH
    ));
    let plan = fx.plan(op);
    let batch = plan.batch().clone();
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )
    .unwrap();
    let results = results(&outcome);
    assert!(
        results[..execute::BATCH]
            .iter()
            .all(|r| matches!(r, ItemResult::Renamed { .. }))
    );
    assert!(
        results[execute::BATCH..]
            .iter()
            .all(|r| matches!(r, ItemResult::Unchanged(Some(_)))),
        "{results:?}"
    );
    whole(&fx);
    fx.reopen();
    let recovery = fx.recover(&disk::Real);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    recover_tests::check(&fx, &was, &recovery, &batch);
}

#[test]
fn every_member_journalled_moved_was_renamed_and_its_folders_synced_first() {
    // Whenever the journal is read between two disk calls of a batched
    // move, each member it has moved was renamed by an earlier call, and
    // both its folders synced after that: the order that matters, however
    // the renames of a batch's items interleave.
    let mut names = many(12);
    names.push("sub/".into());
    let files: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut fx = fixture(&files);
    let photos = (0..12)
        .map(|i| fx.photo(&format!("IMG_{i:04}.NEF")))
        .collect();
    let catalog = fx.catalog.clone();
    let calls: std::rc::Rc<std::cell::RefCell<Vec<Call>>> = Default::default();
    let unsynced: std::rc::Rc<std::cell::RefCell<Vec<String>>> = Default::default();
    let check = {
        let (calls, unsynced) = (calls.clone(), unsynced.clone());
        move || {
            let db = rusqlite::Connection::open(&catalog).unwrap();
            let mut moved = db
                .prepare(
                    "SELECT source, destination FROM file_operation_members WHERE state='moved'",
                )
                .unwrap();
            let moved = moved
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .map(Result::unwrap);
            let calls = calls.borrow();
            for (from, to) in moved {
                let (from, to) = (std::path::PathBuf::from(from), std::path::PathBuf::from(to));
                let renamed = calls
                    .iter()
                    .position(|c| *c == Call::Rename(from.clone(), to.clone()));
                let synced = |dir: &std::path::Path| {
                    renamed.is_some_and(|r| {
                        calls[r + 1..]
                            .iter()
                            .any(|c| *c == Call::Sync(dir.to_path_buf()))
                    })
                };
                if !(synced(from.parent().unwrap()) && synced(to.parent().unwrap())) {
                    unsynced.borrow_mut().push(format!(
                        "{} after {} calls",
                        to.display(),
                        calls.len()
                    ));
                }
            }
        }
    };
    let watched = check.clone();
    let recorded = calls;
    let disk = Faulty {
        watch: Some(Box::new(move |call: &Call| {
            watched();
            recorded.borrow_mut().push(call.clone());
        })),
        ..Faulty::default()
    };
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos,
            root: fx.root_id(),
            to: "sub".into(),
        },
        &NoTrash,
        &disk,
    );
    check();
    assert!(
        outcome
            .items
            .iter()
            .all(|i| matches!(i.result, ItemResult::Moved { .. })),
        "{outcome:?}"
    );
    assert!(unsynced.borrow().is_empty(), "{:?}", unsynced.borrow());
}
