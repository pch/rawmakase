//! The color mixer (HSL), Saturation and Vibrance, as measured Camera Raw responses.
//!
//! Camera Raw's color mixer behaves like a hue/saturation/value lookup in linear
//! ProPhoto RGB. For each slider at its extremes (±100; ±50 for Saturation and
//! Vibrance), `color_mixer_chart.bin` holds the change it makes to the default
//! rendering: hue shift (turns), log2 saturation and log2 value factors, on a grid of
//! 36 hues × 6 saturations (spaced by √s) × 6 values (spaced by v^0.45). The eight
//! bands' tables are fitted on a dense synthetic chart; Saturation's and Vibrance's
//! were measured on nine photos (Fujifilm X100F, Sony A7 II and A7CR). Slider
//! positions scale the hue shift, the value factor's log and positive saturation's log
//! linearly; negative saturation scales the factor itself linearly, which matches
//! Camera Raw at −25 and −50. Several sliders add their changes. See
//! docs/color-mixer.md.
use crate::color::mul;
use crate::model::recipe::Recipe;

/// How much of a slider's measured change at ±100 applies at `s`. Camera Raw's band
/// Luminance is not linear: at −50 it applies about a third of the −100 change, at
/// +50 about 58% of the +100 one (both measured on Blue and Purple).
fn strength(kind: usize, s: f32) -> f32 {
    let s_abs = s.abs().min(1.);
    if kind == LUMINANCE {
        s_abs.powf(if s < 0. {
            LUMINANCE_DARKEN
        } else {
            LUMINANCE_LIGHTEN
        })
    } else {
        s_abs
    }
}

const HUES: usize = 36;
const SATS: usize = 6;
const VALS: usize = 6;
pub(crate) const CELLS: usize = HUES * SATS * VALS;
const TABLE: usize = 3 * CELLS;
/// 8 bands × (hue, saturation, luminance) × (−, +), then Saturation −/+, Vibrance −/+.
const TABLES: usize = 52;
const SCALE: f32 = 1. / 8000.;

/// Vibrance's slider positions, each with a grid in
/// `vibrance_chart.bin`: hue shift, log2 saturation and log2 value factors. The
/// ±25 and ±50 grids are the photo tables there; the ±75 and ±100 ones the chart's.
const VIBRANCE_KNOTS: [f32; 8] = [-1., -0.75, -0.5, -0.25, 0.25, 0.5, 0.75, 1.];
static VIBRANCE_CHART: &[u8] = include_bytes!("vibrance_chart.bin");

fn vibrance_value(knot: usize, channel: usize, cell: usize) -> f32 {
    let i = 2 * (knot * TABLE + channel * CELLS + cell);
    i16::from_le_bytes([VIBRANCE_CHART[i], VIBRANCE_CHART[i + 1]]) as f32 * SCALE
}

/// The chart grids around Vibrance `v` and their weights: linear between the
/// measured positions, and toward no change at 0.
fn vibrance_knots(v: f32) -> Vec<(usize, f32)> {
    let v = v.clamp(-1., 1.);
    let i = VIBRANCE_KNOTS.partition_point(|k| *k < v);
    let below = i.checked_sub(1).map(|k| (k, VIBRANCE_KNOTS[k]));
    let above = (i < VIBRANCE_KNOTS.len()).then(|| (i, VIBRANCE_KNOTS[i]));
    // Between −25 and +25 the other end is no change at all.
    let (lo, hi) = match (below, above) {
        (Some(b), Some(a)) if b.1 < 0. && a.1 > 0. => {
            return if v < 0. {
                vec![(b.0, v / b.1)]
            } else {
                vec![(a.0, v / a.1)]
            };
        }
        (Some(b), Some(a)) => (b, a),
        (Some(b), None) => return vec![(b.0, 1.)],
        (None, Some(a)) => return vec![(a.0, 1.)],
        (None, None) => return Vec::new(),
    };
    let t = (v - lo.1) / (hi.1 - lo.1);
    [(lo.0, 1. - t), (hi.0, t)]
        .into_iter()
        .filter(|(_, w)| *w != 0.)
        .collect()
}

