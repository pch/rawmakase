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

/// Announces a change to every file sent out, as the system's events do,
/// and looks at them until each change, if any, has been taken up, as the
/// passes do in front or behind: a look on its worker, then again once
/// SETTLE has passed (made to pass here).
fn follow(e: &mut Editor) {
    let paths = e.edit_in.sent.iter().map(|s| s.path.clone()).collect();
    e.sent_changed(paths);
    looks(e);
}

/// Looks at the files with a change announced, a few rounds, time made to
/// pass between them.
fn looks(e: &mut Editor) {
    let ctx = e.context.clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    for _ in 0..4 {
        if e.edit_in.due.is_some() {
            e.edit_in.due = Some(Instant::now());
        }
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
    // Sent as the other photo, whose file this isn't: not followed.
    e.launch_editor(dir.path(), &path, Some(&catalog), Some(ids[1]));
    std::fs::write(&path, b"a file put there by someone else")?;
    follow(&mut e);
    assert!(e.edit_in.sent.is_empty(), "not followed");
    // Sent as its own photo, then another catalog open: no longer followed.
    e.launch_editor(dir.path(), &path, Some(&catalog), Some(ids[0]));
    assert_eq!(e.edit_in.sent.len(), 1);
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
            size: (0, 0),
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
            size: (0, 0),
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
            size: (0, 0),
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

/// Files followed and resting cost nothing: no pass is asked for later, as
/// a timer would (a wake now is the system's event for a change, which the
/// real watcher here may send for the test's own writes). A change announced
/// (here a fake event, through the same channel as the system's) is looked
/// at, and passes asked for only until it is taken up.
#[test]
fn followed_files_at_rest_ask_for_no_pass() -> anyhow::Result<()> {
    let (dir, mut e, _) = editor_with_catalog(&[])?;
    let woken = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let record = woken.clone();
    let ctx = e.context.clone();
    ctx.set_request_repaint_callback(move |info| record.lock().unwrap().push(info.delay));
    let pass = |e: &mut Editor| {
        let mut output = ctx.run_ui(Default::default(), |ui| e.background(ui.ctx()));
        output.textures_delta.clear();
    };
    let file = dir.path().canonicalize()?.join("a-Edit.tif");
    std::fs::write(&file, b"rendered")?;
    e.edit_in.launched = Some(Vec::new());
    e.launch_editor(dir.path(), &file, None, None);
    // egui's own first passes ask for more; once it has settled, nothing does.
    for _ in 0..3 {
        pass(&mut e);
    }
    assert!(watch::follows(&e.edit_in.watch, &file));
    woken.lock().unwrap().clear();
    for _ in 0..3 {
        pass(&mut e);
    }
    let timed = |woken: &std::sync::Mutex<Vec<Duration>>| {
        woken.lock().unwrap().iter().any(|delay| !delay.is_zero())
    };
    assert!(!timed(&woken), "{:?}", woken.lock().unwrap());
    // A save, told by an event.
    std::fs::write(&file, b"saved again by the editor")?;
    let _ =
        e.tx.send(crate::app::worker::Event::EditInChanged(vec![file.clone()]));
    pass(&mut e);
    assert!(e.edit_in.sent[0].pending);
    looks(&mut e);
    assert_eq!(e.edit_in.sent[0].identity, Identity::read(&file)?);
    assert!(!e.edit_in.sent[0].pending);
    woken.lock().unwrap().clear();
    for _ in 0..3 {
        pass(&mut e);
    }
    assert!(!timed(&woken), "{:?}", woken.lock().unwrap());
    Ok(())
}

/// A save is told without any help, by the system's event for the folder.
#[test]
fn a_save_is_told_by_the_watch() -> anyhow::Result<()> {
    let (dir, mut e, _) = editor_with_catalog(&[])?;
    let file = dir.path().canonicalize()?.join("a-Edit.tif");
    std::fs::write(&file, b"rendered")?;
    e.edit_in.launched = Some(Vec::new());
    e.launch_editor(dir.path(), &file, None, None);
    let ctx = e.context.clone();
    e.watch_sent(&ctx);
    // The watching thread sets the watch up on its own; an editor's save
    // comes later than this.
    std::thread::sleep(Duration::from_millis(1000));
    // A save by deleting and writing again, as some editors do.
    std::fs::remove_file(&file)?;
    std::fs::write(&file, b"saved again by the editor")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !e.edit_in.sent[0].pending && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        e.events(&ctx);
    }
    assert!(e.edit_in.sent[0].pending, "told of the save");
    Ok(())
}

/// A save to a file whose folder the catalog reaches through a symbolic
/// link is told too: macOS names the folder by where the link leads.
#[cfg(unix)]
#[test]
fn a_save_through_a_linked_folder_is_told_by_the_watch() -> anyhow::Result<()> {
    let (dir, mut e, _) = editor_with_catalog(&[])?;
    let real = dir.path().canonicalize()?.join("real");
    std::fs::create_dir(&real)?;
    let link = dir.path().canonicalize()?.join("link");
    std::os::unix::fs::symlink(&real, &link)?;
    let file = link.join("a-Edit.tif");
    std::fs::write(&file, b"rendered")?;
    e.edit_in.launched = Some(Vec::new());
    e.launch_editor(dir.path(), &file, None, None);
    let ctx = e.context.clone();
    e.watch_sent(&ctx);
    std::thread::sleep(Duration::from_millis(1000));
    std::fs::write(&file, b"saved again by the editor")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !e.edit_in.sent[0].pending && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        e.events(&ctx);
    }
    assert!(
        e.edit_in.sent[0].pending,
        "told of the save through the link"
    );
    assert_eq!(
        e.edit_in.sent[0].path, file,
        "followed as the catalog names it"
    );
    Ok(())
}

