#[cfg(windows)]
#[test]
fn windows_icon_file_contains_the_256_png() {
    let png = super::largest_png_in_ico(include_bytes!("../../packaging/windows/rawmakase.ico"))
        .expect("icon");
    let image = image::load_from_memory(png).expect("png").into_rgba8();
    assert_eq!([image.width(), image.height()], [256, 256]);
}

use super::widgets::tone_curve_ui;
use super::*;
use crate::camera_data::{CameraImage, Metadata};
use crate::{catalog::PhotoId, develop};
use eframe::egui::{Pos2, Rect};
use std::sync::Arc;
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
    let id = l.session.photos[0].id;
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.document.catalog_photo = Some(id);
    editor.document.path = Some(photo.clone());
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
            .catalog
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
#[test]
fn catalog_edits_save_to_database_and_library_renders() -> anyhow::Result<()> {
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
    let id = l.session.photos[0].id;
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.document.catalog_photo = Some(id);
    editor.document.path = Some(photo.clone());
    editor.document.edit.setup_mut().exposure = 1.2;
    editor.document.edit.save_state_mut().mark_changed();
    assert!(editor.flush());
    assert!(!crate::catalog::legacy_sidecar::sidecar_path(&photo).exists());
    assert_eq!(
        editor
            .library
            .as_ref()
            .unwrap()
            .session
            .catalog
            .load_edit(id, &photo)?
            .unwrap()
            .recipe
            .exposure,
        1.2
    );
    editor.module = Module::Library;
    for _ in 0..2 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
    }
    assert!(editor.module == Module::Library);
    Ok(())
}
#[test]
fn curve_pointer_add_drag_and_remove() {
    let ctx = egui::Context::default();
    let mut curve = crate::color::curve::ToneCurve::default();
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(320.))),
                events,
                ..Default::default()
            },
            |ui| tone_curve_ui(ui, &mut curve, &[[0; 256]; 3], 0),
        );
        output.textures_delta.clear();
        curve.points.clone()
    };
    let event = |p: Pos2, button, pressed| egui::Event::PointerButton {
        pos: p,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![]);
    let p = Pos2::new(150., 160.);
    frame(vec![
        egui::Event::PointerMoved(p),
        event(p, egui::PointerButton::Primary, true),
    ]);
    assert_eq!(
        frame(vec![event(p, egui::PointerButton::Primary, false)]).len(),
        3
    );
    frame(vec![event(p, egui::PointerButton::Primary, true)]);
    let q = Pos2::new(200., 110.);
    let moved = frame(vec![egui::Event::PointerMoved(q)]);
    assert!(moved[1][0] > 0.6 && moved[1][1] > 0.6);
    frame(vec![event(q, egui::PointerButton::Primary, false)]);
    frame(vec![event(q, egui::PointerButton::Secondary, true)]);
    assert_eq!(
        frame(vec![event(q, egui::PointerButton::Secondary, false)]).len(),
        2
    );
}
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
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let mut library = crate::app::library::Library::load(&path, ctx.clone())?;
    let ids: Vec<_> = library.session.photos.iter().map(|p| p.id).collect();
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
fn keyboard_fit_and_physical_pixel_region() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    // Z turns the zoom on; F belongs to the chrome now (see `chrome`), so it
    // leaves the photo's zoom as it was.
    for (key, expected) in [(egui::Key::Z, true), (egui::Key::F, true)] {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events: vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| e.draw(ui));
        output.textures_delta.clear();
        assert_eq!(e.view.zoom.on, expected);
    }
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 12,
        height: 8,
        pixels: vec![[0.1; 3]; 96],
        metadata: Metadata {
            width: 12,
            height: 8,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    e.document.set_image(image);
    e.view.zoom.on = true;
    e.view.viewport = Vec2::new(4., 2.);
    assert_eq!(e.region(), Some([4, 3, 4, 2]));
}
/// Cmd+0 is 100%, Cmd+1 fits, and Cmd+= / Cmd+- zoom by a constant factor from
/// the fitted size, stopping at 100% on the way.
#[test]
fn zoom_keys_go_to_100_percent_fit_and_step_by_a_factor() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.module = Module::Develop;
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 1000,
        height: 1000,
        pixels: vec![[0.1; 3]; 1_000_000],
        metadata: Metadata {
            width: 1000,
            height: 1000,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    // Fits at 0.2: 200 pixels of view for 1000 of photo.
    editor.view.viewport = Vec2::splat(200.);
    let mut press = |key, physical, modifiers| {
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: Some(physical),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |_| editor.zoom_keys(&ctx),
        )
        .textures_delta
        .clear();
        (editor.view.zoom.on, editor.view.zoom.level)
    };
    let cmd = egui::Modifiers::COMMAND;
    assert_eq!(press(egui::Key::Num0, egui::Key::Num0, cmd), (true, 1.));
    assert!(!press(egui::Key::Num1, egui::Key::Num1, cmd).0);
    let (on, level) = press(egui::Key::Equals, egui::Key::Equals, cmd);
    assert!(on && (level - 0.2 * std::f32::consts::SQRT_2).abs() < 1e-4);
    // 0.28, 0.4, 0.57, 0.8, then 100% rather than 113%.
    for _ in 0..3 {
        press(egui::Key::Equals, egui::Key::Equals, cmd);
    }
    assert_eq!(press(egui::Key::Equals, egui::Key::Equals, cmd), (true, 1.));
    // Cmd+Option+0 types another character on macOS; the physical key counts.
    press(egui::Key::Num1, egui::Key::Num1, cmd);
    let alt = egui::Modifiers {
        alt: true,
        ..egui::Modifiers::COMMAND
    };
    assert_eq!(press(egui::Key::Equals, egui::Key::Num0, alt), (true, 1.));
    // Fit again from just above it.
    press(egui::Key::Num1, egui::Key::Num1, cmd);
    press(egui::Key::Equals, egui::Key::Equals, cmd);
    assert!(!press(egui::Key::Minus, egui::Key::Minus, cmd).0);
}
#[test]
fn photo_click_zooms_and_drag_pans_without_editing() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.1; 3]; 160000],
        metadata: Metadata {
            width: 400,
            height: 400,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    let recipe = editor.document.edit.recipe().clone();
    let mut frame = |events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                editor.viewport_ui(ui);
            },
        );
        output.textures_delta.clear();
        (
            editor.view.zoom.on,
            editor.view.zoom.pan,
            editor.document.edit.recipe().clone(),
        )
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let p = Pos2::new(100., 100.);
    frame(vec![]);
    frame(vec![egui::Event::PointerMoved(p), button(p, true)]);
    let (zoom, original_pan, _) = frame(vec![button(p, false)]);
    assert!(zoom);
    frame(vec![button(p, true)]);
    let q = Pos2::new(140., 130.);
    let (zoom, pan, after) = frame(vec![egui::Event::PointerMoved(q)]);
    assert!(zoom);
    assert!(pan[0] < original_pan[0] && pan[1] < original_pan[1]);
    assert_eq!(recipe, after);
    let (zoom, _, _) = frame(vec![button(q, false)]);
    assert!(zoom, "Releasing a pan must not toggle zoom");
    frame(vec![button(q, true)]);
    assert!(!frame(vec![button(q, false)]).0);
}

#[test]
fn compact_inspector_keeps_canvas_and_before_preserves_edits() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.document.edit.setup_mut().exposure = 1.25;
    editor.document.edit.setup_mut().crop = [0.1, 0.1, 0.9, 0.9];
    let saved = editor.document.edit.recipe().clone();
    for frame in 0..30 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                events: if frame == 28 {
                    vec![egui::Event::Key {
                        key: egui::Key::Backslash,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }]
                } else {
                    vec![]
                },
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
        assert!(
            editor.view.viewport.x > 400.,
            "Inspector consumed canvas on frame {frame}"
        );
    }
    assert_eq!(editor.view.compare, before_after::Compare::BeforeOnly);
    assert_eq!(*editor.document.edit.recipe(), saved);
    assert_eq!(editor.effective_recipe().crop, saved.crop);
    assert_eq!(editor.effective_recipe().exposure, 0.);
}

#[test]
fn history_snapshot_undo_and_redo() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
fn stale_preview_results_are_discarded() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let (old, _) = e.preview.task.start();
    e.preview.task.start();
    e.tx.send(Event::Rendered {
        id: old,
        pane: worker::Pane::After,
        preview: worker::Preview::Pixels {
            image: crate::rendered::Rendered {
                width: 1,
                height: 1,
                pixels: vec![[1.; 3]],
            },
            display_rgb: vec![255; 3],
            navigator: None,
        },
        histogram: Box::new(crate::rendered::Histogram::EMPTY),
        thumbnail: None,
        samples: None,
        stage: worker::RenderStage::Fit,
        status: "stale".into(),
    })
    .unwrap();
    e.events(&ctx);
    assert!(e.preview.texture.is_none());
}

/// A slider moving starts a render on every change. On a slow GPU each render
/// takes longer than the next change, so cancelling the running one showed
/// nothing until the drag stopped; it now finishes and shows on the way.
#[test]
fn a_render_overtaken_by_an_edit_still_shows_with_its_own_settings() {
    let ctx = egui::Context::default();
    let (mut e, _) = editor_with_blue_photo(&ctx, crate::app::session::Session::default(), true);
    // Only the results sent here arrive: the renderer takes a job and holds it.
    task::wait_for(vec![e.renderer.stop()], std::time::Duration::from_secs(5));
    let (taken_tx, taken) = std::sync::mpsc::channel();
    let (_release, release) = std::sync::mpsc::channel();
    e.renderer = worker::holding(taken_tx, release);
    e.view.viewport = Vec2::new(40., 40.);
    e.schedule();
    let first = e.preview.task.id();
    let first_recipe = e.effective_recipe();
    assert_eq!(
        taken.recv_timeout(std::time::Duration::from_secs(5)),
        Ok(first)
    );
    // Many changes while the first still renders, as on a slow GPU:
    // those replaced before the renderer took them are forgotten, not the first.
    for i in 0..30 {
        in_edit_frame(&ctx, &mut e, |e| {
            e.document.edit.recipe_mut().exposure = 0.5 + i as f32 / 100.
        });
    }
    in_edit_frame(&ctx, &mut e, |e| e.document.edit.recipe_mut().exposure = 1.);
    let second = e.preview.task.id();
    assert!(second > first);
    assert_eq!(e.preview.overtaken.len(), 1);
    let rendered = |id, status: &str| Event::Rendered {
        id,
        pane: worker::Pane::After,
        preview: worker::Preview::Pixels {
            image: crate::rendered::Rendered {
                width: 1,
                height: 1,
                pixels: vec![[0.5; 3]],
            },
            display_rgb: vec![128; 3],
            navigator: None,
        },
        histogram: Box::new(crate::rendered::Histogram::EMPTY),
        thumbnail: None,
        samples: None,
        stage: worker::RenderStage::Fit,
        status: status.into(),
    };
    e.tx.send(rendered(first, "first")).unwrap();
    e.events(&ctx);
    assert_eq!(e.preview.status, "first");
    // Pickers sample it as what it is: the edit before the change.
    assert_eq!(e.preview.samples_recipe.as_ref(), Some(&first_recipe));
    assert!(e.preview.task.is_running());
    e.tx.send(rendered(second, "second")).unwrap();
    e.events(&ctx);
    assert_eq!(e.preview.status, "second");
    assert_eq!(
        e.preview.samples_recipe.as_ref().map(|r| r.exposure),
        Some(1.)
    );
    assert!(!e.preview.task.is_running());
    // An earlier frame arriving late never replaces a later one.
    e.tx.send(rendered(first, "late")).unwrap();
    e.events(&ctx);
    assert_eq!(e.preview.status, "second");
    // A render for anything but an edit, a new view say, cancels what runs.
    e.schedule();
    e.tx.send(rendered(second, "cancelled")).unwrap();
    e.events(&ctx);
    assert_eq!(e.preview.status, "second");
    // So does an edit of another document of the same decoded photo.
    let running = e.preview.task.id();
    e.document.catalog_photo = Some(PhotoId(7));
    in_edit_frame(&ctx, &mut e, |e| e.document.edit.recipe_mut().exposure = 2.);
    e.tx.send(rendered(running, "another document")).unwrap();
    e.events(&ctx);
    assert_eq!(e.preview.status, "second");
}

/// A frame an edit overtook shows on the way, but never becomes the photo's Library
/// thumbnail: that is cached under the current edit, which it was not rendered with.
#[test]
fn only_the_latest_render_becomes_the_library_thumbnail() -> anyhow::Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.RAF"), b"fixture")?;
    let path = d.path().join("test.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&path)?;
    catalog.add_folder(&photos)?;
    drop(catalog);
    let ctx = egui::Context::default();
    let (mut e, _) = editor_with_blue_photo(&ctx, crate::app::session::Session::default(), true);
    task::wait_for(vec![e.renderer.stop()], std::time::Duration::from_secs(5));
    let (taken_tx, taken) = std::sync::mpsc::channel();
    let (_release, release) = std::sync::mpsc::channel();
    e.renderer = worker::holding(taken_tx, release);
    let library = library::Library::load(&path, ctx.clone())?;
    let photo = library.session.photos[0].id;
    e.library = Some(Box::new(library));
    e.document.catalog_photo = Some(photo);
    e.document.path = Some(photos.join("a.RAF"));
    e.view.viewport = Vec2::new(40., 40.);
    e.schedule();
    let first = e.preview.task.id();
    assert_eq!(
        taken.recv_timeout(std::time::Duration::from_secs(5)),
        Ok(first)
    );
    in_edit_frame(&ctx, &mut e, |e| e.document.edit.recipe_mut().exposure = 1.);
    let second = e.preview.task.id();
    let rendered = |id, status: &str| Event::Rendered {
        id,
        pane: worker::Pane::After,
        preview: worker::Preview::Pixels {
            image: crate::rendered::Rendered {
                width: 1,
                height: 1,
                pixels: vec![[0.5; 3]],
            },
            display_rgb: vec![128; 3],
            navigator: None,
        },
        histogram: Box::new(crate::rendered::Histogram::EMPTY),
        thumbnail: Some(image::RgbImage::new(4, 4)),
        samples: None,
        stage: worker::RenderStage::Fit,
        status: status.into(),
    };
    let cached = |e: &Editor| e.library.as_ref().unwrap().has_edited_thumbnail(photo);
    e.tx.send(rendered(first, "first")).unwrap();
    e.events(&ctx);
    assert_eq!(e.preview.status, "first");
    assert!(!cached(&e));
    e.tx.send(rendered(second, "second")).unwrap();
    e.events(&ctx);
    assert_eq!(e.preview.status, "second");
    assert!(cached(&e));
    Ok(())
}

#[test]
fn before_and_after_renders_go_to_their_own_side() {
    use worker::Pane;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    // Each side counts its renders from the same start: ids alone do not tell them apart.
    let (after, _) = e.preview.task.start();
    let (before, _) = e.preview.before.task.start();
    assert_eq!(after, before);
    let rendered = |pane, value: u8| Event::Rendered {
        id: after,
        pane,
        preview: worker::Preview::Pixels {
            image: crate::rendered::Rendered {
                width: 1,
                height: 1,
                pixels: vec![[0.5; 3]],
            },
            display_rgb: vec![value; 3],
            navigator: None,
        },
        histogram: Box::new(crate::rendered::Histogram::EMPTY),
        thumbnail: None,
        samples: None,
        stage: worker::RenderStage::Fit,
        status: "rendered".into(),
    };
    e.tx.send(rendered(Pane::Before, 10)).unwrap();
    e.events(&ctx);
    assert!(e.preview.texture.is_none());
    assert!(e.preview.before.texture.is_some());
    assert!(!e.preview.before.task.is_running());
    assert!(e.preview.task.is_running());
    e.tx.send(rendered(Pane::After, 200)).unwrap();
    e.events(&ctx);
    assert!(e.preview.texture.is_some());
    assert!(!e.preview.task.is_running());
}

