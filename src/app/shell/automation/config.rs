//! Persist device instances separately from the local automation endpoint.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Encoder {
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
    pub(crate) fn ticks(self, value: u8) -> Option<i32> {
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
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Config {
    pub dials: HashMap<u8, Param>,
    pub buttons: HashMap<u8, Action>,
    pub photo_dial: Option<u8>,
    pub photo_detent: i32,
    pub default_encoder: Encoder,
    pub encoders: HashMap<u8, Encoder>,
    pub sensitivity: i32,
}
impl Config {
    #[cfg(test)]
    pub(crate) fn defaults() -> Self {
        Profile::Loupedeck.mapping()
    }
    pub(crate) fn encoder(&self, cc: u8) -> Encoder {
        self.encoders
            .get(&cc)
            .copied()
            .unwrap_or(self.default_encoder)
    }
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let dials: std::collections::BTreeMap<_, _> = self
            .dials
            .iter()
            .map(|(n, p)| (n.to_string(), p.spec()))
            .collect();
        let buttons: std::collections::BTreeMap<_, _> = self
            .buttons
            .iter()
            .map(|(n, a)| (n.to_string(), action_spec(*a)))
            .collect();
        serde_json::json!({"dials":dials,"buttons":buttons,"photo_dial":self.photo_dial,"photo_detent":self.photo_detent,"default_encoder":self.default_encoder,"encoders":self.encoders,"sensitivity":self.sensitivity})
    }
    pub(crate) fn from_json(value: &serde_json::Value) -> anyhow::Result<Self> {
        let mut config = Profile::Custom.mapping();
        config.apply(value);
        config.default_encoder = serde_json::from_value(value["default_encoder"].clone())?;
        config.encoders = serde_json::from_value(value["encoders"].clone())?;
        anyhow::ensure!(
            config.encoders.keys().all(|n| *n <= 127),
            "Invalid control number"
        );
        config.sensitivity = value["sensitivity"].as_i64().unwrap_or(1).clamp(1, 16) as i32;
        Ok(config)
    }
    pub(super) fn apply(&mut self, json: &serde_json::Value) {
        if let Some(dial) = json.get("photo_dial") {
            self.photo_dial = dial.as_u64().and_then(|n| u8::try_from(n).ok());
        }
        if let Some(n) = json["photo_detent"].as_i64() {
            self.photo_detent = n.clamp(1, 64) as i32;
        }
        for (number, value) in json["dials"].as_object().into_iter().flatten() {
            let Ok(cc @ 0..=127) = number.parse::<u8>() else {
                continue;
            };
            match value.as_str().map(Param::parse) {
                Some(Some(param)) => self.dials.insert(cc, param),
                _ => self.dials.remove(&cc),
            };
        }
        for (number, value) in json["buttons"].as_object().into_iter().flatten() {
            let Ok(note @ 0..=127) = number.parse::<u8>() else {
                continue;
            };
            match value.as_str().map(parse_action) {
                Some(Some(action)) => self.buttons.insert(note, action),
                _ => self.buttons.remove(&note),
            };
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DeviceConfig {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    pub profile: Profile,
    pub port: String,
    pub port_id: Option<String>,
    pub exact: bool,
    pub channel: Option<u8>,
    pub mapping: Config,
}
impl DeviceConfig {
    pub(crate) fn new(id: u64, profile: Profile) -> Self {
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
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"id":self.id,"name":self.name,"enabled":self.enabled,"profile":self.profile,"port":self.port,"port_id":self.port_id,"exact":self.exact,"channel":self.channel,"mapping":self.mapping.to_json()})
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Settings {
    pub devices: Vec<DeviceConfig>,
    pub socket: bool,
}
impl Settings {
    pub(crate) fn from_json(value: &serde_json::Value) -> anyhow::Result<Self> {
        let mut result = Self::default();
        if value.get("version").is_none() {
            let mut device = DeviceConfig::new(1, Profile::Loupedeck);
            device.mapping.apply(value);
            device.port = value["port"].as_str().unwrap_or("Loupedeck").into();
            device.exact = false;
            result.devices.push(device);
            result.socket = value["socket"].as_bool().unwrap_or(false);
            return Ok(result);
        }
        anyhow::ensure!(
            value["version"] == 2,
            "Unsupported MIDI configuration version"
        );
        for entry in value["devices"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Missing devices"))?
        {
            let id = entry["id"]
                .as_u64()
                .filter(|id| *id > 0)
                .ok_or_else(|| anyhow::anyhow!("Invalid device ID"))?;
            anyhow::ensure!(
                !result.devices.iter().any(|d| d.id == id),
                "Duplicate device ID"
            );
            let profile = serde_json::from_value(entry["profile"].clone())?;
            let mut device = DeviceConfig::new(id, profile);
            device.name = entry["name"].as_str().unwrap_or(profile.label()).into();
            device.port = entry["port"].as_str().unwrap_or("").into();
            device.port_id = entry["port_id"].as_str().map(str::to_owned);
            device.exact = entry["exact"].as_bool().unwrap_or(true);
            device.enabled = entry["enabled"].as_bool().unwrap_or(false);
            device.channel = serde_json::from_value(entry["channel"].clone())?;
            anyhow::ensure!(
                device.channel.is_none_or(|n| (1..=16).contains(&n)),
                "MIDI channel must be 1–16"
            );
            device.mapping = Config::from_json(&entry["mapping"])?;
            result.devices.push(device);
        }
        Ok(result)
    }
    pub(crate) fn load() -> anyhow::Result<Self> {
        let dir = crate::storage::data_dir();
        let mut config = match std::fs::read(dir.join("midi.json")) {
            Ok(bytes) => Self::from_json(&serde_json::from_slice(&bytes)?)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        match std::fs::read(dir.join("automation.json")) {
            Ok(bytes) => {
                let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                config.socket = value["protocol"] == commands::PROTOCOL
                    && value["socket"].as_bool().unwrap_or(false);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(config)
    }
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"version":2,"devices":self.devices.iter().map(DeviceConfig::to_json).collect::<Vec<_>>()})
    }
    pub(crate) fn save(&self) -> anyhow::Result<()> {
        let dir = crate::storage::data_dir();
        crate::storage::atomic_json(&dir.join("midi.json"), &self.to_json())?;
        crate::storage::atomic_json(
            &dir.join("automation.json"),
            &serde_json::json!({"protocol":commands::PROTOCOL,"socket":self.socket}),
        )
    }
    pub(crate) fn add(&mut self, profile: Profile) -> u64 {
        let id = (1..=u64::MAX)
            .find(|id| !self.devices.iter().any(|d| d.id == *id))
            .expect("available device ID");
        self.devices.push(DeviceConfig::new(id, profile));
        id
    }
}
