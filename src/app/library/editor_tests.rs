//! Editor tests of `library`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;

/// Issue #746: with Picked shown, U in Develop hides the photo open, and the
/// arrows only work again once Develop moves to a photo still shown.
#[test]
fn unflagging_the_photo_open_in_develop_with_picked_shown_opens_one_still_shown()
-> anyhow::Result<()> {
    use crate::app::library::photo_metadata::Edit;
    let (_dir, _ctx, mut editor, [a, b]) = two_photo_editor()?;
    let library = editor.library.as_mut().unwrap();
    library.edit_photos(&[a, b], Edit::Flag(1), false)?;
    let mut layout = library.layout();
    layout.flags = vec![1];
    layout.filters_off = false;
    library.apply_layout(&layout);
    editor.module = Module::Develop;
    editor.develop_catalog_photo(b);
    assert_eq!(editor.document.catalog_photo, Some(b));
    // U, as the Develop shortcut sends it.
    assert!(editor.command_metadata(Edit::Flag(0), None, false).is_ok());
    assert_eq!(editor.library.as_ref().unwrap().photo(b).unwrap().flag, 0);
    assert_eq!(editor.document.catalog_photo, Some(a));
    assert_eq!(editor.library.as_ref().unwrap().selected(), Some(a));
    assert_eq!(editor.module, Module::Develop);
    // The Library moves its selection to B, picked again, but does not
    // open Develop.
    editor.module = Module::Library;
    assert!(
        editor
            .command_metadata(Edit::Flag(1), Some(b), false)
            .is_ok()
    );
    editor.library.as_mut().unwrap().make_active(a);
    assert!(
        editor
            .command_metadata(Edit::Flag(0), Some(a), false)
            .is_ok()
    );
    assert_eq!(editor.library.as_ref().unwrap().selected(), Some(b));
    assert_eq!(editor.module, Module::Library);
    assert_eq!(editor.document.catalog_photo, Some(a));
    Ok(())
}

/// What the session file says, as last written.
fn saved_session(path: &std::path::Path) -> crate::app::preferences::session::Session {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// The Library's place is remembered as soon as it changes, with the layout shown then
/// even while a drag or typing is still changing it; a change of layout alone waits
/// until it settles; without a Library nothing is remembered.
#[test]
fn the_place_is_remembered_at_once_and_a_layout_alone_once_it_settles() -> anyhow::Result<()> {
    use crate::app::library::views::View;
    use crate::app::shell::workspace::LayoutEdit;
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let session = d.path().join("session.json");
    e.session.set_file(Some(session.clone()));
    e.library.as_mut().unwrap().select(Some(ids[0]));
    e.remember_place(LayoutEdit::Settled);
    assert_eq!(saved_session(&session).selected_photo, Some(ids[0]));

    // A new place while the layout changes: both are kept, the layout as shown.
    let library = e.library.as_mut().unwrap();
    library.set_view(View::Loupe);
    library.select(Some(ids[1]));
    e.remember_place(LayoutEdit::Changing);
    let saved = saved_session(&session);
    assert_eq!(saved.selected_photo, Some(ids[1]));
    assert_eq!(saved.library_layout.view, "loupe");

    // The layout alone: not while it changes, then once it settles.
    e.library.as_mut().unwrap().set_view(View::Grid);
    e.remember_place(LayoutEdit::Changing);
    assert_eq!(saved_session(&session).library_layout.view, "loupe");
    e.remember_place(LayoutEdit::Settled);
    assert_eq!(saved_session(&session).library_layout.view, "grid");

    // Without a Library, what was remembered stays.
    std::fs::remove_file(&session)?;
    e.library = None;
    e.remember_place(LayoutEdit::Settled);
    assert!(!session.exists());
    Ok(())
}

/// An editor whose catalog has one folder, `photos`, holding `Aegean/Phase`,
/// `Beta` and `Pharos`, each with a photo; its Library open and drawn once.
fn folders_editor() -> anyhow::Result<(tempfile::TempDir, Editor)> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    for folder in ["Aegean/Phase", "Beta", "Pharos"] {
        let folder = photos.join(folder);
        std::fs::create_dir_all(&folder)?;
        std::fs::write(folder.join("a.RAF"), b"fixture")?;
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
    e.library = Some(Box::new(crate::app::library::Library::load(&path, ctx)?));
    e.module = Module::Library;
    folder_frame(&mut e, vec![]);
    Ok((d, e))
}

/// One frame of the whole editor, with `events`.
fn folder_frame(e: &mut Editor, events: Vec<egui::Event>) {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1200., 800.),
            )),
            events,
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
}

fn key_press(key: egui::Key) -> Vec<egui::Event> {
    [true, false]
        .map(|pressed| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        })
        .into()
}

/// Puts the keyboard in Filter Folders and types `text` there.
fn filter_folders(e: &mut Editor, text: &str) {
    let id = super::sidebar::folder_filter_id();
    e.context.memory_mut(|m| m.request_focus(id));
    folder_frame(e, vec![]);
    folder_frame(e, vec![egui::Event::Text(text.into())]);
    assert!(e.context.memory(|m| m.has_focus(id)));
}

fn library(e: &Editor) -> &super::Library {
    e.library.as_deref().unwrap()
}

