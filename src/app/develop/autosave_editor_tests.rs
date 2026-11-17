//! Editor tests of `autosave`: the whole editor driven through its frames.
use crate::app::*;

#[test]
fn autosave_writes_the_catalog_in_the_background() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    let mut c = crate::catalog::Catalog::create(&catalog)?;
    c.add_folder(&photos)?;
    drop(c);
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
    let settled = || {
        save_state::SaveState::Pending(
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        )
    };
    editor.document.edit.setup_mut().exposure = 0.8;
    *editor.document.edit.save_state_mut() = settled();
    editor.autosave(&ctx);
    assert!(editor.autosave.busy());
    // Edited again while that save runs: still unsaved once it finishes.
    editor.document.edit.setup_mut().exposure = 1.1;
    editor.document.edit.save_state_mut().mark_changed();
    // On a slow machine the save can outlast the settle delay; keep the loop
    // below from starting the next save before this one is checked.
    if let save_state::SaveState::Saving { changed: Some(at) } =
        editor.document.edit.save_state_mut()
    {
        *at += std::time::Duration::from_secs(3600);
    }
    while editor.autosave.busy() {
        std::thread::sleep(std::time::Duration::from_millis(5));
        editor.autosave(&ctx);
    }
    assert!(matches!(
        editor.document.edit.save_state(),
        save_state::SaveState::Pending(_)
    ));
    let saved = |editor: &Editor| -> anyhow::Result<f32> {
        let library = editor.library.as_ref().unwrap();
        Ok(library
            .session
            .catalog()
            .load_edit(id, &photo)?
            .unwrap()
            .recipe
            .exposure)
    };
    assert_eq!(saved(&editor)?, 0.8);
    // A save before navigation waits for the one in flight, then saves.
    *editor.document.edit.save_state_mut() = settled();
    editor.autosave(&ctx);
    editor.document.edit.setup_mut().exposure = 1.4;
    editor.document.edit.save_state_mut().mark_changed();
    assert!(editor.flush());
    assert!(!editor.autosave.busy());
    assert!(!editor.document.edit.save_state().needs_save());
    assert_eq!(saved(&editor)?, 1.4);
    Ok(())
}

/// Issue 998: an autosave writes on a connection of its own; its completion
/// tells the session, so smart collections see the edit, even when the
/// document no longer takes the completion (it was saved some other way
/// while the save ran).
#[test]
fn an_autosave_tells_the_session_whether_or_not_the_document_takes_it() -> anyhow::Result<()> {
    let (_d, mut editor, ids) = crate::app::testing::editor_with_catalog(&["a.RAF"])?;
    let ctx = editor.context.clone();
    let path = editor
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    editor.document.catalog_photo = Some(ids[0]);
    editor.document.opened_at(path);
    let revision = |editor: &Editor| editor.library.as_ref().unwrap().session.query_revision();
    for taken in [true, false] {
        editor.document.edit.setup_mut().exposure += 0.2;
        *editor.document.edit.save_state_mut() = save_state::SaveState::Pending(
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        );
        editor.autosave(&ctx);
        assert!(editor.autosave.busy());
        if !taken {
            editor.document.edit.save_state_mut().saved();
        }
        let before = revision(&editor);
        while editor.autosave.busy() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            editor.autosave(&ctx);
        }
        assert!(revision(&editor) > before, "taken: {taken}");
    }
    Ok(())
}
