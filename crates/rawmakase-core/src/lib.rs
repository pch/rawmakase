//! RAWmakase as a library: open, decode, edit, render and export photos as
//! RAWmakase does, without the desktop app. The guide is `docs/embedding.md`;
//! the plan it is built by is `docs/embedding-plan.md`.
//!
//! **Being built.** This is the library's start: a [`Context`] made from values
//! and the one [`Error`] type. The operations on a photo join it step by step,
//! each once the crates under it take what it needs as a value, so the context
//! never offers a setting that something underneath still looks up for
//! itself. Nothing here is stable.
//!
//! ```
//! use rawmakase_core::{CacheLimits, Context};
//!
//! # fn main() -> rawmakase_core::Result<()> {
//! # let cache = std::env::temp_dir().join("my-app-cache");
//! let ctx = Context::builder()
//!     .cache(&cache, CacheLimits::default(), "my-app 1.0 (build 42)")
//!     .build()?;
//! assert_eq!(ctx.build_id(), Some("my-app 1.0 (build 42)"));
//! # Ok(())
//! # }
//! ```
//!
//! # Configuration
//!
//! RAWmakase's code in the library reads no environment variable and starts
//! no program to configure itself: what it needs is given to the
//! [`Context`]. (`scripts/ambient.py` lists what the crates under it still
//! look up, and the list only shrinks.)
//!
//! # Errors
//!
//! Every operation returns [`Error`], whose [`ErrorKind`] says what happened
//! for the kinds a caller treats differently: cancelled, unsupported,
//! missing, changed, exists, invalid, or anything else. Its message is the
//! chain of causes, printed with `{:#}`.
//!
//! # Threads
//!
//! The API is synchronous. [`Context`] and [`Error`] are `Send` and `Sync`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod context;
mod error;

#[cfg(feature = "workspace")]
pub use context::workspace;
pub use context::{CacheLimits, Context, ContextBuilder};
pub use error::{Error, ErrorKind, Result};
