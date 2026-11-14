//! `<data>/extensions.json`: which extensions are installed, which version of
//! each is active, whether each is enabled, where it came from and its
//! settings.
//!
//! ```json
//! {"version": 1,
//!  "extensions": {"dev.jane.analog": {"active": "1.2.0", "installed": ["1.2.0"],
//!     "enabled": true, "source": {"kind": "github", "repository": "https://github.com/jane/analog"},
//!     "settings": {"default-stock": "HP5"}}},
//!  "developer": {"dev.jane.analog": {"path": "/home/jane/analog", "enabled": true}}}
//! ```
//!
//! The app and `rawmakase extensions` may both change it, so every change goes
//! through [`State::update`]: it takes the lock file `extensions.lock`, reads
//! the file again, applies one narrow change and writes the file atomically,
//! so neither overwrites what the other changed meanwhile. Fields this
//! version doesn't know are kept, at every level, and a file a newer
//! RAWmakase wrote (a higher `version`) is read but never written.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The file's name in the data folder.
pub const FILE: &str = "extensions.json";
/// The lock file every change holds.
pub const LOCK: &str = "extensions.lock";
/// The format this version writes, and the newest it reads.
pub const VERSION: u32 = 1;
/// How long a change waits for another one holding the lock.
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// The whole file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The file's format.
    pub version: u32,
    /// Installed and bundled extensions, by id. A bundled extension has an
    /// entry only once the user changed something about it.
    #[serde(default)]
    pub extensions: BTreeMap<String, Entry>,
    /// Unpacked extensions loaded from a folder in developer mode, by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub developer: BTreeMap<String, Developer>,
    /// Fields a later release wrote.
    #[serde(flatten)]
    pub unknown: Map<String, Value>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: VERSION,
            extensions: BTreeMap::new(),
            developer: BTreeMap::new(),
            unknown: Map::new(),
        }
    }
}

/// One extension's entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The version the app runs, one of `installed`; none for a bundled
    /// extension, which runs the app's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// The versions unpacked under `extensions/<id>/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub installed: Vec<String>,
    /// Off means its commands, panels and program are off.
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Where it was installed from, as written (see [`Entry::source`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<Value>,
    /// Its settings' values, by setting id; a setting without one has its
    /// manifest default.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub settings: Map<String, Value>,
    /// The catalogs each of its `[[source]]`s adds photos to, by source id:
    /// each catalog's file on this computer, as the app opened it. A source
    /// is off for every catalog not listed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, Vec<String>>,
    /// Fields a later release wrote.
    #[serde(flatten)]
    pub unknown: Map<String, Value>,
}
fn enabled() -> bool {
    true
}
impl Default for Entry {
    fn default() -> Self {
        Self {
            active: None,
            installed: Vec::new(),
            enabled: true,
            source: None,
            settings: Map::new(),
            sources: BTreeMap::new(),
            unknown: Map::new(),
        }
    }
}
impl Entry {
    /// A new entry for extension `id`, enabled unless it is a bundled
    /// extension that starts off ([`crate::bundled::OFF_BY_DEFAULT`]), so a
    /// setting changed before it was ever enabled doesn't enable it.
    pub fn new(id: &str) -> Self {
        Self {
            enabled: crate::bundled::enabled_by_default(id),
            ..Self::default()
        }
    }
    /// Where it came from; none when unknown to this version, which keeps
    /// the source as written.
    pub fn source(&self) -> Option<Source> {
        self.source
            .clone()
            .and_then(|s| serde_json::from_value(s).ok())
    }
    /// Records where it came from.
    pub fn set_source(&mut self, source: &Source) {
        self.source = Some(serde_json::to_value(source).expect("a source is JSON"));
    }
    /// Whether its source `source` adds photos to the catalog `catalog`.
    pub fn source_on(&self, source: &str, catalog: &str) -> bool {
        self.sources
            .get(source)
            .is_some_and(|catalogs| catalogs.iter().any(|c| c == catalog))
    }
    /// Turns its source `source` on or off for the catalog `catalog`.
    pub fn set_source_on(&mut self, source: &str, catalog: &str, on: bool) {
        let catalogs = self.sources.entry(source.to_string()).or_default();
        catalogs.retain(|c| c != catalog);
        if on {
            catalogs.push(catalog.to_string());
        }
        if catalogs.is_empty() {
            self.sources.remove(source);
        }
    }
}

/// Where an installed extension came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    /// It ships with the app.
    Bundled,
    /// A package file.
    File {
        /// The file installed, for the user's reference.
        path: PathBuf,
    },
    /// A GitHub repository's releases, which Check for Updates reads again.
    Github {
        /// `https://github.com/<owner>/<repo>`.
        repository: String,
    },
}
impl Source {
    /// How Preferences names it.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Bundled => "Bundled",
            Self::File { .. } => "File",
            Self::Github { .. } => "GitHub",
        }
    }
}

