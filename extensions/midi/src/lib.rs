//! The bundled MIDI extension: control surfaces such as the Loupedeck+ as a
//! control adapter of RAWmakase, built with the extension SDK alone.
//!
//! Each enabled device has a thread of its own that finds its input, connects
//! and finds it again when it is plugged back in ([`listen`]). Its messages
//! are translated where they arrive, on the backend's thread: the device's
//! mapping turns a dial into a parameter's ticks or position and a button
//! into an action, its held modifiers merged in. The operations go into a
//! bounded queue; when it is full, input is dropped rather than held up. One
//! sender thread sends them to the app (`adapter.operate`), a batch at a
//! time, each with the epoch of the connection it came from.
//!
//! A connection's start and end go to the app at once from the device's own
//! thread (`adapter.device`), outside that queue, and the app applies them
//! ahead of anything queued: operations from before a disconnection are
//! refused however many are waiting, and a reconnection's new epoch starts
//! afresh. The held modifiers belong to the connection, so a new one starts
//! with none.
//!
//! What a device's operations do to the editor is the app's: adjustments
//! follow the active mask, the photo dial does nothing in the Library grid
//! and counts its detent there, absolute values span a parameter's range.
//!
//! The devices and their mappings are kept in `midi.json` in the extension's
//! data folder ([`config`]) and edited on its page in Preferences >
//! Extensions, a panel the app draws ([`page`]).
pub mod backend;
pub mod config;
pub mod midi;
mod page;
pub mod profiles;
mod spec;

pub use backend::Backend;
pub use page::PRESETS;

use config::{DeviceConfig, Mapping, Settings};
use midi::{Input, Last, Port};
use rawmakase_extension::protocol::method::{DeviceState, Level, Operation};
use rawmakase_extension::{ControlAdapter, Extension, Host, RemoteError};
use spec::Mods;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

/// The control adapter's id in the manifest.
pub const ADAPTER: &str = "midi";
/// The page's panel id in the manifest.
pub const PANEL: &str = "devices";
/// Operations waiting for the sender; more are dropped, as a device's input
/// always was when the editor fell behind.
const QUEUE: usize = 256;
/// The most operations sent in one `adapter.operate`.
const BATCH: usize = 64;
/// How often the sender wakes without input, to refresh an open page.
const TICK: Duration = Duration::from_millis(250);
/// A gap between two looks at a device far beyond the poll interval means the
/// computer slept; the device comes back as a new endpoint behind the same
/// name, so the connection starts again.
const SLEPT: Duration = Duration::from_secs(10);

/// The program: [`extension_with`] the system's MIDI inputs.
pub fn extension() -> Extension {
    extension_with(backend::system())
}

/// The program with `backend` as its MIDI inputs: a test's, which feeds bytes
/// in.
pub fn extension_with(backend: Arc<dyn Backend>) -> Extension {
    let midi = Arc::new(Midi::new(backend));
    Extension::new()
        .adapter(ADAPTER, Adapter(midi.clone()))
        .panel(PANEL, page::Page(midi))
}

/// What a device's page shows of its connection.
#[derive(Clone, Debug, Default)]
struct Status {
    /// The input's name while connected.
    connected: Option<String>,
    last: Option<Last>,
    problem: Option<String>,
}

/// A device's thread, running while its configuration stays as it was.
struct Listener {
    config: DeviceConfig,
    /// Dropping it ends the thread at its next look.
    _stop: Sender<()>,
}

/// What the page edits, and the threads its devices run.
#[derive(Default)]
struct State {
    settings: Settings,
    /// Why `midi.json` couldn't be read: nothing is edited or saved then.
    load_error: Option<String>,
    data_dir: PathBuf,
    listeners: HashMap<u64, Listener>,
    /// The page's own state.
    page: page::PageState,
}

/// An operation waiting for the sender.
struct Queued {
    device: u64,
    epoch: u64,
    op: Operation,
}

