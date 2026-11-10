//! Transport-neutral automation owner. MIDI devices and local clients feed the
//! same bounded application-command queue; device mappings stay in their adapters.
//!
//! A device's adapter translates its controls into named [`DeviceOp`]s, and
//! [`Editor::device_operate`] applies them with the semantics every control
//! surface shares: adjustments follow the active mask, the photo dial does
//! nothing in the Library grid, metadata actions follow Auto Advance and
//! absolute values span the parameter's range.
mod config;
mod device;
mod legacy;
mod mapping;
mod midi;
mod profiles;
mod settings;
mod socket;
use crate::app::Editor;
use crate::app::Module;
use crate::app::shell::commands::{self, Command, Param};
use config::{Config, DeviceConfig, Encoder, Settings};
use device::Device;
use mapping::*;
use midi::{Last, Status, locked};
use profiles::Profile;

use eframe::egui::{self, Key, Modifiers};
use legacy::Legacy;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, Receiver, SyncSender as Sender},
    },
    time::{Duration, Instant},
};

const PHOTO_IDLE: Duration = Duration::from_millis(600);
/// Raw device input is translated here, never in the application command layer.
enum Msg {
    /// The control socket's legacy `cc` and `note` commands.
    Legacy(Input),
    Command(Command),
    Request(socket::Request),
    #[cfg(any(test, target_os = "macos", target_os = "windows"))]
    Midi(Arc<Mutex<Status>>, u64, Input),
}
/// A control change or a note, as a MIDI device or a legacy socket command sends it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Input {
    Cc(u8, u8),
    Note(u8, bool),
}
/// What a device's control does, once its adapter has translated it.
#[derive(Clone, Debug, PartialEq)]
enum DeviceOp {
    /// Turns a parameter by ticks, already scaled by the device's turn speed.
    Adjust(Param, i32),
    /// Sets a parameter from an absolute control's 0–127 position.
    Value(Param, u8),
    /// The photo dial turned by relative ticks; `detent` ticks move one photo.
    Photo { ticks: i32, detent: i32 },
    /// An action in [`parse_action`]'s notation, its held modifiers merged in.
    /// `advance` moves to the next photo after a metadata action.
    Action { spec: String, advance: bool },
}
/// The photo dial's detent state for one device source. A new source (another
/// device, or a new connection epoch) starts from a whole detent.
struct PhotoDial {
    source: commands::Source,
    ticks: i32,
    dir: i32,
    last: Instant,
}

