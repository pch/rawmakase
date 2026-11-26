//! What rendering makes of a recipe's settings: the measured manual Vignetting and
//! color and luminance noise reduction it runs on the camera image. The recipe
//! itself is [`crate::model::recipe`]'s.
use crate::model::recipe::Recipe;
use rawmakase_cancel::Cancel;

/// What rendering makes of a recipe's settings, as it runs them on the camera image.
pub(crate) trait RenderedRecipe {
    /// Manual lens Vignetting as measured in Camera Raw, applied with the lens
    /// profile's to the camera image; `None` at Amount 0.
    fn manual_vignette(&self) -> Option<crate::develop::effects::ManualVignette>;
    /// Color and luminance noise reduction on the camera image; `None` when both
    /// amounts are zero. Without AI Denoise.
    #[cfg(test)]
    fn denoise(&self) -> Option<Denoise> {
        self.denoise_with(None)
    }
    /// AI Denoise's blend with `ai` (the photo's source, when the caller has one and AI
    /// Denoise renders), then Color and luminance noise reduction; `None` when there is
    /// nothing to do.
    fn denoise_with(&self, ai: Option<&crate::develop::AiSource>) -> Option<Denoise>;
}
impl RenderedRecipe for Recipe {
    fn manual_vignette(&self) -> Option<crate::develop::effects::ManualVignette> {
        crate::develop::effects::ManualVignette::new(
            self.effects.lens_vignette,
            self.effects.lens_vignette_midpoint,
        )
    }
    fn denoise_with(&self, ai: Option<&crate::develop::AiSource>) -> Option<Denoise> {
        let ai = ai.filter(|_| self.ai_denoise.renders()).map(|source| {
            crate::develop::ai_denoise::AiBlend {
                source: source.clone(),
                amount: self.ai_denoise.amount.min(1.),
            }
        });
        let chroma = crate::develop::color_noise::ChromaDenoise::new(
            self.noise_chroma,
            self.effects.chroma_detail,
            self.effects.chroma_smoothness,
        );
        let luminance = crate::develop::luminance_noise::LuminanceDenoise::new(
            self.noise_luma,
            self.effects.luma_detail,
            self.effects.luma_contrast,
        );
        (ai.is_some() || chroma.is_some() || luminance.is_some()).then_some(Denoise {
            ai,
            chroma,
            luminance,
        })
    }
}

