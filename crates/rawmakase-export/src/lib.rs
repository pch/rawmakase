//! Export: developing photos with the renderer (crates/rawmakase-engine), from
//! their full-size decode (crates/rawmakase-decode), and writing JPEG and 16-bit
//! TIFF files with their metadata and text or image watermarks. It builds without
//! the desktop app.
#[cfg(test)]
use rawmakase_catalog::catalog;
use rawmakase_decode::{ai_denoise, color_management, decode, decode_cache, photo, raw};
use rawmakase_engine::develop;
use rawmakase_interop::{edits, exif, export_settings, jpeg, raw_defaults, xmp};
use rawmakase_model::{
    camera_data, camera_profiles, metadata, model, rendered, storage, tiff, time, xml,
};

pub mod build_info;
pub mod export;
pub mod watermark;
