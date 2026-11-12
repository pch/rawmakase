//! Sessions with a scripted fake program: this test binary, run again with
//! `RAWMAKASE_FAKE_PROGRAM` naming a script, speaking through the SDK (or
//! misbehaving on purpose). Each test starts one program, or two where a
//! helper process is the point, to keep the run short.
//!
//! The tests that count the app's open handles take [`exclusive`], every
//! other test [`shared`], so no handle comes or goes beside a count.
use rawmakase_extension::protocol::buffer::{Access, Buffer, Meaning, Region, Sample, Space};
use rawmakase_extension::protocol::framing::{LineReader, write_line};
use rawmakase_extension::protocol::method::{CommandRun, Hello, MaskGenerate, MaskGenerated};
use rawmakase_extension::protocol::{ENV_API, ErrorCode, MAX_LINE, Message, PROTOCOL, RemoteError};
use rawmakase_extension::{Context, Extension, MaskAsk, MaskGenerator};
use rawmakase_extensions::session::{Launch, Session, SessionOptions, Update, check_hello};
use rawmakase_extensions::shm::Ledger;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SCRIPT: &str = "RAWMAKASE_FAKE_PROGRAM";

static COUNTING: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// For a test that opens handles: no count runs beside it.
fn shared() -> std::sync::RwLockReadGuard<'static, ()> {
    COUNTING.read().unwrap_or_else(|e| e.into_inner())
}

/// For a test that counts the app's handles: nothing else runs beside it.
fn exclusive() -> std::sync::RwLockWriteGuard<'static, ()> {
    COUNTING.write().unwrap_or_else(|e| e.into_inner())
}
const PID_FILE: &str = "RAWMAKASE_FAKE_PID_FILE";

/// The fake program's entry point: a test that does nothing in the ordinary
/// run, and runs the script when the binary is started as a program.
#[test]
fn fake_program() {
    let Ok(script) = std::env::var(SCRIPT) else {
        return;
    };
    let code = match script.as_str() {
        "sdk" => sdk(),
        "malformed" => malformed(),
        "version" => version(),
        "deaf" | "sleep" => {
            std::thread::sleep(Duration::from_secs(120));
            0
        }
        "helper" => {
            start_helper();
            std::thread::sleep(Duration::from_secs(120));
            0
        }
        "orphan" => {
            start_helper();
            0
        }
        "flood" => flood(),
        "api" => api(),
        "masks" => masks(),
        #[cfg(unix)]
        "fds" => send_fds(),
        other => panic!("no script {other}"),
    };
    std::process::exit(code);
}

fn sdk() -> i32 {
    let result = Extension::new()
        .command("echo", |_cx: &Context, run: CommandRun| Ok(run.input))
        .command(
            "crash",
            |_cx: &Context, _run: CommandRun| -> Result<Value, RemoteError> {
                std::process::exit(3)
            },
        )
        .command(
            "hang",
            |_cx: &Context, _run: CommandRun| -> Result<Value, RemoteError> {
                std::thread::sleep(Duration::from_secs(120));
                Ok(Value::Null)
            },
        )
        .command("device", |cx: &Context, _run: CommandRun| {
            use rawmakase_extension::protocol::method::DeviceState;
            cx.host()
                .device("dial", "knob", DeviceState::Connected, 1)?;
            Ok(json!("reported"))
        })
        .run();
    i32::from(result.is_err())
}

/// Serves a command answering the API mode the app passed; `run` reads it
/// and refuses to serve a mode it doesn't know.
fn api() -> i32 {
    let result = Extension::new()
        .command("api", |_cx: &Context, _run: CommandRun| {
            Ok(json!(std::env::var(ENV_API).ok()))
        })
        .run();
    i32::from(result.is_err())
}

/// A garbage line, an oversized one and a request missing its method, then
/// an ordinary session.
fn malformed() -> i32 {
    let mut channel = rawmakase_extension::channel::attach().unwrap();
    let w = &mut channel.writer;
    w.write_all(b"this is not json\n").unwrap();
    let mut big = vec![b'x'; MAX_LINE + 100];
    big.push(b'\n');
    w.write_all(&big).unwrap();
    w.write_all(b"{\"t\":\"req\",\"id\":77}\n").unwrap();
    w.flush().unwrap();
    let extension =
        Extension::new().command("echo", |_cx: &Context, run: CommandRun| Ok(run.input));
    i32::from(extension.serve(channel.reader, channel.writer).is_err())
}

/// Answers hello with a protocol from the future.
fn version() -> i32 {
    let mut channel = rawmakase_extension::channel::attach().unwrap();
    let mut lines = LineReader::new(channel.reader);
    if let Ok(Some(Ok(Message::Req { id, .. }))) = lines.next_message() {
        let reply = Message::reply(id, Ok(json!({"protocol": PROTOCOL + 1, "implements": []})));
        write_line(&mut channel.writer, &reply).unwrap();
    }
    while let Ok(Some(_)) = lines.next_message() {}
    0
}

/// A burst of operations, then a device's disconnection behind them: the
/// operations past the host's queue are refused at once, the disconnection
/// is acknowledged anyway. Reports what it was answered as an event once
/// every request is answered.
fn flood() -> i32 {
    let channel = rawmakase_extension::channel::attach().unwrap();
    let mut writer = channel.writer;
    const OPS: u64 = 40;
    for id in 1..=OPS {
        let ops = json!({"adapter": "dial", "device": "knob", "epoch": 1, "ops": []});
        write_line(&mut writer, &Message::request(id, "adapter.operate", ops)).unwrap();
    }
    let device = json!({"adapter": "dial", "device": "knob", "state": "disconnected", "epoch": 1});
    write_line(
        &mut writer,
        &Message::request(OPS + 1, "adapter.device", device),
    )
    .unwrap();
    let mut lines = LineReader::new(channel.reader);
    let (mut full, mut answered, mut device_ok) = (0, 0, false);
    while answered < OPS + 1 {
        match lines.next_message() {
            Ok(Some(Ok(Message::Ok { id, .. }))) => {
                answered += 1;
                device_ok |= id == OPS + 1;
            }
            Ok(Some(Ok(Message::Err { code, .. }))) => {
                answered += 1;
                full += u64::from(code == ErrorCode::ChannelFull);
            }
            Ok(Some(_)) => {}
            _ => return 1,
        }
    }
    let result = json!({"channel_full": full, "device_ok": device_ok});
    write_line(&mut writer, &Message::event("flood-result", result)).unwrap();
    while let Ok(Some(_)) = lines.next_message() {}
    0
}

