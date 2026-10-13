use super::*;
use crate::develop::effects::GrainField;
use crate::model::operators::GrainModel;

fn field(size: f32, roughness: f32, edge: f32) -> FilmGrainField {
    FilmGrainField::new(
        &FilmGrain {
            amount: 1.,
            size,
            roughness,
            ..FilmGrain::default()
        },
        edge,
    )
}
fn mean_and_deviation(values: &[f32]) -> (f32, f32) {
    let n = values.len() as f32;
    let mean = values.iter().sum::<f32>() / n;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n;
    (mean, variance.sqrt())
}
/// Positions far enough apart to sample the grain almost independently.
fn spread(count: usize) -> impl Iterator<Item = (f32, f32)> {
    (0..count).map(|i| ((i % 400) as f32 * 31.3, (i / 400) as f32 * 29.7))
}

/// The texture is scaled to mean 0 and deviation 1 at every Size, Roughness and photo
/// size, so Amount means the same everywhere.
#[test]
fn noise_has_zero_mean_and_unit_deviation() {
    for edge in [1500., 6000.] {
        for size in [0., 0.25, 1.] {
            for roughness in [0., 0.5, 1.] {
                let f = field(size, roughness, edge);
                let values: Vec<f32> = spread(40_000).map(|(x, y)| f.noise(x, y)).collect();
                let (mean, deviation) = mean_and_deviation(&values);
                assert!(
                    mean.abs() < 0.03 && (deviation - 1.).abs() < 0.1,
                    "edge {edge} size {size} roughness {roughness}: {mean} {deviation}"
                );
            }
        }
    }
}

/// The grain follows the tone as a negative's small grains do: nothing at black and
/// white, as strong as `tone_strength` says between, and about mean-preserving.
#[test]
fn grain_follows_the_tone_and_fades_into_black_and_white() {
    let f = field(0.25, 0.5, 6000.);
    for (x, y) in spread(500) {
        assert_eq!(f.delta(x, y, 1., 0.), 0.);
        assert_eq!(f.delta(x, y, 1., 1.), 0.);
    }
    for l in [0.05, 0.2, 0.4, 0.6, 0.8, 0.95] {
        let values: Vec<f32> = spread(40_000).map(|(x, y)| f.delta(x, y, 1., l)).collect();
        let (mean, deviation) = mean_and_deviation(&values);
        let expected = STRENGTH * tone_strength(l);
        assert!(mean.abs() < 0.003, "l {l}: mean {mean}");
        assert!(
            (deviation / expected - 1.).abs() < 0.1,
            "l {l}: deviation {deviation} against {expected}"
        );
    }
    let peak = (1..100)
        .map(|i| i as f32 / 100.)
        .max_by(|a, b| tone_strength(*a).total_cmp(&tone_strength(*b)))
        .unwrap();
    assert!((peak - 0.39).abs() < 0.02, "{peak}");
}

/// No isolated grains anywhere in the tone range: the grain's values stay about as
/// spread as a normal distribution's (low excess kurtosis) in deep shadows and bright
/// highlights, where sparse opaque grains made bright or dark specks.
#[test]
fn grain_has_no_isolated_specks_in_shadows_or_highlights() {
    for size in [0., 0.25, 1.] {
        let f = field(size, 0.5, 6000.);
        for l in [0.03, 0.5, 0.9] {
            let values: Vec<f32> = spread(40_000).map(|(x, y)| f.delta(x, y, 1., l)).collect();
            let (mean, deviation) = mean_and_deviation(&values);
            let kurtosis = values
                .iter()
                .map(|v| ((v - mean) / deviation).powi(4))
                .sum::<f32>()
                / values.len() as f32
                - 3.;
            assert!(
                kurtosis.abs() < 1.,
                "size {size} l {l}: excess kurtosis {kurtosis}"
            );
        }
    }
}

#[test]
fn film_grain_is_strongest_in_the_midtones() {
    let f = field(0.25, 0.5, 6000.);
    let deviation = |l: f32| {
        let values: Vec<f32> = spread(20_000).map(|(x, y)| f.delta(x, y, 1., l)).collect();
        mean_and_deviation(&values).1
    };
    let mid = deviation(0.5);
    assert!(mid > 0.2 * STRENGTH, "{mid}");
    assert!(deviation(0.06) < mid && deviation(0.97) < mid * 0.5);
}

