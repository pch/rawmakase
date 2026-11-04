//! What moving a user's file needs of the file system, and nothing more:
//! an exclusive rename of a file or directory, making a folder where nothing
//! is, which file object a name is, a folder's exact listing and how it
//! compares names, syncing a folder, whether a path is below a root with no
//! link or mount point between, and what a volume is. Policy-free: it knows
//! no photo, catalog or rule of what may move. A rename never falls back to
//! a hard link and unlink or to a copy, so a step either happened whole or
//! did not happen.
//!
//! For moving a file to another volume it also has the steps of a copy, each
//! one on its own so its caller journals between them ([`copy`]): making an
//! empty file where nothing has its name, copying into it while hashing
//! what was read, syncing it, giving it (or a folder) the source's extras
//! (extended attributes, alternate data streams) and time, hashing a file
//! and a file's or folder's extras, and removing a file (which only a
//! partial copy is for) or an empty folder (which only one a copy made is
//! for).
//!
//! Each system's calls are in a file of their own (`unix.rs`, `windows.rs`);
//! a system with neither answers [`FsError::Unsupported`].
//!
//! It depends on no workspace crate: the catalog's file operations and the app's
//! Trash use it, and the values an edit is made of build without it.
// Every unsafe operation here crosses into a system API: each one says why it is
// sound, and an unsafe function's body is no unsafe block of its own.
#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]
// Every public item says what it is for: this crate is a contract other crates
// and clients build on (#808 step 4.13).
#![warn(missing_docs)]
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::{fmt, io};

mod copy;
#[cfg(any(test, feature = "test-support"))]
pub use copy::set_extra;
pub use copy::{
    COPY_CHUNK, ContentHash, CopyError, copy_extras, copy_into, copy_times, create_file, extras,
    hash_file, hash_prefix, remove_file, sync_file,
};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;
#[cfg(not(any(unix, windows)))]
mod other;
#[cfg(not(any(unix, windows)))]
use other as os;

/// Why a step on the file system did not happen, as callers branch on it.
#[derive(Debug)]
pub enum FsError {
    /// The destination has a file or folder of that name, which is kept.
    AlreadyExists,
    /// The volume or system cannot do this step as asked (no exclusive
    /// rename, say); nothing was changed.
    Unsupported,
    /// The name to act on, or a folder on its way, isn't there.
    NotFound,
    /// The system refused the step for this user.
    PermissionDenied,
    /// The source and destination are on different volumes.
    CrossDevice,
    /// The volume is full, or the user's quota on it is.
    NoSpace,
    /// Any other failure, as the system gave it.
    Io(io::Error),
}

impl From<io::Error> for FsError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::AlreadyExists | io::ErrorKind::DirectoryNotEmpty => Self::AlreadyExists,
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            io::ErrorKind::CrossesDevices => Self::CrossDevice,
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Self::NoSpace,
            io::ErrorKind::Unsupported => Self::Unsupported,
            _ => Self::Io(e),
        }
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyExists => f.write_str("a file or folder with that name already exists"),
            Self::Unsupported => f.write_str("the volume can't do this safely"),
            Self::NotFound => f.write_str("not found"),
            Self::PermissionDenied => f.write_str("permission denied"),
            Self::CrossDevice => f.write_str("on another volume"),
            Self::NoSpace => f.write_str("the drive is full"),
            Self::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for FsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// A volume, as the system numbers it while it is mounted (`st_dev`; the
/// volume serial number on Windows).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VolumeId(u64);

/// A file object: its volume and its number there (`st_ino`; the file
/// index on Windows). A rename keeps it; a copy, or a file put in its
/// place, has another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId {
    volume: VolumeId,
    index: u64,
}

impl FileId {
    /// A file id no file is given (volume 0, number 0): for a record that
    /// must name one where none proves anything, so that nothing reading it
    /// takes a file for the one recorded.
    pub const NOBODY: Self = Self {
        volume: VolumeId(0),
        index: 0,
    };

    /// The volume this file object is on.
    pub fn volume(self) -> VolumeId {
        self.volume
    }
    /// The same file object numbered as if on another volume, for tests
    /// that need two volumes on one: applied twice, it is `self` again.
    #[cfg(any(test, feature = "test-support"))]
    pub fn elsewhere(self) -> Self {
        Self {
            volume: VolumeId(!self.volume.0),
            index: self.index,
        }
    }
    /// The file object numbered `index` on the same volume, for tests that
    /// need a volume renumbering its files.
    #[cfg(any(test, feature = "test-support"))]
    pub fn numbered(self, index: u64) -> Self {
        Self {
            volume: self.volume,
            index,
        }
    }
}

/// `volume:index` in hexadecimal, as journals keep it.
impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}:{:x}", self.volume.0, self.index)
    }
}

impl std::str::FromStr for FileId {
    type Err = std::num::ParseIntError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (volume, index) = text.split_once(':').unwrap_or((text, ""));
        Ok(Self {
            volume: VolumeId(u64::from_str_radix(volume, 16)?),
            index: u64::from_str_radix(index, 16)?,
        })
    }
}

