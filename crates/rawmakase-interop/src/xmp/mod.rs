//! Namespace-aware Adobe settings parsing and application to a develop recipe.
mod apply;
pub(crate) use apply::AUTO_TONE_KEYS;
pub mod descriptive;
mod filter_list;
pub mod local;
pub(crate) mod look;
mod parse;
pub mod preset_write;
pub mod write;
use crate::color::curve::ToneCurve;
pub use parse::parse;
/// RAWmakase's own settings (`xml::ns::RAWMAKASE`) are read and written under this
/// prefix among the Camera Raw settings.
pub(crate) const RAWMAKASE_PREFIX: &str = "rawmakase:";
/// The Lensfun lens an edit chose, by its identity: maker and model as Lensfun
/// identifies them, its mounts (`;`-separated), the crop factor and aspect ratio it was
/// calibrated at, the name shown and the database's timestamp. Never its calibration.
pub(crate) const LENSFUN_KEYS: [&str; 7] = [
    "rawmakase:LensfunMaker",
    "rawmakase:LensfunModel",
    "rawmakase:LensfunMounts",
    "rawmakase:LensfunCrop",
    "rawmakase:LensfunAspect",
    "rawmakase:LensfunName",
    "rawmakase:LensfunDatabase",
];
/// A fitted camera look carried whole (`camera_profiles::fitted::carried`): no
/// other application can rebuild it from its name.
pub(crate) const FITTED_PROFILE_KEY: &str = "rawmakase:FittedProfile";
/// A settings key as XMP writes it: Camera Raw's under `crs:`, RAWmakase's own as
/// they are.
pub(crate) fn qualified(key: &str) -> String {
    if key.starts_with(RAWMAKASE_PREFIX) {
        key.to_string()
    } else {
        format!("crs:{key}")
    }
}
/// The namespace declaration RAWmakase's own settings need, when `keys` have any.
pub(crate) fn rawmakase_namespace<'a>(mut keys: impl Iterator<Item = &'a str>) -> String {
    if keys.any(|k| k.starts_with(RAWMAKASE_PREFIX)) {
        format!("\n    xmlns:rawmakase=\"{}\"", crate::xml::ns::RAWMAKASE)
    } else {
        String::new()
    }
}

/// What applying settings that ask for Auto measures on the decoded photo; the
/// parameter type of this module's application functions.
pub use crate::model::measures::PhotoMeasures;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, PartialEq)]
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
mod key_table_tests;
#[cfg(test)]
mod tests;
