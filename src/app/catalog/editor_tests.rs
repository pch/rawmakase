//! Editor tests of `catalog`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;

#[test]
fn a_photo_from_outside_the_library_is_added_and_opened() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    crate::catalog::Catalog::create(&path)?;
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside)?;
    let raw = outside.join("photo.ARW");
    std::fs::write(&raw, b"identity fixture")?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    // As if dropped on the window.
    editor.open(raw.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.waiting.photo_waits() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        editor.events(&ctx);
    }
    let raw = raw.canonicalize()?;
    let id = editor
        .library
        .as_ref()
        .and_then(|l| l.session.photos().iter().find(|p| p.path == raw))
        .map(|p| p.id);
    assert!(id.is_some(), "the photo's folder was added to the catalog");
    assert_eq!(editor.document.catalog_photo, id);
    assert!(editor.module == Module::Develop);
    Ok(())
}

#[test]
fn a_dropped_folder_is_added_itself_not_its_parent() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    crate::catalog::Catalog::create(&path)?;
    let parent = dir.path().join("card");
    let trip = parent.join("trip");
    std::fs::create_dir_all(&trip)?;
    std::fs::write(trip.join("in.ARW"), b"identity fixture")?;
    std::fs::write(parent.join("beside.ARW"), b"identity fixture")?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    // As if dropped on the window.
    editor.open(trip);
    settle_catalog(&mut editor, &ctx);
    let names: Vec<_> = editor
        .library
        .as_ref()
        .map(|l| {
            l.session
                .photos()
                .iter()
                .map(|p| p.filename.clone())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(names, ["in.ARW"]);
    assert!(!editor.waiting.photo_waits());
    assert!(editor.module == Module::Library);
    Ok(())
}

#[test]
fn dropping_a_folder_shows_it_instead_of_unrelated_catalog_photos() -> anyhow::Result<()> {
    let (dir, mut editor, _) = editor_with_catalog(&["unrelated.ARW"])?;
    let folder = dir.path().join("trip");
    std::fs::create_dir_all(folder.join("day1"))?;
    std::fs::write(folder.join("one.ARW"), b"one")?;
    std::fs::write(folder.join("day1/two.ARW"), b"two")?;
    let ctx = editor.context.clone();
    editor.dropped(vec![folder.clone()]);
    settle_catalog(&mut editor, &ctx);
    let library = editor.library.as_ref().unwrap();
    assert_eq!(library.session.photos().len(), 3);
    let mut shown: Vec<_> = library
        .shown()
        .iter()
        .map(|id| library.photo(*id).unwrap().filename.clone())
        .collect();
    shown.sort();
    assert_eq!(shown, ["one.ARW", "two.ARW"]);
    assert!(!library.source_key().is_empty());
    // Dropping an already catalogued subfolder narrows the source too.
    editor.dropped(vec![folder.join("day1")]);
    settle_catalog(&mut editor, &ctx);
    let library = editor.library.as_ref().unwrap();
    assert_eq!(library.shown().len(), 1);
    assert_eq!(
        library.photo(library.shown()[0]).unwrap().filename,
        "two.ARW"
    );
    Ok(())
}

#[test]
fn dropping_a_relocated_folders_parent_does_not_select_its_logical_siblings() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("photos");
    for (folder, name) in [("a/b", "inside.ARW"), ("a/c", "outside.ARW")] {
        std::fs::create_dir_all(root.join(folder))?;
        std::fs::write(root.join(folder).join(name), name.as_bytes())?;
    }
    let path = dir.path().join("test.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&path)?;
    catalog.add_folder(&root)?;
    let moved = catalog
        .folders()?
        .into_iter()
        .find(|f| f.relative == "a/b")
        .unwrap()
        .id;
    let incoming = dir.path().join("incoming");
    std::fs::create_dir(&incoming)?;
    std::fs::rename(root.join("a/b"), incoming.join("x"))?;
    catalog.relink_folder(moved, &incoming.join("x"))?;
    drop(catalog);
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, Default::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    editor.dropped(vec![incoming]);
    settle_catalog(&mut editor, &ctx);
    let library = editor.library.as_ref().unwrap();
    let shown: Vec<_> = library
        .shown()
        .iter()
        .map(|id| library.photo(*id).unwrap().filename.clone())
        .collect();
    assert_eq!(
        shown,
        ["inside.ARW"],
        "source={} roots={:?} folders={:?} status={}",
        library.source_key(),
        library.session.roots(),
        library.session.folders(),
        editor.messages.line()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn dropping_a_folder_mapped_through_a_symlink_selects_it_and_clears_old_filters()
-> anyhow::Result<()> {
    let (dir, mut editor, _) = editor_with_catalog(&["outside.ARW"])?;
    let real = dir.path().join("real");
    let alias = dir.path().join("alias");
    std::fs::create_dir(&real)?;
    std::fs::write(real.join("inside.ARW"), b"inside")?;
    std::os::unix::fs::symlink(&real, &alias)?;
    let library = editor.library.as_mut().unwrap();
    let old_root = library.session.roots()[0].0;
    library.session.catalog_mut_for_tests().add_folder(&real)?;
    let root = library
        .session
        .catalog()
        .folders()?
        .into_iter()
        .find(|f| f.path == real.canonicalize().unwrap())
        .unwrap()
        .root;
    library
        .session
        .catalog_mut_for_tests()
        .relink_root(root, &alias)?;
    library.refresh()?;
    library.click_folder_for_tests(&format!("root:{old_root}"));
    editor.session.layout_mut().query = "outside".into();
    let ctx = editor.context.clone();
    editor.dropped(vec![alias]);
    settle_catalog(&mut editor, &ctx);
    let library = editor.library.as_ref().unwrap();
    let shown: Vec<_> = library
        .shown()
        .iter()
        .map(|id| library.photo(*id).unwrap().filename.clone())
        .collect();
    assert_eq!(shown, ["inside.ARW"]);
    assert_eq!(library.source_key(), format!("root:{root}"));
    Ok(())
}

#[test]
fn dropping_several_folders_adds_each_of_them() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    crate::catalog::Catalog::create(&path)?;
    let (trip, home, card) = (
        dir.path().join("trip"),
        dir.path().join("home"),
        dir.path().join("card"),
    );
    for folder in [&trip, &home, &card] {
        std::fs::create_dir(folder)?;
    }
    std::fs::write(trip.join("t.ARW"), b"trip")?;
    std::fs::write(home.join("h.ARW"), b"home")?;
    std::fs::write(card.join("c1.ARW"), b"card 1")?;
    std::fs::write(card.join("c2.ARW"), b"card 2")?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    let names = |editor: &Editor| -> Vec<String> {
        let mut names: Vec<String> = editor
            .library
            .as_ref()
            .map(|l| {
                l.session
                    .photos()
                    .iter()
                    .map(|p| p.filename.clone())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    };

    editor.dropped(vec![trip, home]);
    // Writing the catalog: closing the window waits for it.
    assert!(editor.activity.is_changing_folder());
    settle_catalog(&mut editor, &ctx);
    assert_eq!(names(&editor), ["h.ARW", "t.ARW"]);

    // Two photos of one folder add it once, and open neither in Develop.
    editor.dropped(vec![card.join("c1.ARW"), card.join("c2.ARW")]);
    settle_catalog(&mut editor, &ctx);
    assert_eq!(names(&editor), ["c1.ARW", "c2.ARW", "h.ARW", "t.ARW"]);
    let library = editor.library.as_ref().unwrap();
    let mut shown: Vec<_> = library
        .shown()
        .iter()
        .map(|id| library.photo(*id).unwrap().filename.clone())
        .collect();
    shown.sort();
    assert_eq!(shown, ["c1.ARW", "c2.ARW"]);

    assert!(editor.module == Module::Library);
    assert_eq!(editor.document.catalog_photo, None);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_dropped_folder_that_fails_leaves_the_others_added() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    crate::catalog::Catalog::create(&path)?;
    let (good, locked) = (dir.path().join("good"), dir.path().join("locked"));
    std::fs::create_dir(&good)?;
    std::fs::create_dir(&locked)?;
    std::fs::write(good.join("g.ARW"), b"good")?;
    std::fs::write(locked.join("l.ARW"), b"locked")?;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))?;
    // Root reads it anyway; nothing to test then.
    if std::fs::read_dir(&locked).is_ok() {
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))?;
        return Ok(());
    }
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    editor.dropped(vec![good, locked.clone()]);
    settle_catalog(&mut editor, &ctx);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))?;
    let library = editor.library.as_ref().unwrap();
    let names: Vec<_> = library
        .session
        .photos()
        .iter()
        .map(|p| &p.filename)
        .collect();
    assert_eq!(names, ["g.ARW"]);
    assert!(
        editor.messages.line().contains("Not added: locked"),
        "{}",
        editor.messages.line()
    );
    Ok(())
}

