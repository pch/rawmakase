//! Exercise the shipped program over real stdio, with a synthetic desktop
//! socket. The app's own tests run it through `rawmakase mcp` against a real
//! editor (`app::shell::automation::mcp_tests`).
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

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
    fn start(dir: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rawmakase-mcp"))
            .arg("--data-dir")
            .arg(dir)
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
                let value = serde_json::from_str(&line.unwrap())
                    .expect("stdout must contain only MCP JSON");
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            stdin,
            replies,
            id: 0,
        };
        let reply = client.request("initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}));
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
        client
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params})
        )
        .unwrap();
        loop {
            let value = self
                .replies
                .recv_timeout(Duration::from_secs(15))
                .expect("MCP did not reply");
            if value["id"] == self.id {
                return value;
            }
        }
    }
    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name":name,"arguments":arguments}))
    }
}

#[test]
fn stdio_discovery_and_missing_app_return_useful_results() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = Client::start(dir.path());
    let reply = client.request("tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    for name in [
        "get_state",
        "get_capabilities",
        "find_photos",
        "open_photo",
        "set_parameter",
        "set_tone_curve",
        "apply_curve_preset",
        "auto_tone",
        "auto_white_balance",
        "run_action",
        "save_photo",
        "preview_photo",
        "export_photo",
        "get_job",
    ] {
        assert!(tools.iter().any(|t| t["name"] == name), "missing {name}");
    }
    let set = tools.iter().find(|t| t["name"] == "set_parameter").unwrap();
    assert!(
        set["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("target"))
    );
    let missing = client.tool("get_state", json!({}));
    assert_eq!(missing["result"]["isError"], true);
    assert_eq!(
        missing["result"]["structuredContent"]["code"],
        "app_unavailable"
    );
    let invalid = client.tool("set_parameter", json!({"param":"exposure","value":1}));
    assert!(invalid.get("error").is_some() || invalid["result"]["isError"] == true);
    assert!(client.tool("nonexistent", json!({})).get("error").is_some());
    assert!(client.request("ping", json!({})).get("result").is_some());
}

struct Desktop {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    requests: Arc<Mutex<Vec<Value>>>,
}
impl Drop for Desktop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let joined = self.thread.take().unwrap().join();
        if !std::thread::panicking() {
            joined.unwrap();
        }
    }
}
impl Desktop {
    fn start(dir: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        std::fs::write(dir.join("control.json"), json!({"protocol":1,"port":listener.local_addr().unwrap().port(),"token":"synthetic-test-token"}).to_string()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (end, seen) = (stop.clone(), requests.clone());
        let thread = std::thread::spawn(move || {
            let mut preview = None;
            let mut loading = false;
            // Photo 18 can't be opened.
            let mut failed = false;
            while !end.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["token"], "synthetic-test-token");
                // Every request names the adapter, so the app can refuse it
                // while the extension is disabled.
                assert_eq!(request["adapter"], rawmakase_mcp::ADAPTER);
                seen.lock().unwrap().push(request.clone());
                let stale = request.get("target").is_some_and(|t| t["revision"] != 7);
                let mut state = json!({"mode":"develop","photo_id":17,"generation":3,"revision":7,"loaded":true,"values":{"exposure":0.5}});
                if request["cmd"] == "state" && failed {
                    state["photo_id"] = Value::Null;
                    state["loaded"] = false.into();
                    state["open_failed"] = 18.into();
                    state["message"] = "Previews built".into();
                    state["open_error"] = "Could not read the AVIF file".into();
                }
                if request["cmd"] == "state" && loading {
                    state["photo_id"] = Value::Null;
                    state["loaded"] = false.into();
                    loading = false;
                }
                let mut reply = json!({"ok":!stale,"protocol":1,"request_id":request["request_id"],"state":state,"result":null});
                if stale {
                    reply["code"] = "stale_target".into();
                    reply["error"] = "Edit changed".into();
                } else {
                    match request["cmd"].as_str().unwrap() {
                        "preview" => {
                            let path = request["path"].as_str().unwrap();
                            image::RgbImage::from_pixel(2, 2, image::Rgb([100u8, 80, 60]))
                                .save(path)
                                .unwrap();
                            preview = Some(path.to_owned());
                            reply["result"] = json!({"job_id":1,"status":"running","revision":7});
                        }
                        "job" => {
                            reply["result"] =
                                json!({"job_id":1,"status":"completed","revision":7,"path":preview})
                        }
                        "open" if request["id"] == 18 => {
                            failed = true;
                            reply["result"] = json!({"opened":{"id":18}});
                        }
                        "open" => {
                            loading = true;
                            reply["result"] = json!({"opened":{"id":17}});
                        }
                        _ => {}
                    }
                }
                writeln!(stream, "{reply}").unwrap();
            }
        });
        Self {
            stop,
            thread: Some(thread),
            requests,
        }
    }
}

