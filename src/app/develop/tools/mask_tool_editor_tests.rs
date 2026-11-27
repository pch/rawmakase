//! Editor tests of `mask_tool`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use eframe::egui::{Pos2, Rect};
use std::sync::Arc;

#[test]
fn a_mask_brush_paints_live_and_preserves_the_overlay_choice() {
    use crate::app::worker::Overlay;
    use crate::model::masks::MaskShape;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.2; 3]; 160000].into(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                editor.viewport_ui(ui);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut editor, vec![]);
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Brush, None)
    });
    let steps = editor.document.edit.history().steps().0.len();
    assert!(
        editor.view.masking.overlay,
        "new editors show masks by default"
    );
    assert!(matches!(editor.overlay(), Overlay::Mask { index: 0, .. }));
    let points = |editor: &Editor| match &editor.document.edit.recipe().masks[0].components[0].shape
    {
        MaskShape::Brush { strokes } => strokes.iter().map(|s| s.points.len()).collect::<Vec<_>>(),
        shape => panic!("{shape:?}"),
    };
    let (a, b) = (Pos2::new(20., 100.), Pos2::new(180., 100.));
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    let mut painted = 0;
    for k in 1..=8 {
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 8.)],
        );
        // Mid-stroke, the photo already has the stroke so far, with the
        // default overlay still enabled.
        let now = points(&editor);
        assert_eq!(now.len(), 1, "one stroke while painting");
        assert!(now[0] >= painted);
        painted = now[0];
        assert!(matches!(editor.overlay(), Overlay::Mask { index: 0, .. }));
    }
    assert!(painted > 3, "{painted}");
    // Still one step to come, once the stroke ends.
    assert_eq!(editor.document.edit.history().steps().0.len(), steps);
    frame(&mut editor, vec![button(b, false)]);
    assert_eq!(points(&editor), vec![painted]);
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    // The overlay stays once the stroke ends, until O hides it.
    assert!(matches!(editor.overlay(), Overlay::Mask { index: 0, .. }));
    let o = egui::Event::Key {
        key: egui::Key::O,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    in_edit_frame(&ctx, &mut editor, |editor| {
        ctx.input_mut(|i| i.events.push(o));
        editor.mask_keys(&ctx);
    });
    assert_eq!(editor.overlay(), Overlay::None);
    // Further strokes start away from the mask's pin, which a drag would move.
    let (a, b) = (Pos2::new(20., 140.), Pos2::new(180., 140.));
    // An Undo while painting takes back the stroke so far and ends the painting.
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a + (b - a) * 0.5)],
    );
    assert_eq!(points(&editor).len(), 2);
    assert!(!editor.view.masking.overlay, "painting must preserve O off");
    assert_eq!(editor.overlay(), Overlay::None);
    in_edit_frame(&ctx, &mut editor, |editor| editor.undo());
    frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
    frame(&mut editor, vec![button(b, false)]);
    assert_eq!(points(&editor), vec![painted]);
    // So does an Undo with the pointer held still: a Redo then brings the stroke back
    // as far as it was painted, and the drag that goes on paints no more of it.
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a + (b - a) * 0.5)],
    );
    let partial = points(&editor);
    assert_eq!(partial.len(), 2);
    in_edit_frame(&ctx, &mut editor, |editor| editor.undo());
    frame(&mut editor, vec![]);
    in_edit_frame(&ctx, &mut editor, |editor| editor.redo());
    assert_eq!(points(&editor), partial);
    frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
    frame(&mut editor, vec![button(b, false)]);
    assert_eq!(points(&editor), partial);
    // Or Undo and Redo in one frame, before the photo is drawn again.
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a + (b - a) * 0.5)],
    );
    let partial = points(&editor);
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.undo();
        editor.redo();
    });
    assert_eq!(points(&editor), partial);
    frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
    frame(&mut editor, vec![button(b, false)]);
    assert_eq!(points(&editor), partial);
    // Space taking over mid-stroke ends it: once Space is let go, the drag that goes
    // on paints nothing more into it.
    let space = |pressed| egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a + (b - a) * 0.25)],
    );
    let started = points(&editor);
    assert_eq!(started.len(), partial.len() + 1);
    frame(&mut editor, vec![space(true)]);
    frame(&mut editor, vec![space(false)]);
    frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
    frame(&mut editor, vec![button(b, false)]);
    assert_eq!(points(&editor), started);
    // Repeated painting and erasing keep the explicit overlay choice (#697).
    for (brush, overlay) in [(0, false), (2, false), (2, true)] {
        editor.view.masking.brush = brush;
        editor.view.masking.overlay = overlay;
        let count = points(&editor).len();
        let (a, b) = (Pos2::new(40., 60.), Pos2::new(160., 60.));
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(a), button(a, true)],
        );
        frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
        assert_eq!(points(&editor).len(), count + 1);
        assert_eq!(editor.view.masking.overlay, overlay);
        assert_eq!(matches!(editor.overlay(), Overlay::Mask { .. }), overlay);
        frame(&mut editor, vec![button(b, false)]);
        assert_eq!(editor.view.masking.overlay, overlay);
        let MaskShape::Brush { strokes } =
            &editor.document.edit.recipe().masks[0].components[0].shape
        else {
            unreachable!()
        };
        assert_eq!(strokes.last().unwrap().erase, brush == 2);
    }
    editor.document.edit.recipe().validate().unwrap();
}

