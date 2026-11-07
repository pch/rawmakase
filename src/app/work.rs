//! The kinds of work the app runs, in one ordered table, each with its whole
//! policy: whether closing waits for it, whether the status line lists it,
//! whether it slows the Library's edited previews or pauses the checksum
//! reader, and its step of the exit sequence. Each reads the state that owns
//! the work (a `Task`, `Activity`, a queue, a service); none keeps a busy flag of
//! its own. The views of what is running (`Editor::closing_waits_for`,
//! `running_work`, `preview_pace`, the checksum pause and `stop_workers`) are
//! read from it, in its order: closing names the first that blocks it, the
//! status line lists them in order, and the exit sequence stops them in order.
//!
//! Changing a row's policy (AI Denoise blocking close, say) changes what the
//! app does: it is a change of behaviour of its own, not a refactor.
use crate::app::shell::messages::Work;
use crate::app::{Editor, task};
use std::borrow::Cow;

/// A kind of work, for the table's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Kind {
    Loader,
    Renderer,
    ReferenceLoader,
    HdrPreview,
    Export,
    Batch,
    FolderChange,
    FileChange,
    PreviewBuilds,
    EditableBuilds,
    StandIns,
    Selection,
    AiDenoise,
    AiLoupe,
    Auto,
    CameraLook,
    Upright,
    Straighten,
    Lensfun,
    MetadataReread,
    CatalogWork,
    Import,
    ProfileDownload,
    Outputs,
    Controls,
    FolderSync,
    LibraryPreviews,
}

