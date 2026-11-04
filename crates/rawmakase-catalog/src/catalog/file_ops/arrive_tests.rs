//! Moving photos into a folder of a root that the catalog has no folder of
//! its own for (one holding only folders, or a root with no photos
//! directly in it): its directory is there; the catalog's folder for it is
//! added in the transaction the photos' rows follow in, and only then.
use super::recover_tests::{before, check};
use super::tests::{FakeTrash, Fault, Faulty, Fx, NoTrash, Op, bytes, crashes, fixture, results};
use super::*;
use anyhow::Result;
use rawmakase_cancel::Cancel;

/// Whether the move of `outcome`'s first item added the catalog's folder.
fn added(fx: &Fx, outcome: &Outcome) -> bool {
    let item = super::journal::item(&fx.cat.db, outcome.items[0].operation)
        .unwrap()
        .unwrap();
    let super::journal::Subject::Photos(pre) = item.subject else {
        panic!("not a photo item")
    };
    pre.into.expect("planned without a folder").added
}

/// The catalog's folders of the fixture's root, by logical path.
fn folders(fx: &Fx) -> Vec<String> {
    let mut all: Vec<String> = fx
        .cat
        .folders()
        .unwrap()
        .into_iter()
        .map(|f| f.relative)
        .collect();
    all.sort();
    all
}

fn into(fx: &Fx, photos: &[&str], logical: &str) -> FileOp {
    FileOp::MovePhotos {
        photos: photos.iter().map(|p| fx.photo(p)).collect(),
        root: fx.root_id(),
        to: logical.into(),
    }
}

#[test]
fn photos_move_into_a_folder_holding_only_folders() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp", "top/day1/", "top/day2/"]);
    let a = fx.photo("a.NEF");
    assert!(!folders(&fx).contains(&"top".to_string()));
    let op = into(&fx, &["a.NEF"], "top");
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    let top = fx.folder("top");
    assert_eq!(results(&outcome), [ItemResult::Moved { to: top }]);
    assert_eq!(fx.rows()[&a], "top/a.NEF");
    assert!(fx.root.join("top/a.xmp").is_file());
    assert!(added(&fx, &outcome));
    assert_eq!(
        folders(&fx).iter().filter(|f| *f == "top").count(),
        1,
        "{:?}",
        folders(&fx)
    );
    Ok(())
}

#[test]
fn photos_move_into_a_root_with_no_photos_of_its_own() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF"]);
    let a = fx.photo("day/a.NEF");
    assert!(!folders(&fx).contains(&String::new()), "{:?}", folders(&fx));
    let op = into(&fx, &["day/a.NEF"], "");
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    assert!(matches!(&results(&outcome)[..], [ItemResult::Moved { .. }]));
    assert_eq!(fx.rows()[&a], "a.NEF");
    Ok(())
}

#[test]
fn a_folder_with_no_directory_is_refused_and_none_is_made() -> Result<()> {
    let fx = fixture(&["a.NEF"]);
    let op = into(&fx, &["a.NEF"], "nowhere");
    assert!(matches!(
        plan(&fx.cat, op),
        Err(FileOpError::Destination(_))
    ));
    assert!(!fx.root.join("nowhere").exists());
    Ok(())
}

#[test]
fn a_directory_another_catalog_folder_is_over_is_refused_when_planned() -> Result<()> {
    // Another root is over top/, which this root has no folder for.
    let mut fx = fixture(&["a.NEF", "top/day1/"]);
    let top = fx.root.join("top");
    fx.add_row(&top, "", "x.NEF");
    let op = into(&fx, &["a.NEF"], "top");
    match plan(&fx.cat, op) {
        Err(FileOpError::Destination(why)) => assert!(why.contains("another folder"), "{why}"),
        other => panic!("not refused: {other:?}"),
    }
    assert!(fx.root.join("a.NEF").is_file());
    Ok(())
}

