//! The model packs the generators need, where they are installed, and setting
//! them up: Subject's (the selection models, which Background shares) and
//! Sky's, each installed and removed as a unit, so neither obliges anyone to
//! fetch the other.
//!
//! A pack lives where the app has always installed it, in
//! `<local data>/models/<id>-v<version>` beside AI Denoise's
//! ([`rawmakase_model_install::models_dir`]): this computer's own folder,
//! never a roaming or `RAWMAKASE_DATA_DIR` one, so an install the app made
//! before this extension counts as installed, and older versions of the app
//! still find it.
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

/// Where the packs are: `<local data>/models`.
pub(crate) struct Models {
    dir: PathBuf,
}

impl Models {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The pack's folder when it is installed: every file there with its
    /// size, and the installer's receipt naming exactly those files.
    pub(crate) fn installed(&self, pack: Pack) -> Option<PathBuf> {
        let dir = self.dir.join(pack.folder());
        is_installed(&dir, &pack.files()).then_some(dir)
    }

    /// Fetches `pack` into its folder with `install`, unless it is installed
    /// already.
    pub(crate) fn set_up(
        &self,
        pack: Pack,
        install: &Install,
        progress: &Progress,
    ) -> Result<(), InstallError> {
        if self.installed(pack).is_some() {
            return Ok(());
        }
        install(&self.dir.join(pack.folder()), &pack.files(), progress)
    }

    /// Removes Subject's and Sky's packs, and nothing else of the folder (AI
    /// Denoise's pack is the app's). Returns how many folders went.
    pub(crate) fn remove(&self) -> Result<usize, String> {
        let mut removed = 0;
        for pack in Pack::ALL {
            let dir = self.dir.join(pack.folder());
            if !dir.exists() {
                continue;
            }
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("could not remove {}: {e}", dir.display()))?;
            removed += 1;
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

    /// A pack in the folder with its receipt is installed where it is, and
    /// setting it up downloads nothing; one without its receipt, or with a
    /// receipt for other files, isn't, and setting it up fetches it into that
    /// same folder. Nothing is moved.
    #[test]
    fn a_pack_with_a_valid_receipt_in_the_folder_is_installed_where_it_is() {
        let root = tempfile::tempdir().unwrap();
        let m = Models::new(root.path().join("models"));
        let subject = root.path().join("models").join(Pack::Subject.folder());
        fake_install(&subject, Pack::Subject, true);
        assert_eq!(m.installed(Pack::Subject), Some(subject.clone()));
        let fetch: Install = Box::new(|_, _, _| panic!("downloaded again"));
        m.set_up(Pack::Subject, &fetch, &Progress::default())
            .unwrap();
        assert!(is_installed(&subject, &Pack::Subject.files()));
        let sky = root.path().join("models").join(Pack::Sky.folder());
        fake_install(&sky, Pack::Sky, false);
        assert_eq!(m.installed(Pack::Sky), None);
        std::fs::write(sky.join(RECEIPT), "a receipt for other files\n").unwrap();
        assert_eq!(m.installed(Pack::Sky), None);
        let fetch: Install = Box::new(|dir, files, _| {
            assert_eq!(files.len(), Pack::Sky.files().len());
            fake_install(dir, Pack::Sky, true);
            Ok(())
        });
        m.set_up(Pack::Sky, &fetch, &Progress::default()).unwrap();
        assert_eq!(m.installed(Pack::Sky), Some(sky));
    }

    /// Removing takes Subject's and Sky's packs and leaves the rest of the
    /// folder, AI Denoise's pack among it.
    #[test]
    fn removing_leaves_the_other_packs_alone() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("models");
        let m = Models::new(dir.clone());
        fake_install(&dir.join(Pack::Subject.folder()), Pack::Subject, true);
        let denoise = dir.join("rawnind-v1");
        std::fs::create_dir_all(&denoise).unwrap();
        std::fs::write(denoise.join("model.onnx"), b"").unwrap();
        assert_eq!(m.remove().unwrap(), 1);
        assert_eq!(m.installed(Pack::Subject), None);
        assert!(denoise.join("model.onnx").is_file());
        assert_eq!(m.remove().unwrap(), 0);
    }
}
