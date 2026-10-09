//! The stages before the per-pixel develop stage, on the device (`logs.wgsl`,
//! `local.wgsl`): the photo is uploaded once and kept, the local-tone blurs are
//! computed and kept there, and each region is sampled through geometry, lens
//! correction and noise reduction straight into the develop stage's input. The gain
//! the blurs give is computed only for the pixels a region samples (and on the fly
//! for the Shadows/Highlights map's reduction), so a Clarity edit does not touch the
//! whole photo unless that map needs it.
use super::sampling;
use super::{
    Processor,
    develop::{Developer, DeviceSamples},
};
use crate::{
    camera_data::CameraImage,
    develop::{
        pipeline::pixel_params::PixelParams,
        stage_cache::{BlurKey, SampleKey},
    },
};
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

/// The local-tone stage of the current photo: log luminance and its blurs in one
/// buffer (`local.wgsl`), and the sliders applied to them.
#[derive(Clone)]
pub(crate) struct LocalTones {
    buffer: wgpu::Buffer,
    texture: bool,
    /// Exposure, Clarity, Texture.
    sliders: [f32; 3],
}
pub(super) struct Resident {
    logs_layout: wgpu::BindGroupLayout,
    logs: wgpu::ComputePipeline,
    /// running_sum, window, local_gain, sample_region, reduce_toned, with their layouts.
    pipelines: Vec<(wgpu::BindGroupLayout, wgpu::ComputePipeline)>,
    dummy: wgpu::Buffer,
    photo: Option<(Arc<CameraImage>, wgpu::Buffer)>,
    blurs: Option<(BlurKey, wgpu::Buffer)>,
    pub(super) samples: Vec<(SampleKey, Arc<DeviceSamples>)>,
    /// The other image's state (Fit renders a pyramid level, 100% the photo), so
    /// switching views does not upload and blur again.
    parked: Option<Parked>,
}
struct Parked {
    photo: (Arc<CameraImage>, wgpu::Buffer),
    blurs: Option<(BlurKey, wgpu::Buffer)>,
    samples: Vec<(SampleKey, Arc<DeviceSamples>)>,
}
const RUNNING_SUM: usize = 0;
const WINDOW: usize = 1;
const GAIN: usize = 2; // region_gain
const SAMPLE: usize = 3;
const REDUCE_ROWS: usize = 4;
const REDUCE: usize = 5;

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
fn uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
impl Resident {
    fn new(device: &wgpu::Device, developer: &Developer) -> Self {
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
        let logs_source = crate::develop::pipeline::pixel_params::wgsl_prelude()
            + include_str!("develop.wgsl")
            + include_str!("logs.wgsl");
        let logs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Log luminance"),
            source: wgpu::ShaderSource::Wgsl(logs_source.into()),
        });
        let logs_layout = layout(
            "Log luminance",
            &[
                storage(0, true),
                storage(1, false),
                storage(2, true),
                uniform(3),
            ],
        );
        let logs = pipeline(
            &logs_module,
            &[&developer.camera_layout, &logs_layout],
            "log_luminance",
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Local stages"),
            source: wgpu::ShaderSource::Wgsl(
                (super::sampling::wgsl_prelude() + include_str!("local.wgsl")).into(),
            ),
        });
        let entries: [(&str, Vec<wgpu::BindGroupLayoutEntry>); 6] = [
            (
                "running_sum",
                vec![storage(0, true), storage(1, false), uniform(3)],
            ),
            (
                "window",
                vec![storage(1, false), storage(2, false), uniform(3)],
            ),
            (
                "region_gain",
                vec![storage(4, true), storage(8, false), storage(12, true)],
            ),
            (
                "sample_region",
                vec![
                    storage(4, true),
                    storage(10, true),
                    storage(11, true),
                    storage(12, true),
                    storage(14, false),
                    storage(15, false),
                ],
            ),
            (
                "reduce_rows",
                vec![
                    storage(4, true),
                    storage(9, false),
                    storage(10, true),
                    storage(12, true),
                ],
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
            logs_layout,
            logs,
            pipelines,
            dummy: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Unused binding"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::UNIFORM,
                mapped_at_creation: false,
            }),
            photo: None,
            blurs: None,
            samples: Vec::new(),
            parked: None,
        }
    }
    fn release(&mut self) {
        self.photo = None;
        self.blurs = None;
        self.samples.clear();
        self.parked = None;
    }
    /// Makes `image` current if it is parked, parking the current one; `false` when
    /// `image` is neither.
    fn switch_to(&mut self, image: &Arc<CameraImage>) -> bool {
        let current = self.photo.take().map(|photo| Parked {
            photo,
            blurs: self.blurs.take(),
            samples: std::mem::take(&mut self.samples),
        });
        let parked = std::mem::replace(&mut self.parked, current);
        match parked {
            Some(p) if Arc::ptr_eq(&p.photo.0, image) => {
                self.photo = Some(p.photo);
                self.blurs = p.blurs;
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
        let developer = self.developer.get_or_insert_with(|| Developer::new(device));
        self.resident
            .get_or_insert_with(|| Resident::new(device, developer))
    }
    /// Whether `image` and its per-pixel buffers fit the device.
    pub(crate) fn fits_resident(&self, image: &CameraImage) -> bool {
        let limits = self.device.limits();
        // The blurs' buffer: four values per pixel.
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
    /// The local-tone stage of `image` on the device, as `quality::local_blurs`
    /// computes its blurs on the CPU; the gain follows from `sliders` (exposure,
    /// Clarity, Texture) where it is read. `camera` holds the camera
    /// stage of the blurs' recipe; `vignetting` the built-in vignetting and its radial
    /// table; `radii` the fine and (for Texture) texture box radii.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn local_tones(
        &mut self,
        image: &Arc<CameraImage>,
        camera: &PixelParams,
        vignetting: Option<&([f32; 4], Vec<f32>)>,
        radii: [Option<u32>; 2],
        sliders: [f32; 3],
        key: BlurKey,
        cancel: &AtomicBool,
    ) -> Result<LocalTones> {
        ensure!(self.fits_resident(image), "Photo exceeds GPU buffer limits");
        let photo = self.photo(image);
        let texture = radii[1].is_some();
        if let Some((kept, buffer)) = &self.resident().blurs
            && *kept == key
        {
            return Ok(LocalTones {
                buffer: buffer.clone(),
                texture,
                sliders,
            });
        }
        self.resident().blurs = None;
        let device = self.device.clone();
        let (w, h) = (image.width, image.height);
        let n = w as u64 * h as u64;
        let tones = self.buffer("Local tones", n * 4 * (2 + texture as u64));
        let mut encoder = device.create_command_encoder(&Default::default());
        let init = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let params = init(
            "Camera parameters",
            bytemuck::cast_slice(&camera.params),
            wgpu::BufferUsages::STORAGE,
        );
        let tables = init(
            "Camera tables",
            bytemuck::cast_slice(&camera.tables),
            wgpu::BufferUsages::STORAGE,
        );
        let unused = self.buffer("Unused", 16);
        let resident = self.resident.as_ref().unwrap();
        let developer = self.developer.as_ref().unwrap();
        // The develop layout's other bindings are unused by the log pass.
        let dummy = &resident.dummy;
        let camera_group = bind(
            &device,
            &developer.camera_layout,
            &[
                (0, dummy),
                (1, dummy),
                (2, &unused),
                (3, &params),
                (4, &tables),
                (5, dummy),
            ],
        );
        let (gx, gy) = groups(&device, n);
        let (offset, header, table) = match vignetting {
            Some((v, table)) => (0, *v, table.clone()),
            None => (-1, [0.; 4], vec![0.; 4]),
        };
        let mut values = [0u32; 12];
        values[..5].copy_from_slice(&[w, h, gx, offset as u32, table.len() as u32 / 2]);
        for (i, v) in header.iter().enumerate() {
            values[5 + i] = v.to_bits();
        }
        let log_uniform = init(
            "Log parameters",
            bytemuck::cast_slice(&values),
            wgpu::BufferUsages::UNIFORM,
        );
        let radial = init(
            "Vignetting",
            bytemuck::cast_slice(&table),
            wgpu::BufferUsages::STORAGE,
        );
        let group = bind(
            &device,
            &resident.logs_layout,
            &[(0, &photo), (1, &tones), (2, &radial), (3, &log_uniform)],
        );
        dispatch(
            &mut encoder,
            &resident.logs,
            &[&camera_group, &group],
            (gx, gy),
        );
        let prefix = self.buffer("Running sums", n * 4);
        let rows = self.buffer("Row blur", n * 4);
        let radii: Vec<u32> = radii.into_iter().flatten().collect();
        ensure!(!radii.is_empty(), "Missing blur radii");
        for (slot, radius) in radii.into_iter().enumerate() {
            // Rows of the logs into `rows`, then its columns into the blur's slot.
            let target = (slot as u32 + 1) * n as u32;
            for (axis, src, dst, from, to) in
                [(0u32, &tones, &rows, 0, 0), (1, &rows, &tones, 0, target)]
            {
                let uniform = init(
                    "Blur parameters",
                    bytemuck::cast_slice(&[w, h, radius, axis, from, to, 0, 0]),
                    wgpu::BufferUsages::UNIFORM,
                );
                let (layout, pipeline) = &resident.pipelines[RUNNING_SUM];
                let group = bind(&device, layout, &[(0, src), (1, &prefix), (3, &uniform)]);
                let lines = if axis == 0 { h } else { w };
                dispatch(&mut encoder, pipeline, &[&group], (lines.div_ceil(64), 1));
                let (layout, pipeline) = &resident.pipelines[WINDOW];
                let group = bind(&device, layout, &[(1, &prefix), (2, dst), (3, &uniform)]);
                dispatch(
                    &mut encoder,
                    pipeline,
                    &[&group],
                    (w.div_ceil(16), h.div_ceil(16)),
                );
            }
        }
        // Blurs recorded here only exist once the encoder is submitted.
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        super::submit(&self.queue, encoder);
        self.resident().blurs = Some((key, tones.clone()));
        Ok(LocalTones {
            buffer: tones,
            texture,
            sliders,
        })
    }
    /// The first sampling-pass parameters for `image` and its local tones.
    fn header(image: &CameraImage, tones: Option<&LocalTones>) -> Vec<f32> {
        let mut header = vec![0f32; sampling::HEADER];
        header[sampling::WIDTH.start] = image.width as f32;
        header[sampling::HEIGHT.start] = image.height as f32;
        if let Some(t) = tones {
            header[sampling::GAIN.start] = 1.;
            let [sliders @ .., blur] = &mut header[sampling::SLIDERS] else {
                unreachable!("four slider slots")
            };
            sliders.copy_from_slice(&t.sliders);
            *blur = t.texture as u8 as f32;
        }
        header
    }
    /// The toned photo reduced to `size` (`pipeline::preview_source`), read back for
    /// the Shadows/Highlights map.
    pub(crate) fn reduce_toned(
        &mut self,
        image: &Arc<CameraImage>,
        tones: Option<&LocalTones>,
        (w, h): (u32, u32),
        cancel: &AtomicBool,
    ) -> Result<CameraImage> {
        ensure!(self.fits_resident(image), "Photo exceeds GPU buffer limits");
        let photo = self.photo(image);
        let device = self.device.clone();
        let mut header = Self::header(image, tones);
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
            &[
                (4, tones.map_or(&resident.dummy, |t| &t.buffer)),
                (9, &partial),
                (10, &photo),
                (12, &params),
            ],
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
    /// on (see `local.wgsl`); `bounds` (x, y, width, height) holds the photo pixels the
    /// region samples, whose local-tone gain is computed once for it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sample(
        &mut self,
        image: &Arc<CameraImage>,
        tones: Option<&LocalTones>,
        mut params: Vec<f32>,
        bounds: [u32; 4],
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
        let header = Self::header(image, tones);
        let size_and_gain = sampling::WIDTH.start..sampling::GAIN.end;
        params[size_and_gain.clone()].copy_from_slice(&header[size_and_gain]);
        params[sampling::SLIDERS].copy_from_slice(&header[sampling::SLIDERS]);
        let (gx, gy) = groups(&device, n);
        params[sampling::COUNT.start] = gx as f32;
        let box_pixels = bounds[2] as u64 * bounds[3] as u64;
        let (bx, by) = groups(&device, box_pixels);
        params[sampling::BOX].copy_from_slice(&bounds.map(|v| v as f32));
        params[sampling::BOX_COUNT.start] = bx as f32;
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
        let gains = match tones {
            Some(_) => self.buffer("Region gain", box_pixels * 4),
            None => self.resident().dummy.clone(),
        };
        let resident = self.resident();
        let local = tones.map_or(&resident.dummy, |t| &t.buffer);
        let mut encoder = device.create_command_encoder(&Default::default());
        if tones.is_some() && box_pixels > 0 {
            let (layout, pipeline) = &resident.pipelines[GAIN];
            let group = bind(&device, layout, &[(4, local), (8, &gains), (12, &params)]);
            dispatch(&mut encoder, pipeline, &[&group], (bx, by));
        }
        let (layout, pipeline) = &resident.pipelines[SAMPLE];
        let group = bind(
            &device,
            layout,
            &[
                (4, local),
                (10, &photo),
                (11, &gains),
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
