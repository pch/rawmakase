//! Editor tests of `crop_tool`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;
use crate::camera_data::{CameraImage, Metadata};
use std::sync::Arc;

#[test]
fn crop_tool_reads_each_photos_own_aspect() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let image = Arc::new(CameraImage {
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
    });
    editor.document.set_image(image);
    let open = |editor: &mut Editor, crop: [f32; 4]| {
        editor.view.tool = crate::app::develop::state::Tool::None;
        editor.document.edit.setup_mut().crop = crop;
        editor.view.toggle(crate::app::develop::state::Tool::Crop);
        let frame = editor.begin_edit_frame();
        editor.finish_edit_frame(frame, &ctx);
        assert_eq!(
            editor.document.edit.recipe().crop,
            crop,
            "reading changes no crop"
        );
        editor.view.aspect
    };
    // Original is chosen (the default session), so each crop's aspect is locked as
    // read; with Free it would stay unlocked (`free_stays_unlocked_when_crop_reopens`).
    // The last photo's XPan crop does not carry over to an uncropped photo.
    editor.view.aspect = 65. / 24.;
    assert_eq!(open(&mut editor, [0., 0., 1., 1.]), -1.);
    // 300 × 111 is 65 x 24 to within rounding.
    let xpan = [0., 0.223, 1., 0.777];
    assert_eq!(open(&mut editor, xpan), 65. / 24.);
    let custom = open(&mut editor, [0., 0., 0.5, 1.]);
    assert!((custom - 0.75).abs() < 1e-6);
    assert_eq!(open(&mut editor, [0.1, 0.1, 0.9, 0.9]), -1.);
}

#[test]
fn crop_tool_stays_open_from_photo_to_photo() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session {
            crop_aspect: "1x1".into(),
            ..Default::default()
        },
        None,
    );
    editor.module = Module::Develop;
    let image = Arc::new(CameraImage {
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
    });
    // Another photo, opened while Crop is: the tool stays and reads it as if
    // opened on it.
    let next = |editor: &mut Editor, crop: [f32; 4]| {
        editor.open_raw("missing-crop-fixture.ARW".into(), None);
        assert!(editor.view.is(crate::app::develop::state::Tool::Crop));
        editor.document.set_image(image.clone());
        editor.document.edit.setup_mut().crop = crop;
        let frame = editor.begin_edit_frame();
        editor.finish_edit_frame(frame, &ctx);
        editor.view.aspect
    };
    let crop = |editor: &Editor| editor.document.edit.recipe().crop;
    let steps = |editor: &Editor| editor.document.edit.history().steps().0.len();
    editor.document.set_image(image.clone());
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    // A cropped photo keeps its crop and shows its own aspect: 300 × 111 is
    // 65 x 24 to within rounding.
    let xpan = [0., 0.223, 1., 0.777];
    assert_eq!(next(&mut editor, xpan), 65. / 24.);
    assert_eq!(crop(&editor), xpan);
    assert!(!editor.document.edit.save_state().needs_save());
    // An uncropped photo takes the chosen 1 x 1 as one undoable step.
    let full = [0., 0., 1., 1.];
    let before = steps(&editor);
    assert_eq!(next(&mut editor, full), 1.);
    let c = crop(&editor);
    assert!(((c[2] - c[0]) * 300. / ((c[3] - c[1]) * 200.) - 1.).abs() < 1e-3);
    assert_eq!(steps(&editor), before + 1);
    assert!(editor.document.edit.save_state().needs_save());
    assert!(editor.document.edit.undo());
    assert_eq!(crop(&editor), full);
    // The same photo loaded again (after a Sync's Undo, say) reads the edit it
    // loads: its own crop's aspect, and an uncropped one stays uncropped.
    let path = std::path::PathBuf::from("missing-crop-fixture.ARW");
    let reload = |editor: &mut Editor, crop: [f32; 4]| {
        editor.document.opened_at(path.clone());
        editor.open_raw(path.clone(), None);
        assert!(editor.view.is(crate::app::develop::state::Tool::Crop));
        editor.document.set_image(image.clone());
        editor.document.edit.setup_mut().crop = crop;
        let frame = editor.begin_edit_frame();
        editor.finish_edit_frame(frame, &ctx);
        assert_eq!(editor.document.edit.recipe().crop, crop);
        assert_eq!(editor.document.edit.history().steps().0.len(), 0);
        assert!(!editor.document.edit.save_state().needs_save());
        editor.view.aspect
    };
    assert_eq!(reload(&mut editor, xpan), 65. / 24.);
    assert_eq!(reload(&mut editor, full), -1.);
    // Removing the photo closes it.
    editor.close_removed_document();
    assert!(editor.view.is(crate::app::develop::state::Tool::None));
}

