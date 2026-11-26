use super::*;
use crate::raw_defaults::{DefaultChoice, RawDefaults};

const CHART: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/corpus/charts/synthetic-d65.dng"
);

/// A copy of the synthetic chart in `dir`, named `name`.
fn chart(dir: &Path, name: &str) -> PathBuf {
    let photo = dir.join(name);
    std::fs::copy(CHART, &photo).unwrap();
    photo
}

/// An XMP preset in `dir` that sets Exposure to `exposure`.
fn exposure_xmp(dir: &Path, name: &str, exposure: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal" crs:Version="15.0" crs:Exposure2012="{exposure}"><crs:Name><rdf:Alt><rdf:li xml:lang="x-default">{name}</rdf:li></rdf:Alt></crs:Name></rdf:Description></rdf:RDF></x:xmpmeta>"#
        ),
    )
    .unwrap();
    path
}

/// Raw defaults whose master is the preset at `preset`, which need not exist.
fn defaults(preset: &Path) -> DevelopDefaults {
    DevelopDefaults::load(RawDefaults {
        master: DefaultChoice::Preset {
            id: preset.to_string_lossy().into_owned(),
            name: "Brighter".into(),
        },
        ..Default::default()
    })
}

/// AI Denoise with a cache in `dir` and no model installed.
fn no_denoise(dir: &Path) -> impl Fn() -> CommandLine {
    let dir = dir.to_path_buf();
    move || {
        CommandLine::new(
            crate::ai_cache::AiCache::new(dir.join("ai"), 1 << 30),
            dir.join("model"),
            dir.join("runtime"),
        )
    }
}

/// What a command printed, and how it ended.
struct Run {
    result: Result<()>,
    out: String,
    err: String,
}

fn run(command: impl FnOnce(&mut Console<'_>) -> Result<()>) -> Run {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = command(&mut Console {
        out: &mut out,
        err: &mut err,
    });
    Run {
        result,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

/// `render` of `photo` to a small TIFF, saving the recipe it used: the Exposure it
/// rendered with, and what it printed.
fn exposure_rendered(
    photo: &Path,
    raw_defaults: &DevelopDefaults,
    change: impl FnOnce(&mut Render),
) -> (f32, Run) {
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = photo.parent().unwrap();
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let recipe = dir.join(format!("recipe-{n}.json"));
    let mut options = Render {
        input: photo.to_path_buf(),
        output: dir.join(format!("out-{n}.tif")),
        save_recipe: Some(recipe.clone()),
        max_edge: 64,
        ..Default::default()
    };
    change(&mut options);
    let ai = no_denoise(dir);
    let run = run(|console| {
        render(
            &options,
            &|| raw_defaults.clone(),
            &ai,
            Cancel::NEVER,
            console,
        )
    });
    assert!(run.result.is_ok(), "{:?}: {}", run.result, run.err);
    let exposure = crate::presets::load_preset(&recipe)
        .unwrap()
        .recipe
        .exposure;
    (exposure, run)
}

/// The edit `render` starts from: `--recipe`, else a legacy sidecar (not with
/// `--catalog`), else the catalog's edit or the raw defaults the caller passed;
/// then `--exposure`, and the `--xmp` preset with `--exposure` again over it.
#[test]
fn render_layers_its_edit_sources_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    let photo = chart(&photos, "chart.dng");
    let brighter = defaults(&exposure_xmp(dir.path(), "Brighter.xmp", "+0.70"));

    // Nothing but the raw defaults, which the caller passes, not the session's.
    assert_eq!(exposure_rendered(&photo, &brighter, |_| {}).0, 0.7);

    let catalog = dir.path().join("photos.rawmakase");
    let mut c = crate::catalog::Catalog::create(&catalog).unwrap();
    c.add_folder(&photos).unwrap();
    drop(c);

    // A legacy sidecar comes before them.
    let metadata = crate::photo::open(&photo).unwrap().metadata;
    let sidecar = Recipe {
        exposure: 0.3,
        ..Recipe::for_metadata(&metadata)
    };
    crate::catalog::legacy_sidecar::save(&photo, &sidecar, &ExportOptions::default()).unwrap();
    assert_eq!(exposure_rendered(&photo, &brighter, |_| {}).0, 0.3);

    // An explicit recipe before both.
    let explicit = dir.path().join("explicit.json");
    let darker = Recipe {
        exposure: -1.,
        ..Default::default()
    };
    crate::presets::save_preset(&explicit, &darker, &metadata).unwrap();
    let with_recipe = |o: &mut Render| o.recipe = Some(explicit.clone());
    assert_eq!(exposure_rendered(&photo, &brighter, with_recipe).0, -1.);

    // --exposure over whichever it is.
    let (exposure, _) = exposure_rendered(&photo, &brighter, |o| {
        with_recipe(o);
        o.exposure = Some(0.25);
    });
    assert_eq!(exposure, 0.25);

    // An XMP preset over the edit, and --exposure again over the preset.
    let xmp = exposure_xmp(dir.path(), "Bright.xmp", "+1.50");
    let with_xmp = |o: &mut Render| o.xmp = Some(xmp.clone());
    assert_eq!(exposure_rendered(&photo, &brighter, with_xmp).0, 1.5);
    let (exposure, _) = exposure_rendered(&photo, &brighter, |o| {
        with_xmp(o);
        o.exposure = Some(0.25);
    });
    assert_eq!(exposure, 0.25);

    // With a catalog the sidecar is not read: a photo the catalog has no edit of
    // starts from the raw defaults. (The catalog was made before the sidecar, which
    // adding the folder would have imported.)
    let (exposure, _) = exposure_rendered(&photo, &brighter, |o| o.catalog = Some(catalog.clone()));
    assert_eq!(exposure, 0.7);
}