#[test]
fn a_failed_before_render_renders_the_edit_again_but_not_before() {
    use worker::{Pane, TaskKind};
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 2,
        height: 2,
        pixels: vec![[0.2; 3]; 4],
        metadata: Metadata {
            width: 2,
            height: 2,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    e.view.viewport = Vec2::new(40., 40.);
    e.set_compare(before_after::Compare::SideBySide(
        before_after::Axis::LeftRight,
    ));
    e.schedule();
    let after = e.preview.task.id();
    e.preview.task.finish(after);
    let before = e.preview.before.task.id();
    // The renderer reset after a panic: the edit's textures are gone as well.
    e.preview.texture = None;
    e.tx.send(Event::Failed {
        id: before,
        task: TaskKind::Render(Pane::Before),
        error: "Rendering failed".into(),
    })
    .unwrap();
    e.events(&ctx);
    assert!(e.preview.task.id() > after);
    assert!(e.preview.task.is_running());
    // Before's failed job is not asked for again.
    assert_eq!(e.preview.before.task.id(), before);
}

#[test]
fn a_failed_magnifier_render_renders_the_edit_again_but_not_the_magnifier() {
    use worker::{Pane, TaskKind};
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 2,
        height: 2,
        pixels: vec![[0.2; 3]; 4],
        metadata: Metadata {
            width: 2,
            height: 2,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    e.view.viewport = Vec2::new(40., 40.);
    e.schedule();
    let after = e.preview.task.id();
    e.preview.task.finish(after);
    let (magnifier, _) = e.preview.magnifier.task.start();
    // The renderer reset after a panic: the edit's textures are gone as well.
    e.preview.texture = None;
    e.tx.send(Event::Failed {
        id: magnifier,
        task: TaskKind::Render(Pane::Magnifier),
        error: "Rendering failed".into(),
    })
    .unwrap();
    e.events(&ctx);
    assert!(e.preview.task.id() > after);
    assert!(e.preview.task.is_running());
    // The magnifier's failed job is not asked for again.
    assert_eq!(e.preview.magnifier.task.id(), magnifier);
}

#[test]
fn a_failed_edit_render_renders_before_again_but_not_the_edit() {
    use worker::{Pane, TaskKind};
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 2,
        height: 2,
        pixels: vec![[0.2; 3]; 4],
        metadata: Metadata {
            width: 2,
            height: 2,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    e.view.viewport = Vec2::new(40., 40.);
    e.set_compare(before_after::Compare::Split(before_after::Axis::LeftRight));
    e.schedule();
    let before = e.preview.before.task.id();
    e.preview.before.task.finish(before);
    let after = e.preview.task.id();
    // The renderer reset after a panic: Before's textures are gone as well.
    e.preview.before.texture = None;
    e.tx.send(Event::Failed {
        id: after,
        task: TaskKind::Render(Pane::After),
        error: "Rendering failed".into(),
    })
    .unwrap();
    e.events(&ctx);
    assert!(e.preview.before.task.id() > before);
    assert_eq!(e.preview.task.id(), after);
}

#[test]
fn worker_failures_are_scoped_and_render_stages_do_not_depend_on_status_text() {
    use worker::{RenderStage, TaskKind};
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let (load_id, _) = editor.load.start();
    let (render_id, _) = editor.preview.task.start();
    editor
        .tx
        .send(Event::Failed {
            id: render_id,
            task: TaskKind::Render(worker::Pane::After),
            error: "render failed".into(),
        })
        .unwrap();
    editor.events(&ctx);
    assert!(editor.load.is_running());
    assert!(!editor.preview.task.is_running());

    let (current, _) = editor.preview.task.start();
    editor
        .tx
        .send(Event::Failed {
            id: render_id,
            task: TaskKind::Render(worker::Pane::After),
            error: "stale failure".into(),
        })
        .unwrap();
    editor
        .tx
        .send(Event::Failed {
            id: load_id,
            task: TaskKind::Load,
            error: "load failed".into(),
        })
        .unwrap();
    editor.events(&ctx);
    assert!(!editor.load.is_running());
    assert!(editor.preview.task.is_running());
    for (stage, status, running) in [
        (RenderStage::Draft, "localized preview text", true),
        (RenderStage::Fit, "Draft is just text here", false),
    ] {
        editor
            .tx
            .send(Event::Rendered {
                id: current,
                pane: worker::Pane::After,
                preview: worker::Preview::Pixels {
                    image: crate::rendered::Rendered {
                        width: 1,
                        height: 1,
                        pixels: vec![[0.5; 3]],
                    },
                    display_rgb: vec![128; 3],
                    navigator: None,
                },
                histogram: Box::new(crate::rendered::Histogram::EMPTY),
                thumbnail: None,
                samples: None,
                stage,
                status: status.into(),
            })
            .unwrap();
        editor.events(&ctx);
        assert_eq!(editor.preview.task.is_running(), running);
    }
}

#[test]
fn catalog_header_keeps_the_recipe_resolved_by_the_loader() -> anyhow::Result<()> {
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
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    editor.document.catalog_photo = Some(id);
    let (generation, _) = editor.load.start();
    let recipe = Recipe {
        exposure: 0.75,
        ..Default::default()
    };
    editor
        .tx
        .send(Event::Header(Box::new(LoadedHeader {
            id: generation,
            path: raw,
            metadata: Metadata::default(),
            recipe: recipe.clone(),
            export: Default::default(),
            status: "Original".into(),
        })))
        .unwrap();
    editor.events(&ctx);
    assert_eq!(*editor.document.edit.recipe(), recipe);
    Ok(())
}

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
        let (generation, _) = editor.load.start();
        editor
            .tx
            .send(Event::Header(Box::new(LoadedHeader {
                id: generation,
                path: raw.clone(),
                metadata: Metadata::default(),
                recipe: Recipe::default(),
                export: Default::default(),
                status: "Original".into(),
            })))
            .unwrap();
        editor.events(&ctx);
    };
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    open(&mut editor);
    for exposure in [0.5, 1.] {
        let before = editor.document.edit.recipe().clone();
        editor.document.edit.setup_mut().exposure = exposure;
        editor.commit_edit(before, None);
    }
    assert!(editor.flush());
    // Another photo, or a restart, starts a new document.
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
            &saved,
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )?;
        drop(catalog);
        let ctx = egui::Context::default();
        let mut editor =
            Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
        editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
        let open = |editor: &mut Editor| {
            editor.document.reset(Some(id));
            let (generation, _) = editor.load.start();
            editor
                .tx
                .send(Event::Header(Box::new(LoadedHeader {
                    id: generation,
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
#[test]
fn navigation_during_an_edit_frame_cannot_dirty_the_next_document() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.document.edit.setup_mut().exposure = 1.25;
    editor.presets.preview = Some(editor.document.edit.recipe().clone());
    editor.view.crop_drag = Some(([0., 0., 1., 1.], 0));
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
fn refreshing_preset_support_cancels_the_hover_render() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 1,
        height: 1,
        pixels: vec![[0.1; 3]],
        metadata: Metadata {
            width: 1,
            height: 1,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.presets.preview = Some(Recipe {
        exposure: 1.,
        ..Default::default()
    });
    let (previous, cancelled) = editor.preview.task.start();
    editor.refresh_preset_support();
    assert!(editor.presets.preview.is_none());
    assert!(cancelled.load(std::sync::atomic::Ordering::Relaxed));
    assert!(editor.preview.task.id() > previous);
}

#[test]
fn session_preferences_use_the_injected_store() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("session.json");
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::session::Session::default(),
        Some(path.clone()),
    );
    editor.document.path = Some(dir.path().join("photo.ARW"));
    editor
        .tx
        .send(Event::Monitor(dir.path().join("display.icc")))
        .unwrap();
    editor.events(&ctx);
    let saved: crate::app::session::Session = serde_json::from_slice(&std::fs::read(path)?)?;
    assert_eq!(saved.last_path, editor.document.path);
    assert_eq!(saved.monitor, Some(dir.path().join("display.icc")));
    assert_eq!(
        editor.view.monitor,
        crate::app::monitor::MonitorProfile::Custom(dir.path().join("display.icc"))
    );
    Ok(())
}
#[test]
fn remove_tool_adds_spots_paints_brushes_and_edits_the_selection() {
    use crate::model::retouch::RetouchShape;
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: (0..160000)
            .map(|i| {
                let (x, y) = ((i % 400) as f32, (i / 400) as f32);
                [0.2 + 0.05 * (x * 0.1).sin() * (y * 0.13).cos(); 3]
            })
            .collect(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor.view.tool = state::Tool::Remove;
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                ctx.input(|i| {
                    if editor.view.is(state::Tool::Remove) {
                        editor.retouch_keys(i)
                    }
                });
                editor.viewport_ui(ui);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
        (
            editor.document.edit.recipe().retouch.clone(),
            editor.view.zoom.on,
            editor.view.retouch.selected,
        )
    };
    let button = |pos, pressed, modifiers| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    let none = egui::Modifiers::NONE;
    frame(vec![]);
    // Click: a spot with an automatic source, and no zoom.
    let p = Pos2::new(60., 60.);
    frame(vec![egui::Event::PointerMoved(p), button(p, true, none)]);
    let (ops, zoomed, _) = frame(vec![button(p, false, none)]);
    assert_eq!(ops.len(), 1);
    assert!(!zoomed);
    assert!(ops[0].offset != [0., 0.] && ops[0].offset.iter().all(|v| v.is_finite()));
    assert!(matches!(ops[0].shape, RetouchShape::Spot { center, .. }
        if (center[0] - 0.3).abs() < 0.02 && (center[1] - 0.3).abs() < 0.02));
    // A drag from empty space paints a brushed area.
    let (a, b) = (Pos2::new(150., 40.), Pos2::new(150., 150.));
    frame(vec![egui::Event::PointerMoved(a), button(a, true, none)]);
    for k in 1..=10 {
        frame(vec![egui::Event::PointerMoved(
            a + (b - a) * k as f32 / 10.,
        )]);
    }
    let (ops, _, selected) = frame(vec![button(b, false, none)]);
    assert_eq!(ops.len(), 2);
    assert!(matches!(&ops[1].shape, RetouchShape::Brush { points, .. } if points.len() > 3));
    assert_eq!(selected, Some(1));
    // ] grows the selected area; Delete removes it.
    let radius = ops[1].radius();
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let (ops, ..) = frame(vec![key(egui::Key::CloseBracket)]);
    assert!(ops[1].radius() > radius);
    let (ops, ..) = frame(vec![key(egui::Key::Delete)]);
    assert_eq!(ops.len(), 1);
    // Dragging the remaining spot moves it and keeps its source in place.
    let before = ops[0].clone();
    let q = Pos2::new(80., 70.);
    frame(vec![egui::Event::PointerMoved(p), button(p, true, none)]);
    frame(vec![egui::Event::PointerMoved(q)]);
    let (ops, ..) = frame(vec![button(q, false, none)]);
    let source = |op: &crate::model::retouch::RetouchOp| {
        [op.pin()[0] + op.offset[0], op.pin()[1] + op.offset[1]]
    };
    assert!((ops[0].pin()[0] - before.pin()[0] - 0.1).abs() < 0.01);
    assert!((source(&ops[0])[0] - source(&before)[0]).abs() < 1e-5);
}
#[test]
fn red_eye_tool_adds_moves_and_deletes_one_history_step_each() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let eyes = [([100., 100.], 10.), ([300., 260.], 8.)];
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: (0..160000)
            .map(|i| {
                let (x, y) = ((i % 400) as f32, (i / 400) as f32);
                let red = eyes.iter().any(|(c, r)| (x - c[0]).hypot(y - c[1]) <= *r);
                if red {
                    [0.6, 0.03, 0.03]
                } else {
                    [0.55, 0.35, 0.25]
                }
            })
            .collect(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor.view.tool = state::Tool::RedEye;
    editor.view.red_eye.size = 0.06;
    let mut frame = |events: Vec<egui::Event>| {
        let edit = editor.begin_edit_frame();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                ctx.input(|i| editor.red_eye_keys(i));
                editor.viewport_ui(ui);
            },
        );
        editor.finish_edit_frame(edit, &ctx);
        output.textures_delta.clear();
        let (steps, applied) = editor.document.edit.history().steps();
        let names: Vec<String> = steps[..applied].iter().map(|s| s.name.clone()).collect();
        (
            editor.document.edit.recipe().red_eye.clone(),
            names,
            editor.messages.line().to_string(),
        )
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![]);
    // With the circle sized (by the wheel or [ ]), pressing on the first eye finds its
    // pupil: one step. Moving while pressed doesn't resize the circle, as in Lightroom.
    let (a, b) = (Pos2::new(50., 50.), Pos2::new(62., 50.));
    frame(vec![egui::Event::PointerMoved(a), button(a, true)]);
    for k in 1..=6 {
        frame(vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 6.)]);
    }
    let (ops, names, _) = frame(vec![button(b, false)]);
    assert_eq!(ops.len(), 1);
    assert!((ops[0].center[0] - 0.25).abs() < 0.005 && (ops[0].center[1] - 0.25).abs() < 0.005);
    assert!(
        (ops[0].radius[0] * 400. - 10.).abs() < 1.5,
        "{:?}",
        ops[0].radius
    );
    assert_eq!(names, ["Add Red Eye Correction"]);
    // A click on the second eye uses the same size.
    let c = Pos2::new(150., 130.);
    frame(vec![egui::Event::PointerMoved(c), button(c, true)]);
    let (ops, names, _) = frame(vec![button(c, false)]);
    assert_eq!(ops.len(), 2);
    assert!((ops[1].center[0] - 0.75).abs() < 0.005 && (ops[1].center[1] - 0.65).abs() < 0.005);
    assert_eq!(names.len(), 2);
    // A click on nothing red adds nothing and says why.
    let d = Pos2::new(180., 20.);
    frame(vec![egui::Event::PointerMoved(d), button(d, true)]);
    let (ops, names, status) = frame(vec![button(d, false)]);
    assert_eq!((ops.len(), names.len()), (2, 2));
    assert!(status.contains("Unable to find red eye"), "{status}");
    // Dragging the first correction moves it in one step.
    let e = Pos2::new(70., 60.);
    frame(vec![egui::Event::PointerMoved(a), button(a, true)]);
    for k in 1..=5 {
        frame(vec![egui::Event::PointerMoved(a + (e - a) * k as f32 / 5.)]);
    }
    let (ops, names, _) = frame(vec![button(e, false)]);
    assert!((ops[0].center[0] - 0.35).abs() < 0.005);
    assert_eq!(names.last().unwrap(), "Update Red Eye Correction");
    assert_eq!(names.len(), 3);
    // Delete removes the selected correction.
    let key = egui::Event::Key {
        key: egui::Key::Delete,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let (ops, names, _) = frame(vec![key]);
    assert_eq!(ops.len(), 1);
    assert_eq!(names.last().unwrap(), "Delete Red Eye Correction");
    // Undo brings it back where it was.
    let mut recipe = editor.document.edit.recipe().clone();
    assert!(editor.document.edit.history_mut().undo(&mut recipe));
    assert_eq!(recipe.red_eye.len(), 2);
    assert!((recipe.red_eye[0].center[0] - 0.35).abs() < 0.005);
}
#[test]
fn a_new_red_eye_correction_turns_the_red_eye_switch_on() {
    use crate::model::panels::{Panel, PanelState};
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 200,
        height: 200,
        pixels: (0..40000)
            .map(|i| {
                let (x, y) = ((i % 200) as f32, (i / 200) as f32);
                if (x - 100.).hypot(y - 100.) <= 8. {
                    [0.6, 0.03, 0.03]
                } else {
                    [0.55, 0.35, 0.25]
                }
            })
            .collect(),
        metadata: Metadata {
            width: 200,
            height: 200,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    let panels = &mut editor.document.edit.setup_mut().panels;
    panels.set(Panel::RedEye, PanelState::Off);
    in_edit_frame(&ctx, &mut editor, |e| e.add_red_eye([0.5, 0.5], 0.1));
    assert_eq!(editor.document.edit.recipe().red_eye.len(), 1);
    assert_eq!(
        editor.document.edit.recipe().panels.state(Panel::RedEye),
        PanelState::On
    );
}
#[test]
fn pet_eye_type_finds_a_glowing_pupil_and_adds_a_catchlight() {
    use crate::model::red_eye::{DEFAULT_CATCHLIGHT, EyeKind};
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 200,
        height: 200,
        pixels: (0..40000)
            .map(|i| {
                let d = ((i % 200) as f32 - 100.).hypot((i / 200) as f32 - 100.);
                if d <= 10. {
                    [0.5, 0.8, 0.15]
                } else if d <= 24. {
                    [0.25, 0.18, 0.05]
                } else {
                    [0.3, 0.25, 0.2]
                }
            })
            .collect(),
        metadata: Metadata {
            width: 200,
            height: 200,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    // Red Eye finds nothing red there.
    in_edit_frame(&ctx, &mut editor, |e| e.add_red_eye([0.5, 0.5], 0.15));
    assert!(editor.document.edit.recipe().red_eye.is_empty());
    assert!(editor.messages.line().contains("Unable to find red eye"));
    editor.view.red_eye.pet = red_eye_tool::PupilType::Pet;
    in_edit_frame(&ctx, &mut editor, |e| e.add_red_eye([0.5, 0.5], 0.15));
    let eyes = &editor.document.edit.recipe().red_eye;
    assert_eq!(eyes.len(), 1);
    assert_eq!(
        eyes[0].kind,
        EyeKind::Pet {
            catchlight: Some(DEFAULT_CATCHLIGHT)
        }
    );
    assert!((eyes[0].radius[0] * 200. - 10.).abs() < 1.5);
    // Selecting a correction makes its type the one new corrections get.
    editor.view.red_eye.pet = red_eye_tool::PupilType::Red;
    editor.select_red_eye(Some(0));
    assert_eq!(editor.view.red_eye.pet, red_eye_tool::PupilType::Pet);
    // Even when the remembered type is stale (after an undo, say), a new correction
    // takes the selected one's type, as the Type menu shows.
    editor.view.red_eye.pet = red_eye_tool::PupilType::Red;
    in_edit_frame(&ctx, &mut editor, |e| e.add_red_eye([0.5, 0.5], 0.15));
    assert_eq!(editor.document.edit.recipe().red_eye.len(), 2);
    editor.document.edit.setup_mut().red_eye.pop();
    editor.select_red_eye(Some(0));
    // A click on the catchlight's handle, even at the pupil's edge outside the
    // ellipse, is not a new search.
    editor.document.edit.setup_mut().red_eye[0].kind = EyeKind::Pet {
        catchlight: Some([1., 0.]),
    };
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor.view.tool = state::Tool::RedEye;
    let handle = editor.document.edit.recipe().red_eye[0]
        .catchlight_at(1.)
        .unwrap();
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                editor.viewport_ui(ui);
            },
        );
        output.textures_delta.clear();
        editor.document.edit.recipe().red_eye.len()
    };
    frame(vec![]);
    let p = Pos2::new(handle[0] * 200., handle[1] * 200.);
    let button = |pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![egui::Event::PointerMoved(p), button(true)]);
    assert_eq!(frame(vec![button(false)]), 1);
}
#[test]
fn red_eye_tool_refuses_a_red_area_too_large_to_be_a_pupil() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: (0..160000)
            .map(|i| {
                let (x, y) = ((i % 400) as f32, (i / 400) as f32);
                if (x - 200.).hypot(y - 200.) <= 150. {
                    [0.6, 0.03, 0.03]
                } else {
                    [0.55, 0.35, 0.25]
                }
            })
            .collect(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, |e| e.add_red_eye([0.5, 0.5], 0.48));
    assert!(editor.document.edit.recipe().red_eye.is_empty());
    assert!(
        editor.messages.line().contains("Unable to find red eye"),
        "{}",
        editor.messages.line()
    );
    editor.document.edit.recipe().validate().unwrap();
}
#[test]
fn a_mask_brush_paints_live_and_preserves_the_overlay_choice() {
    use super::worker::Overlay;
    use crate::model::masks::MaskShape;
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.2; 3]; 160000],
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                editor.viewport_ui(ui);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut editor, vec![]);
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(mask_tool::Kind::Brush, None)
    });
    let steps = editor.document.edit.history().steps().0.len();
    assert!(
        editor.view.masking.overlay,
        "new editors show masks by default"
    );
    assert!(matches!(editor.overlay(), Overlay::Mask { index: 0, .. }));
    let points = |editor: &Editor| match &editor.document.edit.recipe().masks[0].components[0].shape
    {
        MaskShape::Brush { strokes } => strokes.iter().map(|s| s.points.len()).collect::<Vec<_>>(),
        shape => panic!("{shape:?}"),
    };
    let (a, b) = (Pos2::new(20., 100.), Pos2::new(180., 100.));
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    let mut painted = 0;
    for k in 1..=8 {
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 8.)],
        );
        // Mid-stroke, the photo already has the stroke so far, with the
        // default overlay still enabled.
        let now = points(&editor);
        assert_eq!(now.len(), 1, "one stroke while painting");
        assert!(now[0] >= painted);
        painted = now[0];
        assert!(matches!(editor.overlay(), Overlay::Mask { index: 0, .. }));
    }
    assert!(painted > 3, "{painted}");
    // Still one step to come, once the stroke ends.
    assert_eq!(editor.document.edit.history().steps().0.len(), steps);
    frame(&mut editor, vec![button(b, false)]);
    assert_eq!(points(&editor), vec![painted]);
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    // The overlay stays once the stroke ends, until O hides it.
    assert!(matches!(editor.overlay(), Overlay::Mask { index: 0, .. }));
    let o = egui::Event::Key {
        key: egui::Key::O,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    in_edit_frame(&ctx, &mut editor, |editor| {
        ctx.input_mut(|i| i.events.push(o));
        ctx.input(|i| editor.mask_keys(i));
    });
    assert_eq!(editor.overlay(), Overlay::None);
    // Further strokes start away from the mask's pin, which a drag would move.
    let (a, b) = (Pos2::new(20., 140.), Pos2::new(180., 140.));
    // An Undo while painting takes back the stroke so far and ends the painting.
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a + (b - a) * 0.5)],
    );
    assert_eq!(points(&editor).len(), 2);
    assert!(!editor.view.masking.overlay, "painting must preserve O off");
    assert_eq!(editor.overlay(), Overlay::None);
    in_edit_frame(&ctx, &mut editor, |editor| editor.undo());
    frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
    frame(&mut editor, vec![button(b, false)]);
    assert_eq!(points(&editor), vec![painted]);
    // So does an Undo with the pointer held still: a Redo then brings the stroke back
    // as far as it was painted, and the drag that goes on paints no more of it.
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a + (b - a) * 0.5)],
    );
    let partial = points(&editor);
    assert_eq!(partial.len(), 2);
    in_edit_frame(&ctx, &mut editor, |editor| editor.undo());
    frame(&mut editor, vec![]);
    in_edit_frame(&ctx, &mut editor, |editor| editor.redo());
    assert_eq!(points(&editor), partial);
    frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
    frame(&mut editor, vec![button(b, false)]);
    assert_eq!(points(&editor), partial);
    // Or Undo and Redo in one frame, before the photo is drawn again.
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a + (b - a) * 0.5)],
    );
    let partial = points(&editor);
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.undo();
        editor.redo();
    });
    assert_eq!(points(&editor), partial);
    frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
    frame(&mut editor, vec![button(b, false)]);
    assert_eq!(points(&editor), partial);
    // Space taking over mid-stroke ends it: once Space is let go, the drag that goes
    // on paints nothing more into it.
    let space = |pressed| egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a + (b - a) * 0.25)],
    );
    let started = points(&editor);
    assert_eq!(started.len(), partial.len() + 1);
    frame(&mut editor, vec![space(true)]);
    frame(&mut editor, vec![space(false)]);
    frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
    frame(&mut editor, vec![button(b, false)]);
    assert_eq!(points(&editor), started);
    // Repeated painting and erasing keep the explicit overlay choice (#697).
    for (brush, overlay) in [(0, false), (2, false), (2, true)] {
        editor.view.masking.brush = brush;
        editor.view.masking.overlay = overlay;
        let count = points(&editor).len();
        let (a, b) = (Pos2::new(40., 60.), Pos2::new(160., 60.));
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(a), button(a, true)],
        );
        frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
        assert_eq!(points(&editor).len(), count + 1);
        assert_eq!(editor.view.masking.overlay, overlay);
        assert_eq!(matches!(editor.overlay(), Overlay::Mask { .. }), overlay);
        frame(&mut editor, vec![button(b, false)]);
        assert_eq!(editor.view.masking.overlay, overlay);
        let MaskShape::Brush { strokes } =
            &editor.document.edit.recipe().masks[0].components[0].shape
        else {
            unreachable!()
        };
        assert_eq!(strokes.last().unwrap().erase, brush == 2);
    }
    editor.document.edit.recipe().validate().unwrap();
}
#[test]
fn dragging_a_mask_pin_moves_the_mask() {
    use crate::model::masks::{MaskComponent, MaskGroup, MaskShape};
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.2; 3]; 160000],
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                editor.viewport_ui(ui);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut editor, vec![]);
    // A brush mask with a stroke, and a radial gradient added to it.
    let stroke = crate::model::masks::BrushStroke {
        points: vec![[0.3, 0.3], [0.5, 0.3]].into(),
        radius: 0.05,
        feather: 0.5,
        flow: 1.,
        density: 1.,
        erase: false,
        auto_mask: false,
    };
    in_edit_frame(&ctx, &mut editor, |e| {
        e.view.tool = state::Tool::Mask;
        let mut radial = MaskComponent::new(MaskShape::Radial {
            center: [0.7, 0.7],
            radii: [0.1, 0.1],
            angle: 0.,
            feather: 0.5,
        });
        radial.op = crate::model::masks::MaskOp::Add;
        e.document.edit.recipe_mut().masks.push(MaskGroup {
            components: vec![
                MaskComponent::new(MaskShape::Brush {
                    strokes: vec![stroke],
                }),
                radial,
            ],
            ..Default::default()
        });
    });
    frame(&mut editor, vec![]);
    let steps = editor.document.edit.history().steps().0.len();
    // Over the pin, the mask's overlay shows though Show Overlay is off; it goes when
    // the pointer leaves.
    editor.view.masking.overlay = false;
    assert!(!editor.view.masking.overlay);
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(Pos2::new(62., 61.))],
    );
    assert!(matches!(
        editor.overlay(),
        super::worker::Overlay::Mask { index: 0, .. }
    ));
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(Pos2::new(150., 40.))],
    );
    assert_eq!(editor.overlay(), super::worker::Overlay::None);
    // The pin sits on the stroke's first dab; dragging it by a tenth of the photo
    // moves every part of the mask with it, as one step.
    let (a, b) = (Pos2::new(60., 60.), Pos2::new(80., 80.));
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    for k in 1..=4 {
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 4.)],
        );
    }
    frame(&mut editor, vec![button(b, false)]);
    let m = &editor.document.edit.recipe().masks[0];
    let near = |p: [f32; 2], q: [f32; 2]| (p[0] - q[0]).abs() < 0.01 && (p[1] - q[1]).abs() < 0.01;
    let MaskShape::Brush { strokes } = &m.components[0].shape else {
        panic!()
    };
    assert!(
        near(strokes[0].points[0], [0.4, 0.4]),
        "{:?}",
        strokes[0].points
    );
    assert!(
        near(strokes[0].points[1], [0.6, 0.4]),
        "{:?}",
        strokes[0].points
    );
    let MaskShape::Radial { center, .. } = m.components[1].shape else {
        panic!()
    };
    assert!(near(center, [0.8, 0.8]), "{center:?}");
    assert_eq!(strokes.len(), 1, "the drag painted nothing");
    assert_eq!(editor.view.masking.selected, Some(0));
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    assert!(!editor.view.zoom.on);
    // With its brush selected, as after painting, the pin still moves the mask.
    assert!(editor.mask_brush_shown());
    let c = Pos2::new(80., 100.);
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(b), button(b, true)],
    );
    for k in 1..=4 {
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(b + (c - b) * k as f32 / 4.)],
        );
    }
    frame(&mut editor, vec![button(c, false)]);
    let MaskShape::Brush { strokes } = &editor.document.edit.recipe().masks[0].components[0].shape
    else {
        panic!()
    };
    assert_eq!(strokes.len(), 1, "the drag painted nothing");
    assert!(
        near(strokes[0].points[0], [0.4, 0.5]),
        "{:?}",
        strokes[0].points
    );
    // Undo, Space or deleting the mask while moving it ends the move: the drag that
    // goes on moves nothing.
    let space = |pressed| egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let pin = c;
    let d = Pos2::new(120., 120.);
    let base = editor.document.edit.recipe().masks[0].clone();
    for interruption in 0..5 {
        // The mask with its pin at `pin`, and a second like it, which a Delete moves
        // into its place.
        in_edit_frame(&ctx, &mut editor, |e| {
            let mut other = base.clone();
            other.name = "Other".into();
            e.document.edit.recipe_mut().masks = vec![base.clone(), other];
            e.view.masking.selected = None;
        });
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(pin), button(pin, true)],
        );
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(pin + Vec2::new(4., 4.))],
        );
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(pin + Vec2::new(8., 8.))],
        );
        match interruption {
            0 => in_edit_frame(&ctx, &mut editor, |e| e.undo()),
            1 => {
                frame(&mut editor, vec![space(true)]);
                frame(&mut editor, vec![space(false)]);
            }
            // Delete, as its key does; back where the drag started, so the mask that
            // takes its place has the same shapes as the drag last left.
            2 | 3 => {
                if interruption == 3 {
                    frame(&mut editor, vec![egui::Event::PointerMoved(pin)]);
                }
                in_edit_frame(&ctx, &mut editor, |e| {
                    e.document.edit.recipe_mut().masks.remove(0);
                    e.view.masking.selected = None;
                });
            }
            // K starts another mask.
            _ => in_edit_frame(&ctx, &mut editor, |e| {
                e.create_mask(mask_tool::Kind::Brush, None)
            }),
        }
        let before = editor.document.edit.recipe().masks.clone();
        frame(&mut editor, vec![egui::Event::PointerMoved(d)]);
        frame(&mut editor, vec![button(d, false)]);
        assert_eq!(
            editor.document.edit.recipe().masks,
            before,
            "interruption {interruption}"
        );
    }
}
#[test]
fn masking_tool_draws_gradients_paints_brushes_and_edits_handles() {
    use crate::model::masks::MaskShape;
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.2; 3]; 160000],
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    let mut frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                ctx.input(|i| {
                    if editor.view.is(state::Tool::Mask) {
                        editor.mask_keys(i)
                    }
                });
                editor.viewport_ui(ui);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let drag = |editor: &mut Editor,
                frame: &mut dyn FnMut(&mut Editor, Vec<egui::Event>),
                a: Pos2,
                b: Pos2| {
        frame(editor, vec![egui::Event::PointerMoved(a), button(a, true)]);
        for k in 1..=8 {
            frame(
                editor,
                vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 8.)],
            );
        }
        frame(editor, vec![button(b, false)]);
    };
    frame(&mut editor, vec![]);
    // M, then a drag, draws a linear gradient as a new mask.
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(mask_tool::Kind::Linear, None)
    });
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(100., 40.),
        Pos2::new(100., 160.),
    );
    let masks = editor.document.edit.recipe().masks.clone();
    assert_eq!(masks.len(), 1);
    let MaskShape::Linear { from, to } = masks[0].components[0].shape else {
        panic!("{:?}", masks[0].components[0].shape);
    };
    assert!(
        (from[1] - 0.2).abs() < 0.02 && (to[1] - 0.8).abs() < 0.02,
        "{from:?} {to:?}"
    );
    assert!(!editor.view.zoom.on);
    // Dragging its end handle moves only that end.
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(100., 160.),
        Pos2::new(140., 170.),
    );
    let MaskShape::Linear { from: f2, to: t2 } =
        editor.document.edit.recipe().masks[0].components[0].shape
    else {
        panic!()
    };
    assert_eq!(f2, from);
    assert!(
        (t2[0] - 0.7).abs() < 0.02 && (t2[1] - 0.85).abs() < 0.02,
        "{t2:?}"
    );
    // Shift+M makes a radial gradient; K a brush that paints.
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(mask_tool::Kind::Radial, None)
    });
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(60., 60.),
        Pos2::new(90., 80.),
    );
    assert!(matches!(
        editor.document.edit.recipe().masks[1].components[0].shape,
        MaskShape::Radial { radii, .. } if (radii[0] - 0.15).abs() < 0.02 && (radii[1] - 0.1).abs() < 0.02
    ));
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(mask_tool::Kind::Brush, None)
    });
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(20., 180.),
        Pos2::new(180., 180.),
    );
    let MaskShape::Brush { strokes } = &editor.document.edit.recipe().masks[2].components[0].shape
    else {
        panic!()
    };
    assert_eq!(strokes.len(), 1);
    assert!(strokes[0].points.len() > 3);
    // Add a subtracted brush to the brush mask, then delete the mask with Delete.
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(
            mask_tool::Kind::Brush,
            Some(crate::model::masks::MaskOp::Subtract),
        )
    });
    assert_eq!(editor.document.edit.recipe().masks[2].components.len(), 2);
    frame(
        &mut editor,
        vec![egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(editor.document.edit.recipe().masks.len(), 2);
    editor.document.edit.recipe().validate().unwrap();
    // The whole editor draws the Masking and Remove drawers without disturbing edits.
    editor.view.masking.selected = Some(0);
    editor.view.masking.component = Some(0);
    let saved = editor.document.edit.recipe().clone();
    for tool in [state::Tool::Mask, state::Tool::Remove, state::Tool::Crop] {
        editor.view.tool = tool;
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 900.))),
                    ..Default::default()
                },
                |ui| editor.draw(ui),
            );
            output.textures_delta.clear();
        }
    }
    assert_eq!(*editor.document.edit.recipe(), saved);
}

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
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    // As if dropped on the window.
    editor.open(raw.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.pending_photo.is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        editor.events(&ctx);
    }
    let raw = raw.canonicalize()?;
    let id = editor
        .library
        .as_ref()
        .and_then(|l| l.session.photos.iter().find(|p| p.path == raw))
        .map(|p| p.id);
    assert!(id.is_some(), "the photo's folder was added to the catalog");
    assert_eq!(editor.document.catalog_photo, id);
    assert!(editor.module == Module::Develop);
    Ok(())
}

/// Delivers worker events until no catalog work is under way.
fn settle_catalog(editor: &mut Editor, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.activity.is_busy() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        editor.events(ctx);
    }
    assert!(
        !editor.activity.is_busy(),
        "the catalog work never finished"
    );
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
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    // As if dropped on the window.
    editor.open(trip);
    settle_catalog(&mut editor, &ctx);
    let names: Vec<_> = editor
        .library
        .as_ref()
        .map(|l| {
            l.session
                .photos
                .iter()
                .map(|p| p.filename.clone())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(names, ["in.ARW"]);
    assert!(editor.pending_photo.is_none());
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
    assert_eq!(library.session.photos.len(), 3);
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
        library.session.roots,
        library.session.folders,
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
    let old_root = library.session.roots[0].0;
    library.session.catalog.add_folder(&real)?;
    let root = library
        .session
        .catalog
        .folders()?
        .into_iter()
        .find(|f| f.path == real.canonicalize().unwrap())
        .unwrap()
        .root;
    library.session.catalog.relink_root(root, &alias)?;
    library.refresh()?;
    library.click_folder_for_tests(&format!("root:{old_root}"));
    editor.saved_layout.query = "outside".into();
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
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    let names = |editor: &Editor| -> Vec<String> {
        let mut names: Vec<String> = editor
            .library
            .as_ref()
            .map(|l| {
                l.session
                    .photos
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
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    editor.dropped(vec![good, locked.clone()]);
    settle_catalog(&mut editor, &ctx);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))?;
    let library = editor.library.as_ref().unwrap();
    let names: Vec<_> = library.session.photos.iter().map(|p| &p.filename).collect();
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
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx)?));
    let library = editor.library.as_ref().unwrap();
    let away = library
        .session
        .photos
        .iter()
        .find(|p| p.filename == "away.ARW")
        .unwrap()
        .id;
    let trip: std::collections::HashSet<_> = library
        .session
        .folders
        .iter()
        .filter(|f| f.relative.trim_end_matches('/') == "trip")
        .map(|f| f.id)
        .collect();
    assert_eq!(trip.len(), 1);
    editor.document.catalog_photo = Some(away);
    editor.module = Module::Develop;
    // Still loading, and the locked reference too.
    let (loading, _) = editor.load.start();
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
    assert_eq!(editor.library.as_ref().unwrap().session.photos.len(), 2);
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
        .photos
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
    assert!(!editor.load.is_running());
    assert_ne!(editor.load.id(), loading);
    assert_eq!(editor.reference.photo, None);
    assert!(photos.join("trip/away.ARW").is_file());
    Ok(())
}

#[test]
fn a_first_launch_opens_a_new_catalog_beside_the_session() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let session = || crate::app::session::Session {
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
            .map(|l| l.session.catalog.location()),
        Some(&crate::catalog::CatalogLocation::File(catalog.clone()))
    );
    // The next launch opens it as the last catalog, not as a new one.
    editor.save_session()?;
    drop(editor);
    let saved: crate::app::session::Session =
        serde_json::from_slice(&std::fs::read(dir.path().join("session.json"))?)?;
    assert_eq!(saved.last_path.as_ref(), Some(&catalog));

    // A catalog used before that is missing now (on an unplugged drive) is
    // not replaced by a new one.
    let elsewhere = tempfile::tempdir()?;
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::session::Session {
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
    let saved: crate::app::session::Session =
        serde_json::from_slice(&std::fs::read(elsewhere.path().join("session.json"))?)?;
    assert_eq!(
        saved.last_path,
        Some(elsewhere.path().join("Offline.rawmakase"))
    );
    Ok(())
}

