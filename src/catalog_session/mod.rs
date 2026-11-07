//! The open catalog and what was read from it, apart from the window that
//! shows it: opening needs no UI, and reading it again keeps the lists in step.
mod backfill;
pub(crate) mod background;
mod checksums;
mod descriptive;
mod import_stacks;
mod stacks;

use crate::catalog::{
    Catalog, CatalogLocation, Collection, CollectionId, Folder, FolderId, Photo, PhotoId, RootId,
};
use anyhow::Result;
pub(crate) use backfill::{
    capture_time as file_capture_time, read_info as file_photo_info, read_sequence as file_sequence,
};
/// A file's photo info as the catalog reads it for photos added from
/// folders; export names read a photo's without a catalog the same way.
pub(crate) use import_stacks::{BurstRead, BurstsRead};

/// What photos added from folders get where their files have none: the
/// Preferences' Creator and Copyright, giving way to the camera's own,
/// which the RAW decoders also find in a CR3, MRW or X3F, and to a CR3's
/// own XMP, which they find too; with the Preferences' other choices for
/// photos added (Stack Camera Bursts), read once, when adding starts.
pub(crate) fn new_photo_defaults() -> crate::catalog::NewPhotoDefaults {
    photo_defaults(crate::catalog::MetadataDefaults::load())
        .with_options(crate::catalog::ImportOptions::load())
}
fn photo_defaults(metadata: crate::catalog::MetadataDefaults) -> crate::catalog::NewPhotoDefaults {
    crate::catalog::NewPhotoDefaults::new(
        metadata,
        crate::photo::camera_exif,
        crate::photo::raw_xmp,
    )
}
/// Read Metadata from Files' read of one photo: its sidecar and its own
/// XMP, a CR3's found by the RAW decoders, as adding it reads them.
pub(crate) fn read_file_metadata(
    path: &Path,
) -> (
    Option<crate::catalog::FileMetadata>,
    crate::catalog::SidecarReport,
) {
    crate::catalog::read_file_metadata(path, crate::photo::raw_xmp)
}
pub(crate) use background::Wake;
pub(crate) use descriptive::{Committed, DescriptiveChange, DescriptiveEdit};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Its lists are read only from outside: they change only as the session reads
/// the catalog again, or through its methods that keep them in step with the
/// catalog and each other (and start a new generation when photos may have
/// changed place, name, master or id). Writes through the session's catalog
/// that leave the lists as they are (edits, History, snapshots, camera
/// sequences, the computer's name) have methods of their own; nothing outside
/// gets the catalog to change. Workflows that change the catalog through a
/// connection of their own (adding folders, relinking, Synchronize, file
/// operations) open it again or read it again after, as before.
pub(crate) struct CatalogSession {
    catalog: Catalog,
    /// The catalog's photos, without macOS "._" metadata files.
    photos: Vec<Photo>,
    /// Folders, each counting the photos in `photos` it holds.
    folders: Vec<Folder>,
    collections: Vec<Collection>,
    /// Each collection's photos, limited to the ones in `photos`.
    collection_photos: HashMap<CollectionId, HashSet<PhotoId>>,
    roots: Vec<(RootId, String, Option<String>)>,
    /// The folders located apart from their roots on this computer, by root.
    overrides: HashMap<RootId, Vec<crate::catalog::Override>>,
    /// Previous Import: the photos the last Add Photo Folder or Synchronize
    /// added.
    previous_import: HashSet<PhotoId>,
    backfill: backfill::Backfill,
    wake: Wake,
    /// Changes whenever photos may have changed place, name or master (see
    /// [`Self::reload`]), so a background result read before is dropped.
    generation: u64,
}

/// The last structural generation given out, by any session: unique across
/// catalogs, and across one catalog opened again.
static GENERATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A catalog just opened, with what opening it could not finish.
pub(crate) struct Opened {
    pub session: CatalogSession,
    /// Lightroom's keyword export options could not be read; tried again on
    /// the next open.
    pub keyword_export: Option<anyhow::Error>,
}

