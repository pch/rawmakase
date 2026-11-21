//! Auto Align: each frame's homography to the reference frame, found by
//! direct photometric alignment, and the frame warped into the reference's
//! geometry.
//!
//! The two frames are compared as the log of their green, so the exposure
//! between them is one offset, fitted with the motion. Pixels clipped, near
//! clipping or near the noise floor take no part; the others weigh by the
//! inverse of their noise. The fit is inverse-compositional Gauss–Newton
//! (Lucas–Kanade) over a Gaussian pyramid, after an exhaustive search for the
//! shift on the coarsest level. Every step reweighs the pixels by Tukey's
//! biweight of their residual, so a moving object's pixels don't pull the
//! fit, and is lengthened or shortened by a line search on the robust cost.
//! A shift, a similarity and a homography are each fitted, and the identity;
//! [`align`] keeps the one that explains the frames best for its number of
//! parameters.
//!
//! Lens distortion is the same in every frame of a bracket, so it stays in the
//! data and is corrected at render time as for any raw; a homography is the
//! whole of the camera's motion between frames, up to parallax.
use crate::noise::Noise;
use rawmakase_cancel::Cancel;
use rawmakase_decode::raw::Frame;
use rayon::prelude::*;

/// A 3 × 3 homography, row by row, mapping homogeneous points (x, y, 1).
pub type Matrix = [[f64; 3]; 3];

pub(crate) const IDENTITY: Matrix = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];

/// The most a frame may move, at the image's centre and as a fraction of its
/// long edge, and turn, in degrees; more fails the alignment.
pub const MAX_SHIFT: f64 = 0.05;
pub const MAX_ROTATION: f64 = 5.;
/// The most any corner may move, as a fraction of the long edge: a
/// homography that skews or scales the frame this far is a wrong fit.
const MAX_CORNER: f64 = 0.1;

/// Pyramid levels are halved until their long edge is at most this many
/// pixels.
const COARSEST: usize = 200;
/// The most pixels a level is fitted on: the steepest of each cell of
/// [`CELL`] × [`CELL`], so every part of the frame takes part.
const MAX_POINTS: usize = 1 << 18;
const CELL: usize = 16;
/// A pixel takes part below this fraction of its colors' saturation (as the
/// exposure ratio's measurement, `ratio`) and this many standard deviations
/// of its noise above black.
const BRIGHTEST: f32 = 0.85;
const SIGNAL: f64 = 10.;
/// Tukey's biweight's tuning constant, in robust standard deviations.
const TUKEY: f64 = 4.685;
/// The robust standard deviation of the residual is at least this, in
/// natural-log units, so noiseless frames don't weigh one pixel alone.
const MIN_SIGMA: f64 = 1e-4;
/// The least variance a residual is taken to have, in natural-log units²:
/// a pixel is never surer than about a thousandth.
const MIN_VARIANCE: f32 = 1e-6;
/// What a model's parameter costs in its choice, in the units of
/// [`cost`] (about half a squared residual over its noise): the Bayesian
/// information criterion's half of ln N for a quarter of a million points,
/// four times over, as neighbouring pixels' noise is not independent.
const PENALTY: f64 = 25.;
/// How far off, in a level's pixels, a point's residual is taken as
/// misalignment rather than as something that moved.
const SLACK: f64 = 0.5;
/// The fewest pixels a step is fitted on.
const MIN_POINTS: usize = 500;
/// The least share of the matched pixels that agree with the fit: a fit most
/// pixels disagree with is a wrong one.
const MIN_INLIERS: f64 = 0.5;
/// How many of its noise's standard deviations a pixel's residual may be and
/// still agree with the fit. On four handheld brackets the residuals' robust
/// spread over their noise came out 1.05 to 2.4 (the demosaic, resampling and
/// what moved in the scene widen it), and 91% to 100% of the pixels agreed;
/// on another scene, or a frame mostly covered, few do.
const AGREES: f64 = 3. * TUKEY;
/// The most the residual's robust standard deviation may be, in stops. Every
/// pixel the fit uses is [`SIGNAL`] standard deviations above black, so the
/// noise of two such frames' difference is at most about 0.2 stop; more, and
/// the frames don't show the same thing in the same place.
const MAX_RESIDUAL: f64 = 0.5;
/// A level's steps, at most [`ITERATIONS`] ([`FULL_RESOLUTION_ITERATIONS`] at
/// full resolution, where each costs most), stop once one moves no corner
/// more than this, in that level's pixels…
const CONVERGED: f64 = 0.01;
const ITERATIONS: usize = 40;
const FULL_RESOLUTION_ITERATIONS: usize = 15;
/// … or lowers the robust cost by less than this share of it …
const STALLED: f64 = 1e-4;
/// … and the alignment fails when its last step at full resolution still
/// moved a corner this many pixels.
const NOT_CONVERGED: f64 = 0.5;
/// A fit that moves no point of the frame this many pixels is the identity:
/// a tripod bracket is merged as it was taken, without resampling.
const IDENTITY_SNAP: f64 = 0.05;

/// How many rows are warped between checks of the cancel flag.
const BAND: usize = 64;

/// A frame's alignment to the reference.
#[derive(Clone, Debug, PartialEq)]
pub struct Alignment {
    /// The homography from a reference pixel to the frame's, in pixel
    /// coordinates (a pixel's centre at integers), over the frame's sensor
    /// grid as decoded, before orientation.
    pub matrix: Matrix,
    /// How far the image's centre moved, in pixels, and the frame turned, in
    /// degrees, clockwise on the sensor grid as displayed (y points down).
    pub shift: [f64; 2],
    pub rotation: f64,
    /// The robust standard deviation of the difference that remains between
    /// the frames' green, in stops: noise and what moved between them.
    pub residual: f64,
    /// Whether the frame was resampled: false where the fit is the identity.
    pub warped: bool,
}

/// Why a frame couldn't be aligned.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Failure {
    Cancelled,
    /// Too few pixels both frames saw unclipped and above noise.
    TooFewPixels,
    /// The fit had not settled at full resolution: its last step, in pixels.
    NotConverged(f64),
    /// The fit moves the frame further than a bracket's frames move.
    TooFar {
        shift: f64,
        rotation: f64,
    },
    /// Most pixels disagree with the fit: the share that agree.
    Disagrees(f64),
    /// What remains after the best fit differs too much: its robust
    /// standard deviation, in stops.
    Differs(f64),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::TooFewPixels => f.write_str("too little of it is exposed well in both frames"),
            Self::NotConverged(step) => {
                write!(f, "the fit didn't settle (its last step {step:.2} px)")
            }
            Self::TooFar { shift, .. } if !shift.is_finite() => {
                f.write_str("the fit folds the frame over")
            }
            Self::TooFar { shift, rotation } => write!(
                f,
                "it moved {shift:.0} px and turned {rotation:.1}°, more than a bracket's frames do"
            ),
            Self::Differs(stops) => write!(
                f,
                "even its best fit leaves the frames {stops:.2} stop apart"
            ),
            Self::Disagrees(share) => write!(
                f,
                "only {:.0}% of its pixels agree with the best fit",
                share * 100.
            ),
        }
    }
}

