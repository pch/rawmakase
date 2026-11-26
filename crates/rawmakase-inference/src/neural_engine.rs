//! AI Denoise on Apple's Neural Engine: [`RAWNIND`] converted to a half-precision Core
//! ML model, run by Core ML directly. ONNX Runtime cannot put the network there: its
//! CoreML provider keeps it in single precision, which the Neural Engine does not
//! compute, and the network's values (up to about 1.3 million) overflow half precision
//! (65,504) as they are.
//!
//! The conversion (`scripts/models/convert_denoise_ane.py`) multiplies each
//! convolution's output by a power of two so its values fit half precision; every
//! operation in the network is positively homogeneous, so in full precision the result
//! is unchanged. The model therefore takes the input multiplied by `input_scale` and
//! answers the output multiplied by `output_scale`; both are read from its metadata,
//! with the SHA-256 of the ONNX model it was made from, which must be [`RAWNIND`]'s.
//!
//! Nothing of the converted model is downloaded. The repository holds what has no
//! weights (`models/neural-engine`: the package's `Manifest.json`, the ML Program
//! `model.mlmodel`, and `scales.txt`, each weight's and bias's powers of two); the
//! weights are made on the computer that runs them, from [`RAWNIND`]'s installed model
//! ([`weights`]), and checked against [`WEIGHTS_SHA256`]. The first load writes the
//! package in a folder of its own inside [`RAWNIND`]'s ([`FOLDER`]), compiles it for
//! this Mac, and keeps the compiled model there, named for the program and the weights
//! ([`tag`]).
//!
//! Loading for the Neural Engine compiles the network for it again, which can take
//! seconds and, when Apple's compiler service is stuck, never return. A load is
//! therefore given up after [`LOAD_LIMIT`], and for the rest of the process after one
//! has been ([`stalled`]): the caller then uses CoreML's GPU instead. Core ML's load
//! can't be cancelled: a caller waits for it up to that limit (the AI Denoise worker,
//! when it is stopped or the model is removed meanwhile), and a load given up keeps its
//! thread until it returns, if ever; its work folder is removed by a later load an hour
//! on ([`core_ml`]).
use crate::core_ml::{self, Converted, Model, Units, onnx};
use crate::denoise::TileModel;
use crate::{InferenceError, RAWNIND};
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The converted model: the package's `Manifest.json` as the conversion wrote it, and the
/// ML Program (the network's operations, the scales in its metadata, and where in the
/// weights file each tensor is). No weights.
const CONVERTED: Converted = Converted {
    prefix: FOLDER,
    manifest: include_bytes!("../models/neural-engine/Manifest.json"),
    program: include_bytes!("../models/neural-engine/model.mlmodel"),
};
/// Each weight and bias the program reads, in the weights file's order: its name in
/// [`RAWNIND`]'s ONNX model, the axis of its input channels (`-` for a bias) and the power
/// of two each input channel is multiplied by, run-length encoded (`-3*32`).
const SCALES: &str = include_str!("../models/neural-engine/scales.txt");
/// The weights file [`weights`] makes from [`RAWNIND`]'s model, as the conversion wrote it.
pub const WEIGHTS_SHA256: &str = "f87d840b51534fa8d73248c527b0d0d7d203a07f9d5a9b0884c062b93e39dfa7";
const WEIGHTS_BYTES: usize = 15_524_504;

/// The folder inside [`RAWNIND`]'s that the converted model is made in.
pub const FOLDER: &str = "neural-engine";

/// What the conversion changed, after [`RAWNIND`]'s credit, written beside the converted
/// model (CC BY 4.0 asks that changes be indicated). The credit says the ONNX model's
/// weights are unchanged; this says how the copy in this folder differs from it.
pub const CHANGES: &str = "The model in this folder is changed from that ONNX model: \
RAWmakase made it on this computer from model_bayer.onnx, for Apple's Neural Engine, as a \
half-precision Core ML model with the program and scales of \
scripts/models/convert_denoise_ane.py. Each convolution's weights and bias are multiplied by \
powers of two so its values fit half precision, and the weights are then rounded to half \
precision.";

/// How long making, compiling and loading the model for the Neural Engine may take. The
/// first load makes the weights and compiles the network twice (the package for this
/// Mac, then the network for the Neural Engine): a few seconds. A load that outlasts
/// this is taken for Apple's compiler service being stuck.
const LOAD_LIMIT: Duration = Duration::from_secs(60);

