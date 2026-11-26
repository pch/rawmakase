//! Delete Permanently (#1273): photos on a drive with no Trash, deleted
//! outright once the user confirms, and the Trash's plan leaving them out
//! rather than failing on them when it runs.
use super::recover_tests::{Before, before};
use super::tests::{FakeTrash, Fault, Faulty, Fx, NoTrash, Op, crashes, fixture, results};
use super::*;
use anyhow::Result;
use rawmakase_cancel::Cancel;

/// A Trash that says it can't take anything, as Windows' says of a USB
/// stick or a network drive.
fn no_trash(fx: &Fx) -> FakeTrash {
    let mut trash = FakeTrash::new(fx);
    trash.support = TrashSupport::No;
    trash
}

fn delete(fx: &Fx, names: &[&str]) -> FileOp {
    FileOp::DeletePhotos {
        photos: names.iter().map(|n| fx.photo(n)).collect(),
    }
}

fn plan_on(fx: &Fx, op: FileOp, disk: &dyn disk::Disk, trash: &dyn Trash) -> Plan {
    plan::plan_trash_with(&fx.cat, op, disk, trash).unwrap()
}

fn reasons(plan: &Plan) -> Vec<Skip> {
    plan.skipped().iter().map(|s| s.reason.clone()).collect()
}

/// What the Trash's question counted is what it can take: photos on a
/// drive without one are left out when planned, not found when it runs
/// (#1273, where it counted 4 and moved none).
#[test]
fn the_trash_plan_leaves_out_photos_on_a_drive_without_a_trash() {
    let fx = fixture(&["a.NEF", "b.NEF"]);
    let trash = no_trash(&fx);
    let op = FileOp::TrashPhotos {
        photos: vec![fx.photo("a.NEF"), fx.photo("b.NEF")],
    };
    let plan = plan_on(&fx, op, &disk::Real, &trash);
    assert!(plan.is_empty());
    assert_eq!(
        reasons(&plan),
        [
            Skip::Refused(Refusal::NoTrash),
            Skip::Refused(Refusal::NoTrash)
        ]
    );
}

/// An SMB share has no Trash RAWmakase uses, whatever the system's Trash
/// says it can't tell (macOS's never says no).
#[test]
fn an_smb_share_has_no_trash() {
    let fx = fixture(&["a.NEF"]);
    let share = Faulty {
        names_only: vec![fx.root.clone()],
        share: true,
        ..Faulty::default()
    };
    let mut trash = FakeTrash::new(&fx);
    trash.support = TrashSupport::Unknown;
    let trashing = plan_on(
        &fx,
        FileOp::TrashPhotos {
            photos: vec![fx.photo("a.NEF")],
        },
        &share,
        &trash,
    );
    assert_eq!(reasons(&trashing), [Skip::Refused(Refusal::NoTrash)]);
    let deleting = plan_on(&fx, delete(&fx, &["a.NEF"]), &share, &trash);
    assert_eq!(deleting.summary().files, 1);
    assert!(deleting.skipped().is_empty());
}

#[test]
fn photos_on_a_drive_with_a_trash_are_never_deleted_permanently() {
    let fx = fixture(&["a.NEF"]);
    for support in [TrashSupport::Yes, TrashSupport::Unknown] {
        let mut trash = FakeTrash::new(&fx);
        trash.support = support;
        let plan = plan_on(&fx, delete(&fx, &["a.NEF"]), &disk::Real, &trash);
        assert!(plan.is_empty(), "{support:?}");
        assert_eq!(reasons(&plan), [Skip::Refused(Refusal::HasTrash)]);
    }
}

#[test]
fn a_deletion_is_planned_only_knowing_the_trash() {
    let fx = fixture(&["a.NEF"]);
    let refused = plan(&fx.cat, delete(&fx, &["a.NEF"]));
    assert!(matches!(refused, Err(FileOpError::Unsupported(_))));
}

/// Runs a confirmed deletion of `names` on `disk`.
fn run(fx: &mut Fx, names: &[&str], disk: &Faulty) -> Outcome {
    let trash = no_trash(fx);
    let plan = plan_on(fx, delete(fx, names), disk, &trash);
    execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, disk).unwrap()
}

#[test]
fn deleting_permanently_removes_the_files_and_then_the_photos() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp", "b.NEF"]);
    let a = fx.photo("a.NEF");
    fx.cat.create_virtual_copy(a)?;
    let trash = no_trash(&fx);
    let plan = plan_on(&fx, delete(&fx, &["a.NEF"]), &disk::Real, &trash);
    assert_eq!((plan.summary().files, plan.summary().companions), (1, 1));
    assert_eq!(plan.summary().copies, 1);
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &trash,
        Cancel::NEVER,
        &disk::Real,
    )?;
    assert_eq!(results(&outcome), [ItemResult::Deleted]);
    assert_eq!(fx.files().keys().collect::<Vec<_>>(), ["trip/b.NEF"]);
    assert_eq!(fx.rows().into_values().collect::<Vec<_>>(), ["b.NEF"]);
    // Nothing went to the Trash.
    assert_eq!(*trash.calls.borrow(), 0);
    Ok(())
}

