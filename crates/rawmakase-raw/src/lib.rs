//! RAWmakase's own RAW decoders, in pure Rust: containers, maker notes,
//! sensor codecs, camera facts and embedded previews.
//!
//! Three operations, cheapest first, each on the file's bytes in memory:
//! [`open`] reads the header facts ([`RawInfo`]) without decoding the
//! sensor data, [`preview`] finds the embedded JPEG, and [`decode`] unpacks
//! the sensor data ([`Sensor`]), checking `cancel` as it goes. One selector
//! ([`Variant`]) decides which reader and codec handle a file; a file of a
//! known container in a variant not decoded here is [`Error::Unsupported`].
#![forbid(unsafe_code)]
mod codec;
mod color;
mod container;
mod error;
mod facts;
mod geometry;
mod info;
mod io;
mod levels;
mod names;
mod preview;
mod sensor;
mod variant;

pub use error::{Error, Result};
pub use info::{Cfa, Presets, RawInfo, UNSET};
pub use preview::NoPreview;
pub use sensor::Sensor;
use std::sync::atomic::AtomicBool;
pub use variant::Variant;

/// The decoders' version, for the About window and fingerprints.
pub const VERSION: &str = concat!("RAWmakase ", env!("CARGO_PKG_VERSION"));

/// The header facts of the RAW file in `bytes`.
pub fn open(bytes: &[u8]) -> Result<RawInfo> {
    variant::open(bytes)
}

/// The embedded JPEG preview of the file `info` was read from.
pub fn preview<'a>(info: &RawInfo, bytes: &'a [u8]) -> std::result::Result<&'a [u8], NoPreview> {
    let (at, len) = preview::find(info, bytes)?;
    let range = usize::try_from(at)
        .ok()
        .zip(usize::try_from(at.saturating_add(len)).ok());
    range
        .and_then(|(start, end)| bytes.get(start..end))
        .ok_or(NoPreview::Missing)
}

/// The sensor data of the file `info` was read from; stops with
/// [`Error::Cancelled`] soon after `cancel` is set.
pub fn decode(info: &RawInfo, bytes: &[u8], cancel: &AtomicBool) -> Result<Sensor> {
    variant::decode(info, bytes, cancel)
}