/// The variance of `value` at each of 4×4 phases within cells of `cell` pixels:
/// sampled at the same phase in many cells.
fn variance_by_phase(cell: f32, value: impl Fn(f32, f32) -> f32) -> Vec<f32> {
    let mut out = Vec::new();
    for by in 0..4 {
        for bx in 0..4 {
            let values: Vec<f32> = (0..4096)
                .map(|i| {
                    let x = ((i % 64) as f32 + (bx as f32 + 0.5) / 4.) * cell;
                    let y = ((i / 64) as f32 + (by as f32 + 0.5) / 4.) * cell;
                    value(x, y)
                })
                .collect();
            out.push(mean_and_deviation(&values).1.powi(2));
        }
    }
    out
}
fn spread_of(values: &[f32]) -> f32 {
    let max = values.iter().cloned().fold(f32::MIN, f32::max);
    let min = values.iter().cloned().fold(f32::MAX, f32::min);
    max / min
}

/// Grain on a lattice is strongest at its knots; Film Grain has no such structure,
/// which the same measure shows for a value-noise grain.
#[test]
fn film_grain_has_no_lattice() {
    let f = field(0.25, 0.5, 6000.);
    let film = spread_of(&variance_by_phase(f.cell, |x, y| f.delta(x, y, 1., 0.5)));
    assert!(film < 1.2, "film grain: {film}");
    let lattice = GrainField {
        model: GrainModel::Measured,
        amount: 1.,
        cell: 8.,
        coarse: 1.,
        fine: 0.,
        seed: 42,
    };
    let value_noise = spread_of(&variance_by_phase(lattice.cell, |x, y| {
        lattice.noise(x, y, 1., 0.62)
    }));
    assert!(value_noise > 1.5, "value noise: {value_noise}");
}

/// Each cell holds a fixed number of candidates; staggering their grids keeps that from
/// showing as a pattern of the cell's period in the mean. Sampled at 4×4 phases in the
/// same cells, each cell's own mean taken out. With 262 144 cells the noise in the
/// range of the 16 phase means is about 0.003, so a pattern of 0.008 or more fails.
#[test]
fn film_grain_mean_does_not_follow_the_cells() {
    use rayon::prelude::*;
    let mut worst: f32 = 0.;
    for size in [0., 0.25, 1.] {
        let f = field(size, 0.5, 6000.);
        for l in [0.2, 0.5] {
            let cells = 262_144;
            let residuals: Vec<[f32; 16]> = (0..cells)
                .into_par_iter()
                .map(|c| {
                    let (cx, cy) = ((c % 512) as f32, (c / 512) as f32);
                    let values: [f32; 16] = std::array::from_fn(|b| {
                        let x = (cx + ((b % 4) as f32 + 0.5) / 4.) * f.cell;
                        let y = (cy + ((b / 4) as f32 + 0.5) / 4.) * f.cell;
                        f.delta(x, y, 1., l)
                    });
                    let mean = values.iter().sum::<f32>() / 16.;
                    values.map(|v| v - mean)
                })
                .collect();
            let at: Vec<(f32, f32)> = (0..16)
                .map(|b| {
                    let r: Vec<f32> = residuals.iter().map(|v| v[b]).collect();
                    let (mean, deviation) = mean_and_deviation(&r);
                    (mean, deviation / (r.len() as f32).sqrt())
                })
                .collect();
            let error = at.iter().map(|m| m.1).fold(0., f32::max);
            let means: Vec<f32> = at.iter().map(|m| m.0).collect();
            let range = means.iter().cloned().fold(f32::MIN, f32::max)
                - means.iter().cloned().fold(f32::MAX, f32::min);
            worst = worst.max(range);
            // The test must resolve the limit: noise alone is well below it.
            assert!(
                5. * error < 0.004,
                "size {size} l {l}: standard error {error}"
            );
            assert!(range < 0.006, "size {size} l {l}: {means:?} ± {error}");
        }
    }
    eprintln!("largest phase range: {worst}");
}

