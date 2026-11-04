//! Stack Camera Bursts when adding photos: once the files of a batch added
//! with it are read (`catalog_session::import_stacks`), its bursts become
//! stacks, as Auto-Stack's Group Camera Bursts makes them but leaving alone
//! the photos whose cameras say nothing, and the batch is done: one step for
//! Undo, which leaves it done. It is made only in the Library, with no
//! window open over it and Auto-Stack's closed, from what the session has
//! then; the catalog checks again, in the same transaction, what may still be
//! stacked.
use super::Library;
use super::stacks::StackCommand;
use crate::app::ui::widgets::plural;
use crate::catalog::{BurstCandidate, Photo, PhotoId, SavedSequence, StacksChanged};
use crate::catalog_session::{BurstRead, BurstsRead};
use std::collections::HashMap;

impl Library {
    /// Reads the files of the batches whose bursts are still to be stacked,
    /// unless background reads are suspended or reading already.
    pub(super) fn start_import_stacks(&mut self) {
        self.start_import_stacks_by(crate::exif::sequence);
    }
    /// [`Self::start_import_stacks`], each file's sequence read by `read`.
    pub(super) fn start_import_stacks_by(
        &mut self,
        read: fn(&std::path::Path) -> Option<crate::exif::Sequence>,
    ) {
        if self.reads_suspended {
            return;
        }
        let availability = &self.availability;
        self.session
            .start_import_stacks_by(|path| availability.is_available(path), read);
    }
    /// Keeps what the read found, once it is done, until it can be made.
    pub(super) fn poll_import_stacks(&mut self) {
        if let Some(read) = self.session.poll_import_stacks() {
            self.import_bursts = Some(read);
        }
        // Asked while reading (files came online): read what is still to
        // be done again, a batch made meanwhile being left out by the
        // catalog.
        if self.session.import_stacks_again() {
            self.start_import_stacks();
        }
    }
    /// Stacks the bursts read, when they can be: called only while the
    /// Library is shown and free. Waits while Auto-Stack's window is open or
    /// background reads are suspended; read under another structural
    /// generation, reads again.
    pub(in crate::app) fn make_import_stacks(&mut self) {
        if self.import_bursts.is_none() || self.reads_suspended || self.auto_stack.is_some() {
            return;
        }
        let Some(read) = self.import_bursts.take() else {
            return;
        };
        if read.generation != self.session.generation() {
            self.start_import_stacks();
            return;
        }
        let save: Vec<(PhotoId, SavedSequence)> = read
            .read
            .iter()
            .filter_map(|(id, got)| match got {
                BurstRead::Read {
                    save: Some(saved), ..
                } => Some((*id, saved.clone())),
                _ => None,
            })
            .collect();
        if !save.is_empty()
            && let Err(e) = self.session.save_sequences(&save)
        {
            // Used anyway; read again when Auto-Stack wants them.
            self.messages
                .fail(format!("Camera bursts could not be saved: {e:#}"));
        }
        let (batches, stacks) = plan(&read, self.session.photos());
        if batches.is_empty() {
            return;
        }
        let place_before = self.place();
        let committed = match self.session.stack_imported_bursts(&batches, &stacks) {
            Ok(committed) => committed,
            Err(e) => {
                // Still to be done: made on the next read.
                self.messages
                    .fail(format!("Camera bursts were not stacked: {e:#}"));
                return;
            }
        };
        let listed = self.stacks_changed(committed.listed);
        let Some(StacksChanged { before, after }) = committed.value else {
            return;
        };
        if before == after {
            return;
        }
        let summary = format!(
            "Stacked camera bursts · {}",
            plural(after.stacks.len(), "stack", "stacks")
        );
        if listed {
            self.messages.note(summary.clone());
        }
        self.stack_done.push(StackCommand {
            sequence: crate::edit_session::sequence(),
            before,
            after,
            place_before,
            place_after: self.place(),
            summary,
        });
    }
}

/// The batches of `read` whose photos were all read, and the stacks of their
/// bursts, from `photos` as the session has them: each batch's masters in no
/// stack whose cameras say something, a single shot included, as it ends a
/// camera's run. A batch with a photo not read waits.
fn plan(read: &BurstsRead, photos: &[Photo]) -> (Vec<i64>, Vec<Vec<PhotoId>>) {
    let photos: HashMap<PhotoId, &Photo> = photos
        .iter()
        .filter(|p| p.master.is_none())
        .map(|p| (p.id, p))
        .collect();
    let mut batches = Vec::new();
    let mut stacks = Vec::new();
    for batch in &read.batches {
        let members: Vec<&Photo> = batch
            .photos
            .iter()
            .filter_map(|id| photos.get(id).copied())
            .collect();
        let found: Option<Vec<(&Photo, &BurstRead)>> = members
            .iter()
            .map(|p| Some((*p, read.read.get(&p.id)?)))
            .collect();
        let Some(found) = found else {
            continue;
        };
        if found
            .iter()
            .any(|(_, got)| matches!(got, BurstRead::Unreadable))
        {
            continue;
        }
        let candidates: Vec<BurstCandidate<'_>> = found
            .iter()
            .filter(|(p, _)| p.stack.is_none())
            .filter_map(|(p, got)| {
                let BurstRead::Read {
                    sequence: Some(sequence),
                    captured,
                    ..
                } = got
                else {
                    return None;
                };
                let captured = if p.captured.is_empty() {
                    captured.as_deref()?
                } else {
                    p.captured.as_str()
                };
                Some(BurstCandidate {
                    id: p.id,
                    folder: p.folder,
                    captured,
                    filename: &p.filename,
                    format: &p.format,
                    sequence: Some(sequence),
                })
            })
            .collect();
        stacks.extend(crate::catalog::auto_stacks_with_bursts(
            &candidates,
            batch.seconds,
        ));
        batches.push(batch.batch);
    }
    (batches, stacks)
}
