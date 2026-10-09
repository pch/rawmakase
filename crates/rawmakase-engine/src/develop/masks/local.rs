//! A mask's adjustment as per-pixel changes to the normal pipeline. Each pixel gets
//! the weighted sum of its masks' slider values (a [`LocalDelta`]), and the pipeline
//! applies it where the matching global control acts:
//!
//! - Temp and Tint scale the camera channels before white balance, by the white
//!   balance change of a ±50 mired / ±50 tint shift at ±100.
//! - Exposure scales linear light with the global Exposure; Color tints it.
//! - Contrast, Highlights, Shadows, Whites, Blacks and Dehaze run the same measured
//!   Lightroom responses as the global sliders, at the pixel's slider values.
//! - Texture and Clarity scale the local-contrast detail of the camera image.
//! - Hue and Saturation rotate and scale Oklab chroma after the colour mixer.
//! - Sharpness and Noise change the finishing sharpening and noise reduction.
use crate::model::masks::LocalAdjust;
use crate::{camera_data::Metadata, model::recipe::Recipe};

/// Slots of a [`LocalDelta`].
pub(crate) mod slot {
    pub(crate) const TEMPERATURE: usize = 0;
    pub(crate) const TINT: usize = 1;
    pub(crate) const EXPOSURE: usize = 2;
    pub(crate) const CONTRAST: usize = 3;
    pub(crate) const HIGHLIGHTS: usize = 4;
    pub(crate) const SHADOWS: usize = 5;
    pub(crate) const WHITES: usize = 6;
    pub(crate) const BLACKS: usize = 7;
    pub(crate) const TEXTURE: usize = 8;
    pub(crate) const CLARITY: usize = 9;
    pub(crate) const DEHAZE: usize = 10;
    pub(crate) const HUE: usize = 11;
    pub(crate) const SATURATION: usize = 12;
    pub(crate) const SHARPNESS: usize = 13;
    pub(crate) const NOISE: usize = 14;
    /// Three log2 channel gains of linear light.
    pub(crate) const COLOR: usize = 15;
}
pub(crate) const LEN: usize = 18;
/// Slider values at one pixel, summed over its masks.
pub(crate) type LocalDelta = [f32; LEN];

/// Temp/Tint at ±1 as a mired and tint shift.
const MIRED: f32 = 50e-6;
const TINT: f32 = 50.;

/// A mask's adjustment as the slider changes the renderer applies.
pub(crate) trait LocalDeltas {
    /// The adjustment as slider values, times `amount`.
    fn delta(&self, amount: f32) -> LocalDelta;
}
impl LocalDeltas for LocalAdjust {
    fn delta(&self, amount: f32) -> LocalDelta {
        let mut d = [0.; LEN];
        d[slot::TEMPERATURE] = self.temperature;
        d[slot::TINT] = self.tint;
        d[slot::EXPOSURE] = self.exposure;
        d[slot::CONTRAST] = self.contrast;
        d[slot::HIGHLIGHTS] = self.highlights;
        d[slot::SHADOWS] = self.shadows;
        d[slot::WHITES] = self.whites;
        d[slot::BLACKS] = self.blacks;
        d[slot::TEXTURE] = self.texture;
        d[slot::CLARITY] = self.clarity;
        d[slot::DEHAZE] = self.dehaze;
        d[slot::HUE] = self.hue;
        d[slot::SATURATION] = self.saturation;
        d[slot::SHARPNESS] = self.sharpness;
        d[slot::NOISE] = self.noise;
        let [hue, saturation] = self.color;
        if saturation > 0. {
            // A hue at unit luminance, blended in by the swatch's saturation.
            let t = crate::develop::color::hue_rgb(hue);
            let luma = 0.2627 * t[0] + 0.678 * t[1] + 0.0593 * t[2];
            for c in 0..3 {
                let gain = 1. + saturation * (t[c] / luma - 1.);
                d[slot::COLOR + c] = gain.max(0.05).log2();
            }
        }
        d.map(|v| v * amount)
    }
}
/// Adds `weight` times `delta` to `sum`.
pub(crate) fn accumulate(sum: &mut LocalDelta, delta: &LocalDelta, weight: f32) {
    for (s, d) in sum.iter_mut().zip(delta) {
        *s += d * weight;
    }
}
pub(crate) fn uses(d: &LocalDelta, slots: &[usize]) -> bool {
    slots.iter().any(|s| d[*s] != 0.)
}
pub(crate) const TONE_SLOTS: [usize; 4] =
    [slot::CONTRAST, slot::WHITES, slot::BLACKS, slot::DEHAZE];

