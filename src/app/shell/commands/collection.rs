//! Collections by command: making collections and sets, and adding photos
//! to a collection or taking them out. Each change is one undoable step,
//! made through the Library's own methods, as the Collections panel makes it
//! ([`crate::app::library::Library::create_collection`]); the catalog keeps
//! its naming rules. Files on disk are never touched.
use super::{Error, Outcome, Result};
use crate::app::Editor;
use crate::app::library::ListedCollection;
use crate::catalog::{CollectionId, CollectionKind, PhotoId};
use std::collections::HashSet;

impl Editor {
    /// Create Collection… with `photos` in it, or Create Collection Set…
    /// (`set`, which takes no photos), in set `parent` or at the top.
    pub(super) fn command_create_collection(
        &mut self,
        set: bool,
        name: &str,
        parent: Option<CollectionId>,
        photos: Vec<PhotoId>,
    ) -> Result<Outcome> {
        if set && !photos.is_empty() {
            return Err(Error::new(
                "invalid_request",
                "A collection set takes no photos; put them in a collection inside it",
            ));
        }
        let photos = self.collection_change_allowed(photos)?;
        let library = self.library.as_mut().expect("checked above");
        if let Some(parent) = parent {
            match library
                .session
                .collections()
                .iter()
                .find(|c| c.id == parent)
            {
                None => return Err(Error::new("not_found", "No collection set has this ID")),
                Some(c) if c.kind != CollectionKind::Set => {
                    return Err(Error::new(
                        "invalid_request",
                        "Only a collection set can hold collections",
                    ));
                }
                Some(_) => {}
            }
        }
        // The catalog refuses a name in use too; this says so with a code a
        // client can act on, so that it can use the one already there.
        let trimmed = name.trim();
        if let Some(c) = library
            .session
            .collections()
            .iter()
            .find(|c| c.parent == parent && c.name == trimmed)
        {
            return Err(Error::new(
                "already_exists",
                format!("“{trimmed}” is already there, with ID {}", c.id.0),
            ));
        }
        let id = library
            .create_collection(set, name, parent, &photos, false)
            .map_err(|e| Error::new("not_applied", format!("{e:#}")))?;
        Ok(Outcome::CreatedCollection {
            created: listed(library, id)?,
        })
    }
    /// Adds `photos` to collection `id` after its last photo (`add`), or
    /// takes them out of it. Photos already in it, or not in it, are left as
    /// they are.
    pub(super) fn command_change_collection(
        &mut self,
        id: CollectionId,
        photos: Vec<PhotoId>,
        add: bool,
    ) -> Result<Outcome> {
        let photos = self.collection_change_allowed(photos)?;
        let library = self.library.as_mut().expect("checked above");
        let Some(collection) = library.session.collections().iter().find(|c| c.id == id) else {
            return Err(Error::new("not_found", "No collection has this ID"));
        };
        if collection.kind != CollectionKind::Collection || !collection.takes_photos {
            return Err(Error::new(
                "invalid_request",
                match collection.kind {
                    CollectionKind::Set => "A collection set holds collections, not photos",
                    CollectionKind::Smart => {
                        "A smart collection's photos are the ones its rules match"
                    }
                    _ => "This collection can't take photos",
                },
            ));
        }
        let members = library.session.collection_photos().get(&id);
        let changing = photos
            .iter()
            .filter(|p| members.is_some_and(|m| m.contains(p)) != add)
            .count();
        let changed = if add {
            library.add_to_collection(id, photos)
        } else {
            library.take_out_of_collection(id, photos)
        };
        changed.map_err(|e| Error::new("not_applied", format!("{e:#}")))?;
        let (added, removed) = if add { (changing, 0) } else { (0, changing) };
        Ok(Outcome::ChangedCollection {
            collection: listed(library, id)?,
            added,
            removed,
        })
    }
    /// What every change to collections asks first: an open catalog, nothing
    /// busy, and only catalog photos. Returns the photos each once, in the
    /// order given, and ends a gesture in progress so the change is a step
    /// of its own.
    fn collection_change_allowed(&mut self, photos: Vec<PhotoId>) -> Result<Vec<PhotoId>> {
        let Some(library) = self.library.as_ref() else {
            return Err(Error::new("no_catalog", "No catalog is open"));
        };
        let mut seen = HashSet::new();
        let photos: Vec<PhotoId> = photos.into_iter().filter(|p| seen.insert(*p)).collect();
        if let Some(missing) = photos.iter().find(|p| library.photo(**p).is_none()) {
            return Err(Error::new(
                "not_found",
                format!("No catalog photo has ID {}", missing.0),
            ));
        }
        if self.activity.is_busy() || self.command_modal() {
            return Err(Error::new(
                "busy",
                "Close the dialog or wait for the current operation",
            ));
        }
        self.automation.end_turn();
        self.finish_gesture();
        Ok(photos)
    }
}

/// Collection `id` as `collections` lists it.
fn listed(library: &crate::app::library::Library, id: CollectionId) -> Result<ListedCollection> {
    library
        .listed_collections()
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| Error::new("not_found", "The collection is gone"))
}
