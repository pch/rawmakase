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
//! ([`channel`]), reads the manifest's `api` the app passed
//! ([`protocol::ENV_API`]), answers the handshake, and calls the handlers one request at
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
mod shm;

pub use host::Host;
pub use rawmakase_extension_protocol as protocol;
pub use rawmakase_extension_protocol::{ErrorCode, RemoteError};
pub use run::{Context, Extension, ServeOptions};
pub use shm::mapped_buffers;

use rawmakase_extension_protocol::controls::Control;
use rawmakase_extension_protocol::method::{
    CommandRun, ExportBegin, ExportEnd, ExportEnded, ExportFile, MaskGenerate, MaskGenerated,
    RetouchGenerate, RetouchGenerated, SourceStarted,
};
use serde_json::Value;

/// A `[[command]]`'s implementation.
///
/// Serving API 2 ([`ServeOptions::api`]), the SDK checks a request's viewer
/// input ([`CommandRun::viewer_input`]) before calling [`Command::run`], and
/// refuses input that doesn't read or validate with
/// [`ErrorCode::InvalidRequest`]. Serving API 1 it hands the input over as it
/// came, as it always has. Either way the handler reads it with the same
/// call, which checks it.
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

/// What a mask generator is asked: the request, with its buffers mapped for
/// the call. The SDK unmaps them, and closes their handles, once
/// [`MaskGenerator::generate`] has returned, and only then replies.
pub struct MaskAsk<'a> {
    /// The request as the app sent it: the generator, the viewer input, the
    /// buffers' descriptors and the image's key.
    pub request: MaskGenerate,
    /// The image's pixels, laid out as [`MaskGenerate::image`] describes:
    /// 8-bit sRGB, three channels, rows `stride` bytes apart.
    pub image: &'a [u8],
    /// Where to write the coverage, laid out as [`MaskGenerate::coverage`]
    /// describes: 8-bit, one channel, rows `stride` bytes apart.
    pub coverage: &'a mut [u8],
}

/// A `[[mask-generator]]`'s implementation.
///
/// The SDK checks each request ([`MaskGenerate::validate`]) and maps its
/// buffers before calling [`MaskGenerator::generate`], and refuses one that
/// doesn't validate or map with [`ErrorCode::InvalidRequest`]. Mask requests
/// carry handles, so a generator's manifest declares `api = 2`.
pub trait MaskGenerator: Send {
    /// Writes the coverage of the mask the request asks for and names the
    /// model that made it. A generator whose model isn't installed answers
    /// [`ErrorCode::NeedsSetup`] with a
    /// [`NeedsSetup`](protocol::message::NeedsSetup) message; one whose
    /// runtime or worker is missing answers [`ErrorCode::Unavailable`]. One
    /// that sees [`Context::is_cancelled`] returns [`ErrorCode::Cancelled`].
    fn generate(&mut self, cx: &Context, ask: MaskAsk<'_>) -> Result<MaskGenerated, RemoteError>;
    /// Installs what the generator needs, reporting through
    /// [`Context::progress`], after it answered `needs_setup` and the user
    /// agreed. Generators that need nothing keep the default.
    fn setup(&mut self, cx: &Context, generator: &str) -> Result<(), RemoteError> {
        let _ = (cx, generator);
        Ok(())
    }
}

/// What a retouch generator is asked: the request, with its buffers mapped
/// for the call. The SDK unmaps them, and closes their handles, once
/// [`RetouchGenerator::generate`] has returned, and only then replies.
pub struct RetouchAsk<'a> {
    /// The request as the app sent it: the generator, the buffers'
    /// descriptors and the image's key.
    pub request: RetouchGenerate,
    /// The region's pixels, laid out as [`RetouchGenerate::image`]
    /// describes: 8-bit sRGB, three channels, rows `stride` bytes apart.
    pub image: &'a [u8],
    /// How much of each pixel the user painted, 0 to 255, laid out as
    /// [`RetouchGenerate::coverage`] describes: one channel, rows `stride`
    /// bytes apart.
    pub coverage: &'a [u8],
    /// Where to write the region with the painted area regenerated, laid
    /// out as [`RetouchGenerate::result`] describes (the image's layout).
    pub result: &'a mut [u8],
}

