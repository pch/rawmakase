use super::edits::*;
use super::progress::*;
use super::render::*;
use super::source::*;
use super::thumbnails::*;
use crate::catalog::PhotoId;
use crate::catalog::preview_cache::{PreviewCache, Stamp};
use crate::model::recipe::Recipe;
use eframe::egui;
use rawmakase_cancel::Cancel;
use rawmakase_decode::ai::cache::AiCache;
use std::path::{Path, PathBuf};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::Receiver,
};
use std::time::Duration;

/// A grey JPEG at `path`, and a saved edit of it one stop brighter.
pub(in crate::app::library) fn edited_jpeg(path: &Path) -> anyhow::Result<EditSource> {
    image::RgbImage::from_pixel(64, 48, image::Rgb([60, 60, 60])).save(path)?;
    let m = crate::camera_data::Metadata {
        source: crate::camera_data::Source::Rendered,
        ..Default::default()
    };
    let recipe = crate::model::recipe::Recipe {
        exposure: 1.,
        ..crate::model::recipe::Recipe::for_rendered(&m)
    };
    Ok(EditSource::Recipe(serde_json::to_string(&recipe)?))
}

#[test]
fn an_edited_jpeg_renders_with_its_edit() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("photo.jpg");
    let edit = edited_jpeg(&path)?;
    let image = render_edited(&path, &edit, Cancel::NEVER)?;
    assert_eq!(image.dimensions(), (64, 48));
    // One stop brighter than the file's 60.
    assert!(
        image.pixels().all(|p| p.0[1] > 75),
        "{:?}",
        image.get_pixel(0, 0)
    );
    Ok(())
}

#[test]
fn an_edited_preview_is_rendered_as_develops_fit_draws_it() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("textured.png");
    // Larger than the preview, with edges and gradients a few pixels wide
    // that a reduced copy keeps, as a photo's.
    image::RgbImage::from_fn(1800, 1200, |x, y| {
        let v = (((x / 9) * 5 + (y / 7) * 3) % 200 + 20) as u8;
        image::Rgb([v, 255 - v, (x / 4 % 128 + 64) as u8])
    })
    .save(&path)?;
    let m = crate::camera_data::Metadata {
        source: crate::camera_data::Source::Rendered,
        ..Default::default()
    };
    let mut recipe = Recipe {
        exposure: 0.5,
        shadows: 0.4,
        highlights: -0.3,
        ..Recipe::for_rendered(&m)
    };
    recipe.effects.clarity = 0.2;
    let edit = EditSource::Recipe(serde_json::to_string(&recipe)?);
    let cancel = AtomicBool::new(false);
    let preview = render_edited(&path, &edit, Cancel::new(&cancel))?;
    let (image, recipe) = open_quickly(&path, Some(&edit), Cancel::new(&cancel))?;
    let fitted = rgb(crate::develop::render_fitted(
        &image,
        &recipe,
        640,
        Cancel::new(&cancel),
    )?)?;
    assert_eq!(preview, fitted);
    // Close to the photo rendered whole and reduced, as before.
    let whole = finish_quickly(&image, &recipe, 640, Cancel::new(&cancel))?;
    assert_eq!(preview.dimensions(), whole.dimensions());
    let difference = preview
        .as_raw()
        .iter()
        .zip(whole.as_raw())
        .map(|(a, b)| f64::from(a.abs_diff(*b)))
        .sum::<f64>()
        / preview.as_raw().len() as f64;
    assert!(difference < 4., "{difference}");
    Ok(())
}

#[test]
fn an_edited_render_stops_once_cancelled() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("photo.png");
    let EditSource::Recipe(json) = edited_jpeg(&path)? else {
        unreachable!()
    };
    let recipe: Recipe = serde_json::from_str(&json)?;
    let crate::photo::Opened::Rendered(image) = crate::photo::open_source(&path, Cancel::NEVER)?
    else {
        unreachable!("a PNG opens rendered")
    };
    assert!(finish_quickly(&image, &recipe, 640, Cancel::NEVER).is_ok());
    // Set as the render begins, as when the pace drops.
    assert!(finish_quickly(&image, &recipe, 640, Cancel::new(&AtomicBool::new(true))).is_err());
    Ok(())
}

