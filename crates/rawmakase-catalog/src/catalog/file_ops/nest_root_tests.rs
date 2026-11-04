//! Moving a root folder into a folder of another root (#586): on its drive
//! one exclusive rename of its directory, onto another a verified copy of
//! its whole tree; either way its catalog folders join the other root with
//! their ids, and the root leaves the catalog's roots in the same
//! transaction. What is refused, and what a fault at any step leaves.
use super::copy_tests::{Far, trash_as};
use super::recover_tests::{Before, before, check};
use super::tests::{FakeTrash, Fault, Faulty, Fx, NoTrash, Op, bytes, crashes, fixture, results};
use super::*;
use crate::catalog::{Catalog, RootId};
use crate::fsops;
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The fixture's root `trip/` (a photo in it, a folder with a sidecar, a
/// hidden file and a folder in it), and a second root `elsewhere/` beside
/// it holding `trips/`.
fn two_roots() -> (Fx, RootId, PathBuf) {
    let mut fx = fixture(&[
        "a.NEF",
        "day/b.NEF",
        "day/b.xmp",
        "day/.hidden",
        "day/sub/c.NEF",
    ]);
    let other = fx.dir.path().canonicalize().unwrap().join("elsewhere");
    std::fs::create_dir_all(other.join("trips")).unwrap();
    std::fs::write(other.join("trips/e.NEF"), bytes("trips/e.NEF")).unwrap();
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

/// Moves the fixture's root into logical folder `parent` of `to_root`.
fn nest(fx: &Fx, to_root: RootId, parent: &str) -> FileOp {
    FileOp::MoveRoot {
        root: fx.root_id(),
        to_root,
        parent: parent.into(),
    }
}

fn refusal(fx: &Fx, op: FileOp) -> String {
    refusal_on(fx, op, &disk::Real)
}

fn refusal_on(fx: &Fx, op: FileOp, disk: &dyn disk::Disk) -> String {
    match fx.plan_on(op, disk) {
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

fn native(logical: &str) -> String {
    logical
        .split('/')
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
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

/// The catalog's roots, by id.
fn roots(fx: &Fx) -> Vec<RootId> {
    fx.cat
        .roots()
        .unwrap()
        .into_iter()
        .map(|(id, _, _)| id)
        .collect()
}

/// How many locations, on any computer, root `root` has.
fn locations_of(fx: &Fx, root: RootId) -> i64 {
    fx.cat
        .db_for_tests()
        .query_row(
            "SELECT count(*) FROM folder_locations WHERE root=?1",
            [root.0],
            |r| r.get(0),
        )
        .unwrap()
}

/// Gives root `root` a location on another computer too.
fn located_elsewhere(fx: &Fx, root: RootId, relative: &str, path: &str) {
    let db = fx.cat.db_for_tests();
    db.execute(
        "INSERT OR IGNORE INTO computers(id, name) VALUES ('other', 'Other')",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO folder_locations(root, relative_path, computer, path)
         VALUES (?1, ?2, 'other', ?3)",
        rusqlite::params![root.0, relative, path],
    )
    .unwrap();
}

/// The kind of the journalled item of `batch`, as stored.
fn stored_kind(fx: &Fx, batch: &BatchId) -> String {
    fx.cat
        .db_for_tests()
        .query_row(
            "SELECT kind FROM file_operations WHERE batch=?1",
            [batch.to_string()],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn a_root_moves_into_a_folder_of_another_root_and_leaves_the_roots() -> Result<()> {
    let (mut fx, other, there) = two_roots();
    let mine = fx.root_id();
    let (a, b, c) = (
        fx.photo("a.NEF"),
        fx.photo("day/b.NEF"),
        fx.photo("day/sub/c.NEF"),
    );
    let (top, day, sub) = (fx.folder(""), fx.folder("day"), fx.folder("day/sub"));
    located_elsewhere(&fx, mine, "", "/Volumes/Other/trip");
    let (dir_id, file_id) = (
        fsops::entry(&fx.root)?.id,
        fsops::entry(&fx.root.join("day/b.NEF"))?.id,
    );
    let op = nest(&fx, other, "trips");
    let plan = fx.plan(op);
    assert_eq!(plan.summary().photos, 3);
    assert_eq!(plan.summary().folder.as_deref(), Some("trip"));
    assert!(plan.summary().across.is_none());
    let batch = plan.batch().clone();
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(stored_kind(&fx, &batch), "move_root");
    let moved = there.join("trips/trip");
    assert!(!fx.root.exists());
    assert_eq!(fsops::entry(&moved)?.id, dir_id);
    assert_eq!(fsops::entry(&moved.join("day/b.NEF"))?.id, file_id);
    assert!(moved.join("day/b.xmp").is_file());
    assert!(moved.join("day/.hidden").is_file());
    assert_eq!(path_of(&fx, a), moved.join("a.NEF"));
    assert_eq!(path_of(&fx, b), moved.join("day/b.NEF"));
    assert_eq!(path_of(&fx, c), moved.join("day/sub/c.NEF"));
    // Its folders keep their ids, in the other root now.
    assert_eq!(
        place(&fx, top),
        (other, "trips/trip".into(), native("trips/trip"))
    );
    assert_eq!(
        place(&fx, day),
        (other, "trips/trip/day".into(), native("trips/trip/day"))
    );
    assert_eq!(
        place(&fx, sub),
        (
            other,
            "trips/trip/day/sub".into(),
            native("trips/trip/day/sub")
        )
    );
    // The root left, with its locations on every computer.
    assert_eq!(roots(&fx), [other]);
    assert_eq!(locations_of(&fx, mine), 0);
    // The catalog opens again as it was left.
    fx.reopen();
    assert_eq!(path_of(&fx, c), moved.join("day/sub/c.NEF"));
    Ok(())
}

#[test]
fn a_root_moves_into_another_roots_root_folder() -> Result<()> {
    let (mut fx, other, there) = two_roots();
    let b = fx.photo("day/b.NEF");
    let top = fx.folder("");
    let outcome = fx.run(nest(&fx, other, ""), &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(path_of(&fx, b), there.join("trip/day/b.NEF"));
    assert_eq!(place(&fx, top), (other, "trip".into(), native("trip")));
    assert_eq!(roots(&fx), [other]);
    Ok(())
}

#[test]
fn a_root_with_no_catalog_folder_left_moves_and_leaves_the_roots() -> Result<()> {
    // Its one folder moved to the other root, as #572 lets it: the root
    // stays, empty, holding a file the catalog doesn't know.
    let mut fx = fixture(&["day/b.NEF", "notes.txt"]);
    let there = fx.dir.path().canonicalize()?.join("elsewhere");
    std::fs::create_dir_all(there.join("trips"))?;
    std::fs::write(there.join("trips/e.NEF"), bytes("trips/e.NEF"))?;
    fx.cat.add_folder(&there)?;
    let mine = fx.root_id();
    let other = roots(&fx).into_iter().find(|id| *id != mine).unwrap();
    let day = FileOp::MoveFolder {
        root: mine,
        folder: "day".into(),
        to_root: other,
        parent: String::new(),
    };
    assert_eq!(
        results(&fx.run(day, &NoTrash, &Faulty::default())),
        [ItemResult::FolderMoved]
    );
    assert!(roots(&fx).contains(&mine));
    let outcome = fx.run(nest(&fx, other, "trips"), &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert!(there.join("trips/trip/notes.txt").is_file());
    assert!(!fx.root.exists());
    assert_eq!(roots(&fx), [other]);
    Ok(())
}

#[test]
fn a_root_never_moves_into_itself_or_below_itself() -> Result<()> {
    let (mut fx, other, _) = two_roots();
    let mine = fx.root_id();
    for parent in ["", "day", "day/sub"] {
        let why = refusal(&fx, nest(&fx, mine, parent));
        assert!(why.contains("into itself"), "{parent}: {why}");
    }
    // The other root located inside it on disk: dropping it there is
    // dropping it below itself.
    let inside = fx.root.join("day");
    fx.cat.relink_root(other, &inside)?;
    let why = refusal(&fx, nest(&fx, other, ""));
    assert!(why.contains("into itself"), "{why}");
    assert!(fx.root.join("day/b.NEF").is_file());
    Ok(())
}

#[test]
fn a_root_holding_another_root_or_a_folder_located_on_its_own_is_refused() -> Result<()> {
    // Another root over a folder in it.
    let (mut fx, other, _) = two_roots();
    let sub = fx.root.join("day/sub");
    fx.add_row(&sub, "", "c.NEF");
    let why = refusal(&fx, nest(&fx, other, "trips"));
    assert_eq!(
        why,
        "Another folder or root of the catalog is inside it on disk."
    );
    // A folder in it located on its own, here or on another computer.
    let (mut fx, other, _) = two_roots();
    let sub = fx.folder("day/sub");
    fx.cat.relink_folder(sub, &fx.root.join("day/sub"))?;
    let why = refusal(&fx, nest(&fx, other, "trips"));
    assert!(why.contains("located on its own"), "{why}");
    let (fx, other, _) = two_roots();
    located_elsewhere(&fx, fx.root_id(), "day", "/Volumes/Other/trip/day");
    let why = refusal(&fx, nest(&fx, other, "trips"));
    assert!(why.contains("located on its own"), "{why}");
    // Where it is located on another computer is its own: it goes with it.
    let (mut fx, other, there) = two_roots();
    located_elsewhere(&fx, fx.root_id(), "", "/Volumes/Other/trip");
    let outcome = fx.run(nest(&fx, other, "trips"), &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert!(there.join("trips/trip/a.NEF").is_file());
    Ok(())
}

#[test]
fn a_root_holding_the_catalog_is_refused() -> Result<()> {
    let (fx, _, there) = two_roots();
    // A second catalog, kept in the fixture's root folder.
    let inner = fx.root.join("Inner.rawmakase");
    let mut cat = Catalog::create(&inner)?;
    cat.add_folder(&fx.root)?;
    cat.add_folder(&there)?;
    let (mut mine, mut theirs) = (None, None);
    for (id, original, _) in cat.roots()? {
        match Path::new(&original) == fx.root {
            true => mine = Some(id),
            false => theirs = Some(id),
        }
    }
    let op = FileOp::MoveRoot {
        root: mine.unwrap(),
        to_root: theirs.unwrap(),
        parent: "trips".into(),
    };
    match plan(&cat, op) {
        Err(FileOpError::Destination(why)) => {
            assert!(why.contains("holding the catalog"), "{why}")
        }
        other => panic!("not refused: {other:?}"),
    }
    Ok(())
}

#[test]
fn a_root_is_refused_where_its_name_is_taken_there() -> Result<()> {
    // On disk.
    let (mut fx, other, there) = two_roots();
    std::fs::create_dir(there.join("trips/trip"))?;
    let why = refusal(&fx, nest(&fx, other, "trips"));
    assert!(why.contains("already there"), "{why}");
    // A missing catalog folder of that name there, or below it.
    std::fs::remove_dir(there.join("trips/trip"))?;
    fx.add_row(&there, "trips/trip/below", "y.NEF");
    let why = refusal(&fx, nest(&fx, other, "trips"));
    assert!(why.contains("catalog has a folder"), "{why}");
    Ok(())
}

#[test]
fn a_root_or_destination_that_is_offline_is_refused() -> Result<()> {
    let (mut fx, other, _) = two_roots();
    let missing = fx.dir.path().canonicalize()?.join("unplugged");
    std::fs::create_dir(&missing)?;
    let mine = fx.root_id();
    fx.cat.relink_root(mine, &missing)?;
    std::fs::remove_dir(&missing)?;
    let why = refusal(&fx, nest(&fx, other, "trips"));
    assert!(why.contains("offline or missing"), "{why}");
    let (mut fx, other, _) = two_roots();
    std::fs::create_dir(&missing)?;
    fx.cat.relink_root(other, &missing)?;
    std::fs::remove_dir(&missing)?;
    let why = refusal(&fx, nest(&fx, other, "trips"));
    assert!(why.contains("offline or missing"), "{why}");
    Ok(())
}

#[test]
fn a_root_that_is_a_whole_drive_is_refused() {
    let (fx, other, _) = two_roots();
    // Its folder is the top of a drive: another volume than the folder
    // it is in.
    let disk = Faulty::default().elsewhere(&fx.root);
    let why = refusal_on(&fx, nest(&fx, other, "trips"), &disk);
    assert!(why.contains("whole drive"), "{why}");
}

#[test]
fn a_root_to_a_drive_whose_files_can_t_be_proven_is_refused() {
    let (fx, other, there) = two_roots();
    let disk = Faulty {
        unstable_ids: vec![there.join("trips")],
        ..Faulty::default().elsewhere(&there)
    };
    let why = refusal_on(&fx, nest(&fx, other, "trips"), &disk);
    assert!(why.contains("this kind of drive"), "{why}");
}

#[test]
fn the_journal_kind_is_one_older_releases_fail_closed_on() -> Result<()> {
    // What releases up to 0.4.6 read: any other kind fails their recovery,
    // and every file operation and folder move they plan, rather than be
    // taken for a folder move within a root.
    const KNOWN_BEFORE: [&str; 8] = [
        "remove_from_catalog",
        "move_photos",
        "trash_photos",
        "create_folder",
        "rename_photos",
        "rename_folder",
        "move_folder",
        "trash_folder",
    ];
    assert!(!KNOWN_BEFORE.contains(&journal::Kind::MoveRoot.as_str()));
    // A crash right after the rename leaves it unfinished, as that kind.
    let (mut fx, other, _) = two_roots();
    let plan = fx.plan(nest(&fx, other, "trips"));
    let batch = plan.batch().clone();
    let disk = Faulty::at(Op::Rename, 0, Fault::CrashAfter);
    crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &NoTrash,
            Cancel::NEVER,
            &disk,
        ));
    });
    assert_eq!(stored_kind(&fx, &batch), "move_root");
    let unresolved = journal::unresolved(&fx.cat.db, "test")?;
    assert_eq!(unresolved.len(), 1);
    // Read as a root move only: a folder move of a root, or a root move of
    // a folder, is no journal this release wrote, and fails closed.
    let db = fx.cat.db_for_tests();
    db.execute(
        "UPDATE file_operations SET kind='move_folder' WHERE batch=?1",
        [batch.to_string()],
    )?;
    assert!(journal::unresolved(&fx.cat.db, "test").is_err());
    db.execute(
        "UPDATE file_operations SET kind='move_root',
         preconditions=json_set(preconditions, '$.from', 'day') WHERE batch=?1",
        [batch.to_string()],
    )?;
    assert!(journal::unresolved(&fx.cat.db, "test").is_err());
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

/// The root's folder is on disk at exactly one place, and the catalog says
/// the one it is at: still its own root, or the other's folder with the
/// root gone; or lists it unfinished.
fn one_place(fx: &Fx, mine: RootId, top: FolderId, there: &Path, recovery: &Recovery, case: &str) {
    let (old, new) = (fx.root.clone(), there.join("trips/trip"));
    assert!(old.is_dir() != new.is_dir(), "{case}");
    let (root, logical, _) = place(fx, top);
    let moved = logical == "trips/trip";
    assert_eq!(moved, root != mine, "{case}: half re-keyed");
    assert_eq!(
        moved,
        !roots(fx).contains(&mine),
        "{case}: root left behind"
    );
    if moved != new.is_dir() {
        assert!(!recovery.unfinished.is_empty(), "{case}: unreported");
    }
}

#[test]
fn every_fault_at_every_step_of_a_root_move_ends_proven() {
    let mut runs = 0;
    for (at, count) in [(Op::Rename, 1), (Op::Sync, 2)] {
        for nth in 0..count {
            for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                let case = format!("{at:?} {nth} {fault:?}");
                let (mut fx, other, there) = two_roots();
                let (mine, top) = (fx.root_id(), fx.folder(""));
                let was = before(&fx);
                let disk = Faulty::at(at, nth, fault);
                let op = nest(&fx, other, "trips");
                let (batch, recovery) = scenario(&mut fx, op, &disk);
                check(&fx, &was, &recovery, &batch);
                one_place(&fx, mine, top, &there, &recovery, &case);
                for u in &recovery.unfinished {
                    let back =
                        resolve(&mut fx.cat, u.operation, Resolution::PutBack, &NoTrash).unwrap();
                    assert!(
                        matches!(back.result, ItemResult::Unchanged(_)),
                        "{case}: {back:?}"
                    );
                }
                check(&fx, &was, &Recovery::default(), &batch);
                one_place(&fx, mine, top, &there, &Recovery::default(), &case);
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 9);
}

#[test]
fn every_failed_commit_of_a_root_move_is_finished_by_a_restart() -> Result<()> {
    for when in [
        "BEFORE UPDATE ON folders",
        "BEFORE INSERT ON folder_paths",
        "BEFORE UPDATE ON folder_paths",
        "BEFORE DELETE ON folder_locations",
        "BEFORE DELETE ON roots",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'",
    ] {
        let (mut fx, other, there) = two_roots();
        let (mine, top) = (fx.root_id(), fx.folder(""));
        // A location to remove, as every root this computer located has.
        let here = fx.root.clone();
        fx.cat.relink_root(mine, &here)?;
        let was = before(&fx);
        fx.fail_journal(when);
        let plan = fx.plan(nest(&fx, other, "trips"));
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
        // Never half done: the root goes with its folders, or neither.
        let (root, _, _) = place(&fx, top);
        assert_eq!(root == mine, roots(&fx).contains(&mine), "{when}");
        fx.reopen();
        let recovery = fx.recover(&disk::Real);
        assert!(recovery.unfinished.is_empty(), "{when}: {recovery:?}");
        check(&fx, &was, &recovery, &batch);
        assert_eq!(
            place(&fx, top),
            (other, "trips/trip".into(), native("trips/trip")),
            "{when}"
        );
        assert_eq!(roots(&fx), [other], "{when}");
        assert_eq!(locations_of(&fx, mine), 0, "{when}");
        assert!(there.join("trips/trip/day/b.NEF").is_file());
    }
    Ok(())
}

#[test]
fn a_root_or_destination_changed_since_planning_is_left_alone() -> Result<()> {
    // The destination root located elsewhere.
    let (mut fx, other, there) = two_roots();
    let b = fx.photo("day/b.NEF");
    let plan = fx.plan(nest(&fx, other, "trips"));
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
    assert_eq!(path_of(&fx, b), fx.root.join("day/b.NEF"));
    // A folder added to the root since.
    let (mut fx, other, _) = two_roots();
    let mine = fx.root_id();
    let plan = fx.plan(nest(&fx, other, "trips"));
    std::fs::create_dir(fx.root.join("new"))?;
    std::fs::write(fx.root.join("new/n.NEF"), bytes("new/n.NEF"))?;
    let mut again = Catalog::open(&fx.catalog)?;
    again.add_folder(&fx.root.join("new"))?;
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
    assert!(fx.root.join("new/n.NEF").is_file());
    assert!(roots(&fx).contains(&mine));
    Ok(())
}

// Onto another drive.

/// Every name below `dir`, by path relative to it, with a file's bytes
/// (`None` for a folder), but macOS's AppleDouble files on FAT and exFAT.
fn tree_of(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        let Ok(names) = std::fs::read_dir(dir) else {
            return;
        };
        for e in names {
            let p = e.unwrap().path();
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

/// The test disk, with the other root's `trips/` on another volume of
/// `kind`.
fn far(there: &Path, kind: Far) -> Faulty {
    super::copy_tests::far_at(&there.join("trips"), kind)
}

#[test]
fn a_root_moves_to_another_drive_by_a_verified_copy_and_its_original_goes_to_the_trash()
-> Result<()> {
    let (mut fx, other, there) = two_roots();
    let mine = fx.root_id();
    let (b, c) = (fx.photo("day/b.NEF"), fx.photo("day/sub/c.NEF"));
    let (top, sub) = (fx.folder(""), fx.folder("day/sub"));
    let tree = tree_of(&fx.root);
    let disk = far(&there, Far::Ids);
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(nest(&fx, other, "trips"), &disk)?;
    let across = plan.summary().across.clone().expect("it copies");
    assert_eq!(across.files, 5);
    let batch = plan.batch().clone();
    let outcome = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved], "{outcome:?}");
    assert!(outcome.items[0].originals_kept.is_empty(), "{outcome:?}");
    assert_eq!(stored_kind(&fx, &batch), "move_root");
    let moved = there.join("trips/trip");
    assert_eq!(tree_of(&moved), tree);
    assert!(!fx.root.exists());
    // The original went to the Trash whole, as one folder.
    assert_eq!(tree_of(&trash.dir.join("1-trip")), tree);
    assert_eq!(path_of(&fx, b), moved.join("day/b.NEF"));
    assert_eq!(path_of(&fx, c), moved.join("day/sub/c.NEF"));
    assert_eq!(
        place(&fx, top),
        (other, "trips/trip".into(), native("trips/trip"))
    );
    assert_eq!(place(&fx, sub).1, "trips/trip/day/sub");
    assert_eq!(roots(&fx), [other]);
    assert_eq!(locations_of(&fx, mine), 0);
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    // Copied, never renamed across: the one rename is the hidden folder's.
    let renames = disk.renames();
    assert_eq!(renames.len(), 1, "{renames:?}");
    assert!(renames[0].0.to_string_lossy().ends_with(".rawmakase-part"));
    Ok(())
}

/// The least that has every kind of step: a photo in the root, a hidden
/// file, and a photo in a folder of a folder.
fn small() -> (Fx, RootId, PathBuf) {
    let mut fx = fixture(&["a.NEF", "day/.hidden", "day/sub/c.NEF"]);
    let other = fx.dir.path().canonicalize().unwrap().join("elsewhere");
    std::fs::create_dir_all(other.join("trips")).unwrap();
    std::fs::write(other.join("trips/e.NEF"), bytes("trips/e.NEF")).unwrap();
    fx.cat.add_folder(&other).unwrap();
    let mine = fx.root_id();
    let id = roots(&fx).into_iter().find(|id| *id != mine).unwrap();
    (fx, id, other)
}

/// Plans the move on `disk`, runs it (which may crash) with `trash`,
/// restarts and recovers on the same volumes.
fn scenario_far(
    fx: &mut Fx,
    other: RootId,
    there: &Path,
    trash: &dyn Trash,
    disk: &Faulty,
    kind: Far,
) -> (BatchId, Recovery) {
    let plan = fx.plan_on(nest(fx, other, "trips"), disk).unwrap();
    let batch = plan.batch().clone();
    crashes(|| {
        drop(
            execute::execute_with(&mut fx.cat, plan.confirm(), trash, Cancel::NEVER, disk).unwrap(),
        );
    });
    fx.reopen();
    let recovery = fx.recover_with(&far(there, kind), trash);
    (batch, recovery)
}

/// The root as the outcome of `batch` says: moved whole (the copy there,
/// the catalog following and the root gone) or not at all.
#[allow(clippy::too_many_arguments, reason = "one run's facts")]
fn where_it_says(
    fx: &Fx,
    mine: RootId,
    top: FolderId,
    there: &Path,
    tree: &BTreeMap<String, Option<Vec<u8>>>,
    batch: &BatchId,
    what: &str,
) {
    let outcome = super::outcome(&fx.cat, batch).unwrap();
    let moved = there.join("trips/trip");
    let Some(item) = outcome.items.first() else {
        assert_eq!(&tree_of(&fx.root), tree, "{what}");
        assert!(roots(fx).contains(&mine), "{what}");
        return;
    };
    match &item.result {
        ItemResult::FolderMoved => {
            assert_eq!(&tree_of(&moved), tree, "{what}");
            if item.originals_kept.is_empty() {
                assert!(!fx.root.exists(), "{what}");
            }
            assert!(partials(fx).is_empty(), "{what}: {:?}", partials(fx));
            assert_eq!(place(fx, top).1, "trips/trip", "{what}");
            assert!(!roots(fx).contains(&mine), "{what}");
        }
        ItemResult::Unchanged(_) => {
            assert_eq!(&tree_of(&fx.root), tree, "{what}");
            assert!(!moved.exists(), "{what}");
            assert!(partials(fx).is_empty(), "{what}: {:?}", partials(fx));
            assert_eq!(place(fx, top), (mine, String::new(), String::new()));
            assert!(roots(fx).contains(&mine), "{what}");
        }
        ItemResult::NeedsRecovery(_) => {
            // The catalog as it was, or as it follows the copy: never
            // between.
            let (root, _, _) = place(fx, top);
            assert_eq!(root == mine, roots(fx).contains(&mine), "{what}");
        }
        other => panic!("{what}: {other:?}"),
    }
}

/// After any fault and recovery: no original lost, the root moved whole or
/// not at all; what is unfinished is put back in the end.
#[allow(clippy::too_many_arguments, reason = "one run's facts")]
fn proven(
    fx: &mut Fx,
    mine: RootId,
    top: FolderId,
    there: &Path,
    tree: &BTreeMap<String, Option<Vec<u8>>>,
    recovery: &Recovery,
    batch: &BatchId,
    was: &Before,
    what: &str,
    kind: Far,
) {
    check(fx, was, recovery, batch);
    where_it_says(fx, mine, top, there, tree, batch, what);
    let clean = far(there, kind);
    let trash = trash_as(fx, kind);
    *trash.calls.borrow_mut() = 100;
    for u in &recovery.unfinished {
        let back = recover::resolve_with(
            &mut fx.cat,
            u.operation,
            Resolution::PutBack,
            &clean,
            &trash,
        )
        .unwrap();
        assert!(
            matches!(back.result, ItemResult::Unchanged(_)),
            "{what}: {back:?}"
        );
    }
    where_it_says(fx, mine, top, there, tree, batch, what);
}

/// Every fault at every step of moving the root onto a drive of `kind`,
/// then recovery: each ends proven. Returns how many runs it made.
fn every_fault(kind: Far) -> usize {
    let started = std::time::Instant::now();
    // A run without faults says how many steps there are.
    let (mut clean, other, there) = small();
    let counting = far(&there, kind);
    let trash = trash_as(&clean, kind);
    let plan = clean
        .plan_on(nest(&clean, other, "trips"), &counting)
        .unwrap();
    let outcome = execute::execute_with(
        &mut clean.cat,
        plan.confirm(),
        &trash,
        Cancel::NEVER,
        &counting,
    )
    .unwrap();
    assert_eq!(results(&outcome), [ItemResult::FolderMoved], "{kind:?}");
    assert_eq!(counting.count(Op::Copy), 3);
    // And one undone once two files are whole: a full drive at the last.
    let (mut undone, other, there) = small();
    let failing = Faulty {
        faults: vec![(Op::Copy, 2, Fault::Fail)],
        ..far(&there, kind)
    };
    let trash = trash_as(&undone, kind);
    let plan = undone
        .plan_on(nest(&undone, other, "trips"), &failing)
        .unwrap();
    let _ = execute::execute_with(
        &mut undone.cat,
        plan.confirm(),
        &trash,
        Cancel::NEVER,
        &failing,
    );
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
                if kind != Far::Ids && fault == Fault::CrashBefore && nth > 0 {
                    continue;
                }
                for undo in [false, true] {
                    if undo && !undoing {
                        continue;
                    }
                    let (mut fx, other, there) = small();
                    let (mine, top) = (fx.root_id(), fx.folder(""));
                    let was = before(&fx);
                    let tree = tree_of(&fx.root);
                    let mut with = vec![(op, nth, fault)];
                    if undo {
                        with.push((Op::Copy, 2, Fault::Fail));
                    }
                    let disk = Faulty {
                        faults: with,
                        ..far(&there, kind)
                    };
                    let trash = trash_as(&fx, kind);
                    let (batch, recovery) =
                        scenario_far(&mut fx, other, &there, &trash, &disk, kind);
                    let what = format!("{kind:?} {op:?} {nth} {fault:?} undo {undo}");
                    proven(
                        &mut fx, mine, top, &there, &tree, &recovery, &batch, &was, &what, kind,
                    );
                    runs += 1;
                }
            }
        }
    }
    // The Trash failing at, or crashing after, the first and second item.
    for n in 1..=2 {
        for crash in [false, true] {
            let (mut fx, other, there) = small();
            let (mine, top) = (fx.root_id(), fx.folder(""));
            let was = before(&fx);
            let tree = tree_of(&fx.root);
            let disk = far(&there, kind);
            let mut trash = trash_as(&fx, kind);
            match crash {
                true => trash.crash_after = Some(n),
                false => trash.fail_from = Some(n),
            }
            let (batch, recovery) = scenario_far(&mut fx, other, &there, &trash, &disk, kind);
            let what = format!("{kind:?} trash {n} {crash}");
            proven(
                &mut fx, mine, top, &there, &tree, &recovery, &batch, &was, &what, kind,
            );
            runs += 1;
        }
    }
    eprintln!("{kind:?}: {runs} runs in {:?}", started.elapsed());
    runs
}

#[test]
fn every_fault_at_every_step_of_a_root_copy_ends_proven() {
    assert!(every_fault(Far::Ids) >= 100);
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the fault matrix by names is the same code everywhere; Linux and macOS run it, keeping Windows CI within its hour"
)]
fn every_fault_at_every_step_of_a_root_copy_to_exfat_ends_proven() {
    assert!(every_fault(Far::Names) >= 60);
}

#[test]
fn every_failed_journal_write_of_a_root_copy_ends_proven() -> Result<()> {
    for when in [
        "BEFORE INSERT ON file_operation_copies",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'copying'",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'copied'",
        "BEFORE UPDATE ON folders",
        "BEFORE DELETE ON folder_locations",
        "BEFORE DELETE ON roots",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'committed'",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'trashed'",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'",
    ] {
        let (mut fx, other, there) = two_roots();
        let (mine, top) = (fx.root_id(), fx.folder(""));
        let here = fx.root.clone();
        fx.cat.relink_root(mine, &here)?;
        let was = before(&fx);
        let tree = tree_of(&fx.root);
        fx.fail_journal(when);
        let disk = far(&there, Far::Ids);
        let trash = FakeTrash::new(&fx);
        let plan = fx.plan_on(nest(&fx, other, "trips"), &disk)?;
        let batch = plan.batch().clone();
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
        fx.reopen();
        let recovery = fx.recover_with(&far(&there, Far::Ids), &trash);
        proven(
            &mut fx,
            mine,
            top,
            &there,
            &tree,
            &recovery,
            &batch,
            &was,
            when,
            Far::Ids,
        );
    }
    Ok(())
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_OTHER_VOLUME: a writable folder on another volume"]
fn a_root_moves_between_two_real_volumes() -> Result<()> {
    let Some(volume) = std::env::var_os("RAWMAKASE_TEST_OTHER_VOLUME") else {
        return Ok(());
    };
    let (mut fx, _, _) = two_roots();
    let mine = fx.root_id();
    let scratch = tempfile::tempdir_in(&volume)?;
    let there: PathBuf = scratch.path().canonicalize()?;
    assert_ne!(
        fsops::entry(&there)?.id.volume(),
        fsops::entry(&fx.root)?.id.volume(),
        "{} is on the fixture's volume",
        there.display()
    );
    std::fs::create_dir(there.join("trips"))?;
    std::fs::write(there.join("trips/keep.dng"), bytes("keep.dng"))?;
    fx.cat.add_folder(&there)?;
    let other = fx
        .cat
        .roots()?
        .into_iter()
        .find(|(_, original, _)| Path::new(original) == there)
        .unwrap()
        .0;
    let c = fx.photo("day/sub/c.NEF");
    let top = fx.folder("");
    let tree = tree_of(&fx.root);
    let plan = fx.plan(nest(&fx, other, "trips"));
    assert!(plan.summary().across.is_some());
    let trash = FakeTrash::new(&fx);
    let outcome = execute::execute(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved], "{outcome:?}");
    let moved = there.join("trips/trip");
    assert_eq!(tree_of(&moved), tree);
    assert!(!fx.root.exists());
    assert_eq!(path_of(&fx, c), moved.join("day/sub/c.NEF"));
    assert_eq!(place(&fx, top).0, other);
    assert!(!roots(&fx).contains(&mine));
    let leftovers: Vec<_> = tree_of(&there)
        .into_keys()
        .filter(|k| k.contains(".rawmakase-part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // And a root on the other drive onto this one, into a root here.
    let home = fx.dir.path().canonicalize()?.join("home");
    std::fs::create_dir(&home)?;
    std::fs::write(home.join("keep.dng"), bytes("home/keep.dng"))?;
    fx.cat.add_folder(&home)?;
    let away_scratch = tempfile::tempdir_in(&volume)?;
    let away = away_scratch.path().canonicalize()?.join("away");
    for f in ["d.NEF", "d.xmp", ".hidden", "sub/f.NEF"] {
        let path = away.join(f);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, bytes(&format!("away/{f}")))?;
    }
    fx.cat.add_folder(&away)?;
    let root_of = |dir: &Path| {
        fx.cat
            .roots()
            .unwrap()
            .into_iter()
            .find(|(_, original, _)| Path::new(original) == dir)
            .unwrap()
            .0
    };
    let (home_root, away_root) = (root_of(&home), root_of(&away));
    let f = fx
        .cat
        .photos()?
        .into_iter()
        .find(|p| p.path == away.join("sub/f.NEF"))
        .unwrap()
        .id;
    let tree = tree_of(&away);
    let plan = fx.plan(FileOp::MoveRoot {
        root: away_root,
        to_root: home_root,
        parent: String::new(),
    });
    assert!(plan.summary().across.is_some());
    // A Trash on its drive, as the system's is.
    let trash = FakeTrash::at(&away_scratch.path().join("trash"));
    let outcome = execute::execute(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved], "{outcome:?}");
    assert!(outcome.items[0].originals_kept.is_empty(), "{outcome:?}");
    assert_eq!(tree_of(&home.join("away")), tree);
    assert!(!away.exists());
    assert_eq!(path_of(&fx, f), home.join("away/sub/f.NEF"));
    assert!(!roots(&fx).contains(&away_root));
    drop(fx);
    clear(&there)?;
    scratch.close()?;
    away_scratch.close()?;
    Ok(())
}

/// Removes everything in `dir`, by names composed (macOS's exFAT removes a
/// file only by the spelling it is stored in).
fn clear(dir: &Path) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}
