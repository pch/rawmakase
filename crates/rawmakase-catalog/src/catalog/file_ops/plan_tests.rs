//! Planning and executing without faults: what each operation does on disk
//! and in the catalog, which photos a choice stands for, and what is
//! refused.
use super::tests::{FakeTrash, Faulty, NoTrash, bytes, fixture, results};
use super::*;
use crate::catalog::{Catalog, HistoryUpdate};
use crate::export_settings::ExportOptions;
use crate::fsops;
use crate::model::recipe::Recipe;
use crate::storage::Identity;
use anyhow::Result;
use rawmakase_cancel::Cancel;

fn moved(to: FolderId) -> ItemResult {
    ItemResult::Moved { to }
}

#[test]
fn remove_from_catalog_takes_a_master_with_its_copies_and_leaves_its_file() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.NEF.xmp", "b.NEF"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    let copy = fx.cat.create_virtual_copy(a)?;
    let files = fx.files();
    let plan = fx.plan(FileOp::RemoveFromCatalog { photos: vec![a] });
    assert_eq!(plan.summary().photos, 2);
    assert_eq!(plan.summary().copies, 1);
    assert_eq!(plan.summary().files, 0);
    let outcome = fx.run(
        FileOp::RemoveFromCatalog { photos: vec![a] },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::Removed]);
    assert_eq!(outcome.items[0].photos, [a, copy]);
    assert_eq!(fx.rows().into_keys().collect::<Vec<_>>(), [b]);
    assert_eq!(fx.files(), files, "no file changes");
    Ok(())
}

#[test]
fn a_copy_alone_is_removed_alone_but_never_moved_or_trashed() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    let copy = fx.cat.create_virtual_copy(a)?;
    let to = fx.folder("sub");
    for op in [
        FileOp::MovePhotos {
            photos: vec![copy],
            root: fx.root_of(to),
            to: fx.logical(to),
        },
        FileOp::TrashPhotos { photos: vec![copy] },
    ] {
        let plan = fx.plan(op);
        assert!(plan.is_empty());
        assert_eq!(
            plan.skipped(),
            [Skipped {
                photo: copy,
                reason: Skip::CopyOnly
            }]
        );
    }
    let outcome = fx.run(
        FileOp::RemoveFromCatalog { photos: vec![copy] },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(outcome.items[0].photos, [copy]);
    assert!(fx.rows().contains_key(&a) && !fx.rows().contains_key(&copy));
    Ok(())
}

#[test]
fn a_master_chosen_with_its_copy_is_one_item_and_unknown_photos_are_skipped() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    let copy = fx.cat.create_virtual_copy(a)?;
    let gone = PhotoId(9999);
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![copy, a, gone, a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    assert_eq!(plan.summary().photos, 2);
    assert_eq!(plan.summary().files, 1);
    assert_eq!(
        plan.skipped(),
        [Skipped {
            photo: gone,
            reason: Skip::Unknown
        }]
    );
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos: vec![copy, a],
            root: fx.root_of(to),
            to: fx.logical(to),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [moved(to)]);
    assert_eq!(fx.rows()[&copy], "sub/a.NEF");
    assert_eq!(fx.rows()[&a], "sub/a.NEF");
    Ok(())
}

#[test]
fn a_missing_file_is_removed_from_the_catalog_but_skipped_for_disk_operations() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    std::fs::remove_file(fx.root.join("a.NEF"))?;
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    assert_eq!(plan.skipped()[0].reason, Skip::Missing);
    let plan = fx.plan(FileOp::TrashPhotos { photos: vec![a] });
    assert_eq!(plan.skipped()[0].reason, Skip::Missing);
    let outcome = fx.run(
        FileOp::RemoveFromCatalog { photos: vec![a] },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::Removed]);
    Ok(())
}