#[test]
fn worker_persists_previews_and_reuses_them_when_original_is_offline() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("photo.png");
    image::RgbImage::new(720, 480).save(&source)?;
    let cache_path = directory.path().join("previews.sqlite3");
    let (tx, rx, _) = spawn(
        cache_path.clone(),
        egui::Context::default(),
        ShownFiles::default(),
    );
    tx.try_send(source.clone())?;
    let result = rx.recv_timeout(Duration::from_secs(10))?;
    assert_eq!(result.image.unwrap().dimensions(), (640, 427));
    assert!(result.cache_error.is_none());
    // Reopen through another worker: a memory-only result cannot pass this.
    std::fs::remove_file(&source)?;
    let (tx, rx, _) = spawn(cache_path, egui::Context::default(), ShownFiles::default());
    tx.try_send(source.clone())?;
    let result = rx.recv_timeout(Duration::from_secs(10))?;
    assert_eq!(result.path, source);
    assert!(result.image.is_some());
    assert!(result.cache_error.is_none());
    Ok(())
}

#[test]
fn a_panicking_preview_is_unavailable_and_the_next_one_is_made() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let [panics, works] = ["panics.png", "works.png"].map(|name| directory.path().join(name));
    for path in [&panics, &works] {
        image::RgbImage::new(16, 16).save(path)?;
    }
    fn thumbnail(path: &Path) -> anyhow::Result<image::RgbImage> {
        assert!(!path.ends_with("panics.png"), "thumbnail panics");
        Ok(image::open(path)?.to_rgb8())
    }
    let (tx, rx, _) = spawn_with(
        directory.path().join("previews.sqlite3"),
        egui::Context::default(),
        thumbnail,
        ShownFiles::default(),
    );
    tx.try_send(panics.clone())?;
    tx.try_send(works.clone())?;
    // Made side by side, so in either order.
    let mut results = [
        rx.recv_timeout(Duration::from_secs(10))?,
        rx.recv_timeout(Duration::from_secs(10))?,
    ];
    results.sort_by(|a, b| a.path.cmp(&b.path));
    let [first, second] = results;
    assert_eq!(first.path, panics);
    assert!(first.image.is_none());
    assert_eq!(second.path, works);
    assert!(second.image.is_some());
    Ok(())
}

#[test]
fn a_panicking_edited_preview_fails_and_the_next_one_is_rendered() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let [panics, works] = ["panics.png", "works.png"].map(|name| directory.path().join(name));
    for path in [&panics, &works] {
        image::RgbImage::new(16, 16).save(path)?;
    }
    fn render(path: &Path, _: &EditSource, _: Cancel<'_>) -> anyhow::Result<image::RgbImage> {
        assert!(!path.ends_with("panics.png"), "render panics");
        Ok(image::open(path)?.to_rgb8())
    }
    let wanted = Wanted::default();
    wanted.lock().unwrap().extend([PhotoId(1), PhotoId(2)]);
    let (tx, rx, _) = spawn_edited_with(
        directory.path().join("previews.sqlite3"),
        wanted,
        egui::Context::default(),
        render,
    );
    let source = EditSource::Recipe("{}".into());
    let job = |id, path: &PathBuf| EditJob::Render {
        id,
        ticket: 0,
        path: path.clone(),
        source: source.clone(),
        ahead: false,
    };
    tx.send(job(PhotoId(1), &panics))?;
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(10))?,
        EditResult::Failed(PhotoId(1), 0)
    ));
    tx.send(job(PhotoId(2), &works))?;
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(10))?,
        EditResult::Ready(PhotoId(2), 0, _)
    ));
    Ok(())
}