/// The slow look tells of a change once, not at every look after.
#[test]
fn the_slow_look_tells_of_a_change_once() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a-Edit.tif");
    std::fs::write(&file, b"rendered")?;
    let followed = vec![(file.clone(), crate::storage::Stamp::read(&file)?)];
    let mut reported = std::collections::HashMap::new();
    assert!(watch::changed(&followed, &mut reported).is_empty());
    std::fs::write(&file, b"saved again by the editor")?;
    assert_eq!(
        watch::changed(&followed, &mut reported),
        std::slice::from_ref(&file)
    );
    assert!(watch::changed(&followed, &mut reported).is_empty());
    // Missing: nothing to tell.
    std::fs::remove_file(&file)?;
    assert!(watch::changed(&followed, &mut reported).is_empty());
    Ok(())
}

/// A file missing while its change is looked at (deleted and being written
/// again) is still followed, and its save taken up once it is back.
#[test]
fn a_file_missing_mid_save_is_still_followed() -> anyhow::Result<()> {
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
    std::fs::remove_file(&path)?;
    follow(&mut e);
    assert_eq!(e.edit_in.sent.len(), 1, "still followed");
    assert!(e.edit_in.sent[0].pending, "still looked at");
    std::fs::write(&path, b"saved again by the editor")?;
    looks(&mut e);
    assert_eq!(e.edit_in.sent[0].identity, Identity::read(&path)?);
    Ok(())
}

/// A save whose photo has left the catalog can never be taken up: the file
/// is no longer followed, once, rather than tried again and again.
#[test]
fn a_save_that_can_never_be_followed_ends_the_following() -> anyhow::Result<()> {
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
    e.launch_editor(dir.path(), &path, Some(&catalog), Some(ids[0]));
    std::fs::write(&path, b"saved again by the editor")?;
    let paths = vec![path.clone()];
    e.sent_changed(paths);
    // Its photo left the catalog between the save and its look.
    let mut sent = e.edit_in.sent[0].clone();
    sent.photo = Some(ids[1]);
    let now = Identity::read(&path)?;
    assert!(matches!(
        e.follow_save(&sent, &now),
        Err(NotFollowed::Never(_))
    ));
    e.edit_in.sent[0].photo = Some(ids[1]);
    let ctx = e.context.clone();
    e.watch_sent(&ctx);
    assert!(e.edit_in.sent.is_empty(), "no longer followed");
    Ok(())
}

/// With Edit Original, a photo and its virtual copy share one file, which
/// opens once.
#[test]
fn edit_original_opens_a_shared_file_once() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.JPG"])?;
    e.virtual_copy(crate::app::library::CopyAction::Create(ids[0]));
    let ids: Vec<PhotoId> = e
        .library
        .as_ref()
        .unwrap()
        .session
        .photos()
        .iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids.len(), 2);
    ask(&mut e, dir.path(), ids);
    let asking = e.edit_in.asking.take().expect("asked how");
    e.edit_in_as(asking);
    assert_eq!(e.edit_in.launched.as_ref().unwrap().len(), 1);
    assert_eq!(e.edit_in.sent.len(), 1);
    Ok(())
}

#[test]
fn a_copy_is_named_after_its_original_with_its_own_extension() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("IMG_1.JPG");
    assert_eq!(copy_name(&source, &[]), dir.path().join("IMG_1-Edit.JPG"));
    std::fs::write(dir.path().join("IMG_1-Edit.JPG"), b"taken")?;
    assert_eq!(copy_name(&source, &[]), dir.path().join("IMG_1-Edit-2.JPG"));
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
            size: (0, 0),
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

