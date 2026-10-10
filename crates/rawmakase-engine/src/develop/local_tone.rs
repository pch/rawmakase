//! Shadows and Highlights in the scene tone stage: an edge-aware local tone operator
//! fitted to Camera Raw on scene values, before the white point and the profile's tone
//! curve (docs/scene-tone-stage.md). The base level is a guided filter of log2 scene
//! luminance, with a radius of 3.2% of the long edge and ε = 0.5 (log2 units squared).
//! The measured tables in `local_tone_data.rs` give the log2 gain for each base level
//! relative to an image key. The base level and keys come from the photo's measurement
//! copy, so the result does not depend on the rendered region or preview size.
use super::local_tone_data::{Family, HIGHLIGHTS, SHADOWS};
use rayon::prelude::*;

/// Long edge of the reduced image the base level is computed on.
pub(crate) const MAP_EDGE: u32 = 512;
const RADIUS: f32 = 0.032;
const EPSILON: f32 = 0.5;
/// The darkest scene luminance the map tells apart (−14 stops).
pub(crate) const FLOOR: f32 = 6.103_515_6e-5;

pub(crate) struct LocalToneMap {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Guided-filter coefficients: base = a · log2(Y) + b.
    pub(crate) a: Vec<f32>,
    pub(crate) b: Vec<f32>,
    /// Source image size, to convert sample coordinates.
    pub(crate) scale: [f32; 2],
    pub(crate) shadows: Option<Curve>,
    pub(crate) highlights: Option<Curve>,
    /// Image keys of Shadows and Highlights, for masks that evaluate them at their own
    /// slider values.
    pub(crate) keys: [f32; 2],
    /// Clarity's log2 gain on this grid (`clarity.rs`).
    pub(crate) clarity: Option<Vec<f32>>,
    /// Clarity's detail, when masks change Clarity and read it at their own amount.
    pub(crate) clarity_detail: Option<super::clarity::ClarityDetail>,
}
pub(crate) struct Curve {
    pub(crate) key: f32,
    pub(crate) lo: f32,
    pub(crate) hi: f32,
    pub(crate) table: [f32; 48],
}
impl Curve {
    fn new(family: &Family, s: f32, key: f32) -> Option<Self> {
        if s == 0. {
            return None;
        }
        let s = s.clamp(-1., 1.);
        let mut table = [0.; 48];
        // Interpolate between measured positions; 0 is no change.
        let mut points: Vec<(f32, Option<&[f32; 48]>)> = family
            .values
            .iter()
            .zip(&family.tables)
            .map(|(v, t)| (*v, Some(t)))
            .collect();
        points.insert(3, (0., None));
        let j = points
            .windows(2)
            .position(|w| s <= w[1].0)
            .unwrap_or(points.len() - 2);
        let ((s0, t0), (s1, t1)) = (points[j], points[j + 1]);
        let w = (s - s0) / (s1 - s0);
        for (i, v) in table.iter_mut().enumerate() {
            let y0 = t0.map_or(0., |t| t[i]);
            let y1 = t1.map_or(0., |t| t[i]);
            *v = y0 + (y1 - y0) * w;
        }
        Some(Self {
            key,
            lo: family.lo,
            hi: family.hi,
            table,
        })
    }
    fn eval(&self, base: f32) -> f32 {
        let f = ((base - self.key - self.lo) / (self.hi - self.lo) * 48. - 0.5).clamp(0., 47.);
        let i = (f as usize).min(46);
        self.table[i] + (self.table[i + 1] - self.table[i]) * (f - i as f32)
    }
}
/// The slider values a map is built for: Shadows, Highlights and Clarity, and whether
/// masks change Clarity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Sliders {
    pub(crate) shadows: f32,
    pub(crate) highlights: f32,
    pub(crate) clarity: f32,
    pub(crate) mask_clarity: bool,
}
impl Sliders {
    pub(crate) fn of(r: &crate::model::recipe::Recipe) -> Self {
        Self {
            shadows: r.shadows,
            highlights: r.highlights,
            clarity: super::clarity::measured(r),
            mask_clarity: r
                .masks
                .iter()
                .any(|m| m.is_active() && m.adjust.clarity != 0.),
        }
    }
}
/// log2 of a scene luminance, as the map reads it.
pub(crate) fn level(y: f32) -> f32 {
    y.max(FLOOR).log2()
}
impl LocalToneMap {
    /// The map of `small`, the photo's measurement copy (`Toned::measured`) for a
    /// `source`-sized image; `scene` maps a camera sample to its scene luminance at the
    /// scene tone stage's input. Built when either slider or the measured `clarity` is
    /// set, or when `local` (masks change Shadows or Highlights).
    pub(crate) fn build(
        small: &crate::camera_data::CameraImage,
        source: [u32; 2],
        sliders: Sliders,
        local: bool,
        scene: impl Fn([f32; 3]) -> f32 + Sync,
    ) -> Option<Self> {
        if sliders.shadows == 0.
            && sliders.highlights == 0.
            && sliders.clarity == 0.
            && !sliders.mask_clarity
            && !local
        {
            return None;
        }
        let lum: Vec<f32> = small.pixels.par_iter().map(|p| scene(*p)).collect();
        Some(Self::from_luminance(
            lum,
            [small.width, small.height],
            source,
            sliders,
        ))
    }
    /// The map from scene luminance at `size` pixels of a `source`-sized photo, as
    /// `build` computes it.
    pub(crate) fn from_luminance(
        lum: Vec<f32>,
        size: [u32; 2],
        source: [u32; 2],
        sliders: Sliders,
    ) -> Self {
        let (w, h) = (size[0] as usize, size[1] as usize);
        let logs: Vec<f32> = lum.iter().map(|y| level(*y)).collect();
        let percentile = |q: f32| {
            let mut v = logs.clone();
            let k = ((v.len() - 1) as f32 * q) as usize;
            v.select_nth_unstable_by(k, f32::total_cmp);
            v[k]
        };
        let r = ((RADIUS * w.max(h) as f32).round() as usize).max(1);
        // He et al. guided filter with the image as its own guide.
        let mean = |x: &[f32]| blur(x, w, h, r);
        let sq: Vec<f32> = logs.iter().map(|v| v * v).collect();
        let (m, m2) = rayon::join(|| mean(&logs), || mean(&sq));
        let a: Vec<f32> = m
            .iter()
            .zip(&m2)
            .map(|(m, m2)| {
                let var = (m2 - m * m).max(0.);
                var / (var + EPSILON)
            })
            .collect();
        let b: Vec<f32> = m.iter().zip(&a).map(|(m, a)| m - a * m).collect();
        let (a, b) = rayon::join(|| mean(&a), || mean(&b));
        // Masks may evaluate either slider, so both keys are kept. Shadows follows the
        // photo's mean luminance (fitted on photos), Highlights the middle of its
        // range (fitted on synthetic scenes; docs/scene-tone-stage.md).
        let average = logs.iter().map(|l| l.exp2() as f64).sum::<f64>() / logs.len() as f64;
        let keys = [
            (average as f32).log2(),
            0.5 * percentile(0.75) + 0.5 * percentile(0.01),
        ];
        // Clarity's weights were fitted relative to the photo's brightest levels.
        let clarity_key = 0.25 * percentile(0.999) + 0.75 * percentile(0.99);
        let detail = (sliders.clarity != 0. || sliders.mask_clarity).then(|| {
            let base: Vec<f32> = logs
                .iter()
                .zip(a.iter().zip(&b))
                .map(|(l, (a, b))| a * l + b)
                .collect();
            super::clarity::ClarityDetail::of(&logs, &base, w, h, clarity_key)
        });
        let clarity = detail
            .as_ref()
            .filter(|_| sliders.clarity != 0.)
            .map(|d| d.field(sliders.clarity));
        Self {
            width: w,
            height: h,
            a,
            b,
            scale: [w as f32 / source[0] as f32, h as f32 / source[1] as f32],
            shadows: Curve::new(&SHADOWS, sliders.shadows, keys[0]),
            highlights: Curve::new(&HIGHLIGHTS, sliders.highlights, keys[1]),
            keys,
            clarity,
            clarity_detail: detail.filter(|_| sliders.mask_clarity),
        }
    }
    /// Luminance gain for a pixel of scene luminance `lum` at camera-image sample
    /// position `x`, `y`.
    pub(crate) fn gain(&self, x: f32, y: f32, lum: f32) -> f32 {
        let base = self.base(x, y, lum);
        let ev = self.shadows.as_ref().map_or(0., |c| c.eval(base))
            + self.highlights.as_ref().map_or(0., |c| c.eval(base));
        (ev + self.clarity(x, y)).exp2()
    }
    /// As [`Self::gain`], with Shadows, Highlights and Clarity at the given slider
    /// values (a mask's added to the recipe's).
    pub(crate) fn gain_with(&self, x: f32, y: f32, lum: f32, sliders: [f32; 3]) -> f32 {
        let base = self.base(x, y, lum);
        let clarity = match &self.clarity_detail {
            Some(d) => d.at(|v| self.bilinear(x, y, v), sliders[2]),
            None => self.clarity(x, y),
        };
        (family(&SHADOWS, sliders[0], self.keys[0], base)
            + family(&HIGHLIGHTS, sliders[1], self.keys[1], base)
            + clarity)
            .exp2()
    }
    fn base(&self, x: f32, y: f32, lum: f32) -> f32 {
        self.bilinear(x, y, &self.a) * level(lum) + self.bilinear(x, y, &self.b)
    }
    /// The measured Clarity's log2 gain at sample position `x`, `y`.
    fn clarity(&self, x: f32, y: f32) -> f32 {
        self.clarity.as_ref().map_or(0., |c| self.bilinear(x, y, c))
    }
    /// `v` on this grid at camera-image sample position `x`, `y`.
    fn bilinear(&self, x: f32, y: f32, v: &[f32]) -> f32 {
        grid_sample(v, [self.width, self.height], self.scale, [x, y])
    }
}
/// `v` on a `size` grid at camera-image sample position `pos`, bilinearly; `scale` is
/// the grid's size relative to the camera image's.
pub(crate) fn grid_sample(v: &[f32], size: [usize; 2], scale: [f32; 2], pos: [f32; 2]) -> f32 {
    let [width, height] = size;
    let fx = ((pos[0] + 0.5) * scale[0] - 0.5).clamp(0., (width - 1) as f32);
    let fy = ((pos[1] + 0.5) * scale[1] - 0.5).clamp(0., (height - 1) as f32);
    let (ix, iy) = (fx as usize, fy as usize);
    let (jx, jy) = ((ix + 1).min(width - 1), (iy + 1).min(height - 1));
    let (tx, ty) = (fx - ix as f32, fy - iy as f32);
    let top = v[iy * width + ix] * (1. - tx) + v[iy * width + jx] * tx;
    let bottom = v[jy * width + ix] * (1. - tx) + v[jy * width + jx] * tx;
    top * (1. - ty) + bottom * ty
}
/// The measured positions a slider is bracketed in, as `Curve::new` builds them: each
/// position with its table, and the identity at 0 in slot `IDENTITY`.
type Points<'a> = [(f32, Option<&'a [f32; 48]>); 7];
/// Slot the identity takes, after the negative slider positions.
const IDENTITY: usize = 3;

