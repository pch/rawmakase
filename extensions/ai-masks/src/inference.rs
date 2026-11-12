//! What the selection models make of the photo: the model input's pixels and
//! an action in, the coverage and what made it out. This is the half of a
//! selection that runs the models, locally ([`Engine`]) or on the remote
//! worker, and keeps the last photo's analysis between requests. The app
//! renders the input and turns the coverage into a raster.
use crate::engine::Engine;
use crate::{Action, Failure, Feature};
use rawmakase_cancel::Cancel;
use rawmakase_inference::InferenceError;
use rawmakase_remote::{ClientError, RemoteAnalysis, RemoteSession};
use rawmakase_selection::{Coverage, RgbImage, SKY, SUBJECT};
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// What a selection asks the models: the photo as they see it, what to do, and
/// where they run.
pub(crate) struct Ask<'a> {
    /// The model input: 8-bit sRGB, rendered by the app.
    pub(crate) rgb: &'a RgbImage,
    /// The app's digest of the input (its size, pixels and how it was
    /// rendered), which names the photo in what is kept of it.
    pub(crate) image_key: &'a str,
    pub(crate) action: &'a Action,
    /// The model's folder, when the selection runs on this computer.
    pub(crate) model: Option<&'a Path>,
    /// The remote worker to ask instead, when one is configured.
    pub(crate) endpoint: Option<&'a str>,
    pub(crate) cancel: &'a AtomicBool,
}

/// What the models answered, and what a raster made of it records as its
/// provenance.
pub(crate) struct Answer {
    /// At the input's size, unless that is over the coverage limit.
    pub(crate) coverage: Coverage,
    /// What an all-zero coverage means: nothing found, or nothing there.
    pub(crate) empty: Failure,
    /// The network that answered, as [`ModelId::provenance`] names it.
    pub(crate) model: String,
    /// Everything besides the pixels the answer depends on
    /// ([`ModelId::key`]).
    pub(crate) model_key: String,
}

/// Asks the models, reusing what `state` kept of this photo.
pub(crate) fn generate(
    ask: &Ask<'_>,
    state: &mut State,
    engine: &mut dyn Engine,
) -> Result<Answer, Failure> {
    let model = ask.action.model();
    let key = format!("{}|{}", ask.image_key, model.key());
    let (coverage, empty) = match (ask.action, ask.endpoint) {
        (Action::Auto(Feature::Sky), endpoint) => select_sky(
            ask.rgb,
            &key,
            endpoint,
            ask.model,
            Cancel::new(ask.cancel),
            state,
            engine,
        )?,
        (_, Some(endpoint)) => select_remotely(endpoint, ask.rgb, &key, ask, state)?,
        (_, None) => select_locally(ask.rgb, &key, ask, engine)?,
    };
    Ok(Answer {
        coverage,
        empty,
        model: model.provenance(),
        model_key: model.key(),
    })
}

/// Over the remote endpoint, the worker's answer for the photo last asked
/// about, so its clicks are answered from what it kept. The sky of the photo
/// last asked about, from either, so finding it again costs nothing. The local
/// models keep their own ([`Engine`]).
#[derive(Default)]
pub(crate) struct State {
    remote_analysis: Option<(String, RemoteAnalysis)>,
    sky: Option<(String, Coverage)>,
}

/// The photo's sky, from the worker when one is configured, else from the local model.
fn select_sky(
    rgb: &RgbImage,
    key: &str,
    endpoint: Option<&str>,
    model: Option<&Path>,
    cancel: Cancel<'_>,
    state: &mut State,
    engine: &mut dyn Engine,
) -> Result<(Coverage, Failure), Failure> {
    if state.sky.as_ref().is_none_or(|(k, _)| *k != key) {
        state.sky = None;
        let coverage = match endpoint {
            Some(endpoint) => RemoteSession::new(endpoint)
                .sky(rgb, cancel)
                .map_err(remote_failure)?,
            None => {
                let Some(model) = model else {
                    return Err(Failure::ModelMissing);
                };
                engine.sky(model, rgb, cancel).map_err(failure)?
            }
        };
        state.sky = Some((key.to_string(), coverage));
    }
    let (_, coverage) = state.sky.as_ref().expect("found above");
    Ok((coverage.clone(), Failure::NoSky))
}

