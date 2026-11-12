//! The run loop: the handshake, the app's requests one at a time, cancellation
//! and events.
//!
//! The reader thread reads the app's lines and gives each request the handles
//! it declares ([`crate::shm`]). A request is *queued* until the handler loop
//! takes it, then *executing*; both claim that state under one lock. A
//! `cancel` for a queued request is answered `cancelled` at once and its
//! handles closed, and the handler loop skips it; for an executing one it sets
//! the flag [`Context::is_cancelled`] reads.
//!
//! In API 2 the app is the only sender of handles and serialises every line
//! itself, so a framing fault is corruption, not input to tolerate: a line
//! short of its handles, handles left over at a clean frame boundary, cut-off
//! control data, an oversized, malformed or undecodable line, or the stream
//! ending inside a line ends the program ([`OnFault`]). API 1 keeps phase 2's
//! recovery and takes no handles.
use crate::host::{Host, Shared};
use crate::shm::{Handle, Incoming, Mapping, Plain, Receive};
use crate::{Command, ControlAdapter, MaskAsk, MaskGenerator, Panel};
use rawmakase_extension_protocol::framing::{FrameError, LineReader};
use rawmakase_extension_protocol::manifest::API;
use rawmakase_extension_protocol::message::Handles;
use rawmakase_extension_protocol::method::{
    AppEvent, AppRequest, CommandRun, Controls, HelloReply, MaskGenerate,
};
use rawmakase_extension_protocol::{ENV_API, ErrorCode, Message, PROTOCOL, RemoteError};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

type EventHandler = Box<dyn FnMut(&AppEvent, &Host) + Send>;

/// An extension's program: its handlers, by contribution id.
#[derive(Default)]
pub struct Extension {
    commands: BTreeMap<String, Box<dyn Command>>,
    panels: BTreeMap<String, Box<dyn Panel>>,
    adapters: BTreeMap<String, Box<dyn ControlAdapter>>,
    mask_generators: BTreeMap<String, Box<dyn MaskGenerator>>,
    on_event: Option<EventHandler>,
}

/// How [`Extension::serve_with_options`] serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServeOptions {
    /// The manifest's `api`, which the app passes in
    /// [`ENV_API`]: 1 (the default)
    /// up to [`API`].
    ///
    /// API 1 takes no handles: a message declaring some is answered
    /// `invalid_request`, a malformed request too, an oversized line is
    /// skipped, and reading goes on. API 2 takes the handles the app passes
    /// with its requests, and a framing fault ends serving: the program exits
    /// under [`Extension::run`], and [`Extension::serve_with_options`] returns
    /// an error.
    pub api: u32,
}
impl Default for ServeOptions {
    fn default() -> Self {
        Self { api: 1 }
    }
}
impl ServeOptions {
    /// The options the app started this program with: the `api` in
    /// [`ENV_API`], 1 when it isn't set.
    pub fn from_env() -> io::Result<Self> {
        Self::from_var(std::env::var_os(ENV_API))
    }

    fn from_var(value: Option<std::ffi::OsString>) -> io::Result<Self> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        let api = value.to_str().and_then(|v| v.parse().ok()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{ENV_API} is {value:?}, not a number"),
            )
        })?;
        Ok(Self { api })
    }

    fn check(self) -> io::Result<Self> {
        if (1..=API).contains(&self.api) {
            Ok(self)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the manifest's api is {}, and this SDK serves 1 to {API}",
                    self.api
                ),
            ))
        }
    }
}

/// What a handler knows of the request it runs for.
pub struct Context<'a> {
    host: &'a Host,
    id: u64,
    cancelled: Arc<AtomicBool>,
    settings: &'a Map<String, Value>,
    dirs: &'a (PathBuf, PathBuf),
}
impl Context<'_> {
    /// The app, for reads and proposals.
    pub fn host(&self) -> &Host {
        self.host
    }
    /// The id of the app's request this handler answers: a proposal names it
    /// as its `op`, and is admitted only while the request is in flight.
    pub fn op(&self) -> u64 {
        self.id
    }
    /// Whether the app cancelled this request; the handler should return
    /// [`ErrorCode::Cancelled`] soon.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    /// The flag [`Context::is_cancelled`] reads, to lend to work that polls
    /// a flag of its own, such as a model run. It is set once the app
    /// cancels this request, and never cleared.
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancelled
    }
    /// The extension's settings, with every change the app reported so far.
    pub fn settings(&self) -> &Map<String, Value> {
        self.settings
    }
    /// The extension's private data folder.
    pub fn data_dir(&self) -> &Path {
        &self.dirs.0
    }
    /// The extension's cache folder.
    pub fn cache_dir(&self) -> &Path {
        &self.dirs.1
    }
    /// Reports how far this request is.
    pub fn progress(&self, done: Option<f64>, text: Option<&str>) {
        let _ = self.host.0.send(&Message::Progress {
            id: self.id,
            done,
            text: text.map(str::to_string),
        });
    }
}

