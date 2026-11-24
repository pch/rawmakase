//! Auto Sync driven as a person would: edits of the open photo in Develop, the key,
//! Auto, Undo, the Synchronize Settings dialog and the filmstrip bar's buttons.
use crate::app::develop::settings_transfer::{CopyChoice, Transfer, TransferButtons};
use crate::app::testing::{FocusedButton, labelled, panel_frame, press_and_release};
use crate::app::{Editor, Modal, Module};
use crate::catalog::PhotoId;
use crate::model::recipe::Recipe;
use crate::model::settings_groups::{GroupInclusion, GroupSelection, SettingGroup};
use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

type Photo = (PhotoId, PathBuf);

/// A catalog of `count` small PNGs, each brighter than the one before, its Library
/// and its photos in name order.
fn library_of(
    ctx: &egui::Context,
    count: usize,
) -> anyhow::Result<(tempfile::TempDir, crate::app::library::Library, Vec<Photo>)> {
    let d = tempfile::tempdir()?;
    let folder = d.path().join("photos");
    std::fs::create_dir(&folder)?;
    for i in 0..count {
        let level = 0.25 + 0.6 * i as f32 / count.max(2) as f32;
        image::RgbImage::from_fn(48, 32, |x, y| {
            let v = level * (0.3 + 0.7 * x as f32 / 47.);
            image::Rgb([
                (255. * v) as u8,
                (230. * v) as u8,
                (200. * v * (0.5 + y as f32 / 64.)) as u8,
            ])
        })
        .save(folder.join(format!("{i:03}.png")))?;
    }
    let path = d.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&folder)?;
    let library = crate::app::library::Library::load(&path, ctx.clone())?;
    let mut photos: Vec<Photo> = library
        .session
        .photos()
        .iter()
        .map(|p| (p.id, p.path.clone()))
        .collect();
    photos.sort_by(|a, b| a.1.cmp(&b.1));
    Ok((d, library, photos))
}

fn library(
    ctx: &egui::Context,
) -> anyhow::Result<(tempfile::TempDir, crate::app::library::Library, Vec<Photo>)> {
    library_of(ctx, 3)
}

/// An editor on a catalog of `count` photos, in Develop with nothing open yet.
fn editor_of(count: usize) -> anyhow::Result<(tempfile::TempDir, Editor, Vec<Photo>)> {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let (d, library, photos) = library_of(&ctx, count)?;
    e.library = Some(Box::new(library));
    e.module = Module::Develop;
    e.onboarding.visible = false;
    Ok((d, e, photos))
}

