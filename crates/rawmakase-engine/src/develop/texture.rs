//! Presence > Texture, following Camera Raw 18.7, fitted to
//! renders of synthetic charts: sine gratings of 0.004 to 0.25 cycles per pixel at
//! ±0.1 to ±2 EV, large flats and edges, at Texture −100 to +100.
//!
//! Camera Raw's Texture is a local contrast of log luminance over a broad band of
//! scales, a few to about thirty pixels, in pixels of the full-resolution photo
//! (an image twice the size renders the same per pixel). It leaves large flat areas
//! alone, boosts faint detail most (×1.78 at ±0.1 EV and +100) and strong contrast
//! hardly at all (×1.05 at ±2 EV), so edges get soft halos fading over about thirty
//! pixels. Here the detail is a Laplacian pyramid of the log of each colour channel
//! (Camera Raw's Texture also raises colour contrast at colour edges, up to +19
//! chroma beside a saturated red, as if each channel had its own): each level is
//! compressed where it is strong and weighted, and the sum scaled by a strength that
//! Texture sets. Fitted to the gratings within 0.04 RMS (×gain) and to the edges'
//! halos within 2.4% of the edge's step.
use super::laplacian::Plane;
use crate::camera_data::CameraImage;
use anyhow::{Result, ensure};
use rawmakase_cancel::Cancel;
use rayon::prelude::*;

/// The Texture this recipe renders with the measured operator.
pub(crate) fn measured(r: &crate::model::recipe::Recipe) -> f32 {
    r.effects.texture
}

/// Pyramid levels, in full-resolution pixels: level `l` holds detail about `2^l`
/// pixels across.
const LEVELS: usize = 6;
/// Each level's weight and the log2 contrast above which it is compressed.
const WEIGHTS: [f32; LEVELS] = [1.402, 0.665, 1.571, 0.583, 0., 0.072];
const THRESHOLDS: [f32; LEVELS] = [0.506, 0.098, 0.167, 0.192, 0.135, 0.176];
/// Strength by Texture.
const AMOUNTS: [f32; 6] = [-1., -0.5, 0., 0.25, 0.5, 1.];
const STRENGTH: [f32; 6] = [-0.752, -0.499, 0., 0.334, 0.546, 0.852];

pub(crate) fn strength(amount: f32) -> f32 {
    let a = amount.clamp(-1., 1.);
    let i = AMOUNTS
        .partition_point(|v| *v <= a)
        .clamp(1, AMOUNTS.len() - 1);
    let t = (a - AMOUNTS[i - 1]) / (AMOUNTS[i] - AMOUNTS[i - 1]);
    STRENGTH[i - 1] + (STRENGTH[i] - STRENGTH[i - 1]) * t
}

