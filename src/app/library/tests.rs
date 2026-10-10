use super::cell::photo_cell;
use super::filter::{Kind, Label, RatingOp};
use super::tree::{FolderNode, TreeAction, folder_tree_row};
use super::*;
use crate::catalog::{Folder, PhotoId};
use eframe::egui::{Color32, Vec2};
use std::{collections::HashMap, path::PathBuf};
#[test]
fn develop_workspace_drains_library_preview_results() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("previews.rawmakase");
    drop(Catalog::create(&path)?);
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    let (tx, rx) = std::sync::mpsc::sync_channel(24);
    library.cache.thumb_rx = rx;
    for index in 0..24 {
        let path = directory.path().join(format!("{index}.ARW"));
        library.cache.pending.insert(path.clone());
        library.cache.progress.queued();
        tx.try_send(previews::PreviewResult {
            path,
            image: Some(image::RgbImage::new(16, 16)),
            cache_error: None,
        })?;
    }
    let mut editor = crate::app::Editor::with_context(&ctx, None, Default::default(), None);
    editor.library = Some(Box::new(library));
    editor.module = Module::Develop;
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| editor.draw(ui));
    output.textures_delta.clear();
    let library = editor.library.as_ref().unwrap();
    assert!(library.cache.pending.is_empty());
    assert_eq!(library.cache.thumbs.len(), 24);
    assert!(editor.module == Module::Develop);
    Ok(())
}

#[test]
fn develop_says_why_it_cannot_open_a_photo() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::new(8, 8).save(folder.join("a.jpg"))?;
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let library = Library::load(&path, ctx.clone())?;
    let photo = library.session.photos[0].clone();
    assert_eq!(
        develop_refusal(&photo, true),
        Some(Refusal::NotRaw("JPG".into()))
    );
    assert_eq!(develop_refusal(&photo, false), Some(Refusal::Offline));
    assert_eq!(Refusal::NotRaw("JPG".into()).label(), "JPG file");
    let mut editor = crate::app::Editor::with_context(&ctx, None, Default::default(), None);
    editor.library = Some(Box::new(library));
    editor.module = Module::Library;
    editor.develop_catalog_photo(photo.id);
    assert!(editor.module == Module::Library);
    let (title, reason) = editor.not_editable.clone().unwrap();
    assert_eq!(title, "a.jpg can't be opened in Develop");
    assert!(reason.contains("camera RAW"));
    // Dismissed, then the file goes offline.
    editor.not_editable = None;
    std::fs::remove_file(folder.join("a.jpg"))?;
    editor.develop_catalog_photo(photo.id);
    assert!(editor.not_editable.unwrap().1.contains("offline"));
    Ok(())
}

#[test]
fn a_file_found_again_is_checked_back_online() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    let file = folder.join("a.jpg");
    image::RgbImage::new(8, 8).save(&file)?;
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    // As the catalog stores it, which on Windows differs from `file`.
    let stored = library.session.photos[0].path.clone();
    std::fs::rename(&file, folder.join("moved"))?;
    library.refresh()?;
    library.wait_for_availability();
    assert!(!library.is_available(&stored));
    // Restored in place: counted offline until it is found again.
    std::fs::rename(folder.join("moved"), &file)?;
    assert!(!library.is_available(&stored));
    assert_eq!(library.available_count(), 0);
    library.found(&stored);
    // Counted online again while it is checked, as every photo is.
    assert_eq!(library.available_count(), 1);
    library.wait_for_availability();
    assert!(library.is_available(&stored));
    assert_eq!(library.available_count(), 1);
    Ok(())
}

#[test]
fn photo_cells_preserve_texture_proportions_at_different_grid_widths() {
    let ctx = egui::Context::default();
    let photo = Photo {
        id: PhotoId(1),
        folder: FolderId(1),
        path: "photo.RAF".into(),
        filename: "photo.RAF".into(),
        captured: String::new(),
        rating: 0,
        flag: 0,
        label: String::new(),
        format: "RAF".into(),
        copy_name: String::new(),
        master: None,
        keywords: String::new(),
        has_lightroom_edits: false,
    };
    for size in [[360, 240], [160, 240], [240, 240], [360, 90]] {
        let texture = ctx.load_texture(
            "aspect-test",
            egui::ColorImage::filled(size, Color32::WHITE),
            Default::default(),
        );
        for width in [80., 190., 260.] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let shown = cell::Shown {
                    mark: selection::Mark::None,
                    number: 1,
                    available: true,
                    quick: false,
                    style: cell::Style::Compact,
                    details: String::new(),
                };
                photo_cell(ui, &photo, Some(&texture), shown, width);
            });
            let mesh = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Mesh(mesh) if mesh.texture_id == texture.id() => {
                        Some(mesh)
                    }
                    _ => None,
                })
                .expect("photo must be painted");
            let bounds = mesh.calc_bounds();
            let expected = size[0] as f32 / size[1] as f32;
            assert!((bounds.width() / bounds.height() - expected).abs() < 0.0001);
            assert!(bounds.width() <= width * 0.84 + 0.001);
            assert!(bounds.height() <= width - 20.);
            assert_eq!(mesh.vertices[0].uv, egui::Pos2::ZERO);
            assert_eq!(mesh.vertices[3].uv, egui::pos2(1., 1.));
            output.textures_delta.clear();
        }
    }
}
#[test]
fn metadata_edits_persist_toggle_and_advance_through_filtered_photos() -> Result<()> {
    use crate::app::photo_metadata::Edit;
    let d = tempfile::tempdir()?;
    let folder = d.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["a.RAF", "b.RAF", "c.RAF"] {
        std::fs::write(folder.join(name), b"fixture")?;
    }
    let path = d.path().join("metadata.rawmakase");
    let mut catalog = Catalog::create(&path)?;
    catalog.add_folder(&folder)?;
    drop(catalog);
    let mut library = Library::load(&path, egui::Context::default())?;
    let ids: Vec<_> = library.session.photos.iter().map(|p| p.id).collect();
    library.select(Some(ids[0]));
    library.edit_metadata(ids[0], Edit::Rating(5), false)?;
    library.edit_metadata(ids[0], Edit::Flag(1), false)?;
    library.edit_metadata(ids[0], Edit::Label("Client approved".into()), false)?;
    library.edit_metadata(ids[0], Edit::RatingDelta(1), false)?;
    assert_eq!(library.photo(ids[0]).unwrap().rating, 5);
    assert_eq!(library.photo(ids[0]).unwrap().label, "Client approved");
    assert!(library.labels().contains(&"Client approved".into()));
    library.edit_metadata(ids[0], Edit::ToggleLabel("Red".into()), false)?;
    library.edit_metadata(ids[0], Edit::ToggleLabel("Red".into()), false)?;
    assert_eq!(library.photo(ids[0]).unwrap().label, "");
    library.filters.flags = [0].into();
    library.filter();
    library.select(Some(ids[1]));
    assert_eq!(
        library.edit_metadata(ids[1], Edit::Flag(-1), true)?,
        Some(ids[2])
    );
    assert_eq!(library.selected(), Some(ids[2]));
    assert_eq!(library.visible.len(), 1);
    assert_eq!(library.edit_metadata(ids[2], Edit::Flag(1), true)?, None);
    assert_eq!(library.selected(), None);
    assert!(library.visible.is_empty());
    library.filters.flags.clear();
    library.filters.labels = [Label::Color("Purple".into())].into();
    library.filter();
    library.edit_metadata(ids[2], Edit::Label("Purple".into()), false)?;
    assert_eq!(library.visible.len(), 1);
    assert!(
        library
            .edit_metadata(ids[2], Edit::Rating(10), false)
            .is_err()
    );
    assert_eq!(library.photo(ids[2]).unwrap().rating, 0);
    library.refresh()?;
    assert_eq!(library.photo(ids[0]).unwrap().rating, 5);
    assert_eq!(library.photo(ids[1]).unwrap().flag, -1);
    assert_eq!(library.photo(ids[2]).unwrap().label, "Purple");
    assert!(std::fs::read_dir(&folder)?.all(|e| e.unwrap().path().extension().unwrap() == "RAF"));
    Ok(())
}
#[test]
fn tree_locate_action_uses_the_clicked_root() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let root = FolderNode::root(RootId(42), "Photos".into(), "/missing".into());
    let mut expanded = HashSet::new();
    let mut frame = |events: Vec<egui::Event>| {
        let mut action = None;
        let mut row = egui::Rect::NOTHING;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(360., 240.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let top = ui.cursor().top();
                action = folder_tree_row(ui, &root, 0, &mut expanded, "");
                row = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
            },
        );
        output.textures_delta.clear();
        (action, row, output.platform_output.accesskit_update)
    };
    let click = |at: egui::Pos2, button: egui::PointerButton, pressed: bool| {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    let (_, row, _) = frame(vec![]);
    // A click at the row's right end, where "…" once was, selects the root.
    let end = egui::pos2(row.right() - 12., row.center().y);
    frame(click(end, egui::PointerButton::Primary, true));
    let (action, _, _) = frame(click(end, egui::PointerButton::Primary, false));
    assert!(matches!(action, Some(TreeAction::Select(..))));
    // Locating it is in the right-click menu.
    frame(click(end, egui::PointerButton::Secondary, true));
    frame(click(end, egui::PointerButton::Secondary, false));
    let (_, _, update) = frame(vec![]);
    let update = update.expect("accessibility is on");
    let item = update
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Locate root folder…"))
        .and_then(|(_, node)| node.bounds())
        .expect("the menu offers Locate");
    let at = egui::pos2(
        ((item.x0 + item.x1) / 2.) as f32,
        ((item.y0 + item.y1) / 2.) as f32,
    );
    frame(click(at, egui::PointerButton::Primary, true));
    let (action, _, _) = frame(click(at, egui::PointerButton::Primary, false));
    assert!(matches!(action, Some(TreeAction::RelinkRoot(RootId(42)))));
}
#[test]
fn batched_availability_distinguishes_files_directories_and_missing_paths() -> Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("present.ARW"), b"raw")?;
    let mut c = Catalog::create(&d.path().join("catalog.rawmakase"))?;
    c.add_folder(&photos)?;
    let mut rows = c.photos()?;
    let mut missing = rows[0].clone();
    missing.path = photos.join("missing.RAF");
    rows.push(missing);
    let mut directory = rows[0].clone();
    directory.path = photos.join("directory.ARW");
    std::fs::create_dir(&directory.path)?;
    rows.push(directory);
    assert_eq!(
        thumbnails::available_paths(&rows),
        HashSet::from([photos.join("present.ARW").canonicalize()?])
    );
    Ok(())
}
#[test]
fn hierarchy_includes_unregistered_parents_and_descendant_counts() {
    let mut root = FolderNode::root(RootId(1), "Photos".into(), "/old".into());
    for (id, relative, count) in [(10, "2026/09/Trip/", 2), (11, "2026/10/", 3)] {
        root.insert(&Folder {
            id: FolderId(id),
            root: RootId(1),
            relative: relative.into(),
            path: PathBuf::from("/old").join(relative),
            count,
        });
    }
    root.finish();
    assert_eq!(root.count, 5);
    assert_eq!(root.children.len(), 1);
    let year = &root.children["2026"];
    assert_eq!(year.count, 5);
    assert_eq!(year.ids, HashSet::from([FolderId(10), FolderId(11)]));
    assert_eq!(
        year.children["09"].children["Trip"].folder,
        Some(FolderId(10))
    );
}
#[test]
fn root_mapping_survives_reopen() -> Result<()> {
    let d = tempfile::tempdir()?;
    let old = d.path().join("old");
    let new = d.path().join("new");
    std::fs::create_dir(&old)?;
    std::fs::write(old.join("image.ARW"), b"source")?;
    let db = d.path().join("photos.rawmakase");
    let mut c = Catalog::create(&db)?;
    c.add_folder(&old)?;
    let root = c.roots()?[0].0;
    drop(c);
    std::fs::rename(&old, &new)?;
    let ctx = egui::Context::default();
    let mut l = Library::load(&db, ctx.clone())?;
    l.session.catalog.relink_root(root, &new)?;
    drop(l);
    let mut l = Library::load(&db, ctx)?;
    assert_eq!(l.session.photos[0].path, new.join("image.ARW"));
    l.wait_for_availability();
    assert!(l.is_available(&l.session.photos[0].path));
    Ok(())
}

