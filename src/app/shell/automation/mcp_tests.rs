//! The bundled MCP extension end to end: the built `rawmakase` binary's
//! `rawmakase mcp` runs the built `rawmakase-mcp` program, which edits a test
//! editor through its real control socket, pumped on the test thread. One MCP
//! server serves every step, so the test starts two processes: the server and
//! the launcher refusing to start once the extension is disabled.
//!
//! Both programs are looked for beside this test binary's folder
//! (`target/<profile>/`): `make check` and CI build them before the app's
//! tests.
use super::{Hub, Msg, Settings, socket};
use crate::app::extensions::Host;
use crate::app::{Editor, Module};
use crate::catalog::PhotoId;
use eframe::egui;
use rawmakase_extensions::session::Launch;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const MCP: &str = "rawmakase.mcp";

/// A program of the workspace, as `cargo build` leaves it.
fn built(name: &str) -> PathBuf {
    crate::app::testing::built_program(name).unwrap_or_else(|| {
        panic!("{name} isn't built; run `cargo build -p rawmakase -p rawmakase-mcp` first")
    })
}

/// An MCP client of `rawmakase mcp`, over its standard streams.
struct Client {
    child: Child,
    stdin: ChildStdin,
    replies: mpsc::Receiver<Value>,
    id: u64,
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Client {
    fn start(data: &Path) -> Self {
        let mut child = Command::new(built("rawmakase"))
            .arg("mcp")
            .arg("--data-dir")
            .arg(data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let value = serde_json::from_str(&line).expect("stdout holds only MCP JSON");
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            replies,
            id: 0,
        }
    }
    /// Sends a request; its id, for [`Client::reply`].
    fn send(&mut self, method: &str, params: Value) -> u64 {
        self.id += 1;
        let message = json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params});
        writeln!(self.stdin, "{message}").unwrap();
        self.id
    }
    fn call(&mut self, tool: &str, arguments: Value) -> u64 {
        self.send("tools/call", json!({"name":tool,"arguments":arguments}))
    }
}