#[test]
fn a_move_keeps_the_file_object_its_time_identity_edit_and_checksum() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.NEF.xmp", "sub/"]);
    let a = fx.photo("a.NEF");
    let copy = fx.cat.create_virtual_copy(a)?;
    let source = fx.root.join("a.NEF");
    fx.cat.save_edit(
        a,
        &source,
        &crate::storage::Identity::read(&source)?,
        &Recipe::default(),
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    let read = crate::storage::FileChecksum::read_full(&source, |_| true)?;
    let crate::storage::ChecksumRead::Done(checksum) = read else {
        panic!("not read")
    };
    fx.cat.save_checksums(&[crate::catalog::ReadChecksum {
        photo: a,
        folder: fx.folder(""),
        filename: "a.NEF".into(),
        path: source.clone(),
        checksum,
    }])?;
    let (id, identity) = (fsops::entry(&source)?.id, Identity::read(&source)?);
    let modified = std::fs::metadata(&source)?.modified()?;
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    assert_eq!(plan.summary().companions, 1);
    let disk = Faulty::default();
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_of(to),
            to: fx.logical(to),
        },
        &NoTrash,
        &disk,
    );
    assert_eq!(results(&outcome), [moved(to)]);
    let target = fx.root.join("sub/a.NEF");
    assert_eq!(fsops::entry(&target)?.id, id);
    assert_eq!(Identity::read(&target)?, identity);
    assert_eq!(std::fs::metadata(&target)?.modified()?, modified);
    assert!(fx.root.join("sub/a.NEF.xmp").is_file());
    assert!(!source.exists() && !fx.root.join("a.NEF.xmp").exists());
    assert_eq!(fx.rows()[&a], "sub/a.NEF");
    assert_eq!(fx.rows()[&copy], "sub/a.NEF");
    assert!(
        fx.cat.load_edit(a, &target)?.is_some(),
        "the edit still opens"
    );
    assert_eq!(fx.cat.checksums()?[&a].identity, identity);
    // The photo, then its companion; each rename's folders synced before
    // the next step.
    let renames = disk.renames();
    assert_eq!(renames.len(), 2);
    assert!(renames[0].0.ends_with("a.NEF") && renames[1].0.ends_with("a.NEF.xmp"));
    Ok(())
}

#[test]
fn a_trash_moves_files_to_the_trash_and_removes_the_rows() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp", "b.NEF"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    let copy = fx.cat.create_virtual_copy(a)?;
    let trash = FakeTrash::new(&fx);
    let outcome = fx.run(
        FileOp::TrashPhotos { photos: vec![a] },
        &trash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::Trashed]);
    assert_eq!(outcome.items[0].photos, [a, copy]);
    assert_eq!(fx.rows().into_keys().collect::<Vec<_>>(), [b]);
    let files = fx.files();
    assert_eq!(files.get("trash/1-a.NEF"), Some(&bytes("a.NEF")));
    assert_eq!(files.get("trash/2-a.xmp"), Some(&bytes("a.xmp")));
    assert!(!files.contains_key("trip/a.NEF"));
    Ok(())
}

#[test]
fn a_volume_without_a_trash_changes_nothing() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let a = fx.photo("a.NEF");
    let files = fx.files();
    let mut trash = FakeTrash::new(&fx);
    trash.fail_from = Some(1);
    let outcome = fx.run(
        FileOp::TrashPhotos { photos: vec![a] },
        &trash,
        &Faulty::default(),
    );
    assert!(matches!(
        &results(&outcome)[..],
        [ItemResult::Unchanged(Some(_))]
    ));
    assert_eq!(fx.files(), files);
    assert!(fx.rows().contains_key(&a));
    Ok(())
}

#[test]
fn a_companion_the_trash_refuses_puts_the_photo_back() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let a = fx.photo("a.NEF");
    let files = fx.files();
    let mut trash = FakeTrash::new(&fx);
    trash.fail_from = Some(2);
    let outcome = fx.run(
        FileOp::TrashPhotos { photos: vec![a] },
        &trash,
        &Faulty::default(),
    );
    assert!(matches!(
        &results(&outcome)[..],
        [ItemResult::Unchanged(Some(_))]
    ));
    assert_eq!(fx.files(), files, "the photo is back from the Trash");
    assert!(fx.rows().contains_key(&a));
    Ok(())
}

#[test]
fn background_reads_between_plan_and_execute_strand_nothing() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    // What the checksum reader and the capture and info backfills write.
    fx.cat
        .fill_capture_times(&[(a, "2024-05-01T12:00:00".into())])?;
    fx.cat.fill_photo_info(&[(a, None)])?;
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert_eq!(results(&outcome), [moved(to)]);
    Ok(())
}

