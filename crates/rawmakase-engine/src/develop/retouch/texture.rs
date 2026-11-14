//! A Generate operation's patch given the photo's fine texture, at every render,
//! once Heal's membrane has matched its tone ([`super::heal`]):
//!
//! - Detail. A patch is at most 2048 pixels on its long side, so where the photo
//!   has more pixels than the patch over its region (a large area of a large
//!   photo), the patch lacks the detail between its resolution and the photo's.
//!   Each painted pixel takes that detail from the surroundings. A
//!   nearest-neighbour field ([`Field`]), found once per patch and shape on the
//!   patch's own pixels, says which unpainted neighbourhood each painted one looks
//!   like: PatchMatch's search, from random guesses improved by what neighbours
//!   found and by random tries ever closer to the best so far. There, the photo less
//!   what the patch's resolution holds of it (its average over each patch pixel,
//!   interpolated as the patch is) is the detail. It is added in log values, so it
//!   scales with the patch's tone and leaves that tone as it was.
//! - Grain. A generator's fill is smoother than the photo. The noise in a band
//!   around the shape and in the fill is measured as the median absolute deviation
//!   of a 4-neighbour Laplacian, in square-root values, where shot noise is about
//!   constant. What the composite would lack is added as Gaussian noise drawn
//!   from the pixel's position and the patch, so every render of it is the same:
//!   more where the feather or opacity mixes the fill with the photo, since two
//!   independent noises mixed half and half have half the variance.
//!
//! Both read the image the patch is laid on, so noise reduction or an earlier spot
//! changes the detail and grain a patch takes, as it changes the membrane.
use super::heal::{self, DOMAIN, PixelRect, Placed};
use super::patch::PatchPixels;
use crate::camera_data::CameraImage;
use crate::model::image_frame::ImageFrame;
use crate::model::retouch::RetouchShape;
use rayon::prelude::*;

/// Photo pixels per patch pixel from which the patch lacks detail the photo has.
const DETAIL_FROM: f32 = 1.25;
/// How far around the shape the photo's grain is measured, in photo pixels.
pub(super) const BAND: i32 = 8;
/// The neighbourhoods the field compares: (2 `R` + 1)² cells.
const R: i32 = 3;
/// About the most cells the field is found over; a larger patch is searched on
/// cells of several of its pixels.
const MAX_CELLS: f32 = 32768.;
/// Rounds of the field's search.
const ROUNDS: u64 = 6;
/// The fewest pixels a measurement of grain is made from.
const MIN_SAMPLES: usize = 32;

/// Photo pixels per patch pixel where `patch` lies on the photo `frame` describes.
pub(super) fn photo_per_patch(rect: [f32; 4], width: u32, height: u32, frame: &ImageFrame) -> f32 {
    let [w, h] = frame.size();
    ((rect[2] - rect[0]) * w / width as f32).max((rect[3] - rect[1]) * h / height as f32)
}

/// The pixels a Generate operation reads for the detail it takes: the patch's
/// rectangle in decoded pixels, with room for what the patch's resolution holds of
/// the photo. `None` when the patch is as fine as the photo there.
pub(super) fn detail_reads(
    rect: [f32; 4],
    width: u32,
    height: u32,
    frame: &ImageFrame,
) -> Option<PixelRect> {
    let k = photo_per_patch(rect, width, height, frame);
    if k.is_nan() || k < DETAIL_FROM {
        return None;
    }
    let a = frame.to_source([rect[0], rect[1]]);
    let b = frame.to_source([rect[2], rect[3]]);
    let m = k.ceil() as i32 + 2;
    Some([
        a[0].min(b[0]).floor() as i32 - m,
        a[1].min(b[1]).floor() as i32 - m,
        a[0].max(b[0]).ceil() as i32 + m,
        a[1].max(b[1]).ceil() as i32 + m,
    ])
}

