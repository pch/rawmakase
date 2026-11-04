//! Planning: the user's choice expanded into items, each family resolved on
//! this computer and checked against the refusals and the conflict and
//! companion rules, from one snapshot of the catalog. Every file is
//! captured once here (its path, file object and `Identity`), and the same
//! values are what execution checks, journals and acts on. Nothing is
//! changed.
use super::super::db::{Reads, sql};
use super::super::folder_sync::Located;
use super::super::locations::{join, logical_anchored, logical_location};
use super::super::sidecar::caseless;
use super::super::value::row;
use super::super::{Catalog, FolderId, PhotoId, RootId};
use super::companions::{Companions, Listing};
use super::conflicts::{Destination, Names};
use super::disk::Disk;
use super::folder::{FolderState, NewFolder, folder_name_problem};
use super::journal::{Kind, NewMember};
use super::preconditions::Preconditions;
use super::tree::TreeMove;
use super::{BatchId, FileOp, FileOpError, Plan, Refusal, Skip, Skipped, Summary};
use crate::fsops::{self, Containment, EntryKind, FileId, FsError, VolumeId};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// One item of a plan: a family (or a lone virtual copy, for Remove from
/// Catalog) and the files it touches.
#[derive(Clone, Debug)]
pub(super) struct Item {
    pub preconditions: Preconditions,
    /// The photo's file first, then its companions; none when only rows
    /// change.
    pub members: Vec<NewMember>,
    /// Companions another file reads too, which stay.
    pub shared: Vec<PathBuf>,
    /// The photo's companions a rename leaves as they are: they have the
    /// name it gives them already.
    pub kept: Vec<PathBuf>,
    /// How the destination folder compares names.
    pub rules: Names,
    /// How its files are copied, when the destination is on another volume.
    pub copy: Option<CopyTo>,
}

/// An item whose files are copied to another volume, as planned.
#[derive(Clone, Debug)]
pub(super) struct CopyTo {
    /// The destination's volume.
    pub volume: VolumeId,
    /// Each member's partial copy, by ordinal: hidden in the destination
    /// folder, by a name unique to the run, the item and the member.
    pub temps: Vec<PathBuf>,
    /// The bytes it copies.
    pub bytes: u64,
    /// Its originals stay where they are once copied
    /// ([`super::proof::no_trash`]).
    pub originals_stay: bool,
}

/// What a move to another volume must leave free there, beyond what it
/// copies: room for the folders' own records.
pub(super) const SPARE: u64 = 32 << 20;

/// The name of member `ordinal` of item `item`'s partial copy, for its file
/// named `name`, in run `batch`: hidden (so Synchronize Folder and Add
/// Folder pass it by), and with the destination name's start for whoever
/// finds one left.
pub(super) fn temp_name(name: &OsStr, batch: &BatchId, item: usize, ordinal: usize) -> OsString {
    let name = name.to_string_lossy();
    let mut end = name.len().min(64);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        ".{}.{}-{item}-{ordinal}.rawmakase-part",
        &name[..end],
        batch.0
    )
    .into()
}

row! {
    pub(super) struct PhotoRow {
        id: PhotoId,
        folder: FolderId,
        filename: String,
        master: Option<PhotoId>,
        edited: bool,
    }
}

/// A chosen family, before its files are looked at.
pub(super) struct Family {
    pub master: PhotoId,
    pub preconditions: Preconditions,
}

/// Plans `op` in `catalog` on this computer. Reads the catalog in one
/// snapshot and the files after it; changes nothing.
pub fn plan(catalog: &Catalog, op: FileOp) -> Result<Plan, FileOpError> {
    plan_with(catalog, op, &super::disk::Real)
}

