//! Editor tests of `undo`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use crate::catalog::PhotoId;
use eframe::egui::{Pos2, Rect};

#[test]
fn history_snapshot_undo_and_redo() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let original = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 2.;
    e.commit_edit(original.clone(), None);
    assert!(e.document.edit.history().can_undo());
    e.undo();
    assert_eq!(*e.document.edit.recipe(), original);
    assert!(e.document.edit.history().can_redo());
    e.redo();
    assert_eq!(e.document.edit.recipe().exposure, 2.);
    assert!(e.document.edit.history().can_undo());
}

#[test]
fn undo_and_redo_keys_work_while_a_button_has_focus() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let original = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 2.;
    e.commit_edit(original.clone(), None);
    // A clicked button or the tone curve keeps focus; that must not block shortcuts.
    let frame = |input, e: &mut Editor| {
        let mut output = ctx.run_ui(input, |ui| {
            ui.button("focused").request_focus();
            assert!(ctx.egui_wants_keyboard_input());
            e.develop_shortcuts(&ctx);
        });
        output.textures_delta.clear();
    };
    let press = |shift: bool| {
        let modifiers = egui::Modifiers {
            command: true,
            mac_cmd: cfg!(target_os = "macos"),
            ctrl: !cfg!(target_os = "macos"),
            shift,
            ..Default::default()
        };
        egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::Z,
                    physical_key: Some(egui::Key::Z),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        }
    };
    frame(egui::RawInput::default(), &mut e);
    frame(press(false), &mut e);
    assert_eq!(*e.document.edit.recipe(), original);
    frame(press(true), &mut e);
    assert_eq!(e.document.edit.recipe().exposure, 2.);
}

#[test]
fn undo_brings_back_a_range_rejected_under_the_unflagged_filter() -> anyhow::Result<()> {
    use crate::app::library::photo_metadata::Edit;
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF", "d.RAF", "e.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.show_unflagged();
    library.select(Some(ids[1]));
    library.select_range_to(ids[3]);
    library.edit_selection(Edit::Flag(-1), false)?;
    assert_eq!(library.shown().len(), 2);
    e.undo();
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.shown().len(), 5);
    assert_eq!(library.selected(), Some(ids[3]));
    assert_eq!(library.selected_photos(), ids[1..4]);
    assert!(library.session.photos().iter().all(|p| p.flag == 0));
    assert!(e.messages.line().starts_with("Undo 3 photos"));
    e.redo();
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.shown().len(), 2);
    assert_eq!(
        library
            .session
            .photos()
            .iter()
            .filter(|p| p.flag == -1)
            .count(),
        3
    );
    // A write that fails is never logged.
    let library = e.library.as_mut().unwrap();
    assert!(
        library
            .edit_metadata(PhotoId(9999), Edit::Rating(5), false)
            .is_err()
    );
    e.sync_undo();
    assert_eq!(e.undo_log.len(), (1, 0));
    Ok(())
}

#[test]
fn undo_in_develop_reverses_the_flag_before_the_exposure() -> anyhow::Result<()> {
    use crate::app::library::photo_metadata::Edit;
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(d.path().join("photos/a.RAF"));
    let original = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 1.;
    e.commit_edit(original.clone(), None);
    e.library
        .as_mut()
        .unwrap()
        .edit_metadata(ids[0], Edit::Flag(1), false)?;
    e.undo();
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag, 0);
    assert_eq!(e.document.edit.recipe().exposure, 1.);
    assert!(e.module == Module::Develop);
    e.undo();
    assert_eq!(*e.document.edit.recipe(), original);
    e.redo();
    e.redo();
    assert_eq!(e.document.edit.recipe().exposure, 1.);
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag, 1);
    Ok(())
}

/// Undoing a Develop change on a photo closed since saves to the file its saved
/// edit names: the same bytes with another modification time, as a verified
/// move with a coarser time leaves them, still take the undo (issue #870).
#[test]
fn undo_on_a_closed_photo_follows_a_verified_move_of_its_file() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let a = d.path().join("photos/a.RAF");
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(a.clone());
    let original = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 1.;
    e.commit_edit(original, None);
    assert!(e.flush());
    e.sync_undo();
    e.document.reset(Some(ids[1]));
    e.document.opened_at(d.path().join("photos/b.RAF"));
    std::fs::File::options()
        .write(true)
        .open(&a)?
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1))?;
    let library = e.library.as_ref().unwrap();
    assert!(library.session.catalog().load_edit(ids[0], &a)?.is_some());
    e.undo();
    let library = e.library.as_ref().unwrap();
    let saved = library.session.catalog().load_edit(ids[0], &a)?.unwrap();
    assert_eq!(saved.recipe.exposure, 0.);
    Ok(())
}

