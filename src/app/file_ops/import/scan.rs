//! Import Photos' reads of its source, each on a worker of its own and
//! cancelled with the dialog or a newer read: the attached drives offered as
//! sources, the scan of a source, and the thumbnails of what it found.
//!
//! The scan walks the source as Add Folder walks a folder (photo files only,
//! no hidden files, no symbolic links) and reads each photo once
//! ([`ImportSource`]): its file object and identity, what names take from
//! it (with its capture time, which also files it By Date), then its
//! identity again. A file that changed meanwhile is read once more; one
//! that changes again, or can't be read, is listed with why and not
//! imported. Its companions are read as they are. Then the photos are
//! matched against the catalog's for suspected duplicates.
//!
//! Each scan has a generation, and every result carries it: one of an
//! older scan (the source changed, or the dialog closed) is dropped.
//! Thumbnails are the photos' embedded previews, as the Library makes them,
//! carried with the scan's generation too. These workers only read files,
//! so at exit they are left to end with the process.
use crate::app::worker::Event;
use crate::catalog::CatalogLocation;
use crate::catalog::file_ops::{self, Duplicate, ImportSource, ScannedFile};
use crate::export_settings::NameFields;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc::Sender};

/// Reads what names take from a photo file no catalog has yet, its capture
/// time included; why not when it can't be read.
pub(in crate::app) type FileReader = Arc<dyn Fn(&Path) -> Result<NameFields, String> + Send + Sync>;

/// The real reader (`naming::read`'s), reading a photo's info with `lenses`.
pub(super) fn file_reader(lenses: crate::photo::LensProfiles) -> FileReader {
    Arc::new(move |path| crate::app::export::naming::read::read_new_photo(path, &lenses))
}

/// What the scan found at one path.
#[derive(Clone, Debug)]
pub(super) enum Found {
    Photo(Box<ImportSource>),
    /// A photo file or folder that can't be read, with why.
    Unreadable {
        path: PathBuf,
        why: String,
    },
}

/// A scan's answer.
pub(crate) struct Scanned {
    pub(super) generation: u64,
    /// What it found, in path order, and each photo's suspected duplicate;
    /// why it couldn't when the source can't be read.
    pub(super) result: Result<(Vec<Found>, Vec<Option<Duplicate>>), String>,
}

/// One photo's thumbnail, by its place among the scan's photos; `None`
/// when it has none that can be read.
pub(crate) struct Thumbnail {
    pub(super) generation: u64,
    pub(super) index: usize,
    pub(super) image: Option<egui::ColorImage>,
}

/// How far a scan is.
#[derive(Clone, Default)]
pub(super) struct Progress {
    pub(super) found: Arc<AtomicUsize>,
    pub(super) read: Arc<AtomicUsize>,
}

/// Lists the drives `attached` gives on a worker, each with its `DCIM`
/// folder when it has one (a camera's card), for dialog `dialog`.
pub(super) fn list_volumes(
    dialog: u64,
    attached: fn() -> Vec<crate::platform::volume::Volume>,
    events: Sender<Event>,
    ctx: egui::Context,
) {
    crate::app::task::spawn(
        events,
        ctx,
        move |events| {
            let volumes = attached()
                .into_iter()
                .filter_map(|v| {
                    let mount = v.mount?;
                    let dcim = mount.join("DCIM");
                    let card = dcim.is_dir().then_some(dcim);
                    Some(super::Drive {
                        name: v.name,
                        mount,
                        card,
                    })
                })
                .collect();
            let _ = events.send(Event::ImportVolumes(dialog, volumes));
        },
        move |events, _| {
            let _ = events.send(Event::ImportVolumes(dialog, Vec::new()));
        },
    );
}

/// What a scan reads from.
pub(super) struct Source {
    pub(super) generation: u64,
    pub(super) folder: PathBuf,
    pub(super) subfolders: bool,
    /// The catalog suspected duplicates are looked for in.
    pub(super) catalog: CatalogLocation,
    pub(super) reader: FileReader,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) progress: Progress,
}

