//! Renaming in the app: Rename Photo (F2), Rename Photos in capture order
//! (#342) and Rename Folder, driven by keys and frames on a catalog of
//! synthetic files in a temporary folder.
use super::move_tests::{at, decoded, editor_with_folders, folder, photo, root, type_text};
use super::tests::{frame, select, settle, tap};
use crate::catalog::file_ops;

use crate::app::Module;
use eframe::egui::{Key, Modifiers};

/// Sets the catalog's capture time of the photo whose file is `name`.
fn captured(d: &tempfile::TempDir, name: &str, when: &str) -> anyhow::Result<()> {
    let db = rusqlite::Connection::open(d.path().join("test.rawmakase"))?;
    db.execute(
        "UPDATE photos SET captured=?1 WHERE filename=?2",
        rusqlite::params![when, name],
    )?;
    Ok(())
}

/// Replaces the focused field's text with `text`.
fn retype(e: &mut crate::app::Editor, text: &str) {
    tap(e, Key::A, Modifiers::COMMAND);
    type_text(e, text);
}

#[test]
fn f2_renames_one_photo_and_its_sidecar_and_the_open_photo_follows() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "a.xmp", "b.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    e.module = Module::Library;
    e.document.catalog_photo = Some(a);
    e.document.opened_at(d.path().join("photos/a.RAF"));
    e.document.set_image(decoded(false));
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    settle(&mut e);
    let shown = e.file_ops.rename_shown().expect("Rename Photo is open");
    assert_eq!(shown.title, "Rename Photo “a.RAF”");
    // The name, not its extension, is what is typed over.
    type_text(&mut e, "Keeper");
    settle(&mut e);
    let shown = e.file_ops.rename_shown().unwrap();
    assert_eq!(shown.rows, [("a.RAF".into(), "Keeper.RAF".into(), None)]);
    assert!(shown.can_rename);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/Keeper.RAF").is_file());
    assert!(d.path().join("photos/Keeper.xmp").is_file());
    assert!(!d.path().join("photos/a.RAF").exists());
    assert_eq!(at(&e, &d, a), "Keeper.RAF");
    assert_eq!(e.messages.line(), "Renamed a.RAF to Keeper.RAF");
    let open = e.document.path.clone().unwrap();
    assert!(open.ends_with("photos/Keeper.RAF"), "{}", open.display());
    Ok(())
}

#[test]
fn a_name_taken_there_is_said_at_once_and_renames_nothing() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "b.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    frame(&mut e, Vec::new());
    type_text(&mut e, "B");
    settle(&mut e);
    let shown = e.file_ops.rename_shown().unwrap();
    assert!(!shown.can_rename);
    assert!(shown.rows[0].2.is_some(), "{:?}", shown.rows);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(e.file_ops.rename_shown().is_some(), "still asking");
    assert!(d.path().join("photos/a.RAF").is_file());
    // Escape lets the catalog go.
    tap(&mut e, Key::Escape, Modifiers::NONE);
    frame(&mut e, Vec::new());
    assert!(e.file_ops.rename_shown().is_none());
    assert!(!e.activity.is_busy());
    Ok(())
}

#[test]
fn several_photos_are_renamed_in_the_order_they_were_taken() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["x.RAF", "y.RAF", "z.RAF"])?;
    captured(&d, "x.RAF", "2026-05-04T12:00:03")?;
    captured(&d, "y.RAF", "2026-05-04T12:00:01")?;
    captured(&d, "z.RAF", "2026-05-04T12:00:02")?;
    let ids: Vec<_> = ["x.RAF", "y.RAF", "z.RAF"]
        .iter()
        .map(|n| photo(&e, &d, n))
        .collect();
    select(&mut e, &ids);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    settle(&mut e);
    let shown = e.file_ops.rename_shown().expect("Rename Photos is open");
    assert_eq!(shown.title, "Rename 3 Photos");
    type_text(&mut e, "Trip");
    settle(&mut e);
    let shown = e.file_ops.rename_shown().unwrap();
    let names: Vec<(&str, &str)> = shown
        .rows
        .iter()
        .map(|(old, new, _)| (old.as_str(), new.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("y.RAF", "Trip-1.RAF"),
            ("z.RAF", "Trip-2.RAF"),
            ("x.RAF", "Trip-3.RAF")
        ]
    );
    assert!(shown.can_rename);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert_eq!(at(&e, &d, ids[1]), "Trip-1.RAF");
    assert_eq!(at(&e, &d, ids[2]), "Trip-2.RAF");
    assert_eq!(at(&e, &d, ids[0]), "Trip-3.RAF");
    assert_eq!(e.messages.line(), "Renamed 3 photos");
    Ok(())
}