/// Set once a load has outlasted [`LOAD_LIMIT`]: the compiler service stays stuck until
/// the computer restarts, so later loads fail at once.
static STALLED: AtomicBool = AtomicBool::new(false);

/// Whether a load for the Neural Engine has outlasted its limit in this process.
pub fn stalled() -> bool {
    STALLED.load(Ordering::Relaxed)
}

/// Whether this computer has a Neural Engine Core ML can run the model on: Apple
/// Silicon, on macOS 14 or later (iOS and iPadOS 17), which the converted program needs
/// (Core ML's specification 8).
/// Asked of the system once per process: it is asked per request, and per frame while
/// Preferences shows the choice.
pub fn supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| cfg!(target_arch = "aarch64") && core_ml::os_at_least(14, 17))
}

/// Where the converted model is made, for [`RAWNIND`] installed in `dir`.
pub fn folder(dir: &Path) -> PathBuf {
    dir.join(FOLDER)
}

/// Names the converted model by its program and its weights: what its compiled copy and
/// its results' cache key are named for, so a model converted again never stands in for
/// the one before.
/// Hashed once per process: every result's key is named with it.
pub fn tag() -> &'static str {
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| format!("{}{}", CONVERTED.program_digest(6), &WEIGHTS_SHA256[..12]))
}

/// AI Denoise's network on the Neural Engine. One tile at a time.
pub struct NeuralDenoiser {
    model: Model,
    input_scale: f32,
    output_scale: f32,
}

impl NeuralDenoiser {
    /// The converted model for [`RAWNIND`] installed in `dir`, on the Neural Engine: made
    /// and compiled on first use, refused when the installed model doesn't make the
    /// weights it was converted with.
    pub fn load(dir: &Path) -> Result<Self, InferenceError> {
        let onnx = dir.join(RAWNIND.model.name);
        let dir = folder(dir);
        let model = core_ml::within(LOAD_LIMIT, &STALLED, "the Neural Engine", move || {
            core_ml::load_converted(
                &CONVERTED,
                &dir,
                tag(),
                Units::CpuAndNeuralEngine,
                None,
                || made(&onnx),
            )
        })?;
        let source = model.metadata("source_sha256")?;
        if source != RAWNIND.model.sha256 {
            return Err(InferenceError::ModelInvalid(format!(
                "the Neural Engine's model was made from {source}, not {}",
                RAWNIND.model.sha256
            )));
        }
        let scale = |key: &str| -> Result<f32, InferenceError> {
            let v: f32 = model
                .metadata(key)?
                .parse()
                .map_err(|_| InferenceError::ModelInvalid(format!("{key} is not a number")))?;
            (v.is_finite() && v > 0.)
                .then_some(v)
                .ok_or_else(|| InferenceError::ModelInvalid(format!("{key} is {v}")))
        };
        Ok(Self {
            input_scale: scale("input_scale")?,
            output_scale: scale("output_scale")?,
            model,
        })
    }
}

impl TileModel for NeuralDenoiser {
    fn run(&mut self, input: &[f32], cancel: Cancel<'_>) -> Result<Vec<f32>, InferenceError> {
        if cancel.is_cancelled() {
            return Err(InferenceError::Cancelled);
        }
        let side = RAWNIND.tile;
        if input.len() != 4 * side * side {
            return Err(InferenceError::OutputInvalid(format!(
                "a tile of {} values, expected {}",
                input.len(),
                4 * side * side
            )));
        }
        let scale = self.input_scale;
        let out = self
            .model
            .predict(
                RAWNIND.input,
                &[1, 4, side, side],
                |data| {
                    for (d, v) in data.iter_mut().zip(input) {
                        *d = v * scale;
                    }
                },
                &[(
                    RAWNIND.output,
                    &[1, 3, 2 * side, 2 * side],
                    1. / self.output_scale,
                )],
            )?
            .pop()
            .expect("one output asked for");
        // A value beyond half precision comes back as infinity: such a tile is not a
        // result, and the caller redoes the job elsewhere.
        if !out.iter().all(|v| v.is_finite()) {
            return Err(InferenceError::OutputInvalid(
                "the Neural Engine's tile is not finite".into(),
            ));
        }
        Ok(out)
    }
}

