//! How a History step gets its name: a control names the step it makes while it
//! is drawn, the edit frame passes that name on to History as it ends, and
//! History gives it to the next step it records. These pin down when a name is
//! kept, replaced or dropped, through the editor's frames.
use crate::app::Editor;
use crate::app::history::Step;
use eframe::egui::{self, Pos2};

fn editor(ctx: &egui::Context) -> Editor {
    Editor::with_context(
        ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    )
}
/// What a control does as it changes the edit: names the step being made.
fn name(editor: &mut Editor, _ctx: &egui::Context, name: &str, value: &str) {
    editor.step_name.name(name.into(), value.into());
}
/// The name a control gave in a frame not yet finished.
fn pending(editor: &Editor, _ctx: &egui::Context) -> Option<(String, String)> {
    editor.step_name.given()
}
/// The names of the steps History holds, oldest first.
fn steps(editor: &Editor) -> Vec<String> {
    let (steps, applied) = editor.document.edit.history().steps();
    steps[..applied].iter().map(|s| s.name.clone()).collect()
}
/// One frame given `events`, with `draw` running between the frame's start
/// and finish, as the Develop workspace does.
fn frame(
    editor: &mut Editor,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    draw: impl FnOnce(&mut Editor, &egui::Context),
) {
    let mut draw = Some(draw);
    let mut output = ctx.run_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ui| {
            let frame = editor.begin_edit_frame();
            if let Some(draw) = draw.take() {
                draw(editor, ui.ctx());
            }
            editor.finish_edit_frame(frame, ui.ctx());
        },
    );
    output.textures_delta.clear();
}
fn button(pressed: bool) -> Vec<egui::Event> {
    let at = Pos2::new(5., 5.);
    vec![
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

#[test]
fn the_last_name_given_in_a_frame_names_its_step() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    frame(&mut e, &ctx, Vec::new(), |e, ctx| {
        name(e, ctx, "Reset Basic", "");
        e.document.edit.recipe_mut().exposure = 0.5;
        name(e, ctx, "Exposure", "+0.50");
    });
    assert_eq!(steps(&e), ["Exposure"]);
    assert_eq!(pending(&e, &ctx), None, "taken as the frame finished");
}

#[test]
fn a_name_waits_through_frames_that_finish_no_edit_frame() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    // Given in a frame that never finishes an edit frame (the Library's, say).
    name(&mut e, &ctx, "Exposure", "+0.50");
    let mut started = e.begin_edit_frame();
    e.follow_edit_frame(&mut started);
    assert_eq!(pending(&e, &ctx), Some(("Exposure".into(), "+0.50".into())));
    e.document.edit.recipe_mut().exposure = 0.5;
    e.finish_edit_frame(started, &ctx);
    assert_eq!(steps(&e), ["Exposure"]);
}

#[test]
fn a_frame_left_for_another_photo_drops_its_name_as_it_finishes() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    let started = e.begin_edit_frame();
    e.document.edit.recipe_mut().exposure = 0.5;
    name(&mut e, &ctx, "Exposure", "+0.50");
    // Navigation mid-frame: the frame's photo is gone, and so is its name.
    e.open_raw(std::path::PathBuf::from("missing-step-name.ARW"), None);
    e.finish_edit_frame(started, &ctx);
    assert_eq!(pending(&e, &ctx), None);
    frame(&mut e, &ctx, Vec::new(), |e, _| {
        e.document.edit.recipe_mut().contrast = 0.2;
    });
    assert_eq!(steps(&e).len(), 1);
    assert_ne!(steps(&e), ["Exposure"]);
}

#[test]
fn a_frame_restarted_on_another_photo_keeps_the_name_given_before() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    let mut started = e.begin_edit_frame();
    name(&mut e, &ctx, "Exposure", "+0.50");
    e.open_raw(std::path::PathBuf::from("missing-step-name.ARW"), None);
    // The rest of the frame edits the photo now open, and the name stays given.
    e.follow_edit_frame(&mut started);
    e.document.edit.recipe_mut().contrast = 0.2;
    e.finish_edit_frame(started, &ctx);
    assert_eq!(steps(&e), ["Exposure"]);
}

#[test]
fn a_name_given_in_the_frame_outranks_one_history_was_given_directly() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    // The control's name, then History's own: the frame's is passed on last.
    frame(&mut e, &ctx, Vec::new(), |e, ctx| {
        e.document.edit.recipe_mut().exposure = 0.5;
        name(e, ctx, "Exposure", "+0.50");
        e.document.edit.name_next_step(Step::new("Direct", ""));
    });
    // History's own, with no name given in the frame.
    frame(&mut e, &ctx, Vec::new(), |e, _| {
        e.document.edit.recipe_mut().exposure = 0.7;
        e.document.edit.name_next_step(Step::new("Direct", ""));
    });
    assert_eq!(steps(&e), ["Exposure", "Direct"]);
}

#[test]
fn a_name_for_a_frame_that_changed_nothing_names_the_next_unnamed_edit() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    // A Reset of a section already at its defaults: no step, the name kept.
    frame(&mut e, &ctx, Vec::new(), |e, ctx| {
        name(e, ctx, "Reset Basic", "");
    });
    assert!(steps(&e).is_empty());
    frame(&mut e, &ctx, Vec::new(), |e, _| {
        e.document.edit.recipe_mut().exposure = 0.5;
    });
    assert_eq!(steps(&e), ["Reset Basic"]);
}

