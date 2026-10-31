use super::{
    CameraProfile,
    cube_look::{self, Fidelity, LutSpace, Names, Placement},
    enhanced::{LookBase, LookFile},
    from_bytes,
};
use crate::camera_data::Metadata;
use anyhow::{Context, Result, ensure};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
pub fn load(path: &Path, m: &Metadata) -> Result<Arc<CameraProfile>> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
    {
        ensure!(
            std::fs::metadata(path)?.len() <= 16_000_000,
            "Profile too large"
        );
        let look = LookFile::read(path)?;
        let (profiles, _) = installed(m);
        let base = look_base(&look, &profiles, builtin(m).as_deref())
            .context("Unsupported XMP look or missing matching base camera profile")?;
        return Ok(Arc::new(look.compose(base)?));
    }
    let p = read_dcp(path)?;
    p.ensure_camera(m)?;
    Ok(Arc::new(p))
}
fn read_dcp(path: &Path) -> Result<CameraProfile> {
    ensure!(
        std::fs::metadata(path)?.len() <= 16_000_000,
        "Profile too large"
    );
    from_bytes(&std::fs::read(path)?)
}
/// What tells that a profile file may have changed: its size, its modification time
/// and, on Unix, its status change time, which any write or permission change moves
/// even when the modification time is put back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}
impl Stamp {
    fn read(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        Some(Self {
            size: m.len(),
            modified: m.modified().ok(),
            #[cfg(unix)]
            changed: {
                use std::os::unix::fs::MetadataExt;
                (m.ctime(), m.ctime_nsec())
            },
        })
    }
}
/// The camera each DCP in the library was made for, as last read, so a scan parses
/// only the files of the photo's camera: a library holding Adobe's profiles for
/// every camera has thousands. A profile's camera comes from its bytes alone; a
/// file whose [`Stamp`] changed is read again, and one that failed to read is not
/// remembered, so its error is reported every time.
#[derive(Default)]
struct Cameras {
    known: Mutex<HashMap<PathBuf, (Stamp, String)>>,
    /// Files read, for the tests.
    #[cfg(test)]
    reads: std::sync::atomic::AtomicUsize,
}
impl Cameras {
    fn get(&self, path: &Path, stamp: Stamp) -> Option<String> {
        let map = self.known.lock().unwrap_or_else(|e| e.into_inner());
        map.get(path)
            .filter(|(s, _)| *s == stamp)
            .map(|(_, camera)| camera.clone())
    }
    fn set(&self, path: &Path, stamp: Stamp, camera: &str) {
        #[cfg(test)]
        self.reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut map = self.known.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(path.to_path_buf(), (stamp, camera.to_string()));
    }
    /// Forgets the files a scan no longer found.
    fn keep(&self, found: &HashSet<&Path>) {
        let mut map = self.known.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|path, _| found.contains(path.as_path()));
    }
}
static CAMERAS: std::sync::LazyLock<Cameras> = std::sync::LazyLock::new(Cameras::default);
/// The profile a DNG embeds for its camera, which Lightroom lists as the file's own.
/// No Adobe profiles are bundled; RAWmakase's own profiles are in `open`.
pub fn builtin(m: &Metadata) -> Option<Arc<CameraProfile>> {
    let dcp = m.embedded_dcp.as_deref()?;
    super::from_bytes(dcp).ok().map(Arc::new)
}
/// User-installed profiles stay outside the source tree and are filtered by camera.
pub fn library_dirs() -> Vec<std::path::PathBuf> {
    crate::storage::asset_dirs()
        .into_iter()
        .map(|p| p.join("camera-profiles"))
        .collect()
}

