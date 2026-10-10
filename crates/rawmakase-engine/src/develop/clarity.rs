//! Basic > Clarity, following Camera Raw 18.7 in the scene tone stage: a log2 gain of
//! detail at four scales, each an edge-aware blur of the photo's log scene luminance,
//! weighted by the local base level (`local_tone.rs`) relative to the photo's brightest
//! levels. The gain is computed on the Shadows/Highlights map's grid, so previews,
//! tiles and exports agree. See docs/scene-tone-stage.md and docs/tone-controls.md#clarity.
use crate::model::recipe::Recipe;
use rayon::prelude::*;

/// The Clarity this recipe renders in the scene tone stage.
pub(crate) fn measured(r: &Recipe) -> f32 {
    r.effects.clarity
}

/// Blur sizes (Gaussian σ) as a fraction of the map's long edge.
const SCALES: [f32; 4] = [0.004, 0.015, 0.05, 0.15];
/// Range σ of the edge-aware blurs, in log2 units.
const RANGE: f32 = 2.;
/// Base level relative to the photo's highlights (0.25·p99.9 + 0.75·p99 of its log
/// luminance) at the first and last weight knots; levels outside take the end knots.
const KNOT_LO: f32 = -8.;
const KNOT_HI: f32 = 0.5;
include!("clarity_data.rs");

/// The detail weights for Clarity `amount` (−1 to 1): linear from 0 to the fit at ±0.5,
/// then between the fits at ±0.5 and ±1.
fn weights(amount: f32) -> [[f32; KNOTS]; 4] {
    let a = amount.clamp(-1., 1.);
    let (half, full) = if a < 0. {
        (&WEIGHTS_M50, &WEIGHTS_M100)
    } else {
        (&WEIGHTS_50, &WEIGHTS_100)
    };
    let a = a.abs();
    std::array::from_fn(|s| {
        std::array::from_fn(|k| {
            if a <= 0.5 {
                half[s][k] * a / 0.5
            } else {
                half[s][k] + (full[s][k] - half[s][k]) * (a - 0.5) / 0.5
            }
        })
    })
}

/// Clarity's detail on a map grid: the photo's log luminance less its edge-aware blurs
/// at each scale, and the base level relative to the key, so the gain can be read at
/// any amount (a mask's added to the global slider).
pub(crate) struct ClarityDetail {
    details: [Vec<f32>; 4],
    base: Vec<f32>,
}
impl ClarityDetail {
    /// The detail of a `w` × `h` grid: `logs` is the photo's log2 scene luminance there,
    /// `base` its local base level and `key` the level the weights are relative to.
    pub(crate) fn of(logs: &[f32], base: &[f32], w: usize, h: usize, key: f32) -> Self {
        let long = w.max(h) as f32;
        let blurs = bilateral(logs, w, h, SCALES.map(|s| s * long), RANGE);
        Self {
            details: blurs.map(|b| logs.iter().zip(&b).map(|(l, b)| l - b).collect()),
            base: base.iter().map(|b| b - key).collect(),
        }
    }
    /// The log2 gain of Clarity `amount` at every grid point.
    pub(crate) fn field(&self, amount: f32) -> Vec<f32> {
        let weights = weights(amount);
        (0..self.base.len())
            .into_par_iter()
            .map(|i| {
                gain(
                    &weights,
                    self.base[i],
                    self.details.each_ref().map(|d| d[i]),
                )
            })
            .collect()
    }
    /// The log2 gain of Clarity `amount` where `sample` reads a grid.
    pub(crate) fn at(&self, sample: impl Fn(&[f32]) -> f32, amount: f32) -> f32 {
        if amount == 0. {
            return 0.;
        }
        gain(
            &weights(amount),
            sample(&self.base),
            self.details.each_ref().map(|d| sample(d)),
        )
    }
}
/// The gain of `weights` at a base level (relative to the key) and the detail at each
/// scale.
fn gain(weights: &[[f32; KNOTS]; 4], base: f32, details: [f32; 4]) -> f32 {
    let step = (KNOT_HI - KNOT_LO) / (KNOTS - 1) as f32;
    let u = ((base - KNOT_LO) / step).clamp(0., (KNOTS - 1) as f32);
    let k = (u as usize).min(KNOTS - 2);
    let t = u - k as f32;
    details
        .iter()
        .zip(weights)
        .map(|(d, w)| (w[k] * (1. - t) + w[k + 1] * t) * d)
        .sum()
}

