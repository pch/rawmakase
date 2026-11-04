//! The client and the server, talking for real over a loopback socket, with a
//! fake engine standing in for the models: what the app sees when a selection
//! succeeds, fails, is cancelled, arrives stale, or the worker is fed nonsense.
use rawmakase_cancel::Cancel;

use rawmakase_inference::{Coverage, Prompt, RgbImage};
use rawmakase_remote::client::{ClientError, RemoteOptions, RemoteSession};
use rawmakase_remote::protocol::{Op, Request, WirePrompt, write_frame};
use rawmakase_remote::server::{Engine, EngineError, Outcome, Server};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// A selection the tests can shape: what it returns, how long it takes, and
/// what it saw.
struct Fake {
    /// The analysis the fake "computed", as the key and the two coverages.
    photo: Photo,
    /// Blocks inside `analyze` until raised, standing in for a model run.
    holding: Option<Arc<AtomicBool>>,
    /// Fail with this instead.
    fail: Option<EngineError>,
    /// Sleep before answering.
    delay: Duration,
    /// Set when the fake saw its cancel flag raised.
    cancelled: Arc<AtomicBool>,
    /// How many `analyze` runs happened (cache hits included).
    runs: Arc<AtomicU64>,
}

#[derive(Clone)]
struct Photo {
    key: String,
    subject: Coverage,
    sky: Coverage,
}

impl Photo {
    fn of(key: &str, width: usize, height: usize, fill: u8) -> Self {
        Self {
            key: key.to_string(),
            subject: Coverage {
                width,
                height,
                data: vec![fill; width * height],
            },
            sky: Coverage {
                width,
                height,
                data: vec![fill.wrapping_add(1); width * height],
            },
        }
    }
}

