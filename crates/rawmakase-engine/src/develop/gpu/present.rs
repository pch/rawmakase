//! Preview display on the device: the developed pixels are finished (`present.wgsl`)
//! straight into a texture the viewport draws, so a preview is never read back or
//! uploaded again. Only the histogram and, when asked, a small thumbnail or the
//! shown pixels for the white balance loupe come back.
use super::{Processor, develop::Input, uniforms::PresentParams};
use crate::develop::{
    effects::{GrainField, PostCropVignette},
    pipeline::pixel_params::PixelParams,
    quality,
};
use crate::model::recipe::Recipe;
use crate::rendered::{ClipOverlay, Histogram};
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

/// The histogram buffer: 256 bins per channel, then `Clipped`'s highlight and
/// shadow counts per channel.
const HISTOGRAM_WORDS: u64 = 768 + 6;

/// The view a presented image is for; each keeps its own textures, so drawing one
/// never shows another's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// The whole photo: Fit and zoomed-out views.
    Whole,
    /// A 100% region and its reduced preview.
    Region,
    /// Before's whole photo and region beside the edit, in Before/After views: kept
    /// apart so neither side is presented into a texture the other still shows.
    BeforeWhole,
    BeforeRegion,
}
/// How a preview is shown.
pub struct Display {
    pub slot: Slot,
    /// Red and blue overlays for clipped highlights and shadows.
    pub clipping: ClipOverlay,
    pub monitor: Option<Arc<MonitorLut>>,
    /// Long edge of a reduced copy for the Navigator.
    pub navigator: Option<u32>,
    /// Long edge of a reduced copy read back for library thumbnails.
    pub thumbnail: Option<u32>,
    /// Read back the shown pixels without overlays or monitor profile, for the
    /// white balance selector's loupe.
    pub samples: bool,
    /// Textures the viewport is drawing until this preview arrives. They are never
    /// written: panning at 100% renders a region of the same size, and writing it
    /// into the drawn texture would show the new pixels at the old position.
    pub drawn: Vec<wgpu::Texture>,
}
/// A presented preview.
pub struct Frame {
    #[cfg(test)]
    pub width: u32,
    #[cfg(test)]
    pub height: u32,
    /// Rgba8Unorm, holding the 8-bit display values.
    pub texture: wgpu::Texture,
    /// Changes whenever the texture is written, see [`Processor::generation`].
    pub generation: u64,
    pub navigator: Option<wgpu::Texture>,
    /// As [`crate::rendered::Rendered::histogram`] of the finished pixels.
    pub histogram: Box<Histogram>,
    /// Width, height and RGB bytes of the reduced copy for thumbnails.
    pub thumbnail: Option<(u32, u32, Vec<u8>)>,
    /// Width, height and RGB bytes of the shown pixels, when `Display::samples`.
    pub samples: Option<(u32, u32, Vec<u8>)>,
    /// Textures dropped since the previous frame; nothing draws them any more.
    pub released: Vec<wgpu::Texture>,
}
/// The sRGB-to-monitor transform sampled on a lattice of 8-bit values, so the shader
/// applies it without lcms. Built once per profile, from the transform the caller
/// supplies (`raw::display_transform`), so the engine links no colour management.
pub struct MonitorLut {
    size: u32,
    values: Vec<f32>,
}
/// Lattice points per axis: 52 puts one on every fifth 8-bit value.
const LUT_SIZE: u32 = 52;
impl MonitorLut {
    /// Samples `transform`, which changes sRGB triples in place.
    pub fn sample(transform: impl FnOnce(&mut [u8]) -> Result<()>) -> Result<Self> {
        let n = LUT_SIZE as usize;
        let step = |i: usize| (i * 255 / (n - 1)) as u8;
        let mut rgb: Vec<u8> = (0..n * n * n)
            .flat_map(|i| [step(i / (n * n)), step(i / n % n), step(i % n)])
            .collect();
        transform(&mut rgb)?;
        Ok(Self {
            size: LUT_SIZE,
            values: rgb.into_iter().map(f32::from).collect(),
        })
    }
}
/// Finishing of a developed region: sharpening with `sigma`, then spatial effects at
/// `scale` output pixels per full-resolution pixel for the region's `origin` in an
/// output of `full` pixels, showing `crop` of the region.
pub(crate) struct Finish {
    pub(crate) sigma: f32,
    pub(crate) origin: [u32; 2],
    pub(crate) full: [u32; 2],
    pub(crate) scale: f32,
    pub(crate) crop: [u32; 4],
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Photo(Slot),
    Navigator,
    /// Internal: the photo without overlays, for thumbnails and loupe samples.
    Plain,
    Thumbnail,
}
struct Target {
    kind: Kind,
    texture: wgpu::Texture,
    generation: u64,
}
/// Textures kept per kind besides drawn ones: a view alternates between two sizes
/// while a slider moves at 100% (reduced preview, full region).
const TARGETS: usize = 2;

