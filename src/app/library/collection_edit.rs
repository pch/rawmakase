//! Making and changing collections and collection sets in the Library, as
//! Lightroom Classic's Collections panel does: Create Collection… and Create
//! Collection Set…, Rename…, Delete, moving them between sets, and adding
//! photos to a collection by dragging them onto it or with Add to
//! Collection. Each change is one undoable command; the catalog
//! ([`crate::catalog::Catalog::create_collection`] and the rest) owns the
//! rules they follow.
use super::{Library, Place};
use crate::app::ui::design::{self, Tone, button, inset, space, text};
use crate::app::ui::widgets::{self, plural};
use crate::catalog::{CollectionId, CollectionKind, PhotoId, RemovedCollections};
use anyhow::{Context as _, Result};
use eframe::egui;

/// A change to the collections themselves, for the shared undo log.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CollectionTreeCommand {
    /// Orders it among other changes made in the same frame.
    pub sequence: u64,
    pub change: TreeChange,
    pub place_before: Place,
    pub place_after: Place,
    /// What changed, as the status line said it.
    pub summary: String,
}

/// What a [`CollectionTreeCommand`] changed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TreeChange {
    /// Collections or sets made, with the photos they were made with: Undo
    /// deletes them, and `gone` holds them until Redo puts them back.
    Created {
        ids: Vec<CollectionId>,
        gone: Option<RemovedCollections>,
    },
    /// Collections or sets deleted with everything in them: `gone` holds
    /// them until Undo puts them back; Redo deletes them again.
    Deleted {
        ids: Vec<CollectionId>,
        gone: Option<RemovedCollections>,
    },
    Renamed {
        id: CollectionId,
        before: String,
        after: String,
    },
    /// Moved to another set (`None`: the top level).
    Moved {
        id: CollectionId,
        before: Option<CollectionId>,
        after: Option<CollectionId>,
    },
    /// Custom Order's drag: every photo's position `before` and `after`.
    Reordered {
        collection: CollectionId,
        before: Vec<crate::catalog::Member>,
        after: Vec<crate::catalog::Member>,
    },
    /// Save Quick Collection: collection `id` made of the Quick Collection's
    /// photos, which were taken out of it (`cleared`, with their positions)
    /// when Clear Quick Collection After Saving was on.
    SavedQuick {
        id: CollectionId,
        quick: CollectionId,
        gone: Option<RemovedCollections>,
        cleared: Vec<crate::catalog::Member>,
    },
}

impl CollectionTreeCommand {
    /// Takes a photo now removed out of what Undo or Redo would put back:
    /// its id may be given to another photo. The command stays, as it still
    /// changes the collections.
    pub(in crate::app) fn forget_photo(&mut self, id: PhotoId) {
        if let TreeChange::Created { gone, .. }
        | TreeChange::Deleted { gone, .. }
        | TreeChange::SavedQuick { gone, .. } = &mut self.change
            && let Some(gone) = gone
        {
            gone.forget_photo(id);
        }
        match &mut self.change {
            TreeChange::SavedQuick { cleared, .. } => cleared.retain(|m| m.0 != id),
            TreeChange::Reordered { before, after, .. } => {
                before.retain(|m| m.0 != id);
                after.retain(|m| m.0 != id);
            }
            _ => {}
        }
        self.place_before.forget_photo(id);
        self.place_after.forget_photo(id);
    }
}

/// A Create Collection of new virtual copies of `photos`, waiting for the
/// open edit to be saved.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct NewCollection {
    pub name: String,
    pub parent: Option<CollectionId>,
    pub photos: Vec<PhotoId>,
    /// Set as target collection.
    pub target: bool,
}

