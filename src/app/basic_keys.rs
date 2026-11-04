//! Lightroom Classic's keys for the Basic panel's sliders in Develop: `.` and `,`
//! select the next or previous slider, `=` (or `+`) and `-` step the selected one,
//! Shift by more and Option by less. The selection is view state: it stays across
//! photos and modules, and keeps working with the panel folded or a tool open,
//! always on the photo's global setting.
//!
//! Each key event (a press, or a held key's repeat) is one History step, applied
//! after everything else in the frame, just before the frame's pending work (save,
//! close): see [`Editor::develop_key_steps`].
use crate::app::Editor;
use crate::app::history::Step;
use crate::camera_data::Metadata;
use crate::model::params::{KeyStep, ParameterId};
use eframe::egui;

/// The sliders `.` walks through, in the Basic panel's order. Temp and Tint are
/// named as on a RAW file, and move a rendered file's relative ones
/// ([`ParameterId::for_photo`]). Profile and its Amount are not among them, as in
/// Lightroom.
pub(in crate::app) const ORDER: [ParameterId; 13] = [
    ParameterId::Temperature,
    ParameterId::Tint,
    ParameterId::Exposure,
    ParameterId::Contrast,
    ParameterId::Highlights,
    ParameterId::Shadows,
    ParameterId::Whites,
    ParameterId::Blacks,
    ParameterId::Texture,
    ParameterId::Clarity,
    ParameterId::Dehaze,
    ParameterId::Vibrance,
    ParameterId::Saturation,
];

/// The name the selection keeps for a Basic slider: a rendered file's relative
/// Temp and Tint are Temp and Tint, so the selection survives moving between a
/// RAW file and a JPEG.
pub(in crate::app) fn canonical(id: ParameterId) -> ParameterId {
    match id {
        ParameterId::IncrementalTemperature => ParameterId::Temperature,
        ParameterId::IncrementalTint => ParameterId::Tint,
        other => other,
    }
}

/// Whether the Basic panel enables the slider for `id` on `photo`: a monochrome
/// photo has no white balance, so Lightroom disables Temp and Tint. The keys skip
/// and leave alone what the panel disables.
pub(in crate::app) fn enabled(id: ParameterId, photo: Option<&Metadata>) -> bool {
    let white_balance = matches!(canonical(id), ParameterId::Temperature | ParameterId::Tint);
    !(white_balance && photo.is_some_and(|m| m.monochrome))
}

/// The slider `.` (`forward`) or `,` selects after `selected` on `photo`, past the
/// disabled ones and around the ends; from no selection, the first or the last.
fn next(
    selected: Option<ParameterId>,
    forward: bool,
    photo: Option<&Metadata>,
) -> Option<ParameterId> {
    let len = ORDER.len();
    let at = selected.and_then(|s| ORDER.iter().position(|id| *id == s));
    (1..=len)
        .map(|offset| match (at, forward) {
            (Some(at), true) => (at + offset) % len,
            (Some(at), false) => (at + len - offset) % len,
            (None, true) => offset - 1,
            (None, false) => len - offset,
        })
        .map(|i| ORDER[i])
        .find(|id| enabled(*id, photo))
}

/// What one key event asks of the Basic sliders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// `.` (forward) or `,`.
    Select { forward: bool },
    /// `=`, `+` or `-`.
    Step { size: KeyStep, up: bool },
}

