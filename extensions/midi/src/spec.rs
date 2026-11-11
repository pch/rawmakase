//! Button actions in the control layer's notation, as `midi.json` stores them
//! and `adapter.operate` sends them: an action name (`undo`, `mixer:hue`), a
//! shortcut (`cmd+shift+z`) or a held modifier (`hold:shift`).
//!
//! The program needs only their shape: a held modifier changes what the
//! device holds, a shortcut takes the held modifiers into its own, and
//! anything else goes to the app unchanged. What a spec does, and whether it
//! does anything, is the app's to say: it resolves each spec it is sent, and
//! answers `read.action` for one a user typed.

/// Modifiers, as the notation spells them. `cmd` (also `command`, `ctrl`,
/// `control` and `primary`) is the platform's command key, which the app
/// takes for Control as well on Linux and Windows; `control-key` is Control
/// alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Mods {
    pub cmd: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Mods {
    /// The command key, as the app's `cmd` sets it: with Control on Linux and
    /// Windows, where Control is the command key, so releasing `hold:cmd`
    /// releases both, as it always has.
    const fn command() -> Self {
        Self {
            cmd: true,
            ctrl: !cfg!(target_os = "macos"),
            shift: false,
            alt: false,
        }
    }
    fn or(self, other: Self) -> Self {
        Self {
            cmd: self.cmd || other.cmd,
            ctrl: self.ctrl || other.ctrl,
            shift: self.shift || other.shift,
            alt: self.alt || other.alt,
        }
    }
    /// `self` without the modifiers that are on in `other`.
    fn without(self, other: Self) -> Self {
        Self {
            cmd: self.cmd && !other.cmd,
            ctrl: self.ctrl && !other.ctrl,
            shift: self.shift && !other.shift,
            alt: self.alt && !other.alt,
        }
    }
    /// The modifier a word of the notation names, if it names one.
    fn word(word: &str) -> Option<Self> {
        let none = Self::default();
        Some(match word.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "ctrl" | "control" | "primary" => Self::command(),
            "control-key" => Self { ctrl: true, ..none },
            "shift" => Self {
                shift: true,
                ..none
            },
            "alt" | "option" => Self { alt: true, ..none },
            _ => return None,
        })
    }
    /// The words that spell these modifiers, in the app's order.
    fn words(self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.cmd {
            out.push("cmd");
        } else if self.ctrl {
            out.push("control-key");
        }
        if self.shift {
            out.push("shift");
        }
        if self.alt {
            out.push("alt");
        }
        out
    }
}

/// A spec's shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Spec {
    /// Acts as these modifiers while held.
    Hold(Mods),
    /// A key with modifiers: the device's held modifiers join them.
    Key(Mods, String),
    /// An action by name, or something the app will refuse: sent as it is.
    Other,
}

