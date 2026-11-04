//! What AI Denoise needs from decoding, with no model knowledge: the visible mosaic
//! packed as the model reads it and the result assembled back ([`denoise`]), and the
//! results' disk cache ([`cache`]).
pub mod cache;
pub mod denoise;
