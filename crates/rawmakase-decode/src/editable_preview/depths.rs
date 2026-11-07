//! The measurement the Smaller choice was gated on (decision 3 of the editable
//! previews plan): version 1's curve at 12 and 10 bits against its 16, and the
//! Smaller preview itself, 12-bit codes dithered ([`Precision::Smaller`]).
//!
//! Plain 12- and 10-bit codes are curve 1's codes shifted down (code >> 4, >> 6)
//! and read back to the middle of their 16-bit bucket; the dithered 12-bit codes
//! are what [`build`] makes. All are coded losslessly by the same encoder in its
//! 16-bit mode (`zune-jpegxl` codes only 8 or 16 bits), so sizes are what that
//! path makes of the narrower codes; the dithered variant's is its real file.
//!
//! - `precision_and_headroom`: the relative step per code at levels from 1/1000 of
//!   white to 1000 times white, and what each level reads back as.
//! - `banding_on_a_smooth_gradient`: a synthetic, noise-free gradient that changes
//!   only along x, through [`build`], rendered with strong edits. Each column is
//!   averaged over every row, which keeps a contour step and averages dither
//!   away: the largest step between neighbouring columns against the lossless
//!   preview's, the noise left about that profile, and the largest step that noise
//!   alone makes in the profile (about 4 σ √2 / √rows). A step within that floor
//!   is noise, not a contour. Noise-free, so the worst case for contours.
//! - `depths`: for each RAW in `RAWMAKASE_EDITABLE_PREVIEW_SAMPLES`, the file size
//!   of each variant, build, encode and decode times, and CIEDE2000 between renders
//!   of each variant and of the lossless preview (per pixel and over 16 px
//!   blocks), for defaults and strong edits, and an HDR edit's relative luminance
//!   difference. A photo whose HDR render is refused is reported and left out of
//!   that line.
//!
//! Run with `cargo test -p rawmakase-decode --release --lib
//! editable_preview::depths -- --ignored --nocapture --test-threads=1`.
use super::*;
use crate::color_difference::{delta_e2000, lab};
use crate::develop;
use rawmakase_model::model::recipe::Recipe;
use std::time::Instant;

fn widen(c: u16, shift: u32) -> u16 {
    if shift == 0 {
        c
    } else {
        (c << shift) | (1 << (shift - 1))
    }
}

/// The preview's image with its codes quantized to `depth` bits.
fn at_depth(image: &CameraImage, codes: &[[u16; 3]], depth: u32) -> CameraImage {
    let shift = 16 - depth;
    let wide: Vec<[u16; 3]> = codes
        .par_iter()
        .map(|p| p.map(|c| widen(c >> shift, shift)))
        .collect();
    image_of(
        image.width,
        image.height,
        &wide,
        image.metadata.clone(),
        facts::Scale {
            factor: image.scale_factor,
            clipped: image.scale_clipped,
        },
    )
}

fn srgb_lab(p: [f32; 3]) -> [f64; 3] {
    lab(p.map(|v| f64::from(rawmakase_model::color::srgb_decode(v.clamp(0., 1.)))))
}

fn stats(mut d: Vec<f64>) -> [f64; 2] {
    d.sort_by(f64::total_cmp);
    [
        d.iter().sum::<f64>() / d.len() as f64,
        d[(d.len() - 1) * 99 / 100],
    ]
}

/// ΔE2000 per pixel and over 16 px blocks: mean and p99 of each.
fn de(a: &[[f32; 3]], b: &[[f32; 3]], w: u32, h: u32) -> [f64; 4] {
    let px = stats(
        a.par_iter()
            .zip(b)
            .map(|(a, b)| delta_e2000(srgb_lab(*a), srgb_lab(*b)))
            .collect(),
    );
    let block = |p: &[[f32; 3]]| -> Vec<[f32; 3]> {
        let (bw, bh) = (w / 16, h / 16);
        (0..bw * bh)
            .map(|i| {
                let (bx, by) = (i % bw, i / bw);
                let mut s = [0f32; 3];
                for y in 0..16 {
                    for x in 0..16 {
                        let q = p[((by * 16 + y) * w + bx * 16 + x) as usize];
                        (0..3).for_each(|c| s[c] += q[c] / 256.);
                    }
                }
                s
            })
            .collect()
    };
    let (ba, bb) = (block(a), block(b));
    let bl = stats(
        ba.iter()
            .zip(&bb)
            .map(|(a, b)| delta_e2000(srgb_lab(*a), srgb_lab(*b)))
            .collect(),
    );
    [px[0], px[1], bl[0], bl[1]]
}