/// `out`, a Generate operation's healed patch over `grid` of `im` with coverage
/// `alpha`, given the photo's detail and grain.
pub(super) fn apply(
    im: &CameraImage,
    op: &Placed,
    grid: PixelRect,
    alpha: &[f32],
    out: &mut [[f32; 3]],
) {
    let Some((patch, frame)) = &op.patch else {
        return;
    };
    detail(im, op, patch, frame, grid, alpha, out);
    grain(im, op, patch, grid, alpha, out);
}

/// Where each painted pixel of a patch takes its detail from: for cells of `step`
/// × `step` patch pixels, the offset to the unpainted cell whose neighbourhood is
/// most like the painted cell's, in patch pixels. Unpainted cells have none.
pub(crate) struct Field {
    step: usize,
    width: usize,
    height: usize,
    offsets: Vec<Option<[f32; 2]>>,
}
impl Field {
    /// The field of `patch` painted by `shape` (`radius` of the long edge), on a
    /// photo whose width and height over its long edge are `aspect`.
    pub(super) fn new(
        patch: &PatchPixels,
        shape: &RetouchShape,
        radius: f32,
        aspect: [f32; 2],
    ) -> Self {
        let (pw, ph) = (patch.width as usize, patch.height as usize);
        let step = ((pw * ph) as f32 / MAX_CELLS).sqrt().ceil().max(1.) as usize;
        let (w, h) = (pw.div_ceil(step), ph.div_ceil(step));
        let none = || Self {
            step,
            width: w,
            height: h,
            offsets: vec![None; w * h],
        };
        // Each cell's mean log value.
        let mut values = vec![[0f32; 3]; w * h];
        let mut counts = vec![0f32; w * h];
        for y in 0..ph {
            for x in 0..pw {
                let k = (y / step) * w + x / step;
                let p = patch.pixels[y * pw + x];
                for c in 0..3 {
                    values[k][c] += DOMAIN.to(p[c]);
                }
                counts[k] += 1.;
            }
        }
        for (v, n) in values.iter_mut().zip(&counts) {
            *v = v.map(|v| v / n);
        }
        let painted = painted(patch, shape, radius, aspect, step, w, h);
        // A source is an unpainted cell whose whole neighbourhood lies in the patch,
        // clear of the painted area.
        let mut sums = vec![0u32; (w + 1) * (h + 1)];
        for y in 0..h {
            for x in 0..w {
                sums[(y + 1) * (w + 1) + x + 1] = u32::from(painted[y * w + x])
                    + sums[y * (w + 1) + x + 1]
                    + sums[(y + 1) * (w + 1) + x]
                    - sums[y * (w + 1) + x];
            }
        }
        let (wi, hi) = (w as i32, h as i32);
        let source: Vec<bool> = (0..w * h)
            .map(|k| {
                let (x, y) = ((k % w) as i32, (k / w) as i32);
                if x < R || y < R || x >= wi - R || y >= hi - R {
                    return false;
                }
                let at = |x: i32, y: i32| sums[y as usize * (w + 1) + x as usize];
                let (x0, y0, x1, y1) = (x - R, y - R, x + R + 1, y + R + 1);
                at(x1, y1) + at(x0, y0) - at(x0, y1) - at(x1, y0) == 0
            })
            .collect();
        let sources: Vec<u32> = (0..w * h)
            .filter(|k| source[*k])
            .map(|k| k as u32)
            .collect();
        let queries: Vec<usize> = (0..w * h).filter(|k| painted[*k]).collect();
        if sources.is_empty() || queries.is_empty() {
            return none();
        }
        let valid =
            |x: i32, y: i32| x >= 0 && y >= 0 && x < wi && y < hi && source[(y * wi + x) as usize];
        // Mean squared difference of the neighbourhoods of `q` and source `s`.
        let cost = |q: (i32, i32), s: (i32, i32)| {
            let (mut sum, mut n) = (0f32, 0f32);
            for dy in -R..=R {
                for dx in -R..=R {
                    let (qx, qy) = (q.0 + dx, q.1 + dy);
                    if qx < 0 || qy < 0 || qx >= wi || qy >= hi {
                        continue;
                    }
                    let a = values[(qy * wi + qx) as usize];
                    let b = values[((s.1 + dy) * wi + s.0 + dx) as usize];
                    sum += (0..3).map(|c| (a[c] - b[c]).powi(2)).sum::<f32>();
                    n += 1.;
                }
            }
            sum / n
        };
        let at = |k: usize| ((k % w) as i32, (k / w) as i32);
        // Each painted cell's source and its cost, by cell.
        let mut best: Vec<Option<((i32, i32), f32)>> = vec![None; w * h];
        for &q in &queries {
            let s = at(sources[(mix(q as u64) % sources.len() as u64) as usize] as usize);
            best[q] = Some((s, cost(at(q), s)));
        }
        let reach = w.max(h) as i32;
        for round in 0..ROUNDS {
            let next: Vec<((i32, i32), f32)> = queries
                .par_iter()
                .map(|&k| {
                    let q = at(k);
                    let mut found = best[k].expect("every painted cell has a source");
                    let consider = |s: (i32, i32), found: &mut ((i32, i32), f32)| {
                        if s != found.0 && valid(s.0, s.1) {
                            let c = cost(q, s);
                            if c < found.1 {
                                *found = (s, c);
                            }
                        }
                    };
                    // What a neighbour found, shifted by where it lies.
                    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (q.0 + dx, q.1 + dy);
                        if nx < 0 || ny < 0 || nx >= wi || ny >= hi {
                            continue;
                        }
                        if let Some(((sx, sy), _)) = best[(ny * wi + nx) as usize] {
                            consider((sx - dx, sy - dy), &mut found);
                        }
                    }
                    // Random tries around the best so far, ever closer.
                    let mut r = reach;
                    let mut t = 0u64;
                    while r >= 1 {
                        let h = mix(k as u64 ^ (round << 40) ^ (t << 52));
                        let span = (2 * r + 1) as u64;
                        let s = (
                            found.0.0 + (h % span) as i32 - r,
                            found.0.1 + ((h >> 32) % span) as i32 - r,
                        );
                        consider(s, &mut found);
                        r /= 2;
                        t += 1;
                    }
                    found
                })
                .collect();
            for (&k, found) in queries.iter().zip(next) {
                best[k] = Some(found);
            }
        }
        let mut offsets = vec![None; w * h];
        for &k in &queries {
            let (q, ((sx, sy), _)) = (at(k), best[k].expect("found"));
            offsets[k] = Some([
                ((sx - q.0) * step as i32) as f32,
                ((sy - q.1) * step as i32) as f32,
            ]);
        }
        Self {
            step,
            width: w,
            height: h,
            offsets,
        }
    }
    /// The offset for the patch pixel position `c` (pixel `i` spans `[i, i + 1)`).
    fn offset(&self, c: [f32; 2]) -> Option<[f32; 2]> {
        let x = ((c[0] / self.step as f32).floor() as i64).clamp(0, self.width as i64 - 1);
        let y = ((c[1] / self.step as f32).floor() as i64).clamp(0, self.height as i64 - 1);
        self.offsets[y as usize * self.width + x as usize]
    }
}

