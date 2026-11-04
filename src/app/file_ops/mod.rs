//! File operations in the app (#370, planned in #556): the request, keeping
//! the catalog quiet while the operation is planned, confirmed and run, the
//! confirmation, and applying the outcome to the Library, the open photo,
//! the undo log and the caches. `catalog::file_ops` owns everything else
//! (what an operation takes, its refusals, the journal, the disk); nothing
//! here reads files or writes SQL.
//!
//! An operation (Remove from Catalog, moving photos to a folder, making a
//! folder and moving photos into it) goes, in one uninterrupted step on the
//! UI thread: nothing else running (`Activity` idle), the Library's drafts
//! and the open edit saved (a failed save stops here, holding nothing), then
//! `Activity::ChangingFiles`, which autosave, undo and navigation wait for,
//! held until the outcome is applied. Background reads stop, and Read
//! Metadata from Files is cancelled, so no read lands on a photo whose id a
//! removal frees or whose file a move takes. The plan is made on a worker,
//! confirmed in a dialog where Cancel has the focus, run on a worker with
//! its own connection, and what it did is read back from the journal: the
//! same after success, an error, a panic or a lost thread. The open photo
//! follows its file when it moved.
//!
//! The dialogs that come before a plan (Move to Folder…, New Folder
//! Inside…) and the one for what a crash left unfinished, recovered when a
//! catalog opens, are in `file_ops_dialogs.rs`.
use crate::app::shell::messages::Level;
use crate::app::ui::design::{Tone, text};
use crate::app::{Editor, ui::widgets};
#[cfg(test)]
use crate::catalog::file_ops::Summary;
#[cfg(test)]
use crate::catalog::file_ops::TrashReceipt;
use crate::catalog::file_ops::{
    self, BatchId, FileOp, ItemResult, Outcome, Plan, Trash, TrashError, Unfinished,
};
use crate::catalog::{CatalogLocation, PhotoId};
use eframe::egui;
use rawmakase_cancel::Cancel;
#[cfg(test)]
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

mod dialogs;
use dialogs::{Choosing, Naming};
mod rename;
pub(crate) use rename::RenamePlanned;
use rename::{Renaming, RenamingFolder};
mod folders;
use folders::ChoosingParent;

/// A Trash that takes nothing, for tests whose operations have no file to
/// trash.
#[cfg(test)]
pub(in crate::app) struct Unavailable;

#[cfg(test)]
impl Trash for Unavailable {
    fn trash(&self, _: &Path) -> Result<TrashReceipt, TrashError> {
        Err(TrashError::Unsupported)
    }
}

/// What an operation does, for what is said about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum Verb {
    Remove,
    /// Delete: Move to Trash, or Remove from Catalog for photos whose files
    /// can't go; of the rejected photos in the source (Delete Rejected
    /// Photos) when `rejected`.
    Delete {
        rejected: bool,
    },
    /// Move to Trash alone (⌘⌥⇧Delete).
    Trash,
    /// Renames photos; `one`'s old and new names when one is renamed.
    Rename {
        one: Option<(String, String)>,
    },
    /// Moves folder `name` of `root` from logical path `from` to `to` of
    /// `to_root`, into the folder named `into`; a root (`from` "") joins
    /// `to_root`, named `joins`.
    MoveFolder {
        root: crate::catalog::RootId,
        from: String,
        to_root: crate::catalog::RootId,
        to: String,
        name: String,
        into: String,
        joins: Option<String>,
    },
    /// Puts folder `name` of `root`, at logical path `logical`, in the
    /// Trash.
    TrashFolder {
        root: crate::catalog::RootId,
        logical: String,
        name: String,
    },
    /// Renames a folder of `root` from logical path `from` to `to`.
    RenameFolder {
        root: crate::catalog::RootId,
        from: String,
        to: String,
    },
    /// Into the folder named so.
    Move {
        to: String,
    },
    /// Makes folder `name` inside `parent`; `moving` when photos go into it.
    Create {
        name: String,
        parent: String,
        moving: bool,
    },
}

impl Verb {
    /// What the top bar says once Stop is pressed. Photos moved or renamed
    /// within their drive go in batches, which Stop lets finish (#681,
    /// #692); anything else stops after the photo or folder under way.
    fn stopping(&self) -> &'static str {
        match self {
            Self::Rename { .. } | Self::Move { .. } | Self::Create { .. } => {
                "Stopping after the photos under way…"
            }
            Self::Remove
            | Self::Delete { .. }
            | Self::Trash
            | Self::MoveFolder { .. }
            | Self::TrashFolder { .. }
            | Self::RenameFolder { .. } => "Stopping after the photo under way…",
        }
    }

    /// "nothing was removed", for a status line.
    fn nothing(&self) -> &'static str {
        match self {
            Self::Remove | Self::Delete { .. } => "nothing was removed",
            Self::Trash | Self::TrashFolder { .. } => "nothing was moved to the Trash",
            Self::Rename { .. } | Self::RenameFolder { .. } => "nothing was renamed",
            Self::MoveFolder { .. } => "nothing was moved",
            Self::Move { .. } => "nothing was moved",
            Self::Create { .. } => "no folder was made",
        }
    }
    /// "Nothing to remove", for a plan with nothing to do.
    fn nothing_to(&self) -> &'static str {
        match self {
            Self::Remove | Self::Delete { .. } => "Nothing to remove",
            Self::Trash | Self::TrashFolder { .. } => "Nothing to move to the Trash",
            Self::Rename { .. } | Self::RenameFolder { .. } => "Nothing to rename",
            Self::MoveFolder { .. } => "Nothing to move",
            Self::Move { .. } | Self::Create { moving: true, .. } => "Nothing to move",
            Self::Create { .. } => "No folder to make",
        }
    }
    /// "Photos not removed", for a plan that failed.
    fn failed(&self) -> &'static str {
        match self {
            Self::Remove | Self::Delete { .. } | Self::Trash => "Photos not removed",
            Self::Rename { .. } => "Photos not renamed",
            Self::RenameFolder { .. } => "Folder not renamed",
            Self::MoveFolder { .. } => "Folder not moved",
            Self::TrashFolder { .. } => "Folder not moved to the Trash",
            Self::Move { .. } => "Photos not moved",
            Self::Create { .. } => "Folder not made",
        }
    }
}

