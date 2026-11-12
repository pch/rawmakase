//! Shadows and Highlights in the scene tone stage: an edge-aware local tone operator
//! fitted to Camera Raw on scene values, before the white point and the profile's tone
//! curve (docs/scene-tone-stage.md). The base level is a guided filter of log2 scene
//! luminance, with a radius of 3.2% of the long edge and ε = 0.5 (log2 units squared).
//! The measured tables in `local_tone_data.rs` give the log2 gain for each base level
//! relative to an image key. Positive Shadows also reads a regional level, a much wider
//! edge-aware blur ([`REGION_SCALE`], [`REGION_RANGE`]): its gain is a table of the base
//! level less the regional level plus a table of the regional level less the key, so a
//! small dark area inside brighter surroundings is lifted less than a large one. The
//! levels and keys come from the photo's measurement copy, so the result does not
//! depend on the rendered region or preview size.
use super::local_tone_data::{Family, HIGHLIGHTS, SHADOWS, SHADOWS_DETAIL, SHADOWS_REGION};
use rayon::prelude::*;

/// Long edge of the reduced image the base level is computed on.
pub(crate) const MAP_EDGE: u32 = 512;
const RADIUS: f32 = 0.032;
const EPSILON: f32 = 0.5;
/// Radius, in map pixels, of the mean the Highlights tail reads, and how far above it
/// (in stops) the tail's level may sit.
const TAIL_RADIUS: usize = 2;
const TAIL_MARGIN: f32 = 0.5;
/// The darkest scene luminance the map tells apart (−14 stops).
pub(crate) const FLOOR: f32 = 6.103_515_6e-5;
/// The regional level positive Shadows reads: a bilateral blur of log2 luminance with a
/// spatial σ of this fraction of the long edge and a range σ of `REGION_RANGE` stops.
const REGION_SCALE: f32 = 0.25;
const REGION_RANGE: f32 = 4.;

impl Family {
    /// Row `row` continues past `hi` with the slope of its last 7 columns, per stop.
    /// After `monotone` every column step is at least -1 stop, so the output past
    /// `hi` still never decreases (at worst it is flat).
    pub(super) const fn with_tail(mut self, row: usize) -> Self {
        let step = (self.hi - self.lo) / 48.;
        self.tail[row] = (self.tables[row][47] - self.tables[row][40]) / (7. * step);
        self
    }
    /// How far past the last column's centre `base` is, in stops (0 before it).
    fn past_hi(&self, key: f32, base: f32) -> f32 {
        past_hi(self.lo, self.hi, key, base)
    }
    /// Project output log luminance (input + gain), not gain, onto nondecreasing
    /// values. Pool adjacent violators gives the least-squares correction, leaving
    /// every already ordered run alone unless it joins a violating neighbour.
    ///
    /// The unconstrained fits exclude clipped reference pixels and can turn back
    /// on themselves in poorly sampled tails. Do this once, at compile time, on
    /// the shared data: global curves, masks and GPU uploads must all use it.
    /// Linear interpolation of these rows (including the identity) preserves the
    /// constraint. A flat output is allowed; no unmeasured minimum contrast is added.
    pub(super) const fn monotone(mut self) -> Self {
        let step = (self.hi - self.lo) / 48.;
        let mut row = 0;
        while row < 6 {
            let mut sums = [0.; 48];
            let mut counts = [0_usize; 48];
            let mut blocks = 0;
            let mut i = 0;
            while i < 48 {
                sums[blocks] = self.tables[row][i] + i as f32 * step;
                counts[blocks] = 1;
                blocks += 1;
                while blocks > 1
                    && sums[blocks - 2] / counts[blocks - 2] as f32
                        > sums[blocks - 1] / counts[blocks - 1] as f32
                {
                    sums[blocks - 2] += sums[blocks - 1];
                    counts[blocks - 2] += counts[blocks - 1];
                    blocks -= 1;
                }
                i += 1;
            }
            i = 0;
            let mut block = 0;
            while block < blocks {
                let end = i + counts[block];
                while i < end {
                    // Preserve untouched bins bit for bit (especially identity rows).
                    if counts[block] > 1 {
                        self.tables[row][i] = sums[block] / counts[block] as f32 - i as f32 * step;
                    }
                    i += 1;
                }
                block += 1;
            }
            row += 1;
        }
        self
    }
}

