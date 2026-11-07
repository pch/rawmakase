//! How fast editable previews are built, and with how much memory, as the app
//! builds them: for each RAW in the folder `RAWMAKASE_EDITABLE_PREVIEW_SAMPLES`,
//! opened, decoded in full ([`DecodePolicy::Export`], as the build does), built at
//! [`LONG_EDGE`] and written. `RAWMAKASE_EDITABLE_PREVIEW_PACE` is `<builds>x<threads>`:
//! that many photos built side by side, all on one pool of that many threads, as
//! the Library's full pace renders its edited previews (default `1x2`, the build's
//! pace before it had a full one). Run one pace per process, so the peak memory
//! is that pace's alone:
//!
//! ```text
//! RAWMAKASE_EDITABLE_PREVIEW_SAMPLES=… RAWMAKASE_EDITABLE_PREVIEW_PACE=2x3 \
//!   cargo test -p rawmakase-decode --release editable_preview::pace -- --ignored --nocapture
//! ```
//!
//! It prints each photo's steps and, at the end, the batch's wall time and the
//! process's peak resident memory (Linux only), with what it held before the
//! first photo. With one build at a time it also prints each photo's own peak.
//! The Camera Look fit is left out: only photos without colour data of their own
//! have one, and its result is cached.
use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// Resident memory now and at its peak since the last [`reset_peak`], in bytes.
#[cfg(target_os = "linux")]
fn memory() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| -> Option<u64> {
        let line = status.lines().find(|l| l.starts_with(name))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    };
    Some((field("VmRSS:")?, field("VmHWM:")?))
}

#[cfg(not(target_os = "linux"))]
fn memory() -> Option<(u64, u64)> {
    None
}

/// Returns freed memory to the system and starts the peak again from what is
/// resident now, so the next peak is the next photo's.
#[cfg(target_os = "linux")]
fn reset_peak() {
    // SAFETY: malloc_trim only releases free heap pages; it has no preconditions.
    #[cfg(target_env = "gnu")]
    unsafe {
        libc::malloc_trim(0);
    }
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

#[cfg(not(target_os = "linux"))]
fn reset_peak() {}

fn gb(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

/// One photo's steps, in seconds, and its size.
struct Timed {
    name: String,
    pixels: (u32, u32),
    open: f64,
    decode: f64,
    build: f64,
    write: f64,
}

fn one(path: &Path, out: &Path, cache: &crate::decode_cache::DecodeCache) -> Result<Timed> {
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let t = Instant::now();
    let identity = Identity::read(path)?;
    let raw = crate::photo::open(path)?;
    let exif = crate::photo::camera_exif(path);
    let open = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let full =
        crate::decode::FullSize::of_version(Some(&identity), Default::default(), cache.clone());
    let original = full
        .get(
            || Ok(raw),
            crate::decode::DecodePolicy::Export,
            Cancel::NEVER,
        )?
        .context("The photo wasn't decoded")?;
    let decode = t.elapsed().as_secs_f64();
    let pixels = (original.width, original.height);
    let t = Instant::now();
    let preview = build(
        &original,
        &identity,
        exif,
        LONG_EDGE,
        Precision::Lossless,
        Cancel::NEVER,
    )?;
    drop(original);
    let build_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    write(&out.join(format!("{name}.rmep")), &preview)?;
    let write_s = t.elapsed().as_secs_f64();
    Ok(Timed {
        name,
        pixels,
        open,
        decode,
        build: build_s,
        write: write_s,
    })
}

#[test]
#[ignore = "needs RAWs in RAWMAKASE_EDITABLE_PREVIEW_SAMPLES"]
fn pace() {
    let Some(dir) = std::env::var_os("RAWMAKASE_EDITABLE_PREVIEW_SAMPLES") else {
        eprintln!("RAWMAKASE_EDITABLE_PREVIEW_SAMPLES is not set");
        return;
    };
    let pace = std::env::var("RAWMAKASE_EDITABLE_PREVIEW_PACE").unwrap_or_else(|_| "1x2".into());
    let (builds, threads) = pace
        .split_once('x')
        .and_then(|(b, t)| Some((b.parse::<usize>().ok()?, t.parse::<usize>().ok()?)))
        .filter(|&(b, t)| b > 0 && t > 0)
        .expect("RAWMAKASE_EDITABLE_PREVIEW_PACE is <builds>x<threads>, such as 2x3");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| rawmakase_model::storage::is_raw(p))
        .collect();
    paths.sort();
    let out = tempfile::tempdir().unwrap();
    // As the build's: the cache is asked, but a decode is not stored.
    let cache = crate::decode_cache::DecodeCache::new(out.path().join("cache"), 0);
    let pool = crate::raw::background_pool(threads, "editable-preview-pace").unwrap();
    reset_peak();
    let before = memory().map(|(now, _)| now);
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let started = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..builds {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(i) else { break };
                    if builds == 1 {
                        reset_peak();
                    }
                    let timed = pool.install(|| one(path, out.path(), &cache));
                    let peak = (builds == 1).then(memory).flatten().map(|(_, peak)| peak);
                    results.lock().unwrap().push((i, timed, peak));
                }
            });
        }
    });
    let wall = started.elapsed().as_secs_f64();
    let peak = memory().map(|(_, peak)| peak);
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(i, ..)| *i);
    println!("Pace {builds}x{threads}: {builds} built side by side on {threads} threads");
    println!(
        "| File | Pixels | Open (s) | Decode (s) | Build (s) | Write (s) | Total (s) | Peak (GB) |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- |");
    let mut failed = 0;
    for (i, timed, photo_peak) in &results {
        match timed {
            Ok(t) => println!(
                "| {} | {}×{} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {} |",
                t.name,
                t.pixels.0,
                t.pixels.1,
                t.open,
                t.decode,
                t.build,
                t.write,
                t.open + t.decode + t.build + t.write,
                photo_peak.map_or("–".into(), |p| format!("{:.2}", gb(p)))
            ),
            Err(e) => {
                failed += 1;
                println!("| {} | failed: {e:#} |", paths[*i].display());
            }
        }
    }
    let built = results.len() - failed;
    let held = match (peak, before) {
        (Some(peak), Some(before)) => format!(
            "peak {:.2} GB resident, {:.2} GB of it held before the first photo",
            gb(peak),
            gb(before)
        ),
        _ => "peak memory unknown on this system".into(),
    };
    println!(
        "Batch: {built} built, {failed} failed, in {wall:.1} s ({:.2} s a photo); {held}",
        wall / built.max(1) as f64,
    );
}
