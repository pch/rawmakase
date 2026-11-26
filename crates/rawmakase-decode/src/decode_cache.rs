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
use crate::storage::{
    zlib_compress_fast, zlib_compress_stored, zlib_decompress_exact, zlib_max_stored_len,
};
use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

const MAGIC: &[u8; 8] = b"RMKDEC\0\x02";
/// Pixels per independently compressed stripe (768 KiB raw RGB `f32`).
const STRIPE_PIXELS: usize = 65_536;
const MAX_STRIPE_PIXELS: u32 = 1 << 20;
/// Recovered-pixel delta entries per compressed stripe (1 MiB raw).
const STRIPE_CHANGES: usize = 65_536;
const MAX_STRIPE_CHANGES: u32 = 1 << 20;
const WRITE_BATCH_STRIPES: usize = 16;
const MAX_READ_BATCH_RAW_BYTES: usize = 24 << 20;
const MAX_PIXELS: u64 = 150_000_000;
const MAX_STRIPES: u64 = 65_536;
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
    stripe_pixels: u32,
    stripe_changes: u32,
    /// Pixels changed by highlight recovery, stored in stripes after the image stripes.
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
        let mut reader = std::io::BufReader::new(file);
        let mut magic = [0; 8];
        reader.read_exact(&mut magic)?;
        ensure!(&magic == MAGIC, "Not a decode cache entry");
        let mut len = [0; 4];
        reader.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        ensure!(len < 1 << 16, "Invalid decode cache header");
        let mut header = vec![0; len];
        reader.read_exact(&mut header)?;
        let header: Header = serde_json::from_slice(&header)?;
        ensure!(header.key == key, "Decode cache key collision");
        let n = u64::from(header.width) * u64::from(header.height);
        ensure!(
            n > 0 && n <= MAX_PIXELS && header.recovered <= n,
            "Invalid decode cache size"
        );
        let n_usize = usize::try_from(n).context("Invalid decode cache size")?;
        ensure!(
            header.stripe_pixels > 0 && header.stripe_pixels <= MAX_STRIPE_PIXELS,
            "Invalid decode cache stripe size"
        );
        ensure!(
            header.stripe_changes > 0 && header.stripe_changes <= MAX_STRIPE_CHANGES,
            "Invalid decode cache recovery stripe size"
        );
        let stripe_pixels = header.stripe_pixels as usize;
        let stripe_changes = header.stripe_changes as usize;
        let pixel_stripe_count = n.div_ceil(u64::from(header.stripe_pixels));
        let change_stripe_count = header.recovered.div_ceil(u64::from(header.stripe_changes));
        ensure!(
            pixel_stripe_count <= MAX_STRIPES && change_stripe_count <= MAX_STRIPES,
            "Too many decode cache stripes"
        );
        let table_start = 12 + len as u64;
        let total_stripes = pixel_stripe_count
            .checked_add(change_stripe_count)
            .context("Decode cache stripe count overflow")?;
        let table_bytes = total_stripes
            .checked_mul(4)
            .context("Decode cache table overflow")?;
        let stripes_start = table_start
            .checked_add(table_bytes)
            .context("Decode cache table overflow")?;
        ensure!(size >= stripes_start, "Decode cache entry is truncated");
        let mut table_buf = vec![0u8; table_bytes as usize];
        reader.read_exact(&mut table_buf)?;
        let (pixel_table, change_table) = table_buf.split_at((pixel_stripe_count as usize) * 4);
        let mut cursor = stripes_start;
        let pixel_stripes = parse_stripes(
            pixel_table,
            pixel_stripe_count as usize,
            n_usize,
            stripe_pixels,
            12,
            &mut cursor,
        )?;
        let recovered_count =
            usize::try_from(header.recovered).context("Decode cache recovery count overflow")?;
        let change_stripes = parse_stripes(
            change_table,
            change_stripe_count as usize,
            recovered_count,
            stripe_changes,
            16,
            &mut cursor,
        )?;
        ensure!(size == cursor, "Decode cache entry has the wrong size");
        let file = reader.into_inner();
        let mut pixels = Vec::new();
        let pixel_batch = (MAX_READ_BATCH_RAW_BYTES / (stripe_pixels * 12)).max(1);
        for batch in pixel_stripes.chunks(pixel_batch) {
            let decoded_batch: Vec<Vec<[f32; 3]>> = batch
                .par_iter()
                .map(|&(offset, comp_len, raw_pixels)| -> Result<Vec<[f32; 3]>> {
                    let mut compressed = vec![0u8; comp_len];
                    read_exact_at_file(&file, offset, &mut compressed)?;
                    let mut raw = vec![0u8; raw_pixels * 12];
                    zlib_decompress_exact(&compressed, &mut raw)?;
                    let mut out = vec![[0f32; 3]; raw_pixels];
                    unshuffle_pixels(&raw, &mut out)?;
                    Ok(out)
                })
                .collect::<Result<Vec<_>>>()?;
            let batch_pixels: usize = decoded_batch.iter().map(Vec::len).sum();
            try_grow_bounded(&mut pixels, batch_pixels, n_usize)?;
            for stripe in decoded_batch {
                pixels.extend_from_slice(&stripe);
            }
        }
        ensure!(pixels.len() == n_usize, "Decoded pixel length mismatch");
        // Like a decode of the file, the loaded image measures its own Auto CA
        // rather than sharing one measured on the image `metadata` came from.
        let metadata = Metadata {
            lateral_ca: Default::default(),
            ..metadata.clone()
        };
        // The entry's pixels become the recovered image's, and the decode keeps the
        // values the changed ones had: the two images hold one buffer between them.
        let (pixels, recovered) = if change_stripes.is_empty() {
            let mut shared = Pixels::from(pixels);
            shared.share();
            (shared.clone(), shared)
        } else {
            let (changed, had) = apply_change_stripes(
                &file,
                &mut pixels,
                &change_stripes,
                stripe_changes,
                MAX_READ_BATCH_RAW_BYTES,
                || {
                    let mut changed = Vec::new();
                    changed
                        .try_reserve_exact(n_usize.div_ceil(64))
                        .context("Decode cache recovery mask allocation failed")?;
                    changed.resize(n_usize.div_ceil(64), 0);
                    let mut had = Vec::new();
                    had.try_reserve_exact(recovered_count)
                        .context("Decode cache recovery allocation failed")?;
                    Ok((changed, had))
                },
            )?;
            Pixels::pair_masked(pixels, changed, had).context("Invalid recovered pixel")?
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
        let n = u64::from(image.width) * u64::from(image.height);
        ensure!(
            n > 0 && n <= MAX_PIXELS && image.pixels.len() as u64 == n,
            "Invalid decode cache size"
        );
        // Pass 1: count recovery changes without allocating a full delta buffer.
        let recovered_count = count_changes(&image.pixels, &recovered.pixels);
        let header = serde_json::to_vec(&Header {
            key: key.into(),
            width: image.width,
            height: image.height,
            fast: image.fast,
            scale_factor: image.scale_factor,
            scale_clipped: image.scale_clipped,
            stripe_pixels: STRIPE_PIXELS as u32,
            stripe_changes: STRIPE_CHANGES as u32,
            recovered: recovered_count as u64,
        })?;
        ensure!(header.len() < 1 << 16, "Invalid decode cache header");
        let pixel_stripe_count = (n as usize).div_ceil(STRIPE_PIXELS);
        let change_stripe_count = recovered_count.div_ceil(STRIPE_CHANGES);
        ensure!(
            pixel_stripe_count as u64 <= MAX_STRIPES && change_stripe_count as u64 <= MAX_STRIPES,
            "Too many decode cache stripes"
        );
        let total_stripes = pixel_stripe_count + change_stripe_count;
        let table_start = 12 + header.len() as u64;
        let dir = self.location();
        dir.create()?;
        let mut file = tempfile::NamedTempFile::new_in(&dir.path)?;
        let mut stripe_lengths = Vec::new();
        stripe_lengths
            .try_reserve_exact(total_stripes)
            .context("Decode cache table allocation failed")?;
        {
            let mut out = std::io::BufWriter::new(file.as_file_mut());
            out.write_all(MAGIC)?;
            out.write_all(&(header.len() as u32).to_le_bytes())?;
            out.write_all(&header)?;
            out.write_all(&vec![0u8; total_stripes * 4])?;

            // A batch at a time: a decode kept as its recovered image's buffer
            // is never copied out whole.
            let total = image.pixels.len();
            let write_batch = STRIPE_PIXELS * WRITE_BATCH_STRIPES;
            for start in (0..total).step_by(write_batch) {
                let piece = image.pixels.range(start..(start + write_batch).min(total));
                let compressed_batch: Vec<Vec<u8>> = piece
                    .par_chunks(STRIPE_PIXELS)
                    .map(|stripe| compress_block(&shuffle_pixels(stripe)))
                    .collect::<Result<Vec<_>>>()?;
                for stripe in compressed_batch {
                    let len =
                        u32::try_from(stripe.len()).context("Compressed stripe exceeds u32")?;
                    stripe_lengths.push(len);
                    out.write_all(&stripe)?;
                }
            }
            // Pass 2: stream recovery deltas in bounded batches so dense 150 MP
            // images never allocate a full `Vec<Change>` upfront.
            collect_and_write_change_batches(
                &image.pixels,
                &recovered.pixels,
                recovered_count,
                STRIPE_CHANGES,
                WRITE_BATCH_STRIPES,
                |batch| {
                    let compressed_batch: Vec<Vec<u8>> = batch
                        .par_chunks(STRIPE_CHANGES)
                        .map(|chunk| compress_block(&shuffle_changes(chunk)))
                        .collect::<Result<Vec<_>>>()?;
                    for chunk in compressed_batch {
                        let len = u32::try_from(chunk.len())
                            .context("Compressed change stripe exceeds u32")?;
                        stripe_lengths.push(len);
                        out.write_all(&chunk)?;
                    }
                    Ok(())
                },
            )?;
            out.flush()?;
        }
        ensure!(
            stripe_lengths.len() == total_stripes,
            "Decode cache stripe count mismatch"
        );
        let mut table_bytes = Vec::with_capacity(total_stripes * 4);
        for len in stripe_lengths {
            table_bytes.extend_from_slice(&len.to_le_bytes());
        }
        file.as_file_mut().seek(SeekFrom::Start(table_start))?;
        file.as_file_mut().write_all(&table_bytes)?;
        file.as_file_mut().flush()?;
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
fn count_changes(pixels: &Pixels, recovered: &Pixels) -> usize {
    if pixels.shares_buffer(recovered) {
        return pixels.differing().count();
    }
    const CHUNK: usize = 1 << 20;
    let total = pixels.len();
    (0..total.div_ceil(CHUNK))
        .into_par_iter()
        .map(|chunk_idx| {
            let start = chunk_idx * CHUNK;
            let end = (start + CHUNK).min(total);
            let a = pixels.range(start..end);
            let b = recovered.range(start..end);
            a.iter()
                .zip(b.iter())
                .filter(|(pa, pb)| pa.map(f32::to_bits) != pb.map(f32::to_bits))
                .count()
        })
        .sum()
}

fn collect_and_write_change_batches(
    pixels: &Pixels,
    recovered: &Pixels,
    recovered_count: usize,
    stripe_changes: usize,
    batch_stripes: usize,
    mut on_batch: impl FnMut(&[Change]) -> Result<()>,
) -> Result<usize> {
    if recovered_count == 0 {
        return Ok(0);
    }
    let batch_cap = stripe_changes
        .checked_mul(batch_stripes)
        .context("Decode cache change batch overflow")?;
    ensure!(batch_cap > 0, "Invalid decode cache change batch");
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(batch_cap.min(recovered_count))
        .context("Decode cache recovery staging allocation failed")?;
    let mut emitted = 0usize;
    if pixels.shares_buffer(recovered) {
        for i in pixels.differing() {
            let index = u32::try_from(i).context("Decode cache pixel index exceeds u32")?;
            pending.push(Change {
                index,
                value: recovered[i],
            });
            if pending.len() == batch_cap {
                on_batch(&pending)?;
                emitted += pending.len();
                pending.clear();
            }
        }
    } else {
        let total = pixels.len();
        let sub_chunk = stripe_changes.max(1);
        for w_start in (0..total).step_by(batch_cap) {
            let w_end = (w_start + batch_cap).min(total);
            let sub_count = (w_end - w_start).div_ceil(sub_chunk);
            let window_chunks: Vec<Vec<Change>> = (0..sub_count)
                .into_par_iter()
                .map(|k| -> Result<Vec<Change>> {
                    let s_start = w_start + k * sub_chunk;
                    let s_end = (s_start + sub_chunk).min(w_end);
                    let a = pixels.range(s_start..s_end);
                    let b = recovered.range(s_start..s_end);
                    let mut local = Vec::new();
                    for (offset, (pa, pb)) in a.iter().zip(b.iter()).enumerate() {
                        if pa.map(f32::to_bits) != pb.map(f32::to_bits) {
                            let index = u32::try_from(s_start + offset)
                                .context("Decode cache pixel index exceeds u32")?;
                            local.push(Change { index, value: *pb });
                        }
                    }
                    Ok(local)
                })
                .collect::<Result<Vec<_>>>()?;
            for local in window_chunks {
                let mut slice = local.as_slice();
                while !slice.is_empty() {
                    let space = batch_cap - pending.len();
                    let take = space.min(slice.len());
                    pending.extend_from_slice(&slice[..take]);
                    slice = &slice[take..];
                    if pending.len() == batch_cap {
                        on_batch(&pending)?;
                        emitted += pending.len();
                        pending.clear();
                    }
                }
            }
        }
    }
    if !pending.is_empty() {
        on_batch(&pending)?;
        emitted += pending.len();
        pending.clear();
    }
    ensure!(
        emitted == recovered_count,
        "Decode cache recovery count mismatch"
    );
    Ok(emitted)
}

fn max_compressed_len(raw_bytes: u64) -> u64 {
    raw_bytes
        .saturating_add(raw_bytes / 64)
        .saturating_add(128)
        .max(zlib_max_stored_len(raw_bytes))
}

fn min_compressed_len(raw_bytes: u64) -> u64 {
    (raw_bytes / 1040).saturating_add(8)
}

fn compress_block(raw: &[u8]) -> Result<Vec<u8>> {
    let raw_len = raw.len() as u64;
    let cap = max_compressed_len(raw_len);
    let compressed = zlib_compress_fast(raw)?;
    if (compressed.len() as u64) <= cap {
        return Ok(compressed);
    }
    let stored = zlib_compress_stored(raw)?;
    ensure!(
        (stored.len() as u64) <= cap && (stored.len() as u64) >= min_compressed_len(raw_len),
        "Compressed decode cache block outside reader bound"
    );
    Ok(stored)
}

type StripeSpan = (u64, usize, usize);

fn parse_stripes(
    table: &[u8],
    count: usize,
    total_items: usize,
    max_per_stripe: usize,
    bytes_per_item: u64,
    cursor: &mut u64,
) -> Result<Vec<StripeSpan>> {
    let mut stripes = Vec::with_capacity(count);
    let mut remaining = total_items;
    ensure!(table.len() == count * 4, "Decode cache table size mismatch");
    for &chunk in table.as_chunks::<4>().0 {
        let comp_len = u32::from_le_bytes(chunk) as u64;
        let raw_items = remaining.min(max_per_stripe);
        remaining -= raw_items;
        let raw_bytes = (raw_items as u64) * bytes_per_item;
        ensure!(
            raw_items > 0
                && comp_len >= min_compressed_len(raw_bytes)
                && comp_len <= max_compressed_len(raw_bytes),
            "Invalid decode cache stripe length"
        );
        let next = cursor
            .checked_add(comp_len)
            .context("Decode cache stripe offset overflow")?;
        stripes.push((*cursor, comp_len as usize, raw_items));
        *cursor = next;
    }
    ensure!(remaining == 0, "Decode cache stripe count mismatch");
    Ok(stripes)
}

fn try_grow_bounded<T>(vec: &mut Vec<T>, needed_additional: usize, max_cap: usize) -> Result<()> {
    let required = vec
        .len()
        .checked_add(needed_additional)
        .context("Decode cache allocation overflow")?;
    ensure!(required <= max_cap, "Decode cache decoded past image size");
    if vec.capacity() >= required {
        return Ok(());
    }
    let target = vec.capacity().saturating_mul(2).max(required).min(max_cap);
    vec.try_reserve_exact(target - vec.len())
        .context("Decode cache pixel allocation failed")?;
    Ok(())
}

fn decode_change_batch(
    file: &fs::File,
    batch: &[StripeSpan],
    pixel_len: usize,
    last_index: &mut Option<u32>,
) -> Result<Vec<Vec<Change>>> {
    let decoded_batch = batch
        .par_iter()
        .map(|&(offset, comp_len, count)| -> Result<Vec<Change>> {
            let mut compressed = vec![0u8; comp_len];
            read_exact_at_file(file, offset, &mut compressed)?;
            let mut raw = vec![0u8; count * 16];
            zlib_decompress_exact(&compressed, &mut raw)?;
            unshuffle_changes(&raw, count)
        })
        .collect::<Result<Vec<_>>>()?;
    for changes in &decoded_batch {
        for change in changes {
            ensure!(
                (change.index as usize) < pixel_len,
                "Invalid recovered pixel"
            );
            if let Some(prev) = *last_index {
                ensure!(change.index > prev, "Invalid recovered pixel");
            }
            *last_index = Some(change.index);
        }
    }
    Ok(decoded_batch)
}

fn apply_change_stripes(
    file: &fs::File,
    pixels: &mut [[f32; 3]],
    change_stripes: &[StripeSpan],
    stripe_changes: usize,
    max_batch_raw_bytes: usize,
    alloc_recovery: impl FnOnce() -> Result<(Vec<u64>, Vec<[f32; 3]>)>,
) -> Result<(Vec<u64>, Vec<[f32; 3]>)> {
    let pixel_len = pixels.len();
    let change_batch = (max_batch_raw_bytes / (stripe_changes * 16)).max(1);
    let (first_batch, rest) = change_stripes.split_at(change_stripes.len().min(change_batch));
    let mut last_index = None;
    let first_decoded = decode_change_batch(file, first_batch, pixel_len, &mut last_index)?;
    let first_last_index = last_index;
    // Validate every remaining delta batch in bounded scratch before allocating
    // the recovery mask and original-pixel storage, so corrupt, out-of-range,
    // or out-of-order trailing batches fail without a gigabyte-scale allocation.
    for batch in rest.chunks(change_batch) {
        let _ = decode_change_batch(file, batch, pixel_len, &mut last_index)?;
    }
    let (mut changed, mut had) = alloc_recovery()?;
    ensure!(
        changed.len() == pixel_len.div_ceil(64) && had.is_empty(),
        "Recovery buffer size mismatch"
    );
    let mut apply_batch = |batch_changes: Vec<Vec<Change>>| -> Result<()> {
        for changes in batch_changes {
            for change in changes {
                let idx = change.index as usize;
                let word = changed
                    .get_mut(idx / 64)
                    .context("Recovery mask size mismatch")?;
                *word |= 1u64 << (idx % 64);
                let prev = std::mem::replace(
                    pixels.get_mut(idx).context("Invalid recovered pixel")?,
                    change.value,
                );
                had.push(prev);
            }
        }
        Ok(())
    };
    apply_batch(first_decoded)?;
    let mut apply_last = first_last_index;
    for batch in rest.chunks(change_batch) {
        let decoded = decode_change_batch(file, batch, pixel_len, &mut apply_last)?;
        apply_batch(decoded)?;
    }
    let expected_changes: usize = change_stripes.iter().map(|&(_, _, count)| count).sum();
    ensure!(had.len() == expected_changes, "Recovery count mismatch");
    Ok((changed, had))
}

/// Transposes each channel's 4 little-endian `f32` bytes into contiguous byte
/// planes across a stripe so exponent and sign bytes group for deflate.
fn shuffle_pixels(pixels: &[[f32; 3]]) -> Vec<u8> {
    let n = pixels.len();
    let mut out = vec![0u8; n * 12];
    for (i, px) in pixels.iter().enumerate() {
        for (c, &sample) in px.iter().enumerate() {
            let bytes = sample.to_bits().to_le_bytes();
            let plane = c * 4 * n + i;
            out[plane] = bytes[0];
            out[plane + n] = bytes[1];
            out[plane + 2 * n] = bytes[2];
            out[plane + 3 * n] = bytes[3];
        }
    }
    out
}

fn unshuffle_pixels(shuffled: &[u8], out: &mut [[f32; 3]]) -> Result<()> {
    let n = out.len();
    ensure!(shuffled.len() == n * 12, "Unexpected stripe payload length");
    for (i, px) in out.iter_mut().enumerate() {
        for (c, sample) in px.iter_mut().enumerate() {
            let plane = c * 4 * n + i;
            let bytes = [
                shuffled[plane],
                shuffled[plane + n],
                shuffled[plane + 2 * n],
                shuffled[plane + 3 * n],
            ];
            *sample = f32::from_bits(u32::from_le_bytes(bytes));
        }
    }
    Ok(())
}

fn shuffle_changes(changes: &[Change]) -> Vec<u8> {
    let n = changes.len();
    let mut out = vec![0u8; n * 16];
    for (i, change) in changes.iter().enumerate() {
        let idx = change.index.to_le_bytes();
        out[i] = idx[0];
        out[n + i] = idx[1];
        out[2 * n + i] = idx[2];
        out[3 * n + i] = idx[3];
        for (c, &sample) in change.value.iter().enumerate() {
            let bytes = sample.to_bits().to_le_bytes();
            let plane = (c + 1) * 4 * n + i;
            out[plane] = bytes[0];
            out[plane + n] = bytes[1];
            out[plane + 2 * n] = bytes[2];
            out[plane + 3 * n] = bytes[3];
        }
    }
    out
}

fn unshuffle_changes(shuffled: &[u8], count: usize) -> Result<Vec<Change>> {
    ensure!(
        shuffled.len() == count * 16,
        "Unexpected recovered block length"
    );
    let mut changes = Vec::with_capacity(count);
    for i in 0..count {
        let index = u32::from_le_bytes([
            shuffled[i],
            shuffled[count + i],
            shuffled[2 * count + i],
            shuffled[3 * count + i],
        ]);
        let mut value = [0f32; 3];
        for (c, sample) in value.iter_mut().enumerate() {
            let plane = (c + 1) * 4 * count + i;
            let bytes = [
                shuffled[plane],
                shuffled[plane + count],
                shuffled[plane + 2 * count],
                shuffled[plane + 3 * count],
            ];
            *sample = f32::from_bits(u32::from_le_bytes(bytes));
        }
        changes.push(Change { index, value });
    }
    Ok(changes)
}

fn read_exact_at_file(file: &fs::File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        let mut done = 0;
        while done < buf.len() {
            let n = std::os::windows::fs::FileExt::seek_read(
                file,
                &mut buf[done..],
                offset + done as u64,
            )?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
        Ok(())
    }
}

/// Fills `buf` from `offset`, in parallel chunks: one reader leaves most of an SSD's
/// bandwidth unused on a 700 MB image.
pub(crate) fn read_at(file: &fs::File, offset: u64, buf: &mut [u8]) -> Result<()> {
    const CHUNK: usize = 16 << 20;
    buf.par_chunks_mut(CHUNK)
        .enumerate()
        .try_for_each(|(i, chunk)| -> std::io::Result<()> {
            read_exact_at_file(file, offset + (i * CHUNK) as u64, chunk)
        })?;
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct Change {
    index: u32,
    value: [f32; 3],
}

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
        const TAKE: usize = 1 << 14;
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

        // 40x30 has 1 pixel stripe and 1 change stripe (2 x 4-byte table entries).
        let header_len = u32::from_le_bytes(stored[8..12].try_into().unwrap()) as usize;
        let table_start = 12 + header_len;
        let pixel_stripe_len =
            u32::from_le_bytes(stored[table_start..table_start + 4].try_into().unwrap()) as usize;
        let change_table_at = table_start + 4;
        let prefix = &stored[..table_start + 8 + pixel_stripe_len];
        let write_with_changes = |changes: &[Change]| -> Result<()> {
            let block = compress_block(&shuffle_changes(changes))?;
            let mut out = prefix.to_vec();
            out[change_table_at..change_table_at + 4]
                .copy_from_slice(&(block.len() as u32).to_le_bytes());
            out.extend_from_slice(&block);
            fs::write(cache.file("a"), out)?;
            Ok(())
        };

        let c3 = Change {
            index: 3,
            value: [2., 3., 4.],
        };
        let c700 = Change {
            index: 700,
            value: [7.; 3],
        };
        write_with_changes(&[c700, c3])?;
        assert!(cache.load("a", &Metadata::default()).is_none());
        write_with_changes(&[c3, c3])?;
        assert!(cache.load("a", &Metadata::default()).is_none());
        write_with_changes(&[
            c3,
            Change {
                index: 1200,
                value: [7.; 3],
            },
        ])?;
        assert!(cache.load("a", &Metadata::default()).is_none());
        fs::write(cache.file("a"), stored)?;
        assert!(cache.load("a", &Metadata::default()).is_some());
        Ok(())
    }
    #[test]
    fn entries_round_trip_and_stay_within_the_cap() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let a = image(40, 30, 0.2);
        let b = image(40, 30, 0.4);
        let probe = DecodeCache::new(dir.path().join("probe"), u64::MAX);
        probe.store("a", &a)?;
        probe.store("b", &b)?;
        let size_a = fs::metadata(probe.file("a"))?.len();
        let size_b = fs::metadata(probe.file("b"))?.len();
        let cache = DecodeCache::new(
            dir.path().join("decoded"),
            size_a + size_b + (size_a.min(size_b) / 2),
        );
        let metadata = Metadata {
            make: "Test".into(),
            ..Default::default()
        };
        assert!(cache.load("a", &metadata).is_none());
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
        cache.store("b", &b)?;
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(cache.load("a", &metadata).is_some());
        std::thread::sleep(std::time::Duration::from_millis(20));
        // The least recently used entry goes first.
        cache.store("c", &image(40, 30, 0.6))?;
        assert!(cache.contains("a") && cache.contains("c") && !cache.contains("b"));
        Ok(())
    }

    #[test]
    fn bit_exact_f32_multi_stripe_roundtrip_with_recovered_diffs() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("decoded"), u64::MAX);
        // 300x300 spans two 65,536-pixel stripes and mixes special f32 bit patterns.
        let width = 300u32;
        let height = 300u32;
        let total = (width * height) as usize;
        let specials = [
            [0.0f32, -0.0, f32::from_bits(1)],
            [f32::MIN_POSITIVE, -1.25, 16.5],
            [f32::MAX, f32::INFINITY, f32::NEG_INFINITY],
        ];
        let pixels: Vec<[f32; 3]> = (0..total)
            .map(|i| {
                if i < specials.len() {
                    specials[i]
                } else {
                    let t = i as f32 / total as f32;
                    [t, (t * 7.0).sin() * 0.5 + 0.5, t * t]
                }
            })
            .collect();
        let im = CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: pixels.into(),
            metadata: Metadata::default(),
            fast: true,
            scale_factor: 2.25,
            scale_clipped: 12,
            hdr: Default::default(),
        };
        let mut recovered = im.clone();
        // Flip -0.0 -> +0.0 on pixel 0 and touch indices in both stripes.
        recovered.pixels[0] = [0.0, 0.0, f32::from_bits(1)];
        recovered.pixels[5] = [1.5, 2.5, 3.5];
        recovered.pixels[STRIPE_PIXELS + 123] = [4.25, 0.0, -0.5];
        recovered.pixels[total - 1] = [9.0, 8.0, 7.0];
        let _ = im.recovered.set(Arc::new(recovered));

        cache.store("multi", &im)?;
        let loaded = cache
            .load("multi", &Metadata::default())
            .context("Cache miss")?;
        let orig_rec = im.recovered.get().unwrap();
        let load_rec = loaded.recovered.get().unwrap();
        assert!(loaded.pixels.shares_buffer(&load_rec.pixels));
        for (a, b) in loaded
            .pixels
            .range(0..loaded.pixels.len())
            .iter()
            .zip(im.pixels.range(0..im.pixels.len()).iter())
        {
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }
        for (a, b) in load_rec.pixels.iter().zip(orig_rec.pixels.iter()) {
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }
        Ok(())
    }

    #[test]
    fn truncated_and_corrupted_stripes_are_misses() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("decoded"), u64::MAX);
        let im = image(300, 300, 0.3);
        cache.store("k", &im)?;
        let raw = fs::read(cache.file("k"))?;
        let metadata = Metadata::default();
        assert!(cache.load("k", &metadata).is_some());

        let header_len = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
        let table_start = 12 + header_len;
        let stripe0_len =
            u32::from_le_bytes(raw[table_start..table_start + 4].try_into().unwrap()) as usize;
        // 300x300 has 2 pixel stripes plus 1 recovered-delta stripe (3 x 4 bytes).
        let stripe0_start = table_start + 12;

        // Truncate inside the first stripe.
        fs::write(cache.file("k"), &raw[..stripe0_start + stripe0_len / 2])?;
        assert!(cache.load("k", &metadata).is_none());

        // Corrupt a payload byte inside the first stripe.
        let mut bad_stripe = raw.clone();
        bad_stripe[stripe0_start + stripe0_len / 2] ^= 0xff;
        fs::write(cache.file("k"), &bad_stripe)?;
        assert!(cache.load("k", &metadata).is_none());

        // Corrupt a payload byte inside the recovered-changes block.
        let mut bad_recovered = raw.clone();
        let tail = bad_recovered.len() - 2;
        bad_recovered[tail] ^= 0xff;
        fs::write(cache.file("k"), &bad_recovered)?;
        assert!(cache.load("k", &metadata).is_none());

        // Corrupt stripe length in the index table.
        let mut bad_table = raw;
        bad_table[table_start] ^= 0x01;
        fs::write(cache.file("k"), &bad_table)?;
        assert!(cache.load("k", &metadata).is_none());
        Ok(())
    }

    #[test]
    fn v1_magic_entry_is_treated_as_miss() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("decoded"), u64::MAX);
        fs::create_dir_all(cache.dir())?;
        let mut legacy = Vec::from(*b"RMKDEC\0\x01");
        let header = serde_json::to_vec(&serde_json::json!({
            "key": "legacy",
            "width": 2,
            "height": 2,
            "fast": false,
            "scale_factor": 1.0,
            "scale_clipped": 0,
            "recovered": 0
        }))?;
        legacy.extend_from_slice(&(header.len() as u32).to_le_bytes());
        legacy.extend_from_slice(&header);
        legacy.extend_from_slice(&[0u8; 2 * 2 * 12]);
        fs::write(cache.file("legacy"), &legacy)?;
        assert!(cache.load("legacy", &Metadata::default()).is_none());
        Ok(())
    }

    #[test]
    fn smooth_ramp_compresses_substantially() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("decoded"), u64::MAX);
        let width = 512u32;
        let height = 512u32;
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let r = x as f32 / (width - 1) as f32;
                let g = y as f32 / (height - 1) as f32;
                let b = (r + g) * 0.5;
                pixels.push([r, g, b]);
            }
        }
        let im = CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: pixels.into(),
            metadata: Metadata::default(),
            fast: false,
            scale_factor: 1.0,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let _ = im.recovered.set(Arc::new(im.clone()));
        cache.store("ramp", &im)?;
        let on_disk = fs::metadata(cache.file("ramp"))?.len();
        let raw_bytes = (width as u64) * (height as u64) * 12;
        assert!(
            on_disk < raw_bytes / 3,
            "expected smooth ramp ({on_disk} B) to compress below 1/3 of raw ({raw_bytes} B)"
        );
        let loaded = cache
            .load("ramp", &Metadata::default())
            .context("Cache miss")?;
        for (a, b) in loaded.pixels.iter().zip(im.pixels.iter()) {
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }
        Ok(())
    }

    #[test]
    fn corrupt_giant_header_entry_is_rejected_without_allocating_image() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("decoded"), u64::MAX);
        fs::create_dir_all(cache.dir())?;
        let width = 15_000u32;
        let height = 10_000u32;
        let n = u64::from(width) * u64::from(height);
        let stripe_pixels = STRIPE_PIXELS as u32;
        let stripe_changes = STRIPE_CHANGES as u32;
        let pixel_stripes = n.div_ceil(u64::from(stripe_pixels)) as usize;
        let change_stripes = n.div_ceil(u64::from(stripe_changes)) as usize;
        let header = serde_json::to_vec(&Header {
            key: "giant".into(),
            width,
            height,
            fast: false,
            scale_factor: 1.0,
            scale_clipped: 0,
            stripe_pixels,
            stripe_changes,
            recovered: n,
        })?;

        // Case A: 1-byte claimed stripe length must fail during table validation.
        let mut tiny = Vec::from(*MAGIC);
        tiny.extend_from_slice(&(header.len() as u32).to_le_bytes());
        tiny.extend_from_slice(&header);
        for _ in 0..(pixel_stripes + change_stripes) {
            tiny.extend_from_slice(&1u32.to_le_bytes());
        }
        tiny.resize(tiny.len() + pixel_stripes + change_stripes, 0xaa);
        fs::write(cache.file("giant"), &tiny)?;
        assert!(cache.load("giant", &Metadata::default()).is_none());

        // Case B: minimum-sized garbage stripes matching file length must fail on stripe 0 inflate.
        let mut min_garbage = Vec::from(*MAGIC);
        min_garbage.extend_from_slice(&(header.len() as u32).to_le_bytes());
        min_garbage.extend_from_slice(&header);
        let mut payload_len = 0usize;
        for idx in 0..pixel_stripes {
            let items = if idx + 1 == pixel_stripes {
                n - (idx as u64) * u64::from(stripe_pixels)
            } else {
                u64::from(stripe_pixels)
            };
            let len = min_compressed_len(items * 12) as u32;
            min_garbage.extend_from_slice(&len.to_le_bytes());
            payload_len += len as usize;
        }
        for idx in 0..change_stripes {
            let items = if idx + 1 == change_stripes {
                n - (idx as u64) * u64::from(stripe_changes)
            } else {
                u64::from(stripe_changes)
            };
            let len = min_compressed_len(items * 16) as u32;
            min_garbage.extend_from_slice(&len.to_le_bytes());
            payload_len += len as usize;
        }
        min_garbage.resize(min_garbage.len() + payload_len, 0x55);
        fs::write(cache.file("giant"), &min_garbage)?;
        assert!(cache.load("giant", &Metadata::default()).is_none());
        Ok(())
    }

    #[test]
    fn multi_stripe_recovered_deltas_roundtrip_bit_exactly() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("decoded"), u64::MAX);
        let width = 300u32;
        let height = 300u32;
        let total = (width * height) as usize;
        let im = CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: vec![[-0.0, 0.25, 0.5]; total].into(),
            metadata: Metadata::default(),
            fast: false,
            scale_factor: 1.0,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let mut recovered = im.clone();
        // Modify 70,000 pixels to span two STRIPE_CHANGES (65,536) blocks.
        for (i, px) in recovered.pixels[..70_000].iter_mut().enumerate() {
            *px = [0.0, f32::from_bits((i as u32) | 1), 1.0 + i as f32 * 0.001];
        }
        let _ = im.recovered.set(Arc::new(recovered));

        cache.store("multi_delta", &im)?;
        let loaded = cache
            .load("multi_delta", &Metadata::default())
            .context("Cache miss")?;
        let orig_rec = im.recovered.get().unwrap();
        let load_rec = loaded.recovered.get().unwrap();
        assert!(loaded.pixels.shares_buffer(&load_rec.pixels));
        for (a, b) in load_rec.pixels.iter().zip(orig_rec.pixels.iter()) {
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }
        Ok(())
    }

    #[test]
    fn incompressible_stripe_stays_within_reader_cap_and_roundtrips() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("decoded"), u64::MAX);
        let width = 256u32;
        let height = 256u32;
        let total = (width * height) as usize;
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next_u32 = || {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            z as u32
        };
        let pixels: Vec<[f32; 3]> = (0..total)
            .map(|_| {
                [
                    f32::from_bits(next_u32()),
                    f32::from_bits(next_u32()),
                    f32::from_bits(next_u32()),
                ]
            })
            .collect();
        let im = CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: pixels.into(),
            metadata: Metadata::default(),
            fast: false,
            scale_factor: 1.0,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let _ = im.recovered.set(Arc::new(im.clone()));

        cache.store("random", &im)?;
        let raw = fs::read(cache.file("random"))?;
        let header_len = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as usize;
        let stripe0_len = u32::from_le_bytes(
            raw[12 + header_len..12 + header_len + 4]
                .try_into()
                .unwrap(),
        ) as u64;
        assert!(stripe0_len <= max_compressed_len((total as u64) * 12));

        let loaded = cache
            .load("random", &Metadata::default())
            .context("Cache miss")?;
        for (a, b) in loaded.pixels.iter().zip(im.pixels.iter()) {
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }
        Ok(())
    }

    #[test]
    fn bounded_pixel_growth_caps_capacity_at_validated_size() -> Result<()> {
        let max_cap = 150usize;
        let mut buf: Vec<[f32; 3]> = Vec::new();
        for i in 0..8usize {
            let chunk_len = if i == 7 { 10 } else { 20 };
            try_grow_bounded(&mut buf, chunk_len, max_cap)?;
            assert!(
                buf.capacity() <= max_cap,
                "capacity {} exceeded validated max {max_cap}",
                buf.capacity()
            );
            buf.resize(buf.len() + chunk_len, [i as f32; 3]);
        }
        assert_eq!(buf.len(), max_cap);
        assert_eq!(buf.capacity(), max_cap);
        assert!(try_grow_bounded(&mut buf, 1, max_cap).is_err());
        Ok(())
    }

    #[test]
    fn corrupt_or_oob_later_delta_batch_never_allocates_recovery() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let write_batches =
            |name: &str, batches: &[&[Change]]| -> Result<(fs::File, Vec<StripeSpan>)> {
                let path = dir.path().join(name);
                let mut blob = Vec::new();
                let mut stripes = Vec::new();
                for &changes in batches {
                    let compressed = compress_block(&shuffle_changes(changes))?;
                    stripes.push((blob.len() as u64, compressed.len(), changes.len()));
                    blob.extend_from_slice(&compressed);
                }
                fs::write(&path, &blob)?;
                Ok((fs::File::open(&path)?, stripes))
            };

        let batch0 = [
            Change {
                index: 1,
                value: [1.0, 2.0, 3.0],
            },
            Change {
                index: 5,
                value: [4.0, 5.0, 6.0],
            },
        ];
        let batch1_ok = [Change {
            index: 50,
            value: [7.0, 8.0, 9.0],
        }];

        // (a) Valid multi-batch deltas allocate recovery once and apply all changes.
        let (file_ok, stripes_ok) = write_batches("ok.bin", &[&batch0, &batch1_ok])?;
        let mut pixels_ok = vec![[0.0; 3]; 100];
        let mut allocs = 0usize;
        let (changed, had) =
            apply_change_stripes(&file_ok, &mut pixels_ok, &stripes_ok, 4, 4 * 16, || {
                allocs += 1;
                Ok((vec![0u64; 100usize.div_ceil(64)], Vec::with_capacity(3)))
            })?;
        assert_eq!(allocs, 1);
        assert_eq!(pixels_ok[1], [1.0, 2.0, 3.0]);
        assert_eq!(pixels_ok[5], [4.0, 5.0, 6.0]);
        assert_eq!(pixels_ok[50], [7.0, 8.0, 9.0]);
        assert_eq!(had, vec![[0.0; 3]; 3]);
        assert!(Pixels::pair_masked(pixels_ok, changed, had).is_some());

        // (b) Corrupt checksum in trailing batch must fail without calling alloc_recovery.
        let path_bad = dir.path().join("bad.bin");
        let mut raw_bad = fs::read(dir.path().join("ok.bin"))?;
        let last = raw_bad.len() - 2;
        raw_bad[last] ^= 0xff;
        fs::write(&path_bad, &raw_bad)?;
        let file_bad = fs::File::open(&path_bad)?;
        let mut pixels_bad = vec![[0.0; 3]; 100];
        let mut allocs_bad = 0usize;
        assert!(
            apply_change_stripes(&file_bad, &mut pixels_bad, &stripes_ok, 4, 4 * 16, || {
                allocs_bad += 1;
                Ok((vec![0u64; 100usize.div_ceil(64)], Vec::with_capacity(3)))
            })
            .is_err()
        );
        assert_eq!(allocs_bad, 0);
        assert!(pixels_bad.iter().all(|p| *p == [0.0; 3]));

        // (c) Out-of-range pixel index in trailing batch must fail without calling alloc_recovery.
        let batch1_oob = [Change {
            index: 100,
            value: [7.0, 8.0, 9.0],
        }];
        let (file_oob, stripes_oob) = write_batches("oob.bin", &[&batch0, &batch1_oob])?;
        let mut pixels_oob = vec![[0.0; 3]; 100];
        let mut allocs_oob = 0usize;
        assert!(
            apply_change_stripes(&file_oob, &mut pixels_oob, &stripes_oob, 4, 4 * 16, || {
                allocs_oob += 1;
                Ok((vec![0u64; 100usize.div_ceil(64)], Vec::with_capacity(3)))
            })
            .is_err()
        );
        assert_eq!(allocs_oob, 0);
        assert!(pixels_oob.iter().all(|p| *p == [0.0; 3]));

        // (d) Out-of-order or duplicate index across batches must fail without calling alloc_recovery.
        for (name, bad_idx) in [("dup.bin", 5u32), ("backwards.bin", 4u32)] {
            let batch1_bad_order = [Change {
                index: bad_idx,
                value: [7.0, 8.0, 9.0],
            }];
            let (file_order, stripes_order) = write_batches(name, &[&batch0, &batch1_bad_order])?;
            let mut pixels_order = vec![[0.0; 3]; 100];
            let mut allocs_order = 0usize;
            assert!(
                apply_change_stripes(
                    &file_order,
                    &mut pixels_order,
                    &stripes_order,
                    4,
                    4 * 16,
                    || {
                        allocs_order += 1;
                        Ok((vec![0u64; 100usize.div_ceil(64)], Vec::with_capacity(3)))
                    }
                )
                .is_err()
            );
            assert_eq!(allocs_order, 0);
            assert!(pixels_order.iter().all(|p| *p == [0.0; 3]));
        }
        Ok(())
    }

    #[test]
    fn bounded_change_batching_roundtrips_shared_and_separate_buffers() -> Result<()> {
        let total = 200usize;
        let mut base_vec: Vec<[f32; 3]> =
            (0..total).map(|i| [i as f32 * 0.25, -0.0, 1.0]).collect();
        base_vec[10] = [f32::NAN, 0.5, 0.25];
        let mut rec_vec = base_vec.clone();
        // 17 changed indices across the image, including -0.0 -> +0.0 on pixel 0.
        let changed_indices = [
            0usize, 1, 2, 3, 7, 15, 22, 31, 40, 55, 63, 64, 99, 120, 150, 180, 199,
        ];
        for &idx in &changed_indices {
            rec_vec[idx] = [0.0, idx as f32 + 0.5, 2.0];
        }
        let separate_base = Pixels::from(base_vec.clone());
        let separate_rec = Pixels::from(rec_vec.clone());
        let (shared_base, shared_rec) = Pixels::pair(
            rec_vec.clone(),
            changed_indices.iter().map(|&idx| (idx, base_vec[idx])),
        )
        .context("valid pair")?;
        assert!(shared_base.shares_buffer(&shared_rec));

        assert_eq!(count_changes(&separate_base, &separate_rec), 17);
        assert_eq!(count_changes(&shared_base, &shared_rec), 17);

        let collect_batches =
            |pixels: &Pixels, recovered: &Pixels| -> Result<(Vec<usize>, Vec<Change>)> {
                let count = count_changes(pixels, recovered);
                let mut batch_sizes = Vec::new();
                let mut all = Vec::new();
                let emitted =
                    collect_and_write_change_batches(pixels, recovered, count, 4, 2, |batch| {
                        assert!(batch.len() <= 8);
                        batch_sizes.push(batch.len());
                        all.extend_from_slice(batch);
                        Ok(())
                    })?;
                assert_eq!(emitted, count);
                Ok((batch_sizes, all))
            };

        let (sep_sizes, sep_changes) = collect_batches(&separate_base, &separate_rec)?;
        let (shr_sizes, shr_changes) = collect_batches(&shared_base, &shared_rec)?;
        assert_eq!(sep_sizes, vec![8, 8, 1]);
        assert_eq!(shr_sizes, vec![8, 8, 1]);
        assert_eq!(sep_changes.len(), shr_changes.len());
        for (a, b) in sep_changes.iter().zip(shr_changes.iter()) {
            assert_eq!(a.index, b.index);
            assert_eq!(a.value.map(f32::to_bits), b.value.map(f32::to_bits));
        }

        // Applying collected changes reconstructs both base and recovered bit-for-bit.
        let mut rebuilt_rec = base_vec;
        for ch in &shr_changes {
            rebuilt_rec[ch.index as usize] = ch.value;
        }
        for (a, b) in rebuilt_rec.iter().zip(rec_vec.iter()) {
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }

        // Also verify full DecodeCache::store + load round-trips bit-identically across
        // multiple change stripes (> STRIPE_CHANGES) for both shared and separate buffers.
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().join("decoded"), u64::MAX);
        let width = 300u32;
        let height = 300u32;
        let img_total = (width * height) as usize;
        let img_base: Vec<[f32; 3]> = vec![[-0.0, 0.25, 0.5]; img_total];
        let mut img_rec = img_base.clone();
        for (i, px) in img_rec.iter_mut().take(70_000).enumerate() {
            *px = [0.0, f32::from_bits((i as u32) | 1), 1.0 + i as f32 * 0.001];
        }
        let separate_img = CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: img_base.clone().into(),
            metadata: Metadata::default(),
            fast: false,
            scale_factor: 1.0,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let mut separate_rec_img = separate_img.clone();
        separate_rec_img.pixels = img_rec.clone().into();
        let _ = separate_img.recovered.set(Arc::new(separate_rec_img));

        let (paired_base, paired_rec) =
            Pixels::pair(img_rec, (0..70_000).map(|i| (i, img_base[i])))
                .context("valid image pair")?;
        let mut shared_img = separate_img.clone();
        shared_img.recovered = Default::default();
        shared_img.pixels = paired_base;
        let mut shared_rec_img = shared_img.clone();
        shared_rec_img.pixels = paired_rec;
        let _ = shared_img.recovered.set(Arc::new(shared_rec_img));

        cache.store("separate", &separate_img)?;
        cache.store("shared", &shared_img)?;
        let loaded_sep = cache
            .load("separate", &Metadata::default())
            .context("miss")?;
        let loaded_shr = cache.load("shared", &Metadata::default()).context("miss")?;
        assert!(
            loaded_shr
                .pixels
                .shares_buffer(&loaded_shr.recovered.get().unwrap().pixels)
        );
        for (a, b) in loaded_shr
            .pixels
            .range(0..img_total)
            .iter()
            .zip(loaded_sep.pixels.range(0..img_total).iter())
        {
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }
        for (a, b) in loaded_shr
            .recovered
            .get()
            .unwrap()
            .pixels
            .iter()
            .zip(loaded_sep.recovered.get().unwrap().pixels.iter())
        {
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }
        Ok(())
    }
}
