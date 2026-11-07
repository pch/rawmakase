//! One channel of a Laplacian pyramid: the 5-tap binomial reduce and expand that
//! Texture and luminance noise reduction share, and the photo's level a reduced
//! decode's pyramid starts at, which colour noise reduction uses too.
use crate::camera_data::CameraImage;
use rawmakase_cancel::Cancel;
use rayon::prelude::*;

/// The photo's pyramid level that `im`'s finest level shows, `−log₂` of its
/// [`photo_scale`](CameraImage::photo_scale): 0 for a photo decoded in full, 1 for a
/// half-size decode. Within [`WHOLE_LEVEL`] of a whole level it is that level: a
/// half-size decode of a photo with an odd side is a pixel more than half of it
/// (`demosaic::half` rounds up), and its levels are still the photo's whole ones.
pub(crate) fn photo_level_offset(im: &CameraImage) -> f32 {
    let offset = -im.photo_scale().log2();
    let whole = offset.round();
    if (offset - whole).abs() < WHOLE_LEVEL {
        whole
    } else {
        offset
    }
}
/// How far from a whole level [`photo_level_offset`] still takes it: one pixel of
/// rounding on a long edge of 72 pixels or more is less.
const WHOLE_LEVEL: f32 = 0.02;

/// One channel at one pyramid level.
pub(crate) struct Plane {
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) data: Vec<f32>,
}
/// The 5-tap binomial filter, and its expanding weights for even and odd samples.
const TAPS: [f32; 5] = [1. / 16., 4. / 16., 6. / 16., 4. / 16., 1. / 16.];
const EVEN: [f32; 3] = [TAPS[0] * 2., TAPS[2] * 2., TAPS[4] * 2.];
const ODD: f32 = TAPS[1] * 2.;
/// `v` reflected into `0..n` about its ends, without repeating them.
pub(crate) fn mirror(v: isize, n: usize) -> usize {
    let n = n as isize;
    let v = v.abs();
    (if v >= n { 2 * n - 2 - v } else { v }).clamp(0, n - 1) as usize
}
impl Plane {
    /// Blurred and halved. Rows started after `cancel` is set are left at 0, so a
    /// caller that can cancel checks it before using the result.
    pub(crate) fn down(&self, cancel: Cancel<'_>) -> Self {
        let (w, h) = (self.w.div_ceil(2), self.h.div_ceil(2));
        let mut wide = vec![0.; w * self.h];
        wide.par_chunks_mut(w)
            .zip(self.data.par_chunks(self.w))
            .for_each(|(out, row)| {
                if cancel.is_cancelled() {
                    return;
                }
                for (x, o) in out.iter_mut().enumerate() {
                    *o = (0..5)
                        .map(|k| row[mirror(2 * x as isize + k - 2, self.w)] * TAPS[k as usize])
                        .sum();
                }
            });
        let mut data = vec![0.; w * h];
        data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
            if cancel.is_cancelled() {
                return;
            }
            let rows: [&[f32]; 5] = std::array::from_fn(|k| {
                let r = mirror(2 * y as isize + k as isize - 2, self.h);
                &wide[r * w..(r + 1) * w]
            });
            for (x, o) in out.iter_mut().enumerate() {
                *o = (0..5).map(|k| rows[k][x] * TAPS[k]).sum();
            }
        });
        Plane { w, h, data }
    }
    /// Doubled to `w` × `h` and blurred, as a Laplacian pyramid expands a level. Rows
    /// started after `cancel` is set are left at 0, as in [`Plane::down`].
    pub(crate) fn up(&self, w: usize, h: usize, cancel: Cancel<'_>) -> Self {
        let expand = |i: usize, at: &dyn Fn(isize) -> f32| -> f32 {
            let c = (i / 2) as isize;
            if i.is_multiple_of(2) {
                at(c - 1) * EVEN[0] + at(c) * EVEN[1] + at(c + 1) * EVEN[2]
            } else {
                (at(c) + at(c + 1)) * ODD
            }
        };
        let mut wide = vec![0.; w * self.h];
        wide.par_chunks_mut(w)
            .zip(self.data.par_chunks(self.w))
            .for_each(|(out, row)| {
                if cancel.is_cancelled() {
                    return;
                }
                for (x, o) in out.iter_mut().enumerate() {
                    *o = expand(x, &|c| row[mirror(c, self.w)]);
                }
            });
        let mut data = vec![0.; w * h];
        data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
            if cancel.is_cancelled() {
                return;
            }
            let row = |c: isize| {
                let r = mirror(c, self.h);
                &wide[r * w..(r + 1) * w]
            };
            for (x, o) in out.iter_mut().enumerate() {
                *o = expand(y, &|c| row(c)[x]);
            }
        });
        Plane { w, h, data }
    }
}
