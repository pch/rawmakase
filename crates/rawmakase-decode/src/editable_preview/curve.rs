//! Version 1's pixel curve (curve 1): linear camera values to 16-bit codes and back.
//!
//! `code = round(65535 · log2(1 + v / TOE) / log2(1 + TOP / TOE))`, with
//! `TOE` = 2⁻¹² and `TOP` = 2¹² on the scale where 1.0 is the sensor's white (white
//! balanced, as `raw::Raw::develop` makes it) or a rendered file's white.
//!
//! - Above the toe it is logarithmic: 24 stops share the codes, about 2,731 a
//!   stop, so a value reads back within 0.013% of itself (half a step,
//!   `2^(1/2731) − 1` over 2) at every level from deep shadow to `TOP`.
//! - Below the toe it turns linear and reaches 0 exactly: the step there is
//!   about 6 × 10⁻⁸, far finer than one level of a 16-bit sensor.
//! - `TOP` is 12 stops above white, headroom for recovered highlights, white
//!   balance multipliers and HDR edits. Larger values are stored as `TOP`;
//!   negative values, NaN and −∞ as 0. Decoders make neither.
//!
//! These constants are part of the format: a file written with curve 1 reads
//! with them forever. Another curve is another curve number.

/// Where the curve turns from linear to logarithmic.
pub(super) const TOE: f64 = 1. / 4096.;
/// The largest value stored; code 65535.
pub(super) const TOP: f64 = 4096.;
const MAX_CODE: f64 = 65535.;

fn stops() -> f64 {
    (1. + TOP / TOE).log2()
}

/// The code of `v`.
pub(super) fn encode(v: f32) -> u16 {
    let v = f64::from(v);
    // NaN as well as values at or below zero.
    if v.is_nan() || v <= 0. {
        return 0;
    }
    let v = v.min(TOP);
    ((1. + v / TOE).log2() / stops() * MAX_CODE).round() as u16
}

/// The value of `code`, as a 65536-entry table.
pub(super) fn table() -> Vec<f32> {
    let stops = stops();
    (0..=u16::MAX)
        .map(|c| (TOE * ((f64::from(c) / MAX_CODE * stops).exp2() - 1.)) as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code reads back as a value that encodes to it again, so writing an
    /// editable preview read from a file changes nothing.
    #[test]
    fn every_code_round_trips() {
        let table = table();
        assert_eq!(table[0], 0.);
        assert!((f64::from(table[65535]) - TOP).abs() < 1e-3);
        for (c, v) in table.iter().enumerate() {
            assert_eq!(encode(*v) as usize, c, "{c}: {v}");
        }
        assert!(table.windows(2).all(|w| w[0] < w[1]));
    }

    /// Within 0.013% of the value above the toe, and to a few 10⁻⁸ below it, from 2⁻²⁴
    /// to 12 stops above white; out-of-range values clamp.
    #[test]
    fn values_keep_their_precision_and_headroom() {
        let table = table();
        let mut v = 2f64.powi(-24);
        while v <= TOP {
            let back = f64::from(table[encode(v as f32) as usize]);
            let error = (back - v).abs();
            assert!(error <= 1.3e-4 * v + 4e-8, "{v}: {back}");
            v *= 1.01;
        }
        for v in [2., 16., 1000., 4000.] {
            let back = table[encode(v) as usize];
            assert!((back / v - 1.).abs() < 1.3e-4, "{v}: {back}");
        }
        assert_eq!(encode(-1.), 0);
        assert_eq!(encode(f32::NAN), 0);
        assert_eq!(encode(f32::NEG_INFINITY), 0);
        assert_eq!(encode(f32::INFINITY), 65535);
        assert_eq!(encode(1e9), 65535);
    }
}
