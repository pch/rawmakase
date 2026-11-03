//! Synchronize Folder (issue #432): finds photos added, deleted, moved or
//! renamed outside RAWmakase, and brings the catalog in line while moved
//! photos keep their rows, and with them their ratings, edits and copies.
//!
//! [`Catalog::sync_inputs`] reads what a scan needs, each family's digest
//! included, in one snapshot; [`scan`](fn@scan) lists the folders on disk and matches
//! unknown files with the checksums on record, touching no catalog; and
//! [`Catalog::apply_folder_sync`] applies what the user accepted, checking
//! each action again first. Nothing on disk is ever changed.
//!
//! Find on Disk (issue #693) builds the same kind of plan for one offline
//! photo whose file the user picked ([`find_on_disk`]), and the other
//! offline photos of its folder found beside that file.
mod apply;
pub(super) mod digest;
mod find;
mod scan;

use super::db::{Reads, sql};
use super::ingest::{Conflict, Matcher};
use super::locations::{logical_from_legacy, resolve_anchored};
use super::value::row;
use super::{Catalog, FolderId, PhotoId, RootId};
use crate::storage::{FileChecksum, Identity};
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use apply::{Accepted, Applied};
pub use digest::FamilyDigest;
pub use find::{FindInputs, find_on_disk};
pub use scan::scan;

/// What a synchronization covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncScope {
    /// A catalog folder and every catalog folder below it, wherever each is
    /// on this computer.
    Folder(FolderId),
    /// A root folder and every catalog folder in it, as a root may have
    /// no catalog folder of its own.
    Root(RootId),
    /// Every catalog folder.
    All,
}

/// Where a catalog folder is on this computer.
#[derive(Clone, Debug)]
pub(in crate::catalog) struct Located {
    pub root: RootId,
    pub logical: String,
    /// `None` when it can't be on this computer.
    pub path: Option<PathBuf>,
    /// The location it resolves below: its own or an ancestor's, else where
    /// its root was added.
    pub anchor: Option<PathBuf>,
}

/// A master photo as the scan saw it in the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Master {
    pub id: PhotoId,
    pub folder: FolderId,
    pub filename: String,
}

/// What a scan works from, read in one snapshot. Its photos and checksums
/// are the catalog's as they were then; [`Catalog::apply_folder_sync`]
/// checks each again before acting.
pub struct SyncInputs {
    pub(super) scope: Vec<FolderId>,
    /// Locations walked for new directories besides the scope's folders:
    /// for every folder, each root's and each folder located apart, as a
    /// root may have no catalog folder of its own.
    pub(super) walk: Vec<PathBuf>,
    /// Every catalog folder, by id.
    pub(super) folders: HashMap<FolderId, Located>,
    /// Every master, by id.
    pub(super) masters: HashMap<PhotoId, Master>,
    /// The checksums on record, by master.
    pub(super) checksums: HashMap<PhotoId, FileChecksum>,
    /// Masters by (size, quick hash).
    pub(super) by_quick: HashMap<(u64, String), Vec<PhotoId>>,
    /// Masters without a full checksum but with a saved edit, by the size
    /// and first bytes their edit was saved for.
    pub(super) by_edit: HashMap<(u64, String), Vec<PhotoId>>,
    /// Every family's digest, by master.
    pub(super) digests: HashMap<PhotoId, FamilyDigest>,
    pub(super) matcher: Matcher,
}

impl SyncInputs {
    /// Where master `id`'s file is on this computer.
    pub(super) fn path_of(&self, id: PhotoId) -> Option<PathBuf> {
        let master = self.masters.get(&id)?;
        let folder = self.folders.get(&master.folder)?;
        Some(folder.path.as_ref()?.join(&master.filename))
    }
}

/// How a photo's file was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileState {
    Present,
    /// Its directory was listed and lacks it, or its directory is gone from
    /// a location that was listed and is online.
    Missing,
    /// It can't be checked here: its location or volume is offline.
    Unavailable,
    /// Listing or reading failed.
    Error(String),
}

