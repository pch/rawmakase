//! The catalog: RAWmakase's SQLite database of photos, folders, collections and
//! edits, and Lightroom catalog import. It stores the records that
//! `rawmakase_interop::edits` resolves into the edit a photo develops with. Like the
//! model and file formats below it, it builds without the RAW decoders, the
//! renderer, wgpu or the GUI.
use rawmakase_fs as fsops;
use rawmakase_interop::{edits, exif, export_settings, isobmff, jpeg, riff, xmp};
use rawmakase_model::{ids, metadata, model, storage, tiff, time, xml};
#[cfg(test)]
use {
    rawmakase_interop::{lr_develop, presets, raw_defaults},
    rawmakase_model::{camera_data, camera_profiles, optics},
};

pub mod catalog;
