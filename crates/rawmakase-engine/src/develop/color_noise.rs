//! Detail > Color noise reduction, following Camera Raw 18.7.
//!
//! Camera Raw's colour noise reduction is edge-aware and works at several scales: the
//! finest detail keeps a share of its colour noise that Detail sets, coarser blotches
//! are taken out up to a size that grows with Amount and Smoothness, and strong colour
//! edges are kept. Here the chroma of the camera image (in square-root values, where
//! noise is about the same at every brightness) is split into a Laplacian pyramid;
//! each level's detail is kept where it stands out from a threshold and reduced where
//! it doesn't, and the luminance is left alone. It runs once on the camera image,
//! before everything else, so previews and exports agree.
//!
//! Each level's threshold is a multiple of the colour noise measured at that level in
//! the photo itself, as Camera Raw sets it from the noise profile it knows for the
//! camera and ISO: on synthetic noisy charts carrying a noise profile, the share of
//! noise it keeps at each scale is the same from 1/16 to 4 times the noise of a Nikon
//! D7100 at ISO 2000, and at its default Amount 25 it takes out about 95% of the colour
//! noise at every scale. The thresholds were fitted to those renders (Amount 10–100,
//! Detail and Smoothness 0–100).
//!
//! The levels are the photo's: a reduced decode ([`CameraImage::photo_scale`]) is
//! filtered with the thresholds of the photo's scales its levels show, so it keeps the
//! share of each scale's noise the photo keeps.
use crate::camera_data::CameraImage;
use anyhow::{Result, ensure};
use rawmakase_cancel::Cancel;
use rayon::prelude::*;

/// Pyramid levels with a threshold below the finest.
const LEVELS: usize = 4;
/// The thresholds, as multiples of each level's measured noise
/// (finest first), at the Amounts Camera Raw rendered: fitted to the colour noise it
/// keeps at each scale of synthetic noisy charts carrying their noise profile, at
/// [`REFERENCE_NOISE`].
const ADAPTIVE_AMOUNTS: [f32; 4] = [0.1, 0.25, 0.5, 1.];
const ADAPTIVE_THRESHOLDS: [[f32; 4]; LEVELS + 1] = [
    [7.29, 7.08, 6.83, 6.8],
    [1.73, 3.34, 3.92, 4.03],
    [1.86, 4.29, 5.98, 5.98],
    [1.29, 2.74, 3.34, 3.35],
    [1.29, 2.74, 3.34, 3.35],
];
/// Detail's factors on each level's threshold, at 0, 25, 50, 75 and 100: higher
/// Detail keeps more of the finest colour.
const ADAPTIVE_DETAIL: [[f32; 5]; LEVELS + 1] = [
    [1.2, 1.11, 1., 0.58, 0.26],
    [1.13, 1.07, 1., 0.73, 0.54],
    [0.91, 0.93, 1., 1.16, 1.3],
    [0.95, 0.95, 1., 1.06, 1.4],
    [0.95, 0.95, 1., 1.06, 1.4],
];
/// Smoothness's factors on each level's threshold, at 0, 25, 50, 75 and 100: it
/// reaches the coarsest blotches.
const ADAPTIVE_SMOOTHNESS: [[f32; 5]; LEVELS + 1] = [
    [1.; 5],
    [0.96, 0.97, 1., 0.97, 0.98],
    [0.53, 0.8, 1., 1.14, 1.21],
    [0.28, 0.61, 1., 1.63, 2.12],
    [0.28, 0.61, 1., 1.63, 2.12],
];

/// The finest level's noise the thresholds were fitted at, in square-root camera
/// values (a Nikon D7100 at ISO 2000).
const REFERENCE_NOISE: f32 = 0.0075;
/// How much faster than the noise each level's threshold grows, as a power of the
/// finest level's noise over [`REFERENCE_NOISE`]: Camera Raw keeps more of the coarse
/// colour of clean photos than of noisy ones.
const NOISE_EXPONENT: [f32; LEVELS + 1] = [0.16, 0.05, 0.1, 0.6, 0.6];
/// Detail is kept by the sixth power of its size over the threshold, so a strong
/// colour edge stays as sharp as Camera Raw keeps it while noise well under the
/// threshold goes.
const ADAPTIVE_STEEPNESS: i32 = 3;

