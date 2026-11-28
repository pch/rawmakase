//! Optional compute backends for previews and SDR exports: the per-pixel develop
//! stage, sharpening, Lanczos resizing, display finishing, and banded export finishing.
//!
//! For previews on the desktop, [`Processor`] shares the UI's wgpu device so a preview
//! is finished straight into a texture the viewport draws (`present.rs`). Headless
//! callers (tests, benchmarks) create their own device. Buffers are bounded and reused
//! for equal dimensions; callers retain CPU pixels for fallback.
//!
//! For exports, [`ExportBackend`] (`export.rs`) owns a dedicated headless compute device
//! decoupled from eframe's UI device and [`SURFACE`], executing horizontal bands with
//! exact halo overlap, `OutputSpace` encoding, and deterministic two-tier fallback to the
//! CPU reference ([`crate::develop::render_export`]). The CPU stages remain the
//! authoritative reference (`docs/architecture.md`).
//!
//! # Backends and devices
//!
//! Portable WGSL compute through wgpu: Metal on macOS, Vulkan on supported Linux
//! drivers, including NVIDIA and AMD, and DirectX 12 on Windows. Windows leaves
//! Vulkan out while a DirectX 12 device can be created, because an Intel Vulkan
//! driver crashed the app at launch where DirectX 12 worked (#623). If that device
//! fails, it tries Vulkan and then OpenGL before opening the window (#768);
//! `WGPU_BACKEND` overrides all of this. The UI and standalone processors share that
//! choice ([`backends`]) and the DirectX 12 shader compiler choice
//! ([`backend_options`]): on Windows, wgpu's default searches `PATH` for
//! `dxcompiler.dll`, so we load only the copy bundled beside our executable by
//! absolute path and fall back to Windows' built-in FXC when the DLL is absent or
//! fails to start a DirectX 12 device (#1254). The desktop creates the UI's device
//! with the storage limits
//! previews need ([`required_limits`]) and a high-performance adapter. On Linux the
//! window's adapter is the first GPU the system lists instead: the display's GPU, or
//! the discrete one under `prime-run` or `DRI_PRIME=1`, because a render-offload GPU
//! can claim it presents to the window and then fail to configure it. OpenGL and
//! software adapters are not used for compute. Every GPU call runs inside its own
//! validation, internal and out-of-memory error scopes, so a failure disables the
//! preview GPU without reaching the UI. Only ordinary 32-bit storage buffers,
//! Rgba8Unorm storage textures and compute workgroups are used, without vendor
//! extensions. Hardware validated so far: Apple M1 Pro.
//!
//! # What runs here
//!
//! The per-pixel color and tone stage (`develop.wgsl`, fed by
//! `pipeline/pixel_params.rs`) runs on the stage cache's samples for every recipe
//! resolved with a camera profile; unresolved recipes use the CPU stage. Each pass is
//! compiled for the features its parameters use (`develop::Variant`); the
//! Shadows/Highlights map's grids stay on the device per map base, and each sample
//! set's camera color per the recipe fields it reads (`develop::Kept`), written once
//! a second render with the same settings asks for it and read after its pass is
//! submitted.
//!
//! An edit with HDR on has its master's preview rendered too, by the develop pass's
//! HDR variant and `present_hdr.wgsl`, into a half-float texture of the view's own
//! (`Processor::present_hdr`; docs/hdr.md#gpu-preview). It reads back nothing: the
//! histogram, overlays and reduced copies are the SDR rendition's.
//!
//! When the GPU renders a preview, the developed pixels stay on the device and
//! `present.wgsl` finishes them into a texture that egui draws directly (registered
//! as a native texture by the render worker): sharpening, vignettes and grain, the
//! clipping overlay, the monitor profile (a 52³ lattice sampled from
//! `color_management` once per profile, interpolated trilinearly) and 8-bit encoding,
//! in the CPU reference's order. The histogram is counted in the same pass and the
//! minimap's copy is reduced on the GPU; only the histogram (3 KB) and, for catalog
//! photos, a 640-pixel Library thumbnail are read back. Nothing is converted or
//! uploaded on the UI thread. Recipes the port does not cover, and machines without a
//! usable adapter, render on the CPU, and the worker then prepares the display bytes,
//! histogram and reduced copies before the UI uploads the texture. Full-resolution
//! regions keep the sharpening halo before cropping. Lanczos3 resizing of full
//! renders stays on the GPU for readback callers. The status line says `GPU` for
//! presented frames, `GPU finish` when only finishing used the GPU, and `CPU`
//! otherwise.
//!
//! For the same recipes, the stages before the per-pixel stage also run on the
//! device when the photo fits its buffer limits (`resident.rs`, `pyramid.rs`): the
//! photo or pyramid level is kept there, and regions are sampled there through
//! geometry and lens correction. Color and luminance noise reduction run on the full
//! camera image before retouching and before the preview pyramid is built
//! (`develop::recipe::Denoise` captures their settings and execution together; the
//! retouch cache keys that value and the source image identity), using the shared GPU
//! Laplacian pyramid runner (`pyramid.rs`) when the photo fits device limits and
//! falling back to the CPU reference otherwise. Measured Texture computes its
//! [`crate::develop::texture::TextureDetail`] through the same shared GPU pyramid
//! runner (reusing the resident GPU photo buffer when uploaded), cached in the stage
//! cache across slider drags. The Shadows/Highlights map (with Clarity's gain) is
//! built on the CPU from the photo's measurement copy, and the scene tone stage reads
//! it in `develop.wgsl`. A mask's Clarity or Texture renders the whole preview on the
//! CPU (while still sharing GPU-computed Texture detail when available). A failure in
//! these stages turns off only this path for the session.
//!
//! # Textures, buffers and failure
//!
//! Presented textures are kept per view (the whole photo, or a 100% region) and
//! size, two of each, so a slider moving alternates between the reduced preview's and
//! the full render's textures without allocating. Each texture carries a generation
//! that changes whenever it is written; the worker reuses a finished frame when
//! switching views only while its texture still holds it.
//!
//! Buffers are reused by dimensions, limited by the adapter and a 1 GiB aggregate
//! buffer budget. No compatible adapter, oversized buffers, allocation or validation
//! errors and readback failures fall back to the CPU. After a GPU failure the preview
//! renderer releases the backend and does not retry it for every edit; restarting
//! creates a new backend. GPU work already submitted cannot be interrupted: obsolete
//! results are discarded before they are published.
use crate::model::recipe::Recipe;
use crate::rendered::Rendered;
use anyhow::{Context, Result, ensure};
use rawmakase_cancel::Cancel;
use std::{
    sync::{PoisonError, RwLock, RwLockWriteGuard, mpsc},
    time::Duration,
};
use wgpu::util::DeviceExt;
mod develop;
pub mod export;
mod present;
mod pyramid;
mod resident;
pub(crate) mod sampling;
mod uniforms;
mod weights;
pub(crate) use develop::Input;
pub use export::{
    EXPORT_GPU_ENV, ExportBackend, ExportBackendPolicy, ExportBackendStatus, ExportFallbackKind,
};
pub(crate) use present::Finish;
pub use present::{Display, Frame, HdrFrame, MonitorLut, Slot};

