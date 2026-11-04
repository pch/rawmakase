//! New Folder Inside: making a folder, on disk and in the catalog, and
//! moving the chosen photos into it; what is refused; and what a fault at
//! any step leaves, which recovery decides without ever adopting a folder
//! it can't prove is the one it made, or deleting one.
use super::recover_tests::{before, check, check_files};
use super::tests::{Fault, Faulty, Fx, NoTrash, Op, crashes, fixture, results};
use super::*;
use crate::catalog::RootId;
use crate::fsops;
use anyhow::Result;
use rawmakase_cancel::Cancel;

fn root(fx: &Fx) -> RootId {
    fx.cat.roots().unwrap()[0].0
}

fn new_folder(fx: &Fx, parent: &str, name: &str, then_move: Vec<PhotoId>) -> FileOp {
    FileOp::CreateFolder {
        root: root(fx),
        parent: parent.into(),
        name: name.into(),
        then_move,
    }
}

/// The catalog folder at `relative` below the root, if there is one.
fn folder_at(fx: &Fx, relative: &str) -> Option<FolderId> {
    let path = fx.root.join(relative);
    fx.cat
        .folders()
        .unwrap()
        .into_iter()
        .find(|f| f.path == path)
        .map(|f| f.id)
}

#[test]
fn a_folder_is_made_and_the_chosen_photos_move_into_it() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp", "b.NEF", "sub/"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    let copy = fx.cat.create_virtual_copy(a)?;
    let op = new_folder(&fx, "", "Selects", vec![a]);
    let plan = fx.plan(op.clone());
    assert_eq!(plan.summary().folder.as_deref(), Some("Selects"));
    assert_eq!(plan.summary().files, 1);
    assert_eq!(plan.summary().companions, 1);
    assert_eq!(plan.summary().photos, 2);
    assert!(!fx.root.join("Selects").exists(), "planning makes nothing");
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    let made = folder_at(&fx, "Selects").expect("the folder is in the catalog");
    assert_eq!(
        results(&outcome),
        [
            ItemResult::Created { folder: made },
            ItemResult::Moved { to: made }
        ]
    );
    assert!(fx.root.join("Selects/a.NEF").is_file());
    assert!(fx.root.join("Selects/a.xmp").is_file());
    let rows = fx.rows();
    assert_eq!(rows[&a], "Selects/a.NEF");
    assert_eq!(rows[&copy], "Selects/a.NEF");
    assert_eq!(rows[&b], "b.NEF");
    Ok(())
}

#[test]
fn a_folder_is_made_where_the_catalog_has_no_folder_of_its_own() -> Result<()> {
    // Only 2024/day1 is a catalog folder: neither the root nor 2024 is.
    let mut fx = fixture(&["2024/day1/a.NEF"]);
    assert!(folder_at(&fx, "").is_none() && folder_at(&fx, "2024").is_none());
    for (parent, name, at) in [("2024", "day2", "2024/day2"), ("", "Selects", "Selects")] {
        let op = new_folder(&fx, parent, name, Vec::new());
        let outcome = fx.run(op, &NoTrash, &Faulty::default());
        let made = folder_at(&fx, at).expect("the folder is in the catalog");
        assert_eq!(results(&outcome), [ItemResult::Created { folder: made }]);
        assert!(fx.root.join(at).is_dir());
    }
    Ok(())
}

#[test]
fn names_a_folder_cant_have_are_refused() {
    let fx = fixture(&["a.NEF"]);
    let long = "x".repeat(256);
    for name in [
        "", " ", ".", "..", "a/b", "a\\b", "a\0b", "a\nb", "CON", "con.txt", "Lpt1", "x.", "x ",
        &long,
    ] {
        assert!(
            folder_name_problem(name).is_some(),
            "{name:?} should be refused"
        );
        let planned = plan(&fx.cat, new_folder(&fx, "", name, Vec::new()));
        assert!(
            matches!(planned, Err(FileOpError::Destination(_))),
            "{name:?}: {planned:?}"
        );
    }
    for name in ["Selects", "Trip – ąę 2024", "x.y", " leading"] {
        assert_eq!(folder_name_problem(name), None, "{name:?}");
    }
}

