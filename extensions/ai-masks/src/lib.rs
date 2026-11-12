//! The bundled AI masks extension (`rawmakase.ai-masks`): Select Subject, Sky
//! and Background, made by RAWmakase's selection models on this computer or
//! on a remote worker.
// Copied from the app as it is, and not yet called.
#![allow(dead_code)]
mod inference;
mod remote;

use rawmakase_selection::Prompt as ModelPrompt;

/// The version of how the app renders the model input
/// (`develop::masks::SELECTION_INPUT_VERSION`).
const SELECTION_INPUT_VERSION: u32 = 1;

/// Which of the selections a request makes. Background is the same coverage
/// as Subject, with the component inverted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Feature {
    Subject,
    Sky,
    Background,
}

/// What a job does: finds the photo's subject or sky by itself, or answers clicks.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    Auto(Feature),
    Click(ModelPrompt),
}

/// Why a selection did not make a mask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Cancelled,
    /// The model found nothing; no mask is made.
    NoSubject,
    NoSky,
    /// Nothing at the clicked place.
    NothingThere,
    /// The model file is not installed (or was removed meanwhile).
    ModelMissing,
    /// The inference runtime could not be loaded or has no support here.
    RuntimeUnavailable(String),
    Failed(String),
}