#[test]
fn the_prefetched_neighbour_follows_the_direction_of_travel() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    for name in ["a.ARW", "b.ARW", "c.ARW"] {
        std::fs::write(dir.path().join(name), name)?;
    }
    let path = dir.path().join("photos.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(dir.path())?;
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let library = library::Library::load(&path, ctx)?;
    // The filmstrip order, first to last.
    let mut order = vec![library.session.photos[0].id];
    while let Some(previous) = library.navigate(order[0], -1).filter(|p| *p != order[0]) {
        order.insert(0, previous);
    }
    while let Some(next) = library
        .navigate(order[order.len() - 1], 1)
        .filter(|n| !order.contains(n))
    {
        order.push(next);
    }
    let paths: Vec<_> = order
        .iter()
        .map(|id| library.photo(*id).unwrap().path.clone())
        .collect();
    let [first, middle, last] = [order[0], order[1], order[2]];
    editor.library = Some(Box::new(library));
    // Opening a photo, or stepping forward: the next one.
    assert_eq!(editor.prefetch_neighbour(middle), Some(paths[2].clone()));
    editor.document.catalog_photo = Some(first);
    assert_eq!(editor.prefetch_neighbour(middle), Some(paths[2].clone()));
    // Stepping back: the previous one.
    editor.document.catalog_photo = Some(last);
    assert_eq!(editor.prefetch_neighbour(middle), Some(paths[0].clone()));
    // Nothing past the end of the filmstrip.
    editor.document.catalog_photo = Some(middle);
    assert_eq!(editor.prefetch_neighbour(last), None);
    Ok(())
}
#[test]
fn a_rendered_neighbour_is_prefetched_too() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    for name in ["a.jpg", "b.jpg"] {
        image::RgbImage::new(4, 4).save(dir.path().join(name))?;
    }
    let path = dir.path().join("photos.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(dir.path())?;
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let library = library::Library::load(&path, ctx)?;
    let photos: Vec<_> = library.session.photos.iter().map(|p| p.id).collect();
    let first = *photos
        .iter()
        .find(|p| library.navigate(**p, -1).is_none_or(|q| q == **p))
        .unwrap();
    let second = library.navigate(first, 1).unwrap();
    let expected = library.photo(second).unwrap().path.clone();
    editor.library = Some(Box::new(library));
    assert_eq!(editor.prefetch_neighbour(first), Some(expected));
    Ok(())
}
#[test]
fn auto_is_one_undoable_step_that_keeps_edits_made_while_it_ran() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let (width, height) = (64u32, 48u32);
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width,
        height,
        // A dim, warm gradient: Auto brightens it, and the WB menu's Auto cools it.
        pixels: (0..width * height)
            .map(|i| {
                let v = 0.002 + 0.06 * (i % width) as f32 / width as f32;
                [v * 1.2, v, v * 0.8]
            })
            .collect(),
        metadata: Metadata {
            width,
            height,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    editor.document.edit.setup_mut().saturation = 0.25;
    let before = editor.document.edit.recipe().clone();
    editor.start_auto(worker::AutoKind::Settings);
    assert!(editor.document.auto.is_running());
    // A second request while the first runs is ignored.
    editor.start_auto(worker::AutoKind::Settings);
    editor.document.edit.setup_mut().effects.clarity = 0.25;
    let start = std::time::Instant::now();
    while editor.document.auto.is_running() {
        assert!(start.elapsed().as_secs() < 60, "Auto did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
        editor.events(&ctx);
    }
    let auto = editor.document.edit.recipe().clone();
    assert!(auto.exposure > 1., "exposure {}", auto.exposure);
    // Auto sets the tone sliders, Vibrance and Saturation, as Lightroom's does; white
    // balance is the WB menu's Auto.
    assert!(auto.vibrance > 0., "vibrance {}", auto.vibrance);
    assert_ne!(auto.saturation, before.saturation);
    assert_eq!(
        (auto.wb, auto.temperature, auto.tint),
        (before.wb, before.temperature, before.tint)
    );
    // An edit made while it ran is kept.
    assert_eq!(auto.effects.clarity, 0.25);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 1);
    assert_eq!(steps[0].name, "Auto Settings");
    editor.undo();
    let mut expected = before;
    expected.effects.clarity = 0.25;
    assert_eq!(*editor.document.edit.recipe(), expected);
    editor.redo();
    assert_eq!(*editor.document.edit.recipe(), auto);

    editor.start_auto(worker::AutoKind::WhiteBalance);
    while editor.document.auto.is_running() {
        assert!(start.elapsed().as_secs() < 60, "Auto did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
        editor.events(&ctx);
    }
    let r = editor.document.edit.recipe();
    assert!(r.wb[0] < 1. && r.wb[2] > 1., "wb {:?}", r.wb);
    assert_eq!(r.auto_white_balance, Some([r.temperature, r.tint]));
    assert_eq!(r.exposure, auto.exposure);
    let (steps, _) = editor.document.edit.history().steps();
    assert_eq!(steps[1].name, "White Balance");
    // Pasted onto a photo, the values were not estimated for it: the WB menu says Custom.
    editor.copy_settings(crate::model::settings_groups::GroupSelection::default());
    editor.paste_settings();
    let r = editor.document.edit.recipe();
    assert!(r.wb[0] < 1. && r.wb[2] > 1., "wb {:?}", r.wb);
    assert_eq!(r.auto_white_balance, None);
}
#[test]
fn stale_auto_results_are_ignored_after_moving_on() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let before = editor.document.edit.recipe().clone();
    let mut auto = before.clone();
    auto.exposure = 2.;
    editor
        .tx
        .send(worker::Event::Auto {
            id: editor.load.id() + 1,
            kind: worker::AutoKind::Settings,
            result: Ok(Box::new(auto)),
        })
        .unwrap();
    editor.events(&ctx);
    assert_eq!(*editor.document.edit.recipe(), before);
    assert!(!editor.document.edit.history().can_undo());
}
#[test]
fn auto_shortcut_starts_auto_once_the_photo_is_decoded() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let modifiers = egui::Modifiers {
        command: true,
        mac_cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        shift: true,
        ..Default::default()
    };
    let press = |e: &mut Editor| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::U,
                    physical_key: Some(egui::Key::U),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
        output.textures_delta.clear();
    };
    press(&mut e);
    assert!(!e.document.auto.is_running(), "no photo yet");
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64],
        metadata: Metadata {
            width: 8,
            height: 8,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    press(&mut e);
    assert!(e.document.auto.is_running());
}
#[test]
fn select_all_shortcut_selects_the_filmstrip_for_sync() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[1]);
    e.library.as_mut().unwrap().select(Some(ids[1]));
    let ctx = e.context.clone();
    // Cmd+A on macOS, Ctrl+A elsewhere.
    let modifiers = egui::Modifiers::COMMAND;
    let input = egui::RawInput {
        events: vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers,
            },
        ],
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
    output.textures_delta.clear();
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.selected_photos(), ids);
    assert_eq!(library.selected(), Some(ids[1]));
    Ok(())
}
/// In the Library grid, Shift+Cmd+S opens Sync Settings from the active photo to
/// the rest of the selection, with no photo open in Develop (issue #445).
#[test]
fn library_sync_shortcut_syncs_from_the_active_photo() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let press = |e: &mut Editor| {
        let ctx = e.context.clone();
        let modifiers = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::S,
                    physical_key: Some(egui::Key::S),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| e.draw(ui));
        output.textures_delta.clear();
    };
    // A photo alone has nothing to sync to.
    e.library.as_mut().unwrap().select(Some(ids[0]));
    assert_eq!(e.sync_target_count(), 0);
    press(&mut e);
    assert!(e.modal.is_none());
    // The last one clicked is active, and the source.
    e.library.as_mut().unwrap().select_range_to(ids[2]);
    assert_eq!(e.library.as_ref().unwrap().selected(), Some(ids[2]));
    let targets: Vec<_> = e.sync_targets().into_iter().map(|t| t.id).collect();
    assert_eq!(targets, ids[..2]);
    assert_eq!(e.sync_target_count(), 2);
    press(&mut e);
    assert!(
        matches!(&e.modal, Some(Modal::CopySettings(dialog))
            if dialog.purpose == settings_transfer::Transfer::Sync),
        "Shift+Cmd+S opens Synchronize Settings"
    );
    Ok(())
}
/// From the Library, the photo open in Develop can be a Sync's target: it opens
/// again with what the Sync saved, so its stale edit is not saved over it.
/// From the Library, the photo open in Develop can be a target: while an Auto
/// estimate is still to land on it, it would be saved over what the Sync wrote, so
/// Sync waits.
#[test]
fn library_sync_waits_for_auto_on_the_open_photo() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[2]);
    // ids[0] is open in Develop and a target; ids[2] is the source.
    e.document.catalog_photo = Some(ids[0]);
    let _auto = e.document.auto.start();
    assert!(e.sync_targets().is_empty());
    assert_eq!(e.sync_target_count(), 0);
    e.document.auto = Default::default();
    assert_eq!(e.sync_target_count(), 2);
    // Not selected, it is not changed and does not hold the Sync back.
    let _auto = e.document.auto.start();
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[1]));
    library.select_range_to(ids[2]);
    assert_eq!(e.sync_target_count(), 1);
    Ok(())
}
#[test]
fn a_synced_photo_open_in_develop_opens_again() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_ref().unwrap();
    let path = library.photo(ids[0]).unwrap().path.clone();
    let catalog = library.session.catalog.location().clone();
    e.document.catalog_photo = Some(ids[0]);
    e.document.path = Some(path.clone());
    let synced = |id| sync::Synced {
        id,
        path: path.clone(),
        before: sync::EditBefore::Saved(Box::default()),
        after: Default::default(),
        history: crate::catalog::SavedHistory {
            origin: Default::default(),
            steps: Vec::new(),
            applied: 0,
        },
        history_before: crate::catalog::SavedHistory {
            origin: Default::default(),
            steps: Vec::new(),
            applied: 0,
        },
        identity: crate::storage::Identity::read(&path).unwrap(),
    };
    let result = |id| sync::SyncResult {
        change: sync::BatchChange::Settings(Default::default()),
        catalog: catalog.clone(),
        synced: vec![synced(id)],
        failed: Vec::new(),
        notes: Vec::new(),
        cancelled: false,
    };
    let settled = || {
        save_state::SaveState::Pending(
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        )
    };
    let ctx = e.context.clone();
    // Another photo synced: the open one stays as it is, and saves once it is done.
    assert!(
        e.activity
            .begin_sync(std::sync::Arc::new(sync::BatchProgress::new("Sync", 1)))
    );
    e.document.edit.setup_mut().exposure = 0.4;
    *e.document.edit.save_state_mut() = settled();
    e.autosave(&ctx);
    assert!(!e.autosave.busy(), "nothing saves while a Sync writes");
    e.synced(result(ids[1]));
    assert!(!e.load.is_running());
    assert!(e.document.edit.save_state().needs_save());
    // The open photo synced: what an analysis landing during the Sync changed on it
    // (as Upright's would) is not saved over the Sync, and it opens again.
    assert!(
        e.activity
            .begin_sync(std::sync::Arc::new(sync::BatchProgress::new("Sync", 1)))
    );
    e.document.edit.setup_mut().exposure = 0.9;
    *e.document.edit.save_state_mut() = settled();
    e.autosave(&ctx);
    assert!(!e.autosave.busy(), "nothing saves while a Sync writes");
    // Nor a save asked for directly (Restart to Update, closing).
    assert!(!e.flush());
    assert_eq!(
        e.flush_by(std::time::Instant::now() + std::time::Duration::from_secs(1)),
        workflow::Flushed::Late
    );
    assert!(!e.autosave.busy());
    e.synced(result(ids[0]));
    assert!(e.load.is_running(), "the open photo opens again");
    assert!(!e.document.edit.save_state().needs_save());
    let library = e.library.as_ref().unwrap();
    assert!(library.session.catalog.load_edit(ids[0], &path)?.is_none());
    Ok(())
}
#[test]
fn select_none_shortcut_clears_the_filmstrip_selection() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[1]);
    let ctx = e.context.clone();
    let press = |e: &mut Editor, key: egui::Key, modifiers: egui::Modifiers| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
        output.textures_delta.clear();
    };
    // Cmd+A takes every shown photo, the open one active.
    e.library.as_mut().unwrap().select(Some(ids[1]));
    press(&mut e, egui::Key::A, egui::Modifiers::COMMAND);
    assert_eq!(e.library.as_ref().unwrap().selected_photos(), ids);
    // Cmd+D selects none.
    press(&mut e, egui::Key::D, egui::Modifiers::COMMAND);
    let library = e.library.as_ref().unwrap();
    assert!(library.selected_photos().is_empty());
    assert_eq!(library.selected(), None);
    Ok(())
}
#[test]
fn auto_arriving_mid_drag_lands_between_the_two_halves_of_the_drag() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let start = editor.document.edit.recipe().clone();
    // A Shadows drag is under way when the estimate arrives. Auto sets Shadows too,
    // so the drag does not make the estimate stale.
    editor.document.edit.setup_mut().shadows = 0.1;
    let mid = editor.document.edit.recipe().clone();
    editor
        .document
        .edit
        .history_mut()
        .observe(start.clone(), &mid, true);
    let mut auto = start.clone();
    auto.exposure = 1.;
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    let with_auto = editor.document.edit.recipe().clone();
    assert_eq!(with_auto.exposure, 1.);
    editor.document.edit.setup_mut().shadows = 0.2;
    let end = editor.document.edit.recipe().clone();
    editor
        .document
        .edit
        .history_mut()
        .observe(with_auto.clone(), &end, false);
    let names: Vec<_> = editor
        .document
        .edit
        .history()
        .steps()
        .0
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(names[1], "Auto Settings");
    // Undoing the rest of the drag keeps Auto; the next undo removes only Auto.
    editor.undo();
    assert_eq!(*editor.document.edit.recipe(), with_auto);
    editor.undo();
    assert_eq!(*editor.document.edit.recipe(), mid);
    editor.undo();
    assert_eq!(*editor.document.edit.recipe(), start);
}
#[test]
fn auto_runs_again_when_the_crop_changed_while_it_ran() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64],
        metadata: Metadata {
            width: 8,
            height: 8,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    // An estimate for the uncropped photo arrives after the photo was cropped.
    let mut auto = editor.document.edit.recipe().clone();
    auto.exposure = 1.;
    editor.document.edit.setup_mut().crop = [0.1, 0.1, 0.9, 0.9];
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    assert_eq!(editor.document.edit.recipe().exposure, 0.);
    assert!(!editor.document.edit.history().can_undo());
    assert!(
        editor.document.auto.is_running(),
        "Auto runs again for the crop"
    );
}
#[test]
fn auto_runs_again_when_it_failed_on_settings_changed_since() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64],
        metadata: Metadata {
            width: 8,
            height: 8,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    // An estimate that failed on the uncropped photo arrives after the photo was cropped.
    editor.document.auto_input = Some(editor.document.edit.recipe().clone());
    editor.document.edit.setup_mut().crop = [0.1, 0.1, 0.9, 0.9];
    editor.auto_ready(
        worker::AutoKind::Settings,
        Err("Auto: no usable pixels".into()),
    );
    assert!(
        !editor.messages.line().contains("usable"),
        "{}",
        editor.messages.line()
    );
    assert!(
        editor.document.auto.is_running(),
        "Auto runs again for the crop"
    );
}
#[test]
fn auto_is_off_while_its_settings_stand() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    assert!(!editor.auto_in_effect());
    let mut auto = editor.document.edit.recipe().clone();
    auto.exposure = 1.;
    auto.vibrance = 0.15;
    auto.saturation = 0.02;
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    assert_eq!(editor.document.edit.recipe().vibrance, 0.15);
    assert_eq!(editor.document.edit.recipe().saturation, 0.02);
    assert!(editor.auto_in_effect());
    // Any change, to a slider Auto sets or to what it measured, turns it back on, and
    // so does undoing Auto.
    editor.document.edit.setup_mut().vibrance = 0.;
    assert!(!editor.auto_in_effect());
    editor.document.edit.setup_mut().vibrance = 0.15;
    assert!(editor.auto_in_effect());
    editor.document.edit.setup_mut().saturation = 0.;
    assert!(!editor.auto_in_effect());
    editor.document.edit.setup_mut().saturation = 0.02;
    assert!(editor.auto_in_effect());
    editor.document.edit.setup_mut().exposure = 0.5;
    assert!(!editor.auto_in_effect());
    editor.document.edit.setup_mut().exposure = 1.;
    assert!(editor.auto_in_effect());
    editor.document.edit.setup_mut().crop[0] = 0.1;
    assert!(!editor.auto_in_effect());
    editor.document.edit.setup_mut().crop[0] = 0.;
    assert!(editor.auto_in_effect());
    // Adjustments Auto does not measure leave it off.
    editor.document.edit.setup_mut().effects.clarity = 0.3;
    editor.document.edit.setup_mut().curve.points[0] = [0., 0.2];
    editor.document.edit.setup_mut().preset_name = "Curve only".into();
    assert!(editor.auto_in_effect());
    editor.document.edit.setup_mut().effects.clarity = 0.;
    editor.document.edit.setup_mut().curve.points[0] = [0., 0.];
    editor.undo();
    assert!(!editor.auto_in_effect());
}
#[test]
fn undoing_an_upright_mode_turns_it_off_once_analysed() {
    use crate::model::transform::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let original = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().upright.mode = UprightMode::Vertical;
    e.commit_edit(original, None);
    // The analysis arrives after the click that chose the mode.
    let (generation, _) = e.document.upright.start();
    let analysed = e.document.edit.recipe().clone();
    // Copied to Before while the analysis runs: Before gets it too.
    e.transfer(before_after::Transfer::AfterToBefore);
    let mut corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 6];
    corrections[4][6] = 0.1;
    e.upright_ready(generation, &analysed, Ok(corrections.clone()));
    assert_eq!(e.document.edit.recipe().upright.corrections, corrections);
    assert_eq!(
        e.document.before.as_ref().unwrap().upright.corrections,
        corrections
    );
    // It is not a step of its own: one undo leaves Upright off, redo brings it back
    // corrected.
    assert_eq!(e.document.edit.history().steps().1, 1);
    e.undo();
    assert_eq!(e.document.edit.recipe().upright.mode, UprightMode::Off);
    e.redo();
    assert_eq!(e.document.edit.recipe().upright.mode, UprightMode::Vertical);
    assert_eq!(e.document.edit.recipe().upright.corrections, corrections);
}
#[test]
fn upright_analysis_stays_with_its_photo_and_keeps_imported_guided() {
    use crate::model::transform::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let identity = [1., 0., 0., 0., 1., 0., 0., 0., 1.];
    let mut guided = identity;
    guided[2] = 0.05;
    e.document.edit.setup_mut().upright.mode = UprightMode::Guided;
    e.document.edit.setup_mut().upright.corrections =
        vec![identity, identity, identity, identity, identity, guided];
    // Update analyses the other modes; Lightroom's Guided correction survives.
    let (generation, _) = e.document.upright.start();
    let analysed = e.document.edit.recipe().clone();
    e.upright_ready(generation, &analysed, Ok(vec![identity; 5]));
    assert_eq!(e.document.edit.recipe().upright.corrections[5], guided);
    // Pasted onto another photo, the mode comes along but not the corrections: that
    // photo keeps its own, here none yet, for the editor to analyse.
    e.document.edit.setup_mut().upright.mode = UprightMode::Vertical;
    e.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    e.copy_settings(crate::model::settings_groups::GroupSelection::default());
    e.document.edit.replace(Recipe::default());
    e.paste_settings();
    assert_eq!(e.document.edit.recipe().upright.mode, UprightMode::Vertical);
    assert!(e.document.edit.recipe().upright.corrections.is_empty());
}
#[test]
fn upright_analysis_yields_to_corrections_applied_meanwhile() {
    use crate::model::transform::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.document.edit.setup_mut().upright.mode = UprightMode::Level;
    let (generation, _) = e.document.upright.start();
    let analysed = e.document.edit.recipe().clone();
    // A Lightroom preset with its own corrections lands before the analysis.
    let mut imported = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 6];
    imported[3][2] = 0.02;
    e.document.edit.setup_mut().upright.corrections = imported.clone();
    e.upright_ready(
        generation,
        &analysed,
        Ok(vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 5]),
    );
    assert_eq!(e.document.edit.recipe().upright.corrections, imported);
}

#[test]
fn crop_tool_reads_each_photos_own_aspect() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    let open = |editor: &mut Editor, crop: [f32; 4]| {
        editor.view.tool = state::Tool::None;
        editor.document.edit.setup_mut().crop = crop;
        editor.view.toggle(state::Tool::Crop);
        let frame = editor.begin_edit_frame();
        editor.finish_edit_frame(frame, &ctx);
        assert_eq!(
            editor.document.edit.recipe().crop,
            crop,
            "reading changes no crop"
        );
        editor.view.aspect
    };
    // The last photo's XPan crop does not carry over to an uncropped photo.
    editor.view.aspect = 65. / 24.;
    assert_eq!(open(&mut editor, [0., 0., 1., 1.]), -1.);
    // 300 × 111 is 65 x 24 to within rounding.
    let xpan = [0., 0.223, 1., 0.777];
    assert_eq!(open(&mut editor, xpan), 65. / 24.);
    let custom = open(&mut editor, [0., 0., 0.5, 1.]);
    assert!((custom - 0.75).abs() < 1e-6);
    assert_eq!(open(&mut editor, [0.1, 0.1, 0.9, 0.9]), -1.);
}
#[test]
fn crop_tool_stays_open_from_photo_to_photo() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::session::Session {
            crop_aspect: "1x1".into(),
            ..Default::default()
        },
        None,
    );
    editor.module = Module::Develop;
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    // Another photo, opened while Crop is: the tool stays and reads it as if
    // opened on it.
    let next = |editor: &mut Editor, crop: [f32; 4]| {
        editor.open_raw("missing-crop-fixture.ARW".into(), None);
        assert!(editor.view.is(state::Tool::Crop));
        editor.document.set_image(image.clone());
        editor.document.edit.setup_mut().crop = crop;
        let frame = editor.begin_edit_frame();
        editor.finish_edit_frame(frame, &ctx);
        editor.view.aspect
    };
    let crop = |editor: &Editor| editor.document.edit.recipe().crop;
    let steps = |editor: &Editor| editor.document.edit.history().steps().0.len();
    editor.document.set_image(image.clone());
    editor.view.toggle(state::Tool::Crop);
    // A cropped photo keeps its crop and shows its own aspect: 300 × 111 is
    // 65 x 24 to within rounding.
    let xpan = [0., 0.223, 1., 0.777];
    assert_eq!(next(&mut editor, xpan), 65. / 24.);
    assert_eq!(crop(&editor), xpan);
    assert!(!editor.document.edit.save_state().needs_save());
    // An uncropped photo takes the chosen 1 x 1 as one undoable step.
    let full = [0., 0., 1., 1.];
    let before = steps(&editor);
    assert_eq!(next(&mut editor, full), 1.);
    let c = crop(&editor);
    assert!(((c[2] - c[0]) * 300. / ((c[3] - c[1]) * 200.) - 1.).abs() < 1e-3);
    assert_eq!(steps(&editor), before + 1);
    assert!(editor.document.edit.save_state().needs_save());
    assert!(editor.document.edit.undo());
    assert_eq!(crop(&editor), full);
    // The same photo loaded again (after a Sync's Undo, say) reads the edit it
    // loads: its own crop's aspect, and an uncropped one stays uncropped.
    let path = std::path::PathBuf::from("missing-crop-fixture.ARW");
    let reload = |editor: &mut Editor, crop: [f32; 4]| {
        editor.document.path = Some(path.clone());
        editor.open_raw(path.clone(), None);
        assert!(editor.view.is(state::Tool::Crop));
        editor.document.set_image(image.clone());
        editor.document.edit.setup_mut().crop = crop;
        let frame = editor.begin_edit_frame();
        editor.finish_edit_frame(frame, &ctx);
        assert_eq!(editor.document.edit.recipe().crop, crop);
        assert_eq!(editor.document.edit.history().steps().0.len(), 0);
        assert!(!editor.document.edit.save_state().needs_save());
        editor.view.aspect
    };
    assert_eq!(reload(&mut editor, xpan), 65. / 24.);
    assert_eq!(reload(&mut editor, full), -1.);
    // Removing the photo closes it.
    editor.close_removed_document();
    assert!(editor.view.is(state::Tool::None));
}
#[test]
fn choosing_the_photos_own_aspect_leaves_an_uncropped_photo_alone() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    // 100 × 99: the photo's ratio over itself is not exactly 1 in f32.
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 100,
        height: 99,
        pixels: vec![[0.1; 3]; 9900],
        metadata: Metadata {
            width: 100,
            height: 99,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    editor.view.toggle(state::Tool::Crop);
    let frame = editor.begin_edit_frame();
    editor.finish_edit_frame(frame, &ctx);
    let steps = editor.document.edit.history().steps().0.len();
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(-1.);
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(editor.document.edit.recipe().crop, [0., 0., 1., 1.]);
    assert_eq!(editor.document.edit.history().steps().0.len(), steps);
}
#[test]
fn fitting_a_crop_dragged_to_its_smallest_keeps_it_valid() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    editor.view.tool = state::Tool::Crop;
    editor.view.aspect = -1.;
    // A handle dragged to the smallest crop: 0.11 - 0.1 is just under 0.01 in f32,
    // though the crop is already at the photo's aspect.
    let frame = editor.begin_edit_frame();
    editor.document.edit.recipe_mut().crop = [0.1, 0.1, 0.1 + 0.01, 0.1 + 0.01];
    editor.fit_aspect();
    editor.finish_edit_frame(frame, &ctx);
    editor.document.edit.recipe().validate().unwrap();
}
#[test]
fn the_chosen_aspect_is_its_own_edit_before_the_next_operation() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::session::Session {
            crop_aspect: "1x1".into(),
            ..Default::default()
        },
        None,
    );
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    editor.document.edit.change(None, |r| r.exposure = 1.25);
    editor.sync_command_revision();
    let revision = editor.automation.revision;
    let steps = editor.document.edit.history().steps().0.len();
    // A queued command opens Crop; the next command's frame (a Navigate, say, which
    // drops its frame with the photo) finds the photo uncropped.
    editor.view.toggle(state::Tool::Crop);
    let frame = editor.begin_edit_frame();
    drop(frame);
    // The crop is already recorded and to be saved, and an Undo takes back the crop
    // rather than the edit before it.
    let c = editor.document.edit.recipe().crop;
    assert!(((c[2] - c[0]) * 300. / ((c[3] - c[1]) * 200.) - 1.).abs() < 1e-3);
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    assert!(editor.document.edit.save_state().needs_save());
    // Commands see the cropped settings as a revision of their own, before the
    // frame's command reads them.
    assert_eq!(editor.automation.revision, revision + 1);
    editor.sync_command_revision();
    assert_eq!(editor.automation.revision, revision + 1);
    assert!(editor.document.edit.undo());
    assert_eq!(editor.document.edit.recipe().crop, [0., 0., 1., 1.]);
    assert_eq!(editor.document.edit.recipe().exposure, 1.25);
}
#[test]
fn the_chosen_aspect_ends_a_held_slider_drag_first() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::session::Session {
            crop_aspect: "1x1".into(),
            ..Default::default()
        },
        None,
    );
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    // Exposure dragged to +1.25, the button still held as R opens Crop.
    let frame = editor.document.edit.begin();
    editor.document.edit.recipe_mut().exposure = 1.25;
    editor
        .document
        .edit
        .finish(frame, crate::edit_session::Gesture::Held);
    editor.view.toggle(state::Tool::Crop);
    let frame = editor.begin_edit_frame();
    editor.finish_edit_frame(frame, &ctx);
    editor.document.edit.finish_gesture();
    // The drag and the crop are a step each: Undo takes back the crop alone, then
    // the exposure.
    assert!(editor.document.edit.undo());
    assert_eq!(editor.document.edit.recipe().crop, [0., 0., 1., 1.]);
    assert_eq!(editor.document.edit.recipe().exposure, 1.25);
    assert!(editor.document.edit.undo());
    assert_eq!(editor.document.edit.recipe().exposure, 0.);
}
#[test]
fn crop_tool_keeps_the_chosen_aspect_for_uncropped_photos() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let session = dir.path().join("session.json");
    let ctx = egui::Context::default();
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    let new_editor = |s: crate::app::session::Session| {
        let mut editor = Editor::with_context(&ctx, None, s, None);
        editor.session_file = Some(session.clone());
        editor.document.set_image(image.clone());
        editor
    };
    let open = |editor: &mut Editor, crop: [f32; 4]| {
        editor.view.tool = state::Tool::None;
        editor.document.edit.setup_mut().crop = crop;
        editor.view.toggle(state::Tool::Crop);
        let frame = editor.begin_edit_frame();
        editor.finish_edit_frame(frame, &ctx);
        editor.view.aspect
    };
    let crop = |editor: &Editor| editor.document.edit.recipe().crop;
    // The crop's width over its height on the 300 × 200 photo.
    let ratio = |editor: &Editor| {
        let c = crop(editor);
        (c[2] - c[0]) * 300. / ((c[3] - c[1]) * 200.)
    };
    let steps = |editor: &Editor| editor.document.edit.history().steps().0.len();
    let mut editor = new_editor(Default::default());
    let full = [0., 0., 1., 1.];
    // Until one is chosen an uncropped photo shows Original, and stays uncropped.
    let before = steps(&editor);
    assert_eq!(open(&mut editor, full), -1.);
    assert_eq!(crop(&editor), full);
    assert_eq!(steps(&editor), before);
    // 4 x 5 chosen from the menu constrains this photo's crop...
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(1.25);
    editor.finish_edit_frame(frame, &ctx);
    assert!((ratio(&editor) - 1.25).abs() < 1e-3);
    // ...and the next uncropped photo takes it as the tool opens, as one undoable step.
    let before = steps(&editor);
    assert_eq!(open(&mut editor, full), 1.25);
    assert!((ratio(&editor) - 1.25).abs() < 1e-3);
    assert_eq!(steps(&editor), before + 1);
    assert!(editor.document.edit.undo());
    assert_eq!(crop(&editor), full);
    // Choosing it again from the menu crops that photo again.
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(1.25);
    editor.finish_edit_frame(frame, &ctx);
    assert!((ratio(&editor) - 1.25).abs() < 1e-3);
    // A cropped photo keeps its crop and shows its own aspect, and does not replace
    // the choice.
    let xpan = [0., 0.223, 1., 0.777];
    assert_eq!(open(&mut editor, xpan), 65. / 24.);
    assert_eq!(crop(&editor), xpan);
    assert_eq!(open(&mut editor, full), 1.25);
    // Swapping a photo's crop to portrait leaves the choice following each photo.
    let frame = editor.begin_edit_frame();
    editor.swap_crop_orientation();
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(open(&mut editor, full), 1.25);
    assert!((ratio(&editor) - 1.25).abs() < 1e-3);
    // Free is a choice too, and leaves an uncropped photo uncropped.
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(0.);
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(open(&mut editor, full), 0.);
    assert_eq!(crop(&editor), full);
    // The choice is kept in the session for the next launch.
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(16. / 9.);
    editor.finish_edit_frame(frame, &ctx);
    let saved: crate::app::session::Session = serde_json::from_slice(&std::fs::read(&session)?)?;
    assert_eq!(saved.crop_aspect, "16x9");
    let mut relaunched = new_editor(saved);
    assert_eq!(open(&mut relaunched, full), 16. / 9.);
    assert!((ratio(&relaunched) - 16. / 9.).abs() < 1e-3);
    // A name this version does not know reads as Original.
    let mut unknown = new_editor(crate::app::session::Session {
        crop_aspect: "golden".into(),
        ..Default::default()
    });
    assert_eq!(open(&mut unknown, full), -1.);
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
    let id = l.session.photos[0].id;
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.module = Module::Develop;
    editor.document.catalog_photo = Some(id);
    editor.document.path = Some(photo.clone());
    editor.document.edit.setup_mut().exposure = 0.7;
    editor.document.edit.save_state_mut().mark_changed();
    editor.virtual_copy(library::CopyAction::Create(id));
    let library = editor.library.as_ref().unwrap();
    let copy = library.session.photos.iter().find(|p| p.id != id).unwrap();
    assert_eq!((copy.master, copy.copy_name.as_str()), (Some(id), "Copy 1"));
    assert_eq!(library.selected(), Some(copy.id));
    assert_eq!(editor.document.catalog_photo, Some(copy.id));
    for photo_id in [id, copy.id] {
        let saved = library
            .session
            .catalog
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
    let master = l.session.photos[0].id;
    let copy = l.create_virtual_copy(master)?.value;
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
    let copy = l.create_virtual_copy(l.session.photos[0].id)?.value;
    // A copy that is gone from the catalog cannot be renamed.
    l.session.catalog.remove_virtual_copy(copy)?;
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
    let copy = l.create_virtual_copy(l.session.photos[0].id)?.value;
    l.set_copy_as_master(copy)?;
    let path = l.photo(copy).unwrap().path.clone();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(l));
    assert_eq!(editor.catalog_photo_at(&path), Some(copy));
    Ok(())
}
/// An editor with a catalog of `names`, its Library open.
pub(in crate::app) fn editor_with_catalog(
    names: &[&str],
) -> anyhow::Result<(tempfile::TempDir, Editor, Vec<PhotoId>)> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    for name in names {
        std::fs::write(photos.join(name), b"fixture")?;
    }
    let path = d.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let library = crate::app::library::Library::load(&path, ctx)?;
    let ids = library.session.photos.iter().map(|p| p.id).collect();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    Ok((d, e, ids))
}
#[test]
fn undo_brings_back_a_range_rejected_under_the_unflagged_filter() -> anyhow::Result<()> {
    use crate::app::photo_metadata::Edit;
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
    assert!(library.session.photos.iter().all(|p| p.flag == 0));
    assert!(e.messages.line().starts_with("Undo 3 photos"));
    e.redo();
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.shown().len(), 2);
    assert_eq!(
        library
            .session
            .photos
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
    use crate::app::photo_metadata::Edit;
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.path = Some(d.path().join("photos/a.RAF"));
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
#[test]
fn undoing_a_history_click_returns_to_the_exact_step() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
    use crate::app::photo_metadata::Edit;
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[1]));
    library.edit_selection(Edit::Rating(4), false)?;
    e.sync_undo();
    // Off to Develop on the other photo.
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.path = Some(d.path().join("photos/a.RAF"));
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
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
    use crate::app::photo_metadata::Edit;
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
fn press(e: &mut Editor, key: egui::Key, modifiers: egui::Modifiers) {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            // Pressed and released, as a real key is, so the next press of
            // the same key is not a repeat.
            events: [true, false]
                .map(|pressed| egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers,
                })
                .into(),
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
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
    e.auto_advance = true;
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
        crate::catalog::Catalog::open(e.library.as_ref().unwrap().session.catalog.location())?;
    let quick = reopened
        .collections()?
        .into_iter()
        .find(|c| c.name == crate::catalog::QUICK_COLLECTION)
        .unwrap();
    assert_eq!(reopened.collection_photos()?[&quick.id].len(), 2);
    Ok(())
}

