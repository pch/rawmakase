use super::bindings::ALL;
use super::*;
use crate::app::shell::commands::Action;
use crate::app::shell::keymap::Mods;

#[test]
fn ids_are_unique_and_never_hold_a_slash() {
    let mut seen = std::collections::BTreeSet::new();
    for c in COMMANDS {
        assert!(seen.insert(c.id), "{} twice", c.id);
        assert!(!c.id.contains('/'), "{} holds a /", c.id);
        assert!(
            c.id.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == ':'),
            "{} is not lower-case snake_case with `:`",
            c.id
        );
        assert!(!c.title.is_empty(), "{} has no title", c.id);
    }
}

#[test]
fn every_binding_runs_a_command_and_has_a_chord() {
    for b in ALL {
        assert!(
            command_by_id(b.command).is_some(),
            "no command {}",
            b.command
        );
        assert_eq!(
            b.defaults.is_empty(),
            bindings::keyless(b),
            "{} has keys, or none",
            b.command
        );
        for d in b.defaults {
            assert!(!d.keys.is_empty(), "{} has a chord with no key", b.command);
            assert!(!d.label().is_empty(), "{} has an unnamed chord", b.command);
        }
    }
    for b in bindings::METADATA {
        assert!(ALL.iter().any(|a| std::ptr::eq(*a, b)));
    }
}

#[test]
fn a_users_keys_reach_every_command_but_the_fixed_ones() {
    for c in COMMANDS.iter().filter(|c| c.rebindable) {
        assert!(
            ALL.iter().any(|b| b.command == c.id),
            "{} has no binding for a key to run it",
            c.id
        );
    }
}

#[test]
fn every_label_reads_back_as_its_chord() {
    use crate::app::shell::keymap::Chord;
    for b in ALL {
        for d in b.defaults {
            let first = d.chords().next().expect("a key");
            assert_eq!(Chord::parse(&d.label()), Ok(first), "{}", b.command);
        }
    }
}

#[test]
fn no_chord_names_its_commands_variant() {
    for b in ALL {
        for d in b.defaults {
            assert!(
                d.mods.without(b.variants) == d.mods,
                "{} names its variant in {}",
                b.command,
                d.label()
            );
            assert!(
                [Mods::NONE, Mods::SHIFT, Mods::SHIFT.with(Mods::ALT)].contains(&b.variants),
                "{}: only Shift, and Alt with it, are variants",
                b.command
            );
        }
    }
}

/// Whether bindings in `a` and `b` can both be eligible for one press.
fn overlap(a: Context, b: Context) -> bool {
    a.overlaps(b)
}

/// On a punctuation key, Shift and Alt are free unless a binding that can be
/// eligible names that modifier with that key, or rejects it. The bindings
/// carry the result as their `rejects`: a chord leaves no modifier free that
/// another chord on its key, with the same Cmd and Ctrl, names.
#[test]
fn the_punctuation_rule_holds_across_bindings() {
    use crate::app::shell::keymap::chord::is_punctuation;
    let cmd_ctrl = Mods::CMD.with(Mods::CTRL);
    for a in ALL {
        for da in a.defaults {
            for ca in da.chords().filter(|c| is_punctuation(c.key)) {
                let free = ca.free(a.variants, da.rejects);
                for b in ALL.iter().filter(|b| overlap(a.context, b.context)) {
                    for cb in b.defaults.iter().flat_map(|d| d.chords()) {
                        if cb.key != ca.key
                            || cb.mods.without(Mods::SHIFT.with(Mods::ALT))
                                != ca.mods.without(Mods::SHIFT.with(Mods::ALT))
                            || cb == ca
                        {
                            continue;
                        }
                        let named = cb.mods.without(cmd_ctrl).without(ca.mods);
                        for m in [Mods::SHIFT, Mods::ALT] {
                            assert!(
                                !(named.contains(m) && free.contains(m) && !a.variants.contains(m)),
                                "{} ({ca}) leaves free what {} ({cb}) names",
                                a.command,
                                b.command
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn every_automation_action_is_a_command_in_its_order() {
    let names = Action::names();
    for name in &names {
        assert!(command_by_id(name).is_some(), "{name} is not a command");
    }
    // The registry starts with them, in their order: automation's
    // `capabilities` lists them so.
    let ids: Vec<&str> = COMMANDS.iter().take(names.len()).map(|c| c.id).collect();
    assert_eq!(ids, names);
}

#[test]
fn the_automation_action_names_are_unchanged() {
    assert_eq!(
        Action::names(),
        [
            "undo",
            "redo",
            "rating:0",
            "rating:1",
            "rating:2",
            "rating:3",
            "rating:4",
            "rating:5",
            "pick",
            "reject",
            "unflag",
            "label:red",
            "label:yellow",
            "label:green",
            "label:blue",
            "label:purple",
            "label:none",
            "toggle_label:red",
            "toggle_label:yellow",
            "toggle_label:green",
            "toggle_label:blue",
            "toggle_label:purple",
            "copy",
            "paste",
            "paste_previous",
            "sync",
            "reset",
            "auto_tone",
            "auto_white_balance",
            "curve:linear",
            "curve:medium_contrast",
            "curve:strong_contrast",
            "export_dialog",
            "export_previous",
            "toggle:bw",
            "treatment:bw",
            "treatment:color",
            "compare",
            "camera_thumbnail",
            "clipping",
            "zoom",
            "fit",
            "crop",
            "remove",
            "white_balance",
            "mask",
            "mixer:hue",
            "mixer:sat",
            "mixer:lum",
            "library",
            "develop",
            "loupe",
            "next",
            "previous",
            "panels:sides",
            "panels:all",
            "panels:left",
            "panels:right",
            "panels:filmstrip",
            "panels:presets",
            "chrome",
        ]
    );
}

#[test]
fn the_text_editing_commands_are_fixed() {
    for id in ["undo", "redo", "select_all"] {
        assert!(!command_by_id(id).expect("a command").rebindable, "{id}");
    }
    assert!(command_by_id("crop").expect("a command").rebindable);
}

#[test]
fn a_command_has_one_binding_per_context_but_for_ctrl_y() {
    for (i, a) in ALL.iter().enumerate() {
        for b in &ALL[..i] {
            if a.command == b.command && a.context == b.context {
                // Ctrl+Y is read apart from Cmd+Shift+Z in Develop: both in one
                // frame undo and redo.
                assert!(
                    std::ptr::eq(*a, &bindings::DEVELOP_REDO_Y)
                        || std::ptr::eq(*b, &bindings::DEVELOP_REDO_Y),
                    "{} twice in {:?}",
                    a.command,
                    a.context
                );
            }
        }
    }
}
