//! The devices and their mappings, kept in `midi.json` in the extension's
//! data folder (`<data>/extension-data/rawmakase.midi/`). The app copies the
//! file the core used to keep (`<data>/midi.json`) there on the first start
//! and leaves the original, so an older RAWmakase still reads its own.
//!
//! The format is the core's, unchanged: version 2 lists devices; a file with
//! no version is the single-Loupedeck format before it (version 0), read as
//! one Loupedeck+ device with the file's overrides. Its `socket` field was the
//! control socket's switch, which the core still reads from the original.
//! A file this program can't read, or of a newer version, is reported and
//! never written: the user's settings stay as they are.
use crate::profiles::Profile;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// The file's name, in the data folder.
pub const FILE: &str = "midi.json";

/// How a dial's control change carries its movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoder {
    TwosComplement,
    Offset,
    SignMagnitude,
    Absolute,
}

impl Encoder {
    pub(crate) const ALL: [Self; 4] = [
        Self::Absolute,
        Self::TwosComplement,
        Self::Offset,
        Self::SignMagnitude,
    ];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Absolute => "Absolute (0–127)",
            Self::TwosComplement => "Relative (1 / 127)",
            Self::Offset => "Relative (65 / 63)",
            Self::SignMagnitude => "Relative (1 / 65)",
        }
    }
    /// Its name in `midi.json`.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::TwosComplement => "twos_complement",
            Self::Offset => "offset",
            Self::SignMagnitude => "sign_magnitude",
            Self::Absolute => "absolute",
        }
    }
    /// The signed ticks a relative control's `value` stands for; none for an
    /// absolute one, whose value is a position.
    pub fn ticks(self, value: u8) -> Option<i32> {
        match self {
            Self::Absolute => None,
            Self::TwosComplement => Some(if value < 64 {
                i32::from(value)
            } else {
                i32::from(value) - 128
            }),
            Self::Offset => Some(i32::from(value) - 64),
            Self::SignMagnitude => Some(if value < 64 {
                i32::from(value)
            } else {
                -i32::from(value & 63)
            }),
        }
    }
}

/// What a device's controls do. Dials name the control layer's parameters
/// (`exposure`, `band3`) and buttons its actions ([`crate::spec`]), as the
/// app reads them: the program keeps what it doesn't know as written, and
/// the app refuses it when it is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub dials: BTreeMap<u8, String>,
    pub buttons: BTreeMap<u8, String>,
    /// The dial that moves between photos.
    pub photo_dial: Option<u8>,
    /// Ticks of the photo dial per photo.
    pub photo_detent: i32,
    pub default_encoder: Encoder,
    pub encoders: BTreeMap<u8, Encoder>,
    /// Ticks a relative dial's tick turns its slider by.
    pub sensitivity: i32,
}

impl Mapping {
    pub fn encoder(&self, cc: u8) -> Encoder {
        self.encoders
            .get(&cc)
            .copied()
            .unwrap_or(self.default_encoder)
    }

    fn to_json(&self) -> Value {
        let dials: BTreeMap<_, _> = self.dials.iter().map(|(n, p)| (n.to_string(), p)).collect();
        let buttons: BTreeMap<_, _> = self
            .buttons
            .iter()
            .map(|(n, a)| (n.to_string(), a))
            .collect();
        let encoders: BTreeMap<_, _> = self
            .encoders
            .iter()
            .map(|(n, e)| (n.to_string(), e))
            .collect();
        json!({"dials": dials, "buttons": buttons, "photo_dial": self.photo_dial,
               "photo_detent": self.photo_detent, "default_encoder": self.default_encoder,
               "encoders": encoders, "sensitivity": self.sensitivity})
    }

    fn from_json(value: &Value) -> Result<Self, String> {
        let mut mapping = Profile::Custom.mapping();
        mapping.apply(value);
        mapping.default_encoder = serde_json::from_value(value["default_encoder"].clone())
            .map_err(|e| format!("default_encoder: {e}"))?;
        let encoders: BTreeMap<u8, Encoder> = serde_json::from_value(value["encoders"].clone())
            .map_err(|e| format!("encoders: {e}"))?;
        if encoders.keys().any(|n| *n > 127) {
            return Err("Invalid control number".into());
        }
        mapping.encoders = encoders;
        mapping.sensitivity = value["sensitivity"].as_i64().unwrap_or(1).clamp(1, 16) as i32;
        Ok(mapping)
    }

