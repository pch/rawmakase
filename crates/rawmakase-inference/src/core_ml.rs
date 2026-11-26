//! Core ML, run directly, for models RAWmakase converted for it: AI Denoise's on the
//! Neural Engine ([`crate::neural_engine`]) and Select Sky's on the GPU
//! (`rawmakase-selection`'s `core_ml`). ONNX Runtime's CoreML provider can't run either
//! as well: it keeps AI Denoise in single precision, which the Neural Engine does not
//! compute, and splits Select Sky into pieces too small to gain anything.
//!
//! Nothing of a converted model is downloaded. The repository holds what has no weights
//! ([`Converted`]: the package's `Manifest.json` and the ML Program, `model.mlmodel`).
//! The caller makes the weights on the computer that runs them, from the installed ONNX
//! model ([`onnx::initializers`], [`blob_file`]), and checks them against the
//! conversion's digest. The first load writes the package in a work folder inside the
//! caller's folder, compiles it for this computer, and keeps the compiled model there,
//! named for the program and the weights ([`load_converted`]).
//!
//! Core ML's load can't be cancelled, and loading for the Neural Engine can, when Apple's
//! compiler service is stuck, never return: [`within`] gives a load up after a limit. A
//! load given up keeps its thread until it returns, if ever; its work folder is removed
//! by a later load an hour on (`tidy`).
pub mod onnx;

use crate::InferenceError;
use block2::StackBlock;
use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_core_ml::{
    MLComputeUnits, MLDictionaryFeatureProvider, MLFeatureProvider, MLFeatureValue, MLModel,
    MLModelConfiguration, MLModelCreatorDefinedKey, MLMultiArray, MLMultiArrayDataType,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSInteger, NSNumber, NSOperatingSystemVersion, NSProcessInfo,
    NSString, NSURL,
};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// What the repository holds of a converted model: everything but its weights.
pub struct Converted {
    /// Names its compiled models (`<prefix>-<tag>.mlmodelc`) and work folders
    /// (`.<prefix>-<process>-<n>`) in the folder it is made in.
    pub prefix: &'static str,
    /// The package's `Manifest.json`, as the conversion wrote it.
    pub manifest: &'static [u8],
    /// The ML Program: the network's operations, its metadata, and where in the weights
    /// file each tensor is.
    pub program: &'static [u8],
}

impl Converted {
    /// The first `n` bytes of the program's SHA-256, in hex: with the weights' digest,
    /// what a compiled copy is named for.
    pub fn program_digest(&self, n: usize) -> String {
        Sha256::digest(self.program)
            .iter()
            .take(n)
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Where Core ML may run a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Units {
    CpuAndNeuralEngine,
    CpuAndGpu,
}

impl Units {
    fn core_ml(self) -> MLComputeUnits {
        match self {
            Self::CpuAndNeuralEngine => MLComputeUnits::CPUAndNeuralEngine,
            Self::CpuAndGpu => MLComputeUnits::CPUAndGPU,
        }
    }
}

/// A loaded Core ML model.
pub struct Model(Retained<MLModel>);

// SAFETY: Core ML's objects may be used from any thread. A `Model` is used by one thread
// at a time (its callers hold it mutably or alone); `Send` only lets it move to the
// thread that runs it, or from the thread that loaded it.
unsafe impl Send for Model {}

impl Model {
    /// A value the model's maker stored in its metadata.
    pub fn metadata(&self, key: &str) -> Result<String, InferenceError> {
        metadata(&self.0, key)
    }

    /// Runs the model on one `f32` input named `input` of `shape`, written by `fill`, and
    /// answers each of `outputs` (a name, the shape it must be in, and a factor) as
    /// contiguous `f32`s, each multiplied by its factor.
    pub fn predict(
        &mut self,
        input: &str,
        shape: &[usize],
        fill: impl FnOnce(&mut [f32]),
        outputs: &[(&str, &[usize], f32)],
    ) -> Result<Vec<Vec<f32>>, InferenceError> {
        predict(&self.0, input, shape, fill, outputs)
    }
}

/// Whether this is macOS `macos` or later, or iOS and iPadOS `ios` or later elsewhere.
pub fn os_at_least(macos: isize, ios: isize) -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: if cfg!(target_os = "macos") {
            macos
        } else {
            ios
        },
        minorVersion: 0,
        patchVersion: 0,
    })
}

