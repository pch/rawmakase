//! Native recipe presets, built-in presets and installed XMP preset collections.
pub mod amount;
pub mod builtin;
pub mod curves;
mod library;
mod native;
mod photo;
pub mod user;
pub use library::{
    Library, PresetLayout, PresetLibrary, display_name, find_preset, load_favorites, save_favorites,
};
pub use native::{LoadedPreset, applied_to, load_preset, save_preset};
pub use photo::{Applied, apply_to_photo};

#[cfg(test)]
mod tests;
