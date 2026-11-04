//! What applying settings measures on the decoded photo when they ask for Auto.

use crate::{camera_data::Metadata, model::recipe::Recipe};

/// What applying settings measures on the decoded photo when they ask for Auto:
/// Lightroom's Auto Settings (tone), Auto white balance and Auto black & white mix.
/// The renderer provides it (`develop::Measures`), so applying settings (in
/// `rawmakase_interop::xmp`) needs no rendering code of its own, and an Auto in
/// settings is the one the app runs.
pub trait PhotoMeasures {
    /// `base` with the six Tone sliders, Vibrance and Saturation Auto Settings picks.
    fn auto_tone(&self, base: &Recipe) -> anyhow::Result<Recipe>;
    /// `base` with the white balance the WB menu's Auto picks.
    fn auto_white_balance(&self, base: &Recipe) -> anyhow::Result<Recipe>;
    /// The Auto black & white mix for `r`, in slider units.
    fn auto_gray_mix(&self, r: &Recipe, m: &Metadata) -> [f32; 8];
}