/// Develop's render of a file replaced since is not kept as the new file's
/// preview: it is stored under the file as it was rendered, or not at all.
#[test]
fn a_stored_render_of_a_file_replaced_since_is_not_kept() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("photo.png");
    image::RgbImage::new(16, 16).save(&source)?;
    let rendered = Stamp::read(&source)?;
    image::RgbImage::new(24, 24).save(&source)?;
    let cache = directory.path().join("previews.sqlite3");
    let (tx, rx, _) = spawn_edited(cache.clone(), Wanted::default(), egui::Context::default());
    tx.send(EditJob::Store {
        path: source.clone(),
        tag: "edit".into(),
        stamp: rendered,
        image: image::RgbImage::new(4, 4),
    })?;
    // Stores go first: once a later job is answered, the store is done.
    tx.send(EditJob::Render {
        id: PhotoId(1),
        ticket: 0,
        path: source.clone(),
        source: EditSource::Recipe("{}".into()),
        ahead: false,
    })?;
    match rx.recv_timeout(Duration::from_secs(10))? {
        EditResult::Skipped(PhotoId(1), 0) => {}
        EditResult::CacheError(error) => panic!("reported as a failure: {error}"),
        _ => panic!("unexpected result"),
    }
    assert!(
        PreviewCache::open(&cache)?
            .load_tagged(&source, "edit")?
            .is_none(),
        "kept as the replaced file's preview"
    );
    Ok(())
}

/// Stand-ins for the edited-preview renders: a plain one dark, one from AI
/// Denoise's result bright, each counted by file and kind.
static RENDERS: Mutex<Vec<(PathBuf, PreviewAi)>> = Mutex::new(Vec::new());

fn renders_of(path: &Path, kind: PreviewAi) -> usize {
    let renders = RENDERS.lock().unwrap();
    renders
        .iter()
        .filter(|(p, k)| p == path && *k == kind)
        .count()
}

fn plain(path: &Path, _: &EditSource, _: Cancel<'_>) -> anyhow::Result<image::RgbImage> {
    RENDERS
        .lock()
        .unwrap()
        .push((path.into(), PreviewAi::Plain));
    Ok(image::RgbImage::from_pixel(8, 8, image::Rgb([10; 3])))
}

fn with_ai(
    path: &Path,
    _: &crate::storage::Identity,
    _: &EditSource,
    _: &rawmakase_decode::ai::cache::AiEntry,
    _: Cancel<'_>,
) -> anyhow::Result<image::RgbImage> {
    RENDERS
        .lock()
        .unwrap()
        .push((path.into(), PreviewAi::Finished));
    Ok(image::RgbImage::from_pixel(8, 8, image::Rgb([200; 3])))
}

/// An edit with AI Denoise on.
fn with_ai_denoise() -> anyhow::Result<EditSource> {
    let mut recipe = Recipe::default();
    recipe.switch_ai_denoise(true);
    Ok(EditSource::Recipe(serde_json::to_string(&recipe)?))
}

/// Stores a finished AI Denoise result for the file at `path` in `cache`.
fn finish_ai(cache: &AiCache, path: &Path) -> anyhow::Result<()> {
    let layout = rawmakase_decode::ai::denoise::Layout::new(4, 4, [0, 0]);
    let entry = rawmakase_decode::ai::cache::AiEntry {
        layout,
        gain: 1.,
        rgb: vec![0.5; 3 * layout.output_pixels()],
        clipped: vec![false; layout.packed_width * layout.packed_height],
    };
    let key = crate::ai_denoise::key(path, rawmakase_inference::Provider::Cpu)?;
    cache.store(&key, &entry)
}

/// What photo 1's preview, asked for with `ticket`, comes back as: whether it
/// is bright (rendered from AI Denoise's result).
fn preview_of(
    tx: &EditQueue,
    rx: &Receiver<EditResult>,
    path: &Path,
    source: &EditSource,
    ticket: u64,
) -> anyhow::Result<bool> {
    tx.send(EditJob::Render {
        id: PhotoId(1),
        ticket,
        path: path.into(),
        source: source.clone(),
        ahead: false,
    })?;
    match rx.recv_timeout(Duration::from_secs(10))? {
        EditResult::Ready(PhotoId(1), t, image) if t == ticket => {
            Ok(image.get_pixel(0, 0).0[0] > 100)
        }
        _ => anyhow::bail!("no preview"),
    }
}

