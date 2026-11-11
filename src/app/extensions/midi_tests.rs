//! The bundled MIDI extension in the editor, through the real boundary: its
//! program (`rawmakase_midi`, run by this test binary started again, as the
//! example extension is) translates MIDI bytes the test writes to files,
//! and sends what they mean over the protocol to a test editor's host.
//!
//! The tests start few programs: one start of the program serves most of
//! what a device does, a test restarting it counts two, the Failed test
//! three. A scripted adapter speaks the protocol directly where the program
//! can't be made to (a burst past a full queue), and one test runs the
//! built `rawmakase-midi` itself, as a release finds it beside the app.
use super::{Answer, Host, Origin};
use crate::app::shell::commands::Source;
use crate::app::{Editor, Module};
use crate::catalog::PhotoId;
use eframe::egui;
use rawmakase_extensions::protocol::framing::{LineReader, write_line};
use rawmakase_extensions::protocol::method::{AdapterOperate, Operation, ProgramRequest};
use rawmakase_extensions::protocol::{ErrorCode, Message, PROTOCOL};
use rawmakase_extensions::session::Launch;
use rawmakase_midi::backend::Receive;
use rawmakase_midi::config::{Encoder, Settings};
use rawmakase_midi::midi::Port;
use rawmakase_midi::profiles::Profile;
use serde_json::{Value, json};
use std::any::Any;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const MIDI: &str = "rawmakase.midi";
/// The program's input folder, in its environment.
const INPUTS: &str = "RAWMAKASE_MIDI_TEST_INPUTS";
/// The scripted adapter's folder, in its environment.
const SCRIPT: &str = "RAWMAKASE_MIDI_TEST_SCRIPT";

/// MIDI inputs read from files in a folder: `ports` lists the inputs there
/// (one id a line, named `Deck <id>`), `<id>.in` takes what input `id` sends,
/// a message a line in hex (`b0 21 01`), from when it connects; each
/// connection adds a line to `<id>.connections` once it reads, after the
/// program has told the app of it. A line `exit` ends the program, as a crash
/// would; so does connecting while a file `crash` is there.
struct FileInputs(PathBuf);

/// Stops a file input's reader, and waits for it, so its messages stop.
struct Reading(Arc<AtomicBool>, Option<std::thread::JoinHandle<()>>);
impl Drop for Reading {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
        if let Some(thread) = self.1.take() {
            let _ = thread.join();
        }
    }
}