/// What the profile library holds, as one hash: each file's path, size and
/// modification time. Unedited photos' previews follow the library (which profile
/// a default picks, and whether a photo without colour data gets a fitted look,
/// depend on it), so importing or removing any profile changes it.
pub fn fingerprint() -> String {
    fingerprint_of(&library_dirs())
}
pub(crate) fn fingerprint_of(dirs: &[std::path::PathBuf]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for dir in dirs {
        let mut files = Vec::new();
        collect(dir, 0, &mut files);
        for file in files {
            h.update(file.to_string_lossy().as_bytes());
            if let Ok(meta) = std::fs::metadata(&file) {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                h.update(meta.len().to_le_bytes());
                h.update(modified.to_le_bytes());
            }
            h.update([0]);
        }
    }
    h.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn installed(m: &Metadata) -> (Vec<Arc<CameraProfile>>, Vec<String>) {
    let listed = listed(m);
    (listed.profiles, listed.errors)
}
/// The profiles for a photo, as [`installed`] lists them, with where the profile
/// menu shows each.
pub fn listed(m: &Metadata) -> Listed {
    listed_in(&library_dirs(), m, &CAMERAS)
}
/// The profiles that fit a photo, name-sorted, the files that failed to read, and
/// the groups of the looks among them.
#[derive(Clone, Debug, Default)]
pub struct Listed {
    pub profiles: Vec<Arc<CameraProfile>>,
    pub errors: Vec<String>,
    pub groups: Groups,
}
/// The `crs:Group` of each look listed, by its name and UUID, and the menu titles
/// of camera profile files that share a name and camera. Kept beside the profiles
/// rather than in them: a profile is embedded in every recipe that uses it, and
/// earlier releases refuse a recipe whose profile has a field they lack. Shared,
/// as the menu takes a copy each frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Groups {
    groups: Arc<HashMap<(String, String), String>>,
    /// Each DCP whose name and camera another listed DCP has too, titled after
    /// its file so the menu can tell them apart.
    titles: Arc<Vec<(Arc<CameraProfile>, String)>>,
}
/// Where the profile menu lists a profile: at the top level or in a group's
/// submenu, under a section heading inside it if any, as `title`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub group: Option<String>,
    /// A heading inside the group. Imported looks have none; it is for a set of
    /// looks whose variants belong together, such as a film stock's.
    pub section: Option<String>,
    pub title: String,
}
impl Groups {
    fn insert(&mut self, name: &str, uuid: &str, group: &str) {
        Arc::make_mut(&mut self.groups)
            .entry((name.into(), uuid.to_ascii_uppercase()))
            .or_insert_with(|| group.into());
    }
    /// Where the menu lists `profile`. A look goes in the submenu of its group;
    /// camera profiles (DCPs, a DNG's own), RAWmakase's and a look without a group
    /// stay at the top level, as does a look in Lightroom's "Profiles" group:
    /// Adobe Color and the other Adobe Raw and Camera Matching looks, which
    /// Lightroom lists with the camera's profiles. A DCP that shares its name and
    /// camera with another is titled with its file's name too.
    pub fn entry(&self, profile: &CameraProfile) -> MenuEntry {
        let group = profile
            .enhanced
            .as_ref()
            .and_then(|look| {
                self.groups
                    .get(&(profile.name.clone(), look.uuid.to_ascii_uppercase()))
            })
            .filter(|group| !group.is_empty() && *group != "Profiles")
            .cloned();
        let title = self
            .titles
            .iter()
            .find(|(p, _)| p.name == profile.name && p.camera == profile.camera && **p == *profile)
            .map_or_else(
                || super::fitted::title(profile).unwrap_or_else(|| profile.name.clone()),
                |(_, title)| title.clone(),
            );
        MenuEntry {
            group,
            section: None,
            title,
        }
    }
}
/// [`listed`] from `dirs` instead of the user's library, for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn listed_from(dirs: &[PathBuf], m: &Metadata) -> Listed {
    listed_in(dirs, m, &Cameras::default())
}
/// [`import_files`] into `destination` instead of the user's library, for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn import_into_dir(
    paths: &[PathBuf],
    destination: &Path,
    space: LutSpace,
) -> Result<Vec<ImportedProfile>> {
    import_into(paths, destination, space)
}
fn listed_in(dirs: &[PathBuf], m: &Metadata, cameras: &Cameras) -> Listed {
    if m.is_rendered() {
        return Listed {
            profiles: vec![Arc::new(CameraProfile::embedded(m))],
            ..Listed::default()
        };
    }
    let mut profiles = Vec::new();
    let mut errors = Vec::new();
    let mut groups = Groups::default();
    if let Some(p) = builtin(m) {
        profiles.push(p);
    }
    profiles.extend(
        [super::open::standard(m), super::open::color(m)]
            .into_iter()
            .flatten()
            .map(Arc::new),
    );
    let mut files = Vec::new();
    for dir in dirs {
        collect(dir, 0, &mut files);
    }
    let mut looks = Vec::new();
    // The profiles before the library's: the library's DCPs that only repeat one
    // of these by name and camera stay hidden behind it.
    let own = profiles.len();
    // Each library DCP listed, with the file it came from.
    let mut sources: Vec<(Arc<CameraProfile>, &Path)> = Vec::new();
    for path in &files {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
        {
            looks.push(path);
            continue;
        }
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
        {
            continue;
        }
        // Read before the file, so a change while reading it is read again next time.
        let stamp = Stamp::read(path);
        if let Some(camera) = stamp.and_then(|s| cameras.get(path, s))
            && !super::camera_fits(&camera, m)
        {
            continue;
        }
        let loaded = read_dcp(path).and_then(|p| {
            if let Some(stamp) = stamp {
                cameras.set(path, stamp, &p.camera);
            }
            p.ensure_camera(m)?;
            Ok(Arc::new(p))
        });
        match loaded {
            Ok(p) => {
                // A DCP that shares its name and camera with another file is
                // listed too unless it is the same profile, so either can be chosen;
                // lookups by name still find the first file, as before.
                let same = |old: &Arc<CameraProfile>| old.name == p.name && old.camera == p.camera;
                if !profiles[..own].iter().any(same)
                    && !sources.iter().any(|(old, _)| same(old) && **old == *p)
                {
                    sources.push((p.clone(), path));
                    profiles.push(p);
                }
            }
            Err(e) => {
                // Other camera models are expected in a shared user library.
                if e.downcast_ref::<super::OtherCamera>().is_none() {
                    errors.push(format!("{}: {e:#}", path.display()));
                }
            }
        }
    }
    cameras.keep(&files.iter().map(PathBuf::as_path).collect());
    groups.titles = Arc::new(file_titles(&sources, dirs));
    let bases = profiles.clone();
    for path in looks {
        let composed = LookFile::read(path).and_then(|look| {
            let base = look_base(&look, &bases, builtin(m).as_deref()).with_context(|| {
                let name = match &look.base {
                    LookBase::Named(name) => name.as_str(),
                    LookBase::Any => "",
                };
                format!("Missing matching base camera profile {name}")
            })?;
            Ok((look.compose(base)?, look.group))
        });
        match composed {
            Ok((p, group)) => {
                if !profiles
                    .iter()
                    .any(|old| old.name == p.name && old.camera == p.camera)
                {
                    if let Some(look) = &p.enhanced {
                        groups.insert(&p.name, &look.uuid, &group);
                    }
                    profiles.push(Arc::new(p));
                }
            }
            Err(e) => errors.push(format!("{}: {e:#}", path.display())),
        }
    }
    errors.sort();
    errors.dedup();
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Listed {
        profiles,
        errors,
        groups,
    }
}
/// The menu titles of the library DCPs in `sources` that share a name and camera:
/// the name, then the file's path in its library folder, or its full path when
/// another library folder has a file at the same place.
fn file_titles(
    sources: &[(Arc<CameraProfile>, &Path)],
    dirs: &[PathBuf],
) -> Vec<(Arc<CameraProfile>, String)> {
    let shared: Vec<_> = sources
        .iter()
        .filter(|(p, _)| {
            sources
                .iter()
                .filter(|(old, _)| old.name == p.name && old.camera == p.camera)
                .count()
                > 1
        })
        .map(|(p, path)| {
            let file = dirs
                .iter()
                .find_map(|dir| path.strip_prefix(dir).ok())
                .unwrap_or(path);
            (p, *path, file)
        })
        .collect();
    shared
        .iter()
        .map(|(p, path, file)| {
            let clash = shared.iter().any(|(old, other, same)| {
                other != path && same == file && old.name == p.name && old.camera == p.camera
            });
            let file = if clash { path } else { file };
            ((*p).clone(), format!("{} ({})", p.name, file.display()))
        })
        .collect()
}
/// A look profile file over the profile it builds on among `bases` (see `look_base`),
/// without the user's library. `own` is the file's embedded profile, if any.
pub fn compose_look(
    path: &Path,
    bases: &[Arc<CameraProfile>],
    own: Option<&CameraProfile>,
) -> Result<CameraProfile> {
    let look = LookFile::read(path)?;
    look.compose(look_base(&look, bases, own).context("Missing matching base camera profile")?)
}
/// The profile a look goes over: the one it names, or for a creative look, which
/// names none, Adobe Standard as Lightroom uses, else the file's own profile (`own`),
/// else RAWmakase Standard, else any that fits. A look restricted to another camera
/// fits none.
pub(super) fn look_base<'a>(
    look: &LookFile,
    bases: &'a [Arc<CameraProfile>],
    own: Option<&CameraProfile>,
) -> Option<&'a CameraProfile> {
    let fitting = || {
        bases
            .iter()
            .filter(|b| look.fits(b) && look.compose(b).is_ok())
    };
    match &look.base {
        LookBase::Named(_) => fitting().next(),
        LookBase::Any => ["Adobe Standard"]
            .into_iter()
            .chain(own.map(|p| p.name.as_str()))
            .chain([super::open::STANDARD])
            .find_map(|name| fitting().find(|b| b.name == name))
            .or_else(|| fitting().next()),
    }
    .map(|b| &**b)
}
fn collect(dir: &Path, depth: usize, files: &mut Vec<std::path::PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect(&entry.path(), depth + 1, files);
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
}

