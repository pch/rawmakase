//! Moving a folder to another drive (#572 C): its whole tree copied into a
//! hidden folder there, every file verified, then named in one exclusive
//! rename; the catalog follows; the originals go to the Trash. Anything
//! that fails before the catalog follows is undone, and recovery decides
//! whatever a crash leaves at any step. On one volume, the test disk takes
//! `far/` for another volume's.
use super::copy_tests::{Far, far_as, put_back, removed_far, trash_as};
use super::recover_tests::{before, check};
use super::tests::{
    FakeTrash, Fault, Faulty, Fx, NoTrash, Op, bytes, crashes, fixture, results, tagged,
};
use super::*;
use crate::fsops;
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// `day/` holds a photo with its sidecar, a hidden file, an empty folder
/// and a folder with a photo; `far/` is a catalog folder on "another drive".
fn files() -> Fx {
    let fx = fixture(&[
        "day/a.NEF",
        "day/a.xmp",
        "day/.hidden",
        "day/sub/b.NEF",
        "far/",
    ]);
    std::fs::create_dir(fx.root.join("day/empty")).unwrap();
    fx
}

/// The least that has every kind of step: a hidden file, and a photo in a
/// folder of the folder.
fn small() -> Fx {
    fixture(&["day/a.NEF", "day/.hidden", "day/sub/b.NEF", "far/"])
}

fn far(fx: &Fx) -> Faulty {
    far_as(fx, Far::Ids)
}

fn to_far(fx: &Fx) -> FileOp {
    FileOp::MoveFolder {
        root: fx.root_id(),
        folder: "day".into(),
        to_root: fx.root_id(),
        parent: "far".into(),
    }
}

/// Plans `op` on `disk` and runs it, stopping when `cancel` is set.
fn run(fx: &mut Fx, op: FileOp, trash: &dyn Trash, disk: &Faulty, cancel: Cancel<'_>) -> Outcome {
    let plan = fx.plan_on(op, disk).unwrap();
    execute::execute_with(&mut fx.cat, plan.confirm(), trash, cancel, disk).unwrap()
}

/// Every name below `dir`, by path relative to it, with a file's bytes
/// (`None` for a folder).
fn tree_of(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        let Ok(names) = std::fs::read_dir(dir) else {
            return;
        };
        for e in names {
            let p = e.unwrap().path();
            // AppleDouble files the test disk keeps beside others on FAT
            // and exFAT are part of those.
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("._"))
            {
                continue;
            }
            let rel = p
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if p.is_dir() {
                out.insert(rel, None);
                walk(&p, base, out);
            } else {
                out.insert(rel, Some(std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// Partial copies or hidden folders a copy made, anywhere but the Trash.
fn partials(fx: &Fx) -> Vec<String> {
    tree_of(fx.dir.path())
        .into_keys()
        .filter(|k| k.contains(".rawmakase-part") && !k.starts_with("trash/"))
        .collect()
}

/// The logical path of catalog folder `folder`.
fn logical(fx: &Fx, folder: FolderId) -> String {
    fx.cat
        .db_for_tests()
        .query_row(
            "SELECT path FROM folder_paths WHERE folder=?1",
            [folder.0],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn a_folder_moves_to_another_drive_by_a_verified_copy_and_its_original_goes_to_the_trash()
-> Result<()> {
    let mut fx = files();
    let (a, b) = (fx.photo("day/a.NEF"), fx.photo("day/sub/b.NEF"));
    let (day, sub) = (fx.folder("day"), fx.folder("day/sub"));
    let tree = tree_of(&fx.root.join("day"));
    let modified = std::fs::metadata(fx.root.join("day/a.NEF"))?.modified()?;
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let across = plan.summary().across.clone().expect("it copies");
    assert_eq!(across.files, 4);
    let bytes_planned: u64 = ["day/a.NEF", "day/a.xmp", "day/.hidden", "day/sub/b.NEF"]
        .iter()
        .map(|f| bytes(f).len() as u64)
        .sum();
    assert_eq!(across.bytes, bytes_planned);
    let outcome = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert!(outcome.items[0].originals_kept.is_empty(), "{outcome:?}");
    assert_eq!(tree_of(&fx.root.join("far/day")), tree);
    assert!(!fx.root.join("day").exists());
    // The original went to the Trash whole, as one folder.
    assert_eq!(tree_of(&trash.dir.join("1-day")), tree);
    let rows = fx.rows();
    assert_eq!(rows[&a], "far/day/a.NEF");
    assert_eq!(rows[&b], "far/day/sub/b.NEF");
    assert_eq!(logical(&fx, day), "far/day");
    assert_eq!(logical(&fx, sub), "far/day/sub");
    assert_eq!(
        std::fs::metadata(fx.root.join("far/day/a.NEF"))?.modified()?,
        modified
    );
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    // Copied, never renamed across: the one rename is the hidden folder's.
    let renames = disk.renames();
    assert_eq!(renames.len(), 1, "{renames:?}");
    assert!(renames[0].0.to_string_lossy().ends_with(".rawmakase-part"));
    Ok(())
}

/// Sets folder `dir`'s modification time.
fn set_folder_time(dir: &Path, when: std::time::SystemTime) -> Result<()> {
    #[cfg(windows)]
    let folder = {
        use std::os::windows::fs::OpenOptionsExt as _;
        // FILE_WRITE_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS.
        std::fs::OpenOptions::new()
            .access_mode(0x100)
            .custom_flags(0x0200_0000)
            .open(dir)?
    };
    #[cfg(not(windows))]
    let folder = std::fs::File::open(dir)?;
    Ok(folder.set_modified(when)?)
}

#[test]
fn a_folders_extras_dates_and_permissions_go_with_it_to_another_drive() -> Result<()> {
    for kind in [Far::Ids, Far::Names, Far::Direct] {
        let mut fx = files();
        let day = fx.root.join("day");
        let tags = ["", "sub", "sub/b.NEF", "a.xmp"];
        let tagged_all = tags
            .iter()
            .all(|t| tagged(&day.join(t), "tag", t.as_bytes()));
        let extras: Vec<_> = match tagged_all {
            true => tags
                .iter()
                .map(|t| fsops::extras(&day.join(t)))
                .collect::<Result<_, _>>()?,
            false => Vec::new(),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(day.join("sub"), std::fs::Permissions::from_mode(0o750))?;
        }
        let when = |s| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s);
        for (dir, at) in [
            ("sub", 1_500_000_000),
            ("empty", 1_500_000_100),
            ("", 1_400_000_000),
        ] {
            set_folder_time(&day.join(dir), when(at))?;
        }
        let disk = far_as(&fx, kind);
        let trash = FakeTrash::new(&fx);
        let op = to_far(&fx);
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        assert_eq!(results(&outcome), [ItemResult::FolderMoved], "{kind:?}");
        assert!(outcome.items[0].originals_kept.is_empty(), "{outcome:?}");
        let copy = fx.root.join("far/day");
        for (dir, at) in [
            ("sub", 1_500_000_000),
            ("empty", 1_500_000_100),
            ("", 1_400_000_000),
        ] {
            let modified = std::fs::metadata(copy.join(dir))?.modified()?;
            assert_eq!(modified, when(at), "{kind:?} {dir}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(copy.join("sub"))?.permissions().mode();
            assert_eq!(mode & 0o777, 0o750, "{kind:?}");
        }
        if tagged_all {
            for (t, was) in tags.iter().zip(&extras) {
                assert_eq!(&fsops::extras(&copy.join(t))?, was, "{kind:?} {t}");
            }
        }
        assert!(!day.exists(), "{kind:?}");
    }
    Ok(())
}

#[test]
fn a_drive_that_can_t_keep_a_folders_extras_moves_nothing() -> Result<()> {
    let mut fx = files();
    let tree = tree_of(&fx.root.join("day"));
    let disk = Faulty {
        keeps_no_extras: true,
        ..far(&fx)
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert!(why.contains("can't keep"), "{why}");
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    assert!(!fx.root.join("far/day").exists());
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    Ok(())
}

#[test]
fn a_folder_tagged_after_the_catalog_follows_stays_and_its_files_go() -> Result<()> {
    let mut fx = files();
    let day = fx.root.join("day");
    // Read before it is copied, once copied, then before it would go.
    let disk = Faulty {
        retag: Some((day.clone(), 2)),
        ..far(&fx)
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    let kept = &outcome.items[0].originals_kept;
    let top = kept
        .iter()
        .find(|k| k.path == day)
        .expect("the folder stays");
    assert_eq!(top.why, super::copy::EXTRAS_DIFFER);
    assert!(!day.join("a.NEF").exists() && !day.join("sub/b.NEF").exists());
    assert_eq!(
        std::fs::read(fx.root.join("far/day/a.NEF"))?,
        bytes("day/a.NEF")
    );
    Ok(())
}

#[test]
fn a_file_tagged_after_the_catalog_follows_stays_with_its_folder() -> Result<()> {
    let mut fx = files();
    let a = fx.root.join("day/a.xmp");
    let disk = Faulty {
        retag: Some((a.clone(), 2)),
        ..far(&fx)
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    let kept = &outcome.items[0].originals_kept;
    assert_eq!(kept[0].path, a, "{kept:?}");
    assert_eq!(kept[0].why, super::copy::EXTRAS_DIFFER);
    assert_eq!(std::fs::read(&a)?, bytes("day/a.xmp"));
    assert!(!fx.root.join("day/a.NEF").exists());
    Ok(())
}

#[test]
fn recovery_keeps_the_date_a_folders_copy_got_before_its_originals_went() -> Result<()> {
    let mut fx = files();
    let day = fx.root.join("day");
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    set_folder_time(&day, when)?;
    // Not whole (a file tagged since), so its files go one by one, and the
    // process ends once the first is in the Trash, which dates its folder.
    let disk = Faulty {
        retag: Some((day.join("a.xmp"), 2)),
        ..far(&fx)
    };
    let mut trash = FakeTrash::new(&fx);
    trash.crash_after = Some(1);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            Cancel::NEVER,
            &disk,
        ));
    }));
    assert_eq!(
        std::fs::metadata(fx.root.join("far/day"))?.modified()?,
        when
    );
    fx.reopen();
    trash.crash_after = None;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    assert_eq!(
        std::fs::metadata(fx.root.join("far/day"))?.modified()?,
        when
    );
    Ok(())
}

#[test]
fn an_original_gone_before_recovery_keeps_only_its_own_folders_date_from_the_copy() -> Result<()> {
    let mut fx = files();
    let day = fx.root.join("day");
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    set_folder_time(&day.join("sub"), when)?;
    // The process ends once the catalog follows, before any folder there
    // is dated (after its four files' times).
    let disk = Faulty {
        faults: vec![(Op::CopyTimes, 4, Fault::CrashBefore)],
        ..far(&fx)
    };
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            Cancel::NEVER,
            &disk,
        ));
    }));
    // Then the user deletes one original in `day` itself.
    std::fs::remove_file(day.join("a.NEF"))?;
    fx.reopen();
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let copied = std::fs::metadata(fx.root.join("far/day/sub"))?.modified()?;
    assert_eq!(copied, when, "a folder whose files are all there is dated");
    Ok(())
}

