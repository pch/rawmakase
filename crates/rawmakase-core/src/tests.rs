//! The library used as an app uses it, on a color-chart DNG.
use super::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn chart() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng")
}

fn context() -> Context {
    Context::builder().build().unwrap()
}

/// The chart copied into `dir`, for a test that changes or replaces it.
fn chart_in(dir: &Path) -> PathBuf {
    let copy = dir.join("chart.dng");
    std::fs::copy(chart(), &copy).unwrap();
    copy
}

/// The handles an app shares between threads can be, and a renderer can move
/// to the thread that keeps it.
#[test]
fn handles_are_send_and_sync_where_the_docs_say() {
    fn shared<T: Send + Sync>() {}
    fn movable<T: Send>() {}
    shared::<Context>();
    shared::<Photo>();
    shared::<Image>();
    shared::<Error>();
    shared::<Mailbox<Recipe>>();
    movable::<Renderer>();
    movable::<Frame>();
}

#[test]
fn a_photo_opens_with_its_metadata_and_a_valid_starting_recipe() {
    let photo = context().open(chart(), Cancel::NEVER).unwrap();
    assert_eq!(photo.path(), chart());
    assert!(photo.metadata().width > 0);
    let recipe = photo.starting_recipe().unwrap();
    recipe.validate().unwrap();
    assert_eq!(recipe.exposure, 0.);
}

#[test]
fn a_file_that_is_not_there_is_missing_and_one_not_a_photo_unsupported() {
    let e = context()
        .open("/nonexistent/photo.dng", Cancel::NEVER)
        .unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Missing, "{e:#}");
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("notes.dng");
    std::fs::write(&text, b"not a photo").unwrap();
    let e = context().open(&text, Cancel::NEVER).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Unsupported, "{e:#}");
}

/// A decode reads the file again, and refuses one that is no longer the file
/// opened, so a replaced photo is never shown as the one opened.
#[test]
fn a_decode_refuses_a_file_changed_since_it_was_opened() {
    let dir = tempfile::tempdir().unwrap();
    let path = chart_in(dir.path());
    let photo = context().open(&path, Cancel::NEVER).unwrap();
    photo.decode(Decode::Half, Cancel::NEVER).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.push(0);
    std::fs::write(&path, bytes).unwrap();
    let e = photo.decode(Decode::Half, Cancel::NEVER).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Changed, "{e:#}");
    std::fs::remove_file(&path).unwrap();
    let e = photo.decode(Decode::Full, Cancel::NEVER).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Missing, "{e:#}");
}

/// A cancelled open or decode returns the cancelled kind, the fast paths
/// (a rendered file, the cache's copy) included; a file only touched since
/// it was opened is still the photo.
#[test]
fn cancel_is_heard_everywhere_and_a_touched_file_is_the_same_photo() {
    let dir = tempfile::tempdir().unwrap();
    let path = chart_in(dir.path());
    let ctx = Context::builder()
        .cache(dir.path().join("cache"), CacheLimits::default(), "test")
        .build()
        .unwrap();
    let stop = AtomicBool::new(true);
    let e = ctx.open(&path, Cancel::new(&stop)).unwrap_err();
    assert!(e.is_cancelled(), "{e:#}");
    let photo = ctx.open(&path, Cancel::NEVER).unwrap();
    photo.decode(Decode::Full, Cancel::NEVER).unwrap();
    let e = photo.decode(Decode::Full, Cancel::new(&stop)).unwrap_err();
    assert!(e.is_cancelled(), "a cache hit is cancelled too: {e:#}");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(later)
        .unwrap();
    photo.decode(Decode::Half, Cancel::NEVER).unwrap();
}

