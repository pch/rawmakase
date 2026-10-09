//! Headless RAW development: recipes, geometry, color and detail rendering.
mod auto;
mod basic_tone;
mod basic_tone_data;
pub mod black_white;
pub(crate) mod calibration;
pub mod clarity;
pub mod color;
pub mod color_grade;
mod color_grade_curves;
pub mod color_mixer;
pub mod color_noise;
mod crop_constraint;
// Develop code names the curve primitives as `develop::curve`, where they began.
use crate::color::curve;
pub mod effects;
mod geometry;
pub mod gpu;
pub mod guided;
mod image_space;
mod local_tone;
mod local_tone_data;
pub mod masks;
mod orientation;
pub mod parametric;
mod pipeline;
pub mod point_color;
mod preview_renderer;
mod pyramid;
pub mod sharpening;
pub mod targeted;
pub mod texture;
pub mod upright;
pub use preview_renderer::PreviewRenderer;
pub mod quality;
mod recipe;
pub mod red_eye;
pub mod retouch;
mod stage_cache;

pub use crate::color::{mul, srgb_encode};
pub use auto::{
    AutoTone, Measures, auto_tone, auto_tone_basis, auto_tone_cancellable, auto_white_balance,
    auto_white_balance_cancellable,
};
pub use black_white::{AutoMix, ColorSpread};
pub use geometry::{Geometry, rendered_crop};
pub use image_space::ViewMapping;
pub use orientation::{Mirror, QuarterTurn, mirror, turn};
pub use pipeline::{neutral_pick, pick_fringe, preview, render, render_cancellable, render_region};
pub(crate) use pipeline::{profile_matrix, render_base};