/// Issue #1004: a folder among many is found by typing part of its name in
/// Filter Folders and chosen with Enter, without scrolling to it.
#[test]
fn enter_in_filter_folders_chooses_the_first_folder_found() -> anyhow::Result<()> {
    let (_d, mut e) = folders_editor()?;
    let before = library(&e).expanded.clone();
    // Typed in capitals; a P typed here doesn't pick the photo either.
    filter_folders(&mut e, "PHA");
    let l = library(&e);
    assert_eq!(l.folder_filter, "PHA");
    // Typing only filters the panel: the source, and what is unfolded, stay.
    assert!(l.selected_folders.is_empty());
    assert_eq!(l.filters.folder_scope, None);
    assert_eq!(l.expanded, before);
    assert!(l.session.photos().iter().all(|p| p.flag == 0));
    let trees = l.sidebar_trees.clone().unwrap();
    let filter = l.folder_filter(&trees).unwrap();
    let names: Vec<&str> = filter
        .matches
        .iter()
        .map(|k| k.rsplit('/').next().unwrap())
        .collect();
    // Found inside a folded folder, in the order the panel draws them.
    assert_eq!(names, ["Phase", "Pharos"]);
    let phase = filter.matches[0].to_string();
    let rows = l.folder_rows(&trees, Some(&filter));
    let shown: Vec<&str> = rows.iter().map(|k| k.rsplit('/').next().unwrap()).collect();
    assert_eq!(shown.len(), 4, "{shown:?}");
    assert_eq!(&shown[1..], ["Aegean", "Phase", "Pharos"]);
    // Enter chooses Phase, keeps the keyboard in the field, and opens no Loupe.
    folder_frame(&mut e, key_press(egui::Key::Enter));
    let l = library(&e);
    assert_eq!(l.selected_folders.keys(), std::slice::from_ref(&phase));
    assert!(l.filters.folder_scope.is_some());
    assert!(!l.loupe_open());
    let id = super::sidebar::folder_filter_id();
    folder_frame(&mut e, vec![]);
    assert!(e.context.memory(|m| m.has_focus(id)));
    // The folders above it are unfolded, so it stays in view once cleared.
    let l = library(&e);
    let (aegean, _) = phase.rsplit_once('/').unwrap();
    let (root, _) = aegean.rsplit_once('/').unwrap();
    assert!(l.expanded.contains(aegean) && l.expanded.contains(root));
    Ok(())
}

#[test]
fn enter_chooses_nothing_without_a_folder_found_or_the_keyboard() -> anyhow::Result<()> {
    let (_d, mut e) = folders_editor()?;
    // Nothing found: the note shows, nothing is chosen.
    filter_folders(&mut e, "zzzz");
    folder_frame(&mut e, key_press(egui::Key::Enter));
    assert!(library(&e).selected_folders.is_empty());
    // Only spaces: no filter.
    e.library.as_mut().unwrap().folder_filter.clear();
    filter_folders(&mut e, "   ");
    let l = library(&e);
    assert!(l.folder_filter(&l.sidebar_trees.clone().unwrap()).is_none());
    folder_frame(&mut e, key_press(egui::Key::Enter));
    assert!(library(&e).selected_folders.is_empty());
    // Text left in the field while the keyboard is elsewhere: Enter is the
    // grid's, which opens the Loupe on the active photo.
    e.library.as_mut().unwrap().folder_filter = "pha".into();
    e.context.memory_mut(|m| m.stop_text_input());
    folder_frame(&mut e, vec![]);
    folder_frame(&mut e, vec![]);
    folder_frame(&mut e, key_press(egui::Key::Enter));
    assert!(library(&e).selected_folders.is_empty());
    // The keyboard taken by another field since: Enter is that field's,
    // though egui says the filter lost the keyboard for two frames.
    filter_folders(&mut e, "");
    e.context
        .memory_mut(|m| m.request_focus(egui::Id::new("another field")));
    folder_frame(&mut e, vec![]);
    folder_frame(&mut e, key_press(egui::Key::Enter));
    assert!(library(&e).selected_folders.is_empty());
    // Tab away from the field: no folder chosen either.
    filter_folders(&mut e, "");
    folder_frame(&mut e, key_press(egui::Key::Tab));
    assert!(library(&e).selected_folders.is_empty());
    Ok(())
}

#[test]
fn escape_in_filter_folders_leaves_the_field_not_the_loupe() -> anyhow::Result<()> {
    let (_d, mut e) = folders_editor()?;
    e.library.as_mut().unwrap().open_loupe();
    folder_frame(&mut e, vec![]);
    assert!(library(&e).loupe_open());
    filter_folders(&mut e, "pha");
    folder_frame(&mut e, key_press(egui::Key::Escape));
    let id = super::sidebar::folder_filter_id();
    assert!(!e.context.memory(|m| m.has_focus(id)));
    assert!(library(&e).loupe_open());
    // The text stays, as in Lightroom.
    assert_eq!(library(&e).folder_filter, "pha");
    // The next Escape is the Loupe's.
    folder_frame(&mut e, key_press(egui::Key::Escape));
    assert!(!library(&e).loupe_open());
    Ok(())
}

#[test]
fn escape_in_filter_folders_leaves_a_develop_tool_open() -> anyhow::Result<()> {
    use crate::app::develop::state::Tool;
    let (_d, mut e) = folders_editor()?;
    e.module = Module::Develop;
    e.view.set_tool(Tool::Crop);
    folder_frame(&mut e, vec![]);
    filter_folders(&mut e, "pha");
    folder_frame(&mut e, key_press(egui::Key::Escape));
    assert_eq!(e.view.tool(), Tool::Crop);
    folder_frame(&mut e, key_press(egui::Key::Escape));
    assert_eq!(e.view.tool(), Tool::None);
    Ok(())
}