/// What a view's size is said to be is what the renderer gives it, for a
/// straightened photo whose sizes round, at any edge.
#[test]
fn an_image_s_size_at_a_view_is_its_frame_s() {
    let ctx = context();
    let photo = ctx.open(chart(), Cancel::NEVER).unwrap();
    let image = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
    let mut recipe = photo.starting_recipe().unwrap();
    recipe.straighten = 7.3;
    let mut renderer = ctx.renderer();
    for edge in [0, 101, 333, 640] {
        let frame = renderer
            .render(&image, &recipe, View::Fit(edge), Cancel::NEVER)
            .unwrap();
        assert_eq!(
            image.size_in(&recipe, View::Fit(edge)).unwrap(),
            (frame.width(), frame.height()),
            "Fit({edge})"
        );
    }
    let draft = renderer
        .render_draft(&image, &recipe, View::Fit(400), Cancel::NEVER)
        .unwrap();
    assert!(
        !draft.is_draft(),
        "a photo too small for a draft is rendered whole"
    );
}

/// The raw defaults are the context's: a photo starts from the preset they
/// name.
#[test]
fn a_photo_starts_from_the_context_s_raw_defaults() {
    let adobe = context()
        .open(chart(), Cancel::NEVER)
        .unwrap()
        .starting_recipe()
        .unwrap();
    let (presets, _) = context().presets();
    let starts = presets
        .iter()
        .filter(|p| p.id.starts_with("builtin:"))
        .map(|preset| {
            let defaults = RawDefaults {
                master: rawmakase_interop::raw_defaults::DefaultChoice::Preset {
                    id: preset.id.clone(),
                    name: preset.name.clone(),
                },
                ..Default::default()
            };
            let ctx = Context::builder().raw_defaults(defaults).build().unwrap();
            let photo = ctx.open(chart(), Cancel::NEVER).unwrap();
            photo.starting_recipe().unwrap()
        });
    let mut starts = starts.peekable();
    assert!(starts.peek().is_some(), "built-in presets");
    assert!(
        starts.any(|start| start != adobe),
        "a built-in preset as the defaults changes where a photo starts"
    );
}

/// Temp and Tint bring the white balance the renderer multiplies by along; a
/// value a recipe may not hold changes nothing.
#[test]
fn setting_a_parameter_follows_the_models_rules() {
    let photo = context().open(chart(), Cancel::NEVER).unwrap();
    let mut recipe = photo.starting_recipe().unwrap();
    let before = recipe.wb;
    photo
        .set(&mut recipe, Parameter::Temperature, 3200.)
        .unwrap();
    assert_eq!(recipe.temperature, 3200.);
    assert_ne!(recipe.wb, before, "white balance follows Temp");
    assert_eq!(photo.value(&recipe, Parameter::Temperature), 3200.);
    photo.set(&mut recipe, Parameter::Exposure, 1.25).unwrap();
    assert_eq!(recipe.exposure, 1.25);
    let kept = recipe.clone();
    for value in [f32::NAN, 100., -100.] {
        let e = photo
            .set(&mut recipe, Parameter::Exposure, value)
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Invalid);
    }
    assert_eq!(recipe, kept);
    assert_eq!(
        Parameter::named("temperature"),
        Some(Parameter::Temperature)
    );
}

/// A control carries the model's range, step and formatting, so a front end
/// hard-codes none of them.
#[test]
fn a_control_describes_its_slider_from_the_model() {
    let photo = context().open(chart(), Cancel::NEVER).unwrap();
    let mut recipe = photo.starting_recipe().unwrap();
    photo.set(&mut recipe, Parameter::Exposure, 0.5).unwrap();
    photo.set(&mut recipe, Parameter::Contrast, -0.35).unwrap();
    let exposure = photo.control(&recipe, Parameter::Exposure);
    assert_eq!(
        (exposure.id, exposure.label, exposure.min, exposure.max),
        ("exposure", "Exposure", -5., 5.)
    );
    assert_eq!((exposure.step, exposure.scale), (0.05, Scale::Linear));
    assert_eq!((exposure.value, exposure.text.as_str()), (0.5, "+0.50"));
    let contrast = photo.control(&recipe, Parameter::Contrast);
    assert_eq!((contrast.step, contrast.text.as_str()), (0.01, "-35"));
    let temp = photo.control(&recipe, Parameter::Temperature);
    assert_eq!(
        (temp.id, temp.label, temp.scale),
        ("temperature", "Temp", Scale::Reciprocal)
    );
}

