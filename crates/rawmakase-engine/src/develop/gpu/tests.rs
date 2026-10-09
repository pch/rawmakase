use super::*;
use crate::develop::masks::local::LocalDeltas;

#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_matches_cpu_finishing_and_reuses_buffers() -> Result<()> {
    let mut gpu = Processor::new()?;
    eprintln!("GPU: {}", gpu.name());
    let cancel = AtomicBool::new(false);
    for (w, h) in [(97, 63), (1, 33), (33, 1), (1, 1)] {
        let image = Rendered {
            width: w,
            height: h,
            pixels: (0..w * h)
                .map(|i| {
                    let x = (i % w) as f32 / w as f32;
                    let y = (i / w) as f32 / h as f32;
                    [x, y, if i % 7 < 3 { 0.03 } else { 0.96 }]
                })
                .collect(),
        };
        for amount in [0., 0.7] {
            for max_edge in [0, 1, 17, 90] {
                for radius in [0.5, 3.] {
                    let recipe = Recipe {
                        sharpening: amount,
                        sharpening_radius: radius,
                        sharpening_masking: 0.6,
                        sharpening_detail: 0.8,
                        ..Recipe::default()
                    };
                    let mut expected = image.clone();
                    crate::develop::quality::sharpen(&mut expected, &recipe);
                    let expected = crate::develop::quality::resize(expected, max_edge);
                    for _ in 0..2 {
                        let actual = gpu.finish(&image, &recipe, max_edge, &cancel)?;
                        assert_eq!(
                            (actual.width, actual.height),
                            (expected.width, expected.height)
                        );
                        let error = actual
                            .pixels
                            .iter()
                            .flatten()
                            .zip(expected.pixels.iter().flatten())
                            .map(|(a, b)| (a - b).abs())
                            .fold(0f32, f32::max);
                        assert!(
                            error < 2e-5,
                            "{w}x{h}, edge={max_edge}, amount={amount}, radius={radius}: {error}"
                        );
                    }
                }
            }
        }
    }
    cancel.store(true, Ordering::Relaxed);
    let image = Rendered {
        width: 1,
        height: 1,
        pixels: vec![[0.5; 3]],
    };
    assert!(gpu.finish(&image, &Recipe::default(), 1, &cancel).is_err());
    Ok(())
}