#[test]
fn undoing_a_history_click_returns_to_the_exact_step() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    for exposure in [0.25, 0.5, 1.] {
        let before = e.document.edit.recipe().clone();
        e.document.edit.setup_mut().exposure = exposure;
        e.commit_edit(before, None);
    }
    // A click on the first state jumps three steps back as one command.
    assert!(e.document.edit.jump(0));
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    e.undo();
    assert_eq!(e.document.edit.recipe().exposure, 1.);
    assert_eq!(e.document.edit.history().steps().1, 3);
    e.redo();
    assert_eq!(e.document.edit.history().steps().1, 0);
    // The History panel still lists every step.
    assert_eq!(e.document.edit.history().steps().0.len(), 3);
}

#[test]
fn a_library_change_is_undone_in_the_library() -> anyhow::Result<()> {
    use crate::app::library::photo_metadata::Edit;
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[1]));
    library.edit_selection(Edit::Rating(4), false)?;
    e.sync_undo();
    // Off to Develop on the other photo.
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(d.path().join("photos/a.RAF"));
    e.library.as_mut().unwrap().select(Some(ids[0]));
    e.undo();
    assert!(e.module == Module::Library);
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.photo(ids[1]).unwrap().rating, 0);
    assert_eq!(library.selected(), Some(ids[1]));
    Ok(())
}

#[test]
fn undoing_a_history_click_after_a_new_branch_restores_its_own_state() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let before = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 0.5;
    e.commit_edit(before, None);
    // Click the opened state, then edit: History branches.
    assert!(e.document.edit.jump(0));
    let before = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 1.;
    e.commit_edit(before, None);
    e.undo();
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    // The same step count now leads to the other branch; the click's own
    // state comes back.
    e.undo();
    assert_eq!(e.document.edit.recipe().exposure, 0.5);
}

#[test]
fn a_key_that_changes_nothing_is_not_an_undo_step() -> anyhow::Result<()> {
    use crate::app::library::photo_metadata::Edit;
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.edit_metadata(ids[0], Edit::Rating(5), false)?;
    library.edit_metadata(ids[0], Edit::Rating(5), false)?;
    e.sync_undo();
    assert_eq!(e.undo_log.len(), (1, 0));
    e.undo();
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().rating, 0);
    Ok(())
}

#[test]
fn leaving_a_photo_mid_drag_saves_the_drag_as_a_history_step() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"gesture fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let l = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let id = l.session.photos()[0].id;
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(l));
    editor.document.catalog_photo = Some(id);
    editor.document.opened_at(photo);
    // A slider still held down when Left or Right leaves the photo.
    let frame = editor.document.edit.begin();
    editor.document.edit.setup_mut().exposure = 0.6;
    editor
        .document
        .edit
        .finish(frame, crate::edit_session::Gesture::Held);
    assert!(editor.flush());
    assert!(!editor.document.edit.history().in_gesture());
    let library = editor.library.as_ref().unwrap();
    let history = library.session.catalog().load_history(id)?.unwrap();
    assert_eq!(history.applied, 1);
    assert_eq!(history.steps.len(), 1);
    assert_eq!(history.steps[0].recipe.exposure, 0.6);
    // Undo on the next photo reaches it.
    assert_eq!(editor.undo_log.len(), (1, 0));
    // A save that fails keeps the drag going, as one step.
    editor.document.catalog_photo = Some(PhotoId(id.0 + 1000));
    let frame = editor.document.edit.begin();
    editor.document.edit.setup_mut().exposure = 0.9;
    editor
        .document
        .edit
        .finish(frame, crate::edit_session::Gesture::Held);
    assert!(!editor.flush());
    assert!(editor.document.edit.history().in_gesture());
    assert_eq!(editor.undo_log.len(), (1, 0));
    Ok(())
}

#[test]
fn undo_during_a_drag_takes_back_the_drag_and_can_be_redone() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.module = Module::Develop;
    let edit = |editor: &mut Editor, exposure: f32, held: bool| {
        let frame = editor.document.edit.begin();
        editor.document.edit.setup_mut().exposure = exposure;
        let gesture = if held {
            crate::edit_session::Gesture::Held
        } else {
            crate::edit_session::Gesture::Released
        };
        editor.document.edit.finish(frame, gesture);
        editor.sync_undo();
    };
    edit(&mut editor, 0.3, false);
    edit(&mut editor, 0.6, true);
    editor.undo();
    assert_eq!(editor.document.edit.recipe().exposure, 0.3);
    assert_eq!(editor.undo_log.len(), (1, 1));
    editor.redo();
    assert_eq!(editor.document.edit.recipe().exposure, 0.6);
}

#[test]
fn moving_to_the_next_photo_mid_frame_edits_it_in_a_frame_of_its_own() -> anyhow::Result<()> {
    // The editor frame's assertion stays on: the panels drawn after the
    // filmstrip or a shortcut opened the next photo edit that photo.
    let (_dir, ctx, mut editor, [a, b]) = two_photo_editor()?;
    editor.module = Module::Develop;
    editor.develop_catalog_photo(a);
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    };
    for events in [vec![], vec![key(egui::Key::ArrowRight)], vec![]] {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400., 900.))),
                events,
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
    }
    assert_eq!(editor.document.catalog_photo, Some(b));
    Ok(())
}

