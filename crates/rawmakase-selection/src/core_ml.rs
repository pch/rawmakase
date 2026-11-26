//! Select Sky on Apple Silicon's GPU: [`SKY`]'s network converted to a half-precision
//! Core ML model, run by Core ML directly. ONNX Runtime's CoreML provider can't do this:
//! it fails to build the network for the GPU, and on the Neural Engine splits it into
//! pieces that run no faster than the CPU.
//!
//! Nothing of the converted model is downloaded. The repository holds what has no
//! weights (`models/core-ml`: the package's `Manifest.json`, the ML Program
//! `model.mlmodel`, and `tensors.txt`, which of [`SKY`]'s tensors each weight is); the
//! weights are made on the computer that runs them, from [`SKY`]'s installed model
//! ([`weights`]), and checked against [`WEIGHTS_SHA256`]. The first load writes the
//! package in a folder of its own inside the model's ([`FOLDER`]), compiles it for this
//! Mac, and keeps the compiled model there
//! ([`rawmakase_inference::core_ml::load_converted`]).
//!
//! Core ML can't take UperNet at a size known only when it runs (its pyramid pooling
//! and its upsampling need it), so the program has one function per input size
//! ([`functions`]): the long edge at [`SKY`]'s, and the short one for 3:2, 4:3 and 16:9
//! photos with one pixel either side, both ways round, and square ones. The functions
//! share one copy of the weights. A photo of another shape is selected on the CPU.
//! Functions need macOS 15 ([`supported`]).
use crate::manifest::SKY;
use rawmakase_inference::InferenceError;
use rawmakase_inference::core_ml::{self, Converted, Model, Units, onnx};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const CONVERTED: Converted = Converted {
    prefix: FOLDER,
    manifest: include_bytes!("../models/core-ml/Manifest.json"),
    program: include_bytes!("../models/core-ml/model.mlmodel"),
};
/// Each weight the program reads, in the weights file's order: its name in [`SKY`]'s ONNX
/// model, and `T` when it is transposed (a linear layer, which the export wrote as a
/// MatMul) or `-` when it is as it is.
const TENSORS: &str = include_str!("../models/core-ml/tensors.txt");
/// The weights file [`weights`] makes from [`SKY`]'s model, as the conversion wrote it.
pub const WEIGHTS_SHA256: &str = "7d5dad0c7d501185224bf7ccf17e525ec8cb941183b4fd557ad89257c352a9f4";
const WEIGHTS_BYTES: usize = 241_780_204;

/// ADE20K's classes, which the class scores are of.
const CLASSES: usize = 150;

/// The folder inside [`SKY`]'s that the converted model is made in.
pub const FOLDER: &str = "core-ml";

/// The short sides the program has functions for, with the long side at
/// [`SKY`]`.long_edge`: 3:2, 4:3, 16:9 and 1:1, each with one pixel either side.
const SHORT_EDGES: [usize; 4] = [683, 768, 576, 1024];

/// The input sizes (width, height) the program has a function for, as the conversion
/// (`scripts/models/convert_sky_coreml.py`) made them.
pub fn functions() -> &'static [(usize, usize)] {
    static FUNCTIONS: OnceLock<Vec<(usize, usize)>> = OnceLock::new();
    FUNCTIONS.get_or_init(|| {
        let long = SKY.long_edge;
        let mut found = Vec::new();
        for short in SHORT_EDGES {
            for side in [short - 1, short, short + 1] {
                if side > long {
                    continue;
                }
                for shape in [(long, side), (side, long)] {
                    if !found.contains(&shape) {
                        found.push(shape);
                    }
                }
            }
        }
        found
    })
}

/// What the conversion changed, written beside the converted model after the source's
/// credit.
pub const CHANGES: &str = "The model in this folder is changed from that ONNX model: \
RAWmakase made it on this computer from upernet-convnext-base-sky-fp16.onnx, as a \
half-precision Core ML model with the program of scripts/models/convert_sky_coreml.py. Its \
weights are the ONNX model's, in half precision; the linear layers' are transposed.";

/// How long making, compiling and loading the model may take: the first load reads the
/// ONNX model, makes the weights and compiles the network, a few seconds.
const LOAD_LIMIT: Duration = Duration::from_secs(60);

