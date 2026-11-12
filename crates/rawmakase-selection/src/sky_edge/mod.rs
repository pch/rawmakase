//! The sky's edge decided by the photo's colours, in modes suited to different kinds
//! of horizon.
//!
//! The sky model works at a quarter of its input and was trained on crowns drawn as
//! whole polygons, so it calls the sky between branches, struts and around spires "not
//! sky". Near its answer's boundary each mode places pixels instead on the line between
//! the local sky colour and the local foreground colour, spread in from where the model
//! is sure; where those two colours cannot be told apart the model's answer stands.
//! [`dense`] takes only small departures from the model, the size of a gap in a truss or
//! a spire; [`sparse`] also takes a large area laced with twigs and joined to the sky
//! through them, as the sky behind a bare canopy is.
//!
//! Each mode returns coverage (0..=1) at the size it was given, before the final luma
//! guided filter.
//! [`blend`](fn@blend) keeps the model's answer, snapped to the photo's edges, and blends a mode
//! in only where the kind of horizon the model sees calls for it: a crown, or a lattice
//! it is sure of, never water.

use rawmakase_cancel::Cancel;

use crate::refine::mean;
use rawmakase_inference::InferenceError;

mod blend;
mod dense;
#[cfg(test)]
mod fixtures;
mod sparse;

pub(crate) use blend::{Modes, blend};

/// Model answers above this are sure sky, where the photo is also smooth.
const SURE_SKY: f32 = 0.9;
/// Model answers below this, far enough from the sky, are sure foreground.
const SURE_GROUND: f32 = 0.1;
/// How far from the model's sky (fraction of the long side) its answer is questioned.
const BAND: f32 = 0.15;
/// Side of the largest area the colours may decide against the model on their own, as
/// a fraction of the long side: about a gap between branches. Also the window of the
/// local spread.
const GATE: f32 = 0.015;
/// Colour spread (sRGB / 255, standard deviation) below which sure sky is smooth.
const UNIFORM: f32 = 0.04;
/// How far along the line from ground to sky a colour the model sees sky in must lie
/// to count as sky when the sky's own colour is measured.
const SKY_LIKE: f32 = 0.7;
/// Separation of the colours (sRGB / 255) over which the colours take over from the
/// model, from nothing to entirely.
const SEPARATION: (f32, f32) = (0.05, 0.15);
/// How far (pixels) a small departure's soft edge reaches.
const NEAR: usize = 2;

/// `Err(Cancelled)` once `cancel` is raised.
pub(super) fn cancelled(cancel: Cancel<'_>) -> Result<(), InferenceError> {
    if cancel.is_cancelled() {
        Err(InferenceError::Cancelled)
    } else {
        Ok(())
    }
}

/// Where `colour` lies on the line from `ground` (0) to `sky` (1), unclamped; 0 when
/// the two are the same colour.
fn position(colour: [f32; 3], sky: [f32; 3], ground: [f32; 3]) -> f32 {
    let length2 = (0..3).map(|k| (sky[k] - ground[k]).powi(2)).sum::<f32>();
    if length2 <= 0. {
        return 0.;
    }
    let dot = (0..3)
        .map(|k| (colour[k] - ground[k]) * (sky[k] - ground[k]))
        .sum::<f32>();
    dot / length2
}

