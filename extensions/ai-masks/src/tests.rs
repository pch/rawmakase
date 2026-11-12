//! The extension through the SDK's mock host, over shared memory, with the
//! fake models ([`crate::fake`]): what each generator answers, setting up and
//! finding the models, the remote worker, and removing them.
use super::*;
use crate::fake::Fake;
use rawmakase_extension::ServeOptions;
use rawmakase_extension::mock::{MockHost, Recorded};
use rawmakase_extension::protocol::PROTOCOL;
use rawmakase_extension::protocol::buffer::{Access, Buffer, Meaning, Region, Sample, Space};
use rawmakase_extension::protocol::message::NeedsSetup;
use rawmakase_extension::protocol::method::{Hello, MaskGenerate};
use rawmakase_extension::protocol::viewer::{Bounds, Click};
use rawmakase_inference::InferenceError;
use rawmakase_model_install::{RECEIPT, receipt};
use std::path::Path;

const W: u32 = 8;
const H: u32 = 6;

struct Running {
    host: MockHost,
    fake: Fake,
    /// The extension's data folder.
    data: PathBuf,
    /// The app's local data folder.
    local: PathBuf,
    _dir: tempfile::TempDir,
}

/// The extension served by a mock host, with the fake models, `install` and
/// `remote`.
fn start(install: Install, remote: Remote) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let (data, local) = (dir.path().join("data"), dir.path().join("local"));
    let fake = Fake::default();
    let options = Options {
        engine: Box::new(fake.clone()),
        install,
        local_data: local.clone(),
        remote,
    };
    let host = MockHost::start_with_options(extension(options), ServeOptions { api: 2 }, |_, _| {
        Ok(serde_json::Value::Null)
    });
    host.hello_with(Hello {
        protocol: PROTOCOL,
        app_version: "0.4.12".into(),
        extension: ID.into(),
        settings: Default::default(),
        contributions: Vec::new(),
        data_dir: data.clone(),
        cache_dir: dir.path().join("cache"),
        session: 1,
    })
    .unwrap();
    Running {
        host,
        fake,
        data,
        local,
        _dir: dir,
    }
}

/// An install that must not run.
fn no_download() -> Install {
    Box::new(|_, _, _| panic!("nothing is downloaded"))
}

/// `pack`'s files at their sizes in `dir`, with its receipt when `receipt`.
fn fake_install(dir: &Path, pack: Pack, with_receipt: bool) {
    std::fs::create_dir_all(dir).unwrap();
    for file in pack.files() {
        std::fs::File::create(dir.join(file.name))
            .unwrap()
            .set_len(file.size_bytes)
            .unwrap();
    }
    if with_receipt {
        std::fs::write(dir.join(RECEIPT), receipt(&pack.files())).unwrap();
    }
}

impl Running {
    /// Installs `pack` where it belongs, in the local data folder.
    fn installed(&self, pack: Pack) -> PathBuf {
        let dir = self.local.join("models").join(pack.folder());
        fake_install(&dir, pack, true);
        dir
    }
    /// Asks `generator` with `input` about a photo of grey `level`, named
    /// `key`.
    fn ask(
        &self,
        generator: &str,
        input: Option<ViewerInput>,
        level: u8,
        key: &str,
    ) -> Result<(MaskGenerated, Vec<u8>), RemoteError> {
        let image = vec![level; (W * H * 3) as usize];
        self.host
            .generate_mask(request(generator, input, key), &image)
    }
}

fn request(generator: &str, input: Option<ViewerInput>, key: &str) -> MaskGenerate {
    let region = Region {
        space: Space::ImageFrame,
        rect: [0., 0., 1., 1.],
        scale: 1.,
        overlap: 0,
    };
    let image = Buffer {
        handle: 0,
        bytes: u64::from(W * H * 3),
        width: W,
        height: H,
        stride: W * 3,
        channels: 3,
        sample: Sample::U8,
        meaning: MaskGenerate::IMAGE,
        region,
        access: Access::Read,
    };
    let coverage = Buffer {
        handle: 1,
        bytes: u64::from(W * H),
        stride: W,
        channels: 1,
        meaning: Meaning::Coverage,
        access: Access::Write,
        ..image
    };
    MaskGenerate {
        generator: generator.into(),
        input,
        image,
        coverage,
        image_key: key.into(),
    }
}

