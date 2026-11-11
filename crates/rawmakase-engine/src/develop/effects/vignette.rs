//! Lightroom's post-crop vignette, fitted to Camera Raw 18.7 renders of the synthetic
//! chart (the `vignette-*` cases in tests/corpus).
//!
//! The mask rises across an ellipse that fills the frame: Camera Raw's strength is a
//! logistic step in the squared distance from the centre, which Midpoint moves and
//! Feather widens (Feather 100 makes it linear, Feather 0 a hard edge). Roundness bends
//! the ellipse towards a circle or a rounded rectangle.
//!
//! The three styles change pixels differently:
//! - Paint Overlay blends linear output towards black or white.
//! - Highlight Priority changes exposure before the tone curve, so darkening keeps
//!   highlight contrast and lightening lifts shadows most. Highlights protects bright
//!   pixels from darkening.
//! - Color Priority darkens like Highlight Priority mixed with some Paint Overlay, so
//!   highlights darken more, and lightens like Highlight Priority at about half the
//!   strength.
use super::{Effects, VignetteStyle};
use crate::color::{OutputSpace, srgb_decode};
use crate::develop::effects::EffectsRendering;

/// Camera Raw's neutral tone response at default settings, measured on the chart's gray
/// ramp: log2 of linear sRGB output from EV −8 to +4 in half stops, EV 0 being middle
/// gray. Below the table the response is linear; above it, white.
pub(crate) const TONE_LOG2: [f32; 25] = [
    -11.9591, -11.3786, -10.7315, -10.1103, -9.4927, -8.9118, -8.3715, -7.8630, -7.2939, -6.6531,
    -5.9415, -5.1927, -4.4450, -3.7255, -3.0300, -2.3507, -1.7167, -1.1963, -0.7893, -0.4904,
    -0.2758, -0.1308, -0.0448, -0.0049, 0.,
];
const TONE_START_EV: f32 = -8.;
const TONE_STEP_EV: f32 = 0.5;
/// The scene value Highlight and Color Priority lighten towards, in EV.
const SCENE_WHITE_EV: f32 = 3.5;

/// Linear output for a scene exposure in EV.
fn tone(ev: f32) -> f32 {
    let last = TONE_LOG2.len() - 1;
    let at = (ev - TONE_START_EV) / TONE_STEP_EV;
    let log2 = if at <= 0. {
        TONE_LOG2[0] + ev - TONE_START_EV
    } else if at >= last as f32 {
        0.
    } else {
        let i = at as usize;
        TONE_LOG2[i] + (TONE_LOG2[i + 1] - TONE_LOG2[i]) * (at - i as f32)
    };
    log2.exp2()
}
/// The scene exposure, in EV, that `tone` renders as linear output `y` (> 0).
fn tone_ev(y: f32) -> f32 {
    let log2 = y.log2();
    if log2 <= TONE_LOG2[0] {
        return TONE_START_EV + log2 - TONE_LOG2[0];
    }
    let last = TONE_LOG2.len() - 1;
    if log2 >= TONE_LOG2[last] {
        return TONE_START_EV + last as f32 * TONE_STEP_EV;
    }
    let (mut lo, mut hi) = (0, last);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if TONE_LOG2[mid] <= log2 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t = (log2 - TONE_LOG2[lo]) / (TONE_LOG2[hi] - TONE_LOG2[lo]);
    TONE_START_EV + (lo as f32 + t) * TONE_STEP_EV
}
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
/// The scene value, in an HDR master's linear light through a linear profile, that
/// Highlight Priority lightens an HDR master towards: fitted on Camera Raw 18.7's HDR
/// renders of gray tiles from 0.03 to 8 times white at Amount +50, where it is 14.4
/// (13.5–15.2) up to twice white and 14.9–15.2 at 4 and 8 times.
const HDR_SCENE_WHITE: f32 = 14.4;