#[test]
fn a_copy_removed_when_the_catalog_cannot_be_read_again_still_leaves_undo() -> anyhow::Result<()> {
    let (_dir, _ctx, mut editor, [a, b]) = two_photo_editor()?;
    let library = editor.library.as_mut().unwrap();
    let copy = library.create_virtual_copy(a)?.value;
    library.edit_metadata(
        copy,
        crate::app::library::photo_metadata::Edit::Rating(5),
        false,
    )?;
    editor.sync_undo();
    // Removing reads no collections; reading the catalog again does, and fails.
    editor
        .library
        .as_mut()
        .unwrap()
        .session
        .catalog()
        .db_for_tests()
        .execute_batch("ALTER TABLE collections RENAME TO collections_gone")?;
    editor.remove_virtual_copy(copy);
    let catalog = editor.library.as_ref().unwrap().session.catalog();
    assert!(
        catalog.photos()?.iter().all(|p| p.id != copy),
        "the copy is removed"
    );
    let status = editor.messages.line().to_string();
    editor
        .library
        .as_mut()
        .unwrap()
        .session
        .catalog()
        .db_for_tests()
        .execute_batch("ALTER TABLE collections_gone RENAME TO collections")?;
    // The next copy, of B rated 2, is given the removed copy's id.
    let again = editor
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(b)?
        .value;
    assert_eq!(again, copy);
    assert_eq!(rating(&editor, again), 2);
    editor.undo();
    assert_eq!(rating(&editor, again), 2);
    assert!(status.contains("could not be read again"), "{status}");
    Ok(())
}

#[test]
fn undo_never_writes_a_removed_copys_id_given_to_a_new_copy() -> anyhow::Result<()> {
    let (_dir, _ctx, mut editor, [a, b]) = two_photo_editor()?;
    let library = editor.library.as_mut().unwrap();
    let copy = library.create_virtual_copy(a)?.value;
    library.edit_metadata(
        copy,
        crate::app::library::photo_metadata::Edit::Rating(5),
        false,
    )?;
    editor.sync_undo();
    editor.remove_virtual_copy(copy);
    // The catalog gives the removed copy's id to the next copy, of B, rated 2.
    let again = editor
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(b)?
        .value;
    assert_eq!(again, copy);
    assert_eq!(rating(&editor, again), 2);
    editor.undo();
    assert_eq!(rating(&editor, again), 2);
    Ok(())
}

#[test]
fn a_change_to_several_photos_keeps_undoing_those_left_after_one_is_removed() -> anyhow::Result<()>
{
    let (_dir, _ctx, mut editor, [a, b]) = two_photo_editor()?;
    let library = editor.library.as_mut().unwrap();
    let copy = library.create_virtual_copy(a)?.value;
    library.edit_photos(
        &[a, copy],
        crate::app::library::photo_metadata::Edit::Rating(4),
        false,
    )?;
    editor.sync_undo();
    // Undone, then the copy goes, and its id comes back for a copy of B.
    editor.undo();
    assert_eq!(rating(&editor, a), 0);
    editor.remove_virtual_copy(copy);
    let again = editor
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(b)?
        .value;
    assert_eq!(again, copy);
    // Redo rates A again and leaves the new copy as it was made.
    editor.redo();
    assert_eq!(rating(&editor, a), 4);
    assert_eq!(rating(&editor, again), 2);
    Ok(())
}

#[test]
fn a_rating_made_in_develop_on_a_removed_copy_still_undoes_the_photo_it_rated() -> anyhow::Result<()>
{
    let (_dir, _ctx, mut editor, [a, b]) = two_photo_editor()?;
    let copy = editor
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(a)?
        .value;
    // The copy is open in Develop; the filmstrip menu rates B.
    editor.document.catalog_photo = Some(copy);
    editor.module = Module::Develop;
    editor.library.as_mut().unwrap().edit_metadata(
        b,
        crate::app::library::photo_metadata::Edit::Rating(3),
        false,
    )?;
    editor.sync_undo();
    editor.remove_virtual_copy(copy);
    assert_eq!(rating(&editor, b), 3);
    editor.undo();
    assert_eq!(rating(&editor, b), 2);
    Ok(())
}

#[test]
fn a_change_still_waiting_for_the_undo_log_forgets_a_removed_folders_photos() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.ARW"), b"fixture")?;
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx)?));
    let library = editor.library.as_mut().unwrap();
    let a = library.session.photos()[0].id;
    let folders = library.session.folders().iter().map(|f| f.id).collect();
    // A change made, its command not yet in the undo log, when the folder goes:
    // the flush before removing puts it there, to be purged with the photos.
    library.edit_descriptive(
        &[a],
        crate::catalog_session::DescriptiveEdit::AddKeywords(vec![vec!["Trip".into()]]),
    )?;
    editor.remove_folders(&library::FolderRemoval {
        catalog: crate::catalog::CatalogLocation::File(path),
        name: "photos".into(),
        folders,
        photos: 1,
        empty_root: None,
        exported: None,
    });
    editor.sync_undo();
    // Its id may be the next photo's: nothing may undo onto it.
    assert!(!editor.undo_log.can_undo());
    Ok(())
}
