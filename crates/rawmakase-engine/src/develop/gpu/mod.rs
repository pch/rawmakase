//! Optional compute backend for previews: the per-pixel develop stage, sharpening,
//! Lanczos resizing and display finishing.
//!
//! On the desktop it shares the UI's wgpu device, so a preview is finished straight
//! into a texture the viewport draws (`present.rs`). Headless callers (tests, the
//! benchmark) create their own device. Buffers are bounded and reused for equal
//! dimensions; callers retain CPU pixels for fallback.
use crate::model::recipe::Recipe;
use crate::rendered::Rendered;
use anyhow::{Context, Result, ensure};
use std::{
    sync::{
        PoisonError, RwLock, RwLockWriteGuard,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use wgpu::util::DeviceExt;
mod develop;
mod map;
mod present;
mod resident;
pub(crate) mod sampling;
mod uniforms;
mod weights;
pub(crate) use develop::Input;
pub(crate) use map::DeviceMap;
pub(crate) use present::Finish;
pub use present::{Display, Frame, MonitorLut, Slot};

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
    /// The develop stage's tone stage kept for its samples (see `develop::Kept`), most
    /// recently used first.
    kept: Vec<develop::Kept>,
    /// Shadows/Highlights map building, created on first use.
    mapper: Option<map::Mapper>,
    /// Map bases on the device by the tone stage they were made with, most recently
    /// used first (see `map::DeviceMap`).
    maps: Vec<(crate::develop::stage_cache::ToneKey, DeviceMap)>,
    name: String,
}
struct Buffers {
    dimensions: [u32; 4],
    source: wgpu::Buffer,
    scratch: wgpu::Buffer,
    output: wgpu::Buffer,
    staging: wgpu::Buffer,
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
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
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
            kept: Vec::new(),
            mapper: None,
            maps: Vec::new(),
            name: adapter.name.clone(),
        })
    }
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sharpen, then resize. Does not mutate the CPU input on success or failure.
    pub fn finish(
        &mut self,
        image: &Rendered,
        recipe: &Recipe,
        max_edge: u32,
        cancel: &AtomicBool,
    ) -> Result<Rendered> {
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        recipe.validate()?;
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
        dimensions: [u32; 4],
        sizes: [u64; 3],
        cancel: &AtomicBool,
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
        let radius = (sharpener.sigma * 3.).ceil() as u32;
        let mut weights: Vec<f32> = (-(radius as i32)..=radius as i32)
            .map(|x| (-0.5 * (x as f32 / sharpener.sigma).powi(2)).exp())
            .collect();
        let sum: f32 = weights.iter().sum();
        for v in &mut weights {
            *v /= sum;
        }
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
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
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
        if cancel.load(Ordering::Relaxed) {
            b.staging.unmap();
            anyhow::bail!("Render superseded");
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

#[cfg(test)]
mod tests;