#[test]
fn dragging_a_mask_pin_moves_the_mask() {
    use crate::model::masks::{MaskComponent, MaskGroup, MaskShape};
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.2; 3]; 160000].into(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                editor.viewport_ui(ui);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut editor, vec![]);
    // A brush mask with a stroke, and a radial gradient added to it.
    let stroke = crate::model::masks::BrushStroke {
        points: vec![[0.3, 0.3], [0.5, 0.3]].into(),
        radius: 0.05,
        feather: 0.5,
        flow: 1.,
        density: 1.,
        erase: false,
        auto_mask: false,
    };
    in_edit_frame(&ctx, &mut editor, |e| {
        e.view.set_tool(crate::app::develop::state::Tool::Mask);
        let mut radial = MaskComponent::new(MaskShape::Radial {
            center: [0.7, 0.7],
            radii: [0.1, 0.1],
            angle: 0.,
            feather: 0.5,
        });
        radial.op = crate::model::masks::MaskOp::Add;
        e.document.edit.recipe_mut().masks.push(MaskGroup {
            components: vec![
                MaskComponent::new(MaskShape::Brush {
                    strokes: vec![stroke],
                }),
                radial,
            ],
            ..Default::default()
        });
    });
    frame(&mut editor, vec![]);
    let steps = editor.document.edit.history().steps().0.len();
    // Over the pin, the mask's overlay shows though Show Overlay is off; it goes when
    // the pointer leaves.
    editor.view.masking.overlay = false;
    assert!(!editor.view.masking.overlay);
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(Pos2::new(62., 61.))],
    );
    assert!(matches!(
        editor.overlay(),
        crate::app::worker::Overlay::Mask { index: 0, .. }
    ));
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(Pos2::new(150., 40.))],
    );
    assert_eq!(editor.overlay(), crate::app::worker::Overlay::None);
    // The pin sits on the stroke's first dab; dragging it by a tenth of the photo
    // moves every part of the mask with it, as one step.
    let (a, b) = (Pos2::new(60., 60.), Pos2::new(80., 80.));
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(a), button(a, true)],
    );
    for k in 1..=4 {
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 4.)],
        );
    }
    frame(&mut editor, vec![button(b, false)]);
    let m = &editor.document.edit.recipe().masks[0];
    let near = |p: [f32; 2], q: [f32; 2]| (p[0] - q[0]).abs() < 0.01 && (p[1] - q[1]).abs() < 0.01;
    let MaskShape::Brush { strokes } = &m.components[0].shape else {
        panic!()
    };
    assert!(
        near(strokes[0].points[0], [0.4, 0.4]),
        "{:?}",
        strokes[0].points
    );
    assert!(
        near(strokes[0].points[1], [0.6, 0.4]),
        "{:?}",
        strokes[0].points
    );
    let MaskShape::Radial { center, .. } = m.components[1].shape else {
        panic!()
    };
    assert!(near(center, [0.8, 0.8]), "{center:?}");
    assert_eq!(strokes.len(), 1, "the drag painted nothing");
    assert_eq!(editor.view.masking.selected, Some(0));
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    assert!(!editor.view.zoom.on);
    // With its brush selected, as after painting, the pin still moves the mask.
    assert!(editor.mask_brush_shown());
    let c = Pos2::new(80., 100.);
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(b), button(b, true)],
    );
    for k in 1..=4 {
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(b + (c - b) * k as f32 / 4.)],
        );
    }
    frame(&mut editor, vec![button(c, false)]);
    let MaskShape::Brush { strokes } = &editor.document.edit.recipe().masks[0].components[0].shape
    else {
        panic!()
    };
    assert_eq!(strokes.len(), 1, "the drag painted nothing");
    assert!(
        near(strokes[0].points[0], [0.4, 0.5]),
        "{:?}",
        strokes[0].points
    );
    // Undo, Space or deleting the mask while moving it ends the move: the drag that
    // goes on moves nothing.
    let space = |pressed| egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let pin = c;
    let d = Pos2::new(120., 120.);
    let base = editor.document.edit.recipe().masks[0].clone();
    for interruption in 0..5 {
        // The mask with its pin at `pin`, and a second like it, which a Delete moves
        // into its place.
        in_edit_frame(&ctx, &mut editor, |e| {
            let mut other = base.clone();
            other.name = "Other".into();
            e.document.edit.recipe_mut().masks = vec![base.clone(), other];
            e.view.masking.selected = None;
        });
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(pin), button(pin, true)],
        );
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(pin + Vec2::new(4., 4.))],
        );
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(pin + Vec2::new(8., 8.))],
        );
        match interruption {
            0 => in_edit_frame(&ctx, &mut editor, |e| e.undo()),
            1 => {
                frame(&mut editor, vec![space(true)]);
                frame(&mut editor, vec![space(false)]);
            }
            // Delete, as its key does; back where the drag started, so the mask that
            // takes its place has the same shapes as the drag last left.
            2 | 3 => {
                if interruption == 3 {
                    frame(&mut editor, vec![egui::Event::PointerMoved(pin)]);
                }
                in_edit_frame(&ctx, &mut editor, |e| {
                    e.document.edit.recipe_mut().masks.remove(0);
                    e.view.masking.selected = None;
                });
            }
            // K starts another mask.
            _ => in_edit_frame(&ctx, &mut editor, |e| {
                e.create_mask(crate::app::develop::tools::mask_tool::Kind::Brush, None)
            }),
        }
        let before = editor.document.edit.recipe().masks.clone();
        frame(&mut editor, vec![egui::Event::PointerMoved(d)]);
        frame(&mut editor, vec![button(d, false)]);
        assert_eq!(
            editor.document.edit.recipe().masks,
            before,
            "interruption {interruption}"
        );
    }
}

