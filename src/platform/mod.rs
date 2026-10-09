//! Platform integration, independent of catalog and rendering policy.
pub(crate) mod network;
pub(crate) mod quit;
pub(crate) mod reveal;
pub(crate) mod text_scale;
pub(crate) mod volume;
pub(crate) mod web;

/// The Windows taskbar uses the window class icon. eframe sets the title-bar
/// icon with `WM_SETICON` and leaves the class on the generic application icon.
#[cfg(windows)]
pub(crate) fn taskbar_icon(window: &impl winit::raw_window_handle::HasWindowHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::Foundation::{HINSTANCE, HWND};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GCLP_HICON, GCLP_HICONSM, GWLP_HINSTANCE, GetSystemMetrics, GetWindowLongPtrW, ICON_BIG,
        ICON_SMALL, IMAGE_ICON, LoadImageW, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON,
        SendMessageW, SetClassLongPtrW, WM_SETICON,
    };
    use winit::raw_window_handle::RawWindowHandle;
    static SET: AtomicBool = AtomicBool::new(false);
    if SET.load(Ordering::Relaxed) {
        return;
    }
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: eframe supplies the live window, and the icon is the one build.rs
    // embedded in this executable.
    let applied = unsafe {
        let hwnd = handle.hwnd.get() as HWND;
        let instance = GetWindowLongPtrW(hwnd, GWLP_HINSTANCE) as HINSTANCE;
        let load = |cx, cy| LoadImageW(instance, 1 as _, IMAGE_ICON, cx, cy, 0);
        let big = load(GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON));
        let small = load(GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        if big.is_null() || small.is_null() {
            false
        } else {
            SetClassLongPtrW(hwnd, GCLP_HICON, big as _);
            SetClassLongPtrW(hwnd, GCLP_HICONSM, small as _);
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as _, big as _);
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as _, small as _);
            true
        }
    };
    if applied {
        SET.store(true, Ordering::Relaxed);
    }
}

/// Whether winit opens windows through Wayland rather than X11 on Linux: it
/// does whenever the session offers Wayland, by either variable.
#[cfg(any(target_os = "linux", feature = "telemetry"))]
pub(crate) fn wayland() -> bool {
    wayland_in(|name| std::env::var_os(name))
}

#[cfg(any(target_os = "linux", feature = "telemetry"))]
fn wayland_in(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    let set = |name| var(name).is_some_and(|value| !value.is_empty());
    set("WAYLAND_DISPLAY") || set("WAYLAND_SOCKET")
}

#[cfg(all(test, any(target_os = "linux", feature = "telemetry")))]
mod tests {
    use super::wayland_in;
    use std::ffi::OsString;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let vars: Vec<(String, OsString)> = vars
            .iter()
            .map(|(name, value)| (name.to_string(), OsString::from(value)))
            .collect();
        move |name| {
            vars.iter()
                .find(|(set, _)| set == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn either_variable_means_wayland() {
        assert!(wayland_in(env(&[("WAYLAND_DISPLAY", "wayland-1")])));
        assert!(wayland_in(env(&[("WAYLAND_SOCKET", "3")])));
    }

    #[test]
    fn without_them_it_is_x11() {
        assert!(!wayland_in(env(&[("DISPLAY", ":0")])));
        assert!(!wayland_in(env(&[
            ("WAYLAND_DISPLAY", ""),
            ("DISPLAY", ":0")
        ])));
    }
}