/// Held by previews while they submit work, and exclusively while the window's
/// surface is reconfigured for a new size.
static SURFACE: RwLock<()> = RwLock::new(());

/// Keeps previews from submitting work to the shared device until dropped.
///
/// Before configuring a surface, wgpu waits for the GPU to go idle and fails
/// ("Failed to wait for GPU to come idle") when another thread submits during
/// that wait. The surface then keeps its old size while egui draws the frame
/// for the new one, which wgpu rejects. Resizing a tiled window on macOS while
/// Develop rendered did exactly that, and crashed.
pub fn reconfiguring_surface() -> RwLockWriteGuard<'static, ()> {
    SURFACE.write().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `submit` unless the window's surface is being reconfigured, in which
/// case it waits for that to finish first.
fn submitting<T>(submit: impl FnOnce() -> T) -> T {
    let _surface = SURFACE.read().unwrap_or_else(PoisonError::into_inner);
    submit()
}

fn submit(queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
    let commands = encoder.finish();
    submitting(|| queue.submit([commands]))
}

/// Waits up to ten seconds for `submission` to finish on the GPU.
///
/// wgpu holds the device's resource lock through a whole `PollType::Wait`,
/// and the UI thread needs that lock exclusively to acquire and present each
/// window frame. A render waiting in one long poll froze the interface (a
/// slider stopped following the pointer) until the GPU finished; waiting in
/// short slices lets the UI's frames through in between.
fn wait(device: &wgpu::Device, submission: wgpu::SubmissionIndex) -> Result<(), wgpu::PollError> {
    const SLICE: Duration = Duration::from_millis(2);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission.clone()),
            timeout: Some(SLICE),
        }) {
            Err(wgpu::PollError::Timeout) if std::time::Instant::now() < deadline => {}
            result => return result.map(drop),
        }
    }
}

