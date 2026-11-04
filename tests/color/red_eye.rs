//! Red eye correction on a synthetic face through a real camera profile: a red pupil
//! becomes a dark neutral, Darken orders its brightness, and the iris beyond the
//! correction's reach is untouched.
use crate::{chart_path, develop, embedded_profiles, measure};
use rawmakase::model::image_frame::ImageFrame;
use rawmakase::{
    camera_data::CameraImage,
    develop::{
        ViewMapping,
        red_eye::{Glow, find_pupil},
        render,
    },
    model::{recipe::Recipe, red_eye::RedEyeOp},
};
use rawmakase_cancel::Cancel;

const PUPIL: f32 = 14.;

/// The chart's camera and metadata with an eye on skin (camera values): a red pupil
/// inside a reddish-brown iris.
fn face() -> (CameraImage, [f32; 2]) {
    let mut im = develop(&chart_path("synthetic-d65"));
    let w = im.width as usize;
    let c = [im.width as f32 * 0.5, im.height as f32 * 0.5];
    for (i, p) in im.pixels.iter_mut().enumerate() {
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        let d = (x - c[0]).hypot(y - c[1]);
        *p = if d <= PUPIL {
            [0.5, 0.03, 0.03]
        } else if d <= 2.5 * PUPIL {
            [0.2, 0.07, 0.04]
        } else {
            [0.5, 0.33, 0.24]
        };
    }
    (im, c)
}
fn lab(p: [f32; 3]) -> [f64; 3] {
    measure::lab(p.map(|v| (v.clamp(0., 1.) * 65535.) as u16))
}
#[test]
fn red_pupil_turns_dark_and_neutral_and_the_iris_stays() {
    let (im, c) = face();
    let frame = ImageFrame::new(&im);
    let pupil = find_pupil(
        &im,
        frame.to_image(c[0], c[1]),
        3. * PUPIL / frame.long_edge(),
        Glow::Red,
    )
    .expect("a pupil");
    let recipe = Recipe {
        // Sharpening's halo around the darkened pupil is not the correction's own.
        sharpening: 0.,
        ..Recipe::with_profiles(&im.metadata, &embedded_profiles(&im))
    };
    let base = render(&im, &recipe.checked().unwrap(), 0, Cancel::NEVER).unwrap();
    let at = |r: &rawmakase::rendered::Rendered, d: f32, t: f32| {
        let [u, v] = ViewMapping::new(&im, &recipe)
            .to_view(frame.to_image(c[0] + d * t.cos(), c[1] + d * t.sin()));
        let (x, y) = (
            (u * r.width as f32) as usize,
            (v * r.height as f32) as usize,
        );
        lab(r.pixels[y * r.width as usize + x])
    };
    let corrected = |darken: f32| {
        let mut r = recipe.clone();
        r.red_eye = vec![RedEyeOp {
            kind: Default::default(),
            center: pupil.center,
            radius: pupil.radius,
            correlation: pupil.correlation,
            pupil_size: 0.5,
            darken,
        }]
        .into();
        render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap()
    };
    let half = corrected(0.5);
    let (before, after) = (at(&base, 0., 0.), at(&half, 0., 0.));
    assert!(measure::chroma(before) > 40., "red pupil: {before:?}");
    assert!(measure::chroma(after) < 6., "neutral pupil: {after:?}");
    assert!(
        after[0] < before[0],
        "darker pupil: {after:?} from {before:?}"
    );
    let lightness = |darken: f32| at(&corrected(darken), 0., 0.)[0];
    assert!(lightness(0.) > lightness(0.5) && lightness(0.5) > lightness(1.));
    // The iris from 1.6 pupil radii out renders exactly as before.
    for k in 0..12 {
        let t = k as f32 * 0.52;
        for d in [1.6 * PUPIL, 2.2 * PUPIL] {
            let e = measure::delta_e2000(at(&base, d, t), at(&half, d, t));
            assert!(e < 0.01, "iris at {d:.0} px moved by ΔE00 {e:.3}");
        }
    }
}
