use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A request the test server received: its first line and headers, lowercased
/// names.
#[derive(Clone, Debug)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
}
impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}
/// The status, headers and body the server answers.
type Answer = (u16, Vec<(&'static str, String)>, Vec<u8>);
type Reply = Arc<Mutex<Box<dyn Fn(&Seen) -> Answer + Send>>>;

/// A plain HTTP server on localhost answering each request with `answer`,
/// after `delay`; every request is kept.
struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    answer: Reply,
    count: Arc<AtomicUsize>,
}
impl Server {
    fn start(delay: Duration, answer: impl Fn(&Seen) -> Answer + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Seen>>> = Default::default();
        let count: Arc<AtomicUsize> = Default::default();
        let answer: Reply = Arc::new(Mutex::new(Box::new(answer)));
        let (s, c, a) = (seen.clone(), count.clone(), answer.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (s, c, a) = (s.clone(), c.clone(), a.clone());
                std::thread::spawn(move || {
                    let mut out = stream.try_clone().unwrap();
                    let mut reader = BufReader::new(stream);
                    loop {
                        let mut first = String::new();
                        if reader.read_line(&mut first).unwrap_or(0) == 0 {
                            return;
                        }
                        let mut headers = Vec::new();
                        loop {
                            let mut line = String::new();
                            reader.read_line(&mut line).unwrap();
                            let line = line.trim_end();
                            if line.is_empty() {
                                break;
                            }
                            let (n, v) = line.split_once(':').unwrap();
                            headers.push((n.trim().to_lowercase(), v.trim().to_owned()));
                        }
                        let path = first.split_whitespace().nth(1).unwrap_or("").to_owned();
                        let request = Seen { path, headers };
                        c.fetch_add(1, Ordering::SeqCst);
                        s.lock().unwrap().push(request.clone());
                        std::thread::sleep(delay);
                        let (status, headers, body) = (a.lock().unwrap())(&request);
                        let mut head =
                            format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
                        for (n, v) in headers {
                            head.push_str(&format!("{n}: {v}\r\n"));
                        }
                        head.push_str("\r\n");
                        if out.write_all(head.as_bytes()).is_err() || out.write_all(&body).is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        Self {
            port,
            seen,
            answer,
            count,
        }
    }
    fn template(&self) -> Template {
        Template::unchecked(&format!(
            "http://127.0.0.1:{}/{{z}}/{{x}}/{{y}}.png",
            self.port
        ))
    }
    fn answer(&self, answer: impl Fn(&Seen) -> Answer + Send + 'static) {
        *self.answer.lock().unwrap() = Box::new(answer);
    }
    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn png() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(256, 256, image::Rgba([10, 20, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent(user_agent())
        .build()
        .into()
}
const TILE: TileId = TileId { z: 3, x: 4, y: 5 };

#[test]
fn a_tile_is_kept_used_while_fresh_and_then_asked_for_only_if_it_changed() {
    let body = png();
    let server = Server::start(Duration::ZERO, move |_| {
        (
            200,
            vec![
                ("Cache-Control", "max-age=3600".into()),
                ("ETag", "\"v1\"".into()),
            ],
            body.clone(),
        )
    });
    let dir = tempfile::tempdir().unwrap();
    let (template, agent) = (server.template(), agent());
    let folder = template.folder(dir.path());
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let image = load(&agent, &template, &folder, TILE, now).unwrap();
    assert_eq!(image.size, [256, 256]);
    let first = &server.requests()[0];
    assert_eq!(first.path, "/3/4/5.png");
    assert!(
        first
            .header("user-agent")
            .unwrap()
            .starts_with("RAWmakase/")
    );
    assert_eq!(first.header("if-none-match"), None);

    // Within its hour the tile comes from disk.
    load(
        &agent,
        &template,
        &folder,
        TILE,
        now + Duration::from_secs(3000),
    )
    .unwrap();
    assert_eq!(server.requests().len(), 1);

    // Past it the server is asked whether it changed; unchanged, the kept tile
    // is used for another hour.
    server.answer(|_| {
        (
            304,
            vec![("Cache-Control", "max-age=3600".into())],
            Vec::new(),
        )
    });
    let later = now + Duration::from_secs(4000);
    assert_eq!(
        load(&agent, &template, &folder, TILE, later).unwrap().size,
        [256, 256]
    );
    let asked = &server.requests()[1];
    assert_eq!(asked.header("if-none-match"), Some("\"v1\""));
    let (_, meta_file) = tiles::paths(&folder, TILE);
    let meta: Meta = serde_json::from_slice(&std::fs::read(meta_file).unwrap()).unwrap();
    assert_eq!(meta.fresh_until, 1_004_000 + 3600);
    assert_eq!(
        meta.etag.as_deref(),
        Some("\"v1\""),
        "the validator is kept"
    );

    // A server that fails still leaves the kept tile shown.
    server.answer(|_| (503, Vec::new(), Vec::new()));
    let much_later = now + Duration::from_secs(100_000);
    assert!(load(&agent, &template, &folder, TILE, much_later).is_ok());
    // Without a kept tile, the failure is said.
    let other = TileId { z: 3, x: 0, y: 0 };
    let err = load(&agent, &template, &folder, other, much_later).unwrap_err();
    assert!(err.contains("503"), "{err}");
    // A body that isn't an image is refused and not kept.
    server.answer(|_| (200, Vec::new(), b"<html>".to_vec()));
    assert!(load(&agent, &template, &folder, other, much_later).is_err());
    assert!(!tiles::paths(&folder, other).0.exists());
}

fn wait_for_done(fetcher: &Fetcher, n: usize) -> Vec<Done> {
    let until = Instant::now() + Duration::from_secs(10);
    let mut done = Vec::new();
    while done.len() < n && Instant::now() < until {
        done.extend(fetcher.done());
        std::thread::sleep(Duration::from_millis(5));
    }
    done
}

#[test]
fn tiles_asked_for_arrive_and_ones_no_longer_asked_for_are_not_fetched() {
    let body = png();
    let server = Server::start(Duration::from_millis(150), move |_| {
        (200, Vec::new(), body.clone())
    });
    let dir = tempfile::tempdir().unwrap();
    let mut fetcher = Fetcher::new(
        dir.path().to_path_buf(),
        Net { https_only: false },
        egui::Context::default(),
    );
    let template = Arc::new(server.template());
    let many: Vec<TileId> = (0..40).map(|x| TileId { z: 6, x, y: 1 }).collect();
    fetcher.want(&template, many);
    let first = wait_for_done(&fetcher, 2);
    // The two workers take the first two asked for.
    let mut xs: Vec<u32> = first.iter().map(|d| d.tile.x).collect();
    xs.sort();
    assert_eq!(xs, [0, 1], "the first asked for first");
    assert!(
        first
            .iter()
            .all(|d| d.template == template && d.image.is_ok())
    );
    // Not asked for again, the rest are dropped once WANTED_FOR passes: two
    // workers at 150 ms a tile fetch far fewer than 40 in two seconds.
    std::thread::sleep(Duration::from_secs(2));
    let fetched = server.count.load(Ordering::SeqCst);
    assert!(fetched < 20, "{fetched} fetched");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        server.count.load(Ordering::SeqCst),
        fetched,
        "nothing more is fetched"
    );
}

#[test]
fn closing_ends_the_workers_and_nothing_is_fetched_after() {
    let body = png();
    let server = Server::start(Duration::ZERO, move |_| (200, Vec::new(), body.clone()));
    let dir = tempfile::tempdir().unwrap();
    let mut fetcher = Fetcher::new(
        dir.path().to_path_buf(),
        Net { https_only: false },
        egui::Context::default(),
    );
    let template = Arc::new(server.template());
    fetcher.want(&template, vec![TILE]);
    assert_eq!(wait_for_done(&fetcher, 1).len(), 1);
    let stopping = fetcher.close();
    assert_eq!(stopping.len(), WORKERS);
    let waited = crate::app::task::wait_for(stopping, Duration::from_secs(5));
    assert_eq!(waited.detached, 0);
    fetcher.want(&template, vec![TileId { z: 3, x: 1, y: 1 }]);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn the_tiles_fetched_longest_ago_go_first_when_the_cache_is_over_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("server");
    let mut kept = Vec::new();
    for i in 0..4u32 {
        let (file, meta) = tiles::paths(&folder, TileId { z: 1, x: i, y: 0 });
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, vec![0u8; 100]).unwrap();
        std::fs::write(&meta, b"{}").unwrap();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 + u64::from(i));
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(at)
            .unwrap();
        kept.push((file, meta));
    }
    trim(dir.path(), 1000);
    assert!(
        kept.iter().all(|(f, _)| f.exists()),
        "under the limit nothing goes"
    );
    // 400 bytes over a limit of 300: down to 225, so the two oldest go.
    trim(dir.path(), 300);
    let left: Vec<bool> = kept.iter().map(|(f, m)| f.exists() && m.exists()).collect();
    assert_eq!(left, [false, false, true, true]);
    assert!(!kept[0].1.exists());
}

#[test]
fn a_tile_that_isnt_a_square_of_256_or_512_pixels_is_refused() {
    let png = |w, h| {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(w, h)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    };
    assert!(decode(&png(256, 256)).is_ok());
    assert!(decode(&png(512, 512)).is_ok());
    assert!(decode(&png(4096, 4096)).unwrap_err().contains("4096×4096"));
    assert!(decode(&png(256, 128)).is_err());
}

#[test]
fn a_tile_asked_for_twice_is_fetched_once() {
    let body = png();
    let server = Server::start(Duration::from_millis(300), move |_| {
        (200, Vec::new(), body.clone())
    });
    let dir = tempfile::tempdir().unwrap();
    let mut fetcher = Fetcher::new(
        dir.path().to_path_buf(),
        Net { https_only: false },
        egui::Context::default(),
    );
    let template = Arc::new(server.template());
    fetcher.want(&template, vec![TILE, TILE, TILE]);
    assert_eq!(wait_for_done(&fetcher, 1).len(), 1);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(server.requests().len(), 1);
}