#[test]
fn the_preset_list_is_kept_until_what_it_shows_changes() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let preset = |id: &str, group: &str, name: &str| crate::xmp::Preset {
        id: id.into(),
        name: name.into(),
        group: group.into(),
        path: Default::default(),
        settings: Default::default(),
        curves: Default::default(),
        look: String::new(),
        blockers: Vec::new(),
        notes: Vec::new(),
        photo_settings: false,
        local: Default::default(),
        builtin: false,
    };
    editor.presets.library = std::sync::Arc::new(crate::presets::Library {
        presets: vec![
            preset("a", "Film", "Warm⁺"),
            preset("b", "Film", "Cool"),
            preset("c", "Mono", "Grain"),
        ],
        errors: Vec::new(),
    });
    let names = |editor: &mut Editor| -> Vec<(String, Vec<String>)> {
        editor
            .preset_list()
            .iter()
            .map(|g| {
                (
                    g.name.clone(),
                    g.presets.iter().map(|(_, n)| n.clone()).collect(),
                )
            })
            .collect()
    };
    assert_eq!(
        names(&mut editor),
        [
            ("Film".into(), vec!["Warm+".into(), "Cool".into()]),
            ("Mono".into(), vec!["Grain".into()])
        ]
    );
    // Kept while nothing it depends on changes.
    let first = editor.preset_list();
    assert!(std::sync::Arc::ptr_eq(&first, &editor.preset_list()));
    // Rebuilt for a search, and for a favorite.
    editor.presets.filter = "WARM+".into();
    assert_eq!(names(&mut editor), [("Film".into(), vec!["Warm+".into()])]);
    editor.presets.filter.clear();
    editor.presets.favorites_only = true;
    assert!(names(&mut editor).is_empty());
    editor.presets.favorites.insert("c".into());
    editor.presets.revision += 1;
    assert_eq!(names(&mut editor), [("Mono".into(), vec!["Grain".into()])]);
    // Rebuilt when another photo's compatibility replaces this one's.
    editor.presets.favorites_only = false;
    editor.presets.compatible_only = true;
    editor.presets.issues = vec![Some("no".into()), None, Some("no".into())];
    editor.presets.revision += 1;
    assert_eq!(names(&mut editor), [("Film".into(), vec!["Cool".into()])]);
    editor.presets.clear_document();
    assert_eq!(names(&mut editor).len(), 2);
}
#[test]
fn double_clicking_a_defringe_hue_resets_it_to_its_colors_default() {
    let ctx = egui::Context::default();
    let mut effects = crate::model::effects::Effects {
        // Green below its default, as the fringe selector can leave it.
        defringe_ranges: [[0.1, 0.95], [0.0, 0.2]],
        ..Default::default()
    };
    let mut time = 0.;
    let mut frame =
        |effects: &mut crate::model::effects::Effects, events: Vec<egui::Event>, wait: f64| {
            time += wait;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320., 400.))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| super::inspector::defringe_sliders(ui, effects),
            );
            output.textures_delta.clear();
        };
    // Six rows, Purple then Green: Amount, Hue (low end), Hue (high end).
    frame(&mut effects, vec![], 0.);
    let row = 24. + ctx.global_style().spacing.item_spacing.y;
    // Either end resets the whole range: Purple's high end, Green's low end.
    for i in [2, 4] {
        let at = Pos2::new(
            40.,
            ctx.global_style().spacing.window_margin.top as f32 + row * i as f32 + 12.,
        );
        let click = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Apart in time, so egui never counts a triple click.
        frame(&mut effects, vec![egui::Event::PointerMoved(at)], 1.);
        for pressed in [true, false, true, false] {
            frame(&mut effects, vec![click(pressed)], 0.05);
        }
    }
    assert_eq!(effects.defringe_ranges, [[0.3, 0.7], [0.4, 0.6]]);
    // History names the range reset, not the value the clamped end briefly took.
    let step: Option<(String, String)> =
        ctx.data(|d| d.get_temp(super::widgets::history_step_id()));
    assert_eq!(
        step,
        Some(("Defringe Green Hue".to_string(), "40 / 60".to_string()))
    );
}
#[test]
fn paste_works_out_white_balance_for_this_camera_and_previous_pastes_the_last_photo() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let camera = |wb: [f32; 3]| Metadata {
        wb,
        daylight_wb: wb,
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let (first, second) = (camera([2., 1., 1.8]), camera([2.6, 1., 1.3]));
    // A photo from one camera, its settings copied.
    editor.document.metadata = Some(first.clone());
    let mut copied = Recipe {
        temperature: 4200.,
        tint: 8.,
        exposure: 0.4,
        ..Default::default()
    };
    copied.update_wb(&first);
    editor.document.edit.replace(copied.clone());
    editor.copy_settings(crate::model::settings_groups::GroupSelection::default());
    // Pasted onto a photo from another camera.
    editor.document.metadata = Some(second.clone());
    editor.document.edit.replace(Recipe::default());
    editor.paste_settings();
    let pasted = editor.document.edit.recipe().clone();
    assert_eq!((pasted.temperature, pasted.exposure), (4200., 0.4));
    let mut expected = pasted.clone();
    expected.update_wb(&second);
    assert_eq!(pasted.wb, expected.wb);
    assert_ne!(pasted.wb, copied.wb);
    // Moving to another photo makes this one's settings the Previous; opening that
    // photo again leaves it.
    let other = std::path::PathBuf::from("missing-previous-fixture.ARW");
    editor.open_raw(other.clone(), None);
    editor.document.metadata = Some(first);
    editor.document.path = Some(other.clone());
    editor.document.edit.setup_mut().exposure = -1.;
    editor.open_raw(other, None);
    editor.document.metadata = Some(second);
    editor.paste_previous();
    assert_eq!(editor.document.edit.recipe().exposure, 0.4);
}
#[test]
fn copy_settings_copies_the_chosen_groups_and_remembers_them() {
    use crate::model::settings_groups::{GroupInclusion, GroupSelection, SettingGroup};
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    editor.document.edit.setup_mut().exposure = 0.6;
    editor.document.edit.setup_mut().contrast = 0.3;
    editor.open_copy_dialog(settings_transfer::Transfer::Copy);
    // The dialog draws, with its buttons in view in the smallest window.
    for _ in 0..2 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000., 650.))),
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
    }
    let copy = ctx.memory(|m| m.area_rect(egui::Id::new("copy-settings")));
    assert!(copy.is_some_and(|r| r.bottom() <= 650.), "{copy:?}");
    let Some(Modal::CopySettings(dialog)) = &mut editor.modal else {
        panic!("Copy Settings is open");
    };
    dialog.groups = GroupSelection::none();
    dialog
        .groups
        .set(SettingGroup::Exposure, GroupInclusion::Included);
    editor.close_copy_dialog(settings_transfer::CopyChoice::Confirm);
    assert!(editor.modal.is_none());
    editor.document.edit.replace(Recipe::default());
    editor.paste_settings();
    assert_eq!(editor.document.edit.recipe().exposure, 0.6);
    assert_eq!(editor.document.edit.recipe().contrast, 0.);
    // The next Copy Settings starts from that choice; Cancel copies nothing.
    editor.open_copy_dialog(settings_transfer::Transfer::Copy);
    let Some(Modal::CopySettings(dialog)) = &editor.modal else {
        panic!("Copy Settings is open");
    };
    assert!(dialog.groups.contains(SettingGroup::Exposure));
    assert!(!dialog.groups.contains(SettingGroup::Contrast));
    editor.document.edit.setup_mut().exposure = -1.;
    editor.close_copy_dialog(settings_transfer::CopyChoice::Cancel);
    editor.document.edit.replace(Recipe::default());
    editor.paste_settings();
    assert_eq!(editor.document.edit.recipe().exposure, 0.6);
}
#[test]
fn return_confirms_copy_settings_and_escape_cancels_it() {
    use crate::model::settings_groups::GroupSelection;
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    editor.document.edit.setup_mut().exposure = 0.6;
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000., 650.))),
                events,
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
    };
    // Escape cancels: nothing is copied.
    editor.open_copy_dialog(settings_transfer::Transfer::Copy);
    frame(&mut editor, vec![]);
    frame(&mut editor, vec![key(egui::Key::Escape)]);
    assert!(editor.modal.is_none());
    assert!(editor.clipboard.is_none());
    // Return confirms, even with a checkbox focused, which keeps its state.
    editor.open_copy_dialog(settings_transfer::Transfer::Copy);
    let chosen = GroupSelection::default();
    frame(&mut editor, vec![]);
    frame(&mut editor, vec![key(egui::Key::Tab)]);
    assert!(
        ctx.memory(|m| m.focused().is_some()),
        "a checkbox has focus"
    );
    frame(&mut editor, vec![key(egui::Key::Enter)]);
    frame(&mut editor, vec![]);
    assert!(editor.modal.is_none());
    let clipboard = editor.clipboard.as_ref().expect("settings copied");
    assert_eq!(clipboard.groups, chosen);
}
#[test]
fn j_toggles_both_clipping_warnings_but_not_while_typing() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let mut text = String::new();
    let j = || egui::Event::Key {
        key: egui::Key::J,
        physical_key: Some(egui::Key::J),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let mut frame = |events: Vec<egui::Event>, typing: bool, e: &mut Editor| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                if typing {
                    ui.text_edit_singleline(&mut text).request_focus();
                }
                e.develop_shortcuts(&ctx);
            },
        );
        output.textures_delta.clear();
    };
    frame(vec![], false, &mut e);
    frame(vec![j()], false, &mut e);
    let both = crate::rendered::ClipOverlay {
        shadows: true,
        highlights: true,
    };
    assert_eq!(e.view.clipping.overlay(), both);
    // A J typed into a field stays there.
    frame(vec![], true, &mut e);
    frame(vec![j()], true, &mut e);
    assert_eq!(e.view.clipping.overlay(), both);
    frame(vec![], false, &mut e);
    frame(vec![j()], false, &mut e);
    assert_eq!(
        e.view.clipping.overlay(),
        crate::rendered::ClipOverlay::NONE
    );
}
#[test]
fn a_hovered_clipping_triangle_shows_its_warning_until_the_pointer_leaves() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let frame = e.begin_edit_frame();
    // As the histogram does while its highlight triangle is under the pointer.
    e.view
        .clipping
        .set_hover(Some(super::clipping::ClipSide::Highlights));
    e.finish_edit_frame(frame, &ctx);
    assert!(e.view.clipping.overlay().highlights);
    // The next frame draws no hovered triangle: the warning goes, and nothing
    // was turned on.
    let frame = e.begin_edit_frame();
    e.finish_edit_frame(frame, &ctx);
    assert_eq!(
        e.view.clipping.overlay(),
        crate::rendered::ClipOverlay::NONE
    );
}
#[test]
fn crop_keys_swap_and_cycle_the_overlay_but_not_while_typing() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let session = d.path().join("session.json");
    e.session_file = Some(session.clone());
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    let crop = [0.1, 0.2, 0.5, 0.6];
    e.document.edit.setup_mut().crop = crop;
    e.view.toggle(state::Tool::Crop);
    let ctx = e.context.clone();
    let press = |e: &mut Editor, key: egui::Key, shift: bool, typing: bool| {
        let modifiers = egui::Modifiers {
            shift,
            ..Default::default()
        };
        let mut text = String::new();
        // A text field takes focus in one frame and has it for the keys in the next.
        for events in [vec![], vec![true, false]] {
            let input = egui::RawInput {
                events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                    .chain(events.into_iter().map(|pressed| egui::Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed,
                        repeat: false,
                        modifiers,
                    }))
                    .collect(),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                if typing {
                    ui.text_edit_singleline(&mut text).request_focus();
                }
                e.metadata_shortcuts(&ctx);
                let frame = e.begin_edit_frame();
                e.develop_shortcuts(&ctx);
                e.finish_edit_frame(frame, &ctx);
            });
            output.textures_delta.clear();
        }
    };
    // Typing an X or an O in a field changes nothing.
    press(&mut e, egui::Key::X, false, true);
    press(&mut e, egui::Key::O, false, true);
    assert_eq!(e.document.edit.recipe().crop, crop);
    assert_eq!(e.view.crop_guides, Default::default());
    // X swaps the 120 × 80 crop to 80 × 120 about its centre, and rejects nothing.
    press(&mut e, egui::Key::X, false, false);
    let c = e.document.edit.recipe().crop;
    assert!(
        ((c[2] - c[0]) * 300. - 80.).abs() < 1e-3 && ((c[3] - c[1]) * 200. - 120.).abs() < 1e-3,
        "{c:?}"
    );
    assert!(((c[0] + c[2]) / 2. - 0.3).abs() < 1e-6 && ((c[1] + c[3]) / 2. - 0.4).abs() < 1e-6);
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag, 0);
    // O cycles the overlay and Shift+O turns it; both are kept in the session.
    use super::crop_tool::Guide;
    press(&mut e, egui::Key::O, false, false);
    assert_eq!(e.view.crop_guides.guide, Guide::Diagonal);
    press(&mut e, egui::Key::O, false, false);
    press(&mut e, egui::Key::O, true, false);
    assert_eq!(
        (e.view.crop_guides.guide, e.view.crop_guides.orientation),
        (Guide::Triangle, 1)
    );
    // Shift released before the frame is drawn: the press still had it.
    let shifted = egui::Modifiers {
        shift: true,
        ..Default::default()
    };
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(shifted),
                egui::Event::Key {
                    key: egui::Key::O,
                    physical_key: Some(egui::Key::O),
                    pressed: true,
                    repeat: false,
                    modifiers: shifted,
                },
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            ],
            ..Default::default()
        },
        |_| e.develop_shortcuts(&ctx),
    );
    output.textures_delta.clear();
    assert_eq!(
        (e.view.crop_guides.guide, e.view.crop_guides.orientation),
        (Guide::Triangle, 0)
    );
    press(&mut e, egui::Key::O, true, false);
    let saved: crate::app::session::Session = serde_json::from_slice(&std::fs::read(&session)?)?;
    assert_eq!(saved.crop_guides.guide, "triangle");
    assert_eq!(saved.crop_guides.orientation, 1);
    // Outside the Crop tool X rejects as before.
    e.view.tool = state::Tool::None;
    press(&mut e, egui::Key::X, false, false);
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag, -1);
    Ok(())
}

/// Stripes falling 3° to the right, which Auto straighten levels at about −3°.
fn falling_stripes() -> Arc<CameraImage> {
    let (w, h) = (600usize, 400usize);
    let (s, c) = 3f32.to_radians().sin_cos();
    let pixels = (0..w * h)
        .map(|i| {
            // Supersampled, so edges are smooth rather than pixel steps.
            let mut sum = 0.;
            for j in 0..4 {
                let x = (i % w) as f32 + (j % 2) as f32 * 0.5 - 299.75;
                let y = (i / w) as f32 + (j / 2) as f32 * 0.5 - 199.75;
                let across = -s * x + c * y;
                sum += if (across / 70.).rem_euclid(2.) < 1. {
                    0.78
                } else {
                    0.1
                };
            }
            [sum / 4.; 3]
        })
        .collect();
    Arc::new(CameraImage {
        recovered: Default::default(),
        width: w as u32,
        height: h as u32,
        pixels,
        metadata: Metadata {
            width: w as u32,
            height: h as u32,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    })
}

#[test]
fn auto_straighten_sets_the_level_angle_as_one_step_and_measures_again_after_a_turn() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.document.set_image(falling_stripes());
    e.document.edit.setup_mut().wb = [1.; 3];
    let upright = e.document.edit.recipe().upright.clone();
    let next = |e: &mut Editor| loop {
        match e.rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(worker::Event::Straighten {
                generation,
                analysed,
                result,
                ..
            }) => break (generation, analysed, result),
            Ok(_) => continue,
            Err(err) => panic!("no Auto straighten result: {err}"),
        }
    };
    e.start_auto_straighten();
    assert!(e.document.straighten.is_running());
    // Measured before the photo was turned: it is measured again, and nothing changes yet.
    let (generation, analysed, result) = next(&mut e);
    crate::develop::turn(
        e.document.edit.setup_mut(),
        crate::develop::QuarterTurn::Right,
    );
    crate::develop::turn(
        e.document.edit.setup_mut(),
        crate::develop::QuarterTurn::Left,
    );
    e.document.edit.setup_mut().flip_x = true;
    e.auto_straighten_ready(generation, &analysed, result);
    assert_eq!(e.document.edit.recipe().straighten, 0.);
    assert!(e.document.straighten.is_running(), "measured again");
    e.document.edit.setup_mut().flip_x = false;
    let (generation, analysed, result) = next(&mut e);
    e.auto_straighten_ready(generation, &analysed, result);
    // The stale analysis restarted once more for the flip back; take its result.
    let (generation, analysed, result) = next(&mut e);
    e.auto_straighten_ready(generation, &analysed, result);
    let angle = e.document.edit.recipe().straighten;
    assert!((angle + 3.).abs() < 0.2, "{angle}");
    let (steps, applied) = e.document.edit.history().steps();
    assert_eq!(applied, 1);
    assert_eq!(
        (steps[0].name.as_str(), steps[0].value.as_str()),
        ("Straighten", "Auto")
    );
    // Upright is left as it was.
    assert_eq!(e.document.edit.recipe().upright, upright);
    e.undo();
    assert_eq!(e.document.edit.recipe().straighten, 0.);
}

/// Pasting the Crop of a photo whose angle was Auto measures this photo, as Sync does
/// on the photos it changes; the angle found belongs to the Paste's step, which Undo
/// and Redo take back and forth without measuring again.
#[test]
fn pasting_an_auto_straighten_measures_this_photo_within_the_paste() {
    use crate::model::transform::AutoStraighten;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = falling_stripes();
    e.document.metadata = Some(image.metadata.clone());
    e.document.set_image(image);
    e.document.edit.setup_mut().wb = [1.; 3];
    // The Crop copied from a photo Auto turned by 5°.
    e.document.edit.setup_mut().set_auto_straighten(Some(5.));
    let mut crop = crate::model::settings_groups::GroupSelection::none();
    crop.set(
        crate::model::settings_groups::SettingGroup::Crop,
        crate::model::settings_groups::GroupInclusion::Included,
    );
    e.copy_settings(crop);
    *e.document.edit.setup_mut() = Recipe {
        wb: [1.; 3],
        ..Default::default()
    };
    // Pasted as the menu does, within a frame, which records the step.
    let frame = e.begin_edit_frame();
    e.paste_settings();
    e.finish_edit_frame(frame, &ctx);
    let r = e.document.edit.recipe();
    assert_eq!(r.straighten_auto, Some(AutoStraighten::Pending(0.)));
    assert!(e.document.straighten.is_running());
    let (generation, analysed, result) = loop {
        match e.rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(worker::Event::Straighten {
                generation,
                analysed,
                result,
                ..
            }) => break (generation, analysed, result),
            Ok(_) => continue,
            Err(err) => panic!("no Auto straighten result: {err}"),
        }
    };
    e.auto_straighten_ready(generation, &analysed, result);
    let r = e.document.edit.recipe();
    let angle = r.straighten;
    assert!((angle + 3.).abs() < 0.2, "{angle}");
    assert_eq!(r.auto_straighten(), Some(AutoStraighten::Measured(angle)));
    assert!(!e.document.straighten.is_running());
    let (steps, applied) = e.document.edit.history().steps();
    assert_eq!(applied, 1);
    assert_eq!(steps[0].name, "Paste Settings");
    e.undo();
    assert_eq!(e.document.edit.recipe().straighten, 0.);
    assert_eq!(e.document.edit.recipe().auto_straighten(), None);
    e.redo();
    assert_eq!(e.document.edit.recipe().straighten, angle);
    assert!(!e.document.straighten.is_running());
}

/// A photo flipped while its pasted Auto straighten is being measured is measured
/// again as it is now, rather than left waiting or given the stale angle.
#[test]
fn a_pasted_auto_straighten_flipped_before_it_is_measured_is_measured_again() {
    use crate::model::transform::AutoStraighten;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let image = falling_stripes();
    e.document.metadata = Some(image.metadata.clone());
    e.document.set_image(image);
    e.document.edit.setup_mut().set_auto_straighten(Some(5.));
    let mut crop = crate::model::settings_groups::GroupSelection::none();
    crop.set(
        crate::model::settings_groups::SettingGroup::Crop,
        crate::model::settings_groups::GroupInclusion::Included,
    );
    e.copy_settings(crop);
    *e.document.edit.setup_mut() = Recipe {
        wb: [1.; 3],
        straighten: 1.,
        ..Default::default()
    };
    e.paste_settings();
    assert!(e.document.straighten.is_running());
    crate::develop::mirror(
        e.document.edit.setup_mut(),
        crate::develop::Mirror::Horizontal,
    );
    assert_eq!(
        e.document.edit.recipe().straighten_auto,
        Some(AutoStraighten::Pending(-1.))
    );
    let next = |e: &mut Editor| loop {
        match e.rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(worker::Event::Straighten {
                generation,
                analysed,
                result,
                ..
            }) => break e.auto_straighten_ready(generation, &analysed, result),
            Ok(_) => continue,
            Err(err) => panic!("no Auto straighten result: {err}"),
        }
    };
    // Measured before the flip: it doesn't fit, and the photo is measured again.
    next(&mut e);
    assert!(e.document.edit.recipe().straighten_pending());
    assert!(e.document.straighten.is_running(), "measured again");
    next(&mut e);
    // Flipped, the stripes rise to the right.
    let r = e.document.edit.recipe();
    assert!((r.straighten - 3.).abs() < 0.2, "{}", r.straighten);
    assert_eq!(
        r.auto_straighten(),
        Some(AutoStraighten::Measured(r.straighten))
    );
}

#[test]
#[ignore = "writes an image to look at"]
fn render_the_library_loupe_toolbar() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::new(300, 200).save(folder.join("a.jpg"))?;
    let path = directory.path().join("library.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
    super::icons::install(&ctx);
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let mut library = library::Library::load(&path, ctx.clone())?;
    library.select(Some(library.session.photos[0].id));
    library.open_loupe();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    let mut textures = Default::default();
    let image =
        super::ui_snapshot::render(&ctx, &mut textures, Vec2::new(900., 60.), 2., 3, |ui| {
            e.library_loupe_toolbar(ui);
        });
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/library-loupe-toolbar.png");
    image.save(&out)?;
    Ok(())
}
#[test]
#[ignore = "writes an image to look at"]
fn render_the_develop_right_panel() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
    super::icons::install(&ctx);
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    let mut textures = Default::default();
    let image =
        super::ui_snapshot::render(&ctx, &mut textures, Vec2::new(340., 420.), 2., 3, |ui| {
            let frame = e.begin_edit_frame();
            e.develop_right_panel(ui);
            e.finish_edit_frame(frame, &ctx);
        });
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/develop-right-panel.png");
    image.save(&out)?;
    Ok(())
}
#[test]
#[ignore = "writes an image to look at"]
fn render_the_develop_toolbar() -> anyhow::Result<()> {
    for width in [1400., 900., 760.] {
        let ctx = egui::Context::default();
        crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
        super::icons::install(&ctx);
        let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
        e.module = Module::Develop;
        let mut textures = Default::default();
        let image =
            super::ui_snapshot::render(&ctx, &mut textures, Vec2::new(width, 60.), 2., 3, |ui| {
                e.toolbar(ui);
            });
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("target/develop-toolbar-{width}.png"));
        image.save(&out)?;
        eprintln!("{}", out.display());
    }
    Ok(())
}
#[test]
#[ignore = "writes an image to look at"]
fn render_the_crop_drawer() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
    super::icons::install(&ctx);
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.view.tool = state::Tool::Crop;
    let mut textures = Default::default();
    let image =
        super::ui_snapshot::render(&ctx, &mut textures, Vec2::new(320., 300.), 2., 3, |ui| {
            let _labels =
                super::widgets::LabelColumn::enter(ui, super::inspector::DRAWER_LABEL_COLUMN);
            let palette = super::theme::palette(ui.ctx());
            ui.painter()
                .rect_filled(ui.max_rect(), 0., palette.tone(super::design::Tone::CARD));
            let frame = e.begin_edit_frame();
            e.tool_drawer(ui);
            e.finish_edit_frame(frame, &ctx);
        });
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/crop-drawer.png");
    image.save(&out)?;
    eprintln!("{}", out.display());
    Ok(())
}
#[test]
fn the_crop_drawer_keeps_its_layout_whatever_its_buttons_show() {
    let ctx = egui::Context::default();
    super::icons::install(&ctx);
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.view.tool = state::Tool::Crop;
    let size = |e: &mut Editor, width: f32| {
        let mut rect = Rect::NOTHING;
        for _ in 0..2 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(width);
                    let frame = e.begin_edit_frame();
                    rect = ui.scope(|ui| e.tool_drawer(ui)).response.rect;
                    e.finish_edit_frame(frame, &ctx);
                },
            );
            output.textures_delta.clear();
        }
        rect
    };
    let plain = size(&mut e, 320.);
    e.view.ruler = super::crop_tool::Ruler::Armed;
    let _ = e.document.straighten.start();
    e.view.aspect = 0.8;
    e.view.crop_guides.guide = super::crop_tool::Guide::GoldenSpiral;
    let busy = size(&mut e, 320.);
    assert_eq!(plain.size(), busy.size());
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
    let id = l.session.photos[0].id;
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.document.catalog_photo = Some(id);
    editor.document.path = Some(photo);
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
    let history = library.session.catalog.load_history(id)?.unwrap();
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
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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

/// The Guided Upright tool on the photo: drawing a guide, drawing a second, moving an
/// end and deleting a guide are one History step each, and two guides solve to a
/// correction (docs/transform.md#guided-upright).
#[test]
fn guided_upright_gestures_are_one_history_step_each() {
    use crate::model::transform::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    // The other modes analysed already, so the guides are solved at once.
    e.document.edit.setup_mut().upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 5];
    e.view.tool = state::Tool::Guided;
    let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(600., 400.));
    let mut frame = |e: &mut Editor, events: Vec<egui::Event>| {
        let edit = e.begin_edit_frame();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(area),
                events,
                ..Default::default()
            },
            |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(area.size(), egui::Sense::click_and_drag());
                e.guided_overlay(ui, &response, rect, rect);
            },
        );
        output.textures_delta.clear();
        e.finish_edit_frame(edit, &ctx);
    };
    let button = |p: Pos2, pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let drag = |e: &mut Editor,
                frame: &mut dyn FnMut(&mut Editor, Vec<egui::Event>),
                from: Pos2,
                to: Pos2| {
        frame(e, vec![egui::Event::PointerMoved(from), button(from, true)]);
        for k in 1..=4 {
            frame(
                e,
                vec![egui::Event::PointerMoved(
                    from + (to - from) * k as f32 / 4.,
                )],
            );
        }
        frame(e, vec![button(to, false)]);
        frame(e, vec![]);
    };
    frame(&mut e, vec![]);
    // Two converging verticals.
    drag(
        &mut e,
        &mut frame,
        Pos2::new(120., 40.),
        Pos2::new(100., 360.),
    );
    let guides = &e.document.edit.recipe().upright.guides;
    assert_eq!(guides.len(), 1);
    assert!(
        (guides[0].a[0] - 0.2).abs() < 1e-3 && (guides[0].a[1] - 0.1).abs() < 1e-3,
        "{guides:?}"
    );
    assert_eq!(e.document.edit.recipe().upright.mode, UprightMode::Guided);
    drag(
        &mut e,
        &mut frame,
        Pos2::new(480., 40.),
        Pos2::new(500., 360.),
    );
    assert_eq!(e.document.edit.recipe().upright.guides.len(), 2);
    let solved = e
        .document
        .edit
        .recipe()
        .upright
        .correction()
        .expect("a correction");
    assert!(solved[2][1].abs() > 1e-3, "{solved:?}");
    // The first guide's lower end, where it shows now the photo is corrected.
    let g = develop::Geometry::new(e.document.full().unwrap(), e.document.edit.recipe(), 0);
    let [u, v] = g.from_upright_frame(e.document.edit.recipe().upright.guides[0].b);
    let end = Pos2::new(u * 600., v * 400.);
    drag(&mut e, &mut frame, end, end + Vec2::new(-10., 0.));
    assert_eq!(e.document.edit.recipe().upright.guides.len(), 2);
    // Selected by the drag, and deleted.
    let edit = e.begin_edit_frame();
    e.delete_guide();
    e.finish_edit_frame(edit, &ctx);
    assert_eq!(e.document.edit.recipe().upright.guides.len(), 1);
    let (steps, applied) = e.document.edit.history().steps();
    let names: Vec<&str> = steps.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["Add Guide", "Add Guide", "Move Guide", "Delete Guide"]
    );
    assert_eq!(applied, 4);
    // One guide corrects nothing, and says so.
    assert_eq!(e.document.edit.recipe().upright.correction(), None);
    assert!(
        e.messages.line().contains("two or more guides"),
        "{}",
        e.messages.line()
    );
    e.undo();
    assert_eq!(e.document.edit.recipe().upright.guides.len(), 2);
    assert!(e.document.edit.recipe().upright.correction().is_some());
    // Leaving Guided by any route closes the tool, so a drag can't switch it back.
    e.view.tool = state::Tool::Guided;
    let edit = e.begin_edit_frame();
    e.document.edit.setup_mut().upright = Default::default();
    e.finish_edit_frame(edit, &ctx);
    assert!(!e.view.is(state::Tool::Guided));
}

