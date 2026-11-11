//! What the selection models make of the photo: the model input's pixels and
//! an action in, the coverage and what made it out. This is the half of a
//! selection that runs the models, locally or on the remote worker, and keeps
//! their sessions and the last photo's analysis between requests. The other
//! half, in [`super::worker`], renders the input and turns the coverage into a
//! raster; it is the boundary the AI masks extension (#965) answers across.
use super::{Action, Failure, Feature};
use crate::storage::{FNV_OFFSET, fnv1a};
use rawmakase_cancel::Cancel;
use rawmakase_inference::{InferenceError, LoadOptions};
use rawmakase_remote::{ClientError, RemoteAnalysis, RemoteSession};
use rawmakase_selection::{
    Analysis, Coverage, Prompt, RgbImage, SKY, SUBJECT, Session, Sky, SkySession, Subject,
};
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// What a selection asks the models: the photo as they see it, what to do, and
/// where they run.
pub(super) struct Ask<'a> {
    /// The model input: 8-bit sRGB, rendered by the caller.
    pub(super) rgb: &'a RgbImage,
    pub(super) action: &'a Action,
    /// The model's folder, when the selection runs on this computer.
    pub(super) model: Option<&'a Path>,
    /// The remote worker to ask instead, when one is configured.
    pub(super) endpoint: Option<&'a str>,
    pub(super) cancel: &'a AtomicBool,
}

/// What the models answered, and what a raster made of it records as its
/// provenance.
pub(super) struct Answer {
    /// At the input's size, unless that is over the coverage limit.
    pub(super) coverage: Coverage,
    pub(super) feature: Feature,
    /// What an all-zero coverage means: nothing found, or nothing there.
    pub(super) empty: Failure,
    /// The network that answered, as [`ModelId::provenance`] names it.
    pub(super) model: String,
    /// What the answer was made from: the pixels, the model and the prompt.
    pub(super) input: String,
}

/// Asks the models, reusing what `state` kept of this photo.
pub(super) fn generate(ask: &Ask<'_>, state: &mut State) -> Result<Answer, Failure> {
    let model = ask.action.model();
    let key = input_key(ask.rgb, model);
    let (coverage, feature, empty, input) = match (ask.action, ask.endpoint) {
        (Action::Auto(Feature::Sky), endpoint) => select_sky(
            ask.rgb,
            &key,
            endpoint,
            ask.model,
            Cancel::new(ask.cancel),
            state,
        )?,
        (_, Some(endpoint)) => select_remotely(endpoint, ask.rgb, &key, ask, state)?,
        (_, None) => select_locally(ask.rgb, &key, ask, state)?,
    };
    Ok(Answer {
        coverage,
        feature,
        empty,
        model: model.provenance(),
        input,
    })
}

/// The loaded models and the embedding of the photo last selected on. Over the
/// remote endpoint, the worker's answer for the photo last asked about, so its
/// clicks are answered from what it kept. The sky of the photo last asked about, from
/// either, so finding it again costs nothing. Each model loads when first needed: the
/// sky model is large and nothing else uses it.
#[derive(Default)]
pub(super) struct State {
    session: Option<Session>,
    sky_session: Option<SkySession>,
    analysis: Option<(String, Analysis)>,
    remote_analysis: Option<(String, RemoteAnalysis)>,
    sky: Option<(String, Coverage)>,
}

/// The photo's sky, from the worker when one is configured, else from the local model.
fn select_sky(
    rgb: &RgbImage,
    key: &str,
    endpoint: Option<&str>,
    model: Option<&std::path::Path>,
    cancel: Cancel<'_>,
    state: &mut State,
) -> Result<(Coverage, Feature, Failure, String), Failure> {
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
                if state.sky_session.is_none() {
                    state.sky_session =
                        Some(Sky::load_with(model, &load_options()).map_err(failure)?);
                }
                let session = state.sky_session.as_mut().expect("loaded above");
                session.select(rgb, cancel).map_err(failure)?
            }
        };
        state.sky = Some((key.to_string(), coverage));
    }
    let (_, coverage) = state.sky.as_ref().expect("found above");
    Ok((
        coverage.clone(),
        Feature::Sky,
        Failure::NoSky,
        prompt_key(key, "sky", &Prompt::default()),
    ))
}