#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_preview_preserves_regions_spatial_effects_and_falls_back() -> Result<()> {
    use crate::{
        camera_data::{CameraImage, Metadata},
        develop::PreviewRenderer,
    };
    let image = CameraImage {
        width: 137,
        height: 91,
        pixels: (0..137 * 91)
            .map(|i| {
                let x = (i % 137) as f32 / 137.;
                let y = (i / 137) as f32 / 91.;
                [x * 1.2, y * 0.8, 0.2 + x * y]
            })
            .collect(),
        metadata: Metadata {
            width: 137,
            height: 91,
            wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        recovered: Default::default(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    };
    let mut renderer = PreviewRenderer::with_gpu();
    assert!(
        renderer.adapter_name().is_some(),
        "{:?}",
        renderer.fallback_reason()
    );
    let cancel = AtomicBool::new(false);
    for spatial in [false, true] {
        for region in [None, Some([3, 2, 29, 41])] {
            let mut recipe = Recipe {
                exposure: 0.3,
                shadows: 0.2,
                sharpening_radius: 3.,
                rotation: 1,
                crop: [0.1, 0.05, 0.9, 0.95],
                ..Default::default()
            };
            if spatial {
                recipe.effects.grain = 0.4;
                recipe.effects.vignette = -0.3;
            }
            // Reduced Fit sizes render from the pyramid on the CPU; full size and
            // regions use GPU finishing.
            let expected = super::super::quality::render_cancellable(
                &image,
                &recipe.checked()?,
                0,
                region,
                &cancel,
            )?;
            let actual = renderer.render(&image, &recipe, 0, region, &cancel)?;
            assert_eq!(renderer.used_gpu(), !spatial || region.is_none());
            assert_eq!(
                (actual.width, actual.height),
                (expected.width, expected.height)
            );
            let error = actual
                .pixels
                .iter()
                .flatten()
                .zip(expected.pixels.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0f32, f32::max);
            assert!(
                error < 2e-5,
                "spatial={spatial}, region={region:?}, max error={error}"
            );
        }
    }
    // A failed backend is disabled rather than retried on every interaction.
    let invalid = Rendered {
        width: 2,
        height: 1,
        pixels: vec![[0.5; 3]],
    };
    assert!(
        renderer
            .finish(&invalid, &Recipe::default(), 0, &cancel)
            .is_none()
    );
    assert!(renderer.adapter_name().is_none());
    assert!(renderer.fallback_reason().is_some());
    let recipe = Recipe::default();
    let expected = super::super::quality::render(&image, &recipe.checked()?, 0, None)?;
    let actual = renderer.render(&image, &recipe, 0, None, &cancel)?;
    assert_eq!(actual.pixels, expected.pixels);
    assert!(!renderer.used_gpu());
    Ok(())
}

/// The GPU per-pixel stage against the CPU reference, over recipes that exercise every
/// table and branch of `develop.wgsl`.
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients.
fn gpu_develop_matches_cpu_pixel_stage() -> Result<()> {
    use crate::{
        camera_data::{CameraImage, Metadata},
        camera_profiles::CameraProfile,
        develop::pipeline::{Samples, Source, develop_samples, pixel_params::pixel_params},
    };
    use std::sync::Arc;
    let metadata = Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        width: 64,
        height: 48,
        wb: [2.02, 1., 1.89],
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    };
    let wave = |i: usize, k: f32| ((i as f32 * k).sin() * 0.5 + 0.5).powi(2);
    let image = Arc::new(CameraImage {
        width: 64,
        height: 48,
        pixels: (0..64 * 48)
            .map(|i| [wave(i, 0.37) * 1.3, wave(i, 0.21), wave(i, 0.13) * 1.1])
            .collect(),
        metadata: metadata.clone(),
        recovered: Default::default(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    let n = 4000;
    let samples = Arc::new(Samples {
        width: 80,
        height: 50,
        // Dark, mid, bright and out-of-range camera values in all hue directions.
        pixels: (0..n)
            .map(|i| {
                [
                    wave(i, 0.71) * 1.6,
                    wave(i, 0.53) * 1.2,
                    wave(i, 0.29) * 1.5,
                ]
            })
            .collect(),
        positions: (0..n)
            .map(|i| {
                if i % 97 == 0 {
                    [f32::NAN; 2]
                } else {
                    [(i % 64) as f32, (i / 64 % 48) as f32]
                }
            })
            .collect(),
    });
    let plain = CameraProfile::camera_matrix_default(&metadata).unwrap();
    let tables = plain.clone().with_test_tables();
    let base = |profile: &CameraProfile| Recipe {
        profile: Some(Arc::new(profile.clone())),
        temperature: 5000.,
        ..Default::default()
    };
    let mut recipes = vec![base(&plain), base(&tables)];
    // A look's RGB tables: 3D and 1D, clipping and extending the gamut.
    let rgb_tables = tables.clone().with_test_rgb_tables();
    recipes.extend(rgb_tables.iter().map(base));
    let mut r = base(&rgb_tables[0]);
    r.saturation = 0.3;
    r.grading[1] = [0.1, 0.3, 0.];
    recipes.push(r);
    let mut r = base(&tables);
    r.exposure = 0.7;
    r.contrast = 0.4;
    r.whites = -0.3;
    r.blacks = 0.2;
    r.effects.dehaze = 0.25;
    r.curve.insert([0.3, 0.25]);
    r.effects.channels[2].insert([0.6, 0.7]);
    r.effects.parametric = [0.2, -0.1, 0.3, 0.];
    r.curve_saturation = 0.4;
    r.black_point = 0.02;
    r.white_point = 0.97;
    r.midtone = 1.2;
    recipes.push(r.clone());
    // The measured parametric curve, with moved splits, from here on.
    r.effects.parametric = [0.3, 0.2, -0.3, -0.2];
    r.effects.splits = [0.2, 0.45, 0.8];
    r.shadows = 0.5;
    r.highlights = -0.6;
    recipes.push(r.clone());
    r.hsl[1] = [0.3, -0.5, 0.4];
    r.hsl[5] = [-0.2, 0.6, -0.3];
    r.saturation = 0.2;
    r.vibrance = -0.3;
    r.grading[0] = [0.6, 0.4, -0.2];
    r.effects.global_grade = [0.1, 0.2, 0.1];
    r.effects.calibration = [[0.3, -0.2], [-0.4, 0.5], [0.2, 0.1]];
    r.effects.shadow_tint = -0.4;
    recipes.push(r.clone());
    // Grading at other Blending and Balance.
    let mut measured = r.clone();
    measured.grading[2] = [0.1, 0.5, 0.2];
    measured.effects.blending = 0.8;
    measured.effects.balance = -0.3;
    recipes.push(measured.clone());
    // Out-of-gamut colors clipped per channel.
    measured.saturation = 0.8;
    recipes.push(measured.clone());
    // Calibration measured on Camera Raw, between its measured slider positions.
    measured.effects.calibration = [[0.3, -0.75], [-0.4, 0.5], [0.9, 0.1]];
    recipes.push(measured.clone());
    // Saturation fading to gray below −50, with and without band sliders.
    measured.saturation = -0.7;
    recipes.push(measured.clone());
    measured.hsl = [[0.; 3]; 8];
    measured.vibrance = 0.;
    measured.saturation = -1.;
    recipes.push(measured);
    // Black & white from the chart tables, with a mix between measured positions.
    let mut mono = r.clone();
    mono.effects.monochrome = true;
    mono.effects.gray_mix = [0.3, -0.7, 0.1, 0., -0.2, 0.9, 0., -1.];
    recipes.push(mono);
    // Point Color: overlapping swatches, one across red, with Variance and Range.
    let mut warm = crate::model::point_color::PointColor::sampled([0.6, 0.5, 0.2]);
    warm.shift = [0.4, -0.5, 0.3];
    warm.variance = 0.6;
    warm.range = 0.8;
    let mut red = crate::model::point_color::PointColor::sampled([5.8, 0.4, 0.1]);
    red.shift = [-0.6, 0.7, -0.4];
    red.range = 0.2;
    let mut cool = crate::model::point_color::PointColor::sampled([3.5, 0.3, 0.3]);
    cool.shift = [0.2, 0.3, 0.];
    cool.variance = -0.8;
    r.point_colors = vec![warm, red, cool];
    let point_colors = recipes.len();
    recipes.push(r.clone());
    // Visualize Range of the second swatch.
    let mut visualized = r.clone();
    visualized.point_colors =
        crate::model::point_color::visualize_range(&r.point_colors, 1).unwrap();
    recipes.push(visualized);
    r.effects.defringe = [0.5, 0.3];
    recipes.push(r.clone());
    r.effects.monochrome = true;
    r.effects.gray_mix = [0.2, -0.3, 0.1, 0.4, -0.2, 0.3, 0., -0.1];
    recipes.push(r);
    let mut gpu = Processor::new()?;
    let cancel = AtomicBool::new(false);
    // A local-tone gain changes the Shadows/Highlights map's input.
    let gain: Vec<f32> = (0..64 * 48).map(|i| 0.6 + wave(i, 0.05)).collect();
    // The swatches select part of the test image (the same source for both).
    let source = Source::new(&image, (point_colors % 2 == 1).then_some(gain.as_slice()));
    let without = develop_samples(source, &recipes[point_colors - 1], &samples, &cancel, None)?;
    let with = develop_samples(source, &recipes[point_colors], &samples, &cancel, None)?;
    let changed = (with.pixels.iter().zip(&without.pixels))
        .filter(|(a, b)| (0..3).any(|c| (a[c] - b[c]).abs() > 0.01))
        .count();
    assert!(changed > with.pixels.len() / 50, "{changed} pixels changed");
    for (i, recipe) in recipes.iter().enumerate() {
        let source = Source::new(&image, (i % 2 == 1).then_some(gain.as_slice()));
        let params = pixel_params(source, recipe).expect("GPU port covers this recipe");
        let expected = develop_samples(source, recipe, &samples, &cancel, None)?;
        let actual = gpu.develop(&samples, &params, &cancel)?;
        let d: Vec<f32> = actual
            .pixels
            .iter()
            .flatten()
            .zip(expected.pixels.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .collect();
        let mean = d.iter().sum::<f32>() / d.len() as f32;
        let mut sorted = d.clone();
        sorted.sort_by(f32::total_cmp);
        let (p999, max) = (sorted[sorted.len() * 999 / 1000], sorted[sorted.len() - 1]);
        eprintln!("recipe {i}: max {max:.6}, 99.9% {p999:.6}, mean {mean:.8}");
        // Near-neutral pixels have an unstable Oklab hue angle, which can move them to
        // another Monochrome band; everything else agrees to float precision.
        assert!(
            p999 < 1e-3 && max < 0.02 && mean < 2e-5,
            "recipe {i}: max {max}, 99.9% {p999}, mean {mean}"
        );
    }
    Ok(())
}

/// Reads a presented texture back as RGB bytes.
fn read_texture(gpu: &Processor, texture: &wgpu::Texture) -> Result<Vec<u8>> {
    let row = (texture.width() * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: row as u64 * texture.height() as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
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
    gpu.queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
    let bytes = buffer.slice(..).get_mapped_range()?;
    Ok(bytes
        .chunks_exact(row as usize)
        .flat_map(|r| r[..texture.width() as usize * 4].as_chunks::<4>().0.iter())
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect())
}

/// Previews presented on the GPU (develop, sharpening, spatial effects, clipping
/// overlay, histogram) against the CPU renderer's pixels, for Fit, full size and a
/// 100% region.
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients.
fn presented_previews_match_the_cpu_render() -> Result<()> {
    use crate::{
        camera_data::{CameraImage, Metadata},
        camera_profiles::CameraProfile,
        develop::{PreviewRenderer, quality::Output},
    };
    use std::sync::Arc;
    let (w, h) = (157, 103);
    let radial = |values: Vec<f32>| crate::optics::Radial {
        knots: (0..values.len())
            .map(|i| i as f32 / (values.len() - 1) as f32)
            .collect(),
        values,
    };
    let metadata = Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        width: w,
        height: h,
        lens: Some(crate::optics::LensCorrection {
            vignetting: Some(radial(vec![1., 1.1, 1.3, 1.6])),
            distortion: Some(radial(vec![1., 0.99, 1.02, 1.05])),
            chromatic: Some([
                radial(vec![1., 1.001, 1.002]),
                radial(vec![1., 0.999, 0.998]),
            ]),
            ..Default::default()
        }),
        wb: [2.02, 1., 1.89],
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    };
    // Remove Chromatic Aberration's measurement, as if made from the photo.
    let _ = metadata.lateral_ca.set(Some([
        radial(vec![1., 1.002, 1.004]),
        radial(vec![1., 0.998, 0.997]),
    ]));
    let image = CameraImage {
        width: w,
        height: h,
        pixels: (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let v = 0.25 + 0.2 * (x * 0.21).sin() * (y * 0.13).cos() + 0.1 * (x * 0.9).sin();
                [v * 1.3, v, v * 0.8 + x / w as f32 * 0.3]
            })
            .collect(),
        metadata: metadata.clone(),
        recovered: Default::default(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    };
    let profile = CameraProfile::camera_matrix_default(&metadata)
        .unwrap()
        .with_test_tables();
    let base = Recipe {
        profile: Some(Arc::new(profile)),
        temperature: 5000.,
        exposure: 0.4,
        shadows: 0.3,
        sharpening: 0.8,
        sharpening_radius: 1.2,
        sharpening_masking: 0.3,
        ..Default::default()
    };
    let mut gpu = PreviewRenderer::with_gpu();
    let mut cpu = PreviewRenderer::default();
    let cancel = AtomicBool::new(false);
    // `ca`: 1 the measured aberration alone, 2 with the built-in distortion. Each
    // spatial run has another vignette style and amount.
    use crate::model::effects::VignetteStyle::*;
    use crate::rendered::ClipOverlay;
    let (none, both) = (
        ClipOverlay::NONE,
        ClipOverlay {
            shadows: true,
            highlights: true,
        },
    );
    let shadows = ClipOverlay {
        shadows: true,
        ..none
    };
    let highlights = ClipOverlay {
        highlights: true,
        ..none
    };
    // Manual Vignetting is sampled with the lens profile's; `lens_alone` without lens
    // data, where the manual gain alone makes the lens stage.
    for (spatial, clipping, ca, (style, vignette), lens_alone) in [
        (false, shadows, 0, (HighlightPriority, 0.), false),
        (true, none, 0, (HighlightPriority, -0.3), false),
        (true, both, 0, (ColorPriority, -0.6), false),
        (true, highlights, 0, (PaintOverlay, -0.5), false),
        (true, none, 1, (HighlightPriority, 0.5), false),
        (true, none, 2, (ColorPriority, 0.4), false),
        (true, none, 0, (PaintOverlay, 0.7), false),
        (true, none, 0, (ColorPriority, 0.), true),
    ] {
        let mut recipe = base.clone();
        if spatial {
            recipe.effects.grain = 0.4;
            recipe.effects.vignette = vignette;
            recipe.effects.vignette_style = style;
            recipe.effects.vignette_highlights = 0.6;
            recipe.effects.vignette_roundness = -0.3;
            recipe.effects.vignette_midpoint = 0.4;
            recipe.effects.lens_vignette = 0.2;
            recipe.effects.clarity = 0.3;
            recipe.effects.texture = -0.4;
            recipe.whites = 0.6;
            // Geometry, lens correction and noise reduction in the sampling stage.
            recipe.straighten = 3.;
            recipe.crop = [0.05, 0.1, 0.95, 0.92];
            recipe.transform.vertical = 0.2;
            recipe.upright.mode = crate::model::transform::UprightMode::Level;
            recipe.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4];
            recipe.upright.corrections[3] = [1.02, 0.01, -0.02, -0.02, 1.02, 0.01, 0.01, 0., 1.];
            recipe.lens_builtin = true;
            recipe.lens_distortion = 0.8;
            recipe.lens_manual_distortion = 0.3;
            recipe.noise_luma = 0.4;
            recipe.noise_chroma = 0.5;
        }
        if ca > 0 {
            recipe.lens_builtin = ca == 2;
            recipe.lens_ca = true;
        }
        if lens_alone {
            recipe.lens_builtin = false;
            recipe.effects.lens_vignette = -0.6;
        }
        for (max_edge, region) in [(60, None), (0, None), (0, Some([10, 7, 50, 40]))] {
            let display = super::Display {
                slot: if region.is_some() {
                    super::Slot::Region
                } else {
                    super::Slot::Whole
                },
                clipping,
                monitor: None,
                navigator: Some(20),
                thumbnail: Some(30),
                samples: true,
                drawn: Vec::new(),
            };
            let expected = cpu.render(&image, &recipe, max_edge, region, &cancel)?;
            let Output::Frame(frame) =
                gpu.render_to(&image, &recipe, max_edge, region, &cancel, Some(&display))?
            else {
                panic!("{:?}", gpu.fallback_reason());
            };
            assert_eq!(
                (frame.width, frame.height),
                (expected.width, expected.height)
            );
            let mut rgb = expected.rgb8();
            clipping.paint(&mut rgb, &expected.pixels);
            let processor = gpu.gpu().unwrap();
            // The photo stayed on the device: sampled there, not on the CPU.
            assert!(!processor.resident.as_ref().unwrap().samples.is_empty());
            let mut actual = read_texture(processor, &frame.texture)?;
            // A value within float rounding of a clipping threshold may land on
            // either side of it; such pixels are compared without the overlay.
            let near = |v: f32| {
                [
                    crate::rendered::HIGHLIGHT_CLIP,
                    crate::rendered::SHADOW_CLIP,
                ]
                .iter()
                .any(|t| (v - t).abs() < 1e-4)
            };
            for ((a, e), orig) in actual
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(rgb.as_chunks::<3>().0)
                .zip(&expected.pixels)
            {
                if orig.iter().any(|v| near(*v)) {
                    *a = *e;
                }
            }
            let worst = actual
                .iter()
                .zip(&rgb)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            let label = format!(
                "spatial={spatial} clipping={clipping:?} ca={ca} {style:?} {vignette} {max_edge} {region:?}"
            );
            let changed = actual.iter().zip(&rgb).filter(|(a, b)| a != b).count();
            eprintln!(
                "{label}: largest difference {worst}, {changed} of {} values",
                rgb.len()
            );
            assert!(worst <= 1, "{label}: largest difference {worst}");
            let histogram = expected.histogram();
            // Counted from the same values, so within the GPU's float rounding.
            let clipped =
                |h: &crate::rendered::Histogram| [h.clipped.shadows, h.clipped.highlights].concat();
            for (gpu, cpu) in clipped(&frame.histogram).iter().zip(clipped(&histogram)) {
                assert!(
                    gpu.abs_diff(cpu) <= cpu / 100 + 2,
                    "{label}: clipped {gpu} vs {cpu}"
                );
            }
            for (gpu, cpu) in frame.histogram.bins.iter().zip(&histogram.bins) {
                let total: u32 = gpu.iter().sum();
                assert_eq!(total, frame.width * frame.height, "{label}");
                let moved: u32 = gpu.iter().zip(cpu).map(|(a, b)| a.abs_diff(*b)).sum();
                assert!(moved <= total / 50 + 2, "{label}: histogram moved {moved}");
            }
            let navigator = frame.navigator.as_ref().unwrap();
            assert_eq!(navigator.width().max(navigator.height()), 20);
            let (tw, th, bytes) = frame.thumbnail.as_ref().unwrap();
            assert_eq!(((*tw).max(*th), bytes.len()), (30, (tw * th * 3) as usize));
            // The loupe's samples are the photo without the clipping overlay.
            let (sw, sh, samples) = frame.samples.as_ref().unwrap();
            assert_eq!((*sw, *sh), (frame.width, frame.height));
            let plain = expected.rgb8();
            let worst = samples
                .iter()
                .zip(&plain)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(worst <= 1, "{label}: loupe samples differ by {worst}");
        }
    }
    Ok(())
}

/// Panning at 100% renders regions of one size; each goes into a texture other than
/// the one the viewport draws, which would otherwise show it at the old position.
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn panning_never_writes_the_drawn_region() -> Result<()> {
    use crate::{
        camera_data::{CameraImage, Metadata},
        develop::{PreviewRenderer, quality::Output},
    };
    let (w, h) = (120, 80);
    let image = CameraImage {
        width: w,
        height: h,
        pixels: (0..w * h)
            .map(|i| [0.2 + (i % w) as f32 / w as f32 * 0.5, 0.3, 0.4])
            .collect(),
        metadata: Metadata {
            width: w,
            height: h,
            wb: [2., 1., 1.8],
            cam_xyz: [[1.1, -0.5, -0.1], [-0.4, 1.2, 0.2], [-0.1, 0.15, 0.5]],
            ..Default::default()
        },
        recovered: Default::default(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    };
    let recipe = Recipe {
        profile: Some(std::sync::Arc::new(
            crate::camera_profiles::CameraProfile::camera_matrix_default(&image.metadata)
                .unwrap()
                .with_test_tables(),
        )),
        ..Default::default()
    };
    let mut gpu = PreviewRenderer::with_gpu();
    let cancel = AtomicBool::new(false);
    let mut drawn: Option<(wgpu::Texture, u64)> = None;
    for x in [0, 4, 8, 12, 16] {
        let display = super::Display {
            slot: super::Slot::Region,
            clipping: crate::rendered::ClipOverlay::NONE,
            monitor: None,
            navigator: None,
            thumbnail: None,
            samples: false,
            drawn: drawn.iter().map(|(t, _)| t.clone()).collect(),
        };
        let Output::Frame(frame) = gpu.render_to(
            &image,
            &recipe,
            0,
            Some([x, 10, 60, 40]),
            &cancel,
            Some(&display),
        )?
        else {
            panic!("{:?}", gpu.fallback_reason());
        };
        if let Some((texture, generation)) = &drawn {
            assert!(frame.texture != *texture, "x={x}: wrote the drawn region");
            let processor = gpu.gpu().unwrap();
            assert_eq!(processor.generation(texture), Some(*generation), "x={x}");
            assert!(!frame.released.contains(texture), "x={x}: released it");
        }
        drawn = Some((frame.texture, frame.generation));
    }
    Ok(())
}

/// Masks on the GPU match the CPU reference: every slider the port renders, with
/// partial and overlapping weights.
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients.
fn gpu_masks_match_cpu_pixel_stage() -> Result<()> {
    use crate::model::masks::LocalAdjust;
    use crate::{
        camera_data::{CameraImage, Metadata},
        camera_profiles::CameraProfile,
        develop::{
            masks::MaskWeights,
            pipeline::{Samples, Source, develop_samples, pixel_params::pixel_params},
        },
    };
    use std::sync::Arc;
    let metadata = Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        width: 64,
        height: 48,
        wb: [2.02, 1., 1.89],
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    };
    let wave = |i: usize, k: f32| ((i as f32 * k).sin() * 0.5 + 0.5).powi(2);
    let image = Arc::new(CameraImage {
        width: 64,
        height: 48,
        pixels: (0..64 * 48)
            .map(|i| [wave(i, 0.37) * 1.3, wave(i, 0.21), wave(i, 0.13) * 1.1])
            .collect(),
        metadata: metadata.clone(),
        recovered: Default::default(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    let n = 4000;
    let samples = Arc::new(Samples {
        width: 80,
        height: 50,
        pixels: (0..n)
            .map(|i| [wave(i, 0.71) * 1.4, wave(i, 0.53), wave(i, 0.29) * 1.2])
            .collect(),
        positions: (0..n)
            .map(|i| [(i % 64) as f32, (i / 64 % 48) as f32])
            .collect(),
    });
    let profile = CameraProfile::camera_matrix_default(&metadata).unwrap();
    let mut r = Recipe {
        profile: Some(Arc::new(profile)),
        temperature: 5000.,
        exposure: 0.3,
        contrast: 0.2,
        shadows: 0.2,
        ..Default::default()
    };
    r.masks = vec![Default::default(), Default::default()];
    let adjust = [
        LocalAdjust {
            temperature: 0.4,
            tint: -0.3,
            exposure: 0.8,
            contrast: 0.5,
            whites: -0.4,
            blacks: 0.3,
            dehaze: 0.2,
            highlights: -0.5,
            shadows: 0.6,
            ..Default::default()
        },
        LocalAdjust {
            exposure: -0.6,
            hue: 40.,
            saturation: -0.4,
            color: [0.6, 0.5],
            ..Default::default()
        },
    ];
    let weights = MaskWeights {
        deltas: adjust.iter().map(|a| a.delta(0.9)).collect(),
        data: Arc::new(
            (0..n)
                .flat_map(|i| {
                    [
                        (i * 7 % 256) as u8,
                        if i % 3 == 0 { 0 } else { (i * 13 % 256) as u8 },
                    ]
                })
                .collect(),
        ),
    };
    for (m, a) in r.masks.iter_mut().zip(adjust) {
        m.adjust = a;
    }
    let mut gpu = Processor::new()?;
    let cancel = AtomicBool::new(false);
    let source = Source::from(image.as_ref());
    r.whites = 0.3;
    {
        let mut params = pixel_params(source, &r).expect("GPU port covers this recipe");
        assert!(params.set_masks(source, &r, Some(&weights)));
        let expected = develop_samples(source, &r, &samples, &cancel, Some(&weights))?;
        let actual = gpu.develop(&samples, &params, &cancel)?;
        let d: Vec<f32> = actual
            .pixels
            .iter()
            .flatten()
            .zip(expected.pixels.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .collect();
        let mean = d.iter().sum::<f32>() / d.len() as f32;
        let max = d.iter().copied().fold(0., f32::max);
        eprintln!("masks: max {max:.6}, mean {mean:.8}");
        assert!(max < 2e-3 && mean < 2e-5, "max {max}, mean {mean}");
    }
    Ok(())
}

/// Every shader module, assembled as the device receives it, parses and validates
/// as WGSL, with the entry points the pipelines ask for. The hardware tests above are
/// ignored by default, so this is the check that runs in CI.
#[test]
fn shaders_are_valid_wgsl() {
    use wgpu::naga::{
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };
    let prelude = crate::develop::pipeline::pixel_params::wgsl_prelude();
    let modules: [(&str, String, &[&str]); 6] = [
        (
            "develop.wgsl",
            prelude.clone() + include_str!("develop.wgsl"),
            &["develop"],
        ),
        (
            "logs.wgsl",
            prelude + include_str!("develop.wgsl") + include_str!("logs.wgsl"),
            &["log_luminance"],
        ),
        (
            "local.wgsl",
            super::sampling::wgsl_prelude() + include_str!("local.wgsl"),
            &[
                "running_sum",
                "window",
                "region_gain",
                "sample_region",
                "reduce_rows",
                "reduce_toned",
            ],
        ),
        (
            "finish.wgsl",
            include_str!("finish.wgsl").into(),
            &[
                "blur_horizontal",
                "sharpen",
                "resize_vertical",
                "resize_horizontal",
            ],
        ),
        (
            "present.wgsl",
            crate::develop::effects::PostCropVignette::wgsl_tone() + include_str!("present.wgsl"),
            &["blur_horizontal", "sharpen", "present"],
        ),
        (
            "reduce.wgsl",
            include_str!("reduce.wgsl").into(),
            &["reduce"],
        ),
    ];
    for (name, source, entries) in modules {
        let module = wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{name}:\n{}", e.emit_to_string(&source)));
        // No optional feature is requested from the device (`Processor::new` and the
        // UI's device use the defaults), so the shaders must validate without any.
        Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
        let found: Vec<&str> = module
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        for entry in entries {
            assert!(
                found.contains(entry),
                "{name} has no entry point {entry}: {found:?}"
            );
        }
    }
}
#[test]
fn previews_submit_only_once_the_window_surface_is_reconfigured() {
    let surface = reconfiguring_surface();
    let (tx, rx) = mpsc::channel();
    let preview = std::thread::spawn(move || submitting(|| tx.send(()).unwrap()));
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    drop(surface);
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    preview.join().unwrap();
}