/// A small photo of colors spread along blue, with its metadata; decoded unless
/// `decoded` is false.
fn editor_with_blue_photo(
    ctx: &egui::Context,
    session: crate::app::session::Session,
    decoded: bool,
) -> (Editor, Arc<CameraImage>) {
    let mut editor = Editor::with_context(ctx, None, session, None);
    editor.module = Module::Develop;
    let (width, height) = (32u32, 24u32);
    let metadata = Metadata {
        width,
        height,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    editor.document.metadata = Some(metadata.clone());
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: (0..width * height)
            .map(|i| {
                let v = 0.05 + 0.3 * (i % width) as f32 / width as f32;
                [v, v, 2. * v]
            })
            .collect(),
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    if decoded {
        editor.document.set_image(image.clone());
    }
    (editor, image)
}
/// Runs `action` in an edit frame, as the panels and shortcuts do.
pub(crate) fn in_edit_frame(
    ctx: &egui::Context,
    editor: &mut Editor,
    action: impl FnOnce(&mut Editor),
) {
    let frame = editor.begin_edit_frame();
    action(editor);
    editor.finish_edit_frame(frame, ctx);
}
#[test]
fn v_converts_to_black_and_white_with_the_auto_mix_as_one_step() {
    let ctx = egui::Context::default();
    let (mut editor, _) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), true);
    let before = editor.document.edit.recipe().clone();
    // V, through a whole frame of the Develop module.
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events: vec![egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| editor.draw(ui),
    );
    output.textures_delta.clear();
    // Held down, V repeats; the repeats change nothing.
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events: vec![egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| editor.draw(ui),
    );
    output.textures_delta.clear();
    let auto = editor
        .photo_colors()
        .unwrap()
        .auto_mix()
        .for_recipe(editor.document.edit.recipe());
    let r = editor.document.edit.recipe();
    assert!(r.effects.monochrome);
    assert_ne!(auto, [0.; 8]);
    assert_eq!(r.effects.gray_mix, auto);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(
        (applied, steps[0].name.as_str()),
        (1, "Convert to Black & White")
    );
    // Back to color keeps the mix for the next conversion, as Lightroom does.
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(!editor.document.edit.recipe().effects.monochrome);
    assert_eq!(editor.document.edit.recipe().effects.gray_mix, auto);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!((applied, steps[1].name.as_str()), (2, "Convert to Color"));
    editor.undo();
    editor.undo();
    assert_eq!(*editor.document.edit.recipe(), before);

    // The B&W panel's Auto brings back the Auto mix after a slider moved, as one step.
    editor.redo();
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.setup_mut().effects.gray_mix[3] = 0.6
    });
    in_edit_frame(&ctx, &mut editor, Editor::auto_black_white_mix);
    assert_eq!(editor.document.edit.recipe().effects.gray_mix, auto);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 3);
    assert_eq!(
        (steps[2].name.as_str(), steps[2].value.as_str()),
        ("Black & White Mix", "Auto")
    );
    // With the preference off, the first conversion keeps the mix at zero.
    let session = crate::app::session::Session {
        no_auto_black_white_mix: true,
        ..Default::default()
    };
    let (mut editor, _) = editor_with_blue_photo(&ctx, session, true);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(editor.document.edit.recipe().effects.monochrome);
    assert_eq!(editor.document.edit.recipe().effects.gray_mix, [0.; 8]);
}
#[test]
fn converting_while_the_photo_decodes_waits_for_its_auto_mix() {
    let ctx = egui::Context::default();
    // V twice while decoding: the second cancels the first.
    let (mut editor, image) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), false);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, Editor::finish_pending_treatment);
    assert!(!editor.document.edit.recipe().effects.monochrome);
    assert_eq!(editor.document.edit.history().steps().1, 0);
    // Once: the conversion waits for the photo.
    let (mut editor, image) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), false);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(!editor.document.edit.recipe().effects.monochrome);
    assert_eq!(editor.document.edit.history().steps().1, 0);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, Editor::finish_pending_treatment);
    let r = editor.document.edit.recipe();
    assert!(r.effects.monochrome);
    assert_ne!(r.effects.gray_mix, [0.; 8]);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(
        (applied, steps[0].name.as_str()),
        (1, "Convert to Black & White")
    );
    // An edit in the frame the photo decodes in drops the request too.
    let (mut editor, image) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), false);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.setup_mut().exposure = 0.3
    });
    assert!(!editor.document.edit.recipe().effects.monochrome);
    in_edit_frame(&ctx, &mut editor, |_| {});
    assert!(!editor.document.edit.recipe().effects.monochrome);
    // A request lapses when the recipe changes otherwise before the photo decodes.
    let (mut editor, image) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), false);
    let before_exposure = editor.document.edit.recipe().exposure;
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.edit.setup_mut().exposure = 0.5
    });
    // Undone again before it decodes: the request still lapsed.
    in_edit_frame(&ctx, &mut editor, Editor::undo);
    assert_eq!(editor.document.edit.recipe().exposure, before_exposure);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, Editor::finish_pending_treatment);
    assert!(!editor.document.edit.recipe().effects.monochrome);
}

/// A Lightroom preset from its settings, as a file would hold them.
fn preset_from(name: &str, settings: &str) -> crate::xmp::Preset {
    let text = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal" crs:HasSettings="True" {settings}><crs:Name><rdf:Alt><rdf:li xml:lang="x-default">{name}</rdf:li></rdf:Alt></crs:Name></rdf:Description></rdf:RDF></x:xmpmeta>"#
    );
    crate::xmp::parse(std::path::Path::new(&format!("{name}.xmp")), &text).unwrap()
}
fn editor_with_presets(ctx: &egui::Context, presets: Vec<crate::xmp::Preset>) -> Editor {
    let mut editor = Editor::with_context(ctx, None, crate::app::session::Session::default(), None);
    editor.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    editor.presets.library = Arc::new(crate::presets::Library {
        presets,
        errors: Vec::new(),
    });
    editor
}

#[test]
fn preset_amount_scales_the_preset_from_the_settings_before_it() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_presets(
        &ctx,
        vec![
            preset_from(
                "Bright",
                r#"crs:SupportsAmount="True" crs:Exposure2012="+1.00" crs:Contrast2012="+40""#,
            ),
            preset_from(
                "Fixed",
                r#"crs:SupportsAmount="False" crs:Exposure2012="+1.00""#,
            ),
        ],
    );
    editor.document.edit.setup_mut().exposure = 0.2;
    let frame = editor.begin_edit_frame();
    editor.apply_preset(0).unwrap();
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(editor.document.edit.recipe().exposure, 1.);
    // Each drag is one History step, computed again from the settings before the
    // preset, so going back and forth never drifts.
    for amount in [1.7, 0.3, 0.5] {
        let frame = editor.begin_edit_frame();
        editor.set_preset_amount(amount);
        editor.finish_edit_frame(frame, &ctx);
    }
    assert!((editor.document.edit.recipe().exposure - 0.6).abs() < 1e-6);
    assert!((editor.document.edit.recipe().contrast - 0.2).abs() < 1e-6);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 4);
    assert_eq!(steps[3].name, "Preset Amount");
    assert_eq!(steps[3].value, "50");
    // An Upright analysis landing meanwhile keeps the Amount, and the Amount keeps it.
    let analysed = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 2];
    editor.document.edit.setup_mut().upright.corrections = analysed.clone();
    let frame = editor.begin_edit_frame();
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_some());
    let frame = editor.begin_edit_frame();
    editor.set_preset_amount(0.5);
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(editor.document.edit.recipe().upright.corrections, analysed);
    // A new Upright mode is an edit of its own.
    let frame = editor.begin_edit_frame();
    editor.document.edit.setup_mut().upright.mode = crate::model::transform::UprightMode::Level;
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
    editor.apply_preset(0).unwrap();
    // Any other edit ends it, as Lightroom hides the slider, even with the Presets
    // panel closed.
    let frame = editor.begin_edit_frame();
    editor.document.edit.setup_mut().vibrance = 0.1;
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
    // A preset without an Amount shows none.
    editor.apply_preset(1).unwrap();
    assert!(editor.presets.amount.is_none());
    editor.apply_preset(0).unwrap();
    assert!(editor.presets.amount.is_some());
    // A preset with only choices that aren't numbers looks the same at every Amount
    // above 0: moving it is no step, and leaves no name for the next one.
    editor.presets.library = Arc::new(crate::presets::Library {
        presets: vec![preset_from(
            "Mono",
            r#"crs:SupportsAmount="True" crs:ConvertToGrayscale="True""#,
        )],
        errors: Vec::new(),
    });
    let frame = editor.begin_edit_frame();
    editor.apply_preset(0).unwrap();
    editor.finish_edit_frame(frame, &ctx);
    let frame = editor.begin_edit_frame();
    // As the slider does while it moves.
    ctx.data_mut(|d| {
        d.insert_temp(
            super::widgets::history_step_id(),
            ("Preset Amount".to_string(), "50".to_string()),
        )
    });
    editor.set_preset_amount(0.5);
    editor.finish_edit_frame(frame, &ctx);
    let frame = editor.begin_edit_frame();
    editor.document.edit.setup_mut().exposure = 0.9;
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
    editor.apply_preset(0).unwrap();
    let (steps, applied) = editor.document.edit.history().steps();
    assert_ne!(steps[applied - 1].name, "Preset Amount");
    // Undo ends it too.
    let frame = editor.begin_edit_frame();
    let mut recipe = editor.document.edit.recipe().clone();
    editor.document.edit.history_mut().undo(&mut recipe);
    editor.document.edit.replace(recipe);
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
    // An edit earlier in the frame the slider moves in (a mask's name kept as the
    // slider was clicked) ends it there, rather than being taken back by it.
    editor.apply_preset(0).unwrap();
    let frame = editor.begin_edit_frame();
    editor.document.edit.recipe_mut().vibrance = 0.3;
    assert_eq!(
        editor.set_preset_amount(0.5),
        super::presets::AmountChange::Same
    );
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(editor.document.edit.recipe().vibrance, 0.3);
    assert!(editor.presets.amount.is_none());
}
#[test]
fn red_eye_brackets_resize_the_circle_a_click_uses() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.view.tool = state::Tool::RedEye;
    let start = editor.view.red_eye.size;
    let mut press = |key| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |_| ctx.input(|i| editor.red_eye_keys(i)),
        );
        output.textures_delta.clear();
        editor.view.red_eye.size
    };
    let larger = press(egui::Key::CloseBracket);
    assert!(larger > start, "{larger} after {start}");
    let smaller = press(egui::Key::OpenBracket);
    assert!(
        (smaller - start).abs() < 1e-6,
        "{smaller} back from {start}"
    );
    // It stays within sizes a pupil can have.
    for _ in 0..100 {
        press(egui::Key::CloseBracket);
    }
    assert!(editor.view.red_eye.size <= 0.25);
}
#[test]
fn scrolling_over_the_photo_resizes_the_brush_spot_and_red_eye_circle() {
    use super::brush_scroll::{Adjust, MaskBrush, Scroll};
    use crate::model::masks::{MaskComponent, MaskGroup, MaskShape};
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let size = |lines| Scroll {
        lines,
        adjust: Adjust::Size,
        brush: MaskBrush::Current,
    };
    editor.view.tool = state::Tool::Remove;
    let spot = editor.view.retouch.size;
    editor.scroll_tool_size(size(1.));
    assert!(editor.view.retouch.size > spot);
    editor.view.tool = state::Tool::RedEye;
    let eye = editor.view.red_eye.size;
    editor.scroll_tool_size(size(-1.));
    assert!(editor.view.red_eye.size < eye);
    // The mask brush changes only while a brush is in use, as the cursor shows it.
    editor.view.tool = state::Tool::Mask;
    let brush = editor.view.masking.brushes[0];
    editor.scroll_tool_size(size(1.));
    assert_eq!(editor.view.masking.brushes[0], brush);
    editor.document.edit.setup_mut().masks = vec![MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Brush {
            strokes: Vec::new(),
        })],
        ..Default::default()
    }];
    editor.view.masking.selected = Some(0);
    editor.view.masking.component = Some(0);
    editor.scroll_tool_size(size(1.));
    assert!(editor.view.masking.brushes[0].size > brush.size);
    // Shift-scroll changes the feather instead, and with Option/Alt the Erase brush.
    editor.scroll_tool_size(Scroll {
        lines: -1.,
        adjust: Adjust::Feather,
        brush: MaskBrush::Current,
    });
    assert!(editor.view.masking.brushes[0].feather < brush.feather);
    let erase = editor.view.masking.brushes[2];
    editor.scroll_tool_size(Scroll {
        lines: 1.,
        adjust: Adjust::Size,
        brush: MaskBrush::Erase,
    });
    assert!(editor.view.masking.brushes[2].size > erase.size);
    // Other tools ignore it.
    editor.view.tool = state::Tool::Crop;
    let before = (editor.view.retouch.size, editor.view.red_eye.size);
    editor.scroll_tool_size(size(1.));
    assert_eq!((editor.view.retouch.size, editor.view.red_eye.size), before);
}
#[test]
fn wheel_events_carry_their_own_modifiers_and_plain_swipes_sideways_do_nothing() {
    use super::brush_scroll::{Adjust, MaskBrush, Scroll};
    let ctx = egui::Context::default();
    let wheel = |delta: Vec2, modifiers| egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta,
        phase: egui::TouchPhase::Move,
        modifiers,
    };
    let read = |events| {
        let mut scrolls = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| scrolls = ui.input(Scroll::read),
        );
        output.textures_delta.clear();
        scrolls
    };
    // Shift held for the wheel event, even if it's let go by the frame: feather.
    let s = read(vec![wheel(Vec2::new(0., 1.), egui::Modifiers::SHIFT)]);
    assert_eq!((s[0].lines, s[0].adjust), (1., Adjust::Feather));
    // macOS turns Shift+wheel into a sideways scroll.
    let s = read(vec![wheel(Vec2::new(-2., 0.), egui::Modifiers::SHIFT)]);
    assert_eq!((s[0].lines, s[0].adjust), (-2., Adjust::Feather));
    // A plain sideways trackpad swipe sizes nothing.
    assert!(read(vec![wheel(Vec2::new(3., 0.), egui::Modifiers::NONE)]).is_empty());
    // Option/Alt picks the Erase brush.
    let s = read(vec![wheel(Vec2::new(0., -1.), egui::Modifiers::ALT)]);
    assert_eq!((s[0].adjust, s[0].brush), (Adjust::Size, MaskBrush::Erase));
    // A frame that also has a click or a key leaves the wheel alone.
    let click = egui::Event::PointerButton {
        pos: Pos2::new(5., 5.),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    };
    let key = egui::Event::Key {
        key: egui::Key::Num3,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let up = wheel(Vec2::new(0., 1.), egui::Modifiers::NONE);
    assert!(read(vec![up.clone(), click]).is_empty());
    assert!(read(vec![up, key]).is_empty());
}
#[test]
fn a_wheel_scroll_resizing_a_spot_is_one_history_step() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.view.tool = state::Tool::Remove;
    editor
        .document
        .edit
        .setup_mut()
        .retouch
        .push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    editor.view.retouch.selected = Some(0);
    let radius = editor.document.edit.recipe().retouch[0].radius();
    let notch = super::brush_scroll::Scroll {
        lines: 1.,
        adjust: super::brush_scroll::Adjust::Size,
        brush: super::brush_scroll::MaskBrush::Current,
    };
    for _ in 0..5 {
        let edit = editor.begin_edit_frame();
        editor.scroll_tool_size(notch);
        editor.finish_edit_frame(edit, &ctx);
    }
    assert!(editor.document.edit.recipe().retouch[0].radius() > radius);
    assert!(editor.document.edit.history().in_gesture());
    // Once the scroll pauses, the five notches are one step.
    std::thread::sleep(std::time::Duration::from_millis(450));
    let edit = editor.begin_edit_frame();
    editor.finish_edit_frame(edit, &ctx);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(
        applied,
        1,
        "{:?}",
        steps.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}
#[test]
fn brackets_size_the_red_eye_circle_without_rating_the_photo() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("image.ARW"), b"rating fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let l = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let id = l.session.photos[0].id;
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.module = Module::Develop;
    editor.document.catalog_photo = Some(id);
    editor.view.tool = state::Tool::RedEye;
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::CloseBracket,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| editor.metadata_shortcuts(ui.ctx()),
    );
    output.textures_delta.clear();
    let library = editor.library.as_ref().unwrap();
    assert_eq!(
        library
            .session
            .photos
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .rating,
        0
    );
    Ok(())
}
#[test]
fn an_edit_right_after_a_wheel_scroll_is_its_own_history_step() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.view.tool = state::Tool::Remove;
    editor
        .document
        .edit
        .setup_mut()
        .retouch
        .push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    editor.view.retouch.selected = Some(0);
    let edit = editor.begin_edit_frame();
    editor.scroll_tool_size(super::brush_scroll::Scroll {
        lines: 1.,
        adjust: super::brush_scroll::Adjust::Size,
        brush: super::brush_scroll::MaskBrush::Current,
    });
    editor.finish_edit_frame(edit, &ctx);
    // At once, before the scroll pauses, a slider moves, naming its step.
    let edit = editor.begin_edit_frame();
    editor.document.edit.setup_mut().exposure = 0.5;
    ctx.data_mut(|d| {
        d.insert_temp(
            super::widgets::history_step_id(),
            ("Exposure".to_string(), "+0.50".to_string()),
        )
    });
    editor.finish_edit_frame(edit, &ctx);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 2);
    assert_ne!(steps[0].name, "Exposure");
    assert_eq!(steps[1].name, "Exposure");
}
#[test]
fn a_wheel_scroll_is_its_own_step_however_late_the_next_frame_comes() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.view.tool = state::Tool::Remove;
    editor
        .document
        .edit
        .setup_mut()
        .retouch
        .push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    editor.view.retouch.selected = Some(0);
    let notch = super::brush_scroll::Scroll {
        lines: 1.,
        adjust: super::brush_scroll::Adjust::Size,
        brush: super::brush_scroll::MaskBrush::Current,
    };
    let edit = editor.begin_edit_frame();
    editor.scroll_tool_size(notch);
    editor.finish_edit_frame(edit, &ctx);
    // The pause passes with no frame, then the next frame brings a slider change.
    std::thread::sleep(std::time::Duration::from_millis(450));
    let edit = editor.begin_edit_frame();
    editor.document.edit.setup_mut().exposure = 0.5;
    ctx.data_mut(|d| {
        d.insert_temp(
            super::widgets::history_step_id(),
            ("Exposure".to_string(), "+0.50".to_string()),
        )
    });
    editor.finish_edit_frame(edit, &ctx);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 2);
    assert_eq!(steps[1].name, "Exposure");
    // A rating or flag right after a scroll is logged after it, so Undo takes it first.
    let edit = editor.begin_edit_frame();
    editor.scroll_tool_size(notch);
    editor.finish_edit_frame(edit, &ctx);
    editor.sync_undo();
    let logged = editor.undo_log.len().0;
    editor.finish_wheel_gesture();
    assert_eq!(editor.undo_log.len().0, logged + 1);
    assert!(!editor.document.edit.history().in_gesture());
}
#[test]
fn a_wheel_scroll_closes_once_paused_even_while_a_button_goes_down() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.view.tool = state::Tool::Remove;
    editor
        .document
        .edit
        .setup_mut()
        .retouch
        .push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    editor.view.retouch.selected = Some(0);
    let edit = editor.begin_edit_frame();
    editor.scroll_tool_size(super::brush_scroll::Scroll {
        lines: 1.,
        adjust: super::brush_scroll::Adjust::Size,
        brush: super::brush_scroll::MaskBrush::Current,
    });
    editor.finish_edit_frame(edit, &ctx);
    std::thread::sleep(std::time::Duration::from_millis(450));
    // The next frame has the button going down over the photo; nothing changes yet.
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: Pos2::new(5., 5.),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |_| {},
    );
    output.textures_delta.clear();
    let edit = editor.begin_edit_frame();
    editor.finish_edit_frame(edit, &ctx);
    assert!(!editor.document.edit.history().in_gesture());
    assert_eq!(editor.document.edit.history().steps().1, 1);
}
#[test]
fn a_click_after_a_wheel_scroll_closes_it_at_once() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.view.tool = state::Tool::Remove;
    editor
        .document
        .edit
        .setup_mut()
        .retouch
        .push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    editor.view.retouch.selected = Some(0);
    let edit = editor.begin_edit_frame();
    editor.scroll_tool_size(super::brush_scroll::Scroll {
        lines: 1.,
        adjust: super::brush_scroll::Adjust::Size,
        brush: super::brush_scroll::MaskBrush::Current,
    });
    editor.finish_edit_frame(edit, &ctx);
    // A click before the pause (on another spot, say) closes the scroll.
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: Pos2::new(5., 5.),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |_| {},
    );
    output.textures_delta.clear();
    let edit = editor.begin_edit_frame();
    editor.finish_edit_frame(edit, &ctx);
    assert!(!editor.document.edit.history().in_gesture());
    assert_eq!(editor.document.edit.history().steps().1, 1);
}
#[test]
fn a_quick_cmd_a_in_remove_does_not_toggle_visualize_spots() {
    let ctx = egui::Context::default();
    let (mut editor, _) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), true);
    let mut press = |modifiers: egui::Modifiers| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: Some(egui::Key::A),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
                // Cmd released within the same frame: Select All's press.
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| ui.input(|i| editor.retouch_keys(i)));
        output.textures_delta.clear();
        editor.view.retouch.visualize
    };
    assert!(!press(egui::Modifiers::COMMAND));
    assert!(press(egui::Modifiers::NONE));
}
#[test]
fn a_swatch_added_while_visualize_range_is_on_is_visualized_at_once() {
    let ctx = egui::Context::default();
    let (mut editor, _) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), true);
    editor.view.mixer_tab = state::MixerTab::PointColor;
    editor.view.point_color.visualize = true;
    editor.view.toggle(state::Tool::PointColor);
    let sampled = editor.document.edit.recipe().clone();
    editor.point_color_sample_ready(&sampled, Ok([2., 0.6, 0.3]));
    assert_eq!(editor.view.point_color.selected, Some(0));
    // The render scheduled with the swatch shows its range, with no other change.
    let pending = editor.preview.pending.recipe.as_ref().unwrap();
    assert_eq!(
        pending.point_colors[0].view,
        crate::model::point_color::SwatchView::VisualizeRange
    );
}
#[test]
fn point_colors_dropper_adds_a_selected_swatch_as_one_step_and_visualizes_it() {
    use crate::develop::point_color::SampleRefusal;
    let ctx = egui::Context::default();
    let (mut editor, _) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), true);
    editor.view.mixer_tab = state::MixerTab::PointColor;
    editor.view.toggle(state::Tool::PointColor);
    assert!(editor.view.picks_color());
    // Sampled off the UI thread; a second click while it runs is ignored.
    let sample = |editor: &mut Editor| {
        editor.start_point_color_sample(0.7, 0.5);
        assert!(editor.document.point_color_pick.is_running());
        editor.start_point_color_sample(0.1, 0.5);
        let start = std::time::Instant::now();
        while editor.document.point_color_pick.is_running() {
            assert!(start.elapsed().as_secs() < 60, "sampling did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
            editor.events(&ctx);
        }
    };
    sample(&mut editor);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 1);
    assert_eq!(
        (steps[0].name.as_str(), steps[0].value.as_str()),
        ("Point Color", "Add Swatch")
    );
    let swatch = editor.document.edit.recipe().point_colors[0];
    // The photo is blue: a hue near 4 sixths of a turn, sampled with default ranges.
    assert!((swatch.source[0] - 4.).abs() < 0.5, "{swatch:?}");
    assert!(swatch.is_valid());
    assert_eq!(editor.view.point_color.selected, Some(0));
    assert_eq!(editor.view.tool, state::Tool::None);
    // The same color again is refused, and changes nothing.
    editor.view.toggle(state::Tool::PointColor);
    sample(&mut editor);
    assert_eq!(
        editor.messages.line(),
        SampleRefusal::AlreadySampled.message()
    );
    assert_eq!(editor.document.edit.recipe().point_colors.len(), 1);
    // A sample still being taken when the dropper is put away is dropped.
    assert_eq!(editor.view.tool, state::Tool::PointColor);
    in_edit_frame(&ctx, &mut editor, |e| {
        e.start_point_color_sample(0.2, 0.5);
        e.view.toggle(state::Tool::PointColor);
    });
    assert!(!editor.document.point_color_pick.is_running());
    std::thread::sleep(std::time::Duration::from_millis(200));
    editor.events(&ctx);
    assert_eq!(editor.document.edit.recipe().point_colors.len(), 1);
    editor.view.toggle(state::Tool::PointColor);
    // Nor when the Library or Before opens meanwhile: the sample stops at once.
    for leave in [
        (|e: &mut Editor| e.module = Module::Library) as fn(&mut Editor),
        |e: &mut Editor| e.view.compare = before_after::Compare::BeforeOnly,
    ] {
        in_edit_frame(&ctx, &mut editor, |e| e.start_point_color_sample(0.2, 0.5));
        leave(&mut editor);
        editor.events(&ctx);
        assert!(!editor.document.point_color_pick.is_running());
        std::thread::sleep(std::time::Duration::from_millis(200));
        editor.events(&ctx);
        assert_eq!(editor.document.edit.recipe().point_colors.len(), 1);
        editor.module = Module::Develop;
        editor.view.compare = before_after::Compare::Off;
        editor.view.tool = state::Tool::PointColor;
    }
    // A sample of a photo edited meanwhile is dropped.
    let mut changed = editor.document.edit.recipe().clone();
    changed.exposure = 1.;
    editor.point_color_sample_ready(&changed, Ok([2., 0.6, 0.3]));
    assert_eq!(editor.document.edit.recipe().point_colors.len(), 1);
    editor.view.tool = state::Tool::None;
    editor.view.mixer_tab = state::MixerTab::Mixer;
    // Visualize Range shows the selected swatch while the tab is open, on a color photo.
    assert_eq!(editor.visualized_swatch(), None);
    editor.view.mixer_tab = state::MixerTab::PointColor;
    editor.view.point_color.visualize = true;
    editor.view.point_color.ranges = true;
    assert_eq!(editor.visualized_swatch(), Some(0));
    // The whole panel draws, ranges open.
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 1600.))),
            ..Default::default()
        },
        |ui| editor.draw(ui),
    );
    output.textures_delta.clear();
    editor.document.edit.setup_mut().effects.monochrome = true;
    assert_eq!(editor.visualized_swatch(), None);
    editor.document.edit.setup_mut().effects.monochrome = false;
    // The preview's identity includes it, so a picker never takes it for the photo.
    editor.schedule();
    let pending = editor.preview.pending.recipe.as_ref().unwrap();
    assert_eq!(
        pending.point_colors[0].view,
        crate::model::point_color::SwatchView::VisualizeRange
    );
    assert_ne!(Some(pending), Some(&editor.effective_recipe()));
    // Not in Before, which shows the photo's defaults.
    editor.view.compare = before_after::Compare::BeforeOnly;
    assert_eq!(editor.visualized_swatch(), None);
    editor.view.compare = before_after::Compare::Off;
    // Not while an eyedropper is out, which samples the photo as it renders.
    editor.view.toggle(state::Tool::Defringe);
    assert_eq!(editor.visualized_swatch(), None);
    assert_eq!(editor.view.loupe_prompt(), "Pick a purple or green fringe");
    editor.view.toggle(state::Tool::PointColor);
    assert_eq!(editor.visualized_swatch(), None);
    assert_eq!(editor.view.loupe_prompt(), "Pick a color to adjust");
    // The dropper goes with the tab: on the Mixer tab a click adds no hidden swatch.
    in_edit_frame(&ctx, &mut editor, |e| {
        e.view.mixer_tab = state::MixerTab::Mixer
    });
    assert_eq!(editor.view.tool, state::Tool::None);
    editor.view.mixer_tab = state::MixerTab::PointColor;
    // Nor in the Library.
    editor.module = Module::Library;
    assert_eq!(editor.visualized_swatch(), None);
    editor.module = Module::Develop;
    // One History step, which Undo takes back.
    let mut recipe = editor.document.edit.recipe().clone();
    assert!(editor.document.edit.history_mut().undo(&mut recipe));
    assert!(recipe.point_colors.is_empty());
    assert_eq!(editor.visualized_swatch(), Some(0));
}