/// What the reader thread hands the handlers.
enum Inbound {
    Request {
        id: u64,
        method: String,
        params: Value,
    },
    Event {
        name: String,
        params: Value,
    },
}

/// One of the app's requests not yet answered.
struct Entry {
    /// Set by the reader when a `cancel` arrives while the handler runs.
    cancelled: Arc<AtomicBool>,
    /// While the request is queued: the handles it carries. The handler loop
    /// takes them when it starts the request, so `None` means executing.
    queued: Option<Vec<Handle>>,
}

/// The app's requests not yet answered, by id. The reader adds them, and the
/// reader's cancel and the handler loop's start claim a request's state under
/// this lock; neither writes to the channel while holding it.
type Requests = Arc<Mutex<HashMap<u64, Entry>>>;

/// What a framing fault in API 2 does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnFault {
    /// Ends the program, which closes every handle it holds:
    /// [`Extension::run`].
    Exit,
    /// Closes the channel and makes serving return the fault: a test, or a
    /// transport of the caller's.
    Close,
}

/// The status a program ends with on a framing fault.
const FAULT_EXIT: i32 = 70;

impl Extension {
    /// An extension with no handlers yet.
    pub fn new() -> Self {
        Self::default()
    }
    /// Implements the `[[command]]` with this id.
    pub fn command(mut self, id: &str, command: impl Command + 'static) -> Self {
        self.commands.insert(id.into(), Box::new(command));
        self
    }
    /// Implements the `[[panel]]` with this id.
    pub fn panel(mut self, id: &str, panel: impl Panel + 'static) -> Self {
        self.panels.insert(id.into(), Box::new(panel));
        self
    }
    /// Implements the `[[control-adapter]]` with this id.
    pub fn adapter(mut self, id: &str, adapter: impl ControlAdapter + 'static) -> Self {
        self.adapters.insert(id.into(), Box::new(adapter));
        self
    }
    /// Implements the `[[mask-generator]]` with this id.
    pub fn mask_generator(mut self, id: &str, generator: impl MaskGenerator + 'static) -> Self {
        self.mask_generators.insert(id.into(), Box::new(generator));
        self
    }
    /// Called with every event from the app, after the extension's own state
    /// (its settings) has taken it in.
    pub fn on_event(mut self, handler: impl FnMut(&AppEvent, &Host) + Send + 'static) -> Self {
        self.on_event = Some(Box::new(handler));
        self
    }

    /// Attaches the channel the app started this program with and serves it
    /// until the app quits or the channel closes, in the API mode the app
    /// passed ([`ServeOptions::from_env`]).
    pub fn run(self) -> io::Result<()> {
        // Read before attaching, so the mode is known before the first byte.
        let options = ServeOptions::from_env()?.check()?;
        let (reader, writer) = crate::channel::attach_receiving()?;
        self.serve_on(reader, Box::new(writer), options, OnFault::Exit)
    }

    /// Serves the app on this reader and writer: [`Extension::run`] without
    /// attaching, for a test or another transport, in API 1 mode. Returns
    /// when the app sends `quitting` or closes its side, or with an error
    /// when it speaks another protocol.
    pub fn serve(
        self,
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
    ) -> io::Result<()> {
        self.serve_with_options(reader, writer, ServeOptions::default())
    }

