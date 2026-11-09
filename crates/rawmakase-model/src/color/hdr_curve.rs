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
//! 0.004–0.046), but for curves that end much steeper than the identity, whose
//! output Camera Raw holds back far above 16: a curve through (128, 100) renders
//! scene 11.8 as 31.2 where Camera Raw renders 25.8 (0.27 stop). The
//! parametric rule renders each within 0.063 stop (median ≤ 0.016) below and above
//! white, at HDR limits 2, 4 and 8, and the parametric curve applies before the point
//! curve, as in SDR. Above four stops, Camera Raw's point curves drift toward the
//! input, as its look curves do.
//!
//! A single channel's point curve renders by neither rule above white (a red curve
//! that brightens takes red down to 0 by scene 12), so it is not rendered in HDR.
pub use crate::camera_profiles::hdr_look::{decode, encode};
use crate::color::curve::{CurveLut, ToneCurve};

/// The value the parametric curve's axis ends at: four stops above graphics white.
pub const PARAMETRIC_TOP: f32 = crate::camera_profiles::hdr_look::PEAK;

/// The most a curve's extension renders: 16 stops above graphics white, eight above the
/// largest HDR limit's peak, whose roll-off takes it to that peak. A steep curve's
/// extension would otherwise decode to infinity, which sharpening and resizing turn
/// into NaN.
pub const CEILING: f32 = 65536.;

/// A point curve as it renders in HDR, on [`encode`]d values.
#[derive(Clone, Debug, PartialEq)]
pub struct HdrPointCurve {
    lut: CurveLut,
    white: f32,
    slope: f32,
}

impl HdrPointCurve {
    pub fn new(curve: &ToneCurve) -> Self {
        let lut = CurveLut::new(curve);
        Self {
            white: lut.evaluate(1.),
            slope: curve.end_slope(),
            lut,
        }
    }
    /// The encoded output for the encoded input `e`: the curve up to white, its slope
    /// at white above it. The result can fall below 0, which [`decode`] reads as 0.
    pub fn evaluate(&self, e: f32) -> f32 {
        if e <= 1. {
            self.lut.evaluate(e)
        } else {
            self.white + self.slope * (e - 1.)
        }
    }
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
                // Camera Raw holds back outputs far above 16: the curve that ends
                // steepest departs by 0.27 stop at 11.8 (31.2 against 25.8);
                // everywhere else within 0.06.
                let allowed = if x > 10. { 0.28 } else { 0.06 };
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
