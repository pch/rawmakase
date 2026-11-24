//! What tells Edit In a file sent to an editor may have changed: the system's
//! file-change events for the files' folders (not the files: an editor that
//! saves by writing a new file and renaming it over the old one ends a watch
//! on the file), and, for folders on drives that send no events for another
//! computer's changes (network shares), a look every few seconds. Both run
//! on a thread of their own, which owns the system watcher, so making,
//! changing and dropping it (which on macOS waits for its run loop) never
//! holds up the window. Either wakes the window only for a followed file that
//! changed: with nothing changing, nothing runs and nothing is redrawn.
use crate::app::worker::Event;
use crate::storage::Stamp;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

/// How often the files in folders on drives without events are looked at.
const SLOW: std::time::Duration = std::time::Duration::from_secs(5);

/// The files followed, each with its size and modification time as last
/// taken up.
type Followed = Arc<Mutex<HashMap<PathBuf, Stamp>>>;

#[derive(Default)]
pub(in crate::app) struct Watch {
    files: Followed,
    /// The folders to watch, to the thread; dropping it ends the thread.
    folders: Option<mpsc::Sender<HashSet<PathBuf>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    /// Follows `files` from now on, each from the version `Stamp` names, and
    /// no others: the thread watches their folders and no others, and with no
    /// files it ends. Files whose folder can't be watched are sent to the
    /// window ([`Event::EditInUnwatched`]), to stop following.
    pub(in crate::app) fn follow(
        &mut self,
        files: &[(PathBuf, Stamp)],
        tx: &mpsc::Sender<Event>,
        ctx: &egui::Context,
    ) {
        *self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = files.iter().cloned().collect();
        if files.is_empty() {
            // Ends on its own, off the window's thread.
            self.folders = None;
            self.thread = None;
            return;
        }
        let folders: HashSet<PathBuf> = files
            .iter()
            .map(|(path, _)| crate::storage::parent_dir(path).to_path_buf())
            .collect();
        if self.folders.is_none() {
            let (send, receive) = mpsc::channel();
            self.thread = Some(watching(
                self.files.clone(),
                receive,
                tx.clone(),
                ctx.clone(),
            ));
            self.folders = Some(send);
        }
        if let Some(sender) = &self.folders {
            let _ = sender.send(folders);
        }
    }
    /// Ends the thread, at exit: its handle, to wait for within the exit's
    /// deadline.
    pub(in crate::app) fn stop(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.folders = None;
        self.thread.take()
    }
}

/// The thread: keeps the system watcher on the folders it is sent, says
/// which files can't be followed, looks at the files in folders on drives
/// without events every [`SLOW`], and ends, dropping the watcher, once its
/// sender is dropped.
fn watching(
    files: Followed,
    wanted: mpsc::Receiver<HashSet<PathBuf>>,
    tx: mpsc::Sender<Event>,
    ctx: egui::Context,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use notify::Watcher;
        let mut watcher =
            notify::recommended_watcher(handler(files.clone(), tx.clone(), ctx.clone())).ok();
        let mut folders: HashSet<PathBuf> = HashSet::new();
        // The folders on drives that may send no events.
        let mut slow: HashSet<PathBuf> = HashSet::new();
        let mut reported: HashMap<PathBuf, Stamp> = HashMap::new();
        loop {
            let next = if slow.is_empty() {
                wanted
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            } else {
                wanted.recv_timeout(SLOW)
            };
            match next {
                Ok(now) => {
                    let mut refused = Vec::new();
                    match &mut watcher {
                        Some(watcher) => {
                            for gone in folders.difference(&now) {
                                let _ = watcher.unwatch(gone);
                            }
                            for folder in now.difference(&folders) {
                                if watcher
                                    .watch(folder, notify::RecursiveMode::NonRecursive)
                                    .is_err()
                                {
                                    refused.push(folder.clone());
                                }
                            }
                        }
                        None => refused.extend(now.iter().cloned()),
                    }
                    folders = now;
                    folders.retain(|folder| !refused.contains(folder));
                    slow = folders
                        .iter()
                        .filter(|folder| needs_looking(folder))
                        .cloned()
                        .collect();
                    if !refused.is_empty() {
                        let unwatched: Vec<PathBuf> = files
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .keys()
                            .filter(|path| {
                                refused.iter().any(|f| path.parent() == Some(f.as_path()))
                            })
                            .cloned()
                            .collect();
                        if tx.send(Event::EditInUnwatched(unwatched)).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let followed: Vec<(PathBuf, Stamp)> = files
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .iter()
                        .filter(|(path, _)| path.parent().is_some_and(|f| slow.contains(f)))
                        .map(|(path, stamp)| (path.clone(), *stamp))
                        .collect();
                    let changed = changed(&followed, &mut reported);
                    if !changed.is_empty() {
                        if tx.send(Event::EditInChanged(changed)).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        // The system watcher is dropped here, off the window's thread.
        drop(watcher);
    })
}

/// Whether the folder is on a drive that may not send events for changes
/// made elsewhere: one that isn't local, or can't be told.
fn needs_looking(folder: &std::path::Path) -> bool {
    !rawmakase_fs::volume_facts(folder).is_ok_and(|facts| facts.local)
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

/// Whether the slow look looks at files in `folder`, for a test.
#[cfg(test)]
pub(in crate::app) fn looks_at(folder: &std::path::Path) -> bool {
    needs_looking(folder)
}
