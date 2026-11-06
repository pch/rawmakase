//! Editor tests of `events`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use crate::{catalog::PhotoId, develop};
use std::sync::Arc;

#[test]
fn stale_preview_results_are_discarded() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
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
    let (mut e, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
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
    let (mut e, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    task::wait_for(vec![e.renderer.stop()], std::time::Duration::from_secs(5));
    let (taken_tx, taken) = std::sync::mpsc::channel();
    let (_release, release) = std::sync::mpsc::channel();
    e.renderer = worker::holding(taken_tx, release);
    let library = library::Library::load(&path, ctx.clone())?;
    let photo = library.session.photos()[0].id;
    e.library = Some(Box::new(library));
    e.document.catalog_photo = Some(photo);
    e.document.opened_at(photos.join("a.RAF"));
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

/// A Develop render with AI Denoise becomes the Library's preview only once the
/// result is complete, and under its own tag: a plain request of the same edit
/// (no result cached for the Library) never gets it, nor a partial result.
#[test]
fn develop_renders_with_ai_denoise_are_kept_apart_from_plain_previews() -> anyhow::Result<()> {
    use crate::ai_denoise::{State, Update};
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.RAF"), b"fixture")?;
    let path = d.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let (mut e, image) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    task::wait_for(vec![e.renderer.stop()], std::time::Duration::from_secs(5));
    let (taken_tx, _taken) = std::sync::mpsc::channel();
    let (_release, release) = std::sync::mpsc::channel();
    e.renderer = worker::holding(taken_tx, release);
    let mut library = library::Library::load(&path, ctx.clone())?;
    let sent = library.detach_edited_previews();
    let photo = library.session.photos()[0].id;
    e.library = Some(Box::new(library));
    e.document.catalog_photo = Some(photo);
    e.document.opened_at(photos.join("a.RAF"));
    e.document.image_file = e.document.file.clone();
    e.view.viewport = Vec2::new(40., 40.);
    in_edit_frame(&ctx, &mut e, |e| {
        e.document.edit.recipe_mut().switch_ai_denoise(true)
    });
    let source = develop::AiSource::recover((*image).clone(), rawmakase_cancel::Cancel::NEVER)?;
    let render = |e: &mut Editor| {
        e.schedule();
        let id = e.preview.task.id();
        e.tx.send(Event::Rendered {
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
            status: "rendered".into(),
        })
        .unwrap();
        e.events(&ctx);
    };
    e.ai_denoise_update(Update {
        plain: Arc::downgrade(&image),
        state: State::Partial(source.clone()),
    });
    render(&mut e);
    assert!(
        sent.stored_tags().is_empty(),
        "a partial result is not kept"
    );
    e.ai_denoise_update(Update {
        plain: Arc::downgrade(&image),
        state: State::Ready(source),
    });
    render(&mut e);
    let json = serde_json::to_string(e.document.edit.recipe())?;
    let source = library::EditSource::Recipe(json);
    let stored = sent.stored_tags();
    assert_eq!(stored.len(), 1, "{stored:?}");
    assert_ne!(
        stored[0],
        source.preview_tag(library::PreviewAi::Plain),
        "kept as the plain preview"
    );
    assert_eq!(stored[0], source.preview_tag(library::PreviewAi::Finished));
    Ok(())
}

#[test]
fn before_and_after_renders_go_to_their_own_side() {
    use worker::Pane;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
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
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
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
        hdr: Default::default(),
    }));
    e.view.viewport = Vec2::new(40., 40.);
    e.set_compare(crate::app::develop::before_after::Compare::SideBySide(
        crate::app::develop::before_after::Axis::LeftRight,
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
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
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
        hdr: Default::default(),
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
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
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
        hdr: Default::default(),
    }));
    e.view.viewport = Vec2::new(40., 40.);
    e.set_compare(crate::app::develop::before_after::Compare::Split(
        crate::app::develop::before_after::Axis::LeftRight,
    ));
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
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let (load_id, _) = editor.document.start_load();
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
    assert!(editor.document.loading());
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
    assert!(!editor.document.loading());
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
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    editor.document.catalog_photo = Some(id);
    let opened = crate::storage::Identity::read(&raw)?;
    std::fs::write(&raw, b"replaced before the header arrived")?;
    let (generation, _) = editor.document.start_load();
    let recipe = Recipe {
        exposure: 0.75,
        ..Default::default()
    };
    editor
        .tx
        .send(Event::Header(Box::new(LoadedHeader {
            id: generation,
            file: Some(opened.clone()),
            path: raw,
            metadata: Metadata::default(),
            recipe: recipe.clone(),
            export: Default::default(),
            status: "Original".into(),
        })))
        .unwrap();
    editor.events(&ctx);
    assert_eq!(*editor.document.edit.recipe(), recipe);
    // The file as the loader read it before opening it, which saves are checked
    // against, not as it is when the header arrives.
    assert_eq!(editor.document.file, Some(opened));
    Ok(())
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
    let (load_id, _) = editor.document.start_load();
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
    editor.document.start_load();
    assert!(painted_texts(&mut editor, &ctx).contains(&"Loading photo…".to_string()));
}

