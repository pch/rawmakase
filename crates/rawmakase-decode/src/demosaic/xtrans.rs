//! X-Trans demosaicing by directional candidates.
//!
//! X-Trans has no regular two-pixel greens, so one isotropic estimate turns fine
//! periodic detail (louvres, grilles, meshes) into colour moiré. Instead every
//! direction (horizontal, vertical and both diagonals) gets its own full RGB
//! candidate:
//!
//! 1. Green at red/blue sites by interpolating along the direction between the
//!    nearest greens, corrected by the same-colour curvature along that line.
//! 2. Red and blue from the difference to that direction's green, as on Bayer.
//! 3. Per pixel, the candidates are blended by how smooth their colour difference
//!    (R − G, B − G) is around it and how little the luminance changes along their
//!    direction. A correct direction leaves neutral detail neutral; a wrong one
//!    leaves a colour pattern, which scores badly. Detail runs along the correct
//!    direction, so luminance changes least along it; this decides where every
//!    candidate's colour is smooth, as on stripes too fine for the red and blue
//!    samples (X100F DSCF8231's tower).
//!
//! The image is processed in row bands with a mirrored margin so every window fits.
//! Stripes too fine for any direction to get their colour right then lose it
//! (`super::stripes`).
use super::Cfa;
use rayon::prelude::*;
use std::sync::OnceLock;

/// Rows of output per band: a multiple of the colour pattern's rows, so that every
/// band away from the image's top and bottom has the same colour layout and they
/// share one set of [`Side`] tables.
const BAND: usize = 3 * super::PATTERN;
const _: () = assert!(BAND.is_multiple_of(super::PATTERN));
/// Margin around a band: candidates are valid at [`CANDIDATE_EDGE`], then the
/// luminance change along a direction and its window take one and
/// [`LUMINANCE_RADIUS`] more (the colour variation's 3×3 window fits within).
const MARGIN: usize = CANDIDATE_EDGE + 1 + LUMINANCE_RADIUS;
/// Distance from the band edge at which candidates are valid: the green line search,
/// then the colour difference line search.
pub(crate) const CANDIDATE_EDGE: usize = (REACH + COLOUR_REACH) as usize;
/// How far along a line to look for a green.
const REACH: isize = 4;
/// How far along a line to look for a red or blue sample. Along a row or column of
/// X-Trans the same colour is up to five pixels away; with less, lines between two
/// such samples took the isotropic fallback across the detail, which turned stripes
/// about three pixels apart into colour bands.
const COLOUR_REACH: isize = 5;
/// Radius of the window over which luminance change along a direction is summed.
const LUMINANCE_RADIUS: usize = 2;
/// Horizontal, vertical and the two diagonals as (dx, dy).
const DIRECTIONS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];
/// One candidate per direction. An isotropic candidate (greens from all eight
/// neighbours, #523) is not among them: on fine two-dimensional detail such as a
/// mesh its colour is smooth but wrong, so it won the blend and brought the moiré
/// back (X100F DSCF8231's tower).
const CANDIDATES: usize = DIRECTIONS.len();
/// Floor of the colour variation and luminance change measures, in white-normalised
/// units, so that noise on a flat area does not pick a direction.
const VARIATION_FLOOR: f32 = 1e-3;
/// Sharpness of the blend: a candidate twice as rough as the smoothest, or whose
/// luminance changes twice as much as the least, weighs 1/16.
const BLEND_POWER: i32 = 4;
/// Share of the sensor's white from which samples count as clipped, as development
/// counts them.
const CLIPPED: f32 = 0.999;
/// Radius around clipped samples where the blend leaves out luminance change:
/// there colour differences are wrong, and the candidate it picked along bright
/// clipped lines, such as neon tubes, turned them green and jagged.
const CLIP_RADIUS: usize = 2;
/// Weight of the Hamilton–Adams-style curvature correction of the directional green.
const CURVATURE: f32 = 0.2;

