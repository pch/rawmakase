//! Lazy loading of the ONNX Runtime shared library and the model session.
//!
//! The library is never linked: it is opened at run time from a path this
//! module chooses, so an absent or unusable runtime is an
//! [`InferenceError::RuntimeUnavailable`] value and nothing else in the app is
//! affected. The first successfully opened library, and the one environment
//! `ort` allows per process, are kept for the life of the process (ONNX Runtime
//! cannot be unloaded safely).

use rawmakase_cancel::Cancel;
use std::env;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ort::environment::Environment;
use ort::ep::{self, ExecutionProviderDispatch};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{RunOptions, Session as OrtSession};
use ort::value::TensorRef;

use crate::denoise::TileModel;
use crate::error::InferenceError;
use crate::manifest::{ModelFile, RAWNIND, denoise_files};

/// Environment variable naming the ONNX Runtime library file to load.
pub const RUNTIME_ENV: &str = "RAWMAKASE_ORT_LIB";

/// How often a running inference polls the caller's cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(15);

/// Upper bound on the intra-op threads: beyond this the encoder kernels
/// stop scaling and only compete with the UI and render threads.
const MAX_THREADS: usize = 8;

/// The process-wide ONNX Runtime environment, built once the library opens.
/// `ort` refuses a second environment even after the first is dropped, so it
/// is never released.
static ENVIRONMENT: Mutex<Option<Environment>> = Mutex::new(None);

/// The library file names to look for, most specific first.
pub fn runtime_file_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["onnxruntime.dll"]
    } else if cfg!(target_os = "macos") {
        &["libonnxruntime.dylib"]
    } else {
        &["libonnxruntime.so", "libonnxruntime.so.1"]
    }
}

/// Directories next to the running executable that a package may place the
/// runtime in. System search paths are deliberately absent: on Windows a bare
/// `onnxruntime.dll` resolves to the older copy in System32.
pub fn runtime_search_dirs() -> Vec<PathBuf> {
    let Some(exe_dir) = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return Vec::new();
    };
    let mut dirs = vec![exe_dir.clone(), exe_dir.join("lib")];
    if cfg!(target_os = "macos") {
        // Contents/MacOS/rawmakase -> Contents/Frameworks
        dirs.insert(0, exe_dir.join("../Frameworks"));
    } else if cfg!(target_os = "linux") {
        dirs.push(exe_dir.join("../lib/rawmakase"));
        dirs.push(exe_dir.join("../lib"));
    }
    dirs
}

/// Which hardware the sessions are asked to run on. `Cpu` is the default and the
/// only thing every platform ships; the others are a request, granted only when
/// the loaded ONNX Runtime actually registers the provider: a missing CUDA
/// runtime or CoreML device is an error from
/// `rawmakase_selection::Subject::load_with`, never a silent slide back to the CPU.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Provider {
    #[default]
    Cpu,
    /// NVIDIA GPUs through ONNX Runtime's CUDA execution provider.
    Cuda,
    /// Apple Silicon through ONNX Runtime's CoreML execution provider.
    CoreML,
}
impl Provider {
    /// The name ONNX Runtime gives it, for reporting what was registered.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Cuda => "CUDA",
            Self::CoreML => "CoreML",
        }
    }
    /// The providers to register, fastest first, with the CPU last as the
    /// fallback for whatever nodes the accelerated one does not take. Only
    /// `Cpu` is allowed to fail quietly (it cannot).
    fn providers(self) -> Vec<ExecutionProviderDispatch> {
        let cpu = ep::CPU::default().with_arena_allocator(false).build();
        match self {
            Self::Cpu => vec![cpu],
            Self::Cuda => vec![ep::CUDA::default().build().error_on_failure(), cpu],
            Self::CoreML => vec![ep::CoreML::default().build().error_on_failure(), cpu],
        }
    }
}

/// Options for `rawmakase_selection::Subject::load_with`.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Explicit runtime library file. Takes precedence over
    /// [`RUNTIME_ENV`] and the package locations.
    pub runtime_library: Option<PathBuf>,
    /// Folders to look in after [`RUNTIME_ENV`] and before the ones beside
    /// the running executable ([`runtime_search_dirs`]): where the app that
    /// started a bundled extension's program keeps its runtime.
    pub runtime_dirs: Vec<PathBuf>,
    /// Intra-op threads; `None` picks [`default_threads`].
    pub threads: Option<usize>,
    /// Hardware to ask for; `None` is the same as [`Provider::Cpu`].
    pub provider: Option<Provider>,
}