/// The SHA-256 of `bytes`, in hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `model` compiled in `dir` and loaded on `units` (its function `function`, when it has
/// several). The compiled copy is named for `tag`, so a model converted again is compiled
/// anew. When there is none yet, `make` answers the weights file and the notice written
/// beside the model, and the package is written in a work folder, compiled, and moved into
/// place. A compiled copy kept from an earlier load that won't load now is made again,
/// once: a crash while it was written, or damage since.
pub fn load_converted(
    model: &Converted,
    dir: &Path,
    tag: &str,
    units: Units,
    function: Option<&str>,
    make: impl Fn() -> Result<(Vec<u8>, String), InferenceError>,
) -> Result<Model, InferenceError> {
    let target = dir.join(format!("{}-{tag}.mlmodelc", model.prefix));
    let kept = target.join("coremldata.bin").is_file();
    let units = units.core_ml();
    match load(&compiled(model, dir, &target, &make)?, units, function) {
        Ok(loaded) => Ok(Model(loaded)),
        Err(_) if kept => {
            discard(model.prefix, dir, &target);
            load(&compiled(model, dir, &target, &make)?, units, function).map(Model)
        }
        Err(e) => Err(e),
    }
}

/// The compiled model at `target` in `dir`, made when it is not yet: the package written
/// in a work folder (the embedded program and manifest, and the weights `make` answers),
/// compiled, and moved into place.
fn compiled(
    model: &Converted,
    dir: &Path,
    target: &Path,
    make: &impl Fn() -> Result<(Vec<u8>, String), InferenceError>,
) -> Result<PathBuf, InferenceError> {
    let target = target.to_path_buf();
    if target.join("coremldata.bin").is_file() {
        return Ok(target);
    }
    let io = |what: &str, e: std::io::Error| InferenceError::Failed(format!("{what}: {e}"));
    let (weights, notice) = make()?;
    std::fs::create_dir_all(dir).map_err(|e| io("the converted model's folder", e))?;
    tidy(model.prefix, dir, &target);
    std::fs::write(dir.join("NOTICE"), notice)
        .map_err(|e| io("the converted model's notice", e))?;
    let work = tempfile_in(model.prefix, dir).map_err(|e| io("a folder to compile in", e))?;
    let package = work.join("model.mlpackage");
    let data = package.join("Data").join("com.apple.CoreML");
    let written = std::fs::create_dir_all(data.join("weights"))
        .and_then(|()| std::fs::write(package.join("Manifest.json"), model.manifest))
        .and_then(|()| std::fs::write(data.join("model.mlmodel"), model.program))
        .and_then(|()| std::fs::write(data.join("weights").join("weight.bin"), &weights))
        .map_err(|e| io("the package", e));
    drop(weights);
    let result = written.and_then(|()| compile(&package)).and_then(|from| {
        let staged = work.join("compiled.mlmodelc");
        let copied = copy_dir(&from, &staged).map_err(|e| io("the compiled model", e));
        let _ = std::fs::remove_dir_all(&from);
        copied?;
        publish(&staged, &target, &work)
            .map(|()| target.clone())
            .map_err(|e| io("the compiled model", e))
    });
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Core ML's weights file (`weight.bin`) holding `blobs`, half-precision tensors, in
/// order: a header of the blob count and version 2, then for each blob 64 bytes of
/// metadata and its data, each metadata at a multiple of 64. `capacity` is the file's
/// expected size, to allocate it once.
pub fn blob_file(blobs: &[Vec<u8>], capacity: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(capacity.max(64));
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
    out
}

/// Removes the compiled model `target` in `dir`: first moved aside in one step, so no
/// folder is left under its name if the removal is cut short. What is moved aside is a
/// work folder, which [`tidy`] removes later if this doesn't. Only when no work folder
/// can be made is `target` removed where it is.
fn discard(prefix: &str, dir: &Path, target: &Path) {
    match tempfile_in(prefix, dir) {
        Ok(aside) => {
            // Gone already (another load discarded it), or moved: either way the work
            // folder goes, never a model another load may have put there since.
            let _ = std::fs::rename(target, aside.join("removed"));
            let _ = std::fs::remove_dir_all(&aside);
        }
        Err(_) => {
            let _ = std::fs::remove_dir_all(target);
        }
    }
}

/// Moves the compiled model `staged` to `target` in one step: a load running at the same
/// time sees all of it or none. Another load may have put one there first; its copy is
/// as good. A folder without `coremldata.bin` under `target`'s name is what a removal cut
/// short leaves: it is moved into `work`, the caller's work folder, and replaced; one
/// another load moved away first is not missed.
fn publish(staged: &Path, target: &Path, work: &Path) -> std::io::Result<()> {
    let complete = || target.join("coremldata.bin").is_file();
    let mut last = None;
    for attempt in 0..3 {
        match std::fs::rename(staged, target) {
            Ok(()) => return Ok(()),
            Err(_) if complete() => return Ok(()),
            Err(e) if !target.exists() => last = Some(e),
            Err(e) => {
                last = Some(e);
                match std::fs::rename(target, work.join(format!("unfinished-{attempt}"))) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("the compiled model was not moved")))
}

/// How old a work folder of another process must be before it is taken for one a load
/// left behind: longer than any load may take.
const ABANDONED: Duration = Duration::from_secs(3600);

/// Removes from `dir` the compiled models of other versions of the model than `keep`'s,
/// and work folders an earlier process left there (its load given up or the process
/// ended). A work folder of a load still running, in this or another process, is
/// younger than [`ABANDONED`] and is left.
fn tidy(prefix: &str, dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let ours = format!(".{prefix}-{}-", std::process::id());
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let old_compiled =
            name.starts_with(&format!("{prefix}-")) && name.ends_with(".mlmodelc") && path != keep;
        let abandoned = name.starts_with(&format!(".{prefix}-"))
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

/// A new hidden folder in `dir`, named for `prefix` and this process.
fn tempfile_in(prefix: &str, dir: &Path) -> std::io::Result<PathBuf> {
    for n in 0..1000u32 {
        let path = dir.join(format!(".{prefix}-{}-{n}", std::process::id()));
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

/// The compiled model at `path` on `units`: its function `function`, when it has several.
fn load(
    path: &Path,
    units: MLComputeUnits,
    function: Option<&str>,
) -> Result<Retained<MLModel>, InferenceError> {
    autoreleasepool(|_| {
        let url = NSURL::from_file_path(path)
            .ok_or_else(|| InferenceError::Failed("the model's path is not a URL".into()))?;
        let config = unsafe { MLModelConfiguration::new() };
        unsafe { config.setComputeUnits(units) };
        if let Some(function) = function {
            unsafe { config.setFunctionName(Some(&NSString::from_str(function))) };
        }
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

/// Runs `model` on one `f32` input of `shape`, written by `fill`, and answers each of
/// `outputs` (a name, the shape it must be in, and a factor) as contiguous `f32`s, each
/// multiplied by its factor.
fn predict(
    model: &MLModel,
    input: &str,
    shape: &[usize],
    fill: impl FnOnce(&mut [f32]),
    outputs: &[(&str, &[usize], f32)],
) -> Result<Vec<Vec<f32>>, InferenceError> {
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
        write(&array, shape.iter().product(), fill)?;
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
        outputs
            .iter()
            .map(|&(output, expected, scale)| {
                let answer = unsafe { result.featureValueForName(&NSString::from_str(output)) }
                    .and_then(|v| unsafe { v.multiArrayValue() })
                    .ok_or_else(|| {
                        InferenceError::ModelInvalid(format!("the model has no output {output}"))
                    })?;
                read(&answer, expected, scale)
            })
            .collect()
    })
}

/// The values of `array`, a new single-precision array of `count` values, written by
/// `fill`. Core ML lends the bytes for as long as `fill` runs; they are refused unless
/// they are `count` contiguous values.
fn write(
    array: &MLMultiArray,
    count: usize,
    fill: impl FnOnce(&mut [f32]),
) -> Result<(), InferenceError> {
    let shape = shape(array)?;
    let bytes_needed = count.checked_mul(4);
    let fill = Cell::new(Some(fill));
    let done = Cell::new(false);
    let (fill, done, shape) = (&fill, &done, &shape);
    // Core ML may switch the array to another buffer for this call: the strides it
    // passes are the ones that hold.
    let block = StackBlock::new(
        move |bytes: NonNull<c_void>, size: NSInteger, strides: NonNull<NSArray<NSNumber>>| {
            let values = bytes.cast::<f32>().as_ptr();
            // SAFETY: Core ML passes the array's strides, valid while this block runs.
            let strides = sizes(unsafe { strides.as_ref() }, "strides");
            let contiguous = strides.is_ok_and(|s| row_major(&s, shape));
            let fits = bytes_needed
                .zip(usize::try_from(size).ok())
                .is_some_and(|(needed, size)| size >= needed);
            if let (true, true, true, Some(fill)) =
                (contiguous, fits, values.is_aligned(), fill.take())
            {
                // SAFETY: Core ML lends `size` bytes, aligned for `f32` and holding at
                // least `count` contiguous ones (checked above), for as long as this
                // block runs, to this block alone. A new array leaves them
                // uninitialized: they are zeroed before they are seen as a slice.
                fill(unsafe {
                    std::ptr::write_bytes(values, 0, count);
                    std::slice::from_raw_parts_mut(values, count)
                });
                done.set(true);
            }
        },
    );
    unsafe { array.getMutableBytesWithHandler(&block) };
    if done.get() {
        Ok(())
    } else {
        Err(InferenceError::Failed(
            "Core ML lent no room for the model's input".into(),
        ))
    }
}

/// `array`'s shape.
fn shape(array: &MLMultiArray) -> Result<Vec<usize>, InferenceError> {
    sizes(&*unsafe { array.shape() }, "shape")
}

/// `array`'s strides, in values.
fn strides(array: &MLMultiArray) -> Result<Vec<usize>, InferenceError> {
    sizes(&*unsafe { array.strides() }, "strides")
}

/// `list` as sizes, refused when one is negative.
fn sizes(list: &NSArray<NSNumber>, what: &str) -> Result<Vec<usize>, InferenceError> {
    list.iter()
        .map(|n| usize::try_from(n.integerValue()))
        .collect::<Result<_, _>>()
        .map_err(|_| InferenceError::OutputInvalid(format!("the array's {what} are negative")))
}

/// Whether `strides` are those of contiguous values of `shape`, the last dimension fastest.
fn row_major(strides: &[usize], shape: &[usize]) -> bool {
    let mut next = 1;
    strides.len() == shape.len()
        && strides.iter().zip(shape).rev().all(|(&stride, &n)| {
            let ok = stride == next || n == 1;
            next *= n;
            ok
        })
}

/// `array`, which must be `expected` in shape, as contiguous `f32`s times `scale`. The
/// Neural Engine and the GPU answer in half precision and may pad rows: read by the strides, from
/// the bytes Core ML lends, each value's offset checked against their length.
fn read(array: &MLMultiArray, expected: &[usize], scale: f32) -> Result<Vec<f32>, InferenceError> {
    let shape = shape(array)?;
    if shape != expected || shape.contains(&0) {
        return Err(InferenceError::OutputInvalid(format!(
            "the model answered with shape {shape:?}, expected {expected:?}"
        )));
    }
    let strides = strides(array)?;
    if strides.len() != shape.len() {
        return Err(InferenceError::OutputInvalid(format!(
            "the model answered {} strides for {} dimensions",
            strides.len(),
            shape.len()
        )));
    }
    let width = match unsafe { array.dataType() } {
        MLMultiArrayDataType::Float16 => 2,
        MLMultiArrayDataType::Float32 => 4,
        other => {
            return Err(InferenceError::OutputInvalid(format!(
                "the model answered in data type {}",
                other.0
            )));
        }
    };
    // The last value's offset, plus one: how many values the strides reach.
    let reach = shape.iter().zip(&strides).try_fold(1usize, |at, (n, s)| {
        (n - 1).checked_mul(*s)?.checked_add(at)
    });
    let last = shape.len() - 1;
    let (inner, inner_stride) = (shape[last], strides[last]);
    let rows: usize = shape[..last].iter().product();
    // Each row's offset: its index written out over the outer dimensions.
    let row_offset = |mut row: usize| -> usize {
        let mut offset = 0;
        for d in (0..last).rev() {
            offset += (row % shape[d]) * strides[d];
            row /= shape[d];
        }
        offset
    };
    let out: Cell<Option<Result<Vec<f32>, InferenceError>>> = Cell::new(None);
    let out_ref = &out;
    let block = StackBlock::new(move |bytes: NonNull<c_void>, size: NSInteger| {
        let lent = usize::try_from(size).unwrap_or(0);
        let fits = reach
            .and_then(|r| r.checked_mul(width))
            .is_some_and(|b| b <= lent);
        if !fits {
            out_ref.set(Some(Err(InferenceError::OutputInvalid(format!(
                "the model's answer holds {lent} bytes, fewer than its strides reach"
            )))));
            return;
        }
        let base = bytes.cast::<u8>().as_ptr();
        // A row of contiguous values is read as one slice: it holds no padding.
        let rows_whole = inner_stride == 1 && base.align_offset(width) == 0;
        let mut values = Vec::with_capacity(rows * inner);
        for row in 0..rows {
            let start = row_offset(row);
            // SAFETY: every value read is at an offset below `reach` values of `width`
            // bytes, which fit in the `size` bytes Core ML lends for as long as this
            // block runs. A row is seen as a slice only when its values are contiguous
            // and aligned, so the slice holds them alone, never the padding between
            // rows; otherwise each value is read on its own.
            unsafe {
                let at = base.add(start * width);
                match (width, rows_whole) {
                    (2, true) => values.extend(
                        std::slice::from_raw_parts(at.cast::<half::f16>(), inner)
                            .iter()
                            .map(|v| v.to_f32() * scale),
                    ),
                    (_, true) => values.extend(
                        std::slice::from_raw_parts(at.cast::<f32>(), inner)
                            .iter()
                            .map(|v| v * scale),
                    ),
                    (2, false) => values.extend((0..inner).map(|x| {
                        let v = at.add(x * inner_stride * 2).cast::<u16>().read_unaligned();
                        half::f16::from_bits(v).to_f32() * scale
                    })),
                    (_, false) => values.extend((0..inner).map(|x| {
                        at.add(x * inner_stride * 4).cast::<f32>().read_unaligned() * scale
                    })),
                }
            }
        }
        out_ref.set(Some(Ok(values)));
    });
    unsafe { array.getBytesWithHandler(&block) };
    out.take().unwrap_or_else(|| {
        Err(InferenceError::OutputInvalid(
            "Core ML lent none of the model's answer".into(),
        ))
    })
}

/// `load` on a thread of its own, given up after `limit`: Core ML's load cannot be
/// cancelled, so a load that never returns keeps its thread, and `stalled` is set so
/// that later calls fail at once.
pub fn within<T: Send + 'static>(
    limit: Duration,
    stalled: &'static AtomicBool,
    what: &'static str,
    load: impl FnOnce() -> Result<T, InferenceError> + Send + 'static,
) -> Result<T, InferenceError> {
    if stalled.load(Ordering::Relaxed) {
        return Err(InferenceError::Failed(format!(
            "an earlier load for {what} never finished"
        )));
    }
    let (send, answer) = mpsc::channel();
    std::thread::Builder::new()
        .name("core-ml-load".into())
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
                "the load for {what} took over {} s",
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