pub(crate) fn demosaic(cfa: &Cfa) -> Vec<[f32; 3]> {
    let w = cfa.width;
    let mut out = vec![[0f32; 3]; w * cfa.height];
    let shared = OnceLock::new();
    let clip = cfa.white.map(|w| w * CLIPPED);
    out.par_chunks_mut(w * BAND)
        .enumerate()
        .for_each(|(band, rows)| band_candidates(cfa, band * BAND, rows, &shared, clip));
    super::stripes::suppress(&mut out, w, cfa.height);
    out
}

/// A band of sensor samples with its mirrored margin.
struct Band {
    width: usize,
    height: usize,
    raw: Vec<f32>,
    colour: Vec<u8>,
}

impl Band {
    fn new(cfa: &Cfa, y0: usize, rows: usize) -> Self {
        let width = cfa.width + 2 * MARGIN;
        let height = rows + 2 * MARGIN;
        let mut raw = Vec::with_capacity(width * height);
        let mut colour = Vec::with_capacity(width * height);
        for py in 0..height {
            let gy = Cfa::reflect(y0 as isize + py as isize - MARGIN as isize, cfa.height);
            for px in 0..width {
                let gx = Cfa::reflect(px as isize - MARGIN as isize, cfa.width);
                raw.push(cfa.data[gy * cfa.width + gx]);
                colour.push(cfa.color(gx, gy));
            }
        }
        Self {
            width,
            height,
            raw,
            colour,
        }
    }

    /// Nearest sample of `colour` within `reach` on one side of `i`, as (distance,
    /// value), from that side's distances.
    #[inline]
    fn along(&self, i: usize, side: &Side, colour: u8, reach: u8) -> Option<(f32, f32)> {
        let k = side.distance[colour as usize][i];
        (k != 0 && k <= reach).then(|| {
            (
                k as f32,
                self.raw[(i as isize + k as isize * side.step) as usize],
            )
        })
    }

    /// Green at `i` along one direction.
    fn green(&self, i: usize, sides: &[Side; 2]) -> f32 {
        let own = self.colour[i];
        if own == 1 {
            return self.raw[i];
        }
        let c = self.raw[i];
        let reach = REACH as u8;
        let along = |side: &Side, colour| self.along(i, side, colour, reach);
        let (fwd, back) = (along(&sides[0], 1), along(&sides[1], 1));
        let (mut est, lo, hi) = match (fwd, back) {
            (Some((a, ga)), Some((b, gb))) => ((ga * b + gb * a) / (a + b), ga.min(gb), ga.max(gb)),
            (Some((_, g)), None) | (None, Some((_, g))) => (g, g, g),
            (None, None) => return c,
        };
        // Same-colour curvature along the line, as the Hamilton–Adams correction.
        if let (Some((a, ca)), Some((b, cb))) = (along(&sides[0], own), along(&sides[1], own)) {
            est += CURVATURE * (c - (ca * b + cb * a) / (a + b));
        }
        est.clamp(lo, hi).max(0.)
    }
}

