//! A system RAWmakase has no file system calls for: nothing is moved.
use super::{Entry, FileId, FsError, VolumeFacts};
use std::ffi::{OsStr, OsString};
use std::path::Path;

pub(super) fn entry(_path: &Path) -> Result<Entry, FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn rename_exclusive(_from: &Path, _to: &Path) -> Result<(), FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn sync_dir(_dir: &Path) -> Result<(), FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn volume_facts(_path: &Path) -> Result<VolumeFacts, FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn handle_id(_file: &std::fs::File) -> Result<FileId, FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn open_folder(_path: &Path) -> Result<std::fs::File, FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn extra_names(_path: &Path) -> Result<Vec<OsString>, FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn read_extra(_path: &Path, _name: &OsStr) -> Result<Vec<u8>, FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn write_extra(
    _path: &Path,
    _file: Option<&std::fs::File>,
    _name: &OsStr,
    _value: &[u8],
) -> Result<(), FsError> {
    Err(FsError::Unsupported)
}
