//! Move to Trash… for a folder of the Folders panel (#575): the menu offers
//! it for any folder but a root, the question counts what goes with Cancel
//! focused, and the outcome is applied; an empty root can be removed from
//! the catalog; Delete never acts on a folder. Driven by keys and frames on
//! a catalog of synthetic files, with a Trash that moves them into a
//! temporary folder.
use super::move_tests::{editor_with_folders, folder, photo, root};
use super::tests::{select, settle, tap};
use super::*;
use crate::app::Editor;
use eframe::egui::{Key, Modifiers};
use std::path::PathBuf;

/// A Trash that moves what it takes into `dir`.
struct TestTrash {
    dir: PathBuf,
}

impl Trash for TestTrash {
    fn trash(&self, path: &Path) -> Result<TrashReceipt, TrashError> {
        let location = self.dir.join(path.file_name().unwrap());
        std::fs::rename(path, &location).map_err(|e| TrashError::Failed(e.to_string()))?;
        Ok(TrashReceipt { location })
    }
}

/// An editor on a catalog of `names` below `photos/`, whose Trash is
/// `trash/` beside it.
fn editor(names: &[&str]) -> anyhow::Result<(tempfile::TempDir, Editor, PathBuf)> {
    let (d, mut e, _) = editor_with_folders(names)?;
    let trash = d.path().join("trash");
    std::fs::create_dir(&trash)?;
    e.file_ops.trash = Arc::new(TestTrash { dir: trash.clone() });
    Ok((d, e, trash))
}

/// Whether the temporary folder's drive proves file ids, which a folder
/// Trash needs (#573): overlayfs in CI's Linux containers doesn't, and is
/// refused as any such drive is. The engine's tests cover that refusal and
/// every step on a test disk; these real-disk tests say so and end there.
fn ids_proven(d: &tempfile::TempDir) -> bool {
    let proven = rawmakase_fs::volume_facts(d.path()).is_ok_and(|f| f.stable_ids);
    if !proven {
        eprintln!("{}: file ids not proven here", d.path().display());
    }
    proven
}

/// Asks to put the folder at `logical` in the Trash.
fn ask(e: &mut Editor, logical: &str) {
    let r = root(e);
    e.request_folder_trash(r, logical.into());
    settle(e);
}

#[test]
fn the_question_counts_what_goes_and_return_alone_cancels() -> anyhow::Result<()> {
    let (d, mut e, trash) = editor(&[
        "keep.RAF",
        "trip/a.RAF",
        "trip/a.xmp",
        "trip/notes.txt",
        "trip/sub/b.RAF",
    ])?;
    if !ids_proven(&d) {
        return Ok(());
    }
    std::fs::create_dir(d.path().join("photos/trip/empty"))?;
    ask(&mut e, "trip");
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(
        title,
        "Move folder “trip” and its 2 folders, 2 photos and 2 other files to the Trash?"
    );
    assert_eq!(actions, ["Move to Trash"]);
    assert!(
        lines.iter().any(|l| l == "Other files: a.xmp, notes.txt."),
        "{lines:?}"
    );
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/trip/a.RAF").is_file());
    assert!(std::fs::read_dir(&trash)?.next().is_none());
    Ok(())
}

#[test]
fn confirmed_the_folder_goes_and_the_library_leaves_it() -> anyhow::Result<()> {
    let (d, mut e, trash) = editor(&["keep.RAF", "trip/a.RAF", "trip/sub/b.RAF"])?;
    if !ids_proven(&d) {
        return Ok(());
    }
    let (a, keep) = (photo(&e, &d, "trip/a.RAF"), photo(&e, &d, "keep.RAF"));
    let r = root(&e);
    // The Library shows the folder put in the Trash.
    e.library
        .as_mut()
        .unwrap()
        .click_folder_for_tests(&format!("root:{r}/trip/sub"));
    ask(&mut e, "trip");
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert_eq!(
        e.messages.line(),
        "Moved folder trip to the Trash with 2 photos"
    );
    assert!(trash.join("trip/sub/b.RAF").is_file());
    assert!(!d.path().join("photos/trip").exists());
    let library = e.library.as_ref().unwrap();
    assert!(library.photo(a).is_none());
    assert!(library.photo(keep).is_some());
    assert_eq!(folder(&e, "trip"), None);
    assert_eq!(
        e.library.as_ref().unwrap().source_key(),
        format!("root:{r}"),
        "the selection goes to the folder it was in"
    );
    Ok(())
}