/// A photo whose file is now elsewhere.
#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub photo: PhotoId,
    pub from: PathBuf,
    pub to: PathBuf,
    /// The file found, as read.
    pub identity: Identity,
    /// Its full checksum equals the one on record; otherwise only its quick
    /// hash, or the identity its edit was saved for, matched.
    pub verified: bool,
    /// Matched by its name alone: the photo has no checksum on record.
    pub by_name: bool,
    /// The photo's old location was offline, not observed missing: only a
    /// verified match is offered, and the catalog uses this file from now on
    /// even if the old one is still there.
    pub offline: bool,
    /// Where `to` goes in the catalog, as placed when scanned.
    pub(super) target: Target,
}

impl Move {
    /// The directory that becomes a new root folder for this move, if `to`
    /// is in no catalog folder.
    pub fn new_root(&self) -> Option<&Path> {
        match &self.target {
            Target::NewRoot(dir) => Some(dir),
            Target::Folder(..) => None,
        }
    }
}

/// Where a moved photo's file goes in the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// An existing catalog folder, by root and logical path.
    Folder(RootId, String),
    /// A new root folder at this directory, which no location covers: made
    /// with the first move into it that applies.
    NewRoot(PathBuf),
}

/// What a plan was made for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PlanKind {
    /// Synchronize Folder.
    #[default]
    Sync,
    /// Find on Disk for `photo`, a master, whose file the user picked: only
    /// moves and choices, and no import batch, so Previous Import stays.
    FindOnDisk { photo: PhotoId, file: PathBuf },
}

/// Files that match more than one photo, or photos matched by more than one
/// file: the user picks.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// Each pairing that can be accepted.
    pub options: Vec<Move>,
}

/// A file new to the catalog, and where it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Addition {
    pub file: PathBuf,
    /// The file as scanned; one changed since is not added.
    pub identity: Identity,
    /// The photos it is a byte-for-byte copy of, if any.
    pub copy_of: Vec<PhotoId>,
}

/// Something the scan could not check, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotChecked {
    pub path: PathBuf,
    pub reason: String,
}

/// A photo whose file is missing, with nothing found for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub photo: PhotoId,
    pub path: PathBuf,
}

/// What a scan found, for the user to review.
#[derive(Debug, Default)]
pub struct SyncPlan {
    /// Photos found moved or renamed: verified first, then probable ones.
    pub moves: Vec<Move>,
    pub choices: Vec<Choice>,
    /// Files new to the catalog, copies of photos in it included.
    pub additions: Vec<Addition>,
    pub missing: Vec<Missing>,
    /// Catalog folders whose directory was observed gone; removed once
    /// left without photos.
    pub gone_folders: Vec<FolderId>,
    pub not_checked: Vec<NotChecked>,
    /// Directories found whose catalog folder is elsewhere on this computer.
    pub conflicts: Vec<Conflict>,
    /// Directories more than one location claims equally.
    pub ambiguous: Vec<PathBuf>,
    /// Each master the plan acts on, as the scan's snapshot had it, and its
    /// family's digest then.
    pub(super) scanned: HashMap<PhotoId, (Master, FamilyDigest)>,
    pub(super) kind: PlanKind,
}

impl SyncPlan {
    pub fn kind(&self) -> &PlanKind {
        &self.kind
    }
    /// For Find on Disk, the move to the file the user picked.
    pub fn picked(&self) -> Option<&Move> {
        let PlanKind::FindOnDisk { photo, file } = &self.kind else {
            return None;
        };
        self.moves
            .iter()
            .find(|m| m.photo == *photo && m.to == *file)
    }
}