/// Which `w` × `h` cells of `step` patch pixels the shape paints, with a cell and a
/// half to spare: where the generator's fill is.
fn painted(
    patch: &PatchPixels,
    shape: &RetouchShape,
    radius: f32,
    aspect: [f32; 2],
    step: usize,
    w: usize,
    h: usize,
) -> Vec<bool> {
    let [x0, y0, x1, y1] = patch.rect;
    // Cell centres and the path, in long-edge units, to measure distances in.
    let cell = [
        (x1 - x0) * aspect[0] * step as f32 / patch.width as f32,
        (y1 - y0) * aspect[1] * step as f32 / patch.height as f32,
    ];
    let centre = |x: usize, y: usize| {
        [
            x0 * aspect[0] + (x as f32 + 0.5) * cell[0],
            y0 * aspect[1] + (y as f32 + 0.5) * cell[1],
        ]
    };
    let points: Vec<[f32; 2]> = match shape {
        RetouchShape::Spot { center, .. } => vec![*center],
        RetouchShape::Brush { points, .. } => points.to_vec(),
    }
    .into_iter()
    .map(|p| [p[0] * aspect[0], p[1] * aspect[1]])
    .collect();
    let reach = radius + 1.5 * cell[0].max(cell[1]);
    let mut out = vec![false; w * h];
    let segments: Vec<([f32; 2], [f32; 2])> = if points.len() == 1 {
        vec![(points[0], points[0])]
    } else {
        points.windows(2).map(|p| (p[0], p[1])).collect()
    };
    let index = |v: f32, origin: f32, size: f32, n: usize| {
        (((v - origin) / size).floor() as i64).clamp(0, n as i64) as usize
    };
    for (a, b) in segments {
        let cx0 = index(a[0].min(b[0]) - reach, x0 * aspect[0], cell[0], w);
        let cx1 = (index(a[0].max(b[0]) + reach, x0 * aspect[0], cell[0], w) + 1).min(w);
        let cy0 = index(a[1].min(b[1]) - reach, y0 * aspect[1], cell[1], h);
        let cy1 = (index(a[1].max(b[1]) + reach, y0 * aspect[1], cell[1], h) + 1).min(h);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        for y in cy0..cy1 {
            for x in cx0..cx1 {
                let p = centre(x, y);
                let (px, py) = (p[0] - a[0], p[1] - a[1]);
                let t = if len2 > 0. {
                    ((px * dx + py * dy) / len2).clamp(0., 1.)
                } else {
                    0.
                };
                if (px - t * dx).hypot(py - t * dy) < reach {
                    out[y * w + x] = true;
                }
            }
        }
    }
    out
}

