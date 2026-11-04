//! Remove from Catalog in the app, driven by keys and frames as a person
//! would: what the confirmation counts, what dismisses it, what it removes
//! and what the app forgets with it.
use super::*;
use crate::app::testing::editor_with_catalog;
use crate::app::{Editor, Module};
use crate::catalog::PhotoId;
use eframe::egui::{self, Key, Modifiers, Pos2, Rect, Vec2};
use std::time::{Duration, Instant};

/// One frame with `events`.
pub(super) fn frame(e: &mut Editor, events: Vec<egui::Event>) {
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
}

pub(super) fn key(key: Key, pressed: bool, repeat: bool, modifiers: Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat,
        modifiers,
    }
}

/// A key pressed and released.
pub(super) fn tap(e: &mut Editor, k: Key, modifiers: Modifiers) {
    frame(
        e,
        vec![
            key(k, true, false, modifiers),
            key(k, false, false, modifiers),
        ],
    );
}

/// Frames until the workers have answered and the app is idle again, or
/// the confirmation or a dialog is waiting.
pub(super) fn settle(e: &mut Editor) {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        let ctx = e.context.clone();
        e.events(&ctx);
        let settled = |e: &Editor| {
            let waiting = e.file_ops.ordering_cancel().is_some() || e.file_ops.rename_planning();
            !e.activity.is_busy()
                || e.file_ops.confirming()
                || (e.file_ops.dialog_open() && !waiting)
        };
        // Settled once a frame leaves it so: a frame may start what is
        // waited for next (a Rename asked before its plan was in).
        if settled(e) {
            frame(e, Vec::new());
            if settled(e) {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the file operation never settled");
}

pub(super) fn ids_left(e: &Editor) -> Vec<PhotoId> {
    e.library
        .as_ref()
        .unwrap()
        .session
        .photos()
        .iter()
        .map(|p| p.id)
        .collect()
}

/// How many photos the Quick Collection holds.
fn quick_count(e: &Editor) -> usize {
    let session = &e.library.as_ref().unwrap().session;
    session
        .quick_collection()
        .and_then(|q| session.collection_photos().get(&q))
        .map_or(0, |m| m.len())
}

pub(super) fn select(e: &mut Editor, ids: &[PhotoId]) {
    let library = e.library.as_mut().unwrap();
    library.select(ids.first().copied());
    if let Some(last) = ids.last() {
        library.select_range_to(*last);
    }
}

#[test]
fn delete_asks_with_exact_counts_and_cancel_has_the_focus() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let copy = e
        .library
        .as_mut()
        .unwrap()
        .create_virtual_copy(ids[0])?
        .value;
    select(&mut e, &ids[..2]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    let summary = e.file_ops.summary().expect("the confirmation is open");
    assert_eq!(summary.photos, 3, "two photos and a's copy");
    assert_eq!(summary.copies, 1);
    assert_eq!(summary.files, 0);
    // Return alone cancels: Cancel has the focus.
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert!(!e.activity.is_busy());
    assert_eq!(ids_left(&e).len(), 4);
    assert!(ids_left(&e).contains(&copy));
    // Escape too.
    tap(&mut e, Key::Backspace, Modifiers::NONE);
    settle(&mut e);
    assert!(e.file_ops.confirming());
    tap(&mut e, Key::Escape, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert_eq!(ids_left(&e).len(), 4);
    Ok(())
}

#[test]
fn a_held_key_does_nothing_until_released_and_tab_then_return_removes() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    select(&mut e, &ids[..1]);
    // Delete held down: it opens the confirmation, and its repeats, or a
    // Return already down, do nothing while they are held.
    frame(&mut e, vec![key(Key::Delete, true, false, Modifiers::NONE)]);
    settle(&mut e);
    assert!(e.file_ops.confirming());
    for _ in 0..3 {
        frame(&mut e, vec![key(Key::Delete, true, true, Modifiers::NONE)]);
    }
    frame(&mut e, vec![key(Key::Tab, true, false, Modifiers::NONE)]);
    frame(&mut e, vec![key(Key::Enter, true, false, Modifiers::NONE)]);
    assert!(e.file_ops.confirming(), "nothing while Delete is held");
    frame(
        &mut e,
        vec![
            key(Key::Delete, false, false, Modifiers::NONE),
            key(Key::Tab, false, false, Modifiers::NONE),
            key(Key::Enter, false, false, Modifiers::NONE),
        ],
    );
    // Released: Tab moves to Remove from Catalog, Return takes it.
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert_eq!(ids_left(&e), [ids[1]]);
    assert!(d.path().join("photos/a.RAF").is_file(), "the file stays");
    assert!(
        e.messages.line().contains("Removed"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

#[test]
fn option_delete_and_the_menu_ask_the_same() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    select(&mut e, &ids[..1]);
    tap(&mut e, Key::Delete, Modifiers::ALT);
    settle(&mut e);
    assert_eq!(e.file_ops.summary().map(|s| s.photos), Some(1));
    tap(&mut e, Key::Escape, Modifiers::NONE);
    settle(&mut e);
    e.request_photo_removal(vec![ids[1]]);
    settle(&mut e);
    assert_eq!(
        e.file_ops.summary().map(|s| s.names.clone()),
        Some(vec!["b.RAF".into()])
    );
    Ok(())
}

#[test]
fn delete_in_the_quick_collection_takes_photos_out_of_it_only() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.toggle_quick(&ids)?;
    library.show_quick()?;
    select(&mut e, &ids[..2]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming(), "no question for a collection");
    assert_eq!(quick_count(&e), 1);
    assert_eq!(ids_left(&e).len(), 3, "still in the catalog");
    // One undoable step.
    e.sync_undo();
    e.undo();
    assert_eq!(quick_count(&e), 3);
    Ok(())
}

#[test]
fn delete_in_an_imported_collection_changes_nothing_and_says_so() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    let id = library.session.catalog().db_for_tests().query_row(
        "INSERT INTO collections(name, parent, kind) VALUES ('Trip', NULL, 'com.adobe.ag.library.collection') RETURNING id",
        [],
        |r| r.get::<_, i64>(0),
    )?;
    let collection = crate::catalog::CollectionId(id);
    library
        .session
        .catalog_mut_for_tests()
        .change_collection(collection, &ids, &[])?;
    library.session.reload()?;
    library.select_collection(collection);
    select(&mut e, &ids[..1]);
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert_eq!(ids_left(&e).len(), 2);
    assert!(
        e.library
            .as_ref()
            .unwrap()
            .messages
            .line()
            .contains("can't be changed"),
        "{}",
        e.library.as_ref().unwrap().messages.line()
    );
    Ok(())
}

#[test]
fn the_open_edit_is_saved_first_and_a_failed_save_asks_nothing() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.document.catalog_photo = Some(ids[1]);
    e.document.opened_at(d.path().join("photos/b.RAF"));
    let before = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 1.;
    e.commit_edit(before, None);
    // The save fails, as on a full disk: nothing is planned.
    e.library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .db_for_tests()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_save BEFORE UPDATE OF recipe ON photos
             BEGIN SELECT RAISE(FAIL, 'disk full'); END;",
        )?;
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert!(!e.activity.is_busy());
    assert!(e.document.edit.save_state().needs_save());
    // Once it can be saved, it is, before anything is asked.
    e.library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .db_for_tests()
        .execute_batch("DROP TRIGGER fail_save")?;
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    assert!(e.file_ops.confirming());
    assert!(!e.document.edit.save_state().needs_save());
    Ok(())
}

