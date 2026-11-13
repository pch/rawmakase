//! How the user's tone curves render in HDR, as Camera Raw 18.7 renders them
//! (docs/records/hdr-stage1-results.md#tone-curves).
//!
//! Both curves work on values in the encoding a look's curve works in
//! ([`encode`]: sRGB's up to graphics white, logarithmic above it), and both render
//! exactly as in SDR below white. They differ above it:
//!
//! - **The master point curve** keeps its own scale: it applies
//!   unchanged up to white and continues along its slope at its last point
//!   ([`ToneCurve::end_slope`], the spline's own, before clamping), so a curve that
//!   ends steeper than the identity brightens highlights and one that overshoots
//!   white and comes back to it darkens them.
//! - **The parametric curve** is laid over an axis from black to four stops above
//!   white ([`PARAMETRIC_TOP`]): its regions and splits divide that whole range, and
//!   values above it are left alone ([`parametric`]).
//!
//! Measured on gray ramps (sensor white +4 stops, a linear and the ACR3 profile curve,
//! HDR limit 8) through nine point curves and nine parametric settings. Up to four
//! stops above white, the point-curve rule renders each within 0.05 stop (median
//! 0.004–0.046). Camera Raw holds a curve's output back above 16 times white, toward
//! about 49 ([`hold`]): a curve through (128, 100) renders scene 11.8 as 25.8, where
//! its slope alone would reach 31.2; with the hold, 26.3 (0.027 stop). The
//! parametric rule renders each within 0.063 stop (median ≤ 0.016) below and above
//! white, at HDR limits 2, 4 and 8, and the parametric curve applies before the point
//! curve, as in SDR. Above four stops, Camera Raw's point curves drift toward the
//! input, as its look curves do.
//!
//! - **A channel's point curve** renders by the master curve's rule, after it, on that
//!   channel of encoded ProPhoto RGB, as in SDR; its output holds at 16 ([`channel`]).
//!   Measured on the gray ramp through a red curve that brightens and a blue curve that
//!   darkens: within 0.03 stop from white to scene 8, and blue through (128, 100) holds
//!   at 16.003 from scene 7.9. Above scene 8 Camera Raw's red drifts toward the input,
//!   as its master curves do (0.26 stop at scene 15).
pub use crate::camera_profiles::hdr_look::{decode, encode};
use crate::color::curve::{CurveLut, ToneCurve};

/// The value the parametric curve's axis ends at: four stops above graphics white.
pub const PARAMETRIC_TOP: f32 = crate::camera_profiles::hdr_look::PEAK;

/// The most a curve's extension renders: 16 stops above graphics white, eight above the
/// largest HDR limit's peak, whose roll-off takes it to that peak. A steep curve's
/// extension would otherwise decode to infinity, which sharpening and resizing turn
/// into NaN.
pub const CEILING: f32 = 65536.;

/// The encoded value Camera Raw 18.7 holds a steep point curve's HDR output toward:
/// 49.26 times white. Its output follows the curve's extension up to four stops above
/// white ([`PARAMETRIC_TOP`]'s encoding) and rolls off toward this above it, whatever the HDR
/// limit (curves through (128, 140) and (220, 255), (90, 110) and (180, 255), and a
/// profile curve, on gray ramps at limit 8; docs/records/hdr-stage1-results.md#tone-curves).
pub const HOLD: f32 = 2.2723;

/// A point curve's encoded HDR output `e` held toward [`HOLD`] from [`PARAMETRIC_TOP`]'s encoding
/// up: unchanged below it, then an exponential approach with the same slope there.
pub fn hold(e: f32) -> f32 {
    let knee = encode(PARAMETRIC_TOP);
    if e <= knee {
        return e;
    }
    let room = HOLD - knee;
    knee + room * (1. - (-(e - knee) / room).exp())
}

/// A point curve as it renders in HDR, on [`encode`]d values.
#[derive(Clone, Debug, PartialEq)]
pub struct HdrPointCurve {
    lut: CurveLut,
    white: f32,
    slope: f32,
    /// Whether its output is held above four stops over white ([`hold`]): a curve
    /// that isn't the identity.
    holds: bool,
}

