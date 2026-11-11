//! The run loop: the handshake, the app's requests one at a time, cancellation
//! and events.
use crate::host::{Host, Shared};
use crate::{Command, ControlAdapter, Panel};
use rawmakase_extension_protocol::framing::{LineError, LineReader};
use rawmakase_extension_protocol::method::{
    AppEvent, AppRequest, CommandRun, Controls, HelloReply,
};
use rawmakase_extension_protocol::{ErrorCode, Message, PROTOCOL, RemoteError};
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
    on_event: Option<EventHandler>,
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

/// The cancellation flags of the app's requests not yet answered, set by the
/// reader thread when a `cancel` arrives, before or while the handler runs.
type Cancels = Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>;

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
    /// Called with every event from the app, after the extension's own state
    /// (its settings) has taken it in.
    pub fn on_event(mut self, handler: impl FnMut(&AppEvent, &Host) + Send + 'static) -> Self {
        self.on_event = Some(Box::new(handler));
        self
    }

    /// Attaches the channel the app started this program with and serves it
    /// until the app quits or the channel closes.
    pub fn run(self) -> io::Result<()> {
        let channel = crate::channel::attach()?;
        self.serve(channel.reader, channel.writer)
    }

    /// Serves the app on this reader and writer: [`Extension::run`] without
    /// attaching, for a test or another transport. Returns when the app sends
    /// `quitting` or closes its side, or with an error when it speaks another
    /// protocol.
    pub fn serve(
        mut self,
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
    ) -> io::Result<()> {
        let shared = Shared::new(Box::new(writer));
        let host = Host(shared.clone());
        let cancels: Cancels = Arc::default();
        let (inbound, requests) = mpsc::channel();
        {
            let cancels = cancels.clone();
            // Detached: it ends with the channel, which outlives this call only
            // in a program that keeps running after serving.
            std::thread::Builder::new()
                .name("extension reader".into())
                .spawn(move || read(reader, &shared, &cancels, &inbound))?;
        }
        let result = self.handle(&host, &cancels, &requests);
        for adapter in self.adapters.values_mut() {
            adapter.stop();
        }
        result
    }

    fn handle(
        &mut self,
        host: &Host,
        cancels: &Cancels,
        requests: &mpsc::Receiver<Inbound>,
    ) -> io::Result<()> {
        let mut settings = Map::new();
        let mut dirs = (PathBuf::new(), PathBuf::new());
        let mut greeted = false;
        for inbound in requests {
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
                    let cancelled = cancels
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(&id)
                        .cloned()
                        .unwrap_or_default();
                    let result = if cancelled.load(Ordering::SeqCst) {
                        Err(RemoteError::new(
                            ErrorCode::Cancelled,
                            "cancelled before it ran",
                        ))
                    } else {
                        match AppRequest::parse(&method, params) {
                            Err(e) => Err(e),
                            Ok(AppRequest::Hello(hello)) => {
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
                                self.reply(host, cancels, id, Ok(self.hello_reply()));
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
                                self.dispatch(&cx, request)
                            }
                        }
                    };
                    self.reply(host, cancels, id, result);
                }
            }
        }
        Ok(())
    }

    fn reply(&self, host: &Host, cancels: &Cancels, id: u64, result: Result<Value, RemoteError>) {
        cancels
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
        let reply = HelloReply {
            protocol: PROTOCOL,
            implements: commands.chain(panels).chain(adapters).collect(),
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

    fn dispatch(&mut self, cx: &Context, request: AppRequest) -> Result<Value, RemoteError> {
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
        }
    }
}

/// The reader thread: hands requests and events to the handlers, sets
/// cancellation flags and routes the app's replies, until the channel closes.
fn read(reader: impl Read, shared: &Shared, cancels: &Cancels, inbound: &mpsc::Sender<Inbound>) {
    let mut lines = LineReader::new(reader);
    while let Ok(Some(line)) = lines.next_message() {
        match line {
            Ok(Message::Req { id, m, p }) => {
                cancels
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(id, Arc::default());
                if inbound
                    .send(Inbound::Request {
                        id,
                        method: m,
                        params: p,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Ok(Message::Cancel { id }) => {
                if let Some(flag) = cancels.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            Ok(Message::Ok { id, v }) => shared.answer(id, Ok(v)),
            Ok(Message::Err { id, code, message }) => {
                shared.answer(id, Err(RemoteError { code, message }))
            }
            Ok(Message::Event { e, p }) => {
                if inbound.send(Inbound::Event { name: e, params: p }).is_err() {
                    break;
                }
            }
            // The app reports no progress to a program.
            Ok(Message::Progress { .. }) => {}
            Err(LineError::Decode(e)) => {
                if let Some(id) = e.request {
                    let _ = shared.send(&Message::reply(
                        id,
                        Err(RemoteError::new(ErrorCode::InvalidRequest, e.reason)),
                    ));
                }
            }
            Err(LineError::Frame(_)) => {}
        }
    }
    shared.close();
}
