//! Windows: `GetFileInformationByHandle` for entries, `MoveFileExW` without
//! `MOVEFILE_REPLACE_EXISTING` or `MOVEFILE_COPY_ALLOWED` for the exclusive
//! rename, the volume's drive type, flags and free space
//! (`GetDiskFreeSpaceExW`), a network drive's protocol
//! (`FileRemoteProtocolInfo`), and alternate data streams for extras
//! (`FindFirstStreamW`).
use super::{Entry, EntryKind, FileId, FsError, VolumeFacts, VolumeId, VolumeKind};
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::AsRawHandle as _;
use std::path::Path;
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND,
    ERROR_HANDLE_EOF, ERROR_INVALID_FUNCTION, ERROR_INVALID_NAME, ERROR_INVALID_PARAMETER,
    ERROR_NOT_SAME_DEVICE, ERROR_NOT_SUPPORTED, ERROR_PATH_NOT_FOUND, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_REMOTE_PROTOCOL_INFO, FILE_WRITE_ATTRIBUTES, FileRemoteProtocolInfo, FindClose,
    FindFirstStreamW, FindNextStreamW, FindStreamInfoStandard, GetDiskFreeSpaceExW, GetDriveTypeW,
    GetFileInformationByHandle, GetFileInformationByHandleEx, GetVolumeInformationW,
    GetVolumePathNameW, MOVEFILE_WRITE_THROUGH, MoveFileExW, WIN32_FIND_STREAM_DATA,
};
use windows_sys::Win32::Storage::InstallableFileSystems::WNNC_NET_SMB;
use windows_sys::Win32::System::SystemServices::FILE_READ_ONLY_VOLUME;
use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

/// `path` as a NUL-terminated wide string.
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// The error of the last call, as callers branch on it.
fn last_error() -> FsError {
    let e = std::io::Error::last_os_error();
    match e.raw_os_error().map(|c| c as u32) {
        Some(ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) => FsError::AlreadyExists,
        Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => FsError::NotFound,
        Some(ERROR_ACCESS_DENIED) => FsError::PermissionDenied,
        Some(ERROR_NOT_SAME_DEVICE) => FsError::CrossDevice,
        Some(ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED) => FsError::Unsupported,
        _ => FsError::from(e),
    }
}

pub(super) fn entry(path: &Path) -> Result<Entry, FsError> {
    let t = std::fs::symlink_metadata(path)?.file_type();
    // Opened for no access, so a photo another program has open can still
    // be identified, and as the reparse point itself rather than its target.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let (id, links) = information(&file)?;
    let kind = if t.is_symlink() {
        EntryKind::Symlink
    } else if t.is_dir() {
        EntryKind::Directory
    } else if t.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    Ok(Entry { id, kind, links })
}

/// An open file's file object and how many names it has.
fn information(file: &std::fs::File) -> Result<(FileId, u64), FsError> {
    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value of
    // the plain C struct, which the call then fills.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the duration of the call, and `info`
    // is a valid, writable struct of the type the call expects.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(last_error());
    }
    let id = FileId {
        volume: VolumeId(u64::from(info.dwVolumeSerialNumber)),
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    };
    Ok((id, u64::from(info.nNumberOfLinks)))
}

/// The kind of file system `name`, as `GetVolumeInformationW` names it.
/// ReFS's 64-bit file index isn't promised unique, so it is another kind.
/// Not for a network drive, which names the server's ([`remote_kind`]).
pub(super) fn kind_of(name: &str) -> VolumeKind {
    match name.to_ascii_uppercase().as_str() {
        "NTFS" => VolumeKind::Ntfs,
        "FAT" | "FAT12" | "FAT16" | "FAT32" => VolumeKind::Fat,
        "EXFAT" => VolumeKind::Exfat,
        _ => VolumeKind::Other,
    }
}

