//! Whites in an HDR render, measured on Camera Raw 18.7's renders of gray ramps through
//! a linear profile at HDR limit 8, as gains over the scene value.
//!
//! Negative Whites is the same gain at sensor whites of 4, 16 and 64 and at limits 2.3
//! and 8 (below the limit), whatever the photo: −100 darkens scene 1 by 0.7 stop and
//! scene 10 by 1.16. Between the measured positions (−100, −50, −25) the gain is
//! interpolated; beyond the ramp (scene 12) it stays at its last value.
//!
//! Positive Whites follows the photo's level ([`super::PhotoMeasures::hdr_whites_level`],
//! with Exposure): the lower it is, the further Whites stretches. From a level of 4 up the
//! gain no longer changes (+100 brightens scene 1 by 1.32 stops and scene 8 by 1.88); at
//! a level of 0.5 it brightens scene 0.3 by 3.9 stops. It does not follow the sensor's
//! white or the HDR limit. The table holds the measured positions +25, +50, +75 and +100
//! at levels 1/8 to 4 in quarter stops, interpolated between them and to no gain at 0;
//! outside those levels the nearest applies. Above the brightest scene value a level's
//! ramps measured (where Camera Raw's PQ output reaches 10,000 cd/m²) the gain stays at
//! its last value.
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
mod data {
    include!("hdr_whites_data.rs");
    include!("hdr_whites_positive_data.rs");
}
use data::{
    POSITIVE, POSITIVE_FIRST, POSITIVE_LEVEL_FIRST, POSITIVE_LEVEL_STEP, POSITIVE_LEVELS,
    POSITIVE_SAMPLES, POSITIVE_STEP, POSITIVE_VALUES, WHITES, WHITES_FIRST, WHITES_STEP,
    WHITES_VALUES,
};

/// The gains' samples, over log2 scene values from [`FIRST`] in steps of [`STEP`]: the
/// positive table's grid, which extends the negative one's.
pub(crate) const SAMPLES: usize = POSITIVE_SAMPLES;
const FIRST: f32 = POSITIVE_FIRST;
const STEP: f32 = POSITIVE_STEP;
const _: () = assert!(WHITES_FIRST == POSITIVE_FIRST && WHITES_STEP == POSITIVE_STEP);
/// The gains' grid over log2 scene value: the first sample and the step, for the GPU
/// port, which reads [`gain`]'s samples as they are.
pub(crate) const GRID: [f32; 2] = [FIRST, STEP];
/// Scene values from here up render as themselves in HDR at Whites 0; below, the
/// default black point's toe applies as in SDR.
pub(crate) const IDENTITY_FROM: f32 = 0.5;

/// The gains (log2) at Whites `whites` (−1 to 1) on a photo whose level is `level`
/// (log2, at the render's Exposure; only positive Whites reads it), over log2 scene values.
pub(crate) fn gains(whites: f32, level: f32) -> [f32; SAMPLES] {
    let whites = whites.clamp(-1., 1.);
    if whites > 0. {
        return positive(whites, level);
    }
    // The measured positions with 0 (no gain) after −0.25.
    let positions = [-1., -0.5, -0.25, 0.];
    let row = |i: usize, k: usize| match i {
        0..=2 => WHITES[i][k.min(WHITES[i].len() - 1)],
        _ => 0.,
    };
    debug_assert_eq!(WHITES_VALUES[..], positions[..3]);
    let i = positions
        .windows(2)
        .position(|w| whites <= w[1])
        .unwrap_or(positions.len() - 2);
    let t = (whites - positions[i]) / (positions[i + 1] - positions[i]);
    std::array::from_fn(|k| row(i, k) * (1. - t) + row(i + 1, k) * t)
}

