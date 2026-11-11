//! The installed extensions: `<data>/extensions/<id>/<version>/`, one folder
//! per installed version, and their entries in `extensions.json`.
//!
//! An install unpacks and verifies the package in a staging folder
//! (`extensions/.staging-<random>`), then, under the state's lock, renames it
//! to its version's folder and makes that version active in the same change.
//! An install that fails or is interrupted at any point therefore leaves the
//! previous version installed and active; a staging folder it left behind is
//! removed by a later [`Store::clean`]. A version folder is only ever renamed
//! into place complete and never changed afterwards; installing its version
//! again replaces it, the same way, when it no longer holds exactly its
//! package's files ([`package::is_unpacked_in`]).
//!
//! A removal, under the same lock, removes the entry and renames `<id>/`
//! aside to a staging name in one change, and deletes it after the lock is
//! released; an install meanwhile publishes into a new `<id>/`.
use crate::package;
use crate::state::{Source, State};
use crate::{Error, Manifest, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The folder under the data folder holding installed extensions.
pub const DIR: &str = "extensions";
const STAGING: &str = ".staging-";
const DOWNLOAD: &str = ".download-";
/// How old a staging folder or download must be before [`Store::clean`]
/// takes it for one a crashed install left.
const STALE: Duration = Duration::from_secs(60 * 60);
/// The prefix of official extensions' ids, which only the app's bundled
/// extensions use.
pub const OFFICIAL: &str = "rawmakase.";

/// An installed version.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    /// Its manifest.
    pub manifest: Manifest,
    /// Its folder.
    pub dir: PathBuf,
    /// The version that was active before, if another.
    pub replaced: Option<String>,
}

/// An extension as `extensions.json` and its folder record it.
#[derive(Clone, Debug)]
pub struct Listed {
    /// Its id.
    pub id: String,
    /// Its entry.
    pub entry: crate::state::Entry,
    /// The active version's manifest, or why it can't be read.
    pub manifest: Option<Result<Manifest>>,
}

/// The installed extensions of one data folder.
#[derive(Clone, Debug)]
pub struct Store {
    data_dir: PathBuf,
}

