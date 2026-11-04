//! Device instances have their own settings. Script/AI setup is independent.
use super::*;
use crate::app::ui::design::{Tone, text};
use crate::app::{
    develop::panels::inspector::BANDS,
    preferences::{gap, group},
    ui::theme,
    ui::widgets::form_row,
};
use mapping::supported_action as parse_action;
/// What a button can be set to from the list: its name and the key it presses,
/// spelled as in `midi.json`. Anything else is typed in the field beside it.
pub(super) const PRESETS: [(&str, &str); 47] = [
    ("Do nothing", ""),
    ("Undo", "cmd+z"),
    ("Redo", "cmd+shift+z"),
    ("Previous photo", "left"),
    ("Next photo", "right"),
    ("Arrow up", "arrowup"),
    ("Arrow down", "arrowdown"),
    ("Star rating 0", "0"),
    ("Star rating 1", "1"),
    ("Star rating 2", "2"),
    ("Star rating 3", "3"),
    ("Star rating 4", "4"),
    ("Star rating 5", "5"),
    ("Pick", "p"),
    ("Reject", "x"),
    ("Unflag", "u"),
    ("Label red", "6"),
    ("Label yellow", "7"),
    ("Label green", "8"),
    ("Label blue", "9"),
    ("Before / after", "backslash"),
    ("Show clipping", "j"),
    ("Toggle zoom", "z"),
    ("Crop & Straighten", "r"),
    ("Spot removal", "q"),
    ("White balance selector", "w"),
    ("Masking", "shift+w"),
    ("Hide / show the side panels", "tab"),
    ("Hide / show all panels", "shift+tab"),
    ("Convert to black & white or color", "toggle:bw"),
    ("Color Mixer: Hue", "mixer:hue"),
    ("Color Mixer: Saturation", "mixer:sat"),
    ("Color Mixer: Luminance", "mixer:lum"),
    ("Copy settings…", "cmd+shift+c"),
    ("Paste settings", "cmd+shift+v"),
    ("Paste settings from previous", "cmd+alt+v"),
    ("Sync settings…", "cmd+shift+s"),
    ("Reset all settings", "cmd+shift+r"),
    ("Auto tone", "cmd+shift+u"),
    ("Export…", "cmd+shift+e"),
    ("Export with previous", "cmd+alt+shift+e"),
    ("Library grid", "g"),
    ("Develop", "d"),
    ("Hold Shift", "hold:shift"),
    ("Hold Command", "hold:cmd"),
    ("Hold Alt", "hold:alt"),
    ("Show in Loupe", "e"),
];

/// What a dial does.
#[derive(Clone, Copy, PartialEq)]
enum DialUse {
    None,
    Slider(Param),
    /// Previous / next photo.
    Photos,
}
impl DialUse {
    fn label(self) -> String {
        match self {
            DialUse::None => "Do nothing".into(),
            DialUse::Photos => "Previous / next photo".into(),
            DialUse::Slider(Param::Band(i)) => format!("Color Mixer: {}", BANDS[i]),
            DialUse::Slider(p) => {
                let name = p.spec();
                let mut chars = name.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            }
        }
    }
    /// Every choice, in the order of the list.
    fn all() -> Vec<DialUse> {
        let sliders = Param::NAMED.iter().map(|(_, p)| DialUse::Slider(*p));
        let bands = (0..8).map(|i| DialUse::Slider(Param::Band(i)));
        [DialUse::None]
            .into_iter()
            .chain(sliders)
            .chain(bands)
            .chain([DialUse::Photos])
            .collect()
    }
}

fn dial_use(config: &Config, cc: u8) -> DialUse {
    if config.photo_dial == Some(cc) {
        DialUse::Photos
    } else {
        config
            .dials
            .get(&cc)
            .map_or(DialUse::None, |p| DialUse::Slider(*p))
    }
}
fn set_dial(config: &mut Config, cc: u8, choice: DialUse) {
    config.dials.remove(&cc);
    if config.photo_dial == Some(cc) {
        config.photo_dial = None;
    }
    match choice {
        DialUse::None => {}
        DialUse::Slider(p) => {
            config.dials.insert(cc, p);
        }
        // Only one dial moves between photos.
        DialUse::Photos => config.photo_dial = Some(cc),
    }
}

fn small(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(text::LABEL)
            .color(theme::palette(ui.ctx()).tone(Tone::MUTED)),
    );
}
pub(super) fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(text::LABEL)
                .color(theme::palette(ui.ctx()).tone(Tone::MUTED)),
        )
        .wrap(),
    );
}