#[test]
fn removing_the_open_photo_closes_it_and_forgets_it() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(d.path().join("photos/a.RAF"));
    e.reference.photo = Some(ids[0]);
    e.library.as_mut().unwrap().edit_metadata(
        ids[0],
        crate::app::library::photo_metadata::Edit::Rating(3),
        false,
    )?;
    e.sync_undo();
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    e.confirm_file_op();
    settle(&mut e);
    assert_eq!(e.document.catalog_photo, None, "nothing can save into it");
    assert!(e.module == Module::Library);
    assert_eq!(e.reference.photo, None);
    assert_eq!(e.undo_log.len(), (0, 0), "its undo goes with it");
    assert_eq!(ids_left(&e), [ids[1]]);
    Ok(())
}

#[test]
fn the_outcome_is_read_from_the_journal_even_when_the_worker_stopped() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    // The run happens, but its thread reports only that it stopped.
    let (batch, plan) = e.file_ops.take_confirmed_for_tests();
    let location = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    let mut other = crate::catalog::Catalog::open(&location)?;
    let _ = crate::catalog::file_ops::execute(&mut other, plan, &Unavailable, Cancel::NEVER)?;
    e.file_op_done(batch, Err("Stopped unexpectedly".into()));
    assert!(!e.activity.is_busy());
    assert_eq!(ids_left(&e), [ids[1]]);
    assert!(
        e.messages.line().contains("Removed"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

#[test]
fn an_outcome_for_another_catalog_is_dropped() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    let (batch, _plan) = e.file_ops.take_confirmed_for_tests();
    // Another catalog is open now.
    let (_d2, other, _) = editor_with_catalog(&["x.RAF"])?;
    e.library = other.library;
    e.file_op_done(batch, Ok(Default::default()));
    assert!(!e.activity.is_busy());
    assert_eq!(ids_left(&e).len(), 1);
    Ok(())
}

#[test]
fn closing_waits_for_a_file_operation() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    assert!(e.file_ops.confirming());
    assert!(e.quitting_would_cut_off_work());
    // Autosave and undo wait while it holds the catalog.
    assert!(e.activity.holds_catalog_writes());
    Ok(())
}

