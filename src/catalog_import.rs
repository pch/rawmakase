//! An import with an extension's `[[importer]]` outside the app, for
//! `rawmakase import-catalog`: the extension's program started in
//! a [`Session`] this module polls itself, `hello` and `import.run` sent and
//! the program's progress reported, then the same
//! [`PlanImport`] as the app's. What a failed `import.run` means for the
//! user ([`failure`]) is shared with the app.
//!
//! The extension is found as the app finds it: a developer folder over an
//! installed version over a bundled one, enabled or not as `extensions.json`
//! says. The program is told the settings `extensions.json` holds; secret
//! settings, which only the app reads from the system's credential store,
//! are empty here.
use crate::catalog::plan_import::PlanImport;
use anyhow::{Context, Result, anyhow, bail, ensure};
use rawmakase_extensions::protocol::manifest::{Manifest, SettingControl};
use rawmakase_extensions::protocol::method::{AppRequest, Hello, ImportDone, ImportRun};
use rawmakase_extensions::protocol::{ErrorCode, PROTOCOL, RemoteError};
use rawmakase_extensions::session::{self, Launch, Session, SessionOptions, Update, check_hello};
use rawmakase_extensions::state::State;
use rawmakase_extensions::store::Store;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

/// The longest wait between looks at the session, when nothing wakes it.
const POLL: Duration = Duration::from_millis(200);

/// What a failed `import.run` of extension `name` means for the user:
/// nothing to say when it was cancelled.
pub fn failure(name: &str, e: &RemoteError) -> Option<String> {
    Some(match e.code {
        ErrorCode::Cancelled => return None,
        ErrorCode::Disabled => format!("{name} was disabled during the import"),
        ErrorCode::Deadline => {
            format!("{name} stopped reporting progress, so the import was stopped")
        }
        ErrorCode::Exited => format!("{name} ended before it answered"),
        _ => format!("{name}: {}", e.message),
    })
}

/// An extension found, with what its program is started with.
#[derive(Clone, Debug)]
pub struct Located {
    pub manifest: Manifest,
    /// Its folder: the program runs there.
    pub dir: PathBuf,
    pub program: PathBuf,
    /// Its settings, as `extensions.json` holds them over the manifest's
    /// defaults.
    pub settings: Map<String, Value>,
}

/// Extension `id` as `data_dir`'s `extensions.json` and `bundled` (the
/// compiled-in manifests, their programs looked for in `search_dirs`) have
/// it. Fails when it is disabled ("The ‹name› extension is disabled in
/// Preferences > Extensions"), unknown, incompatible with `app`, or its program is
/// missing.
pub fn locate(
    data_dir: &Path,
    id: &str,
    bundled: &[&str],
    search_dirs: &[PathBuf],
    app: &rawmakase_extensions::semver::Version,
) -> Result<Located> {
    let state = State::read(data_dir).map_err(|e| anyhow!("{e}"))?;
    let (manifest, dir, program, enabled) = if let Some(developer) = state.developer.get(id) {
        let manifest = rawmakase_extensions::read_manifest(&developer.path)?;
        ensure!(
            manifest.extension.id == id,
            "{} now holds {}, not {id}",
            developer.path.display(),
            manifest.extension.id
        );
        let program = manifest
            .program
            .as_ref()
            .map(|p| rawmakase_extensions::program_file(&developer.path, p));
        (manifest, developer.path.clone(), program, developer.enabled)
    } else if let Some(version) = state.extensions.get(id).and_then(|e| e.active.as_ref()) {
        let dir = Store::new(data_dir).version_dir(id, version);
        let manifest = rawmakase_extensions::read_manifest(&dir)?;
        let program = manifest
            .program
            .as_ref()
            .map(|p| rawmakase_extensions::program_file(&dir, p));
        (manifest, dir, program, state.enabled(id))
    } else {
        let found = rawmakase_extensions::bundled::discover(bundled, search_dirs)
            .into_iter()
            .filter_map(Result::ok)
            .find(|b| b.manifest.extension.id == id)
            .ok_or_else(|| anyhow!("No extension {id} is installed"))?;
        // Disabled is said before a missing program.
        if let Some(problem) = found.problem().filter(|_| state.enabled(id)) {
            bail!(problem);
        }
        let dir = found
            .program
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        (found.manifest, dir, found.program, state.enabled(id))
    };
    let name = &manifest.extension.name;
    ensure!(
        enabled,
        "The {name} extension is disabled in Preferences > Extensions"
    );
    rawmakase_extensions::check_compatible(&manifest, app)?;
    let program = program
        .filter(|p| p.is_file())
        .ok_or_else(|| anyhow!("{name} has no program to run"))?;
    let entry = state.extensions.get(id);
    let settings = manifest
        .settings
        .iter()
        .map(|s| {
            let value = match s.control {
                SettingControl::Secret => Value::String(String::new()),
                _ => entry
                    .and_then(|e| e.settings.get(&s.id))
                    .filter(|v| s.control.accepts(v))
                    .cloned()
                    .unwrap_or_else(|| s.default.clone()),
            };
            (s.id.clone(), value)
        })
        .collect();
    Ok(Located {
        manifest,
        dir,
        program,
        settings,
    })
}

