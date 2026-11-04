//! The operation lock: one RAWmakase process at a time changes files of a
//! catalog, and only that process recovers its unfinished operations.
//!
//! The lock is an OS advisory lock (`flock`, `LockFileEx`) on a file beside
//! the catalog named after its canonical path, so the catalog opened through
//! a link and through its real path takes the same lock. The file is never
//! deleted (deleting a lock file another process holds would let a third one
//! lock a new file of the same name). The lock is held while the returned
//! [`OperationLock`] lives, and the system releases it when the process
//! ends, however it ends.
use super::super::{Catalog, CatalogLocation};
use super::FileOpError;
use crate::fsops::{self, EntryKind, FileId};
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};

/// Held for a whole execution or recovery.
#[must_use]
pub(super) struct OperationLock {
    _file: File,
}

/// The file the lock of the catalog at `canonical` is taken on.
pub(super) fn lock_path(canonical: &Path) -> PathBuf {
    let mut name = canonical.file_name().unwrap_or_default().to_owned();
    name.push(".file-operations-lock");
    canonical.with_file_name(name)
}

/// Runs `open`, which connects to the catalog at `location`, with the file
/// object the catalog is: the same one before and after it opened, else
/// `None`, and the lock is never given.
pub(in crate::catalog) fn opening<R>(
    location: &CatalogLocation,
    open: impl FnOnce() -> R,
) -> (R, Option<FileId>) {
    let CatalogLocation::File(path) = location;
    let id = || {
        std::fs::canonicalize(path)
            .ok()
            .and_then(|p| fsops::entry(&p).ok())
            .map(|e| e.id)
    };
    let before = id();
    let opened = open();
    let after = id();
    (opened, before.filter(|b| Some(*b) == after))
}

/// Takes the operation lock of `catalog`, refusing a catalog file operations
/// can't be made safe for: one on a network volume, where locks are
/// unreliable; a file with more than one hard link, whose other names would
/// take other locks; and a catalog whose file is no longer the one opened.
pub(super) fn acquire(catalog: &Catalog) -> Result<OperationLock, FileOpError> {
    let CatalogLocation::File(path) = catalog.location();
    let canonical = std::fs::canonicalize(path).map_err(anyhow::Error::from)?;
    let found = fsops::entry(&canonical).map_err(anyhow::Error::from)?;
    if found.kind != EntryKind::File || found.links != 1 {
        return Err(FileOpError::Unsupported(
            "The catalog file has more than one name (a hard link), so RAWmakase can't keep other windows from changing the same files",
        ));
    }
    let volume = fsops::volume_facts(&canonical).map_err(anyhow::Error::from)?;
    if !volume.local {
        return Err(FileOpError::Unsupported(
            "The catalog is on a network volume, where RAWmakase can't keep other computers from changing the same files",
        ));
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(&canonical))
        .map_err(anyhow::Error::from)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(FileOpError::Busy),
        Err(TryLockError::Error(e)) => return Err(FileOpError::Catalog(e.into())),
    }
    // Locked: the catalog file must still be the one this connection has.
    let now = fsops::entry(&canonical).map_err(anyhow::Error::from)?;
    if catalog.file != Some(now.id) || now.id != found.id {
        return Err(FileOpError::Unsupported(
            "The catalog file was replaced since it was opened; open it again",
        ));
    }
    Ok(OperationLock { _file: file })
}