/// Adds the photo's detail to `out` where `patch` is coarser than the photo.
fn detail(
    im: &CameraImage,
    op: &Placed,
    patch: &PatchPixels,
    frame: &ImageFrame,
    grid: PixelRect,
    alpha: &[f32],
    out: &mut [[f32; 3]],
) {
    let k = photo_per_patch(patch.rect, patch.width, patch.height, frame);
    if k.is_nan() || k < DETAIL_FROM {
        return;
    }
    let [w, h] = frame.size();
    let long = frame.long_edge();
    let field = patch.field(&op.shape, op.shape_radius, [w / long, h / long]);
    let [x0, y0, x1, y1] = patch.rect;
    let (pw, ph) = (patch.width as f32, patch.height as f32);
    // Patch pixel positions of image-frame positions, and back.
    let to_patch = |u: [f32; 2]| [(u[0] - x0) / (x1 - x0) * pw, (u[1] - y0) / (y1 - y0) * ph];
    let to_frame = |c: [f32; 2]| [x0 + c[0] / pw * (x1 - x0), y0 + c[1] / ph * (y1 - y0)];
    let gw = (grid[2] - grid[0]) as usize;
    let (iw, ih) = (im.width as i32, im.height as i32);
    // Each painted pixel's source pixel, and that pixel's position on the patch.
    let sources: Vec<(usize, usize, [f32; 2])> = (0..alpha.len())
        .into_par_iter()
        .filter(|i| alpha[*i] > 0.)
        .filter_map(|i| {
            let (x, y) = (grid[0] + (i % gw) as i32, grid[1] + (i / gw) as i32);
            let c = to_patch(frame.to_image(x as f32, y as f32));
            let o = field.offset(c)?;
            let s = frame.to_source(to_frame([c[0] + o[0], c[1] + o[1]]));
            // Whole pixels away, so the detail is the photo's own, not interpolated.
            let sx = (x + (s[0] - x as f32).round() as i32).clamp(0, iw - 1);
            let sy = (y + (s[1] - y as f32).round() as i32).clamp(0, ih - 1);
            let at = to_patch(frame.to_image(sx as f32, sy as f32));
            Some((i, sy as usize * im.width as usize + sx as usize, at))
        })
        .collect();
    if sources.is_empty() {
        return;
    }
    // What the patch's resolution holds of the photo: its average over each patch
    // pixel, as the request rendered it, over the patch pixels the sources need.
    let (mut lx, mut ly, mut hx, mut hy) = (usize::MAX, usize::MAX, 0, 0);
    let pixel = |v: f32, n: f32| (v - 0.5).floor().clamp(0., n - 1.) as usize;
    for (_, _, c) in &sources {
        let (ax, ay) = (pixel(c[0], pw), pixel(c[1], ph));
        lx = lx.min(ax);
        ly = ly.min(ay);
        hx = hx.max((ax + 1).min(patch.width as usize - 1));
        hy = hy.max((ay + 1).min(patch.height as usize - 1));
    }
    let (bw, bh) = (hx - lx + 1, hy - ly + 1);
    let taps = k.ceil().clamp(1., 16.) as usize;
    let coarse: Vec<[f32; 3]> = (0..bw * bh)
        .into_par_iter()
        .map(|i| {
            let (px, py) = ((lx + i % bw) as f32, (ly + i / bw) as f32);
            let mut sum = [0f32; 3];
            for ty in 0..taps {
                for tx in 0..taps {
                    let c = [
                        px + (tx as f32 + 0.5) / taps as f32,
                        py + (ty as f32 + 0.5) / taps as f32,
                    ];
                    let s = frame.to_source(to_frame(c));
                    let p = heal::bilinear(im, s[0], s[1]);
                    (0..3).for_each(|c| sum[c] += p[c]);
                }
            }
            sum.map(|v| DOMAIN.to(v / (taps * taps) as f32))
        })
        .collect();
    let low = |c: [f32; 2]| -> [f32; 3] {
        let x = (c[0] - 0.5).clamp(lx as f32, hx as f32) - lx as f32;
        let y = (c[1] - 0.5).clamp(ly as f32, hy as f32) - ly as f32;
        let (ix, iy) = (x as usize, y as usize);
        let (jx, jy) = ((ix + 1).min(bw - 1), (iy + 1).min(bh - 1));
        let (fx, fy) = (x - ix as f32, y - iy as f32);
        let v = |x: usize, y: usize| coarse[y * bw + x];
        let (a, b, c, d) = (v(ix, iy), v(jx, iy), v(ix, jy), v(jx, jy));
        std::array::from_fn(|i| {
            (a[i] * (1. - fx) + b[i] * fx) * (1. - fy) + (c[i] * (1. - fx) + d[i] * fx) * fy
        })
    };
    for (i, s, c) in sources {
        let photo = im.pixels[s];
        let low = low(c);
        out[i] = std::array::from_fn(|ch| {
            DOMAIN.from(DOMAIN.to(out[i][ch]) + DOMAIN.to(photo[ch]) - low[ch])
        });
    }
}

