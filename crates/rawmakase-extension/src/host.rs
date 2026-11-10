//! The program's side of its requests to the app.
use rawmakase_extension_protocol::controls::Control;
use rawmakase_extension_protocol::framing::write_line;
use rawmakase_extension_protocol::method::{
    AdapterDevice, AdapterOperate, DeviceState, Empty, Level, Notify, Operated, PanelUpdate,
    ProgramEvent, ProgramRequest, ProposeSettings, Proposed, ReadPhotos, SettingsChange, Snapshot,
    Status,
};
use rawmakase_extension_protocol::{ErrorCode, Message, RemoteError};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// What the run loop's reader thread, its handlers and every [`Host`] share.
pub(crate) struct Shared {
    writer: Mutex<Box<dyn Write + Send>>,
    /// The program's requests waiting for the app's reply, by id.
    pending: Mutex<HashMap<u64, mpsc::Sender<Result<Value, RemoteError>>>>,
    next_id: AtomicU64,
    /// The channel closed: nothing more is sent or answered.
    closed: AtomicBool,
    /// The program start's number from `hello`.
    pub(crate) session: AtomicU64,
}

impl Shared {
    pub(crate) fn new(writer: Box<dyn Write + Send>) -> Arc<Self> {
        Arc::new(Self {
            writer: Mutex::new(writer),
            pending: Mutex::default(),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            session: AtomicU64::new(0),
        })
    }

    /// Writes one message. A reply too long for a line goes as
    /// [`ErrorCode::TooLarge`] instead, so the app's request still ends.
    pub(crate) fn send(&self, message: &Message) -> io::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let result = match write_line(&mut *writer, message) {
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => match message {
                Message::Ok { id, .. } => write_line(
                    &mut *writer,
                    &Message::reply(
                        *id,
                        Err(RemoteError::new(
                            ErrorCode::TooLarge,
                            "the reply is longer than a protocol line",
                        )),
                    ),
                ),
                _ => return Err(e),
            },
            other => other,
        };
        if result.is_err() {
            self.close();
        }
        result
    }

    /// Routes the app's reply to the request waiting for it. A reply to no
    /// request waiting (one that timed out) is dropped.
    pub(crate) fn answer(&self, id: u64, result: Result<Value, RemoteError>) {
        let waiting = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        if let Some(waiting) = waiting {
            let _ = waiting.send(result);
        }
    }

    /// The channel closed: every request waiting fails.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let waiting = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        for (_, waiting) in waiting {
            let _ = waiting.send(Err(closed()));
        }
    }
}

fn closed() -> RemoteError {
    RemoteError::new(ErrorCode::Exited, "the channel to RAWmakase closed")
}

/// The app, as a program sees it: requests and events to it. Cloned freely
/// and usable from any thread, so an adapter's device threads report through
/// their own copies.
#[derive(Clone)]
pub struct Host(pub(crate) Arc<Shared>);

/// How long a request waits for the app by default. The app answers reads and
/// proposals within a frame or two; a minute means it is stuck.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