impl Catalog {
    /// What synchronizing `scope` needs, read in one snapshot.
    pub fn sync_inputs(&self, scope: SyncScope) -> Result<SyncInputs> {
        row! {
            struct PhotoRow {
                id: PhotoId,
                folder: FolderId,
                filename: String,
                identity: Option<String>,
            }
        }
        // One snapshot: the families' digests are of the very rows the
        // checksums and paths matched come from.
        row! {
            struct RootRow {
                id: RootId,
                original: String,
                location: Option<String>,
            }
        }
        let (folders, photos, checksums, digests, roots) = self.db.snapshot(|s| {
            let photos: Vec<PhotoRow> = s.read(
                sql!("SELECT id, folder, filename, identity FROM photos WHERE master_id IS NULL"),
                &[],
            )?;
            let roots: Vec<RootRow> = s.read(
                sql!(
                    "SELECT r.id, r.original_path, l.path FROM roots r LEFT JOIN folder_locations l
                     ON l.root = r.id AND l.relative_path = '' AND l.computer = ?"
                ),
                &[&self.computer.id],
            )?;
            Ok((
                self.locate_folders(s)?,
                photos,
                super::checksums::stored(s)?,
                digest::every_family(s)?,
                roots,
            ))
        })?;
        // Where each root is here: its location, else where it was added.
        let roots: Vec<(RootId, PathBuf)> = roots
            .into_iter()
            .map(|r| (r.id, PathBuf::from(r.location.unwrap_or(r.original))))
            .filter(|(_, path)| !path.as_os_str().is_empty())
            .collect();
        let walk = match scope {
            SyncScope::All | SyncScope::Root(_) => {
                let in_scope = |root: RootId| match scope {
                    SyncScope::Root(r) => r == root,
                    _ => true,
                };
                // Each root where it is here, and each folder located apart.
                let mut anchors: Vec<PathBuf> = folders
                    .values()
                    .filter(|f| in_scope(f.root))
                    .filter_map(|f| f.anchor.clone())
                    .collect();
                anchors.extend(
                    roots
                        .iter()
                        .filter(|(root, _)| in_scope(*root))
                        .map(|(_, path)| path.clone()),
                );
                anchors.sort();
                anchors.dedup();
                anchors
            }
            SyncScope::Folder(_) => Vec::new(),
        };
        let scope = match scope {
            SyncScope::All => folders.keys().copied().collect(),
            SyncScope::Root(root) => folders
                .iter()
                .filter(|(_, f)| f.root == root)
                .map(|(id, _)| *id)
                .collect(),
            SyncScope::Folder(id) => {
                let top = folders
                    .get(&id)
                    .ok_or_else(|| anyhow::anyhow!("Unknown folder"))?;
                folders
                    .iter()
                    .filter(|(_, f)| f.root == top.root && below(&f.logical, &top.logical))
                    .map(|(id, _)| *id)
                    .collect()
            }
        };
        let mut by_quick: HashMap<(u64, String), Vec<PhotoId>> = HashMap::new();
        for (id, checksum) in &checksums {
            by_quick
                .entry((checksum.size, checksum.quick.clone()))
                .or_default()
                .push(*id);
        }
        let mut by_edit: HashMap<(u64, String), Vec<PhotoId>> = HashMap::new();
        let mut masters = HashMap::new();
        for p in photos {
            let full = checksums.get(&p.id).is_some_and(|c| c.full.is_some());
            if !full
                && let Some(identity) = p
                    .identity
                    .and_then(|text| serde_json::from_str::<Identity>(&text).ok())
            {
                by_edit
                    .entry((identity.size, identity.prefix_hash))
                    .or_default()
                    .push(p.id);
            }
            masters.insert(
                p.id,
                Master {
                    id: p.id,
                    folder: p.folder,
                    filename: p.filename,
                },
            );
        }
        Ok(SyncInputs {
            scope,
            walk,
            digests,
            folders,
            masters,
            checksums,
            by_quick,
            by_edit,
            matcher: Matcher::new(self)?,
        })
    }
    /// Every catalog folder and where it is on this computer, read in `db`.
    pub(super) fn locate_folders(&self, db: &impl Reads) -> Result<HashMap<FolderId, Located>> {
        locate_folders(db, &self.computer.id)
    }
}

