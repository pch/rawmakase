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

/// Rendered files' (JPEG, PNG, TIFF) measured Saturation and Vibrance tables in
/// `rendered_sat_vib.bin`: 7 Saturation grids at [`RENDERED_SAT_KNOTS`] followed by
/// 8 Vibrance grids at [`VIBRANCE_KNOTS`], fitted to Camera Raw 18.7 on dense
/// ProPhoto-HSV sRGB and ProPhoto JPEG charts (docs/color-mixer.md#rendered-files).
const RENDERED_SAT_KNOTS: [f32; 7] = [-0.75, -0.5, -0.25, 0.25, 0.5, 0.75, 1.];
const RENDERED_TABLES: usize = RENDERED_SAT_KNOTS.len() + VIBRANCE_KNOTS.len();
/// Where a rendered file's Saturation stops interpolating its measured grids and
/// fades linearly to Rec. 709 luminance, reaching exact gray at −100.
const RENDERED_GRAY_FROM: f32 = -0.75;
static RENDERED_SAT_VIB: &[u8] = include_bytes!("rendered_sat_vib.bin");

fn rendered_value(grid_index: usize, channel: usize, cell: usize) -> f32 {
    let i = 2 * (grid_index * TABLE + channel * CELLS + cell);
    i16::from_le_bytes([RENDERED_SAT_VIB[i], RENDERED_SAT_VIB[i + 1]]) as f32 * SCALE
}

/// Adds a rendered file's measured Saturation or Vibrance grid at `v` to `grid`.
///
/// When `factor_sat` is true (Saturation of either sign and negative Vibrance), the
/// saturation channel interpolates the linear factor `2^ds` between knots (and from
/// `1` between 0 and ±25), matching Camera Raw's intermediate slider positions.
/// Positive Vibrance interpolates `log2(s)` directly.
fn add_rendered_slider(
    grid: &mut [[f32; 3]],
    knots: &[f32],
    offset: usize,
    v: f32,
    factor_sat: bool,
) {
    if v == 0. {
        return;
    }
    let v = v.clamp(knots[0], knots[knots.len() - 1]);
    let i = knots.partition_point(|k| *k < v);
    let below = i.checked_sub(1).map(|k| (k, knots[k]));
    let above = (i < knots.len()).then(|| (i, knots[i]));
    match (below, above) {
        // Between −25 and +25 the other end is the identity (zero shift, factor 1).
        (Some(b), Some(a)) if b.1 < 0. && a.1 > 0. => {
            let (k, w) = if v < 0. {
                (b.0, v / b.1)
            } else {
                (a.0, v / a.1)
            };
            let t = offset + k;
            for (cell, d) in grid.iter_mut().enumerate() {
                d[0] += w * rendered_value(t, 0, cell);
                let s = rendered_value(t, 1, cell);
                d[1] += if factor_sat {
                    (1. + w * (s.exp2() - 1.)).max(1e-3).log2()
                } else {
                    w * s
                };
                d[2] += w * rendered_value(t, 2, cell);
            }
        }
        (Some(lo), Some(hi)) => {
            let t = (v - lo.1) / (hi.1 - lo.1);
            let (t0, t1) = (offset + lo.0, offset + hi.0);
            for (cell, d) in grid.iter_mut().enumerate() {
                d[0] += (1. - t) * rendered_value(t0, 0, cell) + t * rendered_value(t1, 0, cell);
                let s0 = rendered_value(t0, 1, cell);
                let s1 = rendered_value(t1, 1, cell);
                d[1] += if factor_sat {
                    ((1. - t) * s0.exp2() + t * s1.exp2()).max(1e-3).log2()
                } else {
                    (1. - t) * s0 + t * s1
                };
                d[2] += (1. - t) * rendered_value(t0, 2, cell) + t * rendered_value(t1, 2, cell);
            }
        }
        (Some((k, _)), None) | (None, Some((k, _))) => {
            let t = offset + k;
            for (cell, d) in grid.iter_mut().enumerate() {
                for (c, out) in d.iter_mut().enumerate() {
                    *out += rendered_value(t, c, cell);
                }
            }
        }
        (None, None) => {}
    }
}

/// Camera Raw 18.7's RAW SDR Saturation operator in linear ProPhoto RGB, applied
/// after the scene tone stage and before `ProfileLookTable` and `ProfileToneCurve`.
///
/// Scales each channel's chroma offset `(pro[c] - Y)` by `1 + saturation`, where `Y`
/// is ProPhoto luminance ([`PRO_LUMA`]), clamps each channel's signed chroma offset
/// to `±scale * (1 - w_c)` where `scale = max(1, max(p))`, and floors channels at 0.
/// For `p ∈ [0, 1]^3` (`scale = 1`), the clamp matches Camera Raw 18.7 on a 384-patch
/// linear ProPhoto DNG to 0.000014–0.000018 MAE across −50, +50 and +100 (and on
/// over-range scene values compressed by Camera Raw's white shoulder); scaling the
/// clamp by `max(1, max(p))` keeps the operator continuous through Saturation 0 when
/// RAWmakase's unclamped scene tone stage delivers channels above 1.
pub(crate) fn apply_raw_saturation(pro: [f32; 3], saturation: f32) -> [f32; 3] {
    if saturation == 0. {
        return pro;
    }
    let p = pro.map(|v| v.max(0.));
    let y = p[0] * PRO_LUMA[0] + p[1] * PRO_LUMA[1] + p[2] * PRO_LUMA[2];
    let scale = p[0].max(p[1]).max(p[2]).max(1.);
    let k = 1. + saturation.clamp(-1., 1.);
    std::array::from_fn(|c| {
        let lim = scale * (1. - PRO_LUMA[c]);
        (y + (k * (p[c] - y)).clamp(-lim, lim)).max(0.)
    })
}

/// Grid dimensions and axes of `raw_vibrance.bin`: 8 knots at [`VIBRANCE_KNOTS`],
/// 2 channels (linear saturation factor `s'/s` and linear value factor `v'/v`) on a
/// 36 × 12 × 11 linear ProPhoto HSV grid, directly measured on a 16-bit lossless
/// linear-profile DNG chart rendered in Camera Raw 18.7 (where Vibrance runs after
/// `ProfileLookTable` and before `ProfileToneCurve`, preserving linear ProPhoto hue
/// to 0.03°). The 36 hues sit at exact 10° multiples (`0°, 10°, …, 350°`) so every
/// 30° RGB hexcone corner lands on a knot.
pub(crate) const RAW_VIBRANCE_HUES: usize = 36;
pub(crate) const RAW_VIBRANCE_SATS: usize = 12;
pub(crate) const RAW_VIBRANCE_VALS: usize = 11;
pub(crate) const RAW_VIBRANCE_CELLS: usize =
    RAW_VIBRANCE_HUES * RAW_VIBRANCE_SATS * RAW_VIBRANCE_VALS;
