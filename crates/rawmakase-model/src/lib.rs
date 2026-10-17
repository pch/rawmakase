//! What a photo's edit is, as values: the recipe and the settings it is made of,
//! camera metadata and profiles, lens profiles, colour primitives and output pixel
//! buffers, with the file, storage and calendar helpers they need. It builds without LibRaw,
//! the renderer, wgpu or the desktop app, which the RAWmakase crate adds on top.
pub mod camera_data;
pub mod camera_profiles;
pub mod cameras;
pub mod color;
pub mod dng;
pub mod ids;
pub mod lens;
pub mod metadata;
pub mod model;
pub mod optics;
pub mod rendered;
pub mod storage;
pub use rawmakase_tiff as tiff;
pub mod time;
pub mod xml;
