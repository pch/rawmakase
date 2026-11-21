//! Editor tests of `live_brush`: a stroke drawn as it is painted matches what
//! its render shows, and goes when the render, an Undo or another photo does.
use crate::app::testing::*;
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use crate::model::masks::MaskShape;
use eframe::egui::{Color32, Pos2, Rect};
use std::sync::Arc;

const VIEW: f32 = 240.;

fn image() -> Arc<CameraImage> {
    Arc::new(CameraImage {
        recovered: Default::default(),
        width: 600,
        height: 400,
        pixels: vec![[0.2; 3]; 240_000].into(),
        metadata: Metadata {
            width: 600,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    })
}

/// An editor showing a 3:2 photo with a brush mask selected and its overlay on.
fn editor(ctx: &egui::Context) -> Editor {
    let mut editor = Editor::with_context(
        ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.document.set_image(image());
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([300, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    frame(ctx, &mut editor, vec![]);
    in_edit_frame(ctx, &mut editor, |editor| {
        editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Brush, None)
    });
    editor
}

fn frame(ctx: &egui::Context, editor: &mut Editor, events: Vec<egui::Event>) {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(VIEW))),
            events,
            ..Default::default()
        },
        |ui| {
            let frame = editor.begin_edit_frame();
            editor.viewport_ui(ui);
            editor.finish_edit_frame(frame, ctx);
        },
    );
    output.textures_delta.clear();
}

fn button(pos: Pos2, pressed: bool, alt: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: if alt {
            egui::Modifiers::ALT
        } else {
            egui::Modifiers::NONE
        },
    }
}

/// Paints from `a` to `b` in eight steps; `release` ends the drag.
fn paint(ctx: &egui::Context, editor: &mut Editor, a: Pos2, b: Pos2, release: bool) {
    frame(
        ctx,
        editor,
        vec![egui::Event::PointerMoved(a), button(a, true, false)],
    );
    for k in 1..=8 {
        frame(
            ctx,
            editor,
            vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 8.)],
        );
    }
    if release {
        frame(ctx, editor, vec![button(b, false, false)]);
    }
}

/// How many strokes are drawn as painted: the one being painted and those
/// waiting for their render.
fn drawn(editor: &Editor) -> usize {
    usize::from(editor.view.masking.live().is_some()) + editor.view.masking.painted()
}

/// The mask's committed weights at the live buffer's pixels: a render of the
/// whole photo at the buffer's size, so their pixels fall on the same places.
fn committed(editor: &Editor, recipe: &crate::model::recipe::Recipe, size: [usize; 2]) -> Vec<f32> {
    let out = crate::rendered::Rendered {
        width: size[0] as u32,
        height: size[1] as u32,
        pixels: vec![[0.2; 3]; size[0] * size[1]],
    };
    let image = editor.document.full().unwrap();
    crate::develop::masks::overlay_weights(image, recipe, 0, &out, None).unwrap()
}

/// The largest and the mean difference between the buffer's coverage and
/// `want`, the mean over the pixels either covers.
fn differences(editor: &Editor, want: impl Fn(usize, usize) -> f32) -> (f32, f32) {
    let live = editor
        .view
        .masking
        .live()
        .expect("the stroke drawn as painted");
    let [w, h] = live.size();
    let (mut worst, mut sum, mut n) = (0f32, 0., 0);
    for y in 0..h {
        for x in 0..w {
            let (got, want) = (live.coverage_at(x, y), want(x, y));
            worst = worst.max((got - want).abs());
            if got > 0. || want > 0. {
                sum += (got - want).abs();
                n += 1;
            }
        }
    }
    (worst, sum / n.max(1) as f32)
}

/// How near the render's coverage the buffer's is. The buffer is the stroke's
/// coverage at each screen pixel; the render interpolates a raster of it with
/// a pixel every quarter radius (its own test pins the two together at the
/// raster's pixels). Where the feather is narrower than that pixel, as at 20%,
/// the two differ by up to a fifth on the edge; over the stroke, by under 3%.
fn assert_near((worst, mean): (f32, f32), what: &str) {
    assert!(
        worst < 0.25 && mean < 0.03,
        "{what}: worst {worst}, mean {mean}"
    );
}