/// [`plan`] with the volumes and destination folders as `disk` sees them.
pub(super) fn plan_with(
    catalog: &Catalog,
    op: FileOp,
    disk: &dyn Disk,
) -> Result<Plan, FileOpError> {
    let batch = new_batch()?;
    let mut stems = HashMap::new();
    let (kind, chosen, to, create) = match op {
        FileOp::RemoveFromCatalog { photos } => (Kind::RemoveFromCatalog, photos, None, None),
        FileOp::MovePhotos { photos, root, to } => {
            (Kind::MovePhotos, photos, Some((root, to)), None)
        }
        FileOp::TrashPhotos { photos } => (Kind::TrashPhotos, photos, None, None),
        FileOp::CreateFolder {
            root,
            parent,
            name,
            then_move,
        } => (
            Kind::CreateFolder,
            then_move,
            None,
            Some((root, parent, name)),
        ),
        FileOp::RenamePhotos { renames } => {
            let mut ids = Vec::new();
            for (photo, stem) in renames {
                ids.push(photo);
                // The same photo given two names has neither.
                let entry = stems.entry(photo).or_insert_with(|| Some(stem.clone()));
                if entry.as_ref() != Some(&stem) {
                    *entry = None;
                }
            }
            (Kind::RenamePhotos, ids, None, None)
        }
        FileOp::RenameFolder { root, folder, name } => {
            let tree = super::tree::rename(catalog, disk, root, &folder, &name)?;
            return tree_plan(catalog, Kind::RenameFolder, tree, batch);
        }
        FileOp::MoveFolder {
            root,
            folder,
            to_root,
            parent,
        } => {
            let tree = super::tree::relocate(catalog, disk, root, &folder, to_root, &parent)?;
            return tree_plan(catalog, Kind::MoveFolder, tree, batch);
        }
        FileOp::MoveRoot {
            root,
            to_root,
            parent,
        } => {
            let tree = super::tree::nest(catalog, disk, root, to_root, &parent)?;
            return tree_plan(catalog, Kind::MoveRoot, tree, batch);
        }
        // Its drive's Trash is asked first.
        FileOp::TrashFolder { .. } => {
            return Err(FileOpError::Unsupported(
                "A folder is put in the Trash only by a plan that knows the Trash.",
            ));
        }
    };
    let chosen: BTreeSet<PhotoId> = chosen.into_iter().collect();
    let mut skipped = Vec::new();
    let (photos, folders, families, computers) = catalog.db.snapshot(|s| {
        let photos: Vec<PhotoRow> = s.read(
            sql!(
                "SELECT id, folder, filename, master_id, recipe IS NOT NULL FROM photos ORDER BY id"
            ),
            &[],
        )?;
        let folders = catalog.locate_folders(s)?;
        let families = expand(s, kind, &chosen, &photos, &mut skipped)?;
        let computers: i64 = s.read_one(sql!("SELECT count(*) FROM computers"), &[])?;
        Ok((photos, folders, families, computers))
    })?;
    let by_id: HashMap<PhotoId, &PhotoRow> = photos.iter().map(|p| (p.id, p)).collect();
    let mut made = None;
    let mut across = None;
    let items = match kind {
        Kind::RemoveFromCatalog => families
            .into_iter()
            .map(|f| Item {
                preconditions: f.preconditions,
                members: Vec::new(),
                shared: Vec::new(),
                kept: Vec::new(),
                rules: UNKNOWN,
                copy: None,
            })
            .collect(),
        Kind::MovePhotos | Kind::TrashPhotos | Kind::CreateFolder => {
            let mut files = Files::new(catalog, &folders, disk);
            let destination = match (to, create) {
                (Some((root, to)), _) => Some(files.destination(root, &to)?),
                (None, Some((root, parent, name))) => {
                    let (new, target) = files.new_folder(root, &parent, &name)?;
                    made = Some(new);
                    Some(target)
                }
                (None, None) => None,
            };
            let mut items = Vec::new();
            for f in families {
                match files.family(f, &by_id, destination.as_ref()) {
                    Ok(item) => items.push(item),
                    Err((photo, reason)) => skipped.push(Skipped { photo, reason }),
                }
            }
            refuse_twice(&mut items, &mut skipped);
            if let Some(destination) = &destination {
                arrivals(disk, destination, &mut items, &mut skipped);
                across = copies(&batch, destination, &mut items)?;
            }
            items
        }
        Kind::RenamePhotos => {
            let mut files = Files::new(catalog, &folders, disk);
            super::rename::items(&mut files, families, &by_id, &stems, &mut skipped)?
        }
        Kind::RenameFolder | Kind::MoveFolder | Kind::MoveRoot | Kind::TrashFolder => Vec::new(),
    };
    let mut summary = summarize(&items, &by_id, computers > 1);
    if kind == Kind::RenamePhotos {
        summary.renames = items.iter().filter_map(super::rename::names).collect();
    }
    summary.folder = made.as_ref().map(|m| m.state.name.clone());
    summary.across = across;
    skipped.sort_by_key(|s| s.photo);
    Ok(Plan {
        batch,
        location: catalog.location.clone(),
        computer: catalog.computer.id.clone(),
        kind,
        create: made,
        tree: None,
        trash_tree: None,
        items,
        skipped,
        summary,
    })
}

/// [`plan_with`] with the `trash` the plan would run with, which a folder
/// Trash asks whether its drive has one.
pub(super) fn plan_trash_with(
    catalog: &Catalog,
    op: FileOp,
    disk: &dyn Disk,
    trash: &dyn super::Trash,
) -> Result<Plan, FileOpError> {
    match op {
        FileOp::TrashFolder { root, folder } => {
            super::tree_trash::plan(catalog, disk, trash, root, &folder, new_batch()?)
        }
        op => plan_with(catalog, op, disk),
    }
}

/// The plan of folder change `tree`, of `kind`, run as `batch`. A folder
/// copied to another volume is built in a hidden folder named for it.
fn tree_plan(
    catalog: &Catalog,
    kind: Kind,
    mut tree: TreeMove,
    batch: BatchId,
) -> Result<Plan, FileOpError> {
    let computers: i64 = catalog
        .db
        .read_one(sql!("SELECT count(*) FROM computers"), &[])?;
    let name = tree.destination.file_name().unwrap_or_default().to_owned();
    let dir = super::disk::parent(&tree.destination).to_path_buf();
    let destination = tree.destination.clone();
    let across = tree.copy.as_mut().map(|copy| {
        // Where the volume can't rename exclusively, it is built at its
        // name (#573).
        copy.hidden = match copy.proofs.direct {
            true => destination.clone(),
            false => dir.join(temp_name(&name, &batch, 0, 0)),
        };
        super::Across {
            files: copy.files(),
            companions: 0,
            bytes: copy.bytes,
            volume: copy.top.clone(),
            available: copy.available,
            originals_stay: usize::from(copy.originals_stay),
        }
    });
    let summary = Summary {
        photos: tree.state.photos.len(),
        other_computers: computers > 1,
        folder: tree
            .destination
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
        across,
        ..Summary::default()
    };
    Ok(Plan {
        batch,
        location: catalog.location.clone(),
        computer: catalog.computer.id.clone(),
        kind,
        create: None,
        tree: Some(tree),
        trash_tree: None,
        items: Vec::new(),
        skipped: Vec::new(),
        summary,
    })
}

/// Seconds since 1970.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A run's id: random, so two runs never share one.
fn new_batch() -> anyhow::Result<BatchId> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(BatchId(bytes.iter().map(|b| format!("{b:02x}")).collect()))
}

