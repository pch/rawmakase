//! The extension's page in Preferences > Extensions: a panel the app draws
//! from the controls this program sends. It lists the devices, and edits the
//! selected one: its input, channel, turn speed, photo dial and mappings,
//! with its last input and problems. Every change is saved at once; a device
//! whose configuration changed connects again.
//!
//! A typed action is checked with the app (`read.action`): one it doesn't
//! run stays in its field with the reason under it, and isn't saved.
use crate::config::{Encoder, Settings};
use crate::midi::Last;
use crate::profiles::Profile;
use crate::{Midi, PANEL, lock, spec};
use rawmakase_extension::protocol::controls::{Choice, Control, ControlKind};
use rawmakase_extension::{Context, ErrorCode, Panel, RemoteError};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

/// What a button can be set to from the list: its name and the key it
/// presses, spelled as in `midi.json`. Anything else is typed in the field
/// under it. The app's tests check that it runs each.
pub const PRESETS: [(&str, &str); 47] = [
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

/// The sliders a dial can turn, by the control layer's names: those with a
/// name of their own, then the Color Mixer's bands, which turn whichever
/// channel the mixer shows. A dial bound to another name keeps it, listed as
/// it is spelled.
const SLIDERS: [&str; 14] = [
    "exposure",
    "contrast",
    "highlights",
    "shadows",
    "whites",
    "blacks",
    "texture",
    "clarity",
    "dehaze",
    "vibrance",
    "saturation",
    "temperature",
    "tint",
    "straighten",
];
const BANDS: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];

pub(crate) const NO_ACTION: &str = "Not an action RAWmakase runs. Enter a name such as undo, pick or treatment:bw, or a supported shortcut such as cmd+shift+z.";

/// What the page keeps between requests.
#[derive(Default)]
pub(crate) struct PageState {
    /// The page was opened: changes are pushed to it.
    open: bool,
    selected: Option<u64>,
    /// The system's inputs, as last listed (on opening and Refresh inputs).
    ports: Option<Vec<crate::Port>>,
    /// The number Add a control adds.
    number: u8,
    /// Controls added by number and not bound yet, by device.
    dials: HashMap<u64, BTreeSet<u8>>,
    buttons: HashMap<u64, BTreeSet<u8>>,
    /// Text typed for a button that isn't an action, with why.
    typed: HashMap<(u64, u8), (String, String)>,
    /// Why the last change wasn't saved.
    error: Option<String>,
}

impl PageState {
    /// The device the page shows and edits: the one chosen, while it is
    /// there, else the first. Drawing and every change use it, so a device
    /// saved before a restart is edited without being chosen first.
    fn shown(&self, settings: &Settings) -> Option<u64> {
        self.selected
            .filter(|id| settings.devices.iter().any(|d| d.id == *id))
            .or_else(|| settings.devices.first().map(|d| d.id))
    }

    /// Drops what the page kept for device `id` beyond its saved mapping:
    /// controls added and not bound yet, and text typed that isn't an
    /// action. Ids are reused, so a device added later must not show them.
    fn forget(&mut self, id: u64) {
        self.dials.remove(&id);
        self.buttons.remove(&id);
        self.typed.retain(|(device, _), _| *device != id);
    }
}

/// The panel's side of the run loop.
pub(crate) struct Page(pub(crate) Arc<Midi>);

impl Panel for Page {
    fn open(&mut self, _cx: &Context) -> Result<Vec<Control>, RemoteError> {
        let midi = &self.0;
        let scan = lock(&midi.state).page.ports.is_none();
        if scan {
            let ports = midi.backend.ports();
            lock(&midi.state).page.ports = Some(ports);
        }
        lock(&midi.state).page.open = true;
        Ok(controls(midi))
    }

