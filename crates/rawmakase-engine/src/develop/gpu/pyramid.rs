//! Shared GPU Laplacian pyramid compute runner for Color Noise Reduction,
//! Luminance Noise Reduction, Texture, and measurement box reduction.
//!
//! Currently wired into headless exports ([`super::export::ExportBackend`]) and
//! decoupled from submission/polling via [`PyramidHost`] so interactive previews
//! ([`super::Processor`]) can reuse the exact same kernels and CPU quantile logic.
//!
//! Temporary pyramid working buffers live only for the duration of each stage call
//! and are dropped before downstream banded develop/finish dispatches begin (cached
//! band buffers from a previous photo are released on idle or OOM retry).

use super::uniforms::PyramidParams;
use crate::develop::{
    color_noise::{self, ChromaDenoise},
    laplacian,
    luminance_noise::{self, LuminanceDenoise},
    texture,
};
use rawmakase_cancel::Cancel;
use std::{sync::mpsc, time::Duration};
use wgpu::util::DeviceExt;

const PIPELINE_NAMES: [&str; 24] = [
    "chroma_extract",
    "down_vec2_h",
    "down_vec2_v",
    "up_vec2_h",
    "up_vec2_v_sub",
    "chroma_block_stats_l0",
    "chroma_block_stats",
    "mask_down_l0",
    "mask_down",
    "up_vec2_v_reconstruct_chroma",
    "chroma_writeback",
    "luma_extract",
    "down_f32_h",
    "down_f32_v",
    "up_f32_h",
    "up_f32_v_sub",
    "luma_block_stats",
    "up_f32_v_reconstruct_luma",
    "luma_writeback",
    "texture_extract",
    "up_f32_v_compress_texture",
    "up_f32_v_add_inplace",
    "texture_apply_channel",
    "box_reduce",
];

const P_CHROMA_EXTRACT: usize = 0;
const P_DOWN_VEC2_H: usize = 1;
const P_DOWN_VEC2_V: usize = 2;
const P_UP_VEC2_H: usize = 3;
const P_UP_VEC2_V_SUB: usize = 4;
const P_CHROMA_STATS_L0: usize = 5;
const P_CHROMA_STATS: usize = 6;
const P_MASK_DOWN_L0: usize = 7;
const P_MASK_DOWN: usize = 8;
const P_UP_VEC2_RECON: usize = 9;
const P_CHROMA_WRITEBACK: usize = 10;
const P_LUMA_EXTRACT: usize = 11;
const P_DOWN_F32_H: usize = 12;
const P_DOWN_F32_V: usize = 13;
const P_UP_F32_H: usize = 14;
const P_UP_F32_V_SUB: usize = 15;
const P_LUMA_STATS: usize = 16;
const P_UP_F32_RECON_LUMA: usize = 17;
const P_LUMA_WRITEBACK: usize = 18;
const P_TEXTURE_EXTRACT: usize = 19;
const P_UP_F32_COMPRESS_TEX: usize = 20;
const P_UP_F32_ADD_INPLACE: usize = 21;
const P_TEXTURE_APPLY: usize = 22;
const P_BOX_REDUCE: usize = 23;

/// Host abstraction decoupling [`GpuPyramid`] from headless export vs. UI preview submission.
pub(super) trait PyramidHost {
    type Error;
    fn device(&self) -> &wgpu::Device;
    fn submit(&self, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex;
    fn wait(
        &self,
        submission: wgpu::SubmissionIndex,
        cancel: Cancel<'_>,
    ) -> std::result::Result<(), Self::Error>;
    fn cancelled(&self) -> Self::Error;
    fn readback_error(&self, message: String) -> Self::Error;
    fn exceeds_limits(&self, message: String) -> Self::Error;
}

/// Compiled compute pipelines for the shared GPU Laplacian pyramid passes.
pub(super) struct GpuPyramid {
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::ComputePipeline; 24],
    dummy: wgpu::Buffer,
}

