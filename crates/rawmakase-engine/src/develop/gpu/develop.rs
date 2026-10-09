//! GPU port of the per-pixel color and tone stage (`develop.wgsl`). Samples from the
//! stage cache stay on the device while only the recipe changes; parameters and tables
//! are uploaded per render. The result is read back for the CPU finishing steps.
use super::Processor;
use crate::develop::pipeline::{Samples, pixel_params::PixelParams};
use crate::rendered::Rendered;
use anyhow::{Context, Result, ensure};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use wgpu::util::DeviceExt;

/// Invocations per workgroup, as declared in `develop.wgsl`.
const GROUP: u32 = 256;
/// Stand-in position for samples outside the photo; `develop.wgsl` tests against it
/// rather than NaN, which shader compilers may assume never occurs.
const OUTSIDE: f32 = -3e38;

pub(super) struct Developer {
    pub(super) layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    /// Uploaded sample sets, most recently used first, so switching between Fit and
    /// 100% does not upload again.
    samples: Vec<Uploaded>,
}
/// Uploaded sample sets kept, and their device memory budget.
const UPLOADS: usize = 3;
const UPLOAD_BUDGET: u64 = 512 << 20;
struct Uploaded {
    source: Arc<Samples>,
    pixels: wgpu::Buffer,
    positions: wgpu::Buffer,
    output: wgpu::Buffer,
    staging: wgpu::Buffer,
}
impl Developer {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let source =
            crate::develop::pipeline::pixel_params::wgsl_prelude() + include_str!("develop.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Develop"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let entries: Vec<_> = (0..7)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 5 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: binding != 2,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Develop buffers"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Develop layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Develop"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("develop"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            layout,
            pipeline,
            samples: Vec::new(),
        }
    }
}
/// Samples made on the device (`resident.rs`), and the develop stage's output.
pub(crate) struct DeviceSamples {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixels: wgpu::Buffer,
    pub(super) positions: wgpu::Buffer,
    pub(super) output: wgpu::Buffer,
}
/// What the develop stage reads: samples prepared on the CPU, or on the device.
#[derive(Clone, Copy)]
pub(crate) enum Input<'a> {
    Cpu(&'a Arc<Samples>),
    Device(&'a DeviceSamples),
}
impl Input<'_> {
    pub(super) fn size(&self) -> (u32, u32) {
        match self {
            Input::Cpu(s) => (s.width, s.height),
            Input::Device(s) => (s.width, s.height),
        }
    }
}
impl Developer {
    /// Makes `samples` the most recent upload, uploading them unless they are there.
    fn upload(&mut self, device: &wgpu::Device, samples: &Arc<Samples>) {
        let developer = self;
        let n = samples.pixels.len() as u64;
        if let Some(i) = developer
            .samples
            .iter()
            .position(|u| Arc::ptr_eq(&u.source, samples))
        {
            let uploaded = developer.samples.remove(i);
            developer.samples.insert(0, uploaded);
        } else {
            // Release the least recently used buffers before allocating more.
            let bytes = |u: &Uploaded| u.source.pixels.len() as u64 * 44;
            developer.samples.truncate(UPLOADS - 1);
            while developer.samples.iter().map(bytes).sum::<u64>() + n * 44 > UPLOAD_BUDGET
                && developer.samples.pop().is_some()
            {}
            let positions: Vec<[f32; 2]> = samples
                .positions
                .iter()
                .map(|p| if p[0].is_nan() { [OUTSIDE; 2] } else { *p })
                .collect();
            let init = |label, contents: &[u8]| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::STORAGE,
                })
            };
            let buffer = |label, usage| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: n * 12,
                    usage,
                    mapped_at_creation: false,
                })
            };
            let uploaded = Uploaded {
                source: samples.clone(),
                pixels: init("Develop samples", bytemuck::cast_slice(&samples.pixels)),
                positions: init("Develop positions", bytemuck::cast_slice(&positions)),
                output: buffer(
                    "Developed pixels",
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                ),
                staging: buffer(
                    "Develop readback",
                    wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                ),
            };
            developer.samples.insert(0, uploaded);
        }
    }
}
impl Processor {
    /// Uploads CPU samples unless they are on the device already, and records the
    /// develop pass; returns the buffer that holds its result.
    pub(super) fn record_develop(
        &mut self,
        input: Input,
        params: &PixelParams,
        encoder: &mut wgpu::CommandEncoder,
    ) -> wgpu::Buffer {
        let device = &self.device;
        let developer = self.developer.get_or_insert_with(|| Developer::new(device));
        let (w, h) = input.size();
        let n = w as u64 * h as u64;
        let (pixels, positions, output) = match input {
            Input::Device(s) => (s.pixels.clone(), s.positions.clone(), s.output.clone()),
            Input::Cpu(samples) => {
                developer.upload(device, samples);
                let u = &developer.samples[0];
                (u.pixels.clone(), u.positions.clone(), u.output.clone())
            }
        };
        let storage = |label, data: &[f32]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(data),
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let params_buffer = storage("Develop parameters", &params.params);
        let tables = storage("Develop tables", &params.tables);
        let weights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mask weights"),
            contents: bytemuck::cast_slice(if params.weights.is_empty() {
                &[0u32; 4]
            } else {
                &params.weights[..]
            }),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let groups = (n as u32).div_ceil(GROUP);
        let max = device.limits().max_compute_workgroups_per_dimension;
        let (gx, gy) = (groups.min(max), groups.div_ceil(groups.min(max)));
        let size = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Develop size"),
            contents: bytemuck::cast_slice(&[n as u32, gx, 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let entries: Vec<_> = [
            &pixels,
            &positions,
            &output,
            &params_buffer,
            &tables,
            &size,
            &weights,
        ]
        .into_iter()
        .enumerate()
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding: binding as u32,
            resource: buffer.as_entire_binding(),
        })
        .collect();
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Develop bindings"),
            layout: &developer.layout,
            entries: &entries,
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Develop pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&developer.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        output
    }
    /// The per-pixel stage over `samples`. Does not change CPU state on failure.
    pub(crate) fn develop(
        &mut self,
        samples: &Arc<Samples>,
        params: &PixelParams,
        cancel: &AtomicBool,
    ) -> Result<Rendered> {
        self.check_develop(Input::Cpu(samples), params, cancel)?;
        self.scoped(|gpu| gpu.develop_inner(samples, params, cancel))
    }
    /// Whether the device can develop `samples` at all.
    pub(super) fn check_develop(
        &self,
        input: Input,
        params: &PixelParams,
        cancel: &AtomicBool,
    ) -> Result<()> {
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        let (w, h) = input.size();
        let n = w as u64 * h as u64;
        ensure!(n > 0, "Empty develop region");
        let limits = self.device.limits();
        ensure!(
            n * 12 <= limits.max_storage_buffer_binding_size
                && n * 12 <= limits.max_buffer_size
                && (params.tables.len() as u64 * 4) <= limits.max_storage_buffer_binding_size,
            "Region exceeds GPU buffer limits"
        );
        ensure!(
            n * 44 <= 1024 * 1024 * 1024,
            "Region exceeds GPU memory budget"
        );
        Ok(())
    }
    /// Runs `work` inside validation, internal and memory error scopes; a device error
    /// releases the uploaded samples and resident buffers and fails.
    pub(crate) fn scoped<T>(&mut self, work: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let allocation = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let result = work(self);
        let memory_error = pollster::block_on(allocation.pop());
        let internal_error = pollster::block_on(internal.pop());
        let validation_error = pollster::block_on(validation.pop());
        if let Some(error) = memory_error.or(internal_error).or(validation_error) {
            if let Some(d) = &mut self.developer {
                d.samples.clear();
            }
            self.release_resident();
            anyhow::bail!("{error}");
        }
        result
    }
    fn develop_inner(
        &mut self,
        samples: &Arc<Samples>,
        params: &PixelParams,
        cancel: &AtomicBool,
    ) -> Result<Rendered> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.record_develop(Input::Cpu(samples), params, &mut encoder);
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        let device = &self.device;
        let n = samples.pixels.len() as u64;
        let uploaded = &self.developer.as_ref().unwrap().samples[0];
        encoder.copy_buffer_to_buffer(&uploaded.output, 0, &uploaded.staging, 0, n * 12);
        let submission = super::submit(&self.queue, encoder);
        let (tx, rx) = mpsc::sync_channel(1);
        uploaded
            .staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        let poll = super::wait(device, submission);
        if let Err(error) = poll {
            uploaded.staging.unmap();
            return Err(error.into());
        }
        rx.recv_timeout(std::time::Duration::from_secs(1))
            .context("GPU readback timed out")??;
        if cancel.load(Ordering::Relaxed) {
            uploaded.staging.unmap();
            anyhow::bail!("Render superseded");
        }
        let mapped = uploaded.staging.slice(..).get_mapped_range()?;
        let pixels = bytemuck::cast_slice::<u8, [f32; 3]>(&mapped).to_vec();
        drop(mapped);
        uploaded.staging.unmap();
        Ok(Rendered {
            width: samples.width,
            height: samples.height,
            pixels,
        })
    }
}