/// The action of `event`: a press or a repeat, with the modifiers held for it. The
/// key's place on the keyboard decides, as Lightroom's shortcuts go by it: Shift
/// with `=` is still `=`, Option does not change it on macOS; the typed key only
/// when the place is unknown. The keypad's `+` and `-` step too. Cmd and Ctrl are
/// never these keys (Cmd+= zooms, Cmd+, opens Preferences), and `.` and `,` only
/// without any modifier.
fn decode(event: &egui::Event) -> Option<Action> {
    let egui::Event::Key {
        key,
        physical_key,
        pressed: true,
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    if modifiers.command || modifiers.ctrl || modifiers.mac_cmd {
        return None;
    }
    let size = if modifiers.alt {
        KeyStep::Fine
    } else if modifiers.shift {
        KeyStep::Large
    } else {
        KeyStep::Normal
    };
    match physical_key.unwrap_or(*key) {
        egui::Key::Period if !modifiers.any() => Some(Action::Select { forward: true }),
        egui::Key::Comma if !modifiers.any() => Some(Action::Select { forward: false }),
        egui::Key::Equals | egui::Key::Plus => Some(Action::Step { size, up: true }),
        egui::Key::Minus => Some(Action::Step { size, up: false }),
        _ => None,
    }
}

impl Editor {
    /// Applies this frame's Basic slider keys, each a History step of its own.
    ///
    /// Runs after everything else in the frame that may open a photo, a window or
    /// a dialog, and just before [`Editor::pending_work`] saves or closes: a key
    /// edits the photo the frame ends on, never one closing or about to be left.
    /// Outside an edit frame, through [`Editor::change_edit`].
    ///
    /// `typing` says whether a text field had the keys as the frame began: a field
    /// that took a `-` and gave up the keys on Return in this frame keeps that `-`.
    pub(in crate::app) fn develop_key_steps(&mut self, ctx: &egui::Context, typing: bool) {
        let actions: Vec<Action> = ctx.input(|i| i.events.iter().filter_map(decode).collect());
        if actions.is_empty() || typing || !self.basic_keys_ready(ctx) {
            return;
        }
        // A drag, a wheel scroll or a dial turn still under way keeps the settings.
        let held = ctx.input(|i| i.pointer.any_down()) || self.document.edit.history().in_gesture();
        let photo = self.document.metadata.clone();
        for action in actions {
            match action {
                Action::Select { forward } => {
                    self.view.selected_setting =
                        next(self.view.selected_setting, forward, photo.as_ref());
                    ctx.request_repaint();
                }
                Action::Step { size, up } => {
                    if held {
                        continue;
                    }
                    let Some(selected) = self
                        .view
                        .selected_setting
                        .filter(|id| enabled(*id, photo.as_ref()))
                    else {
                        continue;
                    };
                    let id = selected.for_photo(photo.as_ref());
                    let value = *id.value_mut(&mut self.document.edit.recipe().clone());
                    let stepped = id.key_stepped(value, size, up);
                    // At the end of the slider: nothing changes, and no step.
                    if stepped == value {
                        continue;
                    }
                    let step = Step::new(id.descriptor().label, id.text(stepped));
                    self.change_edit(Some(step), |r| {
                        crate::model::edit::change_setting(r, id, photo.as_ref(), |r| {
                            let value = id.value_mut(r);
                            *value = id.key_stepped(*value, size, up);
                        })
                    });
                    ctx.request_repaint();
                }
            }
        }
    }
    /// Whether the keys may act this frame: a decoded photo open in Develop, which
    /// no dialog, window, typing or busy task has the keys of, and that is not
    /// closing, being left for files dropped on the window, or waiting to close.
    fn basic_keys_ready(&self, ctx: &egui::Context) -> bool {
        self.require_develop().is_ok()
            && !self.activity.is_busy()
            && !self.onboarding.visible
            && !self.workspace_modal()
            && !ctx.text_edit_focused()
            && !self.close_confirm
            && !self.close_after_work
            && self.quit_by.is_none()
            && !ctx.input(|i| i.viewport().close_requested() || !i.raw.dropped_files.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: egui::Key, physical: Option<egui::Key>, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: physical,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn keys_decode_by_their_place_with_their_own_modifiers() {
        use egui::{Key, Modifiers};
        let alt_shift = Modifiers {
            alt: true,
            shift: true,
            ..Default::default()
        };
        let step = |size, up| Some(Action::Step { size, up });
        for (event, expected) in [
            (
                key(Key::Equals, Some(Key::Equals), Modifiers::NONE),
                step(KeyStep::Normal, true),
            ),
            // Shift+= types `+` on a US keyboard: the key is still `=`, the step large.
            (
                key(Key::Plus, Some(Key::Equals), Modifiers::SHIFT),
                step(KeyStep::Large, true),
            ),
            (
                key(Key::Minus, Some(Key::Minus), Modifiers::SHIFT),
                step(KeyStep::Large, false),
            ),
            (
                key(Key::Minus, Some(Key::Minus), Modifiers::ALT),
                step(KeyStep::Fine, false),
            ),
            (
                key(Key::Equals, Some(Key::Equals), alt_shift),
                step(KeyStep::Fine, true),
            ),
            // Option changes the typed character on macOS; the place decides.
            (
                key(Key::A, Some(Key::Minus), Modifiers::ALT),
                step(KeyStep::Fine, false),
            ),
            // A layout typing `=` somewhere else: the place is not `=`.
            (key(Key::Equals, Some(Key::Num0), Modifiers::NONE), None),
            // The keypad's `+`.
            (
                key(Key::Plus, Some(Key::Plus), Modifiers::NONE),
                step(KeyStep::Normal, true),
            ),
            // No place reported: the typed key.
            (
                key(Key::Plus, None, Modifiers::NONE),
                step(KeyStep::Normal, true),
            ),
            (
                key(Key::Period, Some(Key::Period), Modifiers::NONE),
                Some(Action::Select { forward: true }),
            ),
            (
                key(Key::Comma, Some(Key::Comma), Modifiers::NONE),
                Some(Action::Select { forward: false }),
            ),
            (key(Key::Period, Some(Key::Period), Modifiers::SHIFT), None),
            (key(Key::Comma, Some(Key::Comma), Modifiers::ALT), None),
            // Cmd+, is Preferences, Cmd+= and Cmd+- zoom; Ctrl is never these keys.
            (key(Key::Comma, Some(Key::Comma), Modifiers::COMMAND), None),
            (
                key(Key::Equals, Some(Key::Equals), Modifiers::COMMAND),
                None,
            ),
            (key(Key::Minus, Some(Key::Minus), Modifiers::MAC_CMD), None),
            (key(Key::Minus, Some(Key::Minus), Modifiers::CTRL), None),
        ] {
            assert_eq!(decode(&event), expected, "{event:?}");
        }
        // A release is no step; a held key's repeat is one.
        let release = egui::Event::Key {
            key: Key::Minus,
            physical_key: Some(Key::Minus),
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        assert_eq!(decode(&release), None);
        let repeat = egui::Event::Key {
            key: Key::Minus,
            physical_key: Some(Key::Minus),
            pressed: true,
            repeat: true,
            modifiers: Modifiers::NONE,
        };
        assert_eq!(decode(&repeat), step(KeyStep::Normal, false));
    }

    #[test]
    fn selection_walks_the_basic_order_around_the_ends_past_disabled_sliders() {
        let raw = Metadata::default();
        let mono = Metadata {
            monochrome: true,
            ..Default::default()
        };
        let raw = Some(&raw);
        assert_eq!(next(None, true, raw), Some(ParameterId::Temperature));
        assert_eq!(next(None, false, raw), Some(ParameterId::Saturation));
        assert_eq!(
            next(Some(ParameterId::Temperature), true, raw),
            Some(ParameterId::Tint)
        );
        assert_eq!(
            next(Some(ParameterId::Saturation), true, raw),
            Some(ParameterId::Temperature)
        );
        assert_eq!(
            next(Some(ParameterId::Temperature), false, raw),
            Some(ParameterId::Saturation)
        );
        // The whole order, and back.
        let mut selected = None;
        for expected in ORDER {
            selected = next(selected, true, raw);
            assert_eq!(selected, Some(expected));
        }
        for expected in ORDER.into_iter().rev().skip(1) {
            selected = next(selected, false, raw);
            assert_eq!(selected, Some(expected));
        }
        // A monochrome photo has no white balance to select.
        let mono = Some(&mono);
        assert_eq!(next(None, true, mono), Some(ParameterId::Exposure));
        assert_eq!(
            next(Some(ParameterId::Saturation), true, mono),
            Some(ParameterId::Exposure)
        );
        assert_eq!(
            next(Some(ParameterId::Exposure), false, mono),
            Some(ParameterId::Saturation)
        );
        // From Temp kept from a colour photo, on to the next enabled slider.
        assert_eq!(
            next(Some(ParameterId::Temperature), true, mono),
            Some(ParameterId::Exposure)
        );
        assert!(!enabled(ParameterId::IncrementalTint, mono));
        assert!(enabled(ParameterId::Exposure, mono));
        assert_eq!(
            canonical(ParameterId::IncrementalTemperature),
            ParameterId::Temperature
        );
    }
}

#[cfg(test)]
#[path = "basic_keys_tests.rs"]
mod app_tests;
