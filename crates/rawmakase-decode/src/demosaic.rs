//! RAWmakase's own demosaicing of unpacked sensor data.
//!
//! Input is the visible-area CFA, black-subtracted, normalized to white and white
//! balanced, with a 48×48 colour pattern (0 red, 1 green, 2 blue), which covers Bayer
//! (2 or 16 pixel) and X-Trans (6 pixel) periods. Both Bayer and
//! X-Trans use the same two passes:
//!
//! 1. Green at red/blue sites, interpolated along the direction of least gradient
//!    (Hamilton–Adams with Laplacian correction on Bayer, where the colour two pixels
//!    away is the same; gradient-weighted neighbours on X-Trans).
//! 2. Red and blue everywhere from the colour difference to green (R − G, B − G) of
//!    the nearest same-colour samples, which keeps edges aligned across channels.
use rayon::prelude::*;

mod stripes;
mod xtrans;
#[cfg(test)]
pub(crate) use xtrans::demosaic as xtrans_demosaic;

/// Side of the colour pattern: a common multiple of the 2, 6 and 16 pixel periods.
pub(crate) const PATTERN: usize = 48;

pub(crate) struct Cfa<'a> {
    pub data: &'a [f32],
    pub width: usize,
    pub height: usize,
    pub pattern: &'a [u8; PATTERN * PATTERN],
    /// Per colour, the level of a sample at the sensor's white: its white balance.
    pub white: [f32; 3],
}
impl Cfa<'_> {
    #[inline]
    fn color(&self, x: usize, y: usize) -> u8 {
        self.pattern[(y % PATTERN) * PATTERN + x % PATTERN]
    }
    /// Mirrors coordinates at the borders, which keeps the Bayer colour parity.
    #[inline]
    fn reflect(v: isize, n: usize) -> usize {
        let n = n as isize;
        let v = if v < 0 { -v } else { v };
        let v = if v >= n { 2 * (n - 1) - v } else { v };
        v.clamp(0, n - 1) as usize
    }
    #[inline]
    fn at(&self, x: isize, y: isize) -> f32 {
        let (x, y) = (Self::reflect(x, self.width), Self::reflect(y, self.height));
        self.data[y * self.width + x]
    }
    #[inline]
    fn color_at(&self, x: isize, y: isize) -> u8 {
        let (x, y) = (Self::reflect(x, self.width), Self::reflect(y, self.height));
        self.color(x, y)
    }
    /// A Bayer pattern repeats every two pixels with two greens per 2×2 block.
    fn is_bayer(&self) -> bool {
        (0..PATTERN).all(|y| (0..PATTERN).all(|x| self.color(x, y) == self.color(x % 2, y % 2)))
            && (0..2)
                .flat_map(|y| (0..2).map(move |x| (x, y)))
                .filter(|(x, y)| self.color(*x, *y) == 1)
                .count()
                == 2
    }
}

/// A half-size image of ⌈W/2⌉ × ⌈H/2⌉ pixels, for drafts: pixel (i, j) is
/// the 2 × 2 block of the mosaic at (2i, 2j), reaching one pixel back where
/// an odd side leaves a single row or column. A Bayer block gives its red,
/// its blue and the mean of its greens; another pattern is demosaiced in full
/// and the block's pixels averaged.
pub(crate) fn half(cfa: &Cfa) -> (usize, usize, Vec<[f32; 3]>) {
    let (w, h) = (cfa.width.div_ceil(2), cfa.height.div_ceil(2));
    // The block's corner, inside the image.
    let start = |i: usize, n: usize| (2 * i).min(n.saturating_sub(2));
    let one = |n: usize| n < 2;
    let pixels = if cfa.is_bayer() && !one(cfa.width) && !one(cfa.height) {
        (0..w * h)
            .into_par_iter()
            .map(|k| {
                let (x0, y0) = (start(k % w, cfa.width), start(k / w, cfa.height));
                let mut sum = [0f32; 3];
                for y in y0..y0 + 2 {
                    for x in x0..x0 + 2 {
                        sum[usize::from(cfa.color(x, y))] += cfa.data[y * cfa.width + x];
                    }
                }
                [sum[0].max(0.), (sum[1] / 2.).max(0.), sum[2].max(0.)]
            })
            .collect()
    } else {
        let full = demosaic(cfa);
        let span = |n: usize| if n < 2 { 1 } else { 2 };
        let (sx, sy) = (span(cfa.width), span(cfa.height));
        (0..w * h)
            .into_par_iter()
            .map(|k| {
                let (x0, y0) = (start(k % w, cfa.width), start(k / w, cfa.height));
                let mut sum = [0f32; 3];
                for y in y0..y0 + sy {
                    for x in x0..x0 + sx {
                        let p = full[y * cfa.width + x];
                        for c in 0..3 {
                            sum[c] += p[c];
                        }
                    }
                }
                sum.map(|v| v / (sx * sy) as f32)
            })
            .collect()
    };
    (w, h, pixels)
}