/// The coverage drawn while painting is the coverage the render of the stroke
/// shows, at the same pixels, for both brushes' settings.
#[test]
fn a_stroke_drawn_as_painted_matches_its_render() {
    for (brush, feather, flow, density) in [(0, 0.5, 1., 1.), (1, 0.2, 0.5, 0.6)] {
        let ctx = egui::Context::default();
        let mut editor = editor(&ctx);
        let b = &mut editor.view.masking.brushes[brush];
        (b.size, b.feather, b.flow, b.density) = (0.06, feather, flow, density);
        editor.view.masking.brush = brush;
        paint(
            &ctx,
            &mut editor,
            Pos2::new(40., 110.),
            Pos2::new(200., 130.),
            false,
        );
        let live = editor.view.masking.live().expect("drawn while painting");
        // The whole photo is in view: the buffer is its rect, a pixel a point.
        let size = live.size();
        assert!(size[0] > 200 && size[0] <= VIEW as usize, "{size:?}");
        let want = committed(&editor, editor.document.edit.recipe(), size);
        assert_near(
            differences(&editor, |x, y| want[y * size[0] + x]),
            &format!("brush {brush}"),
        );
    }
}

/// An erasing stroke draws what it takes away of the mask under it, as its
/// render does.
#[test]
fn an_erasing_stroke_drawn_as_painted_matches_its_render() {
    let ctx = egui::Context::default();
    let mut editor = editor(&ctx);
    editor.view.masking.brushes[0].size = 0.2;
    editor.view.masking.brushes[0].feather = 0.;
    paint(
        &ctx,
        &mut editor,
        Pos2::new(40., 120.),
        Pos2::new(200., 120.),
        true,
    );
    let under = editor.document.edit.recipe().clone();
    editor.view.masking.render_shown(&under);
    assert_eq!(drawn(&editor), 0);
    let e = &mut editor.view.masking.brushes[2];
    (e.size, e.feather, e.flow) = (0.05, 0.5, 0.8);
    editor.view.masking.brush = 2;
    paint(
        &ctx,
        &mut editor,
        Pos2::new(60., 115.),
        Pos2::new(190., 125.),
        false,
    );
    let size = editor
        .view
        .masking
        .live()
        .expect("drawn while erasing")
        .size();
    let before = committed(&editor, &under, size);
    let after = committed(&editor, editor.document.edit.recipe(), size);
    let differences = differences(&editor, |x, y| {
        let at = y * size[0] + x;
        // What was taken away, as a share of what was there, all of it here.
        if before[at] > 0.99 {
            1. - after[at] / before[at]
        } else {
            editor.view.masking.live().unwrap().coverage_at(x, y)
        }
    });
    assert_near(differences, "erase");
}

/// The stroke drawn as painted stays after the drag until a render with all of
/// it shows; an Undo or another photo takes it away at once; nothing of it goes
/// into the edit or History.
#[test]
fn the_stroke_drawn_as_painted_goes_with_its_render() {
    let ctx = egui::Context::default();
    let mut editor = editor(&ctx);
    paint(
        &ctx,
        &mut editor,
        Pos2::new(40., 110.),
        Pos2::new(200., 130.),
        false,
    );
    // A render of the stroke so far, while it is still painted, doesn't end it.
    let partial = editor.document.edit.recipe().clone();
    frame(
        &ctx,
        &mut editor,
        vec![egui::Event::PointerMoved(Pos2::new(210., 140.))],
    );
    editor.view.masking.render_shown(&partial);
    frame(
        &ctx,
        &mut editor,
        vec![button(Pos2::new(210., 140.), false, false)],
    );
    assert!(editor.view.masking.live().is_none());
    assert_eq!(drawn(&editor), 1, "ended, but not rendered");
    let steps = editor.document.edit.history().steps().0.len();
    let recipe = editor.document.edit.recipe().clone();
    frame(&ctx, &mut editor, vec![]);
    assert_eq!(editor.document.edit.recipe(), &recipe);
    assert_eq!(editor.document.edit.history().steps().0.len(), steps);
    editor.view.masking.render_shown(&partial);
    assert_eq!(drawn(&editor), 1, "an older render");
    editor.view.masking.render_shown(&recipe);
    assert_eq!(drawn(&editor), 0);

    // Undo before the render: the stroke is gone from the edit, and from the view.
    paint(
        &ctx,
        &mut editor,
        Pos2::new(40., 150.),
        Pos2::new(200., 150.),
        true,
    );
    assert_eq!(drawn(&editor), 1);
    in_edit_frame(&ctx, &mut editor, |editor| editor.undo());
    frame(&ctx, &mut editor, vec![]);
    assert_eq!(drawn(&editor), 0);

    // Another photo, or closing Masking, before the render.
    // Each stroke away from the others' pins, which a drag would move.
    for (leave, y) in [
        (Editor::close_masking as fn(&mut Editor), 60.),
        (|e: &mut Editor| e.view.masking.clear_document(), 185.),
    ] {
        in_edit_frame(&ctx, &mut editor, |editor| {
            editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Brush, None)
        });
        paint(
            &ctx,
            &mut editor,
            Pos2::new(40., y),
            Pos2::new(200., y),
            true,
        );
        assert_eq!(drawn(&editor), 1);
        leave(&mut editor);
        assert_eq!(drawn(&editor), 0);
    }
}

