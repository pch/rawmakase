//! Colour of stripes too fine for X-Trans's red and blue samples.
//!
//! Stripes about 2½ to 3½ pixels apart (a mesh, a grille, louvres seen from afar)
//! are sampled by X-Trans's red and blue at nearly the same phase of the stripes
//! over long stretches: red sits mostly at one column of every three. Their colour
//! then cannot be told from their brightness, by any choice of direction, and the
//! demosaic leaves green and magenta bands along them that drift with the beat
//! between the stripes and the pattern (X100F DSCF8231's tower). Where luminance
//! along a row or a column, averaged over the lines beside it, is dominated by
//! such a period, luminance-normalised R − G and B − G are averaged along that
//! axis, preserving resolved colour changes along the slats.
//! Each pixel keeps its luminance. Single edges, text,
//! noise and coarser detail have little of their luminance in that band and keep
//! their colour.
//!
//! Where to suppress, and the mean, change slowly, so they are found on a grid of
//! every [`GRID`]th row and column; the detector still reads every pixel of a
//! line.
use rayon::prelude::*;

/// Frequencies, in cycles per pixel, at which luminance is tested: together, with
/// the detector's bandwidth, about 0.25 to 0.45 (periods of 2.2 to 4 pixels).
const FREQUENCIES: [f32; 2] = [0.317, 0.383];
/// Half the taps of the detector along a row or column.
const HALF_TAPS: usize = 6;
/// Pixels between grid points.
const GRID: usize = 4;
/// Lines on either side averaged into the line the detector reads.
const LINES: usize = 4;
/// Grid lines on either side over which the detector's energy is summed.
const ACROSS: usize = 1;
/// Share of the variance along a line at one of [`FREQUENCIES`] where
/// suppression starts, and where it is full.
const SHARE: (f32, f32) = (0.25, 0.45);
/// Added to the variance, in units of the square root of luminance, so that noise
/// on a flat area is not taken for stripes.
const NOISE: f32 = 2e-4;
/// Grid radius over which the suppression is spread, so that it covers the
/// stripes' edges.
const SPREAD: usize = 1;
/// Grid radius along the slats within which a reliable core can cover their edge.
const COVERAGE_RADIUS: usize = 3;
/// Grid radius, along the stripes' period, of the stretch whose mean colour
/// replaces theirs.
const AREA: usize = 12;
/// Least total weight of the points a mean is taken over, so that a mean rests
/// on at least part of a fully suppressed point.
const MIN_TOTAL: f32 = 0.25;
/// Pixels this close to the image's edges, whose demosaic read mirrored samples,
/// count for nothing in the mean: their colour can be far off on such stripes.
const EDGE: usize = 16;
/// Rows the detector, the spread and the area reach beyond a band; a multiple of
/// [`GRID`], so that grid rows are the same image rows in every band.
const MARGIN: usize = (HALF_TAPS + LINES + GRID * (ACROSS + SPREAD + AREA + COVERAGE_RADIUS + 1))
    .next_multiple_of(GRID);
/// Rows of output per band; a multiple of [`GRID`], as [`MARGIN`].
const BAND: usize = 256;
const _: () = assert!(BAND.is_multiple_of(GRID));

/// Replaces the colour of stripes in [`FREQUENCIES`] by luminance-pooled chromaticity along their period.
pub(super) fn suppress(image: &mut [[f32; 3]], w: usize, h: usize) {
    if w == 0 || h == 0 {
        return;
    }
    let changes: Vec<Vec<(usize, [f32; 3])>> = (0..h.div_ceil(BAND))
        .into_par_iter()
        .map(|band| band_changes(image, w, h, band * BAND, ((band + 1) * BAND).min(h)))
        .collect();
    for (i, p) in changes.into_iter().flatten() {
        image[i] = p;
    }
}

