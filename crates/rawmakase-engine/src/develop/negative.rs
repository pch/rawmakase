//! Film negative conversion as rendered (`model::negative` holds the settings): a
//! scan of a negative turned into a scene-linear positive, first in the per-pixel
//! stage (`pipeline::scene_color`), before white balance. Everything after it (white
//! balance, the profile, the scene tone stage, curves, the mixer, grading, masks) sees
//! a positive and renders as for any photo.
//!
//! Per pixel, on the decoded camera values `v` (a rendered file's, its linear values:
//! see [`Inversion::apply`]):
//!
//! ```text
//! d   = log10(base / max(v, EPS))            density above the film base
//! d   = mean(d) in every channel             black & white film only
//! R   = mean(dmax - dmin) / FILM_GAMMA       the scene's range, in decades
//! out = 10 ^ ((d - dmax) / (dmax - dmin) · R)
//! ```
//!
//! so the frame's densest content (`dmax`, the scene's white) renders at 1.0 and its
//! thinnest (`dmin`) at 10^-R, unclamped. The range read back through the film's
//! contrast means a thin (underexposed or flat) negative gives a short scene range
//! rather than being stretched to a fixed one. A gain on `v` before the conversion
//! (the light, the scan's exposure) cancels, as `base` is measured in the same values
//! and scales alike.
//!
//! Wherever the camera image is reduced before the per-pixel stage (the measurement
//! copy, pyramid levels, resampling) it is averaged before the conversion: that
//! averages transmitted light, as a scan at a lower resolution would record it.
use crate::camera_data::{CameraImage, Metadata};
use crate::model::negative::{FilmType, MIN_DENSITY_RANGE, NegativeConversion};
use crate::model::recipe::Recipe;
use anyhow::{Result, bail, ensure};
use rawmakase_cancel::Cancel;

/// The film's contrast (gamma): density per decade of scene exposure along the
/// characteristic curve's straight line. A frame's scene range is its mean density
/// range divided by it.
///
/// Provisional: about 0.6 is typical of colour negative film, and close for black &
/// white film developed normally. A fixed scene range of two decades was tried first
/// and crushed thin camera-scanned black & white negatives. To be fitted per stock once
/// datasheet curves or a chart shot on film are available.
pub(crate) const FILM_GAMMA: f32 = 0.6;
/// The smallest camera value the conversion reads, so the logarithm stays finite: a
/// scan that recorded no light is the densest film there is.
const EPS: f32 = 1e-6;
/// The value the conversion's white (`dmax`) renders at, in every channel, before
/// white balance: the white point the scene tone stage follows for a converted
/// negative, in place of the sensor's clip (which is the film base's end).
pub(crate) const WHITE: f32 = 1.;
/// The share of the sensor's white (a camera value over the as-shot white balance,
/// `Metadata::wb`; a rendered file's linear value) from which a channel counts as
/// clipped: it records the light, not the film.
const CLIPPED: f32 = 0.98;

/// A conversion prepared for rendering: the per-render constants of a
/// [`NegativeConversion`], so each pixel costs a logarithm, a few multiplies and an
/// exponential per channel. Base-2 logarithms throughout, with the decimal constants
/// folded in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Inversion {
    /// Black & white film: one density, the channels' mean.
    black_white: bool,
    /// The photo is a rendered file, whose decoded values are its pseudo-scene
    /// (`develop::rendered`): its own linear values are converted, not those.
    rendered: bool,
    /// log2 of the film base.
    log_base: [f32; 3],
    /// `dmax` in base-2 units (`dmax · log2 10`).
    dmax: [f32; 3],
    /// R / (dmax − dmin): from density above `dmax` to the log of the output.
    stretch: [f32; 3],
}

impl Inversion {
    /// The recipe's conversion for a photo with `metadata`, prepared; `None` when it
    /// has none.
    pub(crate) fn of(r: &Recipe, metadata: &Metadata) -> Option<Self> {
        r.negative
            .as_ref()
            .map(|n| Self::new(n, metadata.is_rendered()))
    }

    pub(crate) fn new(n: &NegativeConversion, rendered: bool) -> Self {
        // Black & white film has one density, and so one pair of bounds: their mean,
        // should a recipe hold one per channel.
        let n = &match n.film {
            FilmType::BlackWhite => n.with_film(FilmType::BlackWhite),
            FilmType::Colour => n.clone(),
        };
        let log2_10 = std::f32::consts::LOG2_10;
        let decades = decades(n);
        Self {
            black_white: n.film == FilmType::BlackWhite,
            rendered,
            log_base: n.base.map(f32::log2),
            dmax: n.dmax.map(|d| d * log2_10),
            stretch: std::array::from_fn(|c| decades / (n.dmax[c] - n.dmin[c])),
        }
    }

