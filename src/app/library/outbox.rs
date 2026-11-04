//! What the Library asks the editor to do (a menu's Find on Disk, Remove Folder, a file
//! operation…), queued in the order it was asked and handed over in that order, all in
//! the frame it was asked in.
//!
//! The rules, which [`Outbox`] and [`super::Library::next_request`] keep:
//! - **Order:** requests go to the editor in the order they were made.
//! - **Coalescing:** a request equal to the last one queued is not queued again (one
//!   key pressed several times in a frame); different requests are all handed over.
//! - **Generation:** each request remembers the catalog's generation when it was made.
//!   When the lists have changed by the time it is handed over, the photos it names
//!   are checked again: those no longer listed are left out, and a request left with
//!   none is not handed over. Requests naming folders or roots are handed over as
//!   they are: their handlers check them against the catalog.
//!
//! The editor refuses, with a message, a request that opens a window (Remove Copy,
//! Remove Folder, Read Metadata, a file operation) while another window is open
//! (`Editor::library_requests`).
use super::{CatalogExportRequest, CopyAction, FileRequest, FolderRemoval, PreviewsRequest};
use crate::catalog::PhotoId;
use std::collections::VecDeque;

/// One request from the Library.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Request {
    /// A virtual copy command from a thumbnail menu.
    Copy(CopyAction),
    /// A thumbnail menu's Build or Discard Previews.
    Previews(PreviewsRequest),
    /// Synchronize Folder chosen in the Folders panel: its scope and its name.
    Sync(crate::catalog::folder_sync::SyncScope, String),
    /// Find on Disk… chosen in a photo's menu.
    Find(PhotoId),
    /// A folder chosen in the Folders panel to be removed, for the editor to confirm.
    Removal(FolderRemoval),
    /// Export as Catalog chosen in the Folders panel or a photo's menu.
    Export(CatalogExportRequest),
    /// Read Metadata from Files for these photos, once confirmed.
    Read(Vec<PhotoId>),
    /// A file operation (Delete, the thumbnail menu), for the editor to plan and
    /// confirm.
    File(FileRequest),
}

impl Request {
    /// The request with the photos it names that `listed` still holds; `None` when
    /// it named photos and none is left. Requests naming folders or roots, or no
    /// photo, are kept as they are.
    pub(super) fn revalidated(self, listed: impl Fn(PhotoId) -> bool) -> Option<Self> {
        let keep = |ids: Vec<PhotoId>| {
            let left: Vec<PhotoId> = ids.into_iter().filter(|id| listed(*id)).collect();
            (!left.is_empty()).then_some(left)
        };
        Some(match self {
            Self::Copy(
                CopyAction::Create(id) | CopyAction::SetMaster(id) | CopyAction::Remove(id),
            ) if !listed(id) => return None,
            Self::Find(id) if !listed(id) => return None,
            Self::Read(ids) => Self::Read(keep(ids)?),
            Self::Export(CatalogExportRequest {
                name,
                scope: crate::catalog::ExportScope::Photos(ids),
                removal,
            }) => Self::Export(CatalogExportRequest {
                name,
                scope: crate::catalog::ExportScope::Photos(keep(ids)?),
                removal,
            }),
            Self::Previews(PreviewsRequest::Build(ids, kind)) => {
                Self::Previews(PreviewsRequest::Build(keep(ids)?, kind))
            }
            Self::Previews(PreviewsRequest::Discard(ids, kinds)) => {
                Self::Previews(PreviewsRequest::Discard(keep(ids)?, kinds))
            }
            Self::File(file) => Self::File(match file {
                FileRequest::Remove(ids) => FileRequest::Remove(keep(ids)?),
                FileRequest::Delete(ids) => FileRequest::Delete(keep(ids)?),
                FileRequest::Trash(ids) => FileRequest::Trash(keep(ids)?),
                FileRequest::Rename(ids) => FileRequest::Rename(keep(ids)?),
                FileRequest::ChooseFolder(ids) => FileRequest::ChooseFolder(keep(ids)?),
                FileRequest::Move {
                    photos,
                    root,
                    to,
                    name,
                } => FileRequest::Move {
                    photos: keep(photos)?,
                    root,
                    to,
                    name,
                },
                // Its photos are an offer to move the selection along: a new folder
                // is still asked for without them.
                FileRequest::NewFolder {
                    root,
                    parent,
                    name,
                    photos,
                    inside,
                } => {
                    let photos = keep(photos).unwrap_or_default();
                    FileRequest::NewFolder {
                        root,
                        parent,
                        name,
                        inside: inside && !photos.is_empty(),
                        photos,
                    }
                }
                other => other,
            }),
            other => other,
        })
    }
}

/// The Library's requests not yet handed to the editor, oldest first, each with the
/// catalog's generation when it was made.
#[derive(Default)]
pub(super) struct Outbox(VecDeque<(Request, u64)>);

impl Outbox {
    /// Queues `request`, made at catalog generation `generation`, unless it equals the
    /// last one queued.
    pub(super) fn push(&mut self, request: Request, generation: u64) {
        if self.0.back().is_some_and(|(last, _)| *last == request) {
            return;
        }
        self.0.push_back((request, generation));
    }
    /// The oldest request and the generation it was made at.
    pub(super) fn pop(&mut self) -> Option<(Request, u64)> {
        self.0.pop_front()
    }
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