/// An unpacked extension in developer mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Developer {
    /// The folder holding its manifest.
    pub path: PathBuf,
    /// Whether it runs.
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Fields a later release wrote.
    #[serde(flatten)]
    pub unknown: Map<String, Value>,
}

/// Why the file couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// A newer RAWmakase wrote it; this one leaves it as it is.
    Newer(u32),
    /// It isn't JSON of this shape; it is left as it is.
    Unreadable(String),
    /// The file system refused.
    Io(String),
}
impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Newer(v) => write!(
                f,
                "{FILE} is version {v}, from a newer RAWmakase; this one leaves it unchanged"
            ),
            Self::Unreadable(why) => {
                write!(f, "{FILE} can't be read ({why}); it is left unchanged")
            }
            Self::Io(why) => write!(f, "{FILE} can't be read: {why}"),
        }
    }
}
impl std::error::Error for ReadError {}
impl From<ReadError> for Error {
    fn from(e: ReadError) -> Self {
        Error::new(e.to_string())
    }
}

impl State {
    /// The file in `data_dir`; the default state when there is none.
    pub fn read(data_dir: &Path) -> std::result::Result<Self, ReadError> {
        let path = data_dir.join(FILE);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(ReadError::Io(e.to_string())),
        };
        // The version first, so a newer file is told apart from a broken one.
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|e| ReadError::Unreadable(e.to_string()))?;
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| ReadError::Unreadable("it has no version".into()))?;
        if version > u64::from(VERSION) {
            return Err(ReadError::Newer(version.try_into().unwrap_or(u32::MAX)));
        }
        serde_json::from_value(value).map_err(|e| ReadError::Unreadable(e.to_string()))
    }

    /// Applies one change to the file in `data_dir` under its lock: reads it
    /// again, calls `change`, and writes the result atomically when it differs.
    /// Nothing is written when `change` fails, or when the file is newer or
    /// unreadable.
    pub fn update<T>(data_dir: &Path, change: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        Self::update_or_undo(data_dir, change, |_| {})
    }

    /// [`State::update`] for a change that also changes files: when `change`
    /// succeeds but its state can't be written, `undo` is given its result,
    /// still under the lock, to put those files back.
    pub(crate) fn update_or_undo<T>(
        data_dir: &Path,
        change: impl FnOnce(&mut State) -> Result<T>,
        undo: impl FnOnce(T),
    ) -> Result<T> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| Error::from(e).context(data_dir.display()))?;
        let _lock = lock(data_dir)?;
        let before = Self::read(data_dir)?;
        let mut state = before.clone();
        let result = change(&mut state)?;
        if state != before {
            state.version = VERSION;
            if let Err(e) = write(data_dir, &state) {
                undo(result);
                return Err(e);
            }
        }
        Ok(result)
    }

    /// The entry for `id`, made by [`Entry::new`] when there is none.
    pub fn entry(&mut self, id: &str) -> &mut Entry {
        self.extensions
            .entry(id.to_string())
            .or_insert_with(|| Entry::new(id))
    }

    /// Whether extension `id` is enabled: its entry's state, or without one
    /// [`bundled::enabled_by_default`](crate::bundled::enabled_by_default).
    pub fn enabled(&self, id: &str) -> bool {
        self.extensions
            .get(id)
            .map_or_else(|| crate::bundled::enabled_by_default(id), |e| e.enabled)
    }
}

