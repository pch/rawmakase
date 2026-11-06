//! DNG 1.7's overrange encoding, which HDR camera profiles (`ProfileDynamicRange` 1)
//! use for the value axis of their tables and for their tone curve. The formulas are
//! those of Adobe DNG SDK 1.7.1 (`dng_utils.h`, `EncodeOverrange`/`DecodeOverrange`;
//! licence in licenses/Adobe-DNG-SDK.txt).
//!
//! The encoding takes linear values from 0 (graphics black) through 1 (graphics
//! white, encoded as about 0.502) to 16 (four stops above white, encoded as 1). Larger
//! values encode above 1; the tables and the curve clamp or extend them as their
//! callers say.

/// Linear value to overrange encoding. Negative values and NaN encode as 0.
pub fn encode(x: f32) -> f32 {
    let x = x.max(0.);
    x * (256. + x) / (256. * (1. + x))
}

/// Overrange encoding back to linear: the inverse of [`encode`] for every value it
/// returns. Negative values and NaN decode as 0.
pub fn decode(y: f32) -> f32 {
    let y = y.max(0.);
    // The SDK's 16 (8y − 8 + √D), with D = 64y² − 127y + 64, rationalised: D − (8 − 8y)²
    // is y, so this is the same value without the cancellation near black.
    16. * y / ((64. * y * y - 127. * y + 64.).sqrt() + 8. - 8. * y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_and_four_stops_above_it_are_the_encodings_anchors() {
        assert_eq!(encode(0.), 0.);
        assert!((encode(1.) - 257. / 512.).abs() < 1e-7);
        assert!((encode(16.) - 1.).abs() < 1e-6);
        assert!((decode(1.) - 16.).abs() < 1e-4);
        assert_eq!(encode(-1.), 0.);
        assert_eq!(encode(f32::NAN), 0.);
        assert_eq!(decode(f32::NAN), 0.);
    }

    #[test]
    fn decoding_inverts_encoding_over_the_hdr_range() {
        for i in 0..=400 {
            let x = (i as f32 / 400. * 12. - 8.).exp2();
            let back = decode(encode(x));
            assert!((back - x).abs() <= 2e-4 * x.max(1e-3), "{x} {back}");
        }
        let mut last = 0.;
        for i in 1..=1000 {
            let y = encode(i as f32 * 0.05);
            assert!(y > last);
            last = y;
        }
    }
}
