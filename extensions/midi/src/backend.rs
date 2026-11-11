//! Where MIDI comes from: the system's inputs through midir on macOS and
//! Windows, none on Linux (midir would need ALSA's headers there), or a test's
//! own [`Backend`] that feeds bytes in.
use crate::midi::Port;
use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

/// Called with each MIDI message an input sends, on the backend's thread.
pub type Receive = Box<dyn FnMut(&[u8]) + Send>;

/// A source of MIDI inputs.
pub trait Backend: Send + Sync + 'static {
    /// Whether this platform has MIDI input at all; the page says so when it
    /// hasn't.
    fn available(&self) -> bool {
        true
    }
    /// The inputs there are now.
    fn ports(&self) -> Vec<Port>;
    /// Opens `port`, calling `receive` with each message until the returned
    /// connection is dropped. Dropping it stops the calls before it returns.
    fn connect(&self, port: &Port, receive: Receive) -> Result<Box<dyn Any>, String>;
    /// How often a device's thread looks for its input coming and going: no
    /// portable event announces a device plugged in.
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(2)
    }
}

/// The system's MIDI inputs.
pub fn system() -> Arc<dyn Backend> {
    Arc::new(platform::System)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod platform {
    use super::*;
    use midir::MidiInput;

    pub(super) struct System;
    impl Backend for System {
        fn ports(&self) -> Vec<Port> {
            let Ok(input) = MidiInput::new("RAWmakase devices") else {
                return Vec::new();
            };
            input
                .ports()
                .iter()
                .map(|p| Port {
                    id: p.id(),
                    name: input.port_name(p).unwrap_or_default(),
                })
                .collect()
        }
        fn connect(&self, port: &Port, mut receive: Receive) -> Result<Box<dyn Any>, String> {
            let input = MidiInput::new("RAWmakase").map_err(|e| e.to_string())?;
            let found = input
                .ports()
                .into_iter()
                .find(|p| p.id() == port.id)
                .ok_or("The MIDI input is gone")?;
            let connection = input
                .connect(
                    &found,
                    "RAWmakase input",
                    move |_, bytes, _| receive(bytes),
                    (),
                )
                .map_err(|e| e.to_string())?;
            Ok(Box::new(connection))
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::*;

    pub(super) struct System;
    impl Backend for System {
        fn available(&self) -> bool {
            false
        }
        fn ports(&self) -> Vec<Port> {
            Vec::new()
        }
        fn connect(&self, _: &Port, _: Receive) -> Result<Box<dyn Any>, String> {
            Err("MIDI input isn't supported on this platform".into())
        }
    }
}
