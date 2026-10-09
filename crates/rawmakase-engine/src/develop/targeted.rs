//! Lightroom's Targeted Adjustment Tool: dragging up or down on the photo moves the
//! sliders that change the color under the pointer. The color is sampled where each
//! control sees it (`quality::targeted_sample`), not from the screen, and a drag is
//! shared among the sliders as the render weighs them for that color:
//!
//! - Tone Curve: the parametric region the pixel's curve input falls in.
//! - Color Mixer: each band's measured change to that color (the size of its ±100
//!   tables there, which the slider scales).
//! - The black & white mix: the two bands either side of the color's Oklab hue, as
//!   the render interpolates them.
//!
//! The band most involved moves with the pointer and the others in proportion.
use crate::develop::effects::EffectsRendering;
use crate::model::recipe::Recipe;

/// The Color Mixer's bands, in slider order.
pub const BANDS: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];
/// The parametric curve's regions, in slider order.
pub const REGIONS: [&str; 4] = ["Shadows", "Darks", "Lights", "Highlights"];

/// Slider units (−1..=1) a drag moves the most involved slider by, per point of
/// pointer travel: 250 points take it from 0 to 100.
pub const DRAG_RATE: f32 = 1. / 250.;

/// What the tool adjusts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The parametric curve's regions.
    ToneCurve,
    /// One of the Color Mixer's HSL slider sets.
    Hsl(HslChannel),
    /// The black & white mix.
    BlackWhite,
}

/// The Color Mixer's Hue, Saturation and Luminance sliders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HslChannel {
    Hue,
    Saturation,
    Luminance,
}
impl HslChannel {
    pub const ALL: [Self; 3] = [Self::Hue, Self::Saturation, Self::Luminance];
    /// Its place in a band's `[hue, saturation, luminance]`.
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn name(self) -> &'static str {
        ["Hue", "Saturation", "Luminance"][self.index()]
    }
}

/// A pixel as the controls the tool drives see it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetSample {
    /// The parametric curve's input, 0–1.
    pub tone: f32,
    /// Linear ProPhoto RGB where the color mixer works.
    pub mixer: [f32; 3],
    /// Oklab where the black & white mix takes the hue.
    pub color: [f32; 3],
}

/// Below this Oklab chroma a color has no hue to speak of, and the color controls
/// change it little: the tool leaves such colors alone.
const NEUTRAL_CHROMA: f32 = 0.01;
/// The same for the measured mixer, as HSV saturation of its linear ProPhoto input.
const NEUTRAL_SATURATION: f32 = 0.02;
/// Bands changing a color less than this share of the most involved band are left
/// alone, so measurement noise in unrelated bands doesn't move them.
const MIN_SHARE: f32 = 0.1;

/// How a drag is shared among the target's sliders.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetWeights {
    pub target: Target,
    /// Each slider's share of the drag, the largest 1; regions use the first four.
    pub shares: [f32; 8],
}

