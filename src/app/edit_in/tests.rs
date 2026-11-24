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
fn a_file_saved_again_is_found_once_and_a_missing_one_forgotten() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (kept, saved, gone) = (
        dir.path().join("a-Edit.tif"),
        dir.path().join("b-Edit.tif"),
        dir.path().join("c-Edit.tif"),
    );
    for path in [&kept, &saved, &gone] {
        std::fs::write(path, b"rendered")?;
    }
    let mut sent: Vec<Sent> = [&kept, &saved, &gone]
        .iter()
        .map(|path| Sent {
            path: path.to_path_buf(),
            identity: Identity::read(path).unwrap(),
            seen: None,
        })
        .collect();
    let was = sent[1].identity.clone();
    std::fs::write(&saved, b"saved again by the editor")?;
    std::fs::remove_file(&gone)?;
    // Seen once: not taken up until it has settled.
    assert!(rewritten(&mut sent).is_empty());
    let changed = rewritten(&mut sent);
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].0, saved);
    assert_eq!(changed[0].1, was);
    assert_eq!(changed[0].2, Identity::read(&saved)?);
    assert_eq!(
        sent.iter().map(|s| s.path.clone()).collect::<Vec<_>>(),
        [kept.clone(), saved]
    );
    // Looked at again with nothing saved since: nothing to do.
    assert!(rewritten(&mut sent).is_empty());
    // Still being written at each look: never taken up mid-write.
    for part in ["s", "sa", "sav"] {
        std::fs::write(&kept, part)?;
        assert!(rewritten(&mut sent).is_empty(), "{part}");
    }
    assert_eq!(rewritten(&mut sent).len(), 1, "settled");
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
            originals: vec![Some(ids[0])],
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
    assert_eq!(e.edit_in.launched, Some(vec![(editor, tiff.clone())]));
    assert_eq!(e.messages.line(), "Opened IMG_1-Edit.tif in Editor");

    // Edited here, then saved again by the editor while the window was
    // behind it: back at the front, the edit follows the new version.
    let was = Identity::read(&tiff)?;
    let recipe = crate::model::recipe::Recipe {
        exposure: 0.5,
        ..Default::default()
    };
    e.library.as_mut().unwrap().session.save_edit(
        photo,
        &tiff,
        &was,
        &recipe,
        &Default::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    e.document.catalog_photo = Some(photo);
    e.document.path = Some(tiff.clone());
    e.document.file = Some(was.clone());
    std::fs::write(&tiff, b"saved again by the editor")?;
    let library = e.library.as_ref().unwrap();
    assert!(library.session.catalog().load_edit(photo, &tiff).is_err());
    // Taken up by the passes that run with the window behind the editor:
    // seen once, then settled at the next look.
    let ctx = e.context.clone();
    for _ in 0..2 {
        e.edit_in.checked = None;
        e.background(&ctx);
    }
    let now = Identity::read(&tiff)?;
    let library = e.library.as_ref().unwrap();
    let saved = library.session.catalog().load_edit(photo, &tiff)?;
    assert_eq!(saved.map(|s| s.recipe.exposure), Some(0.5));
    assert!(e.document.loading(), "the photo shown opens again");
    assert_eq!(e.document.catalog_photo, Some(photo));
    assert!(!was.same_photo(&now));
    assert_eq!(e.edit_in.sent[0].identity, now);
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
            originals: vec![Some(ids[0])],
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

/// While files are sent out the passes keep coming about once a second, as
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
        seen: None,
    });
    pass(&mut e);
    let woken = woken.lock().unwrap();
    assert!(
        woken.iter().any(|delay| *delay <= Duration::from_secs(1)),
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
            e.edit_in.asking = Some(Asking {
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
