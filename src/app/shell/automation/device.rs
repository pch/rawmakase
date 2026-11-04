//! Independent mapping and held-control state for one MIDI device.
use super::*;
pub(super) struct Device {
    pub binding: DeviceConfig,
    #[cfg(any(test, target_os = "macos", target_os = "windows"))]
    pub identity: u64,
    pub held: Modifiers,
    photo_ticks: i32,
    photo_dir: i32,
    last_photo: Option<Instant>,
    pub midi_epoch: u64,
    pub status: Arc<Mutex<Status>>,
    /// The listener's stop: dropping it (with the device, or by starting again)
    /// ends the listener at once.
    stop: Option<std::sync::mpsc::Sender<()>>,
    listener: Option<std::thread::JoinHandle<()>>,
}
impl Device {
    pub(crate) fn new(binding: DeviceConfig) -> Self {
        #[cfg(any(test, target_os = "macos", target_os = "windows"))]
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            #[cfg(any(test, target_os = "macos", target_os = "windows"))]
            identity: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            binding,
            held: Modifiers::NONE,
            photo_ticks: 0,
            photo_dir: 0,
            last_photo: None,
            midi_epoch: 0,
            status: Arc::default(),
            stop: None,
            listener: None,
        }
    }
    pub(crate) fn start(
        &mut self,
        tx: Sender<Msg>,
        ctx: egui::Context,
        claims: Arc<Mutex<std::collections::HashSet<String>>>,
    ) {
        if self.binding.enabled {
            let (stop, stopped) = std::sync::mpsc::channel();
            self.stop = Some(stop);
            self.listener = midi::listen(
                self.binding.clone(),
                claims,
                tx,
                ctx,
                self.status.clone(),
                stopped,
            );
        }
    }
    /// Ends the listener at once, to wait for at exit.
    pub(in crate::app) fn stop(&mut self) -> crate::app::task::Stopping {
        self.stop = None;
        crate::app::task::Stopping::new(self.listener.take())
    }
    pub(crate) fn sync_midi_epoch(&mut self) {
        let epoch = locked(&self.status).epoch;
        if epoch != self.midi_epoch {
            self.midi_epoch = epoch;
            self.held = Modifiers::NONE;
            self.photo_ticks = 0;
            self.photo_dir = 0;
            self.last_photo = None;
        }
    }
    fn photo_turn(&mut self, t: i32) -> i32 {
        if t == 0 {
            return 0;
        }
        let fresh = self.last_photo.is_none_or(|p| p.elapsed() > PHOTO_IDLE);
        if fresh || t.signum() != self.photo_dir {
            self.photo_ticks = t.signum() * self.binding.mapping.photo_detent;
        } else {
            self.photo_ticks += t;
        }
        self.photo_dir = t.signum();
        self.last_photo = Some(Instant::now());
        let step = self.photo_ticks / self.binding.mapping.photo_detent;
        self.photo_ticks %= self.binding.mapping.photo_detent;
        step.signum()
    }
    pub(crate) fn translate(&mut self, msg: Msg) -> commands::Result<Option<Command>> {
        use commands::{Action as A, Error, Operation as O};
        match &msg {
            Msg::Cc(cc, value) => {
                locked(&self.status).last = Some(Last::Dial(*cc, i32::from(*value)))
            }
            Msg::Note(note, true) => locked(&self.status).last = Some(Last::Button(*note)),
            _ => {}
        }
        let action = match msg {
            Msg::Command(command) => return Ok(Some(command)),
            Msg::Cc(cc, v) if self.binding.mapping.photo_dial == Some(cc) => {
                let ticks = self.binding.mapping.encoder(cc).ticks(v).ok_or_else(|| {
                    Error::new(
                        "unsupported_control",
                        "Photo navigation requires a relative encoder",
                    )
                })?;
                let step = self.photo_turn(ticks);
                return Ok((step != 0).then(|| Command::new(O::DeviceNavigate(step))));
            }
            Msg::Cc(cc, v) => {
                let p = self
                    .binding
                    .mapping
                    .dials
                    .get(&cc)
                    .copied()
                    .ok_or_else(|| {
                        Error::new("unmapped_control", "No action is mapped to this CC")
                    })?;
                return Ok(Some(Command::new(
                    match self.binding.mapping.encoder(cc).ticks(v) {
                        Some(ticks) => O::Adjust(p, ticks * self.binding.mapping.sensitivity),
                        None => O::ControlValue(p, v),
                    },
                )));
            }
            Msg::Note(n, down) => {
                let a = self
                    .binding
                    .mapping
                    .buttons
                    .get(&n)
                    .copied()
                    .ok_or_else(|| {
                        Error::new("unmapped_control", "No action is mapped to this note")
                    })?;
                if let Action::Hold(m) = a {
                    self.held = if down {
                        self.held | m
                    } else {
                        without(self.held, m)
                    };
                    return Ok(None);
                }
                if !down {
                    return Ok(None);
                }
                a
            }
            #[cfg(any(test, target_os = "macos", target_os = "windows"))]
            Msg::Midi(..) => unreachable!("MIDI envelope removed before translation"),
            Msg::Request(_) => return Err(Error::new("invalid_request", "Nested request")),
        };
        let advance = self.held.shift || matches!(action,Action::Key(_,m) if m.shift);
        let action = match action {
            Action::Mixer(c) => A::Mixer(c),
            Action::ToggleMono => A::ToggleMono,
            Action::Named(a) => a,
            Action::Key(k, m) => shortcut_action(k, m | self.held).ok_or_else(|| {
                Error::new(
                    "unsupported_action",
                    "This shortcut has no application action",
                )
            })?,
            Action::Hold(_) => return Ok(None),
        };
        let operation = if let Some(edit) = action.metadata() {
            O::Metadata { edit, advance }
        } else {
            O::Action(action)
        };
        Ok(Some(Command::new(operation)))
    }
}
