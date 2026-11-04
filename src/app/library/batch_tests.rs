//! Batch edits from the Library, driven as a person would: the keys and Quick
//! Develop start them on the selection, and each photo ends up as Develop would have
//! made it.
use crate::app::library::sync::{BatchChange, BatchProgress};
use crate::app::{Editor, Modal, Module};
use crate::catalog::PhotoId;
use eframe::egui;
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A gradient scene of `level` brightness, tinted, with stripes falling 3° to the
/// right for Upright to level, as a PNG.
fn scene(path: &Path, level: f32) {
    let (s, c) = 3f32.to_radians().sin_cos();
    image::RgbImage::from_fn(240, 160, |x, y| {
        let (fx, fy) = (x as f32 - 120., y as f32 - 80.);
        let stripe = if ((-s * fx + c * fy) / 28.).rem_euclid(2.) < 1. {
            1.
        } else {
            0.55
        };
        let v = level * stripe * (0.1 + 0.9 * x as f32 / 239.);
        image::Rgb([
            (255. * v).min(255.) as u8,
            (215. * v).min(255.) as u8,
            (170. * v).min(255.) as u8,
        ])
    })
    .save(path)
    .unwrap();
}

/// A catalog photo and its file.
type Photo = (PhotoId, PathBuf);

/// An editor on a catalog of scenes, `(name, level)`, in the Library.
fn editor_with_scenes(
    scenes: &[(&str, f32)],
) -> anyhow::Result<(tempfile::TempDir, Editor, Vec<Photo>)> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    for (name, level) in scenes {
        scene(&photos.join(name), *level);
    }
    let path = d.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.raw_defaults = Arc::new(crate::raw_defaults::DevelopDefaults::with_presets(
        Default::default(),
        |_| None,
    ));
    let library = crate::app::library::Library::load(&path, ctx)?;
    let ids = scenes
        .iter()
        .map(|(name, _)| {
            let p = library
                .session
                .photos()
                .iter()
                .find(|p| p.filename == *name);
            let p = p.unwrap();
            (p.id, p.path.clone())
        })
        .collect();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    e.onboarding.visible = false;
    Ok((d, e, ids))
}

/// Runs the editor's events until `done`, or fails after a minute.
fn wait(e: &mut Editor, what: &str, done: impl Fn(&Editor) -> bool) {
    let ctx = e.context.clone();
    let until = Instant::now() + Duration::from_secs(60);
    while !done(e) {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        e.events(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Opens `photo` in Develop and waits until it is decoded in full and analysed.
fn open(e: &mut Editor, photo: &Photo) {
    e.open_raw(photo.1.clone(), Some(photo.0));
    wait(e, "the photo to open", settled);
}

/// The open photo decoded in full, with nothing still to land on its edit.
fn settled(e: &Editor) -> bool {
    let d = &e.document;
    !e.document.loading()
        && d.full().is_some_and(|im| !im.fast)
        && d.pending_lightroom.is_none()
        && !d.auto.is_running()
        && !d.upright.is_running()
        && !d.straighten.is_running()
        && !d.edit.recipe().upright.needs_analysis()
        && !d.edit.recipe().straighten_pending()
}

/// Selects the photos from the first of `ids` to the last, the last one active.
fn select(e: &mut Editor, ids: &[PhotoId]) {
    let library = e.library.as_mut().unwrap();
    library.select(ids.first().copied());
    if let Some(last) = ids.last().filter(|_| ids.len() > 1) {
        library.select_range_to(*last);
    }
    assert_eq!(library.selected_photos().len(), ids.len());
}

/// Waits for the running batch to report back.
fn finish_batch(e: &mut Editor) {
    wait(e, "the batch", |e| !e.activity.is_syncing());
}

fn saved(e: &Editor, photo: &Photo) -> Option<crate::model::recipe::Recipe> {
    let library = e.library.as_ref().unwrap();
    let edit = library
        .session
        .catalog()
        .load_edit(photo.0, &photo.1)
        .unwrap();
    edit.map(|e| e.recipe)
}

/// A key pressed in a frame of the whole window.
fn press(e: &mut Editor, key: egui::Key, modifiers: egui::Modifiers) {
    let ctx = e.context.clone();
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
            // Released, so the next press is not taken for a held key.
            egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: false,
                repeat: false,
                modifiers,
            },
        ],
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| e.draw(ui));
    output.textures_delta.clear();
}

fn preset(name: &str, attrs: &str) -> crate::xmp::Preset {
    let text = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Name="{name}" {attrs}/></rdf:RDF></x:xmpmeta>"#
    );
    crate::xmp::parse(Path::new(&format!("{name}.xmp")), &text).unwrap()
}