impl CatalogSession {
    /// The catalog, to read. No method of [`Catalog`] that takes `&self` writes
    /// it (`export_catalog` writes another catalog), except the test-support
    /// ones (`db_for_tests`, `fail_metadata_writes`).
    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    /// The catalog's photos, without macOS "._" metadata files.
    pub(crate) fn photos(&self) -> &[Photo] {
        &self.photos
    }
    /// Folders, each counting the photos in [`Self::photos`] it holds.
    pub(crate) fn folders(&self) -> &[Folder] {
        &self.folders
    }
    pub(crate) fn collections(&self) -> &[Collection] {
        &self.collections
    }
    /// Each collection's photos, limited to the ones in [`Self::photos`].
    pub(crate) fn collection_photos(&self) -> &HashMap<CollectionId, HashSet<PhotoId>> {
        &self.collection_photos
    }
    /// Each root: its id, the path it was added under and where it is found on
    /// this computer, when elsewhere.
    pub(crate) fn roots(&self) -> &[(RootId, String, Option<String>)] {
        &self.roots
    }
    /// The folders located apart from their roots on this computer, by root.
    pub(crate) fn overrides(&self) -> &HashMap<RootId, Vec<crate::catalog::Override>> {
        &self.overrides
    }
    /// Previous Import: the photos the last Add Photo Folder or Synchronize
    /// added.
    pub(crate) fn previous_import(&self) -> &HashSet<PhotoId> {
        &self.previous_import
    }
    /// Saves photo `id`'s edit ([`Catalog::save_edit`]). Changes no list.
    pub(crate) fn save_edit<'a>(
        &mut self,
        id: PhotoId,
        original: impl Into<crate::edits::Original<'a>>,
        source: &crate::storage::Identity,
        recipe: &crate::model::recipe::Recipe,
        export: &crate::export_settings::ExportOptions,
        history: crate::catalog::HistoryUpdate<'_>,
    ) -> Result<()> {
        self.catalog
            .save_edit(id, original, source, recipe, export, history)
    }
    /// Saves and clears edits in one transaction ([`Catalog::change_edits`]).
    /// Changes no list.
    pub(crate) fn change_edits(
        &mut self,
        changes: &[crate::catalog::EditChange<'_, '_>],
    ) -> Result<()> {
        self.catalog.change_edits(changes)
    }
    /// Adds a snapshot of `photo` ([`Catalog::add_snapshot`]). Changes no list.
    pub(crate) fn add_snapshot(
        &mut self,
        photo: PhotoId,
        name: &str,
        recipe: &crate::model::recipe::Recipe,
    ) -> Result<i64> {
        self.catalog.add_snapshot(photo, name, recipe)
    }
    /// Changes no list.
    pub(crate) fn update_snapshot(
        &mut self,
        id: i64,
        recipe: &crate::model::recipe::Recipe,
    ) -> Result<()> {
        self.catalog.update_snapshot(id, recipe)
    }
    /// Changes no list.
    pub(crate) fn rename_snapshot(&mut self, id: i64, name: &str) -> Result<()> {
        self.catalog.rename_snapshot(id, name)
    }
    /// Changes no list.
    pub(crate) fn delete_snapshot(&mut self, id: i64) -> Result<()> {
        self.catalog.delete_snapshot(id)
    }
    /// Saves camera sequences read from the files ([`Catalog::save_sequences`]).
    /// Changes no list.
    pub(crate) fn save_sequences(
        &mut self,
        read: &[(PhotoId, crate::catalog::SavedSequence)],
    ) -> Result<()> {
        self.catalog.save_sequences(read)
    }
    /// Discards the editable previews of `photos` (their masters', for virtual
    /// copies; [`Catalog::discard_editable_previews`]), returning the masters
    /// whose went. Changes no list.
    pub(crate) fn discard_editable_previews(&mut self, photos: &[PhotoId]) -> Result<Vec<PhotoId>> {
        self.catalog.discard_editable_previews(photos)
    }
    /// Discards every editable preview of the catalog. Changes no list.
    pub(crate) fn discard_all_editable_previews(&mut self) -> Result<Vec<PhotoId>> {
        let photos: Vec<PhotoId> = self
            .catalog
            .photos_with_editable_previews()?
            .into_iter()
            .collect();
        self.catalog.discard_editable_previews(&photos)
    }
    /// Renames this computer for the folder locations
    /// ([`Catalog::rename_computer`]). Changes no list.
    pub(crate) fn rename_computer(&mut self, name: &str) -> Result<()> {
        self.catalog.rename_computer(name)
    }
    /// Orders the photos by capture time, then file name and id, as the
    /// catalog lists them, after capture times were filled in or read again.
    pub(crate) fn sort_photos_by_capture_time(&mut self) {
        self.photos.sort_by(|a, b| {
            (&a.captured, &a.filename, a.id).cmp(&(&b.captured, &b.filename, b.id))
        });
    }
    /// Forgets the photos, folders, roots, collection members and Previous
    /// Import when the catalog could not be read again after a change made
    /// through another connection: a folder removed may have given its id to a
    /// new one, so none is offered to be changed. The collections and overrides
    /// stay as listed; the failed read started a new generation already.
    pub(crate) fn forget_lists(&mut self) {
        self.photos.clear();
        self.folders.clear();
        self.roots.clear();
        self.collection_photos.clear();
        self.previous_import.clear();
    }
    /// The catalog, for a test to change before reading it again.
    #[cfg(test)]
    pub(crate) fn catalog_mut_for_tests(&mut self) -> &mut Catalog {
        &mut self.catalog
    }
    /// The photos as listed, for a test to set up flags or capture times.
    #[cfg(test)]
    pub(crate) fn photos_mut_for_tests(&mut self) -> &mut Vec<Photo> {
        &mut self.photos
    }
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
        // PNGs whose info earlier releases stored without their EXIF; read by
        // the photo info backfill.
        let _ = catalog.reread_png_info_once();
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
            overrides: HashMap::new(),
            previous_import: HashSet::new(),
            backfill: Default::default(),
            wake: std::sync::Arc::new(|| {}),
            generation: 0,
        };
        session.reload()?;
        Ok(Opened {
            session,
            keyword_export,
        })
    }
    /// The structural generation (see [`Self::reload`]): a plan scanned
    /// under another applies to nothing.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    /// Starts a new structural generation without reading the catalog
    /// again: what background readers read before is dropped when it comes
    /// back, as before a change that may free photos' ids.
    pub(crate) fn bump_generation(&mut self) {
        self.generation = GENERATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
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
        // Read again after a change that may move, rename, promote or remove
        // photos: what was read before no longer applies, even if this read fails.
        self.generation = GENERATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
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
        let overrides = self.catalog.overrides()?;
        let previous_import = self.catalog.previous_import()?.into_iter().collect();
        self.photos = photos;
        self.previous_import = previous_import;
        self.folders = folders;
        self.collections = collections;
        self.collection_photos = collection_photos;
        self.roots = roots;
        self.overrides = overrides;
        self.count_checksums();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalog of two JPEGs, `a` with a virtual copy in a collection, and a session
    /// whose next read of the catalog fails (its collections are gone).
    fn session_that_cannot_read_again() -> Result<(tempfile::TempDir, CatalogSession, PhotoId)> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        image::RgbImage::new(8, 8).save(folder.join("a.jpg"))?;
        image::RgbImage::new(8, 8).save(folder.join("b.jpg"))?;
        let path = directory.path().join("library.rawmakase");
        let mut catalog = Catalog::create(&path)?;
        catalog.add_folder(&folder)?;
        let a = catalog.photos()?[0].id;
        let copy = catalog.create_virtual_copy(a)?;
        let collection = catalog.quick_collection()?;
        catalog.change_collection(collection, &[copy], &[])?;
        drop(catalog);
        let Opened { session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
        session
            .catalog
            .db_for_tests()
            .execute_batch("ALTER TABLE collections RENAME TO collections_gone")?;
        Ok((directory, session, copy))
    }

    /// Issue 808 (M3, A2): a removed copy's id may be given to the next new photo, so
    /// once it is removed the lists drop it and what background readers read before
    /// is dropped, even when the catalog cannot be read again.
    #[test]
    fn a_removed_copy_leaves_the_lists_and_the_generation_when_reading_again_fails() -> Result<()> {
        let (_directory, mut session, copy) = session_that_cannot_read_again()?;
        let generation = session.generation();
        let removed = session.remove_virtual_copy(copy)?;
        assert!(removed.listed.is_err());
        assert!(session.photos.iter().all(|p| p.id != copy));
        assert!(
            session
                .collection_photos
                .values()
                .all(|m| !m.contains(&copy))
        );
        assert_eq!(session.folders[0].count, 2);
        assert_ne!(session.generation(), generation);
        Ok(())
    }

    /// Issue 808 (M3, review): a read of photo info or capture times under way when a
    /// copy is removed, or a copy made the master, may hold an id given to another
    /// photo next (or a master's file a copy now names): it stops, and is started
    /// again by whoever shows the catalog, as when folders are removed.
    #[test]
    fn background_reads_stop_when_a_copy_is_removed_or_made_the_master() -> Result<()> {
        for change in ["remove", "promote"] {
            let directory = tempfile::tempdir()?;
            let folder = directory.path().join("photos");
            std::fs::create_dir(&folder)?;
            image::RgbImage::new(8, 8).save(folder.join("a.jpg"))?;
            let path = directory.path().join("library.rawmakase");
            let mut catalog = Catalog::create(&path)?;
            catalog.add_folder(&folder)?;
            let a = catalog.photos()?[0].id;
            let copy = catalog.create_virtual_copy(a)?;
            drop(catalog);
            let Opened { mut session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
            session.start_photo_info(|_| true);
            session.start_capture_times(|_| true);
            assert!(session.reading_photo_info(), "{change}");
            match change {
                "remove" => session.remove_virtual_copy(copy)?.listed?,
                _ => session.set_copy_as_master(copy)?.listed?,
            }
            assert!(!session.reading_photo_info(), "{change}");
            assert!(!session.reading_capture_times(), "{change}");
        }
        Ok(())
    }

    /// Removing folders, whose photos' ids may be reused, starts a new generation
    /// even when the catalog cannot be read again.
    #[test]
    fn removed_folders_start_a_new_generation_when_reading_again_fails() -> Result<()> {
        let (_directory, mut session, _) = session_that_cannot_read_again()?;
        let generation = session.generation();
        let folder = session.folders[0].id;
        let removed = session.remove_folders(&[folder], None)?;
        assert!(removed.listed.is_err());
        assert!(session.photos.is_empty());
        assert_ne!(session.generation(), generation);
        Ok(())
    }

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
        // Its sequence is read in the same pass, so Auto-Stack opens no
        // file of it: it says nothing. a's info is left for later (no image
        // to size), and so is its sequence.
        let sequences = session.catalog.sequences()?;
        assert_eq!(sequences.keys().collect::<Vec<_>>(), [&b]);
        assert_eq!(sequences[&b].sequence, None);
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
    fn import_facts_get_the_exif_date_whatever_date_the_photo_has() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        let dated = crate::export::exif::dated_file(true, "2024:03:02 10:00:01", "");
        let size = dated.len() as i64;
        std::fs::write(folder.join("a.jpg"), dated)?;
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
        // Added from a folder: its name and size, its EXIF still to read.
        assert_eq!(
            session.catalog.import_facts(a)?,
            Some(("a.jpg".into(), None, Some(size), false))
        );
        // A date from elsewhere (XMP, the user) is not the file's EXIF.
        session
            .catalog
            .fill_capture_times(&[(a, "2020-01-01T00:00:00.000".into())])?;
        session.reload()?;
        session.start_capture_times(|_| true);
        let mut saved = Vec::new();
        let started = std::time::Instant::now();
        while session.reading_capture_times() {
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            let capture = session.poll_capture_times();
            assert!(capture.error.is_none());
            saved.extend(capture.saved);
            std::thread::yield_now();
        }
        assert!(saved.is_empty(), "{saved:?}");
        let photo = session.photos.iter().find(|p| p.id == a).unwrap();
        assert_eq!(photo.captured, "2020-01-01T00:00:00.000");
        assert_eq!(
            session.catalog.import_facts(a)?,
            Some((
                "a.jpg".into(),
                Some("2024-03-02T10:00:01.000".into()),
                Some(size),
                true
            ))
        );
        // A file with no date is read once: it has none.
        let (_, captured, _, read) = session.catalog.import_facts(b)?.unwrap();
        assert_eq!((captured, read), (None, true));
        assert!(session.catalog.photos_to_read_exif_capture()?.is_empty());
        Ok(())
    }

    #[test]
    fn an_undated_copy_of_a_dated_master_shows_the_date_its_file_gives() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        std::fs::write(
            folder.join("a.jpg"),
            crate::export::exif::dated_file(true, "2024:03:02 10:00:01", ""),
        )?;
        let path = directory.path().join("library.rawmakase");
        Catalog::create(&path)?.add_folder(&folder)?;
        let Opened { mut session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
        let a = session.photos[0].id;
        let copy = session.catalog.create_virtual_copy(a)?;
        // The master dated since (by its XMP), its copy not.
        session.catalog_mut_for_tests().db_for_tests().execute(
            "UPDATE photos SET captured='2020-01-01T00:00:00.000' WHERE id=?",
            [a.0],
        )?;
        session.reload()?;
        session.start_capture_times(|_| true);
        let mut saved = Vec::new();
        let started = std::time::Instant::now();
        while session.reading_capture_times() {
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            saved.extend(session.poll_capture_times().saved);
            std::thread::yield_now();
        }
        let captured = |session: &CatalogSession, id| {
            session
                .photos
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .captured
                .clone()
        };
        // As the catalog has them, so the Library sorts them again.
        assert_eq!(captured(&session, a), "2020-01-01T00:00:00.000");
        assert_eq!(captured(&session, copy), "2024-03-02T10:00:01.000");
        assert!(!saved.is_empty());
        session.reload()?;
        assert_eq!(captured(&session, copy), "2024-03-02T10:00:01.000");
        Ok(())
    }

    /// A little-endian TIFF of one directory of ASCII tags, values of up to
    /// four bytes in their entries.
    fn ascii_tiff(tags: &[(u16, &str)]) -> Vec<u8> {
        let end = 8 + 2 + 12 * tags.len() + 4;
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        out.extend((tags.len() as u16).to_le_bytes());
        let mut values = Vec::new();
        for (tag, text) in tags {
            let mut v = text.as_bytes().to_vec();
            v.push(0);
            out.extend(tag.to_le_bytes());
            out.extend(2u16.to_le_bytes());
            out.extend((v.len() as u32).to_le_bytes());
            if v.len() <= 4 {
                v.resize(4, 0);
                out.extend(v);
            } else {
                out.extend(((end + values.len()) as u32).to_le_bytes());
                values.extend(v);
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(values);
        out
    }

    /// A CR3 of its metadata alone: IFD0 (`CMT1`) and the EXIF directory
    /// (`CMT2`) in Canon's box in `moov`.
    fn dated_cr3(original: &str, subseconds: &str) -> Vec<u8> {
        let bx = |kind: &[u8], body: &[u8]| {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend(kind);
            v.extend(body);
            v
        };
        let canon = [
            0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b,
            0x6a, 0x48,
        ];
        let cmt1 = ascii_tiff(&[(271, "Canon"), (306, "2030:01:01 00:00:00")]);
        let cmt2 = ascii_tiff(&[(36867, original), (37521, subseconds)]);
        let uuid = [canon.to_vec(), bx(b"CMT1", &cmt1), bx(b"CMT2", &cmt2)].concat();
        [
            bx(b"ftyp", b"crx \0\0\0\x01crx isom"),
            bx(b"moov", &bx(b"uuid", &uuid)),
        ]
        .concat()
    }

    /// A CRW whose heap holds ImageProps with its TimeStamp record alone:
    /// seconds since 1970 in the camera's time.
    fn dated_crw(seconds: u32) -> Vec<u8> {
        // A heap block: the records' values, the directory, its offset.
        let block = |records: &[(u16, Vec<u8>)]| {
            let mut data = Vec::new();
            let mut entries = Vec::new();
            for (tag, value) in records {
                entries.extend(tag.to_le_bytes());
                entries.extend((value.len() as u32).to_le_bytes());
                entries.extend((data.len() as u32).to_le_bytes());
                data.extend(value);
            }
            let dir = data.len() as u32;
            data.extend((records.len() as u16).to_le_bytes());
            data.extend(entries);
            data.extend(dir.to_le_bytes());
            data
        };
        let stamp = [seconds, 0, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let props = block(&[(0x180e, stamp)]);
        let mut file = b"II".to_vec();
        file.extend(26u32.to_le_bytes());
        file.extend(b"HEAPCCDR");
        file.extend([0; 12]);
        file.extend(block(&[(0x300a, props)]));
        file
    }

    #[test]
    fn canon_raws_added_from_a_folder_are_dated_from_their_own_records() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        std::fs::write(folder.join("a.CR3"), dated_cr3("2024:03:02 10:00:01", "12"))?;
        // 2004-05-01 10:43:58.
        std::fs::write(folder.join("b.CRW"), dated_crw(1_083_408_238))?;
        let path = directory.path().join("library.rawmakase");
        Catalog::create(&path)?.add_folder(&folder)?;
        // Catalogued without a date, as releases that could not read these
        // files left them: the next start reads them.
        let Opened { mut session, .. } = CatalogSession::open(&CatalogLocation::from(&path))?;
        assert!(session.photos.iter().all(|p| p.captured.is_empty()));
        session.start_capture_times(|_| true);
        let started = std::time::Instant::now();
        while session.reading_capture_times() {
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            assert!(session.poll_capture_times().error.is_none());
            std::thread::yield_now();
        }
        session.reload()?;
        let mut dated: Vec<_> = session
            .photos
            .iter()
            .map(|p| (p.filename.as_str(), p.captured.as_str()))
            .collect();
        dated.sort();
        assert_eq!(
            dated,
            [
                ("a.CR3", "2024-03-02T10:00:01.120"),
                ("b.CRW", "2004-05-01T10:43:58.000"),
            ]
        );
        // Dated photos are not read again, even once retried.
        session.retry_capture_times();
        session.start_capture_times(|_| true);
        assert!(!session.reading_capture_times());
        Ok(())
    }

    /// Export names of a photo opened without a catalog read its capture
    /// time by the same rule.
    #[test]
    fn a_file_alone_is_dated_as_the_catalog_dates_it() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let (cr3, crw) = (
            directory.path().join("a.CR3"),
            directory.path().join("b.CRW"),
        );
        std::fs::write(&cr3, dated_cr3("2024:03:02 10:00:01", "12"))?;
        std::fs::write(&crw, dated_crw(1_083_408_238))?;
        assert_eq!(
            file_capture_time(&cr3).as_deref(),
            Some("2024-03-02T10:00:01.120")
        );
        assert_eq!(
            file_capture_time(&crw).as_deref(),
            Some("2004-05-01T10:43:58.000")
        );
        Ok(())
    }

    /// The Preferences' Creator and Copyright give way to a CR3's own,
    /// which the catalog reads through the app's reader (#671).
    #[test]
    fn a_cr3s_own_creator_and_copyright_keep_the_defaults_out() -> Result<()> {
        use crate::exif::{CameraExif, Field, tag};
        use crate::export::exif::{Container, camera_raw};
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        let camera = |main: Vec<Field>| CameraExif {
            main,
            ..Default::default()
        };
        let own = camera(vec![
            Field::ascii(tag::MAKE, "Canon"),
            Field::ascii(tag::ARTIST, "Ann Example"),
            Field::ascii(tag::COPYRIGHT, "Ann Example 2024"),
        ]);
        std::fs::write(folder.join("own.CR3"), camera_raw(Container::Cr3, own))?;
        let none = camera(vec![Field::ascii(tag::MAKE, "Canon")]);
        std::fs::write(folder.join("none.CR3"), camera_raw(Container::Cr3, none))?;
        let mut catalog = Catalog::create(&directory.path().join("Photos.rawmakase"))?;
        let defaults = photo_defaults(crate::catalog::MetadataDefaults {
            creator: "Default Creator".into(),
            copyright: "© Default".into(),
        });
        catalog.add_folder_with(&folder, &defaults)?;
        let creator = |name: &str| -> Result<_> {
            let photo = catalog.photos()?.into_iter().find(|p| p.filename == name);
            Ok(catalog.descriptive(photo.expect(name).id)?.creator)
        };
        assert_eq!(creator("own.CR3")?, None);
        assert_eq!(
            creator("none.CR3")?,
            Some(crate::metadata::Value::Set(vec!["Default Creator".into()]))
        );
        Ok(())
    }

    /// The XMP a camera embeds in a CR3 (its box after `moov`) gives a new
    /// photo its in-camera rating and title, and Read Metadata from Files
    /// reads it the same way (#680).
    #[test]
    fn a_cr3s_own_xmp_gives_its_rating_and_title() -> Result<()> {
        use crate::export::exif::{Container, camera_raw};
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        let packet = r#"<?xpacket begin='' id='W5M0MpCehiHzreSzNTczkc9d'?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmp:Rating="3"><dc:title><rdf:Alt><rdf:li xml:lang="x-default">Rated in camera</rdf:li></rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta>
    <?xpacket end='w'?>"#;
        let mut cr3 = camera_raw(Container::Cr3, Default::default());
        let mut body = vec![
            0xbe, 0x7a, 0xcf, 0xcb, 0x97, 0xa9, 0x42, 0xe8, 0x9c, 0x71, 0x99, 0x94, 0x91, 0xe3,
            0xaf, 0xac,
        ];
        body.extend(packet.as_bytes());
        cr3.extend(((body.len() + 8) as u32).to_be_bytes());
        cr3.extend(b"uuid");
        cr3.extend(body);
        let file = folder.join("rated.CR3");
        std::fs::write(&file, cr3)?;
        let mut catalog = Catalog::create(&directory.path().join("Photos.rawmakase"))?;
        catalog.add_folder_with(&folder, &photo_defaults(Default::default()))?;
        let photo = catalog.photos()?.into_iter().next().expect("added");
        assert_eq!(photo.rating, 3);
        let title = crate::metadata::Value::Set(crate::metadata::LangAlt::new("Rated in camera"));
        assert_eq!(catalog.descriptive(photo.id)?.title, Some(title));
        let (read, report) = read_file_metadata(&file);
        assert_eq!(report, Default::default());
        assert_eq!(read.and_then(|r| r.rating), Some(3));
        Ok(())
    }

    /// Reads photo info until the reader is done.
    fn read_photo_info(session: &mut CatalogSession) {
        let started = std::time::Instant::now();
        while session.reading_photo_info() {
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            assert!(session.poll_photo_info().error.is_none());
            std::thread::yield_now();
        }
    }

    #[test]
    fn pngs_stored_with_their_size_alone_get_their_camera_once() -> Result<()> {
        use crate::metadata::PhotoInfo;
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        std::fs::write(
            folder.join("a.png"),
            crate::export::exif::camera_png(6, 4, ("SONY", "ILCE-7M4"), "Test lens"),
        )?;
        image::RgbImage::new(8, 2).save(folder.join("b.png"))?;
        image::RgbImage::new(2, 2).save(folder.join("c.jpg"))?;
        let path = directory.path().join("library.rawmakase");
        let mut catalog = Catalog::create(&path)?;
        catalog.add_folder(&folder)?;
        let id = |name: &str, catalog: &Catalog| -> Result<PhotoId> {
            Ok(catalog
                .photos()?
                .iter()
                .find(|p| p.filename == name)
                .unwrap()
                .id)
        };
        let (a, b, c) = (
            id("a.png", &catalog)?,
            id("b.png", &catalog)?,
            id("c.jpg", &catalog)?,
        );
        // What releases that read no EXIF from PNGs stored: the size alone.
        let size = |dimensions| PhotoInfo {
            dimensions: Some(dimensions),
            ..PhotoInfo::default()
        };
        catalog.fill_photo_info(&[
            (a, Some(size((6, 4)))),
            (b, Some(size((8, 2)))),
            (c, Some(size((2, 2)))),
        ])?;
        drop(catalog);
        let open = || -> Result<CatalogSession> {
            Ok(CatalogSession::open(&CatalogLocation::from(&path))?.session)
        };

        // Offline, nothing is read, and they wait for the next open.
        let mut session = open()?;
        // JPEGs' EXIF always was read: only the PNGs are read again.
        let mut listed = session.catalog.photos_to_read_info()?;
        let mut pngs = [a, b];
        listed.sort();
        pngs.sort();
        assert_eq!(listed, pngs);
        session.start_photo_info(|_| false);
        assert!(!session.reading_photo_info());
        drop(session);
        let mut session = open()?;
        session.start_photo_info(|_| true);
        assert!(session.reading_photo_info());
        read_photo_info(&mut session);
        let info = session.catalog.photo_info(a)?.unwrap();
        assert_eq!(info.camera.as_deref(), Some("SONY ILCE-7M4"));
        assert_eq!(info.lens.as_deref(), Some("Test lens"));
        assert_eq!(info.dimensions, Some((6, 4)));
        assert_eq!(session.catalog.photo_info(b)?, Some(size((8, 2))));
        // Read once: neither is read again, after reopening either.
        session.start_photo_info(|_| true);
        assert!(!session.reading_photo_info());
        drop(session);
        let mut session = open()?;
        session.start_photo_info(|_| true);
        assert!(!session.reading_photo_info());
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
