//! Detail > Sharpening, following Camera Raw 18.7, fitted to
//! renders of a synthetic chart (slanted edges at three contrasts, sine gratings from
//! 0.04 to 0.3 cycles per pixel, random texture and noisy flats) at Amount 25 to 150,
//! Radius 0.5 to 3, Detail 0 to 100 and Masking 10 to 100.
//!
//! An unsharp mask of the encoded luminance is added to every channel. Camera Raw's
//! holds back halos at strong edges rather than clipping them: the high-pass `d` is shaped as
//! `d / (1 + (|d| / halo)³)`, dark halos at 0.57 of light ones. Detail mostly scales
//! the strength (0.3× at 0, 2.1× at 100 against the default 25) and Masking leaves out
//! the weakest detail. Radius maps to a slightly different blur than its value.
use crate::model::recipe::Recipe;

/// Radius: the slider's values, the Gaussian sigma and a strength factor.
const RADIUS: [f32; 6] = [0.5, 0.8, 1., 1.5, 2., 3.];
const RADIUS_SIGMA: [f32; 6] = [0.705, 0.793, 1., 1.101, 1.342, 1.806];
const RADIUS_GAIN: [f32; 6] = [1.115, 1.214, 1., 1.111, 0.906, 0.608];
/// Detail (0–1): strength factor and halo scale.
const DETAIL: [f32; 5] = [0., 0.25, 0.5, 0.75, 1.];
const DETAIL_GAIN: [f32; 5] = [0.3179, 1., 1.538, 1.848, 2.084];
const DETAIL_HALO: [f32; 5] = [1.578, 1., 0.99, 0.986, 0.979];
/// Masking (0–1): the high-pass below which sharpening fades out.
const MASKING_THRESHOLD: [f32; 5] = [0., 0.012, 0.09, 0.138, 0.2];
/// Strength per Amount at Radius 1 and Detail 25 (Amount 1 is Lightroom's 150).
const MEASURED_GAIN: f32 = 4.872;
const MEASURED_HALO: f32 = 0.0863;
const MEASURED_DARK: f32 = 0.574;

fn interpolate(x: f32, xs: &[f32], ys: &[f32]) -> f32 {
    let i = xs.partition_point(|v| *v <= x).clamp(1, xs.len() - 1);
    let t = ((x - xs[i - 1]) / (xs[i] - xs[i - 1])).clamp(0., 1.);
    ys[i - 1] + (ys[i] - ys[i - 1]) * t
}

/// A recipe's sharpening parameters, for the CPU and the GPU shaders.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sharpener {
    /// Gaussian sigma of the blur at full resolution.
    pub(crate) sigma: f32,
    /// Strength per unit of Amount.
    pub(crate) gain: f32,
    /// High-pass below which sharpening fades out (0: none).
    pub(crate) threshold: f32,
    /// The halo shaping's scale.
    pub(crate) halo: f32,
    /// Strength of dark halos relative to light ones.
    pub(crate) dark: f32,
}
impl Sharpener {
    pub(crate) fn new(r: &Recipe) -> Self {
        Self {
            sigma: interpolate(r.sharpening_radius, &RADIUS, &RADIUS_SIGMA),
            gain: MEASURED_GAIN
                * interpolate(r.sharpening_radius, &RADIUS, &RADIUS_GAIN)
                * interpolate(r.sharpening_detail, &DETAIL, &DETAIL_GAIN),
            threshold: interpolate(r.sharpening_masking, &DETAIL, &MASKING_THRESHOLD),
            halo: MEASURED_HALO * interpolate(r.sharpening_detail, &DETAIL, &DETAIL_HALO),
            dark: MEASURED_DARK,
        }
    }
    /// The luminance change for high-pass `d` (luminance minus its blur) at `amount`
    /// (the recipe's Amount plus any mask's Sharpness). Below zero it blurs.
    pub(crate) fn delta(&self, d: f32, amount: f32) -> f32 {
        if amount < 0. {
            return -d * (-amount).min(1.);
        }
        let mask = if self.threshold == 0. {
            1.
        } else {
            (d.abs() / self.threshold).clamp(0., 1.)
        };
        let k = amount * self.gain * mask;
        let shaped = d / (1. + (d.abs() / self.halo).powi(3));
        k * if shaped < 0. {
            shaped * self.dark
        } else {
            shaped
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(amount: f32) -> Recipe {
        Recipe {
            sharpening: amount / 150.,
            sharpening_radius: 1.,
            sharpening_detail: 0.25,
            sharpening_masking: 0.,
            ..Recipe::default()
        }
    }

    #[test]
    fn strong_edges_get_smaller_halos_and_dark_halos_smaller_still() {
        let s = Sharpener::new(&measured(80.));
        let a = 80. / 150.;
        let small = s.delta(0.01, a) / 0.01;
        let large = s.delta(0.3, a) / 0.3;
        assert!(large < small * 0.1, "{small} {large}");
        assert!((s.delta(-0.01, a) / s.delta(0.01, a) + MEASURED_DARK).abs() < 1e-6);
        // Negative local Sharpness blurs.
        assert_eq!(s.delta(0.1, -0.5), -0.05);
    }

    #[test]
    fn detail_and_masking_shape_the_strength() {
        let detail = |v: f32| {
            Sharpener::new(&Recipe {
                sharpening_detail: v,
                ..measured(80.)
            })
            .gain
        };
        assert!(detail(0.) < detail(0.25) * 0.4 && detail(1.) > detail(0.25) * 2.);
        let masked = Sharpener::new(&Recipe {
            sharpening_masking: 1.,
            ..measured(80.)
        });
        assert_eq!(masked.delta(0., 0.5), 0.);
        assert!(masked.delta(0.02, 0.5) < Sharpener::new(&measured(80.)).delta(0.02, 0.5));
    }
}
