//! Versioned, bounded local control transport. Every request carries its own
//! reply channel; a timed-out request still in the queue is cancelled atomically.
use super::{Action, Msg, Sender, parse_action, shortcut_action};
use crate::app::shell::commands::{
    self, Command, Error, Operation, Param, PhotoTarget, PresetTarget, Target, Until,
};
use crate::catalog::PhotoId;
use eframe::egui;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const ANSWER: Duration = Duration::from_secs(3);
const MAX_REQUEST: u64 = 64 * 1024;
const MAX_CLIENTS: usize = 16;

pub(super) struct Request {
    pub messages: Vec<Msg>,
    pub reply: mpsc::SyncSender<commands::Result<commands::Reply>>,
    phase: Arc<AtomicU8>, // pending, executing, cancelled
    deadline: Instant,
    stop: Arc<AtomicBool>,
}
impl Request {
    pub(crate) fn begin(&self) -> bool {
        if self.stop.load(Ordering::SeqCst) || Instant::now() >= self.deadline {
            let _ = self
                .phase
                .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst);
            return false;
        }
        self.phase
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new("invalid_request", message)
}

pub(super) fn command(request: &Value) -> commands::Result<Vec<Msg>> {
    use crate::color::OutputSpace;
    use rawmakase_protocol::request::{self as wire, Press, Request, Until as WireUntil};
    if request
        .get("protocol")
        .is_some_and(|v| v.as_u64() != Some(u64::from(commands::PROTOCOL)))
    {
        return Err(Error::new(
            "unsupported_protocol",
            "Supported protocol version is 1",
        ));
    }
    let target: Target = match request.get("target") {
        Some(v) => serde_json::from_value::<rawmakase_protocol::Target>(v.clone())
            .map_err(|e| invalid(e.to_string()))?
            .into(),
        None => Target::default(),
    };
    // The envelope's fields sit beside the command's, which ignores them.
    let parsed = Request::deserialize(request).map_err(|e| invalid(e.to_string()))?;
    // A parameter for set or turn: one value, never a whole band.
    let param = |name: &str| match Param::parse(name) {
        None => Err(invalid("Unknown parameter")),
        Some(Param::Band(_)) => Err(invalid("Use an explicit band channel, such as band3.sat")),
        Some(p) => Ok(p),
    };
    let operation = match parsed {
        Request::State => Operation::State,
        Request::Capabilities => Operation::Capabilities,
        Request::Photos {
            query,
            offset,
            limit,
        } => Operation::Photos {
            query: query.unwrap_or_default(),
            offset: offset.unwrap_or(0).min(usize::MAX as u64) as usize,
            limit: limit
                .unwrap_or(wire::DEFAULT_PHOTOS)
                .clamp(1, wire::MAX_PHOTOS) as usize,
        },
        Request::Presets { group } => Operation::Presets { group },
        Request::Preset { id, name, group } => Operation::Preset(match (id, name) {
            (Some(id), _) => PresetTarget::Id(id),
            (None, Some(name)) => PresetTarget::Name { name, group },
            _ => return Err(invalid("preset requires id or name")),
        }),
        Request::Curve { channel, points } => Operation::Curve(channel, points),
        Request::Set { param: name, value } => Operation::Set(param(&name)?, value as f32),
        Request::Turn { param: name, ticks } => Operation::Adjust(param(&name)?, ticks),
        Request::Action { action: name } => {
            let action = commands::Action::parse(&name)
                .or_else(|| match parse_action(&name)? {
                    Action::Key(k, m) => shortcut_action(k, m),
                    Action::Mixer(c) => Some(commands::Action::Mixer(c)),
                    Action::ToggleMono => Some(commands::Action::ToggleMono),
                    Action::Named(a) => Some(a),
                    Action::Hold(_) => None,
                })
                .ok_or_else(|| {
                    invalid("Unknown action; use capabilities to list supported actions")
                })?;
            Operation::Action(action)
        }
        Request::Open { id, name } => Operation::Open(match (id, name) {
            (Some(id), _) => PhotoTarget::Id(PhotoId(id)),
            (None, Some(name)) => PhotoTarget::Name(name),
            _ => return Err(invalid("open requires id or name")),
        }),
        Request::Search { text } => Operation::Search(text),
        Request::Module { module } => Operation::Module(module == wire::Module::Develop),
        Request::Photo { step } => Operation::Navigate(step),
        Request::Save => Operation::Save,
        Request::Export {
            path,
            max_edge,
            color_space,
        } => Operation::Output {
            path,
            max_edge: max_edge.unwrap_or(0),
            space: match color_space.unwrap_or_default() {
                wire::ColorSpace::Srgb => OutputSpace::Srgb,
                wire::ColorSpace::AdobeRgb => OutputSpace::AdobeRgb,
                wire::ColorSpace::Prophoto => OutputSpace::ProPhoto,
            },
            preview: false,
        },
        Request::Preview { path, max_edge } => Operation::Output {
            path,
            max_edge: max_edge.unwrap_or(wire::DEFAULT_PREVIEW_EDGE),
            space: OutputSpace::Srgb,
            preview: true,
        },
        Request::BuildPreviews { kind } => Operation::BuildPreviews(match kind {
            wire::PreviewKind::Standard => crate::catalog::preview_cache::PreviewKind::Standard,
            wire::PreviewKind::OneToOne => crate::catalog::preview_cache::PreviewKind::OneToOne,
        }),
        Request::BuildEditablePreviews => Operation::BuildEditablePreviews,
        Request::DiscardEditablePreviews { include_offline } => {
            Operation::DiscardEditablePreviews { include_offline }
        }
        Request::Job { job_id } => Operation::Job(job_id),
        // Answered once the photo is loaded or the job finished, or after
        // timeout_ms, within clients' read timeout.
        Request::Wait { until, timeout_ms } => Operation::Wait(
            match until {
                WireUntil::Loaded { photo_id } => Until::Loaded(PhotoId(photo_id)),
                WireUntil::Job { job_id } => Until::Job(job_id),
            },
            Duration::from_millis(timeout_ms.unwrap_or(wire::DEFAULT_WAIT_MS)),
        ),
        // Legacy device-level commands remain an adapter. Explicit targets
        // belong to semantic commands, never to mutable device mappings.
        Request::Cc { .. } | Request::Note { .. } if request.get("target").is_some() => {
            return Err(invalid("Use set/turn/action for explicit targets"));
        }
        Request::Cc { cc, value } => return Ok(vec![Msg::Cc(cc, value)]),
        Request::Note { note, press } => {
            return Ok(match press {
                Press::Click => vec![Msg::Note(note, true), Msg::Note(note, false)],
                Press::Down => vec![Msg::Note(note, true)],
                Press::Up => vec![Msg::Note(note, false)],
            });
        }
    };
    Ok(vec![Msg::Command(Command { operation, target })])
}

