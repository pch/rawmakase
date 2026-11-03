//! The renderer: developing a decoded camera image with a recipe, on the CPU
//! (the reference) and through the GPU preview port. It builds over the model alone,
//! without the file formats, the RAW decoders, the catalog or the GUI, and its tests need no GPU
//! (those that do are ignored).
use rawmakase_model::{camera_data, camera_profiles, color, lens, model, optics, rendered};
#[cfg(test)]
use {rawmakase_interop::xmp, rawmakase_model::xml};

pub mod develop;