/// Scans `source` on a worker, which sends [`Event::ImportScanned`].
pub(super) fn start(source: Source, events: Sender<Event>, ctx: egui::Context) {
    let generation = source.generation;
    let repaint = ctx.clone();
    crate::app::task::spawn(
        events,
        ctx,
        move |events| {
            let Some(result) = scan(&source, &repaint) else {
                // Nobody waits for it any more.
                return;
            };
            let _ = events.send(Event::ImportScanned(Box::new(Scanned {
                generation,
                result,
            })));
        },
        move |events, message| {
            let failed = Scanned {
                generation,
                result: Err(message),
            };
            let _ = events.send(Event::ImportScanned(Box::new(failed)));
        },
    );
}

type ScanResult = Result<(Vec<Found>, Vec<Option<Duplicate>>), String>;

/// The scan itself; `None` once cancelled.
pub(super) fn scan(source: &Source, repaint: &egui::Context) -> Option<ScanResult> {
    let cancelled = || source.cancel.load(Ordering::Relaxed);
    let mut files = Vec::new();
    let mut found = Vec::new();
    if let Err(e) = std::fs::read_dir(&source.folder) {
        return Some(Err(format!(
            "{} can't be read: {e}",
            source.folder.display()
        )));
    }
    walk(
        &source.folder,
        source.subfolders,
        &cancelled,
        &mut files,
        &mut found,
    );
    if cancelled() {
        return None;
    }
    files.sort();
    source.progress.found.store(files.len(), Ordering::Relaxed);
    for path in files {
        if cancelled() {
            return None;
        }
        found.push(read_photo(&path, &source.reader));
        source.progress.read.fetch_add(1, Ordering::Relaxed);
        repaint.request_repaint();
    }
    found.sort_by(|a, b| path_of(a).cmp(path_of(b)));
    let sources: Vec<ImportSource> = found
        .iter()
        .filter_map(|f| match f {
            Found::Photo(s) => Some((**s).clone()),
            Found::Unreadable { .. } => None,
        })
        .collect();
    let duplicates = crate::catalog::Catalog::open(&source.catalog)
        .and_then(|catalog| file_ops::suspected_duplicates(&catalog, &sources))
        .map_err(|e| format!("The catalog could not be checked for duplicates: {e:#}"));
    Some(duplicates.map(|duplicates| (found, duplicates)))
}

pub(super) fn path_of(found: &Found) -> &Path {
    match found {
        Found::Photo(s) => &s.file.path,
        Found::Unreadable { path, .. } => path,
    }
}

/// The photo files below `folder` (in it alone without `subfolders`), as
/// Add Folder walks; a subfolder that can't be read is in `found`.
fn walk(
    folder: &Path,
    subfolders: bool,
    cancelled: &dyn Fn() -> bool,
    files: &mut Vec<PathBuf>,
    found: &mut Vec<Found>,
) {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) => {
            found.push(Found::Unreadable {
                path: folder.to_path_buf(),
                why: format!("This folder can't be read: {e}"),
            });
            return;
        }
    };
    for entry in entries {
        if cancelled() {
            return;
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() || crate::storage::is_hidden(&path) {
            continue;
        }
        if kind.is_dir() {
            if subfolders {
                walk(&path, true, cancelled, files, found);
            }
        } else if crate::storage::is_photo(&path) {
            files.push(path);
        }
    }
}