/// What resolving the edit warns about is printed on standard error, and the render
/// goes on: here, raw defaults whose preset is gone start from Adobe Default.
#[test]
fn render_prints_the_edits_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let photo = chart(dir.path(), "chart.dng");
    let gone = defaults(&dir.path().join("Gone.xmp"));
    let (exposure, run) = exposure_rendered(&photo, &gone, |_| {});
    assert_eq!(exposure, 0.);
    assert!(run.err.contains("Brighter"), "{}", run.err);
    assert!(!run.out.contains("Brighter"), "{}", run.out);
    // What it rendered goes to standard output.
    assert!(run.out.contains("| develop "), "{}", run.out);
}

/// An existing output is refused, and left as it was, unless `--overwrite`.
#[test]
fn render_refuses_an_existing_output_without_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let photo = chart(dir.path(), "chart.dng");
    let output = dir.path().join("taken.jpg");
    std::fs::write(&output, b"kept").unwrap();
    let options = Render {
        input: photo,
        output: output.clone(),
        max_edge: 64,
        ..Default::default()
    };
    let raw_defaults = defaults(&dir.path().join("none.xmp"));
    let ai = no_denoise(dir.path());
    let refused = run(|console| {
        render(
            &options,
            &|| raw_defaults.clone(),
            &ai,
            Cancel::NEVER,
            console,
        )
    });
    let error = refused.result.unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error:#}");
    assert_eq!(std::fs::read(&output).unwrap(), b"kept");
    let replaced = run(|console| {
        render(
            &Render {
                overwrite: true,
                ..options.clone()
            },
            &|| raw_defaults.clone(),
            &ai,
            Cancel::NEVER,
            console,
        )
    });
    assert!(replaced.result.is_ok(), "{:?}", replaced.result);
    let written = image::open(&output).unwrap();
    assert_eq!(written.width().max(written.height()), 64);
}

