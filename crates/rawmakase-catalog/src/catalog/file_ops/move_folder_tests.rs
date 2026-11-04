//! Moving a folder into another folder of its root: the same change as a
//! rename (one exclusive rename of its directory, the catalog's folders of
//! its subtree re-keyed) to another parent; what is refused; and what a
//! fault at any step leaves.
use super::recover_tests::{before, check};
use super::tests::{Fault, Faulty, Fx, NoTrash, Op, crashes, fixture, results};
use super::*;
use crate::catalog::{Catalog, RootId};
use crate::fsops;
use anyhow::Result;
use rawmakase_cancel::Cancel;

fn root(fx: &Fx) -> RootId {
    fx.cat.roots().unwrap()[0].0
}

fn move_folder(fx: &Fx, relative: &str, parent: &str) -> FileOp {
    FileOp::MoveFolder {
        root: root(fx),
        folder: relative.into(),
        to_root: root(fx),
        parent: parent.into(),
    }
}

/// The plan's refusal, as the user reads it.
fn refusal(fx: &Fx, op: FileOp) -> String {
    match plan(&fx.cat, op) {
        Err(FileOpError::Destination(why)) => why,
        other => panic!("not refused: {other:?}"),
    }
}

/// Each catalog folder's logical path, by id.
fn logical(fx: &Fx, folder: FolderId) -> Option<String> {
    fx.cat
        .db_for_tests()
        .query_row(
            "SELECT path FROM folder_paths WHERE folder=?1",
            [folder.0],
            |r| r.get(0),
        )
        .ok()
}

#[test]
fn a_folder_moves_into_another_with_its_subtree_and_its_photos_follow() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/a.xmp", "day/sub/b.NEF", "trips/c.NEF"]);
    let (a, b) = (fx.photo("day/a.NEF"), fx.photo("day/sub/b.NEF"));
    let (day, sub) = (fx.folder("day"), fx.folder("day/sub"));
    let (dir_id, file_id) = (
        fsops::entry(&fx.root.join("day"))?.id,
        fsops::entry(&fx.root.join("day/a.NEF"))?.id,
    );
    let op = move_folder(&fx, "day", "trips");
    let plan = fx.plan(op.clone());
    assert_eq!(plan.summary().photos, 2);
    assert_eq!(plan.summary().folder.as_deref(), Some("day"));
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert!(!fx.root.join("day").exists());
    assert_eq!(fsops::entry(&fx.root.join("trips/day"))?.id, dir_id);
    assert_eq!(fsops::entry(&fx.root.join("trips/day/a.NEF"))?.id, file_id);
    assert!(fx.root.join("trips/day/a.xmp").is_file());
    let rows = fx.rows();
    assert_eq!(rows[&a], "trips/day/a.NEF");
    assert_eq!(rows[&b], "trips/day/sub/b.NEF");
    assert_eq!(logical(&fx, day).as_deref(), Some("trips/day"));
    assert_eq!(logical(&fx, sub).as_deref(), Some("trips/day/sub"));
    // And back up to the root folder.
    let outcome = fx.run(
        move_folder(&fx, "trips/day", ""),
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(fx.rows()[&b], "day/sub/b.NEF");
    Ok(())
}