#[test]
fn delete_in_develop_asks_for_the_open_photo_unless_a_tool_is_open() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[1]);
    e.document.opened_at(d.path().join("photos/b.RAF"));
    // A tool takes Delete for its own (a spot, a mask, a guide).
    e.view.tool = crate::app::develop::state::Tool::Crop;
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    e.view.tool = crate::app::develop::state::Tool::None;
    tap(&mut e, Key::Delete, Modifiers::NONE);
    settle(&mut e);
    assert_eq!(
        e.file_ops.summary().map(|s| s.names.clone()),
        Some(vec!["b.RAF".to_string()])
    );
    Ok(())
}

/// An XMP sidecar giving `rating`.
fn rated_sidecar(rating: u8) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="{rating}"/></rdf:RDF></x:xmpmeta>"#
    )
}

#[test]
fn a_file_operation_stops_read_metadata_from_files() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    std::fs::write(d.path().join("photos/a.RAF.xmp"), rated_sidecar(5))?;
    e.library
        .as_mut()
        .unwrap()
        .read_metadata_from_files(&[ids[0]])?;
    e.request_photo_removal(vec![ids[1]]);
    assert!(
        e.library
            .as_ref()
            .unwrap()
            .messages
            .line()
            .contains("stopped"),
        "{}",
        e.library.as_ref().unwrap().messages.line()
    );
    settle(&mut e);
    e.confirm_file_op();
    settle(&mut e);
    // Nothing it read lands afterwards.
    for _ in 0..20 {
        frame(&mut e, Vec::new());
    }
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().rating, 0);
    Ok(())
}

#[test]
fn what_files_said_before_the_catalog_was_read_again_is_dropped() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    std::fs::write(d.path().join("photos/a.RAF.xmp"), rated_sidecar(5))?;
    let library = e.library.as_mut().unwrap();
    library.read_metadata_from_files(&[ids[0]])?;
    // Another change read the catalog again meanwhile.
    library.refresh()?;
    let until = Instant::now() + Duration::from_secs(10);
    while !library.take_reread_finished() && Instant::now() < until {
        library.poll_reread_for_tests();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(library.photo(ids[0]).unwrap().rating, 0);
    assert!(
        library.messages.line().contains("changed"),
        "{}",
        library.messages.line()
    );
    Ok(())
}

#[test]
fn background_reads_stay_stopped_until_the_catalog_is_let_go() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let generation = e.library.as_ref().unwrap().session.generation();
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    assert!(e.file_ops.confirming());
    // What read before is dropped when it comes back.
    assert_ne!(e.library.as_ref().unwrap().session.generation(), generation);
    // Knowing which photos are online again would start them; not now.
    e.library.as_mut().unwrap().refresh()?;
    for _ in 0..50 {
        frame(&mut e, Vec::new());
        std::thread::sleep(Duration::from_millis(2));
    }
    let library = e.library.as_ref().unwrap();
    assert!(!library.session.reading_capture_times());
    assert!(!library.reading_photo_info());
    Ok(())
}

