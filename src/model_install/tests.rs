use super::*;

const FILE: ModelFile = ModelFile {
    name: "x.onnx",
    size_bytes: 3,
    sha256: "",
    url: "",
    fallback: "",
};

#[test]
fn copying_checks_size_and_digest_and_stops_when_asked() {
    let progress = Progress::default();
    // Too short, then too long.
    assert!(copy_checked(&b"ab"[..], &mut Vec::new(), &progress, &FILE, 0).is_err());
    assert!(copy_checked(&b"abcd"[..], &mut Vec::new(), &progress, &FILE, 0).is_err());
    // The right bytes give their SHA-256, and progress counts earlier files too.
    let digest = copy_checked(&b"abc"[..], &mut Vec::new(), &progress, &FILE, 10).unwrap();
    assert_eq!(
        digest,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(progress.done.load(Ordering::Relaxed), 13);
    // Cancelled before the first chunk.
    progress.cancel.store(true, Ordering::Relaxed);
    let error = copy_checked(&b"abc"[..], &mut Vec::new(), &progress, &FILE, 0).unwrap_err();
    assert_eq!(error, InstallError::Cancelled);
}

/// A full disk stops the copy with its own error, shown as before; a cancel and a
/// full disk are not worth another request, any other failure is.
#[test]
fn a_full_disk_and_a_cancel_are_final_and_other_failures_retry() {
    struct Full;
    impl Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::StorageFull.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let error = copy_checked(&b"abc"[..], &mut Full, &Progress::default(), &FILE, 0).unwrap_err();
    assert_eq!(error, InstallError::DiskFull);
    assert_eq!(error.to_string(), "the disk is full");
    assert_eq!(InstallError::Cancelled.to_string(), "Cancelled");
    for stop in [InstallError::Cancelled, InstallError::DiskFull] {
        assert!(matches!(Failed::from(stop.clone()), Failed::Final(e) if e == stop));
    }
    let other = InstallError::Failed("timed out".into());
    assert!(matches!(Failed::from(other), Failed::Retry(why) if why == "timed out"));
}

/// How the test host answers one request.
enum Answer {
    /// The whole file, closing after `send` bytes.
    Whole { send: usize },
    /// The whole file, sending `send` bytes and then nothing until the client
    /// hangs up.
    Stall { send: usize },
    /// The rest of the file from where the request's range starts.
    Rest,
}

const LEN: usize = 100_000;

/// The bytes the test host serves.
fn body() -> Vec<u8> {
    (0..LEN).map(|i| (i * 7 % 251) as u8).collect()
}

/// Serves `body()` on a local port, one connection per request, answering each
/// as told. Gives the file as the manifest describes it, both sources pointing at
/// the host, and where each request asked to start, sent before it is answered so
/// it has arrived by the time the client has its answer.
///
/// The host is never joined: if the client stops early, its test fails on what
/// the host saw instead of waiting in `accept`. Reads time out for the same
/// reason, and the thread ends with the test process.
fn host(answers: Vec<Answer>) -> (ModelFile, std::sync::mpsc::Receiver<Option<u64>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/x.onnx", listener.local_addr().unwrap());
    let (asked, requests) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let body = body();
        for answer in answers {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            // A client that hung up mid-answer still gets the next one.
            let _ = answer_one(stream, &body, answer, &asked);
        }
    });
    let file = ModelFile {
        name: "x.onnx",
        size_bytes: LEN as u64,
        sha256: Box::leak(hex(Sha256::new_with_prefix(body())).into_boxed_str()),
        url: Box::leak(url.into_boxed_str()),
        fallback: "",
    };
    (
        ModelFile {
            fallback: file.url,
            ..file
        },
        requests,
    )
}

fn answer_one(
    mut stream: std::net::TcpStream,
    body: &[u8],
    answer: Answer,
    asked: &std::sync::mpsc::Sender<Option<u64>>,
) -> std::io::Result<()> {
    use std::io::BufRead;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut from = None;
    let mut reader = std::io::BufReader::new(stream.try_clone()?);
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            break;
        }
        if let Some(range) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
            from = range.trim().trim_end_matches('-').parse::<usize>().ok();
        }
    }
    let _ = asked.send(from.map(|f| f as u64));
    let len = body.len();
    match (answer, from) {
        (Answer::Whole { send }, _) => {
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {len}\r\n\r\n")?;
            stream.write_all(&body[..send])
        }
        (Answer::Stall { send }, _) => {
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {len}\r\n\r\n")?;
            stream.write_all(&body[..send])?;
            // Until the client gives up on the body and closes.
            std::io::copy(&mut reader, &mut std::io::sink()).map(|_| ())
        }
        (Answer::Rest, Some(from)) => {
            write!(
                stream,
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {from}-{}/{len}\r\n\
                 Content-Length: {}\r\n\r\n",
                len - 1,
                len - from
            )?;
            stream.write_all(&body[from..])
        }
        (Answer::Rest, None) => {
            stream.write_all(b"HTTP/1.1 500 No range\r\nContent-Length: 0\r\n\r\n")
        }
    }
}

