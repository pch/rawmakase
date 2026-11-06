//! The scene tone stage's global curves, measured in Camera Raw 18.7 on neutral ramps
//! (`scripts/corpus/scene-tone-tables.py`): the white point with Whites, then Blacks.
//!
//! - `white3.bin`: output per sensor white (log2, −3 to 4 in half stops), stops the
//!   photo's maximum is below it (0 to 5 in half stops) and Whites (−100 to 100 in steps
//!   of 12.5), at log2(x / maximum) −14 to 0 in eighth stops; 16-bit, 0–65535 for 0–1.
//!   Positive Whites stretches toward the photo's maximum, so the curve needs both; on
//!   photos that level is `PhotoMeasures::whites_top`.
//! - `white.bin`: output per white point W* (log2, −1 to 5 in quarter stops) and Whites,
//!   at log2 scene values −16 to 6 in eighth stops, f32. Here only Whites 0: the shoulder
//!   above a photo's maximum (specks brighter than it), and the toe at W* = 1.
//! - `blacks.bin`: the default render's output to the Blacks render's, per black key
//!   (log2 of the photo's darkest level: −8, then −6.5 to −2 in half stops) and Blacks,
//!   at log2 outputs −16 to 0 in eighth stops, f32.
//! - `default_black.bin`: measured DefaultBlackRender None → Auto transfers,
//!   per absolute scene dark key (−14 to −7 in half stops), at log2 outputs
//!   −16 to 0 in eighth stops, f32. The key includes Exposure and camera baseline
//!   exposure. The response stops changing above −7; lower tails continue with
//!   the first measured sample's ratio. The white tables already include their
//!   calibration's black response (darkest patch 14 stops below their maximum).
//!   `black_correction` replaces it with the photo's response, rather than adding
//!   a second black curve. CPU and GPU use the same composed samples.
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
static DEFAULT_BLACK: &[u8] = include_bytes!("default_black.bin");
/// Absolute scene log luminance of the darkest region, including Exposure.
/// The measured response stops changing above −7 stops.
const DEFAULT_BLACK_KEYS: [f32; 15] = [
    -14., -13.5, -13., -12.5, -12., -11.5, -11., -10.5, -10., -9.5, -9., -8.5, -8., -7.5, -7.,
];
/// The masks' slider positions in `masks.bin`; 0 is the identity.
/// The toe's output above which, without a black point, the white curves resolve it.
const LINEAR_TOE: f32 = 1. / 512.;
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
    /// log2 of the level Whites stretches toward (at the render's Exposure), within the
    /// measured range of sensor whites and maxima.
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
    /// Without a black point: the scene value below which the curve continues linearly,
    /// where the 16-bit white curves no longer resolve the toe.
    linear_below: f32,
    /// Replace the black response already in the white calibration with the photo's
    /// response, over log2 white-curve outputs. Absent for DefaultBlackRender None.
    default_black: Option<Box<[f32; Y_SAMPLES]>>,
    blacks_curve: Option<Box<[f32; Y_SAMPLES]>>,
    /// Default-black curves at positive Shadows 0..1 in steps of 0.1, for masks.
    shadow_black: Option<(f32, Vec<f32>)>,
    /// An HDR render's ([`Self::hdr`]): its Whites as log2 gains over log2 scene value.
    hdr: Option<Box<[f32; super::hdr_whites::SAMPLES]>>,
}
impl GlobalTone {
    /// For the sensor's white, the level Whites stretches toward (the probes' maximum;
    /// `PhotoMeasures::whites_top`) and the white point (log2, at the render's
    /// Exposure), Whites and Blacks (−1 to 1), the photo's black key (log2 of its darkest
    /// level, for Blacks), the level the default black follows (log2 of its 0.1th
    /// percentile; `PhotoMeasures::dark_key`) and whether the profile has Camera Raw's default black point (its
    /// DefaultBlackRender is Auto).
    pub(crate) fn new(
        [sensor, top, stretch, white_point]: [f32; 4],
        whites: f32,
        blacks: f32,
        black_key: f32,
        dark_key: f32,
        black_point: bool,
    ) -> Self {
        // Outside the measured sensor whites and maxima the nearest curves apply to the
        // scene values as they are: the curves are over x relative to the maximum, so it
        // is clamped with them, and Exposure keeps its effect.
        let last = SENSOR_FIRST + (SENSORS - 1) as f32 * SENSOR_STEP;
        let sensor = sensor.clamp(SENSOR_FIRST, last);
        let within = |t: f32| sensor - (sensor - t).clamp(0., (BELOWS - 1) as f32 * BELOW_STEP);
        let (top, stretch) = (within(top), within(stretch));
        let row = |w: usize, k: usize| value(WHITE, (w * SLIDERS + SLIDERS / 2) * T_SAMPLES + k);
        let (w, tw) = bracket(white_point, WHITE_FIRST, WHITE_STEP, WHITE_POINTS);
        let tail: Box<[f32; T_SAMPLES]> = Box::new(std::array::from_fn(|k| {
            row(w, k) * (1. - tw) + row(w + 1, k) * tw
        }));
        let tail_at = |log: f32| evaluate(&tail[..], T_FIRST, T_STEP, log.exp2());
        // The measured curves around a sensor white and maximum `max` at Whites row `s`,
        // read at log2 scene value `log` each (relative to its own maximum) and blended.
        // Above a curve's maximum it rolls on to white with the default curve, as
        // `white_at` does above the photo's.
        let curve = |max: f32, s: usize, log: f32| {
            let (i, ti) = bracket(sensor, SENSOR_FIRST, SENSOR_STEP, SENSORS);
            let (j, tj) = bracket(sensor - max, 0., BELOW_STEP, BELOWS);
            let at = |i: usize, j: usize| {
                let corner = SENSOR_FIRST + i as f32 * SENSOR_STEP - j as f32 * BELOW_STEP;
                let base = ((i * BELOWS + j) * SLIDERS + s) * U_SAMPLES;
                let f = (log - corner - U_FIRST) / U_STEP;
                if f > (U_SAMPLES - 1) as f32 {
                    let at_top = value16(WHITE3, base + U_SAMPLES - 1);
                    let (r, rm) = (tail_at(log), tail_at(corner));
                    return if rm < 1. {
                        at_top + (1. - at_top) * ((r - rm) / (1. - rm)).clamp(0., 1.)
                    } else {
                        at_top
                    };
                }
                let f = f.max(0.);
                let n = (f as usize).min(U_SAMPLES - 2);
                let t = f - n as f32;
                value16(WHITE3, base + n) * (1. - t) + value16(WHITE3, base + n + 1) * t
            };
            (at(i, j) * (1. - tj) + at(i, j + 1) * tj) * (1. - ti)
                + (at(i + 1, j) * (1. - tj) + at(i + 1, j + 1) * tj) * ti
        };
        // The default and negative Whites follow the photo's maximum; positive Whites
        // adds the stretch toward `stretch` (twice its 99th percentile on photos), so the
        // rows meet at Whites 0.
        let one = ((0. - WHITE_FIRST) / WHITE_STEP) as usize;
        let toe = (!black_point).then(|| Box::new(std::array::from_fn(|k| row(one, k))));
        let linear_below = toe.as_ref().map_or(0., |toe| {
            let k = toe
                .iter()
                .position(|y| *y >= LINEAR_TOE)
                .unwrap_or(T_SAMPLES - 1);
            (T_FIRST + k as f32 * T_STEP).exp2()
        });
        let zero = SLIDERS / 2;
        let rows: Box<[[f32; U_SAMPLES]; SLIDERS]> = Box::new(std::array::from_fn(|s| {
            let mut row: [f32; U_SAMPLES] = std::array::from_fn(|k| {
                let log = top + U_FIRST + k as f32 * U_STEP;
                let default = curve(top, s.min(zero), log);
                if s <= zero {
                    default
                } else {
                    default + curve(stretch, s, log) - curve(stretch, zero, log)
                }
            });
            // Without a black point, Camera Raw's toe is divided out of the curve.
            if let Some(toe) = &toe {
                for (k, v) in row.iter_mut().enumerate() {
                    let x = (top + U_FIRST + k as f32 * U_STEP).exp2();
                    *v *= x.min(1.) / evaluate(&toe[..], T_FIRST, T_STEP, x).max(1e-9);
                }
            }
            // The stretch's difference, added to the default curve, can dip where the
            // stretch level is below the photo's maximum, and so can the toe's removal;
            // the curve never decreases.
            for k in 1..U_SAMPLES {
                row[k] = row[k].max(row[k - 1]);
            }
            row
        }));
        // The white probes' darkest patch is 14 stops below their maximum.
        // Their black response is already present; adding the photo's response
        // directly would apply black rendering twice.
        let default_black = black_point
            .then(|| black_correction(dark_key, top - 14.))
            .filter(|curve| {
                // An output LUT interpolated on a log input grid does not render
                // the identity exactly between knots. Bypass a true no-op.
                curve
                    .iter()
                    .enumerate()
                    .any(|(k, value)| *value != (Y_FIRST + k as f32 * Y_STEP).exp2())
            });
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
            linear_below,
            default_black,
            blacks_curve,
            shadow_black: None,
            hdr: None,
        };
        tone.white = tone.row_at(whites);
        tone
    }
    /// The global curves of an HDR render, as Camera Raw 18.7 renders them in HDR:
    /// the default curve is the identity from [`super::hdr_whites::IDENTITY_FROM`] up,
    /// with the default black point's toe below it (the default curve at a white point
    /// of 1, which is that toe), whatever the photo's sensor white and maximum. Whites
    /// is a measured gain over scene value that follows neither of them nor the HDR
    /// limit. Blacks and the default black follow the photo as in SDR: below 0.25
    /// Camera Raw's HDR and SDR renders of Blacks ±100 agree to 0.001 stop.
    pub(crate) fn hdr(
        whites: f32,
        blacks: f32,
        black_key: f32,
        dark_key: f32,
        black_point: bool,
    ) -> Self {
        let mut tone = Self::new([0.; 4], 0., blacks, black_key, dark_key, black_point);
        tone.whites = whites;
        tone.hdr = Some(Box::new(super::hdr_whites::gains(whites)));
        tone
    }
    /// The white curve at Whites `whites`, between the two rows around it.
    fn row_at(&self, whites: f32) -> [f32; U_SAMPLES] {
        let (s, t) = bracket(whites, -1., SLIDER_STEP, SLIDERS);
        std::array::from_fn(|k| self.rows[s][k] * (1. - t) + self.rows[s + 1][k] * t)
    }
    /// The white curve at the recipe's Whites on one value, as [`Self::apply`] reads
    /// it for each channel it curves.
    pub(crate) fn white_curve(&self, x: f32) -> f32 {
        self.white_at(&self.white, x)
    }
    /// The white curve `curve` at scene value `x`.
    fn white_at(&self, curve: &[f32; U_SAMPLES], x: f32) -> f32 {
        if let Some(gains) = &self.hdr {
            let toe = if x >= super::hdr_whites::IDENTITY_FROM {
                x
            } else {
                self.sdr_white_at(curve, x)
            };
            return toe * super::hdr_whites::gain(gains, x).exp2();
        }
        self.sdr_white_at(curve, x)
    }
    fn sdr_white_at(&self, curve: &[f32; U_SAMPLES], x: f32) -> f32 {
        let top = self.top.exp2();
        let y = if x <= top {
            evaluate(curve, self.top + U_FIRST, U_STEP, x)
        } else {
            // Specks brighter than the maximum roll on to white with the default curve;
            // a curve Whites has already stretched past white stays there.
            let at_top = curve[U_SAMPLES - 1];
            let r = evaluate(&self.tail[..], T_FIRST, T_STEP, x);
            let rm = evaluate(&self.tail[..], T_FIRST, T_STEP, top);
            if rm < 1. && at_top < 1. {
                at_top + (1. - at_top) * ((r - rm) / (1. - rm)).clamp(0., 1.)
            } else {
                at_top
            }
        };
        // The rows carry the toe's removal (`new`); below `linear_below` the curve
        // without a black point is linear.
        match &self.toe {
            Some(_) if x > 0. && x < self.linear_below => {
                self.sdr_white_at(curve, self.linear_below) * x / self.linear_below
            }
            _ => y,
        }
    }
    pub(crate) fn set_shadow_black(&mut self, shadows: f32, dark_keys: &[f32; 11]) {
        if self.black_point {
            self.shadow_black = Some((
                shadows,
                dark_keys
                    .iter()
                    .flat_map(|key| black_correction(*key, self.top - 14.).to_vec())
                    .collect(),
            ));
        }
    }
    pub(crate) fn shadow_black_samples(&self) -> Option<(f32, &[f32])> {
        self.shadow_black
            .as_ref()
            .map(|(s, rows)| (*s, rows.as_slice()))
    }
    pub(crate) fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        self.apply_with_shadow(p, 0.)
    }
    fn apply_with_shadow(&self, p: [f32; 3], shadow_delta: f32) -> [f32; 3] {
        let p = rgb_tone(p, |x| self.white_at(&self.white, x));
        let p = if let Some((shadows, rows)) = &self.shadow_black
            && shadow_delta != 0.
        {
            let f = (shadows + shadow_delta).clamp(0., 1.) * 10.;
            let i = (f as usize).min(9);
            let t = f - i as f32;
            p.map(|v| {
                if v < 1. {
                    let a = evaluate(
                        &rows[i * Y_SAMPLES..(i + 1) * Y_SAMPLES],
                        Y_FIRST,
                        Y_STEP,
                        v,
                    );
                    let b = evaluate(
                        &rows[(i + 1) * Y_SAMPLES..(i + 2) * Y_SAMPLES],
                        Y_FIRST,
                        Y_STEP,
                        v,
                    );
                    a + (b - a) * t
                } else {
                    v
                }
            })
        } else {
            match &self.default_black {
                // Above white (positive Whites' overshoot) it leaves the output alone.
                Some(curve) => p.map(|v| {
                    if v < 1. {
                        evaluate(&curve[..], Y_FIRST, Y_STEP, v)
                    } else {
                        v
                    }
                }),
                None => p,
            }
        };
        match &self.blacks_curve {
            // In HDR, Blacks leaves values above white alone, as Camera Raw's HDR
            // renders of Blacks −100 keep the default's peak.
            Some(blacks) if self.hdr.is_some() => p.map(|v| {
                if v < 1. {
                    evaluate(&blacks[..], Y_FIRST, Y_STEP, v)
                } else {
                    v
                }
            }),
            Some(blacks) => p.map(|v| evaluate(&blacks[..], Y_FIRST, Y_STEP, v)),
            None => p,
        }
    }
    /// As [`Self::apply`], then a mask's Whites and Blacks (−1 to 1) on the result, as
    /// their own curves.
    pub(crate) fn apply_at(&self, p: [f32; 3], whites: f32, blacks: f32, shadows: f32) -> [f32; 3] {
        let p = self.apply_with_shadow(p, shadows);
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
    /// there is a black point, the recipe's Whites and Blacks, and where the curve
    /// continues linearly without a black point.
    pub(crate) fn gpu_keys(&self) -> [f32; 6] {
        [
            self.top,
            self.black_key,
            self.black_point as u8 as f32,
            self.whites,
            self.blacks,
            self.linear_below,
        ]
    }
    /// The curves for the GPU port: the white rows (which carry the toe's removal
    /// without a black point), then the tail.
    pub(crate) fn gpu_curves(&self) -> Vec<f32> {
        let mut out: Vec<f32> = self.rows.iter().flatten().copied().collect();
        out.extend(self.tail.iter());
        out
    }
    /// The default black's samples, for the GPU port.
    pub(crate) fn default_black_samples(&self) -> Option<&[f32]> {
        self.default_black.as_deref().map(|b| &b[..])
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
/// Camera Raw's None → Auto transfer at an absolute scene dark key,
/// interpolated between measured keys and clamped outside their range.
fn default_black(key: f32, k: usize) -> f32 {
    let (b, t) = bracket_in(&DEFAULT_BLACK_KEYS, key);
    default_black_row(b, k) * (1. - t) + default_black_row(b + 1, k) * t
}
/// Measured DefaultBlackRender None → Auto transfer. The generator fills each
/// low tail from its first measured sample; zero is an output, not missing data.
fn default_black_row(b: usize, k: usize) -> f32 {
    value(DEFAULT_BLACK, b * Y_SAMPLES + k)
}

/// Compose the desired response with the inverse of the calibration's response.
/// Both run after the white curve. Subtracting the sampled calibration output
/// preserves the identity exactly when the two keys match.
fn black_correction(key: f32, calibration_key: f32) -> Box<[f32; Y_SAMPLES]> {
    let calibration: [f32; Y_SAMPLES] = std::array::from_fn(|k| default_black(calibration_key, k));
    let desired: [f32; Y_SAMPLES] = std::array::from_fn(|k| default_black(key, k));
    Box::new(std::array::from_fn(|k| {
        let y = (Y_FIRST + k as f32 * Y_STEP).exp2();
        let input = if y < calibration[0] {
            y / calibration[0].max(1e-12) * Y_FIRST.exp2()
        } else {
            let j = calibration
                .partition_point(|v| *v < y)
                .clamp(1, Y_SAMPLES - 1);
            let t = ((y - calibration[j - 1]) / (calibration[j] - calibration[j - 1]).max(1e-12))
                .clamp(0., 1.);
            (Y_FIRST + (j as f32 - 1. + t) * Y_STEP).exp2()
        };
        let before = evaluate(&calibration, Y_FIRST, Y_STEP, input);
        let after = evaluate(&desired, Y_FIRST, Y_STEP, input);
        (y + (after - before)).max(0.)
    }))
}
/// DNG RGBTone: `curve` maps the brightest and darkest channel, and the middle one
/// keeps its relative place between them.
pub(crate) fn rgb_tone(p: [f32; 3], curve: impl Fn(f32) -> f32) -> [f32; 3] {
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
        GlobalTone::new(
            [sensor, sensor, sensor, sensor],
            whites,
            blacks,
            -8.,
            -8.,
            black_point,
        )
    }

    /// Beyond the measured sensor whites and maxima the curves keep the scene's scale:
    /// Exposure keeps darkening a photo far below its white, and a maximum far below the
    /// sensor's white does not brighten the photo.
    #[test]
    fn exposure_keeps_working_outside_the_measured_range() {
        let gray = |sensor: f32| reaching(sensor, 0., 0., true).apply([0.18 * sensor.exp2(); 3])[1];
        for sensor in [-2., -3., -4., -5., -6.] {
            assert!(gray(sensor - 1.) < gray(sensor) * 0.75, "{sensor}");
        }
        let dim = |top: f32| {
            GlobalTone::new([0., top, top, 0.], 0., 0., -8., -8., true).apply([2f32.powf(-7.); 3])
                [1]
        };
        assert!(
            (dim(-6.) / dim(-10.) - 1.).abs() < 0.1,
            "{} {}",
            dim(-6.),
            dim(-10.)
        );
    }

    /// Without a black point the darkest values stay linear, below the levels the 16-bit
    /// white curves resolve.
    #[test]
    fn without_a_black_point_the_darkest_values_stay_linear() {
        let tone = reaching(0., 0., 0., false);
        for x in [1e-4f32, 3e-5, 1e-5] {
            let (a, b) = (tone.apply([x; 3])[1], tone.apply([2. * x; 3])[1]);
            assert!(a > 0. && (b / a - 2.).abs() < 0.05, "{x}: {a} {b}");
        }
    }

    /// Between the measured sensor whites and maxima the curves blend at the same scene
    /// value: a mid gray renders as at every measured corner, whatever the photo's maximum.
    #[test]
    fn curves_between_measured_maxima_keep_mid_gray() {
        let gray = |sensor: f32, top: f32| {
            GlobalTone::new([sensor, top, top, sensor], 0., 0., -8., -8., true).apply([0.18; 3])[1]
        };
        let corner = gray(0.5, 0.5);
        for (sensor, top) in [(0.862, 0.726), (0.25, -0.6), (1.7, 0.2)] {
            let g = gray(sensor, top);
            assert!(
                (g / corner - 1.).abs() < 0.005,
                "{sensor} {top}: {g} against {corner}"
            );
        }
    }

    /// Between measured maxima, a measured curve continues past its own maximum with the
    /// default shoulder rather than stopping: no shoulder appears below the photo's
    /// maximum (found in review: 0.774 where the curve at W* = 1 gives 0.842).
    #[test]
    fn no_shoulder_below_the_photos_maximum() {
        let x = 0.840896;
        let between =
            GlobalTone::new([0., -0.25, -0.25, 0.], 0., 0., -8., -8., true).apply([x; 3])[1];
        let measured = reaching(0., 0., 0., true).apply([x; 3])[1];
        assert!(
            (between - measured).abs() < 0.02,
            "{between} against {measured}"
        );
    }

    /// The level positive Whites stretches toward leaves the default and negative Whites
    /// as the photo's maximum makes them, and positive Whites meets them at 0.
    #[test]
    fn the_stretch_level_moves_only_positive_whites() {
        let at = |stretch: f32, whites: f32, x: f32| {
            GlobalTone::new([2., 2., stretch, 2.], whites, 0., -8., -8., true).apply([x; 3])[1]
        };
        for whites in [0., -0.5, -1.] {
            for x in [0.1, 1., 2.] {
                assert_eq!(at(2., whites, x), at(-1., whites, x), "{whites} {x}");
            }
        }
        for x in [0.1, 1., 2.] {
            assert!((at(-1., 0.001, x) - at(-1., 0., x)).abs() < 0.005, "{x}");
        }
        // Below white, a lower level stretches further.
        for x in [0.05, 0.1, 0.2] {
            assert!(at(-1., 1., x) > at(2., 1., x), "{x}");
        }
    }

    #[test]
    fn tables_have_their_documented_size() {
        assert_eq!(WHITE3.len(), 2 * SENSORS * BELOWS * SLIDERS * U_SAMPLES);
        assert_eq!(WHITE.len(), 4 * WHITE_POINTS * SLIDERS * T_SAMPLES);
        assert_eq!(BLACKS.len(), 4 * BLACK_KEYS.len() * SLIDERS * Y_SAMPLES);
        assert_eq!(
            DEFAULT_BLACK.len(),
            4 * DEFAULT_BLACK_KEYS.len() * Y_SAMPLES
        );
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
        let dim = GlobalTone::new([2., -2., -2., 0.], 1., 0., -8., -8., true);
        let bright = reaching(2., 1., 0., true);
        let x = 0.2;
        assert!(dim.apply([x; 3])[0] > bright.apply([x; 3])[0] + 0.2);
    }

    #[test]
    fn curves_are_monotone() {
        let monotone = |tone: &GlobalTone, case: &str| {
            let mut last = -1f32;
            for i in 0..400 {
                let x = 2f32.powf(-14. + i as f32 * 0.05);
                let y = tone.apply([x; 3])[0];
                assert!(y >= last - 2e-4, "{case}: {x} -> {y} < {last}");
                last = y;
            }
        };
        for (sensor, top) in [(-1., -1.), (0., -2.), (1.3, 1.3), (2., -1.), (3.7, 1.)] {
            // Whites stretches toward the photo's maximum or, on photos, a level below it
            // (a dark photo with a small bright highlight).
            for stretch in [top, top - 1., top - 3.] {
                for whites in [-1., -0.3, 0., 0.6, 1.] {
                    for blacks in [-1., 0., 0.5, 1.] {
                        // With Camera Raw's black point, and without (DefaultBlackRender
                        // None), where its toe is divided out.
                        for black_point in [true, false] {
                            let tone = GlobalTone::new(
                                [sensor, top, stretch, top + 1.],
                                whites,
                                blacks,
                                -5.,
                                -5.,
                                black_point,
                            );
                            monotone(
                                &tone,
                                &format!(
                                    "{sensor} {top} {stretch} {whites} {blacks} {black_point}"
                                ),
                            );
                        }
                    }
                }
            }
        }
        // Found in review: a brighter input came out darker.
        let tone = GlobalTone::new([2., 2., -1., 2.], 1., 0., -14., -14., true);
        monotone(&tone, "review case");
        let tone = GlobalTone::new([-1., -2., -5., -1.], 0.75, 0., -14., -14., false);
        monotone(&tone, "review case without a black point");
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
        let render = GlobalTone::new([1.3, 0.2, 0.2, 1.2], 0.2, -0.1, -5., -5., true);
        let p = [0.4f32, 0.3, 0.6];
        assert_eq!(render.apply_at(p, 0., 0., 0.), render.apply(p));
        // Measured: a mask's Whites +50 takes 0.18 (default output 0.177) to 0.240.
        let plain = reaching(0., 0., 0., true);
        let lifted = plain.apply_at([0.18; 3], 0.5, 0., 0.)[0];
        assert!((lifted - 0.240).abs() < 0.005, "{lifted}");
        // Continuous through 0.
        let small = plain.apply_at([0.18; 3], 0.01, 0., 0.)[0];
        assert!((small - plain.apply([0.18; 3])[0]).abs() < 0.005);
        let dark = plain.apply_at([0.05; 3], 0., -0.5, 0.)[0];
        assert!(dark < plain.apply([0.05; 3])[0]);
    }

    /// Camera Raw's default black adapts to the photo's darkest level (its 0.1th
    /// percentile) relative to the sensor's white: a photo whose darkest level is high
    /// (8.5 stops below the sensor's white) renders its darkest values darker than one
    /// whose darkest level is low (14 stops below, as the white ramps the white curves
    /// come from: none), and mid gray the same. The synthetic chart, 11.6 stops below,
    /// has none either.
    #[test]
    fn the_default_black_follows_the_photos_darkest_level() {
        // The minimum (Blacks' key) at 2^-14 throughout: the default black reads the
        // percentile only.
        let keyed = |dark: f32, black_point: bool| {
            GlobalTone::new([0., 0., 0., 0.], 0., 0., -14., dark, black_point)
        };
        assert!(keyed(-14., false).default_black_samples().is_none());
        assert_eq!(
            GlobalTone::new([0.; 4], 0., 0., -8.5, -8.5, true).default_black_samples(),
            keyed(-8.5, true).default_black_samples()
        );
        // Camera baseline exposure is not subtracted from the scene key. The
        // Camera Raw ramp regression covers the pixel outputs across baselines
        // and the change in response when Exposure moves the scene values.
        let (high, low) = (keyed(-8.5, true), keyed(-14., true));
        for x in [2f32.powf(-8.), 2f32.powf(-7.5)] {
            let (h, l) = (high.apply([x; 3])[1], low.apply([x; 3])[1]);
            assert!(h < 0.95 * l, "{x}: {h} against {l}");
        }
        let (h, l) = (high.apply([0.18; 3])[1], low.apply([0.18; 3])[1]);
        assert!((h / l - 1.).abs() < 0.01, "{h} against {l}");
        // Without a default black the level changes nothing.
        for x in [2f32.powf(-10.), 2f32.powf(-6.), 0.18] {
            assert_eq!(
                keyed(-8.5, false).apply([x; 3]),
                keyed(-14., false).apply([x; 3])
            );
        }
    }

    #[test]
    fn black_correction_preserves_order_and_the_calibration_identity() {
        for step in 0..=28 {
            let key = -14. + step as f32 * 0.25;
            let identity = black_correction(key, key);
            for (k, y) in identity.iter().enumerate() {
                assert_eq!(*y, (Y_FIRST + k as f32 * Y_STEP).exp2());
            }
            for calibration in [-16., -14., -12., -10., -8., -6.] {
                let curve = black_correction(key, calibration);
                assert!(curve.iter().all(|v| v.is_finite() && *v >= 0.));
                assert!(
                    curve.windows(2).all(|v| v[1] + 1e-7 >= v[0]),
                    "key {key}, calibration {calibration}"
                );
            }
        }
    }

    /// Through the white curve; the default black after it works on each channel, so
    /// matching the calibration's darkest level leaves that response unchanged.
    #[test]
    fn the_middle_channel_keeps_its_place() {
        let tone = GlobalTone::new([2.; 4], 0.5, 0., -12., -12., true);
        let [r, g, b] = tone.apply([1.2, 0.6, 0.3]);
        assert!(r > g && g > b);
        assert!(((g - b) / (r - b) - (0.6 - 0.3) / (1.2 - 0.3)).abs() < 1e-5);
    }
}
