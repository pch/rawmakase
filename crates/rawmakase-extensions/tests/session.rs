//! Sessions with a scripted fake program: this test binary, run again with
//! `RAWMAKASE_FAKE_PROGRAM` naming a script, speaking through the SDK (or
//! misbehaving on purpose). Each test starts one program, or two where a
//! helper process is the point, to keep the run short.
use rawmakase_extension::protocol::framing::{LineReader, write_line};
use rawmakase_extension::protocol::method::{CommandRun, Hello};
use rawmakase_extension::protocol::{ENV_API, ErrorCode, MAX_LINE, Message, PROTOCOL, RemoteError};
use rawmakase_extension::{Context, Extension};
use rawmakase_extensions::session::{Launch, Session, SessionOptions, Update, check_hello};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SCRIPT: &str = "RAWMAKASE_FAKE_PROGRAM";
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

fn run(session: &mut Session, command: &str, input: Value, deadline: Instant) -> u64 {
    let run = json!({"command": command, "input": input});
    session.request("command.run", run, deadline).unwrap()
}

#[test]
fn a_program_answers_cancels_reports_devices_and_times_out() {
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

    let echo = run(&mut session, "echo", json!({"n": 1}), later(10_000));
    assert_eq!(wait_reply(&mut session, echo).unwrap(), json!({"n": 1}));

    // Cancelled before its reply is handed over: cancelled it stays, and the
    // program's answer, arriving after, is dropped.
    let raced = run(&mut session, "echo", json!(2), later(10_000));
    assert!(session.cancel(raced));
    assert!(!session.is_admitted(raced));
    let after = run(&mut session, "echo", json!(3), later(10_000));
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
    let echo = run(&mut session, "echo", json!("still here"), later(10_000));
    assert_eq!(wait_reply(&mut session, echo).unwrap(), json!("still here"));
    let stopping = session.stop(later(2_000)).unwrap();
    stopping.join();
}

#[test]
fn a_program_of_another_protocol_is_refused() {
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
