use super::*;

const CMD_SHIFT: Mods = Mods::CMD.with(Mods::SHIFT);

fn chord(text: &str) -> Chord {
    Chord::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

#[test]
fn a_chord_prints_as_it_reads() {
    for text in [
        "Cmd+Shift+E",
        "Cmd+Alt+Shift+Delete",
        "Shift+Esc",
        "Return",
        "Page Up",
        "Page Down",
        "Left",
        "Cmd+'",
        "Cmd+,",
        "Cmd+/",
        "Cmd+=",
        "Cmd+-",
        "Cmd++",
        "+",
        "`",
        "[",
        "{",
        "?",
        "\\",
        "Shift+\\",
        "|",
        "Cmd+0",
        "Cmd+Alt+0",
        "F2",
        "Shift+F8",
        "Tab",
    ] {
        assert_eq!(chord(text).to_string(), text);
    }
    assert_eq!(chord("Cmd+Shift+E"), Chord::new(Key::E, CMD_SHIFT));
    assert_eq!(chord("Cmd++"), Chord::new(Key::Plus, Mods::CMD));
    assert_eq!(chord("Shift++"), Chord::new(Key::Plus, Mods::SHIFT));
    assert_eq!(chord("Cmd+-"), Chord::new(Key::Minus, Mods::CMD));
}

#[test]
fn every_key_and_modifier_set_survives_printing() {
    let keys = [
        Key::A,
        Key::Z,
        Key::Num0,
        Key::Num9,
        Key::Escape,
        Key::Enter,
        Key::Tab,
        Key::Delete,
        Key::Backspace,
        Key::Home,
        Key::End,
        Key::PageUp,
        Key::PageDown,
        Key::ArrowLeft,
        Key::ArrowDown,
        Key::F2,
        Key::F8,
    ]
    .into_iter()
    .chain([
        Key::Comma,
        Key::Period,
        Key::Minus,
        Key::Equals,
        Key::Plus,
        Key::Slash,
        Key::Questionmark,
        Key::Backslash,
        Key::Pipe,
        Key::OpenBracket,
        Key::CloseBracket,
        Key::OpenCurlyBracket,
        Key::CloseCurlyBracket,
        Key::Backtick,
        Key::Quote,
        Key::Semicolon,
        Key::Colon,
        Key::Exclamationmark,
    ]);
    for key in keys {
        for bits in 0..16u8 {
            let mods = Mods(bits);
            // Off the Mac there is no Ctrl apart from Cmd to write.
            if mods.contains(Mods::CTRL) && !cfg!(target_os = "macos") {
                continue;
            }
            let c = Chord::new(key, mods);
            assert_eq!(Chord::parse(&c.to_string()), Ok(c), "{c}");
        }
    }
}

#[test]
fn modifiers_print_in_one_order_whatever_order_they_are_read_in() {
    assert_eq!(chord("Shift+Alt+Cmd+K").to_string(), "Cmd+Alt+Shift+K");
    assert_eq!(chord("shift+cmd+e").to_string(), "Cmd+Shift+E");
    assert_eq!(chord(" Cmd + Shift + E ").to_string(), "Cmd+Shift+E");
}

#[test]
fn platform_names_read_as_the_modifier_they_are() {
    assert_eq!(chord("Option+Y"), chord("Alt+Y"));
    assert_eq!(chord("Command+Z"), chord("Cmd+Z"));
    assert_eq!(chord("Control+Z"), chord("Ctrl+Z"));
    // Ctrl is a modifier of its own only on a Mac; elsewhere it is the command key.
    let ctrl_z = chord("Ctrl+Z");
    if cfg!(target_os = "macos") {
        assert_eq!(ctrl_z, Chord::new(Key::Z, Mods::CTRL));
        assert_eq!(ctrl_z.to_string(), "Ctrl+Z");
        assert_eq!(chord("Cmd+Ctrl+Z").to_string(), "Cmd+Ctrl+Z");
    } else {
        assert_eq!(ctrl_z, Chord::new(Key::Z, Mods::CMD));
        assert_eq!(ctrl_z.to_string(), "Cmd+Z");
        assert_eq!(
            Chord::parse("Cmd+Ctrl+Z"),
            Err(ChordError::Repeated("Ctrl".into()))
        );
    }
}

#[test]
fn text_that_is_no_chord_says_why() {
    assert_eq!(Chord::parse(""), Err(ChordError::NoKey));
    assert_eq!(Chord::parse("Cmd+"), Err(ChordError::NoKey));
    assert_eq!(
        Chord::parse("Hyper+R"),
        Err(ChordError::UnknownModifier("Hyper".into()))
    );
    assert_eq!(
        Chord::parse("Cmd+Banana"),
        Err(ChordError::UnknownKey("Banana".into()))
    );
    assert_eq!(
        Chord::parse("Shift+Shift+A"),
        Err(ChordError::Repeated("Shift".into()))
    );
}

#[test]
fn a_press_holds_ctrl_only_where_it_is_not_the_command_key() {
    use eframe::egui::Modifiers;
    // Off the Mac egui-winit sets both for Ctrl.
    assert_eq!(Mods::of(Modifiers::CTRL | Modifiers::COMMAND), Mods::CMD);
    // A Mac's Cmd, Control, and both.
    assert_eq!(Mods::of(Modifiers::MAC_CMD | Modifiers::COMMAND), Mods::CMD);
    assert_eq!(Mods::of(Modifiers::CTRL), Mods::CTRL);
    assert_eq!(
        Mods::of(Modifiers::MAC_CMD | Modifiers::COMMAND | Modifiers::CTRL),
        Mods::CMD.with(Mods::CTRL)
    );
    assert_eq!(
        Mods::of(Modifiers::ALT | Modifiers::SHIFT),
        Mods::ALT.with(Mods::SHIFT)
    );
}

#[test]
fn variants_are_free_and_never_part_of_the_match() {
    let j = Chord::new(Key::J, Mods::NONE);
    let free = j.free(Mods::SHIFT, Mods::NONE);
    assert_eq!(free, Mods::SHIFT);
    assert!(j.accepts(Key::J, Mods::NONE, free));
    assert!(j.accepts(Key::J, Mods::SHIFT, free));
    assert!(!j.accepts(Key::J, Mods::ALT, free));
    assert!(!j.accepts(Key::J, Mods::SHIFT.with(Mods::ALT), free));
    // A letter has no punctuation rule.
    assert_eq!(j.free(Mods::NONE, Mods::NONE), Mods::NONE);
    assert!(!j.accepts(Key::K, Mods::NONE, free));
}

#[test]
fn shift_and_alt_are_free_on_punctuation_unless_named_or_rejected() {
    let both = Mods::SHIFT.with(Mods::ALT);
    let slash = Chord::new(Key::Slash, Mods::CMD);
    assert_eq!(slash.free(Mods::NONE, Mods::NONE), both);
    for held in [Mods::NONE, Mods::SHIFT, Mods::ALT, both] {
        assert!(slash.accepts(Key::Slash, Mods::CMD.with(held), both));
    }
    assert!(!slash.accepts(Key::Slash, Mods::NONE, both));
    assert!(!slash.accepts(Key::Slash, Mods::CMD.with(Mods::CTRL), both));
    // A chord that names Shift needs it; Alt stays free.
    let thumbnail = Chord::new(Key::Backslash, Mods::SHIFT);
    let free = thumbnail.free(Mods::NONE, Mods::NONE);
    assert_eq!(free, Mods::ALT);
    assert!(thumbnail.accepts(Key::Backslash, Mods::SHIFT, free));
    assert!(thumbnail.accepts(Key::Backslash, both, free));
    assert!(!thumbnail.accepts(Key::Backslash, Mods::NONE, free));
    // A rejected modifier is matched.
    let comma = Chord::new(Key::Comma, Mods::NONE);
    let free = comma.free(Mods::NONE, Mods::SHIFT);
    assert_eq!(free, Mods::ALT);
    assert!(!comma.accepts(Key::Comma, Mods::SHIFT, free));
    assert!(comma.accepts(Key::Comma, Mods::ALT, free));
}
