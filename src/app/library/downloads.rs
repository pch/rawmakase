//! Photos whose originals are in Adobe's cloud (imported from a Lightroom
//! library) coming online as Lightroom downloads them, and the Lightroom
//! edits it downloads with them. Nothing else tells
//! the Library a file has appeared, so while there are such photos their
//! files are looked at every few seconds, off the UI thread; when one is
//! there with its original's size (or the size of another of its folder's,
//! which Lightroom numbered the other way round), the catalog takes it as
//! arrived and the Library shows it from its file.
use super::Library;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// How often the files are looked at.
const EVERY: Duration = Duration::from_secs(8);

#[derive(Default)]
pub(super) struct Downloads {
    /// When the files are next looked at.
    pub(super) next: Option<Instant>,
    /// The look under way: the files that may have arrived.
    pub(super) looking: Option<Receiver<Vec<(PathBuf, u64)>>>,
    /// What the last look that was settled found: the same again (a file of
    /// a pair whose other hasn't arrived) isn't settled again.
    settled: Vec<(PathBuf, u64)>,
}

impl Library {
    /// Looks for originals Lightroom has downloaded, and shows the photos
    /// whose originals are here as arrived.
    pub(super) fn poll_downloads(&mut self) {
        if let Some(looking) = &self.downloads.looking {
            match looking.try_recv() {
                Ok(found) => {
                    self.downloads.looking = None;
                    // Remembered only once settled, so a failure is tried again.
                    if !found.is_empty()
                        && found != self.downloads.settled
                        && self.settle_downloads()
                    {
                        self.downloads.settled = found;
                    }
                }
                Err(TryRecvError::Disconnected) => self.downloads.looking = None,
                Err(TryRecvError::Empty) => return,
            }
        }
        let cloud = self.session.cloud_photos();
        let edits = self.session.pending_lightroom_edits();
        if cloud.is_empty() && edits.is_empty() {
            self.downloads.next = None;
            return;
        }
        let now = Instant::now();
        let next = *self.downloads.next.get_or_insert(now + EVERY);
        if now < next {
            self.ctx.request_repaint_after(next - now);
            return;
        }
        self.downloads.next = Some(now + EVERY);
        let files: Vec<(PathBuf, u64)> = self
            .session
            .photos()
            .iter()
            .filter(|p| p.master.is_none() && !p.path.as_os_str().is_empty())
            .filter_map(|p| cloud.get(&p.id).map(|c| (p.path.clone(), c.size)))
            .collect();
        let edits = edits.to_vec();
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let mut found = arrived(&files);
            // An edit Lightroom has downloaded, with its original or after it.
            found.extend(edits.into_iter().filter_map(|edit| {
                let len = std::fs::metadata(&edit).ok()?.len();
                Some((edit, len))
            }));
            found.sort();
            let _ = tx.send(found);
            ctx.request_repaint();
        });
        self.downloads.looking = Some(rx);
    }

    /// Takes the photos whose originals are here as arrived, and shows them
    /// from their files; whether that could be done.
    pub(super) fn settle_downloads(&mut self) -> bool {
        let before: HashMap<_, _> = self
            .session
            .photos()
            .iter()
            .filter(|p| self.session.cloud(p).is_some())
            .map(|p| (p.id, p.path.clone()))
            .collect();
        match self.session.settle_lightroom_downloads() {
            Ok(settled) if settled.arrived.is_empty() && settled.edits.is_empty() => true,
            Ok(settled) => {
                // An edit read now is rendered afresh.
                for id in &settled.edits {
                    self.cache.forget(*id);
                }
                let arrived: HashSet<_> = settled.arrived.into_iter().collect();
                // Its edit failed to render while the file wasn't there, and
                // isn't asked for again unless forgotten; so for its copies.
                let ids: Vec<_> = self
                    .session
                    .photos()
                    .iter()
                    .filter(|p| arrived.contains(&p.master.unwrap_or(p.id)))
                    .map(|p| (p.id, p.path.clone()))
                    .collect();
                for (id, path) in ids {
                    self.cache.forget_file(&path);
                    self.cache.forget(id);
                    self.screen.forget(id);
                }
                for path in before.values() {
                    self.cache.forget_file(path);
                }
                self.reloaded();
                self.check_files();
                true
            }
            Err(e) => {
                self.messages
                    .warn(format!("Downloaded originals couldn't be taken: {e:#}"));
                false
            }
        }
    }
}

/// Those of `files` that are there with their original's size, or the size
/// of another of their folder's (photos of a day and name that Lightroom
/// numbered in another order), with the size they have, in order.
fn arrived(files: &[(PathBuf, u64)]) -> Vec<(PathBuf, u64)> {
    let mut sizes: HashMap<Option<&std::path::Path>, HashSet<u64>> = HashMap::new();
    for (path, size) in files {
        sizes.entry(path.parent()).or_default().insert(*size);
    }
    let mut found: Vec<(PathBuf, u64)> = files
        .iter()
        .filter_map(|(path, _)| {
            let len = std::fs::metadata(path).ok()?.len();
            sizes[&path.parent()]
                .contains(&len)
                .then(|| (path.clone(), len))
        })
        .collect();
    found.sort();
    found
}