/// The collection window open, if any.
pub(super) enum CollectionDialog {
    /// Create Collection… (`set` false) or Create Collection Set….
    Create {
        set: bool,
        name: String,
        parent: Option<CollectionId>,
        /// The photos selected when it opened.
        photos: Vec<PhotoId>,
        /// Include selected photos (on by default, as in Lightroom).
        include: bool,
        /// Make new virtual copies: the copies go in, not the photos.
        copies: bool,
        /// Set as target collection.
        target: bool,
        focused: bool,
    },
    /// Save Quick Collection…: a name, and whether to empty it after.
    SaveQuick {
        name: String,
        clear: bool,
        focused: bool,
    },
    Rename {
        id: CollectionId,
        name: String,
        focused: bool,
    },
    /// Asks before deleting a collection or a set with what is in it.
    Delete { id: CollectionId },
}

impl Library {
    /// Whether a collection window is open; it takes the keyboard.
    pub(in crate::app) fn collection_dialog_open(&self) -> bool {
        self.collection_dialog.is_some()
    }
    /// Opens Create Collection… or Create Collection Set… for a new one in
    /// `parent`. A collection is offered the selected photos.
    pub(in crate::app) fn ask_create_collection(
        &mut self,
        set: bool,
        parent: Option<CollectionId>,
    ) {
        let photos = if set { Vec::new() } else { self.selected_ids() };
        self.collection_dialog = Some(CollectionDialog::Create {
            set,
            name: String::new(),
            parent,
            include: !photos.is_empty(),
            photos,
            copies: false,
            target: false,
            focused: false,
        });
    }
    /// Save Quick Collection… (Cmd+Option+B), with Clear Quick Collection
    /// After Saving on, as in Lightroom.
    pub(in crate::app) fn ask_save_quick(&mut self) {
        if self.quick_count() == 0 {
            self.messages.warn("The Quick Collection is empty");
            return;
        }
        self.collection_dialog = Some(CollectionDialog::SaveQuick {
            name: String::new(),
            clear: true,
            focused: false,
        });
    }
    /// Saves the Quick Collection as top-level collection `name`, emptying
    /// it when `clear`: one transaction, one command.
    pub(in crate::app) fn save_quick(&mut self, name: &str, clear: bool) -> Result<CollectionId> {
        let place_before = self.place();
        let (id, quick, cleared) = self.session.save_quick_collection(name, clear)?;
        self.tree_changed();
        let count = self
            .session
            .collection_photos()
            .get(&id)
            .map_or(0, |m| m.len());
        let summary = format!(
            "Saved the Quick Collection as “{}” · {}",
            self.collection_name(id),
            plural(count, "photo", "photos")
        );
        self.record_tree(
            TreeChange::SavedQuick {
                id,
                quick,
                gone: None,
                cleared: cleared.removed,
            },
            place_before,
            summary,
        );
        Ok(id)
    }
    pub(in crate::app) fn ask_rename_collection(&mut self, id: CollectionId) {
        if let Some(c) = self.session.collections().iter().find(|c| c.id == id) {
            self.collection_dialog = Some(CollectionDialog::Rename {
                id,
                name: c.name.clone(),
                focused: false,
            });
        }
    }
    pub(in crate::app) fn ask_delete_collection(&mut self, id: CollectionId) {
        self.collection_dialog = Some(CollectionDialog::Delete { id });
    }