/// Adds to `out` the grain it lacks against the photo around the shape, so that
/// the composite, where coverage and opacity mix the fill with the photo, is as
/// grainy as the photo.
fn grain(
    im: &CameraImage,
    op: &Placed,
    patch: &PatchPixels,
    grid: PixelRect,
    alpha: &[f32],
    out: &mut [[f32; 3]],
) {
    let band = heal::clip(
        [
            grid[0] - BAND,
            grid[1] - BAND,
            grid[2] + BAND,
            grid[3] + BAND,
        ],
        im.width,
        im.height,
    );
    // The band: within `BAND` pixels of the shape's edge, outside its coverage.
    let ring = heal::coverage(&op.points, op.radius, op.feather, op.profile, band);
    let reach = heal::coverage(
        &op.points,
        op.radius + BAND as f32,
        0.,
        heal::FeatherProfile::Smoothstep,
        band,
    );
    let width = im.width as usize;
    let photo = residuals(band, |x, y| {
        let k = ((y - band[1]) * (band[2] - band[0]) + x - band[0]) as usize;
        (ring[k] <= 0. && reach[k] > 0.).then(|| im.pixels[y as usize * width + x as usize])
    });
    let gw = grid[2] - grid[0];
    let fill = residuals(grid, |x, y| {
        let i = ((y - grid[1]) * gw + x - grid[0]) as usize;
        (alpha[i] >= 0.99).then(|| out[i])
    });
    let (photo, fill): ([f32; 3], [f32; 3]) = (
        std::array::from_fn(|c| photo[c].unwrap_or(0.)),
        std::array::from_fn(|c| fill[c].unwrap_or(0.)),
    );
    if photo.iter().zip(&fill).all(|(p, f)| p <= f) {
        return;
    }
    let gw = gw as usize;
    out.par_iter_mut().enumerate().for_each(|(i, p)| {
        // The composite is `(1 - a)` of the photo and `a` of the fill, whose noise
        // is independent: its variance is σ_p² when the fill's is σ_p² (2 / a - 1).
        // Kept to coverage of 5% and more, where the factor is at most 39.
        let a = alpha[i] * op.opacity;
        if a <= 0. {
            return;
        }
        let gain = 2. / a.max(0.05) - 1.;
        let (x, y) = (grid[0] + (i % gw) as i32, grid[1] + (i / gw) as i32);
        for c in 0..3 {
            let lacking = (photo[c] * photo[c] * gain - fill[c] * fill[c])
                .max(0.)
                .sqrt();
            if lacking > 0. {
                let n = gaussian(patch.seed, x, y, c);
                p[c] = (p[c].max(0.).sqrt() + lacking * n).max(0.).powi(2);
            }
        }
    });
}