/// Where the file operation is.
pub(in crate::app) struct FileOps {
    state: State,
    /// The Trash runs put files in.
    trash: Arc<dyn Trash + Send + Sync>,
    /// A catalog just opened, whose unfinished file operations are to be
    /// recovered once nothing else runs.
    pending_recovery: Option<CatalogLocation>,
    /// The open photo, moved before it was fully opened: opened again from
    /// its new file once the catalog is let go.
    reopen: Option<(PhotoId, std::path::PathBuf)>,
    /// Tests stop a run before its worker looks, to see it obeys Stop.
    #[cfg(test)]
    stop_first: bool,
    /// Tests answer the rename dialog's plans themselves, to choose the
    /// frame an answer lands in.
    #[cfg(test)]
    hold_plans: bool,
    /// Tests: Rename Photos' remembered choices, in place of `rename.json`.
    #[cfg(test)]
    rename_settings: crate::export_settings::RenameSettings,
    /// Tests: what reads a rename's files for their names.
    #[cfg(test)]
    name_reader: Option<crate::app::export::naming::read::Reader>,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    /// Move to Folder…: the folder being chosen. Holds nothing.
    Choosing(Box<Choosing>),
    /// New Folder Inside…: the name being given. Holds nothing.
    Naming(Box<Naming>),
    /// Rename Photo(s): the photos' order read on a worker, which `cancel`
    /// stops. Holds the catalog, as the dialog after it does.
    Ordering {
        catalog: CatalogLocation,
        cancel: Arc<AtomicBool>,
    },
    /// Rename Photo or Rename Photos, open; holds the catalog.
    Renaming(Box<Renaming>),
    /// Rename Folder…, open. Holds nothing.
    RenamingFolder(Box<RenamingFolder>),
    /// Move Folder…, while where it goes is chosen. Holds nothing.
    ChoosingParent(Box<ChoosingParent>),
    /// Planned on a worker, for this catalog.
    Planning {
        catalog: CatalogLocation,
        verb: Verb,
    },
    /// Waiting for the user. `generation` is the session's when it was
    /// planned: a catalog read again since may have given its photos' ids
    /// to others, and the plan is not run.
    /// `trash` is Delete's other choice, Move to Trash, when any of its
    /// photos' files can go there.
    Confirming {
        plan: Box<Plan>,
        trash: Option<Box<Plan>>,
        catalog: CatalogLocation,
        generation: u64,
        verb: Verb,
    },
    /// Run on a worker, for `photos`, which may all be gone or moved when
    /// what it did can't be read back; `cancel` stops it between photos.
    Running {
        batch: BatchId,
        catalog: CatalogLocation,
        photos: Vec<PhotoId>,
        cancel: Arc<AtomicBool>,
        verb: Verb,
        progress: Progress,
    },
    /// What a crash left unfinished, decided on a worker as the catalog
    /// opens.
    Recovering { catalog: CatalogLocation },
    /// What recovery couldn't decide, for the user. Holds nothing.
    Unfinished {
        catalog: CatalogLocation,
        items: Vec<Unfinished>,
    },
    /// The user's choice for `operation`, carried out on a worker; `items`
    /// are the rest, shown again after.
    Resolving {
        catalog: CatalogLocation,
        operation: file_ops::OperationId,
        items: Vec<Unfinished>,
    },
}

/// How far a run is, read from its journal now and then.
pub(in crate::app) struct Progress {
    /// Its items, the folder it makes included.
    steps: usize,
    done: usize,
    read: Option<Instant>,
}

impl Default for FileOps {
    fn default() -> Self {
        Self {
            state: State::Idle,
            trash: Arc::new(crate::platform::trash::SystemTrash),
            pending_recovery: None,
            reopen: None,
            #[cfg(test)]
            stop_first: false,
            #[cfg(test)]
            hold_plans: false,
            #[cfg(test)]
            rename_settings: Default::default(),
            #[cfg(test)]
            name_reader: None,
        }
    }
}

