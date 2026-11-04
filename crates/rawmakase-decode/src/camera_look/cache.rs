//! The fit cache: what fitting a photo's default look gave (the profile, or why
//! none), so a photo without colour data is fitted once per file and build rather
//! than on every open, preview and export. Beside the decode cache, in
//! `camera-look/`; an entry is small (a carried profile, about 100 KB) and only
//! such photos have one.
use super::FIT_VERSION;
use rawmakase_model::{
    camera_data::Metadata,
    camera_profiles::{CameraProfile, fitted},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a fit's result is filed under, captured once before the fit: the file's
/// identity (size, modification time, a hash of its first 64 KB), the fit's
/// version and the build. A changed or replaced file, a new fit or another build
/// finds nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FitKey(String);
impl FitKey {
    pub fn of(path: &Path) -> anyhow::Result<Self> {
        let id = crate::storage::Identity::read(path)?;
        Ok(Self(crate::decode_cache::source_key(
            &id,
            format_args!("v{FIT_VERSION}"),
        )))
    }
    fn file(&self, dir: &Path) -> PathBuf {
        use sha2::Digest;
        let name: String = sha2::Sha256::digest(self.0.as_bytes())
            .iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect();
        dir.join(format!("{name}.json"))
    }
}

#[derive(Serialize, Deserialize)]
struct Entry {
    key: String,
    /// The carried profile (`fitted::carried`), or why there is none.
    profile: Option<String>,
    refusal: Option<String>,
}

/// The cache's folder.
pub fn dir() -> PathBuf {
    crate::decode_cache::cache_dir().join("camera-look")
}

/// The result stored under `key` in `dir`, checked as a carried profile is: none
/// when there is no entry or it no longer reads.
pub(super) fn load(
    dir: &Path,
    key: &FitKey,
    m: &Metadata,
) -> Option<Result<CameraProfile, String>> {
    let bytes = std::fs::read(key.file(dir)).ok()?;
    let entry: Entry = serde_json::from_slice(&bytes).ok()?;
    if entry.key != key.0 {
        return None;
    }
    match (entry.profile, entry.refusal) {
        (Some(text), _) => fitted::from_carried(&text, m).ok().map(Ok),
        (None, Some(why)) => Some(Err(why)),
        (None, None) => None,
    }
}

/// Stores `result` under `key` in `dir`, whole or not at all (written aside,
/// then renamed).
pub(super) fn store(
    dir: &Path,
    key: &FitKey,
    result: &Result<CameraProfile, String>,
) -> anyhow::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let entry = Entry {
        key: key.0.clone(),
        profile: result.as_ref().ok().map(fitted::carried),
        refusal: result.as_ref().err().cloned(),
    };
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(&serde_json::to_vec(&entry)?)?;
    file.persist(key.file(dir))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fit is filed under the file's identity, the fit's version and the build, in
    /// the form the fits already on disk have.
    #[test]
    fn a_fit_key_keeps_its_form() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("a.raw");
        std::fs::write(&photo, b"one").unwrap();
        let id = crate::storage::Identity::read(&photo).unwrap();
        assert_eq!(
            FitKey::of(&photo).unwrap().0,
            format!(
                "{}-{}-{}-v{FIT_VERSION}-{}",
                id.prefix_hash,
                id.size,
                id.modified_ns,
                crate::decode_cache::build_identity()
            )
        );
    }

    #[test]
    fn a_stored_fit_is_found_only_under_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("a.raw");
        std::fs::write(&photo, b"one").unwrap();
        let m = Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        };
        let p = fitted::with_look(
            &fitted::standard_base(&m).unwrap(),
            &fitted::FittedLook::identity(),
        )
        .unwrap();
        let cache = dir.path().join("cache");
        let key = FitKey::of(&photo).unwrap();
        assert!(load(&cache, &key, &m).is_none());
        store(&cache, &key, &Ok(p.clone())).unwrap();
        assert_eq!(load(&cache, &key, &m), Some(Ok(p)));
        // A refusal is kept too, so it isn't fitted again.
        let other = dir.path().join("b.raw");
        std::fs::write(&other, b"two").unwrap();
        let refused = FitKey::of(&other).unwrap();
        store(&cache, &refused, &Err("No embedded preview".into())).unwrap();
        assert_eq!(
            load(&cache, &refused, &m),
            Some(Err("No embedded preview".into()))
        );
        // The file replaced: another key, nothing found.
        std::fs::write(&photo, b"changed contents").unwrap();
        assert!(load(&cache, &FitKey::of(&photo).unwrap(), &m).is_none());
        // A cache that can't be written to leaves the result unstored.
        let blocked = dir.path().join("file");
        std::fs::write(&blocked, b"").unwrap();
        assert!(store(&blocked, &key, &Err("x".into())).is_err());
    }
}