#[test]
fn a_load_of_the_open_photo_under_way_is_dropped_when_it_is_removed() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(d.path().join("photos/a.RAF"));
    let (loading, _) = e.document.start_load();
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    e.confirm_file_op();
    settle(&mut e);
    assert!(
        !e.document.load_counts(loading),
        "its result must not bring it back"
    );
    assert_eq!(e.document.path, None);
    Ok(())
}

#[test]
fn an_unreadable_outcome_forgets_every_photo_that_may_be_gone() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(d.path().join("photos/a.RAF"));
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    let (batch, plan) = e.file_ops.take_confirmed_for_tests();
    let location = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    let mut other = crate::catalog::Catalog::open(&location)?;
    let _ = crate::catalog::file_ops::execute(&mut other, plan, &Unavailable, Cancel::NEVER)?;
    // The journal can't be read back, and the worker said only that it
    // stopped: nothing may be taken for still there.
    other
        .db_for_tests()
        .execute_batch("ALTER TABLE file_operations RENAME TO hidden")?;
    e.file_op_done(batch, Err("Stopped unexpectedly".into()));
    assert!(!e.activity.is_busy());
    assert_eq!(e.document.catalog_photo, None, "it may be gone");
    assert_eq!(ids_left(&e), [ids[1]], "the lists are read again");
    assert!(
        !e.messages.line().contains("not removed"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

#[test]
fn remove_from_catalog_always_means_the_catalog_even_in_a_collection() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.toggle_quick(&ids)?;
    library.show_quick()?;
    select(&mut e, &ids[..1]);
    // ⌥Delete in the Quick Collection asks to remove from the catalog.
    tap(&mut e, Key::Delete, Modifiers::ALT);
    settle(&mut e);
    assert_eq!(e.file_ops.summary().map(|s| s.photos), Some(1));
    assert_eq!(quick_count(&e), 2, "the collection is not changed");
    Ok(())
}

#[test]
fn a_plan_confirmed_for_a_catalog_no_longer_shown_is_not_run() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    let (_d2, other, _) = editor_with_catalog(&["x.RAF"])?;
    e.library = other.library;
    e.confirm_file_op();
    settle(&mut e);
    assert!(!e.activity.is_busy());
    let first = crate::catalog::Catalog::open(d.path().join("test.rawmakase"))?;
    assert_eq!(first.photos()?.len(), 1, "nothing was removed from it");
    Ok(())
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
fn closing_stops_a_running_removal_and_close_anyway_closes() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.request_photo_removal(vec![ids[0]]);
    settle(&mut e);
    // Running, as far as the app knows (its worker not started).
    let _ = e.file_ops.take_confirmed_for_tests();
    let ctx = e.context.clone();
    // A close asks it to stop after the item under way, and waits.
    let mut output = ctx.run_ui(close_request(), |ui| e.pending_work(ui.ctx()));
    output.textures_delta.clear();
    assert!(e.file_ops.stopping());
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose)
    );
    // Close anyway goes, saving nothing while the catalog is held.
    e.close_anyway = true;
    let mut output = ctx.run_ui(close_request(), |ui| e.pending_work(ui.ctx()));
    output.textures_delta.clear();
    assert!(
        !output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose)
    );
    assert!(e.quit_by.is_some());
    Ok(())
}

#[test]
fn stop_asks_the_run_to_stop_between_photos() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    e.request_photo_removal(ids.clone());
    settle(&mut e);
    // Stopped before the worker looks: it removes nothing.
    e.file_ops.stop_first = true;
    e.confirm_file_op();
    assert!(e.file_ops.stopping());
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert_eq!(ids_left(&e), ids);
    Ok(())
}

