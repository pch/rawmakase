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
//! on ([`tidy`]).
use crate::denoise::TileModel;
use crate::{InferenceError, RAWNIND};
use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_core_ml::{
    MLComputeUnits, MLDictionaryFeatureProvider, MLFeatureProvider, MLFeatureValue, MLModel,
    MLModelConfiguration, MLModelCreatorDefinedKey, MLMultiArray, MLMultiArrayDataType,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSNumber, NSOperatingSystemVersion, NSProcessInfo, NSString,
    NSURL,
};
use rawmakase_cancel::Cancel;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// The package's `Manifest.json`, as the conversion wrote it.
const MANIFEST: &[u8] = include_bytes!("../models/neural-engine/Manifest.json");
/// The ML Program: the network's operations, the scales in its metadata, and where in
/// the weights file each tensor is. No weights.
const PROGRAM: &[u8] = include_bytes!("../models/neural-engine/model.mlmodel");
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
/// model (CC BY 4.0 asks that changes be indicated).
pub const CHANGES: &str = "Changes: converted for Apple's Neural Engine to a half-precision \
Core ML model, made on this computer from model_bayer.onnx with the program and scales of \
scripts/models/convert_denoise_ane.py: each convolution's weights and bias multiplied by powers \
of two so its values fit half precision, the weights then rounded to half precision.";

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
pub fn supported() -> bool {
    cfg!(target_arch = "aarch64")
        && NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
            majorVersion: if cfg!(target_os = "macos") { 14 } else { 17 },
            minorVersion: 0,
            patchVersion: 0,
        })
}

/// Where the converted model is made, for [`RAWNIND`] installed in `dir`.
pub fn folder(dir: &Path) -> PathBuf {
    dir.join(FOLDER)
}

/// Names the converted model by its program and its weights: what its compiled copy and
/// its results' cache key are named for, so a model converted again never stands in for
/// the one before.
pub fn tag() -> String {
    let program: String = Sha256::digest(PROGRAM)
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{program}{}", &WEIGHTS_SHA256[..12])
}

/// AI Denoise's network on the Neural Engine. One tile at a time.
pub struct NeuralDenoiser {
    model: Retained<MLModel>,
    input_scale: f32,
    output_scale: f32,
}

// SAFETY: Core ML's objects may be used from any thread. A `NeuralDenoiser` is used by
// one thread at a time (`run` takes `&mut self`); `Send` only lets it move to the
// thread that runs it.
unsafe impl Send for NeuralDenoiser {}

/// A model loaded on the load's own thread, handed to the thread that asked for it.
struct Loaded(Retained<MLModel>);
// SAFETY: as for `NeuralDenoiser`: the model moves once, and is used by one thread.
unsafe impl Send for Loaded {}

