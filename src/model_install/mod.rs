//! The app's model packs (Select Subject's and Sky's, AI Denoise's): which files each
//! holds and where it is installed. Installing is
//! [`rawmakase_model_install`]'s; the app's `Installer`
//! (`app/develop/model_installer.rs`) runs an install on its own thread, only on
//! the user's choice, and the AI Denoise service and the selection worker find the
//! installed models and the runtime here.
use rawmakase_inference::ModelFile;
use rawmakase_inference::manifest::denoise_files;
use rawmakase_selection::manifest::{all_files, sky_files};
use std::path::{Path, PathBuf};

pub(crate) use rawmakase_model_install::{InstallError, Progress, runtime_dir};

/// One set of model files, installed and removed as a unit: Subject and Background share
/// one, Sky and AI Denoise each have their own, so none obliges anyone to fetch another's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Pack {
    #[default]
    Subject,
    Sky,
    /// AI Denoise's model, with its `LICENSE` and `NOTICE`: an install without them is
    /// not one.
    Denoise,
}
impl Pack {
    fn files(self) -> Vec<ModelFile> {
        match self {
            Self::Subject => all_files(),
            Self::Sky => sky_files(),
            Self::Denoise => denoise_files(),
        }
    }
    pub(crate) fn bytes(self) -> u64 {
        self.files().iter().map(|f| f.size_bytes).sum()
    }
    /// Where it is kept: under this computer's own data folder, in a folder named for the
    /// model and its contract version, so a replacement model sits beside it.
    pub(crate) fn dir(self) -> PathBuf {
        let (id, version) = match self {
            Self::Subject => (
                rawmakase_selection::SUBJECT.id,
                rawmakase_selection::SUBJECT.version,
            ),
            Self::Sky => (
                rawmakase_selection::SKY.id,
                rawmakase_selection::SKY.version,
            ),
            Self::Denoise => (
                rawmakase_inference::RAWNIND.id,
                rawmakase_inference::RAWNIND.version,
            ),
        };
        rawmakase_model_install::models_dir().join(format!("{id}-v{version}"))
    }
    /// Whether every file is in place and verified.
    pub(crate) fn is_installed(self) -> bool {
        rawmakase_model_install::is_installed(&self.dir(), &self.files())
    }
    /// Where the pack is installed (whether or not it is).
    pub(crate) fn installed_dir(self) -> PathBuf {
        self.dir()
    }
    /// The megabytes the drawer quotes.
    pub(crate) fn megabytes(self) -> u64 {
        self.bytes().div_ceil(1_000_000)
    }
}

/// Fetches or copies `pack`'s files into its folder and checks them; see
/// [`rawmakase_model_install::install`].
pub(crate) fn install(
    pack: Pack,
    import: Option<&Path>,
    progress: &Progress,
) -> Result<(), InstallError> {
    rawmakase_model_install::install(&pack.dir(), &pack.files(), import, progress)
}

#[cfg(test)]
mod tests;
