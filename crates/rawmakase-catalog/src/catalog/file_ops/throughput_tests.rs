//! A large batch's cost (#692): renaming or moving photos within a drive
//! commits the catalog, syncs a folder and lists one for a batch of
//! photos at a time, not for each, so many photos take seconds, not
//! minutes. Counted rather than timed, so it holds on any drive; the
//! time itself is `tests/file_ops_throughput.rs`'s.
use super::tests::{Faulty, Fx, NoTrash, Op, fixture};
use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The photos of the batch, every tenth with an XMP sidecar.
const PHOTOS: usize = 250;

fn large() -> Fx {
    let mut files = Vec::new();
    for i in 0..PHOTOS {
        files.push(format!("IMG_{i:04}.NEF"));
        if i % 10 == 0 {
            files.push(format!("IMG_{i:04}.xmp"));
        }
    }
    files.push("sub/".into());
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    fixture(&files)
}

fn masters(fx: &Fx) -> Vec<PhotoId> {
    (0..PHOTOS)
        .map(|i| fx.photo(&format!("IMG_{i:04}.NEF")))
        .collect()
}

/// What running `op` on `fx` cost: the catalog's commits, and the
/// folders synced and listed.
#[derive(Debug)]
struct Cost {
    commits: usize,
    syncs: usize,
    lists: usize,
}

fn run(fx: &mut Fx, op: FileOp) -> (Outcome, Cost) {
    let commits = Arc::new(AtomicUsize::new(0));
    let counted = commits.clone();
    fx.cat
        .db_mut_for_tests()
        .commit_hook(Some(move || {
            counted.fetch_add(1, Ordering::Relaxed);
            false
        }))
        .unwrap();
    let disk = Faulty::default();
    let outcome = fx.run(op, &NoTrash, &disk);
    fx.cat
        .db_mut_for_tests()
        .commit_hook(None::<fn() -> bool>)
        .unwrap();
    let cost = Cost {
        commits: commits.load(Ordering::Relaxed),
        syncs: disk.count(Op::Sync),
        lists: disk.count(Op::List),
    };
    (outcome, cost)
}

/// At most one of each for every ten photos.
fn cheap(cost: &Cost) -> bool {
    [cost.commits, cost.syncs, cost.lists]
        .iter()
        .all(|n| n * 10 <= PHOTOS)
}

#[test]
fn renaming_a_large_batch_commits_syncs_and_lists_per_batch_not_per_photo() {
    let mut fx = large();
    let renames = masters(&fx)
        .into_iter()
        .enumerate()
        .map(|(i, p)| (p, format!("Trip {i:04}")))
        .collect();
    let (outcome, cost) = run(&mut fx, FileOp::RenamePhotos { renames });
    assert!(outcome.stopped.is_none(), "{:?}", outcome.stopped);
    let renamed = outcome
        .items
        .iter()
        .filter(|i| matches!(i.result, ItemResult::Renamed { .. }))
        .count();
    assert_eq!(renamed, PHOTOS);
    assert!(fx.root.join("Trip 0010.NEF").is_file());
    assert!(fx.root.join("Trip 0010.xmp").is_file());
    assert!(!fx.root.join("IMG_0010.xmp").exists());
    assert_eq!(fx.rows()[&fx.photo("Trip 0249.NEF")], "Trip 0249.NEF");
    assert!(cheap(&cost), "{cost:?}");
}

#[test]
fn moving_a_large_batch_commits_syncs_and_lists_per_batch_not_per_photo() {
    let mut fx = large();
    let photos = masters(&fx);
    let root = fx.root_id();
    let (outcome, cost) = run(
        &mut fx,
        FileOp::MovePhotos {
            photos,
            root,
            to: "sub".into(),
        },
    );
    assert!(outcome.stopped.is_none(), "{:?}", outcome.stopped);
    let sub = fx.folder("sub");
    let moved = outcome
        .items
        .iter()
        .filter(|i| i.result == ItemResult::Moved { to: sub })
        .count();
    assert_eq!(moved, PHOTOS);
    assert!(fx.root.join("sub/IMG_0010.xmp").is_file());
    assert_eq!(fx.rows()[&fx.photo("sub/IMG_0249.NEF")], "sub/IMG_0249.NEF");
    assert!(cheap(&cost), "{cost:?}");
}

#[test]
fn a_folders_photos_read_once_follow_the_transactions_own_moves() -> anyhow::Result<()> {
    use super::plan::{Folders, occupied, shared};
    use crate::catalog::db::sql;
    use crate::catalog::folder_sync::locate_folders;
    use crate::fsops;
    use std::ffi::OsStr;
    let mut fx = fixture(&["a.NEF", "b.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    let copy = fx.cat.create_virtual_copy(a)?;
    let b = fx.photo("b.NEF");
    let sub = fx.folder("sub");
    let computer = fx.cat.computer.id.clone();
    let folders = Folders::index(&fx.cat.db.snapshot(|s| locate_folders(s, &computer))?);
    let (root, below) = (fx.root.clone(), fx.root.join("sub"));
    let b_file = fsops::entry(&root.join("b.NEF"))?.id;
    // Every question `shared` and `occupied` are asked of both folders.
    let ask = |w: &crate::catalog::db::Write<'_>| -> anyhow::Result<Vec<bool>> {
        let mut answers = Vec::new();
        for name in ["a.NEF", "A.nef", "b.NEF", "c.NEF", "C.nef", "keep.dng"] {
            let name = OsStr::new(name);
            for family in [&[][..], &[a, copy][..], &[b][..]] {
                for (dir, to) in [(&root, None), (&below, None), (&root, Some(sub))] {
                    answers.push(occupied(w, &folders, to, dir, name, family)?);
                }
                for dir in [&root, &below] {
                    answers.push(shared(w, &folders, dir, name, None, family)?);
                    answers.push(shared(w, &folders, dir, name, Some(b_file), family)?);
                }
            }
        }
        Ok(answers)
    };
    let (read, kept) = fx.cat.db.write_immediate(|w| {
        folders.caching(|| {
            let read = ask(w)?;
            // As `moved` moves a family: to `sub`, renamed `c.NEF`.
            w.execute(
                sql!("UPDATE photos SET folder=?, filename=? WHERE id=? OR master_id=?"),
                &[&sub, &"c.NEF", &a, &a],
            )?;
            folders.refresh(w, a)?;
            Ok((read, ask(w)?))
        })
    })?;
    // Asked afresh, outside the transaction's reads.
    let fresh = fx.cat.db.write_immediate(|w| ask(w))?;
    assert_eq!(kept, fresh);
    assert_ne!(read, fresh, "the move changes some answers");
    Ok(())
}
