use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
use tempfile::{NamedTempFile, PersistError};

/// Per-user data: ~/Library/Application Support/RAWmakase on macOS,
/// %APPDATA%\RAWmakase on Windows, and $XDG_DATA_HOME/rawmakase (default
/// ~/.local/share/rawmakase) elsewhere. RAWMAKASE_DATA_DIR overrides all of them.
pub use rawmakase_protocol::paths::data_dir;
use rawmakase_protocol::paths::{home, xdg_data_home};

/// This computer's own data folder, never synced: the platform default even
/// when RAWMAKASE_DATA_DIR points elsewhere (a shared or synced folder), and
/// Windows's local rather than roaming AppData.
pub fn local_data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/RAWmakase")
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("RAWmakase"))
            .unwrap_or_else(|| home().join(".local/share/rawmakase"))
    } else {
        xdg_data_home().join("rawmakase")
    }
}

/// What persisting does when the destination already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replace {
    Overwrite,
    /// Fail instead, even if another writer creates the file meanwhile.
    NoClobber,
}
/// Writes `path` whole or not at all: `write` fills a temporary file beside
/// it, which is synced and renamed over it, and the folder is synced.
pub fn write_atomic(
    path: &Path,
    replace: Replace,
    write: impl FnOnce(&mut NamedTempFile) -> Result<()>,
) -> Result<()> {
    persist(stage(parent_dir(path), write)?, path, replace)
}
/// A synced temporary file in `dir` holding what `write` wrote, for
/// [`persist`] to put in place later.
pub fn stage(
    dir: &Path,
    write: impl FnOnce(&mut NamedTempFile) -> Result<()>,
) -> Result<NamedTempFile> {
    let mut file = NamedTempFile::new_in(dir)?;
    write(&mut file)?;
    file.as_file().sync_all()?;
    Ok(file)
}
/// Renames a file from [`stage`] to `path`, in the same folder, durably.
pub fn persist(staged: NamedTempFile, path: &Path, replace: Replace) -> Result<()> {
    match replace {
        Replace::Overwrite => staged.persist(path),
        Replace::NoClobber => persist_noclobber(staged, path),
    }
    .map_err(|e| e.error)?;
    sync_dir(parent_dir(path))
}
/// [`NamedTempFile::persist_noclobber`], also on drives that support neither
/// an exclusive rename nor hard links: exFAT on macOS refuses both with
/// ENOTSUP. There the staged file is copied into a file created only if no
/// file had the name, so it is never replaced; but a crash while copying
/// leaves that file partly written.
pub fn persist_noclobber(staged: NamedTempFile, path: &Path) -> Result<fs::File, PersistError> {
    or_claim(staged.persist_noclobber(path), path)
}
/// Moves `from` to `to` unless a file has that name, as [`persist_noclobber`].
pub fn rename_noclobber(from: &Path, to: &Path) -> io::Result<()> {
    match fs::hard_link(from, to) {
        Ok(()) => {}
        Err(e) if unsupported(&e) => drop(claim(from, to)?),
        Err(e) => return Err(e),
    }
    // `to` is in place; a leftover `from` is only clutter.
    let _ = fs::remove_file(from);
    Ok(())
}
/// `attempt`, or when the volume could not make it, the same by [`claim`].
fn or_claim(
    attempt: Result<fs::File, PersistError>,
    path: &Path,
) -> Result<fs::File, PersistError> {
    match attempt {
        // Dropping the staged file deletes it.
        Err(e) if unsupported(&e.error) => {
            claim(e.file.path(), path).map_err(|error| PersistError {
                error,
                file: e.file,
            })
        }
        attempt => attempt,
    }
}
/// A copy of `from` at `to`, made only if no file had that name. Renaming
/// over an empty file made first would replace whatever another writer put
/// there meanwhile.
fn claim(from: &Path, to: &Path) -> io::Result<fs::File> {
    let mut source = fs::File::open(from)?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(to)?;
    match io::copy(&mut source, &mut file).and_then(|_| file.sync_all()) {
        Ok(()) => Ok(file),
        Err(e) => {
            remove_if_still(file, to);
            Err(e)
        }
    }
}
/// Deletes `path` if it is still `file`, rather than one another writer put
/// in its place. Nothing deletes a path only if it is a given file, so a
/// writer replacing it between the check and the deletion still loses its
/// file; keeping ours instead would leave a truncated photo under the name
/// whenever a drive fills up or is unplugged mid-copy.
fn remove_if_still(file: fs::File, path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let same = match (file.metadata(), fs::symlink_metadata(path)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        };
        if !same {
            return;
        }
    }
    drop(file);
    let _ = fs::remove_file(path);
}
/// Whether `e` says the volume cannot do what was asked, rather than that it
/// failed.
fn unsupported(e: &io::Error) -> bool {
    // macOS's ENOTSUP has no ErrorKind of its own.
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::ENOTSUP) {
        return true;
    }
    // ERROR_INVALID_FUNCTION and ERROR_NOT_SUPPORTED: what Windows answers
    // a hard link with on FAT and exFAT.
    #[cfg(windows)]
    if matches!(e.raw_os_error(), Some(1 | 50)) {
        return true;
    }
    e.kind() == io::ErrorKind::Unsupported
}
pub fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    fs::create_dir_all(parent_dir(path))?;
    write_atomic(path, Replace::Overwrite, |f| {
        serde_json::to_writer_pretty(&mut *f, value)?;
        Ok(f.write_all(b"\n")?)
    })
}
/// The JSON at `path`, or the default when it is missing or unreadable.
pub fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
/// Makes a rename into `dir` durable. Unix only: Windows cannot open a folder
/// as a file, and NTFS journals the rename itself.
pub fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}
pub fn list_raws(path: &Path) -> Result<Vec<PathBuf>> {
    let dir = if path.is_dir() {
        path
    } else {
        parent_dir(path)
    };
    let mut files = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_raw(p) && p.is_file())
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}
/// Camera RAW extensions RAWmakase opens as RAW files, lower case.
pub(super) const RAW_EXTENSIONS: [&str; 28] = [
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "gpr", "iiq", "k25", "kdc",
    "mef", "mos", "mrw", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf",
    "srw", "x3f",
];
/// Dot files, including the "._NAME" AppleDouble files macOS writes next to
/// every file on exFAT/FAT drives; they share the photo's extension but are
/// only metadata.
pub fn is_hidden(p: &Path) -> bool {
    p.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
}
pub fn is_raw(p: &Path) -> bool {
    !is_hidden(p)
        && p.extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| RAW_EXTENSIONS.iter().any(|ext| v.eq_ignore_ascii_case(ext)))
}