/// A `[[retouch-generator]]`'s implementation.
///
/// The SDK checks each request ([`RetouchGenerate::validate`]) and maps its
/// buffers before calling [`RetouchGenerator::generate`], and refuses one
/// that doesn't validate or map with [`ErrorCode::InvalidRequest`]. Serving
/// an `api` below 3 it refuses the request whole, as the app never sends it
/// one.
pub trait RetouchGenerator: Send {
    /// Writes the result and names the model that made it. One that needs
    /// the user to set something up first (an API key) answers
    /// [`ErrorCode::NeedsSetup`], one whose service can't be reached or
    /// declined to work now (offline, quota, rate limit)
    /// [`ErrorCode::Unavailable`], and one whose service declined the
    /// request's content [`ErrorCode::Refused`]. One that sees
    /// [`Context::is_cancelled`] returns [`ErrorCode::Cancelled`].
    fn generate(
        &mut self,
        cx: &Context,
        ask: RetouchAsk<'_>,
    ) -> Result<RetouchGenerated, RemoteError>;
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

/// A `[[source]]`'s implementation: while a scope of it runs, it watches for
/// files (a folder, a tethered camera) on threads of its own and reports each
/// complete one with [`Host::report`]. Sources need `api = 3`.
///
/// The app starts a scope for each catalog the user turned the source on
/// for, while that catalog is open, and stops it when the catalog closes or
/// the source is turned off; a program start gets its scopes again after
/// `hello`. The SDK stops every scope still running when the session ends.
pub trait Source: Send {
    /// A scope started: what the source reports in it, naming
    /// [`SourceStarted::scope`], goes to the catalog the scope is for. Starts
    /// watching on a thread that keeps `host`.
    fn start(&mut self, host: Host, started: SourceStarted) -> Result<(), RemoteError>;
    /// Scope `scope` ended: reports in it are refused. Stops watching,
    /// without waiting on a device.
    fn stop(&mut self, scope: u64);
}

/// An `[[export-destination]]`'s implementation: it sends an export's files
/// on (an upload, a web gallery, a printer) as the export writes them.
/// Export destinations need `api = 4`.
///
/// A delivery is `begin`, then one `file` for each file written, then
/// `end`, all naming the same delivery id; requests come one at a time, in
/// that order. The SDK checks each request (`validate`), and that it follows
/// its delivery (a file within `begin`'s count and only once, an `end` that
/// counts the files received), before calling the destination, and refuses
/// one that doesn't with
/// [`ErrorCode::InvalidRequest`], as it does a request when serving an `api`
/// below 4. The answer to `end` is checked too ([`ExportEnded::validate`]):
/// one that doesn't validate goes to the app as
/// [`ErrorCode::InvalidRequest`]. A destination that needs the whole set
/// first, as a web gallery does, can use [`collected`].
pub trait ExportDestination: Send {
    /// A delivery starts. One that needs the user to set something up first
    /// (an account) answers [`ErrorCode::NeedsSetup`], one whose service
    /// can't be reached [`ErrorCode::Unavailable`], one that can't take this
    /// export (its files beside originals in several folders, say)
    /// [`ErrorCode::InvalidRequest`]: then no files come.
    fn begin(&mut self, cx: &Context, begin: ExportBegin) -> Result<(), RemoteError>;
    /// A file of a delivery, in place: answer as soon as it is taken in (an
    /// upload can start on another thread), since how it went is said in
    /// [`ExportDestination::end`].
    fn file(&mut self, cx: &Context, file: ExportFile) -> Result<(), RemoteError>;
    /// The delivery's export is over: finish sending and say how each file
    /// went. A long one reports how far it is through [`Context::progress`],
    /// which keeps it within its deadline. One that sees
    /// [`Context::is_cancelled`] returns [`ErrorCode::Cancelled`].
    fn end(&mut self, cx: &Context, end: ExportEnd) -> Result<ExportEnded, RemoteError>;
    /// Delivery `delivery` stopped before its end (Stop, or a request of it
    /// that failed): no more of it comes. Stop what it still sends in the
    /// background and forget it. Nothing by default.
    fn stopped(&mut self, delivery: &str) {
        let _ = delivery;
    }
}

/// A whole delivery, as [`collected`] hands it over.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    /// Its `export.begin`.
    pub begin: ExportBegin,
    /// Its files, in the order they came.
    pub files: Vec<ExportFile>,
    /// Its `export.end`.
    pub end: ExportEnd,
}

/// An export destination that keeps each delivery's files until its `end`,
/// then hands `send` the whole delivery.
pub fn collected<F>(send: F) -> impl ExportDestination
where
    F: FnMut(&Context, Delivery) -> Result<ExportEnded, RemoteError> + Send,
{
    Collected {
        send,
        open: std::collections::BTreeMap::new(),
    }
}

struct Collected<F> {
    send: F,
    open: std::collections::BTreeMap<String, (ExportBegin, Vec<ExportFile>)>,
}
impl<F> ExportDestination for Collected<F>
where
    F: FnMut(&Context, Delivery) -> Result<ExportEnded, RemoteError> + Send,
{
    fn begin(&mut self, _: &Context, begin: ExportBegin) -> Result<(), RemoteError> {
        self.open
            .insert(begin.delivery.clone(), (begin, Vec::new()));
        Ok(())
    }
    fn file(&mut self, _: &Context, file: ExportFile) -> Result<(), RemoteError> {
        match self.open.get_mut(&file.delivery) {
            Some((_, files)) => {
                files.push(file);
                Ok(())
            }
            None => Err(unknown_delivery(&file.delivery)),
        }
    }
    fn end(&mut self, cx: &Context, end: ExportEnd) -> Result<ExportEnded, RemoteError> {
        let (begin, files) = self
            .open
            .remove(&end.delivery)
            .ok_or_else(|| unknown_delivery(&end.delivery))?;
        (self.send)(cx, Delivery { begin, files, end })
    }
    fn stopped(&mut self, delivery: &str) {
        self.open.remove(delivery);
    }
}

fn unknown_delivery(id: &str) -> RemoteError {
    RemoteError::new(
        ErrorCode::InvalidRequest,
        format!("no delivery {id} has begun"),
    )
}