impl GpuPyramid {
    /// Compiles all pyramid compute pipelines on `device`.
    pub(super) fn new(device: &wgpu::Device) -> Self {
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
            label: Some("Pyramid bindings"),
            entries: &[
                storage(0, true),
                storage(1, true),
                storage(2, false),
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
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pyramid.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pyramid.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pyramid layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipelines = PIPELINE_NAMES.map(|entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        let dummy = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Pyramid dummy"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        Self {
            layout,
            pipelines,
            dummy,
        }
    }

    fn alloc_storage(
        device: &wgpu::Device,
        label: &str,
        bytes: u64,
        copy_src: bool,
    ) -> wgpu::Buffer {
        let mut usage = wgpu::BufferUsages::STORAGE;
        if copy_src {
            usage |= wgpu::BufferUsages::COPY_SRC;
        }
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.next_multiple_of(4).max(16),
            usage,
            mapped_at_creation: false,
        })
    }

    fn check_limits<H: PyramidHost>(
        host: &H,
        max_buffer_bytes: u64,
        w: u32,
        h: u32,
    ) -> std::result::Result<(), H::Error> {
        let limits = host.device().limits();
        let max_binding = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let max_wg = limits.max_compute_workgroups_per_dimension;
        if max_buffer_bytes > max_binding || w.div_ceil(16) > max_wg || h.div_ceil(16) > max_wg {
            return Err(
                host.exceeds_limits("Pyramid working buffer or dispatch exceeds GPU limits".into())
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: usize,
        src_a: &wgpu::Buffer,
        src_b: &wgpu::Buffer,
        dst: &wgpu::Buffer,
        uniform: PyramidParams,
    ) {
        if uniform.dst_w == 0 || uniform.dst_h == 0 {
            return;
        }
        let u_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Pyramid uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Pyramid pass bindings"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: src_a.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: src_b.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: dst.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: u_buf.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(PIPELINE_NAMES[pipeline]),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipelines[pipeline]);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(uniform.dst_w.div_ceil(16), uniform.dst_h.div_ceil(16), 1);
    }

    /// Copies `bytes` from `src` to a fresh staging buffer, submits `encoder`, waits with
    /// cancellation support, and returns the readback `f32` vector.
    fn submit_and_readback_f32<H: PyramidHost>(
        host: &H,
        mut encoder: wgpu::CommandEncoder,
        src: &wgpu::Buffer,
        float_count: usize,
        cancel: Cancel<'_>,
    ) -> std::result::Result<Vec<f32>, H::Error> {
        if cancel.is_cancelled() {
            return Err(host.cancelled());
        }
        let byte_len = (float_count as u64 * 4).max(4);
        let staging = host.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("Pyramid readback staging"),
            size: byte_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(src, 0, &staging, 0, byte_len);
        let submission = host.submit(encoder);

        let (tx, rx) = mpsc::sync_channel(1);
        let slice = staging.slice(..byte_len);
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        if let Err(err) = host.wait(submission, cancel) {
            staging.unmap();
            return Err(err);
        }
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                staging.unmap();
                return Err(host.readback_error(format!("Pyramid readback map failed: {e}")));
            }
            Err(_) => {
                staging.unmap();
                return Err(host.readback_error("Pyramid readback timed out".into()));
            }
        }
        if cancel.is_cancelled() {
            staging.unmap();
            return Err(host.cancelled());
        }
        let data = {
            let mapped = slice
                .get_mapped_range()
                .map_err(|e| host.readback_error(format!("Pyramid mapped range failed: {e}")))?;
            bytemuck::cast_slice::<u8, f32>(&mapped[..float_count * 4]).to_vec()
        };
        staging.unmap();
        Ok(data)
    }

    /// Runs [`ChromaDenoise`] in-place on the GPU camera RGB buffer `photo` (`width` × `height`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_chroma_denoise<H: PyramidHost>(
        &self,
        host: &H,
        photo: &wgpu::Buffer,
        width: u32,
        height: u32,
        wb: [f32; 3],
        photo_scale: f32,
        denoise: ChromaDenoise,
        cancel: Cancel<'_>,
    ) -> std::result::Result<(), H::Error> {
        if cancel.is_cancelled() {
            return Err(host.cancelled());
        }
        let offset = -photo_scale.log2();
        let mut levels: Vec<(u32, u32, f32, f32)> = Vec::new();
        let (mut w, mut h) = (width, height);
        for level in 0..=color_noise::LEVELS {
            let photo_level = level as f32 + offset;
            let share = laplacian::filtered_share(photo_level, color_noise::LEVELS + 1);
            if share <= 0. || w < 4 || h < 4 {
                break;
            }
            levels.push((w, h, photo_level, share));
            w = w.div_ceil(2);
            h = h.div_ceil(2);
        }
        if levels.is_empty() {
            return Ok(());
        }
        let residual_size = (w, h);

        let n0 = u64::from(width) * u64::from(height);
        Self::check_limits(host, n0 * 8, width, height)?;

        let device = host.device();
        let wb_safe = wb.map(|v| v.max(0.001));

        // Allocate chroma pyramid planes (`levels.len() + 1`, last is unfiltered residual).
        let mut planes: Vec<wgpu::Buffer> = Vec::with_capacity(levels.len() + 1);
        for &(lw, lh, _, _) in &levels {
            planes.push(Self::alloc_storage(
                device,
                "Chroma pyramid plane",
                u64::from(lw) * u64::from(lh) * 8,
                false,
            ));
        }
        planes.push(Self::alloc_storage(
            device,
            "Chroma pyramid residual",
            u64::from(residual_size.0) * u64::from(residual_size.1) * 8,
            false,
        ));

        // Scratch buffer holds max(ceil(w/2) * h, w * ceil(h/2)) vec2 floats.
        let scratch_px = (u64::from(width.div_ceil(2)) * u64::from(height))
            .max(u64::from(width) * u64::from(height.div_ceil(2)));
        let scratch = Self::alloc_storage(device, "Chroma pyramid scratch", scratch_px * 8, false);

        // Mask ping-pong buffers start at level 1 (`ceil(w/2) x ceil(h/2)`), since level 0
        // reads `photo` directly without allocating a full-res mask buffer.
        let mask_a = if levels.len() > 1 {
            let (w1, h1) = (width.div_ceil(2), height.div_ceil(2));
            Some(Self::alloc_storage(
                device,
                "Chroma mask A",
                u64::from(w1) * u64::from(h1) * 4,
                false,
            ))
        } else {
            None
        };
        let mask_b = if levels.len() > 2 {
            let (w2, h2) = (
                width.div_ceil(2).div_ceil(2),
                height.div_ceil(2).div_ceil(2),
            );
            Some(Self::alloc_storage(
                device,
                "Chroma mask B",
                u64::from(w2) * u64::from(h2) * 4,
                false,
            ))
        } else {
            None
        };

        // Pack all levels' 8x8 block stats into one contiguous buffer for a single readback.
        let mut stats_offsets: Vec<(usize, u32, u32)> = Vec::with_capacity(levels.len());
        let mut total_stat_floats: usize = 0;
        for &(lw, lh, _, _) in &levels {
            let bw = lw / color_noise::BLOCK as u32;
            let bh = lh / color_noise::BLOCK as u32;
            stats_offsets.push((total_stat_floats, bw, bh));
            total_stat_floats += (bw as usize) * (bh as usize) * 2;
        }
        let stats_buf = Self::alloc_storage(
            device,
            "Chroma block stats",
            (total_stat_floats as u64 * 4).max(16),
            true,
        );

        // --- Pass 1: Extract + full top-down pyramid + mask down + block stats ---
        let mut encoder = device.create_command_encoder(&Default::default());
        self.dispatch(
            device,
            &mut encoder,
            P_CHROMA_EXTRACT,
            photo,
            &self.dummy,
            &planes[0],
            PyramidParams {
                src_w: width,
                src_h: height,
                dst_w: width,
                dst_h: height,
                ..Default::default()
            },
        );

        for (l, &(lw, lh, _, _)) in levels.iter().enumerate() {
            let (cw, ch) = (lw.div_ceil(2), lh.div_ceil(2));
            // Downsample chroma[l] -> chroma[l + 1]
            self.dispatch(
                device,
                &mut encoder,
                P_DOWN_VEC2_H,
                &planes[l],
                &self.dummy,
                &scratch,
                PyramidParams {
                    src_w: lw,
                    src_h: lh,
                    dst_w: cw,
                    dst_h: lh,
                    ..Default::default()
                },
            );
            self.dispatch(
                device,
                &mut encoder,
                P_DOWN_VEC2_V,
                &scratch,
                &self.dummy,
                &planes[l + 1],
                PyramidParams {
                    src_w: cw,
                    src_h: lh,
                    dst_w: cw,
                    dst_h: ch,
                    ..Default::default()
                },
            );
            // Expand chroma[l + 1] and subtract in-place from chroma[l] -> detail[l]
            self.dispatch(
                device,
                &mut encoder,
                P_UP_VEC2_H,
                &planes[l + 1],
                &self.dummy,
                &scratch,
                PyramidParams {
                    src_w: cw,
                    src_h: ch,
                    dst_w: lw,
                    dst_h: ch,
                    ..Default::default()
                },
            );
            self.dispatch(
                device,
                &mut encoder,
                P_UP_VEC2_V_SUB,
                &scratch,
                &self.dummy,
                &planes[l],
                PyramidParams {
                    src_w: lw,
                    src_h: ch,
                    dst_w: lw,
                    dst_h: lh,
                    ..Default::default()
                },
            );

            // Compute 8x8 block stats for level `l`
            let (stat_off, bw, bh) = stats_offsets[l];
            if bw > 0 && bh > 0 {
                if l == 0 {
                    self.dispatch(
                        device,
                        &mut encoder,
                        P_CHROMA_STATS_L0,
                        &planes[0],
                        photo,
                        &stats_buf,
                        PyramidParams {
                            src_w: lw,
                            src_h: lh,
                            dst_w: bw,
                            dst_h: bh,
                            aux0: stat_off as u32,
                            f0: wb_safe[0],
                            f1: wb_safe[1],
                            f2: wb_safe[2],
                            f3: color_noise::MEASURABLE_PEAK,
                            ..Default::default()
                        },
                    );
                } else {
                    let cur_mask = if l % 2 == 1 {
                        mask_a.as_ref().expect("mask_a allocated")
                    } else {
                        mask_b.as_ref().expect("mask_b allocated")
                    };
                    self.dispatch(
                        device,
                        &mut encoder,
                        P_CHROMA_STATS,
                        &planes[l],
                        cur_mask,
                        &stats_buf,
                        PyramidParams {
                            src_w: lw,
                            src_h: lh,
                            dst_w: bw,
                            dst_h: bh,
                            aux0: stat_off as u32,
                            ..Default::default()
                        },
                    );
                }
            }

            // Downsample measurability mask for level `l + 1` if another level follows.
            if l + 1 < levels.len() {
                if l == 0 {
                    self.dispatch(
                        device,
                        &mut encoder,
                        P_MASK_DOWN_L0,
                        photo,
                        &self.dummy,
                        mask_a.as_ref().expect("mask_a allocated"),
                        PyramidParams {
                            src_w: lw,
                            src_h: lh,
                            dst_w: cw,
                            dst_h: ch,
                            f0: wb_safe[0],
                            f1: wb_safe[1],
                            f2: wb_safe[2],
                            f3: color_noise::MEASURABLE_PEAK,
                            ..Default::default()
                        },
                    );
                } else {
                    let (src_mask, dst_mask) = if l % 2 == 1 {
                        (
                            mask_a.as_ref().expect("mask_a"),
                            mask_b.as_ref().expect("mask_b"),
                        )
                    } else {
                        (
                            mask_b.as_ref().expect("mask_b"),
                            mask_a.as_ref().expect("mask_a"),
                        )
                    };
                    self.dispatch(
                        device,
                        &mut encoder,
                        P_MASK_DOWN,
                        src_mask,
                        &self.dummy,
                        dst_mask,
                        PyramidParams {
                            src_w: lw,
                            src_h: lh,
                            dst_w: cw,
                            dst_h: ch,
                            ..Default::default()
                        },
                    );
                }
            }
        }

        // Read back the tiny block-stats buffer (or submit without readback if 0 blocks).
        let raw_stats = if total_stat_floats > 0 {
            Self::submit_and_readback_f32(host, encoder, &stats_buf, total_stat_floats, cancel)?
        } else {
            let sub = host.submit(encoder);
            host.wait(sub, cancel)?;
            Vec::new()
        };
        drop(mask_a);
        drop(mask_b);
        drop(stats_buf);

        // --- CPU exact quantile & per-level thresholds ---
        let mut thresholds = vec![0f32; levels.len()];
        let mut finer_noise = 0f32;
        let mut finest_noise = 0f32;
        for (l, &(_, _, photo_level, _)) in levels.iter().enumerate() {
            let (off, bw, bh) = stats_offsets[l];
            let count = (bw as usize) * (bh as usize);
            let mut blocks = Vec::with_capacity(count);
            for b in 0..count {
                let energy = raw_stats[off + b * 2];
                if energy >= 0. {
                    blocks.push((energy, raw_stats[off + b * 2 + 1]));
                }
            }
            let noise =
                color_noise::chain_level_noise(l, color_noise::smoothest(blocks), finer_noise);
            if l == 0 {
                finest_noise = noise;
            }
            finer_noise = noise;
            thresholds[l] = denoise.threshold_at(photo_level, noise, finest_noise);
        }

        if cancel.is_cancelled() {
            return Err(host.cancelled());
        }

        // --- Pass 2: Bottom-up reconstruction + writeback into `photo` ---
        let mut encoder = device.create_command_encoder(&Default::default());
        for l in (0..levels.len()).rev() {
            let (lw, lh, _, share) = levels[l];
            let (cw, ch) = (lw.div_ceil(2), lh.div_ceil(2));
            self.dispatch(
                device,
                &mut encoder,
                P_UP_VEC2_H,
                &planes[l + 1],
                &self.dummy,
                &scratch,
                PyramidParams {
                    src_w: cw,
                    src_h: ch,
                    dst_w: lw,
                    dst_h: ch,
                    ..Default::default()
                },
            );
            self.dispatch(
                device,
                &mut encoder,
                P_UP_VEC2_RECON,
                &scratch,
                &self.dummy,
                &planes[l],
                PyramidParams {
                    src_w: lw,
                    src_h: ch,
                    dst_w: lw,
                    dst_h: lh,
                    f0: thresholds[l],
                    f1: share,
                    ..Default::default()
                },
            );
        }
        self.dispatch(
            device,
            &mut encoder,
            P_CHROMA_WRITEBACK,
            &planes[0],
            &self.dummy,
            photo,
            PyramidParams {
                src_w: width,
                src_h: height,
                dst_w: width,
                dst_h: height,
                ..Default::default()
            },
        );
        let sub = host.submit(encoder);
        host.wait(sub, cancel)?;
        Ok(())
    }

    /// Runs [`LuminanceDenoise`] in-place on the GPU camera RGB buffer `photo` (`width` × `height`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_luminance_denoise<H: PyramidHost>(
        &self,
        host: &H,
        photo: &wgpu::Buffer,
        width: u32,
        height: u32,
        wb: [f32; 3],
        photo_scale: f32,
        denoise: LuminanceDenoise,
        cancel: Cancel<'_>,
    ) -> std::result::Result<(), H::Error> {
        if cancel.is_cancelled() {
            return Err(host.cancelled());
        }
        let offset = -photo_scale.log2();
        let mut levels: Vec<(u32, u32, f32, f32)> = Vec::new();
        let (mut w, mut h) = (width, height);
        for level in 0..luminance_noise::LEVELS {
            let photo_level = level as f32 + offset;
            let share = laplacian::filtered_share(photo_level, luminance_noise::LEVELS);
            if share <= 0. || w < 4 || h < 4 {
                break;
            }
            levels.push((w, h, photo_level, share));
            w = w.div_ceil(2);
            h = h.div_ceil(2);
        }
        if levels.is_empty() {
            return Ok(());
        }
        let residual_size = (w, h);

        let n0 = u64::from(width) * u64::from(height);
        Self::check_limits(host, n0 * 4, width, height)?;

        let device = host.device();
        let white = luminance_noise::white_threshold(wb);

        // Ping-pong Gaussian/reconstructed buffers (`plane_a` holds level 0, `plane_b` holds level 1+).
        let plane_a = Self::alloc_storage(device, "Luma plane A", n0 * 4, false);
        let n1 = u64::from(width.div_ceil(2)) * u64::from(height.div_ceil(2));
        let plane_b = Self::alloc_storage(device, "Luma plane B", n1 * 4, false);

        // Detail bands (`levels.len()` buffers) and residual buffer.
        let mut bands: Vec<wgpu::Buffer> = Vec::with_capacity(levels.len());
        for &(lw, lh, _, _) in &levels {
            bands.push(Self::alloc_storage(
                device,
                "Luma detail band",
                u64::from(lw) * u64::from(lh) * 4,
                false,
            ));
        }
        let scratch_px = (u64::from(width.div_ceil(2)) * u64::from(height))
            .max(u64::from(width) * u64::from(height.div_ceil(2)));
        let scratch = Self::alloc_storage(device, "Luma pyramid scratch", scratch_px * 4, false);

        let mut stats_offsets: Vec<(usize, u32, u32)> = Vec::with_capacity(levels.len());
        let mut total_stat_floats: usize = 0;
        for &(lw, lh, _, _) in &levels {
            let bw = lw / 8;
            let bh = lh / 8;
            stats_offsets.push((total_stat_floats, bw, bh));
            total_stat_floats += (bw as usize) * (bh as usize);
        }
        let stats_buf = Self::alloc_storage(
            device,
            "Luma block stats",
            (total_stat_floats as u64 * 4).max(16),
            true,
        );

        // --- Pass 1: Extract luma + top-down pyramid + detail bands + block stats ---
        let mut encoder = device.create_command_encoder(&Default::default());
        self.dispatch(
            device,
            &mut encoder,
            P_LUMA_EXTRACT,
            photo,
            &self.dummy,
            &plane_a,
            PyramidParams {
                src_w: width,
                src_h: height,
                dst_w: width,
                dst_h: height,
                ..Default::default()
            },
        );

        for (l, &(lw, lh, _, _)) in levels.iter().enumerate() {
            let (cw, ch) = (lw.div_ceil(2), lh.div_ceil(2));
            let (cur, next) = if l % 2 == 0 {
                (&plane_a, &plane_b)
            } else {
                (&plane_b, &plane_a)
            };
            // Downsample cur -> next
            self.dispatch(
                device,
                &mut encoder,
                P_DOWN_F32_H,
                cur,
                &self.dummy,
                &scratch,
                PyramidParams {
                    src_w: lw,
                    src_h: lh,
                    dst_w: cw,
                    dst_h: lh,
                    ..Default::default()
                },
            );
            self.dispatch(
                device,
                &mut encoder,
                P_DOWN_F32_V,
                &scratch,
                &self.dummy,
                next,
                PyramidParams {
                    src_w: cw,
                    src_h: lh,
                    dst_w: cw,
                    dst_h: ch,
                    ..Default::default()
                },
            );
            // Expand next -> scratch, compute band[l] = cur - up(next)
            self.dispatch(
                device,
                &mut encoder,
                P_UP_F32_H,
                next,
                &self.dummy,
                &scratch,
                PyramidParams {
                    src_w: cw,
                    src_h: ch,
                    dst_w: lw,
                    dst_h: ch,
                    ..Default::default()
                },
            );
            self.dispatch(
                device,
                &mut encoder,
                P_UP_F32_V_SUB,
                cur,
                &scratch,
                &bands[l],
                PyramidParams {
                    src_w: lw,
                    src_h: ch,
                    dst_w: lw,
                    dst_h: lh,
                    ..Default::default()
                },
            );
            let (stat_off, bw, bh) = stats_offsets[l];
            if bw > 0 && bh > 0 {
                self.dispatch(
                    device,
                    &mut encoder,
                    P_LUMA_STATS,
                    cur,
                    &bands[l],
                    &stats_buf,
                    PyramidParams {
                        src_w: lw,
                        src_h: lh,
                        dst_w: bw,
                        dst_h: bh,
                        aux0: stat_off as u32,
                        f0: white,
                        ..Default::default()
                    },
                );
            }
        }

        let raw_stats = if total_stat_floats > 0 {
            Self::submit_and_readback_f32(host, encoder, &stats_buf, total_stat_floats, cancel)?
        } else {
            let sub = host.submit(encoder);
            host.wait(sub, cancel)?;
            Vec::new()
        };
        drop(stats_buf);

        // --- CPU exact quantile & per-level thresholds ---
        let mut thresholds = vec![0f32; levels.len()];
        let mut finer_noise = 0f32;
        for (l, &(_, _, photo_level, _)) in levels.iter().enumerate() {
            let (off, bw, bh) = stats_offsets[l];
            let count = (bw as usize) * (bh as usize);
            let mut energies: Vec<f32> = raw_stats[off..off + count]
                .iter()
                .copied()
                .filter(|&e| e >= 0.)
                .collect();
            let noise = luminance_noise::chain_level_noise(
                l,
                luminance_noise::band_noise_from_blocks(&mut energies),
                finer_noise,
            );
            finer_noise = noise;
            thresholds[l] = denoise.threshold_at(photo_level, noise);
        }

        if cancel.is_cancelled() {
            return Err(host.cancelled());
        }

        // --- Pass 2: Bottom-up reconstruction + writeback into `photo` ---
        // After `levels.len()` downsamples, the unfiltered residual sits in `plane_a` if
        // `levels.len()` is even, or `plane_b` if odd — which naturally aligns so level `l`
        // reads coarse from `(l + 1) % 2` and writes reconstructed level `l` to `l % 2`,
        // finishing with level 0 in `plane_a`!
        let _ = residual_size;
        let mut encoder = device.create_command_encoder(&Default::default());
        for l in (0..levels.len()).rev() {
            let (lw, lh, _, share) = levels[l];
            let (cw, ch) = (lw.div_ceil(2), lh.div_ceil(2));
            let (coarse_buf, dst_buf) = if l % 2 == 0 {
                (&plane_b, &plane_a)
            } else {
                (&plane_a, &plane_b)
            };
            self.dispatch(
                device,
                &mut encoder,
                P_UP_F32_H,
                coarse_buf,
                &self.dummy,
                &scratch,
                PyramidParams {
                    src_w: cw,
                    src_h: ch,
                    dst_w: lw,
                    dst_h: ch,
                    ..Default::default()
                },
            );
            self.dispatch(
                device,
                &mut encoder,
                P_UP_F32_RECON_LUMA,
                &scratch,
                &bands[l],
                dst_buf,
                PyramidParams {
                    src_w: lw,
                    src_h: ch,
                    dst_w: lw,
                    dst_h: lh,
                    f0: thresholds[l],
                    f1: share,
                    ..Default::default()
                },
            );
        }
        self.dispatch(
            device,
            &mut encoder,
            P_LUMA_WRITEBACK,
            &plane_a,
            &self.dummy,
            photo,
            PyramidParams {
                src_w: width,
                src_h: height,
                dst_w: width,
                dst_h: height,
                ..Default::default()
            },
        );
        let sub = host.submit(encoder);
        host.wait(sub, cancel)?;
        Ok(())
    }

    /// Runs measured Texture (`texture.rs`) in-place on the GPU camera RGB buffer `photo`
    /// (`width` × `height`), submitting one channel at a time with zero readbacks so
    /// cancellation is checked between channels and each submission stays bounded.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_texture<H: PyramidHost>(
        &self,
        host: &H,
        photo: &wgpu::Buffer,
        width: u32,
        height: u32,
        level_offset: usize,
        amount: f32,
        cancel: Cancel<'_>,
    ) -> std::result::Result<(), H::Error> {
        if cancel.is_cancelled() {
            return Err(host.cancelled());
        }
        let strength = texture::strength(amount);
        if strength == 0. {
            return Ok(());
        }
        let mut levels: Vec<(u32, u32, usize)> = Vec::new();
        let (mut w, mut h) = (width, height);
        let mut level = level_offset;
        while level < texture::LEVELS && w >= 4 && h >= 4 {
            levels.push((w, h, level));
            w = w.div_ceil(2);
            h = h.div_ceil(2);
            level += 1;
        }
        if levels.is_empty() {
            return Ok(());
        }
        let residual_size = (w, h);

        let n0 = u64::from(width) * u64::from(height);
        Self::check_limits(host, n0 * 4, width, height)?;

        let device = host.device();
        let mut planes: Vec<wgpu::Buffer> = Vec::with_capacity(levels.len() + 1);
        for &(lw, lh, _) in &levels {
            planes.push(Self::alloc_storage(
                device,
                "Texture pyramid plane",
                u64::from(lw) * u64::from(lh) * 4,
                false,
            ));
        }
        planes.push(Self::alloc_storage(
            device,
            "Texture pyramid coarse",
            u64::from(residual_size.0) * u64::from(residual_size.1) * 4,
            false,
        ));
        let scratch_px = (u64::from(width.div_ceil(2)) * u64::from(height))
            .max(u64::from(width) * u64::from(height.div_ceil(2)));
        let scratch = Self::alloc_storage(device, "Texture pyramid scratch", scratch_px * 4, false);

        for channel in 0..3u32 {
            if cancel.is_cancelled() {
                return Err(host.cancelled());
            }
            let mut encoder = device.create_command_encoder(&Default::default());
            self.dispatch(
                device,
                &mut encoder,
                P_TEXTURE_EXTRACT,
                photo,
                &self.dummy,
                &planes[0],
                PyramidParams {
                    src_w: width,
                    src_h: height,
                    dst_w: width,
                    dst_h: height,
                    aux0: channel,
                    ..Default::default()
                },
            );
            // Top-down: downsample `planes[k] -> planes[k+1]` and overwrite `planes[k]`
            // in-place with the compressed weighted detail band for level `tex_level`.
            for (k, &(lw, lh, tex_level)) in levels.iter().enumerate() {
                let (cw, ch) = (lw.div_ceil(2), lh.div_ceil(2));
                self.dispatch(
                    device,
                    &mut encoder,
                    P_DOWN_F32_H,
                    &planes[k],
                    &self.dummy,
                    &scratch,
                    PyramidParams {
                        src_w: lw,
                        src_h: lh,
                        dst_w: cw,
                        dst_h: lh,
                        ..Default::default()
                    },
                );
                self.dispatch(
                    device,
                    &mut encoder,
                    P_DOWN_F32_V,
                    &scratch,
                    &self.dummy,
                    &planes[k + 1],
                    PyramidParams {
                        src_w: cw,
                        src_h: lh,
                        dst_w: cw,
                        dst_h: ch,
                        ..Default::default()
                    },
                );
                self.dispatch(
                    device,
                    &mut encoder,
                    P_UP_F32_H,
                    &planes[k + 1],
                    &self.dummy,
                    &scratch,
                    PyramidParams {
                        src_w: cw,
                        src_h: ch,
                        dst_w: lw,
                        dst_h: ch,
                        ..Default::default()
                    },
                );
                self.dispatch(
                    device,
                    &mut encoder,
                    P_UP_F32_COMPRESS_TEX,
                    &scratch,
                    &self.dummy,
                    &planes[k],
                    PyramidParams {
                        src_w: lw,
                        src_h: ch,
                        dst_w: lw,
                        dst_h: lh,
                        f0: texture::WEIGHTS[tex_level],
                        f1: texture::THRESHOLDS[tex_level],
                        ..Default::default()
                    },
                );
            }
            // Bottom-up: expand accumulated detail `planes[k+1]` and add into `planes[k]`.
            if levels.len() >= 2 {
                for k in (0..levels.len() - 1).rev() {
                    let (lw, lh, _) = levels[k];
                    let (cw, ch) = (lw.div_ceil(2), lh.div_ceil(2));
                    self.dispatch(
                        device,
                        &mut encoder,
                        P_UP_F32_H,
                        &planes[k + 1],
                        &self.dummy,
                        &scratch,
                        PyramidParams {
                            src_w: cw,
                            src_h: ch,
                            dst_w: lw,
                            dst_h: ch,
                            ..Default::default()
                        },
                    );
                    self.dispatch(
                        device,
                        &mut encoder,
                        P_UP_F32_ADD_INPLACE,
                        &scratch,
                        &self.dummy,
                        &planes[k],
                        PyramidParams {
                            src_w: lw,
                            src_h: ch,
                            dst_w: lw,
                            dst_h: lh,
                            ..Default::default()
                        },
                    );
                }
            }
            self.dispatch(
                device,
                &mut encoder,
                P_TEXTURE_APPLY,
                &planes[0],
                &self.dummy,
                photo,
                PyramidParams {
                    src_w: width,
                    src_h: height,
                    dst_w: width,
                    dst_h: height,
                    aux0: channel,
                    f0: strength,
                    f1: texture::DETAIL_STEP,
                    ..Default::default()
                },
            );
            let sub = host.submit(encoder);
            host.wait(sub, cancel)?;
        }
        Ok(())
    }

    /// Runs crop-clamped box reduction on `photo` (`width` × `height`) to `(out_w, out_h)`
    /// and reads back the small RGB buffer (matching `pipeline::sampling::crop_clamped_preview`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn box_reduce<H: PyramidHost>(
        &self,
        host: &H,
        photo: &wgpu::Buffer,
        width: u32,
        height: u32,
        crop: [u32; 4],
        out_w: u32,
        out_h: u32,
        cancel: Cancel<'_>,
    ) -> std::result::Result<Vec<[f32; 3]>, H::Error> {
        if cancel.is_cancelled() {
            return Err(host.cancelled());
        }
        if width == 0 || height == 0 || out_w == 0 || out_h == 0 {
            return Ok(Vec::new());
        }
        let cx0 = crop[0].min(width - 1);
        let cy0 = crop[1].min(height - 1);
        let cx1 = crop[2].clamp(cx0 + 1, width);
        let cy1 = crop[3].clamp(cy0 + 1, height);
        let out_floats = out_w as usize * out_h as usize * 3;
        Self::check_limits(host, out_floats as u64 * 4, out_w, out_h)?;
        let device = host.device();
        let reduced = Self::alloc_storage(device, "Box reduced copy", out_floats as u64 * 4, true);
        let mut encoder = device.create_command_encoder(&Default::default());
        self.dispatch(
            device,
            &mut encoder,
            P_BOX_REDUCE,
            photo,
            &self.dummy,
            &reduced,
            PyramidParams {
                src_w: width,
                src_h: height,
                dst_w: out_w,
                dst_h: out_h,
                aux0: cx0,
                aux1: cy0,
                aux2: cx1,
                aux3: cy1,
                ..Default::default()
            },
        );
        let floats = Self::submit_and_readback_f32(host, encoder, &reduced, out_floats, cancel)?;
        Ok(floats.as_chunks::<3>().0.to_vec())
    }

    /// Reads back the full camera RGB buffer `photo` (`width` × `height`). Only needed when
    /// CPU-side retouching or an Auto Mask brush runs after GPU pyramid stages.
    pub(super) fn readback_photo<H: PyramidHost>(
        host: &H,
        photo: &wgpu::Buffer,
        width: u32,
        height: u32,
        cancel: Cancel<'_>,
    ) -> std::result::Result<Vec<[f32; 3]>, H::Error> {
        let floats_len = width as usize * height as usize * 3;
        let encoder = host.device().create_command_encoder(&Default::default());
        let floats = Self::submit_and_readback_f32(host, encoder, photo, floats_len, cancel)?;
        Ok(floats.as_chunks::<3>().0.to_vec())
    }
}

#[cfg(test)]
pub(super) fn wgsl_source() -> &'static str {
    include_str!("pyramid.wgsl")
}