/// A catalog of one small TIFF, `name`, in an editor, its window behind
/// another: frames run unfocused, as a second screen leaves it.
fn unfocused_editor_with_tiff(
    name: &str,
) -> anyhow::Result<(tempfile::TempDir, Editor, PhotoId, PathBuf)> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().canonicalize()?.join("photos");
    std::fs::create_dir(&photos)?;
    let tiff = photos.join(name);
    image::RgbImage::from_pixel(64, 48, image::Rgb([200, 120, 40])).save(&tiff)?;
    let path = dir.path().join("t.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = eframe::egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, Default::default(), None);
    e.onboarding.visible = false;
    e.library = Some(Box::new(crate::app::library::Library::load(&path, ctx)?));
    let id = e.library.as_ref().unwrap().session.photos()[0].id;
    e.edit_in.launched = Some(Vec::new());
    Ok((dir, e, id, tiff))
}

/// Frames, unfocused, until `done` or a few seconds.
fn frames_until(e: &mut Editor, done: impl Fn(&Editor) -> bool) -> bool {
    use eframe::egui;
    let ctx = e.context.clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1400., 900.),
                )),
                focused: false,
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
        if done(e) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// A save in the editor, told as the system's event tells it.
fn save_as(e: &mut Editor, file: &Path, width: u32) -> anyhow::Result<()> {
    image::RgbImage::from_pixel(width, width * 3 / 4, image::Rgb([40, 120, 200])).save(file)?;
    let _ = e.tx.send(crate::app::worker::Event::EditInChanged(vec![
        file.to_path_buf(),
    ]));
    Ok(())
}

/// Edit In from Develop: the copy takes the original's place, selected and
/// open in Develop, so the photo shown is the file being edited; a save to
/// it then shows there, with the window behind the editor.
#[test]
fn the_copy_opens_in_place_of_the_photo_open_and_shows_its_saves() -> anyhow::Result<()> {
    let (_dir, mut e, id, original) = unfocused_editor_with_tiff("IMG_1.tif")?;
    e.develop_catalog_photo(id);
    let loaded = |e: &Editor| !e.document.loading() && e.document.full().is_some_and(|f| !f.fast);
    assert!(frames_until(&mut e, loaded));
    let tiff = original.with_file_name("IMG_1-Edit.tif");
    image::RgbImage::from_pixel(64, 48, image::Rgb([90, 90, 90])).save(&tiff)?;
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
            editor: original.parent().unwrap().to_path_buf(),
            catalog: Some(catalog),
            originals: vec![Some((id, original.clone()))],
        },
        &[Outcome::Exported {
            path: tiff.clone(),
            notes: Vec::new(),
            size: (0, 0),
        }],
    );
    assert!(frames_until(&mut e, |e| e
        .catalog_photo_at(&tiff)
        .is_some()));
    let copy = e.catalog_photo_at(&tiff).unwrap();
    assert!(frames_until(&mut e, loaded));
    assert_eq!(e.document.catalog_photo, Some(copy), "the copy is open");
    assert_eq!(e.document.path.as_deref(), Some(tiff.as_path()));
    assert_eq!(e.library.as_ref().unwrap().selected(), Some(copy));
    save_as(&mut e, &tiff, 80)?;
    assert!(
        frames_until(&mut e, |e| loaded(e)
            && e.document.full().is_some_and(|f| f.width == 80)),
        "the save shows in Develop"
    );
    Ok(())
}

/// The Library's Loupe shows its own preview of an unedited TIFF: a save
/// makes it again, with the window behind the editor.
#[test]
fn the_loupe_shows_a_save() -> anyhow::Result<()> {
    let (dir, mut e, id, tiff) = unfocused_editor_with_tiff("IMG_1-Edit.tif")?;
    e.module = crate::app::Module::Library;
    e.library.as_mut().unwrap().select(Some(id));
    e.library.as_mut().unwrap().open_loupe();
    let size = |e: &Editor| e.library.as_ref().unwrap().loupe_texture_size_for_tests();
    assert!(frames_until(&mut e, |e| size(e) == Some([64, 48])));
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    e.launch_editor(dir.path(), &tiff, Some(&catalog), Some(id));
    save_as(&mut e, &tiff, 80)?;
    assert!(
        frames_until(&mut e, |e| size(e) == Some([80, 60])),
        "the save shows in the Loupe: {:?}",
        size(&e)
    );
    Ok(())
}