impl HdrPointCurve {
    pub fn new(curve: &ToneCurve) -> Self {
        let slope = curve.end_slope();
        let lut = CurveLut::new(curve);
        Self {
            white: lut.evaluate(1.),
            slope,
            holds: !curve.is_identity(),
            lut,
        }
    }
    /// Whether its output is held above four stops over white ([`hold`]).
    pub fn holds(&self) -> bool {
        self.holds
    }
    /// The encoded output for the encoded input `e`: the curve up to white, its slope
    /// at white above it. The result can fall below 0, which [`decode`] reads as 0.
    /// The curve's value at white and its slope beyond, for the GPU port, which reads
    /// the curve's samples from its own copy of the same [`CurveLut`].
    pub fn extension(&self) -> [f32; 2] {
        [self.white, self.slope]
    }
    pub fn evaluate(&self, e: f32) -> f32 {
        let out = if e <= 1. {
            self.lut.evaluate(e)
        } else {
            self.white + self.slope * (e - 1.)
        };
        if self.holds { hold(out) } else { out }
    }
}

/// The linear value one channel's curve renders the encoded value `e` as: the curve's
/// HDR rendering ([`HdrPointCurve::evaluate`]), held at [`PARAMETRIC_TOP`] (16), as
/// Camera Raw holds a channel's output there, and not below 0.
pub fn channel(curve: &HdrPointCurve, e: f32) -> f32 {
    decode(curve.evaluate(e)).min(PARAMETRIC_TOP)
}

/// Refine Saturation on the master point curve in HDR, on [`encode`]d values: `input`
/// before the curve, `curved` after it. SDR's rule ([`crate::color::curve::refine_saturation`])
/// without its bound at white, which would take the colour out of light above white:
/// the colour keeps its channel differences from before the curve, scaled down only as
/// far as it must to stay above 0. Camera Raw 18.7 renders it so within the acceptance
/// rule at 50% on gray and colour patches and at 0% on six photos; at 0% on the patches
/// it adds more colour error than SDR does (docs/records/hdr-stage1-results.md#tone-curves).
pub fn refine_saturation(input: [f32; 3], curved: [f32; 3], amount: f32) -> [f32; 3] {
    crate::color::curve::refine_saturation_unbounded_above(input, curved, amount)
}

/// Levels (RAWmakase's black point, white point and midtone, which Camera Raw doesn't
/// have) on the encoded value `e` in HDR, rendered as Camera Raw renders a point curve
/// of the same shape ([`HdrPointCurve`]): as in SDR up to white, and along its slope
/// at white above it. That slope is 0 when the white point is below white, as a point
/// curve that reaches white early stays near it, and `1 / (midtone (1 − black))`
/// otherwise.
pub fn levels(e: f32, black: f32, white: f32, midtone: f32) -> f32 {
    let sdr = |e: f32| {
        ((e - black) / (white - black))
            .clamp(0., 1.)
            .powf(1. / midtone)
    };
    if e <= 1. {
        return sdr(e);
    }
    let slope = if white < 1. {
        0.
    } else {
        1. / (midtone * (1. - black))
    };
    sdr(1.) + slope * (e - 1.)
}

