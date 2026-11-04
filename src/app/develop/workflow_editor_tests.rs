//! Editor tests of `workflow`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use eframe::egui::{Pos2, Rect};

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
            .catalog()
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
fn the_prefetched_neighbour_follows_the_direction_of_travel() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    for name in ["a.ARW", "b.ARW", "c.ARW"] {
        std::fs::write(dir.path().join(name), name)?;
    }
    let path = dir.path().join("photos.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(dir.path())?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let library = library::Library::load(&path, ctx)?;
    // The filmstrip order, first to last.
    let mut order = vec![library.session.photos()[0].id];
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
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let library = library::Library::load(&path, ctx)?;
    let photos: Vec<_> = library.session.photos().iter().map(|p| p.id).collect();
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
fn opening_a_jpeg_says_it_is_reading_the_photo() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("photo.jpg");
    image::RgbImage::new(8, 8).save(&file)?;
    assert!(editor.load_raw(file, None));
    assert_eq!(editor.messages.line(), "Reading photo…");
    Ok(())
}

/// Closing with an edit that can't be checked against its file, as its identity
/// could not be read when it opened, is a failed save, not a saved one.
#[test]
fn closing_with_an_unidentified_file_is_a_failed_save() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.path = Some(d.path().join("photos/a.RAF"));
    assert!(e.document.file.is_none());
    let original = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 1.;
    e.commit_edit(original, None);
    assert_eq!(
        e.flush_by(std::time::Instant::now() + std::time::Duration::from_secs(1)),
        crate::app::develop::workflow::Flushed::Failed
    );
    assert!(e.document.edit.save_state().needs_save());
    assert!(!e.flush());
    let library = e.library.as_ref().unwrap();
    assert!(
        library
            .session
            .catalog()
            .edit_record(ids[0])?
            .saved(&d.path().join("photos/a.RAF"))?
            .is_none()
    );
    Ok(())
}
