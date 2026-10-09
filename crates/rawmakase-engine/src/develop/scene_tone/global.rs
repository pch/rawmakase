//! The scene tone stage's global curves, measured in Camera Raw 18.7 on neutral ramps
//! (`scripts/corpus/scene-tone-tables.py`): the white point with Whites, then Blacks.
//!
//! - `white3.bin`: output per sensor white (log2, −3 to 4 in half stops), stops the
//!   photo's maximum is below it (0 to 5 in half stops) and Whites (−100 to 100 in steps
//!   of 12.5), at log2(x / maximum) −14 to 0 in eighth stops; 16-bit, 0–65535 for 0–1.
//!   Positive Whites stretches toward the photo's maximum, so the curve needs both.
//! - `white.bin`: output per white point W* (log2, −1 to 5 in quarter stops) and Whites,
//!   at log2 scene values −16 to 6 in eighth stops, f32. Here only Whites 0: the shoulder
//!   above a photo's maximum (specks brighter than it), and the toe at W* = 1.
//! - `blacks.bin`: the default render's output to the Blacks render's, per black key
//!   (log2 of the photo's darkest level: −8, then −6.5 to −2 in half stops) and Blacks,
//!   at log2 outputs −16 to 0 in eighth stops, f32.
//!
//! - `masks.bin`: a mask's Whites, then its Blacks: the global stage's output to the
//!   output with the mask, at −1, −0.75, −0.5, −0.25, 0.25, 0.5, 0.75 and 1, over log2
//!   outputs −16 to 0 in eighth stops, f32. Camera Raw applies them after the global
//!   curves, the same at every white point.
//!
//! All include Camera Raw's black point (its toe). Values between the measured ones are
//! interpolated linearly.

static WHITE3: &[u8] = include_bytes!("white3.bin");
static WHITE: &[u8] = include_bytes!("white.bin");
static BLACKS: &[u8] = include_bytes!("blacks.bin");
static MASKS: &[u8] = include_bytes!("masks.bin");
/// The masks' slider positions in `masks.bin`; 0 is the identity.
pub(crate) const MASK_VALUES: [f32; 8] = [-1., -0.75, -0.5, -0.25, 0.25, 0.5, 0.75, 1.];

/// log2 sensor whites of `white3.bin` and the stops below them.
const SENSOR_FIRST: f32 = -3.;
const SENSOR_STEP: f32 = 0.5;
const SENSORS: usize = 15;
const BELOW_STEP: f32 = 0.5;
const BELOWS: usize = 11;
/// log2(x / maximum) of `white3.bin`'s samples.
pub(crate) const U_FIRST: f32 = -14.;
pub(crate) const U_STEP: f32 = 0.125;
pub(crate) const U_SAMPLES: usize = 113;
/// log2 W* of `white.bin`'s first white point and the step between them.
const WHITE_FIRST: f32 = -1.;
const WHITE_STEP: f32 = 0.25;
const WHITE_POINTS: usize = 25;
/// Whites and Blacks, −1 to 1 in steps of 0.125.
pub(crate) const SLIDER_STEP: f32 = 0.125;
pub(crate) const SLIDERS: usize = 17;
/// log2 scene values of `white.bin`'s samples.
pub(crate) const T_FIRST: f32 = -16.;
pub(crate) const T_STEP: f32 = 0.125;
pub(crate) const T_SAMPLES: usize = 177;
pub(crate) const BLACK_KEYS: [f32; 11] =
    [-8., -6.5, -6., -5.5, -5., -4.5, -4., -3.5, -3., -2.5, -2.];
/// log2 outputs of the blacks table's samples.
pub(crate) const Y_FIRST: f32 = -16.;
pub(crate) const Y_STEP: f32 = 0.125;
pub(crate) const Y_SAMPLES: usize = 129;

