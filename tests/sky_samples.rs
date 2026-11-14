//! Select Sky against Lightroom's on private sample photos. Ignored by default; it needs:
//!
//! - `RAWMAKASE_SKY_SAMPLES`: a folder with `samples.json`, a list of
//!   `{"raw", "unmasked", "masked", "ev"}`: the RAW and two Lightroom JPEG exports that
//!   differ only by a Sky mask whose Exposure is `ev`;
//! - `RAWMAKASE_SKY_MODEL`: the installed sky model's folder;
//! - `RAWMAKASE_ORT_LIB`: an ONNX Runtime library;
//! - `RAWMAKASE_SKY_OUT`: where to write each photo's `ours.png`, `lightroom.png` and
//!   `diff.png`;
//! - `RAWMAKASE_SKY_EDGE` (optional): the selection input's long edge, 1280 (the app's) by default.
//!
//! Lightroom's mask is recovered from the exports' brightness ratio, through its tone
//! curve and JPEG: a reference shape, not ground truth. Pixels near black or white in
//! either export are left out.
use image::{GrayImage, Luma, Rgb, RgbImage as Image, imageops::FilterType};
use rawmakase::{camera_data, model::recipe::Recipe};
use rawmakase_cancel::Cancel;
use rawmakase_inference::LoadOptions;
use rawmakase_selection::{RgbImage, Sky};
use std::path::PathBuf;
use std::time::Instant;

fn var(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set")))
}

#[derive(serde::Deserialize)]
struct Sample {
    raw: String,
    unmasked: String,
    masked: String,
    ev: f32,
}

fn linear(v: u8) -> f32 {
    let c = f32::from(v) / 255.;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Lightroom's coverage at `w` x `h`, `None` where it cannot be told.
fn lightroom(unmasked: &Image, masked: &Image, ev: f32, w: u32, h: u32) -> Vec<Option<f32>> {
    let u = image::imageops::resize(unmasked, w, h, FilterType::Triangle);
    let m = image::imageops::resize(masked, w, h, FilterType::Triangle);
    let luma =
        |p: &Rgb<u8>| 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
    let lin = |p: &Rgb<u8>| 0.2126 * linear(p[0]) + 0.7152 * linear(p[1]) + 0.0722 * linear(p[2]);
    u.pixels()
        .zip(m.pixels())
        .map(|(a, b)| {
            let clipped = |p| !(5.0..250.0).contains(&luma(p));
            if clipped(a) || clipped(b) {
                None
            } else {
                Some(((lin(b) / lin(a)).log2() / ev).clamp(0., 1.))
            }
        })
        .collect()
}

/// Mean of `values` over the `(2r+1)²` window around each pixel, clipped at the frame.
fn box_mean(values: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut sum = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            sum[(y + 1) * (w + 1) + x + 1] =
                values[y * w + x] as f64 + sum[y * (w + 1) + x + 1] + sum[(y + 1) * (w + 1) + x]
                    - sum[y * (w + 1) + x];
        }
    }
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
            let s = sum[y1 * (w + 1) + x1] - sum[y0 * (w + 1) + x1] - sum[y1 * (w + 1) + x0]
                + sum[y0 * (w + 1) + x0];
            (s / ((x1 - x0) * (y1 - y0)) as f64) as f32
        })
        .collect()
}

