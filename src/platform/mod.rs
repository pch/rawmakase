//! Platform integration, independent of catalog and rendering policy.
pub(crate) mod network;
pub(crate) mod quit;
pub(crate) mod reveal;
pub(crate) mod text_scale;
pub(crate) mod volume;
pub(crate) mod web;

/// Whether winit opens windows through Wayland rather than X11 on Linux: it
/// does whenever the session offers Wayland, by either variable.
#[cfg(target_os = "linux")]
pub(crate) fn wayland() -> bool {
    wayland_in(|name| std::env::var_os(name))
}

#[cfg(target_os = "linux")]
fn wayland_in(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    let set = |name| var(name).is_some_and(|value| !value.is_empty());
    set("WAYLAND_DISPLAY") || set("WAYLAND_SOCKET")
}

#[cfg(all(test, target_os = "linux"))]
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
