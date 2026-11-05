//! Bounded, asynchronous disk-cache work and its UI progress: the thumbnail workers
//! (`thumbnails`), the edited-preview scheduler (`edits`), what an edited preview is
//! rendered from (`source`) and how (`render`), and the progress line (`progress`).
mod edits;
mod progress;
mod render;
mod source;
mod thumbnails;
pub(in crate::app) use edits::Pace;
#[cfg(test)]
pub(in crate::app) use edits::Sent;
pub(in crate::app::library) use edits::{EditJob, EditQueue, EditResult, Wanted, spawn_edited};
pub(in crate::app::library) use progress::Progress;
pub(in crate::app::library) use render::{develop_for_preview, develop_quickly};
pub(in crate::app) use source::{EditSource, PreviewAi};
pub(in crate::app::library) use thumbnails::{PreviewResult, ShownFiles, spawn};

#[cfg(test)]
pub(super) mod tests;
