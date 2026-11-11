//! The buffers' budgets and platform guarantees. Whether handles end up
//! closed, by the open handle count, is tested in `tests/session.rs`, where no
//! other test opens files meanwhile.
use super::*;

#[test]
fn budgets_refuse_before_anything_is_allocated() {
    let ledger = Ledger::new();
    let mut request = Buffers::new(&ledger);
    let over = request.output(MAX_REQUEST_BYTES + 1).unwrap_err();
    assert_eq!(over.code, ErrorCode::TooLarge);
    assert_eq!((request.count(), ledger.in_use()), (0, 0));
    assert_eq!(
        request.output(0).unwrap_err().code,
        ErrorCode::InvalidRequest
    );

    // Two requests at the request budget fill the extension's.
    let mut first = Buffers::new(&ledger);
    first.output(MAX_REQUEST_BYTES).unwrap();
    let mut second = Buffers::new(&ledger);
    second.output(MAX_REQUEST_BYTES).unwrap();
    assert_eq!(ledger.in_use(), MAX_EXTENSION_BYTES);
    let busy = request.output(1).unwrap_err();
    assert_eq!(busy.code, ErrorCode::Busy);
    assert_eq!(request.count(), 0);
    // A lease ends when its buffers go.
    drop(first);
    assert_eq!(ledger.in_use(), MAX_REQUEST_BYTES);
    assert_eq!(request.output(1).unwrap(), 0);
    drop((second, request));
    assert_eq!(ledger.in_use(), 0);

    let mut many = Buffers::new(&ledger);
    for _ in 0..rawmakase_extension_protocol::MAX_HANDLES {
        many.output(1).unwrap();
    }
    assert_eq!(many.output(1).unwrap_err().code, ErrorCode::TooLarge);
}

/// A fill that panics leaves nothing counted: the reservation is given back
/// as the panic unwinds, and the budget is whole for the next request.
#[test]
fn a_panicking_fill_gives_its_reservation_back() {
    let ledger = Ledger::new();
    let mut buffers = Buffers::new(&ledger);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        buffers.input(MAX_REQUEST_BYTES, |_| panic!("the render failed"))
    }));
    assert!(panicked.is_err());
    assert_eq!(buffers.count(), 0);
    drop(buffers);
    assert_eq!(ledger.in_use(), 0);
    let mut first = Buffers::new(&ledger);
    first.output(MAX_REQUEST_BYTES).unwrap();
    let mut second = Buffers::new(&ledger);
    second.output(MAX_REQUEST_BYTES).unwrap();
    assert_eq!(ledger.in_use(), MAX_EXTENSION_BYTES);
}

