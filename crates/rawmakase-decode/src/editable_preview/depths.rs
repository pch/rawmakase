//! The measurement behind a smaller editable preview: version 1's curve with its
//! codes quantized to 12 and 10 bits, against its 16. Codes are shifted down
//! (code >> 4, >> 6), coded losslessly by the same encoder in its 16-bit mode
//! (`zune-jpegxl` codes only 8 or 16 bits, so sizes are what its 16-bit path makes
//! of the narrower codes; an encoder with a true 12-bit mode may code them otherwise), and read back to the middle of their 16-bit bucket. Nothing here
//! changes the format: a smaller choice would be a new codec or curve code.
//!
//! - `precision_and_headroom`: the relative step per code at levels from 1/1000 of
//!   white to 1000 times white, and what each level reads back as.
//! - `banding_on_a_smooth_gradient`: a synthetic, noise-free gradient through
//!   [`build`], rendered with strong edits; the largest step between neighbouring
//!   output pixels (any channel) and the green levels a row reaches. Noise-free, so the worst case.
//! - `depths`: for each RAW in `RAWMAKASE_EDITABLE_PREVIEW_SAMPLES`, the file size
//!   at each depth, encode and decode times, and CIEDE2000 between renders of the
//!   12- and 10-bit previews and the 16-bit one (per pixel and over 16 px blocks),
//!   for defaults and strong edits, and an HDR edit's relative luminance difference.
//!   A photo whose HDR render is refused is reported and left out of that line.
//!
//! Run with `cargo test -p rawmakase-decode --release --lib
//! editable_preview::depths -- --ignored --nocapture --test-threads=1`.
use super::*;
use crate::color_difference::{delta_e2000, lab};
use crate::develop;
use rawmakase_model::model::recipe::Recipe;
use std::time::Instant;

const DEPTHS: [u32; 3] = [16, 12, 10];

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
        let row: Vec<String> = DEPTHS
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