/// A family's log2 gain at slider `s` and base level `base`, as `Curve::new(..).eval`
/// without building the table.
pub(crate) fn family(f: &Family, s: f32, key: f32, base: f32) -> f32 {
    if s == 0. {
        return 0.;
    }
    let s = s.clamp(-1., 1.);
    let bin = |t: &[f32; 48]| {
        let x = ((base - key - f.lo) / (f.hi - f.lo) * 48. - 0.5).clamp(0., 47.);
        let i = (x as usize).min(46);
        t[i] + (t[i + 1] - t[i]) * (x - i as f32)
    };
    // On the stack: this bracket is built per pixel, for both families.
    let points: Points = std::array::from_fn(|slot| {
        if slot == IDENTITY {
            (0., None)
        } else {
            let i = if slot < IDENTITY { slot } else { slot - 1 };
            (f.values[i], Some(&f.tables[i]))
        }
    });
    let j = points
        .windows(2)
        .position(|w| s <= w[1].0)
        .unwrap_or(points.len() - 2);
    let ((s0, t0), (s1, t1)) = (points[j], points[j + 1]);
    let w = (s - s0) / (s1 - s0);
    let (y0, y1) = (t0.map_or(0., bin), t1.map_or(0., bin));
    y0 + (y1 - y0) * w
}
/// The measured families for `develop.wgsl`: Shadows, Highlights and Dehaze, each its
/// 6 × 48 table values, `lo` and `hi`, then its six slider positions.
pub(crate) fn gpu_families() -> Vec<f32> {
    let mut out = Vec::new();
    for f in [&SHADOWS, &HIGHLIGHTS, &super::local_tone_data::DEHAZE] {
        out.extend(f.tables.iter().flatten());
        out.extend([f.lo, f.hi]);
        out.extend(f.values);
    }
    out
}
/// Mean over a (2r+1)² window, clamped at the borders, via running sums.
pub(super) fn blur(x: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    // One line: `out[i]` for `len` values read through `get`; lines are independent,
    // so they run in parallel with the same arithmetic.
    let line = |len: usize, get: &dyn Fn(usize) -> f32| -> Vec<f32> {
        let get = |i: isize| get(i.clamp(0, len as isize - 1) as usize) as f64;
        let mut out = vec![0.; len];
        let mut sum: f64 = (-(r as isize)..=r as isize).map(get).sum();
        for (i, v) in out.iter_mut().enumerate() {
            *v = (sum / (2 * r + 1) as f64) as f32;
            sum += get(i as isize + r as isize + 1) - get(i as isize - r as isize);
        }
        out
    };
    let rows: Vec<f32> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| line(w, &|i| x[y * w + i]))
        .collect();
    let columns: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|c| line(h, &|i| rows[i * w + c]))
        .collect();
    let mut out = vec![0.; x.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (c, v) in row.iter_mut().enumerate() {
            *v = columns[c][y];
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blur_preserves_constants_and_means() {
        let x = vec![2.; 30];
        assert!(blur(&x, 6, 5, 2).iter().all(|v| (v - 2.).abs() < 1e-6));
        let mut x = vec![0.; 25];
        x[12] = 25.;
        let b = blur(&x, 5, 5, 1);
        assert!((b[12] - 25. / 9.).abs() < 1e-5);
    }
    #[test]
    fn curves_interpolate_and_vanish_at_zero() {
        assert!(Curve::new(&SHADOWS, 0., 0.).is_none());
        let c = Curve::new(&SHADOWS, 0.5, -1.).unwrap();
        // Positive Shadows lifts dark bases and leaves the key level almost unchanged.
        assert!(c.eval(-7.) > 0.5);
        assert!(c.eval(-1.).abs() < 0.1);
        let half = Curve::new(&SHADOWS, 0.125, -1.).unwrap();
        let full = Curve::new(&SHADOWS, 0.25, -1.).unwrap();
        assert!((half.eval(-6.) - full.eval(-6.) / 2.).abs() < 1e-5);
        // Negative Highlights darkens bases above its key.
        let h = Curve::new(&HIGHLIGHTS, -0.5, -3.).unwrap();
        assert!(h.eval(0.) < -0.2);
        // Per-pixel evaluation matches the interpolated table.
        for base in [-8., -3., -1., 0.2] {
            assert!(
                (family(&SHADOWS, 0.45, -1., base)
                    - Curve::new(&SHADOWS, 0.45, -1.).unwrap().eval(base))
                .abs()
                    < 1e-5
            );
            assert!(
                (family(&HIGHLIGHTS, -0.8, -3., base)
                    - Curve::new(&HIGHLIGHTS, -0.8, -3.).unwrap().eval(base))
                .abs()
                    < 1e-5
            );
        }
    }
}
