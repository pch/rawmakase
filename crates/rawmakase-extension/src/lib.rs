//! Write a RAWmakase extension's program.
//!
//! Register a handler for each contribution the manifest declares, then run:
//!
//! ```no_run
//! use rawmakase_extension::{Extension, RemoteError, Context};
//! use rawmakase_extension::protocol::method::CommandRun;
//!
//! fn main() -> std::io::Result<()> {
//!     Extension::new()
//!         .command("hello", |cx: &Context, _run: CommandRun| {
//!             cx.host().notify(rawmakase_extension::protocol::method::Level::Info, "Hello")?;
//!             Ok(serde_json::Value::Null)
//!         })
//!         .run()
//! }
//! ```
//!
//! [`Extension::run`] attaches the channel the app started the program with
//! ([`channel`]), answers the handshake, and calls the handlers one request at
//! a time on the calling thread while another thread keeps reading, so
//! cancellation and the app's replies arrive while a handler runs. It returns
//! when the app sends `quitting` or closes the channel. [`mock::MockHost`]
//! runs an extension in a test without the app.
//!
//! This crate depends on the protocol crate alone, so the bundled extensions
//! use nothing a third party can't.
#![warn(missing_docs)]
pub mod channel;
mod host;
pub mod mock;
mod run;

pub use host::Host;
pub use rawmakase_extension_protocol as protocol;
pub use rawmakase_extension_protocol::{ErrorCode, RemoteError};
pub use run::{Context, Extension};

use rawmakase_extension_protocol::controls::Control;
use rawmakase_extension_protocol::method::CommandRun;
use serde_json::Value;

/// A `[[command]]`'s implementation.
pub trait Command: Send {
    /// Runs the command. Its edits go to the app as proposals
    /// ([`Host::propose_settings`] with [`Context::op`]) while it runs; what it
    /// returns is the reply to `command.run`. A command that sees
    /// [`Context::is_cancelled`] returns [`ErrorCode::Cancelled`].
    fn run(&mut self, cx: &Context, run: CommandRun) -> Result<Value, RemoteError>;
}
impl<F> Command for F
where
    F: FnMut(&Context, CommandRun) -> Result<Value, RemoteError> + Send,
{
    fn run(&mut self, cx: &Context, run: CommandRun) -> Result<Value, RemoteError> {
        self(cx, run)
    }
}

/// A `[[panel]]`'s implementation: it answers each request with the panel's
/// controls, whole.
pub trait Panel: Send {
    /// The panel is shown.
    fn open(&mut self, cx: &Context) -> Result<Vec<Control>, RemoteError>;
    /// The user committed a new value. A value that doesn't validate is
    /// answered with the controls, the offending one carrying the error as its
    /// `help`.
    fn changed(
        &mut self,
        cx: &Context,
        control: &str,
        value: Value,
    ) -> Result<Vec<Control>, RemoteError>;
    /// The user clicked a button. Panels without buttons keep the default.
    fn clicked(&mut self, cx: &Context, control: &str) -> Result<Vec<Control>, RemoteError> {
        let _ = cx;
        Err(RemoteError::new(
            ErrorCode::InvalidRequest,
            format!("{control} is not a button"),
        ))
    }
}

/// A `[[control-adapter]]` the app launches: it watches its devices on threads
/// of its own and reports them with [`Host::device`] and [`Host::operate`].
pub trait ControlAdapter: Send {
    /// Called once the handshake is done. Starts the adapter's threads, which
    /// keep `host` to report through.
    fn start(&mut self, host: Host) -> Result<(), RemoteError>;
    /// Called when the session ends: stop the threads, without waiting for a
    /// device.
    fn stop(&mut self) {}
}
