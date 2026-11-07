//! Moving photos to a folder and New Folder Inside in the app, and what a
//! crash left unfinished when a catalog opens: driven by frames and keys as
//! a person would, on a catalog of synthetic files in a temporary folder.
use super::tests::{frame, ids_left, select, settle, tap};
use super::*;
use crate::app::{Editor, Module};
use crate::catalog::{FolderId, PhotoId};
use eframe::egui::{self, Key, Modifiers};
use std::path::{Path, PathBuf};

/// An editor on a catalog of `photos/` holding `names` (paths below it).
pub(super) fn editor_with_folders(
    names: &[&str],
) -> anyhow::Result<(tempfile::TempDir, Editor, Vec<PhotoId>)> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    for name in names {
        let path = photos.join(name);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, format!("fixture {name}"))?;
    }
    let path = d.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let library = crate::app::library::Library::load(&path, ctx)?;
    let ids = library.session.photos().iter().map(|p| p.id).collect();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    Ok((d, e, ids))
}

/// The photo whose file is `name` (below `photos/`).
pub(super) fn photo(e: &Editor, d: &tempfile::TempDir, name: &str) -> PhotoId {
    let path = d.path().join("photos").join(name);
    let library = e.library.as_ref().unwrap();
    library
        .session
        .photos()
        .iter()
        .find(|p| p.path == path || p.path.canonicalize().ok() == path.canonicalize().ok())
        .unwrap_or_else(|| panic!("no photo {name}"))
        .id
}

/// Where the catalog says photo `id`'s file is, below `photos/`.
pub(super) fn at(e: &Editor, d: &tempfile::TempDir, id: PhotoId) -> String {
    let library = e.library.as_ref().unwrap();
    let path = &library.photo(id).unwrap().path;
    let base = d.path().join("photos");
    let base = path
        .strip_prefix(&base)
        .or_else(|_| path.strip_prefix(base.canonicalize().unwrap()))
        .unwrap();
    base.to_string_lossy().replace('\\', "/")
}

/// The catalog folder whose logical path is `relative`.
pub(super) fn folder(e: &Editor, relative: &str) -> Option<FolderId> {
    let library = e.library.as_ref().unwrap();
    library
        .session
        .folders()
        .iter()
        .find(|f| f.relative == relative)
        .map(|f| f.id)
}

pub(super) fn root(e: &Editor) -> crate::catalog::RootId {
    e.library.as_ref().unwrap().session.roots()[0].0
}

#[test]
fn moving_asks_with_its_counts_and_the_files_rows_and_open_photo_follow() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "a.xmp", "b.RAF", "sub/x.RAF"])?;
    let (a, b) = (photo(&e, &d, "a.RAF"), photo(&e, &d, "b.RAF"));
    // The open photo, decoded in full, with an edit saved before anything
    // moves.
    e.module = Module::Develop;
    e.document.catalog_photo = Some(a);
    e.document.opened_at(d.path().join("photos/a.RAF"));
    e.document.set_image(decoded(false));
    e.request_photo_move(vec![a, b], root(&e), "sub".into(), "sub".into());
    settle(&mut e);
    let summary = e.file_ops.summary().expect("the confirmation is open");
    assert_eq!((summary.files, summary.companions), (2, 1));
    let (title, lines, action) = e.file_ops.question().unwrap();
    assert_eq!(title, "Move 2 photos to “sub”?");
    assert!(
        lines
            .iter()
            .any(|l| l == "2 files (and 1 sidecar file) will be moved on disk.")
    );
    assert_eq!(action, ["Move"]);
    // Return alone cancels: nothing moved.
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming() && !e.activity.is_busy());
    assert!(d.path().join("photos/a.RAF").is_file());
    // Tab to Move, then Return.
    e.request_photo_move(vec![a, b], root(&e), "sub".into(), "sub".into());
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/sub/a.RAF").is_file());
    assert!(d.path().join("photos/sub/a.xmp").is_file());
    assert!(!d.path().join("photos/a.RAF").exists());
    assert_eq!(at(&e, &d, a), "sub/a.RAF");
    assert_eq!(at(&e, &d, b), "sub/b.RAF");
    assert_eq!(e.messages.line(), "Moved 2 photos to sub");
    // The open photo stays open, at its file's new place.
    assert_eq!(e.document.catalog_photo, Some(a));
    let open = e.document.path.clone().unwrap();
    assert!(open.ends_with("sub/a.RAF"), "{}", open.display());
    Ok(())
}

