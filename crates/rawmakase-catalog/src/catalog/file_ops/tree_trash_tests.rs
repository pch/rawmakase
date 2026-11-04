//! Putting a folder in the Trash (#575): the whole tree in one Trash call
//! and the subtree's rows on the evidence; what is refused; what changed
//! since the question stops; and what a failure or a crash at every step
//! leaves, which recovery decides from the journal and the disk. On drives
//! whose file ids stay and on FAT and exFAT (by names, with the ids
//! renumbered at every step), whatever order folders list in.
use super::journal::{self, ItemState, MemberState};
use super::tests::{
    FakeTrash, Fault, Faulty, Fx, NoTrash, Op, PathHook, bytes, crashes, fixture, results,
};
use super::*;
use crate::catalog::HistoryUpdate;
use crate::export_settings::ExportOptions;
use crate::model::recipe::Recipe;
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The catalog of `trip/` (the root) with `r.NEF` and `other/c.NEF`, and
/// the folder put in the Trash, `day/`: `a.NEF` with `a.xmp`, `.hidden`,
/// `notes.txt`, `sub/b.NEF`, an empty folder `empty/`, and a catalog
/// folder `missing/` whose photo's file is gone; a virtual copy of `a.NEF`
/// with an edit, and one of `other/c.NEF` that an older tool put in `day`.
struct Trip {
    fx: Fx,
    a: PhotoId,
    b: PhotoId,
    m: PhotoId,
    c: PhotoId,
    r: PhotoId,
    copy: PhotoId,
    stray: PhotoId,
}

fn trip() -> Trip {
    let mut fx = fixture(&[
        "r.NEF",
        "other/c.NEF",
        "day/a.NEF",
        "day/a.xmp",
        "day/.hidden",
        "day/notes.txt",
        "day/sub/b.NEF",
        "day/missing/m.NEF",
    ]);
    std::fs::create_dir(fx.root.join("day/empty")).unwrap();
    std::fs::remove_dir_all(fx.root.join("day/missing")).unwrap();
    let (a, b, m, c, r) = (
        fx.photo("day/a.NEF"),
        fx.photo("day/sub/b.NEF"),
        fx.photo("day/missing/m.NEF"),
        fx.photo("other/c.NEF"),
        fx.photo("r.NEF"),
    );
    let copy = fx.cat.create_virtual_copy(a).unwrap();
    fx.cat
        .save_edit(
            copy,
            &fx.root.join("day/a.NEF"),
            &crate::storage::Identity::read(&fx.root.join("day/a.NEF")).unwrap(),
            &Recipe::default(),
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )
        .unwrap();
    let stray = fx.cat.create_virtual_copy(c).unwrap();
    let day = fx.folder("day");
    fx.cat
        .db_for_tests()
        .execute("UPDATE photos SET folder=?1 WHERE id=?2", [day.0, stray.0])
        .unwrap();
    Trip {
        fx,
        a,
        b,
        m,
        c,
        r,
        copy,
        stray,
    }
}

impl Trip {
    /// The rows the folder Trash removes, sorted.
    fn removed(&self) -> Vec<PhotoId> {
        let mut ids = vec![self.a, self.b, self.m, self.copy, self.stray];
        ids.sort();
        ids
    }
}

fn op(fx: &Fx, folder: &str) -> FileOp {
    FileOp::TrashFolder {
        root: fx.root_id(),
        folder: folder.into(),
    }
}

/// Plans putting `folder` in `trash` on `disk`.
fn plan_on(
    fx: &Fx,
    folder: &str,
    trash: &dyn Trash,
    disk: &dyn disk::Disk,
) -> Result<Plan, FileOpError> {
    plan::plan_trash_with(&fx.cat, op(fx, folder), disk, trash)
}

/// The plan's refusal, as the user reads it.
fn refusal(fx: &Fx, folder: &str, trash: &dyn Trash, disk: &dyn disk::Disk) -> String {
    match plan_on(fx, folder, trash, disk) {
        Err(FileOpError::Destination(why)) => why,
        other => panic!("not refused: {other:?}"),
    }
}

/// Executes `plan` with `trash` on `disk`.
fn execute(fx: &mut Fx, plan: Plan, trash: &dyn Trash, disk: &dyn disk::Disk) -> Outcome {
    execute::execute_with(&mut fx.cat, plan.confirm(), trash, Cancel::NEVER, disk).unwrap()
}

/// Plans and executes putting `folder` in `trash` on `disk`.
fn run(fx: &mut Fx, folder: &str, trash: &dyn Trash, disk: &dyn disk::Disk) -> (BatchId, Outcome) {
    let plan = plan_on(fx, folder, trash, disk).unwrap();
    let batch = plan.batch().clone();
    let outcome = execute(fx, plan, trash, disk);
    (batch, outcome)
}

/// The journalled item of `batch`.
fn item(fx: &Fx, batch: &BatchId) -> journal::Item {
    let mut items = journal::of_batch(&fx.cat.db, &batch.0).unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    items.remove(0)
}

/// Every file's bytes below the fixture's folder, in the Trash or not,
/// sorted: nothing is ever lost.
fn every_byte(fx: &Fx) -> Vec<Vec<u8>> {
    let mut all: Vec<Vec<u8>> = fx.files().into_values().collect();
    all.sort();
    all
}

/// Whether the trip's rows are all as they were.
fn rows_intact(t: &Trip) -> bool {
    let rows = t.fx.rows();
    [t.a, t.b, t.m, t.c, t.r, t.copy, t.stray]
        .iter()
        .all(|p| rows.contains_key(p))
}

/// The fixture's tree as the disk has it below `dir`: every name, for
/// renumbering.
fn tree_paths(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![dir.to_path_buf()];
    let mut next = 0;
    while next < out.len() {
        let at = out[next].clone();
        next += 1;
        if at.is_dir() {
            for e in std::fs::read_dir(&at).unwrap() {
                out.push(e.unwrap().path());
            }
        }
    }
    out
}

/// A remount of the names-only volume: every name of `dir` numbered anew,
/// and one number given again to another file. Asserts it happened.
fn remount(disk: &Faulty, dir: &Path) {
    let paths = tree_paths(dir);
    let before: Vec<_> = paths
        .iter()
        .map(|p| disk::Disk::entry(disk, p).unwrap().id)
        .collect();
    let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let files: Vec<&Path> = refs.iter().copied().filter(|p| p.is_file()).collect();
    let reuse = match files.as_slice() {
        [a, b, ..] => Some((*a, *b)),
        _ => None,
    };
    disk.remount(&refs, reuse);
    for (p, was) in paths.iter().zip(&before) {
        assert_ne!(
            disk::Disk::entry(disk, p).unwrap().id,
            *was,
            "{} kept its id",
            p.display()
        );
    }
    if let Some((a, b)) = reuse {
        assert_eq!(
            disk::Disk::entry(disk, b).unwrap().id,
            before[paths.iter().position(|p| p == a).unwrap()],
            "a number given again"
        );
    }
}

/// A disk proving names only (FAT, exFAT): the whole fixture's folder on
/// one names-only volume, its Trash too, numbered anew on remounts.
fn names_only(fx: &Fx) -> Faulty {
    let dir = fx.dir.path().canonicalize().unwrap();
    Faulty {
        names_only: vec![dir.clone()],
        elsewhere: Some(dir),
        ..Faulty::default()
    }
}

fn trash_at(fx: &Fx) -> PathBuf {
    fx.dir.path().canonicalize().unwrap().join("trash")
}

// Happy paths.

