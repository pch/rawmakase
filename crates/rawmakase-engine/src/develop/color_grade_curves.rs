//! Color grading as Camera Raw 18.7 renders it, measured on the synthetic chart with
//! `scripts/corpus/color-grading.py` (see docs/color-mixer.md#color-grading).
//!
//! Camera Raw grades each channel of linear ProPhoto RGB with its own curve: a
//! channel's output depends on that channel's input only. A region's tint gains are
//! 1 + Σ A_k(x) p_k, with profiles A over the channel value that Blending and Balance
//! shape, and per-channel coefficients p that hue and saturation set; regions
//! multiply. Luminance is one curve for all channels, applied before the tint.
use super::parametric::bracket;
use crate::model::recipe::Recipe;

/// Samples of each curve over E = x^(1/2.2), 0–1.
pub(crate) const SAMPLES: usize = 64;
const BLENDS: [f32; 5] = [0., 0.25, 0.5, 0.75, 1.];
const BALANCES: [f32; 17] = [
    -1., -0.88, -0.75, -0.62, -0.5, -0.38, -0.25, -0.12, 0., 0.12, 0.25, 0.38, 0.5, 0.62, 0.75,
    0.88, 1.,
];
const HUES: usize = 12;
const SATURATIONS: [f32; 5] = [0., 0.25, 0.5, 0.75, 1.];
/// Luminance slider positions, 0 being no change.
const LUMINANCES: [f32; 9] = [-1., -0.75, -0.5, -0.25, 0., 0.25, 0.5, 0.75, 1.];
/// Profile terms per region; Global's third is used by Global only.
const TERMS: usize = 3;
const SHAPED_TERMS: usize = 2;

static DATA: &[u8] = include_bytes!("color_grade_curves.bin");
// Offsets in f32 values, in the order the script writes the tables.
const GRID: usize = 0;
const GRID_LEN: usize = 3 * BLENDS.len() * BALANCES.len() * SAMPLES * SHAPED_TERMS;
const GLOBAL: usize = GRID + GRID_LEN;
const COEF: usize = GLOBAL + SAMPLES * TERMS;
const COEF_LEN: usize = 4 * HUES * (SATURATIONS.len() - 1) * 3 * TERMS;
const LUM: usize = COEF + COEF_LEN;
const LEN: usize = LUM + 4 * (LUMINANCES.len() - 1) * SAMPLES;

fn value(i: usize) -> f32 {
    debug_assert_eq!(DATA.len(), LEN * 4);
    let at = 4 * i;
    f32::from_le_bytes([DATA[at], DATA[at + 1], DATA[at + 2], DATA[at + 3]])
}

/// Shadows, Midtones, Highlights and Global, as `Recipe` orders them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Shadows,
    Midtones,
    Highlights,
    Global,
}
const REGIONS: [Region; 4] = [
    Region::Shadows,
    Region::Midtones,
    Region::Highlights,
    Region::Global,
];

/// Where Blending and Balance put a region's tint: its profiles at every sample.
type Profiles = [[f32; TERMS]; SAMPLES];

fn profiles(region: Region, blending: f32, balance: f32) -> Profiles {
    let mut out = [[0.; TERMS]; SAMPLES];
    if region == Region::Global {
        for (k, p) in out.iter_mut().enumerate() {
            *p = std::array::from_fn(|t| value(GLOBAL + k * TERMS + t));
        }
        return out;
    }
    let (i, u) = bracket(&BLENDS, blending);
    let (j, v) = bracket(&BALANCES, balance);
    let at = |b: usize, a: usize, k: usize, t: usize| {
        let cell = ((region as usize * BLENDS.len() + b) * BALANCES.len() + a) * SAMPLES + k;
        value(GRID + cell * SHAPED_TERMS + t)
    };
    for (k, p) in out.iter_mut().enumerate() {
        for (t, slot) in p.iter_mut().enumerate().take(SHAPED_TERMS) {
            *slot = at(i, j, k, t) * (1. - u) * (1. - v)
                + at(i + 1, j, k, t) * u * (1. - v)
                + at(i, j + 1, k, t) * (1. - u) * v
                + at(i + 1, j + 1, k, t) * u * v;
        }
    }
    out
}