#[test]
fn an_existing_folder_file_or_catalog_folder_is_never_adopted() -> Result<()> {
    let mut fx = fixture(&["a.NEF"]);
    std::fs::create_dir(fx.root.join("Taken"))?;
    std::fs::write(fx.root.join("Taken/x.txt"), b"x")?;
    std::fs::write(fx.root.join("file.txt"), b"file")?;
    // A catalog folder missing from disk holds its name too, in any case.
    let r = fx.root.clone();
    fx.add_row(&r, "Gone", "x.NEF");
    for name in ["Taken", "file.txt", "Gone", "GONE"] {
        let planned = plan(&fx.cat, new_folder(&fx, "", name, Vec::new()));
        assert!(
            matches!(planned, Err(FileOpError::Destination(_))),
            "{name}: {planned:?}"
        );
    }
    assert!(folder_at(&fx, "Taken").is_none());
    assert_eq!(std::fs::read(fx.root.join("Taken/x.txt"))?, b"x");
    Ok(())
}

#[test]
fn a_folder_made_meanwhile_is_not_adopted_and_nothing_moves() -> Result<()> {
    let mut fx = fixture(&["a.NEF"]);
    let a = fx.photo("a.NEF");
    let plan = fx.plan(new_folder(&fx, "", "New", vec![a]));
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
    assert!(folder_at(&fx, "New").is_none());
    assert!(fx.root.join("a.NEF").is_file());
    assert_eq!(std::fs::read(fx.root.join("New/theirs.txt"))?, b"theirs");
    Ok(())
}

#[test]
fn a_folder_that_cant_be_made_moves_nothing() -> Result<()> {
    let mut fx = fixture(&["a.NEF"]);
    let a = fx.photo("a.NEF");
    let op = new_folder(&fx, "", "New", vec![a]);
    let outcome = fx.run(op, &NoTrash, &Faulty::at(Op::CreateDir, 0, Fault::Fail));
    assert!(
        matches!(results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
        "{outcome:?}"
    );
    assert!(!fx.root.join("New").exists());
    assert!(fx.root.join("a.NEF").is_file());
    assert_eq!(fx.rows()[&a], "a.NEF");
    Ok(())
}

#[test]
fn a_parent_located_elsewhere_since_planning_is_left() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF"]);
    let plan = fx.plan(new_folder(&fx, "day", "New", Vec::new()));
    // The folder is located somewhere else meanwhile.
    let elsewhere = fx.dir.path().canonicalize()?.join("elsewhere");
    std::fs::create_dir(&elsewhere)?;
    std::fs::copy(fx.root.join("day/a.NEF"), elsewhere.join("a.NEF"))?;
    let day = fx.folder("day");
    fx.cat.relink_folder(day, &elsewhere)?;
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
    assert!(!fx.root.join("day/New").exists() && !elsewhere.join("New").exists());
    Ok(())
}