impl Fake {
    fn of(photo: Photo) -> Self {
        Self {
            photo,
            holding: None,
            fail: None,
            delay: Duration::ZERO,
            cancelled: Arc::new(AtomicBool::new(false)),
            runs: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl Engine for Fake {
    fn analyze(&mut self, _photo: &RgbImage, cancel: Cancel<'_>) -> Result<Outcome, EngineError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        if let Some(holding) = &self.holding {
            // Hold until the test says go, so a cancel can arrive mid-run.
            while holding.load(Ordering::SeqCst) && !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        std::thread::sleep(self.delay);
        if cancel.is_cancelled() {
            self.cancelled.store(true, Ordering::SeqCst);
            return Err(EngineError::Cancelled);
        }
        if let Some(fail) = self.fail.clone() {
            return Err(fail);
        }
        Ok(Outcome {
            key: self.photo.key.clone(),
            subject: self.photo.subject.clone(),
        })
    }

    fn segment(
        &mut self,
        key: &str,
        _prompt: &Prompt,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, EngineError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        if key != self.photo.key {
            return Err(EngineError::UnknownKey);
        }
        if let Some(fail) = self.fail.clone() {
            return Err(fail);
        }
        Ok(self.photo.subject.clone())
    }

    fn sky(&mut self, _photo: &RgbImage, cancel: Cancel<'_>) -> Result<Coverage, EngineError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        if let Some(holding) = &self.holding {
            while holding.load(Ordering::SeqCst) && !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        if let Some(fail) = self.fail.clone() {
            return Err(fail);
        }
        Ok(self.photo.sky.clone())
    }

    fn describe(&self) -> rawmakase_remote::server::Info {
        rawmakase_remote::server::Info {
            model: "fake@0".into(),
            panoptic: "fake-panoptic".into(),
            sky: "fake-sky@0".into(),
            provider: "CPU".into(),
        }
    }
}

/// A worker on an ephemeral port, returning how to reach it and the fake.
fn worker(engine: Fake) -> (String, Arc<AtomicBool>, Arc<AtomicU64>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let cancelled = engine.cancelled.clone();
    let runs = engine.runs.clone();
    let server = Arc::new(Server::new(engine));
    std::thread::spawn(move || server.serve(listener));
    (endpoint, cancelled, runs)
}

fn photo(width: usize, height: usize) -> RgbImage {
    RgbImage {
        width,
        height,
        data: (0..width * height * 3).map(|i| (i % 251) as u8).collect(),
    }
}

fn click(x: f32, y: f32) -> Prompt {
    Prompt {
        points: vec![rawmakase_inference::Point {
            x,
            y,
            positive: true,
        }],
        bounds: None,
    }
}

#[test]
fn a_selection_round_trips_and_a_second_one_reuses_the_analysis() {
    let fake = Fake::of(Photo::of("k1", 4, 3, 200));
    let runs = fake.runs.clone();
    let (endpoint, _, _) = worker(fake);
    let session = RemoteSession::new(&endpoint);
    let info = session.ping().unwrap();
    assert_eq!(info.model, "fake@0");
    assert_eq!(info.provider, "CPU");

    let cancel = AtomicBool::new(false);
    let analysis = session.analyze(&photo(4, 3), Cancel::new(&cancel)).unwrap();
    assert_eq!(&analysis.key, "k1");
    assert_eq!(analysis.subject.data, vec![200; 12]);
    assert_eq!(
        session
            .sky(&photo(4, 3), Cancel::new(&cancel))
            .unwrap()
            .data,
        vec![201; 12]
    );

    let mask = session
        .segment(&analysis, &click(0.5, 0.5), Cancel::new(&cancel))
        .unwrap();
    assert_eq!(mask, analysis.subject);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        3,
        "analyze, sky and segment; a ping runs no model"
    );
}

#[test]
fn a_cancelled_selection_says_so_and_stops_the_worker() {
    let mut fake = Fake::of(Photo::of("k1", 2, 2, 1));
    let holding = Arc::new(AtomicBool::new(true));
    fake.holding = Some(holding);
    let engine_cancelled = fake.cancelled.clone();
    let (endpoint, _, _) = worker(fake);
    let session = RemoteSession::new(&endpoint);

    let cancel = Arc::new(AtomicBool::new(false));
    let watcher = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            cancel.store(true, Ordering::SeqCst);
        })
    };
    let started = std::time::Instant::now();
    let error = session
        .analyze(&photo(2, 2), Cancel::new(&cancel))
        .unwrap_err();
    watcher.join().unwrap();
    assert_eq!(error, ClientError::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the cancel was waited for, not a timeout"
    );
    // The request is over: the worker takes new ones. The ping waits for the
    // engine, and the held run only ends on the forwarded flag, so once it is
    // answered the flag has been seen; checked before it, the flag can still
    // be on its way on the cancel connection.
    assert!(session.ping().is_ok());
    assert!(
        engine_cancelled.load(Ordering::SeqCst),
        "the worker's run saw the forwarded flag"
    );
}

#[test]
fn a_worker_that_fails_is_an_error_the_app_can_show() {
    let mut fake = Fake::of(Photo::of("k1", 2, 2, 1));
    fake.fail = Some(EngineError::Failed(
        rawmakase_inference::InferenceError::Failed("the model is broken".into()),
    ));
    let (endpoint, _, _) = worker(fake);
    let session = RemoteSession::new(&endpoint);
    let cancel = AtomicBool::new(false);
    let error = session
        .analyze(&photo(2, 2), Cancel::new(&cancel))
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Failed(why) if why.contains("the model is broken")),
        "{error:?}"
    );
}

#[test]
fn a_stale_click_after_the_worker_forgot_the_photo_asks_for_it_again() {
    let fake = Fake::of(Photo::of("k1", 2, 2, 1));
    let (endpoint, _, _) = worker(fake);
    let session = RemoteSession::new(&endpoint);
    let cancel = AtomicBool::new(false);
    let analysis = session.analyze(&photo(2, 2), Cancel::new(&cancel)).unwrap();
    let stale = rawmakase_remote::RemoteAnalysis {
        key: "gone".into(),
        subject: analysis.subject,
    };
    assert_eq!(
        session.segment(&stale, &click(0.5, 0.5), Cancel::new(&cancel)),
        Err(ClientError::Expired)
    );
    // And a cancel for a request that no longer runs is acknowledged, not an error.
    let request = Request {
        v: rawmakase_remote::protocol::VERSION,
        id: "late".into(),
        op: Op::Cancel {
            target: "no-such-request".into(),
        },
    };
    let mut stream = std::net::TcpStream::connect(&endpoint).unwrap();
    write_frame(&mut stream, &serde_json::to_vec(&request).unwrap()).unwrap();
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).unwrap();
    let mut reply = vec![0u8; u32::from_le_bytes(length) as usize];
    stream.read_exact(&mut reply).unwrap();
    let answered: rawmakase_remote::Response = serde_json::from_slice(&reply).unwrap();
    assert!(answered.ok.is_some(), "{answered:?}");
}

