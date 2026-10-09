//! Read-only preview timing harness: cargo run --release --example preview_benchmark -- PHOTO [ITERATIONS]
//!
//! Times what the desktop does: the first Fit after opening, Fit renders while a
//! slider moves, a 100% region, and the full-resolution render used for export.
use anyhow::{Context, Result};
use rawmakase::model::recipe::Recipe;
use rawmakase::{camera_data, develop};
use std::{sync::atomic::AtomicBool, time::Instant};

/// Fit size of a 1600-pixel viewport, the size used by earlier measurements.
const FIT: u32 = 1600;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.
}
fn mean_error(a: &rawmakase::rendered::Rendered, b: &rawmakase::rendered::Rendered) -> f32 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let d: f32 = a
        .pixels
        .iter()
        .flatten()
        .zip(b.pixels.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .sum();
    d / (a.pixels.len() * 3) as f32
}
/// What the desktop does with a CPU-side render before it can be drawn: 8-bit
/// conversion and histogram on the render worker, then on the UI thread the texture
/// image and, for whole-photo views, the Navigator thumbnail.
fn cpu_display(out: &rawmakase::rendered::Rendered, navigator: bool) -> usize {
    let rgb = out.rgb8();
    let histogram = out.histogram();
    let image = eframe::egui::ColorImage::from_rgb([out.width as usize, out.height as usize], &rgb);
    let mut n = image.pixels.len() + histogram.bins[0][0] as usize;
    if navigator && let Some(full) = image::RgbImage::from_raw(out.width, out.height, rgb) {
        let scale = (360. / out.width.max(out.height) as f32).min(1.);
        let small = image::imageops::thumbnail(
            &full,
            ((out.width as f32 * scale) as u32).max(1),
            ((out.height as f32 * scale) as u32).max(1),
        );
        n += small.len();
    }
    n
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let path = args.get(1).context("Supply a RAW path")?;
    let iterations: usize = args.get(2).map_or(Ok(3), |n| n.parse())?;
    anyhow::ensure!((1..=100).contains(&iterations), "Use 1–100 iterations");
    let cancel = AtomicBool::new(false);
    let start = Instant::now();
    let image = rawmakase::photo::open(std::path::Path::new(path))?
        .develop(camera_data::Decode::full(Default::default()), &cancel)?;
    println!(
        "Decode {}x{}: {:.1} ms",
        image.width,
        image.height,
        ms(start)
    );
    let (profiles, _) = rawmakase::camera_profiles::installed(&image.metadata);
    let recipe = Recipe::with_profiles(&image.metadata, &profiles);
    println!(
        "Camera: {} {}; profile: {:?}",
        image.metadata.make,
        image.metadata.model,
        recipe.profile.as_ref().map(|p| &p.name)
    );
    // Decode cache: store with recovered highlights, then open from it, in a temporary
    // directory that is removed afterwards.
    let t = Instant::now();
    develop::quality::recover_highlights(&image);
    println!("Highlight recovery: {:.1} ms", ms(t));
    {
        let dir = tempfile::tempdir()?;
        let cache = rawmakase::decode_cache::DecodeCache::new(dir.path().to_owned(), u64::MAX);
        let cached = image.clone();
        let _ = cached
            .recovered
            .set(std::sync::Arc::new(develop::quality::recover_highlights(
                &image,
            )));
        let t = Instant::now();
        cache.store("benchmark", &cached)?;
        println!("Decode cache store: {:.1} ms", ms(t));
        let t = Instant::now();
        let loaded = cache
            .load("benchmark", &image.metadata)
            .context("Decode cache miss")?;
        println!("Open from decode cache: {:.1} ms", ms(t));
        anyhow::ensure!(loaded.pixels == image.pixels, "Decode cache changed pixels");
    }
    let mut gpu = develop::PreviewRenderer::with_gpu();
    println!(
        "GPU: {:?}; fallback: {:?}",
        gpu.adapter_name(),
        gpu.fallback_reason()
    );
    let mut cpu = develop::PreviewRenderer::default();
    let image = std::sync::Arc::new(image);
    // First Fit after opening: highlight recovery and any per-photo preparation.
    let t = Instant::now();
    gpu.render(&image, &recipe, FIT, None, &cancel)?;
    println!("First Fit after open: {:.1} ms", ms(t));
    let g = develop::Geometry::new(&image, &recipe, 0);
    let (rw, rh) = (1600.min(g.width), 1000.min(g.height));
    let region = [(g.width - rw) / 2, (g.height - rh) / 2, rw, rh];
    for local in [false, true] {
        let mut recipe = recipe.clone();
        if local {
            recipe.shadows = 0.4;
            recipe.highlights = -0.3;
            recipe.effects.clarity = 0.2;
        }
        recipe.exposure = iterations as f32 * 0.1;
        let t = Instant::now();
        let full =
            develop::quality::render_cancellable(&image, &recipe.checked()?, 0, None, &cancel)?;
        println!("Export resolution, local={local}: {:.1} ms", ms(t));
        let reference = develop::quality::resize(full, FIT);
        let modes: &[&str] = if local {
            &[
                "cpu-fit",
                "gpu-fit",
                "gpu-region",
                "region-preview",
                "clarity-fit",
                "clarity-region",
                "clarity-region-preview",
            ]
        } else {
            // Clarity alone: no Shadows/Highlights map to feed.
            &[
                "cpu-fit",
                "gpu-fit",
                "gpu-region",
                "region-preview",
                "clarity-region",
            ]
        };
        if local {
            // Switching between Fit and 100% with Clarity on, editing exposure in each:
            // Fit renders a pyramid level, 100% the photo, both kept on the device.
            let mut times = Vec::new();
            let mut recipe = recipe.clone();
            for i in 0..=2 * iterations {
                recipe.exposure = i as f32 * 0.05;
                let slot = if i % 2 == 0 {
                    develop::gpu::Slot::Whole
                } else {
                    develop::gpu::Slot::Region
                };
                let display = develop::gpu::Display {
                    slot,
                    clipping: rawmakase::rendered::ClipOverlay::NONE,
                    monitor: None,
                    navigator: None,
                    thumbnail: None,
                    samples: false,
                    drawn: Vec::new(),
                };
                let t = Instant::now();
                let (edge, at) = if i % 2 == 0 {
                    (FIT, None)
                } else {
                    (0, Some(region))
                };
                gpu.render_to(&image, &recipe, edge, at, &cancel, Some(&display))?;
                if i > 1 {
                    times.push(ms(t));
                }
            }
            times.sort_by(f64::total_cmp);
            println!(
                "switch views, local=true: median {:.1} ms, max {:.1} ms",
                times[times.len() / 2],
                times[times.len() - 1]
            );
        }
        for &mode in modes {
            let mut times = Vec::new();
            let mut shown = Vec::new();
            let mut last = None;
            for i in 0..=iterations {
                // Exposure edits, or Clarity edits, which change the local-tone stage.
                if mode.starts_with("clarity") {
                    recipe.effects.clarity = 0.2 + (iterations - i) as f32 * 0.05;
                } else {
                    recipe.exposure = i as f32 * 0.1;
                }
                let t = Instant::now();
                let out = match mode {
                    "cpu-fit" => cpu.render(&image, &recipe, FIT, None, &cancel)?,
                    "gpu-fit" | "clarity-fit" => gpu.render(&image, &recipe, FIT, None, &cancel)?,
                    // The reduced 100% view shown while dragging, before the full region.
                    "region-preview" | "clarity-region-preview" => {
                        gpu.render_region_preview(&image, &recipe, region, &cancel)?
                    }
                    _ => gpu.render(&image, &recipe, 0, Some(region), &cancel)?,
                };
                let rendered = ms(t);
                std::hint::black_box(cpu_display(&out, !mode.contains("region")));
                if i > 0 {
                    times.push(rendered);
                    shown.push(ms(t));
                }
                last = Some(out);
            }
            // The desktop path: presented into a texture on the GPU, ready to draw once
            // the histogram is back.
            let mut presented = Vec::new();
            let before = recipe.clone();
            if mode != "cpu-fit" {
                let slot = if mode.contains("region") {
                    develop::gpu::Slot::Region
                } else {
                    develop::gpu::Slot::Whole
                };
                let display = develop::gpu::Display {
                    slot,
                    clipping: rawmakase::rendered::ClipOverlay::NONE,
                    monitor: None,
                    navigator: (slot == develop::gpu::Slot::Whole).then_some(360),
                    thumbnail: None,
                    samples: false,
                    drawn: Vec::new(),
                };
                for i in 0..=iterations {
                    if mode.starts_with("clarity") {
                        recipe.effects.clarity = 0.25 + (iterations - i) as f32 * 0.05;
                    } else {
                        recipe.exposure = i as f32 * 0.1 + 0.05;
                    }
                    let t = Instant::now();
                    let out = match mode {
                        "region-preview" | "clarity-region-preview" => gpu
                            .render_region_preview_to(
                                &image,
                                &recipe,
                                region,
                                &cancel,
                                Some(&display),
                            )?,
                        "gpu-region" | "clarity-region" => gpu.render_to(
                            &image,
                            &recipe,
                            0,
                            Some(region),
                            &cancel,
                            Some(&display),
                        )?,
                        _ => gpu.render_to(&image, &recipe, FIT, None, &cancel, Some(&display))?,
                    };
                    if i > 0 {
                        presented.push(ms(t));
                    }
                    if let develop::quality::Output::Pixels(out) = out {
                        std::hint::black_box(cpu_display(&out, slot == develop::gpu::Slot::Whole));
                        println!("  ({mode} fell back to CPU pixels)");
                    }
                }
                presented.sort_by(f64::total_cmp);
            }
            recipe = before;
            times.sort_by(f64::total_cmp);
            shown.sort_by(f64::total_cmp);
            let last = last.unwrap();
            let error = if mode.contains("region") {
                String::new()
            } else {
                let e = mean_error(&last, &reference);
                anyhow::ensure!(e < 0.01, "{mode} differs from the export render: {e}");
                format!(", mean error vs export {e:.5}")
            };
            println!(
                "{mode}, local={local}: median {:.1} ms, max {:.1} ms, ready to draw {:.1} ms, presented {}{error}",
                times[times.len() / 2],
                times[times.len() - 1],
                shown[shown.len() / 2],
                presented
                    .get(presented.len() / 2)
                    .map_or("-".into(), |t| format!("{t:.1} ms"))
            );
        }
    }
    Ok(())
}
