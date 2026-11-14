//! A test-only extension of a photo source (`sources/rawmakase-extension.toml`):
//! it reports the files a test names, as a watched folder or a tethered camera
//! reports what arrives, so the app's tests can check scopes and reports end
//! to end.
//!
//! - Each scope started or stopped is said in a notification: `started
//!   <scope> <catalog>`, `stopped <scope>`.
//! - A file `report.json` in its working folder, `{"folder": …, "files": [
//!   …], "show": …}`, is taken (renamed) and reported in the scope running
//!   then, or in `"scope"` when it names one (a scope that has stopped, say).
//!   The reply, or `{"error": code}`, is written to `reported.json`, whole,
//!   and the report is said in a notification, `reported`.
use rawmakase_extension::protocol::method::{Level, SourceReport, SourceStarted};
use rawmakase_extension::{Extension, Host, RemoteError, Source};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The drop folder: one watcher for the program's life, reporting in
/// whichever scope runs.
#[derive(Default)]
struct Drop {
    watcher: Arc<Watcher>,
    watching: bool,
}

/// What the run loop's thread and the watcher share.
#[derive(Default)]
struct Watcher {
    /// The scope running now.
    scope: Mutex<Option<u64>>,
    /// Scopes stopped and not yet said.
    stopped: Mutex<Vec<u64>>,
}

impl Source for Drop {
    fn start(&mut self, host: Host, started: SourceStarted) -> Result<(), RemoteError> {
        *self.watcher.scope.lock().unwrap() = Some(started.scope);
        let _ = host.notify(
            Level::Info,
            &format!("started {} {}", started.scope, started.catalog),
        );
        if !self.watching {
            self.watching = true;
            let watcher = self.watcher.clone();
            std::thread::spawn(move || watch(&host, &watcher));
        }
        Ok(())
    }
    /// The watcher says so: the run loop's own thread doesn't wait for the
    /// app's answer while the session may be ending.
    fn stop(&mut self, scope: u64) {
        let mut current = self.watcher.scope.lock().unwrap();
        if *current == Some(scope) {
            *current = None;
        }
        self.watcher.stopped.lock().unwrap().push(scope);
    }
}

/// Says each scope stopped, and takes each `report.json` in the working
/// folder and reports it.
fn watch(host: &Host, watcher: &Watcher) {
    loop {
        let stopped = std::mem::take(&mut *watcher.stopped.lock().unwrap());
        for scope in stopped {
            if host
                .notify(Level::Info, &format!("stopped {scope}"))
                .is_err()
            {
                return;
            }
        }
        if let Some(asked) = take(Path::new("report.json")) {
            let now = *watcher.scope.lock().unwrap();
            let scope = asked["scope"].as_u64().or(now).unwrap_or(0);
            let files = asked["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(PathBuf::from))
                .collect();
            let report = SourceReport {
                source: "drop".into(),
                scope,
                folder: asked["folder"].as_str().unwrap_or_default().into(),
                files,
                show: asked["show"].as_bool().unwrap_or(false),
            };
            let reply = match host.report(report) {
                Ok(reported) => serde_json::to_value(reported).expect("a reply is JSON"),
                Err(e) => json!({"error": e.code.as_str()}),
            };
            let written = Path::new("reported.json.part");
            if std::fs::write(written, reply.to_string()).is_err()
                || std::fs::rename(written, "reported.json").is_err()
                || host.notify(Level::Info, "reported").is_err()
            {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The JSON of `path`, which is taken away, if it is there.
fn take(path: &Path) -> Option<Value> {
    let taken = path.with_extension("json.taken");
    std::fs::rename(path, &taken).ok()?;
    let text = std::fs::read_to_string(&taken).ok()?;
    serde_json::from_str(&text).ok()
}

/// The program's extension.
pub fn extension() -> Extension {
    Extension::new().source("drop", Drop::default())
}