/// A photo's HDR response to a neutral and its inverse, in log2 on both axes: what
/// Highlight Priority exposes through in an HDR master.
pub(crate) struct HdrResponse {
    outputs: Vec<f32>,
}
const RESPONSE_FIRST: f32 = -20.;
const RESPONSE_STEP: f32 = 1. / 16.;
const RESPONSE_SAMPLES: usize = 30 * 16 + 1;
impl HdrResponse {
    /// From `render`, what the master renders a neutral of scene value `x` as
    /// (`pipeline::hdr_neutral_response`); made non-decreasing so that it inverts.
    pub(crate) fn new(render: impl Fn(f32) -> f32) -> Self {
        let mut previous = f32::NEG_INFINITY;
        let outputs = (0..RESPONSE_SAMPLES)
            .map(|k| {
                let x = (RESPONSE_FIRST + k as f32 * RESPONSE_STEP).exp2();
                previous = previous.max(render(x).max(f32::MIN_POSITIVE).log2());
                previous
            })
            .collect();
        Self { outputs }
    }
    /// The identity, as a linear profile's curve renders in HDR.
    #[cfg(test)]
    pub(crate) fn identity() -> Self {
        Self::new(|x| x)
    }
    /// The output for scene value `x`; along the end samples' slope beyond them.
    pub(crate) fn render(&self, x: f32) -> f32 {
        if x.is_nan() || x <= 0. {
            return 0.;
        }
        let f = (x.log2() - RESPONSE_FIRST) / RESPONSE_STEP;
        let last = self.outputs.len() - 1;
        let i = (f.floor().max(0.) as usize).min(last - 1);
        let t = f - i as f32;
        (self.outputs[i] + (self.outputs[i + 1] - self.outputs[i]) * t).exp2()
    }
    /// The smallest scene value [`Self::render`] takes to `y`.
    pub(crate) fn scene(&self, y: f32) -> f32 {
        if y.is_nan() || y <= 0. {
            return 0.;
        }
        let ly = y.log2();
        let o = &self.outputs;
        let last = o.len() - 1;
        let i = o.partition_point(|&v| v < ly).clamp(1, last);
        let (a, b) = (o[i - 1], o[i]);
        let t = if b > a { (ly - a) / (b - a) } else { 0. };
        (RESPONSE_FIRST + ((i - 1) as f32 + t) * RESPONSE_STEP).exp2()
    }
}

/// The share of a full black overlay that an amount (0 to 1) of darkening applies.
fn darkening(amount: f32) -> f32 {
    1. - srgb_decode(1. - amount)
}

