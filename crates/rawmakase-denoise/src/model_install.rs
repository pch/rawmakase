//! The app's model pack, AI Denoise's: which files it holds and where it is
//! installed. Installing is [`rawmakase_model_install`]'s; the app's `Installer`
//! (`app/develop/model_installer.rs`) runs an install on its own thread, only on
//! the user's choice, and the app and the command line pass the AI Denoise
//! service the folder here ([`crate::model_dir`]) and the runtime's. Select Subject's and Sky's packs sit beside it in the same
//! `models` folder, installed by the AI masks extension (`extensions/ai-masks`)
//! there, as the app installed them before; nothing here lists or cleans that
//! folder, so neither is taken for an orphan.
use rawmakase_inference::ModelFile;
use rawmakase_inference::manifest::denoise_files;
use std::path::{Path, PathBuf};

pub use rawmakase_model_install::{InstallError, Progress, runtime_dir};

/// AI Denoise's model, with its `LICENSE` and `NOTICE`: an install without them
/// is not one.
fn files() -> Vec<ModelFile> {
    denoise_files()
}
/// The pack's size in bytes.
pub fn bytes() -> u64 {
    files().iter().map(|f| f.size_bytes).sum()
}
/// Where it is kept: under this computer's own data folder, in a folder named for the
/// model and its contract version, so a replacement model sits beside it.
pub fn dir() -> PathBuf {
    let (id, version) = (
        rawmakase_inference::RAWNIND.id,
        rawmakase_inference::RAWNIND.version,
    );
    rawmakase_model_install::models_dir().join(format!("{id}-v{version}"))
}
/// Whether every file is in place and verified.
pub fn is_installed() -> bool {
    is_installed_in(&dir())
}
/// Whether every file is in place and verified in `dir`, a folder of the pack's
/// own (its [`dir`] or another the caller chose).
pub fn is_installed_in(dir: &Path) -> bool {
    rawmakase_model_install::is_installed(dir, &files())
}
/// The megabytes the drawer quotes.
pub fn megabytes() -> u64 {
    bytes().div_ceil(1_000_000)
}

/// Fetches or copies the pack's files into its folder and checks them; see
/// [`rawmakase_model_install::install`].
pub fn install(import: Option<&Path>, progress: &Progress) -> Result<(), InstallError> {
    rawmakase_model_install::install(&dir(), &files(), import, progress)
}

#[cfg(test)]
mod tests;
