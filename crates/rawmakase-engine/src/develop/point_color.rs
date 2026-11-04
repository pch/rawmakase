//! Lightroom's Point Color (Color Mixer › Point Color): up to eight sampled colors,
//! each with Hue, Saturation and Luminance shifts, a Variance and a selection range.
//!
//! Measured with Camera Raw 18.7 on synthetic charts (docs/color-mixer.md). Camera Raw
//! works in HSV of linear ProPhoto RGB, after the tone curves, where the color mixer
//! works: a swatch's hue is the HSV hue in sixths of a turn (0–6), its saturation the
//! HSV saturation and its luminance the HSV value. The saturation range is on HSV
//! saturation and the luminance range on sRGB-encoded value. Each range is a trapezoid
//! (outer and inner points), smoothed; Range scales the distance from the swatch before
//! the trapezoids are applied. A pixel's weight is the product of its hue, saturation
//! and luminance weights, faded out towards neutral. Within the selection, Hue turns
//! the HSV hue (±35° at ±100), Saturation and Luminance scale HSV saturation and value,
//! and Variance spreads or gathers hues, saturations and values around the swatch.
//! Swatches apply in turn, each to the result of the ones before. The swatches
//! themselves are [`crate::model::point_color`]'s.
use crate::color::{
    hsv::{hsv_to_rgb, rgb_to_hsv},
    srgb_decode, srgb_encode,
};
use crate::model::point_color::{MAX_SWATCHES, PointColor, SwatchView};
use std::f32::consts::TAU;

/// Why the dropper did not add a swatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleRefusal {
    /// Saturation below where every selection fades out.
    TooNeutral,
    TooDark,
    /// A swatch of this color is already there.
    AlreadySampled,
    /// Eight swatches already.
    Full,
}

impl SampleRefusal {
    /// What the status line says.
    pub fn message(self) -> &'static str {
        match self {
            Self::TooNeutral => "Too neutral to sample: click a more colorful area",
            Self::TooDark => "Too dark to sample: click a brighter area",
            Self::AlreadySampled => "That color already has a swatch",
            Self::Full => "Point Color holds up to 8 swatches",
        }
    }
}

/// Adds a swatch of `source` (as `point_color_pick` gives it) with the default
/// ranges, and returns its index.
pub fn add_sample(list: &mut Vec<PointColor>, source: [f32; 3]) -> Result<usize, SampleRefusal> {
    let [h, s, v] = source;
    if list.len() >= MAX_SWATCHES {
        return Err(SampleRefusal::Full);
    }
    if s < fit::NEUTRAL {
        return Err(SampleRefusal::TooNeutral);
    }
    if srgb_encode(v) < TOO_DARK {
        return Err(SampleRefusal::TooDark);
    }
    let close = |p: &PointColor| {
        let dh = (p.source[0] - h).rem_euclid(6.);
        dh.min(6. - dh) < 0.05
            && (p.source[1] - s).abs() < 0.02
            && (srgb_encode(p.source[2]) - srgb_encode(v)).abs() < 0.02
    };
    if list.iter().any(close) {
        return Err(SampleRefusal::AlreadySampled);
    }
    list.push(PointColor::sampled([h.rem_euclid(6.).min(5.9999), s, v]));
    Ok(list.len() - 1)
}

/// Encoded value below which the dropper refuses a color as too dark.
const TOO_DARK: f32 = 0.06;

/// Half the hue window at Range 50, in radians: where the hue range's outer points
/// sit by default.
pub const HUE_WINDOW: f32 = fit::HUE_WINDOW;

/// Fitted constants (docs/color-mixer.md#point-color).
mod fit {
    /// Half the hue window at Range 50, in radians.
    pub(super) const HUE_WINDOW: f32 = 0.7236;
    /// How Range scales the distance from the swatch, per dimension (hue, saturation,
    /// luminance): 2^(k · (0.5 − Range)), with k below and above Range 50.
    pub(super) const RANGE_BELOW: [f32; 3] = [0.767, 1.7637, 0.5482];
    pub(super) const RANGE_ABOVE: [f32; 3] = [0.1186, 0.3886, 0.0598];
    /// Exponents of the smoothed ramps: hue, then saturation and luminance rising and
    /// falling.
    pub(super) const HUE_RAMP: f32 = 1.0282;
    pub(super) const SATURATION_RAMP: [f32; 2] = [1.8348, 0.9897];
    pub(super) const LUMINANCE_RAMP: [f32; 2] = [2.7765, 1.1354];
    /// Saturation below which pixels fade out of every selection.
    pub(super) const NEUTRAL: f32 = 0.0578;
    /// Hue turn at Hue ±100, radians (35°).
    pub(super) const HUE_SHIFT: f32 = 0.6109;
    /// Saturation ±100 multiplies or divides saturation by 1.9.
    pub(super) const SATURATION_SHIFT: f32 = 0.9;
    /// Luminance: log2 of the value factor is k1·|x| + k2·x².
    pub(super) const LUMINANCE_SHIFT: [f32; 2] = [1.3932, -0.2366];
    /// Variance ±100 scales distances from the swatch in saturation and encoded value
    /// by 2^±k (hue distances by 1 ± Variance).
    pub(super) const VARIANCE_SATURATION: f32 = 0.7904;
    pub(super) const VARIANCE_LUMINANCE: f32 = 0.2123;
}