#[test]
fn tools_forward_guards_preserve_errors_and_return_preview_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let desktop = Desktop::start(dir.path());
    let mut client = Client::start(dir.path());
    let target = json!({"photo_id":17,"generation":3,"revision":7});
    let set = client.tool(
        "set_parameter",
        json!({"param":"exposure","value":0.5,"target":target}),
    );
    assert_ne!(set["result"]["isError"], true);
    assert_eq!(set["result"]["structuredContent"]["state"]["revision"], 7);
    let bad = client.tool(
        "set_parameter",
        json!({"param":"exposure","value":1,"target":{"generation":3,"revision":6}}),
    );
    assert_eq!(bad["result"]["isError"], true);
    assert_eq!(bad["result"]["structuredContent"]["code"], "stale_target");
    let points = json!([[0, 0], [0.25, 0.2], [0.75, 0.8], [1, 1]]);
    let curve = client.tool(
        "set_tone_curve",
        json!({"channel":"rgb","points":points,"target":target}),
    );
    assert_ne!(curve["result"]["isError"], true, "{curve}");
    assert!(curve.get("error").is_none(), "{curve}");
    client.tool("auto_tone", json!({"target":target}));
    client.tool("auto_white_balance", json!({"target":target}));
    let opened = client.tool("open_photo", json!({"id":17}));
    assert_eq!(
        opened["result"]["structuredContent"]["state"]["loaded"],
        true
    );
    let reply = client.tool("preview_photo", json!({"target":target,"max_edge":100}));
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    let content = reply["result"]["content"].as_array().unwrap();
    let image = content.iter().find(|c| c["type"] == "image").unwrap();
    assert_eq!(image["mimeType"], "image/jpeg");
    let pixels = STANDARD.decode(image["data"].as_str().unwrap()).unwrap();
    assert_eq!(image::load_from_memory(&pixels).unwrap().width(), 2);
    let path = reply["result"]["structuredContent"]["result"]["path"]
        .as_str()
        .unwrap();
    assert!(
        !Path::new(path).exists(),
        "preview temporary files must be cleaned up"
    );
    // A photo that can't be opened fails at once, with the reason, rather than
    // after 30 s of waiting for it to load.
    let started = std::time::Instant::now();
    let failed = client.tool("open_photo", json!({"id":18}));
    assert_eq!(failed["result"]["isError"], true, "{failed}");
    assert_eq!(
        failed["result"]["structuredContent"]["code"], "open_failed",
        "{failed}"
    );
    assert_eq!(
        failed["result"]["structuredContent"]["error"], "Could not read the AVIF file",
        "{failed}"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    let requests = desktop.requests.lock().unwrap();
    let curve = requests.iter().find(|r| r["cmd"] == "curve").unwrap();
    assert_eq!(curve["target"]["revision"], 7);
    assert_eq!(
        serde_json::from_value::<Vec<[f32; 2]>>(curve["points"].clone()).unwrap(),
        serde_json::from_value::<Vec<[f32; 2]>>(points).unwrap()
    );
    assert!(requests.iter().any(|r| r["action"] == "auto_tone"));
    assert!(requests.iter().any(|r| r["action"] == "auto_white_balance"));
    // A failed mutation was sent once; the adapter never retries it.
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["target"]["revision"] == 6)
            .count(),
        1
    );
}