/// The raw defaults are read only when the photo starts from them, and AI Denoise is
/// made only for a photo that is developed and rendered: an explicit recipe, a
/// sidecar or a refused output never wait on the presets the defaults read.
#[test]
fn render_asks_for_what_the_desktop_keeps_only_when_it_needs_it() {
    use std::cell::Cell;
    let dir = tempfile::tempdir().unwrap();
    let photo = chart(dir.path(), "chart.dng");
    let raw_defaults = defaults(&dir.path().join("none.xmp"));
    let (defaults_read, denoise_made) = (Cell::new(0), Cell::new(0));
    let read_defaults = || {
        defaults_read.set(defaults_read.get() + 1);
        raw_defaults.clone()
    };
    let make_denoise = || {
        denoise_made.set(denoise_made.get() + 1);
        no_denoise(dir.path())()
    };
    let render_to = |output: &str, change: &dyn Fn(&mut Render)| {
        let mut options = Render {
            input: photo.clone(),
            output: dir.path().join(output),
            max_edge: 64,
            ..Default::default()
        };
        change(&mut options);
        let run = run(|console| {
            render(
                &options,
                &read_defaults,
                &make_denoise,
                Cancel::NEVER,
                console,
            )
        });
        (
            run.result.is_ok(),
            defaults_read.replace(0),
            denoise_made.replace(0),
        )
    };
    // A refused output asks for neither.
    assert_eq!(render_to("out.webp", &|_| {}), (false, 0, 0));
    assert_eq!(
        render_to("out.tif", &|o| o.jxl_quality = Some(80)),
        (false, 0, 0)
    );
    assert_eq!(
        render_to("out.tif", &|o| o.quality = Some(80)),
        (false, 0, 0)
    );
    // An explicit recipe needs no raw defaults.
    let explicit = dir.path().join("explicit.json");
    let metadata = crate::photo::open(&photo).unwrap().metadata;
    crate::presets::save_preset(&explicit, &Recipe::default(), &metadata).unwrap();
    assert_eq!(
        render_to("recipe.tif", &|o| o.recipe = Some(explicit.clone())),
        (true, 0, 1)
    );
    // A stage tap renders without AI Denoise.
    assert_eq!(
        render_to("tap.tif", &|o| o.tap = Some(Tap::SceneOutput)),
        (true, 1, 0)
    );
    // A photo without an edit reads them once.
    assert_eq!(render_to("plain.tif", &|_| {}), (true, 1, 1));
}

/// `--quality` sets a JPEG's quality, 92 when it isn't given as before, and is
/// refused for another format before anything renders.
#[test]
fn render_writes_a_jpeg_at_the_quality_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let photo = chart(dir.path(), "chart.dng");
    let raw_defaults = defaults(&dir.path().join("none.xmp"));
    let ai = no_denoise(dir.path());
    let jpeg = |name: &str, quality: Option<u8>| {
        let output = dir.path().join(name);
        let options = Render {
            input: photo.clone(),
            output: output.clone(),
            max_edge: 256,
            quality,
            ..Default::default()
        };
        let run = run(|console| {
            render(
                &options,
                &|| raw_defaults.clone(),
                &ai,
                Cancel::NEVER,
                console,
            )
        });
        run.result.map(|()| std::fs::read(&output).unwrap())
    };
    let (low, high) = (
        jpeg("10.jpg", Some(10)).unwrap(),
        jpeg("100.jpg", Some(100)).unwrap(),
    );
    assert!(low.len() * 2 < high.len(), "{} / {}", low.len(), high.len());
    assert_eq!(
        jpeg("default.jpg", None).unwrap(),
        jpeg("92.jpg", Some(92)).unwrap()
    );
    let refused = jpeg("out.tif", Some(80)).unwrap_err();
    assert!(refused.to_string().contains("--quality"), "{refused:#}");
    assert!(!dir.path().join("out.tif").exists());
}

/// `render-batch --quality` sets its JPEG jobs' quality, and leaves its other
/// formats as they were.
#[test]
fn render_batch_writes_its_jpegs_at_the_quality_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let photo = chart(dir.path(), "chart.dng");
    let xmp = exposure_xmp(dir.path(), "a.xmp", "0");
    let ai = no_denoise(dir.path());
    let batch = |folder: &str, quality: Option<u8>| {
        let folder = dir.path().join(folder);
        std::fs::create_dir(&folder).unwrap();
        let jobs = serde_json::json!([
            {"xmp": xmp, "output": folder.join("a.jpg"), "max_edge": 256},
            {"xmp": xmp, "output": folder.join("a.png"), "max_edge": 64},
        ]);
        let listing = folder.join("jobs.json");
        std::fs::write(&listing, jobs.to_string()).unwrap();
        let options = RenderBatch {
            input: photo.clone(),
            jobs: listing,
            color_space: OutputSpace::Srgb,
            quality,
            gpu: false,
        };
        let run = run(|console| render_batch(&options, &ai, Cancel::NEVER, console));
        assert!(run.result.is_ok() && run.err.is_empty(), "{}", run.err);
        let read = |name: &str| std::fs::read(folder.join(name)).unwrap();
        (read("a.jpg"), read("a.png"))
    };
    let (default_jpeg, default_png) = batch("default", None);
    let (jpeg_92, _) = batch("92", Some(92));
    let (jpeg_10, png_10) = batch("10", Some(10));
    assert_eq!(default_jpeg, jpeg_92);
    assert!(jpeg_10.len() * 2 < default_jpeg.len());
    assert_eq!(png_10, default_png);
}