#[test]
fn choosing_the_photos_own_aspect_leaves_an_uncropped_photo_alone() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    // 100 × 99: the photo's ratio over itself is not exactly 1 in f32.
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 100,
        height: 99,
        pixels: vec![[0.1; 3]; 9900],
        metadata: Metadata {
            width: 100,
            height: 99,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }));
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    let frame = editor.begin_edit_frame();
    editor.finish_edit_frame(frame, &ctx);
    let steps = editor.document.edit.history().steps().0.len();
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(-1.);
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(editor.document.edit.recipe().crop, [0., 0., 1., 1.]);
    assert_eq!(editor.document.edit.history().steps().0.len(), steps);
}

#[test]
fn fitting_a_crop_dragged_to_its_smallest_keeps_it_valid() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.document.set_image(Arc::new(CameraImage {
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
    editor.view.tool = crate::app::develop::state::Tool::Crop;
    editor.view.aspect = -1.;
    // A handle dragged to the smallest crop: 0.11 - 0.1 is just under 0.01 in f32,
    // though the crop is already at the photo's aspect.
    let frame = editor.begin_edit_frame();
    editor.document.edit.recipe_mut().crop = [0.1, 0.1, 0.1 + 0.01, 0.1 + 0.01];
    editor.fit_aspect();
    editor.finish_edit_frame(frame, &ctx);
    editor.document.edit.recipe().validate().unwrap();
}

#[test]
fn the_chosen_aspect_is_its_own_edit_before_the_next_operation() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session {
            crop_aspect: "1x1".into(),
            ..Default::default()
        },
        None,
    );
    editor.document.set_image(Arc::new(CameraImage {
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
    editor.document.edit.change(None, |r| r.exposure = 1.25);
    editor.sync_command_revision();
    let revision = editor.automation.revision;
    let steps = editor.document.edit.history().steps().0.len();
    // A queued command opens Crop; the next command's frame (a Navigate, say, which
    // drops its frame with the photo) finds the photo uncropped.
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    let frame = editor.begin_edit_frame();
    drop(frame);
    // The crop is already recorded and to be saved, and an Undo takes back the crop
    // rather than the edit before it.
    let c = editor.document.edit.recipe().crop;
    assert!(((c[2] - c[0]) * 300. / ((c[3] - c[1]) * 200.) - 1.).abs() < 1e-3);
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    assert!(editor.document.edit.save_state().needs_save());
    // Commands see the cropped settings as a revision of their own, before the
    // frame's command reads them.
    assert_eq!(editor.automation.revision, revision + 1);
    editor.sync_command_revision();
    assert_eq!(editor.automation.revision, revision + 1);
    assert!(editor.document.edit.undo());
    assert_eq!(editor.document.edit.recipe().crop, [0., 0., 1., 1.]);
    assert_eq!(editor.document.edit.recipe().exposure, 1.25);
}

#[test]
fn the_chosen_aspect_ends_a_held_slider_drag_first() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session {
            crop_aspect: "1x1".into(),
            ..Default::default()
        },
        None,
    );
    editor.document.set_image(Arc::new(CameraImage {
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
    // Exposure dragged to +1.25, the button still held as R opens Crop.
    let frame = editor.document.edit.begin();
    editor.document.edit.recipe_mut().exposure = 1.25;
    editor
        .document
        .edit
        .finish(frame, crate::edit_session::Gesture::Held);
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    let frame = editor.begin_edit_frame();
    editor.finish_edit_frame(frame, &ctx);
    editor.document.edit.finish_gesture();
    // The drag and the crop are a step each: Undo takes back the crop alone, then
    // the exposure.
    assert!(editor.document.edit.undo());
    assert_eq!(editor.document.edit.recipe().crop, [0., 0., 1., 1.]);
    assert_eq!(editor.document.edit.recipe().exposure, 1.25);
    assert!(editor.document.edit.undo());
    assert_eq!(editor.document.edit.recipe().exposure, 0.);
}

#[test]
fn crop_tool_keeps_the_chosen_aspect_for_uncropped_photos() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let session = dir.path().join("session.json");
    let ctx = egui::Context::default();
    let image = Arc::new(CameraImage {
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
    });
    let new_editor = |s: crate::app::preferences::session::Session| {
        let mut editor = Editor::with_context(&ctx, None, s, None);
        editor.session.set_file(Some(session.clone()));
        editor.document.set_image(image.clone());
        editor
    };
    let open = |editor: &mut Editor, crop: [f32; 4]| {
        editor.view.tool = crate::app::develop::state::Tool::None;
        editor.document.edit.setup_mut().crop = crop;
        editor.view.toggle(crate::app::develop::state::Tool::Crop);
        let frame = editor.begin_edit_frame();
        editor.finish_edit_frame(frame, &ctx);
        editor.view.aspect
    };
    let crop = |editor: &Editor| editor.document.edit.recipe().crop;
    // The crop's width over its height on the 300 × 200 photo.
    let ratio = |editor: &Editor| {
        let c = crop(editor);
        (c[2] - c[0]) * 300. / ((c[3] - c[1]) * 200.)
    };
    let steps = |editor: &Editor| editor.document.edit.history().steps().0.len();
    let mut editor = new_editor(Default::default());
    let full = [0., 0., 1., 1.];
    // Until one is chosen an uncropped photo shows Original, and stays uncropped.
    let before = steps(&editor);
    assert_eq!(open(&mut editor, full), -1.);
    assert_eq!(crop(&editor), full);
    assert_eq!(steps(&editor), before);
    // 4 x 5 chosen from the menu constrains this photo's crop...
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(1.25);
    editor.finish_edit_frame(frame, &ctx);
    assert!((ratio(&editor) - 1.25).abs() < 1e-3);
    // ...and the next uncropped photo takes it as the tool opens, as one undoable step.
    let before = steps(&editor);
    assert_eq!(open(&mut editor, full), 1.25);
    assert!((ratio(&editor) - 1.25).abs() < 1e-3);
    assert_eq!(steps(&editor), before + 1);
    assert!(editor.document.edit.undo());
    assert_eq!(crop(&editor), full);
    // Choosing it again from the menu crops that photo again.
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(1.25);
    editor.finish_edit_frame(frame, &ctx);
    assert!((ratio(&editor) - 1.25).abs() < 1e-3);
    // A cropped photo keeps its crop and shows its own aspect, and does not replace
    // the choice.
    let xpan = [0., 0.223, 1., 0.777];
    assert_eq!(open(&mut editor, xpan), 65. / 24.);
    assert_eq!(crop(&editor), xpan);
    assert_eq!(open(&mut editor, full), 1.25);
    // Swapping a photo's crop to portrait leaves the choice following each photo.
    let frame = editor.begin_edit_frame();
    editor.swap_crop_orientation();
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(open(&mut editor, full), 1.25);
    assert!((ratio(&editor) - 1.25).abs() < 1e-3);
    // Free is a choice too, and leaves an uncropped photo uncropped.
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(0.);
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(open(&mut editor, full), 0.);
    assert_eq!(crop(&editor), full);
    // The choice is kept in the session for the next launch.
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(16. / 9.);
    editor.finish_edit_frame(frame, &ctx);
    let saved: crate::app::preferences::session::Session =
        serde_json::from_slice(&std::fs::read(&session)?)?;
    assert_eq!(saved.crop_aspect, "16x9");
    let mut relaunched = new_editor(saved);
    assert_eq!(open(&mut relaunched, full), 16. / 9.);
    assert!((ratio(&relaunched) - 16. / 9.).abs() < 1e-3);
    // A name this version does not know reads as Original.
    let mut unknown = new_editor(crate::app::preferences::session::Session {
        crop_aspect: "golden".into(),
        ..Default::default()
    });
    assert_eq!(open(&mut unknown, full), -1.);
    Ok(())
}

#[test]
fn crop_keys_swap_and_cycle_the_overlay_but_not_while_typing() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let session = d.path().join("session.json");
    e.session.set_file(Some(session.clone()));
    e.module = Module::Develop;
    e.document.catalog_photo = Some(ids[0]);
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
    let crop = [0.1, 0.2, 0.5, 0.6];
    e.document.edit.setup_mut().crop = crop;
    e.view.toggle(crate::app::develop::state::Tool::Crop);
    let ctx = e.context.clone();
    let press = |e: &mut Editor, key: egui::Key, shift: bool, typing: bool| {
        let modifiers = egui::Modifiers {
            shift,
            ..Default::default()
        };
        let mut text = String::new();
        // A text field takes focus in one frame and has it for the keys in the next.
        for events in [vec![], vec![true, false]] {
            let input = egui::RawInput {
                events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                    .chain(events.into_iter().map(|pressed| egui::Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed,
                        repeat: false,
                        modifiers,
                    }))
                    .collect(),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                if typing {
                    ui.text_edit_singleline(&mut text).request_focus();
                }
                e.metadata_shortcuts(&ctx);
                let frame = e.begin_edit_frame();
                e.develop_shortcuts(&ctx);
                e.finish_edit_frame(frame, &ctx);
            });
            output.textures_delta.clear();
        }
    };
    // Typing an X or an O in a field changes nothing.
    press(&mut e, egui::Key::X, false, true);
    press(&mut e, egui::Key::O, false, true);
    assert_eq!(e.document.edit.recipe().crop, crop);
    assert_eq!(e.view.crop_guides, Default::default());
    // X swaps the 120 × 80 crop to 80 × 120 about its centre, and rejects nothing.
    press(&mut e, egui::Key::X, false, false);
    let c = e.document.edit.recipe().crop;
    assert!(
        ((c[2] - c[0]) * 300. - 80.).abs() < 1e-3 && ((c[3] - c[1]) * 200. - 120.).abs() < 1e-3,
        "{c:?}"
    );
    assert!(((c[0] + c[2]) / 2. - 0.3).abs() < 1e-6 && ((c[1] + c[3]) / 2. - 0.4).abs() < 1e-6);
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag, 0);
    // O cycles the overlay and Shift+O turns it; both are kept in the session.
    use crate::app::develop::tools::crop_tool::Guide;
    press(&mut e, egui::Key::O, false, false);
    assert_eq!(e.view.crop_guides.guide, Guide::Diagonal);
    press(&mut e, egui::Key::O, false, false);
    press(&mut e, egui::Key::O, true, false);
    assert_eq!(
        (e.view.crop_guides.guide, e.view.crop_guides.orientation),
        (Guide::Triangle, 1)
    );
    // Shift released before the frame is drawn: the press still had it.
    let shifted = egui::Modifiers {
        shift: true,
        ..Default::default()
    };
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(shifted),
                egui::Event::Key {
                    key: egui::Key::O,
                    physical_key: Some(egui::Key::O),
                    pressed: true,
                    repeat: false,
                    modifiers: shifted,
                },
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            ],
            ..Default::default()
        },
        |_| e.develop_shortcuts(&ctx),
    );
    output.textures_delta.clear();
    assert_eq!(
        (e.view.crop_guides.guide, e.view.crop_guides.orientation),
        (Guide::Triangle, 0)
    );
    press(&mut e, egui::Key::O, true, false);
    let saved: crate::app::preferences::session::Session =
        serde_json::from_slice(&std::fs::read(&session)?)?;
    assert_eq!(saved.crop_guides.guide, "triangle");
    assert_eq!(saved.crop_guides.orientation, 1);
    // Outside the Crop tool X rejects as before.
    e.view.tool = crate::app::develop::state::Tool::None;
    press(&mut e, egui::Key::X, false, false);
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag, -1);
    Ok(())
}

