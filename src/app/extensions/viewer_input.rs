//! Viewer input: what the user points at on the photo before a request runs,
//! collected here and sent as [`ViewerInput`].
//!
//! The `prompt` kind serves mask generators that aim and refine with clicks
//! and a box (Select Subject's own aiming among them), and commands that take
//! it (`super::command_input`):
//!
//! - a click adds a positive point, an Alt-click a negative one;
//! - a drag draws a box, and a new box starts over, clearing the clicks;
//! - a negative first click, with no box yet, first takes the extent of the
//!   raster being refined as the box, since a negative point means nothing
//!   without something to be negative about;
//! - at most [`MAX_CLICKS`] clicks are kept;
//! - a drag under 1% of the photo on either side is not a box. The minimum is
//!   the gesture's alone: a box taken from a raster's extent may be thinner.
//!
//! Commands that take `point`, `points` or `box` collect them here too: plain
//! clicks ([`Collector::add`]) or one dragged box ([`Collector::drag`]).
//!
//! The caller decides when a change is sent: the mask generator's flow allows
//! one request in flight and ignores input while it runs. Coordinates are in
//! the image frame ([`crate::model::image_frame`]), normalised to [0, 1], as
//! masks and the protocol use them.
use crate::app::ui::design::text;
use eframe::egui;
use rawmakase_extensions::protocol::viewer::{Bounds, Click, MAX_CLICKS, ViewerInput};

/// The smallest dragged box, as a fraction of the photo on each side.
const MIN_DRAG: f32 = 0.01;

/// The clicks and box collected so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::app) struct Collector {
    /// Oldest first, at most [`MAX_CLICKS`].
    pub(in crate::app) clicks: Vec<Click>,
    /// `[x0, y0, x1, y1]`, ordered.
    pub(in crate::app) bounds: Option<[f32; 4]>,
    /// Where a drag for a box began.
    drag_from: Option<[f32; 2]>,
}

/// What a gesture on the photo was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) enum Gesture {
    Click { at: [f32; 2], positive: bool },
    Drag { from: [f32; 2], to: [f32; 2] },
}

impl Collector {
    /// Takes a click at `at`. A negative first click with no box takes
    /// `extent()` as the box, and is dropped when there is none. Returns
    /// whether the input changed, and so is to be sent again; a click past the
    /// cap changes nothing but is still sent, as the user asked again.
    pub(in crate::app) fn click(
        &mut self,
        at: [f32; 2],
        positive: bool,
        extent: impl FnOnce() -> Option<[f32; 4]>,
    ) -> bool {
        if !inside(at[0]) || !inside(at[1]) {
            return false;
        }
        if !positive && self.clicks.iter().all(|c| !c.positive) && self.bounds.is_none() {
            let Some(extent) = extent() else {
                return false;
            };
            self.bounds = Some(extent);
        }
        if self.clicks.len() < MAX_CLICKS {
            self.clicks.push(Click {
                x: at[0],
                y: at[1],
                positive,
            });
        }
        true
    }

    /// Adds a click at `at` as it is, for `point` and `points`, which have
    /// no box for a negative click to need. Returns whether it was added: not
    /// off the photo, nor past [`MAX_CLICKS`].
    pub(in crate::app) fn add(&mut self, at: [f32; 2], positive: bool) -> bool {
        if !inside(at[0]) || !inside(at[1]) || self.clicks.len() >= MAX_CLICKS {
            return false;
        }
        self.clicks.push(Click {
            x: at[0],
            y: at[1],
            positive,
        });
        true
    }

    /// Takes a box dragged from `a` to `b`, clamped to the photo. A box under
    /// [`MIN_DRAG`] on either side is ignored; one that isn't starts over: the
    /// clicks so far were about the previous outline. Returns whether it was
    /// taken.
    pub(in crate::app) fn drag(&mut self, a: [f32; 2], b: [f32; 2]) -> bool {
        let clamp = |v: f32| v.clamp(0., 1.);
        let bounds = [
            clamp(a[0].min(b[0])),
            clamp(a[1].min(b[1])),
            clamp(a[0].max(b[0])),
            clamp(a[1].max(b[1])),
        ];
        if !(bounds[2] - bounds[0] >= MIN_DRAG && bounds[3] - bounds[1] >= MIN_DRAG) {
            return false;
        }
        self.clicks.clear();
        self.bounds = Some(bounds);
        true
    }

    /// Forgets the clicks and box, so the next click starts over.
    pub(in crate::app) fn clear(&mut self) {
        self.clicks.clear();
        self.bounds = None;
        self.drag_from = None;
    }

    /// The input as a request carries it: `prompt`, with the box when there
    /// is one.
    pub(in crate::app) fn input(&self) -> ViewerInput {
        ViewerInput::Prompt {
            clicks: self.clicks.clone(),
            bounds: self
                .bounds
                .map(|[x0, y0, x1, y1]| Bounds { x0, y0, x1, y1 }),
        }
    }

