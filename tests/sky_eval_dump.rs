//! Select Sky's coverage for a list of photos, for `scripts/sky-eval`. Ignored by default.
//!
//! - `RAWMAKASE_SKY_LIST`: a text file, one photo path per line (RAW, JPEG, PNG or TIFF);
//! - `RAWMAKASE_SKY_MODEL`: the installed sky model's folder;
//! - `RAWMAKASE_ORT_LIB`: an ONNX Runtime library;
//! - `RAWMAKASE_SKY_EDGE`: the selection input's long edge (the app's: 1280);
//! - `RAWMAKASE_SKY_OUT`: where each photo's coverage goes, as `<stem>.png`.
//!
//! The file is self-contained so `scripts/sky-eval/run.sh` can copy it into a worktree of an
//! older commit and run the same harness there; it drops the lines between the
//! `sky-eval: timings` markers, which need `SkySession::last_timings`.
//!
//! Prints `<photo> <w>x<h> <ms> ms` per photo and, where the session reports them,
//! `timing <photo> model <ms> edge <ms> modes <ms> sparse <0|1> dense <0|1>`.
use image::GrayImage;
use rawmakase::{camera_data, model::recipe::Recipe};
use rawmakase_cancel::Cancel;
use rawmakase_inference::LoadOptions;
use rawmakase_selection::{RgbImage, Sky};
use std::path::PathBuf;
use std::time::Instant;

fn var(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set")))
}

#[test]
#[ignore = "needs the sky model, ONNX Runtime and a photo list (RAWMAKASE_SKY_* variables)"]
fn sky_eval_dump() {
    let out = var("RAWMAKASE_SKY_OUT");
    std::fs::create_dir_all(&out).unwrap();
    let edge: u32 = std::env::var("RAWMAKASE_SKY_EDGE")
        .expect("RAWMAKASE_SKY_EDGE is not set")
        .parse()
        .expect("RAWMAKASE_SKY_EDGE is a number");
    let list = std::fs::read_to_string(var("RAWMAKASE_SKY_LIST")).unwrap();
    let cancel = Cancel::NEVER;
    let mut session = Sky::load_with(&var("RAWMAKASE_SKY_MODEL"), &LoadOptions::default()).unwrap();
    for line in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let path = PathBuf::from(line);
        let image = match rawmakase::photo::open_source(&path, cancel)
            .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()))
        {
            rawmakase::photo::Opened::Raw(raw) => raw
                .develop(camera_data::Decode::full(Default::default()), cancel)
                .unwrap_or_else(|e| panic!("{}: {e:?}", path.display())),
            rawmakase::photo::Opened::Rendered(image) => image,
        };
        let input =
            rawmakase::develop::masks::selection_input(&image, &Recipe::default(), edge, cancel)
                .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        let rgb = RgbImage {
            width: input.width as usize,
            height: input.height as usize,
            data: input.rgb8(),
        };
        if std::env::var_os("RAWMAKASE_SKY_SAVE_INPUT").is_some() {
            // The photo exactly as the model saw it, for checking its framing.
            image::RgbImage::from_raw(rgb.width as u32, rgb.height as u32, rgb.data.clone())
                .unwrap_or_else(|| panic!("{}: the input does not fit its size", path.display()))
                .save(out.join(format!(
                    "{}.input.jpg",
                    path.file_stem().unwrap().to_string_lossy()
                )))
                .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        }
        let started = Instant::now();
        let cov = session
            .select(&rgb, cancel)
            .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        println!(
            "{} {}x{} {} ms",
            path.display(),
            cov.width,
            cov.height,
            started.elapsed().as_millis()
        );
        // sky-eval: timings start
        if let Some(t) = session.last_timings() {
            println!(
                "timing {} model {} edge {} modes {} sparse {} dense {}",
                path.display(),
                t.model.as_millis(),
                t.edge.as_millis(),
                t.modes.as_millis(),
                u8::from(t.sparse),
                u8::from(t.dense)
            );
        }
        // sky-eval: timings end
        GrayImage::from_raw(cov.width as u32, cov.height as u32, cov.data.clone())
            .unwrap_or_else(|| panic!("{}: the coverage does not fit its size", path.display()))
            .save(out.join(format!(
                "{}.png",
                path.file_stem().unwrap().to_string_lossy()
            )))
            .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
    }
}
