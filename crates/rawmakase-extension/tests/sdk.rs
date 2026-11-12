//! The run loop against the mock host: the handshake, requests both ways,
//! cancellation, panels, adapters and mask generators.
use rawmakase_extension::mock::{MockHost, Recorded};
use rawmakase_extension::protocol::buffer::{
    Access, Alpha, Buffer, Meaning, Primaries, Region, Sample, Space, Transfer, White,
};
use rawmakase_extension::protocol::controls::{Control, ControlKind};
use rawmakase_extension::protocol::message::NeedsSetup;
use rawmakase_extension::protocol::method::{
    AppEvent, AppRequest, CommandRun, DeviceState, Empty, EmptyMask, Hello, MaskGenerate,
    MaskGenerated, Operation, SettingChanged, SettingsChange,
};
use rawmakase_extension::protocol::viewer::{Bounds, Click, MAX_CLICKS, ViewerInput};
use rawmakase_extension::{
    Context, ControlAdapter, ErrorCode, Extension, Host, MaskAsk, MaskGenerator, Panel,
    RemoteError, ServeOptions,
};
use serde_json::{Value, json};
use std::sync::mpsc;
use std::time::Duration;

fn toggle(value: bool, help: Option<&str>) -> Vec<Control> {
    vec![Control {
        id: Some("on".into()),
        label: Some("Listen".into()),
        help: help.map(str::to_string),
        visible_when: None,
        disabled: false,
        kind: ControlKind::Toggle { value },
    }]
}

struct Switch(bool);
impl Panel for Switch {
    fn open(&mut self, _cx: &Context) -> Result<Vec<Control>, RemoteError> {
        Ok(toggle(self.0, None))
    }
    fn changed(
        &mut self,
        _cx: &Context,
        control: &str,
        value: Value,
    ) -> Result<Vec<Control>, RemoteError> {
        match (control, value.as_bool()) {
            ("on", Some(on)) => {
                self.0 = on;
                Ok(toggle(on, None))
            }
            _ => Ok(toggle(self.0, Some("Not a switch value"))),
        }
    }
}

#[test]
fn a_session_says_hello_runs_commands_and_ends_on_quitting() {
    let extension = Extension::new()
        .command("echo", |_cx: &Context, run: CommandRun| Ok(run.input))
        .command("setting", |cx: &Context, _run: CommandRun| {
            Ok(cx.settings().get("speed").cloned().unwrap_or_default())
        })
        .panel("switch", Switch(false));
    let host = MockHost::start(extension);
    // Nothing before the handshake.
    let early = host.run_command("echo", None, json!(1)).unwrap_err();
    assert_eq!(early.code, ErrorCode::InvalidRequest);
    let hello = host.hello().unwrap();
    assert_eq!(hello.protocol, rawmakase_extension::protocol::PROTOCOL);
    assert_eq!(
        hello.implements,
        ["command/echo", "command/setting", "panel/switch"]
    );
    // API 1 hands the input over as it came, whatever it is.
    assert_eq!(
        host.run_command("echo", None, json!({"a": 1})).unwrap(),
        json!({"a": 1})
    );
    let unknown = host.run_command("nope", None, Value::Null).unwrap_err();
    assert_eq!(unknown.code, ErrorCode::InvalidRequest);
    let method = host
        .request(AppRequest::PanelOpen(
            rawmakase_extension::protocol::method::PanelOpen {
                panel: "none".into(),
            },
        ))
        .unwrap_err();
    assert_eq!(method.code, ErrorCode::InvalidRequest);
    // Settings changes reach the handlers.
    host.event(AppEvent::SettingChanged(SettingChanged {
        id: "speed".into(),
        value: json!(3),
    }));
    assert_eq!(
        host.run_command("setting", None, Value::Null).unwrap(),
        json!(3)
    );
    // Panels answer with their controls, and a bad value with its help.
    assert_eq!(host.open_panel("switch").unwrap(), toggle(false, None));
    assert_eq!(
        host.change("switch", "on", json!(true)).unwrap(),
        toggle(true, None)
    );
    let refused = host.change("switch", "on", json!("yes")).unwrap();
    assert_eq!(refused, toggle(true, Some("Not a switch value")));
    assert_eq!(
        host.click("switch", "on").unwrap_err().code,
        ErrorCode::InvalidRequest
    );
    host.finish().unwrap();
}

