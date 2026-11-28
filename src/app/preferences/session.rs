//! The desktop session: the last photo and catalog place, the panels and
//! layouts as the user left them, and the preferences kept between launches.
use crate::catalog::PhotoId;
use crate::storage::{data_dir, read_json_or_default};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// `Session::copy_groups` as releases before AI Denoise read it: without AI Denoise's,
/// High Dynamic Range's and Negative Conversion's groups, which are saved apart.
mod known_groups {
    use crate::model::settings_groups::{GroupInclusion, GroupSelection, SettingGroup};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub(super) fn serialize<S: Serializer>(
        groups: &Option<GroupSelection>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        groups
            .as_ref()
            .map(|g| {
                g.groups()
                    .filter(|g| {
                        !matches!(
                            g,
                            SettingGroup::AiDenoise
                                | SettingGroup::HighDynamicRange
                                | SettingGroup::NegativeConversion
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .serialize(s)
    }
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<GroupSelection>, D::Error> {
        let names =
            Option::<Vec<serde_json::Value>>::deserialize(serde_json::Value::deserialize(d)?)
                .unwrap_or_default();
        Ok(names.map(|names| {
            let mut groups = GroupSelection::none();
            for group in names
                .into_iter()
                .filter_map(|n| serde_json::from_value::<SettingGroup>(n).ok())
            {
                groups.set(group, GroupInclusion::Included);
            }
            groups
        }))
    }
}
/// Reads a session field, or its default when the value does not read: a malformed
/// field resets itself, not the whole session.
fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    Ok(T::deserialize(serde_json::Value::deserialize(d)?).unwrap_or_default())
}
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Session {
    #[serde(default, deserialize_with = "lenient")]
    pub last_path: Option<PathBuf>,
    /// Preferences > Monitor profile: a chosen ICC profile; none follows the
    /// system, or with `monitor_srgb` shows sRGB as it is.
    #[serde(default, deserialize_with = "lenient")]
    pub monitor: Option<PathBuf>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub monitor_srgb: bool,
    /// The window was maximized when last seen; it opens maximized again.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub maximized: bool,
    /// Titles of panel sections the user collapsed; all others start open unless
    /// marked collapsed by default.
    #[serde(default, deserialize_with = "lenient")]
    pub collapsed: std::collections::BTreeSet<String>,
    /// Titles of default-collapsed panel sections (such as Negative) the user expanded.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::collections::BTreeSet::is_empty"
    )]
    pub expanded: std::collections::BTreeSet<String>,
    /// The sides of the window in Solo Mode, where opening a panel closes the
    /// others: "develop-left", "develop-right", "library-left", "library-right".
    #[serde(default, deserialize_with = "lenient")]
    pub solo: std::collections::BTreeSet<String>,
    /// The panels Develop and the Library show; all of them by default.
    #[serde(default, deserialize_with = "crate::app::shell::panels::lenient")]
    pub panels: crate::app::shell::panels::WorkspacePanels,
    /// The first-run setup was completed; until then it opens on launch.
    #[serde(default, deserialize_with = "lenient")]
    pub onboarding_done: bool,
    /// Where the user was in the last catalog: the Library folder, the
    /// selected photo, and whether Develop was open.
    #[serde(default, deserialize_with = "lenient")]
    pub library_source: String,
    #[serde(default, deserialize_with = "lenient")]
    pub selected_photo: Option<PhotoId>,
    #[serde(default, deserialize_with = "lenient")]
    pub develop: bool,
    /// Which demosaic full-size decodes use.
    #[serde(default, deserialize_with = "lenient")]
    pub demosaic: crate::camera_data::Demosaic,
    /// The user turned off checking for updates, which is on by default.
    #[serde(default, deserialize_with = "lenient")]
    pub no_update_checks: bool,
    /// Preferences › General's Map: shown under the Metadata panel's GPS
    /// row; off by default, as its tiles tell the server where photos were taken.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub map: bool,
    /// The map's tile server address, with `{z}`, `{x}` and `{y}`; none is
    /// OpenStreetMap's.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub map_tiles: Option<String>,
    /// The credit shown on the map; none is OpenStreetMap's.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub map_credit: Option<String>,
    /// Quitting asks first; off by default.
    #[serde(default, deserialize_with = "lenient")]
    pub confirm_quit: bool,
    /// A release the user chose to skip; newer ones are still offered.
    #[serde(default, deserialize_with = "lenient")]
    pub skipped_version: Option<String>,
    /// The interface's palette file; None is RAWmakase's own greys.
    #[serde(default, deserialize_with = "lenient")]
    pub theme: Option<String>,
    /// The user picked `theme`. Until then Omarchy's theme is followed on
    /// an Omarchy desktop, and RAWmakase's greys elsewhere.
    #[serde(default, deserialize_with = "lenient")]
    pub theme_chosen: bool,
    /// Photo > Auto Advance: a rating, flag or label moves to the next photo.
    #[serde(default, deserialize_with = "lenient")]
    pub auto_advance: bool,
    /// The user turned off applying the Auto mix when first converting to black &
    /// white, which is on by default.
    #[serde(default, deserialize_with = "lenient")]
    pub no_auto_black_white_mix: bool,
    /// Preferences > Profiles & Presets: the user turned off Lens Corrections'
    /// suggestions of Lensfun profiles, which are on by default.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub no_lensfun_suggestions: bool,
    /// Cameras ("make model", lower case) whose community camera-profile offer the
    /// user dismissed under Develop's Profile menu.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::collections::BTreeSet::is_empty"
    )]
    pub dismissed_community_profiles: std::collections::BTreeSet<String>,
    /// The groups Manage Profiles hides from Develop's Profile menu, by name.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::collections::BTreeSet::is_empty"
    )]
    pub hidden_profile_groups: std::collections::BTreeSet<String>,
    /// The profiles the Profile menu lists first, by `camera_profiles::manage::key`:
    /// a look by its UUID, which a rename keeps, another profile by its name.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::collections::BTreeSet::is_empty"
    )]
    pub favorite_profiles: std::collections::BTreeSet<String>,
    /// Preferences > Catalog: the user paused reading checksums.
    #[serde(default, deserialize_with = "lenient")]
    pub pause_checksums: bool,
    /// How the Library showed its photos last time.
    #[serde(default, deserialize_with = "lenient")]
    pub library_layout: LibraryLayout,
    /// The groups Copy Settings copied last time; None until it has been used. Written
    /// with only the groups releases before AI Denoise know (an unknown one makes them
    /// drop the whole session) and read leniently, skipping names this release does
    /// not know; AI Denoise's group is in `copy_ai_denoise`, High Dynamic Range's in
    /// `copy_hdr`, Negative Conversion's in `copy_negative`.
    #[serde(default, with = "known_groups")]
    pub copy_groups: Option<crate::model::settings_groups::GroupSelection>,
    /// The groups Synchronize Settings applied last time, which Auto Sync applies;
    /// None until it has been used. Read leniently, skipping names this release does
    /// not know.
    #[serde(
        default,
        deserialize_with = "known_groups::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub sync_groups: Option<crate::model::settings_groups::GroupSelection>,
    /// Develop's Auto Sync: an edit of the open photo goes to the other selected
    /// photos too.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub auto_sync: bool,
    /// Whether Copy Settings includes AI Denoise, apart from `copy_groups`.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub copy_ai_denoise: Option<bool>,
    /// Whether Copy Settings includes High Dynamic Range, apart from `copy_groups`.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub copy_hdr: Option<bool>,
    /// Whether Copy Settings includes Negative Conversion, apart from `copy_groups`.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub copy_negative: Option<bool>,
    /// Develop's Presets panel shows its presets as a list of names rather than
    /// as thumbnails of the photo.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub preset_list: bool,
    /// The color behind the photo, by name ("white", "gray", "black"); none, or
    /// a name this version does not know, is the theme's.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub backdrop: Option<String>,
    /// The zoom level a click on the photo zooms to, in screen pixels per
    /// image pixel (1 is 100%): the level last chosen, zoomed in or not. None
    /// is 100%.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub zoom_level: Option<f32>,
    /// The corner of the view the zoomed-in minimap sits in, by
    /// [`crate::app::view::minimap::Corner::key`]; none, or a name this version
    /// does not know, is the bottom right.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub minimap_corner: Option<String>,
    /// The Crop tool's guide overlay.
    #[serde(default, deserialize_with = "lenient")]
    pub crop_guides: CropGuideLayout,
    /// The Crop tool's aspect last chosen from its menu, by stable name ("original",
    /// "free", "1x1", "4x5", …); an uncropped photo opens with it. A name this
    /// version does not know reads as Original.
    #[serde(default, deserialize_with = "lenient")]
    pub crop_aspect: String,
    /// Preferences > Raw Defaults: what photos without an edit start from.
    #[serde(default, deserialize_with = "crate::raw_defaults::lenient_settings")]
    pub raw_defaults: crate::raw_defaults::RawDefaults,
    /// Preferences > Standard Preview Size, the long edge in pixels.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub standard_preview_size: Option<u32>,
    /// Preferences > Performance > Thumbnail cache: how many MiB of Library
    /// thumbnails the preview cache keeps, one of [`THUMBNAIL_CACHE_SIZES`] or
    /// another in [`THUMBNAIL_CACHE_SIZE_RANGE`] a later release offers.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub thumbnail_cache_size_mib: Option<u32>,
    /// Preferences > Catalog: decoded-image disk cache limit in GiB.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub decoded_cache_size_gib: Option<u32>,
    /// Preferences > Performance: the folder chosen for decoded images; `None`
    /// keeps them in the per-user cache directory.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub decoded_cache_folder: Option<PathBuf>,
    /// Preferences > Automatically Discard 1:1 Previews, in days unused; 0 is
    /// Never.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub one_to_one_discard_days: Option<u32>,
    /// Preferences > Performance > AI Denoise: run it on the Neural Engine (Apple
    /// Silicon). Off unless chosen; a release before it ignores the field.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub ai_denoise_neural_engine: bool,
    /// Preferences > External Editing: the programs Edit In opens photos in,
    /// in the order the user put them; the first is Cmd+E's.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub external_editors: Vec<PathBuf>,
    /// Preferences > External Editing: the color space of Edit In's TIFFs, by
    /// [`crate::color::OutputSpace::name`]; none, or a name this
    /// version does not know, is ProPhoto RGB.
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub edit_in_color_space: Option<String>,
    /// Fields this release does not know, written by a later or an earlier one: kept
    /// as they were read and written back, so going back to that release keeps them.
    #[serde(flatten)]
    pub unknown: Unknown,
}
/// A session's fields this release does not know. A retired field that earlier
/// releases still read strictly is kept only when it reads as they expect: a
/// malformed one would make them drop the whole session.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct Unknown(serde_json::Map<String, serde_json::Value>);
impl<'de> Deserialize<'de> for Unknown {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut fields = serde_json::Map::deserialize(d)?;
        fields.retain(|name, value| match name.as_str() {
            // Pinned panels, read by v0.3.0 to v0.4.7.
            "pinned" => std::collections::BTreeSet::<String>::deserialize(value.clone()).is_ok(),
            _ => true,
        });
        Ok(Self(fields))
    }
}