#[test]
fn an_unreachable_worker_is_reported_as_such() {
    // Port 1 on the loopback is not ours to listen on.
    let session = RemoteSession::with_options(
        "127.0.0.1:1",
        RemoteOptions {
            connect_timeout: Duration::from_millis(500),
            request_timeout: Duration::from_secs(5),
        },
    );
    let error = session.ping().unwrap_err();
    assert!(matches!(error, ClientError::Unavailable(_)), "{error:?}");
}

/// Two sessions on one worker: their requests run side by side in the queue,
/// their ids never collide, and a cancel meant for one leaves the other's
/// request and cancel registry entry alone.
#[test]
fn two_sessions_never_confuse_each_others_requests_or_cancels() {
    // One engine, holding each analyze until its flag says go.
    struct Gated {
        gate: Arc<AtomicBool>,
    }
    impl Engine for Gated {
        fn analyze(
            &mut self,
            _photo: &RgbImage,
            cancel: Cancel<'_>,
        ) -> Result<Outcome, EngineError> {
            while !self.gate.load(Ordering::SeqCst) && !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            if cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            Ok(Outcome {
                key: "k".into(),
                subject: Coverage {
                    width: 2,
                    height: 2,
                    data: vec![7; 4],
                },
            })
        }
        fn segment(
            &mut self,
            _key: &str,
            _prompt: &Prompt,
            _cancel: Cancel<'_>,
        ) -> Result<Coverage, EngineError> {
            Err(EngineError::UnknownKey)
        }
        fn sky(&mut self, _photo: &RgbImage, _cancel: Cancel<'_>) -> Result<Coverage, EngineError> {
            Err(EngineError::UnknownKey)
        }
        fn describe(&self) -> rawmakase_remote::server::Info {
            rawmakase_remote::server::Info {
                model: "fake@0".into(),
                panoptic: "fake".into(),
                sky: String::new(),
                provider: "CPU".into(),
            }
        }
    }
    let gate = Arc::new(AtomicBool::new(false));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    {
        let gate = gate.clone();
        std::thread::spawn(move || Arc::new(Server::new(Gated { gate })).serve(listener))
    };

    // Both sessions queue their analyze; the ids differ even though both
    // counters start at one.
    let first_cancel = Arc::new(AtomicBool::new(false));
    let second_cancel = Arc::new(AtomicBool::new(false));
    let first = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            RemoteSession::new(&endpoint).analyze(&photo(2, 2), Cancel::new(&first_cancel))
        });
        let b = scope.spawn(|| {
            RemoteSession::new(&endpoint).analyze(&photo(2, 2), Cancel::new(&second_cancel))
        });
        // The engine is gated shut: cancel the first while it runs.
        std::thread::sleep(Duration::from_millis(150));
        first_cancel.store(true, Ordering::SeqCst);
        let a = a.join().unwrap();
        // The second runs only when the first has given up.
        gate.store(true, Ordering::SeqCst);
        (a, b.join().unwrap())
    });
    assert_eq!(
        first.0,
        Err(ClientError::Cancelled),
        "the cancelled one says so"
    );
    let second = first.1.unwrap();
    assert_eq!(second.subject.data, vec![7; 4], "the other one still ran");
}

