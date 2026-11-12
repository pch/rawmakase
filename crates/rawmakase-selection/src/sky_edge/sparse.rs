//! The sparse mode: the sky behind a bare canopy, between twigs far apart.
//!
//! The sky's colour is measured as a local mean and spread, spread in from where the
//! model is sure, and each pixel is placed on the line between that sky and the colour
//! of what is clearly not sky right around it: a twig is the foreground of the gaps
//! beside it, a facade or a roof its own. Where the colours find sky over a large area
//! the model calls foreground (a wall, water) the model's answer stands, unless that
//! area is laced with twigs and joined to the sky through them.

use rawmakase_cancel::Cancel;

use super::{
    BAND, GATE, NEAR, SEPARATION, SKY_LIKE, SURE_GROUND, SURE_SKY, UNIFORM, cancelled, dilate,
    open, position, pull_push, questioned_seed, reached, settle, spread, step,
};
use crate::refine::{mean, smooth};
use rawmakase_inference::InferenceError;

/// Windows (fraction of the long side) over which the sky's colour and its variation
/// are measured: close by, for its gradient, and broadly, wide enough to hold both the
/// blue and the clouds of a cloudy sky.
const SKY_WINDOW: (f32, f32) = (0.08, 0.16);
/// Least spread (sRGB / 255, per channel) the sky is given: an even sky still varies
/// this much with noise and gradient.
const SKY_FLOOR: f32 = 0.03;
/// Distances from the sky (in its spreads) within which a colour is sky, and beyond
/// which it is clearly not.
const CLOSE: (f32, f32) = (3., 6.);
/// How far from the smooth sky (in its spreads) such a colour may be.
const SKY_LIKE_CLOSE: f32 = 8.5;
/// Share of a [`GATE`] window around a bright patch that may be darker than the sky
/// for the patch to count as a puff of cloud floating in it.
const CLEAR: f32 = 0.02;
/// How far (sRGB / 255) a cloud's colour may lie off the line from the sky to white.
const CLOUD: f32 = 0.08;
/// Width (fraction of the long side, at least 2 pixels) below which what is clearly
/// not sky is a thin structure: a twig, a wire, a strut.
const THIN: f32 = 0.0015;
/// Share of thin structures around a sky-coloured area (over twice the [`GATE`]) from
/// which it is a canopy's gaps rather than a wall or water, from not at all to fully.
const LACED: (f32, f32) = (0.01, 0.03);
/// Share of a window taken by the colours from which its centre is taken entirely:
/// how a decision fades out around where it was made.
const FILL: f32 = 0.02;
/// Model answers below which it is all but certain of foreground, up to where it
/// leaves room for doubt: a sunlit facade as pale as the sky behind a tree is the
/// former, the sky between a canopy's twigs the latter.
const DOUBT: (f32, f32) = (0.001, 0.003);
/// Distances from the sky (in its spreads) within which a small area is the sky itself,
/// taken even where the model is certain of foreground (the sky between a truss's
/// struts), and beyond which it needs the model's doubt.
const SKY_ITSELF: (f32, f32) = (1.5, 2.5);
/// How many times coarser a grid the sky's colour is measured on over a wide window.
const COARSE: usize = 4;

