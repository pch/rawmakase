//! The program through the SDK's mock host, with a backend whose inputs the
//! test plugs in, unplugs and sends bytes from.
use super::*;
use rawmakase_extension::mock::{MockHost, Recorded};
use rawmakase_extension::protocol::PROTOCOL;
use rawmakase_extension::protocol::controls::{Control, ControlKind};
use rawmakase_extension::protocol::method::Hello;
use serde_json::{Map, Value, json};
use std::path::Path;

/// Inputs a test controls.
#[derive(Default)]
pub(crate) struct Fake {
    ports: Mutex<Vec<Port>>,
    connected: Mutex<HashMap<String, Receive>>,
}
type Receive = Arc<Mutex<backend::Receive>>;

/// Ends a fake connection: its messages stop.
struct Connection(Arc<Fake>, String);
impl Drop for Connection {
    fn drop(&mut self) {
        lock(&self.0.connected).remove(&self.1);
    }
}

/// [`Fake`] behind an `Arc`, which its connections keep.
#[derive(Clone, Default)]
pub(crate) struct Inputs(Arc<Fake>);
impl Backend for Inputs {
    fn ports(&self) -> Vec<Port> {
        lock(&self.0.ports).clone()
    }
    fn connect(&self, port: &Port, receive: backend::Receive) -> Result<Box<dyn Any>, String> {
        lock(&self.0.connected).insert(port.id.clone(), Arc::new(Mutex::new(receive)));
        Ok(Box::new(Connection(self.0.clone(), port.id.clone())))
    }
    fn poll_interval(&self) -> Duration {
        Duration::from_millis(5)
    }
}
impl Inputs {
    fn plug(&self, id: &str) {
        lock(&self.0.ports).push(Port {
            id: id.into(),
            name: format!("Deck {id}"),
        });
    }
    fn unplug(&self, id: &str) {
        lock(&self.0.ports).retain(|p| p.id != id);
    }
    /// Waits until `id` is connected, then sends it `bytes`.
    fn send(&self, id: &str, bytes: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let receive = loop {
            if let Some(r) = lock(&self.0.connected).get(id).cloned() {
                break r;
            }
            assert!(Instant::now() < deadline, "{id} never connected");
            std::thread::sleep(Duration::from_millis(2));
        };
        (lock(&receive))(bytes);
    }
    fn connected(&self, id: &str) -> bool {
        lock(&self.0.connected).contains_key(id)
    }
}

/// Answers the program as the app would: the actions it knows, and whether a
/// typed action is one (anything but `nonsense`).
fn app(method: &str, params: &Value) -> Result<Value, RemoteError> {
    Ok(match method {
        "read.capabilities" => json!({"actions": ["undo", "redo", "zoom"]}),
        "read.action" => json!({"supported": params["spec"] != "nonsense"}),
        "adapter.operate" => json!({"applied": params["ops"].as_array().map_or(0, Vec::len)}),
        _ => Value::Null,
    })
}

/// The program on `inputs`, its data in `dir`, after its handshake.
fn start(inputs: &Inputs, dir: &Path) -> MockHost {
    let host = MockHost::start_with(extension_with(Arc::new(inputs.clone())), app);
    host.hello_with(Hello {
        protocol: PROTOCOL,
        app_version: "0.0.0".into(),
        extension: "rawmakase.midi".into(),
        settings: Map::new(),
        contributions: Vec::new(),
        data_dir: dir.to_path_buf(),
        cache_dir: dir.join("cache"),
        session: 1,
    })
    .unwrap();
    host
}

/// Two Loupedeck+ devices listening to `a` and `b`.
fn two_decks(dir: &Path) {
    let mut settings = Settings::default();
    for port in ["a", "b"] {
        let id = settings.add(profiles::Profile::Loupedeck);
        let device = settings.devices.iter_mut().find(|d| d.id == id).unwrap();
        device.port = format!("Deck {port}");
    }
    settings.save(dir).unwrap();
}