/// Grain is the same share of the frame at any photo size: larger in pixels on a
/// larger photo, and larger with Size. Measured by how far the grain stays correlated:
/// the sum of its autocorrelation over lags of 1 to 4 pixels.
#[test]
fn film_grain_grows_with_size_and_the_photo() {
    let extent = |f: &FilmGrainField| {
        let at = |x: f32, y: f32| f.delta(x, y, 1., 0.5);
        let base: Vec<f32> = spread(20_000).map(|(x, y)| at(x, y)).collect();
        let (a, sa) = mean_and_deviation(&base);
        (1..=4)
            .map(|lag| {
                let shifted: Vec<f32> =
                    spread(20_000).map(|(x, y)| at(x + lag as f32, y)).collect();
                let (b, sb) = mean_and_deviation(&shifted);
                base.iter()
                    .zip(&shifted)
                    .map(|(x, y)| (x - a) * (y - b))
                    .sum::<f32>()
                    / base.len() as f32
                    / (sa * sb)
            })
            .sum::<f32>()
    };
    let small = extent(&field(0., 0.5, 6000.));
    let large = extent(&field(1., 0.5, 6000.));
    eprintln!("correlation extent: Size 0 {small}, Size 1 {large}");
    assert!(large > small * 1.5 + 0.2, "{small} {large}");
    let photo_small = extent(&field(0.5, 0.5, 1500.));
    let photo_large = extent(&field(0.5, 0.5, 6000.));
    eprintln!("correlation extent: 1500 px {photo_small}, 6000 px {photo_large}");
    assert!(
        photo_large > photo_small * 1.5 + 0.1,
        "{photo_small} {photo_large}"
    );
}

/// A preview at half size averages exactly the pixels the full render has there;
/// smaller ones approximate the averaged grain's strength.
#[test]
fn previews_average_the_full_resolution_grain() {
    let l = 0.4;
    let f = field(0.25, 0.5, 6000.);
    for n in [2u32] {
        let scale = 1. / n as f32;
        for origin in [0i32, 7, -3] {
            for py in 0..12 {
                for px in 0..12 {
                    let (px, py) = (px + origin, py + origin);
                    let x = (px as f32 + 0.5) / scale - 0.5;
                    let y = (py as f32 + 0.5) / scale - 0.5;
                    let preview = f.delta(x, y, scale, l);
                    let mut full = 0.;
                    for j in 0..n as i32 {
                        for i in 0..n as i32 {
                            let fx = (px * n as i32 + i) as f32;
                            let fy = (py * n as i32 + j) as f32;
                            full += f.delta(fx, fy, 1., l);
                        }
                    }
                    full /= (n * n) as f32;
                    assert!(
                        (preview - full).abs() < 1e-4,
                        "{n} {px} {py}: {preview} {full}"
                    );
                }
            }
        }
    }
    for (size, scale) in [
        (0., 1. / 4.),
        (0.25, 1. / 4.),
        (1., 1. / 4.),
        (0., 1. / 8.),
        (0.25, 1. / 8.),
        (1., 1. / 8.),
        (0.25, 1. / 16.),
    ] {
        let f = field(size, 0.5, 6000.);
        let k = (1. / scale) as usize;
        let (mut preview, mut full) = (Vec::new(), Vec::new());
        for i in 0..3000 {
            let (px, py) = ((i % 60) as f32 * 3., (i / 60) as f32 * 3.);
            preview.push(f.delta((px + 0.5) / scale - 0.5, (py + 0.5) / scale - 0.5, scale, l));
            let mut sum = 0.;
            for j in 0..k {
                for m in 0..k {
                    sum += f.delta(px * k as f32 + m as f32, py * k as f32 + j as f32, 1., l);
                }
            }
            full.push(sum / (k * k) as f32);
        }
        let ratio = mean_and_deviation(&preview).1 / mean_and_deviation(&full).1;
        eprintln!("size {size} scale {scale}: preview/full deviation {ratio}");
        assert!(
            (ratio - 1.).abs() < 0.35,
            "size {size} scale {scale}: {ratio}"
        );
    }
}

fn layers(color: f32) -> FilmGrainLayers {
    FilmGrainLayers::new(
        &FilmGrain {
            amount: 1.,
            color,
            ..FilmGrain::default()
        },
        6000.,
    )
}
fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let ((ma, sa), (mb, sb)) = (mean_and_deviation(a), mean_and_deviation(b));
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - ma) * (y - mb))
        .sum::<f32>()
        / a.len() as f32
        / (sa * sb)
}

/// How far a grain stays correlated: its autocorrelation summed over lags of 1 to 6.
fn extent(value: impl Fn(f32, f32) -> f32) -> f32 {
    let base: Vec<f32> = spread(20_000).map(|(x, y)| value(x, y)).collect();
    (1..=6)
        .map(|lag| {
            let shifted: Vec<f32> = spread(20_000)
                .map(|(x, y)| value(x + lag as f32, y))
                .collect();
            correlation(&base, &shifted)
        })
        .sum()
}

