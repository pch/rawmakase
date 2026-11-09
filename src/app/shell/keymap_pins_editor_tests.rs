//! Pins of what each keyboard shortcut does today, before the shortcuts move into
//! one command registry (#963): each fires in its place and not outside it, and
//! keeps its guards behind a window, under a menu, while a field is typed in and
//! with a button focused. Then the Escape chain, the keys that change module, and
//! frames that bring several presses at once.
//!
//! Each pin starts from a newly built editor in a [`Place`]; see
//! [`Fixture::new`] for what it holds. Photos are named by their index in the
//! catalog: 1 to 3 are selected, 3 active (open, in Develop).
use super::keymap_pins_harness::*;
use crate::app::*;
use eframe::egui::{Key, Modifiers};
use std::collections::BTreeMap;

const NONE: Modifiers = Modifiers::NONE;
const SHIFT: Modifiers = Modifiers::SHIFT;
const ALT: Modifiers = Modifiers::ALT;

fn cmd() -> Modifiers {
    command(NONE)
}
fn cmd_shift() -> Modifiers {
    command(SHIFT)
}
fn cmd_alt() -> Modifiers {
    command(ALT)
}
fn cmd_alt_shift() -> Modifiers {
    command(ALT | SHIFT)
}

/// What one frame changed: the fingerprint's fields, and whether the key's press
/// was taken from the frame's input.
#[derive(Debug)]
struct Outcome {
    changed: BTreeMap<&'static str, String>,
    taken: bool,
}

