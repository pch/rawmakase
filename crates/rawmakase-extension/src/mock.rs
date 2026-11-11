//! A stand-in for the app, to test an extension without it: send it requests
//! and events, answer its requests with a function of your own, and look at
//! what it sent.
//!
//! ```
//! use rawmakase_extension::{Context, Extension, mock::MockHost};
//! use rawmakase_extension::protocol::method::CommandRun;
//! use serde_json::json;
//!
//! let extension = Extension::new().command("echo", |_cx: &Context, run: CommandRun| Ok(run.input));
//! let host = MockHost::start(extension);
//! host.hello().unwrap();
//! assert_eq!(host.run_command("echo", None, json!(5)).unwrap(), json!(5));
//! host.finish().unwrap();
//! ```
use crate::{Extension, ServeOptions};
use rawmakase_extension_protocol::controls::Control;
use rawmakase_extension_protocol::framing::{LineReader, write_line};
use rawmakase_extension_protocol::method::{
    AppEvent, AppRequest, CommandRun, Controls, Empty, Hello, HelloReply, MaskGenerate,
    MaskGenerated, MaskSetup, PanelChanged, PanelClicked, PanelOpen, Snapshot,
};
use rawmakase_extension_protocol::{ErrorCode, Message, PROTOCOL, RemoteError};
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What the extension sent, other than replies.
#[derive(Clone, Debug, PartialEq)]
pub enum Recorded {
    /// A request, with the reply the responder gave it.
    Request {
        /// Its method.
        method: String,
        /// Its parameters.
        params: Value,
        /// What the responder answered.
        reply: Result<Value, RemoteError>,
    },
    /// An event.
    Event {
        /// Its name.
        name: String,
        /// Its parameters.
        params: Value,
    },
    /// Progress of one of the mock's requests.
    Progress {
        /// The request.
        id: u64,
        /// The fraction done.
        done: Option<f64>,
        /// The text.
        text: Option<String>,
    },
}

type Responder = Box<dyn FnMut(&str, &Value) -> Result<Value, RemoteError> + Send>;

/// How long the mock waits for a reply before failing a test, rather than
/// hanging it.
const WAIT: Duration = Duration::from_secs(10);

/// An extension served on a thread, with the app's side in the test's hands.
pub struct MockHost {
    /// Shared with the listener, which answers the extension's requests.
    to_program: Arc<Mutex<PipeWriter>>,
    state: Arc<(Mutex<State>, Condvar)>,
    next_id: Mutex<u64>,
    program: JoinHandle<io::Result<()>>,
}

#[derive(Default)]
struct State {
    replies: HashMap<u64, Result<Value, RemoteError>>,
    recorded: Vec<Recorded>,
    closed: bool,
}

impl MockHost {
    /// Serves `extension`, answering each of its requests with `null`.
    pub fn start(extension: Extension) -> Self {
        Self::start_with(extension, |_, _| Ok(Value::Null))
    }

    /// Serves `extension`, answering each of its requests with `responder`,
    /// called with the method and parameters.
    pub fn start_with(
        extension: Extension,
        responder: impl FnMut(&str, &Value) -> Result<Value, RemoteError> + Send + 'static,
    ) -> Self {
        Self::start_with_options(extension, ServeOptions::default(), responder)
    }

    /// [`MockHost::start_with`], serving in the mode `options` sets (an
    /// `api` of 2 for a manifest written for it). Each mock has its own mode,
    /// so mocks of both run side by side in one test.
    pub fn start_with_options(
        extension: Extension,
        options: ServeOptions,
        responder: impl FnMut(&str, &Value) -> Result<Value, RemoteError> + Send + 'static,
    ) -> Self {
        let (app_reader, program_writer) = pipe();
        let (program_reader, app_writer) = pipe();
        let program = std::thread::spawn(move || {
            extension.serve_with_options(program_reader, program_writer, options)
        });
        let state: Arc<(Mutex<State>, Condvar)> = Arc::default();
        let to_program = Arc::new(Mutex::new(app_writer));
        {
            let state = state.clone();
            let to_program = to_program.clone();
            let responder: Responder = Box::new(responder);
            std::thread::spawn(move || listen(app_reader, &state, &to_program, responder));
        }
        Self {
            to_program,
            state,
            next_id: Mutex::new(1),
            program,
        }
    }

    fn send(&self, message: &Message) {
        let mut writer = self.to_program.lock().unwrap_or_else(|e| e.into_inner());
        let _ = write_line(&mut *writer, message);
    }