fn prompt(clicks: &[(f32, f32, bool)], bounds: Option<[f32; 4]>) -> Option<ViewerInput> {
    Some(ViewerInput::Prompt {
        clicks: clicks
            .iter()
            .map(|&(x, y, positive)| Click { x, y, positive })
            .collect(),
        bounds: bounds.map(|[x0, y0, x1, y1]| Bounds { x0, y0, x1, y1 }),
    })
}

/// The selected pixels of a coverage, as rows of `#` and `.`.
fn picture(coverage: &[u8]) -> Vec<String> {
    coverage
        .chunks(W as usize)
        .map(|row| row.iter().map(|v| if *v > 0 { '#' } else { '.' }).collect())
        .collect()
}

fn setup_of(error: &RemoteError) -> NeedsSetup {
    assert_eq!(error.code, ErrorCode::NeedsSetup, "{error:?}");
    NeedsSetup::read(&error.message).unwrap()
}

/// Without the models, each generator says what to download: the selection
/// models for Subject and Background (and aiming), the sky model for Sky,
/// with their sizes, as the drawer showed them.
#[test]
fn each_generator_asks_for_its_own_models() {
    let r = start(no_download(), Remote::Fixed(None));
    let subject = setup_of(&r.ask(SUBJECT, None, 100, "a").unwrap_err());
    assert_eq!(subject, Pack::Subject.needs_setup());
    assert_eq!(subject.what, "the selection models");
    let aimed = setup_of(
        &r.ask(SUBJECT, prompt(&[(0.5, 0.5, true)], None), 100, "a")
            .unwrap_err(),
    );
    assert_eq!(aimed, subject);
    let sky = setup_of(&r.ask(SKY, None, 100, "a").unwrap_err());
    assert_eq!(sky.what, "the sky model");
    assert!(sky.bytes > 200_000_000 && subject.bytes > 250_000_000);
    // Having one doesn't install the other.
    r.installed(Pack::Subject);
    assert!(r.ask(SUBJECT, None, 100, "a").is_ok());
    setup_of(&r.ask(SKY, None, 100, "a").unwrap_err());
    assert_eq!(r.fake.calls().skies, 0);
}

/// Automatic Subject and Sky, and a prompt: the coverage the models made, at
/// the photo's size, named by the model that made it. The models load from
/// the extension's folder.
#[test]
fn the_generators_answer_with_the_models_coverage_and_identity() {
    let r = start(no_download(), Remote::Fixed(None));
    let subject_dir = r.installed(Pack::Subject);
    r.installed(Pack::Sky);
    let (answer, coverage) = r.ask(SUBJECT, None, 100, "a").unwrap();
    assert_eq!(answer.empty, None);
    assert!(
        answer.model.starts_with("sam2.1-hiera-small@"),
        "{}",
        answer.model
    );
    assert!(answer.model_key.starts_with("sam2.1-hiera-small|"));
    assert_eq!(picture(&coverage), vec!["####...."; H as usize]);
    assert!(coverage.iter().all(|v| [0, 100].contains(v)));
    let (sky, coverage) = r.ask(SKY, None, 100, "a").unwrap();
    assert!(sky.model.starts_with("upernet-convnext-base@"));
    assert_ne!(sky.model_key, answer.model_key);
    assert_eq!(
        picture(&coverage),
        [
            "########", "########", "########", "........", "........", "........"
        ]
    );
    // A box, and a click taking away a disc in the middle of it.
    let (aimed, coverage) = r
        .ask(
            SUBJECT,
            prompt(&[(0.75, 0.75, false)], Some([0.5, 0.5, 1., 1.])),
            100,
            "a",
        )
        .unwrap();
    assert_eq!(aimed.model, answer.model);
    assert_eq!(
        picture(&coverage),
        [
            "........", "........", "........", "....#..#", "........", "....#..#"
        ]
    );
    assert_eq!(
        r.fake.calls().folders[0],
        r.local.join("models").join(Pack::Subject.folder())
    );
    assert!(r.fake.calls().folders.contains(&subject_dir));
}

