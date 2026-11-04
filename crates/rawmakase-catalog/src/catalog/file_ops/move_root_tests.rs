//! Moving a folder into a folder of another root on the same drive (#572):
//! still one exclusive rename of its directory; in the catalog its subtree's
//! folders move to the other root, keeping their ids. What is refused, and
//! what a fault at any step leaves.
use super::recover_tests::{before, check};
use super::tests::{Fault, Faulty, Fx, NoTrash, Op, bytes, crashes, fixture, results};
use super::*;
use crate::catalog::{Catalog, RootId};
use crate::fsops;
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};

/// The fixture's root, and a second root `elsewhere/` beside it holding
/// `files` (a name ending in `/` a folder with a photo `keep.dng`).
fn two_roots(files: &[&str]) -> (Fx, RootId, PathBuf) {
    let mut fx = fixture(&["day/a.NEF", "day/a.xmp", "day/sub/b.NEF", "trips/c.NEF"]);
    let other = fx.dir.path().canonicalize().unwrap().join("elsewhere");
    for f in files {
        let path = other.join(f);
        if f.ends_with('/') {
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("keep.dng"), bytes(&format!("{f}keep.dng"))).unwrap();
        } else {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes(f)).unwrap();
        }
    }
    fx.cat.add_folder(&other).unwrap();
    let mine = fx.root_id();
    let id = fx
        .cat
        .roots()
        .unwrap()
        .into_iter()
        .map(|(id, _, _)| id)
        .find(|id| *id != mine)
        .unwrap();
    (fx, id, other)
}

fn across(fx: &Fx, folder: &str, to_root: RootId, parent: &str) -> FileOp {
    FileOp::MoveFolder {
        root: fx.root_id(),
        folder: folder.into(),
        to_root,
        parent: parent.into(),
    }
}

fn refusal(fx: &Fx, op: FileOp) -> String {
    match plan(&fx.cat, op) {
        Err(FileOpError::Destination(why)) => why,
        other => panic!("not refused: {other:?}"),
    }
}

/// Catalog folder `folder`'s root, logical path and legacy relative path.
fn place(fx: &Fx, folder: FolderId) -> (RootId, String, String) {
    fx.cat
        .db_for_tests()
        .query_row(
            "SELECT f.root, p.path, f.relative_path FROM folders f
             JOIN folder_paths p ON p.folder=f.id WHERE f.id=?1",
            [folder.0],
            |r| Ok((RootId(r.get(0)?), r.get(1)?, r.get(2)?)),
        )
        .unwrap()
}

/// Where the catalog says photo `id` is.
fn path_of(fx: &Fx, id: PhotoId) -> PathBuf {
    fx.cat
        .photos()
        .unwrap()
        .into_iter()
        .find(|p| p.id == id)
        .unwrap()
        .path
}