/// The partial file's place, holding `held` from an earlier attempt.
fn partial(dir: &tempfile::TempDir, held: &[u8]) -> PathBuf {
    let part = dir.path().join(".x.onnx.part");
    std::fs::write(&part, held).unwrap();
    part
}

#[test]
fn a_dropped_download_resumes_where_it_stopped() {
    let (file, requests) = host(vec![Answer::Whole { send: 40_000 }, Answer::Rest]);
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &[]);
    let progress = Progress::default();
    assert_eq!(download(&part, &progress, &file, 0, BODY_BUDGET), Ok(()));
    assert_eq!(std::fs::read(&part).unwrap(), body());
    assert_eq!(progress.done.load(Ordering::Relaxed), LEN as u64);
    // The second request asked for the rest only.
    assert_eq!(
        requests.try_iter().collect::<Vec<_>>(),
        [None, Some(40_000)]
    );
}

/// The 0.2.2 bug: a body longer in coming than its budget started over every
/// time, so on a slow connection a large file never arrived.
#[test]
fn a_download_slower_than_its_budget_keeps_what_arrived() {
    let (file, requests) = host(vec![Answer::Stall { send: 30_000 }, Answer::Rest]);
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &[]);
    let budget = Duration::from_millis(200);
    assert_eq!(
        download(&part, &Progress::default(), &file, 0, budget),
        Ok(())
    );
    assert_eq!(std::fs::read(&part).unwrap(), body());
    assert_eq!(
        requests.try_iter().collect::<Vec<_>>(),
        [None, Some(30_000)]
    );
}

/// A client giving up before it has read what was sent resets the connection;
/// on a slow machine `download` does that at its budget. The host must still
/// answer the next request rather than refuse it.
#[test]
fn the_host_answers_after_a_client_resets_the_connection() {
    let (file, requests) = host(vec![Answer::Stall { send: 30_000 }, Answer::Rest]);
    let address = file
        .url
        .trim_start_matches("http://")
        .trim_end_matches("/x.onnx");
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream.write_all(b"GET /x.onnx HTTP/1.1\r\n\r\n").unwrap();
    stream.read_exact(&mut [0u8; 4]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    // Unread bytes left behind: closing resets the connection.
    drop(stream);
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &body()[..60_000]);
    assert_eq!(
        download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
        Ok(())
    );
    assert_eq!(
        requests.try_iter().collect::<Vec<_>>(),
        [None, Some(60_000)]
    );
}

#[test]
fn a_download_left_by_an_earlier_session_is_resumed() {
    let (file, requests) = host(vec![Answer::Rest]);
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &body()[..60_000]);
    assert_eq!(
        download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
        Ok(())
    );
    assert_eq!(std::fs::read(&part).unwrap(), body());
    assert_eq!(requests.try_iter().collect::<Vec<_>>(), [Some(60_000)]);
}

#[test]
fn a_host_ignoring_the_range_and_dropping_gets_no_further_and_is_left() {
    // Each answer starts over, ending at 40 or 60 KB: never past 60 KB, so three
    // requests after reaching it the source is given up, and so is the fallback.
    let answers = (0..10).map(|i| Answer::Whole {
        send: if i % 2 == 0 { 40_000 } else { 60_000 },
    });
    let (mut file, requests) = host(answers.collect());
    file.fallback = host(vec![]).0.url;
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &[]);
    assert!(download(&part, &Progress::default(), &file, 0, BODY_BUDGET).is_err());
    assert_eq!(requests.try_iter().count(), 5);
    // What it got is kept for the next try.
    assert_eq!(std::fs::read(&part).unwrap(), &body()[..40_000]);
}