/// Starts this binary again as a helper that sleeps, and writes its pid.
#[expect(
    clippy::zombie_processes,
    reason = "the helper is left on purpose; the test checks that the host ends it"
)]
fn start_helper() {
    let helper = std::process::Command::new(std::env::current_exe().unwrap())
        .args(fake_args())
        .env(SCRIPT, "sleep")
        .spawn()
        .unwrap();
    let file = std::env::var_os(PID_FILE).unwrap();
    let staged = Path::new(&file).with_extension("tmp");
    std::fs::write(&staged, helper.id().to_string()).unwrap();
    std::fs::rename(staged, file).unwrap();
}

fn fake_args() -> [&'static str; 4] {
    ["--exact", "fake_program", "--nocapture", "--test-threads=1"]
}

fn launch(dir: &Path, script: &str) -> Launch {
    Launch {
        program: std::env::current_exe().unwrap(),
        args: fake_args().iter().map(Into::into).collect(),
        dir: dir.to_path_buf(),
        env: vec![
            (SCRIPT.into(), script.into()),
            (PID_FILE.into(), dir.join("helper.pid").into()),
        ],
        log: dir.join("log.txt"),
    }
}

fn hello(dir: &Path) -> Value {
    serde_json::to_value(Hello {
        protocol: PROTOCOL,
        app_version: "0.4.0".into(),
        extension: "dev.test.fake".into(),
        settings: Default::default(),
        contributions: vec![],
        data_dir: dir.join("data"),
        cache_dir: dir.join("cache"),
        session: 7,
    })
    .unwrap()
}

const PATIENCE: Duration = Duration::from_secs(20);

