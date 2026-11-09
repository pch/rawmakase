//! Adding a folder of photos to the catalog, with the edits they got from
//! releases that saved them beside the photo.
use super::db::{Reads, Write, is_storage_error, sql};
use super::locations::{FolderLocation, join, logical_from_os, resolve_in};
use super::value::row;
use super::{Catalog, FolderId, PhotoId, RootId};
use anyhow::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// What adding a folder did.
#[derive(Debug, Default)]
pub struct Added {
    /// Photos added.
    pub added: usize,
    /// Sidecars that could not be read.
    pub report: super::SidecarReport,
    /// Folders not added: a location on this computer says their photos are
    /// elsewhere.
    pub conflicts: Vec<Conflict>,
    /// Folders that equally specific locations claim. Nothing is added
    /// until one of the options is chosen for each.
    pub ambiguous: Vec<Ambiguity>,
}
/// A folder on disk not added because its catalog folder is elsewhere on
/// this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub directory: PathBuf,
    pub root: RootId,
    /// The catalog folder it would have been, by logical path.
    pub relative: String,
    /// Where that folder is on this computer.
    pub located: PathBuf,
}
impl Conflict {
    pub fn message(&self) -> String {
        format!(
            "{} is linked to {} on this computer; {} was not added",
            if self.relative.is_empty() {
                "Its root folder"
            } else {
                &self.relative
            },
            self.located.display(),
            self.directory.display()
        )
    }
}
/// Folders on disk that more than one location on this computer contains
/// equally closely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ambiguity {
    pub directories: Vec<PathBuf>,
    pub options: Vec<FolderLocation>,
}
/// The answer to an `Ambiguity`: which of its options the folders are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub options: Vec<FolderLocation>,
    pub chosen: FolderLocation,
}
/// What added a batch of photos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::catalog) enum ImportKind {
    Add,
    Sync,
}
impl ImportKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sync => "sync",
        }
    }
}
/// Starts a batch of photos added by `kind`, in `w`'s transaction: Previous
/// Import from now on.
pub(in crate::catalog) fn new_batch(w: &mut Write<'_>, kind: ImportKind) -> Result<i64> {
    w.insert_returning_id(
        sql!("INSERT INTO import_batches(kind) VALUES(?) RETURNING id"),
        &[&kind.as_str()],
    )
}
/// The longest gap between two frames of a burst whose camera didn't number
/// them (Canon's and Nikon's continuous drive), for Stack Camera Bursts when
/// adding photos: kept with the batch, the input its stacking takes.
pub const IMPORT_BURST_SECONDS: f64 = 2.;

/// A batch added with Stack Camera Bursts whose bursts aren't stacked yet.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingBurstStacks {
    pub batch: i64,
    /// The longest gap between frames a camera didn't number.
    pub seconds: f64,
    /// The batch's photos still in the catalog.
    pub photos: Vec<PhotoId>,
}

impl Catalog {
    /// The batches added with Stack Camera Bursts whose bursts are still to
    /// be stacked, oldest first: kept until stacked, across restarts.
    pub fn pending_burst_stacks(&self) -> Result<Vec<PendingBurstStacks>> {
        row! {
            struct Pending {
                batch: i64,
                seconds: f64,
            }
        }
        row! {
            struct Member {
                batch: i64,
                photo: PhotoId,
            }
        }
        self.db.snapshot(|s| {
            let pending: Vec<Pending> = s.read(
                sql!("SELECT batch, seconds FROM import_burst_stacks ORDER BY batch"),
                &[],
            )?;
            let members: Vec<Member> = s.read(
                sql!(
                    "SELECT i.batch, i.photo FROM photo_imports i
                     JOIN import_burst_stacks b ON b.batch = i.batch
                     ORDER BY i.photo"
                ),
                &[],
            )?;
            let mut photos: HashMap<i64, Vec<PhotoId>> = HashMap::new();
            for m in members {
                photos.entry(m.batch).or_default().push(m.photo);
            }
            Ok(pending
                .into_iter()
                .map(|p| PendingBurstStacks {
                    batch: p.batch,
                    seconds: p.seconds,
                    photos: photos.remove(&p.batch).unwrap_or_default(),
                })
                .collect())
        })
    }
    /// Lightroom's Previous Import: the photos the last Add Photo Folder or
    /// Synchronize added that are still in the catalog.
    pub fn previous_import(&self) -> Result<Vec<PhotoId>> {
        self.db.read(
            sql!(
                "SELECT photo FROM photo_imports
                 WHERE batch = (SELECT max(id) FROM import_batches)"
            ),
            &[],
        )
    }
}
/// Where the directories of files found on disk go in the catalog.
#[derive(Default)]
pub(in crate::catalog) struct Placed {
    pub(in crate::catalog) places: HashMap<PathBuf, Place>,
    /// Directories not placed: their catalog folder is elsewhere here.
    pub(in crate::catalog) conflicts: Vec<Conflict>,
    /// Directories not placed until one of their options is chosen.
    pub(in crate::catalog) ambiguous: Vec<Ambiguity>,
}
/// Where a folder on disk goes in the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::catalog) enum Place {
    /// A folder of an existing root, by logical path.
    Folder(RootId, String),
    /// A folder of the root the added folder becomes, by logical path.
    New(String),
}