#[test]
fn a_folder_made_just_before_its_mkdir_is_never_taken_for_it() -> Result<()> {
    // Between the check and the mkdir: the mkdir refuses, nothing moves.
    let mut fx = fixture(&["a.NEF"]);
    let a = fx.photo("a.NEF");
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
        fn rename(&self, a: &std::path::Path, b: &std::path::Path) -> Result<(), fsops::FsError> {
            self.0.rename(a, b)
        }
        fn create_dir(&self, p: &std::path::Path) -> Result<(), fsops::FsError> {
            std::fs::create_dir(p).unwrap();
            std::fs::write(p.join("theirs.txt"), b"theirs").unwrap();
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
    let op = new_folder(&fx, "", "New", vec![a]);
    let outcome = fx.run(op, &NoTrash, &Racing(Faulty::default()));
    assert!(
        matches!(results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
        "{outcome:?}"
    );
    assert!(folder_at(&fx, "New").is_none());
    assert!(fx.root.join("a.NEF").is_file());
    assert_eq!(std::fs::read(fx.root.join("New/theirs.txt"))?, b"theirs");
    Ok(())
}

fn create_files() -> Fx {
    fixture(&["a.NEF", "a.xmp", "b.NEF"])
}

fn create_op(fx: &Fx) -> FileOp {
    new_folder(fx, "", "New", vec![fx.photo("a.NEF"), fx.photo("b.NEF")])
}

#[test]
fn every_fault_at_every_step_of_making_a_folder_ends_proven() {
    let mut clean = create_files();
    let counting = Faulty::default();
    let op = create_op(&clean);
    let _ = clean.run(op, &NoTrash, &counting);
    let steps = [
        (Op::CreateDir, counting.count(Op::CreateDir)),
        (Op::Rename, counting.count(Op::Rename)),
        (Op::Sync, counting.count(Op::Sync)),
    ];
    assert_eq!(steps[0].1, 1);
    assert_eq!(steps[1].1, 3);
    let mut runs = 0;
    for (at, count) in steps {
        for nth in 0..count {
            for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                let mut fx = create_files();
                let was = before(&fx);
                let disk = Faulty::at(at, nth, fault);
                let op = create_op(&fx);
                let plan = fx.plan(op);
                let batch = plan.batch().clone();
                crashes(|| {
                    drop(
                        execute::execute_with(
                            &mut fx.cat,
                            plan.confirm(),
                            &NoTrash,
                            Cancel::NEVER,
                            &disk,
                        )
                        .unwrap(),
                    );
                });
                fx.reopen();
                let recovery = fx.recover(&disk::Real);
                check(&fx, &was, &recovery, &batch);
                let case = format!("{at:?} {nth} {fault:?}");
                // The catalog has the folder only where it is on disk.
                let made = folder_at(&fx, "New");
                assert!(made.is_none() || fx.root.join("New").is_dir(), "{case}");
                // A photo's rows are in the new folder only with it there.
                for p in fx.rows().values() {
                    assert!(!p.starts_with("New/") || made.is_some(), "{case}: {p}");
                }
                // Whatever is unfinished is decided by what it offers.
                for u in &recovery.unfinished {
                    let choice = if u.choices.contains(&Resolution::AddFolder) {
                        Resolution::AddFolder
                    } else {
                        u.choices[0]
                    };
                    resolve(&mut fx.cat, u.operation, choice, &NoTrash).unwrap();
                }
                check_files(&fx, &was, &Recovery::default(), &batch);
                let made = folder_at(&fx, "New");
                assert!(made.is_none() || fx.root.join("New").is_dir(), "{case}");
                runs += 1;
            }
        }
    }
    assert!(runs >= 21);
}

#[test]
fn a_folder_made_before_its_id_was_journalled_is_never_adopted() -> Result<()> {
    let mut fx = create_files();
    let op = create_op(&fx);
    let plan = fx.plan(op);
    let disk = Faulty::at(Op::CreateDir, 0, Fault::CrashAfter);
    assert!(crashes(|| {
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &NoTrash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    let recovery = fx.recover(&disk::Real);
    assert!(fx.root.join("New").is_dir());
    assert!(folder_at(&fx, "New").is_none());
    let [unfinished] = &recovery.unfinished[..] else {
        panic!("{recovery:?}");
    };
    assert_eq!(unfinished.choices, [Resolution::Keep]);
    assert!(unfinished.photos.is_empty());
    // A folder is never deleted to put it back.
    assert!(matches!(
        resolve(
            &mut fx.cat,
            unfinished.operation,
            Resolution::PutBack,
            &NoTrash
        ),
        Err(FileOpError::Unsupported(_))
    ));
    assert!(matches!(
        resolve(
            &mut fx.cat,
            unfinished.operation,
            Resolution::AddFolder,
            &NoTrash
        ),
        Err(FileOpError::Unsupported(_))
    ));
    let kept = resolve(
        &mut fx.cat,
        unfinished.operation,
        Resolution::Keep,
        &NoTrash,
    )?;
    assert_eq!(kept.result, ItemResult::Kept);
    assert!(fx.root.join("New").is_dir());
    assert!(fx.recover(&disk::Real).unfinished.is_empty());
    Ok(())
}

/// Makes "New" on disk with the catalog's row failing: unfinished, with
/// the folder's id journalled. Nothing moved.
fn made_but_not_added() -> (Fx, BatchId) {
    let mut fx = create_files();
    fx.fail_journal("BEFORE INSERT ON folders");
    let op = create_op(&fx);
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
    assert!(
        matches!(results(&outcome)[..], [ItemResult::NeedsRecovery(_)]),
        "{outcome:?}"
    );
    assert!(fx.root.join("a.NEF").is_file(), "the moves wait for it");
    fx.reopen();
    (fx, batch)
}

#[test]
fn a_made_folder_not_yet_in_the_catalog_is_added_when_asked() -> Result<()> {
    let (mut fx, _) = made_but_not_added();
    let recovery = fx.recover(&disk::Real);
    let [unfinished] = &recovery.unfinished[..] else {
        panic!("{recovery:?}");
    };
    assert_eq!(
        unfinished.choices,
        [Resolution::AddFolder, Resolution::Keep]
    );
    assert!(folder_at(&fx, "New").is_none(), "only when asked");
    let added = resolve(
        &mut fx.cat,
        unfinished.operation,
        Resolution::AddFolder,
        &NoTrash,
    )?;
    let made = folder_at(&fx, "New").expect("added");
    assert_eq!(added.result, ItemResult::Created { folder: made });
    assert!(fx.recover(&disk::Real).unfinished.is_empty());
    Ok(())
}

#[test]
fn a_made_folder_gone_or_replaced_before_recovery_is_never_added() -> Result<()> {
    // Gone: nothing is left to do.
    let (mut fx, _) = made_but_not_added();
    std::fs::remove_dir(fx.root.join("New"))?;
    let recovery = fx.recover(&disk::Real);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    assert!(matches!(
        recovery.finished[0].result,
        ItemResult::Unchanged(_)
    ));
    // Another folder in its place is not the one made. (The one made is
    // kept elsewhere, so the new one can't be given its number.)
    let (mut fx, _) = made_but_not_added();
    std::fs::rename(fx.root.join("New"), fx.root.join("Old"))?;
    std::fs::create_dir(fx.root.join("New"))?;
    let recovery = fx.recover(&disk::Real);
    let [unfinished] = &recovery.unfinished[..] else {
        panic!("{recovery:?}");
    };
    let added = resolve(
        &mut fx.cat,
        unfinished.operation,
        Resolution::AddFolder,
        &NoTrash,
    )?;
    assert!(
        matches!(added.result, ItemResult::NeedsRecovery(_)),
        "{added:?}"
    );
    assert!(folder_at(&fx, "New").is_none());
    Ok(())
}

#[test]
fn a_photo_named_so_there_since_planning_keeps_the_folder_from_being_made() -> Result<()> {
    let mut fx = fixture(&["a.NEF"]);
    let a = fx.photo("a.NEF");
    let plan = fx.plan(new_folder(&fx, "", "New.NEF", Vec::new()));
    // Meanwhile another window's catalog has a photo of that name there,
    // its file offline.
    fx.cat
        .db_for_tests()
        .execute("UPDATE photos SET filename='new.nef' WHERE id=?1", [a.0])?;
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
    assert!(!fx.root.join("New.NEF").exists());
    assert!(folder_at(&fx, "New.NEF").is_none());
    // Planned now, it is refused at once.
    let again = super::plan(&fx.cat, new_folder(&fx, "", "New.NEF", Vec::new()));
    assert!(
        matches!(&again, Err(FileOpError::Destination(why)) if why.contains("A photo of the catalog is named New.NEF")),
        "{again:?}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_photo_named_so_there_through_a_link_to_its_folder_keeps_the_folder_from_being_made()
-> Result<()> {
    let mut fx = fixture(&["New.NEF", "a.NEF"]);
    // New.NEF's file goes elsewhere; its photo stays, offline.
    let elsewhere = tempfile::tempdir()?;
    std::fs::rename(fx.root.join("New.NEF"), elsewhere.path().join("New.NEF"))?;
    // The root is reached through a link now.
    let alias = elsewhere.path().join("alias");
    std::os::unix::fs::symlink(&fx.root, &alias)?;
    let r = root(&fx);
    fx.cat.relink_root(r, &alias)?;
    let planned = super::plan(&fx.cat, new_folder(&fx, "", "New.NEF", Vec::new()));
    assert!(
        matches!(&planned, Err(FileOpError::Destination(why)) if why.contains("A photo of the catalog is named New.NEF")),
        "{planned:?}"
    );
    assert!(!fx.root.join("New.NEF").exists());
    Ok(())
}