/// A file changed since the question was asked isn't what the user agreed
/// to lose: nothing is deleted.
#[test]
fn a_file_changed_since_it_was_planned_is_not_deleted() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let trash = no_trash(&fx);
    let plan = plan_on(&fx, delete(&fx, &["a.NEF"]), &disk::Real, &trash);
    std::fs::write(fx.root.join("a.NEF"), b"another file")?;
    let was = before(&fx);
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &trash,
        Cancel::NEVER,
        &disk::Real,
    )?;
    assert!(matches!(results(&outcome)[..], [ItemResult::Unchanged(_)]));
    unchanged(&fx, &was);
    Ok(())
}

fn unchanged(fx: &Fx, was: &Before) {
    assert_eq!(fx.files(), was.files);
    assert_eq!(fx.rows(), was.rows);
}

#[test]
fn a_photo_whose_file_cant_be_deleted_stays_as_it_was() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let was = before(&fx);
    let outcome = run(
        &mut fx,
        &["a.NEF"],
        &Faulty::at(Op::RemoveFile, 0, Fault::Fail),
    );
    assert!(matches!(results(&outcome)[..], [ItemResult::Unchanged(_)]));
    unchanged(&fx, &was);
    fx.reopen();
    assert!(fx.recover(&disk::Real).unfinished.is_empty());
    Ok(())
}

/// Deleted can't be put back: a sidecar that can't be deleted after its
/// photo was leaves the item for the user, who may take the photo out of
/// the catalog; the sidecar stays.
#[test]
fn a_deletion_cut_short_is_never_put_back_and_can_be_removed_from_the_catalog() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp", "b.NEF"]);
    let a = fx.photo("a.NEF");
    let outcome = run(
        &mut fx,
        &["a.NEF"],
        &Faulty::at(Op::RemoveFile, 1, Fault::Fail),
    );
    let [ItemResult::NeedsRecovery(why)] = &results(&outcome)[..] else {
        panic!("{outcome:?}");
    };
    assert!(why.contains("a.NEF was deleted, but"), "{why}");
    assert!(fx.rows().contains_key(&a));
    fx.reopen();
    let recovery = fx.recover(&disk::Real);
    let [unfinished] = &recovery.unfinished[..] else {
        panic!("{recovery:?}");
    };
    assert_eq!(
        unfinished.choices,
        [Resolution::RemoveFromCatalog, Resolution::Keep]
    );
    let put_back = resolve(
        &mut fx.cat,
        unfinished.operation,
        Resolution::PutBack,
        &NoTrash,
    );
    assert!(matches!(put_back, Err(FileOpError::Unsupported(_))));
    let removed = resolve(
        &mut fx.cat,
        unfinished.operation,
        Resolution::RemoveFromCatalog,
        &NoTrash,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(removed.result, ItemResult::Removed);
    assert!(!fx.rows().contains_key(&a));
    assert_eq!(
        fx.files().keys().collect::<Vec<_>>(),
        ["trip/a.xmp", "trip/b.NEF"]
    );
    Ok(())
}

/// Removing the photos of a deletion cut short needs the photo's own file
/// gone: one still there is kept as it is.
#[test]
fn a_deletion_whose_photo_is_still_there_is_not_removed_from_the_catalog() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let a = fx.photo("a.NEF");
    // Crashed before anything was deleted, and another file then given
    // the photo's name: not the photo planned, so not at its source.
    let disk = Faulty::at(Op::RemoveFile, 0, Fault::CrashBefore);
    let trash = no_trash(&fx);
    let plan = plan_on(&fx, delete(&fx, &["a.NEF"]), &disk, &trash);
    assert!(crashes(|| {
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
    }));
    // The original kept aside, so the new file can't be given its number
    // again (Linux reuses a removed file's inode at once).
    std::fs::rename(fx.root.join("a.NEF"), fx.dir.path().join("aside.NEF"))?;
    std::fs::write(fx.root.join("a.NEF"), b"another file")?;
    fx.reopen();
    let recovery = fx.recover(&disk::Real);
    let [unfinished] = &recovery.unfinished[..] else {
        panic!("{recovery:?}");
    };
    let refused = resolve(
        &mut fx.cat,
        unfinished.operation,
        Resolution::RemoveFromCatalog,
        &NoTrash,
    );
    assert!(matches!(refused, Err(FileOpError::Destination(_))));
    assert!(fx.rows().contains_key(&a));
    Ok(())
}