/// At Color 0 every channel takes the mono grain over the luminance, clumped, exactly
/// as before Color existed.
#[test]
fn color_zero_is_the_mono_grain_on_every_channel() {
    let film = layers(0.);
    let mono = FilmGrainField::new(
        &FilmGrain {
            amount: 1.,
            ..FilmGrain::default()
        },
        6000.,
    );
    assert_eq!(film.mono, mono);
    for (x, y) in spread(2000) {
        let rgb = [0.6, 0.4, 0.2];
        let l = crate::color::luminance(rgb);
        assert_eq!(
            film.delta(x, y, 1., rgb),
            [mono.delta(x, y, 1., l) * film.clumps(x, y); 3]
        );
    }
}

/// Color changes the grain's colour, not its brightness: the luminance of what it adds
/// is the mono grain's at any Color, and its colour grows with Color, about
/// mean-preserving in each channel.
#[test]
fn color_changes_the_grains_colour_not_its_brightness() {
    let rgb = [0.55, 0.45, 0.35];
    let l = crate::color::luminance(rgb);
    let mono = layers(0.).mono;
    let chroma = |color: f32| {
        let film = layers(color);
        let (mut spread_rb, mut channels) = (Vec::new(), [Vec::new(), Vec::new(), Vec::new()]);
        for (x, y) in spread(20_000) {
            let d = film.delta(x, y, 1., rgb);
            let brightness = crate::color::luminance(d);
            let expected = mono.delta(x, y, 1., l) * film.clumps(x, y);
            assert!(
                (brightness - expected).abs() < 1e-5,
                "Color {color}: {brightness} {expected}"
            );
            spread_rb.push(d[0] - d[2]);
            for c in 0..3 {
                channels[c].push(d[c] - expected);
            }
        }
        for (c, channel) in channels.iter().enumerate() {
            let mean = mean_and_deviation(channel).0;
            assert!(mean.abs() < 0.01, "Color {color}, channel {c}: mean {mean}");
        }
        mean_and_deviation(&spread_rb).1
    };
    let (half, full) = (chroma(0.5), chroma(1.));
    eprintln!("red minus blue deviation: Color 0.5 {half}, Color 1 {full}");
    assert!(full > 0.05 * STRENGTH, "{full}");
    assert!((half / full - 0.5).abs() < 0.02, "{half} {full}");
}

/// At Color 1 the grain's colour is as in scans of colour negatives (Kodak Ektar 100
/// and Ultramax 400 at 4000 dpi): as fine as its brightness, the channels' grain
/// correlated by 0.31–0.51 (red–green), 0.35–0.44 (green–blue) and 0.14–0.20
/// (red–blue), its colour varying 1.2–1.35 times as much as its brightness.
#[test]
fn colour_grain_is_as_in_scans_of_colour_negatives() {
    let film = layers(1.);
    let rgb = [0.5; 3];
    let deltas: Vec<[f32; 3]> = spread(40_000)
        .map(|(x, y)| film.delta(x, y, 1., rgb))
        .collect();
    let channel = |c: usize| deltas.iter().map(|d| d[c]).collect::<Vec<f32>>();
    let [r, g, b] = [channel(0), channel(1), channel(2)];
    let (rg, gb, rb) = (
        correlation(&r, &g),
        correlation(&g, &b),
        correlation(&r, &b),
    );
    eprintln!("correlation red–green {rg}, green–blue {gb}, red–blue {rb}");
    assert!((0.25..0.55).contains(&rg) && (0.3..0.55).contains(&gb) && (0.1..0.25).contains(&rb));
    let luma: Vec<f32> = deltas.iter().map(|d| crate::color::luminance(*d)).collect();
    let chroma: Vec<f32> = deltas
        .iter()
        .map(|d| d[0] - crate::color::luminance(*d))
        .collect();
    let ratio = mean_and_deviation(&chroma).1 / mean_and_deviation(&luma).1;
    assert!(
        (0.9..1.6).contains(&ratio),
        "red's colour over brightness: {ratio}"
    );
    let brightness = extent(|x, y| film.mono.delta(x, y, 1., 0.5));
    let colour = extent(|x, y| {
        let d = film.delta(x, y, 1., rgb);
        d[0] - crate::color::luminance(d)
    });
    eprintln!("correlation extent: brightness {brightness}, colour {colour}");
    assert!(
        (colour / brightness - 1.).abs() < 0.15,
        "{brightness} {colour}"
    );
}