#[test]
fn a_decode_gives_its_statistics_and_decodes_again() {
    let photo = context().open(chart(), Cancel::NEVER).unwrap();
    let full = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
    let stats = full.stats();
    assert_eq!((stats.width, stats.height), (970, 742));
    assert!(!stats.reduced);
    assert!(stats.highest_level > 0.5, "{stats:?}");
    assert_eq!(full.source(), chart());
    let half = photo.decode(Decode::Half, Cancel::NEVER).unwrap();
    let stats = half.stats();
    assert_eq!((stats.width, stats.height, stats.reduced), (485, 371, true));
}

/// A full decode is kept in the context's cache folder, under its build id,
/// and the cache's copy renders the same pixels.
#[test]
fn a_full_decode_is_kept_in_the_contexts_cache() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Context::builder()
        .cache(dir.path(), CacheLimits::default(), "test-1")
        .build()
        .unwrap();
    let render = |ctx: &Context| {
        let photo = ctx.open(chart(), Cancel::NEVER).unwrap();
        let image = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
        let recipe = photo.starting_recipe().unwrap();
        ctx.render(&image, &recipe, View::Fit(0), Cancel::NEVER)
            .unwrap()
            .rgb16()
    };
    let decoded = render(&ctx);
    let entries = std::fs::read_dir(dir.path().join("decoded"))
        .unwrap()
        .count();
    assert_eq!(entries, 1);
    assert_eq!(render(&ctx), decoded, "the cache's copy");
    assert_eq!(render(&context()), decoded, "no cache");
}

#[test]
fn a_renderer_renders_the_whole_photo_at_a_size_and_a_region_at_full_size() {
    let ctx = context();
    let photo = ctx.open(chart(), Cancel::NEVER).unwrap();
    let image = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
    let recipe = photo.starting_recipe().unwrap();
    let (width, height) = image.output_size(&recipe).unwrap();
    let mut renderer = ctx.renderer();
    let fit = renderer
        .render(&image, &recipe, View::Fit(400), Cancel::NEVER)
        .unwrap();
    assert_eq!(fit.width().max(fit.height()), 400);
    assert_eq!(
        (fit.width(), fit.height()),
        image.size_in(&recipe, View::Fit(400)).unwrap()
    );
    assert!(!fit.is_draft());
    let rgba = fit.rgba8();
    let rgb = fit.rgb8();
    assert_eq!(rgba.len(), (fit.width() * fit.height() * 4) as usize);
    assert!(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .zip(rgb.as_chunks::<3>().0)
            .all(|(a, b)| a[..3] == *b && a[3] == 255)
    );
    let full = renderer
        .render(&image, &recipe, View::Fit(0), Cancel::NEVER)
        .unwrap();
    assert_eq!((full.width(), full.height()), (width, height));
    let region = renderer
        .render(
            &image,
            &recipe,
            View::Region([100, 50, 200, 120]),
            Cancel::NEVER,
        )
        .unwrap();
    assert_eq!((region.width(), region.height()), (200, 120));
}