/// The parametric curve's HDR rendering of the encoded value `e`, given the curve
/// `curve` over 0–1 as SDR applies it: the curve laid over 0 to
/// [`encode`]`(`[`PARAMETRIC_TOP`]`)`, and `e` itself above that.
pub fn parametric(e: f32, curve: impl Fn(f32) -> f32) -> f32 {
    let top = encode(PARAMETRIC_TOP);
    if e >= top {
        e
    } else {
        curve(e.max(0.) / top) * top
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn natural(points: &[[f32; 2]]) -> ToneCurve {
        ToneCurve {
            points: points.iter().map(|p| [p[0] / 255., p[1] / 255.]).collect(),
            ..Default::default()
        }
    }

    fn render(curve: &ToneCurve, x: f32) -> f32 {
        decode(HdrPointCurve::new(curve).evaluate(encode(x)))
    }

    /// Refine Saturation in HDR keeps the colour's channel differences from before the
    /// curve, as in SDR, but not within white: a colour whose curved luma is near or
    /// above white keeps them in full, where SDR's would scale them down.
    #[test]
    fn hdr_refine_saturation_is_not_bounded_at_white() {
        let input = [1.3, 1.0, 0.8];
        let curved = [1.45, 1.15, 0.95];
        let kept = refine_saturation(input, curved, 0.);
        let luma = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
        assert!((luma(kept) - luma(curved)).abs() < 1e-5);
        for c in 0..3 {
            assert!(((kept[c] - kept[1]) - (input[c] - input[1])).abs() < 1e-5);
        }
        let sdr = crate::color::curve::refine_saturation(input, curved, 0.);
        assert!(sdr[0] - sdr[2] < kept[0] - kept[2]);
        assert_eq!(refine_saturation(input, curved, 1.), curved);
        // A dark colour is still scaled down to stay above 0.
        let dark = refine_saturation([0.3, 0.05, 0.01], [0.1, 0.02, 0.0], 0.);
        assert!(dark.iter().all(|v| *v >= -1e-6), "{dark:?}");
    }

    /// Camera Raw 18.7's gray ramp through point curves, HDR limit 8, a linear
    /// profile: the input, and the rendered value.
    #[test]
    fn point_curves_render_as_camera_raw_renders_them() {
        // Each curve's points, and (input, rendered) pairs.
        type Case<'a> = (&'a [[f32; 2]], [(f32, f32); 4]);
        let cases: [Case; 5] = [
            (
                &[
                    [0., 0.],
                    [64., 44.],
                    [128., 132.],
                    [192., 214.],
                    [255., 255.],
                ],
                [
                    (0.3013, 0.3648),
                    (0.9798, 0.9899),
                    (3.8628, 1.8858),
                    (11.8173, 3.16),
                ],
            ),
            (
                &[[0., 0.], [128., 100.], [255., 255.]],
                [
                    (0.3013, 0.1957),
                    (0.9798, 0.9731),
                    (3.8628, 6.3877),
                    (11.8173, 25.7986),
                ],
            ),
            (
                &[[0., 0.], [230., 255.], [255., 255.]],
                [
                    (0.3013, 0.635),
                    (0.9798, 1.0012),
                    (3.8628, 0.9391),
                    (11.8173, 0.8965),
                ],
            ),
            (
                &[[0., 25.], [255., 255.]],
                [
                    (0.3013, 0.3493),
                    (0.9798, 0.9817),
                    (3.8628, 3.3385),
                    (11.8173, 9.0562),
                ],
            ),
            (
                &[[40., 0.], [255., 255.]],
                [
                    (0.3013, 0.2213),
                    (0.9798, 0.976),
                    (3.8628, 5.1197),
                    (11.8173, 19.3676),
                ],
            ),
        ];
        for (points, measured) in cases {
            let curve = natural(points);
            for (x, want) in measured {
                let got = render(&curve, x);
                let stops = (got / want).log2().abs();
                // Camera Raw holds back outputs far above 16 ([`hold`]): the curve
                // that ends steepest renders 11.8 as 25.8, the slope alone 31.2.
                assert!(stops < 0.06, "{points:?} at {x}: {got} against {want}");
            }
        }
    }

    /// Camera Raw 18.7's gray ramp through one channel's point curve, HDR limit 8, a
    /// linear profile: the scene value, and that ProPhoto channel as rendered. The
    /// other channels stay the scene's.
    #[test]
    fn channel_curves_render_as_camera_raw_renders_them() {
        type Case<'a> = (&'a [[f32; 2]], [(f32, f32); 6]);
        let cases: [Case; 2] = [
            (
                &[[0., 0.], [128., 160.], [255., 255.]],
                [
                    (0.3041, 0.4558),
                    (0.9785, 0.9874),
                    (1.9928, 1.515),
                    (3.8574, 2.2305),
                    (7.8561, 3.388),
                    (11.7959, 4.5863),
                ],
            ),
            (
                &[[0., 0.], [128., 100.], [255., 255.]],
                [
                    (0.3041, 0.196),
                    (0.9785, 0.9752),
                    (1.9928, 2.5746),
                    (3.8574, 6.4009),
                    (7.8561, 16.0031),
                    (11.7959, 16.0029),
                ],
            ),
        ];
        for (points, measured) in cases {
            let curve = HdrPointCurve::new(&natural(points));
            for (x, want) in measured {
                let got = channel(&curve, encode(x));
                let stops = (got / want).log2().abs();
                // Far above white Camera Raw drifts toward the input, as for the master
                // curve: red at 11.8 departs by 0.09 stop.
                let allowed = if x > 10. { 0.28 } else { 0.03 };
                assert!(stops < allowed, "{points:?} at {x}: {got} against {want}");
            }
        }
    }

    /// Below white the point curve renders as in SDR, on sRGB-encoded values.
    #[test]
    fn below_white_a_point_curve_is_its_sdr_curve() {
        let curve = natural(&[[0., 0.], [128., 160.], [255., 255.]]);
        let lut = CurveLut::new(&curve);
        let hdr = HdrPointCurve::new(&curve);
        for i in 0..=100 {
            let e = i as f32 / 100.;
            assert_eq!(hdr.evaluate(e), lut.evaluate(e));
        }
        // And it continues from its value at white without a step.
        assert!((hdr.evaluate(1.0001) - lut.evaluate(1.)).abs() < 1e-3);
    }

    /// The end slope is the spline's own derivative at its last point, not the clamped
    /// table's: a curve that overshoots white and comes back slopes down.
    #[test]
    fn the_end_slope_is_the_splines_unclamped_derivative() {
        let dark = natural(&[[0., 0.], [128., 100.], [255., 255.]]);
        assert!(
            (dark.end_slope() - 1.33).abs() < 0.01,
            "{}",
            dark.end_slope()
        );
        let clip = natural(&[[0., 0.], [230., 255.], [255., 255.]]);
        assert!(
            (clip.end_slope() + 0.054).abs() < 0.002,
            "{}",
            clip.end_slope()
        );
        assert_eq!(ToneCurve::default().end_slope(), 1.);
        let linear = ToneCurve {
            points: vec![[0., 0.], [0.5, 0.25], [1., 1.]],
            smooth: false,
            natural: false,
        };
        assert_eq!(linear.end_slope(), 1.5);
    }

    /// Levels renders as in SDR below white and continues along its slope at white
    /// without a step; with its white point below white it holds white above it.
    #[test]
    fn levels_continue_along_their_slope_at_white() {
        let sdr = |e: f32, b: f32, w: f32, m: f32| ((e - b) / (w - b)).clamp(0., 1.).powf(1. / m);
        for (b, w, m) in [(0., 1., 1.), (0.1, 1., 1.), (0., 1., 1.5), (0.05, 0.9, 0.8)] {
            for i in 0..=100 {
                let e = i as f32 / 100.;
                assert_eq!(levels(e, b, w, m), sdr(e, b, w, m));
            }
            assert!((levels(1.0001, b, w, m) - levels(1., b, w, m)).abs() < 1e-3);
        }
        assert_eq!(levels(1.5, 0., 1., 1.), 1.5);
        // Black 0.1, midtone 1.5: slope 1 / (1.5 × 0.9) above white.
        assert!((levels(2., 0.1, 1., 1.5) - (1. + 1. / 1.35)).abs() < 1e-5);
        assert_eq!(levels(1.8, 0.05, 0.9, 0.8), 1.);
    }

    #[test]
    fn the_parametric_axis_spans_four_stops_above_white() {
        let identity = |x: f32| x;
        for x in [0., 0.01, 0.5, 1., 4., 15.9, 16., 40.] {
            let e = encode(x);
            assert!((parametric(e, identity) - e).abs() < 1e-5);
        }
        // A curve that doubles its input near black reaches further up the encoding
        // than in SDR, and nothing above the top moves.
        let top = encode(PARAMETRIC_TOP);
        assert!((parametric(top * 0.25, |x| x * 2.) - top * 0.5).abs() < 1e-5);
        assert_eq!(parametric(encode(20.), |_| 0.), encode(20.));
    }
}