/// The plan's refusal, as the user reads it.
fn refusal(fx: &Fx, disk: &Faulty) -> String {
    match fx.plan_on(to_far(fx), disk) {
        Err(e @ (FileOpError::Destination(_) | FileOpError::NoSpace { .. })) => e.to_string(),
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn a_folder_holding_what_can_t_be_copied_or_proven_is_refused() -> Result<()> {
    let fx = files();
    // Not enough room there.
    let disk = Faulty {
        space: Some(100),
        ..far(&fx)
    };
    assert!(
        refusal(&fx, &disk).contains("free"),
        "{}",
        refusal(&fx, &disk)
    );
    // A drive whose file ids can't be proven, either side.
    for unstable in [fx.root.join("far"), fx.root.clone()] {
        let disk = Faulty {
            unstable_ids: vec![unstable],
            ..far(&fx)
        };
        assert!(
            refusal(&fx, &disk).contains("this kind of drive"),
            "{}",
            refusal(&fx, &disk)
        );
    }
    // A file with another name elsewhere.
    std::fs::hard_link(fx.root.join("day/a.xmp"), fx.root.join("hard.xmp"))?;
    assert!(refusal(&fx, &far(&fx)).contains("another name"));
    std::fs::remove_file(fx.root.join("hard.xmp"))?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(fx.root.join("day/a.NEF"), fx.root.join("day/sub/link"))?;
        assert!(refusal(&fx, &far(&fx)).contains("link"));
    }
    assert!(fx.root.join("day/a.NEF").is_file());
    Ok(())
}

/// Plans the move, runs it on `disk` (which may crash) with `trash`,
/// restarts and recovers on the same volumes, with `far/` on a volume of
/// `kind`.
fn scenario_as(fx: &mut Fx, trash: &dyn Trash, disk: &Faulty, kind: Far) -> (BatchId, Recovery) {
    let plan = fx.plan_on(to_far(fx), disk).unwrap();
    let batch = plan.batch().clone();
    crashes(|| {
        drop(
            execute::execute_with(&mut fx.cat, plan.confirm(), trash, Cancel::NEVER, disk).unwrap(),
        );
    });
    fx.reopen();
    let clean = far_as(fx, kind);
    let recovery = fx.recover_with(&clean, trash);
    (batch, recovery)
}

/// After any fault and recovery: no original lost (`check`), and the
/// folder moved whole (copies there, originals in the Trash or reported) or
/// not at all (no copy left outside the Trash); what is unfinished is put
/// back in the end.
fn proven(
    fx: &mut Fx,
    tree: &BTreeMap<String, Option<Vec<u8>>>,
    recovery: &Recovery,
    batch: &BatchId,
    was: &super::recover_tests::Before,
    what: &str,
) {
    proven_as(fx, tree, recovery, batch, was, what, Far::Ids)
}

/// [`proven`] with `far/` on a volume of `kind`.
fn proven_as(
    fx: &mut Fx,
    tree: &BTreeMap<String, Option<Vec<u8>>>,
    recovery: &Recovery,
    batch: &BatchId,
    was: &super::recover_tests::Before,
    what: &str,
    kind: Far,
) {
    check(fx, was, recovery, batch);
    where_it_says(fx, tree, batch, what);
    let clean = far_as(fx, kind);
    let trash = trash_as(fx, kind);
    *trash.calls.borrow_mut() = 100;
    for u in &recovery.unfinished {
        put_back(fx, u.operation, &clean, &trash, what, kind);
    }
    where_it_says(fx, tree, batch, what);
}

/// The folder as the outcome of `batch` says.
fn where_it_says(fx: &Fx, tree: &BTreeMap<String, Option<Vec<u8>>>, batch: &BatchId, what: &str) {
    let outcome = super::outcome(&fx.cat, batch).unwrap();
    let Some(item) = outcome.items.first() else {
        assert_eq!(&tree_of(&fx.root.join("day")), tree, "{what}");
        return;
    };
    match &item.result {
        ItemResult::FolderMoved => {
            assert_eq!(&tree_of(&fx.root.join("far/day")), tree, "{what}");
            if item.originals_kept.is_empty() {
                assert!(!fx.root.join("day").exists(), "{what}");
            }
            assert!(partials(fx).is_empty(), "{what}: {:?}", partials(fx));
        }
        ItemResult::Unchanged(_) => {
            assert_eq!(&tree_of(&fx.root.join("day")), tree, "{what}");
            assert!(!fx.root.join("far/day").exists(), "{what}");
            assert!(partials(fx).is_empty(), "{what}: {:?}", partials(fx));
        }
        ItemResult::NeedsRecovery(_) => {}
        other => panic!("{what}: {other:?}"),
    }
}

#[test]
fn every_fault_at_every_step_of_a_folder_copy_ends_proven() {
    assert!(every_fault(Far::Ids) >= 100);
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the fault matrix by names is the same code everywhere; Linux and macOS run it, keeping Windows CI within its hour"
)]
fn every_fault_at_every_step_of_a_folder_copy_to_exfat_ends_proven() {
    assert!(every_fault(Far::Names) >= 60);
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the fault matrix by names is the same code everywhere; Linux and macOS run it, keeping Windows CI within its hour"
)]
fn every_fault_at_every_step_of_a_folder_built_at_its_name_ends_proven() {
    assert!(every_fault(Far::Direct) >= 50);
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the fault matrix by names is the same code everywhere; Linux and macOS run it, keeping Windows CI within its hour"
)]
fn every_fault_at_every_step_of_a_folder_copy_to_an_smb_share_ends_proven() {
    assert!(every_fault(Far::Share) >= 50);
}