#[test]
fn removing_a_folder_closes_its_photo_and_leaves_the_files() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    let photos = dir.path().join("photos");
    std::fs::create_dir_all(photos.join("trip"))?;
    std::fs::write(photos.join("home.ARW"), b"identity fixture")?;
    std::fs::write(photos.join("trip/away.ARW"), b"identity fixture 2")?;
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx)?));
    let library = editor.library.as_ref().unwrap();
    let away = library
        .session
        .photos()
        .iter()
        .find(|p| p.filename == "away.ARW")
        .unwrap()
        .id;
    let trip: std::collections::HashSet<_> = library
        .session
        .folders()
        .iter()
        .filter(|f| f.relative.trim_end_matches('/') == "trip")
        .map(|f| f.id)
        .collect();
    assert_eq!(trip.len(), 1);
    editor.document.catalog_photo = Some(away);
    editor.module = Module::Develop;
    // Still loading, and the locked reference too.
    let (loading, _) = editor.document.start_load();
    editor.reference.photo = Some(away);
    editor.reference.locked = true;

    // Chosen in another catalog, confirmed after this one opened: refused.
    let removal = |catalog| library::FolderRemoval {
        catalog,
        name: "trip".into(),
        folders: trip.clone(),
        photos: 1,
        empty_root: None,
        exported: None,
    };
    let other = crate::catalog::CatalogLocation::File(dir.path().join("other.rawmakase"));
    editor.remove_folders(&removal(other));
    assert_eq!(editor.library.as_ref().unwrap().session.photos().len(), 2);
    assert!(
        editor.messages.line().contains("another catalog"),
        "{}",
        editor.messages.line()
    );

    editor.remove_folders(&removal(crate::catalog::CatalogLocation::File(path)));
    let names: Vec<_> = editor
        .library
        .as_ref()
        .unwrap()
        .session
        .photos()
        .iter()
        .map(|p| p.filename.clone())
        .collect();
    assert_eq!(names, ["home.ARW"]);
    assert_eq!(editor.document.catalog_photo, None);
    assert!(editor.module == Module::Library);
    assert!(
        editor.messages.line().contains("still on disk"),
        "{}",
        editor.messages.line()
    );
    // The load's late results are no longer its.
    assert!(!editor.document.loading());
    assert_ne!(editor.document.load_id(), loading);
    assert_eq!(editor.reference.photo, None);
    assert!(photos.join("trip/away.ARW").is_file());
    Ok(())
}

