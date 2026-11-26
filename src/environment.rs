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

/// The installed camera profiles of a context [`context`] made: those in the
/// data folder and the older Linux one.
pub fn camera_profiles(ctx: &rawmakase_core::Context) -> &crate::camera_profiles::ProfileLibrary {
    rawmakase_core::workspace::camera_profiles(ctx)
}

/// The XMP presets of a context [`context`] made: those in the data folder
/// and the older Linux one, and their layout in the data folder.
pub fn presets(ctx: &rawmakase_core::Context) -> &crate::presets::PresetLibrary {
    rawmakase_core::workspace::presets(ctx)
}

/// The profile manager's folder: where a context [`context`] made imports
/// camera profiles to, the data folder's `camera-profiles`.
pub fn profile_manager(ctx: &rawmakase_core::Context) -> crate::camera_profiles::manage::Library {
    camera_profiles(ctx)
        .manager()
        .expect("the environment's context has a data folder")
}

/// What photos are opened with and their edits resolved among, taken
/// together from a context: its imported lens profiles and installed camera
/// profiles. Cheap to clone; the default has neither.
#[derive(Clone, Debug)]
pub struct Libraries {
    pub lenses: crate::photo::LensProfiles,
    pub profiles: crate::camera_profiles::ProfileLibrary,
    /// Lensfun's database, which a Lensfun lens an edit names resolves against.
    pub lensfun: crate::lens::lensfun::installed::Handle,
    /// Camera Look's fit cache, which a photo without color data is fitted
    /// through.
    pub fits: crate::camera_look::cache::FitCache,
}
impl Default for Libraries {
    fn default() -> Self {
        Self {
            lenses: Default::default(),
            profiles: Default::default(),
            lensfun: crate::lens::lensfun::installed::Handle::none(),
            fits: Default::default(),
        }
    }
}

/// The libraries of a context [`context`] made.
pub fn libraries(ctx: &rawmakase_core::Context) -> Libraries {
    Libraries {
        lenses: lens_profiles(ctx).clone(),
        profiles: camera_profiles(ctx).clone(),
        lensfun: lensfun(ctx).clone(),
        fits: rawmakase_core::workspace::fit_cache(ctx),
    }
}

/// The saved watermarks of a context, its first data folder's `watermarks`
/// (none without a data folder), drawing text with this computer's [`fonts`].
/// The app's until the library's export operations hold them (step 15 of
/// docs/embedding-plan.md). Made once: its clones share the fonts listed.
pub fn watermarks(ctx: &rawmakase_core::Context) -> crate::watermark::Watermarks {
    let marks = ctx
        .data_dirs()
        .first()
        .map_or_else(crate::watermark::Watermarks::none, |data| {
            crate::watermark::Watermarks::in_dir(data.join("watermarks"))
        });
    marks.with_fonts(fonts())
}

/// The fonts text watermarks draw with on this computer: the platform's
/// system font, looked up when a face is first drawn, and installed fonts
/// in the folders the interface finds them in.
pub fn fonts() -> crate::watermark::fonts::Fonts {
    use crate::watermark::fonts::{Fonts, SystemFont};
    Fonts::new(
        SystemFont::Lookup(system_face),
        fastframe_fonts::system::font_directories(),
    )
}

/// The file and collection index of the platform's interface font at
/// `weight`: San Francisco on macOS, Segoe UI on Windows, and what
/// fontconfig gives for `system-ui` elsewhere.
fn system_face(weight: f32) -> anyhow::Result<(std::path::PathBuf, u32)> {
    #[cfg(target_os = "macos")]
    {
        use std::path::PathBuf;
        let _ = weight;
        Ok((PathBuf::from("/System/Library/Fonts/SFNS.ttf"), 0))
    }
    #[cfg(target_os = "windows")]
    {
        use anyhow::Context as _;
        use std::path::PathBuf;
        let root = std::env::var_os("SystemRoot").context("SystemRoot unavailable")?;
        let fonts = PathBuf::from(root).join("Fonts");
        Ok((
            if fonts.join("SegUIVar.ttf").is_file() {
                fonts.join("SegUIVar.ttf")
            } else {
                fonts.join(if weight >= 650. {
                    "segoeuib.ttf"
                } else if weight >= 500. {
                    "seguisb.ttf"
                } else {
                    "segoeui.ttf"
                })
            },
            0,
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        use anyhow::Context as _;
        use std::process::{Command, Stdio};
        let output = tempfile::tempfile()?;
        let mut child = Command::new("fc-match")
            .args(["-f", "%{file}\n%{index}", &fontconfig_pattern(weight)])
            .stdout(output.try_clone()?)
            .stderr(Stdio::null())
            .spawn()
            .context("Cannot resolve system watermark font")?;
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                anyhow::ensure!(status.success(), "System font lookup failed");
                break;
            }
            if start.elapsed() > std::time::Duration::from_secs(1) {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("System font lookup timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        use std::io::{Read, Seek};
        let mut output = output;
        output.rewind()?;
        let mut text = String::new();
        output.read_to_string(&mut text)?;
        parse_fontconfig(&text)
    }
}

#[cfg(any(test, not(any(target_os = "macos", target_os = "windows"))))]
fn fontconfig_pattern(weight: f32) -> String {
    format!(
        r"system\-ui:weight={}:slant=0",
        if weight >= 650. {
            200
        } else if weight >= 550. {
            180
        } else if weight >= 450. {
            100
        } else {
            80
        }
    )
}

#[cfg(any(test, not(any(target_os = "macos", target_os = "windows"))))]
fn parse_fontconfig(text: &str) -> anyhow::Result<(std::path::PathBuf, u32)> {
    use anyhow::Context as _;
    let mut lines = text.lines();
    let path = lines
        .next()
        .filter(|s| !s.is_empty())
        .context("No system font available")?;
    // Fontconfig stores a variable font's named instance in the upper bits.
    Ok((
        std::path::PathBuf::from(path),
        lines.next().unwrap_or("0").parse::<u32>()? & 0xFFFF,
    ))
}

/// Lensfun's database of a context [`context`] made, in the data folder.
pub fn lensfun(ctx: &rawmakase_core::Context) -> &crate::lens::lensfun::installed::Handle {
    rawmakase_core::workspace::lensfun(ctx)
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
    use std::path::PathBuf;

    #[test]
    fn fontconfig_system_family_is_escaped() {
        use super::fontconfig_pattern;
        assert_eq!(fontconfig_pattern(400.), r"system\-ui:weight=80:slant=0");
        assert_eq!(fontconfig_pattern(500.), r"system\-ui:weight=100:slant=0");
        assert_eq!(fontconfig_pattern(600.), r"system\-ui:weight=180:slant=0");
        assert_eq!(fontconfig_pattern(700.), r"system\-ui:weight=200:slant=0");
    }

    #[test]
    fn fontconfig_variable_instances_keep_the_collection_index() {
        use super::parse_fontconfig;
        assert_eq!(
            parse_fontconfig("/fonts/variable.ttf\n589824").unwrap(),
            (PathBuf::from("/fonts/variable.ttf"), 0)
        );
        assert_eq!(
            parse_fontconfig("/fonts/collection.ttc\n720899").unwrap().1,
            3
        );
        assert!(parse_fontconfig("").is_err());
    }

    #[test]
    #[ignore = "requires installed system fonts"]
    fn system_fonts_render_at_every_weight() {
        let fonts = super::fonts();
        for style in ["Regular", "Medium", "SemiBold", "Bold"] {
            fonts.load(crate::watermark::fonts::SYSTEM, style).unwrap();
        }
    }

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
