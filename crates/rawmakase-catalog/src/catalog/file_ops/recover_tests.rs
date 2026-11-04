//! Faults at every disk step and every journal and catalog commit, then a
//! restart and recovery; and each recovery rule on its own.
//!
//! After any of them: every original's bytes are at exactly one place, its
//! source or where the operation put it (its destination, the Trash);
//! nothing is overwritten; every catalog row is where its file is proven to
//! be, or as it was; and every item is `done`, `no_change` or listed as
//! unfinished.
use super::journal::{self, ItemState, Kind, MemberState, NewMember};
use super::preconditions::Preconditions;
use super::tests::{FakeTrash, Fault, Faulty, Fx, NoTrash, Op, bytes, crashes, fixture, results};
use super::*;
use crate::catalog::Catalog;
use crate::fsops;
use crate::storage::Identity;
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::collections::{BTreeMap, HashSet};

/// The files and rows a scenario starts from.
pub(super) struct Before {
    pub files: BTreeMap<String, Vec<u8>>,
    pub rows: BTreeMap<PhotoId, String>,
}

pub(super) fn before(fx: &Fx) -> Before {
    Before {
        files: fx.files(),
        rows: fx.rows(),
    }
}

/// The invariants, after a restart and recovery.
pub(super) fn check(fx: &Fx, before: &Before, recovery: &Recovery, batch: &BatchId) {
    check_files(fx, before, recovery, batch);
    let unfinished: HashSet<PhotoId> = recovery
        .unfinished
        .iter()
        .flat_map(|u| u.photos.iter().copied())
        .collect();
    for item in super::outcome(&fx.cat, batch).unwrap().items {
        match item.result {
            ItemResult::NeedsRecovery(_) => {
                assert!(item.photos.iter().all(|p| unfinished.contains(p)));
            }
            ItemResult::Interrupted | ItemResult::Kept => panic!("{item:?} left undecided"),
            _ => {}
        }
    }
}

/// The files and rows invariants alone, which hold whatever the user
/// decided: every original once, and every row where its file is proven.
pub(super) fn check_files(fx: &Fx, before: &Before, recovery: &Recovery, batch: &BatchId) {
    let removed = journal::of_batch(&fx.cat.db, &batch.0)
        .unwrap()
        .iter()
        .any(|i| i.kind == Kind::RemoveFromCatalog);
    let copies = journal::of_batch(&fx.cat.db, &batch.0)
        .unwrap()
        .iter()
        .any(|i| i.copies());
    // Every original once, nothing new, nothing overwritten.
    let mut was: Vec<&Vec<u8>> = before.files.values().collect();
    let now = fx.files();
    let mut is: Vec<&Vec<u8>> = now
        .iter()
        // A partial copy, which only an unfinished item may leave, is
        // looked at on its own.
        .filter(|(k, _)| !k.ends_with(".rawmakase-part"))
        .map(|(_, v)| v)
        .collect();
    was.sort();
    is.sort();
    if copies {
        // A move to another volume may also leave copies of originals: in
        // the Trash, or beside an original it couldn't put there; and, for
        // an unfinished item, part of one, at its name on a share (#675).
        let mut extra = is.clone();
        for w in &was {
            let at = extra.iter().position(|v| v == w);
            let at = at.unwrap_or_else(|| panic!("a file was lost: {:?}", now.keys()));
            extra.remove(at);
        }
        for e in extra {
            let part = !recovery.unfinished.is_empty() && was.iter().any(|w| w.starts_with(e));
            assert!(was.contains(&e) || part, "a new file: {:?}", now.keys());
        }
    } else {
        assert_eq!(was, is, "files: {:?}", now.keys().collect::<Vec<_>>());
    }
    let unfinished: HashSet<PhotoId> = recovery
        .unfinished
        .iter()
        .flat_map(|u| u.photos.iter().copied())
        .collect();
    let rows = fx.rows();
    // A row's file as `fx.files` names it: below the root, or, in another
    // root beside it, by its absolute path, compared with `/` for `\` and
    // without the `\\?\` prefix Windows' canonical paths have.
    let plain = |p: String| {
        let p = p.replace('\\', "/");
        p.strip_prefix("//?/").map(str::to_string).unwrap_or(p)
    };
    let base = plain(
        fx.dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    );
    let file = |row: &str| match plain(row.to_string())
        .strip_prefix(&base)
        .and_then(|rel| rel.strip_prefix('/'))
    {
        Some(rel) => rel.to_string(),
        None => format!("trip/{row}"),
    };
    for (id, path) in &before.rows {
        // A photo whose file was already missing is never touched.
        let Some(original) = before.files.get(&file(path)) else {
            assert_eq!(rows.get(id), Some(path), "{id:?} changed without its file");
            continue;
        };
        match rows.get(id) {
            // Rows follow a file proven where they say.
            Some(now_at) if now_at != path => {
                assert_eq!(now.get(&file(now_at)), Some(original), "{id:?}");
            }
            // Unchanged rows: their file is there, or the photo is listed.
            Some(_) => {
                if now.get(&file(path)) != Some(original) {
                    assert!(unfinished.contains(id), "{id:?} lost its file unreported");
                }
            }
            // Gone rows: removed from the catalog alone, or their file
            // went to the Trash.
            None if removed => {}
            None => {
                assert!(
                    now.iter()
                        .any(|(k, v)| k.starts_with("trash/") && v == original),
                    "{id:?} removed with its file not in the Trash"
                );
            }
        }
    }
}

