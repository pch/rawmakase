use std::fmt;

/// Why a model could not produce its result (a selection, or AI Denoise's
/// tiles). Every variant is a value the caller can show or retry on; nothing in
/// this crate panics on a missing runtime, a damaged model or a bad output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceError {
    /// No usable ONNX Runtime library was found or it was rejected (missing,
    /// wrong architecture, older than the minimum API). The string lists what
    /// was tried. The app keeps working and saved masks keep rendering.
    RuntimeUnavailable(String),
    /// The model file is missing, has the wrong size, or its tensors do not
    /// match the manifest contract.
    ModelInvalid(String),
    /// The caller's cancel flag was raised before the result was produced.
    Cancelled,
    /// The runtime failed while loading or running the model.
    Failed(String),
    /// The model ran but its output is not valid (a matte's or a denoised
    /// tile's shape, NaN, infinity, values outside the manifest's output
    /// range), or the input image was malformed.
    OutputInvalid(String),
}

impl fmt::Display for InferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeUnavailable(why) => write!(f, "ONNX Runtime is unavailable: {why}"),
            Self::ModelInvalid(why) => write!(f, "the model is invalid: {why}"),
            Self::Cancelled => f.write_str("the selection was cancelled"),
            Self::Failed(why) => write!(f, "inference failed: {why}"),
            Self::OutputInvalid(why) => write!(f, "the model output is invalid: {why}"),
        }
    }
}

impl std::error::Error for InferenceError {}
