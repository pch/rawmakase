//! Opening and decoding photos: RAW files read by RAWmakase's own decoders ([`raw`]),
//! colour management of what is shown and exported ([`color_management`]),
//! RAWmakase's own demosaicing (`demosaic`), opening a photo ([`photo`]) and its
//! full-size decode through the decode cache ([`decode`], [`decode_cache`]), its
//! decode and develop fingerprints for regression checks ([`fingerprint`],
//! [`decode_dump`]) and its half-size checks ([`half_check`]), and
//! opening a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL, which is not
//! developed ([`raster`]). It also holds what AI Denoise needs from a decode, with
//! no model knowledge ([`ai_denoise`], and its results' disk cache [`ai_cache`]), and
//! fitting Camera Look to a RAW's embedded JPEG ([`camera_look`]).
//!
//! No C library is built or bundled: a HEIC's pixels come from the operating
//! system's ImageIO or Windows Imaging Component on macOS and Windows, and
//! elsewhere from the system's libheif, opened at run time. Highlight recovery
//! after a decode is the renderer's (crates/rawmakase-engine).
use rawmakase_engine::develop;
use rawmakase_model::{camera_data, camera_profiles, dng, lens, storage};

pub mod ai_cache;
pub mod ai_denoise;
pub mod camera_look;
pub mod color_management;
pub mod decode;
pub mod decode_cache;
pub mod decode_dump;
mod demosaic;
pub mod fingerprint;
pub mod half_check;
pub mod photo;
pub mod raster;
pub mod raw;