/// Explicitly import files into RAWmakase's own library: DCPs and look XMPs are
/// copied, and a `.cube` becomes a look XMP (see `cube_look`) for colours in `space`,
/// one for the whole batch (DCPs and XMPs have their own). Validate the complete batch
/// before writing; never follow or discover Adobe application directories.
pub fn import_files(paths: &[std::path::PathBuf], space: LutSpace) -> Result<Vec<ImportedProfile>> {
    import_into(
        paths,
        &crate::storage::data_dir().join("camera-profiles"),
        space,
    )
}
/// One file [`import_files`] took into the library.
#[derive(Clone, Debug)]
pub struct ImportedProfile {
    /// The file chosen.
    pub source: PathBuf,
    /// The library's file: a DCP's or look's copy, or the look made from a `.cube`.
    pub path: PathBuf,
    /// For a `.cube`, the look it became.
    pub cube: Option<CubeImport>,
}
/// What became of a `.cube`.
#[derive(Clone, Debug)]
pub struct CubeImport {
    /// The look's name in the profile list.
    pub name: String,
    /// The cube's file stem, which the look is named after unless a different
    /// profile has that name.
    pub stem: String,
    /// In the library already, under `name`: nothing was written.
    pub already: bool,
    /// How faithfully the look holds the cube.
    pub fidelity: Fidelity,
    /// The colour space the cube was imported as expecting.
    pub space: LutSpace,
}
impl CubeImport {
    /// Named other than its file, as a different profile has that name.
    pub fn renamed(&self) -> bool {
        self.name != self.stem
    }
}
impl std::fmt::Display for CubeImport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // sRGB, the default, goes unsaid, as before the space could be chosen.
        let space = match self.space {
            LutSpace::Srgb => String::new(),
            space => format!(" in {space}"),
        };
        if self.already {
            return write!(f, "already imported as look \"{}\"{space}", self.name);
        }
        write!(f, "imported as look \"{}\"{space}", self.name)?;
        if self.renamed() {
            write!(f, ", as another profile is named \"{}\"", self.stem)?;
        }
        write!(f, ": {}", self.fidelity)
    }
}
/// Whether a `.cube` can be imported, and how faithfully: what [`import_files`] would
/// make of its bytes, in any space (the space changes neither).
pub fn read_cube(bytes: &[u8]) -> Result<Fidelity> {
    ensure!(
        bytes.len() as u64 <= cube_look::MAX_BYTES,
        "the cube is too large"
    );
    Ok(cube_look::convert(bytes, LutSpace::Srgb)?.fidelity)
}
/// What [`import_readable`] did.
#[derive(Debug, Default)]
pub struct Imported {
    /// The library's copies.
    pub imported: Vec<std::path::PathBuf>,
    /// Files left out, with why RAWmakase can't read them.
    pub skipped: Vec<(std::path::PathBuf, String)>,
}
/// [`import_files`] for a set the user didn't pick file by file (a camera's
/// profiles from Camera Raw's folder): DCPs RAWmakase can't read, such as Camera
/// Raw's HDR profiles, are skipped instead of failing the rest. A `.cube` among
/// them imports at the sRGB default.
pub fn import_readable(paths: &[std::path::PathBuf]) -> Result<Imported> {
    import_readable_into(paths, &crate::storage::data_dir().join("camera-profiles"))
}
fn import_readable_into(paths: &[std::path::PathBuf], destination: &Path) -> Result<Imported> {
    let mut readable = Vec::new();
    let mut skipped = Vec::new();
    for path in paths {
        let dcp = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dcp"));
        match std::fs::read(path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                if dcp {
                    from_bytes(&bytes)?;
                }
                Ok(())
            }) {
            Ok(()) => readable.push(path.clone()),
            Err(e) => skipped.push((path.clone(), format!("{e:#}"))),
        }
    }
    let imported = if readable.is_empty() {
        Vec::new()
    } else {
        import_into(&readable, destination, LutSpace::Srgb)?
            .into_iter()
            .map(|done| done.path)
            .collect()
    };
    Ok(Imported { imported, skipped })
}
pub(super) fn import_into(
    paths: &[std::path::PathBuf],
    destination: &Path,
    space: LutSpace,
) -> Result<Vec<ImportedProfile>> {
    use anyhow::Context;
    use std::io::Write;
    ensure!(
        !paths.is_empty() && paths.len() <= 1024,
        "Choose 1–1024 profile files"
    );
    let mut files = Vec::new();
    let mut bases = Vec::new();
    // The names a cube's look must not take: the library's profiles and looks, this
    // import's, and RAWmakase's own.
    let mut names = Names::default();
    // Lightroom edits naming Adobe's two fall back to RAWmakase's by name alone.
    for name in [
        super::open::STANDARD,
        super::open::COLOR,
        super::fitted::LOOK,
        super::fitted::MATRIX_BASE,
        "Adobe Standard",
        "Adobe Color",
    ] {
        names.profile(name);
    }
    let mut existing = Vec::new();
    collect(destination, 0, &mut existing);
    let mut looks = Vec::new();
    for p in existing {
        if p.parent() == Some(destination)
            && let Some(file) = p.file_name()
        {
            names.file(&file.to_string_lossy());
        }
        let ext = |e: &str| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(e));
        if ext("dcp")
            && let Ok(bytes) = std::fs::read(&p)
            && let Ok(profile) = from_bytes(&bytes)
        {
            names.profile(&profile.name);
            bases.push(profile);
        } else if ext("xmp") {
            looks.push(p);
        }
    }
    let mut cubes = Vec::new();
    for path in paths {
        let ext = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        ensure!(
            matches!(ext.as_str(), "dcp" | "xmp" | "cube"),
            "Choose DCP or XMP profiles or .cube files"
        );
        let limit = if ext == "cube" {
            cube_look::MAX_BYTES
        } else {
            16_000_000
        };
        ensure!(
            std::fs::metadata(path)?.len() <= limit,
            "Profile too large: {}",
            path.display()
        );
        let bytes = std::fs::read(path)?;
        if ext == "cube" {
            cubes.push((path, bytes));
            continue;
        }
        if ext == "dcp" {
            let profile = from_bytes(&bytes).with_context(|| format!("{}", path.display()))?;
            names.profile(&profile.name);
            bases.push(profile);
        }
        let target = destination.join(path.file_name().context("Missing profile filename")?);
        if target.exists() {
            ensure!(
                std::fs::read(&target)? == bytes,
                "A different profile named {} is already imported",
                target.file_name().unwrap().to_string_lossy()
            );
        }
        if let Some((_, _, previous, _, _)) = files.iter().find(|(_, p, _, _, _)| *p == target) {
            ensure!(
                *previous == bytes,
                "Conflicting profile filenames in import"
            );
        } else {
            names.file(&target.file_name().unwrap().to_string_lossy());
            files.push((path.clone(), target, bytes, ext, None));
        }
    }
    for (_, path, bytes, ext, _) in &files {
        if ext == "xmp" {
            let file = path.file_name().unwrap().to_string_lossy();
            let text = std::str::from_utf8(bytes)?;
            let look = LookFile::parse(text).with_context(|| format!("{file}"))?;
            let (name, uuid) = LookFile::identity(text)?;
            names.look(&name, &uuid, path);
            if let LookBase::Named(name) = &look.base {
                // The base must also be for the camera the look is restricted to.
                if !bases
                    .iter()
                    .any(|base| look.fits(base) && look.compose(base).is_ok())
                {
                    return Err(super::MissingBase {
                        file: file.into_owned(),
                        name: name.clone(),
                    }
                    .into());
                }
            }
        }
    }
    // The library's looks, read only when a cube needs their names.
    if !cubes.is_empty() {
        for p in looks {
            if std::fs::metadata(&p).is_ok_and(|m| m.len() <= 16_000_000)
                && let Ok(text) = std::fs::read_to_string(&p)
                && let Ok((name, uuid)) = LookFile::identity(&text)
            {
                names.look(&name, &uuid, &p);
            }
        }
    }
    // Cubes last, so their names avoid every other profile's in this import.
    let mut imported = Vec::new();
    for (path, bytes) in cubes {
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let converted = cube_look::convert(&bytes, space).with_context(|| format!("{file}"))?;
        let (stem, group) = cube_look::stem_and_group(path)?;
        let report = |name: String, already| CubeImport {
            name,
            stem: stem.clone(),
            already,
            fidelity: converted.fidelity.clone(),
            space,
        };
        match names.place(&stem, &group, &converted.uuid)? {
            Placement::Already(name, library) => imported.push(ImportedProfile {
                source: path.clone(),
                path: library,
                cube: Some(report(name, true)),
            }),
            Placement::New(name) => {
                let file = names.file_for(&name);
                let target = destination.join(&file);
                names.file(&file);
                names.look(&name, &converted.uuid, &target);
                let xml = cube_look::look_xmp(&converted, &name, &group);
                files.push((
                    path.clone(),
                    target,
                    xml.into_bytes(),
                    "cube".into(),
                    Some(report(name, false)),
                ));
            }
        }
    }
    std::fs::create_dir_all(destination)?;
    let mut written = Vec::new();
    for (source, target, bytes, _, cube) in files {
        // A copy equal to the library's file is there already; a cube's look is
        // new, and a file that appeared under its name since is an error.
        if cube.is_some() || !target.exists() {
            crate::storage::write_atomic(&target, crate::storage::Replace::NoClobber, |f| {
                Ok(f.write_all(&bytes)?)
            })?;
        }
        written.push(ImportedProfile {
            source,
            path: target,
            cube,
        });
    }
    written.extend(imported);
    // In the order chosen.
    written.sort_by_key(|done| paths.iter().position(|p| *p == done.source));
    Ok(written)
}

