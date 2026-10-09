//! Detail > Color noise reduction, following Camera Raw 18.7,
//! fitted to the frequency response of its renders of synthetic noisy charts (Amount
//! 10–100 at two noise levels, Detail and Smoothness 0–100).
//!
//! Camera Raw's colour noise reduction is edge-aware and works at several scales: the
//! finest detail keeps a share of its colour noise that Detail sets (about a quarter at
//! the default 50, most of it at 100), coarser blotches are taken out up to a size
//! that grows with Amount and Smoothness, and strong colour edges are kept. Here the
//! chroma of the camera image (in square-root values, where noise is about the same at
//! every brightness) is split into a Laplacian pyramid; each level's detail is kept
//! where it stands out from a threshold and reduced where it doesn't, and the
//! luminance is left alone. It runs once on the camera image, before everything else,
//! so previews and exports agree.
//!
//! Camera Raw also reduces strong single-pixel colour lines by about a quarter, which
//! this keeps, and takes out somewhat less noise than this on very noisy photos.
use crate::camera_data::CameraImage;
use anyhow::{Result, ensure};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// The lowest Amount measured in Camera Raw, which already reduces the finest level as
/// much as the default does.
const LOWEST_MEASURED: f32 = 0.1;
/// Pyramid levels with a threshold below the finest.
const LEVELS: usize = 4;
/// The finest level: the share of its colour noise kept, by Detail 0, 25, 50, 75 and
/// 100, and the threshold above which its detail is kept whole.
const FINE_KEPT: [f32; 5] = [0.19, 0.22, 0.254, 0.45, 0.81];
const FINE_THRESHOLD: f32 = 0.057;
/// Thresholds of the coarser levels, in square-root camera values, as Amount (0–1)
/// above a level's start times its slope.
const START: [f32; LEVELS] = [0.18, 0.39, 0.3, 0.3];
const SLOPE: [f32; LEVELS] = [0.037, 0.026, 0.0026, 0.0026];
/// Detail's factor on the first coarser level's threshold, at Detail 0, 25, 50, 75
/// and 100 (higher Detail keeps more of it).
const DETAIL_FACTOR: [f32; 5] = [5., 2.5, 1., 0.4, 0.35];
/// Smoothness's factor on each coarser level's threshold, at Smoothness 0, 25, 50, 75
/// and 100: it reaches the coarsest blotches.
const SMOOTHNESS_FACTOR: [[f32; 5]; LEVELS] = [
    [0.6, 0.85, 1., 1.55, 1.85],
    [0., 0.5, 1., 2.3, 2.9],
    [0., 0.5, 1., 7., 11.],
    [0., 0., 0., 7., 14.],
];

fn at_quarters(table: &[f32; 5], v: f32) -> f32 {
    let x = v.clamp(0., 1.) * 4.;
    let i = (x as usize).min(3);
    table[i] + (table[i + 1] - table[i]) * (x - i as f32)
}

/// The measured colour noise reduction for a recipe's sliders.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChromaDenoise {
    /// Share of the finest level's noise kept.
    fine_kept: f32,
    /// Thresholds of the coarser levels; 0 leaves a level alone.
    thresholds: [f32; LEVELS],
}
impl ChromaDenoise {
    /// `None` at Amount 0.
    pub(crate) fn new(amount: f32, detail: f32, smoothness: f32) -> Option<Self> {
        if amount <= 0. {
            return None;
        }
        let thresholds = std::array::from_fn(|l| {
            let base = (amount.min(1.) - START[l]).max(0.) * SLOPE[l];
            let detail = if l == 0 {
                at_quarters(&DETAIL_FACTOR, detail)
            } else {
                1.
            };
            base * detail * at_quarters(&SMOOTHNESS_FACTOR[l], smoothness)
        });
        // The finest level comes in over the lowest measured Amount, 10, so the
        // slider has no step at its start.
        let ramp = (amount / LOWEST_MEASURED).min(1.);
        Some(Self {
            fine_kept: 1. - (1. - at_quarters(&FINE_KEPT, detail)) * ramp,
            thresholds,
        })
    }
    /// `im` with its colour noise reduced; stops with an error when `cancel` is set.
    pub(crate) fn apply(&self, im: &CameraImage, cancel: &AtomicBool) -> Result<CameraImage> {
        let (w, h) = (im.width as usize, im.height as usize);
        let (luma, data): (Vec<f32>, Vec<[f32; 2]>) = im
            .pixels
            .par_iter()
            .map(|p| {
                let [r, g, b] = p.map(|v| v.max(0.).sqrt());
                ((r + 2. * g + b) * 0.25, [r - g, b - g])
            })
            .unzip();
        let chroma = Plane { w, h, data };
        let chroma = self.filter(chroma, 0, cancel)?;
        let mut out = im.clone();
        out.recovered = Default::default();
        out.pixels
            .par_iter_mut()
            .zip(luma.par_iter().zip(chroma.data.par_iter()))
            .for_each(|(p, (y, [a, b]))| {
                let g = y - (a + b) * 0.25;
                *p = [g + a, g, g + b].map(|v| v.max(0.).powi(2));
            });
        Ok(out)
    }
    /// Filters `plane` as pyramid level `level`, recursing to the coarser ones.
    fn filter(&self, plane: Plane, level: usize, cancel: &AtomicBool) -> Result<Plane> {
        if level > LEVELS || plane.w < 4 || plane.h < 4 {
            return Ok(plane);
        }
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        let coarse = plane.down();
        let up = coarse.up(plane.w, plane.h);
        let coarse = self.filter(coarse, level + 1, cancel)?;
        ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
        let coarse_up = coarse.up(plane.w, plane.h);
        let (kept, threshold) = match level {
            0 => (self.fine_kept, FINE_THRESHOLD),
            l => (0., self.thresholds[l - 1]),
        };
        let data = plane
            .data
            .par_iter()
            .zip(up.data.par_iter().zip(coarse_up.data.par_iter()))
            .map(|(v, (u, c))| {
                let d = [v[0] - u[0], v[1] - u[1]];
                let gain = if threshold > 0. {
                    let x2 = (d[0] * d[0] + d[1] * d[1]) / (threshold * threshold);
                    kept + (1. - kept) * x2 / (1. + x2)
                } else {
                    1.
                };
                [c[0] + d[0] * gain, c[1] + d[1] * gain]
            })
            .collect();
        Ok(Plane {
            w: plane.w,
            h: plane.h,
            data,
        })
    }
}

