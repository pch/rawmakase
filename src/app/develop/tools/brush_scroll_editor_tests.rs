//! Editor tests of `brush_scroll`: the whole editor driven through its frames.
use crate::app::*;
use eframe::egui::Pos2;

#[test]
fn red_eye_brackets_resize_the_circle_a_click_uses() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.view.tool = crate::app::develop::state::Tool::RedEye;
    let start = editor.view.red_eye.size;
    let mut press = |key| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |_| ctx.input(|i| editor.red_eye_keys(i)),
        );
        output.textures_delta.clear();
        editor.view.red_eye.size
    };
    let larger = press(egui::Key::CloseBracket);
    assert!(larger > start, "{larger} after {start}");
    let smaller = press(egui::Key::OpenBracket);
    assert!(
        (smaller - start).abs() < 1e-6,
        "{smaller} back from {start}"
    );
    // It stays within sizes a pupil can have.
    for _ in 0..100 {
        press(egui::Key::CloseBracket);
    }
    assert!(editor.view.red_eye.size <= 0.25);
}

#[test]
fn scrolling_over_the_photo_resizes_the_brush_spot_and_red_eye_circle() {
    use crate::app::develop::tools::brush_scroll::{Adjust, MaskBrush, Scroll};
    use crate::model::masks::{MaskComponent, MaskGroup, MaskShape};
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let size = |lines| Scroll {
        lines,
        adjust: Adjust::Size,
        brush: MaskBrush::Current,
    };
    editor.view.tool = crate::app::develop::state::Tool::Remove;
    let spot = editor.view.retouch.size;
    editor.scroll_tool_size(size(1.));
    assert!(editor.view.retouch.size > spot);
    editor.view.tool = crate::app::develop::state::Tool::RedEye;
    let eye = editor.view.red_eye.size;
    editor.scroll_tool_size(size(-1.));
    assert!(editor.view.red_eye.size < eye);
    // The mask brush changes only while a brush is in use, as the cursor shows it.
    editor.view.tool = crate::app::develop::state::Tool::Mask;
    let brush = editor.view.masking.brushes[0];
    editor.scroll_tool_size(size(1.));
    assert_eq!(editor.view.masking.brushes[0], brush);
    editor.document.edit.setup_mut().masks = vec![MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Brush {
            strokes: Vec::new(),
        })],
        ..Default::default()
    }];
    editor.view.masking.selected = Some(0);
    editor.view.masking.component = Some(0);
    editor.scroll_tool_size(size(1.));
    assert!(editor.view.masking.brushes[0].size > brush.size);
    // Shift-scroll changes the feather instead, and with Option/Alt the Erase brush.
    editor.scroll_tool_size(Scroll {
        lines: -1.,
        adjust: Adjust::Feather,
        brush: MaskBrush::Current,
    });
    assert!(editor.view.masking.brushes[0].feather < brush.feather);
    let erase = editor.view.masking.brushes[2];
    editor.scroll_tool_size(Scroll {
        lines: 1.,
        adjust: Adjust::Size,
        brush: MaskBrush::Erase,
    });
    assert!(editor.view.masking.brushes[2].size > erase.size);
    // Other tools ignore it.
    editor.view.tool = crate::app::develop::state::Tool::Crop;
    let before = (editor.view.retouch.size, editor.view.red_eye.size);
    editor.scroll_tool_size(size(1.));
    assert_eq!((editor.view.retouch.size, editor.view.red_eye.size), before);
}