/// A swatch prepared for rendering. `params` is the flattened form the GPU port reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Swatch {
    hue: f32,
    saturation: f32,
    luminance: f32,
    scale: [f32; 3],
    hue_points: [f32; 4],
    saturation_points: [f32; 4],
    luminance_points: [f32; 4],
    hue_turn: f32,
    log_saturation: f32,
    log_value: f32,
    variance: f32,
    view: SwatchView,
}

/// Length of one swatch in `PointColors::params`.
pub(crate) const SWATCH_PARAMS: usize = 23;
/// Length of the fitted constants that precede the swatches in `PointColors::params`.
pub(crate) const CONSTANT_PARAMS: usize = 8;

impl Swatch {
    fn new(p: &PointColor) -> Self {
        let scale = std::array::from_fn(|d| {
            let k = if p.range < 0.5 {
                fit::RANGE_BELOW[d]
            } else {
                fit::RANGE_ABOVE[d]
            };
            (k * (0.5 - p.range)).exp2()
        });
        let [dh, ds, dl] = p.shift;
        let l = dl.abs();
        Self {
            hue: p.source[0] / 6. * TAU,
            saturation: p.source[1],
            luminance: srgb_encode(p.source[2]),
            scale,
            hue_points: p.hue_range.map(|x| (x - 0.5) * 2. * fit::HUE_WINDOW),
            saturation_points: p.saturation_range,
            luminance_points: p.luminance_range,
            hue_turn: dh * fit::HUE_SHIFT,
            log_saturation: (1. + fit::SATURATION_SHIFT * ds.abs()).log2() * ds.signum(),
            log_value: (fit::LUMINANCE_SHIFT[0] * l + fit::LUMINANCE_SHIFT[1] * l * l)
                * dl.signum(),
            variance: p.variance,
            view: p.view,
        }
    }
    fn params(&self) -> [f32; SWATCH_PARAMS] {
        let mut out = [0.; SWATCH_PARAMS];
        out[0] = self.hue;
        out[1] = self.saturation;
        out[2] = self.luminance;
        out[3..6].copy_from_slice(&self.scale);
        out[6..10].copy_from_slice(&self.hue_points);
        out[10..14].copy_from_slice(&self.saturation_points);
        out[14..18].copy_from_slice(&self.luminance_points);
        out[18] = self.hue_turn;
        out[19] = self.log_saturation;
        out[20] = self.log_value;
        out[21] = self.variance;
        out[22] = match self.view {
            SwatchView::Adjust => 0.,
            SwatchView::VisualizeRange => 1.,
        };
        out
    }
}

/// The active swatches of a recipe, ready to apply.
#[derive(Clone, Debug, PartialEq)]
pub struct PointColors {
    swatches: Vec<Swatch>,
}

impl PointColors {
    pub fn new(list: &[PointColor]) -> Option<Self> {
        let swatches: Vec<_> = list
            .iter()
            .take(MAX_SWATCHES)
            .filter(|p| p.is_valid() && (p.is_active() || p.view == SwatchView::VisualizeRange))
            .map(Swatch::new)
            .collect();
        (!swatches.is_empty()).then_some(Self { swatches })
    }
    /// Flattened for the GPU: the fitted constants (`CONSTANT_PARAMS`), then
    /// `SWATCH_PARAMS` values per swatch.
    pub(crate) fn params(&self) -> Vec<f32> {
        let constants = [
            fit::HUE_RAMP,
            fit::SATURATION_RAMP[0],
            fit::SATURATION_RAMP[1],
            fit::LUMINANCE_RAMP[0],
            fit::LUMINANCE_RAMP[1],
            fit::NEUTRAL,
            fit::VARIANCE_SATURATION,
            fit::VARIANCE_LUMINANCE,
        ];
        debug_assert_eq!(constants.len(), CONSTANT_PARAMS);
        constants
            .into_iter()
            .chain(self.swatches.iter().flat_map(|s| s.params()))
            .collect()
    }
    pub(crate) fn len(&self) -> usize {
        self.swatches.len()
    }
    /// `p` is linear ProPhoto RGB. Swatches apply in turn, each to the result of
    /// the ones before, as Camera Raw's do: a second swatch selects the color the
    /// first has made.
    pub fn apply_prophoto(&self, p: [f32; 3]) -> [f32; 3] {
        self.render_prophoto(p).color
    }
}

