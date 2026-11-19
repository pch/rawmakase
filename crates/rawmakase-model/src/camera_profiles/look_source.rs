//! A look's settings as Lightroom writes them with the look in a photo's settings or
//! a preset (#666), kept so RAWmakase can write a look that Lightroom and Camera Raw
//! render.
//!
//! Lightroom Classic writes a look as `<crs:Look>` with the look's own settings in
//! `<crs:Parameters>` (its `ConvertToGrayscale`, table ids, point curve and sliders),
//! and a user look's tables as `crs:Table_<md5>` attributes beside the photo's
//! settings. It leaves out the tables of Adobe's own looks, which Lightroom and
//! Camera Raw have built in. A `Look` that names a look without its settings renders
//! no look in Camera Raw 18.7, and Lightroom applies it as the named look with empty
//! settings.
//!
//! The parsed look ([`super::enhanced::Enhanced`]) can't be written back in that
//! form: its curve is sampled and its tables are decoded. So the settings are kept as
//! the look file has them, read when the library reads the file
//! (`LookFile::parse`), and stored once in a catalog beside the profiles
//! ([`crate::storage::recipe_assets`]). They are found again by the look's UUID and a
//! fingerprint of the parsed look ([`find`]), so an edit whose look file has changed
//! since never gets the new file's settings.
use super::{CameraProfile, enhanced::Enhanced};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, MutexGuard, Weak},
};

/// A look's settings as Lightroom writes them. See the module documentation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LookSource {
    /// The look's `crs:UUID`.
    pub uuid: String,
    /// [`fingerprint`] of the parsed look these settings are the source of.
    pub fingerprint: String,
    /// The `SupportsAmount`, `SupportsMonochrome` and `SupportsOutputReferred` flags
    /// the file has, which Lightroom writes on the `Look`.
    pub supports: Vec<(String, String)>,
    /// The look's `crs:` settings in file order, without its preset metadata and
    /// tables: what Lightroom writes in `Parameters`.
    pub parameters: Vec<(String, String)>,
    /// The look's point curves (`ToneCurvePV2012…`), as Lightroom's `"x, y"` points.
    pub curves: Vec<(String, Vec<String>)>,
    /// The tables `parameters` name, as `Table_<md5>` attributes. Empty for Adobe's
    /// looks, as Lightroom writes them.
    pub tables: Vec<(String, String)>,
}

/// Settings a look file has that are not the look's settings: preset metadata,
/// which Lightroom writes on the `Look` or not at all, and settings a photo's look
/// doesn't carry.
const METADATA: &[&str] = &[
    "PresetType",
    "Cluster",
    "UUID",
    "CameraModelRestriction",
    "Copyright",
    "ContactInfo",
    "HasSettings",
    "ShowInPresets",
    "ShowInQuickActions",
    "RequiresRGBTables",
];
/// The `Supports…` flags Lightroom writes on a photo's `Look`.
const SUPPORTS: &[&str] = &[
    "SupportsAmount",
    "SupportsMonochrome",
    "SupportsOutputReferred",
];