/// A worker that accepts the request and then never answers: the request ends
/// within its wall-clock budget, and a cancel ends it promptly, though the
/// worker itself cannot be interrupted.
#[test]
fn a_silent_worker_is_bounded_and_a_cancel_is_prompt() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    // A "worker" that reads frames and says nothing, forever.
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                let mut sink = [0u8; 4096];
                while let Ok(n) = stream.read(&mut sink) {
                    if n == 0 {
                        break;
                    }
                }
            });
        }
    });

    // The wall-clock budget: one request, answered never, ends in time.
    let budget = Duration::from_millis(700);
    let session = RemoteSession::with_options(
        &endpoint,
        RemoteOptions {
            connect_timeout: Duration::from_secs(2),
            request_timeout: budget,
        },
    );
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let error = session
        .analyze(&photo(2, 2), Cancel::new(&cancel))
        .unwrap_err();
    let elapsed = started.elapsed();
    assert!(
        matches!(&error, ClientError::Failed(why) if why.contains("in time")),
        "{error:?}"
    );
    assert!(
        elapsed >= budget && elapsed < budget + Duration::from_secs(3),
        "ended in {elapsed:?}, budget {budget:?}"
    );

    // Prompt cancellation against the same silent worker.
    let cancel = Arc::new(AtomicBool::new(false));
    let watcher = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            cancel.store(true, Ordering::SeqCst);
        })
    };
    let started = std::time::Instant::now();
    let error = session
        .analyze(&photo(2, 2), Cancel::new(&cancel))
        .unwrap_err();
    watcher.join().unwrap();
    assert_eq!(error, ClientError::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "cancel took {:?}; it must not wait out the budget",
        started.elapsed()
    );
}

/// A worker that answers one byte every so often never extends its own
/// deadline: the request's budget is one wall-clock whole, not per read.
#[test]
fn a_trickling_worker_cannot_outlast_the_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let stream = listener.incoming().next().unwrap().unwrap();
        let mut stream = stream;
        let _ = stream.set_nodelay(true);
        // Answer the length prefix so slowly that per-read timeouts alone
        // would never fire.
        for byte in (4u32 * 3).to_le_bytes() {
            let _ = stream.write_all(&[byte]);
            std::thread::sleep(Duration::from_millis(300));
        }
        loop {
            let _ = stream.write_all(&[0]);
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    let session = RemoteSession::with_options(
        &endpoint,
        RemoteOptions {
            connect_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_millis(500),
        },
    );
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let error = session
        .analyze(&photo(2, 2), Cancel::new(&cancel))
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Failed(why) if why.contains("in time")),
        "{error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the trickle kept it alive {:?}",
        started.elapsed()
    );
}

/// The same, with the prefix arriving whole and the answer's body trickling:
/// the budget covers the whole request, the body included.
#[test]
fn a_trickling_body_cannot_outlast_the_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let mut stream = listener.incoming().next().unwrap().unwrap();
        let _ = stream.set_nodelay(true);
        // A prefix promising 100 bytes, delivered a byte at a time forever.
        let _ = stream.write_all(&100u32.to_le_bytes());
        loop {
            let _ = stream.write_all(&[0]);
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    let session = RemoteSession::with_options(
        &endpoint,
        RemoteOptions {
            connect_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_millis(500),
        },
    );
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let error = session
        .analyze(&photo(2, 2), Cancel::new(&cancel))
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Failed(why) if why.contains("in time")),
        "{error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the body trickle kept it alive {:?}",
        started.elapsed()
    );
}

/// A worker that sends a plausible prefix and hangs up mid-body is an error,
/// not a wait and not a loop.
#[test]
fn a_worker_hanging_up_mid_body_is_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let mut stream = listener.incoming().next().unwrap().unwrap();
        let _ = stream.set_nodelay(true);
        let _ = stream.write_all(&100u32.to_le_bytes());
        let _ = stream.write_all(&[0; 9]);
        // ...and that is all: the socket closes after nine of one hundred.
    });
    let session = RemoteSession::with_options(
        &endpoint,
        RemoteOptions {
            connect_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_secs(5),
        },
    );
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let error = session
        .analyze(&photo(2, 2), Cancel::new(&cancel))
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Failed(why) if why.contains("closed the connection")),
        "{error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the hang-up took {:?} to notice",
        started.elapsed()
    );
}

