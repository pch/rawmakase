//! The merge: each pixel the weighted mean of the frames' estimates of its
//! light, each frame weighed by how little noise its estimate has.
//!
//! A frame of gain `g` (its exposure over the darkest frame's) and sample `s`
//! estimates the light as `s / g`, with variance `(shot · s + read) / g²`; its
//! weight is the inverse of that, `g² / (shot · s + read)`, times a ramp from 1
//! to 0 between [`RAMP_START`] and [`RAMP_END`] of its saturation, and 0 where
//! it is clipped. Bright frames win wherever they are unclipped: their
//! estimates are the least noisy. The darkest frame is never ramped or
//! dropped: where every brighter frame clipped, it alone says what the light
//! was, clipped or not, as a single RAW would.
//!
//! The ramp and the clip mark take a pixel's three channels together, so all
//! three come from the same frames in the same shares and a ratio slightly
//! off can't tint the passage from one frame to the next.
use crate::Error;
use crate::noise::Noise;
use rawmakase_cancel::Cancel;
use rawmakase_decode::raw::Frame;
use rayon::prelude::*;

/// Where a frame's weight starts falling and where it reaches 0, as
/// fractions of its saturation.
pub(crate) const RAMP_START: f32 = 0.85;
pub(crate) const RAMP_END: f32 = 0.98;

/// How many rows are merged between checks of the cancel flag.
const BAND: usize = 64;

/// The running sums of a merge: per pixel and channel, the weighted
/// estimates and the weights. Single precision is enough: at most nine terms
/// are summed, and the result is stored to half precision.
pub(crate) struct Accumulator {
    width: usize,
    sums: Vec<[f32; 6]>,
}

impl Accumulator {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width: width as usize,
            sums: vec![[0.; 6]; width as usize * height as usize],
        }
    }

    /// Adds `frame`, of `gain` over the darkest frame and `noise`; `darkest`
    /// for the darkest frame, which is never ramped or dropped where it
    /// covers the image. `covered`, for a frame warped by Auto Align, says
    /// which pixels it covers; it adds nothing to the others.
    pub(crate) fn add(
        &mut self,
        frame: &Frame,
        gain: f64,
        noise: Noise,
        darkest: bool,
        covered: Option<&[bool]>,
        cancel: Cancel<'_>,
    ) -> Result<(), Error> {
        if frame.pixels.len() != self.sums.len()
            || covered.is_some_and(|c| c.len() != self.sums.len())
        {
            return Err(Error::Invalid("frames of different sizes"));
        }
        let g = gain as f32;
        let inverse_saturation = frame.saturation.map(|s| 1. / s.max(f32::MIN_POSITIVE));
        let (shot, read) = (noise.shot as f32, noise.read as f32);
        let width = self.width;
        self.sums
            .par_chunks_mut(width * BAND)
            .enumerate()
            .try_for_each(|(band, sums)| {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                let start = band * width * BAND;
                for (k, sum) in sums.iter_mut().enumerate() {
                    let i = start + k;
                    if covered.is_some_and(|c| !c[i]) {
                        continue;
                    }
                    let p = frame.pixels[i];
                    let ramp = if darkest {
                        1.
                    } else if frame.clipped[i] {
                        continue;
                    } else {
                        let level = (0..3)
                            .map(|c| p[c] * inverse_saturation[c])
                            .fold(0f32, f32::max);
                        ramp(level)
                    };
                    if ramp <= 0. {
                        continue;
                    }
                    for c in 0..3 {
                        let w = ramp * g * g / (shot * p[c].max(0.) + read);
                        sum[c] += w * p[c] / g;
                        sum[c + 3] += w;
                    }
                }
                Ok(())
            })
    }

    /// The merged light, in the darkest frame's units: 1 is what reached its
    /// white level.
    pub(crate) fn finish(self) -> Vec<[f32; 3]> {
        self.sums
            .into_par_iter()
            .map(|s| std::array::from_fn(|c| if s[c + 3] > 0. { s[c] / s[c + 3] } else { 0. }))
            .collect()
    }
}

/// A frame's weight for a pixel `level` of its saturation: 1 below
/// [`RAMP_START`], 0 from [`RAMP_END`], smoothly between.
pub(crate) fn ramp(level: f32) -> f32 {
    let t = ((RAMP_END - level) / (RAMP_END - RAMP_START)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ramp_falls_smoothly_from_1_to_0() {
        assert_eq!(ramp(0.), 1.);
        assert_eq!(ramp(RAMP_START), 1.);
        assert_eq!(ramp(RAMP_END), 0.);
        assert_eq!(ramp(1.5), 0.);
        let mid = ramp((RAMP_START + RAMP_END) / 2.);
        assert!((mid - 0.5).abs() < 1e-6);
        let mut last = 1.;
        for i in 0..=100 {
            let v = ramp(RAMP_START + (RAMP_END - RAMP_START) * i as f32 / 100.);
            assert!(v <= last);
            last = v;
        }
    }
}
