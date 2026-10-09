//! RAWmakase's domain APIs and desktop application.
//!
//! Start with [`raw`] for decoding and native color management, [`camera_profiles`]
//! for DCP transforms, and [`develop`] for recipes and rendering. [`xmp`] translates
//! Adobe settings; [`presets`] manages reusable native and XMP presets.
//!
//! [`storage`] owns file identity and paths, [`catalog`] owns legacy sidecar import,
//! the photo database, edits and Lightroom import, and [`export`] writes finished images.
//! [`app`] composes these APIs into the desktop editor; [`comparison`] provides
//! reference-image validation and [`platform`] isolates OS integration.
//!
//! The repository's `docs/code-map.md` maps implementation files and runtime flows,
//! and `docs/architecture.md` records ownership rules. This library exists for the
//! RAWmakase binary, its examples and its tests; it is not a stable public API.
// The values a photo's edit is made of, built on their own (crates/rawmakase-model).
// The catalog and which edit a photo develops with (crates/rawmakase-catalog).
pub use rawmakase_catalog::{catalog, edits};
// Rendering (crates/rawmakase-engine).
pub use rawmakase_engine::develop;
// Developing and writing exports, with their watermarks (crates/rawmakase-export).
pub use rawmakase_export::{export, watermark};
// LibRaw and Little CMS, demosaicing, opening and decoding a photo
// (crates/rawmakase-native).
pub(crate) use rawmakase_native::decode;
pub use rawmakase_native::{decode_cache, photo, raw};
// File formats and presets over the model (crates/rawmakase-interop).
pub use rawmakase_interop::{exif, export_settings, jpeg, lr_develop, presets, raw_defaults, xmp};
pub(crate) use rawmakase_model::time;
pub use rawmakase_model::{
    camera_data, camera_profiles, cameras, color, dng, ids, lens, metadata, model, optics,
    rendered, storage, tiff, xml,
};
pub mod app;
pub(crate) mod catalog_session;
pub mod comparison;
pub(crate) mod edit_session;
pub(crate) mod platform;
pub mod updates;

#[cfg(test)]
mod tests {
    /// Exports name the app's release, though the export crate has its own version.
    #[test]
    fn exports_name_this_release() {
        assert_eq!(
            rawmakase_export::build_info::SOFTWARE,
            concat!("RAWmakase ", env!("CARGO_PKG_VERSION"))
        );
    }
}