#[test]
fn a_drop_where_the_photos_are_already_does_nothing() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "sub/x.RAF"])?;
    let x = photo(&e, &d, "sub/x.RAF");
    let r = root(&e);
    let library = e.library.as_mut().unwrap();
    library.request_move(vec![x], r, "sub".into(), "sub".into());
    assert!(library.next_request().is_none());
    Ok(())
}

#[test]
fn move_to_folder_keeps_keys_from_the_photos_and_escape_closes_it() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "sub/x.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    e.choose_folder_for(vec![a]);
    frame(&mut e, Vec::new());
    assert!(e.file_ops.dialog_open());
    // Delete goes to the dialog, not to the photo behind it.
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert!(e.file_ops.dialog_open());
    tap(&mut e, Key::Escape, Modifiers::NONE);
    assert!(!e.file_ops.dialog_open());
    assert!(!e.activity.is_busy());
    assert_eq!(ids_left(&e).len(), 2);
    Ok(())
}

/// A decoded photo: the half-size one first shown when `fast`.
pub(super) fn decoded(fast: bool) -> std::sync::Arc<crate::camera_data::CameraImage> {
    std::sync::Arc::new(crate::camera_data::CameraImage {
        recovered: Default::default(),
        width: 2,
        height: 2,
        pixels: vec![[0.1; 3]; 4],
        metadata: Default::default(),
        fast,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    })
}

/// Types `text` into the focused field.
pub(super) fn type_text(e: &mut Editor, text: &str) {
    frame(e, vec![egui::Event::Text(text.into())]);
}

#[test]
fn new_folder_inside_takes_a_typed_name_and_moves_the_selection_into_it() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "b.RAF", "sub/x.RAF"])?;
    let (a, b) = (photo(&e, &d, "a.RAF"), photo(&e, &d, "b.RAF"));
    select(&mut e, &[a]);
    let r = root(&e);
    e.name_new_folder(r, String::new(), "photos".into(), vec![a], true);
    frame(&mut e, Vec::new());
    type_text(&mut e, "Selects");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    // The move is confirmed once more, Cancel first.
    let (title, ..) = e.file_ops.question().expect("the confirmation is open");
    assert_eq!(
        title,
        "Make folder “Selects” in “photos” and move “a.RAF” into it?"
    );
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/Selects/a.RAF").is_file());
    assert!(folder(&e, "Selects").is_some(), "the catalog has it");
    assert_eq!(at(&e, &d, a), "Selects/a.RAF");
    assert_eq!(at(&e, &d, b), "b.RAF");
    assert_eq!(
        e.messages.line(),
        "Made folder Selects and moved 1 photo into it"
    );
    Ok(())
}

#[test]
fn a_new_folder_with_nothing_to_move_is_made_on_create() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "2024/day1/x.RAF"])?;
    let r = root(&e);
    // 2024 is no catalog folder of its own.
    assert!(folder(&e, "2024").is_none());
    e.name_new_folder(r, "2024".into(), "2024".into(), Vec::new(), false);
    frame(&mut e, Vec::new());
    type_text(&mut e, "day2");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming(), "Create was the confirmation");
    assert!(d.path().join("photos/2024/day2").is_dir());
    assert!(folder(&e, "2024/day2").is_some());
    assert_eq!(e.messages.line(), "Made folder day2");
    // Its parent unfolds, so the folder shows in the tree.
    let library = e.library.as_ref().unwrap();
    assert!(library.is_unfolded(&format!("root:{r}/2024")));
    Ok(())
}

#[test]
fn a_name_a_folder_cant_have_makes_nothing() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    let r = root(&e);
    e.name_new_folder(r, String::new(), "photos".into(), Vec::new(), false);
    frame(&mut e, Vec::new());
    type_text(&mut e, "a/b");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(e.file_ops.dialog_open(), "still naming it");
    assert!(!d.path().join("photos/a").exists());
    tap(&mut e, Key::Escape, Modifiers::NONE);
    assert!(!e.file_ops.dialog_open());
    Ok(())
}