#[test]
fn a_family_changed_by_another_connection_after_planning_is_left_alone() -> Result<()> {
    for remove in [false, true] {
        let mut fx = fixture(&["a.NEF", "b.NEF", "sub/"]);
        let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
        let to = fx.folder("sub");
        let op = match remove {
            false => FileOp::MovePhotos {
                photos: vec![a, b],
                root: fx.root_of(to),
                to: fx.logical(to),
            },
            true => FileOp::RemoveFromCatalog { photos: vec![a, b] },
        };
        let plan = fx.plan(op);
        let mut other = Catalog::open(&fx.catalog)?;
        other.set_metadata(a, 4, 0, "")?;
        let outcome = execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &NoTrash,
            Cancel::NEVER,
            &Faulty::default(),
        )?;
        let done = match remove {
            false => moved(to),
            true => ItemResult::Removed,
        };
        // Items run in photo id order, which the folder's listing decides.
        let of = |id| {
            outcome
                .items
                .iter()
                .find(|i| i.photos == [id])
                .map(|i| i.result.clone())
        };
        assert_eq!(
            of(a),
            Some(ItemResult::Unchanged(Some(preconditions::CHANGED.into())))
        );
        assert_eq!(of(b), Some(done));
        assert!(fx.rows().contains_key(&a));
        assert!(fx.root.join("a.NEF").is_file());
    }
    Ok(())
}

#[test]
fn a_photo_removed_and_added_again_with_new_edits_is_refused() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    // Another connection removes it and adds the same file again, under
    // the same id (SQLite reuses the largest), with an edit of its own.
    let other = Catalog::open(&fx.catalog)?;
    let row: (i64, String) = other.db_for_tests().query_row(
        "SELECT folder, original_path FROM photos WHERE id=?1",
        [a.0],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    other.db_for_tests().execute_batch(&format!(
        "DELETE FROM photo_imports WHERE photo={0}; DELETE FROM photos WHERE id={0};
         INSERT INTO photos(id, folder, filename, original_path, rating) VALUES ({0}, {1}, 'a.NEF', '{2}', 5);",
        a.0, row.0, row.1
    ))?;
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert!(matches!(
        &results(&outcome)[..],
        [ItemResult::Unchanged(Some(_))]
    ));
    assert!(fx.root.join("a.NEF").is_file());
    Ok(())
}

#[test]
fn a_destination_created_between_plan_and_execute_is_left_alone() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    std::fs::write(fx.root.join("sub/a.NEF"), b"someone else's")?;
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert!(matches!(
        &results(&outcome)[..],
        [ItemResult::Unchanged(Some(_))]
    ));
    assert_eq!(std::fs::read(fx.root.join("sub/a.NEF"))?, b"someone else's");
    assert_eq!(std::fs::read(fx.root.join("a.NEF"))?, bytes("a.NEF"));
    Ok(())
}

#[test]
fn a_destination_created_just_before_the_rename_is_never_replaced() -> Result<()> {
    // Between the check and the rename: the exclusive rename refuses, and
    // nothing has moved.
    let mut fx = fixture(&["a.NEF", "a.xmp", "sub/"]);
    let a = fx.photo("a.NEF");
    let to = fx.folder("sub");
    struct Racing<'a>(Faulty, &'a std::path::Path);
    impl disk::Disk for Racing<'_> {
        fn entry(&self, p: &std::path::Path) -> Result<fsops::Entry, fsops::FsError> {
            self.0.entry(p)
        }
        fn list(&self, d: &std::path::Path) -> Result<Vec<std::ffi::OsString>, fsops::FsError> {
            self.0.list(d)
        }
        fn identity(&self, p: &std::path::Path) -> anyhow::Result<Identity> {
            self.0.identity(p)
        }
        fn rename(&self, a: &std::path::Path, b: &std::path::Path) -> Result<(), fsops::FsError> {
            if b.ends_with("a.xmp") {
                std::fs::write(self.1, b"raced").unwrap();
            }
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
    let raced = fx.root.join("sub/a.xmp");
    let disk = Racing(Faulty::default(), &raced);
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_of(to),
            to: fx.logical(to),
        },
        &NoTrash,
        &disk,
    );
    assert!(matches!(
        &results(&outcome)[..],
        [ItemResult::Unchanged(Some(_))]
    ));
    assert_eq!(std::fs::read(&raced)?, b"raced");
    assert_eq!(
        std::fs::read(fx.root.join("a.NEF"))?,
        bytes("a.NEF"),
        "put back"
    );
    assert_eq!(std::fs::read(fx.root.join("a.xmp"))?, bytes("a.xmp"));
    assert_eq!(fx.rows()[&a], "a.NEF");
    Ok(())
}

