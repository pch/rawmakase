//! The file system as planning, execution and recovery see it: exactly the
//! [`fsops`] calls they make, behind one trait, so tests inject failures
//! and crashes at any step and record which folders were synced. The real
//! one is [`Real`]; nothing else implements it outside tests.
use crate::fsops::{self, ContentHash, CopyError, Entry, FileId, FsError};
use crate::storage::Identity;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(super) trait Disk {
    fn entry(&self, path: &Path) -> Result<Entry, FsError>;
    fn list(&self, dir: &Path) -> Result<Vec<OsString>, FsError>;
    fn identity(&self, path: &Path) -> anyhow::Result<Identity>;
    fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError>;
    fn create_dir(&self, path: &Path) -> Result<(), FsError>;
    fn sync_dir(&self, dir: &Path) -> Result<(), FsError>;
    /// `path` with every link on the way resolved.
    fn canonical(&self, path: &Path) -> Result<PathBuf, FsError>;
    fn volume_facts(&self, path: &Path) -> Result<fsops::VolumeFacts, FsError>;
    /// The steps of a copy to another volume ([`fsops::create_file`] and
    /// the rest), each on its own. A copy is written through the handle
    /// its file was made through (`into`); `to` names it.
    fn create_file(&self, path: &Path) -> Result<std::fs::File, FsError>;
    fn copy_into(
        &self,
        from: &Path,
        to: &Path,
        into: &mut std::fs::File,
        go_on: &mut dyn FnMut(u64) -> bool,
    ) -> Result<ContentHash, CopyError>;
    /// The file object `file`, the handle the file at `at` was made
    /// through, is ([`fsops::handle_id`]).
    fn handle_id(&self, at: &Path, file: &std::fs::File) -> Result<FileId, FsError>;
    /// Makes the contents of the file at `at` durable, through `file`, the
    /// handle it was made through.
    fn sync_file(&self, at: &Path, file: &std::fs::File) -> Result<(), FsError>;
    /// Gives file or folder `to` the extras of `from` (extended attributes,
    /// alternate data streams), [`FsError::Unsupported`] where its volume
    /// can't keep them; a file's through `into`, the handle it was made
    /// through, where given.
    fn copy_extras(
        &self,
        from: &Path,
        to: &Path,
        into: Option<&std::fs::File>,
    ) -> Result<(), FsError>;
    /// Gives file or folder `to` the times and permission bits of `from`,
    /// as [`Disk::copy_extras`] its extras.
    fn copy_times(
        &self,
        from: &Path,
        to: &Path,
        into: Option<&std::fs::File>,
    ) -> Result<(), FsError>;
    /// The SHA-256 of file or folder `path`'s extras.
    fn extras(&self, path: &Path) -> Result<ContentHash, FsError>;
    fn hash(&self, path: &Path) -> Result<ContentHash, FsError>;
    /// The SHA-256 of `path`'s first `len` bytes.
    fn hash_prefix(&self, path: &Path, len: u64) -> Result<ContentHash, FsError>;
    /// When `path` was made.
    fn created(&self, path: &Path) -> Result<std::time::SystemTime, FsError>;
    /// Removes a partial copy, which its caller proves its own.
    fn remove_file(&self, path: &Path) -> Result<(), FsError>;
    /// Removes an empty folder a copy made, which its caller proves its own.
    fn remove_dir(&self, path: &Path) -> Result<(), FsError>;
}

/// The file system itself.
pub(super) struct Real;