impl Host {
    /// Sends a request and waits for its reply, at most a minute.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, RemoteError> {
        self.request_timeout(method, params, DEFAULT_TIMEOUT)
    }

    /// Sends a request and waits for its reply at most `timeout`; past it the
    /// request is cancelled and fails with [`ErrorCode::Deadline`].
    pub fn request_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RemoteError> {
        let shared = &self.0;
        if shared.closed.load(Ordering::SeqCst) {
            return Err(closed());
        }
        let id = shared.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        if let Err(e) = shared.send(&Message::request(id, method, params)) {
            shared
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return Err(if e.kind() == io::ErrorKind::InvalidInput {
                RemoteError::new(
                    ErrorCode::TooLarge,
                    "the request is longer than a protocol line",
                )
            } else {
                closed()
            });
        }
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(closed()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                shared
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                let _ = shared.send(&Message::Cancel { id });
                Err(RemoteError::new(
                    ErrorCode::Deadline,
                    format!("{method} had no reply in {timeout:?}"),
                ))
            }
        }
    }

    /// Sends one of the protocol's requests and reads its reply as `T`.
    fn call<T: serde::de::DeserializeOwned>(
        &self,
        request: ProgramRequest,
    ) -> Result<T, RemoteError> {
        let (method, params) = request.into_parts();
        let value = self.request(&method, params)?;
        serde_json::from_value(value).map_err(|e| {
            RemoteError::new(ErrorCode::InvalidRequest, format!("{method}'s reply: {e}"))
        })
    }

    /// The Library's selection, with a token for proposals.
    pub fn read_selection(&self) -> Result<Snapshot, RemoteError> {
        self.call(ProgramRequest::ReadSelection(Empty {}))
    }
    /// The photo open in Develop, with a token for proposals.
    pub fn read_open_photo(&self) -> Result<Snapshot, RemoteError> {
        self.call(ProgramRequest::ReadOpenPhoto(Empty {}))
    }
    /// Photos by catalog id, with a token for proposals.
    pub fn read_photos(&self, ids: Vec<i64>) -> Result<Snapshot, RemoteError> {
        self.call(ProgramRequest::ReadPhotos(ReadPhotos { ids }))
    }
    /// The control layer's parameters and actions.
    pub fn read_capabilities(&self) -> Result<Value, RemoteError> {
        self.call(ProgramRequest::ReadCapabilities(Empty {}))
    }
    /// Proposes settings changes for photos of the snapshot `token` names,
    /// during the app's `command.run` request `op` ([`crate::Context::op`]).
    pub fn propose_settings(
        &self,
        op: u64,
        token: String,
        changes: Vec<SettingsChange>,
    ) -> Result<Proposed, RemoteError> {
        self.call(ProgramRequest::ProposeSettings(ProposeSettings {
            op,
            token,
            changes,
        }))
    }
    /// Reports a device's lifecycle. The app applies it ahead of any
    /// operations still queued.
    pub fn device(
        &self,
        adapter: &str,
        device: &str,
        state: DeviceState,
        epoch: u64,
    ) -> Result<(), RemoteError> {
        self.call::<Value>(ProgramRequest::AdapterDevice(AdapterDevice {
            adapter: adapter.into(),
            device: device.into(),
            state,
            epoch,
        }))
        .map(drop)
    }
    /// Sends a device's operations, made in `epoch`.
    pub fn operate(
        &self,
        adapter: &str,
        device: &str,
        epoch: u64,
        ops: Vec<rawmakase_extension_protocol::method::Operation>,
    ) -> Result<Operated, RemoteError> {
        self.call(ProgramRequest::AdapterOperate(AdapterOperate {
            adapter: adapter.into(),
            device: device.into(),
            epoch,
            ops,
        }))
    }
    /// Shows the user a notification.
    pub fn notify(&self, level: Level, text: &str) -> Result<(), RemoteError> {
        self.call::<Value>(ProgramRequest::Notify(Notify {
            level,
            text: text.into(),
        }))
        .map(drop)
    }
    /// Sets the extension's entry in the status line, or removes it.
    pub fn status(&self, text: Option<&str>) -> Result<(), RemoteError> {
        self.call::<Value>(ProgramRequest::Status(Status {
            text: text.map(str::to_string),
        }))
        .map(drop)
    }
    /// Replaces an open panel's controls, unasked.
    pub fn panel_update(&self, panel: &str, controls: Vec<Control>) -> io::Result<()> {
        let (name, params) = ProgramEvent::PanelUpdate(PanelUpdate {
            panel: panel.into(),
            controls,
        })
        .into_parts();
        self.0.send(&Message::event(&name, params))
    }
    /// This program start's number, which the app issued in `hello`; 0 before
    /// it.
    pub fn session(&self) -> u64 {
        self.0.session.load(Ordering::SeqCst)
    }
}
