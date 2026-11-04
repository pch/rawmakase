//! `rawmakase denoise`, the development command: AI Denoise on one photo, timed and
//! measured.
use super::*;
use rawmakase_cancel::Cancel;

/// Options of `rawmakase denoise`, the development command.
pub struct Command {
    pub input: PathBuf,
    /// The model's folder; the installed model by default.
    pub model: Option<PathBuf>,
    pub provider: Provider,
    /// AI Denoise's Amount for `output` (0–1).
    pub amount: f32,
    /// A JPEG or PNG of the photo with its default settings and AI Denoise at `amount`.
    pub output: Option<PathBuf>,
    /// The same without AI Denoise.
    pub plain_output: Option<PathBuf>,
    pub max_edge: u32,
    /// The model's output with the gain applied, three little-endian f32 planes at the
    /// padded sensor size, and a JSON beside it with the layout and gain.
    pub dump: Option<PathBuf>,
    /// The user's AI Denoise cache (`rawmakase::app::user_ai_denoise_cache`), or
    /// `None` for a throwaway one.
    pub cache: Option<AiCache>,
    /// Time Develop's renders while Amount moves: a Fit view of this long edge and a
    /// 100% region of 1600 × 1000.
    pub bench_fit: Option<u32>,
    /// Writes centre crops of `camera_crop` pixels of the camera image (white balanced
    /// camera RGB, highlights recovered) here: `plain.f32`, `ai.f32` and `nr<L>.f32` for
    /// each Luminance noise reduction amount in `nr_luma` (Color at its default), with
    /// `crop.json`. Phase 0's quality measures read them.
    pub camera_dump: Option<PathBuf>,
    pub camera_crop: u32,
    pub nr_luma: Vec<f32>,
    /// With `bench_fit`: the same renders without AI Denoise (Exposure moving instead),
    /// for the memory and time baseline; nothing is denoised.
    pub bench_plain: bool,
}

/// The process's resident memory in MB, as `ps` reports it (development command only).
fn resident_mb() -> f64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map_or(0., |kb| kb / 1024.)
}