/// What [`sparse`] measures before it decides: where the model and the colours are
/// sure of sky, the sky's colour around each pixel and how far each pixel is from it,
/// and what is clearly not sky. `None` where there is no sure sky, no known
/// foreground or nothing clearly not sky, and the model's answer stands.
fn measure(rgb: &[[f32; 3]], p: &[f32], width: usize, height: usize) -> Option<Measured> {
    let (w, h) = (width, height);
    let n = w * h;
    let long = w.max(h) as f32;
    let gate = ((GATE * long).round() as usize).max(2);
    let band = ((BAND * long).round() as usize).max(gate);
    let thin_r = ((THIN * long).round() as usize).max(2);
    let window = |f: f32| ((f * long).round() as usize).max(gate);
    let (close, broad) = (window(SKY_WINDOW.0), window(SKY_WINDOW.1));
    let fine = spread(rgb, w, h, thin_r);
    let spread = spread(rgb, w, h, gate);
    let sky_sure: Vec<bool> = p
        .iter()
        .zip(&spread)
        .map(|(p, s)| *p > SURE_SKY && *s < UNIFORM)
        .collect();
    let questioned = dilate(&questioned_seed(p), w, h, band);
    let ground_far: Vec<bool> = questioned.iter().map(|q| !q).collect();
    // The ground far from the sky, or, where all of the photo is near the sky (a canopy
    // filling the frame), all the model calls foreground.
    let far_known = if ground_far.contains(&true) {
        ground_far
    } else {
        p.iter().map(|p| *p < SURE_GROUND).collect()
    };
    if !sky_sure.contains(&true) || !far_known.contains(&true) {
        return None;
    }
    // The sky's colour, from where it is smooth first; then also from wherever the
    // model sees sky (sure, or fading) and the colour clearly sides with that sky,
    // within its variation: streaks of cloud or a duller patch are measured against
    // their own sky; a spire, a twig or a facade brighter than the sky is not. Small
    // patches brighter than the sky where the model mostly sees sky, with nothing
    // darker than the sky around them, are puffs of cloud and part of the sky's
    // colour; a facade it fades over is large, or cut up by what stands before it.
    let far = pull_push(rgb, &far_known, w, h);
    let (sky0, spread0) = distribution(rgb, &sky_sure, w, h, close);
    let brighter: Vec<bool> = (0..n)
        .map(|i| !sky_sure[i] && p[i] > 0.5 && cloud_like(rgb[i], sky0[i]))
        .collect();
    let large_brighter = open(&brighter, w, h, gate);
    let darker: Vec<bool> = (0..n)
        .map(|i| {
            let off = distance(rgb[i], &sky0[i], &spread0[i]) > CLOSE.1;
            off && luma(rgb[i]) < luma(sky0[i])
        })
        .collect();
    // Only what is thicker than a twig counts: puffs seen among twigs are still puffs.
    let darker_around = mean(&as_f32(&open(&darker, w, h, thin_r)), w, h, gate);
    let sky_like: Vec<bool> = (0..n)
        .map(|i| {
            sky_sure[i]
                || (p[i] > SURE_SKY && !darker[i])
                || (brighter[i] && !large_brighter[i] && darker_around[i] <= CLEAR)
                || (p[i] >= SURE_GROUND
                    && distance(rgb[i], &sky0[i], &spread0[i]) < SKY_LIKE_CLOSE
                    && position(rgb[i], sky0[i], far[i]) >= SKY_LIKE)
        })
        .collect();
    let (sky, sky_spread) = distribution(rgb, &sky_like, w, h, close);
    let (wide, wide_spread) = distribution(rgb, &sky_sure, w, h, broad);
    let near_sky: Vec<f32> = (0..n)
        .map(|i| {
            distance(rgb[i], &sky[i], &sky_spread[i]).min(distance(
                rgb[i],
                &wide[i],
                &wide_spread[i],
            ))
        })
        .collect();
    // The foreground: what is clearly not sky and not sure sky to the model, its colour
    // and variation taken around each pixel and spread in from there.
    let not_sky: Vec<bool> = (0..n)
        .map(|i| near_sky[i] > CLOSE.1 && p[i] <= SURE_SKY)
        .collect();
    if !not_sky.contains(&true) {
        return None;
    }
    Some(Measured {
        gate,
        thin_r,
        fine,
        spread,
        sky_sure,
        questioned,
        sky,
        sky_spread,
        wide,
        wide_spread,
        near_sky,
        not_sky,
    })
}

/// [`measure`]'s findings, at the size it was given.
struct Measured {
    /// [`GATE`] in pixels.
    gate: usize,
    /// [`THIN`] in pixels.
    thin_r: usize,
    /// The colours' spread over a [`THIN`] window.
    fine: Vec<f32>,
    /// The colours' spread over a [`GATE`] window.
    spread: Vec<f32>,
    /// Sure sky: the model is sure and the colours are smooth.
    sky_sure: Vec<bool>,
    /// Where the model's answer is questioned, within [`BAND`] of its sky.
    questioned: Vec<bool>,
    /// The sky's colour close by, and its spread.
    sky: Vec<[f32; 3]>,
    sky_spread: Vec<[f32; 3]>,
    /// The sky's colour broadly, and its spread.
    wide: Vec<[f32; 3]>,
    wide_spread: Vec<[f32; 3]>,
    /// How far each pixel is from the sky, in its spreads.
    near_sky: Vec<f32>,
    /// What is clearly not sky and not sure sky to the model.
    not_sky: Vec<bool>,
}

/// The thin structures among what [`measure`] found clearly not sky (narrower than
/// [`THIN`]: twigs, wires, struts), and how much the area around each pixel (over twice
/// the [`GATE`]) is laced with them, ramped over [`LACED`] from not at all to fully.
fn lace(m: &Measured, w: usize, h: usize) -> (Vec<bool>, Vec<f32>) {
    let thick = open(&m.not_sky, w, h, m.thin_r);
    let thin: Vec<bool> = (0..w * h).map(|i| m.not_sky[i] && !thick[i]).collect();
    let lace = mean(&as_f32(&thin), w, h, 2 * m.gate)
        .into_iter()
        .map(|v| step(LACED.0, LACED.1, v))
        .collect();
    (thin, lace)
}