impl TargetWeights {
    /// The shares for `sample`, rendered with `r`.
    pub fn new(target: Target, sample: &TargetSample, r: &Recipe) -> Self {
        // Neutral where the shares come from: the measured mixer's input, or the
        // Oklab color the hue weights take.
        let neutral = if matches!(target, Target::Hsl(_)) {
            let max = sample.mixer.into_iter().fold(0f32, f32::max);
            let min = sample.mixer.into_iter().fold(f32::INFINITY, f32::min);
            max <= 1e-6 || (max - min) / max < NEUTRAL_SATURATION
        } else {
            sample.color[1].hypot(sample.color[2]) < NEUTRAL_CHROMA
        };
        let shares = match target {
            Target::Hsl(_) | Target::BlackWhite if neutral => [0.; 8],
            Target::ToneCurve => {
                let mut shares = [0.; 8];
                shares[r.effects.parametric_region(sample.tone)] = 1.;
                shares
            }
            Target::Hsl(channel) => normalized(super::color_mixer::band_responses(
                sample.mixer,
                channel.index(),
            )),
            Target::BlackWhite => hue_shares(sample.color),
        };
        Self { target, shares }
    }
    /// Whether the drag would move nothing: a neutral color, or black.
    pub fn is_empty(&self) -> bool {
        self.shares.iter().all(|s| *s == 0.)
    }
    /// Sets the sliders to `start`'s moved by `amount` (slider units, −1..=1) times
    /// each share, within their range, and turns their panel on.
    pub fn apply(&self, start: &Recipe, amount: f32, r: &mut Recipe) {
        use crate::model::panels::{Panel, PanelState};
        // A change in a panel that is off turns it on, as a slider's does.
        let panel = match self.target {
            Target::ToneCurve => Panel::ToneCurve,
            Target::Hsl(_) => Panel::ColorMixer,
            Target::BlackWhite => Panel::BlackWhiteMix,
        };
        r.panels.set(panel, PanelState::On);
        let moved = |from: f32, share: f32| (from + amount * share).clamp(-1., 1.);
        match self.target {
            Target::ToneCurve => {
                for i in 0..4 {
                    if self.shares[i] > 0. {
                        r.effects.parametric[i] =
                            moved(start.effects.parametric[i], self.shares[i]);
                    }
                }
            }
            Target::Hsl(channel) => {
                let c = channel.index();
                for (band, share) in self.shares.iter().enumerate() {
                    if *share > 0. {
                        r.hsl[band][c] = moved(start.hsl[band][c], *share);
                    }
                }
            }
            Target::BlackWhite => {
                for (band, share) in self.shares.iter().enumerate() {
                    if *share > 0. {
                        r.effects.gray_mix[band] = moved(start.effects.gray_mix[band], *share);
                    }
                }
            }
        }
    }
    /// The History step for a drag that left `r`: the most involved slider and its
    /// value, as the slider itself names it ("Orange Saturation", "+12").
    pub fn step(&self, r: &Recipe) -> (String, String) {
        let main = (0..8)
            .max_by(|a, b| self.shares[*a].total_cmp(&self.shares[*b]))
            .unwrap_or(0);
        let (name, value) = match self.target {
            Target::ToneCurve => (
                format!("Region {}", REGIONS[main.min(3)]),
                r.effects.parametric[main.min(3)],
            ),
            Target::Hsl(channel) => (
                format!("{} {}", BANDS[main], channel.name()),
                r.hsl[main][channel.index()],
            ),
            Target::BlackWhite => (
                format!("Black & White Mix {}", BANDS[main]),
                r.effects.gray_mix[main],
            ),
        };
        (name, format!("{:+.0}", value * 100.))
    }
}