/// Adds Vibrance `v`'s chart grid to `grid`.
fn add_vibrance(grid: &mut [[f32; 3]], v: f32) {
    for (knot, w) in vibrance_knots(v) {
        for (cell, d) in grid.iter_mut().enumerate() {
            for (c, d) in d.iter_mut().enumerate() {
                *d += w * vibrance_value(knot, c, cell);
            }
        }
    }
}

/// Where Saturation stops scaling the measured −50 table and starts fading to gray
/// (docs/color-mixer.md#saturation).
const GRAY_FROM: f32 = -0.5;

/// The slider kind of a band's Luminance tables.
const LUMINANCE: usize = 2;
/// Exponents of the slider position for band Luminance (`strength`).
const LUMINANCE_DARKEN: f32 = 1.6;
const LUMINANCE_LIGHTEN: f32 = 0.79;
static CHART: &[u8] = include_bytes!("color_mixer_chart.bin");

fn value(table: usize, channel: usize, cell: usize) -> f32 {
    let i = 2 * (table * TABLE + channel * CELLS + cell);
    i16::from_le_bytes([CHART[i], CHART[i + 1]]) as f32 * SCALE
}

/// The combined change of all active sliders, one grid.
pub(crate) struct ColorMixer {
    pub(crate) delta: Vec<[f32; 3]>,
    /// How far colors fade to their luminance after the tables (Saturation below
    /// −50), from 0 (not at all) to 1 (gray).
    pub(crate) saturation_gray: f32,
    /// The other sliders' grid, whose result gives the gray its luminance while
    /// colors fade; `None` when they are all at 0 and the color's own is used.
    pub(crate) gray_source: Option<Vec<[f32; 3]>>,
}
impl ColorMixer {
    pub(crate) fn new(r: &Recipe) -> Option<Self> {
        debug_assert_eq!(CHART.len(), TABLES * TABLE * 2);
        let mut active: Vec<(usize, f32)> = Vec::new();
        for (band, controls) in r.hsl.iter().enumerate() {
            for (kind, s) in controls.iter().enumerate() {
                if *s != 0. {
                    let sign = usize::from(*s > 0.);
                    active.push(((band * 3 + kind) * 2 + sign, strength(kind, *s)));
                }
            }
        }
        let saturation = r.saturation.max(GRAY_FROM);
        // Up to ±50 Vibrance is the photo tables, scaled; beyond, the chart's grids.
        let chart_vibrance = r.vibrance.abs() > 0.5;
        let vibrance = if chart_vibrance { 0. } else { r.vibrance };
        // Measured at ±50: positions beyond extrapolate linearly.
        for (i, s) in [saturation, vibrance].into_iter().enumerate() {
            if s != 0. {
                active.push((48 + i * 2 + usize::from(s > 0.), (s.abs() * 2.).min(2.)));
            }
        }
        let saturation_gray = ((GRAY_FROM - r.saturation) / (1. + GRAY_FROM)).clamp(0., 1.);
        if active.is_empty() && !chart_vibrance {
            return None;
        }
        // While colors fade to gray, the other sliders still set its brightness.
        let others: Vec<_> = active
            .iter()
            .copied()
            .filter(|(t, _)| *t != 48 && *t != 49)
            .collect();
        let with_vibrance = |mut g: Vec<[f32; 3]>| {
            if chart_vibrance {
                add_vibrance(&mut g, r.vibrance);
            }
            g
        };
        let gray_source = (saturation_gray > 0. && (!others.is_empty() || chart_vibrance))
            .then(|| with_vibrance(grid(&others)));
        Some(Self {
            delta: with_vibrance(grid(&active)),
            saturation_gray,
            gray_source,
        })
    }

