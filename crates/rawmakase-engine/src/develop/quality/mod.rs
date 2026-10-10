//! Full-resolution detail processing shared by Fit, 100% regions and exports.
use crate::develop::masks::{MaskWeights, local::slot};
use crate::develop::sharpening::Sharpener;
use crate::develop::{
    pipeline::Toned,
    preview_renderer::Stages,
    stage_cache::{StageCache, TextureKey},
};
use crate::rendered::Rendered;
use crate::{
    camera_data::CameraImage,
    develop::{self, Geometry},
    model::{recipe::Recipe, valid::ValidRecipe},
};
use anyhow::{Result, ensure};
use rayon::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
    Ok(())
}

use crate::color::luminance;
pub fn fit_edge(width: u32, height: u32, viewport: [u32; 2]) -> u32 {
    let scale = (viewport[0].max(1) as f64 / width as f64)
        .min(viewport[1].max(1) as f64 / height as f64)
        .min(1.);
    ((width.max(height) as f64 * scale).round() as u32).max(1)
}
pub(crate) fn output_size(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    if max_edge == 0 || width.max(height) <= max_edge {
        return (width, height);
    }
    let scale = max_edge as f64 / width.max(height) as f64;
    (
        (width as f64 * scale).round().max(1.) as u32,
        (height as f64 * scale).round().max(1.) as u32,
    )
}
/// A preview render: pixels on the CPU, or a frame presented into a texture on the
/// GPU when a display was given.
pub enum Output {
    Pixels(Rendered),
    Frame(Box<develop::gpu::Frame>),
}
impl Output {
    /// The pixels of a render made without a display, which is never a frame.
    pub fn pixels(self) -> Rendered {
        match self {
            Output::Pixels(pixels) => pixels,
            Output::Frame(_) => unreachable!("Frames are only presented to a display"),
        }
    }
}
pub fn resize(image: Rendered, max_edge: u32) -> Rendered {
    if max_edge == 0 || image.width.max(image.height) <= max_edge {
        return image;
    }
    let (w, h) = output_size(image.width, image.height, max_edge);
    let buffer = image::Rgb32FImage::from_raw(
        image.width,
        image.height,
        image.pixels.into_iter().flatten().collect(),
    )
    .unwrap();
    let buffer = image::imageops::resize(&buffer, w, h, image::imageops::FilterType::Lanczos3);
    Rendered {
        width: w,
        height: h,
        pixels: buffer
            .into_raw()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| [p[0].clamp(0., 1.), p[1].clamp(0., 1.), p[2].clamp(0., 1.)])
            .collect(),
    }
}