/// Level `l`'s threshold at `amount`: interpolated between the measured Amounts, and
/// from none at Amount 0 to the lowest.
fn adaptive_threshold(l: usize, amount: f32) -> f32 {
    let t = &ADAPTIVE_THRESHOLDS[l];
    let a = amount.clamp(0., 1.);
    if a <= ADAPTIVE_AMOUNTS[0] {
        return t[0] * a / ADAPTIVE_AMOUNTS[0];
    }
    let i = ADAPTIVE_AMOUNTS
        .iter()
        .rposition(|k| *k <= a)
        .unwrap()
        .min(2);
    let f = (a - ADAPTIVE_AMOUNTS[i]) / (ADAPTIVE_AMOUNTS[i + 1] - ADAPTIVE_AMOUNTS[i]);
    t[i] + (t[i + 1] - t[i]) * f
}

fn at_quarters(table: &[f32; 5], v: f32) -> f32 {
    let x = v.clamp(0., 1.) * 4.;
    let i = (x as usize).min(3);
    table[i] + (table[i + 1] - table[i]) * (x - i as f32)
}

/// `table`'s entry at the fractional pyramid level `level`, between the entries of the
/// levels either side by `mix` (given the finer, the coarser and how far from the
/// finer), and past the coarsest the coarsest's. A whole level reads its own entry.
fn at_level(table: &[f32; LEVELS + 1], level: f32, mix: fn(f32, f32, f32) -> f32) -> f32 {
    let level = level.clamp(0., LEVELS as f32);
    let i = level as usize;
    let f = level - i as f32;
    if f == 0. {
        return table[i];
    }
    mix(table[i], table[i + 1], f)
}