    fn changed(
        &mut self,
        cx: &Context,
        control: &str,
        value: Value,
    ) -> Result<Vec<Control>, RemoteError> {
        let midi = &self.0;
        let invalid = || {
            RemoteError::new(
                ErrorCode::InvalidRequest,
                format!("{value} doesn't fit {control}"),
            )
        };
        let text = value.as_str().map(str::trim);
        let number = value.as_f64().map(|n| n.round() as i64);
        // Checked before the state is taken: the app answers while the
        // devices' threads go on.
        let action = match (control.strip_prefix("keys:"), text) {
            (Some(_), Some(spec)) if !spec.is_empty() => Some(
                cx.host()
                    .read_action(spec)
                    .map_err(|e| format!("The action wasn't checked: {e}")),
            ),
            _ => None,
        };
        edit(midi, |settings, page| {
            if control == "device" {
                page.selected = text.and_then(|t| t.parse().ok());
                return Ok(());
            }
            if control == "number" {
                page.number = number.ok_or_else(invalid)?.clamp(0, 127) as u8;
                return Ok(());
            }
            let id = page.shown(settings).ok_or_else(invalid)?;
            let Some(device) = settings.devices.iter_mut().find(|d| d.id == id) else {
                return Err(invalid());
            };
            let mapping = &mut device.mapping;
            match control {
                "name" => device.name = text.ok_or_else(invalid)?.to_string(),
                "enabled" => device.enabled = value.as_bool().ok_or_else(invalid)?,
                "port" => match text.ok_or_else(invalid)? {
                    "" => {
                        device.port.clear();
                        device.port_id = None;
                        device.exact = true;
                    }
                    choice => {
                        let port_id = choice.strip_prefix("id:").ok_or_else(invalid)?;
                        let ports = page.ports.as_deref().unwrap_or_default();
                        let port = ports.iter().find(|p| p.id == port_id).ok_or_else(invalid)?;
                        device.port.clone_from(&port.name);
                        device.port_id = Some(port.id.clone());
                        device.exact = true;
                    }
                },
                "port-name" => {
                    device.port = text.ok_or_else(invalid)?.to_string();
                    device.port_id = None;
                }
                "exact" => {
                    device.exact = value.as_bool().ok_or_else(invalid)?;
                    device.port_id = None;
                }
                "channel" => {
                    device.channel = match text.ok_or_else(invalid)? {
                        "all" => None,
                        n => Some(
                            n.parse()
                                .ok()
                                .filter(|n| (1..=16).contains(n))
                                .ok_or_else(invalid)?,
                        ),
                    }
                }
                "speed" => mapping.sensitivity = number.ok_or_else(invalid)?.clamp(1, 16) as i32,
                "detent" => mapping.photo_detent = number.ok_or_else(invalid)?.clamp(1, 64) as i32,
                _ => {
                    let (kind, n) = control.split_once(':').ok_or_else(invalid)?;
                    let n: u8 = n.parse().ok().filter(|n| *n <= 127).ok_or_else(invalid)?;
                    let choice = text.ok_or_else(invalid)?;
                    match kind {
                        "dial" => {
                            mapping.dials.remove(&n);
                            if mapping.photo_dial == Some(n) {
                                mapping.photo_dial = None;
                            }
                            if choice == "photos" {
                                // Only one dial moves between photos.
                                mapping.photo_dial = Some(n);
                            } else if let Some(param) = choice.strip_prefix("param:") {
                                mapping.dials.insert(n, param.to_string());
                            }
                        }
                        "format" => {
                            let encoder = Encoder::ALL
                                .into_iter()
                                .find(|e| e.id() == choice)
                                .ok_or_else(invalid)?;
                            mapping.encoders.insert(n, encoder);
                        }
                        "button" => {
                            if choice != "custom" {
                                let i: usize = choice
                                    .strip_prefix("preset:")
                                    .and_then(|i| i.parse().ok())
                                    .filter(|i| *i < PRESETS.len())
                                    .ok_or_else(invalid)?;
                                page.typed.remove(&(id, n));
                                match PRESETS[i].1 {
                                    "" => mapping.buttons.remove(&n),
                                    spec => mapping.buttons.insert(n, spec.to_string()),
                                };
                            }
                        }
                        "keys" => match action {
                            None => {
                                page.typed.remove(&(id, n));
                                mapping.buttons.remove(&n);
                            }
                            Some(Ok(true)) => {
                                page.typed.remove(&(id, n));
                                mapping.buttons.insert(n, choice.to_string());
                            }
                            Some(Ok(false)) => {
                                page.typed
                                    .insert((id, n), (choice.to_string(), NO_ACTION.into()));
                            }
                            Some(Err(why)) => {
                                page.typed.insert((id, n), (choice.to_string(), why));
                            }
                        },
                        _ => return Err(invalid()),
                    }
                }
            }
            Ok(())
        })?;
        Ok(controls(midi))
    }