impl Disk for Real {
    fn entry(&self, path: &Path) -> Result<Entry, FsError> {
        fsops::entry(path)
    }
    fn list(&self, dir: &Path) -> Result<Vec<OsString>, FsError> {
        fsops::list(dir)
    }
    fn identity(&self, path: &Path) -> anyhow::Result<Identity> {
        Identity::read(path)
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        fsops::rename_exclusive(from, to)
    }
    fn create_dir(&self, path: &Path) -> Result<(), FsError> {
        fsops::create_dir(path)
    }
    fn sync_dir(&self, dir: &Path) -> Result<(), FsError> {
        fsops::sync_dir(dir)
    }
    fn canonical(&self, path: &Path) -> Result<PathBuf, FsError> {
        Ok(std::fs::canonicalize(path)?)
    }
    fn volume_facts(&self, path: &Path) -> Result<fsops::VolumeFacts, FsError> {
        fsops::volume_facts(path)
    }
    fn create_file(&self, path: &Path) -> Result<std::fs::File, FsError> {
        fsops::create_file(path)
    }
    fn copy_into(
        &self,
        from: &Path,
        _to: &Path,
        into: &mut std::fs::File,
        go_on: &mut dyn FnMut(u64) -> bool,
    ) -> Result<ContentHash, CopyError> {
        fsops::copy_into(from, into, go_on)
    }
    fn handle_id(&self, _at: &Path, file: &std::fs::File) -> Result<FileId, FsError> {
        fsops::handle_id(file)
    }
    fn hash_prefix(&self, path: &Path, len: u64) -> Result<ContentHash, FsError> {
        fsops::hash_prefix(path, len)
    }
    fn created(&self, path: &Path) -> Result<std::time::SystemTime, FsError> {
        fsops::created(path)
    }
    fn sync_file(&self, _at: &Path, file: &std::fs::File) -> Result<(), FsError> {
        fsops::sync_file(file)
    }
    fn copy_extras(
        &self,
        from: &Path,
        to: &Path,
        into: Option<&std::fs::File>,
    ) -> Result<(), FsError> {
        fsops::copy_extras(from, to, into)
    }
    fn copy_times(
        &self,
        from: &Path,
        to: &Path,
        into: Option<&std::fs::File>,
    ) -> Result<(), FsError> {
        fsops::copy_times(from, to, into)
    }
    fn extras(&self, path: &Path) -> Result<ContentHash, FsError> {
        fsops::extras(path)
    }
    fn hash(&self, path: &Path) -> Result<ContentHash, FsError> {
        fsops::hash_file(path)
    }
    fn remove_file(&self, path: &Path) -> Result<(), FsError> {
        fsops::remove_file(path)
    }
    fn remove_dir(&self, path: &Path) -> Result<(), FsError> {
        fsops::remove_dir(path)
    }
}

/// Where the volume folder `dir` is on is mounted: `dir`'s furthest
/// ancestor on the same volume, or `dir` when that can't be read.
pub(super) fn top(disk: &dyn Disk, dir: &Path) -> PathBuf {
    let Ok(here) = disk.entry(dir) else {
        return dir.to_path_buf();
    };
    let mut top = dir;
    while let Some(up) = top.parent() {
        match disk.entry(up) {
            Ok(e) if e.id.volume() == here.id.volume() => top = up,
            _ => break,
        }
    }
    top.to_path_buf()
}

/// The folder `path` is named in.
pub(super) fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new(""))
}

/// Where a journalled file object is, by its exact name in its folder's
/// listing and its file id: never by looking its name up, which on a
/// case-insensitive or normalising volume finds `A.NEF` as `a.NEF`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Presence {
    /// The name is in the listing, and is that file object.
    Here,
    /// The folder listed whole, is on the file object's volume (so that
    /// volume is there), and has no such name.
    Absent,
    /// Anything else: another file object has the name, the folder can't be
    /// listed or is on another volume (offline, or a mount point left
    /// behind), or reading failed.
    Unknown,
}

/// Whether the file object `id` is at `path`.
pub(super) fn presence(disk: &dyn Disk, path: &Path, id: fsops::FileId) -> Presence {
    let (dir, Some(name)) = (parent(path), path.file_name()) else {
        return Presence::Unknown;
    };
    let Ok(names) = disk.list(dir) else {
        return Presence::Unknown;
    };
    if names.iter().any(|n| n == name) {
        return match disk.entry(path) {
            Ok(e) if e.id == id => Presence::Here,
            _ => Presence::Unknown,
        };
    }
    match disk.entry(dir) {
        Ok(d) if d.id.volume() == id.volume() => Presence::Absent,
        _ => Presence::Unknown,
    }
}

/// Whether the file object `id` is gone from `path`: [`Presence::Absent`],
/// no name in the folder is `path`'s folded, and looking `path` up finds
/// nothing, so it isn't there under another spelling or alias either. What
/// lets a trashed photo's rows go.
pub(super) fn gone(disk: &dyn Disk, path: &Path, id: fsops::FileId) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let wanted = super::super::sidecar::caseless(name);
    // And looked up, which finds it under any name the volume takes for
    // this one, beyond what folding tells (`ß.NEF` as `SS.NEF`).
    presence(disk, path, id) == Presence::Absent
        && matches!(disk.entry(path), Err(FsError::NotFound))
        && disk.list(parent(path)).is_ok_and(|names| {
            names
                .iter()
                .all(|n| super::super::sidecar::caseless(n) != wanted)
        })
}

/// `path` as the disk spells the name of file object `id` there, which a
/// volume that normalises names may spell otherwise than it was given: the
/// one name in its folder's listing that `sidecar::caseless` takes for it
/// and that is that file object.
pub(super) fn spelled(disk: &dyn Disk, path: &Path, id: fsops::FileId) -> Result<PathBuf, FsError> {
    let dir = parent(path);
    let wanted = super::super::sidecar::caseless(path.file_name().unwrap_or_default());
    let names = disk.list(dir)?;
    let mut spelled = names.iter().filter(|n| {
        super::super::sidecar::caseless(n) == wanted
            && disk.entry(&dir.join(n)).is_ok_and(|e| e.id == id)
    });
    match (spelled.next(), spelled.next()) {
        (Some(name), None) => Ok(dir.join(name)),
        _ => Err(FsError::Io(std::io::Error::other(
            "its name in its folder can't be told",
        ))),
    }
}

