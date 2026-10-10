//! Lightroom's parametric tone curve (the Tone Curve's Shadows, Darks, Lights and
//! Highlights regions and their three splits), as Camera Raw 18.7 renders it on the
//! synthetic chart. See docs/tone-controls.md#parametric-curve.
//!
//! Camera Raw applies it as one curve, DNG RGBTone fashion (the brightest and darkest
//! channel curved, the middle one keeping its place, so hue holds), in encoded ProPhoto
//! RGB between the Basic panel's tone and the point curve.

const SIZE: usize = 1024;

/// The measured curve for one recipe's regions and splits, sampled at `SIZE + 1`
/// points over 0–1.
#[derive(Clone, Debug)]
pub(crate) struct ParametricCurve {
    lut: Vec<f32>,
}
impl ParametricCurve {
    /// `None` when every region is 0, as the curve is then the identity.
    pub(crate) fn new(regions: [f32; 4], splits: [f32; 3]) -> Option<Self> {
        if regions == [0.; 4] {
            return None;
        }
        let shape = Shape::new(regions, splits);
        let lut = (0..=SIZE)
            .map(|i| shape.eval(i as f32 / SIZE as f32))
            // Measured tables carry small non-monotone noise; tone must never invert.
            .scan(0f32, |max, y| {
                *max = max.max(y);
                Some(*max)
            })
            .collect();
        Some(Self { lut })
    }
    /// `first`, then `then`, as one curve: the user's, then a look's.
    pub(crate) fn then(first: Option<Self>, then: Option<Self>) -> Option<Self> {
        match (first, then) {
            (Some(a), Some(b)) => Some(Self {
                lut: a.lut.iter().map(|y| b.eval(*y)).collect(),
            }),
            (a, b) => a.or(b),
        }
    }
    pub(crate) fn values(&self) -> &[f32] {
        &self.lut
    }
    pub(crate) fn eval(&self, x: f32) -> f32 {
        let f = x.clamp(0., 1.) * SIZE as f32;
        let i = (f as usize).min(SIZE - 1);
        self.lut[i] + (self.lut[i + 1] - self.lut[i]) * (f - i as f32)
    }
    /// DNG RGBTone: the curve maps the largest and smallest channel, and the middle
    /// channel keeps its relative position, preserving hue.
    pub(crate) fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let p = p.map(|v| v.clamp(0., 1.));
        let lo = p.into_iter().fold(f32::INFINITY, f32::min);
        let hi = p.into_iter().fold(0f32, f32::max);
        let (a, b) = (self.eval(lo), self.eval(hi));
        if hi - lo > 1e-8 {
            p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
        } else {
            [a; 3]
        }
    }
}

/// The parametric curve at `count + 1` points over 0–1, for drawing it in the Tone
/// Curve panel.
pub fn samples(e: &crate::model::effects::Effects, count: usize) -> Vec<f32> {
    let at = |i: usize| i as f32 / count as f32;
    // The rendered table, with its monotone clean-up, so the panel shows what renders.
    match ParametricCurve::new(e.parametric, e.splits) {
        Some(curve) => (0..=count).map(|i| curve.eval(at(i))).collect(),
        // No region set: the identity.
        None => (0..=count).map(at).collect(),
    }
}

