//! The native boundary: RAW files read by RAWmakase's own decoders ([`raw`]),
//! colour management of what is shown and exported ([`color_management`]),
//! RAWmakase's own demosaicing ([`demosaic`]), opening a photo ([`photo`]) and its
//! full-size decode through the decode cache ([`decode`], [`decode_cache`]), its
//! decode and develop fingerprints for regression checks ([`fingerprint`],
//! [`decode_dump`]) and its half-size checks ([`half_check`]), and
//! opening a JPEG, PNG, TIFF, WebP, AVIF, HEIF or JPEG XL, which is not
//! developed ([`raster`]). Everything here is Rust: no C library is built or
//! linked. Highlight recovery after a decode is the renderer's
//! (crates/rawmakase-engine).
use rawmakase_engine::develop;
use rawmakase_model::{camera_data, camera_profiles, dng, lens, storage};

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
