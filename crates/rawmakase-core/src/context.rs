//! The context: the configuration an app gives the library once, as values,
//! and what is made from them.
use crate::{Error, ErrorKind, Result};
use rawmakase_decode::ai::cache::{self as ai_cache, AiCache};
use rawmakase_decode::decode_cache::{self, DecodeCache};
use rawmakase_decode::photo::LensProfiles;
use rawmakase_interop::presets::PresetLibrary;
use rawmakase_model::camera_profiles::ProfileLibrary;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How much the caches in the context's cache folder may hold, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheLimits {
    /// Decoded photos.
    pub decoded: u64,
    /// AI Denoise's results.
    pub ai_denoise: u64,
}

impl Default for CacheLimits {
    /// The desktop's defaults: 4 GiB of decoded photos and 10 GiB of AI
    /// Denoise's results.
    fn default() -> Self {
        Self {
            decoded: decode_cache::DEFAULT_LIMIT,
            ai_denoise: ai_cache::DEFAULT_LIMIT,
        }
    }
}

/// The library's configuration, given as values, and what is made from them:
/// an app makes one and keeps it. A clone is the same context; it is `Send` and
/// `Sync`.
///
/// Each value is the context's only once the library reads it from here; one
/// that nothing reads yet is not offered. Today that is the data folders, in
/// which the imported lens profiles are found, and the cache folder, its limits
/// and its build id, from which the decode cache and AI Denoise's cache are
/// made. Fonts and the rest join as the crates under the library take them as
/// values (`docs/embedding-plan.md`, step 4).
#[derive(Clone, Debug)]
pub struct Context {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    data_dirs: Vec<PathBuf>,
    // Read by opening a photo (step 5 of docs/embedding-plan.md), and today by
    // RAWmakase's app through `workspace`.
    #[cfg_attr(not(feature = "workspace"), allow(dead_code))]
    lenses: LensProfiles,
    #[cfg_attr(not(feature = "workspace"), allow(dead_code))]
    profiles: ProfileLibrary,
    #[cfg_attr(not(feature = "workspace"), allow(dead_code))]
    presets: PresetLibrary,
    cache: Option<Cache>,
}

#[derive(Debug)]
struct Cache {
    dir: PathBuf,
    build_id: String,
    // Read by the operations that decode (step 5 of docs/embedding-plan.md),
    // and today by RAWmakase's app through `workspace`.
    #[cfg_attr(not(feature = "workspace"), allow(dead_code))]
    decoded: DecodeCache,
    #[cfg_attr(not(feature = "workspace"), allow(dead_code))]
    ai_denoise: AiCache,
}

/// Builds a [`Context`]; made by [`Context::builder`].
#[derive(Debug, Default)]
#[must_use]
pub struct ContextBuilder {
    data_dirs: Vec<PathBuf>,
    cache: Option<(PathBuf, CacheLimits, String)>,
}

impl ContextBuilder {
    /// The data folders, in order. The first is where RAWmakase would write
    /// what an app imports; all are searched for what has been imported: the
    /// camera profiles in each folder's `camera-profiles`, the lens profiles in
    /// its `lens-profiles` and the XMP presets in its `xmp-presets`; the
    /// presets' order is kept in the first. Without any, no installed profile
    /// or preset is found.
    pub fn data_dirs<P: Into<PathBuf>>(mut self, dirs: impl IntoIterator<Item = P>) -> Self {
        self.data_dirs = dirs.into_iter().map(Into::into).collect();
        self
    }

    /// The cache folder, its limits and the build id its entries belong to,
    /// given together. There is no default build id: a default would let a
    /// newer library read an older one's results. An app gives one that
    /// changes with every build of itself and of the library. Without a cache,
    /// decoded photos and AI Denoise's results are not kept.
    pub fn cache(
        mut self,
        dir: impl Into<PathBuf>,
        limits: CacheLimits,
        build_id: impl Into<String>,
    ) -> Self {
        self.cache = Some((dir.into(), limits, build_id.into()));
        self
    }

    /// The context, or [`ErrorKind::Invalid`] for an empty build id.
    pub fn build(self) -> Result<Context> {
        let cache = match self.cache {
            None => None,
            Some((dir, limits, build_id)) => {
                if build_id.trim().is_empty() {
                    return Err(Error::new(
                        ErrorKind::Invalid,
                        "A cache needs a build id: the id of the build that made its entries",
                    ));
                }
                Some(Cache {
                    decoded: DecodeCache::under(&dir, limits.decoded, &build_id),
                    ai_denoise: AiCache::under(&dir, limits.ai_denoise, &build_id),
                    dir,
                    build_id,
                })
            }
        };
        Ok(Context {
            inner: Arc::new(Inner {
                lenses: LensProfiles::in_data_dirs(&self.data_dirs),
                profiles: ProfileLibrary::in_data_dirs(&self.data_dirs),
                presets: PresetLibrary::in_data_dirs(&self.data_dirs),
                data_dirs: self.data_dirs,
                cache,
            }),
        })
    }
}

impl Context {
    /// Starts a context's configuration.
    pub fn builder() -> ContextBuilder {
        ContextBuilder::default()
    }