    fn clicked(&mut self, _cx: &Context, control: &str) -> Result<Vec<Control>, RemoteError> {
        let midi = &self.0;
        if control == "refresh" {
            let ports = midi.backend.ports();
            lock(&midi.state).page.ports = Some(ports);
            return Ok(controls(midi));
        }
        edit(midi, |settings, page| {
            match control {
                "add-loupedeck" => page.selected = Some(settings.add(Profile::Loupedeck)),
                "add-custom" => page.selected = Some(settings.add(Profile::Custom)),
                _ => {
                    let id = page.shown(settings).ok_or_else(|| unknown(control))?;
                    match control {
                        "reset" => {
                            if let Some(d) = settings.devices.iter_mut().find(|d| d.id == id) {
                                d.mapping = d.profile.mapping();
                            }
                            page.forget(id);
                        }
                        "remove" => {
                            settings.devices.retain(|d| d.id != id);
                            page.selected = None;
                            page.forget(id);
                        }
                        "add-dial" => {
                            page.dials.entry(id).or_default().insert(page.number);
                        }
                        "add-button" => {
                            page.buttons.entry(id).or_default().insert(page.number);
                        }
                        _ => return Err(unknown(control)),
                    }
                }
            }
            Ok(())
        })?;
        Ok(controls(midi))
    }
}

fn unknown(control: &str) -> RemoteError {
    RemoteError::new(
        ErrorCode::InvalidRequest,
        format!("{control} is not a button"),
    )
}

/// Applies a change of the page: saved first, then taken up by the devices'
/// threads. A file that wasn't read is never written, and a change that isn't
/// saved is dropped with why.
fn edit(
    midi: &Arc<Midi>,
    change: impl FnOnce(&mut Settings, &mut PageState) -> Result<(), RemoteError>,
) -> Result<(), RemoteError> {
    let changed = {
        let mut state = lock(&midi.state);
        if state.load_error.is_some() {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                "the MIDI settings weren't read, so they aren't changed",
            ));
        }
        let mut settings = state.settings.clone();
        change(&mut settings, &mut state.page)?;
        if settings == state.settings {
            false
        } else {
            match settings.save(&state.data_dir) {
                Ok(()) => {
                    state.settings = settings;
                    state.page.error = None;
                    true
                }
                Err(why) => {
                    state.page.error = Some(format!("Not saved: {why}"));
                    false
                }
            }
        }
    };
    if changed {
        midi.apply();
    }
    Ok(())
}

/// Sends the page's controls to the app, when it was opened.
pub(crate) fn push(midi: &Midi) {
    if !lock(&midi.state).page.open {
        return;
    }
    if let Some(host) = midi.host() {
        let _ = host.panel_update(PANEL, controls(midi));
    }
}

