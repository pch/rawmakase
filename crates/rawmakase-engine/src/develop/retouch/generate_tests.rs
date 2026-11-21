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

/// A Heal brushed area around pixel (`x`, `y`) of `im`, with a radius of `r` pixels.
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
            summary: None,
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
        &placed.starts,
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
/// photo's color, which the composite can't. This is what decided
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
        summary: None,
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
        summary: None,
    });
    let ops = [op.clone()];
    let retouching = Retouching {
        red_eye: &[],
        retouch: &ops,
    };
    let error = apply(&base, retouching).err().unwrap();
    assert!(error.to_string().contains("generated"), "{error}");
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

/// The same pixels laid at two places are two placements, and a reference
/// describing the pixels at another size fails even once they are decoded.
#[test]
fn the_decoded_patch_is_the_placement_asked_for() {
    let samples: Vec<f32> = (0..2 * 2 * 3).map(|i| 0.1 + i as f32 * 0.07).collect();
    let id = mask_assets::register(patches::encode(2, 2, &samples).unwrap()).unwrap();
    let at = |rect: [f32; 4], width: u32| RetouchPatch {
        id: id.clone(),
        width,
        height: 2,
        rect,
        generator: "dev.rawmakase.remove/fill".into(),
        model: "test".into(),
        ai_denoise: false,
        summary: None,
    };
    let left = super::patch::resolve(&at([0., 0., 0.5, 0.5], 2)).unwrap();
    let right = super::patch::resolve(&at([0.5, 0.5, 1., 1.], 2)).unwrap();
    // At (0.375, 0.375) the left placement has its bottom-right pixel's centre;
    // the right one, held at its edge, its top-left pixel.
    let pixel = |i: usize| [samples[i * 3], samples[i * 3 + 1], samples[i * 3 + 2]];
    let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|c| (a[c] - b[c]).abs() < 1e-3);
    assert!(near(left.sample([0.375, 0.375]), pixel(3)));
    assert!(near(right.sample([0.375, 0.375]), pixel(0)));
    assert!(super::patch::resolve(&at([0., 0., 0.5, 0.5], 3)).is_err());
}

/// The request renders the photo as every render does: Detail switched off
/// leaves its noise reduction out, though its sliders keep their values.
#[test]
fn a_request_renders_switched_off_panels_as_bypassed() {
    use crate::model::panels::{Panel, PanelState};
    let im = image(64, 48, |x, y| {
        let n = ((x * 7. + y * 13.) % 5.) * 0.01;
        [0.2 + n, 0.18 - n, 0.15 + n]
    });
    let op = brush(&im, 32., 24., 6.);
    let cancel = AtomicBool::new(false);
    let render = |r: &Recipe| {
        let rendition = Rendition::of(&im.metadata, r).unwrap();
        rendition::render(&im, r, None, &op, &rendition, Cancel::new(&cancel)).unwrap()
    };
    let mut noisy = plain();
    noisy.noise_chroma = 60.;
    noisy.noise_luma = 60.;
    let mut off = noisy.clone();
    off.panels.set(Panel::Detail, PanelState::Off);
    assert_ne!(render(&noisy).rgb, render(&plain()).rgb);
    assert_eq!(render(&off).rgb, render(&plain()).rgb);
}

/// Outside the painted area the patch is the photo's own rendition, whatever
/// the program wrote there: the protocol leaves those pixels to it.
#[test]
fn what_a_program_writes_outside_the_painted_area_is_not_kept() {
    let im = image(64, 48, |_, _| [0.2; 3]);
    let op = brush(&im, 32., 24., 6.);
    let r = plain();
    let rendition = Rendition::of(&im.metadata, &r).unwrap();
    let cancel = AtomicBool::new(false);
    let input = rendition::render(&im, &r, None, &op, &rendition, Cancel::new(&cancel)).unwrap();
    let stride = input.region.width as usize * 3 + 6;
    let rows = input.region.height as usize;
    let garbage = vec![7u8; stride * rows];
    let kept = rendition::keep_unpainted(&input, &garbage, stride).unwrap();
    for (i, c) in input.coverage.iter().enumerate() {
        let want = if *c > 0 {
            [7; 3]
        } else {
            [input.rgb[i * 3], input.rgb[i * 3 + 1], input.rgb[i * 3 + 2]]
        };
        assert_eq!(kept[i * 3..i * 3 + 3], want);
    }
    assert!(rendition::keep_unpainted(&input, &garbage[..10], stride).is_err());
}