pub(crate) struct LocalToneMap {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// What the map takes from the photo: the guided-filter coefficients (base =
    /// a · log2(Y) + b) and the regional level.
    base: std::sync::Arc<MapBase>,
    /// Source image size, to convert sample coordinates.
    pub(crate) scale: [f32; 2],
    pub(crate) shadows: Option<Curve>,
    pub(crate) highlights: Option<Curve>,
    /// Whether positive Shadows may read the regional level (the slider or masks).
    regional: bool,
    /// Positive Shadows' tables at the slider: of the base level less the regional
    /// level, and of the regional level less the Shadows key.
    pub(crate) lift: Option<[Curve; 2]>,
    /// Image keys of negative Shadows, Highlights and positive Shadows, for masks
    /// that evaluate them at their own slider values.
    pub(crate) keys: [f32; 3],
    /// Clarity's measured gains and luminance guidance on this grid (`clarity.rs`).
    pub(crate) clarity: Option<super::clarity::ClarityField>,
    /// Clarity's detail, when masks change Clarity and read it at their own amount.
    pub(crate) clarity_detail: Option<super::clarity::ClarityDetail>,
    /// The photo's global rule at full strength (`local_tone_rule`): positive Shadows
    /// (+100) and negative Highlights (−100), for masks that evaluate them at their
    /// own slider values. Built when either slider or a mask may need them.
    pub(crate) rule: [Option<Curve>; 2],
}
pub(crate) struct Curve {
    pub(crate) key: f32,
    pub(crate) lo: f32,
    pub(crate) hi: f32,
    pub(crate) table: [f32; 48],
    /// Log2 gain per stop past the last column (`Family::tail`).
    pub(crate) tail: f32,
}
/// How far `base` is past the last column's centre of a table over [lo, hi], in stops.
fn past_hi(lo: f32, hi: f32, key: f32, base: f32) -> f32 {
    (base - key - (hi - (hi - lo) / 96.)).max(0.)
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
        let tail_at = |v: f32, t: Option<&[f32; 48]>| {
            t.and_then(|_| family.values.iter().position(|x| *x == v))
                .map_or(0., |i| family.tail[i])
        };
        let tail = tail_at(s0, t0) + (tail_at(s1, t1) - tail_at(s0, t0)) * w;
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
            tail,
        })
    }
    fn eval(&self, base: f32) -> f32 {
        self.eval_t(base, base)
    }
    fn eval_t(&self, base: f32, tb: f32) -> f32 {
        let f = ((base - self.key - self.lo) / (self.hi - self.lo) * 48. - 0.5).clamp(0., 47.);
        let i = (f as usize).min(46);
        self.table[i]
            + (self.table[i + 1] - self.table[i]) * (f - i as f32)
            + self.tail * past_hi(self.lo, self.hi, self.key, tb)
    }
}
/// The slider values a map is built for: Shadows, Highlights and Clarity, whether
/// masks change Clarity, and whether it is for an HDR master.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Sliders {
    pub(crate) shadows: f32,
    pub(crate) highlights: f32,
    pub(crate) clarity: f32,
    pub(crate) mask_clarity: bool,
    /// Whether the map is for an HDR master.
    pub(crate) hdr: bool,
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
            hdr: r.hdr.is_active(),
        }
    }
}
/// What a map takes from the photo whatever the slider values: the measurement copy's
/// scene luminance and its log, the guided filter's coefficients and the keys, and
/// (made the first time a map reads them) positive Shadows' key offset and regional
/// level and Clarity's detail. It depends only on the copy and its scene luminance, so
/// renders that change only the sliders keep it (`scene_tone::Measured::map_base`).
pub(crate) struct MapBase {
    width: usize,
    height: usize,
    lum: Vec<f32>,
    logs: Vec<f32>,
    a: Vec<f32>,
    b: Vec<f32>,
    /// Mean of log2 luminance over a few map pixels (`TAIL_RADIUS`), then the largest of
    /// those means over the same reach, so the first pixels inside a large bright
    /// area read its inside rather than the dark outside.
    fine: Vec<f32>,
    shadow_key: f32,
    highlight_key: f32,
    clarity_key: f32,
    lift_offset: std::sync::OnceLock<f32>,
    region: std::sync::OnceLock<Vec<f32>>,
    detail: std::sync::OnceLock<super::clarity::ClarityDetail>,
    rule: std::sync::OnceLock<super::local_tone_rule::RuleStats>,
}
impl std::fmt::Debug for MapBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapBase")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}
impl MapBase {
    /// Of `small`, the measurement copy, with `scene` its samples' scene luminance.
    pub(crate) fn of(
        small: &crate::camera_data::CameraImage,
        scene: impl Fn([f32; 3]) -> f32 + Sync,
    ) -> Self {
        let lum: Vec<f32> = small.pixels.par_iter().map(|p| scene(*p)).collect();
        Self::new(lum, [small.width, small.height])
    }
    /// From scene luminance at `size` pixels.
    pub(crate) fn new(lum: Vec<f32>, size: [u32; 2]) -> Self {
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
        let fine = dilate(&blur(&logs, w, h, TAIL_RADIUS), w, h, TAIL_RADIUS);
        let (a, b) = rayon::join(|| mean(&a), || mean(&b));
        let average = logs.iter().map(|l| l.exp2() as f64).sum::<f64>() / logs.len() as f64;
        let highlight_key = 0.5 * percentile(0.75) + 0.5 * percentile(0.01);
        // Clarity's weights were fitted relative to the photo's brightest levels.
        let clarity_key = 0.25 * percentile(0.999) + 0.75 * percentile(0.99);
        Self {
            width: w,
            height: h,
            lum,
            logs,
            a,
            b,
            fine,
            shadow_key: (average as f32).log2(),
            highlight_key,
            clarity_key,
            lift_offset: Default::default(),
            region: Default::default(),
            detail: Default::default(),
            rule: Default::default(),
        }
    }
    /// The histogram statistics the global rule reads (of the base level).
    fn rule(&self) -> &super::local_tone_rule::RuleStats {
        self.rule.get_or_init(|| {
            let base: Vec<f32> = self
                .logs
                .iter()
                .zip(self.a.iter().zip(&self.b))
                .map(|(l, (a, b))| a * l + b)
                .collect();
            super::local_tone_rule::RuleStats::new(&self.logs, &base, self.width, self.height)
        })
    }
    /// The copy's scene luminance.
    pub(crate) fn lum(&self) -> &[f32] {
        &self.lum
    }
    /// Positive Shadows' key less the negative one: the photo's spatial scene context.
    fn lift_offset(&self) -> f32 {
        *self.lift_offset.get_or_init(|| {
            super::shadow_context::offset(&self.logs, self.shadow_key, self.width, self.height)
        })
    }
    /// The regional level positive Shadows reads.
    fn region(&self) -> &Vec<f32> {
        self.region.get_or_init(|| {
            let [k] = super::clarity::bilateral(
                &self.logs,
                self.width,
                self.height,
                [REGION_SCALE * self.width.max(self.height) as f32],
                REGION_RANGE,
            );
            k
        })
    }
    /// Clarity's detail.
    fn detail(&self) -> &super::clarity::ClarityDetail {
        self.detail.get_or_init(|| {
            let base: Vec<f32> = self
                .logs
                .iter()
                .zip(self.a.iter().zip(&self.b))
                .map(|(l, (a, b))| a * l + b)
                .collect();
            super::clarity::ClarityDetail::of(
                &self.logs,
                &base,
                self.width,
                self.height,
                self.clarity_key,
            )
        })
    }
}
/// A map's grids as the GPU reads them (`develop.wgsl`'s `grid`): the guided filter's
/// a, then b, then the fine mean, then (when positive Shadows reads it) the regional level. They depend
/// only on the map's base, which renders share while the sliders move, so the device
/// keeps them per base instead of receiving them with every render.
#[derive(Clone)]
pub(crate) struct MapGrid {
    base: std::sync::Arc<MapBase>,
    region: bool,
}
impl MapGrid {
    /// Whether `other` holds the same values.
    pub(crate) fn same(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.base, &other.base) && self.region == other.region
    }
    /// Cells per grid.
    pub(crate) fn cells(&self) -> usize {
        self.base.a.len()
    }
    /// Offset of the fine mean.
    pub(crate) fn fine_at(&self) -> usize {
        2 * self.cells()
    }
    /// Offset of the regional level, if present.
    pub(crate) fn region_at(&self) -> Option<usize> {
        self.region.then(|| 3 * self.cells())
    }
    /// The values, in order.
    pub(crate) fn values(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(4 * self.cells());
        v.extend(&self.base.a);
        v.extend(&self.base.b);
        v.extend(&self.base.fine);
        if self.region {
            v.extend(self.base.region());
        }
        v
    }
}
/// log2 of a scene luminance, as the map reads it.
pub(crate) fn level(y: f32) -> f32 {
    y.max(FLOOR).log2()
}
impl LocalToneMap {
    /// Whether a map (of the photo's measurement copy, `MapBase`) is built for
    /// `sliders`: when either slider or the measured Clarity is set, or masks change
    /// Clarity, or `local` (masks change Shadows or Highlights).
    pub(crate) fn needed(sliders: Sliders, local: bool) -> bool {
        sliders.shadows != 0.
            || sliders.highlights != 0.
            || sliders.clarity != 0.
            || sliders.mask_clarity
            || local
    }
    /// The map from scene luminance at `size` pixels of a `source`-sized photo; `local`
    /// when masks change Shadows or Highlights.
    #[cfg(test)]
    pub(crate) fn from_luminance(
        lum: Vec<f32>,
        size: [u32; 2],
        source: [u32; 2],
        sliders: Sliders,
        local: bool,
    ) -> Self {
        Self::from_base(
            &std::sync::Arc::new(MapBase::new(lum, size)),
            source,
            sliders,
            local,
        )
    }
    /// The map of `base` for a `source`-sized photo at these slider values.
    pub(crate) fn from_base(
        base: &std::sync::Arc<MapBase>,
        source: [u32; 2],
        sliders: Sliders,
        local: bool,
    ) -> Self {
        let (w, h) = (base.width, base.height);
        // Masks may evaluate either sign. Negative Shadows follows mean luminance;
        // positive Shadows also measures spatial scene context. Highlights keeps
        // its range key (docs/scene-tone-stage.md).
        let lift_key = base.shadow_key
            + if sliders.shadows > 0. || local {
                base.lift_offset()
            } else {
                0.
            };
        let keys = [base.shadow_key, base.highlight_key, lift_key];
        // An HDR master's Clarity is stronger beside light above white, for the global
        // slider and masks alike.
        let detail = (sliders.clarity != 0. || sliders.mask_clarity).then(|| {
            let d = base.detail();
            if sliders.hdr {
                std::borrow::Cow::Owned(d.scaled(&hdr_clarity_weights(base)))
            } else {
                std::borrow::Cow::Borrowed(d)
            }
        });
        let clarity = detail
            .as_ref()
            .filter(|_| sliders.clarity != 0.)
            .map(|d| d.field(sliders.clarity));
        // Positive Shadows and negative Highlights follow the photo's global rule
        // (`local_tone_rule`); the two-scale lift and the regional level are not read.
        let rule = if sliders.shadows > 0. || sliders.highlights < 0. || local {
            let stats = base.rule();
            [Some(stats.curve(true)), Some(stats.curve(false))]
        } else {
            [None, None]
        };
        let shadows = match &rule[0] {
            Some(unit) if sliders.shadows > 0. => {
                Some(super::local_tone_rule::at(unit, sliders.shadows))
            }
            _ => Curve::new(&SHADOWS, sliders.shadows, keys[0]),
        };
        let highlights = match &rule[1] {
            Some(unit) if sliders.highlights < 0. => {
                Some(super::local_tone_rule::at(unit, sliders.highlights))
            }
            _ => Curve::new(&HIGHLIGHTS, sliders.highlights, keys[1]),
        };
        Self {
            width: w,
            height: h,
            base: base.clone(),
            scale: [w as f32 / source[0] as f32, h as f32 / source[1] as f32],
            shadows,
            highlights,
            regional: false,
            lift: None,
            keys,
            clarity,
            clarity_detail: detail
                .filter(|_| sliders.mask_clarity)
                .map(std::borrow::Cow::into_owned),
            rule,
        }
    }
    /// Luminance gain for a pixel of scene luminance `lum` at camera-image sample
    /// position `x`, `y`. `clarity_lum` is the same pixel before mask colour and
    /// Exposure, matching the measurement copy that supplied Clarity's blur.
    pub(crate) fn gain(&self, x: f32, y: f32, lum: f32, clarity_lum: f32) -> f32 {
        let base = self.base(x, y, lum);
        let tb = self.tail_base(x, y, base);
        let mut ev = self.shadows.as_ref().map_or(0., |c| c.eval(base))
            + self.highlights.as_ref().map_or(0., |c| c.eval_t(base, tb));
        if let (Some([detail, region]), Some(k)) = (&self.lift, self.region()) {
            let k = self.bilinear(x, y, k);
            ev += detail.eval(base - k) + region.eval(k);
        }
        (ev + self.clarity(x, y, clarity_lum)).exp2()
    }
    /// As [`Self::gain`], with Shadows, Highlights and Clarity at the given slider
    /// values (a mask's added to the recipe's).
    pub(crate) fn gain_with(
        &self,
        x: f32,
        y: f32,
        lum: f32,
        clarity_lum: f32,
        sliders: [f32; 3],
    ) -> f32 {
        let base = self.base(x, y, lum);
        let clarity = match &self.clarity_detail {
            Some(d) => d.at(self.clarity_sample(x, y), sliders[2], level(clarity_lum)),
            None => self.clarity(x, y, clarity_lum),
        };
        let lift = self.region().map_or(0., |k| {
            let k = self.bilinear(x, y, k);
            family(&SHADOWS_DETAIL, sliders[0], 0., base - k)
                + family(&SHADOWS_REGION, sliders[0], self.keys[2], k)
        });
        let shadows = match &self.rule[0] {
            Some(unit) if sliders[0] > 0. => unit.eval(base) * sliders[0].min(1.),
            _ => family(&SHADOWS, sliders[0], self.keys[0], base),
        };
        let highlights = match &self.rule[1] {
            Some(unit) if sliders[1] < 0. => unit.eval(base) * (-sliders[1]).min(1.),
            _ => family_t(
                &HIGHLIGHTS,
                sliders[1],
                self.keys[1],
                base,
                self.tail_base(x, y, base),
            ),
        };
        (shadows + highlights + lift + clarity).exp2()
    }
    /// The regional level on this grid, when positive Shadows may read it.
    pub(crate) fn region(&self) -> Option<&[f32]> {
        self.regional.then(|| self.base.region().as_slice())
    }
    /// What the GPU reads of this map's grids (`MapGrid`).
    pub(crate) fn grid(&self) -> MapGrid {
        MapGrid {
            base: self.base.clone(),
            region: self.regional,
        }
    }
    /// The level Highlights' tail reads: the base, but no more than `TAIL_MARGIN` above
    /// the (dilated) mean of the few pixels around, so stripes and specks too fine to
    /// carry a pull of their own keep their detail (docs/scene-tone-stage.md).
    fn tail_base(&self, x: f32, y: f32, base: f32) -> f32 {
        base.min(self.bilinear(x, y, &self.base.fine) + TAIL_MARGIN)
    }
    fn base(&self, x: f32, y: f32, lum: f32) -> f32 {
        self.bilinear(x, y, &self.base.a) * level(lum) + self.bilinear(x, y, &self.base.b)
    }
    /// The measured Clarity's log2 gain at sample position `x`, `y`.
    fn clarity(&self, x: f32, y: f32, lum: f32) -> f32 {
        self.clarity
            .as_ref()
            .map_or(0., |c| c.at(self.clarity_sample(x, y), level(lum)))
    }
    /// The same four neighbours used by bilinear reconstruction.
    fn clarity_sample(&self, x: f32, y: f32) -> super::clarity::ClaritySample {
        let fx = ((x + 0.5) * self.scale[0] - 0.5).clamp(0., (self.width - 1) as f32);
        let fy = ((y + 0.5) * self.scale[1] - 0.5).clamp(0., (self.height - 1) as f32);
        let (ix, iy) = (fx as usize, fy as usize);
        let (jx, jy) = ((ix + 1).min(self.width - 1), (iy + 1).min(self.height - 1));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        super::clarity::ClaritySample {
            indices: [
                iy * self.width + ix,
                iy * self.width + jx,
                jy * self.width + ix,
                jy * self.width + jx,
            ],
            weights: [
                (1. - tx) * (1. - ty),
                tx * (1. - ty),
                (1. - tx) * ty,
                tx * ty,
            ],
        }
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
type Points<'a> = [(f32, Option<(&'a [f32; 48], f32)>); 7];
/// Slot the identity takes, after the negative slider positions.
const IDENTITY: usize = 3;