/// The changed pixels of rows `y0..y1`, as (image index, colour).
fn band_changes(
    image: &[[f32; 3]],
    w: usize,
    h: usize,
    y0: usize,
    y1: usize,
) -> Vec<(usize, [f32; 3])> {
    let (lo, hi) = (y0.saturating_sub(MARGIN), (y1 + MARGIN).min(h));
    let bh = hi - lo;
    let pixels = &image[lo * w..hi * w];
    // The square root of luminance, so that shares do not depend on exposure.
    let luminance: Vec<f32> = pixels
        .iter()
        .map(|p| ((p[0] + 2. * p[1] + p[2]) * 0.25).max(0.).sqrt())
        .collect();
    let grid = Plane::new(w, bh, GRID);
    let (strength, along_rows) = grid.strength(&luminance);
    if strength.iter().all(|&s| s == 0.) {
        return Vec::new();
    }
    let at_grid = |v: &dyn Fn(usize) -> f32| -> Vec<f32> {
        (0..grid.w * grid.h).map(|g| v(grid.pixel(g))).collect()
    };
    let spread_n = ((2 * SPREAD + 1) * (2 * SPREAD + 1)) as f32;
    let weight: Vec<f32> = grid
        .box_sum(&strength, SPREAD, SPREAD)
        .into_iter()
        .map(|v| (v / spread_n).min(1.))
        .collect();
    let inside = |g: usize| {
        let (x, y) = ((g % grid.w) * GRID, lo + (g / grid.w) * GRID);
        (EDGE..w.saturating_sub(EDGE)).contains(&x) && (EDGE..h.saturating_sub(EDGE)).contains(&y)
    };
    // Per axis of the period, the points that count towards its means.
    let counted = [true, false].map(|rows| -> Vec<f32> {
        (0..grid.w * grid.h)
            .map(|g| {
                if inside(g) && along_rows[g] == rows {
                    weight[g]
                } else {
                    0.
                }
            })
            .collect()
    });
    // Pool channel differences and luminance before division, across blocks as
    // well as within them. Normalising each dark block separately would give its
    // noisy chromaticity the same vote as a bright block's reliable colour.
    let block = |c: usize, i: usize| {
        let (x0, y0) = (i % w, i / w);
        let mut sum = 0.;
        for y in y0..(y0 + GRID).min(bh) {
            for x in x0..(x0 + GRID).min(w) {
                let p = pixels[y * w + x];
                sum += if c == 1 {
                    (p[0] + 2. * p[1] + p[2]) * 0.25
                } else {
                    p[c] - p[1]
                };
            }
        }
        sum
    };
    let sums = [0, 2, 1].map(|c| at_grid(&|i| block(c, i)));
    // Keep detection support separate from the luminance denominator: a dark
    // but confidently detected stripe still has a valid mean.
    let means = [(0, AREA, 0), (1, 0, AREA)].map(|(axis, rx, ry)| {
        let counted = &counted[axis];
        let pooled = sums.each_ref().map(|c| {
            let v: Vec<f32> = c.iter().zip(counted).map(|(d, wt)| d * wt).collect();
            grid.box_sum(&v, rx, ry)
        });
        (pooled, grid.box_sum(counted, rx, ry))
    });
    let mean: Vec<Option<[f32; 2]>> = (0..grid.w * grid.h)
        .map(|g| {
            let ([red, blue, light], support) = &means[usize::from(!along_rows[g])];
            (support[g] > MIN_TOTAL && light[g] > 1e-8)
                .then(|| [red[g] / light[g], blue[g] / light[g]])
        })
        .collect();
    // Confidence describes the detected area, not fractional coverage of each
    // edge pixel. A box average dilutes a fully detected slat with its flat
    // neighbours and blends the original alias back into the boundary.
    // Spread coverage only from a nearby reliable core of the same orientation;
    // retain the original weights for estimating colour and for weak detections.
    let coverage: Vec<f32> = (0..grid.w * grid.h)
        .map(|g| {
            let (x, y) = (g % grid.w, g / grid.w);
            let mut core = 0f32;
            for offset in -(COVERAGE_RADIUS as isize)..=COVERAGE_RADIUS as isize {
                let (xx, yy) = if along_rows[g] {
                    (
                        x,
                        (y as isize + offset).clamp(0, grid.h as isize - 1) as usize,
                    )
                } else {
                    (
                        (x as isize + offset).clamp(0, grid.w as isize - 1) as usize,
                        y,
                    )
                };
                let j = yy * grid.w + xx;
                if along_rows[j] == along_rows[g] {
                    core = core.max(weight[j]);
                }
            }
            let reliable = ((core - 0.75) / 0.25).clamp(0., 1.);
            weight[g] + reliable * ((weight[g] * spread_n).min(1.) - weight[g])
        })
        .collect();
    let mut changes = Vec::new();
    for y in y0..y1 {
        for x in 0..w {
            // Between the four nearest grid points' blocks, so that the mean and
            // the weight follow colour that changes within a block's width.
            let (wt, m) = grid.interpolate(x, y - lo, &coverage, &mean);
            let Some(m) = m.filter(|_| wt > 0.) else {
                continue;
            };
            let p = pixels[(y - lo) * w + x];
            let l = (p[0] + 2. * p[1] + p[2]) * 0.25;
            let m = m.map(|v| v * l);
            let (r, b) = (p[0] - p[1], p[2] - p[1]);
            let r = r + wt * (m[0] - r);
            let b = b + wt * (m[1] - b);
            changes.push((y * w + x, with_colour(p, r, b)));
        }
    }
    changes
}