impl Catalog {
    #[cfg(any(test, feature = "test-support"))]
    pub fn add_folder(&mut self, folder: &Path) -> Result<usize> {
        Ok(self.add_folder_with(folder, &Default::default())?.0)
    }
    /// Lightroom's Remove for a folder: `folders` leave the catalog with
    /// their photos (and those photos' virtual copies), edits and metadata,
    /// and a root left without folders goes too. Nothing on disk is touched.
    /// Returns the photos removed; their ids may be given to new photos.
    pub fn remove_folders(&mut self, folders: &[FolderId]) -> Result<Vec<PhotoId>> {
        self.db.write(|w| remove_folders(w, folders))
    }
    /// Removes root `root` from the catalog when it has no folder left (a
    /// folder put in the Trash or moved to another root took its last one);
    /// nothing on disk is touched. Returns whether it was removed.
    pub fn remove_empty_root(&mut self, root: RootId) -> Result<bool> {
        self.db.write(|w| {
            let left: Option<FolderId> =
                w.read_optional(sql!("SELECT id FROM folders WHERE root=?"), &[&root])?;
            if left.is_some() {
                return Ok(false);
            }
            w.execute(sql!("DELETE FROM folder_locations WHERE root=?"), &[&root])?;
            Ok(w.execute(sql!("DELETE FROM roots WHERE id=?"), &[&root])? == 1)
        })
    }
    /// `import_folder` for callers that can't ask which location a folder
    /// belongs to: they get an error instead.
    pub fn add_folder_with(
        &mut self,
        folder: &Path,
        defaults: &super::NewPhotoDefaults,
    ) -> Result<(usize, super::SidecarReport)> {
        let added = self.import_folder(folder, defaults, &[])?;
        ensure!(
            added.ambiguous.is_empty(),
            "Choose which folder of the catalog {} is",
            added.ambiguous[0].directories[0].display()
        );
        Ok((added.added, added.report))
    }
    /// Adds a folder's new photos with the metadata of their XMP sidecars and
    /// of the XMP inside JPEGs and TIFFs, then the default Creator and
    /// Copyright where neither the file nor its sidecar has one. Photos
    /// already in the catalog are left alone; Read Metadata from Files reads
    /// theirs.
    ///
    /// Each folder found on disk, the one chosen and every one below it, is
    /// matched with this computer's locations (see `locations`): it joins the
    /// catalog folder of the most specific location containing it, if that
    /// folder is found there; a folder no location contains goes under a new
    /// root, the folder chosen. `choices` settle the ambiguous ones of an
    /// earlier call.
    pub fn import_folder(
        &mut self,
        folder: &Path,
        defaults: &super::NewPhotoDefaults,
        choices: &[Choice],
    ) -> Result<Added> {
        let folder = folder.canonicalize()?;
        let files = walk(&folder)?;
        let placed = self.place(&files, &folder, choices)?;
        let mut result = Added {
            conflicts: placed.conflicts,
            ..Default::default()
        };
        if !placed.ambiguous.is_empty() {
            result.ambiguous = placed.ambiguous;
            return Ok(result);
        }
        (result.added, result.report) =
            self.insert(&folder, files, &placed.places, defaults, ImportKind::Add)?;
        Ok(result)
    }
    /// Where the directories of `files`, found below the `chosen` folder, go
    /// in the catalog (see `import_folder`).
    pub(in crate::catalog) fn place(
        &self,
        files: &[PathBuf],
        chosen: &Path,
        choices: &[Choice],
    ) -> Result<Placed> {
        let mut placed = Placed::default();
        let mut ambiguous: BTreeMap<Vec<FolderLocation>, Vec<PathBuf>> = BTreeMap::new();
        let matcher = Matcher::new(self)?;
        let directories: BTreeSet<_> = files.iter().filter_map(|f| f.parent()).collect();
        for directory in directories {
            match matcher.place(directory, chosen, choices) {
                Matched::Place(place) => {
                    placed.places.insert(directory.to_path_buf(), place);
                }
                Matched::Conflict(conflict) => placed.conflicts.push(conflict),
                Matched::Ambiguous(options) => ambiguous
                    .entry(options)
                    .or_default()
                    .push(directory.to_path_buf()),
            }
        }
        placed.ambiguous = ambiguous
            .into_iter()
            .map(|(options, directories)| Ambiguity {
                directories,
                options,
            })
            .collect();
        Ok(placed)
    }
    /// Adds the photos of `files` the catalog doesn't have, each with what
    /// its files hold, in one transaction, their folders where `places` says
    /// (a new root being the `chosen` folder). Files in a directory `places`
    /// has no place for are left out. Returns how many were added, and what
    /// reading their sidecars found.
    pub(in crate::catalog) fn insert(
        &mut self,
        chosen: &Path,
        files: Vec<PathBuf>,
        places: &HashMap<PathBuf, Place>,
        defaults: &super::NewPhotoDefaults,
        kind: ImportKind,
    ) -> Result<(usize, super::SidecarReport)> {
        // Photos already in the catalog are left before their files are read.
        let files: Vec<(PathBuf, &Place)> = self.db.snapshot(|s| {
            let mut new = Vec::new();
            for file in files {
                let Some(place) = file.parent().and_then(|d| places.get(d)) else {
                    continue;
                };
                if let Place::Folder(root, logical) = place
                    && known(s, *root, logical, &file)?
                {
                    continue;
                }
                new.push((file, place));
            }
            Ok(new)
        })?;
        // Everything each photo brings is read before the transaction, so
        // that it is written with the photo or not at all.
        let photos = self.read_new_photos(files, defaults);
        let stores = stores(&photos);
        self.write_new_photos(stores, |w, writer| {
            // The newest batch is Previous Import, empty when nothing new
            // was found.
            let batch = new_batch(w, kind)?;
            let mut new_root = None;
            let mut folders: HashMap<(RootId, String), FolderId> = HashMap::new();
            let mut added = 0;
            let mut report = super::SidecarReport::default();
            for photo in photos {
                let (root, logical) = match photo.place {
                    Place::Folder(root, logical) => (*root, logical.clone()),
                    Place::New(logical) => {
                        let root = match new_root {
                            Some(root) => root,
                            None => *new_root.insert(w.insert_returning_id(
                                sql!("INSERT INTO roots(original_path) VALUES(?) RETURNING id"),
                                &[&chosen.to_string_lossy()],
                            )?),
                        };
                        (root, logical.clone())
                    }
                };
                // Added since the files were read.
                if known(w, root, &logical, &photo.file)? {
                    continue;
                }
                let key = (root, logical);
                let fid = match folders.get(&key) {
                    Some(id) => *id,
                    None => {
                        let id = folder_id(w, key.0, &key.1)?;
                        folders.insert(key, id);
                        id
                    }
                };
                let (_, read) = writer.insert(w, photo, fid, batch)?;
                report.add(read);
                added += 1;
            }
            // Its bursts are stacked once their files are read; asked with
            // the batch, so it is there with the photos or not at all.
            if defaults.stack_bursts() && added > 0 {
                w.execute(
                    sql!("INSERT INTO import_burst_stacks(batch, seconds) VALUES(?, ?)"),
                    &[&batch, &IMPORT_BURST_SECONDS],
                )?;
            }
            Ok((added, report))
        })
    }
    /// What each of `files` brings (see [`NewPhoto`]), read from its files,
    /// with its legacy edit checked for the catalog. The files are read side
    /// by side, as one after another leaves a slow disk or a network share
    /// waiting on each; the checks against the catalog follow in order.
    /// `place` travels with each photo for its caller.
    ///
    /// Each folder's names are read once, by the first reader to come to one
    /// of its photos, so that whether a sidecar exists beside each photo needn't
    /// be asked of the file system name by name (`sidecar::Listing`).
    pub(in crate::catalog) fn read_new_photos<P: Send>(
        &self,
        files: Vec<(PathBuf, P)>,
        defaults: &super::NewPhotoDefaults,
    ) -> Vec<NewPhoto<P>> {
        let readers = readers(files.first().and_then(|(file, _)| file.parent()));
        let listings: HashMap<PathBuf, OnceLock<Option<super::sidecar::Listing>>> = files
            .iter()
            .filter_map(|(file, _)| file.parent())
            .map(|folder| (folder.to_path_buf(), OnceLock::new()))
            .collect();
        side_by_side(files, readers, |(file, place)| {
            let listing = file.parent().and_then(|folder| {
                listings
                    .get(folder)?
                    .get_or_init(|| super::sidecar::Listing::read(folder))
                    .as_ref()
            });
            read_new_photo(file, place, defaults, listing)
        })
        .into_iter()
        .map(|read| self.new_photo(read))
        .collect()
    }
    /// Runs `write` in one transaction, with a [`PhotoWriter`] for the
    /// photos it inserts, then tells the edits it stored that they were
    /// committed. `stores` (see [`stores`]) raises the catalog to the format
    /// those edits need in the same transaction.
    pub(in crate::catalog) fn write_new_photos<R>(
        &mut self,
        stores: super::format_upgrade::Stores,
        write: impl FnOnce(&mut Write<'_>, &mut PhotoWriter) -> Result<R>,
    ) -> Result<R> {
        let mut writer = PhotoWriter {
            edited_at: rawmakase_model::time::now_text(),
            edits: Vec::new(),
        };
        let written = match stores {
            super::format_upgrade::Stores::Recipes => self.db.write(|w| write(w, &mut writer))?,
            _ => self.write_recipes(stores, |w| write(w, &mut writer))?,
        };
        writer.edits.iter().for_each(|change| change.committed());
        Ok(written)
    }
    /// Records capture times read from the photos' files, in one transaction.
    /// Only empty dates are filled, never one Lightroom or the user set, and a
    /// photo's virtual copies get its date too.
    pub fn fill_capture_times(&mut self, times: &[(PhotoId, String)]) -> Result<()> {
        self.db.write(|w| fill_capture_times(w, times))
    }
    /// The master photos whose import facts still want the capture time
    /// their file's EXIF gives, whatever date the photo has (one from XMP or
    /// the user doesn't count).
    pub fn photos_to_read_exif_capture(&self) -> Result<Vec<PhotoId>> {
        self.db.read(
            sql!(
                "SELECT f.photo FROM photo_import_facts f JOIN photos p ON p.id = f.photo
                 WHERE f.exif_read = 0 AND p.master_id IS NULL ORDER BY f.photo"
            ),
            &[],
        )
    }
    /// The import facts of `photo`: its original name, EXIF capture time,
    /// size and whether its EXIF was read; `None` without a row.
    #[cfg(any(test, feature = "test-support"))]
    pub fn import_facts(&self, photo: PhotoId) -> Result<Option<StoredFacts>> {
        row! {
            struct Facts {
                original_name: String,
                exif_captured: Option<String>,
                size: Option<i64>,
                exif_read: i64,
            }
        }
        let facts: Option<Facts> = self.db.read_optional(
            sql!(
                "SELECT original_name, exif_captured, size, exif_read
                 FROM photo_import_facts WHERE photo=?"
            ),
            &[&photo],
        )?;
        Ok(facts.map(|f| (f.original_name, f.exif_captured, f.size, f.exif_read != 0)))
    }
    /// Records what reading photos' files for their capture times found, in
    /// one transaction: each date fills the photo's empty date as
    /// [`Self::fill_capture_times`] does, and every read (`None`: the file
    /// has no date) completes import facts not read yet. Only the facts
    /// row's EXIF capture time is written for a photo that has a date.
    pub fn record_capture_reads(&mut self, reads: &[(PhotoId, Option<String>)]) -> Result<()> {
        let dated: Vec<(PhotoId, String)> = reads
            .iter()
            .filter_map(|(id, time)| Some((*id, time.clone()?)))
            .collect();
        self.db.write(|w| {
            fill_capture_times(w, &dated)?;
            for (id, time) in reads {
                w.execute(
                    sql!(
                        "UPDATE photo_import_facts SET exif_captured=?1, exif_read=1
                         WHERE photo=?2 AND exif_read=0"
                    ),
                    &[time, id],
                )?;
            }
            Ok(())
        })
    }
    /// What a new photo brings, its legacy edit checked for the catalog.
    fn new_photo<P>(&self, read: FilesRead<P>) -> NewPhoto<P> {
        let edit = read.legacy.and_then(|legacy| {
            legacy
                .and_then(|legacy| self.check_legacy_edit(legacy))
                // A sidecar that no longer matches its photo stays unused on disk.
                .ok()
                .flatten()
        });
        NewPhoto {
            file: read.file,
            place: read.place,
            facts: read.facts,
            edit,
            metadata: read.metadata,
            report: read.report,
            defaults: read.defaults,
        }
    }
    /// The edit a photo got outside any catalog, in its photo.rawmakase.json
    /// sidecar, checked for the catalog. The sidecar stays on disk.
    fn check_legacy_edit(&self, legacy: Option<Legacy>) -> Result<Option<LegacyEdit>> {
        let Some((sidecar, bitmaps)) = legacy else {
            return Ok(None);
        };
        // The identity the sidecar was checked against as it was read, beside
        // its photo: a RAW replaced since then doesn't get this edit.
        let checked = self.check_new_edit(&sidecar.source, &sidecar.recipe, &sidecar.export)?;
        Ok(Some(LegacyEdit {
            bitmaps,
            checked,
            stores: super::format_upgrade::Stores::of([&sidecar.recipe]),
        }))
    }
}

/// What a new photo's files hold: the edit an older release saved beside it
/// (a RAW's only), its sidecar's and its own XMP metadata, and the defaults
/// where neither has a value. Nothing of the catalog is read.
fn read_new_photo<P>(
    file: PathBuf,
    place: P,
    defaults: &super::NewPhotoDefaults,
    listing: Option<&super::sidecar::Listing>,
) -> FilesRead<P> {
    // Its masks compressed as they are read, so a folder of them is never
    // held decompressed at once.
    let legacy = crate::storage::is_raw(&file).then(|| {
        let Some((sidecar, bitmaps)) = super::legacy_sidecar::import(&file, listing)? else {
            return Ok(None);
        };
        let bitmaps = bitmaps
            .iter()
            .map(|bitmap| Ok((bitmap.hash(), bitmap.compress()?)))
            .collect::<Result<_>>()?;
        Ok(Some((sidecar, bitmaps)))
    });
    let (metadata, report) = super::sidecar::read_file_listed(&file, defaults.raw_xmp(), listing);
    let defaults = defaults.for_file(&file, metadata.as_ref());
    let facts = ImportFacts::of_file(&file);
    FilesRead {
        file,
        place,
        facts,
        legacy,
        metadata,
        report,
        defaults,
    }
}
/// An older release's edit beside a photo, with its spots' and masks' bitmaps
/// (hash, compressed).
type Legacy = (super::legacy_sidecar::Sidecar, Vec<(String, Vec<u8>)>);
/// [`read_new_photo`]'s.
struct FilesRead<P> {
    file: PathBuf,
    place: P,
    facts: ImportFacts,
    legacy: Option<Result<Option<Legacy>>>,
    metadata: Option<crate::xmp::descriptive::Read>,
    report: super::SidecarReport,
    defaults: super::defaults::Fill,
}

/// Threads that read files side by side.
const READERS: usize = 4;
/// Threads that read files side by side on a network volume, where each
/// read mostly waits on round trips to the server rather than on the disk
/// or the processor.
const NETWORK_READERS: usize = 16;
/// How many threads read new photos' files from `folder`, the first's: on a
/// network volume [`NETWORK_READERS`], else one a core, up to [`READERS`].
/// Asking the volume can wait on a stalled mount, so this runs off the UI
/// thread, as reading the files does.
fn readers(folder: Option<&Path>) -> usize {
    let local = folder
        .is_none_or(|folder| rawmakase_fs::volume_facts(folder).map_or(true, |facts| facts.local));
    if local {
        local_readers()
    } else {
        NETWORK_READERS
    }
}
fn local_readers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, READERS)
}
/// `read` of each of `items`, in their order, on up to `readers` threads
/// side by side. A panic in one stops the others before their next item and
/// is the caller's, as it would be read in turn; items no thread could be
/// started for are read on the caller's.
fn side_by_side<T: Send, R: Send>(
    items: Vec<T>,
    readers: usize,
    read: impl Fn(T) -> R + Sync,
) -> Vec<R> {
    let threads = readers.max(1).min(items.len());
    if threads <= 1 {
        return items.into_iter().map(read).collect();
    }
    let items = std::sync::Mutex::new(items.into_iter().enumerate());
    let stopped = std::sync::atomic::AtomicBool::new(false);
    let next = || {
        if stopped.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next()
    };
    /// Stops the readers as one unwinds.
    struct Stop<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.0.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    let mut done: Vec<(usize, R)> = std::thread::scope(|scope| {
        let readers: Vec<_> = (0..threads)
            .filter_map(|_| {
                std::thread::Builder::new()
                    .spawn_scoped(scope, || {
                        let _stop = Stop(&stopped);
                        let mut done = Vec::new();
                        while let Some((index, item)) = next() {
                            done.push((index, read(item)));
                        }
                        done
                    })
                    .ok()
            })
            .collect();
        let mut done: Vec<_> = readers
            .into_iter()
            .flat_map(|reader| {
                reader
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect();
        // Left when no reader could be started.
        while let Some((index, item)) = next() {
            done.push((index, read(item)));
        }
        done
    });
    done.sort_unstable_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, read)| read).collect()
}