/// Leaves catalog `catalog` as a crash right after making folder `name` in
/// the root would: its item journalled, the folder there, no file object
/// recorded.
fn crashed_while_making(catalog: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let db = rusqlite::Connection::open(catalog)?;
    let (root, original): (i64, String) =
        db.query_row("SELECT id, original_path FROM roots", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    let computer: String = db.query_row("SELECT id FROM computers", [], |r| r.get(0))?;
    let made = Path::new(&original).join(name);
    std::fs::create_dir(&made)?;
    let state = serde_json::json!({
        "root": root,
        "parent": "",
        "parent_path": original,
        "name": name,
        "folder": null,
    });
    db.execute(
        "INSERT INTO file_operations(batch, computer, kind, state, preconditions, created_at)
         VALUES ('crashed', ?1, 'create_folder', 'planned', ?2, '2026-10-20 10:00:00')",
        rusqlite::params![computer, state.to_string()],
    )?;
    let id = db.last_insert_rowid();
    db.execute(
        "INSERT INTO file_operation_members(operation, ordinal, source, state)
         VALUES (?1, 0, ?2, 'planned')",
        rusqlite::params![id, made.to_str().unwrap()],
    )?;
    Ok(made)
}

#[test]
fn what_a_crash_left_unfinished_is_asked_about_when_the_catalog_opens() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    let made = crashed_while_making(&d.path().join("test.rawmakase"), "Selects")?;
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    e.recover_files_later(catalog.clone());
    frame(&mut e, Vec::new());
    settle(&mut e);
    frame(&mut e, Vec::new());
    assert!(e.file_ops.dialog_open(), "{}", e.messages.line());
    let State::Unfinished { items, .. } = &e.file_ops.state else {
        panic!("not asking");
    };
    // A folder it can't tell it made: never added, never deleted.
    assert_eq!(items[0].choices, [file_ops::Resolution::Keep]);
    assert!(folder(&e, "Selects").is_none());
    // Keys go to the dialog, not to the photos behind it.
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    // Decide Later: asked again at the next open.
    e.file_ops.state = State::Idle;
    e.recover_files_later(catalog);
    frame(&mut e, Vec::new());
    settle(&mut e);
    let State::Unfinished { items, .. } = &e.file_ops.state else {
        panic!("not asking again");
    };
    let operation = items[0].operation;
    e.resolve_unfinished(operation, file_ops::Resolution::Keep);
    settle(&mut e);
    frame(&mut e, Vec::new());
    assert!(!e.file_ops.dialog_open());
    assert!(!e.activity.is_busy());
    assert_eq!(e.messages.line(), "Kept as it is");
    assert!(made.is_dir(), "the folder stays");
    assert!(folder(&e, "Selects").is_none());
    Ok(())
}