/// Runs the editor's control requests until the reply `id` comes, or fails
/// after 20 s. Replies to other requests are kept in `early`.
fn pump(e: &mut Editor, client: &Client, early: &mut Vec<Value>, id: u64) -> Value {
    let ctx = e.context.clone();
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(at) = early.iter().position(|r| r["id"] == id) {
            return early.remove(at);
        }
        e.control_commands(&ctx);
        while let Ok(reply) = client.replies.try_recv() {
            early.push(reply);
        }
        assert!(Instant::now() < until, "no reply to request {id}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A tool call's error code, when it failed.
fn code(reply: &Value) -> Option<&str> {
    (reply["result"]["isError"] == true)
        .then(|| reply["result"]["structuredContent"]["code"].as_str())
        .flatten()
}

/// Takes the control requests waiting in the editor's queue, without
/// running them, until there are `count`.
fn take_queued(e: &mut Editor, count: usize) -> Vec<Msg> {
    let until = Instant::now() + Duration::from_secs(10);
    let mut taken = Vec::new();
    while taken.len() < count {
        taken.extend(e.controls.rx.try_iter());
        assert!(Instant::now() < until, "nothing was queued");
        std::thread::sleep(Duration::from_millis(1));
    }
    taken
}

struct Fixture {
    _dir: tempfile::TempDir,
    editor: Editor,
    data: PathBuf,
    photos: Vec<(PhotoId, PathBuf)>,
    /// The control socket's queue, to put taken requests back.
    tx: mpsc::SyncSender<Msg>,
}

/// An editor in Develop with the first of two photos open, the bundled
/// extensions loaded from an empty data folder and the control socket on,
/// announced in that folder's `control.json`.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let photos_dir = dir.path().join("photos");
    std::fs::create_dir(&photos_dir).unwrap();
    for (i, name) in ["a.png", "b.png"].iter().enumerate() {
        image::RgbImage::from_pixel(32, 24, image::Rgb([60 + 80 * i as u8, 90, 120]))
            .save(photos_dir.join(name))
            .unwrap();
    }
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)
        .unwrap()
        .add_folder(&photos_dir)
        .unwrap();
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.raw_defaults = Arc::new(crate::raw_defaults::DevelopDefaults::with_presets(
        Default::default(),
        |_| None,
    ));
    editor.onboarding.visible = false;
    let library = crate::app::library::Library::load(&catalog, ctx.clone()).unwrap();
    let mut photos: Vec<_> = library
        .session
        .photos()
        .iter()
        .map(|p| (p.id, p.path.clone()))
        .collect();
    photos.sort_by(|a, b| a.1.cmp(&b.1));
    editor.library = Some(Box::new(library));
    let data = dir.path().join("data");
    // Nothing is started: the MCP extension's program is launched by its
    // client, and the bundled manifest is all the host needs.
    let never = Launch {
        program: dir.path().join("no-such-program"),
        args: Vec::new(),
        dir: PathBuf::new(),
        env: Vec::new(),
        log: PathBuf::new(),
    };
    editor.extensions = Host::start_for_tests(&data, never, Duration::from_secs(30));
    assert!(editor.extensions.adapter_enabled("rawmakase.mcp/mcp"));
    let (tx, rx) = mpsc::sync_channel(256);
    editor.controls = Hub::new(Settings::default(), rx);
    editor.controls.socket = Some(
        socket::start_at(
            tx.clone(),
            ctx.clone(),
            rawmakase_protocol::Endpoint::path(&data),
        )
        .unwrap(),
    );
    // The first photo, open in Develop and loaded.
    editor.module = Module::Develop;
    editor.open_raw(photos[0].1.clone(), Some(photos[0].0));
    let until = Instant::now() + Duration::from_secs(30);
    while editor.document.loading() || editor.document.full().is_none() {
        assert!(Instant::now() < until, "the photo never opened");
        editor.events(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    Fixture {
        _dir: dir,
        editor,
        data,
        photos,
        tx,
    }
}

#[test]
fn an_mcp_client_edits_through_the_bundled_extension_until_it_is_disabled() {
    let mut f = fixture();
    let e = &mut f.editor;
    let mut client = Client::start(&f.data);
    let mut early = Vec::new();

    // The server is the program, reporting the app's version.
    let init = client.send(
        "initialize",
        json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}),
    );
    let reply = pump(e, &client, &mut early, init);
    assert_eq!(reply["result"]["serverInfo"]["name"], "rawmakase");
    assert_eq!(
        reply["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    writeln!(
        client.stdin,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();

    // An edit: the recipe changes, as one History step.
    let read = client.call("get_state", json!({}));
    let state = pump(e, &client, &mut early, read);
    assert_eq!(code(&state), None, "{state}");
    let state = &state["result"]["structuredContent"]["state"];
    let steps = e.document.edit.history().steps().0.len();
    let target = json!({
        "photo_id": state["photo_id"],
        "generation": state["generation"],
        "revision": state["revision"],
    });
    let set = client.call(
        "set_parameter",
        json!({"param":"exposure","value":0.75,"target":target}),
    );
    let reply = pump(e, &client, &mut early, set);
    assert_eq!(code(&reply), None, "{reply}");
    assert!((e.document.edit.recipe().exposure - 0.75).abs() < 1e-6);
    assert_eq!(e.document.edit.history().steps().0.len(), steps + 1);

    // A wait parked: opening the second photo waits for it to load, which
    // it can't while only control requests run.
    let open = client.call("open_photo", json!({"id": f.photos[1].0.0}));
    let until = Instant::now() + Duration::from_secs(10);
    while e.controls.waits.is_empty() {
        e.control_commands(&e.context.clone());
        assert!(Instant::now() < until, "the open never waited");
        std::thread::sleep(Duration::from_millis(1));
    }
    let parked_until = e.controls.waits[0].deadline;
    // A request queued, not yet run.
    let queued = client.call("get_state", json!({}));
    let taken = take_queued(e, 1);

    e.set_extension_enabled(MCP, false);
    assert!(!e.extensions.adapter_enabled("rawmakase.mcp/mcp"));
    for message in taken {
        f.tx.send(message).unwrap();
    }
    e.control_commands(&e.context.clone());
    // The parked wait was refused, not left to time out.
    assert!(e.controls.waits.is_empty());
    assert!(Instant::now() < parked_until, "the wait timed out instead");
    let reply = pump(e, &client, &mut early, queued);
    assert_eq!(code(&reply), Some("adapter_disabled"), "{reply}");
    let reply = pump(e, &client, &mut early, open);
    assert_eq!(code(&reply), Some("adapter_disabled"), "{reply}");
    // The next request.
    let next = client.call("get_state", json!({}));
    let reply = pump(e, &client, &mut early, next);
    assert_eq!(code(&reply), Some("adapter_disabled"), "{reply}");

    // A request naming no adapter still runs.
    let connection = rawmakase_ctl::Connection::read(&f.data).unwrap();
    let plain = std::thread::spawn(move || connection.request(json!({"cmd":"state"})));
    let until = Instant::now() + Duration::from_secs(10);
    while !plain.is_finished() {
        e.control_commands(&e.context.clone());
        assert!(Instant::now() < until, "no reply without an adapter");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(plain.join().unwrap().unwrap()["ok"], true);
    drop(client);

    // Disabled, `rawmakase mcp` doesn't start the server.
    let refused = Command::new(built("rawmakase"))
        .arg("mcp")
        .arg("--data-dir")
        .arg(&f.data)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("The MCP extension is disabled in Preferences > Extensions"),
        "{stderr}"
    );
    assert!(refused.stdout.is_empty());
}