impl Session {
    /// The Copy Settings groups saved last time, with AI Denoise's, High Dynamic
    /// Range's and Negative Conversion's (saved apart), which a session from before
    /// them includes, as they are by default.
    pub(crate) fn copy_groups(&self) -> Option<crate::model::settings_groups::GroupSelection> {
        use crate::model::settings_groups::{GroupInclusion, SettingGroup};
        let mut groups = self.copy_groups.clone()?;
        let inclusion = if self.copy_ai_denoise.unwrap_or(true) {
            GroupInclusion::Included
        } else {
            GroupInclusion::Excluded
        };
        groups.set(SettingGroup::AiDenoise, inclusion);
        let inclusion = if self.copy_hdr.unwrap_or(true) {
            GroupInclusion::Included
        } else {
            GroupInclusion::Excluded
        };
        groups.set(SettingGroup::HighDynamicRange, inclusion);
        let inclusion = if self.copy_negative.unwrap_or(true) {
            GroupInclusion::Included
        } else {
            GroupInclusion::Excluded
        };
        groups.set(SettingGroup::NegativeConversion, inclusion);
        Some(groups)
    }
    /// The Thumbnail Cache size in MiB, or the default when none in
    /// [`THUMBNAIL_CACHE_SIZE_RANGE`] was saved.
    pub(crate) fn thumbnail_cache_size_mib(&self) -> u32 {
        self.thumbnail_cache_size_mib
            .filter(|size| THUMBNAIL_CACHE_SIZE_RANGE.contains(size))
            .unwrap_or((crate::catalog::preview_cache::DEFAULT_THUMBNAIL_BUDGET >> 20) as u32)
    }
    pub(crate) fn decoded_cache_size_gib(&self) -> u32 {
        self.decoded_cache_size_gib
            .filter(|size| DECODED_CACHE_SIZE_RANGE.contains(size))
            .unwrap_or((crate::decode_cache::DEFAULT_LIMIT >> 30) as u32)
    }
    /// The Standard Preview Size, or the default when none of the offered
    /// sizes was saved.
    pub(crate) fn standard_preview_size(&self) -> u32 {
        self.standard_preview_size
            .filter(|size| crate::app::library::preview_build::STANDARD_SIZES.contains(size))
            .unwrap_or(crate::app::library::preview_build::DEFAULT_STANDARD_SIZE)
    }
    /// After how many days unused 1:1 previews are discarded; `None` keeps them.
    pub(crate) fn discard_one_to_one_after(&self) -> Option<u32> {
        use crate::app::library::preview_build::{DEFAULT_DISCARD_DAYS, DISCARD_CHOICES};
        match self.one_to_one_discard_days {
            Some(0) => None,
            Some(days) if DISCARD_CHOICES.iter().any(|(d, _)| *d == Some(days)) => Some(days),
            _ => DEFAULT_DISCARD_DAYS,
        }
    }
    /// Whether restoring this session would reopen Develop, a non-Grid Library
    /// view (Loupe, Compare or Survey) or a selected photo.
    pub(in crate::app) fn has_photo_or_view(&self) -> bool {
        self.develop
            || self.selected_photo.is_some()
            || (!self.library_layout.view.is_empty() && self.library_layout.view != "grid")
    }
}

