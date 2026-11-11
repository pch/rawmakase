//! Windows: unnamed file mappings backed by the paging file, duplicated into
//! the program before their line is sent, and closed there again with
//! `DUPLICATE_CLOSE_SOURCE` when the line never goes.
use std::io::{self, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::Arc;
use windows_sys::Win32::Foundation::{
    DUPLICATE_CLOSE_SOURCE, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// The program's process, which the handles are duplicated into. Its handle
/// has `PROCESS_DUP_HANDLE` access, as `CreateProcessW` gives its creator all.
#[derive(Clone)]
pub(crate) struct Child(pub(crate) Arc<OwnedHandle>);

/// One buffer: its mapping handle, whether the program writes it, and an
/// output's read-only view.
pub(crate) struct Memory {
    handle: OwnedHandle,
    write: bool,
    view: Option<View>,
}
impl Memory {
    /// An input, written by `fill` through a view that is gone before the
    /// program sees it. The program gets a read-only duplicate.
    pub(crate) fn input(len: usize, fill: impl FnOnce(&mut [u8])) -> io::Result<Self> {
        let handle = create(len)?;
        let mut view = View::map(&handle, len, true)?;
        fill(view.bytes_mut());
        drop(view);
        Ok(Self {
            handle,
            write: false,
            view: None,
        })
    }

    /// An output, zeroed, mapped read-only for the app.
    pub(crate) fn output(len: usize) -> io::Result<Self> {
        let handle = create(len)?;
        let view = View::map(&handle, len, false)?;
        Ok(Self {
            handle,
            write: true,
            view: Some(view),
        })
    }

    pub(crate) fn contents(&self) -> Option<&[u8]> {
        self.view.as_ref().map(View::bytes)
    }

    /// Duplicates the handle into `process`, not inheritable, read-only for
    /// an input and read-write for an output. Returns its value there.
    pub(crate) fn duplicate_into(&self, process: HANDLE) -> io::Result<u64> {
        let access = if self.write {
            FILE_MAP_READ | FILE_MAP_WRITE
        } else {
            FILE_MAP_READ
        };
        let mut duplicate: HANDLE = std::ptr::null_mut();
        // SAFETY: a handle this process owns and a process handle with
        // PROCESS_DUP_HANDLE; the result is checked.
        let done = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
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
}

/// A view of a mapping, unmapped when dropped.
struct View {
    address: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
}
// SAFETY: the view is plain shared memory, owned by this value alone.
unsafe impl Send for View {}
// SAFETY: shared references give out only `&[u8]`.
unsafe impl Sync for View {}
impl View {
    fn map(handle: &OwnedHandle, len: usize, write: bool) -> io::Result<Self> {
        let access = if write { FILE_MAP_WRITE } else { FILE_MAP_READ };
        // SAFETY: a mapping handle this process owns; the result is checked.
        let address = unsafe { MapViewOfFile(handle.as_raw_handle(), access, 0, 0, len) };
        if address.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { address, len })
    }
    fn bytes(&self) -> &[u8] {
        // SAFETY: `len` bytes mapped readable for this value's life; the
        // mapping's size can't change.
        unsafe { std::slice::from_raw_parts(self.address.Value.cast(), self.len) }
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: mapped writable (inputs only), borrowed mutably through
        // this value alone.
        unsafe { std::slice::from_raw_parts_mut(self.address.Value.cast(), self.len) }
    }
}
impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: unmaps the view `map` mapped; nothing borrows it now.
        unsafe { UnmapViewOfFile(self.address) };
    }
}

/// A new, unnamed file mapping of `len` bytes, not inheritable.
fn create(len: usize) -> io::Result<OwnedHandle> {
    let len = len as u64;
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

/// The duplicates one line lists in `hv`, already in the program. Until the
/// line's first byte is written they are the app's to close, which dropping
/// this does in the program (`DUPLICATE_CLOSE_SOURCE`); after it the program
/// owns them.
pub(crate) struct Passing {
    child: Child,
    values: Vec<u64>,
    sent: bool,
}
impl Passing {
    pub(super) fn new(items: &[Memory], child: &Child) -> io::Result<Self> {
        let mut passing = Self {
            child: child.clone(),
            values: Vec::with_capacity(items.len()),
            sent: false,
        };
        for item in items {
            // On a failure the duplicates made so far close as `passing` drops.
            let value = item.duplicate_into(child.0.as_raw_handle())?;
            passing.values.push(value);
        }
        Ok(passing)
    }
    /// What the line lists in `hv`.
    pub(crate) fn values(&self) -> Option<Vec<u64>> {
        Some(self.values.clone())
    }
    pub(crate) fn count(&self) -> usize {
        self.values.len()
    }
}
impl Drop for Passing {
    fn drop(&mut self) {
        if self.sent {
            return;
        }
        for &value in &self.values {
            // SAFETY: closes, in the program, a duplicate this app made there
            // for a line never sent, so the program never learned its value.
            unsafe {
                DuplicateHandle(
                    self.child.0.as_raw_handle(),
                    value as usize as HANDLE,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                    0,
                    DUPLICATE_CLOSE_SOURCE,
                );
            }
        }
    }
}

/// The host's sending end.
pub(crate) struct Writer(pub(crate) Box<dyn Write + Send>);
impl Writer {
    /// Writes one whole line. Once its first byte is written, its handles are
    /// the program's; until then a failure closes them in the program.
    pub(crate) fn send(&mut self, bytes: &[u8], passing: Option<Passing>) -> io::Result<()> {
        let mut rest = bytes;
        if let Some(mut passing) = passing
            && !rest.is_empty()
        {
            let written = loop {
                match self.0.write(rest) {
                    Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
                    Ok(n) => break n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            };
            passing.sent = true;
            rest = &rest[written..];
        }
        self.0.write_all(rest)?;
        self.0.flush()
    }
}
