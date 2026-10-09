//! The global Contrast slider, as Camera Raw's measured response on the synthetic
//! chart moved to each photo's pivot. Values between the measured slider positions are
//! interpolated linearly, with 0 as the identity.
use super::basic_tone_data::{CONTRAST_CHART, CONTRAST_PIVOT, SLIDER_VALUES};
use crate::color::{srgb_decode, srgb_encode};

/// What a render's Contrast follows of the photo.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PhotoTone {
    /// The encoded level Contrast pivots at.
    pub(crate) contrast_pivot: f32,
}
/// The pivot of a photo whose own isn't measured: about the middle of the photos'.
pub(crate) const TYPICAL_PIVOT: f32 = 0.5;

/// Camera Raw's Contrast pivot for a photo, from its default rendering (encoded sRGB)
/// reduced to blocks: it rises with the mean luminance and falls with the middle of
/// its range (the 1st and 99th percentiles), fitted on 33 photos and the chart.
pub(crate) fn photo_pivot(blocks: &[[f32; 3]]) -> f32 {
    let mut lum: Vec<f32> = blocks
        .iter()
        .map(|p| {
            let [r, g, b] = p.map(|v| srgb_decode(v.clamp(0., 1.)));
            srgb_encode((crate::color::luminance([r, g, b])).clamp(1e-5, 1.))
        })
        .collect();
    if lum.is_empty() {
        return TYPICAL_PIVOT;
    }
    let mean = lum.iter().sum::<f32>() / lum.len() as f32;
    lum.sort_by(f32::total_cmp);
    let last = lum.len() - 1;
    let percentile = |q: f32| {
        let at = last as f32 * q;
        let i = at as usize;
        let j = (i + 1).min(last);
        lum[i] + (lum[j] - lum[i]) * (at - i as f32)
    };
    let middle = (percentile(0.01) + percentile(0.99)) / 2.;
    (PIVOT_FIT[0] + PIVOT_FIT[1] * mean + PIVOT_FIT[2] * middle).clamp(0.3, 0.75)
}
/// `pixels` (`width` × `height`, encoded) averaged into blocks 48 across, as the
/// photos [`photo_pivot`] was fitted on were measured.
pub(crate) fn blocks(pixels: &[[f32; 3]], width: usize, height: usize) -> Vec<[f32; 3]> {
    const COLUMNS: usize = 48;
    let rows = ((COLUMNS * height) as f32 / width.max(1) as f32)
        .round()
        .max(1.) as usize;
    let edge = |i: usize, n: usize, size: usize| i * size / n;
    let mut out = Vec::with_capacity(rows * COLUMNS);
    for r in 0..rows {
        for c in 0..COLUMNS {
            let (x0, x1) = (edge(c, COLUMNS, width), edge(c + 1, COLUMNS, width));
            let (y0, y1) = (edge(r, rows, height), edge(r + 1, rows, height));
            let mut sum = [0f32; 3];
            let mut n = 0;
            for y in y0..y1.max(y0 + 1).min(height) {
                for x in x0..x1.max(x0 + 1).min(width) {
                    let p = pixels[y * width + x];
                    (0..3).for_each(|k| sum[k] += p[k]);
                    n += 1;
                }
            }
            if n > 0 {
                out.push(sum.map(|v| v / n as f32));
            }
        }
    }
    out
}
/// Constant, mean and range-middle weights of [`photo_pivot`].
const PIVOT_FIT: [f32; 3] = [0.577, 0.568, -0.689];

const SIZE: usize = 1024;

/// The Contrast curve, sampled at `SIZE + 1` points over 0–1.
#[derive(Clone)]
pub(crate) struct BasicTone {
    pub(crate) lut: Vec<f32>,
}
impl BasicTone {
    pub(crate) fn new(contrast: f32, photo: &PhotoTone) -> Option<Self> {
        if contrast == 0. {
            return None;
        }
        let lut = (0..=SIZE)
            .map(|i| {
                let x = i as f32 / SIZE as f32;
                contrast_at(contrast, photo.contrast_pivot, x)
            })
            // Measured tables carry small non-monotone noise; tone must never invert.
            .scan(0f32, |max, y| {
                *max = max.max(y);
                Some(*max)
            })
            .collect();
        Some(Self { lut })
    }
    pub(crate) fn eval(&self, x: f32) -> f32 {
        let f = x.clamp(0., 1.) * SIZE as f32;
        let i = (f as usize).min(SIZE - 1);
        self.lut[i] + (self.lut[i + 1] - self.lut[i]) * (f - i as f32)
    }
    /// DNG RGBTone: the curve maps the largest and smallest channel, and the middle
    /// channel keeps its relative position, preserving hue.
    pub(crate) fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let p = p.map(|v| v.clamp(0., 1.));
        let lo = p.into_iter().fold(f32::INFINITY, f32::min);
        let hi = p.into_iter().fold(0f32, f32::max);
        let (a, b) = (self.eval(lo), self.eval(hi));
        if hi - lo > 1e-8 {
            p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
        } else {
            [a; 3]
        }
    }
}