#[test]
#[ignore = "needs the sky model, ONNX Runtime and the private samples"]
fn sky_masks_against_lightroom() {
    let samples_dir = var("RAWMAKASE_SKY_SAMPLES");
    let out_dir = var("RAWMAKASE_SKY_OUT");
    let edge: u32 = std::env::var("RAWMAKASE_SKY_EDGE")
        .map(|v| v.parse().expect("RAWMAKASE_SKY_EDGE is a number"))
        .unwrap_or(1280);
    let samples: Vec<Sample> =
        serde_json::from_slice(&std::fs::read(samples_dir.join("samples.json")).unwrap()).unwrap();
    let cancel = Cancel::NEVER;
    let mut session = Sky::load_with(&var("RAWMAKASE_SKY_MODEL"), &LoadOptions::default()).unwrap();
    println!("edge {edge}");
    println!("photo         input ms  sky ms   band px     mae  missed_sky  false_sky");
    for sample in &samples {
        let raw = rawmakase::photo::open(&samples_dir.join(&sample.raw)).unwrap();
        let image = raw
            .develop(camera_data::Decode::full(Default::default()), cancel)
            .unwrap();
        let started = Instant::now();
        let input =
            rawmakase::develop::masks::selection_input(&image, &Recipe::default(), edge, cancel)
                .unwrap();
        let input_ms = started.elapsed().as_millis();
        let rgb = RgbImage {
            width: input.width as usize,
            height: input.height as usize,
            data: input.rgb8(),
        };
        let started = Instant::now();
        let ours = session.select(&rgb, cancel).unwrap();
        let sky_ms = started.elapsed().as_millis();
        let (w, h) = (ours.width, ours.height);

        let unmasked = image::open(samples_dir.join(&sample.unmasked))
            .unwrap()
            .to_rgb8();
        let masked = image::open(samples_dir.join(&sample.masked))
            .unwrap()
            .to_rgb8();
        assert_eq!(
            unmasked.dimensions(),
            masked.dimensions(),
            "{}: exports differ in size",
            sample.raw
        );
        let (ew, eh) = unmasked.dimensions();
        let aspect = |a: u32, b: u32| a as f64 / b as f64;
        assert!(
            (aspect(ew, eh) * h as f64 - w as f64).abs() <= 1.5,
            "{}: exports are {ew}x{eh}, the selection {w}x{h}: not the same frame",
            sample.raw
        );
        let lr = lightroom(&unmasked, &masked, sample.ev, w as u32, h as u32);

        let lr_or_ours: Vec<f32> = lr
            .iter()
            .zip(&ours.data)
            .map(|(l, o)| l.unwrap_or(f32::from(*o) / 255.))
            .collect();
        let mean = box_mean(&lr_or_ours, w, h, 10);
        let (mut band, mut error, mut missed, mut false_sky) = (0usize, 0f64, 0usize, 0usize);
        for i in 0..w * h {
            let Some(l) = lr[i] else { continue };
            if !(0.05..0.95).contains(&mean[i]) {
                continue;
            }
            let o = f32::from(ours.data[i]) / 255.;
            band += 1;
            error += (o - l).abs() as f64;
            missed += usize::from(l > 0.7 && o <= 0.5);
            false_sky += usize::from(l < 0.3 && o > 0.5);
        }
        let share = |n: usize| n as f64 / band.max(1) as f64;
        println!(
            "{:<12} {input_ms:>9} {sky_ms:>7} {band:>9} {:>7.3} {:>11.3} {:>10.3}",
            sample.raw,
            error / band.max(1) as f64,
            share(missed),
            share(false_sky)
        );

        let dir = out_dir.join(std::path::Path::new(&sample.raw).file_stem().unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        GrayImage::from_raw(w as u32, h as u32, ours.data.clone())
            .unwrap()
            .save(dir.join("ours.png"))
            .unwrap();
        let lr_png = GrayImage::from_fn(w as u32, h as u32, |x, y| {
            Luma([lr[y as usize * w + x as usize].map_or(128, |v| (v * 255. + 0.5) as u8)])
        });
        lr_png.save(dir.join("lightroom.png")).unwrap();
        let diff = Image::from_fn(w as u32, h as u32, |x, y| {
            let i = y as usize * w + x as usize;
            let o = f32::from(ours.data[i]) / 255.;
            match lr[i] {
                Some(l) if o - l > 0.3 => Rgb([220, 40, 40]),
                Some(l) if l - o > 0.3 => Rgb([40, 80, 220]),
                _ => {
                    let g = 60 + (o * 120.) as u8;
                    Rgb([g, g, g])
                }
            }
        });
        diff.save(dir.join("diff.png")).unwrap();
    }
}
