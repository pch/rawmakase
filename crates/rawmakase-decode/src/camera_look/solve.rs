//! Solving a fitted look (tone curve and HSV table) and a camera matrix from the
//! pairs. Every loop runs in a fixed order, so a fit is the same on any machine
//! with any number of threads.
use super::Rgb;
use rawmakase_model::{
    camera_profiles::{
        CameraProfile, default_tone_at,
        fitted::{self, FittedLook, TABLE_DIMS},
    },
    color::{srgb_decode, srgb_encode},
};
use std::sync::OnceLock;

type Matrix = [[f32; 3]; 3];
const LUT: usize = 4096;

fn clamp01(p: Rgb) -> Rgb {
    p.map(|v| if v.is_finite() { v.clamp(0., 1.) } else { 0. })
}
/// A monotone LUT of 4097 samples over 0–1 at `x`, linearly between samples.
fn at(lut: &[f32], x: f32) -> f32 {
    let p = x.clamp(0., 1.) * LUT as f32;
    let i = (p as usize).min(LUT - 1);
    lut[i] + (lut[i + 1] - lut[i]) * (p - i as f32)
}
/// The inverse of the nondecreasing `f` on 0–1, as a LUT: for each output the
/// middle of the inputs that reach it.
fn inverse(f: impl Fn(f32) -> f32) -> Vec<f32> {
    let ys: Vec<f32> = (0..=LUT).map(|i| f(i as f32 / LUT as f32)).collect();
    (0..=LUT)
        .map(|j| {
            let v = j as f32 / LUT as f32;
            let i = ys.partition_point(|y| *y < v);
            if i == 0 {
                return 0.;
            }
            if i > LUT {
                return 1.;
            }
            let (a, b) = (ys[i - 1], ys[i]);
            let t = if b > a { (v - a) / (b - a) } else { 0.5 };
            (i as f32 - 1. + t) / LUT as f32
        })
        .collect()
}
/// The ACR3 base tone's inverse.
fn base_inverse() -> &'static [f32] {
    static LUT: OnceLock<Vec<f32>> = OnceLock::new();
    LUT.get_or_init(|| inverse(default_tone_at))
}
/// `CameraProfile::finish`'s rule for a tone curve: the darkest and brightest
/// channels through `f`, the middle one kept in its place between them.
fn extrema(p: Rgb, f: impl Fn(f32) -> f32) -> Rgb {
    let lo = p.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = p.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let (a, b) = (f(lo), f(hi));
    if hi - lo > 1e-8 {
        p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
    } else {
        [a; 3]
    }
}
/// The colour the look's table must make for the tone curves (the base's, then the
/// look's `curve`, on encoded values) to give `target`.
fn untoned(target: Rgb, curve_inverse: &[f32]) -> Rgb {
    let e = extrema(clamp01(target).map(srgb_encode), |v| at(curve_inverse, v));
    extrema(e.map(srgb_decode), |v| at(base_inverse(), v))
}
/// Hue (0–1), saturation and value as profile tables take them.
fn hsv(p: Rgb) -> Rgb {
    let max = p.iter().copied().fold(0f32, f32::max);
    let min = p.iter().copied().fold(f32::INFINITY, f32::min).max(0.);
    let d = max - min;
    let h = if d < 1e-9 {
        0.
    } else if max == p[0] {
        ((p[1] - p[2]) / d).rem_euclid(6.) / 6.
    } else if max == p[1] {
        ((p[2] - p[0]) / d + 2.) / 6.
    } else {
        ((p[0] - p[1]) / d + 4.) / 6.
    };
    [h, if max > 0. { d / max } else { 0. }, max]
}

/// The look's curve from `knots` of the composite tone (base tone then look curve):
/// linear scene value to linear output, sorted, from (0, 0) to (1, 1).
fn curve_from_composite(knots: &[(f32, f32)]) -> Vec<f32> {
    let mut last = 0f32;
    (0..=LUT)
        .map(|i| {
            let x = at(base_inverse(), srgb_decode(i as f32 / LUT as f32));
            let v = srgb_encode(interpolate(knots, x)).clamp(last, 1.);
            last = v;
            v
        })
        .collect()
}
fn interpolate(knots: &[(f32, f32)], x: f32) -> f32 {
    let i = knots
        .partition_point(|k| k.0 <= x)
        .clamp(1, knots.len() - 1);
    let (a, b) = (knots[i - 1], knots[i]);
    if b.0 > a.0 {
        a.1 + (b.1 - a.1) * ((x - a.0) / (b.0 - a.0)).clamp(0., 1.)
    } else {
        b.1
    }
}
/// Knots from (0, 0) to (1, 1) through `points` (sorted by x), each y at least the
/// one before: monotone, the curve's ends fixed.
fn monotone_knots(points: impl IntoIterator<Item = (f32, f32)>) -> Vec<(f32, f32)> {
    let mut knots = vec![(0f32, 0f32)];
    for (x, y) in points {
        let last = *knots.last().expect("starts at 0");
        if x > last.0 + 1e-5 && x < 1. - 1e-5 {
            knots.push((x, y.clamp(last.1, 1.)));
        }
    }
    knots.push((1., 1.));
    knots
}
/// The `q` quantile of sorted `v`.
fn quantile(v: &[f32], q: f32) -> f32 {
    let i = (q * (v.len() - 1) as f32).round() as usize;
    v[i.min(v.len() - 1)]
}

/// The starting curve: the i-th quantile of the scene's value (the brightest
/// channel, which the tone curves map) goes to the i-th quantile of the preview's,
/// at 32 knots.
pub(super) fn quantile_curve(scene: &[Rgb], target: &[Rgb]) -> Vec<f32> {
    let values = |v: &[Rgb], f: fn(Rgb) -> f32| {
        let mut out: Vec<f32> = v.iter().map(|p| f(clamp01(*p))).collect();
        out.sort_by(f32::total_cmp);
        out
    };
    let brightest = |p: Rgb| p[0].max(p[1]).max(p[2]);
    let (xs, ys) = (values(scene, brightest), values(target, brightest));
    curve_from_composite(&monotone_knots(
        (1..32).map(|k| (quantile(&xs, k as f32 / 32.), quantile(&ys, k as f32 / 32.))),
    ))
}
/// The starting curve of a black and white look: luminance quantiles.
pub(super) fn luminance_curve(scene: &[Rgb], target: &[Rgb]) -> Vec<f32> {
    let values = |v: &[Rgb]| {
        let mut out: Vec<f32> = v
            .iter()
            .map(|p| super::pairs::luminance(clamp01(*p)))
            .collect();
        out.sort_by(f32::total_cmp);
        out
    };
    let (xs, ys) = (values(scene), values(target));
    curve_from_composite(&monotone_knots(
        (1..32).map(|k| (quantile(&xs, k as f32 / 32.), quantile(&ys, k as f32 / 32.))),
    ))
}

