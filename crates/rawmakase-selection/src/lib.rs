//! Subject, Background and Sky selection with local ONNX models.
//!
//! This crate owns the selection models' pinned contracts ([`manifest`]), the pure
//! image to tensor and matte to coverage mappings ([`process`]), reading the
//! panoptic model's answer ([`panoptic`]), automatic Subject and Sky from what the
//! models said ([`auto`]), edge refinement ([`refine`]) and the models' sessions
//! ([`runtime`]). Callers pass an 8-bit sRGB [`RgbImage`] and get 8-bit
//! [`Coverage`] back in the same frame. It knows no edit, catalog, renderer,
//! decoder or extension host.
//!
//! The ONNX Runtime loader, the provider choice, the session helpers,
//! [`ModelFile`](rawmakase_inference::ModelFile) and
//! [`InferenceError`](rawmakase_inference::InferenceError) are
//! `rawmakase-inference`'s, shared with AI Denoise; the only other workspace crate
//! this one depends on is the cancellation token (`rawmakase-cancel`). Where the
//! model files live, how they are downloaded and verified, job scheduling and cache
//! identity belong to the callers.

pub mod auto;
mod kinds;
pub mod manifest;
mod onnx_patch;
pub mod panoptic;
pub mod process;
pub mod refine;
pub mod runtime;
mod sky_edge;

pub use manifest::{ModelSpec, PANOPTIC, PanopticSpec, SKY, SUBJECT, SkySpec};
pub use process::{Coverage, Point, Prompt, RgbImage};
pub use runtime::{Analysis, Embedding, Session, Sky, SkySession, SkyTimings, Subject};
