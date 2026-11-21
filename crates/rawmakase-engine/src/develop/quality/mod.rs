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
use rawmakase_cancel::Cancel;
use rayon::prelude::*;
use std::sync::Arc;

fn check_cancel(cancel: Cancel<'_>) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "Render superseded");
    Ok(())
}

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
    /// An HDR master's preview, presented for the window (`PreviewRenderer::render_hdr_to`).
    Hdr(Box<develop::gpu::HdrFrame>),
}
impl Output {
    /// The pixels of a render made without a display, which is never a frame.
    pub fn pixels(self) -> Rendered {
        match self {
            Output::Pixels(pixels) => pixels,
            Output::Frame(_) | Output::Hdr(_) => {
                unreachable!("Frames are only presented to a display")
            }
        }
    }
}
/// An HDR master's preview the GPU port doesn't render; the CPU's master
/// (`render_hdr`) is the fallback, never an SDR stage.
#[derive(Debug)]
pub(crate) struct HdrNotOnGpu;
impl std::fmt::Display for HdrNotOnGpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The GPU doesn't render this HDR preview")
    }
}
impl std::error::Error for HdrNotOnGpu {}
pub fn resize(image: Rendered, max_edge: u32) -> Rendered {
    let mut out = resize_unbounded(image, max_edge);
    for p in &mut out.pixels {
        *p = p.map(|v| v.clamp(0., 1.));
    }
    out
}
/// [`resize`] without clipping at 1: for an HDR master's encoded values, which its
/// caller bounds. `image`'s resize clips float samples to 0–1, so values above 1 are
/// scaled into that range first, with room for Lanczos's overshoot, and back after;
/// values at most 1 (every SDR render's) are resized as they are.
pub(crate) fn resize_unbounded(image: Rendered, max_edge: u32) -> Rendered {
    if max_edge == 0 || image.width.max(image.height) <= max_edge {
        return image;
    }
    let (w, h) = output_size(image.width, image.height, max_edge);
    let top = image.pixels.iter().flatten().copied().fold(0f32, f32::max);
    let scale = if top > 1. { 2. * top } else { 1. };
    let buffer = image::Rgb32FImage::from_raw(
        image.width,
        image.height,
        image
            .pixels
            .into_iter()
            .flatten()
            .map(|v| if scale == 1. { v } else { v / scale })
            .collect(),
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
            .map(|p| {
                if scale == 1. {
                    [p[0], p[1], p[2]]
                } else {
                    [p[0] * scale, p[1] * scale, p[2] * scale]
                }
            })
            .collect(),
    }
}

