//! What RAWmakase and its extensions agree on.
//!
//! - [`manifest`]: `rawmakase-extension.toml`, the one file the app reads to know
//!   what an extension contributes, and its JSON Schema.
//! - [`message`]: the envelope every line of the extension protocol carries, and
//!   the error codes.
//! - [`method`]: the requests and events each side sends, with their parameters.
//! - [`framing`]: one JSON object per line, at most [`MAX_LINE`] bytes.
//! - [`controls`]: the host-drawn controls a panel or a setting is made of.
//! - [`viewer`]: viewer input, the clicks and boxes a user makes on the photo.
//! - [`buffer`]: the descriptors of pixel buffers in shared memory.
//!
//! This crate depends on no other crate of RAWmakase's, so an extension's SDK
//! builds without the app, as `rawmakase-protocol` does for control clients.
//! The reference is in `docs/extensions/` (manifest.md and protocol.md).
// Every public item says what it is for: this crate is a contract third-party
// extensions build on.
#![warn(missing_docs)]
pub mod buffer;
pub mod controls;
pub mod framing;
pub mod manifest;
pub mod message;
pub mod method;
pub mod viewer;

pub use message::{ErrorCode, Message, RemoteError};

/// The version of the extension protocol. The app sends it in `hello` and the
/// program answers with its own; either side ends the session on another.
pub const PROTOCOL: u32 = 1;

/// The longest line either side sends or accepts, in bytes, without its
/// newline. Bulk data never travels in JSON.
pub const MAX_LINE: usize = 1 << 20;

/// The environment variable naming the channel's file descriptor in the program
/// on Linux and macOS: the host passes its end of a socket pair as this
/// descriptor (always 3).
pub const ENV_FD: &str = "RAWMAKASE_EXTENSION_FD";

/// The environment variable naming the channel's named pipe in the program on
/// Windows (`\\.\pipe\rawmakase-ext-<pid>-<random>`), which the program opens
/// once.
pub const ENV_PIPE: &str = "RAWMAKASE_EXTENSION_PIPE";

/// The environment variable telling the program its manifest's `api`, so the
/// SDK knows which framing rules apply before it reads the first line. Absent
/// means 1.
pub const ENV_API: &str = "RAWMAKASE_EXTENSION_API";
