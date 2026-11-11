//! MIDI messages and inputs: what a device sends, read into [`Input`], and
//! which of the system's inputs a device's binding names. No mapping lives
//! here.
use crate::config::DeviceConfig;

/// A control change or a note, as a device sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// Control `.0` sent value `.1` (0–127).
    Cc(u8, u8),
    /// Note `.0` went down (`true`) or up.
    Note(u8, bool),
}

/// Reads one MIDI message. With `channel` (1–16), messages on other channels
/// are ignored. A note-on with velocity 0 is a note-off, as MIDI defines it.
pub fn parse(bytes: &[u8], channel: Option<u8>) -> Option<Input> {
    if channel.is_some_and(|c| bytes.first().is_none_or(|s| (s & 15) + 1 != c)) {
        return None;
    }
    match *bytes {
        [status, d1, d2] if status & 0xF0 == 0xB0 => Some(Input::Cc(d1, d2)),
        [status, d1, d2] if status & 0xF0 == 0x90 && d2 > 0 => Some(Input::Note(d1, true)),
        [status, d1, _] if status & 0xF0 == 0x80 => Some(Input::Note(d1, false)),
        [status, d1, 0] if status & 0xF0 == 0x90 => Some(Input::Note(d1, false)),
        _ => None,
    }
}

/// The latest message from a device, for its page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Last {
    Dial(u8, u8),
    Button(u8),
}

/// One of the system's MIDI inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    /// The backend's id for it, which tells two inputs of the same name apart.
    pub id: String,
    /// The name it shows.
    pub name: String,
}

/// The input `binding` names among `available`: by id when one was chosen
/// from the list, else by name, whole or in part. No input is no error; more
/// than one matching a name is, so the user picks one.
pub(crate) fn select_port(
    binding: &DeviceConfig,
    available: &[Port],
) -> Result<Option<String>, String> {
    if let Some(id) = &binding.port_id {
        return Ok(available.iter().find(|p| &p.id == id).map(|p| p.id.clone()));
    }
    if binding.port.trim().is_empty() {
        return Ok(None);
    }
    let candidates: Vec<_> = available
        .iter()
        .filter(|p| {
            if binding.exact {
                p.name == binding.port
            } else {
                p.name.contains(&binding.port)
            }
        })
        .collect();
    match candidates.as_slice() {
        [] => Ok(None),
        [port] => Ok(Some(port.id.clone())),
        _ => Err("Multiple ports match; choose a specific MIDI input".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::Profile;

    #[test]
    fn parses_device_messages() {
        assert_eq!(parse(&[0xB0, 48, 1], None), Some(Input::Cc(48, 1)));
        assert_eq!(parse(&[0x90, 68, 64], None), Some(Input::Note(68, true)));
        assert_eq!(parse(&[0x80, 68, 64], None), Some(Input::Note(68, false)));
        assert_eq!(parse(&[0x90, 68, 0], None), Some(Input::Note(68, false)));
        assert_eq!(parse(&[0xF8], None), None);
    }

    #[test]
    fn a_channel_filters_messages() {
        assert_eq!(parse(&[0xB1, 12, 127], Some(1)), None);
        assert_eq!(parse(&[0xB1, 12, 127], Some(2)), Some(Input::Cc(12, 127)));
    }

    #[test]
    fn port_selection_requires_an_unambiguous_binding() {
        let ports = vec![
            Port {
                id: "a".into(),
                name: "Controller".into(),
            },
            Port {
                id: "b".into(),
                name: "Controller".into(),
            },
        ];
        let mut binding = DeviceConfig::new(1, Profile::Custom);
        assert_eq!(select_port(&binding, &ports).unwrap(), None);
        binding.port = "Controller".into();
        assert!(select_port(&binding, &ports).is_err());
        binding.port_id = Some("b".into());
        assert_eq!(select_port(&binding, &ports).unwrap(), Some("b".into()));
        assert_eq!(
            select_port(&binding, &ports[..1]).unwrap(),
            None,
            "Do not switch to another identical controller after disconnect"
        );
        // A name in part, as a migrated Loupedeck binding has it.
        binding.port_id = None;
        binding.port = "Contr".into();
        binding.exact = false;
        assert_eq!(
            select_port(&binding, &ports[..1]).unwrap(),
            Some("a".into())
        );
    }
}
