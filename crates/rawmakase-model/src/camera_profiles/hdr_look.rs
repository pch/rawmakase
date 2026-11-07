//! How a look's tone curve renders in HDR, as Camera Raw 18.7 renders it
//! (docs/records/hdr-stage1-results.md#looks).
//!
//! A look's curve (`crs:ToneCurvePV2012` in its profile) works on sRGB-encoded values
//! from 0 to 1 in SDR. In HDR, Camera Raw applies it in an encoding that is sRGB's up
//! to graphics white and grows with the logarithm of the value above it
//! ([`encode`]), and changes the curve above white in one of two ways:
//!
//! - A curve that ends at white (its last point is 255, 255) follows itself up to
//!   where it lifts its input most, and from there runs parallel to the identity at
//!   that lift, up to four stops above white ([`PEAK`]). A curve that never lifts its
//!   input therefore renders as the identity, below white too.
//! - A curve that ends below white keeps its shape up to white and continues along
//!   its slope there.
//!
//! Measured on gray ramps (linear and ACR3 profile curves, sensor white +4 and +6
//! stops, HDR limits 2, 4 and 8) through sixteen synthetic curves and Adobe Color's.
//! The rule and the encoding's two constants, fitted on all of them, render each
//! within 0.063 stop up to scene 15.5 (median 0.006). The result does not depend on the
//! HDR limit. Above four stops, a curve that ends at white is held at [`PEAK`];
//! Camera Raw drifts from it by up to 0.25 stop by five stops.
use crate::color::{curve::CurveLut, srgb_decode, srgb_encode};

/// The encoding's slope above white, in encoded units per stop at white.
const SLOPE: f32 = 0.19207;
/// How fast that slope falls with each stop above white.
const BEND: f32 = 0.08544;
/// The brightest value a curve that ends at white renders: four stops above white.
pub const PEAK: f32 = 16.;

/// A value (linear, 1 = graphics white) in the encoding a look's curve works in, in
/// HDR: sRGB's up to white, logarithmic above. Negative values encode as 0.
pub fn encode(x: f32) -> f32 {
    if x <= 1. {
        srgb_encode(x.max(0.))
    } else {
        1. + SLOPE / BEND * (1. + BEND * x.log2()).log2()
    }
}

/// The inverse of [`encode`].
pub fn decode(e: f32) -> f32 {
    if e <= 1. {
        srgb_decode(e.max(0.))
    } else {
        ((((e - 1.) * BEND / SLOPE).exp2() - 1.) / BEND).exp2()
    }
}

/// A look curve's HDR rendering, on [`encode`]d values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LookTone {
    /// A curve that ends at white: the curve up to `until`, then the input plus
    /// `lift`, at most [`PEAK`].
    Lifted { until: f32, lift: f32 },
    /// A curve that ends below white: the curve up to white, then along `slope`.
    Extended { slope: f32 },
}

impl LookTone {
    /// The HDR rendering of a look's `curve` (sRGB-encoded, 0–1).
    pub fn of(curve: &CurveLut) -> Self {
        let v = curve.values();
        let last = v.len() - 1;
        if v[last] < 1. - 1e-4 {
            return Self::Extended {
                slope: (v[last] - v[last - 1]) * last as f32,
            };
        }
        // The first point of the largest lift; a lift within rounding of another
        // does not move it, so a curve that never lifts stays the identity.
        let (mut until, mut lift) = (0., 0.);
        for (i, y) in v.iter().enumerate() {
            let x = i as f32 / last as f32;
            if y - x > lift + 1e-6 {
                (until, lift) = (x, y - x);
            }
        }
        Self::Lifted { until, lift }
    }

    /// The encoded output for the encoded input `e`.
    pub fn evaluate(&self, curve: &CurveLut, e: f32) -> f32 {
        match *self {
            Self::Lifted { until, lift } => {
                if e <= until {
                    curve.evaluate_as_look(e)
                } else {
                    (e + lift).min(encode(PEAK))
                }
            }
            Self::Extended { slope } => {
                if e <= 1. {
                    curve.evaluate_as_look(e)
                } else {
                    curve.evaluate_as_look(1.) + slope * (e - 1.)
                }
            }
        }
    }

