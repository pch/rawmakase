//! The exposure ratio between two frames, measured on their pixels: nominal
//! shutter speeds are rounded (a camera's 1/60 s is 1/64 s), and a ratio taken
//! from the EXIF would leave seams where the merge passes from one frame to
//! the other.
use crate::noise::Noise;
use rawmakase_decode::raw::Frame;
use rayon::prelude::*;

/// The fewest pixels a ratio is measured on.
const MIN_PIXELS: usize = 1000;
/// About how many pixels a ratio is measured on, spread over the frame.
const PIXELS: usize = 1 << 21;
/// A pixel counts when its brighter frame is below this fraction of its
/// saturation, under the clip ramp of `merge`, …
const BRIGHTEST: f32 = 0.85;
/// … and its darker frame is this many of its noise's standard deviations
/// above black.
const SIGNAL: f64 = 20.;
/// Tukey's biweight's tuning constant, in robust standard deviations.
const TUKEY: f64 = 4.685;
const ITERATIONS: usize = 5;

/// How much more light `bright` gathered than `dark` (above 1 when it is
/// brighter), from the pixels both saw unclipped and well above noise: the
/// median of their ratios, refined by Tukey's biweight so that a moving
/// object's pixels don't pull it. `None` with fewer than [`MIN_PIXELS`] such
/// pixels.
pub(crate) fn measure(dark: &Frame, bright: &Frame, dark_noise: Noise) -> Option<f64> {
    let n = dark.pixels.len().min(bright.pixels.len());
    let step = (n / PIXELS).max(1);
    let top = bright.saturation.iter().fold(f32::MAX, |a, b| a.min(*b)) * BRIGHTEST;
    let logs: Vec<f64> = (0..n.div_ceil(step))
        .into_par_iter()
        .filter_map(|k| {
            let i = k * step;
            if dark.clipped[i] || bright.clipped[i] {
                return None;
            }
            let (d, b) = (dark.pixels[i], bright.pixels[i]);
            if b.iter().any(|v| *v >= top) {
                return None;
            }
            let (sd, sb) = (sum(d), sum(b));
            // Each channel's variance, summed.
            let sigma = (0..3)
                .map(|c| dark_noise.variance(f64::from(d[c])))
                .sum::<f64>()
                .sqrt();
            (sd > SIGNAL * sigma && sb > 0.).then(|| (sb / sd).ln())
        })
        .collect();
    if logs.len() < MIN_PIXELS {
        return None;
    }
    Some(robust_mean(logs).exp())
}

fn sum(p: [f32; 3]) -> f64 {
    p.iter().map(|v| f64::from(*v)).sum()
}

/// The median of `values`, refined by iterations of Tukey's biweight with
/// the median absolute deviation as its scale.
fn robust_mean(mut values: Vec<f64>) -> f64 {
    let mut centre = median(&mut values);
    let mut deviations: Vec<f64> = values.iter().map(|v| (v - centre).abs()).collect();
    let scale = 1.4826 * median(&mut deviations);
    if scale <= 0. {
        return centre;
    }
    let reach = TUKEY * scale;
    for _ in 0..ITERATIONS {
        let (sum, weight) = values
            .par_iter()
            .map(|v| {
                let u = (v - centre) / reach;
                if u.abs() >= 1. {
                    (0., 0.)
                } else {
                    let w = (1. - u * u).powi(2);
                    (w * v, w)
                }
            })
            .reduce(|| (0., 0.), |a, b| (a.0 + b.0, a.1 + b.1));
        if weight <= 0. {
            break;
        }
        centre = sum / weight;
    }
    centre
}

fn median(values: &mut [f64]) -> f64 {
    let mid = values.len() / 2;
    *values.select_nth_unstable_by(mid, f64::total_cmp).1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Outliers, a fifth of the values, don't move the estimate.
    #[test]
    fn the_robust_mean_ignores_outliers() {
        let mut values: Vec<f64> = (0..1000)
            .map(|i| 2f64.ln() + ((i * 7919) % 101) as f64 / 101. * 0.02 - 0.01)
            .collect();
        values.extend(std::iter::repeat_n(5., 250));
        let got = robust_mean(values);
        assert!((got - 2f64.ln()).abs() < 1e-3, "{got}");
    }
}