/// Holds the lock file while it is open; waits for another holder at most
/// [`LOCK_WAIT`].
fn lock(data_dir: &Path) -> Result<std::fs::File> {
    let file = std::fs::File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join(LOCK))
        .map_err(|e| Error::from(e).context(data_dir.join(LOCK).display()))?;
    let start = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < LOCK_WAIT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::new(format!(
                    "Another RAWmakase is changing {FILE}; try again"
                )));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

/// Writes the file beside its place, syncs it, renames it over the old one and
/// syncs the folder, so a crash leaves the old file or the new one.
fn write(data_dir: &Path, state: &State) -> Result<()> {
    let path = data_dir.join(FILE);
    let staged = data_dir.join(format!(".{FILE}.{}.tmp", crate::random_suffix()));
    let written = (|| -> std::io::Result<()> {
        let mut bytes = serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        let mut file = std::fs::File::create_new(&staged)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&staged, &path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&staged);
        return Err(Error::from(e).context(format!("Could not write {}", path.display())));
    }
    let _ = rawmakase_fs::sync_dir(data_dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_missing_file_reads_as_empty_and_a_change_writes_it() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(State::read(dir.path()).unwrap(), State::default());
        State::update(dir.path(), |s| {
            s.entry("dev.a.b").enabled = false;
            Ok(())
        })
        .unwrap();
        let state = State::read(dir.path()).unwrap();
        assert!(!state.extensions["dev.a.b"].enabled);
        // An unchanged state writes nothing.
        let path = dir.path().join(FILE);
        let before = std::fs::read(&path).unwrap();
        std::fs::write(&path, &before).unwrap();
        State::update(dir.path(), |_| Ok(())).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    /// MIDI starts off without an entry, and a setting changed before it
    /// was ever enabled leaves it off; others start on.
    #[test]
    fn midi_is_off_until_enabled() {
        let mut state = State::default();
        assert!(!state.enabled("rawmakase.midi"));
        assert!(state.enabled("rawmakase.mcp"));
        assert!(state.enabled("dev.a.b"));
        state
            .entry("rawmakase.midi")
            .settings
            .insert("speed".into(), 2.into());
        assert!(!state.enabled("rawmakase.midi"));
        state.entry("rawmakase.midi").enabled = true;
        assert!(state.enabled("rawmakase.midi"));
        assert!(state.entry("dev.a.b").enabled);
    }

    #[test]
    fn fields_this_version_does_not_know_survive_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let later = json!({
            "version": 1,
            "catalogue": {"url": "https://example.org"},
            "extensions": {"dev.a.b": {
                "active": "1.0.0", "installed": ["1.0.0"], "enabled": true,
                "source": {"kind": "catalogue", "index": "official"},
                "pinned": true
            }},
            "developer": {"dev.c.d": {"path": "/src/d", "enabled": true, "watch": false}}
        });
        std::fs::write(dir.path().join(FILE), later.to_string()).unwrap();
        State::update(dir.path(), |s| {
            s.entry("dev.a.b").settings.insert("speed".into(), json!(2));
            Ok(())
        })
        .unwrap();
        let written: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(FILE)).unwrap()).unwrap();
        assert_eq!(written["catalogue"], later["catalogue"]);
        let entry = &written["extensions"]["dev.a.b"];
        assert_eq!(entry["pinned"], json!(true));
        assert_eq!(entry["source"], later["extensions"]["dev.a.b"]["source"]);
        assert_eq!(entry["settings"]["speed"], json!(2));
        assert_eq!(written["developer"]["dev.c.d"]["watch"], json!(false));
        // A source this version doesn't know reads as none, and stays.
        let state = State::read(dir.path()).unwrap();
        assert_eq!(state.extensions["dev.a.b"].source(), None);
    }

    #[test]
    fn a_newer_or_broken_file_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        for text in [
            r#"{"version": 2, "extensions": {}}"#,
            "{not json",
            r#"{"extensions":{}}"#,
        ] {
            std::fs::write(dir.path().join(FILE), text).unwrap();
            assert!(State::read(dir.path()).is_err());
            let error = State::update(dir.path(), |s| {
                s.entry("dev.a.b").enabled = false;
                Ok(())
            })
            .unwrap_err();
            assert!(!error.0.is_empty());
            assert_eq!(
                std::fs::read_to_string(dir.path().join(FILE)).unwrap(),
                text
            );
        }
        std::fs::write(dir.path().join(FILE), r#"{"version": 2}"#).unwrap();
        assert_eq!(State::read(dir.path()), Err(ReadError::Newer(2)));
    }

    #[test]
    fn a_failed_change_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let error = State::update(dir.path(), |s| -> Result<()> {
            s.entry("dev.a.b").enabled = false;
            Err(Error::new("refused"))
        })
        .unwrap_err();
        assert_eq!(error.0, "refused");
        assert!(!dir.path().join(FILE).exists());
    }

    /// Two writers, as the app and the command line, each change their own
    /// extension many times at once: every change survives.
    #[test]
    fn interleaved_changes_from_two_writers_all_survive() {
        let dir = tempfile::tempdir().unwrap();
        let writers: Vec<_> = ["dev.app.one", "dev.cli.two"]
            .into_iter()
            .map(|id| {
                let dir = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    for n in 0..20 {
                        State::update(&dir, |s| {
                            s.entry(id).settings.insert(format!("s{n}"), json!(n));
                            Ok(())
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let state = State::read(dir.path()).unwrap();
        for id in ["dev.app.one", "dev.cli.two"] {
            assert_eq!(state.extensions[id].settings.len(), 20, "{id}");
        }
    }

    #[test]
    fn sources_round_trip() {
        let mut entry = Entry::default();
        for source in [
            Source::Bundled,
            Source::File {
                path: "/tmp/x.rmext".into(),
            },
            Source::Github {
                repository: "https://github.com/a/b".into(),
            },
        ] {
            entry.set_source(&source);
            assert_eq!(entry.source(), Some(source));
        }
    }

    #[test]
    fn a_photo_source_is_turned_on_per_catalog_and_kept_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        State::update(dir.path(), |s| {
            let entry = s.entry("dev.a.tether");
            entry.set_source_on("camera", "/c/Studio.rawmakase", true);
            entry.set_source_on("camera", "/c/Studio.rawmakase", true);
            entry.set_source_on("camera", "/c/Home.rawmakase", true);
            Ok(())
        })
        .unwrap();
        let state = State::read(dir.path()).unwrap();
        let entry = &state.extensions["dev.a.tether"];
        assert_eq!(
            entry.sources["camera"],
            ["/c/Studio.rawmakase", "/c/Home.rawmakase"]
        );
        assert!(entry.source_on("camera", "/c/Home.rawmakase"));
        assert!(!entry.source_on("camera", "/c/Other.rawmakase"));
        assert!(!entry.source_on("folder", "/c/Home.rawmakase"));
        let mut entry = entry.clone();
        entry.set_source_on("camera", "/c/Studio.rawmakase", false);
        entry.set_source_on("camera", "/c/Home.rawmakase", false);
        assert!(
            entry.sources.is_empty(),
            "a source on for nothing is dropped"
        );
        let written = serde_json::to_value(&entry).unwrap();
        assert!(written.get("sources").is_none());
    }
}