#[test]
fn a_name_taken_in_the_destination_is_refused() -> Result<()> {
    let fx = fixture(&["a.NEF", "b.NEF", "c.NEF", "sub/A.NEF", "sub/x/"]);
    let (a, b, c) = (fx.photo("a.NEF"), fx.photo("b.NEF"), fx.photo("c.NEF"));
    // b's name is the catalog's in the destination, though its file is
    // gone (offline photos keep their names).
    std::fs::write(fx.root.join("sub/x/B.nef"), b"b")?;
    let mut fx = fx;
    fx.cat.add_folder(&fx.root.join("sub/x"))?;
    std::fs::remove_file(fx.root.join("sub/x/B.nef"))?;
    let to = fx.folder("sub/x");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![b, c],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    assert_eq!(
        plan.skipped(),
        [Skipped {
            photo: b,
            reason: Skip::Refused(Refusal::NameTaken("b.NEF".into()))
        }]
    );
    // A photo of the catalog there holds its name in any case.
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    assert_eq!(
        plan.skipped().first().map(|s| s.reason.clone()),
        Some(Skip::Refused(Refusal::NameTaken("a.NEF".into())))
    );
    // A file the catalog doesn't know, compared as the folder compares
    // names, or folded where it isn't shown to tell them apart.
    let to = fx.folder("sub/x");
    std::fs::write(fx.root.join("sub/x/C.NEF"), b"not in the catalog")?;
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![c],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    let refused = plan.skipped().first().map(|s| s.reason.clone());
    let dir = fx.root.join("sub/x");
    let rules = fsops::probe(&dir, &fsops::list(&dir)?);
    if rules.case == fsops::Folding::Exact {
        assert_eq!(refused, None);
    } else {
        assert_eq!(
            refused,
            Some(Skip::Refused(Refusal::NameTaken("c.NEF".into())))
        );
    }
    Ok(())
}

#[test]
fn two_photos_arriving_with_one_name_are_both_refused() -> Result<()> {
    let fx = fixture(&["one/a.NEF", "two/a.NEF", "sub/"]);
    let (x, y) = (fx.photo("one/a.NEF"), fx.photo("two/a.NEF"));
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![x, y],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert!(plan.is_empty());
    let reasons: Vec<_> = plan.skipped().iter().map(|s| s.reason.clone()).collect();
    assert_eq!(
        reasons,
        [
            Skip::Refused(Refusal::NameTwice("a.NEF".into())),
            Skip::Refused(Refusal::NameTwice("a.NEF".into()))
        ]
    );
    Ok(())
}

#[cfg(target_vendor = "apple")]
#[test]
fn an_nfd_name_takes_the_nfc_name_on_a_normalising_volume() -> Result<()> {
    let fx = fixture(&["caf\u{e9}.NEF", "sub/cafe\u{301}.NEF"]);
    let a = fx.photo("caf\u{e9}.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert!(matches!(
        &plan.skipped()[0].reason,
        Skip::Refused(Refusal::NameTaken(_))
    ));
    Ok(())
}

#[test]
fn a_photo_already_in_the_folder_is_skipped() -> Result<()> {
    let fx = fixture(&["sub/a.NEF"]);
    let a = fx.photo("sub/a.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert_eq!(plan.skipped()[0].reason, Skip::AlreadyThere);
    Ok(())
}

#[test]
fn a_shared_sidecar_stays_and_is_said_to() -> Result<()> {
    // `A.NEF.xmp` is A.NEF's sidecar and A.NEF.DNG's stem sidecar.
    let mut fx = fixture(&["A.NEF", "A.NEF.DNG", "A.NEF.xmp", "sub/"]);
    let a = fx.photo("A.NEF");
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    assert_eq!(plan.summary().companions, 0);
    assert_eq!(
        plan.summary().shared_companions,
        [fx.root.join("A.NEF.xmp")]
    );
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_of(to),
            to: fx.logical(to),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [moved(to)]);
    assert!(fx.root.join("A.NEF.xmp").is_file());
    assert!(fx.root.join("A.NEF.DNG").is_file());
    Ok(())
}