/// The measured tables in the order `develop.wgsl`'s local Contrast reads them: the
/// slider positions, the chart's Contrast (6 × 64) and its pivot.
pub(crate) fn gpu_tables() -> Vec<f32> {
    let mut out: Vec<f32> = SLIDER_VALUES.to_vec();
    out.extend(CONTRAST_CHART.iter().flatten());
    out.push(CONTRAST_PIVOT);
    out
}
/// One measured table: curve for slider `s` at input `x`.
fn slider(table: &[[f32; 64]; 6], s: f32, x: f32) -> f32 {
    if s == 0. {
        return x;
    }
    let s = s.clamp(-1., 1.);
    // Bracketing measured positions, with the identity at 0.
    let mut points: Vec<(f32, Option<&[f32; 64]>)> = SLIDER_VALUES
        .iter()
        .zip(table)
        .map(|(v, t)| (*v, Some(t)))
        .collect();
    points.insert(3, (0., None));
    let j = points
        .windows(2)
        .position(|w| s <= w[1].0)
        .unwrap_or(points.len() - 2);
    let (s0, t0) = points[j];
    let (s1, t1) = points[j + 1];
    let w = (s - s0) / (s1 - s0);
    let y0 = t0.map_or(x, |t| curve(t, x));
    let y1 = t1.map_or(x, |t| curve(t, x));
    y0 + (y1 - y0) * w
}

/// The chart's Contrast at slider `s`, moved so it pivots at `pivot` instead of
/// [`CONTRAST_PIVOT`]: a power warp of gamma-2.2 encoded values takes one pivot to the
/// other, which fits each photo's Camera Raw Contrast to 0.002–0.01 at every amount.
/// The local adjustments evaluate it per pixel, without `BasicTone`'s table and
/// monotone clean-up.
pub(crate) fn contrast_at(s: f32, pivot: f32, x: f32) -> f32 {
    if s == 0. {
        return x;
    }
    let to = |v: f32| srgb_decode(v.clamp(0., 1.)).powf(1. / 2.2);
    let from = |w: f32| srgb_encode(w.clamp(0., 1.).powf(2.2));
    let k = to(CONTRAST_PIVOT).ln() / to(pivot).ln();
    let y = slider(&CONTRAST_CHART, s, from(to(x).powf(k)));
    from(to(y).powf(1. / k))
}

/// Linear interpolation between bin centres; linear extrapolation to 0 and 1.
fn curve(t: &[f32; 64], x: f32) -> f32 {
    let f = (x * 64. - 0.5).clamp(-0.5, 63.5);
    let i = (f.floor() as isize).clamp(0, 62) as usize;
    let y = t[i] + (t[i + 1] - t[i]) * (f - i as f32);
    y.clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_contrast_is_identity_and_curves_are_monotone() {
        let typical = PhotoTone {
            contrast_pivot: TYPICAL_PIVOT,
        };
        assert!(BasicTone::new(0., &typical).is_none());
        for c in [-1., -0.6, -0.1, 0.1, 0.4, 1.] {
            let t = BasicTone::new(c, &typical).unwrap();
            assert!(t.lut.windows(2).all(|p| p[1] >= p[0] - 1e-4), "{c}");
            assert!(t.lut.iter().all(|v| (0. ..=1.).contains(v)));
        }
        // A small slider value changes the curve only slightly.
        let t = BasicTone::new(0.01, &typical).unwrap();
        assert!((0..=10).all(|i| (t.eval(i as f32 / 10.) - i as f32 / 10.).abs() < 0.01));
        // Positive contrast darkens shadows and brightens highlights.
        let t = BasicTone::new(0.5, &typical).unwrap();
        assert!(t.eval(0.2) < 0.2 && t.eval(0.8) > 0.8);
        let gray = t.apply([0.3; 3]);
        assert!((gray[0] - gray[1]).abs() < 1e-6 && (gray[1] - gray[2]).abs() < 1e-6);
    }

    /// Camera Raw 18.7 on the synthetic chart's gray ramp, whose Contrast pivot is the
    /// chart's: the default level and the level with this Contrast (encoded, 0–1).
    #[test]
    fn contrast_matches_camera_raw_on_the_chart() {
        let cases: [(f32, [[f32; 2]; 3]); 2] = [
            (0.5, [[0.2372, 0.1823], [0.4798, 0.4527], [0.6915, 0.7256]]),
            (-1., [[0.1803, 0.2759], [0.4798, 0.5181], [0.7847, 0.7118]]),
        ];
        for (contrast, levels) in cases {
            let t = BasicTone::new(
                contrast,
                &PhotoTone {
                    contrast_pivot: CONTRAST_PIVOT,
                },
            )
            .unwrap();
            for [x, expected] in levels {
                let y = t.eval(x);
                assert!(
                    (y - expected).abs() < 2. / 255.,
                    "Contrast {contrast}: {x} gives {y}, Camera Raw {expected}"
                );
            }
        }
    }

    #[test]
    fn contrast_pivots_where_the_photo_puts_it() {
        for pivot in [0.35, 0.45, 0.6] {
            let t = BasicTone::new(
                0.8,
                &PhotoTone {
                    contrast_pivot: pivot,
                },
            )
            .unwrap();
            assert!((t.eval(pivot) - pivot).abs() < 0.01, "{pivot}");
            assert!(t.eval(pivot - 0.1) < pivot - 0.1 && t.eval(pivot + 0.1) > pivot + 0.1);
        }
        // An even photo pivots a little below the middle, and one whose levels sit low
        // in their range (dark, with a few bright highlights) lower still.
        let flat = |level: f32| vec![[level; 3]; 100];
        assert!((photo_pivot(&flat(0.5)) - (0.577 + 0.568 * 0.5 - 0.689 * 0.5)).abs() < 1e-3);
        let mut dark = flat(0.3);
        dark.extend(flat(0.95).into_iter().take(5));
        assert!(photo_pivot(&dark) < photo_pivot(&flat(0.3)) - 0.15);
        assert_eq!(photo_pivot(&[]), TYPICAL_PIVOT);
        assert_eq!(blocks(&vec![[0.5; 3]; 960 * 640], 960, 640).len(), 48 * 32);
    }
}