impl Outcome {
    fn of(
        before: &[(&'static str, String)],
        after: Vec<(&'static str, String)>,
        left: Option<bool>,
    ) -> Self {
        let changed = after
            .into_iter()
            .filter(|(name, value)| {
                before
                    .iter()
                    .find(|(n, _)| n == name)
                    .is_none_or(|(_, v)| v != value)
            })
            .collect();
        Outcome {
            changed,
            taken: left == Some(false),
        }
    }
    fn get(&self, field: &str) -> Option<&str> {
        self.changed.get(field).map(String::as_str)
    }
    /// Whether `field` changed to `value`.
    fn is(&self, field: &str, value: &str) -> bool {
        self.get(field) == Some(value)
    }
    fn nothing(&self) -> bool {
        self.changed.is_empty()
    }
}

/// `key` tapped with `m` in a new editor in `place`.
fn tap(place: Place, key: Key, m: Modifiers) -> Outcome {
    let mut f = Fixture::new(place);
    tap_in(&mut f, key, m)
}

fn tap_in(f: &mut Fixture, key: Key, m: Modifiers) -> Outcome {
    let before = f.fingerprint();
    let left = f.frame_watching(tap_events(key, key, m), Some(key));
    Outcome::of(&before, f.fingerprint(), left)
}

/// `key` typing `key` at `physical`, tapped with `m`.
fn tap_typed(place: Place, key: Key, physical: Key, m: Modifiers) -> Outcome {
    let mut f = Fixture::new(place);
    let before = f.fingerprint();
    let left = f.frame_watching(tap_events(key, physical, m), Some(key));
    Outcome::of(&before, f.fingerprint(), left)
}

/// The first repeat of `key` held with `m` in `place`.
fn repeat(place: Place, key: Key, m: Modifiers) -> Outcome {
    let mut f = Fixture::new(place);
    f.hold(key, m);
    let before = f.fingerprint();
    let left = f.first_repeat(key, m);
    Outcome::of(&before, f.fingerprint(), left)
}

/// Several taps in one frame in `place`.
fn taps(place: Place, keys: &[(Key, Modifiers)]) -> Outcome {
    let mut f = Fixture::new(place);
    let before = f.fingerprint();
    f.taps(keys);
    Outcome::of(&before, f.fingerprint(), None)
}

/// The photos field with photo `i` as `state` (e.g. "r5f0_"), the rest as at the
/// start.
fn photos_with(changes: &[(usize, &str)]) -> String {
    let start = [
        "r2f1_q", "r0f0_", "r0f0_", "r0f0_", "r0f0_", "r0f0_", "r0f0_", "r0f-1_",
    ];
    (0..PHOTOS)
        .map(|i| {
            let state = changes
                .iter()
                .find(|(j, _)| *j == i)
                .map_or(start[i], |(_, s)| *s);
            format!("{i}:{state}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The guards every shortcut but a few keeps: nothing behind a window or while a
/// field is typed in, as today.
fn assert_inert_behind_a_window_and_while_typing(key: Key, m: Modifiers, what: &str) {
    for place in [
        Place::GridModal,
        Place::DevelopModal,
        Place::GridTyping,
        Place::DevelopTyping,
    ] {
        let o = tap(place, key, m);
        let changed: Vec<_> = o
            .changed
            .keys()
            .filter(|k| !matches!(**k, "field" | "popup"))
            .collect();
        assert!(changed.is_empty(), "{what} in {place:?}: {o:?}");
    }
}

// ---------------------------------------------------------------------------
// Everywhere: messages, the chrome, modules, virtual copies, undo, Preferences,
// the shortcut sheet and the panels.

#[test]
fn shift_escape_dismisses_every_message_before_anything_else_reads_escape() {
    let o = tap(Place::Crop, Key::Escape, SHIFT);
    assert!(o.is("messages", "0") && o.taken, "{o:?}");
    assert!(o.get("tool").is_none(), "the tool stays open: {o:?}");
    // Behind a window the messages stay.
    let o = tap(Place::GridModal, Key::Escape, SHIFT);
    assert!(o.get("messages").is_none(), "{o:?}");
    // With no message it is not taken, and Escape goes on: the Loupe closes.
    let mut f = Fixture::new(Place::Loupe);
    f.tap(Key::Escape, SHIFT);
    let o = tap_in(&mut f, Key::Escape, SHIFT);
    assert!(o.is("view", "grid"), "{o:?}");
    assert_eq!(f.e.messages.entry_count(), 0);
}

#[test]
fn escape_brings_the_hidden_chrome_back_before_it_closes_anything() {
    // The Loupe: the chrome first, then the Loupe.
    let mut f = Fixture::new(Place::Loupe);
    f.tap(Key::F, NONE);
    assert!(f.e.chrome.hidden(Module::Library));
    let o = tap_in(&mut f, Key::Escape, NONE);
    assert!(o.taken && o.get("chrome").is_some(), "{o:?}");
    assert!(f.e.chrome.shown(Module::Library));
    assert!(
        f.e.library.as_ref().unwrap().loupe_open(),
        "the Loupe stays"
    );
    assert!(tap_in(&mut f, Key::Escape, NONE).is("view", "grid"));
    // Develop: the chrome, then the tool.
    let mut f = Fixture::new(Place::Crop);
    f.tap(Key::F, NONE);
    let o = tap_in(&mut f, Key::Escape, NONE);
    assert!(
        o.get("chrome").is_some() && o.get("tool").is_none(),
        "{o:?}"
    );
    assert!(tap_in(&mut f, Key::Escape, NONE).is("tool", "None"));
    // A menu closes first: the chrome stays hidden and the Loupe open.
    let mut f = Fixture::new(Place::LoupePopup);
    f.tap(Key::F, NONE);
    let hidden = f.e.chrome.hidden(Module::Library);
    let o = tap_in(&mut f, Key::Escape, NONE);
    assert!(o.is("popup", "false"), "{o:?}");
    assert_eq!(f.e.chrome.hidden(Module::Library), hidden);
    assert!(f.e.library.as_ref().unwrap().loupe_open());
}

#[test]
fn escape_closes_the_innermost_thing_in_each_view() {
    assert!(tap(Place::Loupe, Key::Escape, NONE).is("view", "grid"));
    assert!(
        tap(Place::Loupe, Key::Escape, cmd()).is("view", "grid"),
        "any modifiers"
    );
    assert!(tap(Place::Compare, Key::Escape, NONE).is("view", "grid"));
    assert!(
        tap(Place::Compare, Key::Escape, cmd())
            .get("view")
            .is_none()
    );
    assert!(tap(Place::Survey, Key::Escape, NONE).is("view", "grid"));
    for place in [Place::Crop, Place::Remove, Place::RedEye, Place::Guided] {
        assert!(
            tap(place, Key::Escape, NONE).is("tool", "None"),
            "{place:?}"
        );
        assert!(
            tap(place, Key::Escape, cmd()).is("tool", "None"),
            "{place:?}"
        );
    }
    // Masking with nothing aimed or pending: its own chain closes it, and with
    // Cmd or Alt it closes at once.
    for m in [NONE, cmd(), ALT] {
        assert!(tap(Place::Mask, Key::Escape, m).is("tool", "None"), "{m:?}");
    }
    // A text field gives Escape up; behind a window the window takes it.
    let o = tap(Place::DevelopTyping, Key::Escape, NONE);
    assert!(o.get("tool").is_none(), "{o:?}");
}

#[test]
fn g_shows_the_grid_from_anywhere_shift_too_but_not_cmd_or_alt() {
    for place in [Place::Develop, Place::Crop] {
        let o = tap(place, Key::G, NONE);
        assert!(o.is("module", "Library"), "{place:?} {o:?}");
        assert!(tap(place, Key::G, SHIFT).is("module", "Library"));
        assert!(
            repeat(place, Key::G, NONE).is("module", "Library"),
            "repeats"
        );
        assert!(tap(place, Key::G, cmd()).get("module").is_none());
        assert!(tap(place, Key::G, ALT).get("module").is_none());
    }
    for place in [Place::Loupe, Place::Compare, Place::Survey] {
        assert!(tap(place, Key::G, NONE).is("view", "grid"), "{place:?}");
    }
    assert_inert_behind_a_window_and_while_typing(Key::G, NONE, "G");
    assert!(tap(Place::DevelopButton, Key::G, NONE).is("module", "Library"));
}

#[test]
fn e_in_develop_shows_the_open_photo_in_the_loupe() {
    let mut f = Fixture::new(Place::Develop);
    let o = tap_in(&mut f, Key::E, NONE);
    assert!(o.is("module", "Library") && o.is("view", "loupe"), "{o:?}");
    assert_eq!(f.active(), Some(f.ids[3]));
    assert!(tap(Place::Develop, Key::E, SHIFT).is("view", "loupe"));
    assert!(repeat(Place::Develop, Key::E, NONE).is("view", "loupe"));
    assert!(tap(Place::Develop, Key::E, ALT).get("module").is_none());
    assert!(tap(Place::Develop, Key::E, cmd()).get("module").is_none());
    assert_inert_behind_a_window_and_while_typing(Key::E, NONE, "E");
}

#[test]
fn cmd_quote_makes_one_virtual_copy_in_the_library_and_develop() {
    for place in [Place::Grid, Place::Loupe, Place::Develop, Place::Crop] {
        let o = tap(place, Key::Quote, cmd());
        assert!(
            o.get("photos").is_some_and(|p| p.contains("new")),
            "{place:?} {o:?}"
        );
        assert!(
            repeat(place, Key::Quote, cmd()).get("photos").is_none(),
            "{place:?}"
        );
    }
    assert!(tap(Place::Grid, Key::Quote, NONE).get("photos").is_none());
    assert_inert_behind_a_window_and_while_typing(Key::Quote, cmd(), "Cmd+'");
}

#[test]
fn f_hides_and_shows_the_chrome_once_per_press_and_alone() {
    for (place, chrome) in [
        (
            Place::Grid,
            "{ hidden: Some(Library), photo: false, opened: false }",
        ),
        (
            Place::Loupe,
            "{ hidden: Some(Library), photo: true, opened: false }",
        ),
        (
            Place::Develop,
            "{ hidden: Some(Develop), photo: true, opened: false }",
        ),
    ] {
        assert!(tap(place, Key::F, NONE).is("chrome", chrome), "{place:?}");
        assert!(tap(place, Key::F, SHIFT).get("chrome").is_none());
        assert!(tap(place, Key::F, cmd()).get("chrome").is_none());
        assert!(repeat(place, Key::F, NONE).get("chrome").is_none(), "held");
    }
    assert_inert_behind_a_window_and_while_typing(Key::F, NONE, "F");
}

#[test]
fn d_develops_the_active_photo_and_cmd_d_selects_none() {
    let o = tap(Place::Grid, Key::D, NONE);
    assert!(o.is("module", "Develop") && o.is("open", "3"), "{o:?}");
    assert!(tap(Place::Grid, Key::D, SHIFT).is("module", "Develop"));
    assert!(repeat(Place::Grid, Key::D, NONE).is("module", "Develop"));
    assert!(tap(Place::Loupe, Key::D, NONE).is("module", "Develop"));
    let o = tap(Place::Grid, Key::D, cmd());
    assert!(o.get("module").is_none() && o.is("selected", ""), "{o:?}");
    assert!(tap(Place::Grid, Key::D, ALT).nothing());
    assert_inert_behind_a_window_and_while_typing(Key::D, NONE, "D");
}

#[test]
fn library_undo_and_redo_take_their_keys_with_a_button_focused_too() {
    let undone = photos_with(&[(0, "r2f0_q")]);
    let redone = photos_with(&[(0, "r2f1Gq")]);
    for place in [Place::Grid, Place::Loupe, Place::GridButton] {
        let o = tap(place, Key::Z, cmd());
        assert!(o.is("photos", &undone) && o.taken, "{place:?} {o:?}");
        let o = tap(place, Key::Z, cmd_shift());
        assert!(o.is("photos", &redone) && o.taken, "{place:?} {o:?}");
        if !cfg!(target_os = "macos") {
            assert!(tap(place, Key::Y, cmd()).is("photos", &redone), "Ctrl+Y");
        }
    }
    assert!(tap(Place::Develop, Key::Z, cmd()).is("photos", &undone));
    assert!(tap(Place::Develop, Key::Z, cmd_shift()).is("photos", &redone));
    assert!(tap(Place::Grid, Key::Z, ALT).get("photos").is_none());
    assert_inert_behind_a_window_and_while_typing(Key::Z, cmd(), "Cmd+Z");
}

#[test]
fn cmd_comma_opens_preferences_even_while_typing() {
    for place in [
        Place::Grid,
        Place::Develop,
        Place::GridTyping,
        Place::DevelopTyping,
    ] {
        let o = tap(place, Key::Comma, cmd());
        assert!(o.is("dialog", "Preferences") && o.taken, "{place:?} {o:?}");
    }
    assert!(
        tap(Place::Grid, Key::Comma, cmd_shift()).is("dialog", "Preferences"),
        "loose"
    );
    // Not over another window.
    let mut f = Fixture::new(Place::Grid);
    f.tap(Key::Slash, cmd());
    assert!(f.e.view.shortcuts);
    let o = tap_in(&mut f, Key::Comma, cmd());
    assert!(!f.e.preferences.open, "{o:?}");
}

#[test]
fn cmd_slash_opens_and_closes_the_shortcut_sheet_even_while_typing() {
    for place in [Place::Grid, Place::Develop, Place::GridTyping] {
        let mut f = Fixture::new(place);
        let o = tap_in(&mut f, Key::Slash, cmd());
        assert!(o.is("dialog", "Shortcuts") && o.taken, "{place:?} {o:?}");
        let o = tap_in(&mut f, Key::Slash, cmd());
        assert!(o.is("dialog", "-") && o.taken, "{place:?} {o:?}");
    }
    // Behind Preferences it does not open.
    assert!(
        tap(Place::GridModal, Key::Slash, cmd())
            .get("dialog")
            .is_none()
    );
    // Escape closes the sheet; the keys behind it do nothing.
    let mut f = Fixture::new(Place::Grid);
    f.tap(Key::Slash, cmd());
    assert!(tap_in(&mut f, Key::J, NONE).nothing());
    assert!(tap_in(&mut f, Key::Escape, NONE).is("dialog", "-"));
}

#[test]
fn escape_clears_the_sheets_search_before_it_closes_the_sheet() {
    let mut f = Fixture::new(Place::Grid);
    f.tap(Key::Slash, cmd());
    f.frame(vec![eframe::egui::Event::Text("crop".into())]);
    f.frame(vec![]);
    assert!(f.e.view.shortcuts);
    let o = tap_in(&mut f, Key::Escape, NONE);
    assert!(
        o.get("dialog").is_none() && o.taken,
        "the search clears: {o:?}"
    );
    assert!(tap_in(&mut f, Key::Escape, NONE).is("dialog", "-"));
}

#[test]
fn panel_keys_change_panels_unless_a_control_has_the_focus() {
    let develop = |d: &str| format!("develop {d} library LRFPA tab Presets");
    let library = |l: &str| format!("develop LRFPA library {l} tab Presets");
    for (key, m, in_library, d) in [
        (Key::Tab, NONE, library("lrFPA"), "lrFpA"),
        (Key::Tab, SHIFT, library("lrfPA"), "lrfpA"),
        (Key::F7, NONE, library("lRFPA"), "lRFPA"),
        (Key::F8, NONE, library("LrFPA"), "LrFPA"),
        // The Library has no Presets panel: Shift+F8 there hides Develop's.
        (Key::F8, SHIFT, develop("LRFpA"), "LRFpA"),
        (Key::F6, NONE, library("LRfPA"), "LRfPA"),
    ] {
        let o = tap(Place::Grid, key, m);
        assert!(
            o.is("panels", &in_library) && o.taken,
            "{key:?} {m:?} {o:?}"
        );
        // In a window too narrow for it, Shift+F8 in Develop shows the tab
        // panel over the photo rather than changing the layout.
        let changes = |o: &Outcome| {
            if d == "LRFpA" {
                o.is("tab panel", "true")
            } else {
                o.is("panels", &develop(d))
            }
        };
        let o = tap(Place::Develop, key, m);
        assert!(changes(&o) && o.taken, "{key:?} {m:?} {o:?}");
        assert!(changes(&tap(Place::Crop, key, m)), "a tool open");
        // Alt held too, as the keys match loosely.
        let o = tap(Place::Develop, key, m | ALT);
        assert!(changes(&o), "{key:?} with Alt: {o:?}");
        for place in [
            Place::GridButton,
            Place::DevelopButton,
            Place::DevelopPopup,
            Place::LoupePopup,
            Place::GridTyping,
            Place::DevelopTyping,
            Place::GridModal,
            Place::DevelopModal,
        ] {
            let o = tap(place, key, m);
            assert!(
                o.get("panels").is_none() && o.get("tab panel").is_none(),
                "{place:?} {key:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rating, flags and labels, in the Library and Develop.

/// The photos field after `edit` reached the photos `on`.
fn edited(on: &[usize], state: &str) -> String {
    let changes: Vec<(usize, &str)> = on.iter().map(|i| (*i, state)).collect();
    photos_with(&changes)
}

#[test]
fn metadata_keys_edit_the_selection_in_the_grid_and_the_active_photo_elsewhere() {
    let cases: [(Key, Modifiers, &str); 15] = [
        (Key::Num1, NONE, "r1f0_"),
        (Key::Num5, NONE, "r5f0_"),
        (Key::Num6, NONE, "r0f0R"),
        (Key::Num7, NONE, "r0f0Y"),
        (Key::Num8, NONE, "r0f0G"),
        (Key::Num9, NONE, "r0f0B"),
        (Key::P, NONE, "r0f1_"),
        (Key::X, NONE, "r0f-1_"),
        (Key::Backtick, NONE, "r0f1_"),
        (Key::CloseBracket, NONE, "r1f0_"),
        (Key::CloseCurlyBracket, NONE, "r1f0_"),
        (Key::ArrowUp, cmd(), "r0f1_"),
        (Key::ArrowDown, cmd(), "r0f-1_"),
        (Key::Num3, SHIFT, "r3f0_"),
        (Key::P, SHIFT, "r0f1_"),
    ];
    for (key, m, state) in cases {
        let o = tap(Place::Grid, key, m);
        assert!(
            o.is("photos", &edited(&[1, 2, 3], state)) && o.taken,
            "{key:?} {o:?}"
        );
        for place in [Place::Loupe, Place::Develop, Place::GridButton] {
            let on: &[usize] = if place == Place::GridButton {
                &[1, 2, 3]
            } else {
                &[3]
            };
            let o = tap(place, key, m);
            assert!(
                o.is("photos", &edited(on, state)),
                "{place:?} {key:?} {o:?}"
            );
        }
        // Once per press, and not behind a window or while typing.
        assert!(
            repeat(Place::Develop, key, m).get("photos").is_none(),
            "{key:?}"
        );
        assert_inert_behind_a_window_and_while_typing(key, m, "a metadata key");
    }
    // Nothing with Alt, Ctrl or Cmd (but Cmd+Up and Down).
    for m in [ALT, cmd(), cmd_alt()] {
        assert!(
            tap(Place::Loupe, Key::Num3, m).get("photos").is_none(),
            "{m:?}"
        );
    }
    assert!(
        tap(Place::Loupe, Key::ArrowUp, cmd_alt())
            .get("photos")
            .is_none()
    );
    assert!(
        tap(Place::Loupe, Key::ArrowUp, cmd_shift())
            .get("photos")
            .is_none()
    );
}

#[test]
fn shift_moves_on_to_the_next_photo_after_a_metadata_key() {
    for (key, physical, state) in [
        (Key::Num3, Key::Num3, "r3f0_"),
        // Shift+5 types `%`, which egui has no key for: egui-winit sends Num5.
        (Key::Num5, Key::Num5, "r5f0_"),
        // Shift+1 types `!`.
        (Key::Exclamationmark, Key::Num1, "r1f0_"),
        // US Shift+] types `}`, and Shift+` types `~`, sent as Backtick.
        (Key::CloseCurlyBracket, Key::CloseBracket, "r1f0_"),
        (Key::Backtick, Key::Backtick, "r0f1_"),
    ] {
        let o = tap_typed(Place::Loupe, key, physical, SHIFT);
        assert!(o.is("photos", &edited(&[3], state)), "{key:?} {o:?}");
        assert!(o.is("active", "4"), "and on to the next: {key:?} {o:?}");
        let o = tap_typed(Place::Loupe, key, physical, NONE);
        assert!(o.get("active").is_none(), "{key:?} without Shift stays");
    }
}

#[test]
fn the_number_row_rates_by_its_place_and_cmd_keeps_what_the_key_types() {
    // AZERTY: `'` on the 4 key rates 4, `-` on the 6 key labels Red.
    let o = tap_typed(Place::Develop, Key::Quote, Key::Num4, NONE);
    assert!(o.is("photos", &edited(&[3], "r4f0_")), "{o:?}");
    let o = tap_typed(Place::Develop, Key::Minus, Key::Num6, NONE);
    assert!(o.is("photos", &edited(&[3], "r0f0R")), "{o:?}");
    assert!(o.get("recipe").is_none(), "not a Basic step");
    // With Cmd: a virtual copy, and zooming out.
    let o = tap_typed(Place::Develop, Key::Quote, Key::Num4, cmd());
    assert!(o.get("photos").is_some_and(|p| p.contains("new")), "{o:?}");
    let mut f = Fixture::new(Place::Develop);
    f.tap(Key::Equals, cmd());
    let before = f.fingerprint();
    let left = f.frame_watching(tap_events(Key::Minus, Key::Num6, cmd()), Some(Key::Minus));
    let o = Outcome::of(&before, f.fingerprint(), left);
    assert!(
        o.get("photos").is_none() && o.get("zoom").is_some(),
        "{o:?}"
    );
    // Dvorak types `]` on the `=` key: a rating, not a Basic step.
    let o = tap_typed(Place::Develop, Key::CloseBracket, Key::Equals, NONE);
    assert!(o.is("photos", &edited(&[3], "r1f0_")), "{o:?}");
    assert!(o.get("recipe").is_none(), "{o:?}");
}

#[test]
fn a_tool_that_takes_a_metadata_key_keeps_it() {
    // A brush's size, not the rating.
    for place in [Place::Remove, Place::RedEye, Place::Mask] {
        for key in [Key::OpenBracket, Key::CloseBracket] {
            let o = tap(place, key, NONE);
            assert!(o.get("photos").is_none(), "{place:?} {key:?} {o:?}");
        }
    }
    // Crop swaps the crop with X, and Shift+X neither swaps nor rejects.
    let o = tap(Place::Crop, Key::X, NONE);
    assert!(
        o.get("photos").is_none() && o.get("recipe").is_some(),
        "{o:?}"
    );
    assert!(tap(Place::Crop, Key::X, SHIFT).nothing());
    // Other tools leave [ ] and X to the rating.
    assert!(tap(Place::Guided, Key::CloseBracket, NONE).is("photos", &edited(&[3], "r1f0_")));
    assert!(tap(Place::Remove, Key::X, NONE).is("photos", &edited(&[3], "r0f-1_")));
}

#[test]
fn only_a_frames_first_metadata_key_applies() {
    let o = taps(Place::Grid, &[(Key::Num3, NONE), (Key::Num6, NONE)]);
    assert!(o.is("photos", &edited(&[1, 2, 3], "r3f0_")), "{o:?}");
    let o = taps(Place::Develop, &[(Key::P, NONE), (Key::Num4, NONE)]);
    assert!(o.is("photos", &edited(&[3], "r0f1_")), "{o:?}");
}

// ---------------------------------------------------------------------------
// The Library's views.

#[test]
fn grid_keys_open_views_cycle_cells_and_act_on_the_selection() {
    for (key, m, field, value) in [
        (Key::E, NONE, "view", "loupe"),
        (Key::E, SHIFT, "view", "loupe"),
        (Key::Enter, NONE, "view", "loupe"),
        (Key::C, NONE, "view", "compare"),
        (Key::N, NONE, "view", "survey"),
        (Key::J, NONE, "cells", "expanded"),
        (Key::Slash, NONE, "active", "2"),
        (Key::ArrowLeft, NONE, "active", "2"),
        (Key::ArrowRight, NONE, "active", "4"),
        (Key::Home, NONE, "active", "0"),
        (Key::End, NONE, "active", "7"),
        (Key::PageUp, NONE, "active", "0"),
        (Key::PageDown, NONE, "active", "7"),
        (Key::ArrowRight, SHIFT, "selected", "1,2,3,4"),
        (Key::Home, SHIFT, "selected", "0,1"),
        (Key::A, cmd(), "selected", "0,1,2,3,4,5,6,7"),
        (Key::D, cmd(), "selected", ""),
        (Key::F2, NONE, "dialog", "File Ordering"),
        (
            Key::Delete,
            NONE,
            "dialog",
            "File Planning Delete { rejected: false }",
        ),
        (
            Key::Backspace,
            NONE,
            "dialog",
            "File Planning Delete { rejected: false }",
        ),
        (Key::Delete, ALT, "dialog", "File Planning Remove"),
        (
            Key::Delete,
            cmd(),
            "dialog",
            "File Planning Delete { rejected: true }",
        ),
        (
            Key::Delete,
            cmd_alt_shift(),
            "dialog",
            "File Planning Trash",
        ),
        (Key::L, cmd(), "filters", "off"),
    ] {
        let o = tap(Place::Grid, key, m);
        assert!(o.is(field, value), "{key:?} {m:?} {o:?}");
        // Not while typing, behind a window or under a menu (Cmd+L excepted).
        if !(key == Key::L) {
            assert!(
                tap(Place::GridTyping, key, m).get(field).is_none(),
                "{key:?} {m:?}"
            );
        }
        assert!(
            tap(Place::GridModal, key, m).get(field).is_none(),
            "{key:?} {m:?}"
        );
    }
    // Quick Collection: B toggles the selection, Cmd+B shows it, Cmd+Shift+B
    // clears it; Alt+B and Shift+B do nothing.
    let o = tap(Place::Grid, Key::B, NONE);
    assert!(
        o.is(
            "photos",
            &photos_with(&[(1, "r0f0_q"), (2, "r0f0_q"), (3, "r0f0_q")])
        ),
        "{o:?}"
    );
    assert!(tap(Place::Grid, Key::B, cmd()).is("source", "Quick Collection"));
    assert!(tap(Place::Grid, Key::B, cmd_shift()).is("photos", &photos_with(&[(0, "r2f1_")])));
    assert!(tap(Place::Grid, Key::B, ALT).nothing());
    assert!(tap(Place::Grid, Key::B, SHIFT).nothing());
    // Held: J, B, Delete and F2 act once; the arrows move on.
    for key in [Key::J, Key::B, Key::Delete, Key::F2] {
        assert!(repeat(Place::Grid, key, NONE).nothing(), "{key:?}");
    }
    assert!(repeat(Place::Grid, Key::ArrowRight, NONE).is("active", "4"));
    // Modifiers the keys refuse.
    assert!(tap(Place::Grid, Key::C, SHIFT).get("view").is_none());
    assert!(tap(Place::Grid, Key::J, SHIFT).get("cells").is_none());
    assert!(tap(Place::Grid, Key::E, cmd()).get("view").is_none());
    assert!(
        tap(Place::Grid, Key::ArrowRight, cmd())
            .get("active")
            .is_none()
    );
}

#[test]
fn stack_keys_group_and_ungroup_the_selection() {
    let mut f = Fixture::new(Place::Grid);
    let o = tap_in(&mut f, Key::G, cmd());
    assert!(
        o.get("photos").is_some_and(|p| p.contains('s')),
        "Cmd+G stacks: {o:?}"
    );
    let o = tap_in(&mut f, Key::S, NONE);
    assert!(o.get("shown").is_some(), "S collapses the stack: {o:?}");
    let o = tap_in(&mut f, Key::G, cmd_shift());
    assert!(o.get("photos").is_some(), "Cmd+Shift+G unstacks: {o:?}");
    assert!(tap(Place::Grid, Key::G, cmd_alt()).get("photos").is_none());
    assert!(repeat(Place::Grid, Key::G, cmd()).get("photos").is_none());
    // With no stack, S says so.
    assert!(
        tap(Place::Loupe, Key::S, NONE)
            .get("message")
            .is_some_and(|m| m.contains("stack"))
    );
    assert!(tap(Place::Grid, Key::S, ALT).nothing());
}

#[test]
fn cmd_l_toggles_the_filters_from_a_field_too_but_not_under_a_menu() {
    for place in [
        Place::Grid,
        Place::Loupe,
        Place::GridTyping,
        Place::GridButton,
    ] {
        assert!(tap(place, Key::L, cmd()).is("filters", "off"), "{place:?}");
    }
    assert!(
        tap(Place::LoupePopup, Key::L, cmd())
            .get("filters")
            .is_none()
    );
    assert!(
        tap(Place::Grid, Key::L, cmd_shift())
            .get("filters")
            .is_none(),
        "exact"
    );
    assert!(repeat(Place::Grid, Key::L, cmd()).get("filters").is_none());
    assert!(
        tap(Place::Develop, Key::L, cmd()).get("filters").is_none(),
        "Library only"
    );
}

#[test]
fn loupe_keys_move_one_photo_and_act_on_the_photo_shown() {
    for (key, m, field, value) in [
        (Key::ArrowLeft, NONE, "active", "2"),
        (Key::ArrowUp, NONE, "active", "2"),
        (Key::ArrowRight, NONE, "active", "4"),
        (Key::ArrowDown, NONE, "active", "4"),
        (Key::Home, NONE, "active", "0"),
        (Key::End, NONE, "active", "7"),
        (Key::C, NONE, "view", "compare"),
        (Key::N, NONE, "view", "survey"),
        (Key::I, NONE, "info", "Info1"),
        (Key::F2, NONE, "dialog", "File Ordering"),
        (
            Key::Delete,
            NONE,
            "dialog",
            "File Planning Delete { rejected: false }",
        ),
        (Key::Z, NONE, "zoom", "on 1 at [0.5, 0.5]"),
        (Key::Z, SHIFT, "magnifier", "true"),
    ] {
        let o = tap(Place::Loupe, key, m);
        assert!(o.is(field, value), "{key:?} {m:?} {o:?}");
        assert!(
            tap(Place::LoupePopup, key, m).get(field).is_none(),
            "under a menu: {key:?}"
        );
    }
    let o = tap(Place::Loupe, Key::B, NONE);
    assert!(o.is("photos", &photos_with(&[(3, "r0f0_q")])), "{o:?}");
    // Shift does not extend in the Loupe, and Cmd does not move.
    assert!(tap(Place::Loupe, Key::ArrowRight, SHIFT).is("selected", "4"));
    assert!(
        tap(Place::Loupe, Key::ArrowRight, cmd())
            .get("active")
            .is_none()
    );
    assert!(repeat(Place::Loupe, Key::I, NONE).nothing());
    assert!(repeat(Place::Loupe, Key::Z, NONE).nothing());
}

#[test]
fn the_loupe_and_develop_share_the_zoom_keys() {
    for place in [Place::Loupe, Place::Develop] {
        let zoom = |key, m| tap(place, key, m).get("zoom").map(str::to_string);
        assert!(
            zoom(Key::Num0, cmd()).is_some_and(|z| z.starts_with("on")),
            "Cmd+0 100%"
        );
        assert!(zoom(Key::Num0, cmd_alt()).is_some(), "Cmd+Alt+0 too");
        assert!(zoom(Key::Num0, cmd_shift()).is_none());
        assert!(zoom(Key::Equals, cmd()).is_some(), "Cmd+= zooms in");
        assert!(zoom(Key::Plus, cmd()).is_some());
        // Cmd+- zooms out, down to Fit and no further.
        assert!(zoom(Key::Minus, cmd()).is_none(), "already at Fit");
        let mut f = Fixture::new(place);
        f.tap(Key::Equals, cmd());
        assert!(tap_in(&mut f, Key::Minus, cmd()).get("zoom").is_some());
        assert!(zoom(Key::Z, NONE).is_some());
        assert!(zoom(Key::Z, ALT).is_none());
    }
    // Cmd+1 fits a zoomed photo.
    let mut f = Fixture::new(Place::Develop);
    f.tap(Key::Z, NONE);
    assert!(f.e.view.zoom.on);
    let o = tap_in(&mut f, Key::Num1, cmd());
    assert!(o.get("zoom").is_some_and(|z| z.starts_with("off")), "{o:?}");
    assert!(tap(Place::Grid, Key::Z, NONE).nothing(), "not in the grid");
}

#[test]
fn compare_keys_step_the_candidate_and_swap() {
    for (key, field, value) in [
        // Compare starts with 3 the select and 1 the candidate.
        (Key::ArrowLeft, "compared", "3 0 Candidate"),
        (Key::ArrowRight, "compared", "3 2 Candidate"),
        // Up makes the candidate the select, and the next one the candidate.
        (Key::ArrowUp, "compared", "1 2 Candidate"),
        // Down swaps them.
        (Key::ArrowDown, "compared", "1 3 Candidate"),
        (Key::E, "view", "loupe"),
        (Key::Enter, "view", "loupe"),
        (Key::N, "view", "survey"),
    ] {
        let o = tap(Place::Compare, key, NONE);
        assert!(o.is(field, value), "{key:?} {o:?}");
        assert!(
            tap(Place::Compare, key, cmd()).get(field).is_none(),
            "{key:?} with Cmd"
        );
        assert!(
            tap(Place::Compare, key, ALT).get(field).is_none(),
            "{key:?} with Alt"
        );
    }
    assert!(
        repeat(Place::Compare, Key::ArrowUp, NONE)
            .get("compared")
            .is_none()
    );
    assert!(
        repeat(Place::Compare, Key::ArrowRight, NONE)
            .get("compared")
            .is_some()
    );
    assert!(tap(Place::Compare, Key::N, SHIFT).get("view").is_none());
    // B acts on the active photo, whatever the modifiers.
    let o = tap(Place::Compare, Key::B, NONE);
    assert!(o.is("photos", &photos_with(&[(3, "r0f0_q")])), "{o:?}");
    assert!(tap(Place::Compare, Key::B, cmd()).is("source", "Quick Collection"));
}

#[test]
fn survey_keys_step_the_active_photo() {
    for (key, field, value) in [
        (Key::ArrowLeft, "active", "2"),
        (Key::ArrowUp, "active", "2"),
        (Key::E, "view", "loupe"),
        (Key::Enter, "view", "loupe"),
        (Key::C, "view", "compare"),
    ] {
        let o = tap(Place::Survey, key, NONE);
        assert!(o.is(field, value), "{key:?} {o:?}");
        assert!(
            tap(Place::Survey, key, cmd()).get(field).is_none(),
            "{key:?} with Cmd"
        );
    }
    // At the last photo surveyed, Right and Down stay; after Left they come back.
    for key in [Key::ArrowRight, Key::ArrowDown] {
        assert!(tap(Place::Survey, key, NONE).nothing(), "{key:?}");
        let mut f = Fixture::new(Place::Survey);
        f.tap(Key::ArrowLeft, NONE);
        assert!(tap_in(&mut f, key, NONE).is("active", "3"), "{key:?}");
    }
    let o = tap(Place::Survey, Key::B, NONE);
    assert!(o.is("photos", &photos_with(&[(3, "r0f0_q")])), "{o:?}");
}

#[test]
fn library_export_sync_and_auto_settings_keys() {
    assert!(tap(Place::Grid, Key::E, cmd_shift()).is("dialog", "Export"));
    // No previous export in an empty data folder: the dialog.
    assert!(tap(Place::Grid, Key::E, cmd_alt_shift()).is("dialog", "Export"));
    assert!(
        repeat(Place::Grid, Key::E, cmd_shift())
            .get("dialog")
            .is_none()
    );
    let o = tap(Place::Grid, Key::S, cmd_shift());
    assert!(o.is("dialog", "Sync") && o.taken, "{o:?}");
    let o = tap(Place::Grid, Key::U, cmd_shift());
    assert!(
        o.get("busy").is_some() && o.taken,
        "Auto Settings runs: {o:?}"
    );
    for (key, m) in [
        (Key::E, cmd_shift()),
        (Key::S, cmd_shift()),
        (Key::U, cmd_shift()),
    ] {
        assert_inert_behind_a_window_and_while_typing(key, m, "a Library command");
    }
}

// ---------------------------------------------------------------------------
// Develop.

#[test]
fn develop_keys_change_the_view() {
    for (key, m, field, value) in [
        (Key::Y, NONE, "compare", "SideBySide(LeftRight)"),
        (Key::Y, ALT, "compare", "SideBySide(TopBottom)"),
        (Key::Y, SHIFT, "compare", "Split(LeftRight)"),
        (Key::Backslash, NONE, "compare", "BeforeOnly"),
        (Key::Backslash, SHIFT, "compare", "CameraThumbnail"),
        (Key::Pipe, NONE, "compare", "CameraThumbnail"),
        (Key::R, SHIFT, "compare", "Reference(LeftRight)"),
        (Key::I, NONE, "info", "Info1"),
        (Key::J, NONE, "clipping", "shadows true highlights true"),
        (Key::C, NONE, "tool", "Crop"),
        (Key::C, SHIFT, "tool", "Crop"),
        (Key::R, NONE, "tool", "Crop"),
        (Key::W, NONE, "tool", "WhiteBalance"),
        (Key::Q, NONE, "tool", "Remove"),
        (Key::W, SHIFT, "tool", "Mask"),
        (Key::T, SHIFT, "tool", "Guided"),
        (Key::ArrowRight, NONE, "open", "4"),
        (Key::ArrowLeft, NONE, "open", "2"),
        (Key::ArrowRight, SHIFT, "open", "4"),
        (Key::A, cmd(), "selected", "0,1,2,3,4,5,6,7"),
        (Key::D, cmd(), "selected", ""),
        (
            Key::Delete,
            NONE,
            "dialog",
            "File Planning Delete { rejected: false }",
        ),
        (Key::Delete, ALT, "dialog", "File Planning Remove"),
    ] {
        let o = tap(Place::Develop, key, m);
        assert!(o.is(field, value), "{key:?} {m:?} {o:?}");
        assert!(
            tap(Place::DevelopTyping, key, m).get(field).is_none(),
            "typing {key:?}"
        );
        assert!(
            tap(Place::DevelopModal, key, m).get(field).is_none(),
            "modal {key:?}"
        );
        assert!(
            tap(Place::DevelopButton, key, m).is(field, value),
            "a button {key:?}"
        );
    }
    // Under a menu every Develop key but Delete still acts.
    assert!(
        tap(Place::DevelopPopup, Key::J, NONE)
            .get("clipping")
            .is_some()
    );
    assert!(
        tap(Place::DevelopPopup, Key::Delete, NONE)
            .get("dialog")
            .is_none()
    );
    // Held: Y, I and Delete act once; the arrows and J repeat.
    for key in [Key::Y, Key::I, Key::Delete] {
        assert!(repeat(Place::Develop, key, NONE).nothing(), "{key:?}");
    }
    assert!(
        repeat(Place::Develop, Key::J, NONE)
            .get("clipping")
            .is_some()
    );
    assert!(repeat(Place::Develop, Key::ArrowRight, NONE).is("open", "4"));
    // Modifiers they refuse.
    assert!(tap(Place::Develop, Key::Y, cmd()).get("compare").is_none());
    assert!(tap(Place::Develop, Key::C, cmd()).get("tool").is_none());
    assert!(tap(Place::Develop, Key::R, cmd()).get("tool").is_none());
    assert!(tap(Place::Develop, Key::Q, SHIFT).get("tool").is_none());
    assert!(tap(Place::Develop, Key::J, ALT).get("clipping").is_none());
    assert!(tap(Place::Develop, Key::I, SHIFT).get("info").is_none());
    // Delete is the tools' own while one is open.
    assert!(tap(Place::Crop, Key::Delete, NONE).get("dialog").is_none());
}

#[test]
fn develop_keys_change_the_edit() {
    for (key, m, step) in [
        (Key::V, NONE, "Convert to Black & White"),
        (Key::V, cmd_shift(), "Paste Settings"),
        (Key::V, cmd_alt(), "Paste from Previous"),
    ] {
        let o = tap(Place::Develop, key, m);
        assert!(
            o.get("history").is_some_and(|h| h.contains(step)),
            "{key:?} {o:?}"
        );
        assert!(o.get("recipe").is_some());
        assert_inert_behind_a_window_and_while_typing(key, m, step);
    }
    assert!(
        repeat(Place::Develop, Key::V, NONE).nothing(),
        "V once per press"
    );
    assert!(repeat(Place::Develop, Key::V, cmd_alt()).nothing());
    assert!(
        repeat(Place::Develop, Key::V, cmd_shift())
            .get("recipe")
            .is_some()
    );
    let o = tap(Place::Develop, Key::R, cmd_shift());
    assert!(o.get("recipe").is_some(), "Reset: {o:?}");
    // Copy After's settings to Before, Before's to After, and Swap.
    let o = tap(Place::Develop, Key::ArrowLeft, cmd_alt_shift());
    assert!(
        o.get("before").is_some() && o.get("open").is_none(),
        "{o:?}"
    );
    let o = tap(Place::Develop, Key::ArrowRight, cmd_alt_shift());
    assert!(
        o.get("recipe").is_some() && o.get("open").is_none(),
        "{o:?}"
    );
    let o = tap(Place::Develop, Key::ArrowUp, cmd_alt_shift());
    assert!(o.get("recipe").is_some(), "{o:?}");
    // Undo and redo in Develop share the Library's log.
    assert!(tap(Place::Develop, Key::Z, cmd()).get("photos").is_some());
}

#[test]
fn develop_keys_open_windows_and_tools() {
    for (key, m, field, value) in [
        (Key::C, cmd_shift(), "dialog", "Copy"),
        (Key::N, cmd_shift(), "dialog", "NewPreset"),
        (Key::S, cmd_shift(), "dialog", "Sync"),
        (Key::E, cmd_shift(), "dialog", "Export"),
        (Key::E, cmd_alt_shift(), "dialog", "Export"),
        (Key::T, cmd_alt_shift(), "tool", "Targeted(ToneCurve)"),
        (Key::H, cmd_alt_shift(), "tool", "Targeted(Hsl(Hue))"),
        (Key::S, cmd_alt_shift(), "tool", "Targeted(Hsl(Saturation))"),
        (Key::L, cmd_alt_shift(), "tool", "Targeted(Hsl(Luminance))"),
    ] {
        let o = tap(Place::Develop, key, m);
        assert!(o.is(field, value), "{key:?} {m:?} {o:?}");
        assert!(tap(Place::DevelopTyping, key, m).get(field).is_none());
        assert!(tap(Place::DevelopModal, key, m).get(field).is_none());
    }
    // B&W's targeted tool refuses a colour photo, with a message.
    let o = tap(Place::Develop, Key::G, cmd_alt_shift());
    assert!(
        o.get("tool").is_none() && o.get("message").is_some(),
        "{o:?}"
    );
    // Match Total Exposures needs exposure data a JPEG lacks, and says so; Auto
    // Settings starts work.
    let o = tap(Place::Develop, Key::M, cmd_alt_shift());
    assert!(
        o.get("message")
            .is_some_and(|m| m.starts_with("Match Total Exposures")),
        "{o:?}"
    );
    assert!(
        tap(Place::Develop, Key::U, cmd_shift())
            .get("busy")
            .is_some()
    );
}

#[test]
fn enter_puts_crop_and_guided_away() {
    assert!(tap(Place::Crop, Key::Enter, NONE).is("tool", "None"));
    assert!(tap(Place::Crop, Key::Enter, cmd()).is("tool", "None"));
    assert!(tap(Place::Guided, Key::Enter, NONE).is("tool", "None"));
    assert!(tap(Place::Remove, Key::Enter, NONE).get("tool").is_none());
}

#[test]
fn mask_keys_make_new_masks() {
    let o = tap(Place::Develop, Key::K, NONE);
    assert!(
        o.is("tool", "Mask") && o.get("history").is_some_and(|h| h.contains("Brush")),
        "{o:?}"
    );
    for (key, m) in [(Key::M, NONE), (Key::M, SHIFT), (Key::J, SHIFT)] {
        let o = tap(Place::Develop, key, m);
        assert!(o.is("tool", "Mask"), "{key:?} {m:?} {o:?}");
        assert!(
            o.get("mask").is_some_and(|m| m.contains("placing true")),
            "{o:?}"
        );
    }
    assert!(tap(Place::Develop, Key::K, SHIFT).get("tool").is_none());
    assert!(tap(Place::Develop, Key::M, ALT).get("tool").is_none());
}

#[test]
fn crop_keys() {
    let o = tap(Place::Crop, Key::X, NONE);
    assert!(
        o.get("recipe").is_some() && o.get("aspect").is_some(),
        "swap: {o:?}"
    );
    assert!(
        tap(Place::Crop, Key::A, NONE).get("aspect").is_some(),
        "aspect lock"
    );
    assert!(tap(Place::Crop, Key::A, SHIFT).get("aspect").is_none());
    assert!(
        tap(Place::Crop, Key::O, NONE)
            .is("guides", "{ guide: Diagonal, orientation: 0, show: Auto }")
    );
    // Shift+O turns a guide that has a way round; Thirds has none.
    assert!(tap(Place::Crop, Key::O, SHIFT).nothing());
    let mut f = Fixture::new(Place::Crop);
    f.taps(&[(Key::O, NONE), (Key::O, NONE)]);
    let o = tap_in(&mut f, Key::O, SHIFT);
    assert!(
        o.is("guides", "{ guide: Triangle, orientation: 1, show: Auto }"),
        "{o:?}"
    );
    assert!(
        repeat(Place::Crop, Key::O, NONE).get("guides").is_some(),
        "repeats"
    );
    for m in [cmd(), ALT] {
        assert!(tap(Place::Crop, Key::O, m).get("guides").is_none(), "{m:?}");
    }
    assert!(tap(Place::DevelopTyping, Key::O, NONE).nothing());
}

#[test]
fn remove_keys() {
    let o = tap(Place::Remove, Key::CloseBracket, NONE);
    assert!(
        o.get("remove").is_some_and(|r| r.contains("size 0.0")) && o.get("recipe").is_some(),
        "{o:?}"
    );
    assert!(
        tap(Place::Remove, Key::OpenBracket, NONE)
            .get("remove")
            .is_some()
    );
    assert!(
        tap(Place::Remove, Key::OpenCurlyBracket, NONE)
            .get("remove")
            .is_some()
    );
    let o = tap(Place::Remove, Key::CloseBracket, SHIFT);
    assert!(
        o.get("remove").is_some_and(|r| r.contains("feather 0.6")),
        "feather: {o:?}"
    );
    assert!(
        tap(Place::Remove, Key::Slash, NONE).get("recipe").is_some(),
        "a new source"
    );
    let o = tap(Place::Remove, Key::Delete, NONE);
    assert!(
        o.get("remove").is_some_and(|r| r.contains("selected None")) && o.get("recipe").is_some()
    );
    assert!(
        tap(Place::Remove, Key::H, NONE)
            .get("remove")
            .is_some_and(|r| r.contains("pins hidden"))
    );
    assert!(tap(Place::Remove, Key::H, SHIFT).get("remove").is_none());
    assert!(
        tap(Place::Remove, Key::A, NONE)
            .get("remove")
            .is_some_and(|r| r.contains("visualize true"))
    );
    assert!(tap(Place::Remove, Key::A, SHIFT).get("remove").is_none());
    for m in [cmd(), ALT] {
        assert!(
            tap(Place::Remove, Key::CloseBracket, m)
                .get("remove")
                .is_none()
        );
    }
}

#[test]
fn red_eye_and_guided_keys() {
    let o = tap(Place::RedEye, Key::Delete, NONE);
    assert!(
        o.get("red eye")
            .is_some_and(|r| r.contains("selected None"))
            && o.get("recipe").is_some(),
        "{o:?}"
    );
    assert!(
        tap(Place::RedEye, Key::CloseBracket, NONE)
            .get("red eye")
            .is_some()
    );
    assert!(
        tap(Place::RedEye, Key::OpenBracket, NONE)
            .get("red eye")
            .is_some()
    );
    assert!(tap(Place::RedEye, Key::Delete, ALT).get("recipe").is_none());
    let o = tap(Place::Guided, Key::Delete, NONE);
    assert!(
        o.get("recipe").is_some() && o.get("history").is_some(),
        "{o:?}"
    );
    assert!(
        tap(Place::Guided, Key::Backspace, NONE)
            .get("recipe")
            .is_some()
    );
    assert!(
        tap(Place::Guided, Key::Delete, cmd())
            .get("recipe")
            .is_none()
    );
}

#[test]
fn masking_keys() {
    let o = tap(Place::Mask, Key::CloseBracket, NONE);
    assert!(
        o.get("mask").is_some_and(|m| m.contains("size 0.0345")),
        "{o:?}"
    );
    let o = tap(Place::Mask, Key::OpenBracket, SHIFT);
    assert!(
        o.get("mask").is_some_and(|m| m.contains("feather 0.4")),
        "{o:?}"
    );
    assert!(
        tap(Place::Mask, Key::O, NONE)
            .get("mask")
            .is_some_and(|m| m.contains("overlay false"))
    );
    assert!(tap(Place::Mask, Key::O, SHIFT).get("mask").is_none());
    assert!(
        tap(Place::Mask, Key::Slash, NONE)
            .get("mask")
            .is_some_and(|m| m.starts_with("brush 1"))
    );
    let o = tap(Place::Mask, Key::Delete, NONE);
    assert!(
        o.get("recipe").is_some() && o.get("mask").is_some(),
        "{o:?}"
    );
    assert!(
        tap(Place::Mask, Key::CloseBracket, cmd())
            .get("mask")
            .is_none()
    );
}

#[test]
fn basic_keys_select_and_step_the_selected_slider() {
    assert!(tap(Place::Develop, Key::Period, NONE).is("slider", "Some(Contrast)"));
    assert!(tap(Place::Develop, Key::Comma, NONE).is("slider", "Some(Tint)"));
    assert!(
        tap(Place::Develop, Key::Period, SHIFT)
            .get("slider")
            .is_none()
    );
    let step = |key, m| {
        tap(Place::Develop, key, m)
            .get("recipe")
            .map(str::to_string)
    };
    let up = step(Key::Equals, NONE);
    assert!(up.is_some());
    assert_eq!(step(Key::Plus, NONE), up, "+ is =");
    assert!(
        step(Key::Equals, SHIFT).is_some_and(|r| Some(&r) != up.as_ref()),
        "large"
    );
    assert!(
        step(Key::Equals, ALT).is_some_and(|r| Some(&r) != up.as_ref()),
        "fine"
    );
    assert!(step(Key::Minus, NONE).is_some());
    assert!(step(Key::Minus, cmd()).is_none(), "Cmd+- zooms");
    // Held, it steps again; a tool open does not stop it; typing does.
    assert!(
        repeat(Place::Develop, Key::Equals, NONE)
            .get("recipe")
            .is_some()
    );
    assert!(tap(Place::Crop, Key::Equals, NONE).get("recipe").is_some());
    assert!(tap(Place::DevelopTyping, Key::Equals, NONE).nothing());
    assert!(
        tap(Place::DevelopModal, Key::Equals, NONE)
            .get("recipe")
            .is_none()
    );
    // Not in the Library.
    assert!(tap(Place::Loupe, Key::Equals, NONE).nothing());
}

// ---------------------------------------------------------------------------
// The Escape chain and the keys that change module.

#[test]
fn escape_is_taken_in_order_messages_chrome_menu_view_tool() {
    // Shift+Esc with messages: only the messages.
    let mut f = Fixture::new(Place::Crop);
    f.tap(Key::F, NONE);
    let o = tap_in(&mut f, Key::Escape, SHIFT);
    assert!(
        o.is("messages", "0") && o.get("chrome").is_none() && o.get("tool").is_none(),
        "{o:?}"
    );
    // Then plain Escape: the chrome, then the tool, then nothing.
    assert!(tap_in(&mut f, Key::Escape, NONE).get("chrome").is_some());
    assert!(tap_in(&mut f, Key::Escape, NONE).is("tool", "None"));
    assert!(tap_in(&mut f, Key::Escape, NONE).nothing());
    // In the Library: the chrome, then the Loupe, then nothing.
    let mut f = Fixture::new(Place::Loupe);
    f.tap(Key::F, NONE);
    assert!(tap_in(&mut f, Key::Escape, NONE).get("chrome").is_some());
    assert!(tap_in(&mut f, Key::Escape, NONE).is("view", "grid"));
    // Behind Preferences, Escape closes Preferences alone.
    let o = tap(Place::DevelopModal, Key::Escape, NONE);
    assert!(o.is("dialog", "-") && o.get("tool").is_none(), "{o:?}");
}

#[test]
fn a_key_that_changes_module_does_not_act_again_in_the_new_one() {
    // D from the grid: Develop, and D is nothing more there.
    let o = tap(Place::Grid, Key::D, NONE);
    assert!(
        o.is("module", "Develop") && o.get("selected").is_none(),
        "{o:?}"
    );
    // G from Develop: the grid, without Cmd+G's stack or anything else.
    let o = tap(Place::Develop, Key::G, NONE);
    assert!(
        o.is("module", "Library") && o.get("photos").is_none(),
        "{o:?}"
    );
    // E from Develop: the Loupe, not E's other uses there.
    let o = tap(Place::Develop, Key::E, NONE);
    assert!(o.is("view", "loupe") && o.get("dialog").is_none(), "{o:?}");
    // Cmd+Z in the Library is taken there: Develop's keys never see it.
    let mut f = Fixture::new(Place::Develop);
    f.tap(Key::V, NONE);
    f.tap(Key::G, NONE);
    assert_eq!(f.e.module, Module::Library);
    let o = tap_in(&mut f, Key::Z, cmd());
    assert!(o.taken, "{o:?}");
    let (_, applied) = f.e.document.edit.history().steps();
    assert_eq!(applied, 0, "one step undone: {o:?}");
}

// ---------------------------------------------------------------------------
// Several presses in one frame.

#[test]
fn several_presses_in_one_frame() {
    // E then J in the grid: the Loupe, and the cells' style changes.
    let o = taps(Place::Grid, &[(Key::E, NONE), (Key::J, NONE)]);
    assert!(o.is("view", "loupe") && o.is("cells", "expanded"), "{o:?}");
    // Q then C, and C then Q: Remove ends active either way.
    for keys in [
        [(Key::Q, NONE), (Key::C, NONE)],
        [(Key::C, NONE), (Key::Q, NONE)],
    ] {
        assert!(taps(Place::Develop, &keys).is("tool", "Remove"), "{keys:?}");
    }
    // F7 then Tab: Tab wins.
    let o = taps(Place::Develop, &[(Key::F7, NONE), (Key::Tab, NONE)]);
    assert!(
        o.is("panels", "develop lrFpA library LRFPA tab Presets"),
        "{o:?}"
    );
    // F twice: the chrome changes once.
    let o = taps(Place::Develop, &[(Key::F, NONE), (Key::F, NONE)]);
    assert!(o.get("chrome").is_some(), "{o:?}");
    // R and C: Crop toggles once.
    assert!(taps(Place::Develop, &[(Key::R, NONE), (Key::C, NONE)]).is("tool", "Crop"));
    // Two metadata keys: only the first.
    let o = taps(Place::Loupe, &[(Key::Num2, NONE), (Key::X, NONE)]);
    assert!(o.is("photos", &edited(&[3], "r2f0_")), "{o:?}");
    // Q then ]: Remove opens; the ] is a rating, taken before Remove saw it.
    let o = taps(Place::Develop, &[(Key::Q, NONE), (Key::CloseBracket, NONE)]);
    assert!(
        o.is("tool", "Remove") && o.is("photos", &edited(&[3], "r1f0_")),
        "{o:?}"
    );
    assert!(o.get("remove").is_none(), "{o:?}");
    // Right then Left in the grid: back on the first photo.
    let o = taps(
        Place::Grid,
        &[(Key::ArrowRight, NONE), (Key::ArrowLeft, NONE)],
    );
    assert!(o.is("selected", "3") && o.get("active").is_none(), "{o:?}");
    // Minus then Period with a Basic slider selected: Exposure steps down, then
    // Contrast is selected.
    let o = taps(Place::Develop, &[(Key::Minus, NONE), (Key::Period, NONE)]);
    assert!(o.is("slider", "Some(Contrast)"), "{o:?}");
    assert!(
        o.get("history").is_some_and(|h| h.contains("Exposure")),
        "{o:?}"
    );
}

/// Today one press can act twice when another press in its frame changes
/// module, view or tool between two steps that both read it. The command
/// registry makes the first step to take a press its only owner, so these pins
/// are expected to change with it.
#[test]
fn one_press_acts_once_across_a_module_change() {
    // Each press runs one command, in the step that takes it first; a module
    // changed by another press in the frame doesn't give it a second one.
    // D then J: Develop from the grid, and J restyles the grid's cells only,
    // where it once also turned on Develop's clipping warnings.
    let o = taps(Place::Grid, &[(Key::D, NONE), (Key::J, NONE)]);
    assert!(o.is("module", "Develop"), "{o:?}");
    assert!(o.is("cells", "expanded"), "{o:?}");
    assert!(o.get("clipping").is_none(), "{o:?}");
    // [ then Q in Develop: the [ is a rating key, taken before Remove opens.
    let o = taps(Place::Develop, &[(Key::OpenBracket, NONE), (Key::Q, NONE)]);
    assert!(
        o.is("tool", "Remove") && o.get("message").is_some(),
        "{o:?}"
    );
    assert!(o.get("remove").is_none(), "{o:?}");
    // D then C from the grid: C opens Compare in the grid, and no longer also
    // Crop in Develop.
    let o = taps(Place::Grid, &[(Key::D, NONE), (Key::C, NONE)]);
    assert!(
        o.is("module", "Develop") && o.is("view", "compare"),
        "{o:?}"
    );
    assert!(o.get("tool").is_none(), "{o:?}");
}

// ---------------------------------------------------------------------------
// What reaches the shortcuts from around them.

#[test]
fn copy_and_paste_with_shift_or_alt_reach_the_shortcuts_as_keys() {
    let ctx = eframe::egui::Context::default();
    let keys = |events: Vec<eframe::egui::Event>| {
        let mut input = eframe::egui::RawInput {
            events,
            ..Default::default()
        };
        super::clipboard_shortcut_keys(&ctx, &mut input);
        input
            .events
            .iter()
            .filter_map(|e| match e {
                eframe::egui::Event::Key { key, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let with = |m: Modifiers, event: eframe::egui::Event| {
        vec![eframe::egui::Event::ModifiersChanged(m), event]
    };
    use eframe::egui::Event::{Copy, Paste};
    assert_eq!(keys(with(cmd_shift(), Copy)), vec![(Key::C, cmd_shift())]);
    assert_eq!(
        keys(with(cmd_alt(), Paste(String::new()))),
        vec![(Key::V, cmd_alt())]
    );
    // Plain Cmd+C and Cmd+V stay the text field's.
    assert!(keys(with(cmd(), Copy)).is_empty());
    assert!(keys(with(cmd(), Paste(String::new()))).is_empty());
    // And then Cmd+Shift+C opens Copy Settings in Develop.
    let mut f = Fixture::new(Place::Develop);
    let mut input = eframe::egui::RawInput {
        events: with(cmd_shift(), Copy),
        ..Default::default()
    };
    super::clipboard_shortcut_keys(&f.e.context, &mut input);
    f.frame(input.events);
    assert!(matches!(f.e.modal, Some(Modal::CopySettings(_))));
}

#[test]
fn enter_confirms_copy_settings_unless_a_menu_is_open() {
    let mut f = Fixture::new(Place::Develop);
    f.e.clipboard = None;
    f.tap(Key::C, cmd_shift());
    assert!(matches!(f.e.modal, Some(Modal::CopySettings(_))));
    let o = tap_in(&mut f, Key::Enter, NONE);
    assert!(
        o.is("dialog", "-") && o.is("message", "Settings copied") && o.taken,
        "{o:?}"
    );
    assert!(f.e.clipboard.is_some());
}

#[test]
fn auto_advance_moves_on_after_every_metadata_key() {
    let mut f = Fixture::new(Place::Loupe);
    f.e.session.auto_advance = true;
    let o = tap_in(&mut f, Key::Num3, NONE);
    assert!(
        o.is("photos", &edited(&[3], "r3f0_")) && o.is("active", "4"),
        "{o:?}"
    );
}

#[test]
fn busy_work_holds_the_keyboard_shortcuts() {
    // A system file dialog open is busy work.
    let mut f = Fixture::new(Place::Grid);
    assert!(f.e.activity.begin_dialog());
    assert!(f.e.activity.is_busy());
    for (key, m) in [
        (Key::Num3, NONE),
        (Key::F, NONE),
        (Key::D, NONE),
        (Key::Tab, NONE),
        (Key::Comma, cmd()),
        (Key::Quote, cmd()),
        (Key::Z, cmd()),
    ] {
        let o = tap_in(&mut f, key, m);
        assert!(o.nothing(), "{key:?} {m:?} while busy: {o:?}");
    }
}