#[test]
fn wheel_events_carry_their_own_modifiers_and_plain_swipes_sideways_do_nothing() {
    use crate::app::develop::tools::brush_scroll::{Adjust, MaskBrush, Scroll};
    let ctx = egui::Context::default();
    let wheel = |delta: Vec2, modifiers| egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta,
        phase: egui::TouchPhase::Move,
        modifiers,
    };
    let read = |events| {
        let mut scrolls = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| scrolls = ui.input(Scroll::read),
        );
        output.textures_delta.clear();
        scrolls
    };
    // Shift held for the wheel event, even if it's let go by the frame: feather.
    let s = read(vec![wheel(Vec2::new(0., 1.), egui::Modifiers::SHIFT)]);
    assert_eq!((s[0].lines, s[0].adjust), (1., Adjust::Feather));
    // macOS turns Shift+wheel into a sideways scroll.
    let s = read(vec![wheel(Vec2::new(-2., 0.), egui::Modifiers::SHIFT)]);
    assert_eq!((s[0].lines, s[0].adjust), (-2., Adjust::Feather));
    // A plain sideways trackpad swipe sizes nothing.
    assert!(read(vec![wheel(Vec2::new(3., 0.), egui::Modifiers::NONE)]).is_empty());
    // Option/Alt picks the Erase brush.
    let s = read(vec![wheel(Vec2::new(0., -1.), egui::Modifiers::ALT)]);
    assert_eq!((s[0].adjust, s[0].brush), (Adjust::Size, MaskBrush::Erase));
    // A frame that also has a click or a key leaves the wheel alone.
    let click = egui::Event::PointerButton {
        pos: Pos2::new(5., 5.),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    };
    let key = egui::Event::Key {
        key: egui::Key::Num3,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let up = wheel(Vec2::new(0., 1.), egui::Modifiers::NONE);
    assert!(read(vec![up.clone(), click]).is_empty());
    assert!(read(vec![up, key]).is_empty());
}

#[test]
fn a_wheel_scroll_resizing_a_spot_is_one_history_step() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.view.tool = crate::app::develop::state::Tool::Remove;
    editor
        .document
        .edit
        .setup_mut()
        .retouch
        .push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    editor.view.retouch.selected = Some(0);
    let radius = editor.document.edit.recipe().retouch[0].radius();
    let notch = crate::app::develop::tools::brush_scroll::Scroll {
        lines: 1.,
        adjust: crate::app::develop::tools::brush_scroll::Adjust::Size,
        brush: crate::app::develop::tools::brush_scroll::MaskBrush::Current,
    };
    for _ in 0..5 {
        let edit = editor.begin_edit_frame();
        editor.scroll_tool_size(notch);
        editor.finish_edit_frame(edit, &ctx);
    }
    assert!(editor.document.edit.recipe().retouch[0].radius() > radius);
    assert!(editor.document.edit.history().in_gesture());
    // Once the scroll pauses, the five notches are one step.
    std::thread::sleep(std::time::Duration::from_millis(450));
    let edit = editor.begin_edit_frame();
    editor.finish_edit_frame(edit, &ctx);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(
        applied,
        1,
        "{:?}",
        steps.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}

#[test]
fn brackets_size_the_red_eye_circle_without_rating_the_photo() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("image.ARW"), b"rating fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let l = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let id = l.session.photos()[0].id;
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(l));
    editor.module = Module::Develop;
    editor.document.catalog_photo = Some(id);
    editor.view.tool = crate::app::develop::state::Tool::RedEye;
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::CloseBracket,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| editor.metadata_shortcuts(ui.ctx()),
    );
    output.textures_delta.clear();
    let library = editor.library.as_ref().unwrap();
    assert_eq!(
        library
            .session
            .photos()
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .rating,
        0
    );
    Ok(())
}

#[test]
fn an_edit_right_after_a_wheel_scroll_is_its_own_history_step() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.view.tool = crate::app::develop::state::Tool::Remove;
    editor
        .document
        .edit
        .setup_mut()
        .retouch
        .push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    editor.view.retouch.selected = Some(0);
    let edit = editor.begin_edit_frame();
    editor.scroll_tool_size(crate::app::develop::tools::brush_scroll::Scroll {
        lines: 1.,
        adjust: crate::app::develop::tools::brush_scroll::Adjust::Size,
        brush: crate::app::develop::tools::brush_scroll::MaskBrush::Current,
    });
    editor.finish_edit_frame(edit, &ctx);
    // At once, before the scroll pauses, a slider moves, naming its step.
    let edit = editor.begin_edit_frame();
    editor.document.edit.setup_mut().exposure = 0.5;
    editor
        .step_name
        .name("Exposure".to_string(), "+0.50".to_string());
    editor.finish_edit_frame(edit, &ctx);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 2);
    assert_ne!(steps[0].name, "Exposure");
    assert_eq!(steps[1].name, "Exposure");
}

