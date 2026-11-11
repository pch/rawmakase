//! Linux and macOS: descriptors received with `recvmsg`, mapped with `mmap`,
//! and, for the mock host, made and sent as the app does.
//!
//! - Linux: `memfd_create`. An input is sealed against writing, shrinking and
//!   growing once filled, and a program refuses one that isn't; an output is
//!   sealed against shrinking and growing, so a mapping of it can't fault.
//! - macOS: `shm_open` with a random name, unlinked at once. Its size is fixed
//!   once set; it can't be sealed.
use super::{Receive, truncated};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;

/// How many descriptors a `recvmsg` has room for: more than any kernel
/// delivers with one message (Linux 253, `SCM_MAX_FD`; 64-bit XNU 254), so
/// cut-off control data can't hide a descriptor the kernel installed. A line
/// may still declare at most [`MAX_HANDLES`](rawmakase_extension_protocol::MAX_HANDLES),
/// which the reader checks as a protocol fault.
pub(crate) const RECEIVE_ROOM: usize = 512;

/// A handle the app passed: a descriptor this process owns.
pub(crate) type Handle = OwnedFd;

impl Receive for UnixStream {
    fn receive(&mut self, buf: &mut [u8], handles: &mut VecDeque<Handle>) -> io::Result<usize> {
        let (read, cut) = receive(self.as_fd(), buf, RECEIVE_ROOM, handles)?;
        if cut {
            return Err(truncated());
        }
        Ok(read)
    }
    fn shutdown(&self) {
        let _ = UnixStream::shutdown(self, std::net::Shutdown::Both);
    }
}

/// The control buffer's size for `fds` descriptors.
fn control_space(fds: usize) -> usize {
    let bytes = (fds * size_of::<libc::c_int>()) as libc::c_uint;
    // SAFETY: arithmetic on a length; it touches no memory.
    unsafe { libc::CMSG_SPACE(bytes) as usize }
}

