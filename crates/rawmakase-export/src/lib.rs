//! Export: developing photos with the renderer (crates/rawmakase-engine), from
//! their full-size decode (crates/rawmakase-native), and writing JPEG and 16-bit
//! TIFF files with their metadata and text or image watermarks. It builds without
//! the desktop app.
#[cfg(test)]
use rawmakase_catalog::catalog;
use rawmakase_catalog::edits;
use rawmakase_engine::develop;
use rawmakase_interop::{exif, export_settings, jpeg, raw_defaults, xmp};
use rawmakase_model::{
    camera_data, camera_profiles, metadata, model, rendered, storage, tiff, time, xml,
};
use rawmakase_native::{color_management, decode, photo, raw};

pub mod build_info;
pub mod export;
pub mod watermark;
