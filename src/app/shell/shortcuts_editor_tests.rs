//! Editor tests of `shortcuts`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use eframe::egui::{Pos2, Rect};

#[test]
fn catalog_metadata_keys_work_in_both_modules_without_zoom_or_dialog_edits() -> anyhow::Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.RAF"), b"fixture")?;
    std::fs::write(photos.join("b.RAF"), b"fixture")?;
    let path = d.path().join("test.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&path)?;
    catalog.add_folder(&photos)?;
    drop(catalog);
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let mut library = crate::app::library::Library::load(&path, ctx.clone())?;
    let ids: Vec<_> = library.session.photos().iter().map(|p| p.id).collect();
    library.select(Some(ids[0]));
    e.library = Some(Box::new(library));
    for (library_mode, key, expected_rating, expected_flag) in [
        (true, egui::Key::Num5, 5, 0),
        (true, egui::Key::P, 5, 1),
        (false, egui::Key::Num1, 1, 0),
        (false, egui::Key::X, 1, -1),
        (false, egui::Key::U, 1, 0),
        (false, egui::Key::Num0, 0, 0),
    ] {
        e.module = if library_mode {
            Module::Library
        } else {
            Module::Develop
        };
        e.document.catalog_photo = Some(ids[1]);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
        let id = ids[usize::from(!library_mode)];
        let photo = e.library.as_ref().unwrap().photo(id).unwrap();
        assert_eq!((photo.rating, photo.flag), (expected_rating, expected_flag));
        assert!(!e.view.zoom.on);
    }
    assert!(e.activity.begin_dialog());
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Num5,
                physical_key: Some(egui::Key::Num5),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
    assert_eq!(e.library.as_ref().unwrap().photo(ids[1]).unwrap().rating, 0);
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().rating, 5);
    let reopened = crate::catalog::Catalog::open(&path)?;
    assert_eq!(
        reopened
            .photos()?
            .iter()
            .find(|p| p.id == ids[0])
            .unwrap()
            .flag,
        1
    );
    Ok(())
}

#[test]
fn loupe_keys_change_only_the_photo_shown_and_auto_advance_moves_on() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[1]);
    press(&mut e, egui::Key::E, egui::Modifiers::NONE);
    assert!(e.library.as_ref().unwrap().loupe_open());
    // Cmd+Up picks the photo shown, not the rest of the selection.
    press(&mut e, egui::Key::ArrowUp, egui::Modifiers::COMMAND);
    let library = e.library.as_ref().unwrap();
    let flags: Vec<_> = ids
        .iter()
        .map(|id| library.photo(*id).unwrap().flag)
        .collect();
    assert_eq!(flags, [0, 1, 0]);
    press(&mut e, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!e.library.as_ref().unwrap().loupe_open());
    // With Auto Advance a key moves on as Shift would.
    e.session.auto_advance = true;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    press(&mut e, egui::Key::Num3, egui::Modifiers::NONE);
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.photo(ids[0]).unwrap().rating, 3);
    assert_eq!(library.selected(), Some(ids[1]));
    Ok(())
}

#[test]
fn double_click_in_the_loupe_goes_back_to_the_grid() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    e.library.as_mut().unwrap().open_loupe();
    let ctx = e.context.clone();
    let at = Pos2::new(600., 300.);
    let click = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let frame = |e: &mut Editor, events: Vec<egui::Event>, time: f64| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
    };
    frame(&mut e, vec![egui::Event::PointerMoved(at)], 0.0);
    frame(&mut e, vec![click(true)], 0.1);
    frame(&mut e, vec![click(false)], 0.15);
    frame(&mut e, vec![click(true)], 0.2);
    frame(&mut e, vec![click(false)], 0.25);
    frame(&mut e, vec![], 0.3);
    assert!(!e.library.as_ref().unwrap().loupe_open());
    // A click just before a double-click makes egui count a triple click.
    e.library.as_mut().unwrap().open_loupe();
    for (pressed, time) in [
        (true, 1.0),
        (false, 1.05),
        (true, 1.2),
        (false, 1.25),
        (true, 1.4),
        (false, 1.45),
    ] {
        frame(&mut e, vec![click(pressed)], time);
    }
    frame(&mut e, vec![], 1.5);
    assert!(!e.library.as_ref().unwrap().loupe_open());
    Ok(())
}