fn editor() -> anyhow::Result<(tempfile::TempDir, Editor, Vec<Photo>)> {
    editor_of(3)
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

fn open(e: &mut Editor, photo: &Photo) {
    e.open_raw(photo.1.clone(), Some(photo.0));
    wait(e, "the photo to open", |e| {
        !e.document.loading() && e.document.full().is_some_and(|im| !im.fast)
    });
}

/// An edit of the open photo as a panel makes it, then a frame of the window, which
/// records it as a History step and starts Auto Sync's batch.
fn edit(e: &mut Editor, change: impl FnOnce(&mut Recipe)) {
    let ctx = e.context.clone();
    crate::app::testing::in_edit_frame(&ctx, e, |e| change(e.document.edit.recipe_mut()));
    panel_frame(e, Vec::new(), FocusedButton::No);
}

/// Runs frames until Auto Sync has written every step.
fn settle(e: &mut Editor) {
    let until = Instant::now() + Duration::from_secs(60);
    while e.auto_sync_busy() {
        assert!(Instant::now() < until, "Auto Sync never finished");
        e.run_auto_sync();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Holds each Auto Sync batch until the returned sender lets one go.
fn hold_batches(e: &mut Editor) -> mpsc::Sender<()> {
    let (tx, rx) = mpsc::channel();
    e.auto_sync.gate = Some(Arc::new(Mutex::new(rx)));
    tx
}

fn finish_batch(e: &mut Editor) {
    wait(e, "the batch", |e| !e.activity.is_syncing());
}

fn saved(e: &Editor, photo: &Photo) -> Recipe {
    saved_id(e, photo.0, &photo.1)
}

fn saved_id(e: &Editor, id: PhotoId, path: &std::path::Path) -> Recipe {
    let library = e.library.as_ref().unwrap();
    let edit = library.session.catalog().load_edit(id, path);
    edit.unwrap().map(|e| e.recipe).unwrap_or_default()
}

fn select(e: &mut Editor, from: &Photo, to: &Photo) {
    let library = e.library.as_mut().unwrap();
    library.select(Some(from.0));
    library.select_range_to(to.0);
}

/// Photo `a` open, with `b`, whose own Contrast is +30, and `c` selected with it.
fn three_selected() -> anyhow::Result<(tempfile::TempDir, Editor, Vec<Photo>)> {
    let (d, mut e, photos) = editor()?;
    open(&mut e, &photos[1]);
    edit(&mut e, |r| r.contrast = 0.3);
    assert!(e.flush(), "{}", e.messages.line());
    open(&mut e, &photos[0]);
    select(&mut e, &photos[0], &photos[2]);
    assert_eq!(e.sync_target_count(), 2);
    Ok((d, e, photos))
}

/// [`three_selected`] with Auto Sync on, taking Exposure and Grain.
fn auto_syncing() -> anyhow::Result<(tempfile::TempDir, Editor, Vec<Photo>)> {
    let (d, mut e, photos) = three_selected()?;
    let mut chosen = GroupSelection::none();
    chosen.set(SettingGroup::Exposure, GroupInclusion::Included);
    chosen.set(SettingGroup::Grain, GroupInclusion::Included);
    e.session.sync_groups = Some(chosen);
    e.session.auto_sync = true;
    Ok((d, e, photos))
}

fn undo_entries(e: &Editor) -> usize {
    e.undo_log.len().0
}

#[test]
fn with_auto_sync_off_an_edit_changes_only_the_open_photo() -> anyhow::Result<()> {
    let (_d, mut e, photos) = three_selected()?;
    assert!(!e.session.auto_sync);
    edit(&mut e, |r| r.exposure = 0.5);
    assert!(!e.auto_sync_busy() && !e.activity.is_syncing());
    assert_eq!(saved(&e, &photos[1]).exposure, 0.);
    assert_eq!(saved(&e, &photos[2]).exposure, 0.);
    Ok(())
}

/// The key turns Auto Sync on; an Exposure change goes to the other selected photos,
/// which keep their own Contrast; a change in a group Synchronize did not apply
/// stays on the open photo; the groups are those the Sync dialog chose last.
#[test]
fn auto_sync_takes_the_changed_groups_synchronize_chose() -> anyhow::Result<()> {
    let (_d, mut e, photos) = three_selected()?;
    // Synchronize Settings with Exposure and Grain only: what Auto Sync takes after.
    e.open_copy_dialog(Transfer::Sync);
    let mut chosen = GroupSelection::none();
    chosen.set(SettingGroup::Exposure, GroupInclusion::Included);
    chosen.set(SettingGroup::Grain, GroupInclusion::Included);
    if let Some(Modal::CopySettings(dialog)) = &mut e.modal {
        dialog.groups = chosen.clone();
    }
    e.close_copy_dialog(CopyChoice::Confirm);
    finish_batch(&mut e);
    assert_eq!(e.sync_groups(), chosen);
    // Its dialog opens with them again, though Copy Settings chose others since.
    e.session.copy_groups = GroupSelection::all();
    e.open_copy_dialog(Transfer::Sync);
    assert!(matches!(&e.modal, Some(Modal::CopySettings(d)) if d.groups == chosen));
    e.modal = None;

    let keys = egui::Modifiers::COMMAND | egui::Modifiers::ALT | egui::Modifiers::SHIFT;
    crate::app::testing::press(&mut e, egui::Key::A, keys);
    assert!(e.session.auto_sync);

    edit(&mut e, |r| r.exposure = 0.5);
    assert!(e.auto_sync_busy(), "{}", e.messages.line());
    settle(&mut e);
    for target in &photos[1..] {
        assert_eq!(saved(&e, target).exposure, 0.5);
    }
    assert_eq!(saved(&e, &photos[1]).contrast, 0.3);
    assert_eq!(saved(&e, &photos[2]).contrast, 0.);

    // Contrast is not among Synchronize's groups.
    edit(&mut e, |r| r.contrast = -0.2);
    assert!(!e.auto_sync_busy());
    assert_eq!(saved(&e, &photos[1]).contrast, 0.3);

    // Off again, by the key, an edit stays on the open photo.
    crate::app::testing::press(&mut e, egui::Key::A, keys);
    assert!(!e.session.auto_sync);
    edit(&mut e, |r| r.exposure = 1.);
    assert!(!e.auto_sync_busy());
    assert_eq!(saved(&e, &photos[1]).exposure, 0.5);
    Ok(())
}

/// One Undo takes back the open photo's step and what Auto Sync wrote with it;
/// one Redo makes both again.
#[test]
fn one_undo_takes_back_the_step_on_every_photo() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let before = undo_entries(&e);
    edit(&mut e, |r| r.exposure = 0.5);
    settle(&mut e);
    assert_eq!(undo_entries(&e), before + 1, "one command for the step");
    let exposures = |e: &Editor| {
        (
            e.document.edit.recipe().exposure,
            saved(e, &photos[1]).exposure,
            saved(e, &photos[2]).exposure,
        )
    };
    assert_eq!(exposures(&e), (0.5, 0.5, 0.5));
    e.undo();
    assert_eq!(exposures(&e), (0., 0., 0.));
    // Their own Contrast is back as it was too, untouched.
    assert_eq!(saved(&e, &photos[1]).contrast, 0.3);
    e.redo();
    assert_eq!(exposures(&e), (0.5, 0.5, 0.5));
    // An Undo made while the batch still runs waits for it, and takes both back.
    let release = hold_batches(&mut e);
    edit(&mut e, |r| r.exposure = 1.);
    assert!(e.auto_sync_busy());
    release.send(())?;
    e.undo();
    assert_eq!(exposures(&e), (0.5, 0.5, 0.5));
    Ok(())
}

/// While a batch writes the other photos, nothing is disabled, no window opens and
/// no message is shown; editing goes on.
#[test]
fn a_batch_runs_without_blocking_the_window() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let release = hold_batches(&mut e);
    let messages = e.messages.entry_count();
    edit(&mut e, |r| r.exposure = 0.5);
    assert!(e.auto_sync_busy());
    assert!(!e.activity.is_busy(), "the window is not disabled");
    assert!(e.modal.is_none());
    edit(&mut e, |r| r.contrast = 0.4);
    assert_eq!(e.document.edit.recipe().contrast, 0.4);
    assert_eq!(e.messages.entry_count(), messages, "{}", e.messages.line());
    release.send(())?;
    settle(&mut e);
    assert_eq!(e.messages.entry_count(), messages, "{}", e.messages.line());
    assert_eq!(saved(&e, &photos[2]).exposure, 0.5);
    Ok(())
}

/// Quick steps made while a batch runs go together in the next one, in order: the
/// other photos end with the last value, and each Undo takes back a batch's steps.
#[test]
fn quick_steps_are_joined_into_the_next_batch() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let release = hold_batches(&mut e);
    for step in 1..=10 {
        edit(&mut e, |r| r.exposure = step as f32 / 10.);
    }
    assert_eq!(
        e.auto_sync.started, 1,
        "one batch runs, the rest wait for it"
    );
    release.send(())?;
    release.send(())?;
    settle(&mut e);
    assert_eq!(e.auto_sync.started, 2);
    assert_eq!(saved(&e, &photos[1]).exposure, 1.);
    assert_eq!(saved(&e, &photos[2]).exposure, 1.);
    // The second batch carried steps 2 to 10.
    e.undo();
    assert_eq!(e.document.edit.recipe().exposure, 0.1);
    assert_eq!(saved(&e, &photos[1]).exposure, 0.1);
    e.undo();
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    assert_eq!(saved(&e, &photos[2]).exposure, 0.);
    Ok(())
}