/// Threads for one inference: the available parallelism capped at 8. ONNX
/// Runtime's own default would use every core including the efficiency cores.
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(1, MAX_THREADS)
}

/// The loaded AI Denoise model ([`RAWNIND`]). One tile at a time; dropping it releases the
/// model's memory.
pub struct Denoiser {
    network: OrtSession,
    provider: Provider,
}

impl Denoiser {
    /// Opens the runtime as `rawmakase_selection::Subject::load_with` does and the
    /// denoiser in folder `dir` (with its `LICENSE` and `NOTICE`). A provider the
    /// manifest does not allow is an error. On CoreML the graph runs as an ML Program
    /// on the CPU and GPU, in single precision: the network's activations overflow
    /// half precision.
    pub fn load_with(dir: &Path, options: &LoadOptions) -> Result<Self, InferenceError> {
        let provider = options.provider.unwrap_or_default();
        if !RAWNIND.providers.contains(&provider) {
            return Err(InferenceError::Failed(format!(
                "AI Denoise does not run on {}",
                provider.name()
            )));
        }
        let environment = load_runtime_with(options)?;
        check_files(dir, &denoise_files())?;
        let threads = options.threads.unwrap_or_else(default_threads).max(1);
        let network = catch_unwind(AssertUnwindSafe(|| {
            build_session_with(
                &environment,
                &dir.join(RAWNIND.model.name),
                threads,
                GraphOptimizationLevel::All,
                provider,
                denoise_providers(provider),
            )
        }))
        .map_err(|_| {
            InferenceError::Failed("ONNX Runtime panicked while loading the model".into())
        })??;
        Ok(Self { network, provider })
    }

    /// The provider the runtime accepted.
    pub fn provider(&self) -> Provider {
        self.provider
    }
}

impl TileModel for Denoiser {
    fn run(&mut self, input: &[f32], cancel: Cancel<'_>) -> Result<Vec<f32>, InferenceError> {
        cancelled(cancel)?;
        let side = RAWNIND.tile;
        if input.len() != 4 * side * side {
            return Err(InferenceError::OutputInvalid(format!(
                "a denoiser tile holds {} values, expected {}",
                input.len(),
                4 * side * side
            )));
        }
        let network = &mut self.network;
        guarded(cancel, |options| {
            let tensor =
                TensorRef::from_array_view(([1usize, 4, side, side], input)).map_err(failed)?;
            let outputs = network
                .run_with_options(ort::inputs![RAWNIND.input => tensor], options)
                .map_err(failed)?;
            let output = outputs.get(RAWNIND.output).ok_or_else(|| {
                InferenceError::ModelInvalid(format!(
                    "the denoiser has no output `{}`",
                    RAWNIND.output
                ))
            })?;
            let (shape, data) = output.try_extract_tensor::<f32>().map_err(failed)?;
            let expected = [1, 3, 2 * side as i64, 2 * side as i64];
            if **shape != expected {
                return Err(InferenceError::OutputInvalid(format!(
                    "the denoiser answered with shape {shape:?}, expected {expected:?}"
                )));
            }
            Ok(data.to_vec())
        })
    }
}

/// The denoiser's providers: CoreML as an ML Program on the CPU and GPU (the measured
/// configuration), the CPU otherwise.
fn denoise_providers(provider: Provider) -> Vec<ExecutionProviderDispatch> {
    match provider {
        Provider::CoreML => vec![
            ep::CoreML::default()
                .with_model_format(ep::coreml::ModelFormat::MLProgram)
                .with_compute_units(ep::coreml::ComputeUnits::CPUAndGPU)
                .with_static_input_shapes(true)
                .build()
                .error_on_failure(),
            ep::CPU::default().with_arena_allocator(false).build(),
        ],
        other => other.providers(),
    }
}

/// A borrowed tensor view of a `[1, c, h, w]` feature map, for feeding a session.
/// Public for `rawmakase-selection`, which feeds its decoder the encoder's outputs.
pub fn view(f: &([usize; 4], Vec<f32>)) -> Result<TensorRef<'_, f32>, InferenceError> {
    TensorRef::from_array_view((f.0, &f.1[..])).map_err(failed)
}