/// What nothing found means, for each: no subject, no sky, nothing there.
#[test]
fn nothing_found_says_what_wasnt() {
    let r = start(no_download(), Remote::Fixed(None));
    r.installed(Pack::Subject);
    r.installed(Pack::Sky);
    let empty = |generator, input| r.ask(generator, input, 0, "black").unwrap().0.empty;
    assert_eq!(empty(SUBJECT, None), Some(EmptyMask::NoSubject));
    assert_eq!(empty(SKY, None), Some(EmptyMask::NoSky));
    assert_eq!(
        empty(SUBJECT, prompt(&[(0.5, 0.5, false)], None)),
        Some(EmptyMask::NothingThere)
    );
}

/// The photo is analysed once: the clicks after the first are answered from
/// what was kept of it, and another photo is analysed anew.
#[test]
fn a_photo_is_analysed_once_for_its_clicks() {
    let r = start(no_download(), Remote::Fixed(None));
    r.installed(Pack::Subject);
    r.ask(SUBJECT, None, 100, "a").unwrap();
    r.ask(SUBJECT, prompt(&[(0.2, 0.2, true)], None), 100, "a")
        .unwrap();
    r.ask(
        SUBJECT,
        prompt(&[(0.2, 0.2, true), (0.8, 0.8, true)], None),
        100,
        "a",
    )
    .unwrap();
    assert_eq!(r.fake.calls().analyses, 1);
    r.ask(SUBJECT, None, 90, "b").unwrap();
    assert_eq!(r.fake.calls().analyses, 2);
}

/// A model failure is the answer: no runtime is `unavailable`, anything else
/// a failure with its reason; Sky takes no clicks.
#[test]
fn failures_reach_the_app_as_their_codes() {
    let r = start(no_download(), Remote::Fixed(None));
    r.installed(Pack::Subject);
    *r.fake.fail.lock().unwrap() = Some(InferenceError::RuntimeUnavailable("no runtime".into()));
    let e = r.ask(SUBJECT, None, 100, "a").unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (ErrorCode::Unavailable, "no runtime")
    );
    *r.fake.fail.lock().unwrap() = Some(InferenceError::ModelInvalid("truncated".into()));
    let e = r.ask(SUBJECT, None, 100, "a").unwrap_err();
    assert_eq!(e.code, ErrorCode::Other("failed".into()));
    assert!(e.message.contains("not usable: truncated"), "{}", e.message);
    *r.fake.fail.lock().unwrap() = None;
    let e = r
        .ask(SKY, prompt(&[(0.5, 0.5, true)], None), 100, "a")
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidRequest);
}

/// A pack in the local data folder's `models` with its receipt (as the app
/// installed it before this extension) is used where it is, with nothing
/// downloaded or moved, and nothing written to the extension's own data
/// folder. One with a bad receipt isn't, and the models are asked for.
#[test]
fn the_apps_models_are_used_where_they_are() {
    let r = start(no_download(), Remote::Fixed(None));
    let subject = r.local.join("models").join(Pack::Subject.folder());
    fake_install(&subject, Pack::Subject, true);
    let sky = r.local.join("models").join(Pack::Sky.folder());
    fake_install(&sky, Pack::Sky, false);
    std::fs::write(sky.join(RECEIPT), "not this release's files\n").unwrap();
    r.ask(SUBJECT, None, 100, "a").unwrap();
    assert_eq!(r.fake.calls().folders[0], subject);
    assert!(subject.join(RECEIPT).is_file(), "left where it is");
    assert!(!r.data.join("models").exists());
    setup_of(&r.ask(SKY, None, 100, "a").unwrap_err());
    assert!(sky.exists(), "a folder that isn't an install is left alone");
}

/// The program's models are the app's: `Options::default` looks in the
/// folder the installer has always used.
#[test]
fn the_default_models_folder_is_the_apps() {
    assert_eq!(
        Options::default().local_data.join("models"),
        rawmakase_model_install::models_dir()
    );
}

