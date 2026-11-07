//! The measurement the editable preview format was chosen on (decision 3 of the
//! editable previews plan): for each RAW in the folder `RAWMAKASE_EDITABLE_PREVIEW_SAMPLES`,
//! the file's size at [`LONG_EDGE`], the time to build, write and read it, and how
//! its renders differ from the original's at the same size. Run with
//! `cargo test -p rawmakase-decode --release editable_preview::measure -- --ignored --nocapture`.
//!
//! Renders are compared in CIEDE2000 over every pixel. "format" compares the
//! reduced image before and after the file, so it is the curve's loss alone; the
//! recipes compare renders of the original and of the editable preview, per pixel and
//! averaged over 16-pixel blocks, both with the camera's default crop taken off
//! ([`uncropped`]); an aspect crop the camera recorded reaches both as the recipe's
//! crop. They also differ by what Stage 1 of the plan fixes: detail radii scaled to
//! the image in memory.
use super::*;
use crate::color_difference::{delta_e2000, lab};
use crate::develop;
use rawmakase_model::model::recipe::Recipe;
use std::time::Instant;

/// `im` averaged over blocks of 16 × 16 pixels: what is left of a difference once
/// detail (sharpening radii, resampling) is averaged out.
fn blocks(im: &rawmakase_model::rendered::Rendered) -> (Vec<[f32; 3]>, u32, u32) {
    let (w, h) = (im.width / 16, im.height / 16);
    let pixels = (0..w * h)
        .map(|i| {
            let (bx, by) = (i % w, i / w);
            let mut sum = [0f32; 3];
            for y in 0..16 {
                for x in 0..16 {
                    let p = im.pixels[((by * 16 + y) * im.width + bx * 16 + x) as usize];
                    for c in 0..3 {
                        sum[c] += p[c] / 256.;
                    }
                }
            }
            sum
        })
        .collect();
    (pixels, w, h)
}

/// `im` with the camera's default and aspect crops taken off, its recovered image
/// too. Comparisons render both images so, at the same size: an editable preview is
/// 2560 pixels before the crop, so its cropped frame is a few pixels smaller, and a
/// render never enlarges.
fn uncropped(im: &CameraImage) -> CameraImage {
    let strip = |im: &CameraImage| CameraImage {
        recovered: Default::default(),
        metadata: Metadata {
            crop_left: 0,
            crop_top: 0,
            crop_width: im.metadata.width,
            crop_height: im.metadata.height,
            camera_crop: None,
            ..im.metadata.clone()
        },
        ..im.clone()
    };
    let out = strip(im);
    if let Some(recovered) = im.recovered.get() {
        let _ = out.recovered.set(Arc::new(strip(recovered)));
    }
    out
}

fn srgb_lab(p: [f32; 3]) -> [f64; 3] {
    lab(p.map(|v| f64::from(rawmakase_model::color::srgb_decode(v.clamp(0., 1.)))))
}

/// Mean, 99th percentile and largest difference between `a` and `b`, compared where
/// they overlap (a render can round its size differently by a pixel).
fn difference(a: (&[[f32; 3]], u32, u32), b: (&[[f32; 3]], u32, u32)) -> [f64; 3] {
    let (w, h) = (a.1.min(b.1) as usize, a.2.min(b.2) as usize);
    let mut d: Vec<f64> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            delta_e2000(
                srgb_lab(a.0[y * a.1 as usize + x]),
                srgb_lab(b.0[y * b.1 as usize + x]),
            )
        })
        .collect();
    d.sort_by(f64::total_cmp);
    let mean = d.iter().sum::<f64>() / d.len() as f64;
    [mean, d[(d.len() - 1) * 99 / 100], d[d.len() - 1]]
}

/// The same difference for linear camera values, each shown as sRGB would show
/// them at `exposure` stops: the curve's loss, from shadows to highlights.
fn camera_difference(a: &CameraImage, b: &CameraImage, exposure: f32) -> [f64; 3] {
    let shown = |im: &CameraImage| -> Vec<[f32; 3]> {
        im.pixels
            .par_iter()
            .map(|p| {
                p.map(|v| rawmakase_model::color::srgb_encode((v * exposure.exp2()).clamp(0., 1.)))
            })
            .collect()
    };
    difference(
        (&shown(a), a.width, a.height),
        (&shown(b), b.width, b.height),
    )
}