/// [`sparse_with`], never cancelled.
#[cfg(test)]
pub(crate) fn sparse(rgb: &[[f32; 3]], p: &[f32], width: usize, height: usize) -> Vec<f32> {
    sparse_with(rgb, p, width, height, Cancel::NEVER).expect("a flag that is never raised")
}

/// Sky coverage (0..=1) from the model's answer, `p` (0..=1 at `width` x `height`),
/// refined with `rgb`, the photo's colours (sRGB / 255) at the same size. `cancel` is
/// checked between the passes.
pub(crate) fn sparse_with(
    rgb: &[[f32; 3]],
    p: &[f32],
    width: usize,
    height: usize,
    cancel: Cancel<'_>,
) -> Result<Vec<f32>, InferenceError> {
    let (w, h) = (width, height);
    let n = w * h;
    let Some(m) = measure(rgb, p, w, h) else {
        return Ok(p.iter().map(|p| smooth(*p)).collect());
    };
    cancelled(cancel)?;
    let (thin, lace) = lace(&m, w, h);
    let Measured {
        gate,
        fine,
        spread,
        sky_sure,
        questioned,
        sky,
        sky_spread,
        wide,
        wide_spread,
        near_sky,
        not_sky,
        thin_r,
        ..
    } = m;
    let (ground, ground_spread) = distribution(rgb, &not_sky, w, h, gate);
    cancelled(cancel)?;
    // Sure sky that is clearly not sky's colour (the even body of a tower the model
    // swallowed) is questioned too.
    let unknown: Vec<bool> = (0..n)
        .map(|i| !sky_sure[i] || near_sky[i] > CLOSE.1)
        .collect();
    // What the colours say (where between the local ground and the sky, and only as
    // much sky as it is close to the line between them), and how far they can be
    // trusted to say it.
    let (colour, trust): (Vec<f32>, Vec<f32>) = (0..n)
        .map(|i| {
            if !unknown[i] {
                return (p[i], 0.);
            }
            let (b, f) = (sky[i], ground[i]);
            let length2 = (0..3).map(|k| (b[k] - f[k]).powi(2)).sum::<f32>();
            if length2 <= 0. {
                return (p[i], 0.);
            }
            // Where the model sees foreground, the colours must also vary there: a crown
            // as dark as the night behind it does not tell the two apart, and the
            // model's answer stands. Where it sees some sky it is enough that the sky
            // differs from the ground or from the pixel itself (a facade's face, which
            // with its dark windows averages to the grey of the sky).
            let itself = (0..3).map(|k| (rgb[i][k] - b[k]).powi(2)).sum::<f32>();
            let apart = if p[i] >= SURE_GROUND {
                length2.max(itself).sqrt()
            } else {
                length2.sqrt().min(2. * spread[i])
            };
            let t = position(rgb[i], b, f).clamp(0., 1.);
            let on_line: [f32; 3] = std::array::from_fn(|k| f[k] + t * (b[k] - f[k]));
            let off = distance(rgb[i], &on_line, &sky_spread[i]).min(distance(
                rgb[i],
                &wide[i],
                &wide_spread[i],
            ));
            // A colour off the sky's own variation is partly sky only as a blend: where
            // it stands out from the foreground around it (a twig paler with the sky
            // behind it, not slate between darker courses), or, short of clearly not
            // sky, on an edge rather than inside an even surface (a grey facade).
            let blend = if not_sky[i] {
                step(CLOSE.0, CLOSE.1, distance(rgb[i], &f, &ground_spread[i]))
            } else {
                step(0.5 * UNIFORM, UNIFORM, fine[i])
            };
            // A colour within the sky's own variation is sky, wherever it lies on the line.
            let within = 1. - step(CLOSE.0, CLOSE.1, near_sky[i]);
            let mix = (t * blend).max(within) * (1. - step(CLOSE.0, CLOSE.1, off));
            (mix, step(SEPARATION.0, SEPARATION.1, apart))
        })
        .unzip();
    let mixed: Vec<f32> = (0..n)
        .map(|i| p[i] + trust[i] * (colour[i] - p[i]))
        .collect();
    // Where the colours find sky the model calls foreground, judged on the colours
    // alone: an even area the colours cannot be trusted over is still one area.
    let add: Vec<bool> = (0..n)
        .map(|i| unknown[i] && colour[i] > 0.5 && p[i] <= 0.5)
        .collect();
    // Out of what the model is sure is sky, only what is darker than the sky is taken
    // out: a spire, a cross, twigs; a puff of cloud stays.
    let remove: Vec<bool> = (0..n)
        .map(|i| {
            unknown[i]
                && colour[i] < 0.5
                && p[i] > 0.5
                && (p[i] <= SURE_SKY || luma(rgb[i]) < luma(sky[i]))
        })
        .collect();
    // Small areas near the sky are taken as they are; a large one only when thin
    // structures lace it and join it to the model's sky, however far that reaches: the
    // sky behind a bare canopy, not a lake or a wall.
    let large_add = open(&add, w, h, gate);
    let path: Vec<bool> = (0..n).map(|i| add[i] || thin[i]).collect();
    let beside_sky = dilate(&p.iter().map(|p| *p > 0.5).collect::<Vec<_>>(), w, h, 1);
    let joined = reached(&path, &beside_sky, w, h);
    let taken_add: Vec<f32> = (0..n)
        .map(|i| {
            let doubt = step(DOUBT.0, DOUBT.1, p[i]);
            if !add[i] {
                0.
            } else if !large_add[i] && questioned[i] {
                doubt.max(1. - step(SKY_ITSELF.0, SKY_ITSELF.1, near_sky[i]))
            } else if joined[i] {
                doubt * lace[i]
            } else {
                0.
            }
        })
        .collect();
    cancelled(cancel)?;
    // What the colours take out of the model's sure sky: a small thing (a spire's tip,
    // a cross), or the top of something whose body the model fades out of. Out of sky
    // the model is sure of, only where it is joined to what the model doubts or calls
    // ground, or is all thinner than [`THIN`] (a wire): not a grey cloud floating in the
    // open sky, rim and all.
    let taken_remove: Vec<f32> = {
        let large = open(&remove, w, h, gate);
        let fading: Vec<bool> = (0..n).map(|i| remove[i] && p[i] <= SURE_SKY).collect();
        let body = reached(&remove, &fading, w, h);
        let doubted: Vec<bool> = (0..n).map(|i| p[i] <= 0.5 || fading[i]).collect();
        let through: Vec<bool> = (0..n).map(|i| remove[i] || doubted[i]).collect();
        let anchored = reached(&through, &doubted, w, h);
        let floating: Vec<bool> = (0..n)
            .map(|i| remove[i] && p[i] > SURE_SKY && !anchored[i])
            .collect();
        let cloud = reached(&floating, &open(&floating, w, h, thin_r), w, h);
        (0..n)
            .map(|i| f32::from(u8::from(remove[i] && (!large[i] || body[i]) && !cloud[i])))
            .collect()
    };
    cancelled(cancel)?;
    // How much each pixel takes the colours' answer: a smooth field around what was
    // taken, never a square zone. Around taken gaps, twigs take their share too, but a
    // sky-coloured area that was not taken stays as the model said, and nothing lowers
    // sky the model found. The model's unsure edge is the colours' wherever it is the
    // edge of the sky: a faint wisp it draws on water, cut off from the sky, is not.
    // Gaps inside what the model calls foreground, not along its outline.
    let outline = reached(
        &taken_add.iter().map(|t| *t > 0.).collect::<Vec<_>>(),
        &beside_sky,
        w,
        h,
    );
    let gaps: Vec<f32> = (0..n)
        .map(|i| if outline[i] { 0. } else { taken_add[i] })
        .collect();
    let around_add = mean(&gaps, w, h, gate);
    let near_add = mean(&taken_add, w, h, NEAR);
    let near_remove = mean(&taken_remove, w, h, NEAR);
    let edge_of_sky = reached(&questioned_seed(p), &sky_sure, w, h);
    cancelled(cancel)?;
    Ok((0..n)
        .map(|i| {
            if !unknown[i] {
                return smooth(p[i]);
            }
            let unsure = if edge_of_sky[i] {
                step(0.5 * SURE_GROUND, 1.5 * SURE_GROUND, p[i])
                    * (1. - step(SURE_SKY - 0.05, SURE_SKY, p[i]))
            } else {
                0.
            };
            let below = 1. - step(0.4, 0.6, p[i]);
            // A gap's own pixels take the colours' answer also where most of the few
            // pixels around them were taken: its rim, blended into a strut.
            let gaps = if add[i] {
                taken_add[i].max(step(0.2, 0.5, near_add[i]))
            } else {
                ((around_add[i] + near_add[i]) / FILL).min(1.)
            };
            let weight = unsure
                .max(below * gaps)
                .max((near_remove[i] / FILL).min(1.));
            let model = smooth(p[i]);
            model + weight * (settle(mixed[i]) - model)
        })
        .collect())
}