#[test]
fn a_folder_moves_into_one_the_catalog_has_no_folder_of_its_own_for() -> Result<()> {
    // Neither the root nor 2024 is a catalog folder.
    let mut fx = fixture(&["2024/day1/a.NEF", "2025/day2/b.NEF"]);
    let a = fx.photo("2024/day1/a.NEF");
    let outcome = fx.run(
        move_folder(&fx, "2024/day1", "2025"),
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(fx.rows()[&a], "2025/day1/a.NEF");
    Ok(())
}

#[test]
fn a_folder_is_never_moved_into_itself_across_roots_or_onto_a_name_taken() -> Result<()> {
    let mut fx = fixture(&[
        "top.NEF",
        "day/a.NEF",
        "day/sub/b.NEF",
        "trips/day/",
        "other/x.NEF",
    ]);
    let r = root(&fx);
    let into = |fx: &Fx, parent: &str| refusal(fx, move_folder(fx, "day", parent));
    assert!(into(&fx, "day").contains("into itself"));
    assert!(into(&fx, "day/sub").contains("into itself"));
    assert!(into(&fx, "DAY/sub").contains("into itself"));
    assert_eq!(into(&fx, ""), "It is in that folder already.");
    // A folder of that name there, on disk and in the catalog.
    assert!(
        into(&fx, "trips").contains("already there"),
        "{}",
        into(&fx, "trips")
    );
    // A missing catalog folder of that name, or below it.
    let root_path = fx.root.clone();
    std::fs::create_dir(fx.root.join("gone"))?;
    std::fs::create_dir(fx.root.join("deep"))?;
    fx.add_row(&root_path, "gone/day", "x.NEF");
    fx.add_row(&root_path, "deep/day/below", "y.NEF");
    for parent in ["gone", "deep"] {
        assert!(
            into(&fx, parent).contains("catalog has a folder"),
            "{parent}: {}",
            into(&fx, parent)
        );
    }
    // Another root.
    let elsewhere = fx.dir.path().canonicalize()?.join("elsewhere");
    std::fs::create_dir(&elsewhere)?;
    std::fs::write(elsewhere.join("e.NEF"), b"e")?;
    fx.cat.add_folder(&elsewhere)?;
    let other = fx
        .cat
        .roots()?
        .into_iter()
        .map(|(id, _, _)| id)
        .find(|id| *id != r)
        .unwrap();
    // A folder is named in its own root: the other has no "day".
    let why = refusal(
        &fx,
        FileOp::MoveFolder {
            root: other,
            folder: "day".into(),
            to_root: other,
            parent: String::new(),
        },
    );
    assert!(why.contains("no longer in the catalog"), "{why}");
    // The root folder itself.
    let why = refusal(
        &fx,
        FileOp::MoveFolder {
            root: r,
            folder: String::new(),
            to_root: r,
            parent: "other".into(),
        },
    );
    assert!(why.contains("root folder"), "{why}");
    assert!(fx.root.join("day/a.NEF").is_file());
    Ok(())
}

#[test]
fn a_subtree_or_destination_with_a_location_of_its_own_is_refused() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/sub/b.NEF", "trips/c.NEF"]);
    let sub = fx.folder("day/sub");
    fx.cat.relink_folder(sub, &fx.root.join("day/sub"))?;
    let why = refusal(&fx, move_folder(&fx, "day", "trips"));
    assert!(why.contains("located on its own"), "{why}");
    Ok(())
}

#[test]
fn a_folder_another_root_is_inside_is_refused() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/sub/b.NEF", "trips/c.NEF"]);
    let sub = fx.root.join("day/sub");
    fx.add_row(&sub, "", "b.NEF");
    let why = refusal(&fx, move_folder(&fx, "day", "trips"));
    assert_eq!(
        why,
        "Another folder or root of the catalog is inside it on disk."
    );
    Ok(())
}

#[test]
fn a_destination_taken_between_plan_and_execute_is_never_merged_into() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "trips/c.NEF"]);
    let a = fx.photo("day/a.NEF");
    let plan = fx.plan(move_folder(&fx, "day", "trips"));
    std::fs::create_dir(fx.root.join("trips/day"))?;
    std::fs::write(fx.root.join("trips/day/theirs.txt"), b"theirs")?;
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
    assert_eq!(fx.rows()[&a], "day/a.NEF");
    assert_eq!(
        std::fs::read(fx.root.join("trips/day/theirs.txt"))?,
        b"theirs"
    );
    Ok(())
}

#[test]
fn a_destination_located_elsewhere_since_planning_is_left_alone() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "trips/c.NEF"]);
    let a = fx.photo("day/a.NEF");
    let plan = fx.plan(move_folder(&fx, "day", "trips"));
    let elsewhere = fx.dir.path().canonicalize()?.join("elsewhere");
    std::fs::create_dir(&elsewhere)?;
    std::fs::copy(fx.root.join("trips/c.NEF"), elsewhere.join("c.NEF"))?;
    let trips = fx.folder("trips");
    let mut other = Catalog::open(&fx.catalog)?;
    other.relink_folder(trips, &elsewhere)?;
    drop(other);
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
    assert!(fx.root.join("day/a.NEF").is_file());
    assert_eq!(fx.rows()[&a], "day/a.NEF");
    Ok(())
}

