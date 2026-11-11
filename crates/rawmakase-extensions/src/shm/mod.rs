//! Shared memory for the buffers of the app's requests: allocating it, leasing
//! it against the budgets, and passing its handles to the program.
//!
//! The app allocates every buffer, in both directions; the program only maps
//! them. The unsafe code is in `unix.rs` (Linux and macOS) and `windows.rs`,
//! one file per system.
//!
//! | | Allocates | An input | An output |
//! | --- | --- | --- | --- |
//! | Linux | `memfd_create`, `ftruncate` | written, unmapped, then sealed against writing, shrinking and growing | sealed against shrinking and growing |
//! | macOS | `shm_open` with a random name, unlinked at once, `ftruncate` | not sealable; the app never reads it back | its size fixed |
//! | Windows | `CreateFileMappingW` with no name | duplicated into the program with `FILE_MAP_READ` alone | duplicated with read and write access |
//!
//! Handles travel one way, from the app to the program: on Linux and macOS as
//! `SCM_RIGHTS` on the `sendmsg` carrying a line's first byte, on Windows
//! duplicated into the program, not inheritable, and listed in the line's
//! `hv`. The host's reader on Linux and macOS receives with `recvmsg` and
//! closes at once any descriptor a program sends; that is a protocol fault
//! (`Session` then hangs the program up).
//!
//! A request's [`Buffers`] are its lease: their bytes count against
//! [`MAX_REQUEST_BYTES`] and the session's [`Ledger`]
//! ([`MAX_EXTENSION_BYTES`]) from before they are allocated until they are
//! dropped.
use rawmakase_extension_protocol::buffer::{MAX_EXTENSION_BYTES, MAX_REQUEST_BYTES};
use rawmakase_extension_protocol::{ErrorCode, RemoteError};
use std::io;
use std::sync::{Arc, Mutex};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::*;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(test)]
mod tests;

/// The bytes one extension's requests hold in shared memory, live or
/// cancelled and not yet answered, against [`MAX_EXTENSION_BYTES`]. Clones
/// share the count, so the sessions of one extension's successive programs
/// may share one ledger.
#[derive(Clone, Debug, Default)]
pub struct Ledger(Arc<Mutex<u64>>);
impl Ledger {
    /// A ledger with nothing leased.
    pub fn new() -> Self {
        Self::default()
    }
    /// The bytes leased now.
    pub fn in_use(&self) -> u64 {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn take(&self, bytes: u64) -> Result<(), RemoteError> {
        let mut used = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if used.saturating_add(bytes) > MAX_EXTENSION_BYTES {
            return Err(RemoteError::new(
                ErrorCode::Busy,
                format!(
                    "the extension holds {} bytes of shared memory, and {bytes} more would pass its \
                     {MAX_EXTENSION_BYTES}-byte budget; its earlier requests have to end first",
                    *used
                ),
            ));
        }
        *used += bytes;
        Ok(())
    }
    fn give(&self, bytes: u64) {
        let mut used = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *used = used.saturating_sub(bytes);
    }
}

/// One request's buffers in shared memory, and their lease: their bytes count
/// in the ledger until this is dropped. The index each buffer is added at is
/// its handle's index in the message, which its descriptor names.
pub struct Buffers {
    ledger: Ledger,
    bytes: u64,
    items: Vec<Memory>,
}
impl std::fmt::Debug for Buffers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffers")
            .field("count", &self.items.len())
            .field("bytes", &self.bytes)
            .finish()
    }
}

impl Buffers {
    /// No buffers yet, leased against `ledger`.
    pub fn new(ledger: &Ledger) -> Self {
        Self {
            ledger: ledger.clone(),
            bytes: 0,
            items: Vec::new(),
        }
    }

    /// Adds an input the program reads, of `len` bytes, which `fill` writes.
    /// Once filled it can't change: the app's view is gone, and on Linux it
    /// is sealed. Returns its handle index.
    pub fn input(&mut self, len: u64, fill: impl FnOnce(&mut [u8])) -> Result<u32, RemoteError> {
        self.add(len, |len| Memory::input(len, fill))
    }

    /// Adds an output the program writes, of `len` bytes, zeroed. Returns its
    /// handle index.
    pub fn output(&mut self, len: u64) -> Result<u32, RemoteError> {
        self.add(len, Memory::output)
    }

    /// What an output holds; `None` for an input or an index past the last.
    /// Read it once the program has answered: until then it may still write.
    pub fn read(&self, index: u32) -> Option<&[u8]> {
        self.items.get(index as usize).and_then(Memory::contents)
    }

    /// How many buffers there are.
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// Their bytes together.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Leases `len` more bytes, refused before anything is allocated when the
    /// request or the extension would pass its budget, then allocates.
    fn add(
        &mut self,
        len: u64,
        allocate: impl FnOnce(usize) -> io::Result<Memory>,
    ) -> Result<u32, RemoteError> {
        if len == 0 {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                "a buffer of no bytes",
            ));
        }
        if self.items.len() >= rawmakase_extension_protocol::MAX_HANDLES {
            return Err(RemoteError::new(
                ErrorCode::TooLarge,
                format!(
                    "a message carries at most {} handles",
                    rawmakase_extension_protocol::MAX_HANDLES
                ),
            ));
        }
        let total = self.bytes.saturating_add(len);
        if total > MAX_REQUEST_BYTES {
            return Err(RemoteError::new(
                ErrorCode::TooLarge,
                format!(
                    "{total} bytes of buffers pass a request's {MAX_REQUEST_BYTES}-byte budget"
                ),
            ));
        }
        self.ledger.take(len)?;
        let size = usize::try_from(len).map_err(|_| {
            RemoteError::new(ErrorCode::TooLarge, "a buffer too large for this system")
        });
        match size.and_then(|size| {
            allocate(size).map_err(|e| {
                RemoteError::new(
                    ErrorCode::Unavailable,
                    format!("shared memory couldn't be allocated: {e}"),
                )
            })
        }) {
            Ok(memory) => {
                self.items.push(memory);
                self.bytes = total;
                Ok(self.items.len() as u32 - 1)
            }
            Err(e) => {
                self.ledger.give(len);
                Err(e)
            }
        }
    }

    /// The handles to send with the request's line, in index order: copies
    /// the transport closes once sent, or on Windows duplicates already in
    /// the program, closed there again if the line never goes.
    pub(crate) fn pass(&self, child: &Child) -> io::Result<Passing> {
        Passing::new(&self.items, child)
    }
}
impl Drop for Buffers {
    fn drop(&mut self) {
        // The views and handles close before the bytes leave the ledger.
        self.items.clear();
        self.ledger.give(self.bytes);
    }
}

/// A successful request's buffers, handed over with its reply: read what the
/// program wrote ([`Returned::read`]), then drop it. The lease ends when the
/// last clone is dropped, so it can travel to the thread that copies out.
#[derive(Clone)]
pub struct Returned(Arc<Buffers>);
impl Returned {
    pub(crate) fn new(buffers: Buffers) -> Self {
        Self(Arc::new(buffers))
    }
    /// What output `index` holds; `None` for an input or an index past the
    /// last.
    pub fn read(&self, index: u32) -> Option<&[u8]> {
        self.0.read(index)
    }
    /// The buffers' bytes together, still leased.
    pub fn bytes(&self) -> u64 {
        self.0.bytes()
    }
}
impl PartialEq for Returned {
    /// The same lease.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl std::fmt::Debug for Returned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
