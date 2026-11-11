//! Linux and macOS: shared memory made with `memfd_create` (sealed) or
//! `shm_open` (unlinked at once), descriptors sent with `sendmsg` and
//! `SCM_RIGHTS`, and the reader's `recvmsg`, which closes any descriptor a
//! program sends.
use rawmakase_extension_protocol::MAX_HANDLES;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// How many descriptors the reader's control buffer has room for: more than
/// any kernel delivers with one message (Linux 253, `SCM_MAX_FD`; 64-bit XNU
/// 254, its control data being capped at 2048 bytes), so a cut-off control
/// buffer can't hide a descriptor it installed.
pub(crate) const RECEIVE_ROOM: usize = 512;

/// The program, as the handles are passed to it: on Linux and macOS the
/// socket carries them, so nothing else is needed.
pub(crate) struct Child;

/// One buffer: its descriptor, and an output's read-only view.
pub(crate) struct Memory {
    fd: OwnedFd,
    view: Option<View>,
}
impl Memory {
    /// An input: written by `fill` through a view that is gone before the
    /// program sees it, and on Linux then sealed against writing, shrinking,
    /// growing and further seals.
    pub(crate) fn input(len: usize, fill: impl FnOnce(&mut [u8])) -> io::Result<Self> {
        let fd = create(len)?;
        let mut view = View::map(&fd, len, true)?;
        fill(view.bytes_mut());
        // Linux refuses F_SEAL_WRITE while a writable view exists.
        drop(view);
        #[cfg(target_os = "linux")]
        seal(
            &fd,
            libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE | libc::F_SEAL_SEAL,
        )?;
        Ok(Self { fd, view: None })
    }

    /// An output, zeroed, its size sealed on Linux so a program can't shrink
    /// it under the app's view; mapped read-only for the app.
    pub(crate) fn output(len: usize) -> io::Result<Self> {
        let fd = create(len)?;
        #[cfg(target_os = "linux")]
        seal(&fd, libc::F_SEAL_SHRINK | libc::F_SEAL_GROW)?;
        let view = View::map(&fd, len, false)?;
        Ok(Self {
            fd,
            view: Some(view),
        })
    }

    pub(crate) fn contents(&self) -> Option<&[u8]> {
        self.view.as_ref().map(View::bytes)
    }

    #[cfg(test)]
    pub(crate) fn fd(&self) -> &OwnedFd {
        &self.fd
    }
}