/// `p` with R − G and B − G of `r` and `b`, keeping its luminance, (R + 2G + B) / 4.
/// Where that would make a channel negative, the colour is moved towards neutral
/// until it is not, rather than clipping the channel, which would add light.
fn with_colour(p: [f32; 3], r: f32, b: f32) -> [f32; 3] {
    let l = ((p[0] + 2. * p[1] + p[2]) * 0.25).max(0.);
    // Each channel as luminance plus a share of the colour.
    let offsets = [(3. * r - b) * 0.25, -(r + b) * 0.25, (3. * b - r) * 0.25];
    let share = offsets
        .iter()
        .filter(|&&o| o < 0.)
        .fold(1f32, |t, o| t.min(l / -o));
    offsets.map(|o| (l + share * o).max(0.))
}

/// Per grid point, the share of luminance's variance at one of [`FREQUENCIES`]
/// along the rows and down the columns. Each axis reads a line averaged over
/// [`LINES`] lines on either side, which keeps stripes across the line and
/// averages out noise and detail along it (a mesh's rods across slats); energy
/// and variance are pooled over [`ACROSS`] grid lines across the period before
/// thresholding.
fn periodic_shares(l: &[f32], grid: &Plane) -> [Vec<f32>; 2] {
    let (w, h, step) = (grid.image_w, grid.image_h, grid.step);
    let taps = FREQUENCIES.map(Taps::new);
    let len = grid.w * grid.h;
    let lines = (2 * LINES + 1) as f32;
    // Energy and variance of the 2 × HALF_TAPS samples around `at` on `line`.
    let measure = |line: &[f32], at: usize| -> (f32, f32) {
        let n = line.len();
        let v: [f32; 2 * HALF_TAPS] =
            std::array::from_fn(|k| line[(at + k).saturating_sub(HALF_TAPS).min(n - 1)]);
        let energy = taps.iter().map(|t| t.energy(&v)).fold(0f32, f32::max);
        (energy, taps[0].variance(&v))
    };
    let mut rows = [vec![0f32; len], vec![0f32; len]];
    let mut line = vec![0f32; w];
    for gy in 0..grid.h {
        let y = gy * step;
        line.fill(0.);
        for yy in y.saturating_sub(LINES)..=(y + LINES) {
            let yy = yy.min(h - 1);
            for (s, v) in line.iter_mut().zip(&l[yy * w..(yy + 1) * w]) {
                *s += v;
            }
        }
        // Rows clamped at the top edge are missing from the sum; count them as
        // the first row, as everywhere else.
        for _ in y..LINES {
            for (s, v) in line.iter_mut().zip(&l[..w]) {
                *s += v;
            }
        }
        line.iter_mut().for_each(|s| *s /= lines);
        for gx in 0..grid.w {
            let (e, v) = measure(&line, gx * step);
            rows[0][gy * grid.w + gx] = e;
            rows[1][gy * grid.w + gx] = v;
        }
    }
    // Per grid column, the image's rows averaged over the columns around it,
    // stored a grid column at a time.
    let mut columns_avg = vec![0f32; grid.w * h];
    for y in 0..h {
        let row = &l[y * w..(y + 1) * w];
        for gx in 0..grid.w {
            let x = gx * step;
            let sum: f32 = (x as isize - LINES as isize..=(x + LINES) as isize)
                .map(|xx| row[xx.clamp(0, w as isize - 1) as usize])
                .sum();
            columns_avg[gx * h + y] = sum / lines;
        }
    }
    let mut columns = [vec![0f32; len], vec![0f32; len]];
    for gx in 0..grid.w {
        let column = &columns_avg[gx * h..(gx + 1) * h];
        for gy in 0..grid.h {
            let (e, v) = measure(column, gy * step);
            columns[0][gy * grid.w + gx] = e;
            columns[1][gy * grid.w + gx] = v;
        }
    }
    let noise = NOISE * (2 * ACROSS + 1) as f32;
    let share = |[e, v]: [Vec<f32>; 2], rx: usize, ry: usize| -> Vec<f32> {
        let (e, v) = (grid.box_sum(&e, rx, ry), grid.box_sum(&v, rx, ry));
        e.iter().zip(&v).map(|(e, v)| e / (v + noise)).collect()
    };
    [share(rows, 0, ACROSS), share(columns, ACROSS, 0)]
}