#[test]
fn a_batch_with_one_name_taken_renames_none() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "b.RAF", "Trip-2.RAF"])?;
    let (a, b) = (photo(&e, &d, "a.RAF"), photo(&e, &d, "b.RAF"));
    select(&mut e, &[a, b]);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    frame(&mut e, Vec::new());
    type_text(&mut e, "Trip");
    settle(&mut e);
    let shown = e.file_ops.rename_shown().unwrap();
    assert!(!shown.can_rename);
    assert!(shown.rows.iter().all(|r| r.2.is_some()), "{:?}", shown.rows);
    tap(&mut e, Key::Escape, Modifiers::NONE);
    frame(&mut e, Vec::new());
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/a.RAF").is_file());
    Ok(())
}

#[test]
fn rename_folder_renames_it_on_disk_and_in_the_catalog() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["trip/a.RAF", "trip/day1/b.RAF"])?;
    let (a, b) = (
        photo(&e, &d, "trip/a.RAF"),
        photo(&e, &d, "trip/day1/b.RAF"),
    );
    let trip = folder(&e, "trip").unwrap();
    let r = root(&e);
    e.rename_folder(r, "trip".into());
    frame(&mut e, Vec::new());
    retype(&mut e, "Iceland");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/Iceland/day1/b.RAF").is_file());
    assert!(!d.path().join("photos/trip").exists());
    assert_eq!(at(&e, &d, a), "Iceland/a.RAF");
    assert_eq!(at(&e, &d, b), "Iceland/day1/b.RAF");
    assert_eq!(folder(&e, "Iceland"), Some(trip), "the same folder");
    assert_eq!(e.messages.line(), "Renamed folder trip to Iceland");
    Ok(())
}

#[test]
fn a_folder_name_taken_there_renames_nothing_and_says_why() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["trip/a.RAF", "other/b.RAF"])?;
    let r = root(&e);
    e.rename_folder(r, "trip".into());
    frame(&mut e, Vec::new());
    retype(&mut e, "other");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/trip/a.RAF").is_file());
    assert!(
        e.messages.line().contains("already there"),
        "{}",
        e.messages.line()
    );
    Ok(())
}

#[test]
fn a_name_typed_and_returned_in_one_frame_is_the_one_renamed() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    frame(&mut e, Vec::new());
    type_text(&mut e, "Keeper");
    frame(&mut e, Vec::new());
    // More typed, and Return, before the next frame plans it.
    frame(
        &mut e,
        vec![
            eframe::egui::Event::Text("2".into()),
            super::tests::key(Key::Enter, true, false, Modifiers::NONE),
            super::tests::key(Key::Enter, false, false, Modifiers::NONE),
        ],
    );
    settle(&mut e);
    assert_eq!(at(&e, &d, a), "Keeper2.RAF");
    Ok(())
}

#[test]
fn reading_the_photos_order_can_be_cancelled_and_its_answer_is_then_dropped() -> anyhow::Result<()>
{
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "b.RAF"])?;
    let ids = vec![photo(&e, &d, "a.RAF"), photo(&e, &d, "b.RAF")];
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    let order = file_ops::rename_order(e.library.as_ref().unwrap().session.catalog(), &ids)?;
    e.request_rename(ids);
    assert!(e.activity.is_busy(), "the catalog is held while it reads");
    let cancel = e.file_ops.ordering_cancel().expect("reading");
    e.cancel_ordering();
    assert!(
        cancel.load(std::sync::atomic::Ordering::Relaxed),
        "the worker is told"
    );
    assert!(!e.activity.is_busy());
    // Its answer, coming after, opens nothing.
    e.rename_ordered(catalog, cancel, Ok(order));
    assert!(e.file_ops.rename_shown().is_none());
    assert!(!e.activity.is_busy());
    Ok(())
}