/// Listing every folder can take seconds on a network share, so the Library
/// opens first and marks missing originals once the check is done.
#[test]
fn library_opens_before_the_online_check_and_then_marks_missing_photos() -> Result<()> {
    let d = tempfile::tempdir()?;
    std::fs::write(d.path().join("kept.ARW"), b"source")?;
    std::fs::write(d.path().join("gone.ARW"), b"source")?;
    let db = d.path().join("photos.rawmakase");
    Catalog::create(&db)?.add_folder(d.path())?;
    std::fs::remove_file(d.path().join("gone.ARW"))?;
    let mut l = Library::load(&db, egui::Context::default())?;
    let gone = d.path().canonicalize()?.join("gone.ARW");
    assert!(l.session.photos.iter().any(|p| p.path == gone));
    assert_eq!(l.available_count(), 2);
    l.filters.only_missing = true;
    l.filter();
    assert!(l.visible.is_empty());
    l.wait_for_availability();
    assert_eq!(l.available_count(), 1);
    assert!(!l.is_available(&gone));
    assert_eq!(l.visible.len(), 1);
    Ok(())
}

/// The sidebar asks for the online count every frame; it is kept, and
/// follows photos leaving the catalog.
#[test]
fn the_online_count_follows_photos_removed_from_the_catalog() -> Result<()> {
    let d = tempfile::tempdir()?;
    for folder in ["a", "b"] {
        std::fs::create_dir(d.path().join(folder))?;
        std::fs::write(d.path().join(folder).join("1.ARW"), b"source")?;
        std::fs::write(d.path().join(folder).join("2.ARW"), b"source")?;
    }
    let db = d.path().join("photos.rawmakase");
    Catalog::create(&db)?.add_folder(d.path())?;
    let mut l = Library::load(&db, egui::Context::default())?;
    l.wait_for_availability();
    assert_eq!(l.available_count(), 4);
    let b: HashSet<FolderId> = l
        .session
        .folders
        .iter()
        .filter(|f| f.path.ends_with("b"))
        .map(|f| f.id)
        .collect();
    assert_eq!(b.len(), 1);
    l.remove_folders(&b, "b")?;
    assert_eq!(l.available_count(), 2);
    Ok(())
}

/// Once the online check is in, the Library says how many folders have no
/// photo on this computer; a folder with one of them online is not counted,
/// nor is a folder without photos, as Lightroom catalogs have.
#[test]
fn folders_with_no_photo_online_are_counted_once_the_check_is_in() -> Result<()> {
    let d = tempfile::tempdir()?;
    for folder in ["kept", "gone", "partly"] {
        std::fs::create_dir(d.path().join(folder))?;
        std::fs::write(d.path().join(folder).join("1.ARW"), b"source")?;
        std::fs::write(d.path().join(folder).join("2.ARW"), b"source")?;
    }
    let db = d.path().join("photos.rawmakase");
    Catalog::create(&db)?.add_folder(d.path())?;
    rusqlite::Connection::open(&db)?.execute_batch(
        "INSERT INTO folders (root,relative_path) SELECT root,'empty/' FROM folders LIMIT 1",
    )?;
    std::fs::remove_dir_all(d.path().join("gone"))?;
    std::fs::remove_file(d.path().join("partly").join("1.ARW"))?;
    let mut l = Library::load(&db, egui::Context::default())?;
    l.wait_for_availability();
    assert!(
        l.message
            .starts_with("1 folder isn't found on this computer"),
        "{}",
        l.message
    );
    Ok(())
}

#[test]
fn thumbnails_keep_portrait_and_landscape_proportions() {
    use super::thumbnails::fit;
    assert_eq!(fit(4000, 6000, 640), (427, 640));
    assert_eq!(fit(6000, 4000, 640), (640, 427));
    assert_eq!(fit(300, 200, 640), (300, 200));
}

#[test]
fn a_stuck_volume_check_is_not_started_again() {
    use std::sync::{Arc, Mutex, mpsc};
    let online = Arc::new(Mutex::new(HashMap::new()));
    let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (release, stalled) = mpsc::channel::<()>();
    let stalled = Mutex::new(stalled);
    let (changed_tx, changed) = mpsc::channel();
    let mounts = || vec![PathBuf::from("/Volumes/Stalled")];
    // A probe that hangs, as `is_dir` can on a stalled mount.
    assert!(volumes::spawn_volume_check(
        &online,
        &busy,
        mounts(),
        move || changed_tx.send(()).unwrap(),
        move |_| {
            stalled.lock().unwrap().recv().unwrap();
            (true, None)
        },
    ));
    for _ in 0..3 {
        assert!(!volumes::spawn_volume_check(
            &online,
            &busy,
            mounts(),
            || {},
            |_| unreachable!("a second check started"),
        ));
    }
    release.send(()).unwrap();
    changed
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        online
            .lock()
            .unwrap()
            .get(&PathBuf::from("/Volumes/Stalled")),
        Some(&(true, None))
    );
    // Once it finishes, the next check runs.
    let started = std::time::Instant::now();
    while busy.load(std::sync::atomic::Ordering::Acquire) {
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        std::thread::yield_now();
    }
    assert!(volumes::spawn_volume_check(
        &online,
        &busy,
        mounts(),
        || {},
        |_| (false, None)
    ));
}
#[test]
fn copy_previews_ignore_stale_results_and_reuse_of_a_removed_id() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("image.ARW"), b"synthetic raw")?;
    let path = directory.path().join("copies.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    let master = library.session.photos[0].id;
    let (tx, rx) = std::sync::mpsc::channel();
    library.cache.edit_rx = rx;
    let copy = library.create_virtual_copy(master)?.value;
    library.cache.edited_requested.insert(copy, 7);
    library.cache.edit_seen.insert(copy);
    // A result for an older request is dropped.
    tx.send(previews::EditResult::Ready(
        copy,
        6,
        image::RgbImage::new(4, 4),
    ))?;
    library.poll_previews(&ctx);
    assert!(!library.has_edited_thumbnail(copy));
    tx.send(previews::EditResult::Ready(
        copy,
        7,
        image::RgbImage::new(4, 4),
    ))?;
    library.poll_previews(&ctx);
    assert!(library.has_edited_thumbnail(copy));
    // Removing the copy forgets it, so a new copy given its id renders again,
    // and the removed copy's late result is dropped.
    assert_eq!(library.remove_virtual_copy(copy)?.value, Some(master));
    assert!(!library.cache.edited_requested.contains_key(&copy));
    assert!(!library.cache.edited_order.contains(&copy));
    assert!(!library.cache.edit_seen.contains(&copy));
    tx.send(previews::EditResult::Ready(
        copy,
        7,
        image::RgbImage::new(4, 4),
    ))?;
    library.poll_previews(&ctx);
    assert!(!library.has_edited_thumbnail(copy));
    Ok(())
}
#[test]
fn a_copy_name_being_typed_is_saved_when_committed() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("image.ARW"), b"synthetic raw")?;
    let path = directory.path().join("names.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    let copy = library
        .create_virtual_copy(library.session.photos[0].id)?
        .value;
    library.copy_names.draft = Some((copy, " B&W ".into()));
    library.commit_drafts()?;
    let saved = library.session.catalog.photos()?;
    assert_eq!(
        saved.iter().find(|p| p.id == copy).unwrap().copy_name,
        "B&W"
    );
    assert_eq!(library.photo(copy).unwrap().copy_name, "B&W");
    // A removed copy's draft never renames a new copy that reuses its id.
    library.remove_virtual_copy(copy)?;
    let next = library
        .create_virtual_copy(library.session.photos[0].id)?
        .value;
    library.commit_drafts()?;
    assert_eq!(library.photo(next).unwrap().copy_name, "Copy 1");
    Ok(())
}
#[test]
fn selecting_another_copy_keeps_the_name_being_typed() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("image.ARW"), b"synthetic raw")?;
    let path = directory.path().join("names.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    let master = library.session.photos[0].id;
    let first = library.create_virtual_copy(master)?.value;
    let second = library.create_virtual_copy(master)?.value;
    library.copy_names.draft = Some((first, "B&W".into()));
    // The panel is drawn for the newly selected copy before the field
    // reports losing focus.
    let photo = library.photo(second).unwrap().clone();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let _ = library.copy_names.row(ui, &photo, &mut library.session);
    });
    output.textures_delta.clear();
    assert_eq!(library.photo(first).unwrap().copy_name, "B&W");
    assert_eq!(library.copy_names.draft, Some((second, "Copy 2".into())));
    Ok(())
}
#[test]
fn a_copy_name_that_fails_to_save_survives_selecting_another_copy() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("image.ARW"), b"synthetic raw")?;
    let path = directory.path().join("names.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    let master = library.session.photos[0].id;
    let first = library.create_virtual_copy(master)?.value;
    let second = library.create_virtual_copy(master)?.value;
    // Renaming fails once the copy is gone from the catalog.
    library.session.catalog.remove_virtual_copy(first)?;
    library.copy_names.draft = Some((first, "B&W".into()));
    let photo = library.photo(second).unwrap().clone();
    for _ in 0..2 {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = library.copy_names.row(ui, &photo, &mut library.session);
        });
        output.textures_delta.clear();
    }
    assert_eq!(library.copy_names.draft, Some((first, "B&W".into())));
    assert!(library.commit_drafts().is_err());
    library.discard_drafts();
    assert!(library.commit_drafts().is_ok());
    Ok(())
}

