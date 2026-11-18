//! Lightroom's crop and Straighten, converted to and from RAWmakase's.
//!
//! Lightroom records `CropLeft`, `CropTop` and `CropRight`, `CropBottom` as opposite
//! corners of the crop rectangle, in 0–1 of the unrotated frame, and `CropAngle` as the
//! angle the rectangle is turned by. A positive angle turns the photo counter-clockwise
//! in the result. The crop's width and height are the corner-to-corner vector measured
//! along the turned axes. Camera Raw renders the following, measured on a synthetic
//! 970 × 742 chart (docs/transform.md):
//!
//! - A +5° crop from (0.1, 0.1) to (0.9, 0.9) comes out 825 × 524.
//! - A crop that reaches past the photo shrinks about its centre until it fits, also
//!   one whose corners are the whole frame (0, 0, 1, 1).
//! - Settings without corners leave the photo unturned, whatever the angle
//!   ([`super::apply`] keeps that rule, since it knows which keys were given).
//!
//! RAWmakase keeps an upright crop in 0–1 of the photo after Straighten has turned it
//! and enlarged it to fill its frame ([`straighten_zoom`]). A positive Straighten turns
//! the photo clockwise.
//!
//! Both work in the unrotated frame. A camera's quarter turn turns the photo and the
//! crop together, so it leaves the angle as it is.
use crate::model::image_frame::{crop_as_rendered, straighten_zoom};
use crate::model::recipe::Recipe;

/// The whole frame, as Lightroom's corners and as RAWmakase's crop.
pub(super) const WHOLE: [f32; 4] = [0., 0., 1., 1.];

/// The smallest crop side [`Recipe::validate`](crate::model::recipe::Recipe::validate)
/// accepts, as a fraction of the photo.
const MINIMUM: f32 = 0.01;

/// Lightroom's corners and angle for RAWmakase's `crop` (in the unrotated frame) and
/// `straighten` (degrees), on a photo of `size` (unrotated, any unit).
pub(super) fn to_lightroom(crop: [f32; 4], straighten: f32, size: [f32; 2]) -> ([f32; 4], f32) {
    // A crop that no longer lies on the photo writes the crop that renders, since its
    // corners would lie past the photo, where Lightroom's can't.
    let crop = crop_as_rendered(crop, straighten, size);
    if straighten == 0. {
        // Within the six decimals' margin of the photo's edge.
        return (crop.map(|v| v.clamp(0., 1.)), 0.);
    }
    let [width, height] = size;
    let zoom = straighten_zoom(straighten, size);
    // The crop's centre and size in pixels of the turned photo, from its centre.
    let x = ((crop[0] + crop[2]) / 2. - 0.5) * width / zoom;
    let y = ((crop[1] + crop[3]) / 2. - 0.5) * height / zoom;
    let w = (crop[2] - crop[0]) * width / zoom;
    let h = (crop[3] - crop[1]) * height / zoom;
    let angle = -straighten;
    let (s, c) = angle.to_radians().sin_cos();
    // The centre on the unturned photo, and half the corner-to-corner vector, along
    // the crop's axes (c, s) and (−s, c).
    let (px, py) = (c * x - s * y, s * x + c * y);
    let (hx, hy) = ((w * c - h * s) / 2., (w * s + h * c) / 2.);
    let corners = [
        (px - hx) / width + 0.5,
        (py - hy) / height + 0.5,
        (px + hx) / width + 0.5,
        (py + hy) / height + 0.5,
    ]
    .map(|v| v.clamp(0., 1.));
    (corners, angle)
}

