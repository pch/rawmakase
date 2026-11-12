//! What runs the selection models on this computer: the ONNX sessions of
//! `rawmakase-selection`, or a test's stand-in ([`crate::fake`]). The remote
//! worker's client is [`crate::inference`]'s, beside it.
use rawmakase_cancel::Cancel;
use rawmakase_inference::InferenceError;
use rawmakase_selection::{
    Analysis, Coverage, Prompt, RgbImage, Session, Sky, SkySession, Subject,
};
use std::path::Path;

/// The selection models, loaded from the folder each call names. A call that
/// names a photo by `key` may answer from what it kept of the photo it last
/// analysed under that key.
pub trait Engine: Send {
    /// The photo's people and animals: all zero where there are none.
    fn subject(
        &mut self,
        models: &Path,
        photo: &RgbImage,
        key: &str,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError>;
    /// What the clicks and box of `prompt` outline on the photo.
    fn segment(
        &mut self,
        models: &Path,
        photo: &RgbImage,
        key: &str,
        prompt: &Prompt,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError>;
    /// The photo's sky, from the sky model in `models`.
    fn sky(
        &mut self,
        models: &Path,
        photo: &RgbImage,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError>;
    /// Lets go of the loaded models and what was kept of the photo, so their
    /// files can be removed.
    fn unload(&mut self);
}

/// The models on this computer's CPU: one loaded session of each, and the
/// embedding of the photo last selected on, so a second click on it takes
/// milliseconds instead of a second. Each loads when first needed: the sky
/// model is large and nothing else uses it.
#[derive(Default)]
pub struct Onnx {
    session: Option<Session>,
    sky_session: Option<SkySession>,
    analysis: Option<(String, Analysis)>,
}

impl Onnx {
    /// The subject session and the photo's analysis, loading and analysing
    /// as needed.
    fn analysed(
        &mut self,
        models: &Path,
        photo: &RgbImage,
        key: &str,
        cancel: Cancel<'_>,
    ) -> Result<(&mut Session, &Analysis), InferenceError> {
        if self.session.is_none() {
            self.session = Some(Subject::load_with(models, &crate::runtime::load_options())?);
        }
        let session = self.session.as_mut().expect("loaded above");
        if self.analysis.as_ref().is_none_or(|(k, _)| *k != key) {
            self.analysis = None;
            let analysis = session.analyze(photo, cancel)?;
            self.analysis = Some((key.to_string(), analysis));
        }
        let (_, analysis) = self.analysis.as_ref().expect("analysed above");
        Ok((session, analysis))
    }
}

impl Engine for Onnx {
    fn subject(
        &mut self,
        models: &Path,
        photo: &RgbImage,
        key: &str,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError> {
        let (_, analysis) = self.analysed(models, photo, key, cancel)?;
        analysis.subject()
    }

    fn segment(
        &mut self,
        models: &Path,
        photo: &RgbImage,
        key: &str,
        prompt: &Prompt,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError> {
        let (session, analysis) = self.analysed(models, photo, key, cancel)?;
        session.segment(analysis.embedding(), prompt, cancel)
    }

    fn sky(
        &mut self,
        models: &Path,
        photo: &RgbImage,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError> {
        if self.sky_session.is_none() {
            self.sky_session = Some(Sky::load_with(models, &crate::runtime::load_options())?);
        }
        let session = self.sky_session.as_mut().expect("loaded above");
        session.select(photo, cancel)
    }

    fn unload(&mut self) {
        *self = Self::default();
    }
}
