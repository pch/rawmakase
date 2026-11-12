//! The model packs the generators need, where they are installed, and setting
//! them up: Subject's (the selection models, which Background shares) and
//! Sky's, each installed and removed as a unit, so neither obliges anyone to
//! fetch the other.
//!
//! A pack lives in the extension's data folder, in `models/<id>-v<version>`.
//! The app installed the same packs in `<local data>/models/<id>-v<version>`
//! before this extension took them over; on first use a pack found there with
//! its receipt is adopted by renaming it into the extension's folder, or used
//! where it is when it can't be moved, so nobody downloads it again.
use rawmakase_extension::protocol::message::NeedsSetup;
use rawmakase_inference::ModelFile;
use rawmakase_model_install::{InstallError, Progress, is_installed};
use rawmakase_selection::manifest::{all_files, sky_files};
use rawmakase_selection::{SKY, SUBJECT};
use std::path::{Path, PathBuf};

/// One set of model files, installed and removed as a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pack {
    Subject,
    Sky,
}

impl Pack {
    pub(crate) const ALL: [Self; 2] = [Self::Subject, Self::Sky];

    pub(crate) fn files(self) -> Vec<ModelFile> {
        match self {
            Self::Subject => all_files(),
            Self::Sky => sky_files(),
        }
    }
    /// The folder it is kept in, named for the model and its contract version,
    /// so a replacement model sits beside it.
    pub(crate) fn folder(self) -> String {
        let (id, version) = match self {
            Self::Subject => (SUBJECT.id, SUBJECT.version),
            Self::Sky => (SKY.id, SKY.version),
        };
        format!("{id}-v{version}")
    }
    /// What setting it up downloads, as the drawer says it.
    pub(crate) fn needs_setup(self) -> NeedsSetup {
        NeedsSetup {
            what: match self {
                Self::Subject => "the selection models",
                Self::Sky => "the sky model",
            }
            .into(),
            bytes: self.files().iter().map(|f| f.size_bytes).sum(),
        }
    }
}

/// How a pack's files are fetched into its folder: [`download`] in the
/// program, a stand-in in tests.
pub type Install =
    Box<dyn Fn(&Path, &[ModelFile], &Progress) -> Result<(), InstallError> + Send + Sync>;

/// Fetches the files from where the release pins them and checks them
/// ([`rawmakase_model_install::install`]).
pub fn download(dir: &Path, files: &[ModelFile], progress: &Progress) -> Result<(), InstallError> {
    rawmakase_model_install::install(dir, files, None, progress)
}

/// Where the packs are: the extension's own folder, and the app's from before.
pub(crate) struct Models {
    /// `<extension data>/models`.
    own: PathBuf,
    /// `<local data>/models`, where the app installed them.
    legacy: PathBuf,
}

impl Models {
    pub(crate) fn new(own: PathBuf, legacy: PathBuf) -> Self {
        Self { own, legacy }
    }

    /// The pack's folder when it is installed, adopting the app's install of
    /// it on first use: moved into the extension's folder, or, when it can't
    /// be moved, used where it is. A folder without its receipt, or whose
    /// files don't match it, is not an install, and is left alone.
    pub(crate) fn installed(&self, pack: Pack) -> Option<PathBuf> {
        let files = pack.files();
        let own = self.own.join(pack.folder());
        if is_installed(&own, &files) {
            return Some(own);
        }
        let legacy = self.legacy.join(pack.folder());
        if !is_installed(&legacy, &files) {
            return None;
        }
        // A download that stopped part way may have left a folder here: it
        // holds nothing the app's install hasn't.
        if own.is_dir() && std::fs::read_dir(&own).is_ok_and(|mut d| d.next().is_none()) {
            let _ = std::fs::remove_dir(&own);
        }
        let moved = std::fs::create_dir_all(&self.own).is_ok()
            && !own.exists()
            && std::fs::rename(&legacy, &own).is_ok();
        if moved && is_installed(&own, &files) {
            Some(own)
        } else {
            Some(legacy)
        }
    }

    /// Fetches `pack` into the extension's folder with `install`, unless it
    /// is installed already, here or in the app's folder.
    pub(crate) fn set_up(
        &self,
        pack: Pack,
        install: &Install,
        progress: &Progress,
    ) -> Result<(), InstallError> {
        if self.installed(pack).is_some() {
            return Ok(());
        }
        install(&self.own.join(pack.folder()), &pack.files(), progress)
    }

