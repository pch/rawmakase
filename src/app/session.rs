//! The desktop session: the last photo and catalog place, the panels and
//! layouts as the user left them, and the preferences kept between launches.
use crate::catalog::PhotoId;
use crate::storage::{data_dir, read_json_or_default};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Session {
    pub last_path: Option<PathBuf>,
    /// Preferences > Monitor profile: a chosen ICC profile; none follows the
    /// system, or with `monitor_srgb` shows sRGB as it is.
    pub monitor: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub monitor_srgb: bool,
    /// The window was maximized when last seen; it opens maximized again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub maximized: bool,
    /// Titles of panel sections the user collapsed; all others start open.
    #[serde(default)]
    pub collapsed: std::collections::BTreeSet<String>,
    /// The sides of the window in Solo Mode, where opening a panel closes the
    /// others: "develop-left", "develop-right", "library-left", "library-right".
    #[serde(default)]
    pub solo: std::collections::BTreeSet<String>,
    /// Titles of panel sections the user pinned above the scrolling panels:
    /// "Navigator" and "Histogram" scroll with them until pinned.
    #[serde(default)]
    pub pinned: std::collections::BTreeSet<String>,
    /// The panels Develop and the Library show; all of them by default.
    #[serde(default, deserialize_with = "crate::app::panels::lenient")]
    pub panels: crate::app::panels::WorkspacePanels,
    /// The first-run setup was completed; until then it opens on launch.
    #[serde(default)]
    pub onboarding_done: bool,
    /// Where the user was in the last catalog: the Library folder, the
    /// selected photo, and whether Develop was open.
    #[serde(default)]
    pub library_source: String,
    #[serde(default)]
    pub selected_photo: Option<PhotoId>,
    #[serde(default)]
    pub develop: bool,
    /// Which demosaic full-size decodes use.
    #[serde(default)]
    pub demosaic: crate::camera_data::Demosaic,
    /// The user turned off checking for updates, which is on by default.
    #[serde(default)]
    pub no_update_checks: bool,
    /// A release the user chose to skip; newer ones are still offered.
    #[serde(default)]
    pub skipped_version: Option<String>,
    /// The interface's palette file; None is RAWmakase's own greys.
    #[serde(default)]
    pub theme: Option<String>,
    /// The user picked `theme`. Until then Omarchy's theme is followed on
    /// an Omarchy desktop, and RAWmakase's greys elsewhere.
    #[serde(default)]
    pub theme_chosen: bool,
    /// Photo > Auto Advance: a rating, flag or label moves to the next photo.
    #[serde(default)]
    pub auto_advance: bool,
    /// The user turned off applying the Auto mix when first converting to black &
    /// white, which is on by default.
    #[serde(default)]
    pub no_auto_black_white_mix: bool,
    /// Preferences > Catalog: the user paused reading checksums.
    #[serde(default)]
    pub pause_checksums: bool,
    /// How the Library showed its photos last time.
    #[serde(default)]
    pub library_layout: LibraryLayout,
    /// The groups Copy Settings copied last time; None until it has been used.
    #[serde(default)]
    pub copy_groups: Option<crate::model::settings_groups::GroupSelection>,
    /// The Crop tool's guide overlay.
    #[serde(default)]
    pub crop_guides: CropGuideLayout,
    /// The Crop tool's aspect last chosen from its menu, by stable name ("original",
    /// "free", "1x1", "4x5", …); an uncropped photo opens with it. A name this
    /// version does not know reads as Original.
    #[serde(default)]
    pub crop_aspect: String,
    /// Develop's backdrop around the photo, by stable name ("black", "dark-gray",
    /// …, "white"); empty, or a name this version does not know, is the theme's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub develop_backdrop: String,
    /// The border Develop leaves around the photo, in points; none, or one out of
    /// range, is the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub develop_border: Option<u32>,
    /// Preferences > Raw Defaults: what photos without an edit start from.
    #[serde(default, deserialize_with = "crate::raw_defaults::lenient_settings")]
    pub raw_defaults: crate::raw_defaults::RawDefaults,
    /// Preferences > Standard Preview Size, the long edge in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard_preview_size: Option<u32>,
    /// Preferences > Catalog: decoded-image disk cache limit in GiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoded_cache_size_gib: Option<u32>,
    /// Preferences > Performance: the folder chosen for decoded images; `None`
    /// keeps them in the per-user cache directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoded_cache_folder: Option<PathBuf>,
    /// Preferences > Automatically Discard 1:1 Previews, in days unused; 0 is
    /// Never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_to_one_discard_days: Option<u32>,
}
impl Session {
    pub(crate) fn decoded_cache_size_gib(&self) -> u32 {
        self.decoded_cache_size_gib
            .filter(|size| DECODED_CACHE_SIZE_RANGE.contains(size))
            .unwrap_or((crate::decode_cache::DEFAULT_LIMIT >> 30) as u32)
    }
    /// The Standard Preview Size, or the default when none of the offered
    /// sizes was saved.
    pub(crate) fn standard_preview_size(&self) -> u32 {
        self.standard_preview_size
            .filter(|size| crate::app::preview_build::STANDARD_SIZES.contains(size))
            .unwrap_or(crate::app::preview_build::DEFAULT_STANDARD_SIZE)
    }
    /// After how many days unused 1:1 previews are discarded; `None` keeps them.
    pub(crate) fn discard_one_to_one_after(&self) -> Option<u32> {
        use crate::app::preview_build::{DEFAULT_DISCARD_DAYS, DISCARD_CHOICES};
        match self.one_to_one_discard_days {
            Some(0) => None,
            Some(days) if DISCARD_CHOICES.iter().any(|(d, _)| *d == Some(days)) => Some(days),
            _ => DEFAULT_DISCARD_DAYS,
        }
    }
}

pub(super) const DECODED_CACHE_SIZE_RANGE: std::ops::RangeInclusive<u32> = 1..=1_048_576;

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
