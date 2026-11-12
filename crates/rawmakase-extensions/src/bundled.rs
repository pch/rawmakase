//! The extensions that ship with the app. Their manifests are compiled into
//! it (`include_str!` of `extensions/*/rawmakase-extension.toml`, added as each
//! moves out of the core), and their programs are installed beside its
//! executable, where [`program_search_dirs`] looks, as ONNX Runtime is found
//! (`rawmakase_inference::runtime::runtime_search_dirs`).
//!
//! A bundled extension without an entry in `extensions.json` is enabled,
//! unless it is one of [`OFF_BY_DEFAULT`]; it updates with the app and can't
//! be uninstalled.
use crate::{Error, Manifest, Result};
use std::path::{Path, PathBuf};

/// Bundled extensions that stay off until the user enables them: MIDI, which
/// only people with a control surface need. The app enables it once for
/// someone whose earlier version had MIDI devices set up.
pub const OFF_BY_DEFAULT: &[&str] = &["rawmakase.midi"];

/// Whether extension `id` is enabled while `extensions.json` has no entry for
/// it.
pub fn enabled_by_default(id: &str) -> bool {
    !OFF_BY_DEFAULT.contains(&id)
}

/// A bundled extension, found.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundled {
    /// Its compiled-in manifest.
    pub manifest: Manifest,
    /// Its program beside the app, when its manifest names one and it was
    /// found.
    pub program: Option<PathBuf>,
}
impl Bundled {
    /// Why it can't run, when its manifest names a program that isn't there
    /// (an incomplete install).
    pub fn problem(&self) -> Option<String> {
        let program = self.manifest.program.as_ref()?;
        self.program.is_none().then(|| {
            format!(
                "{} is not installed beside RAWmakase",
                program_name(&program.path)
            )
        })
    }
}

/// Reads the compiled-in manifests and finds their programs in `dirs`, in
/// order. A manifest that doesn't read is a build error of the app, reported
/// with the others' results so the rest still load.
pub fn discover(manifests: &[&str], dirs: &[PathBuf]) -> Vec<Result<Bundled>> {
    manifests
        .iter()
        .map(|text| {
            let manifest = Manifest::parse(text)
                .map_err(|e| Error::new(format!("A bundled manifest is invalid: {e}")))?;
            let program = manifest.program.as_ref().and_then(|program| {
                let name = program_name(&program.path);
                dirs.iter().map(|d| d.join(&name)).find(|p| p.is_file())
            });
            Ok(Bundled { manifest, program })
        })
        .collect()
}

/// A bundled program's file name: the last part of its manifest path, with
/// `.exe` on Windows when it has no extension. Bundled programs are installed
/// flat, beside the app.
pub fn program_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    if cfg!(windows) && !name.contains('.') {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Where bundled programs are looked for, beside the running executable.
pub fn program_search_dirs() -> Vec<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .map(|dir| search_dirs_from(&dir, std::env::consts::OS))
        .unwrap_or_default()
}

/// [`program_search_dirs`] for an executable in `exe_dir` on `os`: the
/// executable's folder (Windows, the Linux archive and AppImage, and
/// `Contents/MacOS` in the macOS app), then `../lib/rawmakase` on Linux, where
/// the packages and `make install` put private files.
pub fn search_dirs_from(exe_dir: &Path, os: &str) -> Vec<PathBuf> {
    let mut dirs = vec![exe_dir.to_path_buf()];
    if os == "linux" {
        dirs.push(exe_dir.join("../lib/rawmakase"));
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    const MCP: &str = "[extension]\nid = \"rawmakase.mcp\"\nname = \"MCP\"\nversion = \"1.0.0\"\n\
        rawmakase = \"*\"\napi = 1\n[program]\npath = \"rawmakase-mcp\"\n\
        [[control-adapter]]\nid = \"mcp\"\nlaunch = \"external\"\n";

    #[test]
    fn programs_are_found_in_the_first_folder_that_has_them() {
        let dir = tempfile::tempdir().unwrap();
        let (exe, lib) = (dir.path().join("bin"), dir.path().join("lib/rawmakase"));
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::create_dir_all(&lib).unwrap();
        let dirs = search_dirs_from(&exe, "linux");
        assert_eq!(dirs, [exe.clone(), exe.join("../lib/rawmakase")]);
        let found = discover(&[MCP], &dirs);
        let bundled = found[0].as_ref().unwrap();
        assert_eq!(bundled.program, None);
        assert!(bundled.problem().unwrap().contains("rawmakase-mcp"));
        std::fs::write(lib.join(program_name("rawmakase-mcp")), b"").unwrap();
        let found = discover(&[MCP], &dirs);
        let program = found[0].as_ref().unwrap().program.clone().unwrap();
        assert!(program.starts_with(&exe));
        assert!(program.ends_with(program_name("rawmakase-mcp")));
        assert_eq!(found[0].as_ref().unwrap().problem(), None);
    }

    #[test]
    fn a_broken_manifest_is_reported_alone() {
        let found = discover(&["[extension]\nid = \"x\"", MCP], &[]);
        assert!(found[0].is_err());
        assert!(found[1].is_ok());
    }

    #[test]
    fn other_platforms_look_beside_the_executable_only() {
        for os in ["macos", "windows"] {
            assert_eq!(
                search_dirs_from(Path::new("/app"), os),
                [PathBuf::from("/app")]
            );
        }
    }
}