#[test]
fn a_companion_another_file_there_would_read_is_refused() -> Result<()> {
    // A.NEF with A.xmp, into a folder with A.DNG: A.DNG would read it.
    let fx = fixture(&["A.NEF", "A.xmp", "sub/A.DNG"]);
    let a = fx.photo("A.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert_eq!(
        plan.skipped()[0].reason,
        Skip::Refused(Refusal::CompanionClash("A.DNG".into()))
    );
    // B.NEF into a folder with an orphaned B.NEF.xmp: it would read it.
    let fx = fixture(&["B.NEF", "sub/", "sub/B.NEF.xmp"]);
    let b = fx.photo("B.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![b],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert_eq!(
        plan.skipped()[0].reason,
        Skip::Refused(Refusal::CompanionClash("B.NEF.xmp".into()))
    );
    // A.NEF and A.DNG from two folders, one bringing A.xmp: each would
    // read it there.
    let fx = fixture(&["one/A.NEF", "one/A.xmp", "two/A.DNG", "sub/"]);
    let (x, y) = (fx.photo("one/A.NEF"), fx.photo("two/A.DNG"));
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![x, y],
        root: fx.root_id(),
        to: "sub".into(),
    });
    // Skips come in photo id order, which the listing decides.
    let reason = |id| {
        plan.skipped()
            .iter()
            .find(|s| s.photo == id)
            .map(|s| s.reason.clone())
    };
    assert_eq!(plan.skipped().len(), 2);
    assert_eq!(
        reason(x),
        Some(Skip::Refused(Refusal::CompanionClash("A.DNG".into())))
    );
    assert_eq!(
        reason(y),
        Some(Skip::Refused(Refusal::CompanionClash("A.xmp".into())))
    );
    Ok(())
}

#[test]
fn two_folders_over_one_directory_make_its_files_shared() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    // A second root over the same directory, through a link, with its own
    // photo of the file.
    let alias = fx.dir.path().join("alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&fx.root, &alias)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&fx.root, &alias)?;
    fx.add_row(&alias, "", "a.NEF");
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    assert_eq!(plan.skipped()[0].reason, Skip::Refused(Refusal::Shared));
    // Or a duplicate row of the same file in its own folder.
    let mut fx = fixture(&["b.NEF", "sub/"]);
    let b = fx.photo("b.NEF");
    let root = fx.root.clone();
    fx.add_row(&root, "", "B.nef");
    let plan = fx.plan(FileOp::TrashPhotos { photos: vec![b] });
    assert_eq!(plan.skipped()[0].reason, Skip::Refused(Refusal::Shared));
    Ok(())
}

#[test]
fn a_hard_linked_file_is_shared() -> Result<()> {
    let fx = fixture(&["a.NEF"]);
    let a = fx.photo("a.NEF");
    std::fs::hard_link(fx.root.join("a.NEF"), fx.dir.path().join("elsewhere.NEF"))?;
    let plan = fx.plan(FileOp::TrashPhotos { photos: vec![a] });
    assert_eq!(plan.skipped()[0].reason, Skip::Refused(Refusal::Shared));
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_link_inside_the_root_is_not_followed() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let real = fx.dir.path().join("real");
    std::fs::create_dir(&real)?;
    std::fs::write(real.join("b.NEF"), b"b")?;
    std::os::unix::fs::symlink(&real, fx.root.join("linked"))?;
    // A folder of the root reached through the link, as a catalog made
    // before the link was there would have it.
    let root = fx.root.clone();
    let b = fx.add_row(&root, "linked", "b.NEF");
    let plan = fx.plan(FileOp::TrashPhotos { photos: vec![b] });
    assert!(matches!(
        &plan.skipped()[0].reason,
        Skip::Refused(Refusal::NotPlain(p)) if p.ends_with("linked")
    ));
    // Nor into one.
    let a = fx.photo("a.NEF");
    let linked = fx
        .cat
        .folders()?
        .into_iter()
        .find(|f| f.path.ends_with("linked"))
        .unwrap()
        .id;
    let into = plan::plan(
        &fx.cat,
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_of(linked),
            to: fx.logical(linked),
        },
    );
    assert!(matches!(into, Err(FileOpError::Destination(_))));
    Ok(())
}