#[test]
fn a_gesture_that_ends_where_it_began_uses_up_the_name() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    frame(&mut e, &ctx, Vec::new(), |e, ctx| {
        name(e, ctx, "Reset Basic", "");
    });
    // A drag away and back: its step changes nothing and is not kept, but it
    // takes the name.
    frame(&mut e, &ctx, button(true), |e, _| {
        e.document.edit.recipe_mut().exposure = 0.5;
    });
    assert!(e.document.edit.history().in_gesture());
    frame(&mut e, &ctx, button(false), |e, _| {
        e.document.edit.recipe_mut().exposure = 0.;
    });
    assert!(!e.document.edit.history().in_gesture());
    assert!(steps(&e).is_empty());
    frame(&mut e, &ctx, Vec::new(), |e, _| {
        e.document.edit.recipe_mut().exposure = 0.5;
    });
    assert_eq!(steps(&e).len(), 1);
    assert_ne!(steps(&e), ["Reset Basic"]);
}

#[test]
fn a_drag_is_one_step_named_by_the_last_name_given_while_it_was_held() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    frame(&mut e, &ctx, button(true), |e, ctx| {
        e.document.edit.recipe_mut().exposure = 0.3;
        name(e, ctx, "Exposure", "+0.30");
    });
    frame(&mut e, &ctx, button(true), |e, ctx| {
        e.document.edit.recipe_mut().exposure = 0.5;
        name(e, ctx, "Exposure", "+0.50");
    });
    frame(&mut e, &ctx, button(false), |_, _| {});
    let (all, applied) = e.document.edit.history().steps();
    assert_eq!(applied, 1);
    assert_eq!(
        (all[0].name.as_str(), all[0].value.as_str()),
        ("Exposure", "+0.50")
    );
}

#[test]
fn undo_takes_no_name_and_leaves_the_one_given_for_the_next_edit() {
    let ctx = egui::Context::default();
    let mut e = editor(&ctx);
    frame(&mut e, &ctx, Vec::new(), |e, ctx| {
        e.document.edit.recipe_mut().exposure = 0.5;
        name(e, ctx, "Exposure", "+0.50");
    });
    // A name given in the frame Undo runs in is History's for the next step.
    frame(&mut e, &ctx, Vec::new(), |e, ctx| {
        e.undo();
        name(e, ctx, "Contrast", "+20");
    });
    assert!(steps(&e).is_empty());
    frame(&mut e, &ctx, Vec::new(), |e, _| {
        e.document.edit.recipe_mut().whites = 0.1;
    });
    assert_eq!(steps(&e), ["Contrast"]);
}

/// An editor for a photo with `presets` (each with an Amount), named and set
/// as XMP gives them.
fn preset_editor(ctx: &egui::Context, presets: &[(&str, &str)]) -> Editor {
    let mut e = editor(ctx);
    e.document.metadata = Some(crate::camera_data::Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    let preset = |(name, settings): &(&str, &str)| {
        let text = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal" crs:HasSettings="True" crs:SupportsAmount="True" {settings}><crs:Name><rdf:Alt><rdf:li xml:lang="x-default">{name}</rdf:li></rdf:Alt></crs:Name></rdf:Description></rdf:RDF></x:xmpmeta>"#
        );
        crate::xmp::parse(std::path::Path::new(&format!("{name}.xmp")), &text).unwrap()
    };
    e.presets.library = std::sync::Arc::new(crate::presets::Library {
        presets: presets.iter().map(preset).collect(),
        errors: Vec::new(),
    });
    e
}

#[test]
fn the_preset_amount_drops_its_name_when_another_edit_ended_it() {
    let ctx = egui::Context::default();
    let mut e = preset_editor(&ctx, &[("Bright", r#"crs:Exposure2012="+1.00""#)]);
    frame(&mut e, &ctx, Vec::new(), |e, _| {
        e.apply_preset(0).unwrap();
    });
    assert!(e.presets.amount.is_some());
    // Another edit earlier in the frame ends the Amount: the slider's name goes.
    frame(&mut e, &ctx, Vec::new(), |e, ctx| {
        e.document.edit.recipe_mut().contrast = 0.2;
        name(e, ctx, "Preset Amount", "50");
        e.set_preset_amount(0.5);
        assert_eq!(pending(e, ctx), None);
    });
    assert!(e.presets.amount.is_none());
    assert_ne!(steps(&e).last().map(String::as_str), Some("Preset Amount"));
}

#[test]
fn the_preset_amount_drops_its_name_when_it_changes_nothing() {
    let ctx = egui::Context::default();
    let mut e = preset_editor(&ctx, &[("Bright", r#"crs:Exposure2012="+1.00""#)]);
    e.apply_preset(0).unwrap();
    assert!(e.presets.amount.is_some());
    let before = steps(&e);
    frame(&mut e, &ctx, Vec::new(), |e, ctx| {
        // The Amount it is at already: the photo stays as it is.
        name(e, ctx, "Preset Amount", "100");
        assert_eq!(
            e.set_preset_amount(1.),
            crate::app::presets::AmountChange::Same
        );
        assert_eq!(pending(e, ctx), None);
    });
    assert_eq!(steps(&e), before);
    frame(&mut e, &ctx, Vec::new(), |e, _| {
        e.document.edit.recipe_mut().whites = 0.1;
    });
    assert_ne!(steps(&e).last().map(String::as_str), Some("Preset Amount"));
}
