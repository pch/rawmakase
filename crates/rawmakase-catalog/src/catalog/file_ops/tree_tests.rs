//! Renaming a folder: the directory renamed once, the catalog's folders of
//! its subtree re-keyed, its photos following; what is refused; and what a
//! fault at any step leaves, which recovery decides from the journal and
//! the disk.
use super::recover_tests::{before, check};
use super::tests::{Fault, Faulty, Fx, NoTrash, Op, crashes, fixture, results};
use super::*;
use crate::catalog::{Catalog, HistoryUpdate};
use crate::export_settings::ExportOptions;
use crate::fsops;
use crate::model::recipe::Recipe;
use anyhow::Result;
use rawmakase_cancel::Cancel;

fn rename_folder(fx: &Fx, relative: &str, name: &str) -> FileOp {
    FileOp::RenameFolder {
        root: fx.cat.roots().unwrap()[0].0,
        folder: relative.into(),
        name: name.into(),
    }
}

/// Each catalog folder's logical path and legacy relative path, by id.
fn keys(fx: &Fx) -> Vec<(FolderId, Option<String>, String)> {
    let db = fx.cat.db_for_tests();
    let mut stmt = db
        .prepare(
            "SELECT f.id, p.path, f.relative_path FROM folders f
             LEFT JOIN folder_paths p ON p.folder=f.id ORDER BY f.id",
        )
        .unwrap();
    stmt.query_map([], |r| Ok((FolderId(r.get(0)?), r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The plan's refusal, as the user reads it.
fn refusal(fx: &Fx, op: FileOp) -> String {
    match plan(&fx.cat, op) {
        Err(FileOpError::Destination(why)) => why,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn a_folder_is_renamed_with_its_subtree_and_its_photos_follow() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/a.xmp", "day/sub/b.NEF", "other/c.NEF"]);
    let (a, b) = (fx.photo("day/a.NEF"), fx.photo("day/sub/b.NEF"));
    let (day, sub) = (fx.folder("day"), fx.folder("day/sub"));
    let source = fx.root.join("day/a.NEF");
    fx.cat.save_edit(
        a,
        &source,
        &crate::storage::Identity::read(&source)?,
        &Recipe::default(),
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    let (dir_id, file_id) = (
        fsops::entry(&fx.root.join("day"))?.id,
        fsops::entry(&source)?.id,
    );
    let plan = fx.plan(rename_folder(&fx, "day", "Day 1"));
    assert_eq!(plan.summary().photos, 2);
    assert_eq!(plan.summary().folder.as_deref(), Some("Day 1"));
    // Ids follow the folder's listing order, which differs between systems.
    let mut both = [a, b];
    both.sort();
    let mut planned = plan.photos();
    planned.sort();
    assert_eq!(planned, both);
    assert!(fx.root.join("day").is_dir(), "planning renames nothing");
    let outcome = fx.run(
        rename_folder(&fx, "day", "Day 1"),
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::FolderRenamed]);
    let mut photos = outcome.items[0].photos.clone();
    photos.sort();
    assert_eq!(photos, both);
    assert!(!fx.root.join("day").exists());
    assert_eq!(fsops::entry(&fx.root.join("Day 1"))?.id, dir_id);
    let target = fx.root.join("Day 1/a.NEF");
    assert_eq!(fsops::entry(&target)?.id, file_id);
    let rows = fx.rows();
    assert_eq!(rows[&a], "Day 1/a.NEF");
    assert_eq!(rows[&b], "Day 1/sub/b.NEF");
    assert_eq!(fx.folder("Day 1"), day, "folders keep their ids");
    assert_eq!(fx.folder("Day 1/sub"), sub);
    let keys = keys(&fx);
    let key = |id| keys.iter().find(|(f, _, _)| *f == id).unwrap().clone();
    assert_eq!(key(day), (day, Some("Day 1".into()), "Day 1".into()));
    let native: std::path::PathBuf = ["Day 1", "sub"].iter().collect();
    assert_eq!(
        key(sub),
        (
            sub,
            Some("Day 1/sub".into()),
            native.to_string_lossy().into()
        )
    );
    assert!(
        fx.cat.load_edit(a, &target)?.is_some(),
        "the edit still opens"
    );
    Ok(())
}

#[test]
fn a_folder_below_one_the_catalog_lacks_and_one_an_older_release_added_are_renamed() -> Result<()> {
    // Only 2024/day1 and 2024/day1/x are catalog folders; x has no logical
    // path yet, as an older release adds folders.
    let mut fx = fixture(&["2024/day1/a.NEF", "2024/day1/x/b.NEF"]);
    let (day, x) = (fx.folder("2024/day1"), fx.folder("2024/day1/x"));
    fx.cat
        .db_for_tests()
        .execute("DELETE FROM folder_paths WHERE folder=?1", [x.0])?;
    let outcome = fx.run(
        rename_folder(&fx, "2024/day1", "day 1"),
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::FolderRenamed]);
    assert_eq!(fx.folder("2024/day 1"), day, "the same folder");
    assert_eq!(
        fx.rows()[&fx.photo("2024/day 1/x/b.NEF")],
        "2024/day 1/x/b.NEF"
    );
    let keys = keys(&fx);
    assert!(
        keys.contains(&(
            x,
            Some("2024/day 1/x".into()),
            ["2024", "day 1", "x"]
                .iter()
                .collect::<std::path::PathBuf>()
                .to_string_lossy()
                .into()
        ))
    );
    Ok(())
}

#[test]
fn a_root_or_a_name_a_folder_cant_have_is_refused() {
    let fx = fixture(&["a.NEF", "day/b.NEF"]);
    assert!(refusal(&fx, rename_folder(&fx, "", "Trip")).contains("root folder"));
    for name in ["", "a/b", "CON", "x."] {
        let why = refusal(&fx, rename_folder(&fx, "day", name));
        assert_eq!(Some(why.as_str()), folder_name_problem(name), "{name:?}");
    }
    assert_eq!(
        refusal(&fx, rename_folder(&fx, "day", "day")),
        "It has that name already."
    );
}

#[test]
fn a_name_on_disk_or_in_the_catalog_is_never_merged_into() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "taken/x.NEF"]);
    std::fs::write(fx.root.join("file.txt"), b"file")?;
    let root = fx.root.clone();
    // A missing catalog folder, and one below a path the catalog lacks.
    fx.add_row(&root, "Gone", "x.NEF");
    fx.add_row(&root, "Deep/below", "y.NEF");
    for name in ["taken", "TAKEN", "file.txt", "Gone", "gone", "Deep"] {
        let why = refusal(&fx, rename_folder(&fx, "day", name));
        let case = fsops::probe(&fx.root, &fsops::list(&fx.root)?).case;
        if name == "TAKEN" && case == fsops::Folding::Exact {
            continue;
        }
        assert!(
            why.contains("already there") || why.contains("catalog has a folder"),
            "{name}: {why}"
        );
    }
    assert!(fx.root.join("day/a.NEF").is_file());
    Ok(())
}

#[test]
fn a_subtree_with_a_location_of_its_own_is_refused_on_any_computer() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/sub/b.NEF"]);
    let elsewhere = fx.dir.path().canonicalize()?.join("elsewhere");
    std::fs::create_dir(&elsewhere)?;
    // Another computer finds day/sub somewhere of its own.
    let other = crate::catalog::locations::Computer {
        id: "another-computer".into(),
        name: "Other".into(),
    };
    let sub = fx.folder("day/sub");
    let mut there = Catalog::open_as(&fx.catalog, &other)?;
    there.relink_folder(sub, &elsewhere)?;
    drop(there);
    // Not only another case of it, which some volumes refuse first.
    let why = refusal(&fx, rename_folder(&fx, "day", "Week"));
    assert!(why.contains("located on its own"), "{why}");
    // And this one, the folder itself.
    fx.cat.db_for_tests().execute(
        "DELETE FROM folder_locations WHERE computer='another-computer'",
        [],
    )?;
    fx.cat
        .db_for_tests()
        .execute("DELETE FROM folder_mappings", [])?;
    assert!(plan(&fx.cat, rename_folder(&fx, "day", "Day 2")).is_ok());
    let day = fx.folder("day");
    fx.cat.relink_folder(day, &fx.root.join("day"))?;
    let why = refusal(&fx, rename_folder(&fx, "day", "Day 2"));
    assert!(why.contains("located on its own"), "{why}");
    Ok(())
}

