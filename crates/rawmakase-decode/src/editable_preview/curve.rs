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
//!
//! The Smaller choice (codec 2) keeps curve 1 with 12-bit codes: 16-bit code
//! `16 q + 8`, the middle of the 16 codes 12-bit code `q` stands for, about 171
//! a stop, so a step of 0.41% ([`widen`]). Its codes are dithered when built
//! ([`dithered`]): triangular noise of up to one 12-bit step, the same for every
//! photo at a given pixel and channel, so flat areas average to their value
//! rather than to the nearest step, and smooth gradients don't break into
//! contours under strong edits. Black reads back as about 5 × 10⁻⁷, not 0.

/// Where the curve turns from linear to logarithmic.
pub(super) const TOE: f64 = 1. / 4096.;
/// The largest value stored; code 65535.
pub(super) const TOP: f64 = 4096.;
const MAX_CODE: f64 = 65535.;

fn stops() -> f64 {
    (1. + TOP / TOE).log2()
}

/// Where `v` falls on the 16-bit code scale, unrounded: 0 to 65535.
pub(super) fn position(v: f32) -> f64 {
    let v = f64::from(v);
    // NaN as well as values at or below zero.
    if v.is_nan() || v <= 0. {
        return 0.;
    }
    let v = v.min(TOP);
    (1. + v / TOE).log2() / stops() * MAX_CODE
}

/// The code of `v`.
pub(super) fn encode(v: f32) -> u16 {
    position(v).round() as u16
}

/// The 16-bit code a 12-bit code `q` reads as: the middle of its 16 codes.
pub(super) fn widen(q: u16) -> u16 {
    debug_assert!(q < 4096);
    q << 4 | 8
}

/// The 12-bit code of `v`, the `channel` of sample `index` (pixels in row order),
/// dithered: rounded after adding triangular noise of −1 to 1 steps, which
/// [`noise`] draws from the position alone.
pub(super) fn dithered(v: f32, index: usize, channel: usize) -> u16 {
    // On the 12-bit scale, where code q's bucket is centred on q.
    let y = position(v) / 16. - 0.5;
    (y + noise(index as u64 * 3 + channel as u64))
        .round()
        .clamp(0., 4095.) as u16
}

/// Triangular noise on (−1, 1) for sample `n`: the sum of two uniform draws from
/// SplitMix64 of `n`, a fixed function so that builds are reproducible.
fn noise(n: u64) -> f64 {
    let mut z = n.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    let unit = |bits: u64| bits as f64 / f64::from(u32::MAX);
    unit(z >> 32) + unit(z & 0xffff_ffff) - 1.
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

    /// The dither's noise is triangular on (−1, 1), centred on 0; its values are
    /// part of what a build makes, so a few are pinned.
    #[test]
    fn the_dither_noise_is_triangular_and_fixed() {
        let n = 200_000;
        let draws: Vec<f64> = (0..n).map(noise).collect();
        assert!(draws.iter().all(|v| (-1.0..=1.0).contains(v)));
        let mean = draws.iter().sum::<f64>() / n as f64;
        let variance = draws.iter().map(|v| v * v).sum::<f64>() / n as f64;
        assert!(mean.abs() < 0.005, "{mean}");
        // A triangular distribution on (−1, 1) has variance 1/6.
        assert!((variance - 1. / 6.).abs() < 0.005, "{variance}");
        let near = draws.iter().filter(|v| v.abs() < 0.1).count() as f64 / n as f64;
        assert!((near - 0.19).abs() < 0.01, "{near}");
        let pinned: Vec<String> = (0..3).map(|n| format!("{:.6}", noise(n))).collect();
        assert_eq!(pinned, PINNED_NOISE);
    }

    const PINNED_NOISE: [&str; 3] = ["0.364234", "0.101754", "-0.297126"];

    /// Every 12-bit code widens into its own bucket's middle and reads back as a
    /// value whose dithered code, at zero noise, is that code again.
    #[test]
    fn twelve_bit_codes_widen_to_their_buckets_middle() {
        let table = table();
        for q in 0..4096u16 {
            let wide = widen(q);
            assert_eq!(wide >> 4, q);
            assert_eq!(encode(table[wide as usize]), wide);
        }
        assert_eq!(widen(4095), 65528);
    }

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