#[test]
fn cancelling_stops_between_items() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "b.NEF", "sub/"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a, b],
        root: fx.root_id(),
        to: "sub".into(),
    });
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::new(&cancel),
        &Faulty::default(),
    )?;
    assert!(outcome.items.is_empty());
    assert!(fx.root.join("a.NEF").is_file() && fx.root.join("b.NEF").is_file());
    Ok(())
}

#[test]
fn a_plan_runs_only_in_its_own_catalog() -> Result<()> {
    let fx = fixture(&["a.NEF"]);
    let mut other = fixture(&["a.NEF"]);
    let plan = fx.plan(FileOp::RemoveFromCatalog {
        photos: vec![fx.photo("a.NEF")],
    });
    let run = execute::execute_with(
        &mut other.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    );
    assert!(matches!(run, Err(FileOpError::WrongCatalog)));
    Ok(())
}

#[test]
fn the_outcome_reads_the_same_from_the_journal_afterwards() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
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
    )?;
    fx.reopen();
    assert_eq!(super::outcome(&fx.cat, &batch)?, outcome);
    Ok(())
}

#[test]
fn copies_of_one_master_chosen_without_it_are_removed_together() -> Result<()> {
    let mut fx = fixture(&["a.NEF"]);
    let a = fx.photo("a.NEF");
    let (x, y) = (
        fx.cat.create_virtual_copy(a)?,
        fx.cat.create_virtual_copy(a)?,
    );
    let outcome = fx.run(
        FileOp::RemoveFromCatalog { photos: vec![x, y] },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::Removed]);
    assert_eq!(outcome.items[0].photos, [x, y]);
    assert_eq!(fx.rows().into_keys().collect::<Vec<_>>(), [a]);
    Ok(())
}

#[test]
fn a_sidecar_reader_appearing_after_planning_leaves_the_photo_alone() -> Result<()> {
    // In the destination (it would read the arriving a.xmp), or in the
    // source (a.xmp would no longer be a.NEF's alone).
    for at in ["sub/a.DNG", "a.DNG"] {
        let mut fx = fixture(&["a.NEF", "a.xmp", "sub/"]);
        let a = fx.photo("a.NEF");
        let plan = fx.plan(FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_id(),
            to: "sub".into(),
        });
        std::fs::write(fx.root.join(at), b"another reader")?;
        let outcome = execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &NoTrash,
            Cancel::NEVER,
            &Faulty::default(),
        )?;
        assert!(
            matches!(&results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
            "{at}"
        );
        assert!(fx.root.join("a.NEF").is_file() && fx.root.join("a.xmp").is_file());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_destination_replaced_by_a_link_after_planning_is_not_followed() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    let elsewhere = fx.dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere)?;
    std::fs::rename(fx.root.join("sub"), fx.dir.path().join("sub-aside"))?;
    std::os::unix::fs::symlink(&elsewhere, fx.root.join("sub"))?;
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert!(matches!(
        &results(&outcome)[..],
        [ItemResult::Unchanged(Some(_))]
    ));
    assert!(fx.root.join("a.NEF").is_file());
    assert!(std::fs::read_dir(&elsewhere)?.next().is_none());
    Ok(())
}

#[test]
fn a_sidecar_with_another_name_elsewhere_stays_and_the_photo_still_moves() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp", "sub/"]);
    let a = fx.photo("a.NEF");
    std::fs::hard_link(fx.root.join("a.xmp"), fx.dir.path().join("linked.xmp"))?;
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    assert_eq!(plan.summary().shared_companions, [fx.root.join("a.xmp")]);
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_of(to),
            to: fx.logical(to),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [moved(to)]);
    assert!(fx.root.join("a.xmp").is_file());
    Ok(())
}