const RAW_VIBRANCE_SAT_AXIS: [f32; RAW_VIBRANCE_SATS] = [
    0.0, 0.02, 0.06, 0.12, 0.20, 0.32, 0.45, 0.58, 0.70, 0.82, 0.91, 1.0,
];
const RAW_VIBRANCE_VAL_AXIS: [f32; RAW_VIBRANCE_VALS] = [
    0.0, 0.004, 0.015, 0.04, 0.08, 0.16, 0.30, 0.50, 0.75, 0.95, 1.0,
];
const RAW_VIBRANCE_TABLE: usize = 2 * RAW_VIBRANCE_CELLS;
const RAW_VIBRANCE_SCALE: f32 = 1. / 8000.;
static RAW_VIBRANCE: &[u8] = include_bytes!("raw_vibrance.bin");

fn axis_bracket(value: f32, axis: &[f32]) -> (usize, usize, f32) {
    let last = axis.len() - 1;
    let v = value.clamp(axis[0], axis[last]);
    let hi = axis.partition_point(|&k| k < v).clamp(1, last);
    let lo = hi - 1;
    let span = (axis[hi] - axis[lo]).max(1e-12);
    (lo, hi, ((v - axis[lo]) / span).clamp(0., 1.))
}

fn raw_vibrance_value(knot: usize, channel: usize, cell: usize) -> f32 {
    let i = 2 * (knot * RAW_VIBRANCE_TABLE + channel * RAW_VIBRANCE_CELLS + cell);
    u16::from_le_bytes([RAW_VIBRANCE[i], RAW_VIBRANCE[i + 1]]) as f32 * RAW_VIBRANCE_SCALE
}

/// A RAW SDR render's pre-tone-curve Vibrance lookup table in linear ProPhoto HSV.
pub(crate) struct RawVibrance {
    /// `[saturation_factor, value_factor]` per cell of the 36 × 12 × 11 grid.
    pub(crate) factors: Vec<[f32; 2]>,
}

impl RawVibrance {
    /// Builds the interpolated pre-tone-curve Vibrance grid for `vibrance` (`-1..=1`),
    /// or `None` when `vibrance == 0`. Negative Vibrance interpolates both linear
    /// factors directly between knots (and from 1 between 0 and −25); positive Vibrance
    /// interpolates in log space (`f0^(1-t) * f1^t`).
    pub(crate) fn new(vibrance: f32) -> Option<Self> {
        if vibrance == 0. {
            return None;
        }
        debug_assert_eq!(
            RAW_VIBRANCE.len(),
            VIBRANCE_KNOTS.len() * RAW_VIBRANCE_TABLE * 2
        );
        let v = vibrance.clamp(-1., 1.);
        let i = VIBRANCE_KNOTS.partition_point(|k| *k < v);
        let below = i.checked_sub(1).map(|k| (k, VIBRANCE_KNOTS[k]));
        let above = (i < VIBRANCE_KNOTS.len()).then(|| (i, VIBRANCE_KNOTS[i]));
        let mut factors = vec![[1.; 2]; RAW_VIBRANCE_CELLS];
        let positive = v > 0.;
        match (below, above) {
            (Some(b), Some(a)) if b.1 < 0. && a.1 > 0. => {
                let (k, w) = if v < 0. {
                    (b.0, v / b.1)
                } else {
                    (a.0, v / a.1)
                };
                for (cell, out) in factors.iter_mut().enumerate() {
                    for (c, o) in out.iter_mut().enumerate() {
                        let f1 = raw_vibrance_value(k, c, cell);
                        *o = if positive {
                            f1.max(1e-4).powf(w)
                        } else {
                            1. + w * (f1 - 1.)
                        };
                    }
                }
            }
            (Some(lo), Some(hi)) => {
                let t = (v - lo.1) / (hi.1 - lo.1);
                for (cell, out) in factors.iter_mut().enumerate() {
                    for (c, o) in out.iter_mut().enumerate() {
                        let f0 = raw_vibrance_value(lo.0, c, cell);
                        let f1 = raw_vibrance_value(hi.0, c, cell);
                        *o = if positive {
                            f0.max(1e-4).powf(1. - t) * f1.max(1e-4).powf(t)
                        } else {
                            (1. - t) * f0 + t * f1
                        };
                    }
                }
            }
            (Some((k, _)), None) | (None, Some((k, _))) => {
                for (cell, out) in factors.iter_mut().enumerate() {
                    out[0] = raw_vibrance_value(k, 0, cell);
                    out[1] = raw_vibrance_value(k, 1, cell);
                }
            }
            (None, None) => return None,
        }
        Some(Self { factors })
    }

    /// Applies pre-tone-curve Vibrance to linear ProPhoto RGB `pro`.
    pub(crate) fn apply(&self, pro: [f32; 3]) -> [f32; 3] {
        let p = pro.map(|v| v.max(0.));
        let Some([h, s, max]) = hsv(p) else {
            return pro;
        };
        let fh = h.rem_euclid(1.) * RAW_VIBRANCE_HUES as f32;
        let h0 = fh.floor() as usize % RAW_VIBRANCE_HUES;
        let h1 = (h0 + 1) % RAW_VIBRANCE_HUES;
        let wh = fh - fh.floor();
        let (s0, s1, ws) = axis_bracket(s, &RAW_VIBRANCE_SAT_AXIS);
        let (v0, v1, wv) = axis_bracket(max, &RAW_VIBRANCE_VAL_AXIS);
        let mut out = [0f32; 2];
        for (hi, a) in [(h0, 1. - wh), (h1, wh)] {
            for (si, b) in [(s0, 1. - ws), (s1, ws)] {
                for (vi, c) in [(v0, 1. - wv), (v1, wv)] {
                    let w = a * b * c;
                    let f = self.factors[(hi * RAW_VIBRANCE_SATS + si) * RAW_VIBRANCE_VALS + vi];
                    out[0] += w * f[0];
                    out[1] += w * f[1];
                }
            }
        }
        let h6 = h.rem_euclid(1.) * 6.;
        let s_out = (s * out[0]).clamp(0., 1.);
        let v_out = (max * out[1]).max(0.);
        let c = v_out * s_out;
        let x = c * (1. - (h6 % 2. - 1.).abs());
        let q = match h6 as usize {
            0 => [c, x, 0.],
            1 => [x, c, 0.],
            2 => [0., c, x],
            3 => [0., x, c],
            4 => [x, 0., c],
            _ => [c, 0., x],
        };
        q.map(|ch| ch + v_out - c)
    }
}

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
    /// An HDR master's: the parts of `delta`'s and `gray_source`'s log2 value factors
    /// that Vibrance and band Luminance make ([`Lookup::Hdr`]). `None` for every other
    /// render.
    hdr: Option<[HdrFactors; 2]>,
}