/// Demosaics one band into `out` (whole rows starting at image row `y0`).
fn band_candidates(
    cfa: &Cfa,
    y0: usize,
    out: &mut [[f32; 3]],
    shared: &OnceLock<Sides>,
    clip: [f32; 3],
) {
    let w = cfa.width;
    let rows = out.len() / w;
    let band = Band::new(cfa, y0, rows);
    let (pw, ph) = (band.width, band.height);
    let steps = DIRECTIONS.map(|(dx, dy)| dy * pw as isize + dx).into_iter();
    // Bands whose margin is all inside the image have the same colour layout.
    let inside = y0 >= MARGIN && rows == BAND && y0 + rows + MARGIN <= cfa.height;
    let own;
    let sides = if inside {
        shared.get_or_init(|| Side::all(&band))
    } else {
        own = Side::all(&band);
        &own
    };
    let candidates: Vec<_> = sides
        .iter()
        .map(|sides| {
            let rgb = candidate(&band, sides);
            let variation = window_sum(&variation(&rgb, pw, ph), 1, pw, ph);
            (rgb, variation)
        })
        .collect();
    let luminance = luminance(&candidates, pw, ph);
    let changes: Vec<_> = steps
        .map(|step| change_along(&luminance, step, pw, ph))
        .collect();
    let clipped: Vec<f32> = band
        .raw
        .iter()
        .zip(&band.colour)
        .map(|(&v, &c)| f32::from(u8::from(v >= clip[usize::from(c)])))
        .collect();
    let near_clipped = window_sum(&clipped, CLIP_RADIUS, pw, ph);
    for (r, row) in out.chunks_mut(w).enumerate() {
        for (x, px) in row.iter_mut().enumerate() {
            let i = (r + MARGIN) * pw + x + MARGIN;
            // On the stack: one allocation per pixel cost as much as the blend.
            let windows: [f32; CANDIDATES] =
                std::array::from_fn(|k| candidates[k].1[i] + VARIATION_FLOOR);
            let changes: [f32; CANDIDATES] =
                std::array::from_fn(|k| changes[k][i] + VARIATION_FLOOR);
            // Relative to the smoothest candidate and the least change, so a
            // candidate best by both weighs 1.
            let min = |v: &[f32; CANDIDATES]| v.iter().fold(f32::INFINITY, |a, b| a.min(*b));
            let (best, least) = (min(&windows), min(&changes));
            let by_luminance = near_clipped[i] == 0.;
            let mut sum = [0f32; 3];
            let mut total = 0.;
            for (((rgb, _), window), change) in candidates.iter().zip(windows).zip(changes) {
                let along = if by_luminance { least / change } else { 1. };
                let weight = (best / window * along).powi(BLEND_POWER);
                for c in 0..3 {
                    sum[c] += rgb[i][c] * weight;
                }
                total += weight;
            }
            *px = sum.map(|v| v / total);
        }
    }
}

/// Distances from every pixel along `step` to the nearest sample of each colour
/// within [`COLOUR_REACH`] (0 where there is none), so that the interpolation reads
/// them instead of searching the line for each pixel.
#[cfg_attr(test, derive(Debug, PartialEq))]
struct Side {
    step: isize,
    /// One plane per colour.
    distance: [Vec<u8>; 3],
}

/// Both sides of every direction, in [`DIRECTIONS`]' order.
type Sides = [[Side; 2]; CANDIDATES];

impl Side {
    fn all(band: &Band) -> Sides {
        let pw = band.width;
        DIRECTIONS.map(|(dx, dy)| {
            let step = dy * pw as isize + dx;
            [
                Side::new(&band.colour, step, pw),
                Side::new(&band.colour, -step, pw),
            ]
        })
    }

    fn new(colour: &[u8], step: isize, pw: usize) -> Self {
        let n = colour.len();
        let reach = COLOUR_REACH as u8;
        let mut distance = [vec![0u8; n], vec![0u8; n], vec![0u8; n]];
        // A pixel's distance follows from its neighbour's along `step`. Off the
        // horizontal the neighbour is in the next or previous row, so a whole row
        // follows from another, its pixels independent of each other.
        let dy = (step as f64 / pw as f64).round() as isize;
        let dx = step - dy * pw as isize;
        let xs = (-dx).max(0) as usize..pw - dx.max(0) as usize;
        let ph = n / pw;
        for (c, plane) in distance.iter_mut().enumerate() {
            let c = c as u8;
            if dy == 0 {
                for y in 0..ph {
                    let row = y * pw;
                    let mut fill = |x: usize| {
                        let j = (row + x) as isize + step;
                        let j = j as usize;
                        let d = plane[j];
                        plane[row + x] = if colour[j] == c {
                            1
                        } else if d != 0 && d < reach {
                            d + 1
                        } else {
                            0
                        };
                    };
                    if step > 0 {
                        xs.clone().rev().for_each(&mut fill)
                    } else {
                        xs.clone().for_each(&mut fill)
                    }
                }
            } else {
                let rows: Box<dyn Iterator<Item = usize>> = if dy > 0 {
                    Box::new((0..ph - 1).rev())
                } else {
                    Box::new(1..ph)
                };
                for y in rows {
                    let src = (y as isize + dy) as usize;
                    let (cur, next) = if dy > 0 {
                        let (a, b) = plane.split_at_mut(src * pw);
                        (&mut a[y * pw..(y + 1) * pw], &b[..pw])
                    } else {
                        let (a, b) = plane.split_at_mut(y * pw);
                        (&mut b[..pw], &a[src * pw..(src + 1) * pw])
                    };
                    let next_colour = &colour[src * pw..(src + 1) * pw];
                    let shift = |x: usize| (x as isize + dx) as usize;
                    for x in xs.clone() {
                        let (d, own) = (next[shift(x)], next_colour[shift(x)]);
                        cur[x] = if own == c {
                            1
                        } else if d != 0 && d < reach {
                            d + 1
                        } else {
                            0
                        };
                    }
                }
            }
        }
        Self { step, distance }
    }
}

