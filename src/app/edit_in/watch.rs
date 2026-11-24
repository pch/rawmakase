//! What tells Edit In a file sent to an editor may have changed: the system's
//! file-change events for the files' folders (not the files: an editor that
//! saves by writing a new file and renaming it over the old one ends a watch
//! on the file), and, for drives that send no events for another computer's
//! changes (network shares), a look of its own every few seconds on a thread
//! of its own. Either wakes the window only for a followed file that changed:
//! with nothing changing, nothing runs and nothing is redrawn.
use crate::app::worker::Event;
use crate::storage::Stamp;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

/// How often the slow look stats the files followed.
const SLOW: std::time::Duration = std::time::Duration::from_secs(5);

/// The files followed, each with its size and modification time as last
/// taken up.
type Followed = Arc<Mutex<HashMap<PathBuf, Stamp>>>;

#[derive(Default)]
pub(in crate::app) struct Watch {
    files: Followed,
    watcher: Option<notify::RecommendedWatcher>,
    folders: HashSet<PathBuf>,
    /// The slow look's thread ends once this is dropped.
    slow: Option<mpsc::Sender<()>>,
}

impl Watch {
    /// Follows `files` from now on, each from the version `Stamp` names, and
    /// no others: their folders are watched, the folders no file is in any
    /// more no longer, and with no files the watcher and the slow look stop.
    /// The files whose folder can't be watched, to stop following.
    pub(in crate::app) fn follow(
        &mut self,
        files: &[(PathBuf, Stamp)],
        tx: &mpsc::Sender<Event>,
        ctx: &egui::Context,
    ) -> Vec<PathBuf> {
        *self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = files.iter().cloned().collect();
        if files.is_empty() {
            self.stop();
            return Vec::new();
        }
        if self.slow.is_none() {
            self.slow = Some(slow_look(self.files.clone(), tx.clone(), ctx.clone()));
        }
        let wanted: HashSet<PathBuf> = files
            .iter()
            .map(|(path, _)| crate::storage::parent_dir(path).to_path_buf())
            .collect();
        if self.watcher.is_none() {
            match notify::recommended_watcher(handler(self.files.clone(), tx.clone(), ctx.clone()))
            {
                Ok(watcher) => self.watcher = Some(watcher),
                Err(_) => return files.iter().map(|(path, _)| path.clone()).collect(),
            }
        }
        let watcher = self.watcher.as_mut().expect("made above");
        use notify::Watcher;
        for gone in self.folders.difference(&wanted) {
            let _ = watcher.unwatch(gone);
        }
        self.folders.retain(|folder| wanted.contains(folder));
        let mut unwatched = Vec::new();
        for folder in wanted {
            if self.folders.contains(&folder) {
                continue;
            }
            match watcher.watch(&folder, notify::RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.folders.insert(folder);
                }
                Err(_) => unwatched.extend(
                    files
                        .iter()
                        .filter(|(path, _)| crate::storage::parent_dir(path) == folder)
                        .map(|(path, _)| path.clone()),
                ),
            }
        }
        unwatched
    }
    /// Stops watching: the watcher's own thread ends as it is dropped, and the
    /// slow look's at its next wake, within [`SLOW`].
    pub(in crate::app) fn stop(&mut self) {
        self.watcher = None;
        self.folders.clear();
        self.slow = None;
    }
}

/// Sends the window the followed files an event names, and wakes it; events
/// for other files in the folders, and reads, wake nothing.
fn handler(
    files: Followed,
    tx: mpsc::Sender<Event>,
    ctx: egui::Context,
) -> impl notify::EventHandler {
    move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else {
            return;
        };
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        let followed = files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed: Vec<PathBuf> = event
            .paths
            .into_iter()
            .filter(|path| followed.contains_key(path))
            .collect();
        drop(followed);
        if !changed.is_empty() && tx.send(Event::EditInChanged(changed)).is_ok() {
            ctx.request_repaint();
        }
    }
}

/// Starts the slow look: every [`SLOW`], a stat of each file followed; one
/// that isn't as last taken up, and wasn't so at the look before, is sent to
/// the window, which is woken. It ends once the returned sender is dropped.
fn slow_look(files: Followed, tx: mpsc::Sender<Event>, ctx: egui::Context) -> mpsc::Sender<()> {
    let (stop, stopped) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut reported: HashMap<PathBuf, Stamp> = HashMap::new();
        while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(SLOW) {
            let followed: Vec<(PathBuf, Stamp)> = files
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|(path, stamp)| (path.clone(), *stamp))
                .collect();
            let changed = changed(&followed, &mut reported);
            if !changed.is_empty() {
                if tx.send(Event::EditInChanged(changed)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        }
    });
    stop
}

/// The files of `followed` whose size or modification time isn't as last
/// taken up and wasn't so at the last look (`reported`, which keeps it).
pub(in crate::app) fn changed(
    followed: &[(PathBuf, Stamp)],
    reported: &mut HashMap<PathBuf, Stamp>,
) -> Vec<PathBuf> {
    reported.retain(|path, _| followed.iter().any(|(p, _)| p == path));
    followed
        .iter()
        .filter_map(|(path, baseline)| {
            let now = Stamp::read(path).ok()?;
            (now != *baseline && reported.insert(path.clone(), now) != Some(now))
                .then(|| path.clone())
        })
        .collect()
}

/// Whether `path` is followed, for a test.
#[cfg(test)]
pub(in crate::app) fn follows(watch: &Watch, path: &std::path::Path) -> bool {
    watch.files.lock().unwrap().contains_key(path)
}