/// [`Catalog::fill_capture_times`] in `w`'s transaction.
fn fill_capture_times(w: &mut Write<'_>, times: &[(PhotoId, String)]) -> Result<()> {
    // Two statements, each on an index, rather than one OR that scans.
    for (id, captured) in times {
        w.execute(
            sql!("UPDATE photos SET captured=?1 WHERE id=?2 AND captured=''"),
            &[captured, id],
        )?;
        w.execute(
            sql!("UPDATE photos SET captured=?1 WHERE master_id=?2 AND captured=''"),
            &[captured, id],
        )?;
    }
    Ok(())
}

/// The catalog folder at `logical` in `root`, made if there is none, in
/// `w`'s transaction.
pub(in crate::catalog) fn folder_id(
    w: &mut Write<'_>,
    root: RootId,
    logical: &str,
) -> Result<FolderId> {
    let existing: Option<FolderId> = w.read_one(
        sql!(
            "SELECT min(f.id) FROM folders f JOIN folder_paths p ON p.folder=f.id
             WHERE f.root=? AND p.path=?"
        ),
        &[&root, &logical],
    )?;
    if let Some(id) = existing {
        return Ok(id);
    }
    // Older releases read the folder in this system's form.
    let native: PathBuf = super::locations::names(logical).collect();
    let id: FolderId = w.insert_returning_id(
        sql!("INSERT INTO folders(root,relative_path) VALUES(?,?) RETURNING id"),
        &[&root, &native.to_string_lossy()],
    )?;
    w.execute(
        sql!("INSERT INTO folder_paths(folder,path) VALUES(?,?)"),
        &[&id, &logical],
    )?;
    Ok(id)
}

