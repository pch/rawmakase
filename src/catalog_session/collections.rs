//! Collections, collection sets and their photos: changes to the catalog
//! that the session's `collections` and `collection_photos` follow.
use super::CatalogSession;
use crate::catalog::{CollectionId, Member, MembershipChange, PhotoId};
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
}