/// One level of a pyramid: per pixel its log-luminance (NaN where the pixel
/// takes no part) and that's variance from the frame's noise.
struct Level {
    width: usize,
    height: usize,
    values: Vec<[f32; 2]>,
}

impl Level {
    fn at(&self, x: usize, y: usize) -> f32 {
        self.values[y * self.width + x][0]
    }

    /// The bilinear sample at `u`, `v` (pixel centres at integers) and its
    /// variance; `None` outside the level or where a pixel it reads takes no
    /// part.
    fn sample(&self, u: f64, v: f64) -> Option<(f32, f32)> {
        let (w, h) = (self.width, self.height);
        if !(u >= 0. && v >= 0. && u < (w - 1) as f64 && v < (h - 1) as f64) {
            return None;
        }
        let (x0, y0) = (u as usize, v as usize);
        let (fx, fy) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
        let i = y0 * w + x0;
        let (a, b, c, d) = (
            self.values[i],
            self.values[i + 1],
            self.values[i + w],
            self.values[i + w + 1],
        );
        let mix = |k: usize| {
            (a[k] * (1. - fx) + b[k] * fx) * (1. - fy) + (c[k] * (1. - fx) + d[k] * fx) * fy
        };
        let s = mix(0);
        // The variance as the pixels' own, mixed alike: a little more than
        // the mean's, which keeps a sample between pixels no surer than one
        // on them.
        s.is_finite().then(|| (s, mix(1)))
    }

    /// The next level: blurred by the binomial [1 3 3 1] / 8 each way and
    /// halved, the variance as the blur averages independent noise. A pixel
    /// whose footprint leaves the level or reads a pixel that takes no part
    /// takes none either.
    fn half(&self) -> Level {
        const K: [f32; 4] = [1., 3., 3., 1.];
        let (w, h) = (self.width / 2, self.height / 2);
        let mut values = vec![[f32::NAN, 0.]; w * h];
        values.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
            for (i, out) in row.iter_mut().enumerate() {
                let (x0, y0) = ((2 * i) as isize - 1, (2 * j) as isize - 1);
                if x0 < 0 || y0 < 0 || x0 as usize + 4 > self.width || y0 as usize + 4 > self.height
                {
                    continue;
                }
                let (mut sum, mut variance) = (0., 0.);
                for (b, kb) in K.iter().enumerate() {
                    for (a, ka) in K.iter().enumerate() {
                        let [v, var] =
                            self.values[(y0 as usize + b) * self.width + x0 as usize + a];
                        let k = ka * kb;
                        sum += k * v;
                        variance += k * k * var;
                    }
                }
                // NaN spreads through the sum.
                *out = [sum / 64., variance / 4096.];
            }
        });
        Level {
            width: w,
            height: h,
            values,
        }
    }
}

/// A frame prepared for alignment: its log-luminance from full resolution
/// down to the coarsest level.
pub(crate) struct Pyramid {
    levels: Vec<Level>,
}

impl Pyramid {
    /// The pyramid of a full-resolution log-luminance, NaN where a pixel
    /// takes no part, and its variance.
    pub(crate) fn new(width: usize, height: usize, values: Vec<[f32; 2]>) -> Self {
        assert_eq!(values.len(), width * height);
        let mut levels = vec![Level {
            width,
            height,
            values,
        }];
        loop {
            let last = levels.last().expect("one level");
            if last.width.max(last.height) <= COARSEST || last.width.min(last.height) < 32 {
                break;
            }
            let next = last.half();
            levels.push(next);
        }
        Self { levels }
    }

    /// The pyramid of `frame`, whose noise is `noise`: a pixel takes part
    /// when no color of it is clipped or above [`BRIGHTEST`] of its
    /// saturation, and its green is [`SIGNAL`] standard deviations above
    /// black.
    pub(crate) fn of_frame(frame: &Frame, noise: Noise) -> Self {
        let top = frame.saturation.map(|s| s * BRIGHTEST);
        let values = frame
            .pixels
            .par_iter()
            .zip(frame.clipped.par_iter())
            .map(|(p, clipped)| {
                if *clipped || (0..3).any(|c| p[c] >= top[c]) {
                    return [f32::NAN, 0.];
                }
                // Green, which a Bayer sensor samples on every row and
                // column: where the demosaic puts a red or blue edge depends
                // on its phase on the color filter, and a frame moved by an
                // odd number of pixels would see its edges moved by that.
                let y = f64::from(p[1]);
                let variance = noise.variance(y);
                if y > SIGNAL * variance.sqrt() {
                    // The log's variance: the relative variance.
                    [y.ln() as f32, (variance / (y * y)) as f32]
                } else {
                    [f32::NAN, 0.]
                }
            })
            .collect();
        Self::new(frame.width as usize, frame.height as usize, values)
    }
}

/// Full-resolution pixel coordinates against the normalised ones the fit
/// works in, centred on the image and 1 at half its long edge, the same at
/// every level.
#[derive(Clone, Copy)]
struct Geometry {
    cx: f64,
    cy: f64,
    c: f64,
    width: usize,
    height: usize,
}

impl Geometry {
    fn new(width: usize, height: usize) -> Self {
        Self {
            cx: (width as f64 - 1.) / 2.,
            cy: (height as f64 - 1.) / 2.,
            c: width.max(height) as f64 / 2.,
            width,
            height,
        }
    }

    /// A level's pixel, of a level `s` times smaller than full resolution, to
    /// normalised coordinates: level pixel `u` covers full-resolution pixels
    /// around `s · (u + 0.5) − 0.5`.
    fn normalised(&self, u: f64, v: f64, s: f64) -> (f64, f64) {
        (
            (s * (u + 0.5) - 0.5 - self.cx) / self.c,
            (s * (v + 0.5) - 0.5 - self.cy) / self.c,
        )
    }

    fn level(&self, x: f64, y: f64, s: f64) -> (f64, f64) {
        (
            (x * self.c + self.cx + 0.5) / s - 0.5,
            (y * self.c + self.cy + 0.5) / s - 0.5,
        )
    }

    /// The image's corners, normalised.
    fn corners(&self) -> [(f64, f64); 4] {
        let (x, y) = (self.cx / self.c, self.cy / self.c);
        [(-x, -y), (x, -y), (-x, y), (x, y)]
    }

    /// The most a homography `m` in normalised coordinates moves a corner,
    /// in full-resolution pixels.
    fn displacement(&self, m: &Matrix) -> f64 {
        self.corners()
            .into_iter()
            .chain([(0., 0.)])
            .map(|(x, y)| match apply(m, x, y) {
                Some((u, v)) => (u - x).hypot(v - y) * self.c,
                None => f64::INFINITY,
            })
            .fold(0., f64::max)
    }