/// The photo files below `folder`: RAWs, JPEGs, PNGs and TIFFs, not hidden
/// and not through a symbolic link.
pub(in crate::catalog) fn walk(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    walk_into(folder, &mut files)?;
    Ok(files)
}
fn walk_into(p: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(p)? {
        let e = entry?;
        let t = e.file_type()?;
        if t.is_symlink() || crate::storage::is_hidden(&e.path()) {
            continue;
        }
        if t.is_dir() {
            walk_into(&e.path(), files)?
        } else if crate::storage::is_photo(&e.path()) {
            files.push(e.path());
        }
    }
    Ok(())
}

/// Whether the catalog has the photo at `file` in the folder at `logical`
/// of `root`. A photo is the same one by root, folder and name; its virtual
/// copies share them and stay as they are.
pub(in crate::catalog) fn known(
    r: &impl Reads,
    root: RootId,
    logical: &str,
    file: &Path,
) -> Result<bool> {
    let filename = file.file_name().unwrap_or_default().to_string_lossy();
    let found: Option<i64> = r.read_optional(
        sql!(
            "SELECT 1 FROM photos WHERE filename=? AND folder IN
             (SELECT f.id FROM folders f JOIN folder_paths p ON p.folder=f.id
              WHERE f.root=? AND p.path=?)"
        ),
        &[&filename, &root, &logical],
    )?;
    Ok(found.is_some())
}