/// RAWmakase's crop (in the unrotated frame) and Straighten for Lightroom's `corners`
/// and `angle` (degrees) on a photo of `size` (unrotated, any unit), or `None` when
/// the corners make no rectangle at that angle.
///
/// A crop on the photo, rotated by `angle`, keeps its size and place, as in Lightroom and
/// Camera Raw, though it may reach past the frame RAWmakase's Straighten enlarges (the
/// crop then has values outside 0-1). One that reaches past the photo shrinks about its
/// centre until it fits.
pub(super) fn from_lightroom(
    corners: [f32; 4],
    angle: f32,
    size: [f32; 2],
) -> Option<([f32; 4], f32)> {
    let (crop, straighten) = if angle == 0. {
        (corners, 0.)
    } else {
        turned(corners, angle, size)?
    };
    (crop[0] < crop[2] && crop[1] < crop[3]).then_some((crop, straighten))
}

/// `crop` (RAWmakase's, as the recipe holds it) with a side up to 1e-4 under the
/// smallest size widened back to it. The conversion, the six decimals Lightroom writes
/// and the camera orientation's turn move a side by about 1e-5, so a crop written at
/// that size would otherwise read back just under it and be refused.
pub(super) fn at_least_minimum(mut crop: [f32; 4]) -> [f32; 4] {
    for (lo, hi) in [(0, 2), (1, 3)] {
        if (MINIMUM - 1e-4..MINIMUM).contains(&(crop[hi] - crop[lo])) {
            // A crop at the frame's edge widens inwards; one past it, which a turned
            // crop can be, stays where it is.
            let end = if crop[hi] > 1. {
                *Recipe::CROP_RANGE.end()
            } else {
                1.
            };
            crop[hi] = (crop[lo] + MINIMUM + 1e-6).min(end);
            crop[lo] = crop[lo].min(crop[hi] - MINIMUM - 1e-6);
        }
    }
    crop
}