impl NeuralDenoiser {
    /// The converted model for [`RAWNIND`] installed in `dir`, on the Neural Engine: made
    /// and compiled on first use, refused when the installed model doesn't make the
    /// weights it was converted with.
    pub fn load(dir: &Path) -> Result<Self, InferenceError> {
        let onnx = dir.join(RAWNIND.model.name);
        let dir = folder(dir);
        let Loaded(model) = within(LOAD_LIMIT, &STALLED, move || {
            let target = dir.join(format!("neural-engine-{}.mlmodelc", tag()));
            let kept = target.join("coremldata.bin").is_file();
            let units = MLComputeUnits::CPUAndNeuralEngine;
            match load(&compiled(&onnx, &dir, &target)?, units) {
                Ok(model) => Ok(Loaded(model)),
                // A compiled model kept from an earlier load that won't load now is made
                // again, once: a crash while it was written, or damage since.
                Err(_) if kept => {
                    let _ = std::fs::remove_dir_all(&target);
                    load(&compiled(&onnx, &dir, &target)?, units).map(Loaded)
                }
                Err(e) => Err(e),
            }
        })?;
        let source = metadata(&model, "source_sha256")?;
        if source != RAWNIND.model.sha256 {
            return Err(InferenceError::ModelInvalid(format!(
                "the Neural Engine's model was made from {source}, not {}",
                RAWNIND.model.sha256
            )));
        }
        let scale = |key: &str| -> Result<f32, InferenceError> {
            let v: f32 = metadata(&model, key)?
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
        let out = predict(
            &self.model,
            RAWNIND.input,
            &[1, 4, side, side],
            |data| {
                for (d, v) in data.iter_mut().zip(input) {
                    *d = v * scale;
                }
            },
            RAWNIND.output,
            &[1, 3, 2 * side, 2 * side],
            1. / self.output_scale,
        )?;
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

/// The compiled model at `target` in `dir`, made from the ONNX model at `onnx` when it is
/// not yet: the package written in a work folder (the embedded program and manifest, the
/// weights from [`weights`], checked against [`WEIGHTS_SHA256`]), compiled, and moved into
/// place. `target` is named by [`tag`], so a model converted again is compiled anew.
fn compiled(onnx: &Path, dir: &Path, target: &Path) -> Result<PathBuf, InferenceError> {
    let target = target.to_path_buf();
    if target.join("coremldata.bin").is_file() {
        return Ok(target);
    }
    let io = |what: &str, e: std::io::Error| InferenceError::Failed(format!("{what}: {e}"));
    let source = std::fs::read(onnx).map_err(|e| io("AI Denoise's model", e))?;
    // Only RAWmakase's own model is read: a damaged or replaced file is refused before a
    // byte of it is parsed.
    let digest: String = Sha256::digest(&source)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if digest != RAWNIND.model.sha256 {
        return Err(InferenceError::ModelInvalid(format!(
            "AI Denoise's model is not the one RAWmakase installs ({digest})"
        )));
    }
    let weights = weights(&source)?;
    drop(source);
    std::fs::create_dir_all(dir).map_err(|e| io("the Neural Engine's folder", e))?;
    tidy(dir, &target);
    std::fs::write(
        dir.join("NOTICE"),
        format!("{}\n{CHANGES}\n", RAWNIND.attribution),
    )
    .map_err(|e| io("the Neural Engine model's notice", e))?;
    let work = tempfile_in(dir).map_err(|e| io("a folder to compile in", e))?;
    let package = work.join("model.mlpackage");
    let data = package.join("Data").join("com.apple.CoreML");
    let written = std::fs::create_dir_all(data.join("weights"))
        .and_then(|()| std::fs::write(package.join("Manifest.json"), MANIFEST))
        .and_then(|()| std::fs::write(data.join("model.mlmodel"), PROGRAM))
        .and_then(|()| std::fs::write(data.join("weights").join("weight.bin"), &weights))
        .map_err(|e| io("the package", e));
    drop(weights);
    let result = written.and_then(|()| compile(&package)).and_then(|from| {
        // Into place in one step: a load running at the same time sees all of it or
        // none. Another load may have put it there first; its copy is as good.
        let staged = work.join("compiled.mlmodelc");
        let copied = copy_dir(&from, &staged).map_err(|e| io("the compiled model", e));
        let _ = std::fs::remove_dir_all(&from);
        copied?;
        match std::fs::rename(&staged, &target) {
            Ok(()) => Ok(target.clone()),
            Err(_) if target.join("coremldata.bin").is_file() => Ok(target.clone()),
            Err(e) => Err(io("the compiled model", e)),
        }
    });
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// The converted model's weights file, made from `onnx`, the bytes of [`RAWNIND`]'s ONNX
/// model: each tensor [`SCALES`] names, multiplied by its powers of two (exact in single
/// precision), rounded to half precision, in Core ML's blob file (a header of the blob
/// count and version 2, then for each blob 64 bytes of metadata and its data, each
/// metadata at a multiple of 64). Refused unless it is the file the conversion wrote
/// ([`WEIGHTS_SHA256`]): another ONNX model, or a mistake here, never reaches Core ML.
pub fn weights(onnx: &[u8]) -> Result<Vec<u8>, InferenceError> {
    let out = weights_with(onnx, SCALES)?;
    let digest: String = Sha256::digest(&out)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
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
    let tensors = initializers(onnx)?;
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
        if tensor.data.len() != count * 4 {
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
    let mut out = Vec::with_capacity(if scales == SCALES { WEIGHTS_BYTES } else { 64 });
    out.extend((blobs.len() as u32).to_le_bytes());
    out.extend(2u32.to_le_bytes());
    out.resize(64, 0);
    let last = blobs.len().saturating_sub(1);
    for (i, blob) in blobs.iter().enumerate() {
        let meta = out.len();
        out.extend(0xDEAD_BEEFu32.to_le_bytes());
        // Core ML's code for half precision.
        out.extend(1u32.to_le_bytes());
        out.extend((blob.len() as u64).to_le_bytes());
        out.extend(((meta + 64) as u64).to_le_bytes());
        out.resize(meta + 64, 0);
        out.extend(blob);
        if i < last {
            out.resize(out.len().div_ceil(64) * 64, 0);
        }
    }
    Ok(out)
}

/// A float tensor of an ONNX model: its shape and its little-endian `f32` bytes.
struct Tensor<'a> {
    dims: Vec<usize>,
    data: &'a [u8],
}

/// `ModelProto.graph`, `GraphProto.initializer`, and the `TensorProto` fields read.
const MODEL_GRAPH: u64 = 7;
const GRAPH_INITIALIZER: u64 = 5;
const TENSOR_DIMS: u64 = 1;
const TENSOR_DATA_TYPE: u64 = 2;
const TENSOR_NAME: u64 = 8;
const TENSOR_RAW_DATA: u64 = 9;
const TENSOR_DATA_LOCATION: u64 = 14;
/// `TensorProto.DataType.FLOAT`.
const FLOAT: u64 = 1;
const VARINT: u64 = 0;
const FIXED64: u64 = 1;
const DELIMITED: u64 = 2;
const FIXED32: u64 = 5;

/// The float tensors stored in the ONNX model `onnx` (a serialized `ModelProto`) with
/// their data inline, by name. Only those fields are read; nothing is copied.
fn initializers(onnx: &[u8]) -> Result<HashMap<&str, Tensor<'_>>, InferenceError> {
    let mut out = HashMap::new();
    for graph in fields(onnx)?
        .into_iter()
        .filter(|f| f.number == MODEL_GRAPH && f.wire == DELIMITED)
    {
        for init in fields(graph.body)?
            .into_iter()
            .filter(|f| f.number == GRAPH_INITIALIZER && f.wire == DELIMITED)
        {
            let (mut dims, mut float, mut name, mut data, mut external) =
                (Vec::new(), false, None, None, false);
            for field in fields(init.body)? {
                match (field.number, field.wire) {
                    (TENSOR_DIMS, VARINT) => dims.push(usize::try_from(field.value).unwrap_or(0)),
                    (TENSOR_DIMS, DELIMITED) => {
                        let mut at = 0;
                        while at < field.body.len() {
                            let d = read_varint(field.body, &mut at)?;
                            dims.push(usize::try_from(d).unwrap_or(0));
                        }
                    }
                    (TENSOR_DATA_TYPE, VARINT) => float = field.value == FLOAT,
                    (TENSOR_NAME, DELIMITED) => name = std::str::from_utf8(field.body).ok(),
                    (TENSOR_RAW_DATA, DELIMITED) => data = Some(field.body),
                    (TENSOR_DATA_LOCATION, VARINT) => external = field.value != 0,
                    _ => {}
                }
            }
            if let (Some(name), Some(data), true, false) = (name, data, float, external) {
                out.insert(name, Tensor { dims, data });
            }
        }
    }
    Ok(out)
}

/// One field of a protobuf message: its number, wire type, its value for a varint and
/// its bytes for a length-delimited field.
struct Field<'a> {
    number: u64,
    wire: u64,
    value: u64,
    body: &'a [u8],
}

/// The top-level fields of a message, checked to fill it exactly.
fn fields(message: &[u8]) -> Result<Vec<Field<'_>>, InferenceError> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < message.len() {
        let tag = read_varint(message, &mut at)?;
        let (number, wire) = (tag >> 3, tag & 7);
        let (mut value, len) = (
            0,
            match wire {
                VARINT => 0,
                FIXED64 => 8,
                FIXED32 => 4,
                DELIMITED => usize::try_from(read_varint(message, &mut at)?)
                    .map_err(|_| unreadable("a field is too long"))?,
                _ => return Err(unreadable("a field has an unsupported wire type")),
            },
        );
        if wire == VARINT {
            value = read_varint(message, &mut at)?;
        }
        let end = at
            .checked_add(len)
            .filter(|end| *end <= message.len())
            .ok_or_else(|| unreadable("it ends inside a field"))?;
        out.push(Field {
            number,
            wire,
            value,
            body: &message[at..end],
        });
        at = end;
    }
    Ok(out)
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Result<u64, InferenceError> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes
            .get(*at)
            .ok_or_else(|| unreadable("it ends inside a number"))?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(unreadable("a number is too long"))
}