/// The families `chosen` stands for, as `db` has them: a master with every
/// copy of it; a copy whose master isn't chosen alone, for Remove from
/// Catalog, and left out (it has no file of its own) otherwise.
fn expand(
    db: &impl Reads,
    kind: Kind,
    chosen: &BTreeSet<PhotoId>,
    photos: &[PhotoRow],
    skipped: &mut Vec<Skipped>,
) -> anyhow::Result<Vec<Family>> {
    let by_id: HashMap<PhotoId, &PhotoRow> = photos.iter().map(|p| (p.id, p)).collect();
    let mut copies: HashMap<PhotoId, Vec<PhotoId>> = HashMap::new();
    for p in photos {
        if let Some(master) = p.master {
            copies.entry(master).or_default().push(p.id);
        }
    }
    // Each master's family, or the copies of it chosen without it.
    let mut groups: Vec<(PhotoId, Vec<PhotoId>)> = Vec::new();
    for id in chosen {
        let Some(photo) = by_id.get(id) else {
            skipped.push(Skipped {
                photo: *id,
                reason: Skip::Unknown,
            });
            continue;
        };
        match photo.master {
            None => {
                let mut rows = vec![*id];
                rows.extend(copies.get(id).into_iter().flatten());
                rows.sort();
                groups.push((*id, rows));
            }
            // Its master's family takes it.
            Some(master) if chosen.contains(&master) => {}
            Some(master) if kind == Kind::RemoveFromCatalog => {
                match groups.iter_mut().find(|(m, _)| *m == master) {
                    Some((_, rows)) => rows.push(*id),
                    None => groups.push((master, vec![*id])),
                }
            }
            Some(_) => skipped.push(Skipped {
                photo: *id,
                reason: Skip::CopyOnly,
            }),
        }
    }
    let mut families = Vec::new();
    for (master, rows) in groups {
        let first = rows.first().copied().unwrap_or(master);
        match Preconditions::read(db, master, rows)? {
            Some(preconditions) => families.push(Family {
                master,
                preconditions,
            }),
            None => skipped.push(Skipped {
                photo: first,
                reason: Skip::Unknown,
            }),
        }
    }
    Ok(families)
}

/// Whether `path` is somewhere no operation touches: in a Trash, or in
/// RAWmakase's own folders.
pub(super) fn protected(path: &Path) -> bool {
    let trash = path.components().any(|c| {
        let name = c.as_os_str().to_string_lossy().to_lowercase();
        name == ".trash"
            || name == ".trashes"
            || name == "$recycle.bin"
            || name.starts_with(".trash-")
    });
    let own = [crate::storage::data_dir(), crate::storage::local_data_dir()]
        .into_iter()
        .filter_map(|d| std::fs::canonicalize(d).ok())
        .any(|d| path.starts_with(d));
    let freedesktop = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .and_then(|d| std::fs::canonicalize(d.join("Trash")).ok())
        .is_some_and(|t| path.starts_with(t));
    trash || own || freedesktop
}

/// Catalog folders by the directory they resolve to on this computer: one
/// index per plan or run, not a scan per photo. Two roots or folders over
/// one directory resolve to the same directory here.
pub(super) struct Folders {
    by_dir: HashMap<FileId, Vec<FolderId>>,
    /// The file object at each path looked up, read once per plan or run
    /// (so before anything of it moves): a volume may take two different
    /// names (`ß.NEF`, `SS.NEF`) for one file, which no folding tells.
    files: std::cell::RefCell<HashMap<PathBuf, Option<FileId>>>,
    /// Each catalog filename by `sidecar::caseless`, folded once per plan
    /// or run, not once per photo checked against its folder (#692).
    folded: std::cell::RefCell<HashMap<String, String>>,
    /// While [`Self::caching`], the photos of each catalog folder [`shared`]
    /// and [`occupied`] asked about, by the folder and the directory they
    /// were asked about it over.
    photos: std::cell::RefCell<Option<HashMap<(FolderId, PathBuf), Photos>>>,
}

/// A catalog folder's photos, as read in a transaction, kept as its own
/// writes change them.
struct Photos {
    rows: Vec<Photo>,
    /// Whether each master's file object is read.
    files: bool,
}

/// A photo of a folder, as [`shared`] and [`occupied`] ask about it.
struct Photo {
    id: PhotoId,
    master: bool,
    /// Its filename, by `sidecar::caseless`.
    name: String,
    filename: String,
    /// A master's file object there, once [`Photos::files`].
    file: Option<FileId>,
}

impl Folders {
    pub(super) fn index(folders: &HashMap<FolderId, Located>) -> Self {
        let mut by_dir: HashMap<FileId, Vec<FolderId>> = HashMap::new();
        for (id, f) in folders {
            let found = f
                .path
                .as_deref()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .and_then(|p| fsops::entry(&p).ok());
            if let Some(e) = found {
                by_dir.entry(e.id).or_default().push(*id);
            }
        }
        Self {
            by_dir,
            files: Default::default(),
            folded: Default::default(),
            photos: Default::default(),
        }
    }

