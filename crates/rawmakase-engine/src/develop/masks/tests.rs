use super::local::slot;
use super::*;
use crate::model::masks::{BrushStroke, LocalAdjust, MaskComponent, MaskGroup, MaskOp, MaskShape};
use crate::model::recipe::Recipe;
use crate::{camera_data::CameraImage, develop::Geometry};
use std::sync::Arc;

fn image(width: u32, height: u32) -> CameraImage {
    CameraImage {
        width,
        height,
        pixels: (0..width * height)
            .map(|i| {
                let (x, y) = ((i % width) as f32, (i / width) as f32);
                let v = 0.05 + 0.4 * x / width as f32 + 0.05 * (y * 0.2).sin();
                [v * 1.1, v, v * 0.8]
            })
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
fn group(components: Vec<MaskComponent>) -> MaskGroup {
    MaskGroup {
        components,
        adjust: LocalAdjust {
            exposure: 1.,
            ..Default::default()
        },
        ..Default::default()
    }
}
/// Weights of one mask over the whole `w` × `h` image, row by row.
fn weights(im: &CameraImage, m: MaskGroup, range: Option<&crate::rendered::Rendered>) -> Vec<f32> {
    let r = Recipe::default();
    let g = Geometry::new(im, &r, 0);
    let masks = [m];
    let weigher = Weigher::new(im, &masks, Selection::One(0));
    let w = weigher.weights(im, &r, &g, [0, 0, g.width, g.height], range);
    w.data.iter().map(|v| *v as f32 / 255.).collect()
}
#[test]
fn linear_gradient_fades_from_start_to_end() {
    let im = image(200, 100);
    let w = weights(
        &im,
        group(vec![MaskComponent::new(MaskShape::Linear {
            from: [0.25, 0.5],
            to: [0.75, 0.5],
        })]),
        None,
    );
    let at = |x: usize| w[50 * 200 + x];
    assert_eq!(at(10), 1.);
    assert_eq!(at(190), 0.);
    assert!((at(100) - 0.5).abs() < 0.02, "{}", at(100));
    // Smooth, monotone transition.
    assert!((50..150).all(|x| at(x + 1) <= at(x)));
    assert!(at(55) > 0.98 && at(145) < 0.02);
}
#[test]
fn radial_gradient_feathers_within_its_edge_and_rotates() {
    let im = image(200, 200);
    let radial = |feather: f32, angle: f32| {
        group(vec![MaskComponent::new(MaskShape::Radial {
            center: [0.5, 0.5],
            radii: [0.3, 0.1],
            angle,
            feather,
        })])
    };
    let w = weights(&im, radial(0.5, 0.), None);
    let at = |w: &[f32], x: usize, y: usize| w[y * 200 + x];
    assert_eq!(at(&w, 100, 100), 1.);
    // Half the radius is solid, the outer half fades.
    assert_eq!(at(&w, 125, 100), 1.);
    assert!(at(&w, 145, 100) > 0. && at(&w, 145, 100) < 1.);
    assert_eq!(at(&w, 165, 100), 0.);
    assert_eq!(at(&w, 100, 125), 0.);
    let hard = weights(&im, radial(0., 0.), None);
    assert_eq!(at(&hard, 155, 100), 1.);
    assert_eq!(at(&hard, 162, 100), 0.);
    // Turned by 90°, the long axis is vertical.
    let turned = weights(&im, radial(0., 90.), None);
    assert_eq!(at(&turned, 100, 150), 1.);
    assert_eq!(at(&turned, 150, 100), 0.);
}
#[test]
fn components_combine_and_invert() {
    let im = image(100, 100);
    let left = || {
        MaskComponent::new(MaskShape::Linear {
            from: [0.4, 0.5],
            to: [0.41, 0.5],
        })
    };
    let top = || {
        MaskComponent::new(MaskShape::Linear {
            from: [0.5, 0.4],
            to: [0.5, 0.41],
        })
    };
    let at = |w: &[f32], x: usize, y: usize| w[y * 100 + x];
    let with = |op: MaskOp| MaskComponent { op, ..top() };
    let add = weights(&im, group(vec![left(), with(MaskOp::Add)]), None);
    let sub = weights(&im, group(vec![left(), with(MaskOp::Subtract)]), None);
    let int = weights(&im, group(vec![left(), with(MaskOp::Intersect)]), None);
    // Quadrants: (left, top) = (20, 20); left only = (20, 80); top only = (80, 20).
    assert_eq!(
        [
            at(&add, 20, 20),
            at(&add, 20, 80),
            at(&add, 80, 20),
            at(&add, 80, 80)
        ],
        [1., 1., 1., 0.]
    );
    assert_eq!(
        [at(&sub, 20, 20), at(&sub, 20, 80), at(&sub, 80, 20)],
        [0., 1., 0.]
    );
    assert_eq!(
        [at(&int, 20, 20), at(&int, 20, 80), at(&int, 80, 20)],
        [1., 0., 0.]
    );
    let mut inverted = group(vec![MaskComponent {
        invert: true,
        opacity: 0.5,
        ..left()
    }]);
    let w = weights(&im, inverted.clone(), None);
    assert!((at(&w, 80, 50) - 0.5).abs() < 0.01 && at(&w, 20, 50) == 0.);
    inverted.invert = true;
    let w = weights(&im, inverted, None);
    assert!((at(&w, 80, 50) - 0.5).abs() < 0.01 && at(&w, 20, 50) == 1.);
}
#[test]
fn brush_strokes_build_up_to_density_and_erase() {
    let im = image(200, 100);
    let stroke = |points: Vec<[f32; 2]>, flow: f32, density: f32, erase: bool| BrushStroke {
        points: points.into(),
        radius: 0.05,
        feather: 0.,
        flow,
        density,
        erase,
        auto_mask: false,
    };
    let line = vec![[0.2, 0.5], [0.8, 0.5]];
    let brush = |strokes| group(vec![MaskComponent::new(MaskShape::Brush { strokes })]);
    let at = |w: &[f32], x: usize, y: usize| w[y * 200 + x];
    let full = weights(&im, brush(vec![stroke(line.clone(), 1., 1., false)]), None);
    assert!(at(&full, 100, 50) > 0.99 && at(&full, 100, 80) == 0.);
    let half = weights(&im, brush(vec![stroke(line.clone(), 0.5, 1., false)]), None);
    assert!((at(&half, 100, 50) - 0.5).abs() < 0.02);
    let twice = weights(
        &im,
        brush(vec![
            stroke(line.clone(), 0.5, 1., false),
            stroke(line.clone(), 0.5, 1., false),
        ]),
        None,
    );
    assert!((at(&twice, 100, 50) - 0.75).abs() < 0.02);
    let capped = weights(
        &im,
        brush(vec![
            stroke(line.clone(), 0.5, 0.6, false),
            stroke(line.clone(), 0.5, 0.6, false),
        ]),
        None,
    );
    assert!((at(&capped, 100, 50) - 0.6).abs() < 0.02);
    let erased = weights(
        &im,
        brush(vec![
            stroke(line, 1., 1., false),
            stroke(vec![[0.5, 0.2], [0.5, 0.8]], 1., 1., true),
        ]),
        None,
    );
    assert!(at(&erased, 100, 50) < 0.01 && at(&erased, 60, 50) > 0.99);
}
#[test]
fn auto_mask_stops_at_edges() {
    // Dark left half, bright right half; a stroke along the middle with Auto Mask.
    let mut im = image(200, 100);
    for (i, p) in im.pixels.iter_mut().enumerate() {
        *p = if i % 200 < 100 { [0.05; 3] } else { [0.6; 3] };
    }
    let stroke = BrushStroke {
        points: vec![[0.4, 0.5], [0.45, 0.5]].into(),
        radius: 0.1,
        feather: 0.,
        flow: 1.,
        density: 1.,
        erase: false,
        auto_mask: true,
    };
    let w = weights(
        &im,
        group(vec![MaskComponent::new(MaskShape::Brush {
            strokes: vec![stroke],
        })]),
        None,
    );
    assert!(w[50 * 200 + 90] > 0.99);
    assert!(w[50 * 200 + 110] < 0.01);
}
/// Weights of `m` over the whole image, with brush rasters from `cache`.
fn cached_weights(im: &Arc<CameraImage>, m: &MaskGroup, cache: &mut RasterCache) -> Vec<f32> {
    let r = Recipe::default();
    let g = Geometry::new(im, &r, 0);
    let masks = [m.clone()];
    let weigher = Weigher::cached(im, &masks, Selection::One(0), Some(cache));
    let w = weigher.weights(im, &r, &g, [0, 0, g.width, g.height], None);
    w.data.iter().map(|v| *v as f32 / 255.).collect()
}
fn brush(auto_mask: bool) -> MaskGroup {
    group(vec![MaskComponent::new(MaskShape::Brush {
        strokes: vec![BrushStroke {
            points: vec![[0.3, 0.5], [0.7, 0.5]].into(),
            radius: 0.1,
            feather: 0.5,
            flow: 1.,
            density: 1.,
            erase: false,
            auto_mask,
        }],
    })])
}
#[test]
fn cached_brushes_follow_the_photos_proportions() {
    let mut cache = RasterCache::default();
    let m = brush(false);
    let wide = Arc::new(image(200, 100));
    let tall = Arc::new(image(100, 200));
    let _ = cached_weights(&wide, &m, &mut cache);
    // The same strokes pasted onto a portrait photo.
    assert_eq!(
        cached_weights(&tall, &m, &mut cache),
        weights(&tall, m.clone(), None)
    );
}
/// Dark left of `edge` (a fraction of the width), bright right of it.
fn split(width: u32, height: u32, edge: f32) -> CameraImage {
    let mut im = image(width, height);
    for (i, p) in im.pixels.iter_mut().enumerate() {
        let x = (i as u32 % width) as f32 / width as f32;
        *p = if x < edge { [0.05; 3] } else { [0.6; 3] };
    }
    im
}
#[test]
fn cached_auto_masks_follow_retouched_images() {
    let mut cache = RasterCache::default();
    let m = brush(true);
    let before = Arc::new(split(200, 100, 0.5));
    let first = cached_weights(&before, &m, &mut cache);
    assert_eq!(first, weights(&before, m.clone(), None));
    // A retouch replaces the image, which may reuse the old one's memory.
    drop(before);
    let after = Arc::new(split(200, 100, 0.6));
    let second = cached_weights(&after, &m, &mut cache);
    assert_eq!(second, weights(&after, m.clone(), None));
    assert_ne!(first, second);
}
#[test]
fn cached_auto_masks_follow_the_pyramid_level() {
    let mut cache = RasterCache::default();
    let m = brush(true);
    let fit = Arc::new(split(100, 50, 0.55));
    let full = Arc::new(split(200, 100, 0.5));
    for level in [&fit, &full, &fit] {
        assert_eq!(
            cached_weights(level, &m, &mut cache),
            weights(level, m.clone(), None)
        );
    }
    // Back at Fit, its raster came from the cache.
    assert_eq!(cache.len(), 2);
}
#[test]
fn ranges_use_the_developed_colors() {
    let im = image(100, 10);
    let developed = crate::rendered::Rendered {
        width: 100,
        height: 10,
        pixels: (0..1000)
            .map(|i| {
                let x = (i % 100) as f32 / 100.;
                if x < 0.5 { [x, x, x] } else { [0.9, 0.2, 0.2] }
            })
            .collect(),
    };
    let lum = weights(
        &im,
        group(vec![MaskComponent::new(MaskShape::LuminanceRange {
            low: 0.,
            high: 0.4,
            falloff: [0., 0.],
        })]),
        Some(&developed),
    );
    assert_eq!(lum[5 * 100 + 5], 1.);
    assert_eq!(lum[5 * 100 + 45], 0.);
    let red = super::range::oklab([0.9, 0.2, 0.2]);
    let color = weights(
        &im,
        group(vec![MaskComponent::new(MaskShape::ColorRange {
            samples: vec![red],
            amount: 0.5,
        })]),
        Some(&developed),
    );
    assert_eq!(color[5 * 100 + 80], 1.);
    assert_eq!(color[5 * 100 + 20], 0.);
}
/// A mask covering the whole photo renders like the same global change where Camera Raw
/// renders them alike: Shadows, Highlights, Dehaze, Clarity and Contrast. (A mask's
/// Exposure keeps the photo's white point, and its Whites and Blacks are curves of
/// their own: `exposure_of_a_mask_scales_the_scene_as_the_global_slider_does` and
/// `scene_tone::global`.)
#[test]
fn full_frame_mask_equals_the_global_slider() {
    let im = image(120, 80);
    let everywhere = || {
        MaskComponent::new(MaskShape::Linear {
            from: [0., -3.],
            to: [0., -2.9],
        })
    };
    let everywhere = MaskComponent {
        invert: true,
        ..everywhere()
    };
    let base = Recipe::with_profiles(&im.metadata, &[]);
    for (local, global) in [
        (
            LocalAdjust {
                shadows: 0.5,
                highlights: -0.4,
                ..Default::default()
            },
            Recipe {
                shadows: 0.5,
                highlights: -0.4,
                ..base.clone()
            },
        ),
        (
            LocalAdjust {
                dehaze: 0.4,
                clarity: 0.3,
                ..Default::default()
            },
            Recipe {
                effects: crate::model::effects::Effects {
                    dehaze: 0.4,
                    clarity: 0.3,
                    ..base.effects.clone()
                },
                ..base.clone()
            },
        ),
        (
            LocalAdjust {
                contrast: 0.45,
                ..Default::default()
            },
            Recipe {
                contrast: 0.45,
                ..base.clone()
            },
        ),
    ] {
        let masked = Recipe {
            masks: vec![MaskGroup {
                components: vec![everywhere.clone()],
                adjust: local,
                ..Default::default()
            }],
            ..base.clone()
        };
        let a = crate::develop::render(&im, &masked.checked().unwrap(), 0).unwrap();
        let b = crate::develop::render(&im, &global.checked().unwrap(), 0).unwrap();
        let max = a
            .pixels
            .iter()
            .flatten()
            .zip(b.pixels.iter().flatten())
            .map(|(x, y)| (x - y).abs())
            .fold(0., f32::max);
        // Local contrast skips the global curve's monotone clean-up of measurement
        // noise, so it differs slightly; the rest is exact.
        assert!(max < 3e-3, "{local:?}: {max}");
    }
}
/// The scene a full-frame mask's Exposure leaves the scene tone stage is the global
/// Exposure's: only the white point stays the photo's, as in Camera Raw.
#[test]
fn exposure_of_a_mask_scales_the_scene_as_the_global_slider_does() {
    let im = image(120, 80);
    let base = Recipe::with_profiles(&im.metadata, &[]);
    let everywhere = MaskComponent {
        invert: true,
        ..MaskComponent::new(MaskShape::Linear {
            from: [0., -3.],
            to: [0., -2.9],
        })
    };
    let masked = Recipe {
        masks: vec![MaskGroup {
            components: vec![everywhere],
            adjust: LocalAdjust {
                exposure: 0.8,
                ..Default::default()
            },
            ..Default::default()
        }],
        ..base.clone()
    };
    let global = Recipe {
        exposure: 0.8,
        ..base
    };
    let scene = |r: &Recipe| {
        crate::develop::quality::render_stage(
            &im,
            r,
            crate::develop::quality::Stage::SceneInput,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
    };
    let (a, b) = (scene(&masked), scene(&global));
    let max = a
        .pixels
        .iter()
        .flatten()
        .zip(b.pixels.iter().flatten())
        .map(|(x, y)| (x - y).abs() / y.abs().max(1e-3))
        .fold(0., f32::max);
    assert!(max < 1e-4, "{max}");
}
#[test]
fn partial_masks_blend_and_amount_scales() {
    let im = image(120, 80);
    let base = Recipe::with_profiles(&im.metadata, &[]);
    let left_half = MaskComponent::new(MaskShape::Linear {
        from: [0.49, 0.5],
        to: [0.51, 0.5],
    });
    let mut r = base.clone();
    r.masks.push(MaskGroup {
        components: vec![left_half],
        adjust: LocalAdjust {
            exposure: 1.,
            ..Default::default()
        },
        amount: 0.5,
        ..Default::default()
    });
    // Compared where Exposure acts: the scene tone stage's input.
    let scene = |r: &Recipe| {
        crate::develop::quality::render_stage(
            &im,
            r,
            crate::develop::quality::Stage::SceneInput,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
    };
    let masked = scene(&r);
    let plain = scene(&base);
    let half = scene(&Recipe {
        exposure: 0.5,
        ..base
    });
    let at = |im: &crate::rendered::Rendered, x: usize| im.pixels[40 * 120 + x][1];
    assert!((at(&masked, 20) / at(&half, 20) - 1.).abs() < 1e-4);
    assert!((at(&masked, 100) / at(&plain, 100) - 1.).abs() < 1e-4);
    let _ = slot::EXPOSURE;
}

fn register(width: u32, height: u32, data: Vec<u8>) -> (String, MaskShape) {
    use rawmakase_model::storage::{bitmaps::Bitmap, mask_assets};
    let id = mask_assets::register(Bitmap {
        width,
        height,
        channels: 1,
        depth: 1,
        data,
    })
    .unwrap();
    let shape = MaskShape::Bitmap(crate::model::masks::BitmapMask {
        id: id.clone(),
        width,
        height,
        sampling: crate::model::masks::BITMAP_SAMPLING,
        source: None,
    });
    (id, shape)
}
#[test]
fn a_bitmap_component_samples_its_raster_over_the_image_frame() {
    let im = image(200, 100);
    // Left half unselected, right half selected, a soft step between the centres.
    let (_, shape) = register(2, 1, vec![0, 255]);
    let w = weights(&im, group(vec![MaskComponent::new(shape.clone())]), None);
    let at = |x: usize, y: usize| w[y * 200 + x];
    assert!(
        at(10, 50) < 0.01 && at(190, 50) > 0.99,
        "{} {}",
        at(10, 50),
        at(190, 50)
    );
    // Pixel centres of the raster sit at a quarter and three quarters of the width.
    assert!((at(100, 50) - 0.5).abs() < 0.02);
    assert!((at(50, 20) - 0.).abs() < 0.01 && (at(150, 20) - 1.).abs() < 0.01);
    // Inverted as a component, it is the complement inside the frame.
    let mut inverted = MaskComponent::new(shape.clone());
    inverted.invert = true;
    let v = weights(&im, group(vec![inverted]), None);
    for (a, b) in w.iter().zip(&v) {
        assert!((a + b - 1.).abs() < 0.01, "{a} {b}");
    }
    // Opacity and Subtract compose like any component.
    let mut half = MaskComponent::new(shape.clone());
    half.opacity = 0.5;
    let h = weights(&im, group(vec![half]), None);
    assert!((h[50 * 200 + 190] - 0.5).abs() < 0.01);
    let mut cut = MaskComponent::new(shape);
    cut.op = MaskOp::Subtract;
    let none = weights(
        &im,
        group(vec![
            MaskComponent::new(MaskShape::Radial {
                center: [0.5, 0.5],
                radii: [4., 4.],
                angle: 0.,
                feather: 0.,
            }),
            cut,
        ]),
        None,
    );
    assert!(none[50 * 200 + 190] < 0.01 && none[50 * 200 + 10] > 0.99);
}
#[test]
fn a_bitmap_contributes_nothing_outside_the_frame_or_without_its_raster() {
    let (_, shape) = register(2, 2, vec![255; 4]);
    let mut inverted = MaskComponent::new(shape);
    inverted.invert = true;
    // Inverted full coverage is empty everywhere in the frame.
    let im = image(60, 40);
    let w = weights(&im, group(vec![inverted]), None);
    assert!(w.iter().all(|v| *v < 0.01));
    // A raster nothing can provide contributes zero, even inverted.
    let missing = MaskShape::Bitmap(crate::model::masks::BitmapMask {
        id: format!("sha256:{}", "ab".repeat(32)),
        width: 2,
        height: 2,
        sampling: crate::model::masks::BITMAP_SAMPLING,
        source: None,
    });
    let mut c = MaskComponent::new(missing);
    c.invert = true;
    let w = weights(&im, group(vec![c]), None);
    assert!(w.iter().all(|v| *v == 0.));
}
#[test]
fn rendering_a_mask_whose_raster_is_missing_is_an_error_not_an_empty_mask() {
    let im = image(60, 40);
    let missing = MaskShape::Bitmap(crate::model::masks::BitmapMask {
        id: format!("sha256:{}", "ef".repeat(32)),
        width: 2,
        height: 2,
        sampling: crate::model::masks::BITMAP_SAMPLING,
        source: None,
    });
    let mut r = Recipe {
        masks: vec![group(vec![MaskComponent::new(missing)])],
        ..Default::default()
    };
    let error = crate::develop::render(&im, &r.checked().unwrap(), 60)
        .err()
        .unwrap();
    assert!(error.to_string().contains("missing"), "{error}");
    // A stored raster of another size than the mask describes is as bad as a missing one.
    let (_, other) = register(3, 2, vec![255; 6]);
    let MaskShape::Bitmap(mut described) = other else {
        unreachable!()
    };
    described.width = 2;
    described.height = 3;
    r.masks[0].components[0].shape = MaskShape::Bitmap(described);
    let error = crate::develop::render(&im, &r.checked().unwrap(), 60)
        .err()
        .unwrap();
    assert!(error.to_string().contains("damaged"), "{error}");
    // Hidden, the mask is not rendered and needs nothing.
    r.masks[0].hidden = true;
    assert!(crate::develop::render(&im, &r.checked().unwrap(), 60).is_ok());
    // And its raster, once provided, renders.
    let (_, shape) = register(2, 2, vec![255; 4]);
    r.masks[0].hidden = false;
    r.masks[0].components[0].shape = shape;
    assert!(crate::develop::render(&im, &r.checked().unwrap(), 60).is_ok());
}

#[test]
fn the_selection_input_keeps_switched_off_spots_off() {
    use crate::model::panels::{Panel, PanelState};
    let mut user = Recipe::default();
    user.panels.set(Panel::SpotRemoval, PanelState::Off);
    let input = selection_input_recipe(&user);
    assert_eq!(input.panels.state(Panel::SpotRemoval), PanelState::Off);
    assert_eq!(
        input.panels.state(Panel::RedEye),
        user.panels.state(Panel::RedEye)
    );
}
