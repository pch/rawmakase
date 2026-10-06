//! Headless RAW development: recipes, geometry, color and detail rendering.
mod auto;
mod basic_tone;
mod basic_tone_data;
pub mod black_white;
pub(crate) mod calibration;
pub mod clarity;
pub(crate) mod color;
pub mod color_grade;
mod color_grade_curves;
mod color_grade_data;
pub mod color_mixer;
pub mod color_noise;
mod crop_constraint;
// Develop code names the curve primitives as `develop::curve`, where they began.
use crate::color::curve;
pub mod defaults;
pub mod effects;
mod geometry;
pub mod gpu;
pub mod guided;
mod image_space;
mod local_tone;
mod local_tone_data;
pub mod lut3d;
pub mod masks;
mod orientation;
pub mod panels;
pub mod parametric;
pub mod params;
mod pipeline;
pub mod point_color;
mod preview_renderer;
mod pyramid;
pub mod settings_groups;
pub mod sharpening;
pub mod targeted;
pub mod texture;
pub mod upright;
pub use pipeline::GamutModel;
pub use preview_renderer::PreviewRenderer;
pub mod quality;
mod recipe;
pub mod red_eye;
mod rendered;
pub mod retouch;
mod stage_cache;
mod white_balance;

pub use crate::color::{mul, srgb_encode};
pub use auto::{
    AutoTone, auto_tone, auto_tone_basis, auto_tone_cancellable, auto_white_balance,
    auto_white_balance_cancellable,
};
pub use basic_tone::{ContrastModel, WhitesModel};
pub use black_white::{AutoMix, ColorSpread, Treatment, is_monochrome};
pub use geometry::{Geometry, Transform, Upright, UprightGuide, UprightMode, display_axes};
pub use image_space::{ImageFrame, ViewMapping};
pub use orientation::{Mirror, QuarterTurn, mirror, turn};
pub use pipeline::{
    neutral_pick, pick_fringe, preview, render, render_legacy, render_region, render_region_legacy,
};
pub(crate) use pipeline::{profile_matrix, render_base};
pub use recipe::{
    EXPOSURE_LIMIT, LocalEdits, ProfileCorrections, ProfilePreference, Recipe, TEMPERATURE_MAX,
    TEMPERATURE_MIN, TINT_LIMIT, camera_matching_names, camera_matching_profile,
};
pub(crate) use rendered::unit_to_u8;
pub use rendered::{ClipOverlay, Clipped, HIGHLIGHT_CLIP, Histogram, Rendered, SHADOW_CLIP};
pub use white_balance::{NamedWhiteBalance, TemperatureTint};
