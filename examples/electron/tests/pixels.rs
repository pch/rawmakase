//! The desktop's pixels, the library plan's fourth measure: the example's
//! export and `rawmakase render --recipe` of the same photo with the recipe
//! the example saved decode to equal pixels.
//!
//! Each program has an empty data folder of its own, so neither sees the
//! profiles and presets installed on this computer. The example is given its
//! folders as values (`headless export --home`); the desktop's command line
//! still reads them from the environment of its own process.
//!
//! Needs the desktop binary, so it is ignored by default:
//!
//! ```sh
//! cargo build --bin rawmakase                # in the repository's root
//! cargo test --test pixels -- --ignored      # here
//! ```
use std::path::{Path, PathBuf};
use std::process::Command;

fn desktop() -> PathBuf {
    ["release", "debug"]
        .map(|profile| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join(profile)
                .join(format!("rawmakase{}", std::env::consts::EXE_SUFFIX))
        })
        .into_iter()
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        // The one built last: what `make check` or CI has just built, never a
        // release build left from before.
        .max_by_key(|(built, _)| *built)
        .map(|(_, p)| p)
        .expect("rawmakase built in the repository's target folder: `cargo build --bin rawmakase` in its root")
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The desktop's command line, with empty folders of its own under `home`.
fn desktop_render(home: &Path) -> Command {
    let mut command = Command::new(desktop());
    command
        .arg("render")
        .env("RAWMAKASE_DATA_DIR", home.join("data"))
        .env("RAWMAKASE_CACHE_DIR", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("xdg"));
    command
}

fn pixels(path: &Path) -> (u32, u32, Vec<u16>) {
    let image = image::open(path).unwrap();
    assert_eq!(image.color(), image::ColorType::Rgb16, "{}", path.display());
    let image = image.into_rgb16();
    (image.width(), image.height(), image.into_raw())
}

fn compare(chart: &str) {
    let chart = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/charts")
        .join(chart);
    let dir = tempfile::tempdir().unwrap();
    let at = |name: &str| dir.path().join(name);
    // The example: opened, edited through the editor the addon wraps, the
    // recipe saved and the photo exported as a 16-bit sRGB TIFF.
    let mut example = Command::new(env!("CARGO_BIN_EXE_headless"));
    example.arg("export").arg("--home").arg(at("example"));
    for (control, value) in [
        ("temperature", "5200"),
        ("tint", "12"),
        ("exposure", "0.35"),
        ("contrast", "0.2"),
        ("highlights", "-0.4"),
        ("shadows", "0.3"),
        ("whites", "0.1"),
        ("blacks", "-0.15"),
    ] {
        example.arg("--set").arg(format!("{control}={value}"));
    }
    example.args(["--curve", "0.25,0.18", "--curve", "0.7,0.78"]);
    example.arg("--recipe").arg(at("recipe.json"));
    example.arg(&chart).arg(at("example.tif"));
    run(&mut example);
    run(desktop_render(&at("desktop"))
        .arg("--recipe")
        .arg(at("recipe.json"))
        .arg(&chart)
        .arg(at("desktop.tif")));

    let (width, height, ours) = pixels(&at("example.tif"));
    let (desktop_width, desktop_height, theirs) = pixels(&at("desktop.tif"));
    assert_eq!((width, height), (desktop_width, desktop_height));
    let differing = ours.iter().zip(&theirs).filter(|(a, b)| a != b).count();
    let largest = ours
        .iter()
        .zip(&theirs)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    println!(
        "{}: {width}×{height}, {} samples, {differing} differ, largest difference {largest} of 65535",
        chart.file_name().unwrap().to_string_lossy(),
        ours.len()
    );
    assert_eq!(differing, 0, "the example's pixels are not the desktop's");
    // The edit did something: these are not two default renders.
    let plain = at("plain.tif");
    run(desktop_render(&at("plain")).arg(&chart).arg(&plain));
    assert_ne!(pixels(&plain).2, theirs);
}

#[test]
#[ignore = "needs the desktop binary; see this file's first lines"]
fn the_examples_export_has_the_desktops_pixels() {
    compare("synthetic-d65.dng");
    compare("sony-ilce-7m4-d65.dng");
    compare("fujifilm-x-t5-d65.dng");
}