/// What the local models make of the photo, and the prompt on it.
fn select_locally(
    rgb: &RgbImage,
    key: &str,
    ask: &Ask<'_>,
    state: &mut State,
) -> Result<(Coverage, Feature, Failure, String), Failure> {
    let Some(model) = ask.model else {
        return Err(Failure::ModelMissing);
    };
    if state.session.is_none() {
        state.session = Some(load(model)?);
    }
    let session = state.session.as_mut().expect("loaded above");
    if state.analysis.as_ref().is_none_or(|(k, _)| *k != key) {
        state.analysis = None;
        let analysis = session
            .analyze(rgb, Cancel::new(ask.cancel))
            .map_err(failure)?;
        state.analysis = Some((key.to_string(), analysis));
    }
    let (_, analysis) = state.analysis.as_ref().expect("analysed above");
    Ok(match ask.action {
        Action::Auto(feature) => (
            analysis.subject().map_err(failure)?,
            *feature,
            Failure::NoSubject,
            prompt_key(key, "subject", &Prompt::default()),
        ),
        Action::Click(prompt) => (
            session
                .segment(analysis.embedding(), prompt, Cancel::new(ask.cancel))
                .map_err(failure)?,
            Feature::Subject,
            Failure::NothingThere,
            prompt_key(key, "click", prompt),
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
) -> Result<(Coverage, Feature, Failure, String), Failure> {
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
            Ok((
                analysis.subject,
                Feature::Subject,
                Failure::NoSubject,
                prompt_key(key, "subject", &Prompt::default()),
            ))
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
            Ok((
                coverage,
                Feature::Subject,
                Failure::NothingThere,
                prompt_key(key, "click", prompt),
            ))
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

/// What the result was made from: the photo as the model saw it and the prompt.
fn prompt_key(input: &str, what: &str, prompt: &Prompt) -> String {
    let mut text = format!("{input}|{what}");
    for p in &prompt.points {
        text.push_str(&format!("|{:.4},{:.4},{}", p.x, p.y, p.positive));
    }
    if let Some(b) = prompt.bounds {
        text.push_str(&format!("|{b:.4?}"));
    }
    format!("{:016x}", fnv1a(FNV_OFFSET, text.as_bytes()))
}

fn load(model: &std::path::Path) -> Result<Session, Failure> {
    Subject::load_with(model, &load_options()).map_err(failure)
}

fn load_options() -> LoadOptions {
    let bundled = crate::model_install::runtime_dir();
    let runtime_library = rawmakase_inference::runtime::runtime_file_names()
        .iter()
        .map(|name| bundled.join(name))
        .find(|path| path.is_file());
    LoadOptions {
        runtime_library,
        threads: None,
        // Selections made on this computer run on its CPU, as before; the
        // accelerators are the remote worker's concern.
        provider: None,
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
pub(super) enum ModelId {
    Subject,
    Sky,
}
impl Action {
    /// Sky is the sky model's; everything else, clicks included, the subject models'.
    pub(super) fn model(&self) -> ModelId {
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
    pub(super) fn provenance(self) -> String {
        let (id, digest, ..) = self.identity();
        format!("{id}@{}", &digest[..16])
    }
}

/// Identifies what the model was given: the pixels, the model and the versions of
/// every step that shaped them. The same values execute the request.
fn input_key(rgb: &RgbImage, model: ModelId) -> String {
    let (id, digest, version, processing) = model.identity();
    let header = [
        rgb.width.to_le_bytes().as_slice(),
        rgb.height.to_le_bytes().as_slice(),
        id.as_bytes(),
        digest.as_bytes(),
        &version.to_le_bytes(),
        &processing.to_le_bytes(),
        &crate::develop::masks::SELECTION_INPUT_VERSION.to_le_bytes(),
    ]
    .concat();
    let hash = |seed| fnv1a(fnv1a(seed, &header), &rgb.data);
    format!("{:016x}{:016x}", hash(FNV_OFFSET), hash(0x84222325cbf29ce4))
}

/// Whether an ONNX Runtime library this copy can load is in a place it looks.
pub(super) fn runtime_present() -> bool {
    let names = rawmakase_inference::runtime::runtime_file_names();
    let mut dirs = rawmakase_inference::runtime::runtime_search_dirs();
    dirs.push(crate::model_install::runtime_dir());
    dirs.iter()
        .any(|dir| names.iter().any(|name| dir.join(name).is_file()))
        || std::env::var_os(rawmakase_inference::RUNTIME_ENV).is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn pixels() -> RgbImage {
        RgbImage {
            width: 2,
            height: 1,
            data: vec![1, 2, 3, 4, 5, 6],
        }
    }

    #[test]
    fn sky_and_subject_answers_never_share_a_key_or_a_provenance() {
        let rgb = pixels();
        assert_ne!(
            input_key(&rgb, ModelId::Sky),
            input_key(&rgb, ModelId::Subject)
        );
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
    }

    #[test]
    fn only_automatic_sky_is_the_sky_models() {
        assert_eq!(Action::Auto(Feature::Sky).model(), ModelId::Sky);
        assert_eq!(Action::Auto(Feature::Subject).model(), ModelId::Subject);
        assert_eq!(Action::Auto(Feature::Background).model(), ModelId::Subject);
        assert_eq!(Action::Click(Prompt::default()).model(), ModelId::Subject);
    }

    /// The photo the same, the sky's answer is reused; another photo is asked anew. With no
    /// model and no worker, only the cached photo's answer can come back.
    #[test]
    fn a_sky_found_on_a_photo_is_reused_only_for_that_photo() {
        let rgb = pixels();
        let key = input_key(&rgb, ModelId::Sky);
        let found = Coverage {
            width: 2,
            height: 1,
            data: vec![255, 0],
        };
        let mut state = State {
            sky: Some((key.clone(), found)),
            ..State::default()
        };
        let cancel = AtomicBool::new(false);
        let (coverage, feature, ..) =
            select_sky(&rgb, &key, None, None, Cancel::new(&cancel), &mut state).unwrap();
        assert_eq!((coverage.data, feature), (vec![255, 0], Feature::Sky));
        assert!(matches!(
            select_sky(
                &rgb,
                "another",
                None,
                None,
                Cancel::new(&cancel),
                &mut state
            ),
            Err(Failure::ModelMissing)
        ));
        assert!(state.sky.is_none(), "the old photo's answer is let go of");
    }
}