/// Shift+Cmd+U in the Library is Auto Settings on every selected photo, the active
/// one included; plain U still unflags, Develop's key stays the open photo's, and
/// the automation command still needs Develop.
#[test]
fn library_auto_settings_key_applies_to_the_whole_selection() -> anyhow::Result<()> {
    let (_d, mut e, photos) =
        editor_with_scenes(&[("a.png", 0.3), ("b.png", 0.6), ("c.png", 0.9)])?;
    let ids: Vec<_> = photos.iter().map(|p| p.0).collect();
    let auto = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    // Nothing selected: nothing to apply to.
    e.library.as_mut().unwrap().select(None);
    assert_eq!(e.batch_target_count(), 0);
    press(&mut e, egui::Key::U, auto);
    assert!(!e.activity.is_busy());
    // A single photo is a selection too.
    select(&mut e, &ids[..1]);
    assert_eq!(e.batch_target_count(), 1);
    // Two of three, the active one included.
    select(&mut e, &ids[..2]);
    assert_eq!(e.batch_targets().len(), 2);
    press(&mut e, egui::Key::U, auto);
    let progress = e.activity.batch().expect("a batch runs");
    assert_eq!(progress.name(), "Auto Settings");
    assert_eq!(progress.counts().1, 2);
    // A second press while it runs starts nothing.
    press(&mut e, egui::Key::U, auto);
    finish_batch(&mut e);
    assert!(saved(&e, &photos[0]).is_some() && saved(&e, &photos[1]).is_some());
    assert!(saved(&e, &photos[2]).is_none());
    assert!(
        e.messages
            .line()
            .starts_with("Auto Settings: 2 photos changed"),
        "{}",
        e.messages.line()
    );
    // Plain U unflags the selection, as before.
    select(&mut e, &ids[2..]);
    e.library
        .as_mut()
        .unwrap()
        .edit_selection(crate::app::library::photo_metadata::Edit::Flag(1), false)?;
    press(&mut e, egui::Key::U, egui::Modifiers::NONE);
    assert!(!e.activity.is_busy());
    let library = e.library.as_ref().unwrap();
    assert_eq!(
        library.photo(ids[2]).unwrap().flag,
        0,
        "{}",
        e.messages.line()
    );
    // Behind a dialog, the key does nothing.
    e.modal = Some(Modal::RemoveCopy(ids[2]));
    press(&mut e, egui::Key::U, auto);
    assert!(!e.activity.is_busy());
    e.modal = None;
    // The automation command keeps its meaning: Auto on the photo open in Develop.
    let ctx = e.context.clone();
    let refused = e.execute_command(
        crate::app::shell::commands::Command::new(crate::app::shell::commands::Operation::Action(
            crate::app::shell::commands::Action::AutoTone,
        )),
        &ctx,
    );
    assert!(refused.is_err());
    assert!(!e.activity.is_busy());
    Ok(())
}

