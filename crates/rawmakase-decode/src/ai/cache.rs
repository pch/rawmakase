//! Disk cache of AI Denoise results, apart from decoded images
//! (docs/rendering-quality.md#ai-denoise): its own subfolder of the default directory
//! and of a chosen folder, its own
//! file extension and its own size cap, so decodes never evict results that took a
//! minute, and relocating or purging one kind leaves the other alone. The size cap,
//! eviction, purge and relocation are the decode cache's code.
//!
//! An entry holds the model's output with its gain applied, in unbalanced camera RGB
//! (sensor white at 1), at half precision, with the packed layout and the clipped-sample
//! flags assembly needs. The model's raw output cannot be stored in half precision: its
//! scale is arbitrary (medians reach 10⁵, beyond half precision's 65,504). Every value is
//! checked finite when stored and when loaded. Saved edits never need the cache.
use crate::ai::denoise::Layout;
use crate::decode_cache::{Dir, Namespace, cache_dir, entry_in, purge, read_at, trim};
use anyhow::{Result, ensure};
use half::f16;
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

const MAGIC: &[u8; 8] = b"RMKAID\0\x01";
/// The payload's format; part of every entry's header.
const FORMAT: u32 = 1;
/// Default size cap: results take a minute each to compute.
pub const DEFAULT_LIMIT: u64 = 10 << 30;
/// AI Denoise results.
const AI: Namespace = Namespace {
    default: "ai-denoise",
    chosen: "RAWmakase AI Denoise",
    extension: "aidenoise",
};

/// One photo's AI Denoise result.
#[derive(Clone, Debug, PartialEq)]
pub struct AiEntry {
    pub layout: Layout,
    /// The gain applied, kept for reference.
    pub gain: f32,
    /// Red, green and blue planes of `2 × packed_width` by `2 × packed_height`, the gain
    /// applied (`crate::ai::denoise::ModelOutput::rgb`).
    pub rgb: Vec<f32>,
    /// `crate::ai::denoise::PackedBayer::clipped`.
    pub clipped: Vec<bool>,
}

#[derive(Serialize, Deserialize, PartialEq)]
struct Header {
    format: u32,
    key: String,
    width: usize,
    height: usize,
    pad: [usize; 2],
    gain: f32,
}

/// A result with values a half-precision float cannot hold: it is not a result, and
/// the job that made it fails rather than renders or caches it.
#[derive(Debug)]
pub struct BeyondHalfPrecision;
impl std::fmt::Display for BeyondHalfPrecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The AI Denoise result has values beyond half precision")
    }
}
impl std::error::Error for BeyondHalfPrecision {}

