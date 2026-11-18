//! When Windows changes how its displays show colour, so the window reads the
//! display again only then rather than on a timer.
//!
//! Each read lists the display paths and asks the driver about the window's
//! display. Doing that every few seconds is the suspected cause of a
//! whole-screen flicker on an NVIDIA GeForce under Windows 10 (#1126).
//!
//! A thread of its own watches for:
//! - `WM_DISPLAYCHANGE`, sent to every top-level window when a display's mode
//!   changes, including when HDR is turned on or off. The thread's hidden
//!   window receives it.
//! - The registry keys Windows writes when a display's profile is assigned
//!   (per user and system-wide, as Microsoft's WCS documentation describes), and
//!   each display's `MonitorDataStore` key, where the display settings keep the
//!   HDR switch and the SDR content brightness.
//!
//! The thread runs for the life of the process. It blocks in
//! `MsgWaitForMultipleObjects` and holds nothing that needs cleaning up at exit.
//! When it can't watch all of these, or stops, [`announced`] is false and the
//! window goes back to reading the display every [`RECHECK`](super::RECHECK).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET,
    REG_NOTIFY_CHANGE_NAME, REG_NOTIFY_THREAD_AGNOSTIC, RegCloseKey, RegCreateKeyExW,
    RegNotifyChangeKeyValue, RegOpenKeyExW,
};
use windows_sys::Win32::System::Threading::CreateEventW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetWindowLongPtrW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
    RegisterClassW, SetWindowLongPtrW, WM_DISPLAYCHANGE, WNDCLASSW, WS_OVERLAPPED,
};

/// How many changes have been announced since the process started.
static CHANGES: AtomicU64 = AtomicU64::new(0);
/// The thread watches everything listed above.
static ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// How many display changes Windows has announced. A reading taken at another
/// count may be out of date.
pub(crate) fn count() -> u64 {
    CHANGES.load(Ordering::Acquire)
}

/// Whether every change that matters is announced, so the window need not read
/// its display on a timer.
pub(crate) fn announced() -> bool {
    ANNOUNCED.load(Ordering::Acquire)
}

/// Starts watching, once per process; `wake` is called after each change,
/// from the watching thread, so the window reads its display again.
pub(crate) fn watch(wake: impl Fn() + Send + Sync + 'static) {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let changed: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            CHANGES.fetch_add(1, Ordering::AcqRel);
            wake();
        });
        let spawned = std::thread::Builder::new()
            .name("display changes".into())
            .spawn(move || {
                let Some(watcher) = Watcher::new(&KEYS, changed.clone()) else {
                    return;
                };
                ANNOUNCED.store(watcher.complete, Ordering::Release);
                // A change made before the keys were watched isn't announced.
                changed();
                watcher.run();
                // Back to reading on a timer, from the next frame.
                ANNOUNCED.store(false, Ordering::Release);
                changed();
            });
        if let Err(error) = spawned {
            log::warn!("display changes aren't watched: {error}");
        }
    });
}

/// A key to watch with its subtree: the first of `paths` under `root` that opens.
/// The later paths are parents, for when the first doesn't exist yet.
struct KeyToWatch {
    root: HKEY,
    paths: &'static [&'static str],
    /// Create the first path when it doesn't exist, rather than watch a parent.
    create: bool,
}

const KEYS: [KeyToWatch; 3] = [
    // Per-user display profile associations. Its parent changes whenever a
    // program starts (app compatibility data), so the key is created instead,
    // empty, as Windows would to assign a profile.
    KeyToWatch {
        root: HKEY_CURRENT_USER,
        paths: &[r"Software\Microsoft\Windows NT\CurrentVersion\ICM"],
        create: true,
    },
    // System-wide ones, in the monitor device class.
    KeyToWatch {
        root: HKEY_LOCAL_MACHINE,
        paths: &[r"SYSTEM\CurrentControlSet\Control\Class\{4d36e96e-e325-11ce-bfc1-08002be10318}"],
        create: false,
    },
    // Each display's HDR switch, SDR content brightness and advanced colour.
    KeyToWatch {
        root: HKEY_LOCAL_MACHINE,
        paths: &[
            r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers\MonitorDataStore",
            r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers",
        ],
        create: false,
    },
];

/// A watched key and the event its next change signals.
struct Watched {
    key: HKEY,
    event: HANDLE,
}