fn native(logical: &str) -> String {
    logical
        .split('/')
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn a_folder_moves_into_a_folder_of_another_root_and_keeps_its_ids() -> Result<()> {
    let (mut fx, other, there) = two_roots(&["trips/e.NEF"]);
    let (a, b) = (fx.photo("day/a.NEF"), fx.photo("day/sub/b.NEF"));
    let (day, sub) = (fx.folder("day"), fx.folder("day/sub"));
    let (dir_id, file_id) = (
        fsops::entry(&fx.root.join("day"))?.id,
        fsops::entry(&fx.root.join("day/a.NEF"))?.id,
    );
    let op = across(&fx, "day", other, "trips");
    let plan = fx.plan(op.clone());
    assert_eq!(plan.summary().photos, 2);
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert!(!fx.root.join("day").exists());
    assert_eq!(fsops::entry(&there.join("trips/day"))?.id, dir_id);
    assert_eq!(fsops::entry(&there.join("trips/day/a.NEF"))?.id, file_id);
    assert!(there.join("trips/day/a.xmp").is_file());
    assert_eq!(path_of(&fx, a), there.join("trips/day/a.NEF"));
    assert_eq!(path_of(&fx, b), there.join("trips/day/sub/b.NEF"));
    assert_eq!(
        place(&fx, day),
        (other, "trips/day".into(), native("trips/day"))
    );
    assert_eq!(
        place(&fx, sub),
        (other, "trips/day/sub".into(), native("trips/day/sub"))
    );
    // And into the first root's root folder again.
    let mine = fx.root_id();
    let back = FileOp::MoveFolder {
        root: other,
        folder: "trips/day".into(),
        to_root: mine,
        parent: String::new(),
    };
    let outcome = fx.run(back, &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(path_of(&fx, b), fx.root.join("day/sub/b.NEF"));
    assert_eq!(place(&fx, day), (mine, "day".into(), native("day")));
    Ok(())
}

#[test]
fn a_folder_moves_into_another_roots_root_folder() -> Result<()> {
    let (mut fx, other, there) = two_roots(&["e.NEF"]);
    let a = fx.photo("day/a.NEF");
    let outcome = fx.run(across(&fx, "day", other, ""), &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(path_of(&fx, a), there.join("day/a.NEF"));
    assert_eq!(place(&fx, fx.folder_at(&there.join("day"))).1, "day");
    Ok(())
}

#[test]
fn a_move_into_another_root_is_refused_where_a_move_within_one_would_be() -> Result<()> {
    let (mut fx, other, there) = two_roots(&["trips/day/", "busy/x.NEF"]);
    // A folder of that name there, on disk and in the catalog.
    let why = refusal(&fx, across(&fx, "day", other, "trips"));
    assert!(why.contains("already there"), "{why}");
    // A missing catalog folder of that name there, or below it.
    std::fs::create_dir(there.join("gone"))?;
    fx.add_row(&there, "gone/day/below", "y.NEF");
    let why = refusal(&fx, across(&fx, "day", other, "gone"));
    assert!(why.contains("catalog has a folder"), "{why}");
    // A folder in it located on its own.
    let sub = fx.folder("day/sub");
    fx.cat.relink_folder(sub, &fx.root.join("day/sub"))?;
    let why = refusal(&fx, across(&fx, "day", other, "busy"));
    assert!(why.contains("located on its own"), "{why}");
    assert!(fx.root.join("day/a.NEF").is_file());
    Ok(())
}

#[test]
fn a_destination_root_that_is_offline_is_refused() -> Result<()> {
    let (mut fx, other, _) = two_roots(&["trips/e.NEF"]);
    let missing = fx.dir.path().canonicalize()?.join("unplugged");
    std::fs::create_dir(&missing)?;
    fx.cat.relink_root(other, &missing)?;
    std::fs::remove_dir(&missing)?;
    let why = refusal(&fx, across(&fx, "day", other, "trips"));
    assert!(why.contains("offline or missing"), "{why}");
    Ok(())
}

#[test]
fn a_subtree_anchoring_another_root_never_moves_to_another() -> Result<()> {
    let (mut fx, other, _) = two_roots(&["trips/e.NEF"]);
    let sub = fx.root.join("day/sub");
    fx.add_row(&sub, "", "b.NEF");
    let why = refusal(&fx, across(&fx, "day", other, "trips"));
    assert_eq!(
        why,
        "Another folder or root of the catalog is inside it on disk."
    );
    Ok(())
}

#[test]
fn a_root_never_moves_into_another() -> Result<()> {
    let (fx, other, _) = two_roots(&["trips/e.NEF"]);
    let why = refusal(&fx, across(&fx, "", other, "trips"));
    assert!(why.contains("root folder"), "{why}");
    Ok(())
}

#[test]
fn a_same_root_move_journals_as_before_and_an_old_item_reads() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "trips/c.NEF"]);
    let mine = fx.root_id();
    let op = FileOp::MoveFolder {
        root: mine,
        folder: "day".into(),
        to_root: mine,
        parent: "trips".into(),
    };
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    let json: String = fx.cat.db_for_tests().query_row(
        "SELECT preconditions FROM file_operations WHERE id=?1",
        [outcome.items[0].operation.0],
        |r| r.get(0),
    )?;
    // 0.4.2's journal, unchanged: no field it doesn't know.
    assert!(!json.contains("into"), "{json}");
    let state: tree::TreeState = serde_json::from_str(&json)?;
    assert_eq!(state.into, None);
    Ok(())
}

/// Plans `op`, runs it on `disk` (which may crash), restarts and recovers.
fn scenario(fx: &mut Fx, op: FileOp, disk: &Faulty) -> (BatchId, Recovery) {
    let plan = fx.plan(op);
    let batch = plan.batch().clone();
    crashes(|| {
        drop(
            execute::execute_with(&mut fx.cat, plan.confirm(), &NoTrash, Cancel::NEVER, disk)
                .unwrap(),
        );
    });
    fx.reopen();
    let recovery = fx.recover(&disk::Real);
    (batch, recovery)
}

