//! Move to Trash in the app: Delete asking whether to trash the files or
//! only remove the photos, ⌘⌥⇧Delete, Delete Rejected Photos, and a volume
//! with no Trash; driven by keys and frames, with a Trash that moves files
//! into a temporary folder.
use super::tests::{frame, ids_left, select, settle, tap};
use super::*;
use crate::app::Editor;
use crate::app::testing::editor_with_catalog;
use crate::catalog::PhotoId;
use eframe::egui::{Key, Modifiers};
use std::path::PathBuf;

/// A Trash that moves files into `dir`, or has none.
struct TestTrash {
    dir: Option<PathBuf>,
}

impl Trash for TestTrash {
    fn trash(&self, path: &Path) -> Result<TrashReceipt, TrashError> {
        let Some(dir) = &self.dir else {
            return Err(TrashError::Unsupported);
        };
        let location = dir.join(path.file_name().unwrap());
        std::fs::rename(path, &location).map_err(|e| TrashError::Failed(e.to_string()))?;
        Ok(TrashReceipt { location })
    }
}

/// An editor on a catalog of `names`, whose Trash is `trash/` beside it.
fn editor(names: &[&str]) -> anyhow::Result<(tempfile::TempDir, Editor, PathBuf)> {
    let (d, mut e, _) = editor_with_catalog(names)?;
    let trash = d.path().join("trash");
    std::fs::create_dir(&trash)?;
    e.file_ops.trash = Arc::new(TestTrash {
        dir: Some(trash.clone()),
    });
    Ok((d, e, trash))
}

fn id(e: &Editor, name: &str) -> PhotoId {
    e.library
        .as_ref()
        .unwrap()
        .session
        .photos()
        .iter()
        .find(|p| p.filename == name)
        .unwrap_or_else(|| panic!("no photo {name}"))
        .id
}

const TRASH_KEYS: Modifiers = Modifiers {
    alt: true,
    shift: true,
    ..Modifiers::COMMAND
};

