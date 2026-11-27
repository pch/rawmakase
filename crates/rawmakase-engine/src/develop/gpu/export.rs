//! Dedicated headless GPU compute backend for SDR, HDR, and gain-map exports with
//! deterministic two-tier CPU fallback.
//!
//! [`ExportBackend`] owns its own headless `wgpu::Device` (decoupled from eframe's UI
//! device and [`super::SURFACE`]) and executes exports in horizontal bands (~6 MP per
//! band) with exact vertical halo overlap for sharpening and Lanczos3 resizing so
//! band seams are bit-exact against single-band full-frame output. Band readbacks
//! use double-buffered staging buffers so band `k` executes on the GPU while band
//! `k - 1` maps and copies back to host memory. When a large sensor's full camera
//! buffer exceeds the adapter's `max_storage_buffer_binding_size`, [`ExportBackend`]
//! falls back to per-band source-strip upload (uploading only the conservative source
//! rows each output band samples via `S_PHOTO_Y0`) before resorting to Tier-1 CPU.
//!
//! No `wgpu` types cross the `rawmakase-engine` boundary: callers interact solely
//! through [`ExportBackend`], [`ExportBackendPolicy`], [`ExportBackendStatus`],
//! [`ExportFallbackKind`], [`crate::develop::ExportFormat`], and
//! [`crate::develop::ExportImage`].
//!
//! # Two-tier CPU fallback
//!
//! - **Tier 1 (per-photo)**: unsupported recipes (unresolved non-profile recipes, HDR
//!   or SDR-rendition sliders, range masks, local Texture/Clarity/Sharpness/Noise
//!   masks), photos exceeding adapter storage limits, or single-photo out-of-memory
//!   (after one retry with halved band height) fall back to the CPU reference
//!   [`crate::develop::render_export`] while keeping the GPU active for subsequent
//!   photos.
//! - **Tier 2 (session-wide)**: no hardware compute adapter, validation or internal
//!   driver errors, device loss, poll timeouts (>5 s per GPU submission), or an unwinding panic
//!   disable the GPU for the remainder of the session and fall back to the CPU
//!   reference.
//! - **Cancellation** is never a fallback: when [`Cancel::is_cancelled`] becomes true
//!   (checked before upload, between pyramid stages/channels and bands, and in 2 ms poll slices),
//!   rendering aborts immediately without running the CPU fallback or disabling the GPU.

use super::{
    develop::{Developer, Variant},
    pyramid::{GpuPyramid, PyramidHost},
    resident::Resident,
    sampling as sample_slot,
    uniforms::ExportFinishParams,
    weights,
};
use crate::{
    camera_data::CameraImage,
    color::OutputSpace,
    develop::{
        AiSource, ExportFormat, ExportImage, ExportPixels, Geometry,
        effects::{GrainField, PostCropVignette},
        local_tone,
        masks::{Selection, Weigher},
        pipeline::{self, Source, pixel_params},
        quality,
        recipe::{Denoise, RenderedRecipe},
        retouch::{self, Retouching},
        sharpening::Sharpener,
        texture,
    },
    model::{
        masks::MaskShape,
        point_color::{SwatchView, without_visualization},
        recipe::Recipe,
        valid::ValidRecipe,
    },
};
use anyhow::{Result, anyhow, ensure};
use rawmakase_cancel::Cancel;
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use wgpu::util::DeviceExt;

/// Environment variable overriding GPU export (`0` disables GPU, `1` enables GPU).
pub const EXPORT_GPU_ENV: &str = "RAWMAKASE_EXPORT_GPU";

/// Target compute pixels per horizontal band (~6 MP keeps working VRAM around ~240–290 MB
/// regardless of whether the photo is 24 MP or 61+ MP).
const DEFAULT_BAND_PIXELS: u64 = 6_000_000;
/// Minimum default band output height before OOM halving.
const MIN_BAND_HEIGHT: u64 = 64;
/// Poll slice interval while waiting for a band submission.
const BAND_POLL_SLICE: Duration = Duration::from_millis(2);
/// Per-submission GPU poll deadline before treating the driver as stalled (Tier-2 fallback).
const BAND_POLL_DEADLINE: Duration = Duration::from_secs(5);
/// Workgroup width for 1D compute dispatches (`sample_region`, `develop`, `pack_*`).
const WORKGROUP_1D: u32 = 256;

/// Policy governing whether [`ExportBackend`] attempts GPU compute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportBackendPolicy {
    /// Probe and use a hardware compute adapter when available, falling back
    /// deterministically to the CPU reference on unsupported recipes or GPU errors.
    /// Honoring `RAWMAKASE_EXPORT_GPU=0` disables the GPU.
    #[default]
    Auto,
    /// Always run on the CPU reference without probing or creating a GPU device.
    CpuOnly,
}

/// Why an export (or the entire session) fell back to the CPU reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportFallbackKind {
    /// Policy was [`ExportBackendPolicy::CpuOnly`] or `RAWMAKASE_EXPORT_GPU=0`.
    ConfiguredCpu,
    /// No hardware compute adapter was found (or only software/OpenGL adapters).
    NoHardwareAdapter,
    /// Recipe uses a feature not yet ported to GPU export.
    UnsupportedRecipe,
    /// Source image or band exceeds adapter storage, buffer, or dispatch limits.
    ExceedsLimits,
    /// Single-photo GPU out-of-memory even after retrying with halved band height.
    OutOfMemory,
    /// GPU validation error, internal driver error, or unwinding panic.
    DriverError,
    /// GPU device lost or per-band poll deadline exceeded.
    DeviceLostOrTimeout,
}

/// Snapshot of [`ExportBackend`]'s current state and cumulative render telemetry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportBackendStatus {
    /// Effective policy after environment overrides.
    pub policy: ExportBackendPolicy,
    /// Hardware adapter name once initialized, if a GPU device was created.
    pub adapter_name: Option<String>,
    /// Whether GPU compute is currently active (initialized and not session-disabled).
    pub gpu_active: bool,
    /// Whether lazy GPU initialization has not yet run (`Auto` policy before first render).
    pub uninitialized: bool,
    /// Session-wide fallback reason, if GPU is unavailable or disabled by a Tier-2 error.
    pub fallback_reason: Option<String>,
    /// Structured Tier-2 fallback category, if session-disabled.
    pub fallback_kind: Option<ExportFallbackKind>,
    /// Count of renders completed on the GPU, by every caller sharing the backend
    /// (in the app, exports and preview builds alike).
    pub gpu_renders: u64,
    /// Count of renders completed on the CPU reference, by every caller sharing the
    /// backend.
    pub cpu_renders: u64,
    /// Per-reason fallback counts (both Tier-1 per-photo and Tier-2 session fallbacks).
    pub fallback_counts: Vec<(ExportFallbackKind, u64)>,
}

impl ExportBackendStatus {
    /// Returns the number of fallbacks recorded for `kind`.
    pub fn fallback_count(&self, kind: ExportFallbackKind) -> u64 {
        self.fallback_counts
            .iter()
            .find_map(|&(k, count)| (k == kind).then_some(count))
            .unwrap_or(0)
    }
}

/// Thread-safe GPU export backend with deterministic CPU fallback.
///
/// Owns no persistent background threads: GPU submissions execute synchronously on
/// the calling thread under `exec` so multiple CPU decode/encode lanes (and, in the
/// app, the preview build worker) share one compute device and one working-set VRAM
/// budget without contending with the UI device. A caller waiting for `exec` can't
/// be cancelled while it waits; it checks its cancel as soon as it holds `exec`,
/// before any GPU work or CPU fallback. Telemetry and status live under a separate
/// short-lived `meta` lock so
/// [`Self::status`] and [`Self::reset_stats`] never wait on an in-flight GPU export.
/// Lock ordering: `exec` may briefly lock `meta`; `meta` never locks `exec`.
pub struct ExportBackend {
    exec: Mutex<ExecState>,
    meta: Mutex<StatusState>,
    /// SDR renders that have asked for `exec`, so a test knows one is waiting for it.
    #[cfg(test)]
    exec_asked: std::sync::atomic::AtomicUsize,
}

// Compile-time check that ExportBackend is Send + Sync.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ExportBackend>();
};

impl Drop for ExportBackend {
    /// Drops a live GPU device on a detached thread. A cancel stops waiting for a
    /// submitted band without waiting for the GPU, and `wgpu`'s `Queue::drop` waits
    /// for the device to go idle with no deadline: dropped on the thread that drops
    /// the last owner (in the app, the UI thread at exit), a stalled band would
    /// hang it.
    fn drop(&mut self) {
        let exec = self.exec.get_mut().unwrap_or_else(PoisonError::into_inner);
        let engine = std::mem::replace(&mut exec.engine, Engine::Disabled);
        retire_engine(engine, true);
    }
}

impl Default for ExportBackend {
    fn default() -> Self {
        Self::auto()
    }
}

impl std::fmt::Debug for ExportBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status = self.status();
        f.debug_struct("ExportBackend")
            .field("status", &status)
            .finish()
    }
}

struct ExecState {
    engine: Engine,
    #[cfg(test)]
    band_height_override: Option<u32>,
    #[cfg(test)]
    max_binding_override: Option<u64>,
}

struct StatusState {
    policy: ExportBackendPolicy,
    adapter_name: Option<String>,
    gpu_active: bool,
    uninitialized: bool,
    session_fallback: Option<(ExportFallbackKind, String)>,
    gpu_renders: u64,
    cpu_renders: u64,
    fallback_counts: Vec<(ExportFallbackKind, u64)>,
}

enum Engine {
    Uninitialized,
    Disabled,
    Gpu(Box<GpuExportContext>),
    #[cfg(test)]
    Mock(Box<dyn MockGpuDriver + Send>),
}

impl Engine {
    /// Whether dropping this engine drops a device (or, in tests, a driver standing in
    /// for one).
    fn holds_device(&self) -> bool {
        match self {
            Engine::Gpu(_) => true,
            #[cfg(test)]
            Engine::Mock(_) => true,
            Engine::Uninitialized | Engine::Disabled => false,
        }
    }
}

/// Drops a retired GPU engine outside any mutex. When `async_drop` is true (stalled
/// driver timeout, device loss, panic unwind, or the backend itself dropped), drops a
/// real `wgpu::Device`/`Queue` on a detached thread so `wgpu`'s unbounded `Queue::drop`
/// idle wait cannot stall the caller, sibling export lanes, or shutdown past the 5 s
/// band deadline.
fn retire_engine(engine: Engine, async_drop: bool) {
    retire_engine_on(engine, async_drop, |rx| {
        std::thread::Builder::new()
            .name("export-gpu-drop".into())
            .spawn(move || drop(rx.recv()))
            .map(drop)
    });
}

/// [`retire_engine`], starting the thread the engine drops on with `spawn`, which
/// is handed the engine only once it has started. When the thread can't start, the
/// engine is leaked: its device's memory is lost, where dropping it here could hang.
/// Nothing is written to stderr there: a closed or full pipe would panic (dropping
/// the engine here after all) or block.
fn retire_engine_on(
    engine: Engine,
    async_drop: bool,
    spawn: impl FnOnce(mpsc::Receiver<Engine>) -> std::io::Result<()>,
) {
    if !(async_drop && engine.holds_device()) {
        drop(engine);
        return;
    }
    let (tx, rx) = mpsc::channel();
    let unsent = match spawn(rx) {
        Ok(()) => tx.send(engine).err().map(|mpsc::SendError(engine)| engine),
        Err(_) => Some(engine),
    };
    if let Some(engine) = unsent {
        std::mem::forget(engine);
    }
}

/// Internal classification of why a GPU render attempt did not produce an image.
enum GpuFailure {
    /// Cancellation was requested; propagate error without CPU fallback or disabling GPU.
    Cancelled,
    /// Hard non-GPU error (e.g. missing mask raster, invalid camera profile); propagate error.
    Hard(anyhow::Error),
    /// Tier 1: per-photo CPU fallback; GPU remains active for other photos.
    PerPhoto(ExportFallbackKind, String),
    /// Tier 1 retryable OOM: release cached buffers and retry once with halved band height.
    OutOfMemory(String),
    /// Tier 2: session-wide CPU fallback; drop GPU device and stay on CPU.
    Session(ExportFallbackKind, String),
}

/// Classifies popped wgpu error scopes (`OutOfMemory`, `Internal`, `Validation`).
///
/// Out-of-memory takes precedence over validation and internal errors: in `wgpu-core`,
/// a failed buffer allocation leaves an invalid handle whose subsequent `write_buffer`,
/// `create_bind_group`, or submit raises a cascade `Validation` (`InvalidResource`)
/// error in the same scope. Checking OOM first ensures real OOMs follow the Tier-1
/// halved-band retry path rather than disabling the GPU for the session.
fn classify_scope_errors(
    memory_error: Option<wgpu::Error>,
    internal_error: Option<wgpu::Error>,
    validation_error: Option<wgpu::Error>,
) -> Option<GpuFailure> {
    if let Some(err) = memory_error {
        return Some(GpuFailure::OutOfMemory(err.to_string()));
    }
    if let Some(err) = validation_error.or(internal_error) {
        return Some(GpuFailure::Session(
            ExportFallbackKind::DriverError,
            err.to_string(),
        ));
    }
    None
}

/// RAII guard that detects unwinding panics during GPU work, retiring the GPU context
/// cleanly so no unbalanced error scopes or half-mutated buffers survive into later calls.
struct PanicGuard<'a> {
    exec: &'a mut ExecState,
    meta: &'a Mutex<StatusState>,
    armed: bool,
}

impl<'a> PanicGuard<'a> {
    fn new(exec: &'a mut ExecState, meta: &'a Mutex<StatusState>) -> Self {
        Self {
            exec,
            meta,
            armed: true,
        }
    }
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for PanicGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let old = std::mem::replace(&mut self.exec.engine, Engine::Disabled);
            {
                let mut meta = self.meta.lock().unwrap_or_else(PoisonError::into_inner);
                meta.mark_disabled(
                    ExportFallbackKind::DriverError,
                    "GPU export panicked; disabled GPU for remainder of session".into(),
                );
            }
            retire_engine(old, true);
        }
    }
}

/// Resolves the effective policy given a requested `policy`, an optional `RAWMAKASE_EXPORT_GPU`
/// environment value, and whether `env_can_enable` allows `"1"` to upgrade `CpuOnly` to `Auto`.
fn resolve_policy(
    policy: ExportBackendPolicy,
    env_value: Option<&str>,
    env_can_enable: bool,
) -> (ExportBackendPolicy, Option<(ExportFallbackKind, String)>) {
    match (policy, env_value.map(str::trim)) {
        (_, Some("0")) => (
            ExportBackendPolicy::CpuOnly,
            Some((
                ExportFallbackKind::ConfiguredCpu,
                format!("Disabled by {EXPORT_GPU_ENV}=0"),
            )),
        ),
        (ExportBackendPolicy::CpuOnly, Some("1")) if env_can_enable => {
            (ExportBackendPolicy::Auto, None)
        }
        (ExportBackendPolicy::CpuOnly, _) => (
            ExportBackendPolicy::CpuOnly,
            Some((
                ExportFallbackKind::ConfiguredCpu,
                "Configured for CPU-only export".into(),
            )),
        ),
        (ExportBackendPolicy::Auto, _) => (ExportBackendPolicy::Auto, None),
    }
}