#[test]
fn masking_tool_draws_gradients_paints_brushes_and_edits_handles() {
    use crate::model::masks::MaskShape;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.2; 3]; 160000].into(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    let mut frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                if editor.view.is(crate::app::develop::state::Tool::Mask) {
                    editor.mask_keys(ui.ctx())
                }
                editor.viewport_ui(ui);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let drag = |editor: &mut Editor,
                frame: &mut dyn FnMut(&mut Editor, Vec<egui::Event>),
                a: Pos2,
                b: Pos2| {
        frame(editor, vec![egui::Event::PointerMoved(a), button(a, true)]);
        for k in 1..=8 {
            frame(
                editor,
                vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 8.)],
            );
        }
        frame(editor, vec![button(b, false)]);
    };
    frame(&mut editor, vec![]);
    // M, then a drag, draws a linear gradient as a new mask.
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Linear, None)
    });
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(100., 40.),
        Pos2::new(100., 160.),
    );
    let masks = editor.document.edit.recipe().masks.clone();
    assert_eq!(masks.len(), 1);
    let MaskShape::Linear { from, to } = masks[0].components[0].shape else {
        panic!("{:?}", masks[0].components[0].shape);
    };
    assert!(
        (from[1] - 0.2).abs() < 0.02 && (to[1] - 0.8).abs() < 0.02,
        "{from:?} {to:?}"
    );
    assert!(!editor.view.zoom.on);
    // Dragging its end handle moves only that end.
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(100., 160.),
        Pos2::new(140., 170.),
    );
    let MaskShape::Linear { from: f2, to: t2 } =
        editor.document.edit.recipe().masks[0].components[0].shape
    else {
        panic!()
    };
    assert_eq!(f2, from);
    assert!(
        (t2[0] - 0.7).abs() < 0.02 && (t2[1] - 0.85).abs() < 0.02,
        "{t2:?}"
    );
    // Shift+M makes a radial gradient; K a brush that paints.
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Radial, None)
    });
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(60., 60.),
        Pos2::new(90., 80.),
    );
    assert!(matches!(
        editor.document.edit.recipe().masks[1].components[0].shape,
        MaskShape::Radial { radii, .. } if (radii[0] - 0.15).abs() < 0.02 && (radii[1] - 0.1).abs() < 0.02
    ));
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Brush, None)
    });
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(20., 180.),
        Pos2::new(180., 180.),
    );
    let MaskShape::Brush { strokes } = &editor.document.edit.recipe().masks[2].components[0].shape
    else {
        panic!()
    };
    assert_eq!(strokes.len(), 1);
    assert!(strokes[0].points.len() > 3);
    // Add a subtracted brush to the brush mask, then delete the mask with Delete.
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(
            crate::app::develop::tools::mask_tool::Kind::Brush,
            Some(crate::model::masks::MaskOp::Subtract),
        )
    });
    assert_eq!(editor.document.edit.recipe().masks[2].components.len(), 2);
    frame(
        &mut editor,
        vec![egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(editor.document.edit.recipe().masks.len(), 2);
    editor.document.edit.recipe().validate().unwrap();
    // The whole editor draws the Masking and Remove drawers without disturbing edits.
    editor.view.masking.selected = Some(0);
    editor.view.masking.component = Some(0);
    let saved = editor.document.edit.recipe().clone();
    for tool in [
        crate::app::develop::state::Tool::Mask,
        crate::app::develop::state::Tool::Remove,
        crate::app::develop::state::Tool::Crop,
    ] {
        editor.view.set_tool(tool);
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 900.))),
                    ..Default::default()
                },
                |ui| editor.draw(ui),
            );
            output.textures_delta.clear();
        }
    }
    assert_eq!(*editor.document.edit.recipe(), saved);
}