/// The parts of a grid's log2 value factors (one per cell) that an HDR master applies
/// apart from its other changes ([`Lookup::Hdr`]).
pub(crate) struct HdrFactors {
    /// Vibrance's, which Camera Raw applies below white only.
    pub(crate) vibrance: Vec<f32>,
    /// Band Luminance's whole change (hue, saturation and log2 value), which an HDR
    /// master looks up apart from the others and whose value factor it applies to each
    /// channel by its own level; `None` when no band's Luminance is set.
    pub(crate) luminance: Option<Vec<[f32; 3]>>,
}

/// How a render looks a color up in a grid.
#[derive(Clone, Copy)]
pub(crate) enum Lookup<'a> {
    /// At the color's own value, clamped to 1: the grids' measured domain.
    Sdr,
    /// An HDR master's, as Camera Raw 18.7 renders HDR (docs/hdr.md#the-color-stage):
    /// hue, saturation and value changes looked up at the color's value up to
    /// [`HDR_VALUE`], and the same change above it, so a color keeps them at any
    /// brightness. The value factors Vibrance makes (the slice, one log2 factor per
    /// cell) are looked up at the color's own value and fade out from [`HDR_FADE`] to
    /// graphics white. Band Luminance's value factors are looked up at the color's
    /// value up to [`HDR_LUMINANCE_VALUE`], then applied to each ProPhoto channel by
    /// its scene level (`scene`: linear ProPhoto before the profile's curve, changed
    /// as the mixer's other changes change the channel; the channel's own level when
    /// `None`): in full up to [`HDR_LUMINANCE_FADE`], fading out linearly to none at
    /// graphics white.
    Hdr(Option<&'a HdrFactors>, Option<[f32; 3]>),
}

/// The value up to which an HDR master looks up the grids at the color's own value.
/// Camera Raw's SDR renders, on which the grids were fitted, roll off from about 0.5;
/// its HDR renders keep the low-value response at every brightness (Saturation +100
/// scales saturation by 1.73 to 1.74 from 0.125 to 8 times white).
pub(crate) const HDR_VALUE: f32 = 0.2;
/// The value from which an HDR master's Vibrance value factors fade, linearly, to
/// none at graphics white. Camera Raw's HDR renders apply them in full at 0.5 and not
/// from white up (Vibrance +100: value ×1.06 at 0.5, ×1.01 at 1 to 8 on colors of
/// saturation 0.3).
pub(crate) const HDR_FADE: f32 = 0.5;
/// The value up to which an HDR master looks band Luminance's changes up at the
/// color's own value: below it the SDR grids match Camera Raw's HDR renders as they
/// match its SDR ones, and they carry the SDR roll-off from about there. Its hue and
/// saturation changes move from that lookup to the other changes' ([`HDR_VALUE`])
/// between this value and graphics white.
pub(crate) const HDR_LUMINANCE_VALUE: f32 = 0.5;
/// The channel level from which an HDR master's band Luminance fades, linearly, to
/// none at graphics white. Camera Raw 18.7 changes each channel below about white and
/// not the ones above it: Luminance +100 of every band scales value by 1.67 at 0.5 and
/// by 1.00 from 1 to 8, and turns a color at white almost gray. Fitted with
/// [`HDR_LUMINANCE_VALUE`] on synthetic patches (12 hues, saturations 0.3 to 0.9, 0.2
/// to 3 times white, ±50 and ±100, a linear and the ACR3 profile curve; fades from 0.5
/// to 0.7 and lookup values from 0.4 to 0.6 score alike).
pub(crate) const HDR_LUMINANCE_FADE: f32 = 0.6;
/// How much stronger negative band Luminance is in an HDR master on a color whose
/// brightest channel's scene level is at white, growing from none at
/// [`HDR_LUMINANCE_DARKEN_FROM`]. Camera Raw 18.7's HDR Luminance −100 darkens bright
/// colors just above white more than the SDR grids' change faded by channel gives
/// (docs/records/hdr-stage1-results.md#band-luminance).
pub(crate) const HDR_LUMINANCE_DARKEN: f32 = 0.2;
pub(crate) const HDR_LUMINANCE_DARKEN_FROM: f32 = 0.5;
/// How much stronger positive band Luminance is in an HDR master on a color whose
/// scene luminance has reached [`HDR_LUMINANCE_FADE`], from none at
/// [`HDR_LUMINANCE_LIFT_FROM`], in proportion to how far the profile's curve brightens
/// the color, in full from [`HDR_LUMINANCE_LIFT_CURVE`] stops. Camera Raw 18.7's HDR
/// Luminance +100 lifts colors a stop or two under white 1.3–1.6 times as much as the
/// SDR grids give on photos through Adobe Standard; through a linear profile, which
/// leaves the scene as it is, its HDR and SDR changes agree
/// (docs/records/hdr-stage1-results.md#band-luminance).
pub(crate) const HDR_LUMINANCE_LIFT: f32 = 0.5;
/// The color's scene luminance (linear ProPhoto) from which [`HDR_LUMINANCE_LIFT`]
/// grows.
pub(crate) const HDR_LUMINANCE_LIFT_FROM: f32 = 0.35;
/// How far, in stops, the profile's curve brightens a color's luminance for the full
/// [`HDR_LUMINANCE_LIFT`].
pub(crate) const HDR_LUMINANCE_LIFT_CURVE: f32 = 0.5;
/// ProPhoto RGB's luminance weights.
const PRO_LUMA: [f32; 3] = [0.2880402, 0.7118741, 0.0000857];

