//! The mouse wheel over the photo sizes the tool under it, as in Lightroom: the Remove
//! brush (and the selected spot), the mask brush shown and the Red Eye circle. With
//! Shift it changes the feather; with Option/Alt it sizes the mask's Erase brush.
use crate::app::Editor;
use crate::app::develop::state::Tool;
use eframe::egui;
use std::time::{Duration, Instant};

/// Points of scrolling that count as one wheel notch, one `[` or `]` press.
const POINTS_PER_LINE: f32 = 40.;
/// Lines a page of scrolling counts as.
const LINES_PER_PAGE: f32 = 10.;
/// How much one notch scales a size, as `[` and `]` do.
const SIZE_STEP: f32 = 1.15;
/// How much one notch changes a feather.
const FEATHER_STEP: f32 = 0.1;
/// Scrolling with pauses shorter than this is one gesture, one History step.
const GESTURE_PAUSE: Duration = Duration::from_millis(400);

/// What a scroll changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Adjust {
    Size,
    Feather,
}

/// Which mask brush a scroll changes: the one in use, or Erase while Option/Alt is
/// held, as the cursor shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum MaskBrush {
    Current,
    Erase,
}

/// One wheel event over the photo: notches up (positive) or down, and what it changes,
/// from the modifiers held when it happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Scroll {
    pub(in crate::app) lines: f32,
    pub(in crate::app) adjust: Adjust,
    pub(in crate::app) brush: MaskBrush,
}
impl Scroll {
    /// This frame's wheel events. A sideways scroll counts only with Shift, which macOS
    /// turns a Shift+wheel into; a plain sideways trackpad swipe changes nothing. While
    /// a button is held (a stroke, a moved spot or correction) or in a frame that also
    /// has a click or a key, the wheel is left alone, so it never mixes into another
    /// gesture or History step.
    pub(in crate::app) fn read(i: &egui::InputState) -> Vec<Self> {
        let busy = i.pointer.any_down()
            || i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::PointerButton { .. } | egui::Event::Key { pressed: true, .. }
                )
            });
        if busy {
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
                let amount = if modifiers.shift && delta.y == 0. {
                    delta.x
                } else {
                    delta.y
                };
                let lines = match unit {
                    egui::MouseWheelUnit::Point => amount / POINTS_PER_LINE,
                    egui::MouseWheelUnit::Line => amount,
                    egui::MouseWheelUnit::Page => amount * LINES_PER_PAGE,
                };
                (lines != 0.).then_some(Scroll {
                    lines,
                    adjust: if modifiers.shift {
                        Adjust::Feather
                    } else {
                        Adjust::Size
                    },
                    brush: if modifiers.alt {
                        MaskBrush::Erase
                    } else {
                        MaskBrush::Current
                    },
                })
            })
            .collect()
    }
    fn scale(self) -> f32 {
        SIZE_STEP.powf(self.lines)
    }
    fn feather(self, feather: f32) -> f32 {
        (feather + FEATHER_STEP * self.lines).clamp(0., 1.)
    }
}

/// Whether a frame changed the edit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum Edit {
    Unchanged,
    Changed,
}

/// When the wheel last changed a size, so a scroll is recorded as one History step.
#[derive(Default)]
pub(in crate::app) struct WheelGesture {
    last: Option<Instant>,
    /// Whether the wheel changed a size this frame; another edit ends the gesture.
    this_frame: bool,
}
impl WheelGesture {
    fn scrolled(&mut self) {
        self.last = Some(Instant::now());
        self.this_frame = true;
    }
    /// Called before a frame's edits are recorded. Returns whether a scroll ends here,
    /// to be recorded as its own step first: it paused, or this frame's edit came
    /// from something else (a slider, a click, a press that starts a drag).
    pub(in crate::app) fn ends_before(&mut self, edit: Edit) -> bool {
        let ends =
            self.last.is_some() && !self.this_frame && (edit == Edit::Changed || !self.active());
        if ends {
            self.last = None;
        }
        self.this_frame = false;
        ends
    }
    /// Whether a scroll is still going on, and so not yet a History step.
    pub(in crate::app) fn active(&self) -> bool {
        self.last.is_some_and(|at| at.elapsed() < GESTURE_PAUSE)
    }
    /// Whether a scroll's change may still be waiting to be recorded.
    pub(in crate::app) fn pending(&self) -> bool {
        self.last.is_some()
    }
    /// How long until the scroll counts as finished.
    pub(in crate::app) fn remaining(&self) -> Option<Duration> {
        self.last
            .map(|at| GESTURE_PAUSE.saturating_sub(at.elapsed()))
            .filter(|d| !d.is_zero())
    }
}

impl Editor {
    /// Records a scroll still being grouped as its own History step and hands it to
    /// Undo, before something that isn't an edit (a rating, a flag) is logged after it.
    pub(in crate::app) fn finish_wheel_gesture(&mut self) {
        if self.view.wheel.pending() {
            self.view.wheel = WheelGesture::default();
            self.document.edit.finish_gesture();
            self.sync_undo();
        }
    }
    /// Whether the open tool has a size, which the wheel over the photo and `[` `]`
    /// change.
    pub(in crate::app) fn tool_has_size(&self) -> bool {
        self.view.is(Tool::Remove) || self.view.is(Tool::Mask) || self.view.is(Tool::RedEye)
    }
    /// Sizes the open tool's brush or circle (or its feather) by `scroll`, with the
    /// limits `[` and `]` keep. The selected spot follows, as it does for the keys; the
    /// mask brush changes only while one is shown.
    pub(in crate::app) fn scroll_tool_size(&mut self, scroll: Scroll) {
        if self.view.is(Tool::Remove) {
            // A spot being moved takes its size from where the drag began.
            if self.view.retouch.is_dragging() {
                return;
            }
            let tool = &mut self.view.retouch;
            let op = tool
                .selected
                .and_then(|i| self.document.edit.recipe_mut().retouch.get_mut(i));
            // Only a selected spot's change is an edit, grouped as one History step;
            // the brush for new spots is a tool setting.
            if op.is_some() {
                self.view.wheel.scrolled();
            }
            match scroll.adjust {
                Adjust::Size => {
                    tool.size = (tool.size * scroll.scale()).clamp(0.001, 0.25);
                    if let Some(op) = op {
                        op.set_radius((op.radius() * scroll.scale()).clamp(0.001, 0.25));
                    }
                }
                Adjust::Feather => {
                    tool.feather = scroll.feather(tool.feather);
                    if let Some(op) = op {
                        op.feather = scroll.feather(op.feather);
                    }
                }
            }
        } else if self.view.is(Tool::Mask) && self.mask_brush_shown() {
            let t = &mut self.view.masking;
            let brush = match scroll.brush {
                MaskBrush::Erase => &mut t.brushes[2],
                MaskBrush::Current => &mut t.brushes[t.brush],
            };
            match scroll.adjust {
                Adjust::Size => brush.size = (brush.size * scroll.scale()).clamp(0.002, 0.5),
                Adjust::Feather => brush.feather = scroll.feather(brush.feather),
            }
        } else if self.view.is(Tool::RedEye) && scroll.adjust == Adjust::Size {
            self.view.red_eye.scale(scroll.scale());
        }
    }
}

#[cfg(test)]
#[path = "brush_scroll_editor_tests.rs"]
mod editor_tests;
