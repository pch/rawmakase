//! Windows: file mappings the app duplicated into this process, adopted from
//! the values a line lists in `hv`, mapped with `MapViewOfFile`; and, for the
//! mock host, made and duplicated as the app does.
//!
//! An input arrives as a duplicate with `FILE_MAP_READ` access alone, so this
//! process can't map it writable; an output with read and write access. No
//! duplicate is inheritable.
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// A handle the app passed, owned by this process.
pub(crate) type Handle = OwnedHandle;

/// Takes ownership of the handles a line lists in `hv`. The caller has
/// checked that they are distinct.
pub(crate) fn adopt(values: &[u64]) -> Vec<Handle> {
    values
        .iter()
        .map(|&value| {
            // SAFETY: the app duplicated this handle into this process for
            // this message alone (docs/extensions/protocol.md), so nothing
            // else here owns it.
            unsafe { OwnedHandle::from_raw_handle(value as usize as HANDLE) }
        })
        .collect()
}

/// A mapped view of a buffer, unmapped when dropped.
pub(crate) struct View {
    address: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
    write: bool,
}
// SAFETY: the view is plain shared memory, owned by this value alone.
unsafe impl Send for View {}

impl View {
    /// Maps `len` bytes of `handle`, writable when `write`. Fails when the
    /// mapping is smaller, or when the handle lacks the access, as an input's
    /// read-only duplicate does for writing.
    pub(crate) fn new(handle: &Handle, len: usize, write: bool) -> io::Result<Self> {
        let access = if write { FILE_MAP_WRITE } else { FILE_MAP_READ };
        // SAFETY: a mapping handle this process owns; the result is checked.
        let address = unsafe { MapViewOfFile(handle.as_raw_handle(), access, 0, 0, len) };
        if address.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            address,
            len,
            write,
        })
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        // SAFETY: `len` bytes mapped readable for this value's life.
        unsafe { std::slice::from_raw_parts(self.address.Value.cast(), self.len) }
    }

    pub(crate) fn bytes_mut(&mut self) -> Option<&mut [u8]> {
        // SAFETY: `len` bytes mapped writable for this value's life, borrowed
        // mutably through it alone.
        self.write
            .then(|| unsafe { std::slice::from_raw_parts_mut(self.address.Value.cast(), self.len) })
    }
}
impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: unmaps the view `new` mapped; nothing borrows it now.
        unsafe { UnmapViewOfFile(self.address) };
    }
}

/// Shared memory the mock host allocates, as the app does.
pub(crate) struct Memory {
    handle: OwnedHandle,
    write: bool,
    /// An output's view, for reading what the program wrote.
    view: Option<View>,
}
impl Memory {
    /// An input holding `bytes`.
    pub(crate) fn input(bytes: &[u8]) -> io::Result<Self> {
        let handle = create(bytes.len())?;
        if !bytes.is_empty() {
            let mut view = View::new(&handle, bytes.len(), true)?;
            if let Some(target) = view.bytes_mut() {
                target.copy_from_slice(bytes);
            }
        }
        Ok(Self {
            handle,
            write: false,
            view: None,
        })
    }

    /// An output of `len` bytes, mapped to read.
    pub(crate) fn output(len: usize) -> io::Result<Self> {
        let handle = create(len)?;
        let view = View::new(&handle, len.max(1), false)?;
        Ok(Self {
            handle,
            write: true,
            view: Some(view),
        })
    }

    /// Duplicates the handle into this process as the app duplicates it into
    /// the program: read-only for an input, not inheritable. The duplicate is
    /// the receiver's to close.
    pub(crate) fn duplicate(&self) -> io::Result<u64> {
        let access = if self.write {
            FILE_MAP_READ | FILE_MAP_WRITE
        } else {
            FILE_MAP_READ
        };
        let mut duplicate: HANDLE = std::ptr::null_mut();
        // SAFETY: a handle this process owns, duplicated within it; the
        // result is checked.
        let done = unsafe {
            let process = GetCurrentProcess();
            DuplicateHandle(
                process,
                self.handle.as_raw_handle(),
                process,
                &mut duplicate,
                access,
                0,
                0,
            )
        };
        if done == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(duplicate as usize as u64)
    }

    /// What an output holds.
    pub(crate) fn contents(&self) -> &[u8] {
        self.view.as_ref().map_or(&[], View::bytes)
    }
}

/// Closes duplicates [`Memory::duplicate`] made whose message wasn't sent.
pub(crate) fn close(values: &[u64]) {
    drop(adopt(values));
}

/// A new, unnamed file mapping of `len` bytes backed by the paging file.
fn create(len: usize) -> io::Result<OwnedHandle> {
    let len = len.max(1) as u64;
    // SAFETY: no file, no name, default security; the result is checked.
    let handle = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            (len >> 32) as u32,
            len as u32,
            std::ptr::null(),
        )
    };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a handle just created, owned here alone.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