/// Runs one frame of `add` in a 360-point-wide window at `time`.
fn widget_frame(
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    add: impl FnMut(&mut egui::Ui),
) {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(360., 600.))),
            time: Some(time),
            events,
            ..Default::default()
        },
        add,
    );
    output.textures_delta.clear();
}
fn click_at(at: Pos2, button: egui::PointerButton) -> Vec<egui::Event> {
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(at), press(true), press(false)]
}
#[test]
fn a_panel_header_switch_turns_the_panel_off_and_on_without_opening_it() {
    use crate::model::panels::PanelState;
    let ctx = egui::Context::default();
    let mut state = PanelState::On;
    let area = std::cell::Cell::new(Rect::NOTHING);
    let draw = |state: &mut PanelState, events, time| {
        widget_frame(&ctx, time, events, |ui| {
            area.set(ui.max_rect());
            super::widgets::switched_section(ui, "Tone Curve", state, |ui| {
                ui.label("contents");
            });
        })
    };
    draw(&mut state, vec![], 0.);
    // The switch sits left of the reset button, which sits left of the
    // chevron, in the 28-point header 14 points down.
    let area = area.get();
    let switch = Pos2::new(area.right() - 26. - 26. - 18., area.top() + 14. + 14.);
    draw(
        &mut state,
        click_at(switch, egui::PointerButton::Primary),
        1.,
    );
    assert_eq!(state, PanelState::Off);
    draw(
        &mut state,
        click_at(switch, egui::PointerButton::Primary),
        2.,
    );
    assert_eq!(state, PanelState::On);
    let collapsed: Option<std::collections::BTreeSet<String>> =
        ctx.data(|d| d.get_temp(super::widgets::collapsed_sections_id()));
    assert!(collapsed.is_none_or(|set| set.is_empty()));
}
#[test]
fn changing_a_setting_in_a_panel_that_is_off_turns_it_back_on() {
    use crate::model::panels::{Panel, PanelState};
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
#[test]
fn solo_mode_opens_one_panel_per_side_and_is_set_from_the_header_menu() {
    use super::widgets::{SectionGroup, SectionSide};
    let ctx = egui::Context::default();
    let titles = ["Basic", "Tone Curve", "Detail"];
    let headers = std::cell::RefCell::new(Vec::new());
    let draw = |events, time| {
        widget_frame(&ctx, time, events, |ui| {
            let _side = SectionSide::enter(ui, SectionGroup::DevelopRight);
            headers.borrow_mut().clear();
            for title in titles {
                headers.borrow_mut().push(ui.cursor().top() + 8. + 14.);
                super::widgets::section(ui, title, false, |ui| {
                    ui.label(title);
                });
            }
        })
    };
    let collapsed = || -> std::collections::BTreeSet<String> {
        ctx.data(|d| d.get_temp(super::widgets::collapsed_sections_id()))
            .unwrap_or_default()
    };
    draw(vec![], 0.);
    // Right-click Tone Curve's header, then Solo Mode in its menu.
    let header = Pos2::new(60., headers.borrow()[1]);
    draw(click_at(header, egui::PointerButton::Secondary), 1.);
    draw(vec![], 1.1);
    let item = Pos2::new(header.x + 30., header.y + 14.);
    draw(click_at(item, egui::PointerButton::Primary), 2.);
    let solo: Option<std::collections::BTreeSet<String>> =
        ctx.data(|d| d.get_temp(super::widgets::solo_sections_id()));
    assert_eq!(solo, Some(["develop-right".to_string()].into()));
    assert_eq!(
        collapsed(),
        ["Basic".to_string(), "Detail".to_string()].into()
    );
    // Opening Detail closes Tone Curve.
    draw(vec![], 3.);
    let detail = Pos2::new(60., headers.borrow()[2]);
    draw(click_at(detail, egui::PointerButton::Primary), 4.);
    assert_eq!(
        collapsed(),
        ["Basic".to_string(), "Tone Curve".to_string()].into()
    );
}
#[test]
fn the_panel_header_menu_fits_its_item_not_the_header() {
    use super::widgets::{SectionGroup, SectionSide};
    let ctx = egui::Context::default();
    let header = std::cell::Cell::new(0.);
    let draw = |events, time| {
        widget_frame(&ctx, time, events, |ui| {
            let _side = SectionSide::enter(ui, SectionGroup::DevelopRight);
            header.set(ui.cursor().top() + 14. + 14.);
            super::widgets::section(ui, "Basic", false, |ui| {
                ui.label("Basic");
            });
        })
    };
    draw(vec![], 0.);
    draw(
        click_at(Pos2::new(60., header.get()), egui::PointerButton::Secondary),
        1.,
    );
    draw(vec![], 1.1);
    let menus: Vec<Rect> = ctx.memory(|m| {
        m.areas()
            .visible_layer_ids()
            .into_iter()
            .filter(|layer| layer.order == egui::Order::Foreground)
            .filter_map(|layer| m.area_rect(layer.id))
            .collect()
    });
    assert_eq!(menus.len(), 1, "{menus:?}");
    // The header spans the 360 pt panel; Solo Mode needs far less.
    assert!(menus[0].width() < 200., "{menus:?}");
}
#[test]
fn up_and_down_nudge_the_hovered_slider_but_never_while_typing_or_scrolling() {
    let ctx = egui::Context::default();
    let mut value = 0.;
    let mut text = String::new();
    let row = std::cell::Cell::new(Rect::NOTHING);
    let mut draw = |value: &mut f32, focus: bool, events: Vec<egui::Event>, time| {
        widget_frame(&ctx, time, events, |ui| {
            let top = ui.cursor().min;
            super::widgets::slider(ui, "Contrast", value, -1. ..=1., 0.);
            row.set(Rect::from_min_max(
                top,
                Pos2::new(ui.max_rect().right(), ui.cursor().top()),
            ));
            let field = ui.text_edit_singleline(&mut text);
            if focus {
                field.request_focus();
            }
        })
    };
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    };
    draw(&mut value, false, vec![], 0.);
    let row = row.get();
    let over = egui::Event::PointerMoved(Pos2::new(row.center().x, row.top() + 12.));
    draw(&mut value, false, vec![over], 1.);
    draw(
        &mut value,
        false,
        vec![key(egui::Key::ArrowUp, egui::Modifiers::NONE)],
        2.,
    );
    assert!((value - 0.01).abs() < 1e-6, "{value}");
    draw(
        &mut value,
        false,
        vec![key(egui::Key::ArrowDown, egui::Modifiers::SHIFT)],
        3.,
    );
    assert!((value + 0.09).abs() < 1e-6, "{value}");
    // Scrolling over it never moves it.
    let scroll = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: Vec2::new(0., -3.),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    };
    draw(&mut value, false, vec![scroll], 4.);
    assert!((value + 0.09).abs() < 1e-6, "{value}");
    // While a text field has the keyboard, the keys are the field's.
    draw(&mut value, true, vec![], 5.);
    draw(
        &mut value,
        true,
        vec![key(egui::Key::ArrowUp, egui::Modifiers::NONE)],
        6.,
    );
    assert!((value + 0.09).abs() < 1e-6, "{value}");
}
#[test]
fn a_disabled_slider_ignores_up_and_down() {
    let ctx = egui::Context::default();
    let mut value = 0.;
    let row = std::cell::Cell::new(Rect::NOTHING);
    let draw = |value: &mut f32, events: Vec<egui::Event>, time| {
        widget_frame(&ctx, time, events, |ui| {
            let top = ui.cursor().min;
            ui.add_enabled_ui(false, |ui| {
                super::widgets::slider(ui, "Contrast", value, -1. ..=1., 0.);
            });
            row.set(Rect::from_min_max(
                top,
                Pos2::new(ui.max_rect().right(), ui.cursor().top()),
            ));
        })
    };
    draw(&mut value, vec![], 0.);
    let over = egui::Event::PointerMoved(row.get().center());
    draw(&mut value, vec![over], 1.);
    let up = egui::Event::Key {
        key: egui::Key::ArrowUp,
        physical_key: Some(egui::Key::ArrowUp),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    draw(&mut value, vec![up], 2.);
    assert_eq!(value, 0.);
}
#[test]
fn b_and_w_opens_and_closes_with_the_color_mixer_in_solo_mode() {
    use super::widgets::{SectionGroup, SectionSide};
    let ctx = egui::Context::default();
    ctx.data_mut(|d| {
        d.insert_temp(
            super::widgets::solo_sections_id(),
            std::collections::BTreeSet::from(["develop-right".to_string()]),
        )
    });
    let headers = std::cell::RefCell::new(Vec::new());
    let draw = |mixer: &str, events, time| {
        widget_frame(&ctx, time, events, |ui| {
            let _side = SectionSide::enter(ui, SectionGroup::DevelopRight);
            headers.borrow_mut().clear();
            for title in ["Tone Curve", mixer] {
                headers.borrow_mut().push(ui.cursor().top() + 8. + 14.);
                super::widgets::section(ui, title, false, |ui| {
                    ui.label(title);
                });
            }
        })
    };
    let collapsed = || -> std::collections::BTreeSet<String> {
        ctx.data(|d| d.get_temp(super::widgets::collapsed_sections_id()))
            .unwrap_or_default()
    };
    draw("Color Mixer", vec![], 0.);
    // Open Tone Curve alone, then convert to black & white: B&W stays closed.
    let mixer = Pos2::new(60., headers.borrow()[1]);
    draw(
        "Color Mixer",
        click_at(mixer, egui::PointerButton::Primary),
        1.,
    );
    assert_eq!(collapsed(), ["Color Mixer".to_string()].into());
    draw("B&W", vec![], 3.);
    assert_eq!(collapsed(), ["Color Mixer".to_string()].into());
    // Opening B&W closes Tone Curve, and the Color Mixer comes back open.
    draw("B&W", click_at(mixer, egui::PointerButton::Primary), 4.);
    assert_eq!(collapsed(), ["Tone Curve".to_string()].into());
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
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
            .catalog
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

/// A photo exported in a batch, never opened, has exactly the pixels of the same
/// photo exported from Develop with the same settings: a saved edit with a mask and
/// a spot, a Lightroom-only edit with Auto settings left to compute, the raw
/// defaults, Upright Auto and Guided without stored corrections, a virtual copy, and
/// the open photo with adjustments not yet saved.
#[test]
fn a_batch_export_matches_develops_export_pixel_for_pixel() -> anyhow::Result<()> {
    use crate::export::{
        Replace,
        batch::{self, BatchPhoto, Edit},
    };
    use crate::export_settings::{Destination, Existing, ExportSettings, Format};
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/charts/synthetic-d65.dng");
    let names = ["a.dng", "b.dng", "c.dng", "d.dng", "e.dng", "f.dng"];
    for name in names {
        std::fs::copy(&chart, photos.join(name))?;
    }
    let catalog = dir.path().join("test.rawmakase");
    let mut c = crate::catalog::Catalog::create(&catalog)?;
    c.add_folder(&photos)?;
    let mut ids: Vec<(PhotoId, std::path::PathBuf)> =
        c.photos()?.into_iter().map(|p| (p.id, p.path)).collect();
    let metadata = crate::photo::open(&ids[0].1)?.metadata;
    let (profiles, _) = crate::camera_profiles::installed(&metadata);
    let base = Recipe::with_profiles(&metadata, &profiles);
    let save = |c: &mut crate::catalog::Catalog,
                (id, path): &(PhotoId, std::path::PathBuf),
                r: &Recipe| {
        c.save_edit(
            *id,
            path,
            r,
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )
    };
    // a: a mask and a spot.
    let mut local = base.clone();
    local.exposure = 0.3;
    local.masks.push(crate::model::masks::MaskGroup {
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
    local.retouch = vec![crate::model::retouch::RetouchOp {
        mode: crate::model::retouch::RetouchMode::Heal,
        shape: crate::model::retouch::RetouchShape::Spot {
            center: [0.3, 0.3],
            radius: 0.03,
        },
        feather: 0.4,
        opacity: 1.,
        offset: [0.1, 0.],
    }];
    save(&mut c, &ids[0], &local)?;
    // b: Lightroom's settings only, with Auto Tone and Auto white balance to compute.
    c.db_for_tests().execute(
        "UPDATE photos SET lightroom_develop='s = { AutoTone = true, WhiteBalance = \"Auto\", Contrast2012 = 20 }' WHERE id=?",
        [ids[1].0.0],
    )?;
    // c: nothing. d: Upright Auto, e: Guided, neither analysed.
    let mut auto = base.clone();
    auto.upright.mode = crate::model::transform::UprightMode::Auto;
    save(&mut c, &ids[3], &auto)?;
    let mut guided = base.clone();
    guided.upright.mode = crate::model::transform::UprightMode::Guided;
    guided.upright.guides = vec![
        crate::model::transform::UprightGuide {
            a: [0.2, 0.1],
            b: [0.25, 0.9],
        },
        crate::model::transform::UprightGuide {
            a: [0.8, 0.1],
            b: [0.75, 0.9],
        },
    ];
    save(&mut c, &ids[4], &guided)?;
    // A virtual copy of f, with an edit of its own.
    let copy = c.create_virtual_copy(ids[5].0)?;
    let mut copied = base;
    copied.contrast = 0.4;
    ids.push((copy, ids[5].1.clone()));
    save(&mut c, &ids[6], &copied)?;
    // Taken before any photo is opened: the batch works each edit out itself.
    let records = c.photo_records(&ids.iter().map(|(id, _)| *id).collect::<Vec<_>>())?;
    drop(c);

    let ctx = egui::Context::default();
    let library = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let defaults = Arc::new(crate::raw_defaults::DevelopDefaults::with_presets(
        Default::default(),
        |_| None,
    ));
    editor.raw_defaults = defaults.clone();
    editor.library = Some(Box::new(library));
    let settings = |folder: &str| ExportSettings {
        destination: Destination::Folder,
        folder: Some(dir.path().join(folder)),
        format: Format::Tiff,
        resize: true,
        long_edge: 160,
        existing: Existing::Unique,
        ..Default::default()
    };
    let pixels = |path: &std::path::Path| image::open(path).unwrap().into_rgb16().into_raw();
    let mut rendered = Vec::new();
    for (i, ((id, path), record)) in ids.iter().zip(records).enumerate() {
        editor.open_raw(path.clone(), Some(*id));
        // Open once decoded in full and Upright's analysis is in.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            editor.events(&ctx);
            let open = editor.document.full().is_some_and(|im| !im.fast)
                && !editor.document.upright.is_running()
                && !editor.document.edit.recipe().upright.needs_analysis();
            if open {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "photo {i} not opened");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let last = i == ids.len() - 1;
        if last {
            // Adjusted on screen, not saved.
            editor.document.edit.setup_mut().exposure = -0.4;
        }
        let develop = dir.path().join(format!("develop/{i}.tif"));
        std::fs::create_dir_all(develop.parent().unwrap())?;
        crate::export::job::run(
            editor.export_photo().unwrap(),
            &settings("develop"),
            &develop,
            Replace::NoClobber,
            &Default::default(),
            |_| {},
        )?;
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let mut photo = BatchPhoto::from_record(record, path.clone(), name, Default::default());
        if last {
            photo.edit = Edit::Shown {
                recipe: Box::new(editor.document.edit.recipe().clone()),
                unsaved: true,
                file: Some(crate::storage::Identity::read(path)?),
            };
        }
        let settings = settings(&format!("batch/{i}"));
        let photos = vec![photo];
        let outcomes = batch::run(
            &batch::Batch {
                plan: batch::plan(&photos, &settings, None).unwrap(),
                photos,
                settings,
                defaults: defaults.clone(),
                watermark: None,
                demosaic: Default::default(),
                cache: editor.decode_cache.clone(),
            },
            &Default::default(),
            |_| {},
        );
        let batch::Outcome::Exported { path: exported, .. } = &outcomes[0] else {
            panic!("photo {i}: {:?}", outcomes[0]);
        };
        let developed = pixels(&develop);
        assert!(pixels(exported) == developed, "photo {i} differs");
        rendered.push(developed);
    }
    // Every edit shows: none of them rendered as the unedited photo (c).
    for i in [0, 1, 3, 4, 6] {
        assert!(rendered[i] != rendered[2], "photo {i} rendered unedited");
    }
    Ok(())
}
#[test]
fn only_size_changes_hold_previews_gpu_work() {
    use winit::{dpi::PhysicalSize, event::WindowEvent};
    assert!(reconfigures_surface(&WindowEvent::Resized(
        PhysicalSize::new(1512, 491)
    )));
    assert!(!reconfigures_surface(&WindowEvent::RedrawRequested));
    assert!(!reconfigures_surface(&WindowEvent::Focused(true)));
}
#[cfg(target_os = "linux")]
#[test]
fn the_window_draws_on_the_gpu_the_system_lists_first() {
    use wgpu::{DeviceType::*, PowerPreference};
    // An Optimus laptop (#375): Mesa's device-select layer lists the Intel GPU,
    // which drives the display, before the render-offload-only NVIDIA GPU.
    let optimus = [
        ("Intel(R) Iris(R) Xe Graphics", IntegratedGpu),
        ("NVIDIA GeForce MX450", DiscreteGpu),
        ("llvmpipe (LLVM 20.1.8, 256 bits)", Cpu),
    ];
    assert_eq!(display_adapter(&optimus, None, None), Some(0));
    // prime-run lists the NVIDIA GPU first.
    let offloaded = [optimus[1], optimus[0], optimus[2]];
    assert_eq!(display_adapter(&offloaded, None, None), Some(0));
    // Software rendering only when no GPU can draw the window.
    assert_eq!(
        display_adapter(&[optimus[2], optimus[0]], None, None),
        Some(1)
    );
    assert_eq!(display_adapter(&[optimus[2]], None, None), Some(0));
    assert_eq!(display_adapter(&[], None, None), None);
    // WGPU_POWER_PREF and WGPU_ADAPTER_NAME still choose.
    let high = Some(PowerPreference::HighPerformance);
    assert_eq!(display_adapter(&optimus, high, None), Some(1));
    let low = Some(PowerPreference::LowPower);
    assert_eq!(display_adapter(&offloaded, low, None), Some(1));
    let none = Some(PowerPreference::None);
    assert_eq!(display_adapter(&offloaded, none, None), Some(0));
    assert_eq!(display_adapter(&optimus, None, Some("NVIDIA")), Some(1));
    assert_eq!(display_adapter(&optimus, None, Some("radeon")), Some(0));
}
#[test]
fn eframes_reason_for_giving_up_is_kept() {
    use log::Log;
    let record = |target: &str, level, message: &str| {
        EframeErrors.log(
            &log::Record::builder()
                .target(target)
                .level(level)
                .args(format_args!("{message}"))
                .build(),
        );
    };
    record(
        "wgpu_core",
        log::Level::Error,
        "Exiting because of error: other",
    );
    record(
        "eframe::native::run",
        log::Level::Warn,
        "Exiting because of error: warn",
    );
    assert_eq!(EframeErrors::take(), None);
    record(
        "eframe::native::run",
        log::Level::Error,
        "Exiting because of error: No suitable GPU adapter",
    );
    assert_eq!(
        EframeErrors::take().as_deref(),
        Some("No suitable GPU adapter")
    );
    assert_eq!(EframeErrors::take(), None);

    // wgpu only logs the driver call behind "Parent device is lost" (#768).
    record(
        "wgpu_hal::dx12",
        log::Level::Error,
        "Queue creation failed: E_INVALIDARG",
    );
    record(
        "eframe::native::run",
        log::Level::Error,
        "Exiting because of error: WGPU error: Parent device is lost",
    );
    assert_eq!(
        EframeErrors::take().as_deref(),
        Some("WGPU error: Parent device is lost\n\nQueue creation failed: E_INVALIDARG")
    );
}
#[test]
fn an_imported_value_outside_the_slider_survives_being_shown_and_nudged() {
    let ctx = egui::Context::default();
    let row = std::cell::Cell::new(Rect::NOTHING);
    let draw = |value: &mut f32, events: Vec<egui::Event>, time| {
        widget_frame(&ctx, time, events, |ui| {
            let top = ui.cursor().min;
            super::widgets::setting_slider(
                ui,
                crate::model::params::ParameterId::Exposure,
                value,
                0.,
            );
            row.set(Rect::from_min_max(
                top,
                Pos2::new(ui.max_rect().right(), ui.cursor().top()),
            ));
        })
    };
    let key = |key| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    // An imported +6 EV, beyond the slider's ±5.
    let mut value = 6.;
    draw(&mut value, vec![], 0.);
    assert_eq!(value, 6., "showing the slider changed the value");
    let over = egui::Event::PointerMoved(row.get().center());
    draw(&mut value, vec![over.clone()], 1.);
    assert_eq!(value, 6.);
    // Up moves it no further out; Down moves it one step toward the range,
    // not to its end.
    draw(&mut value, vec![over.clone(), key(egui::Key::ArrowUp)], 2.);
    assert_eq!(value, 6.);
    draw(&mut value, vec![over, key(egui::Key::ArrowDown)], 3.);
    assert!(5. < value && value < 6., "{value}");
}
#[test]
fn a_swatch_added_while_color_mixer_is_off_turns_it_on() {
    use crate::model::panels::{Panel, PanelState};
    let ctx = egui::Context::default();
    let (mut editor, _) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), true);
    editor
        .document
        .edit
        .setup_mut()
        .panels
        .set(Panel::ColorMixer, PanelState::Off);
    editor.view.mixer_tab = state::MixerTab::PointColor;
    editor.view.toggle(state::Tool::PointColor);
    editor.start_point_color_sample(0.7, 0.5);
    let start = std::time::Instant::now();
    while editor.document.point_color_pick.is_running() {
        assert!(start.elapsed().as_secs() < 60, "sampling did not finish");
        std::thread::sleep(std::time::Duration::from_millis(5));
        editor.events(&ctx);
    }
    assert_eq!(editor.document.edit.recipe().point_colors.len(), 1);
    // As a slider moved in a switched-off panel does, so the swatch shows.
    assert_eq!(
        editor
            .document
            .edit
            .recipe()
            .panels
            .state(Panel::ColorMixer),
        PanelState::On
    );
    editor.undo();
    assert_eq!(
        editor
            .document
            .edit
            .recipe()
            .panels
            .state(Panel::ColorMixer),
        PanelState::Off
    );
}
#[test]
fn auto_ends_a_conversion_waiting_for_the_photo() {
    let ctx = egui::Context::default();
    let (mut editor, _) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), false);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(editor.document.pending_treatment.is_some());
    let mut auto = editor.document.edit.recipe().clone();
    auto.exposure = 1.;
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    // Any other edit drops the waiting conversion, as an edit made with a slider does.
    assert_eq!(editor.document.edit.recipe().exposure, 1.);
    assert!(editor.document.pending_treatment.is_none());
}
#[test]
fn quitting_saves_an_edit_still_waiting_for_autosave() -> anyhow::Result<()> {
    // Quit on macOS (Cmd-Q) closes the window without a close request, so only the
    // exit hook sees it; the close guard's flush never runs.
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let path = d.path().join("photos/a.RAF");
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.path = Some(path.clone());
    let before = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 0.7;
    e.commit_edit(before, None);
    assert!(e.document.edit.save_state().needs_save());
    eframe::App::on_exit(&mut e, None);
    assert!(!e.document.edit.save_state().needs_save());
    let library = e.library.as_ref().unwrap();
    let saved = library.session.catalog.load_edit(ids[0], &path)?.unwrap();
    assert_eq!(saved.recipe.exposure, 0.7);
    Ok(())
}
/// An editor showing a catalog photo with an edit not saved yet, on a catalog whose
/// saves stall, as on a network share that stopped answering.
fn editor_with_a_stalled_catalog() -> anyhow::Result<(tempfile::TempDir, Editor)> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.path = Some(d.path().join("photos/a.RAF"));
    e.autosave = autosave::Autosave::stalled();
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
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
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
/// A catalog of two photos, A and B, the latter rated 2, in an editor.
fn two_photo_editor() -> anyhow::Result<(tempfile::TempDir, egui::Context, Editor, [PhotoId; 2])> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.ARW"), b"identity fixture a")?;
    std::fs::write(photos.join("b.ARW"), b"identity fixture b")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut l = library::Library::load(&catalog, ctx.clone())?;
    let id = |name: &str, l: &library::Library| {
        l.session
            .photos
            .iter()
            .find(|p| p.filename.ends_with(name))
            .unwrap()
            .id
    };
    let (a, b) = (id("a.ARW", &l), id("b.ARW", &l));
    l.edit_metadata(b, crate::app::photo_metadata::Edit::Rating(2), false)?;
    l.take_done();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.module = Module::Library;
    Ok((dir, ctx, editor, [a, b]))
}
fn rating(editor: &Editor, id: PhotoId) -> i32 {
    editor.library.as_ref().unwrap().photo(id).unwrap().rating
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
    library.edit_metadata(copy, crate::app::photo_metadata::Edit::Rating(5), false)?;
    editor.sync_undo();
    // Removing reads no collections; reading the catalog again does, and fails.
    editor
        .library
        .as_mut()
        .unwrap()
        .session
        .catalog
        .db_for_tests()
        .execute_batch("ALTER TABLE collections RENAME TO collections_gone")?;
    editor.remove_virtual_copy(copy);
    let catalog = &editor.library.as_ref().unwrap().session.catalog;
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
        .catalog
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
    library.edit_metadata(copy, crate::app::photo_metadata::Edit::Rating(5), false)?;
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
        crate::app::photo_metadata::Edit::Rating(4),
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
        crate::app::photo_metadata::Edit::Rating(3),
        false,
    )?;
    editor.sync_undo();
    editor.remove_virtual_copy(copy);
    assert_eq!(rating(&editor, b), 3);
    editor.undo();
    assert_eq!(rating(&editor, b), 2);
    Ok(())
}
/// Issue #746: with Picked shown, U in Develop hides the photo open, and the
/// arrows only work again once Develop moves to a photo still shown.
#[test]
fn unflagging_the_photo_open_in_develop_with_picked_shown_opens_one_still_shown()
-> anyhow::Result<()> {
    use crate::app::photo_metadata::Edit;
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
    let id = l.session.photos[0].id;
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.document.catalog_photo = Some(id);
    editor.document.path = Some(photo.clone());
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
        .catalog
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
        .catalog
        .load_edit(id, &photo)?
        .unwrap();
    assert_eq!(saved.recipe.exposure, 0.);
    Ok(())
}
/// A frame of the whole window, with a button holding the keyboard focus when
/// `focus` says so.
fn panel_frame(e: &mut Editor, events: Vec<egui::Event>, focus: FocusedButton) {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events,
            ..Default::default()
        },
        |ui| {
            if focus == FocusedButton::Yes {
                egui::Area::new(egui::Id::new("focus-holder"))
                    .show(ui.ctx(), |ui| ui.button("focused").request_focus());
            }
            e.draw(ui)
        },
    );
    output.textures_delta.clear();
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusedButton {
    Yes,
    No,
}
/// A frame of the workspace, and the commands it sent the window.
fn panel_frame_commands(e: &mut Editor, events: Vec<egui::Event>) -> Vec<egui::ViewportCommand> {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events,
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
    output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .clone()
}
fn key_up(key: egui::Key) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]
}
fn key_down(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }]
}
#[test]
fn tab_hides_develops_side_panels_and_the_photo_takes_their_room() {
    use super::panels::WorkspacePanel;
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("session.json");
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::session::Session::default(),
        Some(session.clone()),
    );
    // The first launch's catalog opens first, in the Library.
    settle_catalog(&mut e, &ctx);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    // Panels take their width in the first frames.
    for _ in 0..3 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    let with_panels = e.view.viewport;
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    assert!(!e.panel_shown(WorkspacePanel::Right));
    assert!(e.panel_shown(WorkspacePanel::Filmstrip));
    assert!(e.view.viewport.x > with_panels.x + 400.);
    assert_eq!(e.view.viewport.y, with_panels.y);
    // Tab took no focus to the first control on its way.
    assert!(ctx.memory(|m| m.focused().is_none()));
    // Kept for the next launch, for Develop only.
    let saved: crate::app::session::Session =
        serde_json::from_slice(&std::fs::read(&session).unwrap()).unwrap();
    assert_eq!(saved.panels, e.panels);
    assert!(saved.panels.library.shown(WorkspacePanel::Left));
    // Shift+Tab hides the filmstrip and status bar too; again brings back
    // what it hid, the filmstrip, and Tab then the sides.
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::SHIFT),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Filmstrip));
    assert!(e.view.viewport.y > with_panels.y);
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::SHIFT),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Filmstrip));
    assert!(!e.panel_shown(WorkspacePanel::Left));
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert_eq!(e.view.viewport, with_panels);
    // F8 hides the adjustments alone.
    panel_frame(
        &mut e,
        key_down(egui::Key::F8, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(!e.panel_shown(WorkspacePanel::Right));
}
#[test]
fn f_gives_the_photo_the_window_without_touching_the_window() -> anyhow::Result<()> {
    use super::panels::WorkspacePanel;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    // Panels take their width in the first frames.
    for _ in 0..3 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    let with_chrome = e.view.viewport;
    let layout = e.panels;
    let commands = panel_frame_commands(&mut e, key_down(egui::Key::F, egui::Modifiers::NONE));
    panel_frame(&mut e, vec![], FocusedButton::No);
    // The mode is the app's own: no display is filled and the window keeps its
    // size, as the window's own full screen would change it.
    assert!(
        !commands.iter().any(|command| matches!(
            command,
            egui::ViewportCommand::Fullscreen(_)
                | egui::ViewportCommand::Maximized(_)
                | egui::ViewportCommand::Minimized(_)
                | egui::ViewportCommand::WindowLevel(_)
        )),
        "{commands:?}"
    );
    for panel in [
        WorkspacePanel::Left,
        WorkspacePanel::Right,
        WorkspacePanel::Filmstrip,
        WorkspacePanel::Presets,
    ] {
        assert!(!e.panel_shown(panel), "{panel:?} still shows");
    }
    // The bar, the panels, the filmstrip and Develop's toolbar are the photo's
    // room now, less the margin it is drawn with.
    let margin = crate::app::workspace::PHOTO_MARGIN;
    assert_eq!(
        e.view.viewport,
        Vec2::new(1200., 800.) - Vec2::splat(2. * margin)
    );
    // Only what is drawn changed: the panels keep the layout saved for them.
    assert_eq!(e.panels, layout);
    // F again brings the window back, as it was.
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert_eq!(e.view.viewport, with_chrome);
    Ok(())
}
#[test]
fn f_hides_the_chrome_of_the_module_it_was_turned_on_in() {
    use super::panels::WorkspacePanel;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Library;
    e.onboarding.visible = false;
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    // Develop draws its chrome, and the Library's is back when it returns: the
    // mode belongs to the module it was turned on in.
    e.module = Module::Develop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    e.module = Module::Library;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(e.panel_shown(WorkspacePanel::Right));
}
#[test]
fn escape_leaves_the_whole_window_mode() {
    use super::panels::WorkspacePanel;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    panel_frame(&mut e, vec![], FocusedButton::No);
    // With the chrome up, Escape is left to Develop's tools: it neither hides
    // the chrome nor is lost to it.
    panel_frame(
        &mut e,
        key_down(egui::Key::Escape, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::Escape), FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    // Escape brings the window back, as it does in Lightroom.
    panel_frame(
        &mut e,
        key_down(egui::Key::Escape, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::Escape), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(e.panel_shown(WorkspacePanel::Right));
}
#[test]
fn in_the_library_f_gives_the_window_to_the_grid_then_the_photo() -> anyhow::Result<()> {
    use super::panels::WorkspacePanel;
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.onboarding.visible = false;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(!e.library.as_ref().unwrap().loupe_open());
    // The first F gives the window to the grid; the photo is not opened for it.
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    assert!(!e.library.as_ref().unwrap().loupe_open());
    // The next F gives it to the photo, in the Loupe, with the chrome still
    // hidden.
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    assert!(e.library.as_ref().unwrap().loupe_open());
    // The third brings the chrome back, and the grid with it: the mode opened
    // the Loupe, so leaving the mode closes it again.
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(!e.library.as_ref().unwrap().loupe_open());
    Ok(())
}
#[test]
fn the_mode_keeps_a_loupe_it_did_not_open() -> anyhow::Result<()> {
    use super::panels::WorkspacePanel;
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    // A photo already open is what the mode gives the window to, instead of the
    // grid, and leaving the mode leaves it open.
    e.library.as_mut().unwrap().open_loupe();
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    assert!(e.library.as_ref().unwrap().loupe_open());
    panel_frame(
        &mut e,
        key_down(egui::Key::Escape, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::Escape), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(e.library.as_ref().unwrap().loupe_open());
    Ok(())
}
#[test]
fn entering_another_module_closes_a_loupe_the_mode_opened() -> anyhow::Result<()> {
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    for _ in 0..2 {
        panel_frame(
            &mut e,
            key_down(egui::Key::F, egui::Modifiers::NONE),
            FocusedButton::No,
        );
        panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    assert!(e.library.as_ref().unwrap().loupe_open());
    // D draws Develop's chrome, and the Library is back at its grid: the mode
    // belongs to the module it was turned on in.
    e.module = Module::Develop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!e.library.as_ref().unwrap().loupe_open());
    Ok(())
}
#[test]
fn f_leaves_the_mode_over_a_loupe_opened_in_it() -> anyhow::Result<()> {
    use super::panels::WorkspacePanel;
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    let f = |e: &mut Editor| {
        panel_frame(
            e,
            key_down(egui::Key::F, egui::Modifiers::NONE),
            FocusedButton::No,
        );
        panel_frame(e, key_up(egui::Key::F), FocusedButton::No);
        panel_frame(e, vec![], FocusedButton::No);
    };
    f(&mut e);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    // The grid has the window, and a photo is opened from it, as a double
    // click does: the photo has the window now, so F leaves the mode, and the
    // Loupe stays, as the mode did not open it.
    e.library.as_mut().unwrap().open_loupe();
    panel_frame(&mut e, vec![], FocusedButton::No);
    f(&mut e);
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(e.library.as_ref().unwrap().loupe_open());
    Ok(())
}
#[test]
fn panel_keys_bring_the_hidden_chrome_back_as_it_was() {
    use super::panels::WorkspacePanel;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    panel_frame(&mut e, vec![], FocusedButton::No);
    let layout = e.panels;
    // Each panel key, with every panel hidden by the mode, brings the chrome
    // back with the layout it had, rather than changing one no one sees.
    for (key, modifiers) in [
        (egui::Key::Tab, egui::Modifiers::NONE),
        (egui::Key::Tab, egui::Modifiers::SHIFT),
        (egui::Key::F7, egui::Modifiers::NONE),
        (egui::Key::F8, egui::Modifiers::SHIFT),
    ] {
        panel_frame(
            &mut e,
            key_down(egui::Key::F, egui::Modifiers::NONE),
            FocusedButton::No,
        );
        panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
        panel_frame(&mut e, vec![], FocusedButton::No);
        assert!(!e.panel_shown(WorkspacePanel::Left));
        panel_frame(&mut e, key_down(key, modifiers), FocusedButton::No);
        panel_frame(&mut e, key_up(key), FocusedButton::No);
        panel_frame(&mut e, vec![], FocusedButton::No);
        assert_eq!(e.panels, layout, "{modifiers:?} {key:?}");
        for panel in [
            WorkspacePanel::Left,
            WorkspacePanel::Right,
            WorkspacePanel::Filmstrip,
            WorkspacePanel::Presets,
        ] {
            assert!(
                e.panel_shown(panel),
                "{panel:?} after {modifiers:?} {key:?}"
            );
        }
    }
}
#[test]
fn a_loupe_reopened_by_hand_in_the_mode_stays_on_leaving_it() -> anyhow::Result<()> {
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    let press = |e: &mut Editor, key| {
        panel_frame(e, key_down(key, egui::Modifiers::NONE), FocusedButton::No);
        panel_frame(e, key_up(key), FocusedButton::No);
        panel_frame(e, vec![], FocusedButton::No);
    };
    // The mode opens the Loupe, and the user goes back to the grid and opens
    // it again: that Loupe is the user's, and leaving the mode keeps it.
    press(&mut e, egui::Key::F);
    press(&mut e, egui::Key::F);
    assert!(e.library.as_ref().unwrap().loupe_open());
    e.library.as_mut().unwrap().close_loupe();
    panel_frame(&mut e, vec![], FocusedButton::No);
    press(&mut e, egui::Key::E);
    assert!(e.library.as_ref().unwrap().loupe_open());
    press(&mut e, egui::Key::F);
    assert!(!e.chrome.hidden(e.module));
    assert!(e.library.as_ref().unwrap().loupe_open());
    Ok(())
}
#[test]
fn hiding_the_chrome_saves_a_library_field_being_typed_first() -> anyhow::Result<()> {
    use super::commands::Action;
    use super::panels::WorkspacePanel;
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    let copy = e
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(ids[0])?
        .value;
    e.library.as_mut().unwrap().select(Some(copy));
    panel_frame(&mut e, vec![], FocusedButton::No);
    // The info panel goes with the chrome, as hiding it does: what is being
    // typed in it is saved first.
    e.library
        .as_mut()
        .unwrap()
        .set_copy_name_draft(copy, "Typed name");
    e.execute_action(Action::Chrome, None).unwrap();
    assert!(!e.panel_shown(WorkspacePanel::Right));
    assert_eq!(
        e.library.as_ref().unwrap().photo(copy).unwrap().copy_name,
        "Typed name"
    );
    let view = e.chrome.show();
    e.chrome_view(view);
    // A field that cannot be saved keeps the panel, and the command says why.
    let library = e.library.as_mut().unwrap();
    library.session.catalog.remove_virtual_copy(copy)?;
    library.set_copy_name_draft(copy, "Unsaved name");
    let refused = e.execute_action(Action::Chrome, None).unwrap_err();
    assert_eq!(refused.code, "save_failed");
    assert!(e.panel_shown(WorkspacePanel::Right));
    Ok(())
}
#[test]
fn state_reports_the_panels_the_chrome_hides() {
    use super::commands::Action;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let state = serde_json::to_value(e.command_state()).unwrap();
    assert_eq!(state["panels"]["left"], "shown");
    assert_eq!(state["chrome"], "shown");
    e.execute_action(Action::Chrome, None).unwrap();
    let state = serde_json::to_value(e.command_state()).unwrap();
    for panel in ["left", "right", "filmstrip", "presets"] {
        assert_eq!(state["panels"][panel], "hidden", "{panel}");
    }
    assert_eq!(state["chrome"], "hidden");
}
#[test]
fn escape_closes_a_menu_before_it_leaves_the_mode() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    panel_frame(
        &mut e,
        key_down(egui::Key::F, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    panel_frame(&mut e, key_up(egui::Key::F), FocusedButton::No);
    let ctx = e.context.clone();
    let cell = ctx
        .read_response(egui::Id::new(("catalog-photo-cell", ids[0])))
        .unwrap();
    let pos = cell.rect.center();
    panel_frame(
        &mut e,
        vec![egui::Event::PointerMoved(pos)],
        FocusedButton::No,
    );
    for pressed in [true, false] {
        panel_frame(
            &mut e,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }],
            FocusedButton::No,
        );
    }
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(egui::Popup::is_any_open(&ctx));
    // The photo's menu takes Escape first, and the mode stays.
    panel_frame(
        &mut e,
        key_down(egui::Key::Escape, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(!egui::Popup::is_any_open(&ctx));
    assert!(e.chrome.hidden(e.module));
    Ok(())
}
#[test]
fn f_leaves_the_mode_over_a_library_view_with_no_photo() -> anyhow::Result<()> {
    let (_d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    e.onboarding.visible = false;
    let library = e.library.as_mut().unwrap();
    let mut layout = library.layout();
    layout.query = "nothing-matches-this".into();
    layout.filters_off = false;
    library.apply_layout(&layout);
    assert!(library.selected_or_first().is_none());
    // The grid takes the window; with no photo to give it to next, the next
    // F brings the chrome back rather than asking for a Loupe forever.
    let f = |e: &mut Editor| {
        panel_frame(
            e,
            key_down(egui::Key::F, egui::Modifiers::NONE),
            FocusedButton::No,
        );
        panel_frame(e, key_up(egui::Key::F), FocusedButton::No);
        panel_frame(e, vec![], FocusedButton::No);
    };
    f(&mut e);
    assert!(e.chrome.hidden(e.module));
    f(&mut e);
    assert!(!e.chrome.hidden(e.module));
    assert!(!e.library.as_ref().unwrap().loupe_open());
    Ok(())
}
#[test]
fn tab_moves_the_focus_on_while_a_control_has_it() {
    use super::panels::WorkspacePanel;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    panel_frame(&mut e, vec![], FocusedButton::Yes);
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(e.panel_shown(WorkspacePanel::Left));
    assert!(e.panel_shown(WorkspacePanel::Right));
}
#[test]
fn each_module_keeps_its_own_panels() -> anyhow::Result<()> {
    use super::panels::WorkspacePanel;
    let (_d, mut e, _ids) = editor_with_catalog(&["a.RAF"])?;
    panel_frame(&mut e, vec![], FocusedButton::No);
    panel_frame(
        &mut e,
        key_down(egui::Key::F7, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(!e.panel_shown(WorkspacePanel::Left));
    e.module = Module::Develop;
    assert!(e.panel_shown(WorkspacePanel::Left));
    e.module = Module::Library;
    // The Library draws with its filmstrip hidden.
    panel_frame(
        &mut e,
        key_down(egui::Key::F6, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(!e.panel_shown(WorkspacePanel::Filmstrip));
    panel_frame(&mut e, vec![], FocusedButton::No);
    Ok(())
}
#[test]
fn shift_f8_folds_develops_tab_panel_alone() {
    use super::panels::WorkspacePanel;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let drawn = |ctx: &egui::Context| {
        ctx.read_response(egui::Id::new("presets").with("__resize"))
            .is_some()
    };
    for _ in 0..2 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    // 1200 points leave it folded by itself; Shift+F8 opens it, as its rail does.
    assert!(!drawn(&ctx));
    let shift_f8 = || key_down(egui::Key::F8, egui::Modifiers::SHIFT);
    panel_frame(&mut e, shift_f8(), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(drawn(&ctx));
    // F8's plain pattern did not take it: the adjustments stay.
    assert!(e.panel_shown(WorkspacePanel::Right));
    panel_frame(&mut e, shift_f8(), FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!drawn(&ctx));
    assert!(e.panel_shown(WorkspacePanel::Right));
}
#[test]
fn hiding_develops_right_panel_takes_its_tab_panel_and_rail() {
    use super::panels::{DevelopTab, PanelChange, WorkspacePanel};
    use super::side_tabs::rail_id;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    for _ in 0..2 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    assert!(ctx.read_response(rail_id(DevelopTab::Presets)).is_some());
    assert!(e.change_panels(PanelChange::Toggle(WorkspacePanel::Right)));
    for _ in 0..2 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    assert!(ctx.read_response(rail_id(DevelopTab::Presets)).is_none());
    assert!(
        ctx.read_response(egui::Id::new("presets").with("__resize"))
            .is_none()
    );
    // A tool's drawer keeps to the hidden side too.
    e.view.tool = state::Tool::Crop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(ctx.read_response(rail_id(DevelopTab::Presets)).is_none());
}
#[test]
fn a_tools_drawer_takes_the_adjustments_place_beside_the_tabs() {
    // The rail's first group (Edit and the tools) switches the adjustments
    // panel; Presets and History keep their panel beside it.
    use super::panels::{DevelopTab, PanelChange, WorkspacePanel};
    use super::side_tabs::{edit_id, rail_id};
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    // Room for the tab panel beside the photo.
    assert!(e.change_panels(PanelChange::Toggle(WorkspacePanel::Left)));
    let tabs_drawn = |ctx: &egui::Context| {
        ctx.read_response(egui::Id::new("presets").with("__resize"))
            .is_some()
    };
    let click = |e: &mut Editor, id: egui::Id| {
        let at = ctx
            .read_response(id)
            .expect("the rail's button")
            .rect
            .center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        panel_frame(e, vec![egui::Event::PointerMoved(at)], FocusedButton::No);
        panel_frame(e, vec![button(true)], FocusedButton::No);
        panel_frame(e, vec![button(false)], FocusedButton::No);
        panel_frame(e, vec![], FocusedButton::No);
    };
    for _ in 0..3 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    assert!(tabs_drawn(&ctx));
    // Crop's drawer leaves the Presets tab open.
    e.view.tool = state::Tool::Crop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(tabs_drawn(&ctx));
    // Switching the tab keeps the tool.
    click(&mut e, rail_id(DevelopTab::History));
    assert_eq!(e.panels.develop_tab, DevelopTab::History);
    assert_eq!(e.view.tool, state::Tool::Crop);
    assert!(tabs_drawn(&ctx));
    // Edit closes the drawer, the tab panel stays.
    click(&mut e, edit_id());
    assert_eq!(e.view.tool, state::Tool::None);
    assert!(tabs_drawn(&ctx));
    // Folding the tabs leaves a tool open.
    e.view.tool = state::Tool::Mask;
    click(&mut e, rail_id(DevelopTab::History));
    assert!(!tabs_drawn(&ctx));
    assert_eq!(e.view.tool, state::Tool::Mask);
}
#[test]
fn hiding_the_presets_ends_a_preset_preview() {
    use super::panels::PanelChange;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.presets.preview = Some(Recipe {
        exposure: 1.,
        ..Default::default()
    });
    e.presets.hover = Some((0, std::time::Instant::now()));
    assert!(e.change_panels(PanelChange::Sides));
    assert!(e.presets.preview.is_none());
    assert!(e.presets.hover.is_none());
}
#[test]
fn a_narrow_window_folds_develops_tabs_until_one_is_asked_for() {
    use super::panels::{DevelopTab, WorkspacePanel};
    use super::side_tabs::rail_id;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let drawn = |ctx: &egui::Context| {
        ctx.read_response(egui::Id::new("presets").with("__resize"))
            .is_some()
    };
    for _ in 0..3 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    // 1200 points leave no room for it beside the sidebar and adjustments:
    // folded to its rail, the photo keeps its width.
    assert!(e.panel_shown(WorkspacePanel::Presets));
    assert!(!drawn(&ctx));
    assert!(e.view.viewport.x > 400.);
    let at = ctx
        .read_response(rail_id(DevelopTab::History))
        .expect("the rail's History button")
        .rect
        .center();
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    panel_frame(
        &mut e,
        vec![egui::Event::PointerMoved(at)],
        FocusedButton::No,
    );
    panel_frame(&mut e, vec![button(true)], FocusedButton::No);
    panel_frame(&mut e, vec![button(false)], FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    // Asked for, it opens on that tab; the layout saved is unchanged.
    assert_eq!(e.panels.develop_tab, DevelopTab::History);
    assert!(drawn(&ctx));
    assert!(e.panel_shown(WorkspacePanel::Presets));
}
#[test]
fn the_library_info_panel_stays_while_a_field_in_it_cannot_be_saved() -> anyhow::Result<()> {
    use super::panels::{PanelChange, WorkspacePanel};
    let (_d, mut e, ids) = editor_with_catalog(&["a.ARW"])?;
    let library = e.library.as_mut().unwrap();
    let copy = library.create_virtual_copy(ids[0])?.value;
    library.session.catalog.remove_virtual_copy(copy)?;
    library.set_copy_name_draft(copy, "B&W");
    assert!(!e.change_panels(PanelChange::Toggle(WorkspacePanel::Right)));
    assert!(e.panel_shown(WorkspacePanel::Right));
    assert!(e.messages.line().starts_with("Not saved"));
    // The left panel holds no field; it hides.
    assert!(e.change_panels(PanelChange::Toggle(WorkspacePanel::Left)));
    e.library.as_mut().unwrap().discard_drafts();
    assert!(e.change_panels(PanelChange::Sides));
    assert!(!e.panel_shown(WorkspacePanel::Right));
    Ok(())
}
#[test]
fn the_bars_panel_buttons_hide_and_show_their_panels() {
    use super::panels::{WorkspacePanel, toggle_id};
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let click = |e: &mut Editor, panel: WorkspacePanel| {
        let at = ctx
            .read_response(toggle_id(panel))
            .expect("the bar draws the panel's button")
            .rect
            .center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        panel_frame(e, vec![egui::Event::PointerMoved(at)], FocusedButton::No);
        panel_frame(e, vec![button(true)], FocusedButton::No);
        panel_frame(e, vec![button(false)], FocusedButton::No);
    };
    panel_frame(&mut e, vec![], FocusedButton::No);
    click(&mut e, WorkspacePanel::Left);
    assert!(!e.panel_shown(WorkspacePanel::Left));
    assert!(e.panel_shown(WorkspacePanel::Right));
    click(&mut e, WorkspacePanel::Left);
    assert!(e.panel_shown(WorkspacePanel::Left));
    click(&mut e, WorkspacePanel::Right);
    assert!(!e.panel_shown(WorkspacePanel::Right));
    click(&mut e, WorkspacePanel::Filmstrip);
    assert!(!e.panel_shown(WorkspacePanel::Filmstrip));
    // A button clicked keeps no focus that would take Tab from the panels.
    panel_frame(
        &mut e,
        key_down(egui::Key::Tab, egui::Modifiers::NONE),
        FocusedButton::No,
    );
    assert!(
        !e.panel_shown(WorkspacePanel::Left),
        "Tab hid the left panel"
    );
    // The setup view has no panels, and no buttons for them.
    e.onboarding.visible = true;
    panel_frame(&mut e, vec![], FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(ctx.read_response(toggle_id(WorkspacePanel::Left)).is_none());
}

#[test]
fn long_lens_names_keep_the_develop_panel_on_screen() {
    // Issues 362 and 363: a long lens or lens profile name widened the panel past
    // the window, and the photo was drawn over the panel's left part.
    let viewport = |lens: &str, profile: &str| {
        let ctx = egui::Context::default();
        let mut editor =
            Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
        let mut m = Metadata {
            make: "Testcam".into(),
            lens_model: lens.into(),
            focal: 35.,
            aperture: 2.,
            width: 600,
            height: 400,
            ..Default::default()
        };
        let lcp = crate::lens::lcp::test_profile("Testcam", lens, profile, -0.05, -0.5);
        m.lens_profiles =
            crate::lens::lcp::Library::from_texts([("Testcam - RAW.lcp", lcp.as_str())])
                .for_photo(&m);
        assert_eq!(m.lens_profiles.all().len(), 1);
        editor.document.metadata = Some(m);
        editor.document.edit.setup_mut().lens_profile = true;
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1400., 800.));
        for _ in 0..5 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| editor.draw(ui),
            );
            output.textures_delta.clear();
        }
        let panel = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("adjustments"))
            .expect("the Develop panel is shown");
        assert!(
            panel.outer_rect.max.x <= screen.max.x,
            "panel {:?} leaves the window",
            panel.outer_rect
        );
        editor.view.viewport
    };
    let long = "AF-S VR Zoom-Nikkor 70-200mm f/2.8G IF-ED with a converter's long description";
    let short = viewport("35mm F2", "Adobe (35mm F2)");
    assert_eq!(viewport(long, "Adobe (35mm F2)"), short);
    assert_eq!(
        viewport("35mm F2", &format!("Adobe ({long}, Testcam)")),
        short
    );
}

#[test]
fn a_side_panels_edge_shows_the_resize_cursor() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    // Enough photos that the grid's cells reach the panels' edges.
    for i in 0..60 {
        std::fs::write(photos.join(format!("a{i}.ARW")), format!("fixture {i}"))?;
    }
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    e.onboarding.visible = false;
    let frame = |e: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                events,
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
        output.platform_output.cursor_icon
    };
    // Develop's tab panel last, with room made for it by hiding the sidebar.
    for (module, panels) in [
        (Module::Library, &["library-sidebar", "library-info"][..]),
        (Module::Develop, &["library-sidebar", "adjustments"][..]),
        (Module::Develop, &["presets"][..]),
    ] {
        e.module = module;
        if panels == ["presets"] {
            e.change_panels(super::panels::PanelChange::Toggle(
                super::panels::WorkspacePanel::Left,
            ));
        }
        for _ in 0..3 {
            frame(&mut e, vec![]);
        }
        for &panel in panels {
            let edge = ctx
                .read_response(egui::Id::new(panel).with("__resize"))
                .expect("the panel's edge")
                .rect;
            // Both halves of its grab area: the panel's and the one over the
            // grid or the photo, whose cells are drawn later.
            let half = edge.width() / 2. - 1.;
            for at in [
                edge.center(),
                edge.center() - Vec2::new(half, 0.),
                edge.center() + Vec2::new(half, 0.),
            ] {
                frame(&mut e, vec![egui::Event::PointerMoved(at)]);
                let cursor = frame(&mut e, vec![egui::Event::PointerMoved(at)]);
                assert!(
                    matches!(
                        cursor,
                        egui::CursorIcon::ResizeHorizontal
                            | egui::CursorIcon::ResizeEast
                            | egui::CursorIcon::ResizeWest
                    ),
                    "{panel} at {at:?}: {cursor:?}"
                );
            }
        }
    }
    Ok(())
}

/// Pinned panels are gone: a session saved with some still opens, the rest of it
/// kept.
#[test]
fn a_session_with_pinned_panels_still_opens() {
    let saved: crate::app::session::Session = serde_json::from_str(
        r#"{"pinned": ["Histogram", "Navigator"], "solo": ["develop-right"]}"#,
    )
    .expect("an older session");
    assert!(saved.solo.contains("develop-right"));
}

/// With the chrome hidden the Loupe's toolbar goes, and its photo takes the row.
#[test]
fn the_chrome_hidden_gives_the_loupe_its_toolbar_row() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::from_fn(300, 200, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    })
    .save(folder.join("a.jpg"))?;
    let path = directory.path().join("library.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let mut library = library::Library::load(&path, ctx.clone())?;
    library.wait_for_availability();
    library.select(Some(library.session.photos[0].id));
    library.open_loupe();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    let mut view = |chrome: bool| {
        for _ in 0..2 {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                    ..Default::default()
                },
                |ui| {
                    e.library_view(ui, chrome);
                },
            )
            .textures_delta
            .clear();
        }
        e.library.as_ref().unwrap().loupe_view()
    };
    let with_toolbar = view(true);
    let without = view(false);
    // The view is taller by the toolbar's row, and no wider for it.
    assert!(
        without.y > with_toolbar.y + 20.,
        "{without:?} against {with_toolbar:?}"
    );
    assert_eq!(without.x, with_toolbar.x);
    Ok(())
}
/// In the Library's Loupe, a RAW (drawn through Develop's pipeline) can be
/// magnified at Fit, not zoomed in; a point chosen on the minimap of a photo the
/// Loupe draws itself zooms in there.
#[test]
fn the_librarys_loupe_magnifies_raws_at_fit_and_its_minimap_zooms() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    for name in ["a.ARW", "b.ARW"] {
        std::fs::write(photos.join(name), name)?;
    }
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.library = Some(Box::new(library::Library::load(&path, ctx)?));
    e.module = Module::Library;
    e.library.as_mut().unwrap().open_loupe();
    assert!(e.library.as_ref().unwrap().loupe_develops().is_some());
    assert_eq!(e.magnifier_blocked(), None);
    e.toggle_magnifier();
    assert!(e.view.magnifier.on);
    e.library_inspect([0.25, 0.75]);
    assert!(e.view.zoom.on);
    assert_eq!(e.view.zoom.pan, [0.25, 0.75]);
    assert!(e.magnifier_blocked().is_some());
    Ok(())
}

#[test]
fn side_panels_stop_where_their_contents_stop() -> anyhow::Result<()> {
    // Dragged narrower than its contents, a panel was painted only that wide
    // but laid out as wide as them: the strip between showed the window's
    // black. Each one stops at its minimum, which its contents fit.
    use super::workspace::{ADJUSTMENTS_MIN, LIBRARY_INFO_MIN, LIBRARY_SIDEBAR_MIN, PRESETS_MIN};
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.ARW"), b"fixture")?;
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    e.onboarding.visible = false;
    let frame = |e: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                events,
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
    };
    let button = |at: Pos2, pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    for (module, panel, outward, min) in [
        (Module::Develop, "library-sidebar", 0., LIBRARY_SIDEBAR_MIN),
        (Module::Develop, "presets", 1200., PRESETS_MIN),
        (Module::Develop, "adjustments", 1200., ADJUSTMENTS_MIN),
        (Module::Library, "library-sidebar", 0., LIBRARY_SIDEBAR_MIN),
        (Module::Library, "library-info", 1200., LIBRARY_INFO_MIN),
    ] {
        e.module = module;
        // The window has room for Develop's tab panel without the sidebar.
        if panel == "presets" {
            e.change_panels(super::panels::PanelChange::Toggle(
                super::panels::WorkspacePanel::Left,
            ));
        }
        for _ in 0..3 {
            frame(&mut e, vec![]);
        }
        let edge = |ctx: &egui::Context| {
            ctx.read_response(egui::Id::new(panel).with("__resize"))
                .expect("the panel's edge")
                .rect
                .center()
                .x
        };
        let start = Pos2::new(edge(&ctx), 400.);
        frame(&mut e, vec![egui::Event::PointerMoved(start)]);
        frame(&mut e, vec![button(start, true)]);
        // Mid-drag, as far as it goes: laid out at its minimum, as painted.
        let to = Pos2::new(outward, start.y);
        for _ in 0..3 {
            frame(&mut e, vec![egui::Event::PointerMoved(to)]);
        }
        // Develop's tab panel stops at the adjustments, and they at the tab
        // rail, not the window's edge.
        let inner = |id: &str| {
            egui::containers::panel::PanelState::load(&ctx, egui::Id::new(id))
                .expect("the panel beside")
                .outer_rect
                .left()
        };
        let stop = match (module, panel) {
            (Module::Develop, "presets") => inner("adjustments"),
            (Module::Develop, "adjustments") => inner("develop-rail"),
            _ => outward,
        };
        let laid_out = (edge(&ctx) - stop).abs();
        assert!(
            (laid_out - min).abs() <= 1.,
            "{panel}: laid out {laid_out}, painted {min}"
        );
        frame(&mut e, vec![button(to, false)]);
        for _ in 0..2 {
            frame(&mut e, vec![]);
        }
    }
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
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx)?));
    let library = editor.library.as_mut().unwrap();
    let a = library.session.photos[0].id;
    let folders = library.session.folders.iter().map(|f| f.id).collect();
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
#[test]
fn shift_cmd_c_and_v_reach_develop_though_winit_reads_them_as_the_clipboard() {
    use eframe::App as _;
    let ctx = egui::Context::default();
    let (mut editor, _) =
        editor_with_blue_photo(&ctx, crate::app::session::Session::default(), true);
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
            if dialog.purpose == settings_transfer::Transfer::Copy),
        "Shift+Cmd+C opens Copy Settings"
    );
    // The C, with its modifiers gone by the frame's end, is not Crop's.
    assert!(editor.view.tool == state::Tool::None);
    editor.close_copy_dialog(settings_transfer::CopyChoice::Confirm);
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
/// The accessibility tree of `editor`'s Develop controls in a frame given `events`.
fn controls_frame(
    ctx: &egui::Context,
    editor: &mut Editor,
    events: Vec<egui::Event>,
) -> egui::accesskit::TreeUpdate {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(320., 1600.))),
            events,
            ..Default::default()
        },
        |ui| {
            let frame = editor.begin_edit_frame();
            egui::CentralPanel::default().show(ui, |ui| editor.controls(ui));
            editor.finish_edit_frame(frame, ui.ctx());
        },
    );
    output.textures_delta.clear();
    output
        .platform_output
        .accesskit_update
        .expect("accessibility is on")
}
fn press_and_release(at: Pos2) -> [Vec<egui::Event>; 2] {
    [true, false].map(|pressed| {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    })
}
fn centre(node: &egui::accesskit::Node) -> Pos2 {
    let b = node.bounds().expect("placed");
    Pos2::new(((b.x0 + b.x1) / 2.) as f32, ((b.y0 + b.y1) / 2.) as f32)
}
/// A raw photo's Profile menu offers Import .cube LUTs…, which opens the window at
/// sRGB. Choosing Rec. 2020, the last of Create Profile's five spaces, and Choose
/// Files… asks for the file chooser for Rec. 2020 LUTs; Cancel asks for nothing; the
/// window opens at sRGB again.
#[test]
fn import_cube_luts_asks_the_space_before_the_files() {
    use egui::accesskit::Role;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let m = Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        width: 64,
        height: 48,
        ..Default::default()
    };
    let (profiles, _) = crate::camera_profiles::installed(&m);
    editor.document.reset(None);
    let (generation, _) = editor.load.start();
    for event in [
        Event::Header(Box::new(worker::LoadedHeader {
            id: generation,
            path: "photo.dng".into(),
            metadata: m,
            recipe: Recipe::default(),
            export: Default::default(),
            status: "Original".into(),
        })),
        Event::Profiles {
            id: generation,
            profiles,
            errors: Vec::new(),
            groups: Default::default(),
        },
    ] {
        editor.tx.send(event).unwrap();
    }
    editor.events(&ctx);
    controls_frame(&ctx, &mut editor, vec![]);
    let update = controls_frame(&ctx, &mut editor, vec![]);
    let profile = update
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == Role::ComboBox && n.value() == Some("RAWmakase Color"))
        .cloned()
        .expect("the Profile menu");
    let [down, up] = press_and_release(centre(&profile));
    controls_frame(&ctx, &mut editor, down);
    controls_frame(&ctx, &mut editor, up);
    let menu = controls_frame(&ctx, &mut editor, vec![]);
    let node = |update: &egui::accesskit::TreeUpdate, label: &str| {
        update
            .nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(label) || n.value() == Some(label))
            .cloned()
            .unwrap_or_else(|| panic!("no {label}"))
    };
    let item = node(&menu, "Import .cube LUTs…");
    let [down, up] = press_and_release(centre(&item));
    controls_frame(&ctx, &mut editor, down);
    controls_frame(&ctx, &mut editor, up);
    assert_eq!(
        editor.lut_import,
        Some(crate::camera_profiles::LutSpace::Srgb)
    );

    // The window on its own, as the workspace draws it over everything.
    let frame = |editor: &mut Editor, events| {
        let mut asked = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800., 600.))),
                events,
                ..Default::default()
            },
            |ui| asked = editor.lut_import_ui(ui.ctx()),
        );
        output.textures_delta.clear();
        let update = output.platform_output.accesskit_update.expect("on");
        (asked, update)
    };
    let click = |editor: &mut Editor, label: &str| {
        let (_, shown) = frame(editor, vec![]);
        let [down, up] = press_and_release(centre(&node(&shown, label)));
        let (first, _) = frame(editor, down);
        let (second, _) = frame(editor, up);
        first.or(second)
    };
    frame(&mut editor, vec![]);
    assert_eq!(click(&mut editor, "sRGB"), None);
    assert_eq!(click(&mut editor, "Rec. 2020"), None);
    assert_eq!(
        editor.lut_import,
        Some(crate::camera_profiles::LutSpace::Rec2020)
    );
    assert_eq!(
        click(&mut editor, "Choose Files…"),
        Some(dialogs::FileDialog::Luts(
            crate::camera_profiles::LutSpace::Rec2020
        ))
    );
    assert_eq!(editor.lut_import, None);
    // Again: sRGB, whatever was chosen before; Cancel asks for nothing.
    editor.open_lut_import();
    let (_, shown) = frame(&mut editor, vec![]);
    node(&shown, "sRGB");
    assert_eq!(click(&mut editor, "Cancel"), None);
    assert_eq!(editor.lut_import, None);
    let (asked, _) = frame(&mut editor, vec![]);
    assert_eq!(asked, None);
}
#[test]
fn the_basic_panel_offers_relative_white_balance_for_a_jpeg() {
    use egui::accesskit::Role;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let m = Metadata {
        source: crate::camera_data::Source::Rendered,
        width: 64,
        height: 48,
        ..Default::default()
    };
    let (profiles, _) = crate::camera_profiles::installed(&m);
    editor.document.reset(None);
    let (generation, _) = editor.load.start();
    for event in [
        Event::Header(Box::new(worker::LoadedHeader {
            id: generation,
            path: "photo.jpg".into(),
            metadata: m.clone(),
            recipe: Recipe::for_rendered(&m),
            export: Default::default(),
            status: "Original".into(),
        })),
        Event::Profiles {
            id: generation,
            profiles,
            errors: Vec::new(),
            groups: Default::default(),
        },
    ] {
        editor.tx.send(event).unwrap();
    }
    editor.events(&ctx);
    controls_frame(&ctx, &mut editor, vec![]);
    let update = controls_frame(&ctx, &mut editor, vec![]);
    let nodes = || update.nodes.iter().map(|(_, node)| node);
    let combo = |value: &str| {
        nodes()
            .find(|n| n.role() == Role::ComboBox && n.value() == Some(value))
            .cloned()
            .unwrap_or_else(|| panic!("no {value} menu"))
    };
    let white_balance = combo("As Shot");
    let below = centre(&white_balance).y;
    // The first slider under the WB menu is relative Temp, then Tint: ±100 each.
    let mut numbers: Vec<_> = nodes()
        .filter(|n| n.role() == Role::SpinButton && centre(n).y > below)
        .collect();
    numbers.sort_by(|a, b| centre(a).y.total_cmp(&centre(b).y));
    for number in &numbers[..2] {
        assert_eq!(
            (number.min_numeric_value(), number.max_numeric_value()),
            (Some(-100.), Some(100.))
        );
    }
    // The profile is the file's own; nothing to import.
    let profile = combo("Embedded");
    let [down, up] = press_and_release(centre(&profile));
    controls_frame(&ctx, &mut editor, down);
    controls_frame(&ctx, &mut editor, up);
    let menu = controls_frame(&ctx, &mut editor, vec![]);
    let labels: Vec<String> = menu
        .nodes
        .iter()
        .filter_map(|(_, n)| n.label().or(n.value()).map(String::from))
        .collect();
    let count = |labels: &[String], label: &str| labels.iter().filter(|l| *l == label).count();
    // Open: the menu lists the profile beside the box showing it.
    assert_eq!(count(&labels, "Embedded"), 2, "{labels:?}");
    for absent in [
        "Default (camera matrix)",
        "Import Adobe profiles for this camera",
        "Import profiles…",
        "Import profiles from folder…",
        "Import .cube LUTs…",
    ] {
        assert_eq!(count(&labels, absent), 0, "{absent}: {labels:?}");
    }
    // Close it, then open the WB menu: no Kelvin presets.
    controls_frame(
        &ctx,
        &mut editor,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let [down, up] = press_and_release(centre(&white_balance));
    controls_frame(&ctx, &mut editor, down);
    controls_frame(&ctx, &mut editor, up);
    let menu = controls_frame(&ctx, &mut editor, vec![]);
    let labels: Vec<String> = menu
        .nodes
        .iter()
        .filter_map(|(_, n)| n.label().or(n.value()).map(String::from))
        .collect();
    // The box shows As Shot, and the menu offers it with Auto and Custom.
    assert_eq!(count(&labels, "As Shot"), 2, "{labels:?}");
    for offered in ["Auto", "Custom"] {
        assert_eq!(count(&labels, offered), 1, "{offered}: {labels:?}");
    }
    assert_eq!(count(&labels, "Daylight"), 0, "{labels:?}");
}
#[test]
fn opening_a_jpeg_says_it_is_reading_the_photo() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("photo.jpg");
    image::RgbImage::new(8, 8).save(&file)?;
    assert!(editor.load_raw(file, None));
    assert_eq!(editor.messages.line(), "Reading photo…");
    Ok(())
}
#[test]
fn auto_white_balance_on_a_jpeg_sets_its_relative_temp_and_tint() {
    let ctx = egui::Context::default();
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let (width, height) = (64u32, 48u32);
    let metadata = Metadata {
        source: crate::camera_data::Source::Rendered,
        width,
        height,
        ..Default::default()
    };
    // A warm gradient, which Auto cools.
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: (0..width * height)
            .map(|i| {
                let v = 0.05 + 0.5 * (i % width) as f32 / width as f32;
                [v * 1.2, v, v * 0.8]
            })
            .collect(),
        metadata: metadata.clone(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    *editor.document.edit.setup_mut() = Recipe::for_rendered(&metadata);
    editor.start_auto(worker::AutoKind::WhiteBalance);
    let start = std::time::Instant::now();
    while editor.document.auto.is_running() {
        assert!(start.elapsed().as_secs() < 60, "Auto did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
        editor.events(&ctx);
    }
    let r = editor.document.edit.recipe();
    assert!(
        r.incremental_temperature < 0.,
        "{}",
        r.incremental_temperature
    );
    assert_eq!((r.temperature, r.tint), (6500., 0.));
    // The sliders carry it (the gains stay 1), and the WB menu shows Auto.
    assert_eq!(r.wb, [1.; 3]);
    assert_eq!(
        r.auto_white_balance,
        Some([r.incremental_temperature, r.incremental_tint])
    );
}
#[test]
fn a_tools_drawer_keeps_the_adjustments_width() {
    let ctx = egui::Context::default();
    super::icons::install(&ctx);
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let width = |e: &mut Editor| {
        for _ in 0..3 {
            panel_frame(e, vec![], FocusedButton::No);
        }
        egui::containers::panel::PanelState::load(&ctx, egui::Id::new("adjustments"))
            .unwrap()
            .outer_rect
            .width()
    };
    let edit = width(&mut e);
    for tool in [
        state::Tool::Crop,
        state::Tool::Remove,
        state::Tool::RedEye,
        state::Tool::Mask,
    ] {
        e.view.tool = tool;
        assert_eq!(width(&mut e), edit, "{tool:?} widened the panel");
        e.view.tool = state::Tool::None;
        assert_eq!(width(&mut e), edit, "after {tool:?}");
    }
}
#[test]
fn edit_on_the_rail_folds_the_adjustments_and_brings_them_back() {
    use super::panels::WorkspacePanel;
    use super::side_tabs::edit_id;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.module = Module::Develop;
    e.onboarding.visible = false;
    let drawn = |ctx: &egui::Context| {
        ctx.read_response(egui::Id::new("adjustments").with("__resize"))
            .is_some()
    };
    let click = |e: &mut Editor| {
        let at = ctx.read_response(edit_id()).expect("Edit").rect.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        panel_frame(e, vec![egui::Event::PointerMoved(at)], FocusedButton::No);
        panel_frame(e, vec![button(true)], FocusedButton::No);
        panel_frame(e, vec![button(false)], FocusedButton::No);
        panel_frame(e, vec![], FocusedButton::No);
    };
    for _ in 0..2 {
        panel_frame(&mut e, vec![], FocusedButton::No);
    }
    assert!(drawn(&ctx));
    // Edit, already showing, folds the adjustments; the rail stays.
    click(&mut e);
    assert!(!drawn(&ctx));
    assert!(!e.panel_shown(WorkspacePanel::Adjustments));
    assert!(e.panel_shown(WorkspacePanel::Right));
    // A tool's drawer shows while they are folded.
    e.view.tool = state::Tool::Crop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(drawn(&ctx));
    // Closing it folds them again; Edit from it brings the adjustments back.
    e.view.tool = state::Tool::None;
    panel_frame(&mut e, vec![], FocusedButton::No);
    panel_frame(&mut e, vec![], FocusedButton::No);
    assert!(!drawn(&ctx));
    e.view.tool = state::Tool::Crop;
    panel_frame(&mut e, vec![], FocusedButton::No);
    click(&mut e);
    assert_eq!(e.view.tool, state::Tool::None);
    assert!(drawn(&ctx));
    assert!(e.panel_shown(WorkspacePanel::Adjustments));
    // Tab leaves the fold alone: it is the rail's.
    click(&mut e);
    let tab = || key_down(egui::Key::Tab, egui::Modifiers::NONE);
    panel_frame(&mut e, tab(), FocusedButton::No);
    panel_frame(&mut e, tab(), FocusedButton::No);
    assert!(e.panel_shown(WorkspacePanel::Right));
    assert!(!e.panel_shown(WorkspacePanel::Adjustments));
}

/// The texts one frame of `editor` paints.
fn painted_texts(editor: &mut Editor, ctx: &egui::Context) -> Vec<String> {
    fn collect(shape: &egui::Shape, texts: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, texts)),
            _ => {}
        }
    }
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| editor.draw(ui));
    output.textures_delta.clear();
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut texts);
    }
    texts
}