/// Copies made of the photos selected are selected in their stead, the copy
/// of the active photo active.
#[test]
fn the_copies_are_selected_in_place_of_their_originals() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.JPG", "IMG_2.JPG", "IMG_3.JPG"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[1]));
    library.select_all();
    library.make_active(ids[1]);
    let paths: Vec<PathBuf> = ids[..2]
        .iter()
        .map(|id| library.photo(*id).unwrap().path.clone())
        .collect();
    let catalog = library.session.catalog().location().clone();
    e.edit_in.launched = Some(Vec::new());
    e.edit_in_as(Asking {
        catalog,
        ids: ids[..2].to_vec(),
        plain: vec![(ids[0], paths[0].clone()), (ids[1], paths[1].clone())],
        editor: dir.path().to_path_buf(),
        how: How::Copy,
        adjusted: false,
    });
    settle(&mut e);
    let copies: Vec<PhotoId> = ["IMG_1-Edit.JPG", "IMG_2-Edit.JPG"]
        .iter()
        .map(|name| e.catalog_photo_at(&paths[0].with_file_name(name)).unwrap())
        .collect();
    let library = e.library.as_ref().unwrap();
    let mut selected = library.selected_ids();
    selected.sort();
    let mut wanted = vec![copies[0], copies[1], ids[2]];
    wanted.sort();
    assert_eq!(selected, wanted);
    assert_eq!(library.selected(), Some(copies[1]));
    Ok(())
}

/// Opened again in its editor after a save not taken up yet: the version it
/// is followed from stays the one the catalog's edit is bound to, so the
/// save is still taken up and the edit follows it.
#[test]
fn opening_a_file_again_keeps_the_version_it_is_followed_from() -> anyhow::Result<()> {
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
    let was = Identity::read(&path)?;
    let recipe = crate::model::recipe::Recipe {
        exposure: 0.5,
        ..Default::default()
    };
    e.library.as_mut().unwrap().session.save_edit(
        ids[0],
        &path,
        &was,
        &recipe,
        &Default::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    e.launch_editor(dir.path(), &path, Some(&catalog), Some(ids[0]));
    std::fs::write(&path, b"saved again by the editor")?;
    e.launch_editor(dir.path(), &path, Some(&catalog), Some(ids[0]));
    assert_eq!(e.edit_in.sent[0].identity, was);
    looks(&mut e);
    assert_eq!(e.edit_in.sent[0].identity, Identity::read(&path)?);
    let library = e.library.as_ref().unwrap();
    let saved = library.session.catalog().load_edit(ids[0], &path)?;
    assert_eq!(saved.map(|s| s.recipe.exposure), Some(0.5));
    Ok(())
}

/// A save taken up while the photo shown can't be opened again yet (the file
/// changed once more before its edit could be saved) is taken up all the
/// same; the next save follows on from it, and the change not saved in
/// Develop is kept.
#[test]
fn a_save_followed_by_another_before_the_reopen_keeps_every_edit() -> anyhow::Result<()> {
    let (_dir, mut e, id, tiff) = unfocused_editor_with_tiff("IMG_1-Edit.tif")?;
    e.develop_catalog_photo(id);
    let loaded = |e: &Editor| !e.document.loading() && e.document.full().is_some_and(|f| !f.fast);
    assert!(frames_until(&mut e, loaded));
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    e.launch_editor(tiff.parent().unwrap(), &tiff, Some(&catalog), Some(id));
    e.change_edit(None, |r| r.exposure = 0.7);
    // B saved and settled; C lands before B is taken up.
    image::RgbImage::from_pixel(72, 54, image::Rgb([1, 2, 3])).save(&tiff)?;
    let b = Identity::read(&tiff)?;
    image::RgbImage::from_pixel(80, 60, image::Rgb([4, 5, 6])).save(&tiff)?;
    let announced = e.edit_in.sent[0].announced;
    e.edit_in.sent[0].pending = true;
    e.sent_looked(Looked {
        looks: vec![(tiff.clone(), announced, Look::Settled(b.clone()))],
        at: Instant::now(),
    });
    assert_eq!(e.edit_in.sent[0].identity, b, "B taken up");
    assert!(e.edit_in.sent[0].reopen, "its reopening waits");
    // C is announced and taken up from B.
    let _ =
        e.tx.send(crate::app::worker::Event::EditInChanged(vec![tiff.clone()]));
    looks(&mut e);
    assert!(frames_until(&mut e, |e| loaded(e)
        && e.document.full().is_some_and(|f| f.width == 80)));
    assert_eq!(e.edit_in.sent[0].identity, Identity::read(&tiff)?);
    assert_eq!(e.document.edit.recipe().exposure, 0.7);
    assert!(!e.document.edit.save_state().is_protected());
    let library = e.library.as_ref().unwrap();
    let saved = library.session.catalog().load_edit(id, &tiff)?;
    assert_eq!(saved.map(|s| s.recipe.exposure), Some(0.7));
    Ok(())
}

/// A name taken by a link that leads nowhere is passed over, not tried
/// again and again.
#[cfg(unix)]
#[test]
fn a_copy_passes_over_a_name_a_dangling_link_takes() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.JPG"])?;
    let path = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    std::os::unix::fs::symlink(
        dir.path().join("nowhere"),
        path.with_file_name("IMG_1-Edit.JPG"),
    )?;
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
        ids: vec![ids[0]],
        plain: vec![(ids[0], path.clone())],
        editor: dir.path().to_path_buf(),
        how: How::Copy,
        adjusted: false,
    });
    settle(&mut e);
    let launched = &e.edit_in.launched.as_ref().unwrap()[0].1;
    assert_eq!(*launched, path.with_file_name("IMG_1-Edit-2.JPG"));
    assert_eq!(
        copy_name(&path, &[]),
        path.with_file_name("IMG_1-Edit-3.JPG")
    );
    Ok(())
}

