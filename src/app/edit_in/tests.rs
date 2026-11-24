//! Edit In's settings, its TIFFs' way into the catalog and to their program
//! (with a stub in place of starting it), and following a file saved again.
use super::*;
use crate::app::testing::editor_with_catalog;
use crate::export::batch::Outcome;
use std::time::{Duration, Instant};

#[test]
fn the_settings_name_a_16_bit_tiff_beside_the_original() {
    let settings = settings(OutputSpace::ProPhoto);
    let source = Path::new("/photos/DSC_3150.ARW");
    let (name, notes) = settings.file_name_for(source, &crate::export_settings::NameContext::ONE);
    assert_eq!(name, "DSC_3150-Edit.tif");
    assert!(notes.is_empty());
    assert_eq!(
        settings.base_folder(source).as_deref(),
        Some(Path::new("/photos"))
    );
    assert_eq!(settings.format, Format::Tiff);
    assert_eq!(settings.color_space, OutputSpace::ProPhoto);
    assert!(!settings.resize && !settings.watermark && settings.hdr.is_none());
    // The edit is in the pixels: its settings would be applied again.
    assert!(!settings.develop);
    settings.check_output().unwrap();
    for space in [OutputSpace::AdobeRgb, OutputSpace::Srgb] {
        assert_eq!(super::settings(space).color_space, space);
    }
}

#[test]
fn the_color_space_reads_from_the_session_and_defaults_to_prophoto() {
    assert_eq!(EditIn::new(Vec::new(), None).space, OutputSpace::ProPhoto);
    assert_eq!(
        EditIn::new(Vec::new(), Some("adobe-rgb")).space,
        OutputSpace::AdobeRgb
    );
    assert_eq!(
        EditIn::new(Vec::new(), Some("a later space")).space,
        OutputSpace::ProPhoto
    );
}

#[test]
fn a_save_is_taken_up_once_it_has_settled_and_a_missing_file_dropped() -> anyhow::Result<()> {
    use crate::storage::Stamp;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a-Edit.tif");
    std::fs::write(&file, b"rendered")?;
    let baseline = Stamp::read(&file)?;
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    assert_eq!(look(&file, baseline, None, at(0)), Look::Same);
    std::fs::write(&file, b"saved again by the editor")?;
    let stamp = Stamp::read(&file)?;
    // Seen: not taken up until it has stayed as it is for SETTLE.
    assert_eq!(look(&file, baseline, None, at(0)), Look::Changing(stamp));
    let seen = Some((stamp, at(0)));
    assert_eq!(look(&file, baseline, seen, at(250)), Look::Changing(stamp));
    assert_eq!(
        look(&file, baseline, seen, at(300)),
        Look::Settled(Identity::read(&file)?)
    );
    // Still being written since it was seen: not settled.
    std::fs::write(&file, b"saved again by the editor, more")?;
    let more = Stamp::read(&file)?;
    assert_eq!(look(&file, baseline, seen, at(600)), Look::Changing(more));
    std::fs::remove_file(&file)?;
    assert_eq!(look(&file, baseline, seen, at(900)), Look::Gone);
    Ok(())
}

/// Looks at the files sent out until each one's change, if any, has been
/// taken up, as the passes do in front or behind: a look on its worker,
/// then again once SETTLE has passed.
fn follow(e: &mut Editor) {
    let ctx = e.context.clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    for _ in 0..3 {
        e.edit_in.checked = None;
        e.watch_sent(&ctx);
        while e.edit_in.looking && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
            e.events(&ctx);
        }
        for sent in &mut e.edit_in.sent {
            if let Some((_, since)) = &mut sent.seen {
                *since -= SETTLE;
            }
        }
    }
}

/// A change that can't be taken up yet (the catalog busy) stays pending,
/// the file's version as it was, and is taken up at a later look.
#[test]
fn a_save_that_cant_be_followed_yet_is_tried_again() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.JPG"])?;
    e.edit_in.launched = Some(Vec::new());
    let path = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    e.launch_editor(dir.path(), &path, Some(&catalog), Some(ids[0]));
    let was = e.edit_in.sent[0].identity.clone();
    std::fs::write(&path, b"saved again by the editor")?;
    assert!(e.activity.begin_folder_change());
    follow(&mut e);
    assert_eq!(e.edit_in.sent[0].identity, was, "still the version before");
    assert!(e.edit_in.sent[0].seen.is_some(), "still pending");
    e.activity.finish_dialog();
    follow(&mut e);
    assert_eq!(e.edit_in.sent[0].identity, Identity::read(&path)?);
    assert!(e.edit_in.sent[0].seen.is_none());
    Ok(())
}