/// The region's per-channel coefficients for `hue` (0–1, a turn) and `saturation` (0–1).
fn coefficients(region: Region, hue: f32, saturation: f32) -> [[f32; TERMS]; 3] {
    let f = hue.rem_euclid(1.) * HUES as f32;
    let (h0, w) = ((f as usize) % HUES, f.fract());
    let (s, u) = bracket(&SATURATIONS, saturation);
    // Saturation 0 has no tint; the table starts at 25.
    let at = |h: usize, s: usize, c: usize, t: usize| match s {
        0 => 0.,
        s => {
            let i = (((region as usize * HUES + h) * (SATURATIONS.len() - 1) + s - 1) * 3 + c)
                * TERMS
                + t;
            value(COEF + i)
        }
    };
    std::array::from_fn(|c| {
        std::array::from_fn(|t| {
            let hue_at = |s| at(h0, s, c, t) * (1. - w) + at((h0 + 1) % HUES, s, c, t) * w;
            hue_at(s) * (1. - u) + hue_at(s + 1) * u
        })
    })
}

/// log2 of the measured Luminance gains at `amount` (−1 to 1), Blending 50 and Balance 0.
fn luminance_log(region: Region, amount: f32) -> [f32; SAMPLES] {
    let (j, u) = bracket(&LUMINANCES, amount);
    let table = |i: usize| -> [f32; SAMPLES] {
        match i.cmp(&4) {
            std::cmp::Ordering::Equal => [0.; SAMPLES],
            order => {
                let row = if order == std::cmp::Ordering::Less {
                    i
                } else {
                    i - 1
                };
                let at = LUM + (region as usize * (LUMINANCES.len() - 1) + row) * SAMPLES;
                std::array::from_fn(|k| value(at + k).log2())
            }
        }
    };
    let (a, b) = (table(j), table(j + 1));
    std::array::from_fn(|k| a[k] * (1. - u) + b[k] * u)
}

/// The region's first profile, scaled to peak at 1: where it sits along the channel.
fn weight(region: Region, blending: f32, balance: f32) -> [f32; SAMPLES] {
    let p = profiles(region, blending, balance);
    let peak = p
        .iter()
        .map(|t| t[0])
        .fold(0f32, |m, v| if v.abs() > m.abs() { v } else { m });
    std::array::from_fn(|k| if peak == 0. { 0. } else { p[k][0] / peak })
}

fn argmax(v: &[f32]) -> usize {
    (0..v.len()).fold(0, |m, i| if v[i] > v[m] { i } else { m })
}

/// Linear interpolation of `table` at fractional sample `at`.
fn sample(table: &[f32; SAMPLES], at: f32) -> f32 {
    let at = at.clamp(0., (SAMPLES - 1) as f32);
    let i = (at as usize).min(SAMPLES - 2);
    let t = at - i as f32;
    table[i] * (1. - t) + table[i + 1] * t
}

fn encoded(k: usize) -> f32 {
    k as f32 / (SAMPLES - 1) as f32
}