/// Quick Develop's Auto Settings and Saved Preset start the same batch, on the
/// selection, with the preset read when it starts.
#[test]
fn quick_develop_requests_apply_to_the_selection() -> anyhow::Result<()> {
    let (_d, mut e, photos) = editor_with_scenes(&[("a.png", 0.3), ("b.png", 0.6)])?;
    let ids: Vec<_> = photos.iter().map(|p| p.0).collect();
    e.presets.library = Arc::new(crate::presets::Library {
        presets: vec![preset("Bright", r#"crs:Exposure2012="+0.50""#)],
        errors: Vec::new(),
    });
    select(&mut e, &ids);
    e.quick_develop_request(super::QuickRequest::Preset(0));
    assert_eq!(e.activity.batch().unwrap().name(), "Apply Preset: Bright");
    // The library changing while it runs does not change what it applies.
    e.presets.library = Arc::new(crate::presets::Library::default());
    finish_batch(&mut e);
    for photo in &photos {
        assert_eq!(saved(&e, photo).unwrap().exposure, 0.5);
    }
    let library = e.library.as_ref().unwrap();
    let history = library.session.catalog().load_history(ids[0])?.unwrap();
    let step = history.steps.last().unwrap();
    assert_eq!(
        (step.name.as_str(), step.value.as_str()),
        ("Preset", "Bright")
    );
    // One Undo reverts both.
    e.undo();
    assert!(photos.iter().all(|p| saved(&e, p).is_none()));
    assert!(
        e.messages.line().contains("Apply Preset: Bright"),
        "{}",
        e.messages.line()
    );
    e.quick_develop_request(super::QuickRequest::AutoSettings);
    assert_eq!(e.activity.batch().unwrap().name(), "Auto Settings");
    finish_batch(&mut e);
    Ok(())
}

/// Each photo gets what Develop's Auto gives it: a copy of the open photo ends up
/// with exactly the open photo's Auto, the others with their own.
#[test]
fn batch_auto_settings_matches_develops_auto() -> anyhow::Result<()> {
    let (_d, mut e, photos) =
        editor_with_scenes(&[("a.png", 0.45), ("b.png", 0.45), ("c.png", 0.9)])?;
    open(&mut e, &photos[0]);
    e.start_auto(crate::app::worker::AutoKind::Settings);
    wait(&mut e, "Auto", |e| !e.document.auto.is_running());
    let develop = e.document.edit.recipe().clone();
    assert_ne!(develop.exposure, 0.);
    e.module = Module::Library;
    select(&mut e, &[photos[1].0, photos[2].0]);
    e.start_batch(BatchChange::AutoSettings);
    finish_batch(&mut e);
    assert_eq!(saved(&e, &photos[1]).unwrap(), develop);
    assert_ne!(saved(&e, &photos[2]).unwrap().exposure, develop.exposure);
    Ok(())
}

/// A preset applied to a selection gives each photo what a click in Develop's
/// Presets panel gives it: sparse settings, explicit zeros, a profile this photo
/// lacks, Auto Settings, Auto white balance, Auto black & white and Upright.
#[test]
fn batch_preset_matches_the_preset_clicked_in_develop() -> anyhow::Result<()> {
    let presets = [
        preset("Sparse", r#"crs:Exposure2012="+0.50""#),
        preset("Zeros", r#"crs:Contrast2012="0" crs:Shadows2012="0""#),
        preset(
            "Missing Profile",
            r#"crs:CameraProfile="Adobe Standard" crs:Clarity2012="20""#,
        ),
        preset("Auto", r#"crs:AutoTone="True""#),
        preset("Auto WB", r#"crs:WhiteBalance="Auto""#),
        preset(
            "Auto BW",
            r#"crs:ConvertToGrayscale="True" crs:AutoGrayscaleMix="True""#,
        ),
        preset("Level", r#"crs:PerspectiveUpright="3""#),
        preset("Switched Off", r#"crs:Sharpness="90""#),
    ];
    let names: Vec<String> = (0..presets.len())
        .flat_map(|i| [format!("d{i}.png"), format!("b{i}.png")])
        .collect();
    let scenes: Vec<(&str, f32)> = names.iter().map(|n| (n.as_str(), 0.6)).collect();
    let (_d, mut e, photos) = editor_with_scenes(&scenes)?;
    e.presets.library = Arc::new(crate::presets::Library {
        presets: presets.to_vec(),
        errors: Vec::new(),
    });
    // Both photos of each pair start from the same edit, which a preset's zeros
    // must replace and whose Detail panel is off.
    let m = crate::photo::open_source(&photos[0].1, Cancel::NEVER)?
        .metadata()
        .clone();
    let mut start = crate::model::recipe::Recipe::with_profiles(&m, &[]);
    start.contrast = 0.3;
    start.shadows = 0.2;
    start.panels.set(
        crate::model::panels::Panel::Detail,
        crate::model::panels::PanelState::Off,
    );
    {
        let library = e.library.as_mut().unwrap();
        for (id, path) in &photos {
            library.session.save_edit(
                *id,
                path,
                &crate::storage::Identity::read(path)?,
                &start,
                &Default::default(),
                crate::catalog::HistoryUpdate::Keep,
            )?;
        }
    }
    let ctx = e.context.clone();
    for (i, pair) in photos.chunks(2).enumerate() {
        let (develop_photo, batch_photo) = (&pair[0], &pair[1]);
        // A click in Develop, as a frame of the window makes it.
        open(&mut e, develop_photo);
        let frame = e.begin_edit_frame();
        e.apply_preset(i)?;
        e.finish_edit_frame(frame, &ctx);
        wait(&mut e, "the preset's analysis", settled);
        let develop = e.document.edit.recipe().clone();
        assert_ne!(develop, start, "{}", presets[i].name);
        // The same preset on the other photo, from the Library.
        e.module = Module::Library;
        select(&mut e, &[batch_photo.0]);
        e.quick_develop_request(super::QuickRequest::Preset(i));
        finish_batch(&mut e);
        assert_eq!(
            saved(&e, batch_photo).unwrap(),
            develop,
            "{}: {}",
            presets[i].name,
            e.messages.line()
        );
    }
    Ok(())
}

/// The open photo can be one of the photos changed: it is saved first, opens again
/// with what the batch wrote, and stays as it was when the batch is cancelled.
#[test]
fn the_open_photo_is_saved_first_and_opens_again() -> anyhow::Result<()> {
    let (_d, mut e, photos) = editor_with_scenes(&[("a.png", 0.3), ("b.png", 0.8)])?;
    let ids: Vec<_> = photos.iter().map(|p| p.0).collect();
    open(&mut e, &photos[0]);
    let ctx = e.context.clone();
    let frame = e.begin_edit_frame();
    e.document.edit.recipe_mut().effects.clarity = 0.4;
    e.finish_edit_frame(frame, &ctx);
    assert!(e.document.edit.save_state().needs_save());
    e.module = Module::Library;
    select(&mut e, &ids);
    // Cancelled: nothing changes, and the open photo keeps its edit, now saved.
    e.start_batch(BatchChange::AutoSettings);
    assert!(!e.document.edit.save_state().needs_save());
    assert_eq!(saved(&e, &photos[0]).unwrap().effects.clarity, 0.4);
    assert!(e.activity.batch().unwrap().cancel());
    finish_batch(&mut e);
    assert!(
        e.messages.line().contains("cancelled"),
        "{}",
        e.messages.line()
    );
    assert_eq!(saved(&e, &photos[0]).unwrap().exposure, 0.);
    assert!(saved(&e, &photos[1]).is_none());
    assert!(!e.document.loading());
    assert_eq!(e.document.edit.recipe().effects.clarity, 0.4);
    // Applied: the open photo opens again with Auto, its clarity kept.
    e.start_batch(BatchChange::AutoSettings);
    finish_batch(&mut e);
    assert!(e.document.loading(), "the open photo opens again");
    wait(&mut e, "the photo to open again", settled);
    let written = saved(&e, &photos[0]).unwrap();
    assert_ne!(written.exposure, 0.);
    assert_eq!(written.effects.clarity, 0.4);
    assert_eq!(*e.document.edit.recipe(), written);
    Ok(())
}

/// An open edit that can't be saved holds the batch back: nothing starts.
#[test]
fn an_open_edit_that_cannot_be_saved_holds_the_batch() -> anyhow::Result<()> {
    let (_d, mut e, photos) = editor_with_scenes(&[("a.png", 0.3), ("b.png", 0.8)])?;
    select(&mut e, &[photos[0].0, photos[1].0]);
    // An edit of a photo no longer in the catalog: saving it fails.
    e.document.catalog_photo = Some(PhotoId(9999));
    e.document.opened_at(photos[0].1.clone());
    e.document.edit.setup_mut().exposure = 0.3;
    e.document.edit.save_state_mut().mark_changed();
    e.start_batch(BatchChange::AutoSettings);
    assert!(!e.activity.is_busy());
    assert!(photos.iter().all(|p| saved(&e, p).is_none()));
    Ok(())
}

/// The progress window's Cancel ends nothing by itself: the batch stays running
/// until its worker reports back, and once it saves Cancel is refused.
#[test]
fn cancel_waits_for_the_worker() {
    let mut activity = crate::app::activity::Activity::default();
    let progress = Arc::new(BatchProgress::new("Auto Settings", 3));
    assert!(activity.begin_sync(progress.clone()));
    assert!(progress.cancel());
    assert!(activity.is_syncing() && activity.holds_catalog_writes());
    assert_eq!(
        activity.batch().unwrap().stage(),
        crate::app::library::sync::BatchStage::Cancelling
    );
    activity.finish_sync();
    assert!(!activity.is_busy());
}

/// While Auto or Auto Straighten is still to land on the open photo, a batch that
/// would change it waits: the photo opens again after the batch, and what landed
/// meanwhile would be lost.
#[test]
fn a_batch_waits_for_auto_or_straighten_on_the_open_photo() -> anyhow::Result<()> {
    let (_d, mut e, photos) = editor_with_scenes(&[("a.png", 0.3), ("b.png", 0.8)])?;
    select(&mut e, &[photos[0].0, photos[1].0]);
    e.document.catalog_photo = Some(photos[0].0);
    assert_eq!(e.batch_target_count(), 2);
    let _straighten = e.document.straighten.start();
    assert_eq!(e.batch_target_count(), 0);
    e.start_batch(BatchChange::AutoSettings);
    assert!(!e.activity.is_busy());
    e.document.straighten = Default::default();
    let _auto = e.document.auto.start();
    assert_eq!(e.batch_target_count(), 0);
    e.document.auto = Default::default();
    // Not selected, the open photo holds nothing back.
    select(&mut e, &[photos[1].0]);
    let _straighten = e.document.straighten.start();
    assert_eq!(e.batch_target_count(), 1);
    Ok(())
}
