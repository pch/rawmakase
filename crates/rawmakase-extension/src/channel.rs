//! The program's end of the message channel, which the app created before
//! starting it.
//!
//! On Linux and macOS the app passes one end of a socket pair as file
//! descriptor 3 and names it in `RAWMAKASE_EXTENSION_FD`. On Windows it names a
//! named pipe in `RAWMAKASE_EXTENSION_PIPE`, which the program opens once; the
//! app accepts only its own child there.
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// The channel's two directions, each usable from its own thread.
pub struct Channel {
    /// Messages from the app.
    pub reader: Box<dyn Read + Send>,
    /// Messages to the app.
    pub writer: Box<dyn Write + Send>,
}

/// The channel is taken once: a second [`attach`] would own the same descriptor
/// or pipe twice.
static ATTACHED: AtomicBool = AtomicBool::new(false);

/// The channel the app started this program with. Fails when the program
/// wasn't started by the app (the variable is missing) or was attached already.
pub fn attach() -> io::Result<Channel> {
    if ATTACHED.swap(true, Ordering::SeqCst) {
        return Err(io::Error::other(
            "the extension channel is attached already",
        ));
    }
    platform::attach()
}

fn missing(variable: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{variable} is not set: extension programs are started by RAWmakase"),
    )
}

#[cfg(unix)]
mod platform {
    use super::*;
    use rawmakase_extension_protocol::ENV_FD;
    use std::os::fd::FromRawFd;
    use std::os::unix::net::UnixStream;

    pub(super) fn attach() -> io::Result<Channel> {
        let fd: i32 = std::env::var(ENV_FD)
            .map_err(|_| missing(ENV_FD))?
            .parse()
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{ENV_FD} is not a number"),
                )
            })?;
        if fd < 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{ENV_FD} names a standard stream"),
            ));
        }
        // SAFETY: the app passed this descriptor, an end of a socket pair, for
        // the channel alone, and `ATTACHED` makes this the only place that ever
        // takes ownership of it.
        let socket = unsafe { UnixStream::from_raw_fd(fd) };
        // The clones are close-on-exec, and dropping `socket` closes the
        // inherited descriptor, so a helper the program starts doesn't hold the
        // channel open after the program exits.
        let reader = socket.try_clone()?;
        let writer = socket.try_clone()?;
        drop(socket);
        Ok(Channel {
            reader: Box::new(reader),
            writer: Box::new(writer),
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use rawmakase_extension_protocol::ENV_PIPE;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::Arc;
    use windows_sys::Win32::Foundation::{
        ERROR_BROKEN_PIPE, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, GetLastError,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, SECURITY_IDENTIFICATION,
        SECURITY_SQOS_PRESENT, WriteFile,
    };
    use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::CreateEventW;

    pub(super) fn attach() -> io::Result<Channel> {
        let name = std::env::var_os(ENV_PIPE).ok_or_else(|| missing(ENV_PIPE))?;
        if !name.to_string_lossy().starts_with(r"\\.\pipe\") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{ENV_PIPE} is not a local pipe"),
            ));
        }
        let wide: Vec<u16> = name.encode_wide().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated name; the returned handle is
        // checked before it is owned. Identification-level impersonation
        // only: the app can't act as this program.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a valid handle this function just opened and owns alone.
        let pipe = Arc::new(unsafe { OwnedHandle::from_raw_handle(handle) });
        Ok(Channel {
            reader: Box::new(Overlapped::new(pipe.clone())?),
            writer: Box::new(Overlapped::new(pipe)?),
        })
    }

    /// One direction of a pipe opened for overlapped I/O: each direction has
    /// its own event, so a read waiting for the app never holds up a write.
    struct Overlapped {
        pipe: Arc<OwnedHandle>,
        event: OwnedHandle,
    }
    impl Overlapped {
        fn new(pipe: Arc<OwnedHandle>) -> io::Result<Self> {
            // SAFETY: a manual-reset event with no name or security.
            let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                pipe,
                // SAFETY: a valid handle just created and owned here alone.
                event: unsafe { OwnedHandle::from_raw_handle(event) },
            })
        }

        /// Starts one operation and waits for it, returning the bytes moved.
        fn run(
            &mut self,
            start: impl FnOnce(*mut core::ffi::c_void, *mut OVERLAPPED) -> i32,
        ) -> io::Result<usize> {
            // SAFETY: an all-zero OVERLAPPED is its documented initial state.
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            overlapped.hEvent = self.event.as_raw_handle();
            let handle = self.pipe.as_raw_handle();
            if start(handle, &mut overlapped) == 0 {
                // SAFETY: reads this thread's last error.
                let error = unsafe { GetLastError() };
                if error != ERROR_IO_PENDING {
                    return Err(io::Error::from_raw_os_error(error as i32));
                }
            }
            let mut moved = 0u32;
            // SAFETY: `overlapped` lives on this frame until the operation
            // it describes has completed, which this call waits for.
            if unsafe { GetOverlappedResult(handle, &overlapped, &mut moved, 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(moved as usize)
        }
    }
    impl Read for Overlapped {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let len = buf.len().min(u32::MAX as usize) as u32;
            let ptr = buf.as_mut_ptr();
            // SAFETY: `buf` outlives the operation, which `run` waits for.
            match self.run(|h, o| unsafe { ReadFile(h, ptr, len, std::ptr::null_mut(), o) }) {
                Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => Ok(0),
                other => other,
            }
        }
    }
    impl Write for Overlapped {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let len = buf.len().min(u32::MAX as usize) as u32;
            let ptr = buf.as_ptr();
            // SAFETY: `buf` outlives the operation, which `run` waits for.
            self.run(|h, o| unsafe { WriteFile(h, ptr, len, std::ptr::null_mut(), o) })
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::*;
    pub(super) fn attach() -> io::Result<Channel> {
        let _ = missing;
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}