type Edit = (&'static str, fn(&mut Recipe));

fn edits() -> Vec<Edit> {
    vec![
        ("default", |_| {}),
        ("+4 EV", |r| r.exposure += 4.),
        ("−3 EV, Shadows +100", |r| {
            r.exposure -= 3.;
            r.shadows = 1.;
        }),
        ("Contrast, Clarity, Texture +100", |r| {
            r.contrast = 1.;
            r.effects.clarity = 1.;
            r.effects.texture = 1.;
        }),
        ("steep point curve", |r| {
            r.curve.points = vec![[0., 0.], [0.35, 0.1], [0.55, 0.9], [1., 1.]];
        }),
        ("Saturation, Vibrance +100", |r| {
            r.saturation = 1.;
            r.vibrance = 1.;
        }),
    ]
}

/// HDR renders: relative difference of linear values, mean and p99.
fn hdr_difference(a: &CameraImage, b: &CameraImage, base: &Recipe) -> Result<[f64; 2]> {
    let mut r = base.clone();
    r.hdr.enabled = true;
    r.hdr.limit = 3.;
    r.exposure += 1.;
    let r = r.checked()?;
    let x = develop::render_hdr(a, &r, None, LONG_EDGE, Cancel::NEVER)?;
    let y = develop::render_hdr(b, &r, None, LONG_EDGE, Cancel::NEVER)?;
    let lum = |p: [f32; 3]| f64::from(0.3 * p[0] + 0.6 * p[1] + 0.1 * p[2]);
    Ok(stats(
        x.pixels
            .par_iter()
            .zip(&y.pixels)
            .map(|(a, b)| {
                let (a, b) = (lum(*a), lum(*b));
                (a - b).abs() / a.max(b).max(1e-3)
            })
            .collect(),
    ))
}

#[test]
#[ignore = "a measurement, not a check"]
fn precision_and_headroom() {
    let table = curve::table();
    println!("| Level | 16-bit step | 12-bit step | 10-bit step |\n| --- | --- | --- | --- |");
    for v in [1e-3f32, 0.18, 1., 8., 64., 1000.] {
        let c = curve::encode(v);
        let row: Vec<String> = [16u32, 12, 10]
            .iter()
            .map(|d| {
                let s = 16 - d;
                let lo = (c >> s) << s;
                let hi = (lo as u32 + (1 << s)).min(65535) as usize;
                let step = (table[hi] - table[lo as usize]) / v;
                let back = table[widen(c >> s, s) as usize];
                format!("{:.4}% (reads {:.5})", step * 100., back / v)
            })
            .collect();
        println!("| {v} × white | {} |", row.join(" | "));
    }
}

/// The variants compared: 16-bit (the lossless preview), 12-bit codes rounded,
/// 12-bit codes dithered (the Smaller choice, through [`build`] and [`write`]) and
/// 10-bit codes rounded.
const VARIANTS: [&str; 4] = ["16", "12", "12 dithered", "10"];

/// `image` (a lossless preview's) as each of [`VARIANTS`]; `smaller` is the Smaller
/// preview built from the same original.
fn variants(image: &CameraImage, smaller: &CameraImage) -> Vec<CameraImage> {
    let codes = encode_pixels(&image.pixels);
    vec![
        at_depth(image, &codes, 16),
        at_depth(image, &codes, 12),
        smaller.clone(),
        at_depth(image, &codes, 10),
    ]
}

/// The sky-like preview of a smooth, noise-free gradient that changes only along
/// x, built at `precision`.
fn gradient(precision: Precision) -> EditablePreview {
    let (w, h) = (6000u32, 600u32);
    let pixels: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let v = 0.03 + 0.25 * (i % w) as f32 / w as f32;
            [v * 0.8, v * 0.95, v * 1.3]
        })
        .collect();
    let metadata = Metadata {
        width: w,
        height: h,
        wb: [1., 1., 1.],
        cam_xyz: [[0.7, -0.2, -0.1], [-0.4, 1.2, 0.2], [-0.1, 0.2, 0.6]],
        ..Default::default()
    };
    let im = CameraImage {
        width: w,
        height: h,
        recovered: Default::default(),
        pixels,
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    };
    let id = crate::storage::Identity {
        size: 1,
        modified_ns: 1,
        prefix_hash: "0".into(),
    };
    build(&im, &id, None, LONG_EDGE, precision, Cancel::NEVER).unwrap()
}