    /// The positive of camera sample `v`: unclamped scene-linear values, the film's
    /// `dmax` at [`WHITE`].
    #[inline]
    pub(crate) fn apply(&self, v: [f32; 3]) -> [f32; 3] {
        let d = self.density_log2(v);
        std::array::from_fn(|c| ((d[c] - self.dmax[c]) * self.stretch[c]).exp2())
    }

    /// Density above the film base of camera sample `v`, in the recipe's decimal
    /// units: what `dmin` and `dmax` bound (black & white film: the mean, in every
    /// channel).
    pub(crate) fn density(&self, v: [f32; 3]) -> [f32; 3] {
        self.density_log2(v).map(|d| d / std::f32::consts::LOG2_10)
    }

    /// The values the film base is measured in: a camera sample as it is, or a
    /// rendered file's linear value.
    pub(crate) fn transmitted(&self, v: [f32; 3]) -> [f32; 3] {
        transmitted(v, self.rendered)
    }

    /// The constants the GPU port reads (`slot::NEGATIVE`): on, black & white, the base's
    /// log2, `dmax` in base-2 units and the stretch.
    pub(crate) fn gpu(&self) -> [f32; 11] {
        let mut out = [0.; 11];
        out[0] = 1.;
        out[1] = self.black_white as u8 as f32;
        out[2..5].copy_from_slice(&self.log_base);
        out[5..8].copy_from_slice(&self.dmax);
        out[8..11].copy_from_slice(&self.stretch);
        out
    }

    /// Whether the GPU port renders this conversion: a RAW's, not a rendered file's,
    /// whose linear values the port doesn't reconstruct.
    pub(crate) fn on_gpu(&self) -> bool {
        !self.rendered
    }

    #[inline]
    fn density_log2(&self, v: [f32; 3]) -> [f32; 3] {
        let v = self.transmitted(v);
        // `max` takes EPS for NaN too.
        let d: [f32; 3] = std::array::from_fn(|c| self.log_base[c] - v[c].max(EPS).log2());
        if self.black_white {
            [(d[0] + d[1] + d[2]) / 3.; 3]
        } else {
            d
        }
    }
}

/// The scene's range in decades that conversion `n` renders between its thin end
/// (10^-decades) and white: its mean density range through [`FILM_GAMMA`].
pub(crate) fn decades(n: &NegativeConversion) -> f32 {
    (0..3).map(|c| n.dmax[c] - n.dmin[c]).sum::<f32>() / 3. / FILM_GAMMA
}

/// The light the film let through at camera sample `v`: a RAW's camera values as they
/// are, a rendered file's linear values (Camera Raw's default rendering of its
/// pseudo-scene, which the decode inverted).
fn transmitted(v: [f32; 3], rendered: bool) -> [f32; 3] {
    match rendered {
        true => crate::develop::rendered::rendered_from_scene(v),
        false => v,
    }
}

/// Whether camera sample `v` of a photo with `m` has a channel at the sensor's clip
/// (or isn't a number), so it records the light rather than the film.
pub(super) fn clipped(v: [f32; 3], m: &Metadata) -> bool {
    let v = transmitted(v, m.is_rendered());
    let white = if m.is_rendered() {
        [1.; 3]
    } else {
        m.wb.map(|w| w.max(1e-3))
    };
    // Written so NaN counts as clipped.
    !(0..3).all(|c| v[c] / white[c] < CLIPPED)
}

/// Why the film base picker read no base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilmBaseRefusal {
    /// A channel of the patch is at the sensor's clip: the light around the film, or a
    /// scan exposed too brightly to measure its base.
    Clipped,
    /// The patch recorded no light in a channel.
    Dark,
}

/// Half the side of the film base picker's patch, in decoded pixels: 9 × 9 of them.
const PICK_REACH: i64 = 4;