#[test]
fn another_protocol_ends_the_session() {
    let host = MockHost::start(Extension::new());
    let hello = Hello {
        protocol: 99,
        app_version: "9.0.0".into(),
        extension: "test.extension".into(),
        settings: Default::default(),
        contributions: vec![],
        data_dir: "data".into(),
        cache_dir: "cache".into(),
        session: 1,
    };
    let error = host.hello_with(hello).unwrap_err();
    assert_eq!(error.code, ErrorCode::UnsupportedProtocol);
    assert!(host.finish().is_err());
}

#[test]
fn a_command_proposes_through_the_host_and_sees_its_reply() {
    let extension = Extension::new().command("auto", |cx: &Context, run: CommandRun| {
        let snapshot = cx.host().read_selection()?;
        assert!(run.target.is_none());
        let changes = snapshot
            .photos
            .iter()
            .map(|p| SettingsChange {
                photo: p.id,
                settings: json!({"exposure": 0.5}).as_object().unwrap().clone(),
            })
            .collect();
        let proposed = cx
            .host()
            .propose_settings(cx.op(), snapshot.token, changes)?;
        cx.progress(Some(1.0), Some("done"));
        Ok(json!(proposed.applied))
    });
    let host = MockHost::start_with(extension, |method, params| match method {
        "read.selection" => Ok(json!({"token": "t1", "photos": [
            {"id": 4, "path": "/p/a.dng"}, {"id": 5, "path": "/p/b.dng"}]})),
        "propose.settings" => {
            assert_eq!(params["token"], "t1");
            Ok(json!({"applied": [4], "refused": [{"photo": 5, "code": "stale_target"}]}))
        }
        other => Err(RemoteError::new(ErrorCode::UnknownMethod, other)),
    });
    host.hello().unwrap();
    let id = host.send_request(AppRequest::CommandRun(CommandRun {
        command: "auto".into(),
        target: None,
        input: Value::Null,
    }));
    assert_eq!(host.wait(id).unwrap(), json!([4]));
    let recorded = host.recorded();
    let methods: Vec<_> = recorded
        .iter()
        .filter_map(|r| match r {
            Recorded::Request { method, params, .. } => Some((method.as_str(), params.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(methods[0].0, "read.selection");
    assert_eq!(methods[1].0, "propose.settings");
    // The proposal names the app's request it belongs to.
    assert_eq!(methods[1].1["op"], json!(id));
    assert!(
        recorded
            .iter()
            .any(|r| matches!(r, Recorded::Progress { id: i, .. } if *i == id))
    );
    host.finish().unwrap();
}

#[test]
fn a_cancelled_request_ends_cancelled_whether_it_ran_or_waited() {
    let (started_tx, started) = mpsc::channel();
    let extension = Extension::new().command("slow", move |cx: &Context, _run: CommandRun| {
        let _ = started_tx.send(());
        // The flag lent to work that polls its own is the same one.
        let flag = cx.cancel_flag();
        for _ in 0..1000 {
            if flag.load(std::sync::atomic::Ordering::Relaxed) {
                assert!(cx.is_cancelled());
                return Err(RemoteError::new(ErrorCode::Cancelled, "stopped"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(json!("finished"))
    });
    let host = MockHost::start(extension);
    host.hello().unwrap();
    let run = || {
        host.send_request(AppRequest::CommandRun(CommandRun {
            command: "slow".into(),
            target: None,
            input: Value::Null,
        }))
    };
    let running = run();
    let waiting = run();
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    // The second is cancelled while it waits behind the first, then the first
    // while it runs.
    host.cancel(waiting);
    host.cancel(running);
    assert_eq!(host.wait(running).unwrap_err().code, ErrorCode::Cancelled);
    assert_eq!(host.wait(waiting).unwrap_err().code, ErrorCode::Cancelled);
    // A cancel for a request already answered changes nothing.
    host.cancel(running);
    assert_eq!(
        host.run_command("missing", None, Value::Null)
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
    host.finish().unwrap();
}

#[test]
fn api_2_checks_a_commands_viewer_input_before_its_handler_runs() {
    let (ran_tx, ran) = mpsc::channel();
    let extension = Extension::new().command("pick", move |_cx: &Context, run: CommandRun| {
        let _ = ran_tx.send(());
        let input = run.viewer_input()?;
        Ok(serde_json::to_value(input).unwrap())
    });
    let host =
        MockHost::start_with_options(extension, ServeOptions { api: 2 }, |_, _| Ok(Value::Null));
    host.hello().unwrap();
    let clicks = json!({"kind": "points", "clicks": [
        {"x": 0.25, "y": 0.5, "positive": true},
        {"x": 0.75, "y": 0.5, "positive": false}]});
    assert_eq!(
        host.run_command("pick", None, clicks.clone()).unwrap(),
        clicks
    );
    assert_eq!(
        host.run_command("pick", None, Value::Null).unwrap(),
        Value::Null
    );
    assert_eq!(ran.try_iter().count(), 2);
    let too_many = json!({"kind": "points", "clicks":
        vec![json!({"x": 0.5, "y": 0.5, "positive": true}); MAX_CLICKS + 1]});
    for bad in [
        json!({"a": 1}),
        json!({"kind": "brush"}),
        json!({"kind": "point", "x": 1.5, "y": 0.5}),
        json!({"kind": "box", "x0": 0.6, "y0": 0.1, "x1": 0.5, "y1": 0.9}),
        too_many,
    ] {
        let error = host.run_command("pick", None, bad.clone()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{bad}");
    }
    assert_eq!(ran.try_iter().count(), 0, "refused before the handler ran");
    host.finish().unwrap();
    // API 1 hands it over as it came; the handler checks it if it asks.
    let (ran_tx, ran) = mpsc::channel();
    let extension = Extension::new().command("pick", move |_cx: &Context, run: CommandRun| {
        let _ = ran_tx.send(());
        run.viewer_input()
            .map(|input| serde_json::to_value(input).unwrap())
    });
    let host = MockHost::start(extension);
    host.hello().unwrap();
    let error = host
        .run_command("pick", None, json!({"kind": "brush"}))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(ran.try_iter().count(), 1, "the handler ran and refused it");
    host.finish().unwrap();
}

#[test]
fn a_reply_too_long_for_a_line_arrives_as_too_large() {
    let extension = Extension::new().command("big", |_cx: &Context, _run: CommandRun| {
        Ok(json!("x".repeat(rawmakase_extension::protocol::MAX_LINE)))
    });
    let host = MockHost::start(extension);
    host.hello().unwrap();
    let error = host.run_command("big", None, Value::Null).unwrap_err();
    assert_eq!(error.code, ErrorCode::TooLarge);
    host.finish().unwrap();
}

/// An adapter whose device connects, turns a dial, and disconnects, from a
/// thread of its own.
struct Dial(Option<std::thread::JoinHandle<()>>);
impl ControlAdapter for Dial {
    fn start(&mut self, host: Host) -> Result<(), RemoteError> {
        self.0 = Some(std::thread::spawn(move || {
            let epoch = 1;
            host.device("dial", "knob", DeviceState::Connected, epoch)
                .unwrap();
            let ops = vec![Operation::Adjust {
                param: "exposure".into(),
                ticks: 2,
            }];
            let operated = host.operate("dial", "knob", epoch, ops).unwrap();
            assert_eq!(operated.applied, 1);
            host.device("dial", "knob", DeviceState::Disconnected, epoch)
                .unwrap();
        }));
        Ok(())
    }
    fn stop(&mut self) {
        if let Some(thread) = self.0.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn an_adapter_starts_after_hello_and_reports_from_its_own_thread() {
    let host = MockHost::start_with(Extension::new().adapter("dial", Dial(None)), |method, _| {
        Ok(if method == "adapter.operate" {
            json!({"applied": 1})
        } else {
            Value::Null
        })
    });
    let hello = host.hello().unwrap();
    assert_eq!(hello.implements, ["control-adapter/dial"]);
    let recorded = host.wait_for(|r| r.len() >= 3);
    let methods: Vec<_> = recorded
        .iter()
        .map(|r| match r {
            Recorded::Request { method, params, .. } => (method.clone(), params["state"].clone()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        methods,
        [
            ("adapter.device".to_string(), json!("connected")),
            ("adapter.operate".to_string(), Value::Null),
            ("adapter.device".to_string(), json!("disconnected")),
        ]
    );
    // Events reach the handler, and quitting ends the session.
    host.event(AppEvent::SelectionChanged(Empty {}));
    host.finish().unwrap();
}

#[test]
fn a_request_to_a_host_that_went_away_fails_instead_of_waiting() {
    let (tx, rx) = mpsc::channel();
    let extension = Extension::new().on_event(move |event, host: &Host| {
        if matches!(event, AppEvent::CatalogClosed(_)) {
            let _ = tx.send(host.clone());
        }
    });
    let host = MockHost::start(extension);
    host.hello().unwrap();
    host.event(AppEvent::CatalogClosed(Empty {}));
    let program_host = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(program_host.session(), 1);
    host.finish().unwrap();
    // The program's reader sees the channel close soon after the session ends;
    // from then on every request fails at once.
    let closed = (0..500).any(|_| {
        let failed = program_host
            .request_timeout("read.selection", Value::Null, Duration::from_millis(10))
            .is_err_and(|e| e.code == ErrorCode::Exited);
        failed || {
            std::thread::sleep(Duration::from_millis(10));
            false
        }
    });
    assert!(closed);
}

/// `serve(reader, writer)` keeps phase 2's signature, so custom transports and
/// test harnesses written against it still compile.
#[test]
fn serve_keeps_its_signature() {
    let _: fn(Extension, std::io::Empty, std::io::Sink) -> std::io::Result<()> = Extension::serve;
    let _: fn(Extension, std::io::Empty, std::io::Sink, ServeOptions) -> std::io::Result<()> =
        Extension::serve_with_options;
}

#[test]
fn a_mode_the_sdk_does_not_serve_is_refused_before_reading() {
    for api in [0, rawmakase_extension::protocol::manifest::API + 1] {
        let error = Extension::new()
            .serve_with_options(std::io::empty(), std::io::sink(), ServeOptions { api })
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{api}");
    }
    assert_eq!(ServeOptions::default().api, 1);
}

/// The tests that map buffers take this lock: the SDK's count of mapped
/// buffers is the process's, and one of them checks it.
fn mapping() -> std::sync::MutexGuard<'static, ()> {
    static MAPPING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    MAPPING.lock().unwrap_or_else(|e| e.into_inner())
}

/// A generator that records what it was asked, needs setup once, finds
/// nothing without input, and writes each pixel's red as its coverage.
struct Subject {
    installed: bool,
    asked: mpsc::Sender<MaskGenerate>,
}
impl MaskGenerator for Subject {
    fn generate(&mut self, _cx: &Context, ask: MaskAsk<'_>) -> Result<MaskGenerated, RemoteError> {
        if !self.installed {
            return Err(NeedsSetup {
                what: "the Subject model".into(),
                bytes: 184_000_000,
            }
            .error());
        }
        let (image, coverage) = (&ask.request.image, &ask.request.coverage);
        for y in 0..image.height as usize {
            for x in 0..image.width as usize {
                ask.coverage[y * coverage.stride as usize + x] =
                    ask.image[y * image.stride as usize + x * 3];
            }
        }
        let empty = ask.request.input.is_none().then_some(EmptyMask::NoSubject);
        let _ = self.asked.send(ask.request);
        Ok(MaskGenerated {
            model: "sam2.1-hiera-small@0123456789abcdef".into(),
            model_key: "sam2.1/v3".into(),
            empty,
        })
    }
    fn setup(&mut self, cx: &Context, generator: &str) -> Result<(), RemoteError> {
        assert_eq!(generator, "subject");
        cx.progress(Some(1.0), Some("Downloaded"));
        self.installed = true;
        Ok(())
    }
}

/// A 64x48 image whose red is `(x + y) % 256`.
fn image() -> Vec<u8> {
    (0..48usize)
        .flat_map(|y| (0..64usize).flat_map(move |x| [((x + y) % 256) as u8, 7, 9]))
        .collect()
}

/// The coverage [`Subject`] writes for [`image`].
fn expected_coverage() -> Vec<u8> {
    (0..48usize)
        .flat_map(|y| (0..64usize).map(move |x| ((x + y) % 256) as u8))
        .collect()
}

/// A mask request for a 64x48 image, as the app will send it.
fn mask_request(input: Option<ViewerInput>) -> MaskGenerate {
    let region = Region {
        space: Space::ImageFrame,
        rect: [0.0, 0.0, 1.0, 1.0],
        scale: 0.05,
        overlap: 0,
    };
    let image = Buffer {
        handle: 0,
        bytes: 64 * 48 * 3,
        width: 64,
        height: 48,
        stride: 64 * 3,
        channels: 3,
        sample: Sample::U8,
        meaning: Meaning::Color {
            primaries: Primaries::Srgb,
            white: White::D65,
            transfer: Transfer::Srgb,
            alpha: Alpha::None,
        },
        region,
        access: Access::Read,
    };
    let coverage = Buffer {
        handle: 1,
        bytes: 64 * 48,
        stride: 64,
        channels: 1,
        meaning: Meaning::Coverage,
        access: Access::Write,
        ..image
    };
    MaskGenerate {
        generator: "subject".into(),
        input,
        image,
        coverage,
        image_key: "0123456789abcdef".into(),
    }
}

#[test]
fn a_mask_generator_sets_up_then_generates_from_checked_requests() {
    let _mapping = mapping();
    let (asked, requests) = mpsc::channel();
    let extension = Extension::new().mask_generator(
        "subject",
        Subject {
            installed: false,
            asked,
        },
    );
    let host =
        MockHost::start_with_options(extension, ServeOptions { api: 2 }, |_, _| Ok(Value::Null));
    let hello = host.hello().unwrap();
    assert_eq!(hello.implements, ["mask-generator/subject"]);
    let prompt = ViewerInput::Prompt {
        clicks: vec![Click {
            x: 0.25,
            y: 0.5,
            positive: true,
        }],
        bounds: Some(Bounds {
            x0: 0.1,
            y0: 0.1,
            x1: 0.9,
            y1: 0.9,
        }),
    };
    // Not installed: the setup it needs travels as the error's message.
    let needs = host
        .generate_mask(mask_request(Some(prompt.clone())), &image())
        .unwrap_err();
    assert_eq!(needs.code, ErrorCode::NeedsSetup);
    let setup = NeedsSetup::read(&needs.message).unwrap();
    assert_eq!(setup.bytes, 184_000_000);
    host.setup_mask("subject").unwrap();
    assert!(
        host.recorded()
            .iter()
            .any(|r| matches!(r, Recorded::Progress { done: Some(d), .. } if *d == 1.0))
    );
    // Installed: the request reaches the handler whole, descriptors as sent,
    // with the image's pixels, and the coverage it writes comes back.
    let (result, coverage) = host
        .generate_mask(mask_request(Some(prompt.clone())), &image())
        .unwrap();
    assert_eq!(coverage, expected_coverage());
    assert_eq!(rawmakase_extension::mapped_buffers(), 0);
    assert_eq!(result.empty, None);
    assert_eq!(result.model_key, "sam2.1/v3");
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(10)).unwrap(),
        mask_request(Some(prompt))
    );
    let (automatic, _) = host.generate_mask(mask_request(None), &image()).unwrap();
    assert_eq!(automatic.empty, Some(EmptyMask::NoSubject));
    requests.recv_timeout(Duration::from_secs(10)).unwrap();
    // A request that doesn't validate never reaches the handler.
    let outside = mask_request(Some(ViewerInput::Point { x: 1.5, y: 0.5 }));
    let mut short = mask_request(None);
    short.coverage.bytes = 10;
    for request in [outside, short] {
        assert_eq!(
            host.generate_mask(request, &image()).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }
    assert!(requests.try_recv().is_err());
    // Generators it doesn't implement.
    let mut sky = mask_request(None);
    sky.generator = "sky".into();
    assert_eq!(
        host.generate_mask(sky, &image()).unwrap_err().code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        host.setup_mask("sky").unwrap_err().code,
        ErrorCode::InvalidRequest
    );
    host.finish().unwrap();
}

#[test]
fn the_mock_refuses_a_mask_result_the_app_would_refuse() {
    let _mapping = mapping();
    let extension = Extension::new().mask_generator("subject", LongModel);
    let host =
        MockHost::start_with_options(extension, ServeOptions { api: 2 }, |_, _| Ok(Value::Null));
    host.hello().unwrap();
    let error = host
        .generate_mask(mask_request(None), &image())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("model"), "{error}");
    host.finish().unwrap();
}

#[test]
fn an_api_1_mock_passes_no_handles() {
    let host = MockHost::start(Extension::new().mask_generator("subject", LongModel));
    host.hello().unwrap();
    let error = host
        .generate_mask(mask_request(None), &image())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("API 1"), "{error}");
    host.finish().unwrap();
}

struct LongModel;
impl MaskGenerator for LongModel {
    fn generate(&mut self, _cx: &Context, _: MaskAsk<'_>) -> Result<MaskGenerated, RemoteError> {
        Ok(MaskGenerated {
            model: "m".repeat(129),
            model_key: "k".into(),
            empty: None,
        })
    }
}

/// Requests a [`Buffer`] can describe but the mask contract doesn't take: each
/// buffer is valid on its own, so only the mask shape refuses them.
fn off_contract() -> Vec<(&'static str, MaskGenerate)> {
    let base = || mask_request(None);
    let with = |name, change: &dyn Fn(&mut MaskGenerate)| {
        let mut request = base();
        change(&mut request);
        request.image.validate().unwrap();
        request.coverage.validate().unwrap();
        (name, request)
    };
    let color = |primaries, white, transfer, alpha| Meaning::Color {
        primaries,
        white,
        transfer,
        alpha,
    };
    vec![
        with("u16 image", &|r| {
            r.image.sample = Sample::U16;
            r.image.stride = 64 * 3 * 2;
            r.image.bytes = 64 * 3 * 2 * 48;
        }),
        with("rgba image", &|r| {
            r.image.channels = 4;
            r.image.meaning = color(Primaries::Srgb, White::D65, Transfer::Srgb, Alpha::Straight);
            r.image.stride = 64 * 4;
            r.image.bytes = 64 * 4 * 48;
        }),
        with("linear image", &|r| {
            r.image.meaning = color(Primaries::Srgb, White::D65, Transfer::Linear, Alpha::None);
        }),
        with("camera primaries", &|r| {
            r.image.meaning = color(
                Primaries::Camera,
                White::Camera,
                Transfer::Linear,
                Alpha::None,
            );
        }),
        with("f32 coverage", &|r| {
            r.coverage.sample = Sample::F32;
            r.coverage.stride = 64 * 4;
            r.coverage.bytes = 64 * 4 * 48;
        }),
        with("output space", &|r| {
            r.image.region.space = Space::Output;
            r.coverage.region.space = Space::Output;
        }),
        with("partial rect", &|r| {
            r.image.region.rect = [0.0, 0.0, 0.5, 1.0];
            r.coverage.region.rect = [0.0, 0.0, 0.5, 1.0];
        }),
        with("overlap", &|r| {
            r.image.region.overlap = 1;
            r.coverage.region.overlap = 1;
        }),
    ]
}

#[test]
fn a_request_off_the_mask_contract_never_reaches_the_handler() {
    let _mapping = mapping();
    let (asked, requests) = mpsc::channel();
    let extension = Extension::new().mask_generator(
        "subject",
        Subject {
            installed: true,
            asked,
        },
    );
    let host =
        MockHost::start_with_options(extension, ServeOptions { api: 2 }, |_, _| Ok(Value::Null));
    host.hello().unwrap();
    for (name, request) in off_contract() {
        let error = host.generate_mask(request, &image()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{name}");
    }
    assert!(requests.try_recv().is_err());
    // The contract's own shape still gets through.
    host.generate_mask(mask_request(None), &image()).unwrap();
    requests.recv_timeout(Duration::from_secs(10)).unwrap();
    host.finish().unwrap();
}