#[test]
fn a_folder_another_root_or_the_catalog_is_inside_is_refused() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/sub/b.NEF"]);
    // A second root over a folder inside it, as an older release or
    // another tool might have added.
    let sub = fx.root.join("day/sub");
    fx.add_row(&sub, "", "b.NEF");
    let why = refusal(&fx, rename_folder(&fx, "day", "Other"));
    assert_eq!(
        why,
        "Another folder or root of the catalog is inside it on disk."
    );
    // A catalog inside it.
    let fx = fixture(&["day/a.NEF"]);
    let inside = fx.root.join("day/Inside.rawmakase");
    let mut cat = Catalog::create(&inside)?;
    cat.add_folder(&fx.root)?;
    match plan(
        &cat,
        FileOp::RenameFolder {
            root: cat.roots()?[0].0,
            folder: "day".into(),
            name: "Day".into(),
        },
    ) {
        Err(FileOpError::Destination(why)) => assert!(why.contains("catalog"), "{why}"),
        other => panic!("{other:?}"),
    }
    Ok(())
}

#[test]
fn a_case_only_folder_rename_works_where_the_volume_renames_spelling() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF"]);
    let a = fx.photo("day/a.NEF");
    let facts = fsops::volume_facts(&fx.root)?;
    let case = fsops::probe(&fx.root, &fsops::list(&fx.root)?).case;
    let op = rename_folder(&fx, "day", "Day");
    if !facts.renames_spelling && case != fsops::Folding::Exact {
        assert!(refusal(&fx, op).contains("only the case"));
        return Ok(());
    }
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    assert!(
        matches!(results(&outcome)[..], [ItemResult::FolderRenamed]),
        "{outcome:?}"
    );
    assert_eq!(fsops::list(&fx.root)?, ["Day"]);
    assert_eq!(fx.rows()[&a], "Day/a.NEF");
    Ok(())
}

