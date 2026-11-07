//! Manual luminance noise reduction on the full camera image, before the preview
//! pyramid and tone stage. A square-root luminance pyramid separates grain from
//! multi-pixel blotches. Each band's smooth blocks estimate its noise; soft
//! thresholding retains detail above that noise. This is not Adobe's algorithm.
//!
//! The bands are the photo's: a reduced decode ([`CameraImage::photo_scale`]) is
//! filtered with the settings of the photo's bands its own bands show, and stops where
//! the photo does.
use super::laplacian::{Plane, mirror, photo_level_offset};
use crate::camera_data::CameraImage;
use anyhow::{Result, ensure};
use rawmakase_cancel::Cancel;
use rayon::prelude::*;

const LEVELS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LuminanceDenoise {
    amount: f32,
    detail: f32,
    contrast: f32,
}
impl LuminanceDenoise {
    pub(crate) fn new(amount: f32, detail: f32, contrast: f32) -> Option<Self> {
        (amount > 0.).then_some(Self {
            amount,
            detail,
            contrast,
        })
    }

    pub(crate) fn apply(&self, im: &CameraImage, cancel: Cancel<'_>) -> Result<CameraImage> {
        check(cancel)?;
        let plane = Plane {
            w: im.width as usize,
            h: im.height as usize,
            data: im.pixels.par_iter().map(|p| luma(*p)).collect(),
        };
        // Camera pixels carry the as-shot WB. Exclude near-clipped flats from the
        // estimate, where clipping suppresses noise without making the capture clean.
        let white = luma(im.metadata.wb.map(|v| v.max(0.001))) * 0.97;
        let offset = photo_level_offset(im);
        let filtered = self.filter(&plane, 0, offset, 0., white, cancel)?;
        let mut out = im.clone();
        out.recovered = Default::default();
        out.pixels
            .par_chunks_mut(1024)
            .enumerate()
            .for_each(|(chunk, pixels)| {
                if cancel.is_cancelled() {
                    return;
                }
                for (j, p) in pixels.iter_mut().enumerate() {
                    let i = chunk * 1024 + j;
                    let delta = filtered.data[i] - plane.data[i];
                    // Preserve square-root chroma, as the color denoiser preserves this
                    // luminance. Avoid RGB ratios near black and never divide by zero.
                    if delta != 0. {
                        *p = p.map(|v| (v.max(0.).sqrt() + delta).max(0.).powi(2));
                    }
                }
            });
        check(cancel)?;
        Ok(out)
    }

    /// Filters `plane` as band `level`, recursing to the coarser ones; `offset` is the
    /// photo's band that this image's finest shows.
    fn filter(
        &self,
        plane: &Plane,
        level: usize,
        offset: f32,
        finer_noise: f32,
        white: f32,
        cancel: Cancel<'_>,
    ) -> Result<Plane> {
        check(cancel)?;
        // The photo's band this one shows. A band up to one past the photo's last is
        // filtered with the last's settings. One that starts at the photo's
        // unfiltered residual, or past it, is left alone, as the photo leaves it.
        let photo_level = level as f32 + offset;
        if photo_level >= LEVELS as f32 || plane.w < 4 || plane.h < 4 {
            return Ok(Plane {
                w: plane.w,
                h: plane.h,
                data: plane.data.clone(),
            });
        }
        let coarse = down(plane, cancel)?;
        let mut band = up(&coarse, plane.w, plane.h, cancel)?;
        band.data
            .par_iter_mut()
            .zip(&plane.data)
            .for_each(|(b, p)| *b = p - *b);
        let noise = band_noise(plane, &band, white, cancel)?.map_or(finer_noise * 0.6, |n| {
            if level == 0 { n } else { n.min(finer_noise) }
        });
        let coarse = self.filter(&coarse, level + 1, offset, noise, white, cancel)?;
        let mut out = up(&coarse, plane.w, plane.h, cancel)?;
        // Amount grows the threshold instead of just blending a fixed blur. Detail
        // protects fine structures; Contrast protects coarser structure separately.
        // Both change from the photo's finest band to the next: between them, a reduced
        // decode's band takes them in proportion.
        let coarse_share = photo_level.min(1.);
        let detail = 2f32.powf((0.5 - self.detail) * (1.5 - coarse_share));
        let contrast = 1. - self.contrast * 0.6 * coarse_share;
        let threshold = noise * 3.8 * self.amount.powf(1.3) * detail * contrast;
        out.data
            .par_chunks_mut(1024)
            .enumerate()
            .for_each(|(chunk, pixels)| {
                if cancel.is_cancelled() {
                    return;
                }
                for (j, p) in pixels.iter_mut().enumerate() {
                    let i = chunk * 1024 + j;
                    let d = band.data[i];
                    // Judge detail by its neighborhood's energy. A threshold on a
                    // single coefficient mistakes noise outliers for edges and
                    // leaves isolated specks in otherwise smooth areas.
                    let (x, y) = ((i % plane.w) as isize, (i / plane.w) as isize);
                    let mut energy = 0.;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            energy += band.data
                                [mirror(y + dy, plane.h) * plane.w + mirror(x + dx, plane.w)]
                            .powi(2)
                                / 9.;
                        }
                    }
                    let gain = if threshold <= 0. {
                        1.
                    } else {
                        1. / (1. + (threshold * threshold / energy.max(1e-20)).powi(2))
                    };
                    *p += d * gain;
                }
            });
        check(cancel)?;
        Ok(out)
    }
}