    /// A homography in normalised coordinates as one in pixel coordinates.
    fn to_pixels(self, m: &Matrix) -> Matrix {
        let n = [
            [1. / self.c, 0., -self.cx / self.c],
            [0., 1. / self.c, -self.cy / self.c],
            [0., 0., 1.],
        ];
        let inverse = [[self.c, 0., self.cx], [0., self.c, self.cy], [0., 0., 1.]];
        multiply(&multiply(&inverse, m), &n)
    }
}

/// A pixel of the reference that the fit is made on: its normalised place,
/// its log-luminance, that's variance and its gradient in normalised
/// coordinates.
#[derive(Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
    t: f32,
    v: f32,
    gx: f32,
    gy: f32,
}

/// The reference frame, prepared once for every other frame's alignment:
/// the pixels of each level that the fit is made on, and its coarsest level
/// for the search.
pub(crate) struct Template {
    geometry: Geometry,
    points: Vec<Vec<Point>>,
    coarsest: Level,
}

impl Template {
    pub(crate) fn new(pyramid: Pyramid) -> Self {
        let full = &pyramid.levels[0];
        let geometry = Geometry::new(full.width, full.height);
        let points = pyramid
            .levels
            .iter()
            .enumerate()
            .map(|(l, level)| points(level, &geometry, f64::from(1u32 << l)))
            .collect();
        let coarsest = pyramid.levels.into_iter().last().expect("one level");
        Self {
            geometry,
            points,
            coarsest,
        }
    }
}

/// The gradient of `level` at `x`, `y`, in its pixels; `None` at its edge or
/// where the pixel or a neighbour takes no part.
///
/// Sobel's: the difference across, smoothed [1 2 1] / 4 along, so a noisy
/// level's gradients, which the fit's normal equations square, are less so:
/// noise in them shortens every Gauss–Newton step.
fn gradient(level: &Level, x: usize, y: usize) -> Option<(f32, f32, f32)> {
    if x == 0 || y == 0 || x + 1 >= level.width || y + 1 >= level.height {
        return None;
    }
    let t = level.at(x, y);
    let at = |dx: usize, dy: usize| level.at(x + dx - 1, y + dy - 1);
    let gx = ((at(2, 0) - at(0, 0)) + 2. * (at(2, 1) - at(0, 1)) + (at(2, 2) - at(0, 2))) / 8.;
    let gy = ((at(0, 2) - at(0, 0)) + 2. * (at(1, 2) - at(1, 0)) + (at(2, 2) - at(2, 0))) / 8.;
    (t.is_finite() && gx.is_finite() && gy.is_finite()).then_some((t, gx, gy))
}

/// The pixels of `level`, `s` times smaller than full resolution, that the
/// fit is made on: every one with a gradient, or, past [`MAX_POINTS`], the
/// steepest share of each cell.
fn points(level: &Level, geometry: &Geometry, s: f64) -> Vec<Point> {
    let (w, h) = (level.width, level.height);
    let steep = |x: usize, y: usize| gradient(level, x, y).filter(|g| g.1 != 0. || g.2 != 0.);
    let count: usize = (0..h)
        .into_par_iter()
        .map(|y| (0..w).filter(|&x| steep(x, y).is_some()).count())
        .sum();
    let share = (MAX_POINTS as f64 / count.max(1) as f64).min(1.);
    // Gradients from level pixels to normalised coordinates.
    let scale = (geometry.c / s) as f32;
    (0..h.div_ceil(CELL))
        .into_par_iter()
        .flat_map_iter(|cy| {
            let mut out = Vec::new();
            let mut cell = Vec::with_capacity(CELL * CELL);
            for cx in 0..w.div_ceil(CELL) {
                cell.clear();
                for y in cy * CELL..((cy + 1) * CELL).min(h) {
                    for x in cx * CELL..((cx + 1) * CELL).min(w) {
                        if let Some((t, gx, gy)) = steep(x, y) {
                            let (nx, ny) = geometry.normalised(x as f64, y as f64, s);
                            cell.push(Point {
                                x: nx as f32,
                                y: ny as f32,
                                t,
                                v: level.values[y * w + x][1],
                                gx: gx * scale,
                                gy: gy * scale,
                            });
                        }
                    }
                }
                if share < 1. {
                    let keep = (cell.len() as f64 * share).ceil() as usize;
                    cell.sort_by(|a, b| (b.gx.hypot(b.gy)).total_cmp(&a.gx.hypot(a.gy)));
                    cell.truncate(keep);
                }
                out.extend_from_slice(&cell);
            }
            out
        })
        .collect()
}

/// The frame `frame`'s alignment to `template`: the homography from a
/// reference pixel to the frame's. `offset` is the frame's exposure over the
/// reference's as far as its EXIF says, in natural-log units: where the fit
/// starts.
///
/// Each [`Model`] is fitted from coarse to fine, starting where the simpler
/// one ended, and the one kept is the one whose robust cost at full
/// resolution, plus [`PENALTY`] per parameter, is least: a richer model has
/// to explain more than noise to be kept, so a frame that only moved is not
/// given a turn or perspective that noise happens to favour. A frame of a
/// tripod bracket keeps the identity unless what differs between the frames
/// (their noise and demosaic) favours a fraction of a pixel's motion; a fit
/// that moves no corner [`IDENTITY_SNAP`] is the identity all the same.
pub(crate) fn align(
    template: &Template,
    frame: &Pyramid,
    cancel: Cancel<'_>,
) -> Result<Alignment, Failure> {
    let geometry = template.geometry;
    let top = frame.levels.len() - 1;
    if frame.levels.len() != template.points.len()
        || frame.levels[0].width != geometry.width
        || frame.levels[0].height != geometry.height
    {
        return Err(Failure::TooFewPixels);
    }
    let s = f64::from(1u32 << top);
    let ((dx, dy), b) = search(&template.coarsest, &frame.levels[top], &geometry, s)?;
    let fit = Fit {
        matrix: [
            [1., 0., dx * s / geometry.c],
            [0., 1., dy * s / geometry.c],
            [0., 0., 1.],
        ],
        offset: b,
    };
    let full = |fit: &mut Fit, model: Model| {
        iterate(
            fit,
            &template.points[0],
            &frame.levels[0],
            &geometry,
            0,
            model,
            cancel,
        )
    };
    let mut fits = models(fit, |fit, model| {
        for l in (1..=top).rev() {
            iterate(
                fit,
                &template.points[l],
                &frame.levels[l],
                &geometry,
                l,
                model,
                cancel,
            )?;
        }
        full(fit, model)
    })?;
    // The identity, its offset fitted.
    let mut identity = Fit {
        matrix: IDENTITY,
        offset: fits[0].1.offset,
    };
    match full(&mut identity, Model::Offset) {
        Ok(settled) => fits.insert(0, (Model::Offset, identity, settled)),
        Err(Failure::Cancelled) => return Err(Failure::Cancelled),
        Err(_) => {}
    }
    // Every model's cost at the richest one's scale of the residual.
    let reach = fits.last().expect("the shift's fit").2.reach;
    let mut best: Option<(f64, usize)> = None;
    for (k, (model, fit, _)) in fits.iter().enumerate() {
        let residuals = residuals(fit, &template.points[0], &frame.levels[0], &geometry, 1.);
        let cost = cost(&residuals, reach) + PENALTY * model.parameters() as f64;
        if best.is_none_or(|(least, _)| cost < least) {
            best = Some((cost, k));
        }
    }
    let (model, fit, last) = &fits[best.expect("the shift's fit").1];
    if last.moved > NOT_CONVERGED {
        return Err(Failure::NotConverged(last.moved));
    }
    let residual = last.sigma / std::f64::consts::LN_2;
    if last.inliers < MIN_INLIERS {
        return Err(Failure::Disagrees(last.inliers));
    }
    if residual > MAX_RESIDUAL {
        return Err(Failure::Differs(residual));
    }
    let matrix = geometry.to_pixels(&fit.matrix);
    let long = geometry.width.max(geometry.height) as f64;
    let (cx, cy) = (geometry.cx, geometry.cy);
    // A fit that takes the centre or its neighbours behind the horizon is
    // as far as a fit goes.
    let beyond = Failure::TooFar {
        shift: f64::INFINITY,
        rotation: 0.,
    };
    let (mx, my) = apply(&matrix, cx, cy).ok_or(beyond.clone())?;
    let shift = [mx - cx, my - cy];
    // The turn of the homography's linear part at the centre.
    let (ax, ay) = apply(&matrix, cx + 1., cy).ok_or(beyond.clone())?;
    let (bx, by) = apply(&matrix, cx, cy + 1.).ok_or(beyond)?;
    let j = [[ax - mx, bx - mx], [ay - my, by - my]];
    let rotation = (j[1][0] - j[0][1]).atan2(j[0][0] + j[1][1]).to_degrees();
    let corner = geometry.displacement(&fit.matrix);
    if shift[0].hypot(shift[1]) > MAX_SHIFT * long
        || rotation.abs() > MAX_ROTATION
        || corner > MAX_CORNER * long
    {
        return Err(Failure::TooFar {
            shift: shift[0].hypot(shift[1]),
            rotation,
        });
    }
    Ok(Alignment {
        matrix,
        shift,
        rotation,
        residual,
        warped: *model != Model::Offset && corner >= IDENTITY_SNAP,
    })
}

