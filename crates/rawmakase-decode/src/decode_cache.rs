//! Disk cache of developed camera images, so reopening a photo, or opening one that
//! was prefetched, skips decoding. An entry holds the image exactly as `raw::Raw::develop`
//! produced it, plus the pixels highlight recovery changed (usually under 1%), so the
//! recovered image is restored without recomputing it.
//!
//! Entries are keyed by the RAW file's identity (size, modification time and a hash of
//! its first 64 KB), the demosaic setting and the running executable's size and
//! modification time, so a rebuilt or updated app never reads another build's output.
//! The directory is capped; the least recently used entries are removed first. Only
//! derived pixels are stored: RAW files, sidecars and catalogs are never touched.
//! The user can move the directory to a folder of their choice, such as another
//! drive.
use crate::camera_data::{CameraImage, Demosaic, Metadata};
use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

const MAGIC: &[u8; 8] = b"RMKDEC\0\x01";
/// Default size cap of the cache directory.
pub const DEFAULT_LIMIT: u64 = 4 << 30;
/// The subfolder of a chosen folder that holds the entries.
pub const CHOSEN_SUBFOLDER: &str = "RAWmakase Cache";

/// The per-user cache directory: ~/Library/Caches/RAWmakase on macOS,
/// %LOCALAPPDATA%\RAWmakase\Cache on Windows and $XDG_CACHE_HOME/rawmakase (default
/// ~/.cache/rawmakase) elsewhere. RAWMAKASE_CACHE_DIR overrides all of them. A
/// folder chosen with [`DecodeCache::set_folder`] overrides this for decoded images.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RAWMAKASE_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(windows)
        && let Some(local) = std::env::var_os("LOCALAPPDATA")
    {
        return PathBuf::from(local).join("RAWmakase").join("Cache");
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        return home.join("Library/Caches/RAWmakase");
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(|p| PathBuf::from(p).join("rawmakase"))
        .unwrap_or_else(|| home.join(".cache/rawmakase"))
}

/// The running executable's size and modification time: what a cache entry made
/// by this build is keyed by, so another build never reads it.
pub fn build_identity() -> String {
    std::env::current_exe()
        .and_then(fs::metadata)
        .map(|m| {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            format!("{}-{modified}", m.len())
        })
        .unwrap_or_default()
}

/// The key of a result this build made from file version `id`: the file's identity
/// ([`Identity::key`](crate::storage::Identity::key)), then `own`, what the cache's
/// result depends on beyond the file (a decode's demosaic, AI Denoise's model,
/// assembly and provider, a Camera Look fit's version), then [`build_identity`].
/// Each cache keeps its own part and reads the file's identity once, when its job
/// is captured; Library previews keep their cheap stamps instead.
pub fn source_key(id: &crate::storage::Identity, own: impl std::fmt::Display) -> String {
    format!("{}-{own}-{}", id.key(), build_identity())
}