/// A deterministic, roughly unit Gaussian value for pixel (`x`, `y`) and `k`.
fn gauss(x: f32, y: f32, k: u64) -> f32 {
    let mut h = (x as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(k.wrapping_mul(0x1656_67B1_9E37_79F9));
    let mut sum = 0.;
    for _ in 0..4 {
        h ^= h >> 31;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 29;
        sum += (h >> 40) as f32 / (1u64 << 24) as f32;
    }
    (sum - 2.) * 3f32.sqrt()
}

/// The RMS of channel `c`'s 4-neighbour Laplacian over the pixels of `im` within
/// `r` of `center`, in square-root values (`sqrt`) or log values: how much fine
/// texture or grain is there.
fn roughness(im: &CameraImage, center: [f32; 2], r: f32, c: usize, sqrt: bool) -> f32 {
    let v = |x: i32, y: i32| {
        let p = im.pixels[(y as u32 * im.width + x as u32) as usize][c].max(0.);
        if sqrt { p.sqrt() } else { (p + 1e-3).ln() }
    };
    let (mut sum, mut n) = (0., 0.);
    for y in 1..im.height as i32 - 1 {
        for x in 1..im.width as i32 - 1 {
            if (x as f32 - center[0]).hypot(y as f32 - center[1]) >= r {
                continue;
            }
            let l = v(x, y) - (v(x - 1, y) + v(x + 1, y) + v(x, y - 1) + v(x, y + 1)) / 4.;
            sum += l * l;
            n += 1.;
        }
    }
    (sum / n).sqrt()
}

/// A Generate operation over `rect` (frame pixels of `im`) laying a `w` × `h`
/// patch whose pixel (`i`, `j`) is `f` of its centre in frame pixels, brushed at
/// (`x`, `y`) with radius `r`.
fn laid_over(
    im: &CameraImage,
    rect: [f32; 4],
    (w, h): (u32, u32),
    f: impl Fn(f32, f32) -> [f32; 3],
    (x, y, r): (f32, f32, f32),
) -> RetouchOp {
    let (sx, sy) = (
        (rect[2] - rect[0]) / w as f32,
        (rect[3] - rect[1]) / h as f32,
    );
    let mut samples = Vec::with_capacity((w * h * 3) as usize);
    for j in 0..h {
        for i in 0..w {
            samples.extend(f(
                rect[0] + (i as f32 + 0.5) * sx,
                rect[1] + (j as f32 + 0.5) * sy,
            ));
        }
    }
    let id = mask_assets::register(patches::encode(w, h, &samples).unwrap()).unwrap();
    let (iw, ih) = (im.width as f32, im.height as f32);
    RetouchOp {
        mode: RetouchMode::Generate,
        patch: Some(RetouchPatch {
            id,
            width: w,
            height: h,
            rect: [rect[0] / iw, rect[1] / ih, rect[2] / iw, rect[3] / ih],
            generator: "dev.rawmakase.remove/fill".into(),
            model: "test".into(),
            ai_denoise: false,
            summary: None,
        }),
        ..brush(im, x, y, r)
    }
}

/// `op` laid on `im` with `blend`.
fn lay(im: &CameraImage, op: &RetouchOp, blend: GenerateBlend) -> CameraImage {
    let frame = ImageFrame::new(im);
    let pixels = super::patch::resolve(op.patch.as_ref().unwrap()).unwrap();
    let placed = Placed::new(op, &frame, heal::FeatherProfile::Measured).with_patch(pixels, &frame);
    let mut out = im.clone();
    heal::apply_blend(&mut out, &placed, blend);
    out
}

/// A dark region is exposed up before it is quantised to 8 bits, a region whose
/// highlights the default rendering would clip is exposed down, a mid-grey one and
/// a rendered file are sent as they were; the answer is inverted through the
/// exposure the request was sent with.
#[test]
fn each_request_is_exposed_for_its_region() {
    let cancel = AtomicBool::new(false);
    let r = plain();
    let send = |im: &CameraImage| {
        let op = brush(im, 100., 75., 12.);
        let base = Rendition::of(&im.metadata, &r).unwrap();
        let input = rendition::render(im, &r, None, &op, &base, Cancel::new(&cancel)).unwrap();
        (base, input)
    };
    // Shadows: the base rendition quantises them coarsely, the request's finely.
    let dark = image(200, 150, |x, y| {
        let v = 0.004 + 0.003 * ((x * 0.05).sin() * (y * 0.07).cos()).abs();
        [v, v * 1.1, v * 0.9]
    });
    let (base, input) = send(&dark);
    assert_ne!(input.rendition, base);
    let error = |rendition: &Rendition| {
        (0..50)
            .map(|k| {
                let v = 0.004 + 0.003 * k as f32 / 50.;
                let p = [v, v * 1.1, v * 0.9];
                let back = rendition.inverse(rendition.forward(p));
                (0..3)
                    .map(|c| (back[c] / p[c]).ln().abs())
                    .fold(0f32, f32::max)
            })
            .fold(0f32, f32::max)
    };
    eprintln!(
        "shadows: base {:.4}, request {:.4}",
        error(&base),
        error(&input.rendition)
    );
    assert!(error(&base) > 0.03, "{}", error(&base));
    assert!(
        error(&input.rendition) < 0.015,
        "{}",
        error(&input.rendition)
    );
    // The painted area doesn't count: a bright object over the shadows changes nothing.
    let object = image(200, 150, |x, y| {
        if (x - 100.).hypot(y - 75.) < 8. {
            [5.; 3]
        } else {
            dark.pixels[(y as u32 * 200 + x as u32) as usize]
        }
    });
    assert_eq!(send(&object).1.rendition, input.rendition);
    // Highlights above what the default rendering shows are brought below white.
    let bright = image(200, 150, |x, _| [2. + x * 0.02; 3]);
    let (base, input) = send(&bright);
    let top = [6.; 3];
    assert_eq!(base.forward(top), [255; 3]);
    assert!(input.rendition.forward(top).iter().all(|v| *v < 255));
    // Mid-tones are sent as they are.
    let grey = image(200, 150, |x, _| [0.15 + x * 0.001; 3]);
    let (base, input) = send(&grey);
    assert_eq!(input.rendition, base);
    // A rendered file is sent as its own pixels.
    let mut file = dark;
    file.metadata.source = crate::camera_data::Source::Rendered;
    let (base, input) = send(&file);
    assert_eq!(input.rendition, base);
}

/// A patch coarser than the photo takes the fine detail of its surroundings: with
/// the detail the fill is about as textured as the photo around it, without it
/// it is smooth; its tone stays the patch's.
#[test]
fn a_coarse_patch_takes_the_detail_of_its_surroundings() {
    let ground = |x: f32, y: f32| {
        let t = 1. + 0.3 * (x * 2.1).sin() * (y * 1.7).cos() + 0.1 * gauss(x, y, 1);
        [0.12 * t, 0.1 * t, 0.08 * t]
    };
    let truth = image(400, 300, ground);
    let photo = image(400, 300, |x, y| {
        if (x - 200.).hypot(y - 150.) < 30. {
            [0.8; 3]
        } else {
            ground(x, y)
        }
    });
    // A patch four times coarser than the photo: the truth averaged over each pixel.
    let coarse = |cx: f32, cy: f32| {
        let mut sum = [0.; 3];
        for j in 0..4 {
            for i in 0..4 {
                let p = ground((cx - 2. + i as f32).floor(), (cy - 2. + j as f32).floor());
                (0..3).for_each(|c| sum[c] += p[c] / 16.);
            }
        }
        sum
    };
    let op = laid_over(
        &photo,
        [100., 75., 300., 225.],
        (50, 38),
        coarse,
        (200., 150., 34.),
    );
    let detailed = lay(&photo, &op, GenerateBlend::Membrane);
    let smooth = lay(&photo, &op, GenerateBlend::Untextured);
    let centre = [200., 150.];
    let texture = |im: &CameraImage| roughness(im, centre, 16., 1, false);
    eprintln!(
        "texture: truth {:.4}, detailed {:.4}, smooth {:.4}",
        texture(&truth),
        texture(&detailed),
        texture(&smooth)
    );
    assert!(texture(&smooth) < 0.35 * texture(&truth));
    let ratio = texture(&detailed) / texture(&truth);
    assert!((0.6..1.4).contains(&ratio), "{ratio}");
    // Its tone is the patch's: the mean over the fill within 2% of the truth's.
    let mean = |im: &CameraImage| {
        let (mut s, mut n) = (0., 0.);
        for y in 134..166 {
            for x in 184..216 {
                if (x as f32 - 200.).hypot(y as f32 - 150.) < 16. {
                    s += im.pixels[y * 400 + x][1];
                    n += 1.;
                }
            }
        }
        s / n
    };
    let e = (mean(&detailed) / mean(&truth)).ln().abs();
    assert!(e < 0.02, "{e}");
    // A patch as fine as the photo has no detail to take, and about the grain it
    // has already.
    let fine = laid_over(
        &photo,
        [100., 75., 300., 225.],
        (200, 150),
        ground,
        (200., 150., 34.),
    );
    let ratio = texture(&lay(&photo, &fine, GenerateBlend::Membrane))
        / texture(&lay(&photo, &fine, GenerateBlend::Untextured));
    assert!((0.95..1.1).contains(&ratio), "{ratio}");
}

/// A fill smoother than the photo takes the photo's grain; one as grainy takes
/// none to speak of, and a photo without grain gives none.
#[test]
fn a_smooth_fill_takes_the_grain_of_its_surroundings() {
    let grainy = |sigma: f32, k: u64| {
        move |x: f32, y: f32| -> [f32; 3] {
            std::array::from_fn(|c| {
                (0.1f32.sqrt() + sigma * gauss(x, y, k + c as u64))
                    .max(0.)
                    .powi(2)
            })
        }
    };
    let photo = image(300, 200, grainy(0.01, 10));
    let centre = [150., 100.];
    let grain = |im: &CameraImage| roughness(im, centre, 16., 1, true);
    let flat = |_: f32, _: f32| [0.1; 3];
    let smooth = laid_over(
        &photo,
        [80., 30., 220., 170.],
        (140, 140),
        flat,
        (150., 100., 30.),
    );
    let matched = lay(&photo, &smooth, GenerateBlend::Membrane);
    let untextured = lay(&photo, &smooth, GenerateBlend::Untextured);
    eprintln!(
        "grain: photo {:.5}, matched {:.5}, untextured {:.5}",
        grain(&photo),
        grain(&matched),
        grain(&untextured)
    );
    assert!(grain(&untextured) < 0.2 * grain(&photo));
    let ratio = grain(&matched) / grain(&photo);
    assert!((0.75..1.25).contains(&ratio), "{ratio}");
    // A fill as grainy as the photo keeps about its own.
    let noisy = laid_over(
        &photo,
        [80., 30., 220., 170.],
        (140, 140),
        grainy(0.01, 20),
        (150., 100., 30.),
    );
    let ratio = grain(&lay(&photo, &noisy, GenerateBlend::Membrane)) / grain(&photo);
    assert!((0.75..1.25).contains(&ratio), "{ratio}");
    // A photo without grain gives none.
    let clean = image(300, 200, flat);
    assert_eq!(
        lay(&clean, &smooth, GenerateBlend::Membrane).pixels,
        lay(&clean, &smooth, GenerateBlend::Untextured).pixels
    );
}

/// An operation that reruns reads its source as it was before later operations
/// changed it: a Heal spot whose source a later Clone spot covers, then changed,
/// renders in the cache as a full rebuild does. The same holds for a Generate
/// spot, which reads its surroundings for their detail and grain.
#[test]
fn a_rerun_reads_what_later_spots_left_alone() {
    let base = Arc::new(image(700, 300, |x, y| {
        let v = 0.1 + 0.05 * (x * 0.3).sin() * (y * 0.2).cos() + 0.01 * gauss(x, y, 3);
        [v, v * 0.9, v * 0.8]
    }));
    let mut heal = brush(&base, 100., 100., 12.);
    heal.offset = [300. / 700., 0.];
    let mut clone = brush(&base, 400., 100., 14.);
    clone.mode = RetouchMode::Clone;
    clone.offset = [0., 120. / 300.];
    let coarse = |_: f32, _: f32| [0.1, 0.09, 0.08];
    let generate = laid_over(
        &base,
        [500., 100., 700., 300.],
        (50, 50),
        coarse,
        (560., 280., 12.),
    );
    let mut late = brush(&base, 660., 140., 10.);
    late.mode = RetouchMode::Clone;
    late.offset = [-300. / 700., 0.];
    let cancel = AtomicBool::new(false);
    let mut cache = RetouchCache::default();
    let mut ops = vec![heal, clone, generate, late];
    let check = |cache: &mut RetouchCache, ops: &[RetouchOp]| {
        let ops = Retouching {
            red_eye: &[],
            retouch: ops,
        };
        let incremental = cache.get(&base, ops, Cancel::new(&cancel)).unwrap();
        assert_eq!(incremental.pixels, apply(&base, ops).unwrap().pixels);
    };
    check(&mut cache, &ops);
    ops[0].opacity = 0.6;
    check(&mut cache, &ops);
    ops[2].opacity = 0.7;
    check(&mut cache, &ops);
}

/// Grain is measured in the band around the shape, not in the rest of its
/// bounding rectangle: noise beyond the band of a diagonal stroke gives none.
#[test]
fn grain_is_measured_around_the_shape_only() {
    let photo = image(400, 400, |x, y| {
        if (x - y).abs() / 2f32.sqrt() < 24. {
            [0.1; 3]
        } else {
            std::array::from_fn(|c| (0.1f32.sqrt() + 0.02 * gauss(x, y, 30 + c as u64)).powi(2))
        }
    });
    let flat = |_: f32, _: f32| [0.1; 3];
    let mut op = laid_over(
        &photo,
        [80., 80., 320., 320.],
        (240, 240),
        flat,
        (200., 200., 8.),
    );
    op.shape = RetouchShape::Brush {
        points: vec![[100.5 / 400., 100.5 / 400.], [300.5 / 400., 300.5 / 400.]].into(),
        radius: 8. / 400.,
    };
    assert_eq!(
        lay(&photo, &op, GenerateBlend::Membrane).pixels,
        lay(&photo, &op, GenerateBlend::Untextured).pixels
    );
}

/// The grain of the composite, not of the fill alone, matches the photo's:
/// across a wide feather, where the fill and the photo mix, it is as strong as
/// in the core.
#[test]
fn grain_holds_across_the_feather() {
    let photo = image(400, 400, |x, y| {
        std::array::from_fn(|c| (0.1f32.sqrt() + 0.01 * gauss(x, y, 40 + c as u64)).powi(2))
    });
    let flat = |_: f32, _: f32| [0.1; 3];
    let mut op = laid_over(
        &photo,
        [40., 40., 360., 360.],
        (320, 320),
        flat,
        (200., 200., 100.),
    );
    op.feather = 0.75;
    let out = lay(&photo, &op, GenerateBlend::Membrane);
    let ring = |im: &CameraImage| {
        let v = |x: i32, y: i32| im.pixels[(y * 400 + x) as usize][1].max(0.).sqrt();
        let (mut sum, mut n) = (0., 0.);
        for y in 1..399 {
            for x in 1..399 {
                let d = (x as f32 - 200.).hypot(y as f32 - 200.);
                if (45. ..80.).contains(&d) {
                    let l = v(x, y) - (v(x - 1, y) + v(x + 1, y) + v(x, y - 1) + v(x, y + 1)) / 4.;
                    sum += l * l;
                    n += 1.;
                }
            }
        }
        (sum / n).sqrt()
    };
    let ratio = ring(&out) / ring(&photo);
    eprintln!("feather grain ratio {ratio:.3}");
    assert!((0.88..1.12).contains(&ratio), "{ratio}");
}

/// A Generate spot crossing the camera's default crop paints decoded pixels
/// beyond its patch; those take no detail, so it reads only what it declares and
/// the cache renders it as a full rebuild does, with a Clone under its detail's
/// reach before it and one copying from beyond the crop after it.
#[test]
fn a_spot_beyond_its_patch_reads_only_what_it_declares() {
    let mut im = image(1050, 1000, |x, y| [0.1 + 0.01 * gauss(x, y, 5); 3]);
    im.metadata.crop_left = 280;
    im.metadata.crop_top = 310;
    im.metadata.crop_width = 500;
    im.metadata.crop_height = 600;
    let base = Arc::new(im);
    let frame = ImageFrame::new(&base);
    let mut op = brush(&base, 100., 100., 10.);
    op.shape = RetouchShape::Spot {
        center: [0., 0.2],
        radius: 0.1,
    };
    let region = rendition::region(&op, &frame).unwrap();
    let id = mask_assets::register(patches::encode(40, 40, &[0.2; 40 * 40 * 3]).unwrap()).unwrap();
    op.mode = RetouchMode::Generate;
    op.patch = Some(RetouchPatch {
        id,
        width: 40,
        height: 40,
        rect: region.rect,
        generator: "dev.rawmakase.remove/fill".into(),
        model: "test".into(),
        ai_denoise: false,
        summary: None,
    });
    let template = op.clone();
    let spot = |at: [f32; 2], from: [f32; 2]| {
        let center = frame.to_image(at[0], at[1]);
        let source = frame.to_image(from[0], from[1]);
        RetouchOp {
            mode: RetouchMode::Clone,
            shape: RetouchShape::Spot {
                center,
                radius: 8. / frame.long_edge(),
            },
            offset: [source[0] - center[0], source[1] - center[1]],
            patch: None,
            ..template.clone()
        }
    };
    let mut ops = vec![
        spot([243., 523.], [600., 523.]),
        op,
        spot([700., 800.], [231., 395.]),
    ];
    for op in &ops {
        op.validate().unwrap();
    }
    let cancel = AtomicBool::new(false);
    let mut cache = RetouchCache::default();
    for _ in 0..2 {
        let r = Retouching {
            red_eye: &[],
            retouch: &ops,
        };
        let incremental = cache.get(&base, r, Cancel::new(&cancel)).unwrap();
        let full = apply(&base, r).unwrap();
        let differ = incremental
            .pixels
            .iter()
            .zip(&full.pixels)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(differ, 0);
        ops[0].opacity = 0.3;
    }
}

/// A photo with grain of `sigma` in square-root values about `level`.
fn grainy_photo(w: u32, h: u32, level: f32, sigma: f32, k: u64) -> CameraImage {
    image(w, h, |x, y| {
        std::array::from_fn(|c| {
            (level.sqrt() + sigma * gauss(x, y, k + c as u64))
                .max(0.)
                .powi(2)
        })
    })
}

/// The mean of channel 1 and its square-root Laplacian RMS over the pixels of `im`
/// between `r0` and `r1` of `centre`.
fn ring_stats(im: &CameraImage, centre: [f32; 2], r0: f32, r1: f32) -> (f32, f32) {
    let w = im.width as i32;
    let v = |x: i32, y: i32| im.pixels[(y * w + x) as usize][1].max(0.);
    let (mut mean, mut sum, mut n) = (0., 0., 0.);
    for y in 1..im.height as i32 - 1 {
        for x in 1..w - 1 {
            let d = (x as f32 - centre[0]).hypot(y as f32 - centre[1]);
            if d >= r0 && d < r1 {
                let s = |x, y| v(x, y).sqrt();
                let l = s(x, y) - (s(x - 1, y) + s(x + 1, y) + s(x, y - 1) + s(x, y + 1)) / 4.;
                mean += v(x, y);
                sum += l * l;
                n += 1.;
            }
        }
    }
    (mean / n, (sum / n).sqrt())
}

/// At half opacity a fill a little grainier than the photo still needs grain:
/// mixed half and half with the photo, the composite would be smoother.
#[test]
fn grain_accounts_for_opacity_when_the_fill_has_some() {
    let photo = grainy_photo(400, 400, 0.1, 0.01, 50);
    let fill = grainy_photo(400, 400, 0.1, 0.011, 60);
    let at = |x: f32, y: f32| fill.pixels[(y as u32 * 400 + x as u32) as usize];
    let mut op = laid_over(
        &photo,
        [40., 40., 360., 360.],
        (320, 320),
        at,
        (200., 200., 100.),
    );
    op.opacity = 0.5;
    let (_, grain) = ring_stats(
        &lay(&photo, &op, GenerateBlend::Membrane),
        [200., 200.],
        0.,
        40.,
    );
    let (_, want) = ring_stats(&photo, [200., 200.], 0., 40.);
    assert!((0.88..1.12).contains(&(grain / want)), "{}", grain / want);
}

/// A small, fully feathered spot over a fill as grainy as the photo keeps the
/// photo's grain: what renders is measured at each mix, so the fill's own grain
/// is never taken for none, and the spot isn't made grainier than the photo.
#[test]
fn a_small_spot_keeps_the_photo_s_grain() {
    let photo = grainy_photo(200, 200, 0.1, 0.01, 70);
    let fill = grainy_photo(200, 200, 0.1, 0.01, 80);
    let at = |x: f32, y: f32| fill.pixels[(y as u32 * 200 + x as u32) as usize];
    let mut op = laid_over(
        &photo,
        [50., 50., 150., 150.],
        (100, 100),
        at,
        (100., 100., 12.),
    );
    op.shape = RetouchShape::Spot {
        center: [100.5 / 200., 100.5 / 200.],
        radius: 12. / 200.,
    };
    op.feather = 1.;
    let (_, want) = ring_stats(&photo, [100., 100.], 0., 12.);
    let (_, got) = ring_stats(
        &lay(&photo, &op, GenerateBlend::Membrane),
        [100., 100.],
        0.,
        12.,
    );
    eprintln!("small spot grain ratio {:.3}", got / want);
    assert!((0.88..1.12).contains(&(got / want)), "{}", got / want);
}

/// Grain leaves a dark fill's tone as the membrane made it, across the feather too.
#[test]
fn grain_keeps_a_dark_fill_s_tone() {
    let photo = grainy_photo(400, 400, 0.0002, 0.004, 90);
    let flat = |_: f32, _: f32| [0.0002; 3];
    let mut op = laid_over(
        &photo,
        [40., 40., 360., 360.],
        (320, 320),
        flat,
        (200., 200., 100.),
    );
    op.feather = 0.75;
    let centre = [200., 200.];
    let textured = lay(&photo, &op, GenerateBlend::Membrane);
    let smooth = lay(&photo, &op, GenerateBlend::Untextured);
    for (r0, r1) in [(0., 30.), (45., 80.)] {
        let (mean, _) = ring_stats(&textured, centre, r0, r1);
        let (want, _) = ring_stats(&smooth, centre, r0, r1);
        let e = (mean / want).ln().abs();
        assert!(e < 0.03, "{r0}..{r1}: {mean} against {want}");
    }
}

/// A hard edge at half opacity over a fill that matches the photo: the mix is as
/// grainy as the photo already, and takes none.
#[test]
fn a_hard_edge_over_a_matching_fill_takes_no_grain() {
    let clean = grainy_photo(400, 400, 0.1, 0.01, 10);
    let mut photo = clean.clone();
    for y in 0..400 {
        for x in 0..400 {
            if (x as f32 - 200.).hypot(y as f32 - 200.) < 10. {
                photo.pixels[y * 400 + x] = [0.9; 3];
            }
        }
    }
    let at = |x: f32, y: f32| clean.pixels[y as usize * 400 + x as usize];
    let mut op = laid_over(
        &photo,
        [40., 40., 360., 360.],
        (320, 320),
        at,
        (200., 200., 100.),
    );
    op.opacity = 0.5;
    op.feather = 0.;
    let (_, before) = ring_stats(
        &lay(&photo, &op, GenerateBlend::Untextured),
        [200., 200.],
        20.,
        40.,
    );
    let (_, after) = ring_stats(
        &lay(&photo, &op, GenerateBlend::Membrane),
        [200., 200.],
        20.,
        40.,
    );
    assert!(
        (0.88..1.12).contains(&(after / before)),
        "{}",
        after / before
    );
}

/// Edges the fill shares with the photo don't count as shared grain: independent
/// grain over a striped ground, mixed at half opacity, is made up as on a flat one.
#[test]
fn shared_edges_are_not_shared_grain() {
    let ground = |x: f32| {
        if (x as usize / 80).is_multiple_of(2) {
            0.64f32
        } else {
            0.04
        }
    };
    let striped = |k: u64| {
        image(400, 400, move |x, y| {
            std::array::from_fn(|c| (ground(x).sqrt() + 0.01 * gauss(x, y, k + c as u64)).powi(2))
        })
    };
    let (photo, fill) = (striped(20), striped(30));
    let at = |x: f32, y: f32| fill.pixels[y as usize * 400 + x as usize];
    let mut op = laid_over(
        &photo,
        [40., 40., 360., 360.],
        (320, 320),
        at,
        (200., 200., 100.),
    );
    op.opacity = 0.5;
    op.feather = 0.75;
    let (_, want) = ring_stats(&photo, [200., 200.], 0., 20.);
    let (_, got) = ring_stats(
        &lay(&photo, &op, GenerateBlend::Membrane),
        [200., 200.],
        0.,
        20.,
    );
    assert!((0.88..1.12).contains(&(got / want)), "{}", got / want);
}

/// The square-root Laplacian RMS of channel 1 of `im` over `rect`.
fn region_grain(im: &CameraImage, rect: [i32; 4]) -> f32 {
    let w = im.width as i32;
    let v = |x: i32, y: i32| im.pixels[(y * w + x) as usize][1].max(0.).sqrt();
    let (mut sum, mut n) = (0., 0.);
    for y in rect[1]..rect[3] {
        for x in rect[0]..rect[2] {
            let l = v(x, y) - (v(x - 1, y) + v(x + 1, y) + v(x, y - 1) + v(x, y + 1)) / 4.;
            sum += l * l;
            n += 1.;
        }
    }
    (sum / n).sqrt()
}

/// A fill that replaces an object with smooth ground and copies the photo's
/// surroundings, grain and all, elsewhere in a generously painted spot: the
/// copied surroundings take no grain because the smooth part lacks it.
#[test]
fn copied_surroundings_take_no_grain_for_a_smooth_part() {
    let clean = grainy_photo(400, 400, 0.1, 0.01, 50);
    let object = |x: f32, y: f32| (x - 215.).hypot(y - 200.) < 65.;
    let photo = image(400, 400, |x, y| {
        if object(x, y) {
            [0.4; 3]
        } else {
            clean.pixels[(y as u32 * 400 + x as u32) as usize]
        }
    });
    let fill = |x: f32, y: f32| {
        if object(x.floor(), y.floor()) {
            [0.1; 3]
        } else {
            photo.pixels[(y as u32 * 400 + x as u32) as usize]
        }
    };
    let mut op = laid_over(
        &photo,
        [40., 40., 360., 360.],
        (320, 320),
        fill,
        (200., 200., 100.),
    );
    op.shape = RetouchShape::Spot {
        center: [200.5 / 400., 200.5 / 400.],
        radius: 100. / 400.,
    };
    op.feather = 0.;
    let out = lay(&photo, &op, GenerateBlend::Membrane);
    let copied = [112, 185, 127, 215];
    let ratio = region_grain(&out, copied) / region_grain(&clean, copied);
    assert!((0.88..1.12).contains(&ratio), "copied: {ratio}");
    let made = [195, 185, 235, 215];
    let ratio = region_grain(&out, made) / region_grain(&clean, made);
    assert!((0.85..1.15).contains(&ratio), "made up: {ratio}");
}

/// Grain is sized to the brightness that renders, not the fill's: a fill darker
/// or brighter than the photo, laid at half opacity, renders as grainy as it.
#[test]
fn grain_is_sized_to_what_renders() {
    for (original, replacement) in [(0.4f32, 0.1f32), (0.1, 0.4)] {
        let photo = grainy_photo(400, 400, original, 0.01, 50);
        let fill = |x: f32, y: f32| {
            if (x.floor() - 200.).hypot(y.floor() - 200.) < 99. {
                [replacement; 3]
            } else {
                photo.pixels[(y as u32 * 400 + x as u32) as usize]
            }
        };
        let mut op = laid_over(
            &photo,
            [40., 40., 360., 360.],
            (320, 320),
            fill,
            (200., 200., 100.),
        );
        op.shape = RetouchShape::Spot {
            center: [200.5 / 400., 200.5 / 400.],
            radius: 100. / 400.,
        };
        op.feather = 0.;
        op.opacity = 0.5;
        let out = lay(&photo, &op, GenerateBlend::Membrane);
        let rect = [175, 175, 225, 225];
        let ratio = region_grain(&out, rect) / region_grain(&photo, rect);
        assert!(
            (0.88..1.12).contains(&ratio),
            "{original} under {replacement}: {ratio}"
        );
    }
}

/// Where `row` of `im` crosses half way between `lo` and `hi` in channel 1,
/// to a fraction of a pixel, between columns `from` and `to`.
fn crossing(im: &CameraImage, row: usize, from: usize, to: usize, lo: f32, hi: f32) -> f32 {
    let w = im.width as usize;
    let mid = (lo + hi) / 2.;
    for x in from..to {
        let (a, b) = (im.pixels[row * w + x][1], im.pixels[row * w + x + 1][1]);
        if (a - mid) * (b - mid) <= 0. && a != b {
            return x as f32 + (mid - a) / (b - a);
        }
    }
    f32::NAN
}

/// A remover that gives back the region as sent leaves the photo where it
/// was: an edge under the painted area stays put, at full size and when the
/// region is sent scaled down, as a large one is.
#[test]
fn a_region_given_back_as_sent_lands_where_it_was_taken() {
    for (w, h, r) in [(900u32, 600u32, 120f32), (3200, 2000, 700.)] {
        let (cx, cy) = (w as f32 / 2., h as f32 / 2.);
        // A vertical step, slightly off the pixel grid, through the area.
        let edge = cx + 0.3;
        let im = image(
            w,
            h,
            |x, _| if x + 0.5 < edge { [0.05; 3] } else { [0.4; 3] },
        );
        let r_ = plain();
        let rendition = Rendition::of(&im.metadata, &r_).unwrap();
        let mut op = brush(&im, cx, cy, r);
        op.feather = 0.;
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let input =
            rendition::render(&im, &r_, None, &op, &rendition, Cancel::new(&cancel)).unwrap();
        let (rw, rh) = (input.region.width, input.region.height);
        let samples =
            rendition::patch_samples(&input.rendition, &input.rgb, rw, rh, rw as usize * 3)
                .unwrap();
        let laid = generated(&op, &input, &samples);
        let row = cy as usize;
        let from = (cx - r / 2.) as usize;
        let to = (cx + r / 2.) as usize;
        let before = crossing(&im, row, from, to, 0.05, 0.4);
        // As laid, and with all the app lays it with (tone, detail, grain).
        for blend in [GenerateBlend::Composite, GenerateBlend::Membrane] {
            let out = lay(&im, &laid, blend);
            let after = crossing(&out, row, from, to, 0.05, 0.4);
            // And no second edge: the row left of the step stays dark.
            let ghost = (from..(before as usize).saturating_sub(3))
                .map(|x| out.pixels[row * w as usize + x][1])
                .fold(0f32, f32::max);
            assert!(
                (after - before).abs() < 0.6 && ghost < 0.1,
                "{blend:?} {w}x{h}, region {rw}x{rh} at scale {}: the edge moved from \
                 {before} to {after}, darkest side reaches {ghost}",
                input.region.scale
            );
        }
    }
}