/// A step goes to the photos selected when it was made, even when the selection
/// changes while it waits.
#[test]
fn a_step_goes_to_the_photos_selected_when_it_was_made() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let release = hold_batches(&mut e);
    edit(&mut e, |r| r.exposure = 0.3);
    // Now only `b` with the open photo.
    select(&mut e, &photos[0], &photos[1]);
    edit(&mut e, |r| r.exposure = 0.6);
    release.send(())?;
    release.send(())?;
    settle(&mut e);
    assert_eq!(saved(&e, &photos[1]).exposure, 0.6);
    assert_eq!(saved(&e, &photos[2]).exposure, 0.3);
    Ok(())
}

/// Opening another photo waits until the steps before are written, so a photo
/// they went to opens with them.
#[test]
fn leaving_the_photo_writes_the_waiting_steps_first() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let release = hold_batches(&mut e);
    edit(&mut e, |r| r.exposure = 0.7);
    release.send(())?;
    e.auto_sync.gate = None;
    open(&mut e, &photos[2]);
    assert_eq!(e.document.edit.recipe().exposure, 0.7);
    assert!(!e.auto_sync_busy());
    // In the Library too: what was waiting goes.
    select(&mut e, &photos[2], &photos[1]);
    e.library.as_mut().unwrap().make_active(photos[2].0);
    edit(&mut e, |r| r.exposure = -0.4);
    e.module = Module::Library;
    settle(&mut e);
    assert_eq!(saved(&e, &photos[1]).exposure, -0.4);
    Ok(())
}

