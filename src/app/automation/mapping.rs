//! Device button notation resolves to shared application actions.
use super::*;
/// What a button does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Action {
    Named(commands::Action),
    /// Legacy shortcut notation, resolved to a named action before execution.
    Key(Key, Modifiers),
    /// Acts as a modifier while held.
    Hold(Modifiers),
    /// Shows Hue, Saturation or Luminance (0..=2) in the Color Mixer, which is
    /// what the Band faders then turn.
    Mixer(usize),
    /// Black & White on or off, as the panel's B&W button does.
    ToggleMono,
}

pub(super) const SHIFT: Modifiers = Modifiers {
    shift: true,
    ..Modifiers::NONE
};
pub(super) fn command() -> Modifiers {
    Modifiers {
        command: true,
        mac_cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Modifiers::NONE
    }
}
/// A key by name, in any case: "Backslash", "z", "left" or "ArrowLeft".
fn key_named(name: &str) -> Option<Key> {
    Key::from_name(name).or_else(|| {
        Key::ALL.iter().copied().find(|k| {
            k.name().eq_ignore_ascii_case(name) || format!("{k:?}").eq_ignore_ascii_case(name)
        })
    })
}
pub(super) fn parse_action(text: &str) -> Option<Action> {
    if !text.starts_with("mixer:")
        && text != "toggle:bw"
        && let Some(action) = commands::Action::parse(text)
    {
        return Some(Action::Named(action));
    }
    if text.eq_ignore_ascii_case("toggle:bw") {
        return Some(Action::ToggleMono);
    }
    if let Some(channel) = text.strip_prefix("mixer:") {
        return match channel.trim().to_ascii_lowercase().as_str() {
            "hue" => Some(Action::Mixer(0)),
            "sat" | "saturation" => Some(Action::Mixer(1)),
            "lum" | "luminance" => Some(Action::Mixer(2)),
            _ => None,
        };
    }
    let (hold, text) = match text.strip_prefix("hold:") {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "ctrl" | "control" | "primary" => modifiers |= command(),
            "control-key" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" | "option" => modifiers.alt = true,
            _ => key = Some(key_named(part)?),
        }
    }
    match (hold, key) {
        (true, None) => Some(Action::Hold(modifiers)),
        (false, Some(key)) => Some(Action::Key(key, modifiers)),
        _ => None,
    }
}
/// Only bindings with an executable application meaning are offered in Preferences.
pub(super) fn supported_action(text: &str) -> Option<Action> {
    let action = parse_action(text)?;
    if let Action::Key(key, modifiers) = action
        && shortcut_action(key, modifiers).is_none()
    {
        return None;
    }
    Some(action)
}
/// The text `parse_action` reads back: `cmd+shift+z`, `hold:shift`, `mixer:hue`.
pub(super) fn action_spec(action: Action) -> String {
    let modifiers = |m: Modifiers| {
        let mut parts = Vec::new();
        if m.command || m.mac_cmd {
            parts.push("cmd");
        } else if m.ctrl {
            parts.push("control-key");
        }
        if m.shift {
            parts.push("shift");
        }
        if m.alt {
            parts.push("alt");
        }
        parts
    };
    match action {
        Action::Named(a) => a.name().into(),
        Action::ToggleMono => "toggle:bw".into(),
        Action::Mixer(c) => format!("mixer:{}", ["hue", "sat", "lum"][c]),
        Action::Hold(m) => format!("hold:{}", modifiers(m).join("+")),
        Action::Key(key, m) => {
            let mut parts = modifiers(m);
            parts.push(key.name());
            parts.join("+")
        }
    }
}
pub(super) fn shortcut_action(key: Key, m: Modifiers) -> Option<commands::Action> {
    use crate::app::panels::{PanelChange, WorkspacePanel};
    use commands::Action as A;
    Some(
        match (key, m.command || m.ctrl || m.mac_cmd, m.shift, m.alt) {
            (Key::Z, true, false, false) => A::Undo,
            (Key::Z, true, true, false) | (Key::Y, true, false, false) => A::Redo,
            (Key::C, true, true, false) => A::Copy,
            (Key::V, true, true, false) => A::Paste,
            (Key::V, true, false, true) => A::PastePrevious,
            (Key::S, true, true, false) => A::Sync,
            (Key::R, true, true, false) => A::Reset,
            (Key::U, true, true, false) => A::AutoTone,
            (Key::E, true, true, false) => A::ExportDialog,
            (Key::E, true, true, true) => A::ExportPrevious,
            (Key::W, false, true, false) => A::Mask,
            (Key::Tab, false, true, false) => A::Panels(PanelChange::All),
            (Key::Tab, false, false, false) => A::Panels(PanelChange::Sides),
            (Key::F7, false, false, false) => A::Panels(PanelChange::Toggle(WorkspacePanel::Left)),
            (Key::F8, false, false, false) => A::Panels(PanelChange::Toggle(WorkspacePanel::Right)),
            (Key::F8, false, true, false) => {
                A::Panels(PanelChange::Toggle(WorkspacePanel::Presets))
            }
            (Key::F6, false, false, false) => {
                A::Panels(PanelChange::Toggle(WorkspacePanel::Filmstrip))
            }
            (key, false, _, false) => match key {
                Key::Num0 => A::Rating(0),
                Key::Num1 => A::Rating(1),
                Key::Num2 => A::Rating(2),
                Key::Num3 => A::Rating(3),
                Key::Num4 => A::Rating(4),
                Key::Num5 => A::Rating(5),
                Key::Num6 => A::ToggleLabel(0),
                Key::Num7 => A::ToggleLabel(1),
                Key::Num8 => A::ToggleLabel(2),
                Key::Num9 => A::ToggleLabel(3),
                Key::P => A::Flag(1),
                Key::X => A::Flag(-1),
                Key::U => A::Flag(0),
                Key::ArrowLeft | Key::ArrowUp => A::Previous,
                Key::ArrowRight | Key::ArrowDown => A::Next,
                Key::Backslash => A::Compare,
                Key::J => A::Clipping,
                Key::Z => A::Zoom,
                Key::F => A::Chrome,
                Key::R | Key::C => A::Crop,
                Key::Q => A::Remove,
                Key::W => A::WhiteBalance,
                Key::V => A::ToggleMono,
                Key::G => A::Library,
                Key::D => A::Develop,
                Key::E => A::Loupe,
                _ => return None,
            },
            _ => return None,
        },
    )
}

/// `a` without the modifiers that are on in `b`.
pub(super) fn without(a: Modifiers, b: Modifiers) -> Modifiers {
    Modifiers {
        alt: a.alt && !b.alt,
        ctrl: a.ctrl && !b.ctrl,
        shift: a.shift && !b.shift,
        mac_cmd: a.mac_cmd && !b.mac_cmd,
        command: a.command && !b.command,
    }
}