#[test]
fn a_first_launch_opens_a_new_catalog_beside_the_session() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let session = || crate::app::preferences::session::Session {
        no_update_checks: true,
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, session(), Some(dir.path().join("session.json")));
    settle_catalog(&mut editor, &ctx);
    let catalog = dir.path().join("Photos.rawmakase");
    assert!(catalog.is_file());
    assert_eq!(
        editor
            .library
            .as_ref()
            .map(|l| l.session.catalog().location()),
        Some(&crate::catalog::CatalogLocation::File(catalog.clone()))
    );
    // The next launch opens it as the last catalog, not as a new one.
    editor.save_session()?;
    drop(editor);
    let saved: crate::app::preferences::session::Session =
        serde_json::from_slice(&std::fs::read(dir.path().join("session.json"))?)?;
    assert_eq!(saved.last_path.as_ref(), Some(&catalog));

    // A catalog used before that is missing now (on an unplugged drive) is
    // not replaced by a new one.
    let elsewhere = tempfile::tempdir()?;
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session {
            last_path: Some(elsewhere.path().join("Offline.rawmakase")),
            ..session()
        },
        Some(elsewhere.path().join("session.json")),
    );
    settle_catalog(&mut editor, &ctx);
    assert!(editor.library.is_none());
    assert!(!elsewhere.path().join("Photos.rawmakase").exists());
    // Saving the session meanwhile keeps naming it, so the next launch is
    // not taken for a first one.
    editor.save_session()?;
    let saved: crate::app::preferences::session::Session =
        serde_json::from_slice(&std::fs::read(elsewhere.path().join("session.json"))?)?;
    assert_eq!(
        saved.last_path,
        Some(elsewhere.path().join("Offline.rawmakase"))
    );
    Ok(())
}