fn wait_cancellable(
    device: &wgpu::Device,
    submission: wgpu::SubmissionIndex,
    cancel: Cancel<'_>,
) -> Result<()> {
    const SLICE: Duration = Duration::from_millis(2);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        cancel.check("Render superseded")?;
        match device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission.clone()),
            timeout: Some(SLICE),
        }) {
            Ok(_) => return Ok(()),
            Err(wgpu::PollError::Timeout) if std::time::Instant::now() < deadline => {}
            Err(err) => return Err(err.into()),
        }
    }
}

struct ProcessorHost<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
}

impl pyramid::PyramidHost for ProcessorHost<'_> {
    type Error = anyhow::Error;

    fn device(&self) -> &wgpu::Device {
        self.device
    }

    fn submit(&self, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        submit(self.queue, encoder)
    }

    fn wait(&self, submission: wgpu::SubmissionIndex, cancel: Cancel<'_>) -> Result<()> {
        wait_cancellable(self.device, submission, cancel)
    }

    fn cancelled(&self) -> anyhow::Error {
        anyhow::anyhow!("Render superseded")
    }

    fn readback_error(&self, message: String) -> anyhow::Error {
        anyhow::anyhow!("{message}")
    }

    fn exceeds_limits(&self, message: String) -> anyhow::Error {
        anyhow::anyhow!("{message}")
    }
}