    /// [`Extension::serve`] in the mode `options` sets. Fails at once when
    /// its `api` isn't one this SDK serves.
    ///
    /// A plain reader carries bytes alone, so on Linux and macOS no handle
    /// reaches it: an API 2 request declaring handles is then short of them,
    /// a framing fault that ends serving with an error.
    pub fn serve_with_options(
        self,
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        options: ServeOptions,
    ) -> io::Result<()> {
        self.serve_on(
            Box::new(Plain(reader)),
            Box::new(writer),
            options,
            OnFault::Close,
        )
    }

    /// Serves on a receiving end that may carry handles.
    pub(crate) fn serve_on(
        mut self,
        reader: Box<dyn Receive>,
        writer: Box<dyn Write + Send>,
        options: ServeOptions,
        on_fault: OnFault,
    ) -> io::Result<()> {
        options.check()?;
        let shared = Shared::new(writer);
        let host = Host(shared.clone());
        let requests: Requests = Arc::default();
        let fault = Arc::new(Mutex::new(None));
        let (inbound, queue) = mpsc::channel();
        {
            let reading = Reading {
                shared,
                requests: requests.clone(),
                inbound,
                api: options.api,
                on_fault,
                fault: fault.clone(),
            };
            // Detached: it ends with the channel, which outlives this call only
            // in a program that keeps running after serving.
            std::thread::Builder::new()
                .name("extension reader".into())
                .spawn(move || reading.run(reader))?;
        }
        let result = self.handle(&host, &requests, &queue);
        for adapter in self.adapters.values_mut() {
            adapter.stop();
        }
        if let Some(why) = fault.lock().unwrap_or_else(|e| e.into_inner()).take() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, why));
        }
        result
    }

    fn handle(
        &mut self,
        host: &Host,
        requests: &Requests,
        queue: &mpsc::Receiver<Inbound>,
    ) -> io::Result<()> {
        let mut settings = Map::new();
        let mut dirs = (PathBuf::new(), PathBuf::new());
        let mut greeted = false;
        for inbound in queue {
            match inbound {
                Inbound::Event { name, params } => {
                    let Ok(event) = AppEvent::parse(&name, params) else {
                        continue;
                    };
                    if let AppEvent::SettingChanged(change) = &event {
                        settings.insert(change.id.clone(), change.value.clone());
                    }
                    if let Some(handler) = &mut self.on_event {
                        handler(&event, host);
                    }
                    if matches!(event, AppEvent::Quitting(_)) {
                        return Ok(());
                    }
                }
                Inbound::Request { id, method, params } => {
                    // Claims the request: from here a cancel sets its flag. A
                    // request missing was cancelled while queued, and its
                    // cancellation answered it.
                    let (cancelled, mut handles) = {
                        let mut requests = requests.lock().unwrap_or_else(|e| e.into_inner());
                        let Some(entry) = requests.get_mut(&id) else {
                            continue;
                        };
                        (entry.cancelled.clone(), entry.queued.take())
                    };
                    let result = if cancelled.load(Ordering::SeqCst) {
                        Err(RemoteError::new(
                            ErrorCode::Cancelled,
                            "cancelled before it ran",
                        ))
                    } else {
                        match AppRequest::parse(&method, params) {
                            Err(e) => Err(e),
                            Ok(AppRequest::Hello(hello)) => {
                                drop(handles.take());
                                if hello.protocol != PROTOCOL {
                                    let _ = host.0.send(&Message::reply(
                                        id,
                                        Err(RemoteError::new(
                                            ErrorCode::UnsupportedProtocol,
                                            format!(
                                                "this extension speaks protocol {PROTOCOL}, the app {}",
                                                hello.protocol
                                            ),
                                        )),
                                    ));
                                    return Err(io::Error::other(format!(
                                        "RAWmakase speaks extension protocol {}, this program {PROTOCOL}",
                                        hello.protocol
                                    )));
                                }
                                greeted = true;
                                settings = hello.settings;
                                hello.data_dir.clone_into(
                                    &mut host.0.data_dir.lock().unwrap_or_else(|e| e.into_inner()),
                                );
                                dirs = (hello.data_dir, hello.cache_dir);
                                host.0.session.store(hello.session, Ordering::SeqCst);
                                self.reply(host, requests, id, Ok(self.hello_reply()));
                                self.start_adapters(host);
                                continue;
                            }
                            Ok(_) if !greeted => Err(RemoteError::new(
                                ErrorCode::InvalidRequest,
                                "the session starts with hello",
                            )),
                            Ok(request) => {
                                let cx = Context {
                                    host,
                                    id,
                                    cancelled,
                                    settings: &settings,
                                    dirs: &dirs,
                                };
                                // The handles go when the handler is done,
                                // before its reply.
                                let handles = handles.take().unwrap_or_default();
                                self.dispatch(&cx, request, handles)
                            }
                        }
                    };
                    // Every other path, a cancel that won the race with the
                    // claim among them, closes the handles here: the app may
                    // end their lease as soon as it reads the reply.
                    drop(handles);
                    self.reply(host, requests, id, result);
                }
            }
        }
        Ok(())
    }

    fn reply(&self, host: &Host, requests: &Requests, id: u64, result: Result<Value, RemoteError>) {
        requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        let _ = host.0.send(&Message::reply(id, result));
    }

    fn hello_reply(&self) -> Value {
        let commands = self.commands.keys().map(|id| format!("command/{id}"));
        let panels = self.panels.keys().map(|id| format!("panel/{id}"));
        let adapters = self
            .adapters
            .keys()
            .map(|id| format!("control-adapter/{id}"));
        let generators = self
            .mask_generators
            .keys()
            .map(|id| format!("mask-generator/{id}"));
        let reply = HelloReply {
            protocol: PROTOCOL,
            implements: commands
                .chain(panels)
                .chain(adapters)
                .chain(generators)
                .collect(),
        };
        serde_json::to_value(reply).expect("a reply is JSON")
    }

    fn start_adapters(&mut self, host: &Host) {
        for (id, adapter) in &mut self.adapters {
            if let Err(e) = adapter.start(host.clone()) {
                let _ = host.notify(
                    rawmakase_extension_protocol::method::Level::Error,
                    &format!("The {id} adapter didn't start: {e}"),
                );
            }
        }
    }

    fn dispatch(
        &mut self,
        cx: &Context,
        request: AppRequest,
        handles: Vec<Handle>,
    ) -> Result<Value, RemoteError> {
        let unknown = |kind: &str, id: &str| {
            RemoteError::new(
                ErrorCode::InvalidRequest,
                format!("this program implements no {kind} {id}"),
            )
        };
        let controls = |controls: Result<Vec<_>, RemoteError>| {
            controls.map(|controls| {
                serde_json::to_value(Controls { controls }).expect("controls are JSON")
            })
        };
        match request {
            AppRequest::Hello(_) => unreachable!("handled before dispatch"),
            AppRequest::CommandRun(run) => {
                let command: CommandRun = run;
                command.viewer_input()?;
                match self.commands.get_mut(&command.command) {
                    Some(handler) => handler.run(cx, command),
                    None => Err(unknown("command", &command.command)),
                }
            }
            AppRequest::PanelOpen(open) => match self.panels.get_mut(&open.panel) {
                Some(panel) => controls(panel.open(cx)),
                None => Err(unknown("panel", &open.panel)),
            },
            AppRequest::PanelChanged(change) => match self.panels.get_mut(&change.panel) {
                Some(panel) => controls(panel.changed(cx, &change.control, change.value)),
                None => Err(unknown("panel", &change.panel)),
            },
            AppRequest::PanelClicked(click) => match self.panels.get_mut(&click.panel) {
                Some(panel) => controls(panel.clicked(cx, &click.control)),
                None => Err(unknown("panel", &click.panel)),
            },
            AppRequest::MaskGenerate(request) => {
                request.validate()?;
                let Some(generator) = self.mask_generators.get_mut(&request.generator) else {
                    return Err(unknown("mask generator", &request.generator));
                };
                let (image, mut coverage) = map_mask(&request, &handles)?;
                let ask = MaskAsk {
                    image: image.bytes(),
                    coverage: coverage
                        .bytes_mut()
                        .expect("the coverage is mapped writable"),
                    request,
                };
                let result = generator.generate(cx, ask);
                // Unmapped, and then closed, before the reply.
                drop((image, coverage, handles));
                result.map(|done| serde_json::to_value(done).expect("a result is JSON"))
            }
            AppRequest::MaskSetup(setup) => match self.mask_generators.get_mut(&setup.generator) {
                Some(generator) => generator.setup(cx, &setup.generator).map(|()| Value::Null),
                None => Err(unknown("mask generator", &setup.generator)),
            },
        }
    }
}