/// Zoomed in, the buffer covers only the part of the photo in view; a stroke
/// whose mask isn't shown as an overlay isn't drawn, nor is an Auto Mask one.
#[test]
fn the_stroke_is_drawn_over_the_view_only_while_its_overlay_shows() {
    let ctx = egui::Context::default();
    let mut editor = editor(&ctx);
    editor.view.zoom.on = true;
    editor.view.zoom.level = 4.;
    frame(&ctx, &mut editor, vec![]);
    paint(
        &ctx,
        &mut editor,
        Pos2::new(60., 110.),
        Pos2::new(180., 120.),
        false,
    );
    let size = editor.view.masking.live().expect("drawn zoomed in").size();
    assert!(
        size[0] <= VIEW as usize && size[1] <= VIEW as usize,
        "{size:?}"
    );
    frame(
        &ctx,
        &mut editor,
        vec![button(Pos2::new(180., 120.), false, false)],
    );

    let mut editor = self::editor(&ctx);
    editor.view.masking.brushes[0].auto_mask = true;
    paint(
        &ctx,
        &mut editor,
        Pos2::new(40., 110.),
        Pos2::new(200., 130.),
        false,
    );
    assert!(
        editor.view.masking.live().is_none(),
        "Auto Mask shows with its render"
    );
    let strokes = match &editor.document.edit.recipe().masks[0].components[0].shape {
        MaskShape::Brush { strokes } => strokes.len(),
        _ => 0,
    };
    assert_eq!(strokes, 1, "the stroke is painted into the edit as before");
}

/// After the first upload, each step of the drag uploads only the rectangle
/// its segment reached, not the whole buffer.
#[test]
fn each_step_uploads_only_what_it_reached() {
    let ctx = egui::Context::default();
    let mut editor = editor(&ctx);
    let (a, b) = (Pos2::new(40., 110.), Pos2::new(200., 130.));
    frame(
        &ctx,
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true, false)],
    );
    let mut partial = 0;
    for k in 1..=8 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(VIEW))),
                events: vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 8.)],
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                editor.viewport_ui(ui);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        let full = editor.view.masking.live().map(|l| l.size());
        for deltas in output.textures_delta.set.values() {
            for delta in deltas {
                if let (Some(_), Some(full)) = (delta.pos, full) {
                    let [w, h] = delta.image.size();
                    assert!(w * h < full[0] * full[1], "{w}x{h} of {full:?}");
                    partial += 1;
                }
            }
        }
        output.textures_delta.clear();
    }
    assert!(partial >= 6, "{partial} partial uploads");
}

/// What the buffer costs a step of the drag: a 2048 × 1280 view (a large
/// window on a Retina display), a brush a tenth of the photo wide, steps of a
/// fifth of its size. Run with `--ignored --nocapture`.
#[test]
#[ignore = "a measurement"]
fn measure_a_drag_step() {
    use crate::app::develop::tools::live_brush::LiveBrush;
    use crate::develop::masks::Space;
    use crate::model::masks::BrushStroke;
    let ctx = egui::Context::default();
    let visible = Rect::from_min_size(Pos2::ZERO, Vec2::new(2048., 1280.));
    let space = Space::new(1.6);
    let to_image = |p: Pos2| [p.x / 2048., p.y / 1280.];
    let to_screen = |p: [f32; 2]| Pos2::new(p[0] * 2048., p[1] * 1280.);
    let radius = |_: [f32; 2], r: f32| r * 2048.;
    for size in [0.05, 0.4] {
        let mut points = vec![[0.1, 0.5]];
        let stroke = BrushStroke {
            points: points.clone().into(),
            radius: size,
            feather: 0.5,
            flow: 1.,
            density: 1.,
            erase: false,
            auto_mask: false,
        };
        let mut live = LiveBrush::new((0, 0, 0), &stroke, space);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let t = std::time::Instant::now();
            live.update(
                ui.ctx(),
                visible,
                &to_image,
                &to_screen,
                &radius,
                Color32::RED,
            );
            eprintln!(
                "first step, laying out: {:.2} ms",
                t.elapsed().as_secs_f64() * 1e3
            );
            let mut times = Vec::new();
            for _ in 0..200 {
                let last = *points.last().unwrap();
                points.push([last[0] + size * 0.2 / 1.6 * 0.9, last[1]]);
                live.follow(&points);
                let t = std::time::Instant::now();
                live.update(
                    ui.ctx(),
                    visible,
                    &to_image,
                    &to_screen,
                    &radius,
                    Color32::RED,
                );
                times.push(t.elapsed().as_secs_f64() * 1e3);
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "brush {size}: a step: median {:.2} ms, 95th percentile {:.2} ms, worst {:.2} ms",
                times[100], times[190], times[199]
            );
        });
        output.textures_delta.clear();
    }
}