impl StatusState {
    fn new(
        policy: ExportBackendPolicy,
        session_fallback: Option<(ExportFallbackKind, String)>,
    ) -> Self {
        let uninitialized = policy == ExportBackendPolicy::Auto;
        Self {
            policy,
            adapter_name: None,
            gpu_active: false,
            uninitialized,
            session_fallback,
            gpu_renders: 0,
            cpu_renders: 0,
            fallback_counts: Vec::new(),
        }
    }

    fn record_fallback(&mut self, kind: ExportFallbackKind) {
        if let Some((_, count)) = self.fallback_counts.iter_mut().find(|(k, _)| *k == kind) {
            *count += 1;
        } else {
            self.fallback_counts.push((kind, 1));
        }
    }

    fn mark_active(&mut self, adapter_name: String) {
        self.adapter_name = Some(adapter_name);
        self.gpu_active = true;
        self.uninitialized = false;
    }

    fn mark_disabled(&mut self, kind: ExportFallbackKind, reason: String) {
        self.gpu_active = false;
        self.uninitialized = false;
        self.session_fallback = Some((kind, reason));
    }

    fn status(&self) -> ExportBackendStatus {
        ExportBackendStatus {
            policy: self.policy,
            adapter_name: self.adapter_name.clone(),
            gpu_active: self.gpu_active,
            uninitialized: self.uninitialized,
            fallback_reason: self.session_fallback.as_ref().map(|(_, r)| r.clone()),
            fallback_kind: self.session_fallback.as_ref().map(|(k, _)| *k),
            gpu_renders: self.gpu_renders,
            cpu_renders: self.cpu_renders,
            fallback_counts: self.fallback_counts.clone(),
        }
    }
}

impl ExportBackend {
    fn build(policy: ExportBackendPolicy, fallback: Option<(ExportFallbackKind, String)>) -> Self {
        let engine = match policy {
            ExportBackendPolicy::Auto => Engine::Uninitialized,
            ExportBackendPolicy::CpuOnly => Engine::Disabled,
        };
        Self {
            exec: Mutex::new(ExecState {
                engine,
                #[cfg(test)]
                band_height_override: None,
                #[cfg(test)]
                max_binding_override: None,
            }),
            meta: Mutex::new(StatusState::new(policy, fallback)),
            #[cfg(test)]
            exec_asked: Default::default(),
        }
    }

    /// Creates an export backend with `policy`.
    ///
    /// - [`ExportBackendPolicy::Auto`] probes a hardware compute adapter lazily on first
    ///   eligible render unless `RAWMAKASE_EXPORT_GPU=0` is set.
    /// - [`ExportBackendPolicy::CpuOnly`] always runs on the CPU reference without
    ///   probing a GPU or consulting the environment.
    pub fn new(policy: ExportBackendPolicy) -> Self {
        let env_val = if policy == ExportBackendPolicy::Auto {
            std::env::var(EXPORT_GPU_ENV).ok()
        } else {
            None
        };
        let (effective, fallback) = resolve_policy(policy, env_val.as_deref(), false);
        Self::build(effective, fallback)
    }

    /// Creates an [`ExportBackendPolicy::Auto`] backend (respecting `RAWMAKASE_EXPORT_GPU=0`).
    pub fn auto() -> Self {
        Self::new(ExportBackendPolicy::Auto)
    }

    /// Creates a deterministic [`ExportBackendPolicy::CpuOnly`] backend.
    pub fn cpu_only() -> Self {
        Self::new(ExportBackendPolicy::CpuOnly)
    }

    /// Creates a backend honoring `RAWMAKASE_EXPORT_GPU=0|1`, falling back to `default`
    /// when the environment variable is unset or unrecognized.
    pub fn from_env(default: ExportBackendPolicy) -> Self {
        let env_val = std::env::var(EXPORT_GPU_ENV).ok();
        let (effective, fallback) = resolve_policy(default, env_val.as_deref(), true);
        Self::build(effective, fallback)
    }

    fn lock_exec(&self) -> std::sync::MutexGuard<'_, ExecState> {
        self.exec.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_meta(&self) -> std::sync::MutexGuard<'_, StatusState> {
        self.meta.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns a snapshot of adapter state, fallback reason, and render telemetry.
    ///
    /// Never blocks on an in-flight GPU export.
    pub fn status(&self) -> ExportBackendStatus {
        self.lock_meta().status()
    }

    /// Resets cumulative per-batch render and fallback counters while preserving device state.
    pub fn reset_stats(&self) {
        let mut meta = self.lock_meta();
        meta.gpu_renders = 0;
        meta.cpu_renders = 0;
        meta.fallback_counts.clear();
    }

    /// Releases reusable GPU photo and band working buffers to return VRAM to the OS
    /// when idle, while keeping the device and compiled compute pipelines resident.
    pub fn release_buffers(&self) {
        let mut exec = self.lock_exec();
        match &mut exec.engine {
            Engine::Gpu(ctx) => ctx.release_buffers(),
            #[cfg(test)]
            Engine::Mock(mock) => mock.release_buffers(),
            Engine::Uninitialized | Engine::Disabled => {}
        }
    }

    /// Renders an SDR export of `image` with `recipe` in `space`, returning pixels in
    /// the requested `format` (`F32`, `Rgb8`, or `Rgb16`).
    ///
    /// Uses the dedicated GPU compute backend when enabled and eligible, falling back
    /// deterministically to [`crate::develop::render_export`] on unsupported recipes,
    /// adapter limits, or GPU errors. Never mutates `image`.
    #[allow(clippy::too_many_arguments)]
    pub fn render_export(
        &self,
        image: &CameraImage,
        recipe: &ValidRecipe,
        ai: Option<&AiSource>,
        max_edge: u32,
        space: OutputSpace,
        format: ExportFormat,
        cancel: Cancel<'_>,
    ) -> Result<ExportImage> {
        ensure!(!cancel.is_cancelled(), "Render superseded");

        // Strip Point Color's Visualize Range before both GPU and CPU paths.
        let (stripped, checked);
        let recipe = if recipe
            .point_colors
            .iter()
            .any(|p| p.view != SwatchView::Adjust)
        {
            let mut r = (**recipe).clone();
            without_visualization(&mut r.point_colors);
            stripped = r;
            checked = stripped.checked()?;
            &checked
        } else {
            recipe
        };

        // Fast check under meta lock: if configured CPU-only or already session-disabled,
        // run CPU directly without touching the GPU execution lock.
        let session_disabled = {
            let meta = self.lock_meta();
            (!meta.gpu_active && !meta.uninitialized).then(|| {
                meta.session_fallback
                    .as_ref()
                    .map_or(ExportFallbackKind::ConfiguredCpu, |(k, _)| *k)
            })
        };
        if let Some(kind) = session_disabled {
            let out = crate::develop::render_export(image, recipe, ai, max_edge, space, cancel)?
                .into_format(format);
            let mut meta = self.lock_meta();
            meta.cpu_renders += 1;
            meta.record_fallback(kind);
            return Ok(out);
        }

        // Resolve recipe and check static GPU eligibility BEFORE doing retouch/NR work,
        // so unsupported recipes go straight to the CPU reference with zero wasted work.
        let resolved = quality::resolve(image, recipe);
        if let Some(reason) = unsupported_recipe_reason(resolved.recipe.as_ref()) {
            let _ = reason;
            let out = crate::develop::render_export(image, recipe, ai, max_edge, space, cancel)?
                .into_format(format);
            let mut meta = self.lock_meta();
            meta.cpu_renders += 1;
            meta.record_fallback(ExportFallbackKind::UnsupportedRecipe);
            return Ok(out);
        }

        // Validate camera profile & active mask rasters early (hard errors on both paths).
        let r = resolved.recipe.as_ref();
        if let Some(p) = &r.profile {
            p.ensure_camera(&image.metadata)?;
        }
        if r.lens_ca {
            crate::lens::auto_ca::prime(image);
        }
        for m in r.masks.iter().filter(|m| m.is_active()) {
            rawmakase_model::storage::mask_assets::ensure_shaped(
                crate::model::masks::bitmap_refs(std::slice::from_ref(m)),
            )?;
        }

        let g = resolved.geometry;
        let scale = image.photo_scale();
        let (out_w, out_h) = quality::output_size(g.width, g.height, max_edge);

        let mut base = r.clone();
        base.sharpening = 0.;
        base.validate()?;

        // Prepare highlight recovery and optional AI Denoise blend on CPU (neither mutates
        // `image`). Color NR, Luminance NR, measurement box-reduction and Texture run on
        // the GPU inside `execute_on_gpu` so fallback never duplicates pyramid work.
        let recovered = quality::recovered(image, cancel)?;
        ensure!(!cancel.is_cancelled(), "Render superseded");

        let denoise = r.denoise_with(ai.filter(|a| a.fits(image)));
        let ai_base = match denoise.as_ref().and_then(|d| d.ai.as_ref()) {
            Some(ai_blend) => ai_blend.apply(&recovered),
            None => recovered,
        };
        ensure!(!cancel.is_cancelled(), "Render superseded");

        let job = PreparedExport {
            ai_base: &ai_base,
            denoise,
            base: &base,
            recipe: r,
            geometry: g,
            scale,
            out_w,
            out_h,
            space,
            format,
        };

        // Acquire the serialized GPU execution lock (`meta` remains free for `status()`).
        #[cfg(test)]
        self.exec_asked.fetch_add(1, Ordering::SeqCst);
        let (gpu_outcome, retired) = {
            let mut exec = self.lock_exec();
            let guard = PanicGuard::new(&mut exec, &self.meta);
            let outcome = execute_on_gpu(guard.exec, &self.meta, job, cancel);
            let retired = match &outcome {
                Err(GpuFailure::Session(kind, reason)) => {
                    let old = std::mem::replace(&mut guard.exec.engine, Engine::Disabled);
                    self.lock_meta().mark_disabled(*kind, reason.clone());
                    Some((old, *kind == ExportFallbackKind::DeviceLostOrTimeout))
                }
                _ => None,
            };
            guard.disarm();
            (outcome, retired)
        };
        if let Some((old_engine, async_drop)) = retired {
            retire_engine(old_engine, async_drop);
        }

        match gpu_outcome {
            Ok(exported) => {
                self.lock_meta().gpu_renders += 1;
                Ok(exported)
            }
            Err(GpuFailure::Cancelled) => Err(anyhow!("Render superseded")),
            Err(GpuFailure::Hard(err)) => Err(err),
            Err(GpuFailure::PerPhoto(kind, _reason)) => {
                let out =
                    crate::develop::render_export(image, recipe, ai, max_edge, space, cancel)?
                        .into_format(format);
                let mut meta = self.lock_meta();
                meta.cpu_renders += 1;
                meta.record_fallback(kind);
                Ok(out)
            }
            Err(GpuFailure::OutOfMemory(_reason)) => {
                let out =
                    crate::develop::render_export(image, recipe, ai, max_edge, space, cancel)?
                        .into_format(format);
                let mut meta = self.lock_meta();
                meta.cpu_renders += 1;
                meta.record_fallback(ExportFallbackKind::OutOfMemory);
                Ok(out)
            }
            Err(GpuFailure::Session(kind, _reason)) => {
                let out =
                    crate::develop::render_export(image, recipe, ai, max_edge, space, cancel)?
                        .into_format(format);
                let mut meta = self.lock_meta();
                meta.cpu_renders += 1;
                meta.record_fallback(kind);
                Ok(out)
            }
        }
    }

    /// Renders an HDR master (`HdrImage`, linear ProPhoto RGB, `1.0 = graphics white`) of
    /// `image` with `recipe` within `max_edge` (`0` for full size).
    ///
    /// Uses the dedicated GPU compute backend when enabled and eligible (matching
    /// [`crate::develop::render_hdr`]: unclipped sRGB-curve sharpening, unclipped Lanczos3
    /// resizing, and `roll_off` to the edit's HDR peak), falling back deterministically to
    /// [`crate::develop::render_hdr`] on unsupported HDR features (grain, vignette, defringe,
    /// masks, Point Color), adapter limits, or GPU errors. Never mutates `image`.
    pub fn render_hdr(
        &self,
        image: &CameraImage,
        recipe: &ValidRecipe,
        ai: Option<&AiSource>,
        max_edge: u32,
        cancel: Cancel<'_>,
    ) -> Result<crate::rendered::HdrImage> {
        ensure!(!cancel.is_cancelled(), "Render superseded");
        ensure!(recipe.hdr.is_active(), "This edit has HDR off");
        crate::model::hdr::check_for(recipe, &image.metadata)?;

        let session_disabled = {
            let meta = self.lock_meta();
            (!meta.gpu_active && !meta.uninitialized).then(|| {
                meta.session_fallback
                    .as_ref()
                    .map_or(ExportFallbackKind::ConfiguredCpu, |(k, _)| *k)
            })
        };
        if let Some(kind) = session_disabled {
            let out = crate::develop::render_hdr(image, recipe, ai, max_edge, cancel)?;
            let mut meta = self.lock_meta();
            meta.cpu_renders += 1;
            meta.record_fallback(kind);
            return Ok(out);
        }

        let mut resolved = recipe.resolved_hdr(&image.metadata);
        let scene_owned;
        let (scene_img, effective_ai) = if image.metadata.hdr_source.is_some() {
            let gain = quality::hdr_file_recipe(resolved.to_mut(), &image.metadata);
            scene_owned = quality::hdr_file_scene(image, gain)?;
            (&scene_owned, None)
        } else {
            (image, ai)
        };
        ensure!(
            resolved
                .profile
                .as_ref()
                .is_some_and(|p| p.hdr_tone().is_ok()),
            "This photo's camera profile does not render in HDR"
        );

        let r = resolved.as_ref();
        if !pixel_params::supported_hdr(r, &scene_img.metadata) {
            let out = crate::develop::render_hdr(image, recipe, ai, max_edge, cancel)?;
            let mut meta = self.lock_meta();
            meta.cpu_renders += 1;
            meta.record_fallback(ExportFallbackKind::UnsupportedRecipe);
            return Ok(out);
        }

        if let Some(p) = &r.profile {
            p.ensure_camera(&scene_img.metadata)?;
        }
        if r.lens_ca {
            crate::lens::auto_ca::prime(scene_img);
        }

        let g = Geometry::new(scene_img, r, 0);
        let scale = scene_img.photo_scale();
        let (out_w, out_h) = quality::output_size(g.width, g.height, max_edge);
        let mut base = r.clone();
        base.sharpening = 0.;
        base.validate()?;

        let recovered = quality::recovered(scene_img, cancel)?;
        ensure!(!cancel.is_cancelled(), "Render superseded");

        let denoise = r.denoise_with(effective_ai.filter(|a| a.fits(scene_img)));
        let ai_base = match denoise.as_ref().and_then(|d| d.ai.as_ref()) {
            Some(ai_blend) => ai_blend.apply(&recovered),
            None => recovered,
        };
        ensure!(!cancel.is_cancelled(), "Render superseded");

        let job = PreparedHdrExport {
            ai_base: &ai_base,
            denoise,
            base: &base,
            recipe: r,
            geometry: g,
            scale,
            out_w,
            out_h,
            peak: r.hdr.peak(),
        };

        let (gpu_outcome, retired) = {
            let mut exec = self.lock_exec();
            let guard = PanicGuard::new(&mut exec, &self.meta);
            let outcome = execute_hdr_on_gpu(guard.exec, &self.meta, job, cancel);
            let retired = match &outcome {
                Err(GpuFailure::Session(kind, reason)) => {
                    let old = std::mem::replace(&mut guard.exec.engine, Engine::Disabled);
                    self.lock_meta().mark_disabled(*kind, reason.clone());
                    Some((old, *kind == ExportFallbackKind::DeviceLostOrTimeout))
                }
                _ => None,
            };
            guard.disarm();
            (outcome, retired)
        };
        if let Some((old_engine, async_drop)) = retired {
            retire_engine(old_engine, async_drop);
        }

        match gpu_outcome {
            Ok(master) => {
                self.lock_meta().gpu_renders += 1;
                Ok(master)
            }
            Err(GpuFailure::Cancelled) => Err(anyhow!("Render superseded")),
            Err(GpuFailure::Hard(err)) => Err(err),
            Err(GpuFailure::PerPhoto(kind, _reason)) => {
                let out = crate::develop::render_hdr(image, recipe, ai, max_edge, cancel)?;
                let mut meta = self.lock_meta();
                meta.cpu_renders += 1;
                meta.record_fallback(kind);
                Ok(out)
            }
            Err(GpuFailure::OutOfMemory(_reason)) => {
                let out = crate::develop::render_hdr(image, recipe, ai, max_edge, cancel)?;
                let mut meta = self.lock_meta();
                meta.cpu_renders += 1;
                meta.record_fallback(ExportFallbackKind::OutOfMemory);
                Ok(out)
            }
            Err(GpuFailure::Session(kind, _reason)) => {
                let out = crate::develop::render_hdr(image, recipe, ai, max_edge, cancel)?;
                let mut meta = self.lock_meta();
                meta.cpu_renders += 1;
                meta.record_fallback(kind);
                Ok(out)
            }
        }
    }

