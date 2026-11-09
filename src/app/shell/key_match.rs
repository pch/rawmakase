//! Which key a key press stands for when a shortcut is matched.
//!
//! A shortcut is matched by what the key types, with one exception: the number
//! row is matched by its place, so ratings and labels work on layouts whose digit
//! row types something else unshifted (AZERTY's `&é"'(-è_çà`). egui-winit
//! already falls back to the place when the typed character has no egui name
//! (Shift+5's `%`, macOS Option+letter), so no other key needs the place. A key
//! matched by its place as well as by what it types could run two shortcuts at
//! once: Dvorak's `U` key types G, which once unflagged the photo and went to the
//! grid.
use eframe::egui::{InputState, Key, Modifiers};

/// The key a press with `key` typed at `physical` stands for: the digit of a
/// number-row key, otherwise the typed key.
pub(in crate::app) fn shortcut_key(key: Key, physical: Option<Key>) -> Key {
    physical.filter(|&p| is_digit(p)).unwrap_or(key)
}

pub(in crate::app) fn is_digit(key: Key) -> bool {
    matches!(
        key,
        Key::Num0
            | Key::Num1
            | Key::Num2
            | Key::Num3
            | Key::Num4
            | Key::Num5
            | Key::Num6
            | Key::Num7
            | Key::Num8
            | Key::Num9
    )
}

/// The modifiers held for a press of one of `keys` this frame whose own
/// modifiers pass `want`, repeats included (as `InputState::key_pressed` counts
/// them). A shortcut reads the modifiers held for its key, not the frame's: a
/// quick Cmd+Shift+R can release both before the frame ends.
pub(in crate::app) fn press(
    i: &InputState,
    keys: &[Key],
    want: impl Fn(Modifiers) -> bool,
) -> Option<Modifiers> {
    presses(i, keys, true, want)
}

/// As [`press`], without repeats: a held key acts once.
pub(in crate::app) fn fresh_press(
    i: &InputState,
    keys: &[Key],
    want: impl Fn(Modifiers) -> bool,
) -> Option<Modifiers> {
    presses(i, keys, false, want)
}

fn presses(
    i: &InputState,
    keys: &[Key],
    repeats: bool,
    want: impl Fn(Modifiers) -> bool,
) -> Option<Modifiers> {
    i.events.iter().find_map(|event| match event {
        eframe::egui::Event::Key {
            key,
            pressed: true,
            repeat,
            modifiers,
            ..
        } if keys.contains(key) && (repeats || !repeat) && want(*modifiers) => Some(*modifiers),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_number_row_goes_by_its_place_and_everything_else_by_what_it_types() {
        // AZERTY's 4 and 6 type `'` and `-`; Shift+1 types `!`.
        assert_eq!(shortcut_key(Key::Quote, Some(Key::Num4)), Key::Num4);
        assert_eq!(shortcut_key(Key::Minus, Some(Key::Num6)), Key::Num6);
        assert_eq!(
            shortcut_key(Key::Exclamationmark, Some(Key::Num1)),
            Key::Num1
        );
        // Dvorak types G on U, `]` on `=`, F on Y.
        assert_eq!(shortcut_key(Key::G, Some(Key::U)), Key::G);
        assert_eq!(
            shortcut_key(Key::CloseBracket, Some(Key::Equals)),
            Key::CloseBracket
        );
        assert_eq!(shortcut_key(Key::F, Some(Key::Y)), Key::F);
        // No place reported.
        assert_eq!(shortcut_key(Key::Num3, None), Key::Num3);
    }
}