/// Polls until `until` takes an update, returning everything polled.
fn poll_until(session: &mut Session, until: impl Fn(&Update) -> bool) -> Vec<Update> {
    let start = Instant::now();
    let mut seen = Vec::new();
    loop {
        let updates = session.poll(Instant::now());
        let done = updates.iter().any(&until);
        seen.extend(updates);
        if done {
            return seen;
        }
        assert!(start.elapsed() < PATIENCE, "gave up waiting; saw {seen:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn reply_of(updates: &[Update], id: u64) -> Option<Result<Value, RemoteError>> {
    updates.iter().find_map(|u| match u {
        Update::Reply { id: i, result, .. } if *i == id => Some(result.clone()),
        _ => None,
    })
}

fn wait_reply(session: &mut Session, id: u64) -> Result<Value, RemoteError> {
    let updates = poll_until(
        session,
        |u| matches!(u, Update::Reply { id: i, .. } if *i == id),
    );
    reply_of(&updates, id).unwrap()
}

fn later(ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(ms)
}

/// Viewer input a command echoes: the SDK refuses input that isn't one.
fn point(x: f32) -> Value {
    json!({"kind": "point", "x": x, "y": 0.5})
}

fn run(session: &mut Session, command: &str, input: Value, deadline: Instant) -> u64 {
    let run = json!({"command": command, "input": input});
    session.request("command.run", run, deadline).unwrap()
}

#[test]
fn a_program_answers_cancels_reports_devices_and_times_out() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(AtomicBool::new(false));
    let lifecycle = {
        let seen = seen.clone();
        Arc::new(move |method: &str, params: &Value| {
            (method == "adapter.device").then(|| {
                assert_eq!(params["state"], "connected");
                seen.store(true, Ordering::SeqCst);
                Ok(Value::Null)
            })
        })
    };
    let options = SessionOptions {
        lifecycle: Some(lifecycle),
        cancel_grace: Duration::from_millis(300),
        ..SessionOptions::default()
    };
    let mut session = Session::spawn(&launch(dir.path(), "sdk"), options).unwrap();
    let id = session
        .request("hello", hello(dir.path()), later(10_000))
        .unwrap();
    let reply = check_hello(wait_reply(&mut session, id).unwrap()).unwrap();
    assert_eq!(
        reply.implements,
        [
            "command/crash",
            "command/device",
            "command/echo",
            "command/hang"
        ]
    );

    let echo = run(&mut session, "echo", point(0.125), later(10_000));
    assert_eq!(wait_reply(&mut session, echo).unwrap(), point(0.125));

    // Cancelled before its reply is handed over: cancelled it stays, and the
    // program's answer, arriving after, is dropped.
    let raced = run(&mut session, "echo", point(0.25), later(10_000));
    assert!(session.cancel(raced));
    assert!(!session.is_admitted(raced));
    let after = run(&mut session, "echo", point(0.375), later(10_000));
    let updates = poll_until(
        &mut session,
        |u| matches!(u, Update::Reply { id, .. } if *id == after),
    );
    assert_eq!(reply_of(&updates, raced), None);
    // Answered first: the answer stands and a cancel changes nothing.
    assert!(!session.cancel(after));

    // A lifecycle request is answered by the reader, not queued.
    let device = run(&mut session, "device", Value::Null, later(10_000));
    assert_eq!(wait_reply(&mut session, device).unwrap(), json!("reported"));
    assert!(seen.load(Ordering::SeqCst));

    // A request past its deadline is cancelled, stops being admitted, and
    // fails after the grace period, as the program ignores the cancel.
    let hang = run(&mut session, "hang", Value::Null, later(200));
    assert!(session.is_admitted(hang));
    let start = Instant::now();
    let error = wait_reply(&mut session, hang).unwrap_err();
    assert_eq!(error.code, ErrorCode::Deadline);
    assert!(start.elapsed() >= Duration::from_millis(400));

    // Stopping a program stuck in a handler ends it by the deadline.
    let start = Instant::now();
    let stopping = session.stop(later(1_000)).unwrap();
    let killer = stopping.killer();
    stopping.join();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(wait_ended(&killer));
}

fn wait_ended(killer: &rawmakase_extensions::session::Killer) -> bool {
    let start = Instant::now();
    while !killer.has_ended() {
        if start.elapsed() > PATIENCE {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

#[test]
fn a_panicking_lifecycle_callback_closes_the_session() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = Arc::new(|method: &str, _: &Value| {
        (method == "adapter.device").then(|| panic!("the app's lifecycle handler failed"))
    });
    let options = SessionOptions {
        lifecycle: Some(lifecycle),
        ..SessionOptions::default()
    };
    let mut session = Session::spawn(&launch(dir.path(), "sdk"), options).unwrap();
    let id = session
        .request("hello", hello(dir.path()), later(10_000))
        .unwrap();
    wait_reply(&mut session, id).unwrap();
    // The program waits on its device report, which the reader died on: the
    // request it is in fails and the session closes, without its deadline.
    let device = run(&mut session, "device", Value::Null, later(60_000));
    let updates = poll_until(&mut session, |u| *u == Update::Closed);
    let error = reply_of(&updates, device).unwrap().unwrap_err();
    assert_eq!(error.code, ErrorCode::Exited);
}

#[test]
fn a_crash_mid_request_fails_what_it_held_and_closes_the_session() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let mut session =
        Session::spawn(&launch(dir.path(), "sdk"), SessionOptions::default()).unwrap();
    let id = session
        .request("hello", hello(dir.path()), later(10_000))
        .unwrap();
    wait_reply(&mut session, id).unwrap();
    let crash = run(&mut session, "crash", Value::Null, later(10_000));
    let updates = poll_until(&mut session, |u| *u == Update::Closed);
    let error = reply_of(&updates, crash).unwrap().unwrap_err();
    assert_eq!(error.code, ErrorCode::Exited);
    // Nothing more comes, and nothing more is sent.
    assert!(session.poll(Instant::now()).is_empty());
    let late = session.request("command.run", json!({"command": "echo"}), later(10_000));
    assert_eq!(late.unwrap_err().code, ErrorCode::Exited);
    let start = Instant::now();
    while session.exit().is_none() && start.elapsed() < PATIENCE {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(session.exit().and_then(|e| e.code), Some(3));
    assert!(session.has_ended());
}

#[test]
fn malformed_and_oversized_lines_are_reported_and_the_session_goes_on() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let mut session =
        Session::spawn(&launch(dir.path(), "malformed"), SessionOptions::default()).unwrap();
    let id = session
        .request("hello", hello(dir.path()), later(10_000))
        .unwrap();
    let updates = poll_until(
        &mut session,
        |u| matches!(u, Update::Reply { id: i, .. } if *i == id),
    );
    let malformed = updates
        .iter()
        .filter(|u| matches!(u, Update::Malformed { .. }))
        .count();
    assert_eq!(malformed, 3, "{updates:?}");
    let echo = run(&mut session, "echo", point(0.5), later(10_000));
    assert_eq!(wait_reply(&mut session, echo).unwrap(), point(0.5));
    let stopping = session.stop(later(2_000)).unwrap();
    stopping.join();
}

#[test]
fn a_program_of_another_protocol_is_refused() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let mut session =
        Session::spawn(&launch(dir.path(), "version"), SessionOptions::default()).unwrap();
    let id = session
        .request("hello", hello(dir.path()), later(10_000))
        .unwrap();
    let error = check_hello(wait_reply(&mut session, id).unwrap()).unwrap_err();
    assert_eq!(error.code, ErrorCode::UnsupportedProtocol);
}

#[test]
fn a_program_that_stops_reading_fills_its_queue_without_blocking() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let options = SessionOptions {
        queue: 4,
        ..SessionOptions::default()
    };
    let mut session = Session::spawn(&launch(dir.path(), "deaf"), options).unwrap();
    let big = json!("y".repeat(256 << 10));
    let mut full = None;
    for n in 0..200 {
        let start = Instant::now();
        let sent = session.request(
            "command.run",
            json!({"command": "x", "input": big}),
            later(60_000),
        );
        assert!(start.elapsed() < Duration::from_secs(1), "a send blocked");
        if let Err(e) = sent {
            full = Some((n, e));
            break;
        }
    }
    let (n, error) = full.expect("the queue never filled");
    assert_eq!(error.code, ErrorCode::ChannelFull);
    assert!(n >= 4, "filled after {n}");
    // Already past the deadline: the stop ends the program at once.
    let start = Instant::now();
    let stopping = session
        .stop(Instant::now() - Duration::from_secs(1))
        .unwrap();
    let killer = stopping.killer();
    stopping.join();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(wait_ended(&killer));
}