/// Auto and Auto White Balance are measured on each photo, as in Lightroom, rather
/// than copied from the open photo; one Undo takes back all of them.
#[test]
fn auto_is_measured_on_each_photo() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let auto = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    crate::app::testing::press(&mut e, egui::Key::U, auto);
    wait(&mut e, "Auto", |e| !e.document.auto.is_running());
    panel_frame(&mut e, Vec::new(), FocusedButton::No);
    settle(&mut e);
    let own = e.document.edit.recipe().exposure;
    let (b, c) = (
        saved(&e, &photos[1]).exposure,
        saved(&e, &photos[2]).exposure,
    );
    assert!(own != 0. && b != 0. && c != 0., "{own} {b} {c}");
    // Brighter photos need less: each has its own.
    assert!(own > b && b > c, "{own} {b} {c}");
    // Of each photo's Auto only the groups Synchronize chose are taken: Contrast is
    // not one, so `b` keeps its own.
    assert_eq!(saved(&e, &photos[1]).contrast, 0.3);
    e.undo();
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    assert_eq!(saved(&e, &photos[1]).exposure, 0.);
    assert_eq!(saved(&e, &photos[1]).contrast, 0.3);

    // Auto White Balance goes nowhere while White Balance is not chosen.
    e.start_auto(crate::app::worker::AutoKind::WhiteBalance);
    wait(&mut e, "Auto White Balance", |e| {
        !e.document.auto.is_running()
    });
    panel_frame(&mut e, Vec::new(), FocusedButton::No);
    assert!(!e.auto_sync_busy());
    assert!(saved(&e, &photos[1]).auto_white_balance.is_none());
    e.undo();
    let mut chosen = e.sync_groups();
    chosen.set(SettingGroup::WhiteBalance, GroupInclusion::Included);
    e.session.sync_groups = Some(chosen);

    // Auto White Balance from the White Balance menu's Auto.
    e.start_auto(crate::app::worker::AutoKind::WhiteBalance);
    wait(&mut e, "Auto White Balance", |e| {
        !e.document.auto.is_running()
    });
    panel_frame(&mut e, Vec::new(), FocusedButton::No);
    settle(&mut e);
    assert_eq!(
        saved(&e, &photos[1]).auto_white_balance.is_some(),
        e.document.edit.recipe().auto_white_balance.is_some()
    );
    assert_eq!(saved(&e, &photos[1]).exposure, 0., "only the white balance");
    Ok(())
}

/// A step recorded in the frame the photo is left in (a drag still held, an edit
/// just made) goes to the other photos before the save returns.
#[test]
fn leaving_the_photo_writes_the_step_of_the_same_frame() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let ctx = e.context.clone();
    crate::app::testing::in_edit_frame(&ctx, &mut e, |e| {
        e.document.edit.recipe_mut().exposure = 0.4
    });
    assert!(e.flush(), "{}", e.messages.line());
    assert!(!e.auto_sync_busy());
    assert_eq!(saved(&e, &photos[1]).exposure, 0.4);
    assert_eq!(saved(&e, &photos[2]).exposure, 0.4);
    Ok(())
}

