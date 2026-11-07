//! Unix: `lstat` for entries, `renameat2(RENAME_NOREPLACE)` on Linux and
//! `renameatx_np(RENAME_EXCL)` on macOS through rustix, `statvfs` and
//! `statfs` for volumes (`f_bavail * f_frsize` the bytes available), and
//! extended attributes for extras, never following a link.
use super::{Entry, EntryKind, FileId, FsError, VolumeFacts, VolumeId, VolumeKind};
use std::ffi::{OsStr, OsString};
#[cfg(any(target_os = "linux", target_os = "android"))]
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

pub(super) fn entry(path: &Path) -> Result<Entry, FsError> {
    let m = std::fs::symlink_metadata(path)?;
    let t = m.file_type();
    let kind = if t.is_symlink() {
        EntryKind::Symlink
    } else if t.is_dir() {
        EntryKind::Directory
    } else if t.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    Ok(Entry {
        id: FileId {
            volume: VolumeId(m.dev()),
            index: m.ino(),
        },
        kind,
        links: m.nlink(),
    })
}

/// The file object an open file is.
pub(super) fn handle_id(file: &std::fs::File) -> Result<FileId, FsError> {
    let m = file.metadata()?;
    Ok(FileId {
        volume: VolumeId(m.dev()),
        index: m.ino(),
    })
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
pub(super) fn rename_exclusive(from: &Path, to: &Path) -> Result<(), FsError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    // Linux's CIFS checks `RENAME_NOREPLACE` only against what this
    // computer has seen, and the server replaces: raced against an
    // exclusive create through another mount, the rename replaced that
    // file every time (Linux 7.2 against Samba 4.22). macOS's smbfs
    // answers ENOTSUP itself.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let dir = from.parent().filter(|d| !d.as_os_str().is_empty());
        if kind(dir.unwrap_or(Path::new(".")))? == VolumeKind::Smb {
            return Err(FsError::Unsupported);
        }
    }
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(errno)
}

/// Whether `path`'s folder is on a FUSE file system on a disk of this
/// computer (`fuseblk`), where [`super::rename_reserving`] renames a file
/// by a placeholder. ntfs-3g, how Linux Mint and Ubuntu mount an NTFS
/// drive, is one; it answers `EINVAL` to `RENAME_NOREPLACE`, as FUSE does
/// for a file system without `rename2` (#914).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn reserves(path: &Path) -> Result<bool, FsError> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    let dir = dir.unwrap_or(Path::new("."));
    let fs = rustix::fs::statfs(dir).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    #[allow(
        clippy::unnecessary_cast,
        reason = "the type differs between architectures"
    )]
    let magic = fs.f_type as u64;
    Ok(on_disk_fuse(magic, std::fs::metadata(dir)?.dev()))
}

/// Elsewhere no rename is by a placeholder.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(super) fn reserves(_path: &Path) -> Result<bool, FsError> {
    Ok(false)
}

/// Whether file system `magic` with a file of device number `dev` is FUSE
/// on a disk (`fuseblk`) rather than one with an anonymous device (a
/// network or virtual FUSE file system, GVFS's and sshfs's among them).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn on_disk_fuse(magic: u64, dev: u64) -> bool {
    magic == 0x6573_5546 && rustix::fs::major(dev) != 0
}

/// Other Unix systems have no exclusive rename rustix reaches.
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
pub(super) fn rename_exclusive(_from: &Path, _to: &Path) -> Result<(), FsError> {
    Err(FsError::Unsupported)
}

/// What a failed rename's error means here. A file system without the
/// exclusive flag answers `EINVAL` on Linux and `ENOTSUP` on macOS (exFAT,
/// some network shares).
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn errno(e: rustix::io::Errno) -> FsError {
    use rustix::io::Errno;
    match e {
        Errno::EXIST | Errno::NOTEMPTY => FsError::AlreadyExists,
        Errno::NOENT => FsError::NotFound,
        Errno::ACCESS | Errno::PERM => FsError::PermissionDenied,
        Errno::XDEV => FsError::CrossDevice,
        Errno::INVAL | Errno::NOTSUP | Errno::NOSYS => FsError::Unsupported,
        e => FsError::Io(e.into()),
    }
}

pub(super) fn sync_dir(dir: &Path) -> Result<(), FsError> {
    let folder = std::fs::File::open(dir)?;
    match folder.sync_all() {
        // macOS's SMB shares take no full sync of a folder (measured on
        // macOS 15), whose names the server made as they were made; there
        // `fsync(2)` is all there is.
        #[cfg(target_vendor = "apple")]
        Err(e) if e.raw_os_error() == Some(rustix::io::Errno::NOTSUP.raw_os_error()) => {
            rustix::fs::fsync(&folder).map_err(|e| std::io::Error::from(e).into())
        }
        synced => Ok(synced?),
    }
}

