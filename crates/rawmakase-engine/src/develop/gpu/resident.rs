//! The stages before the per-pixel develop stage, on the device (`local.wgsl`): the
//! photo is uploaded once and kept there, reduced for the Shadows/Highlights map, and
//! each region is sampled through geometry, lens correction and noise reduction
//! straight into the develop stage's input.
use super::sampling;
use super::{Processor, develop::DeviceSamples};
use crate::{camera_data::CameraImage, develop::stage_cache::SampleKey};
use anyhow::{Context, Result, ensure};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use wgpu::util::DeviceExt;

/// Device samples kept, as the stage cache keeps CPU samples.
const SAMPLES: usize = 4;

pub(super) struct Resident {
    /// sample_region, reduce_rows, reduce_toned, with their layouts.
    pipelines: Vec<(wgpu::BindGroupLayout, wgpu::ComputePipeline)>,
    photo: Option<(Arc<CameraImage>, wgpu::Buffer)>,
    pub(super) samples: Vec<(SampleKey, Arc<DeviceSamples>)>,
    /// The other image's state (Fit renders a pyramid level, 100% the photo), so
    /// switching views does not upload again.
    parked: Option<Parked>,
}
struct Parked {
    photo: (Arc<CameraImage>, wgpu::Buffer),
    samples: Vec<(SampleKey, Arc<DeviceSamples>)>,
}
const SAMPLE: usize = 0;
const REDUCE_ROWS: usize = 1;
const REDUCE: usize = 2;

fn storage(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
impl Resident {
    fn new(device: &wgpu::Device) -> Self {
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let pipeline = |module: &wgpu::ShaderModule, layouts: &[&wgpu::BindGroupLayout], entry| {
            let layouts: Vec<_> = layouts.iter().map(|l| Some(*l)).collect();
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &layouts,
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Local stages"),
            source: wgpu::ShaderSource::Wgsl(
                (super::sampling::wgsl_prelude() + include_str!("local.wgsl")).into(),
            ),
        });
        let entries: [(&str, Vec<wgpu::BindGroupLayoutEntry>); 3] = [
            (
                "sample_region",
                vec![
                    storage(10, true),
                    storage(12, true),
                    storage(14, false),
                    storage(15, false),
                ],
            ),
            (
                "reduce_rows",
                vec![storage(9, false), storage(10, true), storage(12, true)],
            ),
            (
                "reduce_toned",
                vec![storage(9, false), storage(12, true), storage(13, false)],
            ),
        ];
        let pipelines = entries
            .into_iter()
            .map(|(entry, entries)| {
                let layout = layout(entry, &entries);
                let pipeline = pipeline(&module, &[&layout], entry);
                (layout, pipeline)
            })
            .collect();
        Self {
            pipelines,
            photo: None,
            samples: Vec::new(),
            parked: None,
        }
    }
    fn release(&mut self) {
        self.photo = None;
        self.samples.clear();
        self.parked = None;
    }
    /// Makes `image` current if it is parked, parking the current one; `false` when
    /// `image` is neither.
    fn switch_to(&mut self, image: &Arc<CameraImage>) -> bool {
        let current = self.photo.take().map(|photo| Parked {
            photo,
            samples: std::mem::take(&mut self.samples),
        });
        let parked = std::mem::replace(&mut self.parked, current);
        match parked {
            Some(p) if Arc::ptr_eq(&p.photo.0, image) => {
                self.photo = Some(p.photo);
                self.samples = p.samples;
                true
            }
            _ => false,
        }
    }
}
/// Workgroups for `n` invocations of 256 as (x, y) within the device's limits.
fn groups(device: &wgpu::Device, n: u64) -> (u32, u32) {
    let groups = n.div_ceil(256) as u32;
    let max = device.limits().max_compute_workgroups_per_dimension;
    (groups.min(max), groups.div_ceil(groups.min(max).max(1)))
}
fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    entries: &[(u32, &wgpu::Buffer)],
) -> wgpu::BindGroup {
    let entries: Vec<_> = entries
        .iter()
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: buffer.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &entries,
    })
}
fn dispatch(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    groups: &[&wgpu::BindGroup],
    size: (u32, u32),
) {
    let mut pass = encoder.begin_compute_pass(&Default::default());
    pass.set_pipeline(pipeline);
    for (i, group) in groups.iter().enumerate() {
        pass.set_bind_group(i as u32, *group, &[]);
    }
    pass.dispatch_workgroups(size.0, size.1, 1);
}