/// A post-crop vignette prepared for one output size: the mask's shape and the style's
/// operator. The GPU preview receives the same fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PostCropVignette {
    /// −1 (darken) to 1 (lighten).
    pub(crate) amount: f32,
    pub(crate) style: VignetteStyle,
    /// Highlights' protection of bright pixels, already zero where Lightroom ignores it.
    pub(crate) highlights: f32,
    /// Horizontal and vertical scale of the normalized position; Roundness above zero
    /// turns the frame's ellipse towards a circle.
    pub(crate) scale: [f32; 2],
    /// Exponent of the distance: 2 for an ellipse, higher towards a rectangle.
    pub(crate) power: f32,
    /// Logit of the squared distance where the mask is one half.
    pub(crate) midpoint: f32,
    /// Width of the step, in logits of the squared distance.
    pub(crate) feather: f32,
}
impl PostCropVignette {
    /// The vignette of `e` on an output of `size` pixels, or None without one.
    pub(crate) fn new(e: &Effects, size: [u32; 2]) -> Option<Self> {
        if e.vignette == 0. {
            return None;
        }
        let roundness = e.vignette_roundness;
        let (scale, power) = if roundness >= 0. {
            // The aspect ratio of the ellipse, from the frame's (Roundness 0) to a circle
            // (100), keeping the corners at the mask's full strength.
            let aspect = (size[0] as f32 / size[1].max(1) as f32).powf(0.92 * roundness);
            let x = (2. * aspect * aspect / (1. + aspect * aspect)).sqrt();
            ([x, x / aspect], 2.)
        } else {
            ([1., 1.], 2. / (1. + 0.95 * roundness))
        };
        let half = 0.5 * (1.27 * (e.vignette_midpoint - 0.5)).exp();
        Some(Self {
            amount: e.vignette,
            style: e.vignette_style,
            highlights: e.vignette_highlight_protection(),
            scale,
            power,
            midpoint: (half / (1. - half)).ln(),
            feather: e.vignette_feather.max(0.005),
        })
    }
    /// The mask, 0 to 1, at a position normalized to −1..1 across the output.
    pub(crate) fn mask(&self, nx: f32, ny: f32) -> f32 {
        let q = (((nx * self.scale[0]).abs().powf(self.power)
            + (ny * self.scale[1]).abs().powf(self.power))
            / 2.)
            .clamp(1e-6, 1. - 1e-6);
        let z = (((q / (1. - q)).ln() - self.midpoint) / self.feather).clamp(-40., 40.);
        1. / (1. + (-z).exp())
    }
    /// A pixel encoded in `space` with the vignette applied at `mask`. The pixel is
    /// decoded and encoded with `space`'s curve, and its luminance has `space`'s
    /// primaries; the sliders' and the mask's mappings, measured on sRGB renders, are
    /// the same in every space.
    pub(crate) fn apply(&self, encoded: [f32; 3], mask: f32, space: OutputSpace) -> [f32; 3] {
        let b = encoded.map(|v| space.decode(v.clamp(0., 1.)));
        let lighten = self.amount > 0.;
        let x = self.amount.abs();
        let linear = match (self.style, lighten) {
            (VignetteStyle::PaintOverlay, false) => self.paint_darken(b, x, mask),
            (VignetteStyle::PaintOverlay, true) => {
                let opacity = srgb_decode(x) * srgb_decode(mask);
                b.map(|v| v + (1. - v) * opacity)
            }
            (style, true) => {
                let strength = if style == VignetteStyle::ColorPriority {
                    0.61
                } else {
                    1.1
                };
                let opacity = strength * srgb_decode(x) * srgb_decode(mask);
                let white = SCENE_WHITE_EV.exp2();
                b.map(|v| {
                    let scene = tone_ev(v.max(1e-12)).exp2();
                    tone((scene + (white - scene) * opacity).log2())
                })
            }
            (style, false) => {
                let shape = 1.7625 + 0.6875 * x * x;
                let gain = 1. - 0.97 * darkening(x) * (1. - (1. - mask).powf(shape));
                let y = space.luminance(b);
                let protect = if self.highlights > 0. {
                    (1. - smoothstep(0.198, 1.108, y)).powf(self.highlights.powf(1.43))
                } else {
                    1.
                };
                let ev = gain.log2() * protect;
                let exposed = b.map(|v| if v > 0. { tone(tone_ev(v) + ev) } else { 0. });
                if style == VignetteStyle::ColorPriority {
                    let painted = self.paint_darken(b, x, mask);
                    let w = 0.13 + 0.19 * x;
                    std::array::from_fn(|c| exposed[c].powf(1. - w) * painted[c].powf(w))
                } else {
                    exposed
                }
            }
        };
        linear.map(|v| space.encode(v.clamp(0., 1.)))
    }
    /// An HDR master's linear pixel (1 = graphics white, not negative, unclipped) with
    /// the vignette applied at `mask`, as Camera Raw 18.7 renders it
    /// (docs/hdr.md#vignette-and-grain). The mask and the amounts' gains and opacities
    /// are SDR's; on linear light, unclipped, the styles apply them as Camera Raw's HDR
    /// renders were measured, which is not always as SDR does (Color Priority):
    /// - darkening by Paint Overlay and Color Priority is a gain of the output, and by
    ///   Highlight Priority the same gain of the scene, through `tone`, the photo's HDR
    ///   response;
    /// - lightening by Paint Overlay and Color Priority blends towards graphics white,
    ///   which darkens light above it, and by Highlight Priority blends the scene
    ///   towards [`HDR_SCENE_WHITE`].
    ///
    /// Highlights' protection is not rendered: an HDR edit that uses it refuses its
    /// master (`model::hdr::check_recipe`).
    pub(crate) fn apply_hdr(&self, p: [f32; 3], mask: f32, tone: &HdrResponse) -> [f32; 3] {
        let x = self.amount.abs();
        let out = match (self.style, self.amount > 0.) {
            (VignetteStyle::PaintOverlay, false) => self.paint_darken(p, x, mask),
            (_, false) => {
                let shape = 1.7625 + 0.6875 * x * x;
                let gain = 1. - 0.97 * darkening(x) * (1. - (1. - mask).powf(shape));
                if self.style == VignetteStyle::HighlightPriority {
                    // As the gain the response gives the pixel's scene, so that a value
                    // the response doesn't reach (above a flat shoulder of the curve,
                    // raised by Contrast) keeps its light as the gain nears 1.
                    p.map(|v| {
                        let scene = tone.scene(v);
                        let at = tone.render(scene);
                        if at > 0. {
                            v * tone.render(scene * gain) / at
                        } else {
                            v * gain
                        }
                    })
                } else {
                    p.map(|v| v * gain)
                }
            }
            (VignetteStyle::HighlightPriority, true) => {
                let opacity = 1.1 * srgb_decode(x) * srgb_decode(mask);
                // As the light the response adds to the pixel's scene, for the same
                // reason.
                p.map(|v| {
                    let scene = tone.scene(v);
                    v + tone.render(scene + (HDR_SCENE_WHITE - scene) * opacity)
                        - tone.render(scene)
                })
            }
            (_, true) => {
                let opacity = srgb_decode(x) * srgb_decode(mask);
                p.map(|v| v + (1. - v) * opacity)
            }
        };
        out.map(|v| v.max(0.))
    }
    fn paint_darken(&self, b: [f32; 3], amount: f32, mask: f32) -> [f32; 3] {
        let gain = 1. - darkening(amount) * (1. - srgb_decode(1. - mask));
        b.map(|v| v * gain)
    }
    /// The WGSL declaration of `TONE_LOG2` for the GPU preview.
    pub(crate) fn wgsl_tone() -> String {
        let values: Vec<String> = TONE_LOG2.iter().map(|v| format!("{v:?}")).collect();
        format!(
            "const TONE_START_EV: f32 = {TONE_START_EV:?};\nconst TONE_STEP_EV: f32 = {TONE_STEP_EV:?};\nconst SCENE_WHITE_EV: f32 = {SCENE_WHITE_EV:?};\nvar<private> TONE_LOG2: array<f32, {}> = array<f32, {}>({});\n",
            TONE_LOG2.len(),
            TONE_LOG2.len(),
            values.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vignette(
        style: VignetteStyle,
        amount: f32,
        edit: impl Fn(&mut Effects),
    ) -> PostCropVignette {
        let mut e = Effects {
            vignette: amount,
            vignette_style: style,
            ..Default::default()
        };
        edit(&mut e);
        PostCropVignette::new(&e, [970, 742]).unwrap()
    }
    /// Linear output over linear input of a neutral pixel at the middle of the frame's
    /// right edge, where Camera Raw's default vignette has half its strength.
    fn edge_gain(v: &PostCropVignette, encoded: f32) -> f32 {
        let out = v.apply([encoded; 3], v.mask(1., 0.), OutputSpace::Srgb);
        srgb_decode(out[1]) / srgb_decode(encoded)
    }
    /// An HDR master's vignette as Camera Raw 18.7 renders gray tiles through a linear
    /// profile (docs/hdr.md#vignette-and-grain): darkening is the same gain at every
    /// level from a tenth of white to eight times it, in every style; Paint Overlay and
    /// Color Priority lighten towards graphics white, darkening light above it (Amount
    /// +50 at a mask where 0.025 becomes 0.139: 8 becomes 7.2); Highlight Priority
    /// lightens towards 14.4.
    #[test]
    fn an_hdr_vignette_renders_light_above_white_as_camera_raw_does() {
        use VignetteStyle::*;
        let tone = HdrResponse::identity();
        for style in [PaintOverlay, HighlightPriority, ColorPriority] {
            let v = vignette(style, -0.5, |_| ());
            let gain = |x: f32| v.apply_hdr([x; 3], 0.7, &tone)[1] / x;
            let g = gain(0.1);
            assert!(g < 0.8, "{style:?} {g}");
            for x in [0.5, 1., 2., 8.] {
                assert!(
                    (gain(x) / g - 1.).abs() < 1e-3,
                    "{style:?} {x}: {}",
                    gain(x)
                );
            }
        }
        for style in [PaintOverlay, ColorPriority] {
            let v = vignette(style, 0.5, |_| ());
            // The mask whose opacity takes 0.025 to 0.139.
            let mask = (0.1..1.).step_by_mask(|m| v.apply_hdr([0.025; 3], m, &tone)[1], 0.139);
            let at = |x: f32| v.apply_hdr([x; 3], mask, &tone)[1];
            assert!((at(1.) - 1.).abs() < 1e-5);
            assert!((at(8.) - 7.18).abs() < 0.05, "{}", at(8.));
            assert!(at(0.5) > 0.55);
        }
        let v = vignette(HighlightPriority, 0.5, |_| ());
        let opacity = 1.1 * srgb_decode(0.5) * srgb_decode(0.6);
        for x in [0.1, 1., 4.] {
            let out = v.apply_hdr([x; 3], 0.6, &tone)[1];
            let white = (out - x) / opacity + x;
            assert!((white - 14.4).abs() < 0.05, "{x}: {white}");
        }
        // Through a curve, Highlight Priority exposes the scene: a gain of the scene,
        // not of the output.
        let curved = HdrResponse::new(|x| x.sqrt());
        let v = vignette(HighlightPriority, -0.5, |_| ());
        let out = v.apply_hdr([1.; 3], 0.7, &curved)[1];
        let flat = v.apply_hdr([1.; 3], 0.7, &tone)[1];
        assert!((out - flat.sqrt()).abs() < 1e-3, "{out} {flat}");
        // A response with a flat shoulder: a value above it, which it doesn't reach,
        // keeps its light under a vignette that changes nothing there.
        let shoulder = HdrResponse::new(|x| x.min(16.));
        for amount in [-0.5, 0.5] {
            let v = vignette(HighlightPriority, amount, |_| ());
            let out = v.apply_hdr([22.; 3], 1e-12, &shoulder)[1];
            assert!((out - 22.).abs() < 1e-3, "{amount}: {out}");
        }
        // The response inverts.
        for x in [1e-4, 0.03, 0.5, 1., 7., 300.] {
            assert!(
                (curved.scene(curved.render(x)) / x - 1.).abs() < 1e-3,
                "{x}"
            );
        }
    }
    /// The mask in `self` at which `f` reaches `target`, `f` rising with the mask.
    trait StepByMask {
        fn step_by_mask(self, f: impl Fn(f32) -> f32, target: f32) -> f32;
    }
    impl StepByMask for std::ops::Range<f64> {
        fn step_by_mask(self, f: impl Fn(f32) -> f32, target: f32) -> f32 {
            let (mut lo, mut hi) = (self.start as f32, self.end as f32);
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                if f(mid) < target { lo = mid } else { hi = mid }
            }
            0.5 * (lo + hi)
        }
    }
    #[test]
    fn mask_has_camera_raw_shape() {
        use VignetteStyle::PaintOverlay;
        let v = vignette(PaintOverlay, -0.6, |_| ());
        // Camera Raw 18.7 on the synthetic chart: nothing in the middle, half strength at
        // the edges' midpoints, full strength in the corners, 0.09 at 70% of the way out.
        for ((x, y), expected) in [
            ((0., 0.), 0.),
            ((1., 0.), 0.5),
            ((0., 1.), 0.5),
            ((0.7, 0.), 0.092),
            ((0.99, 0.99), 1.),
        ] {
            let mask = v.mask(x, y);
            assert!((mask - expected).abs() < 0.01, "({x}, {y}): {mask}");
        }
        // Feather 100 is linear in the squared distance; Feather 0 a hard edge.
        let soft = vignette(PaintOverlay, -0.6, |e| e.vignette_feather = 1.);
        assert!((soft.mask(0.5, 0.) - 0.125).abs() < 0.005);
        let hard = vignette(PaintOverlay, -0.6, |e| e.vignette_feather = 0.);
        assert!(hard.mask(0.97, 0.) < 0.01 && hard.mask(1., 0.2) > 0.99);
        // Midpoint 0 moves half strength in to 73% of the way out; Roundness 100 makes
        // a circle, reaching the short sides' middle sooner than the long sides'.
        let wide = vignette(PaintOverlay, -0.6, |e| e.vignette_midpoint = 0.);
        assert!((wide.mask(0.73, 0.) - 0.5).abs() < 0.03);
        let round = vignette(PaintOverlay, -0.6, |e| e.vignette_roundness = 1.);
        assert!(round.mask(0.9, 0.) > 0.5 && round.mask(0., 1.) < 0.3);
    }
    #[test]
    fn styles_change_tones_as_camera_raw_does() {
        use VignetteStyle::*;
        // Camera Raw 18.7, Amount ∓60 at half mask, on the chart's gray (0.38) and
        // white (0.94) surrounds: linear gain of each style.
        for (style, amount, gray, white) in [
            (PaintOverlay, -0.6, 0.32, 0.32),
            (HighlightPriority, -0.6, 0.235, 0.593),
            (ColorPriority, -0.6, 0.247, 0.535),
            (PaintOverlay, 0.6, 1.485, 1.010),
            (HighlightPriority, 0.6, 3.307, 1.029),
            (ColorPriority, 0.6, 2.291, 1.017),
        ] {
            let v = vignette(style, amount, |_| ());
            let (g, w) = (edge_gain(&v, 0.38), edge_gain(&v, 0.94));
            assert!(
                (g / gray - 1.).abs() < 0.05 && (w / white - 1.).abs() < 0.05,
                "{style:?} {amount}: {g} {w}"
            );
        }
        // Highlights 80 keeps white's contrast without changing the gray.
        let v = vignette(HighlightPriority, -0.6, |e| e.vignette_highlights = 0.8);
        assert!((edge_gain(&v, 0.38) / 0.235 - 1.).abs() < 0.05);
        assert!(edge_gain(&v, 0.94) > 0.7, "{}", edge_gain(&v, 0.94));
    }
}