/// The program's state, shared by the run loop's handlers, the devices'
/// threads, their backends' callbacks and the sender.
struct Midi {
    backend: Arc<dyn Backend>,
    host: OnceLock<Host>,
    queue: OnceLock<SyncSender<Queued>>,
    /// The app's action names (`read.capabilities`), which tell a named
    /// action from a key of the same spelling.
    actions: RwLock<HashSet<String>>,
    state: Mutex<State>,
    statuses: Mutex<HashMap<u64, Status>>,
    /// Each device's last epoch. It only counts up while the program runs,
    /// across the device's threads, so no two connections share one.
    epochs: Mutex<HashMap<u64, u64>>,
    /// Inputs in use by a device, so two never listen to one.
    claims: Mutex<HashSet<String>>,
    /// Something the page shows changed since it was last sent.
    dirty: AtomicBool,
    stopped: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Midi {
    fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            host: OnceLock::new(),
            queue: OnceLock::new(),
            actions: RwLock::default(),
            state: Mutex::default(),
            statuses: Mutex::default(),
            epochs: Mutex::default(),
            claims: Mutex::default(),
            dirty: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        }
    }

    fn host(&self) -> Option<&Host> {
        self.host.get()
    }

    /// Whether `text` is one of the app's action names.
    fn named(&self, text: &str) -> bool {
        self.actions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(text)
    }

    fn status(&self, device: u64, change: impl FnOnce(&mut Status)) {
        let mut statuses = lock(&self.statuses);
        let status = statuses.entry(device).or_default();
        let before = (
            status.connected.clone(),
            status.last,
            status.problem.clone(),
        );
        change(status);
        if (
            status.connected.clone(),
            status.last,
            status.problem.clone(),
        ) != before
        {
            self.dirty.store(true, Ordering::SeqCst);
        }
    }

    /// Tells the app of a device's connection; it applies it ahead of any
    /// queued operation.
    fn report(&self, device: u64, state: DeviceState, epoch: u64) {
        if let Some(host) = self.host() {
            let _ = host.device(ADAPTER, &device.to_string(), state, epoch);
        }
    }

    /// Reads the settings and the app's actions, then starts the sender and
    /// the devices' threads.
    fn start(self: &Arc<Self>, host: Host) {
        let _ = self.host.set(host.clone());
        let dir = host.data_dir();
        let loaded = Settings::load(&dir);
        {
            let mut state = lock(&self.state);
            state.data_dir = dir;
            match loaded {
                Ok(settings) => state.settings = settings,
                Err(why) => state.load_error = Some(why),
            }
        }
        if let Some(why) = lock(&self.state).load_error.clone() {
            let _ = host.notify(
                Level::Warning,
                &format!("its settings weren't read and are left as they are: {why}"),
            );
        }
        if let Ok(capabilities) = host.read_capabilities() {
            let names = capabilities["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect();
            *self.actions.write().unwrap_or_else(|e| e.into_inner()) = names;
        }
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let _ = self.queue.set(tx);
        let midi = self.clone();
        let sender = std::thread::Builder::new()
            .name("midi sender".into())
            .spawn(move || midi.send(&rx));
        if let Err(e) = sender {
            let _ = host.notify(Level::Error, &format!("it didn't start: {e}"));
            return;
        }
        self.apply();
    }

    /// Ends the devices' threads and the sender, without waiting for them.
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        lock(&self.state).listeners.clear();
    }

    /// Makes the devices' threads the settings': a device whose
    /// configuration changed starts again (a new connection and epoch), one
    /// disabled stops, one removed is reported gone.
    fn apply(self: &Arc<Self>) {
        let mut removed = Vec::new();
        {
            let mut state = lock(&self.state);
            let wanted: Vec<DeviceConfig> = state
                .settings
                .devices
                .iter()
                .filter(|d| d.enabled)
                .cloned()
                .collect();
            let ids: HashSet<u64> = state.settings.devices.iter().map(|d| d.id).collect();
            state.listeners.retain(|id, listener| {
                if !ids.contains(id) {
                    removed.push(*id);
                }
                wanted.contains(&listener.config)
            });
            // Without MIDI input (Linux) a device's thread would find
            // nothing, ever.
            let listening = !self.stopped.load(Ordering::SeqCst) && self.backend.available();
            for config in wanted.into_iter().filter(|_| listening) {
                if state.listeners.contains_key(&config.id) {
                    continue;
                }
                let (stop, stopped) = mpsc::channel();
                let midi = self.clone();
                let thread_config = config.clone();
                let spawned = std::thread::Builder::new()
                    .name("midi".into())
                    .spawn(move || listen(&midi, &thread_config, &stopped));
                match spawned {
                    Ok(_) => {
                        state.listeners.insert(
                            config.id,
                            Listener {
                                config,
                                _stop: stop,
                            },
                        );
                    }
                    Err(e) => {
                        self.status(config.id, |s| s.problem = Some(e.to_string()));
                    }
                }
            }
        }
        for id in removed {
            let epoch = lock(&self.epochs).get(&id).copied().unwrap_or(0);
            self.report(id, DeviceState::Removed, epoch);
            lock(&self.statuses).remove(&id);
        }
    }

    /// The sender: sends queued operations to the app, consecutive ones of
    /// one connection together, and refreshes an open page, until stopped.
    fn send(&self, queue: &Receiver<Queued>) {
        while !self.stopped.load(Ordering::SeqCst) {
            let mut batch = Vec::new();
            match queue.recv_timeout(TICK) {
                Ok(first) => batch.push(first),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            while batch.len() < BATCH
                && let Ok(next) = queue.try_recv()
            {
                batch.push(next);
            }
            let mut batch = batch.into_iter().peekable();
            while let Some(first) = batch.next() {
                let (device, epoch) = (first.device, first.epoch);
                let mut ops = vec![first.op];
                while let Some(next) = batch.next_if(|q| q.device == device && q.epoch == epoch) {
                    ops.push(next.op);
                }
                if let Some(host) = self.host() {
                    // Refused when the connection ended meanwhile: nothing
                    // to do, the next connection sends its own.
                    let _ = host.operate(ADAPTER, &device.to_string(), epoch, ops);
                }
            }
            if self.dirty.swap(false, Ordering::SeqCst) {
                page::push(self);
            }
        }
    }
}

