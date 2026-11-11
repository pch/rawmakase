//! The SDR rendition's sliders (Camera Raw's HDR "SDR Settings": Brightness, Contrast,
//! Highlights, Shadows and Whites) on an HDR edit's SDR rendition, the edit rendered with
//! HDR off. Camera Raw 18.7 changes its own SDR rendition by them as one curve of
//! luminance, the same curve on every photo measured within the ranges
//! [`crate::model::hdr::SdrRendition::unrendered`] allows; RAWmakase applies that change,
//! measured as log2 luminance against log2 luminance of the default rendition
//! ([`super::sdr_rendition_data`]), to its own rendition. Each colour is scaled as a
//! whole, so its hue and saturation stay. They never change the HDR master
//! (docs/records/hdr-stage1-results.md#sdr-rendition).
use super::sdr_rendition_data::{BRIGHTNESS, CONTRAST, FIRST, HIGHLIGHTS, SHADOWS, STEP, WHITES};
use crate::model::hdr::SdrRendition;

const SAMPLES: usize = 49;
/// A slider's measured curves: (slider value, −1 to 1; change at each sample).
type Curves = [(f32, [f32; SAMPLES])];

/// The SDR rendition's sliders as one change of luminance: log2 gain over log2
/// luminance, sampled as the measured curves are.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SdrRenditionTone {
    gain: [f32; SAMPLES],
}

impl SdrRenditionTone {
    /// The sliders of a recipe's SDR rendition (`Recipe::resolved`), or `None` when none is
    /// set or they don't render ([`SdrRendition::unrendered`]).
    pub(crate) fn new(sdr: &SdrRendition) -> Option<Self> {
        if *sdr == SdrRendition::default() || sdr.unrendered().is_some() {
            return None;
        }
        // In the panel's order, each on the luminance the ones before it leave. Camera
        // Raw's order for several at once was not measured.
        let curves: [(&Curves, f32); 5] = [
            (BRIGHTNESS, sdr.brightness),
            (CONTRAST, sdr.contrast),
            (HIGHLIGHTS, sdr.highlights),
            (SHADOWS, sdr.shadows),
            (WHITES, sdr.whites),
        ];
        let gain = std::array::from_fn(|i| {
            let start = FIRST + i as f32 * STEP;
            let mut x = start;
            for (table, value) in curves {
                if value != 0. {
                    x += change(table, value, x);
                }
            }
            x - start
        });
        Some(Self { gain })
    }

    /// Linear display RGB (sRGB primaries) with its luminance changed as the sliders
    /// change it; black stays black.
    pub(crate) fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let y = crate::color::luminance(rgb);
        // Not above 0 (or NaN): nothing to scale.
        if y.partial_cmp(&0.) != Some(std::cmp::Ordering::Greater) {
            return rgb;
        }
        let scale = sample(&self.gain, y.log2()).exp2();
        rgb.map(|v| v * scale)
    }
}

/// The change in log2 luminance one slider at `value` makes at log2 luminance `x`: its
/// measured curves interpolated linearly in the slider's value, none at 0, between the
/// values measured.
fn change(table: &Curves, value: f32, x: f32) -> f32 {
    let at = |i: usize| -> (f32, f32) {
        let (v, curve) = &table[i];
        (*v, sample(curve, x))
    };
    // The measured values on the slider's side of 0, nearest first, with 0 itself.
    let side: Vec<(f32, f32)> = std::iter::once((0., 0.))
        .chain(
            (0..table.len())
                .map(at)
                .filter(|(v, _)| v.signum() == value.signum()),
        )
        .collect();
    let mut side = side;
    if side.len() < 2 {
        return 0.;
    }
    side.sort_by(|a, b| a.0.abs().total_cmp(&b.0.abs()));
    let pair = side
        .windows(2)
        .find(|w| value.abs() <= w[1].0.abs())
        .unwrap_or(&side[side.len() - 2..]);
    let [(v0, g0), (v1, g1)] = [pair[0], pair[1]];
    g0 + (g1 - g0) * (value - v0) / (v1 - v0)
}