/// Syncs the folders of `from` and `to` after a rename between them, so
/// the journal moves past the step only once it is durable.
pub(super) fn sync_parents(disk: &dyn Disk, from: &Path, to: &Path) -> Result<(), FsError> {
    let (a, b) = (parent(from), parent(to));
    disk.sync_dir(a)?;
    if b != a {
        disk.sync_dir(b)?;
    }
    Ok(())
}

/// `disk` with each folder listed once (#692): for checks that change
/// nothing on disk themselves, made one after another in a moment, such
/// as finishing a batch's items in one transaction. Anything that changes
/// the disk through it forgets every listing.
pub(super) struct Listed<'a> {
    disk: &'a dyn Disk,
    listings: RefCell<HashMap<PathBuf, Vec<OsString>>>,
}

impl<'a> Listed<'a> {
    pub(super) fn new(disk: &'a dyn Disk) -> Self {
        Self {
            disk,
            listings: RefCell::default(),
        }
    }

    fn changes<T>(&self, result: T) -> T {
        self.listings.borrow_mut().clear();
        result
    }
}

impl Disk for Listed<'_> {
    fn entry(&self, path: &Path) -> Result<Entry, FsError> {
        self.disk.entry(path)
    }
    fn list(&self, dir: &Path) -> Result<Vec<OsString>, FsError> {
        if let Some(names) = self.listings.borrow().get(dir) {
            return Ok(names.clone());
        }
        let names = self.disk.list(dir)?;
        self.listings
            .borrow_mut()
            .insert(dir.to_path_buf(), names.clone());
        Ok(names)
    }
    fn identity(&self, path: &Path) -> anyhow::Result<Identity> {
        self.disk.identity(path)
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        self.changes(self.disk.rename(from, to))
    }
    fn create_dir(&self, path: &Path) -> Result<(), FsError> {
        self.changes(self.disk.create_dir(path))
    }
    fn sync_dir(&self, dir: &Path) -> Result<(), FsError> {
        self.disk.sync_dir(dir)
    }
    fn canonical(&self, path: &Path) -> Result<PathBuf, FsError> {
        self.disk.canonical(path)
    }
    fn volume_facts(&self, path: &Path) -> Result<fsops::VolumeFacts, FsError> {
        self.disk.volume_facts(path)
    }
    fn create_file(&self, path: &Path) -> Result<std::fs::File, FsError> {
        self.changes(self.disk.create_file(path))
    }
    fn copy_into(
        &self,
        from: &Path,
        to: &Path,
        into: &mut std::fs::File,
        go_on: &mut dyn FnMut(u64) -> bool,
    ) -> Result<ContentHash, CopyError> {
        self.changes(self.disk.copy_into(from, to, into, go_on))
    }
    fn handle_id(&self, at: &Path, file: &std::fs::File) -> Result<FileId, FsError> {
        self.disk.handle_id(at, file)
    }
    fn sync_file(&self, at: &Path, file: &std::fs::File) -> Result<(), FsError> {
        self.disk.sync_file(at, file)
    }
    fn copy_extras(
        &self,
        from: &Path,
        to: &Path,
        into: Option<&std::fs::File>,
    ) -> Result<(), FsError> {
        self.changes(self.disk.copy_extras(from, to, into))
    }
    fn copy_times(
        &self,
        from: &Path,
        to: &Path,
        into: Option<&std::fs::File>,
    ) -> Result<(), FsError> {
        self.changes(self.disk.copy_times(from, to, into))
    }
    fn extras(&self, path: &Path) -> Result<ContentHash, FsError> {
        self.disk.extras(path)
    }
    fn hash(&self, path: &Path) -> Result<ContentHash, FsError> {
        self.disk.hash(path)
    }
    fn hash_prefix(&self, path: &Path, len: u64) -> Result<ContentHash, FsError> {
        self.disk.hash_prefix(path, len)
    }
    fn created(&self, path: &Path) -> Result<std::time::SystemTime, FsError> {
        self.disk.created(path)
    }
    fn remove_file(&self, path: &Path) -> Result<(), FsError> {
        self.changes(self.disk.remove_file(path))
    }
    fn remove_dir(&self, path: &Path) -> Result<(), FsError> {
        self.changes(self.disk.remove_dir(path))
    }
}
