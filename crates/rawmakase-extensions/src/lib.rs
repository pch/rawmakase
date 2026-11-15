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
//! - [`external`]: running a bundled program its clients start themselves
//!   (`rawmakase mcp`), unless its extension is disabled.
//! - [`session`]: starting a program in a process group or job object, the
//!   message channel to it, request deadlines and ending it.
//! - [`secrets`]: where secret settings' values are kept, never in
//!   `extensions.json`.
//! - [`shm`]: the shared memory of the app's request buffers, leased against
//!   the budgets, and passing its handles to the program.
//!
//! The host knows no photo, edit, catalog, renderer or GUI: what an
//! extension's requests mean to the editor is the app's (`app::extensions`).
//! Installing never runs an extension's code.
pub mod bundled;
pub mod external;
pub mod github;
pub mod package;
mod process;
pub mod secrets;
pub mod session;
pub mod shm;
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

/// Most bytes a retouch generator's icon may hold.
pub const MAX_ICON: u64 = 256 * 1024;

/// Checks each retouch generator's icon in the extension at `dir`, as the app
/// will read it: a file of at most [`MAX_ICON`] bytes of UTF-8 text that parses
/// as an SVG. The app has no icon of its own for a generator and shows one that
/// fails as failed, so `cargo rawmakase package` refuses it.
pub fn check_icons(manifest: &Manifest, dir: &Path) -> Result<()> {
    for generator in &manifest.retouch_generators {
        let mut path = dir.to_path_buf();
        path.extend(generator.icon.split('/'));
        let len = std::fs::metadata(&path)
            .map_err(|e| Error::from(e).context(path.display()))?
            .len();
        if len > MAX_ICON {
            return Err(Error::new(format!(
                "{} is larger than an icon may be ({} KB)",
                path.display(),
                MAX_ICON / 1024
            )));
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::from(e).context(path.display()))?;
        let text = String::from_utf8(bytes).map_err(|_| {
            Error::new(format!(
                "{} can't be drawn: it isn't SVG text",
                path.display()
            ))
        })?;
        usvg::Tree::from_str(&text, &usvg::Options::default())
            .map_err(|e| Error::new(format!("{} can't be drawn: {e}", path.display())))?;
    }
    Ok(())
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

    /// A generator's icon is the extension's own, checked before it is
    /// packaged: one missing, too large, not text (a gzipped `.svgz`) or not an
    /// SVG is refused.
    #[test]
    fn a_generators_icon_must_be_an_svg_it_can_draw() {
        let manifest = Manifest::parse(
            "[extension]\nid = \"dev.test.fill\"\nname = \"Fill\"\nversion = \"1.0.0\"\n\
             rawmakase = \">=0.1\"\napi = 3\n[program]\npath = \"bin/fill\"\n\
             [[retouch-generator]]\nid = \"fill\"\nname = \"Fill\"\nicon = \"icons/fill.svg\"\n",
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let error = check_icons(&manifest, dir.path()).unwrap_err();
        assert!(error.0.contains("fill.svg"), "{error}");
        std::fs::create_dir(dir.path().join("icons")).unwrap();
        let icon = dir.path().join("icons/fill.svg");
        for broken in ["<svg", "not an svg"] {
            std::fs::write(&icon, broken).unwrap();
            let error = check_icons(&manifest, dir.path()).unwrap_err();
            assert!(error.0.contains("can't be drawn"), "{error}");
        }
        // A valid SVG gzipped: usvg would read it, the app reads text.
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(
            &mut gz,
            b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"><path d=\"M4 4h16\"/></svg>",
        )
        .unwrap();
        std::fs::write(&icon, gz.finish().unwrap()).unwrap();
        let error = check_icons(&manifest, dir.path()).unwrap_err();
        assert!(error.0.contains("can't be drawn"), "{error}");
        let big = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"><!--{}--></svg>",
            "x".repeat(MAX_ICON as usize)
        );
        std::fs::write(&icon, big).unwrap();
        assert!(check_icons(&manifest, dir.path()).is_err());
        std::fs::write(
            &icon,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" \
             stroke=\"currentColor\"><path d=\"M4 4h16\"/></svg>",
        )
        .unwrap();
        check_icons(&manifest, dir.path()).unwrap();
    }

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