pub(crate) fn demosaic(cfa: &Cfa) -> Vec<[f32; 3]> {
    if cfa.is_bayer() {
        // Samples below black stay negative through interpolation and are clipped
        // here: an interpolated sample averages its neighbours' noise, so the
        // mean of a dark noisy area rises less than clipping each sample would
        // (Camera Raw's dark noisy patches sit at that lower bias).
        let mut out = colour_difference(cfa, &green_bayer(cfa));
        for px in &mut out {
            for v in px {
                *v = v.max(0.);
            }
        }
        out
    } else {
        let data: Vec<f32> = cfa.data.iter().map(|v| v.max(0.)).collect();
        xtrans::demosaic(&Cfa {
            data: &data,
            ..*cfa
        })
    }
}

fn green_bayer(cfa: &Cfa) -> Vec<f32> {
    let (w, h) = (cfa.width, cfa.height);
    let mut g = vec![0f32; w * h];
    g.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let y = y as isize;
        for (x, out) in row.iter_mut().enumerate() {
            let xi = x as isize;
            let c = cfa.at(xi, y);
            if cfa.color(x, y as usize) == 1 {
                *out = c;
                continue;
            }
            let (l, r) = (cfa.at(xi - 1, y), cfa.at(xi + 1, y));
            let (u, d) = (cfa.at(xi, y - 1), cfa.at(xi, y + 1));
            let lap_h = 2. * c - cfa.at(xi - 2, y) - cfa.at(xi + 2, y);
            let lap_v = 2. * c - cfa.at(xi, y - 2) - cfa.at(xi, y + 2);
            let gh = (l + r) * 0.5 + lap_h * 0.25;
            let gv = (u + d) * 0.5 + lap_v * 0.25;
            let dh = (l - r).abs() + lap_h.abs();
            let dv = (u - d).abs() + lap_v.abs();
            // Blend by inverse gradient instead of a hard switch, avoiding zipper edges.
            let (wh, wv) = (1. / (dh + 1e-5), 1. / (dv + 1e-5));
            let est = (gh * wh + gv * wv) / (wh + wv);
            // Keep the estimate within the neighbouring greens (no overshoot halos).
            let lo = l.min(r).min(u).min(d);
            let hi = l.max(r).max(u).max(d);
            *out = est.clamp(lo, hi);
        }
    });
    g
}