#[test]
fn a_virtual_copy_made_in_develop_keeps_the_unsaved_edit_and_opens() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let l = library::Library::load(&catalog, ctx.clone())?;
    let id = l.session.photos()[0].id;
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(l));
    editor.module = Module::Develop;
    editor.document.catalog_photo = Some(id);
    editor.document.opened_at(photo.clone());
    editor.document.edit.setup_mut().exposure = 0.7;
    editor.document.edit.save_state_mut().mark_changed();
    editor.virtual_copy(library::CopyAction::Create(id));
    let library = editor.library.as_ref().unwrap();
    let copy = library
        .session
        .photos()
        .iter()
        .find(|p| p.id != id)
        .unwrap();
    assert_eq!((copy.master, copy.copy_name.as_str()), (Some(id), "Copy 1"));
    assert_eq!(library.selected(), Some(copy.id));
    assert_eq!(editor.document.catalog_photo, Some(copy.id));
    for photo_id in [id, copy.id] {
        let saved = library
            .session
            .catalog()
            .load_edit(photo_id, &photo)?
            .unwrap();
        assert_eq!(saved.recipe.exposure, 0.7);
    }
    Ok(())
}

#[test]
fn removing_a_copy_from_the_library_stays_in_the_library() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("image.ARW"), b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut l = library::Library::load(&catalog, ctx.clone())?;
    let master = l.session.photos()[0].id;
    let copy = l.create_virtual_copy(master)?.value;
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(l));
    // The copy was last open in Develop; the user is back in the Library.
    editor.document.catalog_photo = Some(copy);
    editor.module = Module::Library;
    editor.modal = Some(Modal::RemoveCopy(copy));
    // Return alone never confirms the dialog.
    let input = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| editor.remove_copy_window(ui.ctx()));
    output.textures_delta.clear();
    assert!(matches!(editor.modal, Some(Modal::RemoveCopy(id)) if id == copy));
    assert!(editor.library.as_ref().unwrap().photo(copy).is_some());
    editor.modal = None;
    editor.remove_virtual_copy(copy);
    assert!(editor.module == Module::Library);
    assert_eq!(editor.document.catalog_photo, None);
    let library = editor.library.as_ref().unwrap();
    assert!(library.photo(copy).is_none());
    assert_eq!(library.selected(), Some(master));
    Ok(())
}

/// A virtual copy still loading in the Library's Loupe, removed: no master opens in
/// its place, so the copy's load must not refill the document once it reports, and
/// what was shown of it goes, as when any other open photo is closed.
#[test]
fn removing_a_copy_still_loading_outside_develop_ends_its_load() -> anyhow::Result<()> {
    let (_dir, _ctx, mut editor, [a, _]) = two_photo_editor()?;
    let copy = editor
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(a)?
        .value;
    let path = editor
        .library
        .as_ref()
        .unwrap()
        .photo(a)
        .unwrap()
        .path
        .clone();
    assert!(editor.load_raw(path, Some(copy)));
    assert_eq!(editor.module, Module::Library);
    let ticket = editor.document.load_id();
    assert!(editor.document.loading());
    editor.view.zoom.on = true;
    let presets = editor.presets.revision;
    editor.remove_virtual_copy(copy);
    assert_eq!(editor.document.catalog_photo, None);
    assert_ne!(
        editor.document.load_id(),
        ticket,
        "the copy's load still counts"
    );
    assert!(!editor.document.loading());
    assert!(!editor.view.zoom.on);
    assert!(editor.presets.revision > presets);
    Ok(())
}

#[test]
fn a_copy_name_that_cannot_be_saved_keeps_the_app_from_moving_on() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("image.ARW"), b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut l = library::Library::load(&catalog, ctx.clone())?;
    let copy = l.create_virtual_copy(l.session.photos()[0].id)?.value;
    // A copy that is gone from the catalog cannot be renamed.
    l.session
        .catalog_mut_for_tests()
        .remove_virtual_copy(copy)?;
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(l));
    editor
        .library
        .as_mut()
        .unwrap()
        .set_copy_name_draft(copy, "B&W");
    assert!(!editor.flush());
    editor.library.as_mut().unwrap().discard_drafts();
    assert!(editor.flush());
    Ok(())
}

#[test]
fn opening_a_file_picks_its_master_after_a_copy_is_promoted() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut l = library::Library::load(&catalog, ctx.clone())?;
    let copy = l.create_virtual_copy(l.session.photos()[0].id)?.value;
    l.set_copy_as_master(copy)?;
    let path = l.photo(copy).unwrap().path.clone();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(l));
    assert_eq!(editor.catalog_photo_at(&path), Some(copy));
    Ok(())
}

/// The catalog the editor works on, read again from where `editor_with_catalog` made it.
fn reread(d: &tempfile::TempDir) -> Box<library::Library> {
    Box::new(
        library::Library::load(&d.path().join("test.rawmakase"), egui::Context::default()).unwrap(),
    )
}

