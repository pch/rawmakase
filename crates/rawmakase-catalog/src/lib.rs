//! The catalog: RAWmakase's SQLite database of photos, folders, collections and
//! edits, Lightroom catalog import, and which edit a photo develops with. Like the
//! model and file formats below it, it builds without LibRaw, the renderer, wgpu
//! or the GUI.
use rawmakase_interop::{
    exif, export_settings, isobmff, jpeg, lr_develop, raw_defaults, riff, xmp,
};
use rawmakase_model::{camera_data, camera_profiles, ids, metadata, model, storage, tiff, xml};
#[cfg(test)]
use {rawmakase_interop::presets, rawmakase_model::optics};

pub mod catalog;
pub mod edits;