/// [`from_lightroom`] for a turned crop, before its size is checked.
fn turned(corners: [f32; 4], angle: f32, size: [f32; 2]) -> Option<([f32; 4], f32)> {
    let [width, height] = size;
    let (s, c) = angle.to_radians().sin_cos();
    let (dx, dy) = (
        (corners[2] - corners[0]) * width,
        (corners[3] - corners[1]) * height,
    );
    let (w, h) = (dx * c + dy * s, dy * c - dx * s);
    if w.is_nan() || h.is_nan() || w <= 0. || h <= 0. {
        return None;
    }
    let px = ((corners[0] + corners[2]) / 2. - 0.5) * width;
    let py = ((corners[1] + corners[3]) / 2. - 0.5) * height;
    let (x, y) = (c * px + s * py, c * py - s * px);
    let straighten = -angle;
    let zoom = straighten_zoom(straighten, size);
    // Camera Raw shrinks a crop about its centre only until it lies on the photo, so the
    // photo bounds it, not the enlarged frame. The crop may then reach past 0-1.
    let (ex, ey) = (
        c.abs() * w / 2. + s.abs() * h / 2.,
        s.abs() * w / 2. + c.abs() * h / 2.,
    );
    let fit = ((width / 2. - px.abs()) / ex)
        .min((height / 2. - py.abs()) / ey)
        .clamp(0., 1.);
    let centre = [0.5 + x * zoom / width, 0.5 + y * zoom / height];
    let half = [w * fit * zoom / width / 2., h * fit * zoom / height / 2.];
    let crop = [
        centre[0] - half[0],
        centre[1] - half[1],
        centre[0] + half[0],
        centre[1] + half[1],
    ];
    if crop.iter().all(|v| Recipe::CROP_RANGE.contains(v)) {
        return Some((crop, straighten));
    }
    // Only a long panorama turned far reaches past what the recipe holds. Such a crop
    // shrinks about its centre until it fits the enlarged frame, as every crop past
    // that frame did before.
    let centre = centre.map(|v| v.clamp(0., 1.));
    let fit = [
        centre[0] / half[0],
        (1. - centre[0]) / half[0],
        centre[1] / half[1],
        (1. - centre[1]) / half[1],
    ]
    .into_iter()
    .fold(1f32, f32::min);
    let half = half.map(|v| v * fit);
    let crop = [
        (centre[0] - half[0]).max(0.),
        (centre[1] - half[1]).max(0.),
        (centre[0] + half[0]).min(1.),
        (centre[1] + half[1]).min(1.),
    ];
    Some((crop, straighten))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The synthetic chart Camera Raw rendered (docs/transform.md).
    const CHART: [f32; 2] = [970., 742.];

    /// The output size Camera Raw renders `corners` at `angle` at, in chart pixels:
    /// RAWmakase's crop of the enlarged photo, at the photo's own scale.
    fn rendered(corners: [f32; 4], angle: f32) -> [f32; 2] {
        let (crop, straighten) = from_lightroom(corners, angle, CHART).unwrap();
        let zoom = straighten_zoom(straighten, CHART);
        [
            (crop[2] - crop[0]) * CHART[0] / zoom,
            (crop[3] - crop[1]) * CHART[1] / zoom,
        ]
    }

    /// Camera Raw's output sizes. A crop on the photo comes out at Camera Raw's size even
    /// when it reaches past the frame Straighten enlarges; one reaching past the photo
    /// shrinks about its centre to fit it (922 × 585).
    #[test]
    fn crops_come_out_as_camera_raw_renders_them() {
        for (corners, angle, camera_raw) in [
            ([0.1, 0.1, 0.9, 0.9], 5., [825., 524.]),
            ([0.1, 0.1, 0.9, 0.9], -5., [721., 659.]),
            ([0.06, 0.06, 0.94, 0.94], 5., [907., 576.]),
            ([0.05, 0.2, 0.6, 0.7], 5., [564., 323.]),
            ([0.03, 0.03, 0.97, 0.97], 5., [922., 585.]),
        ] {
            let got = rendered(corners, angle);
            assert!(
                (got[0] - camera_raw[0]).abs() < 1.5 && (got[1] - camera_raw[1]).abs() < 1.5,
                "{corners:?} at {angle}: {got:?}, Camera Raw {camera_raw:?}"
            );
        }
    }

    /// Camera Raw turns corners on the whole frame too, shrunk to fit (922 × 585).
    #[test]
    fn corners_on_the_whole_frame_turn_and_fit() {
        let (_, straighten) = from_lightroom(WHOLE, 5., CHART).unwrap();
        assert_eq!(straighten, -5.);
        let got = rendered(WHOLE, 5.);
        assert!(
            (got[0] / got[1] / (922. / 585.) - 1.).abs() < 0.005,
            "{got:?}"
        );
    }

    /// A crop at the smallest size, written to six decimals as Lightroom writes it,
    /// reads back at that size rather than just under it (found in #1072).
    #[test]
    fn the_smallest_crop_survives_six_decimals() {
        let six = |v: f32| (v * 1e6).round() / 1e6;
        for size in [CHART, [6000., 4000.], [4000., 6000.]] {
            for straighten in [-44.99, -30., -5., -1.23, 0., 1.23, 5., 30., 44.99] {
                for crop in [
                    [0.3, 0.4, 0.31, 0.41],
                    [0.5, 0.2, 0.5100001, 0.9],
                    [0., 0., 0.0100001, 0.0100001],
                    [0.9899999, 0.9899999, 1., 1.],
                ] {
                    let (corners, angle) = to_lightroom(crop, straighten, size);
                    let (back, _) = from_lightroom(corners.map(six), angle, size).unwrap();
                    let back = at_least_minimum(back);
                    let context = format!("{size:?} {straighten} {crop:?}: {back:?}");
                    assert!(
                        back[2] - back[0] >= MINIMUM && back[3] - back[1] >= MINIMUM,
                        "{context}"
                    );
                    // A crop reaching past the enlarged frame may sit a float's width outside 0-1.
                    assert!(
                        back.iter().all(|v| (-1e-4..=1.0001).contains(v)),
                        "{context}"
                    );
                    for (a, b) in back.iter().zip(crop) {
                        assert!((a - b).abs() < 1e-4, "{context}");
                    }
                }
            }
        }
        // A crop clearly under the smallest size is still refused by the recipe's check.
        let crop = at_least_minimum([0.3, 0.4, 0.309, 0.5]);
        assert!(crop[2] - crop[0] < MINIMUM);
    }

    #[test]
    fn corners_that_make_no_rectangle_are_refused() {
        // A thin horizontal strip cannot be a crop turned by 40°.
        assert_eq!(from_lightroom([0.1, 0.5, 0.9, 0.51], 40., CHART), None);
        assert_eq!(from_lightroom([0.6, 0.1, 0.4, 0.9], 0., CHART), None);
    }

    #[test]
    fn crops_round_trip() {
        for size in [CHART, [742., 970.], [6000., 4000.]] {
            for straighten in [-44., -5., -0.5, 0., 1.34, 4.06, 30.] {
                for crop in [WHOLE, [0.1, 0.2, 0.7, 0.9], [0., 0.3, 0.4, 1.]] {
                    let (corners, angle) = to_lightroom(crop, straighten, size);
                    let (back, s) = from_lightroom(corners, angle, size).unwrap();
                    assert!((s - straighten).abs() < 1e-6);
                    for (a, b) in back.iter().zip(crop) {
                        assert!(
                            (a - b).abs() < 2e-5,
                            "{size:?} {straighten} {crop:?}: {back:?}"
                        );
                    }
                }
            }
        }
    }

    /// An uncropped, straightened photo is written as the largest crop at its own
    /// aspect inside the turned photo, as Lightroom's crop tool leaves it.
    #[test]
    fn an_uncropped_straightened_photo_writes_the_fitted_crop() {
        let (corners, angle) = to_lightroom(WHOLE, -5., CHART);
        assert_eq!(angle, 5.);
        assert_ne!(corners, WHOLE);
        assert!(corners.iter().all(|v| (0. ..=1.).contains(v)));
    }

    /// A crop at the smallest size past the enlarged frame, which a turned crop can be,
    /// keeps its place when six decimals leave it just under that size.
    #[test]
    fn the_smallest_crop_past_the_frame_keeps_its_place() {
        let six = |v: f32| (v * 1e6).round() / 1e6;
        let (size, crop) = ([3000., 2000.], [1.01, 0.3, 1.020001, 0.4]);
        let (corners, angle) = to_lightroom(crop, -30., size);
        let (back, _) = from_lightroom(corners.map(six), angle, size).unwrap();
        let back = at_least_minimum(back);
        assert!(back[2] - back[0] >= MINIMUM, "{back:?}");
        for (a, b) in back.iter().zip(crop) {
            assert!((a - b).abs() < 1e-4, "{back:?}");
        }
    }

    /// On a long panorama turned far, a crop on the photo can reach further past the
    /// enlarged frame than the recipe holds (here to 2.01); it shrinks about its centre
    /// into that frame, rather than the edit being refused.
    #[test]
    fn a_panorama_crop_fits_the_recipe() {
        let (crop, straighten) =
            from_lightroom([0.375, 0., 0.425, 1.], 15., [6000., 1000.]).unwrap();
        assert_eq!(straighten, -15.);
        let recipe = Recipe {
            crop,
            straighten,
            ..Default::default()
        };
        assert!(recipe.validate().is_ok(), "{crop:?}");
        assert!(crop.iter().all(|v| (0. ..=1.).contains(v)), "{crop:?}");
    }

    /// Unturned, a crop past 0–1 (left there by Straighten reset to 0) writes the part
    /// that renders, since Lightroom's corners lie inside 0–1 and are refused outside it.
    #[test]
    fn an_unturned_crop_past_the_frame_writes_what_renders() {
        assert_eq!(
            to_lightroom([-0.02, 0.1, 1.02, 0.9], 0., CHART),
            ([0., 0.1, 1., 0.9], 0.)
        );
        assert_eq!(to_lightroom([1.2, 0.1, 1.5, 0.9], 0., CHART), (WHOLE, 0.));
    }
}