#[test]
fn delete_offers_the_trash_and_the_catalog_and_tab_twice_trashes() -> anyhow::Result<()> {
    let (d, mut e, trash) = editor(&["a.RAF", "b.RAF", "c.RAF"])?;
    std::fs::write(d.path().join("photos/a.RAF.xmp"), b"sidecar")?;
    let (a, b) = (id(&e, "a.RAF"), id(&e, "b.RAF"));
    select(&mut e, &[a, b]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(
        title,
        "Move the 2 selected photos to the Trash, or just remove them from the catalog?"
    );
    assert_eq!(actions, ["Remove from Catalog", "Move to Trash"]);
    assert!(
        lines
            .iter()
            .any(|l| l == "2 files (and 1 sidecar file) go to the Trash."),
        "{lines:?}"
    );
    // Return alone cancels.
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert_eq!(ids_left(&e).len(), 3);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(trash.join("a.RAF").is_file() && trash.join("b.RAF").is_file());
    assert!(
        trash.join("a.RAF.xmp").is_file(),
        "its sidecar goes with it"
    );
    assert!(d.path().join("photos/c.RAF").is_file());
    assert_eq!(ids_left(&e), vec![id(&e, "c.RAF")]);
    assert_eq!(e.messages.line(), "Moved 2 photos to the Trash");
    Ok(())
}

#[test]
fn delete_then_tab_once_only_removes_from_the_catalog() -> anyhow::Result<()> {
    let (d, mut e, trash) = editor(&["a.RAF", "b.RAF"])?;
    let a = id(&e, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    let (title, ..) = e.file_ops.question().expect("it asks");
    assert_eq!(
        title,
        "Move “a.RAF” to the Trash, or just remove it from the catalog?"
    );
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/a.RAF").is_file(), "the file stays");
    assert!(!trash.join("a.RAF").exists());
    assert_eq!(ids_left(&e), vec![id(&e, "b.RAF")]);
    assert_eq!(e.messages.line(), "Removed 1 photo from the catalog");
    Ok(())
}

#[test]
fn a_missing_file_can_only_be_removed_from_the_catalog() -> anyhow::Result<()> {
    let (d, mut e, _trash) = editor(&["a.RAF", "b.RAF"])?;
    std::fs::remove_file(d.path().join("photos/a.RAF"))?;
    let a = id(&e, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    let (title, _, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Remove “a.RAF” from the catalog?");
    assert_eq!(actions, ["Remove from Catalog"]);
    Ok(())
}

#[test]
fn command_option_shift_delete_asks_only_to_trash() -> anyhow::Result<()> {
    let (d, mut e, trash) = editor(&["a.RAF", "b.RAF"])?;
    let a = id(&e, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, TRASH_KEYS);
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move “a.RAF” to the Trash?");
    assert_eq!(actions, ["Move to Trash"]);
    assert!(
        lines.iter().any(|l| l.contains("1 file goes to the Trash")),
        "{lines:?}"
    );
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(trash.join("a.RAF").is_file());
    assert!(!d.path().join("photos/a.RAF").exists());
    assert_eq!(ids_left(&e), vec![id(&e, "b.RAF")]);
    Ok(())
}

#[test]
fn edits_are_said_to_be_lost_with_a_trashed_photo() -> anyhow::Result<()> {
    let (_d, mut e, _trash) = editor(&["a.RAF"])?;
    let a = id(&e, "a.RAF");
    let library = e.library.as_mut().unwrap();
    let edit = crate::app::Recipe {
        exposure: 1.,
        ..Default::default()
    };
    let path = library.photo(a).unwrap().path.clone();
    library.session.save_edit(
        a,
        &path,
        &crate::storage::Identity::read(&path)?,
        &edit,
        &Default::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, TRASH_KEYS);
    settle(&mut e);
    let (_, lines, _) = e.file_ops.question().expect("it asks");
    assert!(
        lines.iter().any(|l| l
            == "1 photo has edits in RAWmakase; they are removed with it. Putting the file back from the Trash brings back the file, not its edits."),
        "{lines:?}"
    );
    Ok(())
}

#[test]
fn delete_rejected_photos_asks_for_the_rejected_ones_in_the_source() -> anyhow::Result<()> {
    let (_d, mut e, trash) = editor(&["a.RAF", "b.RAF", "c.RAF"])?;
    let (a, b, c) = (id(&e, "a.RAF"), id(&e, "b.RAF"), id(&e, "c.RAF"));
    for photo in [a, c] {
        select(&mut e, &[photo]);
        tap(&mut e, Key::X, Modifiers::NONE);
    }
    // The selection is b: Delete Rejected asks for a and c, whatever is
    // selected.
    select(&mut e, &[b]);
    tap(&mut e, Key::Delete, Modifiers::COMMAND);
    settle(&mut e);
    let (title, ..) = e.file_ops.question().expect("it asks");
    assert_eq!(
        title,
        "Move the 2 rejected photos to the Trash, or just remove them from the catalog?"
    );
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(trash.join("a.RAF").is_file() && trash.join("c.RAF").is_file());
    assert_eq!(ids_left(&e), vec![b]);
    Ok(())
}

#[test]
fn delete_rejected_with_none_rejected_says_so() -> anyhow::Result<()> {
    let (_d, mut e, _trash) = editor(&["a.RAF"])?;
    tap(&mut e, Key::Delete, Modifiers::COMMAND);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert_eq!(e.messages.line(), "No rejected photos here");
    Ok(())
}

#[test]
fn a_volume_with_no_trash_keeps_the_file_and_the_photo() -> anyhow::Result<()> {
    let (d, mut e, _trash) = editor(&["a.RAF", "b.RAF"])?;
    e.file_ops.trash = Arc::new(TestTrash { dir: None });
    let a = id(&e, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::Delete, TRASH_KEYS);
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/a.RAF").is_file());
    assert_eq!(ids_left(&e).len(), 2);
    assert!(
        e.messages.line().contains("has no Trash")
            && e.messages.line().contains(&format!(
                "Delete it in {} if you're sure",
                crate::platform::reveal::MANAGER
            )),
        "{}",
        e.messages.line()
    );
    Ok(())
}

#[test]
fn the_open_photo_trashed_in_develop_is_closed() -> anyhow::Result<()> {
    let (_d, mut e, trash) = editor(&["a.RAF", "b.RAF"])?;
    let a = id(&e, "a.RAF");
    let path = e.library.as_ref().unwrap().photo(a).unwrap().path.clone();
    e.module = crate::app::Module::Develop;
    e.document.catalog_photo = Some(a);
    e.document.opened_at(path);
    frame(&mut e, Vec::new());
    tap(&mut e, Key::Delete, TRASH_KEYS);
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(trash.join("a.RAF").is_file());
    assert_eq!(e.document.catalog_photo, None);
    Ok(())
}

#[test]
fn delete_says_what_each_choice_takes_when_some_files_cant_go() -> anyhow::Result<()> {
    let (d, mut e, _trash) = editor(&["a.RAF", "b.RAF"])?;
    let (a, b) = (id(&e, "a.RAF"), id(&e, "b.RAF"));
    // b has an edit, and its file is missing.
    let library = e.library.as_mut().unwrap();
    let edit = crate::app::Recipe {
        exposure: 1.,
        ..Default::default()
    };
    let path = library.photo(b).unwrap().path.clone();
    library.session.save_edit(
        b,
        &path,
        &crate::storage::Identity::read(&path)?,
        &edit,
        &Default::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    std::fs::remove_file(d.path().join("photos/b.RAF"))?;
    select(&mut e, &[a, b]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(
        title,
        "Move the 2 selected photos to the Trash, or just remove them from the catalog?"
    );
    assert_eq!(actions, ["Remove from Catalog", "Move to Trash"]);
    assert!(
        lines.iter().any(|l| l == "1 file goes to the Trash."),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l
            .starts_with("1 photo can only be removed from the catalog; Move to Trash leaves it:")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("1 photo has edits in RAWmakase")),
        "the edit removed either way is said: {lines:?}"
    );
    Ok(())
}