/// Two-channel chroma at one pyramid level.
struct Plane {
    w: usize,
    h: usize,
    data: Vec<[f32; 2]>,
}
/// The 5-tap binomial filter.
const TAPS: [f32; 5] = [1. / 16., 4. / 16., 6. / 16., 4. / 16., 1. / 16.];
impl Plane {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            data: vec![[0.; 2]; w * h],
        }
    }
    /// Blurred and halved.
    fn down(&self) -> Self {
        let (w, h) = (self.w.div_ceil(2), self.h.div_ceil(2));
        let mut wide = Plane::new(w, self.h);
        wide.data
            .par_chunks_mut(w)
            .zip(self.data.par_chunks(self.w))
            .for_each(|(out, row)| {
                for (x, o) in out.iter_mut().enumerate() {
                    *o = taps5(|k| row[mirror(2 * x as isize + k - 2, self.w)]);
                }
            });
        let mut out = Plane::new(w, h);
        out.data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
            let rows: [&[[f32; 2]]; 5] = std::array::from_fn(|k| {
                let r = mirror(2 * y as isize + k as isize - 2, self.h);
                &wide.data[r * w..(r + 1) * w]
            });
            for (x, o) in out.iter_mut().enumerate() {
                *o = taps5(|k| rows[k as usize][x]);
            }
        });
        out
    }
    /// Doubled to `w` × `h` and blurred, as a Laplacian pyramid expands a level.
    fn up(&self, w: usize, h: usize) -> Self {
        let mut wide = Plane::new(w, self.h);
        wide.data
            .par_chunks_mut(w)
            .zip(self.data.par_chunks(self.w))
            .for_each(|(out, row)| {
                for (x, o) in out.iter_mut().enumerate() {
                    *o = expand(x, |c| row[mirror(c, self.w)]);
                }
            });
        let mut out = Plane::new(w, h);
        out.data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
            let row = |c: isize| {
                let r = mirror(c, self.h);
                &wide.data[r * w..(r + 1) * w]
            };
            let c = (y / 2) as isize;
            if y.is_multiple_of(2) {
                let (a, b, d) = (row(c - 1), row(c), row(c + 1));
                for (x, o) in out.iter_mut().enumerate() {
                    *o = mix(&[(a[x], EVEN[0]), (b[x], EVEN[1]), (d[x], EVEN[2])]);
                }
            } else {
                let (a, b) = (row(c), row(c + 1));
                for (x, o) in out.iter_mut().enumerate() {
                    *o = mix(&[(a[x], ODD), (b[x], ODD)]);
                }
            }
        });
        out
    }
}
/// Index `v` of `n`, mirrored at the edges.
fn mirror(v: isize, n: usize) -> usize {
    let n = n as isize;
    let v = v.abs();
    (if v >= n { 2 * n - 2 - v } else { v }).clamp(0, n - 1) as usize
}
fn taps5(at: impl Fn(isize) -> [f32; 2]) -> [f32; 2] {
    let mut s = [0.; 2];
    for (k, t) in TAPS.iter().enumerate() {
        let v = at(k as isize);
        s[0] += v[0] * t;
        s[1] += v[1] * t;
    }
    s
}
/// The expanding filter's weights: the binomial taps that land on coarse samples,
/// doubled, for even and odd output samples.
const EVEN: [f32; 3] = [TAPS[0] * 2., TAPS[2] * 2., TAPS[4] * 2.];
const ODD: f32 = TAPS[1] * 2.;
fn expand(i: usize, at: impl Fn(isize) -> [f32; 2]) -> [f32; 2] {
    let c = (i / 2) as isize;
    if i.is_multiple_of(2) {
        mix(&[(at(c - 1), EVEN[0]), (at(c), EVEN[1]), (at(c + 1), EVEN[2])])
    } else {
        mix(&[(at(c), ODD), (at(c + 1), ODD)])
    }
}
fn mix(parts: &[([f32; 2], f32)]) -> [f32; 2] {
    parts
        .iter()
        .fold([0.; 2], |s, (v, t)| [s[0] + v[0] * t, s[1] + v[1] * t])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> CameraImage {
        CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels: (0..w * h).map(|i| f(i % w, i / w)).collect(),
            metadata: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        }
    }
    fn chroma(p: [f32; 3]) -> [f32; 2] {
        let [r, g, b] = p.map(|v| v.max(0.).sqrt());
        [r - g, b - g]
    }
    fn hash(x: u32, y: u32, c: u32) -> f32 {
        let mut v =
            x.wrapping_mul(0x9e3779b9) ^ y.wrapping_mul(0x85ebca6b) ^ c.wrapping_mul(0xc2b2ae35);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2c1b3c6d);
        v ^= v >> 12;
        (v as f32 / u32::MAX as f32) * 2. - 1.
    }

    #[test]
    fn flat_colours_and_luminance_pass_unchanged() {
        let im = image(64, 48, |x, _| {
            if x < 32 {
                [0.2, 0.18, 0.1]
            } else {
                [0.05, 0.3, 0.4]
            }
        });
        let d = ChromaDenoise::new(0.25, 0.5, 0.5).unwrap();
        let out = d.apply(&im, &AtomicBool::new(false)).unwrap();
        // Away from the edge colours are as they were; a strong colour edge stays sharp,
        // within a pixel of where it was.
        for (i, (a, b)) in im.pixels.iter().zip(&out.pixels).enumerate() {
            let x = (i % 64) as i32;
            let tolerance = if (x - 32).abs() > 4 { 2e-3 } else { 0.03 };
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < tolerance, "{x}: {a:?} {b:?}");
            }
        }
        assert!(ChromaDenoise::new(0., 0.5, 0.5).is_none());
    }
    #[test]
    fn colour_noise_is_reduced_more_with_amount_and_less_with_detail() {
        let im = image(160, 160, |x, y| {
            let n = [hash(x, y, 1), hash(x, y, 2), hash(x, y, 3)];
            [0.18 + 0.02 * n[0], 0.18 + 0.02 * n[1], 0.18 + 0.02 * n[2]]
        });
        let noise = |im: &CameraImage| {
            let c: Vec<[f32; 2]> = im.pixels.iter().map(|p| chroma(*p)).collect();
            let n = c.len() as f32;
            let m = c
                .iter()
                .fold([0.; 2], |a, v| [a[0] + v[0] / n, a[1] + v[1] / n]);
            (c.iter()
                .map(|v| (v[0] - m[0]).powi(2) + (v[1] - m[1]).powi(2))
                .sum::<f32>()
                / n)
                .sqrt()
        };
        let before = noise(&im);
        let at = |a: f32, d: f32| {
            noise(
                &ChromaDenoise::new(a, d, 0.5)
                    .unwrap()
                    .apply(&im, &AtomicBool::new(false))
                    .unwrap(),
            ) / before
        };
        // Camera Raw at the default 25 keeps about half of a flat's colour noise.
        assert!((0.3..0.7).contains(&at(0.25, 0.5)), "{}", at(0.25, 0.5));
        assert!(at(1., 0.5) < at(0.25, 0.5));
        assert!(at(0.25, 1.) > at(0.25, 0.5));
        // Luminance is left alone.
        let out = ChromaDenoise::new(1., 0.5, 0.5)
            .unwrap()
            .apply(&im, &AtomicBool::new(false))
            .unwrap();
        for (a, b) in im.pixels.iter().zip(&out.pixels) {
            let luma = |p: &[f32; 3]| {
                let [r, g, b] = p.map(|v| v.max(0.).sqrt());
                r + 2. * g + b
            };
            assert!((luma(a) - luma(b)).abs() < 1e-4);
        }
    }
    #[test]
    fn low_amounts_start_from_no_change_and_cancel_stops_it() {
        let im = image(64, 64, |x, y| {
            [
                0.18 + 0.02 * hash(x, y, 1),
                0.18,
                0.18 + 0.02 * hash(x, y, 3),
            ]
        });
        let cancel = AtomicBool::new(false);
        let barely = ChromaDenoise::new(0.005, 0.5, 0.5)
            .unwrap()
            .apply(&im, &cancel)
            .unwrap();
        for (a, b) in im.pixels.iter().zip(&barely.pixels) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 2e-3, "{a:?} {b:?}");
            }
        }
        cancel.store(true, Ordering::Relaxed);
        assert!(
            ChromaDenoise::new(0.25, 0.5, 0.5)
                .unwrap()
                .apply(&im, &cancel)
                .is_err()
        );
    }
}
