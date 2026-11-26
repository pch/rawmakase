//! Read-only timing of the Presets panel's tiles:
//! cargo run --release --example preset_thumbnail_benchmark -- PHOTO [--installed] [--side PX]
//!
//! Each preset is applied to the photo as a tile applies it (with Auto measured
//! on the decoded photo) and rendered by a `ThumbnailRenderer`, on a low-priority
//! pool of two threads, as the app's worker does. `--installed` adds the presets
//! installed in the library to the built-in ones. It then times Develop's Fit,
//! rendered on the GPU while a slider moves, alone and while the tiles render.
use anyhow::{Context, Result};
use rawmakase::develop::{self, thumbnails::ThumbnailRenderer};
use rawmakase::model::recipe::Recipe;
use rawmakase_cancel::Cancel;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// A 2 × 145-point tile on a 2× display.
const SIDE: u32 = 290;
/// Fit size of a 1600-pixel viewport, as `preview_benchmark` measures.
const FIT: u32 = 1600;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.
}

/// Whether applying `p` measures anything on the photo: Auto tone, Auto white
/// balance or the Auto black & white mix.
fn is_auto(p: &rawmakase::xmp::Preset) -> bool {
    let is = |k: &str, v: &str| p.settings.get(k).is_some_and(|s| s.eq_ignore_ascii_case(v));
    is("AutoTone", "True") || is("WhiteBalance", "Auto") || is("AutoGrayscaleMix", "True")
}

struct Summary {
    label: &'static str,
    times: Vec<f64>,
}
impl Summary {
    fn print(mut self) {
        if self.times.is_empty() {
            println!("{:>28}: none", self.label);
            return;
        }
        self.times.sort_by(f64::total_cmp);
        let n = self.times.len();
        let mean = self.times.iter().sum::<f64>() / n as f64;
        let at = |q: f64| self.times[((n - 1) as f64 * q).round() as usize];
        println!(
            "{:>28}: n {n:4}, mean {mean:7.1} ms, median {:7.1}, p95 {:7.1}, max {:7.1}",
            self.label,
            at(0.5),
            at(0.95),
            at(1.)
        );
    }
}