    /// Makes collection or set `name` in `parent`. A collection gets
    /// `photos`, or, with `copies`, a new virtual copy of each (the copies
    /// stay when the collection is undone: making copies isn't undoable).
    /// One command; returns the new collection.
    pub(in crate::app) fn create_collection(
        &mut self,
        set: bool,
        name: &str,
        parent: Option<CollectionId>,
        photos: &[PhotoId],
        copies: bool,
    ) -> Result<CollectionId> {
        let place_before = self.place();
        let kind = if set {
            CollectionKind::Set
        } else {
            CollectionKind::Collection
        };
        // The copies first: the command is the collection's alone.
        let members = if copies && !set {
            self.make_copies(photos)?
        } else {
            photos.to_vec()
        };
        let members = if set { Vec::new() } else { members };
        let id = self
            .session
            .create_collection(kind, name, parent, &members)?;
        let name = self.collection_name(id);
        let summary = match (set, members.len()) {
            (true, _) => format!("Created collection set “{name}”"),
            (false, 0) => format!("Created collection “{name}”"),
            (false, n) => format!(
                "Created collection “{name}” · {}",
                plural(n, "photo", "photos")
            ),
        };
        self.tree_changed();
        self.record_tree(
            TreeChange::Created {
                ids: vec![id],
                gone: None,
            },
            place_before,
            summary,
        );
        Ok(id)
    }
    /// A new virtual copy of each of `photos`, the Library shown as the
    /// catalog now is after each one made. When one fails, says how many were
    /// made before it: those stay.
    fn make_copies(&mut self, photos: &[PhotoId]) -> Result<Vec<PhotoId>> {
        let mut copies = Vec::new();
        let mut failed = None;
        for photo in photos {
            match self.session.create_virtual_copy(*photo, None) {
                Ok(made) => {
                    copies.push(made.value);
                    if let Err(e) = made.listed {
                        failed = Some(e);
                        break;
                    }
                }
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        if !copies.is_empty() {
            self.reloaded();
        }
        match failed {
            None => Ok(copies),
            Some(e) => Err(e.context(format!(
                "Making virtual copies stopped after {}, which stay in no collection",
                plural(copies.len(), "copy", "copies")
            ))),
        }
    }
    pub(in crate::app) fn rename_collection(&mut self, id: CollectionId, name: &str) -> Result<()> {
        let place_before = self.place();
        let before = self.collection_name(id);
        self.session.rename_collection(id, name)?;
        let after = self.collection_name(id);
        if after == before {
            return Ok(());
        }
        self.tree_changed();
        let summary = format!("Renamed “{before}” to “{after}”");
        self.record_tree(
            TreeChange::Renamed { id, before, after },
            place_before,
            summary,
        );
        Ok(())
    }
    /// Moves a collection or set into set `parent`, or to the top (`None`).
    pub(in crate::app) fn move_collection(
        &mut self,
        id: CollectionId,
        parent: Option<CollectionId>,
    ) -> Result<()> {
        let before = self
            .collection(id)
            .context("That collection is gone")?
            .parent;
        if before == parent {
            return Ok(());
        }
        let place_before = self.place();
        self.session.move_collection(id, parent)?;
        self.tree_changed();
        let summary = match parent {
            Some(set) => format!(
                "Moved “{}” into “{}”",
                self.collection_name(id),
                self.collection_name(set)
            ),
            None => format!("Moved “{}” to the top", self.collection_name(id)),
        };
        self.record_tree(
            TreeChange::Moved {
                id,
                before,
                after: parent,
            },
            place_before,
            summary,
        );
        Ok(())
    }
    /// Deletes a collection or a set with everything in it. Photos stay.
    pub(in crate::app) fn delete_collection(&mut self, id: CollectionId) -> Result<()> {
        let place_before = self.place();
        let name = self.collection_name(id);
        let set = self.collection(id).map(|c| c.kind) == Some(CollectionKind::Set);
        let gone = self.session.remove_collections(&[id])?;
        self.tree_changed();
        let summary = if set {
            format!("Deleted collection set “{name}”")
        } else {
            format!("Deleted collection “{name}”")
        };
        self.record_tree(
            TreeChange::Deleted {
                ids: vec![id],
                gone: Some(gone),
            },
            place_before,
            summary,
        );
        Ok(())
    }
    /// Drag and drop or Add to Collection: adds `photos` to collection `id`.
    pub(in crate::app) fn add_to_collection(
        &mut self,
        id: CollectionId,
        photos: Vec<PhotoId>,
    ) -> Result<()> {
        let members = self.session.collection_photos().get(&id);
        let add: Vec<PhotoId> = photos
            .into_iter()
            .filter(|p| members.is_none_or(|m| !m.contains(p)))
            .collect();
        let name = self.collection_name(id);
        if add.is_empty() {
            self.messages.note(format!("Already in “{name}”"));
            return Ok(());
        }
        let summary = format!(
            "Added to “{name}” · {}",
            plural(add.len(), "photo", "photos")
        );
        self.record_collection(id, add, Vec::new(), summary)
    }

    /// Undoes (`undo`) or redoes a [`TreeChange`], keeping in it what the
    /// other direction needs.
    pub(in crate::app) fn apply_tree(&mut self, change: &mut TreeChange, undo: bool) -> Result<()> {
        match (change, undo) {
            (TreeChange::Created { ids, gone }, true)
            | (TreeChange::Deleted { ids, gone }, false) => {
                *gone = Some(self.session.remove_collections(ids)?);
            }
            (TreeChange::Created { gone, .. }, false)
            | (TreeChange::Deleted { gone, .. }, true) => {
                let removed = gone.as_ref().context("Nothing to put back")?;
                self.session.restore_collections(removed)?;
                *gone = None;
            }
            (TreeChange::Renamed { id, before, after }, undo) => {
                self.session
                    .rename_collection(*id, if undo { before } else { after })?;
            }
            (
                TreeChange::Reordered {
                    collection,
                    before,
                    after,
                },
                undo,
            ) => {
                self.session
                    .restore_positions(*collection, if undo { before } else { after })?;
                // The order shown follows.
                self.filter();
            }
            (TreeChange::Moved { id, before, after }, undo) => {
                self.session
                    .move_collection(*id, if undo { *before } else { *after })?;
            }
            (
                TreeChange::SavedQuick {
                    id,
                    quick,
                    gone,
                    cleared,
                },
                true,
            ) => {
                *gone = Some(self.session.unsave_quick_collection(*id, *quick, cleared)?);
            }
            (
                TreeChange::SavedQuick {
                    quick,
                    gone,
                    cleared,
                    ..
                },
                false,
            ) => {
                let removed = gone.as_ref().context("Nothing to put back")?;
                let photos: Vec<PhotoId> = cleared.iter().map(|m| m.0).collect();
                self.session
                    .resave_quick_collection(removed, *quick, &photos)?;
                // Only once it is all saved.
                *gone = None;
            }
        }
        self.tree_changed();
        Ok(())
    }
    /// The collection changes made since the last call, for the undo log.
    pub(in crate::app) fn take_tree_done(&mut self) -> Vec<CollectionTreeCommand> {
        std::mem::take(&mut self.collection_tree_done)
    }

    /// Records Create Smart Collection's new collection `id` as one undoable
    /// command, as [`Self::create_collection`] records its own.
    pub(super) fn record_smart_created(&mut self, id: CollectionId, place_before: Place) {
        self.tree_changed();
        let summary = format!("Created smart collection “{}”", self.collection_name(id));
        self.record_tree(
            TreeChange::Created {
                ids: vec![id],
                gone: None,
            },
            place_before,
            summary,
        );
    }
    fn record_tree(&mut self, change: TreeChange, place_before: Place, summary: String) {
        self.messages.note(summary.clone());
        self.collection_tree_done.push(CollectionTreeCommand {
            sequence: crate::edit_session::sequence(),
            change,
            place_before,
            place_after: self.place(),
            summary,
        });
    }
    /// After collections changed: the panel is built again, and a source
    /// that is gone gives way to All Photographs.
    fn tree_changed(&mut self) {
        self.sidebar_trees = None;
        if let Some(id) = self.filters.collection {
            if self.collection(id).is_some() {
                self.filters.members = self.source_members(id);
            } else {
                self.filters.collection = None;
                self.filters.members.clear();
            }
            self.filter();
        }
    }
    fn collection(&self, id: CollectionId) -> Option<&crate::catalog::Collection> {
        self.session.collections().iter().find(|c| c.id == id)
    }
    fn collection_name(&self, id: CollectionId) -> String {
        self.collection(id)
            .map_or_else(String::new, |c| c.name.clone())
    }
    /// The photos a source shows: a collection's (a smart one's as last
    /// worked out), or for a set the photos of every collection inside it,
    /// smart ones too, each once, as Lightroom Classic shows a selected set.
    pub(super) fn source_members(&self, id: CollectionId) -> std::collections::HashSet<PhotoId> {
        let mut photos = self.members_of_collection(id);
        let mut sets = vec![id];
        let mut seen = std::collections::HashSet::from([id]);
        while let Some(set) = sets.pop() {
            for child in self
                .session
                .collections()
                .iter()
                .filter(|c| c.parent == Some(set) && seen.insert(c.id))
            {
                if child.kind == CollectionKind::Set {
                    sets.push(child.id);
                } else {
                    // Smart collections' photos too, as Lightroom shows them.
                    photos.extend(self.members_of_collection(child.id));
                }
            }
        }
        photos
    }
    /// Whether `id` is a collection set.
    pub(super) fn is_set(&self, id: CollectionId) -> bool {
        self.collection(id).map(|c| c.kind) == Some(CollectionKind::Set)
    }

    /// Draws the open collection window, and does what it was asked to.
    /// A Create Collection that makes new virtual copies isn't made here:
    /// it is returned, for the editor to save the open edit first
    /// ([`crate::app::Editor::create_collection_of_copies`]), as Create
    /// Virtual Copy does.
    pub(in crate::app) fn collection_dialog_window(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<NewCollection> {
        let mut dialog = self.collection_dialog.take()?;
        let palette = crate::app::ui::theme::palette(ctx);
        let mut done = false;
        let mut cancel = false;
        let mut refusal = None;
        let names = self.sibling_names(&dialog);
        let parents = self.set_choices();
        let response = egui::Modal::new(egui::Id::new("collection-dialog"))
            .frame(widgets::modal_frame(&palette).inner_margin(design::margin_same(inset::DIALOG)))
            .show(ctx, |ui| {
                ui.set_width(360.);
                let title = match &dialog {
                    CollectionDialog::Create { set: false, .. } => "Create Collection".into(),
                    CollectionDialog::Create { set: true, .. } => "Create Collection Set".into(),
                    CollectionDialog::SaveQuick { .. } => "Save Quick Collection".into(),
                    CollectionDialog::Rename { .. } => "Rename".into(),
                    CollectionDialog::Delete { id } => self.delete_title(*id),
                };
                ui.label(
                    egui::RichText::new(title)
                        .size(text::TITLE)
                        .color(palette.tone(Tone::STRONG)),
                );
                ui.add_space(space::M);
                let mut ready = true;
                match &mut dialog {
                    CollectionDialog::Create {
                        set,
                        name,
                        parent,
                        photos,
                        include,
                        copies,
                        target,
                        focused,
                    } => {
                        refusal = name_field(ui, name, focused, &names, &mut done);
                        ui.add_space(space::S);
                        ui.horizontal(|ui| {
                            ui.label("Inside a Collection Set");
                            let shown = parent
                                .and_then(|p| parents.iter().find(|(id, _)| *id == p))
                                .map_or("None", |(_, label)| label.as_str());
                            egui::ComboBox::from_id_salt("collection-parent")
                                .selected_text(shown)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(parent, None, "None");
                                    for (id, label) in &parents {
                                        ui.selectable_value(parent, Some(*id), label);
                                    }
                                });
                        });
                        if !*set && !photos.is_empty() {
                            ui.add_space(space::S);
                            ui.checkbox(
                                include,
                                format!(
                                    "Include selected photos ({})",
                                    plural(photos.len(), "photo", "photos")
                                ),
                            );
                            ui.add_enabled_ui(*include, |ui| {
                                ui.checkbox(copies, "Make new virtual copies");
                            });
                        }
                        if !*set {
                            ui.add_space(space::S);
                            ui.checkbox(target, "Set as target collection");
                        }
                        ready = refusal.is_none();
                    }
                    CollectionDialog::SaveQuick {
                        name,
                        clear,
                        focused,
                    } => {
                        refusal = name_field(ui, name, focused, &names, &mut done);
                        ui.add_space(space::S);
                        ui.checkbox(clear, "Clear Quick Collection After Saving");
                        ready = refusal.is_none();
                    }
                    CollectionDialog::Rename { name, focused, .. } => {
                        refusal = name_field(ui, name, focused, &names, &mut done);
                        ready = refusal.is_none();
                    }
                    CollectionDialog::Delete { .. } => {
                        ui.label(
                            egui::RichText::new("Photos stay in the catalog. Undo puts it back.")
                                .size(text::LABEL)
                                .color(palette.tone(Tone::MUTED)),
                        );
                    }
                }
                if let Some(why) = refusal.as_ref().filter(|r| !r.is_empty()) {
                    ui.add_space(space::XS);
                    ui.label(
                        egui::RichText::new(why)
                            .size(text::SMALL)
                            .color(palette.tone(Tone::MUTED)),
                    );
                }
                ui.add_space(space::XL);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = button::DIALOG;
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let label = match &dialog {
                        CollectionDialog::Create { .. } => "Create",
                        CollectionDialog::SaveQuick { .. } => "Save",
                        CollectionDialog::Rename { .. } => "Rename",
                        CollectionDialog::Delete { .. } => "Delete",
                    };
                    done |= ui
                        .add_enabled_ui(ready, |ui| widgets::primary_button(ui, label))
                        .inner
                        .clicked();
                    done &= ready;
                });
            });
        if cancel || response.should_close() {
            return None;
        }
        if !done {
            self.collection_dialog = Some(dialog);
            return None;
        }
        let made = match dialog {
            CollectionDialog::Create {
                set,
                name,
                parent,
                photos,
                include,
                copies,
                target,
                ..
            } => {
                let photos = if include { photos } else { Vec::new() };
                if copies && !set && !photos.is_empty() {
                    return Some(NewCollection {
                        name,
                        parent,
                        photos,
                        target,
                    });
                }
                self.create_collection(set, &name, parent, &photos, false)
                    .map(|id| {
                        if !set {
                            if target {
                                self.set_target(Some(id));
                            }
                            self.select_collection(id);
                        }
                    })
            }
            CollectionDialog::SaveQuick { name, clear, .. } => {
                self.save_quick(&name, clear).map(|_| ())
            }
            CollectionDialog::Rename { id, name, .. } => self.rename_collection(id, &name),
            CollectionDialog::Delete { id } => self.delete_collection(id),
        };
        if let Err(e) = made {
            self.messages
                .fail(format!("Collections not changed: {e:#}"));
        }
        None
    }
    /// Delete's question, naming what goes with a set.
    fn delete_title(&self, id: CollectionId) -> String {
        let name = self.collection_name(id);
        if !self.is_set(id) {
            return format!("Delete the collection “{name}”?");
        }
        let mut inside = 0;
        let mut sets = vec![id];
        while let Some(set) = sets.pop() {
            for c in self
                .session
                .collections()
                .iter()
                .filter(|c| c.parent == Some(set))
            {
                inside += 1;
                if c.kind == CollectionKind::Set {
                    sets.push(c.id);
                }
            }
        }
        match inside {
            0 => format!("Delete the collection set “{name}”?"),
            n => format!(
                "Delete the collection set “{name}” and the {} in it?",
                plural(n, "collection or set", "collections and sets")
            ),
        }
    }
    /// The names a new or renamed collection can't take: its siblings'.
    fn sibling_names(&self, dialog: &CollectionDialog) -> Vec<String> {
        let (parent, except) = match dialog {
            CollectionDialog::Create { parent, .. } => (*parent, None),
            CollectionDialog::SaveQuick { .. } => (None, None),
            CollectionDialog::Rename { id, .. } => {
                (self.collection(*id).and_then(|c| c.parent), Some(*id))
            }
            CollectionDialog::Delete { .. } => return Vec::new(),
        };
        let mut names: Vec<String> = self
            .session
            .collections()
            .iter()
            .filter(|c| c.parent == parent && Some(c.id) != except)
            .map(|c| c.name.clone())
            .collect();
        names.push(crate::catalog::QUICK_COLLECTION.to_string());
        names
    }
    /// Every set, with its path, for Inside a Collection Set.
    fn set_choices(&self) -> Vec<(CollectionId, String)> {
        let collections = self.session.collections();
        let path = |mut id: CollectionId| {
            let mut parts = Vec::new();
            let mut seen = std::collections::HashSet::new();
            while seen.insert(id) {
                let Some(c) = collections.iter().find(|c| c.id == id) else {
                    break;
                };
                parts.push(c.name.clone());
                match c.parent {
                    Some(p) => id = p,
                    None => break,
                }
            }
            parts.reverse();
            parts.join(" › ")
        };
        let mut sets: Vec<(CollectionId, String)> = collections
            .iter()
            .filter(|c| c.kind == CollectionKind::Set)
            .map(|c| (c.id, path(c.id)))
            .collect();
        sets.sort_by_key(|(_, label)| label.to_lowercase());
        sets
    }
}