/// A 300 × 200 photo with a preview to draw, for crop drags on the viewport.
fn editor_for_crop_drags(ctx: &egui::Context, crop_aspect: &str) -> Editor {
    let mut editor = Editor::with_context(
        ctx,
        None,
        crate::app::preferences::session::Session {
            crop_aspect: crop_aspect.into(),
            ..Default::default()
        },
        None,
    );
    editor.document.set_image(Arc::new(CameraImage {
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
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([300, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor
}

/// A frame of Develop's viewport given `events`, as the window draws it.
fn viewport_frame(ctx: &egui::Context, editor: &mut Editor, events: Vec<egui::Event>) {
    viewport_frame_with(ctx, editor, events, egui::Modifiers::NONE);
}

/// A frame of Develop's viewport given `events`, with `modifiers` held.
fn viewport_frame_with(
    ctx: &egui::Context,
    editor: &mut Editor,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) {
    let mut events = events;
    events.insert(0, egui::Event::ModifiersChanged(modifiers));
    let frame = editor.begin_edit_frame();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::splat(400.),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            editor.viewport_ui(ui);
        },
    );
    output.textures_delta.clear();
    editor.finish_edit_frame(frame, ctx);
}

/// Drags the crop's `handle` (as the viewport numbers them: 5 is the right side, 7 the
/// bottom) by `by`, a fraction of the photo's width and height, with the pointer.
fn drag_crop_handle(ctx: &egui::Context, editor: &mut Editor, handle: usize, by: egui::Vec2) {
    let handle = match handle {
        5 => (1, 0),
        7 => (0, 1),
        _ => unreachable!("not used here"),
    };
    drag_crop(ctx, editor, handle, &[(by, egui::Modifiers::NONE)]);
}

/// Presses the crop's handle `(x, y)` (−1 left or top, 0 the middle, 1 right or bottom)
/// and moves the pointer through `path`, offsets from the press as fractions of the
/// photo's width and height, a frame each, with each one's modifiers held; then releases
/// it where it ends, with the last modifiers still held.
fn drag_crop(
    ctx: &egui::Context,
    editor: &mut Editor,
    (x, y): (i8, i8),
    path: &[(egui::Vec2, egui::Modifiers)],
) {
    // A frame first, so the photo's place on screen is known.
    viewport_frame(ctx, editor, vec![]);
    let rect = editor.view.shown_rect.expect("the photo is drawn");
    let c = editor.document.edit.recipe().crop;
    let side = |s: i8, min: f32, max: f32| match s {
        -1 => min,
        1 => max,
        _ => (min + max) / 2.,
    };
    let from = egui::Pos2::new(
        rect.left() + side(x, c[0], c[2]) * rect.width(),
        rect.top() + side(y, c[1], c[3]) * rect.height(),
    );
    let first = path.first().map_or(egui::Modifiers::NONE, |p| p.1);
    let button = |pos, pressed, modifiers| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    viewport_frame_with(
        ctx,
        editor,
        vec![egui::Event::PointerMoved(from), button(from, true, first)],
        first,
    );
    // A few points first, as a hand moves: the drag starts by the handle it pressed.
    let to = from + path[0].0 * rect.size();
    let step = from + (to - from).normalized() * 8.;
    viewport_frame_with(ctx, editor, vec![egui::Event::PointerMoved(step)], first);
    let mut last = (from, first);
    for &(by, modifiers) in path {
        let to = from + by * rect.size();
        viewport_frame_with(ctx, editor, vec![egui::Event::PointerMoved(to)], modifiers);
        last = (to, modifiers);
    }
    viewport_frame_with(ctx, editor, vec![button(last.0, false, last.1)], last.1);
}

/// Closes and opens the Crop tool again, as R pressed twice does.
fn reopen_crop(ctx: &egui::Context, editor: &mut Editor) {
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    viewport_frame(ctx, editor, vec![]);
}

fn assert_near(a: f32, b: f32, what: &str) {
    assert!((a - b).abs() < 0.01, "{what}: {a} is not {b}");
}

#[test]
fn free_stays_unlocked_when_crop_reopens() {
    let ctx = egui::Context::default();
    let mut editor = editor_for_crop_drags(&ctx, "free");
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    // The right side dragged in: the crop is 225 × 200, a Custom ratio.
    drag_crop_handle(&ctx, &mut editor, 5, egui::vec2(-0.25, 0.));
    assert_near(editor.document.edit.recipe().crop[2], 0.75, "right");
    // Lightroom keeps its padlock open through R pressed twice (measured), so the
    // reopened tool still lets the next drag change the ratio.
    reopen_crop(&ctx, &mut editor);
    assert_eq!(editor.crop_constraint(), None);
    assert_near(
        editor.document.edit.recipe().crop[2],
        0.75,
        "right on reopening",
    );
    drag_crop_handle(&ctx, &mut editor, 7, egui::vec2(0., -0.2));
    let c = editor.document.edit.recipe().crop;
    assert_near(c[2] - c[0], 0.75, "width after the bottom is dragged");
    assert_near(c[3] - c[1], 0.8, "height after the bottom is dragged");
}

#[test]
fn a_chosen_ratio_still_locks_a_reopened_custom_crop() {
    let ctx = egui::Context::default();
    let mut editor = editor_for_crop_drags(&ctx, "4x5");
    editor.document.edit.setup_mut().crop = [0., 0., 0.75, 1.];
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    viewport_frame(&ctx, &mut editor, vec![]);
    // Locked, the photo's own Custom ratio is kept: 0.75 of the width over the
    // whole height, in the crop's 0–1 coordinates.
    let ratio = editor.crop_constraint().expect("locked");
    assert_near(ratio, 0.75, "the crop's ratio");
    drag_crop_handle(&ctx, &mut editor, 7, egui::vec2(0., -0.2));
    let c = editor.document.edit.recipe().crop;
    assert_near((c[2] - c[0]) / (c[3] - c[1]), 0.75, "ratio after the drag");
}

#[test]
fn free_drags_stay_free_while_crop_stays_open() {
    let ctx = egui::Context::default();
    let mut editor = editor_for_crop_drags(&ctx, "free");
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    drag_crop_handle(&ctx, &mut editor, 5, egui::vec2(-0.25, 0.));
    drag_crop_handle(&ctx, &mut editor, 7, egui::vec2(0., -0.2));
    drag_crop_handle(&ctx, &mut editor, 5, egui::vec2(-0.25, 0.));
    let c = editor.document.edit.recipe().crop;
    assert_near(c[2] - c[0], 0.5, "width");
    assert_near(c[3] - c[1], 0.8, "height");
    assert_eq!(editor.crop_constraint(), None);
}

#[test]
fn free_stays_unlocked_from_photo_to_photo_and_on_reload() {
    let ctx = egui::Context::default();
    let mut editor = editor_for_crop_drags(&ctx, "free");
    editor.module = Module::Develop;
    let image = editor.document.full().cloned().unwrap();
    let custom = [0., 0., 0.75, 1.];
    let full = [0., 0., 1., 1.];
    let path = std::path::PathBuf::from("missing-crop-fixture.ARW");
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    // Another photo with a Custom crop, then the same photo loaded again, cropped and
    // uncropped: each reads unlocked, and reading makes no edit.
    for (again, crop) in [(false, custom), (true, custom), (true, full)] {
        if again {
            editor.document.opened_at(path.clone());
        }
        editor.open_raw(path.clone(), None);
        assert!(editor.view.is(crate::app::develop::state::Tool::Crop));
        editor.document.set_image(image.clone());
        editor.document.edit.setup_mut().crop = crop;
        let frame = editor.begin_edit_frame();
        editor.finish_edit_frame(frame, &ctx);
        assert_eq!(
            editor.crop_constraint(),
            None,
            "reloaded: {again}, {crop:?}"
        );
        assert_eq!(editor.document.edit.recipe().crop, crop);
        assert_eq!(editor.document.edit.history().steps().0.len(), 0);
        assert!(!editor.document.edit.save_state().needs_save());
    }
}

#[test]
fn free_stays_unlocked_after_a_relaunch() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let session = dir.path().join("session.json");
    let ctx = egui::Context::default();
    let mut editor = editor_for_crop_drags(&ctx, "1x1");
    editor.session.set_file(Some(session.clone()));
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    viewport_frame(&ctx, &mut editor, vec![]);
    let frame = editor.begin_edit_frame();
    editor.choose_aspect(0.);
    editor.finish_edit_frame(frame, &ctx);
    let saved: crate::app::preferences::session::Session =
        serde_json::from_slice(&std::fs::read(&session)?)?;
    let mut relaunched = editor_for_crop_drags(&ctx, &saved.crop_aspect);
    relaunched.document.edit.setup_mut().crop = [0., 0., 0.75, 1.];
    relaunched
        .view
        .toggle(crate::app::develop::state::Tool::Crop);
    viewport_frame(&ctx, &mut relaunched, vec![]);
    assert_eq!(relaunched.crop_constraint(), None);
    assert_eq!(relaunched.document.edit.recipe().crop, [0., 0., 0.75, 1.]);
    Ok(())
}

#[test]
fn a_and_the_padlock_lock_the_crop_at_its_ratio_and_unlock_it() {
    let ctx = egui::Context::default();
    let mut editor = editor_for_crop_drags(&ctx, "free");
    editor.document.edit.setup_mut().crop = [0., 0., 0.75, 1.];
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    viewport_frame(&ctx, &mut editor, vec![]);
    let steps = editor.document.edit.history().steps().0.len();
    let press_a = |editor: &mut Editor| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: Some(egui::Key::A),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |_| {
                let frame = editor.begin_edit_frame();
                editor.develop_shortcuts(&ctx);
                editor.finish_edit_frame(frame, &ctx);
            },
        );
        output.textures_delta.clear();
    };
    // Locking a Custom crop keeps that ratio on this photo, with Original the choice
    // an uncropped photo opens with; the crop itself is not changed.
    press_a(&mut editor);
    assert_near(
        editor.crop_constraint().expect("locked"),
        0.75,
        "locked ratio",
    );
    assert_eq!(editor.view.chosen_aspect, -1.);
    assert_eq!(editor.document.edit.recipe().crop, [0., 0., 0.75, 1.]);
    assert_eq!(editor.document.edit.history().steps().0.len(), steps);
    drag_crop_handle(&ctx, &mut editor, 7, egui::vec2(0., -0.2));
    let c = editor.document.edit.recipe().crop;
    assert_near((c[2] - c[0]) / (c[3] - c[1]), 0.75, "ratio after the drag");
    // A again unlocks, as Free does.
    press_a(&mut editor);
    assert_eq!(editor.crop_constraint(), None);
    assert_eq!(editor.view.chosen_aspect, 0.);
    // A crop at a preset, either way round, locks to that preset as the menu would.
    // 150 × 200 is 3 x 4 turned (the photo is landscape).
    editor
        .document
        .edit
        .change(None, |r| r.crop = [0., 0., 0.5, 1.]);
    press_a(&mut editor);
    assert_eq!(editor.view.chosen_aspect, 4. / 3.);
    assert_eq!(editor.view.aspect, 3. / 4.);
}