/// A folder moved off a share stays there once copied: macOS has no Trash
/// on an SMB share, and Linux's isn't used. The question says so before,
/// and nothing is deleted.
#[test]
fn a_folder_moved_from_an_smb_share_stays_there_as_its_trash_can_t_take_it() -> Result<()> {
    let mut fx = files();
    // The fixture's volume and `far/`'s both shares.
    let disk = Faulty {
        names_only: vec![fx.root.clone()],
        share: true,
        no_exclusive_rename: true,
        lists_decomposed: true,
        ..far(&fx)
    };
    let tree = tree_of(&fx.root.join("day"));
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let across = plan.summary().across.clone().expect("it copies");
    assert_eq!(across.originals_stay, 1);
    let mut trash = FakeTrash::new(&fx);
    trash.fail_from = Some(1);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(results(&outcome), [ItemResult::FolderMoved], "{outcome:?}");
    assert_eq!(tree_of(&fx.root.join("far/day")), tree);
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    Ok(())
}

/// Every fault at every step of moving `day/` to `far/` as `kind`, then
/// recovery: each ends proven. Returns how many runs it made.
fn every_fault(kind: Far) -> usize {
    let started = std::time::Instant::now();
    // A run without faults says how many steps there are.
    let mut clean = small();
    let counting = far_as(&clean, kind);
    let trash = trash_as(&clean, kind);
    let op = to_far(&clean);
    let outcome = run(&mut clean, op, &trash, &counting, Cancel::NEVER);
    assert_eq!(results(&outcome), [ItemResult::FolderMoved], "{kind:?}");
    assert_eq!(counting.count(Op::Copy), 3);
    // And one undone once two files are whole: a full drive at the last.
    let mut undone = small();
    let failing = Faulty {
        faults: vec![(Op::Copy, 2, Fault::Fail)],
        ..far_as(&undone, kind)
    };
    let trash = trash_as(&undone, kind);
    let op = to_far(&undone);
    let _ = run(&mut undone, op, &trash, &failing, Cancel::NEVER);
    let mut runs = 0;
    for op in [
        Op::CreateDir,
        Op::CreateFile,
        Op::Copy,
        Op::SyncFile,
        Op::CopyExtras,
        Op::CopyTimes,
        Op::Hash,
        Op::Sync,
        Op::Rename,
        Op::RemoveFile,
        Op::RemoveDir,
    ] {
        // Faults while undoing need the undoing run; the others its steps
        // before the fault are the clean run's.
        let undoing = matches!(op, Op::RemoveFile | Op::RemoveDir | Op::Sync)
            || (kind != Far::Ids && op == Op::Hash);
        let count = counting.count(op).max(failing.count(op));
        for nth in 0..count {
            let faults: &[Fault] = match op {
                Op::Copy => &[
                    Fault::Fail,
                    Fault::CrashBefore,
                    Fault::CrashMid,
                    Fault::CrashAfter,
                ],
                _ => &[Fault::Fail, Fault::CrashBefore, Fault::CrashAfter],
            };
            for &fault in faults {
                // On exFAT, a crash before a call only at its first: it is
                // one after the call before but for the journal between,
                // which the ids run covers. Kept so for CI's time.
                if kind != Far::Ids && fault == Fault::CrashBefore && nth > 0 {
                    continue;
                }
                for undo in [false, true] {
                    if undo && !undoing {
                        continue;
                    }
                    let mut fx = small();
                    let was = before(&fx);
                    let tree = tree_of(&fx.root.join("day"));
                    let mut with = vec![(op, nth, fault)];
                    if undo {
                        with.push((Op::Copy, 2, Fault::Fail));
                    }
                    let disk = Faulty {
                        faults: with,
                        ..far_as(&fx, kind)
                    };
                    let trash = trash_as(&fx, kind);
                    let (batch, recovery) = scenario_as(&mut fx, &trash, &disk, kind);
                    let what = format!("{kind:?} {op:?} {nth} {fault:?} undo {undo}");
                    if kind == Far::Share {
                        assert_eq!(removed_far(&fx, &disk), [] as [PathBuf; 0], "{what}");
                    }
                    proven_as(&mut fx, &tree, &recovery, &batch, &was, &what, kind);
                    runs += 1;
                }
            }
        }
    }
    // The Trash failing at, or crashing after, the first and second item.
    for n in 1..=2 {
        for crash in [false, true] {
            let mut fx = small();
            let was = before(&fx);
            let tree = tree_of(&fx.root.join("day"));
            let disk = far_as(&fx, kind);
            let mut trash = trash_as(&fx, kind);
            match crash {
                true => trash.crash_after = Some(n),
                false => trash.fail_from = Some(n),
            }
            let (batch, recovery) = scenario_as(&mut fx, &trash, &disk, kind);
            let what = format!("{kind:?} trash {n} {crash}");
            proven_as(&mut fx, &tree, &recovery, &batch, &was, &what, kind);
            runs += 1;
        }
    }
    eprintln!("{kind:?}: {runs} runs in {:?}", started.elapsed());
    runs
}

#[test]
fn every_failed_journal_write_of_a_folder_copy_ends_proven() -> Result<()> {
    for when in [
        "BEFORE INSERT ON file_operation_copies",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'copying'",
        "BEFORE UPDATE ON file_operation_copies WHEN NEW.content IS NOT NULL",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'copied'",
        "BEFORE UPDATE ON folders",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'committed'",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'trashed'",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'discarded'",
    ] {
        let mut fx = files();
        let was = before(&fx);
        let tree = tree_of(&fx.root.join("day"));
        fx.fail_journal(when);
        let disk = far(&fx);
        let trash = FakeTrash::new(&fx);
        let plan = fx.plan_on(to_far(&fx), &disk)?;
        let batch = plan.batch().clone();
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
        fx.reopen();
        let recovery = fx.recover_with(&far(&fx), &trash);
        assert!(recovery.unfinished.is_empty(), "{when}: {recovery:?}");
        proven(&mut fx, &tree, &recovery, &batch, &was, when);
    }
    Ok(())
}

#[test]
fn stop_stops_a_folder_copy_and_undoes_it() -> Result<()> {
    let mut fx = files();
    let tree = tree_of(&fx.root.join("day"));
    let rows = fx.rows();
    let stop = Arc::new(AtomicBool::new(false));
    let disk = Faulty {
        stop: Some(stop.clone()),
        ..far(&fx)
    };
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &NoTrash, &disk, Cancel::new(&stop));
    let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert_eq!(why, "Stopped.");
    assert_eq!(fx.rows(), rows);
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    assert!(!fx.root.join("far/day").exists());
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    Ok(())
}

