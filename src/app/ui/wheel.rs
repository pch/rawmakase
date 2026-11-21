//! The mouse wheel as the photo views and controls read it: each event in wheel
//! notches, with the modifiers held when it happened, and the rule that leaves the
//! wheel alone while another gesture is under way.
use eframe::egui;
use std::time::Duration;

/// Points of scrolling that count as one wheel notch.
pub(in crate::app) const POINTS_PER_LINE: f32 = 40.;
/// Lines a page of scrolling counts as.
const LINES_PER_PAGE: f32 = 10.;
/// Scrolling with pauses shorter than this is one gesture, one History step.
pub(in crate::app) const GESTURE_PAUSE: Duration = Duration::from_millis(400);

/// One wheel event: how far it scrolled, in notches on each axis (positive up and
/// left, as egui reports it), how the device reported it, and the modifiers held
/// when it happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Wheel {
    pub(in crate::app) notches: egui::Vec2,
    pub(in crate::app) unit: egui::MouseWheelUnit,
    pub(in crate::app) modifiers: egui::Modifiers,
}
impl Wheel {
    /// The notches along the wheel: the vertical scroll, or the sideways one with
    /// Shift, which macOS turns a Shift+wheel into. A plain sideways scroll counts
    /// for nothing.
    pub(in crate::app) fn lines(self) -> f32 {
        if self.modifiers.shift && self.notches.y == 0. {
            self.notches.x
        } else {
            self.notches.y
        }
    }
}

/// Whether the wheel is left alone this frame: a button is held (a stroke, a
/// moved spot or correction), or the frame also has a click or a key, so a scroll
/// never mixes into another gesture or History step. A key a shortcut already
/// took counts too: the frame's raw input still has it.
pub(in crate::app) fn busy(i: &egui::InputState) -> bool {
    i.pointer.any_down()
        || i.raw.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::PointerButton { .. } | egui::Event::Key { pressed: true, .. }
            )
        })
}

/// This frame's wheel events, none while [`busy`].
pub(in crate::app) fn read(i: &egui::InputState) -> Vec<Wheel> {
    if busy(i) {
        return Vec::new();
    }
    i.events
        .iter()
        .filter_map(|event| {
            let egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } = event
            else {
                return None;
            };
            let notches = match unit {
                egui::MouseWheelUnit::Point => *delta / POINTS_PER_LINE,
                egui::MouseWheelUnit::Line => *delta,
                egui::MouseWheelUnit::Page => *delta * LINES_PER_PAGE,
            };
            Some(Wheel {
                notches,
                unit: *unit,
                modifiers: *modifiers,
            })
        })
        .collect()
}