/// The requests of `method` the program sent, by their parameters.
fn sent(recorded: &[Recorded], method: &str) -> Vec<Value> {
    recorded
        .iter()
        .filter_map(|r| match r {
            Recorded::Request {
                method: m, params, ..
            } if m == method => Some(params.clone()),
            _ => None,
        })
        .collect()
}

/// Every operation sent, with its device and epoch.
fn operations(recorded: &[Recorded]) -> Vec<(String, u64, Value)> {
    sent(recorded, "adapter.operate")
        .into_iter()
        .flat_map(|p| {
            let (device, epoch) = (
                p["device"].as_str().unwrap().to_string(),
                p["epoch"].as_u64().unwrap(),
            );
            p["ops"]
                .as_array()
                .unwrap()
                .clone()
                .into_iter()
                .map(move |op| (device.clone(), epoch, op))
        })
        .collect()
}

fn wait_ops(host: &MockHost, n: usize) -> Vec<(String, u64, Value)> {
    operations(&host.wait_for(|r| operations(r).len() >= n))
}

#[test]
fn each_device_translates_its_own_input_with_its_own_modifiers() {
    let dir = tempfile::tempdir().unwrap();
    two_decks(dir.path());
    let inputs = Inputs::default();
    inputs.plug("a");
    inputs.plug("b");
    let host = start(&inputs, dir.path());
    // Shift held on one deck changes that deck's Undo alone.
    inputs.send("a", &[0x90, 66, 127]);
    inputs.send("a", &[0x90, 95, 127]);
    inputs.send("b", &[0x90, 95, 127]);
    // A dial turns by relative ticks; the photo dial sends its own with the
    // detent, which the app counts.
    inputs.send("b", &[0xB0, 33, 127]);
    inputs.send("b", &[0xB0, 48, 1]);
    // A named action takes no modifiers, but Shift still moves on.
    inputs.send("a", &[0x90, 95, 0]);
    let ops = wait_ops(&host, 4);
    let of = |device: &str| -> Vec<Value> {
        ops.iter()
            .filter(|(d, ..)| d == device)
            .map(|(_, _, op)| op.clone())
            .collect()
    };
    assert_eq!(
        of("1"),
        [json!({"op": "action", "spec": "cmd+shift+Z", "advance": true})]
    );
    assert_eq!(
        of("2"),
        [
            json!({"op": "action", "spec": "cmd+Z", "advance": false}),
            json!({"op": "adjust", "param": "exposure", "ticks": -1}),
            json!({"op": "photo", "ticks": 1, "detent": 2}),
        ]
    );
    let devices = sent(&host.recorded(), "adapter.device");
    for device in ["1", "2"] {
        assert!(
            devices
                .iter()
                .any(|d| d["device"] == device && d["state"] == "connected" && d["epoch"] == 1),
            "{devices:?}"
        );
    }
    assert!(ops.iter().all(|(_, epoch, _)| *epoch == 1));
    host.finish().unwrap();
}

#[test]
fn a_reconnection_is_a_new_epoch_without_the_old_modifiers() {
    let dir = tempfile::tempdir().unwrap();
    two_decks(dir.path());
    let inputs = Inputs::default();
    inputs.plug("a");
    let host = start(&inputs, dir.path());
    inputs.send("a", &[0x90, 66, 127]);
    inputs.unplug("a");
    let recorded = host.wait_for(|r| {
        sent(r, "adapter.device")
            .iter()
            .any(|d| d["state"] == "disconnected")
    });
    let devices = sent(&recorded, "adapter.device");
    assert_eq!(
        devices.last().unwrap(),
        &json!({"adapter": "midi", "device": "1", "state": "disconnected", "epoch": 1})
    );
    assert!(!inputs.connected("a"));
    inputs.plug("a");
    inputs.send("a", &[0x90, 95, 127]);
    let ops = wait_ops(&host, 1);
    assert_eq!(
        ops,
        [(
            "1".to_string(),
            2,
            json!({"op": "action", "spec": "cmd+Z", "advance": false})
        )]
    );
    host.finish().unwrap();
}

