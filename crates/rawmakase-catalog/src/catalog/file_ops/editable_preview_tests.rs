//! A photo's editable preview leaves with its rows, by Remove from Catalog or
//! Move to Trash, in the same transaction, through the cascade; its file is
//! then one no row names, for the caller to delete after the commit. A
//! removal that fails keeps both. Moves and renames leave it as it is.
use super::tests::*;
use super::*;
use crate::catalog::editable_preview_file_name;
use crate::storage::Identity;
use anyhow::Result;
use std::path::PathBuf;

/// Records an editable preview of `photo`, whose file is `name` below the
/// root, with a file standing for the copy.
fn record(fx: &mut Fx, photo: PhotoId, name: &str) -> PathBuf {
    let original = Identity::read(&fx.root.join(name)).unwrap();
    let file = editable_preview_file_name(photo, &original);
    let folder = fx.cat.editable_previews_folder();
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(&file), b"editable preview").unwrap();
    fx.cat.record_editable_preview(photo, &original).unwrap();
    folder.join(file)
}

#[test]
fn remove_from_catalog_removes_the_editable_preview() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "b.NEF"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    let (pa, pb) = (record(&mut fx, a, "a.NEF"), record(&mut fx, b, "b.NEF"));
    let outcome = fx.run(
        FileOp::RemoveFromCatalog { photos: vec![a] },
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::Removed]);
    assert_eq!(fx.cat.editable_preview(a)?, None);
    assert_eq!(fx.cat.editable_preview_orphans()?, [pa]);
    assert!(fx.cat.editable_preview(b)?.is_some() && pb.is_file());
    assert!(fx.root.join("a.NEF").is_file(), "the original stays");
    Ok(())
}

#[test]
fn move_to_trash_removes_the_editable_preview_with_the_rows() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "b.NEF"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    let copy = fx.cat.create_virtual_copy(a)?;
    let (pa, _) = (record(&mut fx, a, "a.NEF"), record(&mut fx, b, "b.NEF"));
    let trash = FakeTrash::new(&fx);
    let outcome = fx.run(
        FileOp::TrashPhotos { photos: vec![a] },
        &trash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [ItemResult::Trashed]);
    assert_eq!(fx.cat.editable_preview(a)?, None);
    assert_eq!(fx.cat.editable_preview(copy)?, None);
    assert_eq!(fx.cat.editable_preview_orphans()?, [pa]);
    assert!(fx.cat.editable_preview(b)?.is_some());
    Ok(())
}

#[test]
fn a_trash_whose_rows_could_not_go_keeps_the_editable_preview_until_they_do() -> Result<()> {
    let mut fx = fixture(&["a.NEF"]);
    let a = fx.photo("a.NEF");
    let pa = record(&mut fx, a, "a.NEF");
    let trash = FakeTrash::new(&fx);
    fx.fail_journal("BEFORE DELETE ON editable_previews");
    let outcome = fx.run(
        FileOp::TrashPhotos { photos: vec![a] },
        &trash,
        &Faulty::default(),
    );
    assert_ne!(results(&outcome), [ItemResult::Trashed], "{outcome:?}");
    // The file went to the Trash, but the rows are still there, and so is
    // what they name.
    assert!(fx.rows().contains_key(&a));
    assert!(fx.cat.editable_preview(a)?.is_some());
    assert!(fx.cat.editable_preview_orphans()?.is_empty());
    // Finished on the next start.
    fx.reopen();
    fx.recover_with(&Faulty::default(), &trash);
    assert!(!fx.rows().contains_key(&a));
    assert_eq!(fx.cat.editable_preview(a)?, None);
    assert_eq!(fx.cat.editable_preview_orphans()?, [pa]);
    Ok(())
}

#[test]
fn a_folder_in_the_trash_takes_its_photos_editable_previews() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "other/c.NEF"]);
    let (a, c) = (fx.photo("day/a.NEF"), fx.photo("other/c.NEF"));
    let (pa, pc) = (
        record(&mut fx, a, "day/a.NEF"),
        record(&mut fx, c, "other/c.NEF"),
    );
    let trash = FakeTrash::new(&fx);
    let plan = plan::plan_trash_with(
        &fx.cat,
        FileOp::TrashFolder {
            root: fx.root_id(),
            folder: "day".into(),
        },
        &Faulty::default(),
        &trash,
    )
    .unwrap();
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &trash,
        rawmakase_cancel::Cancel::NEVER,
        &Faulty::default(),
    )
    .unwrap();
    assert_eq!(results(&outcome), [ItemResult::Trashed], "{outcome:?}");
    assert_eq!(fx.cat.editable_preview(a)?, None);
    assert_eq!(fx.cat.editable_preview_orphans()?, [pa]);
    assert!(fx.cat.editable_preview(c)?.is_some() && pc.is_file());
    Ok(())
}

#[test]
fn a_moved_or_renamed_photo_keeps_its_editable_preview() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "sub/"]);
    let a = fx.photo("a.NEF");
    let pa = record(&mut fx, a, "a.NEF");
    let was = fx.cat.editable_preview(a)?;
    let to = fx.folder("sub");
    let moved = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_of(to),
            to: fx.logical(to),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(matches!(results(&moved)[..], [ItemResult::Moved { .. }]));
    let renamed = fx.run(
        FileOp::RenamePhotos {
            renames: vec![(a, "renamed".into())],
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(matches!(
        results(&renamed)[..],
        [ItemResult::Renamed { .. }]
    ));
    assert_eq!(fx.rows()[&a], "sub/renamed.NEF");
    assert_eq!(fx.cat.editable_preview(a)?, was);
    assert!(pa.is_file());
    assert!(fx.cat.editable_preview_orphans()?.is_empty());
    Ok(())
}