    /// Renders both the HDR master (`HdrImage`) and the SDR sRGB base (`ExportImage`,
    /// `ExportFormat::F32`) for a gain-map export in a single GPU pass when eligible,
    /// sharing one photo upload, one GPU noise-reduction/Texture pyramid pass, and one
    /// measurement copy between both renditions.
    ///
    /// Falls back cleanly to separate [`Self::render_hdr`] and [`Self::render_export`]
    /// calls when GPU is disabled, when `image` is an HDR file (whose master uses a
    /// distinct pseudo-scene), or when either rendition requires CPU fallback.
    pub fn render_hdr_and_sdr(
        &self,
        image: &CameraImage,
        recipe: &ValidRecipe,
        ai: Option<&AiSource>,
        max_edge: u32,
        cancel: Cancel<'_>,
    ) -> Result<(crate::rendered::HdrImage, ExportImage)> {
        ensure!(!cancel.is_cancelled(), "Render superseded");
        ensure!(recipe.hdr.is_active(), "This edit has HDR off");
        crate::model::hdr::check_for(recipe, &image.metadata)?;

        // Strip Point Color's Visualize Range before both GPU and CPU paths.
        let (stripped, checked);
        let recipe = if recipe
            .point_colors
            .iter()
            .any(|p| p.view != SwatchView::Adjust)
        {
            let mut r = (**recipe).clone();
            without_visualization(&mut r.point_colors);
            stripped = r;
            checked = stripped.checked()?;
            &checked
        } else {
            recipe
        };

        let session_disabled = {
            let meta = self.lock_meta();
            !meta.gpu_active && !meta.uninitialized
        };
        if session_disabled || image.metadata.hdr_source.is_some() {
            let master = self.render_hdr(image, recipe, ai, max_edge, cancel)?;
            let base = self.render_export(
                image,
                recipe,
                ai,
                max_edge,
                OutputSpace::Srgb,
                ExportFormat::F32,
                cancel,
            )?;
            return Ok((master, base));
        }

        let resolved_hdr = recipe.resolved_hdr(&image.metadata);
        let resolved_sdr = quality::resolve(image, recipe);
        ensure!(
            resolved_hdr
                .profile
                .as_ref()
                .is_some_and(|p| p.hdr_tone().is_ok()),
            "This photo's camera profile does not render in HDR"
        );

        let r_hdr = resolved_hdr.as_ref();
        let r_sdr = resolved_sdr.recipe.as_ref();
        let g_hdr = Geometry::new(image, r_hdr, 0);
        let g_sdr = resolved_sdr.geometry;
        let hdr_denoise = r_hdr.denoise_with(ai.filter(|a| a.fits(image)));
        let sdr_denoise = r_sdr.denoise_with(ai.filter(|a| a.fits(image)));
        let can_share = pixel_params::supported_hdr(r_hdr, &image.metadata)
            && unsupported_recipe_reason(r_sdr).is_none()
            && g_hdr == g_sdr
            && hdr_denoise == sdr_denoise
            && Retouching::of(r_hdr) == Retouching::of(r_sdr)
            && texture::measured(r_hdr) == texture::measured(r_sdr)
            && r_hdr.file_lens_correction_left_out(&image.metadata)
                == r_sdr.file_lens_correction_left_out(&image.metadata);
        if !can_share {
            let master = self.render_hdr(image, recipe, ai, max_edge, cancel)?;
            let base = self.render_export(
                image,
                recipe,
                ai,
                max_edge,
                OutputSpace::Srgb,
                ExportFormat::F32,
                cancel,
            )?;
            return Ok((master, base));
        }

        if let Some(p) = &r_hdr.profile {
            p.ensure_camera(&image.metadata)?;
        }
        if let Some(p) = &r_sdr.profile {
            p.ensure_camera(&image.metadata)?;
        }
        if r_hdr.lens_ca || r_sdr.lens_ca {
            crate::lens::auto_ca::prime(image);
        }
        for m in r_sdr.masks.iter().filter(|m| m.is_active()) {
            rawmakase_model::storage::mask_assets::ensure_shaped(
                crate::model::masks::bitmap_refs(std::slice::from_ref(m)),
            )?;
        }

        let scale = image.photo_scale();
        let (out_w, out_h) = quality::output_size(g_hdr.width, g_hdr.height, max_edge);

        let mut base_hdr = r_hdr.clone();
        base_hdr.sharpening = 0.;
        base_hdr.validate()?;
        let mut base_sdr = r_sdr.clone();
        base_sdr.sharpening = 0.;
        base_sdr.validate()?;

        let recovered = quality::recovered(image, cancel)?;
        ensure!(!cancel.is_cancelled(), "Render superseded");
        let ai_base = match hdr_denoise.as_ref().and_then(|d| d.ai.as_ref()) {
            Some(ai_blend) => ai_blend.apply(&recovered),
            None => recovered,
        };
        ensure!(!cancel.is_cancelled(), "Render superseded");

        let hdr_job = PreparedHdrExport {
            ai_base: &ai_base,
            denoise: hdr_denoise,
            base: &base_hdr,
            recipe: r_hdr,
            geometry: g_hdr,
            scale,
            out_w,
            out_h,
            peak: r_hdr.hdr.peak(),
        };
        let sdr_job = PreparedExport {
            ai_base: &ai_base,
            denoise: sdr_denoise,
            base: &base_sdr,
            recipe: r_sdr,
            geometry: g_sdr,
            scale,
            out_w,
            out_h,
            space: OutputSpace::Srgb,
            format: ExportFormat::F32,
        };

        let (gpu_outcome, retired) = {
            let mut exec = self.lock_exec();
            let guard = PanicGuard::new(&mut exec, &self.meta);
            let outcome = execute_pair_on_gpu(guard.exec, &self.meta, hdr_job, sdr_job, cancel);
            let retired = match &outcome {
                Err(GpuFailure::Session(kind, reason)) => {
                    let old = std::mem::replace(&mut guard.exec.engine, Engine::Disabled);
                    self.lock_meta().mark_disabled(*kind, reason.clone());
                    Some((old, *kind == ExportFallbackKind::DeviceLostOrTimeout))
                }
                _ => None,
            };
            guard.disarm();
            (outcome, retired)
        };
        if let Some((old_engine, async_drop)) = retired {
            retire_engine(old_engine, async_drop);
        }

        match gpu_outcome {
            Ok((master, base)) => {
                self.lock_meta().gpu_renders += 1;
                Ok((master, base))
            }
            Err(GpuFailure::Cancelled) => Err(anyhow!("Render superseded")),
            Err(GpuFailure::Hard(err)) => Err(err),
            Err(GpuFailure::PerPhoto(kind, _)) | Err(GpuFailure::Session(kind, _)) => {
                let master = crate::develop::render_hdr(image, recipe, ai, max_edge, cancel)?;
                let base = crate::develop::render_export(
                    image,
                    recipe,
                    ai,
                    max_edge,
                    OutputSpace::Srgb,
                    cancel,
                )?;
                let mut meta = self.lock_meta();
                meta.cpu_renders += 1;
                meta.record_fallback(kind);
                Ok((master, base))
            }
            Err(GpuFailure::OutOfMemory(_)) => {
                let master = crate::develop::render_hdr(image, recipe, ai, max_edge, cancel)?;
                let base = crate::develop::render_export(
                    image,
                    recipe,
                    ai,
                    max_edge,
                    OutputSpace::Srgb,
                    cancel,
                )?;
                let mut meta = self.lock_meta();
                meta.cpu_renders += 1;
                meta.record_fallback(ExportFallbackKind::OutOfMemory);
                Ok((master, base))
            }
        }
    }
}

/// Checks whether a resolved recipe uses any feature not supported on the GPU export path.
fn unsupported_recipe_reason(r: &Recipe) -> Option<&'static str> {
    if !pixel_params::supported(r) {
        return Some("Recipe is not supported by GPU develop");
    }
    let active_masks: Vec<_> = r.masks.iter().filter(|m| m.is_active()).collect();
    if active_masks.len() > crate::model::masks::MAX_GROUPS {
        return Some("Too many active masks for GPU develop");
    }
    for m in active_masks {
        let a = &m.adjust;
        if m.components.iter().any(|c| c.shape.is_range()) {
            return Some("Range masks fall back to CPU");
        }
        if [a.texture, a.clarity, a.sharpness, a.noise]
            .iter()
            .any(|&v| v != 0.)
        {
            return Some("Local detail masks fall back to CPU");
        }
    }
    None
}

struct PreparedExport<'a> {
    ai_base: &'a Arc<CameraImage>,
    denoise: Option<Denoise>,
    base: &'a Recipe,
    recipe: &'a Recipe,
    geometry: Geometry,
    scale: f32,
    out_w: u32,
    out_h: u32,
    space: OutputSpace,
    format: ExportFormat,
}

struct PreparedHdrExport<'a> {
    ai_base: &'a Arc<CameraImage>,
    denoise: Option<Denoise>,
    base: &'a Recipe,
    recipe: &'a Recipe,
    geometry: Geometry,
    scale: f32,
    out_w: u32,
    out_h: u32,
    peak: f32,
}

#[derive(Clone, Copy)]
enum ExportTarget {
    Sdr {
        space: OutputSpace,
        format: ExportFormat,
    },
    Hdr {
        peak: f32,
    },
}

enum ExportOutput {
    Sdr(ExportImage),
    Hdr(crate::rendered::HdrImage),
}

/// Conservative source-row range `[y0, y1)` in `0..source_img.height` that `local.wgsl`
/// may read while sampling output band `[0..geometry.width, y_comp0..y_comp1)` through
/// `geometry` and optional `recipe` lens distortion / lateral CA / X-Trans safeguard.
pub(crate) fn source_strip_bounds(
    source_img: &CameraImage,
    recipe: &Recipe,
    geometry: &Geometry,
    y_comp0: u32,
    y_comp1: u32,
) -> (u32, u32) {
    let sh = source_img.height;
    if sh == 0 || geometry.width == 0 || y_comp1 <= y_comp0 {
        return (0, sh.min(1));
    }
    let lens = crate::develop::image_space::LensMap::new(source_img, recipe);
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let fw = geometry.width as f32;
    let fh = geometry.height as f32;

    let mut visit = |ox: u32, oy: u32| {
        let [ax, ay] = geometry.source((ox as f32 + 0.5) / fw, (oy as f32 + 0.5) / fh);
        if let Some(lens) = &lens {
            let ([_, dy], scales) = lens.scales(ax, ay);
            for scale in scales {
                let sy = lens.center[1] + dy * scale - 0.5;
                if sy.is_finite() {
                    min_y = min_y.min(sy);
                    max_y = max_y.max(sy);
                }
            }
        } else if ay.is_finite() {
            min_y = min_y.min(ay);
            max_y = max_y.max(ay);
        }
    };

    // Sample every pixel on the top and bottom edges (catches radial lens bow along X),
    // every row on left and right edges, plus a 32-px interior grid for any interior extremum.
    let y_last = y_comp1 - 1;
    let x_last = geometry.width - 1;
    for ox in 0..geometry.width {
        visit(ox, y_comp0);
        if y_last != y_comp0 {
            visit(ox, y_last);
        }
    }
    for oy in y_comp0..y_comp1 {
        visit(0, oy);
        if x_last != 0 {
            visit(x_last, oy);
        }
    }
    let mut oy = y_comp0.saturating_add(32);
    while oy < y_last {
        let mut ox = 32u32;
        while ox < x_last {
            visit(ox, oy);
            ox = ox.saturating_add(32);
        }
        oy = oy.saturating_add(32);
    }

    if !min_y.is_finite() || !max_y.is_finite() {
        return (0, sh);
    }
    // Bilinear + rounding margin (2 px), plus 18 px when X-Trans CA stripe coherence reads
    // up to ±15 vertical neighbours around the green sample.
    let margin: f32 = if source_img.metadata.xtrans && lens.is_some() {
        20.0
    } else {
        2.0
    };
    let y0 = (min_y.floor() - margin).clamp(0.0, (sh - 1) as f32) as u32;
    let y1 = ((max_y.ceil() + margin + 1.0).clamp(1.0, sh as f32) as u32).max(y0 + 1);
    (y0, y1.min(sh))
}

fn has_auto_mask_brush(r: &Recipe) -> bool {
    r.masks.iter().filter(|m| m.is_active()).any(|m| {
        m.components.iter().any(|c| match &c.shape {
            MaskShape::Brush { strokes } => strokes.iter().any(|s| s.auto_mask),
            _ => false,
        })
    })
}

fn reduced_camera_image(
    base: &CameraImage,
    width: u32,
    height: u32,
    pixels: Vec<[f32; 3]>,
) -> CameraImage {
    CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: pixels.into(),
        metadata: base.metadata.clone(),
        fast: base.fast,
        scale_factor: base.scale_factor,
        scale_clipped: base.scale_clipped,
        hdr: Default::default(),
    }
}

