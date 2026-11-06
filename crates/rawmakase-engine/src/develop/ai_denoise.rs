//! AI Denoise in rendering: the photo's AI source, resolved and immutable, blended with
//! the plain image by Amount before Color and luminance noise reduction
//! (docs/rendering-quality.md#ai-denoise). The engine never computes or fetches
//! the source: the Develop session or the export hands it over.
use crate::camera_data::CameraImage;
use anyhow::Result;
use rawmakase_cancel::Cancel;
use rayon::prelude::*;
use std::sync::Arc;

/// A photo's AI Denoise source, highlights recovered on its own (never with the plain
/// image's recovery: recovery depends on each pixel's own values). Each result the
/// session publishes, partial or final, is a new source, so caches keyed by it see the
/// change. Equal only to itself.
#[derive(Clone)]
pub struct AiSource(Arc<CameraImage>);
impl AiSource {
    /// Recovers the highlights of `image`, the assembled AI source
    /// (`rawmakase_decode::ai::denoise::assemble`); its unrecovered pixels are let go.
    pub fn recover(image: CameraImage, cancel: Cancel<'_>) -> Result<Self> {
        Ok(Self(super::quality::recovered(&image, cancel)?))
    }
    /// The recovered image.
    pub fn image(&self) -> &Arc<CameraImage> {
        &self.0
    }
    /// Whether this source belongs to `plain`'s frame: a result for another photo, or
    /// for a draft, is never rendered.
    pub fn fits(&self, plain: &CameraImage) -> bool {
        self.0.width == plain.width && self.0.height == plain.height && !plain.fast
    }
}
impl PartialEq for AiSource {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl std::fmt::Debug for AiSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AiSource({:p})", Arc::as_ptr(&self.0))
    }
}

/// The AI source and how much of it the photo renders.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AiBlend {
    pub source: AiSource,
    /// 0–1, above 0.
    pub amount: f32,
}
/// How much of the AI source's luminance and colour change an Amount keeps: Camera Raw
/// 18.7's Denoise blends its result in by the `BlendParams` it writes, measured at 25,
/// 50 and 100 (0.4375 and 0.5, 0.70 and 0.75, 1 and 1; docs/rendering-quality.md#ai-denoise),
/// and linearly between them and from 0. The first is taken as luminance's, the second,
/// always the larger, as colour's.
const LUMA_WEIGHTS: [(f32, f32); 4] = [(0., 0.), (0.25, 0.4375), (0.5, 0.7), (1., 1.)];
const CHROMA_WEIGHTS: [(f32, f32); 4] = [(0., 0.), (0.25, 0.5), (0.5, 0.75), (1., 1.)];

/// The weight at Amount `a` of the piecewise linear `curve`.
fn weight(curve: &[(f32, f32); 4], a: f32) -> f32 {
    let a = a.clamp(0., 1.);
    curve.windows(2).find(|w| a <= w[1].0).map_or(1., |w| {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        y0 + (a - x0) / (x1 - x0) * (y1 - y0)
    })
}