/// A family's log2 gain at slider `s` and base level `base`, as `Curve::new(..).eval`
/// without building the table.
pub(crate) fn family(f: &Family, s: f32, key: f32, base: f32) -> f32 {
    family_t(f, s, key, base, base)
}
pub(crate) fn family_t(f: &Family, s: f32, key: f32, base: f32, tb: f32) -> f32 {
    if s == 0. {
        return 0.;
    }
    let s = s.clamp(-1., 1.);
    let past = f.past_hi(key, tb);
    let bin = |(t, tail): (&[f32; 48], f32)| {
        let x = ((base - key - f.lo) / (f.hi - f.lo) * 48. - 0.5).clamp(0., 47.);
        let i = (x as usize).min(46);
        t[i] + (t[i + 1] - t[i]) * (x - i as f32) + tail * past
    };
    // On the stack: this bracket is built per pixel, for both families.
    let points: Points = std::array::from_fn(|slot| {
        if slot == IDENTITY {
            (0., None)
        } else {
            let i = if slot < IDENTITY { slot } else { slot - 1 };
            (f.values[i], Some((&f.tables[i], f.tail[i])))
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
/// The measured families for `develop.wgsl`: Shadows, Highlights, Dehaze and positive
/// Shadows' detail and region tables, each its 6 × 48 table values, `lo` and `hi`, its
/// six slider positions, then its six tails.
pub(crate) fn gpu_families() -> Vec<f32> {
    let mut out = Vec::new();
    for f in [
        &SHADOWS,
        &HIGHLIGHTS,
        &super::local_tone_data::DEHAZE,
        &SHADOWS_DETAIL,
        &SHADOWS_REGION,
    ] {
        out.extend(f.tables.iter().flatten());
        out.extend([f.lo, f.hi]);
        out.extend(f.values);
        out.extend(f.tail);
    }
    out
}
/// How much stronger an HDR master's Clarity is near light above white, the base level
/// (log2 scene luminance) that counts as that light, and how near (a fraction of the
/// long edge). Camera Raw 18.7's HDR Clarity departs from its SDR one only beside light
/// above white, where its local contrast is no longer compressed by white: there it
/// lifts the dark side and pulls the light side further (Clarity −50 on the Nikon Z 8
/// photo: −0.53 stop just under white against −0.26 in SDR). Fitted on the twelve
/// validation and holdout photos (docs/records/hdr-stage1-results.md#presence).
const HDR_CLARITY_NEAR_GAIN: f32 = 1.5;
const HDR_CLARITY_LIGHT: f32 = -1.;
const HDR_CLARITY_NEAR: f32 = 0.016;
/// Clarity's per-cell weight in an HDR master: [`HDR_CLARITY_NEAR_GAIN`] within
/// [`HDR_CLARITY_NEAR`] of light above [`HDR_CLARITY_LIGHT`], softened over half that
/// distance, and 1 elsewhere.
fn hdr_clarity_weights(base: &MapBase) -> Vec<f32> {
    let (w, h) = (base.width, base.height);
    let r = ((HDR_CLARITY_NEAR * w.max(h) as f32).round() as usize).max(1);
    let light: Vec<f32> = base
        .lum
        .iter()
        .map(|y| {
            if level(*y) > HDR_CLARITY_LIGHT {
                1.
            } else {
                0.
            }
        })
        .collect();
    let near = blur(&dilate(&light, w, h, r), w, h, (r / 2).max(1));
    near.into_iter()
        .map(|n| 1. + (HDR_CLARITY_NEAR_GAIN - 1.) * n)
        .collect()
}
/// The largest value over a (2r+1)² window, clamped at the borders.
fn dilate(x: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let (px, py) = (i % w, i / w);
                (0..=2 * r)
                    .map(|d| {
                        let d = d as isize - r as isize;
                        let (qx, qy) = if horizontal {
                            ((px as isize + d).clamp(0, w as isize - 1) as usize, py)
                        } else {
                            (px, (py as isize + d).clamp(0, h as isize - 1) as usize)
                        };
                        src[qy * w + qx]
                    })
                    .fold(f32::MIN, f32::max)
            })
            .collect()
    };
    pass(&pass(x, true), false)
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
    fn measured_curves_do_not_reverse_brightness() {
        let mut failures = Vec::new();
        let gpu = gpu_families();
        for (slot, (name, f)) in [
            ("Shadows", &SHADOWS),
            ("Highlights", &HIGHLIGHTS),
            ("Dehaze", &super::super::local_tone_data::DEHAZE),
            ("Shadows detail", &SHADOWS_DETAIL),
            ("Shadows region", &SHADOWS_REGION),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(&gpu[slot * 302..slot * 302 + 288], f.tables.as_flattened());
            assert_eq!(&gpu[slot * 302 + 296..slot * 302 + 302], &f.tail);
            for s in -100..=100 {
                let s = s as f32 / 100.;
                let curve = Curve::new(f, s, 0.);
                let output = |x: f32| x + family(f, s, 0., x);
                for i in -40..520 {
                    let x = f.lo + (f.hi - f.lo) * i as f32 / 480.;
                    let next = x + (f.hi - f.lo) / 480.;
                    assert!(output(x).is_finite());
                    assert!(
                        (family(f, s, 0., x) - curve.as_ref().map_or(0., |c| c.eval(x))).abs()
                            < 1e-5
                    );
                    if output(next) < output(x) - 1e-5 {
                        failures.push(format!("{name} at {s}: reversal near {x}"));
                        break;
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn highlights_do_not_solarize_a_bright_spot_in_a_dark_scene() {
        let w = 240;
        let lum: Vec<f32> = (0..w * w)
            .map(|i| {
                let r = (i % w) as f32 - 120.;
                let y = (i / w) as f32 - 120.;
                let radius = r.hypot(y);
                (-7. + 7. * ((45. - radius) / 25.).clamp(0., 1.)).exp2()
            })
            .collect();
        let m = LocalToneMap::from_luminance(
            lum.clone(),
            [w as u32; 2],
            [w as u32; 2],
            Sliders {
                highlights: -1.,
                ..Default::default()
            },
            true,
        );
        let out = |x: usize| {
            let y = lum[120 * w + x];
            let global = m.gain(x as f32, 120., y, y);
            let masked = m.gain_with(x as f32, 120., y, y, [0., -1., 0.]);
            assert!((global - masked).abs() < 1e-5);
            y * global
        };
        let centre = out(120);
        let rim = (121..165).map(out).fold(0., f32::max);
        // Camera Raw also has a modest edge lift on a clipped plateau: on this
        // probe the rim/centre ratio is 1.30. The broken fit produces 2.20.
        assert!(rim <= centre * 1.35, "centre {centre}, rim {rim}");
    }

    #[test]
    fn dilate_takes_the_largest_value_in_the_window() {
        let mut x = vec![0.; 7 * 5];
        x[2 * 7 + 3] = 1.;
        let d = dilate(&x, 7, 5, 1);
        let lit: Vec<usize> = (0..35).filter(|i| d[*i] == 1.).collect();
        assert_eq!(
            lit,
            [
                7 + 2,
                7 + 3,
                7 + 4,
                14 + 2,
                14 + 3,
                14 + 4,
                21 + 2,
                21 + 3,
                21 + 4
            ]
        );
        assert!(dilate(&[2.; 12], 4, 3, 2).iter().all(|v| *v == 2.));
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
        // Negative Shadows darkens dark bases and leaves the key level almost unchanged.
        let c = Curve::new(&SHADOWS, -0.5, -1.).unwrap();
        assert!(c.eval(-7.) < -0.3, "{}", c.eval(-7.));
        assert!(c.eval(-1.).abs() < 0.1);
        let half = Curve::new(&SHADOWS, -0.125, -1.).unwrap();
        let full = Curve::new(&SHADOWS, -0.25, -1.).unwrap();
        assert!((half.eval(-6.) - full.eval(-6.) / 2.).abs() < 1e-5);
        // Positive Shadows is the lift alone: the base family has no positive rows, and
        // the lift's families none for negative positions.
        assert!((-8..=4).all(|u| family(&SHADOWS, 0.7, 0., u as f32) == 0.));
        assert!((-8..=4).all(|u| family(&SHADOWS_DETAIL, -0.7, 0., u as f32) == 0.));
        assert!((-8..=4).all(|u| family(&SHADOWS_REGION, -0.7, 0., u as f32) == 0.));
        // Negative Highlights darkens bases above its key.
        let h = Curve::new(&HIGHLIGHTS, -0.5, -3.).unwrap();
        assert!(h.eval(0.) < -0.2);
        // Per-pixel evaluation matches the interpolated table.
        for base in [-8., -3., -1., 0.2, 4.5, 9.] {
            for (f, s, key) in [
                (&SHADOWS, -0.45, -1.),
                (&SHADOWS_DETAIL, 0.45, 0.),
                (&SHADOWS_REGION, 0.8, -1.),
                (&HIGHLIGHTS, -0.8, -3.),
            ] {
                let table = Curve::new(f, s, key).unwrap().eval(base);
                assert!((family(f, s, key, base) - table).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn negative_highlights_keeps_pulling_past_its_table() {
        // Camera Raw 18.7 pulls a flat object 8, 9 and 10 stops over a dark background by
        // 7.9, 9.1 and 10.2 stops at -100, past the table's top at 7 stops; the -100 row
        // continues with its last slope, the others hold their last column.
        let at = |s: f32, u: f32| family(&HIGHLIGHTS, s, 0., u);
        assert!(
            at(-1., 10.) < at(-1., 7.) - 2.,
            "{} {}",
            at(-1., 7.),
            at(-1., 10.)
        );
        assert_eq!(at(-0.5, 10.), at(-0.5, 7.));
        // Halfway between -100 and -50, half the tail.
        let tail = |s: f32| at(s, 10.) - at(s, 8.);
        assert!((tail(-0.75) - tail(-1.) / 2.).abs() < 1e-4);
        // The output still never decreases past the table.
        for s in [-1., -0.75] {
            for u in 70..140 {
                let u = u as f32 / 10.;
                assert!(
                    u + 0.1 + at(s, u + 0.1) >= u + at(s, u) - 1e-5,
                    "{s} at {u}"
                );
            }
        }
    }

    #[test]
    fn negative_highlights_pull_a_bright_block_in_a_dark_frame() {
        // A block 10 stops over a dark frame, flat or in 2-pixel stripes. The global
        // rule (`local_tone_rule`) pulls the block from its place at the top of the
        // photo's histogram; stripes are read through the base like the flat block.
        let w = 256;
        let block = |stripes: bool| -> Vec<f32> {
            (0..w * w)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    let inside = (88..168).contains(&x) && (88..168).contains(&y);
                    if inside && (!stripes || y / 2 % 2 == 0) {
                        1.
                    } else {
                        (-10_f32).exp2()
                    }
                })
                .collect()
        };
        let pull = |stripes: bool| {
            let lum = block(stripes);
            let m = LocalToneMap::from_luminance(
                lum,
                [w as u32; 2],
                [w as u32; 2],
                Sliders {
                    highlights: -1.,
                    ..Default::default()
                },
                false,
            );
            m.gain(128., 128., 1., 1.).log2()
        };
        let (flat, striped) = (pull(false), pull(true));
        assert!(flat < -1.5, "flat {flat}");
        assert!(
            (striped - flat).abs() < 2.,
            "flat {flat}, striped {striped}"
        );
    }

    #[test]
    fn an_hdr_masters_clarity_is_stronger_beside_light_above_white() {
        // A frame 3 stops under white with a block 2 stops over it on the left: Clarity's
        // gain is the SDR map's far from the block, and up to HDR_CLARITY_NEAR_GAIN times
        // it beside the block (softened at the edge of the near zone).
        let (w, h) = (256usize, 128usize);
        let lum: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let texture = if (x / 3 + y / 3) % 2 == 0 { 1.1 } else { 0.9 };
                if (20..60).contains(&x) && (40..90).contains(&y) {
                    4.
                } else {
                    0.125 * texture
                }
            })
            .collect();
        let map = |hdr: bool| {
            LocalToneMap::from_luminance(
                lum.clone(),
                [w as u32, h as u32],
                [w as u32, h as u32],
                Sliders {
                    clarity: -0.5,
                    hdr,
                    ..Default::default()
                },
                false,
            )
        };
        let (sdr, hdr) = (map(false), map(true));
        let at = |m: &LocalToneMap, x: usize, y: usize| {
            let l = lum[y * w + x];
            m.gain(x as f32, y as f32, l, l).log2()
        };
        // Far from the block (more than the near distance plus its softening).
        assert_eq!(at(&hdr, 230, 64), at(&sdr, 230, 64));
        // Just beside it, where its local contrast moves the gain.
        let (s, d) = (at(&sdr, 62, 64), at(&hdr, 62, 64));
        assert!(s.abs() > 1e-3, "{s}");
        let ratio = d / s;
        assert!(
            ratio > 1.2 && ratio <= HDR_CLARITY_NEAR_GAIN + 1e-3,
            "{s} {d}"
        );
    }

    #[test]
    fn highlights_tail_pulls_the_edge_of_a_large_block_like_its_inside() {
        // The few pixels around an edge of the block include the dark frame, so their
        // plain mean is low; the tail must still pull the block's first pixels as far
        // as its inside, or the block gets a bright rim.
        let w = 256;
        let lum: Vec<f32> = (0..w * w)
            .map(|i| {
                let inside = (88..168).contains(&(i % w)) && (88..168).contains(&(i / w));
                if inside { 1. } else { (-10_f32).exp2() }
            })
            .collect();
        let m = LocalToneMap::from_luminance(
            lum,
            [w as u32; 2],
            [w as u32; 2],
            Sliders {
                highlights: -1.,
                ..Default::default()
            },
            false,
        );
        let at = |x: f32| m.gain(x, 128., 1., 1.).log2();
        let inside = at(128.);
        for x in [88., 89., 90.] {
            assert!(at(x) < inside + 1., "x {x}: {}, inside {inside}", at(x));
        }
    }

    /// A 240 × 160 map of log2 luminance -2.5 with ±0.3 stops of pixel noise, `dark`
    /// rectangles (x, y, width, height) 4 stops darker, as scene luminance.
    fn scene(dark: &[[usize; 4]]) -> Vec<f32> {
        let (w, h) = (240, 160);
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let noise = ((i as u32).wrapping_mul(2_654_435_761) >> 16) as f32 / 65536. - 0.5;
                let inside = dark.iter().any(|[dx, dy, dw, dh]| {
                    (*dx..dx + dw).contains(&x) && (*dy..dy + dh).contains(&y)
                });
                (-2.5 + 0.6 * noise - if inside { 4. } else { 0. }).exp2()
            })
            .collect()
    }
    fn map(lum: &[f32], shadows: f32, local: bool) -> LocalToneMap {
        let sliders = Sliders {
            shadows,
            ..Default::default()
        };
        LocalToneMap::from_luminance(lum.to_vec(), [240, 160], [240, 160], sliders, local)
    }

    #[test]
    fn positive_shadows_lifts_dark_areas_by_the_photo_rule() {
        // Dark areas 4 stops under a textured surround are lifted by the global rule
        // (`local_tone_rule`), the large one and the small one alike at the same level.
        let lum = scene(&[[20, 30, 100, 100], [190, 75, 8, 8]]);
        let m = map(&lum, 1., false);
        let at = |x: usize, y: usize| {
            m.gain(x as f32, y as f32, lum[y * 240 + x], lum[y * 240 + x])
                .log2()
        };
        let (large, small) = (at(70, 80), at(194, 79));
        assert!(large > 0.5 && small > 0.2, "large {large}, small {small}");
        // The surround is hardly changed.
        assert!(at(200, 20).abs() < 0.3, "{}", at(200, 20));
    }

    #[test]
    fn masks_read_the_same_shadows_as_the_slider() {
        // `gain_with` (masks) and `gain` (the global slider) are one operator.
        let lum = scene(&[[20, 30, 100, 100], [190, 75, 8, 8]]);
        for s in [-1., -0.3, 0.4, 1.] {
            let global = map(&lum, s, false);
            let masked = map(&lum, s, true);
            for (x, y) in [(70, 80), (194, 79), (200, 20), (5, 155)] {
                let l = lum[y * 240 + x];
                let (x, y) = (x as f32, y as f32);
                let a = global.gain(x, y, l, l);
                let b = masked.gain_with(x, y, l, l, [s, 0., 0.]);
                assert!((a - b).abs() < 1e-4 * a, "s {s}: {a} against {b}");
            }
        }
    }

    #[test]
    fn the_rule_is_built_for_positive_shadows_negative_highlights_and_masks() {
        let lum = scene(&[[20, 30, 100, 100]]);
        let negative = map(&lum, -0.6, false);
        assert!(negative.region().is_none() && negative.lift.is_none());
        assert!(negative.rule.iter().all(Option::is_none));
        let positive = map(&lum, 0.6, false);
        // The regional level and the two-scale lift are not read: the rule's curve is.
        assert!(positive.region().is_none() && positive.lift.is_none());
        assert!(positive.rule.iter().all(Option::is_some) && positive.shadows.is_some());
        // Masks may lift Shadows or pull Highlights anywhere, so the curves are there for them.
        assert!(map(&lum, -0.6, true).rule.iter().all(Option::is_some));
    }
}