/// Setting up downloads the pack into the local data folder, saying how far
/// it is as the drawer did; the generator then answers. A cancelled download
/// ends cancelled, and a failed one says to download again.
#[test]
fn setup_downloads_the_models_with_progress() {
    let (gate_tx, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let install: Install = Box::new(move |dir, files, progress| {
        let pack = if files.len() == Pack::Sky.files().len() {
            Pack::Sky
        } else {
            Pack::Subject
        };
        // Half way, then wait for the test.
        let half = files.iter().map(|f| f.size_bytes).sum::<u64>() / 2;
        progress.report(half);
        let gate = gate.lock().unwrap();
        while gate.recv_timeout(Duration::from_millis(5)).is_err() {
            if progress.is_cancelled() {
                return Err(InstallError::Cancelled);
            }
        }
        fake_install(dir, pack, true);
        Ok(())
    });
    let r = start(install, Remote::Fixed(None));
    let id = r.host.send_request(
        rawmakase_extension::protocol::method::AppRequest::MaskSetup(
            rawmakase_extension::protocol::method::MaskSetup {
                generator: SUBJECT.into(),
            },
        ),
    );
    let recorded = r.host.wait_for(|recorded| {
        recorded.iter().any(
            |r| matches!(r, Recorded::Progress { done: Some(d), .. } if (*d - 0.5).abs() < 0.01),
        )
    });
    let text = recorded.iter().rev().find_map(|r| match r {
        Recorded::Progress { text, .. } => text.clone(),
        _ => None,
    });
    let mb = Pack::Subject.needs_setup().bytes;
    assert_eq!(
        text.as_deref(),
        Some(format!("{} of {} MB", mb / 2 / 1_000_000, mb / 1_000_000).as_str())
    );
    gate_tx.send(()).unwrap();
    r.host.wait(id).unwrap();
    assert!(r.ask(SUBJECT, None, 100, "a").is_ok());
    // Cancelled while it waits.
    let id = r.host.send_request(
        rawmakase_extension::protocol::method::AppRequest::MaskSetup(
            rawmakase_extension::protocol::method::MaskSetup {
                generator: SKY.into(),
            },
        ),
    );
    r.host.wait_for(|recorded| {
        recorded
            .iter()
            .filter(|r| matches!(r, Recorded::Progress { .. }))
            .count()
            > 2
    });
    r.host.cancel(id);
    assert_eq!(r.host.wait(id).unwrap_err().code, ErrorCode::Cancelled);
    setup_of(&r.ask(SKY, None, 100, "a").unwrap_err());
    // Set up already, it downloads nothing.
    drop(gate_tx);
    r.host.setup_mask(SUBJECT).unwrap();
}

/// A failed download says why, and that downloading again carries on.
#[test]
fn a_failed_download_says_to_download_again() {
    let install: Install =
        Box::new(|_, _, _| Err(InstallError::Failed("the host answered 503".into())));
    let r = start(install, Remote::Fixed(None));
    let e = r.host.setup_mask(SKY).unwrap_err();
    assert_eq!(
        e.message,
        "the host answered 503. Download again to carry on from where it stopped."
    );
}

/// With a remote worker, nothing has to be installed here: Subject, a
/// prompt and Sky are the worker's, through its protocol, and the request
/// says where it runs.
#[test]
fn a_remote_worker_selects_without_the_models_here() {
    let (endpoint, calls) = crate::fake::remote_worker();
    let r = start(no_download(), Remote::Fixed(Some(endpoint.clone())));
    let (answer, coverage) = r.ask(SUBJECT, None, 100, "a").unwrap();
    assert!(answer.model.starts_with("sam2.1-hiera-small@"));
    assert_eq!(picture(&coverage), vec!["####...."; H as usize]);
    let (_, coverage) = r
        .ask(SUBJECT, prompt(&[(0.75, 0.75, true)], None), 100, "a")
        .unwrap();
    assert_eq!(picture(&coverage)[4], "....####");
    let (_, coverage) = r.ask(SKY, None, 100, "a").unwrap();
    assert_eq!(picture(&coverage)[0], "########");
    let calls = calls.lock().unwrap().clone();
    assert_eq!((calls.analyses, calls.segments, calls.skies), (1, 1, 1));
    assert_eq!(r.fake.calls(), Default::default(), "nothing ran here");
    assert!(r.host.recorded().iter().any(|r| matches!(
        r,
        Recorded::Progress { text: Some(t), .. } if *t == format!("on {endpoint}")
    )));
    // A worker that is gone is unavailable, as a missing runtime is.
    let gone = start(no_download(), Remote::Fixed(Some("127.0.0.1:9".into())));
    assert_eq!(
        gone.ask(SUBJECT, None, 100, "a").unwrap_err().code,
        ErrorCode::Unavailable
    );
}

/// Removing the models lets go of them and deletes Subject's and Sky's
/// packs, and nothing else of the folder; the generators then ask for them
/// again.
#[test]
fn removing_the_models_asks_for_them_again() {
    let r = start(no_download(), Remote::Fixed(None));
    r.installed(Pack::Subject);
    r.installed(Pack::Sky);
    let denoise = r.local.join("models").join("rawnind-v1");
    std::fs::create_dir_all(&denoise).unwrap();
    r.ask(SKY, None, 100, "a").unwrap();
    let removed = r
        .host
        .run_command(REMOVE_MODELS, None, serde_json::Value::Null)
        .unwrap();
    assert_eq!(removed, serde_json::json!({ "removed": 2 }));
    assert_eq!(r.fake.calls().unloads, 1);
    assert!(denoise.is_dir(), "AI Denoise's pack is the app's");
    setup_of(&r.ask(SUBJECT, None, 100, "a").unwrap_err());
    setup_of(&r.ask(SKY, None, 100, "a").unwrap_err());
}

/// The real models through the extension: a click on a bright square on a
/// dark photo selects the square. The models are found through a link in
/// the local data folder's `models`, so their own folder is left as it is.
#[test]
#[cfg(unix)]
#[ignore = "needs RAWMAKASE_TEST_MODEL (the selection models' folder) and RAWMAKASE_ORT_LIB"]
fn the_real_models_select_what_was_clicked() {
    let models = PathBuf::from(std::env::var("RAWMAKASE_TEST_MODEL").unwrap());
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("local");
    std::fs::create_dir_all(local.join("models")).unwrap();
    std::os::unix::fs::symlink(&models, local.join("models").join(Pack::Subject.folder())).unwrap();
    let host = MockHost::start_with_options(
        extension(Options {
            engine: Box::new(Onnx::default()),
            install: no_download(),
            local_data: local,
            remote: Remote::Fixed(None),
        }),
        ServeOptions { api: 2 },
        |_, _| Ok(serde_json::Value::Null),
    );
    host.hello_with(Hello {
        protocol: PROTOCOL,
        app_version: "0.4.12".into(),
        extension: ID.into(),
        settings: Default::default(),
        contributions: Vec::new(),
        data_dir: dir.path().join("data"),
        cache_dir: dir.path().join("cache"),
        session: 1,
    })
    .unwrap();
    let (w, h) = (640u32, 426u32);
    let mut image = vec![40u8; (w * h * 3) as usize];
    for y in 150..280 {
        for x in 250..400 {
            image[((y * w + x) * 3) as usize..][..3].copy_from_slice(&[240, 230, 220]);
        }
    }
    let mut ask = request(
        SUBJECT,
        prompt(&[(325. / 640., 215. / 426., true)], None),
        "real",
    );
    for b in [&mut ask.image, &mut ask.coverage] {
        b.width = w;
        b.height = h;
    }
    ask.image.stride = w * 3;
    ask.image.bytes = u64::from(w * h * 3);
    ask.coverage.stride = w;
    ask.coverage.bytes = u64::from(w * h);
    let (answer, coverage) = host.generate_mask(ask, &image).unwrap();
    assert_eq!(answer.empty, None);
    let at = |x: u32, y: u32| coverage[(y * w + x) as usize];
    assert!(at(325, 215) >= 128, "the square is selected");
    assert!(
        at(50, 50) < 128 && at(600, 400) < 128,
        "the background isn't"
    );
}