impl Watched {
    fn open(watch: &KeyToWatch) -> Option<Self> {
        let key = watch.paths.iter().find_map(|path| {
            let path: Vec<u16> = path.encode_utf16().chain([0]).collect();
            let mut key = std::ptr::null_mut();
            // SAFETY: `path` is NUL-terminated and `key` receives the handle.
            (unsafe { RegOpenKeyExW(watch.root, path.as_ptr(), 0, KEY_NOTIFY, &mut key) } == 0)
                .then_some(key)
        });
        let key = key.or_else(|| {
            let path: Vec<u16> = watch.paths.first()?.encode_utf16().chain([0]).collect();
            let mut key = std::ptr::null_mut();
            // SAFETY: as above; the key is created with default security.
            (watch.create
                && unsafe {
                    RegCreateKeyExW(
                        watch.root,
                        path.as_ptr(),
                        0,
                        std::ptr::null(),
                        0,
                        KEY_NOTIFY,
                        std::ptr::null(),
                        &mut key,
                        std::ptr::null_mut(),
                    )
                } == 0)
                .then_some(key)
        })?;
        // SAFETY: an unnamed auto-reset event with default security.
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        let watched = Self { key, event };
        (!event.is_null() && watched.arm()).then_some(watched)
    }
    /// Asks for the event to be signalled at the key's next change, which a
    /// notification needs again after each one.
    fn arm(&self) -> bool {
        let filter =
            REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_THREAD_AGNOSTIC;
        // SAFETY: the key and the event are ours and open.
        unsafe { RegNotifyChangeKeyValue(self.key, 1, filter, self.event, 1) == 0 }
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        // SAFETY: both are ours, and nothing uses them after this.
        unsafe {
            RegCloseKey(self.key);
            if !self.event.is_null() {
                CloseHandle(self.event);
            }
        }
    }
}

/// The watching thread's keys and hidden window.
struct Watcher {
    keys: Vec<Watched>,
    /// The hidden window, or null.
    window: HWND,
    /// Every key and the window are watched.
    complete: bool,
    /// Boxed, so the window's pointer to it stays put while the watcher moves.
    changed: Box<Arc<dyn Fn() + Send + Sync>>,
}

impl Watcher {
    /// Watches `keys` and `WM_DISPLAYCHANGE` on the calling thread, which must be
    /// the one that [`run`](Self::run)s it; `None` when nothing can be watched.
    fn new(keys: &[KeyToWatch], changed: Arc<dyn Fn() + Send + Sync>) -> Option<Self> {
        let watched: Vec<Watched> = keys.iter().filter_map(Watched::open).collect();
        let changed = Box::new(changed);
        let window = hidden_window(&*changed);
        let complete = watched.len() == keys.len() && !window.is_null();
        if watched.is_empty() && window.is_null() {
            log::warn!("display changes aren't watched");
            return None;
        }
        if !complete {
            log::warn!(
                "display changes are watched in part ({} of {} keys, window {}); \
                 the window reads its display every few seconds",
                watched.len(),
                keys.len(),
                !window.is_null(),
            );
        }
        Some(Self {
            keys: watched,
            window,
            complete,
            changed,
        })
    }

    /// Waits for changes until waiting fails, which shouldn't happen.
    fn run(&self) {
        let events: Vec<HANDLE> = self.keys.iter().map(|k| k.event).collect();
        let n = events.len() as u32;
        loop {
            // SAFETY: `events` holds `n` open events.
            let woken =
                unsafe { MsgWaitForMultipleObjects(n, events.as_ptr(), 0, u32::MAX, QS_ALLINPUT) };
            let index = woken.wrapping_sub(WAIT_OBJECT_0) as usize;
            if let Some(key) = self.keys.get(index) {
                let armed = key.arm();
                self.changed();
                if !armed {
                    log::warn!("a display key can't be watched again");
                    return;
                }
            } else if index == events.len() {
                // Sent messages are delivered by PeekMessageW; posted ones dispatched.
                let mut message = MSG::default();
                // SAFETY: `message` receives each message of this thread.
                while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) }
                    != 0
                {
                    // SAFETY: a message PeekMessageW just filled in.
                    unsafe { DispatchMessageW(&message) };
                }
            } else {
                log::warn!("display changes stopped being watched");
                return;
            }
        }
    }

    fn changed(&self) {
        (self.changed)();
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if !self.window.is_null() {
            // SAFETY: the window is this thread's, so nothing dispatches to it
            // meanwhile; it no longer points at `changed`, then is gone.
            unsafe {
                SetWindowLongPtrW(self.window, GWLP_USERDATA, 0);
                DestroyWindow(self.window);
            }
        }
    }
}

