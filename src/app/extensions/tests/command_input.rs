//! Commands that take viewer input, with the example extension's `pick-*`
//! commands, which answer with the input they were sent and their target's
//! photos. One program per test.
use super::*;
use crate::app::develop::state::Tool;
use rawmakase_extensions::protocol::viewer::{Bounds, Click, MAX_CLICKS, ViewerInput};

/// Starts command `command` collecting on the photo open in Develop.
fn begin(e: &mut Editor, command: &str) {
    e.run_extension_command(&format!("{ID}/{command}"), RunBy::Button)
        .unwrap();
    assert_eq!(
        e.command_input().map(|c| c.id()),
        Some(format!("{ID}/{command}"))
    );
}

/// The results of the commands titled `title` among `outcomes`.
fn results(outcomes: &[Outcome], title: &str) -> Vec<Result<Value, RemoteError>> {
    outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Command {
                title: t, result, ..
            } if t == title => Some(result.clone()),
            _ => None,
        })
        .collect()
}

/// Waits for command `title`'s result, then polls a little more, so a second
/// run would show: its one result, and every outcome.
fn once(e: &mut Editor, title: &str) -> (Value, Vec<Outcome>) {
    let mut all = pump(e, title, |_, o| result(o, title).is_some());
    let ctx = e.context.clone();
    let until = Instant::now() + Duration::from_millis(100);
    while Instant::now() < until {
        e.events(&ctx);
        all.extend(e.extensions_poll(Instant::now()).0);
        std::thread::sleep(Duration::from_millis(5));
    }
    let sent = results(&all, title);
    assert_eq!(sent.len(), 1, "{all:?}");
    (sent[0].clone().unwrap(), all)
}

/// The viewer input a `pick-*` command's answer says it was sent.
fn input_of(answer: &Value) -> ViewerInput {
    serde_json::from_value(answer["input"].clone()).unwrap()
}

/// That nothing was sent, nor is being read to be sent.
fn nothing_sent(e: &Editor) {
    assert!(!e.extensions.command_running(ID));
    assert!(!e.extensions.reading());
}

/// One Develop frame with `events`, taking its keys.
fn frame(e: &mut Editor, events: Vec<egui::Event>) {
    let ctx = e.context.clone();
    let edit = e.begin_develop_frame();
    let mut output = ctx.run_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ui| e.develop_shortcuts(ui.ctx()),
    );
    output.textures_delta.clear();
    e.finish_edit_frame(edit, &ctx);
}

/// One Develop frame as the window draws it, in order: the shortcuts, then
/// the photo, whose overlay takes the pointer. The photo fills the 200 by
/// 200 window's middle; `photo` gives it a texture to draw.
fn photo_frame(e: &mut Editor, events: Vec<egui::Event>) {
    let ctx = e.context.clone();
    if e.preview.texture.is_none() {
        e.preview.texture = Some(
            ctx.load_texture(
                "photo",
                egui::ColorImage::filled([32, 24], egui::Color32::GRAY),
                egui::TextureOptions::LINEAR,
            )
            .into(),
        );
    }
    let edit = e.begin_develop_frame();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::splat(200.),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            e.develop_shortcuts(ui.ctx());
            e.viewport_ui(ui);
        },
    );
    output.textures_delta.clear();
    e.finish_edit_frame(edit, &ctx);
}

/// A press and a release of the primary button at `at`, each in a frame of
/// its own, the release with `with` beside it.
fn click_at(e: &mut Editor, at: egui::Pos2, with: Vec<egui::Event>) {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    photo_frame(e, vec![egui::Event::PointerMoved(at), button(true)]);
    let mut release = with;
    release.push(button(false));
    photo_frame(e, release);
}

/// A drag of the primary button from `from` to `to`, a frame for each step.
fn drag_on(e: &mut Editor, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    photo_frame(e, vec![egui::Event::PointerMoved(from), button(from, true)]);
    photo_frame(e, vec![egui::Event::PointerMoved(from.lerp(to, 0.5))]);
    photo_frame(e, vec![egui::Event::PointerMoved(to)]);
    photo_frame(e, vec![button(to, false)]);
}

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

const fn click(x: f32, y: f32, positive: bool) -> Click {
    Click { x, y, positive }
}