#[test]
fn a_host_ignoring_the_range_is_read_from_the_start() {
    let (file, requests) = host(vec![Answer::Whole { send: LEN }]);
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &body()[..60_000]);
    assert_eq!(
        download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
        Ok(())
    );
    assert_eq!(std::fs::read(&part).unwrap(), body());
    assert_eq!(requests.try_iter().collect::<Vec<_>>(), [Some(60_000)]);
}

#[test]
fn wrong_bytes_kept_from_before_are_fetched_again() {
    // The resumed file fails its checksum, and the other source starts over.
    let (file, requests) = host(vec![Answer::Rest, Answer::Whole { send: LEN }]);
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &[0; 60_000]);
    assert_eq!(
        download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
        Ok(())
    );
    assert_eq!(std::fs::read(&part).unwrap(), body());
    assert_eq!(
        requests.try_iter().collect::<Vec<_>>(),
        [Some(60_000), None]
    );
}

#[test]
fn a_partial_file_longer_than_the_file_is_not_resumed() {
    let (file, requests) = host(vec![Answer::Whole { send: LEN }]);
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &[0; LEN + 1]);
    assert_eq!(
        download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
        Ok(())
    );
    assert_eq!(std::fs::read(&part).unwrap(), body());
    assert_eq!(requests.try_iter().collect::<Vec<_>>(), [None]);
}

#[test]
fn a_cancelled_download_keeps_what_arrived() {
    let (file, requests) = host(vec![]);
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &body()[..60_000]);
    let progress = Progress::default();
    progress.cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        download(&part, &progress, &file, 0, BODY_BUDGET),
        Err(InstallError::Cancelled)
    );
    assert_eq!(std::fs::read(&part).unwrap(), &body()[..60_000]);
    // Nothing is asked of the host once cancelled.
    assert_eq!(requests.try_iter().count(), 0);
}

#[test]
fn reading_kept_bytes_back_stops_at_a_cancel_and_keeps_them() {
    let (file, _) = host(vec![]);
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &body()[..60_000]);
    let progress = Progress::default();
    progress.cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        Partial::open(&part, &file, &progress, 0).err(),
        Some(InstallError::Cancelled)
    );
    assert_eq!(std::fs::read(&part).unwrap(), &body()[..60_000]);
}

#[test]
fn bytes_left_by_a_failed_write_are_cut_off_before_the_next_request() {
    let (file, requests) = host(vec![Answer::Rest]);
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &body()[..60_000]);
    let progress = Progress::default();
    let mut partial = Partial::open(&part, &file, &progress, 0).unwrap();
    // Written but never counted, as by a write that failed part way.
    partial.out.write_all(b"stray").unwrap();
    let agent = agent(BODY_BUDGET);
    assert_eq!(
        fetch(&agent, file.url, &mut partial, &progress, &file, 0),
        Ok(())
    );
    assert_eq!(std::fs::read(&part).unwrap(), body());
    assert_eq!(requests.try_iter().collect::<Vec<_>>(), [Some(60_000)]);
}

#[test]
fn wrong_bytes_kept_from_before_are_fetched_again_from_the_same_source() {
    // The whole file is there but wrong: no request can show it, so the checksum
    // does, and the mirror is asked again rather than the fallback, which is down.
    let (mut file, requests) = host(vec![Answer::Whole { send: LEN }]);
    file.fallback = host(vec![]).0.url;
    let dir = tempfile::tempdir().unwrap();
    let part = partial(&dir, &[0; LEN]);
    assert_eq!(
        download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
        Ok(())
    );
    assert_eq!(std::fs::read(&part).unwrap(), body());
    assert_eq!(requests.try_iter().collect::<Vec<_>>(), [None]);
}

#[test]
fn an_install_waits_for_another_windows_install() {
    let dir = tempfile::tempdir().unwrap();
    let other = lock(dir.path(), &Progress::default()).unwrap();
    // Waiting, not failing: only a cancel ends the wait.
    let cancelled = Progress::default();
    cancelled.cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        lock(dir.path(), &cancelled).err(),
        Some(InstallError::Cancelled)
    );
    // And once the other install ends, this one goes on.
    let path = dir.path().to_owned();
    let waiting = std::thread::spawn(move || lock(&path, &Progress::default()).is_ok());
    drop(other);
    assert!(waiting.join().unwrap());
}

