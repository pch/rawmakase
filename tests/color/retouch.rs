//! Spot removal on a synthetic chart: dust on a smooth gradient and on a lit texture,
//! healed with automatically chosen sources, scored against the clean render.
use crate::{chart_path, develop, embedded_profiles, measure};
use rawmakase::model::image_frame::ImageFrame;
use rawmakase::{
    camera_data::CameraImage,
    develop::{ViewMapping, render, retouch::find_source},
    model::{
        recipe::Recipe,
        retouch::{RetouchMode, RetouchOp, RetouchShape},
    },
};
use rawmakase_cancel::Cancel;

/// The chart's camera and metadata with a gradient above a lit texture, so results
/// render through a real camera profile.
fn scene() -> CameraImage {
    let mut im = develop(&chart_path("synthetic-d65"));
    let (w, h) = (im.width as usize, im.height as usize);
    for (i, p) in im.pixels.iter_mut().enumerate() {
        let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
        let light = 0.08 + 0.3 * x;
        let v = if y < 0.5 {
            light + 0.1 * y
        } else {
            let (px, py) = ((i % w) as f32, (i / w) as f32);
            let texture = (px * 0.7).sin() * (py * 0.5).cos() + 0.5 * (px * 0.23 + py * 0.41).sin();
            light * (1. + 0.15 * texture)
        };
        *p = [v * 0.9, v, v * 0.8];
    }
    im
}
fn dust(im: &mut CameraImage, spots: &[([f32; 2], f32)]) {
    let w = im.width as usize;
    for (i, p) in im.pixels.iter_mut().enumerate() {
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        for (c, r) in spots {
            let d = (x - c[0]).hypot(y - c[1]);
            if d < *r {
                *p = p.map(|v| v * (0.25 + 0.5 * d / r));
            }
        }
    }
}
#[test]
fn dust_on_gradient_and_texture_heals_cleanly() {
    let clean = scene();
    let mut dusty = clean.clone();
    let (w, h) = (clean.width as f32, clean.height as f32);
    let spots = [
        ([w * 0.3, h * 0.25], 6.),
        ([w * 0.7, h * 0.2], 9.),
        ([w * 0.4, h * 0.75], 6.),
        ([w * 0.65, h * 0.8], 8.),
    ];
    dust(&mut dusty, &spots);
    let profiles = embedded_profiles(&clean);
    let mut recipe = Recipe::with_profiles(&clean.metadata, &profiles);
    let frame = ImageFrame::new(&clean);
    let ops: Vec<RetouchOp> = spots
        .iter()
        .map(|(c, r)| {
            let mut op = RetouchOp {
                mode: RetouchMode::Heal,
                shape: RetouchShape::Spot {
                    center: frame.to_image(c[0], c[1]),
                    radius: (r * 1.6) / frame.long_edge(),
                },
                feather: 0.3,
                opacity: 1.,
                offset: [0.; 2],
            };
            op.offset = find_source(&dusty, &op, &[], &[]).expect("a source");
            op
        })
        .collect();
    let reference = render(&clean, &recipe.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    let score = |recipe: &Recipe| {
        let expected = &reference;
        let actual = render(&dusty, &recipe.checked().unwrap(), 0, Cancel::NEVER).unwrap();
        let rw = expected.width as usize;
        let map = ViewMapping::new(&clean, recipe);
        let mut worst: f64 = 0.;
        for (c, r) in &spots {
            let [u, v] = map.to_view(frame.to_image(c[0], c[1]));
            let (cx, cy) = (u * expected.width as f32, v * expected.height as f32);
            let (mut sum, mut n) = (0., 0.);
            for y in (cy - r) as usize..=(cy + r) as usize {
                for x in (cx - r) as usize..=(cx + r) as usize {
                    let to16 = |p: [f32; 3]| p.map(|v| (v.clamp(0., 1.) * 65535.) as u16);
                    let a = measure::lab(to16(expected.pixels[y * rw + x]));
                    let b = measure::lab(to16(actual.pixels[y * rw + x]));
                    sum += measure::delta_e2000(a, b);
                    n += 1.;
                }
            }
            worst = worst.max(sum / n);
        }
        worst
    };
    let before = score(&recipe);
    recipe.retouch = ops;
    let after = score(&recipe);
    assert!(before > 10., "dust should be visible: ΔE00 {before:.2}");
    assert!(
        after < 1.,
        "healed spots differ by mean ΔE00 {after:.2} (dust {before:.2})"
    );
}