/// An edit with AI Denoise shows its plain render until a finished result is
/// cached, then the result's, kept apart from the plain one; offline, the one
/// kept is still shown.
#[test]
fn an_ai_denoise_preview_follows_its_finished_result_and_stays_offline() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("photo.png");
    image::RgbImage::new(16, 16).save(&path)?;
    let ai = AiCache::new(directory.path().join("ai"), u64::MAX);
    let wanted = Wanted::default();
    wanted.lock().unwrap().insert(PhotoId(1));
    let (tx, rx, _) = spawn_edited_with_ai(
        directory.path().join("previews.sqlite3"),
        wanted,
        egui::Context::default(),
        plain,
        with_ai,
    );
    tx.set_ai_cache(ai.clone());
    let source = with_ai_denoise()?;
    assert!(
        !preview_of(&tx, &rx, &path, &source, 0)?,
        "plain without a result"
    );
    assert!(!preview_of(&tx, &rx, &path, &source, 1)?);
    assert_eq!(renders_of(&path, PreviewAi::Plain), 1, "the plain one kept");
    finish_ai(&ai, &path)?;
    assert!(
        preview_of(&tx, &rx, &path, &source, 2)?,
        "the result's once finished"
    );
    assert!(preview_of(&tx, &rx, &path, &source, 3)?);
    assert_eq!(
        renders_of(&path, PreviewAi::Finished),
        1,
        "the result's kept"
    );
    // Offline: no file to check a result against, the preview kept is shown.
    std::fs::rename(&path, directory.path().join("away.png"))?;
    assert!(preview_of(&tx, &rx, &path, &source, 4)?, "shown offline");
    assert_eq!(renders_of(&path, PreviewAi::Finished), 1);
    Ok(())
}

/// A finished result renders a RAW's preview from it: decoded at full size and
/// the result assembled over it, as Develop shows it.
#[test]
fn a_raw_renders_its_preview_from_a_finished_result() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("photo.dng");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-a.dng"),
        &path,
    )?;
    let cancel = AtomicBool::new(false);
    let packed = crate::photo::open(&path)?.packed_bayer(Cancel::new(&cancel))?;
    let layout = packed.layout;
    // A flat result: the preview shows it, not the chart.
    let entry = rawmakase_decode::ai::cache::AiEntry {
        layout,
        gain: 1.,
        rgb: vec![0.3; 3 * layout.output_pixels()],
        clipped: packed.clipped,
    };
    let ai = AiCache::new(directory.path().join("ai"), u64::MAX);
    let key = crate::ai_denoise::key(&path, rawmakase_inference::Provider::Cpu)?;
    ai.store(&key, &entry)?;
    let mut recipe = Recipe::default();
    recipe.switch_ai_denoise(true);
    recipe.ai_denoise.amount = 1.;
    let source = EditSource::Recipe(serde_json::to_string(&recipe)?);
    let (plain, shows) = develop_for_preview(&path, &source, 256, None, Cancel::new(&cancel))?;
    assert_eq!(shows, PreviewAi::Plain);
    let (denoised, shows) =
        develop_for_preview(&path, &source, 256, Some(&ai), Cancel::new(&cancel))?;
    assert_eq!(shows, PreviewAi::Finished);
    assert_eq!(plain.dimensions(), denoised.dimensions());
    // Rendered from the flat result: far from the chart's own colours.
    let difference = plain
        .as_raw()
        .iter()
        .zip(denoised.as_raw())
        .map(|(a, b)| f64::from(a.abs_diff(*b)))
        .sum::<f64>()
        / plain.as_raw().len() as f64;
    assert!(difference > 30., "{difference}");
    Ok(())
}

/// A preview made from another model's result is not shown for this one's.
#[test]
fn a_preview_of_another_models_result_is_made_again() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("photo.png");
    image::RgbImage::new(16, 16).save(&path)?;
    let ai = AiCache::new(directory.path().join("ai"), u64::MAX);
    finish_ai(&ai, &path)?;
    let source = with_ai_denoise()?;
    let previews = directory.path().join("previews.sqlite3");
    PreviewCache::open(&previews)?.store_tagged(
        &path,
        &source.preview_tag_of(PreviewAi::Finished, "an-earlier-model"),
        &Stamp::read(&path)?,
        &image::RgbImage::from_pixel(8, 8, image::Rgb([90; 3])),
    )?;
    let wanted = Wanted::default();
    wanted.lock().unwrap().insert(PhotoId(1));
    let (tx, rx, _) =
        spawn_edited_with_ai(previews, wanted, egui::Context::default(), plain, with_ai);
    tx.set_ai_cache(ai);
    assert!(
        preview_of(&tx, &rx, &path, &source, 0)?,
        "rendered from this model's"
    );
    assert_eq!(renders_of(&path, PreviewAi::Finished), 1);
    Ok(())
}