impl LookSource {
    /// The settings of the look file whose profile description is `d`, which parsed
    /// as `look`. `adobe` leaves its tables out, as Lightroom does for Adobe's looks.
    pub(super) fn read(d: roxmltree::Node, look: &Enhanced, adobe: bool) -> Self {
        use crate::xml::ns::{CRS, RDF};
        let mut source = Self {
            uuid: look.uuid.clone(),
            fingerprint: fingerprint(look),
            supports: Vec::new(),
            parameters: Vec::new(),
            curves: Vec::new(),
            tables: Vec::new(),
        };
        for a in d.attributes().filter(|a| a.namespace() == Some(CRS)) {
            let entry = (a.name().to_string(), a.value().to_string());
            if a.name().starts_with("Table_") {
                if !adobe {
                    source.tables.push(entry);
                }
            } else if SUPPORTS.contains(&a.name()) {
                source.supports.push(entry);
            } else if !METADATA.contains(&a.name()) && !a.name().starts_with("Supports") {
                source.parameters.push(entry);
            }
        }
        for n in d
            .children()
            .filter(|n| n.is_element() && n.tag_name().namespace() == Some(CRS))
            .filter(|n| n.tag_name().name().starts_with("ToneCurvePV2012"))
        {
            let points = n
                .descendants()
                .filter(|v| v.has_tag_name((RDF, "li")))
                .map(|v| v.text().unwrap_or("").trim().to_string())
                .collect();
            source
                .curves
                .push((n.tag_name().name().to_string(), points));
        }
        source
    }
    /// Whether these settings can be written as XMP: every name an XML name of
    /// Lightroom's kind, curves and tables named as Lightroom names them. A catalog's
    /// copy is checked before it is used, so a damaged or made-up one can't put
    /// markup into a written file. Values are escaped when written.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let name = |key: &str| {
            key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        let keys = self.supports.iter().map(|(k, _)| k.as_str());
        let keys = keys.chain(self.parameters.iter().map(|(k, _)| k.as_str()));
        let keys = keys.chain(self.curves.iter().map(|(k, _)| k.as_str()));
        let keys = keys.chain(self.tables.iter().map(|(k, _)| k.as_str()));
        if let Some(bad) = keys.clone().find(|k| !name(k)) {
            return Err(format!("its setting {bad:?} is not a setting's name"));
        }
        if self
            .supports
            .iter()
            .any(|(k, _)| !SUPPORTS.contains(&k.as_str()))
            || self
                .curves
                .iter()
                .any(|(k, _)| !k.starts_with("ToneCurvePV2012"))
            || self.tables.iter().any(|(k, _)| !k.starts_with("Table_"))
        {
            return Err("its flags, curves or tables are not named as Lightroom names them".into());
        }
        Ok(())
    }
    /// The value of the setting `key` in `parameters`, if the look has it.
    pub fn parameter(&self, key: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Whether a look file's copyright is Adobe's: its tables are built into Lightroom
/// and Camera Raw, which write the look without them.
pub(super) fn is_adobe(copyright: &str) -> bool {
    copyright.contains("Adobe Systems") || copyright.contains("Adobe Inc")
}

/// What identifies a parsed look whichever base profile it goes over: the SHA-256 of
/// its JSON without its base profile's name, in hex.
///
/// A catalog stores it with the settings and the profile's own JSON with the edit, so
/// a change to how [`Enhanced`] serializes changes it for looks stored before, whose
/// edits are then written with the look named only. `stored_settings_keep_their_fingerprint`
/// (`enhanced.rs`) pins it through a catalog's round trip for every kind of look.
pub(crate) fn fingerprint(look: &Enhanced) -> String {
    use sha2::{Digest, Sha256};
    let bare = Enhanced {
        base_name: String::new(),
        camera_look: None,
        ..look.clone()
    };
    let bytes = serde_json::to_vec(&bare).expect("a look serializes");
    Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Every look source this process has read, by UUID: the library's looks and those
/// the catalog stores. Held for the life of the process; there are as many as looks.
fn sources() -> MutexGuard<'static, HashMap<String, Vec<Arc<LookSource>>>> {
    static SOURCES: LazyLock<Mutex<HashMap<String, Vec<Arc<LookSource>>>>> =
        LazyLock::new(Default::default);
    SOURCES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Makes `source` one [`find`] returns, and returns the copy held: an earlier one with
/// the same settings when there is one. Settings read from a look file in the library
/// (`from_library`) come before ones a catalog stores for the same look, so a
/// catalog's copy never stands in for the file's.
pub(crate) fn register(source: Arc<LookSource>, from_library: bool) -> Arc<LookSource> {
    let mut all = sources();
    let same = all.entry(source.uuid.clone()).or_default();
    if let Some(held) = same.iter().find(|s| ***s == *source) {
        return held.clone();
    }
    if from_library {
        same.insert(0, source.clone());
    } else {
        same.push(source.clone());
    }
    source
}

/// The settings `profile`'s look was read with, if this process has them: a look in
/// the library, or one a catalog stores for an edit read from it. `None` for a
/// profile that is not a look, a fitted camera look, and a look whose file has
/// changed since the edit was made.
pub fn find(profile: &Arc<CameraProfile>) -> Option<Arc<LookSource>> {
    let look = profile.enhanced.as_ref()?;
    let candidates = sources().get(&look.uuid)?.clone();
    let fingerprint = memoized_fingerprint(profile, look);
    candidates
        .into_iter()
        .find(|s| s.fingerprint == fingerprint)
}

/// [`fingerprint`] of `profile`'s look, computed once per profile in memory: every
/// save of an edit looks its look's source up.
fn memoized_fingerprint(profile: &Arc<CameraProfile>, look: &Enhanced) -> String {
    type Memo = HashMap<usize, (Weak<CameraProfile>, String)>;
    static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Default::default);
    let key = Arc::as_ptr(profile) as usize;
    {
        let memo = MEMO.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((held, fingerprint)) = memo.get(&key)
            && held.upgrade().is_some_and(|p| Arc::ptr_eq(&p, profile))
        {
            return fingerprint.clone();
        }
    }
    let computed = fingerprint(look);
    let mut memo = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    if memo.len() >= 256 {
        memo.retain(|_, (held, _)| held.strong_count() > 0);
    }
    memo.insert(key, (Arc::downgrade(profile), computed.clone()));
    computed
}

/// Look files for tests of writing looks out.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::super::{CameraProfile, enhanced::LookFile, rgb_table::tests as rgb};
    use crate::xml::ns::{CRS, RDF};

    /// A creative look file as Lightroom's Create Profile writes one: an RGB table, a
    /// point curve, Lightroom's flags and settings, named `name` with `uuid`, under
    /// `copyright`. `curve` is its curve's middle point.
    pub fn look_xmp(name: &str, uuid: &str, copyright: &str, curve: &str) -> String {
        let table = rgb::encode(&rgb::table_bytes(5, rgb::fade, rgb::ADOBE));
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{RDF}"><r:Description xmlns:c="{CRS}" c:PresetType="Look" c:Cluster="" c:UUID="{uuid}" c:SupportsAmount="True" c:SupportsColor="True" c:SupportsMonochrome="True" c:SupportsHighDynamicRange="True" c:SupportsNormalDynamicRange="True" c:SupportsSceneReferred="True" c:SupportsOutputReferred="False" c:CameraModelRestriction="" c:Copyright="{copyright}" c:ContactInfo="" c:Version="18.7" c:ProcessVersion="15.4" c:ConvertToGrayscale="False" c:RGBTable="{uuid}" c:Table_{uuid}="{table}"><c:Name><r:Alt><r:li xml:lang="x-default">{name}</r:li></r:Alt></c:Name><c:ToneCurvePV2012><r:Seq><r:li>0, 0</r:li><r:li>{curve}</r:li><r:li>255, 255</r:li></r:Seq></c:ToneCurvePV2012></r:Description></r:RDF></x:xmpmeta>"#
        )
    }
    /// The look in `xml` over `base`, as the profile library makes it.
    pub fn look_profile(xml: &str, base: &CameraProfile) -> CameraProfile {
        LookFile::parse(xml)
            .expect("a test look parses")
            .compose(base)
            .expect("a test look goes over its base")
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};
    use crate::camera_data::Metadata;

    fn base() -> CameraProfile {
        CameraProfile::camera_matrix_default(&Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        })
        .unwrap()
    }

    /// The settings are kept as Lightroom writes them with a photo's look: its
    /// settings for `Parameters`, its flags for the `Look`, its table for beside the
    /// photo's settings, and no preset metadata.
    #[test]
    fn a_look_files_settings_are_kept_as_lightroom_writes_them() {
        let uuid = "6660000000000000000000000000000A";
        let p = look_profile(&look_xmp("Kept", uuid, "Someone", "128, 140"), &base());
        let source = find(&std::sync::Arc::new(p)).expect("the library's look is found");
        let keys: Vec<_> = source.parameters.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "Version",
                "ProcessVersion",
                "ConvertToGrayscale",
                "RGBTable"
            ]
        );
        assert_eq!(source.parameter("RGBTable"), Some(uuid));
        let flags: Vec<_> = source.supports.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(flags, SUPPORTS);
        assert_eq!(source.tables.len(), 1);
        assert_eq!(source.tables[0].0, format!("Table_{uuid}"));
        assert_eq!(
            source.curves,
            [(
                "ToneCurvePV2012".to_string(),
                vec!["0, 0".into(), "128, 140".into(), "255, 255".into()]
            )]
        );
        // Adobe's looks are written without their tables, as Lightroom does.
        let uuid = "6660000000000000000000000000000B";
        let adobe = look_profile(
            &look_xmp("Adobe's", uuid, "© 2018 Adobe Systems, Inc.", "128, 140"),
            &base(),
        );
        let source = find(&std::sync::Arc::new(adobe)).unwrap();
        assert!(source.tables.is_empty());
        assert_eq!(source.parameter("RGBTable"), Some(uuid));
    }

    /// An edit's look is found over any base and after a catalog stores its profile,
    /// but not once its file has changed under the same UUID.
    #[test]
    fn a_look_is_found_by_its_uuid_and_its_content() {
        use crate::storage::recipe_assets::{Asset, AssetKind};
        let uuid = "6660000000000000000000000000000C";
        let xml = look_xmp("Found", uuid, "Someone", "128, 150");
        let p = std::sync::Arc::new(look_profile(&xml, &base()));
        let source = find(&p).expect("the library's look is found");
        let mut other_base = base();
        other_base.name = "Other base".into();
        let over_other = std::sync::Arc::new(look_profile(&xml, &other_base));
        assert!(std::sync::Arc::ptr_eq(&find(&over_other).unwrap(), &source));
        let stored = Asset::profile(&p);
        let read =
            Asset::from_stored(&stored.id, AssetKind::Profile, &stored.compressed()).unwrap();
        let crate::storage::recipe_assets::AssetValue::Profile(read) = read.value else {
            panic!("a profile")
        };
        assert!(std::sync::Arc::ptr_eq(&find(&read).unwrap(), &source));
        // The same UUID with another curve: an edit made with the old file is not
        // written with the new one's settings, nor the other way round.
        let changed = look_profile(&look_xmp("Found", uuid, "Someone", "128, 100"), &base());
        let changed = std::sync::Arc::new(changed);
        let changed_source = find(&changed).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&changed_source, &source));
        assert!(std::sync::Arc::ptr_eq(&find(&p).unwrap(), &source));
        // Not a look, or a look this process never read: nothing.
        assert!(find(&std::sync::Arc::new(base())).is_none());
    }

    /// A catalog's copy is checked before it is used: a name that isn't a setting's
    /// would put markup into a written file. And a look file's own settings come
    /// before a catalog's copy for the same look.
    #[test]
    fn stored_settings_are_checked_and_the_librarys_come_first() {
        use crate::storage::recipe_assets::{Asset, AssetKind};
        let uuid = "6660000000000000000000000000000F";
        let p = std::sync::Arc::new(look_profile(
            &look_xmp("Checked", uuid, "Someone", "128, 165"),
            &base(),
        ));
        let source = find(&p).unwrap();
        let mut bad = (*source).clone();
        bad.parameters
            .push((r#"x"/><crs:Evil a=""#.into(), "1".into()));
        let asset = Asset::look_source(&std::sync::Arc::new(bad));
        assert!(Asset::from_stored(&asset.id, AssetKind::LookSource, &asset.compressed()).is_err());
        let mut curve = (*source).clone();
        curve.curves[0].0 = "Exposure2012".into();
        let asset = Asset::look_source(&std::sync::Arc::new(curve));
        assert!(Asset::from_stored(&asset.id, AssetKind::LookSource, &asset.compressed()).is_err());
        // A catalog's copy of a look (with other metadata) read before the library
        // reads the look's file: once it has, the file's settings are the ones found.
        let later = "66600000000000000000000000000012";
        let mut q = (*p).clone();
        q.enhanced.as_mut().unwrap().uuid = later.into();
        let catalogs = LookSource {
            uuid: later.into(),
            fingerprint: fingerprint(q.enhanced.as_ref().unwrap()),
            parameters: vec![("Version".into(), "1.0".into())],
            ..(*source).clone()
        };
        let asset = Asset::look_source(&std::sync::Arc::new(catalogs.clone()));
        Asset::from_stored(&asset.id, AssetKind::LookSource, &asset.compressed()).unwrap();
        let q = std::sync::Arc::new(q);
        assert_eq!(find(&q).as_deref(), Some(&catalogs));
        let file = look_xmp("Checked", later, "Someone", "128, 165")
            .replace(
                &format!(r#"c:RGBTable="{later}""#),
                &format!(r#"c:RGBTable="{uuid}""#),
            )
            .replace(&format!("c:Table_{later}="), &format!("c:Table_{uuid}="));
        let library = std::sync::Arc::new(look_profile(&file, &base()));
        let found = find(&library).unwrap();
        assert_ne!(*found, catalogs);
        assert!(std::sync::Arc::ptr_eq(&find(&q).unwrap(), &found));
    }

    /// Settings a catalog stores are found again once read, as in a new process.
    #[test]
    fn stored_settings_are_found_once_read() {
        use crate::storage::recipe_assets::{Asset, AssetKind};
        let uuid = "6660000000000000000000000000000D";
        let p = std::sync::Arc::new(look_profile(
            &look_xmp("Stored", uuid, "Someone", "128, 160"),
            &base(),
        ));
        let source = find(&p).unwrap();
        // The same settings under a UUID nothing has read yet, as a catalog would hold
        // them for an edit whose look is no longer in the library.
        let unread = std::sync::Arc::new(LookSource {
            uuid: "6660000000000000000000000000000E".into(),
            ..(*source).clone()
        });
        let mut q = (*p).clone();
        q.enhanced.as_mut().unwrap().uuid = unread.uuid.clone();
        let q = std::sync::Arc::new(q);
        assert!(find(&q).is_none());
        let unread = std::sync::Arc::new(LookSource {
            fingerprint: fingerprint(q.enhanced.as_ref().unwrap()),
            ..(*unread).clone()
        });
        let asset = Asset::look_source(&unread);
        Asset::from_stored(&asset.id, AssetKind::LookSource, &asset.compressed()).unwrap();
        assert_eq!(find(&q).as_deref(), Some(&*unread));
    }
}