/// The weights file and the notice of the converted model, made from the ONNX model at
/// `onnx`, which must be [`RAWNIND`]'s.
fn made(onnx: &Path) -> Result<(Vec<u8>, String), InferenceError> {
    let source = std::fs::read(onnx)
        .map_err(|e| InferenceError::Failed(format!("AI Denoise's model: {e}")))?;
    // Only RAWmakase's own model is read: a damaged or replaced file is refused before a
    // byte of it is parsed.
    let digest = core_ml::sha256_hex(&source);
    if digest != RAWNIND.model.sha256 {
        return Err(InferenceError::ModelInvalid(format!(
            "AI Denoise's model is not the one RAWmakase installs ({digest})"
        )));
    }
    let weights = weights(&source)?;
    Ok((weights, format!("{}\n\n{CHANGES}\n", RAWNIND.attribution)))
}

/// The converted model's weights file, made from `onnx`, the bytes of [`RAWNIND`]'s ONNX
/// model: each tensor [`SCALES`] names, multiplied by its powers of two (exact in single
/// precision), rounded to half precision, in Core ML's blob file (a header of the blob
/// count and version 2, then for each blob 64 bytes of metadata and its data, each
/// metadata at a multiple of 64). Refused unless it is the file the conversion wrote
/// ([`WEIGHTS_SHA256`]): another ONNX model, or a mistake here, never reaches Core ML.
pub fn weights(onnx: &[u8]) -> Result<Vec<u8>, InferenceError> {
    let out = weights_with(onnx, SCALES)?;
    let digest = core_ml::sha256_hex(&out);
    if digest != WEIGHTS_SHA256 {
        return Err(InferenceError::ModelInvalid(format!(
            "the installed AI Denoise model doesn't make the Neural Engine's weights \
             ({digest})"
        )));
    }
    Ok(out)
}

/// [`weights`] with the scales `scales`, unchecked.
fn weights_with(onnx: &[u8], scales: &str) -> Result<Vec<u8>, InferenceError> {
    let tensors = onnx::initializers(onnx)?;
    let invalid = |why: String| InferenceError::ModelInvalid(why);
    let mut blobs = Vec::new();
    for line in scales.lines().filter(|l| !l.trim().is_empty()) {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(axis)) = (parts.next(), parts.next()) else {
            return Err(invalid(format!("the scales' line `{line}` is malformed")));
        };
        let tensor = tensors
            .get(name)
            .ok_or_else(|| invalid(format!("AI Denoise's model has no tensor {name}")))?;
        let count: usize = tensor.dims.iter().product();
        if tensor.half || tensor.data.len() != count * 4 {
            return Err(invalid(format!("{name} holds the wrong number of bytes")));
        }
        let mut exps = Vec::new();
        for run in parts {
            let (exp, n) = run.split_once('*').unwrap_or((run, "1"));
            let (Ok(exp), Ok(n)) = (exp.parse::<i32>(), n.parse::<usize>()) else {
                return Err(invalid(format!("the scales' line `{line}` is malformed")));
            };
            exps.extend(std::iter::repeat_n(exp, n));
        }
        // The input channel each value belongs to: one for a bias.
        let (len, stride) = if axis == "-" {
            (1, count.max(1))
        } else {
            let axis: usize = axis
                .parse()
                .ok()
                .filter(|a| *a < tensor.dims.len())
                .ok_or_else(|| invalid(format!("the scales' line `{line}` is malformed")))?;
            (tensor.dims[axis], tensor.dims[axis + 1..].iter().product())
        };
        if exps.len() != len || stride == 0 {
            return Err(invalid(format!("{name}'s scales don't fit its shape")));
        }
        let mut blob = Vec::with_capacity(count * 2);
        for (i, value) in tensor.data.as_chunks::<4>().0.iter().enumerate() {
            let value = f32::from_le_bytes(*value);
            let scaled = value * 2f32.powi(exps[(i / stride) % len]);
            blob.extend(half::f16::from_f32(scaled).to_le_bytes());
        }
        blobs.push(blob);
    }
    Ok(core_ml::blob_file(
        &blobs,
        if scales == SCALES { WEIGHTS_BYTES } else { 64 },
    ))
}

#[cfg(test)]
mod tests;