/// A draft is due only while the view shown is edited, and a quick full
/// frame needs none: the desktop's rule. The chart is too small for a whole
/// photo's draft; a region's draft covers it at half resolution or less.
#[test]
fn a_draft_is_due_while_a_view_is_edited_unless_its_frames_are_quick() {
    let ctx = context();
    let photo = ctx.open(chart(), Cancel::NEVER).unwrap();
    let image = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
    let recipe = photo.starting_recipe().unwrap();
    let mut renderer = ctx.renderer();
    let region = View::Region([0, 0, 600, 400]);
    assert_eq!(
        renderer.draft_due(&image, &recipe, region),
        None,
        "new view"
    );
    renderer
        .render(&image, &recipe, region, Cancel::NEVER)
        .unwrap();
    // Whether the frame was quick depends on the machine: a draft is due
    // exactly when it wasn't.
    let due = renderer.draft_due(&image, &recipe, region);
    if let Some(size) = due {
        assert_eq!(size, (600, 400));
        let draft = renderer
            .render_draft(&image, &recipe, region, Cancel::NEVER)
            .unwrap();
        assert!(draft.is_draft());
        assert!(draft.width() <= 300 && draft.height() <= 200, "{draft:?}");
    }
    assert_eq!(
        renderer.draft_due(&image, &recipe, View::Fit(400)),
        None,
        "another view, and a small photo"
    );
}

/// A render whose `Cancel` is set returns the cancelled kind, whatever the
/// engine's own text for it; an edit the recipe may not hold is invalid.
#[test]
fn a_cancelled_render_and_an_invalid_recipe_have_their_kinds() {
    let ctx = context();
    let photo = ctx.open(chart(), Cancel::NEVER).unwrap();
    let image = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
    let mut recipe = photo.starting_recipe().unwrap();
    let mut renderer = ctx.renderer();
    let stop = AtomicBool::new(true);
    let e = renderer
        .render(&image, &recipe, View::Fit(400), Cancel::new(&stop))
        .unwrap_err();
    assert!(e.is_cancelled(), "{e:#}");
    renderer
        .render(&image, &recipe, View::Fit(400), Cancel::NEVER)
        .unwrap();
    recipe.exposure = 99.;
    let e = renderer
        .render(&image, &recipe, View::Fit(400), Cancel::NEVER)
        .unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Invalid);
}

#[test]
fn an_edit_and_a_curve_point_change_the_pixels() {
    let ctx = context();
    let photo = ctx.open(chart(), Cancel::NEVER).unwrap();
    let image = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
    let mut recipe = photo.starting_recipe().unwrap();
    let mut renderer = ctx.renderer();
    let mut render = |recipe: &Recipe| {
        renderer
            .render(&image, recipe, View::Fit(300), Cancel::NEVER)
            .unwrap()
            .rgb8()
    };
    let start = render(&recipe);
    photo.set(&mut recipe, Parameter::Exposure, 1.).unwrap();
    let brighter = render(&recipe);
    assert_ne!(brighter, start);
    let Inserted::Added(point) = recipe.curve.insert([0.5, 0.3]) else {
        panic!("a point added");
    };
    assert_eq!(recipe.curve.sampled(3)[1][0], 0.5);
    let curved = render(&recipe);
    assert_ne!(curved, brighter);
    assert!(recipe.curve.remove(point));
    assert_eq!(render(&recipe), brighter);
}

/// Auto measures the decode, and a preset with Auto settings applied with
/// the decode gets them measured.
#[test]
fn auto_tone_measures_the_photo() {
    let photo = context().open(chart(), Cancel::NEVER).unwrap();
    let image = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
    let recipe = photo.starting_recipe().unwrap();
    let auto = photo.auto_tone(&recipe, &image, Cancel::NEVER).unwrap();
    auto.validate().unwrap();
    assert_eq!(auto.temperature, recipe.temperature, "white balance kept");
    let balanced = photo
        .auto_white_balance(&recipe, &image, Cancel::NEVER)
        .unwrap();
    balanced.validate().unwrap();
}

#[cfg(feature = "export")]
mod export {
    use super::*;