/// The kind of a network drive reached over `protocol` (a `WNNC_NET_`
/// network type, as `FileRemoteProtocolInfo` names it; `None` where it
/// can't be read): an SMB share, or another kind. Its file system's name
/// is the server's, `NTFS` from a Synology and from Samba alike (measured
/// for #691), so it tells nothing.
pub(super) fn remote_kind(protocol: Option<u32>) -> VolumeKind {
    match protocol {
        Some(WNNC_NET_SMB) => VolumeKind::Smb,
        _ => VolumeKind::Other,
    }
}

/// The network protocol `path` is reached over, from a handle opened for
/// no access; `None` for a local file, which has none.
fn protocol(path: &Path) -> Result<Option<u32>, FsError> {
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    // SAFETY: an all-zero FILE_REMOTE_PROTOCOL_INFO is a valid value of the
    // plain C struct, which the call fills.
    let mut info: FILE_REMOTE_PROTOCOL_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the duration of the call, and `info`
    // is a writable struct of the size given, of the type the class needs.
    let read = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileRemoteProtocolInfo,
            (&raw mut info).cast(),
            size_of::<FILE_REMOTE_PROTOCOL_INFO>() as u32,
        )
    };
    Ok((read != 0).then_some(info.Protocol))
}

/// The file object an open file is.
pub(super) fn handle_id(file: &std::fs::File) -> Result<FileId, FsError> {
    Ok(information(file)?.0)
}

pub(super) fn rename_exclusive(from: &Path, to: &Path) -> Result<(), FsError> {
    // An SMB server is asked not to replace, but needn't keep to it: raced
    // against an exclusive create through Linux's CIFS, such a rename
    // replaced the file made, against Samba 4.22 and a Synology alike
    // (Windows 11, #691).
    if protocol(from)? == Some(WNNC_NET_SMB) {
        return Err(FsError::Unsupported);
    }
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both are NUL-terminated wide strings that outlive the call.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// NTFS journals renames, and a folder can't be opened to sync it.
pub(super) fn sync_dir(dir: &Path) -> Result<(), FsError> {
    if !std::fs::metadata(dir)?.is_dir() {
        return Err(FsError::NotFound);
    }
    Ok(())
}

pub(super) fn volume_facts(at: &Path) -> Result<VolumeFacts, FsError> {
    let path = wide(at);
    let mut root = vec![0u16; 1024];
    // SAFETY: `path` is NUL-terminated and `root` holds the length given.
    if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return Err(last_error());
    }
    let mut flags = 0u32;
    let mut kind = vec![0u16; 64];
    // SAFETY: `root` is the NUL-terminated volume path the call above
    // wrote; the volume name buffer is null with length 0, which the call
    // allows, `kind` holds the length given, and `flags` is a valid place
    // for the flags.
    let read = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut flags,
            kind.as_mut_ptr(),
            kind.len() as u32,
        )
    };
    if read == 0 {
        return Err(last_error());
    }
    // SAFETY: `root` is NUL-terminated.
    let remote = unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE;
    let mut available = 0u64;
    // SAFETY: `root` is NUL-terminated; `available` is a valid place for
    // the count, and the other two counts are null, which the call allows.
    let read = unsafe {
        GetDiskFreeSpaceExW(
            root.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if read == 0 {
        return Err(last_error());
    }
    let kind = match remote {
        true => remote_kind(protocol(at)?),
        false => kind_of(&String::from_utf16_lossy(
            &kind[..kind.iter().position(|c| *c == 0).unwrap_or(kind.len())],
        )),
    };
    Ok(VolumeFacts {
        kind,
        local: !remote,
        read_only: flags & FILE_READ_ONLY_VOLUME != 0,
        // Not tried on Windows' volumes yet.
        renames_spelling: false,
        stable_ids: !remote && kind.keeps_ids(),
        exclusive_rename: kind.exclusive_rename(),
        available,
    })
}

/// A folder, opened to set its times: only that access, through the
/// handle a folder needs.
pub(super) fn open_folder(path: &Path) -> Result<std::fs::File, FsError> {
    Ok(std::fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?)
}

/// The names of `path`'s extras: its alternate data streams (named
/// `$DATA` streams, `Zone.Identifier` and the like), without the unnamed
/// one, its contents. None where its volume keeps none (FAT, exFAT).
pub(super) fn extra_names(path: &Path) -> Result<Vec<OsString>, FsError> {
    let path = wide(path);
    let mut names = Vec::new();
    // SAFETY: an all-zero WIN32_FIND_STREAM_DATA is a valid value of the
    // plain C struct, which the calls fill.
    let mut found: WIN32_FIND_STREAM_DATA = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and outlives the call; `found` is a
    // valid, writable struct of the type the standard level writes.
    let search = unsafe {
        FindFirstStreamW(
            path.as_ptr(),
            FindStreamInfoStandard,
            (&raw mut found).cast(),
            0,
        )
    };
    if search == INVALID_HANDLE_VALUE {
        // No named stream, or (`ERROR_INVALID_PARAMETER`, as the call's
        // documentation says) a volume without streams.
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(code) if [ERROR_HANDLE_EOF, ERROR_INVALID_PARAMETER].contains(&(code as u32)) => {
                Ok(names)
            }
            _ => match last_error() {
                FsError::Unsupported => Ok(names),
                e => Err(e),
            },
        };
    }
    let listed = loop {
        // `:name:$DATA`; the contents are `::$DATA`.
        let name = &found.cStreamName;
        let name = &name[..name.iter().position(|c| *c == 0).unwrap_or(0)];
        let data: Vec<u16> = ":$DATA".encode_utf16().collect();
        let named = name
            .strip_prefix(&[u16::from(b':')][..])
            .and_then(|n| n.strip_suffix(&data[..]))
            .filter(|n| !n.is_empty());
        if let Some(named) = named {
            names.push(OsString::from_wide(named));
        }
        // SAFETY: `search` is the open search handle; `found` as above.
        if unsafe { FindNextStreamW(search, (&raw mut found).cast()) } == 0 {
            break match std::io::Error::last_os_error().raw_os_error() {
                Some(code) if code as u32 == ERROR_HANDLE_EOF => Ok(names),
                _ => Err(last_error()),
            };
        }
    };
    // SAFETY: `search` is open, and closed once.
    unsafe { FindClose(search) };
    listed
}

