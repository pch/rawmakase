//! The library used from outside RAWmakase's workspace, on the color-chart
//! DNGs (`tests/corpus/charts`): every one opens, decodes, renders and
//! exports, and an export's pixels are the desktop's.
use rawmakase_core::{
    CacheLimits, Cancel, Context, Decode, ErrorKind, ExportSettings, Parameter, Recipe, View,
};
use std::path::{Path, PathBuf};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn charts() -> Vec<PathBuf> {
    let mut charts: Vec<_> = std::fs::read_dir(repository().join("tests/corpus/charts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "dng"))
        .collect();
    charts.sort();
    assert!(!charts.is_empty(), "the chart DNGs");
    charts
}

/// A context of its own, in `dir`: empty data and cache folders.
fn context(dir: &Path) -> Context {
    Context::builder()
        .data_dirs([dir.join("data")])
        .cache(
            dir.join("cache"),
            CacheLimits::default(),
            "rawmakase-embed test",
        )
        .build()
        .unwrap()
}

/// The edit the tests make: a slider, white balance and a curve point.
fn edited(photo: &rawmakase_core::Photo) -> Recipe {
    let mut recipe = photo.starting_recipe().unwrap();
    photo.set(&mut recipe, Parameter::Exposure, 0.4).unwrap();
    photo.set(&mut recipe, Parameter::Contrast, 0.2).unwrap();
    photo
        .set(&mut recipe, Parameter::Temperature, 4800.)
        .unwrap();
    recipe.curve.insert([0.3, 0.24]);
    recipe
}

#[test]
fn every_chart_opens_decodes_renders_and_exports() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(dir.path());
    let mut renderer = ctx.renderer();
    for chart in charts() {
        let name = chart.file_name().unwrap().to_string_lossy().into_owned();
        let photo = ctx.open(&chart, Cancel::NEVER).unwrap();
        let recipe = edited(&photo);
        let half = photo.decode(Decode::Half, Cancel::NEVER).unwrap();
        let fit = renderer
            .render(&half, &recipe, View::Fit(512), Cancel::NEVER)
            .unwrap();
        assert!(fit.width().max(fit.height()) <= 512, "{name}");
        assert_eq!(
            fit.rgba8().len(),
            (fit.width() * fit.height() * 4) as usize,
            "{name}"
        );
        let full = photo.decode(Decode::Full, Cancel::NEVER).unwrap();
        let size = full.output_size(&recipe);
        let out = dir.path().join(format!("{name}.png"));
        ctx.export(
            &photo,
            &recipe,
            &ExportSettings::default(),
            &out,
            false,
            Cancel::NEVER,
            |_| {},
        )
        .unwrap();
        assert_eq!(image::image_dimensions(&out).unwrap(), size, "{name}");
    }
}

#[test]
fn what_an_app_acts_on_has_its_kind() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(dir.path());
    let e = ctx
        .open(dir.path().join("none.dng"), Cancel::NEVER)
        .unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Missing, "{e:#}");
    let chart = &charts()[0];
    let photo = ctx.open(chart, Cancel::NEVER).unwrap();
    let recipe = photo.starting_recipe().unwrap();
    let out = dir.path().join("out.jpg");
    let export = |replace| {
        ctx.export(
            &photo,
            &recipe,
            &ExportSettings::default(),
            &out,
            replace,
            Cancel::NEVER,
            |_| {},
        )
    };
    export(false).unwrap();
    assert_eq!(export(false).unwrap_err().kind(), ErrorKind::Exists);
    export(true).unwrap();
}

/// The plan's fourth measure: the library and `rawmakase render` export the
/// same photo with the same saved recipe to a lossless format, in sRGB, each
/// with an empty data folder of its own, and the decoded pixels are equal.
/// Needs the desktop's binary, which CI builds first.
#[test]
#[ignore = "needs the desktop's binary: cargo build --bin rawmakase"]
fn an_export_has_the_desktop_s_pixels() {
    let desktop = ["release", "debug"]
        .map(|profile| {
            let name = format!("rawmakase{}", std::env::consts::EXE_SUFFIX);
            repository().join("target").join(profile).join(name)
        })
        .into_iter()
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        // The one built last: what `make check` or CI has just built, never a
        // release build left from before.
        .max_by_key(|(built, _)| *built)
        .map(|(_, p)| p)
        .expect("rawmakase built in the repository's target folder");
    let dir = tempfile::tempdir().unwrap();
    let chart = charts()
        .into_iter()
        .find(|c| c.ends_with("synthetic-d65.dng"))
        .unwrap();
    let ctx = context(&dir.path().join("library"));
    let photo = ctx.open(&chart, Cancel::NEVER).unwrap();
    let recipe = edited(&photo);
    let saved = dir.path().join("recipe.json");
    photo.save_recipe(&recipe, &saved).unwrap();
    let ours = dir.path().join("library.tif");
    let loaded = ctx.load_recipe(&saved).unwrap();
    ctx.export(
        &photo,
        &loaded,
        &ExportSettings::default(),
        &ours,
        false,
        Cancel::NEVER,
        |_| {},
    )
    .unwrap();
    // The desktop's own folders, empty, for its process alone.
    let home = dir.path().join("desktop");
    let theirs = dir.path().join("desktop.tif");
    let status = std::process::Command::new(&desktop)
        .arg("render")
        .arg(&chart)
        .arg(&theirs)
        .arg("--recipe")
        .arg(&saved)
        .env("RAWMAKASE_DATA_DIR", home.join("data"))
        .env("RAWMAKASE_CACHE_DIR", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("xdg"))
        .status()
        .unwrap();
    assert!(status.success());
    let pixels = |p: &Path| image::open(p).unwrap().into_rgb16();
    let (ours, theirs) = (pixels(&ours), pixels(&theirs));
    assert_eq!(ours.dimensions(), theirs.dimensions());
    assert!(ours.as_raw() == theirs.as_raw(), "the desktop's pixels");
}