/// Samples the resident memory until dropped, keeping the peak.
struct Peak {
    peak: Arc<std::sync::atomic::AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Peak {
    fn start() -> Self {
        let peak = Arc::new(std::sync::atomic::AtomicU64::new(resident_mb() as u64));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, s) = (peak.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                p.fetch_max(resident_mb() as u64, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            peak,
            stop,
            thread: Some(thread),
        }
    }
    fn finish(mut self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.peak.load(Ordering::Relaxed).max(resident_mb() as u64)
    }
}

/// Runs AI Denoise on one photo and reports times and the fit as JSON.
pub fn command(c: Command) -> Result<()> {
    use std::time::Instant;
    let started = Instant::now();
    // Read before decoding: the result must be of the same file.
    let file = crate::storage::Identity::read(&c.input)?;
    let plain = Arc::new(crate::photo::open(&c.input)?.develop(
        crate::camera_data::Decode::full(Default::default()),
        Cancel::NEVER,
    )?);
    let decoded = started.elapsed();
    if c.bench_plain {
        let recipe = crate::model::recipe::Recipe::for_metadata(&plain.metadata);
        let before = resident_mb();
        let peak = Peak::start();
        bench(&plain, None, &recipe, c.bench_fit.unwrap_or(2000))?;
        println!(
            "{}",
            serde_json::json!({"resident_before_mb": before, "drag_peak_mb": peak.finish()})
        );
        return Ok(());
    }
    let before_job = resident_mb();
    let job_peak = Peak::start();
    let scratch = tempfile::tempdir()?;
    let cache = match c.cache {
        Some(cache) => cache,
        None => AiCache::new(scratch.path().to_path_buf(), 1 << 40),
    };
    let service = Service::with(
        cache,
        c.provider,
        Arc::new(open_mosaic),
        Arc::new(load_model),
    );
    let model = c
        .model
        .clone()
        .unwrap_or_else(|| crate::model_install::Pack::Denoise.installed_dir());
    service.set_model(Some(model));
    let t = Instant::now();
    let entry = service.entry(&c.input, &file, Cancel::NEVER)?;
    let inferred = t.elapsed();
    let t = Instant::now();
    let image = assemble(
        &ModelOutput {
            layout: entry.layout,
            rgb: &entry.rgb,
            done: None,
            clipped: &entry.clipped,
        },
        &plain,
        Cancel::NEVER,
    )?;
    let source = AiSource::recover(image, Cancel::NEVER)?;
    let assembled = t.elapsed();
    // The worker and its ONNX session gone, so what remains is what Develop keeps.
    wait_within(service.stop(), Duration::from_secs(10));
    let job_peak = job_peak.finish();
    if let Some(dump) = &c.dump {
        let bytes: Vec<u8> = entry.rgb.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(dump, bytes)?;
        let packed = open_mosaic(&c.input, Cancel::NEVER)?;
        let planes: Vec<u8> = packed.planes.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(dump.with_extension("planes"), planes)?;
        std::fs::write(
            dump.with_extension("json"),
            serde_json::json!({
                "width": entry.layout.width,
                "height": entry.layout.height,
                "pad": entry.layout.pad,
                "packed": [entry.layout.packed_width, entry.layout.packed_height],
                "gain": entry.gain,
            })
            .to_string(),
        )?;
    }
    // A raw file's defaults, as the app opens it (Color noise reduction 25, as Lightroom).
    let mut recipe = crate::model::recipe::Recipe::for_metadata(&plain.metadata);
    recipe.set_color_noise_defaults();
    if let Some(dir) = &c.camera_dump {
        camera_dump(dir, &plain, &source, &recipe, c.camera_crop, &c.nr_luma)?;
    }
    if let Some(fit) = c.bench_fit {
        let entry_kept = Arc::strong_count(&entry);
        let held = (entry, service);
        drop(held);
        let after_job = resident_mb();
        let peak = Peak::start();
        bench(&plain, Some(&source), &recipe, fit)?;
        println!(
            "{}",
            serde_json::json!({
                "resident_before_job_mb": before_job,
                "job_peak_mb": job_peak,
                "resident_after_job_mb": after_job,
                "drag_peak_mb": peak.finish(),
                "entry_refs": entry_kept,
            })
        );
        return Ok(());
    }
    let save =
        |path: &Path, ai: Option<&AiSource>, recipe: &crate::model::recipe::Recipe| -> Result<()> {
            let out = crate::develop::render_with_ai(
                &plain,
                &recipe.checked()?,
                ai,
                c.max_edge,
                Cancel::NEVER,
            )?;
            image::RgbImage::from_raw(out.width, out.height, out.rgb8())
                .ok_or_else(|| anyhow!("bad render"))?
                .save(path)?;
            Ok(())
        };
    if let Some(path) = &c.plain_output {
        save(path, None, &recipe)?;
    }
    if let Some(path) = &c.output {
        // Switched on as the panel switches it (manual noise reduction to 0).
        recipe.switch_ai_denoise(true);
        recipe.ai_denoise.amount = c.amount;
        save(path, Some(&source), &recipe)?;
    }
    println!(
        "{}",
        serde_json::json!({
            "file": c.input,
            "provider": c.provider.name(),
            "width": plain.width,
            "height": plain.height,
            "decode_s": decoded.as_secs_f64(),
            "denoise_s": inferred.as_secs_f64(),
            "assemble_recover_s": assembled.as_secs_f64(),
            "gain": entry.gain,
        })
    );
    Ok(())
}

/// Centre crops of the camera image without noise reduction, with AI Denoise, and with
/// each Luminance amount (see [`Command::camera_dump`]).
fn camera_dump(
    dir: &Path,
    plain: &Arc<CameraImage>,
    source: &AiSource,
    recipe: &crate::model::recipe::Recipe,
    side: u32,
    nr_luma: &[f32],
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let (w, h) = (side.min(plain.width), side.min(plain.height));
    let (x0, y0) = ((plain.width - w) / 2, (plain.height - h) / 2);
    let cancel = AtomicBool::new(false);
    let write = |name: &str, image: &CameraImage| -> Result<()> {
        let mut bytes = Vec::with_capacity((w * h * 12) as usize);
        for y in y0..y0 + h {
            let row = (y * image.width) as usize;
            for p in &image.pixels[row + x0 as usize..row + (x0 + w) as usize] {
                for v in p {
                    bytes.extend(v.to_le_bytes());
                }
            }
        }
        std::fs::write(dir.join(format!("{name}.f32")), bytes)?;
        Ok(())
    };
    let mut off = recipe.clone();
    off.noise_luma = 0.;
    off.noise_chroma = 0.;
    write(
        "plain",
        &*crate::develop::quality::denoised_camera(plain, &off, None, Cancel::new(&cancel))?,
    )?;
    let mut ai = off;
    ai.ai_denoise = crate::model::ai_denoise::AiDenoise {
        enabled: true,
        amount: 1.,
    };
    write(
        "ai",
        &*crate::develop::quality::denoised_camera(plain, &ai, Some(source), Cancel::new(&cancel))?,
    )?;
    for luma in nr_luma {
        let mut nr = recipe.clone();
        nr.noise_luma = *luma;
        write(
            &format!("nr{:.0}", luma * 100.),
            &*crate::develop::quality::denoised_camera(plain, &nr, None, Cancel::new(&cancel))?,
        )?;
    }
    std::fs::write(
        dir.join("crop.json"),
        serde_json::json!({
            "x": x0, "y": y0, "width": w, "height": h,
            "wb": plain.metadata.wb, "matrix": plain.metadata.matrix,
            "cam_xyz": plain.metadata.cam_xyz, "chroma_default": recipe.noise_chroma,
        })
        .to_string(),
    )?;
    Ok(())
}

/// Develop's renders while AI Denoise's Amount moves, as the preview renderer runs them.
fn bench(
    plain: &Arc<CameraImage>,
    source: Option<&AiSource>,
    recipe: &crate::model::recipe::Recipe,
    fit: u32,
) -> Result<()> {
    use std::time::Instant;
    let mut renderer = if std::env::var_os("RAWMAKASE_BENCH_CPU").is_some() {
        crate::develop::PreviewRenderer::with_processor(Err(anyhow!("CPU bench")))
    } else {
        crate::develop::PreviewRenderer::with_gpu()
    };
    renderer.set_ai(source.cloned());
    let mut recipe = recipe.clone();
    recipe.ai_denoise.enabled = source.is_some();
    let cancel = AtomicBool::new(false);
    let region = [
        plain.width / 2 - 800,
        plain.height / 2 - 500,
        1600.min(plain.width),
        1000.min(plain.height),
    ];
    let mut report = Vec::new();
    for (label, region, edge) in [("fit", None, fit), ("region", Some(region), 0)] {
        let mut times = Vec::new();
        for (i, amount) in [0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8]
            .into_iter()
            .enumerate()
        {
            match source {
                Some(_) => recipe.ai_denoise.amount = amount,
                None => recipe.exposure = amount - 0.5,
            }
            let t = Instant::now();
            let _ = renderer.render(plain, &recipe, edge, region, Cancel::new(&cancel))?;
            if i > 0 {
                times.push(t.elapsed().as_secs_f64() * 1000.);
            }
        }
        times.sort_by(f64::total_cmp);
        report.push(serde_json::json!({
            "view": label,
            "gpu": renderer.used_gpu(),
            "median_ms": times[times.len() / 2],
            "max_ms": times[times.len() - 1],
        }));
    }
    println!("{}", serde_json::Value::Array(report));
    Ok(())
}