/// How far `colour` is from `centre`, in units of `spread` (per channel, then over the
/// channels together).
fn distance(colour: [f32; 3], centre: &[f32; 3], spread: &[f32; 3]) -> f32 {
    (0..3)
        .map(|k| ((colour[k] - centre[k]) / spread[k]).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// Whether `colour` is the `sky` paled towards white, as a cloud in it is: brighter,
/// and off the line from the sky to white by at most [`CLOUD`].
fn cloud_like(colour: [f32; 3], sky: [f32; 3]) -> bool {
    if luma(colour) <= luma(sky) {
        return false;
    }
    let t = position(colour, [1.; 3], sky).clamp(0., 1.);
    let on_line: [f32; 3] = std::array::from_fn(|k| sky[k] + t * (1. - sky[k]));
    let off = (0..3)
        .map(|k| (colour[k] - on_line[k]).powi(2))
        .sum::<f32>();
    off.sqrt() <= CLOUD
}

fn luma(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

fn as_f32(mask: &[bool]) -> Vec<f32> {
    mask.iter().map(|m| f32::from(u8::from(*m))).collect()
}

/// The colour of the `known` pixels in the `(2r+1)²` window around each pixel, as its
/// mean and spread per channel (at least [`SKY_FLOOR`]), spread in from where a window
/// holds enough of them to everywhere else. A wide window is measured on a grid
/// [`COARSE`] times coarser and smoothly enlarged: it is as smooth, and far cheaper.
fn distribution(
    rgb: &[[f32; 3]],
    known: &[bool],
    w: usize,
    h: usize,
    r: usize,
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let f = if r >= 8 * COARSE { COARSE } else { 1 };
    let (cw, ch) = (w.div_ceil(f), h.div_ceil(f));
    // Per cell: how many pixels are known, and the sums of their colours and squares.
    let mut count = vec![0f32; cw * ch];
    let mut sums = vec![[0f32; 3]; cw * ch];
    let mut squares = vec![[0f32; 3]; cw * ch];
    for (i, (c, m)) in rgb.iter().zip(known).enumerate() {
        if *m {
            let cell = (i / w / f) * cw + (i % w) / f;
            count[cell] += 1.;
            for k in 0..3 {
                sums[cell][k] += c[k];
                squares[cell][k] += c[k] * c[k];
            }
        }
    }
    let r = r / f;
    let weight = mean(&count, cw, ch, r);
    let window =
        |v: &[[f32; 3]], k: usize| mean(&v.iter().map(|c| c[k]).collect::<Vec<_>>(), cw, ch, r);
    let (mut centre, mut square) = (vec![[0f32; 3]; cw * ch], vec![[0f32; 3]; cw * ch]);
    for k in 0..3 {
        let (m, m2) = (window(&sums, k), window(&squares, k));
        for i in 0..cw * ch {
            if weight[i] > 0. {
                centre[i][k] = m[i] / weight[i];
                square[i][k] = m2[i] / weight[i];
            }
        }
    }
    let area = (f * f) as f32;
    let enough: Vec<bool> = weight.iter().map(|v| *v > 0.1 * area).collect();
    let enough = if enough.contains(&true) {
        enough
    } else {
        weight.iter().map(|v| *v > 0.).collect()
    };
    let centre = pull_push(&centre, &enough, cw, ch);
    let square = pull_push(&square, &enough, cw, ch);
    let spread: Vec<[f32; 3]> = (0..cw * ch)
        .map(|i| {
            std::array::from_fn(|k| {
                (square[i][k] - centre[i][k] * centre[i][k])
                    .max(0.)
                    .sqrt()
                    .max(SKY_FLOOR)
            })
        })
        .collect();
    if f == 1 {
        return (centre, spread);
    }
    (
        enlarge(&centre, cw, ch, w, h, f),
        enlarge(&spread, cw, ch, w, h, f),
    )
}

/// `values` on a grid of cells `f` pixels wide, bilinearly enlarged to `w` x `h`.
fn enlarge(
    values: &[[f32; 3]],
    cw: usize,
    ch: usize,
    w: usize,
    h: usize,
    f: usize,
) -> Vec<[f32; 3]> {
    let at = |v: usize, cells: usize| {
        let c = ((v as f32 + 0.5) / f as f32 - 0.5).clamp(0., (cells - 1) as f32);
        let c0 = c as usize;
        (c0, (c0 + 1).min(cells - 1), c - c0 as f32)
    };
    let columns: Vec<_> = (0..w).map(|x| at(x, cw)).collect();
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let (y0, y1, ty) = at(y, ch);
        for &(x0, x1, tx) in &columns {
            let (a, b) = (values[y0 * cw + x0], values[y0 * cw + x1]);
            let (c, d) = (values[y1 * cw + x0], values[y1 * cw + x1]);
            out.push(std::array::from_fn(|k| {
                let upper = a[k] + tx * (b[k] - a[k]);
                let lower = c[k] + tx * (d[k] - c[k]);
                upper + ty * (lower - upper)
            }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sparse;
    use crate::sky_edge::fixtures::{
        self, answer, building, canopy_pixels, photo, run, sparse_canopy, tree_twig, truss_strut,
    };

    #[test]
    fn sky_is_where_the_model_says_with_its_edge_on_the_photos() {
        fixtures::sky_is_where_the_model_says_with_its_edge_on_the_photos(sparse);
    }

    #[test]
    fn water_under_the_sky_is_left_out_when_the_model_leaves_it_out() {
        fixtures::water_under_the_sky_is_left_out_when_the_model_leaves_it_out(sparse);
    }

    #[test]
    fn sky_below_leaves_along_the_top_is_kept() {
        fixtures::sky_below_leaves_along_the_top_is_kept(sparse);
    }

    #[test]
    fn sky_between_branches_is_found_and_the_branches_are_not() {
        fixtures::sky_between_branches_is_found_and_the_branches_are_not(sparse);
    }

    #[test]
    fn a_sky_coloured_wall_next_to_the_sky_stays_out_up_to_its_edge() {
        fixtures::a_sky_coloured_wall_next_to_the_sky_stays_out_up_to_its_edge(sparse);
    }

    #[test]
    fn twigs_in_a_crown_with_gaps_are_partly_sky() {
        fixtures::twigs_in_a_crown_with_gaps_are_partly_sky(sparse);
    }

    #[test]
    fn a_reflection_broken_by_reflected_trees_stays_out() {
        fixtures::reflection_stays_out(sparse, 0.12);
    }

    /// The model's own faint answer on water, away from the sky, is no more sky than
    /// it was before the colours were asked.
    #[test]
    fn a_reflection_the_model_is_a_little_unsure_of_stays_out() {
        fixtures::reflection_stays_out(sparse, 0.3);
    }

    #[test]
    fn a_faint_wisp_on_the_water_away_from_the_sky_stays_out() {
        fixtures::a_faint_wisp_on_the_water_away_from_the_sky_stays_out(sparse);
    }

    #[test]
    fn a_streaky_sky_where_the_model_fades_stays_entirely_selected() {
        fixtures::a_streaky_sky_where_the_model_fades_stays_entirely_selected(sparse);
    }

    #[test]
    fn pale_wood_in_a_trunk_beside_the_sky_stays_out() {
        fixtures::pale_wood_in_a_trunk_beside_the_sky_stays_out(sparse);
    }

    #[test]
    fn a_streaky_sky_beside_a_trunk_stays_entirely_selected() {
        fixtures::a_streaky_sky_beside_a_trunk_stays_entirely_selected(sparse);
    }

    #[test]
    fn a_duller_patch_of_sky_where_the_model_fades_stays_entirely_selected() {
        fixtures::a_duller_patch_of_sky_where_the_model_fades_stays_entirely_selected(sparse);
    }

    #[test]
    fn a_cloudy_sky_away_from_its_edge_is_entirely_selected() {
        fixtures::a_cloudy_sky_away_from_its_edge_is_entirely_selected(sparse);
    }

    #[test]
    fn dark_sky_behind_dark_trees_keeps_the_models_answer() {
        fixtures::dark_sky_behind_dark_trees_keeps_the_models_answer(sparse);
    }

    #[test]
    fn tiny_and_extreme_photos_do_not_break() {
        fixtures::tiny_and_extreme_photos_do_not_break(sparse);
    }

    #[test]
    fn all_sky_is_the_models_answer() {
        fixtures::all_sky_is_the_models_answer(sparse);
    }

    /// Small puffs of cloud, brighter than the blue around them, among the outer twigs
    /// of a bare crown where the model's answer is only fairly sure of sky: they are as
    /// much sky as the blue, not white specks left out of it.
    #[test]
    fn puffs_of_cloud_among_twigs_where_the_model_fades_stay_selected() {
        let puff = |x: usize, y: usize| {
            let (cx, cy) = ((x % 64) as i32 - 32, (y % 48) as i32 - 24);
            cx * cx + cy * cy < 14 * 14
        };
        let twig = |x: usize, y: usize| x >= 512 && ((x + y) % 97 < 2 || (x + 3 * y) % 151 < 2);
        let img = photo(1024, 683, |x, y| {
            if y >= 500 || twig(x, y) {
                [35, 32, 30]
            } else if puff(x, y) {
                [215, 216, 212]
            } else {
                [120, 133, 150]
            }
        });
        let probability = answer(1024, 683, |fx, fy| {
            let (x, y) = (fx * 1024., fy * 683.);
            if y >= 500. {
                0.01
            } else if x >= 512. {
                0.7
            } else {
                0.99
            }
        });
        let cov = run(sparse, &img, &probability, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        let centres: Vec<(usize, usize)> = (0..9)
            .flat_map(|j| (0..16).map(move |i| (i * 64 + 32, j * 48 + 24)))
            .filter(|(x, y)| *y < 470 && *x > 20 && *x < 1000)
            .filter(|(x, y)| (x - 4..x + 5).all(|tx| (y - 4..y + 5).all(|ty| !twig(tx, ty))))
            .collect();
        assert!(centres.iter().filter(|(x, _)| *x > 512).count() > 12);
        let missed: Vec<_> = centres.iter().filter(|(x, y)| px(*x, *y) <= 204).collect();
        assert!(
            missed.is_empty(),
            "{} of {} puffs missed, e.g. {:?}: {}",
            missed.len(),
            centres.len(),
            missed[0],
            px(missed[0].0, missed[0].1)
        );
        assert!(px(500, 600) < 10);
    }

    /// Sunlit pale twigs at the top of a crown against a deep blue sky, where the
    /// model's answer is only fairly sure of sky: brighter than the sky, but not a
    /// cloud's white, so they are not taken for puffs of cloud and stay out.
    #[test]
    fn pale_twigs_against_a_deep_blue_sky_are_not_clouds() {
        let twig = |x: usize, y: usize| {
            (300..700).contains(&x) && (100..300).contains(&y) && (x % 9 < 2 || (x + y) % 13 < 2)
        };
        let img = photo(1024, 683, |x, y| {
            if y >= 500 {
                [50, 45, 40]
            } else if twig(x, y) {
                [168, 140, 105]
            } else {
                [45, 90, 160]
            }
        });
        let probability = answer(1024, 683, |fx, fy| {
            let (x, y) = (fx * 1024., fy * 683.);
            if y >= 500. {
                0.01
            } else if (300.0..700.).contains(&x) && (100.0..300.).contains(&y) {
                0.7
            } else {
                0.99
            }
        });
        let cov = run(sparse, &img, &probability, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        for (x, y) in [(450, 151), (451, 200), (540, 250), (630, 133)] {
            assert!(twig(x, y));
            assert!(px(x, y) < 128, "twig {x},{y}: {}", px(x, y));
        }
    }

    #[test]
    fn sky_between_the_twigs_of_a_sparse_canopy_is_found() {
        let (img, p) = sparse_canopy(|_, y| [110 + (y / 8) as u8, 150 + (y / 10) as u8, 225]);
        let cov = run(sparse, &img, &p, 1024, 683);
        let px = |(x, y): (usize, usize)| cov.data[y * cov.width + x];
        let (gaps, twigs) = canopy_pixels();
        let missed: Vec<_> = gaps.iter().filter(|g| px(**g) <= 204).collect();
        assert!(
            missed.is_empty(),
            "{} of {} gaps missed, e.g. {:?}: {}",
            missed.len(),
            gaps.len(),
            missed[0],
            px(*missed[0])
        );
        for t in twigs {
            assert!(px(t) < 51, "twig {t:?}: {}", px(t));
        }
        assert!(px((500, 600)) < 10);
    }

    #[test]
    fn bright_clouds_between_the_twigs_of_a_sparse_canopy_are_found() {
        // Puffs of white cloud, brighter than the blue sky around them, across the
        // whole sky and so behind the canopy too.
        let cloud = |x: usize, y: usize| (x / 70 + y / 45).is_multiple_of(3);
        let (img, p) = sparse_canopy(|x, y| {
            if cloud(x, y) {
                [236, 238, 242]
            } else {
                [120, 160, 225]
            }
        });
        let cov = run(sparse, &img, &p, 1024, 683);
        let px = |(x, y): (usize, usize)| cov.data[y * cov.width + x];
        let (gaps, _) = canopy_pixels();
        let clouds: Vec<_> = gaps.iter().filter(|(x, y)| cloud(*x, *y)).collect();
        assert!(clouds.len() > 20);
        let missed: Vec<_> = gaps.iter().filter(|g| px(**g) <= 204).collect();
        assert!(
            missed.is_empty(),
            "{} of {} gaps missed, e.g. {:?}: {}",
            missed.len(),
            gaps.len(),
            missed[0],
            px(*missed[0])
        );
    }

    #[test]
    fn a_white_facade_under_an_overcast_sky_stays_out() {
        // Brighter and warmer than the sky, with rows of dark windows; the model's
        // answer fades from row 280 to row 340.
        let window = |x: usize, y: usize| (x - 150) % 40 >= 28 && (y - 300) % 50 >= 16;
        let (img, p) = building(
            |x, y| {
                if window(x, y) {
                    [40, 42, 48]
                } else {
                    [236, 228, 210]
                }
            },
            280.0..340.,
        );
        let cov = run(sparse, &img, &p, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        for y in (304..360).step_by(4) {
            for x in (155..865)
                .step_by(5)
                .filter(|x| !window(*x, y) && !tree_twig(*x, y))
            {
                assert!(px(x, y) < 26, "facade {x},{y}: {}", px(x, y));
            }
        }
        assert!(px(512, 250) > 240 && px(512, 150) > 240);
    }

    #[test]
    fn a_slate_roof_where_the_model_fades_stays_out() {
        // Rows of slates, the model's answer 0.8 to 0.4 across the roof's top rows.
        let (img, p) = building(
            |_, y| {
                if (y - 300) % 9 < 2 {
                    [96, 99, 106]
                } else {
                    [124, 127, 135]
                }
            },
            281.0..379.,
        );
        assert!((0.75..0.85).contains(&p[302 * 1024 + 512]));
        let cov = run(sparse, &img, &p, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        for y in (304..360).step_by(4) {
            for x in (155..865).step_by(5).filter(|x| !tree_twig(*x, y)) {
                assert!(px(x, y) < 26, "roof {x},{y}: {}", px(x, y));
            }
        }
        assert!(px(512, 250) > 240);
    }

    /// A tower 40 pixels wide under a thin spire, as even in colour as slate or copper.
    /// The model calls the spire sky and fades out of the tower down its body: both are
    /// out all along, with no step where the model's answer gives way to the colours'.
    #[test]
    fn a_tower_the_model_half_covers_has_no_blocks() {
        let in_tower = |x: f32, y: f32| {
            (490.0..530.).contains(&x) && y >= 250. || (507.0..513.).contains(&x) && y >= 120.
        };
        let img = photo(1024, 683, |x, y| {
            if y >= 500 {
                [50, 48, 45]
            } else if in_tower(x as f32, y as f32) {
                [62, 66, 60]
            } else {
                [176, 180, 189]
            }
        });
        let probability = answer(1024, 683, |fx, fy| {
            let (x, y) = (fx * 1024., fy * 683.);
            if y >= 500. {
                0.01
            } else if in_tower(x, y) {
                (0.99 - 0.98 * (y - 250.) / 200.).clamp(0.01, 0.99)
            } else {
                0.99
            }
        });
        let cov = run(sparse, &img, &probability, 1024, 683);
        let px = |x: usize, y: usize| i32::from(cov.data[y * cov.width + x]);
        for y in 124..250 {
            assert!(px(510, y) < 51, "spire 510,{y}: {}", px(510, y));
        }
        for x in 494..526 {
            for y in 254..495 {
                assert!(px(x, y) < 51, "tower {x},{y}: {}", px(x, y));
                let step = (px(x, y + 1) - px(x, y)).abs();
                assert!(
                    step < 26,
                    "step at {x},{y}: {} to {}",
                    px(x, y),
                    px(x, y + 1)
                );
            }
        }
        assert!(px(300, 200) > 240 && px(700, 400) > 240);
    }

    /// The truss of a bridge's arch ([`truss_strut`]), and a model answer that, like
    /// UperNet's, is all but certain the whole truss down to the deck is not sky.
    #[test]
    fn sky_between_the_struts_of_a_truss_is_found() {
        let img = photo(1024, 683, |x, y| {
            if y >= 500 || truss_strut(x, y) || (200..800).contains(&x) && y >= 390 {
                [40, 44, 46]
            } else {
                [226 + (y / 100) as u8, 228, 230]
            }
        });
        let probability = answer(1024, 683, |fx, fy| {
            let (x, y) = (fx * 1024., fy * 683.);
            if y >= 500. || (200.0..800.).contains(&x) && y >= 250. {
                0.0003
            } else {
                0.99
            }
        });
        let cov = run(sparse, &img, &probability, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        let clear = |x: usize, y: usize| {
            (x - 4..x + 5).all(|tx| (y - 4..y + 5).all(|ty| !truss_strut(tx, ty)))
        };
        let gaps: Vec<(usize, usize)> = (258..382)
            .step_by(5)
            .flat_map(|y| (210..790).step_by(5).map(move |x| (x, y)))
            .filter(|(x, y)| clear(*x, *y))
            .collect();
        assert!(gaps.len() > 100);
        let missed: Vec<_> = gaps.iter().filter(|(x, y)| px(*x, *y) <= 204).collect();
        assert!(
            missed.is_empty(),
            "{} of {} gaps missed, e.g. {:?}: {}",
            missed.len(),
            gaps.len(),
            missed[0],
            px(missed[0].0, missed[0].1)
        );
        for (x, y) in [(300, 290), (500, 292), (700, 291), (400, 338)] {
            assert!(truss_strut(x, y));
            assert!(px(x, y) < 51, "strut {x},{y}: {}", px(x, y));
        }
        assert!(px(500, 450) < 26, "deck {}", px(500, 450));
        assert!(px(500, 600) < 10);
    }
}