/// Imports `source` into a new catalog at `destination` with `extension`'s
/// importer `importer`, the program's `import.run` allowed `inactivity`
/// without progress; `data_dir` holds the extension's data and cache
/// folders. Returns the destination, made absolute, and its photos.
/// `progress` hears the program's progress and the import's.
pub fn import(
    extension: &Located,
    importer: &str,
    source: &Path,
    destination: &Path,
    data_dir: &Path,
    inactivity: Duration,
    progress: &mut dyn FnMut(&str),
) -> Result<(PathBuf, u64)> {
    ensure!(
        extension
            .manifest
            .importers
            .iter()
            .any(|i| i.id == importer),
        "{} has no importer {importer}",
        extension.manifest.extension.name
    );
    let job = PlanImport::prepare(source, destination)?;
    let run = ImportRun {
        importer: importer.to_string(),
        source: job.source().to_path_buf(),
        plan: job.plan(),
    };
    let photos = match ask(extension, run, data_dir, inactivity, progress) {
        Ok(photos) => photos,
        Err(e) => {
            // The program may not have let go of the plan yet, which
            // Windows won't delete meanwhile.
            return Err(match job.discard(session::LEASE_GRACE) {
                Ok(()) => e,
                Err(folder) => e.context(format!("{} was left behind", folder.display())),
            });
        }
    };
    // Dropping the import on failure deletes its folder.
    Ok((job.run(photos, progress)?, photos))
}

/// The program started, `hello` and `import.run` sent; the photos it
/// answered with. The program is asked to stop once it has answered, and
/// ended within [`session::QUIT_GRACE`] if it doesn't.
fn ask(
    extension: &Located,
    run: ImportRun,
    data_dir: &Path,
    inactivity: Duration,
    progress: &mut dyn FnMut(&str),
) -> Result<u64> {
    let id = extension.manifest.extension.id.clone();
    let name = extension.manifest.extension.name.clone();
    let extension_data = Store::new(data_dir).extension_data_dir(&id);
    let cache_dir = data_dir.join("extension-cache").join(&id);
    for dir in [&extension_data, &cache_dir] {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Couldn't create {}", dir.display()))?;
    }
    let mut launch = Launch {
        program: extension.program.clone(),
        args: Vec::new(),
        dir: extension.dir.clone(),
        env: Vec::new(),
        log: extension_data.join("log.txt"),
    };
    launch.set_api(extension.manifest.extension.api);
    let (woken, wake) = sync_channel::<()>(1);
    let options = SessionOptions {
        wake: Some(Arc::new(move || {
            let _ = woken.try_send(());
        })),
        ..SessionOptions::default()
    };
    let mut session =
        Session::spawn(&launch, options).map_err(|e| anyhow!("{name} didn't start: {e}"))?;
    let (method, params) = AppRequest::Hello(Hello {
        protocol: PROTOCOL,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        extension: id,
        settings: extension.settings.clone(),
        contributions: extension.manifest.contributions(),
        data_dir: extension_data,
        cache_dir,
        session: 1,
    })
    .into_parts();
    let hello = session
        .request(&method, params, Instant::now() + session::HELLO_DEADLINE)
        .map_err(|e| anyhow!("{name} didn't start: {e}"))?;
    let mut run = Some(run);
    let mut request = None;
    let answer = 'session: loop {
        let now = Instant::now();
        for update in session.poll(now) {
            match update {
                Update::Reply { id, result, .. } if id == hello => {
                    if let Err(e) = result.and_then(check_hello) {
                        break 'session Err(anyhow!("{name} didn't start: {e}"));
                    }
                    let (method, params) =
                        AppRequest::ImportRun(run.take().expect("sent once")).into_parts();
                    match session.request_with_inactivity(&method, params, inactivity, now) {
                        Ok(id) => request = Some(id),
                        Err(e) => break 'session Err(anyhow!("{name}: {e}")),
                    }
                }
                Update::Reply { id, result, .. } if Some(id) == request => {
                    break 'session match result {
                        Err(e) => Err(anyhow!(
                            failure(&name, &e).unwrap_or_else(|| "The import was stopped".into())
                        )),
                        Ok(v) => serde_json::from_value::<ImportDone>(v)
                            .map(|d| d.photos)
                            .map_err(|e| anyhow!("{name} answered something unusable: {e}")),
                    };
                }
                Update::Progress {
                    id,
                    text: Some(text),
                    ..
                } if Some(id) == request => progress(&text),
                Update::Hung { reason } => {
                    break 'session Err(anyhow!("{name} stopped responding: {reason}"));
                }
                Update::Closed => {
                    let how = session
                        .exit()
                        .map_or_else(|| "its channel closed".to_string(), |x| x.to_string());
                    break 'session Err(anyhow!("{name} ended before it answered ({how})"));
                }
                // The command line has no editor to answer the program's
                // own requests.
                Update::Request { id, .. } => {
                    let _ = session.reply(
                        id,
                        Err(RemoteError::new(
                            ErrorCode::Unavailable,
                            "the command line answers no requests during an import",
                        )),
                    );
                }
                _ => {}
            }
        }
        let wait = session
            .next_deadline()
            .map_or(POLL, |at| at.saturating_duration_since(now).min(POLL));
        let _ = wake.recv_timeout(wait);
    };
    // Done with the program, however it went: asked to exit, and ended by
    // its stopper by then if it hasn't.
    if let Ok(stopping) = session.stop(Instant::now() + session::QUIT_GRACE * 2) {
        stopping.join();
    }
    answer
}
