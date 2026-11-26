//! The Rust side of the Electron example: a single-view photo editor on
//! RAWmakase's library, `rawmakase-core`, which is the only RAWmakase crate it
//! depends on.
//!
//! - [`editor`] is the editor itself: one photo, its recipe and the render
//!   thread, with no Node in it.
//! - [`worker`] is the render thread's scheduling.
//! - `binding` (with the `addon` feature) is the Node addon over the editor.
// No `unsafe` is written here. The editor and the worker forbid it; the root can
// only deny it, because what `#[napi]` expands to in `binding` allows it for
// itself, and `forbid` rejects an `allow` under it.
#![deny(unsafe_code)]

#[cfg(feature = "addon")]
mod binding;
pub mod editor;
pub mod worker;
