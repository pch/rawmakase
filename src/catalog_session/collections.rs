//! Collections, collection sets and their photos: changes to the catalog
//! that the session's `collections` and `collection_photos` follow.
use super::CatalogSession;
use crate::catalog::{
    Collection, CollectionId, CollectionKind, Member, MembershipChange, PhotoId, RemovedCollections,
};
use anyhow::Result;

impl CatalogSession {
    /// Adds `add` to `collection` after its last photo and takes `remove`
    /// out of it, in the catalog and the lists. Returns what changed, with
    /// positions.
    pub(crate) fn change_collection(
        &mut self,
        collection: CollectionId,
        add: &[PhotoId],
        remove: &[PhotoId],
    ) -> Result<MembershipChange> {
        let change = self.catalog.change_collection(collection, add, remove)?;
        self.follow_members(collection, &change);
        Ok(change)
    }
    /// Puts `add` back at their positions and takes `remove` out, for Undo
    /// and Redo, in the catalog and the lists.
    pub(crate) fn put_back_members(
        &mut self,
        collection: CollectionId,
        add: &[Member],
        remove: &[PhotoId],
    ) -> Result<MembershipChange> {
        let change = self.catalog.put_back_members(collection, add, remove)?;
        self.follow_members(collection, &change);
        Ok(change)
    }
    /// Create Smart Collection: `name` in set `parent` (or at the top) with
    /// `rules`, in the catalog and the lists. Returns its id.
    pub(crate) fn create_smart_collection(
        &mut self,
        name: &str,
        parent: Option<CollectionId>,
        rules: &crate::catalog::SmartRules,
    ) -> Result<CollectionId> {
        self.changed();
        let id = self.catalog.create_smart_collection(name, parent, rules)?;
        self.collections = self.catalog.collections()?;
        self.smart_rules.insert(id, rules.clone());
        Ok(id)
    }
    /// Gives smart collection `id` `rules`, in the catalog and the lists.
    pub(crate) fn set_smart_rules(
        &mut self,
        id: CollectionId,
        rules: &crate::catalog::SmartRules,
    ) -> Result<()> {
        self.changed();
        self.catalog.set_smart_rules(id, rules)?;
        self.smart_rules.insert(id, rules.clone());
        Ok(())
    }
    /// Takes smart collection `id`'s rules away, as an older release's import
    /// left it: Undo of the first rules saved for it.
    pub(crate) fn clear_smart_rules(&mut self, id: CollectionId) -> Result<()> {
        self.changed();
        self.catalog.clear_smart_rules(id)?;
        self.smart_rules.remove(&id);
        Ok(())
    }
    /// Smart collection `id`'s rules as stored.
    pub(crate) fn smart_rules_of(&self, id: CollectionId) -> Option<&crate::catalog::SmartRules> {
        self.smart_rules.get(&id)
    }
    /// Makes a collection of `photos`, or a collection set (`photos` must be
    /// empty), in the catalog and the lists: all of it, or nothing.
    pub(crate) fn create_collection(
        &mut self,
        kind: CollectionKind,
        name: &str,
        parent: Option<CollectionId>,
        photos: &[PhotoId],
    ) -> Result<CollectionId> {
        anyhow::ensure!(
            kind == CollectionKind::Collection || photos.is_empty(),
            "A collection set can't take photos"
        );
        let id = if kind == CollectionKind::Collection {
            let (id, change) = self.catalog.create_collection_of(name, parent, photos)?;
            self.follow_members(id, &change);
            id
        } else {
            let id = self.catalog.create_collection(kind, name, parent)?;
            self.changed();
            id
        };
        self.list_collection(Collection {
            id,
            name: name.trim().to_string(),
            parent,
            kind,
        });
        Ok(id)
    }
    /// Renames a collection or set, in the catalog and the lists.
    pub(crate) fn rename_collection(&mut self, id: CollectionId, name: &str) -> Result<()> {
        self.catalog.rename_collection(id, name)?;
        // What smart collections see (the Collection rule's names) changed.
        self.changed();
        if let Some(i) = self.collections.iter().position(|c| c.id == id) {
            let mut collection = self.collections.remove(i);
            collection.name = name.trim().to_string();
            self.list_collection(collection);
        }
        Ok(())
    }
    /// Moves a collection or set to the top or into a set, in the catalog
    /// and the lists.
    pub(crate) fn move_collection(
        &mut self,
        id: CollectionId,
        parent: Option<CollectionId>,
    ) -> Result<()> {
        self.catalog.move_collection(id, parent)?;
        // What smart collections see (the Collection rule's names) changed.
        self.changed();
        if let Some(c) = self.collections.iter_mut().find(|c| c.id == id) {
            c.parent = parent;
        }
        Ok(())
    }
    /// Deletes collections and sets with everything inside them, in the
    /// catalog and the lists.
    pub(crate) fn remove_collections(
        &mut self,
        ids: &[CollectionId],
    ) -> Result<RemovedCollections> {
        let removed = self.catalog.remove_collections(ids)?;
        // What smart collections see (the Collection rule's names) changed.
        self.changed();
        let gone: std::collections::HashSet<CollectionId> = removed.ids().collect();
        self.collections.retain(|c| !gone.contains(&c.id));
        self.smart_rules.retain(|id, _| !gone.contains(id));
        self.collection_photos.retain(|id, _| !gone.contains(id));
        Ok(removed)
    }
    /// Puts back what [`Self::remove_collections`] deleted, in the catalog
    /// and the lists; photos no longer in the catalog stay out.
    pub(crate) fn restore_collections(&mut self, removed: &RemovedCollections) -> Result<()> {
        self.catalog.restore_collections(removed)?;
        // What smart collections see (the Collection rule's names) changed.
        self.changed();
        for collection in removed.collections() {
            self.list_collection(collection);
        }
        // The rules come back with their smart collections, whatever the
        // lists were read again meanwhile.
        for (id, rules) in removed.smart_rules() {
            self.smart_rules.insert(id, rules.clone());
        }
        let listed: std::collections::HashSet<PhotoId> = self.photos.iter().map(|p| p.id).collect();
        for (collection, photo) in removed.members() {
            let members = self.collection_photos.entry(collection).or_default();
            if listed.contains(&photo) {
                members.insert(photo);
            }
        }
        Ok(())
    }
    /// Adds `collection` to the list where the catalog lists it, by name and id.
    fn list_collection(&mut self, collection: Collection) {
        let at = self
            .collections
            .partition_point(|c| (&c.name, c.id) <= (&collection.name, collection.id));
        self.collections.insert(at, collection);
    }
    /// Makes the lists follow a change the catalog made: removals first, as
    /// the catalog makes them.
    fn follow_members(&mut self, collection: CollectionId, change: &MembershipChange) {
        self.changed();
        let members = self.collection_photos.entry(collection).or_default();
        for (id, _) in &change.removed {
            members.remove(id);
        }
        members.extend(change.added.iter().map(|m| m.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, CatalogLocation, Position};
    use crate::catalog_session::Opened;

    /// A photo both taken out and put back in one change stays in, in the
    /// lists as in the catalog.
    #[test]
    fn the_lists_follow_a_change_that_removes_and_adds_one_photo() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        image::RgbImage::new(8, 8).save(folder.join("a.jpg"))?;
        let path = directory.path().join("library.rawmakase");
        let mut catalog = Catalog::create(&path)?;
        catalog.add_folder(&folder)?;
        drop(catalog);
        let Opened { mut session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
        let a = session.photos()[0].id;
        let quick = session.ensure_quick_collection()?;
        session.change_collection(quick, &[a], &[])?;
        session.put_back_members(quick, &[(a, Some(Position::new("4")))], &[a])?;
        session.change_collection(quick, &[a], &[a])?;
        assert!(session.collection_photos()[&quick].contains(&a));
        assert_eq!(
            session.catalog().collection_members(quick)?,
            [(a, Some(Position::new("1")))]
        );
        Ok(())
    }

    fn session() -> Result<(tempfile::TempDir, CatalogSession, Vec<PhotoId>)> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        for name in ["a.jpg", "b.jpg", "c.jpg"] {
            image::RgbImage::new(8, 8).save(folder.join(name))?;
        }
        let path = directory.path().join("library.rawmakase");
        let mut catalog = Catalog::create(&path)?;
        catalog.add_folder(&folder)?;
        drop(catalog);
        let Opened { session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
        // By id, the order members are listed in.
        let mut ids: Vec<PhotoId> = session.photos().iter().map(|p| p.id).collect();
        ids.sort();
        Ok((directory, session, ids))
    }

    /// What the session lists, as the catalog read again would list it.
    fn assert_in_step(session: &CatalogSession) -> Result<()> {
        let read: Vec<(CollectionId, String, Option<CollectionId>, CollectionKind)> = session
            .catalog
            .collections()?
            .into_iter()
            .map(|c| (c.id, c.name, c.parent, c.kind))
            .collect();
        let listed: Vec<_> = session
            .collections()
            .iter()
            .map(|c| (c.id, c.name.clone(), c.parent, c.kind))
            .collect();
        assert_eq!(listed, read);
        let mut members = session.catalog.collection_photos()?;
        members.retain(|_, m| !m.is_empty());
        let mut listed = session.collection_photos().clone();
        listed.retain(|_, m| !m.is_empty());
        assert_eq!(listed, members);
        Ok(())
    }

    #[test]
    fn collection_changes_keep_the_lists_in_step_with_the_catalog() -> Result<()> {
        let (_directory, mut session, ids) = session()?;
        let trips = session.create_collection(CollectionKind::Set, "Trips", None, &[])?;
        let japan =
            session.create_collection(CollectionKind::Collection, " Japan ", Some(trips), &[])?;
        let best = session.create_collection(CollectionKind::Collection, "Best", None, &[])?;
        assert_in_step(&session)?;
        let change = session.change_collection(japan, &ids[..2], &[])?;
        assert_eq!(change.added.len(), 2);
        session.change_collection(best, &ids[2..], &[])?;
        session.rename_collection(best, "Zebras")?;
        session.move_collection(japan, None)?;
        assert_in_step(&session)?;

        let removed = session.change_collection(japan, &[], &[ids[0]])?;
        assert_eq!(removed.removed, [(ids[0], Some(Position::new("1")))]);
        session.put_back_members(japan, &removed.removed, &[])?;
        assert_eq!(
            session.catalog.collection_members(japan)?,
            [
                (ids[0], Some(Position::new("1"))),
                (ids[1], Some(Position::new("2")))
            ]
        );

        session.move_collection(japan, Some(trips))?;
        let deleted = session.remove_collections(&[trips])?;
        assert_in_step(&session)?;
        assert!(!session.collections().iter().any(|c| c.id == japan));
        session.restore_collections(&deleted)?;
        assert_in_step(&session)?;
        assert_eq!(session.collection_photos()[&japan].len(), 2);
        Ok(())
    }
}