/// Set once a load has outlasted [`LOAD_LIMIT`]: later loads fail at once.
static STALLED: AtomicBool = AtomicBool::new(false);

/// Whether this computer can run the converted model: Apple Silicon, on macOS 15 or
/// later (iOS and iPadOS 18), which a program with several functions needs (Core ML's
/// specification 9). Asked of the system once per process.
pub fn supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| cfg!(target_arch = "aarch64") && core_ml::os_at_least(15, 18))
}

/// Where the converted model is made, for [`SKY`] installed in `dir`.
pub fn folder(dir: &Path) -> PathBuf {
    dir.join(FOLDER)
}

/// Names the converted model by its program and its weights: what its compiled copy is
/// named for, so a model converted again is compiled anew.
pub fn tag() -> &'static str {
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| format!("{}{}", CONVERTED.program_digest(6), &WEIGHTS_SHA256[..12]))
}

/// The network's two answers for one input.
pub struct Answer {
    /// The sky's probability at each pixel of the input.
    pub probability: Vec<f32>,
    /// The class scores' shape: one image, ADE20K's classes, and a grid a quarter of the
    /// input's sides.
    pub scores_shape: [usize; 4],
    pub scores: Vec<f32>,
}

/// Select Sky's network on the GPU, one input size's function loaded at a time.
pub struct CoreMlSky {
    dir: PathBuf,
    loaded: Option<((usize, usize), Model)>,
}

impl CoreMlSky {
    /// The converted model for [`SKY`] installed in `dir`. Nothing is loaded until a photo
    /// asks for a function ([`CoreMlSky::run`]).
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            loaded: None,
        }
    }

    /// Whether the program has a function for an input of `width` x `height`.
    pub fn fits(width: usize, height: usize) -> bool {
        functions().contains(&(width, height))
    }

    /// The network's [`Answer`] for `tensor`, an input of `width` x `height` normalized as
    /// [`SKY`] asks. The function for that size is made, compiled and loaded the first
    /// time; refused when the program has none ([`CoreMlSky::fits`]).
    pub fn run(
        &mut self,
        tensor: &[f32],
        width: usize,
        height: usize,
    ) -> Result<Answer, InferenceError> {
        if !Self::fits(width, height) {
            return Err(InferenceError::Failed(format!(
                "the converted sky model has no function for {width}x{height}"
            )));
        }
        if tensor.len() != 3 * width * height {
            return Err(InferenceError::OutputInvalid(format!(
                "an input of {} values, expected {}",
                tensor.len(),
                3 * width * height
            )));
        }
        if self
            .loaded
            .as_ref()
            .is_none_or(|(size, _)| *size != (width, height))
        {
            // One function at a time: each brings its own copy of the weights into memory.
            self.loaded = None;
            let model = self.load(width, height)?;
            self.loaded = Some(((width, height), model));
        }
        let (_, model) = self.loaded.as_mut().expect("loaded above");
        let scores_shape = [1, CLASSES, height / 4, width / 4];
        let mut answers = model.predict(
            "pixel_values",
            &[1, 3, height, width],
            |data| data.copy_from_slice(tensor),
            &[
                ("sky", &[1, height, width], 1.),
                ("logits", &scores_shape, 1.),
            ],
        )?;
        let scores = answers.pop().expect("two outputs asked for");
        let probability = answers.pop().expect("two outputs asked for");
        if !probability.iter().chain(&scores).all(|v| v.is_finite()) {
            return Err(InferenceError::OutputInvalid(
                "the converted sky model's answer is not finite".into(),
            ));
        }
        Ok(Answer {
            probability,
            scores_shape,
            scores,
        })
    }

    /// The function for `width` x `height`, checked to be made from [`SKY`]'s model.
    fn load(&self, width: usize, height: usize) -> Result<Model, InferenceError> {
        let onnx = self.dir.join(SKY.file.name);
        let dir = folder(&self.dir);
        let function = format!("sky_{width}x{height}");
        let model = core_ml::within(LOAD_LIMIT, &STALLED, "the GPU", move || {
            core_ml::load_converted(
                &CONVERTED,
                &dir,
                tag(),
                Units::CpuAndGpu,
                Some(&function),
                || made(&onnx),
            )
        })?;
        let source = model.metadata("source_sha256")?;
        if source != SKY.file.sha256 {
            return Err(InferenceError::ModelInvalid(format!(
                "the converted sky model was made from {source}, not {}",
                SKY.file.sha256
            )));
        }
        let listed = model.metadata("functions")?;
        let expected: Vec<String> = functions()
            .iter()
            .map(|(w, h)| format!("{w}x{h}"))
            .collect();
        if listed.split(',').ne(expected.iter().map(String::as_str)) {
            return Err(InferenceError::ModelInvalid(format!(
                "the converted sky model has the functions {listed}"
            )));
        }
        Ok(model)
    }
}