/// Written to the lock file while an instance owns the session guard; cleared
/// to empty only on a clean shutdown.
const ACTIVE_MARKER: &[u8] = b"running\n";

/// Detects when the previous desktop launch did not exit cleanly, using an OS
/// advisory lock beside `session.json` (`session.lock`).
///
/// The lock file is never deleted: its contents say whether the last lock
/// holder exited cleanly (empty) or died mid-session (`running\n`). The OS
/// releases the lock automatically if the process crashes, leaving the marker
/// behind for the next launch. A second concurrent instance gets `WouldBlock`
/// and fails open without mistaking the running instance for a crash.
pub(in crate::app) struct LaunchGuard {
    file: std::fs::File,
}

impl LaunchGuard {
    /// The lock file beside `session_file`.
    pub(in crate::app) fn path_for(session_file: &std::path::Path) -> PathBuf {
        session_file.with_extension("lock")
    }
    /// Acquires the launch guard beside `session_file`. Returns the held guard
    /// (marked clean only by [`Self::mark_clean`]) and whether the previous
    /// owner did not exit cleanly.
    ///
    /// Fails open when the lock file cannot be opened or locked, or when
    /// another live instance already holds it.
    pub(in crate::app) fn enter(session_file: &std::path::Path) -> (Option<Self>, bool) {
        use std::io::{Read, Seek, SeekFrom, Write};
        let path = Self::path_for(session_file);
        if std::fs::create_dir_all(crate::storage::parent_dir(&path)).is_err() {
            return (None, false);
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
        else {
            return (None, false);
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock | std::fs::TryLockError::Error(_)) => {
                return (None, false);
            }
        }
        let mut buf = Vec::new();
        let _ = (&mut file).take(64).read_to_end(&mut buf);
        let previous_unclean = buf.iter().any(|b| !b.is_ascii_whitespace());
        // Leave an existing dirty marker in place so a crash during recovery
        // startup cannot briefly leave the file empty before rewriting it.
        if !previous_unclean {
            let _ = file
                .seek(SeekFrom::Start(0))
                .and_then(|_| file.set_len(0))
                .and_then(|_| file.write_all(ACTIVE_MARKER))
                .and_then(|_| file.sync_data());
        }
        (Some(Self { file }), previous_unclean)
    }
    /// Clears the active marker and releases the lock on a clean shutdown.
    pub(in crate::app) fn mark_clean(self) {
        let _ = self.file.set_len(0).and_then(|_| self.file.sync_data());
    }
}

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        // Explicitly unlock before closing so a forked child that briefly
        // inherited this open file description before `exec` (e.g. an extension
        // launched via `pre_exec`) cannot keep the `flock` held after we drop.
        let _ = self.file.unlock();
    }
}

/// The Thumbnail Cache sizes offered, in MiB: the default 512 MiB holds about
/// 6,000 thumbnails.
pub(in crate::app) const THUMBNAIL_CACHE_SIZES: [u32; 6] = [512, 1024, 2048, 4096, 8192, 16384];
/// The Thumbnail Cache sizes kept when read, in MiB: those offered and others
/// a later release may offer, so that reading its session here and saving it
/// again doesn't replace the user's choice.
pub(in crate::app) const THUMBNAIL_CACHE_SIZE_RANGE: std::ops::RangeInclusive<u32> = 64..=1_048_576;

pub(in crate::app) const DECODED_CACHE_SIZE_RANGE: std::ops::RangeInclusive<u32> = 1..=1_048_576;

/// The Crop tool's guide overlay, which way round it is and when it shows, by stable
/// names as [`LibraryLayout`]; a name this version does not know reads as the default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct CropGuideLayout {
    /// "grid", "thirds", "diagonal", "triangle", "golden-ratio" or "golden-spiral".
    pub guide: String,
    pub orientation: u32,
    /// "always", "auto" or "never".
    pub show: String,
}

/// The session as the editor keeps it while it runs: the preferences it holds, the
/// place and layout it last remembered, and how to write it all to `session.json`.
///
/// It is the only writer of the file: [`SessionState::write`] takes what other owners
/// keep (the monitor, updates, themes, caches…) as a [`Session`] and adds its own
/// fields and the ones a newer release wrote ([`Unknown`]), which every write carries.
/// Without a file (tests, isolated UI checks) nothing is written.
pub(in crate::app) struct SessionState {
    /// Where the session is written; none writes nothing.
    file: Option<PathBuf>,
    /// Held while running when this instance owns `session.lock`; cleared only
    /// on a clean exit.
    guard: Option<LaunchGuard>,
    /// Fields a newer release wrote, carried to every write.
    unknown: Unknown,
    /// The path the session opened with, saved while no catalog or photo is open.
    fallback_path: Option<PathBuf>,
    /// The Library's place and layout, as last remembered.
    place: crate::app::CatalogPlace,
    layout: LibraryLayout,
    /// Whether the window is maximized, as last saved.
    pub(in crate::app) maximized: bool,
    /// The panel sections folded, default-collapsed sections expanded, and the sides in
    /// solo mode.
    pub(in crate::app) collapsed: std::collections::BTreeSet<String>,
    pub(in crate::app) expanded: std::collections::BTreeSet<String>,
    pub(in crate::app) solo: std::collections::BTreeSet<String>,
    pub(in crate::app) onboarding_done: bool,
    /// Preferences > Performance's demosaic, for every full-size decode.
    pub(in crate::app) demosaic: crate::camera_data::Demosaic,
    pub(in crate::app) auto_advance: bool,
    pub(in crate::app) checksums_paused: bool,
    /// The groups Copy Settings and Synchronize start from.
    pub(in crate::app) copy_groups: crate::model::settings_groups::GroupSelection,
    /// The groups Synchronize applied last time, for Auto Sync and the next
    /// Synchronize; None until it has been used.
    pub(in crate::app) sync_groups: Option<crate::model::settings_groups::GroupSelection>,
    pub(in crate::app) auto_sync: bool,
    pub(in crate::app) first_conversion: crate::app::develop::treatment::FirstConversion,
    /// The zoom level, as last saved; see [`Session::zoom_level`].
    pub(in crate::app) zoom_level: Option<f32>,
}