/// Plans `op`, runs it on `disk` (which may crash), restarts and recovers.
fn scenario(fx: &mut Fx, op: FileOp, trash: &dyn Trash, disk: &Faulty) -> (BatchId, Recovery) {
    let plan = fx.plan(op);
    let batch = plan.batch().clone();
    // Whether it crashes or not, a restart follows.
    // The outcome is read from the journal after the restart.
    crashes(|| {
        drop(
            execute::execute_with(&mut fx.cat, plan.confirm(), trash, Cancel::NEVER, disk).unwrap(),
        );
    });
    fx.reopen();
    let recovery = fx.recover(&disk::Real);
    (batch, recovery)
}

/// [`scenario`], planned on `disk` and recovered with `trash` on a disk
/// that takes `far` for another volume's, as `disk` does.
fn scenario_on(
    fx: &mut Fx,
    op: FileOp,
    trash: &dyn Trash,
    disk: &Faulty,
    far: &std::path::Path,
) -> (BatchId, Recovery) {
    let plan = fx.plan_on(op, disk).unwrap();
    let batch = plan.batch().clone();
    crashes(|| {
        drop(
            execute::execute_with(&mut fx.cat, plan.confirm(), trash, Cancel::NEVER, disk).unwrap(),
        );
    });
    fx.reopen();
    let clean = Faulty::default().elsewhere(far);
    let recovery = fx.recover_with(&clean, trash);
    (batch, recovery)
}

fn move_files() -> Fx {
    fixture(&["a.NEF", "a.xmp", "b.NEF", "sub/"])
}

fn move_op(fx: &Fx) -> FileOp {
    FileOp::MovePhotos {
        photos: vec![fx.photo("a.NEF"), fx.photo("b.NEF")],
        root: fx.root_id(),
        to: "sub".into(),
    }
}