/// What the local models make of the photo, and the prompt on it.
fn select_locally(
    rgb: &RgbImage,
    key: &str,
    ask: &Ask<'_>,
    engine: &mut dyn Engine,
) -> Result<(Coverage, Failure), Failure> {
    let Some(model) = ask.model else {
        return Err(Failure::ModelMissing);
    };
    let cancel = Cancel::new(ask.cancel);
    Ok(match ask.action {
        Action::Auto(_) => (
            engine.subject(model, rgb, key, cancel).map_err(failure)?,
            Failure::NoSubject,
        ),
        Action::Click(prompt) => (
            engine
                .segment(model, rgb, key, prompt, cancel)
                .map_err(failure)?,
            Failure::NothingThere,
        ),
    })
}

/// What the remote worker makes of the photo. The analysis it sent for this
/// photo is kept, so its clicks are answered without asking it to look again;
/// when it has forgotten the photo, it is asked once more.
fn select_remotely(
    endpoint: &str,
    rgb: &RgbImage,
    key: &str,
    ask: &Ask<'_>,
    state: &mut State,
) -> Result<(Coverage, Failure), Failure> {
    let session = RemoteSession::new(endpoint);
    // The worker keeps the photo's analysis under its own name; the one it
    // sent last is kept here, keyed the same way as a local one would be.
    let analysed = |state: &mut State| {
        if state
            .remote_analysis
            .as_ref()
            .is_none_or(|(k, _)| *k != key)
        {
            state.remote_analysis = None;
            let analysis = session
                .analyze(rgb, Cancel::new(ask.cancel))
                .map_err(remote_failure)?;
            state.remote_analysis = Some((key.to_string(), analysis));
        }
        Ok(state
            .remote_analysis
            .as_ref()
            .map(|(_, a)| a.clone())
            .expect("analysed above"))
    };
    match ask.action {
        Action::Auto(_) => {
            let analysis = analysed(state)?;
            Ok((analysis.subject, Failure::NoSubject))
        }
        Action::Click(prompt) => {
            let analysis = analysed(state)?;
            let coverage = match session.segment(&analysis, prompt, Cancel::new(ask.cancel)) {
                Ok(coverage) => coverage,
                Err(ClientError::Expired) => {
                    // The worker keeps only the last photos: ask once more.
                    state.remote_analysis = None;
                    let analysis = analysed(state)?;
                    session
                        .segment(&analysis, prompt, Cancel::new(ask.cancel))
                        .map_err(remote_failure)?
                }
                Err(e) => return Err(remote_failure(e)),
            };
            Ok((coverage, Failure::NothingThere))
        }
    }
}

/// What a remote answer means to the drawer.
fn remote_failure(error: ClientError) -> Failure {
    match error {
        ClientError::Cancelled => Failure::Cancelled,
        ClientError::Unavailable(why) => Failure::RuntimeUnavailable(why),
        ClientError::Expired => Failure::Failed("the remote worker forgot this photo".into()),
        ClientError::Failed(why) => Failure::Failed(format!("the remote worker: {why}")),
    }
}

fn failure(error: InferenceError) -> Failure {
    match error {
        InferenceError::Cancelled => Failure::Cancelled,
        InferenceError::RuntimeUnavailable(why) => Failure::RuntimeUnavailable(why),
        InferenceError::ModelInvalid(why) => {
            Failure::Failed(format!("the installed model is not usable: {why}"))
        }
        InferenceError::Failed(why) | InferenceError::OutputInvalid(why) => Failure::Failed(why),
    }
}