pub(super) fn volume_facts(path: &Path) -> Result<VolumeFacts, FsError> {
    use rustix::fs::{StatVfsMountFlags, statvfs};
    let vfs = statvfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    let local = is_local(path)?;
    let kind = kind(path)?;
    Ok(VolumeFacts {
        kind,
        local,
        read_only: vfs.f_flag.contains(StatVfsMountFlags::RDONLY),
        // Case-insensitive APFS renames a name to another spelling of
        // itself in place (tested in
        // `a_case_only_rename_works_where_the_volume_renames_spelling`);
        // elsewhere a volume that folds names may find the new spelling
        // taken (case-sensitive APFS, for accents), or rename nothing.
        renames_spelling: kind == VolumeKind::Apfs && !case_sensitive(path),
        stable_ids: local && kind.keeps_ids(),
        exclusive_rename: kind.exclusive_rename(),
        available: vfs.f_bavail.saturating_mul(vfs.f_frsize),
    })
}

/// Whether the volume tells names apart by case; a case-sensitive APFS
/// volume still folds accents.
#[cfg(target_vendor = "apple")]
fn case_sensitive(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    // SAFETY: `path` is a valid NUL-terminated string for the call.
    unsafe { libc::pathconf(path.as_ptr(), libc::_PC_CASE_SENSITIVE) != 0 }
}

/// Only macOS volumes are taken to rename spelling.
#[cfg(not(target_vendor = "apple"))]
fn case_sensitive(_path: &Path) -> bool {
    true
}

/// macOS names the file system.
#[cfg(target_vendor = "apple")]
pub(super) fn kind(path: &Path) -> Result<VolumeKind, FsError> {
    let fs = rustix::fs::statfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    let name: Vec<u8> = fs
        .f_fstypename
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    Ok(kind_of(&name))
}

/// The kind of macOS's file system `name` (`f_fstypename`): FSKit's and the
/// kernel's FAT and exFAT have the same names; `smbfs` is an SMB share.
#[cfg(target_vendor = "apple")]
pub(super) fn kind_of(name: &[u8]) -> VolumeKind {
    match name {
        b"apfs" => VolumeKind::Apfs,
        b"hfs" => VolumeKind::Hfs,
        b"msdos" => VolumeKind::Fat,
        b"exfat" => VolumeKind::Exfat,
        b"smbfs" => VolumeKind::Smb,
        _ => VolumeKind::Other,
    }
}

/// Linux numbers the file system type.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn kind(path: &Path) -> Result<VolumeKind, FsError> {
    let fs = rustix::fs::statfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    #[allow(
        clippy::unnecessary_cast,
        reason = "the type differs between architectures"
    )]
    let kind = fs.f_type as u64;
    Ok(kind_of(kind))
}

/// The kind of Linux's file system type `magic` (`f_type`). FUSE (exFAT
/// through exfat-fuse and GVFS's shares among others) is another kind.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn kind_of(magic: u64) -> VolumeKind {
    match magic {
        0xef53 => VolumeKind::Ext,
        0x5846_5342 => VolumeKind::Xfs,
        0x9123_683e => VolumeKind::Btrfs,
        0x4d44 => VolumeKind::Fat,
        0x2011_bab0 => VolumeKind::Exfat,
        // CIFS, SMB2 and the old SMB.
        0xff53_4d42 | 0xfe53_4d42 | 0x517b => VolumeKind::Smb,
        _ => VolumeKind::Other,
    }
}

/// Elsewhere the file system can't be told.
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
pub(super) fn kind(_path: &Path) -> Result<VolumeKind, FsError> {
    Ok(VolumeKind::Other)
}

/// macOS says so in the mount's flags.
#[cfg(target_vendor = "apple")]
fn is_local(path: &Path) -> Result<bool, FsError> {
    let fs = rustix::fs::statfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    Ok(fs.f_flags & libc::MNT_LOCAL as u32 != 0)
}

/// Linux names the file system type: network ones, and FUSE, which GVFS
/// shares and sshfs are, are not local.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn is_local(path: &Path) -> Result<bool, FsError> {
    const REMOTE: [u64; 11] = [
        0x6969,      // NFS
        0x517b,      // SMB
        0xff53_4d42, // CIFS
        0xfe53_4d42, // SMB2
        0x6573_5546, // FUSE
        0x5346_414f, // AFS
        0x00c3_6400, // Ceph
        0x7375_7245, // Coda
        0x0102_1997, // 9P
        0x0bd0_0bd0, // Lustre
        0x0000_564c, // NCP
    ];
    let fs = rustix::fs::statfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    #[allow(
        clippy::unnecessary_cast,
        reason = "the type differs between architectures"
    )]
    let kind = fs.f_type as u64;
    Ok(!REMOTE.contains(&kind))
}

/// Elsewhere it can't be told, so no volume is taken for local.
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn is_local(_path: &Path) -> Result<bool, FsError> {
    Ok(false)
}

/// A folder, opened to set its times and permission bits, which its owner
/// may through any handle.
pub(super) fn open_folder(path: &Path) -> Result<std::fs::File, FsError> {
    Ok(std::fs::File::open(path)?)
}