/// The Name field, focused when the window opens; Enter is `done`.
/// Returns why the name can't be used, as Lightroom says it.
fn name_field(
    ui: &mut egui::Ui,
    name: &mut String,
    focused: &mut bool,
    taken: &[String],
    done: &mut bool,
) -> Option<String> {
    let field = ui.add(
        egui::TextEdit::singleline(name)
            .hint_text("Name")
            .desired_width(f32::INFINITY),
    );
    if !*focused {
        field.request_focus();
        *focused = true;
    }
    let trimmed = name.trim();
    // Empty: nothing to say, but nothing to make either.
    let refusal = if trimmed.is_empty() {
        Some(String::new())
    } else if taken.iter().any(|n| n == trimmed) {
        Some("That name is already in use.".into())
    } else {
        None
    };
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        *done = refusal.is_none();
    }
    refusal
}

impl Library {
    /// The order collection `id` shows in, as Lightroom keeps one per
    /// collection: its stored sort, else Capture Time, ascending (what
    /// Lightroom Classic 15.6 gives a new collection). Custom Order is a
    /// plain collection's alone, as Lightroom offers it. A set shows in the
    /// folders' order. The folders' is kept to come back to.
    pub(super) fn collection_sort(&mut self, id: CollectionId) {
        use super::sort::Sort;
        let kind = self.collection(id).map(|c| c.kind);
        let quick = self.quick() == Some(id);
        let plain = quick || kind == Some(CollectionKind::Collection);
        let own = plain || kind == Some(CollectionKind::Smart);
        if !own {
            if let Some((sort, reverse)) = self.folder_sort.take() {
                self.filters.sort = sort;
                self.filters.reverse = reverse;
            }
            return;
        }
        let stored = self
            .session
            .catalog()
            .collection_options()
            .ok()
            .and_then(|mut all| all.remove(&id));
        let (sort, reverse) = stored
            .and_then(|o| {
                let sort = super::sort::Sort::from_lightroom(&o.sort)?;
                Some((sort, o.direction == "descending"))
            })
            // Custom Order is a plain collection's alone.
            .filter(|(sort, _)| *sort != Sort::Custom || plain)
            .unwrap_or((Sort::CaptureTime, false));
        if self.folder_sort.is_none() {
            self.folder_sort = Some((self.filters.sort, self.filters.reverse));
        }
        self.filters.sort = sort;
        self.filters.reverse = reverse;
    }
    /// Whether collection `id` shows in an order of its own: a collection,
    /// plain or smart (not a set, which shows in the folders' order).
    pub(super) fn has_own_sort(&self, id: CollectionId) -> bool {
        self.quick() == Some(id)
            || matches!(
                self.collection(id).map(|c| c.kind),
                Some(CollectionKind::Collection | CollectionKind::Smart)
            )
    }
    /// Whether the source shown has a custom order: a plain collection.
    pub(super) fn has_custom_order(&self) -> bool {
        self.filters.collection.is_some_and(|id| {
            self.quick() == Some(id)
                || self.collection(id).map(|c| c.kind) == Some(CollectionKind::Collection)
        })
    }
    /// The sort menu chose `sort`: in a collection, kept as its own, as
    /// Lightroom keeps it (`collection_options`), whatever it is.
    pub(super) fn sort_chosen(&mut self) {
        let Some(id) = self.filters.collection else {
            return;
        };
        if self.folder_sort.is_none() {
            return;
        }
        let options = crate::catalog::CollectionOptions {
            sort: self.filters.sort.lightroom().into(),
            direction: super::sort::lightroom_direction(self.filters.reverse).into(),
        };
        if let Err(e) = self.session.set_collection_options(id, &options) {
            self.messages
                .fail(format!("The collection's sort was not kept: {e:#}"));
        }
    }
}