/// Ensures `exec.engine` is lazily initialized (or returns the session fallback reason),
/// then runs `attempt` with one halved-band OOM retry when applicable.
fn run_with_oom_retry<T>(
    exec: &mut ExecState,
    meta: &Mutex<StatusState>,
    geometry: Geometry,
    out_h: u32,
    cancel: Cancel<'_>,
    mut attempt: impl FnMut(&mut Engine, Option<u32>, Option<u64>) -> std::result::Result<T, GpuFailure>,
) -> std::result::Result<T, GpuFailure> {
    if cancel.is_cancelled() {
        return Err(GpuFailure::Cancelled);
    }

    if matches!(exec.engine, Engine::Uninitialized) {
        match GpuExportContext::new() {
            Ok(ctx) => {
                meta.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .mark_active(ctx.name.clone());
                exec.engine = Engine::Gpu(Box::new(ctx));
            }
            Err((kind, reason)) => {
                return Err(GpuFailure::Session(kind, reason));
            }
        }
    }

    if matches!(exec.engine, Engine::Disabled) {
        let (kind, reason) = meta
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .session_fallback
            .clone()
            .unwrap_or((
                ExportFallbackKind::ConfiguredCpu,
                "GPU export disabled".into(),
            ));
        return Err(GpuFailure::PerPhoto(kind, reason));
    }

    #[cfg(test)]
    let initial_cap = exec.band_height_override;
    #[cfg(not(test))]
    let initial_cap: Option<u32> = None;

    #[cfg(test)]
    let max_binding_override = exec.max_binding_override;
    #[cfg(not(test))]
    let max_binding_override: Option<u64> = None;

    let mut band_cap = initial_cap;
    let mut retried_oom = false;

    loop {
        if cancel.is_cancelled() {
            return Err(GpuFailure::Cancelled);
        }
        match attempt(&mut exec.engine, band_cap, max_binding_override) {
            Ok(out) => return Ok(out),
            Err(GpuFailure::OutOfMemory(msg)) => {
                match &mut exec.engine {
                    Engine::Gpu(ctx) => ctx.release_buffers(),
                    #[cfg(test)]
                    Engine::Mock(mock) => mock.release_buffers(),
                    Engine::Uninitialized | Engine::Disabled => {}
                }
                if retried_oom {
                    return Err(GpuFailure::PerPhoto(ExportFallbackKind::OutOfMemory, msg));
                }
                let prev_h = band_cap.unwrap_or_else(|| {
                    default_band_out_height(
                        geometry.width,
                        geometry.height,
                        out_h,
                        DEFAULT_BAND_PIXELS,
                    )
                });
                if prev_h <= 1 {
                    return Err(GpuFailure::PerPhoto(ExportFallbackKind::OutOfMemory, msg));
                }
                retried_oom = true;
                band_cap = Some((prev_h / 2).max(1));
            }
            Err(other) => return Err(other),
        }
    }
}

/// Executes a prepared SDR export on `exec`'s GPU engine.
fn execute_on_gpu(
    exec: &mut ExecState,
    meta: &Mutex<StatusState>,
    job: PreparedExport<'_>,
    cancel: Cancel<'_>,
) -> std::result::Result<ExportImage, GpuFailure> {
    run_with_oom_retry(
        exec,
        meta,
        job.geometry,
        job.out_h,
        cancel,
        |engine, band_cap, max_binding_override| match engine {
            Engine::Gpu(ctx) => ctx.render_banded(&job, band_cap, max_binding_override, cancel),
            #[cfg(test)]
            Engine::Mock(mock) => mock.render_banded(&job, band_cap, cancel),
            Engine::Uninitialized | Engine::Disabled => {
                unreachable!("engine initialized or handled above")
            }
        },
    )
}

/// Executes a prepared HDR master export on `exec`'s GPU engine.
fn execute_hdr_on_gpu(
    exec: &mut ExecState,
    meta: &Mutex<StatusState>,
    job: PreparedHdrExport<'_>,
    cancel: Cancel<'_>,
) -> std::result::Result<crate::rendered::HdrImage, GpuFailure> {
    run_with_oom_retry(
        exec,
        meta,
        job.geometry,
        job.out_h,
        cancel,
        |engine, band_cap, max_binding_override| match engine {
            Engine::Gpu(ctx) => ctx.render_hdr_banded(&job, band_cap, max_binding_override, cancel),
            #[cfg(test)]
            Engine::Mock(mock) => mock.render_hdr_banded(&job, band_cap, cancel),
            Engine::Uninitialized | Engine::Disabled => {
                unreachable!("engine initialized or handled above")
            }
        },
    )
}

/// Executes a shared-source HDR + SDR gain-map pair on `exec`'s GPU engine.
fn execute_pair_on_gpu(
    exec: &mut ExecState,
    meta: &Mutex<StatusState>,
    hdr: PreparedHdrExport<'_>,
    sdr: PreparedExport<'_>,
    cancel: Cancel<'_>,
) -> std::result::Result<(crate::rendered::HdrImage, ExportImage), GpuFailure> {
    run_with_oom_retry(
        exec,
        meta,
        hdr.geometry,
        hdr.out_h,
        cancel,
        |engine, band_cap, max_binding_override| match engine {
            Engine::Gpu(ctx) => {
                ctx.render_pair_banded(&hdr, &sdr, band_cap, max_binding_override, cancel)
            }
            #[cfg(test)]
            Engine::Mock(mock) => mock.render_pair_banded(&hdr, &sdr, band_cap, cancel),
            Engine::Uninitialized | Engine::Disabled => {
                unreachable!("engine initialized or handled above")
            }
        },
    )
}

/// One horizontal strip of an export render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Band {
    /// Output rows `[oy0 .. oy1)` in the target image (`0..out_h`).
    pub(crate) oy0: u32,
    pub(crate) oy1: u32,
    /// Required full-resolution post-spatial rows `[sy_min .. sy_max)` in `0..full_h`.
    pub(crate) sy_min: u32,
    pub(crate) sy_max: u32,
    /// Expanded full-resolution compute rows `[y_comp0 .. y_comp1)` including sharpening halo.
    pub(crate) y_comp0: u32,
    pub(crate) y_comp1: u32,
}

impl Band {
    fn out_h(self) -> u32 {
        self.oy1 - self.oy0
    }
    fn sy_h(self) -> u32 {
        self.sy_max - self.sy_min
    }
    fn comp_h(self) -> u32 {
        self.y_comp1 - self.y_comp0
    }
}

/// Default output rows per band targeting `target_band_pixels` full-resolution compute pixels.
fn default_band_out_height(full_w: u32, full_h: u32, out_h: u32, target_band_pixels: u64) -> u32 {
    let target_src_rows = (target_band_pixels / u64::from(full_w.max(1))).max(1);
    let raw = if out_h == full_h {
        target_src_rows
    } else {
        let ratio_y = f64::from(full_h) / f64::from(out_h.max(1));
        (target_src_rows as f64 / ratio_y).floor().max(1.) as u64
    };
    let min_h = MIN_BAND_HEIGHT.min(u64::from(out_h.max(1)));
    raw.clamp(min_h, u64::from(out_h.max(1))) as u32
}

/// Computes the [`Band`] covering output rows `[oy0 .. oy1)` with exact vertical Lanczos3
/// and sharpening halo overlap.
fn band_for_rows(
    oy0: u32,
    oy1: u32,
    full_h: u32,
    h_sharp: u32,
    y_lanczos: Option<(u32, &[f32])>,
) -> Band {
    let (sy_min, sy_max) = match y_lanczos {
        Some((stride, weights)) => {
            let mut lo = full_h;
            let mut hi = 0u32;
            for oy in oy0..oy1 {
                let off = (oy * stride) as usize;
                let start = weights[off] as u32;
                let count = weights[off + 1] as u32;
                lo = lo.min(start);
                hi = hi.max(start + count);
            }
            (lo.min(full_h.saturating_sub(1)), hi.min(full_h).max(1))
        }
        None => (oy0, oy1),
    };
    let y_comp0 = sy_min.saturating_sub(h_sharp);
    let y_comp1 = sy_max.saturating_add(h_sharp).min(full_h);
    Band {
        oy0,
        oy1,
        sy_min,
        sy_max,
        y_comp0,
        y_comp1,
    }
}

/// Plans contiguous horizontal bands covering `0..out_h` with exact Lanczos3 and
/// sharpening halo vertical overlap.
pub(crate) fn plan_bands(
    full_w: u32,
    full_h: u32,
    out_h: u32,
    h_sharp: u32,
    y_lanczos: Option<(u32, &[f32])>,
    target_band_pixels: u64,
    max_band_height: Option<u32>,
) -> Vec<Band> {
    let mut band_out_h = default_band_out_height(full_w, full_h, out_h, target_band_pixels);
    if let Some(cap) = max_band_height {
        band_out_h = band_out_h.min(cap.max(1)).max(1);
    }
    let mut bands = Vec::with_capacity(out_h.div_ceil(band_out_h) as usize);
    let mut oy0 = 0;
    while oy0 < out_h {
        let oy1 = (oy0 + band_out_h).min(out_h);
        bands.push(band_for_rows(oy0, oy1, full_h, h_sharp, y_lanczos));
        oy0 = oy1;
    }
    bands
}

/// Workgroups `(gx, gy)` for `n` 1D invocations of [`WORKGROUP_1D`] within device limits.
fn groups_1d(max_per_dim: u32, n: u64) -> (u32, u32) {
    let groups = n.div_ceil(u64::from(WORKGROUP_1D)) as u32;
    let gx = groups.min(max_per_dim).max(1);
    let gy = groups.div_ceil(gx).max(1);
    (gx, gy)
}

/// Shared device health flags updated by wgpu's device-lost and uncaptured-error callbacks.
struct DeviceHealth {
    lost: AtomicBool,
    uncaptured: AtomicBool,
    message: Mutex<Option<String>>,
}

impl DeviceHealth {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            lost: AtomicBool::new(false),
            uncaptured: AtomicBool::new(false),
            message: Mutex::new(None),
        })
    }
    fn record_lost(&self, msg: String) {
        self.lost.store(true, Ordering::Release);
        let mut guard = self.message.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            *guard = Some(msg);
        }
    }
    fn record_uncaptured(&self, msg: String) {
        self.uncaptured.store(true, Ordering::Release);
        let mut guard = self.message.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            *guard = Some(msg);
        }
    }
    fn check(&self) -> std::result::Result<(), GpuFailure> {
        if self.lost.load(Ordering::Acquire) {
            let msg = self
                .message
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .unwrap_or_else(|| "GPU export device lost".into());
            return Err(GpuFailure::Session(
                ExportFallbackKind::DeviceLostOrTimeout,
                msg,
            ));
        }
        if self.uncaptured.load(Ordering::Acquire) {
            let msg = self
                .message
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .unwrap_or_else(|| "Uncaptured GPU export error".into());
            return Err(GpuFailure::Session(ExportFallbackKind::DriverError, msg));
        }
        Ok(())
    }
}

/// Reusable GPU buffers for source upload and band working storage.
#[derive(Default)]
struct ExportBuffers {
    photo: Option<wgpu::Buffer>,
    coverage: Option<wgpu::Buffer>,
    sampling: Option<wgpu::Buffer>,
    samples: Option<wgpu::Buffer>,
    positions: Option<wgpu::Buffer>,
    pixels: Option<wgpu::Buffer>,
    scratch: Option<wgpu::Buffer>,
    output: Option<wgpu::Buffer>,
    packed_out: Option<wgpu::Buffer>,
    /// Double-buffered staging buffers so band `k` computes on the GPU while band `k - 1`
    /// maps and copies back to host memory.
    staging: [Option<wgpu::Buffer>; 2],
    params: Option<wgpu::Buffer>,
    tables: Option<wgpu::Buffer>,
    mask_weights: Option<wgpu::Buffer>,
    grid: Option<wgpu::Buffer>,
    finish_weights: Option<wgpu::Buffer>,
}

impl ExportBuffers {
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Live headless GPU compute context owned by [`ExportBackend`].
struct GpuExportContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    health: Arc<DeviceHealth>,
    name: String,
    pyramid: GpuPyramid,
    sample_layout: wgpu::BindGroupLayout,
    sample_pipeline: wgpu::ComputePipeline,
    develop_layout: wgpu::BindGroupLayout,
    develop_pipeline_layout: wgpu::PipelineLayout,
    develop_pipelines: Vec<(Variant, wgpu::ComputePipeline)>,
    finish_layout: wgpu::BindGroupLayout,
    /// `blur_horizontal`, `sharpen`, `spatial_effects`, `resize_vertical`,
    /// `resize_horizontal`, `pack_rgb8`, `pack_rgb16`, `hdr_srgb_encode`,
    /// `hdr_sharpen`, `hdr_resize_horizontal`, `hdr_finish_master`.
    finish_pipelines: [wgpu::ComputePipeline; 11],
    no_kept: wgpu::Buffer,
    buffers: ExportBuffers,
}

impl PyramidHost for GpuExportContext {
    type Error = GpuFailure;

    fn device(&self) -> &wgpu::Device {
        &self.device
    }

    fn submit(&self, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        self.queue.submit([encoder.finish()])
    }

    fn wait(
        &self,
        submission: wgpu::SubmissionIndex,
        cancel: Cancel<'_>,
    ) -> std::result::Result<(), GpuFailure> {
        self.wait_band(submission, cancel)
    }

    fn cancelled(&self) -> GpuFailure {
        GpuFailure::Cancelled
    }

    fn readback_error(&self, message: String) -> GpuFailure {
        GpuFailure::Session(ExportFallbackKind::DeviceLostOrTimeout, message)
    }

    fn exceeds_limits(&self, message: String) -> GpuFailure {
        GpuFailure::PerPhoto(ExportFallbackKind::ExceedsLimits, message)
    }
}

const FINISH_BLUR_H: usize = 0;
const FINISH_SHARPEN: usize = 1;
const FINISH_SPATIAL: usize = 2;
const FINISH_RESIZE_V: usize = 3;
const FINISH_RESIZE_H: usize = 4;
const FINISH_PACK_RGB8: usize = 5;
const FINISH_PACK_RGB16: usize = 6;
const FINISH_HDR_ENCODE: usize = 7;
const FINISH_HDR_SHARPEN: usize = 8;
const FINISH_HDR_RESIZE_H: usize = 9;
const FINISH_HDR_FINISH: usize = 10;

