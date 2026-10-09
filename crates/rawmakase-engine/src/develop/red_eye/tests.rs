use super::detect::DetectError;
use super::*;
use crate::model::red_eye::{
    DEFAULT_CATCHLIGHT, DEFAULT_DARKEN, DEFAULT_PUPIL_SIZE, RedEyeList, RedEyeOp,
};
use crate::{camera_data::CameraImage, model::image_frame::ImageFrame};

const SKIN: [f32; 3] = [0.55, 0.35, 0.25];
const RED_PUPIL: [f32; 3] = [0.6, 0.03, 0.03];
const BROWN_IRIS: [f32; 3] = [0.15, 0.07, 0.03];
const REDDISH_IRIS: [f32; 3] = [0.3, 0.08, 0.05];

struct Eye {
    center: [f32; 2],
    pupil: f32,
    iris: f32,
    iris_color: [f32; 3],
    pupil_color: [f32; 3],
}
fn eye(center: [f32; 2], pupil: f32, iris_color: [f32; 3]) -> Eye {
    Eye {
        center,
        pupil,
        iris: pupil * 2.2,
        iris_color,
        pupil_color: RED_PUPIL,
    }
}
/// Decoded pixels of eyes on skin, with anti-aliased edges; `flip` is the camera's
/// orientation (LibRaw's code).
fn image(width: u32, height: u32, flip: i32, eyes: &[Eye]) -> CameraImage {
    let cover = |d: f32, r: f32| (r + 0.5 - d).clamp(0., 1.);
    let pixels = (0..width * height)
        .map(|i| {
            let (x, y) = ((i % width) as f32, (i / width) as f32);
            let mut p = SKIN;
            for e in eyes {
                let d = (x - e.center[0]).hypot(y - e.center[1]);
                for (color, r) in [(e.iris_color, e.iris), (e.pupil_color, e.pupil)] {
                    let a = cover(d, r);
                    p = std::array::from_fn(|c| p[c] + (color[c] - p[c]) * a);
                }
            }
            p
        })
        .collect();
    CameraImage {
        width,
        height,
        pixels,
        metadata: crate::camera_data::Metadata {
            width,
            height,
            flip,
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
/// The correction for the pupil found within `search` decoded pixels of decoded
/// position `at`.
fn correct(im: &CameraImage, at: [f32; 2], search: f32) -> RedEyeOp {
    let frame = ImageFrame::new(im);
    let pupil = find_pupil(
        im,
        frame.to_image(at[0], at[1]),
        search / frame.long_edge(),
        Glow::Red,
    )
    .expect("a pupil");
    RedEyeOp {
        kind: EyeKind::Red,
        center: pupil.center,
        radius: pupil.radius,
        correlation: pupil.correlation,
        pupil_size: DEFAULT_PUPIL_SIZE,
        darken: DEFAULT_DARKEN,
    }
}
fn rendered(im: &CameraImage, ops: &[RedEyeOp]) -> CameraImage {
    crate::develop::retouch::apply(
        im,
        crate::develop::retouch::Retouching {
            red_eye: ops,
            retouch: &[],
        },
    )
}
fn pixel(im: &CameraImage, x: f32, y: f32) -> [f32; 3] {
    im.pixels[y as usize * im.width as usize + x as usize]
}
fn chroma(p: [f32; 3]) -> f32 {
    let max = p.iter().copied().fold(0., f32::max);
    let min = p.iter().copied().fold(f32::INFINITY, f32::min);
    max / min.max(1e-6)
}

#[test]
fn finds_small_and_large_pupils() {
    for r in [3., 8., 20., 45.] {
        let c = [200.3, 150.6];
        let im = image(400, 300, 0, &[eye(c, r, BROWN_IRIS)]);
        let frame = ImageFrame::new(&im);
        let op = correct(&im, [c[0] + r * 0.3, c[1] - r * 0.2], r * 2.8);
        let [x, y] = frame.to_source(op.center);
        let long = frame.long_edge();
        assert!(
            (x - c[0]).abs() < 0.5 && (y - c[1]).abs() < 0.5,
            "pupil {r}: centre {x},{y}"
        );
        for axis in op.radius {
            let found = axis * long;
            assert!(
                (found - r).abs() < (0.12 * r).max(0.6),
                "pupil {r}: radius {found}"
            );
        }
        assert!(op.correlation.abs() < 0.05, "{}", op.correlation);
    }
}
#[test]
fn large_searches_are_subsampled_and_still_accurate() {
    // A 4000-pixel photo searched with a 900-pixel circle: a grid of every 6th pixel.
    let c = [2000.4, 1300.7];
    let im = image(4000, 2600, 0, &[eye(c, 260., BROWN_IRIS)]);
    let frame = ImageFrame::new(&im);
    let op = correct(&im, c, 900.);
    let [x, y] = frame.to_source(op.center);
    assert!((x - c[0]).abs() < 3. && (y - c[1]).abs() < 3., "{x},{y}");
    for axis in op.radius {
        assert!((axis * 4000. - 260.).abs() < 6., "{}", axis * 4000.);
    }
}
#[test]
fn corrects_the_pupil_and_leaves_a_reddish_iris() {
    let c = [150., 120.];
    let r = 15.;
    let mut e = eye(c, r, REDDISH_IRIS);
    e.iris = 40.;
    let im = image(300, 240, 0, &[e]);
    let op = correct(&im, c, 45.);
    let long = ImageFrame::new(&im).long_edge();
    assert!(
        op.radius.iter().all(|a| (a * long - r).abs() < 1.5),
        "the pupil, not the iris: {:?}",
        op.radius.map(|a| a * long)
    );
    let out = rendered(&im, &[op]);
    let before = pixel(&im, c[0], c[1]);
    let after = pixel(&out, c[0], c[1]);
    assert!(chroma(before) > 10.);
    assert!(chroma(after) < 2., "neutral pupil: {after:?}");
    assert!(after[0] < 0.25 * before[0], "dark pupil: {after:?}");
    // Beyond the falloff the iris is untouched.
    for i in 0..360 {
        let t = (i as f32).to_radians();
        for d in [1.65 * r, 2. * r, 2.5 * r] {
            let (x, y) = (c[0] + d * t.cos(), c[1] + d * t.sin());
            assert_eq!(pixel(&im, x, y), pixel(&out, x, y), "at {x},{y}");
        }
    }
}
#[test]
fn catchlight_inside_the_pupil_is_part_of_it() {
    let c = [100., 100.];
    let mut im = image(200, 200, 0, &[eye(c, 16., BROWN_IRIS)]);
    for (i, p) in im.pixels.iter_mut().enumerate() {
        let (x, y) = ((i % 200) as f32, (i / 200) as f32);
        if (x - 95.).hypot(y - 95.) < 4. {
            *p = [0.9; 3];
        }
    }
    let op = correct(&im, c, 40.);
    let frame = ImageFrame::new(&im);
    let [x, y] = frame.to_source(op.center);
    assert!((x - c[0]).abs() < 1. && (y - c[1]).abs() < 1., "{x},{y}");
    assert!(op.radius.iter().all(|a| (a * 200. - 16.).abs() < 1.5));
}
#[test]
fn no_red_eye_is_reported() {
    let mut e = eye([100., 100.], 12., BROWN_IRIS);
    e.pupil_color = [0.02, 0.02, 0.02];
    let im = image(200, 200, 0, &[e]);
    let frame = ImageFrame::new(&im);
    let found = find_pupil(&im, frame.to_image(100., 100.), 35. / 200., Glow::Red);
    assert_eq!(found, Err(DetectError::NotRed));
    // A circle inside a red area finds no pupil edge.
    let im = image(200, 200, 0, &[eye([100., 100.], 80., BROWN_IRIS)]);
    let found = find_pupil(&im, frame.to_image(100., 100.), 30. / 200., Glow::Red);
    assert_eq!(found, Err(DetectError::NoEdge));
}
#[test]
fn tilted_pupils_and_camera_orientation() {
    // A tilted elliptical pupil in the decoded image.
    let (w, h) = (320u32, 200u32);
    let c = [170., 90.];
    let (a, b, angle) = (22f32, 10f32, 0.6f32);
    let make = |flip: i32| {
        let mut im = image(w, h, flip, &[]);
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32 - c[0], (i as u32 / w) as f32 - c[1]);
            let (u, v) = (
                x * angle.cos() + y * angle.sin(),
                -x * angle.sin() + y * angle.cos(),
            );
            if (u / a).powi(2) + (v / b).powi(2) <= 1. {
                *p = RED_PUPIL;
            }
        }
        im
    };
    let mut results = Vec::new();
    for flip in [0, 6, 3, 5] {
        let im = make(flip);
        let op = correct(&im, c, 40.);
        assert!(op.correlation.abs() > 0.3, "tilted: {}", op.correlation);
        let out = rendered(&im, std::slice::from_ref(&op));
        // Inside the pupil corrected, well outside its short axis untouched.
        let inside = pixel(&out, c[0] + 15. * angle.cos(), c[1] + 15. * angle.sin());
        assert!(chroma(inside) < 2., "flip {flip}: {inside:?}");
        let (nx, ny) = (-angle.sin(), angle.cos());
        let far = [c[0] + 18. * nx, c[1] + 18. * ny];
        assert_eq!(pixel(&out, far[0], far[1]), SKIN, "flip {flip}");
        results.push(out.pixels);
    }
    // The decoded result does not depend on how the camera was held.
    for r in &results[1..] {
        let worst = r
            .iter()
            .zip(&results[0])
            .flat_map(|(p, q)| (0..3).map(move |i| (p[i] - q[i]).abs()))
            .fold(0., f32::max);
        assert!(worst < 1e-4, "{worst}");
    }
}
#[test]
fn darken_and_pupil_size() {
    let c = [100., 100.];
    let im = image(200, 200, 0, &[eye(c, 15., BROWN_IRIS)]);
    let op = correct(&im, c, 40.);
    let at = |op: &RedEyeOp, x: f32| pixel(&rendered(&im, std::slice::from_ref(op)), x, c[1]);
    let level = |p: [f32; 3]| p[1];
    let darker = |d: f32| {
        level(at(
            &RedEyeOp {
                darken: d,
                ..op.clone()
            },
            c[0],
        ))
    };
    assert!(darker(0.) > darker(0.5) && darker(0.5) > darker(1.));
    // A larger pupil size reaches further into the iris.
    let edge = c[0] + 19.;
    let small = at(
        &RedEyeOp {
            pupil_size: 0.,
            ..op.clone()
        },
        edge,
    );
    let large = at(
        &RedEyeOp {
            pupil_size: 1.,
            ..op.clone()
        },
        edge,
    );
    assert_eq!(small, pixel(&im, edge, c[1]));
    assert!(chroma(large) < chroma(small));
}
#[test]
fn moving_a_correction_keeps_it_on_the_photo() {
    let mut op = RedEyeOp {
        kind: EyeKind::Red,
        center: [0.9, 0.1],
        radius: [0.01; 2],
        correlation: 0.,
        pupil_size: 0.5,
        darken: 0.5,
    };
    op.translate([5., -7.]);
    assert_eq!(op.center, [1., 0.]);
    op.validate().unwrap();
}
#[test]
fn outline_lies_on_the_ellipse() {
    let op = RedEyeOp {
        kind: EyeKind::Red,
        center: [0.4, 0.6],
        radius: [0.02, 0.01],
        correlation: 0.5,
        pupil_size: 0.5,
        darken: 0.5,
    };
    for p in op.outline(1.5, 36) {
        let inward = [
            op.center[0] + (p[0] - op.center[0]) * 0.99,
            op.center[1] + (p[1] - op.center[1]) * 0.99,
        ];
        let outward = [
            op.center[0] + (p[0] - op.center[0]) * 1.01,
            op.center[1] + (p[1] - op.center[1]) * 1.01,
        ];
        assert!(op.contains(inward, 1.5) && !op.contains(outward, 1.5));
    }
}
#[test]
fn saved_corrections_from_a_later_release_are_skipped() {
    let json = r#"{"red_eye": [
        {"kind": "Red", "center": [0.5, 0.5], "radius": [0.01, 0.01], "pupil_size": 0.5, "darken": 0.5},
        {"kind": "Cat", "center": [0.2, 0.5], "radius": [0.01, 0.01], "pupil_size": 0.5, "darken": 0.5}
    ], "masks": []}"#;
    let local: crate::model::recipe::LocalEdits = serde_json::from_str(json).unwrap();
    assert_eq!(local.red_eye.len(), 1);
    assert_eq!(local.red_eye[0].center, [0.5, 0.5]);
    // Saved and read back unchanged, the later release's correction included, even
    // after an edit here.
    let mut edited = local;
    edited.red_eye[0].darken = 0.8;
    let text = serde_json::to_string(&edited).unwrap();
    assert!(text.contains("\"Cat\""), "{text}");
    let back: crate::model::recipe::LocalEdits = serde_json::from_str(&text).unwrap();
    assert_eq!(back, edited);
    // A later correction between two of ours keeps its place.
    let json = r#"[
        {"kind": "Red", "center": [0.5, 0.5], "radius": [0.01, 0.01], "pupil_size": 0.5, "darken": 0.5},
        {"kind": "Cat", "center": [0.2, 0.5]},
        {"kind": "Red", "center": [0.7, 0.5], "radius": [0.01, 0.01], "pupil_size": 0.5, "darken": 0.5}
    ]"#;
    let list: RedEyeList = serde_json::from_str(json).unwrap();
    assert_eq!(list.len(), 2);
    let back: Vec<serde_json::Value> =
        serde_json::from_str(&serde_json::to_string(&list).unwrap()).unwrap();
    assert_eq!(back[1]["kind"], "Cat");
    assert_eq!(back[2]["center"][0], 0.7);
    // Through the recipe too.
    let recipe = crate::model::recipe::Recipe::default().with_local(edited.clone());
    assert_eq!(recipe.split_local().1, edited);
}

