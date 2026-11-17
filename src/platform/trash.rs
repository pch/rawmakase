//! The system Trash, for file operations: macOS's through `NSFileManager`,
//! Windows' Recycle Bin through `IFileOperation`, and elsewhere the
//! freedesktop.org Trash, written here. Each fails closed: a file is in the
//! Trash, with a receipt naming where, or it is where it was. Where a volume
//! has no Trash the answer is [`TrashError::Unsupported`]; nothing is ever
//! deleted, or copied and deleted, instead.
//!
//! This is the trusted boundary the catalog's `Trash` seam documents: a
//! receipt says the system moved the file. The catalog still removes rows
//! only once the file is gone from its path as well.
use crate::catalog::file_ops::{Trash, TrashError, TrashReceipt, TrashSupport};
use std::path::Path;

#[cfg(any(all(unix, not(target_vendor = "apple")), all(test, unix)))]
mod freedesktop;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// The system's Trash.
pub(crate) struct SystemTrash;

impl Trash for SystemTrash {
    fn trash(&self, path: &Path) -> Result<TrashReceipt, TrashError> {
        system(path)
    }
    fn supports(&self, path: &Path) -> TrashSupport {
        system_supports(path)
    }
}

#[cfg(target_os = "macos")]
fn system_supports(path: &Path) -> TrashSupport {
    macos::supports(path)
}

#[cfg(windows)]
fn system_supports(path: &Path) -> TrashSupport {
    windows::supports(path)
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn system_supports(path: &Path) -> TrashSupport {
    freedesktop::supports(path, &places())
}

#[cfg(not(any(unix, windows)))]
fn system_supports(_path: &Path) -> TrashSupport {
    TrashSupport::No
}

#[cfg(target_os = "macos")]
fn system(path: &Path) -> Result<TrashReceipt, TrashError> {
    macos::trash(path)
}

#[cfg(windows)]
fn system(path: &Path) -> Result<TrashReceipt, TrashError> {
    windows::trash(path)
}

/// The freedesktop Trash of the user running the app: under
/// `$XDG_DATA_HOME` (by default `~/.local/share`), and at the top of other
/// mounts.
#[cfg(all(unix, not(target_vendor = "apple")))]
fn system(path: &Path) -> Result<TrashReceipt, TrashError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| TrashError::Failed(e.to_string()))?;
    let date = freedesktop::deletion_date(now.as_secs() as i64);
    freedesktop::trash(path, &places(), &date)
}

/// Where the Trashes of the user running the app are.
#[cfg(all(unix, not(target_vendor = "apple")))]
fn places() -> freedesktop::Places<'static> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|d| d.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .filter(|h| h.is_absolute())
                .map(|h| h.join(".local/share"))
        });
    freedesktop::Places {
        home: data.as_deref().and_then(freedesktop::Home::under),
        uid: rustix::process::getuid().as_raw(),
        top: &freedesktop::mount_top,
    }
}

