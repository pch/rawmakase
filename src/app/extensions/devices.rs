//! The devices of one program start's control adapters, as its
//! `adapter.device` requests report them.
//!
//! The session's reader thread applies those requests to this table itself,
//! the moment it reads them, outside the bounded queue of operations
//! (`rawmakase_extensions::session::Lifecycle`), so a device's disconnection
//! is never held up behind its own burst of input. The editor reads the table
//! just before it runs each operation (`Editor::device_operations`): an
//! operation runs only in its device's current epoch while it is connected.
use rawmakase_extensions::protocol::method::{AdapterDevice, DeviceState};
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use std::collections::HashMap;
use std::sync::Mutex;

/// One program start's devices, shared by its reader thread and the editor.
#[derive(Default)]
pub(in crate::app) struct Devices(Mutex<Table>);

#[derive(Default)]
struct Table {
    /// By adapter and the program's id for the device.
    devices: HashMap<(String, String), Device>,
    next: u64,
}

struct Device {
    /// The host's number for it, unique in this program start: a gesture's
    /// source names it.
    number: u64,
    /// The latest epoch reported; epochs only count up.
    epoch: u64,
    connected: bool,
}

impl Devices {
    /// Takes a device's lifecycle in. A connection must bring a new epoch,
    /// higher than any before it, so no operation of an earlier connection
    /// is taken for one of it. The end of an earlier connection than the
    /// latest (a stopped thread's report arriving after its successor's
    /// connection) changes nothing.
    pub(in crate::app) fn apply(&self, report: &AdapterDevice) -> Result<(), RemoteError> {
        let mut table = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Table { devices, next } = &mut *table;
        let key = (report.adapter.clone(), report.device.clone());
        let device = devices.entry(key).or_insert_with(|| {
            *next += 1;
            Device {
                number: *next,
                epoch: 0,
                connected: false,
            }
        });
        match report.state {
            DeviceState::Connected if report.epoch <= device.epoch => Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "{} connected in epoch {}, which isn't after its epoch {}",
                    report.device, report.epoch, device.epoch
                ),
            )),
            DeviceState::Connected => {
                device.epoch = report.epoch;
                device.connected = true;
                Ok(())
            }
            DeviceState::Disconnected | DeviceState::Removed => {
                if report.epoch >= device.epoch {
                    device.epoch = report.epoch;
                    device.connected = false;
                }
                Ok(())
            }
        }
    }

    /// The host's number for `device` of `adapter`, when `epoch` is its
    /// current connection.
    pub(in crate::app) fn current(&self, adapter: &str, device: &str, epoch: u64) -> Option<u64> {
        let table = self.0.lock().unwrap_or_else(|e| e.into_inner());
        table
            .devices
            .get(&(adapter.to_string(), device.to_string()))
            .filter(|d| d.connected && d.epoch == epoch)
            .map(|d| d.number)
    }

    /// Whether device `number`'s current connection is `epoch`.
    pub(in crate::app) fn is_current(&self, number: u64, epoch: u64) -> bool {
        let table = self.0.lock().unwrap_or_else(|e| e.into_inner());
        table
            .devices
            .values()
            .any(|d| d.number == number && d.connected && d.epoch == epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(device: &str, state: DeviceState, epoch: u64) -> AdapterDevice {
        AdapterDevice {
            adapter: "midi".into(),
            device: device.into(),
            state,
            epoch,
        }
    }

    #[test]
    fn only_the_current_connection_is_admitted() {
        let devices = Devices::default();
        assert_eq!(devices.current("midi", "1", 1), None);
        devices
            .apply(&report("1", DeviceState::Connected, 1))
            .unwrap();
        devices
            .apply(&report("2", DeviceState::Connected, 1))
            .unwrap();
        let one = devices.current("midi", "1", 1).unwrap();
        let two = devices.current("midi", "2", 1).unwrap();
        assert_ne!(one, two, "each device has a number of its own");
        assert!(devices.is_current(one, 1));
        devices
            .apply(&report("1", DeviceState::Disconnected, 1))
            .unwrap();
        assert_eq!(devices.current("midi", "1", 1), None);
        assert!(!devices.is_current(one, 1));
        // The same epoch never connects again.
        assert!(
            devices
                .apply(&report("1", DeviceState::Connected, 1))
                .is_err()
        );
        devices
            .apply(&report("1", DeviceState::Connected, 2))
            .unwrap();
        assert_eq!(devices.current("midi", "1", 2), Some(one));
        assert_eq!(devices.current("midi", "1", 1), None);
        // An earlier connection's end, reported late, leaves this one.
        devices
            .apply(&report("1", DeviceState::Disconnected, 1))
            .unwrap();
        assert_eq!(devices.current("midi", "1", 2), Some(one));
        devices
            .apply(&report("1", DeviceState::Removed, 2))
            .unwrap();
        assert_eq!(devices.current("midi", "1", 2), None);
        assert_eq!(devices.current("midi", "2", 1), Some(two));
        assert_eq!(devices.current("other", "2", 1), None);
    }
}
