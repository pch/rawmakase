//! Develop's clipping warnings, as Lightroom's histogram triangles: shadows and
//! highlights are shown independently, hovering a triangle shows its warning
//! while the pointer stays there, and J turns both on or off. View state only,
//! never saved with the edit.
use crate::rendered::{ClipOverlay, Histogram};
use eframe::egui::Color32;

/// One end of the histogram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum ClipSide {
    Shadows,
    Highlights,
}
impl ClipSide {
    pub(in crate::app) const BOTH: [Self; 2] = [Self::Shadows, Self::Highlights];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct ClippingView {
    /// Turned on by a triangle's click or by J.
    on: ClipOverlay,
    /// The triangle under the pointer, shown only while it stays there.
    hover: Option<ClipSide>,
}
impl ClippingView {
    pub(in crate::app) fn is_on(&self, side: ClipSide) -> bool {
        match side {
            ClipSide::Shadows => self.on.shadows,
            ClipSide::Highlights => self.on.highlights,
        }
    }
    /// Both warnings are on, as the toolbar's Clipping button shows.
    pub(in crate::app) fn both_on(&self) -> bool {
        self.on.shadows && self.on.highlights
    }
    /// A triangle's click.
    pub(in crate::app) fn toggle(&mut self, side: ClipSide) {
        let on = !self.is_on(side);
        match side {
            ClipSide::Shadows => self.on.shadows = on,
            ClipSide::Highlights => self.on.highlights = on,
        }
    }
    /// J: both on when either is off, otherwise both off, as in Lightroom.
    pub(in crate::app) fn toggle_both(&mut self) {
        let on = !self.both_on();
        self.on = ClipOverlay {
            shadows: on,
            highlights: on,
        };
    }
    pub(in crate::app) fn set_hover(&mut self, side: Option<ClipSide>) {
        self.hover = side;
    }
    /// Off, as leaving Develop leaves it.
    pub(in crate::app) fn clear(&mut self) {
        *self = Self::default();
    }
    /// The warnings painted over the photo: those turned on and the hovered one.
    pub(in crate::app) fn overlay(&self) -> ClipOverlay {
        let hovered = |side| self.hover == Some(side);
        ClipOverlay {
            shadows: self.on.shadows || hovered(ClipSide::Shadows),
            highlights: self.on.highlights || hovered(ClipSide::Highlights),
        }
    }
}

/// A channel clips visibly when more than this share of the pixels does.
pub(in crate::app) const CLIPPED_SHARE: f32 = 0.001;

/// Which channels clip at `side`: red, green, blue.
pub(in crate::app) fn clipped_channels(histogram: &Histogram, side: ClipSide) -> [bool; 3] {
    let counts = match side {
        ClipSide::Shadows => histogram.clipped.shadows,
        ClipSide::Highlights => histogram.clipped.highlights,
    };
    let total = histogram.total().max(1) as f32;
    counts.map(|n| n as f32 / total > CLIPPED_SHARE)
}
/// The triangle's colour: the clipping channels mixed, white when all three
/// clip, as Lightroom's; `None` when none clips.
pub(in crate::app) fn indicator_color(channels: [bool; 3]) -> Option<Color32> {
    let [r, g, b] = channels.map(|on| if on { 235 } else { 60 });
    channels.contains(&true).then(|| Color32::from_rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_and_highlight_warnings_toggle_independently() {
        let mut view = ClippingView::default();
        view.toggle(ClipSide::Shadows);
        assert_eq!(
            view.overlay(),
            ClipOverlay {
                shadows: true,
                highlights: false
            }
        );
        view.toggle(ClipSide::Highlights);
        view.toggle(ClipSide::Shadows);
        assert_eq!(
            view.overlay(),
            ClipOverlay {
                shadows: false,
                highlights: true
            }
        );
    }

    #[test]
    fn j_turns_both_on_when_either_is_off_and_both_off_otherwise() {
        let mut view = ClippingView::default();
        view.toggle_both();
        assert!(view.both_on());
        view.toggle_both();
        assert_eq!(view.overlay(), ClipOverlay::NONE);
        // One on: J turns the other on too, not this one off.
        view.toggle(ClipSide::Highlights);
        view.toggle_both();
        assert!(view.both_on());
    }

    #[test]
    fn hovering_a_triangle_shows_its_warning_only_while_hovered() {
        let mut view = ClippingView::default();
        view.set_hover(Some(ClipSide::Highlights));
        assert_eq!(
            view.overlay(),
            ClipOverlay {
                shadows: false,
                highlights: true
            }
        );
        assert!(!view.is_on(ClipSide::Highlights));
        view.set_hover(None);
        assert_eq!(view.overlay(), ClipOverlay::NONE);
        // Hovering a warning that is on changes nothing, and leaving keeps it on.
        view.toggle(ClipSide::Shadows);
        view.set_hover(Some(ClipSide::Shadows));
        view.set_hover(None);
        assert!(view.overlay().shadows);
    }

    #[test]
    fn triangles_light_from_the_working_space_counts_not_the_end_bins() {
        let mut h = Histogram::EMPTY;
        h.bins[1][128] = 10_000;
        // Many pixels in the top bin that do not reach the threshold, e.g. 0.998,
        // light nothing.
        h.bins[0][255] = 5_000;
        assert_eq!(clipped_channels(&h, ClipSide::Highlights), [false; 3]);
        h.clipped.highlights = [50, 50, 0];
        h.clipped.shadows = [0, 0, 5];
        assert_eq!(
            clipped_channels(&h, ClipSide::Highlights),
            [true, true, false]
        );
        // 5 of 10,000 is under the visible share.
        assert_eq!(clipped_channels(&h, ClipSide::Shadows), [false; 3]);
        assert_eq!(indicator_color([false; 3]), None);
        assert_eq!(
            indicator_color([true, true, false]),
            Some(Color32::from_rgb(235, 235, 60))
        );
        assert_eq!(
            indicator_color([true; 3]),
            Some(Color32::from_rgb(235, 235, 235))
        );
    }
}

#[cfg(test)]
#[path = "clipping_editor_tests.rs"]
mod editor_tests;