/// The film base at (`u`, `v`) of the photo as rendered, mapped to the decoded image
/// as the white balance picker maps it ([`crate::develop::neutral_pick`]): the scan's
/// own values there, averaged over a 9 × 9 patch of whole pixels, before any
/// conversion (a rendered file's linear values). A patch with a clipped pixel, or a
/// channel without light, holds no film base and is refused.
pub fn film_base_pick(
    im: &CameraImage,
    r: &Recipe,
    u: f32,
    v: f32,
) -> std::result::Result<[f32; 3], FilmBaseRefusal> {
    let g = super::Geometry::new(im, r, 0);
    let [x, y] = g.source(u, v).map(|p| p.round() as i64);
    let rendered = im.metadata.is_rendered();
    let at = |x: i64, y: i64| {
        let x = x.clamp(0, im.width as i64 - 1) as usize;
        let y = y.clamp(0, im.height as i64 - 1) as usize;
        im.pixels[y * im.width as usize + x]
    };
    let mut sum = [0f64; 3];
    let mut taps = 0.;
    // Whole pixels, so a clipped one is seen as it is rather than blended away.
    for dy in -PICK_REACH..=PICK_REACH {
        for dx in -PICK_REACH..=PICK_REACH {
            let p = at(x + dx, y + dy);
            if clipped(p, &im.metadata) {
                return Err(FilmBaseRefusal::Clipped);
            }
            let p = transmitted(p, rendered);
            for c in 0..3 {
                sum[c] += p[c] as f64;
            }
            taps += 1.;
        }
    }
    let base = sum.map(|s| (s / taps) as f32);
    match base.iter().all(|b| *b > EPS) {
        true => Ok(base),
        false => Err(FilmBaseRefusal::Dark),
    }
}

/// The share of the crop's width and height Auto leaves out at each side: a scan
/// framed loosely shows the holder (no light) and the clear rebate (the bare base) at
/// its edges, which are no part of the photo.
const AUTO_INSET: f32 = 0.03;
/// The percentiles of the densities Auto takes as the thin and dense ends (0.5% and
/// 99.5%), so dust, scratches and specular points don't set them.
const AUTO_PERCENTILE: f32 = 0.005;
/// The fewest samples Auto measures a frame from.
const MIN_SAMPLES: usize = 64;

/// No-light pixels. The densest content of a frame (the scene's white) is where the
/// frame's own distribution of mean density ends; a sliver of the film holder or of
/// the camera's mask inside the crop recorded no light and sits apart from it, well
/// above, with only its blurred edge between. Auto looks for that gap among the
/// densest [`NO_LIGHT_SHARE`] of the samples: a run of nearly empty bins
/// [`NO_LIGHT_GAP`] decades long or longer. What lies above it is left out only where
/// it is a holder: a connected region that runs along the crop's edge, its outermost
/// samples, for [`HOLDER_RUN`] of that edge's length or more. A highlight of the
/// frame's own (a window, the sun), inside the frame or near or briefly touching its
/// edge, is kept. A fixed density cap can't tell them apart: a thin negative's white
/// is far below a dense one's.
const NO_LIGHT_SHARE: f32 = 0.1;
/// The width of a density bin when looking for the gap, in decades.
const NO_LIGHT_BIN: f32 = 0.02;
/// The shortest gap between the frame and a no-light cluster, in decades.
const NO_LIGHT_GAP: f32 = 0.3;
/// The most samples a bin of the gap may hold, as a share of all of them: a holder's
/// blurred edge spreads a few samples across it.
const NO_LIGHT_SPARSE: f32 = 0.0005;
/// The share of an edge of the crop a holder runs along, unbroken, at the least.
const HOLDER_RUN: f32 = 0.4;
/// How far a holder's blurred edge reaches into the frame, in grid cells: samples in
/// the gap this close to it are left out with it.
const HOLDER_EDGE: i64 = 2;
/// For a colour negative, the share of the frame, by mean density, in which Auto looks
/// for the dense end's near-neutral tones…
const DENSE_BAND: f32 = 0.02;
/// …the share of those, least coloured first, whose colour is the dense end's…
const NEUTRAL_SHARE: f32 = 0.25;
/// …and the largest spread between a sample's channels, in units of the channels'
/// ranges, for it to count as near neutral.
const MAX_NEUTRAL_SPREAD: f32 = 0.2;

/// A sample Auto measures: its cell in the grid over the crop, and its density above
/// the base in each channel.
#[derive(Clone, Copy, Debug)]
struct Density {
    cell: [u32; 2],
    d: [f32; 3],
}
impl Density {
    fn mean(&self) -> f32 {
        (self.d[0] + self.d[1] + self.d[2]) / 3.
    }
}