impl GpuExportContext {
    fn new() -> std::result::Result<Self, (ExportFallbackKind, String)> {
        if wgpu::Instance::enabled_backend_features().is_empty() {
            return Err((
                ExportFallbackKind::NoHardwareAdapter,
                "No wgpu backend features enabled in this build".into(),
            ));
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: super::backends(),
            backend_options: super::backend_options(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|e| (ExportFallbackKind::NoHardwareAdapter, e.to_string()))?;

        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            return Err((
                ExportFallbackKind::NoHardwareAdapter,
                format!("Software GPU adapter ({})", info.name),
            ));
        }
        if info.backend == wgpu::Backend::Gl {
            return Err((
                ExportFallbackKind::NoHardwareAdapter,
                format!("OpenGL adapters are not used for compute ({})", info.name),
            ));
        }

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("RAWmakase export compute"),
            required_limits: super::required_limits(&adapter),
            ..Default::default()
        }))
        .map_err(|e| (ExportFallbackKind::NoHardwareAdapter, e.to_string()))?;

        let health = DeviceHealth::new();
        {
            let h = health.clone();
            device.set_device_lost_callback(move |reason, message| {
                if reason != wgpu::DeviceLostReason::Destroyed {
                    h.record_lost(format!("GPU export device lost ({reason:?}): {message}"));
                }
            });
        }
        {
            let h = health.clone();
            device.on_uncaptured_error(Arc::new(move |error| {
                let msg = error.to_string();
                eprintln!("RAWmakase export GPU error: {msg}");
                h.record_uncaptured(msg);
            }));
        }

        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);

        let pyramid = GpuPyramid::new(&device);
        let (sample_layout, sample_pipeline) = Resident::create_sample_pipeline(&device);
        let (develop_layout, develop_pipeline_layout) = Developer::create_layouts(&device);
        let (finish_layout, finish_pipelines) = create_export_finish_pipelines(&device);
        let no_kept = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Export unused kept buffer"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let memory_error = pollster::block_on(oom.pop());
        let internal_error = pollster::block_on(internal.pop());
        let validation_error = pollster::block_on(validation.pop());
        if let Some(error) = memory_error.or(internal_error).or(validation_error) {
            return Err((ExportFallbackKind::DriverError, error.to_string()));
        }
        if let Err(GpuFailure::Session(kind, msg)) = health.check() {
            return Err((kind, msg));
        }

        Ok(Self {
            device,
            queue,
            health,
            name: info.name,
            pyramid,
            sample_layout,
            sample_pipeline,
            develop_layout,
            develop_pipeline_layout,
            develop_pipelines: Vec::new(),
            finish_layout,
            finish_pipelines,
            no_kept,
            buffers: ExportBuffers::default(),
        })
    }

    fn release_buffers(&mut self) {
        self.buffers.clear();
    }

    fn develop_pipeline(&mut self, variant: Variant) -> wgpu::ComputePipeline {
        if let Some((_, p)) = self.develop_pipelines.iter().find(|(v, _)| *v == variant) {
            return p.clone();
        }
        let pipeline =
            Developer::compile_pipeline(&self.device, &self.develop_pipeline_layout, variant);
        self.develop_pipelines.push((variant, pipeline.clone()));
        pipeline
    }

    /// Runs `work` inside nested validation, internal, and out-of-memory error scopes.
    fn scoped<T>(
        &mut self,
        work: impl FnOnce(&mut Self) -> std::result::Result<T, GpuFailure>,
    ) -> std::result::Result<T, GpuFailure> {
        self.health.check()?;
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let oom = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let result = work(self);
        let memory_error = pollster::block_on(oom.pop());
        let internal_error = pollster::block_on(internal.pop());
        let validation_error = pollster::block_on(validation.pop());
        if let Some(failure) = classify_scope_errors(memory_error, internal_error, validation_error)
        {
            self.buffers.clear();
            return Err(failure);
        }
        self.health.check()?;
        result
    }

    /// Ensures a reusable buffer slot holds at least `needed` bytes (dropping old before
    /// allocating a larger one so peak VRAM does not double).
    fn ensure_buffer(
        device: &wgpu::Device,
        slot: &mut Option<wgpu::Buffer>,
        label: &str,
        needed: u64,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        let size = needed.next_multiple_of(4).max(16);
        let reuse = slot.as_ref().is_some_and(|b| {
            b.usage().contains(usage)
                && b.size() >= size
                && b.size() <= size.saturating_mul(2).max(65536)
        });
        if !reuse {
            *slot = None;
            *slot = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            }));
        }
        slot.clone().expect("buffer allocated")
    }

    /// Ensures a reusable storage buffer holds `bytes` and uploads `bytes` via the queue.
    fn upload_storage(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot: &mut Option<wgpu::Buffer>,
        label: &str,
        bytes: &[u8],
    ) -> wgpu::Buffer {
        let buf = Self::ensure_buffer(
            device,
            slot,
            label,
            bytes.len() as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        if !bytes.is_empty() {
            queue.write_buffer(&buf, 0, bytes);
        }
        buf
    }

    /// Polls `submission` in short slices up to [`BAND_POLL_DEADLINE`], checking `cancel`
    /// and device health between slices.
    fn wait_band(
        &self,
        submission: wgpu::SubmissionIndex,
        cancel: Cancel<'_>,
    ) -> std::result::Result<(), GpuFailure> {
        let deadline = Instant::now() + BAND_POLL_DEADLINE;
        loop {
            if cancel.is_cancelled() {
                return Err(GpuFailure::Cancelled);
            }
            self.health.check()?;
            match self.device.poll(wgpu::PollType::Wait {
                submission_index: Some(submission.clone()),
                timeout: Some(BAND_POLL_SLICE),
            }) {
                Ok(_) => return Ok(()),
                Err(wgpu::PollError::Timeout) if Instant::now() < deadline => {}
                Err(wgpu::PollError::Timeout) => {
                    return Err(GpuFailure::Session(
                        ExportFallbackKind::DeviceLostOrTimeout,
                        "GPU export band poll timed out".into(),
                    ));
                }
                Err(err) => {
                    return Err(GpuFailure::Session(
                        ExportFallbackKind::DeviceLostOrTimeout,
                        err.to_string(),
                    ));
                }
            }
        }
    }

    fn render_banded(
        &mut self,
        job: &PreparedExport<'_>,
        band_cap: Option<u32>,
        max_binding_override: Option<u64>,
        cancel: Cancel<'_>,
    ) -> std::result::Result<ExportImage, GpuFailure> {
        let (effective_max_binding, max_wg) = self.effective_limits(max_binding_override);
        let source_img = job.ai_base;
        let use_full_photo = fits_full_photo(source_img, effective_max_binding, max_wg);

        let target = ExportTarget::Sdr {
            space: job.space,
            format: job.format,
        };
        let plan = build_pass_plan(
            source_img,
            job.base,
            job.recipe,
            job.geometry,
            job.scale,
            job.out_w,
            job.out_h,
            target,
            !use_full_photo,
            band_cap,
            effective_max_binding,
            max_wg,
        )?;

        let texture_amount = texture::measured(job.base);
        let file_correction = !plan
            .base_no_tex
            .file_lens_correction_left_out(&source_img.metadata);
        let needs_auto_mask_guide = plan.has_masks && has_auto_mask_brush(&plan.base_no_tex);

        self.scoped(|ctx| {
            let prepared = prepare_source_on_gpu(
                ctx,
                source_img,
                job.denoise.as_ref(),
                Retouching::of(job.recipe),
                texture_amount,
                job.scale,
                file_correction,
                needs_auto_mask_guide,
                use_full_photo,
                effective_max_binding,
                cancel,
            )?;
            match run_banded_pass(
                ctx,
                source_img,
                &prepared,
                &plan,
                effective_max_binding,
                max_wg,
                cancel,
            )? {
                ExportOutput::Sdr(img) => Ok(img),
                ExportOutput::Hdr(_) => unreachable!("SDR pass returned HDR output"),
            }
        })
    }

    fn render_hdr_banded(
        &mut self,
        job: &PreparedHdrExport<'_>,
        band_cap: Option<u32>,
        max_binding_override: Option<u64>,
        cancel: Cancel<'_>,
    ) -> std::result::Result<crate::rendered::HdrImage, GpuFailure> {
        let (effective_max_binding, max_wg) = self.effective_limits(max_binding_override);
        let source_img = job.ai_base;
        let use_full_photo = fits_full_photo(source_img, effective_max_binding, max_wg);

        let target = ExportTarget::Hdr { peak: job.peak };
        let plan = build_pass_plan(
            source_img,
            job.base,
            job.recipe,
            job.geometry,
            job.scale,
            job.out_w,
            job.out_h,
            target,
            !use_full_photo,
            band_cap,
            effective_max_binding,
            max_wg,
        )?;

        let texture_amount = texture::measured(job.base);
        let file_correction = !plan
            .base_no_tex
            .file_lens_correction_left_out(&source_img.metadata);

        self.scoped(|ctx| {
            let prepared = prepare_source_on_gpu(
                ctx,
                source_img,
                job.denoise.as_ref(),
                Retouching::of(job.recipe),
                texture_amount,
                job.scale,
                file_correction,
                false,
                use_full_photo,
                effective_max_binding,
                cancel,
            )?;
            match run_banded_pass(
                ctx,
                source_img,
                &prepared,
                &plan,
                effective_max_binding,
                max_wg,
                cancel,
            )? {
                ExportOutput::Hdr(img) => Ok(img),
                ExportOutput::Sdr(_) => unreachable!("HDR pass returned SDR output"),
            }
        })
    }

    fn render_pair_banded(
        &mut self,
        hdr: &PreparedHdrExport<'_>,
        sdr: &PreparedExport<'_>,
        band_cap: Option<u32>,
        max_binding_override: Option<u64>,
        cancel: Cancel<'_>,
    ) -> std::result::Result<(crate::rendered::HdrImage, ExportImage), GpuFailure> {
        let (effective_max_binding, max_wg) = self.effective_limits(max_binding_override);
        let source_img = hdr.ai_base;
        let use_full_photo = fits_full_photo(source_img, effective_max_binding, max_wg);

        let hdr_plan = build_pass_plan(
            source_img,
            hdr.base,
            hdr.recipe,
            hdr.geometry,
            hdr.scale,
            hdr.out_w,
            hdr.out_h,
            ExportTarget::Hdr { peak: hdr.peak },
            !use_full_photo,
            band_cap,
            effective_max_binding,
            max_wg,
        )?;
        let sdr_plan = build_pass_plan(
            source_img,
            sdr.base,
            sdr.recipe,
            sdr.geometry,
            sdr.scale,
            sdr.out_w,
            sdr.out_h,
            ExportTarget::Sdr {
                space: sdr.space,
                format: sdr.format,
            },
            !use_full_photo,
            band_cap,
            effective_max_binding,
            max_wg,
        )?;

        let texture_amount = texture::measured(hdr.base);
        let file_correction = !hdr_plan
            .base_no_tex
            .file_lens_correction_left_out(&source_img.metadata);
        let needs_auto_mask_guide =
            sdr_plan.has_masks && has_auto_mask_brush(&sdr_plan.base_no_tex);

        self.scoped(|ctx| {
            let prepared = prepare_source_on_gpu(
                ctx,
                source_img,
                hdr.denoise.as_ref(),
                Retouching::of(hdr.recipe),
                texture_amount,
                hdr.scale,
                file_correction,
                needs_auto_mask_guide,
                use_full_photo,
                effective_max_binding,
                cancel,
            )?;
            let master = match run_banded_pass(
                ctx,
                source_img,
                &prepared,
                &hdr_plan,
                effective_max_binding,
                max_wg,
                cancel,
            )? {
                ExportOutput::Hdr(img) => img,
                ExportOutput::Sdr(_) => unreachable!("HDR pass returned SDR output"),
            };
            let base = match run_banded_pass(
                ctx,
                source_img,
                &prepared,
                &sdr_plan,
                effective_max_binding,
                max_wg,
                cancel,
            )? {
                ExportOutput::Sdr(img) => img,
                ExportOutput::Hdr(_) => unreachable!("SDR pass returned HDR output"),
            };
            Ok((master, base))
        })
    }

    fn effective_limits(&self, max_binding_override: Option<u64>) -> (u64, u32) {
        let limits = self.device.limits();
        let hw_max_binding = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let effective = max_binding_override.map_or(hw_max_binding, |o| o.min(hw_max_binding));
        (effective, limits.max_compute_workgroups_per_dimension)
    }
}

fn fits_full_photo(source_img: &CameraImage, effective_max_binding: u64, max_wg: u32) -> bool {
    let source_bytes = source_img.pixels.len() as u64 * 12;
    source_bytes <= effective_max_binding
        && source_img.width.div_ceil(16) <= max_wg
        && source_img.height.div_ceil(16) <= max_wg
}

/// Precomputed geometry, weights, band layout, and optional source-strip ranges for one
/// SDR or HDR export pass.
struct PassPlan<'a> {
    recipe: &'a Recipe,
    base_no_tex: Recipe,
    geometry: Geometry,
    scale: f32,
    out_w: u32,
    out_h: u32,
    resize: bool,
    target: ExportTarget,
    sharpen: bool,
    sharpener: Sharpener,
    radius: u32,
    finish_weights: Vec<f32>,
    x_offset: u32,
    x_stride: u32,
    y_offset: u32,
    y_stride: u32,
    effects: bool,
    vignette: Option<PostCropVignette>,
    grain: GrainField,
    has_masks: bool,
    bands: Vec<Band>,
    /// Per-band `[src_y0, src_y1)` ranges when operating in source-strip upload mode.
    strip_ranges: Option<Vec<(u32, u32)>>,
}