#[test]
fn a_folder_goes_to_the_trash_whole_and_its_subtree_leaves_the_catalog() -> Result<()> {
    for reversed in [false, true] {
        let mut t = trip();
        let was = every_byte(&t.fx);
        let disk = Faulty {
            list_reversed: reversed,
            ..Faulty::default()
        };
        let trash = FakeTrash::new(&t.fx);
        let (batch, outcome) = run(&mut t.fx, "day", &trash, &disk);
        assert_eq!(results(&outcome), [ItemResult::Trashed], "{outcome:?}");
        assert_eq!(*trash.calls.borrow(), 1, "one Trash call");
        let mut photos = outcome.items[0].photos.clone();
        photos.sort();
        assert_eq!(photos, t.removed());
        assert_eq!(outcome.items[0].note, None);
        assert!(!t.fx.root.join("day").exists());
        let at = trash.dir.join("1-day");
        for f in ["a.NEF", "a.xmp", ".hidden", "notes.txt", "sub/b.NEF"] {
            assert!(at.join(f).is_file(), "{f} in the Trash");
        }
        assert!(at.join("empty").is_dir());
        assert_eq!(every_byte(&t.fx), was, "nothing lost");
        let rows = t.fx.rows();
        for gone in t.removed() {
            assert!(!rows.contains_key(&gone), "{gone:?} removed");
        }
        assert_eq!(rows[&t.c], "other/c.NEF");
        assert_eq!(rows[&t.r], "r.NEF");
        assert_eq!(t.fx.cat.roots()?.len(), 1, "the root stays");
        let folders: Vec<PathBuf> = t.fx.cat.folders()?.into_iter().map(|f| f.path).collect();
        assert!(
            folders
                .iter()
                .all(|f| !f.starts_with(t.fx.root.join("day")))
        );
        assert!(folders.contains(&t.fx.root.join("other")));
        let journalled = item(&t.fx, &batch);
        assert_eq!(journalled.kind, journal::Kind::TrashFolder);
        assert_eq!(journalled.state, ItemState::Done);
        assert_eq!(journalled.members.len(), 8, "{:?}", journalled.members);
        for m in &journalled.members {
            assert_eq!(m.state, MemberState::Trashed);
            let rel = m.source.strip_prefix(t.fx.root.join("day"))?;
            let receipt = m.receipt.as_deref().unwrap();
            let expected = match rel.as_os_str().is_empty() {
                true => at.clone(),
                false => at.join(rel),
            };
            assert_eq!(receipt, expected);
        }
        // Planned and journalled in walk order: the folder first.
        assert_eq!(journalled.members[0].source, t.fx.root.join("day"));
    }
    Ok(())
}

#[test]
fn a_folder_the_catalog_has_no_folder_of_its_own_for_goes_with_those_below() -> Result<()> {
    let mut fx = fixture(&["keep.NEF", "2024/day1/a.NEF", "2024/day2/b.NEF"]);
    std::fs::create_dir(fx.root.join("2024/only")).unwrap();
    std::fs::create_dir(fx.root.join("2024/only/inner")).unwrap();
    let trash = FakeTrash::new(&fx);
    let (_, outcome) = run(&mut fx, "2024", &trash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::Trashed]);
    assert_eq!(outcome.items[0].photos.len(), 2);
    assert!(trash.dir.join("1-2024/only/inner").is_dir());
    assert_eq!(fx.rows().len(), 1);
    // A folder of only folders, and an empty one.
    let mut fx = fixture(&["keep.NEF", "only/inner/x.NEF"]);
    let trash = FakeTrash::new(&fx);
    let (_, outcome) = run(&mut fx, "only", &trash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::Trashed]);
    let mut fx = fixture(&["keep.NEF", "e/x.NEF"]);
    std::fs::remove_file(fx.root.join("e/x.NEF"))?;
    let x = fx.photo("e/x.NEF");
    let trash = FakeTrash::new(&fx);
    let plan = plan_on(&fx, "e", &trash, &Faulty::default())?;
    let counts = plan.summary().tree.clone().unwrap();
    assert_eq!(
        (counts.folders, counts.photos, counts.other_files),
        (0, 0, 0)
    );
    assert_eq!(counts.missing_photos, 1);
    let outcome = execute(&mut fx, plan, &trash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::Trashed]);
    assert!(!fx.rows().contains_key(&x));
    Ok(())
}

#[test]
fn a_root_left_with_nothing_in_the_catalog_stays() -> Result<()> {
    let mut fx = fixture(&["2024/day1/a.NEF"]);
    let trash = FakeTrash::new(&fx);
    let (_, outcome) = run(&mut fx, "2024", &trash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::Trashed]);
    assert!(fx.rows().is_empty());
    assert!(fx.cat.folders()?.is_empty());
    assert_eq!(
        fx.cat.roots()?.len(),
        1,
        "kept, as Lightroom keeps a parent"
    );
    Ok(())
}

#[test]
fn the_question_counts_what_goes_whatever_order_folders_list_in() -> Result<()> {
    for reversed in [false, true] {
        let t = trip();
        let disk = Faulty {
            list_reversed: reversed,
            ..Faulty::default()
        };
        let plan = plan_on(&t.fx, "day", &FakeTrash::new(&t.fx), &disk)?;
        let summary = plan.summary();
        assert_eq!(
            summary.tree,
            Some(TreeCounts {
                folders: 2,
                photos: 2,
                other_files: 3,
                other_names: vec![".hidden".into(), "a.xmp".into(), "notes.txt".into()],
                missing_photos: 1,
                missing_folders: 1,
                stray_copies: 1,
            })
        );
        assert_eq!((summary.photos, summary.copies, summary.edited), (5, 2, 1));
        assert_eq!(summary.folder.as_deref(), Some("day"));
        let mut photos = plan.photos();
        photos.sort();
        assert_eq!(photos, t.removed());
        assert_eq!(plan.steps(), 1);
        assert!(!plan.is_empty());
        assert!(
            t.fx.root.join("day/a.NEF").is_file(),
            "planning moves nothing"
        );
    }
    Ok(())
}

#[test]
fn on_fat_and_exfat_a_folder_goes_by_its_names_with_ids_renumbered_at_every_step() -> Result<()> {
    let mut t = trip();
    let was = every_byte(&t.fx);
    let disk = Rc::new(names_only(&t.fx));
    let mut trash = FakeTrash::at(&trash_at(&t.fx));
    let hook = Rc::clone(&disk);
    let trash_dir = trash.dir.clone();
    trash.after = Some(Box::new(move |_: &Path| remount(&hook, &trash_dir)));
    let plan = plan_on(&t.fx, "day", &trash, &*disk)?;
    remount(&disk, &t.fx.root.join("day"));
    let batch = plan.batch().clone();
    let outcome = execute(&mut t.fx, plan, &trash, &*disk);
    assert_eq!(results(&outcome), [ItemResult::Trashed], "{outcome:?}");
    match item(&t.fx, &batch).subject {
        journal::Subject::TrashTree(state) => assert_eq!(state.proof, proof::Proof::Names),
        other => panic!("{other:?}"),
    }
    assert!(!t.fx.root.join("day").exists());
    assert_eq!(every_byte(&t.fx), was);
    assert!(!rows_intact(&t));
    Ok(())
}

#[test]
fn on_fat_and_exfat_a_folder_listed_decomposed_goes_by_the_name_it_is_stored_in() -> Result<()> {
    let mut fx = fixture(&["keep.NEF", "d\u{ed}a/a.NEF"]);
    let disk = Faulty {
        lists_decomposed: true,
        ..names_only(&fx)
    };
    let mut trash = FakeTrash::at(&trash_at(&fx));
    trash.stored_only = true;
    let (_, outcome) = run(&mut fx, "d\u{ed}a", &trash, &disk);
    assert_eq!(results(&outcome), [ItemResult::Trashed], "{outcome:?}");
    assert_eq!(fx.rows().len(), 1);
    Ok(())
}

// Refusals: nothing journalled, nothing moved.