impl Store {
    /// The store in `data_dir`.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }
    /// The data folder.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
    /// `<data>/extensions`.
    pub fn root(&self) -> PathBuf {
        self.data_dir.join(DIR)
    }
    /// The folder of one installed version.
    pub fn version_dir(&self, id: &str, version: &str) -> PathBuf {
        self.root().join(id).join(version)
    }
    /// The extension's private data folder, kept across versions and
    /// uninstalls.
    pub fn extension_data_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("extension-data").join(id)
    }
    /// A new folder for a download, which [`Store::clean`] removes if it is
    /// left.
    pub(crate) fn download_dir(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(self.root())?;
        let dir = self
            .root()
            .join(format!("{DOWNLOAD}{}", crate::random_suffix()));
        std::fs::create_dir(&dir)?;
        Ok(dir)
    }

    /// Checks a package before it is installed: its names, its manifest, its
    /// target and the RAWmakase versions it supports.
    pub fn inspect(
        &self,
        package_file: &Path,
        app: &semver::Version,
    ) -> Result<package::Inspected> {
        let inspected = package::inspect(package_file)?;
        let manifest = &inspected.manifest;
        if manifest.extension.id.starts_with(OFFICIAL) {
            return Err(Error::new(format!(
                "{} is an official extension's id; official extensions come with RAWmakase",
                manifest.extension.id
            )));
        }
        crate::check_target(manifest, &inspected.target)?;
        crate::check_compatible(manifest, app)?;
        Ok(inspected)
    }

    /// Installs a package and makes its version the active one, keeping the
    /// extension's enabled state and settings. Installing a version already
    /// installed makes it active again without unpacking when its folder
    /// holds exactly this package's files; a folder that doesn't (one an
    /// interrupted removal left part of) is replaced.
    pub fn install(
        &self,
        package_file: &Path,
        source: &Source,
        app: &semver::Version,
    ) -> Result<Installed> {
        let inspected = self.inspect(package_file, app)?;
        let id = inspected.manifest.extension.id.clone();
        let version = inspected.manifest.extension.version.to_string();
        let dir = self.version_dir(&id, &version);
        let staging = if package::is_unpacked_in(package_file, &dir) {
            None
        } else {
            std::fs::create_dir_all(self.root())?;
            let staging = self
                .root()
                .join(format!("{STAGING}{}", crate::random_suffix()));
            std::fs::create_dir(&staging)?;
            match package::unpack(package_file, &staging) {
                Ok(unpacked) if unpacked == inspected.manifest => Some(staging),
                Ok(_) => {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(Error::new("The package changed while it was installed"));
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(e);
                }
            }
        };
        let published = State::update(&self.data_dir, |state| {
            match &staging {
                Some(staging) => self.publish(staging, &dir)?,
                // `clean` took the folder since it was checked.
                None if !dir.is_dir() => {
                    return Err(Error::new(format!(
                        "{id} {version} was removed while it was installed; install it again"
                    )));
                }
                None => {}
            }
            let entry = state.entry(&id);
            let replaced = entry
                .active
                .replace(version.clone())
                .filter(|v| *v != version);
            if !entry.installed.contains(&version) {
                entry.installed.push(version.clone());
            }
            entry.set_source(source);
            Ok(replaced)
        });
        match published {
            Ok(replaced) => Ok(Installed {
                manifest: inspected.manifest,
                dir,
                replaced,
            }),
            Err(e) => {
                if let Some(staging) = &staging {
                    let _ = std::fs::remove_dir_all(staging);
                }
                Err(e)
            }
        }
    }

    /// Removes an installed extension: its entry and its folder, then the
    /// folder's files. Its private data folder stays. A removal that can't
    /// move the folder (on Windows, while its program runs) is refused and
    /// changes nothing; files that can't be deleted after are left for
    /// [`Store::clean`].
    pub fn remove(&self, id: &str) -> Result<()> {
        if let Some(aside) = self.forget(id)? {
            let _ = std::fs::remove_dir_all(aside);
        }
        Ok(())
    }

    /// The locked step of [`Store::remove`]: removes the entry and moves
    /// `<id>/` aside, under a staging name [`Store::clean`] collects, in one
    /// change, so an install that runs before the files are deleted publishes
    /// into a new `<id>/` that nothing deletes. Returns where the folder went.
    fn forget(&self, id: &str) -> Result<Option<PathBuf>> {
        let dir = self.root().join(id);
        State::update_or_undo(
            &self.data_dir,
            |state| {
                match state.extensions.get(id) {
                    Some(entry) if !entry.installed.is_empty() => {}
                    _ => return Err(Error::new(format!("{id} is not installed"))),
                }
                let aside = self
                    .root()
                    .join(format!("{STAGING}{}", crate::random_suffix()));
                let moved = match std::fs::rename(&dir, &aside) {
                    Ok(()) => Some(aside),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => {
                        return Err(Error::from(e).context(format!(
                            "Could not remove {id}; stop what is using it and try again"
                        )));
                    }
                };
                state.extensions.remove(id);
                Ok(moved)
            },
            |moved| {
                if let Some(aside) = moved {
                    let _ = std::fs::rename(aside, &dir);
                }
            },
        )
    }

    /// Turns an extension on or off: an installed one, a developer folder, or
    /// one of `bundled` (the ids of the extensions that ship with the app),
    /// which gains an entry for it. Any other id is refused and nothing is
    /// written.
    pub fn set_enabled(&self, id: &str, enabled: bool, bundled: &[&str]) -> Result<()> {
        State::update(&self.data_dir, |state| {
            if let Some(developer) = state.developer.get_mut(id) {
                developer.enabled = enabled;
                return Ok(());
            }
            let installed = state
                .extensions
                .get(id)
                .is_some_and(|entry| !entry.installed.is_empty());
            if !installed && !bundled.contains(&id) {
                return Err(Error::new(format!("{id} is not installed")));
            }
            state.entry(id).enabled = enabled;
            Ok(())
        })
    }

    /// Registers an unpacked extension's folder in developer mode, after
    /// checking its manifest.
    pub fn add_developer(&self, folder: &Path) -> Result<Manifest> {
        let manifest = crate::read_manifest(folder)?;
        let path = std::fs::canonicalize(folder)?;
        let id = manifest.extension.id.clone();
        State::update(&self.data_dir, |state| {
            let developer = crate::state::Developer {
                path,
                enabled: true,
                unknown: Default::default(),
            };
            match state.developer.get_mut(&id) {
                Some(existing) => {
                    existing.path = developer.path;
                    existing.enabled = true;
                }
                None => {
                    state.developer.insert(id, developer);
                }
            }
            Ok(())
        })?;
        Ok(manifest)
    }

    /// Every extension `extensions.json` names, with its active version's
    /// manifest when it has one.
    pub fn list(&self) -> Result<Vec<Listed>> {
        let state = State::read(&self.data_dir)?;
        Ok(state
            .extensions
            .into_iter()
            .map(|(id, entry)| {
                let manifest = entry
                    .active
                    .as_ref()
                    .map(|v| crate::read_manifest(&self.version_dir(&id, v)));
                Listed {
                    id,
                    entry,
                    manifest,
                }
            })
            .collect())
    }

    /// Removes what no one uses any more: version folders that are neither
    /// active nor in `in_use` (versions a running job still holds), with
    /// their `installed` entries, and staging folders and downloads a
    /// crashed install left. Errors are skipped: this is housekeeping, run
    /// again at the next start.
    pub fn clean(&self, in_use: &[(String, String)]) {
        // Under the state's lock, as installs publish under it: a version an
        // install is renaming into place is never taken for an orphan.
        let _ = State::update(&self.data_dir, |state| {
            self.clean_locked(state, in_use);
            Ok(())
        });
    }

    fn clean_locked(&self, state: &mut State, in_use: &[(String, String)]) {
        let Ok(entries) = std::fs::read_dir(self.root()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if name.starts_with(STAGING) || name.starts_with(DOWNLOAD) {
                let old = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > STALE);
                if old {
                    let _ = std::fs::remove_dir_all(&path);
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            let Some(listed) = state.extensions.get_mut(&name) else {
                // Not installed (any more): an uninstall that couldn't remove it.
                if path.is_dir() && !in_use.iter().any(|(id, _)| *id == name) {
                    let _ = std::fs::remove_dir_all(&path);
                }
                continue;
            };
            let Ok(versions) = std::fs::read_dir(&path) else {
                continue;
            };
            for version in versions.flatten() {
                let v = version.file_name().to_string_lossy().into_owned();
                let used = listed.active.as_deref() == Some(&v)
                    || in_use.iter().any(|(id, iv)| *id == name && *iv == v);
                if !used && std::fs::remove_dir_all(version.path()).is_ok() {
                    listed.installed.retain(|i| *i != v);
                }
            }
        }
    }

    /// Renames a verified staging folder to its version's folder, under the
    /// state's lock. A folder already there (left part-removed, or published
    /// by another install of the same version meanwhile) is moved aside
    /// first and deleted after; if the staging folder can't take its place,
    /// it is moved back.
    fn publish(&self, staging: &Path, dir: &Path) -> Result<()> {
        let parent = dir.parent().expect("a version folder has a parent");
        let failed = |e: std::io::Error| {
            Error::from(e).context(format!("Could not install {}", dir.display()))
        };
        std::fs::create_dir_all(parent)?;
        let _ = rawmakase_fs::sync_dir(staging);
        // Named as a staging folder, so `clean` removes it if deleting it
        // fails below (a file a running program holds on Windows).
        let aside = self
            .root()
            .join(format!("{STAGING}{}", crate::random_suffix()));
        let moved = match std::fs::symlink_metadata(dir) {
            Ok(_) => {
                std::fs::rename(dir, &aside).map_err(failed)?;
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(failed(e)),
        };
        if let Err(e) = std::fs::rename(staging, dir) {
            if moved {
                let _ = std::fs::rename(&aside, dir);
            }
            return Err(failed(e));
        }
        let _ = rawmakase_fs::sync_dir(parent);
        if moved {
            let _ = std::fs::remove_dir_all(&aside);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::tests::{files, package, raw_zip};

    fn app() -> semver::Version {
        semver::Version::new(0, 4, 0)
    }
    fn source(path: &Path) -> Source {
        Source::File {
            path: path.to_path_buf(),
        }
    }

    #[test]
    fn installing_publishes_the_version_and_makes_it_active() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let first = package(data.path(), "1.0.0");
        let installed = store.install(&first, &source(&first), &app()).unwrap();
        assert_eq!(installed.replaced, None);
        assert!(
            installed
                .dir
                .join(crate::protocol::manifest::FILE)
                .is_file()
        );
        store.set_enabled("dev.test.tool", false, &[]).unwrap();
        let second = package(data.path(), "1.1.0");
        let installed = store.install(&second, &source(&second), &app()).unwrap();
        assert_eq!(installed.replaced.as_deref(), Some("1.0.0"));
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        let entry = &listed[0].entry;
        assert_eq!(entry.active.as_deref(), Some("1.1.0"));
        assert_eq!(entry.installed, ["1.0.0", "1.1.0"]);
        // An update keeps the user's choice.
        assert!(!entry.enabled);
        let manifest = listed[0].manifest.clone().unwrap().unwrap();
        assert_eq!(manifest.extension.version.to_string(), "1.1.0");
        // The old version stays while a job holds it, then goes.
        store.clean(&[("dev.test.tool".into(), "1.0.0".into())]);
        assert!(store.version_dir("dev.test.tool", "1.0.0").is_dir());
        store.clean(&[]);
        assert!(!store.version_dir("dev.test.tool", "1.0.0").exists());
        assert_eq!(store.list().unwrap()[0].entry.installed, ["1.1.0"]);
        // Removing deletes the folders and the entry.
        store.remove("dev.test.tool").unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(!store.root().join("dev.test.tool").exists());
        assert!(store.remove("dev.test.tool").is_err());
    }

    #[test]
    fn a_failed_install_keeps_the_previous_version() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let first = package(data.path(), "1.0.0");
        store.install(&first, &source(&first), &app()).unwrap();
        // 1.1.0 whose program doesn't match its digest: refused after part of
        // it was unpacked.
        let mut broken = files("1.1.0");
        let sums = package::write_sums(broken.iter().map(|(n, b)| (n.as_str(), b.as_slice())));
        broken[1].1 = b"tampered".to_vec();
        let path = data.path().join(format!(
            "dev.test.tool-1.1.0-{}.rmext",
            crate::current_target()
        ));
        let entries: Vec<(&str, &[u8])> = broken
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .chain([(package::SUMS, sums.as_bytes())])
            .collect();
        raw_zip(&path, &entries);
        assert!(store.install(&path, &source(&path), &app()).is_err());
        let entry = &store.list().unwrap()[0].entry;
        assert_eq!(entry.active.as_deref(), Some("1.0.0"));
        assert_eq!(entry.installed, ["1.0.0"]);
        assert!(!store.version_dir("dev.test.tool", "1.1.0").exists());
        let left: Vec<_> = std::fs::read_dir(store.root())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["dev.test.tool"]);
        // An install interrupted after unpacking (its staging folder left
        // behind) changes nothing either.
        std::fs::create_dir(store.root().join(".staging-crashed")).unwrap();
        assert_eq!(
            store.list().unwrap()[0].entry.active.as_deref(),
            Some("1.0.0")
        );
        store.clean(&[]);
        assert!(
            store.root().join(".staging-crashed").is_dir(),
            "not stale yet"
        );
    }

    #[test]
    fn reinstalling_replaces_a_folder_left_part_removed() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let first = package(data.path(), "1.0.0");
        let installed = store.install(&first, &source(&first), &app()).unwrap();
        let program =
            crate::program_file(&installed.dir, installed.manifest.program.as_ref().unwrap());
        // A folder that still matches is reused as it is.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let inode = || std::fs::metadata(&installed.dir).unwrap().ino();
            let before = inode();
            store.install(&first, &source(&first), &app()).unwrap();
            assert_eq!(inode(), before);
        }
        // A removal that stopped after the program (a locked file, a crash),
        // and one that left a stray file: each is unpacked again, whole.
        let damage: [&dyn Fn(); 2] = [&|| std::fs::remove_file(&program).unwrap(), &|| {
            std::fs::write(installed.dir.join("stray"), b"x").unwrap()
        }];
        for damage in damage {
            damage();
            assert!(!package::is_unpacked_in(&first, &installed.dir));
            store.install(&first, &source(&first), &app()).unwrap();
            assert!(package::is_unpacked_in(&first, &installed.dir));
            assert_eq!(std::fs::read(&program).unwrap(), b"program 1.0.0");
        }
        assert_eq!(
            store.list().unwrap()[0].entry.active.as_deref(),
            Some("1.0.0")
        );
        // Nothing staged or moved aside is left.
        let left: Vec<_> = std::fs::read_dir(store.root())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["dev.test.tool"]);
    }

    #[test]
    fn an_install_while_removing_keeps_its_folder() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let first = package(data.path(), "1.0.0");
        store.install(&first, &source(&first), &app()).unwrap();
        // A removal past its locked step, then an install of 1.1.0, then
        // the removal deleting what it took.
        let taken = store.forget("dev.test.tool").unwrap();
        let second = package(data.path(), "1.1.0");
        let installed = store.install(&second, &source(&second), &app()).unwrap();
        if let Some(taken) = taken {
            std::fs::remove_dir_all(taken).unwrap();
        }
        let entry = &store.list().unwrap()[0].entry;
        assert_eq!(entry.active.as_deref(), Some("1.1.0"));
        assert_eq!(entry.installed, ["1.1.0"]);
        assert!(
            installed
                .dir
                .join(crate::protocol::manifest::FILE)
                .is_file()
        );
        assert!(!store.version_dir("dev.test.tool", "1.0.0").exists());
        // Nothing moved aside is left.
        store.remove("dev.test.tool").unwrap();
        assert_eq!(std::fs::read_dir(store.root()).unwrap().count(), 0);
    }

    /// A removal that can't move the folder, or can't record that, leaves
    /// the extension installed with its folder.
    #[cfg(unix)]
    #[test]
    fn a_removal_that_fails_changes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let first = package(data.path(), "1.0.0");
        let installed = store.install(&first, &source(&first), &app()).unwrap();
        let mode = |path: &Path, mode| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
        };
        for locked in [store.root(), data.path().to_path_buf()] {
            mode(&locked, 0o555);
            // Permissions don't bind a privileged user.
            let binding = std::fs::create_dir(locked.join("probe")).is_err();
            let removed = store.remove("dev.test.tool");
            mode(&locked, 0o755);
            if !binding {
                return;
            }
            assert!(removed.is_err(), "{}", locked.display());
            let entry = &store.list().unwrap()[0].entry;
            assert_eq!(entry.active.as_deref(), Some("1.0.0"));
            assert!(package::is_unpacked_in(&first, &installed.dir));
            assert_eq!(std::fs::read_dir(store.root()).unwrap().count(), 1);
        }
    }

    #[test]
    fn a_folder_missing_a_file_its_manifest_names_is_never_reused() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let first = package(data.path(), "1.0.0");
        let installed = store.install(&first, &source(&first), &app()).unwrap();
        let program =
            crate::program_file(&installed.dir, installed.manifest.program.as_ref().unwrap());
        std::fs::remove_file(&program).unwrap();
        // The same version with only its manifest, which its SHA256SUMS lists
        // alone: the damaged folder holds exactly those files.
        let manifest_only = files("1.0.0").remove(0);
        let sums = package::write_sums([(manifest_only.0.as_str(), manifest_only.1.as_slice())]);
        let path = data.path().join("manifest-only");
        std::fs::create_dir(&path).unwrap();
        let path = path.join(first.file_name().unwrap());
        raw_zip(
            &path,
            &[
                (&manifest_only.0, &manifest_only.1),
                (package::SUMS, sums.as_bytes()),
            ],
        );
        assert!(!package::is_unpacked_in(&path, &installed.dir));
        let error = store.install(&path, &source(&path), &app()).unwrap_err();
        assert!(error.0.contains("program.path"), "{error}");
        assert!(!program.exists());
        // The intact package still repairs it.
        store.install(&first, &source(&first), &app()).unwrap();
        assert_eq!(std::fs::read(&program).unwrap(), b"program 1.0.0");
    }

    #[test]
    fn an_incompatible_or_official_package_is_refused_before_unpacking() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let path = package(data.path(), "1.0.0");
        let old_app = semver::Version::new(0, 0, 9);
        assert!(
            store
                .install(&path, &source(&path), &old_app)
                .unwrap_err()
                .0
                .contains("supports")
        );
        let official = data.path().join(format!(
            "rawmakase.midi-1.0.0-{}.rmext",
            crate::current_target()
        ));
        let mut files = files("1.0.0");
        files[0].1 = String::from_utf8(files[0].1.clone())
            .unwrap()
            .replace("dev.test.tool", "rawmakase.midi")
            .into_bytes();
        package::write(&official, &files, None).unwrap();
        assert!(
            store
                .install(&official, &source(&official), &app())
                .unwrap_err()
                .0
                .contains("official")
        );
        assert!(
            !store.root().exists() || std::fs::read_dir(store.root()).unwrap().next().is_none()
        );
    }

    #[test]
    fn enabling_a_bundled_extension_records_only_that() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        // Neither installed nor bundled: refused, and nothing is written.
        for (id, bundled) in [
            ("dev.test.typo", &["rawmakase.mcp"][..]),
            ("rawmakase.mcp", &[]),
        ] {
            let error = store.set_enabled(id, false, bundled).unwrap_err();
            assert!(error.0.contains("not installed"), "{error}");
        }
        assert!(!data.path().join(crate::state::FILE).exists());
        store
            .set_enabled("rawmakase.mcp", false, &["rawmakase.mcp"])
            .unwrap();
        let state = State::read(data.path()).unwrap();
        let entry = &state.extensions["rawmakase.mcp"];
        assert!(!entry.enabled);
        assert!(entry.installed.is_empty() && entry.active.is_none());
        assert_eq!(state.extensions.len(), 1);
        // A bundled extension can't be removed.
        assert!(store.remove("rawmakase.mcp").is_err());
    }

    #[test]
    fn a_developer_folder_is_registered_by_its_manifest() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let folder = data.path().join("src");
        std::fs::create_dir(&folder).unwrap();
        assert!(store.add_developer(&folder).is_err());
        std::fs::write(
            folder.join(crate::protocol::manifest::FILE),
            &files("0.1.0")[0].1,
        )
        .unwrap();
        let manifest = store.add_developer(&folder).unwrap();
        assert_eq!(manifest.extension.id, "dev.test.tool");
        store.set_enabled("dev.test.tool", false, &[]).unwrap();
        let state = State::read(data.path()).unwrap();
        assert!(!state.developer["dev.test.tool"].enabled);
        assert!(state.extensions.is_empty());
    }
}
