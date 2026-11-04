//! Windows: the Recycle Bin through the shell's `IFileOperation`, deleting
//! with `FOFX_RECYCLEONDELETE` and no UI. Its progress sink refuses, before
//! anything happens, a delete the shell would not recycle (no Recycle Bin
//! on the volume, a file too big for it) and keeps the item the Recycle Bin
//! made, whose path is the receipt. A refused delete is
//! [`TrashError::Unsupported`]; any other failure, abort or a delete without
//! a recycled item is [`TrashError::Failed`]. Neither deletes the file.
//!
//! Whether a file's volume has a Recycle Bin is told from its drive type:
//! removable, network, CD and RAM drives have none; on a fixed drive the
//! bin may be turned off or too small, which only the delete tells. A file
//! on an SMB share is refused before the shell is asked, even where it
//! would recycle it (a folder redirected there has a bin, the share's
//! `$RECYCLE.BIN`, measured on Windows 11, #691): that bin is filled by a
//! rename on the server, which can take a file another computer put at the
//! name since it was checked, so it isn't used, as on Linux.
use crate::catalog::file_ops::{TrashError, TrashReceipt, TrashSupport};
use rawmakase_fs as fsops;
use std::cell::RefCell;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{E_ABORT, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOFX_RECYCLEONDELETE,
    FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl,
    IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH, TSF_DELETE_RECYCLE_IF_POSSIBLE,
};
use windows::core::{ComObject, HRESULT, HSTRING, PCWSTR, Ref, implement};

pub(super) fn trash(path: &Path) -> Result<TrashReceipt, TrashError> {
    // Refused too where the volume can't be read but is a network drive
    // (one dropped since it was checked, say), failing closed.
    let share = match fsops::volume_facts(path) {
        Ok(f) => f.kind == fsops::VolumeKind::Smb,
        Err(_) => supports(path) == TrashSupport::No,
    };
    if share {
        return Err(TrashError::Unsupported);
    }
    let _com = Com::init()?;
    let seen = recycle(path).map_err(|e| TrashError::Failed(said(&e)))?;
    match seen {
        Seen { refused: true, .. } => Err(TrashError::Unsupported),
        Seen {
            failed: Some(e), ..
        } => Err(TrashError::Failed(e)),
        Seen { aborted: true, .. } => Err(TrashError::Failed("the delete was cancelled".into())),
        Seen {
            recycled: Some(location),
            ..
        } => Ok(TrashReceipt { location }),
        Seen { .. } => Err(TrashError::Failed(
            "the Recycle Bin didn't say where the file went".into(),
        )),
    }
}

/// Whether `path` can go to the Recycle Bin, by its drive's type.
pub(super) fn supports(path: &Path) -> TrashSupport {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut root = vec![0u16; path.len().max(261) + 1];
    // SAFETY: `path` is NUL-terminated and `root` holds the length given.
    if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return TrashSupport::Unknown;
    }
    // SAFETY: `root` is the NUL-terminated volume path the call above
    // wrote.
    of_drive(unsafe { GetDriveTypeW(root.as_ptr()) })
}

/// What a drive of type `kind` (`GetDriveTypeW`) says: no Recycle Bin on
/// removable (USB sticks and cards, FAT and exFAT among them), network, CD
/// or RAM drives; a fixed one's may be turned off, or too small for what
/// goes, so it can't be told. A network folder that has a bin (a
/// redirected one) is refused too, and [`trash`] refuses one on an SMB
/// share.
fn of_drive(kind: u32) -> TrashSupport {
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
    };
    match kind {
        DRIVE_REMOVABLE | DRIVE_REMOTE | DRIVE_CDROM | DRIVE_RAMDISK => TrashSupport::No,
        _ => TrashSupport::Unknown,
    }
}

/// What one delete did, as its sink and the operation saw it.
#[derive(Default)]
struct Seen {
    /// The sink refused it: it would not have been recycled.
    refused: bool,
    aborted: bool,
    failed: Option<String>,
    recycled: Option<PathBuf>,
}

