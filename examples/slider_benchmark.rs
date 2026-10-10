//! Read-only slider timing harness: cargo run --release --example slider_benchmark -- PHOTO [STEPS] [FIT_EDGE]
//!
//! Times what the desktop does while a Basic panel slider is dragged at Fit: a full
//! render presented on the GPU for each new value, and the reduced draft shown first on
//! slow GPUs, from the recipe a new photo opens with plus Contrast −76 and Highlights −3
//! (an edit that measures the Contrast pivot and builds the Shadows/Highlights map).
//! `FIT_EDGE` is the Fit's long edge in pixels (default 1402, a 2× HiDPI laptop view).
use anyhow::{Context, Result};
use rawmakase::model::recipe::Recipe;
use rawmakase::{camera_data, develop};
use std::{sync::atomic::AtomicBool, time::Instant};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.
}

/// One slider: its name, how to set it, and the values a drag passes through.
struct Slider {
    name: &'static str,
    set: fn(&mut Recipe, f32),
    from: f32,
    to: f32,
}

const SLIDERS: [Slider; 6] = [
    Slider {
        name: "Exposure",
        set: |r, v| r.exposure = v,
        from: -0.5,
        to: 0.5,
    },
    Slider {
        name: "Contrast",
        set: |r, v| r.contrast = v,
        from: -0.76,
        to: 0.2,
    },
    Slider {
        name: "Highlights",
        set: |r, v| r.highlights = v,
        from: -0.03,
        to: -0.6,
    },
    Slider {
        name: "Shadows",
        set: |r, v| r.shadows = v,
        from: 0.,
        to: 0.6,
    },
    Slider {
        name: "Whites",
        set: |r, v| r.whites = v,
        from: 0.,
        to: 0.5,
    },
    Slider {
        name: "Blacks",
        set: |r, v| r.blacks = v,
        from: 0.,
        to: -0.5,
    },
];

/// Minimum and median: the minimum is the work itself, the median adds whatever else
/// the machine was doing.
fn min_median(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2])
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let path = args.get(1).context("Supply a RAW path")?;
    let steps: usize = args.get(2).map_or(Ok(12), |n| n.parse())?;
    let fit: u32 = args.get(3).map_or(Ok(1402), |n| n.parse())?;
    anyhow::ensure!((2..=200).contains(&steps), "Use 2–200 steps");
    let cancel = AtomicBool::new(false);
    let image = rawmakase::photo::open(std::path::Path::new(path))?
        .develop(camera_data::Decode::full(Default::default()), &cancel)?;
    let (profiles, _) = rawmakase::camera_profiles::installed(&image.metadata);
    let mut recipe = Recipe::with_profiles(&image.metadata, &profiles);
    recipe.contrast = -0.76;
    recipe.highlights = -0.03;
    let image = std::sync::Arc::new(image);
    let mut gpu = develop::PreviewRenderer::with_gpu();
    println!(
        "{} {}, {}x{}, profile {:?}; GPU {:?}, fallback {:?}; Fit {fit} px",
        image.metadata.make,
        image.metadata.model,
        image.width,
        image.height,
        recipe.profile.as_ref().map(|p| &p.name),
        gpu.adapter_name(),
        gpu.fallback_reason()
    );
    let display = develop::gpu::Display {
        slot: develop::gpu::Slot::Whole,
        clipping: rawmakase::rendered::ClipOverlay::NONE,
        monitor: None,
        navigator: Some(360),
        thumbnail: None,
        samples: false,
        drawn: Vec::new(),
    };
    // Opening the photo: pyramid, device copy, caches.
    let t = Instant::now();
    gpu.render_to(&image, &recipe, fit, None, &cancel, Some(&display))?;
    println!("First Fit: {:.1} ms", ms(t));
    let draft = fit / 2;
    println!(
        "{:<11} {:>22} {:>22}",
        "per change", "full Fit (min/median)", "draft (min/median)"
    );
    for slider in &SLIDERS {
        let mut full = Vec::new();
        let mut drafts = Vec::new();
        let mut r = recipe.clone();
        for i in 0..=steps {
            let v = slider.from + (slider.to - slider.from) * i as f32 / steps as f32;
            (slider.set)(&mut r, v);
            let t = Instant::now();
            gpu.render_to(&image, &r, draft, None, &cancel, Some(&display))?;
            let d = ms(t);
            let t = Instant::now();
            let out = gpu.render_to(&image, &r, fit, None, &cancel, Some(&display))?;
            let f = ms(t);
            anyhow::ensure!(
                matches!(out, develop::quality::Output::Frame(_)),
                "{} fell back to CPU pixels",
                slider.name
            );
            // The first value only moves the slider off the previous one.
            if i > 0 {
                full.push(f);
                drafts.push(d);
            }
        }
        let (fm, fx) = min_median(full);
        let (dm, dx) = min_median(drafts);
        println!(
            "{:<11} {:>12.1} / {:>6.1} ms {:>12.1} / {:>6.1} ms",
            slider.name, fm, fx, dm, dx
        );
    }
    Ok(())
}