    /// Takes the overrides `json` holds: a dial or button set to a name binds
    /// it, set to anything else (`null`) unbinds it.
    pub(crate) fn apply(&mut self, json: &Value) {
        if let Some(dial) = json.get("photo_dial") {
            self.photo_dial = dial.as_u64().and_then(|n| u8::try_from(n).ok());
        }
        if let Some(n) = json["photo_detent"].as_i64() {
            self.photo_detent = n.clamp(1, 64) as i32;
        }
        let entries = |table: &str| {
            json[table]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(number, value)| match number.parse::<u8>() {
                    Ok(n @ 0..=127) => Some((n, value.as_str().filter(|s| !s.trim().is_empty()))),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        for (cc, param) in entries("dials") {
            match param {
                Some(param) => self.dials.insert(cc, param.to_string()),
                None => self.dials.remove(&cc),
            };
        }
        for (note, action) in entries("buttons") {
            match action {
                Some(action) => self.buttons.insert(note, action.to_string()),
                None => self.buttons.remove(&note),
            };
        }
    }
}

/// One device: what it is, which input it listens to and its mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceConfig {
    /// Unique among the devices, never reused while the device exists; the
    /// app's device id is its decimal text.
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    pub profile: Profile,
    /// The input's name, whole (`exact`) or in part.
    pub port: String,
    /// The input chosen from the list, which tells two of the same name apart.
    pub port_id: Option<String>,
    pub exact: bool,
    /// 1–16, or every channel.
    pub channel: Option<u8>,
    pub mapping: Mapping,
}

impl DeviceConfig {
    pub fn new(id: u64, profile: Profile) -> Self {
        Self {
            id,
            name: profile.label().into(),
            enabled: true,
            profile,
            port: String::new(),
            port_id: None,
            exact: true,
            channel: None,
            mapping: profile.mapping(),
        }
    }
    fn to_json(&self) -> Value {
        json!({"id": self.id, "name": self.name, "enabled": self.enabled, "profile": self.profile,
               "port": self.port, "port_id": self.port_id, "exact": self.exact,
               "channel": self.channel, "mapping": self.mapping.to_json()})
    }
}

/// Every device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub devices: Vec<DeviceConfig>,
}

