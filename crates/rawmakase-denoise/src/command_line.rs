//! AI Denoise for command-line renders.
use super::{Service, wait_within};
use crate::camera_data::CameraImage;
use crate::develop::AiSource;
use anyhow::{Result, anyhow};
use rawmakase_cancel::Cancel;
use rawmakase_decode::ai::cache::AiCache;
use rawmakase_decode::ai::denoise::AiBase;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// AI Denoise for command-line renders (`render`, `render-batch`): one service for the
/// whole command, so the model stays loaded across back-to-back renders (unloading
/// after 60 s idle and reloading on demand). Its worker is stopped when this is
/// dropped, waiting at most three seconds.
pub struct CommandLine {
    service: Service,
    /// The model pack's folder, used when the model is installed there.
    model: PathBuf,
}
impl CommandLine {
    /// With the results' cache `cache`, the model pack installed in `model` and the
    /// ONNX Runtime bundled in `runtime`. The desktop binary passes the user's: the
    /// cache where the app keeps it (`rawmakase::app::user_ai_denoise_cache`), and
    /// [`super::model_dir`] and [`rawmakase_model_install::runtime_dir`]
    /// in this computer's own data folder.
    pub fn new(cache: AiCache, model: PathBuf, runtime: PathBuf) -> Self {
        Self::with(Service::new(cache, runtime), model)
    }
    fn with(service: Service, model: PathBuf) -> Self {
        Self { service, model }
    }
    /// The AI Denoise source for a render of `path` (its full-size plain `image`,
    /// decoded from the file as `file` identifies it, read before decoding) when
    /// `recipe` renders AI Denoise, from the installed model, waiting for it. Never the
    /// plain image in its place: without one, the error says why. `cancel` stops the
    /// run between tiles and the recovery of its highlights.
    pub fn source(
        &self,
        path: &Path,
        file: &crate::storage::Identity,
        image: &Arc<CameraImage>,
        recipe: &crate::model::recipe::Recipe,
        cancel: Cancel<'_>,
    ) -> Result<Option<AiSource>> {
        if !recipe.as_rendered().ai_denoise.renders() {
            return Ok(None);
        }
        anyhow::ensure!(
            !image.fast,
            "AI Denoise needs the full-size image; render without --fast"
        );
        anyhow::ensure!(
            crate::model_install::is_installed_in(&self.model),
            "AI Denoise is on but its model is not installed"
        );
        self.service.set_model(Some(self.model.clone()));
        self.denoise(path, file, image, cancel).map(Some)
    }
    fn denoise(
        &self,
        path: &Path,
        file: &crate::storage::Identity,
        image: &Arc<CameraImage>,
        cancel: Cancel<'_>,
    ) -> Result<AiSource> {
        self.service
            .get(path, file, image, cancel)
            .and_then(|ai| AiSource::recover(Arc::unwrap_or_clone(ai), cancel))
            .map_err(|e| anyhow!("AI Denoise: {e:#}"))
    }
}
impl Drop for CommandLine {
    fn drop(&mut self) {
        wait_within(self.service.stop(), Duration::from_secs(3));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{Behaviour, fixture};
    use rawmakase_inference::Provider;

    /// Issue 808 (D5): the renders of one `render-batch` share one service, so the model
    /// loads once rather than for each job.
    #[test]
    fn command_line_renders_share_one_service() {
        let f = fixture(Provider::Cpu, Behaviour::default());
        let plain = f.plain();
        let cli = CommandLine::with(f.service.clone(), f.dir.path().join("model"));
        for name in ["a.raw", "b.raw"] {
            let photo = f.photo(name);
            let file = crate::storage::Identity::read(&photo).unwrap();
            cli.denoise(&photo, &file, &plain, Cancel::NEVER).unwrap();
        }
        assert_eq!(*f.behaviour.loads.lock().unwrap(), vec![Provider::Cpu]);
        drop(cli);
        assert!(!f.service.busy());
    }
}
