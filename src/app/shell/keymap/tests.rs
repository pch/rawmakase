use super::*;
use eframe::egui::{Event, Key, Modifiers, RawInput};

fn press(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }
}
fn repeat(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: true,
        modifiers,
    }
}
fn release(key: Key) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

/// Runs one frame with `events` and `body`.
fn frame<R>(ctx: &egui::Context, events: Vec<Event>, body: impl FnOnce(&egui::Context) -> R) -> R {
    let mut out = None;
    let mut body = Some(body);
    let mut output = ctx.run_ui(
        RawInput {
            events,
            ..RawInput::default()
        },
        |ui| out = Some((body.take().expect("one pass"))(ui.ctx())),
    );
    output.textures_delta.clear();
    out.expect("the frame ran")
}

fn key_events(ctx: &egui::Context) -> Vec<Event> {
    ctx.input(|i| {
        i.events
            .iter()
            .filter(|e| matches!(e, Event::Key { .. }))
            .cloned()
            .collect()
    })
}

static F: Binding = Binding::new(
    "chrome",
    Context::Everywhere,
    &[Default::new(&[Key::F], rules::none, "F").once_per_press()],
);
static F_ONCE: Binding = Binding::new(
    "chrome",
    Context::Everywhere,
    &[Default::new(&[Key::F], rules::none, "F").once_per_press()],
)
.once();
static ANY_F: Binding = Binding::new(
    "chrome",
    Context::Everywhere,
    &[Default::new(&[Key::F], rules::any, "F")],
);
static TAB: Binding = Binding::new(
    "panels:sides",
    Context::Everywhere,
    &[Default::new(&[Key::Tab], rules::logically_none, "Tab")],
)
.consuming();
static SHIFT_TAB: Binding = Binding::new(
    "panels:all",
    Context::Everywhere,
    &[Default::new(
        &[Key::Tab],
        rules::logically_shift,
        "Shift+Tab",
    )],
)
.consuming();
static Y: Binding = Binding::new(
    "before_after:left_right",
    Context::Develop,
    &[Default::new(&[Key::Y], rules::none, "Y")],
);
static SHIFT_Y: Binding = Binding::new(
    "before_after:split",
    Context::Develop,
    &[Default::new(&[Key::Y], |m| m.shift_only(), "Shift+Y")],
);
static REDO: Binding = Binding::new(
    "redo",
    Context::Library,
    &[
        Default::new(&[Key::Z], rules::logically_command_shift, "Cmd+Shift+Z"),
        Default::new(&[Key::Y], rules::logically_command, "Cmd+Y"),
    ],
)
.consuming();

#[test]
fn a_press_runs_one_command() {
    let ctx = egui::Context::default();
    let (first, second) = frame(&ctx, vec![press(Key::F, Modifiers::NONE)], |ctx| {
        let keys = FrameKeys::of(ctx);
        (keys.take(&F), keys.take(&ANY_F))
    });
    assert_eq!(first, Some(Modifiers::NONE));
    assert_eq!(second, None, "the press was taken");
    // Taking leaves the press in egui's input for the widgets.
    let left = frame(&ctx, vec![press(Key::F, Modifiers::NONE)], |ctx| {
        FrameKeys::of(ctx).take(&F);
        key_events(ctx)
    });
    assert_eq!(left.len(), 1);
}

#[test]
fn each_pass_starts_with_nothing_taken() {
    let ctx = egui::Context::default();
    for _ in 0..2 {
        // Released, or egui counts the next frame's press as a repeat.
        let events = vec![press(Key::F, Modifiers::NONE), release(Key::F)];
        let taken = frame(&ctx, events, |ctx| FrameKeys::of(ctx).take(&F));
        assert!(taken.is_some());
    }
    let after_begin = frame(&ctx, vec![press(Key::F, Modifiers::NONE)], |ctx| {
        let keys = FrameKeys::of(ctx);
        keys.take(&F);
        keys.begin();
        keys.take(&F)
    });
    assert!(after_begin.is_some(), "begin starts the frame afresh");
}