/// Whether extended attribute `name` is the file's own, which a move
/// carries: on macOS all but those the system sets itself for whichever
/// process made a file (`com.apple.provenance`, `com.apple.macl`), protects
/// (`com.apple.rootless`, `com.apple.security.`, `com.apple.system.`), or
/// that are marked never to be kept (an `N` after the last `#`,
/// `xattr_name_with_flags(3)`, but for a suffix also holding `n`, which
/// `xattr_flags_from_name` takes for no flags at all).
#[cfg(target_vendor = "apple")]
pub(super) fn carried(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    let system = [
        "com.apple.provenance",
        "com.apple.macl",
        "com.apple.rootless",
    ];
    let protected = ["com.apple.security.", "com.apple.system."];
    !system.contains(&name.as_ref())
        && !protected.iter().any(|p| name.starts_with(p))
        && !name
            .rsplit_once('#')
            .is_some_and(|(_, f)| f.contains('N') && !f.contains('n'))
}

/// On Linux only the user's own (`user.`): `security.` labels are the
/// destination's to give, `system.` ones are access lists, and `trusted.`
/// ones need privileges.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn carried(name: &OsStr) -> bool {
    name.as_bytes().starts_with(b"user.")
}

/// The names of `path`'s extras: its extended attributes a move carries.
/// None where its volume keeps none.
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
pub(super) fn extra_names(path: &Path) -> Result<Vec<OsString>, FsError> {
    use rustix::fs::{llistxattr, llistxattr_query};
    use rustix::io::Errno;
    // Asked for its size, then read; again should it grow between.
    let list = loop {
        let size = match llistxattr_query(path) {
            Ok(size) => size,
            Err(e) if unsupported(e) => return Ok(Vec::new()),
            Err(e) => return Err(io(e)),
        };
        let mut list = vec![0u8; size];
        match llistxattr(path, &mut list[..]) {
            Ok(len) => {
                list.truncate(len);
                break list;
            }
            Err(Errno::RANGE) => continue,
            Err(e) => return Err(io(e)),
        }
    };
    Ok(list
        .split(|b| *b == 0)
        .filter(|n| !n.is_empty())
        .map(|n| OsString::from_vec(n.to_vec()))
        .filter(|n| carried(n))
        .collect())
}

/// The value of `path`'s extended attribute `name`, whole (a resource
/// fork too).
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
pub(super) fn read_extra(path: &Path, name: &OsStr) -> Result<Vec<u8>, FsError> {
    use rustix::fs::{lgetxattr, lgetxattr_query};
    use rustix::io::Errno;
    loop {
        let size = lgetxattr_query(path, name).map_err(io)?;
        let mut value = vec![0u8; size];
        match lgetxattr(path, name, &mut value[..]) {
            Ok(len) => {
                value.truncate(len);
                return Ok(value);
            }
            Err(Errno::RANGE) => continue,
            Err(e) => return Err(io(e)),
        }
    }
}

/// Gives `path` the extended attribute `name`, replacing one it has:
/// through `file`, a handle to it, where given (a resource fork and
/// FinderInfo too on macOS, whose `fsetxattr(2)` takes them as
/// `setxattr(2)` does).
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
pub(super) fn write_extra(
    path: &Path,
    file: Option<&std::fs::File>,
    name: &OsStr,
    value: &[u8],
) -> Result<(), FsError> {
    use rustix::fs::{XattrFlags, fsetxattr, lsetxattr};
    match file {
        Some(file) => fsetxattr(file, name, value, XattrFlags::empty()),
        None => lsetxattr(path, name, value, XattrFlags::empty()),
    }
    .map_err(io)
}

/// Other Unix systems: no extended attributes rustix reaches, so what a
/// file has can't be told (nor is a copy made to another volume there).
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
pub(super) fn extra_names(_path: &Path) -> Result<Vec<OsString>, FsError> {
    Err(FsError::Unsupported)
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
pub(super) fn read_extra(_path: &Path, _name: &OsStr) -> Result<Vec<u8>, FsError> {
    Err(FsError::Unsupported)
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
pub(super) fn write_extra(
    _path: &Path,
    _file: Option<&std::fs::File>,
    _name: &OsStr,
    _value: &[u8],
) -> Result<(), FsError> {
    Err(FsError::Unsupported)
}

/// A failed call's error as callers branch on it: a volume without
/// extended attributes as [`FsError::Unsupported`].
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn io(e: rustix::io::Errno) -> FsError {
    match unsupported(e) {
        true => FsError::Unsupported,
        false => FsError::from(std::io::Error::from(e)),
    }
}

/// Whether `e` says the volume has no extended attributes: `ENOTSUP`
/// (which std doesn't take for unsupported on macOS) or `EOPNOTSUPP`.
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn unsupported(e: rustix::io::Errno) -> bool {
    use rustix::io::Errno;
    e == Errno::NOTSUP || e == Errno::OPNOTSUPP
}