fn handle(
    line: &str,
    token: &str,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    stop: &Arc<AtomicBool>,
    timeout: Duration,
) -> commands::Result<Value> {
    let request: Value = serde_json::from_str(line).map_err(|e| invalid(e.to_string()))?;
    if request["token"].as_str() != Some(token) {
        return Err(Error::new("unauthorized", "Wrong token; read control.json"));
    }
    if stop.load(Ordering::SeqCst) {
        return Err(Error::new("disabled", "External control is disabled"));
    }
    let messages = command(&request)?;
    // A wait is answered by its own time, not the usual answer's.
    let timeout = match messages[..] {
        [
            Msg::Command(Command {
                operation: Operation::Wait(_, wait),
                ..
            }),
        ] => timeout + wait,
        _ => timeout,
    };
    let (reply, rx) = mpsc::sync_channel(1);
    let phase = Arc::new(AtomicU8::new(0));
    let queued = Request {
        messages,
        reply,
        phase: phase.clone(),
        deadline: Instant::now() + timeout,
        stop: stop.clone(),
    };
    tx.try_send(Msg::Request(queued)).map_err(|e| match e {
        mpsc::TrySendError::Full(_) => Error::new("busy", "The command queue is full"),
        mpsc::TrySendError::Disconnected(_) => Error::new("closed", "RAWmakase is closing"),
    })?;
    ctx.request_repaint();
    match rx.recv_timeout(timeout) {
        Ok(result) => {
            result.map(|reply| serde_json::to_value(reply).expect("serializable command reply"))
        }
        Err(_) => {
            if phase
                .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
                || phase.load(Ordering::SeqCst) == 2
            {
                Err(Error::new(
                    "cancelled",
                    "The command did not start in time and was cancelled",
                ))
            } else {
                Err(Error::new(
                    "outcome_unknown",
                    "Execution started but the reply timed out; inspect state before retrying",
                ))
            }
        }
    }
}
fn serve(
    mut stream: TcpStream,
    token: &str,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    stop: &Arc<AtomicBool>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut line = String::new();
    let read = stream
        .try_clone()
        .and_then(|s| BufReader::new(s.take(MAX_REQUEST + 1)).read_line(&mut line));
    let result = match read {
        Ok(0) => return,
        Ok(_) if line.len() as u64 > MAX_REQUEST || !line.ends_with('\n') => Err(invalid(
            "Request must be a newline-terminated JSON object of at most 64 KiB",
        )),
        Ok(_) => handle(&line, token, tx, ctx, stop, ANSWER),
        Err(e) => Err(invalid(e.to_string())),
    };
    let id = serde_json::from_str::<Value>(&line)
        .ok()
        .and_then(|v| v.get("request_id").cloned());
    let mut reply = match result {
        Ok(mut body) => {
            body["ok"] = true.into();
            body
        }
        Err(error) => json!({"ok":false,"error":error.message,"code":error.code}),
    };
    reply["protocol"] = commands::PROTOCOL.into();
    reply["request_id"] = id.into();
    let _ = writeln!(stream, "{reply}");
}
fn spawn(
    listener: TcpListener,
    token: String,
    tx: Sender<Msg>,
    ctx: egui::Context,
    stop: Arc<AtomicBool>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("control-socket".into())
        .spawn(move || {
            let clients = Arc::new(AtomicUsize::new(0));
            let mut connections = Vec::new();
            for stream in listener.incoming().flatten() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                connections.retain(|c: &std::thread::JoinHandle<()>| !c.is_finished());
                if clients.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
                    clients.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let (token, tx, ctx, stop, clients) = (
                    token.clone(),
                    tx.clone(),
                    ctx.clone(),
                    stop.clone(),
                    clients.clone(),
                );
                let count = clients.clone();
                let spawned = std::thread::Builder::new()
                    .name("control-request".into())
                    .spawn(move || {
                        serve(stream, &token, &tx, &ctx, &stop);
                        clients.fetch_sub(1, Ordering::SeqCst);
                    });
                match spawned {
                    Ok(connection) => connections.push(connection),
                    Err(_) => {
                        count.fetch_sub(1, Ordering::SeqCst);
                    }
                }
            }
            // Bounded once stopped: each connection's reply comes at once from the
            // closed request queue, and its reads and writes time out.
            for connection in connections {
                let _ = connection.join();
            }
        })
}
fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn write_connection(path: &std::path::Path, port: u16, token: &str) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing data directory"))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    let endpoint = rawmakase_protocol::Endpoint {
        protocol: commands::PROTOCOL,
        port,
        token: token.into(),
        pid: Some(std::process::id()),
    };
    serde_json::to_writer(&mut file, &endpoint)?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub(super) struct Handle {
    stop: Arc<AtomicBool>,
    port: u16,
    token: String,
    path: std::path::PathBuf,
    /// The listener, which ends after the connections it accepted.
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Handle {
    /// Stops accepting connections, to wait for at exit. The listener sees the stop
    /// on the wake-up connection made here; should that fail, it is never woken,
    /// and is left to end with the process.
    pub(in crate::app) fn stop(&mut self) -> crate::app::task::Stopping {
        self.signal_stop();
        crate::app::task::Stopping::new(self.thread.take())
    }
    fn signal_stop(&self) {
        if !self.stop.swap(true, Ordering::SeqCst) {
            let _ = TcpStream::connect_timeout(
                &(Ipv4Addr::LOCALHOST, self.port).into(),
                Duration::from_millis(100),
            );
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.signal_stop();
        let ours = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .is_some_and(|v| v["token"].as_str() == Some(&self.token));
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
pub(super) fn start(tx: Sender<Msg>, ctx: egui::Context) -> Option<Handle> {
    start_at(
        tx,
        ctx,
        rawmakase_protocol::Endpoint::path(&crate::storage::data_dir()),
    )
    .map_err(|e| eprintln!("Control socket: {e}"))
    .ok()
}
fn start_at(
    tx: Sender<Msg>,
    ctx: egui::Context,
    path: std::path::PathBuf,
) -> std::io::Result<Handle> {
    let token = new_token()?;
    let stop = Arc::new(AtomicBool::new(false));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    write_connection(&path, port, &token)?;
    // Made first, so that a listener that cannot start removes the connection file.
    let mut handle = Handle {
        stop: stop.clone(),
        port,
        token: token.clone(),
        path,
        thread: None,
    };
    handle.thread = Some(spawn(listener, token, tx, ctx, stop)?);
    Ok(handle)
}

#[cfg(test)]
pub(super) fn test_request(
    messages: Vec<Msg>,
) -> (Request, mpsc::Receiver<commands::Result<commands::Reply>>) {
    let (reply, rx) = mpsc::sync_channel(1);
    (
        Request {
            messages,
            reply,
            phase: Arc::default(),
            deadline: Instant::now() + Duration::from_secs(30),
            stop: Arc::default(),
        },
        rx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_numbers_scopes_and_versions() {
        for value in [
            json!({"cmd":"set","param":"exposure","value":1e100}),
            json!({"cmd":"turn","param":"exposure","ticks":1.5}),
            json!({"cmd":"cc","cc":1.5,"value":1}),
            json!({"cmd":"state","protocol":2}),
            json!({"cmd":"set","param":"band1","value":1}),
            json!({"cmd":"state","target":{"typo":1}}),
        ] {
            assert!(command(&value).is_err(), "{value}");
        }
    }
    #[test]
    fn typed_requests_keep_their_defaults_and_codes() {
        let operation = |value: Value| match command(&value).unwrap().remove(0) {
            Msg::Command(Command { operation, .. }) => operation,
            _ => panic!("{value}"),
        };
        assert!(matches!(
            operation(json!({"token":"t","cmd":"wait","until":"job","job_id":2})),
            Operation::Wait(Until::Job(2), wait) if wait == Duration::from_millis(1000)
        ));
        assert!(matches!(
            operation(json!({"cmd":"photos","limit":9000})),
            Operation::Photos {
                offset: 0,
                limit: 500,
                ..
            }
        ));
        assert!(matches!(
            operation(json!({"cmd":"preview","path":"/tmp/p.jpg"})),
            Operation::Output { max_edge: 1600, .. }
        ));
        assert!(matches!(
            operation(json!({"cmd":"export","path":"/tmp/p.jpg"})),
            Operation::Output {
                max_edge: 0,
                space: crate::color::OutputSpace::Srgb,
                ..
            }
        ));
        for (name, space) in [
            ("srgb", crate::color::OutputSpace::Srgb),
            ("adobe-rgb", crate::color::OutputSpace::AdobeRgb),
            ("prophoto", crate::color::OutputSpace::ProPhoto),
        ] {
            let op = operation(json!({"cmd":"export","path":"/tmp/p.tif","color_space":name}));
            assert!(
                matches!(op, Operation::Output { space: s, .. } if s == space),
                "{name}"
            );
        }
        // Offline photos' editable previews are kept unless asked for.
        assert!(matches!(
            operation(json!({"cmd":"discard_editable_previews"})),
            Operation::DiscardEditablePreviews {
                include_offline: false
            }
        ));
        assert!(matches!(
            operation(json!({"cmd":"discard_editable_previews","include_offline":true})),
            Operation::DiscardEditablePreviews {
                include_offline: true
            }
        ));
        for value in [
            json!({"cmd":"discard_editable_previews","include_offline":1}),
            json!({"cmd":"rename"}),
            json!({"cmd":"cc","cc":1,"value":1,"target":{}}),
            json!({"cmd":"turn","param":"band2","ticks":1}),
            json!({"cmd":"turn","param":"nonsense","ticks":1}),
            json!({"cmd":"open"}),
            json!({"cmd":"export","path":"/tmp/p.tif","color_space":"display-p3"}),
        ] {
            assert_eq!(
                command(&value).err().map(|e| e.code),
                Some("invalid_request"),
                "{value}"
            );
        }
    }
    #[test]
    fn timeout_cancels_queued_commands() {
        let (tx, rx) = mpsc::sync_channel(2);
        let result = handle(
            r#"{"token":"t","cmd":"set","param":"exposure","value":1}"#,
            "t",
            &tx,
            &egui::Context::default(),
            &Arc::default(),
            Duration::from_millis(5),
        );
        assert_eq!(result.unwrap_err().code, "cancelled");
        let Msg::Request(request) = rx.recv().unwrap() else {
            panic!()
        };
        assert!(!request.begin());
    }
    #[test]
    fn clients_receive_only_their_own_results() {
        let (tx, rx) = mpsc::sync_channel(4);
        let clients: Vec<_> = (1..=2)
            .map(|id| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    handle(
                        &json!({"token":"t","cmd":"open","id":id}).to_string(),
                        "t",
                        &tx,
                        &egui::Context::default(),
                        &Arc::default(),
                        Duration::from_secs(5),
                    )
                    .unwrap()
                })
            })
            .collect();
        for _ in 0..2 {
            let Msg::Request(request) = rx.recv().unwrap() else {
                panic!()
            };
            assert!(request.begin());
            let Msg::Command(Command {
                operation: Operation::Open(PhotoTarget::Id(id)),
                ..
            }) = &request.messages[0]
            else {
                panic!()
            };
            let ctx = egui::Context::default();
            let mut editor = super::super::Editor::with_context(
                &ctx,
                None,
                crate::app::preferences::session::Session::default(),
                None,
            );
            request
                .reply
                .send(Ok(commands::Reply {
                    state: editor.command_state(),
                    result: commands::Outcome::Search {
                        query: id.to_string(),
                    },
                    status: "applied",
                }))
                .unwrap();
        }
        for (index, client) in clients.into_iter().enumerate() {
            assert_eq!(
                client.join().unwrap()["result"]["query"],
                (index + 1).to_string()
            );
        }
    }
    #[test]
    fn real_tcp_reply_contains_the_executed_requests_identity() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::sync_channel(8);
        let ctx = egui::Context::default();
        let mut editor = crate::app::Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        editor.controls = super::super::Hub::new(super::super::Settings::default(), rx);
        let handle = start_at(tx, ctx.clone(), dir.path().join("control.json")).unwrap();
        let port = handle.port;
        let token = handle.token.clone();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            writeln!(
                stream,
                "{}",
                json!({"protocol":1,"request_id":"state-1","token":token,"cmd":"state"})
            )
            .unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !client.is_finished() {
            editor.control_commands(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let reply = client.join().unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["protocol"], 1);
        assert_eq!(reply["request_id"], "state-1");
        assert_eq!(
            reply["state"],
            serde_json::to_value(editor.command_state()).unwrap()
        );
        drop(handle);
    }
    #[test]
    fn disable_and_backpressure_prevent_execution() {
        let (request, _) = test_request(vec![]);
        request.stop.store(true, Ordering::SeqCst);
        assert!(!request.begin());
        let (tx, _rx) = mpsc::sync_channel(1);
        tx.try_send(Msg::Command(Command::new(Operation::State)))
            .unwrap();
        let result = handle(
            r#"{"token":"t","cmd":"state"}"#,
            "t",
            &tx,
            &egui::Context::default(),
            &Arc::default(),
            Duration::from_millis(5),
        );
        assert_eq!(result.unwrap_err().code, "busy");
    }

    #[test]
    fn token_disable_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.json");
        let (tx, rx) = mpsc::sync_channel(2);
        let handle = start_at(tx.clone(), egui::Context::default(), path.clone()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let denied = super::handle(
            r#"{"token":"wrong","cmd":"state"}"#,
            &handle.token,
            &tx,
            &egui::Context::default(),
            &handle.stop,
            Duration::from_millis(5),
        );
        assert_eq!(denied.unwrap_err().code, "unauthorized");
        assert!(rx.try_recv().is_err());
        let stop = handle.stop.clone();
        drop(handle);
        assert!(!path.exists());
        assert!(stop.load(Ordering::SeqCst));
    }

    #[test]
    fn a_stopped_listener_ends_after_the_connection_it_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::sync_channel(2);
        let mut handle = start_at(
            tx,
            egui::Context::default(),
            dir.path().join("control.json"),
        )
        .unwrap();
        // A client whose request waits in the queue for the interface's reply.
        let port = handle.port;
        let token = handle.token.clone();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            writeln!(stream, r#"{{"token":"{token}","cmd":"state"}}"#).unwrap();
            let mut reply = String::new();
            let _ = BufReader::new(stream).read_line(&mut reply);
            reply
        });
        let queued = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let stopping = handle.stop();
        // Closing the queue answers the request at once.
        drop(queued);
        drop(rx);
        let waited = crate::app::task::wait_for(vec![stopping], Duration::from_secs(10));
        assert_eq!(waited.detached, 0);
        let reply: Value = serde_json::from_str(&client.join().unwrap()).unwrap();
        assert_eq!(reply["ok"], false);
    }
}