/// Search locations for user-installed assets, including the legacy Linux location.
pub fn asset_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![data_dir()];
    let standard = xdg_data_home().join("rawmakase");
    if !dirs.contains(&standard) {
        dirs.push(standard);
    }
    dirs
}

/// A bare filename is relative to the current directory, not an empty directory.
pub fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_raw_is_known_by_its_extension_in_any_case_unless_hidden() {
        for name in ["A.NEF", "a.nef", "A.Nef", "dir.x/B.DNG", "C.cr3", "D.X3F"] {
            assert!(is_raw(Path::new(name)), "{name}");
        }
        for name in [
            "A.jpg",
            "A.JPEG",
            "A.png",
            "A.tif",
            "A.TIFF",
            "A.xmp",
            "A.ori",
            "A.heic",
            "A.webp",
            "NEF",
            "A.",
            ".A.NEF",
            "._A.NEF",
            "dir/._A.NEF",
        ] {
            assert!(!is_raw(Path::new(name)), "{name}");
        }
    }
    /// A volume that, like exFAT on macOS, can neither rename exclusively nor
    /// link: the publish it refuses.
    #[cfg(unix)]
    fn refused(file: NamedTempFile) -> Result<fs::File, PersistError> {
        Err(PersistError {
            error: io::Error::from_raw_os_error(libc::ENOTSUP),
            file,
        })
    }
    #[cfg(unix)]
    fn staged(dir: &Path, bytes: &[u8]) -> NamedTempFile {
        stage(dir, |f| Ok(f.write_all(bytes)?)).unwrap()
    }
    #[cfg(unix)]
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            // Without the "._" files macOS keeps beside each file on exFAT.
            .filter(|n| !n.starts_with("._"))
            .collect();
        names.sort();
        names
    }
    #[test]
    #[cfg(unix)]
    fn a_volume_without_exclusive_rename_still_gets_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.jpg");
        or_claim(refused(staged(dir.path(), b"photo")), &path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"photo");
        assert_eq!(names(dir.path()), ["out.jpg"]);
    }
    #[test]
    #[cfg(unix)]
    fn a_volume_without_exclusive_rename_never_replaces_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.jpg");
        fs::write(&path, b"previous").unwrap();
        let e = or_claim(refused(staged(dir.path(), b"photo")), &path).unwrap_err();
        assert_eq!(e.error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        // The staged file comes back whole, for a caller to publish elsewhere.
        assert_eq!(fs::read(e.file.path()).unwrap(), b"photo");
        let other = dir.path().join("out-2.jpg");
        persist_noclobber(e.file, &other).unwrap();
        assert_eq!(fs::read(&other).unwrap(), b"photo");
        assert_eq!(names(dir.path()), ["out-2.jpg", "out.jpg"]);
    }
    #[test]
    #[cfg(unix)]
    fn a_failure_other_than_unsupported_is_not_retried() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.jpg");
        let attempt = Err(PersistError {
            error: io::Error::from(io::ErrorKind::PermissionDenied),
            file: staged(dir.path(), b"photo"),
        });
        let e = or_claim(attempt, &path).unwrap_err();
        assert_eq!(e.error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!path.exists());
    }
    #[test]
    #[cfg(unix)]
    fn a_failed_claim_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.jpg");
        let missing = dir.path().join("gone");
        let e = claim(&missing, &path).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(names(dir.path()).is_empty());
    }
    #[test]
    #[cfg(unix)]
    fn a_failed_claim_never_deletes_what_another_writer_put_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.jpg");
        let ours = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        // Another writer replaces the claimed file before ours is cleaned up.
        let theirs = dir.path().join("theirs");
        fs::write(&theirs, b"theirs").unwrap();
        fs::rename(&theirs, &path).unwrap();
        remove_if_still(ours, &path);
        assert_eq!(fs::read(&path).unwrap(), b"theirs");
        // Our own file, still in place, goes.
        fs::remove_file(&path).unwrap();
        let ours = fs::File::create(&path).unwrap();
        remove_if_still(ours, &path);
        assert!(!path.exists());
    }
    #[test]
    fn a_path_is_moved_only_to_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"previous").unwrap();
        let e = rename_noclobber(&from, &to).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"previous");
        fs::remove_file(&to).unwrap();
        rename_noclobber(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"new");
        assert!(!from.exists());
    }
    #[test]
    fn every_listed_raw_extension_is_lower_case_and_listed_once() {
        for ext in RAW_EXTENSIONS {
            assert_eq!(ext, ext.to_ascii_lowercase());
            assert_eq!(RAW_EXTENSIONS.iter().filter(|e| **e == ext).count(), 1);
            assert!(is_raw(Path::new(&format!(
                "A.{}",
                ext.to_ascii_uppercase()
            ))));
        }
    }
}