fn move_files() -> Fx {
    fixture(&["day/a.NEF", "day/a.xmp", "day/sub/b.NEF", "trips/c.NEF"])
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
/// one it is at, or lists it unfinished.
fn one_place(fx: &Fx, day: FolderId, recovery: &Recovery, case: &str) {
    let (old, new) = (fx.root.join("day"), fx.root.join("trips/day"));
    assert!(old.is_dir() != new.is_dir(), "{case}");
    let moved = logical(fx, day).as_deref() == Some("trips/day");
    if moved != new.is_dir() {
        assert!(!recovery.unfinished.is_empty(), "{case}: unreported");
    }
}

#[test]
fn every_fault_at_every_step_of_a_folder_move_ends_proven() {
    let mut clean = move_files();
    let counting = Faulty::default();
    let op = move_folder(&clean, "day", "trips");
    let _ = clean.run(op, &NoTrash, &counting);
    let steps = [
        (Op::Rename, counting.count(Op::Rename)),
        (Op::Sync, counting.count(Op::Sync)),
    ];
    assert_eq!(steps[0].1, 1);
    // Both parents are synced.
    assert_eq!(steps[1].1, 2);
    let mut runs = 0;
    for (at, count) in steps {
        for nth in 0..count {
            for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                let case = format!("{at:?} {nth} {fault:?}");
                let mut fx = move_files();
                let day = fx.folder("day");
                let was = before(&fx);
                let disk = Faulty::at(at, nth, fault);
                let op = move_folder(&fx, "day", "trips");
                let (batch, recovery) = scenario(&mut fx, op, &disk);
                check(&fx, &was, &recovery, &batch);
                one_place(&fx, day, &recovery, &case);
                for u in &recovery.unfinished {
                    let back =
                        resolve(&mut fx.cat, u.operation, Resolution::PutBack, &NoTrash).unwrap();
                    assert!(
                        matches!(back.result, ItemResult::Unchanged(_)),
                        "{case}: {back:?}"
                    );
                }
                check(&fx, &was, &Recovery::default(), &batch);
                one_place(&fx, day, &Recovery::default(), &case);
                runs += 1;
            }
        }
    }
    assert!(runs >= 9);
}

#[test]
fn every_failed_commit_of_a_folder_move_ends_proven_and_a_restart_finishes_it() -> Result<()> {
    for when in [
        "BEFORE INSERT ON file_operations",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'moved'",
        "BEFORE UPDATE ON folders",
        "BEFORE INSERT ON folder_paths",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'",
    ] {
        let mut fx = move_files();
        let was = before(&fx);
        let day = fx.folder("day");
        fx.fail_journal(when);
        let plan = fx.plan(move_folder(&fx, "day", "trips"));
        let batch = plan.batch().clone();
        let outcome = execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &NoTrash,
            Cancel::NEVER,
            &Faulty::default(),
        )?;
        let first = outcome.items.first().map(|i| i.result.clone());
        let journalled = !when.contains("INSERT ON file_operations");
        let moved = journalled && !when.contains("members");
        match (journalled, moved) {
            (false, _) => assert!(outcome.items.is_empty()),
            (true, false) => assert!(
                matches!(first, Some(ItemResult::Unchanged(Some(_)))),
                "{first:?}"
            ),
            (true, true) => assert!(
                matches!(first, Some(ItemResult::NeedsRecovery(_))),
                "{when}: {first:?}"
            ),
        }
        fx.reopen();
        let recovery = fx.recover(&disk::Real);
        assert!(recovery.unfinished.is_empty(), "{when}: {recovery:?}");
        check(&fx, &was, &recovery, &batch);
        if moved {
            assert_eq!(
                results(&super::outcome(&fx.cat, &batch)?),
                [ItemResult::FolderMoved],
                "{when}"
            );
            assert_eq!(logical(&fx, day).as_deref(), Some("trips/day"));
        }
    }
    Ok(())
}