/// An external drive's mount point as each system names it, and its name.
#[cfg(target_os = "macos")]
const DRIVE: (&str, &str) = ("/Volumes/Samsung SSD T5", "Samsung SSD T5");
#[cfg(all(unix, not(target_os = "macos")))]
const DRIVE: (&str, &str) = ("/media/me/Samsung SSD T5", "Samsung SSD T5");
#[cfg(windows)]
const DRIVE: (&str, &str) = ("E:\\", "E:");

#[test]
fn a_move_to_another_drive_says_it_copies_there_and_trashes_the_originals() {
    let across = crate::catalog::file_ops::Across {
        files: 2,
        companions: 1,
        bytes: 1_500_000_000,
        volume: DRIVE.0.into(),
        available: 26_000_000_000,
        originals_stay: 0,
    };
    assert_eq!(
        across_line(&across),
        format!(
            "2 files (and 1 sidecar file) will be copied to “{}” (1.5 GB of 26 GB free), \
         checked, and then their originals put in the Trash.",
            DRIVE.1
        )
    );
    let small = crate::catalog::file_ops::Across {
        files: 1,
        companions: 0,
        bytes: 24_000_000,
        ..across
    };
    assert_eq!(
        across_line(&small),
        format!(
            "1 file will be copied to “{}” (24 MB of 26 GB free), checked, \
         and then its original put in the Trash.",
            DRIVE.1
        )
    );
    // Under a gigabyte free reads in megabytes, not "0 GB".
    let nearly_full = crate::catalog::file_ops::Across {
        available: 400_000_000,
        ..small
    };
    assert!(
        across_line(&nearly_full).contains("(24 MB of 400 MB free)"),
        "{}",
        across_line(&nearly_full)
    );
}

/// A drive mounted outside /Volumes (a disk image under /tmp) is named by
/// its mount point, not taken for the startup disk.
#[test]
#[cfg(unix)]
fn a_move_names_a_drive_mounted_anywhere() {
    let across = crate::catalog::file_ops::Across {
        files: 1,
        companions: 0,
        bytes: 1_000_000,
        volume: "/tmp/rmk590-mnt".into(),
        available: 26_000_000_000,
        originals_stay: 0,
    };
    assert!(
        across_line(&across).contains("copied to “rmk590-mnt”"),
        "{}",
        across_line(&across)
    );
    assert!(folder_across_line(&across).contains("copied to “rmk590-mnt”"));
    // A separate partition holding the home folder (Linux's /home) is the
    // startup disk, as the Folders panel shows it.
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let above_home = crate::catalog::file_ops::Across {
        volume: home.parent().unwrap_or(&home).into(),
        ..across
    };
    let startup = crate::platform::volume::name_of_mount(std::path::Path::new("/"));
    assert!(
        across_line(&above_home).contains(&format!("copied to “{startup}”")),
        "{}",
        across_line(&above_home)
    );
}

#[test]
fn a_folder_moved_to_another_drive_says_it_copies_all_of_it_there() {
    let across = crate::catalog::file_ops::Across {
        files: 12,
        companions: 0,
        bytes: 1_500_000_000,
        volume: DRIVE.0.into(),
        available: 26_000_000_000,
        originals_stay: 0,
    };
    assert_eq!(
        folder_across_line(&across),
        format!(
            "Its 12 files will be copied to “{}” (1.5 GB of 26 GB free), checked, \
         and then the folder put in the Trash.",
            DRIVE.1
        )
    );
    // Only folders in it: nothing is copied but folders.
    let empty = crate::catalog::file_ops::Across {
        files: 0,
        bytes: 0,
        ..across
    };
    assert_eq!(
        folder_across_line(&empty),
        format!(
            "Its folders will be made on “{}”, and then the folder put in the Trash.",
            DRIVE.1
        )
    );
}