/// Full RGB along the direction of `sides`, valid at distance [`CANDIDATE_EDGE`] from the band
/// edge.
fn candidate(band: &Band, sides: &[Side; 2]) -> Vec<[f32; 3]> {
    let (pw, ph) = (band.width, band.height);
    let mut green = vec![0f32; pw * ph];
    for y in REACH as usize..ph - REACH as usize {
        for x in REACH as usize..pw - REACH as usize {
            let i = y * pw + x;
            green[i] = band.green(i, sides);
        }
    }
    let mut rgb = vec![[0f32; 3]; pw * ph];
    for y in CANDIDATE_EDGE..ph - CANDIDATE_EDGE {
        for x in CANDIDATE_EDGE..pw - CANDIDATE_EDGE {
            let i = y * pw + x;
            let own = band.colour[i];
            rgb[i][1] = green[i];
            for c in [0u8, 2] {
                if own == c {
                    rgb[i][c as usize] = band.raw[i];
                    continue;
                }
                // Along the direction, between the nearest samples of the colour on
                // both sides, interpolating their difference to green.
                let line = |side: &Side| {
                    let k = side.distance[c as usize][i];
                    let j = (i as isize + k as isize * side.step) as usize;
                    (k != 0).then(|| (k as f32, band.raw[j] - green[j]))
                };
                if let (Some((a, da)), Some((b, db))) = (line(&sides[0]), line(&sides[1])) {
                    rgb[i][c as usize] = (green[i] + (da * b + db * a) / (a + b)).max(0.);
                    continue;
                }
                // Otherwise the nearest samples within radius 1, else 2, weighted by
                // inverse distance.
                let mut sum = 0.;
                let mut total = 0.;
                for radius in 1..=2isize {
                    for dy in -radius..=radius {
                        for dx in -radius..=radius {
                            let j = (i as isize + dy * pw as isize + dx) as usize;
                            if dx.abs().max(dy.abs()) != radius || band.colour[j] != c {
                                continue;
                            }
                            let weight = 1. / ((dx * dx + dy * dy) as f32).sqrt();
                            sum += (band.raw[j] - green[j]) * weight;
                            total += weight;
                        }
                    }
                    if total > 0. {
                        break;
                    }
                }
                rgb[i][c as usize] = (green[i] + if total > 0. { sum / total } else { 0. }).max(0.);
            }
        }
    }
    rgb
}

/// How much R − G and B − G change across each pixel, valid at distance
/// `CANDIDATE_EDGE + 1` from the band edge.
fn variation(rgb: &[[f32; 3]], pw: usize, ph: usize) -> Vec<f32> {
    let chroma = |i: usize| [rgb[i][0] - rgb[i][1], rgb[i][2] - rgb[i][1]];
    let mut out = vec![0f32; pw * ph];
    let edge = CANDIDATE_EDGE + 1;
    for y in edge..ph - edge {
        for x in edge..pw - edge {
            let i = y * pw + x;
            let (l, r, u, d) = (chroma(i - 1), chroma(i + 1), chroma(i - pw), chroma(i + pw));
            out[i] = (0..2)
                .map(|c| (l[c] - r[c]).abs() + (u[c] - d[c]).abs())
                .sum();
        }
    }
    out
}

