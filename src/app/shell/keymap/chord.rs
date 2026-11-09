//! A chord: one key with exactly the modifiers held for it, and its text.
//!
//! A chord is written `Cmd+Ctrl+Alt+Shift+Key`, the modifiers in that order and
//! the key as the reference card writes it ("Cmd+Shift+E", "Esc", "Page Up",
//! "Cmd+'"). `Cmd` is egui's command key: ⌘ on macOS, Ctrl elsewhere. `Ctrl` is
//! the Mac's Control key, a modifier of its own only there; elsewhere `Ctrl` is
//! read as `Cmd`. `Option` is read as `Alt`.
//!
//! A press is the chord when its modifiers are the chord's, except for the
//! modifiers [`Chord::accepts`] is told are free: a command's variants, and
//! Shift and Alt on a punctuation key, which many layouts need to type the
//! symbol at all.
use eframe::egui::{Key, Modifiers};
use std::fmt;

/// A set of the modifiers a chord names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(in crate::app) struct Mods(u8);

impl Mods {
    pub(in crate::app) const NONE: Self = Self(0);
    /// egui's command key: ⌘ on macOS, Ctrl elsewhere.
    pub(in crate::app) const CMD: Self = Self(1);
    /// The Mac's Control key. Elsewhere Ctrl is [`Mods::CMD`].
    pub(in crate::app) const CTRL: Self = Self(2);
    /// Alt, Option on a Mac, AltGr.
    pub(in crate::app) const ALT: Self = Self(4);
    pub(in crate::app) const SHIFT: Self = Self(8);

    pub(in crate::app) const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub(in crate::app) const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
    pub(in crate::app) const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    /// The modifiers both hold.
    pub(in crate::app) const fn and(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// The modifiers held for a press, as a chord names them. Ctrl counts only
    /// where it is not the command key: on a Mac, or with ⌘ as well. ⌘ alone
    /// (`mac_cmd` without `command`, which egui-winit never sends) is still Cmd.
    pub(in crate::app) fn of(m: Modifiers) -> Self {
        let mut out = Self::NONE;
        if m.command || m.mac_cmd {
            out = out.with(Self::CMD);
        }
        if m.ctrl && (m.mac_cmd || !m.command) {
            out = out.with(Self::CTRL);
        }
        if m.alt {
            out = out.with(Self::ALT);
        }
        if m.shift {
            out = out.with(Self::SHIFT);
        }
        out
    }
}

impl std::ops::BitOr for Mods {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        self.with(other)
    }
}

/// One key with exactly the modifiers held for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::app) struct Chord {
    pub key: Key,
    pub mods: Mods,
}

/// Why a chord's text doesn't read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum ChordError {
    /// Nothing, or a modifier with no key after it ("Cmd+").
    NoKey,
    UnknownKey(String),
    UnknownModifier(String),
    /// A modifier named twice ("Cmd+Ctrl+Z" off the Mac, where both are Cmd).
    Repeated(String),
}

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoKey => write!(f, "no key"),
            Self::UnknownKey(k) => write!(f, "unknown key \"{k}\""),
            Self::UnknownModifier(m) => write!(f, "unknown modifier \"{m}\""),
            Self::Repeated(m) => write!(f, "\"{m}\" named twice"),
        }
    }
}

/// The modifiers in the order a chord writes them.
const ORDER: [(Mods, &str); 4] = [
    (Mods::CMD, "Cmd"),
    (Mods::CTRL, "Ctrl"),
    (Mods::ALT, "Alt"),
    (Mods::SHIFT, "Shift"),
];

impl Chord {
    pub(in crate::app) const fn new(key: Key, mods: Mods) -> Self {
        Self { key, mods }
    }

    /// Reads a chord's text: the modifiers in any order, each once, then the
    /// key. Modifier names don't depend on case; `Command`, `Control` and
    /// `Option` are read as well.
    pub(in crate::app) fn parse(text: &str) -> Result<Self, ChordError> {
        let text = text.trim();
        // The `+` key ends a chord as `++`, or is the whole of it.
        let (modifiers, key) = if text == "+" {
            ("", "+")
        } else if let Some(rest) = text.strip_suffix("++") {
            (rest, "+")
        } else {
            match text.rsplit_once('+') {
                Some((modifiers, key)) => (modifiers, key),
                None => ("", text),
            }
        };
        if key.trim().is_empty() {
            return Err(ChordError::NoKey);
        }
        let key = key_from_text(key.trim()).ok_or_else(|| ChordError::UnknownKey(key.into()))?;
        let mut mods = Mods::NONE;
        for name in modifiers.split('+').filter(|_| !modifiers.is_empty()) {
            let name = name.trim();
            let one =
                modifier_from_text(name).ok_or_else(|| ChordError::UnknownModifier(name.into()))?;
            if mods.contains(one) {
                return Err(ChordError::Repeated(name.into()));
            }
            mods = mods.with(one);
        }
        Ok(Self { key, mods })
    }