/// The measured Texture's detail of each channel of an image, which any amount
/// scales: made once per image, so moving the slider only applies it.
pub(crate) struct TextureDetail {
    /// In steps of [`DETAIL_STEP`], so a 61-megapixel photo's fits the stage cache.
    channels: [Vec<i16>; 3],
}
/// The log2 detail per stored step: ±4 EV at 1/8192 EV.
const DETAIL_STEP: f32 = 1. / 8192.;
impl TextureDetail {
    /// The detail of `im`, an image `scale` times the full-resolution photo's size;
    /// stops with an error when `cancel` is set.
    pub(crate) fn of(im: &CameraImage, scale: f32, cancel: Cancel<'_>) -> Result<Self> {
        // The image's size against the full-resolution photo's, also for a half-size
        // draft decode whose caller passes scale 1 (`render_stage`).
        let full = im.metadata.width.max(im.metadata.height);
        let scale = if full > 0 {
            (im.width.max(im.height) as f32 / full as f32).min(scale.max(1e-3))
        } else {
            scale
        };
        // A reduced image's level 0 is a coarser full-resolution level.
        let offset = (-scale.max(1e-3).log2()).round().max(0.) as usize;
        let mut channels: [Vec<i16>; 3] = Default::default();
        for (c, out) in channels.iter_mut().enumerate() {
            let logs = Plane {
                w: im.width as usize,
                h: im.height as usize,
                data: im
                    .pixels
                    .par_iter()
                    .map(|p| p[c].max(1e-6).log2())
                    .collect(),
            };
            *out = detail(&logs, offset, cancel)?
                .data
                .par_iter()
                .map(|d| (d / DETAIL_STEP).round().clamp(-32767., 32767.) as i16)
                .collect();
        }
        Ok(Self { channels })
    }
    /// `im`, the image the detail was made from, with Texture `amount`.
    pub(crate) fn apply(&self, im: &CameraImage, amount: f32) -> CameraImage {
        let s = strength(amount) * DETAIL_STEP;
        let mut out = im.clone();
        out.recovered = Default::default();
        out.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
            for (v, detail) in p.iter_mut().zip(&self.channels) {
                *v *= (s * detail[i] as f32).exp2();
            }
        });
        out
    }
    /// The log2 detail of each channel at `x`, `y` of the `width`-wide image it was made
    /// from, interpolated bilinearly.
    pub(crate) fn at(&self, x: f32, y: f32, width: usize, height: usize) -> [f32; 3] {
        let fx = x.clamp(0., (width - 1) as f32);
        let fy = y.clamp(0., (height - 1) as f32);
        let (ix, iy) = (fx as usize, fy as usize);
        let (jx, jy) = ((ix + 1).min(width - 1), (iy + 1).min(height - 1));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        self.channels.each_ref().map(|c| {
            let v = |x: usize, y: usize| c[y * width + x] as f32;
            ((v(ix, iy) * (1. - tx) + v(jx, iy) * tx) * (1. - ty)
                + (v(ix, jy) * (1. - tx) + v(jx, jy) * tx) * ty)
                * DETAIL_STEP
        })
    }
    pub(crate) fn bytes(&self) -> usize {
        self.channels.iter().map(Vec::len).sum::<usize>() * 2
    }
}
/// The compressed, weighted detail of `plane` as pyramid level `level` and coarser,
/// expanded to its size. Only the level's plane, its blurred copy and the result are
/// full size at once.
fn detail(plane: &Plane, level: usize, cancel: Cancel<'_>) -> Result<Plane> {
    if level >= LEVELS || plane.w < 4 || plane.h < 4 {
        return Ok(Plane {
            w: plane.w,
            h: plane.h,
            data: vec![0.; plane.w * plane.h],
        });
    }
    ensure!(!cancel.is_cancelled(), "Render superseded");
    let coarse = plane.down(Cancel::NEVER);
    let mut out = detail(&coarse, level + 1, cancel)?.up(plane.w, plane.h, Cancel::NEVER);
    let blurred = coarse.up(plane.w, plane.h, Cancel::NEVER);
    drop(coarse);
    ensure!(!cancel.is_cancelled(), "Render superseded");
    let (w, t) = (WEIGHTS[level], THRESHOLDS[level]);
    out.data
        .par_iter_mut()
        .zip(plane.data.par_iter().zip(blurred.data.par_iter()))
        .for_each(|(o, (v, u))| {
            let band = v - u;
            let x = band / t;
            *o += w * band / (1. + x * x);
        });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// The gain Texture `amount` gives a vertical sine grating of `f` cycles per pixel
    /// and `amplitude` EV, on a full-resolution image.
    fn gain_at(f: f32, amplitude: f32, amount: f32) -> f32 {
        let (w, h) = (1024u32, 128u32);
        let im = CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels: (0..w * h)
                .map(|i| {
                    let x = (i % w) as f32;
                    [0.18 * (amplitude * (std::f32::consts::TAU * f * x).sin()).exp2(); 3]
                })
                .collect(),
            metadata: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let out = TextureDetail::of(&im, 1., Cancel::NEVER)
            .unwrap()
            .apply(&im, amount);
        let amp = |im: &CameraImage| {
            let (mut s, mut c) = (0., 0.);
            for x in 128..896 {
                let v = im.pixels[(64 * w + x) as usize][1].log2();
                let t = std::f32::consts::TAU * f * x as f32;
                (s, c) = (s + v * t.sin(), c + v * t.cos());
            }
            (s * s + c * c).sqrt()
        };
        amp(&out) / amp(&im)
    }

    /// Camera Raw 18.7 at Texture +100 on ±0.5 EV gratings, and on fainter and
    /// stronger ones at 0.03 cycles per pixel.
    #[test]
    fn measured_texture_follows_camera_raw_over_scale_and_contrast() {
        for (f, amplitude, camera_raw) in [
            (0.008, 0.5, 1.14),
            (0.03, 0.5, 1.43),
            (0.12, 0.5, 1.52),
            (0.03, 0.1, 1.78),
            (0.03, 2., 1.05),
        ] {
            let ours = gain_at(f, amplitude, 1.);
            assert!(
                (ours - camera_raw).abs() < 0.15,
                "{f} {amplitude}: {ours} against {camera_raw}"
            );
        }
        // Negative Texture smooths the same band; none leaves the image alone.
        assert!(gain_at(0.03, 0.5, -1.) < 0.75);
        assert!((gain_at(0.03, 0.5, 0.) - 1.).abs() < 1e-4);
    }
    /// A half-size draft decode given scale 1, whose pixels are twice the size of the
    /// photo's, still has Texture work at the photo's scale, as on a reduced copy.
    #[test]
    fn half_size_decodes_texture_at_the_photo_scale() {
        let (w, h) = (256u32, 128u32);
        let image = |full: u32| CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels: (0..w * h)
                .map(|i| [0.1 + 0.1 * ((i % w) as f32 * 0.3).sin().abs(); 3])
                .collect(),
            metadata: crate::camera_data::Metadata {
                width: full,
                height: full * h / w,
                ..Default::default()
            },
            fast: true,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let cancel = AtomicBool::new(false);
        let draft = TextureDetail::of(&image(2 * w), 1., Cancel::new(&cancel)).unwrap();
        let reduced = TextureDetail::of(&image(0), 0.5, Cancel::new(&cancel)).unwrap();
        let full = TextureDetail::of(&image(0), 1., Cancel::new(&cancel)).unwrap();
        assert_eq!(draft.channels, reduced.channels);
        assert_ne!(draft.channels, full.channels);
    }
}