impl rawmakase_midi::Backend for FileInputs {
    fn ports(&self) -> Vec<Port> {
        std::fs::read_to_string(self.0.join("ports"))
            .unwrap_or_default()
            .split_whitespace()
            .map(|id| Port {
                id: id.into(),
                name: format!("Deck {id}"),
            })
            .collect()
    }
    fn connect(&self, port: &Port, mut receive: Receive) -> Result<Box<dyn Any>, String> {
        if self.0.join("crash").exists() {
            std::process::exit(3);
        }
        let path = self.0.join(format!("{}.in", port.id));
        let mut at = std::fs::metadata(&path).map_or(0, |m| m.len());
        {
            use std::io::Write;
            let mut connections = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.0.join(format!("{}.connections", port.id)))
                .map_err(|e| e.to_string())?;
            writeln!(connections, "{at}").map_err(|e| e.to_string())?;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut pending = String::new();
            while !stopped.load(Ordering::SeqCst) {
                if let Ok(mut file) = std::fs::File::open(&path)
                    && file.seek(std::io::SeekFrom::Start(at)).is_ok()
                {
                    let mut text = String::new();
                    if let Ok(n) = file.read_to_string(&mut text) {
                        at += n as u64;
                        pending.push_str(&text);
                    }
                }
                while let Some(end) = pending.find('\n') {
                    let line: String = pending.drain(..=end).collect();
                    let line = line.trim();
                    if line == "exit" {
                        std::process::exit(3);
                    }
                    let bytes: Vec<u8> = line
                        .split_whitespace()
                        .filter_map(|b| u8::from_str_radix(b, 16).ok())
                        .collect();
                    receive(&bytes);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        Ok(Box::new(Reading(stop, Some(thread))))
    }
    fn poll_interval(&self) -> Duration {
        Duration::from_millis(10)
    }
}

/// The MIDI extension's program on [`FileInputs`], when this binary is
/// started as it; a test that does nothing otherwise.
#[test]
fn midi_program() {
    let Some(dir) = std::env::var_os(INPUTS) else {
        return;
    };
    let inputs = Arc::new(FileInputs(dir.into()));
    let code = i32::from(rawmakase_midi::extension_with(inputs).run().is_err());
    std::process::exit(code);
}

fn launch(test: &str, variable: &str, dir: &Path) -> Launch {
    Launch {
        program: std::env::current_exe().expect("the test binary"),
        args: [
            &format!("app::extensions::midi_tests::{test}"),
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ]
        .map(Into::into)
        .to_vec(),
        dir: PathBuf::new(),
        env: vec![(variable.into(), dir.into())],
        log: PathBuf::new(),
    }
}

/// An editor in Develop with a small image open, and a Library of four
/// photos, the first active, in the grid.
fn editor(dir: &Path) -> (Editor, Vec<PhotoId>) {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.onboarding.visible = false;
    let metadata = crate::camera_data::Metadata {
        width: 12,
        height: 8,
        wb: [1.; 3],
        ..Default::default()
    };
    e.document.metadata = Some(metadata.clone());
    e.document
        .set_image(Arc::new(crate::camera_data::CameraImage {
            recovered: Default::default(),
            width: 12,
            height: 8,
            pixels: vec![[0.2, 0.1, 0.05]; 96],
            metadata,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }));
    let photos = dir.join("photos");
    std::fs::create_dir(&photos).unwrap();
    for name in ["a.DNG", "b.DNG", "c.DNG", "d.DNG"] {
        std::fs::write(photos.join(name), b"synthetic").unwrap();
    }
    let db = dir.join("catalog.rawmakase");
    crate::catalog::Catalog::create(&db)
        .unwrap()
        .add_folder(&photos)
        .unwrap();
    let mut library = crate::app::library::Library::load(&db, ctx).unwrap();
    let mut ids = vec![library.session.photos()[0].id];
    library.make_active(ids[0]);
    for _ in 1..4 {
        ids.push(library.navigate(*ids.last().unwrap(), 1).unwrap());
    }
    e.library = Some(Box::new(library));
    e.module = Module::Develop;
    (e, ids)
}

/// Polls the host until `done` holds, or fails after 20 s.
fn pump(e: &mut Editor, what: &str, done: impl Fn(&Editor) -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let (outcomes, _) = e.extensions_poll(Instant::now());
        e.extension_outcomes(outcomes);
        if done(e) {
            return;
        }
        assert!(
            Instant::now() < until,
            "timed out waiting for {what}; top bar: {}",
            e.messages.line()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Polls the host for `time`, for what must not happen in it.
fn pump_for(e: &mut Editor, time: Duration) {
    let until = Instant::now() + time;
    while Instant::now() < until {
        let (outcomes, _) = e.extensions_poll(Instant::now());
        e.extension_outcomes(outcomes);
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The program start's number while MIDI runs, and whether `device` is
/// connected in `epoch` in it.
fn session(e: &Editor) -> Option<u64> {
    e.extensions.get(MIDI)?.run.as_ref().map(|r| r.number)
}
fn connected(e: &Editor, device: &str, epoch: u64) -> bool {
    e.extensions
        .get(MIDI)
        .and_then(|m| m.run.as_ref())
        .is_some_and(|r| r.devices.current("midi", device, epoch).is_some())
}

fn exposure(e: &Editor) -> f32 {
    e.document.edit.recipe().exposure
}
fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Appends MIDI messages (hex) to what `input` sends.
fn send(inputs: &Path, input: &str, messages: &[&str]) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(inputs.join(format!("{input}.in")))
        .unwrap();
    for message in messages {
        writeln!(file, "{message}").unwrap();
    }
}
/// How many times `input` has connected and begun reading.
fn connections(inputs: &Path, input: &str) -> usize {
    std::fs::read_to_string(inputs.join(format!("{input}.connections")))
        .map_or(0, |t| t.lines().count())
}
/// Makes `ports` the inputs there are, at once: a program looking meanwhile
/// sees the old list or the new one, never an empty file.
fn plug(inputs: &Path, ports: &[&str]) {
    let temp = inputs.join("ports.new");
    std::fs::write(&temp, ports.join("\n")).unwrap();
    std::fs::rename(temp, inputs.join("ports")).unwrap();
}

/// Two Loupedeck+ devices on inputs `a` and `b`, the second's CC 41 an
/// absolute Exposure fader, written where the core kept them.
fn two_decks(data: &Path) -> Settings {
    let mut settings = Settings::default();
    for input in ["a", "b"] {
        let id = settings.add(Profile::Loupedeck);
        let device = settings.devices.iter_mut().find(|d| d.id == id).unwrap();
        device.port = format!("Deck {input}");
    }
    let fader = &mut settings.devices[1].mapping;
    fader.dials.insert(41, "exposure".into());
    fader.encoders.insert(41, Encoder::Absolute);
    std::fs::create_dir_all(data).unwrap();
    std::fs::write(
        data.join("midi.json"),
        serde_json::to_vec(&settings.to_json()).unwrap(),
    )
    .unwrap();
    settings
}

/// What a device does, through the program, over the protocol, into the
/// editor: its settings migrated from where the core kept them, absolute
/// faders, held modifiers per device, the photo dial's detent in the Library
/// grid and the Loupe, a disconnection with input still queued, a restart of
/// the program reusing its device ids, and disabling the extension.
#[test]
fn devices_drive_the_editor_through_the_midi_program() {
    let dir = tempfile::tempdir().unwrap();
    let (data, inputs) = (dir.path().join("data"), dir.path().join("inputs"));
    std::fs::create_dir(&inputs).unwrap();
    let settings = two_decks(&data);
    let original = std::fs::read(data.join("midi.json")).unwrap();
    plug(&inputs, &["a", "b"]);
    let (mut e, photos) = editor(dir.path());
    e.extensions = Host::with_bundled_for_tests(
        &data,
        MIDI,
        launch("midi_program", INPUTS, &inputs),
        rawmakase_extensions::session::QUEUE,
    );
    assert_eq!(e.extensions.get(MIDI).unwrap().origin, Origin::Bundled);
    pump(&mut e, "both devices", |e| {
        connected(e, "1", 1) && connected(e, "2", 1)
    });
    pump(&mut e, "both inputs read", |_| {
        connections(&inputs, "a") == 1 && connections(&inputs, "b") == 1
    });
    // The core's file was copied into the extension's folder and left.
    let copied = data.join("extension-data").join(MIDI).join("midi.json");
    assert_eq!(Settings::load(copied.parent().unwrap()).unwrap(), settings);
    assert_eq!(std::fs::read(data.join("midi.json")).unwrap(), original);

    // An absolute fader spans the slider; Undo on the other deck undoes it.
    send(&inputs, "b", &["b0 29 7f"]);
    pump(&mut e, "the fader", |e| near(exposure(e), 5.));
    // Shift held on deck a leaves deck b's Undo an Undo.
    send(&inputs, "a", &["90 42 7f"]);
    send(&inputs, "b", &["90 5f 7f"]);
    pump(&mut e, "deck b's undo", |e| near(exposure(e), 0.));
    // On deck a, the same button is Redo.
    send(&inputs, "a", &["90 5f 7f", "80 42 00"]);
    pump(&mut e, "deck a's redo", |e| near(exposure(e), 5.));

    // The photo dial does nothing in the Library grid, and that tick counts
    // for nothing: the first tick in the Loupe moves, then one per detent of
    // two.
    e.module = Module::Library;
    let selected = |e: &Editor| e.library.as_ref().unwrap().selected();
    send(&inputs, "a", &["b0 30 01"]);
    pump_for(&mut e, Duration::from_millis(250));
    assert_eq!(selected(&e), Some(photos[0]), "the grid ignores the dial");
    e.library.as_mut().unwrap().open_loupe();
    send(&inputs, "a", &["b0 30 01"]);
    pump(&mut e, "the first Loupe tick", |e| {
        selected(e) == Some(photos[1])
    });
    send(&inputs, "a", &["b0 30 01", "b0 30 01", "b0 30 01"]);
    pump(&mut e, "a detent", |e| selected(e) == Some(photos[2]));
    pump_for(&mut e, Duration::from_millis(100));
    assert_eq!(selected(&e), Some(photos[2]), "half a detent moves nothing");

    // A gesture, then input queued behind a disconnection and a
    // reconnection: none of it runs, the gesture and the held Shift end.
    // Exposure is at the top of its slider: the dials turn it down.
    e.module = Module::Develop;
    let before = exposure(&e);
    send(&inputs, "a", &["b0 21 7f"]);
    pump(&mut e, "a turn", |e| exposure(e) < before);
    let turned = exposure(&e);
    assert!(matches!(
        e.automation.turn_source(),
        Some(Source::Device { .. })
    ));
    send(
        &inputs,
        "a",
        &["90 42 7f", "b0 21 7f", "b0 21 7f", "b0 21 7f"],
    );
    std::thread::sleep(Duration::from_millis(200));
    plug(&inputs, &["b"]);
    std::thread::sleep(Duration::from_millis(200));
    plug(&inputs, &["a", "b"]);
    pump(&mut e, "the reconnection", |e| connected(e, "1", 2));
    pump(&mut e, "input a read again", |_| {
        connections(&inputs, "a") == 2
    });
    assert!(
        !e.automation.has_turn(),
        "the disconnection ended the gesture"
    );
    send(&inputs, "a", &["b0 21 01"]);
    pump(&mut e, "the new connection's turn", |e| {
        near(exposure(e), before)
    });
    // Undo, not Redo: Shift was released with the old connection. The turn
    // back was a step of its own.
    send(&inputs, "a", &["90 5f 7f"]);
    pump(&mut e, "the undo", |e| near(exposure(e), turned));

    // The program ends unexpectedly mid-gesture and starts again, with the
    // same device ids and epochs: its turns are a new gesture.
    let first = session(&e).unwrap();
    send(&inputs, "b", &["b0 21 7f"]);
    pump(&mut e, "a turn", |e| exposure(e) < turned);
    let before_restart = exposure(&e);
    send(&inputs, "a", &["exit"]);
    pump(&mut e, "the restart", |e| {
        session(e).is_some_and(|s| s != first) && connected(e, "2", 1)
    });
    pump(&mut e, "input b read again", |_| {
        connections(&inputs, "b") == 2
    });
    assert!(!e.automation.has_turn(), "the crash ended the gesture");
    assert_eq!(e.extensions.get(MIDI).unwrap().crashes, 1);
    send(&inputs, "b", &["b0 21 7f"]);
    pump(&mut e, "a turn after the restart", |e| {
        exposure(e) < before_restart
    });
    // A turn that applied is useful work.
    assert_eq!(e.extensions.get(MIDI).unwrap().crashes, 0);
    send(&inputs, "b", &["90 5f 7f"]);
    pump(&mut e, "the undo", |e| near(exposure(e), before_restart));

    // Disabled, its operations are refused, even one already read, and its
    // program stops.
    let index = e.extensions.index(MIDI).unwrap();
    e.extensions.extensions[index].entry.enabled = false;
    let operate = ProgramRequest::AdapterOperate(AdapterOperate {
        adapter: "midi".into(),
        device: "2".into(),
        epoch: 1,
        ops: vec![Operation::Adjust {
            param: "exposure".into(),
            ticks: 1,
        }],
    });
    let Answer::Now(Err(refused)) =
        e.extensions
            .program_request(index, 0, operate, &mut |_| unreachable!(), &mut Vec::new())
    else {
        panic!("a disabled extension's operation is answered at once, refused");
    };
    assert_eq!(refused.code, ErrorCode::Other("adapter_disabled".into()));
    e.extensions.extensions[index].entry.enabled = true;
    e.set_extension_enabled(MIDI, false);
    assert!(session(&e).is_none());
    assert!(!e.extensions.device_current(first + 1, 1, 1));
    e.extensions.stop(Instant::now() + Duration::from_secs(2));
}

/// Three starts that each do their handshake and report a device, then
/// crash with nothing useful done, leave the extension Failed.
#[test]
fn three_starts_that_crash_after_connecting_fail_the_extension() {
    let dir = tempfile::tempdir().unwrap();
    let (data, inputs) = (dir.path().join("data"), dir.path().join("inputs"));
    std::fs::create_dir(&inputs).unwrap();
    two_decks(&data);
    plug(&inputs, &["a"]);
    std::fs::write(inputs.join("crash"), b"").unwrap();
    let (mut e, _) = editor(dir.path());
    e.extensions = Host::with_bundled_for_tests(
        &data,
        MIDI,
        launch("midi_program", INPUTS, &inputs),
        rawmakase_extensions::session::QUEUE,
    );
    pump(&mut e, "Failed", |e| e.extensions.get(MIDI).unwrap().failed);
    let midi = e.extensions.get(MIDI).unwrap();
    assert_eq!(midi.crashes, super::CRASHES);
    assert!(midi.run.is_none());
    assert_eq!(e.extensions.next_number, 4, "three starts");
    pump_for(&mut e, Duration::from_millis(50));
    assert!(
        e.extensions.get(MIDI).unwrap().run.is_none(),
        "Failed stays stopped"
    );
}

/// A scripted adapter: answers `hello`, then sends a burst of operations, a
/// disconnection and a reconnection, without waiting, and writes the replies
/// it gets to `replies.json` in its folder.
#[test]
fn scripted_adapter() {
    let Some(dir) = std::env::var_os(SCRIPT).map(PathBuf::from) else {
        return;
    };
    let channel = rawmakase_extension::channel::attach().unwrap();
    let mut writer = channel.writer;
    let mut lines = LineReader::new(channel.reader);
    let Ok(Some(Ok(Message::Req { id, .. }))) = lines.next_message() else {
        std::process::exit(2);
    };
    let hello = json!({"protocol": PROTOCOL, "implements": ["control-adapter/midi"]});
    write_line(&mut writer, &Message::reply(id, Ok(hello))).unwrap();
    let device = |state: &str, epoch: u64| json!({"adapter": "midi", "device": "1", "state": state, "epoch": epoch});
    let operate = |epoch: u64| {
        json!({"adapter": "midi", "device": "1", "epoch": epoch,
               "ops": [{"op": "adjust", "param": "exposure", "ticks": 5}]})
    };
    let mut requests = vec![(100, "adapter.device", device("connected", 1))];
    requests.extend((1..=8).map(|id| (id, "adapter.operate", operate(1))));
    requests.push((101, "adapter.device", device("disconnected", 1)));
    requests.push((102, "adapter.device", device("connected", 2)));
    requests.push((9, "adapter.operate", operate(2)));
    for (id, method, params) in &requests {
        write_line(&mut writer, &Message::request(*id, method, params.clone())).unwrap();
    }
    std::fs::write(dir.join("sent"), b"").unwrap();
    let mut replies = serde_json::Map::new();
    while replies.len() < requests.len() {
        match lines.next_message() {
            Ok(Some(Ok(Message::Ok { id, .. }))) => {
                replies.insert(id.to_string(), Value::from("ok"));
            }
            Ok(Some(Ok(Message::Err { id, code, .. }))) => {
                replies.insert(id.to_string(), serde_json::to_value(code).unwrap());
            }
            Ok(Some(_)) => {}
            _ => std::process::exit(2),
        }
    }
    std::fs::write(dir.join("replies.json"), Value::Object(replies).to_string()).unwrap();
    // Until the app ends the session.
    while let Ok(Some(_)) = lines.next_message() {}
    std::process::exit(0);
}

/// A disconnection reaches the device table while the operation queue is
/// full: the operations behind the full queue are refused `channel_full` at
/// once, those already queued are refused as stale once the editor reads
/// them, and none changes the editor.
#[test]
fn a_disconnection_overtakes_a_full_queue() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let script = dir.path().join("script");
    std::fs::create_dir(&script).unwrap();
    let (mut e, _) = editor(dir.path());
    e.extensions =
        Host::with_bundled_for_tests(&data, MIDI, launch("scripted_adapter", SCRIPT, &script), 4);
    // The second poll starts the program; then nothing is read until it has
    // sent everything.
    for _ in 0..2 {
        let (outcomes, _) = e.extensions_poll(Instant::now());
        e.extension_outcomes(outcomes);
    }
    let until = Instant::now() + Duration::from_secs(20);
    while !script.join("sent").exists() {
        assert!(Instant::now() < until, "the script never sent its burst");
        std::thread::sleep(Duration::from_millis(2));
    }
    // The reader applied both reports already, ahead of the queue.
    std::thread::sleep(Duration::from_millis(100));
    assert!(connected(&e, "1", 2));
    pump(&mut e, "every reply", |_| {
        script.join("replies.json").exists()
    });
    let replies: Value =
        serde_json::from_slice(&std::fs::read(script.join("replies.json")).unwrap()).unwrap();
    for id in ["100", "101", "102"] {
        assert_eq!(replies[id], "ok", "{id}");
    }
    // The queue held the handshake's reply and three operations.
    for id in ["1", "2", "3"] {
        assert_eq!(replies[id], "stale_target", "{id}");
    }
    for id in ["4", "5", "6", "7", "8", "9"] {
        assert_eq!(replies[id], "channel_full", "{id}");
    }
    assert_eq!(exposure(&e), 0., "no operation changed the editor");
    e.extensions.stop(Instant::now() + Duration::from_secs(2));
}

/// On its first start the extension takes the core's `midi.json` over: a
/// copy in its folder, unless one is there; the original stays. A copy that
/// fails is said in the top bar. No program runs here.
#[test]
fn the_cores_midi_settings_are_copied_once_and_left() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let missing = Launch {
        program: dir.path().join("no-such-program"),
        args: Vec::new(),
        dir: PathBuf::new(),
        env: Vec::new(),
        log: PathBuf::new(),
    };
    let start = |data: &Path| {
        let mut e = Editor::with_context(
            &egui::Context::default(),
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.extensions = Host::with_bundled_for_tests(data, MIDI, missing.clone(), 64);
        for _ in 0..2 {
            let (outcomes, _) = e.extensions_poll(Instant::now());
            e.extension_outcomes(outcomes);
        }
        let (outcomes, _) = e.extensions_poll(Instant::now());
        (e, outcomes)
    };
    let theirs = data.join("extension-data").join(MIDI).join("midi.json");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("midi.json"), b"{\"version\": 7}").unwrap();
    drop(start(&data));
    assert_eq!(std::fs::read(&theirs).unwrap(), b"{\"version\": 7}");
    assert_eq!(
        std::fs::read(data.join("midi.json")).unwrap(),
        b"{\"version\": 7}"
    );
    // Theirs is never replaced.
    std::fs::write(data.join("midi.json"), b"{}").unwrap();
    drop(start(&data));
    assert_eq!(std::fs::read(&theirs).unwrap(), b"{\"version\": 7}");
    // A copy that fails is reported.
    let other = dir.path().join("other");
    std::fs::create_dir_all(other.join("midi.json")).unwrap();
    let (e, outcomes) = start(&other);
    assert!(
        format!("{outcomes:?} {}", e.messages.line()).contains("wasn't copied"),
        "{outcomes:?}"
    );
    assert!(
        !other
            .join("extension-data")
            .join(MIDI)
            .join("midi.json")
            .exists()
    );
}

/// The built `rawmakase-midi`, found beside the test binary's folder as a
/// release finds it beside the app: it starts with the app, draws its page
/// and saves a device added there. Run `cargo build -p rawmakase-midi`
/// first, as `make check` does.
#[test]
fn the_built_program_runs_its_page() {
    let exe = std::env::current_exe().unwrap();
    let target = exe.parent().and_then(Path::parent).unwrap().to_path_buf();
    let name = rawmakase_extensions::bundled::program_name("rawmakase-midi");
    assert!(
        target.join(&name).is_file(),
        "{} is missing: build it with cargo build -p rawmakase-midi",
        target.join(&name).display()
    );
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let (mut e, _) = editor(dir.path());
    e.extensions = Host::with_search_dirs_for_tests(&data, vec![target]);
    pump(&mut e, "the program", |e| {
        e.extensions
            .get(MIDI)
            .unwrap()
            .run
            .as_ref()
            .is_some_and(|r| r.ready)
    });
    e.extensions.open_panel(MIDI, "devices").unwrap();
    pump(&mut e, "its page", |e| {
        e.extensions
            .panel(MIDI, "devices")
            .is_some_and(|p| !p.waiting && !p.controls.is_empty())
    });
    let page = format!(
        "{:?}",
        e.extensions.panel(MIDI, "devices").unwrap().controls
    );
    assert!(page.contains("add-loupedeck"), "{page}");
    if cfg!(not(any(target_os = "macos", target_os = "windows"))) {
        assert!(page.contains("supported on macOS and Windows"), "{page}");
    }
    e.extensions
        .panel_clicked(MIDI, "devices", "add-loupedeck")
        .unwrap();
    pump(&mut e, "the device", |e| {
        format!(
            "{:?}",
            e.extensions.panel(MIDI, "devices").unwrap().controls
        )
        .contains("\"name\"")
    });
    let saved = Settings::load(&data.join("extension-data").join(MIDI)).unwrap();
    assert_eq!(saved.devices.len(), 1);
    assert_eq!(saved.devices[0].profile, Profile::Loupedeck);
    // Nothing reported a device: Linux lists no inputs, and none is chosen.
    assert!(!e.extensions.device_current(1, 1, 1));
    e.extensions.stop(Instant::now() + Duration::from_secs(2));
}
