//! RAWmakase's film looks: creative looks modelled from Kodak's, Fujifilm's and
//! Agfa's published film and paper datasheets by `scripts/film/film-look.py`,
//! embedded from `assets/looks`.
//! Each is an ordinary look profile with an RGB table, so it also works in
//! Lightroom. They always go over RAWmakase Standard, never over an imported Adobe
//! Standard, so a film look renders the same on every system.
use super::{CameraProfile, enhanced::LookFile};
use crate::camera_data::Metadata;
use std::sync::OnceLock;

macro_rules! files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../../../../assets/looks/", $path)))),*]
    };
}
/// Every file in `assets/looks`, listed by `scripts/film/film-look.py --all`.
pub(super) const FILES: &[(&str, &str)] = include!("film_files.in");

/// The looks, parsed once: their tables are large. A file that fails to parse is a
/// bug the tests catch; at run time it is left out.
pub(super) fn looks() -> &'static [LookFile] {
    static LOOKS: OnceLock<Vec<LookFile>> = OnceLock::new();
    LOOKS.get_or_init(|| {
        FILES
            .iter()
            .filter_map(|(_, text)| LookFile::parse(text).ok())
            .collect()
    })
}

/// The film looks for this camera over RAWmakase Standard, or none without a usable
/// colour matrix.
pub fn profiles(m: &Metadata) -> Vec<CameraProfile> {
    let Some(base) = super::open::standard(m) else {
        return Vec::new();
    };
    looks()
        .iter()
        .filter_map(|look| look.compose(&base).ok())
        .collect()
}

/// The group the film looks and their presets are listed under.
pub const GROUP: &str = "RMKS Film";

/// The variants a film look's name ends with: printed, pushed or pulled.
const VARIANTS: &[&str] = &[" Print +1", " Print -1", " Print", " +1", " -1"];

/// For one of the film looks, the stock it is a variant of and its name within the
/// group, e.g. ("Portra-ish 400", "Portra-ish 400 Print"); `None` for any other
/// profile.
pub fn listing(profile: &CameraProfile) -> Option<(&str, &str)> {
    let title = profile.name.strip_prefix(GROUP)?.strip_prefix(": ")?;
    profile
        .enhanced
        .as_ref()
        .filter(|look| look.base_name == super::open::STANDARD)?;
    let stock = VARIANTS
        .iter()
        .find_map(|variant| title.strip_suffix(variant))
        .unwrap_or(title);
    Some((stock, title))
}
