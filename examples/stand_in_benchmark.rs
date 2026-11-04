//! Read-only timing of what opening a photo shows first (issue #213):
//! cargo run --release --example stand_in_benchmark -- PHOTO
//!
//! Times the camera's embedded JPEG, a stored Standard preview read back, and the
//! first live Fit (from the decode cache and from a decode), and how far the stored
//! preview is from the live Fit it hands over to. The preview cache is a temporary
//! one, removed afterwards; the photo is only read.
use anyhow::{Context, Result};
use rawmakase::catalog::preview_cache::{PreviewCache, PreviewKind, Stamp};
use rawmakase::model::recipe::Recipe;
use rawmakase::{camera_data, develop};
use rawmakase_cancel::Cancel;
use std::{path::Path, sync::atomic::AtomicBool, time::Instant};

/// The Fit of a 1600-pixel viewport, as in `preview_benchmark`.
const FIT: u32 = 1600;
/// The default Standard Preview Size.
const STANDARD: u32 = 2048;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.
}

fn main() -> Result<()> {
    let path = std::env::args().nth(1).context("Supply a RAW path")?;
    let path = Path::new(&path);
    let cancel = AtomicBool::new(false);

    // What the loader shows first today: the camera's JPEG, at most 2560 px.
    let t = Instant::now();
    let mut raw = rawmakase::photo::open(path)?;
    let embedded = rawmakase::raw::thumbnail(&mut raw)?;
    println!(
        "Embedded JPEG {}x{}: {:.1} ms",
        embedded.width(),
        embedded.height(),
        ms(t)
    );

    // Building the Standard preview, as Build Standard-Sized Previews does for a
    // photo with the default edit: the half-size decode when it is large enough.
    let (profiles, _) = rawmakase::camera_profiles::installed(&raw.metadata);
    let recipe = Recipe::with_profiles(&raw.metadata, &profiles);
    let g = develop::Geometry::for_metadata(&raw.metadata, &recipe);
    // FULL=1 builds from the full decode, to tell its share of the handoff difference.
    let half = g.width.max(g.height) / 2 >= STANDARD
        && std::env::var_os("FULL").is_none_or(|v| v.is_empty());
    let t = Instant::now();
    let decoded = if half {
        raw.develop(camera_data::Decode::Half, Cancel::new(&cancel))?
    } else {
        raw.develop(
            camera_data::Decode::full(Default::default()),
            Cancel::new(&cancel),
        )?
    };
    let out = develop::render(&decoded, &recipe.checked()?, STANDARD, Cancel::new(&cancel))?;
    let standard =
        image::RgbImage::from_raw(out.width, out.height, out.rgb8()).context("Preview size")?;
    let dir = tempfile::tempdir()?;
    let cache_path = dir.path().join("previews.sqlite3");
    let mut cache = PreviewCache::open(&cache_path)?;
    cache.store_sized(
        path,
        "benchmark",
        PreviewKind::Standard,
        &Stamp::read(path)?,
        STANDARD,
        &standard,
    )?;
    println!(
        "Standard preview build ({} decode) {}x{}: {:.1} ms",
        if half { "half-size" } else { "full" },
        standard.width(),
        standard.height(),
        ms(t)
    );
    drop(cache);

    // What Develop shows first with it: the row read, decoded and made a texture
    // image, from a cache opened anew (as the reader's first use does).
    let t = Instant::now();
    let cache = PreviewCache::open(&cache_path)?;
    let stored = cache
        .load_sized(path, "benchmark", PreviewKind::Standard)?
        .context("Stored preview missing")?;
    let size = [stored.width() as usize, stored.height() as usize];
    let pixels = eframe::egui::ColorImage::from_rgb(size, stored.as_raw())
        .pixels
        .len();
    println!("Stored preview read: {:.1} ms ({pixels} pixels)", ms(t));

    // The first live Fit: from the decode cache, and from a decode.
    let t = Instant::now();
    let full = rawmakase::photo::open(path)?.develop(
        camera_data::Decode::full(Default::default()),
        Cancel::new(&cancel),
    )?;
    let decode = ms(t);
    let recovered = develop::quality::recover_highlights(&full, Cancel::NEVER)?;
    let _ = full.recovered.set(std::sync::Arc::new(recovered));
    let cached = {
        let decode_cache =
            rawmakase::decode_cache::DecodeCache::new(dir.path().join("decoded"), u64::MAX);
        decode_cache.store("benchmark", &full)?;
        let t = Instant::now();
        let loaded = decode_cache
            .load("benchmark", &full.metadata)
            .context("Decode cache miss")?;
        (loaded, ms(t))
    };
    let image = std::sync::Arc::new(cached.0);
    let mut gpu = develop::PreviewRenderer::with_gpu();
    // BUSY=1 keeps a build running meanwhile, as a background build would.
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let busy = std::env::var_os("BUSY").filter(|v| !v.is_empty()).map(|_| {
        let (stop, path, recipe) = (stop.clone(), path.to_owned(), recipe.clone());
        rawmakase::raw::spawn_background(move || {
            let pool = rawmakase::raw::background_pool(2, "preview-build").unwrap();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                pool.install(|| {
                    let raw = rawmakase::photo::open(&path).unwrap();
                    let im = raw
                        .develop(camera_data::Decode::Half, Cancel::new(&stop))
                        .ok()?;
                    develop::render(&im, &recipe.checked().ok()?, STANDARD, Cancel::new(&stop)).ok()
                });
            }
        })
    });
    if busy.is_some() {
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let t = Instant::now();
    let fit = gpu.render(&image, &recipe, FIT, None, Cancel::new(&cancel))?;
    let first_fit = ms(t);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(busy) = busy {
        let _ = busy.join();
    }
    println!(
        "First live Fit: {:.1} ms from the decode cache ({:.1} ms read), {:.1} ms after a decode ({:.1} ms decode)",
        cached.1 + first_fit,
        cached.1,
        decode + first_fit,
        decode
    );

    // The handoff: the stored preview as the viewport scales it, against the Fit.
    let shown = image::imageops::resize(
        &stored,
        fit.width,
        fit.height,
        image::imageops::FilterType::Triangle,
    );
    let live = fit.rgb8();
    let total: f64 = shown
        .as_raw()
        .iter()
        .zip(&live)
        .map(|(a, b)| (*a as f64 - *b as f64).abs())
        .sum();
    let worst = shown
        .as_raw()
        .iter()
        .zip(&live)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    println!(
        "Handoff {}x{}: mean difference {:.4}, largest {:.3} (0–1 scale, 8-bit sRGB)",
        fit.width,
        fit.height,
        total / live.len() as f64 / 255.,
        worst as f64 / 255.
    );
    Ok(())
}
