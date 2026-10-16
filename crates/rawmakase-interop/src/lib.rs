//! File formats and presets over the model: Adobe XMP settings and packets,
//! Lightroom's Develop settings, native and XMP presets, raw defaults, EXIF and
//! JPEG segments, and export settings. Like the model, it builds
//! without LibRaw, the renderer, wgpu or the GUI.
use rawmakase_model::{
    camera_data, camera_profiles, color, lens, metadata, model, storage, tiff, xml,
};
#[cfg(test)]
use rawmakase_model::{dng, optics};

pub mod exif;
pub mod export_settings;
pub mod isobmff;
pub mod jpeg;
pub mod lr_develop;
pub mod presets;
pub mod raw_defaults;
pub mod riff;
pub mod xmp;
