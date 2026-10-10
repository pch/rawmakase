use super::masks::{
    LocalDelta, LocalMath, MaskWeights,
    local::{self, slot},
};
use super::{Geometry, mul, srgb_encode};
use crate::rendered::Rendered;
use crate::{
    camera_data::{CameraImage, Metadata},
    develop::curve::{CurveLut, refine_saturation},
};
use crate::{
    color::srgb_decode,
    model::{recipe::Recipe, valid::ValidRecipe},
};
use anyhow::{Result, ensure};
use rayon::prelude::*;
use std::sync::Arc;

mod color;
mod fields;
mod gpu_params;
mod pickers;
mod pixel;
mod render;
mod sampling;
mod tone;
pub(crate) use color::*;
pub(crate) use fields::*;
pub(crate) use gpu_params::*;
pub use pickers::*;
pub(crate) use pixel::*;
pub use render::*;
pub use sampling::*;
pub(crate) use tone::*;

pub(crate) mod pixel_params;
#[cfg(test)]
mod tests;
