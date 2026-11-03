//! Title, caption, creator, copyright, location and keywords, collection
//! membership and virtual copies with their Copy Names: changes to the catalog
//! that the session's lists must follow.
use super::CatalogSession;
use crate::catalog::{FileMetadata, Merge, MetadataSnapshot, PhotoId, SidecarReport};
use crate::metadata::TextField;
use anyhow::Result;
use std::path::PathBuf;

/// A descriptive metadata change to some photos.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DescriptiveEdit {
    /// The default language's text; empty clears the field.
    Text(TextField, String),
    /// In order; none clears the field.
    Creators(Vec<String>),
    ClearLocation,
    /// Keywords by path, top first.
    AddKeywords(Vec<Vec<String>>),
    RemoveKeyword(i64),
}

/// A change saved in the catalog, and whether the lists could be read again
/// after it. When they could not, they still show the catalog as it was before
/// the change, which stands: what depends on it (Undo, the open photo) must
/// follow the change anyway.
#[derive(Debug)]
pub(crate) struct Committed<T> {
    pub(crate) value: T,
    pub(crate) listed: Result<()>,
}

/// What a [`DescriptiveEdit`] changed.
#[derive(Debug)]
pub(crate) struct DescriptiveChange {
    pub before: Vec<MetadataSnapshot>,
    pub after: Vec<MetadataSnapshot>,
    /// Reading the photos' keywords back into the lists: the change is saved
    /// either way, so it can be undone even when this failed.
    pub listed: Result<()>,
}

