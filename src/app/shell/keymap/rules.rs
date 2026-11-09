//! Which chords a user may give a command, and which contexts can be active
//! together.
//!
//! A user's chord is refused when the system or a text field owns it (the
//! reserved chords), when egui-winit can't deliver it as a key on Windows, when
//! it names a modifier the command reads as a variant, when the command runs
//! while typing and the chord would type, or when a fixed key (one read in
//! place by a widget or a gesture) has it where the command acts.
use super::{Binding, Chord, Context, Mods};
use crate::app::shell::commands::registry::{Command, bindings};
use eframe::egui::Key;
use std::fmt;

/// The platform a chord is judged for: its reserved keys differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Os {
    Macos,
    Windows,
    /// Linux and the other Unix desktops.
    Other,
}

impl Os {
    pub(in crate::app) const CURRENT: Self = if cfg!(target_os = "macos") {
        Self::Macos
    } else if cfg!(windows) {
        Self::Windows
    } else {
        Self::Other
    };
}

/// Why a chord can't run a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum Refusal {
    /// The command is fixed: Undo, Redo and Select All are the text-editing
    /// keys, and the Edit menu's on macOS.
    Fixed,
    /// Cut, Copy, Paste, Select All, Undo and Redo in text fields, and on
    /// macOS the system's Quit, Close, Hide and Minimize.
    Reserved,
    /// egui-winit turns it into Cut, Copy or Paste on Windows and drops the key,
    /// or its Cut can't be told apart from Ctrl+Shift+Delete's.
    WindowsClipboard,
    /// It holds a modifier the command reads as a variant (Shift goes on to the
    /// next photo after a rating).
    Variant(Mods),
    /// The command runs while a text field has the keys, where a plain key types.
    Typing,
    /// A fixed key has it where the command acts.
    Taken(&'static str),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fixed => write!(f, "this command's keys are fixed"),
            Self::Reserved => write!(f, "the system and text fields keep this key"),
            Self::WindowsClipboard => write!(
                f,
                "Windows turns this key into Cut, Copy or Paste before RAWmakase sees it"
            ),
            Self::Variant(m) => {
                let held = Chord::new(Key::A, *m).to_string();
                let held = held.trim_end_matches("+A");
                write!(f, "{held} is held for this command's other form")
            }
            Self::Typing => write!(
                f,
                "this command works while typing, so its key needs Cmd or Ctrl, or an F key"
            ),
            Self::Taken(what) => write!(f, "{what} uses this key"),
        }
    }
}

/// A key a widget or a gesture reads in place: never dispatched, so no command
/// may have it where the widget works. Shown locked in Preferences.
pub(in crate::app) struct FixedKey {
    pub title: &'static str,
    pub context: Context,
    pub keys: &'static [Key],
    /// The modifiers it is read with; any one of them makes the chord its.
    pub mods: &'static [Mods],
}

/// The fixed keys a command's chord could collide with.
pub(in crate::app) static FIXED_KEYS: &[FixedKey] = &[
    FixedKey {
        title: "Nudging the hovered slider",
        context: Context::Develop,
        keys: &[Key::ArrowUp, Key::ArrowDown],
        mods: &[Mods::NONE, Mods::SHIFT],
    },
    FixedKey {
        title: "Panning while a tool is open",
        context: Context::Develop,
        keys: &[Key::Space],
        mods: &[Mods::NONE],
    },
];

/// The commands that run while a text field has the keys: Preferences, the
/// shortcuts, the filters and dismissing the messages.
fn runs_while_typing(command: &str) -> bool {
    matches!(
        command,
        "preferences" | "shortcuts" | "filters" | "messages:dismiss_all" | "shortcuts:clear_search"
    )
}

/// Whether the system or a text field owns `chord` on `os`.
pub(in crate::app) fn reserved(chord: Chord, os: Os) -> bool {
    let cmd = Mods::CMD;
    let text_editing = [
        Chord::new(Key::C, cmd),
        Chord::new(Key::V, cmd),
        Chord::new(Key::X, cmd),
        Chord::new(Key::A, cmd),
        Chord::new(Key::Z, cmd),
        Chord::new(Key::Z, cmd.with(Mods::SHIFT)),
    ];
    if text_editing.contains(&chord) {
        return true;
    }
    match os {
        // Ctrl+Y redoes in text fields; Ctrl is written Cmd there.
        Os::Windows | Os::Other if chord == Chord::new(Key::Y, cmd) => true,
        Os::Macos => [Key::Q, Key::W, Key::H, Key::M]
            .iter()
            .any(|k| chord == Chord::new(*k, cmd)),
        _ => false,
    }
}