/// Maps a mask request's image to read and its coverage to write.
fn map_mask(request: &MaskGenerate, handles: &[Handle]) -> Result<(Mapping, Mapping), RemoteError> {
    let map = |buffer: &rawmakase_extension_protocol::buffer::Buffer, name: &str| {
        let handle = handles.get(buffer.handle as usize).ok_or_else(|| {
            RemoteError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "the {name} is in handle {}, and the request carries {}",
                    buffer.handle,
                    handles.len()
                ),
            )
        })?;
        Mapping::map(handle, buffer).map_err(|e| {
            RemoteError::new(
                ErrorCode::InvalidRequest,
                format!("the {name} doesn't map: {e}"),
            )
        })
    };
    Ok((
        map(&request.image, "image")?,
        map(&request.coverage, "coverage")?,
    ))
}

/// What the reader thread holds.
struct Reading {
    shared: Arc<Shared>,
    requests: Requests,
    inbound: mpsc::Sender<Inbound>,
    api: u32,
    on_fault: OnFault,
    /// Why serving ended on a framing fault, for [`Extension::serve_on`].
    fault: Arc<Mutex<Option<String>>>,
}

impl Reading {
    /// Hands requests and events to the handlers, gives requests their
    /// handles, answers cancels and routes the app's replies, until the
    /// channel closes or breaks its framing.
    fn run(self, reader: Box<dyn Receive>) {
        let mut lines = LineReader::new(Incoming::new(reader));
        if let Err(why) = self.read(&mut lines) {
            self.fault(&why, lines.get_mut());
        }
        // Handles of requests never started, and any left unassigned, close.
        drop(lines);
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
            .for_each(|entry| drop(entry.queued.take()));
        self.shared.close();
    }