/// Reads the photo file at `path` for the import: its identity and its
/// companions' before and after what `reader` reads (its sidecar's title or
/// rating among it), read again once if any of them changed meanwhile.
pub(super) fn read_photo(path: &Path, reader: &FileReader) -> Found {
    let unreadable = |why: String| Found::Unreadable {
        path: path.to_path_buf(),
        why,
    };
    let scanned = |path: &Path| file_ops::scan_file(path).map_err(|e| format!("{e:#}"));
    // The photo, then each companion it reads, as they are now.
    let stamp = || -> Result<(ScannedFile, Vec<ScannedFile>), String> {
        let file = scanned(path)?;
        let companions = file_ops::import_companions(path)
            .map_err(|e| format!("A sidecar file can't be read: {e:#}"))?
            .iter()
            .map(|c| scanned(c).map_err(|why| format!("A sidecar file can't be read: {why}")))
            .collect::<Result<Vec<ScannedFile>, String>>()?;
        Ok((file, companions))
    };
    let mut before = match stamp() {
        Ok(stamped) => stamped,
        Err(why) => return unreadable(why),
    };
    for _ in 0..2 {
        let fields = match reader(path) {
            Ok(fields) => fields,
            Err(why) => return unreadable(why),
        };
        let after = match stamp() {
            Ok(stamped) => stamped,
            Err(why) => return unreadable(why),
        };
        if after != before {
            before = after;
            continue;
        }
        let (file, companions) = after;
        return Found::Photo(Box::new(ImportSource {
            modified: modified_time(&file.path),
            file,
            companions,
            captured: fields.captured.clone(),
            fields,
        }));
    }
    unreadable("It changed while it was read; try again once the card is idle.".into())
}

/// The local time the file at `path` was last modified, as the catalog
/// keeps a time ("2026-06-06T12:00:00.000"): what Lightroom dates a photo
/// by when it has no capture time.
fn modified_time(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let seconds = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).ok()?,
        Err(before) => -i64::try_from(before.duration().as_secs()).ok()?,
    };
    let [y, m, d, h, mi, s] = crate::platform::clock::local_time(seconds)?;
    Some(format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.000"))
}

/// Reads the thumbnails of `photos` (by their place among the scan's
/// photos) on a worker, one [`Event::ImportThumbnail`] each, until
/// `cancel` is set.
pub(super) fn thumbnails(
    generation: u64,
    photos: Vec<(usize, PathBuf)>,
    cancel: Arc<AtomicBool>,
    events: Sender<Event>,
    ctx: egui::Context,
) {
    crate::app::task::spawn(
        events,
        ctx,
        move |events| {
            for (index, path) in photos {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let image = crate::app::library::thumbnail_of(&path, EDGE).ok();
                let image = image.map(|image| {
                    let size = [image.width() as usize, image.height() as usize];
                    egui::ColorImage::from_rgb(size, image.as_raw())
                });
                let thumbnail = Thumbnail {
                    generation,
                    index,
                    image,
                };
                if events
                    .send(Event::ImportThumbnail(Box::new(thumbnail)))
                    .is_err()
                {
                    return;
                }
            }
        },
        // A thumbnail that can't be read is left blank.
        |_, _| {},
    );
}

/// The thumbnails' edge, in pixels: twice what the grid shows, for
/// high-density screens.
const EDGE: u32 = 240;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sidecar_changed_while_its_photo_is_read_is_read_again() -> anyhow::Result<()> {
        let d = tempfile::tempdir()?;
        let photo = d.path().join("A.NEF");
        let sidecar = d.path().join("A.xmp");
        std::fs::write(&photo, b"a raw")?;
        std::fs::write(&sidecar, b"<x:xmpmeta>old</x:xmpmeta>")?;
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = reads.clone();
        let edited = sidecar.clone();
        // Another program saves the sidecar as its title is read, once.
        let reader: FileReader = Arc::new(move |_: &Path| {
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                std::fs::write(&edited, b"<x:xmpmeta>a newer one</x:xmpmeta>").ok();
            }
            Ok(NameFields::default())
        });
        let Found::Photo(source) = read_photo(&photo, &reader) else {
            panic!("not read");
        };
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        assert_eq!(source.companions[0], file_ops::scan_file(&sidecar)?);
        Ok(())
    }
}