/// Whether egui-winit on Windows turns `chord` into Cut, Copy or Paste (Insert
/// with Ctrl or Shift, Delete with Shift, whatever else is held), or sends a
/// Cut for it that a Shift+Delete chord sends too (Ctrl+Shift+X and
/// Ctrl+Alt+Shift+X), so it can't arrive as itself.
pub(in crate::app) fn windows_clipboard(chord: Chord) -> bool {
    let (cmd, shift) = (
        chord.mods.contains(Mods::CMD),
        chord.mods.contains(Mods::SHIFT),
    );
    match chord.key {
        Key::Insert => cmd || shift,
        Key::Delete => shift,
        Key::X => cmd && shift && !chord.mods.contains(Mods::CTRL),
        _ => false,
    }
}

/// Why `chord` can't run `command`, whose bindings are `bindings`, on `os`; or
/// nothing, when it can.
pub(in crate::app) fn refusal(
    command: &Command,
    bindings: &[&Binding],
    chord: Chord,
    os: Os,
) -> Option<Refusal> {
    if !command.rebindable {
        return Some(Refusal::Fixed);
    }
    if reserved(chord, os) {
        return Some(Refusal::Reserved);
    }
    if os == Os::Windows && windows_clipboard(chord) {
        return Some(Refusal::WindowsClipboard);
    }
    let variants = bindings
        .iter()
        .fold(Mods::NONE, |all, b| all.with(b.variants));
    let named = chord.mods.and(variants);
    if named != Mods::NONE {
        return Some(Refusal::Variant(named));
    }
    if runs_while_typing(command.id) && !chord.mods.contains(Mods::CMD) && !is_f_key(chord.key) {
        // On a Mac, Control types nothing either.
        if !(os == Os::Macos && chord.mods.contains(Mods::CTRL)) {
            return Some(Refusal::Typing);
        }
    }
    for fixed in FIXED_KEYS {
        let here = bindings.iter().any(|b| b.context.overlaps(fixed.context));
        if here && fixed.keys.contains(&chord.key) && fixed.mods.contains(&chord.mods) {
            return Some(Refusal::Taken(fixed.title));
        }
    }
    None
}

/// The bindings of the command `id`.
pub(in crate::app) fn bindings_of(id: &str) -> Vec<&'static Binding> {
    bindings::ALL
        .iter()
        .copied()
        .filter(|b| b.command == id)
        .collect()
}

fn is_f_key(key: Key) -> bool {
    key.name()
        .strip_prefix('F')
        .is_some_and(|n| n.parse::<u8>().is_ok())
}

/// How far a context's bindings take precedence over others on the same chord:
/// a tool's over Develop's, a Library view's over the Library's, a module's
/// over those that work everywhere. The reference card's window is modal.
pub(in crate::app) fn level(context: Context) -> u8 {
    use Context::*;
    match context {
        Everywhere => 0,
        Library | Develop | LoupeZoom => 1,
        Grid | Loupe | Compare | Survey | Tool(_) => 2,
        ShortcutSheet => 3,
    }
}

impl Context {
    /// Whether bindings in `self` and `other` can both be eligible for one press.
    pub(in crate::app) fn overlaps(self, other: Self) -> bool {
        use Context::*;
        let library_view = |c| matches!(c, Grid | Loupe | Compare | Survey);
        let develop = |c| matches!(c, Develop | Tool(_));
        let (a, b) = (self, other);
        // The reference card is a modal window: nothing else takes keys then.
        if a == ShortcutSheet || b == ShortcutSheet {
            return a == b;
        }
        a == b
            || a == Everywhere
            || b == Everywhere
            || (a == Library && library_view(b))
            || (b == Library && library_view(a))
            || (develop(a) && develop(b) && (a == Develop || b == Develop))
            || (a == LoupeZoom && (b == Loupe || b == Library || develop(b)))
            || (b == LoupeZoom && (a == Loupe || a == Library || develop(a)))
    }
}

#[cfg(test)]
mod tests;