/// Every catalog folder and where it is on `computer`, read in `db`.
pub(in crate::catalog) fn locate_folders(
    db: &impl Reads,
    computer: &str,
) -> Result<HashMap<FolderId, Located>> {
    row! {
        struct FolderRow {
            id: FolderId,
            root: RootId,
            original: String,
            relative: String,
            logical: Option<String>,
        }
    }
    row! {
        struct LocationRow {
            root: RootId,
            relative: String,
            path: String,
        }
    }
    let folders: Vec<FolderRow> = db.read(
        sql!(
            "SELECT f.id, f.root, r.original_path, f.relative_path, p.path
                 FROM folders f JOIN roots r ON r.id = f.root
                 LEFT JOIN folder_paths p ON p.folder = f.id"
        ),
        &[],
    )?;
    let located: Vec<LocationRow> = db.read(
        sql!("SELECT root, relative_path, path FROM folder_locations WHERE computer=?"),
        &[&computer],
    )?;
    let mut rows: HashMap<RootId, Vec<(String, PathBuf)>> = HashMap::new();
    for l in located {
        rows.entry(l.root)
            .or_default()
            .push((l.relative, PathBuf::from(l.path)));
    }
    Ok(folders
        .into_iter()
        .map(|f| {
            let logical = f
                .logical
                .unwrap_or_else(|| logical_from_legacy(&f.original, &f.relative));
            let own = rows.get(&f.root).map_or(&[][..], |r| &r[..]);
            let (anchor, path) = resolve_anchored(&f.original, own, &logical, cfg!(windows))
                .filter(|(_, path)| !path.as_os_str().is_empty())
                .unzip();
            (
                f.id,
                Located {
                    root: f.root,
                    logical,
                    path,
                    anchor,
                },
            )
        })
        .collect())
}

/// Master `id` as the catalog has it now, if it still is a master.
pub(super) fn current_master(db: &impl Reads, id: PhotoId) -> Result<Option<Master>> {
    row! {
        struct Row {
            folder: FolderId,
            filename: String,
            master: Option<PhotoId>,
        }
    }
    let row: Option<Row> = db.read_optional(
        sql!("SELECT folder, filename, master_id FROM photos WHERE id=?"),
        &[&id],
    )?;
    Ok(row.filter(|r| r.master.is_none()).map(|r| Master {
        id,
        folder: r.folder,
        filename: r.filename,
    }))
}

/// Lists `directory` to the end, so a listing that fails part way is no
/// evidence of anything.
pub(super) fn list_all(directory: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        entry?;
    }
    Ok(())
}

/// A file name as a case-insensitive volume compares it: NFC, lower case.
pub(in crate::catalog) fn folded(name: &std::ffi::OsStr) -> String {
    use unicode_normalization::UnicodeNormalization;
    name.to_string_lossy()
        .nfc()
        .collect::<String>()
        .to_lowercase()
}

/// How many names deep logical path `path` is.
pub(super) fn depth(path: &str) -> usize {
    super::locations::names(path).count()
}

/// Whether logical path `path` is `top` or below it.
fn below(path: &str, top: &str) -> bool {
    top.is_empty()
        || path == top
        || path
            .strip_prefix(top)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether `path`'s file is there, by the evidence rules every photo is
/// judged by: present when it is a file; missing only when its directory,
/// or the location it resolves below (`anchor`), was listed and `online`
/// says that location's volume is there; unavailable when that location
/// can't be reached; an error otherwise.
pub(super) fn state_of(
    path: Option<&Path>,
    anchor: Option<&Path>,
    online: &dyn Fn(&Path) -> bool,
) -> FileState {
    let (Some(path), Some(anchor)) = (path, anchor) else {
        return FileState::Unavailable;
    };
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() || m.file_type().is_symlink() => return FileState::Present,
        Ok(_) => return FileState::Error(format!("{} is not a file", path.display())),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return FileState::Error(format!("{}: {e}", path.display()));
        }
        Err(_) => {}
    }
    let listed = |dir: &Path| list_all(dir);
    let parent = path.parent().unwrap_or(anchor);
    let evidence = match listed(parent) {
        Ok(()) => listed(anchor),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && parent != anchor => listed(anchor),
        Err(e) => Err(e),
    };
    match evidence {
        Ok(()) if online(anchor) => FileState::Missing,
        Ok(()) => FileState::Unavailable,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileState::Unavailable,
        Err(e) => FileState::Error(format!("{}: {e}", parent.display())),
    }
}

#[cfg(test)]
#[path = "folder_sync_tests.rs"]
mod tests;