#[test]
fn an_empty_folder_and_a_changed_one_say_so() -> anyhow::Result<()> {
    let (d, mut e, _trash) = editor(&["keep.RAF", "trip/a.RAF", "gone/x.RAF"])?;
    if !ids_proven(&d) {
        return Ok(());
    }
    // Its photo's file gone: an empty folder in the catalog's eyes.
    std::fs::remove_file(d.path().join("photos/gone/x.RAF"))?;
    ask(&mut e, "gone");
    let (title, lines, _) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move empty folder “gone” to the Trash?");
    assert!(
        lines.iter().any(|l| l
            == "1 photo in the catalog whose file is missing is removed from the catalog too."),
        "{lines:?}"
    );
    tap(&mut e, Key::Escape, Modifiers::NONE);
    settle(&mut e);
    // Changed after the question: nothing goes.
    ask(&mut e, "trip");
    std::fs::write(d.path().join("photos/trip/late.RAF"), b"late")?;
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert_eq!(
        e.messages.line(),
        "Folder trip not moved to the Trash: late.RAF was added to it since you were asked."
    );
    assert!(d.path().join("photos/trip/a.RAF").is_file());
    Ok(())
}

#[test]
fn a_root_is_refused_and_an_emptied_root_can_be_removed() -> anyhow::Result<()> {
    let (d, mut e, _trash) = editor(&["2024/day1/a.RAF"])?;
    let r = root(&e);
    e.request_folder_trash(r, String::new());
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert!(
        e.messages
            .line()
            .contains("A root folder can't be moved to the Trash"),
        "{}",
        e.messages.line()
    );
    if !ids_proven(&d) {
        return Ok(());
    }
    ask(&mut e, "2024");
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!d.path().join("photos/2024").exists());
    // The root stays, with nothing in the catalog, and goes when asked.
    assert_eq!(e.library.as_ref().unwrap().session.roots().len(), 1);
    e.remove_folders(&crate::app::library::FolderRemoval {
        catalog: e
            .library
            .as_ref()
            .unwrap()
            .session
            .catalog()
            .location()
            .clone(),
        name: "photos".into(),
        folders: Default::default(),
        photos: 0,
        empty_root: Some(r),
        exported: None,
    });
    assert!(e.library.as_ref().unwrap().session.roots().is_empty());
    assert!(d.path().join("photos").is_dir(), "its folder stays");
    assert!(
        e.messages
            .line()
            .contains("Removed photos from the catalog"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

#[test]
fn delete_after_clicking_a_folder_asks_about_the_selected_photos_never_the_folder()
-> anyhow::Result<()> {
    let (d, mut e, _trash) = editor(&["keep.RAF", "trip/a.RAF", "trip/b.RAF"])?;
    let a = photo(&e, &d, "trip/a.RAF");
    let r = root(&e);
    e.library
        .as_mut()
        .unwrap()
        .click_folder_for_tests(&format!("root:{r}/trip"));
    select(&mut e, &[a]);
    for key in [Key::Delete, Key::Backspace] {
        tap(&mut e, key, Modifiers::NONE);
        settle(&mut e);
        let (title, ..) = e.file_ops.question().expect("it asks");
        assert_eq!(
            title,
            "Move “a.RAF” to the Trash, or just remove it from the catalog?"
        );
        tap(&mut e, Key::Escape, Modifiers::NONE);
        settle(&mut e);
    }
    assert!(d.path().join("photos/trip").is_dir());
    Ok(())
}

#[test]
fn a_folder_with_no_photos_leaves_the_panel_and_says_so() -> anyhow::Result<()> {
    let (d, mut e, trash) = editor(&["keep.RAF", "trip/a.RAF"])?;
    if !ids_proven(&d) {
        return Ok(());
    }
    // A catalog folder with no photo rows left (its only photo removed by
    // another window): the outcome names no photos.
    let trip = folder(&e, "trip").expect("in the catalog");
    let db = rusqlite::Connection::open(d.path().join("test.rawmakase"))?;
    db.execute("DELETE FROM photos WHERE folder=?1", [trip.0])?;
    drop(db);
    ask(&mut e, "trip");
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert_eq!(e.messages.line(), "Moved folder trip to the Trash");
    assert!(trash.join("trip/a.RAF").is_file());
    assert_eq!(folder(&e, "trip"), None, "read again");
    Ok(())
}