#[test]
fn each_pack_has_a_folder_named_for_its_model_and_version() {
    assert!(Pack::Subject.dir().ends_with(format!(
        "models/{}-v{}",
        rawmakase_inference::SUBJECT.id,
        rawmakase_inference::SUBJECT.version
    )));
    assert!(Pack::Sky.dir().ends_with(format!(
        "models/{}-v{}",
        rawmakase_inference::SKY.id,
        rawmakase_inference::SKY.version
    )));
    assert!(Pack::Subject.megabytes() > 250 && Pack::Sky.megabytes() > 200);
}

/// A folder holding `pack`'s files at their sizes, with its receipt.
fn fake_install(pack: Pack) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for file in pack.files() {
        let f = std::fs::File::create(dir.path().join(file.name)).unwrap();
        f.set_len(file.size_bytes).unwrap();
    }
    std::fs::write(dir.path().join(RECEIPT), receipt(pack)).unwrap();
    dir
}

/// What an install from before Sky had its own model holds: it must keep working after
/// the update, without fetching anything.
#[test]
fn subjects_models_stay_installed_without_the_sky_model() {
    let dir = fake_install(Pack::Subject);
    assert!(is_installed(dir.path(), Pack::Subject));
    assert!(!is_installed(dir.path(), Pack::Sky));
    let receipt_of_the_old_install = [
        "aacf1f7137bb6fffcf6bf166abcfabe28f57a76059254f3fb611c4a64a208119 467440 vision_encoder.onnx",
        "260fd1f0a34e72a3dc79a739e563b4facc0ba75504818b433a1f808e66637456 162476288 vision_encoder.onnx_data",
        "079c59b261f723ff5c6a125e69b0170a957b21c58738c28d2b0394ecd0587d7f 213114 prompt_encoder_mask_decoder.onnx",
        "f9e59a584ab8ced21fa812c211bc01084204db1c9e92a5ef4fb3a49972b4e864 20958208 prompt_encoder_mask_decoder.onnx_data",
        "afd9f02d864302d690356fd4bfcb2feed2397a1190bf46a7306cb430464d734a 86559030 detr-panoptic-fp16.onnx",
        "",
    ]
    .join("\n");
    assert_eq!(receipt(Pack::Subject), receipt_of_the_old_install);
    let sky = fake_install(Pack::Sky);
    assert!(is_installed(sky.path(), Pack::Sky));
    assert!(!is_installed(sky.path(), Pack::Subject));
}

#[test]
fn a_model_is_installed_only_when_every_file_is_there_with_its_size_and_checked() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!is_installed(dir.path(), Pack::Subject));
    for file in all_files() {
        let f = std::fs::File::create(dir.path().join(file.name)).unwrap();
        f.set_len(file.size_bytes).unwrap();
    }
    // The right sizes are not enough without the installer's receipt.
    assert!(!is_installed(dir.path(), Pack::Subject));
    std::fs::write(dir.path().join(RECEIPT), receipt(Pack::Subject)).unwrap();
    assert!(is_installed(dir.path(), Pack::Subject));
    assert_eq!(
        digest_of(&dir.path().join(all_files()[0].name)).map(|d| d.len()),
        Some(64)
    );
    std::fs::File::create(dir.path().join(all_files()[1].name))
        .unwrap()
        .set_len(5)
        .unwrap();
    assert!(!is_installed(dir.path(), Pack::Subject));
}

/// AI Denoise's model is installed only with its licence and credit beside it: a
/// folder without `LICENSE` or `NOTICE`, receipt or not, is not an install.
#[test]
fn the_denoiser_needs_its_licence_and_notice() {
    let dir = tempfile::tempdir().unwrap();
    for file in Pack::Denoise.files() {
        std::fs::File::create(dir.path().join(file.name))
            .unwrap()
            .set_len(file.size_bytes)
            .unwrap();
    }
    std::fs::write(dir.path().join(RECEIPT), receipt(Pack::Denoise)).unwrap();
    assert!(is_installed(dir.path(), Pack::Denoise));
    for name in ["LICENSE", "NOTICE"] {
        let path = dir.path().join(name);
        let kept = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(!is_installed(dir.path(), Pack::Denoise), "{name}");
        std::fs::write(&path, kept).unwrap();
    }
    assert!(receipt(Pack::Denoise).contains(" LICENSE\n"));
    assert!(receipt(Pack::Denoise).contains(" NOTICE\n"));
    // Its own folder: none of the selection models' files.
    assert_ne!(Pack::Denoise.dir(), Pack::Subject.dir());
}