/// The colour noise reduction for a recipe's sliders.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChromaDenoise {
    /// Each level's threshold, finest first, as a multiple of the colour noise
    /// measured at the level.
    thresholds: [f32; LEVELS + 1],
}
impl ChromaDenoise {
    /// `None` at Amount 0.
    pub(crate) fn new(amount: f32, detail: f32, smoothness: f32) -> Option<Self> {
        if amount <= 0. {
            return None;
        }
        let thresholds = std::array::from_fn(|l| {
            adaptive_threshold(l, amount)
                * at_quarters(&ADAPTIVE_DETAIL[l], detail)
                * at_quarters(&ADAPTIVE_SMOOTHNESS[l], smoothness)
        });
        Some(Self { thresholds })
    }
    /// `im` with its colour noise reduced; stops with an error when `cancel` is set.
    pub(crate) fn apply(&self, im: &CameraImage, cancel: Cancel<'_>) -> Result<CameraImage> {
        let (w, h) = (im.width as usize, im.height as usize);
        let offset = super::laplacian::photo_level_offset(im);
        let wb = im.metadata.wb.map(|v| v.max(0.001));
        let (luma, data): (Vec<f32>, Vec<[f32; 2]>) = im
            .pixels
            .par_iter()
            .map(|p| {
                let [r, g, b] = p.map(|v| v.max(0.).sqrt());
                ((r + 2. * g + b) * 0.25, [r - g, b - g])
            })
            .unzip();
        // Where the noise can be measured: below the sensor's clipping, and above
        // black (see `noise`).
        let measurable = Mask {
            w,
            h,
            data: im
                .pixels
                .par_iter()
                .map(|p| MaskPixel {
                    unclipped: (0..3).all(|c| p[c] / wb[c] < MEASURABLE_PEAK),
                    black: if p.iter().any(|v| *v <= 0.) { 1. } else { 0. },
                })
                .collect(),
        };
        let chroma = Plane { w, h, data };
        let chroma = self.filter(chroma, measurable, 0., 0., 0, offset, cancel)?;
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
    /// Filters `plane` as pyramid level `level`, recursing to the coarser ones;
    /// `measurable` is where its noise can be measured, `finer_noise` and
    /// `finest_noise` the noise measured at the level above and at the finest level,
    /// and `offset` the photo's level that this image's finest shows.
    #[allow(clippy::too_many_arguments)]
    fn filter(
        &self,
        plane: Plane,
        measurable: Mask,
        finer_noise: f32,
        finest_noise: f32,
        level: usize,
        offset: f32,
        cancel: Cancel<'_>,
    ) -> Result<Plane> {
        // The photo's level this one shows. A level up to one past the photo's last
        // is filtered with the last's settings. One that starts at the photo's
        // unfiltered residual, or past it, is left alone, as the photo leaves it.
        let photo_level = level as f32 + offset;
        if photo_level >= (LEVELS + 1) as f32 || plane.w < 4 || plane.h < 4 {
            return Ok(plane);
        }
        ensure!(!cancel.is_cancelled(), "Render superseded");
        let coarse = plane.down();
        let up = coarse.up(plane.w, plane.h);
        // A level too small or too clipped to measure has the finer level's noise, as
        // much smaller as coarser levels usually are. Demosaiced noise is never larger
        // at a coarser level, so more than the finer level's is colour detail.
        let noise = match noise(&plane, &up, &measurable) {
            Some(n) if level == 0 => n,
            Some(n) => n.min(finer_noise),
            None => finer_noise * COARSER_NOISE,
        };
        let finest_noise = if level == 0 { noise } else { finest_noise };
        let coarse = self.filter(
            coarse,
            measurable.down(),
            noise,
            finest_noise,
            level + 1,
            offset,
            cancel,
        )?;
        ensure!(!cancel.is_cancelled(), "Render superseded");
        let coarse_up = coarse.up(plane.w, plane.h);
        // Between two of the photo's levels, the thresholds' ratio is shared out
        // geometrically and the exponents' difference linearly.
        let geometric = |a: f32, b: f32, f: f32| a * (b / a).powf(f);
        let linear = |a: f32, b: f32, f: f32| a + (b - a) * f;
        let threshold = at_level(&self.thresholds, photo_level, geometric)
            * noise
            * (finest_noise / REFERENCE_NOISE).powf(at_level(&NOISE_EXPONENT, photo_level, linear));
        let data = plane
            .data
            .par_iter()
            .zip(up.data.par_iter().zip(coarse_up.data.par_iter()))
            .map(|(v, (u, c))| {
                let d = [v[0] - u[0], v[1] - u[1]];
                let size = d[0] * d[0] + d[1] * d[1];
                let gain = if threshold <= 0. {
                    1.
                } else {
                    // As x / (1 + x) with x = (size / threshold²)^steepness, without
                    // overflowing where the threshold is tiny next to the detail.
                    1. / (1. + (threshold * threshold / size).powi(ADAPTIVE_STEEPNESS))
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

/// Above this share of the sensor's range a channel may clip, and clipped pixels have
/// no noise to measure.
const MEASURABLE_PEAK: f32 = 0.95;
/// How much smaller a level's colour noise is than the finer level's, on a Bayer
/// photo's demosaiced noise.
const COARSER_NOISE: f32 = 0.6;
/// Side of the blocks the noise is measured in, at each level.
const BLOCK: usize = 8;
/// Which of the blocks, from the smoothest, give the noise: real colour detail only
/// adds to a block's variation, so the smoothest blocks are the photo's noise alone.
const BLOCK_PERCENTILE: f32 = 0.1;
/// The fewest measurable blocks a level's noise is measured from.
const MIN_BLOCKS: usize = 16;
/// The colour noise of a level's detail (`plane` − `up`): the RMS of the detail in its
/// smoothest measurable blocks, those with no black when there are enough of them;
/// `None` with fewer than [`MIN_BLOCKS`] either way.
fn noise(plane: &Plane, up: &Plane, measurable: &Mask) -> Option<f32> {
    let (bw, bh) = (plane.w / BLOCK, plane.h / BLOCK);
    let blocks: Vec<(f32, f32)> = (0..bw * bh)
        .into_par_iter()
        .filter_map(|b| {
            let (x0, y0) = ((b % bw) * BLOCK, (b / bw) * BLOCK);
            let (mut sum, mut black) = (0., 0.);
            for y in y0..y0 + BLOCK {
                for x in x0..x0 + BLOCK {
                    let i = y * plane.w + x;
                    if !measurable.data[i].unclipped {
                        return None;
                    }
                    black += measurable.data[i].black;
                    let (v, u) = (plane.data[i], up.data[i]);
                    sum += (v[0] - u[0]).powi(2) + (v[1] - u[1]).powi(2);
                }
            }
            let n = (BLOCK * BLOCK) as f32;
            Some((sum / n, black / n))
        })
        .collect();
    let within = |most: f32| -> Vec<f32> {
        blocks
            .iter()
            .filter(|(_, black)| *black <= most)
            .map(|(energy, _)| *energy)
            .collect()
    };
    let mut energies = within(0.);
    if energies.len() < MIN_BLOCKS {
        energies = within(MAX_BLACK);
    }
    if energies.len() < MIN_BLOCKS {
        return None;
    }
    let k = (energies.len() as f32 * BLOCK_PERCENTILE) as usize;
    let (_, e, _) = energies.select_nth_unstable_by(k, f32::total_cmp);
    Some(e.sqrt())
}
/// The most of a block's pixels that may be black in a channel when too few blocks have
/// none: in a dark, noisy photo the noise reaches black in many pixels and is still
/// noise to take out, while black areas have none to measure.
const MAX_BLACK: f32 = 0.9;
/// Where the noise can be measured at one pyramid level.
struct Mask {
    w: usize,
    h: usize,
    data: Vec<MaskPixel>,
}
#[derive(Clone, Copy)]
struct MaskPixel {
    /// No channel near the sensor's clipping.
    unclipped: bool,
    /// The share of pixels black in a channel.
    black: f32,
}
impl Mask {
    /// Halved as [`Plane::down`] halves: unclipped where every pixel its filter reads
    /// is, with their share of black.
    fn down(&self) -> Self {
        let (w, h) = (self.w.div_ceil(2), self.h.div_ceil(2));
        let data = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % w) as isize, (i / w) as isize);
                let mut out = MaskPixel {
                    unclipped: true,
                    black: 0.,
                };
                for dy in -2..=2 {
                    for dx in -2..=2 {
                        let p = self.data
                            [mirror(2 * y + dy, self.h) * self.w + mirror(2 * x + dx, self.w)];
                        out.unclipped &= p.unclipped;
                        out.black += p.black / 25.;
                    }
                }
                out
            })
            .collect();
        Self { w, h, data }
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
    use std::sync::atomic::{AtomicBool, Ordering};

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
            hdr: Default::default(),
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
        let out = d.apply(&im, Cancel::NEVER).unwrap();
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
        let mut im = image(160, 160, |x, y| {
            let n = [hash(x, y, 1), hash(x, y, 2), hash(x, y, 3)];
            [0.18 + 0.02 * n[0], 0.18 + 0.02 * n[1], 0.18 + 0.02 * n[2]]
        });
        // White balance 1: the sensor clips at 1, so the noise is measurable.
        im.metadata.wb = [1.; 3];
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
                    .apply(&im, Cancel::NEVER)
                    .unwrap(),
            ) / before
        };
        // Camera Raw at the default 25 takes out about 95% of the colour noise.
        assert!(at(0.25, 0.5) < 0.1, "{}", at(0.25, 0.5));
        assert!(at(1., 0.5) < at(0.25, 0.5));
        assert!(at(0.25, 1.) > at(0.25, 0.5));
        // Luminance is left alone.
        let out = ChromaDenoise::new(1., 0.5, 0.5)
            .unwrap()
            .apply(&im, Cancel::NEVER)
            .unwrap();
        for (a, b) in im.pixels.iter().zip(&out.pixels) {
            let luma = |p: &[f32; 3]| {
                let [r, g, b] = p.map(|v| v.max(0.).sqrt());
                r + 2. * g + b
            };
            assert!((luma(a) - luma(b)).abs() < 1e-4);
        }
    }
    fn noisy(w: u32, h: u32, amplitude: f32, f: impl Fn(u32, u32) -> [f32; 3]) -> CameraImage {
        let mut im = image(w, h, f);
        // White balance 1: the sensor clips at 1 in every channel.
        im.metadata.wb = [1.; 3];
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            // Shot noise: its spread grows with the square root of the signal.
            for (c, v) in p.iter_mut().enumerate() {
                *v += amplitude * v.sqrt() * hash(x, y, c as u32);
            }
        }
        im
    }
    fn chroma_noise(im: &CameraImage, x0: u32, x1: u32) -> f32 {
        let c: Vec<[f32; 2]> = im
            .pixels
            .iter()
            .enumerate()
            .filter(|(i, _)| (x0..x1).contains(&(*i as u32 % im.width)))
            .map(|(_, p)| chroma(*p))
            .collect();
        let n = c.len() as f32;
        let m = c
            .iter()
            .fold([0.; 2], |a, v| [a[0] + v[0] / n, a[1] + v[1] / n]);
        (c.iter()
            .map(|v| (v[0] - m[0]).powi(2) + (v[1] - m[1]).powi(2))
            .sum::<f32>()
            / n)
            .sqrt()
    }
    #[test]
    fn adaptive_keeps_strong_colour_edges_and_clean_photos() {
        let cancel = AtomicBool::new(false);
        let d = ChromaDenoise::new(0.25, 0.5, 0.5).unwrap();
        let (red, cyan) = ([0.3, 0.06, 0.06], [0.05, 0.15, 0.25]);
        let side = |x: u32| if x < 128 { red } else { cyan };
        // Without noise, or with next to none (a tiny threshold), nothing changes.
        for amplitude in [0., 1e-7] {
            let clean = noisy(256, 128, amplitude, |x, _| side(x));
            let out = d.apply(&clean, Cancel::new(&cancel)).unwrap();
            for (a, b) in clean.pixels.iter().zip(&out.pixels) {
                for c in 0..3 {
                    assert!((a[c] - b[c]).abs() < 1e-4, "{amplitude}: {a:?} {b:?}");
                }
            }
        }
        // With noise, the noise goes and the edge stays sharp: three pixels from it the
        // chroma is within 5% of the side's.
        let im = noisy(256, 128, 0.03, |x, _| side(x));
        let out = d.apply(&im, Cancel::new(&cancel)).unwrap();
        assert!(chroma_noise(&out, 16, 112) < 0.2 * chroma_noise(&im, 16, 112));
        let mean = |x: u32| {
            let rows = (0..128).map(|y| chroma(out.pixels[(y * 256 + x) as usize]));
            rows.fold([0.; 2], |a, v| [a[0] + v[0] / 128., a[1] + v[1] / 128.])
        };
        for (x, colour) in [(124, red), (131, cyan)] {
            let (want, got) = (chroma(colour), mean(x));
            let error = (want[0] - got[0]).hypot(want[1] - got[1]) / want[0].hypot(want[1]);
            assert!(error < 0.05, "{x}: {got:?} {want:?}");
        }
    }
    #[test]
    fn adaptive_measures_the_noise_where_nothing_clips() {
        // Half the photo clipped (no noise there) or black: the noise is measured in
        // the rest and still taken out.
        let cancel = AtomicBool::new(false);
        let im = noisy(256, 128, 0.03, |x, _| match x {
            0..64 => [1.; 3],
            64..128 => [0.; 3],
            _ => [0.18; 3],
        });
        let out = ChromaDenoise::new(0.25, 0.5, 0.5)
            .unwrap()
            .apply(&im, Cancel::new(&cancel))
            .unwrap();
        let (before, after) = (chroma_noise(&im, 140, 240), chroma_noise(&out, 140, 240));
        assert!(after < 0.2 * before, "{before} {after}");
    }
    #[test]
    fn adaptive_measures_dark_noise_that_clips_at_black() {
        // A dark, noisy flat whose noise reaches black in a channel here and there
        // (raw files clip it at zero), as underexposed high-ISO photos are: every block
        // has some clipped pixels, and its noise is still measured and taken out.
        let cancel = AtomicBool::new(false);
        let mut im = noisy(256, 256, 0.15, |_, _| [0.002, 0.004, 0.003]);
        for p in im.pixels.iter_mut() {
            *p = p.map(|v| v.max(0.));
        }
        let clipped = im.pixels.iter().filter(|p| p.contains(&0.)).count();
        assert!(clipped > im.pixels.len() / 20, "{clipped}");
        let out = ChromaDenoise::new(0.25, 0.5, 0.5)
            .unwrap()
            .apply(&im, Cancel::new(&cancel))
            .unwrap();
        let (before, after) = (chroma_noise(&im, 0, 256), chroma_noise(&out, 0, 256));
        assert!(after < 0.3 * before, "{before} {after}");
        // A black frame has no noise to measure, and stays black.
        let black = noisy(256, 256, 0.15, |_, _| [0.; 3]);
        let out = ChromaDenoise::new(1., 0.5, 0.5)
            .unwrap()
            .apply(&black, Cancel::new(&cancel))
            .unwrap();
        assert!(out.pixels.iter().all(|p| *p == [0.; 3]));
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
            .apply(&im, Cancel::new(&cancel))
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
                .apply(&im, Cancel::new(&cancel))
                .is_err()
        );
    }
}
