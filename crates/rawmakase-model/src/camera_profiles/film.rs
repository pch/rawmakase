//! RAWmakase's film looks: creative looks modelled from Kodak's published film and
//! paper datasheets by `scripts/film/film-look.py`, embedded from `assets/looks`.
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
/// Every file in `assets/looks`.
pub(super) const FILES: &[(&str, &str)] = files![
    "RMKS Film Portra-ish 400.xmp",
    "RMKS Film Portra-ish 400 Print.xmp",
    "RMKS Film Kodachrome-ish 64.xmp",
    "RMKS Film Kodachrome-ish 64 Print.xmp",
];

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