pub(super) struct Presenter {
    layout: wgpu::BindGroupLayout,
    reduce_layout: wgpu::BindGroupLayout,
    /// blur_horizontal, sharpen, present.
    pipelines: [wgpu::ComputePipeline; 3],
    reduce: wgpu::ComputePipeline,
    histogram: wgpu::Buffer,
    histogram_staging: wgpu::Buffer,
    scratch: Option<wgpu::Buffer>,
    lut: Option<(Arc<MonitorLut>, wgpu::Buffer)>,
    no_lut: wgpu::Buffer,
    targets: Vec<Target>,
    released: Vec<wgpu::Texture>,
    generation: u64,
}
impl Presenter {
    fn new(device: &wgpu::Device) -> Self {
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
        let write_texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba8Unorm,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Present bindings"),
            entries: &[
                storage(0, false),
                storage(1, false),
                storage(2, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(4, true),
                storage(5, false),
                write_texture(6),
            ],
        });
        let reduce_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Reduce bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                write_texture(1),
            ],
        });
        let pipeline = |source: &str, layout: &wgpu::BindGroupLayout, entry: &str| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(entry),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let present = PostCropVignette::wgsl_tone() + include_str!("present.wgsl");
        let histogram = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: HISTOGRAM_WORDS * 4,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            pipelines: ["blur_horizontal", "sharpen", "present"]
                .map(|entry| pipeline(&present, &layout, entry)),
            reduce: pipeline(include_str!("reduce.wgsl"), &reduce_layout, "reduce"),
            layout,
            reduce_layout,
            histogram: histogram(
                "Histogram",
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
            histogram_staging: histogram(
                "Histogram readback",
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
            scratch: None,
            lut: None,
            no_lut: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("No monitor profile"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            targets: Vec::new(),
            released: Vec::new(),
            generation: 0,
        }
    }
    /// A texture of `kind` and size, reusing one when the size is unchanged and it is
    /// not `drawn`.
    fn target(
        &mut self,
        device: &wgpu::Device,
        kind: Kind,
        width: u32,
        height: u32,
        drawn: &[wgpu::Texture],
    ) -> &mut Target {
        if let Some(i) = self.targets.iter().position(|t| {
            t.kind == kind
                && t.texture.width() == width
                && t.texture.height() == height
                && !drawn.contains(&t.texture)
        }) {
            let target = self.targets.remove(i);
            self.targets.insert(0, target);
        } else {
            let mut usage = wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC;
            let internal = matches!(kind, Kind::Plain | Kind::Thumbnail);
            if kind == Kind::Thumbnail {
                usage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC;
            }
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Preview"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage,
                view_formats: &[],
            });
            self.targets.insert(
                0,
                Target {
                    kind,
                    texture,
                    generation: 0,
                },
            );
            let keep = if internal { 1 } else { TARGETS };
            let same: Vec<usize> = (0..self.targets.len())
                .filter(|i| {
                    self.targets[*i].kind == kind && !drawn.contains(&self.targets[*i].texture)
                })
                .collect();
            for &i in same.iter().skip(keep).rev() {
                let old = self.targets.remove(i);
                if !internal {
                    self.released.push(old.texture);
                }
            }
        }
        self.generation += 1;
        let target = &mut self.targets[0];
        target.generation = self.generation;
        target
    }
}

