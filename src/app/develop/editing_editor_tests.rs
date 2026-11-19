//! Editor tests of `editing`: the whole editor driven through its frames.
use crate::app::*;
use crate::camera_data::Metadata;
use crate::catalog::PhotoId;
use std::sync::Arc;

#[test]
fn develop_history_survives_reopening_the_photo() -> anyhow::Result<()> {
    use worker::LoadedHeader;
    let dir = tempfile::tempdir()?;
    let raw = dir.path().join("photo.ARW");
    std::fs::write(&raw, b"identity fixture")?;
    let path = dir.path().join("photos.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&path)?;
    catalog.add_folder(dir.path())?;
    let id = catalog.photos()?[0].id;
    drop(catalog);
    let ctx = egui::Context::default();
    let open = |editor: &mut Editor| {
        editor.document.reset(Some(id));
        let (generation, _) = editor.document.start_load();
        editor
            .tx
            .send(Event::Header(Box::new(LoadedHeader {
                from: Default::default(),
                id: generation,
                file: crate::storage::Identity::read(&raw.clone()).ok(),
                path: raw.clone(),
                metadata: Metadata::default(),
                recipe: Recipe::default(),
                export: Default::default(),
                status: "Original".into(),
            })))
            .unwrap();
        editor.events(&ctx);
    };
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    open(&mut editor);
    for exposure in [0.5, 1.] {
        let before = editor.document.edit.recipe().clone();
        editor.document.edit.setup_mut().exposure = exposure;
        editor.commit_edit(before, None);
    }
    assert!(editor.flush());
    // Another photo, or a restart, starts a new document.
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    open(&mut editor);
    assert_eq!(editor.document.edit.recipe().exposure, 1.);
    assert_eq!(editor.document.edit.history().steps().0.len(), 2);
    let mut recipe = editor.document.edit.recipe().clone();
    assert!(editor.document.edit.history_mut().undo(&mut recipe));
    assert_eq!(recipe.exposure, 0.5);
    Ok(())
}

/// A rendered file's saved edit belongs to its file as a RAW's does: once the
/// file is rewritten, opening it again starts from the file, with the saved edit
/// protected rather than shown or replaced.
#[test]
fn an_edited_jpeg_whose_file_changed_reopens_from_the_file() -> anyhow::Result<()> {
    use worker::LoadedHeader;
    for (name, source) in [
        ("photo.ARW", crate::camera_data::Source::Raw),
        ("photo.jpg", crate::camera_data::Source::Rendered),
    ] {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join(name);
        let write = |side: u32| -> anyhow::Result<()> {
            if source == crate::camera_data::Source::Rendered {
                image::RgbImage::from_pixel(side, side, image::Rgb([90, 120, 150])).save(&file)?;
            } else {
                std::fs::write(&file, vec![7u8; side as usize])?;
            }
            Ok(())
        };
        write(16)?;
        let path = dir.path().join("photos.rawmakase");
        let mut catalog = crate::catalog::Catalog::create(&path)?;
        catalog.add_folder(dir.path())?;
        let id = catalog.photos()?[0].id;
        let saved = Recipe {
            exposure: 1.,
            ..Default::default()
        };
        catalog.save_edit(
            id,
            &file,
            &crate::storage::Identity::read(&file)?,
            &saved,
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )?;
        drop(catalog);
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
        let open = |editor: &mut Editor| {
            editor.document.reset(Some(id));
            let (generation, _) = editor.document.start_load();
            editor
                .tx
                .send(Event::Header(Box::new(LoadedHeader {
                    from: Default::default(),
                    id: generation,
                    file: crate::storage::Identity::read(&file.clone()).ok(),
                    path: file.clone(),
                    metadata: Metadata {
                        source,
                        ..Default::default()
                    },
                    recipe: Recipe::default(),
                    export: Default::default(),
                    status: "Original".into(),
                })))
                .unwrap();
            editor.events(&ctx);
        };
        open(&mut editor);
        assert_eq!(editor.document.edit.recipe().exposure, 1., "{name}");
        assert!(!editor.document.edit.save_state().is_protected(), "{name}");

        write(32)?;
        open(&mut editor);
        assert_eq!(editor.document.edit.recipe().exposure, 0., "{name}");
        assert!(editor.document.edit.save_state().is_protected(), "{name}");
        assert!(
            editor.document.lightroom_notice.contains("Photo changed"),
            "{name}: {}",
            editor.document.lightroom_notice
        );
    }
    Ok(())
}

/// A stored History this release can't read (a newer release's, or one holding HDR
/// states it refuses) is never replaced by a save: the photo opens with its edit,
/// protected and saying why, and the History stays in the catalog unchanged.
#[test]
fn an_unreadable_history_protects_the_edit() -> anyhow::Result<()> {
    use std::io::Write as _;
    use worker::LoadedHeader;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("photo.ARW");
    std::fs::write(&file, vec![7u8; 16])?;
    let path = dir.path().join("photos.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&path)?;
    catalog.add_folder(dir.path())?;
    let id = catalog.photos()?[0].id;
    let saved = Recipe {
        exposure: 1.,
        ..Default::default()
    };
    catalog.save_edit(
        id,
        &file,
        &crate::storage::Identity::read(&file)?,
        &saved,
        &Default::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    drop(catalog);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
    z.write_all(br#"{"version": 99}"#)?;
    let blob = z.finish()?;
    rusqlite::Connection::open(&path)?.execute(
        "INSERT INTO develop_history(photo, data) VALUES (?, ?)",
        rusqlite::params![id.0, blob],
    )?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    editor.document.reset(Some(id));
    let (generation, _) = editor.document.start_load();
    editor
        .tx
        .send(Event::Header(Box::new(LoadedHeader {
            from: Default::default(),
            id: generation,
            file: crate::storage::Identity::read(&file).ok(),
            path: file.clone(),
            metadata: Metadata::default(),
            recipe: Recipe::default(),
            export: Default::default(),
            status: "Original".into(),
        })))
        .unwrap();
    editor.events(&ctx);
    assert_eq!(editor.document.edit.recipe().exposure, 1.);
    assert!(editor.document.edit.save_state().is_protected());
    assert!(
        editor.document.lightroom_notice.contains("History"),
        "{}",
        editor.document.lightroom_notice
    );
    let stored: Vec<u8> = rusqlite::Connection::open(&path)?.query_row(
        "SELECT data FROM develop_history WHERE photo=?",
        [id.0],
        |row| row.get(0),
    )?;
    assert_eq!(stored, blob);
    Ok(())
}

/// A photo with no saved edit has no stored identity to protect it: the edit is
/// saved for the file it was opened as, so a file replaced while it is open gets
/// none of it, whether autosave or a save before navigation writes it. The edit
/// stays unsaved.
#[test]
fn a_first_save_after_the_file_was_replaced_is_refused() -> anyhow::Result<()> {
    use worker::LoadedHeader;
    for autosave in [false, true] {
        let dir = tempfile::tempdir()?;
        let raw = dir.path().join("photo.ARW");
        std::fs::write(&raw, b"identity fixture")?;
        let path = dir.path().join("photos.rawmakase");
        let mut catalog = crate::catalog::Catalog::create(&path)?;
        catalog.add_folder(dir.path())?;
        let id = catalog.photos()?[0].id;
        drop(catalog);
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
        editor.document.reset(Some(id));
        let (generation, _) = editor.document.start_load();
        editor
            .tx
            .send(Event::Header(Box::new(LoadedHeader {
                from: Default::default(),
                id: generation,
                file: crate::storage::Identity::read(&raw.clone()).ok(),
                path: raw.clone(),
                metadata: Metadata::default(),
                recipe: Recipe::default(),
                export: Default::default(),
                status: "Original".into(),
            })))
            .unwrap();
        editor.events(&ctx);

        std::fs::write(&raw, b"another photo, same name")?;
        let before = editor.document.edit.recipe().clone();
        editor.document.edit.setup_mut().exposure = 1.;
        editor.commit_edit(before, None);
        if autosave {
            *editor.document.edit.save_state_mut() = save_state::SaveState::Pending(
                std::time::Instant::now() - std::time::Duration::from_secs(1),
            );
            editor.autosave(&ctx);
            while editor.autosave.busy() {
                std::thread::sleep(std::time::Duration::from_millis(5));
                editor.autosave(&ctx);
            }
        } else {
            assert!(!editor.flush(), "autosave {autosave}");
        }
        let catalog = editor.library.as_ref().unwrap().session.catalog();
        assert!(
            catalog.edit_record(id)?.saved(&raw)?.is_none(),
            "autosave {autosave}: the replaced file got the edit"
        );
        assert!(
            editor.document.edit.save_state().needs_save(),
            "autosave {autosave}"
        );
    }
    Ok(())
}

#[test]
fn navigation_during_an_edit_frame_cannot_dirty_the_next_document() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.document.edit.setup_mut().exposure = 1.25;
    editor.presets.preview = Some(editor.document.edit.recipe().clone());
    editor.view.crop_drag = Some(crate::app::develop::tools::crop_drag::CropDrag::new(
        [0., 0., 1., 1.],
        None,
        false,
    ));
    let frame = editor.begin_edit_frame();
    // This does not need a valid RAW: navigation resets state before asynchronous decoding.
    editor.open_raw(
        std::path::PathBuf::from("missing-navigation-fixture.ARW"),
        None,
    );
    editor.finish_edit_frame(frame, &ctx);
    assert!(!editor.document.edit.save_state().needs_save());
    assert!(!editor.document.edit.history().can_undo());
    assert!(!editor.document.edit.history().in_gesture());
    assert!(editor.document.full().is_none());
    assert!(editor.presets.preview.is_none());
    assert!(editor.view.crop_drag.is_none());
}

#[test]
fn changing_a_setting_in_a_panel_that_is_off_turns_it_back_on() {
    use crate::model::panels::{Panel, PanelState};
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let frame = |editor: &mut Editor, edit: &dyn Fn(&mut crate::model::recipe::Recipe)| {
        let started = editor.begin_edit_frame();
        edit(editor.document.edit.setup_mut());
        editor.finish_edit_frame(started, &ctx);
    };
    frame(&mut editor, &|r| {
        r.panels.set(Panel::BlackWhiteMix, PanelState::Off);
        r.panels.set(Panel::Detail, PanelState::Off);
    });
    // An edit made anywhere in the frame, as B&W Auto is after the panel is drawn.
    frame(&mut editor, &|r| r.effects.gray_mix[0] = 0.3);
    let r = editor.document.edit.recipe();
    assert_eq!(r.panels.state(Panel::BlackWhiteMix), PanelState::On);
    // Other panels stay off, and render, so export, at their defaults.
    assert_eq!(r.panels.state(Panel::Detail), PanelState::Off);
    frame(&mut editor, &|r| r.exposure = 0.5);
    assert_eq!(
        editor.document.edit.recipe().panels.state(Panel::Detail),
        PanelState::Off
    );
    // The edit and its switch are one step, undone together.
    editor.undo();
    editor.undo();
    let started = editor.begin_edit_frame();
    editor.finish_edit_frame(started, &ctx);
    let r = editor.document.edit.recipe();
    assert_eq!(r.effects.gray_mix[0], 0.);
    assert_eq!(r.panels.state(Panel::BlackWhiteMix), PanelState::Off);
}

/// Develop opens every photo with the edit the shared resolver gives it, so a photo
/// synchronized or exported without being opened is developed as it would be on
/// screen: a saved edit with its masks, a Lightroom edit, and the raw defaults.
#[test]
fn develop_opens_photos_with_the_edit_the_catalog_resolves() -> anyhow::Result<()> {
    use crate::edits::{self, Origin};
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/charts/synthetic-d65.dng");
    for name in ["a.dng", "b.dng", "c.dng"] {
        std::fs::copy(&chart, photos.join(name))?;
    }
    let catalog = dir.path().join("test.rawmakase");
    let mut c = crate::catalog::Catalog::create(&catalog)?;
    c.add_folder(&photos)?;
    let ids: Vec<(PhotoId, std::path::PathBuf)> =
        c.photos()?.into_iter().map(|p| (p.id, p.path)).collect();
    let metadata = crate::photo::open(&ids[0].1)?.metadata;
    let (profiles, _) = crate::camera_profiles::installed(&metadata);
    let mut saved = Recipe::with_profiles(&metadata, &profiles);
    saved.exposure = 0.4;
    saved.masks.push(crate::model::masks::MaskGroup {
        components: vec![crate::model::masks::MaskComponent::new(
            crate::model::masks::MaskShape::Radial {
                center: [0.5, 0.5],
                radii: [0.2, 0.1],
                angle: 0.,
                feather: 0.5,
            },
        )],
        adjust: crate::model::masks::LocalAdjust {
            shadows: 0.5,
            ..Default::default()
        },
        ..Default::default()
    });
    c.save_edit(
        ids[0].0,
        &ids[0].1,
        &crate::storage::Identity::read(&ids[0].1)?,
        &saved,
        &Default::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    rusqlite::Connection::open(&catalog)?.execute(
        "UPDATE photos SET lightroom_develop='s = { Exposure2012 = 0.25, Contrast2012 = 10 }' WHERE id=?",
        [ids[1].0.0],
    )?;
    drop(c);
    let ctx = egui::Context::default();
    let library = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    // Not this machine's own raw defaults. Adobe Default needs no preset, so the
    // preset scan the editor starts leaves them as they are.
    editor.raw_defaults = Arc::new(crate::raw_defaults::DevelopDefaults::with_presets(
        Default::default(),
        |_| None,
    ));
    editor.library = Some(Box::new(library));
    for ((id, path), origin) in ids
        .iter()
        .zip([Origin::Saved, Origin::Lightroom, Origin::Defaults])
    {
        editor.open_raw(path.clone(), Some(*id));
        // Opened once the decode is in: the header and profiles come before it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while editor.document.full().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "{} not opened",
                path.display()
            );
            editor.events(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let record = editor
            .library
            .as_ref()
            .unwrap()
            .session
            .catalog()
            .edit_record(*id)?;
        let resolved = edits::resolve(
            &record,
            path,
            editor.document.metadata.as_ref().unwrap(),
            &editor.document.profiles,
            &editor.raw_defaults,
        )?;
        assert_eq!(resolved.origin, origin);
        assert_eq!(
            *editor.document.edit.recipe(),
            resolved.recipe,
            "{origin:?}"
        );
        assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    }
    Ok(())
}
