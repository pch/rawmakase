//! Lightroom's Quick Collection and target collection: B adds the selected
//! photos to the target collection (the Quick Collection unless another is
//! set) or takes them out, Cmd+B shows the Quick Collection, Cmd+Shift+B
//! clears it and Cmd+Option+B saves it as a collection. The Quick
//! Collection is a collection like any other, the one imported from
//! Lightroom or made on first use, and each change is one undoable command.
use super::{Library, Place};
use crate::app::ui::widgets::plural;
use crate::catalog::{Member, MembershipChange, PhotoId};
use anyhow::Result;

/// A change to a collection's photos, for the shared undo log.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CollectionCommand {
    /// Orders it among other changes made in the same frame.
    pub sequence: u64,
    pub collection: crate::catalog::CollectionId,
    /// The photos it added and took out, each with its position, so Undo
    /// and Redo put them back where they were.
    pub added: Vec<Member>,
    pub removed: Vec<Member>,
    /// The other photos the change numbered first (adding after photos
    /// with no position), before and after, so Undo and Redo keep their
    /// order exactly.
    pub renumbered_before: Vec<Member>,
    pub renumbered_after: Vec<Member>,
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
    /// Whether `id` is in the Quick Collection, for the keyboard shortcut pins.
    // The pins don't run on Windows (`shell::workspace`).
    #[cfg(all(test, not(target_os = "windows")))]
    pub(in crate::app) fn in_quick_collection(&self, id: PhotoId) -> bool {
        self.in_quick(id)
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
    /// The collections photo `id` is in, the Quick Collection too, by name:
    /// what its collection badge and Go to Collection list. Smart
    /// collections aren't listed, as in Lightroom.
    pub(super) fn photo_collections(
        &self,
        id: PhotoId,
    ) -> Vec<(crate::catalog::CollectionId, String)> {
        let quick = self.quick();
        let members = self.session.collection_photos();
        let mut found: Vec<(crate::catalog::CollectionId, String)> = self
            .session
            .collections()
            .iter()
            .filter(|c| {
                (c.kind == crate::catalog::CollectionKind::Collection || Some(c.id) == quick)
                    && members.get(&c.id).is_some_and(|m| m.contains(&id))
            })
            .map(|c| {
                let name = if Some(c.id) == quick {
                    "Quick Collection".to_string()
                } else {
                    c.name.clone()
                };
                (c.id, name)
            })
            .collect();
        found.sort_by_key(|(_, name)| name.to_lowercase());
        found
    }
    /// Lightroom's target collection, which B adds to: `None` is the
    /// Quick Collection, also when the one set is gone.
    pub(in crate::app) fn target(&self) -> Option<crate::catalog::CollectionId> {
        self.session.catalog().target_collection().ok().flatten()
    }
    /// Set as Target Collection: `None` makes it the Quick Collection.
    pub(in crate::app) fn set_target(&mut self, id: Option<crate::catalog::CollectionId>) {
        match self.session.set_target_collection(id) {
            Ok(()) => {
                self.sidebar_trees = None;
                let name = id
                    .and_then(|id| self.session.collections().iter().find(|c| c.id == id))
                    .map_or_else(
                        || "The Quick Collection".to_string(),
                        |c| format!("“{}”", c.name),
                    );
                self.messages
                    .note(format!("{name} is the target collection"));
            }
            Err(e) => self
                .messages
                .fail(format!("Target collection not changed: {e:#}")),
        }
    }
    /// B: adds `ids` to the target collection, or takes them out when they
    /// are all in it already, as Lightroom's B does.
    pub(in crate::app) fn toggle_quick(&mut self, ids: &[PhotoId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let (target, name) = match self.target() {
            Some(id) => {
                let name = self
                    .session
                    .collections()
                    .iter()
                    .find(|c| c.id == id)
                    .map_or_else(String::new, |c| format!("“{}”", c.name));
                (id, name)
            }
            None => (self.ensure_quick()?, "Quick Collection".to_string()),
        };
        let members = self.session.collection_photos().get(&target);
        let inside = |id: &PhotoId| members.is_some_and(|m| m.contains(id));
        let all_in = ids.iter().all(inside);
        let (added, removed, verb) = if all_in {
            (Vec::new(), ids.to_vec(), "Removed from")
        } else {
            let added: Vec<PhotoId> = ids.iter().copied().filter(|id| !inside(id)).collect();
            (added, Vec::new(), "Added to")
        };
        let count = added.len() + removed.len();
        let summary = format!("{verb} {name} · {}", plural(count, "photo", "photos"));
        self.record_collection(target, added, removed, summary)
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
    /// The Quick Collection command a key ran (B, Cmd+B and Cmd+Shift+B by
    /// default), for `ids`.
    pub(super) fn quick_key(&mut self, command: QuickKey, ids: Vec<PhotoId>) {
        let done = match command {
            QuickKey::Toggle => self.toggle_quick(&ids),
            QuickKey::Show => self.show_quick(),
            QuickKey::Clear => self.clear_quick(),
            QuickKey::Save => {
                self.ask_save_quick();
                Ok(())
            }
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
        add: Vec<PhotoId>,
        remove: Vec<PhotoId>,
        summary: String,
    ) -> Result<()> {
        let place_before = self.place();
        let change = self.change_collection(collection, &add, &remove)?;
        self.messages.note(summary.clone());
        self.collection_done.push(CollectionCommand {
            sequence: crate::edit_session::sequence(),
            collection,
            added: change.added,
            removed: change.removed,
            renumbered_before: change.renumbered_before,
            renumbered_after: change.renumbered_after,
            place_before,
            place_after: self.place(),
            summary,
        });
        Ok(())
    }
    /// Adds photos after the collection's last and removes others, in the
    /// catalog and as shown, for commands.
    pub(in crate::app) fn change_collection(
        &mut self,
        collection: crate::catalog::CollectionId,
        add: &[PhotoId],
        remove: &[PhotoId],
    ) -> Result<MembershipChange> {
        let change = self.session.change_collection(collection, add, remove)?;
        self.show_members();
        Ok(change)
    }
    /// Puts photos back where they were and removes others, in the catalog
    /// and as shown, for Undo and Redo. A photo removed since (a virtual
    /// copy) stays out.
    pub(in crate::app) fn put_back_members(
        &mut self,
        collection: crate::catalog::CollectionId,
        add: &[Member],
        remove: &[PhotoId],
        positions: &[Member],
    ) -> Result<MembershipChange> {
        let add: Vec<Member> = add
            .iter()
            .filter(|(id, _)| self.photo(*id).is_some())
            .cloned()
            .collect();
        let change = self
            .session
            .put_back_members(collection, &add, remove, positions)?;
        self.show_members();
        Ok(change)
    }
    /// Shows the source's photos as they now are.
    fn show_members(&mut self) {
        // Shown by itself, or in a set being shown.
        if let Some(shown) = self.filters.collection
            && self.source_members(shown) != self.filters.members
        {
            self.filters.members = self.source_members(shown);
            self.filter();
        }
    }
    /// The collection changes made since the last call, for the undo log.
    pub(in crate::app) fn take_collection_done(&mut self) -> Vec<CollectionCommand> {
        std::mem::take(&mut self.collection_done)
    }
}

/// What a Quick Collection key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuickKey {
    /// Adds the photos to the target collection, or takes them out.
    Toggle,
    Show,
    Clear,
    /// Save Quick Collection….
    Save,
}
