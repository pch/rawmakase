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