/// Edge-aware blurs of `x` at spatial σ `sigmas` (px) and range σ `range`: the
/// piecewise linear bilateral filter of Durand and Dorsey, with Gaussian range weights
/// at levels `range / 2.5` apart. Each level is blurred on a grid reduced to about a
/// third of its σ and sampled back bilinearly, as a bilateral grid.
pub(super) fn bilateral<const N: usize>(
    x: &[f32],
    w: usize,
    h: usize,
    sigmas: [f32; N],
    range: f32,
) -> [Vec<f32>; N] {
    let (lo, hi) = x
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| {
            (a.min(*v), b.max(*v))
        });
    let step = range / 2.5;
    let count = (((hi - lo + 2. * range) / step).floor() as usize + 1).max(2);
    let level = |j: usize| lo - range + step * j as f32;
    let factors = sigmas.map(|s| ((s / 3.).floor() as usize).max(1));
    // Per level, per scale: the filtered value on that scale's grid.
    let filtered: Vec<[Grid; N]> = (0..count)
        .into_par_iter()
        .map(|j| {
            let l = level(j);
            let weight: Vec<f32> = x
                .iter()
                .map(|v| {
                    let d = (v - l) / range;
                    (-0.5 * d * d).exp()
                })
                .collect();
            let weighted: Vec<f32> = weight.iter().zip(x).map(|(a, v)| a * v).collect();
            std::array::from_fn(|s| {
                let f = factors[s];
                let (den, gw, gh) = reduce(&weight, w, h, f);
                let (num, ..) = reduce(&weighted, w, h, f);
                let sigma = sigmas[s] / f as f32;
                let (den, num) = (gauss(&den, gw, gh, sigma), gauss(&num, gw, gh, sigma));
                Grid {
                    values: num
                        .iter()
                        .zip(&den)
                        .map(|(n, d)| n / d.max(1e-12))
                        .collect(),
                    width: gw,
                    height: gh,
                    factor: f,
                }
            })
        })
        .collect();
    std::array::from_fn(|s| {
        (0..x.len())
            .into_par_iter()
            .map(|i| {
                let v = x[i];
                let j = (((v - lo + range) / step).floor().max(0.) as usize).min(count - 2);
                let t = ((v - level(j)) / step).clamp(0., 1.);
                let (px, py) = (i % w, i / w);
                filtered[j][s].sample(px, py) * (1. - t) + filtered[j + 1][s].sample(px, py) * t
            })
            .collect()
    })
}

/// A blurred level on a grid `factor` times coarser than the map.
struct Grid {
    values: Vec<f32>,
    width: usize,
    height: usize,
    factor: usize,
}
impl Grid {
    /// Bilinear value at map pixel `x`, `y`.
    fn sample(&self, x: usize, y: usize) -> f32 {
        if self.factor == 1 {
            return self.values[y * self.width + x];
        }
        let f = self.factor as f32;
        let fx = ((x as f32 + 0.5) / f - 0.5).clamp(0., (self.width - 1) as f32);
        let fy = ((y as f32 + 0.5) / f - 0.5).clamp(0., (self.height - 1) as f32);
        let (ix, iy) = (fx as usize, fy as usize);
        let (jx, jy) = ((ix + 1).min(self.width - 1), (iy + 1).min(self.height - 1));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let v = &self.values;
        let top = v[iy * self.width + ix] * (1. - tx) + v[iy * self.width + jx] * tx;
        let bottom = v[jy * self.width + ix] * (1. - tx) + v[jy * self.width + jx] * tx;
        top * (1. - ty) + bottom * ty
    }
}