/// `render-batch` skips a job whose output exists, reports a job that fails on
/// standard error and renders the rest, and succeeds.
#[test]
fn render_batch_skips_existing_outputs_and_reports_failed_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let photo = chart(dir.path(), "chart.dng");
    let out = |name: &str| dir.path().join(name);
    std::fs::write(out("kept.tif"), b"kept").unwrap();
    let broken = dir.path().join("broken.xmp");
    std::fs::write(&broken, "not XMP").unwrap();
    let jobs = serde_json::json!([
        {"xmp": exposure_xmp(dir.path(), "a.xmp", "0"), "output": out("kept.tif")},
        {"xmp": broken, "output": out("broken.tif"), "max_edge": 64},
        {"xmp": exposure_xmp(dir.path(), "b.xmp", "+1.00"), "output": out("new.tif"), "max_edge": 64},
    ]);
    let listing = dir.path().join("jobs.json");
    std::fs::write(&listing, jobs.to_string()).unwrap();
    let ai = no_denoise(dir.path());
    let batch = run(|console| {
        render_batch(
            &RenderBatch {
                input: photo.clone(),
                jobs: listing.clone(),
                color_space: OutputSpace::Srgb,
                quality: None,
                gpu: false,
            },
            &ai,
            Cancel::NEVER,
            console,
        )
    });
    assert!(batch.result.is_ok(), "{:?}", batch.result);
    assert_eq!(std::fs::read(out("kept.tif")).unwrap(), b"kept");
    assert!(!batch.err.contains("kept.tif"), "{}", batch.err);
    assert!(batch.err.contains("broken.tif"), "{}", batch.err);
    assert!(!out("broken.tif").exists());
    assert!(out("new.tif").is_file());
}

fn tap() -> crate::rendered::Rendered {
    crate::rendered::Rendered {
        width: 2,
        height: 1,
        pixels: vec![[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]],
    }
}

#[test]
fn stage_taps_never_replace_raws_their_source_or_unasked_files() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("photo.dng");
    std::fs::write(&raw, b"raw bytes").unwrap();
    // Over the RAW it was rendered from, with or without --overwrite.
    for overwrite in [false, true] {
        assert!(save_stage(&raw, &raw, tap(), overwrite).is_err());
        assert_eq!(std::fs::read(&raw).unwrap(), b"raw bytes");
    }
    // Over the source under a non-RAW name.
    let source = dir.path().join("source.tif");
    std::fs::write(&source, b"source").unwrap();
    assert!(save_stage(&source, &source, tap(), true).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"source");
    // An existing file only with --overwrite.
    let output = dir.path().join("tap.tif");
    std::fs::write(&output, b"old").unwrap();
    assert!(save_stage(&output, &raw, tap(), false).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), b"old");
    save_stage(&output, &raw, tap(), true).unwrap();
    let written = image::open(&output).unwrap().to_rgb32f();
    assert_eq!(written.dimensions(), (2, 1));
    assert_eq!(written.get_pixel(1, 0).0, [0.4, 0.5, 0.6]);
}

/// An output whose format can't hold the space is refused before anything is read.
#[test]
fn prophoto_is_refused_outside_tiff() {
    use crate::color::OutputSpace;
    for name in ["a.jpg", "a.png"] {
        let refused = check_output(std::path::Path::new(name), OutputSpace::ProPhoto);
        assert!(refused.unwrap_err().to_string().contains("TIFF"), "{name}");
        check_output(std::path::Path::new(name), OutputSpace::AdobeRgb).unwrap();
    }
    check_output(std::path::Path::new("a.tiff"), OutputSpace::ProPhoto).unwrap();
    assert!(check_output(std::path::Path::new("a.webp"), OutputSpace::Srgb).is_err());
}

#[test]
fn a_stage_tap_is_only_written_under_a_tiff_name() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("photo.dng");
    for name in ["tap.jpg", "tap.png", "tap.exr", "tap"] {
        let output = dir.path().join(name);
        let refused = save_stage(&output, &source, tap(), false).unwrap_err();
        assert!(refused.to_string().contains(".tif"), "{name}: {refused}");
        assert!(!output.exists(), "{name}");
    }
    for name in ["tap.tif", "tap.TIFF"] {
        save_stage(&dir.path().join(name), &source, tap(), false).unwrap();
    }
}