/// A color as one swatch sees it, and how much the swatch selects it.
struct Selected {
    hue: f32,
    saturation: f32,
    value: f32,
    encoded_value: f32,
    /// Hue distance from the swatch, radians.
    distance: f32,
    weight: f32,
}

impl Swatch {
    /// `None` for black, which no swatch selects.
    fn select(&self, q: [f32; 3]) -> Option<Selected> {
        if q.into_iter().fold(0f32, f32::max) <= 1e-6 {
            return None;
        }
        let [h, s, v] = rgb_to_hsv(q);
        let ev = srgb_encode(v.min(1.));
        let d = wrap(h - self.hue);
        let weight = smoothed(d * self.scale[0], &self.hue_points, [fit::HUE_RAMP; 2])
            * smoothed(
                self.saturation + (s - self.saturation) * self.scale[1],
                &self.saturation_points,
                fit::SATURATION_RAMP,
            )
            * smoothed(
                self.luminance + (ev - self.luminance) * self.scale[2],
                &self.luminance_points,
                fit::LUMINANCE_RAMP,
            )
            * (s / fit::NEUTRAL).min(1.);
        Some(Selected {
            hue: h,
            saturation: s,
            value: v,
            encoded_value: ev,
            distance: d,
            weight,
        })
    }
    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let q = p.map(|v| v.max(0.));
        let Some(Selected {
            hue: h,
            saturation: s,
            value: v,
            encoded_value: ev,
            distance: d,
            weight,
        }) = self.select(q).filter(|x| x.weight > 0.)
        else {
            return p;
        };
        let turn = weight * (self.hue_turn + self.variance * d);
        let mut log_s = weight * self.log_saturation;
        let mut log_v = weight * self.log_value;
        if self.variance != 0. {
            let spread =
                |x: f32, c: f32, k: f32| (c + (x - c) * (k * self.variance).exp2()).clamp(1e-4, 1.);
            let s2 = spread(s, self.saturation, fit::VARIANCE_SATURATION);
            let v2 = srgb_decode(spread(ev, self.luminance, fit::VARIANCE_LUMINANCE));
            log_s += weight * (s2 / s.max(1e-4)).log2();
            log_v += weight * (v2 / srgb_decode(ev).max(1e-6)).log2();
        }
        let out = hsv_to_rgb(h + turn, (s * log_s.exp2()).clamp(0., 1.), v * log_v.exp2());
        // Channels clip at white: a brightened orange turns towards yellow rather
        // than past white. What HSV cannot carry (negative channels) stays as an
        // offset.
        std::array::from_fn(|c| out[c].min(q[c].max(1.)) + (p[c] - q[c]))
    }
}

/// A pixel after Point Color: its color and, for Visualize Range, how much the
/// visualized swatch selects it (the finished color is grayed by the rest, see
/// [`visualize`]).
pub(crate) struct Rendered {
    pub(crate) color: [f32; 3],
    pub(crate) selection: Option<f32>,
}

impl PointColors {
    /// As [`Self::apply_prophoto`], also noting the visualized swatch's selection of
    /// the color it sees.
    pub(crate) fn render_prophoto(&self, p: [f32; 3]) -> Rendered {
        let mut color = p;
        let mut selection = None;
        for w in &self.swatches {
            if w.view == SwatchView::VisualizeRange {
                let q = color.map(|v| v.max(0.));
                selection = Some(w.select(q).map_or(0., |x| x.weight));
            }
            color = w.apply(color);
        }
        Rendered { color, selection }
    }
}

/// Visualize Range on a finished, encoded color: gray where the swatch selects
/// nothing, the color where it selects all, so later color controls don't tint what
/// it leaves out.
pub(crate) fn visualize(out: [f32; 3], selection: f32) -> [f32; 3] {
    let y: f32 = [0.2126, 0.7152, 0.0722]
        .iter()
        .zip(out)
        .map(|(k, v)| k * srgb_decode(v))
        .sum();
    let gray = srgb_encode(y);
    out.map(|v| gray + (v - gray) * selection)
}

/// Hue difference in radians, −π to π.
fn wrap(d: f32) -> f32 {
    (d + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3. - 2. * t)
}

/// A trapezoid over `x` with outer points `p[0]`, `p[3]` and inner points `p[1]`,
/// `p[2]`, its rising and falling ramps smoothed and raised to `ramp`.
fn smoothed(x: f32, p: &[f32; 4], ramp: [f32; 2]) -> f32 {
    let rise = if x >= p[1] {
        1.
    } else if x <= p[0] {
        0.
    } else {
        smoothstep((x - p[0]) / (p[1] - p[0])).powf(ramp[0])
    };
    let fall = if x <= p[2] {
        1.
    } else if x >= p[3] {
        0.
    } else {
        smoothstep((p[3] - x) / (p[3] - p[2])).powf(ramp[1])
    };
    rise.min(fall)
}

#[cfg(test)]
mod tests;