#[test]
fn edited_preview_cache_failures_are_reported() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("photo.png");
    image::RgbImage::new(16, 16).save(&source)?;
    let (tx, rx, _) = spawn_edited(
        directory.path().join("previews.sqlite3"),
        Wanted::default(),
        egui::Context::default(),
    );
    // Larger than the cache keeps.
    tx.send(EditJob::Store {
        stamp: Stamp::read(&source)?,
        path: source,
        tag: "edit".into(),
        image: image::RgbImage::new(2048, 8),
    })?;
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(10))?,
        EditResult::CacheError(_)
    ));
    Ok(())
}

#[test]
fn cache_failure_does_not_stop_previews_or_completion() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("photo.png");
    image::RgbImage::new(16, 16).save(&source)?;
    // A directory cannot be opened as a SQLite database.
    let (tx, rx, _) = spawn(
        directory.path().into(),
        egui::Context::default(),
        ShownFiles::default(),
    );
    let mut progress = Progress::default();
    for path in [source.clone(), directory.path().join("missing.ARW")] {
        tx.try_send(path)?;
        progress.queued();
    }
    // Made side by side, so in either order.
    let first = rx.recv_timeout(Duration::from_secs(10))?;
    let second = rx.recv_timeout(Duration::from_secs(10))?;
    let (made, missing) = if first.path == source {
        (first, second)
    } else {
        (second, first)
    };
    assert!(made.image.is_some());
    assert!(made.cache_error.is_some());
    assert!(missing.image.is_none());
    progress.finish(&made);
    assert_eq!((progress.completed, progress.total), (1, 2));
    progress.finish(&missing);
    assert_eq!(
        (progress.completed, progress.total, progress.failed),
        (2, 2, 1)
    );
    assert!(progress.cache_error.is_some());
    progress.queued();
    assert_eq!(
        (progress.completed, progress.total, progress.failed),
        (0, 1, 0)
    );
    Ok(())
}

/// Waits up to 20 s for `done`.
fn until(done: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(20), "timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(super) fn edited_png(directory: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let path = directory.join(name);
    image::RgbImage::new(16, 16).save(&path)?;
    Ok(path)
}

pub(super) fn render_job(id: i64, path: &Path, ahead: bool) -> EditJob {
    EditJob::Render {
        id: PhotoId(id),
        ticket: id as u64,
        path: path.into(),
        source: EditSource::Recipe(format!("{{\"photo\":{id}}}")),
        ahead,
    }
}

#[test]
fn edited_previews_render_side_by_side_at_full_pace() -> anyhow::Result<()> {
    static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    // Ends once the other render is under way too.
    fn render(path: &Path, _: &EditSource, _: Cancel<'_>) -> anyhow::Result<image::RgbImage> {
        RUNNING.fetch_add(1, Ordering::SeqCst);
        until(|| RUNNING.load(Ordering::SeqCst) >= 2);
        Ok(image::open(path)?.to_rgb8())
    }
    let directory = tempfile::tempdir()?;
    let wanted = Wanted::default();
    wanted.lock().unwrap().extend([PhotoId(1), PhotoId(2)]);
    let (tx, rx, _) = spawn_edited_with(
        directory.path().join("previews.sqlite3"),
        wanted,
        egui::Context::default(),
        render,
    );
    tx.set_pace(Pace::Full);
    for id in [1, 2] {
        tx.send(render_job(
            id,
            &edited_png(directory.path(), &format!("{id}.png"))?,
            false,
        ))?;
    }
    for _ in 0..2 {
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(30))?,
            EditResult::Ready(..)
        ));
    }
    Ok(())
}