#[test]
fn a_name_taken_just_before_the_folder_gets_it_is_never_replaced() -> Result<()> {
    let mut fx = files();
    let tree = tree_of(&fx.root.join("day"));
    let disk = Faulty {
        occupy: Some(fx.root.join("far/day")),
        ..far(&fx)
    };
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &NoTrash, &disk, Cancel::NEVER);
    assert!(
        matches!(&results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
        "{outcome:?}"
    );
    assert_eq!(std::fs::read(fx.root.join("far/day"))?, b"raced");
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    Ok(())
}

#[test]
fn an_original_changed_while_copied_keeps_its_whole_copy_until_put_back() -> Result<()> {
    let mut fx = files();
    let rows = fx.rows();
    let disk = Faulty {
        change_source: true,
        ..far(&fx)
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let [ItemResult::NeedsRecovery(_)] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert_eq!(fx.rows(), rows);
    // The copy of the original as it was is the one place those bytes are.
    assert!(!partials(&fx).is_empty());
    let id = outcome.items[0].operation;
    let back = {
        let clean = far(&fx);
        recover::resolve_with(&mut fx.cat, id, Resolution::PutBack, &clean, &trash)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(matches!(back.result, ItemResult::Unchanged(_)), "{back:?}");
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    assert!(!fx.root.join("far/day").exists());
    let first = fx
        .files()
        .into_iter()
        .any(|(k, v)| k.starts_with("trash/") && v == bytes("day/.hidden"));
    assert!(first, "the copy of the original as it was is in the Trash");
    Ok(())
}

#[test]
fn a_file_added_to_the_copy_before_recovery_keeps_it_for_the_user() -> Result<()> {
    let mut fx = files();
    let tree = tree_of(&fx.root.join("day"));
    let disk = Faulty {
        elsewhere: Some(fx.root.join("far")),
        ..Faulty::at(Op::Rename, 0, Fault::CrashAfter)
    };
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            Cancel::NEVER,
            &disk,
        ));
    }));
    std::fs::write(fx.root.join("far/day/theirs.txt"), b"theirs")?;
    fx.reopen();
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert_eq!(recovery.unfinished.len(), 1, "{recovery:?}");
    let u = recovery.unfinished[0].operation;
    let back = {
        let clean = far(&fx);
        recover::resolve_with(&mut fx.cat, u, Resolution::PutBack, &clean, &trash)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(
        matches!(back.result, ItemResult::NeedsRecovery(_)),
        "{back:?}"
    );
    assert_eq!(
        std::fs::read(fx.root.join("far/day/theirs.txt"))?,
        b"theirs"
    );
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    let kept = {
        let clean = far(&fx);
        recover::resolve_with(&mut fx.cat, u, Resolution::Keep, &clean, &trash)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(kept.result, ItemResult::Kept);
    assert_eq!(
        results(&super::outcome(&fx.cat, &batch)?),
        [ItemResult::Kept]
    );
    Ok(())
}

#[test]
fn originals_changed_or_added_to_after_the_catalog_follows_stay_and_are_reported() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("day/a.NEF");
    let disk = far(&fx);
    let mut trash = FakeTrash::new(&fx);
    trash.crash_before = Some(1);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            Cancel::NEVER,
            &disk,
        ));
    }));
    // Meanwhile one original changes and a file is added beside them.
    std::fs::write(fx.root.join("day/a.xmp"), b"edited since")?;
    std::fs::write(fx.root.join("day/sub/new.txt"), b"new")?;
    fx.reopen();
    trash.crash_before = None;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let outcome = super::outcome(&fx.cat, &batch)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(fx.rows()[&a], "far/day/a.NEF");
    // Proven originals went one by one; the changed one, the new file and
    // the folders holding them stay, reported.
    assert!(!fx.root.join("day/a.NEF").exists());
    assert!(!fx.root.join("day/sub/b.NEF").exists());
    assert_eq!(std::fs::read(fx.root.join("day/a.xmp"))?, b"edited since");
    assert_eq!(std::fs::read(fx.root.join("day/sub/new.txt"))?, b"new");
    let kept: Vec<&Path> = outcome.items[0]
        .originals_kept
        .iter()
        .map(|k| k.path.as_path())
        .collect();
    // A file first, the one that says why.
    assert!(kept[0].ends_with("day/a.xmp"), "{kept:?}");
    for stays in ["day/a.xmp", "day", "day/sub"] {
        assert!(kept.iter().any(|k| k.ends_with(stays)), "{stays}: {kept:?}");
    }
    assert_eq!(
        std::fs::read(fx.root.join("far/day/a.xmp"))?,
        bytes("day/a.xmp")
    );
    Ok(())
}

