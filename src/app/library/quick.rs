//! Lightroom's Quick Collection: B adds the selected photos or takes them
//! out, Cmd+B shows it and Cmd+Shift+B clears it. It is a collection like
//! any other, the one imported from Lightroom or made on first use, and each
//! change is one undoable command.
use super::{Library, Place};
use crate::app::ui::widgets::plural;
use crate::catalog::PhotoId;
use anyhow::Result;

/// A change to a collection's photos, for the shared undo log.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CollectionCommand {
    /// Orders it among other changes made in the same frame.
    pub sequence: u64,
    pub collection: crate::catalog::CollectionId,
    pub added: Vec<PhotoId>,
    pub removed: Vec<PhotoId>,
    pub place_before: Place,
    pub place_after: Place,
    /// What changed, as the status line said it.
    pub summary: String,
}

impl Library {
    /// The Quick Collection, once there is one.
    pub(super) fn quick(&self) -> Option<crate::catalog::CollectionId> {
        self.session.quick_collection()
    }
    /// The Quick Collection, made if there is none yet.
    fn ensure_quick(&mut self) -> Result<crate::catalog::CollectionId> {
        self.session.ensure_quick_collection()
    }
    pub(super) fn in_quick(&self, id: PhotoId) -> bool {
        self.quick()
            .and_then(|q| self.session.collection_photos().get(&q))
            .is_some_and(|members| members.contains(&id))
    }
    /// The Quick Collection's photo count, for the Catalog panel.
    pub(super) fn quick_count(&self) -> usize {
        self.quick()
            .and_then(|q| self.session.collection_photos().get(&q))
            .map_or(0, |members| members.len())
    }
    pub(super) fn showing_quick(&self) -> bool {
        self.quick().is_some() && self.filters.collection == self.quick()
    }
    /// B: adds `ids` to the Quick Collection, or takes them out when they are
    /// all in it already.
    pub(in crate::app) fn toggle_quick(&mut self, ids: &[PhotoId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let quick = self.ensure_quick()?;
        let all_in = ids.iter().all(|id| self.in_quick(*id));
        let (added, removed, verb) = if all_in {
            (Vec::new(), ids.to_vec(), "Removed from")
        } else {
            let added: Vec<PhotoId> = ids
                .iter()
                .copied()
                .filter(|id| !self.in_quick(*id))
                .collect();
            (added, Vec::new(), "Added to")
        };
        let count = added.len() + removed.len();
        let summary = format!(
            "{verb} Quick Collection · {}",
            plural(count, "photo", "photos")
        );
        self.record_collection(quick, added, removed, summary)
    }
    /// Cmd+Shift+B: empties the Quick Collection.
    pub(in crate::app) fn clear_quick(&mut self) -> Result<()> {
        let Some(quick) = self.quick() else {
            return Ok(());
        };
        let removed: Vec<PhotoId> = self
            .session
            .collection_photos()
            .get(&quick)
            .map(|m| m.iter().copied().collect())
            .unwrap_or_default();
        if removed.is_empty() {
            return Ok(());
        }
        self.record_collection(
            quick,
            Vec::new(),
            removed,
            "Cleared Quick Collection".into(),
        )
    }
    /// Cmd+B: shows the Quick Collection.
    pub(in crate::app) fn show_quick(&mut self) -> Result<()> {
        let quick = self.ensure_quick()?;
        self.select_collection(quick);
        Ok(())
    }
    /// B, Cmd+B and Cmd+Shift+B, for `ids`.
    pub(super) fn quick_key(&mut self, modifiers: eframe::egui::Modifiers, ids: Vec<PhotoId>) {
        // Option, and Control on a Mac, make other keys.
        if modifiers.alt || (modifiers.ctrl && (modifiers.mac_cmd || !modifiers.command)) {
            return;
        }
        let done = match (modifiers.command, modifiers.shift) {
            (false, false) => self.toggle_quick(&ids),
            (true, false) => self.show_quick(),
            (true, true) => self.clear_quick(),
            (false, true) => Ok(()),
        };
        if let Err(e) = done {
            self.messages
                .fail(format!("Quick Collection not changed: {e}"));
        }
    }
    /// Makes a change and hands it to the undo log once it is saved.
    pub(super) fn record_collection(
        &mut self,
        collection: crate::catalog::CollectionId,
        added: Vec<PhotoId>,
        removed: Vec<PhotoId>,
        summary: String,
    ) -> Result<()> {
        let place_before = self.place();
        self.change_collection(collection, &added, &removed)?;
        self.messages.note(summary.clone());
        self.collection_done.push(CollectionCommand {
            sequence: crate::edit_session::sequence(),
            collection,
            added,
            removed,
            place_before,
            place_after: self.place(),
            summary,
        });
        Ok(())
    }
    /// Adds and removes photos, in the catalog and as shown; for commands
    /// and their undo.
    pub(in crate::app) fn change_collection(
        &mut self,
        collection: crate::catalog::CollectionId,
        add: &[PhotoId],
        remove: &[PhotoId],
    ) -> Result<()> {
        // A photo removed since (a virtual copy) is left out.
        let add: Vec<PhotoId> = add
            .iter()
            .copied()
            .filter(|id| self.photo(*id).is_some())
            .collect();
        let add = add.as_slice();
        let members = self.session.change_collection(collection, add, remove)?;
        if self.filters.collection == Some(collection) {
            self.filters.members = members.clone();
            self.filter();
        }
        Ok(())
    }
    /// The collection changes made since the last call, for the undo log.
    pub(in crate::app) fn take_collection_done(&mut self) -> Vec<CollectionCommand> {
        std::mem::take(&mut self.collection_done)
    }
}