pub struct Processor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::ComputePipeline; 4],
    buffers: Option<Buffers>,
    /// Per-pixel develop stage, created on first use.
    developer: Option<develop::Developer>,
    /// Display finishing, created on first use.
    presenter: Option<present::Presenter>,
    /// The photo and its local-tone and sampling stages on the device.
    resident: Option<resident::Resident>,
    /// Shared GPU Laplacian pyramid pipelines, created on first use.
    pyramid: Option<pyramid::GpuPyramid>,
    /// The develop pass's camera color kept for its samples (see `develop::Kept`), most
    /// recently used first.
    kept: Vec<develop::Kept>,
    name: String,
}
struct Buffers {
    dimensions: [u32; 4],
    source: wgpu::Buffer,
    scratch: wgpu::Buffer,
    output: wgpu::Buffer,
    staging: wgpu::Buffer,
}
struct WindowsChoice {
    backends: wgpu::Backends,
    compiler: wgpu::Dx12Compiler,
}
/// The wgpu backends the UI and previews may use, unless `WGPU_BACKEND` names
/// others. Windows uses DirectX 12 with OpenGL as the fallback and leaves Vulkan out
/// while DirectX 12 works: wgpu prefers Vulkan when both are available, and Intel's
/// Vulkan driver for HD Graphics 530 (igvk64.dll 31.0.101.2125) crashes the process
/// at most launches, while DirectX 12 and OpenGL start reliably there (#623).
pub fn backends() -> wgpu::Backends {
    wgpu::Backends::from_env().unwrap_or_else(|| windows_choice().backends)
}
/// Backend options the UI, previews and GPU export share.
///
/// On Windows, wgpu's default [`wgpu::Dx12Compiler::Auto`] loads `"dxcompiler.dll"`
/// by bare name, so Windows searches `PATH` and can load another application's
/// incompatible build (#1254). Unless `WGPU_DX12_COMPILER` overrides the compiler,
/// load `dxcompiler.dll` only by absolute path beside this executable, and fall back
/// to Windows' built-in FXC compiler when the DLL is absent or fails to start a
/// DirectX 12 device (for example on Shader Model 5.1 drivers or if the DLL cannot
/// load).
pub fn backend_options() -> wgpu::BackendOptions {
    if !cfg!(windows) || wgpu::Dx12Compiler::from_env().is_some() {
        return wgpu::BackendOptions::from_env_or_default();
    }
    let compiler = if wgpu::Backends::from_env()
        .is_some_and(|backends| !backends.contains(wgpu::Backends::DX12))
    {
        preferred_dx12_compiler()
    } else {
        windows_choice().compiler.clone()
    };
    backend_options_with(compiler).with_env()
}
fn windows_choice() -> &'static WindowsChoice {
    static CHOICE: std::sync::OnceLock<WindowsChoice> = std::sync::OnceLock::new();
    CHOICE.get_or_init(|| {
        let env_compiler = wgpu::Dx12Compiler::from_env();
        let allow_fxc_fallback = env_compiler.is_none();
        let preferred = env_compiler.unwrap_or_else(preferred_dx12_compiler);
        choose_windows(
            cfg!(windows),
            preferred,
            allow_fxc_fallback,
            |backends, compiler| device_starts(backends, backend_options_with(compiler).with_env()),
        )
    })
}
fn preferred_dx12_compiler() -> wgpu::Dx12Compiler {
    let exe = cfg!(windows)
        .then(|| std::env::current_exe().ok())
        .flatten();
    dx12_compiler(cfg!(windows), exe.as_deref(), |path| path.is_file())
}
fn backend_options_with(shader_compiler: wgpu::Dx12Compiler) -> wgpu::BackendOptions {
    wgpu::BackendOptions {
        dx12: wgpu::Dx12BackendOptions {
            shader_compiler,
            ..Default::default()
        },
        ..Default::default()
    }
}
/// wgpu commits to one adapter and gives up if its device fails, so on Windows each
/// backend is tried here first (#768). When DirectX 12 fails under the bundled DXC
/// (for example on a Shader Model 5.1 driver where wgpu hides the adapter under DXC,
/// or if the DLL is damaged), DirectX 12 is retried with FXC before moving on to
/// Vulkan (#623) or OpenGL. Vulkan comes before OpenGL because previews don't use
/// OpenGL for compute. With nothing working, the usual choice reports why.
fn choose_windows(
    windows: bool,
    preferred: wgpu::Dx12Compiler,
    allow_fxc_fallback: bool,
    mut starts: impl FnMut(wgpu::Backends, wgpu::Dx12Compiler) -> bool,
) -> WindowsChoice {
    use wgpu::{Backends, Dx12Compiler};
    if !windows {
        return WindowsChoice {
            backends: Backends::PRIMARY | Backends::GL,
            compiler: preferred,
        };
    }
    if starts(Backends::DX12, preferred.clone()) {
        return WindowsChoice {
            backends: Backends::DX12 | Backends::GL,
            compiler: preferred,
        };
    }
    if allow_fxc_fallback
        && !matches!(preferred, Dx12Compiler::Fxc)
        && starts(Backends::DX12, Dx12Compiler::Fxc)
    {
        return WindowsChoice {
            backends: Backends::DX12 | Backends::GL,
            compiler: Dx12Compiler::Fxc,
        };
    }
    let backends = if starts(Backends::VULKAN, preferred.clone()) {
        Backends::VULKAN | Backends::GL
    } else if starts(Backends::GL, preferred.clone()) {
        Backends::GL
    } else {
        Backends::DX12 | Backends::GL
    };
    WindowsChoice {
        backends,
        compiler: preferred,
    }
}
fn dx12_compiler(
    windows: bool,
    exe: Option<&std::path::Path>,
    exists: impl FnOnce(&std::path::Path) -> bool,
) -> wgpu::Dx12Compiler {
    if !windows {
        return wgpu::Dx12Compiler::default();
    }
    if let Some(dxc) = exe
        .and_then(|exe| exe.parent())
        .map(|dir| dir.join("dxcompiler.dll"))
        && dxc.is_absolute()
        && let Some(path) = dxc.to_str()
        && exists(&dxc)
    {
        return wgpu::Dx12Compiler::DynamicDxc {
            dxc_path: path.to_owned(),
        };
    }
    wgpu::Dx12Compiler::Fxc
}
/// Whether `backends` has an adapter whose device can be created, chosen and limited
/// as the window's will be (egui's defaults and [`required_limits`]). DirectX 12 and
/// Vulkan need real hardware: their software adapters must not hide a usable OpenGL
/// driver.
fn device_starts(backends: wgpu::Backends, backend_options: wgpu::BackendOptions) -> bool {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        backend_options,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference:
            wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance),
        force_fallback_adapter: false,
        compatible_surface: None,
        ..Default::default()
    })) else {
        return false;
    };
    let gl = backends == wgpu::Backends::GL;
    if !gl && adapter.get_info().device_type == wgpu::DeviceType::Cpu {
        return false;
    }
    let required_limits = if gl {
        wgpu::Limits {
            max_texture_dimension_2d: 8192,
            ..wgpu::Limits::downlevel_webgl2_defaults()
        }
    } else {
        required_limits(&adapter)
    };
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("RAWmakase backend check"),
        required_limits,
        ..Default::default()
    }))
    .is_ok()
}
/// Device limits previews need: storage buffers for a whole photo kept on the device
/// (735 MB for 61 megapixels), up to 2 GB where the adapter allows.
pub fn required_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    let limits = adapter.limits();
    wgpu::Limits {
        max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size.min(2 << 30),
        max_buffer_size: limits.max_buffer_size.min(2 << 30),
        max_texture_dimension_2d: limits.max_texture_dimension_2d,
        ..wgpu::Limits::default()
    }
}
impl Processor {
    /// A device of its own. Fails cleanly if no hardware compute adapter is available.
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: backends(),
            backend_options: backend_options(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))?;
        let info = adapter.get_info();
        ensure!(
            info.device_type != wgpu::DeviceType::Cpu,
            "Software GPU adapter"
        );
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("RAWmakase preview compute"),
                required_limits: required_limits(&adapter),
                ..Default::default()
            }))?;
        Self::with_device(device, queue, &info)
    }
    /// Uses the UI's device and queue, so previews can be presented into textures it
    /// draws. The device should have [`required_limits`]; smaller limits only make
    /// large regions fall back to the CPU.
    pub fn with_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        adapter: &wgpu::AdapterInfo,
    ) -> Result<Self> {
        ensure!(
            adapter.device_type != wgpu::DeviceType::Cpu,
            "Software GPU adapter"
        );
        ensure!(
            adapter.backend != wgpu::Backend::Gl,
            "OpenGL adapters are not used for compute"
        );
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let allocation = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Preview finishing"),
            source: wgpu::ShaderSource::Wgsl(include_str!("finish.wgsl").into()),
        });
        let entries: Vec<_> = (0..5)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 4 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: binding == 3,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Preview buffers"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Preview compute layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipelines = [
            "blur_horizontal",
            "sharpen",
            "resize_vertical",
            "resize_horizontal",
        ]
        .map(|entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        let memory_error = pollster::block_on(allocation.pop());
        let internal_error = pollster::block_on(internal.pop());
        let validation_error = pollster::block_on(validation.pop());
        if let Some(error) = memory_error.or(internal_error).or(validation_error) {
            anyhow::bail!("{error}");
        }
        Ok(Self {
            device,
            queue,
            layout,
            pipelines,
            buffers: None,
            developer: None,
            presenter: None,
            resident: None,
            pyramid: None,
            kept: Vec::new(),
            name: adapter.name.clone(),
        })
    }
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether `image` fits the GPU Laplacian pyramid buffer and dispatch limits.
    pub(crate) fn fits_pyramid(&self, image: &crate::camera_data::CameraImage) -> bool {
        let limits = self.device.limits();
        let max_binding = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let max_wg = limits.max_compute_workgroups_per_dimension;
        let bytes = image.pixels.len() as u64 * 12;
        bytes > 0
            && bytes <= max_binding
            && image.width.div_ceil(16) <= max_wg
            && image.height.div_ceil(16) <= max_wg
    }

    /// Runs Color and/or Luminance noise reduction on `base` through the shared GPU
    /// Laplacian pyramid runner (`pyramid.rs`), returning a denoised camera image.
    pub(crate) fn denoise(
        &mut self,
        base: &crate::camera_data::CameraImage,
        chroma: Option<super::color_noise::ChromaDenoise>,
        luminance: Option<super::luminance_noise::LuminanceDenoise>,
        cancel: Cancel<'_>,
    ) -> Result<crate::camera_data::CameraImage> {
        cancel.check("Render superseded")?;
        ensure!(self.fits_pyramid(base), "Photo exceeds GPU pyramid limits");
        self.scoped_pyramid(|gpu| {
            let device = &gpu.device;
            let runner = gpu
                .pyramid
                .get_or_insert_with(|| pyramid::GpuPyramid::new(device));
            let host = ProcessorHost {
                device,
                queue: &gpu.queue,
            };
            let photo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Preview pyramid photo"),
                contents: bytemuck::cast_slice(&base.pixels),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            });
            let photo_scale = base.photo_scale();
            if let Some(d) = chroma {
                runner.apply_chroma_denoise(
                    &host,
                    &photo,
                    base.width,
                    base.height,
                    base.metadata.wb,
                    photo_scale,
                    d,
                    cancel,
                )?;
            }
            if let Some(d) = luminance {
                runner.apply_luminance_denoise(
                    &host,
                    &photo,
                    base.width,
                    base.height,
                    base.metadata.wb,
                    photo_scale,
                    d,
                    cancel,
                )?;
            }
            let pixels = pyramid::GpuPyramid::readback_photo(
                &host,
                &photo,
                base.width,
                base.height,
                cancel,
            )?;
            let mut out = base.clone();
            out.recovered = Default::default();
            out.pixels = pixels.into();
            Ok(out)
        })
    }

    /// Computes measured [`super::texture::TextureDetail`] for `image` at `scale` through
    /// the shared GPU Laplacian pyramid runner (`pyramid.rs`), reusing the resident GPU
    /// photo buffer when `image` is already uploaded.
    pub(crate) fn texture_detail(
        &mut self,
        image: &std::sync::Arc<crate::camera_data::CameraImage>,
        scale: f32,
        cancel: Cancel<'_>,
    ) -> Result<super::texture::TextureDetail> {
        cancel.check("Render superseded")?;
        ensure!(self.fits_pyramid(image), "Photo exceeds GPU pyramid limits");
        self.scoped_pyramid(|gpu| {
            let existing = gpu.resident.as_ref().and_then(|r| r.photo_buffer(image));
            let device = &gpu.device;
            let photo = existing.unwrap_or_else(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Preview texture photo"),
                    contents: bytemuck::cast_slice(&image.pixels),
                    usage: wgpu::BufferUsages::STORAGE,
                })
            });
            let runner = gpu
                .pyramid
                .get_or_insert_with(|| pyramid::GpuPyramid::new(device));
            let host = ProcessorHost {
                device,
                queue: &gpu.queue,
            };
            let offset = super::texture::level_offset(image, scale);
            runner.compute_texture_detail(&host, &photo, image.width, image.height, offset, cancel)
        })
    }

    /// Sharpen with a blur of `sigma` (the recipe's, scaled to the image's pixels),
    /// then resize. Does not mutate the CPU input on success or failure.
    pub fn finish(
        &mut self,
        image: &Rendered,
        recipe: &Recipe,
        sigma: f32,
        max_edge: u32,
        cancel: Cancel<'_>,
    ) -> Result<Rendered> {
        cancel.check("Render superseded")?;
        recipe.validate()?;
        ensure!(sigma.is_finite() && sigma > 0., "Invalid sharpening radius");
        ensure!(
            image.width > 0
                && image.height > 0
                && image.pixels.len() as u64 == image.width as u64 * image.height as u64,
            "Invalid GPU image dimensions"
        );
        let (width, height) = super::quality::output_size(image.width, image.height, max_edge);
        let dimensions = [image.width, image.height, width, height];
        let source_bytes = image.pixels.len() as u64 * 12;
        let output_bytes = width as u64 * height as u64 * 12;
        let resize = (width, height) != (image.width, image.height);
        let scratch_bytes = (image.pixels.len() as u64 * 4).max(if resize {
            image.width as u64 * height as u64 * 12
        } else {
            0
        });
        let limit = self.device.limits().max_storage_buffer_binding_size;
        ensure!(
            [source_bytes, output_bytes, scratch_bytes]
                .into_iter()
                .all(|n| n <= limit && n <= self.device.limits().max_buffer_size),
            "Image exceeds GPU buffer limits"
        );
        ensure!(
            source_bytes + scratch_bytes + output_bytes + if resize { output_bytes } else { 4 }
                <= 1024 * 1024 * 1024,
            "Preview exceeds GPU memory budget"
        );
        ensure!(
            image.width.div_ceil(16) <= self.device.limits().max_compute_workgroups_per_dimension
                && image.height.div_ceil(16)
                    <= self.device.limits().max_compute_workgroups_per_dimension,
            "Image exceeds GPU dispatch limits"
        );
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let allocation = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let result = self.finish_inner(
            image,
            recipe,
            sigma,
            dimensions,
            [source_bytes, scratch_bytes, output_bytes],
            cancel,
        );
        let memory_error = pollster::block_on(allocation.pop());
        let internal_error = pollster::block_on(internal.pop());
        let validation_error = pollster::block_on(validation.pop());
        if let Some(error) = memory_error.or(internal_error).or(validation_error) {
            self.buffers = None;
            anyhow::bail!("{error}");
        }
        result
    }
    fn finish_inner(
        &mut self,
        image: &Rendered,
        recipe: &Recipe,
        sigma: f32,
        dimensions: [u32; 4],
        sizes: [u64; 3],
        cancel: Cancel<'_>,
    ) -> Result<Rendered> {
        let [w, h, ow, oh] = dimensions;
        let resize = w != ow || h != oh;
        if self
            .buffers
            .as_ref()
            .is_none_or(|b| b.dimensions != dimensions)
        {
            self.buffers = None; // Release the old allocation before allocating another photo.
            let buffer = |label, size, usage| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage,
                    mapped_at_creation: false,
                })
            };
            self.buffers = Some(Buffers {
                dimensions,
                source: buffer(
                    "Preview RGB",
                    sizes[0],
                    wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC,
                ),
                scratch: buffer("Preview scratch", sizes[1], wgpu::BufferUsages::STORAGE),
                output: buffer(
                    "Resized preview",
                    if resize { sizes[2] } else { 4 },
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                ),
                staging: buffer(
                    "Preview readback",
                    sizes[2],
                    wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                ),
            });
        }
        let b = self.buffers.as_ref().unwrap();
        let sharpener = crate::develop::sharpening::Sharpener::new(recipe);
        // The CPU's taps, so a reduced image's sigma below half a pixel sharpens alike.
        let (radius, mut weights) = crate::develop::quality::gaussian(sigma);
        let radius = radius as u32;
        let (x_stride, x_weights) = weights::lanczos_axis(w, ow);
        let (y_stride, y_weights) = weights::lanczos_axis(h, oh);
        weights.extend(x_weights);
        let y_offset = weights.len() as u32;
        weights.extend(y_weights);
        let coefficients = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Preview coefficients"),
                contents: bytemuck::cast_slice(&weights),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let params = uniforms::FinishParams {
            width: w,
            height: h,
            out_width: ow,
            out_height: oh,
            radius,
            x_stride,
            y_stride,
            y_offset,
            amount: recipe.sharpening * sharpener.gain,
            threshold: sharpener.threshold,
            halo: sharpener.halo,
            dark: sharpener.dark,
        };
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Preview parameters"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let entries: Vec<_> = [&b.source, &b.scratch, &b.output, &coefficients, &uniform]
            .into_iter()
            .enumerate()
            .map(|(binding, buffer)| wgpu::BindGroupEntry {
                binding: binding as u32,
                resource: buffer.as_entire_binding(),
            })
            .collect();
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Preview compute bindings"),
            layout: &self.layout,
            entries: &entries,
        });
        self.queue
            .write_buffer(&b.source, 0, bytemuck::cast_slice(&image.pixels));
        cancel.check("Render superseded")?;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut dispatch = |pipeline: usize, width: u32, height: u32| {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Preview finishing pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipelines[pipeline]);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1);
        };
        if recipe.sharpening != 0. {
            dispatch(0, w, h);
            dispatch(1, w, h);
        }
        if resize {
            dispatch(2, w, oh);
            dispatch(3, ow, oh);
        }
        encoder.copy_buffer_to_buffer(
            if resize { &b.output } else { &b.source },
            0,
            &b.staging,
            0,
            sizes[2],
        );
        let submission = submit(&self.queue, encoder);
        let (tx, rx) = mpsc::sync_channel(1);
        b.staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        let poll = wait(&self.device, submission);
        if let Err(error) = poll {
            b.staging.unmap();
            return Err(error.into());
        }
        rx.recv_timeout(Duration::from_secs(1))
            .context("GPU readback timed out")??;
        if cancel.is_cancelled() {
            b.staging.unmap();
            return Err(rawmakase_cancel::Cancelled::new("Render superseded").into());
        }
        let mapped = b.staging.slice(..).get_mapped_range()?;
        let pixels = bytemuck::cast_slice::<u8, [f32; 3]>(&mapped).to_vec();
        drop(mapped);
        b.staging.unmap();
        Ok(Rendered {
            width: ow,
            height: oh,
            pixels,
        })
    }
}

/// Typed error returned when a GPU Laplacian pyramid stage hits device memory limits,
/// allowing callers to fall back to CPU for that photo without disabling GPU pyramids
/// or resident GPU sampling for subsequent smaller photos.
#[derive(Debug)]
pub(crate) struct PyramidOutOfMemory(pub(crate) String);

impl std::fmt::Display for PyramidOutOfMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PyramidOutOfMemory {}

#[cfg(test)]
mod tests;