#[allow(clippy::too_many_arguments)]
fn build_pass_plan<'a>(
    source_img: &CameraImage,
    base: &Recipe,
    recipe: &'a Recipe,
    geometry: Geometry,
    scale: f32,
    out_w: u32,
    out_h: u32,
    target: ExportTarget,
    strip_mode: bool,
    band_cap: Option<u32>,
    effective_max_binding: u64,
    max_wg: u32,
) -> std::result::Result<PassPlan<'a>, GpuFailure> {
    let full_w = geometry.width;
    let full_h = geometry.height;
    let resize = (out_w, out_h) != (full_w, full_h);

    let coverage_bytes = source_img
        .metadata
        .coverage
        .as_ref()
        .map_or(16, |c| (c.values.len() as u64).next_multiple_of(4).max(16));
    if coverage_bytes > effective_max_binding
        || full_w.div_ceil(16) > max_wg
        || out_w.div_ceil(16) > max_wg
    {
        return Err(GpuFailure::PerPhoto(
            ExportFallbackKind::ExceedsLimits,
            "Photo exceeds GPU storage or dispatch limits".into(),
        ));
    }

    let sharpen = recipe.sharpening > 0.;
    let sharpener = Sharpener::new(recipe);
    let sigma = sharpener.sigma * scale;
    let (radius, mut finish_weights) = if sharpen {
        let (r, w) = quality::gaussian(sigma);
        (r as u32, w)
    } else {
        (0u32, vec![1.0f32])
    };
    let x_offset = finish_weights.len() as u32;
    let (x_stride, y_stride, y_offset, y_lanczos_vec) = if resize {
        let (xs, xw) = weights::lanczos_axis(full_w, out_w);
        finish_weights.extend(xw);
        let yo = finish_weights.len() as u32;
        let (ys, yw) = weights::lanczos_axis(full_h, out_h);
        finish_weights.extend(&yw);
        (xs, ys, yo, Some(yw))
    } else {
        (0, 0, x_offset, None)
    };
    if (finish_weights.len() as u64 * 4) > effective_max_binding {
        return Err(GpuFailure::PerPhoto(
            ExportFallbackKind::ExceedsLimits,
            "Finish weight table exceeds GPU storage buffer limit".into(),
        ));
    }

    let mut base_no_tex = base.clone();
    if texture::measured(base) != 0. {
        base_no_tex.effects.texture = 0.;
    }

    let format = match target {
        ExportTarget::Sdr { format, .. } => format,
        ExportTarget::Hdr { .. } => ExportFormat::F32,
    };

    // Plan horizontal bands, shrinking band height if any band's buffers (or source strip
    // upload in strip mode) exceed adapter limits. Strip bounds are evaluated lazily so a
    // band that cannot fit even at 1 row (e.g. a 90-degree-rotated large sensor) bails out
    // immediately without scanning all bands at every halving.
    let mut cap = band_cap;
    let (bands, strip_ranges) = loop {
        let y_slice = y_lanczos_vec.as_deref().map(|w| (y_stride, w));
        let candidate = plan_bands(
            full_w,
            full_h,
            out_h,
            radius,
            y_slice,
            DEFAULT_BAND_PIXELS,
            cap,
        );
        let mut strips = strip_mode.then(|| Vec::with_capacity(candidate.len()));
        let mut fits = true;
        for b in &candidate {
            let comp_n = u64::from(full_w) * u64::from(b.comp_h());
            let out_n = u64::from(out_w) * u64::from(b.out_h());
            let scratch_bytes = (comp_n * 4).max(if resize {
                u64::from(full_w) * u64::from(b.out_h()) * 12
            } else {
                0
            });
            let readback_bytes = match format {
                ExportFormat::F32 => out_n * 12,
                ExportFormat::Rgb8 => (out_n * 3).next_multiple_of(4),
                ExportFormat::Rgb16 => (out_n * 6).next_multiple_of(4),
            };
            if comp_n * 12 > effective_max_binding
                || scratch_bytes > effective_max_binding
                || out_n * 12 > effective_max_binding
                || readback_bytes > effective_max_binding
                || b.comp_h().div_ceil(16) > max_wg
                || b.out_h().div_ceil(16) > max_wg
            {
                fits = false;
                break;
            }
            if let Some(ranges) = strips.as_mut() {
                let (sy0, sy1) =
                    source_strip_bounds(source_img, &base_no_tex, &geometry, b.y_comp0, b.y_comp1);
                if u64::from(source_img.width) * u64::from(sy1 - sy0) * 12 > effective_max_binding {
                    // Check whether even a single-row band at `b.oy0` exceeds limits; if so,
                    // no smaller band height can fit, so bail out without further halving.
                    if b.out_h() > 1 {
                        let single = band_for_rows(b.oy0, b.oy0 + 1, full_h, radius, y_slice);
                        let (r0, r1) = source_strip_bounds(
                            source_img,
                            &base_no_tex,
                            &geometry,
                            single.y_comp0,
                            single.y_comp1,
                        );
                        if u64::from(source_img.width) * u64::from(r1 - r0) * 12
                            > effective_max_binding
                        {
                            return Err(GpuFailure::PerPhoto(
                                ExportFallbackKind::ExceedsLimits,
                                "Single-row band exceeds GPU storage or dispatch limits".into(),
                            ));
                        }
                    }
                    fits = false;
                    break;
                }
                ranges.push((sy0, sy1));
            }
        }
        if fits {
            break (candidate, strips);
        }
        let cur_h = candidate.first().map_or(1, |b| b.out_h());
        if cur_h <= 1 {
            return Err(GpuFailure::PerPhoto(
                ExportFallbackKind::ExceedsLimits,
                "Single-row band exceeds GPU storage or dispatch limits".into(),
            ));
        }
        cap = Some((cur_h / 2).max(1));
    };

    let effects = match target {
        ExportTarget::Sdr { .. } => recipe.effects.grain != 0. || recipe.effects.vignette != 0.,
        ExportTarget::Hdr { .. } => false,
    };
    let vignette = PostCropVignette::new(&recipe.effects, [full_w, full_h]);
    let grain = GrainField::new(&recipe.effects, full_w.max(full_h) as f32 / scale);
    let has_masks = match target {
        ExportTarget::Sdr { .. } => base.masks.iter().any(|m| m.is_active()),
        ExportTarget::Hdr { .. } => false,
    };

    Ok(PassPlan {
        recipe,
        base_no_tex,
        geometry,
        scale,
        out_w,
        out_h,
        resize,
        target,
        sharpen,
        sharpener,
        radius,
        finish_weights,
        x_offset,
        x_stride,
        y_offset,
        y_stride,
        effects,
        vignette,
        grain,
        has_masks,
        bands,
        strip_ranges,
    })
}

/// Prepared camera pixels on the GPU (full-photo mode) or on the CPU ready for per-band
/// horizontal strip upload (strip mode).
enum PreparedPhoto {
    Full(wgpu::Buffer),
    Strips(Arc<CameraImage>),
}

/// Per-photo preprocessed state shared across all bands (and across both HDR and SDR passes
/// in [`GpuExportContext::render_pair_banded`]).
struct PreparedGpuSource {
    photo: PreparedPhoto,
    coverage_buf: wgpu::Buffer,
    measured: Arc<CameraImage>,
    unretouched_measured: Arc<CameraImage>,
    mask_guide: Option<Arc<CameraImage>>,
}

#[allow(clippy::too_many_arguments)]
fn prepare_source_on_gpu(
    ctx: &mut GpuExportContext,
    source_img: &Arc<CameraImage>,
    denoise: Option<&Denoise>,
    retouch_ops: Retouching<'_>,
    texture_amount: f32,
    scale: f32,
    file_correction: bool,
    needs_auto_mask_guide: bool,
    use_full_photo: bool,
    effective_max_binding: u64,
    cancel: Cancel<'_>,
) -> std::result::Result<PreparedGpuSource, GpuFailure> {
    if cancel.is_cancelled() {
        return Err(GpuFailure::Cancelled);
    }

    let coverage_buf = match &source_img.metadata.coverage {
        Some(mask) => {
            let packed: Vec<u32> = mask
                .values
                .chunks(4)
                .map(|c| {
                    let mut b = [0u8; 4];
                    b[..c.len()].copy_from_slice(c);
                    u32::from_le_bytes(b)
                })
                .collect();
            if (packed.len() as u64 * 4) > effective_max_binding {
                return Err(GpuFailure::PerPhoto(
                    ExportFallbackKind::ExceedsLimits,
                    "Coverage mask exceeds GPU storage buffer limit".into(),
                ));
            }
            GpuExportContext::upload_storage(
                &ctx.device,
                &ctx.queue,
                &mut ctx.buffers.coverage,
                "Export transparency mask",
                bytemuck::cast_slice(&packed),
            )
        }
        None => GpuExportContext::ensure_buffer(
            &ctx.device,
            &mut ctx.buffers.coverage,
            "Export empty coverage",
            16,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        ),
    };

    let chroma = denoise.and_then(|d| d.chroma);
    let luminance = denoise.and_then(|d| d.luminance);

    if !use_full_photo {
        // Source-strip upload mode: camera buffer exceeds storage limits, so run optional
        // noise reduction, retouching, measurement reduction, and Texture on the CPU once,
        // then stream only each band's required horizontal source rows to the GPU.
        let denoised: Arc<CameraImage> = if chroma.is_some() || luminance.is_some() {
            let cpu_denoise = Denoise {
                ai: None,
                chroma,
                luminance,
            };
            cpu_denoise
                .apply_with(source_img, cancel, |_, _, _| None)
                .map_err(|e| {
                    if cancel.is_cancelled() {
                        GpuFailure::Cancelled
                    } else {
                        GpuFailure::Hard(e)
                    }
                })?
        } else {
            source_img.clone()
        };
        if cancel.is_cancelled() {
            return Err(GpuFailure::Cancelled);
        }

        let unretouched_measured = Arc::new(pipeline::measurement_source(
            denoised.as_ref().into(),
            local_tone::MAP_EDGE,
            file_correction,
        ));
        if cancel.is_cancelled() {
            return Err(GpuFailure::Cancelled);
        }

        let (retouched, measured) = if retouch_ops.is_empty() {
            (denoised, unretouched_measured.clone())
        } else {
            let img = Arc::new(retouch::apply(&denoised, retouch_ops).map_err(|e| {
                if cancel.is_cancelled() {
                    GpuFailure::Cancelled
                } else {
                    GpuFailure::Hard(e)
                }
            })?);
            if cancel.is_cancelled() {
                return Err(GpuFailure::Cancelled);
            }
            let meas = Arc::new(pipeline::measurement_source(
                img.as_ref().into(),
                local_tone::MAP_EDGE,
                file_correction,
            ));
            (img, meas)
        };
        if cancel.is_cancelled() {
            return Err(GpuFailure::Cancelled);
        }

        let strip_img = if texture_amount != 0. {
            let detail = texture::TextureDetail::of(&retouched, scale, cancel).map_err(|e| {
                if cancel.is_cancelled() {
                    GpuFailure::Cancelled
                } else {
                    GpuFailure::Hard(e)
                }
            })?;
            Arc::new(detail.apply(&retouched, texture_amount))
        } else {
            retouched
        };
        if cancel.is_cancelled() {
            return Err(GpuFailure::Cancelled);
        }

        return Ok(PreparedGpuSource {
            photo: PreparedPhoto::Strips(strip_img.clone()),
            coverage_buf,
            measured,
            unretouched_measured,
            mask_guide: Some(strip_img),
        });
    }

    // Full-photo GPU mode: upload camera image once and run Color NR, Luminance NR,
    // measurement box reduction, and Texture directly on the device.
    let source_bytes = source_img.pixels.len() as u64 * 12;
    let photo_buf = GpuExportContext::ensure_buffer(
        &ctx.device,
        &mut ctx.buffers.photo,
        "Export photo",
        source_bytes,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
    );
    if !source_img.pixels.is_empty() {
        ctx.queue
            .write_buffer(&photo_buf, 0, bytemuck::cast_slice(&source_img.pixels));
    }

    let photo_scale = source_img.photo_scale();
    if let Some(d) = chroma {
        ctx.pyramid.apply_chroma_denoise(
            ctx,
            &photo_buf,
            source_img.width,
            source_img.height,
            source_img.metadata.wb,
            photo_scale,
            d,
            cancel,
        )?;
    }
    if let Some(d) = luminance {
        ctx.pyramid.apply_luminance_denoise(
            ctx,
            &photo_buf,
            source_img.width,
            source_img.height,
            source_img.metadata.wb,
            photo_scale,
            d,
            cancel,
        )?;
    }

    let (meas_w, meas_h) =
        pipeline::measurement_size(source_img.width, source_img.height, local_tone::MAP_EDGE);
    let meas_crop = local_tone::inner_crop_rect(source_img)
        .map(|[x0, y0, x1, y1]| [x0 as u32, y0 as u32, x1 as u32, y1 as u32])
        .unwrap_or([0, 0, source_img.width, source_img.height]);

    let unretouched_pixels = ctx.pyramid.box_reduce(
        ctx,
        &photo_buf,
        source_img.width,
        source_img.height,
        meas_crop,
        meas_w,
        meas_h,
        cancel,
    )?;
    let unretouched_measured = Arc::new(pipeline::finish_measurement_source(
        reduced_camera_image(source_img, meas_w, meas_h, unretouched_pixels),
        file_correction,
    ));

    let mut gpu_modified = chroma.is_some() || luminance.is_some();
    let mut cpu_guide_override: Option<CameraImage> = None;
    let measured = if retouch_ops.is_empty() {
        unretouched_measured.clone()
    } else {
        let retouch_src_owned;
        let retouch_src: &CameraImage = if gpu_modified {
            let pixels = GpuPyramid::readback_photo(
                ctx,
                &photo_buf,
                source_img.width,
                source_img.height,
                cancel,
            )?;
            retouch_src_owned =
                reduced_camera_image(source_img, source_img.width, source_img.height, pixels);
            &retouch_src_owned
        } else {
            source_img
        };
        if cancel.is_cancelled() {
            return Err(GpuFailure::Cancelled);
        }
        let retouched_img = retouch::apply(retouch_src, retouch_ops).map_err(|e| {
            if cancel.is_cancelled() {
                GpuFailure::Cancelled
            } else {
                GpuFailure::Hard(e)
            }
        })?;
        if cancel.is_cancelled() {
            return Err(GpuFailure::Cancelled);
        }
        ctx.queue
            .write_buffer(&photo_buf, 0, bytemuck::cast_slice(&retouched_img.pixels));
        gpu_modified = true;
        let meas_pixels = ctx.pyramid.box_reduce(
            ctx,
            &photo_buf,
            source_img.width,
            source_img.height,
            meas_crop,
            meas_w,
            meas_h,
            cancel,
        )?;
        if needs_auto_mask_guide && texture_amount == 0. {
            cpu_guide_override = Some(retouched_img);
        }
        Arc::new(pipeline::finish_measurement_source(
            reduced_camera_image(source_img, meas_w, meas_h, meas_pixels),
            file_correction,
        ))
    };

    if texture_amount != 0. {
        let offset = texture::level_offset(source_img, scale);
        ctx.pyramid.apply_texture(
            ctx,
            &photo_buf,
            source_img.width,
            source_img.height,
            offset,
            texture_amount,
            cancel,
        )?;
        gpu_modified = true;
    }

    let mask_guide = if needs_auto_mask_guide && gpu_modified {
        if let Some(img) = cpu_guide_override {
            Some(Arc::new(img))
        } else {
            let pixels = GpuPyramid::readback_photo(
                ctx,
                &photo_buf,
                source_img.width,
                source_img.height,
                cancel,
            )?;
            Some(Arc::new(reduced_camera_image(
                source_img,
                source_img.width,
                source_img.height,
                pixels,
            )))
        }
    } else {
        None
    };

    Ok(PreparedGpuSource {
        photo: PreparedPhoto::Full(photo_buf),
        coverage_buf,
        measured,
        unretouched_measured,
        mask_guide,
    })
}

/// An in-flight band submission whose readback staging buffer is waiting to map while the
/// next band computes on the GPU.
struct PendingBand {
    slot: usize,
    staging_buf: wgpu::Buffer,
    submission: wgpu::SubmissionIndex,
    rx: mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
    oy0: u32,
    band_out_n: u64,
    readback_bytes: u64,
}

/// Unmaps and drops an in-flight staging buffer on cancellation or error so subsequent
/// renders can never race with an uncompleted map.
fn abort_pending_band(ctx: &mut GpuExportContext, pending: &mut Option<PendingBand>) {
    if let Some(p) = pending.take() {
        p.staging_buf.unmap();
        ctx.buffers.staging[p.slot] = None;
    }
}

/// Waits for `pending`'s GPU submission, copies its mapped pixels into `host_pixels`, and
/// unmaps its staging buffer for reuse.
fn drain_pending_band(
    ctx: &mut GpuExportContext,
    pending: PendingBand,
    out_w: u32,
    host_pixels: &mut ExportPixels,
    cancel: Cancel<'_>,
) -> std::result::Result<(), GpuFailure> {
    if let Err(err) = ctx.wait_band(pending.submission, cancel) {
        pending.staging_buf.unmap();
        ctx.buffers.staging[pending.slot] = None;
        return Err(err);
    }

    let map_result = match pending.rx.try_recv() {
        Ok(res) => Ok(res),
        Err(mpsc::TryRecvError::Empty) => {
            let _ = ctx.device.poll(wgpu::PollType::Poll);
            pending.rx.recv_timeout(Duration::from_secs(1))
        }
        Err(mpsc::TryRecvError::Disconnected) => Err(mpsc::RecvTimeoutError::Disconnected),
    };
    match map_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            pending.staging_buf.unmap();
            ctx.buffers.staging[pending.slot] = None;
            return Err(GpuFailure::Session(
                ExportFallbackKind::DeviceLostOrTimeout,
                format!("GPU readback map failed: {e}"),
            ));
        }
        Err(_) => {
            pending.staging_buf.unmap();
            ctx.buffers.staging[pending.slot] = None;
            return Err(GpuFailure::Session(
                ExportFallbackKind::DeviceLostOrTimeout,
                "GPU readback timed out".into(),
            ));
        }
    }

    if cancel.is_cancelled() {
        pending.staging_buf.unmap();
        return Err(GpuFailure::Cancelled);
    }

    {
        let slice = pending.staging_buf.slice(..pending.readback_bytes);
        let mapped = match slice.get_mapped_range() {
            Ok(view) => view,
            Err(e) => {
                pending.staging_buf.unmap();
                ctx.buffers.staging[pending.slot] = None;
                return Err(GpuFailure::Session(
                    ExportFallbackKind::DeviceLostOrTimeout,
                    format!("GPU mapped range failed: {e}"),
                ));
            }
        };
        copy_mapped_band(&mapped, pending.oy0, out_w, pending.band_out_n, host_pixels);
    }
    pending.staging_buf.unmap();
    Ok(())
}

