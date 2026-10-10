use super::heal::{self, Placed};
use super::*;
use crate::camera_data::CameraImage;
use crate::model::red_eye::RedEyeOp;
use crate::model::retouch::{RetouchMode, RetouchShape};
use std::sync::atomic::AtomicBool;

fn image(width: u32, height: u32, f: impl Fn(f32, f32) -> [f32; 3]) -> CameraImage {
    CameraImage {
        width,
        height,
        pixels: (0..width * height)
            .map(|i| f((i % width) as f32, (i / width) as f32))
            .collect(),
        metadata: crate::camera_data::Metadata {
            width,
            height,
            wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        recovered: Default::default(),
    }
}
/// A spot at pixel (`x`, `y`) of a `width` × `height` image with a pixel offset.
fn spot(mode: RetouchMode, im: &CameraImage, c: [f32; 2], r: f32, off: [f32; 2]) -> RetouchOp {
    let (w, h) = (im.width as f32, im.height as f32);
    RetouchOp {
        mode,
        shape: RetouchShape::Spot {
            center: [(c[0] + 0.5) / w, (c[1] + 0.5) / h],
            radius: r / w.max(h),
        },
        feather: 0.3,
        opacity: 1.,
        offset: [off[0] / w, off[1] / h],
    }
}
fn max_error(a: &CameraImage, b: &CameraImage) -> f32 {
    a.pixels
        .iter()
        .flatten()
        .zip(b.pixels.iter().flatten())
        .map(|(x, y)| (x - y).abs())
        .fold(0., f32::max)
}

#[test]
fn heal_of_a_constant_field_is_exact() {
    // Destination 0.4, source area 0.1: healing restores 0.4 exactly.
    let im = image(
        120,
        80,
        |x, _| if x < 60. { [0.4, 0.3, 0.2] } else { [0.1; 3] },
    );
    let mut dusty = im.clone();
    for (i, p) in dusty.pixels.iter_mut().enumerate() {
        let (x, y) = ((i % 120) as f32, (i / 120) as f32);
        if (x - 25.).hypot(y - 40.) < 5. {
            *p = [0.05; 3];
        }
    }
    // Below Feather 25 the edge is hard but for the outer pixels: the dust is inside.
    let op = RetouchOp {
        feather: 0.2,
        ..spot(RetouchMode::Heal, &im, [25., 40.], 8., [55., 0.])
    };
    let healed = apply(&dusty, heals(&[op]));
    assert!(
        max_error(&healed, &im) < 1e-4,
        "{}",
        max_error(&healed, &im)
    );
}
#[test]
fn heal_reproduces_linear_gradients() {
    let gradient = |x: f32, y: f32| [0.1 + x * 0.004, 0.2 + y * 0.003, 0.3];
    let im = image(160, 120, gradient);
    let mut dusty = im.clone();
    for (i, p) in dusty.pixels.iter_mut().enumerate() {
        let (x, y) = ((i % 160) as f32, (i / 160) as f32);
        if (x - 80.).hypot(y - 60.) < 9. {
            *p = [0.9, 0.0, 0.5];
        }
    }
    // A copy from elsewhere on the gradient differs by a smooth amount that heal
    // removes. Linear values would make it exact; the log values heal uses stay within
    // 1% (0.01 EV), and keep textures right (see the next test).
    let op = RetouchOp {
        feather: 0.2,
        ..spot(RetouchMode::Heal, &im, [80., 60.], 14., [-40., 20.])
    };
    let healed = apply(&dusty, heals(&[op]));
    let error = im
        .pixels
        .iter()
        .flatten()
        .zip(healed.pixels.iter().flatten())
        .map(|(a, b)| (a - b).abs() / a)
        .fold(0., f32::max);
    assert!(error < 0.01, "{error}");
    // Clone copies the other part of the gradient unchanged.
    let op = spot(RetouchMode::Clone, &im, [80., 60.], 12., [-40., 0.]);
    let cloned = apply(&dusty, heals(&[op]));
    let i = 60 * 160 + 80;
    assert!((cloned.pixels[i][0] - gradient(40., 60.)[0]).abs() < 1e-4);
}
#[test]
fn cascaded_membrane_matches_a_converged_dense_solve() {
    let (w, h) = (180, 140);
    let values: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            (x * 0.05).sin() + (y * 0.07).cos() * 0.5 + x * 0.002
        })
        .collect();
    let inside: Vec<bool> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            ((x - 90.) / 70.).powi(2) + ((y - 70.) / 55.).powi(2) < 1.
        })
        .collect();
    let fast = heal::membrane(&values, &inside, w, h);
    // Dense reference: plain Gauss-Seidel until converged.
    let mut dense = values;
    for (v, i) in dense.iter_mut().zip(&inside) {
        if *i {
            *v = 0.;
        }
    }
    for _ in 0..30000 {
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                if inside[y * w + x] {
                    dense[y * w + x] = (dense[y * w + x - 1]
                        + dense[y * w + x + 1]
                        + dense[(y - 1) * w + x]
                        + dense[(y + 1) * w + x])
                        * 0.25;
                }
            }
        }
    }
    let error = fast
        .iter()
        .zip(&dense)
        .map(|(a, b)| (a - b).abs())
        .fold(0., f32::max);
    assert!(error < 2e-3, "max error {error}");
}
/// Texture under an illumination gradient, healed from a darker area: log values keep
/// the texture's contrast relative to its surroundings, linear values do not. This is
/// why Heal works on log values. Measured: RMS residual log 0.014, ln(1 + x) 0.028,
/// linear 0.035.
#[test]
fn log_heal_beats_linear_on_lit_texture() {
    let truth = |x: f32, y: f32| {
        let light = 0.05 + x / 200. * 0.8;
        let texture = 1. + 0.25 * (x * 0.9).sin() * (y * 0.7).cos();
        [
            light * texture,
            light * texture * 0.8,
            light * texture * 0.6,
        ]
    };
    let (w, h) = (200usize, 100usize);
    let im = image(w as u32, h as u32, truth);
    let (cx, cy, r) = (150usize, 50usize, 10usize);
    let grid = [cx - r - 2, cy - r - 2, cx + r + 3, cy + r + 3].map(|v| v as i32);
    let (gw, gh) = ((grid[2] - grid[0]) as usize, (grid[3] - grid[1]) as usize);
    let alpha = heal::coverage(
        &[[cx as f32, cy as f32]],
        r as f32,
        0.,
        FeatherProfile::Smoothstep,
        grid,
    );
    let pixel = |x: i32, y: i32| im.pixels[y as usize * w + x as usize];
    let dest: Vec<[f32; 3]> = (0..gw * gh)
        .map(|i| pixel(grid[0] + (i % gw) as i32, grid[1] + (i / gw) as i32))
        .collect();
    // Source 7 texture periods to the left, where the light is much dimmer.
    let shift = -(2. * std::f32::consts::PI / 0.9 * 12.).round() as i32;
    let source: Vec<[f32; 3]> = (0..gw * gh)
        .map(|i| pixel(grid[0] + (i % gw) as i32 + shift, grid[1] + (i / gw) as i32))
        .collect();
    let residual = |domain: heal::Domain| {
        let out = heal::heal(&dest, &source, &alpha, gw, gh, domain);
        let (mut sum, mut n) = (0., 0.);
        for i in 0..gw * gh {
            if alpha[i] > 0. {
                for c in 0..3 {
                    sum += (out[i][c] - dest[i][c]).powi(2);
                    n += 1.;
                }
            }
        }
        (sum / n as f32).sqrt()
    };
    let (log, linear, log1p) = (
        residual(heal::Domain::Log),
        residual(heal::Domain::Linear),
        residual(heal::Domain::Log1p),
    );
    assert!(
        log < linear * 0.5 && log < log1p * 0.6,
        "log {log} linear {linear} log1p {log1p}"
    );
}
#[test]
fn incremental_tiles_match_a_full_rebuild() {
    let im = image(700, 520, |x, y| {
        let v = 0.2 + 0.1 * (x * 0.03).sin() * (y * 0.02).cos() + x * 0.0003;
        [v, v * 0.9, v * 0.7]
    });
    let base = std::sync::Arc::new(im.clone());
    let mut ops = vec![
        spot(RetouchMode::Heal, &im, [100., 100.], 20., [60., 10.]),
        spot(RetouchMode::Clone, &im, [300., 260.], 30., [-80., 40.]),
        // Reads the first spot's result: its source overlaps that spot.
        spot(RetouchMode::Heal, &im, [180., 110.], 15., [-70., -5.]),
        spot(RetouchMode::Heal, &im, [600., 450.], 25., [-60., -60.]),
    ];
    let cancel = AtomicBool::new(false);
    let mut cache = RetouchCache::default();
    let check = |cache: &mut RetouchCache, ops: &[RetouchOp]| {
        let incremental = cache.get(&base, heals(ops), &cancel).unwrap();
        let full = apply(&base, heals(ops));
        assert_eq!(incremental.pixels, full.pixels);
    };
    check(&mut cache, &ops);
    ops[0].offset[0] += 20. / 700.;
    check(&mut cache, &ops);
    ops.remove(1);
    check(&mut cache, &ops);
    ops[2].translate([0.01, 0.02]);
    check(&mut cache, &ops);
    let before = cache.get(&base, heals(&ops), &cancel).unwrap();
    ops.push(spot(RetouchMode::Clone, &im, [650., 60.], 10., [-40., 0.]));
    let after = cache.get(&base, heals(&ops), &cancel).unwrap();
    // Only tiles near the new spot changed.
    let rects = cache.changed_from(&before).unwrap();
    assert!(rects.iter().all(|r| r[0] >= 512 && r[3] <= 256));
    for (i, (a, b)) in before.pixels.iter().zip(&after.pixels).enumerate() {
        if a != b {
            let (x, y) = ((i % 700) as i32, (i / 700) as i32);
            assert!(
                rects
                    .iter()
                    .any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3])
            );
        }
    }
    check(&mut cache, &ops);
    check(&mut cache, &[]);

    // Red eye corrections apply first; a heal copying from one sees the corrected eye.
    let eye = |x: f32, y: f32, r: f32| RedEyeOp {
        kind: Default::default(),
        center: frame_of(&im).to_image(x, y),
        radius: [r / 700.; 2],
        correlation: 0.,
        pupil_size: 0.5,
        darken: 0.5,
    };
    let mut eyes = vec![eye(160., 100., 12.), eye(400., 400., 20.)];
    let mut heal_ops = vec![spot(RetouchMode::Heal, &im, [100., 100.], 20., [60., 0.])];
    let both = |cache: &mut RetouchCache, eyes: &[RedEyeOp], ops: &[RetouchOp]| {
        let ops = Retouching {
            red_eye: eyes,
            retouch: ops,
        };
        let incremental = cache.get(&base, ops, &cancel).unwrap();
        assert_eq!(incremental.pixels, apply(&base, ops).pixels);
    };
    both(&mut cache, &eyes, &heal_ops);
    eyes[0].darken = 1.;
    both(&mut cache, &eyes, &heal_ops);
    eyes[1].translate([0.05, 0.]);
    both(&mut cache, &eyes, &heal_ops);
    eyes.remove(0);
    heal_ops[0].offset[1] += 0.01;
    both(&mut cache, &eyes, &heal_ops);
}
fn heals(ops: &[RetouchOp]) -> Retouching<'_> {
    Retouching {
        red_eye: &[],
        retouch: ops,
    }
}
fn frame_of(im: &CameraImage) -> crate::model::image_frame::ImageFrame {
    crate::model::image_frame::ImageFrame::new(im)
}
#[test]
fn automatic_source_avoids_texture_edges_and_asks_again_for_the_next() {
    // A smooth gradient with a bright stripe to the right; dust at the centre.
    let im = image(400, 300, |x, y| {
        let v = 0.2 + x * 0.0004 + y * 0.0002;
        if (250. ..270.).contains(&x) {
            [0.9; 3]
        } else {
            [v, v, v]
        }
    });
    let mut dusty = im.clone();
    for (i, p) in dusty.pixels.iter_mut().enumerate() {
        let (x, y) = ((i % 400) as f32, (i / 400) as f32);
        if (x - 200.).hypot(y - 150.) < 6. {
            *p = [0.02; 3];
        }
    }
    let mut op = spot(RetouchMode::Heal, &im, [200., 150.], 10., [0., 0.]);
    let offset = find_source(&dusty, &op, &[], &[]).unwrap();
    op.offset = offset;
    // The source is outside the spot and not on the stripe.
    let src = [(0.5 + offset[0]) * 400., (0.5 + offset[1]) * 300.];
    assert!((src[0] - 200.).hypot(src[1] - 150.) >= 14.);
    assert!(!(235. ..285.).contains(&src[0]), "{src:?}");
    let healed = apply(&dusty, heals(&[op.clone()]));
    assert!(
        max_error(&healed, &im) < 0.01,
        "{}",
        max_error(&healed, &im)
    );
    let next = find_source(&dusty, &op, &[], &[offset]).unwrap();
    let d = ((next[0] - offset[0]) * 400.).hypot((next[1] - offset[1]) * 300.);
    assert!(d >= 10., "{d}");
}
#[test]
fn placed_brush_covers_its_path() {
    let im = image(200, 100, |_, _| [0.3; 3]);
    let frame = crate::model::image_frame::ImageFrame::new(&im);
    let op = RetouchOp {
        mode: RetouchMode::Clone,
        shape: RetouchShape::Brush {
            points: vec![[0.2, 0.5], [0.4, 0.5]].into(),
            radius: 0.02,
        },
        feather: 0.,
        opacity: 1.,
        offset: [0., 0.2],
    };
    let p = Placed::new(&op, &frame, FeatherProfile::Smoothstep);
    assert_eq!(p.radius, 4.);
    let rect = [30, 40, 90, 60];
    let a = heal::coverage(&p.points, p.radius, 0., FeatherProfile::Smoothstep, rect);
    let at = |x: i32, y: i32| a[((y - rect[1]) * 60 + x - rect[0]) as usize];
    assert_eq!(at(60, 49), 1.);
    assert_eq!(at(60, 55), 0.);
    assert_eq!(p.offset, [0., 20.]);
}