/// A catalog of `names` in one folder, with the Library open on it.
fn library_of(names: &[&str]) -> Result<(tempfile::TempDir, Library)> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in names {
        std::fs::write(folder.join(name), b"synthetic raw")?;
    }
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let library = Library::load(&path, egui::Context::default())?;
    Ok((directory, library))
}
fn visible_names(library: &Library) -> Vec<&str> {
    library
        .visible
        .iter()
        .map(|i| library.session.photos[*i].filename.as_str())
        .collect()
}
#[test]
fn filters_combine_and_a_hidden_selection_is_cleared() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let id = |library: &Library, name: &str| {
        library
            .session
            .photos
            .iter()
            .find(|p| p.filename == name)
            .unwrap()
            .id
    };
    let (a, b, c) = (
        id(&library, "a.RAF"),
        id(&library, "b.RAF"),
        id(&library, "c.RAF"),
    );
    library.session.catalog.set_metadata(a, 3, 1, "Red")?;
    library.session.catalog.set_metadata(b, 5, 0, "")?;
    library.session.catalog.set_metadata(c, 0, -1, "Client")?;
    library.refresh()?;
    library.wait_for_availability();
    assert_eq!(visible_names(&library), ["a.RAF", "b.RAF", "c.RAF"]);
    library.filters.reverse = true;
    library.filter();
    assert_eq!(visible_names(&library), ["c.RAF", "b.RAF", "a.RAF"]);
    library.filters.reverse = false;
    library.filters.rating = Some(3);
    library.filter();
    assert_eq!(visible_names(&library), ["a.RAF", "b.RAF"]);
    library.filters.flags = [1].into();
    library.filter();
    assert_eq!(visible_names(&library), ["a.RAF"]);
    library.filters.flags.clear();
    library.filters.rating = None;
    library.filters.labels = [Label::Color("Client".into())].into();
    library.filter();
    assert_eq!(visible_names(&library), ["c.RAF"]);
    library.filters.labels.clear();
    library.filters.query = "B.r".into();
    library.filter();
    assert_eq!(visible_names(&library), ["b.RAF"]);
    library.filters.query = "client".into();
    library.filter();
    assert_eq!(visible_names(&library), ["c.RAF"]);
    library.filters.query.clear();
    library.filters.folder_scope = Some(HashSet::new());
    library.filter();
    assert!(library.visible.is_empty());
    library.filters.folder_scope = None;
    library.filters.only_missing = true;
    library.filter();
    assert!(library.visible.is_empty());
    library.filters.only_missing = false;
    // The selection follows the filter out, and comes back through `show`.
    library.select(Some(c));
    library.filters.flags = [1].into();
    library.filter();
    assert_eq!(library.selected(), None);
    library.show(c);
    assert!(library.filters.flags.is_empty());
    assert_eq!(library.selected(), Some(c));
    assert_eq!(visible_names(&library).len(), 3);
    // Navigation clamps at both ends of the visible order.
    assert_eq!(library.navigate(a, -1), Some(a));
    assert_eq!(library.navigate(a, 2), Some(c));
    assert_eq!(library.navigate(c, 5), Some(c));
    assert_eq!(library.navigate(PhotoId(999), 1), None);
    Ok(())
}
#[test]
fn attribute_filters_match_lightroom() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = ids_of(&library);
    library.session.catalog.set_metadata(ids[0], 3, 1, "Red")?;
    library.session.catalog.set_metadata(ids[1], 5, 0, "")?;
    library
        .session
        .catalog
        .set_metadata(ids[2], 0, -1, "Client")?;
    library.session.catalog.set_metadata(ids[3], 1, 0, "Blue")?;
    library.refresh()?;
    library.wait_for_availability();
    let copy = library.create_virtual_copy(ids[1])?.value;
    library.filter();
    // Flags combine: everything but rejects.
    library.filters.flags = [1, 0].into();
    library.filter();
    assert_eq!(ids_of(&library), [ids[0], ids[1], copy, ids[3]]);
    library.filters.flags.clear();
    // The rating compares with ≥, ≤ or =; Unrated is = 0.
    library.filters.rating = Some(3);
    library.filters.rating_op = RatingOp::AtMost;
    library.filter();
    assert_eq!(ids_of(&library), [ids[0], ids[2], ids[3]]);
    library.filters.rating_op = RatingOp::Exactly;
    library.filter();
    assert_eq!(ids_of(&library), [ids[0]]);
    library.filters.rating = Some(0);
    library.filter();
    assert_eq!(ids_of(&library), [ids[2]]);
    library.filters.rating = None;
    // Labels combine, with No label and Other for custom labels.
    library.filters.labels = [Label::Color("Red".into()), Label::Other].into();
    library.filter();
    assert_eq!(ids_of(&library), [ids[0], ids[2]]);
    library.filters.labels = [Label::None].into();
    library.filter();
    assert_eq!(ids_of(&library), [ids[1], copy]);
    library.filters.labels.clear();
    // Masters or virtual copies.
    library.filters.kind = Kind::Copies;
    library.filter();
    assert_eq!(ids_of(&library), [copy]);
    library.filters.kind = Kind::Masters;
    library.filter();
    assert_eq!(ids_of(&library).len(), 4);
    // Cmd+L turns the bar off and on without losing it; Clear resets it.
    library.toggle_filters();
    assert!(!library.filters.enabled && library.filters.bar_set());
    assert_eq!(ids_of(&library).len(), 5);
    // Revealing a photo keeps filters that are off: they hide nothing.
    library.filters.collection = Some(crate::catalog::CollectionId(999));
    library.filter();
    library.show(ids[0]);
    assert_eq!(library.filters.collection, None);
    assert_eq!(library.filters.kind, Kind::Masters);
    library.toggle_filters();
    assert_eq!(ids_of(&library).len(), 4);
    library.filters.clear_bar();
    assert!(library.filters.enabled && !library.filters.bar_set());
    Ok(())
}
#[test]
fn restore_source_scopes_to_the_folder_and_its_subfolders() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("photos");
    for sub in ["", "trip", "trip/day2", "other"] {
        let folder = root.join(sub);
        std::fs::create_dir_all(&folder)?;
        std::fs::write(folder.join("image.RAF"), b"synthetic raw")?;
    }
    let path = directory.path().join("sources.rawmakase");
    Catalog::create(&path)?.add_folder(&root)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    assert_eq!(library.visible.len(), 4);
    let root_id = library.session.roots[0].0;
    let in_day2 = library
        .session
        .photos
        .iter()
        .find(|p| p.path.ends_with("day2/image.RAF"))
        .unwrap()
        .id;
    let key = format!("root:{root_id}/trip");
    library.restore_source(&key, Some(in_day2));
    assert_eq!(library.source_key(), key);
    assert_eq!(library.visible.len(), 2);
    assert_eq!(library.selected(), Some(in_day2));
    assert!(library.expanded.contains(&format!("root:{root_id}")));
    assert!(library.expanded.contains(&key));
    assert_eq!(library.source_name(), "trip");
    // A folder that is not in the catalog leaves the scope as it was.
    library.restore_source("root:999/elsewhere", None);
    assert_eq!(library.source_key(), key);
    assert_eq!(library.visible.len(), 2);
    library.restore_source("", None);
    assert_eq!(library.visible.len(), 2);
    Ok(())
}
#[test]
fn thumbnail_requests_are_not_repeated_while_pending_or_failed() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF"])?;
    let ctx = library.ctx.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    library.cache.thumb_tx = tx;
    let path = library.session.photos[0].path.clone();
    library.cache.request_thumbnail(&path, &ctx);
    library.cache.request_thumbnail(&path, &ctx);
    assert_eq!(rx.try_iter().count(), 1);
    assert!(library.cache.pending.contains(&path));
    library.cache.pending.remove(&path);
    library.cache.failed.insert(path.clone());
    library.cache.request_thumbnail(&path, &ctx);
    assert_eq!(rx.try_iter().count(), 0);
    library.cache.failed.clear();
    library
        .cache
        .insert_thumb(&ctx, path.clone(), &image::RgbImage::new(2, 2));
    library.cache.request_thumbnail(&path, &ctx);
    assert_eq!(rx.try_iter().count(), 0);
    assert!(library.texture(&library.session.photos[0]).is_some());
    Ok(())
}
#[test]
fn preview_textures_keep_the_newest_192() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF"])?;
    let ctx = library.ctx.clone();
    let image = image::RgbImage::new(2, 2);
    for n in 0..193 {
        library
            .cache
            .insert_thumb(&ctx, PathBuf::from(format!("{n}.RAF")), &image);
        library.cache.edited_requested.insert(PhotoId(n), 1);
        library.cache.insert_edited(&ctx, PhotoId(n), &image);
    }
    assert_eq!(library.cache.thumbs.len(), 192);
    assert!(!library.cache.thumbs.contains_key(&PathBuf::from("0.RAF")));
    assert!(library.cache.thumbs.contains_key(&PathBuf::from("192.RAF")));
    assert_eq!(library.cache.edited.len(), 192);
    assert!(!library.has_edited_thumbnail(PhotoId(0)));
    assert!(!library.cache.edited_requested.contains_key(&PhotoId(0)));
    assert!(library.has_edited_thumbnail(PhotoId(192)));
    // Replacing a texture does not count as a new one.
    library
        .cache
        .insert_thumb(&ctx, PathBuf::from("192.RAF"), &image);
    assert_eq!(library.cache.thumb_order.len(), 192);
    Ok(())
}
#[test]
fn an_edited_preview_from_develop_outranks_renders_in_flight() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF"])?;
    let ctx = library.ctx.clone();
    let id = library.session.photos[0].id;
    let (job_tx, jobs) = std::sync::mpsc::channel();
    let (result_tx, results) = std::sync::mpsc::channel();
    let (thumb_tx, _thumbs) = std::sync::mpsc::sync_channel(8);
    library.cache.edit_tx = job_tx;
    library.cache.edit_rx = results;
    library.cache.thumb_tx = thumb_tx;
    // A photo without an edit asks the catalog once and sends nothing.
    let photo = library.session.photos[0].clone();
    library.request_previews(&photo, &ctx);
    library.request_previews(&photo, &ctx);
    assert!(jobs.try_recv().is_err());
    assert_eq!(library.cache.edits_pending, 0);
    let first = library.cache.edited_requested[&id];
    // Develop's render arrives: it is shown, kept, and outranks `first`.
    library.update_edited(&ctx, id, image::RgbImage::new(4, 4), "{}".into());
    assert!(library.has_edited_thumbnail(id));
    assert!(matches!(
        jobs.try_recv(),
        Ok(previews::EditJob::Store { .. })
    ));
    let newest = library.cache.edited_requested[&id];
    assert!(newest > first);
    library.cache.edits_pending = 1;
    result_tx.send(previews::EditResult::Skipped(id, first))?;
    library.poll_previews(&ctx);
    assert_eq!(library.cache.edited_requested.get(&id), Some(&newest));
    assert_eq!(library.cache.edits_pending, 0);
    // A cache error is reported without touching the previews.
    result_tx.send(previews::EditResult::CacheError("disk full".into()))?;
    library.poll_previews(&ctx);
    assert!(library.preview_progress_active());
    assert!(library.has_edited_thumbnail(id));
    Ok(())
}
#[test]
fn collections_panel_shows_imported_collections_and_filters_through_them() -> Result<()> {
    let (directory, library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let crate::catalog::CatalogLocation::File(path) = library.session.catalog.location().clone();
    let ids: Vec<PhotoId> = library.session.photos.iter().map(|p| p.id).collect();
    drop(library);
    {
        let db = rusqlite::Connection::open(&path)?;
        db.execute_batch(
            "INSERT INTO collections VALUES
                (2,'quick collection',NULL,'com.adobe.ag.library.collection'),
                (3,'Smart Collections',NULL,'com.adobe.ag.library.group'),
                (4,'Five Stars',3,'com.adobe.ag.library.smart_collection'),
                (5,'Unsaved Print',NULL,'com.adobe.ag.print.unsaved'),
                (10,'Trips',NULL,'com.adobe.ag.library.group'),
                (11,'Japan',10,'com.adobe.ag.library.collection'),
                (12,'Alps',10,'com.adobe.ag.library.collection'),
                (13,'Empty set',NULL,'com.adobe.ag.library.group'),
                (14,'Archive',NULL,'com.adobe.ag.library.collection');",
        )?;
        for photo in &ids[..2] {
            db.execute("INSERT INTO collection_photos VALUES(11,?,NULL)", [photo.0])?;
            db.execute("INSERT INTO collection_photos VALUES(2,?,NULL)", [photo.0])?;
        }
    }
    let mut library = Library::load(&path, egui::Context::default())?;
    let tree = collections::tree(
        &library.session.collections,
        &library.session.collection_photos,
    );
    // Sets first, then collections, by name; smart and system ones hidden,
    // and a set left empty by hiding them is dropped too.
    let names: Vec<_> = tree.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["Empty set", "Trips", "Archive"]);
    let trips: Vec<_> = tree[1]
        .children
        .iter()
        .map(|n| (n.name.as_str(), n.count))
        .collect();
    assert_eq!(trips, [("Alps", 0), ("Japan", 2)]);

    library.select_collection(crate::catalog::CollectionId(11));
    assert_eq!(library.visible.len(), 2);
    assert_eq!(library.source_name(), "Japan");
    assert_eq!(library.source_key(), "collection:11");
    // The filter bar still applies inside a collection.
    library.session.catalog.set_metadata(ids[0], 0, 1, "")?;
    library.reload()?;
    library.filters.flags = [1].into();
    library.filter();
    assert_eq!(library.visible.len(), 1);
    library.filters.flags.clear();
    library.filter();

    // A saved collection comes back with the session; an unknown one doesn't.
    let mut restored = Library::load(&path, egui::Context::default())?;
    restored.restore_source("collection:11", Some(ids[1]));
    assert_eq!(restored.visible.len(), 2);
    assert_eq!(restored.selected(), Some(ids[1]));
    let mut other = Library::load(&path, egui::Context::default())?;
    other.restore_source("collection:4", None);
    assert_eq!(other.filters.collection, None);
    assert_eq!(other.visible.len(), 3);

    // Choosing a folder clears the collection.
    let root = library.session.roots[0].0;
    library.restore_source(&format!("root:{root}"), None);
    assert_eq!(library.filters.collection, None);
    assert_eq!(library.visible.len(), 3);
    drop(directory);
    Ok(())
}
#[test]
fn capture_times_are_read_in_the_background_and_resort_in_place() -> Result<()> {
    use crate::export::exif::dated_file;
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    // File names sort the other way round from capture times.
    std::fs::write(
        folder.join("a.tif"),
        dated_file(false, "2024:03:02 10:00:02", ""),
    )?;
    std::fs::write(
        folder.join("b.jpg"),
        dated_file(true, "2024:03:02 10:00:01", "9"),
    )?;
    std::fs::write(
        folder.join("c.jpg"),
        dated_file(true, "2024:03:02 10:00:01", "1"),
    )?;
    std::fs::write(folder.join("z.ARW"), b"synthetic raw")?;
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    assert_eq!(
        visible_names(&library),
        ["a.tif", "b.jpg", "c.jpg", "z.ARW"]
    );
    let a = library.session.photos[0].id;
    library.select(Some(a));
    library.wait_for_availability();
    let started = std::time::Instant::now();
    while library.session.reading_capture_times() {
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        library.poll_capture_times();
        std::thread::yield_now();
    }
    // The undated file sorts first, and subseconds order the same second.
    assert_eq!(
        visible_names(&library),
        ["z.ARW", "c.jpg", "b.jpg", "a.tif"]
    );
    assert_eq!(
        library.session.photos[1].captured,
        "2024-03-02T10:00:01.100"
    );
    // The selection stays, and the grid follows it from where it was.
    assert_eq!(library.selected(), Some(a));
    assert_eq!(library.keep_in_place, Some((a, 0)));
    // Saved in the catalog; the undated file isn't read again this session.
    let reopened = Library::load(&path, egui::Context::default())?;
    assert_eq!(
        visible_names(&reopened),
        ["z.ARW", "c.jpg", "b.jpg", "a.tif"]
    );
    library.start_capture_times();
    assert!(!library.session.reading_capture_times());
    Ok(())
}
fn ids_of(library: &Library) -> Vec<PhotoId> {
    library
        .visible
        .iter()
        .map(|i| library.session.photos[*i].id)
        .collect()
}
fn selected_names(library: &Library) -> Vec<&str> {
    library
        .selected_ids()
        .into_iter()
        .map(|id| library.photo(id).unwrap().filename.as_str())
        .collect()
}
#[test]
fn clicks_select_like_lightroom_grid() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF", "e.RAF"])?;
    let [a, b, c, d, e] = ids_of(&library)[..] else {
        unreachable!()
    };
    let none = egui::Modifiers::NONE;
    let command = egui::Modifiers::COMMAND;
    let shift = egui::Modifiers::SHIFT;
    library.click(b, none);
    library.click(d, shift);
    assert_eq!(selected_names(&library), ["b.RAF", "c.RAF", "d.RAF"]);
    assert_eq!(library.selected(), Some(d));
    // Cmd toggles one photo; Cmd+Shift adds a range from the anchor.
    library.click(a, command);
    assert_eq!(
        selected_names(&library),
        ["a.RAF", "b.RAF", "c.RAF", "d.RAF"]
    );
    assert_eq!(library.selected(), Some(a));
    library.click(c, command);
    assert_eq!(selected_names(&library), ["a.RAF", "b.RAF", "d.RAF"]);
    assert_eq!(library.selected(), Some(a));
    library.click(e, command | shift);
    assert_eq!(selected_names(&library).len(), 5);
    // A plain click inside the selection only makes that photo active;
    // outside it, it selects the photo alone.
    library.click(b, none);
    assert_eq!(library.selected_ids().len(), 5);
    assert_eq!(library.selected(), Some(b));
    library.click(c, command);
    assert_eq!(library.mark(b), selection::Mark::Active);
    assert_eq!(library.mark(a), selection::Mark::Selected);
    assert_eq!(library.mark(c), selection::Mark::None);
    library.click(c, none);
    assert_eq!(selected_names(&library), ["c.RAF"]);
    Ok(())
}
#[test]
fn grid_keys_move_extend_and_clear_the_selection() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF", "e.RAF"])?;
    let ids = ids_of(&library);
    library.grid_columns = 2;
    library.select(Some(ids[0]));
    library.step(selection::Step::By(2), false);
    assert_eq!(library.selected(), Some(ids[2]));
    assert!(library.scroll_to_active);
    library.step(selection::Step::By(1), true);
    library.step(selection::Step::End, true);
    assert_eq!(selected_names(&library), ["c.RAF", "d.RAF", "e.RAF"]);
    assert_eq!(library.selected(), Some(ids[4]));
    // `/` drops the active photo; the next selected one takes over.
    library.deselect_active();
    assert_eq!(selected_names(&library), ["c.RAF", "d.RAF"]);
    assert_eq!(library.selected(), Some(ids[3]));
    library.select_all();
    assert_eq!(library.selected_ids(), ids);
    library.step(selection::Step::Home, false);
    assert_eq!(selected_names(&library), ["a.RAF"]);
    library.select(None);
    assert!(library.selected_ids().is_empty());
    // From nothing, a step starts at the first photo.
    library.step(selection::Step::By(1), false);
    assert_eq!(library.selected(), Some(ids[0]));
    Ok(())
}
#[test]
fn a_rejected_range_leaves_the_unflagged_view_in_one_write() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF", "e.RAF"])?;
    let ids = ids_of(&library);
    library.filters.flags = [0].into();
    library.filter();
    library.click(ids[1], egui::Modifiers::NONE);
    library.click(ids[3], egui::Modifiers::SHIFT);
    library.edit_selection(crate::app::photo_metadata::Edit::Flag(-1), false)?;
    assert_eq!(visible_names(&library), ["a.RAF", "e.RAF"]);
    assert_eq!(library.selected(), Some(ids[4]));
    assert_eq!(library.message, "3 photos · Reject");
    let saved = library.session.catalog.photos()?;
    assert_eq!(saved.iter().filter(|p| p.flag == -1).count(), 3);
    // One failing photo saves none of the batch.
    assert!(
        library
            .session
            .catalog
            .set_metadata_of(&[
                (ids[0], 5, 0, String::new()),
                (PhotoId(9999), 5, 0, String::new())
            ])
            .is_err()
    );
    assert!(
        library
            .session
            .catalog
            .photos()?
            .iter()
            .all(|p| p.rating == 0)
    );
    Ok(())
}
#[test]
fn a_toggle_on_a_selection_follows_the_active_photo() -> Result<()> {
    use crate::app::photo_metadata::Edit;
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ids = ids_of(&library);
    library.edit_metadata(ids[0], Edit::Flag(1), false)?;
    library.select(Some(ids[0]));
    library.select_all();
    // The active photo is picked, so the toggle unflags all three.
    library.edit_selection(Edit::TogglePick, false)?;
    assert!(library.session.photos.iter().all(|p| p.flag == 0));
    library.edit_selection(Edit::ToggleLabel("Red".into()), false)?;
    assert!(library.session.photos.iter().all(|p| p.label == "Red"));
    // Shift does not advance a multi-photo selection.
    assert_eq!(library.edit_selection(Edit::Rating(3), true)?, None);
    assert_eq!(library.selected_ids().len(), 3);
    Ok(())
}
#[test]
fn up_and_down_stay_in_their_column_at_the_edges() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF", "e.RAF"])?;
    let ids = ids_of(&library);
    library.grid_columns = 4;
    library.select(Some(ids[3]));
    library.step(selection::Step::By(-4), false);
    assert_eq!(library.selected(), Some(ids[3]));
    library.step(selection::Step::By(4), false);
    assert_eq!(library.selected(), Some(ids[3]));
    library.select(Some(ids[0]));
    library.step(selection::Step::By(4), false);
    assert_eq!(library.selected(), Some(ids[4]));
    Ok(())
}
#[test]
fn a_hidden_active_photo_hands_over_to_the_rest_of_the_selection() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ids = ids_of(&library);
    library.edit_metadata(ids[1], crate::app::photo_metadata::Edit::Rating(3), false)?;
    library.edit_metadata(ids[2], crate::app::photo_metadata::Edit::Rating(3), false)?;
    library.select_all();
    library.select(Some(ids[0]));
    library.select_all();
    library.filters.rating = Some(3);
    library.filter();
    assert_eq!(library.selected(), Some(ids[1]));
    assert_eq!(library.selected_ids(), ids[1..]);
    Ok(())
}
#[test]
fn the_filmstrip_keeps_its_place_across_views() -> Result<()> {
    let (_directory, mut library) = library_of(
        &(0..40)
            .map(|n| format!("{n:02}.RAF"))
            .collect::<Vec<_>>()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    )?;
    let ctx = library.ctx.clone();
    let ids = ids_of(&library);
    let last = ids[39];
    // The strip's horizontal offset after a few frames showing `current`
    // in the Library or in Develop, a second apart so the scroll animation
    // finishes.
    let mut time = 0.;
    let strip = std::cell::Cell::new(None);
    let mut frames = |library: &mut Library, current: Option<PhotoId>, module: Module| {
        for _ in 0..3 {
            time += 1.;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1200., 800.),
                    )),
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    // As `filmstrip_panel` draws it, noting the scroll id.
                    egui::Panel::bottom(super::filmstrip::ID)
                        .exact_size(super::filmstrip::HEIGHT)
                        .show(ui, |ui| {
                            strip.set(Some(ui.make_persistent_id(egui::IdSalt::new("filmstrip"))));
                            library.filmstrip(ui, current, module);
                        });
                },
            );
            output.textures_delta.clear();
        }
        egui::scroll_area::State::load(&ctx, strip.get().unwrap()).map_or(0., |s| s.offset.x)
    };
    // Nothing selected: the strip still draws, at the start.
    assert_eq!(frames(&mut library, None, Module::Library), 0.);
    // A photo shown off the end is brought into view.
    let revealed = frames(&mut library, Some(last), Module::Library);
    assert!(revealed > 0.);
    // Develop, on the same photo, shows the same strip where it was.
    assert_eq!(frames(&mut library, Some(last), Module::Develop), revealed);
    // Scrolled back to the start by hand, it stays there across views
    // while the photo shown is the same.
    let id = strip.get().unwrap();
    let mut state = egui::scroll_area::State::load(&ctx, id).unwrap();
    state.offset.x = 0.;
    state.store(&ctx, id);
    assert_eq!(frames(&mut library, Some(last), Module::Library), 0.);
    assert_eq!(frames(&mut library, Some(last), Module::Develop), 0.);
    // Another photo is brought into view.
    assert!(frames(&mut library, Some(ids[38]), Module::Library) > 0.);
    // A sort that moves the photo shown brings it back into view.
    library.filters.reverse = true;
    library.filter();
    assert!(frames(&mut library, Some(ids[38]), Module::Library) < revealed);
    Ok(())
}
#[test]
fn the_filmstrip_follows_a_change_made_after_it_was_drawn() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF"])?;
    let ctx = library.ctx.clone();
    let ids = ids_of(&library);
    library.select(Some(ids[0]));
    let draw = |library: &mut Library| {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let active = library.selected();
            library.filmstrip_panel(ui, active, Module::Library);
        });
        output.textures_delta.clear();
    };
    draw(&mut library);
    assert!(!library.filmstrip_behind());
    // The grid, drawn after the strip, takes a click: the strip is behind
    // until it draws again.
    library.click(ids[1], egui::Modifiers::NONE);
    assert!(library.filmstrip_behind());
    draw(&mut library);
    assert!(!library.filmstrip_behind());
    // So is a re-sort that keeps the selection.
    library.filters.reverse = true;
    library.filter();
    assert!(library.filmstrip_behind());
    Ok(())
}
#[test]
fn a_strip_scrolled_past_a_shorter_list_draws_again() {
    let view = |x: f32| egui::Rect::from_min_size(egui::pos2(x, 0.), Vec2::new(800., 100.));
    assert_eq!(super::filmstrip::in_view(view(0.), 100., 40), (0..8, false));
    assert_eq!(
        super::filmstrip::in_view(view(3150.), 100., 40),
        (31..40, false)
    );
    // An offset from a longer list, past the 3 photos now shown.
    assert_eq!(
        super::filmstrip::in_view(view(3000.), 100., 3),
        (3..3, true)
    );
    assert_eq!(super::filmstrip::in_view(view(0.), 100., 0), (0..0, false));
}
#[test]
fn a_panel_with_nothing_to_do_keeps_an_earlier_panels_action() {
    // The strip's Open in Develop survives the sidebar drawn after it.
    assert_eq!(
        Action::Develop(PhotoId(1)).then(Action::None),
        Action::Develop(PhotoId(1))
    );
    assert_eq!(
        Action::Develop(PhotoId(1)).then(Action::AddFolder),
        Action::AddFolder
    );
    assert_eq!(
        Action::None.then(Action::Develop(PhotoId(2))),
        Action::Develop(PhotoId(2))
    );
}
#[test]
fn a_filmstrip_click_does_what_the_view_shown_does() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = ids_of(&library);
    let show = egui::Modifiers::NONE;
    // Grid: Cmd adds, Shift extends, a click on the active photo keeps it.
    library.select(Some(ids[0]));
    library.filmstrip_pick(Pick::Show(ids[2]), egui::Modifiers::COMMAND);
    assert_eq!(library.selected_ids(), [ids[0], ids[2]]);
    library.filmstrip_pick(Pick::Show(ids[3]), egui::Modifiers::SHIFT);
    assert_eq!(library.selected_ids(), [ids[2], ids[3]]);
    // Cmd on the active photo takes it out of the selection.
    library.filmstrip_pick(Pick::Show(ids[3]), egui::Modifiers::COMMAND);
    assert_eq!(library.selected_ids(), [ids[2]]);
    assert_eq!(library.selected(), Some(ids[2]));
    // Loupe: a plain click shows the photo alone.
    library.open_loupe();
    library.filmstrip_pick(Pick::Show(ids[1]), show);
    assert_eq!(library.selected_ids(), [ids[1]]);
    // Compare: another photo becomes the candidate, the select activates
    // its own side.
    library.close_loupe();
    library.click(ids[1], show);
    library.click(ids[2], egui::Modifiers::COMMAND);
    library.click(ids[1], show);
    library.open_compare();
    library.filmstrip_pick(Pick::Show(ids[3]), show);
    assert_eq!(
        (library.compare.select, library.compare.candidate),
        (Some(ids[1]), Some(ids[3]))
    );
    assert_eq!(library.selected(), Some(ids[3]));
    library.filmstrip_pick(Pick::Show(ids[1]), show);
    assert_eq!(library.compare.candidate, Some(ids[3]));
    assert_eq!(library.selected(), Some(ids[1]));
    // Open in Develop is the same everywhere.
    assert!(matches!(
        library.filmstrip_pick(Pick::Develop(ids[0]), show),
        Action::Develop(id) if id == ids[0]
    ));
    Ok(())
}
#[test]
fn loupe_shows_a_jpeg_at_the_size_of_the_view() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::from_pixel(3000, 2000, image::Rgb([200, 120, 40]))
        .save(folder.join("a.jpg"))?;
    std::fs::write(folder.join("b.jpg"), b"not a jpeg")?;
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    library.wait_for_availability();
    library.select(Some(library.session.photos[0].id));
    library.open_loupe();
    assert!(library.loupe_open());
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200., 800.),
        )),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input(), |ui| {
        library.grid(ui, &mut Default::default());
    });
    output.textures_delta.clear();
    library.loupe.wait(&ctx);
    // A 1200 px wide view asks for the next step up, 1536 px; never more.
    assert_eq!(library.loupe.state, loupe::State::Ready);
    assert_eq!(library.loupe.texture_size(), Some([1536, 1024]));
    // The next photo replaces it; a damaged file says why.
    library.step(selection::Step::By(1), false);
    let mut output = ctx.run_ui(input(), |ui| {
        library.grid(ui, &mut Default::default());
    });
    output.textures_delta.clear();
    library.loupe.wait(&ctx);
    assert!(matches!(library.loupe.state, loupe::State::Failed(_)));
    library.close_loupe();
    assert!(!library.loupe_open());
    Ok(())
}
#[test]
fn flag_steps_up_and_down_and_stops_at_the_ends() {
    use crate::app::photo_metadata::Edit;
    let photo = |flag| Photo {
        id: PhotoId(1),
        folder: FolderId(1),
        path: "a.RAF".into(),
        filename: "a.RAF".into(),
        captured: String::new(),
        rating: 0,
        flag,
        label: String::new(),
        format: "RAF".into(),
        copy_name: String::new(),
        master: None,
        keywords: String::new(),
        has_lightroom_edits: false,
    };
    assert_eq!(Edit::FlagDelta(1).values(&photo(-1)).1, 0);
    assert_eq!(Edit::FlagDelta(1).values(&photo(0)).1, 1);
    assert_eq!(Edit::FlagDelta(1).values(&photo(1)).1, 1);
    assert_eq!(Edit::FlagDelta(-1).values(&photo(-1)).1, -1);
}
#[test]
fn loupe_zooms_at_the_navigator_levels_and_prepares_the_next_photo() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["a.jpg", "b.jpg"] {
        image::RgbImage::from_fn(3000, 2000, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        })
        .save(folder.join(name))?;
    }
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    library.wait_for_availability();
    library.select(Some(library.session.photos[0].id));
    library.open_loupe();
    let mut zoom = crate::app::navigator::Zoom::default();
    let frame = |library: &mut Library, zoom: &mut crate::app::navigator::Zoom| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200., 800.),
                )),
                ..Default::default()
            },
            |ui| {
                // The filmstrip below, as the workspace draws it.
                let active = library.selected();
                library.filmstrip_panel(ui, active, Module::Library);
                library.grid(ui, zoom);
            },
        );
        output.textures_delta.clear();
    };
    frame(&mut library, &mut zoom);
    library.loupe.wait(&ctx);
    // The next photo is prepared once this one is shown, and shown at once.
    frame(&mut library, &mut zoom);
    library.loupe.wait_ahead(&ctx);
    library.step(selection::Step::By(1), false);
    frame(&mut library, &mut zoom);
    assert_eq!(library.loupe.state, loupe::State::Ready);
    // 100% reads only the view: 1200 by 642 pixels of the 3000 by 2000.
    zoom.set(1.);
    frame(&mut library, &mut zoom);
    library.loupe.regions.wait(&ctx);
    assert_eq!(library.loupe.regions.full, Some([3000, 2000]));
    let (region, rect) = library.loupe.regions.region.clone().unwrap();
    assert_eq!(region.size(), [1200, 642]);
    assert!((rect[0] - 0.3).abs() < 1e-3 && (rect[2] - 0.4).abs() < 1e-3);
    // At 200% half as many image pixels fill the view.
    zoom.set(2.);
    frame(&mut library, &mut zoom);
    library.loupe.regions.wait(&ctx);
    let (region, _) = library.loupe.regions.region.clone().unwrap();
    assert_eq!(region.size(), [600, 321]);
    // Panning past the corner stops at the edge of the photo.
    zoom.pan = [0., 0.];
    frame(&mut library, &mut zoom);
    library.loupe.regions.wait(&ctx);
    frame(&mut library, &mut zoom);
    let (_, rect) = library.loupe.regions.region.clone().unwrap();
    assert_eq!([rect[0], rect[1]], [0., 0.]);
    zoom.set(0.);
    frame(&mut library, &mut zoom);
    assert!(library.loupe.regions.region.is_none());
    Ok(())
}
#[test]
fn photo_info_of_folder_photos_is_read_once_and_kept() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::new(300, 200).save(folder.join("a.jpg"))?;
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    let id = library.session.photos[0].id;
    library.wait_for_availability();
    let started = std::time::Instant::now();
    while library.session.reading_photo_info() {
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        library.poll_photo_info();
        std::thread::yield_now();
    }
    library.select(Some(id));
    let info = library.active_info().unwrap();
    assert_eq!(info.dimensions_text().as_deref(), Some("300 × 200"));
    // Kept: nothing is left to read on the next open.
    assert!(library.session.catalog.photos_without_info()?.is_empty());
    Ok(())
}
#[test]
fn compare_shows_the_select_beside_a_candidate() -> Result<()> {
    use crate::app::photo_metadata::Edit;
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = ids_of(&library);
    // The active photo beside the next one selected with it.
    library.click(ids[1], egui::Modifiers::NONE);
    library.click(ids[3], egui::Modifiers::COMMAND);
    library.click(ids[1], egui::Modifiers::NONE);
    library.open_compare();
    assert!(library.compare_open() && library.edits_active_only());
    assert_eq!(
        (library.compare.select, library.compare.candidate),
        (Some(ids[1]), Some(ids[3]))
    );
    // Arrows move the candidate past the select, and stop at the ends.
    library.step_candidate(-1);
    assert_eq!(library.compare.candidate, Some(ids[2]));
    library.step_candidate(-1);
    assert_eq!(library.compare.candidate, Some(ids[0]));
    library.step_candidate(-1);
    assert_eq!(library.compare.candidate, Some(ids[0]));
    assert_eq!(library.selected(), Some(ids[0]));
    assert_eq!(library.selected_ids(), [ids[0], ids[1]]);
    // Rating keys go to the active photo; Shift moves the candidate on.
    library.edit_compared(Edit::Rating(4), true)?;
    assert_eq!(library.photo(ids[0]).unwrap().rating, 4);
    assert_eq!(library.photo(ids[1]).unwrap().rating, 0);
    assert_eq!(library.compare.candidate, Some(ids[2]));
    // Down swaps, keeping the active photo; Up makes the candidate the select.
    library.swap_compare();
    assert_eq!(
        (library.compare.select, library.compare.candidate),
        (Some(ids[2]), Some(ids[1]))
    );
    assert_eq!(library.selected(), Some(ids[2]));
    library.make_select();
    assert_eq!(
        (library.compare.select, library.compare.candidate),
        (Some(ids[1]), Some(ids[2]))
    );
    // E opens the active photo in the Loupe; C from there compares again.
    library.open_loupe();
    assert!(library.loupe_open() && !library.compare_open());
    library.open_compare();
    assert!(!library.loupe_open() && library.compare_open());
    library.show_grid();
    assert!(!library.compare_open());
    assert_eq!(library.selected_ids().len(), 2);
    // Photos another source or filter hides give way to ones shown: the
    // candidate to the next photo, the select to the candidate.
    library.open_compare();
    library.compare.candidate = Some(ids[3]);
    library.filters.query = "c.RAF".into();
    library.filter();
    assert_eq!(library.keep_compared_shown(), Some(ids[2]));
    assert_eq!(library.compare.candidate, None);
    library.filters.query.clear();
    library.filter();
    library.compare.select = Some(ids[0]);
    library.compare.candidate = Some(ids[2]);
    library.filters.query = "c.RAF".into();
    library.filter();
    assert_eq!(library.keep_compared_shown(), Some(ids[2]));
    library.filters.query.clear();
    library.filter();
    library.show_grid();
    // With one photo shown there is nothing to compare it with.
    library.filters.query = "a.RAF".into();
    library.filter();
    library.open_compare();
    assert_eq!(library.compare.candidate, None);
    Ok(())
}
#[test]
fn compare_follows_edits_sources_and_other_commands() -> Result<()> {
    use crate::app::photo_metadata::Edit;
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = ids_of(&library);
    library.select(Some(ids[0]));
    library.open_compare();
    // Rejecting the select with Shift under an Unflagged filter: it gives
    // way to the candidate, beside the next photo, without skipping one.
    library.filters.flags = [0].into();
    library.filter();
    library.edit_compared(Edit::Flag(-1), true)?;
    assert_eq!(
        (library.compare.select, library.compare.candidate),
        (Some(ids[1]), Some(ids[2]))
    );
    // An active candidate promoted to the select stays active.
    library.step_candidate(1);
    library.step_candidate(-1);
    assert_eq!(library.selected(), Some(ids[2]));
    library.filters.collection = Some(crate::catalog::CollectionId(1));
    library.filters.members = [ids[2], ids[3]].into();
    library.filter();
    assert_eq!(library.keep_compared_shown(), Some(ids[2]));
    assert_eq!(library.compare.candidate, Some(ids[3]));
    assert_eq!(library.compare.active, super::compare::Side::Select);
    assert_eq!(library.selected(), Some(ids[2]));
    // A source with neither photo seeds Compare from what it shows.
    library.filters.members = [ids[0], ids[1]].into();
    library.filter();
    assert_eq!(library.keep_compared_shown(), Some(ids[1]));
    library.filters.collection = None;
    library.filters.flags.clear();
    library.filter();
    // A selection another command makes, such as a new virtual copy, is
    // followed beside the select.
    let select = library.keep_compared_shown();
    let copy = library.create_virtual_copy(ids[1])?.value;
    assert_eq!(library.keep_compared_shown(), select);
    assert_eq!(library.compare.candidate, Some(copy));
    assert_eq!(library.selected(), Some(copy));
    Ok(())
}
#[test]
fn a_compare_edit_keeps_the_select_and_records_where_it_left() -> Result<()> {
    use crate::app::photo_metadata::Edit;
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = ids_of(&library);
    library.select(Some(ids[0]));
    library.open_compare();
    library.filters.flags = [0].into();
    library.filter();
    // B, the active candidate, is rejected and hidden: A stays the select.
    library.step_candidate(1);
    library.step_candidate(-1);
    library.edit_compared(Edit::Flag(-1), false)?;
    assert_eq!(
        (library.compare.select, library.compare.candidate),
        (Some(ids[0]), Some(ids[2]))
    );
    // Shift on a shown candidate: the undo log has the pair it moved to.
    library.edit_compared(Edit::Rating(3), true)?;
    assert_eq!(library.compare.candidate, Some(ids[3]));
    let command = library.take_done().pop().unwrap();
    assert_eq!(command.place_after, library.place());
    // Undo and redo return to the pairs, select and candidate as they were.
    library.go_to_place(&command.place_before);
    library.keep_compared_shown();
    assert_eq!(
        (library.compare.select, library.compare.candidate),
        (Some(ids[0]), Some(ids[2]))
    );
    assert_eq!(library.selected(), Some(ids[2]));
    library.go_to_place(&command.place_after);
    library.keep_compared_shown();
    assert_eq!(
        (library.compare.select, library.compare.candidate),
        (Some(ids[0]), Some(ids[3]))
    );
    // An edit made elsewhere, as from a filmstrip menu, keeps the select.
    library.edit_photos(&[ids[3]], Edit::Flag(-1), false)?;
    library.keep_compared_shown();
    assert_eq!(library.compare.select, Some(ids[0]));
    assert_eq!(library.compare.candidate, Some(ids[2]));
    Ok(())
}
#[test]
fn survey_shows_the_selection_and_rates_the_active_photo() -> Result<()> {
    use crate::app::photo_metadata::Edit;
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = ids_of(&library);
    library.click(ids[0], egui::Modifiers::NONE);
    library.click(ids[2], egui::Modifiers::SHIFT);
    library.open_survey();
    assert!(library.survey_open() && library.edits_active_only());
    assert_eq!(library.surveyed(), ids[..3]);
    // Arrows move the active photo among the photos surveyed, and stop.
    library.step_surveyed(-1);
    assert_eq!(library.selected(), Some(ids[1]));
    library.step_surveyed(-5);
    assert_eq!(library.selected(), Some(ids[0]));
    // Keys rate the active photo alone; Shift moves on.
    library.edit_shown(Edit::Rating(2), true)?;
    assert_eq!(library.photo(ids[0]).unwrap().rating, 2);
    assert_eq!(library.photo(ids[1]).unwrap().rating, 0);
    assert_eq!(library.selected(), Some(ids[1]));
    // A reject the filter hides leaves the survey; the next photo is active.
    library.filters.flags = [0].into();
    library.filter();
    library.edit_shown(Edit::Flag(-1), false)?;
    assert_eq!(library.surveyed(), [ids[0], ids[2]]);
    assert_eq!(library.selected(), Some(ids[2]));
    library.filters.flags.clear();
    library.filter();
    // Taking a photo out keeps at least one.
    library.drop_surveyed(ids[2]);
    assert_eq!(library.surveyed(), [ids[0]]);
    library.drop_surveyed(ids[0]);
    assert_eq!(library.surveyed(), [ids[0]]);
    // The other views close Survey, and G returns to the grid.
    library.open_compare();
    assert!(!library.survey_open() && library.compare_open());
    library.open_survey();
    library.open_loupe();
    assert!(!library.survey_open() && library.loupe_open());
    library.open_survey();
    library.show_grid();
    assert!(!library.survey_open() && !library.edits_active_only());
    Ok(())
}
#[test]
fn a_filter_hiding_the_active_candidate_passes_its_role_on() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ids = ids_of(&library);
    library.session.catalog.set_metadata(ids[1], 0, -1, "")?;
    library.refresh()?;
    library.wait_for_availability();
    library.select(Some(ids[0]));
    library.open_compare();
    library.step_candidate(1);
    library.step_candidate(-1);
    assert_eq!(library.selected(), Some(ids[1]));
    // The sidebar's filter hides rejects: the next photo takes B's place,
    // and its role, so the next rating goes to it.
    library.filters.flags = [0, 1].into();
    library.filter();
    library.keep_compared_shown();
    assert_eq!(
        (library.compare.select, library.compare.candidate),
        (Some(ids[0]), Some(ids[2]))
    );
    assert_eq!(library.compare.active, super::compare::Side::Candidate);
    Ok(())
}
#[test]
fn compare_follows_a_restored_place_that_hides_its_active_photo() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = ids_of(&library);
    for id in [ids[0], ids[1], ids[3]] {
        library.session.catalog.set_metadata(id, 1, 0, "")?;
    }
    library.refresh()?;
    library.wait_for_availability();
    library.select(Some(ids[0]));
    library.open_compare();
    library.step_candidate(1);
    library.keep_compared_shown();
    assert_eq!(library.selected(), Some(ids[2]));
    // Undo returns to a filter that hides C, with D selected.
    let mut place = library.place();
    place.filters.rating = Some(1);
    place.selection = Default::default();
    place.selection.selected = [ids[3]].into();
    place.selection.active = Some(ids[3]);
    library.go_to_place(&place);
    library.keep_compared_shown();
    assert_eq!(
        (library.compare.select, library.compare.candidate),
        (Some(ids[0]), Some(ids[3]))
    );
    assert_eq!(library.compare.active, super::compare::Side::Candidate);
    Ok(())
}
#[test]
fn compare_follows_a_restored_place_within_its_pair() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ids = ids_of(&library);
    for id in [ids[0], ids[1]] {
        library.session.catalog.set_metadata(id, 1, 0, "")?;
    }
    library.refresh()?;
    library.wait_for_availability();
    library.select(Some(ids[0]));
    library.open_compare();
    library.step_candidate(1);
    library.keep_compared_shown();
    assert_eq!(library.selected(), Some(ids[2]));
    // Undo restores A alone, under a filter that hides C: A is active.
    let mut place = library.place();
    place.filters.rating = Some(1);
    place.selection = Default::default();
    place.selection.selected = [ids[0]].into();
    place.selection.active = Some(ids[0]);
    library.go_to_place(&place);
    library.keep_compared_shown();
    assert_eq!(library.compare.select, Some(ids[0]));
    assert_eq!(library.compare.active, super::compare::Side::Select);
    Ok(())
}
#[test]
fn compare_follows_the_master_after_removing_its_copy() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF"])?;
    let ids = ids_of(&library);
    let copy = library.create_virtual_copy(ids[0])?.value;
    library.select(Some(ids[0]));
    library.open_compare();
    library.compare.candidate = Some(copy);
    library.step_candidate(1);
    library.step_candidate(-1);
    library.keep_compared_shown();
    assert_eq!(library.selected(), Some(copy));
    library.remove_virtual_copy(copy)?;
    library.keep_compared_shown();
    assert_eq!(library.compare.select, Some(ids[0]));
    assert_eq!(library.compare.active, super::compare::Side::Select);
    Ok(())
}
#[test]
fn a_large_survey_shows_the_photos_up_to_the_active_one() -> Result<()> {
    let names: Vec<String> = (0..52).map(|i| format!("{i:02}.RAF")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let (_directory, mut library) = library_of(&names)?;
    let ids = ids_of(&library);
    library.select_all();
    library.make_active(ids[50]);
    library.open_survey();
    assert_eq!(library.shown_surveyed(), ids[3..51]);
    library.step_surveyed(1);
    assert_eq!(library.shown_surveyed(), ids[4..52]);
    library.make_active(ids[0]);
    assert_eq!(library.shown_surveyed(), ids[..48]);
    Ok(())
}
#[test]
fn grid_cells_cycle_through_lightrooms_styles() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::new(300, 200).save(folder.join("a.png"))?;
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    library.wait_for_availability();
    let started = std::time::Instant::now();
    while library.session.reading_photo_info() {
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(5));
        library.poll_photo_info();
    }
    // Expanded cells add a line of details: dimensions and format.
    let photo = library.session.photos[0].clone();
    assert_eq!(library.cell_details(&photo), "300 × 200 · PNG");
    assert_eq!(cell::Style::Expanded.height(200.), 216.);
    assert_eq!(cell::Style::Plain.height(200.), 200.);
    // J cycles Compact, Expanded and Photos Only.
    let mut style = cell::Style::default();
    for expected in [
        cell::Style::Expanded,
        cell::Style::Plain,
        cell::Style::Compact,
    ] {
        style = style.next();
        assert_eq!(style, expected);
    }
    Ok(())
}
#[test]
fn photos_sort_in_lightrooms_orders() -> Result<()> {
    use super::sort::Sort;
    let (_directory, mut library) = library_of(&["b10.RAF", "a.NEF", "b9.RAF"])?;
    let id = |library: &Library, name: &str| {
        library
            .session
            .photos
            .iter()
            .find(|p| p.filename == name)
            .unwrap()
            .id
    };
    let (b10, a, b9) = (
        id(&library, "b10.RAF"),
        id(&library, "a.NEF"),
        id(&library, "b9.RAF"),
    );
    library.session.catalog.set_metadata(b10, 2, 1, "Green")?;
    library.session.catalog.set_metadata(a, 5, -1, "")?;
    library.session.catalog.set_metadata(b9, 0, 0, "Red")?;
    library.refresh()?;
    library.wait_for_availability();
    let order = |library: &mut Library, sort| {
        library.filters.sort = sort;
        library.filter();
        ids_of(library)
    };
    assert_eq!(order(&mut library, Sort::Rating), [b9, b10, a]);
    assert_eq!(order(&mut library, Sort::Pick), [a, b9, b10]);
    assert_eq!(order(&mut library, Sort::LabelColor), [b9, b10, a]);
    assert_eq!(order(&mut library, Sort::FileName), [a, b9, b10]);
    assert_eq!(order(&mut library, Sort::Extension), [a, b10, b9]);
    assert_eq!(order(&mut library, Sort::AddedOrder), {
        let mut added = [b10, a, b9];
        added.sort();
        added
    });
    // Reversed, the other way round, with photos alike still in capture
    // order.
    library.filters.reverse = true;
    assert_eq!(order(&mut library, Sort::Rating), [a, b10, b9]);
    assert_eq!(order(&mut library, Sort::Extension), [b10, b9, a]);
    library.filters.reverse = false;
    // Edit time: photos never edited first, then by when.
    let path = library.photo(b9).unwrap().path.clone();
    library.session.catalog.save_edit(
        b9,
        &path,
        &crate::model::recipe::Recipe::default(),
        &Default::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    library.sort_keys = None;
    assert_eq!(order(&mut library, Sort::EditTime).last(), Some(&b9));
    Ok(())
}
#[test]
fn the_library_layout_is_kept_and_returned_to() -> Result<()> {
    use super::filter::{Kind, Label, RatingOp};
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ids = ids_of(&library);
    library.filters.sort = super::sort::Sort::FileName;
    library.filters.reverse = true;
    library.filters.flags = [0, 1].into();
    library.filters.rating = Some(2);
    library.filters.rating_op = RatingOp::AtMost;
    library.filters.labels = [Label::Color("Red".into()), Label::None].into();
    library.filters.kind = Kind::Masters;
    library.filters.enabled = false;
    library.cell_style = cell::Style::Expanded;
    library.thumb_size = 240.;
    library.filter();
    library.select(Some(ids[1]));
    library.open_survey();
    let layout = library.layout();
    // Through the session file and back.
    let saved: crate::app::session::LibraryLayout =
        serde_json::from_str(&serde_json::to_string(&layout)?)?;
    let (_other, mut restored) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    restored.select(Some(ids_of(&restored)[1]));
    restored.apply_layout(&saved);
    assert_eq!(restored.layout(), layout);
    assert!(restored.survey_open());
    // A layout from another version keeps what it can.
    let partial: crate::app::session::LibraryLayout =
        serde_json::from_str(r#"{"sort": "rating", "view": "lightbox", "cell_style": "?"}"#)?;
    restored.apply_layout(&partial);
    assert_eq!(restored.filters.sort, super::sort::Sort::Rating);
    assert_eq!(restored.cell_style, cell::Style::Compact);
    Ok(())
}
#[test]
fn typed_keywords_follow_lightroom_and_refuse_the_separator() -> Result<()> {
    use super::descriptive::parse_keywords;
    assert_eq!(
        parse_keywords("Kraków < Poland < Places, Smith ,")?,
        vec![
            vec!["Places".to_string(), "Poland".into(), "Kraków".into()],
            vec!["Smith".to_string()],
        ]
    );
    assert!(parse_keywords("a|b").is_err());
    assert!(parse_keywords("Child < ").is_err());
    Ok(())
}
#[test]
fn a_mixed_field_left_alone_changes_nothing_and_typing_replaces_it_on_all() -> Result<()> {
    use crate::metadata::{LangAlt, TextField, Value};
    let (_dir, mut library) = library_of(&["a.ARW", "b.ARW"])?;
    let ids: Vec<PhotoId> = library.session.photos.iter().map(|p| p.id).collect();
    library
        .session
        .catalog
        .set_text(&ids[..1], TextField::Title, "Only a")?;
    library.selection.selected = ids.iter().copied().collect();
    library.selection.active = Some(ids[0]);
    library.sync_fields();
    assert_eq!(library.fields.drafts.title, "");
    // Leaving the field without typing.
    library.commit_fields()?;
    assert!(library.take_descriptive_done().is_empty());
    assert_eq!(library.session.catalog.descriptive(ids[1])?.title, None);
    // Typing then moving the selection saves it for the photos it was typed for.
    library.fields.drafts.title = "Both".into();
    library.selection.selected = [ids[1]].into();
    library.selection.active = Some(ids[1]);
    library.sync_fields();
    for id in &ids {
        assert_eq!(
            library.session.catalog.descriptive(*id)?.title,
            Some(Value::Set(LangAlt::new("Both")))
        );
    }
    assert_eq!(library.take_descriptive_done().len(), 1);
    assert_eq!(library.fields.targets, vec![ids[1]]);
    Ok(())
}
#[test]
fn a_descriptive_edit_is_one_command_that_restores_each_photo() -> Result<()> {
    use super::descriptive::DescriptiveEdit;
    use crate::metadata::{TextField, Value};
    let (_dir, mut library) = library_of(&["a.ARW", "b.ARW"])?;
    let ids: Vec<PhotoId> = library.session.photos.iter().map(|p| p.id).collect();
    library
        .session
        .catalog
        .set_text(&ids[1..], TextField::Copyright, "")?;
    library.edit_descriptive(
        &ids,
        DescriptiveEdit::AddKeywords(vec![vec!["Places".into(), "City".into()]]),
    )?;
    library.edit_descriptive(
        &ids,
        DescriptiveEdit::Text(TextField::Copyright, "© Example".into()),
    )?;
    library.edit_descriptive(&ids, DescriptiveEdit::Creators(vec![]))?;
    let done = library.take_descriptive_done();
    assert_eq!(done.len(), 3);
    assert!(library.session.photos.iter().all(|p| p.keywords == "City"));
    for command in done.iter().rev() {
        library.restore_descriptive(&command.before, &command.ratings_before)?;
    }
    assert_eq!(
        library.session.catalog.descriptive(ids[0])?,
        Default::default()
    );
    assert_eq!(
        library.session.catalog.descriptive(ids[1])?.copyright,
        Some(Value::Cleared)
    );
    assert!(library.session.photos.iter().all(|p| p.keywords.is_empty()));
    // Removing a keyword only some photos have, from all of them.
    let city = library.session.catalog.keyword_at(&["City".into()])?;
    library.session.catalog.add_keyword(&ids[..1], city)?;
    library.edit_descriptive(&ids, DescriptiveEdit::RemoveKeyword(city))?;
    assert!(library.session.catalog.keywords(ids[0])?.is_empty());
    Ok(())
}
#[test]
fn a_draft_that_fails_to_save_stays_with_its_photos() -> Result<()> {
    let (_dir, mut library) = library_of(&["a.ARW", "b.ARW"])?;
    let ids: Vec<PhotoId> = library.session.photos.iter().map(|p| p.id).collect();
    library.selection.selected = [ids[0]].into();
    library.selection.active = Some(ids[0]);
    library.sync_fields();
    library.fields.drafts.title = "For a".into();
    // A write that fails, as on a full disk.
    library.session.catalog.fail_metadata_writes()?;
    library.selection.selected = [ids[1]].into();
    library.selection.active = Some(ids[1]);
    library.sync_fields();
    assert_eq!(library.fields.targets, vec![ids[0]]);
    assert_eq!(library.fields.drafts.title, "For a");
    Ok(())
}
#[test]
fn a_keyword_being_typed_is_dropped_when_the_selection_moves() -> Result<()> {
    let (_dir, mut library) = library_of(&["a.ARW", "b.ARW"])?;
    let ids: Vec<PhotoId> = library.session.photos.iter().map(|p| p.id).collect();
    library.selection.selected = [ids[0]].into();
    library.selection.active = Some(ids[0]);
    library.sync_fields();
    library.fields.keyword_entry = "Typed for a".into();
    library.selection.selected = [ids[1]].into();
    library.selection.active = Some(ids[1]);
    library.sync_fields();
    assert!(library.fields.keyword_entry.is_empty());
    Ok(())
}
#[test]
fn read_metadata_from_files_is_one_command_that_undo_reverses() -> Result<()> {
    use crate::metadata::{LangAlt, TextField, Value};
    let (dir, mut library) = library_of(&["a.ARW"])?;
    let id = library.session.photos[0].id;
    library
        .session
        .catalog
        .set_text(&[id], TextField::Title, "My edit")?;
    library.session.catalog.set_metadata(id, 1, 0, "Blue")?;
    library.session.photos[0].rating = 1;
    library.session.photos[0].label = "Blue".into();
    std::fs::write(
        dir.path().join("photos/a.ARW.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
          xmlns:xmp="http://ns.adobe.com/xap/1.0/" dc:title="From the file" xmp:Rating="5" xmp:Label="Red"/>
        </rdf:RDF></x:xmpmeta>"#,
    )?;
    library.read_metadata_from_files(&[id])?;
    // Read in the background, then written.
    while library.reread.is_some() {
        std::thread::sleep(std::time::Duration::from_millis(5));
        library.poll_reread();
    }
    assert_eq!(
        library.session.catalog.descriptive(id)?.title,
        Some(Value::Set(LangAlt::new("From the file")))
    );
    assert_eq!(
        (
            library.session.photos[0].rating,
            library.session.photos[0].label.as_str()
        ),
        (5, "Red")
    );
    let done = library.take_descriptive_done();
    assert_eq!(done.len(), 1);
    library.restore_descriptive(&done[0].before, &done[0].ratings_before)?;
    assert_eq!(
        library.session.catalog.descriptive(id)?.title,
        Some(Value::Set(LangAlt::new("My edit")))
    );
    assert_eq!(
        (
            library.session.photos[0].rating,
            library.session.photos[0].label.as_str()
        ),
        (1, "Blue")
    );
    Ok(())
}
#[test]
fn emptying_a_mixed_field_after_typing_clears_it_on_every_photo() -> Result<()> {
    use crate::metadata::{TextField, Value};
    let (_dir, mut library) = library_of(&["a.ARW", "b.ARW"])?;
    let ids: Vec<PhotoId> = library.session.photos.iter().map(|p| p.id).collect();
    library
        .session
        .catalog
        .set_text(&ids[..1], TextField::Title, "Only a")?;
    library.selection.selected = ids.iter().copied().collect();
    library.selection.active = Some(ids[0]);
    library.sync_fields();
    // Typed, then deleted again.
    library.fields.drafts.title = String::new();
    library.fields.mark_title_edited_for_tests();
    library.commit_fields()?;
    for id in &ids {
        assert_eq!(
            library.session.catalog.descriptive(*id)?.title,
            Some(Value::Cleared)
        );
    }
    Ok(())
}
#[test]
fn the_same_photos_in_another_order_keep_what_is_typed() -> Result<()> {
    let (_dir, mut library) = library_of(&["a.ARW", "b.ARW"])?;
    let ids: Vec<PhotoId> = library.session.photos.iter().map(|p| p.id).collect();
    library.selection.selected = ids.iter().copied().collect();
    library.selection.active = Some(ids[0]);
    library.sync_fields();
    library.fields.keyword_entry = "Typed".into();
    library.visible.reverse();
    library.sync_fields();
    assert_eq!(library.fields.keyword_entry, "Typed");
    Ok(())
}
#[test]
fn a_draft_in_a_hidden_section_is_saved_when_the_values_are_read_again() -> Result<()> {
    use crate::metadata::{LangAlt, Value};
    let (_dir, mut library) = library_of(&["a.ARW"])?;
    let id = library.session.photos[0].id;
    library.selection.selected = [id].into();
    library.selection.active = Some(id);
    library.sync_fields();
    library.fields.drafts.title = "Typed".into();
    // A keyword added meanwhile reads the values again.
    library.edit_descriptive(
        &[id],
        super::descriptive::DescriptiveEdit::AddKeywords(vec![vec!["K".into()]]),
    )?;
    library.sync_fields();
    assert_eq!(
        library.session.catalog.descriptive(id)?.title,
        Some(Value::Set(LangAlt::new("Typed")))
    );
    assert_eq!(library.fields.drafts.title, "Typed");
    // Read again without typing, nothing more is saved.
    let done = library.take_descriptive_done().len();
    library.fields.reload();
    library.sync_fields();
    assert!(library.take_descriptive_done().is_empty());
    assert_eq!(done, 2);
    Ok(())
}
#[test]
fn a_saved_draft_is_not_saved_again_after_undo() -> Result<()> {
    let (_dir, mut library) = library_of(&["a.ARW"])?;
    let id = library.session.photos[0].id;
    library.selection.selected = [id].into();
    library.selection.active = Some(id);
    library.sync_fields();
    library.fields.drafts.title = "Typed".into();
    library.commit_fields()?;
    let done = library.take_descriptive_done();
    assert_eq!(done.len(), 1);
    // Undone with the panel hidden: the old draft isn't saved again.
    library.restore_descriptive(&done[0].before, &done[0].ratings_before)?;
    library.commit_fields()?;
    assert!(library.take_descriptive_done().is_empty());
    assert_eq!(library.session.catalog.descriptive(id)?.title, None);
    Ok(())
}
#[test]
fn develop_filmstrip_cmd_and_shift_select_while_the_open_photo_stays_active() -> anyhow::Result<()>
{
    let (_d, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let ids = library.shown();
    library.select(Some(ids[1]));
    // A plain click is left to Develop, which opens the photo.
    assert!(!library.develop_select(ids[2], Some(ids[1]), egui::Modifiers::NONE));
    assert!(library.develop_select(ids[3], Some(ids[1]), egui::Modifiers::COMMAND));
    assert_eq!(library.selected(), Some(ids[1]));
    assert_eq!(library.selected_photos(), [ids[1], ids[3]]);
    // Cmd on a selected photo takes it out again; the open one stays.
    library.develop_select(ids[3], Some(ids[1]), egui::Modifiers::COMMAND);
    assert_eq!(library.selected_photos(), [ids[1]]);
    library.develop_select(ids[3], Some(ids[1]), egui::Modifiers::SHIFT);
    assert_eq!(library.selected(), Some(ids[1]));
    assert_eq!(library.selected_photos(), ids[1..]);
    Ok(())
}
