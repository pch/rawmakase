//! Moving photos to another volume: each file copied, verified and renamed
//! there before the catalog follows, then its original put in the Trash;
//! anything that fails before the catalog follows undone; and recovery of
//! whatever a crash leaves at any step. On one volume, the test disk takes
//! the destination folder for another volume's; one test, run when
//! `RAWMAKASE_TEST_OTHER_VOLUME` names a folder on another volume, uses two.
use super::journal::{self, ItemState, MemberState};
use super::recover_tests::{Before, before, check};
use super::tests::{
    FakeTrash, Fault, Faulty, Fx, NoTrash, Op, bytes, crashes, fixture, results, tagged,
};
use super::*;
use crate::catalog::HistoryUpdate;
use crate::export_settings::ExportOptions;
use crate::fsops;
use crate::model::recipe::Recipe;
use crate::storage::Identity;
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// A.NEF carries A.xmp; A.NEF.xmp is A.NEF.DNG's too, so it stays.
fn files() -> Fx {
    fixture(&["A.NEF", "A.xmp", "A.NEF.xmp", "A.NEF.DNG", "b.NEF", "far/"])
}

/// The test disk, with `far/` on another volume.
fn far(fx: &Fx) -> Faulty {
    far_as(fx, Far::Ids)
}

/// What `far/`'s volume is in a test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Far {
    /// One that keeps file ids.
    Ids,
    /// exFAT (#573): its files numbered anew when written, renamed or
    /// mounted again; proven by names.
    Names,
    /// exFAT on macOS, which can't rename exclusively either: copies are
    /// made at their names.
    Direct,
    /// An SMB share, as `Direct` but on the network and with no Trash of
    /// its own: proven by names.
    Share,
}

impl Far {
    /// The kinds proven by names here.
    pub(super) fn by_names() -> Vec<Far> {
        vec![Far::Names, Far::Direct, Far::Share]
    }
}

/// The test disk, with `far/` on another volume of `kind`.
pub(super) fn far_as(fx: &Fx, kind: Far) -> Faulty {
    far_at(&fx.root.join("far"), kind)
}

/// The test disk, with folder `dir` on another volume of `kind`.
pub(super) fn far_at(dir: &Path, kind: Far) -> Faulty {
    let dir = dir.to_path_buf();
    let mut disk = Faulty::default().elsewhere(&dir);
    if kind != Far::Ids {
        disk.names_only = vec![dir];
        disk.renumber_on_close = true;
        disk.renumber_on_rename = true;
        disk.no_exclusive_rename = matches!(kind, Far::Direct | Far::Share);
        disk.share = kind == Far::Share;
        // As macOS's drivers list names, and keep what the drive can't
        // (only macOS makes AppleDouble files): not on a share, whose
        // server keeps extended attributes as named streams (a Synology,
        // as measured for #573).
        disk.lists_decomposed = true;
        disk.apple_double = cfg!(target_vendor = "apple") && kind != Far::Share;
    }
    disk
}

/// The Trash as it is beside a drive of `kind`: none on an SMB share
/// (macOS's answers `Unsupported` there, and Linux's isn't used).
pub(super) fn trash_as(fx: &Fx, kind: Far) -> FakeTrash {
    let mut trash = FakeTrash::new(fx);
    if kind == Far::Share {
        trash.refuses = vec![fx.root.join("far")];
    }
    trash
}

fn to_far(fx: &Fx, names: &[&str]) -> FileOp {
    FileOp::MovePhotos {
        photos: names.iter().map(|n| fx.photo(n)).collect(),
        root: fx.root_id(),
        to: "far".into(),
    }
}

/// Plans `op` and runs it on `disk`, stopping when `cancel` is set.
fn run(fx: &mut Fx, op: FileOp, trash: &dyn Trash, disk: &Faulty, cancel: Cancel<'_>) -> Outcome {
    let plan = fx.plan_on(op, disk).unwrap();
    execute::execute_with(&mut fx.cat, plan.confirm(), trash, cancel, disk).unwrap()
}

/// Partial copies anywhere below the fixture but in the Trash.
fn partials(fx: &Fx) -> Vec<String> {
    fx.files()
        .into_keys()
        .filter(|k| k.ends_with(".rawmakase-part") && !k.starts_with("trash/"))
        .collect()
}

/// Saves an edit and reads a checksum for photo `name`.
fn edited(fx: &mut Fx, name: &str) -> Result<()> {
    let id = fx.photo(name);
    let source = fx.root.join(name);
    fx.cat.save_edit(
        id,
        &source,
        &crate::storage::Identity::read(&source)?,
        &Recipe::default(),
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    let crate::storage::ChecksumRead::Done(checksum) =
        crate::storage::FileChecksum::read_full(&source, |_| true)?
    else {
        panic!("not read")
    };
    fx.cat.save_checksums(&[crate::catalog::ReadChecksum {
        photo: id,
        folder: fx.folder(""),
        filename: name.into(),
        path: source,
        checksum,
    }])?;
    Ok(())
}

/// The identity photo `id`'s saved edit names, as stored.
fn saved_identity(fx: &Fx, id: PhotoId) -> Option<Identity> {
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
fn a_move_to_another_drive_copies_verifies_and_trashes_the_original() -> Result<()> {
    let mut fx = files();
    edited(&mut fx, "A.NEF")?;
    let a = fx.photo("A.NEF");
    let copy = fx.cat.create_virtual_copy(a)?;
    let source = fx.root.join("A.NEF");
    let identity = Identity::read(&source)?;
    let modified = std::fs::metadata(&source)?.modified()?;
    let disk = far(&fx);
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
    let across = plan.summary().across.clone().expect("it copies");
    let sizes = bytes("A.NEF").len() + bytes("A.xmp").len();
    assert_eq!(across.bytes, sizes as u64);
    assert_eq!((across.files, across.companions), (1, 1));
    assert_eq!(
        across.volume,
        fx.root.join("far"),
        "where the other volume begins"
    );
    assert!(across.available > 0);
    assert_eq!(plan.summary().companions, 1);
    assert_eq!(
        plan.summary().shared_companions,
        [fx.root.join("A.NEF.xmp")]
    );
    let trash = FakeTrash::new(&fx);
    let outcome = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk)?;
    let to = fx.folder("far");
    assert_eq!(results(&outcome), [ItemResult::Moved { to }]);
    assert!(outcome.items[0].originals_kept.is_empty());
    let target = fx.root.join("far/A.NEF");
    assert_eq!(std::fs::read(&target)?, bytes("A.NEF"));
    assert_eq!(std::fs::read(fx.root.join("far/A.xmp"))?, bytes("A.xmp"));
    assert_eq!(
        std::fs::metadata(&target)?.modified()?,
        modified,
        "its time kept"
    );
    assert_eq!(Identity::read(&target)?, identity, "so its identity");
    assert_ne!(fsops::entry(&target)?.id, fsops::entry(&fx.root)?.id);
    // The originals are in the Trash, with receipts; the shared one stays.
    let files = fx.files();
    assert_eq!(files.get("trash/1-A.NEF"), Some(&bytes("A.NEF")));
    assert_eq!(files.get("trash/2-A.xmp"), Some(&bytes("A.xmp")));
    assert!(!source.exists() && !fx.root.join("A.xmp").exists());
    assert!(fx.root.join("A.NEF.xmp").is_file() && fx.root.join("A.NEF.DNG").is_file());
    let item = &journal::of_batch(&fx.cat.db, &outcome_batch(&fx, &outcome))?[0];
    assert_eq!(item.state, ItemState::Done);
    for m in &item.members {
        assert_eq!(m.state, MemberState::Trashed);
        assert!(m.receipt.is_some());
    }
    // The rows, the copy's too, follow; the edit opens; the checksum holds.
    assert_eq!(fx.rows()[&a], "far/A.NEF");
    assert_eq!(fx.rows()[&copy], "far/A.NEF");
    assert!(fx.cat.load_edit(a, &target)?.is_some());
    assert_eq!(fx.cat.checksums()?[&a].identity, identity);
    assert!(partials(&fx).is_empty());
    // Copied, not renamed.
    assert!(
        disk.renames()
            .iter()
            .all(|(from, _)| from.to_string_lossy().ends_with(".rawmakase-part"))
    );
    Ok(())
}

/// The batch of `outcome`'s first item.
fn outcome_batch(fx: &Fx, outcome: &Outcome) -> String {
    use crate::catalog::db::{Reads, sql};
    fx.cat
        .db
        .read_one(
            sql!("SELECT batch FROM file_operations WHERE id=?"),
            &[&outcome.items[0].operation.0],
        )
        .unwrap()
}

#[test]
fn a_move_from_two_drives_counts_only_what_it_copies() -> Result<()> {
    // c.NEF is on the other drive; A.NEF, with A.xmp, is on this one, as
    // the folder they go to.
    let mut fx = fixture(&["A.NEF", "A.xmp", "sub/", "far/c.NEF"]);
    let disk = far(&fx);
    let op = FileOp::MovePhotos {
        photos: vec![fx.photo("A.NEF"), fx.photo("far/c.NEF")],
        root: fx.root_id(),
        to: "sub".into(),
    };
    let plan = fx.plan_on(op.clone(), &disk)?;
    let summary = plan.summary();
    assert_eq!((summary.files, summary.companions), (2, 1), "all it moves");
    let across = summary.across.clone().expect("it copies c.NEF");
    assert_eq!((across.files, across.companions), (1, 0), "what it copies");
    assert_eq!(across.bytes, bytes("far/c.NEF").len() as u64);
    let trash = FakeTrash::new(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let to = fx.folder("sub");
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved { to }, ItemResult::Moved { to }]
    );
    assert_eq!(
        std::fs::read(fx.root.join("sub/c.NEF"))?,
        bytes("far/c.NEF")
    );
    assert_eq!(fx.files().get("trash/1-c.NEF"), Some(&bytes("far/c.NEF")));
    Ok(())
}

#[test]
fn a_move_to_or_from_a_drive_whose_file_ids_can_t_be_proven_is_refused() -> Result<()> {
    let fx = files();
    let (here, there) = (fx.root.clone(), fx.root.join("far"));
    let why = "Moving photos to or from this kind of drive (network drives other than SMB shares, and others)";
    for unstable in [vec![there.clone()], vec![here.clone()], vec![here, there]] {
        let disk = Faulty {
            unstable_ids: unstable.clone(),
            ..far(&fx)
        };
        let plan = fx.plan_on(to_far(&fx, &["A.NEF", "b.NEF"]), &disk)?;
        assert!(plan.is_empty(), "{unstable:?}");
        assert_eq!(plan.summary().across, None);
        for skipped in plan.skipped() {
            assert_eq!(skipped.reason, Skip::Refused(Refusal::FileIdsUnproven));
            assert!(
                skipped.reason.to_string().starts_with(why),
                "{}",
                skipped.reason
            );
        }
        assert_eq!(plan.skipped().len(), 2);
    }
    // On one such drive, a move still renames.
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let disk = Faulty {
        unstable_ids: vec![fx.root.clone(), fx.root.join("sub")],
        ..Faulty::default()
    };
    let op = FileOp::MovePhotos {
        photos: vec![fx.photo("a.NEF")],
        root: fx.root_id(),
        to: "sub".into(),
    };
    let outcome = run(&mut fx, op, &NoTrash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("sub")
        }]
    );
    Ok(())
}