fn colour_difference(cfa: &Cfa, green: &[f32]) -> Vec<[f32; 3]> {
    let (w, h) = (cfa.width, cfa.height);
    let g = |x: isize, y: isize| green[Cfa::reflect(y, h) * w + Cfa::reflect(x, w)];
    let mut out = vec![[0f32; 3]; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let yi = y as isize;
        for (x, px) in row.iter_mut().enumerate() {
            let xi = x as isize;
            let gc = g(xi, yi);
            let own = cfa.color(x, y);
            px[1] = gc;
            for c in [0u8, 2] {
                if own == c {
                    px[c as usize] = cfa.at(xi, yi);
                    continue;
                }
                // Nearest same-colour samples within radius 1, else 2, weighted by
                // inverse distance, interpolating their difference to green.
                let mut sum = 0.;
                let mut total = 0.;
                for radius in 1..=2isize {
                    for dy in -radius..=radius {
                        for dx in -radius..=radius {
                            if dx.abs().max(dy.abs()) != radius
                                || cfa.color_at(xi + dx, yi + dy) != c
                            {
                                continue;
                            }
                            let wgt = 1. / ((dx * dx + dy * dy) as f32).sqrt();
                            sum += (cfa.at(xi + dx, yi + dy) - g(xi + dx, yi + dy)) * wgt;
                            total += wgt;
                        }
                    }
                    if total > 0. {
                        break;
                    }
                }
                px[c as usize] = gc + if total > 0. { sum / total } else { 0. };
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bayer_pattern() -> [u8; PATTERN * PATTERN] {
        std::array::from_fn(|i| match ((i / PATTERN) % 2, i % 2) {
            (0, 0) => 0,
            (1, 1) => 2,
            _ => 1,
        })
    }
    fn xtrans_pattern() -> [u8; PATTERN * PATTERN] {
        const X: [[u8; 6]; 6] = [
            [1, 1, 0, 1, 1, 2],
            [1, 1, 2, 1, 1, 0],
            [2, 0, 1, 0, 2, 1],
            [1, 1, 2, 1, 1, 0],
            [1, 1, 0, 1, 1, 2],
            [0, 2, 1, 2, 0, 1],
        ];
        std::array::from_fn(|i| X[(i / PATTERN) % 6][(i % PATTERN) % 6])
    }
    fn mosaic(
        pattern: &[u8; PATTERN * PATTERN],
        w: usize,
        h: usize,
        f: impl Fn(usize, usize) -> [f32; 3],
    ) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                f(i % w, i / w)[pattern[((i / w) % PATTERN) * PATTERN + (i % w) % PATTERN] as usize]
            })
            .collect()
    }
    #[test]
    fn flat_colour_is_reconstructed_exactly() {
        for pattern in [bayer_pattern(), xtrans_pattern()] {
            let data = mosaic(&pattern, 24, 18, |_, _| [0.4, 0.3, 0.2]);
            let cfa = Cfa {
                data: &data,
                width: 24,
                height: 18,
                pattern: &pattern,
                white: [1.; 3],
            };
            for p in demosaic(&cfa) {
                assert!(
                    (p[0] - 0.4).abs() < 1e-5
                        && (p[1] - 0.3).abs() < 1e-5
                        && (p[2] - 0.2).abs() < 1e-5,
                    "{p:?}"
                );
            }
        }
    }
    #[test]
    fn gray_edges_stay_gray_and_sharp() {
        for pattern in [bayer_pattern(), xtrans_pattern()] {
            let data = mosaic(
                &pattern,
                32,
                24,
                |x, _| if x < 16 { [0.1; 3] } else { [0.8; 3] },
            );
            let cfa = Cfa {
                data: &data,
                width: 32,
                height: 24,
                pattern: &pattern,
                white: [1.; 3],
            };
            let out = demosaic(&cfa);
            for y in 4..20 {
                for x in 2..30 {
                    let p = out[y * 32 + x];
                    // Away from the edge the colour is exact; at the edge no strong colour fringe.
                    let spread = p.iter().fold(0f32, |a, b| a.max(*b))
                        - p.iter().fold(1f32, |a, b| a.min(*b));
                    assert!(spread < 0.25, "{x},{y}: {p:?}");
                    if !(13..=18).contains(&x) {
                        assert!(spread < 1e-4, "{x},{y}: {p:?}");
                    }
                }
            }
        }
    }
    #[test]
    fn fine_neutral_stripes_stay_neutral_on_xtrans() {
        let pattern = xtrans_pattern();
        let (w, h) = (96, 96);
        // Louvres about as fine as the sensor grid, horizontal and diagonal.
        let stripes: [fn(usize, usize) -> f32; 2] = [
            |_, y| [0.8, 0.2, 0.2][y % 3],
            |x, y| [0.8, 0.2, 0.2][(x + y) % 3],
        ];
        for stripe in stripes {
            let data = mosaic(&pattern, w, h, |x, y| [stripe(x, y); 3]);
            let cfa = Cfa {
                data: &data,
                width: w,
                height: h,
                pattern: &pattern,
                white: [1.; 3],
            };
            let out = demosaic(&cfa);
            let mut worst = 0f32;
            for y in 12..h - 12 {
                for x in 12..w - 12 {
                    let p = out[y * w + x];
                    worst = worst.max((p[0] - p[1]).abs()).max((p[2] - p[1]).abs());
                }
            }
            assert!(worst < 0.08, "colour cast {worst}");
        }
    }
    /// `scene` sampled over each pixel's area, as a lens would.
    fn area_sampled(scene: impl Fn(f32, f32) -> [f32; 3]) -> impl Fn(usize, usize) -> [f32; 3] {
        move |x, y| {
            let mut sum = [0f32; 3];
            for sy in 0..4 {
                for sx in 0..4 {
                    let (fx, fy) = ((sx as f32 + 0.5) / 4., (sy as f32 + 0.5) / 4.);
                    let p = scene(x as f32 + fx, y as f32 + fy);
                    for c in 0..3 {
                        sum[c] += p[c] / 16.;
                    }
                }
            }
            sum
        }
    }
    /// The mean false colour of 6 × 6 blocks, where moiré shows as bands: how far
    /// each block's R − G and B − G are from the scene's.
    fn colour_bands(
        out: &[[f32; 3]],
        (w, h): (usize, usize),
        truth: impl Fn(usize, usize) -> [f32; 3],
    ) -> f32 {
        let mut casts = Vec::new();
        for by in (12..h - 18).step_by(6) {
            for bx in (12..w - 18).step_by(6) {
                let mut c = [0f32; 2];
                for y in by..by + 6 {
                    for x in bx..bx + 6 {
                        let (p, t) = (out[y * w + x], truth(x, y));
                        c[0] += (p[0] - p[1]) - (t[0] - t[1]);
                        c[1] += (p[2] - p[1]) - (t[2] - t[1]);
                    }
                }
                casts.push((c[0] / 36.).abs().max((c[1] / 36.).abs()));
            }
        }
        casts.iter().sum::<f32>() / casts.len() as f32
    }
    #[test]
    fn coloured_stripes_finer_than_red_and_blue_stay_free_of_bands_on_xtrans() {
        // Warm slats and bluish gaps about three pixels apart, nearly upright, like
        // the mesh on X100F DSCF8231's tower. Their colour alternates faster than the
        // red and blue samples, so only the candidate along the slats gets it right;
        // the others leave green and magenta bands that are smooth across a 3 × 3
        // window, which the colour variation alone could not tell from real colour.
        let pattern = xtrans_pattern();
        let (w, h) = (192, 192);
        let truth = area_sampled(|x, y| {
            if ((x + 0.004 * y) / 3.03).fract() < 0.5 {
                [0.13, 0.12, 0.10]
            } else {
                [0.04, 0.05, 0.065]
            }
        });
        let data = mosaic(&pattern, w, h, &truth);
        let cfa = Cfa {
            data: &data,
            width: w,
            height: h,
            pattern: &pattern,
            white: [1.; 3],
        };
        let bands = colour_bands(&demosaic(&cfa), (w, h), &truth);
        // Measured band error 0.000242; a four-pixel colour reach leaves bands
        // above the limit because it falls back across the slats.
        assert!(bands < 0.0005, "colour bands: mean {bands}");
    }
    #[test]
    fn faint_coloured_stripes_in_noise_stay_free_of_bands_on_xtrans() {
        // Faint warm slats and bluish gaps about three pixels apart under sensor
        // noise, like the lower mesh on X100F DSCF8231's tower. Against all of the
        // local variance, noise included, the stripes' share fell short and their
        // bands stayed; along lines averaged across the stripes, noise mostly
        // cancels.
        let pattern = xtrans_pattern();
        let (w, h) = (192, 192);
        let truth = area_sampled(|x, y| {
            if ((x + 0.004 * y) / 3.03).fract() < 0.5 {
                [0.095, 0.09, 0.08]
            } else {
                [0.06, 0.065, 0.075]
            }
        });
        let mut data = mosaic(&pattern, w, h, &truth);
        // Deterministic noise of about 0.005 (a sum of uniform values).
        let mut state = 0x2545_f491_4f6c_dd1du64;
        for v in &mut data {
            let mut n = 0.;
            for _ in 0..4 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                n += (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
            }
            *v += n * 0.0087;
        }
        let cfa = Cfa {
            data: &data,
            width: w,
            height: h,
            pattern: &pattern,
            white: [1.; 3],
        };
        let bands = colour_bands(&demosaic(&cfa), (w, h), &truth);
        // Measured band error 0.001015. Measuring unaveraged lines lets sensor
        // noise hide the stripes instead of suppressing their false colour.
        assert!(bands < 0.0015, "colour bands: mean {bands}");
    }
    #[test]
    fn coarser_coloured_detail_keeps_its_colour_on_xtrans() {
        // Red and blue stripes eight pixels apart, which the samples resolve, and a
        // dark edge with a two-pixel green fringe, like a lens's colour fringe:
        // neither is taken for stripes too fine for their colour.
        let pattern = xtrans_pattern();
        let (w, h) = (192, 192);
        let truth = area_sampled(|x, y| {
            if y < 96. {
                if (x / 8.).fract() < 0.5 {
                    [0.30, 0.10, 0.08]
                } else {
                    [0.06, 0.10, 0.28]
                }
            } else if x < 96. {
                [0.03; 3]
            } else if x < 98. {
                [0.10, 0.30, 0.10]
            } else {
                [0.40; 3]
            }
        });
        let data = mosaic(&pattern, w, h, &truth);
        let cfa = Cfa {
            data: &data,
            width: w,
            height: h,
            pattern: &pattern,
            white: [1.; 3],
        };
        let out = demosaic(&cfa);
        let mut kept = [0f32; 2];
        for (k, (ys, xs)) in [(20..76, 20..172), (110..180, 96..98)]
            .into_iter()
            .enumerate()
        {
            let (mut got, mut want) = (0f32, 0f32);
            for y in ys {
                for x in xs.clone() {
                    let (p, t) = (out[y * w + x], truth(x, y));
                    got += (p[0] - p[1]).abs() + (p[2] - p[1]).abs();
                    want += (t[0] - t[1]).abs() + (t[2] - t[1]).abs();
                }
            }
            kept[k] = got / want;
        }
        assert!(kept.iter().all(|&k| k > 0.8), "colour kept: {kept:?}");
    }
    #[test]
    fn colour_changing_along_fine_stripes_is_kept_on_xtrans() {
        // Neutral stripes about three pixels apart, the period whose colour the
        // samples cannot tell, on a surface whose colour changes along them every
        // 24 pixels, which the directional demosaic resolves: only colour across
        // the stripes is replaced by its mean.
        let pattern = xtrans_pattern();
        let (w, h) = (192, 288);
        let truth = area_sampled(|x, y| {
            let l = 0.16 + 0.07 * (std::f32::consts::TAU * x / 3.03).sin();
            let c = 0.04 * (std::f32::consts::TAU * y / 24.).sin();
            [l + c, l, l - c]
        });
        let data = mosaic(&pattern, w, h, &truth);
        let cfa = Cfa {
            data: &data,
            width: w,
            height: h,
            pattern: &pattern,
            white: [1.; 3],
        };
        let out = demosaic(&cfa);
        // The spread of each row's mean R − B, against the scene's.
        let spread = |f: &dyn Fn(usize, usize) -> f32| {
            let rows: Vec<f32> = (40..h - 40)
                .map(|y| (40..w - 40).map(|x| f(x, y)).sum::<f32>() / (w - 80) as f32)
                .collect();
            let mean = rows.iter().sum::<f32>() / rows.len() as f32;
            (rows.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / rows.len() as f32).sqrt()
        };
        let got = spread(&|x, y| out[y * w + x][0] - out[y * w + x][2]);
        let want = spread(&|x, y| {
            let t = truth(x, y);
            t[0] - t[2]
        });
        // Measured retained amplitude 0.746. An unconditional area mean erases
        // this resolved colour variation along the slats.
        assert!(got / want > 0.65, "colour kept: {}", got / want);
    }
    #[test]
    fn stripes_colour_stays_within_the_colours_it_averages() {
        // Small patches of fine stripes, along the rows and down the columns, in
        // noise: points near them can be weighted while nothing of their own axis
        // counts, and a mean taken over the rounding left in empty running sums
        // made a pixel thousands of times too bright.
        let (w, h) = (400, 600);
        let mut seed = 1u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        let image: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let wave = |v: usize| 0.08 * (std::f32::consts::TAU * v as f32 / 3.03).sin();
                let l = 0.1
                    + match ((x / 20) % 4, (y / 20) % 4) {
                        (0, 0) => wave(x),
                        (2, 2) => wave(y),
                        _ => 0.,
                    };
                [l + 0.03 * noise(), l + 0.03 * noise(), l + 0.03 * noise()]
            })
            .collect();
        let mut out = image.clone();
        stripes::suppress(&mut out, w, h);
        let chroma = |p: &[f32; 3]| [p[0] - p[1], p[2] - p[1]];
        let (lo, hi) = image.iter().map(chroma).fold(
            ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
            |(lo, hi), c| {
                (
                    [lo[0].min(c[0]), lo[1].min(c[1])],
                    [hi[0].max(c[0]), hi[1].max(c[1])],
                )
            },
        );
        assert_ne!(out, image, "the stripes are suppressed");
        for p in &out {
            let c = chroma(p);
            assert!(
                (0..2).all(|k| c[k] >= lo[k] - 1e-4 && c[k] <= hi[k] + 1e-4),
                "{p:?} outside {lo:?}..{hi:?}"
            );
        }
    }
    #[test]
    fn fine_stripes_keep_constant_chromaticity() {
        // One uniformly coloured material under alternating light and shade.
        // Constant channel differences give the dark slats a stronger tint than
        // the bright ones, even when the input has no false colour at all.
        let (w, h) = (192, 192);
        let image: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let l = 0.1 + 0.07 * (std::f32::consts::TAU * (i % w) as f32 / 3.03).sin();
                [1.08 * l, l, 0.92 * l]
            })
            .collect();
        let mut out = image;
        stripes::suppress(&mut out, w, h);
        let mut error = 0f32;
        for y in 40..h - 40 {
            for x in 40..w - 40 {
                let p = out[y * w + x];
                error = error.max((p[0] / p[1] - 1.08).abs());
                error = error.max((p[2] / p[1] - 0.92).abs());
            }
        }
        assert!(error < 0.001, "chromaticity error: {error}");
    }
    #[test]
    fn saturated_fine_stripes_keep_their_brightness_on_xtrans() {
        // Saturated red stripes about three pixels apart: their colour's mean,
        // put on a dark stripe, would need a negative channel, and clipping it
        // added light (5% on average).
        let pattern = xtrans_pattern();
        let (w, h) = (192, 288);
        let truth = area_sampled(|x, _| {
            let a = 0.11 + 0.10 * (std::f32::consts::TAU * x / 3.03).sin();
            [2. * a, 0.3 * a, 0.1 * a]
        });
        let data = mosaic(&pattern, w, h, &truth);
        let cfa = Cfa {
            data: &data,
            width: w,
            height: h,
            pattern: &pattern,
            white: [1.; 3],
        };
        let out = demosaic(&cfa);
        let luminance = |p: [f32; 3]| (p[0] + 2. * p[1] + p[2]) / 4.;
        let (mut got, mut want) = (0f32, 0f32);
        for y in 40..h - 40 {
            for x in 40..w - 40 {
                got += luminance(out[y * w + x]);
                want += luminance(truth(x, y));
            }
        }
        assert!((got / want - 1.).abs() < 0.01, "brightness {}", got / want);
    }
    #[test]
    fn a_fine_mesh_stays_neutral_on_xtrans() {
        // Slats about 3 pixels apart crossed by rods about 4 pixels apart, like the
        // mesh on X100F DSCF8231's tower, area-sampled as a lens would. With an
        // isotropic candidate its smooth but wrong colour won the blend: green and
        // magenta bands six pixels or more across.
        let pattern = xtrans_pattern();
        let (w, h) = (192, 192);
        let scene = area_sampled(|x, y| {
            if (x / 4.1).fract() < 0.3 {
                [0.04; 3]
            } else if ((y + 0.01 * x) / 3.05).fract() < 0.5 {
                [0.12; 3]
            } else {
                [0.05; 3]
            }
        });
        let data = mosaic(&pattern, w, h, &scene);
        let cfa = Cfa {
            data: &data,
            width: w,
            height: h,
            pattern: &pattern,
            white: [1.; 3],
        };
        let mean = colour_bands(&demosaic(&cfa), (w, h), &scene);
        // Measured band error 0.002175. An isotropic candidate can win on
        // smooth false colour instead of keeping the neutral mesh neutral.
        assert!(mean < 0.0023, "colour bands: mean {mean}");
    }
    #[test]
    fn extreme_samples_do_not_make_non_finite_pixels() {
        let pattern = xtrans_pattern();
        let (w, h) = (32, 40);
        let data = mosaic(&pattern, w, h, |x, y| {
            let v = if (x * 7 + y * 13) % 5 == 0 {
                65535.
            } else {
                0.
            };
            [v; 3]
        });
        let cfa = Cfa {
            data: &data,
            width: w,
            height: h,
            pattern: &pattern,
            white: [1.; 3],
        };
        assert!(demosaic(&cfa).iter().flatten().all(|v| v.is_finite()));
    }
    #[test]
    fn recognises_bayer_and_xtrans() {
        let b = bayer_pattern();
        let x = xtrans_pattern();
        assert!(
            Cfa {
                data: &[],
                width: 0,
                height: 0,
                pattern: &b,
                white: [1.; 3],
            }
            .is_bayer()
        );
        assert!(
            !Cfa {
                data: &[],
                width: 0,
                height: 0,
                pattern: &x,
                white: [1.; 3],
            }
            .is_bayer()
        );
    }
    #[test]
    fn half_size_keeps_colour_and_rounds_odd_sides_up() {
        // X-Trans demosaics exactly away from tiny images' borders only.
        for (pattern, sizes) in [
            (bayer_pattern(), vec![(24, 18), (25, 19), (3, 2)]),
            (xtrans_pattern(), vec![(24, 18), (25, 19)]),
        ] {
            for (w, h) in sizes {
                let data = mosaic(&pattern, w, h, |_, _| [0.4, 0.3, 0.2]);
                let cfa = Cfa {
                    data: &data,
                    width: w,
                    height: h,
                    pattern: &pattern,
                    white: [1.; 3],
                };
                let (hw, hh, pixels) = half(&cfa);
                assert_eq!(
                    (hw, hh, pixels.len()),
                    (w.div_ceil(2), h.div_ceil(2), hw * hh)
                );
                for p in pixels {
                    for (got, want) in p.iter().zip([0.4, 0.3, 0.2]) {
                        assert!((got - want).abs() < 1e-5, "{w}x{h}: {p:?}");
                    }
                }
            }
        }
    }
    #[test]
    fn half_size_bayer_is_the_block_it_covers() {
        // A ramp: each block's red is its top-left sample, its blue the
        // bottom-right, its green the mean of the other two.
        let pattern = bayer_pattern();
        let (w, h) = (6, 4);
        let data: Vec<f32> = (0..w * h).map(|i| i as f32).collect();
        let cfa = Cfa {
            data: &data,
            width: w,
            height: h,
            pattern: &pattern,
            white: [1.; 3],
        };
        let (_, _, pixels) = half(&cfa);
        assert_eq!(pixels[0], [0., (1. + 6.) / 2., 7.]);
        assert_eq!(pixels[4], [14., (15. + 20.) / 2., 21.]);
    }
}