#[test]
fn an_empty_folder_renamed_is_shown_by_its_new_name() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    let r = root(&e);
    e.name_new_folder(r, String::new(), "photos".into(), Vec::new(), false);
    frame(&mut e, Vec::new());
    type_text(&mut e, "empty");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    let empty = folder(&e, "empty").expect("made");
    e.rename_folder(r, "empty".into());
    frame(&mut e, Vec::new());
    retype(&mut e, "Selects");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/Selects").is_dir());
    assert_eq!(folder(&e, "Selects"), Some(empty));
    assert!(folder(&e, "empty").is_none());
    Ok(())
}

#[test]
fn a_name_made_invalid_in_the_frame_it_is_returned_keeps_the_dialog() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    frame(&mut e, Vec::new());
    type_text(&mut e, "Keeper");
    frame(&mut e, Vec::new());
    frame(
        &mut e,
        vec![
            eframe::egui::Event::Text("/".into()),
            super::tests::key(Key::Enter, true, false, Modifiers::NONE),
            super::tests::key(Key::Enter, false, false, Modifiers::NONE),
        ],
    );
    settle(&mut e);
    let shown = e.file_ops.rename_shown().expect("still asking");
    assert!(!shown.can_rename);
    assert!(shown.rows[0].2.is_some(), "{:?}", shown.rows);
    assert!(d.path().join("photos/a.RAF").is_file());
    tap(&mut e, Key::Escape, Modifiers::NONE);
    frame(&mut e, Vec::new());
    assert!(!e.activity.is_busy(), "the catalog is let go");
    Ok(())
}

#[test]
fn a_plan_answering_other_input_a_failed_one_or_one_for_a_closed_dialog_runs_nothing()
-> anyhow::Result<()> {
    use super::rename::RenamePlanned;
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    // The name as it opened, planned.
    settle(&mut e);
    assert!(e.file_ops.rename_shown().is_some());
    // Typing plans on a worker; the frame does not wait for it.
    type_text(&mut e, "Keeper");
    let keeper = e.file_ops.rename_request().expect("planned on a worker");
    assert!(e.file_ops.rename_shown().is_none(), "not planned yet");
    // An answer to another plan is dropped.
    e.rename_planned(RenamePlanned::failed(keeper + 1000, "another dialog's"));
    assert_eq!(e.file_ops.rename_request(), Some(keeper));
    // Its worker failing says so, and renames nothing.
    e.rename_planned(RenamePlanned::failed(keeper, "Stopped unexpectedly: boom"));
    let shown = e.file_ops.rename_shown().expect("answered");
    assert!(!shown.can_rename);
    assert_eq!(
        shown.rows,
        [(
            "a.RAF".into(),
            "Keeper.RAF".into(),
            Some("Stopped unexpectedly: boom".into())
        )]
    );
    // Its real answer, coming after, is dropped too.
    settle(&mut e);
    assert!(!e.file_ops.rename_shown().unwrap().can_rename);
    // Typed again, and closed before that plan is in.
    type_text(&mut e, "2");
    let late = e.file_ops.rename_request().expect("planned on a worker");
    tap(&mut e, Key::Escape, Modifiers::NONE);
    assert!(e.file_ops.rename_shown().is_none());
    assert!(!e.activity.is_busy(), "the catalog is let go");
    e.rename_planned(RenamePlanned::failed(late, "late"));
    settle(&mut e);
    assert!(e.file_ops.rename_shown().is_none(), "nothing opens again");
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/a.RAF").is_file());
    Ok(())
}