fn luma(p: [f32; 3]) -> f32 {
    let [r, g, b] = p.map(|v| v.max(0.).sqrt());
    (r + 2. * g + b) * 0.25
}
fn check(cancel: Cancel<'_>) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "Render superseded");
    Ok(())
}
/// `plane` blurred and halved; an error once `cancel` is set.
fn down(plane: &Plane, cancel: Cancel<'_>) -> Result<Plane> {
    check(cancel)?;
    let out = plane.down(cancel);
    check(cancel)?;
    Ok(out)
}
/// `plane` doubled to `w` × `h` and blurred; an error once `cancel` is set.
fn up(plane: &Plane, w: usize, h: usize, cancel: Cancel<'_>) -> Result<Plane> {
    check(cancel)?;
    let out = plane.up(w, h, cancel);
    check(cancel)?;
    Ok(out)
}

/// The tenth percentile of 8×8 block RMS energies rejects most scene structure.
/// Fewer than sixteen measurable blocks fall back to the finer level's estimate.
/// A scene without smooth regions can still overestimate noise; Detail protects
/// structure but this estimator is not a substitute for a calibrated noise profile.
fn band_noise(plane: &Plane, band: &Plane, white: f32, cancel: Cancel<'_>) -> Result<Option<f32>> {
    let (bw, bh) = (plane.w / 8, plane.h / 8);
    let mut energies: Vec<f32> = (0..bw * bh)
        .into_par_iter()
        .filter_map(|i| {
            if cancel.is_cancelled() {
                return None;
            }
            let (x, y) = (i % bw * 8, i / bw * 8);
            let mut energy = 0.;
            let mut valid = 0;
            for yy in y..y + 8 {
                for xx in x..x + 8 {
                    let i = yy * plane.w + xx;
                    let v = plane.data[i];
                    if v >= white {
                        return None;
                    }
                    valid += usize::from(v > 1e-5);
                    energy += band.data[i].powi(2);
                }
            }
            (valid >= 8).then_some(energy / 64.)
        })
        .collect();
    check(cancel)?;
    if energies.len() < 16 {
        return Ok(None);
    }
    let k = energies.len() / 10;
    let (_, e, _) = energies.select_nth_unstable_by(k, f32::total_cmp);
    Ok(Some(e.sqrt()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> CameraImage {
        CameraImage {
            width: w,
            height: h,
            pixels: (0..w * h).map(|i| f(i % w, i / w)).collect(),
            metadata: crate::camera_data::Metadata {
                wb: [1.; 3],
                ..Default::default()
            },
            recovered: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }
    }
    fn hash(x: u32, y: u32) -> f32 {
        let mut v = x.wrapping_mul(0x9e3779b9) ^ y.wrapping_mul(0x85ebca6b);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2c1b3c6d);
        v ^= v >> 12;
        v as f32 / u32::MAX as f32 - 0.5
    }
    fn energy(im: &CameraImage, base: f32) -> f32 {
        im.pixels
            .iter()
            .map(|p| (luma(*p) - base).powi(2))
            .sum::<f32>()
            / im.pixels.len() as f32
    }
    #[test]
    fn flat_colors_black_and_small_odd_images_remain_finite() {
        let cancel = AtomicBool::new(false);
        assert!(LuminanceDenoise::new(0., 0.5, 0.).is_none());
        for (w, h) in [(1, 1), (3, 5), (67, 65)] {
            for color in [[0.; 3], [0.01, 0.04, 0.02], [1.; 3]] {
                let im = image(w, h, |_, _| color);
                let out = LuminanceDenoise::new(1., 0.5, 0.)
                    .unwrap()
                    .apply(&im, Cancel::new(&cancel))
                    .unwrap();
                for p in out.pixels {
                    for (v, expected) in p.into_iter().zip(color) {
                        assert!(
                            v.is_finite() && (v - expected).abs() < 1e-6,
                            "{v} != {expected}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn strength_and_detail_follow_noise_across_brightness_and_amplitude() {
        let cancel = AtomicBool::new(false);
        for base in [0.1, 0.4, 0.7] {
            for amplitude in [0.002, 0.02, 0.06] {
                let im = image(128, 128, |x, y| {
                    [(base + amplitude * hash(x, y)).powi(2); 3]
                });
                let run = |amount, detail| {
                    let out = LuminanceDenoise::new(amount, detail, 0.)
                        .unwrap()
                        .apply(&im, Cancel::new(&cancel))
                        .unwrap();
                    energy(&out, base)
                };
                let original = energy(&im, base);
                let half = run(0.5, 0.5);
                let full = run(1., 0.5);
                assert!(half < original * 0.4, "retained {}", half / original);
                assert!(full < half);
                assert!(run(0.5, 1.) > half);
                assert!(run(0.5, 0.) < half);
            }
        }
    }
    #[test]
    fn noisy_edges_and_color_differences_survive() {
        let im = image(256, 128, |x, y| {
            let y = if x < 128 { 0.25 } else { 0.65 } + 0.025 * hash(x, y);
            [(y + 0.04).powi(2), y * y, (y - 0.04).powi(2)]
        });
        let out = LuminanceDenoise::new(0.5, 0.5, 0.)
            .unwrap()
            .apply(&im, Cancel::NEVER)
            .unwrap();
        for row in 16..112 {
            let a = luma(out.pixels[row * 256 + 127]);
            let b = luma(out.pixels[row * 256 + 128]);
            assert!(b - a > 0.35, "edge lost: {a} {b}");
        }
        for p in out.pixels {
            let [r, g, b] = p.map(f32::sqrt);
            assert!((r - g - 0.04).abs() < 1e-6 && (g - b - 0.04).abs() < 1e-6);
        }
    }
    /// A coefficient-only threshold can turn ordinary grain into sparse specks.
    /// On approximately Gaussian capture noise, the residual must not acquire a
    /// very heavy tail (Gaussian kurtosis is 3).
    #[test]
    fn smoothing_does_not_leave_sparse_noise_outliers() {
        let im = image(128, 128, |x, y| {
            let noise: f32 = (0..6).map(|k| hash(x + 131 * k, y + 73 * k)).sum();
            [(0.4 + 0.02 * noise).powi(2); 3]
        });
        let out = LuminanceDenoise::new(0.5, 0.5, 0.)
            .unwrap()
            .apply(&im, Cancel::NEVER)
            .unwrap();
        let values: Vec<f32> = out.pixels.iter().map(|p| luma(*p)).collect();
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32;
        let fourth = values.iter().map(|v| (v - mean).powi(4)).sum::<f32>() / values.len() as f32;
        let kurtosis = fourth / variance.powi(2);
        assert!(
            kurtosis < 8.,
            "residual became sparse specks: kurtosis {kurtosis}"
        );
    }

    #[test]
    fn cancellation_does_not_publish_partial_pixels() {
        let im = image(65, 67, |x, y| [0.1 + 0.01 * hash(x, y); 3]);
        let cancel = AtomicBool::new(true);
        let d = LuminanceDenoise::new(0.5, 0.5, 0.).unwrap();
        assert!(d.apply(&im, Cancel::new(&cancel)).is_err());
        cancel.store(false, Ordering::Relaxed);
        assert!(d.apply(&im, Cancel::new(&cancel)).is_ok());
    }
}