/// The Luminance gains at `amount`, Blending and Balance: Luminance keeps the strength
/// it has at the default where the region's weight is the same, on the same side of
/// the weight's peak. Darkening keeps its gain there, lifts their encoded offset.
fn luminance(region: Region, amount: f32, blending: f32, balance: f32) -> [f32; SAMPLES] {
    let log = luminance_log(region, amount);
    let default = (blending - 0.5).abs() < 1e-4 && balance.abs() < 1e-4;
    if region == Region::Global || default {
        return log.map(f32::exp2);
    }
    let (w, w0) = (weight(region, blending, balance), weight(region, 0.5, 0.));
    let (peak, peak0) = (argmax(&w), argmax(&w0));
    let at: [f32; SAMPLES] = std::array::from_fn(|k| {
        let side = if k <= peak {
            0..peak0 + 1
        } else {
            peak0..SAMPLES
        };
        let seg: Vec<usize> = side.collect();
        let near = (0..seg.len())
            .min_by(|&a, &b| {
                (w0[seg[a]] - w[k])
                    .abs()
                    .total_cmp(&(w0[seg[b]] - w[k]).abs())
            })
            .unwrap_or(0);
        for (a, b) in [(near.wrapping_sub(1), near), (near, near + 1)] {
            if a < seg.len() && b < seg.len() {
                let (lo, hi) = (w0[seg[a]], w0[seg[b]]);
                if (lo - w[k]) * (hi - w[k]) <= 0. && lo != hi {
                    let t = (w[k] - lo) / (hi - lo);
                    return seg[a] as f32 + t * (seg[b] as f32 - seg[a] as f32);
                }
            }
        }
        seg[near] as f32
    });
    if amount < 0. {
        return std::array::from_fn(|k| sample(&log, at[k]).exp2());
    }
    let offset: [f32; SAMPLES] = std::array::from_fn(|k| encoded(k) * ((log[k] / 2.2).exp2() - 1.));
    std::array::from_fn(|k| match k {
        0 => log[0].exp2(),
        k => ((encoded(k) + sample(&offset, at[k])) / encoded(k)).powf(2.2),
    })
}

type Gains = [[f32; 3]; SAMPLES];

/// The grading curves for Shadows, Midtones, Highlights and Global settings (hue,
/// saturation, luminance), or `None` when they change nothing.
fn gains(settings: [[f32; 3]; 4], blending: f32, balance: f32) -> Option<Gains> {
    if settings.iter().all(|g| g[1] == 0. && g[2] == 0.) {
        return None;
    }
    let mut lum = [1f32; SAMPLES];
    let mut tint = [[1f32; 3]; SAMPLES];
    for (region, [hue, saturation, amount]) in REGIONS.into_iter().zip(settings) {
        if amount != 0. {
            let l = luminance(region, amount, blending, balance);
            lum.iter_mut().zip(l).for_each(|(a, b)| *a *= b);
        }
        if saturation != 0. {
            let p = profiles(region, blending, balance);
            let c = coefficients(region, hue, saturation);
            for (g, a) in tint.iter_mut().zip(p) {
                for (ch, coef) in g.iter_mut().zip(c) {
                    *ch *= 1. + a.iter().zip(coef).map(|(a, c)| a * c).sum::<f32>();
                }
            }
        }
    }
    // Luminance first, then the tint on its result.
    let tint: [[f32; SAMPLES]; 3] = std::array::from_fn(|c| std::array::from_fn(|i| tint[i][c]));
    Some(std::array::from_fn(|k| {
        let after = (encoded(k).powf(2.2) * lum[k]).clamp(0., 1.).powf(1. / 2.2);
        std::array::from_fn(|c| lum[k] * sample(&tint[c], after * (SAMPLES - 1) as f32).max(0.))
    }))
}

/// `first`, then `then` on its result.
fn compose(first: &Gains, then: &Gains) -> Gains {
    let then: [[f32; SAMPLES]; 3] = std::array::from_fn(|c| std::array::from_fn(|i| then[i][c]));
    std::array::from_fn(|k| {
        std::array::from_fn(|c| {
            let after = (encoded(k).powf(2.2) * first[k][c])
                .clamp(0., 1.)
                .powf(1. / 2.2);
            first[k][c] * sample(&then[c], after * (SAMPLES - 1) as f32)
        })
    })
}