#[test]
fn edited_previews_render_one_at_a_time_in_the_background() -> anyhow::Result<()> {
    static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static MOST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    fn render(path: &Path, _: &EditSource, _: Cancel<'_>) -> anyhow::Result<image::RgbImage> {
        let running = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
        MOST.fetch_max(running, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(20));
        RUNNING.fetch_sub(1, Ordering::SeqCst);
        Ok(image::open(path)?.to_rgb8())
    }
    let directory = tempfile::tempdir()?;
    let wanted = Wanted::default();
    wanted.lock().unwrap().extend((1..=3).map(PhotoId));
    let (tx, rx, _) = spawn_edited_with(
        directory.path().join("previews.sqlite3"),
        wanted,
        egui::Context::default(),
        render,
    );
    for id in 1..=3 {
        tx.send(render_job(
            id,
            &edited_png(directory.path(), &format!("{id}.png"))?,
            false,
        ))?;
    }
    for _ in 0..3 {
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(30))?,
            EditResult::Ready(..)
        ));
    }
    assert_eq!(MOST.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn full_pace_renders_give_way_when_the_pace_drops() -> anyhow::Result<()> {
    static STARTED: AtomicBool = AtomicBool::new(false);
    fn render(_: &Path, _: &EditSource, cancel: Cancel<'_>) -> anyhow::Result<image::RgbImage> {
        STARTED.store(true, Ordering::SeqCst);
        until(|| cancel.is_cancelled());
        anyhow::bail!("Cancelled")
    }
    let directory = tempfile::tempdir()?;
    let wanted = Wanted::default();
    wanted.lock().unwrap().insert(PhotoId(1));
    let (tx, rx, _) = spawn_edited_with(
        directory.path().join("previews.sqlite3"),
        wanted,
        egui::Context::default(),
        render,
    );
    tx.set_pace(Pace::Full);
    tx.send(render_job(
        1,
        &edited_png(directory.path(), "1.png")?,
        false,
    ))?;
    until(|| STARTED.load(Ordering::SeqCst));
    // Develop came to the front: asked again when shown.
    tx.set_pace(Pace::Background);
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(30))?,
        EditResult::Skipped(PhotoId(1), 1)
    ));
    Ok(())
}