/// A connected input, with what ends it: dropping the connection stops its
/// messages, then the lease frees the input for another device.
struct Connected {
    port: Port,
    epoch: u64,
    _connection: Box<dyn Any>,
    _lease: Lease,
}

/// A device's claim on an input, released when dropped.
struct Lease {
    id: String,
    midi: Arc<Midi>,
}
impl Lease {
    fn claim(midi: &Arc<Midi>, id: &str) -> Option<Self> {
        lock(&midi.claims).insert(id.to_string()).then(|| Self {
            id: id.to_string(),
            midi: midi.clone(),
        })
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        lock(&self.midi.claims).remove(&self.id);
    }
}

/// A device's thread: finds its input among the system's, connects, and
/// finds it again when it is plugged back in, until its stop is dropped or
/// the program stops.
fn listen(midi: &Arc<Midi>, config: &DeviceConfig, stop: &Receiver<()>) {
    let id = config.id;
    let mut connected: Option<Connected> = None;
    let mut last = Instant::now();
    loop {
        if last.elapsed() > SLEPT
            && let Some(gone) = connected.take()
        {
            disconnect(midi, id, gone);
        }
        last = Instant::now();
        let ports = midi.backend.ports();
        match &connected {
            Some(c) if !ports.iter().any(|p| p == &c.port) => {
                if let Some(gone) = connected.take() {
                    disconnect(midi, id, gone);
                }
            }
            Some(_) => {}
            None => connected = connect(midi, config, &ports),
        }
        let name = connected.as_ref().map(|c| c.port.name.clone());
        midi.status(id, |s| s.connected = name);
        match stop.recv_timeout(midi.backend.poll_interval()) {
            Err(RecvTimeoutError::Timeout) if !midi.stopped.load(Ordering::SeqCst) => {}
            _ => break,
        }
    }
    if let Some(gone) = connected.take() {
        disconnect(midi, id, gone);
    }
    midi.status(id, |s| s.connected = None);
}

