//! AI Remove's patches: the rendition a generator is given and its inverse, the
//! region a request covers, and patches laid among Heal and Clone spots, through
//! the cache and the export path alike.
use super::heal::{self, GenerateBlend, Placed};
use super::rendition::{self, Rendition};
use super::*;
use crate::camera_data::{CameraImage, Metadata};
use crate::model::image_frame::ImageFrame;
use crate::model::recipe::Recipe;
use crate::model::retouch::{RetouchMode, RetouchPatch, RetouchShape};
use rawmakase_cancel::Cancel;
use rawmakase_model::camera_profiles::RGB_TO_PRO;
use rawmakase_model::color::{mul, srgb_decode};
use rawmakase_model::storage::{mask_assets, patches};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// A camera's matrix to linear sRGB, as a RAW's metadata carries one.
const CAMERA: [[f32; 3]; 3] = [[1.6, -0.45, -0.15], [-0.2, 1.45, -0.25], [0.0, -0.4, 1.4]];

fn image(width: u32, height: u32, f: impl Fn(f32, f32) -> [f32; 3]) -> CameraImage {
    CameraImage {
        width,
        height,
        pixels: (0..width * height)
            .map(|i| f((i % width) as f32, (i / width) as f32))
            .collect(),
        metadata: Metadata {
            width,
            height,
            wb: [1.; 3],
            matrix: CAMERA,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
        recovered: Default::default(),
    }
}

/// A recipe that changes nothing before retouching.
fn plain() -> Recipe {
    Recipe {
        noise_chroma: 0.,
        noise_luma: 0.,
        ..Default::default()
    }
}

/// A brushed area around pixel (`x`, `y`) of `im`, `r` pixels wide, made by `mode`.
fn brush(im: &CameraImage, x: f32, y: f32, r: f32) -> RetouchOp {
    let (w, h) = (im.width as f32, im.height as f32);
    let at = |dx: f32| [(x + dx + 0.5) / w, (y + 0.5) / h];
    RetouchOp {
        mode: RetouchMode::Heal,
        shape: RetouchShape::Brush {
            points: vec![at(-4.), at(0.), at(4.)].into(),
            radius: r / w.max(h),
        },
        feather: 0.5,
        opacity: 1.,
        offset: [0.; 2],
        patch: None,
    }
}

/// What a remover answers: `truth`'s pixels where `input` is painted, with each
/// value raised by `drift` (a model's own exposure error), and `input`'s elsewhere.
fn answer(input: &rendition::Input, truth: &rendition::Input, drift: f32) -> Vec<u8> {
    let mut out = input.rgb.clone();
    for (i, c) in input.coverage.iter().enumerate() {
        if *c > 0 {
            for k in 0..3 {
                let v = f32::from(truth.rgb[i * 3 + k]) * drift;
                out[i * 3 + k] = v.round().clamp(0., 255.) as u8;
            }
        }
    }
    out
}

/// `op` made a Generate operation laying `samples` over `input`'s region.
fn generated(op: &RetouchOp, input: &rendition::Input, samples: &[f32]) -> RetouchOp {
    let bitmap = patches::encode(input.region.width, input.region.height, samples).unwrap();
    let id = mask_assets::register(bitmap).unwrap();
    RetouchOp {
        mode: RetouchMode::Generate,
        patch: Some(RetouchPatch {
            id,
            width: input.region.width,
            height: input.region.height,
            rect: input.region.rect,
            generator: "dev.rawmakase.remove/fill".into(),
            model: "test".into(),
            ai_denoise: false,
        }),
        ..op.clone()
    }
}

/// The relative error of `out` against `truth` where `op` paints: the mean, and
/// the largest of the pixels within 2 of the shape's edge (its seam).
fn errors(out: &CameraImage, truth: &CameraImage, op: &RetouchOp) -> (f32, f32) {
    let frame = ImageFrame::new(truth);
    let placed = Placed::new(op, &frame, heal::FeatherProfile::Measured);
    let rect = placed.dest();
    let rect = [
        rect[0].max(0),
        rect[1].max(0),
        rect[2].min(truth.width as i32),
        rect[3].min(truth.height as i32),
    ];
    let alpha = heal::coverage(
        &placed.points,
        placed.radius,
        placed.feather,
        placed.profile,
        rect,
    );
    let w = (rect[2] - rect[0]) as usize;
    let (mut sum, mut n, mut seam) = (0., 0., 0f32);
    for (k, a) in alpha.iter().enumerate() {
        if *a <= 0. {
            continue;
        }
        let (x, y) = (rect[0] as usize + k % w, rect[1] as usize + k / w);
        let i = y * truth.width as usize + x;
        let e = (0..3)
            .map(|c| {
                ((out.pixels[i][c] + 1e-3) / (truth.pixels[i][c] + 1e-3))
                    .ln()
                    .abs()
            })
            .fold(0f32, f32::max);
        sum += e;
        n += 1.;
        let (cx, cy) = (x as f32, y as f32);
        let d = placed
            .points
            .iter()
            .map(|p| (p[0] - cx).hypot(p[1] - cy))
            .fold(f32::INFINITY, f32::min);
        if (d - placed.radius).abs() < 2. {
            seam = seam.max(e);
        }
    }
    (sum / n, seam)
}

#[test]
fn the_rendition_round_trips_what_it_can() {
    let im = image(4, 4, |_, _| [0.2; 3]);
    let r = plain();
    let rendition = Rendition::of(&im.metadata, &r).unwrap();
    // Mid-tones in sRGB's gamut come back within the 8-bit steps.
    for p in [
        [0.18, 0.18, 0.18],
        [0.1, 0.15, 0.08],
        [0.3, 0.2, 0.25],
        [0.05, 0.06, 0.04],
    ] {
        let back = rendition.inverse(rendition.forward(p));
        for c in 0..3 {
            let e = (back[c] / p[c]).ln().abs();
            assert!(e < 0.03, "{p:?} came back as {back:?}");
        }
    }
    // Brighter than F renders white, a value clips: lossy, as documented.
    let clipped = rendition.inverse(rendition.forward([40.; 3]));
    assert!(clipped[1] < 40.);
    // What it captured is what it inverts with, whatever the recipe does next.
    let mut warmer = r;
    warmer.wb = [1.4, 1., 0.7];
    warmer.exposure = 1.5;
    assert_eq!(Rendition::of(&im.metadata, &warmer).unwrap(), rendition);
}

#[test]
fn a_rendered_file_is_given_its_own_pixels() {
    let mut im = image(4, 4, |_, _| [0.2; 3]);
    im.metadata.source = crate::camera_data::Source::Rendered;
    let rendition = Rendition::of(&im.metadata, &plain()).unwrap();
    for srgb in [
        [200u8, 120, 40],
        [12, 14, 16],
        [128, 128, 128],
        [250, 250, 250],
        [3, 90, 200],
    ] {
        let linear = srgb.map(|v| srgb_decode(f32::from(v) / 255.));
        // The file's pixel as it opens: its pseudo-scene in linear ProPhoto.
        let scene = crate::develop::rendered::scene_from_rendered(mul(RGB_TO_PRO, linear));
        let out = rendition.forward(scene);
        for c in 0..3 {
            assert!(out[c].abs_diff(srgb[c]) <= 1, "{srgb:?} became {out:?}");
        }
        let back = rendition.forward(rendition.inverse(out));
        assert_eq!(back, out);
    }
}

#[test]
fn a_region_is_the_shape_with_its_margin_within_the_photo() {
    let im = image(1000, 600, |_, _| [0.2; 3]);
    let frame = ImageFrame::new(&im);
    // A small spot gets at least 64 pixels around it.
    let small = brush(&im, 500., 300., 10.);
    let region = rendition::region(&small, &frame).unwrap();
    let px = |r: &rendition::Region| {
        [
            (r.rect[2] - r.rect[0]) * 1000.,
            (r.rect[3] - r.rect[1]) * 600.,
        ]
    };
    let [w, h] = px(&region);
    assert!(
        (w - (8. + 20. + 128.)).abs() < 1.5 && (h - (20. + 128.)).abs() < 1.5,
        "{w}x{h}"
    );
    assert_eq!(region.scale, 1.);
    assert_eq!(
        (region.width, region.height),
        (w.round() as u32, h.round() as u32)
    );
    // Near an edge it is clipped to the photo.
    let edge = rendition::region(&brush(&im, 5., 300., 10.), &frame).unwrap();
    assert_eq!(edge.rect[0], 0.);
    // A large area of a large photo is reduced to at most 2048 pixels.
    let big = image(8000, 6000, |_, _| [0.2; 3]);
    let wide = RetouchOp {
        shape: RetouchShape::Brush {
            points: vec![[0.2, 0.5], [0.8, 0.5]].into(),
            radius: 0.1,
        },
        ..brush(&big, 0., 0., 1.)
    };
    let reduced = rendition::region(&wide, &ImageFrame::new(&big)).unwrap();
    assert!(reduced.width <= rendition::MAX_SIDE && reduced.height <= rendition::MAX_SIDE);
    assert!(reduced.width == rendition::MAX_SIDE || reduced.height == rendition::MAX_SIDE);
    assert!(reduced.scale < 1.);
    // Outside the photo there is nothing to send.
    let outside = RetouchOp {
        shape: RetouchShape::Spot {
            center: [1.5, 0.5],
            radius: 0.01,
        },
        ..small
    };
    assert_eq!(rendition::region(&outside, &frame), None);
}

/// A patch made through the rendition, laid with Heal's membrane and with a plain
/// feathered composite, on saturated, dark, gradient and textured surroundings,
/// for a remover that answers exactly and one that drifts 3% brighter: the
/// membrane is never worse than the composite, and where the rendition clipped
/// (the saturated ground lies outside sRGB) it takes the patch back to the
/// photo's colour, which the composite can't. This is what decided
/// `GENERATE_BLEND`.
#[test]
fn the_membrane_lays_a_patch_better_than_a_composite() {
    type Ground = Box<dyn Fn(f32, f32) -> [f32; 3]>;
    let grounds: [(&str, Ground); 4] = [
        ("saturated", Box::new(|_, _| [0.03, 0.2, 0.45])),
        ("dark", Box::new(|x, _| [0.004 + x * 1e-5, 0.005, 0.0045])),
        (
            "gradient",
            Box::new(|x, y| [0.04 + x * 0.003, 0.05 + y * 0.002, 0.06]),
        ),
        (
            "textured",
            Box::new(|x, y| {
                let t = 1. + 0.3 * (x / 3.).sin() * (y / 4.).cos();
                [0.2 * t, 0.18 * t, 0.15 * t]
            }),
        ),
    ];
    let cancel = AtomicBool::new(false);
    let r = plain();
    for (name, ground) in grounds {
        let truth = image(200, 150, &ground);
        let object = image(200, 150, |x, y| {
            if (x - 100.).hypot(y - 75.) < 10. {
                [0.9, 0.85, 0.8]
            } else {
                ground(x, y)
            }
        });
        let op = brush(&object, 100., 75., 16.);
        let rendition = Rendition::of(&object.metadata, &r).unwrap();
        let input =
            rendition::render(&object, &r, None, &op, &rendition, Cancel::new(&cancel)).unwrap();
        let clean =
            rendition::render(&truth, &r, None, &op, &rendition, Cancel::new(&cancel)).unwrap();
        assert!(input.coverage.contains(&255));
        for drift in [1., 1.03] {
            let result = answer(&input, &clean, drift);
            let samples = rendition::patch_samples(
                &rendition,
                &result,
                input.region.width,
                input.region.height,
                input.region.width as usize * 3,
            )
            .unwrap();
            let op = generated(&op, &input, &samples);
            let frame = ImageFrame::new(&object);
            let pixels = super::patch::resolve(op.patch.as_ref().unwrap()).unwrap();
            let placed =
                Placed::new(&op, &frame, heal::FeatherProfile::Measured).with_patch(pixels, &frame);
            let laid = |blend| {
                let mut out = object.clone();
                heal::apply_blend(&mut out, &placed, blend);
                errors(&out, &truth, &op)
            };
            let (membrane, membrane_seam) = laid(GenerateBlend::Membrane);
            let (composite, composite_seam) = laid(GenerateBlend::Composite);
            eprintln!(
                "{name} ×{drift}: membrane {membrane:.4} (seam {membrane_seam:.4}), \
                 composite {composite:.4} (seam {composite_seam:.4})"
            );
            assert!(membrane <= composite + 2e-3, "{name} ×{drift}");
            assert!(membrane_seam <= composite_seam + 2e-3, "{name} ×{drift}");
            // With an exact remover the object is gone and what is left is the
            // rendition's own error. A drift inside the painted area stays: the
            // ring outside it, which the membrane matches, never saw it.
            if drift == 1. {
                assert!(membrane < 0.03, "{name}: {membrane}");
            }
        }
    }
}

/// Generate operations interleave with Heal and Clone: one Heal copies from the
/// patched area, and the cache's incremental update equals a full rebuild (the
/// export path) at every change. The patch is camera RGB, so the image the layer
/// makes doesn't follow white balance or exposure.
#[test]
fn generated_spots_interleave_with_heal_and_clone() {
    let base = image(700, 520, |x, y| {
        let v = 0.05 + 0.4 * ((x * 0.013).sin() * (y * 0.021).cos()).abs();
        [v, v * 0.9, v * 0.7]
    });
    let samples: Vec<f32> = (0..60 * 40 * 3)
        .map(|i| 0.1 + (i % 17) as f32 * 0.01)
        .collect();
    let bitmap = patches::encode(60, 40, &samples).unwrap();
    let id = mask_assets::register(bitmap).unwrap();
    let mut patched = brush(&base, 300., 200., 14.);
    patched.mode = RetouchMode::Generate;
    patched.patch = Some(RetouchPatch {
        id,
        width: 60,
        height: 40,
        rect: [260. / 700., 170. / 520., 340. / 700., 230. / 520.],
        generator: "dev.rawmakase.remove/fill".into(),
        model: "test".into(),
        ai_denoise: false,
    });
    let mut copying = brush(&base, 420., 200., 12.);
    copying.offset = [-120. / 700., 0.];
    let mut ops = vec![
        RetouchOp {
            mode: RetouchMode::Clone,
            offset: [0.05, 0.05],
            ..brush(&base, 100., 100., 20.)
        },
        patched,
        // Copies from the patched area.
        copying,
    ];
    let base = Arc::new(base);
    let cancel = AtomicBool::new(false);
    let mut cache = RetouchCache::default();
    let check = |cache: &mut RetouchCache, ops: &[RetouchOp]| {
        let ops = Retouching {
            red_eye: &[],
            retouch: ops,
        };
        let incremental = cache.get(&base, ops, Cancel::new(&cancel)).unwrap();
        let full = apply(&base, ops).unwrap();
        assert_eq!(incremental.pixels, full.pixels);
        incremental
    };
    let first = check(&mut cache, &ops);
    // The patched area holds the patch.
    let i = 200 * 700 + 300;
    let frame = ImageFrame::new(&base);
    let at = frame.to_image(300., 200.);
    let expected = super::patch::resolve(ops[1].patch.as_ref().unwrap())
        .unwrap()
        .sample(at);
    for c in 0..3 {
        assert!(
            (first.pixels[i][c] - expected[c]).abs() < 0.05,
            "{:?} {expected:?}",
            first.pixels[i]
        );
    }
    ops[1].opacity = 0.5;
    check(&mut cache, &ops);
    ops[1].feather = 0.9;
    check(&mut cache, &ops);
    ops[0].translate([0.02, 0.]);
    check(&mut cache, &ops);
    ops.swap(0, 1);
    check(&mut cache, &ops);
    let patched = ops.remove(0);
    check(&mut cache, &ops);

    // White balance and exposure come after: the retouched image is the same.
    let mut r = plain();
    r.retouch = vec![patched];
    let retouched = |r: &Recipe| {
        crate::develop::quality::retouched(&base, r, None, Cancel::new(&cancel), None)
            .unwrap()
            .image
    };
    let mut warmer = r.clone();
    warmer.wb = [1.5, 1., 0.6];
    warmer.exposure = 2.;
    assert_eq!(retouched(&r).pixels, retouched(&warmer).pixels);
}

/// A patch that can't be provided fails the render, in the cache and the export
/// path alike, and never renders as though the spot weren't there; the cache
/// keeps what it had.
#[test]
fn a_patch_that_cant_be_provided_fails_the_render() {
    let base = Arc::new(image(300, 200, |_, _| [0.2; 3]));
    let mut op = brush(&base, 150., 100., 10.);
    op.mode = RetouchMode::Generate;
    op.patch = Some(RetouchPatch {
        id: format!("sha256:{}", "9e".repeat(32)),
        width: 4,
        height: 4,
        rect: [0.4, 0.4, 0.6, 0.6],
        generator: "dev.rawmakase.remove/fill".into(),
        model: "test".into(),
        ai_denoise: false,
    });
    let ops = [op.clone()];
    let retouching = Retouching {
        red_eye: &[],
        retouch: &ops,
    };
    let error = apply(&base, retouching).err().unwrap();
    assert!(error.to_string().contains("AI Remove"), "{error}");
    assert!(ensure_patches(&ops).is_err());
    let cancel = AtomicBool::new(false);
    let mut cache = RetouchCache::default();
    let before = cache
        .get(&base, Retouching::default(), Cancel::new(&cancel))
        .unwrap();
    assert!(cache.get(&base, retouching, Cancel::new(&cancel)).is_err());
    let again = cache
        .get(&base, Retouching::default(), Cancel::new(&cancel))
        .unwrap();
    assert!(Arc::ptr_eq(&before, &again));
    // A coverage raster under a patch's id is no patch.
    let coverage = mask_assets::register(rawmakase_model::storage::bitmaps::Bitmap {
        width: 4,
        height: 4,
        channels: 1,
        depth: 1,
        data: vec![9; 16],
    })
    .unwrap();
    let mut wrong = op;
    wrong.patch.as_mut().unwrap().id = coverage;
    assert!(ensure_patches(&[wrong]).is_err());
    // The export path's measurement goes through the same layer.
    let mut r = plain();
    r.retouch = ops.to_vec();
    assert!(
        crate::develop::quality::retouched(&base, &r, None, Cancel::new(&cancel), None).is_err()
    );
}

/// HDR files and edits are refused before anything is rendered.
#[test]
fn hdr_edits_are_refused() {
    let im = image(64, 64, |_, _| [0.2; 3]);
    let mut r = plain();
    r.hdr.enabled = true;
    let op = brush(&im, 32., 32., 6.);
    let rendition = Rendition::of(&im.metadata, &r).unwrap();
    let cancel = AtomicBool::new(false);
    let error =
        rendition::render(&im, &r, None, &op, &rendition, Cancel::new(&cancel)).unwrap_err();
    assert_eq!(error.to_string(), rendition::HDR_REFUSAL);
}
