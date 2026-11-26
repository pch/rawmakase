//! The context: the configuration an app gives the library once, as values,
//! and what is made from them.
use crate::{Error, ErrorKind, Result};
use rawmakase_decode::ai::cache::{self as ai_cache, AiCache};
use rawmakase_decode::decode_cache::{self, DecodeCache};
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
/// that nothing reads yet is not offered. Today that is the cache folder, its
/// limits and its build id, from which the decode cache and AI Denoise's cache
/// are made. The data folders, fonts and the rest join as the crates under the
/// library take them as values (`docs/embedding-plan.md`, step 4).
#[derive(Clone, Debug)]
pub struct Context {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    cache: Option<Cache>,
}

#[derive(Debug)]
struct Cache {
    dir: PathBuf,
    build_id: String,
    decoded: DecodeCache,
    ai_denoise: AiCache,
}

/// Builds a [`Context`]; made by [`Context::builder`].
#[derive(Debug, Default)]
#[must_use]
pub struct ContextBuilder {
    cache: Option<(PathBuf, CacheLimits, String)>,
}

impl ContextBuilder {
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
                    decoded: DecodeCache::under(&dir, limits.decoded),
                    ai_denoise: AiCache::under(&dir, limits.ai_denoise),
                    dir,
                    build_id,
                })
            }
        };
        Ok(Context {
            inner: Arc::new(Inner { cache }),
        })
    }
}

impl Context {
    /// Starts a context's configuration.
    pub fn builder() -> ContextBuilder {
        ContextBuilder::default()
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
/// app, whose workers pass them to the crates under the library. An app outside
/// the workspace has no use for them: they name types the library doesn't
/// show, and may change or go without notice.
pub mod workspace {
    use super::Context;
    use rawmakase_decode::ai::cache::AiCache;
    use rawmakase_decode::decode_cache::DecodeCache;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use workspace::{ai_denoise_cache, decode_cache};

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
        let decoded = decode_cache(&ctx).unwrap();
        assert_eq!(decoded.dir(), root().join("decoded"));
        assert_eq!(decoded.limit(), 7);
        let ai = ai_denoise_cache(&ctx).unwrap();
        assert_eq!(ai.dir(), root().join("ai-denoise"));
        assert_eq!(ai.limit(), 9);
        // A clone is the same context: a limit changed through one is the other's.
        decode_cache(&ctx.clone()).unwrap().set_limit(11);
        assert_eq!(decode_cache(&ctx).unwrap().limit(), 11);
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
    fn a_context_without_a_cache_has_none() {
        let ctx = Context::builder().build().unwrap();
        assert!(decode_cache(&ctx).is_none() && ctx.build_id().is_none());
    }

    #[test]
    fn contexts_cross_threads() {
        fn send_sync<T: Send + Sync + 'static>() {}
        send_sync::<Context>();
    }
}
