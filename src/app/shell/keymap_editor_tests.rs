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
