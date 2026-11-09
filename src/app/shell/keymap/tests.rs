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
    &[Default::new(&[Key::F], Mods::NONE).once_per_press()],
);
static F_ONCE: Binding = Binding::new(
    "chrome",
    Context::Everywhere,
    &[Default::new(&[Key::F], Mods::NONE).once_per_press()],
)
.once();
static ANY_F: Binding = Binding::new(
    "chrome",
    Context::Everywhere,
    &[Default::new(&[Key::F], Mods::NONE)],
);
static TAB: Binding = Binding::new(
    "panels:sides",
    Context::Everywhere,
    &[Default::new(&[Key::Tab], Mods::NONE)],
)
.consuming();
static SHIFT_TAB: Binding = Binding::new(
    "panels:all",
    Context::Everywhere,
    &[Default::new(&[Key::Tab], Mods::SHIFT)],
)
.consuming();
static Y: Binding = Binding::new(
    "before_after:left_right",
    Context::Develop,
    &[Default::new(&[Key::Y], Mods::NONE)],
);
static SHIFT_Y: Binding = Binding::new(
    "before_after:split",
    Context::Develop,
    &[Default::new(&[Key::Y], Mods::SHIFT)],
);
static REDO: Binding = Binding::new(
    "redo",
    Context::Library,
    &[
        Default::new(&[Key::Z], Mods::CMD.with(Mods::SHIFT)),
        Default::new(&[Key::Y], Mods::CMD),
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
        let keys = FrameKeys::begin(ctx, std::sync::Arc::default());
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
fn a_consuming_binding_takes_out_every_press_of_its_chord() {
    let events = vec![
        press(Key::Tab, Modifiers::ALT),
        repeat(Key::Tab, Modifiers::NONE),
        press(Key::Tab, Modifiers::COMMAND),
        press(Key::Tab, Modifiers::SHIFT),
        release(Key::Tab),
        press(Key::F, Modifiers::NONE),
        press(Key::Tab, Modifiers::NONE),
    ];
    let ctx = egui::Context::default();
    let (taken, left) = frame(&ctx, events, |ctx| {
        (FrameKeys::of(ctx).take(&TAB), key_events(ctx))
    });
    assert_eq!(taken, Some(Modifiers::NONE), "the first press it matches");
    // Tab's repeat and its second press go; Tab with other modifiers stays.
    // (egui counts the presses after the first as repeats.)
    let left: Vec<(Key, bool, Modifiers)> = left
        .iter()
        .filter_map(|e| match e {
            Event::Key {
                key,
                pressed,
                modifiers,
                ..
            } => Some((*key, *pressed, *modifiers)),
            _ => None,
        })
        .collect();
    assert_eq!(
        left,
        vec![
            (Key::Tab, true, Modifiers::ALT),
            (Key::Tab, true, Modifiers::COMMAND),
            (Key::Tab, true, Modifiers::SHIFT),
            (Key::Tab, false, Modifiers::NONE),
            (Key::F, true, Modifiers::NONE),
        ]
    );
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
    let (first, rest, rest_is_y) = frame(&ctx, ys(), |ctx| {
        let keys = FrameKeys::of(ctx);
        let first = keys.take_first(&[&Y, &SHIFT_Y]);
        let rest = keys.presses();
        let rest_is_y = rest.iter().all(|p| keys.is(p, &Y));
        (first, rest, rest_is_y)
    });
    assert_eq!(
        first,
        Some((1, Modifiers::SHIFT)),
        "the first press decides"
    );
    assert_eq!(rest.len(), 1, "the other press is left");
    assert!(rest_is_y);
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
    let none = Mods::NONE;
    let typed = Default::new(&[Key::Num4], none).typed();
    let logical = Default::new(&[Key::Quote], none);
    let placed = Default::new(&[Key::Equals], none).placed();
    let quote = Press::of(&quote_on_4).expect("a press");
    let bracket = Press::of(&bracket_on_equals).expect("a press");
    assert!(typed.matches(&quote, none));
    assert!(logical.matches(&quote, none));
    assert!(placed.matches(&bracket, none));
    assert!(!Default::new(&[Key::Equals], none).matches(&bracket, none));
    assert!(
        Default::new(&[Key::CloseBracket], none)
            .typed()
            .matches(&bracket, none)
    );
    // Repeats only where the chord takes them.
    let held = Press::of(&repeat(Key::F, Modifiers::NONE)).expect("a press");
    assert!(ANY_F.matches(&held));
    assert!(!F.matches(&held));
    let _ = ctx;
}

fn typed(key: Key, physical: Key, modifiers: Modifiers) -> Press {
    Press::of(&Event::Key {
        key,
        physical_key: Some(physical),
        pressed: true,
        repeat: false,
        modifiers,
    })
    .expect("a press")
}

static RATE_UP: Binding = Binding::new(
    "rating:raise",
    Context::Everywhere,
    &[Default::new(&[Key::CloseBracket, Key::CloseCurlyBracket], Mods::NONE).rejecting(Mods::ALT)],
)
.variant(Mods::SHIFT);
static CLIPPING: Binding = Binding::new(
    "clipping",
    Context::Develop,
    &[Default::new(&[Key::J], Mods::NONE)],
);
static SHORTCUTS: Binding = Binding::new(
    "shortcuts",
    Context::Everywhere,
    &[Default::new(&[Key::Slash], Mods::CMD)],
);
static BEFORE: Binding = Binding::new(
    "compare",
    Context::Develop,
    &[Default::new(&[Key::Backslash], Mods::NONE).rejecting(Mods::SHIFT)],
);
static THUMBNAIL: Binding = Binding::new(
    "camera_thumbnail",
    Context::Develop,
    &[
        Default::new(&[Key::Pipe], Mods::NONE),
        Default::new(&[Key::Backslash], Mods::SHIFT),
    ],
);
static NEXT_SLIDER: Binding = Binding::new(
    "basic:next",
    Context::Develop,
    &[Default::new(&[Key::Period], Mods::NONE)
        .placed()
        .rejecting(Mods::ALT.with(Mods::SHIFT))],
);
static STEP_UP: Binding = Binding::new(
    "basic:increase",
    Context::Develop,
    &[Default::new(&[Key::Equals, Key::Plus], Mods::NONE).placed()],
)
.variant(Mods::ALT.with(Mods::SHIFT));

#[test]
fn modifiers_match_exactly_on_a_letter() {
    let (shift, alt) = (Modifiers::SHIFT, Modifiers::ALT);
    assert!(CLIPPING.matches(&typed(Key::J, Key::J, Modifiers::NONE)));
    for extra in [shift, alt, Modifiers::COMMAND, Modifiers::CTRL, shift | alt] {
        assert!(
            !CLIPPING.matches(&typed(Key::J, Key::J, extra)),
            "J with {extra:?}"
        );
    }
}

#[test]
fn variants_are_passed_on_not_matched() {
    // Shift goes on to the next photo; Alt is not a variant and refused.
    assert!(RATE_UP.matches(&typed(
        Key::CloseBracket,
        Key::CloseBracket,
        Modifiers::NONE
    )));
    assert!(RATE_UP.matches(&typed(
        Key::CloseCurlyBracket,
        Key::CloseBracket,
        Modifiers::SHIFT
    )));
    assert!(!RATE_UP.matches(&typed(Key::CloseBracket, Key::CloseBracket, Modifiers::ALT)));
    assert!(!RATE_UP.matches(&typed(
        Key::CloseBracket,
        Key::CloseBracket,
        Modifiers::COMMAND
    )));
    // Shift and Alt pick the step; Cmd is another key.
    for m in [
        Modifiers::NONE,
        Modifiers::SHIFT,
        Modifiers::ALT,
        Modifiers::SHIFT | Modifiers::ALT,
    ] {
        assert!(
            STEP_UP.matches(&typed(Key::Equals, Key::Equals, m)),
            "= with {m:?}"
        );
    }
    assert!(!STEP_UP.matches(&typed(Key::Equals, Key::Equals, Modifiers::COMMAND)));
}

#[test]
fn shift_and_alt_on_punctuation_are_free_unless_named_or_rejected() {
    let cmd = Modifiers::COMMAND;
    let (shift, alt) = (Modifiers::SHIFT, Modifiers::ALT);
    // German Cmd+/ is Cmd+Shift+7; a Mac types other symbols with Option.
    assert!(SHORTCUTS.matches(&typed(Key::Slash, Key::Num7, cmd | shift)));
    assert!(SHORTCUTS.matches(&typed(Key::Slash, Key::Slash, cmd | alt)));
    assert!(!SHORTCUTS.matches(&typed(Key::Slash, Key::Slash, Modifiers::NONE)));
    // Ctrl is never free: on a Mac Cmd+Ctrl+/ is another chord.
    assert!(!SHORTCUTS.matches(&typed(Key::Slash, Key::Slash, Modifiers::CTRL)));
    // `\` rejects Shift, which Shift+`\` names.
    assert!(BEFORE.matches(&typed(Key::Backslash, Key::Backslash, Modifiers::NONE)));
    assert!(BEFORE.matches(&typed(Key::Backslash, Key::Backslash, alt)));
    assert!(!BEFORE.matches(&typed(Key::Backslash, Key::Backslash, shift)));
    assert!(!BEFORE.matches(&typed(Key::Backslash, Key::Backslash, cmd)));
    assert!(THUMBNAIL.matches(&typed(Key::Backslash, Key::Backslash, shift)));
    // German macOS: Option+7 types |, Option+Shift+7 types \.
    assert!(THUMBNAIL.matches(&typed(Key::Pipe, Key::Num7, alt)));
    assert!(THUMBNAIL.matches(&typed(Key::Backslash, Key::Num7, alt | shift)));
    assert!(!THUMBNAIL.matches(&typed(Key::Backslash, Key::Backslash, alt)));
    assert!(!THUMBNAIL.matches(&typed(Key::Pipe, Key::Backslash, cmd | shift)));
    // The Basic panel's `.` refuses Shift and Alt.
    assert!(NEXT_SLIDER.matches(&typed(Key::Period, Key::Period, Modifiers::NONE)));
    assert!(!NEXT_SLIDER.matches(&typed(Key::Period, Key::Period, shift)));
    assert!(!NEXT_SLIDER.matches(&typed(Key::Period, Key::Period, alt)));
}

#[test]
fn the_users_chords_replace_a_commands_defaults() {
    let user = [("chrome", vec![Chord::new(Key::G, Mods::CMD)])]
        .into_iter()
        .collect();
    let map = std::sync::Arc::new(Effective::new(user));
    let ctx = egui::Context::default();
    let (f, g) = frame(
        &ctx,
        vec![
            press(Key::F, Modifiers::NONE),
            press(Key::G, Modifiers::COMMAND),
            release(Key::F),
            release(Key::G),
        ],
        |ctx| {
            let keys = FrameKeys::begin(ctx, map.clone());
            let g = keys.take(&F);
            (keys.presses(), g)
        },
    );
    assert_eq!(g, Some(Modifiers::COMMAND), "Cmd+G runs it");
    assert_eq!(f.len(), 1, "F is no longer its key");
    assert_eq!(f[0].key, Key::F);
    // A handler that asks later in the frame reads the same chords.
    let later = frame(
        &ctx,
        vec![press(Key::G, Modifiers::COMMAND), release(Key::G)],
        |ctx| {
            FrameKeys::begin(ctx, map.clone());
            FrameKeys::of(ctx).take(&F)
        },
    );
    assert_eq!(later, Some(Modifiers::COMMAND));
    // Repeats as the binding's defaults allow: F once per press.
    assert!(map.defaults_of(&F).iter().all(|d| !d.repeats));
    assert!(map.defaults_of(&ANY_F).iter().all(|d| d.repeats));
}

#[test]
fn a_users_chord_on_a_punctuation_key_reads_as_the_binding_does() {
    // The rating keys refuse Alt, which types other symbols there.
    let user = [("rating:raise", vec![Chord::new(Key::Period, Mods::NONE)])]
        .into_iter()
        .collect();
    let map = std::sync::Arc::new(Effective::new(user));
    let ctx = egui::Context::default();
    let taken = frame(
        &ctx,
        vec![
            press(Key::Period, Modifiers::ALT),
            press(Key::Period, Modifiers::SHIFT),
        ],
        |ctx| FrameKeys::begin(ctx, map.clone()).take(&RATE_UP),
    );
    assert_eq!(
        taken,
        Some(Modifiers::SHIFT),
        "Shift, the variant, is passed on"
    );
}

#[test]
fn a_press_two_commands_share_is_reported_once_and_runs_neither() {
    use crate::app::shell::commands::registry::bindings as b;
    let f9 = Chord::new(Key::F9, Mods::NONE);
    let map = std::sync::Arc::new(Effective::new(
        [("grid:cell_style", vec![f9]), ("library:compare", vec![f9])]
            .into_iter()
            .collect(),
    ));
    let ctx = egui::Context::default();
    let (ran, reports) = frame(&ctx, vec![press(Key::F9, Modifiers::NONE)], |ctx| {
        let keys = FrameKeys::begin(ctx, map.clone());
        keys.set_live(Live::new(vec![
            Context::Everywhere,
            Context::Library,
            Context::Grid,
        ]));
        // As the grid's key loop asks each of its bindings in turn.
        let presses = keys.presses();
        let ran: Vec<&str> = [&b::GRID_COMPARE, &b::GRID_CELL_STYLE, &b::GRID_SURVEY]
            .iter()
            .filter(|binding| presses.iter().any(|p| keys.is(p, binding)))
            .map(|binding| binding.command)
            .collect();
        (ran, keys.take_reports())
    });
    assert_eq!(ran, Vec::<&str>::new());
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(
        reports[0].starts_with("F9 is the key of both"),
        "{reports:?}"
    );
}

#[test]
fn a_binding_that_acts_once_takes_its_other_presses_by_precedence_too() {
    use crate::app::shell::commands::registry::bindings as b;
    let x = Chord::new(Key::X, Mods::NONE);
    let map = std::sync::Arc::new(Effective::new(
        [
            ("chrome", vec![Chord::new(Key::F9, Mods::NONE), x]),
            ("panels:sides", vec![x]),
        ]
        .into_iter()
        .collect(),
    ));
    let ctx = egui::Context::default();
    let (chrome, sides, reports) = frame(
        &ctx,
        vec![
            press(Key::F9, Modifiers::NONE),
            press(Key::X, Modifiers::NONE),
        ],
        |ctx| {
            let keys = FrameKeys::begin(ctx, map.clone());
            keys.set_live(Live::new(vec![Context::Everywhere, Context::Develop]));
            let chrome = keys.take(&F_ONCE);
            (chrome, keys.take(&b::PANELS_SIDES), keys.take_reports())
        },
    );
    assert_eq!(chrome, Some(Modifiers::NONE), "F9 runs Chrome");
    assert_eq!(sides, None, "X, shared, runs neither");
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(
        reports[0].starts_with("X is the key of both"),
        "{reports:?}"
    );
}