#[test]
fn every_fault_at_every_disk_step_of_a_move_ends_proven() {
    // A run without faults says how many steps there are.
    let mut clean = move_files();
    let counting = Faulty::default();
    let op = move_op(&clean);
    let _ = clean.run(op, &NoTrash, &counting);
    let steps = [
        (Op::Rename, counting.count(Op::Rename)),
        (Op::Sync, counting.count(Op::Sync)),
    ];
    assert_eq!(steps[0].1, 3);
    // Both folders synced once for each round of renames: the photos, then
    // the sidecar (#692).
    assert_eq!(steps[1].1, 4);
    let mut runs = 0;
    for (at, count) in steps {
        for nth in 0..count {
            for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                let mut fx = move_files();
                let was = before(&fx);
                let disk = Faulty::at(at, nth, fault);
                let op = move_op(&fx);
                let (batch, recovery) = scenario(&mut fx, op, &NoTrash, &disk);
                check(&fx, &was, &recovery, &batch);
                // And whatever is unfinished can be put back.
                for u in &recovery.unfinished {
                    let back =
                        resolve(&mut fx.cat, u.operation, Resolution::PutBack, &NoTrash).unwrap();
                    assert!(
                        matches!(back.result, ItemResult::Unchanged(_)),
                        "{at:?} {nth} {fault:?}: {back:?}"
                    );
                }
                let after = Recovery::default();
                check(&fx, &was, &after, &batch);
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 3 * (3 + 4));
}

/// Photos of two folders, some with sidecars, moved together in one batch
/// (#692): its renames and syncs go round by round.
fn batch_files() -> Fx {
    fixture(&[
        "a.NEF",
        "a.xmp",
        "b.NEF",
        "c.NEF",
        "c.xmp",
        "day/e.NEF",
        "day/e.xmp",
        "sub/",
    ])
}

fn batch_op(fx: &Fx) -> FileOp {
    FileOp::MovePhotos {
        photos: ["a.NEF", "b.NEF", "c.NEF", "day/e.NEF"]
            .map(|p| fx.photo(p))
            .to_vec(),
        root: fx.root_id(),
        to: "sub".into(),
    }
}

#[test]
fn every_fault_at_every_disk_step_of_a_batch_ends_proven() {
    let mut clean = batch_files();
    let counting = Faulty::default();
    let op = batch_op(&clean);
    let outcome = clean.run(op, &NoTrash, &counting);
    let sub = clean.folder("sub");
    assert!(
        results(&outcome)
            .iter()
            .all(|r| *r == ItemResult::Moved { to: sub }),
        "{outcome:?}"
    );
    let steps = [
        (Op::Rename, counting.count(Op::Rename)),
        (Op::Sync, counting.count(Op::Sync)),
    ];
    assert_eq!(steps[0].1, 7);
    // Its three folders synced once for each round: the photos, then the
    // sidecars.
    assert_eq!(steps[1].1, 6);
    let mut runs = 0;
    for (at, count) in steps {
        for nth in 0..count {
            for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                let mut fx = batch_files();
                let was = before(&fx);
                let disk = Faulty::at(at, nth, fault);
                let op = batch_op(&fx);
                let (batch, recovery) = scenario(&mut fx, op, &NoTrash, &disk);
                check(&fx, &was, &recovery, &batch);
                // A step that fails is put back, the run going on: a
                // rename's item alone, a folder's sync every item renamed
                // into or out of it in that round; the others are moved.
                if fault == Fault::Fail {
                    let outcome = super::outcome(&fx.cat, &batch).unwrap();
                    let results = results(&outcome);
                    let moved = results
                        .iter()
                        .filter(|r| **r == ItemResult::Moved { to: sub })
                        .count();
                    let back = results
                        .iter()
                        .filter(|r| matches!(r, ItemResult::Unchanged(Some(_))))
                        .count();
                    assert_eq!(moved + back, 4, "{at:?} {nth}: {outcome:?}");
                    if at == Op::Rename {
                        assert_eq!(moved, 3, "{at:?} {nth}: {outcome:?}");
                    }
                }
                for u in &recovery.unfinished {
                    let back =
                        resolve(&mut fx.cat, u.operation, Resolution::PutBack, &NoTrash).unwrap();
                    assert!(
                        matches!(back.result, ItemResult::Unchanged(_)),
                        "{at:?} {nth} {fault:?}: {back:?}"
                    );
                }
                check(&fx, &was, &Recovery::default(), &batch);
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 3 * (7 + 6));
}

#[test]
fn every_fault_at_every_step_of_a_trash_ends_proven() {
    let files = || fixture(&["a.NEF", "a.xmp", "b.NEF"]);
    let op = |fx: &Fx| FileOp::TrashPhotos {
        photos: vec![fx.photo("a.NEF"), fx.photo("b.NEF")],
    };
    let mut clean = files();
    let counting = Faulty::default();
    let trash = FakeTrash::new(&clean);
    let o = op(&clean);
    let _ = clean.run(o, &trash, &counting);
    let syncs = counting.count(Op::Sync);
    assert_eq!(*trash.calls.borrow(), 3);
    // The Trash refusing or the process ending at each file, and each sync
    // failing or crashing.
    // The Trash failing from, or crashing after, a file; a disk fault.
    type Case = (Option<usize>, Option<usize>, Option<(Op, usize, Fault)>);
    let mut cases: Vec<Case> = Vec::new();
    for n in 1..=3 {
        cases.push((Some(n), None, None));
        cases.push((None, Some(n), None));
    }
    for nth in 0..syncs {
        for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
            cases.push((None, None, Some((Op::Sync, nth, fault))));
        }
    }
    for (fail_from, crash_after, fault) in cases {
        let mut fx = files();
        let was = before(&fx);
        let mut trash = FakeTrash::new(&fx);
        trash.fail_from = fail_from;
        trash.crash_after = crash_after;
        let disk = fault.map_or_else(Faulty::default, |(o, n, f)| Faulty::at(o, n, f));
        let o = op(&fx);
        let (batch, recovery) = scenario(&mut fx, o, &trash, &disk);
        check(&fx, &was, &recovery, &batch);
    }
}

#[test]
fn every_failed_commit_ends_proven_and_a_restart_finishes_it() -> Result<()> {
    for when in [
        // Journalling an item: the run stops, nothing moved.
        "BEFORE INSERT ON file_operations",
        // A member's step: it is put back.
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'moved'",
        // The catalog's change, or marking it done: unfinished, then
        // finished by recovery once the catalog can be written.
        "BEFORE UPDATE ON photos",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'",
    ] {
        let mut fx = move_files();
        let was = before(&fx);
        fx.fail_journal(when);
        let op = move_op(&fx);
        let plan = fx.plan(op);
        let batch = plan.batch().clone();
        let outcome = execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &NoTrash,
            Cancel::NEVER,
            &Faulty::default(),
        )?;
        let first = outcome.items.first().map(|i| i.result.clone());
        match when {
            w if w.contains("INSERT") => {
                assert!(outcome.stopped.is_some());
                assert!(outcome.items.is_empty());
            }
            w if w.contains("members") => {
                assert!(
                    matches!(first, Some(ItemResult::Unchanged(Some(_)))),
                    "{first:?}"
                );
            }
            _ => assert!(
                matches!(first, Some(ItemResult::NeedsRecovery(_))),
                "{first:?}"
            ),
        }
        // A restart: the trigger is gone with the connection.
        fx.reopen();
        let recovery = fx.recover(&disk::Real);
        assert!(recovery.unfinished.is_empty(), "{when}: {recovery:?}");
        check(&fx, &was, &recovery, &batch);
    }
    Ok(())
}