/// Rename Photo open on `a.RAF` with "Keeper" typed and Return pressed
/// before its plan is in, plans held back from their workers.
fn returned_before_planned(
    names: &[&str],
) -> anyhow::Result<(tempfile::TempDir, crate::app::Editor)> {
    let (d, mut e, _) = editor_with_folders(names)?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    settle(&mut e);
    e.file_ops.hold_plans = true;
    type_text(&mut e, "Keeper");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    assert!(e.file_ops.rename_planning());
    assert!(e.file_ops.rename_shown().is_none(), "not planned yet");
    Ok((d, e))
}

#[test]
fn escape_in_the_frame_a_rename_asked_for_is_planned_cancels_it() -> anyhow::Result<()> {
    let (d, mut e) = returned_before_planned(&["a.RAF"])?;
    // The plan lands, and the frame it is read in brings Escape.
    let answer = e.rename_answer_now().expect("planning");
    e.rename_planned(answer);
    assert!(e.file_ops.rename_shown().unwrap().can_rename);
    assert!(
        d.path().join("photos/a.RAF").is_file(),
        "not run on arrival"
    );
    tap(&mut e, Key::Escape, Modifiers::NONE);
    settle(&mut e);
    assert!(e.file_ops.rename_shown().is_none());
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/a.RAF").is_file());
    assert!(!d.path().join("photos/Keeper.RAF").exists());
    Ok(())
}

#[test]
fn a_rename_asked_for_before_its_plan_runs_in_the_frame_after_it_lands() -> anyhow::Result<()> {
    let (d, mut e) = returned_before_planned(&["a.RAF"])?;
    let answer = e.rename_answer_now().expect("planning");
    e.rename_planned(answer);
    assert!(
        d.path().join("photos/a.RAF").is_file(),
        "not run on arrival"
    );
    e.file_ops.hold_plans = false;
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/Keeper.RAF").is_file());
    Ok(())
}

#[test]
fn typing_in_the_frame_a_rename_asked_for_is_planned_waits_for_another_return() -> anyhow::Result<()>
{
    let (d, mut e) = returned_before_planned(&["a.RAF"])?;
    // Back in the field while its plan is still under way.
    tap(&mut e, Key::Tab, Modifiers::NONE);
    assert!(e.file_ops.rename_planning());
    // The plan lands, and the frame it is read in types more.
    let answer = e.rename_answer_now().expect("planning");
    e.rename_planned(answer);
    type_text(&mut e, "2");
    assert!(
        d.path().join("photos/a.RAF").is_file(),
        "what was asked is not run"
    );
    let answer = e.rename_answer_now().expect("Keeper2 planned");
    e.rename_planned(answer);
    frame(&mut e, Vec::new());
    let shown = e.file_ops.rename_shown().expect("still asking");
    assert_eq!(shown.rows[0].1, "Keeper2.RAF");
    assert!(d.path().join("photos/a.RAF").is_file(), "waits for Return");
    // Asked again, it renames what was typed.
    e.file_ops.hold_plans = false;
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/Keeper2.RAF").is_file());
    Ok(())
}

/// A reader naming every camera "Sony ILCE-7RM6", one photo each time
/// `gate` lets it through.
fn gated_reader() -> (
    crate::app::export::naming::read::Reader,
    std::sync::mpsc::Sender<()>,
) {
    use crate::app::export::naming::read::{FileFields, ToRead};
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let gate = std::sync::Mutex::new(gate);
    let reader: crate::app::export::naming::read::Reader =
        std::sync::Arc::new(move |_: &ToRead| {
            let _ = gate.lock().unwrap().recv();
            Ok(FileFields {
                camera: crate::export_settings::CameraNames::Read {
                    make: Some("Sony".into()),
                    model: Some("ILCE-7RM6".into()),
                },
                ..Default::default()
            })
        });
    (reader, open)
}