/// A curve sampled every [`STEP`] from [`FIRST`] at `x`, linearly, held at its ends.
fn sample(curve: &[f32; SAMPLES], x: f32) -> f32 {
    let p = ((x - FIRST) / STEP).clamp(0., (SAMPLES - 1) as f32);
    let i = (p as usize).min(SAMPLES - 2);
    let t = p - i as f32;
    curve[i] + (curve[i + 1] - curve[i]) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sliders(set: impl FnOnce(&mut SdrRendition)) -> SdrRendition {
        let mut sdr = SdrRendition::default();
        set(&mut sdr);
        sdr
    }

    /// At the values measured, a slider's change is its measured curve; between 0 and
    /// them, a share of it; nothing renders at 0, or with any slider beyond what was
    /// measured to render.
    #[test]
    fn the_sliders_render_their_measured_change_and_nothing_else() {
        let gray = |y: f32| [y; 3];
        assert_eq!(SdrRenditionTone::new(&SdrRendition::default()), None);
        let half = SdrRenditionTone::new(&sliders(|s| s.brightness = 0.5)).unwrap();
        for x in [-10f32, -6., -2., -0.5] {
            let y = x.exp2();
            let measured = sample(&BRIGHTNESS[1].1, x);
            let got = (half.apply(gray(y))[0] / y).log2();
            assert!((got - measured).abs() < 1e-4, "{x}: {got} {measured}");
            let quarter = SdrRenditionTone::new(&sliders(|s| s.brightness = 0.25)).unwrap();
            let got = (quarter.apply(gray(y))[0] / y).log2();
            assert!((got - measured / 2.).abs() < 1e-4, "{x}: {got}");
        }
        // Brightness +50 brightens by about half a stop, −50 darkens as much.
        let y = 0.1;
        assert!((half.apply(gray(y))[0] / y).log2() > 0.4);
        let darker = SdrRenditionTone::new(&sliders(|s| s.brightness = -0.5)).unwrap();
        assert!((darker.apply(gray(y))[0] / y).log2() < -0.4);
        // Hue and saturation stay: a colour is scaled as a whole.
        let c = half.apply([0.3, 0.1, 0.05]);
        assert!((c[0] / c[1] - 3.).abs() < 1e-4 && (c[1] / c[2] - 2.).abs() < 1e-4);
        assert_eq!(half.apply([0.; 3]), [0.; 3]);
        // Beyond the measured ranges, or Clarity and Blend, nothing renders.
        for unrendered in [
            sliders(|s| s.brightness = -0.6),
            sliders(|s| s.contrast = 0.6),
            sliders(|s| s.whites = 0.7),
            sliders(|s| s.shadows = 0.1),
            sliders(|s| s.highlights = -0.1),
            sliders(|s| s.clarity = 0.2),
            sliders(|s| {
                s.brightness = 0.3;
                s.blend = 0.2
            }),
        ] {
            assert_eq!(SdrRenditionTone::new(&unrendered), None, "{unrendered:?}");
        }
        // Every slider at every value it renders gives a finite change.
        for v in [-1f32, -0.75, -0.5, -0.2, 0.2, 0.5, 0.75, 1.] {
            for set in [
                sliders(|s| s.brightness = v.max(-0.5)),
                sliders(|s| s.contrast = v.min(0.5)),
                sliders(|s| s.whites = v.min(0.5)),
                sliders(|s| s.shadows = -v.abs()),
                sliders(|s| s.highlights = v.abs()),
            ] {
                let t = SdrRenditionTone::new(&set).unwrap();
                assert!(
                    t.gain.iter().all(|g| g.is_finite() && g.abs() < 4.),
                    "{set:?}"
                );
            }
        }
    }
}
