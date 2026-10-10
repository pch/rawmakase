//! Engine 4 Shadows and Highlights: an edge-aware local tone operator fitted to Camera
//! Raw. The base level is a guided filter of log2 luminance of the toned image, with a
//! radius of 3.2% of the long edge and ε = 1.5 (log2 units squared). The measured
//! tables in `local_tone_data.rs` give the log2 gain for each base level relative to an
//! image key. The base level and keys come from a reduced copy of the photo, so the
//! result does not depend on the rendered region or preview size.
use super::local_tone_data::{Family, HIGHLIGHTS, SHADOWS, SLIDER_VALUES};
use rayon::prelude::*;

/// Long edge of the reduced image the base level is computed on.
pub(crate) const MAP_EDGE: u32 = 512;
const RADIUS: f32 = 0.032;
pub(crate) const EPSILON: f32 = 1.5;

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
    /// The measured positive Clarity's log2 gain on this grid (`clarity.rs`).
    pub(crate) clarity: Option<Vec<f32>>,
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
        let mut points: Vec<(f32, Option<&[f32; 48]>)> = SLIDER_VALUES
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
/// The slider values a map is built for: Shadows, Highlights and the measured
/// positive Clarity (0 when the recipe renders Clarity otherwise).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Sliders {
    pub(crate) shadows: f32,
    pub(crate) highlights: f32,
    pub(crate) clarity: f32,
}
impl Sliders {
    pub(crate) fn of(r: &crate::model::recipe::Recipe) -> Self {
        Self {
            shadows: r.shadows,
            highlights: r.highlights,
            clarity: super::clarity::measured(r),
        }
    }
}
pub(crate) fn luminance(rgb: [f32; 3]) -> f32 {
    (crate::color::luminance(rgb)).max(6e-4)
}
impl LocalToneMap {
    /// `tone` maps a camera sample to linear display RGB after the profile tone curve.
    /// Built when either slider or the measured `clarity` is set, or when `local`
    /// (masks change Shadows or Highlights).
    pub(crate) fn build(
        im: super::pipeline::Source,
        sliders: Sliders,
        local: bool,
        tone: impl Fn([f32; 3]) -> [f32; 3] + Sync,
    ) -> Option<Self> {
        if sliders.shadows == 0. && sliders.highlights == 0. && sliders.clarity == 0. && !local {
            return None;
        }
        let small = match im.reduced {
            Some(small) => std::borrow::Cow::Borrowed(small),
            None => std::borrow::Cow::Owned(super::pipeline::preview_source(im, MAP_EDGE)),
        };
        let lum: Vec<f32> = small
            .pixels
            .par_iter()
            .map(|p| luminance(tone(*p)))
            .collect();
        Some(Self::from_luminance(
            lum,
            [small.width, small.height],
            [im.width, im.height],
            sliders,
        ))
    }
    /// The map from the luminance of the reduced photo toned, `size` pixels of a
    /// `source`-sized photo, as `build` computes it (`luminance` of `tone`).
    pub(crate) fn from_luminance(
        lum: Vec<f32>,
        size: [u32; 2],
        source: [u32; 2],
        sliders: Sliders,
    ) -> Self {
        Self::from_base(&MapBase::new(lum, size, source), sliders)
    }
    /// The map of `base` at these slider values: only the curves and the measured
    /// Clarity depend on them.
    pub(crate) fn from_base(base: &MapBase, sliders: Sliders) -> Self {
        let (w, h) = (base.width, base.height);
        let keys = base.keys;
        let clarity = (sliders.clarity > 0.).then(|| {
            let level: Vec<f32> = base
                .logs
                .iter()
                .zip(base.a.iter().zip(&base.b))
                .map(|(l, (a, b))| a * l + b)
                .collect();
            super::clarity::field(&base.logs, &level, w, h, keys[0], sliders.clarity)
        });
        Self {
            width: w,
            height: h,
            a: base.a.clone(),
            b: base.b.clone(),
            scale: base.scale,
            shadows: Curve::new(&SHADOWS, sliders.shadows, keys[0]),
            highlights: Curve::new(&HIGHLIGHTS, sliders.highlights, keys[1]),
            keys,
            clarity,
        }
    }
    /// Luminance gain for a toned pixel at camera-image sample position `x`, `y`.
    pub(crate) fn gain(&self, x: f32, y: f32, rgb: [f32; 3]) -> f32 {
        let base = self.base(x, y, rgb);
        let ev = self.shadows.as_ref().map_or(0., |c| c.eval(base))
            + self.highlights.as_ref().map_or(0., |c| c.eval(base));
        (ev + self.clarity(x, y)).exp2()
    }
    /// As [`Self::gain`], with Shadows and Highlights at the given slider values.
    pub(crate) fn gain_with(&self, x: f32, y: f32, rgb: [f32; 3], sliders: [f32; 2]) -> f32 {
        let base = self.base(x, y, rgb);
        (family(&SHADOWS, sliders[0], self.keys[0], base)
            + family(&HIGHLIGHTS, sliders[1], self.keys[1], base)
            + self.clarity(x, y))
        .exp2()
    }
    fn base(&self, x: f32, y: f32, rgb: [f32; 3]) -> f32 {
        self.bilinear(x, y, &self.a) * luminance(rgb).log2() + self.bilinear(x, y, &self.b)
    }
    /// The measured Clarity's log2 gain at sample position `x`, `y`.
    fn clarity(&self, x: f32, y: f32) -> f32 {
        self.clarity.as_ref().map_or(0., |c| self.bilinear(x, y, c))
    }
    /// `v` on this grid at camera-image sample position `x`, `y`.
    fn bilinear(&self, x: f32, y: f32, v: &[f32]) -> f32 {
        let fx = ((x + 0.5) * self.scale[0] - 0.5).clamp(0., (self.width - 1) as f32);
        let fy = ((y + 0.5) * self.scale[1] - 0.5).clamp(0., (self.height - 1) as f32);
        let (ix, iy) = (fx as usize, fy as usize);
        let (jx, jy) = ((ix + 1).min(self.width - 1), (iy + 1).min(self.height - 1));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let top = v[iy * self.width + ix] * (1. - tx) + v[iy * self.width + jx] * tx;
        let bottom = v[jy * self.width + ix] * (1. - tx) + v[jy * self.width + jx] * tx;
        top * (1. - ty) + bottom * ty
    }
}
/// What the map takes from the photo, whatever the sliders: the guided filter's
/// coefficients and the image keys. Built from the reduced photo toned at the recipe's
/// exposure, so the Shadows, Highlights and Clarity sliders reuse it.
pub(crate) struct MapBase {
    width: usize,
    height: usize,
    logs: Vec<f32>,
    a: Vec<f32>,
    b: Vec<f32>,
    scale: [f32; 2],
    keys: [f32; 2],
}
impl MapBase {
    /// From the luminance of the reduced photo toned, `size` pixels of a `source`-sized
    /// photo.
    pub(crate) fn new(lum: Vec<f32>, size: [u32; 2], source: [u32; 2]) -> Self {
        let (w, h) = (size[0] as usize, size[1] as usize);
        let logs: Vec<f32> = lum.par_iter().map(|y| y.log2()).collect();
        // Both keys from one copy: the higher percentile first, then the lower one among
        // the values below it, which holds the same element.
        let percentiles = |k: [usize; 2]| -> [f32; 2] {
            let mut v = lum.clone();
            let (lo, hi) = if k[0] <= k[1] { (0, 1) } else { (1, 0) };
            v.select_nth_unstable_by(k[hi], f32::total_cmp);
            let high = v[k[hi]];
            v[..=k[hi]].select_nth_unstable_by(k[lo], f32::total_cmp);
            let mut out = [0.; 2];
            out[hi] = high.log2();
            out[lo] = v[k[lo]].log2();
            out
        };
        let r = radius(w, h);
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
        // Masks may evaluate either slider, so both keys are kept.
        let keys = percentiles(key_ranks(lum.len()));
        Self {
            width: w,
            height: h,
            logs,
            a,
            b,
            scale: scale(size, source),
            keys,
        }
    }
    /// The guided filter's coefficients a and b, and the keys.
    #[cfg(test)]
    pub(crate) fn parts(&self) -> (&[f32], &[f32], [f32; 2]) {
        (&self.a, &self.b, self.keys)
    }
    /// Memory held, for the stage cache's budget.
    pub(crate) fn bytes(&self) -> usize {
        (self.logs.len() + self.a.len() + self.b.len()) * 4
    }
}
/// The Shadows and Highlights curves at `sliders`, for a map whose keys are on the
/// device: their own keys are 0.
pub(crate) fn curves(sliders: Sliders) -> [Option<Curve>; 2] {
    [
        Curve::new(&SHADOWS, sliders.shadows, 0.),
        Curve::new(&HIGHLIGHTS, sliders.highlights, 0.),
    ]
}
/// The guided filter's radius on a `w` × `h` grid.
pub(crate) fn radius(w: usize, h: usize) -> usize {
    ((RADIUS * w.max(h) as f32).round() as usize).max(1)
}
/// The Shadows and Highlights keys' places among `n` luminances sorted ascending.
pub(crate) fn key_ranks(n: usize) -> [usize; 2] {
    [SHADOWS.percentile, HIGHLIGHTS.percentile].map(|q| ((n - 1) as f32 * q) as usize)
}
/// Map cells per camera-image pixel, for a `size` map of a `source`-sized photo.
pub(crate) fn scale(size: [u32; 2], source: [u32; 2]) -> [f32; 2] {
    [
        size[0] as f32 / source[0] as f32,
        size[1] as f32 / source[1] as f32,
    ]
}
/// The measured positions a slider is bracketed in, as `Curve::new` builds them: each
/// position with its table, and the identity at 0 in slot `IDENTITY`.
type Points<'a> = [(f32, Option<&'a [f32; 48]>); SLIDER_VALUES.len() + 1];
/// Slot the identity takes, after the negative slider positions.
const IDENTITY: usize = 3;