fn copy_mapped_band(
    mapped: &[u8],
    oy0: u32,
    out_w: u32,
    band_out_n: u64,
    host_pixels: &mut ExportPixels,
) {
    let row_start = oy0 as usize * out_w as usize;
    let count = band_out_n as usize;
    match host_pixels {
        ExportPixels::F32(dst) => {
            let src = bytemuck::cast_slice::<u8, [f32; 3]>(&mapped[..count * 12]);
            dst[row_start..row_start + count].copy_from_slice(src);
        }
        ExportPixels::Rgb8(dst) => {
            let byte_count = count * 3;
            #[cfg(target_endian = "little")]
            {
                dst[row_start * 3..row_start * 3 + byte_count]
                    .copy_from_slice(&mapped[..byte_count]);
            }
            #[cfg(target_endian = "big")]
            {
                let words = bytemuck::cast_slice::<u8, u32>(mapped);
                for (i, out_b) in dst[row_start * 3..row_start * 3 + byte_count]
                    .iter_mut()
                    .enumerate()
                {
                    *out_b = ((words[i / 4] >> (8 * (i % 4))) & 0xff) as u8;
                }
            }
        }
        ExportPixels::Rgb16(dst) => {
            let sample_count = count * 3;
            #[cfg(target_endian = "little")]
            {
                let src = bytemuck::cast_slice::<u8, u16>(&mapped[..sample_count * 2]);
                dst[row_start * 3..row_start * 3 + sample_count].copy_from_slice(src);
            }
            #[cfg(target_endian = "big")]
            {
                let words = bytemuck::cast_slice::<u8, u32>(mapped);
                for (i, out_s) in dst[row_start * 3..row_start * 3 + sample_count]
                    .iter_mut()
                    .enumerate()
                {
                    *out_s = ((words[i / 2] >> (16 * (i % 2))) & 0xffff) as u16;
                }
            }
        }
    }
}

/// Runs one banded SDR or HDR export pass over `prepared` with double-buffered staging
/// readback overlap.
fn run_banded_pass(
    ctx: &mut GpuExportContext,
    source_img: &Arc<CameraImage>,
    prepared: &PreparedGpuSource,
    plan: &PassPlan<'_>,
    effective_max_binding: u64,
    max_wg: u32,
    cancel: Cancel<'_>,
) -> std::result::Result<ExportOutput, GpuFailure> {
    if cancel.is_cancelled() {
        return Err(GpuFailure::Cancelled);
    }

    let g = plan.geometry;
    let full_w = g.width;
    let full_h = g.height;
    let out_w = plan.out_w;
    let out_h = plan.out_h;
    let resize = plan.resize;
    let format = match plan.target {
        ExportTarget::Sdr { format, .. } => format,
        ExportTarget::Hdr { .. } => ExportFormat::F32,
    };

    // Build per-pixel develop parameters & Shadows/Highlights map from the measurement copies
    // (which intentionally exclude measured Texture).
    let mut param_source = Source::new(source_img);
    param_source.measured = Some(&prepared.measured);
    param_source.unretouched = Some(&prepared.unretouched_measured);
    let maybe_params = match plan.target {
        ExportTarget::Sdr { .. } => pixel_params::pixel_params(param_source, &plan.base_no_tex),
        ExportTarget::Hdr { .. } => pixel_params::pixel_params_hdr(param_source, &plan.base_no_tex),
    };
    let Some(mut params) = maybe_params else {
        return Err(GpuFailure::PerPhoto(
            ExportFallbackKind::UnsupportedRecipe,
            "Recipe is not supported by GPU develop".into(),
        ));
    };
    params.color = None;

    let tables_bytes = (params.tables.len() as u64 * 4).max(16);
    let grid_bytes = params
        .grid
        .as_ref()
        .map_or(16, |gr| (gr.values().len() as u64 * 4).max(16));
    if tables_bytes > effective_max_binding || grid_bytes > effective_max_binding {
        return Err(GpuFailure::PerPhoto(
            ExportFallbackKind::ExceedsLimits,
            "Develop tables or map exceed GPU storage buffer limit".into(),
        ));
    }

    // Prepare per-photo mask weigher once so brush rasters are built once, not once per band.
    let mask_guide_img: &CameraImage = prepared
        .mask_guide
        .as_deref()
        .unwrap_or(source_img.as_ref());
    let weigher = plan
        .has_masks
        .then(|| Weigher::new(mask_guide_img, &plan.base_no_tex.masks, Selection::Active));

    // Prebuild static sampling parameter template (per-band REGION, COUNT, PHOTO_Y0 updated in loop).
    let mut sampling_template = vec![0f32; sample_slot::HEADER];
    sampling_template[sample_slot::WIDTH.start] = source_img.width as f32;
    sampling_template[sample_slot::HEIGHT.start] = source_img.height as f32;
    sampling_template[sample_slot::OUT].copy_from_slice(&[full_w as f32, full_h as f32]);
    sampling_template[sample_slot::SPREAD.start] = 0.;
    sampling_template[sample_slot::CROP.start..sample_slot::HOMOGRAPHY.end]
        .copy_from_slice(&g.gpu_params());
    sampling_template[sample_slot::MANUAL].copy_from_slice(&g.gpu_manual());
    sampling_template[sample_slot::XTRANS.start] = f32::from(source_img.metadata.xtrans);
    if let Some(c) = &source_img.metadata.coverage {
        sampling_template[sample_slot::COVERAGE.start] = c.width as f32;
        sampling_template[sample_slot::COVERAGE.start + 1] = c.height as f32;
    }
    let mut lens_tables = Vec::new();
    let lens = pipeline::lens_gpu_params(
        source_img,
        &plan.base_no_tex,
        sample_slot::HEADER,
        &mut lens_tables,
    );
    sampling_template[sample_slot::LENS.start..sample_slot::GAIN_MAP.end].copy_from_slice(&lens);
    sampling_template.extend(lens_tables);

    // Upload static per-pass auxiliary buffers.
    let grid_buf = match &params.grid {
        Some(grid) => GpuExportContext::upload_storage(
            &ctx.device,
            &ctx.queue,
            &mut ctx.buffers.grid,
            "Export Shadows/Highlights map",
            bytemuck::cast_slice(&grid.values()),
        ),
        None => GpuExportContext::ensure_buffer(
            &ctx.device,
            &mut ctx.buffers.grid,
            "Export empty map",
            16,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        ),
    };
    let finish_weights_buf = GpuExportContext::upload_storage(
        &ctx.device,
        &ctx.queue,
        &mut ctx.buffers.finish_weights,
        "Export finish weights",
        bytemuck::cast_slice(&plan.finish_weights),
    );

    // Pre-size band working buffers once to the maximum across all bands so the shorter final
    // band never reallocates and double-buffered staging stays churn-free.
    let max_comp_n = plan
        .bands
        .iter()
        .map(|b| u64::from(full_w) * u64::from(b.comp_h()))
        .max()
        .unwrap_or(1);
    let max_out_h = plan.bands.iter().map(|b| b.out_h()).max().unwrap_or(1);
    let max_out_n = u64::from(out_w) * u64::from(max_out_h);
    let max_scratch_bytes = (max_comp_n * 4).max(if resize {
        u64::from(full_w) * u64::from(max_out_h) * 12
    } else {
        0
    });
    let (max_pack_words, max_readback_bytes) = match format {
        ExportFormat::F32 => (0u64, max_out_n * 12),
        ExportFormat::Rgb8 => {
            let words = (max_out_n * 3).div_ceil(4);
            (words, words * 4)
        }
        ExportFormat::Rgb16 => {
            let words = (max_out_n * 3).div_ceil(2);
            (words, words * 4)
        }
    };

    let samples_buf = GpuExportContext::ensure_buffer(
        &ctx.device,
        &mut ctx.buffers.samples,
        "Export band samples",
        max_comp_n * 12,
        wgpu::BufferUsages::STORAGE,
    );
    let positions_buf = GpuExportContext::ensure_buffer(
        &ctx.device,
        &mut ctx.buffers.positions,
        "Export band positions",
        max_comp_n * 8,
        wgpu::BufferUsages::STORAGE,
    );
    let pixels_buf = GpuExportContext::ensure_buffer(
        &ctx.device,
        &mut ctx.buffers.pixels,
        "Export band pixels",
        max_comp_n * 12,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let scratch_buf = GpuExportContext::ensure_buffer(
        &ctx.device,
        &mut ctx.buffers.scratch,
        "Export band scratch",
        max_scratch_bytes,
        wgpu::BufferUsages::STORAGE,
    );
    let output_buf = GpuExportContext::ensure_buffer(
        &ctx.device,
        &mut ctx.buffers.output,
        "Export band resized output",
        if resize { max_out_n * 12 } else { 16 },
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let packed_out_buf = GpuExportContext::ensure_buffer(
        &ctx.device,
        &mut ctx.buffers.packed_out,
        "Export band packed output",
        if max_pack_words > 0 {
            max_pack_words * 4
        } else {
            16
        },
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );

    // In strip mode, pre-size the photo strip buffer once to the widest strip across bands.
    let photo_buf = match &prepared.photo {
        PreparedPhoto::Full(buf) => buf.clone(),
        PreparedPhoto::Strips(_) => {
            let max_strip_rows = plan
                .strip_ranges
                .as_deref()
                .expect("strip ranges planned")
                .iter()
                .map(|&(y0, y1)| u64::from(y1 - y0))
                .max()
                .unwrap_or(1);
            let max_strip_bytes = u64::from(source_img.width) * max_strip_rows * 12;
            GpuExportContext::ensure_buffer(
                &ctx.device,
                &mut ctx.buffers.photo,
                "Export photo strip",
                max_strip_bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            )
        }
    };

    // Upload params/tables now if unmasked; otherwise band 0 installs mask headers via `set_masks`.
    let mut masks_initialized = false;
    let mut params_buf = if !plan.has_masks {
        Some(GpuExportContext::upload_storage(
            &ctx.device,
            &ctx.queue,
            &mut ctx.buffers.params,
            "Export develop params",
            bytemuck::cast_slice(&params.params),
        ))
    } else {
        None
    };
    let mut tables_buf = if !plan.has_masks {
        Some(GpuExportContext::upload_storage(
            &ctx.device,
            &ctx.queue,
            &mut ctx.buffers.tables,
            "Export develop tables",
            bytemuck::cast_slice(&params.tables),
        ))
    } else {
        None
    };

    // Allocate final host pixel buffer once for the whole photo.
    let total_out_pixels = out_w as usize * out_h as usize;
    let mut host_pixels = match format {
        ExportFormat::F32 => ExportPixels::F32(vec![[0f32; 3]; total_out_pixels]),
        ExportFormat::Rgb8 => ExportPixels::Rgb8(vec![0u8; total_out_pixels * 3]),
        ExportFormat::Rgb16 => ExportPixels::Rgb16(vec![0u16; total_out_pixels * 3]),
    };

    let (finish_output_space, develop_space, hdr_peak, hdr_encode) = match plan.target {
        ExportTarget::Sdr { space, .. } => {
            (Variant::output_space_code(space), space, 0.0f32, false)
        }
        ExportTarget::Hdr { peak } => (
            Variant::output_space_code(OutputSpace::ProPhoto),
            OutputSpace::Srgb,
            peak,
            plan.sharpen || resize,
        ),
    };

    let mut pending: Option<PendingBand> = None;

    for (band_idx, band) in plan.bands.iter().enumerate() {
        if cancel.is_cancelled() {
            abort_pending_band(ctx, &mut pending);
            return Err(GpuFailure::Cancelled);
        }

        let comp_h = band.comp_h();
        let comp_n = u64::from(full_w) * u64::from(comp_h);
        let band_out_h = band.out_h();
        let band_out_n = u64::from(out_w) * u64::from(band_out_h);
        let crop_y = band.sy_min - band.y_comp0;
        let crop_h = band.sy_h();

        // Evaluate per-band mask weights when active.
        if let Some(weigher) = weigher.as_ref() {
            let region = [0, band.y_comp0, full_w, comp_h];
            let weights = weigher.weights(mask_guide_img, &plan.base_no_tex, &g, region, None);
            if cancel.is_cancelled() {
                abort_pending_band(ctx, &mut pending);
                return Err(GpuFailure::Cancelled);
            }
            if !masks_initialized {
                if !params.set_masks(param_source, &plan.base_no_tex, Some(&weights)) {
                    abort_pending_band(ctx, &mut pending);
                    return Err(GpuFailure::PerPhoto(
                        ExportFallbackKind::UnsupportedRecipe,
                        "Mask configuration not supported on GPU".into(),
                    ));
                }
                if (params.tables.len() as u64 * 4) > effective_max_binding {
                    abort_pending_band(ctx, &mut pending);
                    return Err(GpuFailure::PerPhoto(
                        ExportFallbackKind::ExceedsLimits,
                        "Mask tables exceed GPU storage buffer limit".into(),
                    ));
                }
                params_buf = Some(GpuExportContext::upload_storage(
                    &ctx.device,
                    &ctx.queue,
                    &mut ctx.buffers.params,
                    "Export develop params",
                    bytemuck::cast_slice(&params.params),
                ));
                tables_buf = Some(GpuExportContext::upload_storage(
                    &ctx.device,
                    &ctx.queue,
                    &mut ctx.buffers.tables,
                    "Export develop tables",
                    bytemuck::cast_slice(&params.tables),
                ));
                masks_initialized = true;
            } else {
                params.update_mask_weights(&weights);
            }
        }

        let mask_weights_bytes = params.weights.len() as u64 * 4;
        if mask_weights_bytes > effective_max_binding {
            abort_pending_band(ctx, &mut pending);
            return Err(GpuFailure::PerPhoto(
                ExportFallbackKind::ExceedsLimits,
                "Band mask weights exceed GPU storage buffer limit".into(),
            ));
        }
        let mask_weights_buf = GpuExportContext::upload_storage(
            &ctx.device,
            &ctx.queue,
            &mut ctx.buffers.mask_weights,
            "Export band mask weights",
            bytemuck::cast_slice(&params.weights),
        );

        let (pack_words, readback_bytes) = match format {
            ExportFormat::F32 => (0u64, band_out_n * 12),
            ExportFormat::Rgb8 => {
                let words = (band_out_n * 3).div_ceil(4);
                (words, words * 4)
            }
            ExportFormat::Rgb16 => {
                let words = (band_out_n * 3).div_ceil(2);
                (words, words * 4)
            }
        };

        // Pick the alternate staging slot (guaranteed unmapped because `pending`, if any,
        // uses `(band_idx - 1) % 2`).
        let slot = band_idx % 2;
        let staging_buf = GpuExportContext::ensure_buffer(
            &ctx.device,
            &mut ctx.buffers.staging[slot],
            "Export band staging",
            max_readback_bytes,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );

        // In strip mode, upload this band's conservative source strip before submitting.
        if let PreparedPhoto::Strips(strip_img) = &prepared.photo {
            let (src_y0, src_y1) =
                plan.strip_ranges.as_deref().expect("strip ranges planned")[band_idx];
            let start = src_y0 as usize * strip_img.width as usize;
            let end = src_y1 as usize * strip_img.width as usize;
            if start < end {
                ctx.queue.write_buffer(
                    &photo_buf,
                    0,
                    bytemuck::cast_slice(&strip_img.pixels[start..end]),
                );
            }
            sampling_template[sample_slot::PHOTO_Y0.start] = src_y0 as f32;
        } else {
            sampling_template[sample_slot::PHOTO_Y0.start] = 0.;
        }

        // Update sampling params for this band.
        let (gx_sample, gy_sample) = groups_1d(max_wg, comp_n);
        sampling_template[sample_slot::REGION].copy_from_slice(&[
            0.,
            band.y_comp0 as f32,
            full_w as f32,
            comp_h as f32,
        ]);
        sampling_template[sample_slot::COUNT.start] = gx_sample as f32;
        let sampling_buf = GpuExportContext::upload_storage(
            &ctx.device,
            &ctx.queue,
            &mut ctx.buffers.sampling,
            "Export sampling params",
            bytemuck::cast_slice(&sampling_template),
        );

        let (gx_dev, gy_dev) = groups_1d(max_wg, comp_n);
        let dev_size_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Export develop size"),
                contents: bytemuck::cast_slice(&[comp_n as u32, gx_dev, 0u32, 0u32]),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let (gx_pack, gy_pack) = if pack_words > 0 {
            groups_1d(max_wg, pack_words)
        } else {
            (1, 1)
        };
        let finish_uniform = ExportFinishParams {
            width: full_w,
            height: comp_h,
            crop_y,
            crop_h,
            out_width: out_w,
            out_height: band_out_h,
            out_y0: band.oy0,
            origin_y: band.y_comp0,
            full_w,
            full_h,
            output_space: finish_output_space,
            resize: u32::from(resize),
            radius: plan.radius,
            amount: plan.recipe.sharpening * plan.sharpener.gain,
            threshold: plan.sharpener.threshold,
            halo: plan.sharpener.halo,
            dark: plan.sharpener.dark,
            x_offset: plan.x_offset,
            x_stride: plan.x_stride,
            y_offset: plan.y_offset,
            y_stride: plan.y_stride,
            pack_src_offset: if resize { 0 } else { crop_y * full_w * 3 },
            pack_groups_x: gx_pack,
            scale: plan.scale,
            grain: plan.grain.amount,
            grain_cell: plan.grain.cell,
            grain_coarse: plan.grain.coarse,
            grain_fine: plan.grain.fine,
            grain_seed: plan.grain.seed,
            vignette: plan.vignette.map_or(0., |v| v.amount),
            vignette_style: plan.vignette.map_or(0, |v| v.style.code() as u32),
            vignette_highlights: plan.vignette.map_or(0., |v| v.highlights),
            vignette_scale_x: plan.vignette.map_or(1., |v| v.scale[0]),
            vignette_scale_y: plan.vignette.map_or(1., |v| v.scale[1]),
            vignette_power: plan.vignette.map_or(2., |v| v.power),
            vignette_midpoint: plan.vignette.map_or(0., |v| v.midpoint),
            vignette_feather: plan.vignette.map_or(1., |v| v.feather),
            hdr_peak,
            hdr_decode: u32::from(hdr_encode),
            pad2: 0,
        };
        let finish_uniform_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Export finish params"),
                contents: bytemuck::bytes_of(&finish_uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let develop_pipeline = ctx.develop_pipeline(Variant::of_in(&params, develop_space));

        let sample_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Export sample bindings"),
            layout: &ctx.sample_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: photo_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: prepared.coverage_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: sampling_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: samples_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 15,
                    resource: positions_buf.as_entire_binding(),
                },
            ],
        });

        let p_buf = params_buf.as_ref().expect("params uploaded");
        let t_buf = tables_buf.as_ref().expect("tables uploaded");
        let dev_buffers = [
            &samples_buf,
            &positions_buf,
            &pixels_buf,
            p_buf,
            t_buf,
            &dev_size_buf,
            &mask_weights_buf,
            &grid_buf,
            &ctx.no_kept,
        ];
        let dev_entries: Vec<_> = dev_buffers
            .into_iter()
            .enumerate()
            .map(|(binding, buf)| wgpu::BindGroupEntry {
                binding: binding as u32,
                resource: buf.as_entire_binding(),
            })
            .collect();
        let develop_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Export develop bindings"),
            layout: &ctx.develop_layout,
            entries: &dev_entries,
        });

        let finish_buffers = [
            &pixels_buf,
            &scratch_buf,
            &output_buf,
            &finish_weights_buf,
            &finish_uniform_buf,
            &packed_out_buf,
        ];
        let finish_entries: Vec<_> = finish_buffers
            .into_iter()
            .enumerate()
            .map(|(binding, buf)| wgpu::BindGroupEntry {
                binding: binding as u32,
                resource: buf.as_entire_binding(),
            })
            .collect();
        let finish_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Export finish bindings"),
            layout: &ctx.finish_layout,
            entries: &finish_entries,
        });

        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        // 1. Sample region
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Export sample_region"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&ctx.sample_pipeline);
            pass.set_bind_group(0, &sample_group, &[]);
            pass.dispatch_workgroups(gx_sample, gy_sample, 1);
        }
        // 2. Per-pixel develop (in target OutputSpace for SDR, or linear ProPhoto for HDR)
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Export develop"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&develop_pipeline);
            pass.set_bind_group(0, &develop_group, &[]);
            pass.dispatch_workgroups(gx_dev, gy_dev, 1);
        }
        let dispatch_finish =
            |encoder: &mut wgpu::CommandEncoder, pipeline_idx: usize, gx: u32, gy: u32| {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Export finish pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&ctx.finish_pipelines[pipeline_idx]);
                pass.set_bind_group(0, &finish_group, &[]);
                pass.dispatch_workgroups(gx, gy, 1);
            };

        match plan.target {
            ExportTarget::Sdr { format, .. } => {
                // 3. Sharpening (full resolution over expanded band rows)
                if plan.sharpen {
                    dispatch_finish(
                        &mut encoder,
                        FINISH_BLUR_H,
                        full_w.div_ceil(16),
                        comp_h.div_ceil(16),
                    );
                    dispatch_finish(
                        &mut encoder,
                        FINISH_SHARPEN,
                        full_w.div_ceil(16),
                        comp_h.div_ceil(16),
                    );
                }
                // 4. Post-Crop Vignette & Film Grain (valid full-res rows)
                if plan.effects {
                    dispatch_finish(
                        &mut encoder,
                        FINISH_SPATIAL,
                        full_w.div_ceil(16),
                        crop_h.div_ceil(16),
                    );
                }
                // 5. Separable Lanczos3 resize (full-res -> target band)
                if resize {
                    dispatch_finish(
                        &mut encoder,
                        FINISH_RESIZE_V,
                        full_w.div_ceil(16),
                        band_out_h.div_ceil(16),
                    );
                    dispatch_finish(
                        &mut encoder,
                        FINISH_RESIZE_H,
                        out_w.div_ceil(16),
                        band_out_h.div_ceil(16),
                    );
                }
                // 6. Pack and copy to staging buffer
                match format {
                    ExportFormat::F32 => {
                        let (src, offset) = if resize {
                            (&output_buf, 0u64)
                        } else {
                            (&pixels_buf, u64::from(crop_y) * u64::from(full_w) * 12)
                        };
                        encoder.copy_buffer_to_buffer(src, offset, &staging_buf, 0, readback_bytes);
                    }
                    ExportFormat::Rgb8 => {
                        dispatch_finish(&mut encoder, FINISH_PACK_RGB8, gx_pack, gy_pack);
                        encoder.copy_buffer_to_buffer(
                            &packed_out_buf,
                            0,
                            &staging_buf,
                            0,
                            readback_bytes,
                        );
                    }
                    ExportFormat::Rgb16 => {
                        dispatch_finish(&mut encoder, FINISH_PACK_RGB16, gx_pack, gy_pack);
                        encoder.copy_buffer_to_buffer(
                            &packed_out_buf,
                            0,
                            &staging_buf,
                            0,
                            readback_bytes,
                        );
                    }
                }
            }
            ExportTarget::Hdr { .. } => {
                // 3. Extended sRGB encoding before unbounded sharpening and/or Lanczos3 resize.
                if hdr_encode {
                    dispatch_finish(
                        &mut encoder,
                        FINISH_HDR_ENCODE,
                        full_w.div_ceil(16),
                        comp_h.div_ceil(16),
                    );
                }
                // 4. Unbounded sharpening on ProPhoto luminance (`output_space == 2`).
                if plan.sharpen {
                    dispatch_finish(
                        &mut encoder,
                        FINISH_BLUR_H,
                        full_w.div_ceil(16),
                        comp_h.div_ceil(16),
                    );
                    dispatch_finish(
                        &mut encoder,
                        FINISH_HDR_SHARPEN,
                        full_w.div_ceil(16),
                        comp_h.div_ceil(16),
                    );
                }
                // 5. Unbounded separable Lanczos3 resize (clamped at 0, unclipped above white).
                if resize {
                    dispatch_finish(
                        &mut encoder,
                        FINISH_RESIZE_V,
                        full_w.div_ceil(16),
                        band_out_h.div_ceil(16),
                    );
                    dispatch_finish(
                        &mut encoder,
                        FINISH_HDR_RESIZE_H,
                        out_w.div_ceil(16),
                        band_out_h.div_ceil(16),
                    );
                }
                // 6. Decode sRGB curve back to linear ProPhoto (when encoded) and apply `roll_off`.
                dispatch_finish(
                    &mut encoder,
                    FINISH_HDR_FINISH,
                    out_w.div_ceil(16),
                    band_out_h.div_ceil(16),
                );
                let (src, offset) = if resize {
                    (&output_buf, 0u64)
                } else {
                    (&pixels_buf, u64::from(crop_y) * u64::from(full_w) * 12)
                };
                encoder.copy_buffer_to_buffer(src, offset, &staging_buf, 0, readback_bytes);
            }
        }

        if cancel.is_cancelled() {
            abort_pending_band(ctx, &mut pending);
            return Err(GpuFailure::Cancelled);
        }
        let submission = ctx.queue.submit([encoder.finish()]);

        let (tx, rx) = mpsc::sync_channel(1);
        staging_buf
            .slice(..readback_bytes)
            .map_async(wgpu::MapMode::Read, move |res| {
                let _ = tx.send(res);
            });

        let current = PendingBand {
            slot,
            staging_buf,
            submission,
            rx,
            oy0: band.oy0,
            band_out_n,
            readback_bytes,
        };
        if let Some(prev) = pending.replace(current)
            && let Err(err) = drain_pending_band(ctx, prev, out_w, &mut host_pixels, cancel)
        {
            abort_pending_band(ctx, &mut pending);
            return Err(err);
        }
    }

    if let Some(last) = pending.take() {
        drain_pending_band(ctx, last, out_w, &mut host_pixels, cancel)?;
    }

    match plan.target {
        ExportTarget::Sdr { space, .. } => Ok(ExportOutput::Sdr(ExportImage::from_pixels(
            out_w,
            out_h,
            host_pixels,
            space,
        ))),
        ExportTarget::Hdr { peak } => {
            let ExportPixels::F32(pixels) = host_pixels else {
                unreachable!("HDR output uses F32 pixels");
            };
            Ok(ExportOutput::Hdr(crate::rendered::HdrImage {
                width: out_w,
                height: out_h,
                pixels,
                peak,
            }))
        }
    }
}

