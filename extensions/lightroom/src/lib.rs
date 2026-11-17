//! The bundled Lightroom extension (`rawmakase.lightroom`): Lightroom
//! Classic catalog import ([`catalog`]), and where Lightroom and Camera Raw
//! keep catalogs, profiles and presets, and what Adobe's folder layout says
//! each file is. It answers `files.list` by listing those folders, reading
//! no file; the app decides what fits a photo or a catalog and imports it
//! with its own importers.
//!
//! A file or folder whose path isn't UTF-8 is left out: the protocol's paths
//! are text, and Lightroom runs on macOS and Windows, whose names are.
use rawmakase_extension::Extension;
use rawmakase_extension::protocol::method::{
    FileFolder, FileKind, FileName, FileOrigin, FilesListed, Searched,
};
use std::path::{Path, PathBuf};

pub mod catalog;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;

/// The folders Adobe's apps keep things in on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roots {
    /// Camera Raw's shared folder, installed with Lightroom for all users:
    /// Adobe's camera profiles, its looks (Adobe Color…) and lens profiles.
    pub shared: Option<PathBuf>,
    /// Camera Raw's per-user folder: the user's presets and third-party
    /// profiles.
    pub user: Option<PathBuf>,
    /// The user's Pictures folder, where Lightroom Classic keeps its catalog
    /// and Lightroom its library unless told otherwise.
    pub pictures: Option<PathBuf>,
}

const CAMERA_RAW: &str = "Library/Application Support/Adobe/CameraRaw";

impl Roots {
    /// This computer's. Camera Raw runs on macOS and Windows only; elsewhere
    /// only the Pictures folder is looked in.
    pub fn here() -> Self {
        let var = |name| std::env::var_os(name).map(PathBuf::from);
        if cfg!(target_os = "macos") {
            Self::macos(Path::new("/"), var("HOME"))
        } else if cfg!(windows) {
            Self::windows(
                Path::new("C:\\ProgramData"),
                var("APPDATA"),
                var("USERPROFILE"),
            )
        } else {
            Self {
                pictures: var("HOME").map(|home| home.join("Pictures")),
                ..Self::default()
            }
        }
    }
    /// macOS's layout under `root` (`/` on a Mac), for the user whose home is
    /// `home`.
    pub fn macos(root: &Path, home: Option<PathBuf>) -> Self {
        Self {
            shared: Some(root.join(CAMERA_RAW)),
            user: home.as_ref().map(|home| home.join(CAMERA_RAW)),
            pictures: home.map(|home| home.join("Pictures")),
        }
    }
    /// Windows's layout: `program_data` (`C:\ProgramData`), and the user's
    /// `%APPDATA%` and `%USERPROFILE%`.
    pub fn windows(
        program_data: &Path,
        app_data: Option<PathBuf>,
        profile: Option<PathBuf>,
    ) -> Self {
        let camera_raw = |dir: &Path| dir.join("Adobe").join("CameraRaw");
        Self {
            shared: Some(camera_raw(program_data)),
            user: app_data.as_deref().map(camera_raw),
            pictures: profile.map(|profile| profile.join("Pictures")),
        }
    }
}

/// The extension's program, listing `roots`.
pub fn extension(roots: Roots) -> Extension {
    Extension::new()
        .files(move |_| Ok(list(&roots)))
        .importer("catalog", catalog::Catalog)
}