/// The share of [`HDR_LUMINANCE_LIFT`] a color takes: `scene`, its channels' scene
/// levels, and `rendered`, the same color after the profile's curve, both linear
/// ProPhoto.
pub(crate) fn hdr_luminance_lift(scene: [f32; 3], rendered: [f32; 3]) -> f32 {
    let luma = |q: [f32; 3]| q[0] * PRO_LUMA[0] + q[1] * PRO_LUMA[1] + q[2] * PRO_LUMA[2];
    let (scene, rendered) = (luma(scene).max(1e-6), luma(rendered));
    let level = (scene - HDR_LUMINANCE_LIFT_FROM) / (HDR_LUMINANCE_FADE - HDR_LUMINANCE_LIFT_FROM);
    let curve = (rendered / scene).max(1e-6).log2() / HDR_LUMINANCE_LIFT_CURVE;
    level.clamp(0., 1.) * curve.clamp(0., 1.)
}
impl ColorMixer {
    #[cfg(test)]
    pub(crate) fn new(r: &Recipe) -> Option<Self> {
        Self::for_source(r, false)
    }

    /// Builds the combined mixer grid for a RAW (`is_rendered == false`) or rendered
    /// file (`is_rendered == true`). On RAW SDR renders, Saturation and Vibrance apply
    /// before the profile tone curve ([`apply_raw_saturation`] and [`RawVibrance`]), so
    /// the mixer holds HSL bands only. SDR rendered files use [`RENDERED_SAT_VIB`] for
    /// Saturation and Vibrance; HDR masters keep the legacy HDR tables bit-identical.
    pub(crate) fn for_source(r: &Recipe, is_rendered: bool) -> Option<Self> {
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
        if !is_rendered && !r.hdr.is_active() {
            if active.is_empty() {
                return None;
            }
            return Some(Self {
                delta: grid(&active),
                saturation_gray: 0.,
                gray_source: None,
                hdr: None,
            });
        }
        if is_rendered && !r.hdr.is_active() {
            debug_assert_eq!(RENDERED_SAT_VIB.len(), RENDERED_TABLES * TABLE * 2);
            let has_sat = r.saturation != 0.;
            let has_vib = r.vibrance != 0.;
            if active.is_empty() && !has_sat && !has_vib {
                return None;
            }
            let saturation_gray =
                ((RENDERED_GRAY_FROM - r.saturation) / (1. + RENDERED_GRAY_FROM)).clamp(0., 1.);
            let with_vib = |mut g: Vec<[f32; 3]>| {
                if has_vib {
                    add_rendered_slider(
                        &mut g,
                        &VIBRANCE_KNOTS,
                        RENDERED_SAT_KNOTS.len(),
                        r.vibrance,
                        r.vibrance < 0.,
                    );
                }
                g
            };
            let mut delta = grid(&active);
            if has_sat {
                add_rendered_slider(
                    &mut delta,
                    &RENDERED_SAT_KNOTS,
                    0,
                    r.saturation.max(RENDERED_GRAY_FROM),
                    true,
                );
            }
            let delta = with_vib(delta);
            // While colors fade to gray, HSL and Vibrance still set its brightness.
            let gray_source = (saturation_gray > 0. && (!active.is_empty() || has_vib))
                .then(|| with_vib(grid(&active)));
            return Some(Self {
                delta,
                saturation_gray,
                gray_source,
                hdr: None,
            });
        }
        // HDR masters keep the legacy tables bit-identical.
        let saturation = r.saturation.max(GRAY_FROM);
        // Up to ±50 Vibrance is the photo tables, scaled; beyond, `vibrance_chart.bin`.
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
        // Vibrance's value factors, which fade above white, and band Luminance's, which
        // fade by each channel's level.
        let fading = |active: &[(usize, f32)]| {
            let vibrance: Vec<_> = active
                .iter()
                .copied()
                .filter(|(t, _)| *t == 50 || *t == 51)
                .collect();
            let mut g = grid(&vibrance);
            if chart_vibrance {
                add_vibrance(&mut g, r.vibrance);
            }
            let luminance: Vec<_> = active
                .iter()
                .copied()
                .filter(|(t, _)| *t < 48 && (t / 2) % 3 == 2)
                .collect();
            HdrFactors {
                vibrance: g.into_iter().map(|d| d[2]).collect(),
                luminance: (!luminance.is_empty()).then(|| grid(&luminance)),
            }
        };
        let hdr = r
            .hdr
            .is_active()
            .then(|| [fading(&active), fading(&others)]);
        Some(Self {
            delta: with_vibrance(grid(&active)),
            saturation_gray,
            gray_source,
            hdr,
        })
    }

    /// An HDR master's value factors applied apart ([`Lookup::Hdr`]): the changes'
    /// grid's, then the gray source's; `None` for every other render. For the GPU port.
    pub(crate) fn hdr_fading(&self) -> Option<&[HdrFactors; 2]> {
        self.hdr.as_ref()
    }
    /// `rgb` is linear display RGB (sRGB primaries); above white in an HDR master.
    pub(crate) fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.apply_at(rgb, None)
    }
    /// As [`Self::apply`], in an HDR master whose pixel's scene color (linear
    /// ProPhoto, before the profile's curve) is `scene`: band Luminance fades by it
    /// ([`Lookup::Hdr`]).
    pub(crate) fn apply_at(&self, rgb: [f32; 3], scene: Option<[f32; 3]>) -> [f32; 3] {
        let lookup = |i: usize| match &self.hdr {
            Some(fading) => Lookup::Hdr(Some(&fading[i]), scene),
            None => Lookup::Sdr,
        };
        let mixed = tables_in(&self.delta, rgb, lookup(0));
        if self.saturation_gray == 0. {
            return mixed;
        }
        // Camera Raw's −100 keeps the luminance the color has without Saturation.
        let source = self
            .gray_source
            .as_ref()
            .map_or(rgb, |g| tables_in(g, rgb, lookup(1)));
        let y = crate::color::luminance(source);
        mixed.map(|v| v + self.saturation_gray * (y - v))
    }
}