/// Panning at 100% or more renders the region panned to on every frame. On a GPU
/// slower than the frames, cancelling the running region showed no sharp pixels
/// until the drag stopped, and the photo jumped between blurry and sharp; the
/// running region now finishes and shows where it was rendered.
#[test]
fn a_region_overtaken_by_a_pan_still_shows_where_it_was_rendered() {
    use crate::app::develop::state::TextureMode;
    let ctx = egui::Context::default();
    let (mut e, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    task::wait_for(vec![e.renderer.stop()], std::time::Duration::from_secs(5));
    let (taken_tx, taken) = std::sync::mpsc::channel();
    let (_release, release) = std::sync::mpsc::channel();
    e.renderer = worker::holding(taken_tx, release);
    e.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([32, 24], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    e.view.zoom.on = true;
    e.view.zoom.level = 4.;
    let frame = |e: &mut Editor| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(40., 40.),
                )),
                ..Default::default()
            },
            |ui| {
                e.viewport_ui(ui);
            },
        )
        .textures_delta
        .clear();
    };
    frame(&mut e);
    let first = e.preview.task.id();
    let first_region = e.region().expect("zoomed in to a region");
    assert_eq!(
        taken.recv_timeout(std::time::Duration::from_secs(5)),
        Ok(first)
    );
    // Dragged on while the first region still renders, past its margin.
    for i in 1..=5 {
        e.view.zoom.pan[0] = 0.5 + i as f32 / 25.;
        frame(&mut e);
    }
    let last = e.preview.task.id();
    assert!(last > first);
    assert_ne!(e.region(), Some(first_region));
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
        stage: worker::RenderStage::Region,
        status: status.into(),
    };
    e.tx.send(rendered(first, "first")).unwrap();
    e.events(&ctx);
    assert_eq!(e.preview.status, "first");
    assert_eq!(e.preview.mode, TextureMode::Region(first_region));
    assert!(e.preview.task.is_running());
    e.tx.send(rendered(last, "last")).unwrap();
    e.events(&ctx);
    assert_eq!(e.preview.status, "last");
    assert_eq!(e.preview.mode, TextureMode::Region(e.region().unwrap()));
    assert!(!e.preview.task.is_running());
    // A new zoom level still cancels the region running.
    e.view.zoom.pan[0] = 0.4;
    frame(&mut e);
    let panned = e.preview.task.id();
    assert!(panned > last && e.preview.task.is_running());
    e.view.zoom.level = 8.;
    frame(&mut e);
    e.tx.send(rendered(panned, "another level")).unwrap();
    e.events(&ctx);
    assert_eq!(e.preview.status, "last");
}