const ALT: egui::Modifiers = egui::Modifiers::ALT;
const PLAIN: egui::Modifiers = egui::Modifiers::NONE;

/// A 300 × 200 photo cropped to `crop`, with the Crop tool open at `crop_aspect`.
fn editor_with_crop(ctx: &egui::Context, crop_aspect: &str, crop: [f32; 4]) -> Editor {
    let mut editor = editor_for_crop_drags(ctx, crop_aspect);
    editor.document.edit.setup_mut().crop = crop;
    editor.view.toggle(crate::app::develop::state::Tool::Crop);
    viewport_frame(ctx, &mut editor, vec![]);
    editor
}

fn crop_centre(c: [f32; 4]) -> (f32, f32) {
    ((c[0] + c[2]) / 2., (c[1] + c[3]) / 2.)
}

#[test]
fn alt_drags_resize_a_free_crop_about_its_centre_from_every_handle() {
    let ctx = egui::Context::default();
    for (x, y) in [
        (-1, -1),
        (1, -1),
        (1, 1),
        (-1, 1),
        (-1, 0),
        (1, 0),
        (0, -1),
        (0, 1),
    ] {
        let mut editor = editor_with_crop(&ctx, "free", [0.3, 0.3, 0.7, 0.7]);
        let out = egui::vec2(f32::from(x), f32::from(y)) * 0.1;
        drag_crop(&ctx, &mut editor, (x, y), &[(out, ALT)]);
        let c = editor.document.edit.recipe().crop;
        let what = format!("handle {x}, {y}: {c:?}");
        let (cx, cy) = crop_centre(c);
        assert_near(cx, 0.5, &what);
        assert_near(cy, 0.5, &what);
        assert_near(c[2] - c[0], if x == 0 { 0.4 } else { 0.6 }, &what);
        assert_near(c[3] - c[1], if y == 0 { 0.4 } else { 0.6 }, &what);
    }
}

