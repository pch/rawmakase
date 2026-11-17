//! RAWmakase's desktop application and command line, composed over the workspace's
//! domain crates.
//!
//! The crates' modules are re-exported at this root, so paths read the same on
//! either side: what an edit is, as values ([`model`], [`camera_profiles`], [`lens`],
//! [`optics`], [`storage`]…, crates/rawmakase-model); the file formats and presets
//! over them, and which edit a photo develops with ([`xmp`], [`presets`],
//! [`raw_defaults`], [`edits`], crates/rawmakase-interop); the catalog ([`catalog`],
//! crates/rawmakase-catalog); rendering ([`develop`], crates/rawmakase-engine);
//! opening and decoding photos with RAWmakase's own RAW decoders and colour
//! management ([`raw`], [`photo`], [`color_management`], crates/rawmakase-decode);
//! and exports ([`export`], crates/rawmakase-export).
//!
//! AI Denoise's service, shared by the app and the command line, is [`ai_denoise`]
//! (crates/rawmakase-denoise), with `model_install`, the model pack it runs.
//!
//! This crate's own modules compose those APIs: [`app`] is the desktop editor,
//! `edit_session` keeps an open photo's edits, history and saved state together,
//! `catalog_session` holds the open catalog, [`comparison`] provides
//! reference-image validation and `platform` isolates OS integration.
//!
//! The repository's `docs/code-map.md` maps implementation files and runtime flows,
//! and `docs/architecture.md` records ownership rules. This library exists for the
//! RAWmakase binary, its examples and its tests; it is not a stable public API.
// The values a photo's edit is made of, built on their own (crates/rawmakase-model).
// The catalog (crates/rawmakase-catalog).
pub use rawmakase_catalog::catalog;
// Rendering (crates/rawmakase-engine).
pub use rawmakase_engine::develop;
// Developing and writing exports, with their watermarks (crates/rawmakase-export).
pub use rawmakase_export::{export, watermark};
// RAW decoding, colour management, demosaicing, opening and decoding a photo
// (crates/rawmakase-decode), and its development tools. Its `ai::denoise` is
// reached by its crate path: this crate's own `ai_denoise` is the service.
pub use rawmakase_decode::ai::cache as ai_cache;
pub use rawmakase_decode::dev::{decode_dump, fingerprint, half_check};
pub use rawmakase_decode::{camera_look, color_management, decode_cache, photo, raw};
pub(crate) use rawmakase_decode::{decode, raster};
// AI Denoise's service, shared by the app and the command line, and the model pack
// it runs (crates/rawmakase-denoise).
pub use rawmakase_denoise as ai_denoise;
pub(crate) use rawmakase_denoise::model_install;
// File formats and presets over the model, and which edit a photo develops with
// (crates/rawmakase-interop).
pub use rawmakase_interop::{
    edits, exif, export_settings, jpeg, lr_develop, presets, raw_defaults, xmp,
};
pub(crate) use rawmakase_model::time;
pub use rawmakase_model::{
    camera_data, camera_profiles, cameras, color, dng, ids, lens, metadata, model, optics,
    rendered, storage, tiff, xml,
};
pub mod app;
pub mod catalog_command;
pub(crate) mod catalog_session;
pub mod comparison;
pub mod diagnostics;
pub(crate) mod edit_session;
pub mod lensfun_install;
pub(crate) mod platform;
pub mod render_command;
/// For headless commands: whether a path's volume is mounted.
pub use platform::volume::mounted as volume_mounted;
/// For `rawmakase extensions remove`: deletes the keys extension `id` left
/// in the system's credential store when it was removed, opening the store
/// only when there are some.
pub fn delete_forgotten_keys(data_dir: &std::path::Path, id: &str) -> anyhow::Result<()> {
    platform::credentials::delete_forgotten_keys(
        platform::credentials::system_store,
        &rawmakase_extensions::store::Store::new(data_dir),
        id,
    )
}
pub mod updates;

#[cfg(test)]
mod tests {
    /// Export finds the same installed-font folders as the interface without egui.
    #[test]
    fn watermarks_read_the_fonts_the_interface_does() {
        assert_eq!(
            rawmakase_export::watermark::font_dirs::font_directories(),
            fastframe_fonts::system::font_directories()
        );
    }

    /// Exports name the app's release, though the export crate has its own version.
    #[test]
    fn exports_name_this_release() {
        assert_eq!(
            rawmakase_export::build_info::SOFTWARE,
            concat!("RAWmakase ", env!("CARGO_PKG_VERSION"))
        );
    }
}