#[test]
fn photos_asked_ahead_are_kept_and_then_shown_from_the_cache() -> anyhow::Result<()> {
    static RENDERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    fn render(path: &Path, _: &EditSource, _: Cancel<'_>) -> anyhow::Result<image::RgbImage> {
        RENDERS.fetch_add(1, Ordering::SeqCst);
        Ok(image::open(path)?.to_rgb8())
    }
    let directory = tempfile::tempdir()?;
    let path = edited_png(directory.path(), "1.png")?;
    let wanted = Wanted::default();
    let (tx, rx, _) = spawn_edited_with(
        directory.path().join("previews.sqlite3"),
        wanted.clone(),
        egui::Context::default(),
        render,
    );
    // Not rendered ahead in the background.
    tx.send(render_job(1, &path, true))?;
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(30))?,
        EditResult::Skipped(PhotoId(1), 1)
    ));
    assert_eq!(RENDERS.load(Ordering::SeqCst), 0);
    tx.set_pace(Pace::Full);
    tx.send(render_job(1, &path, true))?;
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(30))?,
        EditResult::Kept(PhotoId(1), 1)
    ));
    // Shown, it comes from the cache.
    wanted.lock().unwrap().insert(PhotoId(1));
    tx.send(render_job(1, &path, false))?;
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(30))?,
        EditResult::Ready(PhotoId(1), 1, _)
    ));
    assert_eq!(RENDERS.load(Ordering::SeqCst), 1);
    // Asked ahead again, it is kept without a render.
    wanted.lock().unwrap().clear();
    tx.send(render_job(1, &path, true))?;
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(30))?,
        EditResult::Kept(PhotoId(1), 1)
    ));
    assert_eq!(RENDERS.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn thumbnails_are_made_side_by_side() -> anyhow::Result<()> {
    static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    // Ends once the other thumbnail is under way too.
    fn thumbnail(path: &Path) -> anyhow::Result<image::RgbImage> {
        RUNNING.fetch_add(1, Ordering::SeqCst);
        until(|| RUNNING.load(Ordering::SeqCst) >= 2);
        Ok(image::open(path)?.to_rgb8())
    }
    let directory = tempfile::tempdir()?;
    let (tx, rx, _) = spawn_with(
        directory.path().join("previews.sqlite3"),
        egui::Context::default(),
        thumbnail,
        ShownFiles::default(),
    );
    for name in ["1.png", "2.png"] {
        tx.try_send(edited_png(directory.path(), name)?)?;
    }
    for _ in 0..2 {
        assert!(rx.recv_timeout(Duration::from_secs(30))?.image.is_some());
    }
    Ok(())
}

#[test]
fn closing_cancels_the_edited_preview_under_way() -> anyhow::Result<()> {
    static STARTED: AtomicBool = AtomicBool::new(false);
    fn render(_: &Path, _: &EditSource, cancel: Cancel<'_>) -> anyhow::Result<image::RgbImage> {
        STARTED.store(true, Ordering::Relaxed);
        // A render that only a cancel ends.
        while !cancel.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        anyhow::bail!("Cancelled")
    }
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("photo.png");
    image::RgbImage::new(16, 16).save(&source)?;
    let wanted = Wanted::default();
    wanted.lock().unwrap().insert(PhotoId(1));
    let (tx, rx, thread) = spawn_edited_with(
        directory.path().join("previews.sqlite3"),
        wanted,
        egui::Context::default(),
        render,
    );
    let job = |ticket| EditJob::Render {
        id: PhotoId(1),
        ticket,
        path: source.clone(),
        source: EditSource::Recipe("{}".into()),
        ahead: false,
    };
    tx.send(job(0))?;
    let started = std::time::Instant::now();
    while !STARTED.load(Ordering::Relaxed) {
        assert!(started.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(1));
    }
    // Queued behind it, and never rendered.
    tx.send(job(1))?;
    tx.close();
    let stopping = crate::app::task::Stopping::new(Some(thread));
    let waited = crate::app::task::wait_for(vec![stopping], Duration::from_secs(20));
    assert_eq!(waited.detached, 0);
    assert!(matches!(
        rx.try_recv(),
        Ok(EditResult::Skipped(PhotoId(1), 0))
    ));
    assert!(rx.try_recv().is_err());
    Ok(())
}

/// A thumbnail asked for and no longer shown when a worker comes to it (the
/// grid moved on to another folder) is skipped, not made and not failed; one
/// still shown is made. Before the views have said what they show, every one
/// is made.
#[test]
fn thumbnails_no_longer_shown_are_skipped() -> anyhow::Result<()> {
    static MADE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    fn thumbnail(path: &Path) -> anyhow::Result<image::RgbImage> {
        MADE.lock().unwrap().push(path.to_path_buf());
        Ok(image::open(path)?.to_rgb8())
    }
    let directory = tempfile::tempdir()?;
    let [left, shown, first] = ["left.png", "shown.png", "first.png"]
        .map(|name| edited_png(directory.path(), name))
        .map(Result::unwrap);
    let wanted = ShownFiles::default();
    let (tx, rx, _) = spawn_with(
        directory.path().join("previews.sqlite3"),
        egui::Context::default(),
        thumbnail,
        wanted.clone(),
    );
    // Nothing said yet: made.
    tx.try_send(first)?;
    let made = rx.recv_timeout(Duration::from_secs(30))?;
    assert!(made.image.is_some() && !made.skipped);
    *wanted.lock().unwrap() = Some([shown.clone()].into_iter().collect());
    tx.try_send(left.clone())?;
    tx.try_send(shown.clone())?;
    let mut results = [
        rx.recv_timeout(Duration::from_secs(30))?,
        rx.recv_timeout(Duration::from_secs(30))?,
    ];
    results.sort_by(|a, b| a.path.cmp(&b.path));
    let [skipped, made] = results;
    assert_eq!(skipped.path, left);
    assert!(skipped.skipped && skipped.image.is_none());
    assert_eq!(made.path, shown);
    assert!(made.image.is_some() && !made.skipped);
    assert!(!MADE.lock().unwrap().contains(&left));
    // A skipped thumbnail is neither done nor failed for the progress line.
    let mut progress = Progress::default();
    progress.queued();
    progress.finish(&skipped);
    assert_eq!(progress.failed, 0);
    Ok(())
}
