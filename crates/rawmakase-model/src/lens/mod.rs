//! Lens corrections. Camera makers embed per-shot vignetting, distortion and lateral
//! chromatic aberration data in their RAW files; Lightroom applies it automatically as
//! the "built-in" lens profile. The decode crate's `raw::lens` reads those tables, next
//! to the RAW containers, into a [`LensCorrection`](crate::optics::LensCorrection), which the renderer evaluates at
//! normalized image radius; [`lcp`] reads Adobe lens profiles into the same model.
pub mod auto_ca;
pub mod choice;
pub mod lcp;
pub mod lensfun;