    /// The data folders as given, in order.
    pub fn data_dirs(&self) -> &[PathBuf] {
        &self.inner.data_dirs
    }

    /// The cache folder as given.
    pub fn cache_dir(&self) -> Option<&Path> {
        self.inner.cache.as_ref().map(|c| c.dir.as_path())
    }

    /// The build id the cache's entries belong to.
    pub fn build_id(&self) -> Option<&str> {
        self.inner.cache.as_ref().map(|c| c.build_id.as_str())
    }
}

/// The crates' own handles on what a [`Context`] holds, for RAWmakase's own
/// app, whose workers pass them to the crates under the library. Only with the
/// `workspace` feature, which the app turns on: they name types the library
/// doesn't show, and are no part of its surface.
#[cfg(feature = "workspace")]
#[doc(hidden)]
pub mod workspace {
    use super::Context;
    use rawmakase_decode::ai::cache::AiCache;
    use rawmakase_decode::decode_cache::DecodeCache;
    use rawmakase_decode::photo::LensProfiles;
    use rawmakase_model::camera_profiles::ProfileLibrary;

    /// The context's decode cache, when it has a cache. A clone is the same
    /// cache, and its folder and limit can be changed while it is in use, as
    /// the desktop's Preferences do.
    pub fn decode_cache(ctx: &Context) -> Option<&DecodeCache> {
        ctx.inner.cache.as_ref().map(|c| &c.decoded)
    }

    /// The context's AI Denoise cache, beside the decode cache.
    pub fn ai_denoise_cache(ctx: &Context) -> Option<&AiCache> {
        ctx.inner.cache.as_ref().map(|c| &c.ai_denoise)
    }

    /// The imported lens profiles in the context's data folders, which every
    /// photo is opened with. A clone shares what was read.
    pub fn lens_profiles(ctx: &Context) -> &LensProfiles {
        &ctx.inner.lenses
    }

    /// The camera profiles installed in the context's data folders, which a
    /// photo's profiles are listed from and its edit resolved among. A clone
    /// shares what was read.
    pub fn camera_profiles(ctx: &Context) -> &ProfileLibrary {
        &ctx.inner.profiles
    }

    /// The XMP presets installed in the context's data folders, and their
    /// layout file in the first.
    pub fn presets(ctx: &Context) -> &rawmakase_interop::presets::PresetLibrary {
        &ctx.inner.presets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        std::env::temp_dir().join("rawmakase-core-context-test")
    }

    #[test]
    fn a_context_makes_its_caches_from_the_folder_and_limits() {
        let limits = CacheLimits {
            decoded: 7,
            ai_denoise: 9,
        };
        let ctx = Context::builder()
            .cache(root(), limits, "build-1")
            .build()
            .unwrap();
        assert_eq!(ctx.cache_dir(), Some(root().as_path()));
        assert_eq!(ctx.build_id(), Some("build-1"));
        let decoded = &ctx.inner.cache.as_ref().unwrap().decoded;
        assert_eq!(decoded.dir(), root().join("decoded"));
        assert_eq!(decoded.limit(), 7);
        let ai = &ctx.inner.cache.as_ref().unwrap().ai_denoise;
        assert_eq!(ai.dir(), root().join("ai-denoise"));
        assert_eq!(ai.limit(), 9);
        // A clone is the same context: a limit changed through one is the other's.
        ctx.clone()
            .inner
            .cache
            .as_ref()
            .unwrap()
            .decoded
            .set_limit(11);
        assert_eq!(ctx.inner.cache.as_ref().unwrap().decoded.limit(), 11);
    }

    #[test]
    fn a_cache_needs_a_build_id() {
        for id in ["", "  "] {
            let e = Context::builder()
                .cache(root(), CacheLimits::default(), id)
                .build()
                .unwrap_err();
            assert_eq!(e.kind(), ErrorKind::Invalid, "{e}");
        }
    }

    #[test]
    fn the_lens_profiles_are_looked_for_in_each_data_folder() {
        let ctx = Context::builder()
            .data_dirs(["/data", "/older"])
            .build()
            .unwrap();
        assert_eq!(
            ctx.data_dirs(),
            [PathBuf::from("/data"), PathBuf::from("/older")]
        );
        assert_eq!(
            ctx.inner.lenses.dirs(),
            [
                PathBuf::from("/data/lens-profiles"),
                PathBuf::from("/older/lens-profiles")
            ]
        );
        assert_eq!(
            ctx.inner.profiles.dirs(),
            [
                PathBuf::from("/data/camera-profiles"),
                PathBuf::from("/older/camera-profiles")
            ]
        );
        let none = Context::builder().build().unwrap();
        assert!(none.inner.lenses.dirs().is_empty() && none.inner.profiles.dirs().is_empty());
    }

    #[test]
    fn a_context_without_a_cache_has_none() {
        let ctx = Context::builder().build().unwrap();
        assert!(ctx.cache_dir().is_none() && ctx.build_id().is_none());
    }

    #[test]
    fn contexts_cross_threads() {
        fn send_sync<T: Send + Sync + 'static>() {}
        send_sync::<Context>();
    }
}
