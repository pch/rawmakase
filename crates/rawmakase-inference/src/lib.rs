//! Local ONNX inference for Subject, Background and Sky selection, and for AI Denoise.
//!
//! This crate owns the pinned model contracts ([`manifest`]), the pure image
//! to tensor and matte to coverage mappings ([`process`]), AI Denoise's tiler and
//! gain fit on packed mosaic planes ([`denoise`]) and the lazily loaded
//! ONNX Runtime sessions ([`runtime`]). The only other workspace crate it
//! depends on is the cancellation token (`rawmakase-cancel`):
//! callers pass an 8-bit sRGB [`RgbImage`] and get 8-bit [`Coverage`] back in
//! the same frame. Where the model file lives, how it is downloaded and
//! verified, job scheduling and cache identity belong to the callers.
//!
//! A missing or unusable ONNX Runtime library is reported as
//! [`InferenceError::RuntimeUnavailable`]; the library is never linked.

pub mod auto;
pub mod denoise;
mod error;
pub mod manifest;
pub mod panoptic;
pub mod process;
pub mod refine;
pub mod runtime;

pub use error::InferenceError;
pub use manifest::{
    DenoiseSpec, ModelFile, ModelSpec, PANOPTIC, PanopticSpec, RAWNIND, SKY, SUBJECT, SkySpec,
};
pub use process::{Coverage, Point, Prompt, RgbImage};
pub use runtime::{
    Analysis, Denoiser, Embedding, LoadOptions, Provider, RUNTIME_ENV, Session, Sky, SkySession,
    Subject,
};
