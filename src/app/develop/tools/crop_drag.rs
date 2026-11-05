//! A drag on the Crop tool's photo: which handle it holds from the press to the
//! release, and the crop it makes. A handle moves its sides; with Alt (Option on a
//! Mac) held, the opposite sides move the other way, so the crop resizes about its
//! centre, as Lightroom's Alt-drag does (docs/transform.md#crop-and-straighten).
//!
//! Crops are `[left, top, right, bottom]` in 0–1 coordinates of the straightened photo
//! ([`crate::app::Editor::crop_frame`]); offsets are fractions of its width and height.
use eframe::egui::{Pos2, Rect, Vec2};

/// Farthest from a handle, in points, that a press still takes it rather than the crop.
pub(in crate::app) const HANDLE_REACH: f32 = 25.;

/// The smallest half width or height a crop resized about its centre is given: just over
/// half of the 0.01 that `Recipe::validate` requires, so rounding the centre's two sides
/// apart never leaves less.
const MIN_HALF: f32 = 0.00501;

/// One of the crop's eight handles: which side it moves across and which up or down,
/// −1 for left or top, 1 for right or bottom, 0 for neither (the middle of a side).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct Handle {
    pub(in crate::app) x: i8,
    pub(in crate::app) y: i8,
}

impl Handle {
    /// The four corners, then the four sides, in the order the viewport draws them.
    pub(in crate::app) const ALL: [Handle; 8] = [
        Handle { x: -1, y: -1 },
        Handle { x: 1, y: -1 },
        Handle { x: 1, y: 1 },
        Handle { x: -1, y: 1 },
        Handle { x: -1, y: 0 },
        Handle { x: 1, y: 0 },
        Handle { x: 0, y: -1 },
        Handle { x: 0, y: 1 },
    ];

    /// Where the handle is drawn on the crop `crop`, a rectangle on screen.
    pub(in crate::app) fn on(self, crop: Rect) -> Pos2 {
        let at = |side: i8, min: f32, max: f32| match side {
            -1 => min,
            1 => max,
            _ => (min + max) / 2.,
        };
        Pos2::new(
            at(self.x, crop.left(), crop.right()),
            at(self.y, crop.top(), crop.bottom()),
        )
    }

    /// The handle a press at `p` takes on the crop `crop` on screen: the nearest, within
    /// [`HANDLE_REACH`]. `None` moves the whole crop.
    pub(in crate::app) fn pressed(crop: Rect, p: Pos2) -> Option<Handle> {
        Self::ALL
            .into_iter()
            .map(|h| (h, h.on(crop).distance(p)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|(_, d)| *d < HANDLE_REACH)
            .map(|(h, _)| h)
    }
}

/// A drag on the crop, from the press to the release. Pressing or releasing Alt while it
/// is held starts a new part of the drag from the crop as it is then, so the crop does
/// not jump and what the earlier part did stays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct CropDrag {
    /// The crop when this part of the drag began.
    pub(in crate::app) start: [f32; 4],
    /// The handle pressed, or `None` when the drag moves the whole crop.
    pub(in crate::app) handle: Option<Handle>,
    /// How far the pointer had moved from the press, in points, when this part began.
    pub(in crate::app) from: Vec2,
    /// Whether this part resizes the crop about its centre (Alt held). Moving the whole
    /// crop ignores Alt.
    pub(in crate::app) about_centre: bool,
}

impl CropDrag {
    /// A drag pressed on `handle` of the crop `crop`.
    pub(in crate::app) fn new(crop: [f32; 4], handle: Option<Handle>, about_centre: bool) -> Self {
        CropDrag {
            start: crop,
            handle,
            from: Vec2::ZERO,
            about_centre: about_centre && handle.is_some(),
        }
    }

    /// Follows Alt as it is now, `moved` points from the press with the crop at `crop`:
    /// when it changes, the next part of the drag starts here.
    pub(in crate::app) fn follow_alt(&mut self, alt: bool, crop: [f32; 4], moved: Vec2) {
        let about_centre = alt && self.handle.is_some();
        if about_centre != self.about_centre {
            *self = CropDrag {
                start: crop,
                handle: self.handle,
                from: moved,
                about_centre,
            };
        }
    }
}