/// `result`, unless the database failed: what one photo's files hold that
/// can't be stored leaves the photo without it, but a failing database
/// keeps nothing.
fn refused<T>(result: Result<T>) -> Result<Result<T>> {
    match result {
        Err(e) if is_storage_error(&e) => Err(e),
        other => Ok(other),
    }
}

/// A photo to add and what its files hold, read before it is written, with
/// its caller's `place` for it.
pub(in crate::catalog) struct NewPhoto<P> {
    pub(in crate::catalog) file: PathBuf,
    pub(in crate::catalog) place: P,
    /// What it is as it enters the catalog (see [`ImportFacts`]).
    pub(in crate::catalog) facts: ImportFacts,
    edit: Option<LegacyEdit>,
    metadata: Option<crate::xmp::descriptive::Read>,
    /// Its sidecars that could not be read, or were ignored.
    report: super::SidecarReport,
    defaults: super::defaults::Fill,
}

/// A photo's `photo_import_facts` row as stored: its original name, EXIF
/// capture time, size and whether its EXIF was read.
#[cfg(any(test, feature = "test-support"))]
pub(in crate::catalog) type StoredFacts = (String, Option<String>, Option<i64>, bool);

/// What a photo is as it enters the catalog, kept in `photo_import_facts`:
/// its file's name then, its size, and the capture time its file's EXIF
/// gives, once read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::catalog) struct ImportFacts {
    pub(in crate::catalog) original_name: String,
    /// `None` when the file's size could not be read.
    pub(in crate::catalog) size: Option<u64>,
    /// The EXIF capture time once read (`Some(None)`: the file has none);
    /// `None` leaves it to the background read of capture times.
    pub(in crate::catalog) exif_captured: Option<Option<String>>,
}
impl ImportFacts {
    /// Those Add Folder and Synchronize know of the file at `file`: its
    /// name and size; its EXIF is read later, in the background.
    fn of_file(file: &Path) -> Self {
        Self {
            original_name: file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            size: std::fs::metadata(file).ok().map(|m| m.len()),
            exif_captured: None,
        }
    }
    /// Records them for new photo `photo`, in `w`'s transaction.
    pub(in crate::catalog) fn insert(&self, w: &mut Write<'_>, photo: PhotoId) -> Result<()> {
        let size = self.size.map(|s| s as i64);
        let (read, captured) = match &self.exif_captured {
            Some(captured) => (1i64, captured.as_deref()),
            None => (0, None),
        };
        w.execute(
            sql!(
                "INSERT INTO photo_import_facts(photo, original_name, exif_captured, size, exif_read)
                 VALUES(?, ?, ?, ?, ?)"
            ),
            &[&photo, &self.original_name, &captured, &size, &read],
        )?;
        Ok(())
    }
}