/// While a mask's overlay shows, the view renders without the SDR rendition's
/// sliders: the overlay is read from the rendered pixels, and a range mask selects
/// from the rendition without them, so the overlay shows what the mask selects. With
/// no overlay, the SDR rendition renders them.
#[test]
fn a_mask_overlay_renders_the_rendition_its_ranges_select_from() {
    let ctx = egui::Context::default();
    let (mut editor, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    editor.document.edit.setup_mut().hdr.enabled = true;
    editor.document.edit.setup_mut().hdr.sdr.brightness = 0.5;
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Brush, None)
    });
    assert!(matches!(
        editor.overlay(),
        crate::app::worker::Overlay::Mask { .. }
    ));
    editor.schedule_edit();
    let pending = editor.preview.pending.recipe.as_ref().unwrap();
    assert_eq!(pending.hdr.sdr, Default::default());
    assert!(pending.hdr.enabled, "only the sliders are left out");
    editor.view.masking.overlay = false;
    editor.schedule_edit();
    let pending = editor.preview.pending.recipe.as_ref().unwrap();
    assert_eq!(pending.hdr.sdr.brightness, 0.5);
}

/// The range pickers sample the rendition a range mask selects from, without the SDR
/// rendition's sliders: the color they read is the same with them as without.
#[test]
fn the_range_pickers_sample_without_the_sdr_rendition_sliders() {
    let ctx = egui::Context::default();
    let (mut editor, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    editor.document.edit.setup_mut().hdr.enabled = true;
    let rect = Rect::from_min_size(Pos2::ZERO, egui::Vec2::splat(100.));
    let at = Pos2::new(50., 50.);
    let plain = editor.developed_lab(at, rect).unwrap();
    editor.document.edit.setup_mut().hdr.sdr.brightness = 0.8;
    assert_eq!(editor.developed_lab(at, rect).unwrap(), plain);
}

/// Zoomed out below Fit, a radial gradient can be created and edited with its
/// centre and handles in the backdrop outside the photo frame, and stays valid.
#[test]
fn radial_mask_can_be_drawn_and_edited_outside_a_zoomed_out_photo() {
    use crate::model::masks::MaskShape;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.2; 3]; 160000].into(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    // Viewport 400x400: Fit is 1.0, zoom at 0.5 draws the 200x200 photo centred
    // at [100..300, 100..300] with 100pt of backdrop on every side.
    editor.view.zoom.set(0.5);
    editor.view.zoom_key = (true, 0.5);
    let mut frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.))),
                events,
                ..Default::default()
            },
            |ui| {
                let frame = editor.begin_edit_frame();
                editor.viewport_ui(ui);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let drag = |editor: &mut Editor,
                frame: &mut dyn FnMut(&mut Editor, Vec<egui::Event>),
                a: Pos2,
                b: Pos2| {
        frame(editor, vec![egui::Event::PointerMoved(a), button(a, true)]);
        for k in 1..=8 {
            frame(
                editor,
                vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 8.)],
            );
        }
        frame(editor, vec![button(b, false)]);
    };
    frame(&mut editor, vec![]);
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Radial, None)
    });
    // Start at x=50 (50pt left of the photo's left edge at x=100, i.e. u = -0.25)
    // and drag onto the photo at x=170, y=260.
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(50., 200.),
        Pos2::new(170., 260.),
    );
    let MaskShape::Radial { center, radii, .. } =
        editor.document.edit.recipe().masks[0].components[0].shape
    else {
        panic!("expected a radial gradient");
    };
    assert!(
        (center[0] - (-0.25)).abs() < 0.02 && (center[1] - 0.5).abs() < 0.02,
        "{center:?}"
    );
    assert!(
        (radii[0] - 0.6).abs() < 0.02 && (radii[1] - 0.3).abs() < 0.02,
        "{radii:?}"
    );
    editor.document.edit.recipe().validate().unwrap();
    // Dragging the off-frame centre pin moves the radial gradient further out.
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(50., 200.),
        Pos2::new(30., 180.),
    );
    let MaskShape::Radial { center, .. } =
        editor.document.edit.recipe().masks[0].components[0].shape
    else {
        panic!();
    };
    assert!(
        (center[0] - (-0.35)).abs() < 0.02 && (center[1] - 0.4).abs() < 0.02,
        "{center:?}"
    );
    editor.document.edit.recipe().validate().unwrap();
    // Zoomed all the way out to 6.25%, dragging across the whole viewport still
    // stays within model::masks's validation bounds.
    editor.view.zoom.set(0.0625);
    editor.view.zoom_key = (true, 0.0625);
    frame(&mut editor, vec![]);
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Radial, None)
    });
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(2., 2.),
        Pos2::new(398., 398.),
    );
    editor.document.edit.recipe().validate().unwrap();
    // Clicking the backdrop while Color Range is armed keeps it armed rather
    // than cancelling it without creating a mask.
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Color, None)
    });
    let masks_before = editor.document.edit.recipe().masks.len();
    let outside = Pos2::new(10., 10.);
    frame(
        &mut editor,
        vec![egui::Event::PointerMoved(outside), button(outside, true)],
    );
    frame(&mut editor, vec![button(outside, false)]);
    assert!(
        editor.view.masking.is_placing(),
        "clicking backdrop must not cancel Color Range"
    );
    assert_eq!(editor.document.edit.recipe().masks.len(), masks_before);
    // A linear gradient dragged far beyond the same ±4 edge does not collapse
    // to a zero-length gradient, and pin-moving near ±4 translates rigidly.
    in_edit_frame(&ctx, &mut editor, |editor| {
        editor.create_mask(crate::app::develop::tools::mask_tool::Kind::Linear, None)
    });
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(390., 390.),
        Pos2::new(398., 398.),
    );
    let MaskShape::Linear { from, to } = editor
        .document
        .edit
        .recipe()
        .masks
        .last()
        .unwrap()
        .components[0]
        .shape
    else {
        panic!("expected linear gradient");
    };
    assert_ne!(from, to, "clamped linear gradient must not collapse");
    editor.document.edit.recipe().validate().unwrap();
}