/// Which network a job's answer comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelId {
    Subject,
    Sky,
}
impl Action {
    /// Sky is the sky model's; everything else, clicks included, the subject models'.
    pub(crate) fn model(&self) -> ModelId {
        match self {
            Action::Auto(Feature::Sky) => ModelId::Sky,
            _ => ModelId::Subject,
        }
    }
}
impl ModelId {
    /// Its name, its weights' digest and the versions of its contract and processing.
    fn identity(self) -> (&'static str, &'static str, u32, u32) {
        match self {
            Self::Subject => (
                SUBJECT.id,
                SUBJECT.files[1].sha256,
                SUBJECT.version,
                SUBJECT.processing_version,
            ),
            Self::Sky => (SKY.id, SKY.file.sha256, SKY.version, SKY.processing_version),
        }
    }
    /// What a generated raster records as having made it.
    pub(crate) fn provenance(self) -> String {
        let (id, digest, ..) = self.identity();
        format!("{id}@{}", &digest[..16])
    }
    /// What an answer depends on besides the pixels: the model, its weights
    /// and the versions of its contract and processing, as the app's input
    /// identity of the raster records them. The app's image key covers the
    /// pixels and how they were rendered.
    pub(crate) fn key(self) -> String {
        let (id, digest, version, processing) = self.identity();
        format!("{id}|{digest}|{version}|{processing}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use rawmakase_extension::protocol::method::MAX_MODEL_LEN;
    use rawmakase_selection::Prompt;

    fn pixels() -> RgbImage {
        RgbImage {
            width: 2,
            height: 1,
            data: vec![1, 2, 3, 4, 5, 6],
        }
    }

    #[test]
    fn sky_and_subject_answers_never_share_a_key_or_a_provenance() {
        assert_ne!(ModelId::Sky.key(), ModelId::Subject.key());
        assert!(
            ModelId::Sky
                .provenance()
                .starts_with("upernet-convnext-base@")
        );
        assert!(
            ModelId::Subject
                .provenance()
                .starts_with("sam2.1-hiera-small@")
        );
        for model in [ModelId::Sky, ModelId::Subject] {
            assert!(model.key().len() <= MAX_MODEL_LEN, "{}", model.key());
            assert!(model.provenance().len() <= MAX_MODEL_LEN);
        }
    }

    #[test]
    fn only_automatic_sky_is_the_sky_models() {
        assert_eq!(Action::Auto(Feature::Sky).model(), ModelId::Sky);
        assert_eq!(Action::Auto(Feature::Subject).model(), ModelId::Subject);
        assert_eq!(Action::Click(Prompt::default()).model(), ModelId::Subject);
    }

    /// The photo the same, the sky's answer is reused; another photo is asked anew. With no
    /// model and no worker, only the cached photo's answer can come back.
    #[test]
    fn a_sky_found_on_a_photo_is_reused_only_for_that_photo() {
        let rgb = pixels();
        let key = "photo|sky";
        let found = Coverage {
            width: 2,
            height: 1,
            data: vec![255, 0],
        };
        let mut state = State {
            sky: Some((key.into(), found)),
            ..State::default()
        };
        let mut engine = Fake::default();
        let cancel = AtomicBool::new(false);
        let (coverage, empty) = select_sky(
            &rgb,
            key,
            None,
            None,
            Cancel::new(&cancel),
            &mut state,
            &mut engine,
        )
        .unwrap();
        assert_eq!((coverage.data, empty), (vec![255, 0], Failure::NoSky));
        assert!(matches!(
            select_sky(
                &rgb,
                "another",
                None,
                None,
                Cancel::new(&cancel),
                &mut state,
                &mut engine
            ),
            Err(Failure::ModelMissing)
        ));
        assert!(state.sky.is_none(), "the old photo's answer is let go of");
    }
}
