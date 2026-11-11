//! Whether local scripts and AI clients may reach the editor: the control
//! socket's switch, in `automation.json`. MIDI devices are the bundled MIDI
//! extension's, in its own data folder.
use super::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Settings {
    pub socket: bool,
}
impl Settings {
    /// Reads the switch. Before `automation.json` existed, it was the
    /// `socket` field of a version 0 `midi.json`, which still counts while
    /// `automation.json` is absent. A file that isn't JSON is an error, so
    /// the page edits nothing until it is fixed.
    pub(crate) fn load() -> anyhow::Result<Self> {
        let dir = crate::storage::data_dir();
        let mut config = match std::fs::read(dir.join("midi.json")) {
            Ok(bytes) => {
                let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                Self::legacy(&value)
            }
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
    /// The switch a `midi.json` holds: only a version 0 file had one.
    fn legacy(midi: &serde_json::Value) -> Self {
        Self {
            socket: midi.get("version").is_none() && midi["socket"].as_bool().unwrap_or(false),
        }
    }
    pub(crate) fn save(&self) -> anyhow::Result<()> {
        crate::storage::atomic_json(
            &crate::storage::data_dir().join("automation.json"),
            &serde_json::json!({"protocol":commands::PROTOCOL,"socket":self.socket}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_version_0_midi_file_switches_the_socket_on() {
        let on = serde_json::json!({"port": "Loupedeck", "socket": true});
        assert!(Settings::legacy(&on).socket);
        assert!(!Settings::legacy(&serde_json::json!({"port": "Loupedeck"})).socket);
        let v2 = serde_json::json!({"version": 2, "devices": [], "socket": true});
        assert!(!Settings::legacy(&v2).socket);
    }
}