fn create_export_finish_pipelines(
    device: &wgpu::Device,
) -> (wgpu::BindGroupLayout, [wgpu::ComputePipeline; 11]) {
    let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Export finish bindings"),
        entries: &[
            storage(0, false),
            storage(1, false),
            storage(2, false),
            storage(3, true),
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            storage(5, false),
        ],
    });
    let source = PostCropVignette::wgsl_tone() + include_str!("export_finish.wgsl");
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("export_finish.wgsl"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Export finish layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipelines = [
        "blur_horizontal",
        "sharpen",
        "spatial_effects",
        "resize_vertical",
        "resize_horizontal",
        "pack_rgb8",
        "pack_rgb16",
        "hdr_srgb_encode",
        "hdr_sharpen",
        "hdr_resize_horizontal",
        "hdr_finish_master",
    ]
    .map(|entry| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
    });
    (layout, pipelines)
}

#[cfg(test)]
trait MockGpuDriver {
    fn render_banded(
        &mut self,
        job: &PreparedExport<'_>,
        band_cap: Option<u32>,
        cancel: Cancel<'_>,
    ) -> std::result::Result<ExportImage, GpuFailure>;
    fn render_hdr_banded(
        &mut self,
        job: &PreparedHdrExport<'_>,
        band_cap: Option<u32>,
        _cancel: Cancel<'_>,
    ) -> std::result::Result<crate::rendered::HdrImage, GpuFailure> {
        let _ = band_cap;
        let n = job.out_w as usize * job.out_h as usize;
        Ok(crate::rendered::HdrImage {
            width: job.out_w,
            height: job.out_h,
            pixels: vec![[0.5, 0.5, 0.5]; n],
            peak: job.peak,
        })
    }
    fn render_pair_banded(
        &mut self,
        hdr: &PreparedHdrExport<'_>,
        sdr: &PreparedExport<'_>,
        band_cap: Option<u32>,
        cancel: Cancel<'_>,
    ) -> std::result::Result<(crate::rendered::HdrImage, ExportImage), GpuFailure> {
        let master = self.render_hdr_banded(hdr, band_cap, cancel)?;
        let base = self.render_banded(sdr, band_cap, cancel)?;
        Ok((master, base))
    }
    fn release_buffers(&mut self);
}

#[cfg(test)]
impl ExportBackend {
    /// Test helper: creates an [`ExportBackend`] backed by a deterministic mock GPU driver
    /// for testing Tier-1/Tier-2 fallbacks, OOM halving retry, mid-band cancel, and panic safety
    /// without requiring GPU hardware.
    fn with_mock(name: &str, driver: Box<dyn MockGpuDriver + Send>) -> Self {
        let mut meta = StatusState::new(ExportBackendPolicy::Auto, None);
        meta.mark_active(name.to_string());
        Self {
            exec: Mutex::new(ExecState {
                engine: Engine::Mock(driver),
                band_height_override: None,
                max_binding_override: None,
            }),
            meta: Mutex::new(meta),
            exec_asked: Default::default(),
        }
    }

    /// Test helper: overrides maximum output band height to force multi-band execution
    /// (e.g. `Some(64)` for band-seam exactness tests or `Some(u32::MAX)` for single-band).
    pub(crate) fn set_band_height_override(&self, height: Option<u32>) {
        self.lock_exec().band_height_override = height;
    }

    /// Test helper: simulates a smaller `max_storage_buffer_binding_size` limit so
    /// source-strip upload banding and limit fallback can be verified on any hardware GPU.
    pub(crate) fn set_max_binding_override(&self, limit: Option<u64>) {
        self.lock_exec().max_binding_override = limit;
    }
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