/// 0 below `from`, 1 above `to`, smooth in between.
fn step(from: f32, to: f32, v: f32) -> f32 {
    let t = ((v - from) / (to - from)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// [`position`] clamped to 0..=1.
fn along(colour: [f32; 3], sky: [f32; 3], ground: [f32; 3]) -> f32 {
    position(colour, sky, ground).clamp(0., 1.)
}

/// Coverage where the colours decided: what is as sure as sure sky is all sky, and as
/// sure as sure ground none, so a darkened sky stays even; in between it stays
/// proportional, so a twig keeps its share. Where the model's answer stands, it is
/// narrowed as before ([`smooth`](crate::refine::smooth)), which would force twigs to
/// one side.
fn settle(v: f32) -> f32 {
    ((v - SURE_GROUND) / (SURE_SKY - SURE_GROUND)).clamp(0., 1.)
}

/// The standard deviation of the colours in the `(2r+1)²` window around each pixel,
/// the three channels' variances summed.
fn spread(rgb: &[[f32; 3]], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut variance = vec![0f32; w * h];
    for k in 0..3 {
        let channel: Vec<f32> = rgb.iter().map(|c| c[k]).collect();
        let squared: Vec<f32> = channel.iter().map(|v| v * v).collect();
        let (m, m2) = (mean(&channel, w, h, r), mean(&squared, w, h, r));
        for i in 0..w * h {
            variance[i] += (m2[i] - m[i] * m[i]).max(0.);
        }
    }
    variance.into_iter().map(f32::sqrt).collect()
}

/// Where the model says anything of sky at all.
fn questioned_seed(p: &[f32]) -> Vec<bool> {
    p.iter().map(|p| *p >= SURE_GROUND).collect()
}

/// The pixels of `mask` connected (8-neighbourly, within `mask`) to one that is also
/// in `from`.
fn reached(mask: &[bool], from: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut seen: Vec<bool> = (0..w * h).map(|i| mask[i] && from[i]).collect();
    let mut queue: Vec<usize> = (0..w * h).filter(|i| seen[*i]).collect();
    while let Some(i) = queue.pop() {
        let (x, y) = (i % w, i / w);
        for ny in y.saturating_sub(1)..(y + 2).min(h) {
            for nx in x.saturating_sub(1)..(x + 2).min(w) {
                let j = ny * w + nx;
                if mask[j] && !seen[j] {
                    seen[j] = true;
                    queue.push(j);
                }
            }
        }
    }
    seen
}

/// Square `(2r+1)²` windows clipped at the frame, rows then columns: whether `all` or
/// any of each window is set.
fn sweep(mask: &[bool], w: usize, h: usize, r: usize, all: bool) -> Vec<bool> {
    let pass = |src: &[bool], len: usize, lines: usize, at: &dyn Fn(usize, usize) -> usize| {
        let mut out = vec![false; src.len()];
        let mut prefix = vec![0usize; len + 1];
        for line in 0..lines {
            for j in 0..len {
                prefix[j + 1] = prefix[j] + usize::from(src[at(line, j)]);
            }
            for j in 0..len {
                let (a, b) = (j.saturating_sub(r), (j + r + 1).min(len));
                let set = prefix[b] - prefix[a];
                out[at(line, j)] = if all { set == b - a } else { set > 0 };
            }
        }
        out
    };
    let rows = pass(mask, w, h, &|y, x| y * w + x);
    pass(&rows, h, w, &|x, y| y * w + x)
}

/// Whether any pixel of the `(2r+1)²` window around each pixel is set; the window is
/// clipped at the frame, so outside it neither adds nor takes away.
fn dilate(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    sweep(mask, w, h, r, false)
}

/// Whether every pixel of the clipped `(2r+1)²` window around each pixel is set.
fn erode(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    sweep(mask, w, h, r, true)
}

/// What of `mask` a `(2r+1)²` square fits inside: the large parts.
fn open(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    dilate(&erode(mask, w, h, r), w, h, r)
}

/// `values` where `known`, elsewhere a smooth fill from the known values around:
/// averaged down a pyramid of halvings, then pushed back up. With nothing known, the
/// values themselves.
fn pull_push(values: &[[f32; 3]], known: &[bool], w: usize, h: usize) -> Vec<[f32; 3]> {
    if !known.contains(&true) {
        return values.to_vec();
    }
    // Each level holds weighted colour sums and their weights.
    let first: Vec<([f32; 3], f32)> = values
        .iter()
        .zip(known)
        .map(|(v, k)| if *k { (*v, 1.) } else { ([0.; 3], 0.) })
        .collect();
    let mut levels = vec![(first, w, h)];
    loop {
        let (last, lw, lh) = levels.last().expect("at least the first level");
        if *lw <= 1 && *lh <= 1 {
            break;
        }
        let (lw, lh) = (*lw, *lh);
        let (nw, nh) = (lw.div_ceil(2), lh.div_ceil(2));
        let mut next = vec![([0f32; 3], 0f32); nw * nh];
        for y in 0..lh {
            for x in 0..lw {
                let (c, wt) = last[y * lw + x];
                let cell = &mut next[(y / 2) * nw + x / 2];
                for (sum, v) in cell.0.iter_mut().zip(c) {
                    *sum += v;
                }
                cell.1 += wt;
            }
        }
        levels.push((next, nw, nh));
    }
    // Back down: each level's own average where it has weight, the coarser level's
    // answer (bilinear) where it has less.
    let (top, _, _) = levels.last().expect("at least the first level");
    let (c, wt) = top[0];
    let mut filled = vec![c.map(|v| v / wt)];
    let (mut fw, mut fh) = (1usize, 1usize);
    for (level, lw, lh) in levels.iter().rev().skip(1) {
        let at = |x: usize, y: usize| filled[y * fw + x];
        let coarse = |x: f32, y: f32| -> [f32; 3] {
            let x = x.clamp(0., (fw - 1) as f32);
            let y = y.clamp(0., (fh - 1) as f32);
            let (x0, y0) = (x as usize, y as usize);
            let (x1, y1) = ((x0 + 1).min(fw - 1), (y0 + 1).min(fh - 1));
            let (tx, ty) = (x - x0 as f32, y - y0 as f32);
            std::array::from_fn(|k| {
                let upper = at(x0, y0)[k] * (1. - tx) + at(x1, y0)[k] * tx;
                let lower = at(x0, y1)[k] * (1. - tx) + at(x1, y1)[k] * tx;
                upper * (1. - ty) + lower * ty
            })
        };
        let next: Vec<[f32; 3]> = (0..lw * lh)
            .map(|i| {
                let (x, y) = (i % lw, i / lw);
                let below = coarse((x as f32 + 0.5) / 2. - 0.5, (y as f32 + 0.5) / 2. - 0.5);
                let (c, wt) = level[i];
                if wt <= 0. {
                    return below;
                }
                let own = wt.min(1.);
                std::array::from_fn(|k| own * c[k] / wt + (1. - own) * below[k])
            })
            .collect();
        filled = next;
        (fw, fh) = (*lw, *lh);
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(w: usize, h: usize, x0: usize, y0: usize, side: usize) -> Vec<bool> {
        (0..w * h)
            .map(|i| (x0..x0 + side).contains(&(i % w)) && (y0..y0 + side).contains(&(i / w)))
            .collect()
    }

    #[test]
    fn opening_removes_what_is_small_and_keeps_what_is_large_exactly() {
        let (w, h) = (100, 80);
        let small = square(w, h, 10, 10, 5);
        let large = square(w, h, 40, 20, 40);
        let both: Vec<bool> = small.iter().zip(&large).map(|(a, b)| *a || *b).collect();
        assert_eq!(open(&both, w, h, 3), large);
    }

    #[test]
    fn a_mask_touching_the_frame_is_not_worn_away_there() {
        let (w, h) = (60, 40);
        let corner = square(w, h, 0, 0, 20);
        assert!(erode(&corner, w, h, 2)[0]);
        assert_eq!(open(&corner, w, h, 2), corner);
        let grown = dilate(&corner, w, h, 2);
        assert!(grown[21 * w + 21] && !grown[23 * w + 23]);
    }

    #[test]
    fn only_what_is_connected_to_the_seed_is_reached() {
        let (w, h) = (20, 10);
        let left = square(w, h, 1, 1, 5);
        let right = square(w, h, 12, 1, 5);
        let both: Vec<bool> = left.iter().zip(&right).map(|(a, b)| *a || *b).collect();
        let seed: Vec<bool> = (0..w * h).map(|i| i == 3 * w + 3).collect();
        assert_eq!(reached(&both, &seed, w, h), left);
    }

    #[test]
    fn pull_push_keeps_what_is_known_and_fills_between() {
        let (w, h) = (32, 8);
        let values: Vec<[f32; 3]> = (0..w * h)
            .map(|i| if i % w < 8 { [0.; 3] } else { [1.; 3] })
            .collect();
        let known: Vec<bool> = (0..w * h).map(|i| i % w < 8 || i % w >= 24).collect();
        let mut hidden = values.clone();
        for (v, k) in hidden.iter_mut().zip(&known) {
            if !k {
                *v = [0.5, 9., -9.];
            }
        }
        let filled = pull_push(&hidden, &known, w, h);
        for i in 0..w * h {
            if known[i] {
                assert_eq!(filled[i], values[i]);
            } else {
                assert!(
                    filled[i].iter().all(|c| (0.0..=1.0).contains(c)),
                    "{:?}",
                    filled[i]
                );
            }
        }
        let middle = filled[4 * w + 16][0];
        assert!((0.2..0.8).contains(&middle), "{middle}");
    }

    #[test]
    fn pull_push_with_nothing_known_returns_the_values() {
        let values = vec![[0.3, 0.4, 0.5]; 12];
        assert_eq!(pull_push(&values, &[false; 12], 4, 3), values);
    }
}