/// A file sent from a catalog stops being followed once another is open, and
/// a save is not taken up for a photo that is no longer that file.
#[test]
fn a_save_is_followed_only_for_its_catalog_and_photo() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.JPG", "IMG_2.JPG"])?;
    e.edit_in.launched = Some(Vec::new());
    let path = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    // Sent as the other photo, whose file this isn't.
    e.launch_editor(dir.path(), &path, Some(&catalog), Some(ids[1]));
    let was = e.edit_in.sent[0].identity.clone();
    std::fs::write(&path, b"a file put there by someone else")?;
    follow(&mut e);
    assert_eq!(e.edit_in.sent[0].identity, was, "not taken up");
    // Another catalog open: no longer followed.
    let (_other, other, _) = editor_with_catalog(&["IMG_9.JPG"])?;
    e.library = other.library;
    let ctx = e.context.clone();
    e.watch_sent(&ctx);
    assert!(e.edit_in.sent.is_empty());
    Ok(())
}

/// Delivers events, as frames do, until the TIFFs waiting are added.
fn settle(e: &mut Editor) {
    let ctx = e.context.clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    while (e.edit_in.adding.is_some() || !e.edit_in.waiting.is_empty()) && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(5));
        e.events(&ctx);
        e.add_edits();
    }
    assert!(e.edit_in.adding.is_none() && e.edit_in.waiting.is_empty());
    assert!(!e.activity.is_busy());
}

#[test]
fn a_tiff_is_added_stacked_on_its_original_and_opened_then_followed() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.CR3"])?;
    e.edit_in.launched = Some(Vec::new());
    let editor = dir.path().join("Editor.app");
    // Beside its original, as the catalog spells the original's path.
    let tiff = dir.path().canonicalize()?.join("photos/IMG_1-Edit.tif");
    std::fs::write(&tiff, b"rendered")?;
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    e.edit_in_rendered(
        Sending {
            editor: editor.clone(),
            catalog: Some(catalog),
            originals: vec![Some((
                ids[0],
                e.library
                    .as_ref()
                    .unwrap()
                    .photo(ids[0])
                    .unwrap()
                    .path
                    .clone(),
            ))],
        },
        &[Outcome::Exported {
            path: tiff.clone(),
            notes: Vec::new(),
        }],
    );
    assert_eq!(
        e.closing_waits_for().as_deref(),
        Some(crate::app::work::FOLDER_CHANGE)
    );
    settle(&mut e);
    let photo = e
        .catalog_photo_at(&tiff)
        .unwrap_or_else(|| panic!("the TIFF is in the Library: {}", e.messages.line()));
    let library = e.library.as_ref().unwrap();
    let stack = library.photo(photo).unwrap().stack.expect("stacked");
    assert!(stack.is_top());
    assert_eq!(stack.count, 2);
    assert_eq!(
        library.photo(ids[0]).unwrap().stack.map(|s| s.id),
        Some(stack.id)
    );
    assert_eq!(e.edit_in.launched, Some(vec![(editor, tiff)]));
    assert_eq!(e.messages.line(), "Opened IMG_1-Edit.tif in Editor");

    Ok(())
}

/// A TIFF shown in Develop with a change not saved yet, saved again by its
/// editor: the change is saved for the new version and the photo opens
/// again with it, decoded from the new file, its edit not protected.
#[test]
fn a_save_reopens_the_photo_shown_keeping_its_unsaved_change() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().canonicalize()?.join("photos");
    std::fs::create_dir(&photos)?;
    let tiff = photos.join("IMG_1-Edit.tif");
    image::RgbImage::from_pixel(64, 48, image::Rgb([200, 120, 40])).save(&tiff)?;
    let path = dir.path().join("t.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = eframe::egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, Default::default(), None);
    e.library = Some(Box::new(crate::app::library::Library::load(
        &path,
        ctx.clone(),
    )?));
    let id = e.library.as_ref().unwrap().session.photos()[0].id;
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    let opened = |e: &mut Editor| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while (e.document.loading() || e.document.full().is_none_or(|f| f.fast))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
            e.events(&ctx);
        }
        assert!(!e.document.loading() && e.document.full().is_some_and(|f| !f.fast));
    };
    e.develop_catalog_photo(id);
    opened(&mut e);
    e.edit_in.launched = Some(Vec::new());
    e.launch_editor(dir.path(), &tiff, Some(&catalog), Some(id));
    e.change_edit(None, |r| r.exposure = 0.7);
    image::RgbImage::from_pixel(80, 60, image::Rgb([40, 120, 200])).save(&tiff)?;
    let now = Identity::read(&tiff)?;
    follow(&mut e);
    assert_eq!(e.edit_in.sent[0].identity, now);
    opened(&mut e);
    assert_eq!(
        e.document.full().unwrap().width,
        80,
        "decoded from the new file"
    );
    assert_eq!(e.document.edit.recipe().exposure, 0.7);
    assert!(!e.document.edit.save_state().is_protected());
    let library = e.library.as_ref().unwrap();
    let saved = library.session.catalog().load_edit(id, &tiff)?;
    assert_eq!(saved.map(|s| s.recipe.exposure), Some(0.7));
    Ok(())
}