fn control(id: impl Into<Option<String>>, label: &str, kind: ControlKind) -> Control {
    Control {
        id: id.into(),
        label: (!label.is_empty()).then(|| label.to_string()),
        help: None,
        visible_when: None,
        disabled: false,
        kind,
    }
}
fn help(text: &str) -> Control {
    control(None, "", ControlKind::Help { text: text.into() })
}
fn label(label: &str, text: String) -> Control {
    control(None, label, ControlKind::Label { text })
}
fn button(id: &str, title: &str) -> Control {
    control(
        id.to_string(),
        "",
        ControlKind::Button {
            title: title.into(),
        },
    )
}
fn section(title: &str, collapsed: bool, controls: Vec<Control>) -> Control {
    control(
        None,
        "",
        ControlKind::Section {
            title: title.into(),
            controls,
            collapsed,
        },
    )
}
fn choice(id: String, label: &str, options: Vec<(String, String)>, value: String) -> Control {
    let options = options
        .into_iter()
        .map(|(id, title)| Choice { id, title })
        .collect();
    control(id, label, ControlKind::Choice { options, value })
}
fn number(id: &str, label: &str, value: i64, min: i64, max: i64) -> Control {
    control(
        id.to_string(),
        label,
        ControlKind::Number {
            value: value as f64,
            min: Some(min as f64),
            max: Some(max as f64),
        },
    )
}
fn text(id: String, label: &str, value: String) -> Control {
    control(
        id,
        label,
        ControlKind::Text {
            value,
            multiline: false,
        },
    )
}
fn toggle(id: &str, label: &str, value: bool) -> Control {
    control(id.to_string(), label, ControlKind::Toggle { value })
}

/// A slider's name as a dial's choice shows it.
fn slider_title(name: &str) -> String {
    if let Some(i) = name
        .strip_prefix("band")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=8).contains(n))
    {
        return format!("Color Mixer: {}", BANDS[i - 1]);
    }
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The page's controls, whole.
fn controls(midi: &Midi) -> Vec<Control> {
    let state = lock(&midi.state);
    if let Some(why) = &state.load_error {
        return vec![
            help(why),
            help(
                "Fix the configuration file and restart RAWmakase to edit MIDI devices. The file has been left as it is.",
            ),
        ];
    }
    let statuses = lock(&midi.statuses).clone();
    let page = &state.page;
    let settings = &state.settings;
    let mut out = Vec::new();
    if !midi.backend.available() {
        out.push(help(
            "MIDI connections are currently supported on macOS and Windows. Scripts and MCP work independently on this platform.",
        ));
    }
    let mut devices = Vec::new();
    if settings.devices.is_empty() {
        devices.push(help(
            "No MIDI devices configured. Add a Loupedeck+ or a custom controller; each device keeps its own mappings.",
        ));
    }
    let selected = page.shown(settings);
    for device in &settings.devices {
        let connected = statuses
            .get(&device.id)
            .is_some_and(|s| s.connected.is_some());
        let state = if !device.enabled {
            "Disabled"
        } else if connected {
            "Connected"
        } else if device.port.is_empty() {
            "Choose an input"
        } else {
            "Not connected"
        };
        devices.push(label(&device.name, state.into()));
    }
    if let Some(selected) = selected
        && settings.devices.len() > 1
    {
        let options = settings
            .devices
            .iter()
            .map(|d| (d.id.to_string(), d.name.clone()))
            .collect();
        devices.push(choice(
            "device".into(),
            "Edit",
            options,
            selected.to_string(),
        ));
    }
    for profile in Profile::ALL {
        devices.push(button(
            &format!("add-{}", profile.id()),
            &format!("Add {}", profile.label()),
        ));
    }
    devices.push(button("refresh", "Refresh inputs"));
    if let Some(why) = &page.error {
        devices.push(help(why));
    }
    out.push(section("Devices", false, devices));
    let Some(device) = selected.and_then(|id| settings.devices.iter().find(|d| d.id == id)) else {
        return out;
    };
    let status = statuses.get(&device.id).cloned().unwrap_or_default();
    let ports = page.ports.as_deref().unwrap_or_default();
    let mut options = vec![(String::new(), "Not assigned".to_string())];
    options.extend(
        ports
            .iter()
            .map(|p| (format!("id:{}", p.id), format!("{} · {}", p.name, p.id))),
    );
    let port_value = match &device.port_id {
        Some(id) => format!("id:{id}"),
        None if device.port.is_empty() => String::new(),
        None => "name".into(),
    };
    if !options.iter().any(|(id, _)| *id == port_value) {
        let title = if device.port.is_empty() {
            "An input not connected".to_string()
        } else {
            device.port.clone()
        };
        options.push((port_value.clone(), title));
    }
    let mut settings_ = vec![
        text("name".into(), "Name", device.name.clone()),
        toggle("enabled", "Listen to this device", device.enabled),
        label("Profile", device.profile.label().into()),
        button("reset", "Reset mappings to profile"),
        choice("port".into(), "MIDI input", options, port_value),
    ];
    if ports.is_empty() {
        settings_.push(help(
            "No MIDI inputs found. Connect your device, then refresh inputs.",
        ));
    }
    let mut channels = vec![("all".to_string(), "All channels".to_string())];
    channels.extend((1..=16).map(|c| (c.to_string(), c.to_string())));
    settings_.push(section(
        "Advanced connection",
        true,
        vec![
            text("port-name".into(), "Port name", device.port.clone()),
            toggle("exact", "Match the entire port name", device.exact),
            choice(
                "channel".into(),
                "MIDI channel",
                channels,
                device.channel.map_or("all".into(), |c| c.to_string()),
            ),
            number(
                "speed",
                "Turn speed (×)",
                i64::from(device.mapping.sensitivity),
                1,
                16,
            ),
            number(
                "detent",
                "Photo dial: ticks per photo",
                i64::from(device.mapping.photo_detent),
                1,
                64,
            ),
        ],
    ));
    if let Some(problem) = &status.problem {
        settings_.push(help(problem));
    }
    settings_.push(label(
        "Last input",
        match status.last {
            Some(Last::Dial(cc, v)) => format!("CC {cc} · value {v}"),
            Some(Last::Button(n)) => format!("Note {n}"),
            None => "Move a dial or press a button to identify it".into(),
        },
    ));
    settings_.push(section(
        "Control mappings",
        true,
        mappings(midi, device, page, status.last),
    ));
    settings_.push(button("remove", "Remove device"));
    out.push(section("Device settings", false, settings_));
    out
}