/// `base`'s film negative conversion with its density bounds (`dmin`, `dmax`) measured
/// from the frame: Auto in the Negative panel. The film type and base are kept.
///
/// - The scan is sampled inside the crop as Auto white balance samples it, on a reduced
///   copy of the decoded values, leaving out [`AUTO_INSET`] at each side and any
///   clipped sample, which records the light rather than the film.
/// - No-light samples, a holder's sliver inside the crop, are left out by the frame's
///   own distribution ([`NO_LIGHT_SHARE`]).
/// - The thin end (`dmin`) is the [`AUTO_PERCENTILE`] of each channel's density; so is
///   the dense end (`dmax`), unless a colour frame's densest tones hold near-neutral
///   ones. Then the dense end is in their colour, so a coloured highlight (denser in
///   one layer than any white) doesn't count as a cast.
/// - Black & white film has one density, so one pair of ends for every channel.
///
/// Errors on a photo without a conversion, a frame with too little unclipped film
/// inside the crop, and a blank or flat frame, whose levels would stretch its noise.
/// Stops with an error once `cancel` is set.
pub fn auto_negative(
    im: &CameraImage,
    base: &Recipe,
    cancel: Cancel<'_>,
) -> Result<NegativeConversion> {
    use super::auto::{WHITE_BALANCE_EDGE, analysis_edge, check_cancel, crop_grid};
    let Some(n) = &base.negative else {
        bail!("Choose the film type before Auto")
    };
    check_cancel(cancel)?;
    // The decoded values, not highlight-recovered ones: averaging them is what a
    // smaller scan records.
    let small = super::preview(im, analysis_edge(im, base, WHITE_BALANCE_EDGE));
    check_cancel(cancel)?;
    let inversion = Inversion::new(n, small.metadata.is_rendered());
    let grid = crop_grid(&small, base, AUTO_INSET);
    let samples: Vec<Density> = grid
        .samples
        .into_iter()
        .filter(|(_, p)| !clipped(*p, &small.metadata))
        .map(|(cell, p)| Density {
            cell,
            d: inversion.density(p),
        })
        .collect();
    ensure!(
        samples.len() >= MIN_SAMPLES,
        "Too little of the frame inside the crop is unclipped film to measure"
    );
    let frame = without_no_light(samples, [grid.columns, grid.rows]);
    let densities: Vec<[f32; 3]> = frame.iter().map(|s| s.d).collect();
    let (dmin, dmax) = match n.film {
        // Every channel holds the same density already.
        FilmType::BlackWhite => (
            percentiles(&densities, AUTO_PERCENTILE),
            percentiles(&densities, 1. - AUTO_PERCENTILE),
        ),
        FilmType::Colour => colour_ends(&densities),
    };
    ensure!(
        (0..3).all(|c| dmax[c] - dmin[c] >= MIN_DENSITY_RANGE),
        "The frame is too flat to level: pick the film base on the clear film edge, \
         and crop to the photo"
    );
    let out = NegativeConversion {
        dmin,
        dmax,
        ..n.clone()
    };
    out.validate()?;
    Ok(out)
}

/// `samples` (in a `size` grid of columns and rows) without a film holder's no-light
/// sliver at their dense end, when there is one (see [`NO_LIGHT_SHARE`]).
fn without_no_light(mut samples: Vec<Density>, size: [u32; 2]) -> Vec<Density> {
    samples.sort_unstable_by(|a, b| a.mean().total_cmp(&b.mean()));
    let n = samples.len();
    let first = ((n as f32 * (1. - NO_LIGHT_SHARE)) as usize).min(n - 1);
    let lo = samples[first].mean();
    let hi = samples[n - 1].mean();
    let bins = ((hi - lo) / NO_LIGHT_BIN).ceil() as usize + 1;
    let mut counts = vec![0usize; bins];
    for s in &samples[first..] {
        counts[((s.mean() - lo) / NO_LIGHT_BIN) as usize] += 1;
    }
    let sparse = ((n as f32 * NO_LIGHT_SPARSE) as usize).max(1);
    let needed = (NO_LIGHT_GAP / NO_LIGHT_BIN).ceil() as usize;
    // Each run of sparse bins long enough, densest content below it first.
    let mut run = 0;
    for (bin, count) in counts.iter().enumerate() {
        if *count > sparse {
            if run >= needed {
                let start = lo + (bin - run) as f32 * NO_LIGHT_BIN;
                let end = lo + bin as f32 * NO_LIGHT_BIN;
                let above = samples.partition_point(|s| s.mean() < end);
                let holder = holder_cells(&samples[above..], size);
                if !holder.is_empty() {
                    // The holder, and its blurred edge in the gap beside it.
                    let [columns, rows] = size.map(i64::from);
                    let mut near = vec![false; (columns * rows) as usize];
                    for [c, r] in holder {
                        for dr in -HOLDER_EDGE..=HOLDER_EDGE {
                            for dc in -HOLDER_EDGE..=HOLDER_EDGE {
                                let (c, r) = (i64::from(c) + dc, i64::from(r) + dr);
                                if (0..columns).contains(&c) && (0..rows).contains(&r) {
                                    near[(r * columns + c) as usize] = true;
                                }
                            }
                        }
                    }
                    samples.retain(|s| {
                        let at = i64::from(s.cell[1]) * columns + i64::from(s.cell[0]);
                        s.mean() < start || !near[at as usize]
                    });
                    return samples;
                }
            }
            run = 0;
        } else {
            run += 1;
        }
    }
    samples
}