/// What a name is, as [`Entry`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A folder.
    Directory,
    /// A symbolic link, or on Windows any name-surrogate reparse point (a
    /// junction, a mount point).
    Symlink,
    /// Anything else: a device, a pipe, a socket.
    Other,
}

/// What a name in a folder is, read without following a link at its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The file object the name is, or for a link the link itself.
    pub id: FileId,
    /// What it is: a link is [`EntryKind::Symlink`], never what it points to.
    pub kind: EntryKind,
    /// How many names the file object has.
    pub links: u64,
}

/// What `path` is, without following a link it ends in.
pub fn entry(path: &Path) -> Result<Entry, FsError> {
    os::entry(path)
}

/// The file object open file `file` is, whatever has its name now.
pub fn handle_id(file: &std::fs::File) -> Result<FileId, FsError> {
    os::handle_id(file)
}

/// Renames `from` to `to`, a file or a directory, only if nothing has the
/// name `to`: never replacing a file, nor an empty directory as a plain
/// rename would. Both must be on one volume ([`FsError::CrossDevice`]).
/// A volume or system without such a rename answers
/// [`FsError::Unsupported`] and nothing is changed: an SMB share among them
/// ([`VolumeKind::exclusive_rename`]).
pub fn rename_exclusive(from: &Path, to: &Path) -> Result<(), FsError> {
    os::rename_exclusive(from, to)
}

/// Makes folder `path`, only if nothing has that name: a folder, file or
/// link already there is never taken for the new one. Its parent must be
/// there; nothing is made on the way.
pub fn create_dir(path: &Path) -> Result<(), FsError> {
    Ok(std::fs::create_dir(path)?)
}

/// Removes folder `path` only while it is empty: a folder holding anything,
/// or a file, is left as it is. Not for a user's folder: its caller proves
/// `path` is one it made.
pub fn remove_dir(path: &Path) -> Result<(), FsError> {
    Ok(std::fs::remove_dir(path)?)
}

/// Makes the renames into or out of `dir` durable. Windows cannot open a
/// folder to sync it, and NTFS journals renames itself, so there it only
/// checks the folder is there.
pub fn sync_dir(dir: &Path) -> Result<(), FsError> {
    os::sync_dir(dir)
}

/// Every name in `dir`, exactly as the folder holds it (on Windows its long
/// names, never an 8.3 alias). A listing that fails part way is an error,
/// never a shorter list.
pub fn list(dir: &Path) -> Result<Vec<OsString>, FsError> {
    let mut names = Vec::new();
    for found in std::fs::read_dir(dir)? {
        names.push(found?.file_name());
    }
    Ok(names)
}

/// How a folder compares one aspect of names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folding {
    /// Names that differ only in it are different files.
    Exact,
    /// Such names are the same file.
    Insensitive,
    /// The folder had no name that shows it, or looking failed.
    Unknown,
}

/// How a folder compares names: by case, and by Unicode normalisation
/// (NFC against NFD).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameRules {
    /// Whether names that differ only in case are one file.
    pub case: Folding,
    /// Whether names that differ only in Unicode form (NFC against NFD) are one
    /// file.
    pub normalization: Folding,
}

/// How `dir` compares names, found through the names it has (`names`, its
/// listing) rather than by writing in it: a name looked up with its case
/// swapped, or in its other normalisation form, finds the same file object
/// only where the folder treats them alike. A folder with no such name
/// leaves that aspect [`Folding::Unknown`].
pub fn probe(dir: &Path, names: &[OsString]) -> NameRules {
    use unicode_normalization::UnicodeNormalization;
    let swapped = |name: &str| {
        let swapped: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect();
        (swapped != name).then_some(swapped)
    };
    let renormalized = |name: &str| {
        let nfc: String = name.nfc().collect();
        let nfd: String = name.nfd().collect();
        if nfc != name {
            Some(nfc)
        } else if nfd != name {
            Some(nfd)
        } else {
            None
        }
    };
    let aspect = |other: &dyn Fn(&str) -> Option<String>| {
        names
            .iter()
            .filter_map(|n| n.to_str())
            .find_map(|name| other(name).map(|o| (name, o)))
            .map_or(Folding::Unknown, |(name, other)| {
                match (entry(&dir.join(name)), entry(&dir.join(other))) {
                    (Ok(a), Ok(b)) if a.id == b.id => Folding::Insensitive,
                    (Ok(_), Ok(_) | Err(FsError::NotFound)) => Folding::Exact,
                    _ => Folding::Unknown,
                }
            })
    };
    NameRules {
        case: aspect(&swapped),
        normalization: aspect(&renormalized),
    }
}

/// Why a path is not plainly below its root.
#[derive(Debug)]
pub enum Containment {
    /// It isn't below the root, or has `.` or `..` in it.
    Outside,
    /// This name on the way, or the path itself, is a link (on Windows a
    /// junction or other name surrogate).
    Symlink(PathBuf),
    /// This name on the way is on another volume than the root: a mount
    /// point.
    MountPoint(PathBuf),
    /// Looking at a name on the way failed.
    Fs(FsError),
}