#[test]
fn a_name_taken_between_plan_and_execute_is_left_alone() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF"]);
    let a = fx.photo("day/a.NEF");
    let plan = fx.plan(rename_folder(&fx, "day", "New"));
    std::fs::create_dir(fx.root.join("New"))?;
    std::fs::write(fx.root.join("New/theirs.txt"), b"theirs")?;
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
    assert_eq!(std::fs::read(fx.root.join("New/theirs.txt"))?, b"theirs");
    Ok(())
}

#[test]
fn a_folder_made_just_before_the_rename_is_never_merged_into() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF"]);
    let a = fx.photo("day/a.NEF");
    struct Racing(Faulty);
    impl disk::Disk for Racing {
        fn entry(&self, p: &std::path::Path) -> Result<fsops::Entry, fsops::FsError> {
            self.0.entry(p)
        }
        fn list(&self, d: &std::path::Path) -> Result<Vec<std::ffi::OsString>, fsops::FsError> {
            self.0.list(d)
        }
        fn identity(&self, p: &std::path::Path) -> anyhow::Result<crate::storage::Identity> {
            self.0.identity(p)
        }
        fn settle_rename(
            &self,
            a: &std::path::Path,
            b: &std::path::Path,
        ) -> Result<(), fsops::FsError> {
            self.0.settle_rename(a, b)
        }
        fn rename(&self, a: &std::path::Path, b: &std::path::Path) -> Result<(), fsops::FsError> {
            std::fs::create_dir(b).unwrap();
            self.0.rename(a, b)
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
        tests::passes_on!(0);
    }
    let outcome = fx.run(
        rename_folder(&fx, "day", "New"),
        &NoTrash,
        &Racing(Faulty::default()),
    );
    assert!(
        matches!(results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
        "{outcome:?}"
    );
    assert!(fx.root.join("day/a.NEF").is_file());
    assert_eq!(std::fs::read_dir(fx.root.join("New"))?.count(), 0);
    assert_eq!(fx.rows()[&a], "day/a.NEF");
    Ok(())
}

#[test]
fn a_subtree_changed_in_the_catalog_after_planning_is_left_alone() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF"]);
    let a = fx.photo("day/a.NEF");
    let plan = fx.plan(rename_folder(&fx, "day", "New"));
    // Another connection adds a folder inside it.
    std::fs::create_dir(fx.root.join("day/late"))?;
    std::fs::write(fx.root.join("day/late/l.NEF"), b"late")?;
    let mut other = Catalog::open(&fx.catalog)?;
    other.add_folder(&fx.root.join("day/late"))?;
    drop(other);
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert_eq!(
        results(&outcome),
        [ItemResult::Unchanged(Some(preconditions::CHANGED.into()))]
    );
    assert!(fx.root.join("day").is_dir());
    assert_eq!(fx.rows()[&a], "day/a.NEF");
    Ok(())
}

