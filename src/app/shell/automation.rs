//! Transport-neutral automation owner. MIDI devices and local clients feed the
//! same bounded application-command queue; device mappings stay in their adapters.
mod config;
mod device;
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
    Cc(u8, u8),
    Note(u8, bool),
    Command(Command),
    Request(socket::Request),
    #[cfg(any(test, target_os = "macos", target_os = "windows"))]
    Midi(Arc<Mutex<Status>>, u64, Box<Msg>),
}

pub(in crate::app) struct Hub {
    rx: Receiver<Msg>,
    settings: Settings,
    devices: Vec<Device>,
    // Raw CC/note requests retain the legacy mapping without borrowing any
    // physical device's modifiers, settings, or connection.
    legacy: Device,
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
    reply: mpsc::SyncSender<commands::Result<commands::Reply>>,
    deadline: Instant,
}
impl Hub {
    pub(crate) fn inactive() -> Self {
        let (_, rx) = mpsc::sync_channel(256);
        Self::new(Settings::default(), rx)
    }
    fn new(settings: Settings, rx: Receiver<Msg>) -> Self {
        let devices = settings.devices.iter().cloned().map(Device::new).collect();
        Self {
            rx,
            devices,
            settings,
            legacy: Device::new(DeviceConfig::new(0, Profile::Loupedeck)),
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
                    if let [
                        Msg::Command(Command {
                            operation: commands::Operation::Wait(until, time),
                            ..
                        }),
                    ] = request.messages[..]
                    {
                        self.controls.waits.push(Waiting {
                            until,
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
            #[cfg(not(any(test, target_os = "macos", target_os = "windows")))]
            let (source, device_index) = (commands::Source::Socket, None::<usize>);
            #[cfg(any(test, target_os = "macos", target_os = "windows"))]
            let (msg, source, device_index) = match msg {
                Msg::Midi(status, epoch, msg) => {
                    let Some(index) = self.controls.devices.iter().position(|d| {
                        d.binding.enabled
                            && Arc::ptr_eq(&status, &d.status)
                            && epoch == d.midi_epoch
                    }) else {
                        continue;
                    };
                    (
                        *msg,
                        commands::Source::Midi(self.controls.devices[index].identity, epoch),
                        Some(index),
                    )
                }
                msg => (msg, commands::Source::Socket, None),
            };
            let controller = match device_index {
                Some(index) => &mut self.controls.devices[index],
                None => &mut self.controls.legacy,
            };
            if matches!(msg, Msg::Cc(cc, _) if controller.binding.mapping.photo_dial == Some(cc))
                && self.module == Module::Library
                && !self.library.as_ref().is_some_and(|l| l.loupe_open())
            {
                continue;
            }
            if device_index.is_some()
                && match &msg {
                    Msg::Cc(cc, _) => {
                        !controller.binding.mapping.dials.contains_key(cc)
                            && controller.binding.mapping.photo_dial != Some(*cc)
                    }
                    Msg::Note(note, _) => !controller.binding.mapping.buttons.contains_key(note),
                    _ => false,
                }
            {
                continue;
            }
            let device = matches!(msg, Msg::Cc(..) | Msg::Note(..));
            if let Some(mut command) = controller.translate(msg)? {
                // Device adjustments follow the active mask; scripts use explicit
                // scope. Unsupported mask parameters fail rather than edit globally.
                if device
                    && matches!(
                        command.operation,
                        commands::Operation::Adjust(..) | commands::Operation::ControlValue(..)
                    )
                    && self.view.is(crate::app::develop::state::Tool::Mask)
                {
                    let index =
                        self.view.masking.selected.ok_or_else(|| {
                            commands::Error::new("no_mask", "Select a mask first")
                        })?;
                    command.target = commands::Target {
                        mask: Some(index),
                        generation: Some(self.document.load_id()),
                        revision: Some(self.automation.revision),
                        ..Default::default()
                    };
                }
                if device
                    && let commands::Operation::Metadata { advance, .. } = &mut command.operation
                {
                    *advance |= self.session.auto_advance;
                }
                result = self.execute_command_from(command, source, ctx)?;
            }
        }
        Ok(result)
    }
}

/// The command a request's JSON is read as, as the control socket reads it, for
/// tests of what automation asks for.
#[cfg(test)]
pub(in crate::app) fn command_of(request: serde_json::Value) -> Command {
    match socket::command(&request)
        .unwrap_or_else(|e| panic!("{request}: {}", e.message))
        .remove(0)
    {
        Msg::Command(command) => command,
        _ => panic!("{request} is not a command"),
    }
}

#[cfg(test)]
mod tests;
