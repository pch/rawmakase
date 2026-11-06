//! How a camera profile's tone curve renders in HDR, as Camera Raw 18.7 renders it
//! (docs/records/hdr-stage1-results.md#ordinary-profile-curves).
//!
//! An HDR profile (DNG 1.7 `ProfileDynamicRange` 1) is the DNG SDK's rule: its curve
//! maps [`super::overrange`]-encoded values, extended above the encoded 1 along its
//! slope there ([`HdrTone::Overrange`]).
//!
//! An ordinary profile's curve is defined on 0–1 only, and Camera Raw extends it in
//! HDR by a rule of its own that is neither the SDK's slope extension nor the overrange
//! rule. It was measured on gray ramps rendered through 58 camera-matching profile
//! curves and the ACR3 default curve, all at HDR limit 8:
//!
//! - A curve that never rises above the identity (a linear curve, gammas above 1)
//!   renders as the identity: the scene value itself.
//! - The ACR3 default curve, which Adobe Standard and every profile without its own
//!   curve use, renders as [`super::hdr_tone_data::DEFAULT_HDR`]: the curve below about 0.15,
//!   and above it a smooth extension that reaches 2.7 at scene 1 and 21 at scene 4.
//! - Any other curve renders as that measured curve times a gain: a linear function of
//!   the curve's log2 ratio to ACR3 at twelve inputs and at the pixel's own input,
//!   whose weights vary with the scene value. Fitted on the 58 curves; left out one at
//!   a time, the median error above graphics white is 0.05 stop or less for 52 of them
//!   and the worst sample 0.27 stop.
//!
//! A curve unlike those it was fitted on (its twelve ratios more than
//! [`MAX_DISTANCE`] from them by Mahalanobis distance) is refused rather than guessed:
//! gamma curves without a toe, for example, miss by 0.15–0.36 stop. Above the measured
//! range (scene 13.5) the curve continues along its last log–log slope; Camera Raw's
//! PQ output cannot show values there.
use super::hdr_tone_data::{
    DEFAULT_HDR, FEATURES, GRID, GRID_FIRST, GRID_STEP, MEAN, PRECISION, WEIGHTS,
};
use anyhow::{Result, ensure};

/// The furthest an ordinary curve's twelve log2 ratios to ACR3 may be from the fitted
/// curves' (Mahalanobis distance). The fitted curves are within 4.2; the synthetic
/// gamma and steep filmic curves that the fit misses are at 4.3 and above.
pub const MAX_DISTANCE: f32 = 4.;

/// A profile tone curve's HDR rendering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HdrTone {
    /// The DNG SDK's overrange rule, for HDR profiles.
    Overrange,
    /// A curve that never brightens: the scene value.
    Identity,
    /// An ordinary curve: the measured ACR3 extension with the fitted deviation.
    Extended {
        /// log2 of the curve over ACR3 at [`FEATURES`].
        ratios: [f32; 12],
    },
}

impl HdrTone {
    /// The HDR rendering of an ordinary profile's `tone` points. Fails for a curve
    /// outside the measured family.
    pub fn ordinary(tone: &[[f32; 2]]) -> Result<Self> {
        let curve = |x: f32| super::evaluate_tone(tone, x);
        if (1..=512).all(|i| {
            let x = i as f32 / 512.;
            curve(x) <= x * (1. + 1e-4)
        }) {
            return Ok(Self::Identity);
        }
        let ratios = FEATURES.map(|y| ratio(curve(y), y));
        let delta: [f32; 12] = std::array::from_fn(|i| ratios[i] - MEAN[i]);
        let distance = (0..12)
            .map(|i| {
                (0..12)
                    .map(|j| delta[i] * PRECISION[i][j] * delta[j])
                    .sum::<f32>()
            })
            .sum::<f32>()
            .max(0.)
            .sqrt();
        ensure!(
            distance <= MAX_DISTANCE,
            "This profile's tone curve is unlike the curves HDR rendering was measured with"
        );
        Ok(Self::Extended { ratios })
    }