#[test]
fn absolute_dials_send_positions_and_a_photo_dial_needs_relative_ticks() {
    let mut mapping = profiles::Profile::Custom.mapping();
    mapping.dials.insert(12, "exposure".into());
    mapping.photo_dial = Some(13);
    mapping.sensitivity = 3;
    let mut held = Mods::default();
    let none = |_: &str| false;
    assert_eq!(
        translate(&mapping, &mut held, Input::Cc(12, 127), &none),
        Ok(Some(Operation::Value {
            param: "exposure".into(),
            value: 127
        }))
    );
    assert!(translate(&mapping, &mut held, Input::Cc(13, 1), &none).is_err());
    mapping.encoders.insert(12, config::Encoder::Offset);
    assert_eq!(
        translate(&mapping, &mut held, Input::Cc(12, 63), &none),
        Ok(Some(Operation::Adjust {
            param: "exposure".into(),
            ticks: -3
        }))
    );
    assert_eq!(
        translate(&mapping, &mut held, Input::Cc(14, 1), &none),
        Ok(None)
    );
    assert_eq!(
        translate(&mapping, &mut held, Input::Note(1, true), &none),
        Ok(None)
    );
}

/// The page adds a device and saves it, checks a typed action with the app,
/// and keeps one it doesn't run out of the file.
#[test]
fn the_page_edits_and_saves_devices() {
    let dir = tempfile::tempdir().unwrap();
    let inputs = Inputs::default();
    inputs.plug("a");
    let host = start(&inputs, dir.path());
    let controls = host.open_panel(PANEL).unwrap();
    assert!(find(&controls, "add-loupedeck").is_some());
    let controls = host.click(PANEL, "add-loupedeck").unwrap();
    assert!(find(&controls, "name").is_some());
    let saved = Settings::load(dir.path()).unwrap();
    assert_eq!(saved.devices.len(), 1);
    // The input chosen from the list, which then connects.
    host.change(PANEL, "port", json!("id:a")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !inputs.connected("a") {
        assert!(Instant::now() < deadline, "the device never connected");
        std::thread::sleep(Duration::from_millis(2));
    }
    let controls = host.change(PANEL, "keys:95", json!("nonsense")).unwrap();
    let keys = find(&controls, "keys:95").unwrap();
    assert_eq!(keys.help.as_deref(), Some(page::NO_ACTION));
    assert_eq!(
        Settings::load(dir.path()).unwrap().devices[0]
            .mapping
            .buttons[&95],
        "cmd+Z"
    );
    host.change(PANEL, "keys:95", json!("redo")).unwrap();
    assert_eq!(
        Settings::load(dir.path()).unwrap().devices[0]
            .mapping
            .buttons[&95],
        "redo"
    );
    host.change(PANEL, "dial:41", json!("param:texture"))
        .unwrap();
    host.change(PANEL, "button:110", json!("preset:1")).unwrap();
    let saved = Settings::load(dir.path()).unwrap();
    let device = &saved.devices[0];
    assert_eq!(device.port_id.as_deref(), Some("a"));
    assert_eq!(device.mapping.dials[&41], "texture");
    assert_eq!(device.mapping.buttons[&110], "cmd+z");
    host.click(PANEL, "remove").unwrap();
    assert!(Settings::load(dir.path()).unwrap().devices.is_empty());
    let removed = host.wait_for(|r| {
        sent(r, "adapter.device")
            .iter()
            .any(|d| d["state"] == "removed")
    });
    assert!(
        sent(&removed, "adapter.device")
            .iter()
            .any(|d| d["state"] == "removed" && d["device"] == "1")
    );
    host.finish().unwrap();
}

/// Devices saved before a restart are edited without choosing one first: the
/// page edits the device it shows, the first until another is chosen, and the
/// one left when the shown one is removed.
#[test]
fn the_page_edits_saved_devices_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    two_decks(dir.path());
    let host = start(&Inputs::default(), dir.path());
    host.open_panel(PANEL).unwrap();
    host.change(PANEL, "name", json!("Left deck")).unwrap();
    let names = |dir: &Path| -> Vec<String> {
        let saved = Settings::load(dir).unwrap();
        saved.devices.into_iter().map(|d| d.name).collect()
    };
    assert_eq!(names(dir.path())[0], "Left deck");
    host.click(PANEL, "remove").unwrap();
    assert_eq!(names(dir.path()).len(), 1);
    host.change(PANEL, "name", json!("Only deck")).unwrap();
    assert_eq!(names(dir.path()), ["Only deck"]);
    host.change(PANEL, "enabled", json!(false)).unwrap();
    assert!(!Settings::load(dir.path()).unwrap().devices[0].enabled);
    host.click(PANEL, "remove").unwrap();
    assert!(Settings::load(dir.path()).unwrap().devices.is_empty());
    host.finish().unwrap();
}

