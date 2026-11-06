//! Editor tests of `guided_tool`: the whole editor driven through its frames.
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use crate::develop;
use eframe::egui::{Pos2, Rect};
use std::sync::Arc;

/// The Guided Upright tool on the photo: drawing a guide, drawing a second, moving an
/// end and deleting a guide are one History step each, and two guides solve to a
/// correction (docs/transform.md#guided-upright).
#[test]
fn guided_upright_gestures_are_one_history_step_each() {
    use crate::model::transform::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.module = Module::Develop;
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
    // The other modes analysed already, so the guides are solved at once.
    e.document.edit.setup_mut().upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 5];
    e.view.tool = crate::app::develop::state::Tool::Guided;
    let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(600., 400.));
    let mut frame = |e: &mut Editor, events: Vec<egui::Event>| {
        let edit = e.begin_edit_frame();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(area),
                events,
                ..Default::default()
            },
            |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(area.size(), egui::Sense::click_and_drag());
                e.guided_overlay(ui, &response, rect, rect);
            },
        );
        output.textures_delta.clear();
        e.finish_edit_frame(edit, &ctx);
    };
    let button = |p: Pos2, pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let drag = |e: &mut Editor,
                frame: &mut dyn FnMut(&mut Editor, Vec<egui::Event>),
                from: Pos2,
                to: Pos2| {
        frame(e, vec![egui::Event::PointerMoved(from), button(from, true)]);
        for k in 1..=4 {
            frame(
                e,
                vec![egui::Event::PointerMoved(
                    from + (to - from) * k as f32 / 4.,
                )],
            );
        }
        frame(e, vec![button(to, false)]);
        frame(e, vec![]);
    };
    frame(&mut e, vec![]);
    // Two converging verticals.
    drag(
        &mut e,
        &mut frame,
        Pos2::new(120., 40.),
        Pos2::new(100., 360.),
    );
    let guides = &e.document.edit.recipe().upright.guides;
    assert_eq!(guides.len(), 1);
    assert!(
        (guides[0].a[0] - 0.2).abs() < 1e-3 && (guides[0].a[1] - 0.1).abs() < 1e-3,
        "{guides:?}"
    );
    assert_eq!(e.document.edit.recipe().upright.mode, UprightMode::Guided);
    drag(
        &mut e,
        &mut frame,
        Pos2::new(480., 40.),
        Pos2::new(500., 360.),
    );
    assert_eq!(e.document.edit.recipe().upright.guides.len(), 2);
    let solved = e
        .document
        .edit
        .recipe()
        .upright
        .correction()
        .expect("a correction");
    assert!(solved[2][1].abs() > 1e-3, "{solved:?}");
    // The first guide's lower end, where it shows now the photo is corrected.
    let g = develop::Geometry::new(e.document.full().unwrap(), e.document.edit.recipe(), 0);
    let [u, v] = g.from_upright_frame(e.document.edit.recipe().upright.guides[0].b);
    let end = Pos2::new(u * 600., v * 400.);
    drag(&mut e, &mut frame, end, end + Vec2::new(-10., 0.));
    assert_eq!(e.document.edit.recipe().upright.guides.len(), 2);
    // Selected by the drag, and deleted.
    let edit = e.begin_edit_frame();
    e.delete_guide();
    e.finish_edit_frame(edit, &ctx);
    assert_eq!(e.document.edit.recipe().upright.guides.len(), 1);
    let (steps, applied) = e.document.edit.history().steps();
    let names: Vec<&str> = steps.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["Add Guide", "Add Guide", "Move Guide", "Delete Guide"]
    );
    assert_eq!(applied, 4);
    // One guide corrects nothing, and says so.
    assert_eq!(e.document.edit.recipe().upright.correction(), None);
    assert!(
        e.messages.line().contains("two or more guides"),
        "{}",
        e.messages.line()
    );
    e.undo();
    assert_eq!(e.document.edit.recipe().upright.guides.len(), 2);
    assert!(e.document.edit.recipe().upright.correction().is_some());
    // Leaving Guided by any route closes the tool, so a drag can't switch it back.
    e.view.tool = crate::app::develop::state::Tool::Guided;
    let edit = e.begin_edit_frame();
    e.document.edit.setup_mut().upright = Default::default();
    e.finish_edit_frame(edit, &ctx);
    assert!(!e.view.is(crate::app::develop::state::Tool::Guided));
}
