//! Where RAWmakase keeps its data, so a client finds the app's `control.json`
//! without the app's own code.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// RAWMAKASE_DATA_DIR, else the platform's application-data folder.
pub fn data_dir() -> PathBuf {
    data_dir_in(
        std::env::var_os("RAWMAKASE_DATA_DIR"),
        std::env::var_os("APPDATA"),
        std::env::var_os("XDG_DATA_HOME"),
        &home(),
        cfg!(windows),
        cfg!(target_os = "macos"),
    )
}
/// The layout behind [`data_dir`], with the environment and the platform
/// read out, so every platform's folder is testable on any host.
fn data_dir_in(
    data_dir: Option<OsString>,
    appdata: Option<OsString>,
    xdg_data_home: Option<OsString>,
    home: &Path,
    windows: bool,
    macos: bool,
) -> PathBuf {
    data_dir
        .map(PathBuf::from)
        .or_else(|| {
            // Windows has no HOME; without this the data would land in the
            // current directory.
            windows
                .then_some(appdata)
                .flatten()
                .map(|p| PathBuf::from(p).join("RAWmakase"))
        })
        .unwrap_or_else(|| {
            if macos {
                home.join("Library/Application Support/RAWmakase")
            } else if windows {
                // XDG_DATA_HOME is not consulted on Windows.
                home.join(".local/share/rawmakase")
            } else {
                xdg_data_home
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("rawmakase")
            }
        })
}
pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}
/// $XDG_DATA_HOME, or its default ~/.local/share.
pub fn xdg_data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_override_wins_on_every_platform() {
        for (windows, macos) in [(false, false), (true, false), (false, true)] {
            let dir = data_dir_in(
                Some("D:/where/i/said".into()),
                Some("C:/Users/me/AppData/Roaming".into()),
                Some("/xdg".into()),
                Path::new("/home/me"),
                windows,
                macos,
            );
            assert_eq!(dir, PathBuf::from("D:/where/i/said"));
        }
    }

    #[test]
    fn macos_uses_library_application_support_and_ignores_appdata() {
        let dir = data_dir_in(
            None,
            Some(r"C:\Users\me\AppData\Roaming".into()),
            Some("/xdg".into()),
            Path::new("/Users/me"),
            false,
            true,
        );
        assert_eq!(
            dir,
            PathBuf::from("/Users/me/Library/Application Support/RAWmakase")
        );
    }

    #[test]
    fn windows_uses_appdata_and_ignores_xdg_data_home() {
        let dir = data_dir_in(
            None,
            Some(r"C:\Users\me\AppData\Roaming".into()),
            Some("/xdg".into()),
            Path::new(r"C:\Users\me"),
            true,
            false,
        );
        assert_eq!(
            dir,
            PathBuf::from(r"C:\Users\me\AppData\Roaming").join("RAWmakase")
        );
        // Without APPDATA the fallback is under home, never XDG_DATA_HOME.
        let dir = data_dir_in(
            None,
            None,
            Some("/xdg".into()),
            Path::new(r"C:\Users\me"),
            true,
            false,
        );
        assert_eq!(dir, PathBuf::from(r"C:\Users\me/.local/share/rawmakase"));
    }

    #[test]
    fn linux_uses_xdg_data_home_or_its_default_under_home() {
        let dir = data_dir_in(
            None,
            None,
            Some("/data".into()),
            Path::new("/home/me"),
            false,
            false,
        );
        assert_eq!(dir, PathBuf::from("/data/rawmakase"));
        let dir = data_dir_in(None, None, None, Path::new("/home/me"), false, false);
        assert_eq!(dir, PathBuf::from("/home/me/.local/share/rawmakase"));
    }
}