#[test]
fn the_journal_moves_past_a_step_only_once_its_folders_are_synced() -> Result<()> {
    // Crash at the first sync after the photo's rename: the journal still
    // has the member planned, as the step isn't durable yet.
    let mut fx = move_files();
    let disk = Faulty::at(Op::Sync, 0, Fault::CrashBefore);
    let op = move_op(&fx);
    let plan = fx.plan(op);
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &NoTrash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    let items = journal::of_batch(&fx.cat.db, &batch.0)?;
    let first = &items[0].members[0];
    assert_eq!(first.state, MemberState::Planned);
    assert!(
        first.destination.as_ref().is_some_and(|d| d.is_file()),
        "the rename itself happened"
    );
    // Every rename's folders are synced before the journal records it,
    // whichever disk step the run ends at: a member journalled moved was
    // renamed, then both its folders synced. Folders of several renames are
    // synced once for all of them (#692), so it isn't the next step.
    let counting = Faulty::default();
    let mut fx = move_files();
    let op = move_op(&fx);
    let _ = fx.run(op, &NoTrash, &counting);
    let mut checked = 0;
    for op in [Op::Rename, Op::Sync] {
        // Each step of the kind, then none: the run ends whole.
        for nth in 0..=counting.count(op) {
            let mut fx = move_files();
            let disk = Faulty::at(op, nth, Fault::CrashBefore);
            let plan = fx.plan(move_op(&fx));
            let batch = plan.batch().clone();
            let crashed = crashes(|| {
                let _ = execute::execute_with(
                    &mut fx.cat,
                    plan.confirm(),
                    &NoTrash,
                    Cancel::NEVER,
                    &disk,
                );
            });
            fx.reopen();
            let calls = disk.calls.borrow();
            // The step it crashed before didn't happen.
            let done = &calls[..calls.len() - usize::from(crashed)];
            for item in journal::of_batch(&fx.cat.db, &batch.0)? {
                for m in item
                    .members
                    .iter()
                    .filter(|m| m.state == MemberState::Moved)
                {
                    let to = m.destination.clone().expect("a destination");
                    let rename = tests::Call::Rename(m.source.clone(), to.clone());
                    let renamed = done.iter().position(|c| *c == rename);
                    let renamed = renamed.unwrap_or_else(|| panic!("{op:?} {nth}: {m:?}"));
                    for dir in [m.source.parent().unwrap(), to.parent().unwrap()] {
                        let sync = tests::Call::Sync(dir.to_path_buf());
                        assert!(
                            done[renamed + 1..].contains(&sync),
                            "{op:?} {nth}: {m:?} journalled before {dir:?} was synced"
                        );
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 0);
    Ok(())
}

/// Moves a.NEF (with a.xmp) to sub, leaving the item unfinished with both
/// files at their destinations: its completion failed.
fn moved_but_unfinished() -> (Fx, BatchId) {
    let mut fx = move_files();
    fx.fail_journal("BEFORE UPDATE ON photos");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![fx.photo("a.NEF")],
        root: fx.root_id(),
        to: "sub".into(),
    });
    let batch = plan.batch().clone();
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )
    .unwrap();
    assert!(matches!(
        results(&outcome)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    fx.reopen();
    (fx, batch)
}

#[test]
fn a_finished_move_whose_rows_did_not_follow_is_completed() -> Result<()> {
    let (mut fx, batch) = moved_but_unfinished();
    let a = fx.photo("a.NEF");
    let recovery = fx.recover(&disk::Real);
    assert_eq!(
        results(&super::outcome(&fx.cat, &batch)?),
        [ItemResult::Moved {
            to: fx.folder("sub")
        }]
    );
    assert!(recovery.unfinished.is_empty());
    assert_eq!(fx.rows()[&a], "sub/a.NEF");
    Ok(())
}

#[test]
fn a_family_changed_before_recovery_is_reported_and_left() -> Result<()> {
    let (mut fx, batch) = moved_but_unfinished();
    let a = fx.photo("a.NEF");
    // Another connection edits it, or removes and adds it again.
    let mut other = Catalog::open(&fx.catalog)?;
    other.set_metadata(a, 2, 0, "")?;
    let recovery = fx.recover(&disk::Real);
    assert_eq!(recovery.unfinished.len(), 1);
    assert_eq!(recovery.unfinished[0].reason, preconditions::CHANGED);
    assert_eq!(fx.rows()[&a], "a.NEF", "rows untouched");
    assert!(fx.root.join("sub/a.NEF").is_file(), "files untouched");
    // Put Back checks the catalog first too, and leaves it.
    let back = resolve(
        &mut fx.cat,
        recovery.unfinished[0].operation,
        Resolution::PutBack,
        &NoTrash,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(matches!(back.result, ItemResult::NeedsRecovery(_)));
    assert!(fx.root.join("sub/a.NEF").is_file());
    // Keep as Is is for good.
    let kept = resolve(&mut fx.cat, back.operation, Resolution::Keep, &NoTrash)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(kept.result, ItemResult::Kept);
    assert!(fx.recover(&disk::Real).unfinished.is_empty());
    assert_eq!(
        results(&super::outcome(&fx.cat, &batch)?),
        [ItemResult::Kept]
    );
    Ok(())
}

#[test]
fn an_offline_destination_leaves_it_unfinished_and_kept_is_never_completed() -> Result<()> {
    let (mut fx, _) = moved_but_unfinished();
    let a = fx.photo("a.NEF");
    // The volume goes away: here, its folder.
    let away = fx.dir.path().join("away");
    std::fs::rename(fx.root.join("sub"), &away)?;
    let recovery = fx.recover(&disk::Real);
    assert_eq!(recovery.unfinished.len(), 1);
    let kept = resolve(
        &mut fx.cat,
        recovery.unfinished[0].operation,
        Resolution::Keep,
        &NoTrash,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(kept.result, ItemResult::Kept);
    // It comes back; the kept item stays as it is.
    std::fs::rename(&away, fx.root.join("sub"))?;
    assert!(fx.recover(&disk::Real).finished.is_empty());
    assert_eq!(fx.rows()[&a], "a.NEF");
    Ok(())
}

#[test]
fn a_put_back_resumes_after_a_crash_and_never_completes_forward() -> Result<()> {
    // The companion's rename fails, the item is restoring, and the process
    // ends before (then, separately, after) the photo is renamed back.
    for fault in [Fault::CrashBefore, Fault::CrashAfter] {
        let mut fx = move_files();
        let was = before(&fx);
        let disk = Faulty::with(&[(Op::Rename, 1, Fault::Fail), (Op::Rename, 2, fault)]);
        let a = fx.photo("a.NEF");
        let plan = fx.plan(FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_id(),
            to: "sub".into(),
        });
        let batch = plan.batch().clone();
        assert!(crashes(|| {
            let _ =
                execute::execute_with(&mut fx.cat, plan.confirm(), &NoTrash, Cancel::NEVER, &disk);
        }));
        fx.reopen();
        let items = journal::of_batch(&fx.cat.db, &batch.0)?;
        assert_eq!(items[0].state, ItemState::Restoring);
        let recovery = fx.recover(&disk::Real);
        assert_eq!(
            results(&super::outcome(&fx.cat, &batch)?),
            [ItemResult::Unchanged(Some("injected failure".into()))]
        );
        check(&fx, &was, &recovery, &batch);
        assert!(fx.root.join("a.NEF").is_file(), "{fault:?}");
        assert_eq!(fx.rows()[&a], "a.NEF");
    }
    Ok(())
}

#[test]
fn a_put_back_after_another_release_synchronized_the_photo_renames_nothing() -> Result<()> {
    let mut fx = move_files();
    let disk = Faulty::with(&[
        (Op::Rename, 1, Fault::Fail),
        (Op::Rename, 2, Fault::CrashBefore),
    ]);
    let a = fx.photo("a.NEF");
    let sub = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(sub),
        to: fx.logical(sub),
    });
    assert!(crashes(|| {
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &NoTrash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    // An older release's Synchronize Folder found the photo in sub.
    fx.cat
        .db_for_tests()
        .execute("UPDATE photos SET folder=?1 WHERE id=?2", [sub.0, a.0])?;
    let recovery = fx.recover(&disk::Real);
    assert_eq!(recovery.unfinished.len(), 1);
    assert!(fx.root.join("sub/a.NEF").is_file(), "not renamed back");
    assert_eq!(fx.rows()[&a], "sub/a.NEF");
    Ok(())
}

/// Trashes a.NEF (with a.xmp), leaving the item unfinished after both went
/// to the Trash: its completion failed.
fn trashed_but_unfinished() -> (Fx, BatchId) {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    fx.fail_journal("BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'");
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan(FileOp::TrashPhotos {
        photos: vec![fx.photo("a.NEF")],
    });
    let batch = plan.batch().clone();
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &trash,
        Cancel::NEVER,
        &Faulty::default(),
    )
    .unwrap();
    assert!(matches!(
        results(&outcome)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    fx.reopen();
    (fx, batch)
}

#[test]
fn a_trash_with_receipts_and_its_files_gone_is_completed() -> Result<()> {
    let (mut fx, batch) = trashed_but_unfinished();
    let recovery = fx.recover(&disk::Real);
    assert!(recovery.unfinished.is_empty());
    assert_eq!(
        results(&super::outcome(&fx.cat, &batch)?),
        [ItemResult::Trashed]
    );
    assert!(fx.rows().is_empty());
    Ok(())
}

#[test]
fn a_file_put_back_from_the_trash_before_recovery_keeps_its_rows() -> Result<()> {
    let (mut fx, batch) = trashed_but_unfinished();
    let a = fx.photo("a.NEF");
    let trash = fx.dir.path().canonicalize()?.join("trash");
    std::fs::rename(trash.join("1-a.NEF"), fx.root.join("a.NEF"))?;
    std::fs::rename(trash.join("2-a.xmp"), fx.root.join("a.xmp"))?;
    let recovery = fx.recover(&disk::Real);
    assert!(recovery.unfinished.is_empty());
    assert!(matches!(
        results(&super::outcome(&fx.cat, &batch)?)[..],
        [ItemResult::Unchanged(_)]
    ));
    assert!(fx.rows().contains_key(&a));
    Ok(())
}

#[test]
fn a_file_trashed_without_a_receipt_keeps_its_rows() -> Result<()> {
    let mut fx = fixture(&["a.NEF"]);
    let was = before(&fx);
    let mut trash = FakeTrash::new(&fx);
    trash.crash_after = Some(1);
    let a = fx.photo("a.NEF");
    let (batch, recovery) = scenario(
        &mut fx,
        FileOp::TrashPhotos { photos: vec![a] },
        &trash,
        &Faulty::default(),
    );
    assert_eq!(recovery.unfinished.len(), 1);
    assert_eq!(recovery.unfinished[0].reason, "it may be in the Trash");
    assert!(fx.rows().contains_key(&a));
    check(&fx, &was, &recovery, &batch);
    Ok(())
}

#[test]
fn another_computers_items_are_not_this_ones() -> Result<()> {
    let (fx, batch) = moved_but_unfinished();
    let other = crate::catalog::locations::Computer {
        id: "another-computer".into(),
        name: "Other".into(),
    };
    let mut there = Catalog::open_as(&fx.catalog, &other)?;
    let recovery = recover(&mut there, &NoTrash).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(recovery, Recovery::default());
    assert!(matches!(
        results(&super::outcome(&fx.cat, &batch)?)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    Ok(())
}

#[test]
fn where_a_member_is_is_decided_by_the_exact_name_in_its_folder() -> Result<()> {
    // A case-only rename, journalled as a move within the folder: on a
    // volume that ignores case, looking `ROUND.NEF` up would find
    // `round.NEF`; the listing tells them apart.
    for renamed in [false, true] {
        let mut fx = fixture(&["ROUND.NEF"]);
        let a = fx.photo("ROUND.NEF");
        let (from, to) = (fx.root.join("ROUND.NEF"), fx.root.join("round.NEF"));
        let mut pre = Preconditions::read(&fx.cat.db, a, vec![a])?.unwrap();
        pre.folder_path = Some(fx.root.clone());
        pre.to = Some((fx.folder(""), fx.root.clone()));
        let member = NewMember {
            source: from.clone(),
            destination: Some(to.clone()),
            id: fsops::entry(&from)?.id,
            identity: Identity::read(&from)?,
        };
        let computer = fx.cat.computer.id.clone();
        fx.cat.db.write(|w| {
            journal::insert(
                w,
                "case",
                &computer,
                Kind::MovePhotos,
                ItemState::Planned,
                &pre,
                &[member],
                None,
            )
            .map(drop)
        })?;
        if renamed {
            fsops::rename_exclusive(&from, &to)?;
        }
        let recovery = fx.recover(&disk::Real);
        assert!(recovery.unfinished.is_empty());
        let names = fsops::list(&fx.root)?;
        if renamed {
            assert_eq!(
                recovery.finished[0].result,
                ItemResult::Moved { to: fx.folder("") }
            );
            assert_eq!(fx.rows()[&a], "round.NEF");
            assert_eq!(names, ["round.NEF"]);
        } else {
            assert_eq!(recovery.finished[0].result, ItemResult::Unchanged(None));
            assert_eq!(fx.rows()[&a], "ROUND.NEF");
            assert_eq!(names, ["ROUND.NEF"]);
        }
    }
    Ok(())
}

/// A small deterministic generator, so a failing sequence can be replayed.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }
}

#[test]
fn seeded_random_operations_with_faults_keep_every_invariant() {
    let mut copied = 0;
    for seed in 0..40u64 {
        let mut rng = Lcg(seed);
        let mut fx = fixture(&[
            "a.NEF",
            "a.xmp",
            "b.NEF",
            "b.NEF.xmp",
            "c.JPG",
            "one/",
            "two/",
            "far/",
        ]);
        // `far/` is taken for another volume's: moves there copy.
        let far = fx.root.join("far");
        for step in 0..3 {
            let was = before(&fx);
            let photos: Vec<PhotoId> = fx
                .cat
                .photos()
                .unwrap()
                .into_iter()
                .filter(|p| p.master.is_none())
                // Planning reads a file's own volume from the disk itself.
                .filter(|p| !p.path.starts_with(&far))
                .filter(|_| rng.next(2) == 0)
                .map(|p| p.id)
                .collect();
            let op = match rng.next(6) {
                0 => FileOp::TrashPhotos { photos },
                5 => FileOp::MovePhotos {
                    photos,
                    root: fx.root_id(),
                    to: "far".into(),
                },
                1 => FileOp::RemoveFromCatalog { photos },
                2 => FileOp::MovePhotos {
                    photos,
                    root: fx.root_id(),
                    to: "one".into(),
                },
                // Names that sometimes clash, chain or are the photo's own.
                3 => FileOp::RenamePhotos {
                    renames: photos
                        .into_iter()
                        .map(|p| (p, ["a", "b", "x", "y", "B"][rng.next(5)].to_string()))
                        .collect(),
                },
                _ => FileOp::MovePhotos {
                    photos,
                    root: fx.root_id(),
                    to: "two".into(),
                },
            };
            let fault = [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter][rng.next(3)];
            let fault = [fault, Fault::CrashMid][rng.next(5) / 4];
            let kinds = [
                Op::Rename,
                Op::Sync,
                Op::CreateFile,
                Op::Copy,
                Op::SyncFile,
                Op::CopyTimes,
                Op::Hash,
                Op::RemoveFile,
            ];
            let kind = kinds[rng.next(kinds.len())];
            let disk = Faulty::at(kind, rng.next(6), fault).elsewhere(&far);
            let mut trash = FakeTrash::new(&fx);
            match rng.next(3) {
                0 => trash.fail_from = Some(1 + rng.next(4)),
                1 => trash.crash_after = Some(1 + rng.next(4)),
                _ => {}
            }
            // Earlier steps' Trash names must not clash.
            *trash.calls.borrow_mut() = 100 * step;
            if let Some(n) = &mut trash.fail_from {
                *n += 100 * step;
            }
            if let Some(n) = &mut trash.crash_after {
                *n += 100 * step;
            }
            let (batch, recovery) = scenario_on(&mut fx, op, &trash, &disk, &far);
            check(&fx, &was, &recovery, &batch);
            copied += journal::of_batch(&fx.cat.db, &batch.0)
                .unwrap()
                .iter()
                .filter(|i| i.copies() && i.members.iter().any(|m| m.state != MemberState::Planned))
                .count();
            let clean = Faulty::default().elsewhere(&far);
            let back = FakeTrash::new(&fx);
            *back.calls.borrow_mut() = 100 * step + 50;
            for u in &recovery.unfinished {
                recover::resolve_with(&mut fx.cat, u.operation, Resolution::PutBack, &clean, &back)
                    .unwrap();
            }
            // What Put Back left is still listed.
            let after = fx.recover_with(&clean, &back);
            check_files(&fx, &was, &after, &batch);
        }
    }
    assert!(copied >= 10, "only {copied} copies began");
}

#[test]
fn bytes_differ_per_file() {
    assert_ne!(bytes("a.NEF"), bytes("b.NEF"));
}

#[test]
fn a_put_back_that_fails_stays_a_put_back() -> Result<()> {
    let (mut fx, batch) = moved_but_unfinished();
    let a = fx.photo("a.NEF");
    // The user puts it back, but something now has the photo's name at
    // its source.
    std::fs::write(fx.root.join("a.NEF"), b"in the way")?;
    let id = journal::of_batch(&fx.cat.db, &batch.0)?[0].id;
    fx.cat
        .db
        .write(|w| journal::set_state(w, id, ItemState::NeedsRecovery, Some("left")))?;
    let back = resolve(&mut fx.cat, id, Resolution::PutBack, &NoTrash)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(
        matches!(back.result, ItemResult::NeedsRecovery(_)),
        "{back:?}"
    );
    // Once the name is free, recovery finishes the put back, never the move.
    std::fs::remove_file(fx.root.join("a.NEF"))?;
    let recovery = fx.recover(&disk::Real);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    assert!(matches!(
        results(&super::outcome(&fx.cat, &batch)?)[..],
        [ItemResult::Unchanged(_)]
    ));
    assert_eq!(std::fs::read(fx.root.join("a.NEF"))?, bytes("a.NEF"));
    assert_eq!(fx.rows()[&a], "a.NEF");
    Ok(())
}

#[test]
fn a_family_shared_with_a_new_photo_before_completion_is_left_unfinished() -> Result<()> {
    let mut fx = move_files();
    let a = fx.photo("a.NEF");
    let folder = fx.folder("");
    // Another connection adds a second photo of the same file while it moves.
    fx.fail_journal_with(&format!(
        "AFTER UPDATE ON file_operation_members WHEN NEW.state = 'moved' AND NEW.ordinal = 0
         BEGIN INSERT INTO photos(folder, filename, original_path) VALUES ({}, 'A.nef', 'x'); END;",
        folder.0
    ));
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_id(),
            to: "sub".into(),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(
        matches!(results(&outcome)[..], [ItemResult::NeedsRecovery(_)]),
        "{outcome:?}"
    );
    assert_eq!(fx.rows()[&a], "a.NEF");
    Ok(())
}

#[test]
fn another_computer_cannot_decide_this_ones_item() -> Result<()> {
    let (fx, batch) = moved_but_unfinished();
    let id = journal::of_batch(&fx.cat.db, &batch.0)?[0].id;
    let other = crate::catalog::locations::Computer {
        id: "another-computer".into(),
        name: "Other".into(),
    };
    let mut there = Catalog::open_as(&fx.catalog, &other)?;
    assert!(resolve(&mut there, id, Resolution::Keep, &NoTrash).is_err());
    assert!(matches!(
        results(&super::outcome(&fx.cat, &batch)?)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    Ok(())
}

#[test]
fn a_file_the_catalog_spells_otherwise_is_journalled_by_its_own_name() -> Result<()> {
    // The catalog says a.NEF; the folder holds A.NEF (Synchronize keeps the
    // catalog's spelling of a file a volume finds either way).
    let mut fx = fixture(&["A.NEF"]);
    let a = fx.photo("A.NEF");
    fx.cat
        .db_for_tests()
        .execute("UPDATE photos SET filename='a.NEF' WHERE id=?1", [a.0])?;
    let Ok(_) = fsops::entry(&fx.root.join("a.NEF")) else {
        return Ok(()); // a volume that tells them apart: the file is missing
    };
    fx.fail_journal("BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'");
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan(FileOp::TrashPhotos { photos: vec![a] });
    let batch = plan.batch().clone();
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &trash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert!(matches!(
        results(&outcome)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    let items = journal::of_batch(&fx.cat.db, &batch.0)?;
    assert!(
        items[0].members[0].source.ends_with("A.NEF"),
        "{:?}",
        items[0].members[0]
    );
    fx.reopen();
    // Put back from the Trash under either spelling: the rows stay.
    for name in ["A.NEF", "a.NEF"] {
        let trashed = fx.dir.path().canonicalize()?.join("trash/1-A.NEF");
        if !trashed.exists() {
            std::fs::rename(fx.root.join("A.NEF"), &trashed)
                .or_else(|_| std::fs::rename(fx.root.join("a.NEF"), &trashed))?;
        }
        std::fs::rename(&trashed, fx.root.join(name))?;
        let id = journal::of_batch(&fx.cat.db, &batch.0)?[0].id;
        fx.cat
            .db
            .write(|w| journal::set_state(w, id, ItemState::NeedsRecovery, None))?;
        fx.recover(&disk::Real);
        assert!(
            fx.rows().contains_key(&a),
            "rows removed with the file back as {name}"
        );
    }
    Ok(())
}

/// Whether this volume takes `a` and `b` for one file, beyond what
/// case-folding and NFC tell (APFS has `ß.NEF` and `SS.NEF` alike).
fn aliases(fx: &Fx, a: &str, b: &str) -> bool {
    let probe = fx.dir.path().join(a);
    std::fs::write(&probe, b"probe").unwrap();
    let alike = fsops::entry(&fx.dir.path().join(b)).is_ok();
    std::fs::remove_file(probe).unwrap();
    alike
}

#[test]
fn a_file_back_from_the_trash_under_an_alias_keeps_its_rows() -> Result<()> {
    let mut fx = fixture(&["\u{df}.NEF"]);
    if !aliases(&fx, "\u{df}x.NEF", "SSx.NEF") {
        return Ok(());
    }
    let a = fx.photo("\u{df}.NEF");
    fx.fail_journal("BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'");
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan(FileOp::TrashPhotos { photos: vec![a] });
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &trash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert!(matches!(
        results(&outcome)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    fx.reopen();
    let trashed = fx.dir.path().canonicalize()?.join("trash/1-\u{df}.NEF");
    std::fs::rename(&trashed, fx.root.join("SS.NEF"))?;
    fx.recover(&disk::Real);
    assert!(
        fx.rows().contains_key(&a),
        "rows removed with the file back as SS.NEF"
    );
    Ok(())
}

#[test]
fn only_a_crash_leaves_something_to_recover_when_a_catalog_opens() {
    let mut fx = move_files();
    assert!(!unfinished_here(&fx.cat).unwrap(), "nothing journalled yet");
    let _ = fx.run(move_op(&fx), &NoTrash, &disk::Real);
    assert!(
        !unfinished_here(&fx.cat).unwrap(),
        "finished items don't count"
    );
    let mut fx = move_files();
    let plan = fx.plan(move_op(&fx));
    let disk = Faulty::at(Op::Rename, 1, Fault::CrashAfter);
    crashes(|| {
        drop(
            execute::execute_with(&mut fx.cat, plan.confirm(), &NoTrash, Cancel::NEVER, &disk)
                .unwrap(),
        );
    });
    fx.reopen();
    assert!(unfinished_here(&fx.cat).unwrap());
    let recovery = fx.recover(&disk::Real);
    for u in &recovery.unfinished {
        resolve(&mut fx.cat, u.operation, Resolution::PutBack, &NoTrash).unwrap();
    }
    assert!(!unfinished_here(&fx.cat).unwrap());
}

#[test]
fn running_an_operation_forgets_finished_ones_older_than_30_days() {
    let mut fx = fixture(&["a.NEF", "b.NEF", "sub/"]);
    let old = fx.plan(FileOp::MovePhotos {
        photos: vec![fx.photo("a.NEF")],
        root: fx.root_id(),
        to: "sub".into(),
    });
    let old_batch = old.batch().clone();
    drop(
        execute::execute_with(
            &mut fx.cat,
            old.confirm(),
            &NoTrash,
            Cancel::NEVER,
            &disk::Real,
        )
        .unwrap(),
    );
    fx.cat
        .db_for_tests()
        .execute_batch("UPDATE file_operations SET created_at='2000-01-01 00:00:00'")
        .unwrap();
    let _ = fx.run(
        FileOp::MovePhotos {
            photos: vec![fx.photo("b.NEF")],
            root: fx.root_id(),
            to: "sub".into(),
        },
        &NoTrash,
        &disk::Real,
    );
    assert!(
        super::outcome(&fx.cat, &old_batch)
            .unwrap()
            .items
            .is_empty()
    );
}