#[test]
fn a_wheel_scroll_is_its_own_step_however_late_the_next_frame_comes() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.view.tool = crate::app::develop::state::Tool::Remove;
    editor
        .document
        .edit
        .setup_mut()
        .retouch
        .push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    editor.view.retouch.selected = Some(0);
    let notch = crate::app::develop::tools::brush_scroll::Scroll {
        lines: 1.,
        adjust: crate::app::develop::tools::brush_scroll::Adjust::Size,
        brush: crate::app::develop::tools::brush_scroll::MaskBrush::Current,
    };
    let edit = editor.begin_edit_frame();
    editor.scroll_tool_size(notch);
    editor.finish_edit_frame(edit, &ctx);
    // The pause passes with no frame, then the next frame brings a slider change.
    std::thread::sleep(std::time::Duration::from_millis(450));
    let edit = editor.begin_edit_frame();
    editor.document.edit.setup_mut().exposure = 0.5;
    editor
        .step_name
        .name("Exposure".to_string(), "+0.50".to_string());
    editor.finish_edit_frame(edit, &ctx);
    let (steps, applied) = editor.document.edit.history().steps();
    assert_eq!(applied, 2);
    assert_eq!(steps[1].name, "Exposure");
    // A rating or flag right after a scroll is logged after it, so Undo takes it first.
    let edit = editor.begin_edit_frame();
    editor.scroll_tool_size(notch);
    editor.finish_edit_frame(edit, &ctx);
    editor.sync_undo();
    let logged = editor.undo_log.len().0;
    editor.finish_wheel_gesture();
    assert_eq!(editor.undo_log.len().0, logged + 1);
    assert!(!editor.document.edit.history().in_gesture());
}

#[test]
fn a_wheel_scroll_closes_once_paused_even_while_a_button_goes_down() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.view.tool = crate::app::develop::state::Tool::Remove;
    editor
        .document
        .edit
        .setup_mut()
        .retouch
        .push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    editor.view.retouch.selected = Some(0);
    let edit = editor.begin_edit_frame();
    editor.scroll_tool_size(crate::app::develop::tools::brush_scroll::Scroll {
        lines: 1.,
        adjust: crate::app::develop::tools::brush_scroll::Adjust::Size,
        brush: crate::app::develop::tools::brush_scroll::MaskBrush::Current,
    });
    editor.finish_edit_frame(edit, &ctx);
    std::thread::sleep(std::time::Duration::from_millis(450));
    // The next frame has the button going down over the photo; nothing changes yet.
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: Pos2::new(5., 5.),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |_| {},
    );
    output.textures_delta.clear();
    let edit = editor.begin_edit_frame();
    editor.finish_edit_frame(edit, &ctx);
    assert!(!editor.document.edit.history().in_gesture());
    assert_eq!(editor.document.edit.history().steps().1, 1);
}

#[test]
fn a_click_after_a_wheel_scroll_closes_it_at_once() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.view.tool = crate::app::develop::state::Tool::Remove;
    editor
        .document
        .edit
        .setup_mut()
        .retouch
        .push(crate::model::retouch::RetouchOp {
            mode: crate::model::retouch::RetouchMode::Heal,
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    editor.view.retouch.selected = Some(0);
    let edit = editor.begin_edit_frame();
    editor.scroll_tool_size(crate::app::develop::tools::brush_scroll::Scroll {
        lines: 1.,
        adjust: crate::app::develop::tools::brush_scroll::Adjust::Size,
        brush: crate::app::develop::tools::brush_scroll::MaskBrush::Current,
    });
    editor.finish_edit_frame(edit, &ctx);
    // A click before the pause (on another spot, say) closes the scroll.
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: Pos2::new(5., 5.),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |_| {},
    );
    output.textures_delta.clear();
    let edit = editor.begin_edit_frame();
    editor.finish_edit_frame(edit, &ctx);
    assert!(!editor.document.edit.history().in_gesture());
    assert_eq!(editor.document.edit.history().steps().1, 1);
}
