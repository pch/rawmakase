//! One channel of a Laplacian pyramid: the 5-tap binomial reduce and expand that
//! Texture and luminance noise reduction share, and the photo's level a reduced
//! decode's pyramid starts at, which color noise reduction uses too.
use crate::camera_data::CameraImage;
use rawmakase_cancel::Cancel;
use rayon::prelude::*;

/// The photo's pyramid level that `im`'s finest level shows, `−log₂` of its
/// [`photo_scale`](CameraImage::photo_scale): 0 for a photo decoded in full, 1 for a
/// half-size decode, a little under 1 for the half-size decode of a photo with an odd
/// side (`demosaic::half` rounds up).
pub(crate) fn photo_level_offset(im: &CameraImage) -> f32 {
    -im.photo_scale().log2()
}
/// How much of a level at the photo's level `photo_level` the photo filters, when it
/// filters every level finer than `residual` and none from it on: all of a level that
/// ends by then, the part before it of one that straddles it, and none of one past it.
/// A reduced decode's level that straddles the start of the residual is filtered in
/// proportion to its part before it, so noise reduction fades out continuously with
/// the decode's scale instead of stopping a level early or late.
pub(crate) fn filtered_share(photo_level: f32, residual: usize) -> f32 {
    (residual as f32 - photo_level).clamp(0., 1.)
}
/// `gain`, the share of a level's detail filtering keeps, for a level of which
/// `share` is filtered ([`filtered_share`]): the rest of the level keeps all of it.
/// A level filtered whole keeps `gain` as it is.
pub(crate) fn partial_gain(gain: f32, share: f32) -> f32 {
    if share >= 1. {
        gain
    } else {
        1. - share * (1. - gain)
    }
}

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
    /// Blurred and halved with exact left-right / top-bottom symmetry at every size.
    ///
    /// On an odd dimension, `0, 2, …, n − 1` already brackets both borders symmetrically
    /// under the 5-tap binomial filter. On an even dimension (`n → n / 2`), sampling only
    /// even indices shifts the center of mass by half a pixel per level, drifting a `1×1`
    /// coarsest level toward `(0, 0)` by ~15% of the frame; the centered 4-tap filter
    /// `[1, 3, 3, 1] / 8` at `2x + 0.5` keeps the center of mass at `0.5` and makes the
    /// full-depth pyramid invariant to 90°/180° rotations and flips.
    pub(crate) fn down_sym(&self, cancel: Cancel<'_>) -> Self {
        let (w, h) = (self.w.div_ceil(2), self.h.div_ceil(2));
        let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
        let mut wide = vec![0.; w * self.h];
        wide.par_chunks_mut(w)
            .zip(self.data.par_chunks(self.w))
            .for_each(|(out, row)| {
                if cancel.is_cancelled() {
                    return;
                }
                if self.w == w {
                    out.copy_from_slice(row);
                } else if self.w.is_multiple_of(2) {
                    for (x, o) in out.iter_mut().enumerate() {
                        let i = 2 * x as isize;
                        *o = (row[clamp(i - 1, self.w)] + row[clamp(i + 2, self.w)]) * 0.125
                            + (row[i as usize] + row[clamp(i + 1, self.w)]) * 0.375;
                    }
                } else {
                    for (x, o) in out.iter_mut().enumerate() {
                        *o = (0..5)
                            .map(|k| row[mirror(2 * x as isize + k - 2, self.w)] * TAPS[k as usize])
                            .sum();
                    }
                }
            });
        let mut data = vec![0.; w * h];
        data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
            if cancel.is_cancelled() {
                return;
            }
            if self.h == h {
                out.copy_from_slice(&wide[y * w..(y + 1) * w]);
            } else if self.h.is_multiple_of(2) {
                let i = 2 * y as isize;
                let r0 = &wide[clamp(i - 1, self.h) * w..][..w];
                let r1 = &wide[i as usize * w..][..w];
                let r2 = &wide[clamp(i + 1, self.h) * w..][..w];
                let r3 = &wide[clamp(i + 2, self.h) * w..][..w];
                for (x, o) in out.iter_mut().enumerate() {
                    *o = (r0[x] + r3[x]) * 0.125 + (r1[x] + r2[x]) * 0.375;
                }
            } else {
                let rows: [&[f32]; 5] = std::array::from_fn(|k| {
                    let r = mirror(2 * y as isize + k as isize - 2, self.h);
                    &wide[r * w..(r + 1) * w]
                });
                for (x, o) in out.iter_mut().enumerate() {
                    *o = (0..5).map(|k| rows[k][x] * TAPS[k]).sum();
                }
            }
        });
        Plane { w, h, data }
    }
    /// Symmetric counterpart to [`Plane::down_sym`], expanding back to `w` × `h`.
    pub(crate) fn up_sym(&self, w: usize, h: usize, cancel: Cancel<'_>) -> Self {
        let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
        let expand_odd = |i: usize, at: &dyn Fn(isize) -> f32| -> f32 {
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
                if w == self.w {
                    out.copy_from_slice(row);
                } else if w.is_multiple_of(2) && self.w == w / 2 {
                    for (x, o) in out.iter_mut().enumerate() {
                        let c = (x / 2) as isize;
                        *o = if x.is_multiple_of(2) {
                            row[clamp(c - 1, self.w)] * 0.25 + row[c as usize] * 0.75
                        } else {
                            row[c as usize] * 0.75 + row[clamp(c + 1, self.w)] * 0.25
                        };
                    }
                } else {
                    for (x, o) in out.iter_mut().enumerate() {
                        *o = expand_odd(x, &|c| row[mirror(c, self.w)]);
                    }
                }
            });
        let mut data = vec![0.; w * h];
        data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
            if cancel.is_cancelled() {
                return;
            }
            if h == self.h {
                out.copy_from_slice(&wide[y * w..(y + 1) * w]);
            } else if h.is_multiple_of(2) && self.h == h / 2 {
                let c = (y / 2) as isize;
                let (ra, rb, wa, wb) = if y.is_multiple_of(2) {
                    (
                        &wide[clamp(c - 1, self.h) * w..][..w],
                        &wide[c as usize * w..][..w],
                        0.25f32,
                        0.75f32,
                    )
                } else {
                    (
                        &wide[c as usize * w..][..w],
                        &wide[clamp(c + 1, self.h) * w..][..w],
                        0.75f32,
                        0.25f32,
                    )
                };
                for (x, o) in out.iter_mut().enumerate() {
                    *o = ra[x] * wa + rb[x] * wb;
                }
            } else {
                let row = |c: isize| {
                    let r = mirror(c, self.h);
                    &wide[r * w..(r + 1) * w]
                };
                for (x, o) in out.iter_mut().enumerate() {
                    *o = expand_odd(y, &|c| row(c)[x]);
                }
            }
        });
        Plane { w, h, data }
    }
}
