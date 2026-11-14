//! A stand-in for the app, to test an extension without it: send it requests
//! and events, answer its requests with a function of your own, and look at
//! what it sent.
//!
//! The mock talks to the extension over the transport the app uses, a Unix
//! socket pair on Linux and macOS and a pipe on Windows, and passes buffers in
//! shared memory as the app does ([`MockHost::generate_mask`]), so a test goes
//! through the same receiving code as a program the app started.
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
use crate::run::OnFault;
use crate::shm::{Memory, Receive};
use crate::{Extension, ServeOptions};
use rawmakase_extension_protocol::controls::Control;
use rawmakase_extension_protocol::framing::LineReader;
use rawmakase_extension_protocol::message::Handles;
use rawmakase_extension_protocol::method::{
    AppEvent, AppRequest, CommandRun, Controls, Empty, Hello, HelloReply, MaskGenerate,
    MaskGenerated, MaskSetup, PanelChanged, PanelClicked, PanelOpen, RetouchGenerate,
    RetouchGenerated, Snapshot,
};
use rawmakase_extension_protocol::{ErrorCode, Message, PROTOCOL, RemoteError};
use serde_json::{Map, Value};
use std::collections::HashMap;
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
    /// Shared with the listener, which answers the extension's requests;
    /// `None` once [`MockHost::finish`] closed it.
    to_program: Arc<Mutex<Option<Sender>>>,
    state: Arc<(Mutex<State>, Condvar)>,
    next_id: Mutex<u64>,
    api: u32,
    /// The coverage buffers of mask requests not yet read back, by id.
    outputs: Mutex<HashMap<u64, (Memory, usize)>>,
    program: JoinHandle<io::Result<()>>,
}

/// The app's sending end.
#[cfg(unix)]
type Sender = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Sender = io::PipeWriter;

/// The channel: the program's receiving and sending ends, the app's sending
/// and receiving ends.
type Ends = (
    Box<dyn Receive>,
    Box<dyn Write + Send>,
    Sender,
    Box<dyn Read + Send>,
);

#[cfg(unix)]
fn channel() -> io::Result<Ends> {
    let (app, program) = std::os::unix::net::UnixStream::pair()?;
    Ok((
        Box::new(program.try_clone()?),
        Box::new(program),
        app.try_clone()?,
        Box::new(app),
    ))
}