#[test]
fn an_unfinished_operation_inside_it_refuses_the_folder() -> Result<()> {
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
    let why = refusal(&fx, rename_folder(&fx, "day", "Day 2"));
    assert_eq!(why, "An unfinished file operation involves it.");
    Ok(())
}

fn tree_files() -> Fx {
    fixture(&["day/a.NEF", "day/a.xmp", "day/sub/b.NEF"])
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

/// The folder is on disk at exactly one of its names, and the catalog
/// says the one it is at, or lists it unfinished.
fn one_place(fx: &Fx, recovery: &Recovery, case: &str) {
    let (old, new) = (fx.root.join("day"), fx.root.join("New"));
    assert!(old.is_dir() != new.is_dir(), "{case}");
    let renamed = keys(fx)
        .iter()
        .any(|(_, logical, _)| logical.as_deref() == Some("New"));
    if renamed != new.is_dir() {
        assert!(!recovery.unfinished.is_empty(), "{case}: unreported");
    }
}

#[test]
fn every_fault_at_every_step_of_a_folder_rename_ends_proven() {
    let mut clean = tree_files();
    let counting = Faulty::default();
    let op = rename_folder(&clean, "day", "New");
    let _ = clean.run(op, &NoTrash, &counting);
    let steps = [
        (Op::Rename, counting.count(Op::Rename)),
        (Op::Sync, counting.count(Op::Sync)),
    ];
    assert_eq!(steps[0].1, 1);
    let mut runs = 0;
    for (at, count) in steps {
        for nth in 0..count {
            for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                let case = format!("{at:?} {nth} {fault:?}");
                let mut fx = tree_files();
                let was = before(&fx);
                let disk = Faulty::at(at, nth, fault);
                let op = rename_folder(&fx, "day", "New");
                let (batch, recovery) = scenario(&mut fx, op, &disk);
                check(&fx, &was, &recovery, &batch);
                one_place(&fx, &recovery, &case);
                for u in &recovery.unfinished {
                    assert_eq!(u.choices, [Resolution::PutBack, Resolution::Keep]);
                    let back =
                        resolve(&mut fx.cat, u.operation, Resolution::PutBack, &NoTrash).unwrap();
                    assert!(
                        matches!(back.result, ItemResult::Unchanged(_)),
                        "{case}: {back:?}"
                    );
                }
                check(&fx, &was, &Recovery::default(), &batch);
                one_place(&fx, &Recovery::default(), &case);
                runs += 1;
            }
        }
    }
    assert!(runs >= 6);
}

