//! The open catalog and what was read from it, apart from the window that
//! shows it: opening needs no UI, and reading it again keeps the lists in step.
mod backfill;
pub(crate) mod background;
mod descriptive;

use crate::catalog::{
    Catalog, CatalogLocation, Collection, CollectionId, Folder, FolderId, Photo, PhotoId, RootId,
};
use anyhow::Result;
pub(crate) use background::Wake;
pub(crate) use descriptive::{Committed, DescriptiveChange, DescriptiveEdit};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub(crate) struct CatalogSession {
    pub catalog: Catalog,
    /// The catalog's photos, without macOS "._" metadata files.
    pub photos: Vec<Photo>,
    /// Folders, each counting the photos in `photos` it holds.
    pub folders: Vec<Folder>,
    pub collections: Vec<Collection>,
    /// Each collection's photos, limited to the ones in `photos`.
    pub collection_photos: HashMap<CollectionId, HashSet<PhotoId>>,
    pub roots: Vec<(RootId, String, Option<String>)>,
    backfill: backfill::Backfill,
    wake: Wake,
}

/// A catalog just opened, with what opening it could not finish.
pub(crate) struct Opened {
    pub session: CatalogSession,
    /// Lightroom's keyword export options could not be read; tried again on
    /// the next open.
    pub keyword_export: Option<anyhow::Error>,
}