    /// `rgb` is linear display RGB (sRGB primaries).
    pub(crate) fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mixed = tables(&self.delta, rgb);
        if self.saturation_gray == 0. {
            return mixed;
        }
        // Camera Raw's −100 keeps the luminance the color has without Saturation.
        let source = self.gray_source.as_ref().map_or(rgb, |g| tables(g, rgb));
        let y = crate::color::luminance(source);
        mixed.map(|v| v + self.saturation_gray * (y - v))
    }
}

/// `rgb` (linear display RGB) through a grid of changes.
pub(crate) fn tables(delta: &[[f32; 3]], rgb: [f32; 3]) -> [f32; 3] {
    let p = mul(crate::camera_profiles::RGB_TO_PRO, rgb).map(|v| v.max(0.));
    let Some([h, s, max]) = hsv(p) else {
        return rgb;
    };
    let [dh, ds, dv] = lookup_with(h, s, max, |cell| delta[cell]);
    let h = (h + dh).rem_euclid(1.) * 6.;
    let s = (s * ds.exp2()).clamp(0., 1.);
    let v = max * dv.exp2();
    let c = v * s;
    let x = c * (1. - (h % 2. - 1.).abs());
    let q = match h as usize {
        0 => [c, x, 0.],
        1 => [x, c, 0.],
        2 => [0., c, x],
        3 => [0., x, c],
        4 => [x, 0., c],
        _ => [c, 0., x],
    };
    mul(
        crate::camera_profiles::PRO_TO_RGB,
        q.map(|v| v + (max * dv.exp2()) - c),
    )
}

/// The summed change of the `active` tables (index, strength), one grid.
fn grid(active: &[(usize, f32)]) -> Vec<[f32; 3]> {
    (0..CELLS)
        .map(|cell| {
            std::array::from_fn(|c| {
                active
                    .iter()
                    .map(|(t, w)| {
                        let v = value(*t, c, cell);
                        // Negative saturation sliders scale the saturation factor
                        // linearly, as Camera Raw does: scaling the log factor of a
                        // strong measured desaturation overshoots at −25 and −50.
                        // Even tables are the negative extremes.
                        if c == 1 && t % 2 == 0 {
                            (1. + w * (v.exp2() - 1.)).max(1e-3).log2()
                        } else {
                            v * w
                        }
                    })
                    .sum::<f32>()
            })
        })
        .collect()
}

/// How strongly each band's `channel` slider (0 hue, 1 saturation, 2 luminance)
/// changes the color `prophoto` (linear ProPhoto RGB, as the mixer sees it): the
/// sizes of its measured changes at ±100 there, which are what the slider scales.
pub(crate) fn band_responses(prophoto: [f32; 3], channel: usize) -> [f32; 8] {
    let Some([h, s, v]) = hsv(prophoto.map(|c| c.max(0.))) else {
        return [0.; 8];
    };
    std::array::from_fn(|band| {
        let [minus, plus] = [0, 1].map(|sign| (band * 3 + channel) * 2 + sign);
        lookup_with(h, s, v, |cell| {
            let size = value(minus, channel, cell).abs() + value(plus, channel, cell).abs();
            [size; 3]
        })[0]
    })
}

/// Hue (turns), saturation and value of linear ProPhoto RGB, or `None` for black.
fn hsv(p: [f32; 3]) -> Option<[f32; 3]> {
    let max = p.into_iter().fold(0f32, f32::max);
    let min = p.into_iter().fold(f32::INFINITY, f32::min);
    if max <= 1e-6 {
        return None;
    }
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
    Some([h, d / max, max])
}