#[test]
fn every_failed_commit_of_a_folder_rename_ends_proven_and_a_restart_finishes_it() -> Result<()> {
    for when in [
        "BEFORE INSERT ON file_operations",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'moved'",
        "BEFORE UPDATE ON folders",
        "BEFORE INSERT ON folder_paths",
        "BEFORE UPDATE ON folder_paths",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'",
    ] {
        let mut fx = tree_files();
        let was = before(&fx);
        let day = fx.folder("day");
        fx.fail_journal(when);
        let plan = fx.plan(rename_folder(&fx, "day", "New"));
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
            w if w.contains("INSERT ON file_operations") => assert!(outcome.items.is_empty()),
            w if w.contains("members") => assert!(
                matches!(first, Some(ItemResult::Unchanged(Some(_)))),
                "{first:?}"
            ),
            _ => assert!(
                matches!(first, Some(ItemResult::NeedsRecovery(_))),
                "{when}: {first:?}"
            ),
        }
        fx.reopen();
        let recovery = fx.recover(&disk::Real);
        assert!(recovery.unfinished.is_empty(), "{when}: {recovery:?}");
        check(&fx, &was, &recovery, &batch);
        if !when.contains("INSERT ON file_operations") && !when.contains("members") {
            assert_eq!(
                results(&super::outcome(&fx.cat, &batch)?),
                [ItemResult::FolderRenamed],
                "{when}"
            );
            assert_eq!(fx.rows()[&fx.photo("New/a.NEF")], "New/a.NEF");
            assert_eq!(fx.folder("New"), day, "{when}: the same folder");
        }
    }
    Ok(())
}

#[test]
fn a_folder_rename_changed_in_the_catalog_before_recovery_is_reported_and_left() -> Result<()> {
    let mut fx = tree_files();
    fx.fail_journal("BEFORE UPDATE ON folders");
    let outcome = fx.run(
        rename_folder(&fx, "day", "New"),
        &NoTrash,
        &Faulty::default(),
    );
    assert!(matches!(
        results(&outcome)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    fx.reopen();
    // Another connection locates a folder of the subtree somewhere else.
    let sub = fx.folder("day/sub");
    let mut other = Catalog::open(&fx.catalog)?;
    other.relink_folder(sub, &fx.root.join("New/sub"))?;
    drop(other);
    let recovery = fx.recover(&disk::Real);
    assert_eq!(recovery.unfinished.len(), 1, "{recovery:?}");
    assert!(fx.root.join("New").is_dir(), "renamed nothing back");
    assert!(
        keys(&fx)
            .iter()
            .all(|(_, l, _)| l.as_deref() != Some("New"))
    );
    Ok(())
}

#[test]
fn a_root_added_inside_since_planning_keeps_the_folder_where_it_is() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/sub/b.NEF"]);
    let plan = fx.plan(rename_folder(&fx, "day", "Other"));
    // Meanwhile another window adds a root over a folder inside it.
    let sub = fx.root.join("day/sub");
    fx.add_row(&sub, "", "b.NEF");
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert!(
        matches!(&results(&outcome)[..], [ItemResult::Unchanged(Some(why))] if why.contains("inside it")),
        "{outcome:?}"
    );
    assert!(fx.root.join("day/sub/b.NEF").is_file());
    assert!(!fx.root.join("Other").exists());
    Ok(())
}

#[test]
fn a_folder_the_catalog_has_no_folder_of_its_own_for_is_renamed_with_those_below() -> Result<()> {
    let mut fx = fixture(&["2024/day1/a.NEF", "2024/day2/b.NEF"]);
    let (a, day1) = (fx.photo("2024/day1/a.NEF"), fx.folder("2024/day1"));
    let outcome = fx.run(
        rename_folder(&fx, "2024", "2024 trip"),
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::FolderRenamed]);
    assert!(fx.root.join("2024 trip/day2/b.NEF").is_file());
    assert_eq!(fx.rows()[&a], "2024 trip/day1/a.NEF");
    assert_eq!(fx.folder("2024 trip/day1"), day1, "the same folder");
    Ok(())
}

#[test]
fn a_folder_inside_a_located_folder_of_folders_is_renamed() -> Result<()> {
    let mut fx = fixture(&["2024/day1/", "2024/day2/"]);
    let root = fx.cat.roots()?[0].0;
    // "2024" holds only folders; located on its own, the folders in it
    // still resolve through it.
    let at = fx.root.join("2024");
    fx.cat
        .relink_folder_at(root, "2024", &at, crate::catalog::Overrides::Keep)?;
    assert!(plan(&fx.cat, rename_folder(&fx, "2024/day1", "day 1")).is_ok());
    let why = refusal(&fx, rename_folder(&fx, "2024", "Year"));
    assert!(why.contains("located on its own"), "{why}");
    Ok(())
}