/// The fit so far: the homography in normalised coordinates and the
/// log-luminance offset of the frame over the reference.
#[derive(Clone)]
struct Fit {
    matrix: Matrix,
    offset: f64,
}

/// The shift, then the similarity, then the homography, each fitted by `one`
/// from where the last one fitted ended. A richer model that can't be fitted
/// is passed over: the simpler ones' fits stand without it.
fn models(
    mut fit: Fit,
    mut one: impl FnMut(&mut Fit, Model) -> Result<Settled, Failure>,
) -> Result<Vec<(Model, Fit, Settled)>, Failure> {
    let mut fits = Vec::with_capacity(4);
    for model in [Model::Shift, Model::Similarity, Model::Homography] {
        let mut next = fit.clone();
        match one(&mut next, model) {
            Ok(settled) => {
                fit = next;
                fits.push((model, fit.clone(), settled));
            }
            Err(why) if why == Failure::Cancelled || model == Model::Shift => return Err(why),
            Err(_) => {}
        }
    }
    Ok(fits)
}

/// What a step may change besides the offset.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Model {
    /// Nothing: the frame is where the reference is.
    Offset,
    Shift,
    /// A shift, a turn and a change of scale.
    Similarity,
    Homography,
}

impl Model {
    /// How many parameters it fits, the offset included.
    fn parameters(self) -> usize {
        match self {
            Self::Offset => 1,
            Self::Shift => 3,
            Self::Similarity => 5,
            Self::Homography => 9,
        }
    }

    /// The derivatives of the residual's model, `∇T · ∂W/∂p` and 1 for the
    /// offset, at the normalised place `x`, `y` with gradient `gx`, `gy`.
    fn row(self, x: f64, y: f64, gx: f64, gy: f64) -> [f64; 9] {
        let mut a = [0.; 9];
        match self {
            Self::Offset => a[0] = 1.,
            Self::Shift => a[..3].copy_from_slice(&[gx, gy, 1.]),
            Self::Similarity => {
                a[..5].copy_from_slice(&[gx * x + gy * y, gy * x - gx * y, gx, gy, 1.])
            }
            Self::Homography => {
                let r = gx * x + gy * y;
                a = [gx * x, gx * y, gx, gy * x, gy * y, gy, -r * x, -r * y, 1.];
            }
        }
        a
    }

    /// The warp of the step `d` and its offset.
    fn update(self, d: &[f64; 9]) -> (Matrix, f64) {
        match self {
            Self::Offset => (IDENTITY, d[0]),
            Self::Shift => ([[1., 0., d[0]], [0., 1., d[1]], [0., 0., 1.]], d[2]),
            Self::Similarity => (
                [
                    [1. + d[0], -d[1], d[2]],
                    [d[1], 1. + d[0], d[3]],
                    [0., 0., 1.],
                ],
                d[4],
            ),
            Self::Homography => (
                [
                    [1. + d[0], d[1], d[2]],
                    [d[3], 1. + d[4], d[5]],
                    [d[6], d[7], 1.],
                ],
                d[8],
            ),
        }
    }
}

/// What a level's last step found.
struct Settled {
    /// The most the step moved a corner, in full-resolution pixels.
    moved: f64,
    /// The robust standard deviation of the residual, in natural-log units.
    sigma: f64,
    /// Tukey's reach, in the residual's noise's standard deviations.
    reach: f64,
    /// The share of the matched pixels within [`AGREES`] standard
    /// deviations of their noise, or a [`SLACK`] misalignment at an edge.
    inliers: f64,
}

/// Each point's residual under `fit` on `frame` (a level `s` times smaller
/// than full resolution) and the variance of the noise in it; NaN where the
/// point lands outside the frame or on a pixel that takes no part.
fn residuals(
    fit: &Fit,
    points: &[Point],
    frame: &Level,
    geometry: &Geometry,
    s: f64,
) -> Vec<(f32, f32)> {
    let offset = fit.offset as f32;
    points
        .par_iter()
        .map(|p| {
            apply(&fit.matrix, f64::from(p.x), f64::from(p.y))
                .and_then(|(x, y)| {
                    let (u, v) = geometry.level(x, y, s);
                    frame.sample(u, v)
                })
                .map_or((f32::NAN, 0.), |(i, v)| (i - offset - p.t, p.v + v))
        })
        .collect()
}

/// The robust cost of `residuals`: Tukey's ρ of each over its noise, with
/// `reach`, about half its square near 0 and at most `reach² / 6`, which a
/// point that matched nothing costs too.
fn cost(residuals: &[(f32, f32)], reach: f64) -> f64 {
    let most = reach * reach / 6.;
    residuals
        .par_iter()
        .map(|&(e, v)| {
            if !e.is_finite() {
                return most;
            }
            let u = f64::from(e) / f64::from(v.max(MIN_VARIANCE)).sqrt() / reach;
            if u.abs() >= 1. {
                most
            } else {
                most * (1. - (1. - u * u).powi(3))
            }
        })
        .sum()
}

