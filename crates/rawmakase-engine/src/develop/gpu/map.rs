//! The engine 4 Shadows/Highlights map's base built on the device (`map.wgsl`), as
//! `local_tone::MapBase` builds it on the CPU, and kept there: the develop pass copies
//! it into its tables (`develop::record_develop`), so nothing is read back.
use super::Processor;
use super::develop::{DeviceSamples, Input};
use crate::camera_data::CameraImage;
use crate::develop::local_tone;
use crate::develop::pipeline::pixel_params::PixelParams;
use crate::develop::stage_cache::ToneKey;
use anyhow::Result;
use wgpu::util::DeviceExt;

/// Maps kept, most recently used first: the Fit's, its draft's and a 100% region's.
const MAPS: usize = 3;
const PREPARE: usize = 0;
const ROWS: usize = 1;
const COLS: usize = 2;
const HISTOGRAM: usize = 3;
const RESOLVE: usize = 4;

/// A map base on the device: the guided filter's coefficients a, then b, `size` cells
/// each, and the Shadows and Highlights keys. Its commands are submitted when it is
/// made; passes that read it are submitted after them.
#[derive(Clone)]
pub(crate) struct DeviceMap {
    pub(super) ab: wgpu::Buffer,
    pub(super) keys: wgpu::Buffer,
    pub(crate) size: [u32; 2],
}
impl DeviceMap {
    /// Cells of each coefficient.
    pub(crate) fn cells(&self) -> usize {
        self.size[0] as usize * self.size[1] as usize
    }
}
pub(super) struct Mapper {
    layout: wgpu::BindGroupLayout,
    pipelines: Vec<wgpu::ComputePipeline>,
}
impl Mapper {
    fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shadows/Highlights map"),
            source: wgpu::ShaderSource::Wgsl(include_str!("map.wgsl").into()),
        });
        let entries: Vec<_> = (0..7)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: match binding {
                        0 => wgpu::BufferBindingType::Uniform,
                        b => wgpu::BufferBindingType::Storage { read_only: b == 1 },
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Map buffers"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Map layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipelines = ["prepare", "rows", "cols", "histogram", "resolve"]
            .map(|entry| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .into();
        Self { layout, pipelines }
    }
}
impl Processor {
    /// The map base of the reduced photo `small` toned by `tone` (`pixel_params`
    /// with `TONE_ONLY`), for `key`: kept from an earlier render, or built on the
    /// device. Building submits its commands without waiting for them.
    pub(crate) fn device_map(
        &mut self,
        key: ToneKey,
        small: &CameraImage,
        tone: &PixelParams,
    ) -> Result<DeviceMap> {
        if let Some(i) = self.maps.iter().position(|(k, _)| *k == key) {
            let entry = self.maps.remove(i);
            let map = entry.1.clone();
            self.maps.insert(0, entry);
            return Ok(map);
        }
        let (w, h) = (small.width, small.height);
        let n = w as u64 * h as u64;
        anyhow::ensure!(n > 0, "Empty map");
        let device = self.device.clone();
        let init = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(16),
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE;
        let input = DeviceSamples {
            width: n as u32,
            height: 1,
            pixels: init("Map pixels", bytemuck::cast_slice(&small.pixels), storage),
            positions: buffer("Map positions", n * 8, storage),
            output: buffer("Toned map pixels", n * 12, storage),
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        let toned = self.record_develop(Input::Device(&input), tone, &mut encoder);
        let mapper = self.mapper.get_or_insert_with(|| Mapper::new(&device));
        let planes = buffer("Map planes", n * 8, storage);
        let means = buffer("Map row means", n * 8, storage);
        let ab = buffer("Map base", n * 8, storage | wgpu::BufferUsages::COPY_SRC);
        let bits = buffer("Map luminance bits", n * 4, storage);
        let hist = buffer(
            "Map key digits",
            2048,
            storage | wgpu::BufferUsages::COPY_DST,
        );
        let ranks = local_tone::key_ranks(n as usize).map(|k| k as u32);
        let state = init(
            "Map key selection",
            bytemuck::cast_slice(&[0, 0, ranks[0], ranks[1], 0, 0, 0, 0]),
            storage,
        );
        let keys = buffer("Map keys", 16, storage | wgpu::BufferUsages::COPY_SRC);
        let radius = local_tone::radius(w as usize, h as usize) as u32;
        let uniform = |mode: u32| {
            let values = [w, h, radius, mode, local_tone::EPSILON.to_bits(), 0, 0, 0];
            init(
                "Map parameters",
                bytemuck::cast_slice(&values),
                wgpu::BufferUsages::UNIFORM,
            )
        };
        let (coefficients, means_only) = (uniform(0), uniform(1));
        let group = |uniform: &wgpu::Buffer, src: &wgpu::Buffer, dst: &wgpu::Buffer| {
            let entries: Vec<_> = [uniform, src, dst, &bits, &hist, &state, &keys]
                .into_iter()
                .enumerate()
                .map(|(binding, buffer)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: buffer.as_entire_binding(),
                })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Map bindings"),
                layout: &mapper.layout,
                entries: &entries,
            })
        };
        let groups = (n as u32).div_ceil(256);
        let all = (groups.min(65535), groups.div_ceil(65535));
        let pass = |encoder: &mut wgpu::CommandEncoder, entry, group, size: (u32, u32)| {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Map pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&mapper.pipelines[entry]);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(size.0, size.1, 1);
        };
        // The guided filter: means of the log luminance and its square, the
        // coefficients, and their means.
        let steps = [
            (PREPARE, group(&coefficients, &toned, &planes)),
            (ROWS, group(&coefficients, &planes, &means)),
            (COLS, group(&coefficients, &means, &planes)),
            (ROWS, group(&coefficients, &planes, &means)),
            (COLS, group(&means_only, &means, &ab)),
        ];
        for (entry, group) in steps {
            pass(&mut encoder, entry, group, all);
        }
        // The keys, a byte of the luminance at a time.
        for _ in 0..4 {
            encoder.clear_buffer(&hist, 0, None);
            pass(
                &mut encoder,
                HISTOGRAM,
                group(&coefficients, &planes, &means),
                all,
            );
            pass(
                &mut encoder,
                RESOLVE,
                group(&coefficients, &planes, &means),
                (1, 1),
            );
        }
        super::submit(&self.queue, encoder);
        super::develop::submitted(&mut self.kept);
        let map = DeviceMap {
            ab,
            keys,
            size: [w, h],
        };
        self.maps.insert(0, (key, map.clone()));
        self.maps.truncate(MAPS);
        Ok(map)
    }
}