    /// An export writes the format its extension names, never over a file
    /// unless asked.
    #[test]
    fn an_export_writes_the_photo_and_never_over_a_file_unless_asked() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context();
        let photo = ctx.open(chart(), Cancel::NEVER).unwrap();
        let image = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
        let mut recipe = photo.starting_recipe().unwrap();
        photo.set(&mut recipe, Parameter::Exposure, 0.5).unwrap();
        recipe.curve.insert([0.3, 0.22]);
        let (width, height) = image.output_size(&recipe).unwrap();
        let settings = ExportSettings::default();
        let jpeg = dir.path().join("out.jpg");
        let progress = std::sync::Mutex::new(Vec::new());
        ctx.export(
            &photo,
            &recipe,
            &settings,
            &jpeg,
            false,
            Cancel::NEVER,
            |p| {
                progress.lock().unwrap().push(p);
            },
        )
        .unwrap();
        assert_eq!(::image::image_dimensions(&jpeg).unwrap(), (width, height));
        assert_eq!(progress.lock().unwrap().last(), Some(&1.));
        let e = ctx
            .export(
                &photo,
                &recipe,
                &settings,
                &jpeg,
                false,
                Cancel::NEVER,
                |_| {},
            )
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Exists, "{e:#}");
        ctx.export(
            &photo,
            &recipe,
            &settings,
            &jpeg,
            true,
            Cancel::NEVER,
            |_| {},
        )
        .unwrap();
        let tiff = dir.path().join("out.tif");
        ctx.export(
            &photo,
            &recipe,
            &settings,
            &tiff,
            false,
            Cancel::NEVER,
            |_| {},
        )
        .unwrap();
        assert_eq!(::image::image_dimensions(&tiff).unwrap(), (width, height));
        // A saved recipe read back exports the same pixels.
        let saved = dir.path().join("recipe.json");
        photo.save_recipe(&recipe, &saved).unwrap();
        let loaded = ctx.load_recipe(&saved).unwrap();
        let reread = dir.path().join("reread.tif");
        ctx.export(
            &photo,
            &loaded,
            &settings,
            &reread,
            false,
            Cancel::NEVER,
            |_| {},
        )
        .unwrap();
        let pixels = |path: &Path| ::image::open(path).unwrap().into_rgb16().into_raw();
        assert_eq!(pixels(&tiff), pixels(&reread));
        let e = ctx.load_recipe(dir.path().join("none.json")).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Missing, "{e:#}");
        // The settings' own format doesn't decide the file's: a ProPhoto
        // export, which only a TIFF holds, goes to a .tif from JPEG settings.
        let mut prophoto = settings;
        prophoto.color_space = OutputSpace::ProPhoto;
        let wide = dir.path().join("wide.tif");
        ctx.export(
            &photo,
            &recipe,
            &prophoto,
            &wide,
            false,
            Cancel::NEVER,
            |_| {},
        )
        .unwrap();
        assert!(wide.is_file());
    }

    #[test]
    fn exports_refuse_what_they_cannot_write() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context();
        let photo = ctx.open(chart(), Cancel::NEVER).unwrap();
        let recipe = photo.starting_recipe().unwrap();
        let settings = ExportSettings::default();
        let export = |recipe: &Recipe, out: &Path, cancel| {
            ctx.export(&photo, recipe, &settings, out, false, cancel, |_| {})
        };
        let e = export(&recipe, &dir.path().join("sub/out.bmp"), Cancel::NEVER).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Invalid, "{e:#}");
        assert!(
            !dir.path().join("sub").exists(),
            "refused before anything is made"
        );
        let e = export(&recipe, photo.path(), Cancel::NEVER).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Invalid, "the photo's own file: {e:#}");
        let mut denoised = recipe.clone();
        denoised.ai_denoise.enabled = true;
        denoised.ai_denoise.amount = 1.;
        let e = export(&denoised, &dir.path().join("out.jpg"), Cancel::NEVER).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Unsupported, "{e:#}");
        let stop = AtomicBool::new(true);
        let out = dir.path().join("cancelled.jpg");
        let e = export(&recipe, &out, Cancel::new(&stop)).unwrap_err();
        assert!(e.is_cancelled(), "{e:#}");
        assert!(!out.exists());
    }
}
