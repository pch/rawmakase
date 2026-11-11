//! RAWmakase's extension host: everything about extensions below the app's
//! interface, shared by the app and the `rawmakase extensions` command.
//!
//! - [`package`]: reading, verifying and unpacking `.rmext` packages.
//! - [`store`]: the installed extensions under `<data>/extensions/`:
//!   installing, removing, enabling and disabling, through [`state`].
//! - [`state`]: `<data>/extensions.json`, changed only through
//!   [`state::State::update`].
//! - [`github`]: installing and updating from a GitHub repository's releases,
//!   in two steps so the user sees what they are about to trust.
//! - [`bundled`]: the extensions that ship with the app.
//! - [`session`]: starting a program in a process group or job object, the
//!   message channel to it, request deadlines and ending it.
//!
//! The host knows no photo, edit, catalog, renderer or GUI: what an
//! extension's requests mean to the editor is the app's (`app::extensions`).
//! Installing never runs an extension's code.
pub mod bundled;
pub mod github;
pub mod package;
mod process;
pub mod session;
pub mod state;
pub mod store;

pub use rawmakase_extension_protocol as protocol;
pub use rawmakase_extension_protocol::manifest::Manifest;
/// Versions and ranges, as manifests state them.
pub use semver;

use std::path::Path;

/// Why an extension operation failed, in words for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);
impl Error {
    /// An error with this message.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
    /// This error, after `context`: "context: error".
    pub fn context(self, context: impl std::fmt::Display) -> Self {
        Self(format!("{context}: {}", self.0))
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}

/// The result of an extension operation.
pub type Result<T> = std::result::Result<T, Error>;

/// The package target this build runs on, `<os>-<arch>` as release assets
/// name them: `linux-x86_64`, `macos-aarch64`, `windows-x86_64`, …
pub fn current_target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The target of a package with no program, which runs everywhere.
pub const ANY_TARGET: &str = "any";

/// Whether a package for `target` runs here: one for this platform, or one
/// with no program marked `any`.
pub fn check_target(manifest: &Manifest, target: &str) -> Result<()> {
    if target == ANY_TARGET {
        return if manifest.program.is_none() {
            Ok(())
        } else {
            Err(Error::new(
                "A package with a program is built for one platform, not \"any\"",
            ))
        };
    }
    let here = current_target();
    if target == here {
        Ok(())
    } else {
        Err(Error::new(format!(
            "{} {} is built for {target}; this computer needs {here}",
            manifest.extension.name, manifest.extension.version
        )))
    }
}

/// Whether the extension supports this RAWmakase, by its manifest's
/// `rawmakase` range.
pub fn check_compatible(manifest: &Manifest, app: &semver::Version) -> Result<()> {
    let e = &manifest.extension;
    if e.rawmakase.matches(app) {
        Ok(())
    } else {
        Err(Error::new(format!(
            "{} {} supports RAWmakase {}, not {app}",
            e.name, e.version, e.rawmakase
        )))
    }
}

/// Reads and validates the manifest of an unpacked extension.
pub fn read_manifest(dir: &Path) -> Result<Manifest> {
    let path = dir.join(protocol::manifest::FILE);
    let text =
        std::fs::read_to_string(&path).map_err(|e| Error::from(e).context(path.display()))?;
    Manifest::parse(&text).map_err(|e| Error::new(e.0).context(path.display()))
}

/// The program's file in an unpacked extension: its manifest path, with
/// `.exe` added on Windows when the path has no extension.
pub fn program_file(dir: &Path, program: &protocol::manifest::Program) -> std::path::PathBuf {
    let mut path = dir.to_path_buf();
    path.extend(program.path.split('/'));
    if cfg!(windows) && path.extension().is_none() {
        path.set_extension("exe");
    }
    path
}

/// A short random name part, for staging folders and pipe names. Not secret:
/// it only keeps concurrent names apart.
pub(crate) fn random_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    hasher.write_u32(std::process::id());
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(program: bool) -> Manifest {
        let program = if program {
            "[program]\npath = \"bin/x\"\n[[command]]\nid = \"c\"\ntitle = \"C\"\n"
        } else {
            ""
        };
        Manifest::parse(&format!(
            "[extension]\nid = \"dev.test.x\"\nname = \"X\"\nversion = \"1.0.0\"\n\
             rawmakase = \">=0.4, <0.6\"\napi = 1\n{program}"
        ))
        .unwrap()
    }

    #[test]
    fn a_package_runs_on_its_own_target_or_anywhere_without_a_program() {
        let here = current_target();
        assert!(check_target(&manifest(true), &here).is_ok());
        assert!(check_target(&manifest(true), "plan9-mips").is_err());
        assert!(check_target(&manifest(true), ANY_TARGET).is_err());
        assert!(check_target(&manifest(false), ANY_TARGET).is_ok());
    }

    #[test]
    fn the_rawmakase_range_decides_compatibility() {
        let m = manifest(false);
        assert!(check_compatible(&m, &semver::Version::new(0, 4, 12)).is_ok());
        assert!(check_compatible(&m, &semver::Version::new(0, 6, 0)).is_err());
        assert!(check_compatible(&m, &semver::Version::new(0, 3, 9)).is_err());
    }

    #[test]
    fn the_program_file_follows_the_manifest_path() {
        let program = protocol::manifest::Program {
            path: "bin/tool".into(),
            start: Default::default(),
        };
        let file = program_file(Path::new("root"), &program);
        let expected = if cfg!(windows) { "tool.exe" } else { "tool" };
        assert_eq!(file.file_name().unwrap(), expected);
        assert_eq!(file.parent().unwrap(), Path::new("root").join("bin"));
    }
}