/// The noise of each channel over `rect`, as a Gaussian's σ in square-root values:
/// the median absolute deviation of the 4-neighbour Laplacian over the pixels that
/// `value` gives with all four of their neighbours. `None` with too few of them.
fn residuals(rect: PixelRect, value: impl Fn(i32, i32) -> Option<[f32; 3]>) -> [Option<f32>; 3] {
    let mut deviations: [Vec<f32>; 3] = Default::default();
    let root = |p: [f32; 3]| p.map(|v| v.max(0.).sqrt());
    for y in rect[1] + 1..rect[3] - 1 {
        for x in rect[0] + 1..rect[2] - 1 {
            let Some(p) = value(x, y) else { continue };
            let n = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)].map(|(x, y)| value(x, y));
            if n.iter().any(Option::is_none) {
                continue;
            }
            let (p, n) = (root(p), n.map(|n| root(n.expect("checked"))));
            for c in 0..3 {
                deviations[c].push((p[c] - (n[0][c] + n[1][c] + n[2][c] + n[3][c]) / 4.).abs());
            }
        }
    }
    deviations.map(|mut d| {
        if d.len() < MIN_SAMPLES {
            return None;
        }
        let mid = d.len() / 2;
        let (_, m, _) = d.select_nth_unstable_by(mid, f32::total_cmp);
        // A Gaussian's median absolute value is 0.6745 σ; the Laplacian of white
        // noise of σ has σ √1.25.
        Some(*m / 0.6745 / 1.25f32.sqrt())
    })
}

/// A standard Gaussian value for channel `c` of pixel (`x`, `y`) under `seed`.
fn gaussian(seed: u64, x: i32, y: i32, c: usize) -> f32 {
    let h = mix(seed ^ mix(((x as u32 as u64) << 32 | y as u32 as u64) ^ ((c as u64) << 62)));
    let u1 = ((h >> 40) as f32 + 0.5) / (1u64 << 24) as f32;
    let u2 = ((h & 0xff_ffff) as f32) / (1u64 << 24) as f32;
    (-2. * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}

/// SplitMix64's finaliser.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
