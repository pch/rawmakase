//! Whites in an HDR render, measured on Camera Raw 18.7's renders of a gray ramp
//! through a linear profile at HDR limit 8: a gain over the scene value that is the
//! same at sensor whites of 4, 16 and 64 and at limits 2.3 and 8 (below the limit).
//! Whites −100 darkens scene 1 by 0.7 stop and scene 10 by 1.16; +100 brightens them
//! by 1.4 and 1.9. Between the measured positions (−100, −50, −25, +25, +50, +100)
//! the gain is interpolated; beyond the ramp (scene 12) it stays at its last value.
//!
//! Positive Whites renders this gain only on photos whose bright end is at least
//! twice graphics white: on darker ones Camera Raw stretches much further, so an HDR
//! edit with positive Whites is refused (`model::hdr::check_recipe`) until that is
//! measured.
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
mod data {
    include!("hdr_whites_data.rs");
}
use data::{WHITES, WHITES_FIRST, WHITES_STEP, WHITES_VALUES};

pub(crate) const SAMPLES: usize = WHITES[0].len();
/// Scene values from here up render as themselves in HDR at Whites 0; below, the
/// default black point's toe applies as in SDR.
pub(crate) const IDENTITY_FROM: f32 = 0.5;

/// The gains (log2) at Whites `whites` (−1 to 1), over log2 scene values.
pub(crate) fn gains(whites: f32) -> [f32; SAMPLES] {
    let whites = whites.clamp(-1., 1.);
    // The measured positions with 0 (no gain) between −0.25 and +0.25.
    let positions = [-1., -0.5, -0.25, 0., 0.25, 0.5, 1.];
    let row = |i: usize, k: usize| match i {
        0..=2 => WHITES[i][k],
        3 => 0.,
        _ => WHITES[i - 1][k],
    };
    debug_assert_eq!(WHITES_VALUES.len() + 1, positions.len());
    let i = positions
        .windows(2)
        .position(|w| whites <= w[1])
        .unwrap_or(positions.len() - 2);
    let t = (whites - positions[i]) / (positions[i + 1] - positions[i]);
    std::array::from_fn(|k| row(i, k) * (1. - t) + row(i + 1, k) * t)
}

/// The gain (log2) of `gains` at scene value `x`.
pub(crate) fn gain(gains: &[f32; SAMPLES], x: f32) -> f32 {
    if x.is_nan() || x <= 0. {
        return gains[0];
    }
    let f = ((x.log2() - WHITES_FIRST) / WHITES_STEP).clamp(0., (SAMPLES - 1) as f32);
    let i = (f as usize).min(SAMPLES - 2);
    let t = f - i as f32;
    gains[i] * (1. - t) + gains[i + 1] * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whites_zero_is_no_gain_and_the_extremes_are_as_measured() {
        assert!(gains(0.).iter().all(|g| *g == 0.));
        // Camera Raw 18.7, gray ramp, HDR limit 8: log2 gain at scene 1.2006.
        assert!((gain(&gains(-1.), 1.2006) + 0.712).abs() < 0.01);
        assert!((gain(&gains(1.), 1.2006) - 1.399).abs() < 0.01);
        assert!((gain(&gains(0.5), 1.2006) - 0.457).abs() < 0.01);
        // Interpolated between measured positions, and monotonic in Whites.
        let mut last = f32::NEG_INFINITY;
        for w in -20..=20 {
            let g = gain(&gains(w as f32 / 20.), 2.);
            assert!(g >= last, "{w}");
            last = g;
        }
    }
}
