//! AI Denoise for command-line renders.
use super::{Service, wait_within};
use crate::camera_data::CameraImage;
use crate::develop::AiSource;
use anyhow::{Result, anyhow};
use rawmakase_cancel::Cancel;
use rawmakase_decode::ai::cache::AiCache;
use rawmakase_decode::ai::denoise::AiBase;
use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

/// AI Denoise for command-line renders (`render`, `render-batch`): one service for the
/// whole command, so the model loads once, with the user's cache where the app keeps
/// it. Its worker is stopped when this is dropped, waiting at most three seconds.
pub struct CommandLine {
    service: Service,
}
impl CommandLine {
    /// With the results' cache `cache`: the user's, where the app keeps it
    /// (`rawmakase::app::user_ai_denoise_cache`).
    pub fn new(cache: AiCache) -> Self {
        Self::with(Service::new(cache))
    }
    fn with(service: Service) -> Self {
        Self { service }
    }
    /// The AI Denoise source for a render of `path` (its full-size plain `image`,
    /// decoded from the file as `file` identifies it, read before decoding) when
    /// `recipe` renders AI Denoise, from the installed model, waiting for it. Never the
    /// plain image in its place: without one, the error says why.
    pub fn source(
        &self,
        path: &Path,
        file: &crate::storage::Identity,
        image: &Arc<CameraImage>,
        recipe: &crate::model::recipe::Recipe,
    ) -> Result<Option<AiSource>> {
        if !recipe.as_rendered().ai_denoise.renders() {
            return Ok(None);
        }
        anyhow::ensure!(
            !image.fast,
            "AI Denoise needs the full-size image; render without --fast"
        );
        let pack = crate::model_install::Pack::Denoise;
        anyhow::ensure!(
            pack.is_installed(),
            "AI Denoise is on but its model is not installed"
        );
        self.service.set_model(Some(pack.installed_dir()));
        self.denoise(path, file, image).map(Some)
    }
    fn denoise(
        &self,
        path: &Path,
        file: &crate::storage::Identity,
        image: &Arc<CameraImage>,
    ) -> Result<AiSource> {
        let cancel = AtomicBool::new(false);
        self.service
            .get(path, file, image, Cancel::new(&cancel))
            .and_then(|ai| AiSource::recover(Arc::unwrap_or_clone(ai), Cancel::new(&cancel)))
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
    use crate::ai_denoise::tests::{Behaviour, fixture};
    use rawmakase_inference::Provider;

    /// Issue 808 (D5): the renders of one `render-batch` share one service, so the model
    /// loads once rather than for each job.
    #[test]
    fn command_line_renders_share_one_service() {
        let f = fixture(Provider::Cpu, Behaviour::default());
        let plain = f.plain();
        let cli = CommandLine::with(f.service.clone());
        for name in ["a.raw", "b.raw"] {
            let photo = f.photo(name);
            let file = crate::storage::Identity::read(&photo).unwrap();
            cli.denoise(&photo, &file, &plain).unwrap();
        }
        assert_eq!(*f.behaviour.loads.lock().unwrap(), vec![Provider::Cpu]);
        drop(cli);
        assert!(!f.service.busy());
    }
}