/// What `roots` hold, a folder at a time: Adobe's camera profiles (Adobe
/// Standard, then the Camera Matching folders), its looks and lens profiles;
/// the user's lens and camera profiles and preset folders; Lightroom's synced
/// preset folders and Lightroom Classic's catalogs. And where it looked for
/// profiles and catalogs.
pub fn list(roots: &Roots) -> FilesListed {
    use {FileKind::*, FileOrigin::*};
    let mut out = FilesListed::default();
    let mut add = |folder: FileFolder| {
        if (!folder.files.is_empty() || folder.kind == Presets) && utf8(&folder.path) {
            out.folders.push(folder);
        }
    };
    if let Some(shared) = &roots.shared {
        let profiles = shared.join("CameraProfiles");
        let standard = profiles.join("Adobe Standard");
        let mut folder = folder(CameraProfile, Shared, &standard, walk(&standard, "dcp"));
        for file in &mut folder.files {
            file.camera = standard_camera(&file.name);
        }
        add(folder);
        for path in folders(&profiles.join("Camera")) {
            let camera = Some(name(&path));
            let files = walk(&path, "dcp");
            add(FileFolder {
                camera,
                ..self::folder(CameraProfile, Shared, &path, files)
            });
        }
        let looks = shared.join("Settings/Adobe/Profiles/Adobe Raw");
        add(self::folder(Look, Shared, &looks, walk(&looks, "xmp")));
        // Grouped by maker: "LensProfiles/1.0/Sigma/….lcp".
        for path in folders(&shared.join("LensProfiles").join("1.0")) {
            let group = Some(name(&path));
            let files = walk(&path, "lcp");
            add(FileFolder {
                group,
                ..self::folder(LensProfile, Shared, &path, files)
            });
        }
    }
    if let Some(user) = &roots.user {
        let lenses = user.join("LensProfiles");
        add(folder(LensProfile, User, &lenses, walk(&lenses, "lcp")));
        let profiles = user.join("CameraProfiles");
        add(folder(
            CameraProfile,
            User,
            &profiles,
            walk(&profiles, "dcp"),
        ));
    }
    // Lightroom Classic's presets, then those Lightroom (not Classic) syncs
    // into a folder of its library for each Adobe account.
    let classic = roots.user.iter().map(|user| user.join("Settings"));
    let library = roots
        .pictures
        .iter()
        .map(|p| p.join("Lightroom Library.lrlibrary"));
    let synced = library
        .flat_map(|l| folders(&l))
        .map(|a| a.join("cr_settings"));
    for presets in classic.chain(synced).filter(|p| p.is_dir()) {
        add(folder(Presets, User, &presets, Vec::new()));
    }
    if let Some(pictures) = &roots.pictures {
        let catalogs = pictures.join("Lightroom");
        let mut folder = folder(Catalog, User, &catalogs, files_here(&catalogs, "lrcat"));
        for file in &mut folder.files {
            file.used = std::fs::metadata(catalogs.join(&file.name))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64);
        }
        add(folder);
        if utf8(&catalogs) {
            out.searched.push(Searched {
                kind: Catalog,
                origin: User,
                path: catalogs,
            });
        }
    }
    // Where profiles were looked for: Camera Raw's folders that are here.
    for (origin, root) in [(Shared, &roots.shared), (User, &roots.user)] {
        for root in root.iter().filter(|r| r.is_dir()) {
            for (kind, path) in [
                (CameraProfile, "CameraProfiles"),
                (LensProfile, "LensProfiles"),
            ] {
                let path = root.join(path);
                if utf8(&path) {
                    out.searched.push(Searched { kind, origin, path });
                }
            }
        }
    }
    out
}

/// `files`, in `path`, as a folder of `kind`.
fn folder(kind: FileKind, origin: FileOrigin, path: &Path, files: Vec<PathBuf>) -> FileFolder {
    let files = files
        .iter()
        .map(|p| {
            let relative = p.strip_prefix(path).unwrap_or(p);
            let parts: Vec<_> = relative.iter().map(|c| c.to_string_lossy()).collect();
            FileName {
                name: parts.join("/"),
                camera: None,
                used: None,
            }
        })
        .collect();
    FileFolder {
        kind,
        origin,
        path: path.to_path_buf(),
        camera: None,
        group: None,
        files,
    }
}

/// The camera an Adobe Standard profile's name says it is for: "Sony
/// ILCE-7M2 Adobe Standard.dcp" is for the "Sony ILCE-7M2".
fn standard_camera(name: &str) -> Option<String> {
    let stem = Path::new(name).file_stem()?.to_str()?;
    let lower = stem.to_ascii_lowercase();
    let end = lower
        .rfind(" adobe standard")
        .or_else(|| lower.rfind(" adobe_standard"))?;
    Some(stem[..end].to_string())
}

fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn utf8(path: &Path) -> bool {
    path.to_str().is_some()
}

/// The folders in `dir`, sorted.
fn folders(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && utf8(p))
        .collect();
    found.sort();
    found
}

/// The files in `dir` with extension `ext`, sorted.
fn files_here(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && has_extension(p, ext) && utf8(p))
        .collect();
    found.sort();
    found
}

/// The files with extension `ext` in `dir` and its subfolders, sorted,
/// skipping hidden names and `__MACOSX`, as the app's importers walk a
/// folder.
fn walk(dir: &Path, ext: &str) -> Vec<PathBuf> {
    fn into(dir: &Path, ext: &str, depth: usize, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            // Follows links, so the depth limit also stops link cycles.
            if path.is_dir() {
                if depth < 12 && name != "__MACOSX" {
                    into(&path, ext, depth + 1, out);
                }
            } else if has_extension(&path, ext) && utf8(&path) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    into(dir, ext, 0, &mut out);
    out.sort();
    out
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

#[cfg(test)]
mod tests;