/// Measured tables (`parametric.bin`, written by `scripts/corpus/parametric-curve.py`):
/// changes to the identity at `SAMPLES` points over 0–1, in gamma-2.2 encoding, as
/// little-endian f32, in this order:
/// - Shadows over 0 to the midtone split, normalized to 0–1: per amount, per shadow
///   split as a fraction of the midtone split (`RATIOS`);
/// - Highlights over the midtone split to 1, normalized: per amount, per highlight
///   split as a fraction of the span above the midtone split;
/// - Darks, then Lights: per amount, per midtone split (`MIDTONES`);
/// - Darks and Lights together at the default splits, per `JOINT` × `JOINT`.
static DATA: &[u8] = include_bytes!("parametric.bin");
const SAMPLES: usize = 65;
/// Measured slider positions (−100 to 100 as −1 to 1); 0 is the identity.
const AMOUNTS: [f32; 8] = [-1., -0.75, -0.5, -0.25, 0.25, 0.5, 0.75, 1.];
const RATIOS: [f32; 5] = [0.2, 0.36, 0.5, 0.64, 0.8];
const MIDTONES: [f32; 7] = [0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
const JOINT: [f32; 6] = [-1., -0.5, -0.25, 0.25, 0.5, 1.];
const LOW: usize = 0;
const HIGH: usize = LOW + AMOUNTS.len() * RATIOS.len();
const DARKS: usize = HIGH + AMOUNTS.len() * RATIOS.len();
const LIGHTS: usize = DARKS + AMOUNTS.len() * MIDTONES.len();
const BOTH: usize = LIGHTS + AMOUNTS.len() * MIDTONES.len();
const TABLES: usize = BOTH + JOINT.len() * JOINT.len();
/// The midtone split's default, where Darks and Lights were measured together.
const DEFAULT_MIDTONE: usize = 3;

/// One measured table, as changes to the identity.
type Table = [f32; SAMPLES];
fn table(i: usize) -> Table {
    debug_assert_eq!(DATA.len(), TABLES * SAMPLES * 4);
    std::array::from_fn(|k| {
        let at = 4 * (i * SAMPLES + k);
        f32::from_le_bytes([DATA[at], DATA[at + 1], DATA[at + 2], DATA[at + 3]])
    })
}
fn blend(tables: &[(Table, f32)]) -> Table {
    std::array::from_fn(|k| tables.iter().map(|(t, w)| t[k] * w).sum())
}
/// The bracketing positions of `v` in `values` (clamped to their range) and the
/// weight of the upper one.
pub(super) fn bracket(values: &[f32], v: f32) -> (usize, f32) {
    let v = v.clamp(values[0], values[values.len() - 1]);
    let j = values
        .windows(2)
        .position(|w| v <= w[1])
        .unwrap_or(values.len() - 2);
    (j, (v - values[j]) / (values[j + 1] - values[j]))
}
/// A region's table at slider `amount`, linear between the measured positions with 0
/// as no change; `column` picks the table at one measured split for an amount.
fn at_amount(amount: f32, column: impl Fn(usize) -> Table) -> Table {
    let zero = AMOUNTS.iter().filter(|a| **a < 0.).count();
    // The measured positions with the identity at 0 between them.
    let mut positions: Vec<(f32, Option<usize>)> = AMOUNTS
        .iter()
        .enumerate()
        .map(|(i, a)| (*a, Some(i)))
        .collect();
    positions.insert(zero, (0., None));
    let values: Vec<f32> = positions.iter().map(|p| p.0).collect();
    let (j, w) = bracket(&values, amount);
    let get = |p: Option<usize>| p.map_or([0.; SAMPLES], &column);
    blend(&[(get(positions[j].1), 1. - w), (get(positions[j + 1].1), w)])
}
/// A table measured per amount and per split value, at `amount` and `split`.
fn at(first: usize, splits: &[f32], amount: f32, split: f32) -> Table {
    let (j, w) = bracket(splits, split);
    at_amount(amount, |a| {
        let row = first + a * splits.len();
        blend(&[(table(row + j), 1. - w), (table(row + j + 1), w)])
    })
}
/// Darks and Lights both set, at the default splits: bilinear over the joint grid,
/// whose axes are the single-region tables.
fn joint(darks: f32, lights: f32) -> Table {
    let mut axis: Vec<f32> = JOINT.to_vec();
    axis.insert(3, 0.);
    let node = |d: usize, l: usize| -> Table {
        match (d, l) {
            (3, 3) => [0.; SAMPLES],
            (3, _) => at(LIGHTS, &MIDTONES, axis[l], MIDTONES[DEFAULT_MIDTONE]),
            (_, 3) => at(DARKS, &MIDTONES, axis[d], MIDTONES[DEFAULT_MIDTONE]),
            _ => {
                let i = |v: usize| if v > 3 { v - 1 } else { v };
                table(BOTH + i(d) * JOINT.len() + i(l))
            }
        }
    };
    let (jd, wd) = bracket(&axis, darks.clamp(-1., 1.));
    let (jl, wl) = bracket(&axis, lights.clamp(-1., 1.));
    blend(&[
        (node(jd, jl), (1. - wd) * (1. - wl)),
        (node(jd + 1, jl), wd * (1. - wl)),
        (node(jd, jl + 1), (1. - wd) * wl),
        (node(jd + 1, jl + 1), wd * wl),
    ])
}
fn lookup(t: &Table, x: f32) -> f32 {
    let f = x.clamp(0., 1.) * (SAMPLES - 1) as f32;
    let i = (f as usize).min(SAMPLES - 2);
    t[i] + (t[i + 1] - t[i]) * (f - i as f32)
}

/// The measured curve for one setting of the regions and splits. Camera Raw applies
/// Shadows (below the midtone split) and Highlights (above it) first, then Darks and
/// Lights, all in gamma-2.2 encoded ProPhoto RGB.
struct Shape {
    midtone: f32,
    shadows: Option<Table>,
    highlights: Option<Table>,
    middle: Option<Table>,
}
impl Shape {
    fn new(regions: [f32; 4], splits: [f32; 3]) -> Self {
        let [shadows, darks, lights, highlights] = regions;
        let [low, midtone, high] = splits;
        let middle = match (darks != 0., lights != 0.) {
            (false, false) => None,
            (true, false) => Some(at(DARKS, &MIDTONES, darks, midtone)),
            (false, true) => Some(at(LIGHTS, &MIDTONES, lights, midtone)),
            // Measured together at the default midtone split; a moved split changes
            // each region as it does alone.
            (true, true) => {
                let default = MIDTONES[DEFAULT_MIDTONE];
                let moved = |first, amount| {
                    let (a, b) = (
                        at(first, &MIDTONES, amount, midtone),
                        at(first, &MIDTONES, amount, default),
                    );
                    std::array::from_fn::<f32, SAMPLES, _>(|k| a[k] - b[k])
                };
                let (j, d, l) = (
                    joint(darks, lights),
                    moved(DARKS, darks),
                    moved(LIGHTS, lights),
                );
                Some(std::array::from_fn(|k| j[k] + d[k] + l[k]))
            }
        };
        Self {
            midtone,
            shadows: (shadows != 0.).then(|| at(LOW, &RATIOS, shadows, low / midtone)),
            highlights: (highlights != 0.)
                .then(|| at(HIGH, &RATIOS, highlights, (high - midtone) / (1. - midtone))),
            middle,
        }
    }
    /// The curve at `x` (encoded with the sRGB transfer function, as the pipeline's
    /// curves are): taken to gamma 2.2, through the regions and back.
    fn eval(&self, x: f32) -> f32 {
        use crate::color::{srgb_decode, srgb_encode};
        let w = srgb_decode(x.clamp(0., 1.)).powf(1. / GAMMA);
        let m = self.midtone;
        let mut y = match (&self.shadows, &self.highlights) {
            (Some(t), _) if w < m => m * (w / m + lookup(t, w / m)),
            (_, Some(t)) if w >= m => {
                let u = (w - m) / (1. - m);
                m + (1. - m) * (u + lookup(t, u))
            }
            _ => w,
        };
        if let Some(t) = &self.middle {
            y = y.clamp(0., 1.);
            y += lookup(t, y);
        }
        srgb_encode(y.clamp(0., 1.).powf(GAMMA))
    }
}

/// The encoding the parametric curve works in: gamma 2.2, which fits Camera Raw's curves
/// eight times closer than the sRGB transfer function the point curves use.
const GAMMA: f32 = 2.2;

#[cfg(test)]
mod tests {
    use super::*;

    /// One Camera Raw 18.7 render of the synthetic chart's gray ramp: the default
    /// render's level and the level with these regions and splits (encoded sRGB, 0–1).
    struct RampCase {
        regions: [f32; 4],
        splits: [f32; 3],
        levels: [[f32; 2]; 3],
    }
    fn case(regions: [f32; 4], splits: [f32; 3], levels: [[f32; 2]; 3]) -> RampCase {
        RampCase {
            regions,
            splits,
            levels,
        }
    }

    #[test]
    fn matches_camera_raw_on_the_gray_ramp() {
        let d = [0.25, 0.5, 0.75];
        let cases = [
            case(
                [0., 0., 0.5, 0.],
                d,
                [[0.3045, 0.3341], [0.5873, 0.7045], [0.7847, 0.9158]],
            ),
            case(
                [0., -0.5, 0., 0.],
                d,
                [[0.0995, 0.0152], [0.2372, 0.1202], [0.4798, 0.3911]],
            ),
            case(
                [0.5, 0., 0., 0.],
                d,
                [[0.0733, 0.1094], [0.1345, 0.1816], [0.2372, 0.2827]],
            ),
            case(
                [0., 0., 0., -0.5],
                d,
                [[0.5873, 0.577], [0.7847, 0.7257], [0.9191, 0.8705]],
            ),
            case(
                [-0.3, -0.4, 0.4, 0.3],
                d,
                [[0.1345, 0.0286], [0.3845, 0.3284], [0.7847, 0.9144]],
            ),
            case(
                [0., 0., 0.5, 0.],
                [0.25, 0.35, 0.75],
                [[0.2372, 0.2702], [0.4798, 0.6224], [0.7847, 0.9471]],
            ),
            case(
                [-0.5, 0., 0., 0.],
                [0.4, 0.6, 0.85],
                [[0.0733, 0.0085], [0.1803, 0.0781], [0.3845, 0.324]],
            ),
            case(
                [0., -0.5, 1., 0.],
                d,
                [[0.1803, 0.0878], [0.4798, 0.5604], [0.8605, 0.9977]],
            ),
        ];
        for RampCase {
            regions,
            splits,
            levels,
        } in cases
        {
            let curve = ParametricCurve::new(regions, splits).unwrap();
            for [x, expected] in levels {
                let y = curve.eval(x);
                assert!(
                    (y - expected).abs() < 1.5 / 255.,
                    "{regions:?} {splits:?}: {x} gives {y}, Camera Raw {expected}"
                );
            }
        }
    }

    #[test]
    fn curves_are_monotone_and_keep_black_and_white() {
        assert!(ParametricCurve::new([0.; 4], [0.25, 0.5, 0.75]).is_none());
        for regions in [
            [1., 1., 1., 1.],
            [-1., -1., -1., -1.],
            [0.7, -0.3, 0.6, -0.9],
        ] {
            for splits in [[0.1, 0.2, 0.3], [0.25, 0.5, 0.75], [0.7, 0.8, 0.9]] {
                let c = ParametricCurve::new(regions, splits).unwrap();
                assert!(
                    c.lut.windows(2).all(|p| p[1] >= p[0]),
                    "{regions:?} {splits:?}"
                );
                assert!(c.eval(0.) < 1e-4 && c.eval(1.) > 1. - 1e-4);
            }
        }
    }

    #[test]
    fn regions_change_only_their_side_of_the_midtone_split() {
        let midtone = crate::color::srgb_encode(0.5f32.powf(GAMMA));
        let highlights = ParametricCurve::new([0., 0., 0., 0.8], [0.25, 0.5, 0.75]).unwrap();
        let shadows = ParametricCurve::new([-0.8, 0., 0., 0.], [0.25, 0.5, 0.75]).unwrap();
        for i in 0..=20 {
            let x = i as f32 / 20.;
            let (h, s) = (highlights.eval(x), shadows.eval(x));
            if x < midtone - 0.02 {
                assert!((h - x).abs() < 1e-3, "{x}: {h}");
                assert!(s < x + 1e-3, "{x}: {s}");
            } else if x > midtone + 0.02 {
                assert!((s - x).abs() < 1e-3, "{x}: {s}");
                assert!(h > x - 1e-3, "{x}: {h}");
            }
        }
    }

    #[test]
    fn regions_join_the_identity_at_the_midtone_split() {
        let midtone = crate::color::srgb_encode(0.5f32.powf(GAMMA));
        for regions in [
            [1., 0., 0., 0.],
            [-1., 0., 0., 0.],
            [0., 0., 0., 1.],
            [0., 0., 0., -1.],
        ] {
            for splits in [[0.32, 0.5, 0.75], [0.1, 0.5, 0.6], [0.4, 0.5, 0.9]] {
                let c = ParametricCurve::new(regions, splits).unwrap();
                for x in [midtone - 1e-3, midtone + 1e-3] {
                    assert!(
                        (c.eval(x) - x).abs() < 0.3 / 255.,
                        "{regions:?} {splits:?} {x}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_hue_holds() {
        let c = ParametricCurve::new([0.4, -0.3, 0.6, 0.2], [0.25, 0.5, 0.75]).unwrap();
        let p = c.apply([0.7, 0.4, 0.2]);
        assert!(((p[1] - p[2]) / (p[0] - p[2]) - 0.4).abs() < 1e-5, "{p:?}");
    }
}
