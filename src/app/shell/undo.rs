//! One undo sequence across Library and Develop, as Lightroom has: Cmd+Z
//! reverses the latest command wherever it was made, after returning to the
//! place it was made in. The History panel keeps its own steps and imported
//! Lightroom history; this log only decides what Cmd+Z and Cmd+Shift+Z do.
//! It lives in memory and is cleared when another catalog opens.
use crate::app::Editor;
use crate::app::Module;
use crate::app::history::{Recorded, Step};
use crate::app::library::{
    CollectionCommand, DescriptiveCommand, Library, MetadataCommand, Place, StackCommand,
};
use crate::catalog::PhotoId;
use anyhow::Result;
use std::collections::VecDeque;

/// Commands kept, as many as History keeps steps.
const LIMIT: usize = 100;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Command {
    /// Rating, flag or label of one or more photos. `develop` is the photo
    /// open in Develop when it was made there; it is undone there.
    Metadata {
        change: Box<MetadataCommand>,
        develop: Option<PhotoId>,
    },
    /// Photos added to or taken out of a collection, e.g. the Quick
    /// Collection; undone in the Library.
    Collection(Box<CollectionCommand>),
    /// Title, caption, creator, copyright, location or keywords of one or
    /// more photos; undone in the Library.
    Descriptive(Box<DescriptiveCommand>),
    /// A Develop step or History click on `photo` (`None`: a file outside
    /// the catalog), in the History identified by `history`, made on the file
    /// `source` (`None`: it could not be read when the photo opened).
    Develop {
        photo: Option<PhotoId>,
        history: u64,
        source: Option<crate::storage::Identity>,
        change: Box<Recorded>,
    },
    /// Settings synchronized to several photos at once; undone and redone together.
    Sync(Box<crate::app::library::sync::SyncCommand>),
    /// Photos grouped into, taken out of or moved in stacks; undone in the
    /// Library.
    Stack(Box<StackCommand>),
}

impl Command {
    /// Takes photo `id` out of the command; false when nothing of it is left.
    fn forget_photo(&mut self, id: PhotoId) -> bool {
        match self {
            Command::Metadata { change, develop } => {
                // Made in Develop on that photo, which is gone: undone in the Library.
                if *develop == Some(id) {
                    *develop = None;
                }
                change.before.retain(|m| m.0 != id);
                change.after.retain(|m| m.0 != id);
                change.place_before.forget_photo(id);
                change.place_after.forget_photo(id);
                !change.before.is_empty()
            }
            Command::Collection(change) => {
                change.added.retain(|p| *p != id);
                change.removed.retain(|p| *p != id);
                change.place_before.forget_photo(id);
                change.place_after.forget_photo(id);
                !(change.added.is_empty() && change.removed.is_empty())
            }
            Command::Descriptive(change) => {
                change.before.retain(|s| s.photo != id);
                change.after.retain(|s| s.photo != id);
                change.ratings_before.retain(|m| m.0 != id);
                change.ratings_after.retain(|m| m.0 != id);
                change.place_before.forget_photo(id);
                change.place_after.forget_photo(id);
                !change.before.is_empty()
            }
            Command::Develop { photo, .. } => *photo != Some(id),
            Command::Stack(change) => change.forget_photo(id),
            Command::Sync(sync) => {
                sync.edits.retain(|e| e.id != id);
                !sync.edits.is_empty()
            }
        }
    }
}

