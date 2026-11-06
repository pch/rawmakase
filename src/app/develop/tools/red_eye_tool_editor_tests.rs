//! Editor tests of `red_eye_tool`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use eframe::egui::{Pos2, Rect};
use std::sync::Arc;

#[test]
fn red_eye_tool_adds_moves_and_deletes_one_history_step_each() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let eyes = [([100., 100.], 10.), ([300., 260.], 8.)];
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: (0..160000)
            .map(|i| {
                let (x, y) = ((i % 400) as f32, (i / 400) as f32);
                let red = eyes.iter().any(|(c, r)| (x - c[0]).hypot(y - c[1]) <= *r);
                if red {
                    [0.6, 0.03, 0.03]
                } else {
                    [0.55, 0.35, 0.25]
                }
            })
            .collect(),
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
    editor.view.tool = crate::app::develop::state::Tool::RedEye;
    editor.view.red_eye.size = 0.06;
    let mut frame = |events: Vec<egui::Event>| {
        let edit = editor.begin_edit_frame();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                ctx.input(|i| editor.red_eye_keys(i));
                editor.viewport_ui(ui);
            },
        );
        editor.finish_edit_frame(edit, &ctx);
        output.textures_delta.clear();
        let (steps, applied) = editor.document.edit.history().steps();
        let names: Vec<String> = steps[..applied].iter().map(|s| s.name.clone()).collect();
        (
            editor.document.edit.recipe().red_eye.clone(),
            names,
            editor.messages.line().to_string(),
        )
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![]);
    // With the circle sized (by the wheel or [ ]), pressing on the first eye finds its
    // pupil: one step. Moving while pressed doesn't resize the circle, as in Lightroom.
    let (a, b) = (Pos2::new(50., 50.), Pos2::new(62., 50.));
    frame(vec![egui::Event::PointerMoved(a), button(a, true)]);
    for k in 1..=6 {
        frame(vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 6.)]);
    }
    let (ops, names, _) = frame(vec![button(b, false)]);
    assert_eq!(ops.len(), 1);
    assert!((ops[0].center[0] - 0.25).abs() < 0.005 && (ops[0].center[1] - 0.25).abs() < 0.005);
    assert!(
        (ops[0].radius[0] * 400. - 10.).abs() < 1.5,
        "{:?}",
        ops[0].radius
    );
    assert_eq!(names, ["Add Red Eye Correction"]);
    // A click on the second eye uses the same size.
    let c = Pos2::new(150., 130.);
    frame(vec![egui::Event::PointerMoved(c), button(c, true)]);
    let (ops, names, _) = frame(vec![button(c, false)]);
    assert_eq!(ops.len(), 2);
    assert!((ops[1].center[0] - 0.75).abs() < 0.005 && (ops[1].center[1] - 0.65).abs() < 0.005);
    assert_eq!(names.len(), 2);
    // A click on nothing red adds nothing and says why.
    let d = Pos2::new(180., 20.);
    frame(vec![egui::Event::PointerMoved(d), button(d, true)]);
    let (ops, names, status) = frame(vec![button(d, false)]);
    assert_eq!((ops.len(), names.len()), (2, 2));
    assert!(status.contains("Unable to find red eye"), "{status}");
    // Dragging the first correction moves it in one step.
    let e = Pos2::new(70., 60.);
    frame(vec![egui::Event::PointerMoved(a), button(a, true)]);
    for k in 1..=5 {
        frame(vec![egui::Event::PointerMoved(a + (e - a) * k as f32 / 5.)]);
    }
    let (ops, names, _) = frame(vec![button(e, false)]);
    assert!((ops[0].center[0] - 0.35).abs() < 0.005);
    assert_eq!(names.last().unwrap(), "Update Red Eye Correction");
    assert_eq!(names.len(), 3);
    // Delete removes the selected correction.
    let key = egui::Event::Key {
        key: egui::Key::Delete,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let (ops, names, _) = frame(vec![key]);
    assert_eq!(ops.len(), 1);
    assert_eq!(names.last().unwrap(), "Delete Red Eye Correction");
    // Undo brings it back where it was.
    let mut recipe = editor.document.edit.recipe().clone();
    assert!(editor.document.edit.history_mut().undo(&mut recipe));
    assert_eq!(recipe.red_eye.len(), 2);
    assert!((recipe.red_eye[0].center[0] - 0.35).abs() < 0.005);
}

