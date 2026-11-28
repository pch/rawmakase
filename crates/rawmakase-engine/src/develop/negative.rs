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
/// Bounds on the base-2 exponent in [`Inversion::apply`], matching `develop.wgsl`:
/// keeps the scene-linear output finite (`[2^-30, 2^16]`) even on valid recipes with
/// a narrow channel density range against an unlit sample, without clipping normal
/// film densities or HDR highlights.
const MIN_EXPONENT: f32 = -30.;
const MAX_EXPONENT: f32 = 16.;
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

    /// The positive of camera sample `v`: scene-linear values with the film's `dmax`
    /// at [`WHITE`], bounded to `[2^-30, 2^16]` so valid narrow-range recipes stay finite.
    #[inline]
    pub(crate) fn apply(&self, v: [f32; 3]) -> [f32; 3] {
        let d = self.density_log2(v);
        std::array::from_fn(|c| {
            ((d[c] - self.dmax[c]) * self.stretch[c])
                .clamp(MIN_EXPONENT, MAX_EXPONENT)
                .exp2()
        })
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
    let v = of_white(v, m);
    // Written so NaN counts as clipped.
    !(0..3).all(|c| v[c] < CLIPPED)
}

/// Camera sample `v` of a photo with `m` as each channel's share of the sensor's
/// white: a camera value over the as-shot white balance (`Metadata::wb`), a rendered
/// file's linear value as it is.
fn of_white(v: [f32; 3], m: &Metadata) -> [f32; 3] {
    let v = transmitted(v, m.is_rendered());
    let white = if m.is_rendered() {
        [1.; 3]
    } else {
        m.wb.map(|w| w.max(1e-3))
    };
    std::array::from_fn(|c| v[c] / white[c])
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
/// as rendering maps it, through the geometry and any lens distortion correction
/// ([`crate::develop::ViewMapping`]): the scan's own values there, averaged over a 9 × 9 patch of whole pixels, before any
/// conversion (a rendered file's linear values). A patch with a clipped pixel, or a
/// channel without light, holds no film base and is refused.
pub fn film_base_pick(
    im: &CameraImage,
    r: &Recipe,
    u: f32,
    v: f32,
) -> std::result::Result<[f32; 3], FilmBaseRefusal> {
    let view = super::ViewMapping::new(im, r);
    let [x, y] = view
        .frame()
        .to_source(view.to_image(u, v))
        .map(|p| p.round() as i64);
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

/// The share of an edge of the crop an unlit strip runs along, unbroken, at the least,
/// for Auto to say the crop holds part of the film holder.
const HOLDER_RUN: f32 = 0.4;
/// The most light a sample lets through to count as unlit, as its brightest channel's
/// share of the sensor's white: 2^-8, eight stops below white. Camera scans of film in
/// a holder were measured to record 0.0005 to 0.002 of the white there (the lens's
/// flare and the black level's noise); film lets more through, unless it is very dense
/// over a dark base. It only decides whether Auto says the crop may hold part of the
/// holder: a needless note at worst, never other levels.
const NO_LIGHT_FLOOR: f32 = 1. / 256.;
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
    /// Its brightest channel's share of the sensor's white.
    light: f32,
}

/// What the Negative panel's Auto measured: the conversion with its levels, and
/// whether the crop seems to hold part of the film holder, which recorded no light and
/// reads as the densest film there is, so the levels come out too dark. Auto measures
/// it as part of the frame all the same: it only says so.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoLevels {
    pub conversion: NegativeConversion,
    pub holder_in_crop: bool,
}

/// `base`'s film negative conversion with its density bounds (`dmin`, `dmax`) measured
/// from the frame: Auto in the Negative panel. The film type and base are kept.
///
/// - The scan is sampled inside the crop as Auto white balance samples it, on a reduced
///   copy of the decoded values, leaving out [`AUTO_INSET`] at each side and any
///   clipped sample, which records the light rather than the film. Nothing else is
///   left out.
/// - The thin end (`dmin`) is the [`AUTO_PERCENTILE`] of each channel's density; so is
///   the dense end (`dmax`), unless a colour frame's densest tones hold near-neutral
///   ones. Then the dense end is in their colour, so a coloured highlight (denser in
///   one layer than any white) doesn't count as a cast.
/// - Black & white film has one density, so one pair of ends for every channel.
/// - An unlit strip along the crop's edge ([`holder_in_crop`]) is reported, not left
///   out: a scene's own dense areas can't be told from it reliably.
///
/// Errors on a photo without a conversion, a frame with too little unclipped film
/// inside the crop, and a blank or flat frame, whose levels would stretch its noise.
/// Stops with an error once `cancel` is set.
pub fn auto_negative(im: &CameraImage, base: &Recipe, cancel: Cancel<'_>) -> Result<AutoLevels> {
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
            light: of_white(p, &small.metadata).into_iter().fold(0., f32::max),
        })
        .collect();
    ensure!(
        samples.len() >= MIN_SAMPLES,
        "Too little of the frame inside the crop is unclipped film to measure"
    );
    let densities: Vec<[f32; 3]> = samples.iter().map(|s| s.d).collect();
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
    let conversion = NegativeConversion {
        dmin,
        dmax,
        ..n.clone()
    };
    conversion.validate()?;
    Ok(AutoLevels {
        conversion,
        holder_in_crop: holder_in_crop(&samples, [grid.columns, grid.rows]),
    })
}

/// Whether `samples` (in a `size` grid of columns and rows) hold an unlit strip along
/// the crop's edge, as a film holder inside the crop shows: samples that let through no
/// more light than [`NO_LIGHT_FLOOR`] running unbroken along one of the grid's
/// outermost rows or columns for [`HOLDER_RUN`] of its length or more.
fn holder_in_crop(samples: &[Density], size: [u32; 2]) -> bool {
    let [columns, rows] = size.map(|v| v as usize);
    let mut unlit = vec![false; columns * rows];
    for s in samples.iter().filter(|s| s.light <= NO_LIGHT_FLOOR) {
        unlit[s.cell[1] as usize * columns + s.cell[0] as usize] = true;
    }
    let along = |line: &mut dyn Iterator<Item = usize>, length: usize| {
        let (mut best, mut run) = (0usize, 0usize);
        for at in line {
            run = if unlit[at] { run + 1 } else { 0 };
            best = best.max(run);
        }
        best as f32 >= HOLDER_RUN * length as f32
    };
    along(&mut (0..columns), columns)
        || along(&mut ((rows - 1) * columns..rows * columns), columns)
        || along(&mut (0..rows).map(|r| r * columns), rows)
        || along(&mut (0..rows).map(|r| r * columns + columns - 1), rows)
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
