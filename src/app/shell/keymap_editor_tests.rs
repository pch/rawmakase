//! The user's key map in the editor: what a rebound, unbound or conflicting
//! command does when its keys are pressed, from an editor in a [`Place`] (see
//! [`Fixture::new`]).
use super::keymap_pins_harness::*;
use crate::app::shell::keymap::Chord;
use eframe::egui::{Key, Modifiers};

const NONE: Modifiers = Modifiers::NONE;

/// Gives the command `id` exactly `keys` in `f`'s editor.
fn rebind(f: &mut Fixture, id: &str, keys: &[&str]) {
    let chords: Vec<Chord> = keys
        .iter()
        .map(|k| Chord::parse(k).expect("a chord"))
        .collect();
    f.e.keymap.set(id, &chords).expect("bound");
}

/// What tapping `key` with `m` changed, as `field=value`.
fn tap(f: &mut Fixture, key: Key, m: Modifiers) -> Vec<String> {
    let before = f.fingerprint();
    f.tap(key, m);
    changes(&before, &f.fingerprint())
}

/// Whether `changed` holds `field` changed to `value`.
fn has(changed: &[String], field: &str, value: &str) -> bool {
    changed.iter().any(|c| c == &format!("{field}={value}"))
}
fn touches(changed: &[String], field: &str) -> bool {
    changed.iter().any(|c| c.starts_with(&format!("{field}=")))
}

#[test]
fn a_rebound_command_runs_on_its_new_keys_alone() {
    let mut f = Fixture::new(Place::Develop);
    rebind(&mut f, "crop", &["F9"]);
    for key in [Key::C, Key::R] {
        let changed = tap(&mut f, key, NONE);
        assert!(!touches(&changed, "tool"), "{key:?}: {changed:?}");
    }
    let changed = tap(&mut f, Key::F9, NONE);
    assert!(has(&changed, "tool", "Crop"), "{changed:?}");
}

#[test]
fn a_rebound_command_has_its_keys_in_every_context() {
    // Cycling photo info is the Loupe's and Develop's.
    for place in [Place::Loupe, Place::Develop] {
        let mut f = Fixture::new(place);
        rebind(&mut f, "info", &["F9"]);
        assert!(!touches(&tap(&mut f, Key::I, NONE), "info"), "{place:?}");
        assert!(touches(&tap(&mut f, Key::F9, NONE), "info"), "{place:?}");
    }
}

#[test]
fn an_unbound_command_has_no_key_and_the_others_keep_theirs() {
    let mut f = Fixture::new(Place::Develop);
    rebind(&mut f, "clipping", &[]);
    assert!(!touches(&tap(&mut f, Key::J, NONE), "clipping"));
    // A command the key map doesn't name keeps its defaults.
    assert!(has(&tap(&mut f, Key::W, NONE), "tool", "WhiteBalance"));
}

fn command(extra: Modifiers) -> Modifiers {
    super::keymap_pins_harness::command(extra)
}

/// What `key` with `m` does in a new editor in `place` with the defaults.
fn default_tap(place: Place, key: Key, m: Modifiers) -> Vec<String> {
    let mut f = Fixture::new(place);
    tap(&mut f, key, m)
}

#[test]
fn two_commands_on_one_key_run_neither_and_say_so() {
    // Clipping in Develop's main step and a Basic key in the frame's last.
    let mut f = Fixture::new(Place::Develop);
    rebind(&mut f, "clipping", &["F9"]);
    rebind(&mut f, "basic:next", &["F9"]);
    let changed = tap(&mut f, Key::F9, NONE);
    assert!(!touches(&changed, "clipping"), "{changed:?}");
    assert!(!touches(&changed, "slider"), "{changed:?}");
    let line = f.e.messages.line().to_string();
    assert!(
        line.contains("Show shadow and highlight clipping")
            && line.contains("Select the next Basic slider")
            && line.contains("Preferences > Shortcuts"),
        "{line}"
    );
}

#[test]
fn a_rebound_tool_command_wins_over_an_earlier_steps_develop_command() {
    // Develop's clipping reads J before Crop's keys do.
    let mut f = Fixture::new(Place::Crop);
    rebind(&mut f, "crop:swap_orientation", &["J"]);
    let changed = tap(&mut f, Key::J, NONE);
    assert!(!touches(&changed, "clipping"), "{changed:?}");
    assert!(touches(&changed, "aspect"), "the crop swapped: {changed:?}");
    // Outside Crop, J is the clipping's again.
    let mut f = Fixture::new(Place::Develop);
    rebind(&mut f, "crop:swap_orientation", &["J"]);
    assert!(touches(&tap(&mut f, Key::J, NONE), "clipping"));
}

#[test]
fn a_press_left_to_a_tool_that_closes_in_the_frame_runs_nothing() {
    let mut f = Fixture::new(Place::Crop);
    rebind(&mut f, "crop:swap_orientation", &["J"]);
    let before = f.fingerprint();
    // J is left to Crop; Escape then puts Crop away before its keys are read.
    f.taps(&[(Key::J, NONE), (Key::Escape, NONE)]);
    let changed = changes(&before, &f.fingerprint());
    assert!(has(&changed, "tool", "None"), "{changed:?}");
    assert!(!touches(&changed, "clipping"), "{changed:?}");
    assert!(!touches(&changed, "aspect"), "{changed:?}");
}