/// [`InferenceError::Cancelled`] when `cancel` is raised. Public for
/// `rawmakase-selection`, which checks it between its own steps.
pub fn cancelled(cancel: Cancel<'_>) -> Result<(), InferenceError> {
    if cancel.is_cancelled() {
        Err(InferenceError::Cancelled)
    } else {
        Ok(())
    }
}

/// Runs `work` with a watcher that terminates the run when `cancel` is raised, and a
/// panic in the runtime turned into an error. Public for `rawmakase-selection`, which
/// runs its own models through it.
pub fn guarded<T>(
    cancel: Cancel<'_>,
    work: impl FnOnce(&RunOptions) -> Result<T, InferenceError>,
) -> Result<T, InferenceError> {
    let options = RunOptions::new().map_err(failed)?;
    let done = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                if cancel.is_cancelled() {
                    let _ = options.terminate();
                    return;
                }
                std::thread::sleep(CANCEL_POLL);
            }
        });
        let result = catch_unwind(AssertUnwindSafe(|| work(&options)));
        done.store(true, Ordering::Release);
        result
    });
    if cancel.is_cancelled() {
        return Err(InferenceError::Cancelled);
    }
    match result {
        Ok(result) => result,
        Err(_) => Err(InferenceError::Failed(
            "ONNX Runtime panicked during inference".into(),
        )),
    }
}

/// A runtime error as [`InferenceError::Failed`]. Public for `rawmakase-selection`, which
/// maps its own runtime calls' errors through it.
pub fn failed(error: impl std::fmt::Display) -> InferenceError {
    InferenceError::Failed(error.to_string())
}

/// [`InferenceError::ModelInvalid`] unless every one of `files` is in `dir` at its
/// manifest size: checked before the runtime is touched. Public for
/// `rawmakase-selection`, which checks its own models' files.
pub fn check_files(dir: &Path, files: &[ModelFile]) -> Result<(), InferenceError> {
    for file in files {
        let path = dir.join(file.name);
        let meta = std::fs::metadata(&path).map_err(|e| {
            InferenceError::ModelInvalid(format!("cannot read {}: {e}", path.display()))
        })?;
        if !meta.is_file() || meta.len() != file.size_bytes {
            return Err(InferenceError::ModelInvalid(format!(
                "{} is not the expected {} bytes",
                path.display(),
                file.size_bytes
            )));
        }
    }
    Ok(())
}

/// A session for the model at `model_path` on `provider`, with the CPU as the fallback
/// for nodes the provider does not take. Public for `rawmakase-selection`, which builds
/// its own models' sessions with it.
pub fn build_session(
    environment: &Environment,
    model_path: &Path,
    threads: usize,
    level: GraphOptimizationLevel,
    provider: Provider,
) -> Result<OrtSession, InferenceError> {
    build_session_with(
        environment,
        model_path,
        threads,
        level,
        provider,
        provider.providers(),
    )
}

fn build_session_with(
    environment: &Environment,
    model_path: &Path,
    threads: usize,
    level: GraphOptimizationLevel,
    provider: Provider,
    providers: Vec<ExecutionProviderDispatch>,
) -> Result<OrtSession, InferenceError> {
    OrtSession::builder(environment)
        .map_err(failed)?
        .with_optimization_level(level)
        .map_err(failed)?
        .with_intra_threads(threads)
        .map_err(failed)?
        .with_inter_threads(1)
        .map_err(failed)?
        .with_intra_op_spinning(false)
        .map_err(failed)?
        .with_memory_pattern(false)
        .map_err(failed)?
        .with_execution_providers(providers)
        .map_err(failed)?
        .commit_from_file(model_path)
        .map_err(|e| {
            InferenceError::ModelInvalid(format!(
                "cannot load {} on {}: {e}",
                model_path.display(),
                provider.name()
            ))
        })
}

/// Opens the ONNX Runtime library if it is not open yet. The first successful
/// load is process-wide; later calls with another path are no-ops. Returns the
/// environment sessions are built in.
pub fn load_runtime(explicit: Option<&Path>) -> Result<Environment, InferenceError> {
    open_first(runtime_candidates(
        explicit,
        env::var_os(RUNTIME_ENV),
        &[],
        &runtime_search_dirs(),
    ))
}

/// [`load_runtime`] with `options`' library, then [`RUNTIME_ENV`], then
/// `options`' folders, then the folders beside the running executable.
pub fn load_runtime_with(options: &LoadOptions) -> Result<Environment, InferenceError> {
    open_first(runtime_candidates(
        options.runtime_library.as_deref(),
        env::var_os(RUNTIME_ENV),
        &options.runtime_dirs,
        &runtime_search_dirs(),
    ))
}