#[cfg(windows)]
fn channel() -> io::Result<Ends> {
    let (app_reader, program_writer) = io::pipe()?;
    let (program_reader, app_writer) = io::pipe()?;
    Ok((
        Box::new(crate::shm::Plain(program_reader)),
        Box::new(program_writer),
        app_writer,
        Box::new(app_reader),
    ))
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
        let (program_reader, program_writer, app_writer, app_reader) =
            channel().expect("the mock's channel");
        let program = std::thread::spawn(move || {
            extension.serve_on(program_reader, program_writer, options, OnFault::Close)
        });
        let state: Arc<(Mutex<State>, Condvar)> = Arc::default();
        let to_program = Arc::new(Mutex::new(Some(app_writer)));
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
            api: options.api,
            outputs: Mutex::default(),
            program,
        }
    }

    fn send(&self, message: &Message) {
        let _ = send(&self.to_program, message, &[]);
    }

    /// Sends a request without waiting; [`MockHost::wait`] reads its reply.
    pub fn send_request(&self, request: AppRequest) -> u64 {
        let id = self.next_id();
        let (method, params) = request.into_parts();
        self.send(&Message::request(id, &method, params));
        id
    }

    fn next_id(&self) -> u64 {
        let mut next = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
        *next += 1;
        *next - 1
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

    /// The handshake, with [`MockHost::hello_params`].
    pub fn hello(&self) -> Result<HelloReply, RemoteError> {
        self.hello_with(Self::hello_params())
    }

    /// What [`MockHost::hello`] sends: no settings, session 1 and folders in
    /// the system's temporary folder, which it doesn't create. A test that
    /// needs settings changes these.
    pub fn hello_params() -> Hello {
        let temp = std::env::temp_dir().join("rawmakase-extension-mock");
        Hello {
            protocol: PROTOCOL,
            app_version: "0.0.0".into(),
            extension: "test.extension".into(),
            settings: Map::new(),
            contributions: Vec::new(),
            data_dir: temp.join("data"),
            cache_dir: temp.join("cache"),
            session: 1,
        }
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

    /// Asks a mask generator for a mask and returns its result with the
    /// coverage it wrote, refusing a result that doesn't validate
    /// ([`MaskGenerated::validate`]) as the app does.
    ///
    /// As the app does, the mock puts `image` in a shared input of
    /// `request.image.bytes` (zero after `image`'s end) and allocates the
    /// coverage, `request.coverage.bytes`, and passes both with the request.
    /// A mock serving API 1 refuses at once with
    /// [`ErrorCode::InvalidRequest`], as the app refuses to pass handles to an
    /// API 1 program.
    pub fn generate_mask(
        &self,
        request: MaskGenerate,
        image: &[u8],
    ) -> Result<(MaskGenerated, Vec<u8>), RemoteError> {
        let id = self.send_mask(request, image)?;
        self.wait_mask(id)
    }

    /// [`MockHost::generate_mask`] without waiting: returns the request's id
    /// for [`MockHost::cancel`] and [`MockHost::wait_mask`].
    pub fn send_mask(&self, request: MaskGenerate, image: &[u8]) -> Result<u64, RemoteError> {
        if self.api < 2 {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                "an API 1 program takes no handles",
            ));
        }
        let failed =
            |e: io::Error| RemoteError::new(ErrorCode::Unavailable, format!("shared memory: {e}"));
        let size = |bytes: u64| usize::try_from(bytes).unwrap_or(usize::MAX);
        let mut pixels = image.to_vec();
        pixels.resize(size(request.image.bytes).max(image.len()), 0);
        let input = Memory::input(&pixels).map_err(failed)?;
        let coverage_len = size(request.coverage.bytes);
        let output = Memory::output(coverage_len).map_err(failed)?;
        // In the order the descriptors index them.
        let buffers = if request.image.handle == 0 {
            [&input, &output]
        } else {
            [&output, &input]
        };
        let id = self.next_id();
        let (method, params) = AppRequest::MaskGenerate(request).into_parts();
        send(
            &self.to_program,
            &Message::request(id, &method, params),
            &buffers,
        )
        .map_err(|e| RemoteError::new(ErrorCode::Exited, e.to_string()))?;
        drop(input);
        self.outputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, (output, coverage_len));
        Ok(id)
    }

    /// The reply to a mask request [`MockHost::send_mask`] sent, with the
    /// coverage the program wrote.
    pub fn wait_mask(&self, id: u64) -> Result<(MaskGenerated, Vec<u8>), RemoteError> {
        let reply = self.wait(id);
        let output = self
            .outputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        let result: MaskGenerated = decode(reply?)?;
        result.validate()?;
        let coverage = output.map_or_else(Vec::new, |(memory, len)| {
            memory.contents()[..len.min(memory.contents().len())].to_vec()
        });
        Ok((result, coverage))
    }

    /// Asks a retouch generator for a region's new pixels and returns its
    /// answer with the result it wrote, refusing an answer that doesn't
    /// validate ([`RetouchGenerated::validate`]) as the app does.
    ///
    /// As the app does, the mock puts `image` and `coverage` in shared inputs
    /// of their descriptors' `bytes` (zero after their ends), allocates the
    /// result, and passes the three in the order their descriptors' handles
    /// name. A mock serving an `api` below 3 refuses at once with
    /// [`ErrorCode::InvalidRequest`], as the app never sends this request to
    /// such a program.
    pub fn generate_retouch(
        &self,
        request: RetouchGenerate,
        image: &[u8],
        coverage: &[u8],
    ) -> Result<(RetouchGenerated, Vec<u8>), RemoteError> {
        let id = self.send_retouch(request, image, coverage)?;
        self.wait_retouch(id)
    }

    /// [`MockHost::generate_retouch`] without waiting: returns the request's
    /// id for [`MockHost::cancel`] and [`MockHost::wait_retouch`].
    pub fn send_retouch(
        &self,
        request: RetouchGenerate,
        image: &[u8],
        coverage: &[u8],
    ) -> Result<u64, RemoteError> {
        if self.api < rawmakase_extension_protocol::manifest::RETOUCH_GENERATOR_API {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                "the app sends retouch requests to API 3 programs only",
            ));
        }
        let failed =
            |e: io::Error| RemoteError::new(ErrorCode::Unavailable, format!("shared memory: {e}"));
        let size = |bytes: u64| usize::try_from(bytes).unwrap_or(usize::MAX);
        let input = |data: &[u8], bytes: u64| {
            let mut pixels = data.to_vec();
            pixels.resize(size(bytes).max(data.len()), 0);
            Memory::input(&pixels).map_err(failed)
        };
        let image_memory = input(image, request.image.bytes)?;
        let coverage_memory = input(coverage, request.coverage.bytes)?;
        let result_len = size(request.result.bytes);
        let output = Memory::output(result_len).map_err(failed)?;
        let mut buffers = [
            (request.image.handle, &image_memory),
            (request.coverage.handle, &coverage_memory),
            (request.result.handle, &output),
        ];
        buffers.sort_by_key(|(handle, _)| *handle);
        let buffers = buffers.map(|(_, memory)| memory);
        let id = self.next_id();
        let (method, params) = AppRequest::RetouchGenerate(request).into_parts();
        send(
            &self.to_program,
            &Message::request(id, &method, params),
            &buffers,
        )
        .map_err(|e| RemoteError::new(ErrorCode::Exited, e.to_string()))?;
        drop((image_memory, coverage_memory));
        self.outputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, (output, result_len));
        Ok(id)
    }

    /// The reply to a retouch request [`MockHost::send_retouch`] sent, with
    /// the result the program wrote.
    pub fn wait_retouch(&self, id: u64) -> Result<(RetouchGenerated, Vec<u8>), RemoteError> {
        let reply = self.wait(id);
        let output = self
            .outputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        let answer: RetouchGenerated = decode(reply?)?;
        answer.validate()?;
        let result = output.map_or_else(Vec::new, |(memory, len)| {
            memory.contents()[..len.min(memory.contents().len())].to_vec()
        });
        Ok((answer, result))
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
        close(&self.to_program);
        self.outputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.program
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the extension panicked")))
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, RemoteError> {
    serde_json::from_value(value)
        .map_err(|e| RemoteError::new(ErrorCode::InvalidRequest, e.to_string()))
}

/// Writes one line with the buffers' handles, as the app does: on Linux and
/// macOS as `SCM_RIGHTS` with its first byte, on Windows duplicated first and
/// listed in `hv`, closed again if the line isn't sent.
fn send(
    to_program: &Mutex<Option<Sender>>,
    message: &Message,
    buffers: &[&Memory],
) -> io::Result<()> {
    let mut sender = to_program.lock().unwrap_or_else(|e| e.into_inner());
    let Some(sender) = sender.as_mut() else {
        return Err(io::Error::from(io::ErrorKind::BrokenPipe));
    };
    let invalid = |e| io::Error::new(io::ErrorKind::InvalidInput, e);
    #[cfg(unix)]
    {
        let handles = Handles {
            handles: buffers.len() as u32,
            hv: None,
        };
        let mut line = message.encode_with(&handles).map_err(invalid)?;
        line.push(b'\n');
        let fds: Vec<_> = buffers.iter().map(|b| b.handle()).collect();
        crate::shm::send(sender, &line, &fds)
    }
    #[cfg(windows)]
    {
        let mut values = Vec::new();
        for buffer in buffers {
            match buffer.duplicate() {
                Ok(value) => values.push(value),
                Err(e) => {
                    crate::shm::close(&values);
                    return Err(e);
                }
            }
        }
        let handles = Handles {
            handles: values.len() as u32,
            hv: (!values.is_empty()).then(|| values.clone()),
        };
        let line = message.encode_with(&handles).map_err(invalid);
        let written = line.and_then(|mut line| {
            line.push(b'\n');
            sender.write_all(&line).and_then(|()| sender.flush())
        });
        if written.is_err() {
            crate::shm::close(&values);
        }
        written
    }
}

/// Ends the stream to the program: it reads what is left, then the end.
fn close(to_program: &Mutex<Option<Sender>>) {
    let sender = to_program.lock().unwrap_or_else(|e| e.into_inner()).take();
    #[cfg(unix)]
    if let Some(sender) = sender {
        let _ = sender.shutdown(std::net::Shutdown::Write);
    }
    #[cfg(windows)]
    drop(sender);
}

/// The mock's reader: records what the extension sends and answers its
/// requests.
fn listen(
    reader: Box<dyn Read + Send>,
    state: &(Mutex<State>, Condvar),
    to_program: &Mutex<Option<Sender>>,
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
            let _ = send(to_program, &Message::reply(id, reply.clone()), &[]);
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