impl From<FsError> for Containment {
    fn from(e: FsError) -> Self {
        Self::Fs(e)
    }
}

/// `path`'s canonical form, when it is `root` or below it with no link and
/// no mount point between `root` and its end: `root` resolved (it may be
/// reached through links), then every name of `path` after it, each one
/// itself. `path` must exist.
pub fn canonical_below(root: &Path, path: &Path) -> Result<PathBuf, Containment> {
    let rest = path.strip_prefix(root).map_err(|_| Containment::Outside)?;
    let mut at = std::fs::canonicalize(root).map_err(FsError::from)?;
    let volume = entry(&at)?.id.volume();
    for part in rest.components() {
        let Component::Normal(name) = part else {
            return Err(Containment::Outside);
        };
        at.push(name);
        let found = entry(&at)?;
        if found.kind == EntryKind::Symlink {
            return Err(Containment::Symlink(at));
        }
        if found.id.volume() != volume {
            return Err(Containment::MountPoint(at));
        }
    }
    Ok(at)
}

/// A volume's file system, as far as moving files on it goes. The variants are
/// named for their file systems; those that cover several say which.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeKind {
    Apfs,
    /// HFS+.
    Hfs,
    /// ext2, ext3 or ext4.
    Ext,
    Xfs,
    Btrfs,
    Ntfs,
    /// FAT12, FAT16 or FAT32.
    Fat,
    Exfat,
    /// An SMB share (macOS's smbfs; Linux's CIFS, SMB2 and SMB; on Windows
    /// a network drive reached over SMB, by drive letter or UNC path).
    Smb,
    /// Any other, NFS, WebDAV and FUSE among them.
    Other,
}

impl VolumeKind {
    /// Whether a file keeps its file id while it is on it, so a file id
    /// proves which file a name is. FAT and exFAT number files anew when
    /// they are written, renamed or mounted again (macOS's msdos at the
    /// first close after a write; exFAT with temporary numbers from
    /// `u64::MAX` down, given again after a remount), and give a number to
    /// another file later.
    pub fn keeps_ids(self) -> bool {
        matches!(
            self,
            Self::Apfs | Self::Hfs | Self::Ext | Self::Xfs | Self::Btrfs | Self::Ntfs
        )
    }

    /// Whether [`rename_exclusive`] works on it: not on macOS's exFAT, which
    /// answers `ENOTSUP` (measured on macOS 15), nor on an SMB share, where
    /// a file another computer puts at the name isn't promised to be kept:
    /// macOS's smbfs answers `ENOTSUP` (measured on macOS 15), Linux's
    /// CIFS checks the name only on this computer, and a server asked by
    /// Windows not to replace a file may all the same (Samba 4.22 and a
    /// Synology did, #691). Elsewhere it is taken
    /// to, and a volume that doesn't answers [`FsError::Unsupported`], as
    /// [`rename_exclusive`] does on an SMB share.
    pub fn exclusive_rename(self) -> bool {
        !((cfg!(target_vendor = "apple") && self == Self::Exfat) || self == Self::Smb)
    }
}

/// What a volume is, as far as moving files on it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeFacts {
    /// Its file system.
    pub kind: VolumeKind,
    /// On this computer, rather than a network share (or a FUSE file
    /// system, which may be one), where locks are unreliable.
    pub local: bool,
    /// Mounted read-only: nothing on it can be renamed, made or removed.
    pub read_only: bool,
    /// Whether an exclusive rename can change only a name's case or Unicode
    /// form (`ROUND.NEF` to `round.NEF`) on it, as APFS's can. Elsewhere it
    /// isn't known (a volume may find the name taken, or do nothing), and
    /// it is `false`.
    pub renames_spelling: bool,
    /// Whether a file keeps its file id while it is on it, so a file id
    /// proves which file a name is: only on local volumes of kinds known to
    /// (APFS and HFS+ on macOS; ext2/3/4, XFS and Btrfs on Linux; NTFS on
    /// Windows). Not on FAT and exFAT (which number files anew when
    /// written, renamed or mounted again, and give a number to another file
    /// later), network volumes, FUSE, ReFS or any other kind.
    pub stable_ids: bool,
    /// Whether a file or folder can be renamed on it only where nothing has
    /// the name ([`VolumeKind::exclusive_rename`]).
    pub exclusive_rename: bool,
    /// The bytes this user may still write on it.
    pub available: u64,
}

/// When file `path` was made, as its volume keeps it (to 2 s on FAT, 10 ms
/// on exFAT).
pub fn created(path: &Path) -> Result<std::time::SystemTime, FsError> {
    Ok(std::fs::metadata(path)?.created()?)
}

/// The facts of the volume `path` is on.
pub fn volume_facts(path: &Path) -> Result<VolumeFacts, FsError> {
    os::volume_facts(path)
}

#[cfg(test)]
mod tests;