/// The integer shift of `frame` (the coarsest level, `s` times smaller than
/// full resolution) over `template` that leaves the least variance of their
/// difference, within [`MAX_SHIFT`] and a little more, and the offset there.
/// Shifts that leave less than half the best overlap are passed over. The
/// shift is refined to a fraction of a pixel.
fn search(
    template: &Level,
    frame: &Level,
    geometry: &Geometry,
    s: f64,
) -> Result<((f64, f64), f64), Failure> {
    let long = geometry.width.max(geometry.height) as f64;
    let reach = (MAX_SHIFT * long / s).ceil() as isize + 2;
    let (w, h) = (template.width as isize, template.height as isize);
    let shifts: Vec<(isize, isize)> = (-reach..=reach)
        .flat_map(|dy| (-reach..=reach).map(move |dx| (dx, dy)))
        .collect();
    let scores: Vec<(usize, f64, f64)> = shifts
        .par_iter()
        .map(|&(dx, dy)| {
            let (mut n, mut sum, mut squares) = (0usize, 0f64, 0f64);
            for y in 0.max(-dy)..h.min(h - dy) {
                for x in 0.max(-dx)..w.min(w - dx) {
                    let t = template.at(x as usize, y as usize);
                    let f = frame.at((x + dx) as usize, (y + dy) as usize);
                    let d = f64::from(f - t);
                    if d.is_finite() {
                        n += 1;
                        sum += d;
                        squares += d * d;
                    }
                }
            }
            if n == 0 {
                return (0, f64::INFINITY, 0.);
            }
            let mean = sum / n as f64;
            (n, squares / n as f64 - mean * mean, mean)
        })
        .collect();
    let most = scores.iter().map(|s| s.0).max().unwrap_or(0);
    if most < MIN_POINTS {
        return Err(Failure::TooFewPixels);
    }
    let (best, _) = scores
        .iter()
        .enumerate()
        .filter(|(_, s)| 2 * s.0 >= most)
        .min_by(|a, b| a.1.1.total_cmp(&b.1.1))
        .expect("the best overlap counts");
    let (dx, dy) = shifts[best];
    // To a fraction of a pixel: the parabola through the best and its
    // neighbours, each way.
    let side = 2 * reach + 1;
    let score = |dx: isize, dy: isize| {
        (dx.abs() <= reach && dy.abs() <= reach)
            .then(|| scores[((dy + reach) * side + dx + reach) as usize])
            .filter(|s| 2 * s.0 >= most)
            .map(|s| s.1)
    };
    let vertex = |before: Option<f64>, at: f64, after: Option<f64>| match (before, after) {
        (Some(a), Some(b)) if a + b - 2. * at > 0. => {
            ((a - b) / (2. * (a + b - 2. * at))).clamp(-0.5, 0.5)
        }
        _ => 0.,
    };
    let at = scores[best].1;
    let fx = vertex(score(dx - 1, dy), at, score(dx + 1, dy));
    let fy = vertex(score(dx, dy - 1), at, score(dx, dy + 1));
    Ok(((dx as f64 + fx, dy as f64 + fy), scores[best].2))
}

/// Gauss–Newton steps of `model` on level `l` until one moves no corner more
/// than [`CONVERGED`] of the level's pixels, or the iterations run out.
fn iterate(
    fit: &mut Fit,
    points: &[Point],
    frame: &Level,
    geometry: &Geometry,
    l: usize,
    model: Model,
    cancel: Cancel<'_>,
) -> Result<Settled, Failure> {
    let s = f64::from(1u32 << l);
    let iterations = if l == 0 {
        FULL_RESOLUTION_ITERATIONS
    } else {
        ITERATIONS
    };
    let mut settled = None;
    for _ in 0..iterations {
        if cancel.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        let step = step(fit, points, frame, geometry, s, model)?;
        let reach = step.settled.reach;
        let found = line_search(step.cost, |scale| {
            let (fit, moved) = stepped(fit, model, &step.delta, scale, geometry)?;
            let c = cost(&residuals(&fit, points, frame, geometry, s), reach);
            Ok(((fit, moved), c))
        })?;
        // Where no length of the step lowers the cost, the fit is as good
        // as this model makes it.
        let Some(((best, moved), least)) = found else {
            settled = Some(step.settled);
            break;
        };
        *fit = best;
        // Settled when the step is small, or when the cost has stopped
        // falling: where parallax leaves no one homography exact, the
        // weights hand pixels from one depth to another and the fit creeps
        // along a valley no lower than where it is.
        let done = moved < CONVERGED * s || step.cost - least < STALLED * step.cost;
        settled = Some(Settled {
            moved,
            ..step.settled
        });
        if done {
            break;
        }
    }
    Ok(settled.expect("one iteration"))
}

/// The step at full length, then longer while that lowers the cost: noise
/// in the gradients makes a Gauss–Newton step fall short, the same way each
/// time. Shorter if it raises the cost; `None` if no length lowers the cost
/// from `start`. `at` takes the step `scale` times over and gives what it
/// makes and its cost.
fn line_search<T>(
    start: f64,
    mut at: impl FnMut(f64) -> Result<(T, f64), Failure>,
) -> Result<Option<(T, f64)>, Failure> {
    let mut best = at(1.)?;
    let scales: &[f64] = if best.1 < start {
        &[2., 4.]
    } else {
        &[0.5, 0.25]
    };
    for &scale in scales {
        let other = at(scale)?;
        if other.1 >= best.1 {
            break;
        }
        best = other;
    }
    Ok((best.1 < start).then_some(best))
}

/// A Gauss–Newton step, not yet taken: its parameters, the robust cost
/// where it starts, and what was found there.
struct Step {
    delta: [f64; 9],
    cost: f64,
    settled: Settled,
}