/// Asserts `why` refused, and nothing was journalled or moved.
fn refused_quietly(fx: &Fx, files: &std::collections::BTreeMap<String, Vec<u8>>) {
    assert_eq!(&fx.files(), files);
    let n: i64 = fx
        .cat
        .db_for_tests()
        .query_row("SELECT count(*) FROM file_operations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn a_root_a_folder_no_longer_in_the_catalog_or_offline_is_refused() -> Result<()> {
    let t = trip();
    let files = t.fx.files();
    let trash = FakeTrash::new(&t.fx);
    let disk = Faulty::default();
    let why = refusal(&t.fx, "", &trash, &disk);
    assert!(
        why.starts_with("A root folder can't be moved to the Trash"),
        "{why}"
    );
    let why = refusal(&t.fx, "nowhere", &trash, &disk);
    assert_eq!(why, "The folder is no longer in the catalog.");
    std::fs::rename(t.fx.root.join("other"), t.fx.root.join("away"))?;
    let why = refusal(&t.fx, "other", &trash, &disk);
    assert_eq!(why, "The folder is offline or missing.");
    std::fs::rename(t.fx.root.join("away"), t.fx.root.join("other"))?;
    refused_quietly(&t.fx, &files);
    Ok(())
}

#[test]
fn a_subtree_with_a_location_of_its_own_or_with_something_else_of_the_catalog_inside_is_refused()
-> Result<()> {
    let mut t = trip();
    let trash = FakeTrash::new(&t.fx);
    let disk = Faulty::default();
    let sub = t.fx.folder("day/sub");
    t.fx.cat.relink_folder(sub, &t.fx.root.join("day/sub"))?;
    let why = refusal(&t.fx, "day", &trash, &disk);
    assert!(why.contains("located on its own"), "{why}");
    let t = trip();
    let mut fx = t.fx;
    let sub = fx.root.join("day/sub");
    fx.add_row(&sub, "", "b.NEF");
    let why = refusal(&fx, "day", &trash, &disk);
    assert_eq!(
        why,
        "Another folder or root of the catalog is inside it on disk."
    );
    // The catalog inside it.
    let fx = fixture(&["day/a.NEF"]);
    let inside = fx.root.join("day/Inside.rawmakase");
    let mut cat = crate::catalog::Catalog::create(&inside)?;
    cat.add_folder(&fx.root)?;
    let refused = plan::plan_trash_with(
        &cat,
        FileOp::TrashFolder {
            root: cat.roots()?[0].0,
            folder: "day".into(),
        },
        &Faulty::default(),
        &FakeTrash::new(&fx),
    );
    match refused {
        Err(FileOpError::Destination(why)) => assert!(why.contains("catalog"), "{why}"),
        other => panic!("{other:?}"),
    }
    Ok(())
}

#[test]
fn a_folder_in_a_trash_is_refused() -> Result<()> {
    let mut fx = fixture(&["keep.NEF", ".Trashes/x.NEF"]);
    // Import passes such folders by; an older tool may have added it.
    let root = fx.root.clone();
    fx.add_row(&root, ".Trashes", "x.NEF");
    let why = refusal(&fx, ".Trashes", &FakeTrash::new(&fx), &Faulty::default());
    assert!(why.contains("in the Trash"), "{why}");
    Ok(())
}

#[test]
fn a_read_only_drive_a_network_share_and_a_drive_without_proof_are_refused() -> Result<()> {
    let t = trip();
    let files = t.fx.files();
    let trash = FakeTrash::new(&t.fx);
    let read_only = Faulty {
        read_only: true,
        ..Faulty::default()
    };
    assert_eq!(
        refusal(&t.fx, "day", &trash, &read_only),
        "The folder's volume is read-only."
    );
    let network = Faulty {
        network: true,
        ..Faulty::default()
    };
    let why = refusal(&t.fx, "day", &trash, &network);
    assert!(
        why.starts_with("Folders on a network drive can't be moved to the Trash"),
        "{why}"
    );
    let unproven = Faulty {
        unstable_ids: vec![t.fx.root.join("day")],
        ..Faulty::default()
    };
    let why = refusal(&t.fx, "day", &trash, &unproven);
    assert!(
        why.starts_with("Moving folders on this kind of drive"),
        "{why}"
    );
    refused_quietly(&t.fx, &files);
    Ok(())
}

#[test]
fn a_drive_whose_trash_surely_refuses_it_is_refused_and_only_then() -> Result<()> {
    let t = trip();
    let mut trash = FakeTrash::new(&t.fx);
    trash.support = TrashSupport::No;
    let why = refusal(&t.fx, "day", &trash, &Faulty::default());
    assert!(
        why.starts_with("“day” is on a drive without a Trash."),
        "{why}"
    );
    trash.support = TrashSupport::Unknown;
    assert!(plan_on(&t.fx, "day", &trash, &Faulty::default()).is_ok());
    Ok(())
}

#[test]
fn plan_without_the_trash_refuses_a_folder_trash() {
    let t = trip();
    assert!(matches!(
        plan(&t.fx.cat, op(&t.fx, "day")),
        Err(FileOpError::Unsupported(_))
    ));
}

#[cfg(unix)]
#[test]
fn links_special_files_and_hard_links_inside_are_refused() -> Result<()> {
    let t = trip();
    let trash = FakeTrash::new(&t.fx);
    let disk = Faulty::default();
    std::os::unix::fs::symlink(t.fx.root.join("r.NEF"), t.fx.root.join("day/sub/link"))?;
    let why = refusal(&t.fx, "day", &trash, &disk);
    assert!(
        why.ends_with("link is a link. The folder can't be moved to the Trash."),
        "{why}"
    );
    std::fs::remove_file(t.fx.root.join("day/sub/link"))?;
    std::fs::hard_link(t.fx.root.join("r.NEF"), t.fx.root.join("day/twin"))?;
    let why = refusal(&t.fx, "day", &trash, &disk);
    assert!(why.contains("a hard link"), "{why}");
    std::fs::remove_file(t.fx.root.join("day/twin"))?;
    // On the way to it.
    let t = trip();
    std::fs::rename(t.fx.root.join("other"), t.fx.dir.path().join("real"))?;
    std::os::unix::fs::symlink(t.fx.dir.path().join("real"), t.fx.root.join("other"))?;
    let why = refusal(&t.fx, "other", &trash, &disk);
    assert!(
        why.contains("is a link or another volume inside its root folder"),
        "{why}"
    );
    Ok(())
}

#[test]
fn two_names_differing_only_in_case_are_refused_where_the_drive_has_both() -> Result<()> {
    let t = trip();
    std::fs::write(t.fx.root.join("day/Notes.txt"), b"other")?;
    let both = std::fs::read(t.fx.root.join("day/notes.txt"))? != b"other";
    let why = plan_on(&t.fx, "day", &FakeTrash::new(&t.fx), &Faulty::default());
    match (both, why) {
        (true, Err(FileOpError::Destination(why))) => {
            assert!(
                why.ends_with(
                    "differ only in case or accents, which RAWmakase can't move to the Trash yet."
                ),
                "{why}"
            );
        }
        // A drive that takes them for one name (macOS's, Windows').
        (false, plan) => assert!(plan.is_ok()),
        (true, other) => panic!("{other:?}"),
    }
    Ok(())
}

#[test]
fn a_folder_that_cant_be_listed_is_refused() -> Result<()> {
    let t = trip();
    let disk = Faulty::default();
    disk.unlistable.borrow_mut().push(t.fx.root.join("day/sub"));
    let why = refusal(&t.fx, "day", &FakeTrash::new(&t.fx), &disk);
    assert!(
        why.ends_with("The folder can't be moved to the Trash."),
        "{why}"
    );
    Ok(())
}

#[test]
fn an_unfinished_operation_involving_it_refuses_it() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/sub/"]);
    fx.fail_journal("BEFORE UPDATE ON photos");
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos: vec![fx.photo("day/a.NEF")],
            root: fx.root_id(),
            to: "day/sub".into(),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(matches!(
        results(&outcome)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    let why = refusal(&fx, "day", &FakeTrash::new(&fx), &Faulty::default());
    assert_eq!(why, "An unfinished file operation involves it.");
    Ok(())
}

// Changed since the question: nothing moves.

/// Plans, lets `change` change the disk or the catalog, then runs: the
/// item `no_change` with a reason containing `says`, nothing moved, the
/// rows intact.
fn changed_since(says: &str, change: impl FnOnce(&mut Trip)) {
    let mut t = trip();
    let trash = FakeTrash::new(&t.fx);
    let plan = plan_on(&t.fx, "day", &trash, &Faulty::default()).unwrap();
    change(&mut t);
    let files = t.fx.files();
    let outcome = execute(&mut t.fx, plan, &trash, &Faulty::default());
    match &results(&outcome)[..] {
        [ItemResult::Unchanged(Some(why))] => assert!(why.contains(says), "{says}: {why}"),
        other => panic!("{says}: {other:?}"),
    }
    assert_eq!(*trash.calls.borrow(), 0, "{says}: never trashed");
    assert_eq!(t.fx.files(), files, "{says}: nothing moved");
    assert!(rows_intact(&t), "{says}");
}

#[test]
fn a_name_added_removed_or_changed_since_the_question_stops_it() {
    changed_since("late.NEF was added to it", |t| {
        std::fs::write(t.fx.root.join("day/late.NEF"), b"late").unwrap()
    });
    changed_since("sub/late.NEF was added to it", |t| {
        std::fs::write(t.fx.root.join("day/sub/late.NEF"), b"late").unwrap()
    });
    changed_since("new was added to it", |t| {
        std::fs::create_dir(t.fx.root.join("day/new")).unwrap()
    });
    changed_since("notes.txt is gone from it", |t| {
        std::fs::remove_file(t.fx.root.join("day/notes.txt")).unwrap()
    });
    changed_since("notes.txt changed", |t| {
        let path = t.fx.root.join("day/notes.txt");
        let mut b = std::fs::read(&path).unwrap();
        b[0] ^= 1;
        std::fs::write(&path, b).unwrap()
    });
    changed_since("notes.txt changed", |t| {
        let path = t.fx.root.join("day/notes.txt");
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000))
            .unwrap();
    });
    // Another file with the same bytes and time: another file object.
    changed_since("notes.txt changed", |t| {
        let path = t.fx.root.join("day/notes.txt");
        let other = t.fx.root.join("day-notes");
        std::fs::copy(&path, &other).unwrap();
        let time = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_modified(time)
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&other, &path).unwrap();
    });
    // The folder itself, another of its name.
    changed_since("changed since you were asked", |t| {
        let day = t.fx.root.join("day");
        std::fs::rename(&day, t.fx.dir.path().join("away")).unwrap();
        std::fs::create_dir(&day).unwrap();
    });
}