/// The bands either side of the Oklab `color`'s hue, weighted as the render
/// interpolates them.
fn hue_shares(color: [f32; 3]) -> [f32; 8] {
    let hue = color[2].atan2(color[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    normalized(super::pipeline::hue_weights(hue))
}

/// `weights` scaled so the largest is 1, with those under [`MIN_SHARE`] of it
/// dropped; none when every weight is (nearly) zero.
fn normalized(weights: [f32; 8]) -> [f32; 8] {
    let max = weights.iter().copied().fold(0f32, f32::max);
    if max < 1e-4 {
        return [0.; 8];
    }
    weights.map(|w| if w / max < MIN_SHARE { 0. } else { w / max })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An Oklab color of `hue` (turns) and `chroma` at lightness 0.6.
    fn oklab(hue: f32, chroma: f32) -> [f32; 3] {
        let a = hue * std::f32::consts::TAU;
        [0.6, a.cos() * chroma, a.sin() * chroma]
    }

    fn sample(color: [f32; 3]) -> TargetSample {
        TargetSample {
            tone: 0.5,
            mixer: [0.5; 3],
            color,
        }
    }

    #[test]
    fn black_and_white_shares_follow_the_mixs_hue_weights() {
        let r = Recipe::default();
        // Between the orange and yellow centres (0.151 and 0.305), a third of the way.
        let hue = 0.151 + (0.305 - 0.151) / 3.;
        let w = TargetWeights::new(Target::BlackWhite, &sample(oklab(hue, 0.1)), &r);
        let weights = super::super::pipeline::hue_weights(hue);
        assert!((weights[1] - 2. / 3.).abs() < 1e-4, "{weights:?}");
        assert!((w.shares[1] - 1.).abs() < 1e-6, "{:?}", w.shares);
        assert!(
            (w.shares[2] - weights[2] / weights[1]).abs() < 1e-5,
            "{:?}",
            w.shares
        );
        assert!(
            w.shares
                .iter()
                .enumerate()
                .all(|(i, s)| i == 1 || i == 2 || *s == 0.)
        );
        // A neutral has no hue: nothing moves.
        assert!(TargetWeights::new(Target::BlackWhite, &sample(oklab(hue, 0.002)), &r).is_empty());
    }

    #[test]
    fn a_drag_moves_the_main_band_fully_and_the_other_in_proportion() {
        let mut start = Recipe::default();
        start.effects.gray_mix[1] = 0.2;
        let hue = 0.151 + (0.305 - 0.151) / 3.;
        let w = TargetWeights::new(Target::BlackWhite, &sample(oklab(hue, 0.1)), &start);
        let mut r = start.clone();
        w.apply(&start, 0.3, &mut r);
        assert!((r.effects.gray_mix[1] - 0.5).abs() < 1e-6);
        assert!((r.effects.gray_mix[2] - 0.3 * w.shares[2]).abs() < 1e-6);
        assert_eq!(r.effects.gray_mix[0], 0.);
        assert_eq!(
            w.step(&r),
            ("Black & White Mix Orange".to_string(), "+50".to_string())
        );
        // Sliders stop at their ends.
        w.apply(&start, 5., &mut r);
        assert_eq!(r.effects.gray_mix[1], 1.);
    }

    #[test]
    fn the_tone_curve_moves_the_region_the_sample_falls_in() {
        let mut r = Recipe::default();
        r.effects.splits = [0.2, 0.5, 0.8];
        for (tone, region) in [(0.1, 0), (0.3, 1), (0.6, 2), (0.9, 3)] {
            let s = TargetSample {
                tone,
                ..sample([0.6, 0., 0.])
            };
            let w = TargetWeights::new(Target::ToneCurve, &s, &r);
            let mut moved = r.clone();
            w.apply(&r, -0.25, &mut moved);
            let mut expected = [0.; 4];
            expected[region] = -0.25;
            assert_eq!(moved.effects.parametric, expected, "tone {tone}");
            assert_eq!(w.step(&moved).0, format!("Region {}", REGIONS[region]));
        }
    }

    #[test]
    fn shares_rank_bands_by_how_much_they_change_the_color() {
        // A skin tone, in linear ProPhoto RGB as the measured mixer sees it.
        let skin = [0.42, 0.30, 0.22];
        let r = Recipe::default();
        let pro_to_rgb = |p| crate::color::mul(crate::camera_profiles::PRO_TO_RGB, p);
        for channel in HslChannel::ALL {
            let w = TargetWeights::new(
                Target::Hsl(channel),
                &TargetSample {
                    mixer: skin,
                    ..sample(oklab(0.15, 0.08))
                },
                &r,
            );
            // Orange is the band for skin, as in Lightroom.
            assert_eq!(w.shares[1], 1., "{channel:?} {:?}", w.shares);
            // How far each band's slider at +100 moves the color through the mixer:
            // the band moving it most leads, and bands that barely touch it stay put.
            let change: Vec<f32> = (0..8)
                .map(|band| {
                    let mut moved = r.clone();
                    moved.hsl[band][channel.index()] = 1.;
                    let mixer = super::super::color_mixer::ColorMixer::new(&moved).unwrap();
                    let out = mixer.apply(pro_to_rgb(skin));
                    let rgb = pro_to_rgb(skin);
                    (0..3).map(|c| (out[c] - rgb[c]).abs()).sum()
                })
                .collect();
            let most = (0..8)
                .max_by(|a, b| change[*a].total_cmp(&change[*b]))
                .unwrap();
            assert_eq!(most, 1, "{channel:?} {change:?}");
            for band in 0..8 {
                if change[band] < change[1] * 0.02 {
                    assert_eq!(w.shares[band], 0., "{channel:?} band {band} {change:?}");
                }
            }
        }
        // A neutral gray: no band changes it much, and none moves.
        let gray = TargetSample {
            mixer: [0.3; 3],
            ..sample([0.6, 0.001, 0.])
        };
        let w = TargetWeights::new(Target::Hsl(HslChannel::Saturation), &gray, &r);
        assert!(w.is_empty(), "{:?}", w.shares);
        // Neutrality is judged where the mixer looks: a skin tone whose finished color
        // an earlier Saturation −100 grayed is still adjustable.
        let grayed = TargetSample {
            mixer: skin,
            ..sample([0.6, 0.001, 0.])
        };
        let w = TargetWeights::new(Target::Hsl(HslChannel::Saturation), &grayed, &r);
        assert_eq!(w.shares[1], 1., "{:?}", w.shares);
    }
}
