//! Disk cache of developed camera images, so reopening a photo, or opening one that
//! was prefetched, skips decoding. An entry holds the image exactly as `raw::Raw::develop`
//! produced it, plus the pixels highlight recovery changed (3% in the median photo
//! measured, #1173), so the recovered image is restored without recomputing it. A
//! pixel counts as changed when any bit of it is (a zero that changed sign too). The
//! two images are loaded as one buffer and those pixels ([`Pixels::pair_masked`]),
//! not as a buffer each.
//!
//! Entries are keyed by the RAW file's identity (size, modification time and a hash of
//! its first 64 KB), the demosaic setting and the running executable's size and
//! modification time, so a rebuilt or updated app never reads another build's output.
//! The directory is capped; the least recently used entries are removed first. Only
//! derived pixels are stored: RAW files, sidecars and catalogs are never touched.
//! The user can move the directory to a folder of their choice, such as another
//! drive.
use crate::camera_data::{CameraImage, Demosaic, Metadata, Pixels};
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
/// Pixels written at a time when an entry is stored.
const PIECE: usize = 1 << 18;
/// Changed pixels one task puts back at a time when an entry is loaded.
const TAKE: usize = 1 << 14;
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
        Self::under(&cache_dir(), DEFAULT_LIMIT)
    }
}
impl DecodeCache {
    /// Decoded images in their subfolder of the cache folder `cache`, up to
    /// `limit` bytes.
    pub fn under(cache: &Path, limit: u64) -> Self {
        Self::new(cache.join(DECODED.default), limit)
    }
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
        // The entry's pixels become the recovered image's, and the decode keeps the
        // values the changed ones had: the two images hold one buffer between them.
        let (changed, had) = exchange(&mut pixels, &changes)?;
        drop(changes);
        let (pixels, recovered) =
            Pixels::pair_masked(pixels, changed, had).context("Invalid recovered pixel")?;
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
        let recovered = CameraImage {
            recovered: Default::default(),
            pixels: recovered,
            metadata: image.metadata.clone(),
            width: image.width,
            height: image.height,
            fast: image.fast,
            scale_factor: image.scale_factor,
            scale_clipped: image.scale_clipped,
            hdr: Default::default(),
        };
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
        let change = |i: usize| Change {
            index: i as u32,
            value: recovered.pixels[i],
        };
        // Images that hold one buffer between them list the pixels that differ;
        // images with a buffer each are compared, bit by bit as recovery in place
        // lists them, so an entry is the same whichever way its image is held.
        let changes: Vec<Change> = if image.pixels.shares_buffer(&recovered.pixels) {
            image.pixels.differing().map(change).collect()
        } else {
            (image.pixels.iter().zip(recovered.pixels.iter()).enumerate())
                .filter(|(_, (a, b))| a.map(f32::to_bits) != b.map(f32::to_bits))
                .map(|(i, _)| change(i))
                .collect()
        };
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
            // A piece at a time: a decode kept as its recovered image's buffer
            // is never copied out whole.
            let n = image.pixels.len();
            for start in (0..n).step_by(PIECE) {
                let piece = image.pixels.range(start..(start + PIECE).min(n));
                out.write_all(bytemuck::cast_slice(&piece[..]))?;
            }
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
/// Puts each change's recovered value in `pixels`, the image as decoded, which
/// become the recovered image's. Returns which pixels changed, one bit a pixel
/// (64 a word, from the lowest bit), and the values those had, in pixel order:
/// what [`Pixels::pair_masked`] takes. `changes` must be in ascending order of
/// index, each within `pixels`, as [`DecodeCache::store`] writes them.
fn exchange(pixels: &mut [[f32; 3]], changes: &[Change]) -> Result<(Vec<u64>, Vec<[f32; 3]>)> {
    let n = pixels.len();
    ensure!(
        changes.par_windows(2).all(|w| w[0].index < w[1].index)
            && changes.last().is_none_or(|c| (c.index as usize) < n),
        "Invalid recovered pixel"
    );
    let mut changed = vec![0u64; n.div_ceil(64)];
    let mut had = vec![[0f32; 3]; changes.len()];
    // A photo recovery changed much of has millions of changes (17 million in one
    // measured), so they are shared out: each task takes a run of changes and the
    // whole words of the mask they fall in, with those words' pixels, so no two
    // tasks write to the same word, pixel or value.
    let mut tasks = Vec::new();
    let (mut words, mut pixels, mut values) = (&mut changed[..], pixels, &mut had[..]);
    let (mut start, mut first_word) = (0, 0);
    while start < changes.len() {
        let mut end = (start + TAKE).min(changes.len());
        // On to the first change that falls in another word.
        while end < changes.len() && changes[end].index / 64 == changes[end - 1].index / 64 {
            end += 1;
        }
        let next_word = match changes.get(end) {
            Some(change) => change.index as usize / 64,
            None => n.div_ceil(64),
        };
        let count = next_word - first_word;
        let (task_words, rest) = std::mem::take(&mut words).split_at_mut(count);
        words = rest;
        let split = (count * 64).min(pixels.len());
        let (task_pixels, rest) = std::mem::take(&mut pixels).split_at_mut(split);
        pixels = rest;
        let (task_values, rest) = std::mem::take(&mut values).split_at_mut(end - start);
        values = rest;
        let run = &changes[start..end];
        tasks.push((run, task_words, task_pixels, task_values, first_word * 64));
        (start, first_word) = (end, next_word);
    }
    tasks
        .into_par_iter()
        .for_each(|(run, words, pixels, values, first)| {
            for (change, had) in run.iter().zip(values) {
                let i = change.index as usize - first;
                *had = std::mem::replace(&mut pixels[i], change.value);
                words[i / 64] |= 1 << (i % 64);
            }
        });
    Ok((changed, had))
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
    /// A loaded entry holds one buffer for the image and its recovered image,
    /// both read as they were stored. Storing it again writes the entry an image
    /// with a buffer each wrote, byte for byte, without copying the image out.
    #[test]
    fn a_loaded_entry_is_one_buffer_and_stores_the_same_entry() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let first = DecodeCache::new(dir.path().join("first"), u64::MAX);
        let second = DecodeCache::new(dir.path().join("second"), u64::MAX);
        let metadata = Metadata::default();
        // Pixels recovery changed at both ends, next to each other and apart.
        let mut a = image(70, 30, 0.2);
        let mut recovered = (**a.recovered.get().unwrap()).clone();
        for i in [0, 1, 63, 64, 1000, 2099] {
            recovered.pixels[i] = [5., 6., i as f32];
        }
        a.recovered = Arc::new(recovered).into();
        let apart = a.recovered.get().unwrap();
        assert!(!a.pixels.shares_buffer(&apart.pixels));
        first.store("a", &a)?;
        let loaded = first.load("a", &metadata).unwrap();
        let one = loaded.recovered.get().unwrap();
        assert!(loaded.pixels.shares_buffer(&one.pixels));
        assert_eq!(loaded.pixels, a.pixels);
        assert_eq!(one.pixels, apart.pixels);
        assert_eq!(
            loaded.pixels.differing().collect::<Vec<_>>(),
            [0, 1, 3, 63, 64, 1000, 2099]
        );
        second.store("a", &loaded)?;
        assert_eq!(fs::read(first.file("a"))?, fs::read(second.file("a"))?);
        assert!(loaded.pixels.shares_buffer(&one.pixels));
        // Read as one slice, the image is copied out and holds a buffer of its own
        // again: it still stores the same entry.
        assert_eq!(loaded.pixels[..].len(), 2100);
        assert!(!loaded.pixels.shares_buffer(&one.pixels));
        let third = DecodeCache::new(dir.path().join("third"), u64::MAX);
        third.store("a", &loaded)?;
        assert_eq!(fs::read(first.file("a"))?, fs::read(third.file("a"))?);
        // An image recovery changed nowhere: no changes stored, one buffer loaded.
        let mut same = image(70, 30, 0.4);
        same.recovered = Arc::new(CameraImage {
            recovered: Default::default(),
            ..same.clone()
        })
        .into();
        first.store("same", &same)?;
        let loaded = first.load("same", &metadata).unwrap();
        let one = loaded.recovered.get().unwrap();
        assert_eq!(loaded.pixels.as_ptr(), one.pixels.as_ptr());
        assert_eq!((&loaded.pixels, &one.pixels), (&same.pixels, &same.pixels));
        Ok(())
    }
    /// A zero recovery gave another sign is a change, and a NaN it left as it was
    /// is none, whether the two images hold one buffer or a buffer each: the
    /// loaded recovered image is the stored one bit for bit.
    #[test]
    fn changes_are_of_bits() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().to_path_buf(), u64::MAX);
        let mut apart = image(40, 30, 0.2);
        apart.pixels[5] = [-0., 0.5, 0.25];
        apart.pixels[6] = [f32::NAN, 0.5, 0.25];
        let mut recovered = apart.clone();
        recovered.recovered = Default::default();
        recovered.pixels[5] = [0., 0.5, 0.25];
        let bits = |image: &CameraImage| -> Vec<[u32; 3]> {
            let pixels = image.pixels.range(0..image.pixels.len());
            pixels.iter().map(|p| p.map(f32::to_bits)).collect()
        };
        let expected = (bits(&apart), bits(&recovered));
        apart.recovered = Arc::new(recovered).into();
        cache.store("apart", &apart)?;
        let one = cache.load("apart", &Metadata::default()).unwrap();
        assert_eq!(one.pixels.differing().collect::<Vec<_>>(), [5]);
        cache.store("one", &one)?;
        for key in ["apart", "one"] {
            let loaded = cache.load(key, &Metadata::default()).unwrap();
            let got = (bits(&loaded), bits(loaded.recovered.get().unwrap()));
            assert!(got == expected, "{key}");
        }
        Ok(())
    }
    /// An entry with more changed pixels than one task puts back loads as it was
    /// stored. Dense: changes next to each other, across the words of the mask and
    /// across the tasks' shares, and the image's last pixel. Sparse: none in the
    /// first words, and stretches without any, short and long, between changes.
    /// Both: a stretch without any where one task's share ends and the next begins.
    #[test]
    fn many_changes_load_as_they_were_stored() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().to_path_buf(), u64::MAX);
        let dense = |i: usize, n: usize| !i.is_multiple_of(3) || i == 3 || i == n - 1;
        let sparse = |i: usize, _: usize| {
            i >= 200
                && i.is_multiple_of(7)
                && !(30_000..31_000).contains(&i)
                && !(100_000..140_000).contains(&i)
        };
        type Changed = fn(usize, usize) -> bool;
        let cases: [(&str, u32, u32, Changed); 2] =
            [("dense", 301, 211, dense), ("sparse", 701, 400, sparse)];
        for (name, width, height, is_changed) in cases {
            let mut a = image(width, height, 0.2);
            let mut recovered = (**a.recovered.get().unwrap()).clone();
            // `image` changed pixel 3; here the case alone says which pixels changed.
            recovered.pixels[3] = a.pixels[3];
            let n = recovered.pixels.len();
            let mut changed: Vec<usize> = (0..n).filter(|i| is_changed(*i, n)).collect();
            // And none in the 3,000 pixels after the last change of the first
            // task's share, so whole words without a change lie between two shares.
            let share = changed[TAKE - 1];
            changed.retain(|i| !(share + 1..=share + 3_000).contains(i));
            assert!(changed[TAKE] / 64 > share / 64 + 40, "{name}");
            assert!(changed.len() > 2 * TAKE && !n.is_multiple_of(64), "{name}");
            for &i in &changed {
                recovered.pixels[i] = [9., i as f32, 0.5];
            }
            a.recovered = Arc::new(recovered).into();
            cache.store(name, &a)?;
            let loaded = cache.load(name, &Metadata::default()).unwrap();
            let one = loaded.recovered.get().unwrap();
            assert!(loaded.pixels.shares_buffer(&one.pixels), "{name}");
            assert!(
                loaded.pixels.differing().eq(changed.iter().copied()),
                "{name}"
            );
            assert!(loaded.pixels == a.pixels, "{name}: the image as decoded");
            assert!(
                one.pixels == a.recovered.get().unwrap().pixels,
                "{name}: the recovered image"
            );
        }
        Ok(())
    }
    /// An entry whose changed pixels are out of order, or outside the image, is a
    /// miss: the photo is decoded again.
    #[test]
    fn an_entry_with_changes_out_of_order_is_a_miss() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().to_path_buf(), u64::MAX);
        let mut a = image(40, 30, 0.2);
        let mut recovered = (**a.recovered.get().unwrap()).clone();
        recovered.pixels[700] = [7.; 3];
        a.recovered = Arc::new(recovered).into();
        cache.store("a", &a)?;
        let stored = fs::read(cache.file("a"))?;
        assert!(cache.load("a", &Metadata::default()).is_some());
        // The two changes (pixels 3 and 700) are the entry's last 32 bytes.
        let (rest, changes) = stored.split_at(stored.len() - 32);
        let swapped = [rest, &changes[16..], &changes[..16]].concat();
        fs::write(cache.file("a"), swapped)?;
        assert!(cache.load("a", &Metadata::default()).is_none());
        let repeated = [rest, &changes[..16], &changes[..16]].concat();
        fs::write(cache.file("a"), repeated)?;
        assert!(cache.load("a", &Metadata::default()).is_none());
        let mut outside = stored.clone();
        let at = outside.len() - 16;
        outside[at..at + 4].copy_from_slice(&1200u32.to_le_bytes());
        fs::write(cache.file("a"), outside)?;
        assert!(cache.load("a", &Metadata::default()).is_none());
        fs::write(cache.file("a"), stored)?;
        assert!(cache.load("a", &Metadata::default()).is_some());
        Ok(())
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
