//! What each view of the work going on makes of it: what closing waits for, the
//! status line's work, and how fast the Library renders edited previews. These
//! read the state that owns each kind of work; set here as the work sets it.
use crate::app::library::Pace;
use crate::app::{Editor, Module, library::sync::BatchProgress};
use eframe::egui;
use std::sync::Arc;

fn editor() -> Editor {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.onboarding.visible = false;
    e
}
fn keys(e: &Editor) -> Vec<String> {
    e.running_work()
        .into_iter()
        .map(|w| w.key.to_string())
        .collect()
}

#[test]
fn closing_waits_for_each_blocking_kind_of_work_by_its_name() {
    let mut e = editor();
    assert_eq!(e.closing_waits_for(), None);
    // Choosing a file is not work closing would cut off.
    assert!(e.activity.begin_dialog());
    assert_eq!(e.closing_waits_for(), None);
    e.activity.finish_dialog();
    assert!(
        e.activity
            .begin_sync(Arc::new(BatchProgress::new("Sync Settings", 3)))
    );
    assert_eq!(e.closing_waits_for().as_deref(), Some("Sync Settings"));
    e.activity.finish_sync();
    assert!(e.activity.begin_folder_change());
    assert_eq!(
        e.closing_waits_for().as_deref(),
        Some(crate::app::work::FOLDER_CHANGE)
    );
    e.activity.finish_dialog();
    assert!(e.activity.begin_file_change());
    assert_eq!(
        e.closing_waits_for().as_deref(),
        Some(crate::app::work::FILE_CHANGE)
    );
    e.activity.finish_file_change();
    e.automation.hold_running_output();
    assert_eq!(
        e.closing_waits_for().as_deref(),
        Some(crate::app::work::OUTPUT)
    );
    e.automation.cancel_outputs();
    assert_eq!(e.closing_waits_for(), None);
}

#[test]
fn a_foreground_operation_is_named_before_a_commands_output() {
    let mut e = editor();
    e.automation.hold_running_output();
    assert!(
        e.activity
            .begin_sync(Arc::new(BatchProgress::new("Auto Settings", 2)))
    );
    assert_eq!(e.closing_waits_for().as_deref(), Some("Auto Settings"));
    e.activity.finish_sync();
    assert!(e.activity.begin_folder_change());
    assert_eq!(
        e.closing_waits_for().as_deref(),
        Some(crate::app::work::FOLDER_CHANGE)
    );
    e.activity.finish_dialog();
    assert!(e.activity.begin_file_change());
    assert_eq!(
        e.closing_waits_for().as_deref(),
        Some(crate::app::work::FILE_CHANGE)
    );
    e.activity.finish_file_change();
    assert_eq!(
        e.closing_waits_for().as_deref(),
        Some(crate::app::work::OUTPUT)
    );
}

#[test]
fn the_status_line_lists_the_running_analyses_and_catalog_work_in_order() {
    let mut e = editor();
    assert!(e.running_work().is_empty());
    e.catalog_work = Some("Opening the catalog…".into());
    e.document.straighten.start();
    e.document.upright.start();
    e.document.camera_look.start();
    e.document.auto.start();
    assert_eq!(
        keys(&e),
        ["auto", "camera look", "upright", "straighten", "catalog"]
    );
    let texts: Vec<String> = e.running_work().into_iter().map(|w| w.text).collect();
    assert_eq!(
        texts,
        [
            "Working out Auto settings…".to_string(),
            "Fitting the camera's look to its JPEG preview…".into(),
            "Analysing for Upright…".into(),
            "Looking for a horizon…".into(),
            "Opening the catalog…".into(),
        ]
    );
    // Gone once each ends.
    e.document.auto.invalidate();
    e.document.upright.invalidate();
    e.catalog_work = None;
    assert_eq!(keys(&e), ["camera look", "straighten"]);
}

#[test]
fn work_with_a_place_of_its_own_is_not_in_the_status_line_and_does_not_block_closing() {
    let mut e = editor();
    // Develop's load shows on the photo; it neither blocks closing nor shows here.
    e.document.start_load();
    assert!(e.running_work().is_empty());
    assert_eq!(e.closing_waits_for(), None);
    // An analysis blocks no close either.
    e.document.auto.start();
    assert_eq!(e.closing_waits_for(), None);
}

#[test]
fn edited_previews_slow_down_for_develops_load_and_a_commands_output_only() {
    let mut e = editor();
    e.module = Module::Library;
    assert_eq!(e.preview_pace(), Pace::Full);
    e.document.start_load();
    assert_eq!(e.preview_pace(), Pace::Background);
    e.document.invalidate_load();
    assert_eq!(e.preview_pace(), Pace::Full);
    e.automation.hold_running_output();
    assert_eq!(e.preview_pace(), Pace::Background);
    e.automation.cancel_outputs();
    assert_eq!(e.preview_pace(), Pace::Full);
    // Work that renders nothing leaves them at full pace.
    e.document.auto.start();
    e.catalog_work = Some("Opening the catalog…".into());
    assert!(
        e.activity
            .begin_sync(Arc::new(BatchProgress::new("Sync Settings", 3)))
    );
    assert_eq!(e.preview_pace(), Pace::Full);
    // Develop in front: always in the background.
    e.module = Module::Develop;
    assert_eq!(e.preview_pace(), Pace::Background);
}

/// Structural: the exit sequence asks the workers to stop in the table's order,
/// the order `stop_workers` kept by hand before the table.
#[test]
fn quitting_stops_the_workers_in_order() {
    use crate::app::work::Kind::*;
    assert_eq!(
        crate::app::work::stopped(),
        [
            Loader,
            Renderer,
            ReferenceLoader,
            HdrPreview,
            Export,
            PreviewBuilds,
            EditableBuilds,
            StandIns,
            Selection,
            AiDenoise,
            AiLoupe,
            Lensfun,
            Outputs,
            Controls,
            FolderSync,
            LibraryPreviews,
        ]
    );
}