/// The gradient through each variant, rendered with strong edits. The gradient
/// changes only along x, so each column is averaged over every row: that keeps a
/// contour step whole and averages the dither's noise away. Reported, on the
/// 16-bit output scale: the largest step between neighbouring columns of that
/// profile, as a multiple of the lossless preview's (contours); the noise left
/// about the profile (standard deviation, any channel); and the largest step that
/// noise alone makes in the profile. A step's excess over the lossless one within
/// that floor is noise, not a contour.
#[test]
#[ignore = "a measurement, not a check"]
fn banding_on_a_smooth_gradient() {
    let lossless = gradient(Precision::Lossless);
    let smaller = gradient(Precision::Smaller);
    let all = variants(lossless.image(), smaller.image());
    println!(
        "| Edit | Variant | Largest column step (16-bit levels) | Step / lossless | Noise σ (16-bit levels) | Noise floor of the step |\n| --- | --- | --- | --- | --- | --- |"
    );
    for (name, edit) in edits() {
        let mut r = Recipe::for_metadata(&lossless.image().metadata);
        edit(&mut r);
        let r = r.checked().unwrap();
        let mut base = 0.;
        for (label, im) in VARIANTS.iter().zip(&all) {
            let out = develop::render(im, &r, LONG_EDGE, Cancel::NEVER).unwrap();
            let (w, h) = (out.width as usize, out.height as usize);
            let profile: Vec<[f64; 3]> = (0..w)
                .map(|x| {
                    let mut sum = [0f64; 3];
                    for y in 0..h {
                        let p = out.pixels[y * w + x];
                        (0..3).for_each(|c| sum[c] += f64::from(p[c]) * 65535. / h as f64);
                    }
                    sum
                })
                .collect();
            let largest = profile
                .windows(2)
                .flat_map(|p| (0..3).map(move |c| (p[0][c] - p[1][c]).abs()))
                .fold(0., f64::max);
            let mut squares = 0.;
            for row in out.pixels.chunks_exact(w) {
                for (p, mean) in row.iter().zip(&profile) {
                    (0..3).for_each(|c| squares += (f64::from(p[c]) * 65535. - mean[c]).powi(2));
                }
            }
            let noise = (squares / (w * h * 3) as f64).sqrt();
            if *label == "16" {
                base = largest;
            }
            // The largest neighbour step the profile's own left-over noise makes:
            // about 4 standard deviations of a difference of two column means.
            let floor = 4. * noise * std::f64::consts::SQRT_2 / (h as f64).sqrt();
            println!(
                "| {name} | {label} | {largest:.1} | {:.2} | {noise:.1} | {floor:.1} |",
                largest / base.max(1e-9)
            );
        }
    }
}