/// The stroke as it is painted, and an erasing one, over a photo, as the
/// viewport draws them at 2×.
#[test]
fn shots_of_a_stroke_being_painted() {
    for (name, erase) in [("live-brush-paint", false), ("live-brush-erase", true)] {
        crate::app::ui::shots::shot(
            name,
            Vec2::new(480., 340.),
            |editor, ctx| {
                editor.document.set_image(image());
                let photo = egui::ColorImage::from_rgb(
                    [300, 200],
                    &(0..300 * 200)
                        .flat_map(|i| {
                            let (x, y) = (i % 300, i / 300);
                            [(60 + x / 3) as u8, (90 + y / 4) as u8, 120]
                        })
                        .collect::<Vec<u8>>(),
                );
                editor.preview.texture = Some(
                    ctx.load_texture("photo", photo, egui::TextureOptions::LINEAR)
                        .into(),
                );
                in_edit_frame(ctx, editor, |editor| {
                    editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Brush, None)
                });
                editor.view.masking.brushes[0].size = 0.06;
                editor.view.masking.brushes[2].size = 0.04;
                let path: Vec<[f32; 2]> = (0..=40)
                    .map(|i| {
                        let t = i as f32 / 40.;
                        [0.15 + 0.7 * t, 0.5 + 0.2 * (t * 6.).sin()]
                    })
                    .collect();
                in_edit_frame(ctx, editor, |editor| {
                    if erase {
                        editor.paint_for_tests(&path, false);
                        editor.view.masking.end_drag();
                        editor.paint_for_tests(&[[0.5, 0.2], [0.5, 0.8]], true);
                    } else {
                        editor.paint_for_tests(&path, false);
                    }
                });
            },
            |editor, ctx| {
                egui::Area::new(egui::Id::new("shot-viewport"))
                    .fixed_pos(Pos2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_size(Vec2::new(480., 340.));
                        ui.set_max_size(Vec2::new(480., 340.));
                        let frame = editor.begin_edit_frame();
                        editor.viewport_ui(ui);
                        editor.finish_edit_frame(frame, ctx);
                    });
            },
        );
    }
}

/// A stroke begun before the last one's render has shown leaves that one
/// drawn; each goes when a render has it. A drag another tool or Space ends
/// keeps only the stroke's texture.
#[test]
fn a_stroke_stays_drawn_while_the_next_is_painted() {
    let ctx = egui::Context::default();
    let mut editor = editor(&ctx);
    paint(
        &ctx,
        &mut editor,
        Pos2::new(40., 80.),
        Pos2::new(200., 80.),
        true,
    );
    let first = editor.document.edit.recipe().clone();
    paint(
        &ctx,
        &mut editor,
        Pos2::new(40., 160.),
        Pos2::new(200., 160.),
        false,
    );
    assert!(editor.view.masking.live().is_some());
    assert_eq!(editor.view.masking.painted(), 1, "the first, still drawn");
    // A render with all of the first and part of the second.
    editor.view.masking.render_shown(&first);
    assert_eq!(editor.view.masking.painted(), 0);
    assert!(editor.view.masking.live().is_some());
    // Another tool ends the drag: the stroke waits for its render.
    editor.view.masking.end_drag();
    assert!(editor.view.masking.live().is_none());
    assert_eq!(editor.view.masking.painted(), 1);
    let all = editor.document.edit.recipe().clone();
    editor.view.masking.render_shown(&all);
    assert_eq!(drawn(&editor), 0);
}

/// A stroke whose coverage doesn't simply add to its mask's (a component
/// subtracted or intersected, inverted, or an inverted mask) shows with its
/// render, as the overlay would otherwise show the opposite of what it does.
#[test]
fn a_stroke_that_takes_away_from_its_mask_shows_with_its_render() {
    use crate::model::masks::MaskOp;
    for change in [
        |e: &mut Editor| {
            e.create_mask(
                crate::app::develop::tools::mask_tool::Kind::Brush,
                Some(MaskOp::Subtract),
            )
        },
        |e: &mut Editor| {
            e.create_mask(
                crate::app::develop::tools::mask_tool::Kind::Brush,
                Some(MaskOp::Intersect),
            )
        },
        |e: &mut Editor| e.document.edit.recipe_mut().masks[0].components[0].invert = true,
        |e: &mut Editor| e.document.edit.recipe_mut().masks[0].invert = true,
    ] as [fn(&mut Editor); 4]
    {
        let ctx = egui::Context::default();
        let mut editor = editor(&ctx);
        in_edit_frame(&ctx, &mut editor, change);
        paint(
            &ctx,
            &mut editor,
            Pos2::new(40., 110.),
            Pos2::new(200., 130.),
            false,
        );
        assert_eq!(drawn(&editor), 0);
    }
}