impl Processor {
    fn resident(&mut self) -> &mut Resident {
        let device = &self.device;
        self.resident.get_or_insert_with(|| Resident::new(device))
    }
    /// Whether `image` and its per-pixel buffers fit the device.
    pub(crate) fn fits_resident(&self, image: &CameraImage) -> bool {
        let limits = self.device.limits();
        // Four values per pixel, the limit this path has always kept to (the photo's
        // buffer holds three).
        let bytes = image.pixels.len() as u64 * 16;
        bytes <= limits.max_storage_buffer_binding_size && bytes <= limits.max_buffer_size
    }
    /// The photo's buffer, uploaded once while it stays the current photo.
    fn photo(&mut self, image: &Arc<CameraImage>) -> wgpu::Buffer {
        let device = self.device.clone();
        let resident = self.resident();
        if let Some((kept, buffer)) = &resident.photo
            && Arc::ptr_eq(kept, image)
        {
            return buffer.clone();
        }
        if resident.switch_to(image) {
            return resident.photo.as_ref().unwrap().1.clone();
        }
        // A photo nothing else holds any more (another photo was opened) is not kept.
        if resident
            .parked
            .as_ref()
            .is_some_and(|p| Arc::strong_count(&p.photo.0) == 1)
        {
            resident.parked = None;
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Photo"),
            contents: bytemuck::cast_slice(&image.pixels),
            usage: wgpu::BufferUsages::STORAGE,
        });
        resident.photo = Some((image.clone(), buffer.clone()));
        buffer
    }
    fn buffer(&self, label: &str, bytes: u64) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.max(16),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
    /// The toned photo reduced to `size` (`pipeline::preview_source`), read back for
    /// the Shadows/Highlights map.
    pub(crate) fn reduce_toned(
        &mut self,
        image: &Arc<CameraImage>,
        (w, h): (u32, u32),
        cancel: &AtomicBool,
    ) -> Result<CameraImage> {
        ensure!(self.fits_resident(image), "Photo exceeds GPU buffer limits");
        let photo = self.photo(image);
        let device = self.device.clone();
        let mut header = vec![0f32; sampling::HEADER];
        header[sampling::WIDTH.start] = image.width as f32;
        header[sampling::HEIGHT.start] = image.height as f32;
        header[sampling::REDUCED].copy_from_slice(&[w as f32, h as f32]);
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Reduce parameters"),
            contents: bytemuck::cast_slice(&header),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bytes = w as u64 * h as u64 * 12;
        let out = self.buffer("Reduced photo", bytes);
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Reduced readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let partial = self.buffer("Row sums", w as u64 * image.height as u64 * 12);
        let resident = self.resident();
        let mut encoder = device.create_command_encoder(&Default::default());
        let (layout, pipeline) = &resident.pipelines[REDUCE_ROWS];
        let group = bind(
            &device,
            layout,
            &[(9, &partial), (10, &photo), (12, &params)],
        );
        dispatch(
            &mut encoder,
            pipeline,
            &[&group],
            (w.div_ceil(16), image.height.div_ceil(16)),
        );
        let (layout, pipeline) = &resident.pipelines[REDUCE];
        let group = bind(&device, layout, &[(9, &partial), (12, &params), (13, &out)]);
        dispatch(
            &mut encoder,
            pipeline,
            &[&group],
            (w.div_ceil(16), h.div_ceil(16)),
        );
        encoder.copy_buffer_to_buffer(&out, 0, &staging, 0, bytes);
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        let submission = super::submit(&self.queue, encoder);
        let (tx, rx) = mpsc::sync_channel(1);
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        super::wait(&device, submission)?;
        rx.recv_timeout(Duration::from_secs(1))
            .context("GPU readback timed out")??;
        let pixels =
            bytemuck::cast_slice::<u8, [f32; 3]>(&staging.slice(..).get_mapped_range()?).to_vec();
        staging.unmap();
        Ok(CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels,
            metadata: image.metadata.clone(),
            fast: image.fast,
            scale_factor: image.scale_factor,
            scale_clipped: image.scale_clipped,
        })
    }
    /// Samples a region on the device (`pipeline::sample_region`), reusing the samples
    /// for an equal `key`. `params` holds geometry, lens and noise reduction from `S_OUT`
    /// on (see `local.wgsl`).
    pub(crate) fn sample(
        &mut self,
        image: &Arc<CameraImage>,
        mut params: Vec<f32>,
        (width, height): (u32, u32),
        key: SampleKey,
        cancel: &AtomicBool,
    ) -> Result<Arc<DeviceSamples>> {
        ensure!(self.fits_resident(image), "Photo exceeds GPU buffer limits");
        let n = width as u64 * height as u64;
        ensure!(
            n > 0 && n * 12 <= self.device.limits().max_storage_buffer_binding_size,
            "Region exceeds GPU buffer limits"
        );
        let photo = self.photo(image);
        let resident = self.resident();
        if let Some(i) = resident.samples.iter().position(|(k, _)| *k == key) {
            let entry = resident.samples.remove(i);
            let samples = entry.1.clone();
            resident.samples.insert(0, entry);
            return Ok(samples);
        }
        let device = self.device.clone();
        params[sampling::WIDTH.start] = image.width as f32;
        params[sampling::HEIGHT.start] = image.height as f32;
        let (gx, gy) = groups(&device, n);
        params[sampling::COUNT.start] = gx as f32;
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sampling parameters"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let samples = DeviceSamples {
            width,
            height,
            pixels: self.buffer("Device samples", n * 12),
            positions: self.buffer("Device positions", n * 8),
            output: self.buffer("Developed pixels", n * 12),
        };
        let resident = self.resident();
        let mut encoder = device.create_command_encoder(&Default::default());
        let (layout, pipeline) = &resident.pipelines[SAMPLE];
        let group = bind(
            &device,
            layout,
            &[
                (10, &photo),
                (12, &params),
                (14, &samples.pixels),
                (15, &samples.positions),
            ],
        );
        dispatch(&mut encoder, pipeline, &[&group], (gx, gy));
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        super::submit(&self.queue, encoder);
        let samples = Arc::new(samples);
        let resident = self.resident();
        resident.samples.insert(0, (key, samples.clone()));
        resident.samples.truncate(SAMPLES);
        Ok(samples)
    }
    /// Drops everything kept on the device for the resident path.
    pub(super) fn release_resident(&mut self) {
        if let Some(r) = &mut self.resident {
            r.release();
        }
    }
}
