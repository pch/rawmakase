//! The real models, local against remote: the same pixels through the app's
//! local session and through the worker protocol must come out the same, byte
//! for byte — the remote worker runs the very same sessions, so anything less
//! means the protocol lost something.
//!
//! Ignored by default like the app's model tests; run with the models' folder
//! and the runtime library:
//!
//! ```sh
//! RAWMAKASE_TEST_MODEL=$HOME/.local/share/rawmakase/models/sam2.1-hiera-small-v1 \
//! RAWMAKASE_ORT_LIB=/path/to/libonnxruntime.so \
//! cargo test -p rawmakase-remote --test models -- --ignored
//! ```
use rawmakase_cancel::Cancel;

use rawmakase_inference::{Coverage, LoadOptions, Point, Prompt, Provider, RgbImage, Sky, Subject};
use rawmakase_remote::client::RemoteSession;
use rawmakase_remote::server::{Model, Server};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// A photo the models see something in: a bright blob on a dark ground, sized
/// like the app's model input. Synthetic on purpose: no private photograph
/// belongs in a repository.
fn photo(width: usize, height: usize) -> RgbImage {
    let mut data = vec![30u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let inside = x > width / 3 && x < 2 * width / 3 && y > height / 4 && y < 3 * height / 4;
            if inside {
                let shade = 180 + ((x + y) % 3) as u8 * 20;
                let at = (y * width + x) * 3;
                data[at..at + 3].copy_from_slice(&[shade, shade, shade]);
            }
        }
    }
    RgbImage {
        width,
        height,
        data,
    }
}

fn click() -> Prompt {
    Prompt {
        points: vec![Point {
            x: 0.5,
            y: 0.5,
            positive: true,
        }],
        bounds: None,
    }
}