/// Ends a connection: its messages stop first, then the app hears of it.
fn disconnect(midi: &Midi, device: u64, connected: Connected) {
    let epoch = connected.epoch;
    drop(connected);
    midi.report(device, DeviceState::Disconnected, epoch);
}

/// Connects `config`'s input, if it is among `ports` and free: a new epoch,
/// told to the app before the first message can arrive.
fn connect(midi: &Arc<Midi>, config: &DeviceConfig, ports: &[Port]) -> Option<Connected> {
    let id = config.id;
    let selected = midi::select_port(config, ports);
    midi.status(id, |s| s.problem = selected.as_ref().err().cloned());
    let Ok(Some(port_id)) = &selected else {
        return None;
    };
    let port = ports.iter().find(|p| &p.id == port_id)?;
    let Some(lease) = Lease::claim(midi, &port.id) else {
        midi.status(id, |s| {
            s.problem = Some("This input is already assigned to another enabled device".into());
        });
        return None;
    };
    let epoch = {
        let mut epochs = lock(&midi.epochs);
        let epoch = epochs.entry(id).or_default();
        *epoch += 1;
        *epoch
    };
    midi.report(id, DeviceState::Connected, epoch);
    let receive = {
        let midi = midi.clone();
        let mapping = config.mapping.clone();
        let channel = config.channel;
        let mut held = Mods::default();
        move |bytes: &[u8]| {
            let Some(input) = midi::parse(bytes, channel) else {
                return;
            };
            midi.status(id, |s| match input {
                Input::Cc(cc, value) => s.last = Some(Last::Dial(cc, value)),
                Input::Note(note, true) => s.last = Some(Last::Button(note)),
                Input::Note(_, false) => {}
            });
            match translate(&mapping, &mut held, input, &|t| midi.named(t)) {
                Ok(Some(op)) => {
                    if let Some(queue) = midi.queue.get() {
                        let _ = queue.try_send(Queued {
                            device: id,
                            epoch,
                            op,
                        });
                    }
                }
                Ok(None) => {}
                Err(why) => midi.status(id, |s| s.problem = Some(why)),
            }
        }
    };
    match midi.backend.connect(port, Box::new(receive)) {
        Ok(connection) => Some(Connected {
            port: port.clone(),
            epoch,
            _connection: connection,
            _lease: lease,
        }),
        Err(why) => {
            midi.status(id, |s| s.problem = Some(why));
            midi.report(id, DeviceState::Disconnected, epoch);
            None
        }
    }
}

/// The operation `input` stands for in `mapping`, with the modifiers `held`
/// on the same connection; none for a control the mapping doesn't bind. The
/// photo dial sends raw ticks with its detent: the app counts them, and drops
/// them where the dial does nothing.
fn translate(
    mapping: &Mapping,
    held: &mut Mods,
    input: Input,
    named: &dyn Fn(&str) -> bool,
) -> Result<Option<Operation>, String> {
    Ok(match input {
        Input::Cc(cc, value) if mapping.photo_dial == Some(cc) => {
            let ticks = mapping
                .encoder(cc)
                .ticks(value)
                .ok_or_else(|| "Photo navigation requires a relative control format".to_string())?;
            Some(Operation::Photo {
                ticks,
                detent: mapping.photo_detent.max(1).unsigned_abs(),
            })
        }
        Input::Cc(cc, value) => {
            mapping
                .dials
                .get(&cc)
                .map(|param| match mapping.encoder(cc).ticks(value) {
                    Some(ticks) => Operation::Adjust {
                        param: param.clone(),
                        ticks: ticks * mapping.sensitivity,
                    },
                    None => Operation::Value {
                        param: param.clone(),
                        value: value.min(127),
                    },
                })
        }
        Input::Note(note, down) => mapping
            .buttons
            .get(&note)
            .and_then(|spec| spec::press(held, spec, down, named))
            .map(|(spec, advance)| Operation::Action { spec, advance }),
    })
}

/// The adapter's side of the run loop.
struct Adapter(Arc<Midi>);
impl ControlAdapter for Adapter {
    fn start(&mut self, host: Host) -> Result<(), RemoteError> {
        self.0.start(host);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.stop();
    }
}

#[cfg(test)]
mod tests;