#[test]
fn alt_drags_keep_a_locked_crops_ratio_and_centre() {
    let ctx = egui::Context::default();
    // Original locks the crop to the photo's own 3:2, which is 1:1 in 0–1 terms.
    for (x, y) in [(1, 1), (-1, 0), (0, 1)] {
        let mut editor = editor_with_crop(&ctx, "original", [0.3, 0.3, 0.7, 0.7]);
        let ratio = editor.crop_constraint().expect("locked");
        assert_near(ratio, 1., "Original in 0–1 terms");
        drag_crop(
            &ctx,
            &mut editor,
            (x, y),
            &[(egui::vec2(0.1, 0.1) * f32::from(x + y).signum(), ALT)],
        );
        let c = editor.document.edit.recipe().crop;
        let what = format!("handle {x}, {y}: {c:?}");
        let (cx, cy) = crop_centre(c);
        assert_near(cx, 0.5, &what);
        assert_near(cy, 0.5, &what);
        assert_near((c[2] - c[0]) / (c[3] - c[1]), 1., &what);
        assert!(c[2] - c[0] > 0.45, "{what}: grew");
    }
}

#[test]
fn alt_pressed_and_released_mid_drag_changes_how_the_rest_resizes() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_crop(&ctx, "free", [0.2, 0.3, 0.6, 0.7]);
    let steps = editor.document.edit.history().steps().0.len();
    // The right side out by 0.1 alone; Alt pressed with the pointer still, which moves
    // nothing; out 0.05 more with Alt, which moves the left side too; Alt released and
    // out 0.05 more, the right side alone again.
    drag_crop(
        &ctx,
        &mut editor,
        (1, 0),
        &[
            (egui::vec2(0.1, 0.), PLAIN),
            (egui::vec2(0.1, 0.), ALT),
            (egui::vec2(0.15, 0.), ALT),
            (egui::vec2(0.15, 0.), PLAIN),
            (egui::vec2(0.2, 0.), PLAIN),
        ],
    );
    let c = editor.document.edit.recipe().crop;
    assert_near(c[0], 0.15, "left");
    assert_near(c[2], 0.8, "right");
    assert_eq!([c[1], c[3]], [0.3, 0.7], "top and bottom untouched");
    // One gesture, one History step, whatever the modifiers did meanwhile.
    assert_eq!(editor.document.edit.history().steps().0.len(), steps + 1);
    editor.undo();
    assert_eq!(editor.document.edit.recipe().crop, [0.2, 0.3, 0.6, 0.7]);
}