/// The catalog format a write of `photos` needs for the edits they bring.
pub(in crate::catalog) fn stores<P>(photos: &[NewPhoto<P>]) -> super::format_upgrade::Stores {
    // An edit naming a Lensfun lens, or with AI Denoise, raises the catalog
    // to the format for it.
    photos
        .iter()
        .filter_map(|p| p.edit.as_ref().map(|e| e.stores))
        .max_by_key(|s| s.version())
        .unwrap_or(super::format_upgrade::Stores::Recipes)
}

/// Inserts photos read by [`Catalog::read_new_photos`] in the transaction of
/// [`Catalog::write_new_photos`], keeping the edits it stores to be told of
/// the commit.
pub(in crate::catalog) struct PhotoWriter {
    edited_at: String,
    edits: Vec<super::edit_rows::CheckedChange<'static>>,
}
impl PhotoWriter {
    /// Inserts `photo` into catalog folder `folder` as one of import batch
    /// `batch`, with its legacy edit, its files' metadata and, where those
    /// were all read, the defaults where they have no value. The caller
    /// checked that the folder has no photo of that name. Returns the new
    /// photo and what reading its sidecars found, including metadata that
    /// could not be stored.
    pub(in crate::catalog) fn insert<P>(
        &mut self,
        w: &mut Write<'_>,
        photo: NewPhoto<P>,
        folder: FolderId,
        batch: i64,
    ) -> Result<(PhotoId, super::SidecarReport)> {
        let file = photo.file;
        let filename = file.file_name().unwrap().to_string_lossy();
        let format = file
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_uppercase();
        let id: PhotoId = w.insert_returning_id(
            sql!(
                "INSERT INTO photos(folder,filename,original_path,format) VALUES(?,?,?,?)
                 RETURNING id"
            ),
            &[&folder, &filename, &file.to_string_lossy(), &format],
        )?;
        w.execute(
            sql!("INSERT INTO photo_imports(photo, batch) VALUES(?, ?)"),
            &[&id, &batch],
        )?;
        photo.facts.insert(w, id)?;
        if let Some(edit) = photo.edit {
            // An edit an older release saved that can't be kept stays
            // unused on disk.
            let written = w.savepoint(|w| {
                for (hash, data) in &edit.bitmaps {
                    super::edits::insert_bitmap(w, hash, data)?;
                }
                let change = edit.checked.change(id);
                super::edit_rows::write_edit(w, &change, &self.edited_at)?;
                Ok(change)
            });
            self.edits.extend(refused(written)?.ok());
        }
        let mut report = photo.report;
        let mut readable = report.unreadable.is_empty();
        if let Some(read) = &photo.metadata
            && let Err(e) = refused(
                w.savepoint(|w| super::sidecar::apply(w, id, read, super::Merge::FillEmpty)),
            )?
        {
            report.unreadable.push((file.clone(), format!("{e:#}")));
            readable = false;
        }
        // A photo whose metadata couldn't be read may have its own: no
        // default goes in its place.
        if readable {
            super::defaults::fill(w, id, &photo.defaults)?;
        }
        Ok((id, report))
    }
}

/// An edit saved beside a photo by a release before the catalog, with the
/// bitmaps (hash, compressed) its spots and masks refer to.
struct LegacyEdit {
    bitmaps: Vec<(String, Vec<u8>)>,
    checked: super::edit_rows::CheckedNew,
    /// What its edit stores, for the catalog format it needs (`format_upgrade::Stores`).
    stores: super::format_upgrade::Stores,
}