/// The cells of `cluster` (in a `size` grid) that make up a film holder: each region
/// of them, connected side to side, that runs unbroken along one of the grid's
/// outermost rows or columns for [`HOLDER_RUN`] of its length or more.
fn holder_cells(cluster: &[Density], size: [u32; 2]) -> Vec<[u32; 2]> {
    let [columns, rows] = size.map(|v| v as usize);
    let mut marked = vec![false; columns * rows];
    for s in cluster {
        marked[s.cell[1] as usize * columns + s.cell[0] as usize] = true;
    }
    // Each marked cell's region, numbered from 1, found by flooding side to side.
    let mut region = vec![0usize; columns * rows];
    let mut regions: Vec<Vec<usize>> = Vec::new();
    for start in 0..columns * rows {
        if !marked[start] || region[start] != 0 {
            continue;
        }
        let id = regions.len() + 1;
        region[start] = id;
        let mut cells = vec![start];
        let mut i = 0;
        while i < cells.len() {
            let at = cells[i];
            let (c, r) = (at % columns, at / columns);
            let neighbours = [
                (c > 0).then(|| at - 1),
                (c + 1 < columns).then(|| at + 1),
                (r > 0).then(|| at - columns),
                (r + 1 < rows).then(|| at + columns),
            ];
            for next in neighbours.into_iter().flatten() {
                if marked[next] && region[next] == 0 {
                    region[next] = id;
                    cells.push(next);
                }
            }
            i += 1;
        }
        regions.push(cells);
    }
    // Whether region `id` runs unbroken along `line` for HOLDER_RUN of its `length`.
    let along = |line: &mut dyn Iterator<Item = usize>, length: usize, id: usize| {
        let (mut best, mut run) = (0usize, 0usize);
        for at in line {
            run = if region[at] == id { run + 1 } else { 0 };
            best = best.max(run);
        }
        best as f32 >= HOLDER_RUN * length as f32
    };
    let mut holder = Vec::new();
    for (index, cells) in regions.iter().enumerate() {
        let id = index + 1;
        let edge = along(&mut (0..columns), columns, id)
            || along(&mut ((rows - 1) * columns..rows * columns), columns, id)
            || along(&mut (0..rows).map(|r| r * columns), rows, id)
            || along(&mut (0..rows).map(|r| r * columns + columns - 1), rows, id);
        if edge {
            holder.extend(
                cells
                    .iter()
                    .map(|at| [(at % columns) as u32, (at / columns) as u32]),
            );
        }
    }
    holder
}