/// Alternate data stream `name` of `path`, as a path: after its last name,
/// never after a separator (`folder\:name` names a file in it).
fn stream(path: &Path, name: &OsStr) -> std::path::PathBuf {
    let path: std::path::PathBuf = path.components().collect();
    let mut at = path.into_os_string();
    at.push(":");
    at.push(name);
    at.into()
}

/// The contents of `path`'s alternate data stream `name`.
pub(super) fn read_extra(path: &Path, name: &OsStr) -> Result<Vec<u8>, FsError> {
    Ok(std::fs::read(stream(path, name))?)
}

/// Gives `path` the alternate data stream `name` holding `value`, durably,
/// replacing one it has. [`FsError::Unsupported`] where its volume keeps
/// none, which takes the stream's name for an invalid one. By its path,
/// whatever handle to it there is, as `CreateFileW` opens a stream
/// (`NtCreateFile` relative to a handle, or `BackupWrite`, might not need
/// it locally; neither is measured, and SMB names the file in each open
/// anyway): on a share, a file another computer put at its name takes it,
/// as Linux's CIFS gives one an extended attribute. The copy then reads
/// back otherwise, and stays for the user.
pub(super) fn write_extra(
    path: &Path,
    _file: Option<&std::fs::File>,
    name: &OsStr,
    value: &[u8],
) -> Result<(), FsError> {
    use std::io::Write as _;
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(stream(path, name))
        .and_then(|mut file| {
            file.write_all(value)?;
            file.sync_all()
        });
    match written {
        Err(e) if e.raw_os_error().map(|c| c as u32) == Some(ERROR_INVALID_NAME) => {
            Err(FsError::Unsupported)
        }
        written => Ok(written?),
    }
}