#[test]
fn an_alt_drag_stops_at_the_photos_edge_and_at_the_smallest_crop() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_crop(&ctx, "free", [0.1, 0.3, 0.5, 0.7]);
    // Out past the left edge, then back in: the drag follows the pointer, not where it
    // stopped.
    drag_crop(
        &ctx,
        &mut editor,
        (1, 0),
        &[(egui::vec2(0.5, 0.), ALT), (egui::vec2(0.05, 0.), ALT)],
    );
    let c = editor.document.edit.recipe().crop;
    assert_near(c[0], 0.05, "left");
    assert_near(c[2], 0.55, "right");
    drag_crop(&ctx, &mut editor, (1, 0), &[(egui::vec2(0.5, 0.), ALT)]);
    let c = editor.document.edit.recipe().crop;
    assert_near(c[0], 0., "left at the edge");
    assert_near(c[2], 0.6, "right");
    // In across the centre: the smallest crop, still about it.
    drag_crop(&ctx, &mut editor, (-1, 0), &[(egui::vec2(0.8, 0.), ALT)]);
    let c = editor.document.edit.recipe().crop;
    assert_near(crop_centre(c).0, 0.3, "centre");
    editor.document.edit.recipe().validate().unwrap();
}

#[test]
fn a_quick_drag_takes_the_handle_it_was_pressed_on() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_crop(&ctx, "free", [0.2, 0.2, 0.8, 0.8]);
    viewport_frame(&ctx, &mut editor, vec![]);
    let rect = editor.view.shown_rect.expect("the photo is drawn");
    let from = egui::pos2(rect.left() + 0.8 * rect.width(), rect.center().y);
    // 40 points in the first frame of the drag, well past the handle's reach.
    let to = from - egui::vec2(40., 0.);
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: PLAIN,
    };
    viewport_frame(
        &ctx,
        &mut editor,
        vec![egui::Event::PointerMoved(from), button(from, true)],
    );
    viewport_frame(&ctx, &mut editor, vec![egui::Event::PointerMoved(to)]);
    viewport_frame(&ctx, &mut editor, vec![button(to, false)]);
    let c = editor.document.edit.recipe().crop;
    assert_near(c[0], 0.2, "left stays");
    assert!(c[2] < 0.75, "the right side moved in: {c:?}");
}