/// A grid of every `step`th row and column of a band of `image_w` × `image_h`
/// pixels, read with coordinates clamped to its edges.
struct Plane {
    w: usize,
    h: usize,
    step: usize,
    image_w: usize,
    image_h: usize,
}

impl Plane {
    fn new(image_w: usize, image_h: usize, step: usize) -> Self {
        Self {
            w: image_w.div_ceil(step),
            h: image_h.div_ceil(step),
            step,
            image_w,
            image_h,
        }
    }

    /// `weight` and `mean` at pixel (`x`, `y`), bilinear between the centres of the
    /// four nearest grid points' blocks; the mean only from points that have one.
    fn interpolate(
        &self,
        x: usize,
        y: usize,
        weight: &[f32],
        mean: &[Option<[f32; 2]>],
    ) -> (f32, Option<[f32; 2]>) {
        let centre = (self.step as f32 - 1.) / 2.;
        let at = |v: usize, n: usize| {
            let f = ((v as f32 - centre) / self.step as f32).clamp(0., (n - 1) as f32);
            let i = (f as usize).min(n.saturating_sub(2));
            (i, (f - i as f32).min(1.))
        };
        let ((gx, fx), (gy, fy)) = (at(x, self.w), at(y, self.h));
        let (mut wt, mut sum, mut total) = (0., [0f32; 2], 0.);
        for (dy, ky) in [(0, 1. - fy), (1, fy)] {
            for (dx, kx) in [(0, 1. - fx), (1, fx)] {
                let (xx, yy) = ((gx + dx).min(self.w - 1), (gy + dy).min(self.h - 1));
                let (g, k) = (yy * self.w + xx, kx * ky);
                wt += weight[g] * k;
                if let Some(m) = mean[g] {
                    sum = [sum[0] + m[0] * k, sum[1] + m[1] * k];
                    total += k;
                }
            }
        }
        (wt, (total > 0.).then(|| sum.map(|v| v / total)))
    }

    /// The band pixel of grid point `g`.
    fn pixel(&self, g: usize) -> usize {
        (g / self.w) * self.step * self.image_w + (g % self.w) * self.step
    }

    /// How much each point is in stripes, by [`SHARE`] of the local luminance
    /// variance at one of [`FREQUENCIES`], and whether their period runs along
    /// the rows (else down the columns).
    fn strength(&self, luminance: &[f32]) -> (Vec<f32>, Vec<bool>) {
        let [rows, columns] = periodic_shares(luminance, self);
        rows.iter()
            .zip(&columns)
            .map(|(&r, &c)| {
                let strength = ((r.max(c) - SHARE.0) / (SHARE.1 - SHARE.0)).clamp(0., 1.);
                (strength, r >= c)
            })
            .unzip()
    }

