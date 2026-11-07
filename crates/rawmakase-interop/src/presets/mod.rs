//! Native recipe presets, built-in presets and installed XMP preset collections.
pub mod amount;
pub mod builtin;
pub mod curves;
pub mod inferred;
mod library;
mod native;
mod photo;
pub mod user;
pub use library::{
    Library, display_name, find_preset, library_dirs, load_favorites, load_library, save_favorites,
};
pub use native::{LoadedPreset, applied_to, load_preset, save_preset};
pub use photo::{Applied, apply_to_photo};

#[cfg(test)]
mod tests;