#[test]
fn a_press_a_widget_consumed_is_not_offered() {
    let ctx = egui::Context::default();
    let taken = frame(&ctx, vec![press(Key::Tab, Modifiers::NONE)], |ctx| {
        ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab));
        FrameKeys::of(ctx).take(&TAB)
    });
    assert_eq!(taken, None);
}

#[test]
fn a_consuming_binding_takes_out_what_consume_key_takes_out() {
    let events = || {
        vec![
            press(Key::Tab, Modifiers::ALT),
            repeat(Key::Tab, Modifiers::NONE),
            press(Key::Tab, Modifiers::COMMAND),
            press(Key::Tab, Modifiers::SHIFT),
            release(Key::Tab),
            press(Key::F, Modifiers::NONE),
            press(Key::Tab, Modifiers::NONE),
        ]
    };
    // A context each: egui counts a key still down from a frame before as
    // repeating.
    let expected = frame(&egui::Context::default(), events(), |ctx| {
        ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab));
        key_events(ctx)
    });
    let ctx = egui::Context::default();
    let (taken, left) = frame(&ctx, events(), |ctx| {
        (FrameKeys::of(ctx).take(&TAB), key_events(ctx))
    });
    assert_eq!(taken, Some(Modifiers::ALT), "the first press it matches");
    assert_eq!(left, expected);
    // With nothing for it, nothing is taken out.
    let left = frame(
        &egui::Context::default(),
        vec![press(Key::Tab, Modifiers::COMMAND)],
        |ctx| {
            assert_eq!(FrameKeys::of(ctx).take(&TAB), None);
            key_events(ctx)
        },
    );
    assert_eq!(left.len(), 1);
}

#[test]
fn chords_are_tried_in_order_as_a_consume_key_chain() {
    let ctx = egui::Context::default();
    // Ctrl+Y comes first, but Cmd+Shift+Z is the first chord.
    let (taken, left) = frame(
        &ctx,
        vec![
            press(Key::Y, Modifiers::COMMAND),
            press(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT),
        ],
        |ctx| (FrameKeys::of(ctx).take(&REDO), key_events(ctx)),
    );
    // Only Cmd+Shift+Z's chord consumes: Ctrl+Y stays, as after
    // `consume_key(COMMAND | SHIFT, Z) || consume_key(COMMAND, Y)`.
    assert_eq!(taken, Some(Modifiers::COMMAND | Modifiers::SHIFT));
    assert_eq!(left, vec![press(Key::Y, Modifiers::COMMAND)]);
}

#[test]
fn once_takes_every_press_and_acts_once() {
    let ctx = egui::Context::default();
    let two = || {
        vec![
            press(Key::F, Modifiers::NONE),
            release(Key::F),
            press(Key::F, Modifiers::NONE),
        ]
    };
    let (once, again) = frame(&ctx, two(), |ctx| {
        let keys = FrameKeys::of(ctx);
        (keys.take(&F_ONCE), keys.take(&F))
    });
    assert!(once.is_some());
    assert_eq!(again, None, "both presses went to the one command");
    let (each, again) = frame(&egui::Context::default(), two(), |ctx| {
        let keys = FrameKeys::of(ctx);
        (keys.take(&F), keys.take(&F))
    });
    assert!(each.is_some());
    assert!(again.is_some(), "the second press is still there");
}