    /// Reads until the end of the stream; a framing fault in API 2 is an
    /// error naming it.
    fn read(&self, lines: &mut LineReader<Incoming>) -> Result<(), String> {
        let strict = self.api >= 2;
        loop {
            let line = match lines.next_line() {
                Ok(Some(line)) => line,
                // The end at a frame boundary; handles still queued close
                // with the reader.
                Ok(None) => return Ok(()),
                Err(e) if strict && lines.get_mut().truncated => return Err(e.to_string()),
                Err(_) => return Ok(()),
            };
            if !strict {
                // API 1 takes no handles: any that came close now.
                lines.get_mut().handles.clear();
            }
            let line = match line {
                Ok(line) => line,
                Err(e) if strict => return Err(e.to_string()),
                // API 1: an oversized line is skipped, and the stream ending
                // inside one ends it.
                Err(FrameError::TooLong | FrameError::Malformed { .. }) => continue,
                Err(FrameError::Truncated) => return Ok(()),
            };
            let message = Message::decode(&line);
            let declared = Handles::read(&line);
            if !strict {
                self.lenient(message, declared);
                continue;
            }
            let message = message.map_err(|e| e.to_string())?;
            let declared = declared.map_err(|e| e.to_string())?;
            let handles = take(lines.get_mut(), &declared)?;
            // A conforming app's handles all belong to lines it has sent
            // whole: any left once every byte read is a line's is a fault.
            if lines.at_boundary() && !lines.get_mut().handles.is_empty() {
                return Err(format!(
                    "{} handles arrived that no message declared",
                    lines.get_mut().handles.len()
                ));
            }
            if !self.message(message, handles) {
                return Ok(());
            }
        }
    }

    /// API 1: phase 2's recovery. A malformed request with a readable id, and
    /// any request declaring handles, is answered `invalid_request`; other
    /// broken lines are skipped.
    fn lenient(
        &self,
        message: Result<Message, rawmakase_extension_protocol::message::DecodeError>,
        declared: Result<Handles, rawmakase_extension_protocol::message::DecodeError>,
    ) {
        let declares = declared.map_or(true, |d| d.handles > 0 || d.hv.is_some());
        match message {
            Ok(Message::Req { id, .. }) if declares => {
                self.invalid(id, "an API 1 program takes no handles".into());
            }
            Ok(message) => {
                self.message(message, Vec::new());
            }
            Err(e) => {
                if let Some(id) = e.request {
                    self.invalid(id, e.reason);
                }
            }
        }
    }