/// Controls added and not bound yet, and text typed that isn't an action, go
/// with their device: a device added later, which may reuse its id, starts
/// without them, and so does the same device after Reset.
#[test]
fn a_removed_devices_unsaved_controls_dont_reach_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let host = start(&Inputs::default(), dir.path());
    host.open_panel(PANEL).unwrap();
    let unsaved = |host: &MockHost| {
        host.change(PANEL, "number", json!(7)).unwrap();
        host.click(PANEL, "add-dial").unwrap();
        host.change(PANEL, "number", json!(40)).unwrap();
        host.click(PANEL, "add-button").unwrap();
        let controls = host.change(PANEL, "keys:40", json!("nonsense")).unwrap();
        assert!(find(&controls, "dial:7").is_some());
        assert_eq!(
            find(&controls, "keys:40").unwrap().help.as_deref(),
            Some(page::NO_ACTION)
        );
    };
    host.click(PANEL, "add-custom").unwrap();
    unsaved(&host);
    host.click(PANEL, "remove").unwrap();
    let controls = host.click(PANEL, "add-custom").unwrap();
    assert!(find(&controls, "dial:7").is_none());
    assert!(find(&controls, "keys:40").is_none());
    unsaved(&host);
    let controls = host.click(PANEL, "reset").unwrap();
    assert!(find(&controls, "dial:7").is_none());
    assert!(find(&controls, "keys:40").is_none());
    host.finish().unwrap();
}

/// A file this program can't read, or of a newer version, is reported and
/// left as it is: the page edits nothing.
#[test]
fn an_unreadable_or_newer_file_is_reported_and_left_intact() {
    for contents in [&b"{"[..], br#"{"version": 3, "devices": []}"#] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(config::FILE), contents).unwrap();
        let inputs = Inputs::default();
        let host = start(&inputs, dir.path());
        let notified = host.wait_for(|r| !sent(r, "notify").is_empty());
        let notify = &sent(&notified, "notify")[0];
        assert_eq!(notify["level"], "warning");
        assert!(notify["text"].as_str().unwrap().contains(config::FILE));
        let controls = host.open_panel(PANEL).unwrap();
        assert!(find(&controls, "add-loupedeck").is_none());
        assert!(host.click(PANEL, "add-loupedeck").is_err());
        assert_eq!(
            std::fs::read(dir.path().join(config::FILE)).unwrap(),
            contents
        );
        host.finish().unwrap();
    }
}

/// The bundled manifest names this program, its adapter, its panel and its
/// version, which the app's version sets.
#[test]
fn the_manifest_declares_this_program_adapter_and_panel() {
    let manifest = include_str!("../rawmakase-extension.toml");
    for line in [
        "id = \"rawmakase.midi\"".to_string(),
        format!("id = \"{ADAPTER}\""),
        format!("id = \"{PANEL}\""),
        format!("version = \"{}\"", env!("CARGO_PKG_VERSION")),
        format!("path = \"{}\"", env!("CARGO_PKG_NAME")),
        "start = \"with-app\"".to_string(),
        "devices = [\"midi\"]".to_string(),
    ] {
        assert!(manifest.lines().any(|l| l == line), "{line}");
    }
}

/// A control of the tree by id.
fn find<'a>(controls: &'a [Control], id: &str) -> Option<&'a Control> {
    controls.iter().find_map(|c| {
        if c.id.as_deref() == Some(id) {
            return Some(c);
        }
        match &c.kind {
            ControlKind::Section { controls, .. } => find(controls, id),
            _ => None,
        }
    })
}