    /// Runs `checks`, all in one transaction, with each catalog folder's
    /// photos read once for [`shared`] and [`occupied`] (#692), not for
    /// each photo checked. The only photo rows `checks` may change are
    /// those `execute::moved` changes, which [`Self::refresh`] reads again.
    pub(super) fn caching<R>(&self, checks: impl FnOnce() -> R) -> R {
        struct Stop<'a>(&'a Folders);
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                self.0.photos.replace(None);
            }
        }
        self.photos.replace(Some(HashMap::new()));
        let _stop = Stop(self);
        checks()
    }

    /// Whether any photo of folder `folder`, over `dir`, outside `family`
    /// passes `test`, read in `db`; each master's file object there read
    /// first when `files`.
    fn any_photo(
        &self,
        db: &impl Reads,
        folder: FolderId,
        dir: &Path,
        files: bool,
        family: &[PhotoId],
        test: impl Fn(&Photo) -> bool,
    ) -> anyhow::Result<Option<bool>> {
        let mut cached = self.photos.borrow_mut();
        let Some(cached) = cached.as_mut() else {
            return Ok(None);
        };
        let photos = match cached.entry((folder, dir.to_path_buf())) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let rows = self.read_photos(
                    db,
                    sql!("SELECT id, master_id, folder, filename FROM photos WHERE folder=?"),
                    &[&folder],
                )?;
                let rows = rows.into_iter().map(|(_, p)| p).collect();
                e.insert(Photos { rows, files: false })
            }
        };
        if files && !photos.files {
            for p in photos.rows.iter_mut().filter(|p| p.master) {
                p.file = self.file_at(&dir.join(&p.filename));
            }
            photos.files = true;
        }
        Ok(Some(
            photos
                .rows
                .iter()
                .any(|p| !family.contains(&p.id) && test(p)),
        ))
    }

    /// The photos `query` reads with `params`, each with its folder.
    fn read_photos(
        &self,
        db: &impl Reads,
        query: super::super::db::Sql,
        params: &[&dyn super::super::value::ToValue],
    ) -> anyhow::Result<Vec<(FolderId, Photo)>> {
        row! {
            struct Row {
                id: PhotoId,
                master_id: Option<PhotoId>,
                folder: FolderId,
                filename: String,
            }
        }
        let rows: Vec<Row> = db.read(query, params)?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let photo = Photo {
                    id: r.id,
                    master: r.master_id.is_none(),
                    name: self.caseless(&r.filename),
                    filename: r.filename,
                    file: None,
                };
                (r.folder, photo)
            })
            .collect())
    }

    /// Reads master `master`'s family again into what [`Self::caching`]
    /// keeps, once `execute::moved` gave it another folder or name in the
    /// transaction.
    pub(super) fn refresh(&self, db: &impl Reads, master: PhotoId) -> anyhow::Result<()> {
        if self.photos.borrow().is_none() {
            return Ok(());
        }
        let family = self.read_photos(
            db,
            sql!("SELECT id, master_id, folder, filename FROM photos WHERE id=? OR master_id=?"),
            &[&master, &master],
        )?;
        let mut cached = self.photos.borrow_mut();
        let Some(cached) = cached.as_mut() else {
            return Ok(());
        };
        for ((folder, dir), photos) in cached.iter_mut() {
            photos
                .rows
                .retain(|p| !family.iter().any(|(_, f)| f.id == p.id));
            for (_, p) in family.iter().filter(|(f, _)| f == folder) {
                photos.rows.push(Photo {
                    id: p.id,
                    master: p.master,
                    name: p.name.clone(),
                    filename: p.filename.clone(),
                    file: match (photos.files, p.master) {
                        (true, true) => self.file_at(&dir.join(&p.filename)),
                        _ => None,
                    },
                });
            }
        }
        Ok(())
    }

    /// Catalog filename `name` by `sidecar::caseless`.
    fn caseless(&self, name: &str) -> String {
        self.folded
            .borrow_mut()
            .entry(name.to_string())
            .or_insert_with(|| caseless(OsStr::new(name)))
            .clone()
    }

    /// The file object at `path`, as first read in this plan or run.
    fn file_at(&self, path: &Path) -> Option<FileId> {
        *self
            .files
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| fsops::entry(path).ok().map(|e| e.id))
    }

    /// The catalog folders over directory `dir`.
    pub(super) fn over(&self, dir: &Path) -> Vec<FolderId> {
        fsops::entry(dir)
            .ok()
            .and_then(|e| self.by_dir.get(&e.id).cloned())
            .unwrap_or_default()
    }
}