#[test]
fn take_any_goes_by_binding_order_and_take_first_by_event_order() {
    let ctx = egui::Context::default();
    let events = || {
        vec![
            press(Key::Tab, Modifiers::NONE),
            press(Key::Tab, Modifiers::SHIFT),
        ]
    };
    let any = frame(&ctx, events(), |ctx| {
        FrameKeys::of(ctx).take_any(&[&SHIFT_TAB, &TAB])
    });
    // Shift+Tab's pattern is tried first, whatever the order of the presses.
    assert_eq!(any, Some((0, Modifiers::SHIFT)));

    let ys = || {
        vec![
            press(Key::Y, Modifiers::SHIFT),
            press(Key::Y, Modifiers::NONE),
        ]
    };
    let (first, rest) = frame(&ctx, ys(), |ctx| {
        let keys = FrameKeys::of(ctx);
        (keys.take_first(&[&Y, &SHIFT_Y]), keys.presses())
    });
    assert_eq!(
        first,
        Some((1, Modifiers::SHIFT)),
        "the first press decides"
    );
    assert_eq!(rest.len(), 1, "the other press is left");
    assert!(rest[0].is(&Y));
}

#[test]
fn identical_presses_are_told_apart() {
    let ctx = egui::Context::default();
    let three = vec![
        press(Key::F, Modifiers::NONE),
        press(Key::F, Modifiers::NONE),
        press(Key::F, Modifiers::NONE),
    ];
    frame(&ctx, three, |ctx| {
        let keys = FrameKeys::of(ctx);
        assert!(keys.take(&F).is_some());
        assert_eq!(keys.presses().len(), 2);
        // Taking one out of the input keeps the others as they were.
        let [second, third] = keys.presses()[..] else {
            panic!("two presses left");
        };
        keys.take_press(&third);
        keys.remove_press(&second);
        assert!(keys.presses().is_empty(), "the third stays taken");
        assert_eq!(key_events(ctx).len(), 2);
        // The first press, taken but not consumed, is still in the input.
        assert_eq!(keys.take(&F), None);
    });
}

#[test]
fn chords_match_by_key_place_or_what_the_key_types() {
    let ctx = egui::Context::default();
    // AZERTY's 4 key types `'`; Dvorak's `=` key types `]`.
    let quote_on_4 = Event::Key {
        key: Key::Quote,
        physical_key: Some(Key::Num4),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    let bracket_on_equals = Event::Key {
        key: Key::CloseBracket,
        physical_key: Some(Key::Equals),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    let typed = Default::new(&[Key::Num4], rules::any, "4").typed();
    let logical = Default::new(&[Key::Quote], rules::any, "'");
    let placed = Default::new(&[Key::Equals], rules::any, "=").placed();
    let quote = Press::of(&quote_on_4).expect("a press");
    let bracket = Press::of(&bracket_on_equals).expect("a press");
    assert!(typed.matches(&quote));
    assert!(logical.matches(&quote));
    assert!(placed.matches(&bracket));
    assert!(!Default::new(&[Key::Equals], rules::any, "=").matches(&bracket));
    assert!(
        Default::new(&[Key::CloseBracket], rules::any, "]")
            .typed()
            .matches(&bracket)
    );
    // Repeats only where the chord takes them.
    let held = Press::of(&repeat(Key::F, Modifiers::NONE)).expect("a press");
    assert!(ANY_F.matches(&held));
    assert!(!F.matches(&held));
    let _ = ctx;
}

#[test]
fn the_logical_rules_are_eguis() {
    let all = [
        Modifiers::NONE,
        Modifiers::SHIFT,
        Modifiers::ALT,
        Modifiers::COMMAND,
        Modifiers::CTRL,
        Modifiers::MAC_CMD | Modifiers::COMMAND,
        Modifiers::COMMAND | Modifiers::SHIFT,
        Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT,
        Modifiers::SHIFT | Modifiers::ALT,
    ];
    for m in all {
        assert_eq!(
            rules::logically_none(m),
            m.matches_logically(Modifiers::NONE)
        );
        assert_eq!(
            rules::logically_shift(m),
            m.matches_logically(Modifiers::SHIFT)
        );
        assert_eq!(
            rules::logically_command(m),
            m.matches_logically(Modifiers::COMMAND)
        );
        assert_eq!(
            rules::logically_command_shift(m),
            m.matches_logically(Modifiers::COMMAND | Modifiers::SHIFT)
        );
    }
}