    /// Reads the pointer over the photo and draws the clicks and box. With
    /// `busy`, gestures are ignored rather than queued, and the cursor says
    /// so; without `drags`, a drag is no gesture and draws no box. Returns
    /// the gesture made this frame, if any.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn overlay(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        busy: bool,
        drags: bool,
        to_screen: &dyn Fn([f32; 2]) -> egui::Pos2,
        to_image: &dyn Fn(egui::Pos2) -> [f32; 2],
    ) -> Option<Gesture> {
        let mut gesture = None;
        if !busy {
            if drags
                && response.drag_started()
                && let Some(origin) = ui.input(|i| i.pointer.press_origin())
            {
                self.drag_from = Some(to_image(origin));
            }
            if response.drag_stopped()
                && let (Some(from), Some(pos)) =
                    (self.drag_from.take(), response.interact_pointer_pos())
            {
                gesture = Some(Gesture::Drag {
                    from,
                    to: to_image(pos),
                });
            }
            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
            {
                let alt = ui.input(|i| i.modifiers.alt);
                gesture = Some(Gesture::Click {
                    at: to_image(pos),
                    positive: !alt,
                });
            }
        }
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let green = egui::Color32::from_rgb(110, 200, 130);
        let white = egui::Stroke::new(1.5, egui::Color32::WHITE);
        if let Some(b) = self.bounds {
            let r = egui::Rect::from_two_pos(to_screen([b[0], b[1]]), to_screen([b[2], b[3]]));
            painter.rect_stroke(
                r,
                0.,
                egui::Stroke::new(1.5, green),
                egui::StrokeKind::Middle,
            );
        }
        if let (Some(from), Some(pos)) = (self.drag_from, response.interact_pointer_pos())
            && response.dragged()
        {
            let r = egui::Rect::from_two_pos(to_screen(from), pos);
            painter.rect_stroke(r, 0., white, egui::StrokeKind::Middle);
        }
        for click in &self.clicks {
            let at = to_screen([click.x, click.y]);
            let color = if click.positive {
                green
            } else {
                egui::Color32::from_rgb(230, 90, 80)
            };
            painter.circle(at, 6., color, white);
            if !click.positive {
                for d in [egui::vec2(3., 3.), egui::vec2(3., -3.)] {
                    painter.line_segment([at - d, at + d], white);
                }
            }
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(if busy {
                egui::CursorIcon::Progress
            } else {
                egui::CursorIcon::Crosshair
            });
        }
        gesture
    }

    /// What has been collected, for the aiming card: "2 points and a box".
    pub(in crate::app) fn summary(&self) -> egui::RichText {
        let points = self.clicks.len();
        egui::RichText::new(format!(
            "{points} point{}{}",
            if points == 1 { "" } else { "s" },
            if self.bounds.is_some() {
                " and a box"
            } else {
                ""
            }
        ))
        .size(text::SMALL)
        .color(egui::Color32::LIGHT_GRAY)
    }
}

fn inside(v: f32) -> bool {
    v.is_finite() && (0. ..=1.).contains(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn at(x: f32, y: f32, positive: bool) -> Click {
        Click { x, y, positive }
    }

    #[test]
    fn a_box_starts_over_and_clicks_join_it() {
        let mut c = Collector::default();
        assert!(c.click([0.5, 0.5], true, || None));
        assert!(c.drag([0.8, 0.9], [0.2, 0.1]));
        assert_eq!((c.clicks.len(), c.bounds), (0, Some([0.2, 0.1, 0.8, 0.9])));
        assert!(c.click([0.3, 0.4], false, || unreachable!("a box is there")));
        assert_eq!(
            c.input(),
            ViewerInput::Prompt {
                clicks: vec![at(0.3, 0.4, false)],
                bounds: Some(Bounds {
                    x0: 0.2,
                    y0: 0.1,
                    x1: 0.8,
                    y1: 0.9
                }),
            }
        );
        assert!(c.input().validate().is_ok());
    }

    #[test]
    fn the_drag_minimum_is_the_gestures_alone() {
        let mut c = Collector::default();
        assert!(!c.drag([0.5, 0.2], [0.509, 0.8]));
        assert!(!c.drag([0.2, 0.5], [0.8, 0.509]));
        assert_eq!(c.bounds, None);
        // A thin extent is a box all the same.
        assert!(c.click([0.002, 0.5], false, || Some([0., 0., 0.005, 1.])));
        assert_eq!(c.bounds, Some([0., 0., 0.005, 1.]));
        assert!(c.input().validate().is_ok());
    }

    #[test]
    fn a_negative_first_click_needs_something_to_leave_out_of() {
        let mut c = Collector::default();
        assert!(!c.click([0.5, 0.5], false, || None));
        assert!(c.clicks.is_empty());
        assert!(!c.click([1.5, 0.5], true, || None), "off the photo");
    }

    #[test]
    fn plain_clicks_stop_at_the_cap_and_need_no_box() {
        let mut c = Collector::default();
        assert!(c.add([0.5, 0.5], false), "a negative click needs no box");
        assert!(!c.add([0.5, 1.5], true), "off the photo");
        for _ in 1..MAX_CLICKS {
            assert!(c.add([0.25, 0.25], true));
        }
        assert!(!c.add([0.75, 0.75], true));
        assert_eq!(c.clicks.len(), MAX_CLICKS);
        assert_eq!(c.clicks[0], at(0.5, 0.5, false));
    }

    #[test]
    fn clicks_past_the_cap_are_sent_without_the_extra_point() {
        let mut c = Collector {
            clicks: vec![at(0.5, 0.5, true); MAX_CLICKS],
            ..Default::default()
        };
        assert!(c.click([0.1, 0.1], true, || None));
        assert_eq!(c.clicks.len(), MAX_CLICKS);
        assert!(!c.clicks.contains(&at(0.1, 0.1, true)));
    }
}