#[cfg(unix)]
#[test]
fn a_file_turned_into_a_link_or_a_link_put_on_the_way_stops_it() {
    changed_since("is a link", |t| {
        let path = t.fx.root.join("day/notes.txt");
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(t.fx.root.join("r.NEF"), &path).unwrap();
    });
    // The root's own folder swapped for a link to where it was.
    changed_since("changed since you were asked", |t| {
        let day = t.fx.root.join("day");
        let real = t.fx.dir.path().canonicalize().unwrap().join("real-day");
        std::fs::rename(&day, &real).unwrap();
        std::os::unix::fs::symlink(&real, &day).unwrap();
    });
}

#[test]
fn the_files_the_system_writes_on_its_own_dont_stop_it_and_still_go() -> Result<()> {
    let mut t = trip();
    std::fs::write(t.fx.root.join("day/.DS_Store"), b"one")?;
    std::fs::write(t.fx.root.join("day/sub/Thumbs.db"), b"one")?;
    let trash = FakeTrash::new(&t.fx);
    let plan = plan_on(&t.fx, "day", &trash, &Faulty::default())?;
    std::fs::write(t.fx.root.join("day/.DS_Store"), b"changed by Finder")?;
    std::fs::remove_file(t.fx.root.join("day/sub/Thumbs.db"))?;
    std::fs::write(t.fx.root.join("day/sub/desktop.ini"), b"new")?;
    let outcome = execute(&mut t.fx, plan, &trash, &Faulty::default());
    assert_eq!(results(&outcome), [ItemResult::Trashed], "{outcome:?}");
    assert_eq!(outcome.items[0].note, None, "not news");
    assert!(trash.dir.join("1-day/.DS_Store").is_file());
    assert!(trash.dir.join("1-day/sub/desktop.ini").is_file());
    // Nothing else: an AppleDouble file is another file's.
    changed_since("._a.NEF was added to it", |t| {
        std::fs::write(t.fx.root.join("day/._a.NEF"), b"AppleDouble").unwrap()
    });
    Ok(())
}

#[test]
fn a_catalog_change_since_the_question_stops_it() {
    changed_since(preconditions::CHANGED.trim_start_matches("it "), |t| {
        // Synchronize Folder adds a photo in it.
        std::fs::write(t.fx.root.join("day/sub/new.NEF"), bytes("new")).unwrap();
        let root = t.fx.root.clone();
        t.fx.add_row(&root, "day/sub", "new.NEF");
    });
    changed_since(preconditions::CHANGED.trim_start_matches("it "), |t| {
        let root = t.fx.root.clone();
        t.fx.add_row(&root, "day/late", "l.NEF");
    });
    changed_since("a photo in it changed", |t| {
        t.fx.cat
            .save_edit(
                t.a,
                &t.fx.root.join("day/a.NEF"),
                &crate::storage::Identity::read(&t.fx.root.join("day/a.NEF")).unwrap(),
                &Recipe::default(),
                &ExportOptions::default(),
                HistoryUpdate::Keep,
            )
            .unwrap();
    });
    // An edit of the copy whose master is elsewhere.
    changed_since("a photo in it changed", |t| {
        t.fx.cat
            .save_edit(
                t.stray,
                &t.fx.root.join("other/c.NEF"),
                &crate::storage::Identity::read(&t.fx.root.join("other/c.NEF")).unwrap(),
                &Recipe::default(),
                &ExportOptions::default(),
                HistoryUpdate::Keep,
            )
            .unwrap();
    });
    changed_since("located on its own", |t| {
        let sub = t.fx.folder("day/sub");
        let at = t.fx.root.join("day/sub");
        t.fx.cat.relink_folder(sub, &at).unwrap();
    });
    changed_since("inside it", |t| {
        let sub = t.fx.root.join("day/sub");
        t.fx.add_row(&sub, "", "b.NEF");
    });
}

#[test]
fn stop_before_or_during_the_last_walk_moves_nothing() -> Result<()> {
    // Before the run: nothing journalled.
    let mut t = trip();
    let trash = FakeTrash::new(&t.fx);
    let plan = plan_on(&t.fx, "day", &trash, &Faulty::default())?;
    let outcome = execute::execute_with(
        &mut t.fx.cat,
        plan.confirm(),
        &trash,
        Cancel::new(&AtomicBool::new(true)),
        &Faulty::default(),
    )?;
    assert!(outcome.items.is_empty());
    assert!(rows_intact(&t));
    // During the last walk: set as `sub` is listed (the plan listed it
    // first).
    let mut t = trip();
    let cancel = Rc::new(AtomicBool::new(false));
    let stop = Rc::clone(&cancel);
    let sub = t.fx.root.join("day/sub");
    let seen = std::cell::Cell::new(0);
    let disk = Faulty {
        on_list: Some(Box::new(move |dir: &Path| {
            if dir == sub {
                seen.set(seen.get() + 1);
                if seen.get() == 2 {
                    stop.store(true, Ordering::Relaxed)
                }
            }
        })),
        ..Faulty::default()
    };
    let plan = plan_on(&t.fx, "day", &trash, &disk)?;
    let batch = plan.batch().clone();
    let outcome = execute::execute_with(
        &mut t.fx.cat,
        plan.confirm(),
        &trash,
        Cancel::new(&cancel),
        &disk,
    )?;
    assert_eq!(
        results(&outcome),
        [ItemResult::Unchanged(Some("Stopped.".into()))]
    );
    assert_eq!(*trash.calls.borrow(), 0);
    assert!(t.fx.root.join("day/a.NEF").is_file());
    assert!(rows_intact(&t));
    assert_eq!(item(&t.fx, &batch).state, ItemState::NoChange);
    Ok(())
}

// The Trash failing.

#[test]
fn a_drive_whose_trash_refuses_it_when_run_leaves_it_as_it_was() -> Result<()> {
    let mut t = trip();
    let mut trash = FakeTrash::new(&t.fx);
    trash.fail_from = Some(1);
    let (_, outcome) = run(&mut t.fx, "day", &trash, &Faulty::default());
    assert_eq!(
        results(&outcome),
        [ItemResult::Unchanged(Some(
            TrashError::Unsupported.to_string()
        ))]
    );
    assert!(t.fx.root.join("day/a.NEF").is_file());
    assert!(rows_intact(&t));
    Ok(())
}