/// Means of `f` × `f` blocks (partial at the right and bottom edges).
fn reduce(x: &[f32], w: usize, h: usize, f: usize) -> (Vec<f32>, usize, usize) {
    if f == 1 {
        return (x.to_vec(), w, h);
    }
    let (gw, gh) = (w.div_ceil(f), h.div_ceil(f));
    let mut sum = vec![0f32; gw * gh];
    let mut count = vec![0u32; gw * gh];
    for (i, v) in x.iter().enumerate() {
        let g = (i / w / f) * gw + (i % w) / f;
        sum[g] += v;
        count[g] += 1;
    }
    (
        sum.iter().zip(&count).map(|(s, c)| s / *c as f32).collect(),
        gw,
        gh,
    )
}

/// A Gaussian blur of σ `sigma` px as three box blurs, clamped at the borders.
fn gauss(x: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma < 0.5 {
        return x.to_vec();
    }
    let r = (((12. * sigma * sigma / 3. + 1.).sqrt() / 2.).round() as usize).max(1);
    let rows = boxes(x, w, r);
    let columns = boxes(&transpose(&rows, w, h), h, r);
    transpose(&columns, h, w)
}
/// Three passes of a (2r+1)-wide mean along each row of width `w`, clamped at the ends.
fn boxes(x: &[f32], w: usize, r: usize) -> Vec<f32> {
    let mut out = x.to_vec();
    let mut line = vec![0f32; w];
    for row in out.chunks_mut(w) {
        for _ in 0..3 {
            let get = |i: isize| row[i.clamp(0, w as isize - 1) as usize];
            let mut sum: f32 = (-(r as isize)..=r as isize).map(get).sum();
            for (i, v) in line.iter_mut().enumerate() {
                *v = sum / (2 * r + 1) as f32;
                sum += get(i as isize + r as isize + 1) - get(i as isize - r as isize);
            }
            row.copy_from_slice(&line);
        }
    }
    out
}
fn transpose(x: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.; x.len()];
    for y in 0..h {
        for c in 0..w {
            out[c * h + y] = x[y * w + c];
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flat_photos_and_zero_clarity_are_unchanged() {
        let (w, h) = (40, 30);
        let logs = vec![-2.; w * h];
        assert!(
            ClarityDetail::of(&logs, &logs, w, h, -1.)
                .field(1.)
                .iter()
                .all(|g| g.abs() < 1e-5)
        );
        let logs: Vec<f32> = (0..w * h)
            .map(|i| ((i * 7919) % 13) as f32 * 0.3 - 4.)
            .collect();
        assert!(
            ClarityDetail::of(&logs, &logs, w, h, -1.)
                .field(0.)
                .iter()
                .all(|g| *g == 0.)
        );
    }
    #[test]
    fn detail_gains_contrast_and_edges_stay_sharp() {
        let (w, h) = (64, 48);
        // A bright square on a dark ground, with fine texture on both.
        let logs: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let level = if (16..48).contains(&x) && (12..36).contains(&y) {
                    -1.
                } else {
                    -5.
                };
                level + if (x + y) % 2 == 0 { 0.2 } else { -0.2 }
            })
            .collect();
        let g = ClarityDetail::of(&logs, &logs, w, h, -1.).field(1.);
        let out: Vec<f32> = logs.iter().zip(&g).map(|(l, g)| l + g).collect();
        // Texture inside the square grows.
        let i = 24 * w + 32;
        assert!((out[i] - out[i + 1]).abs() > (logs[i] - logs[i + 1]).abs());
        // The blur does not cross the 4 EV edge: within 2 EV of each side.
        let [b, ..] = bilateral(&logs, w, h, [4.; 4], RANGE);
        assert!((b[24 * w + 17] + 1.).abs() < 1. && (b[24 * w + 14] + 5.).abs() < 1.);
    }
    #[test]
    fn weights_interpolate_through_the_fits() {
        let close = |a: [[f32; KNOTS]; 4], b: [[f32; KNOTS]; 4]| {
            a.as_flattened()
                .iter()
                .zip(b.as_flattened())
                .all(|(a, b)| (a - b).abs() < 1e-6)
        };
        assert!(close(weights(0.5), WEIGHTS_50));
        assert!(close(weights(1.), WEIGHTS_100));
        assert!((weights(0.25)[1][4] - WEIGHTS_50[1][4] / 2.).abs() < 1e-6);
    }
}
