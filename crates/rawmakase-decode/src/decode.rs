//! A photo's full-size image, made the one way every caller makes it: the decode
//! cache's copy when it has one, else a decode of the RAW, with highlights
//! recovered and stored for next time when the caller keeps it. Develop, the
//! prefetch of a neighbour, Reference View and export differ only in the
//! [`DecodePolicy`] they pass.
use crate::{
    camera_data::{CameraImage, Decode, Demosaic, Metadata},
    decode_cache::DecodeCache,
    raw::Raw,
};
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::path::Path;

/// What a full-size decode does beyond decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodePolicy {
    /// A photo shown in Develop or Reference View: highlights recovered (so the
    /// first render does not wait for them) and the result stored.
    Show,
    /// A neighbour decoded ahead of time: as [`Show`](Self::Show), and nothing at
    /// all when the cache already has it.
    Prefetch,
    /// An export: the cache's copy is used, but a decode is not stored, so a
    /// batch of photos never fills the cache.
    Export,
}

/// One photo's full-size image with one demosaic, and its decode cache entry.
pub struct FullSize {
    demosaic: Demosaic,
    cache: DecodeCache,
    /// `None` when the file's identity cannot be read: nothing is cached then.
    key: Option<String>,
}

impl FullSize {
    /// The photo at `path`, with `demosaic`, cached in `cache`.
    pub fn with_cache(path: &Path, demosaic: Demosaic, cache: DecodeCache) -> Self {
        Self {
            demosaic,
            key: DecodeCache::key(path, demosaic).ok(),
            cache,
        }
    }
    /// The photo whose file is the version `file` names, read by the caller
    /// before opening it, with `demosaic`, cached in `cache`. The caller checks
    /// that what it decodes is still that version; nothing is cached without one.
    pub fn of_version(
        file: Option<&crate::storage::Identity>,
        demosaic: Demosaic,
        cache: DecodeCache,
    ) -> Self {
        Self {
            demosaic,
            key: file.map(|file| DecodeCache::key_of(file, demosaic)),
            cache,
        }
    }
    /// The cache's copy, with its recovered highlights; `metadata` is the opened
    /// photo's.
    pub fn cached(&self, metadata: &Metadata) -> Option<CameraImage> {
        self.key
            .as_ref()
            .and_then(|key| self.cache.load(key, metadata))
    }
    /// Whether the cache has this photo, without reading it.
    pub fn is_cached(&self) -> bool {
        self.key
            .as_ref()
            .is_some_and(|key| self.cache.contains(key))
    }
    /// Decodes `raw` (this photo, opened). Highlights are recovered, so the first
    /// render does not wait for them and the cache holds them, unless `policy` is
    /// [`Export`](DecodePolicy::Export), which renders once.
    pub fn decode(
        &self,
        raw: Raw,
        policy: DecodePolicy,
        cancel: Cancel<'_>,
    ) -> Result<CameraImage> {
        let image = raw.develop(Decode::Full(self.demosaic), cancel)?;
        if policy != DecodePolicy::Export {
            crate::develop::quality::recovered(&image, cancel)?;
        }
        Ok(image)
    }
    /// Keeps `image`, decoded by [`decode`](Self::decode), for next time. Develop
    /// stores after showing the image, so writing it never delays the photo; a
    /// cancelled decode, or a cache that cannot be written, stores nothing.
    pub fn store(&self, image: &CameraImage, cancel: Cancel<'_>) {
        if !cancel.is_cancelled()
            && let Some(key) = &self.key
        {
            let _ = self.cache.store(key, image);
        }
    }
    /// The image as `policy` gets it in one step: the cache's copy, else opened
    /// with `open`, decoded and, unless exporting, stored. `None` for a prefetch the
    /// cache already has.
    pub fn get(
        &self,
        open: impl FnOnce() -> Result<Raw>,
        policy: DecodePolicy,
        cancel: Cancel<'_>,
    ) -> Result<Option<CameraImage>> {
        if policy == DecodePolicy::Prefetch && self.is_cached() {
            return Ok(None);
        }
        let raw = open()?;
        if policy != DecodePolicy::Prefetch
            && let Some(image) = self.cached(&raw.metadata)
        {
            return Ok(Some(image));
        }
        let image = self.decode(raw, policy, cancel)?;
        if policy != DecodePolicy::Export {
            self.store(&image, cancel);
        }
        Ok(Some(image))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn chart() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng")
    }

    #[test]
    fn showing_stores_exporting_reads_and_prefetching_skips_what_is_there() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().to_path_buf(), u64::MAX);
        let full = FullSize::with_cache(&chart(), Demosaic::default(), cache);
        let open = || crate::photo::open(&chart());
        let go = AtomicBool::new(false);
        // An export decodes but leaves the cache as it was.
        assert!(
            full.get(open, DecodePolicy::Export, Cancel::new(&go))?
                .is_some()
        );
        assert!(!full.is_cached());
        // A cancelled decode stores nothing.
        let decoded = full.decode(open()?, DecodePolicy::Show, Cancel::new(&go))?;
        full.store(&decoded, Cancel::new(&AtomicBool::new(true)));
        assert!(!full.is_cached());
        // Showing a photo keeps it, recovered, for next time.
        let shown = full
            .get(open, DecodePolicy::Show, Cancel::new(&go))?
            .unwrap();
        assert!(full.is_cached());
        let again = full.cached(&open()?.metadata).unwrap();
        assert_eq!(again.pixels, shown.pixels);
        // Then a prefetch has nothing to do, and an export uses the cache's copy.
        assert!(
            full.get(open, DecodePolicy::Prefetch, Cancel::new(&go))?
                .is_none()
        );
        let exported = full
            .get(open, DecodePolicy::Export, Cancel::new(&go))?
            .unwrap();
        assert_eq!(exported.pixels, shown.pixels);
        Ok(())
    }
}