#[test]
fn a_new_red_eye_correction_turns_the_red_eye_switch_on() {
    use crate::model::panels::{Panel, PanelState};
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 200,
        height: 200,
        pixels: (0..40000)
            .map(|i| {
                let (x, y) = ((i % 200) as f32, (i / 200) as f32);
                if (x - 100.).hypot(y - 100.) <= 8. {
                    [0.6, 0.03, 0.03]
                } else {
                    [0.55, 0.35, 0.25]
                }
            })
            .collect(),
        metadata: Metadata {
            width: 200,
            height: 200,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    editor.document.set_image(image);
    let panels = &mut editor.document.edit.setup_mut().panels;
    panels.set(Panel::RedEye, PanelState::Off);
    in_edit_frame(&ctx, &mut editor, |e| e.add_red_eye([0.5, 0.5], 0.1));
    assert_eq!(editor.document.edit.recipe().red_eye.len(), 1);
    assert_eq!(
        editor.document.edit.recipe().panels.state(Panel::RedEye),
        PanelState::On
    );
}

#[test]
fn pet_eye_type_finds_a_glowing_pupil_and_adds_a_catchlight() {
    use crate::model::red_eye::{DEFAULT_CATCHLIGHT, EyeKind};
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 200,
        height: 200,
        pixels: (0..40000)
            .map(|i| {
                let d = ((i % 200) as f32 - 100.).hypot((i / 200) as f32 - 100.);
                if d <= 10. {
                    [0.5, 0.8, 0.15]
                } else if d <= 24. {
                    [0.25, 0.18, 0.05]
                } else {
                    [0.3, 0.25, 0.2]
                }
            })
            .collect(),
        metadata: Metadata {
            width: 200,
            height: 200,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    });
    editor.document.set_image(image);
    // Red Eye finds nothing red there.
    in_edit_frame(&ctx, &mut editor, |e| e.add_red_eye([0.5, 0.5], 0.15));
    assert!(editor.document.edit.recipe().red_eye.is_empty());
    assert!(editor.messages.line().contains("Unable to find red eye"));
    editor.view.red_eye.pet = crate::app::develop::tools::red_eye_tool::PupilType::Pet;
    in_edit_frame(&ctx, &mut editor, |e| e.add_red_eye([0.5, 0.5], 0.15));
    let eyes = &editor.document.edit.recipe().red_eye;
    assert_eq!(eyes.len(), 1);
    assert_eq!(
        eyes[0].kind,
        EyeKind::Pet {
            catchlight: Some(DEFAULT_CATCHLIGHT)
        }
    );
    assert!((eyes[0].radius[0] * 200. - 10.).abs() < 1.5);
    // Selecting a correction makes its type the one new corrections get.
    editor.view.red_eye.pet = crate::app::develop::tools::red_eye_tool::PupilType::Red;
    editor.select_red_eye(Some(0));
    assert_eq!(
        editor.view.red_eye.pet,
        crate::app::develop::tools::red_eye_tool::PupilType::Pet
    );
    // Even when the remembered type is stale (after an undo, say), a new correction
    // takes the selected one's type, as the Type menu shows.
    editor.view.red_eye.pet = crate::app::develop::tools::red_eye_tool::PupilType::Red;
    in_edit_frame(&ctx, &mut editor, |e| e.add_red_eye([0.5, 0.5], 0.15));
    assert_eq!(editor.document.edit.recipe().red_eye.len(), 2);
    editor.document.edit.setup_mut().red_eye.pop();
    editor.select_red_eye(Some(0));
    // A click on the catchlight's handle, even at the pupil's edge outside the
    // ellipse, is not a new search.
    editor.document.edit.setup_mut().red_eye[0].kind = EyeKind::Pet {
        catchlight: Some([1., 0.]),
    };
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor.view.tool = crate::app::develop::state::Tool::RedEye;
    let handle = editor.document.edit.recipe().red_eye[0]
        .catchlight_at(1.)
        .unwrap();
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                editor.viewport_ui(ui);
            },
        );
        output.textures_delta.clear();
        editor.document.edit.recipe().red_eye.len()
    };
    frame(vec![]);
    let p = Pos2::new(handle[0] * 200., handle[1] * 200.);
    let button = |pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![egui::Event::PointerMoved(p), button(true)]);
    assert_eq!(frame(vec![button(false)]), 1);
}

#[test]
fn red_eye_tool_refuses_a_red_area_too_large_to_be_a_pupil() {
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
        pixels: (0..160000)
            .map(|i| {
                let (x, y) = ((i % 400) as f32, (i / 400) as f32);
                if (x - 200.).hypot(y - 200.) <= 150. {
                    [0.6, 0.03, 0.03]
                } else {
                    [0.55, 0.35, 0.25]
                }
            })
            .collect(),
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
    in_edit_frame(&ctx, &mut editor, |e| e.add_red_eye([0.5, 0.5], 0.48));
    assert!(editor.document.edit.recipe().red_eye.is_empty());
    assert!(
        editor.messages.line().contains("Unable to find red eye"),
        "{}",
        editor.messages.line()
    );
    editor.document.edit.recipe().validate().unwrap();
}