#[test]
fn preferences_double_click_does_not_close_the_developed_loupe() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    e.library.as_mut().unwrap().open_loupe();
    // Keep the online RAW on the Develop-backed Loupe path without decoding
    // the fixture or racing a background load.
    e.document.catalog_photo = Some(ids[0]);
    e.loupe_tried = Some(ids[0]);
    e.preferences.open = true;
    let ctx = e.context.clone();
    let at = Pos2::new(600., 300.);
    let click = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let frame = |e: &mut Editor, events: Vec<egui::Event>, time: f64| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
    };
    frame(&mut e, vec![egui::Event::PointerMoved(at)], 0.0);
    for (pressed, time) in [(true, 0.1), (false, 0.15), (true, 0.2), (false, 0.25)] {
        frame(&mut e, vec![click(pressed)], time);
    }
    assert!(e.preferences.open);
    assert!(e.library.as_ref().unwrap().loupe_open());
    // The same gesture still closes this Loupe once Preferences is gone.
    e.preferences.open = false;
    frame(&mut e, vec![], 1.0);
    frame(&mut e, vec![], 1.05);
    for (pressed, time) in [(true, 1.1), (false, 1.15), (true, 1.2), (false, 1.25)] {
        frame(&mut e, vec![click(pressed)], time);
    }
    assert!(!e.library.as_ref().unwrap().loupe_open());
    Ok(())
}

#[test]
fn quick_collection_toggles_shows_clears_and_undoes() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[1]);
    press(&mut e, egui::Key::B, egui::Modifiers::NONE);
    press(&mut e, egui::Key::B, egui::Modifiers::COMMAND);
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.shown(), ids[..2]);
    assert_eq!(library.source_name(), "Quick Collection");
    // B again on photos all in it takes them out.
    press(&mut e, egui::Key::B, egui::Modifiers::NONE);
    assert!(e.library.as_ref().unwrap().shown().is_empty());
    e.undo();
    assert_eq!(e.library.as_ref().unwrap().shown(), ids[..2]);
    press(
        &mut e,
        egui::Key::B,
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
    );
    assert!(e.library.as_ref().unwrap().shown().is_empty());
    e.undo();
    assert_eq!(e.library.as_ref().unwrap().shown(), ids[..2]);
    // Kept in the catalog.
    let reopened =
        crate::catalog::Catalog::open(e.library.as_ref().unwrap().session.catalog().location())?;
    let quick = reopened
        .collections()?
        .into_iter()
        .find(|c| c.name == crate::catalog::QUICK_COLLECTION)
        .unwrap();
    assert_eq!(reopened.collection_photos()?[&quick.id].len(), 2);
    Ok(())
}

#[test]
fn shift_cmd_c_and_v_reach_develop_though_winit_reads_them_as_the_clipboard() {
    use eframe::App as _;
    let ctx = egui::Context::default();
    let (mut editor, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    // egui-winit turns Cmd+C and Cmd+V with any other modifier into Copy and
    // Paste, without the key: what Shift+Cmd+C and Shift+Cmd+V arrive as.
    let frame = |editor: &mut Editor, event: egui::Event, modifiers: egui::Modifiers| {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            // Released in the same frame, as a quick shortcut can be. egui-winit
            // drops only the press: the release comes through.
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                event.clone(),
                egui::Event::Key {
                    key: match event {
                        egui::Event::Copy => egui::Key::C,
                        _ => egui::Key::V,
                    },
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers,
                },
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            ],
            ..Default::default()
        };
        editor.raw_input_hook(&ctx, &mut input);
        let mut output = ctx.run_ui(input, |ui| editor.draw(ui));
        output.textures_delta.clear();
    };
    let shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    frame(&mut editor, egui::Event::Copy, shift);
    assert!(
        matches!(&editor.modal, Some(Modal::CopySettings(dialog))
            if dialog.purpose == crate::app::develop::settings_transfer::Transfer::Copy),
        "Shift+Cmd+C opens Copy Settings"
    );
    // The C, with its modifiers gone by the frame's end, is not Crop's.
    assert!(editor.view.tool == crate::app::develop::state::Tool::None);
    editor.close_copy_dialog(crate::app::develop::settings_transfer::CopyChoice::Confirm);
    let copied = editor.document.edit.recipe().exposure;
    editor.document.edit.setup_mut().exposure = copied + 1.5;
    frame(&mut editor, egui::Event::Paste("text".into()), shift);
    assert_eq!(
        editor.document.edit.recipe().exposure,
        copied,
        "Shift+Cmd+V pastes"
    );
    // Not V: no Black & White, even with nothing copied to paste.
    editor.clipboard = None;
    frame(&mut editor, egui::Event::Paste("text".into()), shift);
    assert!(!editor.document.edit.recipe().effects.monochrome);
    // Plain Cmd+C stays the clipboard's: no Copy Settings.
    frame(&mut editor, egui::Event::Copy, egui::Modifiers::COMMAND);
    assert!(editor.modal.is_none());
}
