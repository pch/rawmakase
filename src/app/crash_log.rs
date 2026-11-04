//! What a panic leaves behind. On Windows the desktop app has no console, so a
//! panic's message would go nowhere and the app would vanish without a word
//! (#623). Every panic, including those a worker catches, is appended to
//! `crash.log` in this computer's data folder, and a panic that ends the app
//! becomes an error the caller can show.
use std::{
    backtrace::Backtrace,
    fs,
    io::Write,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

/// Past this size the log moves to `crash.old.log`, so a panic repeated on
/// every frame can't fill the disk.
const LIMIT: u64 = 1 << 20;

/// `crash.log` in this computer's own data folder, never a synced one.
pub(in crate::app) fn path() -> PathBuf {
    crate::storage::local_data_dir().join("crash.log")
}

/// Records every panic in [`path`], after the default hook prints it.
pub(in crate::app) fn install() {
    let printed = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        printed(info);
        let thread = std::thread::current();
        let text = record(
            &crate::time::now_xmp(),
            thread.name().unwrap_or("unnamed"),
            info.payload_as_str().unwrap_or("unknown panic"),
            &info
                .location()
                .map_or_else(|| "unknown location".into(), ToString::to_string),
            &Backtrace::force_capture().to_string(),
        );
        // Nothing to report a failure to from here.
        let _ = append(&path(), &text);
    }));
}

/// Runs `app`, turning a panic that unwinds out of it into an error that names
/// the crash log.
pub(in crate::app) fn run_catching<T>(
    app: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    panic::catch_unwind(AssertUnwindSafe(app)).unwrap_or_else(|panic| {
        Err(anyhow::anyhow!(
            "It stopped unexpectedly: {}\n\nDetails are in {}; please attach it to a bug report.",
            crate::app::worker::panic_message(&*panic),
            path().display()
        ))
    })
}

fn record(time: &str, thread: &str, message: &str, location: &str, backtrace: &str) -> String {
    format!(
        "=== {time} RAWmakase {} ({} {})\nthread '{thread}' panicked at {location}:\n{message}\n{backtrace}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

/// Appends `text`, first moving a full log aside. Threads panicking at once
/// take turns, so none checks the size of a log another is moving aside.
fn append(path: &Path, text: &str) -> std::io::Result<()> {
    static WRITING: Mutex<()> = Mutex::new(());
    // A poisoned lock still serializes; panicking here would abort.
    let _turn = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).is_ok_and(|m| m.len() > LIMIT) {
        fs::rename(path, path.with_file_name("crash.old.log"))?;
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_out_of_the_app_is_an_error_naming_the_crash_log() {
        let error = run_catching::<()>(|| panic!("wgpu error: device lost")).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("wgpu error: device lost"), "{text}");
        assert!(text.contains(&path().display().to_string()), "{text}");
    }

    #[test]
    fn an_app_that_returns_keeps_its_result() {
        assert_eq!(run_catching(|| Ok(7)).unwrap(), 7);
        let error = run_catching::<()>(|| Err(anyhow::anyhow!("no GPU"))).unwrap_err();
        assert_eq!(error.to_string(), "no GPU");
    }

    #[test]
    fn records_are_appended_and_a_full_log_is_moved_aside() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let log = dir.path().join("logs/crash.log");
        let first = record(
            "2026-10-26T10:00:00Z",
            "main",
            "boom",
            "src/app/mod.rs:1:2",
            "bt",
        );
        append(&log, &first)?;
        append(&log, "second\n")?;
        let text = fs::read_to_string(&log)?;
        assert!(
            text.starts_with("=== 2026-10-26T10:00:00Z RAWmakase "),
            "{text}"
        );
        assert!(text.contains("thread 'main' panicked at src/app/mod.rs:1:2:\nboom\nbt\n"));
        assert!(text.ends_with("second\n"));

        fs::write(&log, vec![b'x'; LIMIT as usize + 1])?;
        append(&log, "after\n")?;
        assert_eq!(fs::read_to_string(&log)?, "after\n");
        assert_eq!(
            fs::metadata(dir.path().join("logs/crash.old.log"))?.len(),
            LIMIT + 1
        );
        Ok(())
    }

    #[test]
    fn records_from_threads_at_once_all_survive_moving_a_full_log_aside() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let log = dir.path().join("crash.log");
        let barrier = std::sync::Barrier::new(16);
        for _round in 0..20 {
            fs::write(&log, vec![b'x'; LIMIT as usize + 1])?;
            let _ = fs::remove_file(dir.path().join("crash.old.log"));
            std::thread::scope(|s| {
                for n in 0..16 {
                    let (log, barrier) = (&log, &barrier);
                    s.spawn(move || {
                        barrier.wait();
                        append(log, &format!("record {n}\n")).unwrap();
                    });
                }
            });
            let kept = fs::read_to_string(&log)?
                + &String::from_utf8_lossy(&fs::read(dir.path().join("crash.old.log"))?);
            for n in 0..16 {
                assert!(kept.contains(&format!("record {n}\n")), "lost record {n}");
            }
        }
        Ok(())
    }
}