    /// The rendered value of scene value `x` (linear, 1 = graphics white) through the
    /// curve `tone`, unbounded above.
    pub fn evaluate(&self, tone: &[[f32; 2]], x: f32) -> f32 {
        if x.is_nan() || x <= 0. {
            return 0.;
        }
        match self {
            Self::Overrange => {
                let e = super::overrange::encode(x);
                let y = if e <= 1. {
                    super::evaluate_tone(tone, e)
                } else {
                    // The SDK's slope extension, measured over the last 0.001.
                    let top = super::evaluate_tone(tone, 1.);
                    top + (e - 1.) * (top - super::evaluate_tone(tone, 0.999)) / 0.001
                };
                super::overrange::decode(y)
            }
            Self::Identity => x,
            Self::Extended { ratios } => {
                let own = ratio(super::evaluate_tone(tone, x.min(1.)), x.min(1.));
                let log = x.log2();
                let first = GRID_FIRST;
                let last = GRID_FIRST + (GRID - 1) as f32 * GRID_STEP;
                if log < first {
                    // Below the measured range the rendering is the curve itself, scaled
                    // to meet the fit at the range's start.
                    let at = first.exp2();
                    let start = ratio(super::evaluate_tone(tone, at), at);
                    let scale =
                        value_at(0, ratios, start) / super::evaluate_tone(tone, at).max(1e-6);
                    return super::evaluate_tone(tone, x) * scale;
                }
                if log > last {
                    // Along the last measured log–log slope.
                    let a = value_at(GRID - 2, ratios, own);
                    let b = value_at(GRID - 1, ratios, own);
                    return b * (b / a).powf((log - last) / GRID_STEP);
                }
                let f = (log - first) / GRID_STEP;
                let i = (f as usize).min(GRID - 2);
                let t = f - i as f32;
                let a = value_at(i, ratios, own).ln();
                let b = value_at(i + 1, ratios, own).ln();
                (a + (b - a) * t).exp()
            }
        }
    }
}

/// The rendering at grid point `i` of a curve with `ratios`, whose own log2 ratio to
/// ACR3 at the pixel's input is `own`: the measured ACR3 rendering times the fitted
/// gain.
fn value_at(i: usize, ratios: &[f32; 12], own: f32) -> f32 {
    let w = &WEIGHTS[i];
    let gain = (0..12).map(|k| w[k] * ratios[k]).sum::<f32>() + w[12] * own;
    DEFAULT_HDR[i] * gain.exp2()
}

/// log2 of curve value `y` over ACR3 at `x`, as the fit measured it (both floored).
fn ratio(y: f32, x: f32) -> f32 {
    (y.max(1e-6) / super::default_tone_at(x).max(1e-6)).log2()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(f: impl Fn(f32) -> f32) -> Vec<[f32; 2]> {
        (0..=256)
            .map(|i| {
                let x = i as f32 / 256.;
                [x, f(x)]
            })
            .collect()
    }

    #[test]
    fn the_default_curve_renders_as_measured() {
        let tone = HdrTone::ordinary(&super::super::default_tone()).unwrap();
        assert_eq!(tone, HdrTone::Extended { ratios: [0.; 12] });
        let tone_points = super::super::default_tone();
        // Camera Raw 18.7's gray ramp through the ACR3 curve at HDR limit 8.
        for (x, expected) in [
            (0.0539, 0.086),
            (0.4789, 1.004),
            (1.2006, 3.571),
            (4.064, 21.277),
        ] {
            let got = tone.evaluate(&tone_points, x);
            assert!(
                (got / expected).log2().abs() < 0.03,
                "{x}: {got} {expected}"
            );
        }
        // Below white it is the SDR curve.
        let low = tone.evaluate(&tone_points, 0.02);
        assert!(
            (low / super::super::default_tone_at(0.02) - 1.).abs() < 0.02,
            "{low}"
        );
    }

    #[test]
    fn curves_that_never_brighten_render_the_scene() {
        for tone in [points(|x| x), points(|x| x.powf(1.25)), points(|x| x * x)] {
            let hdr = HdrTone::ordinary(&tone).unwrap();
            assert_eq!(hdr, HdrTone::Identity);
            assert_eq!(hdr.evaluate(&tone, 3.5), 3.5);
        }
    }

    #[test]
    fn curves_unlike_the_measured_family_are_refused() {
        // Camera Raw renders x^0.8 a quarter of a stop away from what the fit predicts.
        assert!(HdrTone::ordinary(&points(|x| x.powf(0.8))).is_err());
        let steep = |x: f32| (1. - (-6. * x.powf(0.8)).exp()) / (1. - (-6f32).exp());
        assert!(HdrTone::ordinary(&points(steep)).is_err());
    }

    #[test]
    fn rendering_is_finite_monotonic_and_unbounded() {
        let tone_points = super::super::default_tone();
        let tone = HdrTone::ordinary(&tone_points).unwrap();
        let mut last = 0.;
        for i in -200..=80 {
            let x = (i as f32 / 10.).exp2();
            let y = tone.evaluate(&tone_points, x);
            assert!(y.is_finite() && y >= last, "{x}: {y} after {last}");
            last = y;
        }
        assert!(last > 100.);
        assert_eq!(tone.evaluate(&tone_points, f32::NAN), 0.);
        assert_eq!(tone.evaluate(&tone_points, -1.), 0.);
    }

    #[test]
    fn hdr_profiles_follow_the_sdk_overrange_rule() {
        let linear = vec![[0., 0.], [1., 1.]];
        let tone = HdrTone::Overrange;
        for x in [0.01, 0.5, 1., 4., 16.] {
            assert!((tone.evaluate(&linear, x) / x - 1.).abs() < 1e-3, "{x}");
        }
        // Above 16 (encoded 1) the slope extension continues the identity.
        assert!((tone.evaluate(&linear, 40.) / 40. - 1.).abs() < 0.01);
    }
}
