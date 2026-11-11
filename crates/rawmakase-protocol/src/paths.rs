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
/// This computer's own data folder, never synced: the platform default even
/// when RAWMAKASE_DATA_DIR points elsewhere (a shared or synced folder), and
/// Windows's local rather than roaming AppData. Downloaded models, the ONNX
/// Runtime, `remote-inference.json` and the crash log live here, so anything
/// outside the app that needs them (an extension) must use this, not
/// [`data_dir`].
pub fn local_data_dir() -> PathBuf {
    local_data_dir_in(
        std::env::var_os("LOCALAPPDATA"),
        std::env::var_os("XDG_DATA_HOME"),
        &home(),
        cfg!(windows),
        cfg!(target_os = "macos"),
    )
}
/// The layout behind [`local_data_dir`], with the environment and the
/// platform read out, so every platform's folder is testable on any host.
fn local_data_dir_in(
    local_appdata: Option<OsString>,
    xdg_data_home: Option<OsString>,
    home: &Path,
    windows: bool,
    macos: bool,
) -> PathBuf {
    if macos {
        home.join("Library/Application Support/RAWmakase")
    } else if windows {
        local_appdata
            .map(|p| PathBuf::from(p).join("RAWmakase"))
            .unwrap_or_else(|| home.join(".local/share/rawmakase"))
    } else {
        xdg_data_home
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("rawmakase")
    }
}
/// The user's home folder (`$HOME`), or an empty path when it is unset.
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
    fn the_local_folder_ignores_the_override_and_roaming_appdata() {
        // RAWMAKASE_DATA_DIR is not a parameter: the local folder never moves
        // with it, so it can't be shared or synced between computers.
        let dir = local_data_dir_in(
            Some(r"C:\Users\me\AppData\Local".into()),
            Some("/xdg".into()),
            Path::new(r"C:\Users\me"),
            true,
            false,
        );
        assert_eq!(
            dir,
            PathBuf::from(r"C:\Users\me\AppData\Local").join("RAWmakase")
        );
        let roaming = data_dir_in(
            None,
            Some(r"C:\Users\me\AppData\Roaming".into()),
            Some("/xdg".into()),
            Path::new(r"C:\Users\me"),
            true,
            false,
        );
        assert_ne!(dir, roaming);
        let dir = local_data_dir_in(
            None,
            Some("/xdg".into()),
            Path::new(r"C:\Users\me"),
            true,
            false,
        );
        assert_eq!(dir, PathBuf::from(r"C:\Users\me/.local/share/rawmakase"));
    }

    #[test]
    fn the_local_folder_is_the_default_data_folder_on_macos_and_linux() {
        let mac = local_data_dir_in(
            Some(r"C:\Users\me\AppData\Local".into()),
            Some("/xdg".into()),
            Path::new("/Users/me"),
            false,
            true,
        );
        assert_eq!(
            mac,
            PathBuf::from("/Users/me/Library/Application Support/RAWmakase")
        );
        let linux = local_data_dir_in(
            None,
            Some("/data".into()),
            Path::new("/home/me"),
            false,
            false,
        );
        assert_eq!(linux, PathBuf::from("/data/rawmakase"));
        let linux = local_data_dir_in(None, None, Path::new("/home/me"), false, false);
        assert_eq!(linux, PathBuf::from("/home/me/.local/share/rawmakase"));
        for (windows, macos, home) in [(false, false, "/home/me"), (false, true, "/Users/me")] {
            assert_eq!(
                local_data_dir_in(None, Some("/data".into()), Path::new(home), windows, macos),
                data_dir_in(
                    None,
                    None,
                    Some("/data".into()),
                    Path::new(home),
                    windows,
                    macos
                ),
            );
        }
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