/// Photos two levels down: the catalog has folders for `2024/day1` and
/// `2024/day2`, none for `2024`.
fn between_files() -> Fx {
    fixture(&[
        "2024/day1/a.NEF",
        "2024/day1/a.xmp",
        "2024/day2/b.NEF",
        "trips/c.NEF",
    ])
}

/// Whether the catalog has a folder of its own at `relative`.
fn has_folder(fx: &Fx, relative: &str) -> bool {
    let path = fx.root.join(relative);
    fx.cat.folders().unwrap().iter().any(|f| f.path == path)
}

#[test]
fn a_folder_the_catalog_has_no_folder_of_its_own_for_moves_with_those_below() -> Result<()> {
    let mut fx = between_files();
    assert!(!has_folder(&fx, "2024"));
    let (a, b) = (fx.photo("2024/day1/a.NEF"), fx.photo("2024/day2/b.NEF"));
    let (day1, day2) = (fx.folder("2024/day1"), fx.folder("2024/day2"));
    let dir_id = fsops::entry(&fx.root.join("2024"))?.id;
    let op = move_folder(&fx, "2024", "trips");
    assert_eq!(fx.plan(op.clone()).summary().photos, 2);
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert!(!fx.root.join("2024").exists());
    assert_eq!(fsops::entry(&fx.root.join("trips/2024"))?.id, dir_id);
    assert!(fx.root.join("trips/2024/day1/a.xmp").is_file());
    let rows = fx.rows();
    assert_eq!(rows[&a], "trips/2024/day1/a.NEF");
    assert_eq!(rows[&b], "trips/2024/day2/b.NEF");
    assert_eq!(logical(&fx, day1).as_deref(), Some("trips/2024/day1"));
    assert_eq!(logical(&fx, day2).as_deref(), Some("trips/2024/day2"));
    // Still no folder of its own, and back up to the root folder.
    assert!(!has_folder(&fx, "trips/2024"));
    let outcome = fx.run(
        move_folder(&fx, "trips/2024", ""),
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(fx.rows()[&b], "2024/day2/b.NEF");
    Ok(())
}

#[test]
fn a_folder_with_no_catalog_folder_at_or_below_it_is_refused() -> Result<()> {
    let fx = fixture(&["day/a.NEF", "trips/c.NEF"]);
    // A folder on disk only, one nowhere, and a name only the start of one.
    std::fs::create_dir(fx.root.join("disk only"))?;
    for folder in ["disk only", "nowhere", "da"] {
        let why = refusal(&fx, move_folder(&fx, folder, "trips"));
        assert!(why.contains("no longer in the catalog"), "{folder}: {why}");
    }
    assert!(fx.root.join("disk only").is_dir());
    Ok(())
}

#[test]
fn every_fault_at_every_step_moving_a_folder_with_no_catalog_folder_ends_proven() {
    let mut runs = 0;
    for at in [Op::Rename, Op::Sync] {
        for nth in 0..2 {
            for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                let case = format!("{at:?} {nth} {fault:?}");
                let mut fx = between_files();
                let day1 = fx.folder("2024/day1");
                let was = before(&fx);
                let disk = Faulty::at(at, nth, fault);
                let op = move_folder(&fx, "2024", "trips");
                let (batch, recovery) = scenario(&mut fx, op, &disk);
                check(&fx, &was, &recovery, &batch);
                let (old, new) = (fx.root.join("2024"), fx.root.join("trips/2024"));
                assert!(old.is_dir() != new.is_dir(), "{case}");
                let moved = logical(&fx, day1).as_deref() == Some("trips/2024/day1");
                if moved != new.is_dir() {
                    assert!(!recovery.unfinished.is_empty(), "{case}: unreported");
                }
                for u in &recovery.unfinished {
                    let back =
                        resolve(&mut fx.cat, u.operation, Resolution::PutBack, &NoTrash).unwrap();
                    assert!(
                        matches!(back.result, ItemResult::Unchanged(_)),
                        "{case}: {back:?}"
                    );
                }
                check(&fx, &was, &Recovery::default(), &batch);
                runs += 1;
            }
        }
    }
    assert!(runs >= 9);
}