/// The widest bin of the refitted curve (encoded), and the least weight it closes with.
const BIN_WIDTH: f32 = 0.06;
const BIN_LEAST: f64 = 8.;

/// The curve again for the table `look` has: the encoded extrema the look's curve
/// receives against the preview's, binned into 32 equal-weight bins, narrower where
/// [`BIN_WIDTH`] says, made monotone (pool adjacent violators).
fn refit_curve(base: &CameraProfile, look: &FittedLook, scene: &[Rgb], target: &[Rgb]) -> Vec<f32> {
    let profile = fitted::with_look(base, look).expect("a valid look");
    let mut points: Vec<(f32, f32, f32)> = Vec::with_capacity(scene.len() * 2);
    for (s, t) in scene.iter().zip(target) {
        let a = fitted::table_output(&profile, clamp01(*s));
        let toned = extrema(a, default_tone_at).map(srgb_encode);
        let t = clamp01(*t).map(srgb_encode);
        let lo = |p: Rgb| p.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = |p: Rgb| p.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        points.push((hi(toned), hi(t), 1.));
        points.push((lo(toned), lo(t), 0.5));
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f32 = points.iter().map(|p| p.2).sum();
    let mut bins: Vec<[f64; 3]> = Vec::new();
    let (mut acc, mut next) = ([0f64; 3], total / 32.);
    let mut seen = 0f32;
    let mut first = 0f32;
    for (u, y, w) in points {
        if acc[2] == 0. {
            first = u;
        }
        let w64 = f64::from(w);
        acc = [
            acc[0] + w64 * f64::from(u),
            acc[1] + w64 * f64::from(y),
            acc[2] + w64,
        ];
        seen += w;
        // A bin spans a limited range too: few pixels in the shadows must still set them.
        if seen >= next || (u - first > BIN_WIDTH && acc[2] >= BIN_LEAST) {
            bins.push(acc);
            acc = [0.; 3];
            next = seen + total / 32.;
        }
    }
    if acc[2] > 0. {
        bins.push(acc);
    }
    // Pool adjacent violators over the bins' means.
    let mut pooled: Vec<[f64; 3]> = Vec::new();
    for b in bins {
        pooled.push(b);
        while pooled.len() > 1 {
            let n = pooled.len();
            let (p, q) = (pooled[n - 2], pooled[n - 1]);
            if p[1] / p[2] <= q[1] / q[2] {
                break;
            }
            pooled.truncate(n - 2);
            pooled.push([p[0] + q[0], p[1] + q[1], p[2] + q[2]]);
        }
    }
    let knots = monotone_knots(
        pooled
            .iter()
            .map(|b| ((b[0] / b[2]) as f32, (b[1] / b[2]) as f32)),
    );
    let mut last = 0f32;
    (0..=LUT)
        .map(|i| {
            let v = interpolate(&knots, i as f32 / LUT as f32).clamp(last, 1.);
            last = v;
            v
        })
        .collect()
}

/// The most a node may shift hue (degrees), and scale saturation and value (as a
/// natural logarithm).
const LIMITS: [f32; 3] = [30., 0.7, 0.7];
/// How firmly neighbouring nodes are tied to each other, against a sample's weight of
/// about 1, and how firmly each node is held to no change, as a share of that.
const SMOOTHNESS: f64 = 3.;
const NO_CHANGE: f64 = 0.05;
/// The most a sample weighs, and the least lightness (encoded value over 0.5) it is
/// weighed by, so that a dark photo's colours still count.
const WEIGHT_MOST: f64 = 9.;
const WEIGHT_DARKEST: f32 = 0.2;

/// The look table's nodes as the solve sees them: which lie beside which, and how
/// firmly neighbours are tied. The neutral column is fixed at no change and takes no
/// part.
///
/// The ties are those of a smooth look over the cylinder of hue, saturation and
/// value. Neighbouring hues of little saturation are nearly one colour, and are tied
/// the more firmly: nodes near grey are learned from few pixels, mostly of soft
/// edges, and a zigzag between them shows as bands around such an edge. Along
/// saturation and value the ties weaken towards grey, as the cells there shrink.
struct Nodes {
    dims: [usize; 3],
    /// Each node's 27 neighbours (itself among them), by offset: hue, saturation and
    /// value each one below, at or one above; `NONE` outside the table and on the
    /// neutral column.
    beside: Vec<[u32; 27]>,
}
const NONE: u32 = u32::MAX;
impl Nodes {
    fn new() -> Self {
        let dims = TABLE_DIMS;
        let [nh, ns, nv] = dims;
        let index = |h: usize, s: usize, v: usize| (v * nh + h) * ns + s;
        let beside = (0..nh * ns * nv)
            .map(|node| {
                let (s, h, v) = (node % ns, node / ns % nh, node / ns / nh);
                std::array::from_fn(|k| {
                    let d = [k as i32 % 3 - 1, k as i32 / 3 % 3 - 1, k as i32 / 9 - 1];
                    let (is, iv) = (s as i32 + d[1], v as i32 + d[2]);
                    if is < 1 || is >= ns as i32 || iv < 0 || iv >= nv as i32 {
                        return NONE;
                    }
                    let ih = (h as i32 + d[0]).rem_euclid(nh as i32) as usize;
                    index(ih, is as usize, iv as usize) as u32
                })
            })
            .collect();
        Self { dims, beside }
    }
    /// The offset (see `beside`) from a cell's corner `a` to its corner `b`, each 0 or
    /// 1 along hue, saturation and value.
    fn offset(a: [usize; 3], b: [usize; 3]) -> usize {
        let d = |c: usize| b[c] + 1 - a[c];
        d(2) * 9 + d(1) * 3 + d(0)
    }
    /// The steps between nodes along hue (radians), saturation and value.
    fn steps(&self) -> [f64; 3] {
        let [nh, ns, nv] = self.dims;
        [
            std::f64::consts::TAU / nh as f64,
            1. / (ns - 1) as f64,
            1. / (nv - 1) as f64,
        ]
    }
    /// How firmly `node` is held to no change: its cell's volume.
    fn hold(&self, node: usize) -> f64 {
        let [dh, ds, dv] = self.steps();
        let s = (node % self.dims[1]) as f64;
        NO_CHANGE * SMOOTHNESS * dh * s * ds * ds * dv
    }
    /// The ties of `node` for `channel` (0 hue, 1 saturation, 2 value): each
    /// neighbour and the tie's weight, the cell's face between them over their
    /// distance. No neighbour is the neutral column's no change: saturation and value
    /// end there, no more firmly than they are tied to each other (a value scale held
    /// at 1 beside grey left a pale colour lighter than the camera made it, and the
    /// soft edge between two colours, which passes through pale ones, with a ring);
    /// a hue shift has nothing to meet.
    fn ties(&self, channel: usize, node: usize) -> [(u32, f64); 6] {
        let [_, ns, nv] = self.dims;
        let [dh, ds, dv] = self.steps();
        let (s, v) = (node % ns, node / ns / self.dims[0]);
        let radius = |s: f64| s * ds;
        let hue = SMOOTHNESS * ds * dv / (dh * radius(s as f64));
        let saturation = |low: usize| SMOOTHNESS * dh * radius(low as f64 + 0.5) * dv / ds;
        let value = SMOOTHNESS * dh * radius(s as f64) * ds / dv;
        let to_grey = if channel == 0 { 0. } else { saturation(0) };
        let beside = |d: [usize; 3]| self.beside[node][Self::offset([1, 1, 1], d)];
        [
            (beside([2, 1, 1]), hue),
            (beside([0, 1, 1]), hue),
            match s > 1 {
                true => (beside([1, 0, 1]), saturation(s - 1)),
                false => (NONE, to_grey),
            },
            (
                beside([1, 2, 1]),
                if s + 1 < ns { saturation(s) } else { 0. },
            ),
            (beside([1, 1, 0]), if v > 0 { value } else { 0. }),
            (beside([1, 1, 2]), if v + 1 < nv { value } else { 0. }),
        ]
    }
    /// What the samples' side `data` (per node, by offset), the ties and the holds make
    /// of `x`.
    fn apply(&self, data: &[[f64; 27]], channel: usize, x: &[f64], out: &mut [f64]) {
        for (node, out) in out.iter_mut().enumerate() {
            if node % self.dims[1] == 0 {
                *out = x[node];
                continue;
            }
            let mut sum = self.hold(node) * x[node];
            for (coefficient, other) in data[node].iter().zip(&self.beside[node]) {
                if *other != NONE {
                    sum += coefficient * x[*other as usize];
                }
            }
            for (other, weight) in self.ties(channel, node) {
                let there = if other == NONE { 0. } else { x[other as usize] };
                sum += weight * (x[node] - there);
            }
            *out = sum;
        }
    }
    /// The table's `channel` the samples ask for: from `now`, the least squares of
    /// their remaining changes (`asked`, already gathered onto the nodes), the ties
    /// and the holds, by conjugate gradients preconditioned with the diagonal. The
    /// same steps in the same order on any machine.
    fn solve(&self, data: &[[f64; 27]], asked: &[f64], now: &[f64], channel: usize) -> Vec<f64> {
        let n = now.len();
        let ns = self.dims[1];
        let mut scratch = vec![0f64; n];
        // The right-hand side: what the samples make of the table as it is, and the
        // change they still ask for.
        let mut b = vec![0f64; n];
        for node in (0..n).filter(|node| node % ns != 0) {
            b[node] = asked[node];
            for (coefficient, other) in data[node].iter().zip(&self.beside[node]) {
                if *other != NONE {
                    b[node] += coefficient * now[*other as usize];
                }
            }
        }
        let diagonal: Vec<f64> = (0..n)
            .map(|node| match node % ns {
                0 => 1.,
                _ => {
                    let ties: f64 = self.ties(channel, node).iter().map(|t| t.1).sum();
                    data[node][13] + self.hold(node) + ties
                }
            })
            .collect();
        let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
        let mut x = now.to_vec();
        self.apply(data, channel, &x, &mut scratch);
        let mut r: Vec<f64> = b.iter().zip(&scratch).map(|(b, m)| b - m).collect();
        let mut z: Vec<f64> = r.iter().zip(&diagonal).map(|(r, d)| r / d).collect();
        let mut p = z.clone();
        let mut rz = dot(&r, &z);
        let small = 1e-18 * dot(&b, &b).max(1e-30);
        for _ in 0..400 {
            if dot(&r, &r) <= small {
                break;
            }
            self.apply(data, channel, &p, &mut scratch);
            let curvature = dot(&p, &scratch);
            if curvature <= 0. {
                break;
            }
            let alpha = rz / curvature;
            for node in 0..n {
                x[node] += alpha * p[node];
                r[node] -= alpha * scratch[node];
                z[node] = r[node] / diagonal[node];
            }
            let next = dot(&r, &z);
            let beta = next / rz;
            rz = next;
            for node in 0..n {
                p[node] = z[node] + beta * p[node];
            }
        }
        x
    }
}

/// One round of the table, by weighted least squares. Each sample asks the eight
/// nodes around where it enters the table, by the table's own interpolation, for the
/// hue, saturation and value change that remains between what the current table makes
/// of it and what the curves need. Neighbouring nodes are tied ([`Nodes`]), so a
/// colour the photo has little of is still learned, and the nodes around it follow
/// instead of staying unchanged beside it: a face among trees is a few hundred
/// samples, and kernel averages shrunk towards no change left it grey.
fn refine_table(base: &CameraProfile, look: &mut FittedLook, scene: &[Rgb], target: &[Rgb]) {
    let profile = fitted::with_look(base, look).expect("a valid look");
    let curve_inverse = inverse(|v| at(&look.curve, v));
    let nodes = Nodes::new();
    let [nh, ns, nv] = nodes.dims;
    let n = nh * ns * nv;
    // The samples' side of the normal equations, for hue and saturation and for value
    // (their weights differ), and the changes asked for, gathered onto the nodes.
    let mut data = [vec![[0f64; 27]; n], vec![[0f64; 27]; n]];
    let mut asked = [vec![0f64; n], vec![0f64; n], vec![0f64; n]];
    for (s, t) in scene.iter().zip(target) {
        let input = clamp01(*s);
        let [h, sat, v] = hsv(input);
        let made = hsv(fitted::table_output(&profile, input));
        let wanted = hsv(untoned(*t, &curve_inverse));
        let d_hue = ((wanted[0] - made[0] + 0.5).rem_euclid(1.) - 0.5) * 360.;
        // Scales are asked for as differences, not logarithms: among noisy bright
        // pixels it is their mean the table should make, not their geometric mean.
        let colour = (sat > 0.05 && made[1] > 0.02 && wanted[1] > 0.)
            .then(|| [f64::from(d_hue), f64::from(wanted[1] / made[1] - 1.)]);
        let value = (made[2] > 1e-3 && wanted[2] > 1e-4)
            .then(|| f64::from(srgb_encode(wanted[2]) / srgb_encode(made[2]) - 1.));
        let encoded = srgb_encode(v).clamp(0., 1.);
        // A change of hue or saturation shows as much as the colour is saturated and
        // light, one of value as much as it is light.
        let light = f64::from((encoded / 0.5).max(WEIGHT_DARKEST)).powi(2);
        let weights = [
            (f64::from(sat / 0.3).powi(2) * light).min(WEIGHT_MOST),
            light,
        ];
        // The cell, as `Table::lookup` finds it.
        let place = [
            h.rem_euclid(1.) * nh as f32,
            sat.clamp(0., 1.) * (ns - 1) as f32,
            encoded * (nv - 1) as f32,
        ];
        let cell = place.map(|c| c.floor() as usize);
        let within: [f32; 3] = std::array::from_fn(|c| place[c] - cell[c] as f32);
        let corners: [(usize, [usize; 3], f64); 8] = std::array::from_fn(|i| {
            let o = [i & 1, i >> 1 & 1, i >> 2 & 1];
            let (ih, is, iv) = (
                (cell[0] + o[0]) % nh,
                (cell[1] + o[1]).min(ns - 1),
                (cell[2] + o[2]).min(nv - 1),
            );
            let share: f32 = (0..3)
                .map(|c| if o[c] == 1 { within[c] } else { 1. - within[c] })
                .product();
            // The neutral column is fixed: its share asks nothing.
            let share = if is == 0 { 0. } else { f64::from(share) };
            ((iv * nh + ih) * ns + is, o, share)
        });
        for (node, a, share) in corners {
            if share == 0. {
                continue;
            }
            if let Some([d_hue, d_saturation]) = colour {
                asked[0][node] += weights[0] * share * d_hue;
                asked[1][node] += weights[0] * share * d_saturation;
            }
            if let Some(d_value) = value {
                asked[2][node] += weights[1] * share * d_value;
            }
            for (_, b, other) in corners {
                let k = Nodes::offset(a, b);
                if colour.is_some() {
                    data[0][node][k] += weights[0] * share * other;
                }
                if value.is_some() {
                    data[1][node][k] += weights[1] * share * other;
                }
            }
        }
    }
    let now = |channel: usize| -> Vec<f64> {
        look.table
            .iter()
            .map(|e| match channel {
                0 => f64::from(e[0]),
                c => f64::from(e[c].ln()),
            })
            .collect()
    };
    let solved: [Vec<f64>; 3] = std::array::from_fn(|channel| {
        let data = &data[usize::from(channel == 2)];
        nodes.solve(data, &asked[channel], &now(channel), channel)
    });
    for (node, entry) in look.table.iter_mut().enumerate() {
        if node % ns == 0 {
            // The neutral column stays as it is: grey's tone is the curve's.
            *entry = [0., 1., 1.];
            continue;
        }
        let next: [f32; 3] =
            std::array::from_fn(|c| (solved[c][node] as f32).clamp(-LIMITS[c], LIMITS[c]));
        *entry = [next[0], next[1].exp(), next[2].exp()];
    }
    limit_slopes(&mut look.table);
}
/// The most a node may differ from its neighbour along hue, saturation and value, for
/// the hue shift (degrees) and the logarithms of the saturation and value scales: a
/// bound on what the solve may make of contradictory samples (skin in two lights,
/// nearly one colour and asked for very different ones), which would bend a gradient
/// into contours. The smoothness of a look is the solve's ties'; these are three times
/// what kernel averages needed, which held a small face's colours back.
const SLOPES: [[f32; 3]; 3] = [[13.5, 15., 15.], [0.24, 0.36, 0.36], [0.12, 0.36, 0.36]];
/// The most the value scale of the least saturated nodes may differ from grey's 1 (as a
/// logarithm): as much as from a neighbouring hue's. Saturation is the least steady
/// thing about a dark or pale colour, so a value scale that leaves grey steeply turns
/// noise and soft edges there into contours (a dark light out of focus, lightened by
/// 1.4 where its noise read as green and darkened beside it). What a camera does to
/// pale colours is within it: 5% between coloured stage lights.
const GREY_SLOPE: f32 = 0.12;

/// The closest table, in sweeps along each axis, whose neighbouring nodes respect
/// [`SLOPES`]; the neutral column stays as it is.
fn limit_slopes(table: &mut [[f32; 3]]) {
    let [nh, ns, nv] = TABLE_DIMS;
    let index = |h: usize, s: usize, v: usize| (v * nh + h) * ns + s;
    let log = |e: [f32; 3]| [e[0], e[1].ln(), e[2].ln()];
    let mut logged: Vec<[f32; 3]> = table.iter().map(|e| log(*e)).collect();
    let limit = |to: &mut f32, from: f32, slope: f32| *to = to.clamp(from - slope, from + slope);
    for _ in 0..2 {
        for c in 0..3 {
            for v in 0..nv {
                for s in 1..ns {
                    // Twice around: hue is a circle.
                    for h in 0..2 * nh {
                        let (a, b) = (index(h % nh, s, v), index((h + 1) % nh, s, v));
                        let from = logged[a][c];
                        limit(&mut logged[b][c], from, SLOPES[c][0]);
                    }
                    for h in (0..2 * nh).rev() {
                        let (a, b) = (index((h + 1) % nh, s, v), index(h % nh, s, v));
                        let from = logged[a][c];
                        limit(&mut logged[b][c], from, SLOPES[c][0]);
                    }
                }
            }
            for v in 0..nv {
                for h in 0..nh {
                    for s in 1..ns {
                        let from = logged[index(h, s - 1, v)][c];
                        let slope = if (c, s) == (2, 1) {
                            GREY_SLOPE
                        } else {
                            SLOPES[c][1]
                        };
                        limit(&mut logged[index(h, s, v)][c], from, slope);
                    }
                    for s in (1..ns - 1).rev() {
                        let from = logged[index(h, s + 1, v)][c];
                        limit(&mut logged[index(h, s, v)][c], from, SLOPES[c][1]);
                    }
                }
            }
            for h in 0..nh {
                for s in 1..ns {
                    for v in 1..nv {
                        let from = logged[index(h, s, v - 1)][c];
                        limit(&mut logged[index(h, s, v)][c], from, SLOPES[c][2]);
                    }
                    for v in (0..nv - 1).rev() {
                        let from = logged[index(h, s, v + 1)][c];
                        limit(&mut logged[index(h, s, v)][c], from, SLOPES[c][2]);
                    }
                }
            }
        }
    }
    for (entry, l) in table.iter_mut().zip(&logged) {
        *entry = [l[0], l[1].exp(), l[2].exp()];
    }
}

/// Most and least a channel of grey is moved, and the fewest grey pairs to judge by.
const GREY_MOST: f32 = 0.05;
const GREY_LEAST: f32 = 0.002;
const GREY_PAIRS: usize = 64;

/// The gains (geometric mean 1) on linear ProPhoto that take the greys of `scene` to
/// those of `target`, where both are near neutral and neither dark nor bright; `None`
/// with too few of them or no difference worth a change.
pub(super) fn grey_gains(scene: &[Rgb], target: &[Rgb]) -> Option<[f32; 3]> {
    let tint = |p: &Rgb| {
        let max = p.iter().copied().fold(f32::MIN, f32::max);
        let min = p.iter().copied().fold(f32::MAX, f32::min);
        let mean = (p[0] + p[1] + p[2]) / 3.;
        (min > 0. && (max - min) / max < 0.08 && (0.05..0.9).contains(&mean))
            .then(|| p.map(|v| (v / mean).ln()))
    };
    let (mut sum, mut n) = ([0f64; 3], 0usize);
    for (s, t) in scene.iter().zip(target) {
        if let (Some(s), Some(t)) = (tint(s), tint(t)) {
            (0..3).for_each(|c| sum[c] += f64::from(t[c] - s[c]));
            n += 1;
        }
    }
    if n < GREY_PAIRS {
        return None;
    }
    let mean = sum.map(|v| (v / n as f64) as f32);
    let centre = (mean[0] + mean[1] + mean[2]) / 3.;
    let log = mean.map(|v| v - centre);
    log.iter()
        .any(|v| v.abs() >= GREY_LEAST)
        .then(|| log.map(|v| v.clamp(-GREY_MOST, GREY_MOST).exp()))
}

/// The look over `base` taking `scene` (linear ProPhoto leaving the scene tone
/// stage) to `target` (the preview's linear ProPhoto): the quantile curve, then
/// alternately the table and the curve through the profile's own operators.
pub(super) fn colour_look(base: &CameraProfile, scene: &[Rgb], target: &[Rgb]) -> FittedLook {
    let mut look = FittedLook {
        curve: quantile_curve(scene, target),
        ..FittedLook::identity()
    };
    for round in 0..3 {
        for _ in 0..2 {
            refine_table(base, &mut look, scene, target);
        }
        if round < 2 {
            look.curve = refit_curve(base, &look, scene, target);
        }
    }
    look
}
/// A look of the curve alone, the table unchanged.
pub(super) fn tone_look(base: &CameraProfile, scene: &[Rgb], target: &[Rgb]) -> FittedLook {
    let mut look = FittedLook {
        curve: quantile_curve(scene, target),
        ..FittedLook::identity()
    };
    look.curve = refit_curve(base, &look, scene, target);
    look
}
/// A black and white look: a luminance curve, and monochrome.
pub(super) fn monochrome_look(scene: &[Rgb], target: &[Rgb]) -> FittedLook {
    FittedLook {
        curve: luminance_curve(scene, target),
        monochrome: true,
        ..FittedLook::identity()
    }
}

/// D50's white, where a forward matrix takes the camera's neutral.
const D50: [f64; 3] = [0.9642, 1., 0.8249];

/// A forward matrix from white-balanced camera RGB to XYZ D50, fitted to the
/// preview's colour. The camera's tone curve changes how bright each colour is, not
/// its direction, so each fitted colour is asked only to point where the preview's
/// does (their cross product, of both at unit length, is least), with a ridge of
/// 10⁻⁴ of the data's trace, and the matrix must take camera neutral [1, 1, 1] to
/// D50 white with Y = 1, as a DNG ForwardMatrix does. Refused when a coefficient
/// reaches 8 or the matrix is badly conditioned (condition number 10⁶).
pub(super) fn forward_matrix(camera: &[Rgb], xyz: &[Rgb]) -> Result<Matrix, String> {
    let unit = |v: Rgb| {
        let v = v.map(f64::from);
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        v.map(|x| x / n)
    };
    let pairs: Vec<([f64; 3], [f64; 3])> = camera
        .iter()
        .zip(xyz)
        .filter(|(c, t)| c.iter().all(|v| *v > 1e-4) && t[1] > 1e-3)
        .map(|(c, t)| (unit(*c), unit(*t)))
        .collect();
    if pairs.len() < 64 {
        return Err("Too few colours to fit a matrix".into());
    }
    // m, row by row, as x[3r + j]; each sample's (m c) × t is linear in x.
    let mut h = [[0f64; 9]; 9];
    for (c, t) in &pairs {
        let mut rows = [[0f64; 9]; 3];
        for j in 0..3 {
            rows[0][3 + j] = c[j] * t[2];
            rows[0][6 + j] = -c[j] * t[1];
            rows[1][6 + j] = c[j] * t[0];
            rows[1][j] = -c[j] * t[2];
            rows[2][j] = c[j] * t[1];
            rows[2][3 + j] = -c[j] * t[0];
        }
        for row in &rows {
            for i in 0..9 {
                for k in 0..9 {
                    h[i][k] += row[i] * row[k];
                }
            }
        }
    }
    let ridge = 1e-4 * (0..9).map(|i| h[i][i]).sum::<f64>();
    // Minimise xᵀ H x with each row summing to D50's: the KKT system.
    let mut kkt = vec![vec![0f64; 13]; 12];
    for i in 0..9 {
        for k in 0..9 {
            kkt[i][k] = h[i][k] + if i == k { ridge } else { 0. };
        }
    }
    for r in 0..3 {
        for j in 0..3 {
            kkt[9 + r][3 * r + j] = 1.;
            kkt[3 * r + j][9 + r] = 1.;
        }
        kkt[9 + r][12] = D50[r];
    }
    let x = gauss_solve(kkt).ok_or("Singular colour data")?;
    let m: [[f64; 3]; 3] = std::array::from_fn(|r| std::array::from_fn(|j| x[3 * r + j]));
    if !m.iter().flatten().all(|v| v.is_finite() && v.abs() < 8.) {
        return Err("The fitted matrix has a coefficient beyond 8".into());
    }
    let inv = invert(m).ok_or("The fitted matrix is singular")?;
    let norm = |m: &[[f64; 3]; 3]| m.iter().flatten().map(|v| v * v).sum::<f64>().sqrt();
    if norm(&m) * norm(&inv) >= 1e6 {
        return Err("The fitted matrix is badly conditioned".into());
    }
    Ok(m.map(|r| r.map(|v| v as f32)))
}
/// The solution of the augmented system `a` (each row its coefficients, then the
/// right-hand side), by Gaussian elimination with partial pivoting.
fn gauss_solve(mut a: Vec<Vec<f64>>) -> Option<Vec<f64>> {
    let n = a.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|i, j| a[*i][col].abs().total_cmp(&a[*j][col].abs()))?;
        if a[pivot][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, pivot);
        let (top, rest) = a.split_at_mut(col + 1);
        let pivot_row = &top[col];
        for row in rest {
            let f = row[col] / pivot_row[col];
            for (v, p) in row[col..].iter_mut().zip(&pivot_row[col..]) {
                *v -= f * p;
            }
        }
    }
    let mut x = vec![0f64; n];
    for i in (0..n).rev() {
        let sum: f64 = (i + 1..n).map(|k| a[i][k] * x[k]).sum();
        x[i] = (a[i][n] - sum) / a[i][i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}
fn invert(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let [a, b, c] = m;
    let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0]);
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let cof =
        |r0: usize, r1: usize, c0: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    Some([
        [
            cof(1, 2, 1, 2) / det,
            -cof(0, 2, 1, 2) / det,
            cof(0, 1, 1, 2) / det,
        ],
        [
            -cof(1, 2, 0, 2) / det,
            cof(0, 2, 0, 2) / det,
            -cof(0, 1, 0, 2) / det,
        ],
        [
            cof(1, 2, 0, 1) / det,
            -cof(0, 2, 0, 1) / det,
            cof(0, 1, 0, 1) / det,
        ],
    ])
}