    fn invalid(&self, id: u64, reason: String) {
        let _ = self.shared.send(&Message::reply(
            id,
            Err(RemoteError::new(ErrorCode::InvalidRequest, reason)),
        ));
    }

    /// Handles one message with the handles it carries. False when the
    /// handler loop is gone.
    fn message(&self, message: Message, handles: Vec<Handle>) -> bool {
        match message {
            Message::Req { id, m, p } => {
                self.requests
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        id,
                        Entry {
                            cancelled: Arc::default(),
                            queued: Some(handles),
                        },
                    );
                let request = Inbound::Request {
                    id,
                    method: m,
                    params: p,
                };
                if self.inbound.send(request).is_err() {
                    return false;
                }
            }
            Message::Cancel { id } => self.cancel(id),
            Message::Ok { id, v } => self.shared.answer(id, Ok(v)),
            Message::Err { id, code, message } => {
                self.shared.answer(id, Err(RemoteError { code, message }));
            }
            Message::Event { e, p } => {
                if self
                    .inbound
                    .send(Inbound::Event { name: e, params: p })
                    .is_err()
                {
                    return false;
                }
            }
            // The app reports no progress to a program.
            Message::Progress { .. } => {}
        }
        true
    }

    /// A queued request is answered `cancelled` now and its handles closed;
    /// an executing one has its flag set.
    fn cancel(&self, id: u64) {
        let handles = {
            let mut requests = self.requests.lock().unwrap_or_else(|e| e.into_inner());
            match requests.get(&id) {
                Some(entry) if entry.queued.is_some() => {
                    requests.remove(&id).and_then(|entry| entry.queued)
                }
                Some(entry) => {
                    entry.cancelled.store(true, Ordering::SeqCst);
                    None
                }
                None => None,
            }
        };
        if let Some(handles) = handles {
            drop(handles);
            let _ = self.shared.send(&Message::reply(
                id,
                Err(RemoteError::new(
                    ErrorCode::Cancelled,
                    "cancelled before it ran",
                )),
            ));
        }
    }

    /// Ends serving on a framing fault, saying why in the program's log.
    fn fault(&self, why: &str, incoming: &Incoming) {
        eprintln!("rawmakase-extension: the channel from RAWmakase broke its framing: {why}");
        match self.on_fault {
            OnFault::Exit => std::process::exit(FAULT_EXIT),
            OnFault::Close => {
                *self.fault.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(format!("a framing fault: {why}"));
                incoming.shutdown();
            }
        }
    }
}

/// The handles a line declares, from those received: on Linux and macOS the
/// next `n` queued, on Windows the values its `hv` lists.
fn take(incoming: &mut Incoming, declared: &Handles) -> Result<Vec<Handle>, String> {
    let wanted = declared.handles as usize;
    if wanted > rawmakase_extension_protocol::MAX_HANDLES {
        return Err(format!(
            "a message declares {wanted} handles, more than the {} one may carry",
            rawmakase_extension_protocol::MAX_HANDLES
        ));
    }
    #[cfg(windows)]
    {
        let values = declared.hv.as_deref().unwrap_or_default();
        let mut distinct = values.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        if values.len() != wanted || distinct.len() != wanted {
            return Err(format!(
                "a message declares {wanted} handles and lists {} distinct values",
                distinct.len()
            ));
        }
        let _ = incoming;
        Ok(crate::shm::adopt(values))
    }
    #[cfg(not(windows))]
    {
        if incoming.handles.len() < wanted {
            return Err(format!(
                "a message declares {wanted} handles and {} arrived",
                incoming.handles.len()
            ));
        }
        Ok(incoming.handles.drain(..wanted).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_mode_comes_from_the_environment_and_defaults_to_1() {
        assert_eq!(ServeOptions::from_var(None).unwrap().api, 1);
        assert_eq!(ServeOptions::from_var(Some("2".into())).unwrap().api, 2);
        assert!(ServeOptions::from_var(Some("two".into())).is_err());
        assert!(ServeOptions::from_var(Some("".into())).is_err());
        assert!(ServeOptions { api: 0 }.check().is_err());
        assert!(ServeOptions { api: API + 1 }.check().is_err());
        assert!(ServeOptions { api: API }.check().is_ok());
    }
}
