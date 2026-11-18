//! Each frame's noise, measured on the frame itself: the variance of a sample
//! of level `s` (1 = white) is `shot · s + read`, photon shot noise plus read
//! noise. Measured on the demosaiced green, whose noise the demosaic lowers
//! by the same factor in every frame of a bracket, so the frames' weights
//! against one another stay right.
use rawmakase_decode::raw::Frame;

/// A frame's noise model, in its own units (1 = white).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Noise {
    pub shot: f64,
    pub read: f64,
}

impl Noise {
    /// The variance of a sample of level `s`.
    pub(crate) fn variance(&self, s: f64) -> f64 {
        self.shot * s.max(0.) + self.read
    }

    /// A rough model for a frame that has too few flat areas to measure:
    /// a sensor of about 20 000 electrons at ISO 100 and 2 electrons of read
    /// noise, both scaling with ISO speed.
    pub(crate) fn of_iso(iso: f64) -> Self {
        let gain = (iso.max(1.) / 100.).max(f64::MIN_POSITIVE);
        let electrons = 20_000. / gain;
        Self {
            shot: 1. / electrons,
            read: (2. / electrons).powi(2),
        }
    }
}

/// The side of the square tiles noise is measured in.
const TILE: usize = 16;
/// Tiles are put in bins half a stop wide by their level, from 2^-14 of white.
const BINS: usize = 28;
const DARKEST: f64 = 1. / 16384.;
/// The fewest tiles a bin needs to count.
const MIN_TILES: usize = 8;
/// A bin's noise is the variance this far up its tiles' (in quantile): the
/// flattest of them, which texture inflates least.
const QUANTILE: f64 = 0.2;

/// `frame`'s noise, measured from its flat tiles: in each, the variance of
/// the differences between a pixel and the next but one, robust to edges by
/// the median absolute deviation, against the tile's level; the line through
/// the flattest tiles of each level is the model. [`Noise::of_iso`] when the
/// frame has too few levels to fit a line through.
pub(crate) fn measure(frame: &Frame) -> Noise {
    use rayon::prelude::*;
    let (w, h) = (frame.width as usize, frame.height as usize);
    let fallback = Noise::of_iso(f64::from(frame.metadata.iso));
    if w < TILE * 4 || h < TILE * 4 {
        return fallback;
    }
    let tiles: Vec<(f64, f64)> = (0..h / TILE)
        .into_par_iter()
        .flat_map_iter(|ty| (0..w / TILE).filter_map(move |tx| tile(frame, tx * TILE, ty * TILE)))
        .collect();
    let mut bins: Vec<Vec<(f64, f64)>> = vec![Vec::new(); BINS];
    for (level, variance) in tiles {
        let bin = ((level / DARKEST).log2() * 2.).floor();
        if bin >= 0. && (bin as usize) < BINS {
            bins[bin as usize].push((level, variance));
        }
    }
    let points: Vec<(f64, f64, f64)> = bins
        .into_iter()
        .filter(|b| b.len() >= MIN_TILES)
        .map(|mut b| {
            b.sort_by(|x, y| x.1.total_cmp(&y.1));
            let (level, variance) = b[((b.len() - 1) as f64 * QUANTILE) as usize];
            (level, variance, b.len() as f64)
        })
        .collect();
    fit(&points).unwrap_or(fallback)
}

/// The level of the tile at `x`, `y` and the variance of its noise; `None`
/// for a tile with a clipped pixel.
fn tile(frame: &Frame, x: usize, y: usize) -> Option<(f64, f64)> {
    let w = frame.width as usize;
    let mut sum = 0.;
    let mut differences = Vec::with_capacity(TILE * (TILE - 2));
    for row in y..y + TILE {
        for col in x..x + TILE {
            let i = row * w + col;
            if frame.clipped[i] {
                return None;
            }
            let g = f64::from(frame.pixels[i][1]);
            sum += g;
            if col + 2 < x + TILE {
                differences.push(g - f64::from(frame.pixels[i + 2][1]));
            }
        }
    }
    let level = sum / (TILE * TILE) as f64;
    let mid = differences.len() / 2;
    differences.select_nth_unstable_by(mid, f64::total_cmp);
    let median = differences[mid];
    let mut deviations: Vec<f64> = differences.iter().map(|d| (d - median).abs()).collect();
    deviations.select_nth_unstable_by(mid, f64::total_cmp);
    // A difference of two samples has twice a sample's variance; 1.4826 MAD
    // is a normal distribution's standard deviation.
    let sigma = 1.4826 * deviations[mid];
    Some((level, sigma * sigma / 2.))
}

/// The line `variance = shot · level + read` through `points` (level,
/// variance, weight) by weighted least squares, with neither term negative;
/// `None` through fewer than three points or with no shot noise.
fn fit(points: &[(f64, f64, f64)]) -> Option<Noise> {
    if points.len() < 3 {
        return None;
    }
    // Relative errors: each point weighed by its count over its variance².
    let weight = |(_, v, n): &(f64, f64, f64)| n / (v * v).max(f64::MIN_POSITIVE);
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0., 0., 0., 0., 0.);
    for p in points {
        let w = weight(p);
        sw += w;
        sx += w * p.0;
        sy += w * p.1;
        sxx += w * p.0 * p.0;
        sxy += w * p.0 * p.1;
    }
    let det = sw * sxx - sx * sx;
    if det <= 0. {
        return None;
    }
    let mut shot = (sw * sxy - sx * sy) / det;
    let mut read = (sy - shot * sx) / sw;
    if read < 0. {
        // Through the origin.
        read = 0.;
        shot = sxy / sxx;
    }
    if !(shot.is_finite() && shot > 0. && read.is_finite()) {
        return None;
    }
    // Some read noise always: a frame whose darkest tiles measure none would
    // weigh its black infinitely.
    let darkest = points.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    Some(Noise {
        shot,
        read: read.max(shot * darkest * 1e-3).max(1e-12),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_is_fitted_through_the_flattest_tiles() {
        let truth = Noise {
            shot: 1e-4,
            read: 1e-7,
        };
        let points: Vec<_> = (0..10)
            .map(|i| {
                let level = 2f64.powi(-i);
                (level, truth.variance(level), 50.)
            })
            .collect();
        let got = fit(&points).unwrap();
        assert!((got.shot / truth.shot - 1.).abs() < 1e-6, "{got:?}");
        assert!((got.read / truth.read - 1.).abs() < 1e-3, "{got:?}");
        assert_eq!(fit(&points[..2]), None);
    }

    #[test]
    fn the_iso_model_is_noisier_at_higher_iso() {
        let (low, high) = (Noise::of_iso(100.), Noise::of_iso(1600.));
        assert!(high.shot > low.shot && high.read > low.read);
        assert!(low.variance(0.5) > 0. && low.variance(-1.) == low.read);
    }
}