/// Glowing pet pupils (yellow-green, white, cyan) inside darker irises.
fn pets() -> (CameraImage, Vec<[f32; 2]>) {
    let glows = [[0.5, 0.8, 0.15], [0.9, 0.9, 0.8], [0.07, 0.6, 0.6]];
    let centers = vec![[80., 80.], [220., 80.], [150., 200.]];
    let eyes: Vec<Eye> = centers
        .iter()
        .zip(glows)
        .map(|(c, glow)| Eye {
            center: *c,
            pupil: 18.,
            iris: 40.,
            iris_color: [0.25, 0.18, 0.05],
            pupil_color: glow,
        })
        .collect();
    (image(300, 280, 0, &eyes), centers)
}
fn pet(im: &CameraImage, at: [f32; 2], catchlight: Option<[f32; 2]>) -> RedEyeOp {
    let frame = ImageFrame::new(im);
    let pupil = find_pupil(
        im,
        frame.to_image(at[0], at[1]),
        50. / frame.long_edge(),
        Glow::Bright,
    )
    .unwrap_or_else(|e| panic!("at {at:?}: {e:?}"));
    RedEyeOp {
        kind: EyeKind::Pet { catchlight },
        center: pupil.center,
        radius: pupil.radius,
        correlation: pupil.correlation,
        pupil_size: DEFAULT_PUPIL_SIZE,
        darken: DEFAULT_DARKEN,
    }
}
#[test]
fn finds_glowing_pet_pupils_of_any_colour() {
    let (im, centers) = pets();
    let frame = ImageFrame::new(&im);
    for c in centers {
        let op = pet(&im, c, None);
        let [x, y] = frame.to_source(op.center);
        assert!((x - c[0]).abs() < 0.6 && (y - c[1]).abs() < 0.6, "{x},{y}");
        assert!(
            op.radius.iter().all(|a| (a * 300. - 18.).abs() < 1.5),
            "{:?}",
            op.radius
        );
        // Not red, so Red Eye finds nothing there.
        let red = find_pupil(&im, op.center, 50. / 300., Glow::Red);
        assert_eq!(red, Err(DetectError::NotRed));
    }
    // A dull eye has no glow to find.
    let mut e = eye([100., 100.], 15., BROWN_IRIS);
    e.pupil_color = [0.02; 3];
    let dull = image(200, 200, 0, &[e]);
    let found = find_pupil(&dull, [0.5, 0.5], 40. / 200., Glow::Bright);
    assert_eq!(found, Err(DetectError::NotRed));
}
#[test]
fn pet_pupils_turn_black_with_a_catchlight_where_it_is_put() {
    let (im, centers) = pets();
    let c = centers[1];
    let plain = pet(&im, c, None);
    let out = rendered(&im, std::slice::from_ref(&plain));
    let centre = pixel(&out, c[0], c[1]);
    assert!(centre.iter().all(|v| *v < 1e-3), "black: {centre:?}");
    // Darken doesn't change a pet eye.
    let darker = RedEyeOp {
        darken: 1.,
        ..plain
    };
    assert_eq!(rendered(&im, &[darker]).pixels, out.pixels);
    // A catchlight up and to the right, half a semi-axis away (and the falloff's
    // half-way distance: 0.585 + 0.975 × 0.5, × 0.953 for Pet Eye).
    let lit = pet(&im, c, Some([0.5, -0.5]));
    let out = rendered(&im, std::slice::from_ref(&lit));
    let half = (0.585 + 0.975 * 0.5) * 0.953;
    let at = [c[0] + 0.5 * 18. * half, c[1] - 0.5 * 18. * half];
    let light = pixel(&out, at[0].round(), at[1].round());
    assert!(light.iter().all(|v| *v > 0.2), "catchlight: {light:?}");
    let mirrored = pixel(&out, (2. * c[0] - at[0]).round(), at[1].round());
    assert!(mirrored.iter().all(|v| *v < 0.02), "{mirrored:?}");
    assert!(lit.catchlight_at(300. / 280.).is_some());
    // Placed outside the pupil, it is kept on its edge.
    let mut moved = lit.clone();
    moved.set_catchlight([lit.center[0] + 0.5, lit.center[1]], 300. / 280.);
    let EyeKind::Pet {
        catchlight: Some(m),
    } = moved.kind
    else {
        panic!()
    };
    assert!((m[0] - 1.).abs() < 1e-5 && m[1].abs() < 1e-5, "{m:?}");
    moved.validate().unwrap();
}
#[test]
fn catchlights_turn_with_the_camera() {
    // The same decoded pupil under each camera orientation: its catchlight lands on
    // the same decoded pixel.
    let mut results = Vec::new();
    for flip in [0, 6, 3, 5] {
        let mut e = eye([150., 100.], 20., [0.25, 0.18, 0.05]);
        e.pupil_color = [0.9, 0.9, 0.8];
        let im = image(300, 200, flip, &[e]);
        let frame = ImageFrame::new(&im);
        // A decoded offset up and to the right, as an image-space one.
        let op = pet(&im, [150., 100.], Some([0., 0.]));
        let half = (0.585 + 0.975 * 0.5) * 0.953;
        let target = frame.to_image(150. + 0.6 * 20. * half, 100. - 0.3 * 20. * half);
        let mut op = op;
        op.set_catchlight(target, frame.aspect());
        results.push(rendered(&im, &[op]).pixels);
    }
    for r in &results[1..] {
        let worst = r
            .iter()
            .zip(&results[0])
            .flat_map(|(p, q)| (0..3).map(move |i| (p[i] - q[i]).abs()))
            .fold(0., f32::max);
        assert!(worst < 1e-3, "{worst}");
    }
}
#[test]
fn a_catchlight_in_a_glowing_pupil_is_not_taken_for_it() {
    let c = [100., 100.];
    let mut e = eye(c, 16., [0.25, 0.18, 0.05]);
    // A dim glow, darker than the face around the eye.
    e.pupil_color = [0.3, 0.45, 0.1];
    let mut im = image(200, 200, 0, &[e]);
    for (i, p) in im.pixels.iter_mut().enumerate() {
        let (x, y) = ((i % 200) as f32, (i / 200) as f32);
        if (x - 95.).hypot(y - 96.) < 3.5 {
            *p = [1.; 3];
        }
    }
    let op = pet(&im, c, None);
    let [x, y] = ImageFrame::new(&im).to_source(op.center);
    assert!((x - c[0]).abs() < 1. && (y - c[1]).abs() < 1., "{x},{y}");
    assert!(
        op.radius.iter().all(|a| (a * 200. - 16.).abs() < 1.5),
        "{:?}",
        op.radius.map(|a| a * 200.)
    );
}
#[test]
fn small_glowing_pupils_are_found_in_wide_circles() {
    // A circle nearly seven times the pupil's radius.
    let c = [150., 150.];
    let mut e = eye(c, 8., [0.25, 0.18, 0.05]);
    e.pupil_color = [0.5, 0.8, 0.15];
    let mut im = image(300, 300, 0, &[e]);
    // Dark fur around a lighter iris.
    for p in im.pixels.iter_mut().filter(|p| **p == SKIN) {
        *p = [0.15, 0.12, 0.1];
    }
    let frame = ImageFrame::new(&im);
    let found = find_pupil(&im, frame.to_image(c[0], c[1]), 55. / 300., Glow::Bright)
        .expect("a glowing pupil");
    assert!(
        found.radius.iter().all(|a| (a * 300. - 8.).abs() < 1.5),
        "{:?}",
        found.radius.map(|a| a * 300.)
    );
}
#[test]
fn catchlights_stay_inside_tilted_pupils() {
    let mut op = RedEyeOp {
        kind: EyeKind::Pet { catchlight: None },
        center: [0.5, 0.5],
        radius: [0.02, 0.02],
        correlation: 0.8,
        pupil_size: 0.5,
        darken: 0.5,
    };
    // Dragged far out along x: kept on the tilted ellipse, not the unit circle.
    op.set_catchlight([0.9, 0.5], 1.);
    let EyeKind::Pet {
        catchlight: Some(c),
    } = op.kind
    else {
        panic!()
    };
    let m = (c[0] * c[0] - 1.6 * c[0] * c[1] + c[1] * c[1]) / (1. - 0.64);
    assert!((m.sqrt() - 1.).abs() < 1e-4, "{c:?} at {}", m.sqrt());
    op.validate().unwrap();
}
#[test]
fn a_dark_pupil_with_a_catchlight_is_not_a_glow() {
    let c = [100., 100.];
    let mut e = eye(c, 14., [0.35, 0.25, 0.1]);
    e.pupil_color = [0.02; 3];
    let mut im = image(200, 200, 0, &[e]);
    for p in im.pixels.iter_mut().filter(|p| **p == SKIN) {
        *p = [0.15, 0.12, 0.1];
    }
    for (i, p) in im.pixels.iter_mut().enumerate() {
        let (x, y) = ((i % 200) as f32, (i / 200) as f32);
        if (x - 96.).hypot(y - 96.) < 2.5 {
            *p = [1.; 3];
        }
    }
    let frame = ImageFrame::new(&im);
    let found = find_pupil(&im, frame.to_image(c[0], c[1]), 60. / 200., Glow::Bright);
    assert_eq!(found, Err(DetectError::NotRed));
}
#[test]
fn the_default_catchlight_fits_tilted_pupils() {
    let mut op = RedEyeOp {
        kind: EyeKind::Pet {
            catchlight: Some(DEFAULT_CATCHLIGHT),
        },
        center: [0.5, 0.5],
        radius: [0.02, 0.02],
        correlation: 0.95,
        pupil_size: 0.5,
        darken: 0.5,
    };
    assert!(op.validate().is_err());
    op.fit_catchlight();
    op.validate().unwrap();
}
#[test]
fn a_light_iris_around_a_dark_pupil_is_not_a_glow() {
    let c = [100., 100.];
    // Wide and constricted pupils alike.
    for pupil in [10., 5.] {
        let mut e = eye(c, pupil, [0.6, 0.5, 0.3]);
        e.iris = 24.;
        e.pupil_color = [0.02; 3];
        let mut im = image(200, 200, 0, &[e]);
        for p in im.pixels.iter_mut().filter(|p| **p == SKIN) {
            *p = [0.15, 0.12, 0.1];
        }
        let frame = ImageFrame::new(&im);
        let found = find_pupil(&im, frame.to_image(c[0], c[1]), 45. / 200., Glow::Bright);
        assert_eq!(found, Err(DetectError::NotRed), "pupil {pupil}");
    }
}