    /// The modifiers a press may hold or not and still be this chord, given
    /// the command's `variants` (passed to it, never matched) and the
    /// modifiers the binding `rejects` on a punctuation key.
    ///
    /// On a punctuation key, Shift and Alt are free unless the chord names
    /// them or the binding rejects them: German Cmd+/ is Cmd+Shift+7, and a
    /// Mac types `|` with Option.
    pub(in crate::app) fn free(&self, variants: Mods, rejects: Mods) -> Mods {
        let typing = if is_punctuation(self.key) {
            Mods::SHIFT.with(Mods::ALT).without(rejects)
        } else {
            Mods::NONE
        };
        variants.with(typing).without(self.mods)
    }

    /// Whether a press of `key`, already read as the binding reads keys, with
    /// `held` is this chord, `free` modifiers aside ([`Chord::free`]).
    pub(in crate::app) fn accepts(&self, key: Key, held: Mods, free: Mods) -> bool {
        key == self.key && held.without(free) == self.mods
    }

    /// The chord as an AppKit key equivalent, if it has one.
    pub(in crate::app) fn key_equivalent(self) -> Option<crate::platform::menu::KeyEquivalent> {
        // AppKit's function-key characters (NSUpArrowFunctionKey and the rest).
        let function = |code: u32| char::from_u32(code).map(String::from);
        let key = match self.key {
            Key::ArrowUp => function(0xF700)?,
            Key::ArrowDown => function(0xF701)?,
            Key::ArrowLeft => function(0xF702)?,
            Key::ArrowRight => function(0xF703)?,
            Key::Insert => function(0xF727)?,
            Key::Delete => function(0xF728)?,
            Key::Home => function(0xF729)?,
            Key::End => function(0xF72B)?,
            Key::PageUp => function(0xF72C)?,
            Key::PageDown => function(0xF72D)?,
            Key::Backspace => "\u{8}".into(),
            Key::Enter => "\r".into(),
            Key::Tab => "\t".into(),
            Key::Escape => "\u{1b}".into(),
            Key::Space => " ".into(),
            // egui names it with a minus sign.
            Key::Minus => "-".into(),
            key => {
                let name = key.name();
                match name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
                    Some(n) if (1..=35).contains(&n) => function(0xF704 + n - 1)?,
                    _ => key.symbol_or_name().to_lowercase(),
                }
            }
        };
        // A Num key's name is its digit; anything longer is no character.
        if key.chars().count() != 1 {
            return None;
        }
        Some(crate::platform::menu::KeyEquivalent {
            key,
            command: self.mods.contains(Mods::CMD),
            shift: self.mods.contains(Mods::SHIFT),
            option: self.mods.contains(Mods::ALT),
            control: self.mods.contains(Mods::CTRL),
        })
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (m, name) in ORDER {
            if self.mods.contains(m) {
                write!(f, "{name}+")?;
            }
        }
        write!(f, "{}", key_text(self.key))
    }
}

/// Whether `key` is punctuation, which some layouts type only with Shift or
/// Alt held.
pub(in crate::app) fn is_punctuation(key: Key) -> bool {
    crate::app::shell::key_match::is_symbol(key)
}

/// A key as a chord writes it: the symbol a punctuation key types, and the
/// card's names for the rest.
fn key_text(key: Key) -> &'static str {
    match key {
        Key::Escape => "Esc",
        Key::Enter => "Return",
        Key::PageUp => "Page Up",
        Key::PageDown => "Page Down",
        // egui writes a minus sign, which no one types.
        Key::Minus => "-",
        Key::Quote => "'",
        k if is_punctuation(k) => k.symbol_or_name(),
        k => k.name(),
    }
}

fn key_from_text(text: &str) -> Option<Key> {
    match text {
        "Page Up" => return Some(Key::PageUp),
        "Page Down" => return Some(Key::PageDown),
        _ => {}
    }
    Key::from_name(text).or_else(|| {
        // A letter in lower case.
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_lowercase() => {
                Key::from_name(&c.to_ascii_uppercase().to_string())
            }
            _ => None,
        }
    })
}

fn modifier_from_text(text: &str) -> Option<Mods> {
    Some(match text.to_ascii_lowercase().as_str() {
        "cmd" | "command" => Mods::CMD,
        "ctrl" | "control" if cfg!(target_os = "macos") => Mods::CTRL,
        // Off the Mac, Ctrl is the command key.
        "ctrl" | "control" => Mods::CMD,
        "alt" | "option" => Mods::ALT,
        "shift" => Mods::SHIFT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