#[test]
#[ignore = "needs RAWs in RAWMAKASE_EDITABLE_PREVIEW_SAMPLES"]
fn measure() {
    let Some(dir) = std::env::var_os("RAWMAKASE_EDITABLE_PREVIEW_SAMPLES") else {
        eprintln!("RAWMAKASE_EDITABLE_PREVIEW_SAMPLES is not set");
        return;
    };
    let out = tempfile::tempdir().unwrap();
    let cache = crate::decode_cache::DecodeCache::new(out.path().join("cache"), 0);
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| rawmakase_model::storage::is_raw(p))
        .collect();
    paths.sort();
    println!(
        "| File | Camera | Original | Preview | Size (MB) | Build (s) | Write (s) | Read (s) | Format ΔE mean/p99/max (+0, +4 EV) | Default ΔE mean/p99/max; 16 px blocks | Detail off | Detail off +4 EV | Detail off −3 EV |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut sizes = Vec::new();
    for path in paths {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let result = (|| -> Result<String> {
            let identity = Identity::read(&path)?;
            let raw = crate::photo::open(&path)?;
            let full = crate::decode::FullSize::of_version(None, Default::default(), cache.clone());
            let original = full.decode(raw, crate::decode::DecodePolicy::Show, Cancel::NEVER)?;
            let t = Instant::now();
            let preview = build(
                &original,
                &identity,
                crate::photo::camera_exif(&path),
                LONG_EDGE,
                Precision::Lossless,
                Cancel::NEVER,
            )?;
            let build_s = t.elapsed().as_secs_f64();
            let file = out.path().join(format!("{name}.rmep"));
            let t = Instant::now();
            write(&file, &preview)?;
            let write_s = t.elapsed().as_secs_f64();
            let t = Instant::now();
            let back = read(&file, Cancel::NEVER)?;
            let read_s = t.elapsed().as_secs_f64();
            let size = std::fs::metadata(&file)?.len() as f64 / 1e6;
            sizes.push(size);
            let image = back.image();
            let reduced = develop::preview(
                &*develop::quality::recovered(&original, Cancel::NEVER)?,
                LONG_EDGE,
            );
            let f = |d: [f64; 3]| format!("{:.3}/{:.2}/{:.1}", d[0], d[1], d[2]);
            let format0 = camera_difference(&reduced, image, 0.);
            let format4 = camera_difference(&reduced, image, 4.);
            let compare = |edit: &dyn Fn(&mut Recipe)| -> Result<String> {
                let mut r = Recipe::for_metadata(&original.metadata);
                edit(&mut r);
                let r = r.checked()?;
                let a = develop::render(&uncropped(&original), &r, LONG_EDGE, Cancel::NEVER)?;
                let b = develop::render(&uncropped(image), &r, LONG_EDGE, Cancel::NEVER)?;
                if (a.width, a.height) != (b.width, b.height) {
                    return Ok(format!(
                        "frame differs: {}×{} vs {}×{}",
                        a.width, a.height, b.width, b.height
                    ));
                }
                let (pa, w, h) = blocks(&a);
                let (pb, ..) = blocks(&b);
                Ok(format!(
                    "{}; {}",
                    f(difference(
                        (&a.pixels, a.width, a.height),
                        (&b.pixels, b.width, b.height),
                    )),
                    f(difference((&pa, w, h), (&pb, w, h)))
                ))
            };
            let off = |r: &mut Recipe| {
                r.sharpening = 0.;
                r.noise_luma = 0.;
                r.noise_chroma = 0.;
            };
            let default = compare(&|_| {})?;
            let detail_off = compare(&off)?;
            let up = compare(&|r| {
                off(r);
                r.exposure += 4.;
            })?;
            let down = compare(&|r| {
                off(r);
                r.exposure -= 3.;
            })?;
            Ok(format!(
                "| {name} | {} {} | {}×{} | {}×{} | {size:.2} | {build_s:.2} | {write_s:.2} | {read_s:.2} | {}, {} | {} | {} | {} | {} |",
                original.metadata.make,
                original.metadata.model,
                original.width,
                original.height,
                image.width,
                image.height,
                f(format0),
                f(format4),
                default,
                detail_off,
                up,
                down,
            ))
        })();
        match result {
            Ok(row) => println!("{row}"),
            Err(e) => println!("| {name} | failed: {e:#} |"),
        }
    }
    if !sizes.is_empty() {
        sizes.sort_by(f64::total_cmp);
        println!(
            "\n{} files: mean {:.2} MB, median {:.2} MB, largest {:.2} MB",
            sizes.len(),
            sizes.iter().sum::<f64>() / sizes.len() as f64,
            sizes[sizes.len() / 2],
            sizes[sizes.len() - 1]
        );
    }
}