/// A second request reusing an id that is already live is refused, so a
/// cancel can never be stolen from the request that owns it.
#[test]
fn a_duplicate_live_id_is_refused() {
    // The engine holds the first analyze until the test says go, so its id
    // really is live when the duplicate arrives.
    struct Gated {
        gate: Arc<AtomicBool>,
    }
    impl Engine for Gated {
        fn analyze(
            &mut self,
            _photo: &RgbImage,
            cancel: Cancel<'_>,
        ) -> Result<Outcome, EngineError> {
            while !self.gate.load(Ordering::SeqCst) && !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            if cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            Ok(Outcome {
                key: "k".into(),
                subject: Coverage {
                    width: 2,
                    height: 2,
                    data: vec![1; 4],
                },
            })
        }
        fn segment(
            &mut self,
            _key: &str,
            _prompt: &Prompt,
            _cancel: Cancel<'_>,
        ) -> Result<Coverage, EngineError> {
            Err(EngineError::UnknownKey)
        }
        fn sky(&mut self, _photo: &RgbImage, _cancel: Cancel<'_>) -> Result<Coverage, EngineError> {
            Err(EngineError::UnknownKey)
        }
        fn describe(&self) -> rawmakase_remote::server::Info {
            rawmakase_remote::server::Info {
                model: "fake@0".into(),
                panoptic: "fake".into(),
                sky: String::new(),
                provider: "CPU".into(),
            }
        }
    }
    let gate = Arc::new(AtomicBool::new(false));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    {
        let gate = gate.clone();
        std::thread::spawn(move || Arc::new(Server::new(Gated { gate })).serve(listener));
    }
    let request = |id: &str| {
        serde_json::to_vec(&Request {
            v: rawmakase_remote::protocol::VERSION,
            id: id.to_string(),
            op: Op::Analyze {
                width: 2,
                height: 2,
                pixels: photo(2, 2).data,
            },
        })
        .unwrap()
    };
    // The first id is running: its connection waits on the gated engine.
    let mut holder = std::net::TcpStream::connect(&endpoint).unwrap();
    write_frame(&mut holder, &request("dup-1")).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    // A second connection reusing the live id is turned away.
    let mut stream = std::net::TcpStream::connect(&endpoint).unwrap();
    write_frame(&mut stream, &request("dup-1")).unwrap();
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).unwrap();
    let mut reply = vec![0u8; u32::from_le_bytes(length) as usize];
    stream.read_exact(&mut reply).unwrap();
    let answer: rawmakase_remote::Response = serde_json::from_slice(&reply).unwrap();
    assert_eq!(
        answer.err.as_ref().map(|e| e.code),
        Some(rawmakase_remote::protocol::Code::Invalid),
        "{answer:?}"
    );
    // The first request is untouched by the refusal: open the gate and it
    // completes normally, its cancel registry entry still its own.
    gate.store(true, Ordering::SeqCst);
    let mut length = [0u8; 4];
    holder.read_exact(&mut length).unwrap();
    let mut reply = vec![0u8; u32::from_le_bytes(length) as usize];
    holder.read_exact(&mut reply).unwrap();
    let done: rawmakase_remote::Response = serde_json::from_slice(&reply).unwrap();
    assert!(
        done.ok.is_some(),
        "the cancel raced the gate; the request finished first: {done:?}"
    );
}

#[test]
fn a_worker_that_takes_too_long_is_an_error_not_a_hang() {
    let mut fake = Fake::of(Photo::of("k1", 2, 2, 1));
    fake.delay = Duration::from_secs(2);
    let (endpoint, _, _) = worker(fake);
    let session = RemoteSession::with_options(
        &endpoint,
        RemoteOptions {
            connect_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_millis(150),
        },
    );
    let cancel = AtomicBool::new(false);
    let error = session
        .analyze(&photo(2, 2), Cancel::new(&cancel))
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Failed(why) if why.contains("in time")),
        "{error:?}"
    );
}

