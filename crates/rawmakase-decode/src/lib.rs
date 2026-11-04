//! Opening and decoding photos: RAW files read by RAWmakase's own decoders ([`raw`]),
//! colour management of what is shown and exported ([`color_management`]) and colour
//! difference ([`color_difference`]), RAWmakase's own demosaicing (`demosaic`),
//! opening a photo ([`photo`]) and its full-size decode through the decode cache
//! ([`decode`], [`decode_cache`]), and opening a JPEG, PNG, TIFF, WebP, AVIF, HEIF or
//! JPEG XL, which is not developed ([`raster`]). It also holds what AI Denoise needs
//! from a decode, with no model knowledge ([`ai`]), fitting Camera Look to a RAW's
//! embedded JPEG ([`camera_look`]), and the command line's regression and diagnosis
//! tools ([`dev`]).
//!
//! No C library is built or bundled: a HEIC's pixels come from the operating
//! system's ImageIO or Windows Imaging Component on macOS and Windows, and
//! elsewhere from the system's libheif, opened at run time. Highlight recovery
//! after a decode is the renderer's (crates/rawmakase-engine).
use rawmakase_engine::develop;
use rawmakase_model::{camera_data, camera_profiles, dng, lens, optics, storage, tiff};

pub mod ai;
pub mod camera_look;
pub mod color_difference;
pub mod color_management;
pub mod decode;
pub mod decode_cache;
mod demosaic;
pub mod dev;
pub mod photo;
pub mod raster;
pub mod raw;