impl AiBlend {
    /// The plain recovered image with the AI source's change blended in by Amount, its
    /// luminance (the channels' mean) and colour (the rest) each by their weight; at
    /// Amount 1 the AI source itself.
    pub(crate) fn apply(&self, plain: &Arc<CameraImage>) -> Arc<CameraImage> {
        let ai = self.source.image();
        if self.amount >= 1. {
            return ai.clone();
        }
        let (wl, wc) = (
            weight(&LUMA_WEIGHTS, self.amount),
            weight(&CHROMA_WEIGHTS, self.amount),
        );
        let pixels = plain
            .pixels
            .par_iter()
            .zip(ai.pixels.par_iter())
            .map(|(p, q)| {
                let d: [f32; 3] = std::array::from_fn(|c| q[c] - p[c]);
                let luma = (d[0] + d[1] + d[2]) / 3.;
                std::array::from_fn(|c| p[c] + wl * luma + wc * (d[c] - luma))
            })
            .collect();
        Arc::new(CameraImage {
            recovered: Default::default(),
            width: plain.width,
            height: plain.height,
            pixels,
            metadata: plain.metadata.clone(),
            fast: plain.fast,
            scale_factor: plain.scale_factor,
            scale_clipped: plain.scale_clipped,
            hdr: Default::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::Metadata;
    use crate::develop::recipe::RenderedRecipe;
    use crate::develop::retouch::RetouchCache;
    use crate::model::{ai_denoise::AiDenoise, recipe::Recipe};
    use std::sync::atomic::AtomicBool;

    fn image(pixel: [f32; 3]) -> CameraImage {
        CameraImage {
            recovered: Default::default(),
            width: 24,
            height: 16,
            pixels: (0..24 * 16)
                .map(|i| pixel.map(|v| v * (0.9 + (i % 5) as f32 * 0.05)))
                .collect(),
            metadata: Metadata {
                width: 24,
                height: 16,
                wb: [1.; 3],
                daylight_wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }
    }

    /// The AI source is recovered on its own: an unclipped pixel with unequal channels
    /// above a quarter of white gets its own recovery, not the plain image's.
    #[test]
    fn the_ai_source_is_recovered_on_its_own() {
        let plain = image([0.8, 0.5, 0.3]);
        let ai = image([0.78, 0.48, 0.32]);
        let expected = crate::develop::quality::recover_highlights(&ai, Cancel::NEVER).unwrap();
        let source = AiSource::recover(ai, Cancel::NEVER).unwrap();
        assert_eq!(source.image().pixels, expected.pixels);
        let plain_recovered =
            crate::develop::quality::recover_highlights(&plain, Cancel::NEVER).unwrap();
        assert_ne!(source.image().pixels, plain_recovered.pixels);
    }

    /// Off, or at Amount 0, nothing changes; at 100 the AI source is the base; a new
    /// source (a final result after a partial one) is a new base for every cache.
    #[test]
    fn amount_zero_is_off_and_a_new_source_replaces_the_cached_base() {
        let plain = Arc::new(image([0.2, 0.3, 0.4]));
        let ai = AiSource::recover(image([0.25, 0.3, 0.35]), Cancel::NEVER).unwrap();
        let mut r = Recipe::default();
        assert!(r.denoise_with(Some(&ai)).is_none());
        r.ai_denoise = AiDenoise {
            enabled: true,
            amount: 0.,
        };
        assert!(r.denoise_with(Some(&ai)).is_none());
        r.ai_denoise.amount = 1.;
        let cancel = AtomicBool::new(false);
        let mut cache = RetouchCache::default();
        let full = cache
            .denoised(&plain, r.denoise_with(Some(&ai)), Cancel::new(&cancel))
            .unwrap();
        assert!(Arc::ptr_eq(&full, ai.image()));
        r.ai_denoise.amount = 0.5;
        let half = cache
            .denoised(&plain, r.denoise_with(Some(&ai)), Cancel::new(&cancel))
            .unwrap();
        for ((h, p), a) in half
            .pixels
            .iter()
            .zip(&plain.pixels)
            .zip(&ai.image().pixels)
        {
            // Camera Raw's weights at 50: 0.70 of the luminance change, 0.75 of colour's.
            let d: [f32; 3] = std::array::from_fn(|c| a[c] - p[c]);
            let luma = (d[0] + d[1] + d[2]) / 3.;
            for c in 0..3 {
                let expected = p[c] + 0.7 * luma + 0.75 * (d[c] - luma);
                assert!((h[c] - expected).abs() < 1e-6);
            }
        }
        // The same Amount with a new source (equal pixels, another result): rebuilt.
        let next = AiSource::recover(image([0.25, 0.3, 0.35]), Cancel::NEVER).unwrap();
        assert!(next != ai);
        let rebuilt = cache
            .denoised(&plain, r.denoise_with(Some(&next)), Cancel::new(&cancel))
            .unwrap();
        assert!(!Arc::ptr_eq(&rebuilt, &half));
        // Reused while nothing changes.
        let again = cache
            .denoised(&plain, r.denoise_with(Some(&next)), Cancel::new(&cancel))
            .unwrap();
        assert!(Arc::ptr_eq(&rebuilt, &again));
    }

    /// AI Denoise off renders exactly as without the feature; on without a source (no
    /// result yet), the preview is the plain render.
    #[test]
    fn off_or_without_a_source_renders_as_today() {
        let plain = image([0.2, 0.3, 0.4]);
        let ai = AiSource::recover(image([0.25, 0.3, 0.35]), Cancel::NEVER).unwrap();
        let mut r = Recipe::default();
        let cancel = AtomicBool::new(false);
        let today =
            crate::develop::render(&plain, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
        let off = crate::develop::render_with_ai(
            &plain,
            &r.checked().unwrap(),
            Some(&ai),
            0,
            Cancel::new(&cancel),
        )
        .unwrap();
        assert_eq!(off.pixels, today.pixels);
        r.ai_denoise.enabled = true;
        let waiting = crate::develop::render_with_ai(
            &plain,
            &r.checked().unwrap(),
            None,
            0,
            Cancel::new(&cancel),
        )
        .unwrap();
        assert_eq!(waiting.pixels, today.pixels);
        let on = crate::develop::render_with_ai(
            &plain,
            &r.checked().unwrap(),
            Some(&ai),
            0,
            Cancel::new(&cancel),
        )
        .unwrap();
        assert_ne!(on.pixels, today.pixels);
    }

    #[test]
    fn amount_weights_follow_camera_raws_blend() {
        for (a, l, c) in [
            (0., 0., 0.),
            (0.25, 0.4375, 0.5),
            (0.5, 0.7, 0.75),
            (0.75, 0.85, 0.875),
            (1., 1., 1.),
        ] {
            assert!((weight(&LUMA_WEIGHTS, a) - l).abs() < 1e-6, "{a}");
            assert!((weight(&CHROMA_WEIGHTS, a) - c).abs() < 1e-6, "{a}");
        }
    }
}