/// The grain is a little stronger in some patches than others, as in film: its strength
/// varies by a few percent over about 0.2–0.5 mm, more with Roughness, slowly compared
/// with the grain, and without changing its overall strength or adding specks.
#[test]
fn grain_clumps_naturally() {
    let mut last = 0.;
    for roughness in [0., 0.5, 1.] {
        let film = FilmGrainLayers::new(
            &FilmGrain {
                amount: 1.,
                roughness,
                ..FilmGrain::default()
            },
            6000.,
        );
        let clumps: Vec<f32> = spread(40_000).map(|(x, y)| film.clumps(x, y)).collect();
        let power = clumps.iter().map(|c| c * c).sum::<f32>() / clumps.len() as f32;
        assert!((power - 1.).abs() < 0.01, "Roughness {roughness}: {power}");
        let (mean, deviation) = mean_and_deviation(&clumps);
        let variation = deviation / mean;
        assert!(
            (variation / film.clumping - 1.).abs() < 0.15,
            "Roughness {roughness}: {variation} against {}",
            film.clumping
        );
        assert!(variation > last);
        last = variation;
        let near = correlation(
            &clumps,
            &spread(40_000)
                .map(|(x, y)| film.clumps(x + 8., y))
                .collect::<Vec<_>>(),
        );
        let far = correlation(
            &clumps,
            &spread(40_000)
                .map(|(x, y)| film.clumps(x + 300., y))
                .collect::<Vec<_>>(),
        );
        assert!(
            near > 0.8 && far.abs() < 0.1,
            "Roughness {roughness}: {near} {far}"
        );
        let deltas: Vec<f32> = spread(40_000)
            .map(|(x, y)| film.delta(x, y, 1., [0.5; 3])[0])
            .collect();
        let (m, d) = mean_and_deviation(&deltas);
        let kurtosis =
            deltas.iter().map(|v| ((v - m) / d).powi(4)).sum::<f32>() / deltas.len() as f32 - 3.;
        assert!(
            kurtosis < 1.,
            "Roughness {roughness}: excess kurtosis {kurtosis}"
        );
    }
}

/// Off, Film Grain leaves the finished photo exactly as it was; on, it changes it.
#[test]
fn spatial_finish_adds_film_grain_only_when_on() {
    use crate::model::recipe::Recipe;
    use crate::rendered::Rendered;
    let image = || Rendered {
        width: 32,
        height: 32,
        pixels: (0..1024).map(|i| [i as f32 / 1024.; 3]).collect(),
    };
    let mut r = Recipe::default();
    r.effects.vignette = -0.3;
    let mut off = image();
    crate::develop::effects::spatial_finish(&mut off, &r, [0, 0], [32, 32]);
    r.film_grain.size = 0.9;
    r.film_grain.seed = 3;
    let mut still_off = image();
    crate::develop::effects::spatial_finish(&mut still_off, &r, [0, 0], [32, 32]);
    assert_eq!(off.pixels, still_off.pixels);
    r.film_grain.amount = 0.3;
    let mut on = image();
    crate::develop::effects::spatial_finish(&mut on, &r, [0, 0], [32, 32]);
    assert_ne!(off.pixels, on.pixels);
}

/// Export-sized Film Grain on the CPU, for the benchmark gate:
/// `cargo test --release -p rawmakase-engine film_grain_export_time -- --ignored --nocapture`.
#[test]
#[ignore = "timing"]
fn film_grain_export_time() {
    use crate::model::recipe::Recipe;
    use crate::rendered::Rendered;
    let (w, h) = (6000u32, 4000u32);
    let image = Rendered {
        width: w,
        height: h,
        pixels: vec![[0.46; 3]; (w * h) as usize],
    };
    for color in [0., 0.5, 1.] {
        let mut image = image.clone();
        let mut r = Recipe::default();
        r.film_grain.amount = 0.3;
        r.film_grain.color = color;
        let start = std::time::Instant::now();
        crate::develop::effects::spatial_finish(&mut image, &r, [0, 0], [w, h]);
        eprintln!("24 MP Film Grain, Color {color}: {:?}", start.elapsed());
    }
}
