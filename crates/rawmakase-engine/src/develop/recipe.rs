//! What rendering makes of a recipe's settings: the measured manual Vignetting and
//! Color noise reduction it runs on the camera image. The recipe itself is
//! [`crate::model::recipe`]'s.
use crate::model::recipe::Recipe;

/// What rendering makes of a recipe's settings, as it runs them on the camera image.
pub(crate) trait RenderedRecipe {
    /// Manual lens Vignetting as measured in Camera Raw, applied with the lens
    /// profile's to the camera image; `None` at Amount 0.
    fn manual_vignette(&self) -> Option<crate::develop::effects::ManualVignette>;
    /// The measured Color noise reduction to run on the camera image; `None` at
    /// Amount 0.
    fn chroma_denoise(&self) -> Option<crate::develop::color_noise::ChromaDenoise>;
}
impl RenderedRecipe for Recipe {
    fn manual_vignette(&self) -> Option<crate::develop::effects::ManualVignette> {
        crate::develop::effects::ManualVignette::new(
            self.effects.lens_vignette,
            self.effects.lens_vignette_midpoint,
        )
    }
    fn chroma_denoise(&self) -> Option<crate::develop::color_noise::ChromaDenoise> {
        crate::develop::color_noise::ChromaDenoise::new(
            self.noise_chroma,
            self.effects.chroma_detail,
            self.effects.chroma_smoothness,
        )
    }
}
