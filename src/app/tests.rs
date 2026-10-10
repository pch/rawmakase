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
    for (key, expected) in [(egui::Key::Z, true), (egui::Key::F, false)] {
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
            |ui| editor.viewport_ui(ui),
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
    assert_eq!(saved.monitor, editor.view.monitor);
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
                editor.viewport_ui(ui)
            },
        );
        editor.finish_edit_frame(edit, &ctx);
        output.textures_delta.clear();
        let (steps, applied) = editor.document.edit.history().steps();
        let names: Vec<String> = steps[..applied].iter().map(|s| s.name.clone()).collect();
        (
            editor.document.edit.recipe().red_eye.clone(),
            names,
            editor.status.to_string(),
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
    assert!(editor.status.contains("Unable to find red eye"));
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
            |ui| editor.viewport_ui(ui),
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
        editor.status.contains("Unable to find red eye"),
        "{}",
        editor.status
    );
    editor.document.edit.recipe().validate().unwrap();
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
        editor.status.contains("Not added: locked"),
        "{}",
        editor.status
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
    };
    let other = crate::catalog::CatalogLocation::File(dir.path().join("other.rawmakase"));
    editor.remove_folders(&removal(other));
    assert_eq!(editor.library.as_ref().unwrap().session.photos.len(), 2);
    assert!(
        editor.status.contains("another catalog"),
        "{}",
        editor.status
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
    assert!(editor.status.contains("still on disk"), "{}", editor.status);
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
    assert!(!editor.status.contains("usable"), "{}", editor.status);
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
    assert!(e.status.starts_with("Undo 3 photos"));
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

#[test]
fn auto_straighten_sets_the_level_angle_as_one_step_and_measures_again_after_a_turn() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    // Stripes falling 3° to the right.
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
    e.document.set_image(Arc::new(CameraImage {
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
    }));
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
                    rect = ui.scope(|ui| e.tool_strip(ui)).response.rect;
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
    assert!(e.status.contains("two or more guides"), "{}", e.status);
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
    let pending = editor.preview.pending_recipe.as_ref().unwrap();
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
    assert_eq!(editor.status, SampleRefusal::AlreadySampled.message());
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
    let pending = editor.preview.pending_recipe.as_ref().unwrap();
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
    // The switch sits left of the reset button, in the 28-point header 8 points down.
    let area = area.get();
    let switch = Pos2::new(area.right() - 28. - 18., area.top() + 8. + 14.);
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
        let mut photo = BatchPhoto::from_record(record, path.clone(), name);
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
    let status = editor.status.clone();
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
fn the_library_info_panel_stays_while_a_field_in_it_cannot_be_saved() -> anyhow::Result<()> {
    use super::panels::{PanelChange, WorkspacePanel};
    let (_d, mut e, ids) = editor_with_catalog(&["a.ARW"])?;
    let library = e.library.as_mut().unwrap();
    let copy = library.create_virtual_copy(ids[0])?.value;
    library.session.catalog.remove_virtual_copy(copy)?;
    library.set_copy_name_draft(copy, "B&W");
    assert!(!e.change_panels(PanelChange::Toggle(WorkspacePanel::Right)));
    assert!(e.panel_shown(WorkspacePanel::Right));
    assert!(e.status.starts_with("Not saved"));
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
    for module in [Module::Library, Module::Develop] {
        e.module = module;
        for _ in 0..3 {
            frame(&mut e, vec![]);
        }
        let panels = match module {
            Module::Library => ["library-sidebar", "library-info"],
            Module::Develop => ["presets", "adjustments"],
        };
        for panel in panels {
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

#[test]
fn a_zoom_chosen_in_the_librarys_navigator_opens_the_loupe_at_it() -> anyhow::Result<()> {
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
    assert!(!e.library.as_ref().unwrap().loupe_open());

    // Nothing selected: 100% opens the first photo shown, at 100%.
    e.library_zoom(navigator::Change::Level(1.));
    let library = e.library.as_ref().unwrap();
    assert!(library.loupe_open());
    assert!(library.selected().is_some());
    assert!(e.view.zoom.on);
    assert_eq!(e.view.zoom.level, 1.);

    // Fit from the Loupe stays in it, fitted.
    e.library_zoom(navigator::Change::Level(0.));
    assert!(e.library.as_ref().unwrap().loupe_open());
    assert!(!e.view.zoom.on);

    // A click in the preview zooms in on that spot.
    e.library_zoom(navigator::Change::Inspect([0.25, 0.75]));
    assert!(e.view.zoom.on);
    assert_eq!(e.view.zoom.pan, [0.25, 0.75]);
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
        (Module::Develop, "presets", 0., PRESETS_MIN),
        (Module::Develop, "adjustments", 1200., ADJUSTMENTS_MIN),
        (Module::Library, "library-sidebar", 0., LIBRARY_SIDEBAR_MIN),
        (Module::Library, "library-info", 1200., LIBRARY_INFO_MIN),
    ] {
        e.module = module;
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
        let laid_out = (edge(&ctx) - outward).abs();
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
    });
    editor.sync_undo();
    // Its id may be the next photo's: nothing may undo onto it.
    assert!(!editor.undo_log.can_undo());
    Ok(())
}
