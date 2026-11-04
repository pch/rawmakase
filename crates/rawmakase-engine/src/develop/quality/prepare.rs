//! What every render of a recipe derives from it before its stages, in one place and
//! one order: the recipe the stages read ([`Recipe::resolved`]: switched-off panels
//! bypassed, a look's settings layered in, the default profile) and the photo's
//! full-resolution geometry under it, both from the decoded image alone; then the
//! source the stages sample, once the caller has checked its output against that
//! geometry and released what a new source replaces.
use super::{Retouched, retouched};
use crate::camera_data::CameraImage;
use crate::develop::{self, Geometry};
use crate::model::{recipe::Recipe, valid::ValidRecipe};
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::borrow::Cow;

/// A recipe resolved for one photo, and the photo's geometry under it.
pub(crate) struct Resolved<'a> {
    /// What every stage reads, retouching and geometry included.
    pub(crate) recipe: Cow<'a, Recipe>,
    /// The full-resolution geometry: the output size, crop and transform the render's
    /// regions are checked against. A retouched source keeps the decoded image's size
    /// and metadata, so this is its geometry too.
    pub(crate) geometry: Geometry,
}

/// `recipe`, checked, resolved for `image`, and `image`'s full-resolution geometry
/// under it. Pure: it reads and caches nothing.
pub(crate) fn resolve<'a>(image: &CameraImage, recipe: &'a ValidRecipe<'_>) -> Resolved<'a> {
    let recipe = recipe.resolved(&image.metadata);
    let geometry = Geometry::new(image, &recipe, 0);
    Resolved { recipe, geometry }
}

impl Resolved<'_> {
    /// The source the stages sample: the camera check (a profile made for another
    /// camera is an error), Auto CA's measurement primed, and the recovered image with
    /// noise reduction, AI Denoise, red eye and spots (`retouched`), from `cache` when
    /// there is one.
    pub(crate) fn source(
        &self,
        image: &CameraImage,
        ai: Option<&develop::AiSource>,
        cancel: Cancel<'_>,
        cache: Option<&mut develop::retouch::RetouchCache>,
    ) -> Result<Retouched> {
        if let Some(p) = &self.recipe.profile {
            p.ensure_camera(&image.metadata)?;
        }
        if self.recipe.lens_ca {
            crate::lens::auto_ca::prime(image);
        }
        retouched(image, &self.recipe, ai, cancel, cache)
    }
}