/// A photo whose opening failed is not shown as still loading: no image is
/// coming, and the status line says why.
#[test]
fn a_photo_that_failed_to_open_is_not_shown_loading() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, Default::default(), None);
    editor.onboarding.visible = false;
    editor.module = Module::Develop;
    editor.document.reset(Some(PhotoId(1)));
    let (load_id, _) = editor.load.start();
    assert!(painted_texts(&mut editor, &ctx).contains(&"Loading photo…".to_string()));
    editor
        .tx
        .send(Event::Failed {
            id: load_id,
            task: worker::TaskKind::Load,
            error: "Could not read the WebP file: An expected chunk was missing".into(),
        })
        .unwrap();
    editor.events(&ctx);
    let texts = painted_texts(&mut editor, &ctx);
    assert!(!texts.contains(&"Loading photo…".to_string()), "{texts:?}");
    assert!(
        texts.contains(&"Couldn't open this photo".to_string()),
        "{texts:?}"
    );
    // Opening it again shows it loading again.
    editor.document.reset(Some(PhotoId(1)));
    editor.load.start();
    assert!(painted_texts(&mut editor, &ctx).contains(&"Loading photo…".to_string()));
}
/// A frame with the window maximized or not, at `time`: the session's value
/// afterwards, and whether the frame asked to maximize the window.
fn window_frame(
    e: &mut Editor,
    ctx: &egui::Context,
    time: f64,
    maximized: bool,
    minimized: bool,
) -> (bool, bool) {
    let mut input = egui::RawInput {
        time: Some(time),
        ..Default::default()
    };
    input.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            maximized: Some(maximized),
            minimized: Some(minimized),
            ..Default::default()
        },
    );
    let mut output = ctx.run_ui(input, |ui| e.remember_maximized(ui.ctx()));
    output.textures_delta.clear();
    let asked = output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .contains(&egui::ViewportCommand::Maximized(true));
    let saved: crate::app::session::Session = e
        .session_file
        .as_ref()
        .and_then(|file| std::fs::read(file).ok())
        .map_or_else(Default::default, |json| {
            serde_json::from_slice(&json).unwrap()
        });
    (saved.maximized, asked)
}
#[test]
fn the_window_reopens_maximized_as_it_was_left() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::session::Session::default(),
        Some(dir.path().join("session.json")),
    );
    let keeps = workspace::KEEPS_MAXIMIZED;
    assert_eq!(window_frame(&mut e, &ctx, 0., true, false), (keeps, false));
    // Minimized from maximized, Windows reports it not maximized.
    assert_eq!(window_frame(&mut e, &ctx, 1., false, true), (keeps, false));
    assert_eq!(window_frame(&mut e, &ctx, 2., true, false), (keeps, false));
    // Restored to its size.
    assert_eq!(window_frame(&mut e, &ctx, 3., false, false), (false, false));
    Ok(())
}
#[test]
fn a_window_reopened_maximized_keeps_that_until_the_window_manager_maximizes_it()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("session.json");
    let session = || crate::app::session::Session {
        maximized: true,
        ..Default::default()
    };
    crate::storage::atomic_json(&file, &session())?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, session(), Some(file.clone()));
    let keeps = workspace::KEEPS_MAXIMIZED;
    // An X11 window manager maximizes the window only once it is shown: the first
    // frame asks again, and the frames before it is maximized don't count.
    assert_eq!(window_frame(&mut e, &ctx, 0., false, false), (true, keeps));
    assert_eq!(window_frame(&mut e, &ctx, 0.5, false, false), (true, false));
    assert_eq!(window_frame(&mut e, &ctx, 1., true, false), (true, false));
    assert_eq!(
        window_frame(&mut e, &ctx, 1.5, false, false),
        (!keeps, false)
    );

    // One that never maximizes it: after a while, the window is saved as it is.
    crate::storage::atomic_json(&file, &session())?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, session(), Some(file.clone()));
    assert_eq!(window_frame(&mut e, &ctx, 0., false, false), (true, keeps));
    assert_eq!(window_frame(&mut e, &ctx, 1.9, false, false), (true, false));
    assert_eq!(
        window_frame(&mut e, &ctx, 2.1, false, false),
        (!keeps, false)
    );

    // Maximized already (Windows, Wayland), or started minimized: nothing to ask.
    for minimized in [false, true] {
        crate::storage::atomic_json(&file, &session())?;
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, session(), Some(file.clone()));
        assert_eq!(
            window_frame(&mut e, &ctx, 0., true, minimized),
            (true, false)
        );
        assert_eq!(window_frame(&mut e, &ctx, 0.1, true, false), (true, false));
    }
    Ok(())
}