fn mappings(
    ui: &mut egui::Ui,
    config: &mut Config,
    profile: Profile,
    device_id: u64,
    last: Option<Last>,
) {
    let ctx = ui.ctx().clone();
    form_row(ui, "Add a control", |ui| {
        let id = egui::Id::new(("control-number", device_id));
        let mut number = ui.data(|d| d.get_temp::<u8>(id)).unwrap_or(0);
        ui.add(egui::DragValue::new(&mut number).range(0..=127));
        ui.data_mut(|d| d.insert_temp(id, number));
        if ui.button("Add dial / slider").clicked() {
            ui.data_mut(|d| d.insert_temp(egui::Id::new(("add-cc", device_id)), number));
        }
        if ui.button("Add button").clicked() {
            ui.data_mut(|d| d.insert_temp(egui::Id::new(("add-note", device_id)), number));
        }
    });
    hint(
        ui,
        "Move a control to discover its number, or add it by CC / note number. Assign an action below.",
    );
    group(ui, "Dials and faders");
    let choices = DialUse::all();
    let mut dials: Vec<(String, u8)> = profile
        .dials()
        .iter()
        .map(|(n, id)| ((*n).into(), *id))
        .collect();
    let mut extra: Vec<u8> = config
        .dials
        .keys()
        .copied()
        .chain(config.encoders.keys().copied())
        .chain(match last {
            Some(Last::Dial(n, _)) => Some(n),
            _ => None,
        })
        .filter(|id| !profile.dials().iter().any(|(_, n)| n == id))
        .collect();
    extra.sort_unstable();
    extra.dedup();
    dials.extend(extra.into_iter().map(|id| (format!("CC {id}"), id)));
    let draft = ui.data(|d| d.get_temp::<u8>(egui::Id::new(("add-cc", device_id))));
    if let Some(cc) = draft
        && !dials.iter().any(|(_, n)| *n == cc)
    {
        dials.push((format!("CC {cc}"), cc));
    }
    for (name, cc) in dials {
        let now = dial_use(config, cc);
        let mut choice = now;
        form_row(ui, &name, |ui| {
            egui::ComboBox::from_id_salt(("automation-dial", device_id, cc))
                .width(210.)
                .selected_text(now.label())
                .show_ui(ui, |ui| {
                    for option in &choices {
                        ui.selectable_value(&mut choice, *option, option.label());
                    }
                });
            small(ui, &format!("CC {cc}"));
        });
        form_row(ui, "Control format", |ui| {
            let mut encoder = config.encoder(cc);
            egui::ComboBox::from_id_salt(("encoder", device_id, cc))
                .selected_text(encoder.label())
                .show_ui(ui, |ui| {
                    for mode in Encoder::ALL {
                        ui.selectable_value(&mut encoder, mode, mode.label());
                    }
                });
            if encoder != config.encoder(cc) {
                config.encoders.insert(cc, encoder);
            }
        });
        if choice == DialUse::Photos && config.encoder(cc) == Encoder::Absolute {
            hint(ui, "Photo navigation needs a relative control format.");
        }
        if choice != now {
            set_dial(config, cc, choice);
        }
    }
    gap(ui);

    group(ui, "Buttons");
    let mut buttons: Vec<(String, u8)> = profile
        .buttons()
        .iter()
        .map(|(n, id)| ((*n).into(), *id))
        .collect();
    let mut extra: Vec<u8> = config
        .buttons
        .keys()
        .copied()
        .chain(match last {
            Some(Last::Button(n)) => Some(n),
            _ => None,
        })
        .filter(|id| !profile.buttons().iter().any(|(_, n)| n == id))
        .collect();
    extra.sort_unstable();
    extra.dedup();
    buttons.extend(extra.into_iter().map(|id| (format!("Note {id}"), id)));
    let draft = ui.data(|d| d.get_temp::<u8>(egui::Id::new(("add-note", device_id))));
    if let Some(note) = draft
        && !buttons.iter().any(|(_, n)| *n == note)
    {
        buttons.push((format!("Note {note}"), note));
    }
    for (name, note) in buttons {
        let current = config.buttons.get(&note).copied();
        let preset = PRESETS
            .iter()
            .find(|(_, spec)| match current {
                None => spec.is_empty(),
                Some(action) => !spec.is_empty() && parse_action(spec) == Some(action),
            })
            .map(|(label, _)| *label);
        let mut picked = None;
        let mut typed = None;
        form_row(ui, &name, |ui| {
            egui::ComboBox::from_id_salt(("automation-button", device_id, note))
                .width(170.)
                .selected_text(preset.unwrap_or("Custom"))
                .show_ui(ui, |ui| {
                    for (label, spec) in PRESETS {
                        if ui.selectable_label(preset == Some(label), label).clicked() {
                            picked = Some(spec);
                        }
                    }
                });
            // What is typed stays as typed while the field has focus, even
            // while it is not yet a key.
            let id = egui::Id::new(("automation-keys", device_id, note));
            let mut text = ui
                .data(|d| d.get_temp::<String>(id))
                .unwrap_or_else(|| current.map(super::action_spec).unwrap_or_default());
            let valid = text.trim().is_empty() || parse_action(text.trim()).is_some();
            let response = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(100.)
                    .hint_text("action")
                    .text_color_opt((!valid).then(|| egui::Color32::from_rgb(230, 100, 90))),
            );
            if response.changed() {
                ui.data_mut(|d| d.insert_temp(id, text.clone()));
                typed = Some(text);
            }
            if response.lost_focus() {
                ui.data_mut(|d| d.remove_temp::<String>(id));
            }
            small(ui, &format!("Note {note}"));
        });
        let spec = picked.or(typed.as_deref()).map(str::trim);
        if picked.is_some() {
            ctx.data_mut(|d| {
                d.remove_temp::<String>(egui::Id::new(("automation-keys", device_id, note)))
            });
        }
        match spec {
            Some("") => {
                config.buttons.remove(&note);
            }
            Some(spec) => {
                if let Some(action) = parse_action(spec) {
                    config.buttons.insert(note, action);
                }
            }
            None => {}
        }
    }
    gap(ui);
    hint(
        ui,
        "Buttons run application actions. Enter a name such as undo, pick or treatment:bw, or a supported shortcut such as cmd+shift+z. Dials follow the selected mask while Masking is active.",
    );
}
impl Editor {
    pub(in crate::app) fn automation_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if let Some(error) = &self.controls.load_error {
            hint(ui, error);
            hint(
                ui,
                "Fix the configuration file and restart to edit automation settings. Existing files have been left intact.",
            );
            return;
        }
        let mut settings = self.controls.settings.clone();
        group(ui, "Scripts and AI");
        form_row(ui, "Local access", |ui| {
            ui.checkbox(&mut settings.socket, "Allow local scripts and applications");
        });
        form_row(ui, "Status", |ui| {
            small(
                ui,
                if self.controls.socket.is_some() {
                    "Ready for scripts and MCP clients"
                } else if settings.socket {
                    "Could not start local access"
                } else {
                    "Off"
                },
            );
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Connect Codex or another MCP client to control the editor. No MIDI device is required. Access is limited to programs running under your local account.",
            );
        });
        form_row(ui, "Setup", |ui| {
            let error_id = egui::Id::new("mcp-guide-error");
            if ui
                .link("MCP setup · macOS, Linux and Windows")
                .on_hover_text("Open the setup guide in your default browser")
                .clicked()
            {
                // This build omits eframe's links feature. Use the same platform
                // launcher as app updates; build.rs selects matching documentation.
                match crate::platform::web::open(env!("RAWMAKASE_MCP_GUIDE")) {
                    Ok(()) => {
                        ui.data_mut(|d| d.remove_temp::<String>(error_id));
                    }
                    Err(error) => ui.data_mut(|d| {
                        d.insert_temp(error_id, format!("Could not open the setup guide: {error}"));
                    }),
                }
            }
            if let Some(error) = ui.data(|d| d.get_temp::<String>(error_id)) {
                hint(ui, &error);
            }
        });
        gap(ui);
        group(ui, "MIDI devices");
        ui.horizontal(|ui| {
            ui.menu_button("Add device", |ui| {
                for profile in Profile::ALL {
                    if ui.button(profile.label()).clicked() {
                        self.controls.selected = Some(settings.add(profile));
                        ui.close();
                    }
                }
            });
            if ui.button("Refresh inputs").clicked() || !self.controls.ports_scanned {
                self.controls.ports = midi::ports();
                self.controls.ports_scanned = true;
            }
        });
        if settings.devices.is_empty() {
            hint(
                ui,
                "No MIDI devices configured. Add a Loupedeck+ or a custom controller; each device keeps its own mappings.",
            );
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        hint(
            ui,
            "MIDI connections are currently supported on macOS and Windows. Scripts and MCP work independently on this platform.",
        );
        for device in &settings.devices {
            let status = self
                .controls
                .devices
                .iter()
                .find(|d| d.binding.id == device.id)
                .map(|d| locked(&d.status).connected.is_some())
                .unwrap_or(false);
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.controls.selected == Some(device.id), &device.name)
                    .clicked()
                {
                    self.controls.selected = Some(device.id);
                }
                small(
                    ui,
                    if !device.enabled {
                        "Disabled"
                    } else if status {
                        "Connected"
                    } else if device.port.is_empty() {
                        "Choose an input"
                    } else {
                        "Not connected"
                    },
                );
            });
        }
        if self.controls.selected.is_none() {
            self.controls.selected = settings.devices.first().map(|d| d.id);
        }
        let mut remove = None;
        if let Some(device) = settings
            .devices
            .iter_mut()
            .find(|d| Some(d.id) == self.controls.selected)
        {
            gap(ui);
            group(ui, "Device settings");
            form_row(ui, "Name", |ui| {
                ui.text_edit_singleline(&mut device.name);
            });
            form_row(ui, "Enabled", |ui| {
                ui.checkbox(&mut device.enabled, "Listen to this device");
            });
            form_row(ui, "Profile", |ui| {
                small(ui, device.profile.label());
                if ui.button("Reset mappings to profile").clicked() {
                    device.mapping = device.profile.mapping();
                }
            });
            form_row(ui, "MIDI input", |ui| {
                egui::ComboBox::from_id_salt(("port", device.id))
                    .selected_text(if device.port.is_empty() {
                        "Choose an input"
                    } else {
                        &device.port
                    })
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(device.port.is_empty(), "Not assigned")
                            .clicked()
                        {
                            device.port.clear();
                            device.port_id = None;
                            device.exact = true;
                        }
                        for port in &self.controls.ports {
                            if ui
                                .selectable_label(
                                    device.port_id.as_ref() == Some(&port.id),
                                    format!("{} · {}", port.name, port.id),
                                )
                                .clicked()
                            {
                                device.port = port.name.clone();
                                device.port_id = Some(port.id.clone());
                                device.exact = true;
                            }
                        }
                    });
            });
            if self.controls.ports.is_empty() {
                form_row(ui, "", |ui| {
                    hint(
                        ui,
                        "No MIDI inputs found. Connect your device, then refresh inputs.",
                    )
                });
            }
            ui.collapsing("Advanced connection", |ui| {
                form_row(ui, "Port name", |ui| {
                    if ui.text_edit_singleline(&mut device.port).changed() {
                        device.port_id = None;
                    }
                });
                form_row(ui, "", |ui| {
                    if ui
                        .checkbox(&mut device.exact, "Match the entire port name")
                        .changed()
                    {
                        device.port_id = None;
                    }
                });
                form_row(ui, "MIDI channel", |ui| {
                    egui::ComboBox::from_id_salt(("channel", device.id))
                        .selected_text(
                            device
                                .channel
                                .map_or("All channels".into(), |c| c.to_string()),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut device.channel, None, "All channels");
                            for c in 1..=16 {
                                ui.selectable_value(&mut device.channel, Some(c), c.to_string());
                            }
                        });
                });
                form_row(ui, "Turn speed", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut device.mapping.sensitivity)
                            .range(1..=16)
                            .suffix("×"),
                    );
                });
                form_row(ui, "Photo dial", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut device.mapping.photo_detent)
                            .range(1..=64)
                            .suffix(" ticks per photo"),
                    );
                });
            });
            let runtime = self
                .controls
                .devices
                .iter()
                .find(|d| d.binding.id == device.id);
            let last = runtime.and_then(|d| locked(&d.status).last);
            let problem = runtime.and_then(|d| locked(&d.status).problem.clone());
            if let Some(problem) = problem {
                hint(ui, &problem);
            }
            form_row(ui, "Last input", |ui| {
                small(
                    ui,
                    &match last {
                        Some(Last::Dial(cc, v)) => format!("CC {cc} · value {v}"),
                        Some(Last::Button(n)) => format!("Note {n}"),
                        None => "Move a dial or press a button to identify it".into(),
                    },
                );
            });
            ui.collapsing("Control mappings", |ui| {
                mappings(ui, &mut device.mapping, device.profile, device.id, last);
            });
            if ui.button("Remove device").clicked() {
                remove = Some(device.id);
            }
        }
        if let Some(id) = remove {
            settings.devices.retain(|d| d.id != id);
            self.controls.selected = None;
        }
        if settings != self.controls.settings && !ctx.text_edit_focused() {
            match settings.save() {
                Ok(()) => self.controls.apply(settings),
                Err(e) => self
                    .messages
                    .fail(format!("Automation settings not saved: {e:#}")),
            }
        } else if settings != self.controls.settings {
            // Persist text edits on every change; defer listener replacement until focus leaves.
            match settings.save() {
                Ok(()) => self.controls.settings = settings,
                Err(e) => self
                    .messages
                    .fail(format!("Automation settings not saved: {e:#}")),
            }
        }
        if !ctx.text_edit_focused()
            && self.controls.devices.iter().map(|d| &d.binding).ne(self
                .controls
                .settings
                .devices
                .iter())
        {
            self.controls.apply(self.controls.settings.clone());
        }
    }
}