/// A shared mapping, unmapped when dropped.
struct View {
    ptr: *mut u8,
    len: usize,
}
// SAFETY: the mapping is plain shared memory, owned by this value alone.
unsafe impl Send for View {}
// SAFETY: shared references give out only `&[u8]`.
unsafe impl Sync for View {}
impl View {
    fn map(fd: &OwnedFd, len: usize, write: bool) -> io::Result<Self> {
        let protection = if write {
            libc::PROT_READ | libc::PROT_WRITE
        } else {
            libc::PROT_READ
        };
        // SAFETY: a new shared mapping of a descriptor this process owns, of
        // the size it was just given; the result is checked.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                protection,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr.cast(),
            len,
        })
    }
    fn bytes(&self) -> &[u8] {
        // SAFETY: `len` bytes mapped readable for this value's life. On Linux
        // the size is sealed, so no access faults; on macOS it is fixed.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: mapped writable (inputs only), borrowed mutably through this
        // value alone.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}
impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly what `map` mapped; nothing borrows it now.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

/// New shared memory of `len` bytes, close-on-exec, in no namespace.
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

/// `shm_open` with a random name, `O_CREAT | O_EXCL`, unlinked at once: no
/// other process can open it by name, and it is close-on-exec, as POSIX
/// makes every `shm_open` descriptor.
#[cfg(not(target_os = "linux"))]
fn open() -> io::Result<OwnedFd> {
    loop {
        // At most 31 bytes on macOS (PSHMNAMLEN).
        let name = std::ffi::CString::new(format!("/rmx-{}", crate::random_suffix()))
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
        // SAFETY: a descriptor just opened, owned here alone; the name goes
        // before anyone else can use it.
        unsafe {
            libc::shm_unlink(name.as_ptr());
            return Ok(OwnedFd::from_raw_fd(fd));
        }
    }
}

#[cfg(target_os = "linux")]
fn seal(fd: &OwnedFd, seals: libc::c_int) -> io::Result<()> {
    // SAFETY: a plain fcntl on a descriptor this process owns.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, seals) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The descriptors one line carries: copies, which the writer closes once the
/// line's first byte has carried them.
pub(crate) struct Passing(pub(super) Vec<OwnedFd>);
impl Passing {
    pub(super) fn new(items: &[Memory], _child: &Child) -> io::Result<Self> {
        let fds = items
            .iter()
            .map(|m| m.fd.try_clone())
            .collect::<io::Result<_>>()?;
        Ok(Self(fds))
    }
    /// What the line lists: nothing on Linux and macOS.
    pub(crate) fn values(&self) -> Option<Vec<u64>> {
        None
    }
    pub(crate) fn count(&self) -> usize {
        self.0.len()
    }
}

/// The host's sending end.
pub(crate) struct Writer(pub(crate) UnixStream);
impl Writer {
    /// Writes one whole line; its handles go as `SCM_RIGHTS` on the `sendmsg`
    /// carrying its first byte. A partial write goes on with plain writes,
    /// and the handles are never sent twice. The copies close when this
    /// returns: after a successful first send the program has its own.
    pub(crate) fn send(&mut self, bytes: &[u8], passing: Option<Passing>) -> io::Result<()> {
        let fds = passing.map(|p| p.0).unwrap_or_default();
        let sent = if fds.is_empty() || bytes.is_empty() {
            0
        } else {
            send_first(&self.0, bytes, &fds)?
        };
        drop(fds);
        self.0.write_all(&bytes[sent..])?;
        self.0.flush()
    }
}

/// The `sendmsg` carrying at least one byte and the descriptors.
fn send_first(socket: &UnixStream, bytes: &[u8], fds: &[OwnedFd]) -> io::Result<usize> {
    debug_assert!(fds.len() <= MAX_HANDLES);
    let payload = std::mem::size_of_val(fds);
    // SAFETY: arithmetic on a length; it touches no memory.
    let space = unsafe { libc::CMSG_SPACE(payload as libc::c_uint) } as usize;
    // u64s, for the alignment a cmsghdr needs.
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
    loop {
        // SAFETY: `msg` points at live buffers of the lengths it states; the
        // descriptors stay open for the call.
        let sent = unsafe { libc::sendmsg(socket.as_raw_fd(), &msg, flags) };
        if sent >= 0 {
            return Ok(sent as usize);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// The host's receiving end. Programs never send handles; any descriptor that
/// arrives anyway is closed at once and marks a protocol fault, as does
/// control data cut off. The bytes read on as before.
pub(crate) struct Reader {
    pub(crate) stream: UnixStream,
    pub(crate) fault: Arc<AtomicBool>,
}
impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let (read, received, cut) = receive(&self.stream, buf, RECEIVE_ROOM)?;
        if received > 0 || cut {
            self.fault.store(true, Ordering::SeqCst);
        }
        Ok(read)
    }
}

/// One `recvmsg` with room for `room` descriptors, closing every one that
/// arrived. Returns the bytes read, how many descriptors arrived and whether
/// the control data was cut off.
fn receive(socket: &UnixStream, buf: &mut [u8], room: usize) -> io::Result<(usize, usize, bool)> {
    let payload = room * size_of::<libc::c_int>();
    // SAFETY: arithmetic on a length; it touches no memory.
    let space = unsafe { libc::CMSG_SPACE(payload as libc::c_uint) } as usize;
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
    let mut received = Vec::new();
    // SAFETY: walks the control messages the kernel wrote, within the length
    // it reported in `msg`; each SCM_RIGHTS payload is an array of
    // descriptors now installed in this process and owned by no one else.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg);
                let header = data as usize - cmsg as usize;
                let count =
                    ((*cmsg).cmsg_len as usize).saturating_sub(header) / size_of::<libc::c_int>();
                for i in 0..count {
                    let fd = data.cast::<libc::c_int>().add(i).read_unaligned();
                    received.push(OwnedFd::from_raw_fd(fd));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    #[cfg(not(target_os = "linux"))]
    for fd in &received {
        // SAFETY: plain fcntl calls on a descriptor this process owns, made
        // close-on-exec in case a process starts before it closes below.
        unsafe {
            let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
    let count = received.len();
    drop(received);
    Ok((read, count, msg.msg_flags & libc::MSG_CTRUNC != 0))
}
