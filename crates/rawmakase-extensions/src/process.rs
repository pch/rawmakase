//! Starting a program as a unit the host can end whole, with the message
//! channel already connected to it.
//!
//! - Linux and macOS: a socket pair; the child's end becomes descriptor 3
//!   (`RAWMAKASE_EXTENSION_FD=3`) in a `pre_exec` hook, and the child leads a
//!   process group of its own, which [`Unit::kill`] ends with `killpg`.
//! - Windows: the host calls `CreateProcessW` itself, suspended, puts the
//!   process in a job object that kills its processes when closed, and only
//!   then lets it run, so none of its code runs outside the job; a failed
//!   assignment ends the child and fails the start. The channel is a named
//!   pipe, the first and only instance of its name, refusing remote clients,
//!   and accepted only from the child's process id.
//!
//! Stdin is the null device; stdout and stderr go to the extension's log file,
//! rotated once it passes [`LOG_ROTATE`].
//!
//! A waiter thread per unit waits for the primary process to exit and then
//! ends the whole unit at once, so a helper it started dies with it. On Unix it
//! waits without reaping first: until the process is reaped its id, the
//! group's, can't be reused, so the group killed is never another's.
use crate::session::{Exit, Launch};
use std::io::{self, Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A log file larger than this is renamed to `<name>.1.txt` before a start.
pub(crate) const LOG_ROTATE: u64 = 1 << 20;

/// The channel's two directions, and how to close the host's sending side.
pub(crate) struct Transport {
    pub(crate) reader: Box<dyn Read + Send>,
    pub(crate) writer: Box<dyn Write + Send>,
    /// Tells the program nothing more will come. On Windows a pipe can't be
    /// half-closed, so the program learns it from `quitting` alone.
    pub(crate) close_write: Box<dyn FnOnce() + Send>,
}

/// A started program.
pub(crate) struct Spawned {
    pub(crate) unit: Arc<Unit>,
    pub(crate) transport: Transport,
    pub(crate) pid: u32,
}

/// A program and everything it started, as one thing to end.
pub(crate) struct Unit {
    /// Whether the unit has ended, and how the primary process exited.
    state: Mutex<(bool, Option<Exit>)>,
    changed: Condvar,
    group: platform::Group,
}
impl Unit {
    fn new(group: platform::Group) -> Self {
        Self {
            state: Mutex::new((false, None)),
            changed: Condvar::new(),
            group,
        }
    }
    /// Ends every process of the unit unless it has ended already. Never
    /// blocks on the program.
    pub(crate) fn kill(&self) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.0 {
            self.group.kill();
        }
    }
    /// The primary process exited and the unit was ended.
    pub(crate) fn ended(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).0
    }
    /// Waits until the unit has ended, at most until `until`.
    pub(crate) fn wait_ended(&self, until: Instant) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while !state.0 {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }
    pub(crate) fn exit(&self) -> Option<Exit> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .clone()
    }
    /// The waiter's step once the primary process exited (and, on Unix,
    /// before it is reaped): end the rest of the unit and say so, under the
    /// lock [`Unit::kill`] takes, so no kill ever comes after the reap.
    fn end(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.group.kill();
        state.0 = true;
        self.changed.notify_all();
    }
    fn set_exit(&self, exit: Option<Exit>) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).1 = exit;
        self.changed.notify_all();
    }
}