/// Develop's Fit of `base` rendered `n` times on the GPU, each at another
/// Exposure as a slider drag renders it.
fn fit_latency(
    renderer: &mut develop::PreviewRenderer,
    image: &rawmakase::camera_data::CameraImage,
    base: &Recipe,
    n: usize,
) -> Result<Vec<f64>> {
    let mut times = Vec::new();
    for i in 0..n {
        let r = Recipe {
            exposure: base.exposure + 0.01 * (i + 1) as f32,
            ..base.clone()
        };
        let t = Instant::now();
        renderer.render(image, &r, FIT, None, Cancel::NEVER)?;
        times.push(ms(t));
    }
    Ok(times)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let path = args.get(1).context("Supply a RAW path")?;
    let installed = args.iter().any(|a| a == "--installed");
    let side: u32 = match args.iter().position(|a| a == "--side") {
        Some(i) => args.get(i + 1).context("--side PX")?.parse()?,
        None => SIDE,
    };
    let t = Instant::now();
    let image = Arc::new(
        rawmakase::photo::open(
            std::path::Path::new(path),
            &rawmakase::photo::LensProfiles::none(),
        )?
        .develop(
            rawmakase::camera_data::Decode::full(Default::default()),
            Cancel::NEVER,
        )?,
    );
    println!(
        "Decode {}x{} {} {}: {:.0} ms",
        image.width,
        image.height,
        image.metadata.make,
        image.metadata.model,
        ms(t)
    );
    let (profiles, _) =
        rawmakase::camera_profiles::ProfileLibrary::for_tests().installed(&image.metadata);
    let base = Recipe::with_profiles(&image.metadata, &profiles);
    let library = if installed {
        rawmakase::presets::PresetLibrary::in_data_dirs(&rawmakase::storage::asset_dirs()).load()
    } else {
        let (presets, errors) = rawmakase::presets::builtin::presets();
        rawmakase::presets::Library { presets, errors }
    };
    let auto = library.presets.iter().filter(|p| is_auto(p)).count();
    println!(
        "{} presets ({auto} with Auto), tiles {side} px square",
        library.presets.len()
    );

    let global = args.iter().any(|a| a == "--global");
    let pool = Arc::new(match global {
        true => rayon::ThreadPoolBuilder::new().build()?,
        false => rawmakase::raw::background_pool(2, "preset-thumbnails")?,
    });
    let measures_image = image.clone();
    let mut tiles = Tiles {
        image: image.clone(),
        renderer: ThumbnailRenderer::default(),
        pool,
        side,
    };
    // The tiles alone: the first pass makes the reduced copy and meets each
    // preset's settings for the first time; the second renders them again.
    let measures = develop::Measures(&measures_image);
    let t = Instant::now();
    let (mut apply, mut apply_auto, mut render, mut first) =
        (Vec::new(), Vec::new(), Vec::new(), None);
    let mut failed = 0;
    let mut recipes = Vec::new();
    for p in &library.presets {
        let t = Instant::now();
        let Ok(applied) = rawmakase::presets::apply_to_photo(
            p,
            &base,
            &image.metadata,
            &profiles,
            Some(&measures),
        ) else {
            failed += 1;
            continue;
        };
        ThumbnailRenderer::prime(&image, &applied.recipe);
        match is_auto(p) {
            true => apply_auto.push(ms(t)),
            false => apply.push(ms(t)),
        }
        let rendered = tiles.render(&applied.recipe)?;
        match first {
            None => first = Some(rendered),
            Some(_) => render.push(rendered),
        }
        recipes.push(applied.recipe);
    }
    println!(
        "All {} tiles once on {}: {:.0} ms ({failed} not applied)",
        library.presets.len(),
        if global {
            "the shared pool"
        } else {
            "two background threads"
        },
        ms(t)
    );
    println!(
        "First tile (reduced copy made, highlights recovered): {:.0} ms",
        first.unwrap_or(0.)
    );
    let mut again = Vec::new();
    for r in &recipes {
        again.push(tiles.render(r)?);
    }
    Summary {
        label: "apply, no Auto",
        times: apply,
    }
    .print();
    Summary {
        label: "apply with Auto",
        times: apply_auto,
    }
    .print();
    Summary {
        label: "render, first pass",
        times: render,
    }
    .print();
    Summary {
        label: "render, second pass",
        times: again,
    }
    .print();

    // The main view as Develop opens it: the GPU renderer, its pyramid built.
    let mut main = develop::PreviewRenderer::with_gpu();
    let t = Instant::now();
    main.render(&image, &base, FIT, None, Cancel::NEVER)?;
    println!(
        "First Fit {FIT} on {}: {:.0} ms",
        main.adapter_name().unwrap_or("the CPU"),
        ms(t)
    );
    let idle = fit_latency(&mut main, &image, &base, 20)?;
    // Tiles rendered over and over, from a fresh renderer, while the main view
    // renders: the worst case of a scrolled grid.
    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let stop = stop.clone();
        let mut tiles = Tiles {
            renderer: ThumbnailRenderer::default(),
            ..tiles
        };
        std::thread::spawn(move || -> Result<usize> {
            let mut n = 0;
            while !stop.load(Ordering::Relaxed) {
                for r in &recipes {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    tiles.render(r)?;
                    n += 1;
                }
            }
            Ok(n)
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(100));
    let busy = fit_latency(&mut main, &image, &base, 20)?;
    stop.store(true, Ordering::Relaxed);
    let rendered = worker
        .join()
        .map_err(|_| anyhow::anyhow!("the tiles panicked"))??;
    Summary {
        label: "main Fit, idle",
        times: idle,
    }
    .print();
    Summary {
        label: "main Fit, tiles rendering",
        times: busy,
    }
    .print();
    println!("({rendered} tiles rendered meanwhile)");
    Ok(())
}

/// A tile renderer on its pool, as the worker renders.
struct Tiles {
    image: Arc<rawmakase::camera_data::CameraImage>,
    renderer: ThumbnailRenderer,
    pool: Arc<rayon::ThreadPool>,
    side: u32,
}
impl Tiles {
    /// Renders `recipe`'s tile, and how long that took in milliseconds.
    fn render(&mut self, recipe: &Recipe) -> Result<f64> {
        let t = Instant::now();
        let (renderer, image, side) = (&mut self.renderer, &self.image, self.side);
        self.pool
            .install(|| renderer.render(image, recipe, side, Cancel::NEVER))?;
        Ok(ms(t))
    }
}