    /// Sends a request without waiting; [`MockHost::wait`] reads its reply.
    pub fn send_request(&self, request: AppRequest) -> u64 {
        let id = {
            let mut next = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
            *next += 1;
            *next - 1
        };
        let (method, params) = request.into_parts();
        self.send(&Message::request(id, &method, params));
        id
    }

    /// The reply to request `id`, waiting at most ten seconds.
    pub fn wait(&self, id: u64) -> Result<Value, RemoteError> {
        let (lock, changed) = &*self.state;
        let deadline = Instant::now() + WAIT;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(reply) = state.replies.remove(&id) {
                return reply;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if state.closed || left.is_zero() {
                return Err(RemoteError::new(
                    if state.closed {
                        ErrorCode::Exited
                    } else {
                        ErrorCode::Deadline
                    },
                    format!("no reply to request {id}"),
                ));
            }
            state = changed
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Sends a request and waits for its reply.
    pub fn request(&self, request: AppRequest) -> Result<Value, RemoteError> {
        let id = self.send_request(request);
        self.wait(id)
    }

    /// Asks the extension to cancel request `id`.
    pub fn cancel(&self, id: u64) {
        self.send(&Message::Cancel { id });
    }

    /// Sends an event.
    pub fn event(&self, event: AppEvent) {
        let (name, params) = event.into_parts();
        self.send(&Message::event(&name, params));
    }

    /// The handshake, with no settings, session 1 and folders in the
    /// system's temporary folder, which it doesn't create.
    pub fn hello(&self) -> Result<HelloReply, RemoteError> {
        let temp = std::env::temp_dir().join("rawmakase-extension-mock");
        self.hello_with(Hello {
            protocol: PROTOCOL,
            app_version: "0.0.0".into(),
            extension: "test.extension".into(),
            settings: Map::new(),
            contributions: Vec::new(),
            data_dir: temp.join("data"),
            cache_dir: temp.join("cache"),
            session: 1,
        })
    }

    /// The handshake with these parameters.
    pub fn hello_with(&self, hello: Hello) -> Result<HelloReply, RemoteError> {
        let reply = self.request(AppRequest::Hello(hello))?;
        decode(reply)
    }

    /// Runs a command and waits for its reply.
    pub fn run_command(
        &self,
        command: &str,
        target: Option<Snapshot>,
        input: Value,
    ) -> Result<Value, RemoteError> {
        self.request(AppRequest::CommandRun(CommandRun {
            command: command.into(),
            target,
            input,
        }))
    }

    /// Opens a panel and returns its controls.
    pub fn open_panel(&self, panel: &str) -> Result<Vec<Control>, RemoteError> {
        let reply = self.request(AppRequest::PanelOpen(PanelOpen {
            panel: panel.into(),
        }))?;
        decode::<Controls>(reply).map(|c| c.controls)
    }

    /// Commits a control's new value and returns the panel's controls.
    pub fn change(
        &self,
        panel: &str,
        control: &str,
        value: Value,
    ) -> Result<Vec<Control>, RemoteError> {
        let reply = self.request(AppRequest::PanelChanged(PanelChanged {
            panel: panel.into(),
            control: control.into(),
            value,
        }))?;
        decode::<Controls>(reply).map(|c| c.controls)
    }

    /// Clicks a button and returns the panel's controls.
    pub fn click(&self, panel: &str, control: &str) -> Result<Vec<Control>, RemoteError> {
        let reply = self.request(AppRequest::PanelClicked(PanelClicked {
            panel: panel.into(),
            control: control.into(),
        }))?;
        decode::<Controls>(reply).map(|c| c.controls)
    }

    /// Asks a mask generator for a mask and returns its result, refusing one
    /// that doesn't validate ([`MaskGenerated::validate`]) as the app does.
    /// The request's buffers are descriptors only: nothing is mapped yet.
    pub fn generate_mask(&self, request: MaskGenerate) -> Result<MaskGenerated, RemoteError> {
        let reply = self.request(AppRequest::MaskGenerate(request))?;
        let result: MaskGenerated = decode(reply)?;
        result.validate()?;
        Ok(result)
    }

    /// Asks a mask generator to install what it needs.
    pub fn setup_mask(&self, generator: &str) -> Result<(), RemoteError> {
        self.request(AppRequest::MaskSetup(MaskSetup {
            generator: generator.into(),
        }))
        .map(drop)
    }

    /// Everything the extension sent other than replies, so far.
    pub fn recorded(&self) -> Vec<Recorded> {
        let (lock, _) = &*self.state;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .recorded
            .clone()
    }

    /// Waits until the extension has sent something `until` accepts, at most
    /// ten seconds, and returns everything it sent.
    pub fn wait_for(&self, until: impl Fn(&[Recorded]) -> bool) -> Vec<Recorded> {
        let (lock, changed) = &*self.state;
        let deadline = Instant::now() + WAIT;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        while !until(&state.recorded) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || state.closed {
                break;
            }
            state = changed
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        state.recorded.clone()
    }

    /// Sends `quitting`, closes the channel and returns what serving returned.
    pub fn finish(self) -> io::Result<()> {
        self.event(AppEvent::Quitting(Empty {}));
        self.to_program
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .close();
        self.program
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the extension panicked")))
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, RemoteError> {
    serde_json::from_value(value)
        .map_err(|e| RemoteError::new(ErrorCode::InvalidRequest, e.to_string()))
}

/// The mock's reader: records what the extension sends and answers its
/// requests.
fn listen(
    reader: PipeReader,
    state: &(Mutex<State>, Condvar),
    to_program: &Mutex<PipeWriter>,
    mut responder: Responder,
) {
    let (lock, changed) = state;
    let mut lines = LineReader::new(reader);
    while let Ok(Some(line)) = lines.next_message() {
        let Ok(message) = line else { continue };
        let record = |state: &mut State, message| match message {
            Message::Ok { id, v } => {
                state.replies.insert(id, Ok(v));
            }
            Message::Err { id, code, message } => {
                state.replies.insert(id, Err(RemoteError { code, message }));
            }
            Message::Progress { id, done, text } => {
                state.recorded.push(Recorded::Progress { id, done, text })
            }
            Message::Event { e, p } => state.recorded.push(Recorded::Event { name: e, params: p }),
            Message::Req { .. } | Message::Cancel { .. } => {}
        };
        if let Message::Req { id, m, p } = message {
            let reply = responder(&m, &p);
            let mut writer = to_program.lock().unwrap_or_else(|e| e.into_inner());
            let _ = write_line(&mut *writer, &Message::reply(id, reply.clone()));
            drop(writer);
            let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
            state.recorded.push(Recorded::Request {
                method: m,
                params: p,
                reply,
            });
        } else {
            record(&mut lock.lock().unwrap_or_else(|e| e.into_inner()), message);
        }
        changed.notify_all();
    }
    lock.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
    changed.notify_all();
}

/// One direction of an in-memory byte pipe.
#[derive(Default)]
struct Pipe {
    bytes: VecDeque<u8>,
    /// The writer is gone: reads end once the bytes are read.
    writer_gone: bool,
    /// The reader is gone: writes fail.
    reader_gone: bool,
}
type PipeEnd = Arc<(Mutex<Pipe>, Condvar)>;
struct PipeReader(PipeEnd);
struct PipeWriter(PipeEnd);

fn pipe() -> (PipeReader, PipeWriter) {
    let end: PipeEnd = Arc::default();
    (PipeReader(end.clone()), PipeWriter(end))
}

impl Read for PipeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let (lock, changed) = &*self.0;
        let mut pipe = lock.lock().unwrap_or_else(|e| e.into_inner());
        while pipe.bytes.is_empty() && !pipe.writer_gone {
            pipe = changed.wait(pipe).unwrap_or_else(|e| e.into_inner());
        }
        let n = out.len().min(pipe.bytes.len());
        for (slot, byte) in out.iter_mut().zip(pipe.bytes.drain(..n)) {
            *slot = byte;
        }
        Ok(n)
    }
}
impl Drop for PipeReader {
    fn drop(&mut self) {
        let (lock, changed) = &*self.0;
        lock.lock().unwrap_or_else(|e| e.into_inner()).reader_gone = true;
        changed.notify_all();
    }
}

impl PipeWriter {
    /// Ends the stream: the reader reads what is left, then the end.
    fn close(&self) {
        let (lock, changed) = &*self.0;
        lock.lock().unwrap_or_else(|e| e.into_inner()).writer_gone = true;
        changed.notify_all();
    }
}
impl Write for PipeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let (lock, changed) = &*self.0;
        let mut pipe = lock.lock().unwrap_or_else(|e| e.into_inner());
        if pipe.reader_gone || pipe.writer_gone {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        pipe.bytes.extend(bytes);
        changed.notify_all();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for PipeWriter {
    fn drop(&mut self) {
        self.close();
    }
}