#[test]
fn each_kind_is_sent_once_as_its_viewer_input() {
    let f = fixture(&["a.png"]);
    let mut e = f.editor;
    open(&mut e, &f.photos[0]);

    // A point: the first click sends it, and collecting ends.
    begin(&mut e, "pick-point");
    nothing_sent(&e);
    e.command_input_click([1.5, 0.5], true);
    assert!(e.command_input().is_some(), "off the photo is no click");
    e.command_input_click([0.25, 0.75], false);
    assert!(e.command_input().is_none());
    let (answer, _) = once(&mut e, "Pick a Point");
    assert_eq!(input_of(&answer), ViewerInput::Point { x: 0.25, y: 0.75 });
    assert_eq!(answer["photos"], json!([]), "it applies to nothing");

    // Points add up, Alt-clicks among them, and are sent by Enter.
    begin(&mut e, "pick-points");
    e.command_input_click([0.25, 0.25], true);
    e.command_input_click([0.5, 0.5], false);
    nothing_sent(&e);
    frame(&mut e, vec![key(egui::Key::Enter)]);
    assert!(e.command_input().is_none());
    let (answer, _) = once(&mut e, "Pick Points");
    assert_eq!(
        input_of(&answer),
        ViewerInput::Points {
            clicks: vec![click(0.25, 0.25, true), click(0.5, 0.5, false)]
        }
    );

    // At most MAX_CLICKS of them; Done with none sends nothing.
    begin(&mut e, "pick-points");
    e.command_input_done();
    assert!(e.command_input().is_some());
    nothing_sent(&e);
    for i in 0..MAX_CLICKS + 3 {
        e.command_input_click([0.5, i as f32 / 64.], true);
    }
    assert_eq!(e.command_input().unwrap().clicks(), MAX_CLICKS);
    e.command_input_done();
    let (answer, _) = once(&mut e, "Pick Points");
    let ViewerInput::Points { clicks } = input_of(&answer) else {
        panic!("points: {answer}");
    };
    assert_eq!(clicks.len(), MAX_CLICKS);

    // A box: a drag under 1% of the photo on either side is none; the first
    // that isn't is sent, with the open photo it applies to, read then.
    begin(&mut e, "pick-box");
    e.command_input_click([0.5, 0.5], true);
    e.command_input_drag([0.5, 0.2], [0.509, 0.8]);
    e.command_input_drag([0.2, 0.5], [0.8, 0.509]);
    nothing_sent(&e);
    assert!(e.command_input().is_some());
    e.command_input_drag([0.75, 0.5], [0.25, 0.125]);
    assert!(e.command_input().is_none());
    let (answer, _) = once(&mut e, "Pick a Box");
    assert_eq!(
        input_of(&answer),
        ViewerInput::Box {
            x0: 0.25,
            y0: 0.125,
            x1: 0.75,
            y1: 0.5
        }
    );
    assert_eq!(answer["photos"], json!([f.photos[0].0.0]));

    // A command that takes none runs at once, as before.
    let settings = run(&mut e, "settings", "Show Settings").unwrap();
    assert_eq!(settings, json!({"step": 0.5}));
    assert!(e.command_input().is_none());
}

#[test]
fn a_prompt_is_sent_at_each_change_one_at_a_time() {
    let f = fixture(&["a.png"]);
    let mut e = f.editor;
    open(&mut e, &f.photos[0]);
    begin(&mut e, "pick-prompt");
    // A negative first click has nothing to leave out of.
    e.command_input_click([0.5, 0.5], false);
    nothing_sent(&e);
    e.command_input_click([0.25, 0.5], true);
    assert!(e.extensions.command_running(ID));
    // Ignored while it runs, not queued: neither a click nor a box.
    e.command_input_click([0.75, 0.5], true);
    e.command_input_drag([0.1, 0.1], [0.9, 0.9]);
    assert_eq!(e.command_input().unwrap().clicks(), 1);
    let (answer, _) = once(&mut e, "Pick a Prompt");
    assert_eq!(
        input_of(&answer),
        ViewerInput::Prompt {
            clicks: vec![click(0.25, 0.5, true)],
            bounds: None,
        }
    );
    // The next change is sent whole: a box starts over.
    e.command_input_drag([0.75, 0.75], [0.25, 0.25]);
    let (answer, _) = once(&mut e, "Pick a Prompt");
    let bounds = Bounds {
        x0: 0.25,
        y0: 0.25,
        x1: 0.75,
        y1: 0.75,
    };
    assert_eq!(
        input_of(&answer),
        ViewerInput::Prompt {
            clicks: Vec::new(),
            bounds: Some(bounds),
        }
    );
    e.command_input_click([0.5, 0.5], false);
    let (answer, _) = once(&mut e, "Pick a Prompt");
    assert_eq!(
        input_of(&answer),
        ViewerInput::Prompt {
            clicks: vec![click(0.5, 0.5, false)],
            bounds: Some(bounds),
        }
    );
    // Done ends it, sending nothing more.
    e.command_input_done();
    assert!(e.command_input().is_none());
    e.command_input_click([0.5, 0.5], true);
    nothing_sent(&e);
    // Escape too.
    begin(&mut e, "pick-prompt");
    frame(&mut e, vec![key(egui::Key::Escape)]);
    assert!(e.command_input().is_none());
    nothing_sent(&e);
}