/// A top-level window that is never shown, for the `WM_DISPLAYCHANGE` Windows
/// sends every top-level window (a message-only window gets no broadcasts).
fn hidden_window(changed: *const Arc<dyn Fn() + Send + Sync>) -> HWND {
    let class: Vec<u16> = "RAWmakase display changes"
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: the class name is NUL-terminated and outlives the registration's use
    // here; registering it again (a second watcher, in tests) fails harmlessly.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wndclass = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        RegisterClassW(&wndclass);
        let window = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if !window.is_null() {
            // The window calls `changed`, which the watcher keeps boxed in place.
            SetWindowLongPtrW(window, GWLP_USERDATA, changed as isize);
        }
        window
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_DISPLAYCHANGE {
        // SAFETY: the user data is 0 or the watcher's `changed`, alive while its
        // thread dispatches this window's messages.
        let changed = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) }
            as *const Arc<dyn Fn() + Send + Sync>;
        if let Some(changed) = unsafe { changed.as_ref() } {
            changed();
        }
    }
    // SAFETY: the message is passed on as received.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;
    use windows_sys::Win32::System::Registry::{
        KEY_WRITE, REG_DWORD, RegDeleteTreeW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// A watcher on its own thread, of the keys `paths` under HKCU, reporting
    /// each change it announces on the returned channel, and its window.
    fn watcher(paths: &'static [&'static str]) -> (mpsc::Receiver<()>, HWND, bool) {
        let (sent, changes) = mpsc::channel();
        let (window_sent, window) = mpsc::channel();
        std::thread::spawn(move || {
            let sent = std::sync::Mutex::new(sent);
            let changed: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                let _ = sent.lock().unwrap().send(());
            });
            let keys = [KeyToWatch {
                root: HKEY_CURRENT_USER,
                paths,
                create: false,
            }];
            let watcher = Watcher::new(&keys, changed).expect("watcher");
            window_sent
                .send((watcher.window as isize, watcher.complete))
                .unwrap();
            watcher.run();
        });
        let (window, complete) = window.recv().unwrap();
        (changes, window as HWND, complete)
    }

    #[test]
    fn a_change_under_a_watched_key_is_announced_each_time() {
        let path: &'static str = Box::leak(
            format!(
                r"Software\RAWmakase\tests\display-changes-{}",
                std::process::id()
            )
            .into_boxed_str(),
        );
        let mut key = std::ptr::null_mut();
        // SAFETY: the path is NUL-terminated and `key` receives the handle.
        let created = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(path).as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(created, 0);
        let paths: &'static [&'static str] = Box::leak(Box::new([path]));
        let (changes, _, complete) = watcher(paths);
        assert!(complete, "the key and the window are watched");
        let quiet = Duration::from_millis(300);
        assert!(changes.recv_timeout(quiet).is_err(), "nothing changed yet");
        for value in [1u32, 2] {
            // SAFETY: a DWORD value written to our own key.
            let set = unsafe {
                RegSetValueExW(
                    key,
                    wide("SDRWhiteLevel").as_ptr(),
                    0,
                    REG_DWORD,
                    (&raw const value).cast(),
                    4,
                )
            };
            assert_eq!(set, 0);
            changes
                .recv_timeout(Duration::from_secs(5))
                .expect("the change is announced");
            while changes.recv_timeout(quiet).is_ok() {}
        }
        // SAFETY: our own test key. The watcher's thread ends with the tests.
        unsafe {
            RegCloseKey(key);
            RegDeleteTreeW(HKEY_CURRENT_USER, wide(path).as_ptr());
        }
    }

    /// Otherwise the window would go back to reading its display on a timer.
    #[test]
    fn windows_display_settings_can_all_be_watched() {
        let watcher = Watcher::new(&KEYS, Arc::new(|| {})).expect("watcher");
        assert!(watcher.complete);
    }

    #[test]
    fn a_display_mode_change_is_announced() {
        // A key that always exists, so the watcher starts.
        let (changes, window, _) = watcher(&[r"Software"]);
        // SAFETY: the watcher's live window; SendMessageW waits for its thread.
        unsafe { SendMessageW(window, WM_DISPLAYCHANGE, 32, 0) };
        changes
            .recv_timeout(Duration::from_secs(5))
            .expect("the change is announced");
    }
}