/// A look started before a change was announced can't settle it, and an
/// earlier launch's failure doesn't end a later launch's following.
#[test]
fn stale_results_leave_newer_news_standing() -> anyhow::Result<()> {
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
    let before = e.edit_in.sent[0].announced;
    e.sent_changed(vec![path.clone()]);
    e.sent_looked(Looked {
        looks: vec![(path.clone(), before, Look::Same)],
        at: Instant::now(),
    });
    assert!(e.edit_in.sent[0].pending, "the newer change stands");
    let first = e.edit_in.sent[0].launch;
    e.launch_editor(dir.path(), &path, Some(&catalog), Some(ids[0]));
    e.editor_launched(Launched {
        file: path.clone(),
        launch: first,
        program: "Editor".into(),
        result: Err("gone".into()),
    });
    assert_eq!(e.edit_in.sent.len(), 1, "still followed");
    Ok(())
}

/// The watching thread ends, dropping the system watcher, once asked; a
/// folder on this computer's own drive gets no slow look.
#[test]
fn the_watch_ends_on_its_own_thread() -> anyhow::Result<()> {
    let (dir, mut e, _) = editor_with_catalog(&[])?;
    assert!(!watch::looks_at(&dir.path().canonicalize()?));
    let file = dir.path().canonicalize()?.join("a-Edit.tif");
    std::fs::write(&file, b"rendered")?;
    e.edit_in.launched = Some(Vec::new());
    e.launch_editor(dir.path(), &file, None, None);
    let ctx = e.context.clone();
    e.watch_sent(&ctx);
    let thread = e.edit_in.stop_watching().expect("a thread");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(thread.is_finished());
    Ok(())
}

/// In a collection the copies aren't in, the copy of the active photo is
/// shown and the selection kept, so each copy takes its original's place.
#[test]
fn copies_keep_the_selection_in_a_collection() -> anyhow::Result<()> {
    let (dir, mut e, ids) = editor_with_catalog(&["IMG_1.JPG", "IMG_2.JPG"])?;
    let library = e.library.as_mut().unwrap();
    let trip = library.create_collection(false, "Trip", None, &ids, false)?;
    library.select_collection(trip);
    library.select(Some(ids[0]));
    library.select_all();
    library.make_active(ids[0]);
    let paths: Vec<PathBuf> = ids
        .iter()
        .map(|id| library.photo(*id).unwrap().path.clone())
        .collect();
    let catalog = library.session.catalog().location().clone();
    e.edit_in.launched = Some(Vec::new());
    e.edit_in_as(Asking {
        catalog,
        ids: ids.clone(),
        plain: vec![(ids[0], paths[0].clone()), (ids[1], paths[1].clone())],
        editor: dir.path().to_path_buf(),
        how: How::Copy,
        adjusted: false,
    });
    settle(&mut e);
    let copies: Vec<PhotoId> = ["IMG_1-Edit.JPG", "IMG_2-Edit.JPG"]
        .iter()
        .map(|name| e.catalog_photo_at(&paths[0].with_file_name(name)).unwrap())
        .collect();
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.selected(), Some(copies[0]));
    let mut selected = library.selected_ids();
    selected.sort();
    assert_eq!(selected, copies);
    Ok(())
}