/// A failed catalog operation gives up the outside photo waiting for it and the open photo
/// it was to follow, but keeps the launch place: the next catalog still restores it.
#[test]
fn a_failed_catalog_keeps_the_launch_place_and_drops_what_waited_for_it() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.waiting = crate::app::catalog::Waiting::restoring(CatalogPlace {
        source: String::new(),
        photo: Some(ids[1]),
        module: Module::Library,
    });
    e.waiting.open_once_ready(d.path().join("outside.ARW"));
    e.catalog_arrived(
        Err("the catalog could not be read".into()),
        Some((ids[0], None)),
    );
    assert!(!e.waiting.photo_waits());
    assert!(e.waiting.restores());
    e.catalog_ready(Ok(reread(&d)));
    assert!(!e.waiting.restores());
    assert_eq!(e.library.as_ref().unwrap().selected(), Some(ids[1]));
    Ok(())
}

/// The launch place is restored once: a catalog arriving later does not go back to it.
#[test]
fn the_launch_place_is_restored_once() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.waiting = crate::app::catalog::Waiting::restoring(CatalogPlace {
        source: String::new(),
        photo: Some(ids[1]),
        module: Module::Library,
    });
    e.catalog_ready(Ok(reread(&d)));
    assert_eq!(e.library.as_ref().unwrap().selected(), Some(ids[1]));
    e.catalog_ready(Ok(reread(&d)));
    assert_ne!(e.library.as_ref().unwrap().selected(), Some(ids[1]));
    Ok(())
}

/// What a catalog operation says while it runs goes once its result arrives, whether the
/// catalog opened, folders were added or it failed.
#[test]
fn the_catalog_status_goes_when_its_result_arrives() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    let ctx = e.context.clone();
    for result in [
        Event::CatalogReady(Err("broken".into())),
        Event::CatalogReady(Ok(reread(&d))),
        Event::FoldersAdded {
            result: Err("broken".into()),
            source: Vec::new(),
        },
        Event::FolderSyncApplied(Err("broken".into())),
    ] {
        e.tx.send(Event::CatalogWorking("Opening the catalog…".into()))
            .unwrap();
        e.events(&ctx);
        assert_eq!(e.catalog_work.as_deref(), Some("Opening the catalog…"));
        e.tx.send(result).unwrap();
        e.events(&ctx);
        assert!(e.catalog_work.is_none());
    }
    Ok(())
}

/// A photo opened while the catalog is still opening waits for it, then has its folder
/// added (a second catalog arriving) and opens.
#[test]
fn a_photo_opened_while_the_catalog_opens_waits_then_is_added_and_opened() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    crate::catalog::Catalog::create(&path)?;
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside)?;
    let raw = outside.join("photo.ARW");
    std::fs::write(&raw, b"identity fixture")?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    // The catalog is opening (its dialog), and the photo comes meanwhile.
    assert!(editor.activity.begin_dialog());
    editor.open(raw.clone());
    assert!(editor.waiting.photo_waits());
    editor.catalog_ready(Ok(Box::new(library::Library::load(&path, ctx.clone())?)));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.waiting.photo_waits() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        editor.events(&ctx);
    }
    let raw = raw.canonicalize()?;
    let id = editor
        .library
        .as_ref()
        .and_then(|l| l.session.photos().iter().find(|p| p.path == raw))
        .map(|p| p.id);
    assert!(id.is_some());
    assert_eq!(editor.document.catalog_photo, id);
    Ok(())
}

/// A photo whose folder was added but which is still not in the catalog is given up,
/// with a message, without adding the folder again.
#[test]
fn a_photo_still_missing_once_its_folder_is_added_is_given_up() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    e.waiting
        .open_once_added(d.path().join("photos/missing.RAF"));
    e.catalog_ready(Ok(reread(&d)));
    assert!(!e.waiting.photo_waits());
    assert!(!e.activity.is_busy());
    assert!(
        e.messages.line().contains("could not be added"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

/// After a synchronization the open photo stays open when the catalog read again still
/// lists it where its file was found; listed somewhere else, it closes.
#[test]
fn the_open_photo_stays_after_a_sync_only_where_its_file_was_found() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.catalog_arrived(Ok(reread(&d)), Some((ids[0], None)));
    assert_eq!(e.document.catalog_photo, Some(ids[0]));
    e.catalog_arrived(
        Ok(reread(&d)),
        Some((ids[0], Some(d.path().join("elsewhere/a.RAF")))),
    );
    assert_eq!(e.document.catalog_photo, None);
    Ok(())
}