impl SessionState {
    /// The state `session` was loaded as, written to `file`, with the editor's own
    /// starting values for the ones it works out at launch. When `recovered` is
    /// true, the remembered Library view resets to Grid and a non-catalog
    /// `last_path` is not carried forward.
    pub(in crate::app) fn new(
        session: &Session,
        file: Option<PathBuf>,
        guard: Option<LaunchGuard>,
        place: crate::app::CatalogPlace,
        onboarding_done: bool,
        copy_groups: crate::model::settings_groups::GroupSelection,
        recovered: bool,
    ) -> Self {
        let mut layout = session.library_layout.clone();
        if recovered {
            layout.view = "grid".into();
        }
        let fallback_path = session
            .last_path
            .clone()
            .filter(|p| !recovered || p.extension().is_some_and(|e| e == "rawmakase"));
        Self {
            file,
            guard,
            unknown: session.unknown.clone(),
            fallback_path,
            place,
            layout,
            maximized: session.maximized,
            collapsed: session.collapsed.clone(),
            expanded: session.expanded.clone(),
            solo: session.solo.clone(),
            onboarding_done,
            demosaic: session.demosaic,
            auto_advance: session.auto_advance,
            checksums_paused: session.pause_checksums,
            copy_groups,
            sync_groups: session.sync_groups.clone(),
            auto_sync: session.auto_sync,
            first_conversion: if session.no_auto_black_white_mix {
                crate::app::develop::treatment::FirstConversion::KeepMix
            } else {
                crate::app::develop::treatment::FirstConversion::AutoMix
            },
            zoom_level: session.zoom_level,
        }
    }
    /// Records that this launch exited cleanly and releases the launch guard.
    pub(in crate::app) fn mark_clean_exit(&mut self) {
        if let Some(guard) = self.guard.take() {
            guard.mark_clean();
        }
    }
    /// Where the session is written, if anywhere.
    pub(in crate::app) fn file(&self) -> Option<&std::path::Path> {
        self.file.as_deref()
    }
    /// The Library layout last remembered.
    pub(in crate::app) fn layout(&self) -> &LibraryLayout {
        &self.layout
    }
    /// Remembers the Library's `place` and, when it is `shown`, its layout: a new place
    /// at once, with the layout shown then even while it is still `changing`; a layout
    /// alone once it has settled. Whether anything changed, for the caller to write.
    pub(in crate::app) fn remember_place(
        &mut self,
        place: crate::app::CatalogPlace,
        shown: Option<LibraryLayout>,
        changing: bool,
    ) -> bool {
        let layout_changed = !changing && shown.as_ref().is_some_and(|l| *l != self.layout);
        if shown.is_none() || (place == self.place && !layout_changed) {
            return false;
        }
        self.place = place;
        if let Some(shown) = shown {
            self.layout = shown;
        }
        true
    }
    /// Writes the session: `owned`, what the other owners keep, with this state's
    /// fields, the unknown ones, and `path` (the catalog or photo open) or else the path
    /// the session opened with. Nothing without a file.
    pub(in crate::app) fn write(
        &self,
        owned: Session,
        path: Option<PathBuf>,
    ) -> anyhow::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        crate::storage::atomic_json(
            file,
            &Session {
                last_path: path.or_else(|| self.fallback_path.clone()),
                maximized: self.maximized,
                collapsed: self.collapsed.clone(),
                expanded: self.expanded.clone(),
                solo: self.solo.clone(),
                onboarding_done: self.onboarding_done,
                library_source: self.place.source.clone(),
                selected_photo: self.place.photo,
                develop: self.place.module == crate::app::Module::Develop,
                demosaic: self.demosaic,
                auto_advance: self.auto_advance,
                no_auto_black_white_mix: self.first_conversion
                    == crate::app::develop::treatment::FirstConversion::KeepMix,
                pause_checksums: self.checksums_paused,
                library_layout: self.layout.clone(),
                copy_groups: Some(self.copy_groups.clone()),
                sync_groups: self.sync_groups.clone(),
                auto_sync: self.auto_sync,
                copy_ai_denoise: Some(
                    self.copy_groups
                        .contains(crate::model::settings_groups::SettingGroup::AiDenoise),
                ),
                copy_hdr: Some(
                    self.copy_groups
                        .contains(crate::model::settings_groups::SettingGroup::HighDynamicRange),
                ),
                copy_negative: Some(
                    self.copy_groups
                        .contains(crate::model::settings_groups::SettingGroup::NegativeConversion),
                ),
                zoom_level: self.zoom_level,
                unknown: self.unknown.clone(),
                ..owned
            },
        )
    }
    #[cfg(test)]
    pub(in crate::app) fn set_file(&mut self, file: Option<PathBuf>) {
        self.file = file;
    }
    #[cfg(test)]
    pub(in crate::app) fn set_fallback_path(&mut self, path: Option<PathBuf>) {
        self.fallback_path = path;
    }
    /// Only the symlink test (Unix) changes the remembered layout directly.
    #[cfg(all(test, unix))]
    pub(in crate::app) fn layout_mut(&mut self) -> &mut LibraryLayout {
        &mut self.layout
    }
}