/// Visual check of a photo zoomed out below Fit with a radial gradient centred
/// outside the frame, for `RAWMAKASE_UI_SHOTS`.
#[test]
fn shots_of_radial_mask_outside_zoomed_out_photo() {
    use crate::model::masks::{MaskComponent, MaskGroup, MaskShape};
    use eframe::egui::{self, Color32, Vec2};
    let Some(dir) = std::env::var_os("RAWMAKASE_UI_SHOTS").map(std::path::PathBuf::from) else {
        return;
    };
    let mut harness = egui_kittest::Harness::builder()
        .with_size(Vec2::new(720., 480.))
        .with_pixels_per_point(2.)
        .wgpu()
        .build_ui_state(
            |ui, editor: &mut Option<Editor>| {
                let editor = editor.get_or_insert_with(|| {
                    let text = fastframe_text::detect();
                    crate::app::install_fonts(ui.ctx(), &text);
                    crate::app::ui::icons::install(ui.ctx());
                    let mut e = Editor::with_context(ui.ctx(), None, Default::default(), None);
                    e.module = crate::app::Module::Develop;
                    let image = Arc::new(CameraImage {
                        recovered: Default::default(),
                        width: 600,
                        height: 400,
                        pixels: vec![[0.25; 3]; 240_000].into(),
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
                    });
                    e.document.set_image(image);
                    e.preview.texture = Some(
                        ui.ctx()
                            .load_texture(
                                "photo",
                                egui::ColorImage::filled(
                                    [300, 200],
                                    Color32::from_rgb(75, 92, 110),
                                ),
                                egui::TextureOptions::LINEAR,
                            )
                            .into(),
                    );
                    e.document.edit.setup_mut().masks.push(MaskGroup {
                        components: vec![MaskComponent::new(MaskShape::Radial {
                            center: [-0.2, 0.5],
                            radii: [0.55, 0.3],
                            angle: 0.,
                            feather: 0.5,
                        })],
                        ..Default::default()
                    });
                    e.view.set_tool(crate::app::develop::state::Tool::Mask);
                    e.view.masking.selected = Some(0);
                    e.view.masking.component = Some(0);
                    // Zoomed out below Fit so room appears around the photo.
                    e.view.zoom.set(0.8);
                    e.view.zoom_key = (true, 0.8);
                    e
                });
                editor.viewport_ui(ui);
            },
            None,
        );
    harness.run_steps(4);
    let image = harness.render().expect("wgpu could not draw the shot");
    std::fs::create_dir_all(&dir).expect("the shots folder");
    image
        .save(dir.join("zoom-out-radial-mask.png"))
        .expect("the shot");
}