    /// Linear RGB (1 = graphics white, unbounded) through the curve, DNG RGBTone
    /// fashion on encoded values, as the SDR look curve applies.
    pub fn apply(&self, curve: &CurveLut, rgb: [f32; 3]) -> [f32; 3] {
        let p = rgb.map(encode);
        let lo = p.into_iter().fold(f32::INFINITY, f32::min);
        let hi = p.into_iter().fold(0., f32::max);
        let a = self.evaluate(curve, lo);
        let b = self.evaluate(curve, hi);
        if hi - lo > 1e-8 {
            p.map(|v| decode(a + (b - a) * (v - lo) / (hi - lo)))
        } else {
            [decode(b); 3]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::curve::ToneCurve;

    fn curve(points: &[[f32; 2]]) -> CurveLut {
        CurveLut::new(&ToneCurve {
            points: points.iter().map(|p| [p[0] / 255., p[1] / 255.]).collect(),
            ..Default::default()
        })
    }

    fn render(c: &CurveLut, x: f32) -> f32 {
        let tone = LookTone::of(c);
        tone.apply(c, [x; 3])[0]
    }

    #[test]
    fn the_encoding_is_srgb_below_white_and_inverts() {
        assert_eq!(encode(0.5), srgb_encode(0.5));
        assert!((encode(1.) - 1.).abs() < 1e-6);
        for i in 0..=200 {
            let x = (i as f32 / 200. * 14. - 8.).exp2();
            let back = decode(encode(x));
            assert!((back / x - 1.).abs() < 1e-4, "{x} {back}");
        }
        let mut last = encode(1.);
        for i in 1..=100 {
            let e = encode(1. + i as f32 * 0.5);
            assert!(e > last);
            last = e;
        }
    }

    /// Camera Raw 18.7's gray ramp through synthetic look curves, HDR limit 8: the
    /// input after the profile's curve, and the rendered value.
    #[test]
    fn look_curves_render_as_camera_raw_renders_them() {
        let s = curve(&[
            [0., 0.],
            [64., 44.],
            [128., 132.],
            [192., 214.],
            [255., 255.],
        ]);
        let top = curve(&[[0., 0.], [192., 192.], [255., 230.]]);
        let knee = curve(&[[0., 0.], [128., 128.], [255., 200.]]);
        let lift = curve(&[[0., 25.], [255., 255.]]);
        let clip = curve(&[[0., 0.], [230., 255.], [255., 255.]]);
        for (c, measured) in [
            (
                &s,
                [
                    (0.3056, 0.3715),
                    (0.9945, 1.2202),
                    (3.9196, 5.0906),
                    (11.984, 15.398),
                ],
            ),
            (
                &top,
                [
                    (0.3056, 0.3462),
                    (0.9945, 0.7942),
                    (3.9196, 1.5974),
                    (11.984, 2.8121),
                ],
            ),
            (
                &knee,
                [
                    (0.3056, 0.2795),
                    (0.9945, 0.5758),
                    (3.9196, 1.04),
                    (11.984, 1.5694),
                ],
            ),
            (
                &lift,
                [
                    (0.3056, 0.4294),
                    (0.9945, 1.2549),
                    (3.9196, 5.2704),
                    (11.984, 15.586),
                ],
            ),
            (
                &clip,
                [
                    (0.3056, 0.6419),
                    (0.9945, 1.7799),
                    (3.9196, 7.9957),
                    (11.984, 15.977),
                ],
            ),
        ] {
            for (x, expected) in measured {
                let got = render(c, x);
                assert!(
                    (got / expected).log2().abs() < 0.07,
                    "{x}: {got} against {expected}"
                );
            }
        }
    }

    /// A colour's brightest and darkest channels go through the curve; the middle one
    /// keeps its encoded place between them, so the hue holds above white.
    #[test]
    fn colours_take_the_curve_rgbtone_fashion() {
        let s = curve(&[
            [0., 0.],
            [64., 44.],
            [128., 132.],
            [192., 214.],
            [255., 255.],
        ]);
        let tone = LookTone::of(&s);
        let rgb = [3., 1.2, 0.2];
        let out = tone.apply(&s, rgb);
        let e = rgb.map(encode);
        let o = out.map(encode);
        assert!((o[0] - tone.evaluate(&s, e[0])).abs() < 1e-5);
        assert!((o[2] - tone.evaluate(&s, e[2])).abs() < 1e-5);
        let place = |v: [f32; 3]| (v[1] - v[2]) / (v[0] - v[2]);
        assert!((place(o) - place(e)).abs() < 1e-4);
    }

    #[test]
    fn a_curve_that_never_lifts_renders_as_the_identity() {
        let dark = curve(&[[0., 0.], [128., 100.], [255., 255.]]);
        assert_eq!(
            LookTone::of(&dark),
            LookTone::Lifted {
                until: 0.,
                lift: 0.
            }
        );
        for x in [0.05, 0.3, 0.98, 4.06, 11.8] {
            assert!((render(&dark, x) / x - 1.).abs() < 1e-4, "{x}");
        }
        let identity = curve(&[[0., 0.], [255., 255.]]);
        assert!((render(&identity, 2.5) / 2.5 - 1.).abs() < 1e-4);
    }

    #[test]
    fn a_curve_that_ends_at_white_stops_four_stops_above_it() {
        let s = curve(&[
            [0., 0.],
            [64., 44.],
            [128., 132.],
            [192., 214.],
            [255., 255.],
        ]);
        for x in [14., 16., 40., 1e4] {
            assert!((render(&s, x) - PEAK).abs() < 1e-3, "{x}");
        }
        assert!(render(&s, f32::INFINITY).is_finite());
        assert_eq!(render(&s, -1.), 0.);
    }
}