#[test]
fn a_photo_added_to_the_folder_since_planning_leaves_it_where_it_is() -> Result<()> {
    let mut fx = files();
    let disk = far(&fx);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    std::fs::write(fx.root.join("day/c.NEF"), bytes("day/c.NEF"))?;
    let root = fx.root.clone();
    fx.cat.add_folder(&root)?;
    let c = fx.photo("day/c.NEF");
    let outcome =
        execute::execute_with(&mut fx.cat, plan.confirm(), &NoTrash, Cancel::NEVER, &disk)?;
    assert!(
        matches!(&results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
        "{outcome:?}"
    );
    assert_eq!(fx.rows()[&c], "day/c.NEF");
    assert!(!fx.root.join("far/day").exists());
    assert!(partials(&fx).is_empty());
    Ok(())
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_OTHER_VOLUME: a writable folder on another volume"]
fn a_folder_moves_between_two_real_volumes() -> Result<()> {
    let Some(other) = std::env::var_os("RAWMAKASE_TEST_OTHER_VOLUME") else {
        return Ok(());
    };
    let mut fx = files();
    let there = tempfile::tempdir_in(&other)?;
    let dir: PathBuf = there.path().canonicalize()?;
    assert_ne!(
        fsops::entry(&dir)?.id.volume(),
        fsops::entry(&fx.root)?.id.volume(),
        "{} is on the fixture's volume",
        dir.display()
    );
    std::fs::write(dir.join("keep.dng"), bytes("keep.dng"))?;
    fx.cat.add_folder(&dir)?;
    let other_root = fx
        .cat
        .roots()?
        .into_iter()
        .map(|(id, _, _)| id)
        .find(|id| *id != fx.root_id())
        .unwrap();
    let a = fx.photo("day/a.NEF");
    // Names FAT and exFAT on macOS list decomposed, stored either way.
    std::fs::write(fx.root.join("day/Caf\u{e9}.NEF"), bytes("cafe"))?;
    std::fs::write(fx.root.join("day/sub/Ru\u{308}gen.NEF"), bytes("rugen"))?;
    // Tags on a folder and a file, and on macOS a custom folder icon: a
    // file with only a resource fork, and its folder's FinderInfo flag.
    let tags = ["", "sub", "a.NEF"];
    let mut extras = Vec::new();
    if super::tests::keeps_extras(&dir) {
        let day = fx.root.join("day");
        if cfg!(target_vendor = "apple") {
            let icon = day.join("Icon\r");
            std::fs::write(&icon, b"")?;
            std::fs::write(icon.join("..namedfork/rsrc"), bytes("icon"))?;
            let mut info = [0u8; 32];
            info[8] = 0x04;
            fsops::set_extra(&day, "com.apple.FinderInfo", &info)?;
        }
        // Never empty: SMB takes an empty extended attribute for none
        // (Linux's CIFS against Samba drops it), and the move is refused.
        for t in tags {
            fsops::set_extra(&day.join(t), "tag", format!("tag {t}").as_bytes())?;
        }
        for t in tags
            .iter()
            .chain(cfg!(target_vendor = "apple").then_some(&"Icon\r"))
        {
            extras.push((t.to_string(), fsops::extras(&day.join(t))?));
        }
    }
    let tree = as_seen(tree_of(&fx.root.join("day")));
    let op = FileOp::MoveFolder {
        root: fx.root_id(),
        folder: "day".into(),
        to_root: other_root,
        parent: String::new(),
    };
    let plan = fx.plan(op);
    assert!(plan.summary().across.is_some());
    let trash = FakeTrash::new(&fx);
    let outcome = execute::execute(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(as_seen(tree_of(&dir.join("day"))), tree);
    for (t, was) in &extras {
        assert_eq!(&fsops::extras(&dir.join("day").join(t))?, was, "{t:?}");
    }
    assert!(!fx.root.join("day").exists());
    let path = fx
        .cat
        .photos()?
        .into_iter()
        .find(|p| p.id == a)
        .unwrap()
        .path;
    assert_eq!(path, dir.join("day/a.NEF"));
    let leftovers: Vec<_> = tree_of(&dir)
        .into_keys()
        .filter(|k| k.contains(".rawmakase-part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    drop(fx);
    clear(&dir)?;
    there.close()?;
    Ok(())
}

/// A tree as names compare on every drive: composed, without macOS's
/// AppleDouble files (`._name` beside `name`) on FAT and exFAT.
fn as_seen(tree: BTreeMap<String, Option<Vec<u8>>>) -> BTreeMap<String, Option<Vec<u8>>> {
    use unicode_normalization::UnicodeNormalization;
    let composed: BTreeMap<String, Option<Vec<u8>>> = tree
        .into_iter()
        .map(|(k, v)| (k.nfc().collect(), v))
        .collect();
    let apple_double = |k: &str| {
        let (dir, name) = k.rsplit_once('/').map_or(("", k), |(d, n)| (d, n));
        name.strip_prefix("._").is_some_and(|of| {
            let main = match dir {
                "" => of.to_string(),
                dir => format!("{dir}/{of}"),
            };
            composed.contains_key(&main)
        })
    };
    composed
        .iter()
        .filter(|(k, _)| !apple_double(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Removes everything in `dir`, by names composed (macOS's exFAT removes a
/// file only by the spelling it is stored in, which this test made so).
fn clear(dir: &Path) -> Result<()> {
    use unicode_normalization::UnicodeNormalization;
    for name in std::fs::read_dir(dir)? {
        let name: String = name?.file_name().to_string_lossy().nfc().collect();
        let path = dir.join(name);
        if path.is_dir() {
            clear(&path)?;
        }
        let removed = match path.is_dir() {
            true => std::fs::remove_dir(&path),
            false => std::fs::remove_file(&path),
        };
        // AppleDouble `._` files go with the file they are of.
        match removed {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            other => other?,
        }
    }
    Ok(())
}

#[test]
fn a_file_added_to_the_original_after_the_catalog_follows_stays_with_its_folder() -> Result<()> {
    let mut fx = files();
    let disk = far(&fx);
    let mut trash = FakeTrash::new(&fx);
    trash.crash_before = Some(1);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            Cancel::NEVER,
            &disk,
        ));
    }));
    // Every original is as copied, but one more file is beside them.
    std::fs::write(fx.root.join("day/new.txt"), b"new")?;
    fx.reopen();
    trash.crash_before = None;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let outcome = super::outcome(&fx.cat, &batch)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(std::fs::read(fx.root.join("day/new.txt"))?, b"new");
    assert!(!fx.root.join("day/a.NEF").exists());
    let kept = &outcome.items[0].originals_kept;
    let day = kept
        .iter()
        .find(|k| k.path == fx.root.join("day"))
        .unwrap_or_else(|| panic!("{kept:?}"));
    // What stays because of what wasn't copied says so.
    assert_eq!(day.why, "it holds new.txt, which wasn't copied");
    Ok(())
}

/// Saves an edit and reads a checksum for photo `name` in folder `folder`.
fn edited(fx: &mut Fx, folder: &str, name: &str) -> Result<()> {
    let id = fx.photo(&format!("{folder}/{name}"));
    let source = fx.root.join(folder).join(name);
    fx.cat.save_edit(
        id,
        &source,
        &crate::storage::Identity::read(&source)?,
        &crate::model::recipe::Recipe::default(),
        &crate::export_settings::ExportOptions::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    let crate::storage::ChecksumRead::Done(checksum) =
        crate::storage::FileChecksum::read_full(&source, |_| true)?
    else {
        panic!("not read")
    };
    fx.cat.save_checksums(&[crate::catalog::ReadChecksum {
        photo: id,
        folder: fx.folder(folder),
        filename: name.into(),
        path: source,
        checksum,
    }])?;
    Ok(())
}

/// The identity photo `id`'s saved edit names.
fn saved_identity(fx: &Fx, id: PhotoId) -> Option<crate::storage::Identity> {
    let text: Option<String> = fx
        .cat
        .db_for_tests()
        .query_row("SELECT identity FROM photos WHERE id=?", [id.0], |r| {
            r.get(0)
        })
        .unwrap();
    text.map(|t| serde_json::from_str(&t).unwrap())
}

#[test]
fn coarser_times_there_rekey_the_folders_saved_edits_and_checksums() -> Result<()> {
    let mut fx = files();
    edited(&mut fx, "day/sub", "b.NEF")?;
    let b = fx.photo("day/sub/b.NEF");
    let copy = fx.cat.create_virtual_copy(b)?;
    let was = crate::storage::Identity::read(&fx.root.join("day/sub/b.NEF"))?;
    let mut disk = far(&fx);
    // FAT's two seconds.
    disk.coarse = Some(2_000_000_000);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    let target = fx.root.join("far/day/sub/b.NEF");
    let now = crate::storage::Identity::read(&target)?;
    assert_ne!(now, was, "the time was cut");
    assert_eq!(saved_identity(&fx, b), Some(now.clone()));
    assert_eq!(saved_identity(&fx, copy), Some(now.clone()));
    assert_eq!(fx.cat.checksums()?[&b].identity, now);
    assert!(fx.cat.load_edit(b, &target)?.is_some(), "the edit opens");
    Ok(())
}

#[test]
fn an_earlier_releases_edit_file_and_coarser_times_leave_the_folder() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/a.NEF.rawmakase.json", "far/"]);
    let tree = tree_of(&fx.root.join("day"));
    let mut disk = far(&fx);
    disk.coarse = Some(2_000_000_000);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert!(why.contains("earlier release"), "{why}");
    assert_eq!(fx.rows()[&fx.photo("day/a.NEF")], "day/a.NEF");
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    assert!(!fx.root.join("far/day").exists());
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    Ok(())
}

#[test]
fn a_crash_once_the_trash_took_the_folder_is_finished_without_keeping_anything() -> Result<()> {
    let mut fx = files();
    let disk = far(&fx);
    let mut trash = FakeTrash::new(&fx);
    trash.crash_after = Some(1);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            Cancel::NEVER,
            &disk,
        ));
    }));
    fx.reopen();
    trash.crash_after = None;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let outcome = super::outcome(&fx.cat, &batch)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert!(outcome.items[0].originals_kept.is_empty(), "{outcome:?}");
    assert!(!fx.root.join("day").exists());
    assert!(trash.dir.join("1-day/sub/b.NEF").is_file());
    Ok(())
}