#[test]
fn without_a_catalog_the_tiff_opens_at_once() -> anyhow::Result<()> {
    let (dir, mut e, _) = editor_with_catalog(&[])?;
    e.edit_in.launched = Some(Vec::new());
    let tiff = dir.path().join("photo-Edit.tif");
    std::fs::write(&tiff, b"rendered")?;
    let editor = dir.path().join("Editor.app");
    e.edit_in_rendered(
        Sending {
            editor: editor.clone(),
            catalog: None,
            originals: vec![None],
        },
        &[Outcome::Exported {
            path: tiff.clone(),
            notes: Vec::new(),
        }],
    );
    assert!(!e.activity.is_busy());
    assert_eq!(e.edit_in.launched, Some(vec![(editor, tiff)]));
    Ok(())
}

#[test]
fn edit_in_with_no_editor_opens_preferences_on_its_page() -> anyhow::Result<()> {
    let (_dir, mut e, ids) = editor_with_catalog(&["IMG_1.CR3"])?;
    e.edit_in(ids, 0);
    assert!(e.preferences.open);
    assert!(e.preferences.tab() == crate::app::preferences::Tab::ExternalEditing);
    Ok(())
}

#[test]
fn a_program_gone_is_said_and_nothing_rendered() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.CR3"])?;
    e.edit_in.set_editors(vec![dir.path().join("Gone.app")]);
    e.edit_in(ids, 0);
    assert!(
        e.messages.line().contains("Gone is no longer at"),
        "{}",
        e.messages.line()
    );
    assert!(!e.exporting());
    Ok(())
}

/// An Edit In TIFF shows its thumbnail as soon as the grid draws it: it is
/// asked for in the frame that adds it and made from the file just written.
#[test]
fn an_added_tiff_shows_its_thumbnail_at_once() -> anyhow::Result<()> {
    use eframe::egui;
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.CR3"])?;
    e.onboarding.visible = false;
    e.edit_in.launched = Some(Vec::new());
    let tiff = dir.path().canonicalize()?.join("photos/IMG_1-Edit.tif");
    image::RgbImage::from_pixel(64, 48, image::Rgb([200, 120, 40])).save(&tiff)?;
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    let ctx = e.context.clone();
    let frame = |e: &mut Editor| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1200., 800.),
                )),
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
    };
    frame(&mut e);
    e.edit_in_rendered(
        Sending {
            editor: dir.path().join("Editor.app"),
            catalog: Some(catalog),
            originals: vec![Some((
                ids[0],
                e.library
                    .as_ref()
                    .unwrap()
                    .photo(ids[0])
                    .unwrap()
                    .path
                    .clone(),
            ))],
        },
        &[Outcome::Exported {
            path: tiff.clone(),
            notes: Vec::new(),
        }],
    );
    let started = Instant::now();
    let shown = loop {
        frame(&mut e);
        let photo = e.catalog_photo_at(&tiff);
        if let Some(photo) = photo
            && e.library.as_ref().unwrap().thumbnail(photo).is_some()
        {
            break started.elapsed();
        }
        assert!(started.elapsed() < Duration::from_secs(5), "no thumbnail");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(shown < Duration::from_secs(2), "{shown:?}");
    Ok(())
}