    /// Sums over the (2rx + 1) × (2ry + 1) window: running sums along each row,
    /// then a running sum of whole rows down the columns.
    fn box_sum(&self, v: &[f32], rx: usize, ry: usize) -> Vec<f32> {
        let (w, h) = (self.w, self.h);
        let mut rows = vec![0f32; w * h];
        for y in 0..h {
            let (src, dst) = (&v[y * w..(y + 1) * w], &mut rows[y * w..(y + 1) * w]);
            let at = |x: isize| src[x.clamp(0, w as isize - 1) as usize];
            let mut s: f32 = (-(rx as isize)..=rx as isize).map(at).sum();
            for (x, d) in dst.iter_mut().enumerate() {
                *d = s;
                let x = x as isize;
                s += at(x + rx as isize + 1) - at(x - rx as isize);
            }
        }
        let row = |k: isize| {
            let k = k.clamp(0, h as isize - 1) as usize;
            &rows[k * w..(k + 1) * w]
        };
        let mut out = vec![0f32; w * h];
        let mut sum = vec![0f32; w];
        for k in -(ry as isize)..=ry as isize {
            for (s, v) in sum.iter_mut().zip(row(k)) {
                *s += v;
            }
        }
        for y in 0..h as isize {
            out[y as usize * w..(y as usize + 1) * w].copy_from_slice(&sum);
            for ((s, add), sub) in sum
                .iter_mut()
                .zip(row(y + ry as isize + 1))
                .zip(row(y - ry as isize))
            {
                *s += add - sub;
            }
        }
        out
    }
}

/// A Hann-windowed, zero-mean quadrature pair at one frequency.
struct Taps {
    cos: [f32; 2 * HALF_TAPS],
    sin: [f32; 2 * HALF_TAPS],
    window: [f32; 2 * HALF_TAPS],
    /// Response to a unit-variance sinusoid at the frequency, averaged over phase.
    norm: f32,
}

impl Taps {
    fn new(f: f32) -> Self {
        let n = 2 * HALF_TAPS;
        let pi = std::f32::consts::PI;
        let window: [f32; 2 * HALF_TAPS] =
            std::array::from_fn(|k| 0.5 - 0.5 * (2. * pi * (k as f32 + 0.5) / n as f32).cos());
        let angle = |k: isize, phase: f32| 2. * pi * f * k as f32 + phase;
        let offset = |k: usize| k as isize - HALF_TAPS as isize;
        let mut cos: [f32; 2 * HALF_TAPS] =
            std::array::from_fn(|k| angle(offset(k), 0.).cos() * window[k]);
        let mut sin: [f32; 2 * HALF_TAPS] =
            std::array::from_fn(|k| angle(offset(k), 0.).sin() * window[k]);
        // Without their mean the pair does not respond to flat luminance.
        let total: f32 = window.iter().sum();
        let (mc, ms) = (cos.iter().sum::<f32>(), sin.iter().sum::<f32>());
        for k in 0..n {
            cos[k] -= mc * window[k] / total;
            sin[k] -= ms * window[k] / total;
        }
        let mut taps = Self {
            cos,
            sin,
            window,
            norm: 1.,
        };
        taps.norm = (0..8)
            .map(|p| {
                let phase = p as f32 * pi / 4.;
                taps.energy(&std::array::from_fn(|k| {
                    2f32.sqrt() * angle(offset(k), phase).sin()
                }))
            })
            .sum::<f32>()
            / 8.;
        taps
    }

    /// The Hann-weighted variance of samples at offsets −[`HALF_TAPS`]..[`HALF_TAPS`].
    fn variance(&self, v: &[f32; 2 * HALF_TAPS]) -> f32 {
        let total: f32 = self.window.iter().sum();
        let mean = v.iter().zip(&self.window).map(|(x, w)| x * w).sum::<f32>() / total;
        v.iter()
            .zip(&self.window)
            .map(|(x, w)| w * (x - mean) * (x - mean))
            .sum::<f32>()
            / total
    }