/// Opens the log for appending, rotating it first when it is large.
fn open_log(path: &std::path::Path) -> io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_ROTATE) {
        let _ = std::fs::rename(path, path.with_extension("1.txt"));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

#[cfg(windows)]
pub(crate) use platform::kill_on_close_job;
pub(crate) use platform::spawn;

#[cfg(unix)]
mod platform {
    use super::*;
    use rawmakase_extension_protocol::{ENV_FD, ENV_PIPE};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};

    /// The process group, whose id is the primary process's.
    pub(super) struct Group(libc::pid_t);
    impl Group {
        pub(super) fn kill(&self) {
            // SAFETY: a plain system call; a group already gone is ESRCH.
            unsafe { libc::killpg(self.0, libc::SIGKILL) };
        }
    }

    pub(crate) fn spawn(launch: &Launch, _connect: Duration) -> io::Result<Spawned> {
        let (parent, child) = UnixStream::pair()?;
        let log = open_log(&launch.log)?;
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .current_dir(&launch.dir)
            .envs(launch.env.iter().map(|(k, v)| (k, v)))
            .env(ENV_FD, "3")
            .env_remove(ENV_PIPE)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0);
        let fd = child.as_raw_fd();
        // SAFETY: the hook runs between fork and exec and makes only
        // async-signal-safe calls. dup2 leaves the copy without
        // close-on-exec; when the end already is 3, the flag is cleared.
        unsafe {
            command.pre_exec(move || {
                if fd == 3 {
                    let flags = libc::fcntl(3, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                } else if libc::dup2(fd, 3) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let process = command.spawn()?;
        drop(child);
        let pid = process.id();
        let unit = Arc::new(Unit::new(Group(pid as libc::pid_t)));
        let waiter = unit.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("extension waiter".into())
            .spawn(move || wait(process, &waiter))
        {
            unit.kill();
            return Err(e);
        }
        let reader = parent.try_clone()?;
        let writer = parent.try_clone()?;
        Ok(Spawned {
            unit,
            transport: Transport {
                reader: Box::new(reader),
                writer: Box::new(writer),
                close_write: Box::new(move || {
                    let _ = parent.shutdown(std::net::Shutdown::Write);
                }),
            },
            pid,
        })
    }

    fn wait(mut process: Child, unit: &Unit) {
        let pid = process.id() as libc::id_t;
        loop {
            // SAFETY: an all-zero siginfo_t is a valid place for the answer.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: waits for this child to exit, leaving it unreaped
            // (WNOWAIT), so its id stays reserved while its group is killed.
            let waited =
                unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) };
            if waited == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
        unit.end();
        let exit = process.wait().ok().map(|status| {
            use std::os::unix::process::ExitStatusExt;
            Exit {
                code: status.code(),
                signal: status.signal(),
            }
        });
        unit.set_exit(exit);
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use rawmakase_extension_protocol::{ENV_FD, ENV_PIPE};
    use std::ffi::{OsStr, OsString, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{
        ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GENERIC_READ, GetLastError,
        HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE,
        FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS,
        OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateEventW,
        CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
        GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
        STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForMultipleObjects,
        WaitForSingleObject,
    };

    /// The job object, which kills its processes when its last handle closes.
    pub(super) struct Group(OwnedHandle);
    impl Group {
        pub(super) fn kill(&self) {
            // SAFETY: a valid job handle this unit owns.
            unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
        }
    }

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    fn check(ok: i32) -> io::Result<()> {
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Takes ownership of a handle a call returned, or its error.
    fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: a valid handle just returned to the caller, owned here
            // alone.
            Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
        }
    }

    /// An inheritable handle for the child's standard streams.
    fn inheritable(path: &OsStr, access: u32, disposition: u32) -> io::Result<OwnedHandle> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let name = wide(path);
        // SAFETY: a NUL-terminated name and a valid SECURITY_ATTRIBUTES.
        owned(unsafe {
            CreateFileW(
                name.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &attributes,
                disposition,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        })
    }

    /// Quotes one argument as CommandLineToArgvW and the C runtime read it.
    fn quote(arg: &OsStr, line: &mut Vec<u16>) {
        let chars: Vec<u16> = arg.encode_wide().collect();
        let special =
            |c: u16| c == u16::from(b' ') || c == u16::from(b'\t') || c == u16::from(b'"');
        if !chars.is_empty() && !chars.iter().copied().any(special) {
            line.extend(chars);
            return;
        }
        let backslash = u16::from(b'\\');
        line.push(u16::from(b'"'));
        let mut backslashes = 0;
        for c in chars {
            if c == backslash {
                backslashes += 1;
                continue;
            }
            let escape = if c == u16::from(b'"') {
                backslashes * 2 + 1
            } else {
                backslashes
            };
            line.extend(std::iter::repeat_n(backslash, escape));
            line.push(c);
            backslashes = 0;
        }
        line.extend(std::iter::repeat_n(backslash, backslashes * 2));
        line.push(u16::from(b'"'));
    }

    /// The command line: the program, quoted, then its arguments.
    pub(super) fn command_line(program: &OsStr, args: &[OsString]) -> Vec<u16> {
        let mut line = Vec::new();
        quote(program, &mut line);
        for arg in args {
            line.push(u16::from(b' '));
            quote(arg, &mut line);
        }
        line.push(0);
        line
    }

    /// The app's environment with `extra` set, as CreateProcessW's block:
    /// `name=value` strings, sorted by name without case, each ended by NUL,
    /// and a NUL after the last.
    fn environment(extra: &[(OsString, OsString)]) -> Vec<u16> {
        let mut vars: std::collections::BTreeMap<String, (OsString, OsString)> =
            std::collections::BTreeMap::new();
        for (name, value) in std::env::vars_os().chain(extra.iter().cloned()) {
            let key = name.to_string_lossy().to_uppercase();
            if key == ENV_FD {
                continue;
            }
            vars.insert(key, (name, value));
        }
        let mut block = Vec::new();
        for (name, value) in vars.values() {
            block.extend(name.encode_wide());
            block.push(u16::from(b'='));
            block.extend(value.encode_wide());
            block.push(0);
        }
        block.push(0);
        block
    }

    /// The host's end of the channel's pipe, shared by the reader and the
    /// writer; the first of them to use it accepts the child's connection.
    struct Server {
        pipe: OwnedHandle,
        process: Arc<OwnedHandle>,
        child: u32,
        deadline: Instant,
        connected: OnceLock<Result<(), String>>,
    }
    impl Server {
        fn ready(&self) -> io::Result<()> {
            self.connected
                .get_or_init(|| self.accept().map_err(|e| e.to_string()))
                .clone()
                .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))
        }

        /// Waits for the child to open the pipe, at most until the deadline or
        /// its exit, and checks that it is the child that did.
        fn accept(&self) -> io::Result<()> {
            let pipe = self.pipe.as_raw_handle();
            let event = event()?;
            // SAFETY: an all-zero OVERLAPPED is its documented initial state.
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            overlapped.hEvent = event.as_raw_handle();
            // SAFETY: `overlapped` outlives the operation: every path below
            // waits for it to complete, or cancels it and then waits.
            if unsafe { ConnectNamedPipe(pipe, &mut overlapped) } == 0 {
                // SAFETY: reads this thread's last error.
                match unsafe { GetLastError() } {
                    ERROR_PIPE_CONNECTED => {}
                    ERROR_IO_PENDING => {
                        let left = self.deadline.saturating_duration_since(Instant::now());
                        let handles = [event.as_raw_handle(), self.process.as_raw_handle()];
                        // SAFETY: two valid handles.
                        let woke = unsafe {
                            WaitForMultipleObjects(
                                2,
                                handles.as_ptr(),
                                0,
                                left.as_millis().min(u128::from(INFINITE - 1)) as u32,
                            )
                        };
                        let mut moved = 0;
                        if woke != WAIT_OBJECT_0 {
                            // SAFETY: cancels this operation, then waits for
                            // it to end before `overlapped` goes.
                            unsafe {
                                CancelIoEx(pipe, &overlapped);
                                GetOverlappedResult(pipe, &overlapped, &mut moved, 1);
                            }
                            return Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "the program didn't open its channel",
                            ));
                        }
                        // SAFETY: the operation completed.
                        check(unsafe { GetOverlappedResult(pipe, &overlapped, &mut moved, 0) })?;
                    }
                    error => return Err(io::Error::from_raw_os_error(error as i32)),
                }
            }
            let mut client = 0u32;
            // SAFETY: a connected pipe and a valid out-parameter.
            check(unsafe { GetNamedPipeClientProcessId(pipe, &mut client) })?;
            if client != self.child {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("process {client}, not the extension's program, opened its channel"),
                ));
            }
            Ok(())
        }
    }

    fn event() -> io::Result<OwnedHandle> {
        // SAFETY: a manual-reset event with no name or security.
        owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })
    }

    /// One direction of the pipe, with its own event, so a read waiting for
    /// the program never holds up a write (synchronous I/O on one pipe
    /// handle is serialized).
    struct End {
        server: Arc<Server>,
        event: OwnedHandle,
    }
    impl End {
        fn run(&mut self, start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> i32) -> io::Result<usize> {
            self.server.ready()?;
            let pipe = self.server.pipe.as_raw_handle();
            // SAFETY: an all-zero OVERLAPPED is its documented initial state.
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            overlapped.hEvent = self.event.as_raw_handle();
            if start(pipe, &mut overlapped) == 0 {
                // SAFETY: reads this thread's last error.
                let error = unsafe { GetLastError() };
                if error != ERROR_IO_PENDING {
                    return Err(io::Error::from_raw_os_error(error as i32));
                }
            }
            let mut moved = 0u32;
            // SAFETY: `overlapped` lives on this frame until the operation has
            // completed, which this call waits for.
            check(unsafe { GetOverlappedResult(pipe, &overlapped, &mut moved, 1) })?;
            Ok(moved as usize)
        }
    }
    impl Read for End {
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
    impl Write for End {
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

    pub(crate) fn spawn(launch: &Launch, connect: Duration) -> io::Result<Spawned> {
        let name = format!(
            r"\\.\pipe\rawmakase-ext-{}-{}",
            std::process::id(),
            crate::random_suffix()
        );
        let pipe_name = wide(OsStr::new(&name));
        // SAFETY: a NUL-terminated name; the handle is checked.
        let pipe = owned(unsafe {
            CreateNamedPipeW(
                pipe_name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 << 10,
                64 << 10,
                0,
                std::ptr::null(),
            )
        })?;
        // The channel ends' events, made before the program starts: failing
        // to make one then leaves nothing running.
        let (read_event, write_event) = (event()?, event()?);

        let job = kill_on_close_job()?;

        drop(open_log(&launch.log)?);
        let log = inheritable(launch.log.as_os_str(), FILE_APPEND_DATA, OPEN_ALWAYS)?;
        let null = inheritable(OsStr::new("NUL"), GENERIC_READ, OPEN_EXISTING)?;

        // Only the standard streams are inherited, whatever else in the app
        // is inheritable at this moment.
        let inherited = [null.as_raw_handle(), log.as_raw_handle()];
        let mut size = 0usize;
        // SAFETY: asks for the size of a one-attribute list; fails by design.
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
        let mut list = vec![0usize; size.div_ceil(size_of::<usize>())];
        let list_ptr = list.as_mut_ptr().cast::<c_void>();
        // SAFETY: `list` has the size asked for, suitably aligned.
        check(unsafe { InitializeProcThreadAttributeList(list_ptr, 1, 0, &mut size) })?;
        // SAFETY: `inherited` lives until CreateProcessW below returns.
        let updated = check(unsafe {
            UpdateProcThreadAttribute(
                list_ptr,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                inherited.as_ptr().cast(),
                size_of_val(&inherited),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        });

        let mut extra = launch.env.clone();
        extra.push((ENV_PIPE.into(), name.into()));
        let mut environment = environment(&extra);
        let mut command_line = command_line(launch.program.as_os_str(), &launch.args);
        let program = wide(launch.program.as_os_str());
        let dir = wide(launch.dir.as_os_str());
        // SAFETY: an all-zero STARTUPINFOEXW, then its size, flags and
        // handles set.
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = null.as_raw_handle();
        startup.StartupInfo.hStdOutput = log.as_raw_handle();
        startup.StartupInfo.hStdError = log.as_raw_handle();
        startup.lpAttributeList = list_ptr;
        // SAFETY: zeroed out-parameters.
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let created = updated.and_then(|()| {
            // SAFETY: every pointer is to a live, NUL-terminated buffer of
            // this frame; the command line is writable, as required.
            check(unsafe {
                CreateProcessW(
                    program.as_ptr(),
                    command_line.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    1,
                    CREATE_SUSPENDED
                        | CREATE_UNICODE_ENVIRONMENT
                        | CREATE_NO_WINDOW
                        | EXTENDED_STARTUPINFO_PRESENT,
                    environment.as_mut_ptr().cast(),
                    dir.as_ptr(),
                    &startup.StartupInfo,
                    &mut info,
                )
            })
        });
        // SAFETY: the list was initialized above.
        unsafe { DeleteProcThreadAttributeList(list_ptr) };
        drop((null, log));
        created?;
        let process = Arc::new(owned(info.hProcess)?);
        let thread = owned(info.hThread)?;

        // Into the job before any of its code runs: a failure ends it.
        // SAFETY: valid job and process handles.
        if let Err(e) =
            check(unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) })
        {
            // SAFETY: a valid process handle.
            unsafe { TerminateProcess(process.as_raw_handle(), 1) };
            return Err(e);
        }
        let unit = Arc::new(Unit::new(Group(job)));
        // SAFETY: a valid thread handle, created suspended.
        if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
            let e = io::Error::last_os_error();
            unit.kill();
            return Err(e);
        }
        drop(thread);
        let pid = info.dwProcessId;
        let waiter = unit.clone();
        let waited = process.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("extension waiter".into())
            .spawn(move || wait(&waited, &waiter))
        {
            unit.kill();
            return Err(e);
        }
        let server = Arc::new(Server {
            pipe,
            process,
            child: pid,
            deadline: Instant::now() + connect,
            connected: OnceLock::new(),
        });
        Ok(Spawned {
            unit,
            transport: Transport {
                reader: Box::new(End {
                    server: server.clone(),
                    event: read_event,
                }),
                writer: Box::new(End {
                    server,
                    event: write_event,
                }),
                close_write: Box::new(|| {}),
            },
            pid,
        })
    }

    /// A job object that kills its processes when its last handle closes.
    pub(crate) fn kill_on_close_job() -> io::Result<OwnedHandle> {
        // SAFETY: an unnamed job with default security; checked.
        let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        // SAFETY: an all-zero limit structure is valid; one flag is set.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the structure and its size match the information class.
        check(unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        })?;
        Ok(job)
    }

    fn wait(process: &OwnedHandle, unit: &Unit) {
        // SAFETY: a valid process handle.
        unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) };
        unit.end();
        let mut code = 0u32;
        // SAFETY: a valid process handle and out-parameter.
        let read = unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } != 0;
        unit.set_exit(read.then_some(Exit {
            code: Some(code as i32),
            signal: None,
        }));
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn arguments_are_quoted_as_the_c_runtime_reads_them() {
            let line = |args: &[&str]| {
                let args: Vec<OsString> = args.iter().map(OsString::from).collect();
                let mut line = command_line(OsStr::new(r"C:\Program Files\x.exe"), &args);
                line.pop();
                String::from_utf16(&line).unwrap()
            };
            assert_eq!(
                line(&[
                    "plain",
                    "two words",
                    "",
                    r#"say "hi""#,
                    r"trailing\",
                    r"a\\b"
                ]),
                r#""C:\Program Files\x.exe" plain "two words" "" "say \"hi\"" trailing\ a\\b"#
            );
            assert_eq!(
                line(&[r"dir\ x\"]),
                r#""C:\Program Files\x.exe" "dir\ x\\""#
            );
        }
    }
}