/// `--gpu` on `render` and `render-batch` succeeds (using GPU when available and
/// falling back cleanly to CPU otherwise) and matches the CPU default output.
#[test]
fn render_and_render_batch_with_gpu_flag_match_cpu_output() {
    let dir = tempfile::tempdir().unwrap();
    let photo = chart(dir.path(), "chart.dng");
    let xmp = exposure_xmp(dir.path(), "a.xmp", "+0.25");
    let ai = no_denoise(dir.path());

    // Single `render` to PNG (Rgb8 zero-copy) and 16-bit TIFF (Rgb16 zero-copy).
    for ext in ["png", "tif"] {
        let cpu_out = dir.path().join(format!("single-cpu.{ext}"));
        let gpu_out = dir.path().join(format!("single-gpu.{ext}"));
        for (gpu, out) in [(false, &cpu_out), (true, &gpu_out)] {
            let options = Render {
                input: photo.clone(),
                output: out.clone(),
                xmp: Some(xmp.clone()),
                max_edge: 128,
                gpu,
                ..Default::default()
            };
            let res = run(|console| {
                render(
                    &options,
                    &DevelopDefaults::default,
                    &ai,
                    Cancel::NEVER,
                    console,
                )
            });
            assert!(
                res.result.is_ok(),
                "{ext} gpu={gpu}: {:?} {}",
                res.result,
                res.err
            );
        }
        if ext == "png" {
            let a = image::open(&cpu_out).unwrap().to_rgb8();
            let b = image::open(&gpu_out).unwrap().to_rgb8();
            assert_eq!(a.dimensions(), b.dimensions());
            let max_diff = a
                .as_raw()
                .iter()
                .zip(b.as_raw())
                .map(|(&x, &y)| x.abs_diff(y))
                .max()
                .unwrap_or(0);
            assert!(max_diff <= 1, "PNG max diff {max_diff}");
        } else {
            let a = image::open(&cpu_out).unwrap().to_rgb16();
            let b = image::open(&gpu_out).unwrap().to_rgb16();
            assert_eq!(a.dimensions(), b.dimensions());
            let max_diff = a
                .as_raw()
                .iter()
                .zip(b.as_raw())
                .map(|(&x, &y)| x.abs_diff(y))
                .max()
                .unwrap_or(0);
            assert!(max_diff <= 6, "TIFF max diff {max_diff}");
        }
    }

    // `render-batch` with `--gpu` vs CPU default across multiple jobs.
    let run_batch = |folder: &str, gpu: bool| {
        let folder = dir.path().join(folder);
        std::fs::create_dir(&folder).unwrap();
        let jobs = serde_json::json!([
            {"xmp": xmp, "output": folder.join("a.png"), "max_edge": 96},
            {"xmp": xmp, "output": folder.join("b.tif"), "max_edge": 96},
        ]);
        let listing = folder.join("jobs.json");
        std::fs::write(&listing, jobs.to_string()).unwrap();
        let options = RenderBatch {
            input: photo.clone(),
            jobs: listing,
            color_space: OutputSpace::Srgb,
            quality: None,
            gpu,
        };
        let res = run(|console| render_batch(&options, &ai, Cancel::NEVER, console));
        assert!(
            res.result.is_ok() && res.err.is_empty(),
            "gpu={gpu}: {}",
            res.err
        );
        (folder.join("a.png"), folder.join("b.tif"))
    };
    let (cpu_png, cpu_tif) = run_batch("batch-cpu", false);
    let (gpu_png, gpu_tif) = run_batch("batch-gpu", true);
    let a = image::open(&cpu_png).unwrap().to_rgb8();
    let b = image::open(&gpu_png).unwrap().to_rgb8();
    assert_eq!(a.dimensions(), b.dimensions());
    let png_diff = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(&x, &y)| x.abs_diff(y))
        .max()
        .unwrap_or(0);
    assert!(png_diff <= 1, "batch PNG max diff {png_diff}");
    let a = image::open(&cpu_tif).unwrap().to_rgb16();
    let b = image::open(&gpu_tif).unwrap().to_rgb16();
    assert_eq!(a.dimensions(), b.dimensions());
    let tif_diff = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(&x, &y)| x.abs_diff(y))
        .max()
        .unwrap_or(0);
    assert!(tif_diff <= 6, "batch TIFF max diff {tif_diff}");
}