/// One inverse-compositional Gauss–Newton step: the update that best explains
/// what remains, `∇T · ∂W/∂p · Δp + Δb = I(W(x; p)) − b − T(x)`, solved by
/// least squares under Tukey's weights, to be composed into `fit` as
/// `W(p) ∘ W(Δp)⁻¹` ([`stepped`]).
fn step(
    fit: &Fit,
    points: &[Point],
    frame: &Level,
    geometry: &Geometry,
    s: f64,
    model: Model,
) -> Result<Step, Failure> {
    let residuals = residuals(fit, points, frame, geometry, s);
    let matched_count = residuals.iter().filter(|r| r.0.is_finite()).count();
    if matched_count < MIN_POINTS {
        return Err(Failure::TooFewPixels);
    }
    // The residual's robust spread, in log units, which is reported, and
    // that of the residuals over their noise, which scales Tukey's reach.
    let spread = |f: &dyn Fn(f32, f32) -> f32| {
        // At most about 2^16 of them: a median needs no more.
        let every = (residuals.len() >> 16).max(1);
        let mut values: Vec<f32> = residuals
            .iter()
            .step_by(every)
            .filter(|r| r.0.is_finite())
            .map(|r| f(r.0, r.1))
            .collect();
        let centre = median(&mut values);
        for e in &mut values {
            *e = (*e - centre).abs();
        }
        1.4826 * f64::from(median(&mut values))
    };
    let sigma = spread(&|e, _| e).max(MIN_SIGMA);
    let reach = TUKEY * spread(&|e, v| e / v.max(MIN_VARIANCE).sqrt()).max(MIN_SIGMA);
    let n = model.parameters();
    // Normalised gradients to the level's pixels.
    let per_pixel = s / geometry.c;
    let (normal, rhs, inliers) = points
        .par_iter()
        .zip(residuals.par_iter())
        .fold(
            || ([0f64; 81], [0f64; 9], 0usize),
            |(mut normal, mut rhs, mut inliers), (p, &(e, v))| {
                let (e, v) = (f64::from(e), f64::from(v.max(MIN_VARIANCE)));
                let (gx, gy) = (f64::from(p.gx), f64::from(p.gy));
                // A residual that a misalignment of [`SLACK`] explains at
                // the point's gradient is not an outlier: an edge carries
                // the fit while it is still a little off, where smooth areas
                // that agree under any shift set a tight reach.
                let slack = gx.hypot(gy) * per_pixel * SLACK;
                let u = e / (v.sqrt() * reach).max(slack);
                // Agreement, though, is measured against the noise the
                // frames' noise model expects, not against the residuals'
                // own spread, which grows with what disagrees.
                if e.abs() < (v.sqrt() * AGREES).max(slack) {
                    inliers += 1;
                }
                if u.abs() < 1. {
                    // Tukey's weight over the noise's variance.
                    let w = (1. - u * u).powi(2) / v;
                    let (x, y) = (f64::from(p.x), f64::from(p.y));
                    let a = model.row(x, y, gx, gy);
                    for i in 0..n {
                        rhs[i] += w * a[i] * e;
                        for j in i..n {
                            normal[i * 9 + j] += w * a[i] * a[j];
                        }
                    }
                }
                (normal, rhs, inliers)
            },
        )
        .reduce(
            || ([0f64; 81], [0f64; 9], 0usize),
            |(mut a, mut b, c), (d, e, f)| {
                for i in 0..81 {
                    a[i] += d[i];
                }
                for i in 0..9 {
                    b[i] += e[i];
                }
                (a, b, c + f)
            },
        );
    let mut normal = normal;
    for i in 0..n {
        for j in 0..i {
            normal[i * 9 + j] = normal[j * 9 + i];
        }
    }
    let delta = solve(&normal, &rhs, n).ok_or(Failure::TooFewPixels)?;
    Ok(Step {
        delta,
        cost: cost(&residuals, reach),
        settled: Settled {
            moved: 0.,
            sigma,
            reach,
            inliers: inliers as f64 / matched_count as f64,
        },
    })
}

/// `fit` with the step `delta` of `model`, `scale` times over, composed in:
/// `W(p) ∘ W(scale · Δp)⁻¹`; and how far the step moved a corner, in
/// full-resolution pixels.
fn stepped(
    fit: &Fit,
    model: Model,
    delta: &[f64; 9],
    scale: f64,
    geometry: &Geometry,
) -> Result<(Fit, f64), Failure> {
    let (update, db) = model.update(&delta.map(|d| d * scale));
    let inverse = invert(&update).ok_or(Failure::NotConverged(f64::INFINITY))?;
    Ok((
        Fit {
            matrix: normalise(multiply(&fit.matrix, &inverse)),
            offset: fit.offset + db,
        },
        geometry.displacement(&update),
    ))
}

/// The median of `values`, 0 when there are none.
fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.;
    }
    let mid = values.len() / 2;
    *values.select_nth_unstable_by(mid, f32::total_cmp).1
}

