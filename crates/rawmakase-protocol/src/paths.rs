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
    use Os::{Linux, Mac, Windows};

    const MAC: &str = "/Users/me/Library/Application Support/RAWmakase";
    const LINUX: &str = "/home/me/.local/share/rawmakase";
    const ROAMING: &str = r"C:\Users\me\AppData\Roaming";
    const LOCAL: &str = r"C:\Users\me\AppData\Local";
    const WIN_HOME: &str = r"C:\Users\me";
    /// Windows without its AppData variable: under home, never XDG_DATA_HOME.
    const WIN_FALLBACK: &str = r"C:\Users\me/.local/share/rawmakase";

    #[derive(Clone, Copy, PartialEq)]
    enum Os {
        Linux,
        Windows,
        Mac,
    }

    fn data(
        over: Option<&str>,
        app: Option<&str>,
        xdg: Option<&str>,
        home: &str,
        os: Os,
    ) -> PathBuf {
        let [over, app, xdg] = [over, app, xdg].map(|v| v.map(OsString::from));
        data_dir_in(over, app, xdg, Path::new(home), os == Windows, os == Mac)
    }

    fn local(app: Option<&str>, xdg: Option<&str>, home: &str, os: Os) -> PathBuf {
        let [app, xdg] = [app, xdg].map(|v| v.map(OsString::from));
        local_data_dir_in(app, xdg, Path::new(home), os == Windows, os == Mac)
    }

    /// The override wins everywhere; macOS ignores APPDATA, Windows XDG_DATA_HOME.
    #[test]
    fn the_data_folder_of_each_platform() {
        for os in [Linux, Windows, Mac] {
            let dir = data(Some("D:/said"), Some(ROAMING), Some("/xdg"), "/home/me", os);
            assert_eq!(dir, Path::new("D:/said"));
        }
        let mac = data(None, Some(ROAMING), Some("/xdg"), "/Users/me", Mac);
        assert_eq!(mac, Path::new(MAC));
        let windows = data(None, Some(ROAMING), Some("/xdg"), WIN_HOME, Windows);
        assert_eq!(windows, Path::new(ROAMING).join("RAWmakase"));
        let windows = data(None, None, Some("/xdg"), WIN_HOME, Windows);
        assert_eq!(windows, Path::new(WIN_FALLBACK));
        let linux = data(None, None, Some("/data"), "/home/me", Linux);
        assert_eq!(linux, Path::new("/data/rawmakase"));
        assert_eq!(data(None, None, None, "/home/me", Linux), Path::new(LINUX));
    }

    /// The local folder takes no RAWMAKASE_DATA_DIR, so it never moves with it and
    /// can't be shared or synced between computers. On Windows it is the local
    /// AppData rather than the roaming one; elsewhere it is the default data folder.
    #[test]
    fn the_local_folder_of_each_platform() {
        let windows = local(Some(LOCAL), Some("/xdg"), WIN_HOME, Windows);
        assert_eq!(windows, Path::new(LOCAL).join("RAWmakase"));
        let windows = local(None, Some("/xdg"), WIN_HOME, Windows);
        assert_eq!(windows, Path::new(WIN_FALLBACK));
        let mac = local(Some(LOCAL), Some("/xdg"), "/Users/me", Mac);
        assert_eq!(mac, Path::new(MAC));
        let linux = local(None, Some("/data"), "/home/me", Linux);
        assert_eq!(linux, Path::new("/data/rawmakase"));
        assert_eq!(local(None, None, "/home/me", Linux), Path::new(LINUX));
    }
}