/// A family's log2 gain at slider `s` and base level `base`, as `Curve::new(..).eval`
/// without building the table.
fn family(f: &Family, s: f32, key: f32, base: f32) -> f32 {
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
            (SLIDER_VALUES[i], Some(&f.tables[i]))
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
/// The measured families for `develop.wgsl`'s local Shadows/Highlights: per family
/// its 6 × 48 table values, then `lo` and `hi`; then the slider positions.
pub(crate) fn gpu_families() -> Vec<f32> {
    let mut out = Vec::new();
    for f in [&SHADOWS, &HIGHLIGHTS] {
        out.extend(f.tables.iter().flatten());
        out.extend([f.lo, f.hi]);
    }
    out.extend(SLIDER_VALUES);
    out
}
/// Mean over a (2r+1)² window, clamped at the borders, via running sums: along rows,
/// then along columns. Each line keeps its own `f64` running sum, added to in the same
/// order whatever the parallelism; the columns are swept in blocks, row by row, so
/// they read memory in order.
pub(super) fn blur(x: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let n = (2 * r + 1) as f64;
    // The running sum of a line read through `get`, as each line starts it.
    let start = |len: usize, get: &dyn Fn(usize) -> f64| -> f64 {
        (-(r as isize)..=r as isize)
            .map(|i| get(i.clamp(0, len as isize - 1) as usize))
            .sum()
    };
    let mut rows = vec![0f32; x.len()];
    rows.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let line = &x[y * w..y * w + w];
        let get = |i: isize| line[i.clamp(0, w as isize - 1) as usize] as f64;
        let mut sum = start(w, &|i| line[i] as f64);
        for (i, v) in out.iter_mut().enumerate() {
            *v = (sum / n) as f32;
            sum += get(i as isize + r as isize + 1) - get(i as isize - r as isize);
        }
    });
    const BLOCK: usize = 64;
    let mut out = vec![0f32; x.len()];
    let blocks: Vec<(usize, Vec<f32>)> = (0..w.div_ceil(BLOCK))
        .into_par_iter()
        .map(|b| {
            let (c0, c1) = (b * BLOCK, ((b + 1) * BLOCK).min(w));
            let get = |y: isize, c: usize| rows[y.clamp(0, h as isize - 1) as usize * w + c] as f64;
            let mut sums: Vec<f64> = (c0..c1)
                .map(|c| start(h, &|y| rows[y * w + c] as f64))
                .collect();
            let mut block = vec![0f32; (c1 - c0) * h];
            for y in 0..h {
                for (k, sum) in sums.iter_mut().enumerate() {
                    block[y * (c1 - c0) + k] = (*sum / n) as f32;
                    *sum += get(y as isize + r as isize + 1, c0 + k)
                        - get(y as isize - r as isize, c0 + k);
                }
            }
            (c0, block)
        })
        .collect();
    for (c0, block) in blocks {
        let width = block.len() / h.max(1);
        for y in 0..h {
            out[y * w + c0..y * w + c0 + width].copy_from_slice(&block[y * width..(y + 1) * width]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The blur as first written, line by line: the reference the faster one keeps to.
    fn reference_blur(x: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
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

    #[test]
    fn both_keys_are_the_percentiles_selected_on_their_own() {
        let lum: Vec<f32> = (0..5000)
            .map(|i| 1e-3 + ((i * 7919) % 997) as f32 / 300.)
            .collect();
        let alone = |q: f32| {
            let mut v = lum.clone();
            let k = ((v.len() - 1) as f32 * q) as usize;
            v.select_nth_unstable_by(k, f32::total_cmp);
            v[k].log2()
        };
        let base = MapBase::new(lum.clone(), [100, 50], [100, 50]);
        assert_eq!(
            base.keys,
            [alone(SHADOWS.percentile), alone(HIGHLIGHTS.percentile)]
        );
    }
    #[test]
    fn the_blur_keeps_the_line_by_line_arithmetic() {
        // Sizes around the column block, radii from one pixel to wider than the image.
        for (w, h, r) in [
            (1, 1, 1),
            (5, 3, 2),
            (64, 9, 4),
            (65, 40, 16),
            (341, 512, 16),
            (130, 7, 90),
        ] {
            let x: Vec<f32> = (0..w * h)
                .map(|i| ((i * 7919) % 1000) as f32 / 37. - 11.)
                .collect();
            assert!(
                blur(&x, w, h, r) == reference_blur(&x, w, h, r),
                "{w}x{h} r={r}"
            );
        }
    }
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
        let c = Curve::new(&SHADOWS, 0.6, -1.).unwrap();
        // Positive Shadows lifts dark bases and leaves the key level almost unchanged.
        assert!(c.eval(-7.) > 0.5);
        assert!(c.eval(-1.).abs() < 0.1);
        let half = Curve::new(&SHADOWS, 0.15, -1.).unwrap();
        let full = Curve::new(&SHADOWS, 0.3, -1.).unwrap();
        assert!((half.eval(-6.) - full.eval(-6.) / 2.).abs() < 1e-5);
        let h = Curve::new(&HIGHLIGHTS, -0.6, -3.).unwrap();
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