#[test]
fn a_second_row_naming_the_same_file_by_an_alias_makes_it_shared() -> Result<()> {
    let mut fx = fixture(&["\u{df}.NEF", "sub/"]);
    let a = fx.photo("\u{df}.NEF");
    // Only where the volume takes both names for one file.
    if fsops::entry(&fx.root.join("SS.NEF")).is_err() {
        return Ok(());
    }
    let root = fx.root.clone();
    fx.add_row(&root, "", "SS.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert_eq!(plan.skipped()[0].reason, Skip::Refused(Refusal::Shared));
    Ok(())
}

/// Whether this volume takes `SS` for `ß` in a name (APFS does).
fn sharp_s_aliases(dir: &std::path::Path) -> bool {
    let probe = dir.join("\u{df}probe");
    std::fs::write(&probe, b"").unwrap();
    let alike = fsops::entry(&dir.join("SSprobe")).is_ok();
    std::fs::remove_file(probe).unwrap();
    alike
}

#[test]
fn companions_are_owned_by_any_name_the_volume_takes_for_theirs() -> Result<()> {
    // SS.DNG reads ß.xmp where the volume takes SS for ß: it stays.
    let fx = fixture(&["\u{df}.NEF", "\u{df}.xmp", "SS.DNG", "sub/"]);
    if !sharp_s_aliases(&fx.root) {
        return Ok(());
    }
    let a = fx.photo("\u{df}.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert_eq!(plan.summary().companions, 0);
    assert_eq!(plan.summary().shared_companions.len(), 1);
    // ß.NEF into a folder with SS.xmp would read it.
    let fx = fixture(&["\u{df}.NEF", "sub/", "sub/SS.xmp"]);
    let a = fx.photo("\u{df}.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert!(matches!(
        &plan.skipped()[0].reason,
        Skip::Refused(Refusal::CompanionClash(_))
    ));
    Ok(())
}

#[test]
fn an_offline_photo_holds_its_name_against_any_alias_of_it() -> Result<()> {
    let mut fx = fixture(&["\u{df}.NEF", "sub/SS.NEF"]);
    if !sharp_s_aliases(&fx.root) {
        return Ok(());
    }
    let a = fx.photo("\u{df}.NEF");
    std::fs::remove_file(fx.root.join("sub/SS.NEF"))?;
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    assert!(matches!(
        plan.skipped(),
        [Skipped {
            reason: Skip::Refused(Refusal::NameTaken(_)),
            ..
        }]
    ));
    let _ = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_of(to),
            to: fx.logical(to),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(fx.root.join("\u{df}.NEF").is_file());
    Ok(())
}

#[test]
fn capital_sharp_s_is_held_to_the_same_rules() -> Result<()> {
    // ẞ, ß and SS are one name where the volume folds them (APFS).
    let fx = fixture(&["\u{1e9e}.NEF", "\u{1e9e}.xmp", "SS.DNG", "sub/"]);
    if !sharp_s_aliases(&fx.root) || fsops::entry(&fx.root.join("SS.xmp")).is_err() {
        return Ok(());
    }
    let a = fx.photo("\u{1e9e}.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert_eq!(plan.summary().companions, 0, "SS.DNG reads it too");
    let fx = fixture(&["\u{1e9e}.NEF", "sub/", "sub/SS.xmp"]);
    let a = fx.photo("\u{1e9e}.NEF");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert!(matches!(
        &plan.skipped()[0].reason,
        Skip::Refused(Refusal::CompanionClash(_))
    ));
    let fx = fixture(&["\u{1e9e}.NEF", "sub/SS.NEF"]);
    let a = fx.photo("\u{1e9e}.NEF");
    std::fs::remove_file(fx.root.join("sub/SS.NEF"))?;
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    assert!(matches!(
        &plan.skipped()[0].reason,
        Skip::Refused(Refusal::NameTaken(_))
    ));
    Ok(())
}

#[test]
fn a_file_only_alike_in_compatibility_form_is_not_a_companion() -> Result<()> {
    // ①.xmp is not 1.NEF's sidecar: no volume reads it as 1.xmp.
    let mut fx = fixture(&["1.NEF", "\u{2460}.xmp", "sub/"]);
    if fsops::entry(&fx.root.join("1.xmp")).is_ok() {
        return Ok(());
    }
    let a = fx.photo("1.NEF");
    let to = fx.folder("sub");
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_of(to),
        to: fx.logical(to),
    });
    assert_eq!(plan.summary().companions, 0);
    let _ = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_of(to),
            to: fx.logical(to),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(fx.root.join("\u{2460}.xmp").is_file());
    Ok(())
}