mod detail;
mod hdr;
mod hdr_defringe;
mod highlights;
mod local;
mod prepare;
mod render;
mod samples;
pub(crate) use detail::*;
pub(crate) use hdr::{hdr_file_recipe, hdr_file_scene};
pub use hdr::{render_hdr, render_hdr_preview, render_hdr_region};
pub use highlights::*;
pub(crate) use local::*;
pub(crate) use prepare::{Resolved, resolve, resolve_hdr};
pub use render::*;
pub use samples::*;

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    #[test]
    fn cancelled_highlight_recovery_keeps_nothing() {
        let cancel = AtomicBool::new(true);
        let image = fixture();
        assert!(recover_highlights(&image, Cancel::new(&cancel)).is_err());
        assert!(image.recovered.get().is_none());
    }
    pub(super) fn fixture() -> CameraImage {
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
            hdr: Default::default(),
            recovered: Default::default(),
        }
    }
    /// [`fixture`] with highlights at and above the sensor's white, bright areas
    /// recovery pulls a little and dark ones it leaves alone.
    fn clipped() -> CameraImage {
        let mut im = fixture();
        for y in 20..40 {
            for x in 30..60 {
                im.pixels[y * 96 + x] = [1., 0.6 + x as f32 * 0.01, 0.4];
            }
        }
        for p in &mut im.pixels[..96 * 10] {
            *p = [0.1, 0.12, 0.05];
        }
        // A color matrix, which Auto white balance needs.
        im.metadata.cam_xyz = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        im
    }
    /// Every bit of every pixel, read without copying the image out.
    fn bits(image: &CameraImage) -> Vec<[u32; 3]> {
        let pixels = image.pixels.range(0..image.pixels.len());
        pixels.iter().map(|p| p.map(f32::to_bits)).collect()
    }
    /// Highlights recovered in a decode's own buffer are the pixels a second
    /// buffer would hold, bit for bit; the decode still reads as it was decoded,
    /// and the two images hold one buffer between them.
    #[test]
    fn recovering_in_place_gives_the_same_two_images_from_one_buffer() -> Result<()> {
        // With a zero recovery gives another sign and a channel that is not a
        // number, which only a comparison of bits tells apart.
        let odd = || {
            let mut im = clipped();
            im.pixels[96 * 50 + 7] = [-0., 0.9, 0.5];
            im.pixels[96 * 50 + 9] = [f32::NAN, 0.9, 0.5];
            im
        };
        let decoded = odd();
        let expected = recover_highlights(&decoded, Cancel::NEVER)?;
        assert_eq!(expected.pixels[96 * 50 + 7][0].to_bits(), 0f32.to_bits());
        let changed = (bits(&decoded).iter().zip(bits(&expected)))
            .filter(|(a, b)| **a != *b)
            .count();
        assert!(changed > 600 && changed < decoded.pixels.len() - 900);
        let image = with_recovered(odd(), Cancel::NEVER)?;
        let made = recovered(&image, Cancel::NEVER)?;
        assert!(Arc::ptr_eq(&made, image.recovered.get().unwrap()));
        assert_eq!(bits(&made), bits(&expected));
        assert_eq!(bits(&image), bits(&decoded));
        assert!(image.pixels.shares_buffer(&made.pixels));
        assert_eq!(image.pixels.differing().count(), changed);
        // The recovered image is the decode's copy in all but its pixels, as
        // `recover_highlights` makes it: the same Auto CA measurement, no
        // recovered image of its own.
        assert!(made.recovered.get().is_none());
        assert!(Arc::ptr_eq(
            &made.metadata.lateral_ca,
            &image.metadata.lateral_ca
        ));
        // Once recovered, an image is left as it is.
        let again = with_recovered(image, Cancel::NEVER)?;
        assert!(Arc::ptr_eq(&made, again.recovered.get().unwrap()));
        assert!(again.pixels.shares_buffer(&made.pixels));
        Ok(())
    }
    /// Where recovery changes nothing the two images are one buffer outright: a
    /// RAW without a bright pixel, and a rendered file, whose recovered image
    /// is the file as it is.
    #[test]
    fn images_recovery_leaves_alone_are_one_buffer() -> Result<()> {
        let mut dark = fixture();
        dark.pixels.fill([0.2, 0.1, 0.05]);
        let dark = with_recovered(dark, Cancel::NEVER)?;
        let made = recovered(&dark, Cancel::NEVER)?;
        assert_eq!(dark.pixels.differing().count(), 0);
        assert_eq!(dark.pixels.as_ptr(), made.pixels.as_ptr());
        assert!(dark.pixels.shares_buffer(&made.pixels));
        // An image without a pixel is recovered too, to one without a pixel.
        let mut none = fixture();
        (none.width, none.height, none.pixels) = (0, 0, Default::default());
        let none = with_recovered(none, Cancel::NEVER)?;
        assert!(recovered(&none, Cancel::NEVER)?.pixels.is_empty());

        let file = || {
            let mut file = clipped();
            file.metadata.source = crate::camera_data::Source::Rendered;
            file
        };
        let expected = bits(&file());
        // Owned: recovered in place.
        let owned = with_recovered(file(), Cancel::NEVER)?;
        // Borrowed: one buffer when the file's pixels are shared, as a file opens.
        let mut opened = file();
        opened.pixels.share();
        // Borrowed with a buffer of its own: a copy, as it always was.
        let apart = file();
        for (image, one_buffer) in [(&owned, true), (&opened, true), (&apart, false)] {
            let made = recovered(image, Cancel::NEVER)?;
            assert_eq!(bits(&made), expected);
            assert_eq!(bits(image), expected);
            assert_eq!(image.pixels.shares_buffer(&made.pixels), one_buffer);
            assert_eq!(image.pixels.as_ptr() == made.pixels.as_ptr(), one_buffer);
        }
        Ok(())
    }
    /// A cancelled recovery in place is an error, as a cancelled recovery is.
    #[test]
    fn a_cancelled_recovery_in_place_is_an_error() {
        let cancel = AtomicBool::new(true);
        assert!(with_recovered(clipped(), Cancel::new(&cancel)).is_err());
    }
    /// A decode that holds one buffer with its recovered image renders, measures
    /// and samples as a decode with a buffer of its own does, whichever of its
    /// two images the reader takes; and none of it copies the decode out, which
    /// would spend the memory again.
    #[test]
    fn a_decode_holding_one_buffer_renders_and_measures_the_same() -> Result<()> {
        use crate::develop::{auto, upright};
        let one = with_recovered(clipped(), Cancel::NEVER)?;
        let apart = clipped();
        let made = recovered(&one, Cancel::NEVER)?;
        assert_ne!(bits(&one), bits(&made));
        let never = Cancel::NEVER;
        let mut edited = Recipe {
            highlights: -0.8,
            shadows: 0.5,
            lens_ca: true,
            ..Default::default()
        };
        edited.effects.texture = 0.3;
        for r in [Recipe::default(), edited] {
            let valid = r.checked()?;
            // Rendered from the recovered image, whole, reduced and a region.
            for (edge, region) in [(0, None), (40, None), (0, Some([30, 15, 40, 30]))] {
                assert_eq!(
                    render(&one, &valid, edge, region, never)?.pixels,
                    render(&apart, &valid, edge, region, never)?.pixels,
                    "{edge} {region:?}"
                );
            }
            // Reduced from the decode itself, then recovered: the Tone Curve's histogram.
            assert_eq!(
                curve_input_histogram(&one, &valid, None, 48, never)?,
                curve_input_histogram(&apart, &valid, None, 48, never)?
            );
            // Auto white balance reads the decode, Auto tone its recovered image.
            assert_eq!(
                auto::auto_white_balance(&one, &r, never)?.wb,
                auto::auto_white_balance(&apart, &r, never)?.wb
            );
            let (a, b) = (
                auto::auto_tone(&one, &r, never)?,
                auto::auto_tone(&apart, &r, never)?,
            );
            assert_eq!(
                (a.exposure, a.highlights, a.shadows),
                (b.exposure, b.highlights, b.shadows)
            );
            // The decode's own samples, before recovery, and a stage after it.
            for stage in [Stage::CameraInput, Stage::SceneInput] {
                assert_eq!(
                    render_stage(&one, &r, stage, never)?.pixels,
                    render_stage(&apart, &r, stage, never)?.pixels,
                    "{stage:?}"
                );
            }
            assert_eq!(
                point_color_pick(&one, &r, 0.45, 0.4, never)?,
                point_color_pick(&apart, &r, 0.45, 0.4, never)?
            );
            // Upright analyses the decode pixel by pixel.
            assert_eq!(
                upright::analysis_image(&one, &r),
                upright::analysis_image(&apart, &r)
            );
        }
        // Auto black & white measures the decode's colors row by row.
        assert_eq!(
            crate::develop::ColorSpread::measure(&one),
            crate::develop::ColorSpread::measure(&apart)
        );
        assert!(
            one.pixels.shares_buffer(&made.pixels),
            "a reader copied the decode out"
        );
        Ok(())
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
        let full = render(&im, &r.checked()?, 0, None, Cancel::NEVER)?;
        let tile = render(&im, &r.checked()?, 0, Some([30, 25, 40, 40]), Cancel::NEVER)?;
        for y in 0..40 {
            for x in 0..40 {
                let a = tile.pixels[y * 40 + x];
                let b = full.pixels[(y + 25) * 96 + x + 30];
                for c in 0..3 {
                    assert!((a[c] - b[c]).abs() < 2e-6);
                }
            }
        }
        assert_eq!(
            full.pixels,
            render(&im, &r.checked()?, 0, None, Cancel::NEVER)?.pixels
        );
        assert_ne!(
            full.pixels,
            render(&im, &Recipe::default().checked()?, 0, None, Cancel::NEVER)?.pixels
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
            let full = render(&im, &r.checked()?, 0, None, Cancel::NEVER)?;
            for [x, y, w, h] in [[0, 0, 20, 30], [30, 25, 40, 40], [80, 60, 16, 20]] {
                let tile = render(&im, &r.checked()?, 0, Some([x, y, w, h]), Cancel::NEVER)?;
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
        let full = render(&im, &r.checked()?, 0, None, Cancel::NEVER)?;
        let expected = resize(full, 48);
        let fit = render(&im, &r.checked()?, 48, None, Cancel::NEVER)?;
        assert_eq!(fit.pixels, expected.pixels);
        Ok(())
    }
    #[test]
    fn cancellation_does_not_publish_partial_frame() {
        let im = fixture();
        assert!(
            render(
                &im,
                &Recipe::default().checked().unwrap(),
                0,
                None,
                Cancel::new(&std::sync::atomic::AtomicBool::new(true))
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
        sharpen(&mut im, &Recipe::default(), None, Cancel::NEVER).unwrap();
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
        sharpen(&mut im, &Recipe::default(), None, Cancel::NEVER).unwrap();
        for (a, b) in im.pixels.iter().zip(before) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-6);
            }
        }
    }
}