/// All noise settings and their execution, shared by cached previews and exports: AI
/// Denoise's blend (with the source captured when the render started), then Color and
/// luminance noise reduction.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Denoise {
    ai: Option<crate::develop::ai_denoise::AiBlend>,
    chroma: Option<crate::develop::color_noise::ChromaDenoise>,
    luminance: Option<crate::develop::luminance_noise::LuminanceDenoise>,
}
impl Denoise {
    /// `source` (the plain image, highlights recovered) with all of it applied.
    pub(crate) fn apply(
        &self,
        source: &std::sync::Arc<crate::camera_data::CameraImage>,
        cancel: Cancel<'_>,
    ) -> anyhow::Result<std::sync::Arc<crate::camera_data::CameraImage>> {
        use std::sync::Arc;
        let base = match &self.ai {
            Some(ai) => ai.apply(source),
            None => source.clone(),
        };
        anyhow::ensure!(!cancel.is_cancelled(), "Render superseded");
        let chroma = self.chroma.map(|d| d.apply(&base, cancel)).transpose()?;
        if let Some(luminance) = self.luminance {
            let mut image = match chroma {
                Some(image) => image,
                None => match Arc::try_unwrap(base) {
                    Ok(image) => image,
                    Err(shared) => return Ok(Arc::new(luminance.apply(&shared, cancel)?)),
                },
            };
            luminance.apply_in_place(&mut image, cancel)?;
            Ok(Arc::new(image))
        } else {
            Ok(chroma.map(Arc::new).unwrap_or(base))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{camera_data::CameraImage, develop::retouch::RetouchCache};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn denoise_cache_recovers_after_cancellation_and_source_replacement() {
        let source = Arc::new(CameraImage {
            width: 65,
            height: 67,
            pixels: (0..65 * 67)
                .map(|i| [0.1 + (i % 7) as f32 * 0.002; 3])
                .collect(),
            metadata: Default::default(),
            recovered: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        });
        let mut r = Recipe {
            noise_luma: 0.5,
            noise_chroma: 0.,
            ..Default::default()
        };
        let mut cache = RetouchCache::default();
        let cancel = AtomicBool::new(false);
        let first = cache
            .denoised(&source, r.denoise(), Cancel::new(&cancel))
            .unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &cache
                .denoised(&source, r.denoise(), Cancel::new(&cancel))
                .unwrap()
        ));
        r.noise_luma = 1.;
        cancel.store(true, Ordering::Relaxed);
        assert!(
            cache
                .denoised(&source, r.denoise(), Cancel::new(&cancel))
                .is_err()
        );
        cancel.store(false, Ordering::Relaxed);
        let retried = cache
            .denoised(&source, r.denoise(), Cancel::new(&cancel))
            .unwrap();
        assert_eq!(
            retried.pixels,
            r.denoise()
                .unwrap()
                .apply(&source, Cancel::new(&cancel))
                .unwrap()
                .pixels
        );
        let mut replacement = (*source).clone();
        replacement.pixels.fill([0.3; 3]);
        let replacement = Arc::new(replacement);
        let replaced = cache
            .denoised(&replacement, r.denoise(), Cancel::new(&cancel))
            .unwrap();
        assert_eq!(
            replaced.pixels,
            r.denoise()
                .unwrap()
                .apply(&replacement, Cancel::new(&cancel))
                .unwrap()
                .pixels
        );
        r.noise_luma = 0.;
        assert!(Arc::ptr_eq(
            &replacement,
            &cache
                .denoised(&replacement, r.denoise(), Cancel::new(&cancel))
                .unwrap()
        ));
    }

    #[test]
    fn denoise_does_not_mutate_shared_source_and_matches_with_chroma_in_place() {
        let original_pixels: Vec<[f32; 3]> = (0..65 * 67)
            .map(|i| {
                let x = (i % 65) as f32;
                let y = (i / 65) as f32;
                [
                    0.12 + ((i * 7) % 11) as f32 * 0.004,
                    0.10 + ((x as usize * 3 + y as usize * 5) % 9) as f32 * 0.003,
                    0.09 + ((i * 13) % 7) as f32 * 0.005,
                ]
            })
            .collect();
        let source = Arc::new(CameraImage {
            width: 65,
            height: 67,
            pixels: original_pixels.clone().into(),
            metadata: crate::camera_data::Metadata {
                wb: [1.; 3],
                ..Default::default()
            },
            recovered: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        });
        let luma_only = Recipe {
            noise_luma: 0.6,
            noise_chroma: 0.,
            ..Default::default()
        };
        let out_luma = luma_only
            .denoise()
            .unwrap()
            .apply(&source, Cancel::NEVER)
            .unwrap();
        assert_eq!(source.pixels, original_pixels);
        assert_ne!(out_luma.pixels, original_pixels);

        let both = Recipe {
            noise_luma: 0.6,
            noise_chroma: 0.4,
            ..Default::default()
        };
        let out_both = both
            .denoise()
            .unwrap()
            .apply(&source, Cancel::NEVER)
            .unwrap();
        assert_eq!(source.pixels, original_pixels);
        let chroma_only = Recipe {
            noise_luma: 0.,
            noise_chroma: 0.4,
            ..Default::default()
        }
        .denoise()
        .unwrap()
        .apply(&source, Cancel::NEVER)
        .unwrap();
        let expected_both = luma_only
            .denoise()
            .unwrap()
            .apply(&chroma_only, Cancel::NEVER)
            .unwrap();
        assert_eq!(out_both.pixels, expected_both.pixels);
    }
}
