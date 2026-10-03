//! Namespace-aware Adobe settings parsing and application to a develop recipe.
mod apply;
pub mod descriptive;
pub mod local;
pub(crate) mod look;
pub mod ns;
mod parse;
pub mod write;
pub(crate) mod xml;
use crate::develop::curve::ToneCurve;
pub use parse::parse;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub group: String,
    pub path: PathBuf,
    pub settings: BTreeMap<String, String>,
    pub curves: BTreeMap<String, ToneCurve>,
    pub look: String,
    pub blockers: Vec<String>,
    pub notes: Vec<String>,
    pub photo_settings: bool,
    /// Spot removal and masks (see `local`), as nested data.
    pub local: BTreeMap<String, local::Node>,
    /// Shipped with RAWmakase (see `presets::builtin`). Like a photo's own edit, a
    /// built-in preset's Adobe profile falls back to RAWmakase's own profile when it
    /// isn't imported.
    pub builtin: bool,
}
#[cfg(test)]
mod tests;