/// This computer's locations, for matching folders found on disk.
pub(in crate::catalog) struct Matcher {
    /// Every location: where each root was added, its own and every
    /// folder's on this computer; with its path as the disk has it.
    locations: Vec<(FolderLocation, PathBuf)>,
    roots: HashMap<RootId, Root>,
}
/// Where a root was added and its rows on this computer, as stored and as
/// the disk has them.
struct Root {
    original: String,
    rows: Vec<(String, PathBuf)>,
    original_on_disk: String,
    rows_on_disk: Vec<(String, PathBuf)>,
}
row! {
    struct LocationRow {
        root: RootId,
        relative: String,
        path: String,
    }
}
row! {
    struct RootRow {
        id: RootId,
        original: String,
    }
}
/// What a folder found on disk is in the catalog.
pub(in crate::catalog) enum Matched {
    Place(Place),
    Conflict(Conflict),
    Ambiguous(Vec<FolderLocation>),
}
impl Matcher {
    pub(in crate::catalog) fn new(catalog: &Catalog) -> Result<Self> {
        Self::read(&catalog.db, &catalog.computer.id)
    }
    /// The locations of `computer`, read in `db`'s transaction.
    pub(in crate::catalog) fn read(db: &impl Reads, computer: &str) -> Result<Self> {
        let mut rows: HashMap<RootId, Vec<(String, PathBuf)>> = HashMap::new();
        let located: Vec<(RootId, String, String)> = db
            .read::<LocationRow>(
                sql!(
                    "SELECT root, relative_path, path FROM folder_locations WHERE computer=?
                     ORDER BY root, relative_path"
                ),
                &[&computer],
            )?
            .into_iter()
            .map(|l| (l.root, l.relative, l.path))
            .collect();
        for (root, relative, path) in located {
            rows.entry(root)
                .or_default()
                .push((relative, PathBuf::from(path)));
        }
        let root_rows: Vec<RootRow> =
            db.read(sql!("SELECT id, original_path FROM roots ORDER BY id"), &[])?;
        // Paths are compared as the disk has them (no symlinks, no trailing
        // separators); one that isn't there stays as it is.
        let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut locations = Vec::new();
        let mut roots = HashMap::new();
        for RootRow { id: root, original } in root_rows {
            let own = rows.remove(&root).unwrap_or_default();
            // Where it was added counts even when it is located elsewhere
            // here: what is left there is then a stale copy, reported by the
            // forward check, never a second root.
            let path = PathBuf::from(&original);
            locations.push((
                FolderLocation {
                    root,
                    relative: String::new(),
                    path: path.clone(),
                },
                real(&path),
            ));
            for (relative, path) in &own {
                locations.push((
                    FolderLocation {
                        root,
                        relative: relative.clone(),
                        path: path.clone(),
                    },
                    real(path),
                ));
            }
            let rows_on_disk = own
                .iter()
                .map(|(relative, path)| (relative.clone(), real(path)))
                .collect();
            let original_on_disk = real(Path::new(&original)).to_string_lossy().into_owned();
            roots.insert(
                root,
                Root {
                    original,
                    rows: own,
                    original_on_disk,
                    rows_on_disk,
                },
            );
        }
        Ok(Self { locations, roots })
    }
    /// Whether a root or folder location is at `directory` or below it, as
    /// the disk has them: then it can't become a root of its own.
    pub(in crate::catalog) fn holds_below(&self, directory: &Path) -> bool {
        self.locations
            .iter()
            .any(|(_, on_disk)| on_disk.starts_with(directory))
    }
    /// Where `directory`, found below the `chosen` folder, goes.
    pub(in crate::catalog) fn place(
        &self,
        directory: &Path,
        chosen: &Path,
        choices: &[Choice],
    ) -> Matched {
        // The directory as the disk spells it, as the locations are compared.
        let on_disk_dir = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());
        // (specificity, location, logical path, whether it resolves back here)
        let mut candidates = Vec::new();
        for (location, on_disk) in &self.locations {
            let root = &self.roots[&location.root];
            // A directory is below a location as the disk spells both (Add
            // Folder's, canonicalized), or as the catalog stores the location
            // (Synchronize's and Find on Disk's, built from it). The two
            // differ on Windows, where only the disk's has the `\\?\` prefix,
            // and through symbolic links (issue #941). The names below the
            // location stay as the caller has them where they can, as they
            // are the catalog folder's; a location only the disk's spelling
            // of the directory is below still counts, so the most specific
            // one wins.
            let (rest, original, rows, here) = if let Ok(rest) = directory.strip_prefix(on_disk) {
                (rest, &root.original_on_disk, &root.rows_on_disk, directory)
            } else if let Ok(rest) = directory.strip_prefix(&location.path) {
                (rest, &root.original, &root.rows, directory)
            } else if let Ok(rest) = on_disk_dir.strip_prefix(on_disk) {
                (
                    rest,
                    &root.original_on_disk,
                    &root.rows_on_disk,
                    on_disk_dir.as_path(),
                )
            } else {
                continue;
            };
            let logical = join(&location.relative, &logical_from_os(rest));
            // The folder it would be must be found right here, not hidden by a
            // more specific location elsewhere.
            let found =
                resolve_in(original, rows, &logical, cfg!(windows)).as_deref() == Some(here);
            candidates.push((on_disk.components().count(), location, logical, found));
        }
        if candidates.is_empty() {
            let rest = directory.strip_prefix(chosen).unwrap_or(Path::new(""));
            return Matched::Place(Place::New(logical_from_os(rest)));
        }
        let valid: Vec<_> = candidates.iter().filter(|c| c.3).collect();
        let Some(best) = valid.iter().map(|c| c.0).max() else {
            let (_, location, logical, _) = candidates.iter().max_by_key(|c| c.0).unwrap();
            let root = &self.roots[&location.root];
            return Matched::Conflict(Conflict {
                directory: directory.to_path_buf(),
                root: location.root,
                relative: logical.clone(),
                located: resolve_in(&root.original, &root.rows, logical, cfg!(windows))
                    .unwrap_or_default(),
            });
        };
        let mut closest: Vec<_> = valid.into_iter().filter(|c| c.0 == best).collect();
        closest.dedup_by(|a, b| a.1.root == b.1.root && a.2 == b.2);
        let distinct: BTreeSet<_> = closest.iter().map(|c| (c.1.root, &c.2)).collect();
        if distinct.len() == 1 {
            let (_, location, logical, _) = closest[0];
            return Matched::Place(Place::Folder(location.root, logical.clone()));
        }
        let mut options: Vec<FolderLocation> = closest.iter().map(|c| c.1.clone()).collect();
        options.sort();
        options.dedup();
        // Only the answer to this very question settles it.
        let answer = choices
            .iter()
            .find(|c| c.options == options)
            .and_then(|choice| {
                closest.iter().find(|c| {
                    c.1.root == choice.chosen.root && c.1.relative == choice.chosen.relative
                })
            });
        if let Some((_, location, logical, _)) = answer {
            return Matched::Place(Place::Folder(location.root, logical.clone()));
        }
        Matched::Ambiguous(options)
    }
}