/// Deletes `path` to the Recycle Bin. An error is a step that didn't
/// happen; what the shell did is in [`Seen`].
///
/// The shell is given `path` without the `\\?\` prefix a catalog root
/// keeps (it is canonicalised), as it refuses those as an incorrect
/// parameter (#744). It parses a plain path its own way: a name only the
/// prefix kept literal (a trailing dot or space, on the file or a folder
/// on the way) may name another file, so the item it made is checked to
/// be `path`'s file before anything is deleted.
fn recycle(path: &Path) -> windows::core::Result<Seen> {
    let plain = crate::platform::volume::plain(path);
    let sink = ComObject::new(Sink::default());
    let progress: IFileOperationProgressSink = sink.to_interface();
    // SAFETY: COM is initialised on this thread (`Com`), every pointer the
    // calls take is a live interface or string borrowed for the call, and
    // the interfaces are released when dropped.
    let (performed, aborted) = unsafe {
        let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
        operation.SetOperationFlags(
            FOFX_RECYCLEONDELETE | FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT,
        )?;
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(plain.as_path()), None)?;
        if !names(&item, path)? {
            return Ok(Seen {
                failed: Some(format!(
                    "the Recycle Bin can't be given “{}” by another name",
                    path.display()
                )),
                ..Seen::default()
            });
        }
        operation.DeleteItem(&item, &progress)?;
        let performed = operation.PerformOperations();
        (performed, operation.GetAnyOperationsAborted()?.as_bool())
    };
    let mut seen = sink.seen.take();
    seen.aborted = aborted;
    if let Err(e) = performed
        && !seen.refused
    {
        seen.failed.get_or_insert(said(&e));
    }
    Ok(seen)
}

/// COM on this thread, single-threaded as `IFileOperation` needs, for as
/// long as it lives.
struct Com;

impl Com {
    fn init() -> Result<Self, TrashError> {
        // SAFETY: no reserved pointer; a success is paired with the
        // `CoUninitialize` in `drop`, on this thread.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if hr == RPC_E_CHANGED_MODE {
            return Err(TrashError::Failed(
                "this thread's COM is multithreaded, and the Recycle Bin needs it not to be".into(),
            ));
        }
        hr.ok()
            .map_err(|e| TrashError::Failed(said(&e)))
            .map(|()| Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: `init` succeeded on this thread (S_OK or S_FALSE), which
        // asks for one call each.
        unsafe { CoUninitialize() }
    }
}

/// Watches the delete: refuses one the shell wouldn't recycle, and keeps
/// what it did.
#[implement(IFileOperationProgressSink)]
#[derive(Default)]
struct Sink {
    seen: RefCell<Seen>,
}

impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn PreDeleteItem(&self, flags: u32, _item: Ref<IShellItem>) -> windows::core::Result<()> {
        if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
            self.seen.borrow_mut().refused = true;
            return Err(E_ABORT.into());
        }
        Ok(())
    }

    fn PostDeleteItem(
        &self,
        _flags: u32,
        _item: Ref<IShellItem>,
        result: HRESULT,
        recycled: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        // Any success, not only S_OK: the shell reports a recycled delete
        // with codes of its own; what makes it one is the item it made.
        // That is asked of the shell before `seen` is borrowed, so nothing
        // it calls back can find it borrowed.
        let found = result.is_ok().then(|| recycled.as_ref().map(file_path));
        let mut seen = self.seen.borrow_mut();
        let Some(found) = found else {
            seen.failed = Some(said(&windows::core::Error::from_hresult(result)));
            return Ok(());
        };
        match found {
            Some(Ok(path)) => seen.recycled = Some(path),
            Some(Err(e)) => seen.failed = Some(said(&e)),
            None => {
                seen.failed = Some(format!(
                    "the shell made no recycled item (0x{:08X})",
                    result.0 as u32
                ))
            }
        }
        Ok(())
    }

    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _result: HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: &PCWSTR,
        _: u32,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

/// What `e` says, with its code: the shell's own codes have no text.
fn said(e: &windows::core::Error) -> String {
    let code = e.code().0 as u32;
    match e.message() {
        m if m.trim().is_empty() => format!("the shell failed with 0x{code:08X}"),
        m => format!("{} (0x{code:08X})", m.trim()),
    }
}

/// Whether the shell's `item` is the entry at `path`, each read without
/// following a link it ends in: the same file object, under the same name
/// in the same folder. A link or another hard link of the file, where the
/// shell parsed the path otherwise, is a different entry: the shell would
/// recycle it and leave `path`.
fn names(item: &IShellItem, path: &Path) -> windows::core::Result<bool> {
    let named = file_path(item)?;
    let same = |a: &Path, b: &Path| match (fsops::entry(a), fsops::entry(b)) {
        (Ok(a), Ok(b)) => a.id == b.id && a.kind == b.kind,
        _ => false,
    };
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    Ok(match (named.parent(), path.parent()) {
        (Some(a), Some(b)) => {
            same(&named, path) && same(a, b) && name(&named).is_some() && name(&named) == name(path)
        }
        _ => false,
    })
}