/// One `recvmsg` with room for `room` descriptors. Every descriptor delivered
/// goes to `fds` as an owning handle before anything else is looked at, so
/// none leaks whatever follows; on Linux they arrive close-on-exec
/// (`MSG_CMSG_CLOEXEC`), on macOS they are made so straight after. Returns the
/// bytes read and whether the control data was cut off (`MSG_CTRUNC`).
pub(crate) fn receive(
    socket: BorrowedFd<'_>,
    buf: &mut [u8],
    room: usize,
    fds: &mut VecDeque<OwnedFd>,
) -> io::Result<(usize, bool)> {
    let space = control_space(room);
    // u64s, for the alignment a cmsghdr needs.
    let mut control = vec![0u64; space.div_ceil(8)];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    // SAFETY: an all-zero msghdr is a valid empty header; its pointers are set
    // below to buffers that outlive the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    #[cfg(target_os = "linux")]
    let flags = libc::MSG_CMSG_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let flags = 0;
    let read = loop {
        // SAFETY: `msg` points at `iov`, `buf` and `control`, all live and of
        // the lengths it states.
        let read = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msg, flags) };
        if read >= 0 {
            break read as usize;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    let first = fds.len();
    // The end of the control bytes the kernel wrote.
    let end = control.as_ptr() as usize + (msg.msg_controllen as usize).min(space);
    // SAFETY: walks the control messages the kernel wrote, within the length
    // it reported in `msg` (CMSG_FIRSTHDR and CMSG_NXTHDR stop there, and each
    // payload is read no further than `end`); each SCM_RIGHTS payload is an
    // array of descriptors now installed in this process and owned by no one
    // else.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg);
                let header = data as usize - cmsg as usize;
                // XNU keeps a cut-off message's full `cmsg_len`: read no
                // further than the control bytes actually returned.
                let returned = end.saturating_sub(cmsg as usize);
                let len = ((*cmsg).cmsg_len as usize).min(returned);
                let count = len.saturating_sub(header) / size_of::<libc::c_int>();
                for i in 0..count {
                    let fd = data.cast::<libc::c_int>().add(i).read_unaligned();
                    fds.push_back(OwnedFd::from_raw_fd(fd));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    #[cfg(not(target_os = "linux"))]
    for fd in fds.iter().skip(first) {
        // SAFETY: plain fcntl calls on a descriptor this process owns. A
        // process started on another thread between recvmsg and here may
        // inherit it (docs/extensions/protocol.md); std's Command doesn't.
        unsafe {
            let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
    #[cfg(target_os = "linux")]
    let _ = first;
    Ok((read, msg.msg_flags & libc::MSG_CTRUNC != 0))
}

/// Writes `bytes` with `fds` as `SCM_RIGHTS` on the `sendmsg` carrying the
/// first byte; when it takes only part, the rest follows as plain writes and
/// the descriptors are never sent again.
pub(crate) fn send(socket: &UnixStream, bytes: &[u8], fds: &[BorrowedFd<'_>]) -> io::Result<()> {
    if fds.is_empty() || bytes.is_empty() {
        let mut socket = socket;
        return socket.write_all(bytes).and_then(|()| socket.flush());
    }
    let payload = std::mem::size_of_val(fds);
    let space = control_space(fds.len());
    let mut control = vec![0u64; space.div_ceil(8)];
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr().cast_mut().cast(),
        iov_len: bytes.len(),
    };
    // SAFETY: an all-zero msghdr is a valid empty header; its pointers are set
    // below to buffers that outlive the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: the control buffer is `space` bytes, room for one header and
    // `fds.len()` descriptors, which are copied into its data.
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(payload as libc::c_uint) as _;
        let data = libc::CMSG_DATA(cmsg).cast::<libc::c_int>();
        for (i, fd) in fds.iter().enumerate() {
            data.add(i).write_unaligned(fd.as_raw_fd());
        }
    }
    #[cfg(target_os = "linux")]
    let flags = libc::MSG_NOSIGNAL;
    #[cfg(not(target_os = "linux"))]
    let flags = 0;
    let sent = loop {
        // SAFETY: `msg` points at live buffers of the lengths it states; the
        // descriptors stay open (borrowed) for the call.
        let sent = unsafe { libc::sendmsg(socket.as_raw_fd(), &msg, flags) };
        if sent >= 0 {
            break sent as usize;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    let mut socket = socket;
    socket.write_all(&bytes[sent..])?;
    socket.flush()
}

/// A mapping of a buffer, unmapped when dropped.
pub(crate) struct View {
    ptr: *mut u8,
    len: usize,
    write: bool,
}
// SAFETY: the mapping is plain shared memory, owned by this value alone.
unsafe impl Send for View {}

impl View {
    /// Maps `len` bytes of `handle`, writable when `write`. Checks first that
    /// the object holds them, so no access can fault, and on Linux that an
    /// input can't be written, shrunk or grown.
    pub(crate) fn new(handle: &Handle, len: usize, write: bool) -> io::Result<Self> {
        let fd = handle.as_raw_fd();
        // SAFETY: an all-zero stat is a valid out-parameter.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: a descriptor this process owns and a valid out-parameter.
        if unsafe { libc::fstat(fd, &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if (stat.st_size as u64) < len as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the buffer's memory holds {} bytes, not {len}",
                    stat.st_size
                ),
            ));
        }
        #[cfg(target_os = "linux")]
        {
            // SAFETY: a plain fcntl on a descriptor this process owns.
            let seals = unsafe { libc::fcntl(fd, libc::F_GET_SEALS) };
            let needed = if write {
                libc::F_SEAL_SHRINK
            } else {
                libc::F_SEAL_SHRINK | libc::F_SEAL_WRITE
            };
            if seals < 0 || seals & needed != needed {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the buffer's memory isn't sealed: the app could change or shrink it",
                ));
            }
        }
        map(fd, len, write)
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        // SAFETY: `len` bytes mapped readable for this value's life.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    pub(crate) fn bytes_mut(&mut self) -> Option<&mut [u8]> {
        // SAFETY: `len` bytes mapped writable for this value's life, borrowed
        // mutably through it alone.
        self.write
            .then(|| unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) })
    }
}
impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly what `map` mapped; nothing borrows it now.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

/// Maps `len` bytes of `fd`, shared.
fn map(fd: libc::c_int, len: usize, write: bool) -> io::Result<View> {
    let protection = if write {
        libc::PROT_READ | libc::PROT_WRITE
    } else {
        libc::PROT_READ
    };
    // SAFETY: a new shared mapping of a descriptor this process owns; the
    // result is checked.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            protection,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    #[cfg(target_os = "linux")]
    if write {
        dont_fork(ptr, len);
    }
    Ok(View {
        ptr: ptr.cast(),
        len,
        write,
    })
}