impl Processor {
    /// Develops `samples` and finishes them into a texture for `display`, reading back
    /// only the histogram and the thumbnail. Fails without side effects on the CPU.
    pub(crate) fn present(
        &mut self,
        input: Input,
        params: &PixelParams,
        recipe: &Recipe,
        finish: &Finish,
        display: &Display,
        cancel: &AtomicBool,
    ) -> Result<Frame> {
        self.check_develop(input, params, cancel)?;
        let (width, height) = input.size();
        let [x, y, w, h] = finish.crop;
        ensure!(
            w > 0
                && h > 0
                && x + w <= width
                && y + h <= height
                && finish.full[0] > 0
                && finish.full[1] > 0,
            "Invalid preview crop"
        );
        let limits = self.device.limits();
        ensure!(
            w.max(h) <= limits.max_texture_dimension_2d
                && w.div_ceil(16).max(h.div_ceil(16))
                    <= limits.max_compute_workgroups_per_dimension
                && width.div_ceil(16).max(height.div_ceil(16))
                    <= limits.max_compute_workgroups_per_dimension,
            "Preview exceeds GPU texture limits"
        );
        self.scoped(|gpu| gpu.present_inner(input, params, recipe, finish, display, cancel))
    }
    /// The generation of `texture` if it is still a preview target, to tell whether a
    /// frame's pixels are still in it.
    pub fn generation(&self, texture: &wgpu::Texture) -> Option<u64> {
        let presenter = self.presenter.as_ref()?;
        presenter
            .targets
            .iter()
            .find(|t| t.texture == *texture)
            .map(|t| t.generation)
    }
    fn present_inner(
        &mut self,
        input: Input,
        params: &PixelParams,
        recipe: &Recipe,
        finish: &Finish,
        display: &Display,
        cancel: &AtomicBool,
    ) -> Result<Frame> {
        let device = self.device.clone();
        let mut encoder = device.create_command_encoder(&Default::default());
        let pixels = self.record_develop(input, params, &mut encoder);
        let presenter = self
            .presenter
            .get_or_insert_with(|| Presenter::new(&device));
        let (width, height) = input.size();
        let n = width as u64 * height as u64;
        if presenter.scratch.as_ref().is_none_or(|b| b.size() < n * 4) {
            presenter.scratch = None;
            presenter.scratch = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Sharpening blur"),
                size: n * 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }));
        }
        let sharpen = recipe.sharpening != 0.;
        let sharpener = crate::develop::sharpening::Sharpener::new(recipe);
        let (radius, weights) = if sharpen {
            quality::gaussian(finish.sigma)
        } else {
            (0, vec![1.])
        };
        let weights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sharpening weights"),
            contents: bytemuck::cast_slice(&weights),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let lut = match &display.monitor {
            Some(lut) => {
                if !presenter
                    .lut
                    .as_ref()
                    .is_some_and(|l| Arc::ptr_eq(&l.0, lut))
                {
                    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Monitor profile"),
                        contents: bytemuck::cast_slice(&lut.values),
                        usage: wgpu::BufferUsages::STORAGE,
                    });
                    presenter.lut = Some((lut.clone(), buffer));
                }
                presenter.lut.as_ref().map(|l| l.1.clone())
            }
            None => None,
        };
        let e = &recipe.effects;
        let effects = e.grain != 0. || e.vignette != 0.;
        let [cx, cy, cw, ch] = finish.crop;
        let vignette = PostCropVignette::new(e, finish.full);
        let grain = GrainField::new(e, finish.full[0].max(finish.full[1]) as f32 / finish.scale);
        let parameters = |shown: bool| -> wgpu::Buffer {
            let values = PresentParams {
                width,
                height,
                crop_x: cx,
                crop_y: cy,
                crop_w: cw,
                crop_h: ch,
                radius: radius as u32,
                sharpen: sharpen as u32,
                amount: recipe.sharpening * sharpener.gain,
                threshold: sharpener.threshold,
                clipping: if shown {
                    display.clipping.shader_flags()
                } else {
                    0
                },
                lut_size: display
                    .monitor
                    .as_ref()
                    .filter(|_| shown)
                    .map_or(0, |l| l.size),
                origin_x: finish.origin[0],
                origin_y: finish.origin[1],
                full_w: finish.full[0],
                full_h: finish.full[1],
                scale: finish.scale,
                grain: grain.amount,
                grain_cell: grain.cell,
                grain_coarse: grain.coarse,
                grain_seed: grain.seed,
                vignette: vignette.map_or(0., |v| v.amount),
                vignette_style: vignette.map_or(0, |v| v.style.code() as u32),
                vignette_highlights: vignette.map_or(0., |v| v.highlights),
                vignette_scale_x: vignette.map_or(1., |v| v.scale[0]),
                vignette_scale_y: vignette.map_or(1., |v| v.scale[1]),
                vignette_power: vignette.map_or(2., |v| v.power),
                vignette_midpoint: vignette.map_or(0., |v| v.midpoint),
                vignette_feather: vignette.map_or(1., |v| v.feather),
                effects: effects as u32,
                count: shown as u32,
                halo: sharpener.halo,
                dark: sharpener.dark,
                grain_fine: grain.fine,
                ..Default::default()
            };
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Present parameters"),
                contents: bytemuck::bytes_of(&values),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let lut = lut.unwrap_or_else(|| presenter.no_lut.clone());
        let scratch = presenter.scratch.clone().unwrap();
        let histogram = presenter.histogram.clone();
        let layout = presenter.layout.clone();
        let [blur, sharpening, present] = presenter.pipelines.clone();
        // Runs `pipelines` over `size` with `view` as the texture. Only the shown image
        // counts towards the histogram and carries the overlay and monitor profile.
        let record = |encoder: &mut wgpu::CommandEncoder,
                      view: &wgpu::TextureView,
                      shown: bool,
                      pipelines: &[&wgpu::ComputePipeline],
                      [x, y]: [u32; 2]| {
            let uniform = parameters(shown);
            let buffers = [&pixels, &scratch, &weights, &uniform, &lut, &histogram];
            let mut entries: Vec<_> = buffers
                .into_iter()
                .enumerate()
                .map(|(binding, buffer)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: buffer.as_entire_binding(),
                })
                .collect();
            entries.push(wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(view),
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Present bindings"),
                layout: &layout,
                entries: &entries,
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Present"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &group, &[]);
            for pipeline in pipelines {
                pass.set_pipeline(pipeline);
                pass.dispatch_workgroups(x.div_ceil(16), y.div_ceil(16), 1);
            }
        };
        let photo = presenter.target(&device, Kind::Photo(display.slot), cw, ch, &display.drawn);
        let (texture, generation) = (photo.texture.clone(), photo.generation);
        let view = texture.create_view(&Default::default());
        if sharpen {
            record(
                &mut encoder,
                &view,
                true,
                &[&blur, &sharpening],
                [width, height],
            );
        }
        // Library thumbnails and loupe samples show the photo without the overlay or
        // monitor profile.
        let overlays = display.clipping.any() || display.monitor.is_some();
        let plain = ((display.thumbnail.is_some() || display.samples) && overlays).then(|| {
            let plain = presenter
                .target(&device, Kind::Plain, cw, ch, &[])
                .texture
                .clone();
            let view = plain.create_view(&Default::default());
            record(&mut encoder, &view, false, &[&present], [cw, ch]);
            (plain, view)
        });
        encoder.clear_buffer(&presenter.histogram, 0, None);
        record(&mut encoder, &view, true, &[&present], [cw, ch]);
        let mut reduce = |kind: Kind,
                          source: &wgpu::TextureView,
                          edge: u32,
                          encoder: &mut wgpu::CommandEncoder| {
            let (rw, rh) = quality::output_size(cw, ch, edge.max(1));
            let target = presenter
                .target(&device, kind, rw, rh, &display.drawn)
                .texture
                .clone();
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Reduce bindings"),
                layout: &presenter.reduce_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &target.create_view(&Default::default()),
                        ),
                    },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Reduce"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&presenter.reduce);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(rw.div_ceil(16), rh.div_ceil(16), 1);
            target
        };
        let navigator = display
            .navigator
            .map(|edge| reduce(Kind::Navigator, &view, edge, &mut encoder));
        let thumbnail = display.thumbnail.map(|edge| {
            let source = plain.as_ref().map_or(&view, |(_, view)| view);
            let texture = reduce(Kind::Thumbnail, source, edge, &mut encoder);
            readback(&device, &mut encoder, &texture, "Thumbnail readback")
        });
        let samples = display.samples.then(|| {
            let source = plain.as_ref().map_or(&texture, |(texture, _)| texture);
            readback(&device, &mut encoder, source, "Loupe readback")
        });
        encoder.copy_buffer_to_buffer(
            &presenter.histogram,
            0,
            &presenter.histogram_staging,
            0,
            HISTOGRAM_WORDS * 4,
        );
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        let submission = super::submit(&self.queue, encoder);
        let (tx, rx) = mpsc::channel();
        let map = |buffer: &wgpu::Buffer| {
            let tx = tx.clone();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
        };
        map(&presenter.histogram_staging);
        let readbacks: Vec<_> = thumbnail.iter().chain(&samples).collect();
        for (.., buffer) in &readbacks {
            map(buffer);
        }
        drop(tx);
        let poll = super::wait(&device, submission);
        let mapped: Result<()> = poll.map_err(anyhow::Error::from).and_then(|_| {
            for _ in 0..1 + readbacks.len() {
                rx.recv_timeout(Duration::from_secs(1))
                    .context("GPU readback timed out")??;
            }
            Ok(())
        });
        let unmap = |presenter: &Presenter| {
            presenter.histogram_staging.unmap();
            for (.., buffer) in &readbacks {
                buffer.unmap();
            }
        };
        if let Err(error) = mapped {
            unmap(presenter);
            return Err(error);
        }
        let mut histogram = Box::new(Histogram::EMPTY);
        {
            let counts = presenter.histogram_staging.slice(..).get_mapped_range()?;
            let counts = bytemuck::cast_slice::<u8, u32>(&counts);
            for (c, channel) in counts[..768].as_chunks::<256>().0.iter().enumerate() {
                histogram.bins[c].copy_from_slice(channel);
            }
            histogram
                .clipped
                .highlights
                .copy_from_slice(&counts[768..771]);
            histogram.clipped.shadows.copy_from_slice(&counts[771..774]);
        }
        let rgb = |(tw, th, row, buffer): &(u32, u32, u32, wgpu::Buffer)| -> Result<_> {
            let bytes = buffer.slice(..).get_mapped_range()?;
            let rgb = bytes
                .chunks_exact(*row as usize)
                .flat_map(|r| r[..*tw as usize * 4].as_chunks::<4>().0.iter())
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            Ok((*tw, *th, rgb))
        };
        let small = thumbnail.as_ref().map(rgb).transpose();
        let shown = samples.as_ref().map(rgb).transpose();
        unmap(presenter);
        let (small, shown) = (small?, shown?);
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        Ok(Frame {
            #[cfg(test)]
            width: cw,
            #[cfg(test)]
            height: ch,
            texture,
            generation,
            navigator,
            histogram,
            thumbnail: small,
            samples: shown,
            released: std::mem::take(&mut presenter.released),
        })
    }
}
/// Records a copy of `texture` into a mappable buffer: width, height, padded row
/// bytes and the buffer.
fn readback(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    label: &str,
) -> (u32, u32, u32, wgpu::Buffer) {
    let row = (texture.width() * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: row as u64 * texture.height() as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    (texture.width(), texture.height(), row, buffer)
}

#[cfg(test)]
mod monitor_tests {
    use super::*;

    #[test]
    fn the_monitor_lut_samples_the_transform_it_is_given() -> Result<()> {
        let n = LUT_SIZE as usize;
        let identity = MonitorLut::sample(|_| Ok(()))?;
        assert_eq!(identity.size, LUT_SIZE);
        assert_eq!(identity.values.len(), n * n * n * 3);
        // Red runs slowest, blue fastest, on every fifth 8-bit value.
        assert_eq!(identity.values[..6], [0., 0., 0., 0., 0., 5.]);
        assert_eq!(identity.values[identity.values.len() - 3..], [255.; 3]);
        let inverted = MonitorLut::sample(|rgb| {
            rgb.iter_mut().for_each(|v| *v = 255 - *v);
            Ok(())
        })?;
        assert_eq!(inverted.values[..3], [255.; 3]);
        assert!(MonitorLut::sample(|_| anyhow::bail!("no profile")).is_err());
        Ok(())
    }
}