#[test]
fn a_move_refused_for_one_photo_leaves_every_other_photos_saved_edit_as_it_was() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/b.NEF", "day/b.NEF.rawmakase.json", "far/"]);
    edited(&mut fx, "day", "a.NEF")?;
    let a = fx.photo("day/a.NEF");
    let saved = saved_identity(&fx, a);
    let checksum = fx.cat.checksums()?[&a].identity.clone();
    let mut disk = far(&fx);
    disk.coarse = Some(2_000_000_000);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert!(why.contains("earlier release"), "{why}");
    assert_eq!(saved_identity(&fx, a), saved);
    assert_eq!(fx.cat.checksums()?[&a].identity, checksum);
    assert!(fx.cat.load_edit(a, &fx.root.join("day/a.NEF"))?.is_some());
    Ok(())
}

#[test]
fn the_catalog_follows_the_names_the_copies_have_on_disk() -> Result<()> {
    let mut fx = files();
    let (b, sub) = (fx.photo("day/sub/b.NEF"), fx.folder("day/sub"));
    // The catalog spells them otherwise than the disk, as a volume that
    // ignores case lets it.
    fx.cat.db_for_tests().execute_batch(&format!(
        "UPDATE photos SET filename='B.nef' WHERE id={};
         UPDATE folder_paths SET path='day/SUB' WHERE folder={};",
        b.0, sub.0
    ))?;
    let ignores_case = fsops::entry(&fx.root.join("day/SUB/B.nef")).is_ok();
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    if ignores_case {
        assert_eq!(fx.rows()[&b], "far/day/sub/b.NEF");
        assert_eq!(logical(&fx, sub), "far/day/sub");
    } else {
        // Missing before, as it was named; missing after.
        assert_eq!(fx.rows()[&b], "far/day/SUB/B.nef");
    }
    Ok(())
}

#[test]
fn names_only_case_tells_apart_are_refused_when_planning() -> Result<()> {
    // A name there that differs only in case, as a volume that compares
    // names exactly lets it have.
    let fx = files();
    std::fs::create_dir(fx.root.join("far/DAY"))?;
    let why = refusal(&fx, &far(&fx));
    assert!(why.contains("already there"), "{why}");
    std::fs::remove_dir(fx.root.join("far/DAY"))?;
    // Two such names in the folder, where its volume can have both.
    std::fs::write(fx.root.join("day/notes.txt"), b"lower")?;
    std::fs::write(fx.root.join("day/NOTES.txt"), b"upper")?;
    if std::fs::read(fx.root.join("day/notes.txt"))? == b"lower" {
        let why = refusal(&fx, &far(&fx));
        assert!(why.contains("would be one name"), "{why}");
    }
    Ok(())
}

#[test]
fn a_file_another_app_saves_anew_keeps_only_its_own_copy_which_put_back_trashes() -> Result<()> {
    let mut fx = files();
    let tree_before = tree_of(&fx.root.join("day"));
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    // Saved anew the usual way: a new file renamed over the old one.
    std::fs::write(fx.root.join("day/.a.xmp.tmp"), b"saved anew")?;
    std::fs::rename(fx.root.join("day/.a.xmp.tmp"), fx.root.join("day/a.xmp"))?;
    let outcome = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk)?;
    let [ItemResult::NeedsRecovery(_)] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    // The copies of what is proven unchanged are gone; the one of what
    // changed stays, in the hidden folder.
    let left = partials(&fx);
    assert!(left.iter().any(|p| p.ends_with("a.xmp")), "{left:?}");
    assert!(
        !left
            .iter()
            .any(|p| p.ends_with("a.NEF") || p.ends_with(".hidden")),
        "{left:?}"
    );
    // The unfinished operation says where its copies are.
    fx.reopen();
    let recovery = fx.recover_with(&far(&fx), &trash);
    let u = &recovery.unfinished[0];
    for m in &u.members {
        if let Some(after) = &m.after {
            assert!(
                after.to_string_lossy().contains(".rawmakase-part"),
                "{after:?}"
            );
        }
    }
    let back = {
        let clean = far(&fx);
        recover::resolve_with(
            &mut fx.cat,
            u.operation,
            Resolution::PutBack,
            &clean,
            &trash,
        )
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(matches!(back.result, ItemResult::Unchanged(_)), "{back:?}");
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    let mut now = tree_before;
    now.insert("a.xmp".into(), Some(b"saved anew".to_vec()));
    assert_eq!(tree_of(&fx.root.join("day")), now);
    assert!(
        fx.files()
            .iter()
            .any(|(k, v)| k.starts_with("trash/") && v == b"saved anew")
    );
    Ok(())
}

#[test]
fn a_missing_folder_below_one_spelled_otherwise_stays_below_it() -> Result<()> {
    let mut fx = fixture(&["day/sub/b.NEF", "day/sub/gone/c.NEF", "far/"]);
    let (sub, gone) = (fx.folder("day/sub"), fx.folder("day/sub/gone"));
    std::fs::remove_file(fx.root.join("day/sub/gone/c.NEF"))?;
    std::fs::remove_dir(fx.root.join("day/sub/gone"))?;
    fx.cat.db_for_tests().execute_batch(&format!(
        "UPDATE folder_paths SET path='day/SUB' WHERE folder={};
         UPDATE folder_paths SET path='day/SUB/gone' WHERE folder={};",
        sub.0, gone.0
    ))?;
    let ignores_case = fsops::entry(&fx.root.join("day/SUB")).is_ok();
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    // Below its folder, however the disk spells that.
    let (sub, gone) = (logical(&fx, sub), logical(&fx, gone));
    assert_eq!(gone, format!("{sub}/gone"));
    if ignores_case {
        assert_eq!(sub, "far/day/sub");
    }
    Ok(())
}

#[test]
fn a_missing_folder_never_takes_the_place_of_another_named_alike() -> Result<()> {
    // Two folders only case tells apart, where the volume can have both.
    let mut fx = fixture(&["day/sub/b.NEF", "day/SUB/c.NEF", "far/"]);
    if std::fs::read_dir(fx.root.join("day"))?.count() != 2 {
        return Ok(());
    }
    let (sub, upper) = (fx.folder("day/sub"), fx.folder("day/SUB"));
    std::fs::remove_file(fx.root.join("day/SUB/c.NEF"))?;
    std::fs::remove_dir(fx.root.join("day/SUB"))?;
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(logical(&fx, sub), "far/day/sub");
    assert_eq!(logical(&fx, upper), "far/day/SUB");
    Ok(())
}