#[test]
fn a_trash_failing_part_way_is_left_for_the_user_until_everything_is_back() -> Result<()> {
    let mut t = trip();
    let was = every_byte(&t.fx);
    let mut trash = FakeTrash::new(&t.fx);
    trash.partial = true;
    let (batch, outcome) = run(&mut t.fx, "day", &trash, &Faulty::default());
    match &results(&outcome)[..] {
        [ItemResult::NeedsRecovery(why)] => {
            assert!(
                why.starts_with("the Trash failed part way: not all its files"),
                "{why}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(rows_intact(&t));
    assert_eq!(every_byte(&t.fx), was);
    // Recovery keeps it so, never "nothing happened".
    t.fx.reopen();
    let recovery = t.fx.recover(&Faulty::default());
    assert_eq!(recovery.unfinished.len(), 1);
    assert_eq!(recovery.unfinished[0].choices, [Resolution::Keep]);
    assert_eq!(item(&t.fx, &batch).state, ItemState::NeedsRecovery);
    // The user puts it back from the Trash.
    let taken = std::fs::read_dir(&trash.dir)?.next().unwrap()?.path();
    std::fs::rename(&taken, t.fx.root.join("day/.hidden"))?;
    let recovery = t.fx.recover(&Faulty::default());
    assert!(recovery.unfinished.is_empty());
    assert_eq!(item(&t.fx, &batch).state, ItemState::NoChange);
    assert!(rows_intact(&t));
    Ok(())
}

#[test]
fn a_failed_sync_after_the_trash_puts_it_back() -> Result<()> {
    let mut t = trip();
    let trash = FakeTrash::new(&t.fx);
    let disk = Faulty::at(Op::Sync, 0, Fault::Fail);
    let (batch, outcome) = run(&mut t.fx, "day", &trash, &disk);
    assert_eq!(
        results(&outcome),
        [ItemResult::Unchanged(Some("injected failure".into()))],
        "{outcome:?}"
    );
    assert!(t.fx.root.join("day/sub/b.NEF").is_file());
    assert!(rows_intact(&t));
    assert!(
        item(&t.fx, &batch)
            .members
            .iter()
            .all(|m| m.state == MemberState::Restored)
    );
    // And when it can't go back, its name taken meanwhile.
    let mut t = trip();
    let day = t.fx.root.join("day");
    let disk = Faulty {
        faults: vec![(Op::Sync, 0, Fault::Fail)],
        ..Faulty::default()
    };
    let mut trash = FakeTrash::new(&t.fx);
    trash.after = Some(Box::new(move |_: &Path| std::fs::create_dir(&day).unwrap()));
    let (batch, outcome) = run(&mut t.fx, "day", &trash, &disk);
    assert_eq!(
        results(&outcome),
        [ItemResult::NeedsRecovery(recover::NOT_BACK.into())]
    );
    assert_eq!(item(&t.fx, &batch).state, ItemState::Restoring);
    assert!(rows_intact(&t));
    Ok(())
}

#[test]
fn a_catalog_that_cant_be_written_after_the_trash_is_finished_by_recovery_with_no_note()
-> Result<()> {
    let mut t = trip();
    let trash = FakeTrash::new(&t.fx);
    t.fx.fail_journal("BEFORE DELETE ON photos");
    let (batch, outcome) = run(&mut t.fx, "day", &trash, &Faulty::default());
    assert!(
        matches!(results(&outcome)[..], [ItemResult::NeedsRecovery(_)]),
        "{outcome:?}"
    );
    assert!(rows_intact(&t));
    t.fx.reopen();
    let recovery = t.fx.recover(&Faulty::default());
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    assert_eq!(recovery.finished[0].result, ItemResult::Trashed);
    assert_eq!(
        recovery.finished[0].note, None,
        "no earlier failure carried"
    );
    assert_eq!(item(&t.fx, &batch).error, None);
    assert!(!t.fx.rows().contains_key(&t.a));
    Ok(())
}

#[test]
fn a_receipt_that_cant_be_journalled_leaves_it_may_be_in_the_trash() -> Result<()> {
    for sync_fails in [false, true] {
        let mut t = trip();
        let trash = FakeTrash::new(&t.fx);
        t.fx.fail_journal("BEFORE UPDATE OF receipt ON file_operation_members");
        let disk = Faulty {
            faults: match sync_fails {
                true => vec![(Op::Sync, 0, Fault::Fail)],
                false => vec![],
            },
            ..Faulty::default()
        };
        let plan = plan_on(&t.fx, "day", &trash, &disk)?;
        let batch = plan.batch().clone();
        let outcome = execute(&mut t.fx, plan, &trash, &disk);
        assert!(outcome.stopped.is_some(), "the run stops with the error");
        assert_eq!(item(&t.fx, &batch).state, ItemState::Planned);
        t.fx.reopen();
        let recovery = t.fx.recover(&Faulty::default());
        assert_eq!(recovery.unfinished.len(), 1);
        assert_eq!(recovery.unfinished[0].reason, tree_trash::MAY_BE_IN_TRASH);
        assert_eq!(
            recovery.unfinished[0].choices,
            [Resolution::Keep, Resolution::RemoveFromCatalog]
        );
        assert!(rows_intact(&t));
    }
    Ok(())
}

#[test]
fn what_slipped_in_after_the_check_goes_too_and_is_named_when_the_trash_can_be_read() -> Result<()>
{
    for readable in [true, false] {
        let mut t = trip();
        let mut trash = FakeTrash::new(&t.fx);
        trash.meanwhile = Some(Box::new(|dir: &Path| {
            std::fs::write(dir.join("late.NEF"), b"late").unwrap()
        }));
        let disk = Faulty::default();
        if !readable {
            disk.unlistable.borrow_mut().push(trash.dir.clone());
        }
        let (_, outcome) = run(&mut t.fx, "day", &trash, &disk);
        assert_eq!(results(&outcome), [ItemResult::Trashed], "{outcome:?}");
        let note = outcome.items[0].note.clone();
        match readable {
            true => assert_eq!(
                note.as_deref(),
                Some("the Trash also took late.NEF, added after it was checked")
            ),
            false => assert_eq!(note, None),
        }
        assert!(std::fs::read(trash.dir.join("1-day/late.NEF")).is_ok_and(|b| b == b"late"));
    }
    Ok(())
}

#[test]
fn a_folder_swapped_in_just_before_the_trash_keeps_the_rows_unless_the_trash_cant_be_read()
-> Result<()> {
    for names in [false, true] {
        for readable in [true, false] {
            let mut t = trip();
            let disk = match names {
                true => names_only(&t.fx),
                false => Faulty::default(),
            };
            let mut trash = FakeTrash::at(&trash_at(&t.fx));
            let away = t.fx.dir.path().canonicalize()?.join("swapped");
            trash.meanwhile = Some(Box::new(move |dir: &Path| {
                std::fs::rename(dir, &away).unwrap();
                std::fs::create_dir(dir).unwrap();
            }));
            if !readable {
                disk.unlistable.borrow_mut().push(trash.dir.clone());
            }
            let (_, outcome) = run(&mut t.fx, "day", &trash, &disk);
            let case = format!("names {names}, readable {readable}");
            match (readable, &results(&outcome)[..]) {
                (true, [ItemResult::NeedsRecovery(why)]) => {
                    assert!(why.contains("isn't the one"), "{case}: {why}");
                    assert!(rows_intact(&t), "{case}");
                }
                // As for a photo's Trash: the accepted residual race.
                (false, [ItemResult::Trashed]) => assert!(!rows_intact(&t), "{case}"),
                other => panic!("{case}: {other:?}"),
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_trashed_folder_that_cant_be_walked_keeps_the_rows_and_recovery_never_walks_in_its_transaction()
-> Result<()> {
    use super::tests::Call;
    for names in [false, true] {
        let mut t = trip();
        let disk = match names {
            true => names_only(&t.fx),
            false => Faulty::default(),
        };
        let mut trash = FakeTrash::at(&trash_at(&t.fx));
        let target = t.fx.root.join("r.NEF");
        trash.after = Some(Box::new(move |at: &Path| {
            std::os::unix::fs::symlink(&target, at.join("link")).unwrap()
        }));
        let (batch, outcome) = run(&mut t.fx, "day", &trash, &disk);
        match &results(&outcome)[..] {
            [ItemResult::NeedsRecovery(why)] => assert!(why.contains("can't be read"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(rows_intact(&t));
        // Recovery reads the receipt before its transaction, never in it.
        t.fx.reopen();
        let receipt = item(&t.fx, &batch).members[0].receipt.clone().unwrap();
        let catalog = t.fx.catalog.clone();
        let inside = Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = Rc::clone(&inside);
        let disk = Faulty {
            watch: Some(Box::new(move |call: &Call| {
                let path = match call {
                    Call::List(p) | Call::Entry(p) => p,
                    Call::Identity(p) => {
                        if writing(&catalog) {
                            seen.borrow_mut().push(format!("{call:?}"));
                        }
                        p
                    }
                    _ => return,
                };
                if path.starts_with(&receipt) && writing(&catalog) {
                    seen.borrow_mut().push(format!("{call:?}"));
                }
            })),
            ..match names {
                true => names_only(&t.fx),
                false => Faulty::default(),
            }
        };
        let recovery = t.fx.recover(&disk);
        assert_eq!(recovery.unfinished.len(), 1);
        assert!(inside.borrow().is_empty(), "{:?}", inside.borrow());
        assert!(rows_intact(&t));
    }
    Ok(())
}

/// Whether a write transaction is open on the catalog at `path`: another
/// connection can't begin one at once.
fn writing(path: &Path) -> bool {
    let db = rusqlite::Connection::open(path).unwrap();
    db.busy_timeout(std::time::Duration::ZERO).unwrap();
    match db.execute_batch("BEGIN IMMEDIATE; ROLLBACK;") {
        Ok(()) => false,
        Err(_) => true,
    }
}

// Crashes at every step, then a restart and recovery.

/// Runs `folder`'s Trash with `trash` on `disk`, which "crashes" somewhere;
/// then reopens and recovers on a fresh disk (remounted first on a
/// names-only one). Returns the batch and the recovery.
fn crash_then_recover(
    t: &mut Trip,
    names: bool,
    trash: &FakeTrash,
    disk: &Faulty,
) -> (BatchId, Recovery) {
    let plan = plan_on(&t.fx, "day", trash, disk).unwrap();
    let batch = plan.batch().clone();
    if names {
        remount(disk, &t.fx.root.join("day"));
    }
    let fx = &mut t.fx;
    assert!(crashes(|| {
        let _ = execute(fx, plan, trash, disk);
    }));
    t.fx.reopen();
    let fresh = match names {
        true => names_only(&t.fx),
        false => Faulty::default(),
    };
    if names {
        for dir in [t.fx.root.join("day"), trash.dir.clone()] {
            if dir.exists() {
                remount(&fresh, &dir);
            }
        }
    }
    let recovery = t.fx.recover(&fresh);
    (batch, recovery)
}

fn on(names: bool, t: &Trip) -> Faulty {
    match names {
        true => names_only(&t.fx),
        false => Faulty::default(),
    }
}

/// A hook that "crashes" at the `nth` listing (from 1) of `dir`.
fn crash_listing(dir: PathBuf, nth: usize) -> PathHook {
    let seen = std::cell::Cell::new(0);
    Some(Box::new(move |listed: &Path| {
        if listed == dir {
            seen.set(seen.get() + 1);
            if seen.get() == nth {
                panic!("injected crash listing {}", listed.display())
            }
        }
    }))
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the same code runs on Linux and macOS; Windows CI is near its time limit"
)]
fn a_crash_before_the_trash_changes_nothing() {
    for names in [false, true] {
        // Just after the journal (the last walk's first listing; the plan
        // listed it once), in the middle of that walk, and just before the
        // Trash call.
        for case in ["journalled", "mid-walk", "before the Trash"] {
            let mut t = trip();
            let was = t.fx.files();
            let mut trash = FakeTrash::at(&trash_at(&t.fx));
            let mut disk = on(names, &t);
            match case {
                "journalled" => disk.on_list = crash_listing(t.fx.root.join("day"), 2),
                "mid-walk" => disk.on_list = crash_listing(t.fx.root.join("day/sub"), 2),
                _ => trash.crash_before = Some(1),
            }
            let (batch, recovery) = crash_then_recover(&mut t, names, &trash, &disk);
            let at = format!("{case}, names {names}");
            assert!(recovery.unfinished.is_empty(), "{at}: {recovery:?}");
            assert_eq!(item(&t.fx, &batch).state, ItemState::NoChange, "{at}");
            assert_eq!(t.fx.files(), was, "{at}");
            assert!(rows_intact(&t), "{at}");
        }
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the same code runs on Linux and macOS; Windows CI is near its time limit"
)]
fn a_crash_after_the_trash_before_its_receipt_is_journalled_leaves_it_may_be_in_the_trash()
-> Result<()> {
    for names in [false, true] {
        for case in ["trash", "sync before", "sync after"] {
            let mut t = trip();
            let mut trash = FakeTrash::at(&trash_at(&t.fx));
            let mut disk = on(names, &t);
            match case {
                "trash" => trash.crash_after = Some(1),
                "sync before" => disk.faults = vec![(Op::Sync, 0, Fault::CrashBefore)],
                _ => disk.faults = vec![(Op::Sync, 0, Fault::CrashAfter)],
            }
            let (batch, recovery) = crash_then_recover(&mut t, names, &trash, &disk);
            let at = format!("{case}, names {names}");
            assert_eq!(recovery.unfinished.len(), 1, "{at}");
            let u = &recovery.unfinished[0];
            assert_eq!(u.reason, tree_trash::MAY_BE_IN_TRASH, "{at}");
            assert_eq!(
                u.choices,
                [Resolution::Keep, Resolution::RemoveFromCatalog],
                "{at}"
            );
            assert!(rows_intact(&t), "{at}");
            let fresh = on(names, &t);
            assert!(matches!(
                recover::resolve_with(
                    &mut t.fx.cat,
                    u.operation,
                    Resolution::PutBack,
                    &fresh,
                    &NoTrash
                ),
                Err(FileOpError::Unsupported(_))
            ));
            // Keep leaves the rows; never revisited.
            if case == "trash" {
                let kept = recover::resolve_with(
                    &mut t.fx.cat,
                    u.operation,
                    Resolution::Keep,
                    &fresh,
                    &NoTrash,
                )?;
                assert_eq!(kept.result, ItemResult::Kept);
                assert!(rows_intact(&t));
                assert!(t.fx.recover(&fresh).unfinished.is_empty());
                assert_eq!(item(&t.fx, &batch).state, ItemState::Kept);
            } else {
                // Remove from Catalog takes its rows, as the user chose.
                let removed = recover::resolve_with(
                    &mut t.fx.cat,
                    u.operation,
                    Resolution::RemoveFromCatalog,
                    &fresh,
                    &NoTrash,
                )?;
                assert_eq!(removed.result, ItemResult::Removed, "{at}");
                assert!(!t.fx.rows().contains_key(&t.a), "{at}");
                assert_eq!(t.fx.rows()[&t.c], "other/c.NEF");
            }
        }
    }
    Ok(())
}

#[test]
fn remove_from_catalog_is_refused_while_the_folder_is_where_it_was() -> Result<()> {
    let mut t = trip();
    let mut trash = FakeTrash::new(&t.fx);
    trash.crash_after = Some(1);
    let (_, recovery) = crash_then_recover(&mut t, false, &trash, &Faulty::default());
    let operation = recovery.unfinished[0].operation;
    // The user put it back by hand.
    let taken = trash.dir.join("1-day");
    std::fs::rename(&taken, t.fx.root.join("day"))?;
    // A file of it changed meanwhile, so it isn't "nothing happened".
    std::fs::write(t.fx.root.join("day/notes.txt"), b"changed")?;
    let refused = recover::resolve_with(
        &mut t.fx.cat,
        operation,
        Resolution::RemoveFromCatalog,
        &Faulty::default(),
        &NoTrash,
    );
    assert!(
        matches!(refused, Err(FileOpError::Destination(_))),
        "{refused:?}"
    );
    assert!(rows_intact(&t));
    Ok(())
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the same code runs on Linux and macOS; Windows CI is near its time limit"
)]
fn a_crash_after_the_receipt_is_journalled_is_finished_by_recovery() {
    for names in [false, true] {
        // Reading the receipt before the catalog's transaction, and in it
        // (proving the folder gone from its path): rolled back.
        for in_transaction in [false, true] {
            let mut t = trip();
            let was = every_byte(&t.fx);
            let trash = FakeTrash::at(&trash_at(&t.fx));
            let mut disk = on(names, &t);
            disk.on_list = match in_transaction {
                false => crash_listing(trash.dir.join("1-day"), 1),
                true => {
                    let (root, catalog) = (t.fx.root.clone(), t.fx.catalog.clone());
                    Some(Box::new(move |dir: &Path| {
                        if dir == root && writing(&catalog) {
                            panic!("injected crash in the transaction")
                        }
                    }))
                }
            };
            let (batch, recovery) = crash_then_recover(&mut t, names, &trash, &disk);
            let at = format!("names {names}, in transaction {in_transaction}");
            assert!(recovery.unfinished.is_empty(), "{at}: {recovery:?}");
            assert_eq!(recovery.finished[0].result, ItemResult::Trashed, "{at}");
            assert_eq!(recovery.finished[0].note, None, "{at}");
            assert_eq!(item(&t.fx, &batch).state, ItemState::Done, "{at}");
            assert!(!t.fx.rows().contains_key(&t.a), "{at}");
            assert_eq!(every_byte(&t.fx), was, "{at}");
        }
    }
}

/// A run that crashed after journalling its receipt, before the catalog
/// followed, on `names` or ids.
fn crashed_with_receipt(names: bool) -> (Trip, FakeTrash, BatchId) {
    let mut t = trip();
    let trash = FakeTrash::at(&trash_at(&t.fx));
    let base = on(names, &t);
    let plan = plan_on(&t.fx, "day", &trash, &base).unwrap();
    let batch = plan.batch().clone();
    let disk = Faulty {
        on_list: crash_listing(trash.dir.join("1-day"), 1),
        ..base
    };
    let fx = &mut t.fx;
    assert!(crashes(|| {
        let _ = execute(fx, plan, &trash, &disk);
    }));
    t.fx.reopen();
    (t, trash, batch)
}

#[test]
fn a_folder_put_back_by_the_user_before_recovery_is_nothing_changed() -> Result<()> {
    let (mut t, trash, batch) = crashed_with_receipt(false);
    std::fs::rename(trash.dir.join("1-day"), t.fx.root.join("day"))?;
    let recovery = t.fx.recover(&Faulty::default());
    assert!(recovery.unfinished.is_empty());
    assert_eq!(item(&t.fx, &batch).state, ItemState::NoChange);
    assert!(rows_intact(&t));
    Ok(())
}

#[test]
fn an_emptied_trash_before_recovery_keeps_the_rows() -> Result<()> {
    let (mut t, trash, _) = crashed_with_receipt(false);
    std::fs::remove_dir_all(trash.dir.join("1-day"))?;
    let recovery = t.fx.recover(&Faulty::default());
    assert_eq!(recovery.unfinished.len(), 1);
    assert_eq!(
        recovery.unfinished[0].reason,
        "it isn't where the Trash put it"
    );
    assert!(rows_intact(&t));
    Ok(())
}

#[test]
fn a_catalog_changed_before_recovery_is_left_and_put_back_or_kept_as_chosen() -> Result<()> {
    for put_back in [true, false] {
        let (mut t, _trash, batch) = crashed_with_receipt(false);
        // Another window rates a photo of it.
        t.fx.cat
            .db_for_tests()
            .execute("UPDATE photos SET rating=3 WHERE id=?1", [t.b.0])?;
        let recovery = t.fx.recover(&Faulty::default());
        assert_eq!(recovery.unfinished.len(), 1);
        let u = &recovery.unfinished[0];
        assert_eq!(u.choices, [Resolution::PutBack, Resolution::Keep]);
        assert!(rows_intact(&t));
        let chosen = match put_back {
            true => Resolution::PutBack,
            false => Resolution::Keep,
        };
        let outcome = recover::resolve_with(
            &mut t.fx.cat,
            u.operation,
            chosen,
            &Faulty::default(),
            &NoTrash,
        )?;
        match put_back {
            true => {
                assert!(matches!(outcome.result, ItemResult::Unchanged(_)));
                assert!(t.fx.root.join("day/sub/b.NEF").is_file());
                assert_eq!(item(&t.fx, &batch).state, ItemState::NoChange);
            }
            false => assert_eq!(outcome.result, ItemResult::Kept),
        }
        assert!(rows_intact(&t));
    }
    Ok(())
}

#[test]
fn a_drive_offline_at_recovery_is_decided_once_it_is_back() -> Result<()> {
    let (mut t, trash, batch) = crashed_with_receipt(false);
    let offline = Faulty::default();
    offline.unlistable.borrow_mut().push(t.fx.root.clone());
    offline.unlistable.borrow_mut().push(trash.dir);
    let recovery = t.fx.recover(&offline);
    assert_eq!(recovery.unfinished.len(), 1, "{recovery:?}");
    assert!(rows_intact(&t));
    let recovery = t.fx.recover(&Faulty::default());
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    assert_eq!(item(&t.fx, &batch).state, ItemState::Done);
    Ok(())
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the same code runs on Linux and macOS; Windows CI is near its time limit"
)]
fn a_crash_while_putting_it_back_is_put_back_by_recovery_unless_its_name_is_taken() -> Result<()> {
    for taken in [false, true] {
        let mut t = trip();
        let trash = FakeTrash::new(&t.fx);
        // The sync fails, and the process ends in the put back's rename.
        let disk = Faulty::with(&[
            (Op::Sync, 0, Fault::Fail),
            (Op::Rename, 0, Fault::CrashBefore),
        ]);
        let plan = plan_on(&t.fx, "day", &trash, &disk)?;
        let batch = plan.batch().clone();
        let fx = &mut t.fx;
        assert!(crashes(|| {
            let _ = execute(fx, plan, &trash, &disk);
        }));
        t.fx.reopen();
        assert_eq!(item(&t.fx, &batch).state, ItemState::Restoring);
        if taken {
            std::fs::create_dir(t.fx.root.join("day"))?;
        }
        let recovery = t.fx.recover(&Faulty::default());
        match taken {
            false => {
                assert!(recovery.unfinished.is_empty());
                assert_eq!(item(&t.fx, &batch).state, ItemState::NoChange);
                assert!(t.fx.root.join("day/a.NEF").is_file());
            }
            true => {
                assert_eq!(recovery.unfinished[0].reason, recover::NOT_BACK);
                assert_eq!(item(&t.fx, &batch).state, ItemState::Restoring);
            }
        }
        assert!(rows_intact(&t));
    }
    Ok(())
}

// Other operations while one is unfinished.

#[test]
fn an_unfinished_folder_trash_keeps_other_operations_off_its_subtree() -> Result<()> {
    // The Trash took part of it: unfinished, the folder where it was.
    let mut t = trip();
    let mut trash = FakeTrash::new(&t.fx);
    trash.partial = true;
    let (_, outcome) = run(&mut t.fx, "day", &trash, &Faulty::default());
    assert!(matches!(
        results(&outcome)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    // Planning a rename or a move of it or of a folder in it.
    for folder in ["day", "day/sub"] {
        match plan(
            &t.fx.cat,
            FileOp::RenameFolder {
                root: t.fx.root_id(),
                folder: folder.into(),
                name: "else".into(),
            },
        ) {
            Err(FileOpError::Destination(why)) => {
                assert_eq!(why, "An unfinished file operation involves it.")
            }
            other => panic!("{other:?}"),
        }
    }
    // Remove from Catalog, and a Trash, of a photo in it.
    let outcome = t.fx.run(
        FileOp::RemoveFromCatalog { photos: vec![t.b] },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(matches!(
        &results(&outcome)[..],
        [ItemResult::Unchanged(Some(_))]
    ));
    let outcome = t.fx.run(
        FileOp::TrashPhotos { photos: vec![t.b] },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(
        results(&outcome),
        [ItemResult::Unchanged(Some(execute::UNFINISHED.into()))]
    );
    assert!(rows_intact(&t));
    // Unrelated ones run.
    let outcome = t.fx.run(
        FileOp::RenameFolder {
            root: t.fx.root_id(),
            folder: "other".into(),
            name: "else".into(),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::FolderRenamed]);
    Ok(())
}

// Compatibility.

#[test]
fn the_journal_reads_this_release_s_first_shape() -> Result<()> {
    let first = r#"{"root":1,"from":"day","from_path":"/p/trip/day","folders":[[2,"day"]],"photos":[3,4],"families":[[3,"0000000000000000000000000000000000000000000000000000000000000000","1111111111111111111111111111111111111111111111111111111111111111"]],"proof":"names","since":1761100000}"#;
    let state: tree_trash::TrashTreeState = serde_json::from_str(first)?;
    assert_eq!(state.proof, proof::Proof::Names);
    assert_eq!(state.photos, [PhotoId(3), PhotoId(4)]);
    // Fields added later default when absent.
    let bare = r#"{"root":1,"from":"day","from_path":"/p/trip/day","folders":[],"photos":[],"families":[]}"#;
    let state: tree_trash::TrashTreeState = serde_json::from_str(bare)?;
    assert_eq!((state.proof, state.since), (proof::Proof::Ids, 0));
    assert_eq!(
        serde_json::from_str::<tree_trash::TrashTreeState>(&serde_json::to_string(&state)?)?,
        state
    );
    assert_eq!(
        "trash_folder".parse::<journal::Kind>()?,
        journal::Kind::TrashFolder
    );
    Ok(())
}

#[test]
fn an_item_of_the_first_shape_is_recovered() -> Result<()> {
    // A crash after the receipt, its preconditions rewritten as this
    // release's first shape wrote them: recovery still finishes it.
    let (mut t, _trash, batch) = crashed_with_receipt(false);
    let stored: String = t.fx.cat.db_for_tests().query_row(
        "SELECT preconditions FROM file_operations WHERE batch=?1",
        [&batch.0],
        |r| r.get(0),
    )?;
    let mut value: serde_json::Value = serde_json::from_str(&stored)?;
    let object = value.as_object_mut().unwrap();
    object.remove("since");
    object.remove("proof");
    t.fx.cat.db_for_tests().execute(
        "UPDATE file_operations SET preconditions=?1 WHERE batch=?2",
        [&value.to_string(), &batch.0],
    )?;
    let recovery = t.fx.recover(&Faulty::default());
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    assert_eq!(item(&t.fx, &batch).state, ItemState::Done);
    Ok(())
}

// Size.

/// A tree of `folders` folders of `files` small files each below `big/`.
fn big(folders: usize, files: usize) -> Fx {
    let fx = fixture(&["keep.NEF", "big/a.NEF"]);
    for d in 0..folders {
        let dir = fx.root.join(format!("big/d{d}"));
        std::fs::create_dir(&dir).unwrap();
        for f in 0..files {
            std::fs::write(dir.join(format!("f{f}.txt")), [d as u8, f as u8]).unwrap();
        }
    }
    fx
}

/// Plans, runs and recovers a big tree's Trash, counting listings.
fn measure(folders: usize, files: usize) -> Result<()> {
    let mut fx = big(folders, files);
    let trash = FakeTrash::new(&fx);
    let disk = Faulty::default();
    let started = std::time::Instant::now();
    let plan = plan_on(&fx, "big", &trash, &disk)?;
    let planned = started.elapsed();
    let lists = disk.count(Op::List);
    let outcome = execute(&mut fx, plan, &trash, &disk);
    assert_eq!(results(&outcome), [ItemResult::Trashed]);
    let ran = started.elapsed() - planned;
    let lists_run = disk.count(Op::List) - lists;
    // Each folder listed a few times at most: planned, checked, the receipt.
    let dirs = folders + 1;
    assert!(
        lists <= dirs * 2 + 16,
        "{lists} listings to plan {dirs} folders"
    );
    assert!(
        lists_run <= dirs * 4 + 32,
        "{lists_run} listings to run {dirs} folders"
    );
    eprintln!(
        "{} files in {dirs} folders: planned in {planned:?} ({lists} listings), run in {ran:?} ({lists_run} listings)",
        folders * files + 1
    );
    Ok(())
}

#[test]
fn a_tree_is_walked_once_per_check_not_once_per_name() -> Result<()> {
    measure(10, 30)
}

#[test]
#[ignore = "measures a 10,000-file tree; run by hand"]
fn a_ten_thousand_file_tree() -> Result<()> {
    measure(100, 100)
}

#[test]
fn a_folder_put_back_after_a_failed_sync_is_back_even_with_a_file_changed_meanwhile() -> Result<()>
{
    let mut t = trip();
    let mut trash = FakeTrash::new(&t.fx);
    // A file changes in the moment between the last check and the Trash.
    trash.meanwhile = Some(Box::new(|dir: &Path| {
        std::fs::write(dir.join("notes.txt"), b"changed meanwhile").unwrap()
    }));
    let disk = Faulty::at(Op::Sync, 0, Fault::Fail);
    let (batch, outcome) = run(&mut t.fx, "day", &trash, &disk);
    assert!(
        matches!(results(&outcome)[..], [ItemResult::Unchanged(_)]),
        "{outcome:?}"
    );
    assert_eq!(item(&t.fx, &batch).state, ItemState::NoChange);
    assert!(t.fx.root.join("day/notes.txt").is_file());
    assert!(rows_intact(&t));
    // And a restoring item whose folder is back is decided so too.
    t.fx.cat.db_for_tests().execute(
        "UPDATE file_operations SET state='restoring' WHERE batch=?1",
        [&batch.0],
    )?;
    assert!(t.fx.recover(&Faulty::default()).unfinished.is_empty());
    assert_eq!(item(&t.fx, &batch).state, ItemState::NoChange);
    Ok(())
}

#[test]
fn put_back_never_takes_another_folder_of_its_name_for_the_one_in_the_trash() -> Result<()> {
    for names in [false, true] {
        let (mut t, trash, batch) = crashed_with_receipt(names);
        // Left for the user: the catalog changed meanwhile.
        t.fx.cat
            .db_for_tests()
            .execute("UPDATE photos SET rating=3 WHERE id=?1", [t.b.0])?;
        let fresh = on(names, &t);
        let recovery = t.fx.recover(&fresh);
        let operation = recovery.unfinished[0].operation;
        // Another folder of its name, holding a file, where it was.
        std::fs::create_dir(t.fx.root.join("day"))?;
        std::fs::write(t.fx.root.join("day/other.txt"), b"other")?;
        let outcome = recover::resolve_with(
            &mut t.fx.cat,
            operation,
            Resolution::PutBack,
            &fresh,
            &NoTrash,
        )?;
        let at = format!("names {names}");
        assert_eq!(
            outcome.result,
            ItemResult::NeedsRecovery(recover::NOT_BACK.into()),
            "{at}"
        );
        assert_eq!(item(&t.fx, &batch).state, ItemState::Restoring, "{at}");
        assert!(
            trash.dir.join("1-day/sub/b.NEF").is_file(),
            "{at}: still in the Trash"
        );
        // Recovery doesn't take it for back either.
        let recovery = t.fx.recover(&on(names, &t));
        assert_eq!(recovery.unfinished.len(), 1, "{at}");
        assert!(rows_intact(&t), "{at}");
    }
    Ok(())
}