/// Undo pressed in the frame of an edit takes back that edit on every photo: it is
/// recorded and written first, never written after it was undone.
#[test]
fn undo_in_the_frame_of_an_edit_takes_it_back_on_every_photo() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let ctx = e.context.clone();
    crate::app::testing::in_edit_frame(&ctx, &mut e, |e| {
        e.document.edit.recipe_mut().exposure = 0.4
    });
    e.undo();
    settle(&mut e);
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    assert_eq!(saved(&e, &photos[1]).exposure, 0.);
    assert_eq!(saved(&e, &photos[2]).exposure, 0.);
    e.redo();
    assert_eq!(e.document.edit.recipe().exposure, 0.4);
    assert_eq!(saved(&e, &photos[1]).exposure, 0.4);
    Ok(())
}

/// A step Auto Sync does not take, between steps it does, keeps an Undo of its own:
/// it is never joined into theirs.
#[test]
fn a_step_not_synced_keeps_its_own_undo() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let release = hold_batches(&mut e);
    edit(&mut e, |r| r.exposure = 0.1);
    edit(&mut e, |r| r.exposure = 0.2);
    // Contrast is not among Synchronize's groups.
    edit(&mut e, |r| r.contrast = 0.5);
    edit(&mut e, |r| r.exposure = 0.3);
    for _ in 0..3 {
        release.send(())?;
    }
    settle(&mut e);
    assert_eq!(saved(&e, &photos[2]).exposure, 0.3);
    e.undo();
    assert_eq!(e.document.edit.recipe().exposure, 0.2);
    assert_eq!(e.document.edit.recipe().contrast, 0.5);
    assert_eq!(saved(&e, &photos[2]).exposure, 0.2);
    e.undo();
    assert_eq!(e.document.edit.recipe().contrast, 0.);
    assert_eq!(e.document.edit.recipe().exposure, 0.2);
    assert_eq!(saved(&e, &photos[2]).exposure, 0.2);
    Ok(())
}

/// When the photo the step was made on is removed, Undo still takes back what Auto
/// Sync wrote to the others.
#[test]
fn removing_the_source_photo_keeps_the_others_undo() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    edit(&mut e, |r| r.exposure = 0.5);
    settle(&mut e);
    e.undo_log.forget_photo(photos[0].0);
    e.undo();
    assert_eq!(saved(&e, &photos[1]).exposure, 0.);
    assert_eq!(saved(&e, &photos[2]).exposure, 0.);
    assert_eq!(saved(&e, &photos[1]).contrast, 0.3);
    Ok(())
}

/// Closing waits for Auto Sync as for any batch; a save that can't wait for it
/// long enough is refused, not taken as done.
#[test]
fn closing_and_leaving_wait_for_auto_sync() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let release = hold_batches(&mut e);
    edit(&mut e, |r| r.exposure = 0.5);
    assert_eq!(
        e.closing_waits_for().as_deref(),
        Some(crate::app::work::AUTO_SYNC)
    );
    assert!(e.quitting_would_cut_off_work());
    assert!(!e.flush(), "the batch is still held");
    assert!(
        e.messages.line().contains("Auto Sync is still writing"),
        "{}",
        e.messages.line()
    );
    release.send(())?;
    settle(&mut e);
    assert_eq!(e.closing_waits_for(), None);
    assert!(e.flush());
    assert_eq!(saved(&e, &photos[1]).exposure, 0.5);
    Ok(())
}

#[test]
fn with_one_photo_selected_auto_sync_does_nothing() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    e.library.as_mut().unwrap().select(Some(photos[0].0));
    edit(&mut e, |r| r.exposure = 0.5);
    assert!(!e.auto_sync_busy());
    assert_eq!(saved(&e, &photos[1]).exposure, 0.);
    Ok(())
}

/// A History click moves within the open photo's own History and is not synced.
#[test]
fn a_history_click_is_not_synced() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    edit(&mut e, |r| r.exposure = 0.5);
    settle(&mut e);
    let ctx = e.context.clone();
    crate::app::testing::in_edit_frame(&ctx, &mut e, |e| {
        e.document.edit.jump(0);
    });
    panel_frame(&mut e, Vec::new(), FocusedButton::No);
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    assert!(!e.auto_sync_busy());
    assert_eq!(saved(&e, &photos[1]).exposure, 0.5);
    Ok(())
}