/// Luminance as the candidates' mean green, which no direction biases, valid at
/// distance [`CANDIDATE_EDGE`] from the band edge.
fn luminance(candidates: &[(Vec<[f32; 3]>, Vec<f32>)], pw: usize, ph: usize) -> Vec<f32> {
    let mut out = vec![0f32; pw * ph];
    for (rgb, _) in candidates {
        for (l, p) in out.iter_mut().zip(rgb) {
            *l += p[1];
        }
    }
    out.iter_mut().for_each(|l| *l /= candidates.len() as f32);
    out
}

/// How much luminance changes along `step`, summed over the window of
/// [`LUMINANCE_RADIUS`] around each pixel, valid at distance [`MARGIN`] from the
/// band edge. Diagonal steps are longer, which favours the horizontal and vertical
/// candidates where no direction stands out.
fn change_along(luminance: &[f32], step: isize, pw: usize, ph: usize) -> Vec<f32> {
    let mut change = vec![0f32; pw * ph];
    let edge = CANDIDATE_EDGE + 1;
    for y in edge..ph - edge {
        for x in edge..pw - edge {
            let i = y * pw + x;
            change[i] = (luminance[(i as isize + step) as usize]
                - luminance[(i as isize - step) as usize])
                .abs();
        }
    }
    window_sum(&change, LUMINANCE_RADIUS, pw, ph)
}

/// `values` summed over the window of radius `r` around each pixel, valid at
/// distance [`MARGIN`] from the band edge where `values` is valid at `MARGIN - r`.
/// Rows first, then whole rows of those, so that both passes run over contiguous
/// memory; each window is summed on its own, so that the result does not depend
/// on where a band starts.
fn window_sum(values: &[f32], r: usize, pw: usize, ph: usize) -> Vec<f32> {
    let mut rows = vec![0f32; pw * ph];
    for y in MARGIN - r..ph - (MARGIN - r) {
        for x in MARGIN..pw - MARGIN {
            let i = y * pw + x;
            rows[i] = values[i - r..=i + r].iter().sum();
        }
    }
    let mut out = vec![0f32; pw * ph];
    for y in MARGIN..ph - MARGIN {
        let sums = &mut out[y * pw..(y + 1) * pw];
        for row in rows[(y - r) * pw..(y + r + 1) * pw].chunks_exact(pw) {
            for (sum, v) in sums.iter_mut().zip(row) {
                *sum += v;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demosaic::PATTERN;

    #[test]
    fn bands_inside_the_image_share_their_colour_layout() {
        // What sharing one set of distance tables across bands rests on.
        const X: [[u8; 6]; 6] = [
            [1, 1, 0, 1, 1, 2],
            [1, 1, 2, 1, 1, 0],
            [2, 0, 1, 0, 2, 1],
            [1, 1, 2, 1, 1, 0],
            [1, 1, 0, 1, 1, 2],
            [0, 2, 1, 2, 0, 1],
        ];
        let pattern: [u8; PATTERN * PATTERN] =
            std::array::from_fn(|i| X[(i / PATTERN) % 6][(i % PATTERN) % 6]);
        let (w, h) = (40, 4 * BAND + 7);
        let data: Vec<f32> = (0..w * h).map(|i| (i % 97) as f32 / 97.).collect();
        let cfa = Cfa {
            data: &data,
            width: w,
            height: h,
            pattern: &pattern,
            white: [1.; 3],
        };
        let [first, second] = [BAND, 2 * BAND].map(|y0| Band::new(&cfa, y0, BAND));
        assert_eq!(first.colour, second.colour);
        assert_eq!(Side::all(&first), Side::all(&second));
    }
}