#[test]
fn malformed_requests_are_refused_without_touching_the_engine() {
    let fake = Fake::of(Photo::of("k1", 2, 2, 1));
    let runs = fake.runs.clone();
    let (endpoint, _, _) = worker(fake);
    let send = |frame: &[u8]| {
        let mut stream = std::net::TcpStream::connect(&endpoint).unwrap();
        write_frame(&mut stream, frame).unwrap();
        let mut length = [0u8; 4];
        match stream.read_exact(&mut length) {
            Ok(()) => {
                let mut reply = vec![0u8; u32::from_le_bytes(length) as usize];
                stream.read_exact(&mut reply).unwrap();
                let response: rawmakase_remote::Response = serde_json::from_slice(&reply).unwrap();
                response
            }
            // The worker hung up: nothing was sent back.
            Err(_) => rawmakase_remote::Response::failure(
                "",
                rawmakase_remote::protocol::Code::Failed,
                "connection closed".into(),
            ),
        }
    };

    // A frame of nonsense JSON.
    let answer = send(b"{not json");
    assert_eq!(
        answer.err.as_ref().map(|e| e.code),
        Some(rawmakase_remote::protocol::Code::Invalid)
    );
    // A photo whose dimensions and bytes disagree.
    let request = Request {
        v: rawmakase_remote::protocol::VERSION,
        id: "m1".into(),
        op: Op::Analyze {
            width: 100,
            height: 100,
            pixels: vec![0; 6],
        },
    };
    let answer = send(&serde_json::to_vec(&request).unwrap());
    assert_eq!(
        answer.err.as_ref().map(|e| e.code),
        Some(rawmakase_remote::protocol::Code::Invalid)
    );
    // A zero-sided photo.
    let request = Request {
        v: rawmakase_remote::protocol::VERSION,
        id: "m2".into(),
        op: Op::Analyze {
            width: 0,
            height: 0,
            pixels: vec![],
        },
    };
    let answer = send(&serde_json::to_vec(&request).unwrap());
    assert_eq!(
        answer.err.as_ref().map(|e| e.code),
        Some(rawmakase_remote::protocol::Code::Invalid)
    );
    // A frame claiming an impossible length: the worker refuses it unread and
    // moves on to the next client.
    let mut stream = std::net::TcpStream::connect(&endpoint).unwrap();
    write_frame(
        &mut stream,
        &(rawmakase_remote::protocol::MAX_FRAME_BYTES as u64 + 8).to_le_bytes(),
    )
    .unwrap();
    let mut length = [0u8; 4];
    let refused = stream.read_exact(&mut length).is_ok();
    assert!(refused, "the worker says why before hanging up");
    let mut reply = vec![0u8; u32::from_le_bytes(length) as usize];
    stream.read_exact(&mut reply).unwrap();
    let response: rawmakase_remote::Response = serde_json::from_slice(&reply).unwrap();
    assert_eq!(
        response.err.map(|e| e.code),
        Some(rawmakase_remote::protocol::Code::Invalid)
    );
    // Another protocol version.
    let request = Request {
        v: 99,
        id: "m3".into(),
        op: Op::Ping,
    };
    let answer = send(&serde_json::to_vec(&request).unwrap());
    assert_eq!(
        answer.err.as_ref().map(|e| e.code),
        Some(rawmakase_remote::protocol::Code::Unsupported)
    );
    // A prompt no model may see.
    let request = Request {
        v: rawmakase_remote::protocol::VERSION,
        id: "m4".into(),
        op: Op::Segment {
            key: "k1".into(),
            prompt: WirePrompt {
                points: vec![],
                bounds: None,
            },
        },
    };
    let answer = send(&serde_json::to_vec(&request).unwrap());
    assert_eq!(
        answer.err.as_ref().map(|e| e.code),
        Some(rawmakase_remote::protocol::Code::Invalid)
    );
    // None of the above reached the engine.
    assert_eq!(runs.load(Ordering::SeqCst), 0, "{runs:?}");
}

/// Reads one reply frame, failing the test instead of hanging when none comes.
fn reply_within(stream: &mut std::net::TcpStream, wait: Duration) -> rawmakase_remote::Response {
    stream.set_read_timeout(Some(wait)).unwrap();
    let mut length = [0u8; 4];
    stream
        .read_exact(&mut length)
        .expect("the worker answered in time");
    let mut reply = vec![0u8; u32::from_le_bytes(length) as usize];
    stream.read_exact(&mut reply).unwrap();
    serde_json::from_slice(&reply).unwrap()
}