/// The solution of the `n` × `n` system `a x = b` (row stride 9), by Gaussian
/// elimination with partial pivoting; `None` when it is singular.
fn solve(a: &[f64; 81], b: &[f64; 9], n: usize) -> Option<[f64; 9]> {
    let mut m = [[0f64; 10]; 9];
    for i in 0..n {
        m[i][..n].copy_from_slice(&a[i * 9..i * 9 + n]);
        m[i][n] = b[i];
    }
    let scale = (0..n).map(|i| m[i][i].abs()).fold(0., f64::max);
    if !(scale > 0. && scale.is_finite()) {
        return None;
    }
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| m[i][col].abs().total_cmp(&m[j][col].abs()))?;
        if m[pivot][col].abs() <= scale * 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        for row in col + 1..n {
            let f = m[row][col] / m[col][col];
            for k in col..=n {
                m[row][k] -= f * m[col][k];
            }
        }
    }
    let mut x = [0f64; 9];
    for row in (0..n).rev() {
        let mut v = m[row][n];
        for k in row + 1..n {
            v -= m[row][k] * x[k];
        }
        x[row] = v / m[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

pub(crate) fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

/// `m` scaled so its last entry is 1.
fn normalise(m: Matrix) -> Matrix {
    let k = m[2][2];
    if k == 0. || !k.is_finite() {
        return m;
    }
    m.map(|row| row.map(|v| v / k))
}

pub(crate) fn invert(m: &Matrix) -> Option<Matrix> {
    let c = |i: usize, j: usize| {
        let (r0, r1) = ((i + 1) % 3, (i + 2) % 3);
        let (c0, c1) = ((j + 1) % 3, (j + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    let det: f64 = (0..3).map(|j| m[0][j] * c(0, j)).sum();
    if det.abs() < 1e-300 || !det.is_finite() {
        return None;
    }
    Some(std::array::from_fn(|i| {
        std::array::from_fn(|j| c(j, i) / det)
    }))
}

/// Where `m` takes the point `x`, `y`; `None` at or behind the horizon.
pub(crate) fn apply(m: &Matrix, x: f64, y: f64) -> Option<(f64, f64)> {
    let w = m[2][0] * x + m[2][1] * y + m[2][2];
    if w.is_nan() || w <= 1e-9 {
        return None;
    }
    Some((
        (m[0][0] * x + m[0][1] * y + m[0][2]) / w,
        (m[1][0] * x + m[1][1] * y + m[1][2]) / w,
    ))
}

/// The weights of Catmull–Rom's cubic for the four samples around a point
/// `t` (0 to 1) past the second.
fn catmull_rom(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        (-t3 + 2. * t2 - t) / 2.,
        (3. * t3 - 5. * t2 + 2.) / 2.,
        (-3. * t3 + 4. * t2 + t) / 2.,
        (t3 - t2) / 2.,
    ]
}

/// `frame` warped into the reference's geometry by `matrix` (a reference
/// pixel to the frame's): each pixel the Catmull–Rom cubic of the 4 × 4 frame
/// pixels around where it lands, marked clipped when one of those it reads
/// is. Pixels that land outside the frame are not covered: black and marked
/// clipped. Also returns which pixels are covered.
pub(crate) fn warp(
    frame: &Frame,
    matrix: &Matrix,
    cancel: Cancel<'_>,
) -> Result<(Frame, Vec<bool>), crate::Error> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut pixels = vec![[0f32; 3]; w * h];
    let mut clipped = vec![true; w * h];
    let mut covered = vec![false; w * h];
    pixels
        .par_chunks_mut(w * BAND)
        .zip(clipped.par_chunks_mut(w * BAND))
        .zip(covered.par_chunks_mut(w * BAND))
        .enumerate()
        .try_for_each(|(band, ((pixels, clipped), covered))| {
            if cancel.is_cancelled() {
                return Err(crate::Error::Cancelled);
            }
            for (k, out) in pixels.iter_mut().enumerate() {
                let i = band * w * BAND + k;
                let (x, y) = ((i % w) as f64, (i / w) as f64);
                let Some((sx, sy)) = apply(matrix, x, y) else {
                    continue;
                };
                if !(sx >= 0. && sy >= 0. && sx <= (w - 1) as f64 && sy <= (h - 1) as f64) {
                    continue;
                }
                let (x0, y0) = (sx.floor(), sy.floor());
                let (wx, wy) = (catmull_rom((sx - x0) as f32), catmull_rom((sy - y0) as f32));
                let (x0, y0) = (x0 as isize, y0 as isize);
                let mut sum = [0f32; 3];
                let mut clip = false;
                for (b, wb) in wy.iter().enumerate() {
                    if *wb == 0. {
                        continue;
                    }
                    let row = (y0 - 1 + b as isize).clamp(0, h as isize - 1) as usize * w;
                    for (a, wa) in wx.iter().enumerate() {
                        if *wa == 0. {
                            continue;
                        }
                        let j = row + (x0 - 1 + a as isize).clamp(0, w as isize - 1) as usize;
                        let p = frame.pixels[j];
                        let weight = wa * wb;
                        for c in 0..3 {
                            sum[c] += weight * p[c];
                        }
                        clip |= frame.clipped[j];
                    }
                }
                *out = sum;
                clipped[k] = clip;
                covered[k] = true;
            }
            Ok(())
        })?;
    Ok((
        Frame {
            width: frame.width,
            height: frame.height,
            pixels,
            clipped,
            saturation: frame.saturation,
            metadata: frame.metadata.clone(),
        },
        covered,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 1536;
    const H: usize = 1024;

    /// A small deterministic generator.
    struct Random(u64);
    impl Random {
        fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
        fn normal(&mut self) -> f64 {
            let (a, b) = (self.uniform(), self.uniform());
            (-2. * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
        }
    }

    /// A scene's log-luminance as a function of the place, so a frame of it
    /// can be rendered exactly under any homography: waves of wavelengths
    /// from 12 to 400 pixels and a few soft edges.
    struct Scene {
        waves: Vec<(f64, f64, f64, f64)>,
        edges: Vec<(f64, f64, f64, f64)>,
    }

    impl Scene {
        fn new(seed: u64) -> Self {
            let mut r = Random(seed);
            let waves = (0..48)
                .map(|_| {
                    let length = 12. * (400f64 / 12.).powf(r.uniform());
                    let angle = r.uniform() * std::f64::consts::TAU;
                    let k = std::f64::consts::TAU / length;
                    (
                        k * angle.cos(),
                        k * angle.sin(),
                        r.uniform() * 6.3,
                        0.5 * (length / 400.).sqrt() * r.uniform(),
                    )
                })
                .collect();
            let edges = (0..12)
                .map(|_| {
                    let angle = r.uniform() * std::f64::consts::TAU;
                    (
                        angle.cos(),
                        angle.sin(),
                        r.uniform() * W as f64,
                        r.uniform() - 0.5,
                    )
                })
                .collect();
            Self { waves, edges }
        }

        fn at(&self, x: f64, y: f64) -> f64 {
            let waves: f64 = self
                .waves
                .iter()
                .map(|(kx, ky, phase, a)| a * (kx * x + ky * y + phase).sin())
                .sum();
            let edges: f64 = self
                .edges
                .iter()
                .map(|(nx, ny, d, a)| {
                    a * ((nx * (x - W as f64 / 2.) + ny * y - d / 2.) / 1.5).tanh()
                })
                .sum();
            waves + edges
        }
    }

    /// The log-luminance of the frame `truth` (a reference pixel to the
    /// frame's) makes of `scene`, `offset` brighter, with noise of `sigma`;
    /// `occlude` paints a block of another scene over it.
    fn render(
        scene: &Scene,
        truth: &Matrix,
        offset: f64,
        sigma: f64,
        seed: u64,
        occlude: Option<(usize, usize, usize)>,
    ) -> Vec<[f32; 2]> {
        let inverse = invert(truth).unwrap();
        let other = Scene::new(seed ^ 0x5555);
        (0..H)
            .into_par_iter()
            .flat_map_iter(|y| {
                let mut r = Random(seed.wrapping_mul(0x9E37_79B9) ^ ((y as u64 + 1) * 7919));
                let inverse = &inverse;
                let other = &other;
                (0..W).map(move |x| {
                    let (u, v) = apply(inverse, x as f64, y as f64).unwrap();
                    let mut value = scene.at(u, v) + offset;
                    if let Some((bx, by, side)) = occlude
                        && (bx..bx + side).contains(&x)
                        && (by..by + side).contains(&y)
                    {
                        value = other.at(x as f64, y as f64) + 1.;
                    }
                    [(value + sigma * r.normal()) as f32, (sigma * sigma) as f32]
                })
            })
            .collect()
    }

    /// A homography moving the image by `dx`, `dy` and turning it by `degrees`
    /// about its centre.
    fn motion(dx: f64, dy: f64, degrees: f64) -> Matrix {
        let (s, c) = degrees.to_radians().sin_cos();
        let (cx, cy) = ((W as f64 - 1.) / 2., (H as f64 - 1.) / 2.);
        let turn = [
            [c, -s, cx - c * cx + s * cy],
            [s, c, cy - s * cx - c * cy],
            [0., 0., 1.],
        ];
        multiply(&[[1., 0., dx], [0., 1., dy], [0., 0., 1.]], &turn)
    }

    /// The most `got` and `want` disagree on where a pixel lands, over a grid
    /// across the frame.
    fn error(got: &Matrix, want: &Matrix) -> f64 {
        let mut worst = 0f64;
        for y in (0..H).step_by(64) {
            for x in (0..W).step_by(64) {
                let a = apply(got, x as f64, y as f64).unwrap();
                let b = apply(want, x as f64, y as f64).unwrap();
                worst = worst.max((a.0 - b.0).hypot(a.1 - b.1));
            }
        }
        worst
    }

    fn template(scene: &Scene) -> Template {
        Template::new(Pyramid::new(
            W,
            H,
            render(scene, &IDENTITY, 0., 0.01, 1, None),
        ))
    }

    fn aligned(template: &Template, frame: Vec<[f32; 2]>) -> Result<Alignment, Failure> {
        align(template, &Pyramid::new(W, H, frame), Cancel::NEVER)
    }

    /// Shifts up to 60 pixels, turns up to 2°, a slight perspective and
    /// sub-pixel shifts are found to within 0.1 pixel anywhere in the frame,
    /// two stops apart.
    #[test]
    fn known_homographies_are_found_to_a_tenth_of_a_pixel() {
        let scene = Scene::new(3);
        let template = template(&scene);
        let mut perspective = motion(-20.3, 14.6, -1.2);
        perspective[2][0] = 2e-6;
        perspective[2][1] = -1.5e-6;
        for (k, truth) in [
            motion(60., 0., 0.),
            motion(-42.5, 37.25, 0.),
            motion(0.37, -0.21, 0.),
            motion(0., 0., 2.),
            motion(25.7, -31.2, -2.),
            perspective,
        ]
        .into_iter()
        .enumerate()
        {
            let frame = render(
                &scene,
                &truth,
                2. * std::f64::consts::LN_2,
                0.01,
                10 + k as u64,
                None,
            );
            let got = aligned(&template, frame).unwrap();
            let off = error(&got.matrix, &truth);
            assert!(off < 0.1, "case {k}: {off} px off, {got:?}");
            assert!(got.warped);
            assert!(got.residual < 0.03, "case {k}: {got:?}");
        }
        let got = aligned(
            &template,
            render(&scene, &motion(0., 0., 2.), 0., 0.01, 30, None),
        )
        .unwrap();
        assert!((got.rotation - 2.).abs() < 0.01, "{got:?}");
    }

    /// A frame of the same place, only its noise different, is the identity:
    /// it is not resampled.
    #[test]
    fn a_tripod_frame_is_the_identity() {
        let scene = Scene::new(4);
        let template = template(&scene);
        let got = aligned(&template, render(&scene, &IDENTITY, 0.7, 0.01, 7, None)).unwrap();
        assert!(!got.warped, "{got:?}");
        assert!(error(&got.matrix, &IDENTITY) < IDENTITY_SNAP, "{got:?}");
    }

    /// Something that moved between the frames, an eighth of the frame,
    /// doesn't pull the fit.
    #[test]
    fn a_moving_object_does_not_bias_the_fit() {
        let scene = Scene::new(5);
        let template = template(&scene);
        let truth = motion(12.4, -7.9, 0.6);
        let frame = render(&scene, &truth, 1., 0.01, 8, Some((300, 250, 450)));
        let got = aligned(&template, frame).unwrap();
        let off = error(&got.matrix, &truth);
        assert!(off < 0.1, "{off} px off, {got:?}");
    }

    /// Frames of another scene, or moved further than a bracket's frames
    /// move, fail rather than give a wrong fit.
    #[test]
    fn what_cannot_be_aligned_fails() {
        let scene = Scene::new(6);
        let template = template(&scene);
        let other = render(&Scene::new(99), &IDENTITY, 0., 0.01, 9, None);
        let got = aligned(&template, other);
        assert!(matches!(got, Err(Failure::Disagrees(_))), "{got:?}");
        let far = render(&scene, &motion(0., 0., 8.), 0., 0.01, 11, None);
        let got = aligned(&template, far);
        assert!(got.is_err(), "{got:?}");
        // Nothing exposed well enough.
        let empty = vec![[f32::NAN, 0.]; W * H];
        assert_eq!(aligned(&template, empty), Err(Failure::TooFewPixels));
    }

    /// A frame that something else covers for the most part fails: what the
    /// fit found on the rest is no reason to trust it.
    #[test]
    fn a_mostly_covered_frame_fails() {
        let scene = Scene::new(8);
        let template = template(&scene);
        let frame = render(
            &scene,
            &motion(5., -3., 0.),
            0.5,
            0.01,
            13,
            Some((100, 50, 1000)),
        );
        let got = aligned(&template, frame);
        assert!(matches!(got, Err(Failure::Disagrees(_))), "{got:?}");
    }

    /// A step that no scale makes cheaper is not taken.
    #[test]
    fn a_step_that_raises_the_cost_is_not_taken() {
        let mut tried = Vec::new();
        let got = line_search(1., |scale| {
            tried.push(scale);
            Ok((scale, 1. + scale))
        });
        assert_eq!(got, Ok(None));
        assert_eq!(tried, [1., 0.5, 0.25]);
        // One that does lower it is taken, as far as it keeps lowering it.
        let got = line_search(10., |scale| Ok((scale, (scale - 2.).abs())));
        assert_eq!(got, Ok(Some((2., 0.))));
    }

    /// A richer model that can't be fitted leaves the simpler ones' fits; a
    /// cancel, or a shift that can't be fitted, still ends the alignment.
    #[test]
    fn a_richer_model_that_fails_is_passed_over() {
        let start = Fit {
            matrix: IDENTITY,
            offset: 0.,
        };
        let settled = || Settled {
            moved: 0.,
            sigma: 0.01,
            reach: TUKEY,
            inliers: 1.,
        };
        let failing = |at: Model, why: Failure| {
            move |_: &mut Fit, model: Model| {
                if model == at {
                    Err(why.clone())
                } else {
                    Ok(settled())
                }
            }
        };
        let fits = models(
            start.clone(),
            failing(Model::Homography, Failure::TooFewPixels),
        )
        .unwrap();
        let kept: Vec<Model> = fits.iter().map(|f| f.0).collect();
        assert_eq!(kept, [Model::Shift, Model::Similarity]);
        let fits = models(
            start.clone(),
            failing(Model::Similarity, Failure::NotConverged(f64::INFINITY)),
        )
        .unwrap();
        let kept: Vec<Model> = fits.iter().map(|f| f.0).collect();
        assert_eq!(kept, [Model::Shift, Model::Homography]);
        assert!(matches!(
            models(
                start.clone(),
                failing(Model::Homography, Failure::Cancelled)
            ),
            Err(Failure::Cancelled)
        ));
        assert!(matches!(
            models(start, failing(Model::Shift, Failure::TooFewPixels)),
            Err(Failure::TooFewPixels)
        ));
    }

    #[test]
    fn a_cancel_stops_the_fit() {
        let scene = Scene::new(7);
        let template = template(&scene);
        let flag = std::sync::atomic::AtomicBool::new(true);
        let frame = Pyramid::new(
            W,
            H,
            render(&scene, &motion(3., 2., 0.), 0., 0.01, 12, None),
        );
        assert_eq!(
            align(&template, &frame, Cancel::new(&flag)),
            Err(Failure::Cancelled)
        );
    }

    #[test]
    fn matrices_invert_and_compose() {
        let m = motion(3., -4., 1.5);
        let back = multiply(&m, &invert(&m).unwrap());
        for (i, row) in back.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                assert!((v - IDENTITY[i][j]).abs() < 1e-12);
            }
        }
        let g = Geometry::new(W, H);
        let p = g.to_pixels(&IDENTITY);
        assert_eq!(
            apply(&p, 10., 20.).map(|(x, y)| (x.round(), y.round())),
            Some((10., 20.))
        );
    }
}
