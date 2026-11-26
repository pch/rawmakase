//! RAWmakase as a library: open, decode, edit, render and export photos as
//! RAWmakase does, without the desktop app. The guide is `docs/embedding.md`;
//! the plan it is built by is `docs/embedding-plan.md`.
//!
//! **Being built.** One photo, start to finish, is here; the rest joins step
//! by step, each once the crates under it take what it needs as a value, so
//! the context never offers a setting that something underneath still looks
//! up for itself. Nothing here is stable.
//!
//! ```no_run
//! # #[cfg(feature = "export")] {
//! use rawmakase_core::{CacheLimits, Cancel, Context, Decode, ExportSettings, Parameter, View};
//!
//! let ctx = Context::builder()
//!     .data_dirs(["/app/data"])
//!     .cache("/app/cache", CacheLimits::default(), "my-app 1.0 (build 42)")
//!     .build()?;
//!
//! let photo = ctx.open("photo.dng", Cancel::NEVER)?;
//! let mut recipe = photo.starting_recipe()?;
//! photo.set(&mut recipe, Parameter::Temperature, 5200.)?; // white balance follows
//! recipe.curve.insert([0.25, 0.2]); // the point curve, by its own methods
//!
//! let image = photo.decode(Decode::Full, Cancel::NEVER)?; // reads the file again
//! let frame = ctx.render(&image, &recipe, View::Fit(2048), Cancel::NEVER)?;
//! let rgba = frame.rgba8();
//!
//! let settings = ExportSettings::default();
//! ctx.export(&photo, &recipe, &settings, "photo.jpg", false, Cancel::NEVER, |_| {})?;
//! # let _ = rgba;
//! # }
//! # Ok::<(), rawmakase_core::Error>(())
//! ```
//!
//! # The parts
//!
//! - **Handles of the library's own** for what an app does things with: a
//!   [`Photo`], a decoded [`Image`], a [`Renderer`] and its [`Frame`]s. The
//!   crates' own types are not shown through them.
//! - **The edit's values, as they are.** A [`Recipe`] and a photo's
//!   [`Metadata`] are the model's types, and an app reads them freely. It
//!   changes a recipe through the model's rules where a setting has them: a
//!   slider with [`Photo::set`], the point curve with [`ToneCurve`]'s
//!   `insert`, `move_point` and `remove`.
//! - **A [`Context`]**, which holds the configuration and is where
//!   operations start.
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
//! Everything is synchronous. [`Context`], [`Photo`], [`Image`] and
//! [`Error`] are `Send` and `Sync`; a [`Renderer`] is `Send` and belongs to
//! the one thread that renders. Long operations take a [`Cancel`], and one
//! that was cancelled returns [`ErrorKind::Cancelled`]. The library starts no
//! thread of its own; pixel work runs on rayon's pool.
//!
//! An editor that renders while its controls move keeps one render thread
//! and gives it only the newest edit through a [`Mailbox`]: the one waiting
//! is replaced, the running one finishes and shows, and [`Shown`] drops a
//! frame older than the one on screen. Before an edit's frame it renders the
//! draft [`Renderer::draft_due`] asks for.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod context;
mod edit;
mod error;
#[cfg(feature = "export")]
mod export;
mod image;
mod mailbox;
mod photo;
mod render;

#[cfg(feature = "workspace")]
pub use context::workspace;
pub use context::{CacheLimits, Context, ContextBuilder};
pub use edit::{Applied, Control, Parameter, Preset, Scale};
pub use error::{Error, ErrorKind, Result};
#[cfg(feature = "export")]
pub use export::ExportSettings;
pub use image::{DecodeStats, Image};
pub use mailbox::{Mailbox, Numbered, Shown, Taken};
pub use photo::{Decode, Photo};
/// The values an [`ExportSettings`]'s fields take.
#[cfg(feature = "export")]
pub use rawmakase_interop::export_settings::Include;
#[cfg(feature = "export")]
pub use rawmakase_model::color::OutputSpace;
#[cfg(feature = "export")]
pub use rawmakase_model::color::hdr_output::{HdrGamut, HdrOutput, HdrTransfer};
pub use render::{Frame, Renderer, View};

pub use rawmakase_cancel::Cancel;
pub use rawmakase_interop::raw_defaults::{CameraDefault, DefaultChoice, RawDefaults};
pub use rawmakase_model::camera_data::Metadata;
pub use rawmakase_model::camera_profiles::CameraProfile;
pub use rawmakase_model::color::curve::{Inserted, ToneCurve};
pub use rawmakase_model::model::recipe::Recipe;

#[cfg(test)]
mod tests;