#[test]
#[ignore = "needs RAWs in RAWMAKASE_EDITABLE_PREVIEW_SAMPLES"]
fn depths() {
    let Some(dir) = std::env::var_os("RAWMAKASE_EDITABLE_PREVIEW_SAMPLES") else {
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
    if paths.is_empty() {
        eprintln!("No RAWs in RAWMAKASE_EDITABLE_PREVIEW_SAMPLES");
        return;
    }
    let n = VARIANTS.len();
    let mut sizes: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut times: Vec<Vec<[f64; 2]>> = vec![Vec::new(); n];
    let mut builds: [Vec<f64>; 2] = Default::default();
    let mut diffs: Vec<Vec<[f64; 4]>> = vec![Vec::new(); edits().len() * (n - 1)];
    let mut hdr: Vec<Vec<[f64; 2]>> = vec![Vec::new(); n - 1];
    println!(
        "| File | Camera | {} |\n| --- | --- |{}",
        VARIANTS.map(|v| format!("{v}-bit MB")).join(" | "),
        " --- |".repeat(n)
    );
    for path in paths {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let raw = crate::photo::open(&path).unwrap();
        let full = crate::decode::FullSize::of_version(None, Default::default(), cache.clone());
        let original = full
            .decode(raw, crate::decode::DecodePolicy::Show, Cancel::NEVER)
            .unwrap();
        let identity = Identity::read(&path).unwrap();
        let exif = crate::photo::camera_exif(&path);
        let mut previews = Vec::new();
        for (k, precision) in [Precision::Lossless, Precision::Smaller]
            .into_iter()
            .enumerate()
        {
            let t = Instant::now();
            let p = build(
                &original,
                &identity,
                exif.clone(),
                LONG_EDGE,
                precision,
                Cancel::NEVER,
            )
            .unwrap();
            builds[k].push(t.elapsed().as_secs_f64());
            previews.push(p);
        }
        let file = out.path().join("p.rmep");
        let _ = std::fs::remove_file(&file);
        write(&file, &previews[0]).unwrap();
        let image = previews[0].image();
        let codes = encode_pixels(&image.pixels);
        // Everything in the file but the pixels: HEAD, FACT, EXIF and framing.
        let pixl16 = codec::encode(image.width, image.height, &codes)
            .unwrap()
            .len();
        let rest = std::fs::metadata(&file).unwrap().len() as f64 - pixl16 as f64;
        let mut row = format!(
            "| {name} | {} {} ",
            image.metadata.make, image.metadata.model
        );
        let all = variants(image, previews[1].image());
        for (i, (label, im)) in VARIANTS.iter().zip(&all).enumerate() {
            let shift = match *label {
                "16" => 0,
                "10" => 6,
                _ => 4,
            };
            let narrow: Vec<[u16; 3]> = encode_pixels(&im.pixels)
                .par_iter()
                .map(|p| p.map(|c| c >> shift))
                .collect();
            let t = Instant::now();
            let bytes = codec::encode(image.width, image.height, &narrow).unwrap();
            let enc = t.elapsed().as_secs_f64();
            let t = Instant::now();
            let back = codec::decode(&bytes, image.width, image.height).unwrap();
            let dec = t.elapsed().as_secs_f64();
            assert!(back == narrow, "lossless at {label} bits");
            let size = if *label == "12 dithered" {
                // The real file the Smaller choice writes.
                let file = out.path().join("smaller.rmep");
                let _ = std::fs::remove_file(&file);
                write(&file, &previews[1]).unwrap();
                let back = read(&file, Cancel::NEVER).unwrap();
                assert!(
                    back.image().pixels == im.pixels,
                    "the Smaller file reads back"
                );
                std::fs::metadata(&file).unwrap().len() as f64 / 1e6
            } else {
                (bytes.len() as f64 + rest) / 1e6
            };
            sizes[i].push(size);
            times[i].push([enc, dec]);
            row += &format!("| {size:.2} ");
        }
        println!("{row}|");
        let base = Recipe::for_metadata(&image.metadata);
        for (e, (_, edit)) in edits().iter().enumerate() {
            let mut r = base.clone();
            edit(&mut r);
            let r = r.checked().unwrap();
            let reference = develop::render(&all[0], &r, LONG_EDGE, Cancel::NEVER).unwrap();
            for (k, v) in all[1..].iter().enumerate() {
                let got = develop::render(v, &r, LONG_EDGE, Cancel::NEVER).unwrap();
                diffs[e * (n - 1) + k].push(de(
                    &reference.pixels,
                    &got.pixels,
                    reference.width,
                    reference.height,
                ));
            }
        }
        for (k, v) in all[1..].iter().enumerate() {
            match hdr_difference(&all[0], v, &base) {
                Ok(d) => hdr[k].push(d),
                Err(e) => eprintln!("{name}: no HDR comparison: {e:#}"),
            }
        }
    }
    let summary = |v: &[f64]| {
        let mut s = v.to_vec();
        s.sort_by(f64::total_cmp);
        format!(
            "mean {:.2}, median {:.2}, max {:.2}",
            s.iter().sum::<f64>() / s.len() as f64,
            s[s.len() / 2],
            s[s.len() - 1]
        )
    };
    println!();
    for (i, label) in VARIANTS.iter().enumerate() {
        let enc: Vec<f64> = times[i].iter().map(|t| t[0]).collect();
        let dec: Vec<f64> = times[i].iter().map(|t| t[1]).collect();
        println!(
            "{label}-bit: size {}; encode mean {:.3} s; decode mean {:.3} s",
            summary(&sizes[i]),
            enc.iter().sum::<f64>() / enc.len() as f64,
            dec.iter().sum::<f64>() / dec.len() as f64
        );
    }
    for (k, label) in ["Lossless", "Smaller"].iter().enumerate() {
        println!("build ({label}): {}", summary(&builds[k]));
    }
    println!(
        "\n| Edit | Variant | Pixel ΔE mean | Pixel p99 (mean / worst) | Block mean | Block p99 (mean / worst) |\n| --- | --- | --- | --- | --- | --- |"
    );
    for (e, (name, _)) in edits().iter().enumerate() {
        for (k, label) in VARIANTS[1..].iter().enumerate() {
            let d = &diffs[e * (n - 1) + k];
            let col = |j: usize| d.iter().map(|x| x[j]).collect::<Vec<_>>();
            let mean = |v: Vec<f64>| v.iter().sum::<f64>() / v.len() as f64;
            let worst = |v: Vec<f64>| v.into_iter().fold(0., f64::max);
            println!(
                "| {name} | {label} | {:.3} | {:.2} / {:.2} | {:.4} | {:.3} / {:.3} |",
                mean(col(0)),
                mean(col(1)),
                worst(col(1)),
                mean(col(2)),
                mean(col(3)),
                worst(col(3))
            );
        }
    }
    for (k, label) in VARIANTS[1..].iter().enumerate() {
        if hdr[k].is_empty() {
            continue;
        }
        let m = hdr[k].iter().map(|x| x[0]).sum::<f64>() / hdr[k].len() as f64;
        let p = hdr[k].iter().map(|x| x[1]).fold(0., f64::max);
        println!(
            "HDR edit (limit 3 stops, +1 EV), {label}-bit: relative luminance difference mean {:.5}, worst p99 {:.4}",
            m, p
        );
    }
}