/// The folder is on disk at exactly one place, and the catalog says the
/// one it is at (its root too), or lists it unfinished.
fn one_place(fx: &Fx, day: FolderId, there: &Path, recovery: &Recovery, case: &str) {
    let (old, new) = (fx.root.join("day"), there.join("trips/day"));
    assert!(old.is_dir() != new.is_dir(), "{case}");
    let (root, logical, _) = place(fx, day);
    let moved = logical == "trips/day";
    assert_eq!(moved, root != fx.root_id(), "{case}: half re-keyed");
    if moved != new.is_dir() {
        assert!(!recovery.unfinished.is_empty(), "{case}: unreported");
    }
}

#[test]
fn every_fault_at_every_step_of_a_move_into_another_root_ends_proven() {
    let mut runs = 0;
    for (at, count) in [(Op::Rename, 1), (Op::Sync, 2)] {
        for nth in 0..count {
            for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                let case = format!("{at:?} {nth} {fault:?}");
                let (mut fx, other, there) = two_roots(&["trips/e.NEF"]);
                let day = fx.folder("day");
                let was = before(&fx);
                let disk = Faulty::at(at, nth, fault);
                let op = across(&fx, "day", other, "trips");
                let (batch, recovery) = scenario(&mut fx, op, &disk);
                check(&fx, &was, &recovery, &batch);
                one_place(&fx, day, &there, &recovery, &case);
                for u in &recovery.unfinished {
                    let back =
                        resolve(&mut fx.cat, u.operation, Resolution::PutBack, &NoTrash).unwrap();
                    assert!(
                        matches!(back.result, ItemResult::Unchanged(_)),
                        "{case}: {back:?}"
                    );
                }
                check(&fx, &was, &Recovery::default(), &batch);
                one_place(&fx, day, &there, &Recovery::default(), &case);
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 9);
}

#[test]
fn every_failed_commit_of_a_move_into_another_root_is_finished_by_a_restart() -> Result<()> {
    for when in [
        "BEFORE UPDATE ON folders",
        "BEFORE INSERT ON folder_paths",
        "BEFORE UPDATE ON folder_paths",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'",
    ] {
        let (mut fx, other, there) = two_roots(&["trips/e.NEF"]);
        let was = before(&fx);
        let day = fx.folder("day");
        fx.fail_journal(when);
        let plan = fx.plan(across(&fx, "day", other, "trips"));
        let batch = plan.batch().clone();
        let outcome = execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &NoTrash,
            Cancel::NEVER,
            &Faulty::default(),
        )?;
        assert!(
            matches!(
                results(&outcome)[..],
                [ItemResult::NeedsRecovery(_)] | [ItemResult::FolderMoved]
            ),
            "{when}: {outcome:?}"
        );
        fx.reopen();
        let recovery = fx.recover(&disk::Real);
        assert!(recovery.unfinished.is_empty(), "{when}: {recovery:?}");
        check(&fx, &was, &recovery, &batch);
        assert_eq!(
            place(&fx, day),
            (other, "trips/day".into(), native("trips/day")),
            "{when}"
        );
        assert!(there.join("trips/day/a.NEF").is_file());
    }
    Ok(())
}

#[test]
fn a_destination_root_relocated_since_planning_is_left_alone() -> Result<()> {
    let (mut fx, other, there) = two_roots(&["trips/e.NEF"]);
    let a = fx.photo("day/a.NEF");
    let plan = fx.plan(across(&fx, "day", other, "trips"));
    let moved = fx.dir.path().canonicalize()?.join("moved");
    std::fs::rename(&there, &moved)?;
    let mut again = Catalog::open(&fx.catalog)?;
    again.relink_root(other, &moved)?;
    drop(again);
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert!(
        matches!(results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
        "{outcome:?}"
    );
    assert_eq!(path_of(&fx, a), fx.root.join("day/a.NEF"));
    Ok(())
}

#[test]
fn a_move_into_another_root_over_the_same_directory_is_refused() -> Result<()> {
    // The other root is located over this one's directory: its root folder
    // is where `day` is already.
    let (mut fx, other, _) = two_roots(&["trips/e.NEF"]);
    let here = fx.root.clone();
    fx.cat.relink_root(other, &here)?;
    let why = refusal(&fx, across(&fx, "day", other, ""));
    assert_eq!(why, "It is in that folder already.");
    assert!(fx.root.join("day/a.NEF").is_file());
    Ok(())
}
