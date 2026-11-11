//! The shared memory the app passes with a request: receiving its handles in
//! order, and mapping the buffers they hold for the handler's call. The mock
//! host allocates and sends them as the app does, so tests go through the same
//! receiving code as a program the app started.
//!
//! The unsafe code is in `unix.rs` (Linux and macOS) and `windows.rs`, one file
//! per system.
//!
//! - **Linux, macOS:** descriptors arrive as `SCM_RIGHTS` beside the line's
//!   first byte. [`Incoming`] takes each one into an owning [`Handle`] as
//!   `recvmsg` returns, before it looks at any flag, and queues it; a line
//!   declaring `handles: n` takes the next `n`.
//! - **Windows:** the app duplicates the handles into this process and lists
//!   their values in the line's `hv`, which `adopt` takes ownership of.
//!
//! A [`Mapping`] lives for one handler call: the SDK drops it, and the handles,
//! after the handler returns and before it replies.
use rawmakase_extension_protocol::buffer::{Access, Buffer};
use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// The buffers mapped right now, in this process.
static MAPPED: AtomicUsize = AtomicUsize::new(0);

/// How many of the app's buffers the SDK has mapped right now. A handler's
/// mappings are gone once it has returned, so between requests this is 0.
pub fn mapped_buffers() -> usize {
    MAPPED.load(Ordering::SeqCst)
}

/// The receiving end of the channel.
pub(crate) trait Receive: Send {
    /// Reads bytes into `buf`, putting the handles that arrived with them at
    /// the back of `handles`. Fails with [`truncated`] when more handles came
    /// than fit, after taking those that did.
    fn receive(&mut self, buf: &mut [u8], handles: &mut VecDeque<Handle>) -> io::Result<usize>;
    /// Ends the channel both ways, so the app sees it close.
    fn shutdown(&self) {}
}

/// A channel that carries bytes alone: a pipe, or a test's reader.
pub(crate) struct Plain<R>(pub(crate) R);
impl<R: Read + Send> Receive for Plain<R> {
    fn receive(&mut self, buf: &mut [u8], _: &mut VecDeque<Handle>) -> io::Result<usize> {
        self.0.read(buf)
    }
}

/// Why [`Receive::receive`] failed when handles were cut off.
#[derive(Debug)]
struct Truncated;
impl std::fmt::Display for Truncated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("more handles arrived than one message carries (MSG_CTRUNC)")
    }
}
impl std::error::Error for Truncated {}

/// The error [`Receive::receive`] fails with when handles were cut off.
#[cfg_attr(
    windows,
    expect(dead_code, reason = "handles reach Windows in the line")
)]
pub(crate) fn truncated() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, Truncated)
}

/// The channel as a byte stream for the line reader, keeping the handles that
/// arrive in the order they arrived.
pub(crate) struct Incoming {
    channel: Box<dyn Receive>,
    /// Handles received and not yet given to a line.
    pub(crate) handles: VecDeque<Handle>,
    /// The control data was cut off: more handles came than fit.
    pub(crate) truncated: bool,
}
impl Incoming {
    pub(crate) fn new(channel: Box<dyn Receive>) -> Self {
        Self {
            channel,
            handles: VecDeque::new(),
            truncated: false,
        }
    }
    pub(crate) fn shutdown(&self) {
        self.channel.shutdown();
    }
}
impl Read for Incoming {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.channel.receive(buf, &mut self.handles);
        if read
            .as_ref()
            .is_err_and(|e| e.get_ref().is_some_and(|e| e.is::<Truncated>()))
        {
            self.truncated = true;
        }
        read
    }
}

/// One of a request's buffers, mapped for its handler's call: read-only when
/// the program reads it, writable when it writes it.
pub(crate) struct Mapping(View);
impl Mapping {
    /// Maps `buffer`'s handle for its access. Refuses a handle smaller than
    /// `buffer.bytes`, and on Linux an input that isn't sealed against
    /// writing, which the app could still change under the handler.
    pub(crate) fn map(handle: &Handle, buffer: &Buffer) -> io::Result<Self> {
        let len = usize::try_from(buffer.bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a buffer too large"))?;
        let write = buffer.access == Access::Write;
        let view = View::new(handle, len, write)?;
        MAPPED.fetch_add(1, Ordering::SeqCst);
        Ok(Self(view))
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        self.0.bytes()
    }
    /// The bytes to write; `None` for a read-only mapping.
    pub(crate) fn bytes_mut(&mut self) -> Option<&mut [u8]> {
        self.0.bytes_mut()
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        MAPPED.fetch_sub(1, Ordering::SeqCst);
    }
}