/// How the Library shows its photos: its view, filter bar, sort and grid,
/// kept across launches. Names are stable keys, so a layout from another
/// version reads as far as it is understood and the rest is the default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct LibraryLayout {
    /// "grid", "loupe", "compare" or "survey".
    pub view: String,
    pub sort: String,
    pub reverse: bool,
    pub cell_style: String,
    pub thumb_size: Option<f32>,
    pub query: String,
    pub flags: Vec<i32>,
    pub rating: Option<i32>,
    pub rating_op: String,
    /// Color names, "none" for no label and "other" for any other.
    pub labels: Vec<String>,
    pub kind: String,
    /// Cmd+L turned the filter bar off.
    pub filters_off: bool,
    /// Show Photos in Subfolders turned off.
    pub subfolders_hidden: bool,
    /// The Metadata filter's choices, by attribute key ("camera", "lens",
    /// "focal_length", "aperture", "shutter_speed", "iso", "file_type",
    /// "keyword"); null is the photos without a value.
    pub metadata: std::collections::BTreeMap<String, Vec<Option<String>>>,
    /// Whether the Metadata strip shows under the filter bar.
    pub metadata_strip: bool,
    /// The attribute keys of the strip's four columns, left to right.
    pub metadata_columns: Vec<String>,
    /// Whether the Metadata filter's keywords are chosen by path (`Paris <
    /// France`), as since keywords of one name were told apart; earlier
    /// layouts chose them by name alone, which chooses every one of it.
    pub keyword_paths: bool,
    /// The Keywording panel's Keyword Set: the set's name, none for Recent
    /// Keywords.
    pub keyword_set: Option<String>,
    /// Its nine slots as Edit Set changed them; empty when unchanged.
    pub keyword_slots: Vec<String>,
}
pub(crate) fn load_session() -> Session {
    read_json_or_default(&data_dir().join("session.json"))
}

#[cfg(test)]
mod raw_defaults_tests {
    use super::Session;
    use crate::raw_defaults::{DefaultChoice, RawDefaults};