#[test]
fn a_folder_the_catalog_gained_meanwhile_is_the_one_its_photos_join() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "top/day1/"]);
    let a = fx.photo("a.NEF");
    let plan = fx.plan(into(&fx, &["a.NEF"], "top"));
    // Added by Add Folder, or another computer, between plan and run.
    std::fs::write(fx.root.join("top/b.NEF"), bytes("top/b.NEF"))?;
    fx.cat.add_folder(&fx.root)?;
    let top = fx.folder("top");
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    // Its family is as planned, so it moves, into the folder there now.
    assert_eq!(results(&outcome), [ItemResult::Moved { to: top }]);
    assert!(!added(&fx, &outcome), "found, not added");
    assert_eq!(fx.rows()[&a], "top/a.NEF");
    assert_eq!(folders(&fx).iter().filter(|f| *f == "top").count(), 1);
    Ok(())
}

#[test]
fn a_failed_commit_adds_no_folder_and_recovery_adds_it_once() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "top/day1/"]);
    let a = fx.photo("a.NEF");
    fx.fail_journal("BEFORE UPDATE ON photos");
    let op = into(&fx, &["a.NEF"], "top");
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    assert!(matches!(
        &results(&outcome)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    assert!(
        !folders(&fx).contains(&"top".to_string()),
        "rolled back with it"
    );
    fx.reopen();
    let recovery = fx.recover(&disk::Real);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    assert_eq!(fx.rows()[&a], "top/a.NEF");
    assert_eq!(folders(&fx).iter().filter(|f| *f == "top").count(), 1);
    Ok(())
}

#[test]
fn every_fault_moving_into_a_new_catalog_folder_ends_proven() {
    let files = || {
        let fx = fixture(&["a.NEF", "a.xmp", "b.NEF", "top/day1/", "far/"]);
        std::fs::create_dir(fx.root.join("far/top")).unwrap();
        fx
    };
    let mut runs = 0;
    for (to, far) in [("top", false), ("far/top", true)] {
        let mut clean = files();
        let elsewhere = clean.root.join("far");
        let counting = match far {
            true => Faulty::default().elsewhere(&elsewhere),
            false => Faulty::default(),
        };
        let op = into(&clean, &["a.NEF", "b.NEF"], to);
        let plan = clean.plan_on(op, &counting).unwrap();
        let trash = FakeTrash::new(&clean);
        let _ = execute::execute_with(
            &mut clean.cat,
            plan.confirm(),
            &trash,
            Cancel::NEVER,
            &counting,
        )
        .unwrap();
        let ops: &[Op] = match far {
            true => &Op::COPYING,
            false => &[Op::Rename, Op::Sync],
        };
        for op in ops {
            for nth in 0..counting.count(*op) {
                for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                    let mut fx = files();
                    let was = before(&fx);
                    let disk = Faulty::at(*op, nth, fault);
                    let disk = match far {
                        true => disk.elsewhere(&fx.root.join("far")),
                        false => disk,
                    };
                    let trash = FakeTrash::new(&fx);
                    let plan = fx
                        .plan_on(into(&fx, &["a.NEF", "b.NEF"], to), &disk)
                        .unwrap();
                    let batch = plan.batch().clone();
                    crashes(|| {
                        drop(
                            execute::execute_with(
                                &mut fx.cat,
                                plan.confirm(),
                                &trash,
                                Cancel::NEVER,
                                &disk,
                            )
                            .unwrap(),
                        );
                    });
                    fx.reopen();
                    let clean = match far {
                        true => Faulty::default().elsewhere(&fx.root.join("far")),
                        false => Faulty::default(),
                    };
                    let recovery = fx.recover_with(&clean, &trash);
                    check(&fx, &was, &recovery, &batch);
                    // The folder is in the catalog exactly when a photo
                    // moved into it, once.
                    let moved = fx
                        .rows()
                        .values()
                        .any(|r| *r == format!("{to}/a.NEF") || *r == format!("{to}/b.NEF"));
                    let added = folders(&fx).iter().filter(|f| *f == to).count();
                    assert_eq!(added, usize::from(moved), "{op:?} {nth} {fault:?} {to}");
                    runs += 1;
                }
            }
        }
    }
    assert!(runs > 40, "{runs}");
}