impl CatalogSession {
    /// Makes `edit` to `ids` in the catalog and lists their keywords again.
    pub(crate) fn edit_descriptive(
        &mut self,
        ids: &[PhotoId],
        edit: DescriptiveEdit,
    ) -> Result<DescriptiveChange> {
        let before = self.catalog.metadata_snapshot(ids)?;
        match edit {
            DescriptiveEdit::Text(field, text) => self.catalog.set_text(ids, field, &text),
            DescriptiveEdit::Creators(names) => self.catalog.set_creators(ids, &names),
            DescriptiveEdit::ClearLocation => self.catalog.clear_location(ids),
            DescriptiveEdit::AddKeywords(paths) => self.catalog.add_keywords(ids, &paths),
            DescriptiveEdit::RemoveKeyword(keyword) => self.catalog.remove_keyword(ids, keyword),
        }?;
        let after = self.catalog.metadata_snapshot(ids)?;
        Ok(DescriptiveChange {
            before,
            after,
            listed: self.refresh_keywords(ids),
        })
    }
    /// Puts photos' descriptive metadata back as `values` has it, with the
    /// rating, flag and label in `ratings` (none for a change that left them),
    /// then lists them again. Both are written before anything is read back,
    /// so a failed read never leaves an undo half done.
    pub(crate) fn restore_descriptive(
        &mut self,
        values: &[MetadataSnapshot],
        ratings: &[(PhotoId, i32, i32, String)],
    ) -> Result<()> {
        self.catalog.restore_metadata(values)?;
        if !ratings.is_empty() {
            self.set_ratings(ratings)?;
        }
        let ids: Vec<PhotoId> = values.iter().map(|s| s.photo).collect();
        self.refresh_photos(&ids)
    }
    /// Writes what was read from the files of `ids` (Read Metadata from
    /// Files), replacing the catalog's values, and lists them again.
    pub(crate) fn apply_file_metadata(
        &mut self,
        ids: &[PhotoId],
        read: &[(PhotoId, PathBuf, FileMetadata)],
    ) -> Result<SidecarReport> {
        let written = self.catalog.apply_file_metadata(read, Merge::Overwrite)?;
        self.refresh_photos(ids)?;
        Ok(written)
    }
    /// Names virtual copy `id` `name`, trimmed, in the catalog and the lists.
    pub(crate) fn rename_copy(&mut self, id: PhotoId, name: &str) -> Result<()> {
        self.catalog.set_copy_name(id, name)?;
        if let Some(p) = self.photos.iter_mut().find(|p| p.id == id) {
            p.copy_name = name.trim().to_string();
        }
        Ok(())
    }
    /// Adds `add` to `collection` and takes `remove` out of it, in the catalog
    /// and the lists. Returns the collection's photos as they now are.
    pub(crate) fn change_collection(
        &mut self,
        collection: crate::catalog::CollectionId,
        add: &[PhotoId],
        remove: &[PhotoId],
    ) -> Result<&std::collections::HashSet<PhotoId>> {
        self.catalog.change_collection(collection, add, remove)?;
        let members = self.collection_photos.entry(collection).or_default();
        members.extend(add);
        for id in remove {
            members.remove(id);
        }
        Ok(members)
    }
    /// Makes a virtual copy of `id` and reads the lists again. An error means
    /// no copy was made.
    pub(crate) fn create_virtual_copy(&mut self, id: PhotoId) -> Result<Committed<PhotoId>> {
        let copy = self.catalog.create_virtual_copy(id)?;
        Ok(Committed {
            value: copy,
            listed: self.reload(),
        })
    }
    /// Makes copy `id` its photo's master and reads the lists again. An error
    /// means nothing changed. Reading capture times and photo info stops, as what
    /// they read was of the masters before; start it again after.
    pub(crate) fn set_copy_as_master(&mut self, id: PhotoId) -> Result<Committed<()>> {
        self.stop_backfill();
        self.catalog.set_copy_as_master(id)?;
        Ok(Committed {
            value: (),
            listed: self.reload(),
        })
    }
    /// Removes root `root`, which has no folder left in the catalog, and
    /// reads the lists again; whether it was removed (not when a folder
    /// was added to it meanwhile).
    pub(crate) fn remove_empty_root(
        &mut self,
        root: crate::catalog::RootId,
    ) -> Result<Committed<bool>> {
        let removed = self.catalog.remove_empty_root(root)?;
        let listed = self.reload();
        if removed && listed.is_err() {
            self.roots.retain(|(id, ..)| *id != root);
        }
        Ok(Committed {
            value: removed,
            listed,
        })
    }
    /// Removes virtual copy `id` and reads the lists again. An error means the
    /// copy is still there; once this returns, it is gone and its id may be
    /// given to a new photo, even when reading the lists failed. Reading capture
    /// times and photo info stops, as it may hold the id; start it again after.
    pub(crate) fn remove_virtual_copy(&mut self, id: PhotoId) -> Result<Committed<()>> {
        self.stop_backfill();
        self.catalog.remove_virtual_copy(id)?;
        let listed = self.reload();
        if listed.is_err() {
            // The lists as they were, less the copy: its id may be given to a new
            // photo.
            if let Some(i) = self.photos.iter().position(|p| p.id == id) {
                let copy = self.photos.remove(i);
                if let Some(folder) = self.folders.iter_mut().find(|f| f.id == copy.folder) {
                    folder.count = folder.count.saturating_sub(1);
                }
            }
            for members in self.collection_photos.values_mut() {
                members.remove(&id);
            }
            self.previous_import.remove(&id);
        }
        Ok(Committed { value: (), listed })
    }
    /// Removes `folders` and their photos from the catalog, leaving the files,
    /// and reads the lists again; returns the photos removed. An error means
    /// nothing was removed; once this returns, their ids may be given to new
    /// photos, even when reading the lists failed. Reading capture times and
    /// photo info stops; start it again for the photos left. Folders
    /// `exported` are removed only while they are still what was exported.
    pub(crate) fn remove_folders(
        &mut self,
        folders: &[crate::catalog::FolderId],
        exported: Option<&crate::catalog::ExportedFolders>,
    ) -> Result<Committed<Vec<PhotoId>>> {
        self.stop_backfill();
        let removed = match exported {
            Some(exported) => self.catalog.remove_exported_folders(exported)?,
            None => self.catalog.remove_folders(folders)?,
        };
        let listed = self.reload();
        if listed.is_err() {
            // The lists as they were, less what is gone from the catalog: the
            // removed ids may be given to new photos.
            self.photos.retain(|p| !removed.contains(&p.id));
            self.folders.retain(|f| !folders.contains(&f.id));
            let roots: std::collections::HashSet<_> = self.folders.iter().map(|f| f.root).collect();
            self.roots.retain(|(root, ..)| roots.contains(root));
            for members in self.collection_photos.values_mut() {
                members.retain(|id| !removed.contains(id));
            }
        }
        Ok(Committed {
            value: removed,
            listed,
        })
    }
}