/// `rgb` (linear display RGB) through a grid of changes, looked up as `lookup` says.
pub(crate) fn tables_in(delta: &[[f32; 3]], rgb: [f32; 3], lookup: Lookup) -> [f32; 3] {
    let p = mul(crate::camera_profiles::RGB_TO_PRO, rgb).map(|v| v.max(0.));
    let Some([h, s, max]) = hsv(p) else {
        return rgb;
    };
    // An HDR master's band Luminance, applied per channel below.
    let mut luminance = 0.;
    let [dh, ds, dv] = match lookup {
        Lookup::Sdr => lookup_with(h, s, max, |cell| delta[cell]),
        Lookup::Hdr(factors, _) => {
            let low = max.min(HDR_VALUE);
            let [mut dh, mut ds, mut dv] = lookup_with(h, s, low, |cell| delta[cell]);
            if let Some(f) = factors {
                let one = |f: &[f32], at: f32| lookup_with(h, s, at, |cell| [0., 0., f[cell]])[2];
                let fade = ((1. - max) / (1. - HDR_FADE)).clamp(0., 1.);
                dv += fade * one(&f.vibrance, max) - one(&f.vibrance, low);
                // Band Luminance's change is looked up at its own value: its value
                // factor leaves `dv`, to apply per channel below, and its hue and
                // saturation join the others, as SDR's up to HDR_LUMINANCE_VALUE and
                // moving back to the others' lookup by white.
                if let Some(l) = f.luminance.as_deref() {
                    let within = lookup_with(h, s, low, |cell| l[cell]);
                    let own = lookup_with(h, s, max.min(HDR_LUMINANCE_VALUE), |cell| l[cell]);
                    let keep = 1.
                        - ((max - HDR_LUMINANCE_VALUE) / (1. - HDR_LUMINANCE_VALUE)).clamp(0., 1.);
                    dh += keep * (own[0] - within[0]);
                    ds += keep * (own[1] - within[1]);
                    dv -= within[2];
                    luminance = own[2];
                }
            }
            [dh, ds, dv]
        }
    };
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
    let q = q.map(|v| v + (max * dv.exp2()) - c);
    // Band Luminance in an HDR master: each channel by its scene level, as the other
    // changes have changed it.
    let q = if luminance == 0. {
        q
    } else {
        let scene = match lookup {
            Lookup::Hdr(_, scene) => scene,
            Lookup::Sdr => None,
        };
        let levels: [f32; 3] = std::array::from_fn(|i| {
            scene.map_or(q[i], |scene| {
                if p[i] > 1e-9 {
                    scene[i].max(0.) * q[i] / p[i]
                } else {
                    q[i]
                }
            })
        });
        // Darkening grows with the color's brightest channel towards white, and
        // brightening with its luminance where the profile's curve brightens it.
        let top = levels.into_iter().fold(0f32, f32::max);
        let luminance = if luminance < 0. {
            let t = (top - HDR_LUMINANCE_DARKEN_FROM) / (1. - HDR_LUMINANCE_DARKEN_FROM);
            luminance * (1. + HDR_LUMINANCE_DARKEN * t.clamp(0., 1.))
        } else {
            luminance * (1. + HDR_LUMINANCE_LIFT * hdr_luminance_lift(levels, q))
        };
        std::array::from_fn(|i| {
            let weight = ((1. - levels[i]) / (1. - HDR_LUMINANCE_FADE)).clamp(0., 1.);
            q[i] * (luminance * weight).exp2()
        })
    };
    mul(crate::camera_profiles::PRO_TO_RGB, q)
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
        // Saturation −100 turns any color to exact ProPhoto luminance gray on RAW SDR,
        // and approaches gray through the rendered/HDR mixer too.
        let pro_skin = mul(crate::camera_profiles::RGB_TO_PRO, skin);
        let raw_gray = apply_raw_saturation(pro_skin, -1.);
        let spread = |p: [f32; 3]| {
            p.iter().fold(0f32, |a, b| a.max(*b)) - p.iter().fold(1f32, |a, b| a.min(*b))
        };
        assert!(spread(raw_gray) < 1e-6, "{raw_gray:?}");
        let r = Recipe {
            saturation: -1.,
            ..Default::default()
        };
        let p = ColorMixer::for_source(&r, true).unwrap().apply(skin);
        assert!(spread(p) < spread(skin) * 0.3);
    }
    #[test]
    fn gray_saturation_reaches_the_colors_luminance_at_minus_100() {
        // HDR masters keep the legacy −50..−100 fade to luminance in ColorMixer.
        let recipe = |saturation| {
            let mut r = Recipe {
                saturation,
                ..Default::default()
            };
            r.hdr.enabled = true;
            r
        };
        let orange = [0.6, 0.3, 0.1];
        let y = crate::color::luminance(orange);
        let at = |s| {
            let m = ColorMixer::new(&recipe(s)).unwrap();
            let mixed = tables_in(&m.delta, orange, Lookup::Sdr);
            if m.saturation_gray == 0. {
                return mixed;
            }
            let src = m
                .gray_source
                .as_deref()
                .map_or(orange, |g| tables_in(g, orange, Lookup::Sdr));
            let y = crate::color::luminance(src);
            mixed.map(|v| v + m.saturation_gray * (y - v))
        };
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
        let src = tables_in(mixer.gray_source.as_deref().unwrap(), orange, Lookup::Sdr);
        let y_src = crate::color::luminance(src);
        let mut no_saturation = darker;
        no_saturation.saturation = 0.;
        let alone = tables_in(
            &ColorMixer::new(&no_saturation).unwrap().delta,
            orange,
            Lookup::Sdr,
        );
        let y_alone = crate::color::luminance(alone);
        assert!((y_src - y_alone).abs() < 1e-5, "{y_src} vs {y_alone}");
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
        // HDR masters keep the legacy photo + vibrance_chart tables.
        let at = |vibrance| {
            let mut r = Recipe {
                vibrance,
                ..Default::default()
            };
            r.hdr.enabled = true;
            ColorMixer::new(&r).unwrap()
        };
        let skin = [0.6, 0.35, 0.25];
        let sat = |p: [f32; 3]| {
            let max = p.iter().fold(0f32, |a, b| a.max(*b));
            (max - p.iter().fold(1f32, |a, b| a.min(*b))) / max
        };
        // Vibrance −100 removes most of a color's saturation, as Camera Raw does.
        let chart = sat(tables_in(&at(-1.).delta, skin, Lookup::Sdr));
        assert!(chart < sat(skin) * 0.4, "{chart}");
        assert!(
            tables_in(&at(0.7).delta, [0.2; 3], Lookup::Sdr)
                .iter()
                .all(|v| (v - 0.2).abs() < 2e-3)
        );
        // Negative Vibrance down to −50 keeps the photo tables, scaled linearly in
        // hue/value and linearly in the saturation factor.
        let (full, half) = (at(-0.5).delta, at(-0.25).delta);
        for (f, h) in full.iter().zip(&half) {
            let expected = (1. + 0.5 * (f[1].exp2() - 1.)).max(1e-3).log2();
            assert!((h[1] - expected).abs() < 1e-5);
            assert!((h[0] - f[0] * 0.5).abs() < 1e-6 && (h[2] - f[2] * 0.5).abs() < 1e-6);
        }
    }
    /// In an HDR master band Luminance changes each channel by its scene level, as
    /// Camera Raw 18.7 renders it (docs/hdr.md#the-color-stage): fully up to
    /// [`HDR_LUMINANCE_FADE`], not at all from white up, so a color whose channels
    /// are all above white keeps its value and one at white turns toward gray.
    #[test]
    fn hdr_band_luminance_fades_out_by_each_channels_level() {
        let mut r = Recipe::default();
        for band in &mut r.hsl {
            band[2] = 1.;
        }
        r.hdr.enabled = true;
        let mixer = ColorMixer::new(&r).unwrap();
        let pro = |rgb: [f32; 3]| mul(crate::camera_profiles::RGB_TO_PRO, rgb);
        let rgb = |pro: [f32; 3]| mul(crate::camera_profiles::PRO_TO_RGB, pro);
        // An orange in ProPhoto, its channels up to 0.6: brightened in full.
        let orange = [0.6, 0.36, 0.14];
        let low = pro(mixer.apply(rgb(orange)));
        assert!(low[0] > orange[0] * 1.1, "{low:?}");
        // All channels above white: the value stays, and only the tables' small hue
        // and saturation changes move the others.
        let bright = orange.map(|v| v * 10.);
        let out = pro(mixer.apply(rgb(bright)));
        assert!((out[0] / bright[0] - 1.).abs() < 1e-4, "{out:?}");
        for c in 1..3 {
            assert!((out[c] / bright[c]).log2().abs() < 0.3, "{out:?}");
        }
        // At white the top channel stays and the others rise: toward gray.
        let white = [1., 0.4, 0.2];
        let out = pro(mixer.apply(rgb(white)));
        assert!((out[0] - 1.).abs() < 1e-4, "{out:?}");
        assert!(out[1] > 0.45 && out[2] > 0.22, "{out:?}");
        // The fade reads the scene's level: a color that a brightening curve took from
        // a scene below HDR_LUMINANCE_FADE up to white takes the full change, as the
        // color at that scene level would.
        let full = pro(mixer.apply_at(rgb(white), Some(white.map(|v| v * 0.5))));
        let gain = |q: [f32; 3]| q[0] / white[0];
        assert!(
            gain(full) > 1.1 && gain(full) > gain(out),
            "{full:?} {out:?}"
        );
    }
    /// Positive band Luminance in an HDR master is stronger on a color the profile's
    /// curve brightens from a scene luminance near [`HDR_LUMINANCE_FADE`], as Camera Raw
    /// 18.7's HDR render of photos through Adobe Standard is; through a linear profile
    /// (scene and rendered alike) it is the SDR grids' change faded by channel, as on
    /// the linear patches (docs/records/hdr-stage1-results.md#band-luminance).
    #[test]
    fn hdr_band_luminance_lifts_where_the_curve_brightens() {
        assert_eq!(hdr_luminance_lift([0.6; 3], [0.6; 3]), 0.);
        assert_eq!(hdr_luminance_lift([0.3; 3], [0.9; 3]), 0.);
        assert!((hdr_luminance_lift([0.6; 3], [0.9; 3]) - 1.).abs() < 1e-6);
        let half = hdr_luminance_lift([0.475; 3], [0.475 * 2f32.powf(0.25); 3]);
        assert!((half - 0.25).abs() < 1e-3, "{half}");
        let mut r = Recipe::default();
        for band in &mut r.hsl {
            band[2] = 1.;
        }
        r.hdr.enabled = true;
        let mixer = ColorMixer::new(&r).unwrap();
        let pro = |rgb: [f32; 3]| mul(crate::camera_profiles::RGB_TO_PRO, rgb);
        let rgb = |pro: [f32; 3]| mul(crate::camera_profiles::PRO_TO_RGB, pro);
        // A warm color at a scene luminance of about half of white: rendered by a
        // linear profile as it is, and by a curve a stop brighter. The curved one's
        // channels gain more (their fade is the same, read from the same scene).
        let scene = [0.55, 0.5, 0.45];
        let gain = |rendered: [f32; 3], out: [f32; 3]| (out[1] / rendered[1]).log2();
        let linear = gain(scene, pro(mixer.apply_at(rgb(scene), Some(scene))));
        let bright = scene.map(|v| v * 2.);
        let curved = gain(bright, pro(mixer.apply_at(rgb(bright), Some(scene))));
        assert!(linear > 0.1 && curved > linear * 1.2, "{curved} {linear}");
        // Negative Luminance is not lifted.
        for band in &mut r.hsl {
            band[2] = -1.;
        }
        let mixer = ColorMixer::new(&r).unwrap();
        let curved = gain(bright, pro(mixer.apply_at(rgb(bright), Some(scene))));
        let linear = gain(scene, pro(mixer.apply_at(rgb(scene), Some(scene))));
        assert!(
            curved < 0. && (curved / linear - 1.).abs() < 0.1,
            "{curved} {linear}"
        );
    }
    /// Below [`HDR_LUMINANCE_VALUE`] and [`HDR_LUMINANCE_FADE`] band Luminance renders
    /// an HDR master's color exactly as SDR does, its hue and saturation changes too:
    /// Camera Raw's HDR and SDR renders of a Luminance edit agree there (the Nikon Z5 II
    /// photo's, to 0.006 stop median below 0.5).
    #[test]
    fn hdr_band_luminance_matches_sdr_below_its_lookup_value() {
        let mut r = Recipe::default();
        r.hsl[3][2] = -0.3;
        r.hsl[5][2] = -0.4;
        r.hsl[1][2] = 0.4;
        let sdr = ColorMixer::new(&r).unwrap();
        r.hdr.enabled = true;
        let hdr = ColorMixer::new(&r).unwrap();
        let rgb = |pro: [f32; 3]| mul(crate::camera_profiles::PRO_TO_RGB, pro);
        for pro in [[0.2, 0.35, 0.15], [0.12, 0.2, 0.42], [0.45, 0.3, 0.1]] {
            let (a, b) = (sdr.apply(rgb(pro)), hdr.apply_at(rgb(pro), Some(pro)));
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-5, "{pro:?}: {a:?} {b:?}");
            }
        }
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

    #[test]
    fn rendered_table_blob_has_expected_size_and_neutrals_stay_neutral() {
        assert_eq!(RENDERED_SAT_VIB.len(), RENDERED_TABLES * TABLE * 2);
        assert!(ColorMixer::for_source(&Recipe::default(), true).is_none());
        for s in [-1., -0.875, -0.75, -0.5, -0.125, 0.125, 0.5, 0.75, 1.] {
            for (sat, vib) in [(s, 0.), (0., s), (s, s * 0.5)] {
                let r = Recipe {
                    saturation: sat,
                    vibrance: vib,
                    ..Default::default()
                };
                let m = ColorMixer::for_source(&r, true).unwrap();
                for level in [0.05, 0.2, 0.7] {
                    let gray = m.apply([level; 3]);
                    assert!(
                        gray.iter()
                            .all(|v| v.is_finite() && (v - level).abs() < 2e-3),
                        "sat={sat} vib={vib} level={level}: {gray:?}"
                    );
                    assert!(
                        (gray[0] - gray[1]).abs() < 1e-4 && (gray[1] - gray[2]).abs() < 1e-4,
                        "sat={sat} vib={vib} level={level}: {gray:?}"
                    );
                }
                assert!(m.apply([0.4, 0.25, 0.1]).iter().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn raw_sdr_saturation_and_vibrance_run_pre_tone_curve_while_keeping_rendered_and_hdr_paths() {
        assert_eq!(
            RAW_VIBRANCE.len(),
            VIBRANCE_KNOTS.len() * RAW_VIBRANCE_TABLE * 2
        );
        // RAW SDR moves Saturation and Vibrance out of ColorMixer (leaving HSL only).
        let r_sv = Recipe {
            saturation: 0.75,
            vibrance: -0.5,
            ..Default::default()
        };
        assert!(ColorMixer::for_source(&r_sv, false).is_none());
        assert!(ColorMixer::for_source(&r_sv, true).is_some());
        let mut r_hsl_sv = r_sv;
        r_hsl_sv.hsl[1][0] = 0.4;
        let mut r_hsl_only = Recipe::default();
        r_hsl_only.hsl[1][0] = 0.4;
        assert_eq!(
            ColorMixer::for_source(&r_hsl_sv, false).unwrap().delta,
            ColorMixer::for_source(&r_hsl_only, false).unwrap().delta
        );

        // Analytic RAW SDR Saturation: exact ProPhoto luminance blend, chroma clamp,
        // neutral invariance and strict monotonicity in HSV saturation.
        let pro = [0.45, 0.25, 0.12];
        assert_eq!(apply_raw_saturation(pro, 0.), pro);
        let y = pro[0] * PRO_LUMA[0] + pro[1] * PRO_LUMA[1] + pro[2] * PRO_LUMA[2];
        let gray = apply_raw_saturation(pro, -1.);
        assert!(gray.iter().all(|v| (v - y).abs() < 1e-6), "{gray:?} vs {y}");
        for level in [0., 0.05, 0.3, 0.85] {
            for s in [-1., -0.5, 0.5, 1.] {
                let g = apply_raw_saturation([level; 3], s);
                assert!(g.iter().all(|v| (v - level).abs() < 1e-6), "{g:?}");
            }
        }
        // Bright saturated magenta that dipped between +50 and +75 under the earlier
        // post-curve blend is strictly monotonic across −100..+100.
        let magenta = [1.0, 0.3, 0.708];
        let mut prev_sat = -1f32;
        for step in -4..=4 {
            let out = apply_raw_saturation(magenta, step as f32 * 0.25);
            let s = hsv(out).map_or(0., |x| x[1]);
            assert!(s >= prev_sat - 1e-6, "step={step}: {s} < {prev_sat}");
            prev_sat = s;
        }
        // High-chroma channel offset clamps to ±scale * (1 - PRO_LUMA[c]) on a [0, 1] pixel
        // (matching Camera Raw's 0.9266 on [0.0501, 0.8767, 0.1211] at Saturation +50), and
        // stays continuous through Saturation 0 on over-range pixels (max(p) > 1).
        let vivid_green = [0.0501, 0.8767, 0.1211];
        let boosted_g = apply_raw_saturation(vivid_green, 0.5);
        let yg = vivid_green[0] * PRO_LUMA[0]
            + vivid_green[1] * PRO_LUMA[1]
            + vivid_green[2] * PRO_LUMA[2];
        assert!(
            (boosted_g[1] - (yg + (1. - PRO_LUMA[1]))).abs() < 1e-5,
            "{boosted_g:?}"
        );
        for over in [[0.05, 1.2, 0.4], [0.1, 1.3, 1.5], [0.2, 0.2, 1.5]] {
            let at0 = apply_raw_saturation(over, 0.);
            for eps in [-0.01, 0.01] {
                let near = apply_raw_saturation(over, eps);
                let step = (0..3)
                    .map(|c| (near[c] - at0[c]).abs())
                    .fold(0f32, f32::max);
                assert!(step < 0.02, "over={over:?} eps={eps}: {at0:?} -> {near:?}");
            }
        }

        // Pre-tone-curve RawVibrance: identity at 0, neutrals/black unchanged, zero hue
        // shift, linear factor interpolation for negative and log interpolation for positive.
        assert!(RawVibrance::new(0.).is_none());
        let skin = [0.42, 0.24, 0.14];
        let h_in = hsv(skin).unwrap()[0];
        for v in [-1., -0.625, -0.5, -0.125, 0.125, 0.5, 0.625, 1.] {
            let rv = RawVibrance::new(v).unwrap();
            assert_eq!(rv.apply([0.; 3]), [0.; 3]);
            for level in [0.02, 0.2, 0.7] {
                let g = rv.apply([level; 3]);
                assert!(
                    g.iter().all(|c| c.is_finite() && (c - level).abs() < 2e-3),
                    "vib={v} level={level}: {g:?}"
                );
                assert!(
                    (g[0] - g[1]).abs() < 1e-5 && (g[1] - g[2]).abs() < 1e-5,
                    "vib={v} level={level}: {g:?}"
                );
            }
            let out = rv.apply(skin);
            let h_out = hsv(out).unwrap()[0];
            let dh = (h_out - h_in + 0.5).rem_euclid(1.) - 0.5;
            assert!(dh.abs() < 1e-4, "vib={v} hue shifted by {dh}");
        }
        assert!(
            hsv(RawVibrance::new(-1.).unwrap().apply(skin)).unwrap()[1]
                < hsv(skin).unwrap()[1] * 0.5
        );
        assert!(
            hsv(RawVibrance::new(1.).unwrap().apply(skin)).unwrap()[1]
                > hsv(skin).unwrap()[1] * 1.1
        );
        let (neg50, neg75, neg625) = (
            RawVibrance::new(-0.5).unwrap().factors,
            RawVibrance::new(-0.75).unwrap().factors,
            RawVibrance::new(-0.625).unwrap().factors,
        );
        for ((a, b), m) in neg50.iter().zip(&neg75).zip(&neg625) {
            assert!((m[0] - 0.5 * (a[0] + b[0])).abs() < 1e-5);
            assert!((m[1] - 0.5 * (a[1] + b[1])).abs() < 1e-5);
        }
        let (pos50, pos75, pos625) = (
            RawVibrance::new(0.5).unwrap().factors,
            RawVibrance::new(0.75).unwrap().factors,
            RawVibrance::new(0.625).unwrap().factors,
        );
        for ((a, b), m) in pos50.iter().zip(&pos75).zip(&pos625) {
            assert!((m[0] - (a[0] * b[0]).sqrt()).abs() < 1e-5);
            assert!((m[1] - (a[1] * b[1]).sqrt()).abs() < 1e-5);
        }
    }

    #[test]
    fn rendered_saturation_fades_from_minus_75_and_interpolates_saturation_factor() {
        let at = |saturation| {
            ColorMixer::for_source(
                &Recipe {
                    saturation,
                    ..Default::default()
                },
                true,
            )
            .unwrap()
        };
        let orange = [0.6, 0.3, 0.1];
        let y = crate::color::luminance(orange);
        // Down to −75 the measured knots render directly with no fade to gray.
        assert_eq!(at(-0.75).saturation_gray, 0.);
        // Halfway between −75 and −100, halfway to luminance.
        let half = at(-0.875).apply(orange);
        let from = at(-0.75).apply(orange);
        assert!((half[0] - (from[0] + y) / 2.).abs() < 1e-5);
        // At −100, exact Rec. 709 luminance, preserving band Luminance via gray_source.
        let gray = at(-1.).apply(orange);
        assert!(gray.iter().all(|v| (v - y).abs() < 1e-5), "{gray:?}");
        let mut darker = Recipe {
            saturation: -1.,
            ..Default::default()
        };
        darker.hsl[1][2] = -1.;
        let toned = ColorMixer::for_source(&darker, true).unwrap().apply(orange);
        let mut no_sat = darker;
        no_sat.saturation = 0.;
        let y_alone =
            crate::color::luminance(ColorMixer::for_source(&no_sat, true).unwrap().apply(orange));
        assert!(
            toned.iter().all(|v| (v - y_alone).abs() < 1e-5),
            "{toned:?}"
        );
        // Between −50 and −75 (and between +25 and +50), saturation interpolates the
        // linear factor 2^ds rather than log2(s).
        let (k50, k75, mid) = (at(-0.5).delta, at(-0.75).delta, at(-0.625).delta);
        for ((a, b), m) in k50.iter().zip(&k75).zip(&mid) {
            let expected = (0.5 * a[1].exp2() + 0.5 * b[1].exp2()).max(1e-3).log2();
            assert!((m[1] - expected).abs() < 1e-5);
            assert!((m[0] - 0.5 * (a[0] + b[0])).abs() < 1e-6);
            assert!((m[2] - 0.5 * (a[2] + b[2])).abs() < 1e-6);
        }
        // Saturation +75 avoids the legacy linear-log extrapolation overshoot on a vivid color.
        let vivid = [0.55, 0.25, 0.12];
        let sat_of = |rgb: [f32; 3]| hsv(mul(crate::camera_profiles::RGB_TO_PRO, rgb)).unwrap()[1];
        let legacy_extrap = {
            let mut r = Recipe {
                saturation: 0.75,
                ..Default::default()
            };
            r.hdr.enabled = true;
            // HDR uses the legacy linear-log extrapolation table (evaluated at SDR value here).
            sat_of(tables_in(
                &ColorMixer::for_source(&r, false).unwrap().delta,
                vivid,
                Lookup::Sdr,
            ))
        };
        let ren_s = sat_of(at(0.75).apply(vivid));
        assert!(
            ren_s > sat_of(vivid) && ren_s < legacy_extrap,
            "{ren_s} vs {legacy_extrap}"
        );
    }

    #[test]
    fn positive_vibrance_boosts_vivid_blues_and_hdr_keeps_legacy_path() {
        let mut r = Recipe {
            vibrance: 1.,
            ..Default::default()
        };
        let ren = ColorMixer::for_source(&r, true).unwrap();
        let raw_vib = RawVibrance::new(1.).unwrap();
        // Bright saturated ProPhoto blue (hue ~247°, s ~0.80, v ~0.84) where the
        // legacy vibrance_chart prior undersaturated vivid colors.
        let blue_pro = [0.20, 0.17, 0.84];
        let blue = mul(crate::camera_profiles::PRO_TO_RGB, blue_pro);
        let sat_of = |rgb: [f32; 3]| hsv(mul(crate::camera_profiles::RGB_TO_PRO, rgb)).unwrap()[1];
        r.hdr.enabled = true;
        let hdr_ren = ColorMixer::for_source(&r, true).unwrap();
        let hdr_raw = ColorMixer::for_source(&r, false).unwrap();
        assert_eq!(hdr_ren.delta, hdr_raw.delta);
        assert_eq!(hdr_ren.saturation_gray, hdr_raw.saturation_gray);
        assert_eq!(hdr_ren.gray_source, hdr_raw.gray_source);
        let fr = hdr_ren.hdr_fading().unwrap();
        let fw = hdr_raw.hdr_fading().unwrap();
        for i in 0..2 {
            assert_eq!(fr[i].vibrance, fw[i].vibrance);
            assert_eq!(fr[i].luminance, fw[i].luminance);
        }
        // HDR master also keeps legacy Saturation extrapolation bit-identical.
        let mut r_sat = Recipe {
            saturation: 1.,
            ..Default::default()
        };
        r_sat.hdr.enabled = true;
        let hdr_sat_raw = ColorMixer::for_source(&r_sat, false).unwrap();
        let hdr_sat_ren = ColorMixer::for_source(&r_sat, true).unwrap();
        assert_eq!(hdr_sat_raw.delta, hdr_sat_ren.delta);
        assert!(
            ColorMixer::for_source(
                &Recipe {
                    saturation: 1.,
                    ..Default::default()
                },
                false
            )
            .is_none()
        );
        // Confirm both rendered and RAW SDR positive Vibrance boost vivid blue more than the legacy grid.
        let legacy_s = sat_of(tables_in(&hdr_raw.delta, blue, Lookup::Sdr));
        assert!(
            sat_of(ren.apply(blue)) > legacy_s + 0.03,
            "ren={} legacy={}",
            sat_of(ren.apply(blue)),
            legacy_s
        );
        let raw_s = hsv(raw_vib.apply(blue_pro)).unwrap()[1];
        assert!(raw_s > legacy_s + 0.02, "raw={raw_s} legacy={legacy_s}");
    }
}
