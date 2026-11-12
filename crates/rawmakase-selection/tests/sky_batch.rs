//! Local comparison tool for Select Sky, ignored by default. Runs the sky model on every
//! binary `.ppm` in a folder and writes each mask as `<stem>.pgm`, printing the time per photo.
//!
//! ```text
//! RAWMAKASE_SKY_INPUT=<folder of .ppm> \
//! RAWMAKASE_SKY_MODEL=<the sky model's folder> \
//! RAWMAKASE_SKY_OUT=<output folder> \
//! cargo test -p rawmakase-inference --test sky_batch -- --ignored --nocapture
//! ```

use rawmakase_cancel::Cancel;
use rawmakase_inference::LoadOptions;
use rawmakase_selection::{RgbImage, Sky};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn var(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set")))
}

fn read_ppm(bytes: &[u8]) -> RgbImage {
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..i]).to_string());
    }
    let (width, height): (usize, usize) = (fields[1].parse().unwrap(), fields[2].parse().unwrap());
    let data = bytes[i + 1..i + 1 + width * height * 3].to_vec();
    RgbImage {
        width,
        height,
        data,
    }
}

#[test]
#[ignore = "needs a sky model and a folder of photos (RAWMAKASE_SKY_* variables)"]
fn sky_batch() {
    let input = var("RAWMAKASE_SKY_INPUT");
    let out = var("RAWMAKASE_SKY_OUT");
    std::fs::create_dir_all(&out).unwrap();
    let cancel = AtomicBool::new(false);
    let mut session = Sky::load_with(&var("RAWMAKASE_SKY_MODEL"), &LoadOptions::default()).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&input)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ppm"))
        .collect();
    files.sort();
    for path in files {
        let rgb = read_ppm(&std::fs::read(&path).unwrap());
        let started = std::time::Instant::now();
        let cov = session.select(&rgb, Cancel::new(&cancel)).unwrap();
        println!("{} {} ms", path.display(), started.elapsed().as_millis());
        let mut pgm = format!("P5\n{} {}\n255\n", cov.width, cov.height).into_bytes();
        pgm.extend(&cov.data);
        std::fs::write(
            out.join(format!(
                "{}.pgm",
                path.file_stem().unwrap().to_string_lossy()
            )),
            pgm,
        )
        .unwrap();
    }
}