/// Whether a master outside `family`, in a catalog folder over directory
/// `dir`, is file object `file` there (or, when it can't be told, is named
/// `filename`, by `sidecar::caseless`): the same file through another photo.
pub(super) fn shared(
    db: &impl Reads,
    folders: &Folders,
    dir: &Path,
    filename: &OsStr,
    file: Option<FileId>,
    family: &[PhotoId],
) -> anyhow::Result<bool> {
    let name = caseless(filename);
    for folder in folders.over(dir) {
        let same =
            |p: &Photo| p.master && (p.name == name || file.is_some_and(|f| p.file == Some(f)));
        let found = match folders.any_photo(db, folder, dir, file.is_some(), family, same)? {
            Some(found) => found,
            None => {
                Masters::read(db, folders, folder, dir, file.is_some())?.share(&name, file, family)
            }
        };
        if found {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The masters of a catalog folder over a directory, by name (by
/// `sidecar::caseless`) and, when asked, by the file object each name is
/// there: read once, for [`shared`].
pub(super) struct Masters {
    by_name: HashMap<String, Vec<PhotoId>>,
    by_file: HashMap<FileId, Vec<PhotoId>>,
}

impl Masters {
    fn read(
        db: &impl Reads,
        folders: &Folders,
        folder: FolderId,
        dir: &Path,
        files: bool,
    ) -> anyhow::Result<Self> {
        row! {
            struct Named {
                id: PhotoId,
                filename: String,
            }
        }
        let masters: Vec<Named> = db.read(
            sql!("SELECT id, filename FROM photos WHERE folder=? AND master_id IS NULL"),
            &[&folder],
        )?;
        let mut out = Self {
            by_name: HashMap::new(),
            by_file: HashMap::new(),
        };
        for p in masters {
            out.by_name
                .entry(folders.caseless(&p.filename))
                .or_default()
                .push(p.id);
            if files && let Some(f) = folders.file_at(&dir.join(&p.filename)) {
                out.by_file.entry(f).or_default().push(p.id);
            }
        }
        Ok(out)
    }

    /// Whether a master outside `family` is named `name` (by
    /// `sidecar::caseless`) or is file object `file`.
    fn share(&self, name: &str, file: Option<FileId>, family: &[PhotoId]) -> bool {
        let named = self.by_name.get(name).into_iter().flatten();
        let same = file
            .and_then(|f| self.by_file.get(&f))
            .into_iter()
            .flatten();
        named.chain(same).any(|p| !family.contains(p))
    }
}

/// Whether a photo outside `family`, in catalog folder `to` or another over
/// its directory `dir`, is named `filename` (by `sidecar::caseless`), offline or not.
pub(super) fn occupied(
    db: &impl Reads,
    folders: &Folders,
    to: Option<FolderId>,
    dir: &Path,
    filename: &OsStr,
    family: &[PhotoId],
) -> anyhow::Result<bool> {
    row! {
        struct Named {
            id: PhotoId,
            filename: String,
        }
    }
    let name = caseless(filename);
    let mut over = folders.over(dir);
    if let Some(to) = to
        && !over.contains(&to)
    {
        over.push(to);
    }
    for folder in over {
        let named = |p: &Photo| p.name == name;
        match folders.any_photo(db, folder, dir, false, family, named)? {
            Some(true) => return Ok(true),
            Some(false) => continue,
            None => {}
        }
        let photos: Vec<Named> = db.read(
            sql!("SELECT id, filename FROM photos WHERE folder=?"),
            &[&folder],
        )?;
        if photos
            .iter()
            .any(|p| !family.contains(&p.id) && folders.caseless(&p.filename) == name)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Where the files go, checked once.
pub(super) struct Target {
    /// The catalog's folder; `None` for one the plan makes, or one the
    /// catalog has no folder of its own for.
    folder: Option<FolderId>,
    /// Of a directory the catalog has no folder for, its root and logical
    /// path, where the catalog's folder is added as photos arrive.
    place: Option<(RootId, String)>,
    /// Where the folder resolves on this computer, as the catalog places it.
    resolved: PathBuf,
    /// Its canonical path, which members' destinations are in.
    dir: PathBuf,
    volume: VolumeId,
    /// Where its volume is mounted, and the bytes free there.
    top: PathBuf,
    available: Option<u64>,
    /// How its volume proves which file a name is, if it can at all.
    proof: Option<super::proof::Proof>,
    /// Whether its volume can rename only where nothing has the name.
    exclusive_rename: bool,
    names: Vec<OsString>,
    rules: Names,
    /// Filenames of the catalog's photos in it, through any folder over the
    /// same directory, by `sidecar::caseless`.
    catalog: HashSet<String>,
}

/// The files side of planning: what is on disk, read once per folder and
/// volume.
pub(super) struct Files<'a> {
    pub catalog: &'a Catalog,
    located: &'a HashMap<FolderId, Located>,
    disk: &'a dyn Disk,
    pub folders: Folders,
    listings: HashMap<PathBuf, Result<Rc<Listing>, String>>,
    /// A listing whose names are used composed where that finds them (#573).
    composed: HashMap<PathBuf, Rc<Listing>>,
    /// How each folder compares names, probed once.
    rules: HashMap<PathBuf, Names>,
    /// The masters of each catalog folder, over the directory they are read in.
    masters: HashMap<(FolderId, PathBuf), Masters>,
    volumes: HashMap<VolumeId, Option<fsops::VolumeFacts>>,
}

impl<'a> Files<'a> {
    fn new(
        catalog: &'a Catalog,
        located: &'a HashMap<FolderId, Located>,
        disk: &'a dyn Disk,
    ) -> Self {
        Self {
            catalog,
            located,
            disk,
            folders: Folders::index(located),
            listings: HashMap::new(),
            composed: HashMap::new(),
            rules: HashMap::new(),
            masters: HashMap::new(),
            volumes: HashMap::new(),
        }
    }

    /// The names in `dir`, listed once.
    pub(super) fn listing(&mut self, dir: &Path) -> Result<Rc<Listing>, String> {
        self.listings
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                fsops::list(dir)
                    .map(|names| Rc::new(Listing::new(names)))
                    .map_err(|e| e.to_string())
            })
            .clone()
    }

    /// `listing`, of `dir`, with each name as [`super::proof::usable`] takes it.
    fn composed(&mut self, dir: &Path, listing: &Listing) -> Rc<Listing> {
        let disk = self.disk;
        self.composed
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                let names = listing.names().iter();
                let usable = names.map(|n| super::proof::usable(disk, dir, n));
                Rc::new(Listing::new(usable.collect()))
            })
            .clone()
    }

    /// How `dir`, whose names are `listing`, compares names.
    pub(super) fn rules(&mut self, dir: &Path, listing: &Listing) -> Names {
        *self
            .rules
            .entry(dir.to_path_buf())
            .or_insert_with(|| Names(fsops::probe(dir, listing.names())))
    }

    /// [`shared`], with each folder's masters read once.
    fn shared(
        &mut self,
        dir: &Path,
        filename: &OsStr,
        file: FileId,
        family: &[PhotoId],
    ) -> anyhow::Result<bool> {
        let name = caseless(filename);
        for folder in self.folders.over(dir) {
            let masters = match self.masters.entry((folder, dir.to_path_buf())) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => e.insert(Masters::read(
                    &self.catalog.db,
                    &self.folders,
                    folder,
                    dir,
                    true,
                )?),
            };
            if masters.share(&name, Some(file), family) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The facts of `volume`, which `path` is on; `None` when they can't be
    /// read.
    fn volume(&mut self, volume: VolumeId, path: &Path) -> Option<fsops::VolumeFacts> {
        *self
            .volumes
            .entry(volume)
            .or_insert_with(|| self.disk.volume_facts(path).ok())
    }

    fn read_only(&mut self, volume: VolumeId, path: &Path) -> bool {
        self.volume(volume, path).is_none_or(|v| v.read_only)
    }

    /// Whether a rename on `volume`, which `path` is on, can change only a
    /// name's spelling.
    pub(super) fn renames_spelling(&mut self, volume: VolumeId, path: &Path) -> bool {
        self.volume(volume, path)
            .is_some_and(|v| v.renames_spelling)
    }

    /// Folder `to`, checked to take files.
    /// The folder at logical path `logical` of `root`, checked to take
    /// files: a catalog folder, or a directory of the root the catalog has
    /// no folder for, which it gains as the photos arrive.
    fn destination(&mut self, root: RootId, logical: &str) -> Result<Target, FileOpError> {
        let refuse = |why: &str| FileOpError::Destination(why.to_string());
        let own = self
            .located
            .iter()
            .find(|(_, f)| f.root == root && f.logical == logical)
            .map(|(id, f)| (*id, f));
        if let Some((to, folder)) = own {
            let (Some(path), Some(anchor)) = (folder.path.clone(), folder.anchor.clone()) else {
                return Err(refuse("The folder is offline or missing."));
            };
            return self.destination_at(Some(to), &path, &anchor);
        }
        let computer = &self.catalog.computer.id;
        let (anchor, path) = logical_anchored(&self.catalog.db, computer, root, logical)?
            .ok_or_else(|| refuse("The folder is offline or missing."))?;
        let mut target = self.destination_at(None, &path, &anchor)?;
        // Another of the catalog's folders there (of another root, or
        // spelled otherwise): the photos would join neither. The commit
        // checks again.
        let key = caseless(OsStr::new(logical));
        let alike = self
            .located
            .values()
            .any(|f| f.root == root && caseless(OsStr::new(&f.logical)) == key);
        if alike || !self.folders.over(&target.dir).is_empty() {
            return Err(refuse("The catalog has another folder there."));
        }
        target.place = Some((root, logical.to_string()));
        Ok(target)
    }

    /// Folder `name` to make below logical folder `parent` of `root`,
    /// checked, and it as where files go.
    fn new_folder(
        &mut self,
        root: RootId,
        parent: &str,
        name: &str,
    ) -> Result<(NewFolder, Target), FileOpError> {
        let refuse = |why: &str| FileOpError::Destination(why.to_string());
        if let Some(problem) = folder_name_problem(name) {
            return Err(refuse(problem));
        }
        let computer = &self.catalog.computer.id;
        let (anchor, path) = logical_anchored(&self.catalog.db, computer, root, parent)?
            .ok_or_else(|| refuse("The folder is offline or missing."))?;
        // The catalog's own folder there, if it has one.
        let own = self
            .located
            .iter()
            .find(|(_, f)| f.root == root && f.logical == parent)
            .map(|(id, _)| *id);
        let at = self.destination_at(own, &path, &anchor)?;
        let new = NewFolder {
            state: FolderState {
                root,
                parent: parent.to_string(),
                parent_path: path.clone(),
                name: name.to_string(),
                folder: None,
            },
            path: at.dir.join(name),
            rules: at.rules,
        };
        let free = super::folder::taken(&self.catalog.db, computer, self.disk, &new)
            .map_err(FileOpError::Catalog)?;
        if let Some(why) = free {
            return Err(FileOpError::Destination(why));
        }
        let resolved = path.join(name);
        let placed = logical_location(&self.catalog.db, computer, root, &join(parent, name))?;
        if placed.as_ref() != Some(&resolved) {
            return Err(refuse(
                "The catalog places a folder of that name elsewhere.",
            ));
        }
        let target = Target {
            folder: None,
            place: None,
            resolved,
            dir: new.path.clone(),
            volume: at.volume,
            top: at.top,
            available: at.available,
            proof: at.proof,
            exclusive_rename: at.exclusive_rename,
            names: Vec::new(),
            rules: at.rules,
            catalog: HashSet::new(),
        };
        Ok((new, target))
    }

    /// The folder at `path`, resolved below `anchor`, checked to take files.
    fn destination_at(
        &mut self,
        to: Option<FolderId>,
        path: &Path,
        anchor: &Path,
    ) -> Result<Target, FileOpError> {
        let refuse = |why: &str| FileOpError::Destination(why.to_string());
        let dir = match fsops::canonical_below(anchor, path) {
            Ok(dir) => dir,
            Err(Containment::Fs(FsError::NotFound)) => {
                return Err(refuse("The folder is offline or missing."));
            }
            Err(Containment::Symlink(p) | Containment::MountPoint(p)) => {
                return Err(FileOpError::Destination(format!(
                    "{} is a link or another volume inside its root folder.",
                    p.display()
                )));
            }
            Err(Containment::Outside) => {
                return Err(refuse("The folder is not inside its root folder."));
            }
            Err(Containment::Fs(e)) => return Err(FileOpError::Destination(e.to_string())),
        };
        // As the disk spells it, which members' destinations are journalled in.
        let dir =
            std::fs::canonicalize(&dir).map_err(|e| FileOpError::Destination(e.to_string()))?;
        let found = self
            .disk
            .entry(&dir)
            .map_err(|e| FileOpError::Destination(e.to_string()))?;
        if found.kind != EntryKind::Directory {
            return Err(refuse("The folder is not a folder on disk."));
        }
        if protected(&dir) {
            return Err(refuse(
                "Files can't be moved into the Trash or RAWmakase's own folders.",
            ));
        }
        if self.read_only(found.id.volume(), &dir) {
            return Err(refuse("The folder's volume is read-only."));
        }
        let names = self.listing(&dir).map_err(FileOpError::Destination)?;
        let names = names.names().to_vec();
        let mut over = self.folders.over(&dir);
        over.extend(to);
        let mut catalog = HashSet::new();
        for folder in over {
            let filenames: Vec<String> = self.catalog.db.read(
                sql!("SELECT filename FROM photos WHERE folder=?"),
                &[&folder],
            )?;
            catalog.extend(filenames.iter().map(|f| caseless(OsStr::new(f))));
        }
        let facts = self.volume(found.id.volume(), &dir);
        let available = facts.map(|v| v.available);
        let proof = facts.as_ref().and_then(super::proof::of);
        let exclusive_rename = facts.is_some_and(|v| v.exclusive_rename);
        Ok(Target {
            folder: to,
            place: None,
            resolved: path.to_path_buf(),
            rules: Names(fsops::probe(&dir, &names)),
            top: super::disk::top(self.disk, &dir),
            dir,
            volume: found.id.volume(),
            available,
            proof,
            exclusive_rename,
            names,
            catalog,
        })
    }

    /// Family `f`'s item: its file and companions resolved and checked, or
    /// why it is left out.
    pub(super) fn family(
        &mut self,
        f: Family,
        by_id: &HashMap<PhotoId, &PhotoRow>,
        target: Option<&Target>,
    ) -> Result<Item, (PhotoId, Skip)> {
        let skip = |reason| (f.master, reason);
        let refuse = |r| (f.master, Skip::Refused(r));
        let master = by_id.get(&f.master).ok_or_else(|| skip(Skip::Unknown))?;
        let folder = self
            .located
            .get(&master.folder)
            .ok_or_else(|| skip(Skip::Unknown))?;
        let (Some(path), Some(anchor)) = (folder.path.clone(), folder.anchor.clone()) else {
            return Err(skip(Skip::Missing));
        };
        let source = path.join(&master.filename);
        let file = match fsops::canonical_below(&anchor, &source) {
            Ok(file) => file,
            Err(Containment::Fs(FsError::NotFound)) => return Err(skip(Skip::Missing)),
            Err(Containment::Symlink(p) | Containment::MountPoint(p)) => {
                return Err(refuse(Refusal::NotPlain(p)));
            }
            Err(Containment::Outside) => return Err(refuse(Refusal::NotPlain(source))),
            Err(Containment::Fs(e)) => return Err(refuse(Refusal::Unreadable(e.to_string()))),
        };
        let found = self
            .disk
            .entry(&file)
            .map_err(|e| refuse(Refusal::Unreadable(e.to_string())))?;
        if found.kind != EntryKind::File {
            return Err(refuse(Refusal::Unreadable("it is not a file".into())));
        }
        // Another name of the same file object.
        if found.links != 1 {
            return Err(refuse(Refusal::Shared));
        }
        // The folder and the name as the disk spells them, which the
        // catalog may not (a volume that ignores case finds either): what
        // is journalled is matched byte for byte, so it must be the disk's.
        let dir = std::fs::canonicalize(super::disk::parent(&file))
            .map_err(|e| refuse(Refusal::Unreadable(e.to_string())))?;
        let listing = self
            .listing(&dir)
            .map_err(|e| refuse(Refusal::Unreadable(e)))?;
        let spelled: Vec<&OsString> = listing
            .alike(file.file_name().unwrap_or_default())
            .filter(|n| {
                self.disk
                    .entry(&dir.join(n))
                    .is_ok_and(|e| e.id == found.id)
            })
            .take(2)
            .collect();
        let [filename] = spelled[..] else {
            return Err(refuse(Refusal::Unreadable(
                "its name in its folder can't be told".into(),
            )));
        };
        let filename = filename.clone();
        // Copied from FAT or exFAT, whose drivers list names decomposed but
        // rename and remove them only by their stored spelling, its names
        // are journalled and used composed where that finds them (#573).
        let across = target.is_some_and(|t| t.volume != found.id.volume());
        let composed = across
            && self
                .volume(found.id.volume(), &dir)
                .as_ref()
                .and_then(super::proof::of)
                == Some(super::proof::Proof::Names);
        // What it makes there is named composed too, so it is stored in
        // the one spelling those drivers then take for it.
        let made_composed =
            across && target.is_some_and(|t| t.proof == Some(super::proof::Proof::Names));
        let there = |name: &OsStr| match made_composed {
            true => super::proof::composed(name),
            false => name.to_os_string(),
        };
        let (filename, listing) = match composed {
            true => (
                super::proof::usable(self.disk, &dir, &filename),
                self.composed(&dir, &listing),
            ),
            false => (filename, listing),
        };
        let file = dir.join(&filename);
        let rows = &f.preconditions.rows;
        let shared = self
            .shared(&dir, &filename, found.id, rows)
            .map_err(|e| refuse(Refusal::Unreadable(format!("{e:#}"))))?;
        if shared {
            return Err(refuse(Refusal::Shared));
        }
        if protected(&file) {
            return Err(refuse(Refusal::Protected));
        }
        if let Some(target) = target
            && dir == target.dir
        {
            return Err(skip(Skip::AlreadyThere));
        }
        if self.read_only(found.id.volume(), &dir) {
            return Err(refuse(Refusal::ReadOnly));
        }
        // A copy is told from any other file at its name, and its original
        // from a file put in its place, by file ids that stay, or on FAT and
        // exFAT by exact names and SHA-256 (#573).
        let mut proofs = super::proof::Proofs::default();
        let mut originals_stay = false;
        if let Some(target) = target
            && target.volume != found.id.volume()
        {
            let facts = self.volume(found.id.volume(), &dir);
            originals_stay = facts.as_ref().is_some_and(super::proof::no_trash);
            let here = facts.as_ref().and_then(super::proof::of);
            let (Some(originals), Some(copies)) = (here, target.proof) else {
                return Err(refuse(Refusal::FileIdsUnproven));
            };
            proofs = super::proof::Proofs {
                originals,
                copies,
                direct: !target.exclusive_rename,
                since: now(),
            };
        }
        let identity = self
            .disk
            .identity(&file)
            .map_err(|e| refuse(Refusal::Unreadable(format!("{e:#}"))))?;
        let same_file = |a: &OsStr, b: &OsStr| match (
            self.disk.entry(&dir.join(a)),
            self.disk.entry(&dir.join(b)),
        ) {
            (Ok(a), Ok(b)) => a.id == b.id,
            _ => false,
        };
        let Companions { carried, shared } = listing.companions(&filename, &same_file);
        let mut members = vec![NewMember {
            destination: target.map(|t| t.dir.join(there(&filename))),
            source: file,
            id: found.id,
            identity,
        }];
        let mut stays: Vec<PathBuf> = shared.iter().map(|n| dir.join(n)).collect();
        for name in carried {
            let path = dir.join(&name);
            let e = self
                .disk
                .entry(&path)
                .map_err(|e| refuse(Refusal::Unreadable(e.to_string())))?;
            // A companion with another name elsewhere stays, as one another
            // file reads does.
            if e.kind != EntryKind::File || e.links != 1 {
                stays.push(path);
                continue;
            }
            let identity = self
                .disk
                .identity(&path)
                .map_err(|e| refuse(Refusal::Unreadable(format!("{e:#}"))))?;
            members.push(NewMember {
                destination: target.map(|t| t.dir.join(there(&name))),
                source: path,
                id: e.id,
                identity,
            });
        }
        let mut preconditions = f.preconditions;
        preconditions.folder_path = Some(path);
        preconditions.proofs = proofs;
        // A folder the plan makes has no id yet: execution gives it.
        preconditions.to = target.and_then(|t| Some((t.folder?, t.resolved.clone())));
        preconditions.into = target.and_then(|t| {
            let (root, logical) = t.place.clone()?;
            Some(super::preconditions::Arrival {
                root,
                logical,
                resolved: t.resolved.clone(),
                added: false,
            })
        });
        // Another volume's folder takes copies; its partial copies are
        // named once the run's items are known.
        let copy = target
            .filter(|t| t.volume != found.id.volume())
            .map(|t| CopyTo {
                volume: t.volume,
                temps: Vec::new(),
                bytes: members.iter().map(|m| m.identity.size).sum(),
                originals_stay,
            });
        Ok(Item {
            preconditions,
            members,
            shared: stays,
            kept: Vec::new(),
            rules: target.map_or(UNKNOWN, |t| t.rules),
            copy,
        })
    }
}

/// Names compared as loosely as any folder could: for a plan with no
/// destination.
const UNKNOWN: Names = Names(fsops::NameRules {
    case: fsops::Folding::Unknown,
    normalization: fsops::Folding::Unknown,
});

/// Refuses the items whose files can't arrive at `target`: a name taken
/// there, a name two of them would have, or a companion one of them brings
/// that another file there would read (or one there its photo would).
fn arrivals(disk: &dyn Disk, target: &Target, items: &mut Vec<Item>, skipped: &mut Vec<Skipped>) {
    let dest = Destination {
        dir: &target.dir,
        names: &target.names,
        rules: target.rules,
        catalog: &target.catalog,
    };
    // The names they arrive with.
    let names_of = |item: &Item| -> Vec<OsString> {
        item.members
            .iter()
            .filter_map(|m| {
                let at = m.destination.as_deref().unwrap_or(&m.source);
                at.file_name().map(OsString::from)
            })
            .collect()
    };
    let mut uses: HashMap<String, usize> = HashMap::new();
    for item in items.iter() {
        for name in names_of(item) {
            *uses.entry(target.rules.key(&name)).or_default() += 1;
        }
    }
    let projected = Listing::new(
        target
            .names
            .iter()
            .cloned()
            .chain(items.iter().flat_map(names_of))
            .collect(),
    );
    let text = |n: &OsString| n.to_string_lossy().into_owned();
    items.retain(|item| {
        let names = names_of(item);
        let Some((photo, carried)) = names.split_first() else {
            return true;
        };
        let refusal = names
            .iter()
            .find(|n| dest.taken(disk, n).is_some())
            .map(|n| Refusal::NameTaken(text(n)))
            .or_else(|| {
                names
                    .iter()
                    .find(|n| uses.get(&target.rules.key(n)).copied().unwrap_or(0) > 1)
                    .map(|n| Refusal::NameTwice(text(n)))
            })
            .or_else(|| {
                projected
                    .arrival_conflict(photo, carried, &|_| false)
                    .map(|n| Refusal::CompanionClash(text(&n)))
            });
        match refusal {
            None => true,
            Some(r) => {
                skipped.push(Skipped {
                    photo: item.preconditions.master,
                    reason: Skip::Refused(r),
                });
                false
            }
        }
    });
}

/// Names the partial copies of the items copied to `target`'s volume, and
/// what they copy there; refuses the whole move when that volume hasn't
/// room for it.
fn copies(
    batch: &BatchId,
    target: &Target,
    items: &mut [Item],
) -> Result<Option<super::Across>, FileOpError> {
    let (mut bytes, mut files, mut companions, mut originals_stay) = (0u64, 0, 0, 0);
    for (index, item) in items.iter_mut().enumerate() {
        let Some(copy) = &mut item.copy else {
            continue;
        };
        files += 1;
        originals_stay += usize::from(copy.originals_stay);
        companions += item.members.len().saturating_sub(1);
        // Where its volume can't rename exclusively, each is made at its
        // name there (#573).
        let direct = item.preconditions.proofs.direct;
        copy.temps = item
            .members
            .iter()
            .enumerate()
            .map(|(ordinal, m)| match (direct, &m.destination) {
                (true, Some(to)) => to.clone(),
                _ => {
                    let at = m.destination.as_deref().unwrap_or(&m.source);
                    let name = at.file_name().unwrap_or_default();
                    target.dir.join(temp_name(name, batch, index, ordinal))
                }
            })
            .collect();
        bytes = bytes.saturating_add(copy.bytes);
    }
    if !items.iter().any(|i| i.copy.is_some()) {
        return Ok(None);
    }
    let Some(available) = target.available else {
        return Err(FileOpError::Destination(
            "How much room the other drive has can't be told.".into(),
        ));
    };
    if bytes.saturating_add(SPARE) > available {
        return Err(FileOpError::NoSpace {
            needed: bytes,
            available,
        });
    }
    Ok(Some(super::Across {
        files,
        companions,
        bytes,
        volume: target.top.clone(),
        available,
        originals_stay,
    }))
}

/// Refuses every item one of whose files another item of the plan also
/// takes: the same file object chosen twice, through two photos.
pub(super) fn refuse_twice(items: &mut Vec<Item>, skipped: &mut Vec<Skipped>) {
    let mut count: HashMap<FileId, usize> = HashMap::new();
    for m in items.iter().flat_map(|i| &i.members) {
        *count.entry(m.id).or_default() += 1;
    }
    items.retain(|item| {
        let twice = item
            .members
            .iter()
            .any(|m| count.get(&m.id).is_some_and(|n| *n > 1));
        if twice {
            skipped.push(Skipped {
                photo: item.preconditions.master,
                reason: Skip::Refused(Refusal::Twice),
            });
        }
        !twice
    });
}

/// What the confirmation counts.
fn summarize(
    items: &[Item],
    by_id: &HashMap<PhotoId, &PhotoRow>,
    other_computers: bool,
) -> Summary {
    let mut s = Summary {
        other_computers,
        ..Summary::default()
    };
    for item in items {
        let rows = &item.preconditions.rows;
        s.photos += rows.len();
        s.copies += rows
            .iter()
            .filter(|r| by_id.get(r).is_some_and(|p| p.master.is_some()))
            .count();
        s.edited += rows
            .iter()
            .filter(|r| by_id.get(r).is_some_and(|p| p.edited))
            .count();
        if let Some(photo) = by_id.get(&item.preconditions.master) {
            s.names.push(photo.filename.clone());
        }
        if !item.members.is_empty() {
            s.files += 1;
            s.companions += item.members.len() - 1;
        }
        s.shared_companions.extend(item.shared.iter().cloned());
    }
    s
}