#[test]
fn a_move_on_one_drive_still_renames() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp", "sub/"]);
    let a = fx.photo("a.NEF");
    let id = fsops::entry(&fx.root.join("a.NEF"))?.id;
    let disk = Faulty::default();
    let op = FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    };
    assert_eq!(fx.plan_on(op.clone(), &disk)?.summary().across, None);
    let outcome = run(&mut fx, op, &NoTrash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("sub")
        }]
    );
    assert_eq!(fsops::entry(&fx.root.join("sub/a.NEF"))?.id, id);
    for op in [Op::CreateFile, Op::Copy, Op::Hash, Op::RemoveFile] {
        assert_eq!(disk.count(op), 0, "{op:?}");
    }
    Ok(())
}

#[test]
fn coarser_times_there_rekey_the_saved_edit_and_checksum() -> Result<()> {
    let mut fx = files();
    edited(&mut fx, "A.NEF")?;
    let a = fx.photo("A.NEF");
    let copy = fx.cat.create_virtual_copy(a)?;
    let was = Identity::read(&fx.root.join("A.NEF"))?;
    let mut disk = far(&fx);
    // FAT's two seconds.
    disk.coarse = Some(2_000_000_000);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx, &["A.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    let target = fx.root.join("far/A.NEF");
    let now = Identity::read(&target)?;
    assert_ne!(now, was, "the time was cut");
    assert_eq!((now.size, &now.prefix_hash), (was.size, &was.prefix_hash));
    assert_eq!(std::fs::read(&target)?, bytes("A.NEF"));
    assert_eq!(saved_identity(&fx, a), Some(now.clone()));
    assert_eq!(saved_identity(&fx, copy), Some(now.clone()));
    assert_eq!(fx.cat.checksums()?[&a].identity, now);
    assert!(fx.cat.load_edit(a, &target)?.is_some(), "the edit opens");
    assert!(fx.cat.load_edit(copy, &target)?.is_some());
    Ok(())
}

#[test]
fn an_edit_saved_for_another_version_is_not_rekeyed() -> Result<()> {
    let mut fx = files();
    edited(&mut fx, "A.NEF")?;
    let a = fx.photo("A.NEF");
    // The saved edit names another version of the file.
    let other = Identity {
        size: 1,
        modified_ns: 1,
        prefix_hash: "0".into(),
    };
    fx.cat.db_for_tests().execute(
        "UPDATE photos SET identity=? WHERE id=?",
        rusqlite::params![serde_json::to_string(&other)?, a.0],
    )?;
    let mut disk = far(&fx);
    disk.coarse = Some(2_000_000_000);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx, &["A.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    assert_eq!(saved_identity(&fx, a), Some(other), "still protected");
    Ok(())
}

#[test]
fn an_earlier_releases_edit_file_and_coarser_times_leave_the_photo() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.NEF.rawmakase.json", "far/"]);
    let a = fx.photo("a.NEF");
    let mut disk = far(&fx);
    disk.coarse = Some(2_000_000_000);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx, &["a.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert!(why.contains("earlier release"), "{why}");
    assert_eq!(fx.rows()[&a], "a.NEF");
    // The originals where they were, the copies in the Trash.
    let files = fx.files();
    assert_eq!(files.get("trip/a.NEF"), Some(&bytes("a.NEF")));
    assert!(!files.contains_key("trip/far/a.NEF"));
    assert!(!files.contains_key("trip/far/a.NEF.rawmakase.json"));
    assert_eq!(
        files.get("trash/1-a.NEF.rawmakase.json"),
        Some(&bytes("a.NEF.rawmakase.json"))
    );
    assert_eq!(files.get("trash/2-a.NEF"), Some(&bytes("a.NEF")));
    assert!(partials(&fx).is_empty());
    // Without coarser times it moves.
    let disk = far(&fx);
    let op = to_far(&fx, &["a.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    Ok(())
}

/// Moves A.NEF to far/ with `disk` faulting, expecting the item
/// unchanged for a reason containing `said`, its rows and the files outside
/// the Trash as before, and no partial copy left.
fn unchanged(disk: Faulty, said: &str) -> Result<(Fx, Faulty)> {
    unchanged_at(files(), disk, said)
}

#[test]
fn a_full_drive_part_way_through_a_copy_removes_the_partial_copy() -> Result<()> {
    let (_fx, disk) = unchanged(Faulty::at(Op::Copy, 0, Fault::Fail), "full")?;
    assert_eq!(disk.count(Op::RemoveFile), 1);
    Ok(())
}

#[test]
fn a_copy_that_reads_back_otherwise_is_undone() -> Result<()> {
    let disk = Faulty {
        corrupt: true,
        ..Faulty::default()
    };
    unchanged(disk, "didn't match the original")?;
    Ok(())
}

#[test]
fn a_photos_extras_go_with_it_to_another_drive() -> Result<()> {
    for kind in [Far::Ids, Far::Names, Far::Direct] {
        let mut fx = files();
        let (a, xmp) = (fx.root.join("A.NEF"), fx.root.join("A.xmp"));
        if !tagged(&a, "tag", b"Red") || !tagged(&xmp, "origin", b"camera") {
            return Ok(());
        }
        let extras = (fsops::extras(&a)?, fsops::extras(&xmp)?);
        let disk = far_as(&fx, kind);
        let trash = FakeTrash::new(&fx);
        let op = to_far(&fx, &["A.NEF"]);
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        let to = fx.folder("far");
        assert_eq!(results(&outcome), [ItemResult::Moved { to }], "{kind:?}");
        assert!(outcome.items[0].originals_kept.is_empty(), "{outcome:?}");
        let copies = (fx.root.join("far/A.NEF"), fx.root.join("far/A.xmp"));
        assert_eq!(
            (fsops::extras(&copies.0)?, fsops::extras(&copies.1)?),
            extras,
            "{kind:?}"
        );
        assert!(!a.exists() && !xmp.exists(), "{kind:?}");
    }
    Ok(())
}

#[test]
fn a_drive_that_can_t_keep_a_photos_extras_moves_nothing() -> Result<()> {
    let disk = Faulty {
        keeps_no_extras: true,
        ..Faulty::default()
    };
    let (fx, _) = unchanged(disk, "can't keep")?;
    assert!(fx.root.join("A.NEF").is_file());
    Ok(())
}

#[test]
fn an_original_tagged_while_it_was_copied_is_left_as_it_is() -> Result<()> {
    let fx = files();
    let disk = Faulty {
        retag: Some((fx.root.join("A.NEF"), 1)),
        ..Faulty::default()
    };
    unchanged_at(fx, disk, "changed while it was copied")?;
    Ok(())
}

#[test]
fn an_original_tagged_after_the_catalog_followed_is_kept() -> Result<()> {
    let mut fx = files();
    let a = fx.root.join("A.NEF");
    // Read before it is copied, once copied, then before it would go.
    let disk = Faulty {
        retag: Some((a.clone(), 2)),
        ..far(&fx)
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx, &["A.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let to = fx.folder("far");
    assert_eq!(results(&outcome), [ItemResult::Moved { to }]);
    let kept: Vec<_> = outcome.items[0]
        .originals_kept
        .iter()
        .map(|k| (k.path.clone(), k.why.clone()))
        .collect();
    assert_eq!(kept, [(a.clone(), super::copy::EXTRAS_DIFFER.to_string())]);
    assert_eq!(std::fs::read(&a)?, bytes("A.NEF"));
    assert!(!fx.root.join("A.xmp").exists(), "its companion went");
    Ok(())
}

/// Runs the move of A.NEF to far/ on `disk`, expecting it left for the
/// user with its copy of A.NEF whole and hidden in far/, and the catalog as
/// it was; returns the fixture and that copy.
fn kept_whole(fx: Fx, disk: Faulty) -> Result<(Fx, PathBuf)> {
    let mut fx = fx;
    let rows = fx.rows();
    let disk = Faulty {
        elsewhere: Some(fx.root.join("far")),
        ..disk
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx, &["A.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert!(
        matches!(&results(&outcome)[..], [ItemResult::NeedsRecovery(_)]),
        "{outcome:?}"
    );
    assert_eq!(fx.rows(), rows);
    let [temp] = &partials(&fx)[..] else {
        panic!("{:?}", partials(&fx))
    };
    let temp = fx.dir.path().canonicalize()?.join(temp);
    assert_eq!(std::fs::read(&temp)?, bytes("A.NEF"), "kept whole");
    assert!(!fx.files().keys().any(|k| k.starts_with("trash/")));
    Ok((fx, temp))
}

#[test]
fn an_original_changed_while_it_was_copied_keeps_its_verified_copy() -> Result<()> {
    // The copy has the bytes the original had: perhaps the only ones left.
    let disk = Faulty {
        change_source: true,
        ..Faulty::default()
    };
    let (fx, _) = kept_whole(files(), disk)?;
    let mut changed = bytes("A.NEF");
    changed.push(b'!');
    assert_eq!(std::fs::read(fx.root.join("A.NEF"))?, changed);
    assert!(!fx.root.join("far/A.NEF").exists());
    Ok(())
}

#[test]
fn an_original_gone_while_it_was_copied_keeps_its_verified_copy() -> Result<()> {
    let fx = files();
    let disk = Faulty {
        take_source: Some(fx.dir.path().canonicalize()?.join("taken.NEF")),
        ..Faulty::default()
    };
    let (mut fx, temp) = kept_whole(fx, disk)?;
    // Recovery keeps it too, while the original can't be proven.
    let recovery = fx.recover_with(&far(&fx), &FakeTrash::new(&fx));
    assert_eq!(recovery.unfinished.len(), 1);
    assert_eq!(std::fs::read(&temp)?, bytes("A.NEF"));
    Ok(())
}

/// The files of `fx` below far/, with their bytes.
fn in_far(fx: &Fx) -> BTreeMap<String, Vec<u8>> {
    fx.files()
        .into_iter()
        .filter(|(k, _)| k.starts_with("trip/far/"))
        .collect()
}

/// A photo whose name is composed (NFC), which a volume like HFS+ keeps
/// decomposed.
const CAFE: &str = "Caf\u{e9}.NEF";

#[test]
fn a_volume_spelling_names_otherwise_still_has_its_copies_found() -> Result<()> {
    // A move that finishes, one undone, and one a crash cuts short part way
    // through a copy, recovered on the same volume.
    for (fault, ends) in [
        (None, "moved"),
        (Some(Fault::Fail), "undone"),
        (Some(Fault::CrashMid), "recovered"),
    ] {
        let mut fx = fixture(&[CAFE, "far/"]);
        let photo = fx.photo(CAFE);
        let base = match fault {
            Some(fault) => Faulty::at(Op::Copy, 0, fault),
            None => Faulty::default(),
        };
        let disk = Faulty {
            decompose: true,
            ..base
        }
        .elsewhere(&fx.root.join("far"));
        let trash = FakeTrash::new(&fx);
        let plan = fx.plan_on(
            FileOp::MovePhotos {
                photos: vec![photo],
                root: fx.root_id(),
                to: "far".into(),
            },
            &disk,
        )?;
        let batch = plan.batch().clone();
        crashes(|| {
            drop(
                execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk)
                    .unwrap(),
            );
        });
        fx.reopen();
        let clean = Faulty {
            decompose: true,
            ..far(&fx)
        };
        let recovery = fx.recover_with(&clean, &trash);
        assert!(recovery.unfinished.is_empty(), "{ends}: {recovery:?}");
        assert!(partials(&fx).is_empty(), "{ends}: {:?}", partials(&fx));
        let result = &results(&super::outcome(&fx.cat, &batch)?)[0];
        match ends {
            "moved" => {
                assert!(matches!(result, ItemResult::Moved { .. }), "{result:?}");
                let copy = fx
                    .cat
                    .photos()?
                    .into_iter()
                    .find(|p| p.id == photo)
                    .unwrap()
                    .path;
                assert_eq!(std::fs::read(&copy)?, bytes(CAFE));
            }
            _ => {
                assert!(
                    matches!(result, ItemResult::Unchanged(_)),
                    "{ends}: {result:?}"
                );
                assert_eq!(std::fs::read(fx.root.join(CAFE))?, bytes(CAFE));
                assert_eq!(in_far(&fx).len(), 1, "{ends}: only keep.dng");
            }
        }
    }
    Ok(())
}

#[test]
fn put_back_of_a_copy_whose_original_changed_discards_its_copies() -> Result<()> {
    // The photo is copied whole and given its name; its sidecar's copy is
    // under way (not whole), or whole but not yet given its name, when the
    // process ends; then another app rewrites an original: the photo's, or
    // the sidecar's, whose whole hidden copy then has the only bytes the
    // sidecar had.
    for whole in [false, true] {
        let mut fx = files();
        let fault = match whole {
            false => Faulty::at(Op::Copy, 1, Fault::CrashMid),
            true => Faulty::at(Op::Rename, 1, Fault::CrashBefore),
        };
        let disk = Faulty {
            elsewhere: Some(fx.root.join("far")),
            ..fault
        };
        let trash = FakeTrash::new(&fx);
        let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
        assert!(crashes(|| {
            let _ =
                execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
        }));
        fx.reopen();
        let [temp] = &partials(&fx)[..] else {
            panic!("{:?}", partials(&fx))
        };
        let temp_name = temp.rsplit('/').next().unwrap().to_string();
        let temp = fx.dir.path().canonicalize()?.join(temp);
        let original = fx.root.join(if whole { "A.xmp" } else { "A.NEF" });
        let id = fsops::entry(&original)?.id;
        let mut f = std::fs::OpenOptions::new().append(true).open(&original)?;
        std::io::Write::write_all(&mut f, b" rewritten")?;
        drop(f);
        let rewritten = std::fs::read(&original)?;
        let recovery = fx.recover_with(&far(&fx), &trash);
        let [u] = &recovery.unfinished[..] else {
            panic!("{whole}: {recovery:?}")
        };
        // The sidecar's copy is shown where it is: hidden, not at its name.
        let xmp = u
            .members
            .iter()
            .find(|m| m.source.ends_with("A.xmp"))
            .unwrap();
        assert_eq!(xmp.after.as_deref(), Some(temp.as_path()));
        let photo = u
            .members
            .iter()
            .find(|m| m.source.ends_with("A.NEF"))
            .unwrap();
        assert_eq!(
            photo.after.as_deref(),
            Some(fx.root.join("far/A.NEF").as_path())
        );
        // Put Back: the originals are where they were, so the copies go:
        // a whole one to the Trash, one not whole removed.
        let clean = far(&fx);
        let back = recover::resolve_with(
            &mut fx.cat,
            u.operation,
            Resolution::PutBack,
            &clean,
            &trash,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        assert!(matches!(back.result, ItemResult::Unchanged(_)), "{back:?}");
        assert!(partials(&fx).is_empty(), "{whole}");
        assert!(!fx.root.join("far/A.NEF").exists());
        let files = fx.files();
        let photo_in_trash = match whole {
            true => {
                assert_eq!(
                    files.get(&format!("trash/1-{temp_name}")),
                    Some(&bytes("A.xmp")),
                    "the whole hidden copy, in the Trash"
                );
                assert_eq!(clean.count(Op::RemoveFile), 0, "nothing unlinked");
                "trash/2-A.NEF"
            }
            false => {
                assert_eq!(clean.count(Op::RemoveFile), 1, "the partial copy removed");
                "trash/1-A.NEF"
            }
        };
        assert_eq!(files.get(photo_in_trash), Some(&bytes("A.NEF")), "{whole}");
        assert_eq!(
            std::fs::read(&original)?,
            rewritten,
            "the original untouched"
        );
        assert_eq!(fsops::entry(&original)?.id, id);
    }
    Ok(())
}

#[test]
fn a_volume_renumbering_its_files_never_loses_or_misjudges_a_copy() -> Result<()> {
    for (close, rename) in [(true, false), (false, true)] {
        let mut fx = files();
        let a = fx.photo("A.NEF");
        let disk = Faulty {
            renumber_on_close: close,
            renumber_on_rename: rename,
            ..far(&fx)
        };
        let trash = FakeTrash::new(&fx);
        let op = to_far(&fx, &["A.NEF"]);
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        let what = format!("close {close} rename {rename}");
        // Its copy can't be told its own once renamed: it stays, the item
        // waits for the user, and nothing is removed or put in the Trash.
        assert!(
            matches!(&results(&outcome)[..], [ItemResult::NeedsRecovery(_)]),
            "{what}: {outcome:?}"
        );
        assert_eq!(
            std::fs::read(fx.root.join("far/A.NEF"))?,
            bytes("A.NEF"),
            "{what}"
        );
        assert_eq!(
            std::fs::read(fx.root.join("A.NEF"))?,
            bytes("A.NEF"),
            "{what}"
        );
        assert_eq!(fx.rows()[&a], "A.NEF", "{what}");
        assert!(
            !fx.files().keys().any(|k| k.starts_with("trash/")),
            "{what}"
        );
        assert_eq!(disk.count(Op::RemoveFile), 0, "{what}");
        let item = &journal::of_batch(&fx.cat.db, &outcome_batch(&fx, &outcome))?[0];
        assert_ne!(item.members[0].state, MemberState::Discarded, "{what}");
        // Recovery, on the same volume, decides nothing either.
        let recovery = fx.recover_with(&disk, &trash);
        assert_eq!(recovery.unfinished.len(), 1, "{what}");
        assert_eq!(
            std::fs::read(fx.root.join("far/A.NEF"))?,
            bytes("A.NEF"),
            "{what}"
        );
    }
    Ok(())
}

#[test]
fn a_remount_renumbering_and_reusing_numbers_misleads_no_recovery() -> Result<()> {
    // Crashed with the copy at its name (journalled copying), or part way
    // through it (the partial copy there); then the volume is remounted:
    // every file is renumbered, and the copy's old number goes to another
    // file there.
    for (op, fault) in [(Op::Rename, Fault::CrashAfter), (Op::Copy, Fault::CrashMid)] {
        let mut fx = files();
        let disk = Faulty {
            elsewhere: Some(fx.root.join("far")),
            ..Faulty::at(op, 0, fault)
        };
        let trash = FakeTrash::new(&fx);
        let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
        assert!(crashes(|| {
            let _ =
                execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
        }));
        fx.reopen();
        let copy = match op {
            Op::Rename => fx.root.join("far/A.NEF"),
            _ => {
                let [temp] = &partials(&fx)[..] else {
                    panic!("{:?}", partials(&fx))
                };
                fx.dir.path().canonicalize()?.join(temp)
            }
        };
        let other = fx.root.join("far/keep.dng");
        let before = in_far(&fx);
        let remounted = far(&fx);
        remounted.remount(&[&copy, &other], Some((&copy, &other)));
        let recovery = fx.recover_with(&remounted, &trash);
        let what = format!("{op:?} {fault:?}");
        assert_eq!(recovery.unfinished.len(), 1, "{what}: {recovery:?}");
        assert_eq!(in_far(&fx), before, "{what}: nothing there changed");
        assert_eq!(std::fs::read(fx.root.join("A.NEF"))?, bytes("A.NEF"));
        assert!(
            !fx.files().keys().any(|k| k.starts_with("trash/")),
            "{what}"
        );
        assert_eq!(remounted.count(Op::RemoveFile), 0, "{what}");
    }
    Ok(())
}

#[test]
fn a_name_taken_just_before_its_copy_gets_it_is_never_replaced() -> Result<()> {
    // The photo is copied; its companion's name is taken at the last
    // moment: the photo's copy goes to the Trash, the file there stays.
    let fx = files();
    let disk = Faulty {
        occupy: Some(fx.root.join("far/A.xmp")),
        ..Faulty::default()
    };
    let (fx, _) = unchanged_at(fx, disk, "A.xmp is there now")?;
    let files = fx.files();
    assert_eq!(files.get("trip/far/A.xmp"), Some(&b"raced".to_vec()));
    assert!(!files.contains_key("trip/far/A.NEF"));
    assert_eq!(
        files.get("trash/1-A.NEF"),
        Some(&bytes("A.NEF")),
        "the copy"
    );
    assert_eq!(
        files.get("trip/A.NEF"),
        Some(&bytes("A.NEF")),
        "the original"
    );
    Ok(())
}

/// [`unchanged`] on fixture `fx`.
fn unchanged_at(mut fx: Fx, disk: Faulty, said: &str) -> Result<(Fx, Faulty)> {
    let rows = fx.rows();
    let outside = |fx: &Fx| -> Vec<(String, Vec<u8>)> {
        fx.files()
            .into_iter()
            .filter(|(k, _)| !k.starts_with("trash/"))
            .collect()
    };
    let originals = outside(&fx);
    let disk = Faulty {
        elsewhere: Some(fx.root.join("far")),
        ..disk
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx, &["A.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert!(why.contains(said), "{why}");
    assert_eq!(fx.rows(), rows);
    if !disk.change_source && disk.occupy.is_none() {
        assert_eq!(outside(&fx), originals);
    }
    assert!(partials(&fx).is_empty());
    Ok((fx, disk))
}

#[test]
fn stop_stops_a_copy_between_chunks_and_undoes_it() -> Result<()> {
    let mut fx = fixture(&["big.NEF", "b.NEF", "far/"]);
    let big: Vec<u8> = (0..3 * fsops::COPY_CHUNK as u32 + 5)
        .map(|i| (i % 251) as u8)
        .collect();
    std::fs::write(fx.root.join("big.NEF"), &big)?;
    // Planned after its bytes changed, so it is the file as planned.
    let stop = Arc::new(AtomicBool::new(false));
    let disk = Faulty {
        stop: Some(stop.clone()),
        ..far(&fx)
    };
    let rows = fx.rows();
    let op = to_far(&fx, &["big.NEF", "b.NEF"]);
    let outcome = run(&mut fx, op, &NoTrash, &disk, Cancel::new(&stop));
    assert_eq!(disk.chunks(), 1, "it stopped after the first chunk");
    let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert_eq!(why, "Stopped.");
    assert_eq!(fx.rows(), rows, "b.NEF never started");
    assert_eq!(std::fs::read(fx.root.join("big.NEF"))?, big);
    assert!(partials(&fx).is_empty());
    assert!(!fx.root.join("far/big.NEF").exists());
    Ok(())
}

#[test]
fn a_volume_without_a_trash_keeps_the_original_and_says_so() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("A.NEF");
    let disk = far(&fx);
    let mut trash = FakeTrash::new(&fx);
    trash.fail_from = Some(1);
    let op = to_far(&fx, &["A.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    let kept: Vec<(PathBuf, String)> = outcome.items[0]
        .originals_kept
        .iter()
        .map(|k| (k.path.clone(), k.why.clone()))
        .collect();
    let why = TrashError::Unsupported.to_string();
    assert_eq!(
        kept,
        [
            (fx.root.join("A.NEF"), why.clone()),
            (fx.root.join("A.xmp"), why)
        ]
    );
    assert_eq!(fx.rows()[&a], "far/A.NEF");
    for name in ["A.NEF", "A.xmp"] {
        assert_eq!(std::fs::read(fx.root.join(name))?, bytes(name));
        assert_eq!(std::fs::read(fx.root.join("far").join(name))?, bytes(name));
    }
    // And it reads back so from the journal.
    let batch = BatchId(outcome_batch(&fx, &outcome));
    assert_eq!(super::outcome(&fx.cat, &batch)?.items, outcome.items);
    Ok(())
}

#[test]
fn not_enough_room_there_refuses_the_whole_move() -> Result<()> {
    let fx = files();
    let disk = Faulty {
        space: Some(1000),
        ..far(&fx)
    };
    let refused = fx.plan_on(to_far(&fx, &["A.NEF", "b.NEF"]), &disk);
    let needed = (bytes("A.NEF").len() + bytes("A.xmp").len() + bytes("b.NEF").len()) as u64;
    match refused {
        Err(FileOpError::NoSpace {
            needed: n,
            available,
        }) => assert_eq!((n, available), (needed, 1000)),
        other => panic!("{other:?}"),
    }
    // Room for it, but not for it and the spare the volume keeps; the
    // refusal says so, rather than seem to have room.
    let disk = Faulty {
        space: Some(needed + 1),
        ..far(&fx)
    };
    let refused = fx.plan_on(to_far(&fx, &["A.NEF", "b.NEF"]), &disk);
    assert!(matches!(refused, Err(FileOpError::NoSpace { .. })));
    let message = refused.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(message.contains("33.6 MB must stay free"), "{message}");
    Ok(())
}

#[test]
fn times_kept_more_coarsely_are_told_from_other_changes() {
    let at = |ns: u128| Identity {
        size: 10,
        modified_ns: ns,
        prefix_hash: "ab".into(),
    };
    let original = at(1_600_000_001_234_567_891);
    for coarse in [
        1_600_000_001_234_567_800, // NTFS
        1_600_000_001_230_000_000, // exFAT
        1_600_000_001_000_000_000, // HFS+
        1_600_000_000_000_000_000, // FAT, down
        1_600_000_002_000_000_000, // FAT, up
    ] {
        assert!(super::copy::coarser(&original, &at(coarse)), "{coarse}");
    }
    for other in [
        1_600_000_001_234_567_892, // not a coarser time
        1_600_000_004_000_000_000, // too far
        1_700_000_000_000_000_000, // now, as a volume ignoring times sets
    ] {
        assert!(!super::copy::coarser(&original, &at(other)), "{other}");
    }
    let mut bigger = at(1_600_000_001_000_000_000);
    bigger.size = 11;
    assert!(!super::copy::coarser(&original, &bigger));
}

/// Plans the move of A.NEF and b.NEF to far/, runs it on `disk` (which
/// may crash) with `trash`, restarts and recovers on the same volumes.
fn scenario(fx: &mut Fx, trash: &dyn Trash, disk: &Faulty) -> (BatchId, Recovery) {
    scenario_of(fx, &["A.NEF", "b.NEF"], trash, disk, Far::Ids)
}

/// [`scenario`] moving the photos `names` alone.
fn scenario_of(
    fx: &mut Fx,
    names: &[&str],
    trash: &dyn Trash,
    disk: &Faulty,
    kind: Far,
) -> (BatchId, Recovery) {
    let plan = fx.plan_on(to_far(fx, names), disk).unwrap();
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

/// After any fault and recovery: no original lost (`check`), each item
/// done or nothing changed (the catalog following only proven copies), no
/// partial copy left unless an item is unfinished, and whatever is
/// unfinished put back in the end.
fn proven(fx: &mut Fx, was: &Before, batch: &BatchId, recovery: &Recovery, what: &str, kind: Far) {
    check(fx, was, recovery, batch);
    copies_where_they_say(fx, was, batch, what);
    if recovery.unfinished.is_empty() {
        assert!(partials(fx).is_empty(), "{what}: {:?}", partials(fx));
    }
    for item in super::outcome(&fx.cat, batch).unwrap().items {
        assert!(
            matches!(
                item.result,
                ItemResult::Moved { .. } | ItemResult::Unchanged(_) | ItemResult::NeedsRecovery(_)
            ),
            "{what}: {item:?}"
        );
    }
    let clean = far_as(fx, kind);
    let trash = trash_as(fx, kind);
    *trash.calls.borrow_mut() = 100;
    for u in &recovery.unfinished {
        put_back(fx, u.operation, &clean, &trash, what, kind);
    }
    assert!(partials(fx).is_empty(), "{what}");
    copies_where_they_say(fx, was, batch, what);
}

/// Puts unfinished item `operation` back with `disk` and `trash`, `far/`
/// on a volume of `kind`: it is unchanged then; on a share, it may say its
/// copies are left there, and is unchanged once the user deletes them
/// (#675). Nothing is ever removed from a share.
pub(super) fn put_back(
    fx: &mut Fx,
    operation: OperationId,
    disk: &Faulty,
    trash: &FakeTrash,
    what: &str,
    kind: Far,
) {
    let back = |fx: &mut Fx| {
        recover::resolve_with(&mut fx.cat, operation, Resolution::PutBack, disk, trash)
            .unwrap()
            .result
    };
    let left = super::copy::LEFT_ON_SHARE.to_string();
    let result = match back(fx) {
        ItemResult::NeedsRecovery(why) if kind == Far::Share && why == left => {
            deleted_by_the_user(fx, operation);
            back(fx)
        }
        result => result,
    };
    assert!(
        matches!(result, ItemResult::Unchanged(_)),
        "{what}: {result:?}"
    );
    if kind == Far::Share {
        assert_eq!(removed_far(fx, disk), [] as [PathBuf; 0], "{what}");
    }
}

/// What item `operation` left on a share, deleted as its user would: each
/// of its copies, leaving the folders it made, which recovery removes
/// once empty.
pub(super) fn deleted_by_the_user(fx: &Fx, operation: OperationId) {
    let item = journal::item(&fx.cat.db, operation).unwrap().unwrap();
    for m in &item.members {
        let made = [
            m.destination.clone(),
            m.copy.as_ref().map(|c| c.temp.clone()),
        ];
        for at in made.into_iter().flatten() {
            if at.is_file() {
                std::fs::remove_file(&at).unwrap();
            }
        }
    }
}

/// The files in `far/` that `disk` removed.
pub(super) fn removed_far(fx: &Fx, disk: &Faulty) -> Vec<PathBuf> {
    let far = fx.root.join("far");
    disk.calls
        .borrow()
        .iter()
        .filter_map(|c| match c {
            super::tests::Call::RemoveFile(p) if p.starts_with(&far) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

/// Each copy item of `batch` as its outcome says: moved, with every copy
/// at its destination and every original in the Trash or reported kept;
/// unchanged, with no copy left outside the Trash.
fn copies_where_they_say(fx: &Fx, was: &Before, batch: &BatchId, what: &str) {
    let base = fx.dir.path().canonicalize().unwrap();
    let rel = |p: &Path| {
        p.strip_prefix(&base)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };
    let now = fx.files();
    let outcome = super::outcome(&fx.cat, batch).unwrap();
    for (item, journalled) in outcome
        .items
        .iter()
        .zip(journal::of_batch(&fx.cat.db, &batch.0).unwrap())
    {
        for m in &journalled.members {
            let (source, at) = (rel(&m.source), rel(m.destination.as_deref().unwrap()));
            let original = &was.files[&source];
            match &item.result {
                ItemResult::Moved { .. } => {
                    assert_eq!(now.get(&at), Some(original), "{what}: {at}");
                    let kept = item.originals_kept.iter().any(|k| k.path == m.source);
                    assert_eq!(now.contains_key(&source), kept, "{what}: {source}");
                }
                ItemResult::Unchanged(_) => {
                    assert_eq!(now.get(&source), Some(original), "{what}: {source}");
                    assert_eq!(now.get(&at), was.files.get(&at), "{what}: {at}");
                }
                _ => {}
            }
        }
    }
}

#[test]
fn every_fault_at_every_step_of_a_copy_ends_proven() {
    assert!(every_fault(Far::Ids) >= 100);
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the fault matrix by names is the same code everywhere; Linux and macOS run it, keeping Windows CI within its hour"
)]
fn every_fault_at_every_step_of_a_copy_to_exfat_ends_proven() {
    assert!(every_fault(Far::Names) >= 80);
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the fault matrix by names is the same code everywhere; Linux and macOS run it, keeping Windows CI within its hour"
)]
fn every_fault_at_every_step_of_a_copy_made_at_its_name_ends_proven() {
    assert!(every_fault(Far::Direct) >= 60);
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the fault matrix by names is the same code everywhere; Linux and macOS run it, keeping Windows CI within its hour"
)]
fn every_fault_at_every_step_of_a_copy_to_an_smb_share_ends_proven() {
    assert!(every_fault(Far::Share) >= 60);
}

/// Every fault at every step of moving A.NEF and b.NEF to `far/` as `kind`,
/// then recovery: each ends proven. Returns how many runs it made.
fn every_fault(kind: Far) -> usize {
    let started = std::time::Instant::now();
    // A run without faults says how many steps there are.
    let mut clean = files();
    let counting = far_as(&clean, kind);
    let trash = trash_as(&clean, kind);
    let op = to_far(&clean, &["A.NEF", "b.NEF"]);
    let outcome = run(&mut clean, op, &trash, &counting, Cancel::NEVER);
    assert!(
        results(&outcome)
            .iter()
            .all(|r| matches!(r, ItemResult::Moved { .. })),
        "{kind:?}: {outcome:?}"
    );
    assert_eq!(counting.count(Op::Copy), 3);
    // And one undoing a complete copy: a full drive at its companion's.
    let mut undone = files();
    let failing = Faulty {
        faults: vec![(Op::Copy, 1, Fault::Fail)],
        ..far_as(&undone, kind)
    };
    let trash = trash_as(&undone, kind);
    let op = to_far(&undone, &["A.NEF", "b.NEF"]);
    let _ = run(&mut undone, op, &trash, &failing, Cancel::NEVER);
    let mut runs = 0;
    for op in Op::COPYING {
        // On exFAT, faults while undoing are run with an undoing run only for
        // the steps undoing takes; the others' steps before the fault are the
        // clean run's, as for ids. Kept so to keep CI's time in check.
        let undoing = kind == Far::Ids || matches!(op, Op::RemoveFile | Op::Sync | Op::Hash);
        let count = counting.count(op).max(failing.count(op));
        for nth in 0..count {
            for fault in [
                Fault::Fail,
                Fault::CrashBefore,
                Fault::CrashAfter,
                Fault::CrashMid,
            ] {
                // A crash before a call is one after the call before it but
                // for the journal between, which the ids run covers.
                if kind != Far::Ids && fault == Fault::CrashBefore && nth > 0 {
                    continue;
                }
                for undo in [false, true] {
                    if undo && !undoing {
                        continue;
                    }
                    let mut fx = files();
                    let was = before(&fx);
                    let mut faults = vec![(op, nth, fault)];
                    if undo && op != Op::Copy {
                        faults.push((Op::Copy, 1, Fault::Fail));
                    }
                    let disk = Faulty {
                        faults,
                        ..far_as(&fx, kind)
                    };
                    let trash = trash_as(&fx, kind);
                    let (batch, recovery) =
                        scenario_of(&mut fx, &["A.NEF", "b.NEF"], &trash, &disk, kind);
                    let what = format!("{kind:?} {op:?} {nth} {fault:?} undo {undo}");
                    if kind == Far::Share {
                        assert_eq!(removed_far(&fx, &disk), [] as [PathBuf; 0], "{what}");
                    }
                    proven(&mut fx, &was, &batch, &recovery, &what, kind);
                    runs += 1;
                }
            }
        }
    }
    // The Trash failing at, or crashing after, each file.
    for n in 1..=4 {
        for crash in [false, true] {
            let mut fx = files();
            let was = before(&fx);
            let disk = far_as(&fx, kind);
            let mut trash = trash_as(&fx, kind);
            match crash {
                true => trash.crash_after = Some(n),
                false => trash.fail_from = Some(n),
            }
            let (batch, recovery) = scenario_of(&mut fx, &["A.NEF", "b.NEF"], &trash, &disk, kind);
            let what = format!("{kind:?} trash {n} {crash}");
            proven(&mut fx, &was, &batch, &recovery, &what, kind);
            runs += 1;
        }
    }
    eprintln!("{kind:?}: {runs} runs in {:?}", started.elapsed());
    runs
}

#[test]
fn every_failed_journal_write_of_a_copy_ends_proven() -> Result<()> {
    for when in [
        "BEFORE INSERT ON file_operation_copies",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'copying'",
        "BEFORE UPDATE ON file_operation_copies WHEN NEW.content IS NOT NULL",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'copied'",
        "BEFORE UPDATE ON photos",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'committed'",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'trashed'",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'discarded'",
    ] {
        let mut fx = files();
        let was = before(&fx);
        fx.fail_journal(when);
        let disk = far(&fx);
        let trash = FakeTrash::new(&fx);
        let plan = fx.plan_on(to_far(&fx, &["A.NEF", "b.NEF"]), &disk)?;
        let batch = plan.batch().clone();
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
        // A restart: the trigger is gone with the connection.
        fx.reopen();
        let recovery = fx.recover_with(&far(&fx), &trash);
        assert!(recovery.unfinished.is_empty(), "{when}: {recovery:?}");
        proven(&mut fx, &was, &batch, &recovery, when, Far::Ids);
    }
    Ok(())
}

#[test]
fn a_crash_after_the_catalog_follows_is_finished_by_recovery() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("A.NEF");
    let disk = far(&fx);
    let mut trash = FakeTrash::new(&fx);
    // After the photo's original went, before its receipt was journalled.
    // A alone: which of several photos goes first follows the order the
    // folder was listed in, which differs between systems.
    trash.crash_after = Some(1);
    let (batch, recovery) = scenario_of(&mut fx, &["A.NEF"], &trash, &disk, Far::Ids);
    assert!(recovery.unfinished.is_empty());
    let items = journal::of_batch(&fx.cat.db, &batch.0)?;
    assert_eq!(items[0].state, ItemState::Done);
    assert_eq!(fx.rows()[&a], "far/A.NEF");
    let files = fx.files();
    assert_eq!(files.get("trash/1-A.NEF"), Some(&bytes("A.NEF")));
    assert_eq!(files.get("trip/far/A.NEF"), Some(&bytes("A.NEF")));
    assert!(!files.contains_key("trip/A.NEF") && !files.contains_key("trip/A.xmp"));
    // Its companion went after the restart, with a receipt.
    let xmp = &items[0].members[1];
    assert_eq!(
        (xmp.state, xmp.receipt.is_some()),
        (MemberState::Trashed, true)
    );
    Ok(())
}

#[test]
fn an_original_changed_after_the_catalog_followed_is_kept() -> Result<()> {
    let mut fx = files();
    let disk = far(&fx);
    let mut trash = FakeTrash::new(&fx);
    trash.crash_after = Some(1);
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    // Its companion changed before the catalog opened again; and the copy
    // of another file there changed too.
    std::fs::write(fx.root.join("A.xmp"), b"rewritten")?;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert!(recovery.unfinished.is_empty());
    let outcome = super::outcome(&fx.cat, &batch)?;
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    let kept: Vec<_> = outcome.items[0]
        .originals_kept
        .iter()
        .map(|k| (k.path.clone(), k.why.clone()))
        .collect();
    assert_eq!(
        kept,
        [(
            fx.root.join("A.xmp"),
            "it changed since it was copied".to_string()
        )]
    );
    assert_eq!(std::fs::read(fx.root.join("A.xmp"))?, b"rewritten");
    assert_eq!(std::fs::read(fx.root.join("far/A.xmp"))?, bytes("A.xmp"));
    Ok(())
}

#[test]
fn an_original_whose_copy_changed_is_kept() -> Result<()> {
    let mut fx = files();
    let disk = far(&fx);
    let mut trash = FakeTrash::new(&fx);
    trash.crash_after = Some(1);
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    // The copy of the companion is changed in place, keeping its size.
    let mut changed = bytes("A.xmp");
    let last = changed.len() - 1;
    changed[last] ^= 1;
    let copy = fx.root.join("far/A.xmp");
    let modified = std::fs::metadata(&copy)?.modified()?;
    std::fs::write(&copy, &changed)?;
    std::fs::File::options()
        .write(true)
        .open(&copy)?
        .set_modified(modified)?;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert!(recovery.unfinished.is_empty());
    let outcome = super::outcome(&fx.cat, &batch)?;
    let kept: Vec<_> = outcome.items[0]
        .originals_kept
        .iter()
        .map(|k| k.path.clone())
        .collect();
    assert_eq!(kept, [fx.root.join("A.xmp")]);
    assert_eq!(
        std::fs::read(fx.root.join("A.xmp"))?,
        bytes("A.xmp"),
        "the original stays"
    );
    Ok(())
}

#[test]
fn a_crash_part_way_through_a_copy_leaves_a_partial_copy_recovery_removes() -> Result<()> {
    for fault in [Fault::CrashMid, Fault::CrashAfter] {
        let mut fx = files();
        let rows = fx.rows();
        let disk = Faulty {
            elsewhere: Some(fx.root.join("far")),
            ..Faulty::at(Op::Copy, 0, fault)
        };
        let trash = FakeTrash::new(&fx);
        let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
        let batch = plan.batch().clone();
        assert!(crashes(|| {
            let _ =
                execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
        }));
        fx.reopen();
        assert_eq!(partials(&fx).len(), 1, "{fault:?}");
        let recovery = fx.recover_with(&far(&fx), &trash);
        assert!(recovery.unfinished.is_empty());
        assert_eq!(
            results(&super::outcome(&fx.cat, &batch)?),
            [ItemResult::Unchanged(Some("it was interrupted".into()))]
        );
        assert!(partials(&fx).is_empty(), "removed outright, not trashed");
        assert!(!fx.files().keys().any(|k| k.starts_with("trash/")));
        assert_eq!(fx.rows(), rows);
    }
    Ok(())
}

#[test]
fn a_partial_copy_made_before_it_was_journalled_is_found_by_its_name() -> Result<()> {
    let mut fx = files();
    let disk = Faulty {
        elsewhere: Some(fx.root.join("far")),
        ..Faulty::at(Op::CreateFile, 0, Fault::CrashAfter)
    };
    let trash = FakeTrash::new(&fx);
    let (batch, recovery) = scenario(&mut fx, &trash, &disk);
    assert!(recovery.unfinished.is_empty());
    let items = journal::of_batch(&fx.cat.db, &batch.0)?;
    let member = &items[0].members[0];
    assert!(member.copy.as_ref().is_some_and(|c| c.id.is_none()));
    assert!(partials(&fx).is_empty());
    // One with bytes in it at that name isn't ours to remove.
    let mut fx = files();
    let disk = Faulty {
        elsewhere: Some(fx.root.join("far")),
        ..Faulty::at(Op::CreateFile, 0, Fault::CrashAfter)
    };
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
    assert!(crashes(|| {
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    let [temp] = &partials(&fx)[..] else {
        panic!("one partial copy")
    };
    let temp = fx.dir.path().canonicalize()?.join(temp);
    std::fs::write(&temp, b"someone else's")?;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert_eq!(recovery.unfinished.len(), 1);
    assert_eq!(std::fs::read(&temp)?, b"someone else's");
    Ok(())
}

#[test]
fn an_original_gone_before_recovery_keeps_its_copy_for_the_user() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("A.NEF");
    // Both files copied; the process ends before the catalog follows.
    let disk = Faulty {
        elsewhere: Some(fx.root.join("far")),
        ..Faulty::at(Op::Sync, 1, Fault::CrashAfter)
    };
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
    assert!(crashes(|| {
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    assert!(fx.root.join("far/A.NEF").is_file());
    // Gone, by whatever means, before the catalog opens again.
    std::fs::rename(fx.root.join("A.NEF"), fx.dir.path().join("elsewhere.NEF"))?;
    let recovery = fx.recover_with(&far(&fx), &trash);
    let [u] = &recovery.unfinished[..] else {
        panic!("{recovery:?}")
    };
    assert_eq!(u.choices, [Resolution::PutBack, Resolution::Keep]);
    // Put Back can't trash the copy, the only one there is now.
    let clean = far(&fx);
    let back = recover::resolve_with(
        &mut fx.cat,
        u.operation,
        Resolution::PutBack,
        &clean,
        &trash,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let ItemResult::NeedsRecovery(why) = &back.result else {
        panic!("{back:?}")
    };
    assert!(why.contains("an original isn't where it was"), "{why}");
    assert_eq!(std::fs::read(fx.root.join("far/A.NEF"))?, bytes("A.NEF"));
    let kept = recover::resolve_with(&mut fx.cat, u.operation, Resolution::Keep, &clean, &trash)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(kept.result, ItemResult::Kept);
    assert_eq!(fx.rows()[&a], "A.NEF", "Synchronize Folder can match it");
    assert_eq!(std::fs::read(fx.root.join("far/A.NEF"))?, bytes("A.NEF"));
    Ok(())
}

#[test]
fn a_whole_partial_copy_stays_while_its_original_is_gone() -> Result<()> {
    let mut fx = files();
    // Copied and verified, not yet given its name.
    let disk = Faulty {
        elsewhere: Some(fx.root.join("far")),
        ..Faulty::at(Op::Rename, 0, Fault::CrashBefore)
    };
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
    assert!(crashes(|| {
        let _ = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    std::fs::rename(fx.root.join("A.NEF"), fx.dir.path().join("elsewhere.NEF"))?;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert_eq!(recovery.unfinished.len(), 1);
    let [temp] = &partials(&fx)[..] else {
        panic!("{:?}", partials(&fx))
    };
    assert_eq!(fx.files()[temp], bytes("A.NEF"), "kept whole");
    // Back where it was, the item is undone.
    std::fs::rename(fx.dir.path().join("elsewhere.NEF"), fx.root.join("A.NEF"))?;
    let u = recovery.unfinished[0].operation;
    let clean = far(&fx);
    let back = recover::resolve_with(&mut fx.cat, u, Resolution::PutBack, &clean, &trash)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(matches!(back.result, ItemResult::Unchanged(_)), "{back:?}");
    assert!(partials(&fx).is_empty());
    Ok(())
}

#[test]
fn a_new_folder_on_another_drive_takes_copies() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("A.NEF");
    let root = fx.cat.folders()?[0].root;
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let op = FileOp::CreateFolder {
        root,
        parent: "far".into(),
        name: "new".into(),
        then_move: vec![a],
    };
    let plan = fx.plan_on(op, &disk)?;
    assert!(plan.summary().across.is_some());
    let outcome = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk)?;
    assert!(matches!(
        &results(&outcome)[..],
        [ItemResult::Created { .. }, ItemResult::Moved { .. }]
    ));
    assert_eq!(fx.rows()[&a], "far/new/A.NEF");
    assert_eq!(
        std::fs::read(fx.root.join("far/new/A.NEF"))?,
        bytes("A.NEF")
    );
    assert!(partials(&fx).is_empty());
    Ok(())
}

/// The master photo whose name is `name`, composed.
fn named(fx: &Fx, name: &str) -> Result<PhotoId> {
    use unicode_normalization::UnicodeNormalization;
    let photos = fx.cat.photos()?;
    let found = photos
        .iter()
        .find(|p| p.master.is_none() && p.filename.nfc().collect::<String>() == name);
    Ok(found.unwrap_or_else(|| panic!("no photo {name}")).id)
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_OTHER_VOLUME: a writable folder on another volume"]
fn a_move_between_two_real_volumes() -> Result<()> {
    let Some(other) = std::env::var_os("RAWMAKASE_TEST_OTHER_VOLUME") else {
        return Ok(());
    };
    // And names FAT and exFAT on macOS list decomposed: one stored
    // composed, one stored decomposed.
    let mut fx = fixture(&["a.NEF", "a.xmp", CAFE, "Ru\u{308}gen.NEF"]);
    let there = tempfile::tempdir_in(&other)?;
    let dir = there.path().canonicalize()?;
    assert_ne!(
        fsops::entry(&dir)?.id.volume(),
        fsops::entry(&fx.root)?.id.volume(),
        "{} is on the fixture's volume",
        dir.display()
    );
    std::fs::write(dir.join("keep.dng"), bytes("keep.dng"))?;
    fx.cat.add_folder(&dir)?;
    let to = fx
        .cat
        .folders()?
        .into_iter()
        .find(|f| f.path == dir)
        .expect("the other volume's folder")
        .id;
    let a = fx.photo("a.NEF");
    let source = fx.root.join("a.NEF");
    let tagged = super::tests::keeps_extras(&dir);
    if tagged {
        fsops::set_extra(&source, "tag", b"Red")?;
    }
    let extras = fsops::extras(&source)?;
    let (identity, modified) = (
        Identity::read(&source)?,
        std::fs::metadata(&source)?.modified()?,
    );
    // Found composed: Add Folder keeps the name as the drive lists it.
    let cafe = named(&fx, CAFE)?;
    let op = FileOp::MovePhotos {
        photos: vec![a, cafe, named(&fx, "R\u{fc}gen.NEF")?],
        root: fx.root_of(to),
        to: fx.logical(to),
    };
    let plan = fx.plan(op);
    let across = plan.summary().across.clone().expect("it copies");
    assert!(dir.starts_with(&across.volume), "{across:?}");
    let trash = FakeTrash::new(&fx);
    let outcome = execute::execute(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER)?;
    assert_eq!(results(&outcome), vec![ItemResult::Moved { to }; 3]);
    assert!(
        outcome.items.iter().all(|i| i.originals_kept.is_empty()),
        "{outcome:?}"
    );
    assert_eq!(std::fs::read(dir.join(CAFE))?, bytes(CAFE));
    assert!(!fx.root.join(CAFE).exists());
    let target = dir.join("a.NEF");
    assert_eq!(std::fs::read(&target)?, bytes("a.NEF"));
    assert_eq!(fsops::extras(&target)?, extras, "tagged {tagged}");
    assert_eq!(std::fs::read(dir.join("a.xmp"))?, bytes("a.xmp"));
    let copied = Identity::read(&target)?;
    assert!(
        copied == identity || super::copy::coarser(&identity, &copied),
        "{copied:?} {identity:?}"
    );
    let kept = std::fs::metadata(&target)?.modified()?;
    let off = kept
        .duration_since(modified)
        .or_else(|_| modified.duration_since(kept))?;
    assert!(off < std::time::Duration::from_secs(2), "{off:?}");
    assert!(!source.exists());
    let files = fx.files();
    assert!(
        files
            .iter()
            .any(|(k, v)| k.starts_with("trash/") && k.ends_with("-a.NEF") && *v == bytes("a.NEF")),
        "{:?}",
        files.keys()
    );
    let leftovers: Vec<_> = std::fs::read_dir(&dir)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<_>>()?;
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().ends_with(".rawmakase-part")),
        "{leftovers:?}"
    );
    drop(fx);
    // exFAT on macOS removes a file only by its name composed, which
    // `remove_dir_all` doesn't give.
    for name in std::fs::read_dir(&dir)?.map(|e| e.map(|e| e.file_name())) {
        let name: String =
            unicode_normalization::UnicodeNormalization::nfc(name?.to_string_lossy().as_ref())
                .collect();
        // AppleDouble `._` files go with the file they are of.
        match std::fs::remove_file(dir.join(name)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            other => other?,
        }
    }
    there.close()?;
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
fn a_move_to_exfat_proves_its_copies_by_names_and_journals_no_file_id() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("A.NEF");
    let disk = far_as(&fx, Far::Names);
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
    let batch = plan.batch().clone();
    let outcome = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk)?;
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    assert_eq!(fx.rows()[&a], "far/A.NEF");
    assert_eq!(std::fs::read(fx.root.join("far/A.NEF"))?, bytes("A.NEF"));
    assert_eq!(std::fs::read(fx.root.join("far/A.xmp"))?, bytes("A.xmp"));
    assert!(!fx.root.join("A.NEF").exists());
    // A release before #573 finds no file object it is: one no file has.
    let ids = copy_ids(&fx, &batch);
    assert!(
        !ids.is_empty() && ids.iter().all(|i| i.as_deref() == Some("0:0")),
        "{ids:?}"
    );
    // Its partial copies were renamed into place.
    assert!(!disk.renames().is_empty());
    assert!(partials(&fx).is_empty());
    Ok(())
}

#[test]
fn a_move_to_a_drive_without_an_exclusive_rename_makes_each_copy_at_its_name() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("A.NEF");
    let disk = far_as(&fx, Far::Direct);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx, &["A.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    assert_eq!(fx.rows()[&a], "far/A.NEF");
    assert_eq!(std::fs::read(fx.root.join("far/A.NEF"))?, bytes("A.NEF"));
    assert!(disk.renames().is_empty(), "{:?}", disk.renames());
    let made: Vec<PathBuf> = disk
        .calls
        .borrow()
        .iter()
        .filter_map(|c| match c {
            super::tests::Call::CreateFile(p) => Some(p.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(made, [fx.root.join("far/A.NEF"), fx.root.join("far/A.xmp")]);
    assert!(fx.files().contains_key("trash/1-A.NEF"));
    Ok(())
}

/// A share's listing lags a file another computer made (measured on an
/// SMB share: 2 to 3 s), while looking a name up finds it: a copy left out
/// of the listing is never taken for gone.
#[test]
fn a_copy_its_folder_doesn_t_list_yet_is_never_taken_for_gone() -> Result<()> {
    let mut fx = files();
    let was = before(&fx);
    // A crash once A.NEF is copied whole, its companion part way.
    let disk = Faulty {
        faults: vec![(Op::Copy, 1, Fault::CrashMid)],
        ..far_as(&fx, Far::Direct)
    };
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
    let batch = plan.batch().clone();
    crashes(|| {
        drop(
            execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk)
                .unwrap(),
        );
    });
    fx.reopen();
    let copy = fx.root.join("far/A.NEF");
    assert_eq!(std::fs::read(&copy)?, bytes("A.NEF"));
    let lagging = Faulty {
        unlisted: vec![copy.clone()],
        ..far_as(&fx, Far::Direct)
    };
    let recovery = fx.recover_with(&lagging, &trash);
    check(&fx, &was, &recovery, &batch);
    assert_eq!(recovery.unfinished.len(), 1, "{recovery:?}");
    assert_eq!(std::fs::read(&copy)?, bytes("A.NEF"));
    assert_eq!(std::fs::read(fx.root.join("A.NEF"))?, bytes("A.NEF"));
    Ok(())
}

#[test]
fn a_move_to_an_smb_share_is_proven_by_names() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("A.NEF");
    let disk = far_as(&fx, Far::Share);
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
    if let Some(across) = &plan.summary().across {
        // Its originals, on this drive, go to its Trash.
        assert_eq!((across.files, across.originals_stay), (1, 0));
    }
    let outcome = execute::execute_with(&mut fx.cat, plan.confirm(), &trash, Cancel::NEVER, &disk)?;
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    assert_eq!(fx.rows()[&a], "far/A.NEF");
    assert_eq!(std::fs::read(fx.root.join("far/A.NEF"))?, bytes("A.NEF"));
    assert_eq!(std::fs::read(fx.root.join("far/A.xmp"))?, bytes("A.xmp"));
    // Made at their names: the share can't rename exclusively.
    assert!(disk.renames().is_empty(), "{:?}", disk.renames());
    assert_eq!(fx.files().get("trash/1-A.NEF"), Some(&bytes("A.NEF")));
    Ok(())
}

/// A share has no Trash (macOS's answers NSFeatureUnsupportedError on an
/// SMB share, measured; Linux's isn't used): its originals stay where they
/// were, beside their copies, which the catalog follows, and the outcome
/// says why. Nothing is deleted instead.
#[test]
fn a_move_from_an_smb_share_keeps_the_originals_its_trash_can_t_take() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("A.NEF");
    // The fixture's volume and `far/`'s both shares.
    let disk = Faulty {
        names_only: vec![fx.root.clone()],
        share: true,
        no_exclusive_rename: true,
        lists_decomposed: true,
        ..far(&fx)
    };
    let mut trash = FakeTrash::new(&fx);
    trash.fail_from = Some(1);
    let op = to_far(&fx, &["A.NEF"]);
    // The question says so before.
    let across = fx.plan_on(op.clone(), &disk)?.summary().across.clone();
    assert_eq!(across.map(|a| (a.files, a.originals_stay)), Some((1, 1)));
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    assert_eq!(fx.rows()[&a], "far/A.NEF");
    let why = TrashError::Unsupported.to_string();
    let kept: Vec<(PathBuf, String)> = outcome.items[0]
        .originals_kept
        .iter()
        .map(|k| (k.path.clone(), k.why.clone()))
        .collect();
    assert_eq!(
        kept,
        [
            (fx.root.join("A.NEF"), why.clone()),
            (fx.root.join("A.xmp"), why)
        ]
    );
    for name in ["A.NEF", "A.xmp"] {
        assert_eq!(std::fs::read(fx.root.join(name))?, bytes(name));
        assert_eq!(std::fs::read(fx.root.join("far").join(name))?, bytes(name));
    }
    Ok(())
}

#[test]
fn a_move_between_exfat_drives_proves_its_originals_by_names() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("A.NEF");
    // The fixture's volume and `far/`'s both exFAT.
    let disk = Faulty {
        names_only: vec![fx.root.clone()],
        renumber_on_close: true,
        ..far(&fx)
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx, &["A.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    assert_eq!(fx.rows()[&a], "far/A.NEF");
    assert_eq!(fx.files().get("trash/1-A.NEF"), Some(&bytes("A.NEF")));
    Ok(())
}

#[test]
fn an_original_on_exfat_changed_where_its_identity_doesn_t_look_is_kept() -> Result<()> {
    let mut fx = fixture(&["big.NEF", "far/"]);
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(fx.root.join("big.NEF"), &big)?;
    let disk = Faulty {
        names_only: vec![fx.root.clone()],
        ..far(&fx)
    };
    let mut trash = FakeTrash::new(&fx);
    trash.crash_before = Some(1);
    let op = FileOp::MovePhotos {
        photos: vec![fx.photo("big.NEF")],
        root: fx.root_id(),
        to: "far".into(),
    };
    let plan = fx.plan_on(op, &disk)?;
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
    // A byte past what its `Identity` reads changes, its time kept.
    let path = fx.root.join("big.NEF");
    let modified = std::fs::metadata(&path)?.modified()?;
    let mut changed = big.clone();
    changed[150_000] ^= 0xff;
    std::fs::write(&path, &changed)?;
    std::fs::File::options()
        .write(true)
        .open(&path)?
        .set_modified(modified)?;
    fx.reopen();
    trash.crash_before = None;
    let recovery = fx.recover_with(&disk, &trash);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let outcome = super::outcome(&fx.cat, &batch)?;
    let kept = &outcome.items[0].originals_kept;
    assert_eq!(kept.len(), 1, "{outcome:?}");
    assert_eq!(kept[0].why, "it changed since it was copied");
    assert_eq!(std::fs::read(&path)?, changed);
    assert_eq!(std::fs::read(fx.root.join("far/big.NEF"))?, big);
    Ok(())
}

#[test]
fn an_empty_file_at_the_name_made_before_the_move_was_planned_is_left() -> Result<()> {
    let mut fx = files();
    let disk = Faulty {
        faults: vec![(Op::CreateFile, 0, Fault::CrashAfter)],
        ..far_as(&fx, Far::Direct)
    };
    let trash = FakeTrash::new(&fx);
    let (_, recovery) = scenario_of(&mut fx, &["A.NEF"], &trash, &disk, Far::Direct);
    // Made after planning, it is the copy begun: removed.
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    assert!(!fx.root.join("far/A.NEF").exists());
    // One the volume says was made long before is never taken for it.
    let mut fx = files();
    let disk = Faulty {
        faults: vec![(Op::CreateFile, 0, Fault::CrashAfter)],
        ..far_as(&fx, Far::Direct)
    };
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
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
    let old = Faulty {
        born_long_ago: vec![fx.root.join("far/A.NEF")],
        ..far_as(&fx, Far::Direct)
    };
    let recovery = fx.recover_with(&old, &trash);
    assert_eq!(recovery.unfinished.len(), 1, "{recovery:?}");
    assert!(fx.root.join("far/A.NEF").is_file());
    Ok(())
}

#[test]
fn a_name_taken_just_before_a_copy_is_made_at_it_is_left_as_it_is() -> Result<()> {
    for theirs in [&b""[..], b"theirs"] {
        let mut fx = files();
        let disk = Faulty {
            occupy_on_create: Some((fx.root.join("far/A.xmp"), theirs.to_vec())),
            ..far_as(&fx, Far::Direct)
        };
        let trash = FakeTrash::new(&fx);
        let op = to_far(&fx, &["A.NEF"]);
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
            panic!("{theirs:?}: {outcome:?}")
        };
        assert!(why.contains("A.xmp"), "{why}");
        assert_eq!(std::fs::read(fx.root.join("far/A.xmp"))?, theirs);
        // Its copy made before it, at its name, went to the Trash.
        assert!(!fx.root.join("far/A.NEF").exists());
        assert_eq!(std::fs::read(fx.root.join("A.NEF"))?, bytes("A.NEF"));
        fx.reopen();
        assert!(
            fx.recover_with(&far_as(&fx, Far::Direct), &trash)
                .unfinished
                .is_empty()
        );
    }
    Ok(())
}

/// A file another program or computer puts at a copy's name just after it
/// is made, before its file object is journalled, is never taken for it:
/// nothing is copied into it, and it is left as it is, through recovery
/// and Put Back: the move is listed for the user, as a name holding bytes
/// that can't be proven the copy's is (review of #573: the file object
/// was read from the name).
#[test]
fn a_file_put_at_a_copys_name_just_after_it_is_made_is_left_as_it_is() -> Result<()> {
    let mut kinds = Far::by_names();
    kinds.push(Far::Ids);
    for kind in kinds {
        let mut fx = files();
        let disk = Faulty {
            swap_after_create: Some(b"theirs".to_vec()),
            ..far_as(&fx, kind)
        };
        let trash = trash_as(&fx, kind);
        let op = to_far(&fx, &["A.NEF"]);
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        assert!(
            matches!(results(&outcome)[..], [ItemResult::NeedsRecovery(_)]),
            "{kind:?}: {outcome:?}"
        );
        let theirs = disk.swapped.borrow().clone().expect("a file was made");
        assert_eq!(std::fs::read(&theirs)?, b"theirs", "{kind:?}");
        for name in ["A.NEF", "A.xmp"] {
            assert_eq!(std::fs::read(fx.root.join(name))?, bytes(name), "{kind:?}");
        }
        fx.reopen();
        let fresh = far_as(&fx, kind);
        let recovery = fx.recover_with(&fresh, &trash);
        assert_eq!(recovery.unfinished.len(), 1, "{kind:?}");
        for u in &recovery.unfinished {
            let _ = recover::resolve_with(
                &mut fx.cat,
                u.operation,
                Resolution::PutBack,
                &fresh,
                &trash,
            );
        }
        assert_eq!(std::fs::read(&theirs)?, b"theirs", "{kind:?}");
        for name in ["A.NEF", "A.xmp"] {
            assert_eq!(std::fs::read(fx.root.join(name))?, bytes(name), "{kind:?}");
        }
    }
    Ok(())
}

/// `Café.NEF` with its sidecar, and `far/`.
fn accented() -> Fx {
    fixture(&[CAFE, "Caf\u{e9}.xmp", "far/"])
}

#[test]
fn an_accented_name_moves_to_exfat_whose_driver_lists_it_decomposed() -> Result<()> {
    for kind in Far::by_names() {
        let mut fx = accented();
        let photo = fx.photo(CAFE);
        let disk = far_as(&fx, kind);
        let trash = FakeTrash::new(&fx);
        let op = to_far(&fx, &[CAFE]);
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        assert_eq!(
            results(&outcome),
            [ItemResult::Moved {
                to: fx.folder("far")
            }],
            "{kind:?}"
        );
        assert_eq!(fx.rows()[&photo], format!("far/{CAFE}"), "{kind:?}");
        assert_eq!(std::fs::read(fx.root.join("far").join(CAFE))?, bytes(CAFE));
        assert!(!fx.root.join(CAFE).exists(), "{kind:?}");
    }
    Ok(())
}

#[test]
fn an_accented_name_moves_from_exfat_whose_driver_lists_it_decomposed() -> Result<()> {
    let mut fx = accented();
    let photo = fx.photo(CAFE);
    let disk = Faulty {
        names_only: vec![fx.root.clone()],
        lists_decomposed: true,
        ..far(&fx)
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx, &[CAFE]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    assert_eq!(fx.rows()[&photo], format!("far/{CAFE}"));
    assert!(outcome.items[0].originals_kept.is_empty(), "{outcome:?}");
    assert!(!fx.root.join(CAFE).exists());
    Ok(())
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the fault matrix by names is the same code everywhere; Linux and macOS run it, keeping Windows CI within its hour"
)]
fn every_fault_moving_an_accented_name_to_exfat_ends_proven() {
    let mut runs = 0;
    for kind in [Far::Names, Far::Direct] {
        let mut clean = accented();
        let counting = far_as(&clean, kind);
        let trash = FakeTrash::new(&clean);
        let op = to_far(&clean, &[CAFE]);
        let _ = run(&mut clean, op, &trash, &counting, Cancel::NEVER);
        for op in Op::COPYING {
            for nth in 0..counting.count(op) {
                for fault in [Fault::Fail, Fault::CrashAfter, Fault::CrashMid] {
                    if fault == Fault::CrashMid && op != Op::Copy {
                        continue;
                    }
                    let mut fx = accented();
                    let was = before(&fx);
                    let disk = Faulty {
                        faults: vec![(op, nth, fault)],
                        ..far_as(&fx, kind)
                    };
                    let trash = FakeTrash::new(&fx);
                    let (batch, recovery) = scenario_of(&mut fx, &[CAFE], &trash, &disk, kind);
                    let what = format!("{kind:?} {op:?} {nth} {fault:?}");
                    proven(&mut fx, &was, &batch, &recovery, &what, kind);
                    runs += 1;
                }
            }
        }
    }
    assert!(runs >= 40, "{runs}");
}

/// `Café.NEF` decomposed, as a file copied from elsewhere may be stored.
const CAFE_NFD: &str = "Cafe\u{301}.NEF";

#[test]
fn a_name_stored_decomposed_moves_to_and_from_fat_and_exfat() -> Result<()> {
    // To them: made there composed, the one spelling their drivers take
    // for what RAWmakase makes.
    for kind in Far::by_names() {
        let mut fx = fixture(&[CAFE_NFD, "far/"]);
        let photo = fx.photo(CAFE_NFD);
        let disk = far_as(&fx, kind);
        let trash = FakeTrash::new(&fx);
        let op = to_far(&fx, &[CAFE_NFD]);
        let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
        assert_eq!(
            results(&outcome),
            [ItemResult::Moved {
                to: fx.folder("far")
            }],
            "{kind:?}"
        );
        assert_eq!(fx.rows()[&photo], format!("far/{CAFE}"), "{kind:?}");
        assert!(
            super::tests::stored_spelling(&fx.root.join("far").join(CAFE)),
            "{kind:?}: stored composed"
        );
    }
    // From them: an original stored decomposed goes to the Trash by that
    // spelling.
    let mut fx = fixture(&[CAFE_NFD, "far/"]);
    let disk = Faulty {
        names_only: vec![fx.root.clone()],
        lists_decomposed: true,
        ..far(&fx)
    };
    let mut trash = FakeTrash::new(&fx);
    trash.stored_only = true;
    let op = to_far(&fx, &[CAFE_NFD]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    assert_eq!(
        results(&outcome),
        [ItemResult::Moved {
            to: fx.folder("far")
        }]
    );
    assert!(outcome.items[0].originals_kept.is_empty(), "{outcome:?}");
    assert!(!fx.root.join(CAFE_NFD).exists());
    Ok(())
}

#[test]
fn a_copy_made_at_its_name_that_a_photo_of_the_catalog_is_now_stays() -> Result<()> {
    let mut fx = files();
    let disk = Faulty {
        faults: vec![(Op::Copy, 0, Fault::CrashAfter)],
        ..far_as(&fx, Far::Direct)
    };
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx, &["A.NEF"]), &disk)?;
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            Cancel::NEVER,
            &disk,
        ));
    }));
    // Meanwhile the catalog takes the file at that name for a photo of its
    // own, as Synchronize Folder may.
    let root = fx.root.clone();
    fx.add_row(&root, "far", "A.NEF");
    fx.reopen();
    let recovery = fx.recover_with(&far_as(&fx, Far::Direct), &trash);
    assert_eq!(recovery.unfinished.len(), 1, "{recovery:?}");
    assert_eq!(std::fs::read(fx.root.join("far/A.NEF"))?, bytes("A.NEF"));
    Ok(())
}

#[test]
fn another_spelling_of_a_name_is_taken_only_for_the_same_file() -> Result<()> {
    // The Trash and a rename fall back to a name's decomposed spelling,
    // which macOS's FAT and exFAT take only so: never where that is another
    // file (#573).
    let dir = tempfile::tempdir()?;
    let composed = dir.path().join(CAFE);
    std::fs::write(&composed, b"original")?;
    let other = super::proof::other_spelling(&super::disk::Real, &composed);
    let decomposed = dir.path().join(CAFE_NFD);
    match decomposed.exists() {
        // Found by either spelling: the same file.
        true => assert_eq!(other, Some(decomposed)),
        // Found only by its bytes: another file of the other spelling is
        // never taken for it.
        false => {
            assert_eq!(other, None);
            std::fs::write(&decomposed, b"another")?;
            assert_eq!(
                super::proof::other_spelling(&super::disk::Real, &composed),
                None
            );
        }
    }
    Ok(())
}

/// A move onto a share that fails leaves its copies there, never removed:
/// removing one by its name, however it was proven, could remove a file
/// another computer put at that name just then, as the test disk does
/// (#675). The move is listed for the user, Put Back changes nothing, and
/// once the user deletes the copies it clears.
#[test]
fn a_move_onto_an_smb_share_that_fails_leaves_its_copies_there() -> Result<()> {
    let mut fx = files();
    let rows = fx.rows();
    // A full share at the sidecar's copy, A.NEF's whole already; another
    // computer puts a file at a copy's name right before it would go.
    let disk = Faulty {
        faults: vec![(Op::Copy, 1, Fault::Fail)],
        swap_before: Some((Op::RemoveFile, b"theirs".to_vec())),
        ..far_as(&fx, Far::Share)
    };
    let trash = trash_as(&fx, Far::Share);
    let op = to_far(&fx, &["A.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    // Nothing removed there: each copy as it was made.
    let theirs = disk.swapped.borrow().clone();
    assert_eq!(theirs.map(|p| std::fs::read(p).ok()), None, "{outcome:?}");
    for name in ["A.NEF", "A.xmp"] {
        assert_eq!(std::fs::read(fx.root.join("far").join(name))?, bytes(name));
        assert_eq!(std::fs::read(fx.root.join(name))?, bytes(name));
    }
    let [ItemResult::NeedsRecovery(why)] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert_eq!(why, super::copy::LEFT_ON_SHARE);
    assert_eq!(fx.rows(), rows);
    // Listed for the user; Put Back leaves the copies too.
    fx.reopen();
    let fresh = far_as(&fx, Far::Share);
    let recovery = fx.recover_with(&fresh, &trash);
    let [u] = &recovery.unfinished[..] else {
        panic!("{recovery:?}")
    };
    assert_eq!(u.reason, super::copy::LEFT_ON_SHARE);
    let back = recover::resolve_with(
        &mut fx.cat,
        u.operation,
        Resolution::PutBack,
        &fresh,
        &trash,
    )?;
    assert_eq!(
        back.result,
        ItemResult::NeedsRecovery(super::copy::LEFT_ON_SHARE.into())
    );
    for name in ["A.NEF", "A.xmp"] {
        assert_eq!(std::fs::read(fx.root.join("far").join(name))?, bytes(name));
    }
    // Deleted by the user, they are gone, and so is the item.
    for name in ["A.NEF", "A.xmp"] {
        std::fs::remove_file(fx.root.join("far").join(name))?;
    }
    assert!(fx.recover_with(&fresh, &trash).unfinished.is_empty());
    assert_eq!(fx.rows(), rows);
    Ok(())
}

/// A file another computer puts at a copy's name while it is given its
/// original's extras, time and permissions takes none of them where the
/// system keeps to the handle the copy was made through, as this test disk
/// does (#675); not Windows' alternate data streams, written by path, nor
/// a share's drivers (see `fsops::copy`).
/// The copy then reads back otherwise, and the move changes nothing.
#[test]
fn a_file_put_at_a_copys_name_while_it_is_finished_takes_nothing_of_it() -> Result<()> {
    let mut kinds = Far::by_names();
    kinds.push(Far::Ids);
    for kind in kinds {
        for step in [Op::SyncFile, Op::CopyExtras, Op::CopyTimes] {
            let what = format!("{kind:?} {step:?}");
            let mut fx = files();
            let rows = fx.rows();
            if !tagged(&fx.root.join("A.NEF"), "com.rawmakase.test", b"synthetic") {
                return Ok(());
            }
            let plain = fx.dir.path().join("plain");
            std::fs::write(&plain, b"")?;
            let disk = Faulty {
                swap_before: Some((step, b"theirs".to_vec())),
                ..far_as(&fx, kind)
            };
            let trash = trash_as(&fx, kind);
            let op = to_far(&fx, &["A.NEF"]);
            let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
            let theirs = disk
                .swapped
                .borrow()
                .clone()
                .expect("a file was swapped in");
            assert_eq!(std::fs::read(&theirs)?, b"theirs", "{what}");
            // Windows writes an alternate data stream by its path, which
            // also changes the time of a file swapped in before it.
            if !cfg!(windows) {
                assert_eq!(
                    fsops::extras(&theirs)?,
                    fsops::extras(&plain)?,
                    "{what}: its extras"
                );
            }
            if !cfg!(windows) || step == Op::CopyTimes {
                let dated = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
                assert_eq!(
                    std::fs::metadata(&theirs)?.modified()?,
                    dated,
                    "{what}: its time"
                );
            }
            assert!(
                matches!(
                    results(&outcome)[..],
                    [ItemResult::NeedsRecovery(_) | ItemResult::Unchanged(_)]
                ),
                "{what}: {outcome:?}"
            );
            assert_eq!(fx.rows(), rows, "{what}");
            for name in ["A.NEF", "A.xmp"] {
                assert_eq!(std::fs::read(fx.root.join(name))?, bytes(name), "{what}");
            }
        }
    }
    Ok(())
}

/// A copy left on a share is said to be the user's to delete only while
/// its original holds every byte of it: a whole copy kept because its
/// original changed meanwhile may hold the only bytes left, and that is
/// said over any other copy left there (review of #675).
#[test]
fn a_copy_left_on_a_share_with_the_only_bytes_isnt_said_to_be_deletable() -> Result<()> {
    let mut fx = files();
    let original = fx.root.join("A.NEF");
    // A.NEF is copied whole; once its sidecar's copy is made, another app
    // rewrites it, and the share fills.
    let watched = original;
    let (sidecar, copied) = (fx.root.join("far/A.xmp"), fx.root.join("far/A.NEF"));
    let disk = Faulty {
        faults: vec![(Op::Copy, 1, Fault::Fail)],
        watch: Some(Box::new(move |call: &super::tests::Call| {
            if matches!(call, super::tests::Call::Entry(at) if *at == sidecar) && copied.exists() {
                std::fs::write(&watched, b"rewritten").unwrap();
            }
        })),
        ..far_as(&fx, Far::Share)
    };
    let trash = trash_as(&fx, Far::Share);
    let op = to_far(&fx, &["A.NEF"]);
    let outcome = run(&mut fx, op, &trash, &disk, Cancel::NEVER);
    let [ItemResult::NeedsRecovery(why)] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert_ne!(why, super::copy::LEFT_ON_SHARE);
    assert_eq!(std::fs::read(fx.root.join("far/A.NEF"))?, bytes("A.NEF"));
    Ok(())
}

/// A partial copy left where its volume can't say what it is (so nothing
/// is removed, as on a share) is said to be the user's to delete only
/// while its original still begins with its bytes (review of #675).
#[test]
fn a_partial_copy_left_on_an_unknown_volume_isnt_said_deletable_once_its_original_changed()
-> Result<()> {
    let mut fx = files();
    let fail = Arc::new(AtomicBool::new(false));
    let (dir, original) = (fx.root.join("far"), fx.root.join("A.NEF"));
    let failing = fail.clone();
    // A full drive part way through A.NEF; as the copy is undone, another
    // app rewrites the original and the drive can't be told any more.
    let disk = Faulty {
        faults: vec![(Op::Copy, 0, Fault::Fail)],
        facts_fail: Some(fail.clone()),
        watch: Some(Box::new(move |call: &super::tests::Call| {
            if matches!(call, super::tests::Call::List(d) if *d == dir)
                && !failing.load(std::sync::atomic::Ordering::Relaxed)
                && std::fs::read_dir(&dir).unwrap().any(|e| {
                    e.unwrap()
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".rawmakase-part")
                })
            {
                std::fs::write(&original, b"rewritten").unwrap();
                failing.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        })),
        ..far(&fx)
    };
    let op = to_far(&fx, &["A.NEF"]);
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