/// A smooth, sky-like gradient through the build path, then strong edits: the
/// largest step between neighbouring output pixels, on the 16-bit output scale,
/// and how many output levels are reached along a row.
#[test]
#[ignore = "a measurement, not a check"]
fn banding_on_a_smooth_gradient() {
    let (w, h) = (6000u32, 600u32);
    let pixels: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
            let v = 0.03 + 0.25 * x + 0.01 * y;
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
    let preview = build(
        &im,
        &crate::storage::Identity {
            size: 1,
            modified_ns: 1,
            prefix_hash: "0".into(),
        },
        None,
        LONG_EDGE,
        Cancel::NEVER,
    )
    .unwrap();
    let image = preview.image();
    let codes = encode_pixels(&image.pixels);
    println!(
        "| Edit | Depth | Largest neighbour step (16-bit output levels) | Green levels on a row | Max step / 16-bit's |\n| --- | --- | --- | --- | --- |"
    );
    for (name, edit) in edits() {
        let mut r = Recipe::for_metadata(&image.metadata);
        edit(&mut r);
        let r = r.checked().unwrap();
        let mut base = 0.;
        for depth in DEPTHS {
            let im = at_depth(image, &codes, depth);
            let out = develop::render(&im, &r, LONG_EDGE, Cancel::NEVER).unwrap();
            let row = out.height as usize / 2;
            let (mut largest, mut levels) = (0f32, std::collections::BTreeSet::new());
            for x in 1..out.width as usize {
                let a = out.pixels[row * out.width as usize + x - 1];
                let b = out.pixels[row * out.width as usize + x];
                for c in 0..3 {
                    largest = largest.max((a[c] - b[c]).abs() * 65535.);
                }
                levels.insert((b[1] * 65535.).round() as u32);
            }
            if depth == 16 {
                base = largest;
            }
            println!(
                "| {name} | {depth} | {largest:.1} | {} | {:.2} |",
                levels.len(),
                largest / base.max(1e-6)
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
    let mut sizes: [Vec<f64>; 3] = Default::default();
    let mut times: [Vec<[f64; 2]>; 3] = Default::default();
    let mut diffs: Vec<Vec<[f64; 4]>> = vec![Vec::new(); edits().len() * 2];
    let mut hdr: [Vec<[f64; 2]>; 2] = Default::default();
    println!(
        "| File | Camera | 16-bit MB | 12-bit MB | 10-bit MB |\n| --- | --- | --- | --- | --- |"
    );
    for path in paths {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let raw = crate::photo::open(&path).unwrap();
        let full = crate::decode::FullSize::of_version(None, Default::default(), cache.clone());
        let original = full
            .decode(raw, crate::decode::DecodePolicy::Show, Cancel::NEVER)
            .unwrap();
        let preview = build(
            &original,
            &Identity::read(&path).unwrap(),
            crate::photo::camera_exif(&path),
            LONG_EDGE,
            Cancel::NEVER,
        )
        .unwrap();
        let file = out.path().join("p.rmep");
        let _ = std::fs::remove_file(&file);
        write(&file, &preview).unwrap();
        let image = preview.image();
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
        let mut variants = Vec::new();
        for (i, depth) in DEPTHS.iter().enumerate() {
            let shift = 16 - depth;
            let narrow: Vec<[u16; 3]> = codes.par_iter().map(|p| p.map(|c| c >> shift)).collect();
            let t = Instant::now();
            let bytes = codec::encode(image.width, image.height, &narrow).unwrap();
            let enc = t.elapsed().as_secs_f64();
            let t = Instant::now();
            let back = codec::decode(&bytes, image.width, image.height).unwrap();
            let dec = t.elapsed().as_secs_f64();
            assert!(back == narrow, "lossless at {depth} bits");
            let size = (bytes.len() as f64 + rest) / 1e6;
            sizes[i].push(size);
            times[i].push([enc, dec]);
            row += &format!("| {size:.2} ");
            variants.push(at_depth(image, &codes, *depth));
        }
        println!("{row}|");
        let base = Recipe::for_metadata(&image.metadata);
        for (e, (_, edit)) in edits().iter().enumerate() {
            let mut r = base.clone();
            edit(&mut r);
            let r = r.checked().unwrap();
            let reference = develop::render(&variants[0], &r, LONG_EDGE, Cancel::NEVER).unwrap();
            for (k, v) in variants[1..].iter().enumerate() {
                let got = develop::render(v, &r, LONG_EDGE, Cancel::NEVER).unwrap();
                diffs[e * 2 + k].push(de(
                    &reference.pixels,
                    &got.pixels,
                    reference.width,
                    reference.height,
                ));
            }
        }
        for (k, v) in variants[1..].iter().enumerate() {
            match hdr_difference(&variants[0], v, &base) {
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
    for (i, depth) in DEPTHS.iter().enumerate() {
        let enc: Vec<f64> = times[i].iter().map(|t| t[0]).collect();
        let dec: Vec<f64> = times[i].iter().map(|t| t[1]).collect();
        println!(
            "{depth}-bit: size {}; encode mean {:.3} s; decode mean {:.3} s",
            summary(&sizes[i]),
            enc.iter().sum::<f64>() / enc.len() as f64,
            dec.iter().sum::<f64>() / dec.len() as f64
        );
    }
    println!(
        "\n| Edit | Depth | Pixel ΔE mean | Pixel p99 (mean / worst) | Block mean | Block p99 (mean / worst) |\n| --- | --- | --- | --- | --- | --- |"
    );
    for (e, (name, _)) in edits().iter().enumerate() {
        for (k, depth) in [12, 10].iter().enumerate() {
            let d = &diffs[e * 2 + k];
            let col = |j: usize| d.iter().map(|x| x[j]).collect::<Vec<_>>();
            let mean = |v: Vec<f64>| v.iter().sum::<f64>() / v.len() as f64;
            let worst = |v: Vec<f64>| v.into_iter().fold(0., f64::max);
            println!(
                "| {name} | {depth} | {:.3} | {:.2} / {:.2} | {:.4} | {:.3} / {:.3} |",
                mean(col(0)),
                mean(col(1)),
                worst(col(1)),
                mean(col(2)),
                mean(col(3)),
                worst(col(3))
            );
        }
    }
    for (k, depth) in [12, 10].iter().enumerate() {
        if hdr[k].is_empty() {
            continue;
        }
        let m = hdr[k].iter().map(|x| x[0]).sum::<f64>() / hdr[k].len() as f64;
        let p = hdr[k].iter().map(|x| x[1]).fold(0., f64::max);
        println!(
            "HDR edit (limit 3 stops, +1 EV), {depth}-bit: relative luminance difference mean {:.5}, worst p99 {:.4}",
            m, p
        );
    }
}
