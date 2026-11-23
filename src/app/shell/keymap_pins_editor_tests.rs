//! Pins of what each keyboard shortcut does: each fires in its place and not
//! outside it, with exactly its modifiers, and keeps its guards behind a window,
//! under a menu, while a field is typed in and with a button focused. Then the
//! Escape chain, frames that bring several presses at once, and other keyboard
//! layouts.
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
/// field is typed in.
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
    // With no message it is not taken, and nothing else takes Shift+Escape:
    // the Loupe's Escape is plain (#963).
    let mut f = Fixture::new(Place::Loupe);
    f.tap(Key::Escape, SHIFT);
    let o = tap_in(&mut f, Key::Escape, SHIFT);
    assert!(o.nothing() && !o.taken, "{o:?}");
    assert_eq!(f.e.messages.entry_count(), 0);
    // Shift+Escape with Alt is no longer Shift+Escape.
    let o = tap(Place::Grid, Key::Escape, ALT | SHIFT);
    assert!(o.get("messages").is_none() && !o.taken, "{o:?}");
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
    // Develop: Shift+Escape dismisses the messages alone, then Escape brings
    // the chrome back, then closes the tool, then does nothing.
    let mut f = Fixture::new(Place::Crop);
    f.tap(Key::F, NONE);
    let o = tap_in(&mut f, Key::Escape, SHIFT);
    assert!(
        o.is("messages", "0") && o.get("chrome").is_none() && o.get("tool").is_none(),
        "{o:?}"
    );
    let o = tap_in(&mut f, Key::Escape, NONE);
    assert!(
        o.get("chrome").is_some() && o.get("tool").is_none(),
        "{o:?}"
    );
    assert!(tap_in(&mut f, Key::Escape, NONE).is("tool", "None"));
    assert!(tap_in(&mut f, Key::Escape, NONE).nothing());
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
    // Escape alone (#963): it took any modifiers.
    for m in [cmd(), ALT] {
        assert!(
            tap(Place::Loupe, Key::Escape, m).get("view").is_none(),
            "{m:?}"
        );
    }
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
        // Escape alone (#963): it took any modifiers.
        for m in [cmd(), ALT] {
            assert!(
                tap(place, Key::Escape, m).get("tool").is_none(),
                "{place:?} {m:?}"
            );
        }
    }
    // Masking with nothing aimed or pending: its own chain closes it. With Cmd
    // or Alt it closed at once; now Escape is Escape alone there too (#963).
    assert!(tap(Place::Mask, Key::Escape, NONE).is("tool", "None"));
    for m in [cmd(), ALT] {
        assert!(
            tap(Place::Mask, Key::Escape, m).get("tool").is_none(),
            "{m:?}"
        );
    }
    // A text field gives Escape up; behind a window the window takes it.
    let o = tap(Place::DevelopTyping, Key::Escape, NONE);
    assert!(o.get("tool").is_none(), "{o:?}");
    let o = tap(Place::DevelopModal, Key::Escape, NONE);
    assert!(o.is("dialog", "-") && o.get("tool").is_none(), "{o:?}");
}

#[test]
fn g_shows_the_grid_from_anywhere_alone() {
    for place in [Place::Develop, Place::Crop] {
        // The grid, without Cmd+G's stack or anything else G does there.
        let o = tap(place, Key::G, NONE);
        assert!(
            o.is("module", "Library") && o.get("photos").is_none(),
            "{place:?} {o:?}"
        );
        // G alone (#963): it took Shift.
        assert!(tap(place, Key::G, SHIFT).get("module").is_none());
        assert!(
            repeat(place, Key::G, NONE).is("module", "Library"),
            "repeats"
        );
        assert!(tap(place, Key::G, cmd()).get("module").is_none());
        assert!(tap(place, Key::G, ALT).get("module").is_none());
    }
    for place in [Place::Loupe, Place::Compare, Place::Survey] {
        assert!(tap(place, Key::G, NONE).is("view", "grid"), "{place:?}");
        assert!(tap(place, Key::G, SHIFT).get("view").is_none(), "{place:?}");
    }
    assert_inert_behind_a_window_and_while_typing(Key::G, NONE, "G");
    assert!(tap(Place::DevelopButton, Key::G, NONE).is("module", "Library"));
}