/// Trilinear lookup of the grid `delta` gives for each cell, between grid centres;
/// hue wraps.
fn lookup_with(h: f32, s: f32, v: f32, delta: impl Fn(usize) -> [f32; 3]) -> [f32; 3] {
    let fh = h.rem_euclid(1.) * HUES as f32 - 0.5;
    let fs = (s.clamp(0., 1.).sqrt() * SATS as f32 - 0.5).clamp(0., (SATS - 1) as f32);
    let fv = (v.clamp(0., 1.).powf(0.45) * VALS as f32 - 0.5).clamp(0., (VALS - 1) as f32);
    let h0 = fh.floor();
    let (th, ts, tv) = (fh - h0, fs.fract(), fv.fract());
    let hi = |d: isize| (h0 as isize + d).rem_euclid(HUES as isize) as usize;
    let (s0, v0) = (fs as usize, fv as usize);
    let (s1, v1) = ((s0 + 1).min(SATS - 1), (v0 + 1).min(VALS - 1));
    let mut out = [0.; 3];
    for (dh, wh) in [(0, 1. - th), (1, th)] {
        for (si, ws) in [(s0, 1. - ts), (s1, ts)] {
            for (vi, wv) in [(v0, 1. - tv), (v1, tv)] {
                let d = delta((hi(dh) * SATS + si) * VALS + vi);
                let w = wh * ws * wv;
                for c in 0..3 {
                    out[c] += d[c] * w;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_blob_has_expected_size_and_neutrals_stay_neutral() {
        assert_eq!(CHART.len(), TABLES * TABLE * 2);
        assert!(ColorMixer::new(&Recipe::default()).is_none());
        let mut r = Recipe::default();
        r.hsl[1] = [0.5, 1., -1.];
        r.saturation = 0.3;
        let m = ColorMixer::new(&r).unwrap();
        let gray = m.apply([0.2; 3]);
        assert!((gray[0] - gray[1]).abs() < 1e-4 && (gray[1] - gray[2]).abs() < 1e-4);
        assert!(m.apply([0.3, 0.2, 0.1]).iter().all(|v| v.is_finite()));
        // No band's Luminance moves a neutral, as in Camera Raw.
        for band in 0..8 {
            for amount in [-1., 1.] {
                let mut r = Recipe::default();
                r.hsl[band][2] = amount;
                let gray = ColorMixer::new(&r).unwrap().apply([0.2; 3]);
                assert!(
                    gray.iter().all(|v| (v - 0.2).abs() < 2e-3),
                    "{band} {gray:?}"
                );
            }
        }
    }
    #[test]
    fn orange_luminance_darkens_skin_tones() {
        let mut r = Recipe::default();
        let skin = [0.5, 0.3, 0.2];
        let lum = |p: [f32; 3]| crate::color::luminance(p);
        r.hsl[1][2] = -1.;
        let darker = ColorMixer::new(&r).unwrap().apply(skin);
        r.hsl[1][2] = 1.;
        let brighter = ColorMixer::new(&r).unwrap().apply(skin);
        assert!(lum(darker) < lum(skin) && lum(brighter) > lum(skin));
        // Saturation −100 of every band approaches gray.
        let r = Recipe {
            saturation: -1.,
            ..Default::default()
        };
        let p = ColorMixer::new(&r).unwrap().apply(skin);
        let spread = |p: [f32; 3]| {
            p.iter().fold(0f32, |a, b| a.max(*b)) - p.iter().fold(1f32, |a, b| a.min(*b))
        };
        assert!(spread(p) < spread(skin) * 0.3);
    }
    #[test]
    fn gray_saturation_reaches_the_colors_luminance_at_minus_100() {
        let recipe = |saturation| Recipe {
            saturation,
            ..Default::default()
        };
        let orange = [0.6, 0.3, 0.1];
        let y = crate::color::luminance(orange);
        let at = |s| ColorMixer::new(&recipe(s)).unwrap().apply(orange);
        // −100 is exactly gray of the color's luminance, as in Camera Raw.
        let gray = at(-1.);
        assert!(gray.iter().all(|v| (v - y).abs() < 1e-5), "{gray:?}");
        // Down to −50 there is no fade to gray.
        assert_eq!(ColorMixer::new(&recipe(-0.5)).unwrap().saturation_gray, 0.);
        // Halfway between −50 and −100, halfway to gray.
        let half = at(-0.75);
        let from = at(-0.5);
        assert!((half[0] - (from[0] + y) / 2.).abs() < 1e-5);
        // Band Luminance still sets the gray's brightness, as without Saturation.
        let mut darker = recipe(-1.);
        darker.hsl[1][2] = -1.;
        let mixer = ColorMixer::new(&darker).unwrap();
        let toned = mixer.apply(orange);
        let mut no_saturation = darker;
        no_saturation.saturation = 0.;
        let alone = ColorMixer::new(&no_saturation).unwrap().apply(orange);
        let y_alone = crate::color::luminance(alone);
        assert!(
            toned.iter().all(|v| (v - y_alone).abs() < 1e-5),
            "{toned:?}"
        );
        assert!(y_alone < y * 0.95);
    }
    #[test]
    fn chart_vibrance_interpolates_its_measured_positions() {
        assert!(vibrance_knots(0.).iter().all(|(_, w)| *w == 0.));
        assert_eq!(vibrance_knots(-1.), vec![(0, 1.)]);
        assert_eq!(vibrance_knots(0.5), vec![(5, 1.)]);
        let between = vibrance_knots(0.6);
        assert_eq!(between.len(), 2);
        assert!((between[0].1 - 0.6).abs() < 1e-6 && (between[1].1 - 0.4).abs() < 1e-6);
        assert_eq!(vibrance_knots(-0.1), vec![(3, 0.4)]);
        let at = |vibrance| {
            ColorMixer::new(&Recipe {
                vibrance,
                ..Default::default()
            })
            .unwrap()
        };
        let skin = [0.6, 0.35, 0.25];
        let sat = |p: [f32; 3]| {
            let max = p.iter().fold(0f32, |a, b| a.max(*b));
            (max - p.iter().fold(1f32, |a, b| a.min(*b))) / max
        };
        // Vibrance −100 removes most of a color's saturation, as Camera Raw does.
        let chart = sat(at(-1.).apply(skin));
        assert!(chart < sat(skin) * 0.4, "{chart}");
        assert!(
            at(0.7)
                .apply([0.2; 3])
                .iter()
                .all(|v| (v - 0.2).abs() < 2e-3)
        );
        // Up to ±50 it is the photo tables, scaled linearly.
        let (full, half) = (at(0.5).delta, at(0.25).delta);
        assert!(
            full.iter()
                .zip(&half)
                .all(|(f, h)| (0..3).all(|c| (h[c] - f[c] * 0.5).abs() < 1e-6))
        );
    }
    #[test]
    fn chart_luminance_follows_camera_raws_slider_curve() {
        let at = |s: f32| {
            let mut r = Recipe::default();
            r.hsl[5][2] = s;
            ColorMixer::new(&r).unwrap().delta
        };
        for (full, half, expected) in [(-1., -0.5, 0.33), (1., 0.5, 0.58)] {
            let (f, h) = (at(full), at(half));
            for (f, h) in f.iter().zip(&h) {
                assert!((h[2] - f[2] * expected).abs() < 0.01 * f[2].abs() + 1e-6);
            }
        }
    }
    #[test]
    fn negative_saturation_scales_the_factor_linearly() {
        let at = |s: f32| {
            let mut r = Recipe::default();
            r.hsl[3][1] = s;
            ColorMixer::new(&r).unwrap().delta
        };
        let (full, half) = (at(-1.), at(-0.5));
        for (f, h) in full.iter().zip(&half) {
            let expected = (1. + 0.5 * (f[1].exp2() - 1.)).max(1e-3).log2();
            assert!((h[1] - expected).abs() < 1e-5);
            // Hue and value keep scaling their measured change linearly.
            assert!((h[0] - f[0] * 0.5).abs() < 1e-6 && (h[2] - f[2] * 0.5).abs() < 1e-6);
        }
        // Positive saturation keeps scaling the log factor.
        let (full, half) = (at(1.), at(0.5));
        assert!(
            full.iter()
                .zip(&half)
                .all(|(f, h)| (h[1] - f[1] * 0.5).abs() < 1e-6)
        );
    }
}