mod detail;
mod highlights;
mod local;
mod render;
mod samples;
pub(crate) use detail::*;
pub use highlights::*;
pub(crate) use local::*;
pub use render::*;
pub use samples::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_highlight_recovery_keeps_nothing() {
        let cancel = AtomicBool::new(true);
        let image = fixture();
        assert!(recover_highlights_cancellable(&image, &cancel).is_err());
        assert!(image.recovered.get().is_none());
    }
    fn fixture() -> CameraImage {
        CameraImage {
            width: 96,
            height: 80,
            pixels: (0..96 * 80)
                .map(|i| {
                    let y = (i % 96) as f32 / 192. + 0.05;
                    [y * 0.8, y, y * 0.6]
                })
                .collect(),
            metadata: crate::camera_data::Metadata {
                width: 96,
                height: 80,
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                wb: [1.; 3],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            recovered: Default::default(),
        }
    }
    #[test]
    fn preset_effects_match_tiles_and_survive_serialization() -> Result<()> {
        let im = fixture();
        let mut r = Recipe::default();
        r.effects.grain = 0.5;
        r.effects.vignette = -0.3;
        r.effects.clarity = 0.4;
        r.effects.texture = -0.2;
        r.effects.channels[0].insert([0.4, 0.5]);
        r.effects.calibration[2] = [0.2, -0.1];
        r.effects.parametric = [0.1, -0.2, 0.1, 0.];
        let saved = serde_json::to_vec(&r)?;
        let restored: Recipe = serde_json::from_slice(&saved)?;
        assert_eq!(r, restored);
        let full = render(&im, &r.checked()?, 0, None)?;
        let tile = render(&im, &r.checked()?, 0, Some([30, 25, 40, 40]))?;
        for y in 0..40 {
            for x in 0..40 {
                let a = tile.pixels[y * 40 + x];
                let b = full.pixels[(y + 25) * 96 + x + 30];
                for c in 0..3 {
                    assert!((a[c] - b[c]).abs() < 2e-6);
                }
            }
        }
        assert_eq!(full.pixels, render(&im, &r.checked()?, 0, None)?.pixels);
        assert_ne!(
            full.pixels,
            render(&im, &Recipe::default().checked()?, 0, None)?.pixels
        );
        Ok(())
    }
    #[test]
    fn region_matches_full_with_large_radius_and_local_tones() -> Result<()> {
        let im = fixture();
        // Every scene stage control: a region measures the whole photo.
        let mut r = Recipe {
            sharpening_radius: 3.,
            sharpening: 0.8,
            shadows: 0.5,
            highlights: -0.4,
            whites: 0.5,
            blacks: -0.3,
            ..Default::default()
        };
        r.effects.clarity = -0.4;
        r.effects.texture = 0.3;
        // Negative Dehaze from its tables, positive from the photo's haze.
        for dehaze in [-0.3, 0.6] {
            r.effects.dehaze = dehaze;
            let full = render(&im, &r.checked()?, 0, None)?;
            for [x, y, w, h] in [[0, 0, 20, 30], [30, 25, 40, 40], [80, 60, 16, 20]] {
                let tile = render(&im, &r.checked()?, 0, Some([x, y, w, h]))?;
                for yy in 0..h {
                    for xx in 0..w {
                        let a = tile.pixels[(yy * w + xx) as usize];
                        let b = full.pixels[((yy + y) * full.width + xx + x) as usize];
                        for c in 0..3 {
                            assert!((a[c] - b[c]).abs() < 2e-6, "{dehaze}");
                        }
                    }
                }
            }
        }
        Ok(())
    }
    #[test]
    fn final_fit_is_export_resized_after_detail() -> Result<()> {
        let im = fixture();
        let r = Recipe {
            sharpening: 0.8,
            ..Default::default()
        };
        let full = render(&im, &r.checked()?, 0, None)?;
        let expected = resize(full, 48);
        let fit = render(&im, &r.checked()?, 48, None)?;
        assert_eq!(fit.pixels, expected.pixels);
        Ok(())
    }
    #[test]
    fn partial_highlight_uses_neighbor_ratios_and_full_clip_is_neutral() {
        let mut im = fixture();
        im.pixels.fill([0.8, 0.4, 0.2]);
        let i = 40 * 96 + 40;
        im.pixels[i] = [1., 0.6, 0.3];
        let recovered = recover_highlights(&im);
        assert!((recovered.pixels[i][0] - 1.2).abs() < 1e-5);
        assert_eq!(recovered.pixels[i][1], 0.6);
        im.pixels.fill([1., 1., 1.]);
        assert!(recover_highlights(&im).pixels.iter().all(|p| *p == [1.; 3]));
    }
    #[test]
    fn cancellation_does_not_publish_partial_frame() {
        let im = fixture();
        assert!(
            render_cancellable(
                &im,
                &Recipe::default().checked().unwrap(),
                0,
                None,
                &std::sync::atomic::AtomicBool::new(true)
            )
            .is_err()
        );
    }
    #[test]
    fn sharpening_increases_edge_contrast_without_tint() {
        let mut im = Rendered {
            width: 32,
            height: 32,
            pixels: (0..1024)
                .map(|i| [if i % 32 < 16 { 0.3 } else { 0.6 }; 3])
                .collect(),
        };
        sharpen(&mut im, &Recipe::default());
        assert!(im.pixels[15][0] < 0.3);
        assert!(im.pixels[16][0] > 0.6);
        assert!(im.pixels.iter().all(|p| p[0] == p[1] && p[1] == p[2]));
    }
    #[test]
    fn physical_fit_size_has_no_fixed_ceiling() {
        assert_eq!(fit_edge(6000, 4000, [3000, 2000]), 3000);
        assert_eq!(fit_edge(6000, 4000, [1000, 2000]), 1000);
    }
    #[test]
    fn sharpening_preserves_flat_fields() {
        let mut im = Rendered {
            width: 20,
            height: 20,
            pixels: vec![[0.4, 0.3, 0.2]; 400],
        };
        let before = im.pixels.clone();
        sharpen(&mut im, &Recipe::default());
        for (a, b) in im.pixels.iter().zip(before) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-6);
            }
        }
    }
}