impl Library {
    /// Whether photos dragged in the grid reorder it: Custom Order of a
    /// plain collection.
    pub(super) fn reorders_by_drag(&self) -> bool {
        self.filters.sort == super::sort::Sort::Custom && self.has_custom_order()
    }
    /// Custom Order's drag: puts `moved` where they were dropped, before the
    /// photo shown at `at` in the grid (at the end when `at` is past the
    /// last), as one undoable command. Only the photos shown move among
    /// themselves: photos the filter bar hides keep their places in the
    /// order.
    pub(in crate::app) fn reorder_shown(&mut self, moved: Vec<PhotoId>, at: usize) -> Result<()> {
        if !self.reorders_by_drag() {
            return Ok(());
        }
        let Some(collection) = self.filters.collection else {
            return Ok(());
        };
        let shown: Vec<PhotoId> = self
            .visible
            .iter()
            .map(|i| self.session.photos()[*i].id)
            .collect();
        let at = at.min(shown.len());
        let moved: Vec<PhotoId> = shown
            .iter()
            .copied()
            .filter(|p| moved.contains(p))
            .collect();
        // The shown photos as they will be on screen.
        let mut arranged: Vec<PhotoId> = shown
            .iter()
            .copied()
            .filter(|p| !moved.contains(p))
            .collect();
        let slot = shown[..at].iter().filter(|p| !moved.contains(p)).count();
        arranged.splice(slot..slot, moved.iter().copied());
        // In the order's own direction.
        if self.filters.reverse {
            arranged.reverse();
        }
        // Into the shown photos' places in the whole order.
        let shown: std::collections::HashSet<PhotoId> = shown.into_iter().collect();
        let mut arranged = arranged.into_iter();
        let order: Vec<PhotoId> = self
            .session
            .catalog()
            .collection_order(collection)?
            .into_iter()
            .map(|p| {
                if shown.contains(&p) {
                    arranged.next().unwrap_or(p)
                } else {
                    p
                }
            })
            .collect();
        let place_before = self.place();
        let Some(before_positions) = self.session.arrange_collection(collection, &order)? else {
            return Ok(());
        };
        let after = self.session.catalog().collection_members(collection)?;
        self.filter();
        let summary = format!(
            "Reordered “{}” · {}",
            self.collection_name(collection),
            plural(moved.len(), "photo", "photos")
        );
        self.record_tree(
            TreeChange::Reordered {
                collection,
                before: before_positions,
                after,
            },
            place_before,
            summary,
        );
        Ok(())
    }
}