impl Settings {
    /// Reads the file's contents: version 2, or the version 0 single
    /// Loupedeck it migrates. Any other version is refused.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let mut result = Self::default();
        if value.get("version").is_none() {
            let mut device = DeviceConfig::new(1, Profile::Loupedeck);
            device.mapping.apply(value);
            device.port = value["port"].as_str().unwrap_or("Loupedeck").into();
            device.exact = false;
            result.devices.push(device);
            return Ok(result);
        }
        if value["version"] != 2 {
            return Err("Unsupported MIDI configuration version".into());
        }
        let entries = value["devices"].as_array().ok_or("Missing devices")?;
        for entry in entries {
            let id = entry["id"]
                .as_u64()
                .filter(|id| *id > 0)
                .ok_or("Invalid device ID")?;
            if result.devices.iter().any(|d| d.id == id) {
                return Err("Duplicate device ID".into());
            }
            let profile: Profile = serde_json::from_value(entry["profile"].clone())
                .map_err(|e| format!("profile: {e}"))?;
            let mut device = DeviceConfig::new(id, profile);
            device.name = entry["name"].as_str().unwrap_or(profile.label()).into();
            device.port = entry["port"].as_str().unwrap_or("").into();
            device.port_id = entry["port_id"].as_str().map(str::to_owned);
            device.exact = entry["exact"].as_bool().unwrap_or(true);
            device.enabled = entry["enabled"].as_bool().unwrap_or(false);
            device.channel = serde_json::from_value(entry["channel"].clone())
                .map_err(|e| format!("channel: {e}"))?;
            if device.channel.is_some_and(|n| !(1..=16).contains(&n)) {
                return Err("MIDI channel must be 1–16".into());
            }
            device.mapping = Mapping::from_json(&entry["mapping"])?;
            result.devices.push(device);
        }
        Ok(result)
    }

    pub fn to_json(&self) -> Value {
        json!({"version": 2, "devices": self.devices.iter().map(DeviceConfig::to_json).collect::<Vec<_>>()})
    }

    /// Reads `dir`'s file; none is no device.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(FILE);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                Self::from_json(&value).map_err(|e| format!("{}: {e}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes `dir`'s file whole, so a crash leaves the old one or the new
    /// one, never a part.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let path = dir.join(FILE);
        let text = serde_json::to_vec_pretty(&self.to_json()).expect("settings are JSON");
        let temp = dir.join(format!(".{FILE}.{}.tmp", std::process::id()));
        let written = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(dir)?;
            let mut file = std::fs::File::create(&temp)?;
            std::io::Write::write_all(&mut file, &text)?;
            file.sync_all()?;
            std::fs::rename(&temp, &path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written.map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Adds a device of `profile` with the lowest id not in use.
    pub fn add(&mut self, profile: Profile) -> u64 {
        let id = (1..=u64::MAX)
            .find(|id| !self.devices.iter().any(|d| d.id == *id))
            .expect("available device ID");
        self.devices.push(DeviceConfig::new(id, profile));
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_values_are_relative_or_absolute() {
        assert_eq!(
            [1, 127, 3, 125].map(|v| Encoder::TwosComplement.ticks(v)),
            [Some(1), Some(-1), Some(3), Some(-3)]
        );
        assert_eq!(Encoder::Offset.ticks(63), Some(-1));
        assert_eq!(Encoder::Offset.ticks(64), Some(0));
        assert_eq!(Encoder::SignMagnitude.ticks(65), Some(-1));
        assert_eq!(Encoder::Absolute.ticks(127), None);
        for encoder in Encoder::ALL {
            assert_eq!(serde_json::to_value(encoder).unwrap(), encoder.id());
        }
    }

    #[test]
    fn overrides_bind_and_unbind() {
        let mut mapping = Profile::Loupedeck.mapping();
        mapping.apply(&json!({
            "dials": {"41": "contrast", "33": null},
            "buttons": {"98": "cmd+shift+u", "95": null, "99": "hold:shift"}
        }));
        assert_eq!(mapping.dials.get(&41).map(String::as_str), Some("contrast"));
        assert!(!mapping.dials.contains_key(&33));
        assert_eq!(
            mapping.buttons.get(&98).map(String::as_str),
            Some("cmd+shift+u")
        );
        assert!(!mapping.buttons.contains_key(&95));
        assert_eq!(
            mapping.buttons.get(&99).map(String::as_str),
            Some("hold:shift")
        );
    }

    #[test]
    fn a_saved_mapping_reads_back_without_inheriting_removed_controls() {
        let mut mapping = Profile::Loupedeck.mapping();
        mapping.dials.insert(41, "band3.sat".into());
        mapping.dials.insert(42, "band8.gray".into());
        mapping.dials.remove(&33);
        mapping.buttons.insert(50, "cmd+shift+U".into());
        mapping.buttons.insert(110, "hold:alt".into());
        mapping.buttons.insert(111, "mixer:lum".into());
        mapping.buttons.remove(&95);
        mapping.photo_dial = None;
        mapping.encoders.insert(41, Encoder::Offset);
        let saved = mapping.to_json();
        assert_eq!(saved["dials"]["34"], "blacks");
        assert_eq!(Mapping::from_json(&saved).unwrap(), mapping);
    }

    /// Version 0 was one Loupedeck+ with overrides at the top level; it
    /// becomes one device matching its port by name.
    #[test]
    fn migration_preserves_legacy_overrides_and_custom_devices_are_empty() {
        let legacy = json!({"port": "My Deck", "socket": true,
            "dials": {"33": null, "41": "texture"}, "buttons": {"95": null}});
        let mut settings = Settings::from_json(&legacy).unwrap();
        let device = &settings.devices[0];
        assert_eq!((device.id, device.profile), (1, Profile::Loupedeck));
        assert_eq!(device.port, "My Deck");
        assert!(!device.exact);
        assert!(!device.mapping.dials.contains_key(&33));
        assert_eq!(
            device.mapping.dials.get(&41).map(String::as_str),
            Some("texture")
        );
        assert!(!device.mapping.buttons.contains_key(&95));
        assert_eq!(
            device.mapping.buttons.get(&96).map(String::as_str),
            Some("cmd+shift+Z")
        );
        let id = settings.add(Profile::Custom);
        let custom = settings.devices.iter_mut().find(|d| d.id == id).unwrap();
        assert!(custom.mapping.dials.is_empty() && custom.mapping.buttons.is_empty());
        assert!(custom.port.is_empty());
        custom.port = "Other device".into();
        custom.mapping.dials.insert(11, "exposure".into());
        custom.mapping.encoders.insert(11, Encoder::Offset);
        let saved = settings.to_json();
        assert!(
            saved.get("socket").is_none(),
            "the socket's switch is the core's"
        );
        assert_eq!(Settings::from_json(&saved).unwrap(), settings);
        assert!(Settings::default().devices.is_empty());
    }

    #[test]
    fn newer_or_damaged_files_are_refused() {
        assert!(Settings::from_json(&json!({"version": 99, "devices": []})).is_err());
        assert!(Settings::from_json(&json!({"version": 2})).is_err());
        let duplicate = json!({"version": 2, "devices": [
            {"id": 1, "profile": "custom", "mapping": {"default_encoder": "absolute", "encoders": {}}},
            {"id": 1, "profile": "custom", "mapping": {"default_encoder": "absolute", "encoders": {}}}]});
        assert!(Settings::from_json(&duplicate).is_err());
        let channel = json!({"version": 2, "devices": [{"id": 1, "profile": "custom",
            "channel": 17, "mapping": {"default_encoder": "absolute", "encoders": {}}}]});
        assert!(Settings::from_json(&channel).is_err());
    }

    #[test]
    fn the_file_is_written_whole_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()).unwrap(), Settings::default());
        let mut settings = Settings::default();
        settings.add(Profile::Loupedeck);
        settings.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()).unwrap(), settings);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE], "no temporary file is left");
        std::fs::write(dir.path().join(FILE), b"{").unwrap();
        assert!(Settings::load(dir.path()).unwrap_err().contains(FILE));
    }
}
