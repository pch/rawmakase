//! GPU port of the per-pixel color and tone stage (`develop.wgsl`). Samples from the
//! stage cache stay on the device while only the recipe changes; parameters and tables
//! are uploaded per render. The result is read back for the CPU finishing steps.
use super::Processor;
use crate::develop::pipeline::{
    Samples,
    pixel_params::{PixelParams, slot},
};
use crate::rendered::Rendered;
use anyhow::{Context, Result, ensure};
use rawmakase_cancel::Cancel;
use std::ops::Range;
use std::sync::{Arc, mpsc};
use wgpu::util::DeviceExt;

/// Invocations per workgroup, as declared in `develop.wgsl`.
const GROUP: u32 = 256;
/// Stand-in position for samples outside the photo; `develop.wgsl` tests against it
/// rather than NaN, which shader compilers may assume never occurs.
const OUTSIDE: f32 = -3e38;

/// Which features a develop pipeline is compiled with: `develop.wgsl` tests them as
/// constants, so a pipeline without one leaves its code out. A shader carrying every
/// feature runs fewer pixels at a time on small GPUs (an Intel UHD 620 ran it eight at
/// a time); a pass that needs less of it runs a smaller program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Variant {
    /// Mask adjustments.
    pub(crate) masks: bool,
    /// The colour mixer, Point Color, a look's RGB table, colour grading and the colour
    /// controls (`ADJUST`).
    pub(crate) color: bool,
    /// The Shadows/Highlights map.
    pub(crate) local: bool,
    /// Dehaze.
    pub(crate) dehaze: bool,
}
impl Variant {
    /// Everything.
    #[cfg(test)]
    pub(crate) const FULL: Self = Self {
        masks: true,
        color: true,
        local: true,
        dehaze: true,
    };
    /// The WGSL constants `develop.wgsl` reads.
    pub(crate) fn constants(self) -> String {
        format!(
            "const V_MASKS: bool = {};\nconst V_COLOR: bool = {};\nconst V_LOCAL: bool = {};\nconst V_DEHAZE: bool = {};\n",
            self.masks, self.color, self.local, self.dehaze
        )
    }
    /// What a pass with `params` needs.
    fn of(params: &PixelParams) -> Self {
        let set = |at: Range<usize>| params.get(at)[0] != 0.;
        let at = |at: Range<usize>| params.get(at)[0] >= 0.;
        let masks = !params.weights.is_empty();
        Self {
            masks,
            color: at(slot::MIXER)
                || at(slot::POINT)
                || (at(slot::RGB) && params.get(slot::RGB)[5] != 0.)
                || at(slot::GRADE)
                || set(slot::ADJUST),
            local: set(slot::LOCAL),
            // A mask's Dehaze adds to the recipe's.
            dehaze: set(slot::DEHAZE) || masks,
        }
    }
}
pub(super) struct Developer {
    pub(super) layout: wgpu::BindGroupLayout,
    /// Develop pipelines compiled so far, one per variant.
    pipelines: Vec<(Variant, wgpu::ComputePipeline)>,
    pipeline_layout: wgpu::PipelineLayout,
    /// The pass's parameters, tables and mask weights, kept and written in place:
    /// creating a buffer of them per render took about 25 ms on an Intel UHD 620.
    inputs: [Option<wgpu::Buffer>; 5],
    /// The Shadows/Highlights map's grids on the device, most recently used first: one
    /// per map base, which a slider drag's renders share (`PixelParams::grid`).
    grids: Vec<(crate::develop::local_tone::MapGrid, wgpu::Buffer)>,
    /// Uploaded sample sets, most recently used first, so switching between Fit and
    /// 100% does not upload again.
    samples: Vec<Uploaded>,
}
/// The camera colour (S1: white balance, the profile's matrix and HueSatMap, Camera
/// Calibration) of one set of samples, for the recipe as it reads it
/// (`PixelParams::color`): no Basic slider, nor Exposure, changes it, so the develop pass
/// reads it instead of computing it again while they move.
pub(super) struct Kept {
    pixels: wgpu::Buffer,
    positions: wgpu::Buffer,
    recipe: crate::model::recipe::Recipe,
    color: wgpu::Buffer,
    /// A submitted pass wrote `color` for these samples and `recipe`. Until then it is
    /// written again: a pass recorded but never submitted (a cancelled render) left it
    /// as it was. `recipe` is that of the last render of these samples, written or not.
    written: bool,
    /// The pass being recorded writes it; marked written once submitted. Recording the
    /// next pass clears it: a pass recorded earlier was never submitted.
    pending: bool,
}
/// What a develop pass does with the kept camera colour of its samples; the value is
/// the shader's `size.z`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Keep {
    /// Computes the colour.
    Run = 0,
    /// Reads the kept colour.
    Read = 1,
    /// Computes the colour and keeps it.
    Write = 2,
}
impl Keep {
    /// From the samples' entry, when there is one: whether its recipe is this render's,
    /// and whether a submitted pass wrote it. It is kept from the second render with the
    /// same recipe, so a white balance drag, where every value is new, does not pay for
    /// writing it.
    pub(super) fn step(entry: Option<(bool, bool)>) -> Self {
        match entry {
            Some((true, true)) => Self::Read,
            Some((true, false)) => Self::Write,
            _ => Self::Run,
        }
    }
}
/// Sample sets whose camera colour is kept: the Fit and its draft, and a 100% region.
const KEPT: usize = 3;
impl Kept {
    /// Whether a submitted pass wrote it, so later renders read it.
    #[cfg(test)]
    pub(super) fn read_once(&self) -> bool {
        self.written
    }
}
/// Marks the kept colour written once the pass that writes it is submitted: the last
/// recorded, as each develop pass is submitted before the next is recorded.
pub(super) fn submitted(kept: &mut [Kept]) {
    for k in kept.iter_mut().filter(|k| k.pending) {
        k.pending = false;
        k.written = true;
    }
}
/// Map grids kept: a Fit, its draft and a 100% region share one per map base.
const GRIDS: usize = 2;
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
        let entries: Vec<_> = (0..9)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 5 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: !matches!(binding, 2 | 8),
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
        Self {
            layout,
            pipelines: Vec::new(),
            pipeline_layout,
            inputs: Default::default(),
            grids: Vec::new(),
            samples: Vec::new(),
        }
    }
    /// Input `slot` (parameters, tables, weights, an empty grid or kept colour), at
    /// least 16 bytes.
    fn input_buffer(&mut self, device: &wgpu::Device, slot: usize) -> wgpu::Buffer {
        self.sized(device, slot, 16)
    }
    /// Input `slot` with room for `size` bytes, grown when too small.
    fn sized(&mut self, device: &wgpu::Device, slot: usize, size: u64) -> wgpu::Buffer {
        let kept = &mut self.inputs[slot];
        if kept.as_ref().is_none_or(|b| b.size() < size) {
            *kept = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(
                    [
                        "Develop parameters",
                        "Develop tables",
                        "Mask weights",
                        "No map",
                        "No kept colour",
                    ][slot],
                ),
                size: size.next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        kept.clone().expect("created above")
    }
    /// Input `slot` (parameters, tables, weights) holding `data`: grown when too small,
    /// and written through `queue`, which copies it before the passes of the next
    /// submission and after those of earlier ones. Each develop pass is submitted
    /// before the next is recorded.
    fn input(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot: usize,
        data: &[u8],
    ) -> wgpu::Buffer {
        let size = (data.len() as u64).next_multiple_of(4).max(16);
        let buffer = self.sized(device, slot, size);
        if !data.is_empty() {
            queue.write_buffer(&buffer, 0, data);
        }
        buffer
    }
    /// The device's copy of `grid`, uploaded unless kept; without one, an empty buffer.
    fn grid(
        &mut self,
        device: &wgpu::Device,
        grid: Option<&crate::develop::local_tone::MapGrid>,
    ) -> wgpu::Buffer {
        let Some(grid) = grid else {
            return self.input_buffer(device, 3);
        };
        let buffer = match self.grids.iter().position(|(g, _)| g.same(grid)) {
            Some(i) => self.grids.remove(i).1,
            None => device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Shadows/Highlights map"),
                contents: bytemuck::cast_slice(&grid.values()),
                usage: wgpu::BufferUsages::STORAGE,
            }),
        };
        self.grids.insert(0, (grid.clone(), buffer.clone()));
        self.grids.truncate(GRIDS);
        buffer
    }
    /// The develop pipeline for `variant`, compiled on first use.
    fn pipeline(&mut self, device: &wgpu::Device, variant: Variant) -> wgpu::ComputePipeline {
        if let Some((_, p)) = self.pipelines.iter().find(|(v, _)| *v == variant) {
            return p.clone();
        }
        let source = crate::develop::pipeline::pixel_params::wgsl_prelude()
            + &variant.constants()
            + include_str!("develop.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Develop"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Develop"),
            layout: Some(&self.pipeline_layout),
            module: &shader,
            entry_point: Some("develop"),
            compilation_options: Default::default(),
            cache: None,
        });
        self.pipelines.push((variant, pipeline.clone()));
        pipeline
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
        // A pass recorded before this one and still pending was never submitted (a
        // cancelled render): it wrote nothing, so submitting this one does not mark it.
        for k in &mut self.kept {
            k.pending = false;
        }
        // 1 reads the kept camera colour, 2 computes and keeps it, 0 computes it.
        let (mode, color) = match &params.color {
            Some(recipe) if params.weights.is_empty() => {
                let bytes = n * 12;
                // The entry for these samples, most recently used first.
                let entry = self
                    .kept
                    .iter()
                    .position(|k| k.pixels == pixels && k.positions == positions)
                    .map(|i| self.kept.remove(i))
                    .filter(|k| k.color.size() >= bytes);
                let step = Keep::step(entry.as_ref().map(|k| (k.recipe == *recipe, k.written)));
                let color = match entry {
                    Some(k) => k.color,
                    None => device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Kept camera colour"),
                        size: bytes,
                        usage: wgpu::BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    }),
                };
                self.kept.insert(
                    0,
                    Kept {
                        pixels: pixels.clone(),
                        positions: positions.clone(),
                        recipe: recipe.clone(),
                        color: color.clone(),
                        written: step == Keep::Read,
                        pending: step == Keep::Write,
                    },
                );
                self.kept.truncate(KEPT);
                (step as u32, Some(color))
            }
            _ => (0, None),
        };
        let color = color.unwrap_or_else(|| developer.input_buffer(device, 4));
        let pipeline = developer.pipeline(device, Variant::of(params));
        let queue = &self.queue;
        let params_buffer = developer.input(device, queue, 0, bytemuck::cast_slice(&params.params));
        let tables = developer.input(device, queue, 1, bytemuck::cast_slice(&params.tables));
        let weights = developer.input(device, queue, 2, bytemuck::cast_slice(&params.weights));
        let grid = developer.grid(device, params.grid.as_ref());
        let groups = (n as u32).div_ceil(GROUP);
        let max = device.limits().max_compute_workgroups_per_dimension;
        let (gx, gy) = (groups.min(max), groups.div_ceil(groups.min(max)));
        let size = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Develop size"),
            contents: bytemuck::cast_slice(&[n as u32, gx, mode, 0]),
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
            &grid,
            &color,
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
            pass.set_pipeline(&pipeline);
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
        cancel: Cancel<'_>,
    ) -> Result<Rendered> {
        self.check_develop(Input::Cpu(samples), params, cancel)?;
        self.scoped(|gpu| gpu.develop_inner(samples, params, cancel))
    }
    /// Whether the device can develop `samples` at all.
    pub(super) fn check_develop(
        &self,
        input: Input,
        params: &PixelParams,
        cancel: Cancel<'_>,
    ) -> Result<()> {
        ensure!(!cancel.is_cancelled(), "Render superseded");
        let (w, h) = input.size();
        ensure!(w as u64 * h as u64 > 0, "Empty develop region");
        ensure!(
            self.fits_develop(w, h, params),
            "Region exceeds GPU buffer limits or memory budget"
        );
        Ok(())
    }
    /// Whether a `w` × `h` region with `params` is within the device's buffer limits
    /// and the develop memory budget. A region outside them renders on the CPU; it
    /// says nothing about the device, which stays in use for the next region.
    pub(crate) fn fits_develop(&self, w: u32, h: u32, params: &PixelParams) -> bool {
        let n = w as u64 * h as u64;
        let limits = self.device.limits();
        n * 12 <= limits.max_storage_buffer_binding_size
            && n * 12 <= limits.max_buffer_size
            && (params.tables.len() as u64 * 4) <= limits.max_storage_buffer_binding_size
            && n * 44 <= 1024 * 1024 * 1024
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
            self.kept.clear();
            self.release_resident();
            anyhow::bail!("{error}");
        }
        result
    }
    fn develop_inner(
        &mut self,
        samples: &Arc<Samples>,
        params: &PixelParams,
        cancel: Cancel<'_>,
    ) -> Result<Rendered> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.record_develop(Input::Cpu(samples), params, &mut encoder);
        ensure!(!cancel.is_cancelled(), "Render superseded");
        let device = &self.device;
        let n = samples.pixels.len() as u64;
        let uploaded = &self.developer.as_ref().unwrap().samples[0];
        encoder.copy_buffer_to_buffer(&uploaded.output, 0, &uploaded.staging, 0, n * 12);
        let submission = super::submit(&self.queue, encoder);
        submitted(&mut self.kept);
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
        if cancel.is_cancelled() {
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

#[cfg(test)]
mod tests {
    use super::{Keep, Variant};
    use crate::develop::pipeline::pixel_params::{PixelParams, slot};
    use std::ops::Range;

    /// A render reads the kept camera colour only after a submitted pass wrote it for
    /// the same recipe; a pass recorded but never submitted (a cancelled render) leaves
    /// it to be written again.
    #[test]
    fn the_kept_colour_is_read_only_once_a_submitted_pass_wrote_it() {
        // (same recipe as the entry, written): new samples, a new white balance, the
        // second render of a recipe, then the third.
        assert_eq!(Keep::step(None), Keep::Run);
        assert_eq!(Keep::step(Some((false, true))), Keep::Run);
        assert_eq!(Keep::step(Some((true, false))), Keep::Write);
        assert_eq!(Keep::step(Some((true, true))), Keep::Read);
    }

    /// Each colour stage on its own compiles the colour code into the pass, a pass
    /// without any leaves it out, and masks bring Dehaze with them.
    #[test]
    fn each_stage_selects_its_variant() {
        let off = [
            (slot::MIXER, &[-1.][..]),
            (slot::POINT, &[-1., 0.]),
            (slot::RGB, &[-1., 0., 0., 0., 0., 1.]),
            (slot::GRADE, &[-1., 0.]),
            (slot::ADJUST, &[0.]),
        ];
        let variant = |on: &[(Range<usize>, &[f32])], masks: bool| {
            let mut values = off.to_vec();
            values.extend_from_slice(on);
            let mut p = PixelParams::with(&values);
            if masks {
                p.weights = vec![1];
            }
            Variant::of(&p)
        };
        let none = variant(&[], false);
        assert!(!none.color && !none.local && !none.dehaze && !none.masks);
        // A look's RGB table at zero amount changes nothing.
        assert!(!variant(&[(slot::RGB, &[0., 0., 0., 0., 0., 0.])], false).color);
        for on in [
            (slot::MIXER, &[0.][..]),
            (slot::POINT, &[0., 0.]),
            (slot::RGB, &[0., 0., 0., 0., 0., 1.]),
            (slot::GRADE, &[0., 0.]),
            (slot::ADJUST, &[1.]),
        ] {
            assert!(
                variant(std::slice::from_ref(&on), false).color,
                "{:?}",
                on.0
            );
        }
        assert!(variant(&[(slot::LOCAL, &[1.])], false).local);
        assert!(variant(&[(slot::DEHAZE, &[0.3, 0.])], false).dehaze);
        let masked = variant(&[], true);
        assert!(masked.masks && masked.dehaze);
    }
}