    /// Removes every pack, the extension's and any of the app's it used in
    /// place. Returns how many folders went.
    pub(crate) fn remove(&self) -> Result<usize, String> {
        let mut removed = 0;
        for pack in Pack::ALL {
            for dir in [
                self.own.join(pack.folder()),
                self.legacy.join(pack.folder()),
            ] {
                if !dir.exists() {
                    continue;
                }
                std::fs::remove_dir_all(&dir)
                    .map_err(|e| format!("could not remove {}: {e}", dir.display()))?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawmakase_model_install::{RECEIPT, receipt};

    /// `pack`'s files at their sizes in `dir`, with its receipt when `receipt`.
    fn fake_install(dir: &Path, pack: Pack, with_receipt: bool) {
        std::fs::create_dir_all(dir).unwrap();
        for file in pack.files() {
            std::fs::File::create(dir.join(file.name))
                .unwrap()
                .set_len(file.size_bytes)
                .unwrap();
        }
        if with_receipt {
            std::fs::write(dir.join(RECEIPT), receipt(&pack.files())).unwrap();
        }
    }

    fn models(root: &Path) -> Models {
        Models::new(root.join("own/models"), root.join("local/models"))
    }

    #[test]
    fn each_pack_has_a_folder_named_for_its_model_and_version() {
        assert_eq!(
            Pack::Subject.folder(),
            format!("{}-v{}", SUBJECT.id, SUBJECT.version)
        );
        assert_eq!(Pack::Sky.folder(), format!("{}-v{}", SKY.id, SKY.version));
        assert!(Pack::Subject.needs_setup().bytes > 250_000_000);
        assert!(Pack::Sky.needs_setup().bytes > 200_000_000);
    }

    /// An install the app made before is adopted without downloading: moved
    /// into the extension's folder, which then has it, and the old one
    /// doesn't.
    #[test]
    fn the_apps_install_with_its_receipt_is_moved_in() {
        let root = tempfile::tempdir().unwrap();
        let m = models(root.path());
        let legacy = root
            .path()
            .join("local/models")
            .join(Pack::Subject.folder());
        fake_install(&legacy, Pack::Subject, true);
        let own = root.path().join("own/models").join(Pack::Subject.folder());
        assert_eq!(m.installed(Pack::Subject), Some(own.clone()));
        assert!(!legacy.exists());
        assert!(is_installed(&own, &Pack::Subject.files()));
        // Sky's wasn't installed, and isn't now.
        assert_eq!(m.installed(Pack::Sky), None);
        // Nothing is fetched to set it up.
        let fetch: Install = Box::new(|_, _, _| panic!("downloaded again"));
        m.set_up(Pack::Subject, &fetch, &Progress::default())
            .unwrap();
    }

    /// One without its receipt, or with a file of the wrong size, is not an
    /// install: it isn't adopted, and setting up downloads into the
    /// extension's own folder.
    #[test]
    fn the_apps_folder_without_a_good_receipt_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let m = models(root.path());
        let legacy = root.path().join("local/models").join(Pack::Sky.folder());
        fake_install(&legacy, Pack::Sky, false);
        assert_eq!(m.installed(Pack::Sky), None);
        assert!(legacy.exists(), "left where it is");
        std::fs::write(legacy.join(RECEIPT), "a receipt for other files\n").unwrap();
        assert_eq!(m.installed(Pack::Sky), None);
        let fetch: Install = Box::new(move |dir, files, _| {
            fake_install(dir, Pack::Sky, true);
            assert_eq!(files.len(), Pack::Sky.files().len());
            Ok(())
        });
        m.set_up(Pack::Sky, &fetch, &Progress::default()).unwrap();
        let own = root.path().join("own/models").join(Pack::Sky.folder());
        assert_eq!(m.installed(Pack::Sky), Some(own));
        assert!(legacy.exists());
    }

    /// When the app's install can't be moved (a folder of the same name is
    /// in the way), it is used where it is.
    #[test]
    fn the_apps_install_that_cant_move_is_used_in_place() {
        let root = tempfile::tempdir().unwrap();
        let m = models(root.path());
        let legacy = root
            .path()
            .join("local/models")
            .join(Pack::Subject.folder());
        fake_install(&legacy, Pack::Subject, true);
        let own = root.path().join("own/models").join(Pack::Subject.folder());
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join(".vision_encoder.onnx.part"), b"part").unwrap();
        assert_eq!(m.installed(Pack::Subject), Some(legacy.clone()));
        assert!(is_installed(&legacy, &Pack::Subject.files()));
        // Removing the models removes that one too.
        assert_eq!(m.remove().unwrap(), 2);
        assert!(!legacy.exists() && !own.exists());
        assert_eq!(m.installed(Pack::Subject), None);
    }
}