/// Per-channel gains of linear ProPhoto RGB at `SAMPLES` points of x^(1/2.2).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChannelCurves {
    pub(crate) gain: Gains,
}
impl ChannelCurves {
    /// `None` when no region has a tint or a Luminance change, and the profile's look
    /// has no split toning.
    pub(crate) fn new(r: &Recipe) -> Option<Self> {
        let settings = [
            r.grading[0],
            r.grading[1],
            r.grading[2],
            r.effects.global_grade,
        ];
        let user = gains(settings, r.effects.blending, r.effects.balance);
        // A look's split toning is a grading pass of its own, after the user's, at
        // Blending 100 as legacy split toning: on the chart, merging it with the user's
        // grading left 3.8 ΔE00 and a second pass 1.6.
        let look = r
            .profile
            .as_ref()
            .and_then(|p| p.enhanced.as_ref()?.settings.toning)
            .and_then(|t| {
                let [sh, ss] = t.shadows;
                let [hh, hs] = t.highlights;
                let none = [0.; 3];
                gains([[sh, ss, 0.], none, [hh, hs, 0.], none], 1., t.balance)
            });
        let gain = match (user, look) {
            (None, None) => return None,
            (Some(g), None) | (None, Some(g)) => g,
            (Some(first), Some(then)) => compose(&first, &then),
        };
        Some(Self { gain })
    }
    /// `p` is linear ProPhoto RGB.
    pub(crate) fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|c| {
            let v = p[c];
            let at = v.clamp(0., 1.).powf(1. / 2.2) * (SAMPLES - 1) as f32;
            let i = (at as usize).min(SAMPLES - 2);
            let t = at - i as f32;
            v * (self.gain[i][c] * (1. - t) + self.gain[i + 1][c] * t)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn graded(r: &Recipe, p: [f32; 3]) -> [f32; 3] {
        ChannelCurves::new(r).unwrap().apply(p)
    }
    #[test]
    fn table_has_the_expected_size() {
        assert_eq!(DATA.len(), LEN * 4);
    }
    #[test]
    fn channels_are_graded_independently() {
        // Camera Raw's grading is a curve per channel: a channel's output doesn't depend
        // on the others.
        let mut r = Recipe::default();
        r.grading[0] = [30. / 360., 0.5, 0.];
        let a = graded(&r, [0.02, 0.3, 0.6]);
        let b = graded(&r, [0.02, 0.05, 0.9]);
        assert_eq!(a[0], b[0]);
        assert!(a[0] > 0.02 * 1.2, "{a:?}");
    }
    #[test]
    fn balance_moves_shadows_up_the_range() {
        let mut r = Recipe::default();
        r.grading[0] = [210. / 360., 0.5, 0.];
        let blue = |r: &Recipe| graded(r, [0.1; 3])[2] / 0.1;
        let at_default = blue(&r);
        r.effects.balance = -1.;
        assert!(blue(&r) > at_default + 0.1, "{} {at_default}", blue(&r));
        r.effects.balance = 1.;
        assert!(blue(&r) < at_default - 0.1);
    }
    #[test]
    fn blending_widens_the_regions() {
        let mut r = Recipe::default();
        r.grading[2] = [40. / 360., 0.5, 0.];
        let red = |r: &Recipe| graded(r, [0.05; 3])[0] / 0.05;
        r.effects.blending = 0.;
        let narrow = red(&r);
        r.effects.blending = 1.;
        assert!(red(&r) > narrow + 0.02, "{} {narrow}", red(&r));
    }
    #[test]
    fn luminance_darkens_and_lifts_shadows() {
        let mut r = Recipe::default();
        r.grading[0] = [0., 0., -0.5];
        let dark = graded(&r, [0.01; 3]);
        assert!(
            dark.iter().all(|v| (v / 0.01 - 0.5).abs() < 0.1),
            "{dark:?}"
        );
        r.grading[0][2] = 0.5;
        assert!(graded(&r, [0.01; 3])[1] > 0.02);
        // Unchanged near white.
        assert!((graded(&r, [0.95; 3])[1] - 0.95).abs() < 0.01);
    }
    #[test]
    fn luminance_follows_blending_and_balance() {
        let mut r = Recipe::default();
        r.grading[0] = [0., 0., -0.5];
        let at = |r: &Recipe| graded(r, [0.04; 3])[1] / 0.04;
        let default = at(&r);
        r.effects.balance = 1.;
        assert!(at(&r) > default + 0.1);
        r.effects.balance = 0.;
        r.effects.blending = 1.;
        assert!(at(&r) < default);
    }
}