/// Shared memory the mock host allocates, as the app does.
pub(crate) struct Memory {
    fd: OwnedFd,
    /// An output's mapping, for reading what the program wrote.
    view: Option<View>,
}
impl Memory {
    /// An input holding `bytes`, sealed on Linux once written.
    pub(crate) fn input(bytes: &[u8]) -> io::Result<Self> {
        let memory = Self::unsealed(bytes)?;
        #[cfg(target_os = "linux")]
        seal(
            &memory.fd,
            libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE | libc::F_SEAL_SEAL,
        )?;
        Ok(memory)
    }

    /// An input holding `bytes`, not sealed: a program refuses it on Linux.
    pub(crate) fn unsealed(bytes: &[u8]) -> io::Result<Self> {
        let fd = create(bytes.len())?;
        let mut view = map(fd.as_raw_fd(), bytes.len().max(1), true)?;
        if let Some(target) = view.bytes_mut() {
            target[..bytes.len()].copy_from_slice(bytes);
        }
        // The writable view goes before sealing: Linux refuses F_SEAL_WRITE
        // while one exists.
        drop(view);
        Ok(Self { fd, view: None })
    }

    /// An output of `len` bytes, its size sealed on Linux, mapped to read.
    pub(crate) fn output(len: usize) -> io::Result<Self> {
        let fd = create(len)?;
        #[cfg(target_os = "linux")]
        seal(&fd, libc::F_SEAL_SHRINK | libc::F_SEAL_GROW)?;
        let view = map(fd.as_raw_fd(), len.max(1), false)?;
        Ok(Self {
            fd,
            view: Some(view),
        })
    }

    pub(crate) fn handle(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// What an output holds.
    pub(crate) fn contents(&self) -> &[u8] {
        self.view.as_ref().map_or(&[], View::bytes)
    }
}

/// New shared memory of `len` bytes, close-on-exec, not in any namespace.
fn create(len: usize) -> io::Result<OwnedFd> {
    let fd = open()?;
    // SAFETY: sizes a descriptor this process owns.
    if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

#[cfg(target_os = "linux")]
fn open() -> io::Result<OwnedFd> {
    // SAFETY: a NUL-terminated name; the result is checked.
    let fd = unsafe {
        libc::memfd_create(
            c"rawmakase-buffer".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a descriptor just created, owned here alone.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(not(target_os = "linux"))]
fn open() -> io::Result<OwnedFd> {
    use std::hash::{BuildHasher, Hasher};
    loop {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u32(std::process::id());
        // At most 31 bytes on macOS (PSHMNAMLEN).
        let name = std::ffi::CString::new(format!("/rmx-{:016x}", hasher.finish()))
            .expect("no NUL in the name");
        // SAFETY: a NUL-terminated name; the result is checked.
        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600 as libc::c_uint,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(error);
        }
        // SAFETY: a descriptor just opened, owned here alone; the name goes at
        // once, so nothing else can open the memory.
        unsafe {
            libc::shm_unlink(name.as_ptr());
            return Ok(OwnedFd::from_raw_fd(fd));
        }
    }
}

#[cfg(target_os = "linux")]
fn seal(fd: &OwnedFd, seals: libc::c_int) -> io::Result<()> {
    // `F_SEAL_WRITE` fails with EBUSY while any writable mapping exists. The
    // app's own view is gone and was never inherited (`MADV_DONTFORK`), but a
    // process forked on another thread between `mmap` and `madvise` holds a
    // copy until it execs, a moment later. So EBUSY is retried: 200 attempts,
    // 1 ms of sleep apart, about 200 ms or a little more in all. Allocation
    // runs off the UI thread (the app's selection worker), so the wait blocks
    // no frame.
    let mut tries = 0;
    loop {
        // SAFETY: a plain fcntl on a descriptor this process owns.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, seals) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        tries += 1;
        if error.raw_os_error() != Some(libc::EBUSY) || tries == 200 {
            return Err(error);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Keeps a writable view out of processes forked while it exists, so none of
/// them holds a writable mapping that would stop the input from being sealed.
#[cfg(target_os = "linux")]
fn dont_fork(ptr: *mut libc::c_void, len: usize) {
    // SAFETY: advice on a mapping this process just made; it changes nothing
    // in this process. A failure only loses the protection the seal's retry
    // covers.
    unsafe { libc::madvise(ptr, len, libc::MADV_DONTFORK) };
}

/// How many descriptors this process has open.
#[cfg(test)]
pub(crate) fn open_handles() -> usize {
    #[cfg(target_os = "linux")]
    let dir = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    let dir = "/dev/fd";
    std::fs::read_dir(dir).map_or(0, Iterator::count)
}
