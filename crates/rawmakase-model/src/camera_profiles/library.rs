use super::{
    CameraProfile,
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

pub fn installed(m: &Metadata) -> (Vec<Arc<CameraProfile>>, Vec<String>) {
    installed_in(&library_dirs(), m, &CAMERAS)
}
fn installed_in(
    dirs: &[PathBuf],
    m: &Metadata,
    cameras: &Cameras,
) -> (Vec<Arc<CameraProfile>>, Vec<String>) {
    if m.is_rendered() {
        return (vec![Arc::new(CameraProfile::embedded(m))], vec![]);
    }
    let mut profiles = Vec::new();
    let mut errors = Vec::new();
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
                if !profiles
                    .iter()
                    .any(|old| old.name == p.name && old.camera == p.camera)
                {
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
            look.compose(base)
        });
        match composed {
            Ok(p) => {
                if !profiles
                    .iter()
                    .any(|old| old.name == p.name && old.camera == p.camera)
                {
                    profiles.push(Arc::new(p));
                }
            }
            Err(e) => errors.push(format!("{}: {e:#}", path.display())),
        }
    }
    errors.sort();
    errors.dedup();
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    (profiles, errors)
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

/// Explicitly import files into RAWmakase's own library. Validate the complete batch
/// before writing; never follow or discover Adobe application directories.
pub fn import_files(paths: &[std::path::PathBuf]) -> Result<Vec<std::path::PathBuf>> {
    import_into(paths, &crate::storage::data_dir().join("camera-profiles"))
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
/// Raw's HDR profiles, are skipped instead of failing the rest.
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
        import_into(&readable, destination)?
    };
    Ok(Imported { imported, skipped })
}
fn import_into(
    paths: &[std::path::PathBuf],
    destination: &Path,
) -> Result<Vec<std::path::PathBuf>> {
    use anyhow::Context;
    use std::io::Write;
    ensure!(
        !paths.is_empty() && paths.len() <= 1024,
        "Choose 1–1024 profile files"
    );
    let mut files = Vec::new();
    let mut bases = Vec::new();
    let mut existing = Vec::new();
    collect(destination, 0, &mut existing);
    for p in existing {
        if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
            && let Ok(bytes) = std::fs::read(&p)
            && let Ok(profile) = from_bytes(&bytes)
        {
            bases.push(profile);
        }
    }
    for path in paths {
        ensure!(
            std::fs::metadata(path)?.len() <= 16_000_000,
            "Profile too large: {}",
            path.display()
        );
        let ext = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        ensure!(
            matches!(ext.as_str(), "dcp" | "xmp"),
            "Choose DCP or XMP profiles"
        );
        let bytes = std::fs::read(path)?;
        if ext == "dcp" {
            bases.push(from_bytes(&bytes).with_context(|| format!("{}", path.display()))?);
        }
        let target = destination.join(path.file_name().context("Missing profile filename")?);
        if target.exists() {
            ensure!(
                std::fs::read(&target)? == bytes,
                "A different profile named {} is already imported",
                target.file_name().unwrap().to_string_lossy()
            );
        }
        if let Some((_, previous, _)) = files.iter().find(|(p, _, _)| *p == target) {
            ensure!(
                *previous == bytes,
                "Conflicting profile filenames in import"
            );
        } else {
            files.push((target, bytes, ext));
        }
    }
    for (path, bytes, ext) in &files {
        if ext == "xmp" {
            let file = path.file_name().unwrap().to_string_lossy();
            let look =
                LookFile::parse(std::str::from_utf8(bytes)?).with_context(|| format!("{file}"))?;
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
    std::fs::create_dir_all(destination)?;
    let mut imported = Vec::new();
    for (target, bytes, _) in files {
        if !target.exists() {
            crate::storage::write_atomic(&target, crate::storage::Replace::NoClobber, |f| {
                Ok(f.write_all(&bytes)?)
            })?;
        }
        imported.push(target);
    }
    Ok(imported)
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
            let count = if *kind == 10 { 9 } else { bytes.len() };
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
        let result = import_into(&selected, &dest)?;
        assert_eq!(std::fs::read(&result[0])?, bytes);
        assert_eq!(std::fs::read(&source)?, bytes);
        assert_eq!(import_into(&selected, &dest)?, result);
        std::fs::write(&result[0], b"existing user data")?;
        assert!(import_into(&selected, &dest).is_err());
        assert_eq!(std::fs::read(&result[0])?, b"existing user data");
        let unsupported = temp.path().join("preset.xmp");
        std::fs::write(&unsupported, "<preset/>")?;
        let new_dest = temp.path().join("fresh-library");
        assert!(import_into(&[source, unsupported], &new_dest).is_err());
        assert!(!new_dest.exists());
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
}