#[derive(Clone, Debug)]
pub struct AiCache {
    dir: Arc<RwLock<Dir>>,
    limit: Arc<AtomicU64>,
}
impl Default for AiCache {
    fn default() -> Self {
        Self::new(cache_dir().join(AI.default), DEFAULT_LIMIT)
    }
}
impl AiCache {
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
    /// Keeps entries in this cache's subfolder of `folder` (the decode cache's chosen
    /// folder), or in the default directory for `None`.
    pub fn set_folder(&self, folder: Option<&Path>) {
        *self.dir.write().unwrap_or_else(|e| e.into_inner()) = Dir::of_in(AI, folder);
    }
    /// [`set_folder`](Self::set_folder), then removes the previous directory's entries,
    /// as [`crate::decode_cache::DecodeCache::relocate`] does.
    pub fn relocate(&self, folder: Option<&Path>) -> std::io::Result<usize> {
        let next = Dir::of_in(AI, folder);
        next.create()?;
        tempfile::tempfile_in(&next.path)?;
        let previous = self.dir();
        *self.dir.write().unwrap_or_else(|e| e.into_inner()) = next;
        if previous == self.dir() {
            return Ok(0);
        }
        Ok(purge(&previous, AI.extension))
    }
    /// Removes every entry, and nothing else. Returns how many.
    pub fn purge(&self) -> usize {
        purge(&self.dir(), AI.extension)
    }
    pub fn limit(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }
    pub fn set_limit(&self, limit: u64) {
        self.limit.store(limit, Ordering::Relaxed);
    }
    fn file(&self, key: &str) -> PathBuf {
        entry_in(&self.dir(), key, AI.extension)
    }
    pub fn contains(&self, key: &str) -> bool {
        self.file(key).is_file()
    }
    /// The entry for `key`; `None` on a miss or an unusable entry (truncated, another
    /// key, a value that is not finite).
    pub fn load(&self, key: &str) -> Option<AiEntry> {
        let path = self.file(key);
        let entry = read(&path, key).ok()?;
        let _ = fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(SystemTime::now()));
        Some(entry)
    }
    /// Stores `entry` under `key` at half precision, then trims to the cap. A value that
    /// is not finite at half precision fails the store (and the job that made it).
    pub fn store(&self, key: &str, entry: &AiEntry) -> Result<()> {
        let layout = entry.layout;
        let pixels = layout.output_pixels();
        ensure!(
            entry.rgb.len() == 3 * pixels
                && entry.clipped.len() == layout.packed_width * layout.packed_height,
            "AI Denoise result of the wrong size"
        );
        let half: Vec<u16> = entry
            .rgb
            .par_iter()
            .map(|v| f16::from_f32(*v).to_bits())
            .collect();
        if !half.par_iter().all(|b| f16::from_bits(*b).is_finite()) {
            return Err(BeyondHalfPrecision.into());
        }
        let header = serde_json::to_vec(&Header {
            format: FORMAT,
            key: key.into(),
            width: layout.width,
            height: layout.height,
            pad: layout.pad,
            gain: entry.gain,
        })?;
        let dir = self.location();
        dir.create()?;
        let mut file = tempfile::NamedTempFile::new_in(&dir.path)?;
        {
            let mut out = std::io::BufWriter::new(file.as_file_mut());
            out.write_all(MAGIC)?;
            out.write_all(&(header.len() as u32).to_le_bytes())?;
            out.write_all(&header)?;
            for b in &half {
                out.write_all(&b.to_le_bytes())?;
            }
            let flags: Vec<u8> = entry.clipped.iter().map(|c| u8::from(*c)).collect();
            out.write_all(&flags)?;
            out.flush()?;
        }
        let path = entry_in(&dir.path, key, AI.extension);
        file.persist(&path)?;
        if self.location() != dir {
            let _ = fs::remove_file(&path);
            return Ok(());
        }
        trim(&dir.path, AI.extension, self.limit())
    }
}