    /// A session saved while RAWmakase offered LibRaw's demosaic reads as
    /// RAWmakase's, and keeps every other preference: an unreadable session
    /// would reset them all.
    #[test]
    fn a_session_that_chose_libraws_demosaic_keeps_its_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(
            &path,
            r#"{"last_path":"/photos/a.ARW","monitor":null,"demosaic":"Libraw",
                "onboarding_done":true,"auto_advance":true,"theme":"nord.toml",
                "skipped_version":"1.2.3","decoded_cache_size_gib":16}"#,
        )
        .unwrap();
        let session: Session = crate::storage::read_json_or_default(&path);
        assert_eq!(session.demosaic, crate::camera_data::Demosaic::Rawmakase);
        assert_eq!(
            session.last_path.as_deref(),
            Some(std::path::Path::new("/photos/a.ARW"))
        );
        assert!(session.onboarding_done && session.auto_advance);
        assert_eq!(session.theme.as_deref(), Some("nord.toml"));
        assert_eq!(session.skipped_version.as_deref(), Some("1.2.3"));
        assert_eq!(session.decoded_cache_size_gib(), 16);
        // Saved again, it is RAWmakase's.
        let saved = serde_json::to_value(&session).unwrap();
        assert_eq!(saved["demosaic"], "Rawmakase");
    }

    #[test]
    fn the_neural_engine_choice_round_trips_and_is_off_unless_chosen() {
        let old: Session = serde_json::from_str(r#"{"last_path":null,"monitor":null}"#).unwrap();
        assert!(!old.ai_denoise_neural_engine);
        let off = serde_json::to_string(&Session::default()).unwrap();
        assert!(!off.contains("ai_denoise_neural_engine"), "{off}");
        let on = Session {
            ai_denoise_neural_engine: true,
            ..Default::default()
        };
        let back: Session = serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
        assert!(back.ai_denoise_neural_engine);
        // A value another release wrote that isn't a boolean leaves it off.
        let odd: Session =
            serde_json::from_str(r#"{"last_path":null,"ai_denoise_neural_engine":"yes"}"#).unwrap();
        assert!(!odd.ai_denoise_neural_engine);
    }

    #[test]
    fn thumbnail_cache_size_round_trips_and_old_or_invalid_sizes_use_default() {
        let old: Session = serde_json::from_str(r#"{"last_path":null,"monitor":null}"#).unwrap();
        assert_eq!(old.thumbnail_cache_size_mib(), 512);
        for size in super::THUMBNAIL_CACHE_SIZES {
            let session = Session {
                thumbnail_cache_size_mib: Some(size),
                ..Default::default()
            };
            let back: Session =
                serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
            assert_eq!(back.thumbnail_cache_size_mib(), size);
        }
        // One a later release offers is kept, so that saving doesn't replace it.
        for size in [64, 3072, 32768] {
            let session = Session {
                thumbnail_cache_size_mib: Some(size),
                ..Default::default()
            };
            assert_eq!(session.thumbnail_cache_size_mib(), size);
        }
        for size in [0, 63, 1_048_577, u32::MAX] {
            let session = Session {
                thumbnail_cache_size_mib: Some(size),
                ..Default::default()
            };
            assert_eq!(session.thumbnail_cache_size_mib(), 512);
        }
        // A value of the wrong type reads as none, not as an unreadable session.
        let typed: Session = serde_json::from_str(
            r#"{"last_path":null,"monitor":null,"thumbnail_cache_size_mib":"big"}"#,
        )
        .unwrap();
        assert_eq!(typed.thumbnail_cache_size_mib(), 512);
    }

    #[test]
    fn decoded_cache_size_round_trips_and_old_or_invalid_sizes_use_default() {
        let old: Session = serde_json::from_str(r#"{"last_path":null,"monitor":null}"#).unwrap();
        assert_eq!(old.decoded_cache_size_gib(), 4);
        for size in [1, 32, 1024, 1025, 1_048_576] {
            let session = Session {
                decoded_cache_size_gib: Some(size),
                ..Default::default()
            };
            let back: Session =
                serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
            assert_eq!(back.decoded_cache_size_gib(), size);
        }
        for size in [0, 1_048_577, u32::MAX] {
            let session = Session {
                decoded_cache_size_gib: Some(size),
                ..Default::default()
            };
            assert_eq!(session.decoded_cache_size_gib(), 4);
        }
    }

    #[test]
    fn raw_defaults_survive_the_session_and_unreadable_ones_reset() {
        let mut settings = RawDefaults {
            camera_overrides: true,
            ..Default::default()
        };
        settings.set_camera("Canon EOS R5", DefaultChoice::Rawmakase);
        // A session from before raw defaults, or with ones that don't read, keeps
        // everything else.
        let old: Session =
            serde_json::from_str(r#"{"last_path":null,"monitor":null,"auto_advance":true}"#)
                .unwrap();
        assert_eq!(old.raw_defaults, RawDefaults::default());
        let odd: Session = serde_json::from_str(
            r#"{"last_path":null,"monitor":null,"auto_advance":true,"raw_defaults":7}"#,
        )
        .unwrap();
        assert!(odd.auto_advance);
        assert_eq!(odd.raw_defaults, RawDefaults::default());
        let session = Session {
            raw_defaults: settings.clone(),
            ..Default::default()
        };
        let json = serde_json::to_string(&session).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back.raw_defaults, settings);
    }
}

#[cfg(test)]
mod copy_groups_tests {
    use super::Session;
    use crate::model::settings_groups::{GroupInclusion, GroupSelection, SettingGroup};

    /// Releases before AI Denoise drop a whole session holding a group name they do not
    /// know: its group is saved apart, and names a later release adds are skipped.
    #[test]
    fn copy_groups_stay_readable_by_releases_before_ai_denoise() {
        for included in [true, false] {
            let mut groups = GroupSelection::default();
            let inclusion = if included {
                GroupInclusion::Included
            } else {
                GroupInclusion::Excluded
            };
            groups.set(SettingGroup::AiDenoise, inclusion);
            groups.set(SettingGroup::HighDynamicRange, inclusion);
            groups.set(SettingGroup::NegativeConversion, inclusion);
            let session = Session {
                copy_groups: Some(groups.clone()),
                copy_ai_denoise: Some(groups.contains(SettingGroup::AiDenoise)),
                copy_hdr: Some(groups.contains(SettingGroup::HighDynamicRange)),
                copy_negative: Some(groups.contains(SettingGroup::NegativeConversion)),
                ..Default::default()
            };
            let json = serde_json::to_value(&session).unwrap();
            let names = json["copy_groups"].as_array().unwrap();
            assert!(!names.iter().any(|n| n == "ai_denoise"), "{names:?}");
            assert!(
                !names.iter().any(|n| n == "high_dynamic_range"),
                "{names:?}"
            );
            assert!(
                !names.iter().any(|n| n == "negative_conversion"),
                "{names:?}"
            );
            assert_eq!(json["copy_negative"], included);
            assert!(names.iter().any(|n| n == "color_noise_reduction"));
            let read: Session = serde_json::from_value(json).unwrap();
            assert_eq!(read.copy_groups(), Some(groups));
        }
        // A session from before AI Denoise: included, as by default. One from a later
        // release with a group this one does not know: read without it.
        let older: Session =
            serde_json::from_str(r#"{"copy_groups":["exposure","some_later_group"]}"#).unwrap();
        let groups = older.copy_groups().unwrap();
        assert!(groups.contains(SettingGroup::Exposure));
        assert!(groups.contains(SettingGroup::AiDenoise));
        assert!(groups.contains(SettingGroup::HighDynamicRange));
        assert!(groups.contains(SettingGroup::NegativeConversion));
        assert!(!groups.contains(SettingGroup::Contrast));
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::Session;
    use crate::app::Editor;
    use crate::model::settings_groups::{GroupInclusion, SettingGroup};
    use serde::Deserialize;
    use serde_json::json;

    /// `Session` as v0.4.10, the last release before AI Denoise, read it: strictly,
    /// so a value it cannot read drops the whole session and resets every
    /// preference. The panels and raw defaults it read leniently are left out.
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct SessionV0_4_10 {
        last_path: Option<std::path::PathBuf>,
        monitor: Option<std::path::PathBuf>,
        #[serde(default)]
        monitor_srgb: bool,
        #[serde(default)]
        maximized: bool,
        #[serde(default)]
        collapsed: std::collections::BTreeSet<String>,
        #[serde(default)]
        solo: std::collections::BTreeSet<String>,
        #[serde(default)]
        onboarding_done: bool,
        #[serde(default)]
        library_source: String,
        #[serde(default)]
        selected_photo: Option<i64>,
        #[serde(default)]
        develop: bool,
        #[serde(default)]
        demosaic: DemosaicV0_4_10,
        #[serde(default)]
        no_update_checks: bool,
        #[serde(default)]
        skipped_version: Option<String>,
        #[serde(default)]
        theme: Option<String>,
        #[serde(default)]
        theme_chosen: bool,
        #[serde(default)]
        auto_advance: bool,
        #[serde(default)]
        no_auto_black_white_mix: bool,
        #[serde(default)]
        no_lensfun_suggestions: bool,
        #[serde(default)]
        pause_checksums: bool,
        #[serde(default)]
        library_layout: LibraryLayoutV0_4_10,
        #[serde(default)]
        copy_groups: Option<Vec<GroupV0_4_10>>,
        #[serde(default)]
        crop_guides: CropGuideLayoutV0_4_10,
        #[serde(default)]
        crop_aspect: String,
        #[serde(default)]
        standard_preview_size: Option<u32>,
        #[serde(default)]
        decoded_cache_size_gib: Option<u32>,
        #[serde(default)]
        decoded_cache_folder: Option<std::path::PathBuf>,
        #[serde(default)]
        one_to_one_discard_days: Option<u32>,
    }
    #[derive(Default, Deserialize)]
    enum DemosaicV0_4_10 {
        #[default]
        #[serde(alias = "Libraw")]
        Rawmakase,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    #[allow(dead_code)]
    struct LibraryLayoutV0_4_10 {
        view: String,
        sort: String,
        reverse: bool,
        cell_style: String,
        thumb_size: Option<f32>,
        query: String,
        flags: Vec<i32>,
        rating: Option<i32>,
        rating_op: String,
        labels: Vec<String>,
        kind: String,
        filters_off: bool,
        subfolders_hidden: bool,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    #[allow(dead_code)]
    struct CropGuideLayoutV0_4_10 {
        guide: String,
        orientation: u32,
        show: String,
    }
    /// The one field v0.3.0 to v0.4.7 read that later releases retired, as they
    /// read it.
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct PinnedV0_4_7 {
        #[serde(default)]
        pinned: std::collections::BTreeSet<String>,
    }
    /// v0.4.10's setting groups; any other name fails its whole session.
    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    #[allow(dead_code)]
    enum GroupV0_4_10 {
        WhiteBalance,
        Exposure,
        Contrast,
        Highlights,
        Shadows,
        Whites,
        Blacks,
        Texture,
        Clarity,
        Dehaze,
        Vibrance,
        Saturation,
        TreatmentAndProfile,
        ToneCurve,
        ColorAdjustments,
        BlackWhiteMix,
        ColorGrading,
        Sharpening,
        LuminanceNoiseReduction,
        ColorNoiseReduction,
        LensProfileCorrections,
        ChromaticAberration,
        LensVignetting,
        UprightMode,
        UprightTransforms,
        TransformAdjustments,
        PostCropVignetting,
        Grain,
        ProcessVersion,
        Calibration,
        SpotRemoval,
        Crop,
        Masking,
    }

    /// A session as a later release might write it: fields this release does not
    /// know, beside ones it does.
    fn later_release_session() -> serde_json::Value {
        json!({
            "last_path": null,
            "monitor": null,
            "onboarding_done": true,
            "auto_advance": true,
            "theme": "nord.toml",
            "copy_groups": ["exposure", "some_later_group"],
            "copy_ai_denoise": true,
            "decoded_cache_size_gib": 16,
            "a_later_preference": {"nested": [1, 2, 3], "on": true},
            "another_later_flag": "kept as written",
        })
    }

    /// Loads `session` into an editor, saves it again, and returns what was saved.
    fn saved_through_the_editor(session: serde_json::Value) -> serde_json::Value {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(&path, serde_json::to_vec(&session).unwrap()).unwrap();
        let session: Session = crate::storage::read_json_or_default(&path);
        let mut e = Editor::with_context(&eframe::egui::Context::default(), None, session, None);
        e.session.set_file(Some(path.clone()));
        e.save_session().unwrap();
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
    }

    /// Fields a later release wrote survive this one loading and saving the session,
    /// so going back to that release keeps its preferences.
    #[test]
    fn fields_this_release_does_not_know_survive_a_save() {
        let saved = saved_through_the_editor(later_release_session());
        assert_eq!(
            saved["a_later_preference"],
            json!({"nested": [1, 2, 3], "on": true})
        );
        assert_eq!(saved["another_later_flag"], "kept as written");
        // Known fields are still this release's.
        assert_eq!(saved["onboarding_done"], true);
        assert_eq!(saved["theme"], "nord.toml");
        assert_eq!(saved["decoded_cache_size_gib"], 16);
    }

    /// A field that does not read as this release expects resets alone: the rest of
    /// the session, and the fields it does not know, are kept.
    #[test]
    fn a_malformed_field_resets_only_itself() {
        let mut session = later_release_session();
        let fields = session.as_object_mut().unwrap();
        fields.insert("maximized".into(), json!("yes"));
        fields.insert("collapsed".into(), json!(7));
        fields.insert("library_layout".into(), json!([1, 2]));
        fields.insert("library_source".into(), json!({"folder": 1}));
        fields.insert("selected_photo".into(), json!("photo"));
        fields.insert("demosaic".into(), json!("Amaze"));
        fields.insert("copy_groups".into(), json!(3));
        fields.insert("copy_ai_denoise".into(), json!("no"));
        fields.insert("crop_guides".into(), json!(false));
        fields.insert("standard_preview_size".into(), json!(-1));
        fields.insert("decoded_cache_folder".into(), json!(42));
        fields.insert("last_path".into(), json!(["not", "a", "path"]));
        let read: Session = serde_json::from_value(session.clone()).unwrap();
        assert!(read.onboarding_done && read.auto_advance);
        assert_eq!(read.theme.as_deref(), Some("nord.toml"));
        assert_eq!(read.decoded_cache_size_gib(), 16);
        assert!(!read.maximized && read.collapsed.is_empty());
        assert_eq!(read.library_layout, Default::default());
        assert_eq!(read.copy_groups(), None);
        assert_eq!(read.last_path, None);
        assert_eq!(read.decoded_cache_folder, None);

        let saved = saved_through_the_editor(session);
        assert_eq!(saved["onboarding_done"], true);
        assert_eq!(saved["another_later_flag"], "kept as written");
        serde_json::from_value::<SessionV0_4_10>(saved).unwrap();
    }

    /// What this release saves stays readable by v0.4.10, which drops a whole session
    /// it cannot read: AI Denoise's group stays out of `copy_groups`, and fields from
    /// a later release are ones it skips.
    #[test]
    fn a_saved_session_stays_readable_by_the_release_before_ai_denoise() {
        for ai_denoise in [GroupInclusion::Included, GroupInclusion::Excluded] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("session.json");
            std::fs::write(&path, serde_json::to_vec(&later_release_session()).unwrap()).unwrap();
            let session: Session = crate::storage::read_json_or_default(&path);
            let mut e =
                Editor::with_context(&eframe::egui::Context::default(), None, session, None);
            e.session.set_file(Some(path.clone()));
            e.session
                .copy_groups
                .set(SettingGroup::AiDenoise, ai_denoise);
            e.session
                .copy_groups
                .set(SettingGroup::ColorNoiseReduction, GroupInclusion::Included);
            e.session.maximized = true;
            e.session.auto_advance = true;
            e.save_session().unwrap();
            let bytes = std::fs::read(&path).unwrap();
            let old: SessionV0_4_10 = serde_json::from_slice(&bytes).unwrap();
            assert!(old.maximized && old.auto_advance && old.onboarding_done);
            assert!(old.copy_groups.is_some_and(|g| !g.is_empty()));
            // And this release reads it back as it was.
            let back: Session = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(back.copy_groups(), Some(e.session.copy_groups.clone()));
        }
    }

    /// The Negative Conversion group, which no release before this one knows, never
    /// enters `copy_groups`: v0.4.10 reads the session strictly and would reset every
    /// preference over it, and the lenient releases after it would skip it anyway. It
    /// is saved apart, in `copy_negative`, a field older releases skip (and since
    /// v0.5.0 keep), so they lose at most that one checkbox. Synchronize's groups are
    /// written whole: every release that reads `sync_groups` skips names it doesn't
    /// know.
    #[test]
    fn a_saved_session_with_negative_conversion_stays_readable_by_older_releases() {
        for negative in [GroupInclusion::Included, GroupInclusion::Excluded] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("session.json");
            std::fs::write(&path, serde_json::to_vec(&later_release_session()).unwrap()).unwrap();
            let session: Session = crate::storage::read_json_or_default(&path);
            let mut e =
                Editor::with_context(&eframe::egui::Context::default(), None, session, None);
            e.session.set_file(Some(path.clone()));
            e.session
                .copy_groups
                .set(SettingGroup::NegativeConversion, negative);
            e.session
                .copy_groups
                .set(SettingGroup::Exposure, GroupInclusion::Included);
            let mut sync = crate::model::settings_groups::GroupSelection::none();
            sync.set(SettingGroup::NegativeConversion, GroupInclusion::Included);
            sync.set(SettingGroup::Exposure, GroupInclusion::Included);
            e.session.sync_groups = Some(sync.clone());
            if negative == GroupInclusion::Included {
                e.session.expanded.insert("Negative".into());
            }
            e.session.maximized = true;
            e.save_session().unwrap();
            let bytes = std::fs::read(&path).unwrap();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(
                !json["copy_groups"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|g| g == "negative_conversion"),
                "{json}"
            );
            assert_eq!(json["copy_negative"], negative == GroupInclusion::Included);
            assert_eq!(
                json.get("expanded").is_some(),
                negative == GroupInclusion::Included
            );
            let old: SessionV0_4_10 = serde_json::from_slice(&bytes).unwrap();
            assert!(old.maximized && old.onboarding_done);
            assert!(
                old.copy_groups
                    .is_some_and(|g| g.iter().any(|g| matches!(g, GroupV0_4_10::Exposure)))
            );
            // This release reads it back as it was, Synchronize's groups and expanded too.
            let back: Session = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(back.copy_groups(), Some(e.session.copy_groups.clone()));
            assert_eq!(back.sync_groups, Some(sync));
            assert_eq!(back.expanded, e.session.expanded);
        }
    }

    /// Pinned panels, retired in v0.4.8, are still read strictly by v0.3.0 to v0.4.7:
    /// kept when they read as those releases expect, dropped when they do not.
    #[test]
    fn a_retired_field_is_kept_only_as_older_releases_read_it() {
        let mut session = later_release_session();
        session["pinned"] = json!(["Histogram", "Navigator"]);
        let saved = saved_through_the_editor(session.clone());
        assert_eq!(saved["pinned"], json!(["Histogram", "Navigator"]));
        serde_json::from_value::<PinnedV0_4_7>(saved).unwrap();
        for malformed in [json!(7), json!([1, 2]), json!(null), json!({"a": "b"})] {
            session["pinned"] = malformed;
            let saved = saved_through_the_editor(session.clone());
            assert!(saved.get("pinned").is_none(), "{saved}");
            assert_eq!(saved["another_later_flag"], "kept as written");
            serde_json::from_value::<PinnedV0_4_7>(saved).unwrap();
        }
    }
}

#[cfg(test)]
#[path = "session_editor_tests.rs"]
mod editor_tests;

#[cfg(test)]
mod external_editing_tests {
    use super::Session;

    /// The external editors and the TIFF's color space survive a save; a
    /// session without them, or with a malformed list, reads as none and
    /// ProPhoto RGB, keeping its other preferences.
    #[test]
    fn external_editors_round_trip_and_old_sessions_read_none() {
        let session = Session {
            external_editors: vec!["/Applications/A.app".into(), "/usr/bin/gimp".into()],
            edit_in_color_space: Some("adobe-rgb".into()),
            ..Default::default()
        };
        let text = serde_json::to_string(&session).unwrap();
        let read: Session = serde_json::from_str(&text).unwrap();
        assert_eq!(read.external_editors, session.external_editors);
        assert_eq!(read.edit_in_color_space.as_deref(), Some("adobe-rgb"));

        let old: Session = serde_json::from_str(r#"{"auto_advance":true}"#).unwrap();
        assert!(old.external_editors.is_empty() && old.edit_in_color_space.is_none());
        let empty = serde_json::to_value(&old).unwrap();
        assert!(empty.get("external_editors").is_none());

        let malformed: Session =
            serde_json::from_str(r#"{"external_editors":7,"auto_advance":true}"#).unwrap();
        assert!(malformed.external_editors.is_empty() && malformed.auto_advance);
    }
}

#[cfg(test)]
mod launch_guard_tests {
    use super::LaunchGuard;

    #[test]
    fn fresh_launch_and_clean_exit_report_clean() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("session.json");
        let lock = LaunchGuard::path_for(&session);

        let (guard, unclean) = LaunchGuard::enter(&session);
        assert!(!unclean);
        // Check size via metadata while held: Windows LockFileEx blocks reads
        // through a second handle until the lock is released.
        assert!(std::fs::metadata(&lock).unwrap().len() > 0);
        guard.unwrap().mark_clean();
        assert!(std::fs::read(&lock).unwrap().is_empty());

        let (guard2, unclean2) = LaunchGuard::enter(&session);
        assert!(!unclean2);
        guard2.unwrap().mark_clean();
    }

    #[test]
    fn unclean_exit_is_detected_and_stays_dirty_until_a_clean_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("session.json");
        let lock = LaunchGuard::path_for(&session);

        let (guard1, unclean1) = LaunchGuard::enter(&session);
        assert!(!unclean1);
        drop(guard1);
        assert!(!std::fs::read(&lock).unwrap().is_empty());

        let (guard2, unclean2) = LaunchGuard::enter(&session);
        assert!(unclean2, "dropped without mark_clean must be detected");
        // A second crash before clean shutdown keeps the dirty marker.
        drop(guard2);

        let (guard3, unclean3) = LaunchGuard::enter(&session);
        assert!(unclean3);
        guard3.unwrap().mark_clean();

        let (guard4, unclean4) = LaunchGuard::enter(&session);
        assert!(!unclean4);
        guard4.unwrap().mark_clean();
    }

    #[test]
    fn a_concurrent_instance_fails_open_without_clearing_the_owners_guard() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("session.json");

        let (owner, unclean1) = LaunchGuard::enter(&session);
        assert!(!unclean1);
        assert!(owner.is_some());

        let (second, unclean2) = LaunchGuard::enter(&session);
        assert!(!unclean2, "a live owner is not a crash");
        assert!(second.is_none(), "second instance does not take ownership");
        drop(second);

        // If the owner later crashes, the next launch still detects it.
        drop(owner);
        let (third, unclean3) = LaunchGuard::enter(&session);
        assert!(unclean3);
        third.unwrap().mark_clean();
    }

    #[test]
    fn an_unwritable_path_fails_open_and_whitespace_counts_as_clean() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"").unwrap();
        let (guard, unclean) = LaunchGuard::enter(&blocker.join("session.json"));
        assert!(guard.is_none() && !unclean);

        let session = dir.path().join("session.json");
        std::fs::write(LaunchGuard::path_for(&session), b"   \n").unwrap();
        let (guard, unclean) = LaunchGuard::enter(&session);
        assert!(!unclean);
        guard.unwrap().mark_clean();
    }

    /// A duplicate descriptor sharing the same open file description (as a
    /// `fork()` before `exec()` creates on Unix) must not keep the `flock`
    /// alive after the guard drops.
    #[cfg(unix)]
    #[test]
    fn dropping_the_guard_unlocks_even_with_a_duplicated_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("session.json");

        let (guard1, unclean1) = LaunchGuard::enter(&session);
        assert!(!unclean1);
        let guard1 = guard1.unwrap();
        let inherited = guard1.file.try_clone().unwrap();
        drop(guard1);

        let (guard2, unclean2) = LaunchGuard::enter(&session);
        assert!(unclean2, "duplicated fd must not block the next launch");
        guard2.unwrap().mark_clean();
        drop(inherited);
    }
}