/// The Lensfun row fits the inspector at its usual width: before and after the
/// download, its text stays inside the panel, what it says wraps below it, and
/// nothing reaches past the panel's right edge (which would widen it).
#[test]
fn lensfun_row_stays_inside_a_narrow_panel() -> anyhow::Result<()> {
    const WIDTH: f32 = 320.;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let dir = tempfile::tempdir()?;
    let archive =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lensfun/good.tar.bz2");
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.lensfun =
        crate::app::lensfun::Lensfun::in_folder(dir.path().to_path_buf(), archive.clone());
    editor.document.metadata = Some(crate::lens::lensfun::fixture::photo());
    let frame = |editor: &mut Editor| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(WIDTH, 6000.))),
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                egui::CentralPanel::default().show(ui, |ui| editor.controls(ui));
                editor.finish_edit_frame(frame, ui.ctx());
            },
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .expect("accessibility is on")
    };
    let find = |tree: &egui::accesskit::TreeUpdate, text: &str| {
        let nodes: Vec<_> = tree
            .nodes
            .iter()
            .map(|(_, n)| n)
            .filter(|n| n.bounds().is_some())
            .collect();
        nodes
            .iter()
            .find(|n| {
                n.label().is_some_and(|l| l.starts_with(text))
                    || n.value().is_some_and(|l| l.starts_with(text))
            })
            .map(|n| (*n).clone())
            .unwrap_or_else(|| {
                let labels: Vec<_> = nodes
                    .iter()
                    .filter_map(|n| n.label().or(n.value()))
                    .collect();
                panic!("no {text:?} among {labels:?}")
            })
    };
    let inside = |node: &egui::accesskit::Node| {
        let b = node.bounds().expect("placed");
        assert!(
            b.x1 as f32 <= WIDTH + 0.5,
            "{:?} reaches {} of {WIDTH}",
            node.label().or(node.value()),
            b.x1
        );
        b
    };
    let tree = frame(&mut editor);
    let row = inside(&find(&tree, "Community lens profiles"));
    let download = inside(&find(&tree, "Download"));
    assert!(download.y0 >= row.y1, "the download is below the title");
    let terms = inside(&find(&tree, "Lensfun database by Lensfun contributors"));
    assert!(terms.y0 >= download.y1, "the terms are below the download");
    crate::lensfun_install::install(
        dir.path(),
        &crate::lensfun_install::Source::File(archive),
        &Default::default(),
    )?;
    frame(&mut editor);
    let tree = frame(&mut editor);
    let lens = inside(&find(&tree, "Canon EF 70-200mm f/2.8L IS II USM"));
    let gives = inside(&find(&tree, "Corrects "));
    assert!(gives.y0 >= lens.y1, "what it corrects is below the lens");
    let use_button = tree
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == egui::accesskit::Role::Button && n.label() == Some("Use"))
        .expect("the Use button");
    let button = inside(use_button);
    assert!(button.y0 >= gives.y1, "Use is below what it corrects");
    Ok(())
}

/// Where no imported profile fits, Lens Corrections offers Lensfun's download, which
/// only installs; installed, it offers the lens this photo matches, and Use chooses
/// it, with Enable Profile Corrections, as one History step.
#[test]
fn lensfun_is_offered_downloaded_and_used_in_one_step() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx.all_styles_mut(|s| s.animation_time = 0.);
    let dir = tempfile::tempdir()?;
    let archive =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lensfun/good.tar.bz2");
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.lensfun =
        crate::app::lensfun::Lensfun::in_folder(dir.path().to_path_buf(), archive.clone());
    editor.document.metadata = Some(crate::lens::lensfun::fixture::photo());
    // Tall and wide enough for the whole panel, Lens Corrections being far down.
    let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(480., 6000.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                egui::CentralPanel::default().show(ui, |ui| editor.controls(ui));
                editor.finish_edit_frame(frame, ui.ctx());
            },
        );
        output.textures_delta.clear();
        output
            .platform_output
            .accesskit_update
            .expect("accessibility is on")
    };
    let find = |tree: &egui::accesskit::TreeUpdate, text: &str| {
        tree.nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| {
                n.label().is_some_and(|l| l.contains(text))
                    || n.value().is_some_and(|v| v.contains(text))
            })
            .cloned()
    };
    let tree = frame(&mut editor, vec![]);
    assert!(find(&tree, "Community lens profiles").is_some());
    assert!(find(&tree, "Download · 0.5 MB").is_some());
    assert!(find(&tree, "CC BY-SA 3.0").is_some());
    let before = editor.document.edit.recipe().clone();
    // The download (an archive here) installs and changes no edit.
    crate::lensfun_install::install(
        dir.path(),
        &crate::lensfun_install::Source::File(archive),
        &Default::default(),
    )?;
    let tree = frame(&mut editor, vec![]);
    assert_eq!(*editor.document.edit.recipe(), before);
    assert!(find(&tree, "Canon EF 70-200mm f/2.8L IS II USM").is_some());
    assert!(find(&tree, "Corrects ").is_some());
    let steps = editor.document.edit.history().steps().0.len();
    let use_button = tree
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == egui::accesskit::Role::Button && n.label() == Some("Use"))
        .cloned()
        .expect("the Use button");
    let [down, up] = press_and_release(centre(&use_button));
    frame(&mut editor, down);
    frame(&mut editor, up);
    frame(&mut editor, vec![]);
    let r = editor.document.edit.recipe();
    assert!(r.lens_profile);
    let chosen = r.lens_profile_choice.lensfun().expect("a Lensfun lens");
    assert!(chosen.is_resolved());
    assert_eq!(chosen.lens.model, "Canon EF 70-200mm f/2.8L IS II USM");
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    assert!(
        r.lens_correction(editor.document.metadata.as_ref().unwrap())
            .unwrap()
            .source
            .starts_with("Lensfun")
    );
    // The menus show it as chosen.
    let tree = frame(&mut editor, vec![]);
    assert!(find(&tree, "Lensfun").is_some());
    // An edit naming the lens only, as read from XMP, resolves on request, as one
    // step.
    let named = editor
        .document
        .edit
        .recipe()
        .lens_profile_choice
        .lensfun()
        .unwrap()
        .clone();
    editor
        .document
        .edit
        .setup_mut()
        .lens_profile_choice
        .choose_lensfun(crate::lens::lensfun::LensfunChoice::unresolved(
            named.lens.clone(),
            named.label.clone(),
            0,
        ));
    let tree = frame(&mut editor, vec![]);
    assert!(find(&tree, "isn't resolved").is_some());
    let steps = editor.document.edit.history().steps().0.len();
    let resolve = tree
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == egui::accesskit::Role::Button && n.label() == Some("Resolve"))
        .cloned()
        .expect("the Resolve button");
    let [down, up] = press_and_release(centre(&resolve));
    frame(&mut editor, down);
    frame(&mut editor, up);
    frame(&mut editor, vec![]);
    let chosen = editor
        .document
        .edit
        .recipe()
        .lens_profile_choice
        .lensfun()
        .unwrap()
        .clone();
    assert!(chosen.is_resolved());
    assert_eq!(chosen.calibration, named.calibration);
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    Ok(())
}

/// A failure said while the Library's own message was showing used to be
/// hidden by it, and either was gone with the next progress.
#[test]
fn a_failed_action_stays_on_screen_past_the_librarys_messages_and_progress() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.onboarding.visible = false;
    e.messages
        .fail("No camera look fitted: The preview does not line up");
    e.messages.note("Reading RAW…");
    for time in [0., 3600.] {
        let input = egui::RawInput {
            time: Some(time),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| e.draw(ui));
        output.textures_delta.clear();
    }
    assert_eq!(e.messages.line(), "Reading RAW…");
    assert!(
        e.messages
            .has_entry("No camera look fitted: The preview does not line up")
    );
}

/// Profile → Fit Camera Look: a refusal (or a panic, which arrives as one) says why
/// and leaves nothing running; a superseded fit is dropped; the fit is one History
/// step Undo takes back; a profile chosen while it ran wins over it.
#[test]
fn a_fitted_camera_look_is_one_step_and_yields_to_later_choices() {
    use crate::camera_profiles::fitted;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    let m = crate::camera_data::Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..Default::default()
    };
    let fit = fitted::with_look(
        &fitted::standard_base(&m).unwrap(),
        &fitted::FittedLook::identity(),
    )
    .unwrap();
    e.document.metadata = Some(m.clone());
    let before = e.document.edit.recipe().profile.clone();
    let (g, _) = e.document.camera_look.start();
    e.camera_look_ready(g, before.clone(), Err("Stopped unexpectedly: boom".into()));
    assert!(!e.document.camera_look.is_running());
    assert!(e.messages.has_entry_with("boom"));
    assert!(e.running_work().is_empty());
    assert_eq!(e.document.edit.history().steps().1, 0);
    // A fit asked for again: the first one's result no longer counts.
    let (first, _) = e.document.camera_look.start();
    let (second, _) = e.document.camera_look.start();
    e.camera_look_ready(first, before.clone(), Ok(Box::new(fit.clone())));
    assert_eq!(e.document.edit.recipe().profile, before);
    assert!(e.document.camera_look.is_running());
    assert_eq!(e.running_work()[0].key, "camera look");
    e.camera_look_ready(second, before.clone(), Ok(Box::new(fit.clone())));
    assert_eq!(e.document.edit.recipe().profile.as_deref(), Some(&fit));
    assert_eq!(e.document.edit.recipe().profile_amount, 1.);
    // Temp and Tint follow the new profile, as after a menu choice.
    let mut synced = e.document.edit.recipe().clone();
    synced.profile_changed(&m);
    assert_eq!(
        (synced.temperature, synced.tint, synced.wb),
        (
            e.document.edit.recipe().temperature,
            e.document.edit.recipe().tint,
            e.document.edit.recipe().wb
        )
    );
    let (steps, applied) = e.document.edit.history().steps();
    assert_eq!((applied, steps[0].name.as_str()), (1, "Fit Camera Look"));
    e.undo();
    assert_eq!(e.document.edit.recipe().profile, before);
    // Another profile chosen while it ran: that choice stays.
    let (g, _) = e.document.camera_look.start();
    let chosen = Some(std::sync::Arc::new(fitted::standard_base(&m).unwrap()));
    e.document.edit.setup_mut().profile = chosen.clone();
    e.camera_look_ready(g, before, Ok(Box::new(fit)));
    assert_eq!(e.document.edit.recipe().profile, chosen);
    assert!(e.messages.has_entry_with("changed"));
    assert!(!e.document.camera_look.is_running());
}

/// Listing a photo's profiles again after an import keeps the fitted default of a
/// photo still without colour data, and drops it once an imported profile fits.
#[test]
fn an_import_keeps_a_fitted_default_until_a_profile_fits() {
    use crate::camera_profiles::fitted;
    let m = crate::camera_data::Metadata {
        make: "Test".into(),
        model: "Bare".into(),
        ..Default::default()
    };
    let with_matrix = crate::camera_data::Metadata {
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..m.clone()
    };
    let fit = std::sync::Arc::new(
        fitted::with_look(
            &fitted::standard_base(&with_matrix).unwrap(),
            &fitted::FittedLook::identity(),
        )
        .unwrap(),
    );
    let had = vec![fit];
    assert_eq!(
        super::camera_look::keep_fitted_default(&had, &m, vec![]),
        had
    );
    let mut dcp = fitted::standard_base(&with_matrix).unwrap();
    dcp.name = "Mine".into();
    let imported = vec![std::sync::Arc::new(dcp)];
    assert_eq!(
        super::camera_look::keep_fitted_default(&had, &m, imported.clone()),
        imported
    );
}
#[test]
fn edited_previews_take_the_computer_only_with_the_library_in_front_and_nothing_else_rendering()
-> anyhow::Result<()> {
    use crate::app::library::Pace;
    let (_directory, mut editor, _) = editor_with_catalog(&["a.RAF"])?;
    editor.module = Module::Library;
    assert_eq!(editor.preview_pace(), Pace::Full);
    editor.module = Module::Develop;
    assert_eq!(editor.preview_pace(), Pace::Background);
    // An output a command started, still running in the Library.
    editor.module = Module::Library;
    editor.automation.hold_running_output();
    assert_eq!(editor.preview_pace(), Pace::Background);
    editor.automation.cancel_outputs();
    assert_eq!(editor.preview_pace(), Pace::Full);
    Ok(())
}