/// The crop `start` with `handle`'s sides moved by `d`, or the whole crop moved by `d`
/// within the photo when no handle is held. Each side stops 0.01 short of the one
/// opposite. The caller then fits the result to the aspect when it is locked.
pub(in crate::app) fn drag_sides(start: [f32; 4], handle: Option<Handle>, d: Vec2) -> [f32; 4] {
    let mut c = start;
    match handle {
        Some(h) => {
            match h.x {
                -1 => c[0] += d.x,
                1 => c[2] += d.x,
                _ => {}
            }
            match h.y {
                -1 => c[1] += d.y,
                1 => c[3] += d.y,
                _ => {}
            }
        }
        None => {
            let dx = d.x.clamp(-c[0], 1. - c[2]);
            let dy = d.y.clamp(-c[1], 1. - c[3]);
            c[0] += dx;
            c[2] += dx;
            c[1] += dy;
            c[3] += dy;
        }
    }
    c[0] = c[0].clamp(0., start[2] - 0.01);
    c[1] = c[1].clamp(0., start[3] - 0.01);
    c[2] = c[2].clamp(c[0] + 0.01, 1.);
    c[3] = c[3].clamp(c[1] + 0.01, 1.);
    c
}

/// The crop `start` resized about its centre, as Alt-dragging `handle` by `d` does: the
/// handle's sides move by `d` and the opposite ones by as much the other way. The centre
/// never moves, so the crop stops where its nearest side meets the photo's edge, and
/// at its smallest size.
///
/// `ratio` is the locked aspect ([`crate::app::Editor::crop_constraint`]): the crop's
/// width over its height in 0–1 coordinates, `None` while unlocked. Locked, a side sets
/// its own dimension and the other follows; a corner takes the size nearest the pointer,
/// measured on the photo as shown, whose width over height is `photo_aspect`. A crop
/// whose centre leaves it no locked size that fits is left as it is, rather than moved or
/// changed in ratio.
pub(in crate::app) fn resize_about_centre(
    start: [f32; 4],
    handle: Handle,
    d: Vec2,
    ratio: Option<f32>,
    photo_aspect: f32,
) -> [f32; 4] {
    let cx = (start[0] + start[2]) / 2.;
    let cy = (start[1] + start[3]) / 2.;
    let half_w = (start[2] - start[0]) / 2.;
    let half_h = (start[3] - start[1]) / 2.;
    // The half sizes the pointer asks for.
    let want_w = half_w + f32::from(handle.x) * d.x;
    let want_h = half_h + f32::from(handle.y) * d.y;
    let room_w = cx.min(1. - cx);
    let room_h = cy.min(1. - cy);
    let (w, h) = match ratio {
        None => {
            // A dimension the handle doesn't move, or one with no room to resize in,
            // stays as it is.
            let fit = |moves: bool, want: f32, room: f32, now: f32| {
                if !moves || room < MIN_HALF {
                    now
                } else {
                    want.clamp(MIN_HALF, room)
                }
            };
            (
                fit(handle.x != 0, want_w, room_w, half_w),
                fit(handle.y != 0, want_h, room_h, half_h),
            )
        }
        Some(ratio) => {
            // The half height; the half width is `ratio` times it.
            let t = match (handle.x, handle.y) {
                (_, 0) => want_w / ratio,
                (0, _) => want_h,
                _ => {
                    // The size along the locked diagonal nearest the pointer, in units of
                    // the photo's height so both directions count alike on screen.
                    let (ax, ay) = (ratio * photo_aspect, 1.);
                    (want_w * photo_aspect * ax + want_h * ay) / (ax * ax + ay * ay)
                }
            };
            let smallest = MIN_HALF.max(MIN_HALF / ratio);
            let largest = room_h.min(room_w / ratio);
            // NaN (a degenerate photo or ratio) counts as no room too.
            if smallest
                .partial_cmp(&largest)
                .is_none_or(std::cmp::Ordering::is_gt)
            {
                return start;
            }
            let t = t.clamp(smallest, largest);
            (ratio * t, t)
        }
    };
    let mut c = [
        (cx - w).max(0.),
        (cy - h).max(0.),
        (cx + w).min(1.),
        (cy + h).min(1.),
    ];
    // Sides a free crop doesn't resize keep their exact values, not ones rounded through
    // the centre.
    if ratio.is_none() {
        if w == half_w {
            (c[0], c[2]) = (start[0], start[2]);
        }
        if h == half_h {
            (c[1], c[3]) = (start[1], start[3]);
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centre(c: [f32; 4]) -> (f32, f32) {
        ((c[0] + c[2]) / 2., (c[1] + c[3]) / 2.)
    }

    fn assert_near(a: f32, b: f32, what: &str) {
        assert!((a - b).abs() < 1e-5, "{what}: {a} is not {b}");
    }

    #[test]
    fn every_handle_resizes_a_free_crop_about_its_centre() {
        let start = [0.2, 0.3, 0.6, 0.7];
        for h in Handle::ALL {
            // Outwards by 0.05 across and down, as the handle points; then inwards.
            for out in [1., -1.] {
                let d = Vec2::new(f32::from(h.x), f32::from(h.y)) * 0.05 * out;
                let c = resize_about_centre(start, h, d, None, 1.5);
                let (cx, cy) = centre(c);
                assert_near(cx, 0.4, "centre x");
                assert_near(cy, 0.5, "centre y");
                let grow = 0.1 * out;
                let w = if h.x != 0 { 0.4 + grow } else { 0.4 };
                let h_ = if h.y != 0 { 0.4 + grow } else { 0.4 };
                assert_near(c[2] - c[0], w, &format!("{h:?} {out} width"));
                assert_near(c[3] - c[1], h_, &format!("{h:?} {out} height"));
            }
        }
    }

    #[test]
    fn a_handle_moved_across_the_axis_of_another_changes_nothing_there() {
        // A side dragged along itself: the right side dragged up and down.
        let start = [0.2, 0.3, 0.6, 0.7];
        let c = resize_about_centre(start, Handle { x: 1, y: 0 }, Vec2::new(0., 0.2), None, 1.);
        assert_eq!(c, start);
    }

    #[test]
    fn a_locked_crop_keeps_its_ratio_and_centre_from_every_handle() {
        // A 4 × 5 crop on a 3:2 photo: 0.8 / 1.5 wide for every 1 high in 0–1 terms.
        let ratio = 0.8 / 1.5;
        let start = [0.5 - 0.1 * ratio, 0.4, 0.5 + 0.1 * ratio, 0.6];
        for h in Handle::ALL {
            for out in [1., -1.] {
                let d = Vec2::new(f32::from(h.x), f32::from(h.y)) * 0.03 * out;
                let c = resize_about_centre(start, h, d, Some(ratio), 1.5);
                let (cx, cy) = centre(c);
                assert_near(cx, 0.5, "centre x");
                assert_near(cy, 0.5, "centre y");
                assert_near(
                    (c[2] - c[0]) / (c[3] - c[1]),
                    ratio,
                    &format!("{h:?} {out} ratio"),
                );
                let grew = c[3] - c[1] > 0.2;
                assert_eq!(grew, out > 0., "{h:?} {out}: {c:?}");
            }
        }
        // A side sets its own dimension: the bottom down by 0.05 is 0.1 taller.
        let c = resize_about_centre(
            start,
            Handle { x: 0, y: 1 },
            Vec2::new(0., 0.05),
            Some(ratio),
            1.5,
        );
        assert_near(c[3] - c[1], 0.3, "height from the bottom");
        let c = resize_about_centre(
            start,
            Handle { x: 1, y: 0 },
            Vec2::new(0.05, 0.),
            Some(ratio),
            1.5,
        );
        assert_near(c[2] - c[0], 0.2 * ratio + 0.1, "width from the right");
    }

    #[test]
    fn a_locked_corner_takes_the_size_nearest_the_pointer_on_screen() {
        // A square crop on a square photo: the corner pulled right only grows the crop
        // by half as much each way, the nearest point on the diagonal.
        let start = [0.4, 0.4, 0.6, 0.6];
        let c = resize_about_centre(
            start,
            Handle { x: 1, y: 1 },
            Vec2::new(0.1, 0.),
            Some(1.),
            1.,
        );
        assert_near(c[2] - c[0], 0.3, "width");
        assert_near(c[3] - c[1], 0.3, "height");
        // On a 2:1 photo, 0.1 of the width is twice as far on screen as 0.1 of the
        // height: the same square in 0–1 terms (2:1 on screen) grows more from a pull
        // across than from one down.
        let across = resize_about_centre(
            start,
            Handle { x: 1, y: 1 },
            Vec2::new(0.1, 0.),
            Some(1.),
            2.,
        );
        let down = resize_about_centre(
            start,
            Handle { x: 1, y: 1 },
            Vec2::new(0., 0.1),
            Some(1.),
            2.,
        );
        assert!(
            across[2] - across[0] > down[2] - down[0],
            "{across:?} {down:?}"
        );
    }

    #[test]
    fn a_crop_resized_about_its_centre_stops_at_the_photo_and_at_its_smallest() {
        let start = [0.1, 0.3, 0.5, 0.7];
        // Out past the left edge: the centre at 0.3 leaves 0.3 each way.
        let c = resize_about_centre(start, Handle { x: 1, y: 0 }, Vec2::new(0.5, 0.), None, 1.5);
        assert_near(c[0], 0., "left");
        assert_near(c[2], 0.6, "right");
        // In past the centre: the smallest crop, still about it, and valid.
        let c = resize_about_centre(
            start,
            Handle { x: -1, y: -1 },
            Vec2::new(0.5, 0.5),
            None,
            1.5,
        );
        assert_near(centre(c).0, 0.3, "centre x");
        assert_near(centre(c).1, 0.5, "centre y");
        assert!(c[2] - c[0] >= 0.01 && c[3] - c[1] >= 0.01, "{c:?}");
        // Locked, the dimension with the least room stops both.
        let c = resize_about_centre(
            start,
            Handle { x: 1, y: 1 },
            Vec2::new(0.5, 0.5),
            Some(1.),
            1.,
        );
        assert_near(c[2] - c[0], 0.6, "width");
        assert_near(c[3] - c[1], 0.6, "height");
        assert_near(c[0], 0., "left");
    }

    #[test]
    fn a_locked_crop_with_no_room_about_its_centre_is_left_as_it_is() {
        // A 0.01-wide sliver at the photo's very edge, locked to a wide ratio: any
        // height the ratio allows there is too narrow to be a crop.
        let start = [0., 0.3, 0.01, 0.7];
        let c = resize_about_centre(
            start,
            Handle { x: 0, y: 1 },
            Vec2::new(0., 0.1),
            Some(10.),
            1.,
        );
        assert_eq!(c, start);
    }

    #[test]
    fn alt_pressed_or_released_starts_the_next_part_from_the_crop_as_it_is() {
        let mut drag = CropDrag::new([0.2, 0.2, 0.6, 0.6], Some(Handle { x: 1, y: 0 }), false);
        drag.follow_alt(false, [0.2, 0.2, 0.7, 0.6], Vec2::new(30., 0.));
        assert_eq!(drag.start, [0.2, 0.2, 0.6, 0.6], "no change, no new part");
        drag.follow_alt(true, [0.2, 0.2, 0.7, 0.6], Vec2::new(30., 0.));
        assert_eq!(
            drag,
            CropDrag {
                start: [0.2, 0.2, 0.7, 0.6],
                handle: Some(Handle { x: 1, y: 0 }),
                from: Vec2::new(30., 0.),
                about_centre: true,
            }
        );
        // Moving the whole crop ignores Alt.
        let mut drag = CropDrag::new([0.2, 0.2, 0.6, 0.6], None, true);
        assert!(!drag.about_centre);
        drag.follow_alt(true, [0.3, 0.2, 0.7, 0.6], Vec2::new(30., 0.));
        assert_eq!(drag.from, Vec2::ZERO);
    }

    #[test]
    fn a_press_takes_the_nearest_handle_within_reach() {
        let crop = Rect::from_min_max(Pos2::new(100., 100.), Pos2::new(300., 200.));
        assert_eq!(
            Handle::pressed(crop, Pos2::new(305., 150.)),
            Some(Handle { x: 1, y: 0 })
        );
        assert_eq!(
            Handle::pressed(crop, Pos2::new(95., 205.)),
            Some(Handle { x: -1, y: 1 })
        );
        assert_eq!(Handle::pressed(crop, Pos2::new(200., 150.)), None);
    }
}