#[test]
fn an_output_reads_back_and_an_input_has_no_view() {
    let ledger = Ledger::new();
    let mut buffers = Buffers::new(&ledger);
    let input = buffers.input(5, |b| b.copy_from_slice(b"abcde")).unwrap();
    let output = buffers.output(4).unwrap();
    assert_eq!((input, output), (0, 1));
    assert_eq!(buffers.read(input), None);
    assert_eq!(buffers.read(output), Some(&[0u8; 4][..]));
    assert_eq!(buffers.bytes(), 9);
    assert_eq!(ledger.in_use(), 9);
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsRawFd;

    fn errno<T>(result: T, failed: bool) -> Option<i32> {
        let _ = result;
        failed.then(|| io::Error::last_os_error().raw_os_error().unwrap())
    }

    /// An input can't be written, mapped writable, shrunk or grown, and its
    /// seals can't change: the app's pixels stay what it rendered.
    #[test]
    fn a_sealed_input_refuses_every_change() {
        let memory = Memory::input(4096, |b| b.fill(7)).unwrap();
        let fd = memory.fd().as_raw_fd();
        let mut file = std::fs::File::from(memory.fd().try_clone().unwrap());
        let write = file.write(b"x");
        assert_eq!(
            write.unwrap_err().raw_os_error(),
            Some(libc::EPERM),
            "write"
        );
        // SAFETY: probes a mapping; a success is unmapped at once.
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert_eq!(mapped, libc::MAP_FAILED, "a writable mapping");
        // SAFETY: plain system calls on a descriptor the test owns.
        unsafe {
            assert_eq!(errno((), libc::ftruncate(fd, 10) != 0), Some(libc::EPERM));
            assert_eq!(errno((), libc::ftruncate(fd, 8192) != 0), Some(libc::EPERM));
            assert_eq!(
                errno((), libc::fcntl(fd, libc::F_ADD_SEALS, 0) != 0),
                Some(libc::EPERM)
            );
        }
        // Still readable, with what the app wrote.
        // SAFETY: a read-only shared mapping, unmapped below.
        unsafe {
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            );
            assert_ne!(ptr, libc::MAP_FAILED);
            assert_eq!(*ptr.cast::<u8>(), 7);
            libc::munmap(ptr, 4096);
        }
    }

    /// A program can't shrink an output under the app's view, which would
    /// make reading it fault, nor grow it.
    #[test]
    fn an_output_refuses_truncation() {
        let memory = Memory::output(4096).unwrap();
        let fd = memory.fd().as_raw_fd();
        // SAFETY: plain system calls on a descriptor the test owns.
        unsafe {
            assert_eq!(errno((), libc::ftruncate(fd, 0) != 0), Some(libc::EPERM));
            assert_eq!(errno((), libc::ftruncate(fd, 8192) != 0), Some(libc::EPERM));
        }
        // Writable by the program.
        let mut file = std::fs::File::from(memory.fd().try_clone().unwrap());
        file.write_all(b"ok").unwrap();
        assert_eq!(&memory.contents().unwrap()[..2], b"ok");
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::{AsFd, BorrowedFd};
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// What a program might send: a line with descriptors beside it.
    fn send_with(socket: &UnixStream, bytes: &[u8], fds: &[BorrowedFd<'_>]) {
        let owned: Vec<_> = fds
            .iter()
            .map(|fd| fd.try_clone_to_owned().unwrap())
            .collect();
        let passing = Passing(owned);
        Writer(socket.try_clone().unwrap())
            .send(bytes, Some(passing))
            .unwrap();
    }

    #[test]
    fn the_reader_marks_descriptors_a_program_sends_and_reads_on() {
        let (program, host) = UnixStream::pair().unwrap();
        let memory: Vec<_> = (0..3).map(|_| Memory::output(16).unwrap()).collect();
        let fds: Vec<_> = memory.iter().map(|m| m.fd().as_fd()).collect();
        let fault = Arc::new(AtomicBool::new(false));
        let mut reader = Reader {
            stream: host,
            fault: fault.clone(),
        };
        send_with(&program, b"{\"t\":\"event\",\"e\":\"x\"}\n", &fds);
        let mut line = [0u8; 64];
        let read = reader.read(&mut line).unwrap();
        assert_eq!(&line[..read], b"{\"t\":\"event\",\"e\":\"x\"}\n");
        assert!(fault.load(Ordering::SeqCst));
        // Plain bytes go on reading.
        (&program).write_all(b"more\n").unwrap();
        let read = reader.read(&mut line).unwrap();
        assert_eq!(&line[..read], b"more\n");
    }

    #[test]
    fn a_line_sent_in_parts_carries_its_handles_once() {
        let (a, b) = UnixStream::pair().unwrap();
        let memory = Memory::output(16).unwrap();
        // Larger than the socket's buffer, so the first send is partial.
        let mut bytes = vec![b'x'; 4 << 20];
        bytes.push(b'\n');
        let sender = std::thread::spawn({
            let a = a.try_clone().unwrap();
            let fd = memory.fd().try_clone().unwrap();
            let bytes = bytes.clone();
            move || {
                Writer(a).send(&bytes, Some(Passing(vec![fd]))).unwrap();
            }
        });
        let fault = Arc::new(AtomicBool::new(false));
        let mut reader = Reader {
            stream: b,
            fault: fault.clone(),
        };
        let mut got = Vec::new();
        let mut chunk = vec![0u8; 1 << 16];
        let mut reads = 0;
        while got.len() < bytes.len() {
            let n = reader.read(&mut chunk).unwrap();
            got.extend_from_slice(&chunk[..n]);
            reads += 1;
        }
        sender.join().unwrap();
        assert_eq!(got, bytes);
        assert!(reads > 1);
        // The reader saw the descriptor (one message's worth) and closed it.
        assert!(fault.load(Ordering::SeqCst));
        drop(a);
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Memory::{FILE_MAP_WRITE, MapViewOfFile, UnmapViewOfFile};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// An input's duplicate maps for reading, never for writing.
    #[test]
    fn an_input_is_passed_read_only() {
        let memory = Memory::input(16, |b| b.fill(3)).unwrap();
        // SAFETY: the current process's pseudo-handle.
        let me = unsafe { GetCurrentProcess() };
        let value = memory.duplicate_into(me).unwrap();
        // SAFETY: the duplicate was just made in this process, for the test.
        let duplicate = unsafe { OwnedHandle::from_raw_handle(value as usize as _) };
        // SAFETY: probes a view; a success is unmapped at once.
        let view = unsafe { MapViewOfFile(duplicate.as_raw_handle(), FILE_MAP_WRITE, 0, 0, 16) };
        assert!(view.Value.is_null(), "a writable view of an input");
        if !view.Value.is_null() {
            // SAFETY: the view just mapped.
            unsafe { UnmapViewOfFile(view) };
        }
        let output = Memory::output(16).unwrap();
        let value = output.duplicate_into(me).unwrap();
        // SAFETY: as above.
        let duplicate = unsafe { OwnedHandle::from_raw_handle(value as usize as _) };
        // SAFETY: maps the output's duplicate writable, then unmaps it.
        unsafe {
            let view = MapViewOfFile(duplicate.as_raw_handle(), FILE_MAP_WRITE, 0, 0, 16);
            assert!(!view.Value.is_null());
            *view.Value.cast::<u8>() = 9;
            UnmapViewOfFile(view);
        }
        assert_eq!(output.contents().unwrap()[0], 9);
    }

    /// A line whose send fails closes its duplicates in the program.
    #[test]
    fn duplicates_of_a_line_never_sent_are_closed() {
        use windows_sys::Win32::Foundation::GetHandleInformation;
        let ledger = Ledger::new();
        let mut buffers = Buffers::new(&ledger);
        buffers.input(16, |_| {}).unwrap();
        buffers.output(16).unwrap();
        // SAFETY: the current process's pseudo-handle, duplicated so the
        // `Child` owns a real handle to itself.
        let me = unsafe {
            let mut handle = std::ptr::null_mut();
            windows_sys::Win32::Foundation::DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentProcess(),
                GetCurrentProcess(),
                &mut handle,
                0,
                0,
                windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            );
            OwnedHandle::from_raw_handle(handle)
        };
        let child = Child(Arc::new(me));
        let passing = buffers.pass(&child).unwrap();
        let values = passing.values().unwrap();
        let open = |value: u64| {
            let mut flags = 0;
            // SAFETY: asks about a handle value of this process; a closed one
            // fails.
            unsafe { GetHandleInformation(value as usize as _, &mut flags) != 0 }
        };
        assert!(values.iter().all(|&v| open(v)));
        struct Broken;
        impl std::io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(
            Writer(Box::new(Broken))
                .send(b"x\n", Some(passing))
                .is_err()
        );
        assert!(!values.iter().any(|&v| open(v)));
    }
}
