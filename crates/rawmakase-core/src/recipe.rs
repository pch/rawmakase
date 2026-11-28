//! A photo's edit, as a value of the library's own: the develop settings,
//! changed through the library's rules so that what depends on a setting
//! follows it.
use crate::{Error, Result};
use rawmakase_model::color::curve::ToneCurve;
use rawmakase_model::model::recipe::Recipe as Settings;

/// A photo's edit: every develop setting, as Lightroom and Camera Raw mean
/// them. Made for a photo by [`Photo::starting_recipe`](crate::Photo::starting_recipe),
/// [`Photo::apply_preset`](crate::Photo::apply_preset) and
/// [`Context::load_recipe`](crate::Context::load_recipe), and rendered and
/// exported with that photo.
///
/// Its settings are changed through the library's rules, which keep what
/// depends on them consistent: a slider with [`Photo::set`](crate::Photo::set),
/// so that the white balance the renderer multiplies by follows Temp and
/// Tint, the profile with [`Photo::set_profile`](crate::Photo::set_profile),
/// and the point curve through [`Recipe::curve_mut`]'s methods. The rest
/// of the edit (masks, crop, retouching) joins as the library gives it
/// rules of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe(pub(crate) Settings);

impl Recipe {
    /// The point curve, applied after the tone sliders.
    pub fn curve(&self) -> &ToneCurve {
        &self.0.curve
    }

    /// The point curve, to change: [`ToneCurve::insert`], `move_point` and
    /// `remove` keep its points in order and within its limits. Its fields
    /// can be set too, and [`Recipe::validate`] then refuses points out of
    /// order.
    pub fn curve_mut(&mut self) -> &mut ToneCurve {
        &mut self.0.curve
    }

    /// The name of the camera profile the recipe renders with, one of
    /// [`Photo::profiles`](crate::Photo::profiles) when the recipe was made
    /// for that photo: `Embedded` for a rendered file's own colors, and
    /// `None` for a RAW developed with its camera's matrix alone. Chosen with
    /// [`Photo::set_profile`](crate::Photo::set_profile).
    pub fn profile(&self) -> Option<&str> {
        self.0.profile.as_deref().map(|p| p.name.as_str())
    }

    /// Whether the edit is in HDR, as the desktop's HDR switch in the Basic
    /// panel turns it on. An HDR export needs it.
    pub fn hdr(&self) -> bool {
        self.0.hdr.is_active()
    }

    /// Turns HDR on or off. The HDR settings are kept while it is off, as
    /// the desktop keeps them.
    pub fn set_hdr(&mut self, on: bool) {
        self.0.hdr.enabled = on;
    }

    /// Whether the recipe renders: each setting within its range, the curve's
    /// points in order, the masks complete. The renderer and an export check
    /// it first; [`ErrorKind::Invalid`](crate::ErrorKind::Invalid) says what
    /// is wrong.
    pub fn validate(&self) -> Result<()> {
        self.0.validate().map_err(invalid)
    }

    /// Whether the edit renders AI Denoise, which the library doesn't run
    /// yet: [`Parameter::AiDenoiseAmount`](crate::Parameter::AiDenoiseAmount)
    /// at 0 turns it off.
    pub fn uses_ai_denoise(&self) -> bool {
        self.0.as_rendered().ai_denoise.renders()
    }
}

pub(crate) fn invalid(e: anyhow::Error) -> Error {
    Error::of(rawmakase_model::error::Invalid::from_error(e).into())
}
