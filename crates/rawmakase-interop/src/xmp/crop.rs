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
use crate::model::image_frame::straighten_zoom;

/// The whole frame, as Lightroom's corners and as RAWmakase's crop.
pub(super) const WHOLE: [f32; 4] = [0., 0., 1., 1.];

/// The smallest crop side [`Recipe::validate`](crate::model::recipe::Recipe::validate)
/// accepts, as a fraction of the photo.
const MINIMUM: f32 = 0.01;

/// Lightroom's corners and angle for RAWmakase's `crop` (in the unrotated frame) and
/// `straighten` (degrees), on a photo of `size` (unrotated, any unit).
pub(super) fn to_lightroom(crop: [f32; 4], straighten: f32, size: [f32; 2]) -> ([f32; 4], f32) {
    if straighten == 0. {
        return (crop, 0.);
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
/// The crop always lies inside 0–1. A Lightroom crop that is wider or taller than the
/// photo enlarged by Straighten allows shrinks about its centre until it fits, as Camera
/// Raw shrinks a crop that reaches past the photo. Lightroom allows such a crop when it
/// has another aspect ratio than the photo, so RAWmakase crops it a little tighter.
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
            crop[hi] = (crop[lo] + MINIMUM + 1e-6).min(1.);
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
    let centre = [
        (0.5 + x * zoom / width).clamp(0., 1.),
        (0.5 + y * zoom / height).clamp(0., 1.),
    ];
    let half = [w * zoom / width / 2., h * zoom / height / 2.];
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

    /// Camera Raw's output sizes. A crop inside the photo Straighten enlarges comes out
    /// at Camera Raw's size; one reaching past it at Camera Raw's aspect, smaller.
    #[test]
    fn crops_come_out_as_camera_raw_renders_them() {
        for (corners, angle, camera_raw, fits) in [
            ([0.1, 0.1, 0.9, 0.9], 5., [825., 524.], true),
            ([0.1, 0.1, 0.9, 0.9], -5., [721., 659.], true),
            // Inside the turned photo, but wider than the enlarged one or reaching past
            // its edge.
            ([0.06, 0.06, 0.94, 0.94], 5., [907., 576.], false),
            ([0.05, 0.2, 0.6, 0.7], 5., [564., 323.], false),
        ] {
            let got = rendered(corners, angle);
            let context = format!("{corners:?} at {angle}: {got:?}, Camera Raw {camera_raw:?}");
            if fits {
                assert!(
                    (got[0] - camera_raw[0]).abs() < 1. && (got[1] - camera_raw[1]).abs() < 1.,
                    "{context}"
                );
            } else {
                let aspect = |s: [f32; 2]| s[0] / s[1];
                assert!(
                    (aspect(got) / aspect(camera_raw) - 1.).abs() < 0.005,
                    "{context}"
                );
                assert!(got[0] < camera_raw[0], "{context}");
            }
        }
        // Past the photo itself, Camera Raw shrinks the crop about its centre to fit
        // (922 × 585); RAWmakase fits it to the enlarged photo at the same aspect.
        let got = rendered([0.03, 0.03, 0.97, 0.97], 5.);
        assert!(
            (got[0] / got[1] / (922. / 585.) - 1.).abs() < 0.005,
            "{got:?}"
        );
    }

    #[test]
    fn a_positive_crop_angle_is_a_negative_straighten() {
        let (_, straighten) = from_lightroom([0.1, 0.1, 0.9, 0.9], 5., CHART).unwrap();
        assert_eq!(straighten, -5.);
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
                    assert!(back.iter().all(|v| (0. ..=1.).contains(v)), "{context}");
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
}
