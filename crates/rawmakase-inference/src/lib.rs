//! Local ONNX inference shared by every model user, and AI Denoise.
//!
//! This crate owns the lazily loaded ONNX Runtime and the helpers any model's
//! sessions are built and run with ([`runtime`]: the library search, the
//! [`Provider`] choice, [`LoadOptions`], cancellation and panic guards), the error
//! every model reports ([`InferenceError`]), the file record a model pack is made
//! of ([`ModelFile`]), and AI Denoise: its pinned contract ([`manifest`]), its tiler
//! and gain fit on packed mosaic planes ([`denoise`]) and its loaded model
//! ([`Denoiser`]), and on Apple's Neural Engine its converted model run by Core ML
//! (`neural_engine`, on Apple platforms only). The selection models (Subject, Background and Sky) are
//! `rawmakase-selection`'s, which builds on this crate. The only other workspace
//! crate it depends on is the cancellation token (`rawmakase-cancel`). Where a
//! model file lives, how it is downloaded and verified, job scheduling and cache
//! identity belong to the callers.
//!
//! A missing or unusable ONNX Runtime library is reported as
//! [`InferenceError::RuntimeUnavailable`]; the library is never linked.

pub mod denoise;
mod error;
pub mod manifest;
#[cfg(target_vendor = "apple")]
pub mod neural_engine;
pub mod runtime;

pub use error::InferenceError;
pub use manifest::{DenoiseSpec, ModelFile, RAWNIND};
pub use runtime::{Denoiser, LoadOptions, Provider, RUNTIME_ENV};