#[test]
fn one_key_in_the_grid_and_in_develop_is_no_conflict() {
    for (place, field) in [(Place::Grid, "cells"), (Place::Develop, "clipping")] {
        let mut f = Fixture::new(place);
        rebind(&mut f, "grid:cell_style", &["F9"]);
        rebind(&mut f, "clipping", &["F9"]);
        assert!(touches(&tap(&mut f, Key::F9, NONE), field), "{place:?}");
    }
}

/// Rebinding a command whose default keys differ only in their modifiers
/// from its neighbours' runs that command, not the one the modifiers chose.
#[test]
fn quick_collection_delete_and_stack_keys_run_the_command_that_matched() {
    let cases: [(Place, &str, Key, Modifiers); 6] = [
        (
            Place::Grid,
            "quick:clear",
            Key::B,
            command(Modifiers::SHIFT),
        ),
        (Place::Grid, "quick:show", Key::B, command(NONE)),
        (
            Place::Loupe,
            "quick:clear",
            Key::B,
            command(Modifiers::SHIFT),
        ),
        (
            Place::Grid,
            "trash",
            Key::Delete,
            command(Modifiers::ALT | Modifiers::SHIFT),
        ),
        (
            Place::Grid,
            "stack:unstack",
            Key::G,
            command(Modifiers::SHIFT),
        ),
        (Place::Loupe, "stack:to_top", Key::S, Modifiers::SHIFT),
    ];
    for (place, id, key, m) in cases {
        let expected = default_tap(place, key, m);
        assert!(!expected.is_empty(), "{id} does something by default");
        let mut f = Fixture::new(place);
        rebind(&mut f, id, &["F9"]);
        assert_eq!(
            tap(&mut f, Key::F9, NONE),
            expected,
            "{id} on F9 in {place:?}"
        );
    }
    // Develop's delete keys too.
    let expected = default_tap(Place::Develop, Key::Delete, Modifiers::ALT);
    let mut f = Fixture::new(Place::Develop);
    rebind(&mut f, "remove_from_catalog", &["F9"]);
    assert_eq!(tap(&mut f, Key::F9, NONE), expected);
}

#[test]
fn a_command_with_no_default_key_runs_on_the_users() {
    let mut f = Fixture::new(Place::Grid);
    rebind(&mut f, "label:green", &["F9"]);
    let changed = tap(&mut f, Key::F9, NONE);
    assert!(
        touches(&changed, "photos") && changed.iter().any(|c| c == "message=3 photos · Green"),
        "{changed:?}"
    );
    let mut f = Fixture::new(Place::Develop);
    rebind(&mut f, "treatment:bw", &["F9"]);
    let changed = tap(&mut f, Key::F9, NONE);
    assert!(touches(&changed, "recipe"), "{changed:?}");
}

#[test]
fn the_clipboard_hook_turns_cut_back_into_x_where_only_x_sends_it() {
    use super::super::keymap::rules::Os;
    use eframe::egui::Event;
    let ctx = eframe::egui::Context::default();
    let keys = |m: Modifiers, os: Os| {
        let mut input = eframe::egui::RawInput {
            events: vec![Event::ModifiersChanged(m), Event::Cut],
            ..Default::default()
        };
        super::clipboard_shortcut_keys_on(&ctx, &mut input, os);
        input
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Key { key, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let (cmd_alt, cmd_shift) = (command(Modifiers::ALT), command(Modifiers::SHIFT));
    for os in [Os::Macos, Os::Other] {
        assert_eq!(keys(cmd_alt, os), [(Key::X, cmd_alt)], "{os:?}");
        assert_eq!(keys(cmd_shift, os), [(Key::X, cmd_shift)], "{os:?}");
    }
    // Windows sends the same Cut for Ctrl+Shift+Delete.
    assert_eq!(keys(cmd_alt, Os::Windows), [(Key::X, cmd_alt)]);
    assert!(keys(cmd_shift, Os::Windows).is_empty());
    assert!(keys(command(Modifiers::ALT | Modifiers::SHIFT), Os::Windows).is_empty());
    // Plain Cmd+X stays the text field's.
    for os in [Os::Macos, Os::Windows, Os::Other] {
        assert!(keys(command(NONE), os).is_empty(), "{os:?}");
    }
    // And a user's Cmd+Alt+X then runs its command.
    let mut f = Fixture::new(Place::Develop);
    rebind(&mut f, "clipping", &["Cmd+Alt+X"]);
    let mut input = eframe::egui::RawInput {
        events: vec![Event::ModifiersChanged(cmd_alt), Event::Cut],
        ..Default::default()
    };
    super::clipboard_shortcut_keys(&f.e.context, &mut input);
    let before = f.fingerprint();
    f.frame(input.events);
    assert!(touches(&changes(&before, &f.fingerprint()), "clipping"));
}
