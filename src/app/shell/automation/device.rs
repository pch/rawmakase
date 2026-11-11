//! Independent mapping and held-control state for one MIDI device, and the
//! translation of its input into named [`DeviceOp`]s. The photo dial's detent
//! state is the core's ([`Hub::photo_turn`]), per device source.
use super::*;
pub(super) struct Device {
    pub binding: DeviceConfig,
    #[cfg(any(test, target_os = "macos", target_os = "windows"))]
    pub identity: u64,
    pub held: Modifiers,
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
        }
    }
    /// Whether `input` comes from the photo dial.
    #[cfg_attr(
        not(any(test, target_os = "macos", target_os = "windows")),
        expect(dead_code, reason = "only a MIDI listener sends device input")
    )]
    pub(crate) fn photo_dial(&self, input: Input) -> bool {
        matches!(input, Input::Cc(cc, _) if self.binding.mapping.photo_dial == Some(cc))
    }
    /// The operation `input` stands for in this device's mapping, if any. A
    /// control the mapping doesn't bind does nothing. The editor is never
    /// consulted: [`Editor::device_operate`] applies the operation.
    #[cfg_attr(
        not(any(test, target_os = "macos", target_os = "windows")),
        expect(dead_code, reason = "only a MIDI listener sends device input")
    )]
    pub(crate) fn translate(&mut self, input: Input) -> commands::Result<Option<DeviceOp>> {
        let mapping = &self.binding.mapping;
        Ok(match input {
            Input::Cc(cc, value) => {
                locked(&self.status).last = Some(Last::Dial(cc, i32::from(value)));
                if mapping.photo_dial == Some(cc) {
                    let ticks = mapping.encoder(cc).ticks(value).ok_or_else(|| {
                        commands::Error::new(
                            "unsupported_control",
                            "Photo navigation requires a relative encoder",
                        )
                    })?;
                    Some(DeviceOp::Photo {
                        ticks,
                        detent: mapping.photo_detent,
                    })
                } else if let Some(&param) = mapping.dials.get(&cc) {
                    Some(match mapping.encoder(cc).ticks(value) {
                        Some(ticks) => DeviceOp::Adjust(param, ticks * mapping.sensitivity),
                        None => DeviceOp::Value(param, value),
                    })
                } else {
                    None
                }
            }
            Input::Note(note, down) => {
                if down {
                    locked(&self.status).last = Some(Last::Button(note));
                }
                let action = mapping.buttons.get(&note).copied();
                action.and_then(|action| press(&mut self.held, action, down))
            }
        })
    }
}