/// A virtual copy is a photo of its own; a photo whose file is gone is left as it
/// was, and said quietly.
#[test]
fn copies_are_targets_and_a_missing_file_is_skipped() -> anyhow::Result<()> {
    let (_d, mut e, photos) = auto_syncing()?;
    let copy = e
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(photos[1].0)?
        .value;
    let library = e.library.as_mut().unwrap();
    library.select(Some(photos[0].0));
    library.select_range_to(photos[2].0);
    // Listed after its master, the copy is in the range.
    assert!(library.is_selected(copy));
    std::fs::remove_file(&photos[2].1)?;
    edit(&mut e, |r| r.exposure = 0.5);
    settle(&mut e);
    assert_eq!(saved(&e, &photos[1]).exposure, 0.5);
    assert_eq!(saved_id(&e, copy, &photos[1].1).exposure, 0.5);
    assert!(
        e.messages
            .line()
            .starts_with("Auto Sync: 002.png not changed"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

/// A slider step with 200 photos selected costs the frame little: the batch runs
/// apart from it.
#[test]
fn a_step_with_many_photos_selected_does_not_hold_the_frame() -> anyhow::Result<()> {
    let (_d, mut e, photos) = editor_of(200)?;
    open(&mut e, &photos[0]);
    select(&mut e, &photos[0], &photos[199]);
    e.library.as_mut().unwrap().make_active(photos[0].0);
    e.session.auto_sync = true;
    let frame = Instant::now();
    edit(&mut e, |r| r.exposure = 0.5);
    let frame = frame.elapsed();
    let batch = Instant::now();
    settle(&mut e);
    let batch = batch.elapsed();
    eprintln!("200 photos: the step's frame {frame:?}, the batch {batch:?}");
    assert!(frame < Duration::from_millis(500), "{frame:?}");
    assert_eq!(saved(&e, &photos[199]).exposure, 0.5);
    Ok(())
}

/// A frame of the whole window given `events`, and its accessibility tree.
fn window(e: &mut Editor, events: Vec<egui::Event>) -> egui::accesskit::TreeUpdate {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400., 900.),
            )),
            events,
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
    output
        .platform_output
        .accesskit_update
        .expect("accessibility is on")
}

fn click(e: &mut Editor, label: &str) {
    let tree = window(e, Vec::new());
    let node = labelled(&tree, label).unwrap_or_else(|| panic!("no {label}"));
    let [down, up] = press_and_release(crate::app::testing::centre(node));
    window(e, down);
    window(e, up);
}

#[test]
fn the_filmstrip_bar_copies_pastes_syncs_and_turns_auto_sync() -> anyhow::Result<()> {
    let (_d, mut e, photos) = editor()?;
    e.context.enable_accesskit();
    e.context.all_styles_mut(|s| s.animation_time = 0.);
    open(&mut e, &photos[0]);
    e.library.as_mut().unwrap().select(Some(photos[0].0));
    let tree = window(&mut e, Vec::new());
    let disabled = |tree: &egui::accesskit::TreeUpdate, label: &str| {
        labelled(tree, label)
            .unwrap_or_else(|| panic!("no {label}"))
            .is_disabled()
    };
    // Nothing copied, and no other photo selected.
    assert!(!disabled(&tree, "Copy…"));
    assert!(disabled(&tree, "Paste"));
    assert!(disabled(&tree, "Sync…"));
    let switch = labelled(&tree, "Auto Sync").expect("the switch");
    assert_eq!(switch.role(), egui::accesskit::Role::CheckBox);

    click(&mut e, "Copy…");
    assert!(matches!(&e.modal, Some(Modal::CopySettings(d)) if d.purpose == Transfer::Copy));
    e.close_copy_dialog(CopyChoice::Confirm);
    let tree = window(&mut e, Vec::new());
    assert!(!disabled(&tree, "Paste"));

    let library = e.library.as_mut().unwrap();
    library.select_range_to(photos[1].0);
    let tree = window(&mut e, Vec::new());
    assert!(!disabled(&tree, "Sync…"));
    click(&mut e, "Sync…");
    assert!(matches!(&e.modal, Some(Modal::CopySettings(d)) if d.purpose == Transfer::Sync));
    e.modal = None;

    // The switch turns Auto Sync on, and Sync… reads Auto Sync.
    click(&mut e, "Auto Sync");
    assert!(e.session.auto_sync);
    let tree = window(&mut e, Vec::new());
    assert!(labelled(&tree, "Sync…").is_none());
    let button = labelled(&tree, "Auto Sync, sync now").expect("the Sync button");
    assert!(!button.is_disabled());
    click(&mut e, "Auto Sync, sync now");
    assert!(e.activity.is_syncing() && e.modal.is_none());
    finish_batch(&mut e);
    click(&mut e, "Auto Sync");
    assert!(!e.session.auto_sync);
    Ok(())
}