#[test]
fn an_open_edit_that_cant_be_saved_puts_off_the_check_once() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    crashed_while_making(&d.path().join("test.rawmakase"), "Selects")?;
    let a = photo(&e, &d, "a.RAF");
    let file = d.path().join("photos/a.RAF");
    e.module = Module::Develop;
    e.document.catalog_photo = Some(a);
    e.document.opened_at(file.clone());
    e.document.edit.change(None, |r| r.exposure = 0.8);
    std::fs::rename(&file, d.path().join("photos/gone.RAF"))?;
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    e.recover_files_later(catalog);
    frame(&mut e, Vec::new());
    // Not tried again every frame: given up, and said so.
    assert!(e.file_ops.pending_recovery.is_none());
    assert!(matches!(e.file_ops.state, State::Idle));
    assert!(!e.activity.is_busy());
    assert!(
        e.messages
            .line()
            .contains("Unfinished file operations weren't checked"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

#[test]
fn unfinished_operations_are_not_asked_about_in_another_catalog() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    crashed_while_making(&d.path().join("test.rawmakase"), "Selects")?;
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    e.recover_files_later(catalog);
    frame(&mut e, Vec::new());
    settle(&mut e);
    frame(&mut e, Vec::new());
    assert!(matches!(e.file_ops.state, State::Unfinished { .. }));
    // Another catalog opens.
    let (_other_dir, mut other, _) = editor_with_folders(&["b.RAF"])?;
    e.library = other.library.take();
    frame(&mut e, Vec::new());
    assert!(!e.file_ops.dialog_open());
    assert!(matches!(e.file_ops.state, State::Idle));
    Ok(())
}

#[test]
fn a_catalog_with_nothing_unfinished_is_not_held_when_it_opens() -> anyhow::Result<()> {
    let (_d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    e.recover_files_later(catalog);
    frame(&mut e, Vec::new());
    // Nothing to recover: keys and saves aren't held up for it.
    assert!(!e.activity.is_busy());
    assert!(matches!(e.file_ops.state, State::Idle));
    assert!(e.file_ops.pending_recovery.is_none());
    Ok(())
}

#[test]
fn a_load_under_way_when_its_photo_moves_never_brings_back_the_old_path() -> anyhow::Result<()> {
    use crate::app::worker::{Event, LoadedHeader};
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "sub/x.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    let old = d.path().join("photos/a.RAF");
    // a is being opened: its header hasn't come yet.
    e.document.reset(Some(a));
    let (opening, _) = e.document.start_load();
    e.request_photo_move(vec![a], root(&e), "sub".into(), "sub".into());
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert_eq!(at(&e, &d, a), "sub/a.RAF");
    // The header read at the old path comes after the move.
    e.tx.send(Event::Header(Box::new(LoadedHeader {
        from: Default::default(),
        id: opening,
        file: crate::storage::Identity::read(&old.clone()).ok(),
        path: old.clone(),
        metadata: Default::default(),
        recipe: Default::default(),
        export: Default::default(),
        status: "Original".into(),
    })))
    .unwrap();
    let ctx = e.context.clone();
    e.events(&ctx);
    assert_ne!(e.document.path.as_deref(), Some(old.as_path()));
    assert_eq!(e.document.catalog_photo, Some(a), "it is opened again");
    assert_ne!(e.document.load_id(), opening, "from where it is now");
    Ok(())
}

#[test]
fn a_photo_shown_at_half_size_when_it_moves_is_opened_again_from_its_new_file() -> anyhow::Result<()>
{
    use crate::app::worker::{Event, TaskKind};
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "sub/x.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    let old = d.path().join("photos/a.RAF");
    // a opened, shown at half size; its full-size decode, of the old path,
    // still to come.
    e.document.reset(Some(a));
    e.document.opened_at(old.clone());
    let (opening, _) = e.document.start_load();
    e.tx.send(Event::Ready {
        id: opening,
        full: decoded(true),
        file: None,
        status: "Loading full resolution…".into(),
    })
    .unwrap();
    let ctx = e.context.clone();
    e.events(&ctx);
    e.request_photo_move(vec![a], root(&e), "sub".into(), "sub".into());
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert_eq!(at(&e, &d, a), "sub/a.RAF");
    // The full-size decode fails at the old path, after the move.
    e.tx.send(Event::Failed {
        id: opening,
        task: TaskKind::Load,
        error: "a.RAF: no such file".into(),
    })
    .unwrap();
    e.events(&ctx);
    assert_ne!(e.messages.line(), "a.RAF: no such file");
    assert_ne!(
        e.document.load_id(),
        opening,
        "opened again where it is now"
    );
    assert_eq!(e.document.catalog_photo, Some(a));
    assert_ne!(e.document.path.as_deref(), Some(old.as_path()));
    Ok(())
}

#[test]
fn a_photo_whose_load_failed_before_it_moved_is_opened_again_from_its_new_file()
-> anyhow::Result<()> {
    use crate::app::worker::{Event, TaskKind};
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "sub/x.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    e.document.reset(Some(a));
    e.document.opened_at(d.path().join("photos/a.RAF"));
    let (opening, _) = e.document.start_load();
    e.tx.send(Event::Failed {
        id: opening,
        task: TaskKind::Load,
        error: "a.RAF: could not be read".into(),
    })
    .unwrap();
    let ctx = e.context.clone();
    e.events(&ctx);
    assert!(e.document.failed.is_some());
    e.request_photo_move(vec![a], root(&e), "sub".into(), "sub".into());
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert_eq!(at(&e, &d, a), "sub/a.RAF");
    // Opened again (its fixture bytes then fail to decode, on their own
    // time).
    assert_ne!(
        e.document.load_id(),
        opening,
        "opened again where it is now"
    );
    Ok(())
}

#[test]
fn photos_dropped_on_a_folder_with_no_catalog_folder_move_into_it() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "2024/day1/x.RAF"])?;
    assert!(folder(&e, "2024").is_none());
    let a = photo(&e, &d, "a.RAF");
    e.request_photo_move(vec![a], root(&e), "2024".into(), "2024".into());
    settle(&mut e);
    let (title, ..) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move “a.RAF” to “2024”?");
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/2024/a.RAF").is_file());
    assert_eq!(at(&e, &d, a), "2024/a.RAF");
    assert!(folder(&e, "2024").is_some(), "now a catalog folder");
    assert_eq!(e.messages.line(), "Moved 1 photo to 2024");
    Ok(())
}

#[test]
fn photos_moved_into_the_folder_shown_stay_shown_when_it_gains_a_catalog_folder()
-> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["2024/day1/x.RAF", "2024/day2/y.RAF"])?;
    let (x, r) = (photo(&e, &d, "2024/day1/x.RAF"), root(&e));
    // The root is shown; it has no photos, so no catalog folder, of its own.
    e.library
        .as_mut()
        .unwrap()
        .restore_source(&format!("root:{}", r.0), None);
    e.request_photo_move(vec![x], r, String::new(), "photos".into());
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert_eq!(at(&e, &d, x), "x.RAF");
    assert!(
        e.library.as_ref().unwrap().shown().contains(&x),
        "the moved photo is still shown in the root it was dropped on"
    );
    Ok(())
}