/// Opens the first of `candidates` that loads.
fn open_first(candidates: Vec<PathBuf>) -> Result<Environment, InferenceError> {
    let mut tried = Vec::new();
    if candidates.is_empty() {
        return Err(InferenceError::RuntimeUnavailable(format!(
            "no ONNX Runtime library found; set {RUNTIME_ENV} or place {} next to the application",
            runtime_file_names()[0]
        )));
    }
    for path in candidates {
        match open_runtime(&path) {
            Ok(environment) => return Ok(environment),
            Err(why) => tried.push(format!("{}: {why}", path.display())),
        }
    }
    Err(InferenceError::RuntimeUnavailable(tried.join("; ")))
}

/// The libraries to try, in order: `explicit`, the file `variable` (the value
/// of [`RUNTIME_ENV`]) names, then the runtime files in each of `dirs` and
/// then of `own`.
fn runtime_candidates(
    explicit: Option<&Path>,
    variable: Option<std::ffi::OsString>,
    dirs: &[PathBuf],
    own: &[PathBuf],
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = explicit {
        candidates.push(path.to_path_buf());
    }
    if let Some(path) = variable.filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(path));
    }
    for dir in dirs.iter().chain(own) {
        for name in runtime_file_names() {
            let path = dir.join(name);
            if path.is_file() {
                candidates.push(path);
            }
        }
    }
    candidates
}

fn open_runtime(path: &Path) -> Result<Environment, String> {
    if !path.is_absolute() {
        return Err("the runtime path must be absolute".into());
    }
    if !path.is_file() {
        return Err("no such file".into());
    }
    // `ort` validates the version and returns an error rather than panicking,
    // but a panic here must still never take the caller down.
    catch_unwind(AssertUnwindSafe(|| {
        let builder = ort::init_from(path).map_err(|e| e.to_string())?;
        let mut environment = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(environment) = environment.as_ref() {
            return Ok(environment.clone());
        }
        let built = builder
            .with_name("rawmakase")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(environment.insert(built).clone())
    }))
    .map_err(|_| "loading the library panicked".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_runtime_is_an_error_value() {
        let missing = Path::new("/nonexistent/rawmakase/libonnxruntime.dylib");
        let err = load_runtime(Some(missing)).unwrap_err();
        assert!(
            matches!(err, InferenceError::RuntimeUnavailable(_)),
            "{err:?}"
        );
    }

    #[test]
    fn a_relative_runtime_path_is_refused() {
        let err = load_runtime(Some(Path::new("libonnxruntime.dylib"))).unwrap_err();
        assert!(matches!(err, InferenceError::RuntimeUnavailable(_)));
    }

    #[test]
    fn a_file_that_is_not_a_library_is_refused() {
        let dir =
            std::env::temp_dir().join(format!("rawmakase-inference-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join(runtime_file_names()[0]);
        std::fs::write(&fake, b"not a library").unwrap();
        let err = load_runtime(Some(&fake)).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(err, InferenceError::RuntimeUnavailable(_)),
            "{err:?}"
        );
    }

    /// The explicit library, then the variable's, then the caller's folders,
    /// then the executable's own: the order a bundled extension's program
    /// relies on to find the app's runtime before its own.
    #[test]
    fn runtime_candidates_keep_their_order() {
        let dir = tempfile::tempdir().unwrap();
        let folder = |name: &str| {
            let path = dir.path().join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join(runtime_file_names()[0]), b"").unwrap();
            path
        };
        let (host, own, empty) = (folder("host"), folder("own"), dir.path().join("empty"));
        let explicit = dir.path().join("explicit");
        let found = runtime_candidates(
            Some(&explicit),
            Some("/the/variable".into()),
            &[empty, host.clone()],
            std::slice::from_ref(&own),
        );
        assert_eq!(
            found,
            [
                explicit,
                PathBuf::from("/the/variable"),
                host.join(runtime_file_names()[0]),
                own.join(runtime_file_names()[0]),
            ]
        );
        // An empty variable names nothing.
        assert_eq!(
            runtime_candidates(None, Some("".into()), &[], std::slice::from_ref(&own)),
            [own.join(runtime_file_names()[0])]
        );
    }

    #[test]
    fn threads_are_bounded() {
        let n = default_threads();
        assert!((1..=MAX_THREADS).contains(&n));
    }
}