/// What the close guard calls work that closing waits for.
#[derive(Clone, Copy)]
enum Label {
    Fixed(&'static str),
    /// The batch edit's own name: Sync Settings, Auto Settings or a preset's.
    Batch,
}

/// What the close guard calls an export or preview a control command started.
/// `pending_work` matches on these names to offer what can be done instead.
pub(in crate::app) const OUTPUT: &str = "the output the control socket asked for";
/// What the close guard calls a folder change.
pub(in crate::app) const FOLDER_CHANGE: &str = "the folder change";
/// What the close guard calls a file operation.
pub(in crate::app) const FILE_CHANGE: &str = "the file operation";

/// One kind of work and its policy.
struct Row {
    /// Which kind of work this is, for the stop-order test.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the stop-order test"))]
    kind: Kind,
    /// Whether it is under way, read from the state that owns it; for the rows
    /// closing, preview pace or the checksum pause read.
    running: Option<fn(&Editor) -> bool>,
    /// Closing waits for it while it runs, naming it so.
    closing: Option<Label>,
    /// Its entry in the status line while it runs.
    status: Option<fn(&Editor) -> Option<Work>>,
    /// The Library's edited previews go to the background while it runs.
    slows_previews: bool,
    /// The checksum reader pauses while it runs.
    pauses_checksums: bool,
    /// Its step of the exit sequence: asked to stop, to be waited for.
    stop: Option<fn(&mut Editor) -> Vec<task::Stopping>>,
}

fn work(key: &'static str, text: &str) -> Work {
    Work {
        key,
        text: text.to_string(),
    }
}

/// Every kind of work, in the order closing names them, the status line lists
/// them and quitting stops them. Where a row has no status-line entry, its
/// comment says where its progress shows instead.
const WORK: &[Row] = &[
    // Shows on the photo (its badge) and the Develop toolbar's spinner.
    Row {
        kind: Kind::Loader,
        running: Some(|e| e.document.loading()),
        closing: None,
        status: None,
        slows_previews: true,
        pauses_checksums: true,
        stop: Some(|e| Vec::from(e.loader.stop())),
    },
    // The Develop toolbar's spinner, shared with the loader.
    Row {
        kind: Kind::Renderer,
        running: None,
        closing: None,
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: Some(|e| vec![e.renderer.stop()]),
    },
    // Reference View's own pane.
    Row {
        kind: Kind::ReferenceLoader,
        running: None,
        closing: None,
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: Some(|e| vec![e.reference_loader.stop()]),
    },
    // Develop's HDR preview, on a worker of its own.
    Row {
        kind: Kind::HdrPreview,
        running: None,
        closing: None,
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: Some(|e| vec![e.hdr_preview.stop()]),
    },
    // Its progress in the bar. Queued exports count, not only the one running.
    Row {
        kind: Kind::Export,
        running: Some(|e| e.exporting()),
        closing: Some(Label::Fixed("the export")),
        status: None,
        slows_previews: true,
        pauses_checksums: true,
        stop: Some(|e| vec![task::Stopping::new(e.exports.close())]),
    },
    // Its window, with its progress and Cancel.
    Row {
        kind: Kind::Batch,
        running: Some(|e| e.activity.batch().is_some()),
        closing: Some(Label::Batch),
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    // Its dialog, until the catalog has opened again.
    Row {
        kind: Kind::FolderChange,
        running: Some(|e| e.activity.is_changing_folder()),
        closing: Some(Label::Fixed(FOLDER_CHANGE)),
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    // Its progress in the bar, from planning and confirming to the outcome.
    Row {
        kind: Kind::FileChange,
        running: Some(|e| e.activity.is_changing_files()),
        closing: Some(Label::Fixed(FILE_CHANGE)),
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    // Its progress in the bar; a finished build's report is not running.
    Row {
        kind: Kind::PreviewBuilds,
        running: Some(|e| e.preview_builds.progress().is_some_and(|p| p.running())),
        closing: None,
        status: None,
        slows_previews: true,
        pauses_checksums: true,
        stop: Some(|e| vec![task::Stopping::new(e.preview_builds.close())]),
    },
    // Its progress in the bar, beside the preview builds'. Closing doesn't wait:
    // the photo being built is cancelled, recording nothing, and its original is
    // still there to build from.
    Row {
        kind: Kind::EditableBuilds,
        running: Some(|e| e.editable_builds.running()),
        closing: None,
        status: None,
        slows_previews: true,
        pauses_checksums: true,
        stop: Some(|e| vec![task::Stopping::new(e.editable_builds.close())]),
    },
    // Stand-ins replace a preview where it shows.
    Row {
        kind: Kind::StandIns,
        running: None,
        closing: None,
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: Some(|e| Vec::from(e.stand_ins.stop())),
    },
    // Its tile (Subject, Sky); the model installers and the upgrade thread are
    // stopped with it.
    Row {
        kind: Kind::Selection,
        running: None,
        closing: None,
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: Some(|e| e.selection.stop()),
    },
    // Detail's AI Denoise status line.
    Row {
        kind: Kind::AiDenoise,
        running: Some(|e| e.ai_denoise.busy()),
        closing: None,
        status: None,
        slows_previews: true,
        pauses_checksums: false,
        stop: Some(|e| vec![task::Stopping::new(e.ai_denoise.stop())]),
    },
    // The Loupe's load of a finished AI Denoise result. Nothing shows while it
    // runs: the photo stays plain until the result arrives.
    Row {
        kind: Kind::AiLoupe,
        running: None,
        closing: None,
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: Some(|e| vec![e.ai_loupe.stop()]),
    },
    Row {
        kind: Kind::Auto,
        running: None,
        closing: None,
        status: Some(|e| {
            e.document
                .auto
                .is_running()
                .then(|| work("auto", "Working out Auto settings…"))
        }),
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    Row {
        kind: Kind::CameraLook,
        running: None,
        closing: None,
        status: Some(|e| {
            e.document.camera_look.is_running().then(|| {
                work(
                    "camera look",
                    "Fitting the camera's look to its JPEG preview…",
                )
            })
        }),
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    Row {
        kind: Kind::Upright,
        running: None,
        closing: None,
        status: Some(|e| {
            e.document
                .upright
                .is_running()
                .then(|| work("upright", "Analysing for Upright…"))
        }),
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    Row {
        kind: Kind::Straighten,
        running: None,
        closing: None,
        status: Some(|e| {
            e.document
                .straighten
                .is_running()
                .then(|| work("straighten", "Looking for a horizon…"))
        }),
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    Row {
        kind: Kind::Lensfun,
        running: None,
        closing: None,
        status: Some(|e| {
            e.lensfun
                .running()
                .map(|job| work("lensfun", job.running_text()))
        }),
        slows_previews: false,
        pauses_checksums: false,
        stop: Some(|e| e.lensfun.stop()),
    },
    Row {
        kind: Kind::MetadataReread,
        running: None,
        closing: None,
        status: Some(|e| {
            e.library
                .as_ref()
                .is_some_and(|l| l.rereading())
                .then(|| work("metadata", "Reading metadata from files…"))
        }),
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    Row {
        kind: Kind::CatalogWork,
        running: None,
        closing: None,
        status: Some(|e| e.catalog_work.as_deref().map(|text| work("catalog", text))),
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    Row {
        kind: Kind::Import,
        running: None,
        closing: None,
        status: Some(|e| {
            e.importing
                .as_ref()
                .map(|progress| work("import", &progress.text()))
        }),
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    // A library operation: shown wherever the user is, not only in its offer.
    Row {
        kind: Kind::ProfileDownload,
        running: None,
        closing: None,
        status: Some(|e| {
            e.profile_download
                .downloading()
                .map(|name| work("profile download", &format!("Downloading {name}…")))
        }),
        slows_previews: false,
        pauses_checksums: false,
        stop: None,
    },
    // An output a command started reports to the command; counts while running
    // and not cancelled.
    Row {
        kind: Kind::Outputs,
        running: Some(|e| e.automation.outputs_running()),
        closing: Some(Label::Fixed(OUTPUT)),
        status: None,
        slows_previews: true,
        pauses_checksums: false,
        stop: Some(|e| e.automation.stop_outputs()),
    },
    // The MIDI listeners and the control socket (their requests close earlier,
    // with the other channels).
    Row {
        kind: Kind::Controls,
        running: None,
        closing: None,
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: Some(|e| e.controls.stop()),
    },
    // Its own progress in the folder's row.
    Row {
        kind: Kind::FolderSync,
        running: None,
        closing: None,
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: Some(|e| e.stop_folder_sync().into_iter().collect()),
    },
    // Their own spinner and "Preparing previews…"/"Rendering …", beside Auto
    // Advance in the Library and in Develop's status row.
    Row {
        kind: Kind::LibraryPreviews,
        running: None,
        closing: None,
        status: None,
        slows_previews: false,
        pauses_checksums: false,
        stop: Some(|e| {
            e.library
                .as_mut()
                .map(|library| library.close_previews())
                .unwrap_or_default()
        }),
    },
];

fn running(e: &Editor, row: &Row) -> bool {
    row.running.is_some_and(|running| running(e))
}

impl Editor {
    /// The work closing waits for, as the close guard names it: closing would cut
    /// it off, and nothing can be saved in its place.
    pub(in crate::app) fn closing_waits_for(&self) -> Option<Cow<'static, str>> {
        WORK.iter()
            .filter(|row| row.closing.is_some() && running(self, row))
            .find_map(|row| match row.closing? {
                Label::Fixed(text) => Some(text.into()),
                Label::Batch => self.activity.batch().map(|b| b.name().to_string().into()),
            })
    }
    /// What the user started that is still running and has no place of its own
    /// that says so, in the table's order.
    pub(in crate::app) fn running_work(&self) -> Vec<Work> {
        WORK.iter()
            .filter_map(|row| row.status.and_then(|status| status(self)))
            .collect()
    }
    /// Whether work that renders or reads photos runs, which edited previews
    /// make way for.
    pub(in crate::app) fn work_slows_previews(&self) -> bool {
        WORK.iter()
            .any(|row| row.slows_previews && running(self, row))
    }
    /// Whether heavier file reading runs, which the checksum reader pauses for.
    pub(in crate::app) fn work_pauses_checksums(&self) -> bool {
        WORK.iter()
            .any(|row| row.pauses_checksums && running(self, row))
    }
    /// The workers the exit sequence waits for, asked to stop in the table's
    /// order, whether or not they are running (stopping cancelled some already);
    /// none is waited for here.
    pub(in crate::app) fn stop_workers(&mut self) -> Vec<task::Stopping> {
        let mut stopping = Vec::new();
        for row in WORK {
            if let Some(stop) = row.stop {
                stopping.extend(stop(self));
            }
        }
        stopping
    }
}

/// The kinds of work quitting stops, in order.
#[cfg(test)]
pub(in crate::app) fn stopped() -> Vec<Kind> {
    WORK.iter()
        .filter(|row| row.stop.is_some())
        .map(|row| row.kind)
        .collect()
}

#[cfg(test)]
#[path = "work_editor_tests.rs"]
mod editor_tests;
