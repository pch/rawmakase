//! Optional remote inference for RAWmakase's selection models.
//!
//! The app normally runs the models itself (see `rawmakase-inference`, which is
//! still the default and still owns the model contract). This crate is the
//! small, explicit alternative: a worker binary,
//! `rawmakase-remote-worker`, that loads the same models where a capable GPU
//! is, and a client the app uses instead of its local session when configured
//! to. Nothing here downloads models, touches a catalog or knows about the
//! app's editing: pixels go in, coverage comes out, over bounded length
//! prefixed JSON frames on a TCP connection ([`protocol`]).
//!
//! The worker is meant to run beside the models it loads, reached through an
//! SSH tunnel from another computer; it binds the loopback by default, speaks
//! no credentials of its own, and answers `ping` with what it actually loaded.
//! See `docs/remote-inference.md` for setup.
// Every public item says what it is for: the protocol and the client are a
// contract between processes (#808 step 4.13).
#![warn(missing_docs)]

pub mod client;
pub mod protocol;
pub mod server;

pub use client::{ClientError, RemoteAnalysis, RemoteOptions, RemoteSession, ServerInfo};
pub use protocol::{Code, Op, Reply, Request, Response, WireCoverage, WireError, WirePrompt};
pub use server::{
    ENTRIES, Engine, EngineError, IDLE_TIMEOUT, Info, MAX_CACHED_BYTES, Model, Outcome, Server,
};