/// The selected device's dials and buttons: its profile's, those it binds,
/// those added by number and the last one moved.
fn mappings(
    midi: &Midi,
    device: &crate::DeviceConfig,
    page: &PageState,
    last: Option<Last>,
) -> Vec<Control> {
    let named = |t: &str| midi.named(t);
    let mapping = &device.mapping;
    let profile = device.profile;
    let mut out = vec![
        number("number", "Control number", i64::from(page.number), 0, 127),
        button("add-dial", "Add dial / slider"),
        button("add-button", "Add button"),
        help(
            "Move a control to discover its number, or add it by CC / note number. Assign an action below.",
        ),
    ];
    let mut dials: Vec<(String, u8)> = profile
        .dials()
        .iter()
        .map(|(n, c)| ((*n).into(), *c))
        .collect();
    let mut extra: BTreeSet<u8> = mapping
        .dials
        .keys()
        .chain(mapping.encoders.keys())
        .chain(mapping.photo_dial.iter())
        .copied()
        .chain(page.dials.get(&device.id).into_iter().flatten().copied())
        .collect();
    if let Some(Last::Dial(cc, _)) = last {
        extra.insert(cc);
    }
    extra.retain(|cc| !dials.iter().any(|(_, n)| n == cc));
    dials.extend(extra.into_iter().map(|cc| (format!("CC {cc}"), cc)));
    let mut sliders: Vec<(String, String)> = vec![("none".into(), "Do nothing".into())];
    sliders.extend(
        SLIDERS
            .iter()
            .map(|s| s.to_string())
            .chain((1..=8).map(|i| format!("band{i}")))
            .map(|s| (format!("param:{s}"), slider_title(&s))),
    );
    sliders.push(("photos".into(), "Previous / next photo".into()));
    let formats: Vec<(String, String)> = Encoder::ALL
        .iter()
        .map(|e| (e.id().to_string(), e.label().to_string()))
        .collect();
    let mut dial_controls = Vec::new();
    for (name, cc) in dials {
        let value = if mapping.photo_dial == Some(cc) {
            "photos".to_string()
        } else {
            mapping
                .dials
                .get(&cc)
                .map_or("none".into(), |p| format!("param:{p}"))
        };
        let mut options = sliders.clone();
        if !options.iter().any(|(id, _)| *id == value) {
            options.push((
                value.clone(),
                value.trim_start_matches("param:").to_string(),
            ));
        }
        let mut dial = choice(
            format!("dial:{cc}"),
            &format!("{name} · CC {cc}"),
            options,
            value.clone(),
        );
        if value == "photos" && mapping.encoder(cc) == Encoder::Absolute {
            dial.help = Some("Photo navigation needs a relative control format.".into());
        }
        dial_controls.push(dial);
        dial_controls.push(choice(
            format!("format:{cc}"),
            "Control format",
            formats.clone(),
            mapping.encoder(cc).id().into(),
        ));
    }
    out.push(section("Dials and faders", false, dial_controls));
    let mut buttons: Vec<(String, u8)> = profile
        .buttons()
        .iter()
        .map(|(n, c)| ((*n).into(), *c))
        .collect();
    let mut extra: BTreeSet<u8> = mapping
        .buttons
        .keys()
        .copied()
        .chain(page.buttons.get(&device.id).into_iter().flatten().copied())
        .collect();
    if let Some(Last::Button(note)) = last {
        extra.insert(note);
    }
    extra.retain(|note| !buttons.iter().any(|(_, n)| n == note));
    buttons.extend(extra.into_iter().map(|note| (format!("Note {note}"), note)));
    let mut presets: Vec<(String, String)> = PRESETS
        .iter()
        .enumerate()
        .map(|(i, (label, _))| (format!("preset:{i}"), (*label).to_string()))
        .collect();
    presets.push(("custom".into(), "Custom".into()));
    let mut button_controls = Vec::new();
    for (name, note) in buttons {
        let current = mapping.buttons.get(&note);
        let preset = PRESETS.iter().position(|(_, spec)| match current {
            None => spec.is_empty(),
            Some(bound) => !spec.is_empty() && spec::same(spec, bound, &named),
        });
        button_controls.push(choice(
            format!("button:{note}"),
            &format!("{name} · Note {note}"),
            presets.clone(),
            preset.map_or("custom".into(), |i| format!("preset:{i}")),
        ));
        let (value, why) = match page.typed.get(&(device.id, note)) {
            Some((typed, why)) => (typed.clone(), Some(why.clone())),
            None => (current.cloned().unwrap_or_default(), None),
        };
        let mut keys = text(format!("keys:{note}"), "Action", value);
        keys.help = why;
        button_controls.push(keys);
    }
    out.push(section("Buttons", false, button_controls));
    out.push(help(
        "Buttons run application actions. Enter a name such as undo, pick or treatment:bw, or a supported shortcut such as cmd+shift+z. Dials follow the selected mask while Masking is active.",
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_is_a_shape_the_app_reads() {
        for (label, preset) in PRESETS {
            if preset.is_empty() {
                continue;
            }
            assert!(
                preset == "toggle:bw"
                    || preset.starts_with("mixer:")
                    || !matches!(spec::Spec::parse(preset, &|_| false), spec::Spec::Other),
                "{label}: {preset}"
            );
        }
    }

    #[test]
    fn the_loupedeck_buttons_show_their_presets() {
        let mapping = Profile::Loupedeck.mapping();
        let preset = |note: u8| {
            let bound = &mapping.buttons[&note];
            PRESETS
                .iter()
                .find(|(_, spec)| !spec.is_empty() && spec::same(spec, bound, &|_| false))
                .map(|(label, _)| *label)
        };
        assert_eq!(preset(95), Some("Undo"));
        assert_eq!(preset(76), Some("Arrow up"));
        assert_eq!(preset(78), Some("Previous photo"));
        assert_eq!(preset(102), Some("Before / after"));
        assert_eq!(preset(66), Some("Hold Shift"));
        assert_eq!(preset(67), None, "Control alone has no preset");
    }

    #[test]
    fn sliders_are_titled_as_the_page_always_did() {
        assert_eq!(slider_title("exposure"), "Exposure");
        assert_eq!(slider_title("band3"), "Color Mixer: Yellow");
        assert_eq!(slider_title("band3.sat"), "Band3.sat");
    }
}