/// The unconstrained reference fit the representation is measured against: the
/// fitted look's render plus a free correction of its remaining error on a 17³
/// lattice over the encoded scene colour, by Gaussian kernels one node wide and
/// shrunk towards no correction. The look's representation (an HSV table and
/// extrema curves) cannot make what the correction adds.
pub(super) struct Lattice {
    profile: CameraProfile,
    data: Vec<Rgb>,
}
const LATTICE: usize = 17;
impl Lattice {
    pub(super) fn fit(profile: &CameraProfile, scene: &[Rgb], target_srgb: &[Rgb]) -> Self {
        let n = LATTICE;
        let step = (n - 1) as f32;
        let mut sums = vec![[0f64; 4]; n * n * n];
        for (s, t) in scene.iter().zip(target_srgb) {
            let made = clamp01(profile.finish(*s));
            let t = clamp01(*t);
            let at = clamp01(*s).map(|v| srgb_encode(v) * step);
            let centre = at.map(|v| v.round() as i32);
            for dz in -2..=2 {
                for dy in -2..=2 {
                    for dx in -2..=2 {
                        let k = [centre[0] + dx, centre[1] + dy, centre[2] + dz];
                        if k.iter().any(|v| *v < 0 || *v >= n as i32) {
                            continue;
                        }
                        let d2: f32 = (0..3).map(|c| (k[c] as f32 - at[c]).powi(2)).sum();
                        let w = f64::from((-0.5 * d2).exp());
                        let i = (k[2] as usize * n + k[1] as usize) * n + k[0] as usize;
                        for c in 0..3 {
                            sums[i][c] += w * f64::from(t[c] - made[c]);
                        }
                        sums[i][3] += w;
                    }
                }
            }
        }
        let data = sums
            .iter()
            .map(|s| std::array::from_fn(|c| (s[c] / (s[3] + 2.)) as f32))
            .collect();
        Self {
            profile: profile.clone(),
            data,
        }
    }
    /// The look's render of scene colour `s` with the correction there, trilinearly.
    pub(super) fn apply(&self, s: Rgb) -> Rgb {
        let n = LATTICE;
        let at = clamp01(s).map(|v| srgb_encode(v) * (n - 1) as f32);
        let base = at.map(|v| (v.floor() as usize).min(n - 2));
        let f: [f32; 3] = std::array::from_fn(|c| at[c] - base[c] as f32);
        let mut out = clamp01(self.profile.finish(s));
        for corner in 0..8 {
            let o = [corner & 1, corner >> 1 & 1, corner >> 2 & 1];
            let w: f32 = (0..3)
                .map(|c| if o[c] == 1 { f[c] } else { 1. - f[c] })
                .product();
            let i = ((base[2] + o[2]) * n + base[1] + o[1]) * n + base[0] + o[0];
            for (o, v) in out.iter_mut().zip(self.data[i]) {
                *o += w * v;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A few dark pixels among many bright ones: equal-weight bins alone would draw one
    /// line from black to the bright tones and lift the blacks.
    #[test]
    fn a_few_dark_pixels_still_set_the_blacks() {
        let base = base();
        let look = FittedLook::identity();
        let dark = (0..40).map(|i| 0.004 + i as f32 / 40. * 0.004);
        let bright = (0..4000).map(|i| 0.3 + i as f32 / 4000. * 0.3);
        let scene: Vec<Rgb> = dark.chain(bright).map(|v| [v; 3]).collect();
        let profile = fitted::with_look(&base, &look).unwrap();
        // A camera deeper in its blacks than the profile, and the same elsewhere.
        let target: Vec<Rgb> = scene
            .iter()
            .map(|s| {
                let made = rawmakase_model::color::mul(
                    rawmakase_model::camera_profiles::RGB_TO_PRO,
                    profile.finish(*s),
                );
                if s[0] < 0.1 {
                    made.map(|v| v * 0.5)
                } else {
                    made
                }
            })
            .collect();
        let curve = refit_curve(&base, &look, &scene, &target);
        let toned = |s: Rgb| {
            let a = fitted::table_output(&profile, s);
            srgb_encode(default_tone_at(a[0]))
        };
        let x = toned(scene[20]);
        let wanted = srgb_encode(target[20][0]);
        assert!(
            (at(&curve, x) - wanted).abs() < 0.02,
            "{} {wanted}",
            at(&curve, x)
        );
    }

    /// The bands around a soft edge: neighbouring hues near grey, each learned from a
    /// few pixels that disagree, come out together; plenty of pixels keep their own.
    #[test]
    fn hues_near_grey_are_drawn_together_unless_plenty_was_learned() {
        let base = base();
        let [nh, ns, _] = TABLE_DIMS;
        // Pale colours on every other hue node, asked for a hue shift of +6° on one
        // node and -6° on the next: a zigzag.
        let pale = |count: usize| -> (Vec<Rgb>, Vec<Rgb>) {
            let mut look = FittedLook::identity();
            for (node, entry) in look.table.iter_mut().enumerate() {
                let hue = node / ns % nh;
                *entry = [if hue.is_multiple_of(2) { 6. } else { -6. }, 1., 1.];
            }
            let camera = fitted::with_look(&base, &look).unwrap();
            let scene: Vec<Rgb> = (0..nh * count)
                .map(|i| {
                    let hue = (i % nh) as f32 / nh as f32 * std::f32::consts::TAU;
                    let third = std::f32::consts::TAU / 3.;
                    [hue, hue - third, hue + third].map(|a| 0.3 * (1. + 0.12 * a.cos()))
                })
                .collect();
            let target = scene
                .iter()
                .map(|s| {
                    rawmakase_model::color::mul(
                        rawmakase_model::camera_profiles::RGB_TO_PRO,
                        camera.finish(*s),
                    )
                })
                .collect();
            (scene, target)
        };
        let zigzag = |count: usize| {
            let (scene, target) = pale(count);
            let mut look = FittedLook::identity();
            refine_table(&base, &mut look, &scene, &target);
            let shifts: Vec<f32> = (0..nh)
                .map(|h| look.table[(4 * nh + h) * ns + 1][0])
                .collect();
            (0..nh)
                .map(|h| (shifts[h] - shifts[(h + 1) % nh]).abs())
                .sum::<f32>()
                / nh as f32
        };
        let (few, plenty) = (zigzag(1), zigzag(400));
        assert!(few < 1., "{few}");
        assert!(plenty > 4. * few, "{few} {plenty}");
    }

    /// The mean of what `look` makes of `colours` over what the camera did, less 1.
    fn off(base: &CameraProfile, look: &FittedLook, colours: &[Rgb], camera: &[Rgb]) -> f32 {
        let p = fitted::with_look(base, look).unwrap();
        let sum: f32 = colours
            .iter()
            .zip(camera)
            .map(|(s, t)| {
                let got = rawmakase_model::color::mul(
                    rawmakase_model::camera_profiles::RGB_TO_PRO,
                    p.finish(*s),
                );
                got[0].max(got[1]).max(got[2]) / t[0].max(t[1]).max(t[2])
            })
            .sum();
        sum / colours.len() as f32 - 1.
    }
    /// Colours of every hue and one small saturation, at one value.
    fn pale(saturation: f32, value: f32, of_each: usize) -> Vec<Rgb> {
        let nh = TABLE_DIMS[0];
        let third = std::f32::consts::TAU / 3.;
        (0..nh * of_each)
            .map(|i| {
                let hue = (i % nh) as f32 / nh as f32 * std::f32::consts::TAU;
                [hue, hue - third, hue + third].map(|a| value * (1. + saturation * a.cos()))
            })
            .collect()
    }
    /// What the profile without a look makes of `scene`, each colour scaled by `by`.
    fn scaled(base: &CameraProfile, scene: &[Rgb], by: impl Fn(usize) -> f32) -> Vec<Rgb> {
        let plain = fitted::with_look(base, &FittedLook::identity()).unwrap();
        scene
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let made = rawmakase_model::color::mul(
                    rawmakase_model::camera_profiles::RGB_TO_PRO,
                    plain.finish(*s),
                );
                made.map(|v| v * by(i))
            })
            .collect()
    }

    /// The soft edge between two coloured lights passes through pale colours, which a
    /// camera makes a little darker than its greys, as it does the colours beside
    /// them. With their value held at grey's they came out lighter than those, a ring
    /// around a pink highlight on blue.
    #[test]
    fn pale_colours_are_as_dark_as_the_camera_made_them() {
        let base = base();
        // A few pixels a node, as a photo has of such colours.
        let (pale, coloured) = (pale(0.12, 0.3, 2), pale(0.24, 0.3, 2));
        let scene = [pale.clone(), coloured].concat();
        let camera = scaled(&base, &scene, |_| 0.93);
        let mut look = FittedLook::identity();
        for _ in 0..2 {
            refine_table(&base, &mut look, &scene, &camera);
        }
        let left = off(&base, &look, &pale, &camera[..pale.len()]);
        assert!(left.abs() < 0.015, "{left}");
    }

    /// A dark light out of focus: dark, pale colours whose noise reads as one hue here
    /// and another there, asked for very different lightness. The value scale leaves
    /// grey within [`GREY_SLOPE`], or the noise shows as contours.
    #[test]
    fn dark_pale_colours_stay_nearly_as_light_as_grey() {
        let base = base();
        let [nh, ns, _] = TABLE_DIMS;
        let scene = pale(0.07, 0.02, 40);
        // Half the hue circle twice as light as the profile makes it, half at half.
        let target = scaled(&base, &scene, |i| if i % nh < nh / 2 { 2. } else { 0.5 });
        let mut look = FittedLook::identity();
        refine_table(&base, &mut look, &scene, &target);
        let beside_grey = || look.table.iter().skip(1).step_by(ns).map(|e| e[2].ln());
        let most = beside_grey().fold(0f32, |m, v| m.max(v.abs()));
        assert!(most <= GREY_SLOPE + 1e-3, "{most}");
        // The bound is met: the colours did ask.
        assert!(most > 0.9 * GREY_SLOPE, "{most}");
    }

    #[test]
    fn a_grey_cast_is_measured_on_greys_alone() {
        let scene: Vec<Rgb> = (0..200).map(|i| [0.4 + i as f32 / 1000.; 3]).collect();
        let target: Vec<Rgb> = scene.iter().map(|p| [p[0], p[1] * 1.02, p[2]]).collect();
        let g = grey_gains(&scene, &target).expect("a cast");
        assert!(g[1] / g[0] > 1.019 && g[1] / g[0] < 1.021, "{g:?}");
        assert_eq!(grey_gains(&scene, &scene), None, "none to correct");
        assert_eq!(grey_gains(&scene[..10], &target[..10]), None, "too few");
        // Colours say nothing about grey.
        let red: Vec<Rgb> = vec![[0.6, 0.2, 0.1]; 200];
        assert_eq!(grey_gains(&red, &target), None);
    }
    use rawmakase_model::camera_data::Metadata;

    fn base() -> CameraProfile {
        fitted::standard_base(&Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.4, 1.2, 0.2], [-0.05, 0.2, 0.6]],
            ..Default::default()
        })
        .unwrap()
    }
    /// Colours spread over the gamut, as a scene's would be, in a fixed order.
    fn scene() -> Vec<Rgb> {
        let mut seed = 12345u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        (0..6000)
            .map(|_| {
                let v = 0.02 + 0.6 * next().powi(2);
                let s = next() * 0.8;
                let h = next();
                [h, (h + 0.33) % 1., (h + 0.66) % 1.]
                    .map(|x| v * (1. - s * (0.5 + 0.5 * (x * std::f32::consts::TAU).cos())))
            })
            .collect()
    }

    /// Skin in two lights, nearly the same colour and asked for very different ones:
    /// the table bends only as far as its slopes allow, or a gradient shows contours.
    #[test]
    fn a_table_never_steps_between_neighbouring_nodes() {
        let base = base();
        let mut scene = Vec::new();
        let mut target = Vec::new();
        for i in 0..4000 {
            let t = i as f32 / 4000.;
            let warm = i % 2 == 0;
            scene.push([0.3 + 0.1 * t, 0.2 + 0.05 * t, 0.15]);
            target.push(if warm {
                [0.6, 0.3, 0.1]
            } else {
                [0.2, 0.2, 0.5]
            });
        }
        let look = colour_look(&base, &scene, &target);
        let [nh, ns, nv] = TABLE_DIMS;
        let at = |h: usize, s: usize, v: usize| look.table[(v * nh + h) * ns + s];
        for v in 0..nv {
            for s in 1..ns {
                for h in 0..nh {
                    let (a, b) = (at(h, s, v), at((h + 1) % nh, s, v));
                    assert!(
                        (a[0] - b[0]).abs() <= SLOPES[0][0] + 1e-3,
                        "hue {a:?} {b:?}"
                    );
                    assert!((a[1].ln() - b[1].ln()).abs() <= SLOPES[1][0] + 1e-3);
                    assert!((a[2].ln() - b[2].ln()).abs() <= SLOPES[2][0] + 1e-3);
                }
            }
        }
    }

    /// A face among trees: a few hundred samples of skin, with nothing of their colour
    /// around them, are still taken to the camera's skin.
    #[test]
    fn a_small_patch_of_colour_is_learned_beside_a_large_one() {
        let base = base();
        let plain = fitted::with_look(&base, &FittedLook::identity()).unwrap();
        let to_pro =
            |p: Rgb| rawmakase_model::color::mul(rawmakase_model::camera_profiles::RGB_TO_PRO, p);
        // Greens, cyans and blues, rendered as the base renders them.
        let mut scene: Vec<Rgb> = scene()
            .into_iter()
            .filter(|p| (0.25..0.75).contains(&hsv(*p)[0]))
            .collect();
        let mut target: Vec<Rgb> = scene.iter().map(|s| to_pro(plain.finish(*s))).collect();
        let trees = scene.len();
        // Skin, which the camera draws warmer and fuller than the base does.
        let mut camera = FittedLook::identity();
        for entry in camera.table.iter_mut() {
            *entry = [8., 1.25, 1.];
        }
        let camera = fitted::with_look(&base, &camera).unwrap();
        for i in 0..200 {
            let t = i as f32 / 200.;
            let skin = [0.20 + 0.06 * t, 0.155 + 0.04 * t, 0.125 + 0.03 * t];
            scene.push(skin);
            target.push(to_pro(camera.finish(skin)));
        }
        let look = colour_look(&base, &scene, &target);
        let fit = fitted::with_look(&base, &look).unwrap();
        let error = |p: &CameraProfile| {
            let sse: f32 = scene[trees..]
                .iter()
                .zip(&target[trees..])
                .map(|(s, t)| {
                    let got = to_pro(p.finish(*s));
                    (0..3).map(|c| (got[c] - t[c]).powi(2)).sum::<f32>()
                })
                .sum();
            (sse / (3 * 200) as f32).sqrt()
        };
        let (before, after) = (error(&plain), error(&fit));
        assert!(after < 0.25 * before, "{before} → {after}");
    }

    #[test]
    fn a_known_look_is_recovered() {
        let base = base();
        // A look with a curve and a hue-dependent shift: what a camera might do.
        let mut known = FittedLook::identity();
        known.curve = (0..=LUT)
            .map(|i| {
                let x = i as f32 / LUT as f32;
                x + 0.15 * x * (1. - x) * (1. - 2. * x)
            })
            .collect();
        let [nh, ns, nv] = TABLE_DIMS;
        for z in 0..nv {
            for x in 0..nh {
                for y in 1..ns {
                    let hue = x as f32 / nh as f32 * std::f32::consts::TAU;
                    known.table[(z * nh + x) * ns + y] =
                        [8. * hue.sin(), 1. + 0.2 * hue.cos(), 1. + 0.05 * hue.sin()];
                }
            }
        }
        let truth = fitted::with_look(&base, &known).unwrap();
        let scene = scene();
        let target: Vec<Rgb> = scene
            .iter()
            .map(|s| {
                rawmakase_model::color::mul(
                    rawmakase_model::camera_profiles::RGB_TO_PRO,
                    truth.finish(*s),
                )
            })
            .collect();
        let rms = |look: &FittedLook| {
            let p = fitted::with_look(&base, look).unwrap();
            let sse: f32 = scene
                .iter()
                .zip(&target)
                .map(|(s, t)| {
                    let got = rawmakase_model::color::mul(
                        rawmakase_model::camera_profiles::RGB_TO_PRO,
                        p.finish(*s),
                    );
                    (0..3).map(|c| (got[c] - t[c]).powi(2)).sum::<f32>()
                })
                .sum();
            (sse / (3 * scene.len()) as f32).sqrt()
        };
        let before = rms(&FittedLook::identity());
        let fit = colour_look(&base, &scene, &target);
        let after = rms(&fit);
        assert!(after < 0.25 * before && after < 0.01, "{before} → {after}");
        // The same fit again, to the bit.
        assert_eq!(colour_look(&base, &scene, &target), fit);
    }

    #[test]
    fn a_known_matrix_is_recovered_whatever_the_tone() {
        let truth: Matrix = [
            [0.7, 0.2, 0.0642],
            [0.25, 0.8, -0.05],
            [0.02, -0.15, 0.9549],
        ];
        let camera = scene();
        // The preview's colour, through a tone curve that changes each colour's
        // brightness but not its chromaticity.
        let xyz: Vec<Rgb> = camera
            .iter()
            .map(|c| {
                let x = rawmakase_model::color::mul(truth, *c);
                let k = (x[1].max(1e-6)).powf(-0.4);
                x.map(|v| v * k)
            })
            .collect();
        let m = forward_matrix(&camera, &xyz).unwrap();
        for (a, b) in m.iter().flatten().zip(truth.iter().flatten()) {
            assert!((a - b).abs() < 1e-2, "{m:?}");
        }
        let white = rawmakase_model::color::mul(m, [1.; 3]);
        assert!(
            (white[1] - 1.).abs() < 1e-4 && (white[0] - 0.9642).abs() < 1e-4,
            "{white:?}"
        );
        assert!(forward_matrix(&camera[..10], &xyz[..10]).is_err());
    }

    #[test]
    fn the_quantile_curve_is_monotone_with_fixed_ends() {
        let scene = scene();
        let target: Vec<Rgb> = scene.iter().map(|p| p.map(|v| v.sqrt())).collect();
        let curve = quantile_curve(&scene, &target);
        assert_eq!(curve.len(), LUT + 1);
        assert_eq!(curve[0], 0.);
        assert!((curve[LUT] - 1.).abs() < 1e-6);
        assert!(curve.windows(2).all(|w| w[0] <= w[1]));
    }
}