/// Removes catalog folders `folders` in `w`'s transaction, with their
/// photos and those photos' virtual copies (wherever they are), edits and
/// metadata; their roots stay, even with no folder left. Nothing on disk
/// is touched. Returns the photos removed, sorted. Ungated: its callers
/// decide that the rows may go (Remove Folder from Catalog, or a folder
/// proven in the Trash).
/// [`Catalog::remove_folders`] in `w`'s transaction.
pub(in crate::catalog) fn remove_folders(
    w: &mut Write<'_>,
    folders: &[FolderId],
) -> Result<Vec<PhotoId>> {
    let mut roots = BTreeSet::new();
    for folder in folders {
        let root: Option<RootId> =
            w.read_optional(sql!("SELECT root FROM folders WHERE id=?"), &[folder])?;
        roots.extend(root);
    }
    let photos = remove_folder_rows(w, folders)?;
    for root in &roots {
        let left: Option<FolderId> =
            w.read_optional(sql!("SELECT id FROM folders WHERE root=?"), &[root])?;
        if left.is_none() {
            w.execute(sql!("DELETE FROM folder_locations WHERE root=?"), &[root])?;
            w.execute(sql!("DELETE FROM roots WHERE id=?"), &[root])?;
        }
    }
    Ok(photos)
}
pub(in crate::catalog) fn remove_folder_rows(
    w: &mut Write<'_>,
    folders: &[FolderId],
) -> Result<Vec<PhotoId>> {
    let photos = folder_rows(w, folders)?;
    for photo in &photos {
        super::copies::delete_photo(w, *photo)?;
    }
    for folder in folders {
        super::locations::forget_folder_locations(w, *folder)?;
        w.execute(
            sql!("DELETE FROM folder_mappings WHERE folder=?"),
            &[folder],
        )?;
        w.execute(sql!("DELETE FROM folder_paths WHERE folder=?"), &[folder])?;
        w.execute(sql!("DELETE FROM folders WHERE id=?"), &[folder])?;
    }
    Ok(photos)
}

/// The photos [`remove_folder_rows`] removes with `folders`, sorted: those
/// in them and those photos' virtual copies, wherever they are.
pub(in crate::catalog) fn folder_rows(
    db: &impl Reads,
    folders: &[FolderId],
) -> Result<Vec<PhotoId>> {
    let mut photos = BTreeSet::new();
    for folder in folders {
        let ids: Vec<PhotoId> = db.read(sql!("SELECT id FROM photos WHERE folder=?"), &[folder])?;
        photos.extend(ids);
    }
    // A copy is kept with its master's folder; one found elsewhere
    // still can't outlive its master.
    for master in photos.clone() {
        let copies: Vec<PhotoId> =
            db.read(sql!("SELECT id FROM photos WHERE master_id=?"), &[&master])?;
        photos.extend(copies);
    }
    Ok(photos.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn new_photos_files_are_read_side_by_side_and_kept_in_order() {
        for readers in [super::local_readers(), super::NETWORK_READERS] {
            reads_side_by_side(readers);
        }
        assert!(super::side_by_side(Vec::<u32>::new(), 4, |n| n).is_empty());
    }

    fn reads_side_by_side(readers: usize) {
        let running = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let read = super::side_by_side((0..40).collect(), readers, |n: u32| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            // Waits a little for the others, as a slow disk would.
            let started = Instant::now();
            while running.load(Ordering::SeqCst) < readers
                && started.elapsed() < Duration::from_millis(200)
            {
                std::thread::yield_now();
            }
            running.fetch_sub(1, Ordering::SeqCst);
            n * 2
        });
        assert_eq!(read, (0..40).map(|n| n * 2).collect::<Vec<_>>());
        assert_eq!(most.load(Ordering::SeqCst), readers);
    }

    #[test]
    fn a_panicking_read_stops_the_others() {
        let reads = AtomicUsize::new(0);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::side_by_side((0..1000).collect(), super::local_readers(), |n: u32| {
                reads.fetch_add(1, Ordering::SeqCst);
                assert_ne!(n, 3, "unreadable");
                std::thread::sleep(Duration::from_millis(10));
            })
        }));
        assert!(panicked.is_err());
        // The others stop once it unwinds, after the panic hook (a backtrace
        // can take a while), long before the 2.5 s the rest would take.
        assert!(reads.load(Ordering::SeqCst) < 1000, "{reads:?}");
    }
}