fn same(a: &Coverage, b: &Coverage, what: &str) {
    assert_eq!(a.width, b.width, "{what}: width");
    assert_eq!(a.height, b.height, "{what}: height");
    let differing = a.data.iter().zip(&b.data).filter(|(x, y)| x != y).count();
    assert_eq!(differing, 0, "{what}: {differing} bytes differ");
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_MODEL (the models' folder) and RAWMAKASE_ORT_LIB"]
fn the_remote_worker_returns_what_the_local_session_computed() {
    let dir = std::path::PathBuf::from(std::env::var("RAWMAKASE_TEST_MODEL").unwrap());
    let options = LoadOptions {
        runtime_library: std::env::var_os("RAWMAKASE_ORT_LIB").map(std::path::PathBuf::from),
        threads: None,
        provider: Some(Provider::Cpu),
    };
    let cancel = AtomicBool::new(false);
    let image = photo(640, 426);

    // Local, as the app runs it.
    let mut session = Subject::load_with(&dir, &options).expect("the models load");
    let analysis = session
        .analyze(&image, Cancel::new(&cancel))
        .expect("analyze");
    let subject = analysis.subject().expect("subject");
    let clicked = session
        .segment(analysis.embedding(), &click(), Cancel::new(&cancel))
        .expect("segment");

    // Remote: the same sessions behind the worker protocol.
    let worker = Model::load(&dir, &options).expect("the worker's models load");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || Arc::new(Server::new(worker)).serve(listener));
    let remote = RemoteSession::new(&endpoint);
    let info = remote.ping().expect("ping");
    assert_eq!(info.provider, "CPU");
    let analysis = remote
        .analyze(&image, Cancel::new(&cancel))
        .expect("remote analyze");
    same(&analysis.subject, &subject, "automatic subject");
    let remote_click = remote
        .segment(&analysis, &click(), Cancel::new(&cancel))
        .expect("remote segment");
    same(&remote_click, &clicked, "clicked selection");
}

/// A real photograph, the real worker binary on a real socket: both automatic
/// selections find something (an empty subject and an empty sky would make the
/// equality above vacuous), and what the worker sends is what the local
/// session computed. The photo is read from the caller's disk and never
/// leaves this machine; the test is ignored unless a path is given.
#[test]
#[ignore = "needs RAWMAKASE_TEST_MODEL, RAWMAKASE_TEST_SKY_MODEL, RAWMAKASE_ORT_LIB and RAWMAKASE_TEST_RGB (an sRGB JPEG with sky)"]
fn a_real_photo_selects_the_same_through_the_worker_process() {
    use image::imageops::FilterType;

    let dir = std::path::PathBuf::from(std::env::var("RAWMAKASE_TEST_MODEL").unwrap());
    let options = LoadOptions {
        runtime_library: std::env::var_os("RAWMAKASE_ORT_LIB").map(std::path::PathBuf::from),
        threads: None,
        provider: Some(Provider::Cpu),
    };
    let cancel = AtomicBool::new(false);
    let decoded = image::open(std::env::var("RAWMAKASE_TEST_RGB").unwrap()).unwrap();
    // What the app feeds the models: 8-bit sRGB, longest side 1280.
    let edge = 1280.;
    let scale = edge / decoded.width().max(decoded.height()) as f32;
    let small = decoded.resize_to_fill(
        (decoded.width() as f32 * scale) as u32,
        (decoded.height() as f32 * scale) as u32,
        FilterType::Triangle,
    );
    let image = RgbImage {
        width: small.width() as usize,
        height: small.height() as usize,
        data: small.to_rgb8().into_raw(),
    };

    // Local, as the app runs it.
    let mut session = Subject::load_with(&dir, &options).expect("the models load");
    let analysis = session
        .analyze(&image, Cancel::new(&cancel))
        .expect("analyze");
    let subject = analysis.subject().expect("subject");
    let sky_dir = std::env::var("RAWMAKASE_TEST_SKY_MODEL").unwrap();
    let sky = Sky::load_with(std::path::Path::new(&sky_dir), &options)
        .expect("the sky model loads")
        .select(&image, Cancel::new(&cancel))
        .expect("sky");
    let chosen = |what: &str, coverage: &Coverage| {
        let share =
            coverage.data.iter().filter(|v| **v >= 128).count() as f32 / coverage.data.len() as f32;
        println!("{what}: {share:.3} of the photo selected");
        share
    };
    assert!(
        chosen("subject", &subject) > 0.005 && chosen("subject", &subject) < 0.95,
        "the model found nothing (or everything) for a subject"
    );
    assert!(
        chosen("sky", &sky) > 0.005 && chosen("sky", &sky) < 0.95,
        "the model found nothing (or everything) for the sky"
    );
    // The click aims at what the subject selection found, so the equality
    // below says something about a real mask, wherever the subject sits.
    let click = {
        let (mut x, mut y, mut n) = (0f32, 0f32, 0f32);
        for (i, v) in subject.data.iter().enumerate() {
            if *v >= 128 {
                x += (i % subject.width) as f32;
                y += (i / subject.width) as f32;
                n += 1.;
            }
        }
        Prompt {
            points: vec![Point {
                x: (x / n + 0.5) / subject.width as f32,
                y: (y / n + 0.5) / subject.height as f32,
                positive: true,
            }],
            bounds: None,
        }
    };
    let clicked = session
        .segment(analysis.embedding(), &click, Cancel::new(&cancel))
        .expect("segment");
    assert!(
        chosen("clicked", &clicked) > 0.005,
        "the click on the subject found nothing"
    );

    // The worker binary itself, on an ephemeral loopback port.
    let mut worker = std::process::Command::new(env!("CARGO_BIN_EXE_rawmakase-remote-worker"))
        .args([
            "--model",
            dir.to_str().unwrap(),
            "--sky-model",
            &sky_dir,
            "--listen",
            "127.0.0.1:0",
            "--provider",
            "cpu",
        ])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = worker.stderr.take().unwrap();
    let endpoint = {
        use std::io::BufRead;
        let mut endpoint = None;
        for line in std::io::BufReader::new(stderr).lines() {
            let line = line.unwrap();
            if let Some(at) = line.split("serving on ").nth(1) {
                endpoint = Some(at.trim().to_string());
                break;
            }
        }
        endpoint.expect("the worker announced its port")
    };

    let remote = RemoteSession::new(&endpoint);
    let info = remote.ping().expect("ping");
    assert_eq!(info.provider, "CPU", "{info:?}");
    let analysis = remote
        .analyze(&image, Cancel::new(&cancel))
        .expect("remote analyze");
    same(&analysis.subject, &subject, "automatic subject");
    same(
        &remote
            .sky(&image, Cancel::new(&cancel))
            .expect("remote sky"),
        &sky,
        "automatic sky",
    );
    let remote_click = remote
        .segment(&analysis, &click, Cancel::new(&cancel))
        .expect("remote segment");
    same(&remote_click, &clicked, "clicked selection");
    // Stopping is part of the test: nothing is left holding the port.
    worker.kill().unwrap();
    worker.wait().unwrap();
}