/// Per-render constants of the local adjustments.
#[derive(Clone, Debug, Default)]
pub(crate) struct LocalMath {
    /// log2 camera channel gains of Temp +1 and Tint +1.
    pub(crate) white_balance: [[f32; 3]; 2],
}
impl LocalMath {
    pub(crate) fn new(m: &Metadata, r: &Recipe) -> Self {
        let wb = |temperature: f32, tint: f32| {
            r.color_profile(m)
                .and_then(|p| p.white_balance(temperature, tint, m))
        };
        let t = r.temperature;
        let warmer = 1. / (1. / t - MIRED).max(1. / crate::model::recipe::TEMPERATURE_MAX);
        let white_balance = match (wb(t, r.tint), wb(warmer, r.tint), wb(t, r.tint + TINT)) {
            (Some(base), Some(temp), Some(tint)) => {
                let gain = |w: [f32; 3]| std::array::from_fn(|c| (w[c] / base[c]).log2());
                [gain(temp), gain(tint)]
            }
            // Without a profile's white balance model: a plain warm/cool and
            // green/magenta shift of similar size.
            _ => [[0.25, 0., -0.3], [0.12, -0.2, 0.12]],
        };
        Self { white_balance }
    }
    /// Temp and Tint as camera channel gains.
    pub(crate) fn white_balance_gain(&self, d: &LocalDelta) -> [f32; 3] {
        std::array::from_fn(|c| {
            (d[slot::TEMPERATURE] * self.white_balance[0][c]
                + d[slot::TINT] * self.white_balance[1][c])
                .exp2()
        })
    }
}
/// Local Contrast, Whites, Blacks and Dehaze: the measured global curves at the pixel's
/// slider values, applied to ProPhoto-encoded values as DNG RGBTone does.
pub(crate) fn tone(
    d: &LocalDelta,
    p: [f32; 3],
    photo: &crate::develop::basic_tone::PhotoTone,
) -> [f32; 3] {
    let curve = |x| {
        crate::develop::basic_tone::compose(
            d[slot::CONTRAST],
            d[slot::WHITES],
            d[slot::BLACKS],
            d[slot::DEHAZE],
            photo,
            x,
        )
    };
    let p = p.map(|v| v.clamp(0., 1.));
    let lo = p.into_iter().fold(f32::INFINITY, f32::min);
    let hi = p.into_iter().fold(0f32, f32::max);
    let (a, b) = (curve(lo), curve(hi));
    if hi - lo > 1e-8 {
        p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
    } else {
        [a; 3]
    }
}
/// Local Hue (degrees) and Saturation on Oklab.
pub(crate) fn hue_saturation(d: &LocalDelta, mut lab: [f32; 3]) -> [f32; 3] {
    let (hue, saturation) = (d[slot::HUE], d[slot::SATURATION]);
    if hue == 0. && saturation == 0. {
        return lab;
    }
    let (s, c) = hue.to_radians().sin_cos();
    let k = (1. + saturation).max(0.);
    let (a, b) = (lab[1], lab[2]);
    lab[1] = (a * c - b * s) * k;
    lab[2] = (a * s + b * c) * k;
    lab
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deltas_scale_with_amount_and_neutral_color_adds_nothing() {
        let a = LocalAdjust {
            exposure: 1.,
            contrast: 0.5,
            ..Default::default()
        };
        let d = a.delta(0.5);
        assert_eq!(d[slot::EXPOSURE], 0.5);
        assert_eq!(d[slot::CONTRAST], 0.25);
        assert!(d[slot::COLOR..].iter().all(|v| *v == 0.));
        let tinted = LocalAdjust {
            color: [0.6, 0.5],
            ..Default::default()
        }
        .delta(1.);
        // A blue tint raises blue and keeps luminance near unchanged.
        let g = std::array::from_fn::<f32, 3, _>(|c| tinted[slot::COLOR + c].exp2());
        assert!(g[2] > 1. && g[0] < 1.);
        let luma = 0.2627 * g[0] + 0.678 * g[1] + 0.0593 * g[2];
        assert!((luma - 1.).abs() < 1e-4);
    }
    #[test]
    fn local_tone_is_the_global_curve_and_hue_rotates() {
        use crate::develop::basic_tone::{BasicTone, PhotoTone, TYPICAL_PIVOT, WhitesTable};
        let mut d = [0.; LEN];
        let typical = PhotoTone {
            contrast_pivot: TYPICAL_PIVOT,
            whites: WhitesTable::original(),
        };
        assert_eq!(tone(&d, [0.2, 0.4, 0.6], &typical), [0.2, 0.4, 0.6]);
        d[slot::CONTRAST] = 0.5;
        d[slot::WHITES] = 0.4;
        d[slot::BLACKS] = -0.3;
        let adaptive = PhotoTone {
            contrast_pivot: 0.45,
            whites: WhitesTable::for_highlights(0.8),
        };
        for photo in [typical, adaptive] {
            let global = BasicTone::new(0.5, 0.4, -0.3, 0., &photo).unwrap();
            for p in [[0.2; 3], [0.1, 0.5, 0.9]] {
                let (a, b) = (tone(&d, p, &photo), global.apply(p));
                assert!(
                    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3),
                    "{photo:?}: {a:?} {b:?}"
                );
            }
        }
        d[slot::HUE] = 90.;
        let lab = hue_saturation(&d, [0.5, 0.1, 0.]);
        assert!(lab[1].abs() < 1e-6 && (lab[2] - 0.1).abs() < 1e-6);
    }
}