impl CatalogSession {
    pub(crate) fn open(location: &CatalogLocation) -> Result<Opened> {
        crate::platform::network::prepare_filesystem_bridge();
        let mut catalog = Catalog::open(location)?;
        // Masks made from a selection are read back from this catalog wherever they
        // are rendered: Develop, the Library's thumbnails, previews and exports.
        crate::storage::mask_assets::add_loader(catalog.mask_asset_loader());
        // Catalogs imported before history was kept: recover it from the
        // stored Lightroom catalog. Best effort; a failure only hides history.
        let _ = catalog.backfill_lightroom_history();
        let _ = catalog.backfill_lightroom_snapshots();
        let _ = catalog.backfill_lightroom_info();
        let _ = catalog.backfill_lightroom_metadata();
        // Unlike those, a failure here could export keywords Lightroom keeps
        // out, so it is said; it is tried again on the next open.
        let keyword_export = catalog.backfill_keyword_export().err();
        let mut session = Self {
            catalog,
            photos: Vec::new(),
            folders: Vec::new(),
            collections: Vec::new(),
            collection_photos: HashMap::new(),
            roots: Vec::new(),
            backfill: Default::default(),
            wake: std::sync::Arc::new(|| {}),
        };
        session.reload()?;
        Ok(Opened {
            session,
            keyword_export,
        })
    }
    /// Calls `wake` whenever a background read has something to save, for
    /// whoever shows the catalog to poll it.
    pub(crate) fn wake_with(&mut self, wake: Wake) {
        self.wake = wake;
    }
    /// Sets rating, flag and label in the catalog, in one transaction, then
    /// in `photos`.
    pub(crate) fn set_ratings(&mut self, values: &[(PhotoId, i32, i32, String)]) -> Result<()> {
        self.catalog.set_metadata_of(values)?;
        for (id, rating, flag, label) in values {
            if let Some(p) = self.photos.iter_mut().find(|p| p.id == *id) {
                p.rating = *rating;
                p.flag = *flag;
                p.label = label.clone();
            }
        }
        Ok(())
    }
    /// Reads rating, flag, label, capture time and keywords of `ids` again
    /// from the catalog into `photos`, after the catalog changed them. The
    /// photos keep their order.
    pub(crate) fn refresh_photos(&mut self, ids: &[PhotoId]) -> Result<()> {
        let wanted: HashSet<PhotoId> = ids.iter().copied().collect();
        let fresh: HashMap<PhotoId, Photo> = self
            .catalog
            .photos()?
            .into_iter()
            .filter(|p| wanted.contains(&p.id))
            .map(|p| (p.id, p))
            .collect();
        for p in &mut self.photos {
            if let Some(f) = fresh.get(&p.id) {
                p.rating = f.rating;
                p.flag = f.flag;
                p.label = f.label.clone();
                p.captured = f.captured.clone();
            }
        }
        self.refresh_keywords(ids)
    }
    /// Reads the keywords of `ids` again from the catalog into `photos`.
    pub(crate) fn refresh_keywords(&mut self, ids: &[PhotoId]) -> Result<()> {
        let mut names = HashMap::new();
        for id in ids {
            let keywords: Vec<String> = self
                .catalog
                .keywords(*id)?
                .into_iter()
                .map(|k| k.name)
                .collect();
            names.insert(*id, keywords.join(", "));
        }
        for p in &mut self.photos {
            if let Some(n) = names.remove(&p.id) {
                p.keywords = n;
            }
        }
        Ok(())
    }
    /// Lightroom's Quick Collection, once there is one.
    pub(crate) fn quick_collection(&self) -> Option<CollectionId> {
        use crate::catalog::{CollectionKind, QUICK_COLLECTION};
        self.collections
            .iter()
            .find(|c| {
                c.kind == CollectionKind::System && c.name == QUICK_COLLECTION && c.parent.is_none()
            })
            .map(|c| c.id)
    }
    /// The Quick Collection, made if there is none yet.
    pub(crate) fn ensure_quick_collection(&mut self) -> Result<CollectionId> {
        if let Some(id) = self.quick_collection() {
            return Ok(id);
        }
        let id = self.catalog.quick_collection()?;
        self.collections = self.catalog.collections()?;
        self.collection_photos.entry(id).or_default();
        Ok(id)
    }
    /// Reads the photos, folders, collections and roots again. All or nothing:
    /// when a read fails, the lists stay as they were, still matching each other.
    pub(crate) fn reload(&mut self) -> Result<()> {
        // Earlier imports could pick up macOS "._" metadata files; never show them.
        let mut photos = self.catalog.photos()?;
        photos.retain(|p| !crate::storage::is_hidden(Path::new(&p.filename)));
        let mut folders = self.catalog.folders()?;
        // One pass over the photos: a large catalog has thousands of folders.
        let mut counts: HashMap<FolderId, usize> = HashMap::new();
        for photo in &photos {
            *counts.entry(photo.folder).or_default() += 1;
        }
        for folder in &mut folders {
            folder.count = counts.get(&folder.id).copied().unwrap_or(0);
        }
        let collections = self.catalog.collections()?;
        let ids: HashSet<PhotoId> = photos.iter().map(|p| p.id).collect();
        let mut collection_photos = self.catalog.collection_photos()?;
        for members in collection_photos.values_mut() {
            members.retain(|id| ids.contains(id));
        }
        let roots = self.catalog.roots()?;
        self.photos = photos;
        self.folders = folders;
        self.collections = collections;
        self.collection_photos = collection_photos;
        self.roots = roots;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_reads_the_catalog_with_no_window() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        image::RgbImage::new(8, 8).save(folder.join("a.jpg"))?;
        image::RgbImage::new(8, 8).save(folder.join("b.jpg"))?;
        let path = directory.path().join("library.rawmakase");
        let mut catalog = Catalog::create(&path)?;
        catalog.add_folder(&folder)?;
        let collection = catalog.quick_collection()?;
        let a = catalog.photos()?[0].id;
        catalog.change_collection(collection, &[a], &[])?;
        drop(catalog);

        let Opened { mut session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
        assert_eq!(session.photos.len(), 2);
        assert_eq!(session.folders.len(), 1);
        assert_eq!(session.folders[0].count, 2);
        assert_eq!(session.collections.len(), 1);
        assert_eq!(session.collection_photos[&collection], HashSet::from([a]));

        // Changes made through the catalog show once it is read again.
        session.catalog.create_virtual_copy(a)?;
        session.catalog.change_collection(collection, &[], &[a])?;
        session.reload()?;
        assert_eq!(session.photos.len(), 3);
        assert_eq!(session.folders[0].count, 3);
        assert!(!session.collection_photos.contains_key(&collection));
        Ok(())
    }

    #[test]
    fn capture_times_and_photo_info_are_read_saved_and_listed_with_no_window() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        std::fs::write(
            folder.join("a.jpg"),
            crate::export::exif::dated_file(true, "2024:03:02 10:00:01", ""),
        )?;
        // A file with a size to read, and no date.
        image::RgbImage::new(6, 4).save(folder.join("b.png"))?;
        let path = directory.path().join("library.rawmakase");
        Catalog::create(&path)?.add_folder(&folder)?;
        let Opened { mut session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
        let id = |session: &CatalogSession, name: &str| {
            session
                .photos
                .iter()
                .find(|p| p.filename == name)
                .unwrap()
                .id
        };
        let (a, b) = (id(&session, "a.jpg"), id(&session, "b.png"));
        let copy = session.catalog.create_virtual_copy(a)?;
        session.reload()?;
        let woken = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = woken.clone();
        session.wake_with(std::sync::Arc::new(move || {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));

        session.start_capture_times(|_| true);
        session.start_photo_info(|_| true);
        let (mut dated, mut infos) = (Vec::new(), 0);
        let started = std::time::Instant::now();
        while session.reading_capture_times() || session.reading_photo_info() {
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            let capture = session.poll_capture_times();
            assert!(capture.error.is_none());
            dated.extend(capture.saved);
            let info = session.poll_photo_info();
            assert!(info.error.is_none());
            infos += info.saved;
            std::thread::yield_now();
        }
        assert_eq!(dated, [(a, "2024-03-02T10:00:01.000".to_string())]);
        assert!(infos > 0);
        assert!(woken.load(std::sync::atomic::Ordering::Relaxed) > 0);
        // The copy shows its master's time, as the catalog reads it again.
        for id in [a, copy] {
            let photo = session.photos.iter().find(|p| p.id == id).unwrap();
            assert_eq!(photo.captured, "2024-03-02T10:00:01.000");
        }
        let info = session.catalog.photo_info(b)?.expect("read from the file");
        assert_eq!(info.dimensions, Some((6, 4)));
        session.reload()?;
        let captured = |id| {
            session
                .photos
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .captured
                .clone()
        };
        assert_eq!(captured(copy), "2024-03-02T10:00:01.000");
        // Tried once: not read again until asked to retry.
        session.start_capture_times(|_| true);
        assert!(!session.reading_capture_times());
        Ok(())
    }

    #[test]
    fn ratings_and_the_quick_collection_change_the_catalog_and_the_lists() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        image::RgbImage::new(8, 8).save(folder.join("a.jpg"))?;
        let path = directory.path().join("library.rawmakase");
        Catalog::create(&path)?.add_folder(&folder)?;
        let Opened { mut session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
        let a = session.photos[0].id;

        session.set_ratings(&[(a, 4, 1, "Red".into())])?;
        let listed = &session.photos[0];
        assert_eq!(
            (listed.rating, listed.flag, listed.label.as_str()),
            (4, 1, "Red")
        );
        let stored = &session.catalog.photos()?[0];
        assert_eq!(
            (stored.rating, stored.flag, stored.label.as_str()),
            (4, 1, "Red")
        );

        assert_eq!(session.quick_collection(), None);
        let quick = session.ensure_quick_collection()?;
        assert_eq!(session.quick_collection(), Some(quick));
        assert_eq!(session.ensure_quick_collection()?, quick);
        assert!(session.collection_photos[&quick].is_empty());
        let members = session.change_collection(quick, &[a], &[])?;
        assert!(members.contains(&a));
        assert!(session.catalog.collection_photos()?[&quick].contains(&a));
        session.change_collection(quick, &[], &[a])?;
        assert!(session.collection_photos[&quick].is_empty());

        let copy = session.create_virtual_copy(a)?.value;
        session.rename_copy(copy, " B&W ")?;
        let named = |photos: &[Photo]| {
            photos
                .iter()
                .find(|p| p.id == copy)
                .unwrap()
                .copy_name
                .clone()
        };
        assert_eq!(named(&session.photos), "B&W");
        assert_eq!(named(&session.catalog.photos()?), "B&W");
        Ok(())
    }

    #[test]
    fn descriptive_edits_and_copies_change_the_catalog_and_the_lists() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        image::RgbImage::new(8, 8).save(folder.join("a.jpg"))?;
        let path = directory.path().join("library.rawmakase");
        Catalog::create(&path)?.add_folder(&folder)?;
        let Opened { mut session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
        let a = session.photos[0].id;

        let change = session.edit_descriptive(
            &[a],
            DescriptiveEdit::AddKeywords(vec![vec!["Places".into(), "Kraków".into()]]),
        )?;
        assert!(change.listed.is_ok());
        assert_ne!(change.before, change.after);
        assert_eq!(session.photos[0].keywords, "Kraków");
        // Undone: the lists follow the catalog back.
        session.restore_descriptive(&change.before, &[(a, 3, -1, "Blue".into())])?;
        assert_eq!(session.photos[0].keywords, "");
        assert_eq!(session.photos[0].rating, 3);
        assert_eq!(session.catalog.photos()?[0].label, "Blue");
        assert_eq!(session.catalog.metadata_snapshot(&[a])?, change.before);

        let copy = session.create_virtual_copy(a)?.value;
        assert!(session.photos.iter().any(|p| p.id == copy));
        session.set_copy_as_master(copy)?;
        assert_eq!(
            session.photos.iter().find(|p| p.id == a).unwrap().master,
            Some(copy)
        );
        session.remove_virtual_copy(a)?;
        assert_eq!(session.photos.len(), 1);
        assert_eq!(session.folders[0].count, 1);
        Ok(())
    }

    #[test]
    fn a_reload_that_fails_partway_keeps_the_lists_as_they_were() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        image::RgbImage::new(8, 8).save(folder.join("a.jpg"))?;
        let path = directory.path().join("library.rawmakase");
        Catalog::create(&path)?.add_folder(&folder)?;
        let Opened { mut session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
        let a = session.photos[0].id;

        // The photos read again would include the copy; a later read fails.
        session.catalog.create_virtual_copy(a)?;
        rusqlite::Connection::open(&path)?
            .execute_batch("ALTER TABLE collection_photos RENAME TO collection_photos_gone")?;
        assert!(session.reload().is_err());
        assert_eq!(session.photos.len(), 1);
        assert_eq!(session.folders[0].count, 1);
        Ok(())
    }
}
