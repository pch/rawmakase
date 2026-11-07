//! The desktop session: the last photo and catalog place, the panels and
//! layouts as the user left them, and the preferences kept between launches.
use crate::catalog::PhotoId;
use crate::storage::{data_dir, read_json_or_default};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// `Session::copy_groups` as releases before AI Denoise read it: without AI Denoise's
/// and High Dynamic Range's groups, which are saved apart.
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
                        !matches!(g, SettingGroup::AiDenoise | SettingGroup::HighDynamicRange)
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
    /// Titles of panel sections the user collapsed; all others start open.
    #[serde(default, deserialize_with = "lenient")]
    pub collapsed: std::collections::BTreeSet<String>,
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
    /// `copy_hdr`.
    #[serde(default, with = "known_groups")]
    pub copy_groups: Option<crate::model::settings_groups::GroupSelection>,
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
    /// The Copy Settings groups saved last time, with AI Denoise's and High Dynamic
    /// Range's (saved apart), which a session from before them includes, as they are
    /// by default.
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
    /// Fields a newer release wrote, carried to every write.
    unknown: Unknown,
    /// The path the session opened with, saved while no catalog or photo is open.
    fallback_path: Option<PathBuf>,
    /// The Library's place and layout, as last remembered.
    place: crate::app::CatalogPlace,
    layout: LibraryLayout,
    /// Whether the window is maximized, as last saved.
    pub(in crate::app) maximized: bool,
    /// The panel sections folded, and the ones in solo mode.
    pub(in crate::app) collapsed: std::collections::BTreeSet<String>,
    pub(in crate::app) solo: std::collections::BTreeSet<String>,
    pub(in crate::app) onboarding_done: bool,
    /// Preferences > Performance's demosaic, for every full-size decode.
    pub(in crate::app) demosaic: crate::camera_data::Demosaic,
    pub(in crate::app) auto_advance: bool,
    pub(in crate::app) checksums_paused: bool,
    /// The groups Copy Settings and Synchronize start from.
    pub(in crate::app) copy_groups: crate::model::settings_groups::GroupSelection,
    pub(in crate::app) first_conversion: crate::app::develop::treatment::FirstConversion,
}

impl SessionState {
    /// The state `session` was loaded as, written to `file`, with the editor's own
    /// starting values for the ones it works out at launch.
    pub(in crate::app) fn new(
        session: &Session,
        file: Option<PathBuf>,
        place: crate::app::CatalogPlace,
        onboarding_done: bool,
        copy_groups: crate::model::settings_groups::GroupSelection,
    ) -> Self {
        Self {
            file,
            unknown: session.unknown.clone(),
            fallback_path: session.last_path.clone(),
            place,
            layout: session.library_layout.clone(),
            maximized: session.maximized,
            collapsed: session.collapsed.clone(),
            solo: session.solo.clone(),
            onboarding_done,
            demosaic: session.demosaic,
            auto_advance: session.auto_advance,
            checksums_paused: session.pause_checksums,
            copy_groups,
            first_conversion: if session.no_auto_black_white_mix {
                crate::app::develop::treatment::FirstConversion::KeepMix
            } else {
                crate::app::develop::treatment::FirstConversion::AutoMix
            },
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
                copy_ai_denoise: Some(
                    self.copy_groups
                        .contains(crate::model::settings_groups::SettingGroup::AiDenoise),
                ),
                copy_hdr: Some(
                    self.copy_groups
                        .contains(crate::model::settings_groups::SettingGroup::HighDynamicRange),
                ),
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
    /// Colour names, "none" for no label and "other" for any other.
    pub labels: Vec<String>,
    pub kind: String,
    /// Cmd+L turned the filter bar off.
    pub filters_off: bool,
    /// Show Photos in Subfolders turned off.
    pub subfolders_hidden: bool,
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
            let session = Session {
                copy_groups: Some(groups.clone()),
                copy_ai_denoise: Some(groups.contains(SettingGroup::AiDenoise)),
                copy_hdr: Some(groups.contains(SettingGroup::HighDynamicRange)),
                ..Default::default()
            };
            let json = serde_json::to_value(&session).unwrap();
            let names = json["copy_groups"].as_array().unwrap();
            assert!(!names.iter().any(|n| n == "ai_denoise"), "{names:?}");
            assert!(
                !names.iter().any(|n| n == "high_dynamic_range"),
                "{names:?}"
            );
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
