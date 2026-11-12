//! Which ONNX Runtime the models run on: the one the app bundles, found as the
//! app finds it, so AI masks and AI Denoise share one copy. In order:
//!
//! 1. a library in `<local data>/runtime`, the app's own place for one it
//!    didn't come with;
//! 2. the file `RAWMAKASE_ORT_LIB` names, inherited from the user's
//!    environment untouched;
//! 3. the folders the app searches beside its executable, which it passes in
//!    [`HOST_RUNTIME_DIRS`] when it starts this program;
//! 4. the folders beside this program.
use rawmakase_inference::LoadOptions;
use rawmakase_inference::runtime::HOST_RUNTIME_DIRS;
use std::ffi::OsString;
use std::path::Path;

/// The options every model loads with: the runtime above, and this
/// computer's CPU. The accelerators are the remote worker's concern.
pub(crate) fn load_options() -> LoadOptions {
    options_from(
        &rawmakase_model_install::runtime_dir(),
        std::env::var_os(HOST_RUNTIME_DIRS),
    )
}

/// [`load_options`] with `local` for `<local data>/runtime` and `host` for
/// the value of [`HOST_RUNTIME_DIRS`]. The runtime loader adds
/// `RAWMAKASE_ORT_LIB` after the first and this program's own folders after
/// the second ([`rawmakase_inference::runtime::runtime_candidates`]).
fn options_from(local: &Path, host: Option<OsString>) -> LoadOptions {
    LoadOptions {
        runtime_library: rawmakase_inference::runtime::runtime_file_names()
            .iter()
            .map(|name| local.join(name))
            .find(|path| path.is_file()),
        runtime_dirs: host
            .map(|dirs| std::env::split_paths(&dirs).collect())
            .unwrap_or_default(),
        threads: None,
        provider: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawmakase_inference::runtime::{runtime_candidates, runtime_file_names};
    use std::path::PathBuf;

    /// The local runtime, then the user's variable, then the app's folders,
    /// then this program's own.
    #[test]
    fn the_local_runtime_comes_first_then_the_users_then_the_apps_then_ours() {
        let root = tempfile::tempdir().unwrap();
        let name = runtime_file_names()[0];
        let folder = |path: &str| {
            let dir = root.path().join(path);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(name), b"").unwrap();
            dir
        };
        let (local, app, app_lib, ours) = (
            folder("local/runtime"),
            folder("app"),
            folder("app/lib"),
            folder("ours"),
        );
        let host = std::env::join_paths([&app, &app_lib]).unwrap();
        let options = options_from(&local, Some(host));
        let user = PathBuf::from("/the/user's/libonnxruntime");
        let order = runtime_candidates(
            options.runtime_library.as_deref(),
            Some(user.clone().into()),
            &options.runtime_dirs,
            std::slice::from_ref(&ours),
        );
        assert_eq!(
            order,
            [
                local.join(name),
                user,
                app.join(name),
                app_lib.join(name),
                ours.join(name)
            ]
        );
        // Without a local runtime or the app's folders, the user's comes
        // first, then ours.
        let empty = root.path().join("nothing");
        let options = options_from(&empty, None);
        assert_eq!(options.runtime_library, None);
        assert!(options.runtime_dirs.is_empty());
        assert_eq!(options.provider, None, "on the CPU");
    }
}