/// Where `item` is on disk.
fn file_path(item: &IShellItem) -> windows::core::Result<PathBuf> {
    // SAFETY: `GetDisplayName` returns a string this owns; it is copied,
    // then freed once with `CoTaskMemFree`.
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(OsString::from_wide(name.as_wide()));
        CoTaskMemFree(Some(name.0 as *const _));
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::of_drive;
    use crate::catalog::file_ops::TrashSupport;
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
        DRIVE_UNKNOWN,
    };

    /// A name only the `\\?\` prefix keeps (a trailing dot) is not given
    /// to the shell without it, where it names another file: nothing goes.
    #[test]
    fn a_name_the_plain_path_would_change_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let dotted = canonical.join("a.jpg.");
        let other = canonical.join("a.jpg");
        std::fs::write(&dotted, b"dotted").unwrap();
        std::fs::write(&other, b"other").unwrap();
        let got = super::trash(&dotted);
        assert!(
            matches!(&got, Err(crate::catalog::file_ops::TrashError::Failed(m)) if m.contains("by another name")),
            "{got:?}"
        );
        assert_eq!(std::fs::read(&dotted).unwrap(), b"dotted");
        assert_eq!(std::fs::read(&other).unwrap(), b"other");
    }

    /// The same for a folder on the way: Windows' file calls keep the
    /// trailing space of `album `, the shell drops it and names `album`.
    #[test]
    fn a_folder_the_shell_would_name_otherwise_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let (spaced, other) = (canonical.join("album "), canonical.join("album"));
        std::fs::create_dir(&spaced).unwrap();
        std::fs::create_dir(&other).unwrap();
        std::fs::write(spaced.join("a.jpg"), b"spaced").unwrap();
        std::fs::write(other.join("a.jpg"), b"other").unwrap();
        let got = super::trash(&spaced.join("a.jpg"));
        assert!(
            matches!(&got, Err(crate::catalog::file_ops::TrashError::Failed(m)) if m.contains("by another name")),
            "{got:?}"
        );
        assert_eq!(std::fs::read(spaced.join("a.jpg")).unwrap(), b"spaced");
        assert_eq!(std::fs::read(other.join("a.jpg")).unwrap(), b"other");
    }

    /// Where the shell names `album\a.jpg` for `album \a.jpg`, a link or a
    /// hard link there to the photo is the same file once followed, but not
    /// the same entry: the shell would recycle it and leave the photo.
    #[test]
    fn a_link_to_the_file_where_the_shell_would_look_is_refused() {
        type Link = fn(&std::path::Path, &std::path::Path) -> std::io::Result<()>;
        let links: [(&str, Link); 2] = [
            ("symlink", |to, at| {
                std::os::windows::fs::symlink_file(to, at)
            }),
            ("hard link", |to, at| std::fs::hard_link(to, at)),
        ];
        for (kind, link) in links {
            let dir = tempfile::tempdir().unwrap();
            let canonical = dir.path().canonicalize().unwrap();
            let (spaced, other) = (canonical.join("album "), canonical.join("album"));
            std::fs::create_dir(&spaced).unwrap();
            std::fs::create_dir(&other).unwrap();
            let photo = spaced.join("a.jpg");
            std::fs::write(&photo, b"photo").unwrap();
            // Making a symlink takes a privilege (or Developer Mode).
            if let Err(e) = link(&photo, &other.join("a.jpg")) {
                println!("no {kind}: {e}");
                continue;
            }
            let got = super::trash(&photo);
            assert!(
                matches!(&got, Err(crate::catalog::file_ops::TrashError::Failed(m)) if m.contains("by another name")),
                "{kind}: {got:?}"
            );
            assert_eq!(std::fs::read(&photo).unwrap(), b"photo", "{kind}");
            assert!(
                std::fs::symlink_metadata(other.join("a.jpg")).is_ok(),
                "{kind}"
            );
        }
    }

    #[test]
    fn drives_without_a_recycle_bin_say_no() {
        for kind in [DRIVE_REMOVABLE, DRIVE_REMOTE, DRIVE_CDROM, DRIVE_RAMDISK] {
            assert_eq!(of_drive(kind), TrashSupport::No, "{kind}");
        }
        for kind in [DRIVE_FIXED, DRIVE_UNKNOWN, DRIVE_NO_ROOT_DIR] {
            assert_eq!(of_drive(kind), TrashSupport::Unknown, "{kind}");
        }
    }
}