fn read(path: &Path, key: &str) -> Result<AiEntry> {
    let file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut reader = std::io::BufReader::new(file);
    let mut magic = [0; 8];
    reader.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "Not an AI Denoise cache entry");
    let mut len = [0; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    ensure!(len < 1 << 16, "Invalid AI Denoise cache header");
    let mut header = vec![0; len];
    reader.read_exact(&mut header)?;
    let header: Header = serde_json::from_slice(&header)?;
    ensure!(
        header.format == FORMAT && header.key == key,
        "AI Denoise cache entry of another format or key"
    );
    ensure!(
        header.width > 0
            && header.height > 0
            && header.width * header.height <= 400_000_000
            && header.pad.iter().all(|p| *p <= 1)
            && header.gain.is_finite()
            && header.gain > 0.,
        "Invalid AI Denoise cache entry"
    );
    let layout = Layout::new(header.width, header.height, header.pad);
    let pixels = layout.output_pixels() as u64;
    let packed = (layout.packed_width * layout.packed_height) as u64;
    let start = 12 + len as u64;
    ensure!(
        size == start + 3 * pixels * 2 + packed,
        "AI Denoise cache entry has the wrong size"
    );
    let file = reader.into_inner();
    let mut bytes = vec![0u8; (3 * pixels * 2) as usize];
    read_at(&file, start, &mut bytes)?;
    let rgb: Vec<f32> = bytes
        .par_chunks_exact(2)
        .map(|b| f16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32())
        .collect();
    ensure!(
        rgb.par_iter().all(|v| v.is_finite()),
        "AI Denoise cache entry has a value that is not finite"
    );
    let mut flags = vec![0u8; packed as usize];
    read_at(&file, start + 3 * pixels * 2, &mut flags)?;
    ensure!(
        flags.iter().all(|f| *f <= 1),
        "Invalid AI Denoise cache flags"
    );
    Ok(AiEntry {
        layout,
        gain: header.gain,
        rgb,
        clipped: flags.into_iter().map(|f| f == 1).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_cache::DecodeCache;

    fn entry(width: usize, height: usize, pad: [usize; 2], scale: f32) -> AiEntry {
        let layout = Layout::new(width, height, pad);
        let n = 3 * layout.output_pixels();
        AiEntry {
            layout,
            gain: 9.9e-7,
            // The measured ranges: gain-applied values up to about 1.2 (white at 1, with
            // some overshoot), and tiny shadows.
            rgb: (0..n)
                .map(|i| scale * ((i * 7919) % 10007) as f32 / 10007. * 1.2 + 1e-5)
                .collect(),
            clipped: (0..layout.packed_width * layout.packed_height)
                .map(|i| i % 11 == 0)
                .collect(),
        }
    }

    #[test]
    fn entries_round_trip_within_half_precision() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = AiCache::new(dir.path().join("ai"), DEFAULT_LIMIT);
        let stored = entry(33, 21, [1, 0], 1.);
        cache.store("k", &stored)?;
        let loaded = cache.load("k").expect("entry");
        assert_eq!(loaded.layout, stored.layout);
        assert_eq!(loaded.clipped, stored.clipped);
        assert_eq!(loaded.gain, stored.gain);
        for (a, b) in loaded.rgb.iter().zip(&stored.rgb) {
            // Half precision: 11 significant bits, relative error at most 2⁻¹¹.
            assert!((a - b).abs() <= b.abs() * 4.9e-4 + 6e-8, "{a} {b}");
        }
        assert!(cache.load("other").is_none());
        Ok(())
    }

    #[test]
    fn values_beyond_half_precision_fail_the_store_and_damaged_entries_are_misses() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = AiCache::new(dir.path().join("ai"), DEFAULT_LIMIT);
        // The model's raw scale (medians of 10⁵) cannot be stored: the gain goes first.
        let raw = entry(8, 8, [0, 0], 1.3e5);
        let error = cache.store("raw", &raw).unwrap_err();
        // Typed, for the service that fails the job on it, and worded as before.
        assert!(error.is::<BeyondHalfPrecision>());
        assert_eq!(
            error.to_string(),
            "The AI Denoise result has values beyond half precision"
        );
        let mut nan = entry(8, 8, [0, 0], 1.);
        nan.rgb[3] = f32::NAN;
        assert!(cache.store("nan", &nan).is_err());
        assert!(!cache.contains("raw") && !cache.contains("nan"));
        // Truncated.
        cache.store("k", &entry(8, 8, [0, 0], 1.))?;
        let path = cache.file("k");
        let bytes = fs::read(&path)?;
        fs::write(&path, &bytes[..bytes.len() - 1])?;
        assert!(cache.load("k").is_none());
        // A value made infinite on disk.
        let mut bytes = bytes;
        let header = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let at = 12 + header + 10;
        bytes[at..at + 2].copy_from_slice(&f16::INFINITY.to_bits().to_le_bytes());
        fs::write(&path, &bytes)?;
        assert!(cache.load("k").is_none());
        Ok(())
    }

    #[test]
    fn relocation_keeps_namespaces_apart_and_decodes_never_evict_results() -> Result<()> {
        let root = tempfile::tempdir()?;
        let chosen = root.path().join("drive");
        fs::create_dir(&chosen)?;
        let decoded = DecodeCache::new(root.path().join("decoded"), 1);
        let ai = AiCache::new(root.path().join("ai"), DEFAULT_LIMIT);
        decoded.relocate(Some(&chosen))?;
        ai.relocate(Some(&chosen))?;
        assert_ne!(decoded.dir(), ai.dir());
        assert!(ai.dir().starts_with(&chosen) && decoded.dir().starts_with(&chosen));
        ai.store("k", &entry(8, 8, [0, 0], 1.))?;
        // A decode store trims its own directory to a one-byte cap: the result stays.
        let image = crate::camera_data::CameraImage {
            recovered: Default::default(),
            width: 2,
            height: 2,
            pixels: vec![[0.5; 3]; 4],
            metadata: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let recovered = image.clone();
        let _ = image.recovered.set(Arc::new(recovered));
        decoded.store("d", &image)?;
        assert!(ai.contains("k"));
        // Purging decodes leaves results; purging results leaves decodes' folder alone.
        decoded.purge();
        assert!(ai.contains("k"));
        assert_eq!(ai.purge(), 1);
        // Relocating results back removes them from the chosen folder only.
        ai.store("k", &entry(8, 8, [0, 0], 1.))?;
        assert_eq!(ai.relocate(None)?, 1);
        Ok(())
    }

    #[test]
    fn results_are_trimmed_to_their_own_cap() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let one = {
            let probe = AiCache::new(dir.path().join("probe"), DEFAULT_LIMIT);
            probe.store("p", &entry(8, 8, [0, 0], 1.))?;
            fs::metadata(probe.file("p"))?.len()
        };
        let cache = AiCache::new(dir.path().join("ai"), 2 * one);
        for key in ["a", "b", "c"] {
            cache.store(key, &entry(8, 8, [0, 0], 1.))?;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!cache.contains("a") && cache.contains("b") && cache.contains("c"));
        Ok(())
    }
}