/// Takes the editor's events until `done`.
fn until(e: &mut crate::app::Editor, done: impl Fn(&crate::app::Editor) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !done(e) {
        assert!(std::time::Instant::now() < deadline, "never happened");
        let ctx = e.context.clone();
        e.events(&ctx);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

const MODEL: &str = "{{com.adobe.model}}-{{naming_sequenceNumber_2Digits}}";

/// File Naming > Edit… with `tokens`, the editor then on top.
fn edit(e: &mut crate::app::Editor, tokens: &str) {
    e.file_ops.rename_edit(tokens);
    frame(e, Vec::new());
    frame(e, Vec::new());
}

/// Rename Photos opened on x.RAF and y.RAF (y taken first), with its files
/// read by `reader`.
fn rename_two(
    reader: crate::app::export::naming::read::Reader,
) -> anyhow::Result<(
    tempfile::TempDir,
    crate::app::Editor,
    Vec<crate::catalog::PhotoId>,
)> {
    let (d, mut e, _) = editor_with_folders(&["x.RAF", "y.RAF"])?;
    captured(&d, "x.RAF", "2026-05-04T12:00:03")?;
    captured(&d, "y.RAF", "2026-05-04T12:00:01")?;
    let ids: Vec<_> = ["x.RAF", "y.RAF"]
        .iter()
        .map(|n| photo(&e, &d, n))
        .collect();
    e.file_ops.name_reader = Some(reader);
    select(&mut e, &ids);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    settle(&mut e);
    Ok((d, e, ids))
}

/// File Naming > Edit…: a template naming the camera is made in the
/// editor; Rename waits for the files to be read, the names then have the
/// camera, Rename renames by them, and the next Rename Photos starts from
/// that template.
#[test]
fn several_photos_are_named_by_a_template_made_in_the_editor() -> anyhow::Result<()> {
    let (reader, open) = gated_reader();
    let (d, mut e, ids) = rename_two(reader)?;
    edit(&mut e, MODEL);
    assert!(
        e.file_ops.rename_reading(),
        "the editor's Example reads them"
    );
    // Return is the editor's Done.
    tap(&mut e, Key::Enter, Modifiers::NONE);
    assert!(!e.file_ops.rename_editor_open());
    // While the files are read, Return renames nothing.
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/x.RAF").is_file());
    open.send(())?;
    open.send(())?;
    until(&mut e, |e| !e.file_ops.rename_reading());
    settle(&mut e);
    let shown = e.file_ops.rename_shown().expect("planned with the cameras");
    let names: Vec<&str> = shown.rows.iter().map(|(_, new, _)| new.as_str()).collect();
    assert_eq!(names, ["ILCE-7RM6-01.RAF", "ILCE-7RM6-02.RAF"]);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert_eq!(at(&e, &d, ids[1]), "ILCE-7RM6-01.RAF");
    assert_eq!(at(&e, &d, ids[0]), "ILCE-7RM6-02.RAF");
    assert_eq!(e.file_ops.rename_settings.tokens.as_deref(), Some(MODEL));
    // The next Rename Photos starts from it, and reads the files again.
    let (reader, open) = gated_reader();
    e.file_ops.name_reader = Some(reader);
    select(&mut e, &ids);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    settle(&mut e);
    assert!(e.file_ops.rename_reading());
    open.send(())?;
    open.send(())?;
    until(&mut e, |e| !e.file_ops.rename_reading());
    settle(&mut e);
    let shown = e.file_ops.rename_shown().unwrap();
    assert_eq!(shown.rows[0].1, "ILCE-7RM6-01.RAF", "same name again");
    Ok(())
}

/// Escape closes the editor first, then the dialog, which stops its read
/// and lets the catalog go; the read's late completion changes nothing.
#[test]
fn escape_closes_the_editor_then_the_dialog_and_its_read() -> anyhow::Result<()> {
    let (reader, open) = gated_reader();
    let (d, mut e, _) = rename_two(reader)?;
    edit(&mut e, MODEL);
    assert!(e.file_ops.rename_reading());
    tap(&mut e, Key::Escape, Modifiers::NONE);
    assert!(!e.file_ops.rename_editor_open());
    assert!(e.file_ops.dialog_open(), "the dialog stays");
    frame(&mut e, Vec::new());
    tap(&mut e, Key::Escape, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.file_ops.dialog_open());
    assert!(!e.activity.is_busy());
    open.send(())?;
    open.send(())?;
    for _ in 0..20 {
        let ctx = e.context.clone();
        e.events(&ctx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!e.file_ops.dialog_open());
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/x.RAF").is_file());
    assert_eq!(
        e.file_ops.rename_settings,
        Default::default(),
        "nothing kept"
    );
    Ok(())
}

/// A file that can't be read: the names say what they lack, and Rename
/// still renames by what could be read.
#[test]
fn a_failed_read_is_said_and_rename_goes_on_without_it() -> anyhow::Result<()> {
    let reader: crate::app::export::naming::read::Reader =
        std::sync::Arc::new(|_: &crate::app::export::naming::read::ToRead| {
            Err("disk gone".to_string())
        });
    let (d, mut e, ids) = rename_two(reader)?;
    edit(&mut e, MODEL);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    until(&mut e, |e| !e.file_ops.rename_reading());
    settle(&mut e);
    let shown = e.file_ops.rename_shown().unwrap();
    assert_eq!(shown.rows[0].1, "-01.RAF");
    assert!(
        shown.rows[0]
            .2
            .as_deref()
            .is_some_and(|p| p.contains("make and model were not read")),
        "{:?}",
        shown.rows[0]
    );
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert_eq!(at(&e, &d, ids[1]), "-01.RAF");
    Ok(())
}

/// A read the dialog's template no longer needs (the editor that wanted it
/// was cancelled) doesn't drop a Rename asked for before it ends.
#[test]
fn a_read_no_longer_needed_lets_a_rename_asked_for_run() -> anyhow::Result<()> {
    let (reader, open) = gated_reader();
    let (d, mut e, ids) = rename_two(reader)?;
    edit(&mut e, MODEL);
    assert!(e.file_ops.rename_reading());
    tap(&mut e, Key::Escape, Modifiers::NONE);
    assert!(!e.file_ops.rename_editor_open());
    settle(&mut e);
    e.file_ops.hold_plans = true;
    e.file_ops.rename_custom_text("Trip");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    assert!(e.file_ops.rename_planning(), "asked before its plan is in");
    // The read ends before the plan of what was asked is in.
    open.send(())?;
    open.send(())?;
    until(&mut e, |e| !e.file_ops.rename_reading());
    let answer = e.rename_answer_now().expect("planning");
    e.rename_planned(answer);
    e.file_ops.hold_plans = false;
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert_eq!(at(&e, &d, ids[1]), "Trip-1.RAF");
    Ok(())
}

/// A plan made before the files were read, of a template that needs them,
/// never comes back as current: picked again after the read ended under
/// another template, it is planned again with what the files gave.
#[test]
fn a_plan_from_before_the_read_is_not_taken_again() -> anyhow::Result<()> {
    let (reader, open) = gated_reader();
    let (d, mut e, ids) = rename_two(reader)?;
    edit(&mut e, MODEL);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(e.file_ops.rename_reading());
    // Planned without the cameras while they are read.
    assert!(e.file_ops.rename_shown().is_some());
    e.file_ops.hold_plans = true;
    e.file_ops
        .rename_choose("{{image_name}}-{{naming_sequenceNumber_2Digits}}");
    frame(&mut e, Vec::new());
    open.send(())?;
    open.send(())?;
    until(&mut e, |e| !e.file_ops.rename_reading());
    e.file_ops.rename_choose(MODEL);
    frame(&mut e, Vec::new());
    assert!(
        e.file_ops.rename_shown().is_none(),
        "the plan without the cameras is not shown as current"
    );
    // The plan of the other template lands, then the template's again.
    let answer = e.rename_answer_now().expect("planning");
    e.rename_planned(answer);
    let answer = e.rename_answer_now().expect("planned again");
    e.rename_planned(answer);
    e.file_ops.hold_plans = false;
    settle(&mut e);
    let shown = e.file_ops.rename_shown().expect("planned with the cameras");
    assert_eq!(shown.rows[0].1, "ILCE-7RM6-01.RAF");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert_eq!(at(&e, &d, ids[1]), "ILCE-7RM6-01.RAF");
    Ok(())
}