impl FileOps {
    /// Asks a running operation to stop after the photo under way (the
    /// top bar's Stop, closing the window). What it did by then stands.
    pub(in crate::app) fn stop(&self) {
        if let State::Running { cancel, .. } = &self.state {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    /// Whether a dialog of a file operation with a field or a list is
    /// open: keys go to it, not to the photos behind.
    pub(in crate::app) fn dialog_open(&self) -> bool {
        matches!(
            self.state,
            State::Choosing(_)
                | State::Naming(_)
                | State::Unfinished { .. }
                | State::Ordering { .. }
                | State::Renaming(_)
                | State::RenamingFolder(_)
                | State::ChoosingParent(_)
        )
    }
    /// Whether a running operation was asked to stop.
    #[cfg(test)]
    pub(in crate::app) fn stopping(&self) -> bool {
        matches!(&self.state, State::Running { cancel, .. } if cancel.load(Ordering::Relaxed))
    }
    /// Whether the confirmation is open.
    #[cfg(test)]
    pub(in crate::app) fn confirming(&self) -> bool {
        matches!(self.state, State::Confirming { .. })
    }
    /// What the open confirmation counts.
    #[cfg(test)]
    pub(in crate::app) fn summary(&self) -> Option<&Summary> {
        match &self.state {
            State::Confirming { plan, .. } => Some(plan.summary()),
            _ => None,
        }
    }
    /// The open confirmation's question, what it says and its action.
    #[cfg(test)]
    pub(in crate::app) fn question(&self) -> Option<(String, Vec<String>, Vec<&'static str>)> {
        match &self.state {
            State::Confirming {
                plan, trash, verb, ..
            } => Some(question(plan, trash.as_deref(), verb)),
            _ => None,
        }
    }
    /// The confirmed plan, as the dialog's action hands it to the worker,
    /// without starting one.
    #[cfg(test)]
    pub(in crate::app) fn take_confirmed_for_tests(&mut self) -> (BatchId, file_ops::Confirmed) {
        let State::Confirming {
            plan,
            catalog,
            verb,
            ..
        } = std::mem::take(&mut self.state)
        else {
            panic!("nothing to confirm");
        };
        let batch = plan.batch().clone();
        self.state = State::Running {
            batch: batch.clone(),
            catalog,
            photos: plan.photos(),
            cancel: Default::default(),
            verb,
            progress: Progress {
                steps: plan.steps(),
                done: 0,
                read: None,
            },
        };
        (batch, plan.confirm())
    }
}

/// The names of `s`'s photos, the first 8 and how many more.
fn names_line(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(8).map(String::as_str).collect();
    let more = names.len().saturating_sub(shown.len());
    if more > 0 {
        format!("{} and {more} more", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

/// The confirmation's question, what it says below it and its actions
/// after Cancel. Delete asks whether to move the files to the Trash or
/// just remove the photos, when `trash` can take any of them.
fn question(
    plan: &Plan,
    trash: Option<&Plan>,
    verb: &Verb,
) -> (String, Vec<String>, Vec<&'static str>) {
    match (verb, trash) {
        (Verb::Delete { rejected }, Some(trash)) => {
            let (s, t) = (plan.summary(), trash.summary());
            let mut lines = Vec::new();
            if s.names.len() > 1 {
                lines.push(names_line(&s.names));
            }
            lines.push(trash_line(t));
            // What Remove takes and Move to Trash leaves: files that can't go.
            let only_removed: Vec<&file_ops::Skipped> = trash
                .skipped()
                .iter()
                .filter(|k| plan.skipped().iter().all(|r| r.photo != k.photo))
                .collect();
            if let Some(first) = only_removed.first() {
                let n = only_removed.len();
                lines.push(format!(
                    "{} can only be removed from the catalog; Move to Trash leaves {}: {}",
                    widgets::plural(n, "photo", "photos"),
                    if n == 1 { "it" } else { "them" },
                    first.reason
                ));
            }
            // Either way, these go with the photos removed.
            copies_line(&mut lines, s);
            edits_line(&mut lines, s);
            shared_line(&mut lines, t);
            if t.other_computers {
                lines.push(OTHER_COMPUTERS_TRASH.into());
            }
            if let Some(first) = plan.skipped().first() {
                let n = plan.skipped().len();
                lines.push(format!(
                    "{} {}: {}",
                    widgets::plural(n, "photo is", "photos are"),
                    if n == 1 {
                        "left as it is"
                    } else {
                        "left as they are"
                    },
                    first.reason
                ));
            }
            let title = match s.names.as_slice() {
                [one] if !rejected => {
                    format!("Move “{one}” to the Trash, or just remove it from the catalog?")
                }
                names => format!(
                    "Move the {} {} to the Trash, or just remove {} from the catalog?",
                    names.len(),
                    match (rejected, names.len()) {
                        (true, 1) => "rejected photo",
                        (true, _) => "rejected photos",
                        (false, 1) => "selected photo",
                        (false, _) => "selected photos",
                    },
                    if names.len() == 1 { "it" } else { "them" }
                ),
            };
            (title, lines, vec!["Remove from Catalog", "Move to Trash"])
        }
        (Verb::Delete { .. }, None) => {
            let (title, lines, action) = question_of(plan, &Verb::Remove);
            (title, lines, vec![action])
        }
        _ => {
            let (title, lines, action) = question_of(plan, verb);
            (title, lines, vec![action])
        }
    }
}

/// `files` files, and `companions` sidecar files when there are any.
fn files_and_sidecars(files: usize, companions: usize) -> String {
    if companions > 0 {
        format!(
            "{} (and {})",
            widgets::plural(files, "file", "files"),
            widgets::plural(companions, "sidecar file", "sidecar files")
        )
    } else {
        widgets::plural(files, "file", "files")
    }
}

/// What a move does on disk: copied to another drive, with the rest (on
/// the destination's drive already) moved by renaming.
fn move_lines(s: &file_ops::Summary) -> Vec<String> {
    let mut lines = Vec::new();
    let (mut files, mut companions) = (s.files, s.companions);
    if let Some(across) = &s.across {
        lines.push(across_line(across));
        files = files.saturating_sub(across.files);
        companions = companions.saturating_sub(across.companions);
    }
    if files > 0 || s.across.is_none() {
        lines.push(format!(
            "{} will be moved on disk.",
            files_and_sidecars(files, companions)
        ));
    }
    lines
}

/// What a move to another drive does: copies the files there, checks
/// them, then puts their originals in the Trash; those on a network share,
/// which has no Trash, stay there.
fn across_line(across: &file_ops::Across) -> String {
    let (files, companions) = (across.files, across.companions);
    let what = files_and_sidecars(files, companions);
    let drive = crate::platform::volume::name_of_mount(&across.volume);
    let (copied, free) = (size(across.bytes), size(across.available));
    if across.originals_stay > 0 && across.originals_stay >= files {
        let stay = match files + companions {
            1 => "Its original stays",
            _ => "Their originals stay",
        };
        return format!(
            "{what} will be copied to “{drive}” ({copied} of {free} free) and checked. {stay} on the network share: it has no Trash."
        );
    }
    let mut line = format!(
        "{what} will be copied to “{drive}” ({copied} of {free} free), checked, and then {} put in the Trash.",
        if files + companions == 1 {
            "its original"
        } else {
            "their originals"
        }
    );
    match across.originals_stay {
        0 => {}
        1 => line.push_str(" The original of 1 file stays on its network share: it has no Trash."),
        n => line.push_str(&format!(
            " The originals of {n} files stay on their network share: it has no Trash."
        )),
    }
    line
}

/// What moving a folder to another drive does: copies all of it there,
/// checks it, then puts the folder in the Trash; one on a network share,
/// which has no Trash, stays there.
fn folder_across_line(across: &file_ops::Across) -> String {
    let drive = crate::platform::volume::name_of_mount(&across.volume);
    let stays = across.originals_stay > 0;
    let then = match stays {
        true => ". The folder stays on the network share: it has no Trash.",
        false => ", and then the folder put in the Trash.",
    };
    if across.files == 0 {
        return format!("Its folders will be made on “{drive}”{then}");
    }
    let checked = match stays {
        true => " and checked",
        false => ", checked",
    };
    format!(
        "Its {} will be copied to “{}” ({} of {} free){checked}{then}",
        widgets::plural(across.files, "file", "files"),
        drive,
        size(across.bytes),
        size(across.available),
    )
}

/// `bytes` as a drive's size or free space is shown: gigabytes, or under
/// one, megabytes.
fn size(bytes: u64) -> String {
    match bytes as f64 {
        b if b >= 1e9 => format!("{:.1} GB", b / 1e9).replace(".0 GB", " GB"),
        b => format!("{:.0} MB", (b / 1e6).max(1.)),
    }
}

/// The originals a move to another drive left beside their copies, as the
/// top bar ends: the first, and why.
fn kept_line(kept: &[file_ops::KeptOriginal]) -> Option<String> {
    let first = kept.first()?;
    let name = first.path.file_name().unwrap_or_default().to_string_lossy();
    Some(format!(
        " · {}: {name}, {}",
        if kept.len() == 1 {
            "1 original stays where it was beside its copy".to_string()
        } else {
            format!(
                "{} originals stay where they were beside their copies",
                kept.len()
            )
        },
        first.why
    ))
}

/// [`question`] for one plan and its one action.
fn question_of(plan: &Plan, verb: &Verb) -> (String, Vec<String>, &'static str) {
    let s = plan.summary();
    let mut lines = Vec::new();
    if s.names.len() > 1 {
        lines.push(names_line(&s.names));
    }
    let left = |lines: &mut Vec<String>, stay: (&str, &str)| {
        if !plan.skipped().is_empty() {
            let n = plan.skipped().len();
            lines.push(format!(
                "{} {}: {}",
                widgets::plural(n, "photo is", "photos are"),
                if n == 1 { stay.0 } else { stay.1 },
                plan.skipped()
                    .first()
                    .map(|s| s.reason.to_string())
                    .unwrap_or_default()
            ));
        }
    };
    let (title, action) = match verb {
        Verb::Remove | Verb::Delete { .. } => {
            lines.push("The files stay on disk.".into());
            if s.copies > 0 {
                lines.push(format!(
                    "{} removed with {}.",
                    widgets::plural(s.copies, "virtual copy is", "virtual copies are"),
                    if s.copies == 1 {
                        "its photo"
                    } else {
                        "their photos"
                    }
                ));
            }
            if s.edited > 0 {
                lines.push(format!(
                    "{} edits in RAWmakase; they are removed too. Adding the file again doesn't bring them back.",
                    widgets::plural(s.edited, "photo has", "photos have")
                ));
            }
            left(&mut lines, ("left as it is", "left as they are"));
            let title = match s.names.as_slice() {
                [one] if s.photos == 1 => format!("Remove “{one}” from the catalog?"),
                _ => format!(
                    "Remove {} from the catalog?",
                    widgets::plural(s.photos, "photo", "photos")
                ),
            };
            (title, "Remove from Catalog")
        }
        Verb::Trash => {
            lines.push(trash_line(s));
            copies_line(&mut lines, s);
            edits_line(&mut lines, s);
            shared_line(&mut lines, s);
            if s.other_computers {
                lines.push(OTHER_COMPUTERS_TRASH.into());
            }
            left(&mut lines, ("left as it is", "left as they are"));
            let title = match s.names.as_slice() {
                [one] => format!("Move “{one}” to the Trash?"),
                names => format!(
                    "Move {} to the Trash?",
                    widgets::plural(names.len(), "photo", "photos")
                ),
            };
            (title, "Move to Trash")
        }
        Verb::TrashFolder { name, .. } => {
            let tree = s.tree.clone().unwrap_or_default();
            lines.extend(trash_folder_lines(s, &tree));
            (trash_folder_title(name, &tree), "Move to Trash")
        }
        Verb::MoveFolder {
            name, into, joins, ..
        } => {
            lines.push(format!(
                "The folder moves on disk with the {} and the folders in it.",
                widgets::plural(s.photos, "photo", "photos")
            ));
            if let Some(joins) = joins {
                lines.push(format!(
                    "“{name}” is no longer a root folder: it becomes a folder of “{joins}”."
                ));
            }
            if let Some(across) = &s.across {
                lines.push(folder_across_line(across));
            }
            if s.other_computers {
                lines.push(
                    "Other computers using this catalog find these photos missing until the folder moves there too."
                        .into(),
                );
            }
            (format!("Move folder “{name}” into “{into}”?"), "Move")
        }
        // Their dialogs ask; this is what they planned.
        Verb::Rename { .. } | Verb::RenameFolder { .. } => {
            for (old, new) in s.renames.iter().take(8) {
                lines.push(format!("{old} → {new}"));
            }
            left(&mut lines, ("left as it is", "left as they are"));
            (
                format!(
                    "Rename {}?",
                    widgets::plural(s.renames.len(), "photo", "photos")
                ),
                "Rename",
            )
        }
        Verb::Move { .. } | Verb::Create { .. } => {
            lines.extend(move_lines(s));
            if s.copies > 0 {
                lines.push(format!(
                    "{} with {}.",
                    widgets::plural(s.copies, "virtual copy goes", "virtual copies go"),
                    if s.copies == 1 {
                        "its photo"
                    } else {
                        "their photos"
                    }
                ));
            }
            shared_line(&mut lines, s);
            if s.other_computers {
                lines.push(
                    "Other computers using this catalog find these photos missing until the files move there too."
                        .into(),
                );
            }
            left(
                &mut lines,
                ("staying where it is", "staying where they are"),
            );
            let photos = match s.names.as_slice() {
                [one] => format!("“{one}”"),
                _ => widgets::plural(s.files, "photo", "photos"),
            };
            match verb {
                Verb::Create { name, parent, .. } => (
                    format!("Make folder “{name}” in “{parent}” and move {photos} into it?"),
                    "Make Folder and Move",
                ),
                Verb::Move { to } => (format!("Move {photos} to “{to}”?"), "Move"),
                Verb::Remove
                | Verb::Delete { .. }
                | Verb::Trash
                | Verb::Rename { .. }
                | Verb::RenameFolder { .. }
                | Verb::MoveFolder { .. }
                | Verb::TrashFolder { .. } => unreachable!(),
            }
        }
    };
    (title, lines, action)
}

/// "Move folder “trip” and its 3 folders, 120 photos and 14 other files
/// to the Trash?", each part left out when there is none.
fn trash_folder_title(name: &str, t: &file_ops::TreeCounts) -> String {
    let parts: Vec<String> = [
        (t.folders, "folder", "folders"),
        (t.photos, "photo", "photos"),
        (t.other_files, "other file", "other files"),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, one, many)| widgets::plural(n, one, many))
    .collect();
    match parts.as_slice() {
        [] => format!("Move empty folder “{name}” to the Trash?"),
        [one] => format!("Move folder “{name}” and its {one} to the Trash?"),
        [rest @ .., last] => format!(
            "Move folder “{name}” and its {} and {last} to the Trash?",
            rest.join(", ")
        ),
    }
}

/// What a folder's question says below its title: its other files by name,
/// the virtual copies and edits that go, and what the catalog has in it
/// that isn't on disk.
fn trash_folder_lines(s: &file_ops::Summary, t: &file_ops::TreeCounts) -> Vec<String> {
    let mut lines = Vec::new();
    if t.other_files > 0 {
        let more = t.other_files - t.other_names.len();
        let names = t.other_names.join(", ");
        lines.push(match more {
            0 => format!("Other files: {names}."),
            n => format!("Other files: {names} and {n} more."),
        });
    }
    // Copies of photos elsewhere are said apart.
    let own = file_ops::Summary {
        copies: s.copies.saturating_sub(t.stray_copies),
        ..s.clone()
    };
    copies_line(&mut lines, &own);
    if t.stray_copies > 0 {
        lines.push(format!(
            "{} in it, of {} elsewhere, {} removed too.",
            widgets::plural(t.stray_copies, "virtual copy", "virtual copies"),
            if t.stray_copies == 1 {
                "a photo"
            } else {
                "photos"
            },
            if t.stray_copies == 1 { "is" } else { "are" }
        ));
    }
    edits_line(&mut lines, s);
    if t.missing_photos > 0 {
        lines.push(format!(
            "{} in the catalog whose {} missing {} removed from the catalog too.",
            widgets::plural(t.missing_photos, "photo", "photos"),
            if t.missing_photos == 1 {
                "file is"
            } else {
                "files are"
            },
            if t.missing_photos == 1 { "is" } else { "are" }
        ));
    }
    if t.missing_folders > 0 {
        lines.push(format!(
            "{} in the catalog that {} missing on disk {} removed from the catalog too.",
            widgets::plural(t.missing_folders, "folder", "folders"),
            if t.missing_folders == 1 { "is" } else { "are" },
            if t.missing_folders == 1 { "is" } else { "are" }
        ));
    }
    if s.other_computers {
        lines.push(OTHER_COMPUTERS_TRASH.into());
    }
    lines
}

/// "2 files (and 1 sidecar file) go to the Trash."
fn trash_line(s: &file_ops::Summary) -> String {
    let files = match (s.files, s.companions) {
        (n, 0) => widgets::plural(n, "file", "files"),
        (n, c) => format!(
            "{} (and {})",
            widgets::plural(n, "file", "files"),
            widgets::plural(c, "sidecar file", "sidecar files")
        ),
    };
    format!(
        "{files} {} to the Trash.",
        if s.files == 1 { "goes" } else { "go" }
    )
}

/// "1 virtual copy is removed with its photo."
fn copies_line(lines: &mut Vec<String>, s: &file_ops::Summary) {
    if s.copies > 0 {
        lines.push(format!(
            "{} removed with {}.",
            widgets::plural(s.copies, "virtual copy is", "virtual copies are"),
            if s.copies == 1 {
                "its photo"
            } else {
                "their photos"
            }
        ));
    }
}

/// That edits made in RAWmakase go with photos removed, and don't come
/// back with a file put back from the Trash.
fn edits_line(lines: &mut Vec<String>, s: &file_ops::Summary) {
    match s.edited {
        0 => {}
        1 => lines.push("1 photo has edits in RAWmakase; they are removed with it. Putting the file back from the Trash brings back the file, not its edits.".into()),
        n => lines.push(format!(
            "{n} photos have edits in RAWmakase; they are removed with them. Putting the files back from the Trash brings back the files, not their edits."
        )),
    }
}

/// What other computers sharing the catalog see of photos trashed here.
const OTHER_COMPUTERS_TRASH: &str =
    "Other computers using this catalog lose these photos too; their copies of the files stay.";

/// "1 sidecar file stays where it is: other files read it too."
fn shared_line(lines: &mut Vec<String>, s: &file_ops::Summary) {
    let n = s.shared_companions.len();
    if n > 0 {
        lines.push(format!(
            "{} where {}: other files read {} too.",
            widgets::plural(n, "sidecar file stays", "sidecar files stay"),
            if n == 1 { "it is" } else { "they are" },
            if n == 1 { "it" } else { "them" }
        ));
    }
}

/// The top bar after a run: "Removed 3 photos from the catalog", with
/// what was left and why.
/// How a finished operation is reported: stopped or left unfinished is a
/// failure, items it left as they were a warning.
fn outcome_level(outcome: &Outcome) -> Level {
    let unfinished =
        |r: &ItemResult| matches!(r, ItemResult::NeedsRecovery(_) | ItemResult::Interrupted);
    let left = |r: &ItemResult| matches!(r, ItemResult::Unchanged(_) | ItemResult::Kept);
    if outcome.stopped.is_some() || outcome.items.iter().any(|i| unfinished(&i.result)) {
        Level::Failure
    } else if outcome.items.iter().any(|i| left(&i.result)) {
        Level::Warning
    } else {
        Level::Done
    }
}

fn summary_of(outcome: &Outcome, verb: &Verb) -> String {
    let count = |f: fn(&ItemResult) -> bool| -> usize {
        outcome.items.iter().filter(|i| f(&i.result)).count()
    };
    let removed: usize = outcome
        .items
        .iter()
        .filter(|i| i.result == ItemResult::Removed)
        .map(|i| i.photos.len())
        .sum();
    let moved = count(|r| matches!(r, ItemResult::Moved { .. }));
    let created = count(|r| matches!(r, ItemResult::Created { .. })) > 0;
    let trashed = count(|r| *r == ItemResult::Trashed);
    let renamed: Vec<&str> = outcome
        .items
        .iter()
        .filter_map(|i| match &i.result {
            ItemResult::Renamed { name } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let folder_renamed = count(|r| matches!(r, ItemResult::FolderRenamed)) > 0;
    let folder_moved = count(|r| matches!(r, ItemResult::FolderMoved)) > 0;
    let last = |path: &str| path.rsplit('/').next().unwrap_or_default().to_string();
    let mut text = match verb {
        Verb::Rename {
            one: Some((old, _)),
        } if renamed.len() == 1 => format!("Renamed {old} to {}", renamed[0]),
        Verb::Rename { .. } => format!(
            "Renamed {}",
            widgets::plural(renamed.len(), "photo", "photos")
        ),
        Verb::RenameFolder { from, .. } if !folder_renamed => {
            format!("Folder {} not renamed", last(from))
        }
        Verb::RenameFolder { from, to, .. } => {
            format!("Renamed folder {} to {}", last(from), last(to))
        }
        Verb::TrashFolder { name, .. } if trashed == 0 => {
            format!("Folder {name} not moved to the Trash")
        }
        Verb::TrashFolder { name, .. } => {
            match outcome.items.iter().map(|i| i.photos.len()).sum() {
                0 => format!("Moved folder {name} to the Trash"),
                n => format!(
                    "Moved folder {name} to the Trash with {}",
                    widgets::plural(n, "photo", "photos")
                ),
            }
        }
        Verb::MoveFolder { name, .. } if !folder_moved => format!("Folder {name} not moved"),
        Verb::MoveFolder { name, into, .. } => format!("Moved folder {name} into {into}"),
        Verb::Trash => format!(
            "Moved {} to the Trash",
            widgets::plural(trashed, "photo", "photos")
        ),
        Verb::Remove | Verb::Delete { .. } => format!(
            "Removed {} from the catalog",
            widgets::plural(removed, "photo", "photos")
        ),
        Verb::Move { to } => format!(
            "Moved {} to {to}",
            widgets::plural(moved, "photo", "photos")
        ),
        Verb::Create { name, .. } if !created => format!("Folder {name} not made"),
        Verb::Create {
            name, moving: true, ..
        } => format!(
            "Made folder {name} and moved {} into it",
            widgets::plural(moved, "photo", "photos")
        ),
        Verb::Create { name, .. } => format!("Made folder {name}"),
    };
    let left: Vec<String> = outcome
        .items
        .iter()
        .filter_map(|i| match &i.result {
            ItemResult::Removed
            | ItemResult::Moved { .. }
            | ItemResult::Trashed
            | ItemResult::Renamed { .. }
            | ItemResult::FolderRenamed
            | ItemResult::FolderMoved
            | ItemResult::Created { .. } => None,
            ItemResult::Unchanged(why) => Some(why.clone().unwrap_or_else(|| "unchanged".into())),
            ItemResult::NeedsRecovery(why) => Some(why.clone()),
            ItemResult::Kept => Some("kept as it is".into()),
            ItemResult::Interrupted => Some("interrupted".into()),
        })
        .collect();
    if !left.is_empty() {
        let what = match verb {
            Verb::Create { .. } if !created => String::new(),
            Verb::RenameFolder { .. } | Verb::MoveFolder { .. } | Verb::TrashFolder { .. } => {
                String::new()
            }
            _ => format!(" · {} left", widgets::plural(left.len(), "photo", "photos")),
        };
        text.push_str(&format!("{what}: {}", left[0]));
        // Nothing is deleted instead: that is the user's to do.
        let no_trash = TrashError::Unsupported.to_string();
        if left.contains(&no_trash) {
            text.push_str(&format!(
                " Delete it in {} if you're sure.",
                crate::platform::reveal::MANAGER
            ));
        }
    }
    let kept: Vec<file_ops::KeptOriginal> = outcome
        .items
        .iter()
        .flat_map(|i| i.originals_kept.iter().cloned())
        .collect();
    if let Some(kept) = kept_line(&kept) {
        text.push_str(&kept);
    }
    // What the Trash also took of a folder, added after it was checked.
    for note in outcome.items.iter().filter_map(|i| i.note.as_deref()) {
        text.push_str(&format!(" · {note}"));
    }
    if let Some(stopped) = &outcome.stopped {
        text.push_str(&format!(" · stopped: {stopped}"));
    }
    text
}

impl Editor {
    /// Remove from Catalog for `photos` (⌥Delete, the thumbnail menu):
    /// saves what is open, keeps the catalog quiet and plans it, then asks.
    /// Nothing is held when the save fails.
    pub(in crate::app) fn request_photo_removal(&mut self, photos: Vec<PhotoId>) {
        if !photos.is_empty() {
            self.request_file_op(FileOp::RemoveFromCatalog { photos }, None, Verb::Remove);
        }
    }

    /// Delete for `photos`: asks whether to move their files to the Trash
    /// or just remove them from the catalog, both planned at once; the
    /// rejected photos of the source when `rejected` (Delete Rejected
    /// Photos).
    pub(in crate::app) fn request_photo_delete(&mut self, photos: Vec<PhotoId>, rejected: bool) {
        if !photos.is_empty() {
            let trash = FileOp::TrashPhotos {
                photos: photos.clone(),
            };
            self.request_file_op(
                FileOp::RemoveFromCatalog { photos },
                Some(trash),
                Verb::Delete { rejected },
            );
        }
    }

    /// Move to Trash… for the folder of `root` at logical path `logical`
    /// (a folder's menu in the Folders panel): saves what is open, keeps
    /// the catalog quiet and plans it, then asks.
    pub(in crate::app) fn request_folder_trash(
        &mut self,
        root: crate::catalog::RootId,
        logical: String,
    ) {
        let name = logical.rsplit('/').next().unwrap_or_default().to_string();
        let op = FileOp::TrashFolder {
            root,
            folder: logical.clone(),
        };
        self.request_file_op(
            op,
            None,
            Verb::TrashFolder {
                root,
                logical,
                name,
            },
        );
    }

    /// Move to Trash for `photos` (⌘⌥⇧Delete).
    pub(in crate::app) fn request_photo_trash(&mut self, photos: Vec<PhotoId>) {
        if !photos.is_empty() {
            self.request_file_op(FileOp::TrashPhotos { photos }, None, Verb::Trash);
        }
    }

    /// Delete Rejected Photos (⌘Delete, the Library menu): Delete for the
    /// rejected photos of the source shown, whatever is selected or
    /// filtered.
    pub(in crate::app) fn request_rejected_delete(&mut self) {
        let Some(library) = &self.library else {
            return;
        };
        let rejected = library.rejected_in_source();
        if rejected.is_empty() {
            self.messages.warn("No rejected photos here");
        } else {
            self.request_photo_delete(rejected, true);
        }
    }

    /// Moves `photos` to catalog folder `to`, named `name` (dropped on it,
    /// or chosen in Move to Folder…), once confirmed.
    pub(in crate::app) fn request_photo_move(
        &mut self,
        photos: Vec<PhotoId>,
        root: crate::catalog::RootId,
        to: String,
        name: String,
    ) {
        if !photos.is_empty() {
            self.request_file_op(
                FileOp::MovePhotos { photos, root, to },
                None,
                Verb::Move { to: name },
            );
        }
    }

    /// Plans `op`, and `trash` beside it for Delete: saves what is open,
    /// keeps the catalog quiet and plans them on a worker, then asks.
    /// Nothing is held when the save fails.
    fn request_file_op(&mut self, op: FileOp, trash: Option<FileOp>, verb: Verb) {
        // A dialog that led here holds nothing.
        if matches!(
            self.file_ops.state,
            State::Choosing(_)
                | State::Naming(_)
                | State::RenamingFolder(_)
                | State::ChoosingParent(_)
        ) {
            self.file_ops.state = State::Idle;
        }
        if self.activity.is_busy() || !matches!(self.file_ops.state, State::Idle) {
            self.messages.warn("Finish what is running first");
            return;
        }
        let Some(location) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone())
        else {
            return;
        };
        // Saved first, with nothing processed in between: a failed save
        // stops here.
        if !self.flush() {
            self.messages.fail(format!(
                "The open photo's edit could not be saved; {}",
                verb.nothing()
            ));
            return;
        }
        if !self.activity.begin_file_change() {
            return;
        }
        if let Some(library) = &mut self.library {
            library.suspend_background_reads();
        }
        self.file_ops.state = State::Planning {
            catalog: location.clone(),
            verb,
        };
        let ctx = self.context.clone();
        let system_trash = self.file_ops.trash.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            ctx,
            move |tx| {
                let planned = crate::catalog::Catalog::open(&location)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|catalog| {
                        // The Trash runs put files in, which a folder's
                        // Trash asks whether its drive has one.
                        let plan = file_ops::plan_with_trash(&catalog, op, &*system_trash)
                            .map_err(|e| e.to_string())?;
                        let trash = trash
                            .map(|op| file_ops::plan(&catalog, op).map_err(|e| e.to_string()))
                            .transpose()?;
                        Ok((plan, trash))
                    });
                let _ = tx.send(crate::app::worker::Event::FileOpPlanned(Box::new(planned)));
            },
            |tx, message| {
                let _ = tx.send(crate::app::worker::Event::FileOpPlanned(Box::new(Err(
                    message,
                ))));
            },
        );
    }

    /// Delete and Backspace in Develop, as in the Library: for the
    /// filmstrip's selection when the open photo is in it, else the open
    /// photo. Not while a tool is open, as tools take Delete for their own
    /// spots, masks and guides, nor behind a menu or a dialog.
    pub(in crate::app) fn develop_delete_key(&mut self, ctx: &egui::Context) {
        if !self.view.is(crate::app::develop::state::Tool::None)
            || egui::Popup::is_any_open(ctx)
            || self.command_modal()
        {
            return;
        }
        // Whether it was pressed, and with what (see `Library::delete_press`).
        let pressed = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key: egui::Key::Delete | egui::Key::Backspace,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => Some(*modifiers),
                _ => None,
            })
        });
        let (Some(modifiers), Some(open), Some(library)) =
            (pressed, self.document.catalog_photo, self.library.as_mut())
        else {
            return;
        };
        let selected = library.selected_photos();
        let ids = if selected.contains(&open) {
            selected
        } else {
            vec![open]
        };
        library.delete_press(modifiers, ids);
    }

    /// The plan came back: it is asked about, or nothing is to be done. A
    /// folder made with nothing moving into it was asked for already, by
    /// New Folder Inside…'s Create.
    pub(in crate::app) fn file_op_planned(
        &mut self,
        planned: Result<(Plan, Option<Plan>), String>,
    ) {
        let State::Planning { catalog, verb } = std::mem::take(&mut self.file_ops.state) else {
            return;
        };
        if !self.showing_catalog(&catalog) {
            self.release_files();
            return;
        }
        match planned {
            Ok((plan, _)) if plan.is_empty() => {
                let text: String = match plan.skipped().first() {
                    Some(s) => format!("{}: {}", verb.nothing_to(), s.reason),
                    None => verb.nothing_to().into(),
                };
                self.messages.warn(text);
                self.release_files();
            }
            Ok((plan, trash)) => {
                let generation = self.library.as_ref().map_or(0, |l| l.session.generation());
                // Asked already: by Create, or by Rename Folder…'s Rename.
                let asked = matches!(
                    verb,
                    Verb::Create { moving: false, .. } | Verb::RenameFolder { .. }
                );
                self.file_ops.state = State::Confirming {
                    plan: Box::new(plan),
                    // None of the files can go: Remove from Catalog alone.
                    trash: trash.filter(|t| !t.is_empty()).map(Box::new),
                    catalog,
                    generation,
                    verb,
                };
                if asked {
                    self.confirm_file_op();
                }
            }
            Err(e) => {
                self.messages.fail(format!("{}: {e}", verb.failed()));
                self.release_files();
            }
        }
    }

    /// The file operations' dialogs, while one is open.
    pub(in crate::app) fn file_op_window(&mut self, ctx: &egui::Context) {
        match &self.file_ops.state {
            State::Choosing(_) => return self.choose_folder_window(ctx),
            State::Naming(_) => return self.new_folder_window(ctx),
            State::Unfinished { .. } => return self.unfinished_window(ctx),
            State::Ordering { .. } => return self.ordering_window(ctx),
            State::Renaming(_) => return self.rename_window(ctx),
            State::RenamingFolder(_) => return self.rename_folder_window(ctx),
            State::ChoosingParent(_) => return self.choose_parent_window(ctx),
            _ => {}
        }
        let State::Confirming {
            plan, trash, verb, ..
        } = &self.file_ops.state
        else {
            return;
        };
        let (title, lines, actions) = question(plan, trash.as_deref(), verb);
        let id = match verb {
            Verb::Remove => "remove-photos",
            Verb::Delete { .. } => "delete-photos",
            Verb::Trash => "trash-photos",
            Verb::Move { .. } | Verb::Create { .. } => "move-photos",
            Verb::Rename { .. } | Verb::RenameFolder { .. } => "rename",
            Verb::MoveFolder { .. } => "move-folder",
            Verb::TrashFolder { .. } => "trash-folder",
        };
        // Delete's second action is Move to Trash.
        let trashing = trash.is_some();
        match widgets::destructive_choice(ctx, id, &title, &lines, "Cancel", &actions) {
            None => {}
            Some(Some(1)) if trashing => self.confirm_file_op_with(true),
            Some(Some(_)) => self.confirm_file_op_with(false),
            Some(None) => {
                self.file_ops.state = State::Idle;
                self.release_files();
            }
        }
    }

    /// The confirmation's action: runs the plan on a worker.
    pub(in crate::app) fn confirm_file_op(&mut self) {
        self.confirm_file_op_with(false);
    }

    /// The confirmation's action, Delete's Move to Trash when `trash`:
    /// runs that plan on a worker.
    fn confirm_file_op_with(&mut self, trash: bool) {
        let State::Confirming {
            plan,
            trash: trashing,
            catalog,
            generation,
            verb,
        } = std::mem::take(&mut self.file_ops.state)
        else {
            return;
        };
        let (plan, verb) = match (verb, trashing) {
            (Verb::Delete { .. }, Some(plan)) if trash => (plan, Verb::Trash),
            (Verb::Delete { .. }, _) => (plan, Verb::Remove),
            (verb, _) => (plan, verb),
        };
        // Still the catalog it was planned for, not read again since.
        let current = self.library.as_ref().map(|l| l.session.generation());
        if !self.showing_catalog(&catalog) || current != Some(generation) {
            self.messages.warn(format!(
                "The catalog changed; {}. Try again.",
                verb.nothing()
            ));
            self.release_files();
            return;
        }
        let batch = plan.batch().clone();
        #[cfg(test)]
        let cancel = Arc::new(AtomicBool::new(self.file_ops.stop_first));
        #[cfg(not(test))]
        let cancel: Arc<AtomicBool> = Default::default();
        self.file_ops.state = State::Running {
            batch: batch.clone(),
            catalog: catalog.clone(),
            photos: plan.photos(),
            cancel: cancel.clone(),
            verb,
            progress: Progress {
                steps: plan.steps(),
                done: 0,
                read: None,
            },
        };
        let confirmed = plan.confirm();
        let failed_batch = batch.clone();
        let system_trash = self.file_ops.trash.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let done = crate::catalog::Catalog::open(&catalog)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|mut catalog| {
                        file_ops::execute(
                            &mut catalog,
                            confirmed,
                            &*system_trash,
                            Cancel::new(&cancel),
                        )
                        .map_err(|e| e.to_string())
                    });
                let _ = tx.send(crate::app::worker::Event::FileOpDone(batch, Box::new(done)));
            },
            move |tx, message| {
                let _ = tx.send(crate::app::worker::Event::FileOpDone(
                    failed_batch,
                    Box::new(Err(message)),
                ));
            },
        );
    }

    /// The run finished, failed or its thread stopped: what it did is read
    /// from the journal and applied, and the catalog is let go.
    pub(in crate::app) fn file_op_done(&mut self, batch: BatchId, done: Result<Outcome, String>) {
        let State::Running {
            batch: running,
            catalog,
            photos,
            cancel,
            verb,
            progress,
        } = std::mem::take(&mut self.file_ops.state)
        else {
            return;
        };
        if running != batch {
            self.file_ops.state = State::Running {
                batch: running,
                catalog,
                photos,
                cancel,
                verb,
                progress,
            };
            return;
        }
        if !self.showing_catalog(&catalog) {
            self.release_files();
            return;
        }
        let read = self
            .library
            .as_ref()
            .map(|l| file_ops::outcome(l.session.catalog(), &batch));
        let outcome = match (read, done) {
            (Some(Ok(mut outcome)), done) => {
                outcome.stopped = match done {
                    Ok(done) => done.stopped,
                    Err(e) => Some(e),
                };
                outcome
            }
            (_, Ok(outcome)) => outcome,
            // Neither the journal nor the worker says what was done: any of
            // the photos may be gone or moved, so all are forgotten as if
            // they were, and the catalog's own lists say where they are.
            (_, Err(e)) => {
                self.messages.fail(format!(
                    "The file operation stopped ({e}); the catalog was read again"
                ));
                self.forget_removed_photos(&photos);
                self.reread_after_files(&photos, &[]);
                self.release_files();
                return;
            }
        };
        let summary = summary_of(&outcome, &verb);
        match outcome_level(&outcome) {
            Level::Failure => self.messages.fail(summary),
            Level::Warning => self.messages.warn(summary),
            _ => self.messages.done(summary),
        }
        // The tree's selection and unfolded folders follow a folder renamed
        // or moved.
        let followed = match &verb {
            Verb::RenameFolder { root, from, to } => Some((*root, from, *root, to)),
            Verb::MoveFolder {
                root,
                from,
                to_root,
                to,
                ..
            } => Some((*root, from, *to_root, to)),
            _ => None,
        };
        if let Some((root, from, to_root, to)) = followed
            && outcome.items.iter().any(|i| {
                matches!(
                    i.result,
                    ItemResult::FolderRenamed | ItemResult::FolderMoved
                )
            })
            && let Some(library) = &mut self.library
        {
            library.folder_renamed(root, from, to_root, to);
        }
        // A selection in a folder put in the Trash goes to its parent.
        let trashed_folder = match &verb {
            Verb::TrashFolder { root, logical, .. }
                if outcome
                    .items
                    .iter()
                    .any(|i| i.result == ItemResult::Trashed) =>
            {
                Some((*root, logical.clone()))
            }
            _ => None,
        };
        if let (Some((root, logical)), Some(library)) = (&trashed_folder, &mut self.library) {
            library.folder_trashed(*root, logical);
        }
        // After the summary: a catalog that can't be read again says so.
        self.apply_file_outcome(&outcome);
        self.release_files();
    }

    /// Brings the app in step with what file operations did: removed
    /// photos forgotten, moved ones followed (the open photo, preview
    /// requests), the catalog read again for those and for folders made.
    fn apply_file_outcome(&mut self, outcome: &Outcome) {
        let ids = |f: fn(&ItemResult) -> bool| -> Vec<PhotoId> {
            outcome
                .items
                .iter()
                .filter(|i| f(&i.result))
                .flat_map(|i| i.photos.iter().copied())
                .collect()
        };
        let removed = ids(|r| matches!(r, ItemResult::Removed | ItemResult::Trashed));
        // A folder renamed, moved or put in the Trash (by a run, recovery
        // or the user's choice) is read again, photos in it or not.
        let folder_changed = outcome.items.iter().any(|i| {
            matches!(
                i.result,
                ItemResult::FolderRenamed
                    | ItemResult::FolderMoved
                    | ItemResult::Removed
                    | ItemResult::Trashed
            )
        });
        // Each a photo whose file is at another path now.
        let moved = ids(|r| {
            matches!(
                r,
                ItemResult::Moved { .. }
                    | ItemResult::Renamed { .. }
                    | ItemResult::FolderRenamed
                    | ItemResult::FolderMoved
            )
        });
        let created = outcome
            .items
            .iter()
            .any(|i| matches!(i.result, ItemResult::Created { .. }));
        self.forget_removed_photos(&removed);
        if created && let Some(library) = &mut self.library {
            library.unfold_new_folder();
        }
        if !removed.is_empty() || !moved.is_empty() || created || folder_changed {
            self.reread_after_files(&removed, &moved);
        }
    }

    /// Forgets photos removed from the catalog, whose ids new photos may
    /// be given: their previews, undo steps and selection, the reference,
    /// and the open photo, which nothing may save into now.
    fn forget_removed_photos(&mut self, removed: &[PhotoId]) {
        for &id in removed {
            self.forget_preview_intent(id);
            self.undo_log.forget_photo(id);
        }
        if self.reference.photo.is_some_and(|id| removed.contains(&id)) {
            self.clear_reference();
        }
        if self
            .document
            .catalog_photo
            .is_some_and(|id| removed.contains(&id))
        {
            self.close_removed_document();
        }
    }

    /// Reads the catalog again after photos were `removed` or `moved`
    /// (their cached thumbnails are dropped), moves preview requests with
    /// moved photos, and has the open photo follow its file.
    fn reread_after_files(&mut self, removed: &[PhotoId], moved: &[PhotoId]) {
        let Some(library) = &mut self.library else {
            return;
        };
        let changed: Vec<PhotoId> = removed.iter().chain(moved).copied().collect();
        if let Err(e) = library.reread_after(&changed) {
            self.messages.fail(format!(
                "Done, but the catalog could not be read again: {e:#}"
            ));
            return;
        }
        // Each moved photo, its copies included, at its file now.
        let files: Vec<(PhotoId, std::path::PathBuf)> = library
            .session
            .photos()
            .iter()
            .filter(|p| moved.contains(&p.id) || p.master.is_some_and(|m| moved.contains(&m)))
            .map(|p| (p.id, p.path.clone()))
            .collect();
        let catalog = library.session.catalog().location().clone();
        // The open photo's file is where the catalog says now: its edit
        // saves there.
        let open = self
            .document
            .catalog_photo
            .and_then(|id| library.photo(id))
            .map(|p| p.path.clone())
            .filter(|path| self.document.path.as_ref() != Some(path));
        self.move_preview_intents(&catalog, &files);
        if let Some(path) = open {
            // A load that may still read the old path (under way, or the
            // full-size decode after the half-size one shown) or failed
            // there, leaving no full-size image, is dropped, and the photo
            // opened again where it is now. Its edit was saved before the
            // move, so nothing is lost.
            let unfinished =
                self.document.loading() || self.document.full().is_none_or(|image| image.fast);
            if unfinished && let Some(id) = self.document.catalog_photo {
                self.document.invalidate_load();
                self.file_ops.reopen = Some((id, path.clone()));
            }
            self.document.relink(path);
        }
    }

    /// The top bar's part while an operation runs: what it does, and Stop.
    pub(in crate::app) fn file_op_progress(&mut self, ui: &mut egui::Ui) {
        let text = match &mut self.file_ops.state {
            State::Recovering { .. } | State::Resolving { .. } => {
                "Checking unfinished file operations…".to_string()
            }
            State::Running {
                batch,
                photos,
                cancel,
                verb,
                progress,
                ..
            } => {
                if cancel.load(Ordering::Relaxed) {
                    verb.stopping().to_string()
                } else {
                    // How far it is, read from the journal now and then.
                    let due = progress
                        .read
                        .is_none_or(|at| at.elapsed() >= Duration::from_millis(250));
                    if due && let Some(library) = &self.library {
                        progress.read = Some(Instant::now());
                        if let Ok(outcome) = file_ops::outcome(library.session.catalog(), batch) {
                            progress.done = outcome
                                .items
                                .iter()
                                .filter(|i| i.result != ItemResult::Interrupted)
                                .count();
                        }
                    }
                    match verb {
                        Verb::Remove | Verb::Delete { .. } => format!(
                            "Removing {} from the catalog",
                            widgets::plural(photos.len(), "photo", "photos")
                        ),
                        Verb::Trash => {
                            let at = (progress.done + 1).min(progress.steps);
                            format!("Moving {at} of {} to the Trash…", progress.steps)
                        }
                        Verb::Rename { .. } => {
                            let at = (progress.done + 1).min(progress.steps);
                            format!("Renaming {at} of {}…", progress.steps)
                        }
                        Verb::RenameFolder { .. } => "Renaming the folder…".to_string(),
                        Verb::MoveFolder { name, .. } => format!("Moving folder {name}…"),
                        Verb::TrashFolder { name, .. } => {
                            format!("Moving folder {name} to the Trash…")
                        }
                        Verb::Move { to } => {
                            let at = (progress.done + 1).min(progress.steps);
                            format!("Moving {at} of {} to {to}…", progress.steps)
                        }
                        Verb::Create { name, .. } if progress.done == 0 => {
                            format!("Making folder {name}…")
                        }
                        Verb::Create { name, .. } => format!(
                            "Moving {} of {} into {name}…",
                            progress.done,
                            progress.steps.saturating_sub(1)
                        ),
                    }
                }
            }
            _ => return,
        };
        let palette = crate::app::ui::theme::palette(ui.ctx());
        let stoppable = matches!(
            &self.file_ops.state,
            State::Running { cancel, .. } if !cancel.load(Ordering::Relaxed)
        );
        ui.label(
            egui::RichText::new(text)
                .size(text::LABEL)
                .color(palette.tone(Tone::SOFT)),
        );
        if stoppable
            && ui
                .small_button("Stop")
                .on_hover_text("Stop after the photo under way")
                .clicked()
        {
            self.file_ops.stop();
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }

    /// Whether the Library still shows catalog `location`.
    fn showing_catalog(&self, location: &CatalogLocation) -> bool {
        self.library
            .as_ref()
            .is_some_and(|l| l.session.catalog().location() == location)
    }

    /// Lets the catalog go: what waited may write again, and the
    /// background reads go on.
    fn release_files(&mut self) {
        self.activity.finish_file_change();
        if let Some(library) = &mut self.library {
            library.resume_background_reads();
        }
        if let Some((id, path)) = self.file_ops.reopen.take()
            && self.document.catalog_photo == Some(id)
        {
            // What the operation did stays on the top bar.
            let line = self.messages.saved_line();
            self.load_raw(path, Some(id));
            self.messages.restore_line(line);
        }
    }
}

#[cfg(test)]
mod move_folder_tests;
#[cfg(test)]
mod move_tests;
#[cfg(test)]
mod rename_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod trash_folder_tests;
#[cfg(test)]
mod trash_tests;
