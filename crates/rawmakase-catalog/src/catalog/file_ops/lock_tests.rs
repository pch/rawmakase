//! The operation lock.
use super::tests::{Faulty, NoTrash, fixture};
use super::*;
use crate::catalog::Catalog;
use anyhow::Result;
use rawmakase_cancel::Cancel;

#[test]
fn a_held_lock_keeps_another_window_from_changing_or_recovering_files() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    // Another process, as far as the lock goes: another open file.
    let other = Catalog::open(&fx.catalog)?;
    let held = lock::acquire(&other).map_err(|e| anyhow::anyhow!("{e}"))?;
    let plan = fx.plan(FileOp::MovePhotos {
        photos: vec![a],
        root: fx.root_id(),
        to: "sub".into(),
    });
    let run = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    );
    assert!(matches!(run, Err(FileOpError::Busy)));
    assert!(matches!(
        recover(&mut fx.cat, &NoTrash),
        Err(FileOpError::Busy)
    ));
    assert!(fx.root.join("a.NEF").is_file());
    drop(held);
    assert!(recover(&mut fx.cat, &NoTrash).is_ok());
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_catalog_opened_through_a_link_takes_the_same_lock() -> Result<()> {
    let fx = fixture(&["a.NEF"]);
    let link = fx.dir.path().join("Link.rawmakase");
    std::os::unix::fs::symlink(&fx.catalog, &link)?;
    let through_link = Catalog::open(&link)?;
    let held = lock::acquire(&fx.cat).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(matches!(
        lock::acquire(&through_link),
        Err(FileOpError::Busy)
    ));
    drop(held);
    assert!(lock::acquire(&through_link).is_ok());
    // The lock file is named after the real path, and stays.
    assert!(lock::lock_path(&fx.catalog.canonicalize()?).is_file());
    Ok(())
}

#[test]
fn a_hard_linked_catalog_is_refused() -> Result<()> {
    let fx = fixture(&["a.NEF"]);
    std::fs::hard_link(&fx.catalog, fx.dir.path().join("Other.rawmakase"))?;
    assert!(matches!(
        lock::acquire(&fx.cat),
        Err(FileOpError::Unsupported(_))
    ));
    Ok(())
}

// Windows refuses to rename a file another handle has open, so an open
// catalog can't be replaced there.
#[cfg(unix)]
#[test]
fn a_catalog_replaced_since_it_opened_is_refused() -> Result<()> {
    let fx = fixture(&["a.NEF"]);
    let moved = fx.dir.path().join("Moved.rawmakase");
    std::fs::rename(&fx.catalog, &moved)?;
    std::fs::copy(&moved, &fx.catalog)?;
    assert!(matches!(
        lock::acquire(&fx.cat),
        Err(FileOpError::Unsupported(_))
    ));
    Ok(())
}

#[test]
fn remove_from_catalog_needs_no_lock_as_it_touches_no_file() -> Result<()> {
    // A hard-linked catalog can't take the lock, and another window may
    // hold it: removing rows works all the same.
    let mut fx = fixture(&["a.NEF", "b.NEF"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    std::fs::hard_link(&fx.catalog, fx.dir.path().join("Other.rawmakase"))?;
    let outcome = fx.run(
        FileOp::RemoveFromCatalog { photos: vec![a] },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(outcome.items[0].result, ItemResult::Removed);
    assert!(!fx.rows().contains_key(&a) && fx.rows().contains_key(&b));
    Ok(())
}

#[test]
fn remove_from_catalog_leaves_photos_an_unfinished_operation_involves() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "b.NEF", "sub/"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    // A move of a whose completion failed: unfinished.
    fx.fail_journal("BEFORE UPDATE ON photos");
    let moved = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_id(),
            to: "sub".into(),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(matches!(
        moved.items[0].result,
        ItemResult::NeedsRecovery(_)
    ));
    fx.reopen();
    // Another window holds the lock, as a window recovering it would.
    let other = Catalog::open(&fx.catalog)?;
    let _held = lock::acquire(&other).map_err(|e| anyhow::anyhow!("{e}"))?;
    let outcome = fx.run(
        FileOp::RemoveFromCatalog { photos: vec![a, b] },
        &NoTrash,
        &Faulty::default(),
    );
    let of = |id| {
        outcome
            .items
            .iter()
            .find(|i| i.photos == [id])
            .map(|i| i.result.clone())
    };
    assert!(matches!(of(a), Some(ItemResult::Unchanged(Some(_)))));
    assert_eq!(of(b), Some(ItemResult::Removed));
    assert!(fx.rows().contains_key(&a));
    Ok(())
}
