use super::bindings::ALL;
use super::*;
use crate::app::shell::commands::Action;

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
        assert!(!b.defaults.is_empty(), "{} has no keys", b.command);
        for d in b.defaults {
            assert!(!d.keys.is_empty(), "{} has a chord with no key", b.command);
            assert!(!d.label.is_empty(), "{} has an unnamed chord", b.command);
        }
    }
    for b in bindings::METADATA {
        assert!(ALL.iter().any(|a| std::ptr::eq(*a, b)));
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