/// macOS has no Trash on an SMB share: the question says what is moved
/// off one keeps its originals there.
#[test]
fn a_move_off_a_network_share_says_its_originals_stay_there() {
    let across = crate::catalog::file_ops::Across {
        files: 2,
        companions: 1,
        bytes: 1_500_000_000,
        volume: DRIVE.0.into(),
        available: 26_000_000_000,
        originals_stay: 2,
    };
    assert_eq!(
        across_line(&across),
        format!(
            "2 files (and 1 sidecar file) will be copied to “{}” (1.5 GB of 26 GB free) \
         and checked. Their originals stay on the network share: it has no Trash.",
            DRIVE.1
        )
    );
    let one = crate::catalog::file_ops::Across {
        files: 1,
        companions: 0,
        bytes: 24_000_000,
        originals_stay: 1,
        ..across.clone()
    };
    assert_eq!(
        across_line(&one),
        format!(
            "1 file will be copied to “{}” (24 MB of 26 GB free) and checked. \
         Its original stays on the network share: it has no Trash.",
            DRIVE.1
        )
    );
    // Some from a share, some from a drive with a Trash.
    let some = crate::catalog::file_ops::Across {
        files: 3,
        companions: 0,
        originals_stay: 1,
        ..across.clone()
    };
    assert_eq!(
        across_line(&some),
        format!(
            "3 files will be copied to “{}” (1.5 GB of 26 GB free), checked, \
         and then their originals put in the Trash. The original of 1 file stays \
         on its network share: it has no Trash.",
            DRIVE.1
        )
    );
    let folder = crate::catalog::file_ops::Across {
        files: 12,
        companions: 0,
        originals_stay: 1,
        ..across
    };
    assert_eq!(
        folder_across_line(&folder),
        format!(
            "Its 12 files will be copied to “{}” (1.5 GB of 26 GB free) and checked. \
         The folder stays on the network share: it has no Trash.",
            DRIVE.1
        )
    );
    let empty = crate::catalog::file_ops::Across {
        files: 0,
        bytes: 0,
        ..folder
    };
    assert_eq!(
        folder_across_line(&empty),
        format!(
            "Its folders will be made on “{}”. The folder stays on the network share: it has no Trash.",
            DRIVE.1
        )
    );
}

#[test]
fn originals_a_move_to_another_drive_kept_are_said() {
    use crate::catalog::file_ops::KeptOriginal;
    assert_eq!(kept_line(&[]), None);
    let kept = [KeptOriginal {
        path: "/Users/me/Photos/a.ARW".into(),
        why: "its drive has no Trash".into(),
    }];
    assert_eq!(
        kept_line(&kept).as_deref(),
        Some(" · 1 original stays where it was beside its copy: a.ARW, its drive has no Trash")
    );
    let two = [kept[0].clone(), kept[0].clone()];
    assert_eq!(
        kept_line(&two).as_deref(),
        Some(
            " · 2 originals stay where they were beside their copies: a.ARW, its drive has no Trash"
        )
    );
}

#[test]
fn a_move_from_two_drives_says_what_is_copied_and_what_is_moved() {
    use crate::catalog::file_ops::{Across, Summary};
    let s = Summary {
        files: 3,
        companions: 1,
        across: Some(Across {
            files: 1,
            companions: 0,
            bytes: 24_000_000,
            volume: DRIVE.0.into(),
            available: 26_000_000_000,
            originals_stay: 0,
        }),
        ..Summary::default()
    };
    assert_eq!(
        move_lines(&s),
        [
            format!(
                "1 file will be copied to “{}” (24 MB of 26 GB free), checked, \
             and then its original put in the Trash.",
                DRIVE.1
            ),
            "2 files (and 1 sidecar file) will be moved on disk.".to_string(),
        ]
    );
    let same_drive = Summary { across: None, ..s };
    assert_eq!(
        move_lines(&same_drive),
        ["3 files (and 1 sidecar file) will be moved on disk."]
    );
}

#[test]
fn stop_says_a_batch_of_photos_finishes_when_photos_go_in_batches() {
    // Moves and renames within a drive go in batches that Stop lets finish
    // (#681, #692); the Trash goes photo by photo.
    let batched = [
        Verb::Rename { one: None },
        Verb::Move { to: "Day".into() },
        Verb::Create {
            name: "Day".into(),
            parent: "Trip".into(),
            moving: true,
        },
    ];
    for verb in batched {
        assert_eq!(verb.stopping(), "Stopping after the photos under way…");
    }
    for verb in [Verb::Trash, Verb::Delete { rejected: false }] {
        assert_eq!(verb.stopping(), "Stopping after the photo under way…");
    }
}
