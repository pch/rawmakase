//! Lightroom's Red Eye Correction: an ellipse over a pupil whose color is replaced by
//! a dark neutral, with Pupil Size and Darken sliders, or for Pet Eye by black with
//! an optional catchlight. Corrections are stored as
//! parameters beside the recipe, like spot removal, and render on the linear camera
//! image before Heal and Clone, so every later edit and export sees them.
//!
//! Positions are in image space (see [`crate::model::image_frame::ImageFrame`]); sizes are
//! fractions of the photo's long edge. The ellipse is Lightroom's: semi-axes along the
//! image's x and y and their correlation (`crs:RedEyeInfo`'s `width`, `height` and
//! `alpha`), not a rotation angle. The corrections themselves are
//! [`crate::model::red_eye`]'s.
mod detect;
mod render;

use crate::model::red_eye::EyeKind;

pub use detect::{Glow, find_pupil};
pub(crate) use render::Placed;

/// What a pupil of this kind glows like, for finding it.
pub trait PupilGlow {
    /// What the pupil glows like, for finding it.
    fn glow(self) -> Glow;
}
impl PupilGlow for EyeKind {
    fn glow(self) -> Glow {
        match self {
            EyeKind::Red => Glow::Red,
            EyeKind::Pet { .. } => Glow::Bright,
        }
    }
}

#[cfg(test)]
mod tests;