#[test]
fn every_way_out_sends_nothing() {
    let f = fixture(&["a.png", "b.png"]);
    let mut e = f.editor;
    open(&mut e, &f.photos[0]);
    type Leave = fn(&mut Editor, &[(PhotoId, PathBuf)]);
    let ways: [(&str, Leave); 5] = [
        ("Escape", |e, _| frame(e, vec![key(egui::Key::Escape)])),
        ("Cancel", |e, _| e.end_command_input()),
        ("a tool", |e, _| e.view.set_tool(Tool::Crop)),
        ("Library", |e, _| e.module = Module::Library),
        ("another photo", |e, photos| open(e, &photos[1])),
    ];
    for (what, leave) in ways {
        e.module = Module::Develop;
        e.view.set_tool(Tool::None);
        begin(&mut e, "pick-points");
        e.command_input_click([0.5, 0.5], true);
        leave(&mut e, &f.photos);
        e.end_command_input_when_stale();
        assert!(e.command_input().is_none(), "{what}");
        // A click after it changes nothing, and Enter sends nothing.
        e.command_input_click([0.5, 0.5], true);
        e.command_input_done();
        nothing_sent(&e);
    }
    let mut late = Vec::new();
    let until = Instant::now() + Duration::from_millis(200);
    while Instant::now() < until {
        late.extend(e.extensions_poll(Instant::now()).0);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(results(&late, "Pick Points").is_empty(), "{late:?}");

    // Starting it puts the open tool away, Masking included.
    open(&mut e, &f.photos[0]);
    e.view.set_tool(Tool::Mask);
    begin(&mut e, "pick-point");
    assert!(e.view.is(Tool::None));
    e.end_command_input_when_stale();
    assert!(e.command_input().is_some(), "its own tool change isn't one");
}

#[test]
fn a_crash_or_disabling_the_extension_ends_it() {
    let f = fixture(&["a.png"]);
    let mut e = f.editor;
    open(&mut e, &f.photos[0]);
    begin(&mut e, "pick-prompt");
    e.command_input_click([0.5, 0.5], true);
    once(&mut e, "Pick a Prompt");
    e.end_command_input_when_stale();
    assert!(e.command_input().is_some(), "an answer is no way out");
    // Its program crashing, by another command.
    e.run_extension_command(&format!("{ID}/crash"), RunBy::Button)
        .unwrap();
    pump(&mut e, "the crash", |_, o| result(o, "Crash").is_some());
    e.end_command_input_when_stale();
    assert!(e.command_input().is_none());
    // Disabled while collecting, before anything was sent.
    begin(&mut e, "pick-points");
    e.command_input_click([0.5, 0.5], true);
    e.extensions.set_enabled(ID, false).unwrap();
    e.end_command_input_when_stale();
    assert!(e.command_input().is_none());
    e.command_input_done();
    nothing_sent(&e);
    // Disabled, it doesn't start.
    assert!(
        e.run_extension_command(&format!("{ID}/pick-point"), RunBy::Button)
            .is_err()
    );
    assert!(e.command_input().is_none());
}

#[test]
fn it_needs_a_photo_in_develop_and_collects_no_brush() {
    let f = fixture(&["a.png"]);
    let mut e = f.editor;
    // In the Library, and in Develop with nothing open.
    for module in [Module::Library, Module::Develop] {
        e.module = module;
        let error = e
            .run_extension_command(&format!("{ID}/pick-point"), RunBy::Key)
            .unwrap_err();
        assert_eq!(error.code, "no_document");
        assert_eq!(error.message, "Open a photo in Develop first");
        assert!(e.command_input().is_none());
    }
    // A manifest of API 1 that declares brush loads; its command doesn't run.
    assert!(
        e.extensions
            .get(ID)
            .unwrap()
            .manifest
            .commands
            .iter()
            .any(|c| c.id == "paint")
    );
    open(&mut e, &f.photos[0]);
    let error = e
        .run_extension_command(&format!("{ID}/paint"), RunBy::Button)
        .unwrap_err();
    assert_eq!(error.code, "unsupported");
    assert!(error.message.contains("brush"), "{}", error.message);
    assert!(e.command_input().is_none());
    nothing_sent(&e);
    assert!(!e.extensions.get(ID).unwrap().running(), "nothing started");
}

#[test]
fn the_host_sends_only_valid_input_of_the_declared_kind() {
    let f = fixture(&["a.png"]);
    let mut e = f.editor;
    let point = ViewerInput::Point { x: 0.5, y: 0.5 };
    let refused = [
        ("pick-point", None),
        ("pick-box", Some(point.clone())),
        ("settings", Some(point)),
        ("pick-point", Some(ViewerInput::Point { x: 1.5, y: 0.5 })),
        (
            "pick-box",
            Some(ViewerInput::Box {
                x0: 0.5,
                y0: 0.1,
                x1: 0.5,
                y1: 0.9,
            }),
        ),
        (
            "pick-points",
            Some(ViewerInput::Points {
                clicks: vec![click(0.5, 0.5, true); MAX_CLICKS + 1],
            }),
        ),
    ];
    for (command, input) in refused {
        assert!(
            e.extensions
                .run_command(ID, command, None, input.clone(), RunBy::Button)
                .is_err(),
            "{command} {input:?}"
        );
    }
    nothing_sent(&e);
    assert!(!e.extensions.get(ID).unwrap().running(), "nothing started");
}

#[test]
fn another_command_running_holds_back_only_a_prompt() {
    let f = fixture(&["a.png"]);
    let mut e = f.editor;
    open(&mut e, &f.photos[0]);
    photo_frame(&mut e, vec![]);
    e.run_extension_command(&format!("{ID}/hang"), RunBy::Button)
        .unwrap();
    assert_eq!(e.extensions.commands_in_flight(ID), ["hang"]);
    let middle = egui::Pos2::new(100., 100.);
    // A point, through the pointer: sent at once, to wait behind `hang` in
    // the program as any command does.
    begin(&mut e, "pick-point");
    click_at(&mut e, middle, vec![]);
    assert!(e.command_input().is_none());
    assert_eq!(e.extensions.commands_in_flight(ID), ["hang", "pick-point"]);
    // Points, sent at Enter.
    begin(&mut e, "pick-points");
    click_at(&mut e, middle, vec![]);
    assert_eq!(e.command_input().unwrap().clicks(), 1);
    photo_frame(&mut e, vec![key(egui::Key::Enter)]);
    assert!(e.command_input().is_none());
    assert_eq!(
        e.extensions.commands_in_flight(ID),
        ["hang", "pick-point", "pick-points"]
    );
    // A box, at the end of its drag.
    begin(&mut e, "pick-box");
    drag_on(
        &mut e,
        egui::Pos2::new(60., 70.),
        egui::Pos2::new(140., 130.),
    );
    assert!(e.command_input().is_none());
    assert_eq!(
        e.extensions.commands_in_flight(ID),
        ["hang", "pick-box", "pick-point", "pick-points"]
    );
    // A prompt waits: its clicks are ignored while a command runs.
    begin(&mut e, "pick-prompt");
    click_at(&mut e, middle, vec![]);
    assert_eq!(e.command_input().unwrap().clicks(), 0);
    assert_eq!(
        e.extensions.commands_in_flight(ID),
        ["hang", "pick-box", "pick-point", "pick-points"]
    );
}

#[test]
fn a_tool_taken_up_in_the_same_frame_gets_the_click_alone() {
    let f = fixture(&["a.png"]);
    let mut e = f.editor;
    open(&mut e, &f.photos[0]);
    photo_frame(&mut e, vec![]);
    let middle = egui::Pos2::new(100., 100.);
    // W opens White Balance in the frame the click is released in: the
    // click is the tool's, and the command isn't sent.
    begin(&mut e, "pick-point");
    let before = e.document.edit.recipe().clone();
    click_at(&mut e, middle, vec![key(egui::Key::W)]);
    assert!(e.command_input().is_none());
    nothing_sent(&e);
    // White Balance picked a neutral with it, and put itself away.
    assert_ne!(*e.document.edit.recipe(), before, "the tool took the click");
    assert!(e.view.is(Tool::None));
    // W with Enter: points aren't sent either, whichever key came first.
    for keys in [
        [key(egui::Key::W), key(egui::Key::Enter)],
        [key(egui::Key::Enter), key(egui::Key::W)],
    ] {
        e.view.set_tool(Tool::None);
        begin(&mut e, "pick-points");
        click_at(&mut e, middle, vec![]);
        assert_eq!(e.command_input().unwrap().clicks(), 1);
        photo_frame(&mut e, keys.to_vec());
        assert!(e.command_input().is_none());
        nothing_sent(&e);
    }
    // A tool opened and put away within the frame is taken up all the same:
    // C or R opens Crop, and Enter finishes it, before the release.
    for crop in [egui::Key::C, egui::Key::R] {
        e.view.set_tool(Tool::None);
        begin(&mut e, "pick-point");
        click_at(&mut e, middle, vec![key(crop), key(egui::Key::Enter)]);
        assert!(e.view.is(Tool::None), "{crop:?} then Enter closes Crop");
        assert!(e.command_input().is_none(), "{crop:?}");
        nothing_sent(&e);
    }
}