/// While files are sent out the passes keep coming every 150 ms, as
/// nothing else wakes a window behind the editor; with none, nothing does.
#[test]
fn files_sent_out_keep_the_window_waking() -> anyhow::Result<()> {
    let (dir, mut e, _) = editor_with_catalog(&[])?;
    let woken = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let record = woken.clone();
    let ctx = e.context.clone();
    ctx.set_request_repaint_callback(move |info| record.lock().unwrap().push(info.delay));
    let pass = |e: &mut Editor| {
        let mut output = ctx.run_ui(Default::default(), |ui| e.watch_sent(ui.ctx()));
        output.textures_delta.clear();
    };
    // egui's own first passes ask for more; once it has settled, nothing does.
    for _ in 0..3 {
        pass(&mut e);
    }
    woken.lock().unwrap().clear();
    pass(&mut e);
    assert!(
        woken.lock().unwrap().is_empty(),
        "{:?}",
        woken.lock().unwrap()
    );
    let file = dir.path().join("a-Edit.tif");
    std::fs::write(&file, b"rendered")?;
    e.edit_in.sent.push(Sent {
        identity: Identity::read(&file)?,
        path: file,
        catalog: None,
        photo: None,
        seen: None,
        failing: false,
    });
    pass(&mut e);
    let woken = woken.lock().unwrap();
    assert!(
        woken
            .iter()
            .any(|delay| *delay <= Duration::from_millis(150)),
        "{woken:?}"
    );
    Ok(())
}

#[test]
fn a_copy_is_named_after_its_original_with_its_own_extension() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("IMG_1.JPG");
    assert_eq!(copy_name(&source), dir.path().join("IMG_1-Edit.JPG"));
    std::fs::write(dir.path().join("IMG_1-Edit.JPG"), b"taken")?;
    assert_eq!(copy_name(&source), dir.path().join("IMG_1-Edit-2.JPG"));
    Ok(())
}

/// An editor that exists, and Edit In on `ids` with it.
fn ask(e: &mut Editor, dir: &Path, ids: Vec<PhotoId>) {
    e.edit_in.launched = Some(Vec::new());
    e.edit_in.set_editors(vec![dir.to_path_buf()]);
    e.edit_in(ids, 0);
}

#[test]
fn a_raw_is_rendered_without_asking() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.CR3"])?;
    ask(&mut e, dir.path(), ids);
    assert!(e.edit_in.asking.is_none());
    assert!(e.exporting());
    Ok(())
}

#[test]
fn edit_original_opens_the_file_itself_and_follows_it() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.JPG"])?;
    ask(&mut e, dir.path(), ids.clone());
    let asking = e.edit_in.asking.take().expect("asked how");
    assert_eq!(asking.how, How::Original, "the default");
    assert!(!asking.adjusted);
    e.edit_in_as(asking);
    let original = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    assert_eq!(
        e.edit_in.launched,
        Some(vec![(dir.path().to_path_buf(), original.clone())])
    );
    assert_eq!(e.edit_in.sent[0].path, original);
    assert!(!e.exporting());
    assert_eq!(e.library.as_ref().unwrap().session.photos().len(), 1);
    Ok(())
}

#[test]
fn edit_a_copy_adds_the_file_as_it_is_stacked_on_top() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.JPG"])?;
    for name in ["IMG_1-Edit.JPG", "IMG_1-Edit-2.JPG"] {
        ask(&mut e, dir.path(), vec![ids[0]]);
        let mut asking = e.edit_in.asking.take().expect("asked how");
        asking.how = How::Copy;
        e.edit_in_as(asking);
        settle(&mut e);
        assert!(!e.exporting());
        let copy = dir.path().canonicalize()?.join("photos").join(name);
        assert_eq!(std::fs::read(&copy)?, b"fixture", "copied as it is");
        let photo = e
            .catalog_photo_at(&copy)
            .unwrap_or_else(|| panic!("{name} is in the Library: {}", e.messages.line()));
        let library = e.library.as_ref().unwrap();
        assert!(
            library
                .photo(photo)
                .unwrap()
                .stack
                .expect("stacked")
                .is_top()
        );
        assert_eq!(e.edit_in.launched.as_ref().unwrap().last().unwrap().1, copy);
    }
    Ok(())
}

#[test]
fn a_copy_with_adjustments_is_rendered_and_raws_always_are() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.CR3", "IMG_2.JPG"])?;
    ask(&mut e, dir.path(), ids);
    let asking = e.edit_in.asking.take().expect("asked about the JPEG");
    assert_eq!(asking.plain.len(), 1);
    // Edit Original for the JPEG: the RAW is still rendered.
    e.edit_in_as(asking);
    assert!(e.exporting());
    assert_eq!(e.edit_in.launched.as_ref().unwrap().len(), 1);

    let (dir, mut e, ids) = editor_with_catalog(&["IMG_2.JPG"])?;
    ask(&mut e, dir.path(), ids);
    let mut asking = e.edit_in.asking.take().expect("asked how");
    asking.how = How::Adjusted;
    e.edit_in_as(asking);
    assert!(e.exporting());
    assert_eq!(e.edit_in.launched, Some(Vec::new()));
    Ok(())
}