/// What the bar shows `width` points wide: whether the captions show, and Copy's
/// width, when it shows.
fn bar_at(
    library: &mut crate::app::library::Library,
    current: Option<PhotoId>,
    ctx: &egui::Context,
    width: f32,
) -> (bool, Option<f32>) {
    let buttons = TransferButtons {
        can_copy: true,
        can_paste: true,
        can_sync: true,
        auto_sync: false,
        syncing: false,
    };
    let mut tree = None;
    // The bar measures its parts as it draws them; the second frame shows them.
    for _ in 0..3 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 200.),
                )),
                ..Default::default()
            },
            |ui| {
                library.filmstrip_panel(
                    ui,
                    current,
                    crate::app::library::Module::Develop,
                    Some(buttons),
                );
            },
        );
        output.textures_delta.clear();
        tree = output.platform_output.accesskit_update;
    }
    let tree = tree.expect("accessibility is on");
    let captions = labelled(&tree, "All Photographs").is_some();
    let copy = labelled(&tree, "Copy…").map(|n| {
        let b = n.bounds().expect("placed");
        (b.x1 - b.x0) as f32
    });
    (captions, copy)
}

/// Short of room the bar drops its captions first, then shows the buttons as icons,
/// still named, and leaves them out only when even those don't fit.
#[test]
fn a_narrow_bar_drops_the_captions_then_the_labels() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let (_d, mut library, photos) = library(&ctx)?;
    library.select(Some(photos[0].0));
    let mut stages = Vec::new();
    let mut width = 1400.;
    while width >= 300. {
        let (captions, copy) = bar_at(&mut library, Some(photos[0].0), &ctx, width);
        let stage = match (captions, copy) {
            (true, Some(w)) if w > 40. => 0,
            (false, Some(w)) if w > 40. => 1,
            (false, Some(_)) => 2,
            (true, None) => 3,
            other => panic!("at {width} pt: {other:?}"),
        };
        if stages.last() != Some(&stage) {
            stages.push(stage);
        }
        width -= 10.;
    }
    assert_eq!(stages, [0, 1, 2, 3]);
    Ok(())
}

/// The filmstrip's bar in Develop with the Copy, Paste and Sync buttons: Auto Sync
/// off and on, wide, and narrow with icons, for `RAWMAKASE_UI_SHOTS`
/// ([`crate::app::ui::shots`]).
#[test]
fn shots_of_the_filmstrip_bar_buttons() {
    for (name, width, auto_sync, syncing) in [
        ("filmstrip-bar-sync", 1200., false, false),
        ("filmstrip-bar-auto-sync", 1200., true, true),
        ("filmstrip-bar-no-captions", 620., true, false),
        ("filmstrip-bar-icons", 500., false, false),
    ] {
        crate::app::ui::shots::shot(
            name,
            egui::vec2(width, 150.),
            |e, ctx| {
                let (d, mut library, photos) = library(ctx).unwrap();
                // The catalog outlives the shot's editor.
                std::mem::forget(d);
                library.select(Some(photos[0].0));
                library.select_range_to(photos[2].0);
                e.library = Some(Box::new(library));
                e.document.catalog_photo = Some(photos[0].0);
                e.module = Module::Develop;
            },
            move |e, ctx| {
                let buttons = TransferButtons {
                    can_copy: true,
                    can_paste: false,
                    can_sync: true,
                    auto_sync,
                    syncing,
                };
                let current = e.document.catalog_photo;
                let library = e.library.as_mut().unwrap();
                crate::app::ui::shots::panel(ctx, width - 20., |ui| {
                    ui.set_height(130.);
                    library.filmstrip_panel(
                        ui,
                        current,
                        crate::app::library::Module::Develop,
                        Some(buttons),
                    );
                });
            },
        );
    }
}