/// Whether `name` is one of the photo's cameras followed by any phone module, so
/// the import offers the phone's other modules too.
fn module_of_camera(name: &str, m: &Metadata) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.match_indices(' ').any(|(i, _)| {
        super::PhoneModule::parse(&lower[i + 1..]).is_some() && super::camera_fits(&name[..i], m)
    })
}

/// Adobe's own profiles for this camera from a local Lightroom / Camera Raw
/// installation (Adobe Standard plus Camera Matching), skipping ones already
/// in the library, so the app can offer to import them. Nothing is read from
/// those folders otherwise; rendering only sees imported profiles.
pub fn adobe_installed(m: &Metadata) -> Vec<std::path::PathBuf> {
    let root = Path::new(if cfg!(target_os = "macos") {
        "/Library/Application Support/Adobe/CameraRaw/CameraProfiles"
    } else if cfg!(windows) {
        "C:\\ProgramData\\Adobe\\CameraRaw\\CameraProfiles"
    } else {
        return Vec::new();
    });
    // Every Adobe Standard whose camera fits the photo, under the names Adobe
    // uses for it (see `CameraProfile::ensure_camera`), every module's for a phone
    // so photos from its other cameras fit too, and its Camera Matching folder.
    let fits = |name: &str| super::camera_fits(name, m) || module_of_camera(name, m);
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("Adobe Standard")) {
        paths.extend(entries.flatten().map(|e| e.path()).filter(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| {
                    let lower = s.to_ascii_lowercase();
                    let end = lower
                        .rfind(" adobe standard")
                        .or_else(|| lower.rfind(" adobe_standard"))?;
                    Some(s[..end].to_string())
                })
                .is_some_and(|camera| fits(&camera))
        }));
    }
    if let Ok(entries) = std::fs::read_dir(root.join("Camera")) {
        for dir in entries.flatten().map(|e| e.path()) {
            if dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| super::camera_fits(n, m))
                && let Ok(files) = std::fs::read_dir(&dir)
            {
                paths.extend(files.flatten().map(|e| e.path()));
            }
        }
    }
    let installed = crate::storage::data_dir().join("camera-profiles");
    paths
        .into_iter()
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
                && p.file_name().is_some_and(|n| !installed.join(n).exists())
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn installed_in(
        dirs: &[PathBuf],
        m: &Metadata,
        cameras: &Cameras,
    ) -> (Vec<Arc<CameraProfile>>, Vec<String>) {
        let listed = listed_in(dirs, m, cameras);
        (listed.profiles, listed.errors)
    }
    /// A minimal DCP: the camera it is for, its name and an identity forward matrix.
    fn dcp(camera: &str, name: &str) -> Vec<u8> {
        dcp_with(camera, name, [])
    }
    /// [`dcp`] with more tags after its own, in ascending order.
    fn dcp_with<const N: usize>(
        camera: &str,
        name: &str,
        more: [(u16, u16, Vec<u8>); N],
    ) -> Vec<u8> {
        let text = |s: &str| [s.as_bytes(), b"\0"].concat();
        let matrix: Vec<u8> = (0..9)
            .flat_map(|i| [i32::from(i % 4 == 0), 1])
            .flat_map(i32::to_le_bytes)
            .collect();
        let entries: Vec<_> = [
            (50708u16, 2u16, text(camera)),
            (50936, 2, text(name)),
            (50964, 10, matrix),
        ]
        .into_iter()
        .chain(more)
        .collect();
        let mut data_at = 8 + 2 + entries.len() * 12 + 4;
        let mut b = b"IIRC".to_vec();
        b.extend(8u32.to_le_bytes());
        b.extend((entries.len() as u16).to_le_bytes());
        let mut data: Vec<u8> = Vec::new();
        for (tag, kind, bytes) in &entries {
            let count = bytes.len()
                / match kind {
                    5 | 10 => 8,
                    4 | 9 | 11 => 4,
                    3 | 8 => 2,
                    _ => 1,
                };
            b.extend(tag.to_le_bytes());
            b.extend(kind.to_le_bytes());
            b.extend((count as u32).to_le_bytes());
            b.extend((data_at as u32).to_le_bytes());
            data_at += bytes.len();
            data.extend(bytes);
        }
        b.extend(0u32.to_le_bytes());
        b.extend(data);
        b
    }
    fn camera(model: &str) -> Metadata {
        Metadata {
            make: "Sony".into(),
            model: model.into(),
            ..Default::default()
        }
    }
    fn names(profiles: &[Arc<CameraProfile>]) -> Vec<&str> {
        profiles
            .iter()
            .filter(|p| p.camera.starts_with("Sony"))
            .map(|p| p.name.as_str())
            .collect()
    }
    #[test]
    fn a_rescan_reads_only_the_photos_cameras_profiles_and_changed_files() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let dirs = [dir.path().to_path_buf()];
        let (mine, other) = (dir.path().join("mine.dcp"), dir.path().join("other.dcp"));
        std::fs::write(&mine, dcp("Sony ILCE-7M2", "Mine"))?;
        std::fs::write(&other, dcp("Sony ILCE-7M4", "Other"))?;
        let cameras = Cameras::default();
        let m = camera("ILCE-7M2");
        let (profiles, errors) = installed_in(&dirs, &m, &cameras);
        assert_eq!((names(&profiles), errors.len()), (vec!["Mine"], 0));
        let reads = || cameras.reads.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(reads(), 2);
        // While unchanged, the other camera's file is not read again; this
        // camera's is, for its profile.
        let (profiles, errors) = installed_in(&dirs, &m, &cameras);
        assert_eq!((names(&profiles), errors.len()), (vec!["Mine"], 0));
        assert_eq!(reads(), 3);
        // Changed, it is read again: now made for this camera.
        std::fs::write(&other, dcp("Sony ILCE-7M2", "Now mine"))?;
        let (profiles, _) = installed_in(&dirs, &m, &cameras);
        assert_eq!(names(&profiles), ["Mine", "Now mine"]);
        // A file that cannot be read reports its error on every scan.
        std::fs::write(&other, b"not a profile")?;
        for _ in 0..2 {
            let (profiles, errors) = installed_in(&dirs, &m, &cameras);
            assert_eq!((names(&profiles), errors.len()), (vec!["Mine"], 1));
        }
        // A removed file is forgotten.
        std::fs::remove_file(&other)?;
        installed_in(&dirs, &m, &cameras);
        assert_eq!(cameras.known.lock().unwrap().len(), 1);
        // Another camera's photo gets its own profiles from the same index.
        std::fs::write(&other, dcp("Sony ILCE-7M4", "Other"))?;
        let (profiles, _) = installed_in(&dirs, &camera("ILCE-7M4"), &cameras);
        assert_eq!(names(&profiles), ["Other"]);
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn a_rescan_notices_changes_that_keep_the_size_and_modification_time() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let dirs = [dir.path().to_path_buf()];
        let other = dir.path().join("other.dcp");
        std::fs::write(&other, dcp("Sony ILCE-7M4", "Other"))?;
        let cameras = Cameras::default();
        let m = camera("ILCE-7M2");
        assert!(names(&installed_in(&dirs, &m, &cameras).0).is_empty());
        // Rewritten for this camera, same size, its modification time put back.
        let modified = std::fs::metadata(&other)?.modified()?;
        std::fs::write(&other, dcp("Sony ILCE-7M2", "Other"))?;
        std::fs::File::options()
            .write(true)
            .open(&other)?
            .set_modified(modified)?;
        assert_eq!(names(&installed_in(&dirs, &m, &cameras).0), ["Other"]);
        // Made unreadable, it is reported as the first scan would, for any camera.
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o000))?;
        let unreadable = std::fs::read(&other).is_err();
        let (_, errors) = installed_in(&dirs, &camera("ILCE-7M4"), &cameras);
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o600))?;
        // Root reads it anyway.
        if unreadable {
            assert_eq!(errors.len(), 1, "{errors:?}");
        }
        Ok(())
    }
    #[test]
    #[ignore = "Needs a published DCP; set RAWMAKASE_TEST_DCP"]
    fn imports_are_explicit_persistent_and_do_not_overwrite_different_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("selected.dcp");
        let bytes = &std::fs::read(std::env::var_os("RAWMAKASE_TEST_DCP").unwrap())?[..];
        std::fs::write(&source, bytes)?;
        let dest = temp.path().join("rawmakase-library");
        let selected = vec![source.clone()];
        let paths = |done: Vec<ImportedProfile>| -> Vec<PathBuf> {
            done.into_iter().map(|d| d.path).collect()
        };
        let result = paths(import_into(&selected, &dest, LutSpace::Srgb)?);
        assert_eq!(std::fs::read(&result[0])?, bytes);
        assert_eq!(std::fs::read(&source)?, bytes);
        assert_eq!(
            paths(import_into(&selected, &dest, LutSpace::Srgb)?),
            result
        );
        std::fs::write(&result[0], b"existing user data")?;
        assert!(import_into(&selected, &dest, LutSpace::Srgb).is_err());
        assert_eq!(std::fs::read(&result[0])?, b"existing user data");
        let unsupported = temp.path().join("preset.xmp");
        std::fs::write(&unsupported, "<preset/>")?;
        let new_dest = temp.path().join("fresh-library");
        assert!(import_into(&[source, unsupported], &new_dest, LutSpace::Srgb).is_err());
        assert!(!new_dest.exists());
        Ok(())
    }
    /// Two DCPs that share a name and camera but render differently are both
    /// listed and titled after their files; one that repeats another exactly is
    /// not. Lookups by name still find the first file.
    #[test]
    fn profiles_sharing_a_name_and_camera_are_listed_by_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        let exposure = |stops: i32| (51109, 10, [stops, 1].map(i32::to_le_bytes).concat());
        let mut picks = Vec::new();
        for (file, stops) in [("A.dcp", 0), ("B.dcp", 0), ("Z.dcp", 1)] {
            let path = temp.path().join(file);
            std::fs::write(
                &path,
                dcp_with("Sony ILCE-7M2", "Camera Neutral", [exposure(stops)]),
            )?;
            picks.push(path);
        }
        std::fs::write(temp.path().join("other.dcp"), dcp("Sony ILCE-7M2", "Other"))?;
        picks.push(temp.path().join("other.dcp"));
        import_into(&picks, &dest, LutSpace::Srgb)?;
        let listed = listed_in(&[dest], &camera("ILCE-7M2"), &Cameras::default());
        assert_eq!(
            names(&listed.profiles),
            ["Camera Neutral", "Camera Neutral", "Other"]
        );
        let titles: Vec<_> = listed
            .profiles
            .iter()
            .filter(|p| p.camera.starts_with("Sony"))
            .map(|p| listed.groups.entry(p).title)
            .collect();
        assert_eq!(
            titles,
            ["Camera Neutral (A.dcp)", "Camera Neutral (Z.dcp)", "Other"]
        );
        // An XMP's profile name resolves to the first file, as before.
        let first = listed
            .profiles
            .iter()
            .find(|p| p.name == "Camera Neutral")
            .unwrap();
        assert_eq!(first.exposure, 0.);
        // A recipe's own copy of Z's profile is titled as Z's file.
        let z = listed.profiles.iter().find(|p| p.exposure == 1.).unwrap();
        let saved = Arc::new((**z).clone());
        assert_eq!(listed.groups.entry(&saved).title, "Camera Neutral (Z.dcp)");
        // Files at the same place in two library folders are titled by full path.
        let (one, two) = (temp.path().join("one"), temp.path().join("two"));
        for (dir, stops) in [(&one, 0), (&two, 1)] {
            std::fs::create_dir_all(dir)?;
            std::fs::write(
                dir.join("Neutral.dcp"),
                dcp_with("Sony ILCE-7M2", "Camera Neutral", [exposure(stops)]),
            )?;
        }
        let listed = listed_in(
            &[one.clone(), two.clone()],
            &camera("ILCE-7M2"),
            &Cameras::default(),
        );
        let titles: Vec<_> = listed
            .profiles
            .iter()
            .filter(|p| p.camera.starts_with("Sony"))
            .map(|p| listed.groups.entry(p).title)
            .collect();
        let full = |dir: &Path| format!("Camera Neutral ({})", dir.join("Neutral.dcp").display());
        assert_eq!(titles, [full(&one), full(&two)]);
        Ok(())
    }
    #[test]
    fn importing_a_cameras_adobe_profiles_skips_the_unreadable_ones() -> Result<()> {
        // Camera Raw files HDR Camera Matching profiles with the others for newer
        // bodies (Nikon's Z 8, say); one of them must not stop the rest.
        let temp = tempfile::tempdir()?;
        let standard = temp.path().join("NIKON Z 8 Adobe Standard.dcp");
        std::fs::write(&standard, dcp("NIKON Z 8", "Adobe Standard"))?;
        let hdr = temp.path().join("Nikon Z 8 Camera Flat HDR.dcp");
        let flag = (52551, 7, vec![1, 0, 0, 0, 0, 0, 0, 0]);
        std::fs::write(&hdr, dcp_with("NIKON Z 8", "Camera Flat HDR", [flag]))?;
        let dest = temp.path().join("library");
        let done = import_readable_into(&[standard, hdr.clone()], &dest)?;
        assert_eq!(done.imported, [dest.join("NIKON Z 8 Adobe Standard.dcp")]);
        assert_eq!(done.skipped.len(), 1, "{:?}", done.skipped);
        assert_eq!(done.skipped[0].0, hdr);
        assert!(done.skipped[0].1.contains("HDR"), "{}", done.skipped[0].1);
        assert!(!dest.join("Nikon Z 8 Camera Flat HDR.dcp").exists());
        Ok(())
    }
    #[test]
    fn a_rendered_photo_is_offered_only_the_embedded_profile() {
        let m = Metadata {
            source: crate::camera_data::Source::Rendered,
            make: "Apple".into(),
            model: "iPhone 15".into(),
            ..Default::default()
        };
        let (profiles, errors) = installed_in(&[], &m, &CAMERAS);
        assert_eq!(names_all(&profiles), ["Embedded"]);
        assert!(errors.is_empty());
    }
    fn names_all(profiles: &[Arc<CameraProfile>]) -> Vec<&str> {
        profiles.iter().map(|p| p.name.as_str()).collect()
    }

    /// A camera RAWmakase Standard is made for, so creative looks have a base.
    fn with_matrix() -> Metadata {
        Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        }
    }
    fn write_cube(dir: &Path, name: &str, f: impl Fn([f32; 3]) -> [f32; 3]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, super::super::cube_look::tests::cube_text(9, f)).unwrap();
        path
    }
    fn warm([r, g, b]: [f32; 3]) -> [f32; 3] {
        [0.05 + 0.9 * r, 0.02 + 0.93 * g, 0.85 * b]
    }
    fn cool([r, g, b]: [f32; 3]) -> [f32; 3] {
        [0.85 * r, 0.02 + 0.93 * g, 0.05 + 0.9 * b]
    }
    fn cube_of(done: &ImportedProfile) -> &CubeImport {
        done.cube.as_ref().expect("a cube's report")
    }

    /// A cube becomes a look XMP in the library, named after its file and grouped by
    /// its folder, which the profile list then offers over the photo's base profile.
    #[test]
    fn a_cube_imports_as_a_look_the_library_lists() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cube = write_cube(&temp.path().join("Film"), "Portra.cube", warm);
        let dest = temp.path().join("library");
        let done = import_into(std::slice::from_ref(&cube), &dest, LutSpace::Srgb)?;
        assert_eq!(done.len(), 1);
        let report = cube_of(&done[0]);
        assert_eq!((report.name.as_str(), report.already), ("Portra", false));
        assert_eq!(report.fidelity, Fidelity::Exact);
        assert_eq!(done[0].path, dest.join("Portra.xmp"));
        let xml = std::fs::read_to_string(&done[0].path)?;
        assert!(xml.contains(">Film</rdf:li>") && xml.contains(">Test cube</rdf:li>"));
        // The cube itself is not copied: the look is what the library holds.
        assert_eq!(std::fs::read_dir(&dest)?.count(), 1);
        let (profiles, errors) = installed_in(&[dest], &with_matrix(), &Cameras::default());
        assert!(errors.is_empty(), "{errors:?}");
        let look = profiles
            .iter()
            .find(|p| p.name == "Portra")
            .expect("listed");
        let enhanced = look.enhanced.as_ref().unwrap();
        assert_eq!(enhanced.base_name, super::super::open::STANDARD);
        assert!(enhanced.rgb.is_some() && enhanced.amount.is_some());
        Ok(())
    }

    /// A cube imported for Adobe RGB is a look the library lists in its group, whose
    /// table is in Adobe RGB and gamma 2.2. Again in Adobe RGB it is imported already;
    /// in another space it is a look of its own beside the first, named by the usual
    /// rule, and each line says the space. DCPs and look XMPs in the batch are copied
    /// as they are.
    #[test]
    fn a_cube_imports_in_the_space_chosen() -> Result<()> {
        use super::super::{Gamma, Primaries};
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        let cube = write_cube(&temp.path().join("Film"), "Portra.cube", warm);
        let import = |space| import_into(std::slice::from_ref(&cube), &dest, space);
        let done = import(LutSpace::AdobeRgb)?;
        assert_eq!(
            cube_of(&done[0]).to_string(),
            "imported as look \"Portra\" in Adobe RGB (1998): exact"
        );
        let first = std::fs::read(dest.join("Portra.xmp"))?;
        let listed = || {
            listed_in(
                std::slice::from_ref(&dest),
                &with_matrix(),
                &Cameras::default(),
            )
        };
        let table = |listed: &Listed, name: &str| {
            let look = listed.profiles.iter().find(|p| p.name == name).expect(name);
            let table = look
                .enhanced
                .as_ref()
                .and_then(|e| e.rgb())
                .unwrap()
                .table();
            (
                listed.groups.entry(look).group,
                table.primaries(),
                table.gamma(),
            )
        };
        let once = listed();
        assert!(once.errors.is_empty(), "{:?}", once.errors);
        assert_eq!(
            table(&once, "Portra"),
            (Some("Film".into()), Primaries::AdobeRgb, Gamma::Power22)
        );
        let done = import(LutSpace::AdobeRgb)?;
        assert_eq!(
            cube_of(&done[0]).to_string(),
            "already imported as look \"Portra\" in Adobe RGB (1998)"
        );
        // At the sRGB default afterwards, and in ProPhoto: new looks, renamed, the first
        // kept as it was.
        let done = import(LutSpace::Srgb)?;
        assert_eq!(
            cube_of(&done[0]).to_string(),
            "imported as look \"Portra (Film)\", as another profile is named \"Portra\": exact"
        );
        let done = import(LutSpace::ProPhotoRgb)?;
        assert_eq!(
            cube_of(&done[0]).to_string(),
            "imported as look \"Portra (Film 2)\" in ProPhoto RGB, as another profile is \
             named \"Portra\": exact"
        );
        assert_eq!(std::fs::read(dest.join("Portra.xmp"))?, first);
        let all = listed();
        assert!(all.errors.is_empty(), "{:?}", all.errors);
        for (name, primaries, gamma) in [
            ("Portra", Primaries::AdobeRgb, Gamma::Power22),
            ("Portra (Film)", Primaries::Srgb, Gamma::Srgb),
            ("Portra (Film 2)", Primaries::ProPhoto, Gamma::Power18),
        ] {
            assert_eq!(
                table(&all, name),
                (Some("Film".into()), primaries, gamma),
                "{name}"
            );
        }
        // A batch of a DCP, a look XMP and a cube: the space is the cube's alone.
        let src = temp.path().join("mixed");
        std::fs::create_dir_all(&src)?;
        let dcp_path = src.join("mine.dcp");
        std::fs::write(&dcp_path, dcp("Test Camera", "Camera Neutral"))?;
        let converted = super::super::cube_look::convert(&std::fs::read(&cube)?, LutSpace::Srgb)?;
        let xmp_path = src.join("Look.xmp");
        std::fs::write(
            &xmp_path,
            super::super::cube_look::look_xmp(&converted, "Look", ""),
        )?;
        let other = temp.path().join("other");
        let done = import_into(
            &[dcp_path.clone(), xmp_path.clone(), cube.clone()],
            &other,
            LutSpace::ProPhotoRgb,
        )?;
        assert!(cube_of(&done[2]).to_string().contains("in ProPhoto RGB"));
        for (copy, source) in [(&done[0], &dcp_path), (&done[1], &xmp_path)] {
            assert!(copy.cube.is_none());
            assert_eq!(std::fs::read(&copy.path)?, std::fs::read(source)?);
        }
        Ok(())
    }

    /// Importing the same cube again changes nothing and says so, whatever folder it
    /// comes from: its table, not its bytes or its group, is what identifies it.
    #[test]
    fn importing_a_cube_again_is_idempotent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        let cube = write_cube(&temp.path().join("Film"), "Portra.cube", warm);
        import_into(std::slice::from_ref(&cube), &dest, LutSpace::Srgb)?;
        let before = std::fs::read(dest.join("Portra.xmp"))?;
        let moved = write_cube(&temp.path().join("Elsewhere"), "Portra.cube", warm);
        // Rewritten with other whitespace and a comment: other bytes, the same table.
        let text = std::fs::read_to_string(&moved)?;
        std::fs::write(
            &moved,
            format!("# exported again\n{}", text.replace(' ', "  ")),
        )?;
        for path in [cube, moved] {
            let done = import_into(std::slice::from_ref(&path), &dest, LutSpace::Srgb)?;
            let report = cube_of(&done[0]);
            assert!(report.already, "{report}");
            assert_eq!(done[0].path, dest.join("Portra.xmp"));
        }
        assert_eq!(std::fs::read(dest.join("Portra.xmp"))?, before);
        assert_eq!(std::fs::read_dir(&dest)?.count(), 1);
        Ok(())
    }

    /// A different cube under a name the library has is renamed after its folder,
    /// never shadowing or shadowed: another cube's look, a look XMP (an Adobe name
    /// included), a DCP or RAWmakase's own profiles. A renamed one imports again as
    /// already imported.
    #[test]
    fn importing_or_removing_any_profile_changes_the_fingerprint() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        std::fs::create_dir_all(&dest)?;
        let dirs = [dest.clone()];
        let empty = fingerprint_of(&dirs);
        assert_eq!(fingerprint_of(&dirs), empty, "stable");
        std::fs::write(
            dest.join("mine.dcp"),
            dcp("Test Camera", "Whatever I Called It"),
        )?;
        let one = fingerprint_of(&dirs);
        assert_ne!(one, empty);
        std::fs::remove_file(dest.join("mine.dcp"))?;
        assert_eq!(fingerprint_of(&dirs), empty);
        Ok(())
    }

    #[test]
    fn a_name_collision_is_renamed_and_reported() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        std::fs::create_dir_all(&dest)?;
        let warm_cube = write_cube(&temp.path().join("Warm"), "Portra.cube", warm);
        import_into(&[warm_cube], &dest, LutSpace::Srgb)?;
        // An Adobe look's name, held by a look XMP whose file is named otherwise.
        let other = write_cube(&temp.path().join("x"), "Other.cube", warm);
        let converted = super::super::cube_look::convert(&std::fs::read(&other)?, LutSpace::Srgb)?;
        std::fs::write(
            dest.join("adobe-vivid.xmp"),
            super::super::cube_look::look_xmp(&converted, "Adobe Vivid", "Adobe"),
        )?;
        std::fs::write(dest.join("mine.dcp"), dcp("Test Camera", "Camera Neutral"))?;
        let cool_dir = temp.path().join("Cool");
        let picks = [
            write_cube(&cool_dir, "Portra.cube", cool),
            write_cube(&cool_dir, "Adobe Vivid.cube", cool),
            write_cube(&cool_dir, "Camera Neutral.cube", cool),
            write_cube(&cool_dir, "RAWmakase Standard.cube", cool),
            write_cube(&cool_dir, "Camera Look (fitted).cube", cool),
            // Not imported, but the name Lightroom edits fall back by.
            write_cube(&cool_dir, "Adobe Standard.cube", cool),
        ];
        let done = import_into(&picks, &dest, LutSpace::Srgb)?;
        let names: Vec<_> = done.iter().map(|d| cube_of(d).name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Portra (Cool)",
                "Adobe Vivid (Cool)",
                "Camera Neutral (Cool)",
                "RAWmakase Standard (Cool)",
                "Camera Look (fitted) (Cool)",
                "Adobe Standard (Cool)"
            ]
        );
        assert!(
            done.iter()
                .all(|d| cube_of(d).renamed() && !cube_of(d).already)
        );
        assert!(
            cube_of(&done[0])
                .to_string()
                .contains("as another profile is named \"Portra\"")
        );
        // A third Portra goes further; the renamed one imports again as itself.
        let third = write_cube(&temp.path().join("b/Cool"), "Portra.cube", |p| p);
        let done = import_into(&[third], &dest, LutSpace::Srgb)?;
        assert_eq!(cube_of(&done[0]).name, "Portra (Cool 2)");
        let done = import_into(&picks[..1], &dest, LutSpace::Srgb)?;
        assert!(cube_of(&done[0]).already);
        assert_eq!(cube_of(&done[0]).name, "Portra (Cool)");
        // The same cube from another folder is found under its new name.
        let copy = write_cube(&temp.path().join("Cool copy"), "Portra.cube", cool);
        let done = import_into(&[copy], &dest, LutSpace::Srgb)?;
        assert!(cube_of(&done[0]).already, "{}", cube_of(&done[0]));
        assert_eq!(cube_of(&done[0]).name, "Portra (Cool)");
        // Every look lists under its own name.
        let (profiles, _) = installed_in(&[dest], &with_matrix(), &Cameras::default());
        let listed = names_all(&profiles);
        for name in [
            "Portra",
            "Portra (Cool)",
            "Portra (Cool 2)",
            "Adobe Vivid",
            "Adobe Vivid (Cool)",
        ] {
            assert_eq!(
                listed.iter().filter(|n| **n == name).count(),
                1,
                "{name}: {listed:?}"
            );
        }
        Ok(())
    }

    /// The listing says where the menu shows every profile: a look in its group's
    /// submenu; camera profiles, RAWmakase's, a look without a group and Adobe's
    /// "Profiles" looks at the top level. The group stays out of the profile, so the
    /// recipe that embeds it is what earlier releases read.
    #[test]
    fn the_listing_groups_looks_and_keeps_profiles_at_the_top() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        std::fs::create_dir_all(&dest)?;
        std::fs::write(dest.join("mine.dcp"), dcp("Test Camera", "Camera Neutral"))?;
        let pack = temp.path().join("Pack Moody/For Other Programs/Cube Files");
        import_into(
            &[write_cube(&pack, "Portra.cube", warm)],
            &dest,
            LutSpace::Srgb,
        )?;
        let look = |name: &str, group: &str, f: fn([f32; 3]) -> [f32; 3]| -> Result<()> {
            let cube = write_cube(&temp.path().join("src"), &format!("{name}.cube"), f);
            let converted =
                super::super::cube_look::convert(&std::fs::read(&cube)?, LutSpace::Srgb)?;
            std::fs::write(
                dest.join(format!("{name}.xmp")),
                super::super::cube_look::look_xmp(&converted, name, group),
            )?;
            Ok(())
        };
        look("Adobe Vivid", "Profiles", cool)?;
        look("Loose", "", |p| p)?;
        look("Faded", "B&W", |[r, g, b]| [r * 0.9, g * 0.9, b * 0.9])?;
        let listed = listed_in(
            std::slice::from_ref(&dest),
            &with_matrix(),
            &Cameras::default(),
        );
        assert!(listed.errors.is_empty(), "{:?}", listed.errors);
        let entry = |name: &str| {
            let p = listed.profiles.iter().find(|p| p.name == name).expect(name);
            listed.groups.entry(p)
        };
        assert_eq!(
            entry("Portra"),
            MenuEntry {
                group: Some("Pack Moody".into()),
                section: None,
                title: "Portra".into()
            }
        );
        assert_eq!(entry("Faded").group.as_deref(), Some("B&W"));
        for name in [
            "Camera Neutral",
            super::super::open::STANDARD,
            "Adobe Vivid",
            "Loose",
        ] {
            assert_eq!(entry(name).group, None, "{name}");
            assert_eq!(entry(name).title, name);
        }
        // A profile the listing never saw (an edit's embedded copy, its file gone)
        // has no group, and neither does a look with the name but another table.
        let portra = listed.profiles.iter().find(|p| p.name == "Portra").unwrap();
        let mut other = (**portra).clone();
        other.enhanced.as_mut().unwrap().uuid = "0".repeat(32);
        assert_eq!(listed.groups.entry(&other).group, None);
        assert_eq!(Groups::default().entry(portra).group, None);
        let saved = serde_json::to_string(&**portra)?;
        assert!(!saved.contains("Pack Moody"), "{saved}");
        Ok(())
    }

    /// Two packs that both keep their cubes in "Cube Files" rename a colliding cube
    /// after the pack, as it lists under that group too.
    #[test]
    fn a_collision_is_renamed_after_the_meaningful_group() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        let pack = |name: &str| temp.path().join(name).join("Cube Files");
        let first = write_cube(&pack("Pack A"), "Portra.cube", warm);
        let second = write_cube(&pack("Pack B"), "Portra.cube", cool);
        let done = import_into(&[first, second.clone()], &dest, LutSpace::Srgb)?;
        let names: Vec<_> = done.iter().map(|d| cube_of(d).name.as_str()).collect();
        assert_eq!(names, ["Portra", "Portra (Pack B)"]);
        // Imported again from elsewhere, it is found by its table, not its group.
        let moved = write_cube(&temp.path().join("Moved/LUTs"), "Portra.cube", cool);
        for path in [second, moved] {
            let done = import_into(&[path], &dest, LutSpace::Srgb)?;
            assert!(cube_of(&done[0]).already);
            assert_eq!(cube_of(&done[0]).name, "Portra (Pack B)");
        }
        let listed = listed_in(&[dest], &with_matrix(), &Cameras::default());
        let groups: Vec<_> = listed
            .profiles
            .iter()
            .filter(|p| p.name.starts_with("Portra"))
            .map(|p| listed.groups.entry(p).group.unwrap_or_default())
            .collect();
        assert_eq!(groups, ["Pack A", "Pack B"]);
        Ok(())
    }

    /// Something under the look's file name the library scan doesn't list (a
    /// folder, say) is an error, not a look reported as imported but never written.
    #[test]
    fn a_cubes_look_never_claims_a_file_it_did_not_write() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dest = temp.path().join("library");
        std::fs::create_dir_all(dest.join("Portra.xmp"))?;
        let cube = write_cube(&temp.path().join("Film"), "Portra.cube", warm);
        assert!(import_into(&[cube], &dest, LutSpace::Srgb).is_err());
        Ok(())
    }

    /// A cube the table cannot hold refuses the batch, as any unreadable profile does,
    /// before anything is written.
    #[test]
    fn a_refused_cube_fails_the_batch_before_writing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let good = write_cube(&temp.path().join("Pack"), "Good.cube", warm);
        let bad = temp.path().join("Pack/Log.cube");
        std::fs::write(
            &bad,
            super::super::cube_look::tests::cube_text(2, |p| p).replace(
                "LUT_3D_SIZE 2",
                "LUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 4 4 4",
            ),
        )?;
        let dest = temp.path().join("library");
        let error = import_into(&[good, bad], &dest, LutSpace::Srgb).unwrap_err();
        assert!(format!("{error:#}").starts_with("Log.cube: "), "{error:#}");
        assert!(!dest.join("Good.xmp").exists());
        assert!(read_cube(&std::fs::read(temp.path().join("Pack/Good.cube"))?).is_ok());
        Ok(())
    }
}