#[test]
fn a_burst_of_operations_never_holds_up_a_lifecycle_request() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(AtomicBool::new(false));
    let lifecycle = {
        let seen = seen.clone();
        Arc::new(move |method: &str, _: &Value| {
            (method == "adapter.device").then(|| {
                seen.store(true, Ordering::SeqCst);
                Ok(Value::Null)
            })
        })
    };
    let options = SessionOptions {
        queue: 4,
        lifecycle: Some(lifecycle),
        ..SessionOptions::default()
    };
    let mut session = Session::spawn(&launch(dir.path(), "flood"), options).unwrap();
    // The disconnection is applied while the owner hasn't drained anything.
    let start = Instant::now();
    while !seen.load(Ordering::SeqCst) {
        assert!(start.elapsed() < PATIENCE);
        std::thread::sleep(Duration::from_millis(5));
    }
    let updates = session.poll(Instant::now());
    let queued: Vec<u64> = updates
        .iter()
        .filter_map(|u| match u {
            Update::Request { id, method, .. } if method == "adapter.operate" => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(queued, [1, 2, 3, 4]);
    for id in queued {
        session.reply(id, Ok(json!({"applied": 0}))).unwrap();
    }
    let updates = poll_until(
        &mut session,
        |u| matches!(u, Update::Event { name, .. } if name == "flood-result"),
    );
    let result = updates
        .iter()
        .find_map(|u| match u {
            Update::Event { params, .. } => Some(params.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(result, json!({"channel_full": 36, "device_ok": true}));
}

fn helper_pid(dir: &Path) -> u32 {
    let file = dir.join("helper.pid");
    let start = Instant::now();
    loop {
        if let Ok(pid) = std::fs::read_to_string(&file) {
            return pid.trim().parse().unwrap();
        }
        assert!(start.elapsed() < PATIENCE, "the helper never started");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_helper_dies_with_its_program_when_the_program_is_stopped() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let session = Session::spawn(&launch(dir.path(), "helper"), SessionOptions::default()).unwrap();
    let helper = helper_pid(dir.path());
    assert!(alive(helper));
    // The program ignores quitting: the stop ends its group at the deadline.
    let stopping = session.stop(later(600)).unwrap();
    let killer = stopping.killer();
    stopping.join();
    killer.kill_remaining();
    assert!(wait_ended(&killer));
    assert!(dies(helper), "the helper outlived its program");
}

#[test]
fn a_helper_left_by_a_program_that_exits_at_once_is_ended() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let mut session =
        Session::spawn(&launch(dir.path(), "orphan"), SessionOptions::default()).unwrap();
    let helper = helper_pid(dir.path());
    poll_until(&mut session, |u| *u == Update::Closed);
    assert!(dies(helper), "the helper outlived its program");
    drop(session);
}

/// Whether the process is gone within the patience.
fn dies(pid: u32) -> bool {
    let start = Instant::now();
    while alive(pid) {
        if start.elapsed() > PATIENCE {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // A killed helper whose program is gone is reparented, and stays a
    // zombie until its new parent reaps it, which a container's first
    // process may never do. A zombie runs nothing, so it counts as ended.
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // The state follows the parenthesised command name.
        let state = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.trim_start().chars().next());
        return !matches!(state, Some('Z' | 'X'));
    }
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: opens the process to wait on it, closing the handle after.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let running = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        CloseHandle(handle);
        running
    }
}

#[test]
fn the_program_is_told_its_manifest_api() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let mut launch = launch(dir.path(), "api");
    launch.env.push((ENV_API.into(), "7".into()));
    launch.set_api(2);
    let mut session = Session::spawn(&launch, SessionOptions::default()).unwrap();
    let id = session
        .request("hello", hello(dir.path()), later(10_000))
        .unwrap();
    wait_reply(&mut session, id).unwrap();
    let api = run(&mut session, "api", Value::Null, later(10_000));
    assert_eq!(wait_reply(&mut session, api).unwrap(), json!("2"));
    session.stop(later(2_000)).unwrap().join();

    // An api the SDK doesn't serve ends the program before it reads.
    launch.set_api(rawmakase_extension::protocol::manifest::API + 1);
    let mut session = Session::spawn(&launch, SessionOptions::default()).unwrap();
    let id = session
        .request("hello", hello(dir.path()), later(10_000))
        .unwrap();
    assert_eq!(
        wait_reply(&mut session, id).unwrap_err().code,
        ErrorCode::Exited
    );
    let start = Instant::now();
    while session.exit().is_none() && start.elapsed() < PATIENCE {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(session.exit().and_then(|e| e.code), Some(1));
}

// Buffers in shared memory: leases, lingering and the Hung transition.

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;

/// Waits until it is cancelled, at most 20 s.
fn until_cancelled(cx: &Context) {
    let start = Instant::now();
    while !cx.is_cancelled() && start.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn generated() -> MaskGenerated {
    MaskGenerated {
        model: "fake".into(),
        model_key: "fake/1".into(),
        empty: None,
    }
}

/// The mask generators, by what they do:
///
/// - `fill` writes each pixel's red as its coverage;
/// - `slow` reports it started, waits to be cancelled, then holds its
///   buffers 300 ms more;
/// - `stubborn` reports it started and ignores the cancel;
/// - `flood` reports it started, waits to be cancelled, then sends 20 events
///   before its reply, so a host not draining drops the reply;
/// - `refuse` sends 20 events, then refuses the request as invalid;
/// - `flood-ok` and `flood-err` send 20 events, then answer `ok`, or
///   `unavailable`;
/// - `crash` exits mid-request.
struct Fake;
impl MaskGenerator for Fake {
    fn generate(&mut self, cx: &Context, ask: MaskAsk<'_>) -> Result<MaskGenerated, RemoteError> {
        let cancelled = || Err(RemoteError::new(ErrorCode::Cancelled, "stopped"));
        let flood = || {
            for _ in 0..20 {
                let _ = cx.host().panel_update("p", vec![]);
            }
        };
        match ask.request.generator.as_str() {
            "fill" => {
                let (image, coverage) = (&ask.request.image, &ask.request.coverage);
                for y in 0..image.height as usize {
                    for x in 0..image.width as usize {
                        ask.coverage[y * coverage.stride as usize + x] =
                            ask.image[y * image.stride as usize + x * 3];
                    }
                }
                Ok(generated())
            }
            "slow" => {
                cx.progress(Some(0.0), Some("started"));
                until_cancelled(cx);
                std::thread::sleep(Duration::from_millis(300));
                cancelled()
            }
            "stubborn" => {
                cx.progress(Some(0.0), Some("started"));
                std::thread::sleep(Duration::from_secs(120));
                Ok(generated())
            }
            "flood" => {
                cx.progress(Some(0.0), Some("started"));
                until_cancelled(cx);
                flood();
                cancelled()
            }
            "refuse" => {
                flood();
                Err(RemoteError::new(ErrorCode::InvalidRequest, "refused"))
            }
            "flood-ok" => {
                flood();
                Ok(generated())
            }
            "flood-err" => {
                flood();
                Err(RemoteError::new(ErrorCode::Unavailable, "no runtime"))
            }
            "crash" => std::process::exit(3),
            other => Err(RemoteError::new(ErrorCode::InvalidRequest, other)),
        }
    }
}

fn masks() -> i32 {
    let result = Extension::new()
        .mask_generator("fill", Fake)
        .mask_generator("slow", Fake)
        .mask_generator("stubborn", Fake)
        .mask_generator("flood", Fake)
        .mask_generator("refuse", Fake)
        .mask_generator("crash", Fake)
        .mask_generator("flood-ok", Fake)
        .mask_generator("flood-err", Fake)
        .command("echo", |_cx: &Context, run: CommandRun| Ok(run.input))
        .command("long", |cx: &Context, _run: CommandRun| {
            cx.progress(Some(0.0), Some("started"));
            until_cancelled(cx);
            Err(RemoteError::new(ErrorCode::Cancelled, "stopped"))
        })
        .command("stubborn", |cx: &Context, _run: CommandRun| {
            cx.progress(Some(0.0), Some("started"));
            std::thread::sleep(Duration::from_secs(120));
            Ok(Value::Null)
        })
        .command("mapped", |_cx: &Context, _run: CommandRun| {
            Ok(json!(rawmakase_extension::mapped_buffers()))
        })
        .run();
    i32::from(result.is_err())
}

/// Sends the app as many descriptors as one message carries, beside an
/// event, then waits to be ended.
#[cfg(unix)]
fn send_fds() -> i32 {
    // SCM_MAX_FD on Linux; 64-bit XNU's control data holds 254.
    #[cfg(target_os = "linux")]
    const COUNT: usize = 253;
    #[cfg(not(target_os = "linux"))]
    const COUNT: usize = 254;
    raise_fd_limit();
    let fd: i32 = std::env::var(rawmakase_extension::protocol::ENV_FD)
        .unwrap()
        .parse()
        .unwrap();
    // SAFETY: duplicates of stdin; the test's own descriptors.
    let fds: Vec<i32> = (0..COUNT).map(|_| unsafe { libc::dup(0) }).collect();
    assert!(fds.iter().all(|&fd| fd >= 0), "out of descriptors");
    let line = b"{\"t\":\"event\",\"e\":\"hello\"}\n";
    let payload = std::mem::size_of_val(fds.as_slice());
    // SAFETY: a msghdr with one iovec and an SCM_RIGHTS control message
    // sized for `fds`, all on this frame.
    let sent = unsafe {
        let space = libc::CMSG_SPACE(payload as u32) as usize;
        let mut control = vec![0u64; space.div_ceil(8)];
        let mut iov = libc::iovec {
            iov_base: line.as_ptr().cast_mut().cast(),
            iov_len: line.len(),
        };
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(payload as u32) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(cmsg).cast::<i32>(), fds.len());
        libc::sendmsg(fd, &msg, 0)
    };
    if sent < 0 {
        eprintln!("sendmsg: {}", std::io::Error::last_os_error());
        return 1;
    }
    std::thread::sleep(Duration::from_secs(120));
    0
}

/// Raises this process's descriptor limit to what the system allows, at most
/// 4096: room for a message's worth of descriptors besides those open.
#[cfg(unix)]
fn raise_fd_limit() {
    // SAFETY: reads and sets this process's own limit.
    unsafe {
        let mut limit: libc::rlimit = std::mem::zeroed();
        libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit);
        limit.rlim_cur = limit.rlim_max.min(4096);
        libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
    }
}

/// How many handles the app has open.
#[cfg(unix)]
fn open_handles() -> usize {
    #[cfg(target_os = "linux")]
    let dir = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    let dir = "/dev/fd";
    std::fs::read_dir(dir).map_or(0, Iterator::count)
}

#[cfg(windows)]
fn open_handles() -> usize {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let mut count = 0u32;
    // SAFETY: the current process's pseudo-handle and a valid out-parameter.
    unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
    count as usize
}

/// The app's handle count once it holds still: the threads of a session an
/// earlier test dropped let go of their ends a moment after. The first
/// program started opens handles the system keeps for later ones (Windows), so
/// one program runs to its end before the first count.
fn baseline() -> usize {
    static WARM: std::sync::Once = std::sync::Once::new();
    WARM.call_once(|| {
        let dir = tempfile::tempdir().unwrap();
        let mut session = masks_session(dir.path(), SessionOptions::default());
        let fill = send_mask(&mut session, "fill").unwrap();
        wait_reply(&mut session, fill).unwrap();
        session.stop(later(2_000)).unwrap().join();
    });
    let mut count = open_handles();
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let now = open_handles();
        if now == count {
            return count;
        }
        count = now;
    }
}

/// Waits until the app's handle count is back to `baseline`: the session's
/// threads let go of their ends once the program is gone.
fn settles(baseline: usize) {
    let start = Instant::now();
    while open_handles() > baseline {
        assert!(
            start.elapsed() < PATIENCE,
            "{} handles open, {baseline} before",
            open_handles()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A mask request's parameters: an image in handle `image`, a coverage in
/// handle `coverage`.
fn mask_params(generator: &str, image: u32, coverage: u32) -> Value {
    let region = Region {
        space: Space::ImageFrame,
        rect: [0.0, 0.0, 1.0, 1.0],
        scale: 0.05,
        overlap: 0,
    };
    let image = Buffer {
        handle: image,
        bytes: u64::from(WIDTH * HEIGHT * 3),
        width: WIDTH,
        height: HEIGHT,
        stride: WIDTH * 3,
        channels: 3,
        sample: Sample::U8,
        meaning: MaskGenerate::IMAGE,
        region,
        access: Access::Read,
    };
    let coverage = Buffer {
        handle: coverage,
        bytes: u64::from(WIDTH * HEIGHT),
        stride: WIDTH,
        channels: 1,
        meaning: Meaning::Coverage,
        access: Access::Write,
        ..image
    };
    serde_json::to_value(MaskGenerate {
        generator: generator.into(),
        input: None,
        image,
        coverage,
        image_key: "key".into(),
    })
    .unwrap()
}

/// The red of the image [`send_mask`] sends: `(x + 2y) % 256`.
fn red(x: u32, y: u32) -> u8 {
    ((x + 2 * y) % 256) as u8
}

/// Sends a mask request with its buffers, as the app will: the image
/// rendered into an input, the coverage an output.
fn send_mask(session: &mut Session, generator: &str) -> Result<u64, RemoteError> {
    let mut buffers = session.buffers();
    let image = buffers.input(u64::from(WIDTH * HEIGHT * 3), |bytes| {
        for (i, pixel) in bytes.chunks_mut(3).enumerate() {
            let (x, y) = (i as u32 % WIDTH, i as u32 / WIDTH);
            pixel.copy_from_slice(&[red(x, y), 1, 2]);
        }
    })?;
    let coverage = buffers.output(u64::from(WIDTH * HEIGHT))?;
    session.request_with(
        "mask.generate",
        mask_params(generator, image, coverage),
        buffers,
        later(60_000),
    )
}

fn started(session: &mut Session, id: u64) {
    poll_until(
        session,
        |u| matches!(u, Update::Progress { id: i, .. } if *i == id),
    );
}

fn masks_session(dir: &Path, options: SessionOptions) -> Session {
    let mut launch = launch(dir, "masks");
    launch.set_api(2);
    let mut session = Session::spawn(&launch, options).unwrap();
    let id = session.request("hello", hello(dir), later(10_000)).unwrap();
    wait_reply(&mut session, id).unwrap();
    session
}

/// Polls until the ledger is empty, returning everything polled.
fn released(session: &mut Session) -> Vec<Update> {
    let start = Instant::now();
    let mut seen = Vec::new();
    while session.ledger().in_use() > 0 {
        assert!(start.elapsed() < PATIENCE, "never released; saw {seen:?}");
        seen.extend(session.poll(Instant::now()));
        std::thread::sleep(Duration::from_millis(5));
    }
    seen
}

/// One program through every lease that ends without a fault: on a reply, on
/// a cancel while the handler holds the buffers, on a cancel while queued
/// behind a long command, on a reply the full queue dropped, and against the
/// budget. Then the stop, and the app's handles back where they were.
#[test]
fn leases_end_with_the_program_s_reply() {
    let _guard = exclusive();
    let dir = tempfile::tempdir().unwrap();
    let baseline = baseline();
    let options = SessionOptions {
        queue: 4,
        lease_grace: Duration::from_secs(60),
        ..SessionOptions::default()
    };
    let mut session = masks_session(dir.path(), options);

    // A reply hands the buffers over, with what the program wrote; the lease
    // ends when the app drops them.
    let fill = send_mask(&mut session, "fill").unwrap();
    assert!(session.ledger().in_use() > 0);
    let updates = poll_until(
        &mut session,
        |u| matches!(u, Update::Reply { id, .. } if *id == fill),
    );
    let Some(Update::Reply {
        result, buffers, ..
    }) = updates
        .iter()
        .find(|u| matches!(u, Update::Reply { id, .. } if *id == fill))
    else {
        unreachable!()
    };
    let generated: MaskGenerated = serde_json::from_value(result.clone().unwrap()).unwrap();
    assert_eq!(generated.model, "fake");
    let buffers = buffers.clone().expect("the buffers come back");
    let coverage = buffers.read(1).unwrap();
    assert_eq!(coverage.len() as u32, WIDTH * HEIGHT);
    assert!(
        (0..HEIGHT).all(|y| (0..WIDTH).all(|x| coverage[(y * WIDTH + x) as usize] == red(x, y)))
    );
    assert_eq!(buffers.read(0), None, "an input isn't read back");
    drop((updates, buffers));
    assert_eq!(session.ledger().in_use(), 0);

    // Cancelled while the handler holds its buffers: settled at once, the
    // lease kept until the program's cancelled reply.
    let slow = send_mask(&mut session, "slow").unwrap();
    started(&mut session, slow);
    let cancelled = Instant::now();
    assert!(session.cancel(slow));
    assert!(!session.is_admitted(slow));
    assert_eq!(session.in_flight(), 0, "busy clears at once");
    assert!(session.ledger().in_use() > 0, "the program still holds it");
    assert!(
        session.next_deadline().is_some(),
        "the lease's grace is a deadline"
    );
    let seen = released(&mut session);
    assert!(cancelled.elapsed() >= Duration::from_millis(250));
    assert!(
        reply_of(&seen, slow).is_none(),
        "settled once, at the cancel"
    );
    let mapped = run(&mut session, "mapped", Value::Null, later(10_000));
    assert_eq!(wait_reply(&mut session, mapped).unwrap(), json!(0));

    // Queued behind a long command: answered at once, and nothing is killed.
    let long = run(&mut session, "long", Value::Null, later(60_000));
    started(&mut session, long);
    let queued = send_mask(&mut session, "fill").unwrap();
    assert!(session.cancel(queued));
    released(&mut session);
    assert!(session.is_admitted(long), "the long command runs on");
    assert!(!session.is_hung());
    assert!(session.cancel(long));

    // A cancelled request answered while the queue is full: the reply is
    // dropped, the lease released all the same, and nothing hangs.
    let flood = send_mask(&mut session, "flood").unwrap();
    started(&mut session, flood);
    // Counted before the cancel lets the program flood the queue.
    let dropped = session.dropped();
    assert!(session.cancel(flood));
    // 20 events past a queue of 4, and the reply.
    wait_dropped(&session, dropped + 17);
    let updates = session.poll(Instant::now());
    assert_eq!(session.ledger().in_use(), 0, "{updates:?}");
    assert!(!updates.iter().any(|u| matches!(u, Update::Hung { .. })));

    // A pending request whose reply the full queue dropped is settled once
    // from what the reader recorded: an `ok`, whose value is lost, as
    // `channel_full`, an error as it was sent; its lease ends either way.
    for (generator, code) in [
        ("flood-ok", ErrorCode::ChannelFull),
        ("flood-err", ErrorCode::Unavailable),
    ] {
        let dropped = session.dropped();
        let id = send_mask(&mut session, generator).unwrap();
        wait_dropped(&session, dropped + 17);
        let mut seen = session.poll(Instant::now());
        let replies = |seen: &[Update]| {
            seen.iter()
                .filter(|u| matches!(u, Update::Reply { id: i, .. } if *i == id))
                .count()
        };
        assert_eq!(replies(&seen), 1, "{generator}: {seen:?}");
        assert_eq!(reply_of(&seen, id).unwrap().unwrap_err().code, code);
        assert!(!session.is_admitted(id));
        assert_eq!(session.ledger().in_use(), 0, "{generator}");
        seen.extend(session.poll(Instant::now()));
        assert_eq!(replies(&seen), 1, "{generator}: one outcome");
    }

    // Cancel and retry while the handler can't stop: the budget refuses
    // before allocating.
    let big = 200 << 20;
    let with = |session: &mut Session, command: &str| {
        let mut buffers = session.buffers();
        buffers.output(big)?;
        let run = json!({"command": command});
        session.request_with("command.run", run, buffers, later(60_000))
    };
    let stuck = with(&mut session, "stubborn").unwrap();
    started(&mut session, stuck);
    assert!(session.cancel(stuck));
    let retry = with(&mut session, "echo").unwrap();
    assert!(session.cancel(retry));
    poll_until_ledger(&mut session, big);
    let again = with(&mut session, "echo").unwrap();
    assert_eq!(session.ledger().in_use(), 2 * big);
    let refused = with(&mut session, "echo").unwrap_err();
    assert_eq!(refused.code, ErrorCode::Busy);
    assert_eq!(session.ledger().in_use(), 2 * big);
    assert!(session.is_admitted(again));

    // The stop ends the program at its deadline and every lease with it.
    let ledger = session.ledger().clone();
    let stopping = session.stop(later(600)).unwrap();
    let killer = stopping.killer();
    stopping.join();
    assert!(wait_ended(&killer));
    assert_eq!(ledger.in_use(), 0);
    // The killer holds the job object on Windows.
    drop(killer);
    settles(baseline);
}

/// Waits, without polling, until the reader has dropped `count` messages in
/// all.
fn wait_dropped(session: &Session, count: u64) {
    let start = Instant::now();
    while session.dropped() < count {
        assert!(start.elapsed() < PATIENCE, "the queue never filled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn poll_until_ledger(session: &mut Session, bytes: u64) {
    let start = Instant::now();
    while session.ledger().in_use() != bytes {
        assert!(start.elapsed() < PATIENCE);
        session.poll(Instant::now());
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Polls until `Closed` and the unit has ended, returning everything polled.
fn until_gone(session: &mut Session) -> Vec<Update> {
    let mut seen = poll_until(session, |u| *u == Update::Closed);
    let start = Instant::now();
    while !session.has_ended() || session.ledger().in_use() > 0 {
        assert!(start.elapsed() < PATIENCE);
        seen.extend(session.poll(Instant::now()));
        std::thread::sleep(Duration::from_millis(5));
    }
    seen
}

#[test]
fn a_crash_releases_every_lease_once_the_program_is_gone() {
    let _guard = exclusive();
    let dir = tempfile::tempdir().unwrap();
    let baseline = baseline();
    {
        let mut session = masks_session(dir.path(), SessionOptions::default());
        let crash = send_mask(&mut session, "crash").unwrap();
        let seen = until_gone(&mut session);
        assert_eq!(
            reply_of(&seen, crash).unwrap().unwrap_err().code,
            ErrorCode::Exited
        );
        assert_eq!(session.ledger().in_use(), 0);
    }
    settles(baseline);
}

/// A cancelled request whose handler ignores the cancel past the lease's
/// grace, while another waits behind it: the program is killed, the waiting
/// request fails once with `exited`, the cancelled one stays cancelled, and
/// the lease ends with the program.
#[test]
fn a_lease_past_its_grace_hangs_the_program_up() {
    let _guard = exclusive();
    let dir = tempfile::tempdir().unwrap();
    let baseline = baseline();
    {
        let options = SessionOptions {
            lease_grace: Duration::from_millis(300),
            ..SessionOptions::default()
        };
        let mut session = masks_session(dir.path(), options);
        let stuck = send_mask(&mut session, "stubborn").unwrap();
        started(&mut session, stuck);
        let cancelled = Instant::now();
        assert!(session.cancel(stuck));
        let waiting = run(&mut session, "echo", point(0.625), later(60_000));
        let mut seen = poll_until(&mut session, |u| matches!(u, Update::Hung { .. }));
        assert!(cancelled.elapsed() >= Duration::from_millis(300));
        assert!(session.is_hung());
        assert_eq!(
            session
                .request("command.run", json!({"command": "echo"}), later(1_000))
                .unwrap_err()
                .code,
            ErrorCode::Exited
        );
        seen.extend(until_gone(&mut session));
        let replies = |id| {
            seen.iter()
                .filter(|u| matches!(u, Update::Reply { id: i, .. } if *i == id))
                .count()
        };
        assert_eq!(replies(waiting), 1);
        assert_eq!(
            reply_of(&seen, waiting).unwrap().unwrap_err().code,
            ErrorCode::Exited
        );
        assert_eq!(replies(stuck), 0, "cancelled it stays");
        assert_eq!(
            seen.iter()
                .filter(|u| matches!(u, Update::Hung { .. }))
                .count(),
            1
        );
    }
    settles(baseline);
}

/// A request that carried handles refused as invalid, with the reply dropped
/// by a full queue: the outcome the reader recorded hangs the program up all
/// the same, and the lease ends only when the program is gone.
#[test]
fn a_refused_handle_request_hangs_the_program_up_even_unread() {
    let _guard = exclusive();
    let dir = tempfile::tempdir().unwrap();
    let baseline = baseline();
    {
        let options = SessionOptions {
            queue: 4,
            ..SessionOptions::default()
        };
        let mut session = masks_session(dir.path(), options);
        let dropped = session.dropped();
        let refused = send_mask(&mut session, "refuse").unwrap();
        wait_dropped(&session, dropped + 17);
        let updates = session.poll(Instant::now());
        assert!(
            updates.iter().any(|u| matches!(u, Update::Hung { .. })),
            "{updates:?}"
        );
        assert!(session.ledger().in_use() > 0 || session.has_ended());
        // Its one outcome is the error it was answered with, from the
        // reader's record; the hang-up fails nothing twice.
        assert_eq!(
            reply_of(&updates, refused).unwrap().unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        let rest = until_gone(&mut session);
        let replies = updates
            .iter()
            .chain(&rest)
            .filter(|u| matches!(u, Update::Reply { id, .. } if *id == refused))
            .count();
        assert_eq!(replies, 1);
    }
    settles(baseline);
}

/// A program sends the app a message's worth of descriptors: each is closed
/// as it arrives, and the program is hung up.
#[cfg(unix)]
#[test]
fn descriptors_a_program_sends_are_closed_and_hang_it_up() {
    let _guard = exclusive();
    raise_fd_limit();
    let dir = tempfile::tempdir().unwrap();
    let baseline = baseline();
    {
        let mut session =
            Session::spawn(&launch(dir.path(), "fds"), SessionOptions::default()).unwrap();
        let seen = poll_until(&mut session, |u| matches!(u, Update::Hung { .. }));
        let reason = seen.iter().find_map(|u| match u {
            Update::Hung { reason } => Some(reason.clone()),
            _ => None,
        });
        assert!(reason.unwrap().contains("handle"));
        until_gone(&mut session);
    }
    settles(baseline);
}

#[test]
fn an_api_1_program_is_passed_no_handles() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let mut session =
        Session::spawn(&launch(dir.path(), "sdk"), SessionOptions::default()).unwrap();
    let mut buffers = session.buffers();
    buffers.output(16).unwrap();
    let error = session
        .request_with("mask.generate", json!({}), buffers, later(10_000))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(session.ledger().in_use(), 0);
    assert_eq!(session.in_flight(), 0);
}

/// Waits for the unit to end, checking all the while that the leases are
/// counted until it has: the ledger is read first, so an empty ledger seen
/// before the unit has ended is a lease released too soon.
fn leases_outlive_the_program(ledger: &Ledger, killer: &rawmakase_extensions::session::Killer) {
    let start = Instant::now();
    loop {
        let in_use = ledger.in_use();
        let ended = killer.has_ended();
        assert!(in_use > 0 || ended, "released before the program ended");
        if ended && in_use == 0 {
            return;
        }
        assert!(start.elapsed() < PATIENCE, "never released");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A program stuck in a handler that holds its buffers: stopping it, and
/// dropping its session, end the unit, and the leases end only once the unit
/// has, on the ledger a restart would share. The stop keeps its deadline.
#[test]
fn leases_end_when_a_stopped_or_dropped_program_does() {
    let _guard = shared();
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::new();
    let options = || SessionOptions {
        ledger: ledger.clone(),
        lease_grace: Duration::from_secs(60),
        ..SessionOptions::default()
    };

    let mut session = masks_session(dir.path(), options());
    let stuck = send_mask(&mut session, "stubborn").unwrap();
    started(&mut session, stuck);
    let killer = session.killer();
    let start = Instant::now();
    let stopping = session.stop(later(600)).unwrap();
    assert!(ledger.in_use() > 0 || killer.has_ended());
    stopping.join();
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "the stop kept its deadline"
    );
    leases_outlive_the_program(&ledger, &killer);

    let mut session = masks_session(dir.path(), options());
    let stuck = send_mask(&mut session, "stubborn").unwrap();
    started(&mut session, stuck);
    // Cancelled first, so the lease is lingering as well as held.
    assert!(session.cancel(stuck));
    let held = send_mask(&mut session, "fill").unwrap();
    assert!(session.is_admitted(held));
    let killer = session.killer();
    drop(session);
    leases_outlive_the_program(&ledger, &killer);
}
