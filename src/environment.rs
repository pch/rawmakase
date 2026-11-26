//! What the desktop app and the command line take from the environment to
//! configure the library: the folders each platform keeps RAWmakase's files
//! in (`RAWMAKASE_DATA_DIR`, the older Linux data folder searched too),
//! `RAWMAKASE_CACHE_DIR` and the build the caches are keyed by. Read here,
//! once, at startup, and given to [`rawmakase_core::Context`] as values; the
//! library's crates read none of it themselves (`docs/embedding-plan.md`).
//!
//! It has no window in it, so the command line builds its context here too.
/// The context the app and the command line run with: the platform's cache
/// folder, with the desktop's default limits (Preferences change them on the
/// context's caches once the session is read), keyed by this executable's
/// build.
pub fn context() -> rawmakase_core::Context {
    rawmakase_core::Context::builder()
        .data_dirs(crate::storage::asset_dirs())
        .cache(
            crate::decode_cache::cache_dir(),
            rawmakase_core::CacheLimits::default(),
            build_id(),
        )
        .build()
        .expect("a context with a build id builds")
}

/// The decode cache of a context [`context`] made, which always has a cache.
pub fn decode_cache(ctx: &rawmakase_core::Context) -> &crate::decode_cache::DecodeCache {
    rawmakase_core::workspace::decode_cache(ctx).expect("the environment's context has a cache")
}

/// AI Denoise's cache of a context [`context`] made.
pub fn ai_denoise_cache(ctx: &rawmakase_core::Context) -> &crate::ai_cache::AiCache {
    rawmakase_core::workspace::ai_denoise_cache(ctx).expect("the environment's context has a cache")
}

/// The imported lens profiles of a context [`context`] made: those in the
/// data folder and the older Linux one, as the app has always found them.
pub fn lens_profiles(ctx: &rawmakase_core::Context) -> &crate::photo::LensProfiles {
    rawmakase_core::workspace::lens_profiles(ctx)
}

/// What the caches' entries made by this build are keyed by: the executable's
/// size and time, as the decode cache has always keyed them, so entries made
/// before survive. When the executable can't be read, its version.
fn build_id() -> String {
    match crate::decode_cache::build_identity() {
        id if id.is_empty() => format!("rawmakase {}", env!("CARGO_PKG_VERSION")),
        id => id,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_context_has_the_platform_s_cache_and_this_build() {
        let ctx = super::context();
        let cache = crate::decode_cache::cache_dir();
        assert_eq!(ctx.cache_dir(), Some(cache.as_path()));
        assert_eq!(
            super::decode_cache(&ctx).dir(),
            crate::decode_cache::DecodeCache::default().dir()
        );
        assert_eq!(
            super::ai_denoise_cache(&ctx).dir(),
            crate::ai_cache::AiCache::default().dir()
        );
        // The desktop keys its caches as it always has, by this executable, so
        // the entries it made before the library existed are still read.
        let build = crate::decode_cache::build_identity();
        assert_eq!(ctx.build_id(), Some(build.as_str()));
        assert_eq!(super::decode_cache(&ctx).build(), build);
        assert_eq!(super::ai_denoise_cache(&ctx).build(), build);
        // Lens profiles are found in both data folders, as before.
        let dirs: Vec<_> = crate::storage::asset_dirs()
            .into_iter()
            .map(|d| d.join("lens-profiles"))
            .collect();
        assert_eq!(super::lens_profiles(&ctx).dirs(), dirs.as_slice());
    }
}