/// Other systems have no Trash this knows.
#[cfg(not(any(unix, windows)))]
fn system(_path: &Path) -> Result<TrashReceipt, TrashError> {
    Err(TrashError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::SystemTrash;
    use crate::catalog::file_ops::{Trash, TrashError, TrashReceipt, TrashSupport};

    /// The real Trash takes real files, the user's own on a desktop: a
    /// plain `cargo test` would leave one in it per run. These run on CI,
    /// where the Trash is the runner's and thrown away, or when asked with
    /// `RAWMAKASE_TRASH_TESTS=1`; the files are made here, in a temporary
    /// folder, and nothing else is ever trashed.
    fn real_trash() -> bool {
        std::env::var_os("CI").is_some_and(|v| !v.is_empty())
            || std::env::var_os("RAWMAKASE_TRASH_TESTS").is_some_and(|v| v == "1")
    }

    /// One real trashing at a time. The app trashes one item after
    /// another; two at once, on a fresh Windows runner, once had the shell
    /// refuse to recycle one of them. This lock holds within one test
    /// process; under cargo-nextest, which runs each test in its own
    /// process, the `real-trash` test group in `.config/nextest.toml` keeps
    /// these tests apart.
    fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
        static TRASHING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        TRASHING.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn a_synthetic_file_goes_to_the_trash_with_a_receipt() {
        if !real_trash() {
            return;
        }
        let _one = one_at_a_time();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rawmakase-trash-test.jpg");
        let bytes = b"RAWmakase trash test, synthetic";
        std::fs::write(&path, bytes).unwrap();
        let receipt = SystemTrash.trash(&path).unwrap();
        assert!(!path.exists());
        assert!(receipt.location.exists(), "{receipt:?}");
        assert_eq!(std::fs::read(&receipt.location).unwrap(), bytes);
        // Out of the Trash again, so runs don't pile up there.
        std::fs::remove_file(&receipt.location).unwrap();
    }

    /// The catalog keeps a root as canonicalised, which on Windows is a
    /// verbatim path (`\\?\C:\...`), and its photos' paths below it (#744).
    #[test]
    fn a_file_named_by_its_canonical_path_goes_to_the_trash() {
        if !real_trash() {
            return;
        }
        let _one = one_at_a_time();
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let path = canonical.join("rawmakase-trash-test.jpg");
        std::fs::write(&path, b"canonical").unwrap();
        let receipt = SystemTrash.trash(&path).unwrap();
        assert!(!path.exists());
        assert_eq!(std::fs::read(&receipt.location).unwrap(), b"canonical");
        std::fs::remove_file(&receipt.location).unwrap();
    }

    #[test]
    fn a_synthetic_folder_goes_to_the_trash_with_a_receipt() {
        if !real_trash() {
            return;
        }
        let _one = one_at_a_time();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("rawmakase-trash-test");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.jpg"), b"a").unwrap();
        let receipt = SystemTrash.trash(&folder).unwrap();
        assert!(!folder.exists());
        assert_eq!(std::fs::read(receipt.location.join("a.jpg")).unwrap(), b"a");
        std::fs::remove_dir_all(&receipt.location).unwrap();
    }

    /// Asking writes nothing, so it runs everywhere: a temporary folder,
    /// on the drive of the user's own files, has a Trash, or one that
    /// can't be told before trying.
    #[test]
    fn a_temporary_folder_is_not_on_a_drive_without_a_trash() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.jpg");
        std::fs::write(&file, b"a").unwrap();
        for path in [dir.path(), &file] {
            let got = SystemTrash.supports(path);
            assert_ne!(got, TrashSupport::No, "{}", path.display());
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// On the volume `RAWMAKASE_TEST_OTHER_VOLUME` names (a disk image, a
    /// card): what asking says, and, with `RAWMAKASE_TRASH_TESTS=1`, what
    /// the Trash then does with a synthetic folder made there. A `No` must
    /// be a refusal, a `Yes` a receipt. Run with `--nocapture` to record
    /// the answers.
    #[test]
    #[ignore = "needs RAWMAKASE_TEST_OTHER_VOLUME: a writable folder on another volume"]
    fn another_volume_says_what_its_trash_does() {
        let Some(other) = std::env::var_os("RAWMAKASE_TEST_OTHER_VOLUME") else {
            return;
        };
        let other = std::path::Path::new(&other);
        println!(
            "{}: supports {:?}",
            other.display(),
            SystemTrash.supports(other)
        );
        if !real_trash() {
            return;
        }
        let _one = one_at_a_time();
        let there = tempfile::tempdir_in(other).unwrap();
        let folder = there.path().join("rawmakase-trash-test");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.jpg"), b"a").unwrap();
        // Of the folder itself: `other` may be the top of its mount, which
        // can't go.
        let asked = SystemTrash.supports(&folder);
        println!("{}: supports {asked:?}", folder.display());
        let got = SystemTrash.trash(&folder);
        // What kind of answer, not the receipt: its path names the user.
        let took = match &got {
            Ok(_) => "a receipt",
            Err(TrashError::Unsupported) => "unsupported",
            Err(TrashError::Failed(_)) => "failed",
        };
        println!("{}: trash {took}", other.display());
        match asked {
            TrashSupport::No => assert_eq!(took, "unsupported"),
            TrashSupport::Yes => assert_eq!(took, "a receipt"),
            TrashSupport::Unknown => {}
        }
        if let Ok(receipt) = got {
            assert!(!folder.exists());
            std::fs::remove_dir_all(&receipt.location).unwrap();
        }
    }

    /// A Trash that doesn't say can't be told before trying.
    #[test]
    fn a_trash_that_does_not_say_is_unknown() {
        struct Silent;
        impl Trash for Silent {
            fn trash(&self, _: &std::path::Path) -> Result<TrashReceipt, TrashError> {
                Err(TrashError::Unsupported)
            }
        }
        assert_eq!(
            Silent.supports(std::path::Path::new("/")),
            TrashSupport::Unknown
        );
    }

    /// An SMB share has no Trash on macOS (measured on macOS 15 against a
    /// Synology share), and Linux's and Windows' (a folder redirected to a
    /// share has a Recycle Bin there) aren't used, as no rename there can
    /// promise not to replace a file: the file stays, and nothing is
    /// deleted or made there. Run when `RAWMAKASE_TEST_NETWORK_SHARE` names
    /// a writable folder on one; on Windows, also with it redirected to as
    /// a known folder (Pictures, say).
    #[test]
    #[ignore = "needs RAWMAKASE_TEST_NETWORK_SHARE: a writable folder on an SMB share"]
    fn a_file_on_an_smb_share_is_kept_as_its_volume_has_no_trash() {
        let Some(share) = std::env::var_os("RAWMAKASE_TEST_NETWORK_SHARE") else {
            return;
        };
        let dir = tempfile::tempdir_in(share).unwrap();
        let path = dir.path().join("rawmakase-trash-test.jpg");
        std::fs::write(&path, b"RAWmakase trash test, synthetic").unwrap();
        // macOS 15 can't tell before trying: Unknown, never Yes. Linux and
        // Windows say no.
        let asked = SystemTrash.supports(&path);
        assert_ne!(asked, TrashSupport::Yes);
        assert!(cfg!(target_os = "macos") || asked == TrashSupport::No);
        let got = SystemTrash.trash(&path);
        assert!(matches!(got, Err(TrashError::Unsupported)), "{got:?}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"RAWmakase trash test, synthetic"
        );
    }

    /// Nothing to trash: an error, and no Trash is touched.
    #[test]
    fn a_missing_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let got = SystemTrash.trash(&dir.path().join("missing.jpg"));
        assert!(matches!(got, Err(TrashError::Failed(_))), "{got:?}");
    }
}