/// The weights file and the notice of the converted model, made from the ONNX model at
/// `onnx`, which must be [`SKY`]'s.
fn made(onnx: &Path) -> Result<(Vec<u8>, String), InferenceError> {
    let source =
        std::fs::read(onnx).map_err(|e| InferenceError::Failed(format!("the sky model: {e}")))?;
    // Only RAWmakase's own model is read: a damaged or replaced file is refused before a
    // byte of it is parsed.
    let digest = core_ml::sha256_hex(&source);
    if digest != SKY.file.sha256 {
        return Err(InferenceError::ModelInvalid(format!(
            "the sky model is not the one RAWmakase installs ({digest})"
        )));
    }
    let weights = weights(&source)?;
    Ok((
        weights,
        format!("{}\n{}\n\n{CHANGES}\n", SKY.attribution, SKY.license),
    ))
}

/// The converted model's weights file, made from `onnx`, the bytes of [`SKY`]'s ONNX
/// model: each tensor [`TENSORS`] names, in half precision (exact for those the model
/// stores so; the few small ones it stores in single precision are rounded), transposed
/// where marked, in Core ML's blob file. Refused unless it is the file the conversion
/// wrote ([`WEIGHTS_SHA256`]): another ONNX model, or a mistake here, never reaches
/// Core ML.
pub fn weights(onnx: &[u8]) -> Result<Vec<u8>, InferenceError> {
    let out = weights_with(onnx, TENSORS)?;
    let digest = core_ml::sha256_hex(&out);
    if digest != WEIGHTS_SHA256 {
        return Err(InferenceError::ModelInvalid(format!(
            "the installed sky model doesn't make the converted model's weights ({digest})"
        )));
    }
    Ok(out)
}

/// [`weights`] with the tensors `tensors`, unchecked.
fn weights_with(onnx: &[u8], tensors: &str) -> Result<Vec<u8>, InferenceError> {
    let found = onnx::initializers(onnx)?;
    let invalid = |why: String| InferenceError::ModelInvalid(why);
    let mut blobs = Vec::new();
    for line in tensors.lines().filter(|l| !l.trim().is_empty()) {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(how), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(invalid(format!("the tensors' line `{line}` is malformed")));
        };
        let tensor = found
            .get(name)
            .ok_or_else(|| invalid(format!("the sky model has no tensor {name}")))?;
        let count: usize = tensor.dims.iter().product();
        let width = if tensor.half { 2 } else { 4 };
        if tensor.data.len() != count * width {
            return Err(invalid(format!("{name} holds the wrong number of bytes")));
        }
        let half: Vec<[u8; 2]> = if tensor.half {
            tensor.data.as_chunks::<2>().0.to_vec()
        } else {
            tensor
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| half::f16::from_f32(f32::from_le_bytes(*v)).to_le_bytes())
                .collect()
        };
        let blob: Vec<u8> = match how {
            "-" => half.concat(),
            "T" => {
                let [rows, columns] = tensor.dims[..] else {
                    return Err(invalid(format!("{name} is not a matrix to transpose")));
                };
                (0..columns)
                    .flat_map(|c| (0..rows).map(move |r| r * columns + c))
                    .flat_map(|i| half[i])
                    .collect()
            }
            _ => return Err(invalid(format!("the tensors' line `{line}` is malformed"))),
        };
        blobs.push(blob);
    }
    let capacity = if tensors == TENSORS {
        WEIGHTS_BYTES
    } else {
        64
    };
    Ok(core_ml::blob_file(&blobs, capacity))
}

#[cfg(test)]
mod tests;