impl Spec {
    /// Reads `text`'s shape. `named` says whether a text is one of the app's
    /// action names (`read.capabilities`), which win over a key of the same
    /// spelling, as in the app.
    pub(crate) fn parse(text: &str, named: &dyn Fn(&str) -> bool) -> Self {
        let lower = text.to_ascii_lowercase();
        if lower == "toggle:bw" || text.starts_with("mixer:") || named(text) {
            return Self::Other;
        }
        let (hold, rest) = match text.strip_prefix("hold:") {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let mut mods = Mods::default();
        let mut key = None;
        for part in rest.split('+').map(str::trim) {
            match Mods::word(part) {
                Some(m) => mods = mods.or(m),
                // The app reads each part as a key and keeps the last.
                None => key = Some(part.to_string()),
            }
        }
        match (hold, key) {
            (true, None) => Self::Hold(mods),
            (false, Some(key)) if !key.is_empty() => Self::Key(mods, key),
            _ => Self::Other,
        }
    }
}

/// A shortcut's text: its modifiers in the app's order, then the key.
fn key_spec(mods: Mods, key: &str) -> String {
    let mut parts = mods.words();
    parts.push(key);
    parts.join("+")
}

/// What pressing (`down`) or releasing a button bound to `spec` does, with
/// the modifiers `held` on the same device: the spec to send and whether to
/// move on afterwards. A held modifier changes `held` and sends nothing;
/// anything else acts on the press, a shortcut with the held modifiers merged
/// in. Shift, held or in the shortcut, moves on after a metadata action.
pub(crate) fn press(
    held: &mut Mods,
    spec: &str,
    down: bool,
    named: &dyn Fn(&str) -> bool,
) -> Option<(String, bool)> {
    let shape = Spec::parse(spec, named);
    if let Spec::Hold(m) = shape {
        *held = if down { held.or(m) } else { held.without(m) };
        return None;
    }
    if !down {
        return None;
    }
    Some(match shape {
        Spec::Key(m, key) => (key_spec(m.or(*held), &key), held.shift || m.shift),
        _ => (spec.to_string(), held.shift),
    })
}

/// Whether two specs bind the same action, as far as their shape tells:
/// modifiers in any order and spelling, keys in any case, and the arrows with
/// or without `arrow` (`left`, `ArrowLeft`). For the page, which shows a
/// binding by the name of the choice it equals.
pub(crate) fn same(a: &str, b: &str, named: &dyn Fn(&str) -> bool) -> bool {
    let key = |k: &str| {
        let k = k.to_ascii_lowercase();
        k.strip_prefix("arrow").map(str::to_string).unwrap_or(k)
    };
    match (Spec::parse(a, named), Spec::parse(b, named)) {
        (Spec::Hold(x), Spec::Hold(y)) => x == y,
        (Spec::Key(x, k), Spec::Key(y, l)) => x == y && key(&k) == key(&l),
        (Spec::Other, Spec::Other) => a.trim().eq_ignore_ascii_case(b.trim()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn shapes_read_as_the_app_reads_them() {
        assert_eq!(
            Spec::parse("Cmd+Shift+Z", &none),
            Spec::Key(
                Mods {
                    shift: true,
                    ..Mods::command()
                },
                "Z".into()
            )
        );
        assert_eq!(
            Spec::parse("hold:alt", &none),
            Spec::Hold(Mods {
                alt: true,
                ..Mods::default()
            })
        );
        assert_eq!(Spec::parse("toggle:bw", &none), Spec::Other);
        assert_eq!(Spec::parse("mixer:lum", &none), Spec::Other);
        assert_eq!(Spec::parse("hold:shift+z", &none), Spec::Other);
        assert_eq!(Spec::parse("shift", &none), Spec::Other);
        // An action name is not a key, even when it could spell one.
        assert_eq!(Spec::parse("zoom", &|t| t == "zoom"), Spec::Other);
        assert_eq!(
            Spec::parse("zoom", &none),
            Spec::Key(Mods::default(), "zoom".into())
        );
    }

    #[test]
    fn held_modifiers_join_and_leave() {
        let mut held = Mods::default();
        assert_eq!(press(&mut held, "hold:shift", true, &none), None);
        assert!(held.shift);
        assert_eq!(press(&mut held, "hold:cmd", true, &none), None);
        assert_eq!(
            press(&mut held, "Z", true, &none),
            Some(("cmd+shift+Z".into(), true))
        );
        // Releasing a button does nothing but release its modifier.
        assert_eq!(press(&mut held, "Z", false, &none), None);
        assert_eq!(press(&mut held, "hold:cmd", false, &none), None);
        assert_eq!(
            held,
            Mods {
                shift: true,
                ..Mods::default()
            }
        );
        assert_eq!(press(&mut held, "hold:shift", false, &none), None);
        assert_eq!(held, Mods::default());
        // A named action takes no modifiers; Shift still moves on.
        held.shift = true;
        assert_eq!(
            press(&mut held, "undo", true, &|t| t == "undo"),
            Some(("undo".into(), true))
        );
        // Shift in the binding itself moves on too.
        held = Mods::default();
        assert_eq!(
            press(&mut held, "shift+1", true, &none),
            Some(("shift+1".into(), true))
        );
        assert_eq!(
            press(&mut held, "1", true, &none),
            Some(("1".into(), false))
        );
    }

    #[test]
    fn control_key_is_control_alone() {
        let mut held = Mods::default();
        press(&mut held, "hold:control-key", true, &none);
        assert_eq!(
            press(&mut held, "Z", true, &none),
            Some(("control-key+Z".into(), false))
        );
    }

    #[test]
    fn specs_compare_by_what_they_bind() {
        assert!(same("cmd+Z", "cmd+z", &none));
        assert!(same("Up", "arrowup", &none));
        assert!(same("cmd+alt+shift+e", "cmd+shift+alt+E", &none));
        assert!(same("hold:cmd", "hold:ctrl", &none));
        assert!(same("toggle:bw", "Toggle:BW", &none));
        assert!(!same("cmd+z", "cmd+shift+z", &none));
        assert!(!same("hold:shift", "shift", &none));
    }
}