fn value(data: &[u8], i: usize) -> f32 {
    f32::from_le_bytes([
        data[4 * i],
        data[4 * i + 1],
        data[4 * i + 2],
        data[4 * i + 3],
    ])
}
fn value16(data: &[u8], i: usize) -> f32 {
    u16::from_le_bytes([data[2 * i], data[2 * i + 1]]) as f32 / 65535.
}
/// The two measured positions around `v` on a grid from `first` in `step`s of `n`
/// positions (clamped to it), and the weight of the upper one.
fn bracket(v: f32, first: f32, step: f32, n: usize) -> (usize, f32) {
    let f = ((v - first) / step).clamp(0., (n - 1) as f32);
    let i = (f as usize).min(n - 2);
    (i, f - i as f32)
}
fn bracket_in(values: &[f32], v: f32) -> (usize, f32) {
    let v = v.clamp(values[0], values[values.len() - 1]);
    let i = values
        .windows(2)
        .position(|w| v <= w[1])
        .unwrap_or(values.len() - 2);
    (i, (v - values[i]) / (values[i + 1] - values[i]))
}
/// A curve sampled at log2 inputs `first`, `first + step`, …: linear below the first
/// sample, as the stage is in deep shadows, and constant above the last.
pub(crate) fn evaluate(samples: &[f32], first: f32, step: f32, x: f32) -> f32 {
    evaluate_with(first, step, samples.len(), x, |k| samples[k])
}
/// [`evaluate`] with the samples read on demand.
fn evaluate_with(first: f32, step: f32, n: usize, x: f32, at: impl Fn(usize) -> f32) -> f32 {
    let low = first.exp2();
    if x <= low {
        return x * at(0) / low;
    }
    let f = (x.log2() - first) / step;
    if f >= (n - 1) as f32 {
        return at(n - 1);
    }
    let i = f as usize;
    let t = f - i as f32;
    at(i) * (1. - t) + at(i + 1) * t
}