/// Edit In's question for a JPEG with an edit, for `RAWMAKASE_UI_SHOTS`
/// ([`crate::app::ui::shots`]).
#[test]
fn shots_of_the_edit_in_question() {
    crate::app::ui::shots::shot(
        "edit-in-question",
        eframe::egui::Vec2::new(900., 600.),
        |e, _| {
            e.onboarding.visible = false;
            // The question is about the catalog open.
            let (dir, other, _) = editor_with_catalog(&["IMG_2.JPG"]).unwrap();
            std::mem::forget(dir);
            e.library = other.library;
            let catalog = e
                .library
                .as_ref()
                .unwrap()
                .session
                .catalog()
                .location()
                .clone();
            e.edit_in.asking = Some(Asking {
                catalog,
                ids: vec![PhotoId(1), PhotoId(2)],
                plain: vec![(PhotoId(2), PathBuf::from("/photos/IMG_2.JPG"))],
                editor: PathBuf::from("/Applications/Affinity.app"),
                how: How::Original,
                adjusted: true,
            });
        },
        |e, ctx| e.edit_in_window(ctx),
    );
}

/// Two copies of one file asked for together (a photo and its virtual copy)
/// each get a name of their own: the second never takes the first's file.
#[test]
fn copies_of_one_file_are_each_published_under_their_own_name() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.JPG"])?;
    let path = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    e.edit_in.launched = Some(Vec::new());
    e.edit_in_as(Asking {
        catalog,
        ids: vec![ids[0], ids[0]],
        plain: vec![(ids[0], path.clone()), (ids[0], path)],
        editor: dir.path().to_path_buf(),
        how: How::Copy,
        adjusted: false,
    });
    settle(&mut e);
    let photos = dir.path().canonicalize()?.join("photos");
    let launched: Vec<PathBuf> = e
        .edit_in
        .launched
        .as_ref()
        .unwrap()
        .iter()
        .map(|l| l.1.clone())
        .collect();
    assert_eq!(
        launched,
        [
            photos.join("IMG_1-Edit.JPG"),
            photos.join("IMG_1-Edit-2.JPG")
        ]
    );
    for copy in &launched {
        assert_eq!(std::fs::read(copy)?, b"fixture");
    }
    // Nothing staged is left behind.
    let left: Vec<_> = std::fs::read_dir(&photos)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".partial"))
        .collect();
    assert!(left.is_empty());
    Ok(())
}

/// A TIFF whose photo has gone, its id now another photo's, is added but
/// stacked with nothing.
#[test]
fn a_tiff_is_stacked_only_with_the_photo_it_was_made_from() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.CR3"])?;
    e.edit_in.launched = Some(Vec::new());
    let tiff = dir.path().canonicalize()?.join("photos/IMG_1-Edit.tif");
    std::fs::write(&tiff, b"rendered")?;
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    e.edit_in_rendered(
        Sending {
            editor: dir.path().to_path_buf(),
            catalog: Some(catalog),
            // Made from a photo at another file, whose id this one has now.
            originals: vec![Some((ids[0], dir.path().join("photos/GONE.CR3")))],
        },
        &[Outcome::Exported {
            path: tiff.clone(),
            notes: Vec::new(),
        }],
    );
    settle(&mut e);
    let photo = e.catalog_photo_at(&tiff).expect("added");
    assert!(
        e.library
            .as_ref()
            .unwrap()
            .photo(photo)
            .unwrap()
            .stack
            .is_none()
    );
    Ok(())
}

/// The question is about the catalog it was asked in: with another open, it
/// closes and does nothing there.
#[test]
fn the_question_ends_with_its_catalog() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.JPG"])?;
    e.edit_in.launched = Some(Vec::new());
    e.edit_in.set_editors(vec![dir.path().to_path_buf()]);
    e.edit_in(ids, 0);
    let asking = e.edit_in.asking.take().expect("asked");
    let (_other_dir, other, _) = editor_with_catalog(&["IMG_1.JPG"])?;
    e.library = other.library;
    e.edit_in.asking = Some(asking);
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(Default::default(), |ui| e.edit_in_window(ui.ctx()));
    output.textures_delta.clear();
    assert!(e.edit_in.asking.is_none());
    assert!(e.messages.line().contains("another catalog"));
    assert_eq!(e.edit_in.launched, Some(Vec::new()));
    Ok(())
}