    /// The pair's energy over samples at offsets −[`HALF_TAPS`]..[`HALF_TAPS`],
    /// relative to [`Taps::norm`].
    fn energy(&self, v: &[f32; 2 * HALF_TAPS]) -> f32 {
        let (mut c, mut s) = (0f32, 0f32);
        for ((x, tc), ts) in v.iter().zip(&self.cos).zip(&self.sin) {
            c += x * tc;
            s += x * ts;
        }
        (c * c + s * s) / self.norm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_luminance_stripe_border_still_loses_false_colour() {
        let (w, h) = (240, 192);
        let image: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let l = 0.1
                    + if (76..112).contains(&y) {
                        0.06 * (std::f32::consts::TAU * x as f32 / 3.03).sin()
                    } else {
                        0.
                    };
                let c = if y == 75 || y == 112 {
                    0.2 * (std::f32::consts::TAU * x as f32 / 15.).sin()
                } else {
                    0.
                };
                [l * (1. + c), l, l * (1. - c)]
            })
            .collect();
        let mut out = image;
        suppress(&mut out, w, h);
        let mut error = 0f32;
        for y in [75, 112] {
            for x in 64..176 {
                let p = out[y * w + x];
                error = error.max((p[0] - p[2]).abs() / 0.1);
            }
        }
        assert!(error < 0.03, "flat-luminance border colour: {error}");
    }

    #[test]
    fn stripe_mean_does_not_give_dark_noise_a_bright_vote() {
        let (w, h) = (240, 192);
        let image: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let l = 0.1 + 0.06 * (std::f32::consts::TAU * x as f32 / 3.03).sin();
                // A mesh continuing into deep shadow, with a red noise floor.
                let rod = if y % 17 < 2 { 0.7 } else { 1. };
                if x < 120 {
                    [l * rod; 3]
                } else {
                    [l * rod * 0.001 + 0.0002, l * rod * 0.001, l * rod * 0.001]
                }
            })
            .collect();
        let mut out = image;
        suppress(&mut out, w, h);
        let mut error = 0f32;
        for y in 64..128 {
            for x in 96..116 {
                let p = out[y * w + x];
                error = error.max((p[0] - p[1]).abs() / ((p[0] + 2. * p[1] + p[2]) * 0.25));
            }
        }
        assert!(error < 0.01, "shadow noise tinted lit mesh: {error}");
    }

    #[test]
    fn finite_stripe_edges_lose_false_colour_without_bleeding() {
        let (w, h) = (240, 192);
        let image: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if (76..112).contains(&y) {
                    let l = 0.1 + 0.06 * (std::f32::consts::TAU * x as f32 / 3.03).sin();
                    let c = 0.25 * (std::f32::consts::TAU * x as f32 / 15.).sin();
                    [l * (1. + c), l, l * (1. - c)]
                } else {
                    [0.06, 0.1, 0.14]
                }
            })
            .collect();
        let mut out = image;
        suppress(&mut out, w, h);
        let mut error = 0f32;
        for y in [76, 77, 78, 109, 110, 111] {
            let chroma: Vec<f32> = (64..176)
                .map(|x| {
                    let p = out[y * w + x];
                    (p[0] - p[2]) / ((p[0] + 2. * p[1] + p[2]) * 0.25)
                })
                .collect();
            let mean = chroma.iter().sum::<f32>() / chroma.len() as f32;
            for c in chroma {
                error = error.max((c - mean).abs());
            }
        }
        // Without coverage from the reliable core, the averaged mask leaves
        // oscillating chromaticity at the boundary.
        assert!(error < 0.03, "finite stripe edge chromaticity: {error}");
        for y in [60, 64, 124, 128] {
            for x in 64..176 {
                let p = out[y * w + x];
                assert!(
                    (p[0] - 0.06).abs() < 0.005 && (p[2] - 0.14).abs() < 0.005,
                    "window colour changed at {x},{y}: {p:?}"
                );
            }
        }
    }

    #[test]
    fn band_boundaries_do_not_change_the_filter() {
        let (w, h) = (180, 620);
        let image: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let l = 0.1 + 0.06 * (std::f32::consts::TAU * y / 3.03).sin();
                let c = 0.01 * (x / 17. + y / 53.).sin();
                [l + c, l, l - c]
            })
            .collect();
        let mut whole = image.clone();
        for (i, p) in band_changes(&image, w, h, 0, h) {
            whole[i] = p;
        }
        let mut bands = image;
        suppress(&mut bands, w, h);
        let error = whole
            .iter()
            .zip(&bands)
            .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (a - b).abs()))
            .fold(0f32, f32::max);
        assert!(error < 1e-5, "band boundary error: {error}");
    }
}