/// The client's watcher sends its cancel on a connection of its own, so it can
/// reach the worker before the request it cancels has been read. That request
/// still stops, instead of running the model to the end for nobody.
#[test]
fn a_cancel_that_overtakes_its_request_still_stops_it() {
    let mut fake = Fake::of(Photo::of("k1", 2, 2, 1));
    // Without the cancel, the run would hold forever.
    fake.holding = Some(Arc::new(AtomicBool::new(true)));
    let (endpoint, engine_cancelled, _) = worker(fake);
    let frame = |id: &str, op: Op| {
        serde_json::to_vec(&Request {
            v: rawmakase_remote::protocol::VERSION,
            id: id.to_string(),
            op,
        })
        .unwrap()
    };

    let mut canceller = std::net::TcpStream::connect(&endpoint).unwrap();
    write_frame(
        &mut canceller,
        &frame(
            "cancel-early",
            Op::Cancel {
                target: "early".into(),
            },
        ),
    )
    .unwrap();
    assert!(
        reply_within(&mut canceller, Duration::from_secs(5))
            .ok
            .is_some()
    );

    let mut stream = std::net::TcpStream::connect(&endpoint).unwrap();
    write_frame(
        &mut stream,
        &frame(
            "early",
            Op::Analyze {
                width: 2,
                height: 2,
                pixels: photo(2, 2).data,
            },
        ),
    )
    .unwrap();
    let answer = reply_within(&mut stream, Duration::from_secs(5));
    assert_eq!(
        answer.err.as_ref().map(|e| e.code),
        Some(rawmakase_remote::protocol::Code::Cancelled),
        "{answer:?}"
    );
    assert!(engine_cancelled.load(Ordering::SeqCst));
}

/// A cancel the request loop notices before the watcher does is still
/// forwarded: the worker hears about every cancelled request.
#[test]
fn a_cancel_is_forwarded_even_when_the_request_notices_it_first() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let (seen, cancels) = std::sync::mpsc::channel();
    // A "worker" that reports every cancel it is sent and answers nothing.
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let seen = seen.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                if let Ok(Some(frame)) = rawmakase_remote::protocol::read_frame(
                    &mut stream,
                    rawmakase_remote::protocol::MAX_FRAME_BYTES,
                ) && let Ok(request) = serde_json::from_slice::<Request>(&frame)
                    && let Op::Cancel { target } = request.op
                {
                    let _ = seen.send(target);
                }
            });
        }
    });
    let session = RemoteSession::new(&endpoint);
    // Raised before the request starts: the request loop sees it at once.
    let cancel = AtomicBool::new(true);
    assert_eq!(
        session.analyze(&photo(2, 2), Cancel::new(&cancel)),
        Err(ClientError::Cancelled)
    );
    let target = cancels
        .recv_timeout(Duration::from_secs(5))
        .expect("the cancel was forwarded");
    assert!(!target.is_empty());
}

#[test]
fn sky_runs_by_itself_with_no_analysis_before_it() {
    let (endpoint, _, runs) = worker(Fake::of(Photo::of("k1", 2, 2, 50)));
    let session = RemoteSession::new(&endpoint);
    let cancel = AtomicBool::new(false);
    // Sky by itself: one model run, nothing kept for it to depend on.
    assert_eq!(
        session
            .sky(&photo(2, 2), Cancel::new(&cancel))
            .unwrap()
            .data,
        vec![51; 4]
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(session.ping().unwrap().sky, "fake-sky@0");
}

#[test]
fn a_cancelled_sky_is_cancelled_and_a_worker_without_the_model_says_why() {
    let mut fake = Fake::of(Photo::of("k1", 2, 2, 50));
    let holding = Arc::new(AtomicBool::new(true));
    fake.holding = Some(holding.clone());
    let (endpoint, _, _) = worker(fake);
    let cancel = Arc::new(AtomicBool::new(false));
    let raised = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        raised.store(true, Ordering::SeqCst);
    });
    assert_eq!(
        RemoteSession::new(&endpoint).sky(&photo(2, 2), Cancel::new(&cancel)),
        Err(ClientError::Cancelled)
    );
    holding.store(false, Ordering::SeqCst);

    let mut fake = Fake::of(Photo::of("k1", 2, 2, 50));
    fake.fail = Some(EngineError::Failed(
        rawmakase_inference::InferenceError::ModelInvalid(
            "this worker has no sky model; start it with --sky-model".into(),
        ),
    ));
    let (endpoint, _, _) = worker(fake);
    match RemoteSession::new(&endpoint).sky(&photo(2, 2), Cancel::NEVER) {
        Err(ClientError::Failed(why)) => assert!(why.contains("--sky-model"), "{why}"),
        other => panic!("{other:?}"),
    }
}
