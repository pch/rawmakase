//! The native boundary: LibRaw through `native/raw.cpp` ([`raw`]), colour
//! management of what is shown and exported ([`color_management`]),
//! RAWmakase's own demosaicing ([`demosaic`]), opening a photo ([`photo`]) and its
//! full-size decode through the decode cache ([`decode`], [`decode_cache`]), its
//! decode and develop fingerprints for regression checks ([`fingerprint`],
//! [`decode_dump`]), and
//! opening a JPEG, PNG or TIFF, which is not developed ([`raster`]). The
//! C++ bridge is compiled and linked by this crate's build script, so nothing that
//! does not depend on it links LibRaw by accident. Highlight recovery after a
//! decode is the renderer's (crates/rawmakase-engine).
use rawmakase_engine::develop;
use rawmakase_model::{camera_data, camera_profiles, dng, lens, storage};

pub mod color_management;
pub mod decode;
pub mod decode_cache;
pub mod decode_dump;
mod demosaic;
pub mod fingerprint;
pub mod photo;
pub mod raster;
pub mod raw;