#[test]
fn e_in_develop_shows_the_open_photo_in_the_loupe() {
    let mut f = Fixture::new(Place::Develop);
    let o = tap_in(&mut f, Key::E, NONE);
    assert!(
        o.is("module", "Library") && o.is("view", "loupe") && o.get("dialog").is_none(),
        "{o:?}"
    );
    assert_eq!(f.active(), Some(f.ids[3]));
    // E alone (#963): it took Shift.
    assert!(tap(Place::Develop, Key::E, SHIFT).get("module").is_none());
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
    // Develop, and D does nothing more there.
    let o = tap(Place::Grid, Key::D, NONE);
    assert!(
        o.is("module", "Develop") && o.is("open", "3") && o.get("selected").is_none(),
        "{o:?}"
    );
    // D alone (#963): it took Shift.
    assert!(tap(Place::Grid, Key::D, SHIFT).get("module").is_none());
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
    for place in [Place::Develop, Place::DevelopButton] {
        assert!(tap(place, Key::Z, cmd()).is("photos", &undone), "{place:?}");
        assert!(
            tap(place, Key::Z, cmd_shift()).is("photos", &redone),
            "{place:?}"
        );
    }
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
    // The left panel (L) and the filmstrip (F) are both modules'; the right
    // side (R, P, A) is each module's own.
    let panels = |d: &str, l: &str| format!("develop {d} library {l} tab Presets");
    for (key, m, in_library, in_develop) in [
        (
            Key::Tab,
            NONE,
            panels("lRFPA", "lrFPA"),
            panels("lrFpA", "lRFPA"),
        ),
        (
            Key::Tab,
            SHIFT,
            panels("lRfPA", "lrfPA"),
            panels("lrfpA", "lRfPA"),
        ),
        (
            Key::F7,
            NONE,
            panels("lRFPA", "lRFPA"),
            panels("lRFPA", "lRFPA"),
        ),
        (
            Key::F8,
            NONE,
            panels("LRFPA", "LrFPA"),
            panels("LrFPA", "LRFPA"),
        ),
        // The Library has no Presets panel: Shift+F8 there hides Develop's.
        (
            Key::F8,
            SHIFT,
            panels("LRFpA", "LRFPA"),
            panels("LRFpA", "LRFPA"),
        ),
        (
            Key::F6,
            NONE,
            panels("LRfPA", "LRfPA"),
            panels("LRfPA", "LRfPA"),
        ),
    ] {
        let o = tap(Place::Grid, key, m);
        assert!(
            o.is("panels", &in_library) && o.taken,
            "{key:?} {m:?} {o:?}"
        );
        // In a window too narrow for it, Shift+F8 in Develop shows the tab
        // panel over the photo rather than changing the layout.
        let changes = |o: &Outcome| {
            if key == Key::F8 && m == SHIFT {
                o.is("tab panel", "true")
            } else {
                o.is("panels", &in_develop)
            }
        };
        let o = tap(Place::Develop, key, m);
        assert!(changes(&o) && o.taken, "{key:?} {m:?} {o:?}");
        assert!(changes(&tap(Place::Crop, key, m)), "a tool open");
        // Not with Alt held too (#963): the keys matched loosely.
        let o = tap(Place::Develop, key, m | ALT);
        assert!(o.nothing() && !o.taken, "{key:?} with Alt: {o:?}");
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

// ---------------------------------------------------------------------------
// The Library's views.

#[test]
fn grid_keys_open_views_cycle_cells_and_act_on_the_selection() {
    for (key, m, field, value) in [
        (Key::E, NONE, "view", "loupe"),
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
    // Modifiers the keys refuse. E and Return alone (#963), and the arrows,
    // Page keys, Home and End with Shift only, which extends the selection.
    for m in [SHIFT, ALT] {
        assert!(tap(Place::Grid, Key::E, m).get("view").is_none(), "E {m:?}");
        assert!(
            tap(Place::Grid, Key::Enter, m).get("view").is_none(),
            "Return {m:?}"
        );
    }
    for key in [
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::ArrowUp,
        Key::ArrowDown,
        Key::PageUp,
        Key::PageDown,
        Key::Home,
        Key::End,
    ] {
        for m in [ALT, ALT | SHIFT] {
            assert!(tap(Place::Grid, key, m).nothing(), "{key:?} {m:?}");
        }
    }
    for m in [cmd_shift(), cmd_alt()] {
        assert!(tap(Place::Grid, Key::A, m).nothing(), "Cmd+A {m:?}");
        assert!(tap(Place::Grid, Key::D, m).nothing(), "Cmd+D {m:?}");
    }
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
    // The arrows, Home and End alone (#963): Shift, which extends in the
    // Grid, and Alt moved here too. Cmd does not move.
    for key in [
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::ArrowUp,
        Key::ArrowDown,
        Key::Home,
        Key::End,
    ] {
        for m in [SHIFT, ALT, cmd()] {
            assert!(
                tap(Place::Loupe, key, m).get("active").is_none(),
                "{key:?} {m:?}"
            );
        }
    }
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

/// Lightroom's Space: the grid's Space opens the Loupe, and the Loupe's and
/// Develop's toggles the zoom as Z does, unless a tool is open, where holding
/// it pans instead.
#[test]
fn space_opens_the_loupe_from_the_grid_and_toggles_the_zoom() {
    assert!(tap(Place::Grid, Key::Space, NONE).is("view", "loupe"));
    for m in [SHIFT, ALT, cmd()] {
        assert!(
            tap(Place::Grid, Key::Space, m).get("view").is_none(),
            "{m:?}"
        );
    }
    for place in [Place::Loupe, Place::Develop] {
        let o = tap(place, Key::Space, NONE);
        assert!(
            o.get("zoom").is_some_and(|z| z.starts_with("on")),
            "{place:?} {o:?}"
        );
        let mut f = Fixture::new(place);
        f.tap(Key::Space, NONE);
        let o = tap_in(&mut f, Key::Space, NONE);
        assert!(
            o.get("zoom").is_some_and(|z| z.starts_with("off")),
            "{place:?} {o:?}"
        );
        // Once per press: a held Space must not flicker the zoom.
        assert!(
            repeat(place, Key::Space, NONE).get("zoom").is_none(),
            "{place:?}"
        );
        assert!(
            tap(place, Key::Space, SHIFT).get("zoom").is_none(),
            "{place:?}"
        );
    }
    // A tool keeps Space for its hand.
    for place in [
        Place::Crop,
        Place::Remove,
        Place::RedEye,
        Place::Mask,
        Place::Guided,
    ] {
        assert!(
            tap(place, Key::Space, NONE).get("zoom").is_none(),
            "{place:?}"
        );
    }
    // A focused button keeps Space, and nothing behind a window or while typing.
    assert!(
        tap(Place::GridButton, Key::Space, NONE)
            .get("view")
            .is_none()
    );
    assert!(
        tap(Place::DevelopButton, Key::Space, NONE)
            .get("zoom")
            .is_none()
    );
    assert!(
        tap(Place::LoupePopup, Key::Space, NONE)
            .get("zoom")
            .is_none()
    );
    assert_inert_behind_a_window_and_while_typing(Key::Space, NONE, "Space");
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
        (Key::R, NONE, "tool", "Crop"),
        (Key::W, NONE, "tool", "WhiteBalance"),
        (Key::Q, NONE, "tool", "Remove"),
        (Key::W, SHIFT, "tool", "Mask"),
        (Key::T, SHIFT, "tool", "Guided"),
        (Key::ArrowRight, NONE, "open", "4"),
        (Key::ArrowLeft, NONE, "open", "2"),
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
    assert!(tap(Place::Develop, Key::R, SHIFT).get("tool").is_none());
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
    // B&W's targeted tool refuses a color photo, with a message.
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
    // Return alone (#963): it took any modifiers.
    for place in [Place::Crop, Place::Guided] {
        for m in [cmd(), SHIFT, ALT] {
            assert!(
                tap(place, Key::Enter, m).get("tool").is_none(),
                "{place:?} {m:?}"
            );
        }
    }
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

/// What `crop_tool_editor_tests` leaves out: A locks the aspect, the modifiers each
/// crop key takes, and that O repeats.
#[test]
fn crop_keys() {
    assert!(
        tap(Place::Crop, Key::A, NONE).get("aspect").is_some(),
        "aspect lock"
    );
    assert!(tap(Place::Crop, Key::A, SHIFT).get("aspect").is_none());
    // Shift+O turns a guide that has a way round; Thirds has none.
    assert!(tap(Place::Crop, Key::O, SHIFT).nothing());
    assert!(
        repeat(Place::Crop, Key::O, NONE).get("guides").is_some(),
        "repeats"
    );
    for m in [cmd(), ALT] {
        assert!(tap(Place::Crop, Key::O, m).get("guides").is_none(), "{m:?}");
    }
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
        o.is("panels", "develop lrFpA library lRFPA tab Presets"),
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

/// A press belongs to the first step that takes it, so it acts once even when
/// it, or another press in its frame, changes module, view or tool before a
/// later step reads it.
#[test]
fn one_press_acts_once_across_a_module_change() {
    // Cmd+Z after G from Develop is taken in the Library: Develop's keys never
    // see it, and one step is undone.
    let mut f = Fixture::new(Place::Develop);
    f.tap(Key::V, NONE);
    f.tap(Key::G, NONE);
    assert_eq!(f.e.module, Module::Library);
    let o = tap_in(&mut f, Key::Z, cmd());
    assert!(o.taken, "{o:?}");
    let (_, applied) = f.e.document.edit.history().steps();
    assert_eq!(applied, 0, "one step undone: {o:?}");
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

// ---------------------------------------------------------------------------
// Exact modifiers (#963): the default chords match their modifiers exactly,
// except for a command's variants and Shift and Alt on punctuation keys.

/// The extra modifiers the defaults used to accept and no longer do. Each
/// press changes nothing that the chord without them changes.
#[test]
fn extra_modifiers_no_longer_run_a_command() {
    use Place::*;
    let alt_shift = ALT | SHIFT;
    let mut cases: Vec<(Place, Key, Modifiers, &str)> = vec![
        // C and R toggle Crop alone.
        (Develop, Key::C, SHIFT, "tool"),
        (Develop, Key::C, ALT, "tool"),
        (Develop, Key::R, ALT, "tool"),
        // Develop's Left and Right alone.
        (Develop, Key::ArrowLeft, SHIFT, "open"),
        (Develop, Key::ArrowRight, ALT, "open"),
        (Develop, Key::ArrowRight, cmd(), "open"),
        // `\\`, `|` and Shift+`\\` without Cmd.
        (Develop, Key::Backslash, cmd(), "compare"),
        (Develop, Key::Backslash, cmd_shift(), "compare"),
        (Develop, Key::Pipe, cmd(), "compare"),
        // Cmd+A and Cmd+D in Develop with Shift or Alt.
        (Develop, Key::A, cmd_shift(), "selected"),
        (Develop, Key::D, cmd_alt(), "selected"),
        // Cmd+Shift+S and Cmd+Shift+U in the Library, Cmd+Shift+U in Develop,
        // with Alt.
        (Grid, Key::S, cmd_alt_shift(), "dialog"),
        (Grid, Key::U, cmd_alt_shift(), "busy"),
        (Develop, Key::U, cmd_alt_shift(), "busy"),
        // Cmd+Shift+N, R, C and V with Alt.
        (Develop, Key::N, cmd_alt_shift(), "dialog"),
        (Develop, Key::R, cmd_alt_shift(), "recipe"),
        (Develop, Key::C, cmd_alt_shift(), "dialog"),
        (Develop, Key::V, cmd_alt_shift(), "recipe"),
        // Shift+W and Shift+T with Alt.
        (Develop, Key::W, alt_shift, "tool"),
        (Develop, Key::T, alt_shift, "tool"),
        // Compare's and Survey's keys with Shift.
        (Compare, Key::E, SHIFT, "view"),
        (Compare, Key::Enter, SHIFT, "view"),
        (Survey, Key::E, SHIFT, "view"),
        (Compare, Key::ArrowLeft, SHIFT, "compared"),
        (Compare, Key::ArrowUp, SHIFT, "compared"),
        (Compare, Key::ArrowDown, SHIFT, "compared"),
        (Survey, Key::ArrowLeft, SHIFT, "active"),
        (Survey, Key::ArrowUp, SHIFT, "active"),
        // Option+Shift+Y, Delete with Shift in a tool, and Undo and Redo with
        // Alt.
        (Develop, Key::Y, alt_shift, "compare"),
        (Remove, Key::Delete, SHIFT, "recipe"),
        (RedEye, Key::Backspace, SHIFT, "recipe"),
        (Mask, Key::Delete, SHIFT, "recipe"),
        (Guided, Key::Delete, SHIFT, "recipe"),
        (Grid, Key::Z, cmd_alt(), "photos"),
        (Grid, Key::Z, cmd_alt_shift(), "photos"),
        (Develop, Key::Z, cmd_alt(), "photos"),
        (Develop, Key::Z, cmd_alt_shift(), "photos"),
        // F6 and F7 with Shift or Alt, Shift+F8 with Alt.
        (Grid, Key::F6, SHIFT, "panels"),
        (Grid, Key::F7, ALT, "panels"),
        (Develop, Key::F8, alt_shift, "tab panel"),
    ];
    if !cfg!(target_os = "macos") {
        // Ctrl+Y redoes alone.
        cases.extend([
            (Grid, Key::Y, cmd_shift(), "photos"),
            (Grid, Key::Y, cmd_alt(), "photos"),
            (Develop, Key::Y, cmd_alt(), "photos"),
        ]);
    }
    for (place, key, m, field) in cases {
        let o = tap(place, key, m);
        assert!(o.get(field).is_none(), "{place:?} {key:?} {m:?}: {o:?}");
    }
}

/// The aliases and variants the defaults keep.
#[test]
fn kept_aliases_and_variants_still_run_their_command() {
    use Place::*;
    let undone = photos_with(&[(0, "r2f0_q")]);
    let redone = photos_with(&[(0, "r2f1Gq")]);
    for place in [Grid, Develop] {
        assert!(tap(place, Key::Z, cmd()).is("photos", &undone), "{place:?}");
        assert!(
            tap(place, Key::Z, cmd_shift()).is("photos", &redone),
            "{place:?}"
        );
        if !cfg!(target_os = "macos") {
            assert!(tap(place, Key::Y, cmd()).is("photos", &redone), "{place:?}");
        }
    }
    // Lightroom's Cmd+Option+0.
    assert!(
        tap(Develop, Key::Num0, cmd_alt())
            .get("zoom")
            .is_some_and(|z| z.starts_with("on"))
    );
    // `{` `}` and `?` beside `[` `]` and `/`.
    assert!(tap(Develop, Key::CloseCurlyBracket, NONE).is("photos", &edited(&[3], "r1f0_")));
    assert!(
        tap(Remove, Key::OpenCurlyBracket, NONE)
            .get("remove")
            .is_some()
    );
    assert!(
        tap(RedEye, Key::CloseCurlyBracket, NONE)
            .get("red eye")
            .is_some()
    );
    assert!(
        tap(Mask, Key::Questionmark, SHIFT)
            .get("mask")
            .is_some_and(|m| m.starts_with("brush 1"))
    );
    assert!(
        tap(Remove, Key::Questionmark, SHIFT)
            .get("recipe")
            .is_some()
    );
    // `|` beside Shift+`\\`; Delete beside Backspace; Shift+Tab before Tab.
    assert!(tap(Develop, Key::Pipe, SHIFT).is("compare", "CameraThumbnail"));
    for key in [Key::Delete, Key::Backspace] {
        assert!(tap(Remove, key, NONE).get("recipe").is_some(), "{key:?}");
        assert!(tap(Develop, key, NONE).get("dialog").is_some(), "{key:?}");
    }
    assert!(tap(Grid, Key::Tab, SHIFT).is("panels", "develop lRfPA library lrfPA tab Presets"));
    // Shift goes on to the next photo, extends the Grid's selection and
    // feathers a brush; Shift and Alt pick the Basic slider's step.
    let o = tap(Develop, Key::CloseBracket, SHIFT);
    assert!(
        o.get("photos").is_some() && o.get("open").is_some(),
        "{o:?}"
    );
    for key in [Key::ArrowDown, Key::PageDown, Key::End] {
        let o = tap(Grid, key, SHIFT);
        assert!(o.get("selected").is_some(), "{key:?} {o:?}");
    }
    let o = tap(Remove, Key::OpenCurlyBracket, SHIFT);
    assert!(
        o.get("remove").is_some_and(|r| r.contains("feather 0.4")),
        "{o:?}"
    );
    let step = |m| {
        tap(Develop, Key::Minus, m)
            .get("recipe")
            .map(str::to_string)
    };
    let normal = step(NONE);
    for m in [SHIFT, ALT, ALT | SHIFT] {
        assert!(
            step(m).is_some_and(|r| Some(&r) != normal.as_ref()),
            "{m:?}"
        );
    }
    // Cmd+Shift+= is US Cmd++, as `+` with Shift held; Cmd+= with Alt too.
    for (key, m) in [
        (Key::Plus, cmd_shift()),
        (Key::Equals, cmd_shift()),
        (Key::Equals, cmd_alt()),
    ] {
        assert!(tap(Develop, key, m).get("zoom").is_some(), "{key:?} {m:?}");
    }
}

/// Punctuation as layouts that need Shift or Alt to type it send it, the
/// events as egui-winit makes them: the key typed, its place, and the
/// modifiers held.
#[test]
fn layouts_that_type_punctuation_with_shift_or_alt_keep_their_shortcuts() {
    use Place::*;
    // German: `/` is Shift+7, so Cmd+/ is Cmd+Shift+7.
    let o = tap_typed(Grid, Key::Slash, Key::Num7, cmd_shift());
    assert!(o.is("dialog", "Shortcuts") && o.taken, "{o:?}");
    // German: `'` is Shift and the `#` key, at the US `\\` key's place.
    let o = tap_typed(Grid, Key::Quote, Key::Backslash, cmd_shift());
    assert!(o.get("photos").is_some_and(|p| p.contains("new")), "{o:?}");
    // AZERTY: `/` is Shift and the `:` key, at the US `.` key's place.
    let o = tap_typed(Grid, Key::Slash, Key::Period, SHIFT);
    assert!(o.is("active", "2"), "{o:?}");
    // Shift+`\\` arriving as Backslash with Shift.
    let o = tap_typed(Develop, Key::Backslash, Key::Backslash, SHIFT);
    assert!(o.is("compare", "CameraThumbnail"), "{o:?}");
    // US Shift+, and Shift+. type `<` and `>`, which egui doesn't name:
    // egui-winit sends the key's place. Neither selects a Basic slider.
    for key in [Key::Comma, Key::Period] {
        let o = tap_typed(Develop, key, key, SHIFT);
        assert!(o.get("slider").is_none(), "{key:?} {o:?}");
    }
    // German macOS: Option+7 types `|`, Option+Shift+7 types `\\`.
    let o = tap_typed(Develop, Key::Pipe, Key::Num7, ALT);
    assert!(o.is("compare", "CameraThumbnail"), "{o:?}");
    assert!(o.get("photos").is_none(), "not the 7 key's label: {o:?}");
    let o = tap_typed(Develop, Key::Backslash, Key::Num7, ALT | SHIFT);
    assert!(o.is("compare", "CameraThumbnail"), "{o:?}");
    assert!(o.get("photos").is_none(), "{o:?}");
}

#[test]
fn a_press_that_is_a_basic_key_by_its_place_runs_only_the_letter_it_types() {
    // Dvorak types V on the `.` key and W on the `,` key: B&W and the White
    // Balance selector, no longer also a Basic slider chosen.
    let o = tap_typed(Place::Develop, Key::V, Key::Period, NONE);
    assert!(
        o.get("recipe").is_some() && o.get("slider").is_none(),
        "{o:?}"
    );
    let o = tap_typed(Place::Develop, Key::W, Key::Comma, NONE);
    assert!(
        o.get("tool").is_some() && o.get("slider").is_none(),
        "{o:?}"
    );
}

#[test]
fn a_refused_delete_leaves_the_next_delete_in_the_frame_to_delete() {
    // Shift+Delete does nothing in Develop; a Delete after it in the same frame
    // still asks to delete the open photo.
    let o = taps(Place::Develop, &[(Key::Delete, SHIFT), (Key::Delete, NONE)]);
    assert!(
        o.get("dialog").is_some_and(|d| d.contains("Delete")),
        "{o:?}"
    );
}

// ---------------------------------------------------------------------------
// Every default shortcut at once.

/// Every action a default shortcut takes, in the place it is taken: each must
/// change something there, so the place's start gives it something to act on
/// and a shortcut that stops working shows here even without a pin of its own.
#[test]
fn every_default_shortcut_changes_something_from_its_places_start() {
    let (none, shift, alt) = (NONE, SHIFT, ALT);
    let (cmd, cmd_shift, cmd_alt, cmd_alt_shift) = (cmd(), cmd_shift(), cmd_alt(), cmd_alt_shift());
    use Place::*;
    let mut actions: Vec<(Place, Key, Modifiers, &str)> = vec![
        // Everywhere.
        (Grid, Key::Escape, shift, "dismiss messages"),
        (Develop, Key::G, none, "Library grid"),
        (Loupe, Key::G, none, "Library grid from the Loupe"),
        (Develop, Key::E, none, "Library Loupe"),
        (Grid, Key::Quote, cmd, "virtual copy"),
        (Develop, Key::Quote, cmd, "virtual copy in Develop"),
        (Grid, Key::F, none, "hide the chrome"),
        (Grid, Key::D, none, "develop the active photo"),
        (Grid, Key::Z, cmd, "undo"),
        (Grid, Key::Z, cmd_shift, "redo"),
        (Grid, Key::Comma, cmd, "Preferences"),
        (GridTyping, Key::Comma, cmd, "Preferences while typing"),
        (Grid, Key::Slash, cmd, "the sheet"),
        (Grid, Key::Tab, none, "panels: all sides"),
        (Grid, Key::Tab, shift, "panels: all"),
        (Grid, Key::F6, none, "filmstrip"),
        (Grid, Key::F7, none, "left panel"),
        (Grid, Key::F8, none, "right panel"),
        (Grid, Key::F8, shift, "Presets panel"),
        (Develop, Key::F8, shift, "Develop's tab panel"),
        // Metadata.
        (Grid, Key::ArrowUp, cmd, "flag +1"),
        (Grid, Key::ArrowDown, cmd, "flag -1"),
        (Grid, Key::P, none, "pick"),
        (Grid, Key::U, none, "unflag"),
        (Grid, Key::X, none, "reject"),
        (Grid, Key::Backtick, none, "toggle pick"),
        (Grid, Key::OpenBracket, none, "rating -1"),
        (Grid, Key::CloseBracket, none, "rating +1"),
        (Loupe, Key::Num3, shift, "rating and advance"),
    ];
    for (i, key) in [
        Key::Num0,
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ]
    .into_iter()
    .enumerate()
    {
        actions.push((
            Grid,
            key,
            none,
            [
                "0", "1", "2", "3", "4", "5", "Red", "Yellow", "Green", "Blue",
            ][i],
        ));
    }
    actions.extend([
        // The grid.
        (Grid, Key::E, none, "Loupe"),
        (Grid, Key::Enter, none, "Loupe by Enter"),
        (Grid, Key::C, none, "Compare"),
        (Grid, Key::N, none, "Survey"),
        (Grid, Key::J, none, "cell style"),
        (Grid, Key::B, none, "Quick Collection toggle"),
        (Grid, Key::B, cmd, "show the Quick Collection"),
        (Grid, Key::B, cmd_shift, "clear the Quick Collection"),
        (Grid, Key::Delete, none, "delete"),
        (Grid, Key::Backspace, none, "delete by Backspace"),
        (Grid, Key::Delete, alt, "remove from the catalog"),
        (Grid, Key::Delete, cmd, "delete rejected photos"),
        (Grid, Key::Delete, cmd_alt_shift, "move to the Trash"),
        (Grid, Key::F2, none, "rename"),
        (Grid, Key::G, cmd, "stack"),
        (Grid, Key::S, none, "stack collapse"),
        (Grid, Key::A, cmd, "select all"),
        (Grid, Key::D, cmd, "select none"),
        (Grid, Key::Slash, none, "deselect the active photo"),
        (Grid, Key::ArrowLeft, none, "left"),
        (Grid, Key::ArrowRight, none, "right"),
        (Grid, Key::ArrowUp, none, "up"),
        (Grid, Key::ArrowDown, shift, "down, extending"),
        (Grid, Key::PageUp, none, "page up"),
        (Grid, Key::PageDown, none, "page down"),
        (Grid, Key::Home, none, "home"),
        (Grid, Key::End, none, "end"),
        (Grid, Key::L, cmd, "filters"),
        (GridTyping, Key::L, cmd, "filters while typing"),
        (Grid, Key::E, cmd_shift, "export"),
        (Grid, Key::E, cmd_alt_shift, "export with previous"),
        (Grid, Key::S, cmd_shift, "sync"),
        (Grid, Key::U, cmd_shift, "auto settings"),
        // The Loupe.
        (Loupe, Key::ArrowLeft, none, "previous"),
        (Loupe, Key::ArrowUp, none, "previous by Up"),
        (Loupe, Key::ArrowRight, none, "next"),
        (Loupe, Key::ArrowDown, none, "next by Down"),
        (Loupe, Key::Home, none, "first"),
        (Loupe, Key::End, none, "last"),
        (Loupe, Key::Escape, none, "close the Loupe"),
        (Loupe, Key::Delete, none, "delete the photo"),
        (Loupe, Key::F2, none, "rename the photo"),
        (Loupe, Key::C, none, "Compare"),
        (Loupe, Key::N, none, "Survey"),
        (Loupe, Key::B, none, "Quick Collection"),
        (Loupe, Key::I, none, "info"),
        (Loupe, Key::S, none, "stack"),
        (Loupe, Key::Num0, cmd, "100%"),
        (Loupe, Key::Equals, cmd, "zoom in"),
        (Loupe, Key::Plus, cmd, "zoom in by +"),
        (Loupe, Key::Z, none, "zoom"),
        (Loupe, Key::Z, shift, "magnifier"),
        // Compare and Survey.
        (Compare, Key::B, none, "Quick Collection"),
        (Compare, Key::ArrowLeft, none, "candidate -1"),
        (Compare, Key::ArrowRight, none, "candidate +1"),
        (Compare, Key::ArrowUp, none, "make select"),
        (Compare, Key::ArrowDown, none, "swap"),
        (Compare, Key::Escape, none, "close"),
        (Compare, Key::E, none, "Loupe"),
        (Compare, Key::Enter, none, "Loupe by Enter"),
        (Compare, Key::N, none, "Survey"),
        (Survey, Key::B, none, "Quick Collection"),
        (Survey, Key::ArrowLeft, none, "previous"),
        (Survey, Key::ArrowUp, none, "previous by Up"),
        (Survey, Key::Escape, none, "close"),
        (Survey, Key::E, none, "Loupe"),
        (Survey, Key::Enter, none, "Loupe by Enter"),
        (Survey, Key::C, none, "Compare"),
        // Develop.
        (Develop, Key::Num0, cmd, "100%"),
        (Develop, Key::Equals, cmd, "zoom in"),
        (Develop, Key::Z, none, "zoom"),
        (Develop, Key::Z, shift, "magnifier"),
        (Develop, Key::Delete, none, "delete"),
        (Develop, Key::ArrowLeft, cmd_alt_shift, "After to Before"),
        (Develop, Key::ArrowRight, cmd_alt_shift, "Before to After"),
        (
            Develop,
            Key::ArrowUp,
            cmd_alt_shift,
            "swap Before and After",
        ),
        (Develop, Key::ArrowLeft, none, "previous photo"),
        (Develop, Key::ArrowRight, none, "next photo"),
        (Develop, Key::Y, none, "Before/After"),
        (Develop, Key::Y, alt, "Before/After top and bottom"),
        (Develop, Key::Y, shift, "Before/After split"),
        (Develop, Key::C, cmd_shift, "Copy Settings"),
        (Develop, Key::V, cmd_shift, "Paste Settings"),
        (Develop, Key::V, cmd_alt, "Paste from Previous"),
        (Develop, Key::M, cmd_alt_shift, "Match Total Exposures"),
        (Develop, Key::N, cmd_shift, "New Develop Preset"),
        (Develop, Key::T, cmd_alt_shift, "targeted Tone Curve"),
        (Develop, Key::H, cmd_alt_shift, "targeted Hue"),
        (Develop, Key::S, cmd_alt_shift, "targeted Saturation"),
        (Develop, Key::L, cmd_alt_shift, "targeted Luminance"),
        (Develop, Key::G, cmd_alt_shift, "targeted B&W"),
        (Develop, Key::S, cmd_shift, "Sync"),
        (Develop, Key::R, cmd_shift, "Reset"),
        (Develop, Key::V, none, "B&W"),
        (Develop, Key::U, cmd_shift, "Auto Settings"),
        (Develop, Key::E, cmd_shift, "Export"),
        (Develop, Key::E, cmd_alt_shift, "Export with Previous"),
        (Develop, Key::A, cmd, "select all"),
        (Develop, Key::D, cmd, "select none"),
        (Develop, Key::Z, cmd, "undo"),
        (Develop, Key::Z, cmd_shift, "redo"),
        (Develop, Key::C, none, "Crop"),
        (Develop, Key::R, none, "Crop by R"),
        (Develop, Key::R, shift, "Reference View"),
        (Develop, Key::I, none, "info"),
        (Develop, Key::J, none, "clipping"),
        (Develop, Key::Pipe, none, "camera thumbnail"),
        (
            Develop,
            Key::Backslash,
            shift,
            "camera thumbnail by Shift+\\",
        ),
        (Develop, Key::Backslash, none, "Before only"),
        (Crop, Key::Enter, none, "Crop away"),
        (Guided, Key::Enter, none, "Guided away"),
        (Develop, Key::W, none, "White Balance"),
        (Develop, Key::Q, none, "Remove"),
        (Develop, Key::W, shift, "Masking"),
        (Develop, Key::T, shift, "Guided"),
        (Crop, Key::Escape, none, "tool away"),
        (Develop, Key::K, none, "brush mask"),
        (Develop, Key::M, none, "linear mask"),
        (Develop, Key::M, shift, "radial mask"),
        (Develop, Key::J, shift, "color range mask"),
        // The tools.
        (Crop, Key::X, none, "crop orientation"),
        (Crop, Key::A, none, "aspect lock"),
        (Crop, Key::O, none, "next overlay"),
        (Remove, Key::OpenBracket, none, "smaller"),
        (Remove, Key::OpenBracket, shift, "less feather"),
        (Remove, Key::CloseBracket, none, "larger"),
        (Remove, Key::CloseBracket, shift, "more feather"),
        (Remove, Key::Slash, none, "new source"),
        (Remove, Key::Delete, none, "delete the spot"),
        (Remove, Key::H, none, "hide pins"),
        (Remove, Key::A, none, "visualize spots"),
        (RedEye, Key::Delete, none, "delete the correction"),
        (RedEye, Key::OpenBracket, none, "smaller"),
        (RedEye, Key::CloseBracket, none, "larger"),
        (Mask, Key::OpenBracket, none, "smaller brush"),
        (Mask, Key::CloseBracket, shift, "more feather"),
        (Mask, Key::Escape, none, "close Masking"),
        (Mask, Key::O, none, "overlay"),
        (Mask, Key::Slash, none, "brush A/B"),
        (Mask, Key::Delete, none, "delete the mask"),
        (Guided, Key::Delete, none, "delete the guide"),
        // Basic.
        (Develop, Key::Period, none, "next slider"),
        (Develop, Key::Comma, none, "previous slider"),
        (Develop, Key::Equals, none, "step up"),
        (Develop, Key::Plus, none, "step up by +"),
        (Develop, Key::Minus, none, "step down"),
    ]);
    let mut t = Template::new();
    let idle: Vec<String> = actions
        .iter()
        .filter(|(place, key, m, _)| tap_in(&mut t.fixture(*place), *key, *m).nothing())
        .map(|(place, key, m, what)| format!("{what} ({place:?} {key:?} {m:?})"))
        .collect();
    assert!(
        idle.is_empty(),
        "these change nothing from their start: {idle:#?}"
    );
}