#[derive(Default)]
pub(in crate::app) struct UndoLog {
    undo: VecDeque<Command>,
    redo: Vec<Command>,
}
impl UndoLog {
    pub(in crate::app) fn push(&mut self, command: Command) {
        if self.undo.len() == LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(command);
        self.redo.clear();
    }
    /// Takes a photo now removed out of every command, Undo's and Redo's alike: the
    /// catalog can give its id to the next photo added, which no command made
    /// before may then write. A command with nothing else left is dropped; one that
    /// also changed other photos keeps undoing and redoing those.
    pub(in crate::app) fn forget_photo(&mut self, id: PhotoId) {
        self.undo.retain_mut(|c| c.forget_photo(id));
        self.redo.retain_mut(|c| c.forget_photo(id));
    }
    pub(in crate::app) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
    pub(in crate::app) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub(in crate::app) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    #[cfg(test)]
    pub(in crate::app) fn len(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Undo,
    Redo,
}

impl Editor {
    /// Moves the changes Library and Develop made into the log, in the order
    /// they were made.
    pub(in crate::app) fn sync_undo(&mut self) {
        let photo = self.document.catalog_photo;
        let history = self.document.edit.history().id();
        let mut commands: Vec<(u64, Command)> = self
            .document
            .edit
            .take_recorded()
            .into_iter()
            .map(|change| {
                (
                    change.sequence,
                    Command::Develop {
                        photo,
                        history,
                        source: self.document.file.clone(),
                        change: Box::new(change),
                    },
                )
            })
            .collect();
        if let Some(library) = &mut self.library {
            let develop = if self.module == Module::Library {
                None
            } else {
                photo
            };
            commands.extend(library.take_done().into_iter().map(|change| {
                (
                    change.sequence,
                    Command::Metadata {
                        change: Box::new(change),
                        develop,
                    },
                )
            }));
            commands.extend(
                library
                    .take_collection_done()
                    .into_iter()
                    .map(|change| (change.sequence, Command::Collection(Box::new(change)))),
            );
            commands.extend(
                library
                    .take_stack_done()
                    .into_iter()
                    .map(|change| (change.sequence, Command::Stack(Box::new(change)))),
            );
            commands.extend(
                library
                    .take_descriptive_done()
                    .into_iter()
                    .map(|change| (change.sequence, Command::Descriptive(Box::new(change)))),
            );
        }
        commands.sort_by_key(|(sequence, _)| *sequence);
        for (_, command) in commands {
            self.undo_log.push(command);
        }
    }
    /// Records a drag still in progress as a step, in the log, so it is undone in
    /// its place; the rest of the drag becomes a step of its own.
    pub(in crate::app) fn finish_gesture(&mut self) {
        self.document.edit.finish_gesture();
        self.sync_undo();
    }
    /// [`Self::finish_gesture`] for a gesture whose state was just saved.
    pub(in crate::app) fn finish_saved_gesture(&mut self) {
        self.document.edit.finish_saved_gesture();
        self.sync_undo();
    }
    /// Waits while Sync writes edits, which Undo could otherwise race.
    pub(in crate::app) fn undo(&mut self) {
        self.command_history(false);
    }
    pub(in crate::app) fn redo(&mut self) {
        self.command_history(true);
    }
    /// Shared by UI shortcuts and external commands; false preserves a failed step.
    pub(in crate::app) fn command_history(&mut self, redo: bool) -> bool {
        if self.activity.holds_catalog_writes() {
            return false;
        }
        let applied = self.step(if redo {
            Direction::Redo
        } else {
            Direction::Undo
        });
        self.sync_command_revision();
        self.load_reference();
        applied
    }
    /// Reverses the latest command, or makes the latest reversed one again.
    /// What the Library panels hold is saved first, so it is a command
    /// before the one to reverse is picked.
    fn step(&mut self, direction: Direction) -> bool {
        if !self.commit_library_drafts() {
            return false;
        }
        // A drag still held is the latest change, so it is what Undo takes back. A
        // mask drag ends there, so a Redo in the same frame cannot revive it.
        self.finish_gesture();
        self.view.masking.end_drag();
        let log = &mut self.undo_log;
        let command = match direction {
            Direction::Undo => log.undo.pop_back(),
            Direction::Redo => log.redo.pop(),
        };
        let Some(command) = command else {
            return false;
        };
        let applied = self.apply(&command, direction);
        let log = &mut self.undo_log;
        match (direction, applied) {
            (Direction::Undo, true) | (Direction::Redo, false) => log.redo.push(command),
            (Direction::Undo, false) | (Direction::Redo, true) => log.undo.push_back(command),
        }
        applied
    }
    /// Returns to where `command` was made and sets its state from before
    /// (undo) or after (redo). False when nothing could be written, so the
    /// command stays where it was.
    fn apply(&mut self, command: &Command, direction: Direction) -> bool {
        let verb = match direction {
            Direction::Undo => "Undo",
            Direction::Redo => "Redo",
        };
        match command {
            Command::Metadata { change, develop } => {
                let (values, place) = match direction {
                    Direction::Undo => (&change.before, &change.place_before),
                    Direction::Redo => (&change.after, &change.place_after),
                };
                self.apply_library(verb, &change.summary, place, *develop, |library| {
                    library.set_metadata(values)
                })
            }
            Command::Collection(change) => {
                let (add, remove, place) = match direction {
                    Direction::Undo => (&change.removed, &change.added, &change.place_before),
                    Direction::Redo => (&change.added, &change.removed, &change.place_after),
                };
                self.apply_library(verb, &change.summary, place, None, |library| {
                    library.change_collection(change.collection, add, remove)
                })
            }
            Command::Descriptive(change) => {
                let (values, ratings, place) = match direction {
                    Direction::Undo => {
                        (&change.before, &change.ratings_before, &change.place_before)
                    }
                    Direction::Redo => (&change.after, &change.ratings_after, &change.place_after),
                };
                self.apply_library(verb, &change.summary, place, None, |library| {
                    library.restore_descriptive(values, ratings)
                })
            }
            Command::Stack(change) => {
                let (state, place) = match direction {
                    Direction::Undo => (&change.before, &change.place_before),
                    Direction::Redo => (&change.after, &change.place_after),
                };
                self.apply_library(verb, &change.summary, place, None, |library| {
                    library.restore_stacks(state)
                })
            }
            Command::Sync(sync) => {
                // The open photo is saved first, so a failure changes nothing.
                if !self.flush() {
                    return false;
                }
                let Some(library) = &mut self.library else {
                    return false;
                };
                let side = match direction {
                    Direction::Undo => crate::app::library::sync::SyncSide::Before,
                    Direction::Redo => crate::app::library::sync::SyncSide::After,
                };
                // Each photo where it is now, after any relink since the Sync,
                // and, offline, as it would open: from its editable preview,
                // whose recorded identity stands in for the file.
                let mut located: std::collections::HashMap<
                    PhotoId,
                    crate::app::library::sync::Located,
                > = sync
                    .edits
                    .iter()
                    .filter_map(|e| {
                        let photo = library.photo(e.id)?;
                        let at = crate::app::library::sync::Located {
                            path: photo.path.clone(),
                            stand_in: library.stand_in(photo),
                        };
                        Some((e.id, at))
                    })
                    .collect();
                let locate = |id: PhotoId| located.remove(&id);
                match crate::app::library::sync::restore(
                    &mut library.session,
                    &sync.edits,
                    side,
                    locate,
                ) {
                    Ok(()) => {}
                    // Nothing to return to: the command is used up, not retried.
                    Err(e @ crate::app::library::sync::SyncRestoreError::PhotoRemoved) => {
                        self.messages.fail(format!("{verb} Sync Settings: {e}"));
                        return true;
                    }
                    Err(e) => {
                        self.messages.fail(format!("{verb} failed: {e}"));
                        return false;
                    }
                }
                if let Some(library) = &mut self.library {
                    library.edits_changed(sync.edits.iter().map(|e| e.id));
                }
                let changed: Vec<_> = sync.edits.iter().map(|e| e.id).collect();
                self.refresh_previews(&changed);
                // Written back, or with no edit at all.
                self.reopen_synced(&changed);
                self.messages.note(format!(
                    "{verb} {} ({})",
                    sync.change.name(),
                    crate::app::ui::widgets::plural(sync.edits.len(), "photo", "photos")
                ));
                true
            }
            Command::Develop {
                photo,
                history,
                source,
                change,
            } => {
                let (target, at) = match direction {
                    Direction::Undo => (&change.before, change.at_before),
                    Direction::Redo => (&change.after, change.at_after),
                };
                // The History that recorded it goes back to the exact state;
                // the same photo opened again since gets the recipe as a step.
                let same_history = self.document.edit.history().id() == *history;
                // A file replaced since the change was made gets none of it.
                let reopened = photo.is_some()
                    && self.document.catalog_photo == *photo
                    && self.document.path.is_some()
                    && self.document.file == *source;
                if same_history || reopened {
                    self.module = Module::Develop;
                    let step = Step::new(verb, "");
                    if same_history {
                        self.document.edit.restore(at, target, step);
                    } else {
                        self.document.edit.set(target, step);
                    }
                    // Not a change of its own for the log.
                    self.document.edit.take_recorded();
                    self.document.edit.save_state_mut().mark_changed();
                    self.ensure_analysis();
                    self.schedule();
                    self.messages.note(format!("{verb} in Develop"));
                    return true;
                }
                // The photo was closed since: save the state and open it.
                let Some(id) = *photo else {
                    self.messages
                        .warn(format!("{verb}: that photo is no longer open"));
                    return true;
                };
                // The open photo is saved first, so a failure leaves both
                // photos and the command as they were.
                if !self.flush() {
                    return false;
                }
                let Some(library) = &mut self.library else {
                    return false;
                };
                let Some(path) = library.photo(id).map(|p| p.path.clone()) else {
                    self.messages
                        .warn(format!("{verb}: that photo is no longer in the catalog"));
                    return true;
                };
                // Offline, as it would open: from its editable preview, whose
                // recorded identity stands in for the file.
                let recorded = library.recorded_original(id);
                let original = crate::edits::Original {
                    path: &path,
                    recorded: recorded.as_ref(),
                };
                let saved = library
                    .session
                    .catalog()
                    .load_edit(id, original)
                    .and_then(|edit| {
                        // A saved edit's identity, which a verified move of its
                        // file refreshes, protects the file; without one, the
                        // file the change was made on does.
                        let source = match &edit {
                            Some(_) => original.identity()?,
                            None => source.clone().ok_or_else(|| {
                                anyhow::anyhow!("its file could not be read when it was edited")
                            })?,
                        };
                        let export = edit.map(|e| e.export).unwrap_or_default();
                        library.session.save_edit(
                            id,
                            original,
                            &source,
                            target,
                            &export,
                            crate::catalog::HistoryUpdate::Keep,
                        )
                    });
                if let Err(e) = saved {
                    self.messages.fail(format!("{verb} failed: {e}"));
                    return false;
                }
                self.develop_catalog_photo(id);
                true
            }
        }
    }
    /// Writes a Library command with `write` and returns to where it was
    /// made: `place` in the Library, or `develop`, the photo open in Develop.
    /// False when nothing could be written.
    fn apply_library(
        &mut self,
        verb: &str,
        summary: &str,
        place: &Place,
        develop: Option<PhotoId>,
        write: impl FnOnce(&mut Library) -> Result<()>,
    ) -> bool {
        // Leaving the open photo saves it first; if that fails, the command
        // stays and the save error stays on the top bar.
        if !self.flush() {
            return false;
        }
        let Some(library) = &mut self.library else {
            return false;
        };
        if let Err(e) = write(library) {
            self.messages.fail(format!("{verb} failed: {e}"));
            return false;
        }
        match develop {
            Some(photo) => self.show_in_develop(photo),
            None => {
                self.module = Module::Library;
                if let Some(library) = &mut self.library {
                    library.go_to_place(place);
                }
            }
        }
        self.messages.note(format!("{verb} {summary}"));
        true
    }
    /// Shows `photo` in Develop, keeping its edit when it is already open.
    fn show_in_develop(&mut self, photo: PhotoId) {
        if self.document.catalog_photo == Some(photo) && self.document.path.is_some() {
            self.module = Module::Develop;
            if let Some(library) = &mut self.library {
                library.make_active(photo);
            }
        } else {
            self.develop_catalog_photo(photo);
        }
    }
}

#[cfg(test)]
#[path = "undo_editor_tests.rs"]
mod editor_tests;