/// Camera Raw 18.7's soft edge on Clone spots: the source's weight in the linear blend
/// reaches one half at 0.95 of the radius at Feather 25, 0.89 at 50, 0.79 at 75 and
/// 0.65 at 100 (where the rendered output, after the tone curve, is half way at 0.96,
/// 0.91, 0.83 and 0.71). The original smoothstep crossed much further in.
#[test]
fn measured_feather_crosses_half_where_camera_raw_does() {
    let half = |profile: FeatherProfile, feather: f32| {
        let radius = 200.;
        let row = heal::coverage(&[[0., 0.]], radius, feather, profile, [0, 0, 220, 1]);
        row.iter().position(|a| *a < 0.5).unwrap() as f32 / radius
    };
    for (feather, camera_raw) in [(0.25, 0.952), (0.5, 0.89), (0.75, 0.788), (1., 0.648)] {
        let ours = half(FeatherProfile::Measured, feather);
        assert!(
            (ours - camera_raw).abs() < 0.015,
            "{feather}: {ours} against {camera_raw}"
        );
    }
    assert!(half(FeatherProfile::Smoothstep, 0.5) < 0.78);
    // A hard edge is the same either way.
    assert_eq!(
        half(FeatherProfile::Measured, 0.),
        half(FeatherProfile::Smoothstep, 0.)
    );
}
