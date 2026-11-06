//! Rotate and Flip as the Crop panel applies them: on the photo as shown, keeping the
//! crop and straightening on the same part of the photo (docs/transform.md#crop-and-straighten).
use crate::model::recipe::Recipe;

/// A quarter turn of the photo as shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuarterTurn {
    Left,
    Right,
}

/// A mirror of the photo as shown, across its vertical or horizontal centre line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mirror {
    Horizontal,
    Vertical,
}

/// Turns the photo as shown a quarter turn. The crop turns with it, so it frames the
/// same part of the photo, and the straighten angle is unchanged.
pub fn turn(r: &mut Recipe, turn: QuarterTurn) {
    // The flips apply after the turns, as the photo is shown: under a single mirror a
    // turn of the recorded photo shows the other way round.
    let mirrored = r.flip_x != r.flip_y;
    let clockwise = (turn == QuarterTurn::Right) != mirrored;
    r.rotation = if clockwise {
        (r.rotation + 1) % 4
    } else {
        (r.rotation + 3) % 4
    };
    let [left, top, right, bottom] = r.crop;
    r.crop = match turn {
        QuarterTurn::Right => [1. - bottom, left, 1. - top, right],
        QuarterTurn::Left => [top, 1. - right, bottom, 1. - left],
    };
}

/// Mirrors the photo as shown. The crop mirrors with it and the straighten angle turns
/// the other way, so the same part of the photo stays framed.
pub fn mirror(r: &mut Recipe, mirror: Mirror) {
    let [left, top, right, bottom] = r.crop;
    match mirror {
        Mirror::Horizontal => {
            r.flip_x = !r.flip_x;
            r.crop = [1. - right, top, 1. - left, bottom];
        }
        Mirror::Vertical => {
            r.flip_y = !r.flip_y;
            r.crop = [left, 1. - bottom, right, 1. - top];
        }
    }
    r.straighten = -r.straighten;
    // The mirrored photo measures as the mirrored angle; Auto stays Auto.
    r.straighten_auto = r.straighten_auto.map(|auto| auto.mirrored());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{camera_data::CameraImage, develop::Geometry};

    fn photo() -> CameraImage {
        CameraImage {
            width: 300,
            height: 200,
            pixels: vec![[0.2; 3]; 300 * 200],
            metadata: crate::camera_data::Metadata {
                width: 300,
                height: 200,
                wb: [1.; 3],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
            recovered: Default::default(),
        }
    }

    /// Where each corner of the crop and its centre come from in the photo as decoded,
    /// in no particular order.
    fn framed(r: &Recipe) -> Vec<[f32; 2]> {
        let g = Geometry::new(&photo(), r, 0);
        let mut points: Vec<[f32; 2]> = [[0., 0.], [1., 0.], [1., 1.], [0., 1.], [0.5, 0.5]]
            .iter()
            .map(|&[u, v]| g.source(u, v))
            .collect();
        points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
        points
    }

    fn assert_same(a: &[[f32; 2]], b: &[[f32; 2]], what: &str) {
        for (p, q) in a.iter().zip(b) {
            assert!(
                (p[0] - q[0]).abs() < 0.05 && (p[1] - q[1]).abs() < 0.05,
                "{what}: {a:?} != {b:?}"
            );
        }
    }

    fn cropped() -> Vec<Recipe> {
        let mut out = Vec::new();
        for rotation in 0..4 {
            for (flip_x, flip_y) in [(false, false), (true, false), (false, true), (true, true)] {
                out.push(Recipe {
                    crop: [0.1, 0.2, 0.55, 0.9],
                    straighten: 7.,
                    rotation,
                    flip_x,
                    flip_y,
                    ..Default::default()
                });
            }
        }
        out
    }

    #[test]
    fn turning_keeps_the_crop_on_the_same_part_of_the_photo() {
        for r in cropped() {
            for t in [QuarterTurn::Left, QuarterTurn::Right] {
                let mut turned = r.clone();
                turn(&mut turned, t);
                let what = format!("{t:?} from {r:?}");
                assert_same(&framed(&turned), &framed(&r), &what);
                assert_eq!(turned.straighten, r.straighten, "{what}");
            }
        }
    }

    #[test]
    fn rotate_right_turns_the_shown_photo_clockwise() {
        for r in cropped() {
            let mut turned = r.clone();
            turn(&mut turned, QuarterTurn::Right);
            let (before, after) = (
                Geometry::new(&photo(), &r, 0),
                Geometry::new(&photo(), &turned, 0),
            );
            // The crop's top-left corner moves to its top-right.
            let (a, b) = (before.source(0., 0.), after.source(1., 0.));
            assert!(
                (a[0] - b[0]).abs() < 0.05 && (a[1] - b[1]).abs() < 0.05,
                "{r:?}: {a:?} {b:?}"
            );
        }
    }

    #[test]
    fn mirroring_keeps_the_crop_on_the_same_part_of_the_photo() {
        for r in cropped() {
            for m in [Mirror::Horizontal, Mirror::Vertical] {
                let mut mirrored = r.clone();
                mirror(&mut mirrored, m);
                let what = format!("{m:?} from {r:?}");
                assert_same(&framed(&mirrored), &framed(&r), &what);
                // Mirrored on screen: the crop's top-left corner is now its top-right
                // (horizontal) or bottom-left (vertical).
                let (before, after) = (
                    Geometry::new(&photo(), &r, 0),
                    Geometry::new(&photo(), &mirrored, 0),
                );
                let corner = match m {
                    Mirror::Horizontal => [1., 0.],
                    Mirror::Vertical => [0., 1.],
                };
                let (a, b) = (before.source(0., 0.), after.source(corner[0], corner[1]));
                assert!(
                    (a[0] - b[0]).abs() < 0.05 && (a[1] - b[1]).abs() < 0.05,
                    "{what}: {a:?} {b:?}"
                );
            }
        }
    }

    /// A flip turns the angle the other way, and Auto's angle with it: a Straighten
    /// waiting to be measured still waits.
    #[test]
    fn mirroring_keeps_an_auto_straighten_auto() {
        use crate::model::transform::AutoStraighten;
        for m in [Mirror::Horizontal, Mirror::Vertical] {
            let mut r = Recipe {
                straighten: 1.,
                straighten_auto: Some(AutoStraighten::Pending(1.)),
                ..Default::default()
            };
            mirror(&mut r, m);
            assert_eq!(r.straighten, -1.);
            assert!(r.straighten_pending(), "{m:?}");
            r.set_auto_straighten(Some(2.));
            mirror(&mut r, m);
            assert_eq!(r.auto_straighten(), Some(AutoStraighten::Measured(-2.)));
        }
    }

    #[test]
    fn four_turns_and_two_mirrors_give_back_the_same_numbers() {
        for r in cropped() {
            let mut a = r.clone();
            for _ in 0..4 {
                turn(&mut a, QuarterTurn::Right);
            }
            turn(&mut a, QuarterTurn::Left);
            turn(&mut a, QuarterTurn::Right);
            mirror(&mut a, Mirror::Horizontal);
            mirror(&mut a, Mirror::Vertical);
            mirror(&mut a, Mirror::Vertical);
            mirror(&mut a, Mirror::Horizontal);
            for (x, y) in a.crop.iter().zip(r.crop) {
                assert!((x - y).abs() < 1e-6, "{:?} {:?}", a.crop, r.crop);
            }
            a.crop = r.crop;
            assert_eq!(a, r);
        }
    }
}