/// A crash at any step leaves every file either deleted as confirmed or
/// where it was, and the photos' rows only once all of their files went.
#[test]
fn every_crash_at_every_step_of_a_deletion_ends_proven() {
    let names = ["a.NEF", "a.xmp", "b.NEF"];
    for op in [Op::RemoveFile, Op::Sync] {
        for nth in 0..3 {
            for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                let mut fx = fixture(&names);
                let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
                let disk = Faulty::at(op, nth, fault);
                let trash = no_trash(&fx);
                let plan = plan_on(&fx, delete(&fx, &["a.NEF", "b.NEF"]), &disk, &trash);
                crashes(|| {
                    let _ = execute::execute_with(
                        &mut fx.cat,
                        plan.confirm(),
                        &trash,
                        Cancel::NEVER,
                        &disk,
                    );
                });
                fx.reopen();
                let recovery = fx.recover(&disk::Real);
                let files = fx.files();
                let rows = fx.rows();
                let case = format!("{op:?} {nth} {fault:?}");
                // A photo's rows go only with every file of it.
                if !rows.contains_key(&a) {
                    assert!(!files.contains_key("trip/a.NEF"), "{case}");
                    assert!(!files.contains_key("trip/a.xmp"), "{case}");
                }
                if !rows.contains_key(&b) {
                    assert!(!files.contains_key("trip/b.NEF"), "{case}");
                }
                // A file left is the one it was.
                for (name, bytes) in &files {
                    let original = name.strip_prefix("trip/").unwrap();
                    assert_eq!(bytes, &super::tests::bytes(original), "{case}");
                }
                // A file gone while its photo stays is left for the user,
                // never taken for nothing changed.
                let a_lost = !files.contains_key("trip/a.NEF") || !files.contains_key("trip/a.xmp");
                if rows.contains_key(&a) && a_lost {
                    assert!(
                        recovery.unfinished.iter().any(|u| u.photos.contains(&a)),
                        "{case}"
                    );
                }
                if rows.contains_key(&b) && !files.contains_key("trip/b.NEF") {
                    assert!(
                        recovery.unfinished.iter().any(|u| u.photos.contains(&b)),
                        "{case}"
                    );
                }
                // Nothing is ever put back, so nothing is being put back.
                for u in &recovery.unfinished {
                    assert!(!u.choices.contains(&Resolution::PutBack), "{case}");
                }
            }
        }
    }
}

/// Once a file is removed, a step failing after it (syncing its folder,
/// journalling it) leaves the item for the user: the file is gone.
#[test]
fn a_file_removed_before_its_step_failed_is_never_taken_for_nothing_changed() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let a = fx.photo("a.NEF");
    let outcome = run(&mut fx, &["a.NEF"], &Faulty::at(Op::Sync, 0, Fault::Fail));
    let [ItemResult::NeedsRecovery(why)] = &results(&outcome)[..] else {
        panic!("{outcome:?}");
    };
    assert!(why.contains("a.NEF may have been deleted"), "{why}");
    assert!(!fx.files().contains_key("trip/a.NEF"));
    fx.reopen();
    let recovery = fx.recover(&disk::Real);
    assert!(recovery.unfinished.iter().any(|u| u.photos.contains(&a)));
    Ok(())
}

/// A Trash asked about a folder at the top of its drive answers for the
/// drive above (Linux's freedesktop Trash): it is asked about the photo's
/// file instead, so a drive with a Trash keeps its photos out of a
/// permanent deletion.
#[test]
fn the_trash_is_asked_about_the_photos_file_not_its_folder() {
    struct FilesOnly;
    impl Trash for FilesOnly {
        fn trash(&self, _: &std::path::Path) -> Result<TrashReceipt, TrashError> {
            panic!("nothing should be trashed")
        }
        fn supports(&self, path: &std::path::Path) -> TrashSupport {
            match path.is_dir() {
                true => TrashSupport::No,
                false => TrashSupport::Yes,
            }
        }
    }
    let fx = fixture(&["a.NEF"]);
    let op = FileOp::TrashPhotos {
        photos: vec![fx.photo("a.NEF")],
    };
    assert!(
        plan_on(&fx, op, &disk::Real, &FilesOnly)
            .skipped()
            .is_empty()
    );
    let deleting = plan_on(&fx, delete(&fx, &["a.NEF"]), &disk::Real, &FilesOnly);
    assert!(deleting.is_empty());
}

/// After a crash, a file at a photo's name with the photo's file object
/// but other contents (written in place, or given a deleted file's inode
/// again, as Linux does) isn't taken for the photo untouched: the deletion
/// stays for the user.
#[test]
fn a_file_at_the_name_with_other_contents_is_not_taken_for_nothing_deleted() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let a = fx.photo("a.NEF");
    let disk = Faulty::at(Op::RemoveFile, 0, Fault::CrashBefore);
    let trash = no_trash(&fx);
    let plan = plan_on(&fx, delete(&fx, &["a.NEF"]), &disk, &trash);
    assert!(crashes(|| {
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
    }));
    // The same file object, other bytes.
    std::fs::write(fx.root.join("a.NEF"), b"another file")?;
    fx.reopen();
    let recovery = fx.recover(&disk::Real);
    assert!(
        recovery.unfinished.iter().any(|u| u.photos.contains(&a)),
        "{recovery:?}"
    );
    assert!(fx.rows().contains_key(&a));
    Ok(())
}