/// Positive Whites' gains at `whites` (0 to 1) and `level`.
fn positive(whites: f32, level: f32) -> [f32; SAMPLES] {
    let levels = POSITIVE_LEVELS;
    let f = ((level - POSITIVE_LEVEL_FIRST) / POSITIVE_LEVEL_STEP).clamp(0., (levels - 1) as f32);
    let l = (f as usize).min(levels - 2);
    let tl = f - l as f32;
    let at = |w: usize, k: usize| POSITIVE[w][l][k] * (1. - tl) + POSITIVE[w][l + 1][k] * tl;
    let i = POSITIVE_VALUES
        .iter()
        .position(|v| whites <= *v)
        .unwrap_or(POSITIVE_VALUES.len() - 1);
    let (low, t) = if i == 0 {
        (None, whites / POSITIVE_VALUES[0])
    } else {
        (
            Some(i - 1),
            (whites - POSITIVE_VALUES[i - 1]) / (POSITIVE_VALUES[i] - POSITIVE_VALUES[i - 1]),
        )
    };
    std::array::from_fn(|k| low.map_or(0., |w| at(w, k)) * (1. - t) + at(i, k) * t)
}

/// The gain (log2) of `gains` at scene value `x`.
pub(crate) fn gain(gains: &[f32; SAMPLES], x: f32) -> f32 {
    if x.is_nan() || x <= 0. {
        return gains[0];
    }
    let f = ((x.log2() - FIRST) / STEP).clamp(0., (SAMPLES - 1) as f32);
    let i = (f as usize).min(SAMPLES - 2);
    let t = f - i as f32;
    gains[i] * (1. - t) + gains[i + 1] * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whites_zero_is_no_gain_and_negative_whites_is_as_measured() {
        assert!(gains(0., 0.).iter().all(|g| *g == 0.));
        // Camera Raw 18.7, gray ramp, HDR limit 8: log2 gain at scene 1.2006, the same at
        // every level.
        for level in [-3., 0., 4.] {
            assert!((gain(&gains(-1., level), 1.2006) + 0.712).abs() < 0.01);
        }
    }

    /// Camera Raw 18.7, gray ramps through a linear profile at HDR limit 8: positive
    /// Whites stretches a darker photo much further, and stops changing from a level of
    /// 4 up. log2 gains over the Whites 0 render.
    #[test]
    fn positive_whites_follows_the_photo_level() {
        // A ramp peaking at 16 (level 4 and above): +100 at scene 1 and 8.
        assert!((gain(&gains(1., 4.), 1.) - 1.322).abs() < 0.01);
        assert!((gain(&gains(1., 4.), 8.) - 1.875).abs() < 0.01);
        assert!((gain(&gains(0.5, 4.), 1.) - 0.424).abs() < 0.01);
        assert_eq!(gains(1., 2.), gains(1., 6.));
        // A ramp peaking at 0.5 (level −1): +50 and +100 at scene 0.3.
        assert!((gain(&gains(0.5, -1.), 0.3) - 0.891).abs() < 0.01);
        assert!((gain(&gains(1., -1.), 0.3) - 3.885).abs() < 0.01);
        // A ramp peaking at 1, at +25.
        assert!((gain(&gains(0.25, 0.), 1.) - 0.30).abs() < 0.01);
        // Monotonic in Whites, and from no gain at 0.
        for level in [-3., -1.3, 0., 1.7, 4.] {
            let mut last = 0.;
            for w in 1..=20 {
                let g = gain(&gains(w as f32 / 20., level), 0.4);
                assert!(g >= last, "{level} {w}");
                last = g;
            }
            assert!(gain(&gains(0.01, level), 0.4) < 0.05);
        }
    }

    /// The output never decreases with the scene value, at any level and Whites.
    #[test]
    fn positive_whites_keeps_the_order_of_values() {
        for level in [-3., -2.1, -1., 0., 0.6, 2.] {
            for w in [0.1, 0.25, 0.6, 1.] {
                let g = gains(w, level);
                let mut last = 0f32;
                for k in 0..400 {
                    let x = (-14. + k as f32 * 0.05f32).exp2();
                    let y = x * gain(&g, x).exp2();
                    assert!(y >= last * 0.9999, "{level} {w} {x}");
                    last = y;
                }
            }
        }
    }
}
