use super::{
    CameraProfile,
    enhanced::{LookBase, LookFile},
    from_bytes,
};
use crate::camera_data::Metadata;
use anyhow::{Context, Result, ensure};
use std::{path::Path, sync::Arc};
pub fn load(path: &Path, m: &Metadata) -> Result<Arc<CameraProfile>> {
    ensure!(
        std::fs::metadata(path)?.len() <= 16_000_000,
        "Profile too large"
    );
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
    {
        let look = LookFile::read(path)?;
        let (profiles, _) = installed(m);
        let base = look_base(&look, &profiles, builtin(m).as_deref())
            .context("Unsupported XMP look or missing matching base camera profile")?;
        return Ok(Arc::new(look.compose(base)?));
    }
    let p = from_bytes(&std::fs::read(path)?)?;
    p.ensure_camera(m)?;
    Ok(Arc::new(p))
}
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
    let mut profiles = Vec::new();
    let mut errors = Vec::new();
    if let Some(p) = builtin(m) {
        profiles.push(p);
    }
    profiles.extend(
        [super::open::standard(m), super::open::color(m)]
            .into_iter()
            .flatten()
            .chain(super::film::profiles(m))
            .map(Arc::new),
    );
    let mut files = Vec::new();
    for dir in library_dirs() {
        collect(&dir, 0, &mut files);
    }
    let mut looks = Vec::new();
    for path in files {
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
        match load(&path, m) {
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
    let bases = profiles.clone();
    for path in looks {
        let composed = LookFile::read(&path).and_then(|look| {
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
    let camera = format!("{} {}", m.make, m.model);
    let mut paths = vec![
        root.join("Adobe Standard")
            .join(format!("{camera} Adobe Standard.dcp")),
    ];
    if let Ok(entries) = std::fs::read_dir(root.join("Camera").join(&camera)) {
        paths.extend(entries.flatten().map(|e| e.path()));
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
}