/// The thin and dense ends of a colour negative's densities (see [`auto_negative`]).
fn colour_ends(densities: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let lo = percentiles(densities, AUTO_PERCENTILE);
    let hi = percentiles(densities, 1. - AUTO_PERCENTILE);
    let range: [f32; 3] = std::array::from_fn(|c| (hi[c] - lo[c]).max(MIN_DENSITY_RANGE));
    // Each sample's level in each channel, 0 at the thin end and 1 at the dense one.
    let level = |d: &[f32; 3]| -> [f32; 3] { std::array::from_fn(|c| (d[c] - lo[c]) / range[c]) };
    let mean = |t: [f32; 3]| (t[0] + t[1] + t[2]) / 3.;
    let spread = |t: [f32; 3]| {
        t.iter().copied().fold(f32::MIN, f32::max) - t.iter().copied().fold(f32::MAX, f32::min)
    };
    let mut band: Vec<&[f32; 3]> = densities.iter().collect();
    band.sort_unstable_by(|a, b| mean(level(b)).total_cmp(&mean(level(a))));
    band.truncate(((densities.len() as f32 * DENSE_BAND).ceil() as usize).max(1));
    band.sort_unstable_by(|a, b| spread(level(a)).total_cmp(&spread(level(b))));
    band.truncate(((band.len() as f32 * NEUTRAL_SHARE).ceil() as usize).max(1));
    band.retain(|d| spread(level(d)) <= MAX_NEUTRAL_SPREAD);
    if band.is_empty() {
        return (lo, hi);
    }
    let neutral: [f32; 3] =
        std::array::from_fn(|c| band.iter().map(|d| d[c]).sum::<f32>() / band.len() as f32);
    let at = mean(level(&neutral));
    // A neutral this thin is no dense end: a frame of colours only.
    if at < 0.5 {
        return (lo, hi);
    }
    // From the thin end through the neutral's colour to the percentiles' mean level:
    // a grey's densities lie on a straight line from the thin end, so greys render
    // grey.
    (
        lo,
        std::array::from_fn(|c| lo[c] + (neutral[c] - lo[c]) / at),
    )
}

/// Each channel's `p` quantile (0–1) of `samples`, nearest rank.
fn percentiles(samples: &[[f32; 3]], p: f32) -> [f32; 3] {
    std::array::from_fn(|c| {
        let mut v: Vec<f32> = samples.iter().map(|s| s[c]).collect();
        if v.is_empty() {
            return 0.;
        }
        let k = ((v.len() - 1) as f32 * p).round() as usize;
        *v.select_nth_unstable_by(k, f32::total_cmp).1
    })
}

/// Synthetic negatives for tests: a forward film model from a scene to a camera scan.
#[cfg(test)]
pub(crate) mod synthetic {
    use super::*;

    /// Decades of the synthetic scene: from 10^-SCENE_DECADES to 1.
    pub(crate) const SCENE_DECADES: f32 = 2.;
    /// A film: its base (an orange mask for colour film), each dye layer's gamma and
    /// fog, as densities above the base.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Film {
        pub(crate) base: [f32; 3],
        pub(crate) gamma: [f32; 3],
        pub(crate) fog: [f32; 3],
    }
    /// A colour negative: an orange base, a different gamma per layer (their mean
    /// [`FILM_GAMMA`], so a grey ramp converts back to the scene) and a little fog.
    pub(crate) const COLOUR: Film = Film {
        base: [0.9, 0.55, 0.3],
        gamma: [0.64, 0.56, 0.6],
        fog: [0.08, 0.12, 0.15],
    };
    /// A black & white negative, on a slightly warm base.
    pub(crate) const BLACK_WHITE: Film = Film {
        base: [0.82, 0.78, 0.7],
        gamma: [FILM_GAMMA; 3],
        fog: [0.1; 3],
    };
    impl Film {
        /// The layers' densities above the base for scene exposure `e`: the
        /// characteristic curve's straight line, the scene's darkest tone at the fog.
        pub(crate) fn density(&self, e: [f32; 3]) -> [f32; 3] {
            std::array::from_fn(|c| {
                self.gamma[c] * (e[c].max(1e-9).log10() + SCENE_DECADES) + self.fog[c]
            })
        }
        /// The scan of the negative exposed to `e`, in camera values.
        pub(crate) fn scan(&self, e: [f32; 3]) -> [f32; 3] {
            let d = self.density(e);
            std::array::from_fn(|c| self.base[c] * 10f32.powf(-d[c]))
        }
        /// The conversion of `film` with this film's base and the densities of the
        /// scene's darkest and brightest greys as its bounds: what the picker and Auto
        /// would measure.
        pub(crate) fn conversion(&self, film: FilmType) -> NegativeConversion {
            NegativeConversion {
                film,
                base: self.base,
                dmin: self.density([10f32.powf(-SCENE_DECADES); 3]),
                dmax: self.density([1.; 3]),
            }
        }
        /// The scan of this film's negative of `positive`, whose values are scene
        /// exposures within the film's range.
        pub(crate) fn scanned(&self, positive: &CameraImage) -> CameraImage {
            let mut out = positive.clone();
            out.recovered = Default::default();
            out.pixels = positive.pixels.iter().map(|e| self.scan(*e)).collect();
            out
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