/// The copy ids journalled for batch `batch`'s items, as text.
fn copy_ids(fx: &Fx, batch: &BatchId) -> Vec<Option<String>> {
    let db = fx.cat.db_for_tests();
    let mut stmt = db
        .prepare(
            "SELECT c.copy_id FROM file_operation_copies c
             JOIN file_operations o ON o.id=c.operation WHERE o.batch=?1",
        )
        .unwrap();
    stmt.query_map([batch.to_string()], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[test]
fn a_folder_moves_to_exfat_proven_by_names_and_journals_no_real_id() -> Result<()> {
    for kind in Far::by_names() {
        let mut fx = files();
        let b = fx.photo("day/sub/b.NEF");
        let tree = tree_of(&fx.root.join("day"));
        let disk = far_as(&fx, kind);
        let trash = FakeTrash::new(&fx);
        let plan = fx.plan_on(to_far(&fx), &disk)?;
        let batch = plan.batch().clone();
        let outcome =
            execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk)?;
        assert_eq!(results(&outcome), [ItemResult::FolderMoved], "{kind:?}");
        assert_eq!(tree_of(&fx.root.join("far/day")), tree, "{kind:?}");
        assert_eq!(fx.rows()[&b], "far/day/sub/b.NEF");
        assert!(!fx.root.join("day").exists());
        let ids = copy_ids(&fx, &batch);
        assert!(
            !ids.is_empty() && ids.iter().all(|i| i.as_deref() == Some("0:0")),
            "{kind:?}: {ids:?}"
        );
        // Built at its name where the volume can't rename exclusively.
        assert_eq!(disk.renames().is_empty(), kind != Far::Names, "{kind:?}");
        assert!(partials(&fx).is_empty(), "{kind:?}: {:?}", partials(&fx));
    }
    Ok(())
}

#[test]
fn a_folder_name_taken_just_before_it_is_made_at_it_is_left_as_it_is() -> Result<()> {
    let mut fx = files();
    let tree = tree_of(&fx.root.join("day"));
    let disk = Faulty {
        occupy_on_create: Some((fx.root.join("far/day"), Vec::new())),
        ..far_as(&fx, Far::Direct)
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert!(
        matches!(&results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
        "{outcome:?}"
    );
    // Theirs: a folder made there by another program, kept.
    assert!(fx.root.join("far/day").is_dir());
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    fx.reopen();
    assert!(
        fx.recover_with(&far_as(&fx, Far::Direct), &trash)
            .unfinished
            .is_empty()
    );
    assert!(fx.root.join("far/day").is_dir());
    Ok(())
}

/// The catalog folder of the fixture's root whose logical path is
/// `logical`, composed.
fn logical_of(fx: &Fx, logical: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let db = fx.cat.db_for_tests();
    let mut stmt = db.prepare("SELECT path FROM folder_paths").unwrap();
    let paths: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    paths
        .into_iter()
        .find(|p| p.nfc().collect::<String>() == logical)
        .unwrap_or_else(|| panic!("no folder {logical}"))
}

#[test]
fn accented_names_stored_either_way_move_with_a_folder_to_and_from_exfat() -> Result<()> {
    // A folder named decomposed, holding a photo named decomposed and one
    // named composed.
    let names = [
        "Cafe\u{301}/Ru\u{308}gen.NEF",
        "Cafe\u{301}/\u{c9}t\u{e9}.NEF",
        "far/",
    ];
    // To exFAT: made there composed.
    for kind in Far::by_names() {
        let mut fx = fixture(&names);
        let disk = far_as(&fx, kind);
        let trash = FakeTrash::new(&fx);
        let op = FileOp::MoveFolder {
            root: fx.root_id(),
            folder: logical_of(&fx, "Caf\u{e9}"),
            to_root: fx.root_id(),
            parent: "far".into(),
        };
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        assert_eq!(results(&outcome), [ItemResult::FolderMoved], "{kind:?}");
        let made = fx.root.join("far/Caf\u{e9}/R\u{fc}gen.NEF");
        assert!(super::tests::stored_spelling(&made), "{kind:?}");
        assert_eq!(
            std::fs::read(&made)?,
            bytes("Cafe\u{301}/Ru\u{308}gen.NEF"),
            "{kind:?}"
        );
        // And renamed there, by the spelling it was made in, as FAT renames
        // only so (exFAT, which can't rename exclusively, renames nothing).
        if kind != Far::Names {
            continue;
        }
        let op = FileOp::RenameFolder {
            root: fx.root_id(),
            folder: logical_of(&fx, "far/Caf\u{e9}"),
            name: "Bar".into(),
        };
        // A rename keeps a file's number while mounted (measured on macOS).
        let disk = Faulty {
            renumber_on_rename: false,
            ..far_as(&fx, kind)
        };
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        assert!(
            matches!(results(&outcome)[..], [ItemResult::FolderRenamed]),
            "{kind:?}: {outcome:?}"
        );
    }
    // A folder others made there stored decomposed renames by that
    // spelling.
    let mut fx = fixture(&["far/Cafe\u{301}/Ru\u{308}gen.NEF"]);
    let op = FileOp::RenameFolder {
        root: fx.root_id(),
        folder: logical_of(&fx, "far/Caf\u{e9}"),
        name: "Bar".into(),
    };
    let disk = Faulty {
        renumber_on_rename: false,
        ..far_as(&fx, Far::Names)
    };
    let trash = FakeTrash::new(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert!(
        matches!(results(&outcome)[..], [ItemResult::FolderRenamed]),
        "{outcome:?}"
    );
    assert!(fx.root.join("far/Bar/Ru\u{308}gen.NEF").exists());
    // From exFAT: its originals go to the Trash by the spelling stored,
    // the folder's own name stored decomposed or composed.
    let composed = [
        "Caf\u{e9}/Ru\u{308}gen.NEF",
        "Caf\u{e9}/\u{c9}t\u{e9}.NEF",
        "far/",
    ];
    for names in [names, composed] {
        let mut fx = fixture(&names);
        let disk = Faulty {
            names_only: vec![fx.root.clone()],
            lists_decomposed: true,
            ..far(&fx)
        };
        let mut trash = FakeTrash::new(&fx);
        trash.stored_only = true;
        let op = FileOp::MoveFolder {
            root: fx.root_id(),
            folder: logical_of(&fx, "Caf\u{e9}"),
            to_root: fx.root_id(),
            parent: "far".into(),
        };
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        assert_eq!(results(&outcome), [ItemResult::FolderMoved], "{names:?}");
        assert!(
            outcome.items[0].originals_kept.is_empty(),
            "{names:?}: {outcome:?}"
        );
        let gone = |name: &str| !fx.root.join(name).exists();
        assert!(gone("Cafe\u{301}") && gone("Caf\u{e9}"), "{names:?}");
    }
    Ok(())
}

#[test]
fn a_folder_named_as_an_apple_double_file_moves_with_its_folder() -> Result<()> {
    // Only a file is macOS's AppleDouble file: a folder `._a.NEF` beside
    // `a.NEF` on FAT or exFAT is the user's own.
    let mut fx = fixture(&["day/a.NEF", "day/._a.NEF/only.txt", "far/"]);
    let disk = Faulty {
        names_only: vec![fx.root.clone()],
        lists_decomposed: true,
        ..far(&fx)
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(
        std::fs::read(fx.root.join("far/day/._a.NEF/only.txt"))?,
        bytes("day/._a.NEF/only.txt")
    );
    Ok(())
}

#[cfg(target_vendor = "apple")]
#[test]
fn a_file_named_as_anothers_apple_double_file_is_not_copied_to_fat_or_exfat() {
    // There macOS would take it for `a.NEF`'s, and write over it.
    for kind in Far::by_names() {
        let fx = fixture(&["day/a.NEF", "day/._a.NEF", "far/"]);
        let refused = fx.plan_on(to_far(&fx), &far_as(&fx, kind));
        assert!(
            matches!(&refused, Err(FileOpError::Destination(why)) if why.contains("._a.NEF")),
            "{kind:?}: {refused:?}"
        );
    }
}

#[test]
fn a_folder_holding_the_catalog_is_refused_by_either_spelling() -> Result<()> {
    // On FAT and exFAT the folder is used by its composed name; the
    // catalog's own path may spell it decomposed, as the drive lists it.
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?.join("trip");
    let holder = root.join("Cafe\u{301}");
    std::fs::create_dir_all(&holder)?;
    std::fs::write(holder.join("a.NEF"), bytes("a.NEF"))?;
    let catalog = holder.join("Photos.rawmakase");
    let mut cat = crate::catalog::Catalog::create(&catalog)?;
    cat.add_folder(&root)?;
    let fx = Fx {
        dir,
        root,
        catalog,
        cat,
    };
    let disk = Faulty {
        names_only: vec![fx.root.clone()],
        lists_decomposed: true,
        ..Faulty::default()
    };
    let op = FileOp::RenameFolder {
        root: fx.root_id(),
        folder: logical_of(&fx, "Caf\u{e9}"),
        name: "Bar".into(),
    };
    let refused = fx.plan_on(op, &disk);
    assert!(
        matches!(&refused, Err(FileOpError::Destination(why)) if why.contains("catalog")),
        "{refused:?}"
    );
    Ok(())
}

/// A folder move onto a share that fails leaves what it copied there,
/// never removed (#675): removing a copy by its name could remove a file
/// another computer put at that name just then, as the test disk does.
#[test]
fn a_folder_move_onto_an_smb_share_that_fails_leaves_its_copies_there() -> Result<()> {
    let mut fx = small();
    let tree = tree_of(&fx.root.join("day"));
    let rows = fx.rows();
    // A full share at the last file, the others whole already.
    let disk = Faulty {
        faults: vec![(Op::Copy, 2, Fault::Fail)],
        swap_before: Some((Op::RemoveFile, b"theirs".to_vec())),
        ..far_as(&fx, Far::Share)
    };
    let trash = trash_as(&fx, Far::Share);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let theirs = disk.swapped.borrow().clone();
    assert_eq!(theirs.map(|p| std::fs::read(p).ok()), None, "{outcome:?}");
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    // What was copied, as it was copied.
    let left = tree_of(&fx.root.join("far/day"));
    assert_eq!(left.len(), tree.len(), "{left:?}");
    let [ItemResult::NeedsRecovery(why)] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert_eq!(why, super::copy::LEFT_ON_SHARE);
    assert_eq!(fx.rows(), rows);
    // Its files deleted by the user, the item clears, and the folders it
    // made, empty, go with it (review of #675: a folder named there was
    // taken for one not as made, and stayed unfinished).
    super::copy_tests::deleted_by_the_user(&fx, outcome.items[0].operation);
    assert!(fx.root.join("far/day/sub").is_dir());
    fx.reopen();
    assert!(
        fx.recover_with(&far_as(&fx, Far::Share), &trash)
            .unfinished
            .is_empty()
    );
    assert_eq!(fx.rows(), rows);
    Ok(())
}

/// A copy left on a share whose original changed meanwhile, which may hold
/// the only bytes left, is never said to be the user's to delete, whatever
/// else is left there (review of #675).
#[test]
fn a_folder_copy_left_on_a_share_with_the_only_bytes_isnt_said_to_be_deletable() -> Result<()> {
    let mut fx = small();
    let (day, far) = (fx.root.join("day"), fx.root.join("far/day"));
    // As each file's copy is made, another app rewrites the originals of
    // those copied whole before it, and the share fills at the last.
    let disk = Faulty {
        faults: vec![(Op::Copy, 2, Fault::Fail)],
        watch: Some(Box::new(move |call: &super::tests::Call| {
            let super::tests::Call::Entry(at) = call else {
                return;
            };
            let Ok(rel) = at.strip_prefix(&far) else {
                return;
            };
            // Only as a file's copy is just made, before the share fills.
            if !std::fs::metadata(at).is_ok_and(|m| m.is_file() && m.len() == 0) {
                return;
            }
            for other in [".hidden", "a.NEF", "sub/b.NEF"] {
                let (original, copy) = (day.join(other), far.join(other));
                if Path::new(other) != rel
                    && std::fs::read(&copy).ok() == std::fs::read(&original).ok()
                    && copy.is_file()
                {
                    std::fs::write(&original, b"rewritten").unwrap();
                }
            }
        })),
        ..far_as(&fx, Far::Share)
    };
    let trash = trash_as(&fx, Far::Share);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let [ItemResult::NeedsRecovery(why)] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert_ne!(why, super::copy::LEFT_ON_SHARE);
    let rewritten = ["day/.hidden", "day/a.NEF", "day/sub/b.NEF"]
        .iter()
        .filter(|n| std::fs::read(fx.root.join(n)).unwrap() == b"rewritten")
        .count();
    assert!(rewritten > 0);
    Ok(())
}

/// A folder left on a share holding a file the move didn't make (another
/// computer's) is never said to be the user's to delete (review of #675).
#[test]
fn a_folder_left_on_a_share_with_another_file_in_it_isnt_said_to_be_deletable() -> Result<()> {
    let mut fx = small();
    let disk = Faulty {
        faults: vec![(Op::Copy, 2, Fault::Fail)],
        ..far_as(&fx, Far::Share)
    };
    let trash = trash_as(&fx, Far::Share);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let operation = outcome.items[0].operation;
    // Another computer puts a file in it; the user deletes the copies.
    let theirs = fx.root.join("far/day/sub/peer-only.txt");
    std::fs::write(&theirs, b"theirs")?;
    super::copy_tests::deleted_by_the_user(&fx, operation);
    fx.reopen();
    let recovery = fx.recover_with(&far_as(&fx, Far::Share), &trash);
    let [u] = &recovery.unfinished[..] else {
        panic!("{recovery:?}")
    };
    assert_ne!(u.reason, super::copy::LEFT_ON_SHARE);
    assert_eq!(std::fs::read(&theirs)?, b"theirs");
    Ok(())
}

/// A folder left on a share holding only the move's copies is the user's
/// to delete however the share lists their names (decomposed, as macOS's
/// smbfs does); not while it holds an AppleDouble file (`._name`), which
/// can't be told from another computer's file (review of #675).
#[test]
fn a_folder_left_on_a_share_is_deletable_however_its_names_are_listed() -> Result<()> {
    for apple_double in [false, true] {
        let mut fx = fixture(&["day/a.NEF", "day/.h\u{ed}dden", "day/sub/b.NEF", "far/"]);
        let disk = Faulty {
            faults: vec![(Op::Copy, 2, Fault::Fail)],
            apple_double,
            ..far_as(&fx, Far::Share)
        };
        let trash = trash_as(&fx, Far::Share);
        let op = to_far(&fx);
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        let why = match apple_double {
            false => super::copy::LEFT_ON_SHARE,
            true => super::recover::NOT_BACK,
        };
        assert_eq!(
            results(&outcome),
            [ItemResult::NeedsRecovery(why.into())],
            "AppleDouble {apple_double}"
        );
    }
    Ok(())
}

/// A partial copy of a folder's file left where its volume can't say what
/// it is is said to be the user's to delete only while its original still
/// begins with its bytes (review of #675).
#[test]
fn a_folder_copy_left_on_an_unknown_volume_isnt_said_deletable_once_its_originals_changed()
-> Result<()> {
    fn written(dir: &Path) -> bool {
        std::fs::read_dir(dir).is_ok_and(|names| {
            names.flatten().any(|e| {
                let p = e.path();
                match p.is_dir() {
                    true => written(&p),
                    false => std::fs::metadata(&p).is_ok_and(|m| m.len() > 0),
                }
            })
        })
    }
    let mut fx = small();
    let fail = Arc::new(AtomicBool::new(false));
    let (dir, day) = (fx.root.join("far"), fx.root.join("day"));
    let failing = fail.clone();
    // A full drive part way through the first file; as the copy is
    // undone, another app rewrites the originals and the drive can't be
    // told any more.
    let disk = Faulty {
        faults: vec![(Op::Copy, 0, Fault::Fail)],
        facts_fail: Some(fail.clone()),
        watch: Some(Box::new(move |call: &super::tests::Call| {
            if matches!(call, super::tests::Call::List(d) if d.starts_with(&dir))
                && !failing.load(std::sync::atomic::Ordering::Relaxed)
                && std::fs::read_dir(&dir).is_ok_and(|names| {
                    names.flatten().any(|e| {
                        e.file_name().to_string_lossy().ends_with(".rawmakase-part")
                            && written(&e.path())
                    })
                })
            {
                for name in [".hidden", "a.NEF", "sub/b.NEF"] {
                    std::fs::write(day.join(name), b"rewritten").unwrap();
                }
                failing.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        })),
        ..far(&fx)
    };
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &NoTrash, &disk, Cancel::NEVER);
    assert!(
        fail.load(std::sync::atomic::Ordering::Relaxed),
        "{outcome:?}"
    );
    let [ItemResult::NeedsRecovery(why)] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert_ne!(why, super::copy::LEFT_ON_SHARE);
    Ok(())
}