pub(in crate::app) struct Hub {
    rx: Receiver<Msg>,
    settings: Settings,
    /// This hub's identity in its devices' gesture sources, so a device of
    /// another hub never continues one of this hub's gestures.
    #[cfg(any(test, target_os = "macos", target_os = "windows"))]
    session: u64,
    devices: Vec<Device>,
    // Raw CC/note requests retain the legacy mapping without borrowing any
    // physical device's modifiers, settings, or connection.
    legacy: Legacy,
    photo_dials: Vec<PhotoDial>,
    link: Option<(Sender<Msg>, egui::Context)>,
    socket: Option<socket::Handle>,
    claims: Arc<Mutex<std::collections::HashSet<String>>>,
    selected: Option<u64>,
    load_error: Option<String>,
    ports: Vec<midi::Port>,
    ports_scanned: bool,
    /// Control requests waiting for a photo to load or an output to finish.
    waits: Vec<Waiting>,
}
/// A `wait` request, answered once its condition holds or its time is up.
struct Waiting {
    until: commands::Until,
    /// The control adapter that sent it, checked again when it is answered.
    adapter: Option<String>,
    reply: mpsc::SyncSender<commands::Result<commands::Reply>>,
    deadline: Instant,
}
impl Hub {
    pub(crate) fn inactive() -> Self {
        let (_, rx) = mpsc::sync_channel(256);
        Self::new(Settings::default(), rx)
    }
    fn new(settings: Settings, rx: Receiver<Msg>) -> Self {
        #[cfg(any(test, target_os = "macos", target_os = "windows"))]
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let devices = settings.devices.iter().cloned().map(Device::new).collect();
        Self {
            rx,
            #[cfg(any(test, target_os = "macos", target_os = "windows"))]
            session: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            devices,
            settings,
            legacy: Legacy::default(),
            photo_dials: Vec::new(),
            link: None,
            socket: None,
            claims: Arc::default(),
            selected: None,
            load_error: None,
            ports: Vec::new(),
            ports_scanned: false,
            waits: Vec::new(),
        }
    }
    pub(crate) fn start(ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::sync_channel(256);
        let (settings, error) = match Settings::load() {
            Ok(s) => (s, None),
            Err(e) => (
                Settings::default(),
                Some(format!("Could not read automation settings: {e:#}")),
            ),
        };
        let mut hub = Self::new(settings, rx);
        hub.load_error = error;
        for device in &mut hub.devices {
            device.start(tx.clone(), ctx.clone(), hub.claims.clone());
        }
        hub.link = Some((tx, ctx.clone()));
        hub.set_socket(hub.settings.socket);
        hub
    }
    /// Drops the control request queue at exit: requests in it, or waiting for a
    /// photo or an output, and any sent later are answered at once, so no connection
    /// waits for the interface.
    pub(in crate::app) fn close_requests(&mut self) {
        self.rx = mpsc::sync_channel(0).1;
        self.waits.clear();
        self.link = None;
    }
    /// Stops the MIDI listeners and the control socket, to wait for at exit.
    pub(in crate::app) fn stop(&mut self) -> Vec<crate::app::task::Stopping> {
        let mut stopping: Vec<_> = self.devices.iter_mut().map(Device::stop).collect();
        stopping.extend(self.socket.as_mut().map(socket::Handle::stop));
        stopping
    }
    fn set_socket(&mut self, on: bool) {
        self.socket = None;
        if on && let Some((tx, ctx)) = &self.link {
            self.socket = socket::start(tx.clone(), ctx.clone());
        }
    }
    fn apply(&mut self, settings: Settings) {
        let socket_changed = self.socket.is_some() != settings.socket;
        let mut previous = std::mem::take(&mut self.devices);
        for binding in &settings.devices {
            let device = if let Some(index) = previous.iter().position(|d| &d.binding == binding) {
                previous.remove(index)
            } else {
                // Drop the old listener before replacing its binding. Stale messages
                // carry its status identity and cannot affect the replacement.
                if let Some(index) = previous.iter().position(|d| d.binding.id == binding.id) {
                    previous.remove(index);
                }
                let mut device = Device::new(binding.clone());
                if let Some((tx, ctx)) = &self.link {
                    device.start(tx.clone(), ctx.clone(), self.claims.clone());
                }
                device
            };
            self.devices.push(device);
        }
        self.settings = settings;
        if socket_changed {
            self.set_socket(self.settings.socket);
        }
    }
    /// Whether operations from `source` may run: a device's only while it is
    /// enabled and `source`'s epoch is its connection's current one. The epoch
    /// is read from the listener's status, outside the bounded queue, so input
    /// queued before a disconnect never runs.
    fn current(&self, source: commands::Source) -> bool {
        match source {
            commands::Source::Socket => true,
            #[cfg(any(test, target_os = "macos", target_os = "windows"))]
            commands::Source::Device {
                session,
                device,
                epoch,
            } => {
                session == self.session
                    && self.devices.iter().any(|d| {
                        d.identity == device
                            && d.binding.enabled
                            && locked(&d.status).epoch == epoch
                    })
            }
        }
    }
    /// The photos `ticks` of the photo dial move by, at most one either way:
    /// one per `detent` ticks in a row, the first tick of a fresh turn or a
    /// change of direction moving at once.
    fn photo_turn(&mut self, source: commands::Source, ticks: i32, detent: i32) -> i32 {
        if ticks == 0 {
            return 0;
        }
        let detent = detent.max(1);
        // Sources no longer current (old epochs, removed devices) are dropped.
        let mut dials = std::mem::take(&mut self.photo_dials);
        dials.retain(|d| self.current(d.source));
        self.photo_dials = dials;
        let index = match self.photo_dials.iter().position(|d| d.source == source) {
            Some(index) => index,
            None => {
                self.photo_dials.push(PhotoDial {
                    source,
                    ticks: 0,
                    dir: 0,
                    last: Instant::now(),
                });
                self.photo_dials.len() - 1
            }
        };
        let dial = &mut self.photo_dials[index];
        let fresh = dial.dir == 0 || dial.last.elapsed() > PHOTO_IDLE;
        if fresh || ticks.signum() != dial.dir {
            dial.ticks = ticks.signum() * detent;
        } else {
            dial.ticks += ticks;
        }
        dial.dir = ticks.signum();
        dial.last = Instant::now();
        let step = dial.ticks / detent;
        dial.ticks %= detent;
        step.signum()
    }
}
/// The command an action in [`parse_action`]'s notation names. A metadata
/// action moves on afterward when `advance` is set.
fn action_operation(spec: &str, advance: bool) -> commands::Result<commands::Operation> {
    let action = parse_action(spec).and_then(resolve).ok_or_else(|| {
        commands::Error::new(
            "unsupported_action",
            "This shortcut has no application action",
        )
    })?;
    Ok(match action.metadata() {
        Some(edit) => commands::Operation::Metadata { edit, advance },
        None => commands::Operation::Action(action),
    })
}
impl Editor {
    pub(in crate::app) fn control_commands(&mut self, ctx: &egui::Context) {
        for device in &mut self.controls.devices {
            device.sync_midi_epoch();
        }
        let messages: Vec<_> = self.controls.rx.try_iter().take(128).collect();
        let full = messages.len() == 128;
        for message in messages {
            match message {
                Msg::Request(request) => {
                    if !request.begin() {
                        continue;
                    }
                    if let Some(refused) = self.adapter_refused(request.adapter.as_deref()) {
                        let _ = request.reply.send(Err(refused));
                        continue;
                    }
                    if let [
                        Msg::Command(Command {
                            operation: commands::Operation::Wait(until, time),
                            ..
                        }),
                    ] = request.messages[..]
                    {
                        self.controls.waits.push(Waiting {
                            until,
                            adapter: request.adapter,
                            reply: request.reply,
                            deadline: Instant::now() + time,
                        });
                        continue;
                    }
                    let result = self.control_messages(request.messages, ctx);
                    let state = self.command_state();
                    let _ = request.reply.send(result.map(|result| commands::Reply {
                        state,
                        result,
                        status: "applied",
                    }));
                }
                message => {
                    if let Err(error) = self.control_messages(vec![message], ctx) {
                        self.messages
                            .warn(format!("Control surface: {}", error.message));
                    }
                }
            }
        }
        if full {
            ctx.request_repaint();
        }
        self.answer_waits(ctx);
    }
    /// Answers each waiting request whose condition now holds, or whose time is up
    /// ("timed_out", with the state as it is). Loading a photo and finishing an
    /// output both wake the interface, so a wait is answered in the frame it holds.
    fn answer_waits(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let mut waiting = Vec::new();
        for wait in std::mem::take(&mut self.controls.waits) {
            // Its adapter disabled while it waited: refused now, not when it holds.
            if let Some(refused) = self.adapter_refused(wait.adapter.as_deref()) {
                let _ = wait.reply.send(Err(refused));
                continue;
            }
            let holds = self.holds(wait.until);
            if holds || now >= wait.deadline {
                let state = self.command_state();
                let _ = wait.reply.send(Ok(commands::Reply {
                    state,
                    result: commands::Outcome::Empty,
                    status: if holds { "applied" } else { "timed_out" },
                }));
            } else {
                waiting.push(wait);
            }
        }
        if let Some(next) = waiting.iter().map(|w| w.deadline).min() {
            ctx.request_repaint_after(next.saturating_duration_since(now));
        }
        self.controls.waits = waiting;
    }
    /// Why a control request from `adapter` may not run: the adapter it names
    /// isn't an enabled control adapter of an enabled extension (D8). A
    /// request naming no adapter always may.
    fn adapter_refused(&self, adapter: Option<&str>) -> Option<commands::Error> {
        let adapter = adapter?;
        (!self.extensions.adapter_enabled(adapter)).then(|| {
            commands::Error::new(
                "adapter_disabled",
                format!(
                    "{adapter} is not an enabled control adapter; enable its extension in Preferences > Extensions"
                ),
            )
        })
    }
    fn holds(&self, until: commands::Until) -> bool {
        match until {
            commands::Until::Loaded(id) => {
                let develop = self.module == Module::Develop && self.document.metadata.is_some();
                let photo = develop.then_some(self.document.catalog_photo).flatten();
                let loaded = develop && self.document.full().is_some() && !self.document.loading();
                // So does its opening failing: it won't load, and the state
                // says it isn't loaded.
                let failed = self.open_failed() == Some(id);
                // Another photo replacing it ends the wait too: the client reads
                // the state and sees it is stale.
                photo.is_some_and(|p| p != id) || (loaded && photo == Some(id)) || failed
            }
            commands::Until::Job(id) => self.automation.job_finished(id),
        }
    }
    fn control_messages(
        &mut self,
        messages: Vec<Msg>,
        ctx: &egui::Context,
    ) -> commands::Result<commands::Outcome> {
        let mut result = commands::Outcome::Empty;
        for msg in messages {
            self.sync_command_revision();
            for device in &mut self.controls.devices {
                device.sync_midi_epoch();
            }
            let (source, op) = match msg {
                Msg::Command(command) => {
                    result = self.execute_command_from(command, commands::Source::Socket, ctx)?;
                    continue;
                }
                Msg::Request(_) => {
                    return Err(commands::Error::new("invalid_request", "Nested request"));
                }
                Msg::Legacy(input) => (
                    commands::Source::Socket,
                    self.controls.legacy.translate(input)?,
                ),
                #[cfg(any(test, target_os = "macos", target_os = "windows"))]
                Msg::Midi(status, epoch, input) => {
                    let ignored = self.photo_dial_ignored();
                    let Some(device) = self.controls.devices.iter_mut().find(|d| {
                        d.binding.enabled
                            && Arc::ptr_eq(&status, &d.status)
                            && epoch == d.midi_epoch
                    }) else {
                        continue;
                    };
                    let source = commands::Source::Device {
                        session: self.controls.session,
                        device: device.identity,
                        epoch,
                    };
                    // An absolute photo dial is refused only where the dial acts;
                    // in the Library grid it is ignored, as any photo dial input is.
                    if ignored && device.photo_dial(input) {
                        continue;
                    }
                    (source, device.translate(input)?)
                }
            };
            if let Some(op) = op {
                result = self.device_operate(source, op, ctx)?;
            }
        }
        Ok(result)
    }
    /// Whether the photo dial does nothing now: in the Library's grid.
    fn photo_dial_ignored(&self) -> bool {
        self.module == Module::Library && !self.library.as_ref().is_some_and(|l| l.loupe_open())
    }
    /// Runs one operation a control device's adapter produced, with device
    /// semantics, as `source`'s gesture. An operation from a device that is
    /// disabled, removed or reconnected since is dropped.
    fn device_operate(
        &mut self,
        source: commands::Source,
        op: DeviceOp,
        ctx: &egui::Context,
    ) -> commands::Result<commands::Outcome> {
        use commands::Operation as O;
        if !self.controls.current(source) {
            return Ok(commands::Outcome::Empty);
        }
        let mut command = Command::new(match op {
            DeviceOp::Adjust(param, ticks) => O::Adjust(param, ticks),
            DeviceOp::Value(param, value) => O::ControlValue(param, value),
            DeviceOp::Photo { ticks, detent } => {
                // Ignored before it counts toward a detent, so the first tick
                // in the Loupe after turning in the grid still moves.
                if self.photo_dial_ignored() {
                    return Ok(commands::Outcome::Empty);
                }
                match self.controls.photo_turn(source, ticks, detent) {
                    0 => return Ok(commands::Outcome::Empty),
                    step => O::DeviceNavigate(step),
                }
            }
            DeviceOp::Action { spec, advance } => {
                action_operation(&spec, advance || self.session.auto_advance)?
            }
        });
        // Device adjustments follow the active mask; scripts use explicit
        // scope. Unsupported mask parameters fail rather than edit globally.
        if matches!(command.operation, O::Adjust(..) | O::ControlValue(..))
            && self.view.is(crate::app::develop::state::Tool::Mask)
        {
            let index = self
                .view
                .masking
                .selected
                .ok_or_else(|| commands::Error::new("no_mask", "Select a mask first"))?;
            command.target = commands::Target {
                mask: Some(index),
                generation: Some(self.document.load_id()),
                revision: Some(self.automation.revision),
                ..Default::default()
            };
        }
        self.execute_command_from(command, source, ctx)
    }
}

#[cfg(test)]
mod tests;