#[derive(Clone, Debug)]
pub struct DecodeCache {
    dir: Arc<RwLock<Dir>>,
    limit: Arc<AtomicU64>,
}
/// Where one kind of cache entry lives: its subfolder of the default directory and of a
/// chosen folder, and its file extension. Decoded images and AI Denoise results each
/// have their own, so relocating, purging or trimming one never touches the other's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Namespace {
    pub default: &'static str,
    pub chosen: &'static str,
    pub extension: &'static str,
}
/// Decoded images.
pub(crate) const DECODED: Namespace = Namespace {
    default: "decoded",
    chosen: CHOSEN_SUBFOLDER,
    extension: "decoded",
};
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Dir {
    pub path: PathBuf,
    /// The folder the user chose, if any. Its subfolder is created only while
    /// it exists, so an unplugged drive's mount point is never recreated on
    /// the system disk; the default directory is created with its parents.
    pub chosen: Option<PathBuf>,
}
impl Dir {
    fn of(folder: Option<&Path>) -> Self {
        Self::of_in(DECODED, folder)
    }
    pub(crate) fn of_in(namespace: Namespace, folder: Option<&Path>) -> Self {
        match folder {
            None => Self {
                path: cache_dir().join(namespace.default),
                chosen: None,
            },
            Some(folder) => Self {
                path: folder.join(namespace.chosen),
                chosen: Some(folder.to_path_buf()),
            },
        }
    }
    pub(crate) fn create(&self) -> std::io::Result<()> {
        if self.chosen.is_none() {
            return fs::create_dir_all(&self.path);
        }
        match fs::create_dir(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && self.path.is_dir() => Ok(()),
            result => result,
        }
    }
}
#[derive(Serialize, Deserialize, PartialEq)]
struct Header {
    key: String,
    width: u32,
    height: u32,
    fast: bool,
    scale_factor: f32,
    scale_clipped: u32,
    /// Pixels changed by highlight recovery, stored after the image.
    recovered: u64,
}
impl Default for DecodeCache {
    fn default() -> Self {
        Self::new(cache_dir().join("decoded"), DEFAULT_LIMIT)
    }
}
impl DecodeCache {
    pub fn new(dir: PathBuf, limit: u64) -> Self {
        Self {
            dir: Arc::new(RwLock::new(Dir {
                path: dir,
                chosen: None,
            })),
            limit: Arc::new(AtomicU64::new(limit)),
        }
    }
    fn location(&self) -> Dir {
        self.dir.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// The directory holding the entries, shared with clones.
    pub fn dir(&self) -> PathBuf {
        self.location().path
    }
    /// The folder chosen with [`set_folder`](Self::set_folder); `None` for the
    /// default directory.
    pub fn folder(&self) -> Option<PathBuf> {
        self.location().chosen
    }
    /// Keeps entries in [`CHOSEN_SUBFOLDER`] of `folder`, or in the default
    /// directory for `None`, for this cache and its clones. Entries already in
    /// the previous directory stay there.
    pub fn set_folder(&self, folder: Option<&Path>) {
        *self.dir.write().unwrap_or_else(|e| e.into_inner()) = Dir::of(folder);
    }
    /// [`set_folder`](Self::set_folder), then removes the previous directory's
    /// entries, which nothing reads any more. Returns how many were removed.
    /// The new directory is created and written to first: if either fails,
    /// the cache and its entries stay where they are.
    pub fn relocate(&self, folder: Option<&Path>) -> std::io::Result<usize> {
        let next = Dir::of(folder);
        next.create()?;
        tempfile::tempfile_in(&next.path)?;
        let previous = self.dir();
        *self.dir.write().unwrap_or_else(|e| e.into_inner()) = next;
        if previous == self.dir() {
            return Ok(0);
        }
        Ok(purge(&previous, DECODED.extension))
    }
    /// Removes every entry, and nothing else in the directory. Returns how
    /// many were removed.
    pub fn purge(&self) -> usize {
        purge(&self.dir(), DECODED.extension)
    }
    /// The size cap in bytes, shared with clones used by existing workers.
    pub fn limit(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }
    /// Changes the cap for this cache and its clones. The next store trims to
    /// the new cap; changing it performs no disk I/O on the caller's thread.
    pub fn set_limit(&self, limit: u64) {
        self.limit.store(limit, Ordering::Relaxed);
    }
    /// The key of `path` as the current build develops it at full size with `demosaic`.
    pub fn key(path: &Path, demosaic: Demosaic) -> Result<String> {
        Ok(Self::key_of(
            &crate::storage::Identity::read(path)?,
            demosaic,
        ))
    }
    /// The key of the file version `id`, as [`key`](Self::key) names it.
    pub fn key_of(id: &crate::storage::Identity, demosaic: Demosaic) -> String {
        source_key(id, format_args!("{demosaic:?}"))
    }
    fn file(&self, key: &str) -> PathBuf {
        file_in(&self.dir(), key)
    }
    pub fn contains(&self, key: &str) -> bool {
        self.file(key).is_file()
    }
    /// The cached image for `key`, with its recovered image set; `metadata` comes
    /// from opening the RAW, which is cheap. `None` on a miss or an unusable entry.
    pub fn load(&self, key: &str, metadata: &Metadata) -> Option<CameraImage> {
        let path = self.file(key);
        let image = Self::read(&path, key, metadata).ok()?;
        // Mark as recently used for eviction.
        let _ = fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(SystemTime::now()));
        Some(image)
    }
    fn read(path: &Path, key: &str, metadata: &Metadata) -> Result<CameraImage> {
        let file = fs::File::open(path)?;
        let size = file.metadata()?.len();
        let mut file = std::io::BufReader::new(file);
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        ensure!(&magic == MAGIC, "Not a decode cache entry");
        let mut len = [0; 4];
        file.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        ensure!(len < 1 << 16, "Invalid decode cache header");
        let mut header = vec![0; len];
        file.read_exact(&mut header)?;
        let header: Header = serde_json::from_slice(&header)?;
        ensure!(header.key == key, "Decode cache key collision");
        let n = header.width as u64 * header.height as u64;
        ensure!(
            n > 0 && n <= 150_000_000 && header.recovered <= n,
            "Invalid decode cache size"
        );
        let start = 12 + len as u64;
        ensure!(
            size == start + n * 12 + header.recovered * 16,
            "Decode cache entry has the wrong size"
        );
        let file = file.into_inner();
        let mut pixels = vec![[0f32; 3]; n as usize];
        read_at(&file, start, bytemuck::cast_slice_mut(&mut pixels))?;
        let mut changes = vec![Change::default(); header.recovered as usize];
        read_at(
            &file,
            start + n * 12,
            bytemuck::cast_slice_mut(&mut changes),
        )?;
        // Like a decode of the file, the loaded image measures its own Auto CA
        // rather than sharing one measured on the image `metadata` came from.
        let metadata = Metadata {
            lateral_ca: Default::default(),
            ..metadata.clone()
        };
        let image = CameraImage {
            recovered: Default::default(),
            width: header.width,
            height: header.height,
            pixels,
            metadata,
            fast: header.fast,
            scale_factor: header.scale_factor,
            scale_clipped: header.scale_clipped,
            hdr: Default::default(),
        };
        let mut recovered = CameraImage {
            recovered: Default::default(),
            pixels: vec![[0.; 3]; image.pixels.len()],
            metadata: image.metadata.clone(),
            width: image.width,
            height: image.height,
            fast: image.fast,
            scale_factor: image.scale_factor,
            scale_clipped: image.scale_clipped,
            hdr: Default::default(),
        };
        recovered
            .pixels
            .par_chunks_mut(1 << 20)
            .zip(image.pixels.par_chunks(1 << 20))
            .for_each(|(a, b)| a.copy_from_slice(b));
        for change in changes {
            let p = recovered
                .pixels
                .get_mut(change.index as usize)
                .context("Invalid recovered pixel")?;
            *p = change.value;
        }
        let _ = image.recovered.set(Arc::new(recovered));
        Ok(image)
    }
    /// Stores `image` and its recovered image (which must be set) under `key`, then
    /// trims the directory to the size cap. Written to a temporary file and renamed,
    /// so a concurrent reader never sees a partial entry.
    pub fn store(&self, key: &str, image: &CameraImage) -> Result<()> {
        // RAWs only: a rendered file opens fast enough, and an HDR file's light isn't
        // in the cache's format (docs/records/hdr-stage4-design.md).
        ensure!(
            !image.metadata.is_rendered(),
            "The decode cache keeps RAWs only"
        );
        let recovered = image.recovered.get().context("Image not recovered")?;
        ensure!(
            recovered.pixels.len() == image.pixels.len(),
            "Recovered size mismatch"
        );
        let changes: Vec<Change> = image
            .pixels
            .iter()
            .zip(&recovered.pixels)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, (_, b))| Change {
                index: i as u32,
                value: *b,
            })
            .collect();
        let header = serde_json::to_vec(&Header {
            key: key.into(),
            width: image.width,
            height: image.height,
            fast: image.fast,
            scale_factor: image.scale_factor,
            scale_clipped: image.scale_clipped,
            recovered: changes.len() as u64,
        })?;
        let dir = self.location();
        dir.create()?;
        let mut file = tempfile::NamedTempFile::new_in(&dir.path)?;
        {
            let mut out = std::io::BufWriter::new(file.as_file_mut());
            out.write_all(MAGIC)?;
            out.write_all(&(header.len() as u32).to_le_bytes())?;
            out.write_all(&header)?;
            out.write_all(bytemuck::cast_slice(&image.pixels))?;
            out.write_all(bytemuck::cast_slice(&changes))?;
            out.flush()?;
        }
        let path = file_in(&dir.path, key);
        file.persist(&path)?;
        // Moved meanwhile: the old directory's entries were removed, or are
        // about to be, and this one goes too, so none outlives the move.
        if self.location() != dir {
            let _ = fs::remove_file(&path);
            return Ok(());
        }
        self.trim(&dir.path)
    }
    /// Removes the least recently used entries until `dir` fits the cap.
    fn trim(&self, dir: &Path) -> Result<()> {
        trim(dir, DECODED.extension, self.limit())
    }
}
/// Removes the least recently used `extension` entries of `dir` until they fit `limit`.
pub(crate) fn trim(dir: &Path, extension: &str, limit: u64) -> Result<()> {
    let mut entries: Vec<(SystemTime, u64, PathBuf)> = fs::read_dir(dir)?
        .filter_map(|e| {
            let e = e.ok()?;
            let path = e.path();
            (path.extension()? == extension).then_some(())?;
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), path))
        })
        .collect();
    let mut total: u64 = entries.iter().map(|e| e.1).sum();
    entries.sort();
    for (_, size, path) in entries {
        if total <= limit {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
    Ok(())
}
fn file_in(dir: &Path, key: &str) -> PathBuf {
    entry_in(dir, key, DECODED.extension)
}
/// The file of `key` in `dir`.
pub(crate) fn entry_in(dir: &Path, key: &str, extension: &str) -> PathBuf {
    let hash = crate::storage::fnv1a(crate::storage::FNV_OFFSET, key.as_bytes());
    dir.join(format!("{hash:016x}.{extension}"))
}
/// Removes every `extension` entry of `dir`, and nothing else. Returns how many.
pub(crate) fn purge(dir: &Path, extension: &str) -> usize {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == extension))
        .filter(|e| fs::remove_file(e.path()).is_ok())
        .count()
}
/// Fills `buf` from `offset`, in parallel chunks: one reader leaves most of an SSD's
/// bandwidth unused on a 700 MB image.
pub(crate) fn read_at(file: &fs::File, offset: u64, buf: &mut [u8]) -> Result<()> {
    const CHUNK: usize = 16 << 20;
    buf.par_chunks_mut(CHUNK)
        .enumerate()
        .try_for_each(|(i, chunk)| -> std::io::Result<()> {
            let at = offset + (i * CHUNK) as u64;
            #[cfg(unix)]
            {
                std::os::unix::fs::FileExt::read_exact_at(file, chunk, at)
            }
            #[cfg(windows)]
            {
                let mut done = 0;
                while done < chunk.len() {
                    let n = std::os::windows::fs::FileExt::seek_read(
                        file,
                        &mut chunk[done..],
                        at + done as u64,
                    )?;
                    if n == 0 {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    done += n;
                }
                Ok(())
            }
        })?;
    Ok(())
}
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Change {
    index: u32,
    value: [f32; 3],
}
// SAFETY: four 4-byte fields, `repr(C)`, no padding; every bit pattern is valid.
unsafe impl bytemuck::Zeroable for Change {}
unsafe impl bytemuck::Pod for Change {}

#[cfg(test)]
mod tests {
    use super::*;
    /// A decode is filed under the file's identity, the demosaic and the build, in
    /// the form the entries already on disk have.
    #[test]
    fn a_decode_key_keeps_its_form() {
        let id = crate::storage::Identity {
            size: 12,
            modified_ns: 34,
            prefix_hash: "ab".into(),
        };
        assert_eq!(
            DecodeCache::key_of(&id, Demosaic::Rawmakase),
            format!("ab-12-34-Rawmakase-{}", build_identity())
        );
    }
    #[test]
    fn changed_limit_reaches_existing_workers_and_controls_eviction() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut rendered = image(4, 3, 0.2);
        rendered.metadata.source = rawmakase_model::camera_data::Source::Rendered;
        assert!(
            DecodeCache::new(dir.path().join("rendered"), u64::MAX)
                .store("r", &rendered)
                .is_err(),
            "the decode cache keeps RAWs only"
        );
        let cache = DecodeCache::new(dir.path().to_path_buf(), 1);
        let worker = cache.clone();
        cache.set_limit(1 << 20);
        worker.store("a", &image(40, 30, 0.2))?;
        assert!(cache.contains("a"));
        cache.set_limit(1);
        worker.store("b", &image(40, 30, 0.4))?;
        assert!(!cache.contains("a"));
        assert!(!cache.contains("b"));
        Ok(())
    }

    #[test]
    fn a_chosen_folder_reaches_existing_workers_and_relocating_removes_old_entries() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("default"), u64::MAX);
        let worker = cache.clone();
        cache.store("a", &image(40, 30, 0.2))?;
        fs::write(dir.path().join("default/notes.txt"), "kept")?;
        let chosen = dir.path().join("other drive");
        fs::create_dir(&chosen)?;
        assert_eq!(cache.relocate(Some(&chosen))?, 1);
        assert_eq!(worker.dir(), chosen.join(CHOSEN_SUBFOLDER));
        assert_eq!(worker.folder(), Some(chosen.clone()));
        assert!(!worker.contains("a"));
        assert!(dir.path().join("default/notes.txt").is_file());
        worker.store("b", &image(40, 30, 0.4))?;
        assert!(cache.contains("b"));
        assert!(fs::read_dir(chosen.join(CHOSEN_SUBFOLDER))?.count() == 1);
        // Choosing the same folder again removes nothing.
        assert_eq!(cache.relocate(Some(&chosen))?, 0);
        assert!(cache.contains("b"));
        Ok(())
    }

    /// A chosen folder whose cache subfolder cannot be made or written keeps
    /// the cache, and its entries, where they are.
    #[test]
    fn relocating_to_an_unusable_folder_changes_nothing() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("default"), u64::MAX);
        cache.store("a", &image(40, 30, 0.2))?;
        let blocked = dir.path().join("blocked");
        fs::create_dir(&blocked)?;
        fs::write(blocked.join(CHOSEN_SUBFOLDER), "a file, not a folder")?;
        for folder in [blocked, dir.path().join("missing")] {
            assert!(cache.relocate(Some(&folder)).is_err());
            assert_eq!(cache.dir(), dir.path().join("default"));
            assert!(cache.contains("a"));
        }
        Ok(())
    }

    /// An unplugged drive's folder is not recreated on the system disk: storing
    /// fails and nothing is written.
    #[test]
    fn a_missing_chosen_folder_is_not_created() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("default"), u64::MAX);
        let unplugged = dir.path().join("Volumes/Photos SSD");
        cache.set_folder(Some(&unplugged));
        assert!(cache.store("a", &image(40, 30, 0.2)).is_err());
        assert!(!dir.path().join("Volumes").exists());
        assert!(!cache.contains("a"));
        Ok(())
    }

    fn image(width: u32, height: u32, seed: f32) -> CameraImage {
        let mut im = CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: (0..width * height)
                .map(|i| [(i as f32 * 0.1 + seed).sin().abs(), 0.5, seed])
                .collect(),
            metadata: Metadata::default(),
            fast: false,
            scale_factor: 1.5,
            scale_clipped: 7,
            hdr: Default::default(),
        };
        let mut recovered = im.clone();
        recovered.pixels[3] = [2., 3., 4.];
        im.recovered = Arc::new(recovered).into();
        im
    }
    #[test]
    fn entries_round_trip_and_stay_within_the_cap() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let bytes = 40 * 30 * 12 + 200;
        let cache = DecodeCache::new(dir.path().join("decoded"), 2 * bytes);
        let metadata = Metadata {
            make: "Test".into(),
            ..Default::default()
        };
        assert!(cache.load("a", &metadata).is_none());
        let a = image(40, 30, 0.2);
        cache.store("a", &a)?;
        assert!(cache.contains("a"));
        let loaded = cache.load("a", &metadata).unwrap();
        assert_eq!(loaded.pixels, a.pixels);
        assert_eq!(loaded.metadata.make, "Test");
        assert_eq!((loaded.scale_factor, loaded.scale_clipped), (1.5, 7));
        assert_eq!(
            loaded.recovered.get().unwrap().pixels,
            a.recovered.get().unwrap().pixels
        );
        // Another key never reads this entry, even on a file-name collision.
        assert!(DecodeCache::read(&cache.file("a"), "b", &metadata).is_err());
        // A truncated entry is a miss.
        let bytes_on_disk = fs::read(cache.file("a"))?;
        fs::write(cache.file("a"), &bytes_on_disk[..bytes_on_disk.len() - 4])?;
        assert!(cache.load("a", &metadata).is_none());
        cache.store("a", &a)?;
        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.store("b", &image(40, 30, 0.4))?;
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(cache.load("a", &metadata).is_some());
        std::thread::sleep(std::time::Duration::from_millis(20));
        // The least recently used entry goes first.
        cache.store("c", &image(40, 30, 0.6))?;
        assert!(cache.contains("a") && cache.contains("c") && !cache.contains("b"));
        Ok(())
    }
}
