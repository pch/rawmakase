//! Lowering a background thread's scheduling priority, so photo work behind
//! the user's back takes only cores the foreground leaves idle. Linux: ten
//! steps nicer (at most 19), which needs no privilege and applies to the one
//! thread. Windows: below the process's class. macOS keeps the priority.

/// Lowers the calling thread's priority from now on.
pub(super) fn lower() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: gettid takes nothing and returns the caller's thread id;
        // getpriority and setpriority read and change only that thread's nice
        // value. errno is reset first so getpriority's -1 result is told from
        // an error.
        unsafe {
            let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
            *libc::__errno_location() = 0;
            let nice = libc::getpriority(libc::PRIO_PROCESS, tid);
            if *libc::__errno_location() == 0 && nice < 19 {
                libc::setpriority(libc::PRIO_PROCESS, tid, (nice + 10).min(19));
            }
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        // SAFETY: GetCurrentThread returns a pseudo-handle for the caller,
        // valid for this call, and SetThreadPriority changes only its priority.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
    }
}
