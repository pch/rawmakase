//! Editor tests of `exit`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;

#[test]
fn quitting_saves_an_edit_still_waiting_for_autosave() -> anyhow::Result<()> {
    // Quit on macOS (Cmd-Q) closes the window without a close request, so only the
    // exit hook sees it; the close guard's flush never runs.
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let path = d.path().join("photos/a.RAF");
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(path.clone());
    let before = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 0.7;
    e.commit_edit(before, None);
    assert!(e.document.edit.save_state().needs_save());
    eframe::App::on_exit(&mut e, None);
    assert!(!e.document.edit.save_state().needs_save());
    let library = e.library.as_ref().unwrap();
    let saved = library.session.catalog().load_edit(ids[0], &path)?.unwrap();
    assert_eq!(saved.recipe.exposure, 0.7);
    Ok(())
}

/// An editor showing a catalog photo with an edit not saved yet, on a catalog whose
/// saves stall, as on a network share that stopped answering.
fn editor_with_a_stalled_catalog() -> anyhow::Result<(tempfile::TempDir, Editor)> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(d.path().join("photos/a.RAF"));
    e.autosave = crate::app::develop::autosave::Autosave::stalled();
    let before = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 0.7;
    e.commit_edit(before, None);
    Ok((d, e))
}

#[test]
fn an_editor_quitting_leaves_another_tests_export_in_place() -> anyhow::Result<()> {
    // Tests share one process: an editor's exit sequence deleting every export
    // still being written failed a parallel test's export (missing file).
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("export.tif");
    let image = crate::rendered::Rendered {
        width: 2,
        height: 2,
        pixels: vec![[0.5; 3]; 4],
    };
    let staged = crate::export::stage(
        &path,
        &image,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.exit_within(std::time::Duration::from_secs(3));
    crate::storage::persist(staged.file, &path, crate::export::Replace::NoClobber)?;
    assert!(path.exists());
    Ok(())
}

#[test]
fn quitting_gives_up_on_a_save_the_catalog_does_not_answer() -> anyhow::Result<()> {
    let (_d, mut e) = editor_with_a_stalled_catalog()?;
    let started = std::time::Instant::now();
    e.exit_within(std::time::Duration::from_millis(100));
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    assert!(e.document.edit.save_state().needs_save());
    Ok(())
}

#[test]
fn closing_while_the_catalog_does_not_answer_keeps_the_window_open() -> anyhow::Result<()> {
    let (_d, mut e) = editor_with_a_stalled_catalog()?;
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(close_request(), |ui| e.pending_work(ui.ctx()));
    output.textures_delta.clear();
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose)
    );
    assert!(e.close_confirm);
    assert!(e.autosave.busy(), "the save goes on");
    // Closes once the save is done, without asking again.
    assert!(e.close_after_work);
    assert!(e.quit_by.is_none());
    assert!(e.document.edit.save_state().needs_save());
    Ok(())
}

#[test]
fn closing_during_a_folder_change_waits_for_it_then_closes() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    assert!(e.activity.begin_folder_change());
    assert!(e.quitting_would_cut_off_work());
    let commands = |e: &mut Editor, input| {
        let mut output = ctx.run_ui(input, |ui| e.pending_work(ui.ctx()));
        output.textures_delta.clear();
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .clone()
    };
    assert!(commands(&mut e, close_request()).contains(&egui::ViewportCommand::CancelClose));
    assert!(e.close_confirm);
    // Still running: the window stays open and asks nothing more.
    assert!(!commands(&mut e, Default::default()).contains(&egui::ViewportCommand::Close));
    // Once the catalog opens again, the close goes ahead.
    e.activity.finish_dialog();
    assert!(commands(&mut e, Default::default()).contains(&egui::ViewportCommand::Close));
    assert!(!e.close_confirm);
}

#[test]
fn closing_anyway_does_not_wait_for_a_folder_change() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    assert!(e.activity.begin_folder_change());
    // Close anyway, as the question offers: the next close goes ahead.
    e.close_anyway = true;
    let mut output = ctx.run_ui(close_request(), |ui| e.pending_work(ui.ctx()));
    output.textures_delta.clear();
    let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(!commands.contains(&egui::ViewportCommand::CancelClose));
    assert!(e.quit_by.is_some());
    assert!(!e.close_anyway, "for that close only");
}

/// The input of a frame in which the window is asked to close.
fn close_request() -> egui::RawInput {
    let mut input = egui::RawInput::default();
    input.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            events: vec![egui::ViewportEvent::Close],
            ..Default::default()
        },
    );
    input
}

#[test]
fn quitting_waits_for_every_worker_it_stops() -> anyhow::Result<()> {
    let (_d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    let waited = e.exit_within(std::time::Duration::from_secs(20));
    assert_eq!(waited.detached, 0);
    assert!(waited.finished > 0);
    Ok(())
}

/// The edit's first save fails after an earlier one succeeded: a quit from the Dock
/// must reach the close guard, which keeps the window open and the edit unsaved.
#[test]
fn a_quit_with_an_edit_whose_next_save_fails_keeps_the_window_open() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture")?;
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
    editor.document.opened_at(photo.clone());
    editor.document.edit.save_state_mut().mark_changed();
    assert!(editor.flush());
    assert!(!editor.quitting_would_cut_off_work());
    // A copy shown keeps its saved name as the draft: nothing to save. A name
    // being typed is saved by the guard too.
    let library = editor.library.as_mut().unwrap();
    let copy = library.create_virtual_copy(id)?.value;
    let saved_name = library.photo(copy).unwrap().copy_name.clone();
    library.set_copy_name_draft(copy, &saved_name);
    assert!(!editor.quitting_would_cut_off_work());
    editor
        .library
        .as_mut()
        .unwrap()
        .set_copy_name_draft(copy, "B&W");
    assert!(editor.quitting_would_cut_off_work());
    editor.library.as_mut().unwrap().discard_drafts();
    // The catalog stops taking edits; the next change is not saved yet.
    editor
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .db_for_tests()
        .execute_batch(
            "CREATE TRIGGER no_edits BEFORE UPDATE OF recipe ON photos
             BEGIN SELECT RAISE(ABORT, 'read-only'); END;",
        )?;
    editor.document.edit.change(None, |r| r.exposure = 1.);
    assert!(editor.quitting_would_cut_off_work());
    // The close the Dock's quit turns into: the guard tries to save, fails, and asks.
    let mut output = ctx.run_ui(close_request(), |ui| editor.pending_work(ui.ctx()));
    output.textures_delta.clear();
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose)
    );
    assert!(editor.close_confirm);
    assert!(editor.document.edit.save_state().needs_save());
    let saved = editor
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .load_edit(id, &photo)?
        .unwrap();
    assert_eq!(saved.recipe.exposure, 0.);
    Ok(())
}
