//! MIDI discovery and connections. No editor or mapping policy lives here.
use super::*;
#[derive(Clone, Debug)]
pub(super) struct Port {
    pub id: String,
    pub name: String,
}
pub(super) fn ports() -> Vec<Port> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if let Ok(input) = midir::MidiInput::new("RAWmakase devices") {
        return input
            .ports()
            .iter()
            .map(|p| Port {
                id: p.id(),
                name: input.port_name(p).unwrap_or_default(),
            })
            .collect();
    }
    Vec::new()
}
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub(super) fn select_port(
    binding: &DeviceConfig,
    available: &[Port],
) -> Result<Option<String>, String> {
    if let Some(id) = &binding.port_id {
        return Ok(available.iter().find(|p| &p.id == id).map(|p| p.id.clone()));
    }
    if binding.port.trim().is_empty() {
        return Ok(None);
    }
    let candidates: Vec<_> = available
        .iter()
        .filter(|p| {
            if binding.exact {
                p.name == binding.port
            } else {
                p.name.contains(&binding.port)
            }
        })
        .collect();
    match candidates.as_slice() {
        [] => Ok(None),
        [port] => Ok(Some(port.id.clone())),
        _ => Err("Multiple ports match; choose a specific MIDI input".into()),
    }
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
struct Lease {
    id: String,
    claims: Arc<Mutex<std::collections::HashSet<String>>>,
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Lease {
    fn claim(id: String, claims: Arc<Mutex<std::collections::HashSet<String>>>) -> Option<Self> {
        if !claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone())
        {
            return None;
        }
        Some(Self { id, claims })
    }
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for Lease {
    fn drop(&mut self) {
        self.claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub(super) fn parse(bytes: &[u8], channel: Option<u8>) -> Option<Msg> {
    if channel.is_some_and(|c| bytes.first().is_none_or(|s| (s & 15) + 1 != c)) {
        return None;
    }
    match *bytes {
        [status, d1, d2] if status & 0xF0 == 0xB0 => Some(Msg::Cc(d1, d2)),
        [status, d1, d2] if status & 0xF0 == 0x90 && d2 > 0 => Some(Msg::Note(d1, true)),
        [status, d1, _] if status & 0xF0 == 0x80 => Some(Msg::Note(d1, false)),
        [status, d1, 0] if status & 0xF0 == 0x90 => Some(Msg::Note(d1, false)),
        _ => None,
    }
}

/// The latest message from the device, for Preferences.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Last {
    Dial(u8, i32),
    Button(u8),
}

/// What the listener tells Preferences.
#[derive(Default)]
pub(super) struct Status {
    /// The MIDI port while it is connected.
    pub connected: Option<String>,
    pub epoch: u64,
    pub last: Option<Last>,
    pub problem: Option<String>,
}

pub(super) fn locked(status: &Mutex<Status>) -> std::sync::MutexGuard<'_, Status> {
    status.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Changes the connection or problem Preferences shows, waking the interface
/// only when it changed, so Preferences need not poll.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn update(status: &Mutex<Status>, ctx: &egui::Context, change: impl FnOnce(&mut Status)) {
    let mut status = locked(status);
    let before = (status.connected.clone(), status.problem.clone());
    change(&mut status);
    if (status.connected.clone(), status.problem.clone()) != before {
        ctx.request_repaint();
    }
}

/// The epoch is shared outside the bounded input queue, so disconnects cannot
/// lose their reset signal and queued input from old connections is discarded.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
pub(super) fn reset_connection(status: &Mutex<Status>, ctx: &egui::Context) -> u64 {
    let mut status = locked(status);
    status.epoch = status.epoch.wrapping_add(1);
    ctx.request_repaint();
    status.epoch
}

/// Listens for the device on a thread of its own, finding it again when it is
/// plugged back in, until `stop`'s sender is dropped. Where there is no MIDI backend nothing
/// is ever sent, and no thread starts.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn listen(
    binding: DeviceConfig,
    claims: Arc<Mutex<std::collections::HashSet<String>>>,
    tx: Sender<Msg>,
    ctx: egui::Context,
    status: Arc<Mutex<Status>>,
    stop: std::sync::mpsc::Receiver<()>,
) -> Option<std::thread::JoinHandle<()>> {
    use midir::{MidiInput, MidiInputConnection};
    use std::sync::mpsc::RecvTimeoutError;
    let spawned = std::thread::Builder::new()
        .name("midi".into())
        .spawn(move || {
            let mut connected: Option<(String, String, MidiInputConnection<()>, Lease)> = None;
            let mut last = Instant::now();
            loop {
                // A gap far beyond the poll interval means the Mac slept; the
                // device comes back as a new endpoint behind the same name.
                if last.elapsed() > Duration::from_secs(10) {
                    connected = None;
                    reset_connection(&status, &ctx);
                }
                last = Instant::now();
                if let Ok(input) = MidiInput::new("RAWmakase") {
                    let ports = input.ports();
                    let named = |p: &midir::MidiInputPort| input.port_name(p).unwrap_or_default();
                    match &connected {
                        Some((name, id, ..))
                            if !ports.iter().any(|p| &named(p) == name && &p.id() == id) =>
                        {
                            connected = None;
                            reset_connection(&status, &ctx);
                        }
                        Some(_) => {}
                        None => {
                            let available: Vec<_> = ports
                                .iter()
                                .map(|p| Port {
                                    id: p.id(),
                                    name: named(p),
                                })
                                .collect();
                            let selected = select_port(&binding, &available);
                            let problem = selected.as_ref().err().cloned();
                            update(&status, &ctx, |s| s.problem = problem);
                            if let Ok(Some(id)) = selected
                                && let Some(p) = ports.iter().find(|p| p.id() == id)
                                && let Some(lease) = Lease::claim(id, claims.clone()).or_else(|| { update(&status, &ctx, |s| s.problem = Some("This input is already assigned to another enabled device".into())); None })
                            {
                                let epoch = reset_connection(&status, &ctx);
                                let (name, id) = (named(p), p.id());
                                let (tx, ctx, status) = (tx.clone(), ctx.clone(), status.clone());
                                connected = input
                                    .connect(
                                        p,
                                        "RAWmakase input",
                                        move |_, bytes, _| {
                                            if let Some(msg) = parse(bytes, binding.channel) {
                                                match msg {
                                                    Msg::Cc(cc, v) => {
                                                        locked(&status).last =
                                                            Some(Last::Dial(cc, i32::from(v)));
                                                    }
                                                    Msg::Note(n, true) => {
                                                        locked(&status).last =
                                                            Some(Last::Button(n));
                                                    }
                                                    _ => {}
                                                }
                                                let _ = tx.try_send(Msg::Midi(
                                                    status.clone(),
                                                    epoch,
                                                    Box::new(msg),
                                                ));
                                                ctx.request_repaint();
                                            }
                                        },
                                        (),
                                    )
                                    .ok()
                                    .map(|c| (name, id, c, lease));
                            }
                        }
                    }
                }
                let name = connected.as_ref().map(|(name, ..)| name.clone());
                update(&status, &ctx, |s| s.connected = name);
                // Looked at again in two seconds, as no portable event announces a
                // device plugged in; a stop ends the wait at once.
                match stop.recv_timeout(Duration::from_secs(2)) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            update(&status, &ctx, |s| s.connected = None);
        });
    spawned.map_err(|e| eprintln!("MIDI listener: {e}")).ok()
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn listen(
    _: DeviceConfig,
    _: Arc<Mutex<std::collections::HashSet<String>>>,
    _: Sender<Msg>,
    _: egui::Context,
    _: Arc<Mutex<Status>>,
    _: std::sync::mpsc::Receiver<()>,
) -> Option<std::thread::JoinHandle<()>> {
    None
}