/// The global part of a render's scene tone stage: the white point and Whites as one
/// curve applied DNG RGBTone fashion (the brightest and darkest channel curved, the
/// middle one keeping its place), then Blacks on each channel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GlobalTone {
    /// log2 of the photo's maximum (at the render's Exposure).
    top: f32,
    black_key: f32,
    black_point: bool,
    whites: f32,
    blacks: f32,
    /// The white curves of this photo's sensor white and maximum, one per Whites
    /// position, over log2(x / maximum).
    rows: Box<[[f32; U_SAMPLES]; SLIDERS]>,
    /// The curve at the recipe's Whites.
    white: [f32; U_SAMPLES],
    /// Above the maximum: the default curve at the photo's white point, over log2 x.
    tail: Box<[f32; T_SAMPLES]>,
    /// Without a black point: the toe to divide out (the default curve at W* = 1).
    toe: Option<Box<[f32; T_SAMPLES]>>,
    blacks_curve: Option<Box<[f32; Y_SAMPLES]>>,
}
impl GlobalTone {
    /// For the sensor's white, the photo's maximum and its white point (log2, at the
    /// render's Exposure), Whites and Blacks (−1 to 1), the photo's black key (log2 of its
    /// darkest level) and whether the profile has Camera Raw's default black point (its
    /// DefaultBlackRender is Auto).
    pub(crate) fn new(
        [sensor, top, white_point]: [f32; 3],
        whites: f32,
        blacks: f32,
        black_key: f32,
        black_point: bool,
    ) -> Self {
        let (i, ti) = bracket(sensor, SENSOR_FIRST, SENSOR_STEP, SENSORS);
        let (j, tj) = bracket(sensor - top, 0., BELOW_STEP, BELOWS);
        let at = |i: usize, j: usize, s: usize, k: usize| {
            value16(WHITE3, ((i * BELOWS + j) * SLIDERS + s) * U_SAMPLES + k)
        };
        let rows: Box<[[f32; U_SAMPLES]; SLIDERS]> = Box::new(std::array::from_fn(|s| {
            std::array::from_fn(|k| {
                (at(i, j, s, k) * (1. - tj) + at(i, j + 1, s, k) * tj) * (1. - ti)
                    + (at(i + 1, j, s, k) * (1. - tj) + at(i + 1, j + 1, s, k) * tj) * ti
            })
        }));
        let row = |w: usize, k: usize| value(WHITE, (w * SLIDERS + SLIDERS / 2) * T_SAMPLES + k);
        let (w, tw) = bracket(white_point, WHITE_FIRST, WHITE_STEP, WHITE_POINTS);
        let tail = Box::new(std::array::from_fn(|k| {
            row(w, k) * (1. - tw) + row(w + 1, k) * tw
        }));
        let one = ((0. - WHITE_FIRST) / WHITE_STEP) as usize;
        let toe = (!black_point).then(|| Box::new(std::array::from_fn(|k| row(one, k))));
        let blacks_curve = (blacks != 0.).then(|| {
            let corners = blacks_corners(black_key, blacks);
            Box::new(std::array::from_fn(|k| blacks_sample(&corners, k)))
        });
        let mut tone = Self {
            top,
            black_key,
            black_point,
            whites,
            blacks,
            rows,
            white: [0.; U_SAMPLES],
            tail,
            toe,
            blacks_curve,
        };
        tone.white = tone.row_at(whites);
        tone
    }
    /// The white curve at Whites `whites`, between the two rows around it.
    fn row_at(&self, whites: f32) -> [f32; U_SAMPLES] {
        let (s, t) = bracket(whites, -1., SLIDER_STEP, SLIDERS);
        std::array::from_fn(|k| self.rows[s][k] * (1. - t) + self.rows[s + 1][k] * t)
    }
    /// The white curve `curve` at scene value `x`.
    fn white_at(&self, curve: &[f32; U_SAMPLES], x: f32) -> f32 {
        let top = self.top.exp2();
        let y = if x <= top {
            evaluate(curve, self.top + U_FIRST, U_STEP, x)
        } else {
            // Specks brighter than the maximum roll on to white with the default curve.
            let at_top = curve[U_SAMPLES - 1];
            let r = evaluate(&self.tail[..], T_FIRST, T_STEP, x);
            let rm = evaluate(&self.tail[..], T_FIRST, T_STEP, top);
            if rm < 1. {
                at_top + (1. - at_top) * ((r - rm) / (1. - rm)).clamp(0., 1.)
            } else {
                at_top
            }
        };
        match &self.toe {
            Some(toe) if x > 0. => y * x.min(1.) / evaluate(&toe[..], T_FIRST, T_STEP, x).max(1e-9),
            _ => y,
        }
    }
    pub(crate) fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let p = rgb_tone(p, |x| self.white_at(&self.white, x));
        match &self.blacks_curve {
            Some(blacks) => p.map(|v| evaluate(&blacks[..], Y_FIRST, Y_STEP, v)),
            None => p,
        }
    }
    /// As [`Self::apply`], then a mask's Whites and Blacks (−1 to 1) on the result, as
    /// their own curves.
    pub(crate) fn apply_at(&self, p: [f32; 3], whites: f32, blacks: f32) -> [f32; 3] {
        let p = self.apply(p);
        let p = if whites != 0. {
            rgb_tone(p, |y| mask_curve(0, whites, y))
        } else {
            p
        };
        if blacks != 0. {
            p.map(|y| mask_curve(1, blacks, y))
        } else {
            p
        }
    }
    /// What the GPU reads with the curves: the photo's maximum, the black key, whether
    /// there is a black point, and the recipe's Whites and Blacks.
    pub(crate) fn gpu_keys(&self) -> [f32; 5] {
        [
            self.top,
            self.black_key,
            self.black_point as u8 as f32,
            self.whites,
            self.blacks,
        ]
    }
    /// The curves for the GPU port: the white rows, then the tail and the toe (the tail
    /// again with a black point, unused).
    pub(crate) fn gpu_curves(&self) -> Vec<f32> {
        let mut out: Vec<f32> = self.rows.iter().flatten().copied().collect();
        out.extend(self.tail.iter());
        out.extend(self.toe.as_deref().unwrap_or(&self.tail).iter());
        out
    }
    /// The blacks curve's samples, for the GPU port.
    pub(crate) fn blacks_samples(&self) -> Option<&[f32]> {
        self.blacks_curve.as_deref().map(|b| &b[..])
    }
}
/// A mask's Whites (`kind` 0) or Blacks (1) at slider `s` applied to output `y`:
/// between the measured positions, with the identity at 0.
fn mask_curve(kind: usize, s: f32, y: f32) -> f32 {
    if s == 0. {
        return y;
    }
    let s = s.clamp(-1., 1.);
    // Slots: the measured positions with the identity at 0 between them.
    const IDENTITY: usize = 4;
    let position = |slot: usize| match slot {
        IDENTITY => 0.,
        _ => MASK_VALUES[if slot < IDENTITY { slot } else { slot - 1 }],
    };
    let output = |slot: usize| match slot {
        IDENTITY => y,
        _ => {
            let row = if slot < IDENTITY { slot } else { slot - 1 };
            evaluate_with(Y_FIRST, Y_STEP, Y_SAMPLES, y, |k| {
                value(MASKS, (kind * MASK_VALUES.len() + row) * Y_SAMPLES + k)
            })
        }
    };
    let j = (0..MASK_VALUES.len())
        .position(|k| s <= position(k + 1))
        .unwrap_or(MASK_VALUES.len() - 1);
    let t = (s - position(j)) / (position(j + 1) - position(j));
    output(j) * (1. - t) + output(j + 1) * t
}
/// The masks' curves as values, for the GPU port.
pub(crate) fn gpu_mask_table() -> Vec<f32> {
    (0..MASKS.len() / 4).map(|i| value(MASKS, i)).collect()
}
/// The four measured blacks curves around a black key and Blacks, and their weights.
fn blacks_corners(black_key: f32, blacks: f32) -> [(usize, f32); 4] {
    let (b, tb) = bracket_in(&BLACK_KEYS, black_key);
    let (s, ts) = bracket(blacks, -1., SLIDER_STEP, SLIDERS);
    [
        (b * SLIDERS + s, (1. - tb) * (1. - ts)),
        (b * SLIDERS + s + 1, (1. - tb) * ts),
        ((b + 1) * SLIDERS + s, tb * (1. - ts)),
        ((b + 1) * SLIDERS + s + 1, tb * ts),
    ]
}
fn blacks_sample(corners: &[(usize, f32); 4], k: usize) -> f32 {
    corners
        .iter()
        .map(|(r, w)| value(BLACKS, r * Y_SAMPLES + k) * w)
        .sum()
}
/// DNG RGBTone: `curve` maps the brightest and darkest channel, and the middle one
/// keeps its relative place between them.
fn rgb_tone(p: [f32; 3], curve: impl Fn(f32) -> f32) -> [f32; 3] {
    let lo = p.into_iter().fold(f32::INFINITY, f32::min);
    let hi = p.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let (a, b) = (curve(lo), curve(hi));
    if hi - lo > 1e-12 {
        p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
    } else {
        [a; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A photo whose maximum is its sensor's white at 2^`sensor`.
    fn reaching(sensor: f32, whites: f32, blacks: f32, black_point: bool) -> GlobalTone {
        GlobalTone::new([sensor, sensor, sensor], whites, blacks, -8., black_point)
    }

    #[test]
    fn tables_have_their_documented_size() {
        assert_eq!(WHITE3.len(), 2 * SENSORS * BELOWS * SLIDERS * U_SAMPLES);
        assert_eq!(WHITE.len(), 4 * WHITE_POINTS * SLIDERS * T_SAMPLES);
        assert_eq!(BLACKS.len(), 4 * BLACK_KEYS.len() * SLIDERS * Y_SAMPLES);
    }

    #[test]
    fn a_white_point_of_one_leaves_scene_values_alone_but_for_the_toe() {
        // Camera Raw's measured outputs, the toe of its black point included.
        let tone = reaching(0., 0., 0., true);
        for (x, want) in [
            (0.01f32, 0.009),
            (0.05, 0.048),
            (0.18, 0.177),
            (0.5, 0.499),
            (1., 1.),
        ] {
            let y = tone.apply([x; 3])[0];
            assert!((y - want).abs() < 0.003, "{x}: {y}, Camera Raw {want}");
        }
    }

    #[test]
    fn a_higher_white_point_rolls_highlights_off_to_it() {
        // Measured: W* = 4 renders scene 1 at 0.693 and 4 at 1.
        let tone = reaching(2., 0., 0., true);
        assert!((tone.apply([1.; 3])[0] - 0.693).abs() < 0.005);
        assert!((tone.apply([4.; 3])[0] - 1.).abs() < 2e-3);
        assert!((tone.apply([0.18; 3])[0] - 0.177).abs() < 0.003);
    }

    #[test]
    fn positive_whites_stretches_a_dim_photo_toward_white() {
        // A photo four stops below its sensor's white: Whites +100 brightens it far more
        // than a photo reaching its white point.
        let dim = GlobalTone::new([2., -2., 0.], 1., 0., -8., true);
        let bright = reaching(2., 1., 0., true);
        let x = 0.2;
        assert!(dim.apply([x; 3])[0] > bright.apply([x; 3])[0] + 0.2);
    }

    #[test]
    fn curves_are_monotone() {
        for (sensor, top) in [(-1., -1.), (0., -2.), (1.3, 1.3), (2., -1.), (3.7, 1.)] {
            for whites in [-1., -0.3, 0., 0.6, 1.] {
                for blacks in [-1., 0., 0.5, 1.] {
                    let tone = GlobalTone::new([sensor, top, top + 1.], whites, blacks, -5., true);
                    let mut last = -1f32;
                    for i in 0..400 {
                        let x = 2f32.powf(-14. + i as f32 * 0.05);
                        let y = tone.apply([x; 3])[0];
                        assert!(
                            y >= last - 2e-4,
                            "{sensor} {top} {whites} {blacks}: {x} -> {y} < {last}"
                        );
                        last = y;
                    }
                }
            }
        }
    }

    #[test]
    fn without_the_black_point_deep_shadows_stay_linear() {
        let without = reaching(0., 0., 0., false);
        for x in [0.0005f32, 0.004, 0.02, 0.18] {
            let y = without.apply([x; 3])[0];
            assert!((y / x - 1.).abs() < 0.025, "{x}: {y}");
        }
        let with = reaching(2., 0., 0., true);
        let without = reaching(2., 0., 0., false);
        for x in [0.001f32, 0.004, 0.02] {
            let y = without.apply([x; 3])[0];
            assert!(y > with.apply([x; 3])[0] && y > 0.75 * x, "{x}: {y}");
        }
        assert!((with.apply([1.; 3])[0] - without.apply([1.; 3])[0]).abs() < 2e-3);
    }

    #[test]
    fn a_masks_whites_and_blacks_follow_the_global_curves() {
        let render = GlobalTone::new([1.3, 0.2, 1.2], 0.2, -0.1, -5., true);
        let p = [0.4f32, 0.3, 0.6];
        assert_eq!(render.apply_at(p, 0., 0.), render.apply(p));
        // Measured: a mask's Whites +50 takes 0.18 (default output 0.177) to 0.240.
        let plain = reaching(0., 0., 0., true);
        let lifted = plain.apply_at([0.18; 3], 0.5, 0.)[0];
        assert!((lifted - 0.240).abs() < 0.005, "{lifted}");
        // Continuous through 0.
        let small = plain.apply_at([0.18; 3], 0.01, 0.)[0];
        assert!((small - plain.apply([0.18; 3])[0]).abs() < 0.005);
        let dark = plain.apply_at([0.05; 3], 0., -0.5)[0];
        assert!(dark < plain.apply([0.05; 3])[0]);
    }

    #[test]
    fn the_middle_channel_keeps_its_place() {
        let tone = reaching(2., 0.5, 0., true);
        let [r, g, b] = tone.apply([1.2, 0.6, 0.3]);
        assert!(r > g && g > b);
        assert!(((g - b) / (r - b) - (0.6 - 0.3) / (1.2 - 0.3)).abs() < 1e-5);
    }
}