fn unreadable(why: &str) -> InferenceError {
    InferenceError::ModelInvalid(format!("AI Denoise's model cannot be read: {why}"))
}

/// How old a work folder of another process must be before it is taken for one a load
/// left behind: longer than any load may take ([`LOAD_LIMIT`]).
const ABANDONED: Duration = Duration::from_secs(3600);

/// Removes from `dir` the compiled models of other versions of the model than `keep`'s,
/// and work folders an earlier process left there (its load given up or the process
/// ended). A work folder of a load still running, in this or another process, is
/// younger than [`ABANDONED`] and is left.
fn tidy(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let ours = format!(".neural-engine-{}-", std::process::id());
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let old_compiled =
            name.starts_with("neural-engine-") && name.ends_with(".mlmodelc") && path != keep;
        let abandoned = name.starts_with(".neural-engine-")
            && !name.starts_with(&ours)
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > ABANDONED));
        if old_compiled || abandoned {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// A new hidden folder in `dir`.
fn tempfile_in(dir: &Path) -> std::io::Result<PathBuf> {
    for n in 0..1000u32 {
        let path = dir.join(format!(".neural-engine-{}-{n}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free folder name"))
}

/// Copies the folder `from` into a new folder `to`.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Compiles the package at `package` for this computer, into a temporary folder Core
/// ML chooses.
// The synchronous call is deprecated for one taking a completion handler; it runs on
// the load's own thread (see `within`), which waits for it either way.
#[allow(deprecated)]
fn compile(package: &Path) -> Result<PathBuf, InferenceError> {
    autoreleasepool(|_| {
        let url = NSURL::from_file_path(package)
            .ok_or_else(|| InferenceError::Failed("the package's path is not a URL".into()))?;
        let compiled = unsafe { MLModel::compileModelAtURL_error(&url) }.map_err(|e| failed(&e))?;
        compiled
            .to_file_path()
            .ok_or_else(|| InferenceError::Failed("the compiled model is not a file".into()))
    })
}

/// The compiled model at `path` on `units`.
fn load(path: &Path, units: MLComputeUnits) -> Result<Retained<MLModel>, InferenceError> {
    autoreleasepool(|_| {
        let url = NSURL::from_file_path(path)
            .ok_or_else(|| InferenceError::Failed("the model's path is not a URL".into()))?;
        let config = unsafe { MLModelConfiguration::new() };
        unsafe { config.setComputeUnits(units) };
        unsafe { MLModel::modelWithContentsOfURL_configuration_error(&url, &config) }
            .map_err(|e| failed(&e))
    })
}

/// A value the model's maker stored in its metadata.
fn metadata(model: &MLModel, key: &str) -> Result<String, InferenceError> {
    autoreleasepool(|_| {
        let all = unsafe { model.modelDescription().metadata() };
        let creator = unsafe { MLModelCreatorDefinedKey }
            .and_then(|k| all.objectForKey(k))
            .and_then(|d| d.downcast::<NSDictionary>().ok())
            .ok_or_else(|| {
                InferenceError::ModelInvalid("the model has no maker's metadata".into())
            })?;
        creator
            .objectForKey(&NSString::from_str(key))
            .and_then(|v| v.downcast::<NSString>().ok())
            .map(|v| v.to_string())
            .ok_or_else(|| {
                InferenceError::ModelInvalid(format!("the model's metadata has no {key}"))
            })
    })
}

/// Runs `model` on one `f32` input of `shape`, written by `fill`, and answers its
/// `output` (which must be `expected` in shape) as contiguous `f32`s, each multiplied
/// by `scale`.
// `dataPointer` is deprecated for `getMutableBytesWithHandler`, which lends the bytes to
// a block. The arrays here are made or answered for this call alone and read at once, on
// this thread, so the pointer is as safe and simpler.
#[allow(deprecated)]
fn predict(
    model: &MLModel,
    input: &str,
    shape: &[usize],
    fill: impl FnOnce(&mut [f32]),
    output: &str,
    expected: &[usize],
    scale: f32,
) -> Result<Vec<f32>, InferenceError> {
    autoreleasepool(|_| {
        let numbers: Vec<Retained<NSNumber>> =
            shape.iter().map(|&n| NSNumber::new_usize(n)).collect();
        let array = unsafe {
            MLMultiArray::initWithShape_dataType_error(
                MLMultiArray::alloc(),
                &NSArray::from_retained_slice(&numbers),
                MLMultiArrayDataType::Float32,
            )
        }
        .map_err(|e| failed(&e))?;
        let count = shape.iter().product();
        let values = unsafe { array.dataPointer() }.cast::<f32>().as_ptr();
        // SAFETY: a new array of this shape is contiguous `f32`s, as many as the shape
        // holds, which Core ML leaves uninitialized: they are zeroed before they are seen
        // as a slice.
        fill(unsafe {
            std::ptr::write_bytes(values, 0, count);
            std::slice::from_raw_parts_mut(values, count)
        });
        let value = unsafe { MLFeatureValue::featureValueWithMultiArray(&array) };
        let key = NSString::from_str(input);
        let features =
            NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[value.as_ref()]);
        let provider = unsafe {
            MLDictionaryFeatureProvider::initWithDictionary_error(
                MLDictionaryFeatureProvider::alloc(),
                &features,
            )
        }
        .map_err(|e| failed(&e))?;
        let result =
            unsafe { model.predictionFromFeatures_error(ProtocolObject::from_ref(&*provider)) }
                .map_err(|e| failed(&e))?;
        let answer = unsafe { result.featureValueForName(&NSString::from_str(output)) }
            .and_then(|v| unsafe { v.multiArrayValue() })
            .ok_or_else(|| {
                InferenceError::ModelInvalid(format!("the model has no output {output}"))
            })?;
        read(&answer, expected, scale)
    })
}

/// `array`, which must be `expected` in shape, as contiguous `f32`s times `scale`. The
/// Neural Engine answers in half precision and may pad rows: read by the strides.
#[allow(deprecated)] // `dataPointer`: see `predict`.
fn read(array: &MLMultiArray, expected: &[usize], scale: f32) -> Result<Vec<f32>, InferenceError> {
    let numbers = |list: Retained<NSArray<NSNumber>>| -> Vec<usize> {
        list.iter().map(|n| n.as_usize()).collect()
    };
    let shape = numbers(unsafe { array.shape() });
    if shape != expected || shape.contains(&0) {
        return Err(InferenceError::OutputInvalid(format!(
            "the model answered with shape {shape:?}, expected {expected:?}"
        )));
    }
    let strides = numbers(unsafe { array.strides() });
    if strides.len() != shape.len() {
        return Err(InferenceError::OutputInvalid(format!(
            "the model answered {} strides for {} dimensions",
            strides.len(),
            shape.len()
        )));
    }
    // The last value's offset, plus one: how far the strides reach.
    let len = shape
        .iter()
        .zip(&strides)
        .map(|(n, s)| (n - 1) * s)
        .sum::<usize>()
        + 1;
    let (inner, inner_stride) = (shape[shape.len() - 1], strides[strides.len() - 1]);
    let rows = shape[..shape.len() - 1].iter().product::<usize>();
    // Each row's offset: its index written out over the outer dimensions.
    let row_offset = |mut row: usize| -> usize {
        let mut offset = 0;
        for d in (0..shape.len() - 1).rev() {
            offset += (row % shape[d]) * strides[d];
            row /= shape[d];
        }
        offset
    };
    let base = unsafe { array.dataPointer() };
    let mut out = Vec::with_capacity(rows * inner);
    match unsafe { array.dataType() } {
        MLMultiArrayDataType::Float16 => {
            // SAFETY: the array holds `len` half-precision values by its own strides.
            let data =
                unsafe { std::slice::from_raw_parts(base.cast::<half::f16>().as_ptr(), len) };
            for row in 0..rows {
                let start = row_offset(row);
                out.extend((0..inner).map(|x| data[start + x * inner_stride].to_f32() * scale));
            }
        }
        MLMultiArrayDataType::Float32 => {
            // SAFETY: as above, single precision.
            let data = unsafe { std::slice::from_raw_parts(base.cast::<f32>().as_ptr(), len) };
            for row in 0..rows {
                let start = row_offset(row);
                out.extend((0..inner).map(|x| data[start + x * inner_stride] * scale));
            }
        }
        other => {
            return Err(InferenceError::OutputInvalid(format!(
                "the model answered in data type {}",
                other.0
            )));
        }
    }
    Ok(out)
}

/// `load` on a thread of its own, given up after `limit`: Core ML's load cannot be
/// cancelled, so a load that never returns keeps its thread, and `stalled` is set so
/// that later calls fail at once.
fn within<T: Send + 'static>(
    limit: Duration,
    stalled: &'static AtomicBool,
    load: impl FnOnce() -> Result<T, InferenceError> + Send + 'static,
) -> Result<T, InferenceError> {
    if stalled.load(Ordering::Relaxed) {
        return Err(InferenceError::Failed(
            "an earlier load for the Neural Engine never finished".into(),
        ));
    }
    let (send, answer) = mpsc::channel();
    std::thread::Builder::new()
        .name("neural-engine-load".into())
        .spawn(move || {
            // The receiver is gone when the load came too late; its model is dropped.
            let _ = send.send(load());
        })
        .map_err(|e| InferenceError::Failed(format!("no thread for the model's load: {e}")))?;
    match answer.recv_timeout(limit) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            stalled.store(true, Ordering::Relaxed);
            Err(InferenceError::Failed(format!(
                "the load for the Neural Engine took over {} s",
                limit.as_secs()
            )))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(InferenceError::Failed(
            "the model's load ended without an answer".into(),
        )),
    }
}

fn failed(e: &NSError) -> InferenceError {
    InferenceError::Failed(e.localizedDescription().to_string())
}

#[cfg(test)]
mod tests;
